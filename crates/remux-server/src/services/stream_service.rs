use crate::{
    AppContext, api, db,
    playback::probe::{probe_stream, resolve_stream_root},
    stream::StreamDescriptor,
};
use remux_sdks::{
    remux::{MediaStreamType, StreamFilter, VideoRangeType},
    remuxdb,
};
use tracing::debug;
use uuid::Uuid;

/// Result of probing a single stream candidate.
pub(crate) struct ProbeResult {
    /// Probed source info with id/name/path/remux already stamped.
    pub source: api::MediaSourceInfo,
    /// Original candidate stream (needed for RTSP check, subtitle extraction).
    pub stream: db::Media,
    /// Effective stream post-fallback (may differ from `stream` if probe failed over).
    pub effective_stream: db::Media,
}

/// Result of `StreamService::probe_candidates`.
pub(crate) struct ProbedStreams {
    pub results: Vec<ProbeResult>,
    /// True when the client named a specific stream — keep its UUID, don't override to item_id.
    pub specific_requested: bool,
}

pub(crate) struct StreamServiceConfig {
    pub ctx: AppContext,
    pub item_id: Uuid,
    pub requested_id: Option<Uuid>,
    pub show_ungrouped: bool,
    pub stream_filter: Option<StreamFilter>,
}

/// Central service for stream selection on a single playback request.
///
/// Construct with `new()`, then call `resolve()` to do all async work (group detection,
/// stream loading, policy filtering). After that the selection and ID-mapping methods
/// are available with no further parameters.
pub(crate) struct StreamService {
    ctx: AppContext,
    pub item_id: Uuid,
    pub requested_id: Option<Uuid>,
    show_ungrouped: bool,
    stream_filter: Option<StreamFilter>,
    // Populated by resolve()
    group: Option<(Uuid, String, Vec<db::Media>)>,
    stream: Option<db::Media>,
    pub streams: Vec<db::Media>,
}

impl StreamService {
    pub fn new(cfg: StreamServiceConfig) -> Self {
        Self {
            ctx: cfg.ctx,
            item_id: cfg.item_id,
            requested_id: cfg.requested_id,
            show_ungrouped: cfg.show_ungrouped,
            stream_filter: cfg.stream_filter,
            group: None,
            stream: None,
            streams: vec![],
        }
    }

    /// Load the service from a pre-fetched media item (playbackinfo path).
    ///
    /// Populates `self.group`, `self.stream`, and `self.streams`. Must be called
    /// before any of the selection or ID-mapping methods.
    pub async fn load(&mut self, media: db::Media) -> anyhow::Result<()> {
        if media.kind == db::MediaKind::StreamGroup {
            self.resolve_stream_group(media)
                .await?;
            return Ok(());
        }

        let mut root = resolve_stream_root(
            &media,
            self.item_id,
            &self
                .ctx
                .db,
        )
        .await;

        self.ctx
            .addons
            .refresh_streams(&mut root, &self.ctx)
            .await
            .inspect_err(|e| tracing::error!("refresh_streams failed: {e:#}"));

        let db_streams = root
            .streams(
                &self
                    .ctx
                    .db,
            )
            .await?;
        let raw = if db_streams.is_empty() {
            vec![root]
        } else {
            db_streams
        };

        let streams = db::StreamGroup::filter_sources(
            &self
                .ctx
                .db,
            raw,
            self.show_ungrouped,
        )
        .await;
        let streams = if let Some(sf) = self
            .stream_filter
            .as_ref()
            .filter(|sf| {
                !sf.rules
                    .is_empty()
            }) {
            let before = streams.len();
            let filtered = db::apply_stream_filter(sf, streams);
            debug!(
                streams_before = before,
                streams_after = filtered.len(),
                rules = sf
                    .rules
                    .len(),
                "stream filter applied"
            );
            filtered
        } else {
            debug!(
                has_filter = self
                    .stream_filter
                    .is_some(),
                "stream filter skipped"
            );
            streams
        };

        if streams.is_empty() {
            return Err(anyhow::anyhow!(
                "no playable sources for {} (filtered out by grouping or stream policy)",
                self.item_id
            ));
        }
        self.stream = streams
            .first()
            .cloned();
        self.streams = streams;
        Ok(())
    }

    /// One-shot lookup for handlers that only need a single resolved stream (subtitles, video).
    ///
    /// Handles StreamGroup → best candidate, device preference, and explicit stream UUID.
    /// Returns the concrete `db::Media` to stream.
    pub async fn lookup(
        ctx: &AppContext,
        item_id: Uuid,
        requested_id: Option<Uuid>,
        device_key: Option<&str>,
    ) -> anyhow::Result<db::Media> {
        let lookup_id = requested_id.unwrap_or(item_id);
        let media = db::Media::get_by_id(&ctx.db, &lookup_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("stream not found: {}", lookup_id))?;
        Self::dispatch_lookup(ctx, item_id, requested_id, device_key, media).await
    }

    async fn dispatch_lookup(
        ctx: &AppContext,
        item_id: Uuid,
        requested_id: Option<Uuid>,
        device_key: Option<&str>,
        media: db::Media,
    ) -> anyhow::Result<db::Media> {
        match media.kind {
            db::MediaKind::StreamGroup => {
                let gid = media.id;
                let mut candidates =
                    db::StreamGroup::streams_for(&ctx.db, &gid, &item_id).await?;
                if candidates.is_empty() {
                    return Err(anyhow::anyhow!(
                        "no streams available for group {}",
                        gid
                    ));
                }
                let cascade =
                    db::StreamGroup::streams_for_groups_after(&ctx.db, &gid, &item_id)
                        .await
                        .unwrap_or_default();
                candidates.extend(cascade);
                Ok(candidates.remove(0))
            }
            db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track => {
                let mut media = media;
                let sources = media
                    .streams(&ctx.db)
                    .await?;
                if let Some(sid) = requested_id.filter(|&sid| sid != item_id) {
                    sources
                        .into_iter()
                        .find(|s| s.id == sid)
                        .ok_or_else(|| anyhow::anyhow!("stream not found: {}", sid))
                } else if let Some(key) = device_key {
                    let saved = ctx
                        .store
                        .get::<Uuid>(&format!("pstream:{}:{}", item_id, key));
                    let by_pref = saved.and_then(|sid| {
                        sources
                            .iter()
                            .find(|s| s.id == sid)
                            .cloned()
                    });
                    by_pref
                        .or_else(|| {
                            sources
                                .into_iter()
                                .next()
                        })
                        .ok_or_else(|| {
                            anyhow::anyhow!("no playable sources for {}", item_id)
                        })
                } else {
                    sources
                        .into_iter()
                        .next()
                        .ok_or_else(|| {
                            anyhow::anyhow!("no playable sources for {}", item_id)
                        })
                }
            }
            _ => Ok(media),
        }
    }

    async fn resolve_stream_group(&mut self, media: db::Media) -> anyhow::Result<()> {
        let gid = media.id;
        let gtitle = media
            .title
            .clone();
        let mut candidates = db::StreamGroup::streams_for(
            &self
                .ctx
                .db,
            &gid,
            &self.item_id,
        )
        .await?;
        if candidates.is_empty() {
            return Err(anyhow::anyhow!("no streams available for group {}", gid));
        }
        let cascade = db::StreamGroup::streams_for_groups_after(
            &self
                .ctx
                .db,
            &gid,
            &self.item_id,
        )
        .await
        .unwrap_or_default();
        candidates.extend(cascade);
        self.stream = Some(candidates[0].clone());
        self.group = Some((gid, gtitle, candidates));
        Ok(())
    }

    /// The concrete resolved stream. Panics if called before `resolve()`.
    pub fn candidate(&self) -> &db::Media {
        self.stream
            .as_ref()
            .expect("StreamService::load() must be called first")
    }

    /// The StreamGroup context, if the request was for a group.
    pub fn group(&self) -> Option<&(Uuid, String, Vec<db::Media>)> {
        self.group
            .as_ref()
    }

    /// UUID the client should see in `MediaSources[0].Id` and `TranscodingUrl MediaSourceId`.
    pub fn client_facing_id(&self) -> Uuid {
        self.group
            .as_ref()
            .map(|(gid, _, _)| *gid)
            .unwrap_or_else(|| {
                self.candidate()
                    .id
            })
    }

    /// UUID for `MediaSources[idx].Id`, using the probe-fallback effective stream.
    pub fn source_id_for(&self, effective: &db::Media) -> Uuid {
        self.group
            .as_ref()
            .map(|(gid, _, _)| *gid)
            .unwrap_or(effective.id)
    }

    /// Display name for `MediaSources[idx].Name`.
    pub fn source_name_for(&self, effective: &db::Media) -> String {
        self.group
            .as_ref()
            .map(|(_, t, _)| t.clone())
            .unwrap_or_else(|| {
                effective
                    .title
                    .clone()
            })
    }

    fn candidates(&self) -> &[db::Media] {
        self.group
            .as_ref()
            .map(|(_, _, c)| c.as_slice())
            .unwrap_or(&[])
    }

    /// Partition `self.streams` into candidate/probe lists and compute selection flags.
    pub(crate) fn select_streams(&self) -> StreamSelection {
        let all_streams = self
            .streams
            .clone();
        let item_id = self.item_id;
        let requested_id = self.requested_id;

        let specific_requested = self
            .group
            .is_some()
            || requested_id
                .map(|sid| {
                    sid != item_id
                        && all_streams
                            .iter()
                            .any(|s| s.id == sid)
                })
                .unwrap_or(false);

        if self
            .group
            .is_some()
        {
            return StreamSelection {
                candidates: vec![
                    self.candidate()
                        .clone(),
                ],
                probe_pool: self
                    .candidates()
                    .to_vec(),
                restrict_resolution: false,
                probe_only_first: false,
                specific_requested: true,
            };
        }

        let probe_pool = all_streams.clone();

        let (candidates, probe_only_first) = if specific_requested {
            let sid = requested_id.unwrap();
            (
                all_streams
                    .into_iter()
                    .filter(|s| s.id == sid)
                    .collect(),
                false,
            )
        } else if requested_id.is_some() {
            // media_source_id == item_id (Android TV auto-play) or stream not found:
            // return only the first stream; specific_requested stays false so
            // source[0].id is overridden to item_id below (required for Android TV routing).
            let mut v = all_streams;
            v.truncate(1);
            (v, false)
        } else {
            // No stream ID: return all versions for the selection UI,
            // probe only the first to avoid spawning N FFmpeg processes.
            (all_streams, true)
        };

        StreamSelection {
            candidates,
            probe_pool,
            restrict_resolution: true,
            probe_only_first,
            specific_requested,
        }
    }

    /// Probe all stream candidates and return stamped results.
    ///
    /// Internally calls `select_streams()`, loads probe config, then invokes `probe_stream`
    /// for each candidate. Source ID/name/path/remux are stamped before returning so the
    /// handler only deals with playback-decision work.
    pub async fn probe_candidates(&self) -> anyhow::Result<ProbedStreams> {
        let sel = self.select_streams();
        let probe_cfg = db::Settings::get_config_or_default(
            &self
                .ctx
                .db,
        )
        .await;
        let timeout = probe_cfg
            .probe_timeout_secs
            .unwrap_or(20) as u64;
        let timeout_p2p = probe_cfg
            .probe_timeout_p2p_secs
            .unwrap_or(60) as u64;
        let auto_next = probe_cfg
            .auto_next_stream_on_probe_fail
            .unwrap_or(true);
        let max_retries = probe_cfg
            .max_probe_fallback_streams
            .unwrap_or(3) as usize;
        let port = self
            .ctx
            .config
            .port;
        let item = db::Media::get_by_id(
            &self
                .ctx
                .db,
            &self.item_id,
        )
        .await
        .ok()
        .flatten();

        let mut results = Vec::with_capacity(
            sel.candidates
                .len(),
        );
        for (idx, stream) in sel
            .candidates
            .into_iter()
            .enumerate()
        {
            let url_opt = stream
                .stream_info
                .as_ref()
                .map(|si| {
                    si.descriptor
                        .server_input(stream.id, port)
                });
            let skip_probe = sel.probe_only_first && idx > 0;
            let was_cached = stream
                .probe_data
                .as_ref()
                .and_then(|pd| pd.video_stream())
                .is_some();
            let timeout_secs = if stream
                .stream_info
                .as_ref()
                .map_or(false, |si| si.is_p2p())
            {
                timeout_p2p
            } else {
                timeout
            };
            let (mut source, effective_stream) = probe_stream(
                &stream,
                url_opt,
                skip_probe,
                timeout_secs,
                auto_next,
                max_retries,
                &sel.probe_pool,
                sel.restrict_resolution,
                port,
                &self
                    .ctx
                    .db,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;

            let cid = self.source_id_for(&effective_stream);
            source.id = cid;
            source.e_tag = cid;
            source.name = Some(self.source_name_for(&effective_stream));
            source.has_segments = true;
            source.path = Some(format!("/remux/{}", effective_stream.id));
            source.is_remote = false;
            // Re-apply binge-group headers — ffmpeg probing produces a fresh
            // MediaSourceInfo and would otherwise drop provider hints.
            source.remux = Some(api::MediaSourceRemuxInfo {
                provider_info: stream
                    .stream_info
                    .as_ref()
                    .and_then(|si| serde_json::to_value(si).ok()),
            });

            let remuxdb_enabled = probe_cfg
                .remuxdb_enabled
                .unwrap_or(true);
            if was_cached {
                debug!(id = %effective_stream.id, "remuxdb: skipping (probe cache hit)");
            } else if !remuxdb_enabled {
                debug!(id = %effective_stream.id, "remuxdb: skipping (disabled)");
            } else if let Some(url) = self
                .ctx
                .config
                .remuxdb_url
                .clone()
            {
                match media_info_from_probe(&source, &effective_stream, item.as_ref()) {
                    Some(mi) => {
                        debug!(id = %effective_stream.id, url, "remuxdb: submitting mediainfo");
                        let token = probe_cfg
                            .remuxdb_token
                            .clone();
                        tokio::spawn(mi.submit(url, token));
                    }
                    None => {
                        debug!(id = %effective_stream.id, "remuxdb: skipping (no stream_info or missing required fields)");
                    }
                }
            }

            results.push(ProbeResult {
                source,
                stream,
                effective_stream,
            });
        }

        Ok(ProbedStreams {
            results,
            specific_requested: sel.specific_requested,
        })
    }

    /// Persist the resolved stream UUID in the device-preference store (24 h TTL).
    /// No-op when this was not a group request.
    pub fn save_preference(&self, device_key: &str) {
        if self
            .group
            .is_none()
        {
            return;
        }
        self.ctx
            .store
            .save(
                format!("pstream:{}:{}", self.item_id, device_key),
                self.candidate()
                    .id,
                std::time::Duration::from_secs(24 * 3600),
            );
    }
}

/// Result of `StreamService::select_streams` — partitioned candidate/probe lists and flags.
pub(crate) struct StreamSelection {
    /// Streams to present to the client and probe.
    pub candidates: Vec<db::Media>,
    /// Full pool used for probe-fallback across sibling streams.
    pub probe_pool: Vec<db::Media>,
    /// When false (group requests), cross-resolution fallback is intentional.
    pub restrict_resolution: bool,
    /// Probe only the first candidate to avoid N parallel FFmpeg processes.
    pub probe_only_first: bool,
    /// True when the client named a specific stream — keep its UUID, don't override to item_id.
    pub specific_requested: bool,
}

fn media_info_from_probe(
    probe: &api::MediaSourceInfo,
    stream: &db::Media,
    item: Option<&db::Media>,
) -> Option<remuxdb::MediaInfo> {
    let (info_hash, file_idx, filename) = match stream
        .stream_info
        .as_ref()
    {
        Some(si) => {
            let (hash, idx) = match &si.descriptor {
                StreamDescriptor::Torrent {
                    info_hash,
                    file_idx,
                    ..
                } => (Some(info_hash.clone()), file_idx.map(|i| i as i32)),
                _ => (None, None),
            };
            (
                hash,
                idx,
                si.filename
                    .clone()
                    .unwrap_or_else(|| {
                        stream
                            .title
                            .clone()
                    }),
            )
        }
        None => (
            None,
            None,
            stream
                .title
                .clone(),
        ),
    };

    let (kind, external_ids, season, episode) = if let Some(item) = item {
        let kind = match item.kind {
            db::MediaKind::Episode => "episode",
            _ => "movie",
        }
        .to_string();
        let imdb_id = item
            .external_ids
            .imdb
            .as_ref()
            .or(item
                .external_ids
                .series_imdb
                .as_ref())
            .map(|v| v.to_string());
        let ids = (imdb_id.is_some()
            || item
                .external_ids
                .tmdb
                .is_some()
            || item
                .external_ids
                .tvdb
                .is_some())
        .then(|| remuxdb::ExternalIds {
            imdb_id,
            tmdb_id: item
                .external_ids
                .tmdb,
            tvdb_id: item
                .external_ids
                .tvdb,
        });
        let season = if item.kind == db::MediaKind::Episode {
            item.parent_idx
                .map(|v| v as i32)
        } else {
            None
        };
        let episode = if item.kind == db::MediaKind::Episode {
            item.idx
                .map(|v| v as i32)
        } else {
            None
        };
        (kind, ids, season, episode)
    } else {
        ("movie".to_string(), None, None, None)
    };

    let tracks = probe
        .media_streams
        .iter()
        .filter_map(|ms| match ms.type_? {
            MediaStreamType::Video => {
                Some(remuxdb::TrackPayload::Video(remuxdb::VideoTrackPayload {
                    idx: ms.index as i32,
                    codec: ms
                        .codec
                        .clone()
                        .unwrap_or_default(),
                    width: ms
                        .width
                        .unwrap_or(0) as i32,
                    height: ms
                        .height
                        .unwrap_or(0) as i32,
                    fps: ms
                        .real_frame_rate
                        .map(|f| f as f64),
                    avg_fps: ms
                        .average_frame_rate
                        .map(|f| f as f64),
                    bit_rate: ms.bit_rate,
                    bit_depth: ms
                        .bit_depth
                        .map(|d| d as i32),
                    profile: ms
                        .profile
                        .clone(),
                    codec_tag: ms
                        .codec_tag
                        .clone(),
                    comment: ms
                        .comment
                        .clone(),
                    title: ms
                        .title
                        .clone(),
                    language: ms
                        .language
                        .clone(),
                    color_primaries: ms
                        .color_primaries
                        .clone(),
                    color_range: ms
                        .color_range
                        .clone(),
                    color_space: ms
                        .color_space
                        .clone(),
                    color_transfer: ms
                        .color_transfer
                        .clone(),
                    aspect_ratio: ms
                        .aspect_ratio
                        .clone(),
                    rotation: ms
                        .rotation
                        .map(|r| r as i32),
                    is_default: ms
                        .is_default
                        .unwrap_or(false),
                    is_forced: ms.is_forced,
                    is_external: ms.is_external,
                    is_hearing_impaired: ms.is_hearing_impaired,
                    is_interlaced: ms.is_interlaced,
                    hdr10_plus_present: matches!(
                        ms.video_range_type,
                        Some(VideoRangeType::Hdr10Plus)
                    ),
                    dv_profile: ms
                        .dv_profile
                        .map(|v| v as i32),
                    dv_level: ms
                        .dv_level
                        .map(|v| v as i32),
                    dv_version_major: ms
                        .dv_version_major
                        .map(|v| v as i32),
                    dv_version_minor: ms
                        .dv_version_minor
                        .map(|v| v as i32),
                    dv_bl_signal_compat_id: ms
                        .dv_bl_signal_compatibility_id
                        .map(|v| v as i32),
                    dv_rpu_present: ms
                        .rpu_present_flag
                        .map_or(false, |v| v != 0),
                    dv_bl_present: ms
                        .bl_present_flag
                        .map_or(false, |v| v != 0),
                    dv_el_present: ms
                        .el_present_flag
                        .map_or(false, |v| v != 0),
                }))
            }
            MediaStreamType::Audio => {
                Some(remuxdb::TrackPayload::Audio(remuxdb::AudioTrackPayload {
                    idx: ms.index as i32,
                    codec: ms
                        .codec
                        .clone()
                        .unwrap_or_default(),
                    channels: ms
                        .channels
                        .unwrap_or(0) as i32,
                    sample_rate: ms
                        .sample_rate
                        .unwrap_or(0) as i32,
                    bit_rate: ms.bit_rate,
                    bit_depth: ms
                        .bit_depth
                        .map(|d| d as i32),
                    channel_layout: ms
                        .channel_layout
                        .clone(),
                    profile: ms
                        .profile
                        .clone(),
                    codec_tag: ms
                        .codec_tag
                        .clone(),
                    comment: ms
                        .comment
                        .clone(),
                    title: ms
                        .title
                        .clone(),
                    language: ms
                        .language
                        .clone(),
                    is_default: ms
                        .is_default
                        .unwrap_or(false),
                    is_forced: ms.is_forced,
                    is_external: ms.is_external,
                    is_hearing_impaired: ms.is_hearing_impaired,
                }))
            }
            MediaStreamType::Subtitle => Some(remuxdb::TrackPayload::Subtitle(
                remuxdb::SubtitleTrackPayload {
                    idx: ms.index as i32,
                    codec: ms
                        .codec
                        .clone(),
                    title: ms
                        .title
                        .clone(),
                    language: ms
                        .language
                        .clone(),
                    comment: ms
                        .comment
                        .clone(),
                    is_default: ms
                        .is_default
                        .unwrap_or(false),
                    is_forced: ms.is_forced,
                    is_external: ms.is_external,
                    is_hearing_impaired: ms.is_hearing_impaired,
                },
            )),
            _ => None,
        })
        .collect();

    Some(remuxdb::MediaInfo {
        client_id: crate::common::server_id(),
        kind,
        filename,
        torrent_info_hash: info_hash,
        torrent_file_idx: file_idx,
        container: probe
            .container
            .clone()
            .unwrap_or_default(),
        size: probe
            .size
            .or_else(|| {
                stream
                    .stream_info
                    .as_ref()
                    .and_then(|si| si.size)
            })
            .filter(|&s| s > 0)?,
        duration: crate::common::ticks_to_seconds(
            probe
                .run_time_ticks
                .unwrap_or(0),
        ),
        bitrate: probe.bitrate,
        season,
        episode,
        external_ids,
        tracks,
    })
}
