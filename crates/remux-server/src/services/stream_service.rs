use crate::db;
use remux_utils::Store;
use uuid::Uuid;

/// Central service for stream selection policy on a single playback request.
///
/// Resolves a client-supplied source UUID to a concrete `db::Media`, holds the full
/// filtered stream list for the item, and encapsulates all candidate/probe-pool
/// partitioning logic so handlers stay thin.
pub(crate) struct StreamService {
    /// The item UUID from the URL route (Movie/Episode/Track).
    pub item_id: Uuid,
    /// When the requested source was a StreamGroup: (group_uuid, display_title, all_candidates).
    pub group: Option<(Uuid, String, Vec<db::Media>)>,
    /// The concrete playable stream (best candidate).
    pub stream: db::Media,
    /// All available, filtered, policy-applied streams for this item.
    /// Populated by the caller after building the stream list.
    pub streams: Vec<db::Media>,
}

impl StreamService {
    /// Full resolution: StreamGroup → best candidate; Movie/Episode/Track → preferred/first source.
    ///
    /// Use this in video-stream and subtitle handlers where exactly one concrete stream is needed.
    pub async fn resolve(
        db: &sqlx::SqlitePool,
        store: &Store,
        item_id: Uuid,
        requested_id: Option<Uuid>,
        device_key: Option<&str>,
    ) -> anyhow::Result<Self> {
        let lookup_id = requested_id.unwrap_or(item_id);
        let media = db::Media::get_by_id(db, &lookup_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("stream not found: {}", lookup_id))?;
        Self::dispatch(db, store, item_id, requested_id, device_key, media).await
    }

    /// Group-only resolution: StreamGroup → best candidate; Movie/Episode/Track → returned as-is.
    ///
    /// Use this in the playbackinfo handler where `streams` is built externally and set
    /// on the returned service before calling `select_streams`.
    pub async fn resolve_group(
        db: &sqlx::SqlitePool,
        store: &Store,
        item_id: Uuid,
        requested_id: Option<Uuid>,
        initial: db::Media,
    ) -> anyhow::Result<Self> {
        if initial.kind == db::MediaKind::StreamGroup {
            Self::resolve_stream_group(db, item_id, initial).await
        } else {
            Ok(Self {
                item_id,
                group: None,
                stream: initial,
                streams: vec![],
            })
        }
    }

    async fn dispatch(
        db: &sqlx::SqlitePool,
        store: &Store,
        item_id: Uuid,
        requested_id: Option<Uuid>,
        device_key: Option<&str>,
        media: db::Media,
    ) -> anyhow::Result<Self> {
        match media.kind {
            db::MediaKind::StreamGroup => {
                Self::resolve_stream_group(db, item_id, media).await
            }
            db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track => {
                let mut media = media;
                let sources = media
                    .streams(db)
                    .await?;
                let stream = if let Some(sid) =
                    requested_id.filter(|&sid| sid != item_id)
                {
                    sources
                        .into_iter()
                        .find(|s| s.id == sid)
                        .ok_or_else(|| anyhow::anyhow!("stream not found: {}", sid))?
                } else if let Some(key) = device_key {
                    let saved =
                        store.get::<Uuid>(&format!("pstream:{}:{}", item_id, key));
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
                        })?
                } else {
                    sources
                        .into_iter()
                        .next()
                        .ok_or_else(|| {
                            anyhow::anyhow!("no playable sources for {}", item_id)
                        })?
                };
                Ok(Self {
                    item_id,
                    group: None,
                    stream,
                    streams: vec![],
                })
            }
            _ => Ok(Self {
                item_id,
                group: None,
                stream: media,
                streams: vec![],
            }),
        }
    }

    async fn resolve_stream_group(
        db: &sqlx::SqlitePool,
        item_id: Uuid,
        media: db::Media,
    ) -> anyhow::Result<Self> {
        let gid = media.id;
        let gtitle = media
            .title
            .clone();
        let mut candidates = db::StreamGroup::streams_for(db, &gid, &item_id).await?;
        if candidates.is_empty() {
            return Err(anyhow::anyhow!("no streams available for group {}", gid));
        }
        let cascade = db::StreamGroup::streams_for_groups_after(db, &gid, &item_id)
            .await
            .unwrap_or_default();
        candidates.extend(cascade);
        let stream = candidates[0].clone();
        Ok(Self {
            item_id,
            group: Some((gid, gtitle, candidates)),
            stream,
            streams: vec![],
        })
    }

    /// UUID the client should see in `MediaSources[0].Id` and `TranscodingUrl MediaSourceId`.
    ///
    /// Returns the group UUID when a StreamGroup was requested, otherwise the stream's own UUID.
    pub fn client_facing_id(&self) -> Uuid {
        self.group
            .as_ref()
            .map(|(gid, _, _)| *gid)
            .unwrap_or(
                self.stream
                    .id,
            )
    }

    /// UUID to echo in `MediaSources[idx].Id`, using the probe-fallback result as the fallback.
    ///
    /// Unlike `client_facing_id()`, this takes the effective stream (which may differ from
    /// `self.stream` when a probe fallback selected a sibling stream) as the non-group fallback.
    pub fn source_id_for(&self, effective: &db::Media) -> Uuid {
        self.group
            .as_ref()
            .map(|(gid, _, _)| *gid)
            .unwrap_or(effective.id)
    }

    /// Display name to echo in `MediaSources[idx].Name` (group title or effective stream title).
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

    /// Probe fallback pool — includes cascade candidates for group requests, empty otherwise.
    pub fn candidates(&self) -> &[db::Media] {
        self.group
            .as_ref()
            .map(|(_, _, c)| c.as_slice())
            .unwrap_or(&[])
    }

    /// Partition `self.streams` into candidate/probe lists and compute selection flags.
    ///
    /// Encapsulates the 4-way selection policy:
    /// - group request   → candidates=[self.stream], probe_pool=cascade candidates, restrict=false
    /// - specific stream → candidates=[that stream], probe_pool=self.streams,       restrict=true
    /// - android-tv      → candidates=[first],       probe_pool=self.streams,       restrict=true
    /// - no stream id    → candidates=all,            probe_pool=self.streams,       restrict=true, probe_only_first=true
    ///
    /// Caller must set `self.streams` before calling this.
    pub(crate) fn select_streams(&self, requested_id: Option<Uuid>) -> StreamSelection {
        let all_streams = self
            .streams
            .clone();
        let item_id = self.item_id;

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
                    self.stream
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

        // Non-group: full filtered list is always the probe fallback pool.
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
    pub fn save_preference(&self, store: &Store, device_key: &str) {
        store.save(
            format!("pstream:{}:{}", self.item_id, device_key),
            self.stream
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
