use crate::{
    AppContext, api, db,
    db::PreProbeQualityExt,
    device_profile::SourceRankingContext,
    playback::probe::{ProbeDataExt, probe_stream, resolve_stream_root},
};
use remux_sdks::{
    remux::{MediaStreamType, StreamFilter, VideoRangeType},
    remuxdb,
};
use tracing::{debug, warn};
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
    pub user_id: Option<Uuid>,
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
    user_id: Option<Uuid>,
    // Populated by resolve()
    group: Option<(Uuid, String, Vec<db::Media>)>,
    stream: Option<db::Media>,
    pub streams: Vec<db::Media>,
}

fn quality_ordered_probe_pool(streams: &[db::Media]) -> Vec<db::Media> {
    let mut pool = streams.to_vec();
    pool.sort_by_cached_key(|stream| std::cmp::Reverse(stream.quality_weight()));
    pool
}

impl StreamService {
    /// Capability-ranks `items` best-first. `describe` yields each item's
    /// `MediaSourceInfo` and whether on-demand subtitle extraction is
    /// feasible for it (only ever true for a local source) — the one place
    /// that per-source flag is applied, so every caller ranks the same way.
    /// Stable, so `SortMediaSourcesMode::Disabled` keeps the input order.
    pub fn rank_sources<T>(
        items: &mut [T],
        ranking: crate::device_profile::SourceRankingContext<'_>,
        describe: impl Fn(&T) -> (api::MediaSourceInfo, bool),
    ) {
        items.sort_by_cached_key(|item| {
            let (info, allow_subtitle_extraction) = describe(item);
            std::cmp::Reverse(ranking.sort_key(&info, allow_subtitle_extraction))
        });
    }

    pub fn new(cfg: StreamServiceConfig) -> Self {
        Self {
            ctx: cfg.ctx,
            item_id: cfg.item_id,
            requested_id: cfg.requested_id,
            show_ungrouped: cfg.show_ungrouped,
            stream_filter: cfg.stream_filter,
            user_id: cfg.user_id,
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
            if let Ok(Some(mut parent)) = db::Media::get_by_id(
                &self
                    .ctx
                    .db,
                &self.item_id,
            )
            .await
            {
                self.ctx
                    .addons
                    .refresh_streams(&mut parent, &self.ctx, self.user_id)
                    .await
                    .inspect_err(|e| tracing::error!("refresh_streams failed: {e:#}"));
            }
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
            .refresh_streams(&mut root, &self.ctx, self.user_id)
            .await
            .inspect_err(|e| tracing::error!("refresh_streams failed: {e:#}"));

        let root_kind = root
            .kind
            .clone();
        let db_streams = root
            .streams(
                &self
                    .ctx
                    .db,
            )
            .await?;
        let raw = if db_streams.is_empty() {
            // Root item can be the stream itself (e.g. locally-imported files)
            // but only when it carries a URL. Addon content uses the root as a
            // container — falling back to it when the addon returned no streams
            // would queue a probe against an item with no stream_info.
            if root
                .stream_info
                .is_some()
            {
                vec![root]
            } else {
                vec![]
            }
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
            })
            .filter(|_| {
                matches!(root_kind, db::MediaKind::Movie | db::MediaKind::Episode)
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
            return Ok(());
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
        user_id: Option<Uuid>,
    ) -> anyhow::Result<db::Media> {
        let lookup_id = requested_id.unwrap_or(item_id);
        // Resolve the id the way PlaybackInfo does: `resolve_item` is
        // `get_by_id` plus the synthetic-id path (search/catalog ids that have
        // no row yet), so a client playing such an item reaches its streams.
        let media = crate::services::MediaResolveService::resolve_item(lookup_id, ctx)
            .await?
            .ok_or_else(|| anyhow::anyhow!("stream not found: {}", lookup_id))?;
        Self::dispatch_lookup(ctx, item_id, requested_id, device_key, user_id, media)
            .await
    }

    async fn dispatch_lookup(
        ctx: &AppContext,
        item_id: Uuid,
        requested_id: Option<Uuid>,
        device_key: Option<&str>,
        user_id: Option<Uuid>,
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
            db::MediaKind::Movie
            | db::MediaKind::Episode
            | db::MediaKind::Track
            | db::MediaKind::TvChannel
            | db::MediaKind::TvProgram => {
                let mut media = media;
                let media_id = media.id;
                let _ = ctx
                    .addons
                    .refresh_streams(&mut media, ctx, user_id)
                    .await
                    .inspect_err(|e| tracing::error!("refresh_streams failed: {e:#}"));
                let sources = media
                    .streams(&ctx.db)
                    .await?;
                // Here `requested_id` resolved to a Movie/Episode/Track, so it
                // is an *item* id (auto-play, or the PlaybackInfo rewrite of
                // source[0].Id — the sibling item's UUID when duplicate items
                // share one IMDB id). An item's id is never one of its stream
                // ids, so matching it against `sources` can only fail; treat it
                // as auto-play and fall through to preference / first source.
                let specific_stream =
                    requested_id.filter(|&sid| sid != item_id && sid != media_id);
                if let Some(sid) = specific_stream {
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
                            .find(|s| s.id == *sid)
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
                specific_requested: true,
            };
        }

        let mut probe_pool = all_streams.clone();

        let candidates = if specific_requested {
            let sid = requested_id.unwrap();
            all_streams
                .into_iter()
                .filter(|s| s.id == sid)
                .collect()
        } else {
            // No specific stream requested — either no ID at all, or
            // media_source_id == item_id (Android TV auto-play sends this
            // instead of omitting the field; specific_requested is already
            // false for it, so source[0].id still gets overridden to item_id
            // by the caller). Both cases mean the same thing: let capability
            // ranking pick the best version, not just whichever happened to
            // be first in DB order — that used to be special-cased to
            // truncate to `all_streams[0]` unranked, which raced ahead of
            // playback.rs's post-probe SourceRankingContext sort by leaving
            // it nothing else to rank.
            //
            // Return all versions for the selection UI. `probe_candidates`
            // real-probes the one PlaybackInfo will list first, since that
            // is the one auto-play plays. Probe fallback tries siblings best
            // filename quality first.
            probe_pool = quality_ordered_probe_pool(&all_streams);
            all_streams
        };

        StreamSelection {
            candidates,
            probe_pool,
            restrict_resolution: true,
            specific_requested,
        }
    }

    /// Puts `candidates` in the order PlaybackInfo lists them: addon order
    /// without `ranking` (or for stream-group representatives, whose order is
    /// admin-authored), else the capability ranking, run on what is known
    /// without probing (cached probe data or a filename guess).
    async fn rank_candidates(
        &self,
        candidates: Vec<db::Media>,
        ranking: Option<SourceRankingContext<'_>>,
    ) -> Vec<db::Media> {
        let ranking = ranking.filter(|_| {
            candidates
                .iter()
                .all(|s| {
                    s.group_id
                        .is_none()
                })
        });
        let Some(ranking) = ranking else {
            return candidates;
        };
        let mut described = Vec::with_capacity(candidates.len());
        for stream in candidates {
            let mut info = api::MediaSourceInfo::from(stream.clone());
            crate::conversions::apply_filename_guess(&mut info, &stream);
            let allow_subtitle_extraction = stream
                .allows_subtitle_extraction(
                    &self
                        .ctx
                        .db,
                )
                .await;
            described.push((stream, info, allow_subtitle_extraction));
        }
        Self::rank_sources(&mut described, ranking, |(_, info, allow)| {
            (info.clone(), *allow)
        });
        described
            .into_iter()
            .map(|(stream, ..)| stream)
            .collect()
    }

    /// Probe all stream candidates and return stamped results.
    ///
    /// Internally calls `select_streams()`, loads probe config, then invokes `probe_stream`
    /// for each candidate. Results come back in the order PlaybackInfo lists
    /// them (see `rank_candidates`; `ranking` is `None` for addon order).
    /// Without a specific request only the first one, the source auto-play
    /// plays, gets a real probe. Source ID/name/path/remux are stamped before
    /// returning so the handler only deals with playback-decision work.
    pub async fn probe_candidates(
        &self,
        ranking: Option<SourceRankingContext<'_>>,
    ) -> anyhow::Result<ProbedStreams> {
        let mut sel = self.select_streams();
        sel.candidates = self
            .rank_candidates(std::mem::take(&mut sel.candidates), ranking)
            .await;
        let preferred_probe_id = if sel.specific_requested {
            None
        } else {
            sel.candidates
                .first()
                .map(|s| s.id)
        };
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
        let mut item = db::Media::get_by_id(
            &self
                .ctx
                .db,
            &self.item_id,
        )
        .await
        .ok()
        .flatten();
        if let Some(ref mut it) = item {
            it.grandparent(
                &self
                    .ctx
                    .db,
            )
            .await
            .ok();
        }

        let mut results = Vec::with_capacity(
            sel.candidates
                .len(),
        );
        for stream in sel
            .candidates
            .into_iter()
        {
            let url_opt = stream
                .stream_info
                .as_ref()
                .map(|si| {
                    si.descriptor
                        .server_input(stream.id, port)
                });
            let skip_probe = preferred_probe_id
                .is_some_and(|preferred| stream.id != preferred)
                // A legacy/RemuxDB H.264 result without a usable ref-frame
                // count is deliberately stale. Probe it even when it is not
                // the preferred candidate so compatibility ranking has the
                // metadata it needs.
                && !stream
                    .probe_data
                    .as_ref()
                    .is_some_and(ProbeDataExt::needs_reprobe);
            // A filename guess is never a completed probe — it must not skip
            // submitting a freshly-probed result to RemuxDB.
            let was_cached = stream
                .probe_data
                .as_ref()
                .is_some_and(|pd| {
                    pd.video_stream()
                        .is_some()
                        && !pd.is_filename_guess()
                });
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

            // Use the StreamGroup UUID when this candidate is a group representative
            // (group_id is set by filter_sources). This ensures the client sends back
            // the stable group UUID, not a stream UUID that can change after a refresh.
            let (cid, name) = if let Some(gid) = stream.group_id {
                (
                    gid,
                    stream
                        .title
                        .clone(),
                )
            } else {
                (
                    self.source_id_for(&effective_stream),
                    self.source_name_for(&effective_stream),
                )
            };
            source.id = cid;
            source.e_tag = cid;
            source.name = Some(name);
            source.has_segments = true;
            // Include the release filename's stem when available (same
            // convention as MediaSourceInfo::from(db::Media) in
            // conversions.rs) so clients that surface `Path` as a display
            // field show the real release name instead of a bare UUID.
            // This path previously always dropped it, even when the addon
            // supplied behaviorHints.filename and it was sitting right
            // there in effective_stream.stream_info.
            let stem = effective_stream
                .stream_info
                .as_ref()
                .and_then(|si| {
                    si.filename
                        .as_deref()
                })
                .and_then(|f| {
                    std::path::Path::new(f)
                        .file_stem()
                        .and_then(|s| s.to_str())
                });
            source.path = Some(match stem {
                Some(s) => format!("/remux/{}/{}", effective_stream.id, s),
                None => format!("/remux/{}", effective_stream.id),
            });
            source.is_remote = false;
            // Re-apply binge-group headers — ffmpeg probing produces a fresh
            // MediaSourceInfo and would otherwise drop provider hints. Must
            // preserve whatever probe_source tag probe_stream() already set
            // (Ffprobe from a real probe, or carried over from cached data) —
            // a blanket ..Default::default() here would silently erase it.
            let probe_source = source
                .remux
                .as_ref()
                .and_then(|r| r.source);
            source.remux = Some(api::MediaSourceRemuxInfo {
                provider_info: effective_stream
                    .stream_info
                    .as_ref()
                    .and_then(|si| si.to_public_json()),
                source: probe_source,
            });

            let remuxdb_enabled = probe_cfg
                .remuxdb_enabled
                .unwrap_or(true);
            let is_remuxdb_kind = item
                .as_ref()
                .map_or(false, |it| {
                    matches!(it.kind, db::MediaKind::Movie | db::MediaKind::Episode)
                });
            if was_cached {
                debug!(id = %effective_stream.id, "remuxdb: skipping (probe cache hit)");
            } else if !remuxdb_enabled {
                debug!(id = %effective_stream.id, "remuxdb: skipping (disabled)");
            } else if !is_remuxdb_kind {
                debug!(id = %effective_stream.id, kind = ?item.as_ref().map(|it| &it.kind), "remuxdb: skipping (not movie/episode)");
            } else if source.is_filename_guess() {
                debug!(id = %effective_stream.id, "remuxdb: skipping (filename guess, not a real probe)");
            } else if let Some(url) = self
                .ctx
                .config
                .remuxdb_url
                .clone()
            {
                match media_info_from_probe(&source, &effective_stream, item.as_ref()) {
                    Ok(mi) => {
                        debug!(id = %effective_stream.id, url, "remuxdb: submitting mediainfo");
                        let token = probe_cfg
                            .remuxdb_token
                            .clone();
                        tokio::spawn(mi.submit(url, token));
                    }
                    Err(reason) => {
                        warn!(id = %effective_stream.id, reason, "remuxdb: skipping submission");
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

    /// Remember which stream a PlaybackInfo answer told the client to play,
    /// after probe fallback and ordering. Clients may request the item ID
    /// (auto-play: PlaybackInfo rewrites `MediaSources[0].Id` to it), a
    /// stream-group ID, or the originally selected stream ID even after
    /// PlaybackInfo returned a fallback. Keying by the play session and that
    /// requested ID makes every playback endpoint serve the stream whose media
    /// info was returned to the client.
    ///
    /// `stream_id` is the candidate as listed; `effective_stream_id` is the
    /// stream probed for it (they differ after a probe fallback).
    pub fn save_probe_fallback(
        &self,
        play_session_id: &str,
        device_id: &str,
        specific_requested: bool,
        stream_id: Uuid,
        effective_stream_id: Uuid,
    ) {
        let source_id = match &self.group {
            Some((gid, _, _)) => *gid,
            None if specific_requested => self
                .requested_id
                .unwrap_or(self.item_id),
            None => self.item_id,
        };
        // Infuse's direct stream URL has MediaSourceId but no PlaySessionId or
        // DeviceId. Keep a brief mapping for that request too, keyed by the
        // device its token belongs to. Deduped: for a specific-stream request,
        // source_id and the listed candidate's own id are frequently identical.
        let recent_ids: std::collections::HashSet<Uuid> = [source_id, stream_id]
            .into_iter()
            .collect();
        for recent_id in recent_ids {
            self.ctx
                .store
                .save(
                    Self::recent_probe_fallback_key(device_id, self.item_id, recent_id),
                    effective_stream_id,
                    std::time::Duration::from_secs(5 * 60),
                );
        }
        self.ctx
            .store
            .save(
                Self::probe_fallback_key(play_session_id, source_id),
                effective_stream_id,
                std::time::Duration::from_secs(24 * 3600),
            );
    }

    /// The stream PlaybackInfo selected when it answered `play_session_id`
    /// with `source_id` (item, group, or stream ID), if any.
    pub fn probe_fallback_for(
        ctx: &AppContext,
        play_session_id: &str,
        source_id: Uuid,
    ) -> Option<Uuid> {
        ctx.store
            .get::<Uuid>(Self::probe_fallback_key(play_session_id, source_id))
            .map(|id| *id)
    }

    /// Fallback for clients that omit PlaySessionId from the stream URL.
    /// Scoped by device, not user: two devices of one user can call
    /// PlaybackInfo for the same item with different capabilities, and each
    /// must get its own selection. A device belongs to one user, so this
    /// also never answers another user's request.
    pub fn recent_probe_fallback_for(
        ctx: &AppContext,
        device_id: &str,
        item_id: Uuid,
        source_id: Uuid,
    ) -> Option<Uuid> {
        ctx.store
            .get::<Uuid>(Self::recent_probe_fallback_key(
                device_id, item_id, source_id,
            ))
            .map(|id| *id)
    }

    /// The stream a playback request for `item_id` / `media_source_id` must
    /// serve, as selected by the PlaybackInfo that answered it: looked up by
    /// play session first, then by the requesting device's recent request.
    pub fn probe_fallback_stream_id(
        ctx: &AppContext,
        device_id: Option<&str>,
        item_id: Uuid,
        media_source_id: Option<Uuid>,
        play_session_id: Option<&str>,
    ) -> Option<Uuid> {
        let source_id = media_source_id.unwrap_or(item_id);
        play_session_id
            .and_then(|psid| Self::probe_fallback_for(ctx, psid, source_id))
            .or_else(|| {
                device_id.and_then(|device_id| {
                    Self::recent_probe_fallback_for(ctx, device_id, item_id, source_id)
                })
            })
    }

    fn recent_probe_fallback_key(
        device_id: &str,
        item_id: Uuid,
        source_id: Uuid,
    ) -> String {
        format!("pstream:recent:{device_id}:{item_id}:{source_id}")
    }

    fn probe_fallback_key(play_session_id: &str, source_id: Uuid) -> String {
        format!("pstream:psid:{play_session_id}:{source_id}")
    }

    /// Persist the resolved stream UUID in the device-preference store (24 h TTL).
    /// Also records the group→item association so /Items/{group_uuid} can redirect
    /// to the correct content item without a DB scan.
    /// No-op when this was not a group request.
    pub fn save_preference(&self, device_key: &str) {
        let Some((gid, _, _)) = &self.group else {
            return;
        };
        self.ctx
            .store
            .save(
                format!("pstream:{}:{}", self.item_id, device_key),
                self.candidate()
                    .id,
                std::time::Duration::from_secs(24 * 3600),
            );
        if let Some(uid) = self.user_id {
            Self::save_group_item(
                &self
                    .ctx
                    .store,
                uid,
                *gid,
                self.item_id,
            );
        }
    }

    /// Record that `group_id` (a stream group UUID) belongs to `item_id` for the given user.
    ///
    /// Keyed per-user to avoid collisions when the same global group appears across multiple
    /// media items. Used by `/Items/{group_uuid}` to redirect back to the owning content item.
    /// TTL is 7 days — long enough to survive normal browsing sessions.
    pub fn save_group_item(
        store: &remux_utils::Store,
        user_id: Uuid,
        group_id: Uuid,
        item_id: Uuid,
    ) {
        store.save(
            format!("gitem:{}:{}", user_id, group_id),
            item_id,
            std::time::Duration::from_secs(7 * 24 * 3600),
        );
    }

    /// Look up the content item that owns `group_id` for `user_id`.
    ///
    /// Returns `None` when the user has not yet browsed an item that carries this stream group,
    /// or the mapping has expired. Callers should surface a 404 in that case.
    pub fn get_group_item(
        store: &remux_utils::Store,
        user_id: Uuid,
        group_id: Uuid,
    ) -> Option<Uuid> {
        store
            .get::<Uuid>(format!("gitem:{}:{}", user_id, group_id))
            .map(|id| *id)
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
    /// True when the client named a specific stream — keep its UUID, don't override to item_id.
    pub specific_requested: bool,
}

fn media_info_from_probe(
    probe: &api::MediaSourceInfo,
    stream: &db::Media,
    item: Option<&db::Media>,
) -> Result<remuxdb::MediaInfoPayload, &'static str> {
    let (info_hash, file_idx, nzb, filename) = match stream
        .stream_info
        .as_ref()
    {
        Some(si) => {
            let (hash, idx) = match si.torrent_identity() {
                Some((hash, idx)) => (Some(hash.to_owned()), idx),
                None => (None, None),
            };
            let nzb = si
                .usenet_guid
                .as_ref()
                .zip(
                    si.usenet_indexer
                        .as_ref(),
                )
                .map(|(guid, indexer)| remuxdb::NzbSubmission {
                    indexer: indexer.clone(),
                    indexer_guid: guid.clone(),
                    title: si
                        .filename
                        .clone(),
                });
            (
                hash,
                idx,
                nzb,
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
            None,
            stream
                .title
                .clone(),
        ),
    };

    if info_hash.is_none() && nzb.is_none() {
        return Err(
            if stream
                .stream_info
                .is_none()
            {
                "stream has no stream_info at all"
            } else {
                "stream_info has neither a torrent hash nor a usenet nzb identity"
            },
        );
    }

    let (kind, external_ids, season, episode) = if let Some(item) = item {
        let kind = match item.kind {
            db::MediaKind::Episode => "episode",
            _ => "movie",
        }
        .to_string();
        let own_imdb = item
            .external_ids
            .imdb
            .as_ref()
            .map(|v| v.to_string());
        let series_imdb = item
            .grandparent
            .as_deref()
            .and_then(|gp| {
                gp.external_ids
                    .imdb
                    .as_ref()
            })
            .map(|v| v.to_string());
        // RemuxDB stores episodes under the series id: an episode's own
        // tconst would 404.
        let imdb_id = if item.kind == db::MediaKind::Episode {
            series_imdb.or(own_imdb)
        } else {
            own_imdb.or(series_imdb)
        };
        let ids = (imdb_id.is_some()
            || item
                .external_ids
                .tmdb
                .is_some()
            || item
                .external_ids
                .tvdb
                .is_some()
            || item
                .external_ids
                .kitsu
                .is_some())
        .then(|| remuxdb::ExternalIds {
            imdb_id,
            tmdb_id: item
                .external_ids
                .tmdb,
            tvdb_id: item
                .external_ids
                .tvdb,
            kitsu_id: item
                .external_ids
                .kitsu,
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

    let size = probe
        .size
        .or_else(|| {
            stream
                .stream_info
                .as_ref()
                .and_then(|si| si.size)
        })
        .filter(|&s| s > 0)
        .ok_or("no positive size on probe or stream_info")?;

    let tracks: Vec<remuxdb::TrackPayload> = probe
        .media_streams
        .iter()
        .filter_map(|ms| remuxdb::TrackPayload::try_from(ms).ok())
        .collect();

    Ok(remuxdb::MediaInfoPayload {
        client_id: Some(crate::common::server_id()),
        kind,
        filename,
        torrent_info_hash: info_hash,
        torrent_file_idx: file_idx,
        nzb,
        container: probe
            .container
            .as_ref()
            .map(|c| c.to_string())
            .unwrap_or_default(),
        size,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{StreamDescriptor, StreamInfo};

    /// Candidates come out in the order PlaybackInfo lists them, and the
    /// first is the one probed and auto-played: addon order with sorting
    /// disabled or for group representatives, capability ranking otherwise.
    #[tokio::test]
    async fn rank_candidates_orders_like_playback_info() {
        use crate::integration_test::{authenticated_server, insert_test_source};

        let (_server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let unprobed = |mut media: db::Media, filename: &str| {
            media.probe_data = None;
            media
                .stream_info
                .as_mut()
                .unwrap()
                .filename = Some(filename.to_string());
            media
        };
        let low = unprobed(
            insert_test_source(ctx).await,
            "Movie.2026.480p.WEB-DL.x264-GROUP.mkv",
        );
        let high = unprobed(
            insert_test_source(ctx).await,
            "Movie.2026.1080p.BluRay.x264-GROUP.mkv",
        );
        let service = StreamService::new(StreamServiceConfig {
            ctx: ctx.clone(),
            item_id: Uuid::new_v4(),
            requested_id: None,
            show_ungrouped: true,
            stream_filter: None,
            user_id: None,
        });
        let ranking = SourceRankingContext {
            mode: Default::default(),
            device_profile: None,
            is_4k_capable: true,
            subtitle_mode: Default::default(),
            explicit_subtitle_index: None,
            max_bitrate: None,
        };
        let candidates = vec![low.clone(), high.clone()];
        let order = async |candidates: Vec<db::Media>, ranking| {
            service
                .rank_candidates(candidates, ranking)
                .await
                .into_iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            order(candidates.clone(), None).await,
            vec![low.id, high.id],
            "sorting disabled keeps addon order"
        );
        assert_eq!(
            order(candidates.clone(), Some(ranking)).await,
            vec![high.id, low.id],
            "sorting enabled puts the ranking winner first"
        );

        let grouped: Vec<_> = candidates
            .into_iter()
            .map(|mut s| {
                s.group_id = Some(Uuid::new_v4());
                s
            })
            .collect();
        assert_eq!(
            order(grouped, Some(ranking)).await,
            vec![low.id, high.id],
            "group order is admin-authored and never re-ranked"
        );
    }

    /// A `MediaSourceId` that is an item id (auto-play, or the PlaybackInfo
    /// rewrite of `source[0].Id` — the sibling's UUID when duplicate items share
    /// one IMDB id) must resolve to a stream. Before the fix the Movie arm looked
    /// for the item's own id among its streams, failed with "stream not found",
    /// and the stream endpoint served the no-streams placeholder (#212): source
    /// #1 of every affected title played blank while the rest worked.
    #[tokio::test]
    async fn lookup_treats_item_id_as_auto_play_not_stream_id() {
        use crate::integration_test::{
            authenticated_server, insert_test_source, seed_movie,
        };

        let (_server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;

        // Movie A owns one stream. `Media::save` is an upsert that doesn't
        // update `parent_id`, so attach the row directly. Stamp
        // `streams_refreshed_at` 30s back: `refresh_streams` (no addons here)
        // takes its TTL fast path, and `Media::streams()` keeps the row.
        let owner = seed_movie(ctx).await;
        let stream = insert_test_source(ctx).await;
        sqlx::query("UPDATE media SET parent_id = ? WHERE id = ?")
            .bind(owner.id)
            .bind(stream.id)
            .execute(&ctx.db)
            .await
            .unwrap();
        sqlx::query("UPDATE media SET streams_refreshed_at = ? WHERE id = ?")
            .bind(chrono::Utc::now().naive_utc() - chrono::Duration::seconds(30))
            .bind(owner.id)
            .execute(&ctx.db)
            .await
            .unwrap();

        // Movie B: an unrelated item with no stream rows of its own. Sharing
        // `owner`'s external ids used to be how this scenario arose (two
        // rows for the same film) — that's now prevented at the DB level,
        // but the code path under test only cares that B's id is a real,
        // distinct Movie row mistakenly handed back as a MediaSourceId, not
        // that it represents the same content as A, so it just needs its
        // own (any) external id to satisfy validation.
        let mut dup = db::Media {
            id: Uuid::new_v4(),
            title: owner
                .title
                .clone(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt9999998").ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        dup.save(&ctx.db)
            .await
            .unwrap();

        // B played with MediaSourceId = A.id (what the auto-play rewrite hands
        // out). Before the fix: Err("stream not found: <A.id>").
        let resolved = StreamService::lookup(ctx, dup.id, Some(owner.id), None, None)
            .await
            .expect("an item id used as MediaSourceId must resolve to a stream");
        assert_eq!(resolved.id, stream.id);

        // Plain auto-play (MediaSourceId == item being played) still works.
        let resolved = StreamService::lookup(ctx, owner.id, Some(owner.id), None, None)
            .await
            .unwrap();
        assert_eq!(resolved.id, stream.id);

        // A real stream id is still honoured.
        let resolved =
            StreamService::lookup(ctx, owner.id, Some(stream.id), None, None)
                .await
                .unwrap();
        assert_eq!(resolved.id, stream.id);
    }

    const DEBRID_HASH: &str = "63259f55cd5c31826321286ae1fde40c931dee1d";
    const DESCRIPTOR_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// Minimal successful probe: only `size` is required by `media_info_from_probe`.
    fn probe_with_size(size: i64) -> api::MediaSourceInfo {
        api::MediaSourceInfo {
            size: Some(size),
            ..Default::default()
        }
    }

    fn stream_media(info: StreamInfo) -> db::Media {
        db::Media {
            title: "Example".to_string(),
            stream_info: Some(info),
            ..Default::default()
        }
    }

    fn quality_stream(filename: &str) -> db::Media {
        let mut stream = stream_media(StreamInfo {
            descriptor: StreamDescriptor::http(format!(
                "https://example.test/{filename}"
            )),
            filename: Some(filename.to_string()),
            ..Default::default()
        });
        stream.id = Uuid::new_v4();
        stream
    }

    #[test]
    fn quality_probe_order_does_not_mutate_addon_order() {
        let original = vec![
            quality_stream("Movie.2026.720p.WEBRip.mkv"),
            quality_stream("Movie.2026.2160p.BluRay.Remux.mkv"),
            quality_stream("Movie.2026.1080p.WEB-DL.mkv"),
        ];
        let original_ids: Vec<_> = original
            .iter()
            .map(|stream| stream.id)
            .collect();

        let probe_pool = quality_ordered_probe_pool(&original);
        let candidate_ids: Vec<_> = original
            .iter()
            .map(|stream| stream.id)
            .collect();
        let probe_ids: Vec<_> = probe_pool
            .iter()
            .map(|stream| stream.id)
            .collect();

        assert_eq!(candidate_ids, original_ids);
        assert_eq!(
            probe_ids,
            vec![original_ids[1], original_ids[2], original_ids[0]]
        );
    }

    #[test]
    fn media_info_from_probe_preserves_torrent_identity_for_http_debrid_stream() {
        let stream = stream_media(StreamInfo {
            descriptor: StreamDescriptor::http("https://debrid.example/playback/123"),
            torrent_info_hash: Some(DEBRID_HASH.to_string()),
            torrent_file_idx: Some(4),
            filename: Some("Example.2026.2160p.mkv".to_string()),
            ..Default::default()
        });
        let payload = media_info_from_probe(
            &probe_with_size(12_340_295_313),
            &stream,
            None,
        )
        .expect("HTTP debrid stream with preserved torrent identity must be submitted");
        assert_eq!(
            payload
                .torrent_info_hash
                .as_deref(),
            Some(DEBRID_HASH)
        );
        assert_eq!(payload.torrent_file_idx, Some(4));
        assert!(
            payload
                .nzb
                .is_none()
        );
    }

    #[test]
    fn media_info_from_probe_prefers_torrent_descriptor_identity() {
        let stream = stream_media(StreamInfo {
            descriptor: StreamDescriptor::Torrent {
                info_hash: DESCRIPTOR_HASH.to_string(),
                file_hint: None,
                file_idx: Some(7),
                trackers: vec![],
            },
            torrent_info_hash: Some(DEBRID_HASH.to_string()),
            torrent_file_idx: Some(4),
            ..Default::default()
        });
        let payload =
            media_info_from_probe(&probe_with_size(1), &stream, None).unwrap();
        assert_eq!(
            payload
                .torrent_info_hash
                .as_deref(),
            Some(DESCRIPTOR_HASH)
        );
        assert_eq!(payload.torrent_file_idx, Some(7));
    }

    #[test]
    fn media_info_from_probe_http_debrid_without_file_idx_still_submits() {
        let stream = stream_media(StreamInfo {
            descriptor: StreamDescriptor::http("https://debrid.example/playback/123"),
            torrent_info_hash: Some(DEBRID_HASH.to_string()),
            torrent_file_idx: None,
            ..Default::default()
        });
        let payload =
            media_info_from_probe(&probe_with_size(1), &stream, None).unwrap();
        assert_eq!(
            payload
                .torrent_info_hash
                .as_deref(),
            Some(DEBRID_HASH)
        );
        assert_eq!(payload.torrent_file_idx, None);
    }

    #[test]
    fn media_info_from_probe_nzb_only_stream_submits_without_torrent_identity() {
        let stream = stream_media(StreamInfo {
            descriptor: StreamDescriptor::http("https://usenet.example/file"),
            usenet_guid: Some("guid-123".to_string()),
            usenet_indexer: Some("NZBgeek".to_string()),
            filename: Some("Example.2026.1080p.mkv".to_string()),
            ..Default::default()
        });
        let payload = media_info_from_probe(&probe_with_size(1), &stream, None)
            .expect("usenet stream must still be submitted");
        let nzb = payload
            .nzb
            .expect("nzb submission populated");
        assert_eq!(nzb.indexer, "NZBgeek");
        assert_eq!(nzb.indexer_guid, "guid-123");
        assert_eq!(
            nzb.title
                .as_deref(),
            Some("Example.2026.1080p.mkv")
        );
        assert!(
            payload
                .torrent_info_hash
                .is_none()
        );
        assert!(
            payload
                .torrent_file_idx
                .is_none()
        );
    }

    #[test]
    fn media_info_from_probe_skips_http_stream_without_any_identity() {
        let stream = stream_media(StreamInfo {
            descriptor: StreamDescriptor::http("https://cdn.example/file.mkv"),
            ..Default::default()
        });
        assert!(media_info_from_probe(&probe_with_size(1), &stream, None).is_err());
    }

    /// A probe fallback must reach the stream request that follows PlaybackInfo.
    /// That request names the item, not a stream, so without this the resolver
    /// serves the first source — the one that just failed — and playback hangs
    /// until the upstream timeout while the second source, picked by hand, plays.
    #[tokio::test]
    async fn probe_fallback_is_remembered_per_play_session() {
        use crate::integration_test::{
            authenticated_server, insert_test_source, seed_movie,
        };
        let (_server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let owner = seed_movie(ctx).await;
        let dead = insert_test_source(ctx).await;
        let alive = insert_test_source(ctx).await;
        assert_ne!(dead.id, alive.id);
        let service = StreamService::new(StreamServiceConfig {
            ctx: ctx.clone(),
            item_id: owner.id,
            requested_id: None,
            show_ungrouped: true,
            stream_filter: None,
            user_id: None,
        });
        // Fell over to `alive`: remembered under the play session.
        service.save_probe_fallback(
            "psid-fallback",
            "device-a",
            false,
            dead.id,
            alive.id,
        );
        assert_eq!(
            StreamService::probe_fallback_for(ctx, "psid-fallback", owner.id),
            Some(alive.id)
        );
        // First source probed fine: the item id still maps to it, so the
        // stream request can't drift to a different (unprobed) source.
        service.save_probe_fallback("psid-clean", "device-a", false, dead.id, dead.id);
        assert_eq!(
            StreamService::probe_fallback_for(ctx, "psid-clean", owner.id),
            Some(dead.id)
        );
        // Two devices of one user auto-playing the same item can be given
        // different versions; a session-less request from each must get its
        // own, not whichever PlaybackInfo answered last.
        service.save_probe_fallback("psid-tv", "device-tv", false, dead.id, dead.id);
        service.save_probe_fallback(
            "psid-phone",
            "device-phone",
            false,
            alive.id,
            alive.id,
        );
        assert_eq!(
            StreamService::probe_fallback_stream_id(
                ctx,
                Some("device-tv"),
                owner.id,
                None,
                None
            ),
            Some(dead.id)
        );
        assert_eq!(
            StreamService::probe_fallback_stream_id(
                ctx,
                Some("device-phone"),
                owner.id,
                None,
                None
            ),
            Some(alive.id)
        );
        assert_eq!(
            StreamService::probe_fallback_stream_id(ctx, None, owner.id, None, None),
            None,
            "a request without a device token never matches"
        );
        // A client can keep requesting the original stream ID for direct play
        // even though PlaybackInfo returned the fallback stream ID.
        let mut specific_service = StreamService::new(StreamServiceConfig {
            ctx: ctx.clone(),
            item_id: owner.id,
            requested_id: Some(dead.id),
            show_ungrouped: true,
            stream_filter: None,
            user_id: None,
        });
        specific_service.streams = vec![dead.clone(), alive.clone()];
        assert!(
            specific_service
                .select_streams()
                .specific_requested
        );
        specific_service.save_probe_fallback(
            "psid-specific",
            "device-a",
            true,
            dead.id,
            alive.id,
        );
        assert_eq!(
            StreamService::probe_fallback_for(ctx, "psid-specific", dead.id),
            Some(alive.id)
        );
        assert_eq!(
            StreamService::recent_probe_fallback_for(
                ctx, "device-a", owner.id, dead.id
            ),
            Some(alive.id),
            "Infuse's sessionless direct request must resolve to the probed stream"
        );
        assert_eq!(
            StreamService::recent_probe_fallback_for(
                ctx, "device-a", alive.id, dead.id
            ),
            None,
            "recent fallback must not leak to another item"
        );
        assert_eq!(
            StreamService::recent_probe_fallback_for(
                ctx, "device-b", owner.id, dead.id
            ),
            None,
            "recent fallback must not leak to another device's session-less request"
        );
        specific_service.save_probe_fallback(
            "psid-recovered",
            "device-a",
            true,
            dead.id,
            dead.id,
        );
        assert_eq!(
            StreamService::recent_probe_fallback_for(
                ctx, "device-a", owner.id, dead.id
            ),
            Some(dead.id),
            "a successful probe must replace a stale fallback"
        );
        // Unknown session: nothing.
        assert_eq!(
            StreamService::probe_fallback_for(ctx, "psid-unknown", owner.id),
            None
        );
    }

    /// A client sending `MediaSourceId == item_id` (Android TV auto-play, or
    /// any client that doesn't omit the field) must get the same
    /// capability-ranked candidate an omitted MediaSourceId would — not just
    /// whichever stream happened to be first in DB order. Regression test for
    /// a bug where this case truncated to `all_streams[0]` before probing,
    /// leaving playback.rs's later ranking sort nothing else to rank.
    #[tokio::test]
    async fn auto_play_with_item_id_ranks_candidates_like_no_id_at_all() {
        use crate::integration_test::{authenticated_server, insert_test_source};

        let (_server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;

        let mut low_quality = insert_test_source(ctx).await;
        low_quality
            .stream_info
            .as_mut()
            .unwrap()
            .filename = Some("Movie.2026.CAM.x264-GROUP.mkv".to_string());
        let mut high_quality = insert_test_source(ctx).await;
        high_quality
            .stream_info
            .as_mut()
            .unwrap()
            .filename = Some("Movie.2026.2160p.BluRay.x265-GROUP.mkv".to_string());

        let item_id = uuid::Uuid::new_v4();
        let mut service = StreamService::new(StreamServiceConfig {
            ctx: ctx.clone(),
            item_id,
            requested_id: Some(item_id),
            show_ungrouped: false,
            stream_filter: None,
            user_id: None,
        });
        // Low quality deliberately listed first — this is exactly what used
        // to get returned verbatim as "the" candidate.
        service.streams = vec![low_quality.clone(), high_quality.clone()];

        let selection = service.select_streams();
        assert!(
            !selection.specific_requested,
            "item_id as MediaSourceId must still be treated as auto-play"
        );
        assert_eq!(
            selection
                .candidates
                .len(),
            2,
            "all candidates must remain available for probing and the later ranking sort"
        );
        assert_eq!(
            selection.probe_pool[0].id, high_quality.id,
            "probe fallback must try the higher-quality candidate first, not just the first in DB order"
        );
    }

    /// With stream groups on, the initial PlaybackInfo lists one representative
    /// stream per group and is not a specific request, so a fallback is
    /// remembered under the item id exactly as without groups. A request for a
    /// group by its UUID answers with the group id, so its fallback is
    /// remembered under the group id.
    #[tokio::test]
    async fn probe_fallback_with_stream_groups() {
        use crate::integration_test::{
            authenticated_server, insert_test_source, seed_movie,
        };
        let (_server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let owner = seed_movie(ctx).await;
        let group_a = uuid::Uuid::new_v4();
        let group_b = uuid::Uuid::new_v4();
        let mut dead = insert_test_source(ctx).await;
        let mut alive = insert_test_source(ctx).await;
        dead.group_id = Some(group_a);
        alive.group_id = Some(group_b);
        // Initial load: group representatives, no group context.
        let mut service = StreamService::new(StreamServiceConfig {
            ctx: ctx.clone(),
            item_id: owner.id,
            requested_id: None,
            show_ungrouped: false,
            stream_filter: None,
            user_id: None,
        });
        service.streams = vec![dead.clone(), alive.clone()];
        let selection = service.select_streams();
        assert!(!selection.specific_requested);
        service.save_probe_fallback(
            "psid-grouped",
            "device-a",
            selection.specific_requested,
            dead.id,
            alive.id,
        );
        assert_eq!(
            StreamService::probe_fallback_for(ctx, "psid-grouped", owner.id),
            Some(alive.id)
        );

        // Group A requested by its UUID.
        let mut service = StreamService::new(StreamServiceConfig {
            ctx: ctx.clone(),
            item_id: owner.id,
            requested_id: Some(group_a),
            show_ungrouped: false,
            stream_filter: None,
            user_id: None,
        });
        service.group = Some((
            group_a,
            "Group A".to_string(),
            vec![dead.clone(), alive.clone()],
        ));
        service.stream = Some(dead.clone());
        service.streams = vec![dead.clone(), alive.clone()];
        let selection = service.select_streams();
        assert!(selection.specific_requested);
        service.save_probe_fallback(
            "psid-group-request",
            "device-a",
            selection.specific_requested,
            dead.id,
            alive.id,
        );
        assert_eq!(
            StreamService::probe_fallback_for(ctx, "psid-group-request", group_a),
            Some(alive.id)
        );
    }
}
