use crate::db;
use remux_utils::Store;
use uuid::Uuid;

/// Resolved stream for a client-supplied source UUID.
///
/// Encapsulates the mapping from a client-facing UUID (which may be a StreamGroup, a direct
/// Stream, or a Movie/Episode/Track) to the concrete `db::Media` that should actually be
/// probed and played, along with the group context needed to echo the right ID back.
pub(crate) struct StreamResolver {
    /// The item UUID from the URL route (Movie/Episode/Track).
    pub item_id: Uuid,
    /// When the requested source was a StreamGroup: (group_uuid, display_title, all_candidates).
    pub group: Option<(Uuid, String, Vec<db::Media>)>,
    /// The concrete playable stream.
    pub stream: db::Media,
}

impl StreamResolver {
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
    /// Use this in the playbackinfo handler where `all_source_medias` building happens externally.
    /// `initial` is typically the result of `MediaResolveService::resolve_item`.
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
                })
            }
            _ => Ok(Self {
                item_id,
                group: None,
                stream: media,
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

    /// Probe fallback pool — includes cascade candidates for group requests, empty otherwise.
    pub fn candidates(&self) -> &[db::Media] {
        self.group
            .as_ref()
            .map(|(_, _, c)| c.as_slice())
            .unwrap_or(&[])
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
