//! Eclipse music addons.
//!
//! An Eclipse addon serves music over a handful of flat HTTP routes; see
//! [`remux_sdks::eclipse`] for the protocol and <https://eclipsemusic.app/docs>
//! for the specification. Any addon speaking it can be installed by URL via the
//! generic [`EclipsePreset`]; the named presets below are the same thing with a
//! default URL pre-filled.
//!
//! # Identity
//!
//! Eclipse item ids are opaque strings minted by the addon and meaningful only
//! to it — two addons may both serve `"track_123"` as different songs. Every id
//! is therefore namespaced with the addon instance's UUID before it reaches
//! `db::ExternalIds::eclipse_id` (see [`scoped_id`]). ISRC, when the addon sends
//! one, is stored unscoped: it identifies a recording across every provider, so
//! the same song from two Eclipse addons dedupes onto one row.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::stream::{self, Stream, StreamExt};
use std::{pin::Pin, sync::Arc};
use tracing::debug;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, CatalogAddon, CatalogInfo, MediaKind,
    ResourceType, SearchAddon, StreamAddon, TreeAddon, stremio::StremioManifestUrl,
};
use crate::{
    AppContext, common, db, sdks,
    sdks::eclipse as ec,
    services::eclipse as eclipse_service,
    stream::{StreamDescriptor, StreamInfo},
};

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Namespaces an addon-scoped Eclipse id with the addon instance it came from.
///
/// Without this, two installed Eclipse addons that both mint `"track_123"`
/// would derive the same UUID and collapse into one row playing whichever
/// addon answered last.
fn scoped_id(addon_id: Uuid, raw: &str) -> String {
    format!("{addon_id}:{raw}")
}

/// Recovers the addon's own id from a scoped one. Returns the whole string when
/// it carries no scope prefix, so a row written before scoping still resolves.
fn unscope(addon_id: Uuid, scoped: &str) -> String {
    scoped
        .strip_prefix(&format!("{addon_id}:"))
        .unwrap_or(scoped)
        .to_string()
}

/// The Eclipse id this media was created from, if any.
fn eclipse_id_of(addon_id: Uuid, media: &db::Media) -> Option<String> {
    media
        .external_ids
        .eclipse_id
        .as_deref()
        .map(|scoped| unscope(addon_id, scoped))
}

/// Stable UUID for an Eclipse item.
///
/// Tracks key on ISRC when present so the same recording served by two
/// different addons — or by Eclipse and Deezer — lands on one row; everything
/// else keys on the scoped id. This mirrors the priority in
/// `MediaIdRaw::canonical`, which is what actually derives persisted UUIDs.
fn item_uuid(
    kind: &db::MediaKind,
    addon_id: Uuid,
    raw_id: &str,
    isrc: Option<&str>,
) -> Uuid {
    match (kind, isrc) {
        (db::MediaKind::Track, Some(isrc)) => {
            common::stable_media_uuid(kind, &format!("isrc:{isrc}"))
        }
        _ => common::stable_media_uuid(kind, &scoped_id(addon_id, raw_id)),
    }
}

// ---------------------------------------------------------------------------
// Mapping
// ---------------------------------------------------------------------------

/// Builds the `ExternalIds` for an Eclipse item. `artist`/`album` names ride
/// along in the flat fields so a track with no parent rows (a search hit, a
/// playlist member) still renders and can still be re-searched later.
fn ids(
    addon_id: Uuid,
    raw_id: &str,
    isrc: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
) -> db::ExternalIds {
    db::ExternalIds {
        eclipse_id: Some(scoped_id(addon_id, raw_id)),
        isrc: isrc.map(str::to_string),
        artist_name: artist.map(str::to_string),
        album_title: album.map(str::to_string),
        ..Default::default()
    }
}

/// A track, standing on its own (a search hit or catalog row).
///
/// `parent`/`grandparent` are left unset: Eclipse search results carry an album
/// and artist *name* but no id for either, so there is nothing to point at. The
/// names live in `external_ids`, which `Media::artist_name`/`album_name` read.
fn track_to_media(addon_id: Uuid, t: &ec::Track) -> db::Media {
    let mut media = db::Media {
        id: item_uuid(
            &db::MediaKind::Track,
            addon_id,
            &t.id,
            t.isrc
                .as_deref(),
        ),
        title: t
            .title
            .clone(),
        kind: db::MediaKind::Track,
        runtime: t.seconds(),
        description: t
            .artist
            .as_ref()
            .map(|a| format!("by {a}")),
        external_ids: ids(
            addon_id,
            &t.id,
            t.isrc
                .as_deref(),
            t.artist
                .as_deref(),
            t.album
                .as_deref(),
        ),
        ..Default::default()
    };
    if let Some(url) = t
        .artwork_url
        .clone()
    {
        media.set_image(db::ImageKind::Primary, url);
    }
    media
}

/// A track known to belong to `album_id`, whose own artwork may be absent
/// because the album carries it.
fn album_track_to_media(
    addon_id: Uuid,
    t: &ec::Track,
    album: &db::Media,
    idx: usize,
) -> db::Media {
    let mut media = track_to_media(addon_id, t);
    media.parent_id = Some(album.id);
    media.grandparent_id = album.grandparent_id;
    // Eclipse album payloads have no track numbers; position in the returned
    // list is the only ordering the addon gives us.
    media.idx = Some(idx as i64 + 1);
    if media
        .external_ids
        .album_title
        .is_none()
    {
        media
            .external_ids
            .album_title = Some(
            album
                .title
                .clone(),
        );
    }
    if media
        .images
        .primary
        .is_empty()
    {
        if let Some(url) = album
            .images
            .primary
            .first()
            .map(|i| {
                i.path
                    .clone()
            })
        {
            media.set_image(db::ImageKind::Primary, url);
        }
    }
    media
}

fn album_to_media(addon_id: Uuid, a: &ec::Album) -> db::Media {
    let mut media = db::Media {
        id: item_uuid(&db::MediaKind::Album, addon_id, &a.id, None),
        title: a
            .title
            .clone(),
        kind: db::MediaKind::Album,
        description: a
            .description
            .clone()
            .or_else(|| {
                a.artist
                    .as_ref()
                    .map(|artist| format!("by {artist}"))
            }),
        released_at: a
            .year
            .and_then(year_to_datetime),
        song_count: a.track_count,
        external_ids: ids(
            addon_id,
            &a.id,
            None,
            a.artist
                .as_deref(),
            None,
        ),
        ..Default::default()
    };
    if let Some(url) = a
        .artwork_url
        .clone()
    {
        media.set_image(db::ImageKind::Primary, url);
    }
    media
}

fn artist_to_media(addon_id: Uuid, a: &ec::Artist) -> db::Media {
    let mut media = db::Media {
        id: item_uuid(&db::MediaKind::Artist, addon_id, &a.id, None),
        title: a
            .name
            .clone(),
        kind: db::MediaKind::Artist,
        description: a
            .bio
            .clone(),
        tags: a
            .genres
            .clone(),
        external_ids: ids(addon_id, &a.id, None, None, None),
        ..Default::default()
    };
    if let Some(url) = a
        .artwork_url
        .clone()
    {
        media.set_image(db::ImageKind::Primary, url);
    }
    media
}

/// A playlist, with its tracks attached as `Playlist`-role relations — the
/// shape the import pipeline's `save_pending_relations` links as members.
fn playlist_to_media(addon_id: Uuid, p: &ec::Playlist) -> db::Media {
    let playlist_id = item_uuid(&db::MediaKind::Playlist, addon_id, &p.id, None);
    let relations: Vec<(db::MediaRelation, db::Media)> = p
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let track = track_to_media(addon_id, t);
            let relation = db::MediaRelation {
                relation_id: Uuid::new_v5(
                    &playlist_id,
                    track
                        .id
                        .as_bytes(),
                ),
                left_media_id: playlist_id,
                right_media_id: track.id,
                weight: Some(i as i64),
                role: Some(db::RelationRole::Playlist),
                ..Default::default()
            };
            (relation, track)
        })
        .collect();

    let mut media = db::Media {
        id: playlist_id,
        title: p
            .title
            .clone(),
        kind: db::MediaKind::Playlist,
        collection_media_kind: Some(db::CollectionMediaKind::Music),
        description: p
            .description
            .clone()
            .or_else(|| {
                p.creator
                    .as_ref()
                    .map(|c| format!("by {c}"))
            }),
        song_count: p
            .track_count
            .or(Some(
                p.tracks
                    .len() as i64,
            )),
        external_ids: ids(
            addon_id,
            &p.id,
            None,
            p.creator
                .as_deref(),
            None,
        ),
        relations: (!relations.is_empty()).then_some(relations),
        refreshed_at: Some(chrono::Utc::now().naive_utc()),
        ..Default::default()
    };
    if let Some(url) = p
        .artwork_url
        .clone()
    {
        media.set_image(db::ImageKind::Primary, url);
    }
    media
}

/// A catalog row entry. Its `kind` decides which detail endpoint opens it, so
/// the mapping funnels through the same per-kind builders as everything else.
fn catalog_item_to_media(addon_id: Uuid, item: &ec::CatalogItem) -> db::Media {
    match item.kind {
        ec::ItemType::Track | ec::ItemType::File => track_to_media(
            addon_id,
            &ec::Track {
                id: item
                    .id
                    .clone(),
                title: item
                    .title
                    .clone(),
                artist: item
                    .artist
                    .clone(),
                album: item
                    .album
                    .clone(),
                duration_ms: item.duration_ms,
                artwork_url: item
                    .artwork_url
                    .clone(),
                isrc: item
                    .isrc
                    .clone(),
                ..Default::default()
            },
        ),
        ec::ItemType::Album => album_to_media(
            addon_id,
            &ec::Album {
                id: item
                    .id
                    .clone(),
                title: item
                    .title
                    .clone(),
                artist: item
                    .artist
                    .clone(),
                artwork_url: item
                    .artwork_url
                    .clone(),
                year: item.year,
                ..Default::default()
            },
        ),
        ec::ItemType::Artist => artist_to_media(
            addon_id,
            &ec::Artist {
                id: item
                    .id
                    .clone(),
                name: item
                    .title
                    .clone(),
                artwork_url: item
                    .artwork_url
                    .clone(),
                ..Default::default()
            },
        ),
        ec::ItemType::Playlist => playlist_to_media(
            addon_id,
            &ec::Playlist {
                id: item
                    .id
                    .clone(),
                title: item
                    .title
                    .clone(),
                artwork_url: item
                    .artwork_url
                    .clone(),
                creator: item
                    .artist
                    .clone(),
                ..Default::default()
            },
        ),
    }
}

/// January 1st of `year`, which is all the precision Eclipse gives for albums.
fn year_to_datetime(year: i64) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDate::from_ymd_opt(year as i32, 1, 1)
        .map(|d| d.and_hms_opt(0, 0, 0))
        .flatten()
}

// ---------------------------------------------------------------------------
// Addon
// ---------------------------------------------------------------------------

pub struct EclipseAddon {
    addon_id: Uuid,
    manifest_url: StremioManifestUrl,
}

impl EclipseAddon {
    fn service(&self) -> Result<eclipse_service::EclipseService> {
        Ok(
            eclipse_service::EclipseService::from_url(&self.manifest_url)?
                .with_shared_rate_limit(common::addon_rate_limit(self.addon_id)),
        )
    }

    /// The addon's own id for `media`, resolving by ISRC when the row carries
    /// one this addon has never seen (a track that arrived from Deezer, TMDB, or
    /// another Eclipse addon).
    ///
    /// Falls back to a text search scored on identity rather than trusting the
    /// first hit: playing a close-but-wrong recording is worse than playing
    /// nothing.
    async fn resolve_track_id(
        &self,
        svc: &eclipse_service::EclipseService,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<String>> {
        if let Some(id) = eclipse_id_of(self.addon_id, media) {
            return Ok(Some(id));
        }

        let manifest = svc
            .get_manifest()
            .await?;

        // An ISRC is an exact recording identity; ask for it directly when the
        // addon supports either lookup.
        if let Some(isrc) = media
            .external_ids
            .isrc
            .as_deref()
        {
            if manifest.has(&ec::Resource::Isrc) {
                if let Some(id) = svc
                    .resolve_isrc(isrc)
                    .await?
                {
                    return Ok(Some(id));
                }
            }
            if manifest.has(&ec::Resource::Resolve) {
                if let Some(item) = svc
                    .resolve(
                        Some(isrc),
                        &media.title,
                        media
                            .artist_name()
                            .unwrap_or_default(),
                        media
                            .runtime
                            .map(|s| s * 1000),
                    )
                    .await?
                {
                    return Ok(Some(item.id));
                }
            }
        }

        let artist = self
            .artist_name_for(media, ctx)
            .await;

        if manifest.has(&ec::Resource::Resolve) {
            if let Some(item) = svc
                .resolve(
                    media
                        .external_ids
                        .isrc
                        .as_deref(),
                    &media.title,
                    artist
                        .as_deref()
                        .unwrap_or_default(),
                    media
                        .runtime
                        .map(|s| s * 1000),
                )
                .await?
            {
                return Ok(Some(item.id));
            }
        }

        let query = media.track_search_query_from(artist.as_deref());
        let hits = svc
            .search(&query)
            .await?
            .tracks;
        Ok(best_match(media, artist.as_deref(), &hits).map(|t| {
            t.id.clone()
        }))
    }

    /// The artist name for a track, preferring the artist row (a track's
    /// grandparent) and falling back to the flat name on the row itself.
    async fn artist_name_for(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Option<String> {
        let from_row = match media.grandparent_id {
            Some(gp_id) => db::Media::get_by_id(&ctx.db, &gp_id)
                .await
                .ok()
                .flatten()
                .map(|m| m.title),
            None => None,
        };
        from_row.or_else(|| {
            media
                .artist_name()
                .map(str::to_string)
        })
    }
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// Minimum score for a search hit to count as the same recording.
///
/// A title agreement alone (2) is not enough; a title plus either the artist or
/// a close duration is.
const MATCH_THRESHOLD: u32 = 4;

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Scores how confidently `hit` is the recording `media` asks for.
///
/// An ISRC agreement wins outright. Otherwise the title must agree, and the
/// artist and duration corroborate it. Deliberately strict: the caller plays
/// whatever this returns without further checks, so a wrong answer here is
/// worse than no answer.
fn match_score(media: &db::Media, artist: Option<&str>, hit: &ec::Track) -> u32 {
    if let (Some(want), Some(got)) = (
        media
            .external_ids
            .isrc
            .as_deref(),
        hit.isrc
            .as_deref(),
    ) {
        if want == got {
            return u32::MAX;
        }
        // Both sides named a recording and they disagree — it is a different
        // recording, whatever the title says.
        return 0;
    }

    let want_title = normalize(&media.title);
    let got_title = normalize(&hit.title);
    if want_title.is_empty() || got_title.is_empty() {
        return 0;
    }
    let mut score = if want_title == got_title {
        3
    } else if got_title.starts_with(&want_title) || want_title.starts_with(&got_title) {
        // A remaster/live suffix is still the same title.
        2
    } else {
        return 0;
    };

    if let (Some(want), Some(got)) = (
        artist,
        hit.artist
            .as_deref(),
    ) {
        let (want, got) = (normalize(want), normalize(got));
        if !want.is_empty()
            && (want == got || got.contains(&want) || want.contains(&got))
        {
            score += 2;
        }
    }

    if let (Some(want), Some(got)) = (media.runtime, hit.seconds()) {
        if want > 0 && (want - got).abs() <= 5 {
            score += 2;
        }
    }

    if let (Some(want), Some(got)) = (
        media.album_name(),
        hit.album
            .as_deref(),
    ) {
        if normalize(want) == normalize(got) {
            score += 1;
        }
    }

    score
}

/// The best hit that clears [`MATCH_THRESHOLD`], or `None`.
fn best_match<'a>(
    media: &db::Media,
    artist: Option<&str>,
    hits: &'a [ec::Track],
) -> Option<&'a ec::Track> {
    hits.iter()
        .map(|hit| (match_score(media, artist, hit), hit))
        .filter(|(score, _)| *score >= MATCH_THRESHOLD)
        .max_by_key(|(score, _)| *score)
        .map(|(_, hit)| hit)
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

#[async_trait]
impl AddonKind for EclipseAddon {
    fn id(&self) -> &'static str {
        "eclipse"
    }

    /// Reports what the live manifest declares, translated into the
    /// resource/type vocabulary the addon service and dashboard already speak.
    async fn available_info(
        &self,
    ) -> Result<
        Option<(
            Vec<sdks::stremio::ResourceRef>,
            Vec<sdks::stremio::MediaType>,
        )>,
    > {
        let manifest = self
            .service()?
            .get_manifest()
            .await?;
        Ok(Some(manifest_info(&manifest)))
    }
}

/// Maps an Eclipse manifest onto the resources and types remux tracks.
///
/// `catalog` covers both the detail endpoints and the catalog rows in Eclipse's
/// vocabulary, so it also implies `meta` — that is the resource gating the
/// browse/tree path that those detail endpoints serve.
fn manifest_info(
    manifest: &ec::Manifest,
) -> (
    Vec<sdks::stremio::ResourceRef>,
    Vec<sdks::stremio::MediaType>,
) {
    let mut resources = Vec::new();
    let mut push = |r: ResourceType| {
        if !resources
            .iter()
            .any(|existing: &sdks::stremio::ResourceRef| existing.name == r)
        {
            resources.push(AddonMetadata::simple_resource(r));
        }
    };

    if manifest.has(&ec::Resource::Stream) {
        push(ResourceType::Stream);
    }
    if manifest.has(&ec::Resource::Search) {
        push(ResourceType::Search);
    }
    if manifest.has(&ec::Resource::Catalog) {
        push(ResourceType::Catalog);
        push(ResourceType::Meta);
    }

    let types = manifest
        .types
        .iter()
        .filter_map(|t| match t {
            ec::ItemType::Track | ec::ItemType::File => {
                Some(sdks::stremio::MediaType::Track)
            }
            ec::ItemType::Album => Some(sdks::stremio::MediaType::Album),
            ec::ItemType::Artist => Some(sdks::stremio::MediaType::Artist),
            ec::ItemType::Playlist => {
                Some(sdks::stremio::MediaType::Other("playlist".to_string()))
            }
        })
        .collect::<Vec<_>>();

    // An addon that declares no types still serves tracks; reporting an empty
    // list would make `supports_type` fall back to the preset's static list.
    let types = if types.is_empty() {
        vec![sdks::stremio::MediaType::Track]
    } else {
        types
    };

    (resources, types)
}

#[async_trait]
impl StreamAddon for EclipseAddon {
    fn supports(&self, media: &db::Media) -> bool {
        // Only a track is playable. Albums and artists are containers whose
        // children carry the streams.
        matches!(media.kind, db::MediaKind::Track)
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        _id_prefixes: Option<&[String]>,
    ) -> Result<Vec<StreamInfo>> {
        let svc = self.service()?;
        let Some(track_id) = self
            .resolve_track_id(&svc, media, ctx)
            .await?
        else {
            debug!(media_id = %media.id, title = %media.title, "eclipse: no matching track");
            return Ok(vec![]);
        };

        let stream = svc
            .get_stream(&track_id)
            .await?;
        if stream
            .url
            .is_empty()
        {
            return Err(anyhow!("eclipse addon returned an empty stream url"));
        }
        Ok(vec![stream_to_info(&stream, media)])
    }
}

/// Builds the stream entry, naming it by the quality/format the addon reported
/// so a user choosing between sources sees what differs between them.
fn stream_to_info(stream: &ec::Stream, media: &db::Media) -> StreamInfo {
    let mut label = Vec::new();
    if let Some(q) = stream
        .quality
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        label.push(q.to_string());
    }
    if let Some(codec) = stream
        .codec
        .as_deref()
        .or(stream
            .format
            .as_deref())
        .filter(|s| !s.is_empty())
    {
        label.push(codec.to_uppercase());
    }
    if let Some(hz) = stream.sample_rate {
        match stream.bit_depth {
            Some(bits) => label.push(format!("{}kHz/{}bit", hz / 1000, bits)),
            None => label.push(format!("{}kHz", hz / 1000)),
        }
    }

    StreamInfo {
        descriptor: StreamDescriptor::http(
            stream
                .url
                .clone(),
        ),
        name: Some(if label.is_empty() {
            "Eclipse".to_string()
        } else {
            format!("Eclipse · {}", label.join(" · "))
        }),
        description: media
            .artist_name()
            .map(str::to_string),
        duration: media.runtime,
        ..Default::default()
    }
}

#[async_trait]
impl SearchAddon for EclipseAddon {
    async fn search_supports(&self, kind: &db::MediaKind) -> bool {
        matches!(
            kind,
            db::MediaKind::Track
                | db::MediaKind::Album
                | db::MediaKind::Artist
                | db::MediaKind::Playlist
        )
    }

    /// Eclipse answers one `/search` call with every type at once, so the
    /// per-kind calls the search service makes each pick their slice out of the
    /// same (cached) response rather than issuing one request per kind.
    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        if !SearchAddon::search_supports(self, kind).await {
            return Ok(None);
        }
        let results = self
            .service()?
            .search(query)
            .await?;

        let mut media: Vec<db::Media> = match kind {
            db::MediaKind::Track => results
                .tracks
                .iter()
                .map(|t| track_to_media(self.addon_id, t))
                .collect(),
            db::MediaKind::Album => results
                .albums
                .iter()
                .map(|a| album_to_media(self.addon_id, a))
                .collect(),
            db::MediaKind::Artist => results
                .artists
                .iter()
                .map(|a| artist_to_media(self.addon_id, a))
                .collect(),
            db::MediaKind::Playlist => results
                .playlists
                .iter()
                .map(|p| playlist_to_media(self.addon_id, p))
                .collect(),
            _ => return Ok(None),
        };
        media.truncate(limit);
        db::Media::preload_parents(&ctx.db, &mut media).await;
        Ok(Some(media))
    }
}

#[async_trait]
impl CatalogAddon for EclipseAddon {
    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        let manifest = self
            .service()?
            .get_manifest()
            .await?;
        Ok(manifest
            .catalogs
            .iter()
            .map(|c| CatalogInfo {
                media_kind: Some(item_type_to_kind(c.kind)),
                collection_media_kind: Some(db::CollectionMediaKind::Music),
                ..CatalogInfo::new(
                    c.id.clone(),
                    c.name
                        .clone(),
                )
            })
            .collect())
    }

    async fn catalog_stream(
        &self,
        _ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        let svc = self.service()?;
        let manifest = svc
            .get_manifest()
            .await?;
        if !manifest
            .catalogs
            .iter()
            .any(|c| c.id == local_id)
        {
            return Ok(None);
        }

        let addon_id = self.addon_id;
        let items = svc
            .get_catalog_stream(local_id.to_string())
            .await?
            .map(move |item| catalog_item_to_media(addon_id, &item));
        Ok(Some(Box::pin(items)))
    }
}

fn item_type_to_kind(t: ec::ItemType) -> db::MediaKind {
    match t {
        ec::ItemType::Track | ec::ItemType::File => db::MediaKind::Track,
        ec::ItemType::Album => db::MediaKind::Album,
        ec::ItemType::Artist => db::MediaKind::Artist,
        ec::ItemType::Playlist => db::MediaKind::Playlist,
    }
}

#[async_trait]
impl TreeAddon for EclipseAddon {
    fn supports(&self, root: &db::Media) -> bool {
        matches!(
            root.kind,
            db::MediaKind::Album | db::MediaKind::Artist | db::MediaKind::Playlist
        ) && root
            .external_ids
            .eclipse_id
            .is_some()
    }

    /// Expands a container into its children: an album into its tracks, an
    /// artist into its albums, a playlist into its tracks.
    async fn get_children(
        &self,
        root: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        if !TreeAddon::supports(self, root) {
            return Ok(None);
        }
        let Some(raw_id) = eclipse_id_of(self.addon_id, root) else {
            return Ok(None);
        };
        let svc = self.service()?;

        let children: Vec<db::Media> = match root.kind {
            db::MediaKind::Album => {
                let Some(album) = svc
                    .get_album(&raw_id)
                    .await?
                else {
                    return Ok(None);
                };
                album
                    .tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| album_track_to_media(self.addon_id, t, root, i))
                    .collect()
            }
            db::MediaKind::Artist => {
                let Some(artist) = svc
                    .get_artist(&raw_id)
                    .await?
                else {
                    return Ok(None);
                };
                artist
                    .albums
                    .iter()
                    .map(|a| {
                        let mut album = album_to_media(self.addon_id, a);
                        album.grandparent_id = Some(root.id);
                        if album
                            .external_ids
                            .artist_name
                            .is_none()
                        {
                            album
                                .external_ids
                                .artist_name = Some(
                                root.title
                                    .clone(),
                            );
                        }
                        album
                    })
                    .collect()
            }
            db::MediaKind::Playlist => {
                let Some(playlist) = svc
                    .get_playlist(&raw_id)
                    .await?
                else {
                    return Ok(None);
                };
                playlist
                    .tracks
                    .iter()
                    .map(|t| track_to_media(self.addon_id, t))
                    .collect()
            }
            _ => return Ok(None),
        };

        if children.is_empty() {
            Ok(None)
        } else {
            Ok(Some(children))
        }
    }
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Every type an Eclipse addon can serve. The live manifest narrows this at
/// load time via [`AddonKind::available_info`]; this is only the fallback for
/// an addon whose manifest could not be fetched.
fn all_types() -> Vec<MediaKind> {
    vec![
        MediaKind::Track,
        MediaKind::Album,
        MediaKind::Artist,
        MediaKind::Playlist,
    ]
}

fn metadata(
    id: &str,
    display_name: &str,
    description: &str,
    options: Vec<AddonOption>,
) -> AddonMetadata {
    AddonMetadata {
        id: id.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        icon: None,
        supported_resources: vec![
            AddonMetadata::simple_resource(ResourceType::Stream),
            AddonMetadata::simple_resource(ResourceType::Search),
            AddonMetadata::simple_resource(ResourceType::Catalog),
            AddonMetadata::simple_resource(ResourceType::Meta),
        ],
        supported_types: all_types(),
        supported_resources_user: vec![ResourceType::Stream, ResourceType::Search],
        supported_types_user: all_types(),
        options,
    }
}

fn manifest_option(default: Option<&str>, description: String) -> AddonOption {
    AddonOption {
        id: "manifest_url".to_string(),
        name: "Manifest URL".to_string(),
        description: Some(description),
        required: default.is_none(),
        default: default.map(|d| serde_json::Value::String(d.to_string())),
        kind: AddonOptionType::Url,
    }
}

/// Builds the capabilities for any Eclipse addon URL.
///
/// All five capabilities are attached unconditionally; which ones actually run
/// is decided by the resources the manifest declares (translated in
/// [`manifest_info`]) intersected with the resources the operator enabled on the
/// stored addon row.
fn capabilities(
    addon_id: Uuid,
    default_url: Option<&str>,
    cfg: &serde_json::Value,
) -> Result<AddonCapabilities> {
    let raw_url = cfg
        .get("manifest_url")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| default_url.map(str::to_string))
        .ok_or_else(|| anyhow!("Eclipse addon missing manifest_url in config"))?;
    let manifest_url = StremioManifestUrl::try_new(raw_url)
        .map_err(|e| anyhow!("Invalid manifest_url: {e}"))?;
    let addon = Arc::new(EclipseAddon {
        addon_id,
        manifest_url,
    });
    Ok(AddonCapabilities {
        kind: Some(addon.clone()),
        stream: Some(addon.clone()),
        search: Some(addon.clone()),
        catalog: Some(addon.clone()),
        tree: Some(addon),
        ..Default::default()
    })
}

/// Any addon speaking the Eclipse protocol, installed by URL.
pub struct EclipsePreset;

inventory::submit! {
    AddonPresetRegistration(|| Box::new(EclipsePreset))
}

impl AddonPreset for EclipsePreset {
    fn id(&self) -> &'static str {
        "eclipse"
    }

    fn metadata(&self) -> AddonMetadata {
        metadata(
            "eclipse",
            "Eclipse addon",
            "Any addon that speaks the Eclipse music addon protocol \
             (manifest.json + /search + /stream). Provides music search, \
             playback, and — when the addon implements them — browsable \
             albums, artists, playlists and catalog rows.",
            vec![manifest_option(
                None,
                "Full URL to the addon's manifest.json".to_string(),
            )],
        )
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        capabilities(addon_id, None, cfg)
    }
}

/// A named Eclipse addon: the generic preset with a default URL pre-filled, so
/// it can be installed without the operator pasting one.
macro_rules! named_eclipse_preset {
    (
        $preset:ident,
        id = $id:literal,
        name = $name:literal,
        description = $description:literal,
        url = $url:literal,
        generate_url = $generate_url:literal $(,)?
    ) => {
        pub struct $preset;

        inventory::submit! {
            AddonPresetRegistration(|| Box::new($preset))
        }

        impl AddonPreset for $preset {
            fn id(&self) -> &'static str {
                $id
            }

            fn metadata(&self) -> AddonMetadata {
                metadata(
                    $id,
                    $name,
                    $description,
                    vec![manifest_option(
                        Some($url),
                        concat!(
                            "Optional. You can generate a new manifest URL at ",
                            $generate_url
                        )
                        .to_string(),
                    )],
                )
            }

            fn from_cfg(
                &self,
                addon_id: Uuid,
                cfg: &serde_json::Value,
                _config: &crate::Config,
            ) -> Result<AddonCapabilities> {
                capabilities(addon_id, Some($url), cfg)
            }
        }
    };
}

named_eclipse_preset!(
    MonochromePreset,
    id = "monochrome",
    name = "Monochrome",
    description = "Search and stream music.",
    url = "https://monochrome1.cyrusna29.workers.dev/u/206f62ce5c9a5c710f2178a16238/manifest.json",
    generate_url = "https://monochrome1.cyrusna29.workers.dev",
);

named_eclipse_preset!(
    SpotiFLACPreset,
    // Must stay "eclipse_spotiflac": persisted addon rows key on this exact
    // string, and AddonService::load_runtimes does an exact-match lookup that
    // silently skips a row whose preset id it doesn't recognize.
    id = "eclipse_spotiflac",
    name = "SpotiFLAC",
    description = "Search and stream lossless music.",
    url = "https://spotiflac.eclipsemusic.app/5baa7290b334d6e2/manifest.json",
    generate_url = "https://spotiflac.eclipsemusic.app",
);

#[cfg(test)]
mod tests {
    use super::*;

    fn addon_uuid() -> Uuid {
        Uuid::from_u128(0x1234)
    }

    /// A mock addon server plus a base URL whose path is unique to this test.
    ///
    /// The SDK response cache is process-wide and keyed by URL, while httpmock
    /// reuses ports across tests — so two tests mocking the same path on "their
    /// own" server can collide, and one reads the other's cached response. A
    /// distinct path prefix per test keeps the keys apart.
    struct MockAddon {
        server: httpmock::MockServer,
        base: String,
        scope: String,
    }

    impl MockAddon {
        fn start() -> Self {
            static N: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let server = httpmock::MockServer::start();
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let scope = format!("/e{n}");
            let base = format!("{}{scope}", server.base_url());
            Self {
                server,
                base,
                scope,
            }
        }

        /// Path within this test's scope, e.g. `path("/search")`.
        fn path(&self, suffix: &str) -> String {
            format!("{}{suffix}", self.scope)
        }

        fn addon(&self) -> EclipseAddon {
            addon(&self.base)
        }
    }

    fn addon(base: &str) -> EclipseAddon {
        EclipseAddon {
            addon_id: addon_uuid(),
            manifest_url: StremioManifestUrl::try_new(base.to_string()).unwrap(),
        }
    }

    fn track(id: &str, title: &str, artist: &str) -> ec::Track {
        ec::Track {
            id: id.to_string(),
            title: title.to_string(),
            artist: Some(artist.to_string()),
            ..Default::default()
        }
    }

    fn db_track(title: &str) -> db::Media {
        db::Media {
            title: title.to_string(),
            kind: db::MediaKind::Track,
            ..Default::default()
        }
    }

    // -- identity ----------------------------------------------------------

    /// Two addons serving the same raw id must not collapse onto one row.
    #[test]
    fn the_same_raw_id_from_two_addons_is_two_items() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_ne!(
            item_uuid(&db::MediaKind::Track, a, "track_1", None),
            item_uuid(&db::MediaKind::Track, b, "track_1", None)
        );
    }

    /// The same recording from two addons *must* collapse onto one row, which is
    /// what makes playback fall back between sources and keeps history intact.
    #[test]
    fn the_same_isrc_from_two_addons_is_one_item() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_eq!(
            item_uuid(&db::MediaKind::Track, a, "track_1", Some("USRC11903813")),
            item_uuid(
                &db::MediaKind::Track,
                b,
                "different_id",
                Some("USRC11903813")
            )
        );
    }

    #[test]
    fn scoping_round_trips() {
        let id = addon_uuid();
        assert_eq!(unscope(id, &scoped_id(id, "track_1")), "track_1");
        // A raw id containing the separator survives, since only the leading
        // scope is stripped.
        assert_eq!(unscope(id, &scoped_id(id, "a:b:c")), "a:b:c");
        // An unscoped value is returned whole rather than mangled.
        assert_eq!(unscope(id, "track_1"), "track_1");
    }

    // -- matching ----------------------------------------------------------

    /// A disagreeing ISRC means a different recording, whatever the title says.
    #[test]
    fn a_conflicting_isrc_never_matches() {
        let mut want = db_track("Hello");
        want.external_ids
            .isrc = Some("USRC11903813".into());
        let hit = ec::Track {
            isrc: Some("GBAAW9500189".into()),
            ..track("t1", "Hello", "Adele")
        };

        assert_eq!(match_score(&want, Some("Adele"), &hit), 0);
        assert!(best_match(&want, Some("Adele"), &[hit]).is_none());
    }

    #[test]
    fn an_agreeing_isrc_wins_outright() {
        let mut want = db_track("Whatever The Title Says");
        want.external_ids
            .isrc = Some("USRC11903813".into());
        let hit = ec::Track {
            isrc: Some("USRC11903813".into()),
            ..track("t1", "Completely Different", "Nobody")
        };

        assert_eq!(
            best_match(&want, Some("Adele"), &[hit]).map(|t| t
                .id
                .as_str()),
            Some("t1")
        );
    }

    /// A title match alone is not enough to play something: the docs are
    /// explicit that a close-but-wrong recording is worse than nothing.
    #[test]
    fn a_title_alone_does_not_clear_the_threshold() {
        let want = db_track("Hello");
        let hit = ec::Track {
            artist: None,
            ..track("t1", "Hello", "")
        };
        assert!(best_match(&want, None, &[hit]).is_none());
    }

    #[test]
    fn title_and_artist_agreement_matches() {
        let want = db_track("Hello");
        let hits = vec![track("t1", "Hello", "Adele")];
        assert_eq!(
            best_match(&want, Some("Adele"), &hits).map(|t| t
                .id
                .as_str()),
            Some("t1")
        );
    }

    /// Punctuation and case differ constantly between catalogs.
    #[test]
    fn matching_ignores_case_and_punctuation() {
        let want = db_track("Don't Stop Me Now!");
        let hits = vec![track("t1", "dont stop me now", "Queen")];
        assert!(best_match(&want, Some("queen"), &hits).is_some());
    }

    /// Duration corroborates a title when no artist is known, and picks the
    /// right cut when several versions share a title.
    #[test]
    fn duration_distinguishes_versions_of_one_title() {
        let mut want = db_track("Hello");
        want.runtime = Some(240);
        let hits = vec![
            ec::Track {
                duration: Some(600),
                ..track("extended", "Hello", "Adele")
            },
            ec::Track {
                duration: Some(241),
                ..track("original", "Hello", "Adele")
            },
        ];
        assert_eq!(
            best_match(&want, Some("Adele"), &hits).map(|t| t
                .id
                .as_str()),
            Some("original")
        );
    }

    #[test]
    fn a_different_title_never_matches() {
        let want = db_track("Hello");
        let hits = vec![track("t1", "Goodbye", "Adele")];
        assert!(best_match(&want, Some("Adele"), &hits).is_none());
    }

    // -- manifest translation ----------------------------------------------

    #[test]
    fn manifest_resources_become_remux_resources() {
        let manifest: ec::Manifest = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "A", "version": "1.0.0",
            "resources": ["search", "stream", "catalog"],
            "types": ["track", "album", "artist", "playlist"],
        }))
        .unwrap();

        let (resources, types) = manifest_info(&manifest);
        let names: Vec<_> = resources
            .iter()
            .map(|r| {
                r.name
                    .clone()
            })
            .collect();

        assert!(names.contains(&ResourceType::Stream));
        assert!(names.contains(&ResourceType::Search));
        assert!(names.contains(&ResourceType::Catalog));
        // `catalog` in Eclipse also covers the detail endpoints, which is what
        // the Meta resource gates in remux.
        assert!(names.contains(&ResourceType::Meta));
        assert_eq!(types.len(), 4);
    }

    /// A search-only addon must still be usable: an absent `types` list means
    /// tracks, and reporting nothing would make the addon serve nothing.
    #[test]
    fn a_manifest_without_types_still_serves_tracks() {
        let manifest: ec::Manifest = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "A", "version": "1.0.0",
            "resources": ["search", "stream"],
        }))
        .unwrap();

        let (resources, types) = manifest_info(&manifest);
        let names: Vec<_> = resources
            .iter()
            .map(|r| {
                r.name
                    .clone()
            })
            .collect();

        assert!(!names.contains(&ResourceType::Catalog));
        assert!(!names.contains(&ResourceType::Meta));
        assert_eq!(types, vec![sdks::stremio::MediaType::Track]);
    }

    // -- mapping -----------------------------------------------------------

    /// A search hit has an album and artist *name* but no id for either, so the
    /// names must survive on the row itself or the track renders bare.
    #[test]
    fn a_track_keeps_its_artist_and_album_names() {
        let media = track_to_media(
            addon_uuid(),
            &ec::Track {
                album: Some("Small Hours".into()),
                duration: Some(240),
                isrc: Some("USRC11903813".into()),
                ..track("t1", "Late Night", "Some Artist")
            },
        );

        assert_eq!(media.kind, db::MediaKind::Track);
        assert_eq!(media.runtime, Some(240));
        assert_eq!(media.artist_name(), Some("Some Artist"));
        assert_eq!(media.album_name(), Some("Small Hours"));
        assert_eq!(
            media
                .external_ids
                .isrc
                .as_deref(),
            Some("USRC11903813")
        );
        assert_eq!(
            media
                .external_ids
                .eclipse_id
                .as_deref(),
            Some(scoped_id(addon_uuid(), "t1").as_str())
        );
        // Persistable without any Deezer or YouTube id.
        assert!(
            media
                .validate()
                .is_ok()
        );
    }

    /// Album tracks come back without positions; list order is the only
    /// ordering the addon gives, and without it a track list renders shuffled.
    #[test]
    fn album_tracks_are_numbered_by_position_and_inherit_the_cover() {
        let mut album = album_to_media(
            addon_uuid(),
            &ec::Album {
                id: "al1".into(),
                title: "Small Hours".into(),
                artist: Some("Some Artist".into()),
                artwork_url: Some("https://example.com/cover.jpg".into()),
                ..Default::default()
            },
        );
        album.grandparent_id = Some(Uuid::from_u128(9));

        let tracks: Vec<db::Media> = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, id)| {
                album_track_to_media(
                    addon_uuid(),
                    &track(id, "Track", "Some Artist"),
                    &album,
                    i,
                )
            })
            .collect();

        assert_eq!(
            tracks
                .iter()
                .map(|t| t.idx)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3)]
        );
        assert!(
            tracks
                .iter()
                .all(|t| t.parent_id == Some(album.id))
        );
        assert!(
            tracks
                .iter()
                .all(|t| t.grandparent_id == album.grandparent_id)
        );
        // A track with no artwork of its own shows the album cover.
        assert_eq!(
            tracks[0]
                .images
                .primary
                .first()
                .map(|i| i
                    .path
                    .as_str()),
            Some("https://example.com/cover.jpg")
        );
        assert_eq!(tracks[0].album_name(), Some("Small Hours"));
    }

    #[test]
    fn a_playlist_carries_its_tracks_as_ordered_members() {
        let media = playlist_to_media(
            addon_uuid(),
            &ec::Playlist {
                id: "p1".into(),
                title: "90s Road Trip".into(),
                tracks: vec![
                    track("t1", "Smells Like Teen Spirit", "Nirvana"),
                    track("t2", "Wonderwall", "Oasis"),
                ],
                ..Default::default()
            },
        );

        assert_eq!(media.kind, db::MediaKind::Playlist);
        let relations = media
            .relations
            .as_ref()
            .expect("playlist members");
        assert_eq!(relations.len(), 2);
        assert_eq!(
            relations
                .iter()
                .map(|(r, _)| r.weight)
                .collect::<Vec<_>>(),
            vec![Some(0), Some(1)]
        );
        assert!(
            relations
                .iter()
                .all(|(r, _)| r.role == Some(db::RelationRole::Playlist)
                    && r.left_media_id == media.id)
        );
    }

    #[test]
    fn an_album_year_becomes_a_release_date() {
        let media = album_to_media(
            addon_uuid(),
            &ec::Album {
                id: "al1".into(),
                title: "T".into(),
                year: Some(2024),
                ..Default::default()
            },
        );
        assert_eq!(
            media
                .released_at
                .map(|d| d
                    .date()
                    .to_string()),
            Some("2024-01-01".to_string())
        );
    }

    /// A catalog row's items open through the detail endpoints, so each has to
    /// map onto the kind its `type` names.
    #[test]
    fn catalog_items_map_to_their_declared_kind() {
        let item = |kind: &str| ec::CatalogItem {
            id: "x1".into(),
            kind: serde_json::from_value(serde_json::Value::String(kind.into()))
                .unwrap(),
            title: "X".into(),
            artist: Some("A".into()),
            album: None,
            isrc: None,
            artwork_url: None,
            duration_ms: Some(181_000),
            explicit: None,
            hi_res: None,
            year: None,
        };

        assert_eq!(
            catalog_item_to_media(addon_uuid(), &item("track")).kind,
            db::MediaKind::Track
        );
        assert_eq!(
            catalog_item_to_media(addon_uuid(), &item("album")).kind,
            db::MediaKind::Album
        );
        assert_eq!(
            catalog_item_to_media(addon_uuid(), &item("artist")).kind,
            db::MediaKind::Artist
        );
        assert_eq!(
            catalog_item_to_media(addon_uuid(), &item("playlist")).kind,
            db::MediaKind::Playlist
        );
        // durationMs is milliseconds, not seconds.
        assert_eq!(
            catalog_item_to_media(addon_uuid(), &item("track")).runtime,
            Some(181)
        );
    }

    // -- end to end --------------------------------------------------------

    fn mock_manifest(mock: &MockAddon, resources: serde_json::Value) {
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/manifest.json"));
                then.status(200)
                    .json_body(serde_json::json!({
                        "id": "com.example.a", "name": "A", "version": "1.0.0",
                        "resources": resources,
                        "types": ["track", "album", "artist"],
                    }));
            });
    }

    async fn test_ctx() -> (AppContext, crate::integration_test::TestGuard) {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = guard
            .0
            .clone();
        (ctx, guard)
    }

    /// The happy path: search for the track, then resolve its stream.
    #[tokio::test]
    async fn resolves_a_stream_by_searching_when_the_row_has_no_eclipse_id() {
        let mock = MockAddon::start();
        mock_manifest(&mock, serde_json::json!(["search", "stream"]));
        let search = mock
            .server
            .mock(|when, then| {
                when.path(mock.path("/search"))
                    .query_param("q", "Adele Hello");
                then.status(200)
                    .json_body(serde_json::json!({"tracks": [
                        {"id": "wrong", "title": "Hullo", "artist": "Someone"},
                        {"id": "right", "title": "Hello", "artist": "Adele", "duration": 240},
                    ]}));
            });
        let stream = mock
            .server
            .mock(|when, then| {
                when.path(mock.path("/stream/right"));
                then.status(200)
                    .json_body(serde_json::json!({
                        "url": "https://cdn.example.com/hello.flac",
                        "codec": "flac", "quality": "lossless",
                        "sampleRate": 44100, "bitDepth": 16,
                    }));
            });

        let (ctx, _guard) = test_ctx().await;
        let mut media = db_track("Hello");
        media.runtime = Some(240);
        media
            .external_ids
            .artist_name = Some("Adele".into());

        let streams = mock
            .addon()
            .get_streams(&media, &ctx, None)
            .await
            .unwrap();

        search.assert();
        stream.assert();
        assert_eq!(streams.len(), 1);
        assert!(matches!(
            &streams[0].descriptor,
            StreamDescriptor::Http { url, .. } if url == "https://cdn.example.com/hello.flac"
        ));
        // The label has to distinguish this source from another one.
        let name = streams[0]
            .name
            .as_deref()
            .unwrap();
        assert!(name.contains("lossless"), "{name}");
        assert!(name.contains("FLAC"), "{name}");
        assert!(name.contains("44kHz/16bit"), "{name}");
    }

    /// A row that already carries this addon's id must be streamed directly —
    /// searching again risks resolving to a different recording.
    #[tokio::test]
    async fn a_known_eclipse_id_streams_without_searching() {
        let mock = MockAddon::start();
        mock_manifest(&mock, serde_json::json!(["search", "stream"]));
        let search = mock
            .server
            .mock(|when, then| {
                when.path(mock.path("/search"));
                then.status(200)
                    .json_body(serde_json::json!({"tracks": []}));
            });
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/stream/track_123"));
                then.status(200)
                    .json_body(
                        serde_json::json!({"url": "https://cdn.example.com/a.mp3"}),
                    );
            });

        let (ctx, _guard) = test_ctx().await;
        let mut media = db_track("Anything");
        media
            .external_ids
            .eclipse_id = Some(scoped_id(addon_uuid(), "track_123"));

        let streams = mock
            .addon()
            .get_streams(&media, &ctx, None)
            .await
            .unwrap();

        assert_eq!(streams.len(), 1);
        search.assert_hits(0);
    }

    /// When the addon can look up an ISRC, that is used instead of a text
    /// search — it is the only way to be certain of the recording.
    #[tokio::test]
    async fn an_isrc_is_resolved_directly_when_the_addon_supports_it() {
        let mock = MockAddon::start();
        mock_manifest(&mock, serde_json::json!(["search", "stream", "isrc"]));
        let search = mock
            .server
            .mock(|when, then| {
                when.path(mock.path("/search"));
                then.status(200)
                    .json_body(serde_json::json!({"tracks": []}));
            });
        let resolve = mock
            .server
            .mock(|when, then| {
                when.path(mock.path("/resolve-isrc"))
                    .query_param("isrc", "USRC11903813");
                then.status(200)
                    .json_body(serde_json::json!({"trackId": "by_isrc"}));
            });
        let stream = mock
            .server
            .mock(|when, then| {
                when.path(mock.path("/stream/by_isrc"));
                then.status(200)
                    .json_body(
                        serde_json::json!({"url": "https://cdn.example.com/a.mp3"}),
                    );
            });

        let (ctx, _guard) = test_ctx().await;
        let mut media = db_track("Late Night");
        media
            .external_ids
            .isrc = Some("USRC11903813".into());

        let streams = mock
            .addon()
            .get_streams(&media, &ctx, None)
            .await
            .unwrap();

        resolve.assert();
        stream.assert();
        assert_eq!(streams.len(), 1);
        search.assert_hits(0);
    }

    /// No confident match means no stream. Playing the closest hit would play
    /// the wrong song.
    #[tokio::test]
    async fn no_confident_match_yields_no_stream() {
        let mock = MockAddon::start();
        mock_manifest(&mock, serde_json::json!(["search", "stream"]));
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/search"));
                then.status(200)
                    .json_body(serde_json::json!({"tracks": [
                        {"id": "other", "title": "Something Else", "artist": "Nobody"}
                    ]}));
            });
        let stream = mock
            .server
            .mock(|when, then| {
                when.path_matches(
                    regex::Regex::new(&format!("^{}/stream/", mock.scope)).unwrap(),
                );
                then.status(200)
                    .json_body(
                        serde_json::json!({"url": "https://cdn.example.com/wrong.mp3"}),
                    );
            });

        let (ctx, _guard) = test_ctx().await;
        let streams = mock
            .addon()
            .get_streams(&db_track("Hello"), &ctx, None)
            .await
            .unwrap();

        assert!(streams.is_empty());
        stream.assert_hits(0);
    }

    /// One `/search` response feeds every kind, so a kind the addon returned
    /// nothing for yields an empty list rather than another request.
    #[tokio::test]
    async fn search_splits_one_response_across_kinds() {
        let mock = MockAddon::start();
        mock_manifest(&mock, serde_json::json!(["search", "stream"]));
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/search"))
                    .query_param("q", "adele");
                then.status(200)
                    .json_body(serde_json::json!({
                        "tracks": [{"id": "t1", "title": "Hello", "artist": "Adele"}],
                        "albums": [{"id": "al1", "title": "25", "artist": "Adele"}],
                    }));
            });

        let (ctx, _guard) = test_ctx().await;
        let a = mock.addon();

        let tracks = a
            .search(&db::MediaKind::Track, "adele", 10, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].kind, db::MediaKind::Track);

        let albums = a
            .search(&db::MediaKind::Album, "adele", 10, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].kind, db::MediaKind::Album);

        // The addon returned no artists; that is an empty result, not an error.
        let artists = a
            .search(&db::MediaKind::Artist, "adele", 10, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(artists.is_empty());

        // Searching for a kind Eclipse has no concept of (e.g. Movie) is
        // declined outright (`None`), not answered with an empty result.
        assert!(
            a.search(&db::MediaKind::Movie, "adele", 10, &ctx)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn search_honours_the_limit() {
        let mock = MockAddon::start();
        mock_manifest(&mock, serde_json::json!(["search", "stream"]));
        let tracks: Vec<_> = (0..10)
            .map(|i| {
                serde_json::json!({
                    "id": format!("t{i}"), "title": format!("Song {i}"), "artist": "A"
                })
            })
            .collect();
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/search"));
                then.status(200)
                    .json_body(serde_json::json!({"tracks": tracks}));
            });

        let (ctx, _guard) = test_ctx().await;
        let found = mock
            .addon()
            .search(&db::MediaKind::Track, "a", 3, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.len(), 3);
    }

    /// Browsing an album fetches its tracks from the addon.
    #[tokio::test]
    async fn browsing_an_album_returns_its_tracks() {
        let mock = MockAddon::start();
        mock_manifest(&mock, serde_json::json!(["search", "stream", "catalog"]));
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/album/al1"));
                then.status(200)
                    .json_body(serde_json::json!({
                        "id": "al1", "title": "Small Hours", "artist": "Some Artist",
                        "tracks": [
                            {"id": "t1", "title": "One", "artist": "Some Artist"},
                            {"id": "t2", "title": "Two", "artist": "Some Artist"},
                        ],
                    }));
            });

        let (ctx, _guard) = test_ctx().await;
        let album = db::Media {
            kind: db::MediaKind::Album,
            title: "Small Hours".into(),
            external_ids: db::ExternalIds {
                eclipse_id: Some(scoped_id(addon_uuid(), "al1")),
                ..Default::default()
            },
            ..Default::default()
        };

        let children = mock
            .addon()
            .get_children(&album, &ctx)
            .await
            .unwrap()
            .expect("album tracks");

        assert_eq!(children.len(), 2);
        assert_eq!(
            children
                .iter()
                .map(|c| c.idx)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2)]
        );
    }

    /// A container with no Eclipse id belongs to some other addon; this one must
    /// not claim it, or it shadows the addon that can actually expand it.
    #[tokio::test]
    async fn does_not_claim_containers_from_other_addons() {
        let mock = MockAddon::start();
        let deezer_album = db::Media {
            kind: db::MediaKind::Album,
            external_ids: db::ExternalIds {
                deezer_album: Some(42),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!TreeAddon::supports(&mock.addon(), &deezer_album));
    }

    /// Catalog rows come from the manifest, and an unknown row id is declined so
    /// another addon's catalog is not served from here.
    #[tokio::test]
    async fn catalogs_come_from_the_manifest() {
        let mock = MockAddon::start();
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/manifest.json"));
                then.status(200)
                    .json_body(serde_json::json!({
                        "id": "a", "name": "A", "version": "1.0.0",
                        "resources": ["search", "stream", "catalog"],
                        "catalogs": [
                            {"id": "top", "type": "track", "name": "Top Songs"},
                            {"id": "new", "type": "album", "name": "New Releases"},
                        ],
                    }));
            });
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/catalog/top"));
                then.status(200)
                    .json_body(serde_json::json!({"items": [
                        {"id": "t1", "type": "track", "title": "Spend Dat", "artist": "Yung Miami"}
                    ]}));
            });

        let (ctx, _guard) = test_ctx().await;
        let a = mock.addon();

        let catalogs = a
            .catalog_list(&ctx)
            .await
            .unwrap();
        assert_eq!(
            catalogs
                .iter()
                .map(|c| (
                    c.provider_catalog_id
                        .as_str(),
                    c.name
                        .as_str(),
                    c.media_kind
                        .clone()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("top", "Top Songs", Some(db::MediaKind::Track)),
                ("new", "New Releases", Some(db::MediaKind::Album)),
            ]
        );

        let items: Vec<db::Media> = a
            .catalog_stream(&ctx, "top")
            .await
            .unwrap()
            .expect("catalog stream")
            .collect()
            .await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Spend Dat");

        assert!(
            a.catalog_stream(&ctx, "not-mine")
                .await
                .unwrap()
                .is_none()
        );
    }

    // -- installed end to end ----------------------------------------------

    /// A complete Eclipse addon: manifest, search, stream, and album detail.
    fn smoke_addon() -> MockAddon {
        let mock = MockAddon::start();
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/manifest.json"));
                then.status(200)
                    .json_body(serde_json::json!({
                        "id": "com.example.smoke", "name": "Smoke Addon",
                        "version": "1.0.0",
                        "resources": ["search", "stream", "catalog", "isrc"],
                        "types": ["track", "album", "artist"],
                        "catalogs": [{"id": "top", "type": "track", "name": "Top Songs"}],
                    }));
            });
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/search"));
                then.status(200)
                    .json_body(serde_json::json!({
                        "tracks": [{
                            "id": "track_123", "title": "Late Night",
                            "artist": "Some Artist", "album": "Small Hours",
                            "duration": 240, "isrc": "USRC11903813",
                            "artworkURL": "https://example.com/cover.jpg",
                        }],
                        "albums": [{
                            "id": "album_456", "title": "Small Hours",
                            "artist": "Some Artist", "trackCount": 2, "year": "2024",
                        }],
                    }));
            });
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/stream/track_123"));
                then.status(200)
                    .json_body(serde_json::json!({
                        "url": "https://cdn.example.com/audio/track_123.flac",
                        "codec": "flac", "quality": "lossless",
                    }));
            });
        mock.server
            .mock(|when, then| {
                when.path(mock.path("/album/album_456"));
                then.status(200)
                    .json_body(serde_json::json!({
                        "id": "album_456", "title": "Small Hours",
                        "artist": "Some Artist",
                        "tracks": [
                            {"id": "track_123", "title": "Late Night",
                             "artist": "Some Artist", "duration": 240,
                             "isrc": "USRC11903813"},
                            {"id": "track_124", "title": "Small Hours",
                             "artist": "Some Artist", "duration": 200},
                        ],
                    }));
            });
        mock
    }

    /// Installs the addon the way `POST /addons` does: derive the enabled
    /// resources and types from the live manifest, then persist the row.
    ///
    /// The seeded default addons (TMDB, Deezer, Monochrome) are disabled first:
    /// the search fan-out returns the first addon that answers, so leaving them
    /// enabled would both hide this addon's results and hit the live network.
    async fn install(ctx: &AppContext, manifest_url: String) -> Uuid {
        sqlx::query("UPDATE addons SET enabled = 0")
            .execute(&ctx.db)
            .await
            .expect("disable seeded addons");

        let id = Uuid::new_v4();
        let cfg = serde_json::json!({ "manifest_url": manifest_url });
        let caps = EclipsePreset
            .from_cfg(id, &cfg, &ctx.config)
            .expect("build capabilities");
        let (resource_refs, raw_types) = caps
            .kind
            .as_deref()
            .expect("kind capability")
            .available_info()
            .await
            .expect("fetch manifest")
            .expect("manifest info");
        let resources: Vec<ResourceType> = resource_refs
            .into_iter()
            .map(|r| r.name)
            .collect();
        let types: Vec<db::MediaKind> = raw_types
            .into_iter()
            .filter_map(crate::addons::recognized_manifest_media_kind)
            .map(db::MediaKind::from)
            .collect();

        let now = chrono::Utc::now().naive_utc();
        super::super::addon::Addon {
            id,
            name: "Smoke Addon".into(),
            preset: super::super::AddonPresetRef {
                kind: "eclipse".into(),
                config: cfg.into(),
            },
            resources,
            types,
            enabled: true,
            priority: 0,
            created_at: now,
            updated_at: now,
            system: false,
            is_default: true,
            http_redirect_stream: false,
            service_filter: vec![],
        }
        .insert(&ctx.db)
        .await
        .expect("insert addon");
        id
    }

    /// The feature end to end: an operator pastes an Eclipse addon URL, a user
    /// searches, clicks a result, and gets a playable stream.
    ///
    /// Covers the wiring the tests above cannot: preset lookup by kind,
    /// manifest-driven capability detection in `AddonService::from_db`, resource
    /// and type gating in the fan-out, search-result persistence, and stream
    /// resolution for the persisted row.
    #[tokio::test]
    async fn an_installed_addon_serves_search_click_and_play() {
        let addon_server = smoke_addon();
        let (ctx, _guard) = test_ctx().await;
        let addon_id = install(
            &ctx,
            addon_server
                .base
                .clone(),
        )
        .await;

        let addons = super::super::AddonService::from_db(&ctx.db, &ctx.config)
            .await
            .expect("load addons");

        // Capability detection: the row declared nothing.
        let runtime = addons
            .get(addon_id)
            .expect("addon runtime");
        let resources: Vec<String> = runtime
            .caps
            .metadata
            .supported_resources
            .iter()
            .map(|r| {
                r.name
                    .to_string()
            })
            .collect();
        assert!(
            resources.contains(&"search".to_string()),
            "resources: {resources:?}"
        );
        assert!(
            resources.contains(&"stream".to_string()),
            "resources: {resources:?}"
        );

        // Search — this is what the /music/search handler calls.
        let results = addons
            .search(&db::MediaKind::Track, "Late Night", 10, &ctx, None)
            .await
            .expect("search");
        assert_eq!(results.len(), 1);
        let hit = &results[0];
        assert_eq!(hit.title, "Late Night");
        assert_eq!(hit.runtime, Some(240));
        assert_eq!(hit.artist_name(), Some("Some Artist"));

        // Click: the transient search hit becomes a persisted, playable row.
        let resolved = crate::services::MediaResolveService::resolve_item(hit.id, &ctx)
            .await
            .expect("resolve clicked item")
            .expect("clicked item resolves to a row");
        assert_eq!(resolved.title, "Late Night");
        assert!(
            db::Media::get_by_id(&ctx.db, &resolved.id)
                .await
                .unwrap()
                .is_some(),
            "the clicked track must be persisted"
        );

        // Play.
        let streams = addons
            .get_streams(&resolved, &ctx, None)
            .await
            .expect("get streams");
        assert_eq!(streams.len(), 1);
        let info = streams[0]
            .stream_info
            .as_ref()
            .expect("stream info");
        assert!(
            matches!(
                &info.descriptor,
                StreamDescriptor::Http { url, .. }
                    if url == "https://cdn.example.com/audio/track_123.flac"
            ),
            "unexpected descriptor: {:?}",
            info.descriptor
        );
        assert_eq!(
            info.source
                .as_deref(),
            Some("Smoke Addon"),
            "the stream must be attributed to the addon"
        );
    }
}
