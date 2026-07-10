use crate::{AppContext, db, playback::probe::resolve_stream_root};
use remux_sdks::remux::StreamFilter;
use tracing::debug;
use uuid::Uuid;

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

    /// Resolve the service from a pre-fetched initial media (playbackinfo path).
    ///
    /// Populates `self.group`, `self.stream`, and `self.streams`. Must be called
    /// before any of the selection or ID-mapping methods.
    pub async fn resolve(&mut self, initial: db::Media) -> anyhow::Result<()> {
        if initial.kind == db::MediaKind::StreamGroup {
            self.resolve_stream_group(initial)
                .await?;
            return Ok(());
        }

        let mut root = resolve_stream_root(
            &initial,
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
    pub fn stream(&self) -> &db::Media {
        self.stream
            .as_ref()
            .expect("StreamService::resolve() must be called first")
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
                self.stream()
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
                    self.stream()
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
                self.stream()
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
