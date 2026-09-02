//! Unified addon abstraction. Each addon kind declares which resources ×
//! media types it serves; user-added instances are rows in the `addons` table.

pub mod addon;
pub mod betterposters;
pub mod deezer;
pub mod eclipse;
pub mod introdb;
pub mod iptv;
pub mod lrclib;
pub mod media_tracker;
pub mod opendal;
pub mod probe;
pub mod squid;
pub mod stremio;
pub mod tmdb;
pub mod torznab;
pub mod trakt;
pub mod ytdlp;

use anyhow::{Result, anyhow};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use futures::Stream;
use sqlx::SqlitePool;
use std::{
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::keyed_lock::KeyedLock;
use libc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    AppContext, api, common::ProgressReporter, db, sdks, stream::StreamDescriptor,
};
pub use addon::{Addon, CatalogState, set_user_addon_override, user_addon_override};
use remux_sdks::remuxdb;

pub use remux_sdks::remux::AddonPresetRef;
use remux_sdks::remux::{LyricDto, MediaSegments, RemoteLyricInfoDto};

pub use remux_sdks::{
    remux::{
        AddonCatalogDto, AddonDto, AddonMetadata, AddonOption, AddonOptionType,
        AddonSelectOption, CreateAddonRequest, MediaKind, UpdateAddonCatalogRequest,
        UpdateAddonRequest,
    },
    stremio::ResourceType,
};

#[derive(Debug, Clone)]
pub struct CatalogInfo {
    pub provider_catalog_id: String,
    pub name: String,
    /// Whether this catalog should be enabled by default (before the user changes it).
    pub default_enabled: bool,
    /// Default per-catalog item limit (before the user changes it).
    pub default_max_items: Option<i64>,
    /// Media kind for auto-created collections backed by this catalog.
    pub collection_media_kind: Option<db::CollectionMediaKind>,
    /// The MediaKind of items this specific catalog yields.
    pub media_kind: Option<db::MediaKind>,
}

impl CatalogInfo {
    pub fn new(
        provider_catalog_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            provider_catalog_id: provider_catalog_id.into(),
            name: name.into(),
            default_enabled: false,
            default_max_items: None,
            collection_media_kind: None,
            media_kind: None,
        }
    }
}

/// A `CatalogInfo` merged with its persisted `CatalogState` override (if any) —
/// the single, fully-resolved view of a catalog that callers should use. Avoids
/// every caller re-implementing "use the stored override, else fall back to the
/// provider's declared default" itself.
#[derive(Debug, Clone)]
pub struct ResolvedCatalog {
    pub provider_catalog_id: String,
    /// Full "addon:{addon_id}:{provider_catalog_id}" id, usable with `make_catalog_stream()`.
    pub catalog_id: String,
    /// Deterministic collection id for this catalog's `media_relations` membership.
    pub collection_id: Uuid,
    pub name: String,
    pub media_kind: Option<db::MediaKind>,
    pub collection_media_kind: Option<db::CollectionMediaKind>,
    pub enabled: bool,
    pub max_items: Option<i64>,
    pub tags: Vec<String>,
}

#[async_trait]
pub trait RemoteMediaStream: Send + Sync {
    async fn stream(
        &self,
        ctx: &AppContext,
    ) -> Result<Pin<Box<dyn Stream<Item = db::Media> + Send>>>;
}

#[derive(Debug)]
pub struct LyricSearchRequest {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
}

/// Save relation links that were deferred onto `media.relations` by `apply_meta`.
/// Must be called after `db::Media::upsert` so `left_media_id` FK constraints are satisfied.
pub(crate) async fn save_pending_relations(ctx: &AppContext, items: &[db::Media]) {
    // TMDB ID is the canonical key for person rows.  Name-keyed person stubs
    // (produced by Stremio/Jellyfin addons when no TMDB ID is available) must NOT
    // be persisted — the TMDB addon will insert them with the correct TMDB-keyed UUID
    // when it enriches the parent movie/series.  Storing them now would create
    // duplicate rows alongside any existing TMDB-keyed row for the same person.
    let name_keyed_person_ids: std::collections::HashSet<Uuid> = items
        .iter()
        .filter_map(|m| {
            m.relations
                .as_ref()
        })
        .flatten()
        .filter(|(_, m)| {
            m.kind == db::MediaKind::Person
                && m.external_ids
                    .tmdb
                    .is_none()
        })
        .map(|(_, m)| m.id)
        .collect();

    // One batched upsert for all relation media (persons/genres) across the whole slice —
    // avoids opening a separate transaction per item (N items → N transactions otherwise).
    let all_rel_media: Vec<db::Media> = items
        .iter()
        .filter_map(|m| {
            m.relations
                .as_ref()
        })
        .flatten()
        .map(|(_, m)| m.clone())
        .filter(|m| !name_keyed_person_ids.contains(&m.id))
        .collect();
    if !all_rel_media.is_empty() {
        if let Err(e) = db::Media::upsert(&ctx.db, &all_rel_media).await {
            warn!(error = %e, "failed to upsert relation media batch");
        }
    }

    // Collect items that have relations, then batch delete + batch upsert
    // (replaces N×delete + N×upsert with 1 delete + 1 upsert).
    let items_with_rels: Vec<&db::Media> = items
        .iter()
        .filter(|m| {
            m.relations
                .as_ref()
                .map_or(false, |r| !r.is_empty())
        })
        .collect();
    if items_with_rels.is_empty() {
        return;
    }

    let all_ids: Vec<uuid::Uuid> = items_with_rels
        .iter()
        .map(|m| m.id)
        .collect();
    // Always use m.id as left_media_id — relations may have been built against a
    // temporary UUID (e.g. before IMDB resolution in stremio_search) that was later
    // recomputed to the stable UUID. m.id is the authoritative current identity.
    let all_rels: Vec<db::MediaRelation> = items_with_rels
        .iter()
        .flat_map(|m| {
            let current_id = m.id;
            m.relations
                .as_ref()
                .unwrap()
                .iter()
                .map(move |(r, _)| db::MediaRelation {
                    left_media_id: current_id,
                    ..r.clone()
                })
        })
        // Don't link relations that point to name-keyed person stubs.
        .filter(|r| !name_keyed_person_ids.contains(&r.right_media_id))
        .collect();

    // Fetch existing relations and only write the delta — avoids the WAL pressure
    // of a full delete+reinsert on steady-state runs where nothing changed.
    let existing = db::MediaRelation::get_by_left_ids(&ctx.db, &all_ids)
        .await
        .unwrap_or_default();

    // `apply_meta` deliberately omits provider genre relations when Genres is
    // locked. If another relation type (for example Cast) is still pending,
    // the replacement logic below must not interpret those omitted genres as
    // deletions. Resolve the existing right-hand media kinds once and retain
    // genre links for every item whose Genres field is locked.
    let genre_locked_ids: std::collections::HashSet<Uuid> = items_with_rels
        .iter()
        .filter(|item| item.is_field_locked(&db::MetadataField::Genres))
        .map(|item| item.id)
        .collect();
    let locked_relation_right_ids: Vec<Uuid> = existing
        .iter()
        .filter(|relation| genre_locked_ids.contains(&relation.left_media_id))
        .map(|relation| relation.right_media_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let locked_genre_right_ids: Option<std::collections::HashSet<Uuid>> =
        if locked_relation_right_ids.is_empty() {
            Some(std::collections::HashSet::new())
        } else {
            match db::Media::get_by_ids(&ctx.db, &locked_relation_right_ids).await {
                Ok(media) => Some(
                    media
                        .into_iter()
                        .filter(|right| {
                            matches!(
                                right.kind,
                                db::MediaKind::Genre | db::MediaKind::MusicGenre
                            )
                        })
                        .map(|right| right.id)
                        .collect(),
                ),
                Err(e) => {
                    warn!(error = %e, "failed to resolve locked genre relations; preserving all locked-item relations");
                    None
                }
            }
        };

    type RelKey = (Uuid, Uuid, Option<db::RelationRole>);

    let existing_map: std::collections::HashMap<RelKey, &db::MediaRelation> = existing
        .iter()
        .map(|r| ((r.left_media_id, r.right_media_id, r.role), r))
        .collect();

    let desired_keys: std::collections::HashSet<RelKey> = all_rels
        .iter()
        .map(|r| (r.left_media_id, r.right_media_id, r.role))
        .collect();

    let to_delete: Vec<Uuid> = existing
        .iter()
        .filter(|r| {
            if desired_keys.contains(&(r.left_media_id, r.right_media_id, r.role)) {
                return false;
            }
            if !genre_locked_ids.contains(&r.left_media_id) {
                return true;
            }
            match &locked_genre_right_ids {
                Some(genre_ids) => !genre_ids.contains(&r.right_media_id),
                None => false,
            }
        })
        .map(|r| r.relation_id)
        .collect();

    let to_upsert: Vec<db::MediaRelation> = all_rels
        .into_iter()
        .filter(|r| {
            let key = (r.left_media_id, r.right_media_id, r.role);
            match existing_map.get(&key) {
                None => true,
                Some(ex) => ex.weight != r.weight || ex.character != r.character,
            }
        })
        .collect();

    if !to_delete.is_empty() {
        db::MediaRelation::delete_by_ids(&ctx.db, &to_delete)
            .await
            .ok();
    }
    if !to_upsert.is_empty() {
        if let Err(e) = db::MediaRelation::upsert(&ctx.db, &to_upsert).await {
            warn!(error = %e, "failed to upsert relations batch");
        }
    }
}

/// Persist `provider:` tags collected from meta addons. Only `provider:`-prefixed
/// tags are touched — user-set tags with other prefixes are left intact.
pub(crate) async fn save_pending_tags(ctx: &AppContext, items: &[db::Media]) {
    // Collect (media_id, tag) pairs for all items with provider tags.
    let mut rows: Vec<(uuid::Uuid, &str)> = Vec::new();
    let mut ids_with_tags: Vec<uuid::Uuid> = Vec::new();
    for item in items {
        let provider_tags: Vec<&str> = item
            .tags
            .iter()
            .filter(|t| t.starts_with("provider:"))
            .map(String::as_str)
            .collect();
        if provider_tags.is_empty() {
            continue;
        }
        ids_with_tags.push(item.id);
        for tag in provider_tags {
            rows.push((item.id, tag));
        }
    }

    if rows.is_empty() {
        return;
    }

    // One DELETE for all affected media IDs, one batch INSERT for all tags.
    let mut delete_qb = sqlx::QueryBuilder::new(
        "DELETE FROM media_tags WHERE tag LIKE 'provider:%' AND media_id IN (",
    );
    let mut sep = delete_qb.separated(", ");
    for id in &ids_with_tags {
        sep.push_bind(id);
    }
    delete_qb.push(")");

    let mut insert_qb =
        sqlx::QueryBuilder::new("INSERT OR IGNORE INTO media_tags (media_id, tag) ");
    insert_qb.push_values(&rows, |mut b, (id, tag)| {
        b.push_bind(id)
            .push_bind(tag);
    });

    if let Err(e) = delete_qb
        .build()
        .execute(&ctx.db)
        .await
    {
        warn!(error = %e, "failed to clear provider tags");
        return;
    }
    if let Err(e) = insert_qb
        .build()
        .execute(&ctx.db)
        .await
    {
        warn!(error = %e, "failed to insert provider tags");
    }
}

pub(crate) async fn save_pending_popularity(ctx: &AppContext, items: &[db::Media]) {
    let today = chrono::Utc::now().date_naive();
    let rows: Vec<_> = items
        .iter()
        .filter_map(|item| {
            item.pending_popularity
                .as_ref()
                .map(|(ext_id, value)| (item.id, ext_id.clone(), value.get()))
        })
        .collect();

    if rows.is_empty() {
        return;
    }

    let mut qb = sqlx::QueryBuilder::new(
        "INSERT INTO popularity_raw (source, external_id, media_id, media_raw, value, date) ",
    );
    qb.push_values(&rows, |mut b, (media_id, ext_id, value)| {
        b.push_bind("tmdb")
            .push_bind(ext_id)
            .push_bind(media_id)
            .push_bind(ext_id)
            .push_bind(value)
            .push_bind(&today);
    });
    qb.push(
        " ON CONFLICT DO UPDATE SET value = excluded.value, \
         media_id = COALESCE(excluded.media_id, popularity_raw.media_id), \
         media_raw = COALESCE(excluded.media_raw, popularity_raw.media_raw)",
    );
    if let Err(e) = qb
        .build()
        .execute(&ctx.db)
        .await
    {
        warn!(error = %e, "failed to write popularity_raw batch");
    }
}

pub(crate) async fn bulk_insert_snapshots(
    ctx: &AppContext,
    snapshots: &[MetricSnapshot],
) -> Result<()> {
    if snapshots.is_empty() {
        return Ok(());
    }
    for chunk in snapshots.chunks(400) {
        let mut qb = sqlx::QueryBuilder::new(
            "INSERT INTO popularity_raw (source, external_id, media_id, media_raw, value, date) ",
        );
        qb.push_values(chunk, |mut b, s| {
            b.push_bind(&s.source)
                .push_bind(&s.external_id)
                .push_bind(s.media_id)
                .push_bind(&s.media_raw)
                .push_bind(
                    s.value
                        .get(),
                )
                .push_bind(&s.date);
        });
        qb.push(
            " ON CONFLICT DO UPDATE SET value = excluded.value, \
             media_id = COALESCE(excluded.media_id, popularity_raw.media_id), \
             media_raw = COALESCE(excluded.media_raw, popularity_raw.media_raw)",
        );
        qb.build()
            .execute(&ctx.db)
            .await?;
    }
    Ok(())
}

pub(crate) fn merge_media(target: &mut db::Media, source: &db::Media, replace: bool) {
    use remux_utils::merge_option;

    if !target.is_field_locked(&db::MetadataField::Name)
        && (replace
            || target
                .title
                .is_empty())
        && !source
            .title
            .is_empty()
    {
        target.title = source
            .title
            .clone();
    }

    if !target.is_field_locked(&db::MetadataField::Overview) {
        merge_option(&mut target.description, &source.description, replace);
    }
    merge_option(&mut target.released_at, &source.released_at, replace);
    if !target.is_field_locked(&db::MetadataField::Runtime) {
        merge_option(&mut target.runtime, &source.runtime, replace);
    }
    merge_option(
        &mut target.rating_audience,
        &source.rating_audience,
        replace,
    );
    if !target.is_field_locked(&db::MetadataField::OfficialRating) {
        merge_option(&mut target.certification, &source.certification, replace);
        merge_option(
            &mut target.certification_age,
            &source.certification_age,
            replace,
        );
    }
    if !target.is_field_locked(&db::MetadataField::ProductionLocations) {
        merge_option(&mut target.country, &source.country, replace);
    }
    merge_option(
        &mut target.original_language,
        &source.original_language,
        replace,
    );
    merge_option(&mut target.trailers, &source.trailers, replace);
    merge_option(
        &mut target.digital_released_at,
        &source.digital_released_at,
        replace,
    );
    merge_option(&mut target.status, &source.status, replace);
    merge_option(&mut target.end_date, &source.end_date, replace);
    merge_option(&mut target.idx, &source.idx, replace);
    merge_option(&mut target.parent_idx, &source.parent_idx, replace);

    target
        .external_ids
        .merge(&source.external_ids, replace);
    merge_option(
        &mut target.external_ratings,
        &source.external_ratings,
        replace,
    );
    if source
        .external_ratings
        .is_some()
    {
        merge_option(&mut target.rating_audience, &source.rating_audience, true);
    }
}

pub(crate) fn apply_title_format(media: &mut db::Media) {
    if media.kind == db::MediaKind::Season {
        media.title = format!(
            "Season {}",
            media
                .idx
                .unwrap_or(1)
        );
    }
    if media.kind == db::MediaKind::Episode {
        // Stored episode titles are kept clean: Jellyfin clients render the
        // season/episode from IndexNumber/ParentIndexNumber, so embedding a
        // "SxxExx - " prefix here double-prints it. Any prefix that slipped in
        // from a raw source is stripped so this stays the single invariant for
        // episode titles.
        if let Some(stripped) = strip_episode_title_prefix(&media.title) {
            media.title = stripped;
        }
    }
}

/// Remove a leading `S<season>E<episode> - ` / `E<episode> - ` prefix (optionally
/// space-separated, e.g. `S01 E07`) from an episode title. Returns `None` when
/// there's nothing to strip.
fn strip_episode_title_prefix(title: &str) -> Option<String> {
    let t = title.trim_start();
    let mut after = t;

    if after.starts_with('S') {
        after = take_digits(&after[1..]);
        after = after.trim_start();
        if !after.starts_with('E') {
            return None;
        }
    } else if !after.starts_with('E') {
        return None;
    }
    after = take_digits(&after[1..]);
    after = after.trim_start();

    match after.strip_prefix('-') {
        Some(rest) => {
            let rest = rest.trim_start();
            (!rest.is_empty()).then(|| rest.to_string())
        }
        None => None,
    }
}

/// The leading run of ASCII digits.
fn take_digits(s: &str) -> &str {
    let n = s
        .as_bytes()
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    &s[n..]
}

fn series_is_active(status: &Option<db::MediaStatus>) -> bool {
    !matches!(
        status,
        Some(db::MediaStatus::Ended) | Some(db::MediaStatus::Unreleased)
    )
}

fn episode_in_active_window(child: &db::Media) -> bool {
    match child.digital_released_at {
        None => true,
        Some(dt) => {
            let cutoff = chrono::Utc::now().naive_utc() - chrono::Duration::days(180);
            dt > cutoff
        }
    }
}

fn child_refresh_force(
    force_refresh: bool,
    in_active_window: bool,
    child: &db::Media,
) -> Option<bool> {
    if force_refresh || in_active_window {
        Some(true)
    } else if child
        .refreshed_at
        .is_none()
    {
        Some(false)
    } else {
        None
    }
}

fn apply_meta(media: &mut db::Media, mut patch: db::Media, replace: bool) {
    // Merge images onto the in-memory struct; db::Media::upsert persists them via
    // sync_from_media after the media row is committed, avoiding FK violations.
    if !patch
        .images
        .is_empty()
    {
        use remux_utils::merge_vec;
        let patch_images = std::mem::take(&mut patch.images);
        let imgs = &mut media.images;
        merge_vec(&mut imgs.primary, patch_images.primary, replace);
        merge_vec(&mut imgs.backdrop, patch_images.backdrop, replace);
        merge_vec(&mut imgs.logo, patch_images.logo, replace);
        merge_vec(&mut imgs.thumb, patch_images.thumb, replace);
    }

    if !patch
        .tags
        .is_empty()
        && !media.is_field_locked(&db::MetadataField::Tags)
    {
        media
            .tags
            .extend(std::mem::take(&mut patch.tags));
        media
            .tags
            .sort_unstable();
        media
            .tags
            .dedup();
    }

    merge_media(media, &patch, replace);

    if let Some(relations) = patch.relations {
        if !relations.is_empty()
            && matches!(
                media.kind,
                db::MediaKind::Movie
                    | db::MediaKind::Series
                    | db::MediaKind::Episode
                    | db::MediaKind::Album
            )
        {
            let pending: Vec<(db::MediaRelation, db::Media)> = relations
                .into_iter()
                .filter(|(_, right_media)| match right_media.kind {
                    db::MediaKind::Person => {
                        !media.is_field_locked(&db::MetadataField::Cast)
                    }
                    db::MediaKind::Genre | db::MediaKind::MusicGenre => {
                        !media.is_field_locked(&db::MetadataField::Genres)
                    }
                    _ => true,
                })
                .collect();
            if !pending.is_empty() {
                match &mut media.relations {
                    Some(existing) => existing.extend(pending),
                    None => media.relations = Some(pending),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
impl From<crate::stream::StreamInfo> for db::Media {
    fn from(si: crate::stream::StreamInfo) -> Self {
        let title = si
            .name
            .clone()
            .or_else(|| {
                si.description
                    .clone()
            })
            .unwrap_or_default();
        let probe_data = si
            .probe_data
            .clone();
        db::Media {
            kind: db::MediaKind::Stream,
            title,
            stream_info: Some(si),
            probe_data,
            ..Default::default()
        }
    }
}

// Preset registry
// ---------------------------------------------------------------------------

pub struct AddonPresetRegistration(pub fn() -> Box<dyn AddonPreset>);
inventory::collect!(AddonPresetRegistration);

pub(super) fn make_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .build()
        .expect("failed to build HTTP client")
}

pub fn registered_presets() -> Vec<Box<dyn AddonPreset>> {
    inventory::iter::<AddonPresetRegistration>
        .into_iter()
        .map(|r| (r.0)())
        .collect()
}

// ---------------------------------------------------------------------------
// AddonPreset trait — kind descriptor + factory
// ---------------------------------------------------------------------------

pub trait AddonPreset: Send + Sync {
    fn id(&self) -> &'static str;
    fn metadata(&self) -> AddonMetadata;
    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
        config: &crate::Config,
    ) -> Result<AddonCapabilities>;

    /// Transform the config before it is persisted to the DB.
    /// Use this to convert inline secrets into file references, strip write-only fields, etc.
    /// The default is a no-op.
    fn normalize_cfg(
        &self,
        cfg: serde_json::Value,
        _config: &crate::Config,
    ) -> Result<serde_json::Value> {
        Ok(cfg)
    }
}

// ---------------------------------------------------------------------------
// AddonKind — lean identity + manifest trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait AddonKind: Send + Sync {
    fn id(&self) -> &'static str;

    /// Returns `Ok(Some((resources, types)))` when the addon can determine its
    /// own capabilities (e.g. by fetching a remote manifest).
    /// Returns `Ok(None)` to signal "no override — caller should fall back to
    /// the preset's `metadata().supported_*`".
    /// Returns `Err` when a required remote fetch fails and the addon cannot
    /// be used (the error propagates to the API caller).
    async fn available_info(
        &self,
    ) -> Result<
        Option<(
            Vec<remux_sdks::stremio::ResourceRef>,
            Vec<remux_sdks::stremio::MediaType>,
        )>,
    > {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Capability traits
// ---------------------------------------------------------------------------

#[async_trait]
pub trait IndexAddon: Send + Sync {
    async fn refresh_index(
        &self,
        ctx: &AppContext,
        addon: &Addon,
        progress: ProgressReporter,
    ) -> Result<()>;
    async fn purge_index(&self, ctx: &AppContext, addon: &Addon) -> Result<()>;
}

#[async_trait]
pub trait CatalogAddon: Send + Sync {
    async fn catalog_list(&self, ctx: &AppContext) -> Result<Vec<CatalogInfo>>;
    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>>;
}

#[async_trait]
pub trait MetaAddon: Send + Sync {
    async fn supports(&self, media: &db::Media) -> bool;
    /// Fetch metadata for `media` and return a partial `db::Media` patch.
    /// Only the fields the addon knows about need to be populated; the caller
    /// merges the patch into the existing record via `merge_media`.
    /// Populate `patch.images` for images, `patch.relations` for people/genres.
    async fn meta_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        config: &api::ServerConfiguration,
    ) -> Result<Option<db::Media>>;
    /// Called after all items for a given meta_id have been processed.
    /// Addons can use this to evict per-series caches they built during the run.
    fn on_series_done(&self, _meta_id: &str) {}
    /// Fetch remote image candidates for manual image selection in the UI.
    async fn images_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        Ok(vec![])
    }
}

#[async_trait]
pub trait TreeAddon: Send + Sync {
    fn supports(&self, root: &db::Media) -> bool;
    async fn get_children(
        &self,
        root: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>>;
}

#[async_trait]
pub trait SearchAddon: Send + Sync {
    async fn search_supports(&self, kind: &db::MediaKind) -> bool;
    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>>;
}

#[derive(Clone)]
pub struct SubtitleInfo {
    pub id: String,
    pub url: Option<crate::stream::StreamDescriptor>,
    pub lang: Option<String>,
    pub is_forced: bool,
    pub is_hi: bool,
}

#[async_trait]
pub trait SubtitleAddon: Send + Sync {
    fn supports(&self, media: &db::Media) -> bool;
    async fn subtitle_fetch(
        &self,
        media: &db::Media,
        db: &SqlitePool,
    ) -> Result<Vec<SubtitleInfo>>;
}

#[async_trait]
pub trait StreamAddon: Send + Sync {
    fn supports(&self, media: &db::Media) -> bool;
    async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        id_prefixes: Option<&[String]>,
    ) -> Result<Vec<crate::stream::StreamInfo>>;
    /// Serve bytes for a stream that requires this addon's config (e.g. credentials).
    /// Only called when `StreamDescriptor::addon_id()` points to this addon.
    async fn serve_stream(
        &self,
        descriptor: &crate::stream::StreamDescriptor,
        headers: &axum::http::HeaderMap,
    ) -> axum_anyhow::ApiResult<axum::response::Response> {
        Err(axum_anyhow::ApiError::builder()
            .status(axum::http::StatusCode::BAD_REQUEST)
            .title("stream")
            .detail("serve_stream not implemented for this addon")
            .build())
    }
}

#[async_trait]
pub trait SegmentAddon: Send + Sync {
    fn supports(&self, media: &db::Media) -> bool;
    async fn segment_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<MediaSegments>;
}

#[async_trait]
pub trait LyricAddon: Send + Sync {
    fn provider_id(&self) -> String;
    async fn lyric_fetch(&self, req: &LyricSearchRequest) -> Result<Option<LyricDto>>;
    async fn lyric_search(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>>;
    async fn lyric_get_by_id(&self, id: &str) -> Result<Option<LyricDto>>;
}

/// A single popularity snapshot emitted by a `MetricsAddon`.
/// Popularity score normalized to \[0.0, 100.0\].
///
/// All `MetricsAddon` implementations must emit values in this range.
/// Use `MetricValue::from_raw(raw, source_max)` to normalize a raw source value.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct MetricValue(f64);

impl MetricValue {
    /// Normalize a raw source value: `(raw / source_max) * 100`, clamped to \[0, 100\].
    pub fn from_raw(raw: f64, source_max: f64) -> Self {
        Self(((raw / source_max) * 100.0).clamp(0.0, 100.0))
    }

    /// Construct from an already-normalized value, clamping to \[0, 100\].
    pub fn from_normalized(v: f64) -> Self {
        Self(v.clamp(0.0, 100.0))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

/// Each addon computes the `value` internally from its own source data.
/// Values must be in \[0.0, 100.0\]; use `MetricValue::from_raw` to normalize.
#[derive(Debug, Clone)]
pub struct MetricSnapshot {
    pub source: String,
    pub external_id: String,
    pub value: MetricValue,
    pub date: chrono::NaiveDate,
    pub media_id: Option<uuid::Uuid>,
    pub media_raw: Option<String>,
}

/// Per-run context passed to `MetricsAddon::metric`. Carries only what addons
/// need (static config + pre-fetched settings) — addons must not touch the DB.
#[derive(Clone)]
pub struct MetricsCtx {
    pub config: Arc<crate::Config>,
    pub settings: api::ServerConfiguration,
}

#[async_trait]
pub trait MetricsAddon: AddonKind + Send + Sync {
    /// Fetch a popularity metric for a single media item.
    /// Returns `None` if this addon has no data for the item.
    /// Values in `MetricSnapshot.value` must be in \[0.0, 100.0\].
    async fn metric(
        &self,
        media: &db::Media,
        ctx: &MetricsCtx,
    ) -> Result<Option<MetricSnapshot>>;
}

// ---------------------------------------------------------------------------
// AddonCapabilities — produced by AddonPreset::from_cfg
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct AddonCapabilities {
    pub metadata: AddonMetadata,
    pub kind: Option<Arc<dyn AddonKind>>,
    pub catalog: Option<Arc<dyn CatalogAddon>>,
    pub meta: Option<Arc<dyn MetaAddon>>,
    pub stream: Option<Arc<dyn StreamAddon>>,
    pub search: Option<Arc<dyn SearchAddon>>,
    pub subtitle: Option<Arc<dyn SubtitleAddon>>,
    pub tree: Option<Arc<dyn TreeAddon>>,
    pub segment: Option<Arc<dyn SegmentAddon>>,
    pub lyric: Option<Arc<dyn LyricAddon>>,
    pub index: Option<Arc<dyn IndexAddon>>,
    pub metrics: Option<Arc<dyn MetricsAddon>>,
    pub media_tracker: Option<Arc<dyn media_tracker::MediaTrackerAddon>>,
}

// ---------------------------------------------------------------------------
// AddonRuntime — one entry in the service Vec
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AddonRuntime {
    pub row: Addon,
    pub caps: AddonCapabilities,
}

impl std::ops::Deref for AddonRuntime {
    type Target = AddonCapabilities;
    fn deref(&self) -> &Self::Target {
        &self.caps
    }
}

impl AddonRuntime {
    /// Fetches this addon's live catalog list and merges in its persisted
    /// per-catalog overrides (enabled/max_items/tags). Catalogs without a
    /// stored override fall back to the provider's own declared defaults.
    pub async fn resolve_catalogs(
        &self,
        ctx: &AppContext,
    ) -> Result<Vec<ResolvedCatalog>> {
        let Some(catalog) = self
            .catalog
            .as_ref()
        else {
            return Ok(vec![]);
        };
        let available = catalog
            .catalog_list(ctx)
            .await?;
        let states = self
            .row
            .catalog_states();
        let addon_id = self
            .row
            .id;
        Ok(available
            .into_iter()
            .map(|info| {
                let state = states.get(&info.provider_catalog_id);
                ResolvedCatalog {
                    catalog_id: make_media_id(addon_id, &info.provider_catalog_id),
                    collection_id: Uuid::new_v5(
                        &addon_id,
                        info.provider_catalog_id
                            .as_bytes(),
                    ),
                    enabled: state
                        .map(|s| s.enabled)
                        .unwrap_or(info.default_enabled),
                    max_items: state
                        .and_then(|s| s.max_items)
                        .or(info.default_max_items),
                    tags: state
                        .map(|s| {
                            s.tags
                                .clone()
                        })
                        .unwrap_or_default(),
                    provider_catalog_id: info.provider_catalog_id,
                    name: info.name,
                    media_kind: info.media_kind,
                    collection_media_kind: info.collection_media_kind,
                }
            })
            .collect())
    }

    pub fn supports_type(&self, kind: &db::MediaKind) -> bool {
        // Manifest types (live metadata) are the authoritative upper bound.
        // "Series" in a type list covers Episode and Season too (Stremio model).
        let mt: Vec<db::MediaKind> = self
            .caps
            .metadata
            .supported_types
            .iter()
            .cloned()
            .map(db::MediaKind::from)
            .collect();
        if !mt.is_empty() && !kind_in_type_list(kind, &mt) {
            return false;
        }
        self.row
            .types
            .is_empty()
            || kind_in_type_list(
                kind,
                &self
                    .row
                    .types,
            )
    }

    /// Returns the `idPrefixes` declared for a resource in the live manifest metadata,
    /// or `None` if the resource has no prefix restriction.
    fn resource_id_prefixes(&self, kind: &ResourceType) -> Option<&[String]> {
        self.caps
            .metadata
            .supported_resources
            .iter()
            .find(|r| &r.name == kind)
            .and_then(|r| {
                r.id_prefixes
                    .as_deref()
            })
    }
}

/// Returns true when `runtime` should run for the given user context.
///
/// `override_ids = None`        → no override active; only `is_default` addons run (background
///                                tasks and users whose addon list hasn't been customised).
/// `override_ids = Some(ids)`   → user has a custom addon list; only those addon IDs run,
///                                plus system addons which always run unconditionally.
fn user_scoped(runtime: &AddonRuntime, override_ids: Option<&[Uuid]>) -> bool {
    if runtime
        .row
        .system
    {
        return true;
    }
    match override_ids {
        None => {
            runtime
                .row
                .is_default
        }
        Some(ids) => ids.contains(
            &runtime
                .row
                .id,
        ),
    }
}

/// Returns `None` for a manifest type with no recognized `MediaKind`
/// equivalent (e.g. fankai's "anime") instead of collapsing it to `Movie`
/// like the default `From<stremio::MediaType>` impl does — that would make
/// `supports_type` believe an anime/series-only addon serves only movies,
/// excluding it from `addons_for::<dyn StreamAddon>` for every
/// Series/Season/Episode lookup.
fn recognized_manifest_media_kind(
    t: sdks::stremio::MediaType,
) -> Option<sdks::remux::MediaKind> {
    use sdks::{remux::MediaKind as MK, stremio::MediaType as MT};
    Some(match t {
        MT::Movie => MK::Movie,
        MT::Series => MK::Series,
        MT::Tv | MT::Channel => MK::TvChannel,
        MT::Album => MK::Album,
        MT::Artist => MK::Artist,
        MT::Track => MK::Track,
        MT::Events => MK::TvProgram,
        MT::Other(s) => match s.as_str() {
            "episode" => MK::Episode,
            "season" => MK::Season,
            "person" => MK::Person,
            "genre" => MK::Genre,
            "studio" => MK::Studio,
            "collection" => MK::Collection,
            "folder" => MK::Folder,
            "stream" => MK::Stream,
            "playlist" => MK::Playlist,
            _ => return None,
        },
    })
}

fn kind_in_type_list(kind: &db::MediaKind, list: &[db::MediaKind]) -> bool {
    list.contains(kind)
        || (matches!(kind, db::MediaKind::Episode | db::MediaKind::Season)
            && list.contains(&db::MediaKind::Series))
}

// ---------------------------------------------------------------------------
// AddonService
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AddonService {
    inner: Arc<ArcSwap<Vec<AddonRuntime>>>,
}

#[async_trait]
trait PickCap<T: ?Sized + Send + Sync> {
    async fn pick(&self, media: &db::Media) -> bool;
}

#[async_trait]
impl PickCap<dyn MetaAddon> for AddonRuntime {
    async fn pick(&self, media: &db::Media) -> bool {
        if !self
            .row
            .resources
            .contains(&ResourceType::Meta)
        {
            return false;
        }
        let Some(cap) = self
            .caps
            .meta
            .as_ref()
        else {
            return false;
        };
        if let Some(prefixes) = self.resource_id_prefixes(&ResourceType::Meta) {
            let gp_ext = media
                .grandparent
                .as_deref()
                .map(|gp| &gp.external_ids);
            let candidates = media.candidate_ids(gp_ext);
            if candidates.is_empty() {
                return false;
            }
            return candidates
                .iter()
                .any(|id| {
                    prefixes
                        .iter()
                        .any(|p| id.starts_with(p.as_str()))
                });
        }
        cap.supports(media)
            .await
    }
}

#[async_trait]
impl PickCap<dyn StreamAddon> for AddonRuntime {
    async fn pick(&self, media: &db::Media) -> bool {
        if !self
            .row
            .resources
            .contains(&ResourceType::Stream)
        {
            return false;
        }
        if let Some(prefixes) = self.resource_id_prefixes(&ResourceType::Stream) {
            let gp_ext = media
                .grandparent
                .as_deref()
                .map(|gp| &gp.external_ids);
            let candidates = media.candidate_ids(gp_ext);
            if candidates.is_empty() {
                return false;
            }
            return candidates
                .iter()
                .any(|id| {
                    prefixes
                        .iter()
                        .any(|p| id.starts_with(p.as_str()))
                });
        }
        match self
            .caps
            .stream
            .as_ref()
        {
            Some(cap) => cap.supports(media),
            None => false,
        }
    }
}

#[async_trait]
impl PickCap<dyn SubtitleAddon> for AddonRuntime {
    async fn pick(&self, media: &db::Media) -> bool {
        if !self
            .row
            .resources
            .contains(&ResourceType::Subtitles)
        {
            return false;
        }
        if let Some(prefixes) = self.resource_id_prefixes(&ResourceType::Subtitles) {
            let gp_ext = media
                .grandparent
                .as_deref()
                .map(|gp| &gp.external_ids);
            let candidates = media.candidate_ids(gp_ext);
            if candidates.is_empty() {
                return false;
            }
            return candidates
                .iter()
                .any(|id| {
                    prefixes
                        .iter()
                        .any(|p| id.starts_with(p.as_str()))
                });
        }
        match self
            .caps
            .subtitle
            .as_ref()
        {
            Some(cap) => cap.supports(media),
            None => false,
        }
    }
}

impl AddonService {
    async fn addons_for<T>(
        &self,
        media: &db::Media,
        db: &SqlitePool,
        user_id: Option<Uuid>,
    ) -> Vec<AddonRuntime>
    where
        T: ?Sized + Send + Sync + 'static,
        AddonRuntime: PickCap<T>,
    {
        let override_ids = match user_id {
            Some(uid) => match addon::user_addon_override(db, uid).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!(error = %e, "failed to load user addon override");
                    return Vec::new();
                }
            },
            None => None,
        };
        let all = self
            .inner
            .load();
        let mut out = Vec::new();
        for r in all
            .iter()
            .filter(|r| r.supports_type(&media.kind))
            .filter(|r| user_scoped(r, override_ids.as_deref()))
        {
            if PickCap::<T>::pick(r, media).await {
                out.push(r.clone());
            }
        }
        if let Some(ids) = &override_ids {
            out.sort_by_key(|r| {
                ids.iter()
                    .position(|id| {
                        *id == r
                            .row
                            .id
                    })
                    .unwrap_or(usize::MAX)
            });
        }
        out
    }

    /// The media tracker capability of one enabled addon, if it has one. Returns
    /// `None` once an addon is disabled or deleted, which is why queued
    /// deliveries treat that as permanent rather than retrying forever.
    pub fn media_tracker_for(
        &self,
        addon_id: Uuid,
    ) -> Option<Arc<dyn media_tracker::MediaTrackerAddon>> {
        self.inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == addon_id
                    && r.row
                        .enabled
            })
            .and_then(|r| {
                r.caps
                    .media_tracker
                    .clone()
            })
    }

    /// Whether any enabled addon can track at all. Nothing can be connected to
    /// an addon that is not installed, so this answers without a query.
    pub fn has_media_tracker(&self) -> bool {
        self.inner
            .load()
            .iter()
            .any(|r| {
                r.row
                    .enabled
                    && r.caps
                        .media_tracker
                        .is_some()
            })
    }

    /// Swaps the live runtime list. Test-only: the real list is built from
    /// `registered_presets()`, which has no way to carry a stub addon, so
    /// without this seam the delivery path can only be exercised by shipping a
    /// provider.
    #[cfg(test)]
    pub fn replace_runtimes_for_test(&self, runtimes: Vec<AddonRuntime>) {
        self.inner
            .store(Arc::new(runtimes));
    }

    pub async fn list_for_user(
        &self,
        db: &SqlitePool,
        user_id: Option<Uuid>,
    ) -> Vec<AddonRuntime> {
        let override_ids = match user_id {
            Some(uid) => match addon::user_addon_override(db, uid).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!(error = %e, "failed to load user addon override");
                    return Vec::new();
                }
            },
            None => None,
        };
        let mut out: Vec<AddonRuntime> = self
            .inner
            .load()
            .iter()
            .filter(|r| user_scoped(r, override_ids.as_deref()))
            .cloned()
            .collect();
        if let Some(ids) = &override_ids {
            out.sort_by_key(|r| {
                ids.iter()
                    .position(|id| {
                        *id == r
                            .row
                            .id
                    })
                    .unwrap_or(usize::MAX)
            });
        }
        out
    }

    pub async fn from_db(db: &SqlitePool, config: &crate::Config) -> Result<Self> {
        let runtimes = Self::load_runtimes(db, config).await?;
        Ok(Self {
            inner: Arc::new(ArcSwap::from_pointee(runtimes)),
        })
    }

    async fn load_runtimes(
        db: &SqlitePool,
        config: &crate::Config,
    ) -> Result<Vec<AddonRuntime>> {
        let presets = registered_presets();
        let addons = Addon::list(db).await?;
        let mut runtimes = Vec::new();

        for mut addon in addons
            .into_iter()
            .filter(|a| a.enabled)
        {
            let Some(preset) = presets
                .iter()
                .find(|p| {
                    p.id()
                        == addon
                            .preset
                            .kind
                })
            else {
                warn!(
                    addon_id = %addon.id,
                    kind = %addon.preset.kind,
                    "skipping addon with unknown preset kind"
                );
                continue;
            };
            match preset.from_cfg(
                addon.id,
                addon
                    .preset
                    .config
                    .expose(),
                config,
            ) {
                Ok(mut caps) => {
                    // Start with the preset's static metadata, then upgrade with live manifest data.
                    caps.metadata = preset.metadata();
                    if let Some(ref kind) = caps.kind {
                        match kind
                            .available_info()
                            .await
                        {
                            Ok(Some((resource_refs, raw_types))) => {
                                caps.metadata
                                    .supported_resources = resource_refs;
                                if !raw_types.is_empty() {
                                    caps.metadata
                                        .supported_types = raw_types
                                        .into_iter()
                                        .filter_map(recognized_manifest_media_kind)
                                        .collect();
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(
                                    addon_id = %addon.id,
                                    name = %addon.name,
                                    error = %e,
                                    "failed to fetch addon manifest at load time"
                                );
                            }
                        }
                    }
                    runtimes.push(AddonRuntime { row: addon, caps });
                }
                Err(e) => warn!(
                    addon_id = %addon.id,
                    kind = %addon.preset.kind,
                    error = %e,
                    "failed to instantiate addon"
                ),
            }
        }
        Ok(runtimes)
    }

    pub async fn reload(&self, db: &SqlitePool, config: &crate::Config) -> Result<()> {
        let runtimes = Self::load_runtimes(db, config).await?;
        self.inner
            .store(Arc::new(runtimes));
        Ok(())
    }

    pub fn list(&self) -> arc_swap::Guard<Arc<Vec<AddonRuntime>>> {
        self.inner
            .load()
    }

    pub fn get(&self, id: Uuid) -> Option<AddonRuntime> {
        self.inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == id
            })
            .cloned()
    }

    pub fn catalog_addons(&self) -> Vec<AddonRuntime> {
        self.inner
            .load()
            .iter()
            .filter(|r| {
                r.catalog
                    .is_some()
            })
            .cloned()
            .collect()
    }

    pub fn metrics_addons(&self) -> Vec<AddonRuntime> {
        self.inner
            .load()
            .iter()
            .filter(|r| {
                r.metrics
                    .is_some()
                    && r.row
                        .resources
                        .contains(&ResourceType::Metrics)
            })
            .cloned()
            .collect()
    }

    pub async fn snapshot_all_metrics(
        &self,
        ctx: &AppContext,
        progress: ProgressReporter,
    ) -> Result<()> {
        use futures::stream::{self, StreamExt as _};
        use std::sync::atomic::{AtomicU64, Ordering};

        let addons = self.metrics_addons();
        if addons.is_empty() {
            progress.set(100.0);
            return Ok(());
        }

        let settings = db::Settings::get_config_or_default(&ctx.db).await;
        let metrics_ctx = MetricsCtx {
            config: Arc::new(
                ctx.config
                    .clone(),
            ),
            settings,
        };

        let total: u64 = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM media WHERE kind IN ('movie', 'series')",
        )
        .fetch_one(&ctx.db)
        .await
        .unwrap_or(0) as u64;

        let num_addons = addons.len() as u64;
        let grand_total = total * num_addons;
        // Shared counter: done items + newly fetched items across all addon loops.
        let processed = Arc::new(AtomicU64::new(0));

        const PAGE: u32 = 250;
        const CONCURRENCY: usize = 25;

        // Each addon runs its own independent paging loop concurrently.
        // This way a rate-limited addon (e.g. Trakt sleeping 5 min) doesn't
        // stall the others.
        futures::future::join_all(
            addons
                .iter()
                .map(|runtime| {
                    let addon = runtime.metrics.as_ref().unwrap().clone();
                    let metrics_ctx = metrics_ctx.clone();
                    let progress = progress.clone();
                    let processed = Arc::clone(&processed);
                    async move {
                        let done: std::collections::HashSet<uuid::Uuid> =
                            sqlx::query_scalar::<_, Option<uuid::Uuid>>(
                                "SELECT media_id FROM popularity_raw \
                                 WHERE source = ? AND date = date('now') AND media_id IS NOT NULL",
                            )
                            .bind(addon.id())
                            .fetch_all(&ctx.db)
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .flatten()
                            .collect();

                        tracing::info!(
                            source = addon.id(),
                            already_fetched = done.len(),
                            remaining = (total as usize).saturating_sub(done.len()),
                            "starting metrics fetch"
                        );

                        // Credit already-fetched items immediately so progress
                        // reflects a resumed or partial run from the start.
                        if grand_total > 0 && !done.is_empty() {
                            let n = processed.fetch_add(done.len() as u64, Ordering::Relaxed)
                                + done.len() as u64;
                            progress.set((n as f64 / grand_total as f64 * 100.0).min(99.0));
                        }

                        let mut offset = 0u32;
                        loop {
                            let page = db::Media::get_by_filter(
                                &ctx.db,
                                &db::MediaFilter {
                                    kind: Some(vec![db::MediaKind::Movie, db::MediaKind::Series]),
                                    limit: Some(PAGE),
                                    offset: Some(offset),
                                    total_count: false,
                                    ..Default::default()
                                },
                            )
                            .await?
                            .records;

                            if page.is_empty() {
                                break;
                            }

                            let batch: Vec<_> =
                                page.into_iter().filter(|m| !done.contains(&m.id)).collect();
                            offset += PAGE;
                            if batch.is_empty() {
                                continue;
                            }

                            let batch_len = batch.len() as u64;
                            let snapshots: Vec<MetricSnapshot> = stream::iter(batch)
                                .map(|item| {
                                    let addon = addon.clone();
                                    let ctx = metrics_ctx.clone();
                                    async move { addon.metric(&item, &ctx).await.ok().flatten() }
                                })
                                .buffer_unordered(CONCURRENCY)
                                .filter_map(|s| async move { s })
                                .collect()
                                .await;

                            if !snapshots.is_empty() {
                                bulk_insert_snapshots(ctx, &snapshots).await?;
                            }

                            if grand_total > 0 {
                                let n = processed.fetch_add(batch_len, Ordering::Relaxed)
                                    + batch_len;
                                progress.set((n as f64 / grand_total as f64 * 100.0).min(99.0));
                            }
                        }
                        Ok::<(), anyhow::Error>(())
                    }
                }),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

        progress.set(100.0);
        Ok(())
    }

    /// Returns `(addon, catalogs)` pairs for every catalog-capable addon that could
    /// produce any of `kinds`, with each addon's catalog list already filtered down to
    /// catalogs whose own `media_kind` is one of `kinds`. Addons are pre-filtered via
    /// `supports_type` as a cheap upper-bound check before calling `catalog_list()`
    /// (which may hit the network); per-addon listing errors are logged and skipped.
    pub async fn catalogs_for_kinds(
        &self,
        ctx: &AppContext,
        kinds: &[db::MediaKind],
    ) -> Vec<(AddonRuntime, Vec<ResolvedCatalog>)> {
        let mut out = Vec::new();
        for runtime in self
            .catalog_addons()
            .into_iter()
            .filter(|r| {
                kinds
                    .iter()
                    .any(|k| r.supports_type(k))
            })
        {
            let addon_id = runtime
                .row
                .id;
            let resolved = match runtime
                .resolve_catalogs(ctx)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to list addon catalogs, skipping");
                    continue;
                }
            };
            if resolved.is_empty() {
                continue;
            }
            out.push((runtime, resolved));
        }
        out
    }

    pub async fn purge_indexes(&self, ctx: &AppContext) -> Result<()> {
        let addons: Vec<AddonRuntime> = self
            .inner
            .load()
            .iter()
            .cloned()
            .collect();
        for runtime in &addons {
            if let Some(index) = &runtime.index {
                if let Err(e) = index
                    .purge_index(ctx, &runtime.row)
                    .await
                {
                    warn!(addon = %runtime.row.name, error = %e, "purge_index failed");
                }
            }
        }
        Ok(())
    }

    pub async fn refresh_indexes(
        &self,
        ctx: &AppContext,
        progress: ProgressReporter,
    ) -> Result<()> {
        let addons: Vec<AddonRuntime> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.row
                    .enabled
            })
            .cloned()
            .collect();
        let total = addons.len();
        for (idx, runtime) in addons
            .iter()
            .enumerate()
        {
            if let Some(index) = &runtime.index {
                let sub = progress.step(idx, total);
                if let Err(e) = index
                    .refresh_index(ctx, &runtime.row, sub)
                    .await
                {
                    warn!(addon = %runtime.row.name, error = %e, "refresh_index failed");
                }
            }
        }
        progress.set(100.0);
        Ok(())
    }

    pub fn get_catalog(&self, id: Uuid) -> Option<Arc<dyn CatalogAddon>> {
        self.inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == id
            })
            .and_then(|r| {
                r.catalog
                    .clone()
            })
    }

    /// Return the tags configured for a specific catalog within an addon.
    pub fn catalog_tags(&self, addon_uuid: &str, local_cat_id: &str) -> Vec<String> {
        let Ok(id) = Uuid::parse_str(addon_uuid) else {
            return vec![];
        };
        self.inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == id
            })
            .map(|r| {
                r.row
                    .catalog_states()
                    .get(local_cat_id)
                    .map(|s| {
                        s.tags
                            .clone()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    pub fn make_catalog_stream(
        &self,
        media_id: &str,
    ) -> Option<Box<dyn RemoteMediaStream>> {
        let rest = media_id.strip_prefix("addon:")?;
        let (uuid_str, local_id) = rest.split_once(':')?;
        let id = Uuid::parse_str(uuid_str).ok()?;
        let addon = self
            .inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == id
            })
            .and_then(|r| {
                r.catalog
                    .clone()
            })?;
        Some(Box::new(AddonCatalogStream {
            addon,
            local_id: local_id.to_string(),
        }))
    }

    #[tracing::instrument(skip_all, fields(title = %media.title, kind = %media.kind))]
    pub async fn refresh_meta(
        &self,
        media: &mut db::Media,
        ctx: &AppContext,
        force_refresh: bool,
        config: &api::ServerConfiguration,
    ) -> Result<()> {
        media
            .grandparent(&ctx.db)
            .await
            .ok();

        let applicable = self
            .addons_for::<dyn MetaAddon>(media, &ctx.db, None)
            .await;

        if applicable.is_empty() {
            return Ok(());
        }

        let results = futures::future::join_all(
            applicable
                .iter()
                .map(|r| {
                    r.meta
                        .as_ref()
                        .unwrap()
                        .meta_fetch(media, ctx, config)
                }),
        )
        .await;

        // Accumulate all addon patches into a fresh empty object so the
        // highest-priority addon (first in list, lowest priority number) wins
        // each field — later addons only fill gaps. The real `media` stays
        // untouched until the combined result is applied once at the end,
        // where `force_refresh` controls whether existing values are replaced.
        let mut combined: Option<db::Media> = None;
        for (r, result) in applicable
            .iter()
            .zip(results)
        {
            match result {
                Ok(Some(patch)) => {
                    let acc = combined.get_or_insert_with(db::Media::default);
                    apply_meta(acc, patch, false);
                }
                Ok(None) => {}
                Err(e) => {
                    error!(addon = %r.row.name, error = ?e, "meta addon error")
                }
            }
        }
        if let Some(combined) = combined {
            apply_meta(media, combined, force_refresh);
        }

        // Apply SxxExx / "Season N" title formatting once, after all patches are merged.
        // Calling it inside apply_meta would re-apply the prefix on every patch.
        apply_title_format(media);

        // Recompute stable UUID once external IDs are resolved by meta enrichment.
        // Season/Episode UUIDs are anchored to parent_id, not external IDs — skip them.
        if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series) {
            let raw = media.media_id_raw();
            if raw
                .canonical()
                .is_some()
            {
                media.id = uuid::Uuid::from(&raw);
            }
        }
        if media.kind == db::MediaKind::Person {
            if let Some(tmdb_id) = media
                .external_ids
                .tmdb
            {
                media.id = crate::common::stable_media_uuid(
                    &db::MediaKind::Person,
                    &tmdb_id.to_string(),
                );
            }
        }

        media.refreshed_at = Some(chrono::Utc::now().naive_utc());

        Ok(())
    }

    pub fn get_tree(
        &self,
        root: db::Media,
        ctx: &AppContext,
    ) -> impl futures::Stream<Item = db::Media> + 'static {
        let svc = self.clone();
        let ctx = ctx.clone();
        async_stream::stream! {
            let mut seen = std::collections::HashSet::new();
            seen.insert(root.id);
            let root_title = root.title.clone();
            let root_id = root.id;
            let mut queue = vec![root];
            let mut total_yielded = 0usize;

            while let Some(node) = queue.pop() {
                let applicable: Vec<Arc<dyn TreeAddon>> = svc
                    .inner
                    .load()
                    .iter()
                    .filter_map(|r| {
                        if !r
                            .tree
                            .as_ref()
                            .map(|t| t.supports(&node))
                            .unwrap_or(false)
                        {
                            return None;
                        }
                        if let Some(prefixes) = r.resource_id_prefixes(&ResourceType::Meta)
                        {
                            let gp_ext = node
                                .grandparent
                                .as_deref()
                                .map(|gp| &gp.external_ids);
                            let candidates = node.candidate_ids(gp_ext);
                            if candidates.is_empty()
                                || !candidates
                                    .iter()
                                    .any(|id| prefixes.iter().any(|p| id.starts_with(p.as_str())))
                            {
                                return None;
                            }
                        }
                        r.tree
                            .as_ref()
                            .cloned()
                    })
                    .collect();

                for addon in &applicable {
                    match addon
                        .get_children(&node, &ctx)
                        .await
                    {
                        Ok(Some(children)) if !children.is_empty() => {
                            for child in children {
                                if seen.insert(child.id) {
                                    let is_leaf = matches!(
                                        child.kind,
                                        db::MediaKind::Episode | db::MediaKind::Track
                                    );
                                    if !is_leaf {
                                        queue.push(child.clone());
                                    }
                                    total_yielded += 1;
                                    yield child;
                                }
                            }
                            break;
                        }
                        Ok(_) => continue,
                        Err(e) => {
                            debug!(id = %node.id, error = %e, "get_children failed");
                            continue;
                        }
                    }
                }
            }
        }
    }

    pub async fn process_meta_batch(
        &self,
        media: Vec<db::Media>,
        ctx: &AppContext,
        force_refresh: bool,
        on_item_done: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<()> {
        use futures::StreamExt;

        let config = db::Settings::get_config_or_default(&ctx.db).await;
        let concurrency = config.meta_concurrency as usize;
        let config = Arc::new(config);

        let svc = self.clone();
        let ctx_owned = ctx.clone();

        futures::stream::iter(media)
            .map(move |m| {
                let svc = svc.clone();
                let ctx = ctx_owned.clone();
                let cfg = Arc::clone(&config);
                async move {
                    svc.process_meta_item(m, ctx, force_refresh, cfg)
                        .await
                }
            })
            .buffer_unordered(concurrency)
            .for_each(move |_| {
                if let Some(ref f) = on_item_done {
                    f();
                }
                async {}
            })
            .await;

        Ok(())
    }

    /// Fetch the direct children of `node` from the first applicable tree addon.
    /// Returns an empty vec if no addon supports this node or none return children.
    async fn get_direct_children(
        &self,
        node: &db::Media,
        ctx: &AppContext,
    ) -> Vec<db::Media> {
        let applicable: Vec<Arc<dyn TreeAddon>> = self
            .inner
            .load()
            .iter()
            .filter_map(|r| {
                if !r
                    .row
                    .resources
                    .contains(&ResourceType::Meta)
                {
                    return None;
                }
                if !r
                    .tree
                    .as_ref()
                    .map(|t| t.supports(node))
                    .unwrap_or(false)
                {
                    return None;
                }
                if let Some(prefixes) = r.resource_id_prefixes(&ResourceType::Meta) {
                    let gp_ext = node
                        .grandparent
                        .as_deref()
                        .map(|gp| &gp.external_ids);
                    let candidates = node.candidate_ids(gp_ext);
                    if candidates.is_empty()
                        || !candidates
                            .iter()
                            .any(|id| {
                                prefixes
                                    .iter()
                                    .any(|p| id.starts_with(p.as_str()))
                            })
                    {
                        return None;
                    }
                }
                r.tree
                    .as_ref()
                    .cloned()
            })
            .collect();

        for addon in &applicable {
            match addon
                .get_children(node, ctx)
                .await
            {
                Ok(Some(children)) if !children.is_empty() => return children,
                Ok(_) => continue,
                Err(e) => debug!(id = %node.id, error = %e, "get_children failed"),
            }
        }
        vec![]
    }

    /// Load a map of `(kind_str, idx) → existing_uuid` for all direct children
    /// of `parent_id` that have a non-null idx. Used to adopt existing UUIDs
    /// for children arriving with a different computed UUID.
    async fn child_uuid_map(
        &self,
        db: &sqlx::SqlitePool,
        parent_id: Uuid,
    ) -> std::collections::HashMap<(String, i64), Uuid> {
        sqlx::query_as::<_, (String, Option<i64>, Uuid)>(
            "SELECT CAST(kind AS TEXT), idx, id FROM media WHERE parent_id = ? AND idx IS NOT NULL",
        )
        .bind(parent_id)
        .fetch_all(db)
        .await
        .inspect_err(|e| error!(parent_id = %parent_id, error = %e, "child_uuid_map query failed"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, idx, id)| idx.map(|i| ((k, i), id)))
        .collect()
    }

    pub(crate) async fn process_meta_item(
        &self,
        mut media: db::Media,
        ctx: AppContext,
        force_refresh: bool,
        config: Arc<api::ServerConfiguration>,
    ) -> Uuid {
        let original_id = media.id;

        if let Err(e) = self
            .refresh_meta(&mut media, &ctx, force_refresh, &config)
            .await
        {
            warn!(id = %media.id, error = %e, "failed to refresh metadata, keeping as-is");
            if let Err(e) = db::Media::upsert(&ctx.db, &[media.clone()]).await {
                error!(id = %media.id, error = %e, "failed to upsert media");
            } else {
                save_pending_relations(&ctx, &[media.clone()]).await;
                save_pending_tags(&ctx, &[media.clone()]).await;
                save_pending_popularity(&ctx, &[media.clone()]).await;
            }
            return media.id;
        }

        // If this Person's ID was rewritten (name-keyed → tmdb-keyed) by refresh_meta,
        // delete the stale name-keyed row so it doesn't linger as a duplicate.
        if media.kind == db::MediaKind::Person && media.id != original_id {
            if let Err(e) = db::Media::delete(&ctx.db, &original_id).await {
                warn!(
                    old_id = %original_id,
                    new_id = %media.id,
                    error = %e,
                    "failed to delete stale name-keyed person row"
                );
            }
        }

        // Resolve actual UUID: adopt an existing DB row that shares any external ID.
        // Cascades stale parent_id / grandparent_id references in existing children
        // before adopting, so those rows stay attached to the correct parent UUID.
        let computed_id = media.id;
        let mut root_was_remapped = false;
        if let Some(existing_id) =
            db::Media::find_existing_id_by_ext(&ctx.db, &media).await
        {
            if existing_id != computed_id {
                if let Err(e) = db::Media::cascade_update_parent_refs(
                    &ctx.db,
                    computed_id,
                    existing_id,
                )
                .await
                {
                    warn!(old = %computed_id, new = %existing_id, error = %e,
                        "cascade_update_parent_refs failed");
                }
                media.id = existing_id;
                root_was_remapped = true;
            }
        }
        let actual_root_id = media.id;

        // Build in-memory grandparent stub so children's refresh_meta calls can read
        // the series TMDB/IMDB ID and genres without hitting the DB.
        let gp_stub = {
            let mut gp = db::Media::default();
            gp.id = actual_root_id;
            gp.external_ids = media
                .external_ids
                .clone();
            if let Some(rels) = media
                .relations
                .as_ref()
            {
                let genre_rels: Vec<(db::MediaRelation, db::Media)> = rels
                    .iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Genre)
                    .cloned()
                    .collect();
                if !genre_rels.is_empty() {
                    gp.relations = Some(genre_rels);
                }
            }
            gp
        };

        // Upsert root.
        if let Err(e) = db::Media::upsert(&ctx.db, &[media.clone()]).await {
            error!(id = %actual_root_id, error = %e, "failed to upsert root media");
            return actual_root_id;
        }
        db::UserMediaState::remap_orphaned_for(&ctx.db, &[media.clone()]).await;
        save_pending_relations(&ctx, &[media.clone()]).await;
        save_pending_tags(&ctx, &[media.clone()]).await;
        save_pending_popularity(&ctx, &[media.clone()]).await;

        let is_continuing = series_is_active(&media.status);

        // Level 1: direct children (Seasons, Albums, etc.)
        let raw_level1 = self
            .get_direct_children(&media, &ctx)
            .await;
        if raw_level1.is_empty() {
            self.notify_series_done(&media);
            return actual_root_id;
        }

        // Always load existing level-1 UUIDs by (kind, idx) position. This lets us adopt
        // the stored UUID for any child whose UUID scheme changed (e.g. old series_imdb-
        // anchored → new parent_id-anchored). Also handles root-remap cases.
        let existing_l1 = self
            .child_uuid_map(&ctx.db, actual_root_id)
            .await;

        // Load ALL grandchild UUIDs in one query keyed by (parent_id, kind, idx).
        // Avoids one query per season (O(n_seasons) → O(1) queries).
        let existing_l2: std::collections::HashMap<(Uuid, String, i64), Uuid> =
            sqlx::query_as::<_, (Uuid, String, Option<i64>, Uuid)>(
                "SELECT parent_id, CAST(kind AS TEXT), idx, id
                 FROM media WHERE grandparent_id = ? AND idx IS NOT NULL",
            )
            .bind(actual_root_id)
            .fetch_all(&ctx.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(pid, k, idx, id)| idx.map(|i| ((pid, k, i), id)))
            .collect();

        let mut level1: Vec<db::Media> = Vec::with_capacity(raw_level1.len());
        for mut child in raw_level1 {
            child.parent_id = Some(actual_root_id);
            child.grandparent = Some(Box::new(gp_stub.clone()));

            // Adopt the existing DB UUID for this (kind, idx) position if found.
            // The new child UUID may differ from what's stored (due to UUID scheme changes
            // or root remapping) — adopting the stored UUID avoids duplicate rows and
            // keeps grandchild parent_id references intact.
            if let Some(idx) = child.idx {
                let key = (
                    child
                        .kind
                        .to_string(),
                    idx,
                );
                if let Some(&existing_id) = existing_l1.get(&key) {
                    if existing_id != child.id {
                        // When the root was remapped, cascade any references to the new
                        // child UUID (which was never in the DB) before adopting.
                        if root_was_remapped {
                            if let Err(e) = db::Media::cascade_update_parent_refs(
                                &ctx.db,
                                child.id,
                                existing_id,
                            )
                            .await
                            {
                                warn!(old = %child.id, new = %existing_id, error = %e,
                                    "cascade for level-1 child failed");
                            }
                        }
                        child.id = existing_id;
                    }
                }
            }

            let in_active_window = is_continuing
                && matches!(child.kind, db::MediaKind::Episode)
                && episode_in_active_window(&child);
            if let Some(effective_force) =
                child_refresh_force(force_refresh, in_active_window, &child)
            {
                if let Err(e) = self
                    .refresh_meta(&mut child, &ctx, effective_force, &config)
                    .await
                {
                    warn!(id = %child.id, error = %e, "failed to refresh level-1 child meta");
                }
            }
            level1.push(child);
        }

        let mut level1_ok: Vec<&db::Media> = Vec::with_capacity(level1.len());
        for chunk in level1.chunks(db::CHUNK_SIZE) {
            if let Err(e) = db::Media::upsert(&ctx.db, chunk).await {
                error!(error = %e, "failed to upsert level-1 children");
            } else {
                db::UserMediaState::remap_orphaned_for(&ctx.db, chunk).await;
                save_pending_relations(&ctx, chunk).await;
                save_pending_tags(&ctx, chunk).await;
                save_pending_popularity(&ctx, chunk).await;
                level1_ok.extend(chunk);
            }
        }

        // Level 2: grandchildren (Episodes, Tracks, etc.) — one fetch+upsert per child.
        // Only process children whose level-1 upsert succeeded to avoid orphaned rows.
        for child in &level1_ok {
            let actual_child_id = child.id;
            let raw_level2 = self
                .get_direct_children(child, &ctx)
                .await;
            if raw_level2.is_empty() {
                continue;
            }

            let mut level2: Vec<db::Media> = Vec::with_capacity(raw_level2.len());
            for mut gc in raw_level2 {
                gc.parent_id = Some(actual_child_id);
                gc.grandparent_id = Some(actual_root_id);
                gc.grandparent = Some(Box::new(gp_stub.clone()));

                // Adopt existing UUID from the pre-loaded grandchild map.
                if let Some(idx) = gc.idx {
                    let key = (
                        actual_child_id,
                        gc.kind
                            .to_string(),
                        idx,
                    );
                    if let Some(&existing_id) = existing_l2.get(&key) {
                        gc.id = existing_id;
                    }
                }

                let in_active_window = is_continuing
                    && matches!(gc.kind, db::MediaKind::Episode)
                    && episode_in_active_window(&gc);
                if let Some(effective_force) =
                    child_refresh_force(force_refresh, in_active_window, &gc)
                {
                    if let Err(e) = self
                        .refresh_meta(&mut gc, &ctx, effective_force, &config)
                        .await
                    {
                        warn!(id = %gc.id, error = %e, "failed to refresh level-2 child meta");
                    }
                }
                level2.push(gc);
            }

            for chunk in level2.chunks(db::CHUNK_SIZE) {
                if let Err(e) = db::Media::upsert(&ctx.db, chunk).await {
                    error!(error = %e, "failed to upsert level-2 children");
                } else {
                    db::UserMediaState::remap_orphaned_for(&ctx.db, chunk).await;
                    save_pending_relations(&ctx, chunk).await;
                    save_pending_tags(&ctx, chunk).await;
                    save_pending_popularity(&ctx, chunk).await;
                }
            }
        }

        self.notify_series_done(&media);
        actual_root_id
    }

    fn notify_series_done(&self, media: &db::Media) {
        if let Some(meta_id) = media
            .external_ids
            .stremio_lookup_id()
        {
            for r in self
                .inner
                .load()
                .iter()
            {
                if let Some(ref meta_addon) = r.meta {
                    meta_addon.on_series_done(&meta_id);
                }
            }
        }
    }

    pub async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
        user_id: Option<Uuid>,
    ) -> Result<Vec<db::Media>> {
        let override_ids = match user_id {
            Some(uid) => addon::user_addon_override(&ctx.db, uid)
                .await
                .unwrap_or(None),
            None => None,
        };
        let addons: Vec<AddonRuntime> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.supports_type(kind)
                    && r.row
                        .resources
                        .contains(&ResourceType::Search)
                    && r.search
                        .is_some()
                    && user_scoped(r, override_ids.as_deref())
            })
            .cloned()
            .collect();

        for r in addons {
            if !r
                .search
                .as_ref()
                .unwrap()
                .search_supports(kind)
                .await
            {
                continue;
            }
            match r
                .search
                .as_ref()
                .unwrap()
                .search(kind, query, limit, ctx)
                .await
            {
                Ok(Some(results)) => {
                    for m in &results {
                        ctx.store
                            .save(
                                m.id.to_string(),
                                m.clone(),
                                Duration::from_secs(3600),
                            );
                    }
                    return Ok(results);
                }
                Ok(None) => continue,
                Err(e) => {
                    warn!(addon = %r.row.name, error = %e, "search addon error")
                }
            }
        }
        Ok(vec![])
    }

    #[tracing::instrument(skip_all, fields(title = %media.title, kind = %media.kind))]
    pub async fn fetch_images(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        let addons = self
            .addons_for::<dyn MetaAddon>(media, &ctx.db, None)
            .await;

        let mut out = Vec::new();
        for r in addons {
            match r
                .meta
                .as_ref()
                .unwrap()
                .images_fetch(media, ctx)
                .await
            {
                Ok(images) => out.extend(images),
                Err(e) => {
                    warn!(addon = %r.row.name, error = %e, "images_fetch failed")
                }
            }
        }
        Ok(out)
    }

    #[tracing::instrument(skip_all, fields(title = %media.title, kind = %media.kind))]
    pub async fn fetch_subtitles(
        &self,
        media: &mut db::Media,
        db: &SqlitePool,
        background: bool,
        user_id: Option<Uuid>,
    ) -> Vec<SubtitleInfo> {
        if media.kind == db::MediaKind::Episode {
            media
                .grandparent(db)
                .await
                .ok();
        }
        let addons = self
            .addons_for::<dyn SubtitleAddon>(media, db, user_id)
            .await;

        debug!(count = addons.len(), "subtitle addons matched");
        let instant = Instant::now();
        let mut subs = vec![];
        for r in &addons {
            debug!(addon = %r.row.name, "fetching subtitles from addon");
            match r
                .subtitle
                .as_ref()
                .unwrap()
                .subtitle_fetch(media, db)
                .await
            {
                Ok(s) => {
                    debug!(addon = %r.row.name, count = s.len(), "subtitle addon returned results");
                    subs.extend(s);
                }
                Err(e) => {
                    warn!(addon = %r.row.name, error = %e, "subtitle addon failed")
                }
            }
        }
        if background {
            debug!(subs = subs.len(), addons = addons.len(), elapsed = ?instant.elapsed(), "subtitles fetched");
        } else {
            info!(subs = subs.len(), addons = addons.len(), elapsed = ?instant.elapsed(), "subtitles fetched");
        }
        subs
    }

    pub async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        user_id: Option<Uuid>,
    ) -> Result<Vec<db::Media>> {
        let addons = self
            .addons_for::<dyn StreamAddon>(media, &ctx.db, user_id)
            .await;

        debug!(
            media_id = %media.id,
            media_kind = ?media.kind,
            addon_count = addons.len(),
            "resolving streams"
        );

        let tasks: Vec<_> = addons
            .into_iter()
            .map(|r| async move {
                let name = &r.row.name;
                let t = std::time::Instant::now();
                let id_prefixes = r
                    .resource_id_prefixes(&ResourceType::Stream)
                    .map(|p| p.to_vec());
                match r
                    .stream
                    .as_ref()
                    .unwrap()
                    .get_streams(media, ctx, id_prefixes.as_deref())
                    .await
                {
                    Ok(mut streams) => {
                        let elapsed = t.elapsed();
                        if streams.is_empty() {
                            debug!(addon = %name, ?elapsed, "addon: no streams");
                        } else {
                            let sf = &r.row.service_filter;
                            if !sf.is_empty() {
                                streams.retain(|s| {
                                    let service_match = s.service_id
                                        .as_deref()
                                        .map(|id| sf.iter().any(|f| f.eq_ignore_ascii_case(id)))
                                        .unwrap_or(false);
                                    let addon_match = s.stream_addon
                                        .as_deref()
                                        .map(|a| sf.iter().any(|f| f.eq_ignore_ascii_case(a)))
                                        .unwrap_or(false);
                                    service_match || addon_match
                                });
                            }
                            debug!(addon = %name, count = streams.len(), ?elapsed, "addon: streams found");
                            let addon_id = r.row.id;
                            for s in &mut streams {
                                s.source = Some(name.clone());
                                s.addon_id = Some(addon_id);
                            }
                        }
                        streams
                    }
                    Err(e) => {
                        warn!(addon = %name, error = %e, elapsed = ?t.elapsed(), "stream addon failed");
                        vec![]
                    }
                }
            })
            .collect();
        let all: Vec<db::Media> = futures::future::join_all(tasks)
            .await
            .into_iter()
            .flatten()
            .map(db::Media::from)
            .collect();
        Ok(all)
    }

    fn stream_dedup_key(s: &db::Media) -> Option<String> {
        let si = s
            .stream_info
            .as_ref()?;
        match &si.descriptor {
            crate::stream::StreamDescriptor::Torrent {
                info_hash,
                file_hint,
                file_idx,
                ..
            } => {
                let file_key = file_hint
                    .as_deref()
                    .or(si
                        .filename
                        .as_deref())
                    .filter(|name| !name.is_empty())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_else(|| {
                        file_idx
                            .map(|index| format!("#{index}"))
                            .unwrap_or_default()
                    });
                Some(format!("torrent:{}:{file_key}", info_hash.to_lowercase()))
            }
            crate::stream::StreamDescriptor::Http { url, .. } => {
                let filename = si
                    .filename
                    .as_deref()
                    .unwrap_or("");
                let size = si
                    .size
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let binge_group = si
                    .binge_group
                    .as_deref()
                    .unwrap_or("");
                let service_id = si
                    .service_id
                    .as_deref()
                    .unwrap_or("");
                let addon_id = si
                    .addon_id
                    .map(|id| id.to_string())
                    .unwrap_or_default();

                if filename.is_empty()
                    && size.is_empty()
                    && binge_group.is_empty()
                    && service_id.is_empty()
                    && addon_id.is_empty()
                {
                    // Plain HTTP with no stable metadata — fall back to URL path.
                    let stable = url
                        .split('?')
                        .next()
                        .unwrap_or(url.as_str());
                    return Some(format!("http:{stable}"));
                }

                Some(format!(
                    "http:{}:{}:{}:{}:{}",
                    filename, size, binge_group, service_id, addon_id
                ))
            }
            crate::stream::StreamDescriptor::Local(path) => {
                Some(format!("local:{}", path.display()))
            }
            crate::stream::StreamDescriptor::Rtsp { url } => {
                Some(format!("rtsp:{url}"))
            }
            crate::stream::StreamDescriptor::Opendal { addon_id, path } => {
                Some(format!("opendal:{addon_id}:{path}"))
            }
        }
    }
}

fn strip_video_ext(name: &str) -> &str {
    if let Some((stem, ext)) = name.rsplit_once('.') {
        if remux_sdks::remux::VideoContainer::parse_known(ext).is_some() {
            return stem;
        }
    }
    name
}

fn match_probe_version<'a>(
    versions: &'a [remuxdb::MediaInfo],
    stream: &db::Media,
) -> Option<&'a remuxdb::MediaInfo> {
    let si = stream
        .stream_info
        .as_ref()?;

    // Torrent: match by info_hash + file_idx. Covers both raw Torrent descriptors and
    // debrid Http streams where the hash comes from AIOStreams streamData.
    let (hash, hash_file_idx) = match si.descriptor {
        StreamDescriptor::Torrent {
            ref info_hash,
            file_idx,
            ..
        } => (Some(info_hash.as_str()), file_idx.map(|i| i as i32)),
        _ => (
            si.torrent_info_hash
                .as_deref(),
            si.torrent_file_idx,
        ),
    };
    if let Some(hash) = hash {
        if let Some(v) = versions
            .iter()
            .find(|v| {
                v.sources
                    .iter()
                    .any(|s| {
                        s.torrent_info_hash
                            .as_deref()
                            == Some(hash)
                            && s.torrent_file_idx == hash_file_idx
                    })
            })
        {
            return Some(v);
        }
    }

    // Usenet: match by indexer_guid (+ indexer name when available) against version sources
    if let Some(ref guid) = si.usenet_guid {
        if let Some(v) = versions
            .iter()
            .find(|v| {
                v.sources
                    .iter()
                    .any(|s| {
                        s.indexer_guid
                            .as_deref()
                            == Some(guid.as_str())
                            && (si
                                .usenet_indexer
                                .is_none()
                                || s.indexer == si.usenet_indexer)
                    })
            })
        {
            return Some(v);
        }
    }

    // HTTP: match by exact file size
    if let Some(size) = si.size {
        if let Some(v) = versions
            .iter()
            .find(|v| v.size == Some(size))
        {
            return Some(v);
        }
    }

    // Fallback: match by filename (with and without video extension) against version sources
    let filename = si
        .filename
        .as_deref()?;
    let stem = strip_video_ext(filename);
    versions
        .iter()
        .find(|v| {
            v.sources
                .iter()
                .any(|s| {
                    s.filename
                        .as_deref()
                        .map(|sf| sf == filename || strip_video_ext(sf) == stem)
                        .unwrap_or(false)
                })
        })
}

impl AddonService {
    #[tracing::instrument(skip_all, fields(title = %media.title, kind = %media.kind))]
    pub async fn refresh_streams(
        &self,
        media: &mut db::Media,
        ctx: &AppContext,
        user_id: Option<Uuid>,
    ) -> Result<()> {
        const STREAMS_TTL_SECS: i64 = 60;
        static STREAM_LOCKS: KeyedLock<Uuid> = KeyedLock::new();

        // Fast path: TTL not expired — skip the lock entirely.
        let is_fresh = |refreshed: Option<chrono::NaiveDateTime>| {
            refreshed.is_some_and(|r| {
                (chrono::Utc::now().naive_utc() - r).num_seconds() < STREAMS_TTL_SECS
            })
        };
        if is_fresh(media.streams_refreshed_at) {
            return Ok(());
        }

        // Acquire per-media lock to prevent concurrent refreshes.
        let _guard = STREAM_LOCKS
            .lock(media.id)
            .await;

        // Re-check after acquiring lock — another task may have just refreshed.
        let refreshed_at = sqlx::query_scalar::<_, Option<chrono::NaiveDateTime>>(
            "SELECT streams_refreshed_at FROM media WHERE id = ?",
        )
        .bind(media.id)
        .fetch_optional(&ctx.db)
        .await
        .ok()
        .flatten()
        .flatten();
        if is_fresh(refreshed_at) {
            media.streams_refreshed_at = refreshed_at;
            return Ok(());
        }

        media
            .grandparent(&ctx.db)
            .await
            .ok();

        let instant = Instant::now();
        let probe_versions_fut = async {
            let Some(url) = ctx
                .config
                .remuxdb_url
                .clone()
            else {
                return None;
            };
            let imdb_id = if media.kind == db::MediaKind::Episode {
                media
                    .grandparent
                    .as_deref()
                    .and_then(|gp| {
                        gp.external_ids
                            .imdb
                            .as_deref()
                    })
                    .or(media
                        .external_ids
                        .imdb
                        .as_deref())
            } else {
                media
                    .external_ids
                    .imdb
                    .as_deref()
            };
            let Some(imdb_id) = imdb_id else {
                return None;
            };
            let cfg = db::Settings::get_config_or_default(&ctx.db).await;
            if !cfg
                .remuxdb_enabled
                .unwrap_or(true)
            {
                return None;
            }
            let (season, episode) = if media.kind == db::MediaKind::Episode {
                (
                    media
                        .parent_idx
                        .map(|v| v as i32),
                    media
                        .idx
                        .map(|v| v as i32),
                )
            } else {
                (None, None)
            };
            remuxdb::fetch_probe(
                &url,
                cfg.remuxdb_token
                    .as_deref(),
                Some(crate::common::server_id().as_str()),
                imdb_id,
                season,
                episode,
            )
            .await
        };
        let probe_t = std::time::Instant::now();
        let (raw, probe_versions) = tokio::join!(
            self.get_streams(media, ctx, user_id),
            tokio::time::timeout(std::time::Duration::from_secs(5), probe_versions_fut,)
        );
        let raw = raw?;
        let probe_versions = match probe_versions {
            Ok(v) => v,
            Err(_) => {
                debug!(remuxdb_elapsed = ?probe_t.elapsed(), "remuxdb probe timed out");
                None
            }
        };
        debug!(
            raw_count = raw.len(),
            remuxdb_elapsed = ?probe_t.elapsed(),
            "raw streams fetched"
        );

        // Dedup by descriptor content; order preserves addon priority (DB load order).
        // First occurrence wins, so higher-priority addons' streams survive.
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<db::Media> = raw
            .into_iter()
            .filter(|s| match Self::stream_dedup_key(s) {
                Some(key) => seen.insert(key),
                None => true,
            })
            .collect();

        let sources: Vec<&str> = {
            let mut seen = std::collections::HashSet::new();
            deduped
                .iter()
                .filter_map(|m| {
                    m.stream_info
                        .as_ref()?
                        .source
                        .as_deref()
                })
                .filter(|s| seen.insert(*s))
                .collect()
        };
        info!(streams = deduped.len(), ?sources, elapsed = ?instant.elapsed(), "streams synced");
        if deduped.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().naive_utc();
        sqlx::query("UPDATE media SET streams_refreshed_at = ? WHERE id = ?")
            .bind(now)
            .bind(media.id)
            .execute(&ctx.db)
            .await?;
        media.streams_refreshed_at = Some(now);
        let mut sources: Vec<db::Media> = deduped
            .into_iter()
            .enumerate()
            .map(|(idx, mut s)| {
                // Stable ID derived from content so the same stream always maps to
                // the same UUID across refreshes, enabling safe upsert semantics.
                let id_key = Self::stream_dedup_key(&s)
                    .unwrap_or_else(|| format!("source_{idx}"));
                s.id = Uuid::new_v5(&media.id, id_key.as_bytes());
                s.parent_id = Some(media.id);
                s.runtime = media.runtime;
                s.idx = Some(idx as i64);
                s.created_at = now;
                s.updated_at = now;
                s
            })
            .collect();

        if let Some(ref versions) = probe_versions {
            for source in &mut sources {
                if source
                    .probe_data
                    .is_some()
                {
                    continue;
                }
                if let Some(version) = match_probe_version(versions, source) {
                    source.probe_data = Some(version.into());
                    debug!(id = %source.id, "remuxdb: probe data applied from version");
                }
            }
        }

        db::Media::upsert(&ctx.db, &sources).await?;

        // delete stale items
        sqlx::query(
            "DELETE FROM media WHERE kind = 'stream' AND parent_id = ? AND updated_at < datetime('now', '-1 days')",
        )
        .bind(media.id)
        .execute(&ctx.db)
        .await?;
        Ok(())
    }

    pub async fn fetch_segments(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        background: bool,
    ) -> MediaSegments {
        let addons: Vec<(String, Arc<dyn SegmentAddon>)> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.row
                    .resources
                    .contains(&ResourceType::Segment)
            })
            .filter_map(|r| {
                r.segment
                    .as_ref()
                    .and_then(|s| {
                        let supports = s.supports(media);
                        debug!(
                            addon = %r.row.name,
                            media_kind = ?media.kind,
                            supports,
                            "segment addon filter"
                        );
                        if supports {
                            Some((
                                r.row
                                    .name
                                    .clone(),
                                s.clone(),
                            ))
                        } else {
                            None
                        }
                    })
            })
            .collect();

        let addon_count = addons.len();
        let instant = Instant::now();
        let mut merged = MediaSegments::default();
        for (name, addon) in addons {
            match addon
                .segment_fetch(media, ctx)
                .await
            {
                Ok(segs) if !segs.is_empty() => merged.merge_from(segs),
                Ok(_) => {}
                Err(e) => {
                    error!(addon = %name, item = %media.id, error = %e, "segment addon failed")
                }
            }
        }
        let found = [
            &merged.intro,
            &merged.outro,
            &merged.recap,
            &merged.preview,
            &merged.commercial,
        ]
        .iter()
        .filter(|s| s.is_some())
        .count();
        if background {
            debug!(segments = found, addons = addon_count, elapsed = ?instant.elapsed(), "segments fetched");
        } else {
            info!(segments = found, addons = addon_count, elapsed = ?instant.elapsed(), "segments fetched");
        }
        merged
    }

    pub async fn lyric_fetch(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Option<LyricDto>> {
        let addons: Vec<(String, Arc<dyn LyricAddon>)> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.row
                    .resources
                    .contains(&ResourceType::Lyrics)
            })
            .filter_map(|r| {
                r.lyric
                    .as_ref()
                    .map(|l| {
                        (
                            r.row
                                .name
                                .clone(),
                            l.clone(),
                        )
                    })
            })
            .collect();

        for (name, addon) in addons {
            match addon
                .lyric_fetch(req)
                .await
            {
                Ok(Some(l)) => return Ok(Some(l)),
                Ok(None) => continue,
                Err(e) => {
                    warn!(addon = %name, error = %e, "lyric addon fetch failed")
                }
            }
        }
        Ok(None)
    }

    pub async fn lyric_search(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>> {
        let addons: Vec<(String, Arc<dyn LyricAddon>)> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.row
                    .resources
                    .contains(&ResourceType::Lyrics)
            })
            .filter_map(|r| {
                r.lyric
                    .as_ref()
                    .map(|l| {
                        (
                            r.row
                                .name
                                .clone(),
                            l.clone(),
                        )
                    })
            })
            .collect();

        let mut out = Vec::new();
        for (name, addon) in addons {
            match addon
                .lyric_search(req)
                .await
            {
                Ok(items) => out.extend(items),
                Err(e) => {
                    warn!(addon = %name, error = %e, "lyric addon search failed")
                }
            }
        }
        Ok(out)
    }

    pub async fn lyric_get_by_composite_id(
        &self,
        composite_id: &str,
    ) -> Result<Option<LyricDto>> {
        let addons: Vec<Arc<dyn LyricAddon>> = self
            .inner
            .load()
            .iter()
            .filter_map(|r| {
                r.lyric
                    .clone()
            })
            .collect();

        for addon in addons {
            let prefix = format!("{}_", addon.provider_id());
            if let Some(inner) = composite_id.strip_prefix(&prefix) {
                return addon
                    .lyric_get_by_id(inner)
                    .await;
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// AddonCatalogStream
// ---------------------------------------------------------------------------

struct AddonCatalogStream {
    addon: Arc<dyn CatalogAddon>,
    local_id: String,
}

#[async_trait]
impl RemoteMediaStream for AddonCatalogStream {
    async fn stream(
        &self,
        ctx: &AppContext,
    ) -> Result<Pin<Box<dyn Stream<Item = db::Media> + Send>>> {
        self.addon
            .catalog_stream(ctx, &self.local_id)
            .await?
            .ok_or_else(|| {
                anyhow!("catalog addon does not serve catalog '{}'", self.local_id)
            })
    }
}

pub fn make_media_id(addon_id: Uuid, local_id: &str) -> String {
    format!("addon:{addon_id}:{local_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent_stream(hash: &str, file_hint: &str, file_idx: usize) -> db::Media {
        db::Media {
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Torrent {
                    info_hash: hash.to_string(),
                    file_hint: Some(file_hint.to_string()),
                    file_idx: Some(file_idx),
                    trackers: Vec::new(),
                },
                filename: Some(file_hint.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn torrent_dedup_preserves_distinct_files_in_a_bundle() {
        let first = torrent_stream("abc", "Bundle/Movie.One.mkv", 0);
        let duplicate = torrent_stream("ABC", "Bundle/Movie.One.mkv", 7);
        let second = torrent_stream("abc", "Bundle/Movie.Two.mkv", 1);

        assert_eq!(
            AddonService::stream_dedup_key(&first),
            AddonService::stream_dedup_key(&duplicate)
        );
        assert_ne!(
            AddonService::stream_dedup_key(&first),
            AddonService::stream_dedup_key(&second)
        );
    }

    fn make_image(path: &str) -> db::MediaImage {
        db::MediaImage {
            id: uuid::Uuid::new_v4(),
            media_id: uuid::Uuid::nil(),
            image_type: db::ImageKind::Primary.to_string(),
            image_index: 0,
            path: path.into(),
            width: None,
            height: None,
        }
    }

    // Simulates the refresh_meta accumulation: patch multiple addon results
    // into a fresh db::Media with replace=false, then apply once to `media`.
    fn accumulate(patches: Vec<db::Media>, force_refresh: bool) -> db::Media {
        let mut combined: Option<db::Media> = None;
        for patch in patches {
            let acc = combined.get_or_insert_with(db::Media::default);
            apply_meta(acc, patch, false);
        }
        let mut media = db::Media::default();
        if let Some(c) = combined {
            apply_meta(&mut media, c, force_refresh);
        }
        media
    }

    #[test]
    fn highest_priority_addon_wins_description() {
        let high = db::Media {
            description: Some("from high priority".into()),
            ..Default::default()
        };
        let low = db::Media {
            description: Some("from low priority".into()),
            ..Default::default()
        };
        // high priority addon is first (lower priority number, ORDER BY priority ASC)
        let result = accumulate(vec![high, low], true);
        assert_eq!(
            result
                .description
                .as_deref(),
            Some("from high priority")
        );
    }

    #[test]
    fn lower_priority_addon_fills_gaps_left_by_higher() {
        let high = db::Media {
            description: None, // high priority addon has no description
            ..Default::default()
        };
        let low = db::Media {
            description: Some("fallback".into()),
            ..Default::default()
        };
        let result = accumulate(vec![high, low], true);
        assert_eq!(
            result
                .description
                .as_deref(),
            Some("fallback")
        );
    }

    #[test]
    fn highest_priority_addon_wins_primary_image() {
        let mut high = db::Media::default();
        high.images
            .primary = vec![make_image("https://high.example/poster.jpg")];

        let mut low = db::Media::default();
        low.images
            .primary = vec![make_image("https://low.example/poster.jpg")];

        let result = accumulate(vec![high, low], true);
        assert_eq!(
            result
                .images
                .primary[0]
                .path,
            "https://high.example/poster.jpg"
        );
    }

    #[test]
    fn lower_priority_fills_missing_image_type() {
        let mut high = db::Media::default();
        high.images
            .primary = vec![make_image("https://high.example/poster.jpg")];
        // high priority has no backdrop

        let mut low = db::Media::default();
        low.images
            .backdrop = vec![make_image("https://low.example/backdrop.jpg")];

        let result = accumulate(vec![high, low], true);
        assert_eq!(
            result
                .images
                .primary[0]
                .path,
            "https://high.example/poster.jpg"
        );
        assert_eq!(
            result
                .images
                .backdrop[0]
                .path,
            "https://low.example/backdrop.jpg"
        );
    }

    #[test]
    fn force_refresh_replaces_existing_media_values() {
        let patch = db::Media {
            description: Some("new description".into()),
            ..Default::default()
        };
        let mut media = db::Media {
            description: Some("old description".into()),
            ..Default::default()
        };
        apply_meta(&mut media, patch, true);
        assert_eq!(
            media
                .description
                .as_deref(),
            Some("new description")
        );
    }

    #[test]
    fn no_force_refresh_preserves_existing_media_values() {
        let patch = db::Media {
            description: Some("new description".into()),
            ..Default::default()
        };
        let mut media = db::Media {
            description: Some("old description".into()),
            ..Default::default()
        };
        apply_meta(&mut media, patch, false);
        assert_eq!(
            media
                .description
                .as_deref(),
            Some("old description")
        );
    }

    fn days_ago(n: i64) -> chrono::NaiveDateTime {
        (chrono::Utc::now() - chrono::Duration::days(n)).naive_utc()
    }

    fn days_from_now(n: i64) -> chrono::NaiveDateTime {
        (chrono::Utc::now() + chrono::Duration::days(n)).naive_utc()
    }

    fn child_with(
        refreshed_at: Option<chrono::NaiveDateTime>,
        digital_released_at: Option<chrono::NaiveDateTime>,
    ) -> db::Media {
        db::Media {
            refreshed_at,
            digital_released_at,
            ..Default::default()
        }
    }

    // --- child_refresh_force ---

    #[test]
    fn active_window_episode_returns_some_true() {
        let child = child_with(Some(days_ago(1)), None);
        assert_eq!(child_refresh_force(false, true, &child), Some(true));
    }

    #[test]
    fn active_window_episode_no_refreshed_at_returns_some_true() {
        let child = child_with(None, None);
        assert_eq!(child_refresh_force(false, true, &child), Some(true));
    }

    #[test]
    fn inactive_episode_with_refreshed_at_returns_none() {
        let child = child_with(Some(days_ago(1)), None);
        assert_eq!(child_refresh_force(false, false, &child), None);
    }

    #[test]
    fn inactive_episode_no_refreshed_at_returns_some_false() {
        let child = child_with(None, None);
        assert_eq!(child_refresh_force(false, false, &child), Some(false));
    }

    #[test]
    fn force_refresh_overrides_everything() {
        let child = child_with(Some(days_ago(1)), Some(days_ago(300)));
        assert_eq!(child_refresh_force(true, false, &child), Some(true));
    }

    // --- episode_in_active_window ---

    #[test]
    fn no_released_at_is_active() {
        let child = child_with(None, None);
        assert!(episode_in_active_window(&child));
    }

    #[test]
    fn future_released_at_is_active() {
        let child = child_with(None, Some(days_from_now(30)));
        assert!(episode_in_active_window(&child));
    }

    #[test]
    fn recent_released_at_is_active() {
        let child = child_with(None, Some(days_ago(30)));
        assert!(episode_in_active_window(&child));
    }

    #[test]
    fn old_released_at_is_inactive() {
        let child = child_with(None, Some(days_ago(200)));
        assert!(!episode_in_active_window(&child));
    }

    // --- series_is_active ---

    #[test]
    fn series_active_when_status_none() {
        assert!(series_is_active(&None));
    }

    #[test]
    fn series_active_when_continuing() {
        assert!(series_is_active(&Some(db::MediaStatus::Continuing)));
    }

    #[test]
    fn series_inactive_when_ended() {
        assert!(!series_is_active(&Some(db::MediaStatus::Ended)));
    }

    #[test]
    fn series_inactive_when_unreleased() {
        assert!(!series_is_active(&Some(db::MediaStatus::Unreleased)));
    }

    // --- apply_title_format idempotency ---

    #[test]
    fn apply_title_format_strips_episode_prefix() {
        let mut media = db::Media {
            kind: db::MediaKind::Episode,
            title: "S3E4 - Tumbleton".into(),
            idx: Some(4),
            parent_idx: Some(3),
            ..Default::default()
        };
        apply_title_format(&mut media);
        assert_eq!(media.title, "Tumbleton");
    }

    #[test]
    fn apply_title_format_keeps_clean_episode_title() {
        let mut media = db::Media {
            kind: db::MediaKind::Episode,
            title: "Tumbleton".into(),
            idx: Some(4),
            parent_idx: Some(3),
            ..Default::default()
        };
        apply_title_format(&mut media);
        assert_eq!(media.title, "Tumbleton");
    }

    #[test]
    fn merge_media_respects_locked_name() {
        let mut target = db::Media {
            title: "User Title".to_string(),
            locked_fields: vec![db::MetadataField::Name],
            ..Default::default()
        };
        let source = db::Media {
            title: "Provider Title".to_string(),
            ..Default::default()
        };
        merge_media(&mut target, &source, true);
        assert_eq!(
            target.title, "User Title",
            "locked Name must not be overwritten"
        );
    }

    #[test]
    fn merge_media_respects_is_locked() {
        let mut target = db::Media {
            title: "User Title".to_string(),
            description: Some("User Overview".to_string()),
            is_locked: true,
            ..Default::default()
        };
        let source = db::Media {
            title: "Provider Title".to_string(),
            description: Some("Provider Overview".to_string()),
            ..Default::default()
        };
        merge_media(&mut target, &source, true);
        assert_eq!(target.title, "User Title");
        assert_eq!(
            target
                .description
                .as_deref(),
            Some("User Overview")
        );
    }

    #[test]
    fn merge_media_unlocked_fields_still_update() {
        let mut target = db::Media {
            locked_fields: vec![db::MetadataField::Name],
            ..Default::default()
        };
        let source = db::Media {
            description: Some("Provider Overview".to_string()),
            ..Default::default()
        };
        merge_media(&mut target, &source, true);
        assert_eq!(
            target
                .description
                .as_deref(),
            Some("Provider Overview"),
            "unlocked Overview must still be updated"
        );
    }

    #[tokio::test]
    async fn save_pending_relations_preserves_locked_genres() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let item = db::Media {
            id: Uuid::new_v4(),
            title: "Locked Genres Series".to_string(),
            kind: db::MediaKind::Series,
            locked_fields: vec![db::MetadataField::Genres],
            ..Default::default()
        };
        let genre = db::Media {
            id: Uuid::new_v4(),
            title: "Acoustic".to_string(),
            kind: db::MediaKind::Genre,
            ..Default::default()
        };
        let actor = db::Media {
            id: Uuid::new_v4(),
            title: "Actor".to_string(),
            kind: db::MediaKind::Person,
            external_ids: db::ExternalIds {
                tmdb: Some(42),
                ..Default::default()
            },
            ..Default::default()
        };
        let item_id = item.id;
        let genre_id = genre.id;
        db::Media::upsert(&ctx.db, &[item.clone(), genre.clone(), actor.clone()])
            .await
            .unwrap();
        db::MediaRelation::upsert(
            &ctx.db,
            &[db::MediaRelation {
                left_media_id: item.id,
                right_media_id: genre.id,
                ..Default::default()
            }],
        )
        .await
        .unwrap();

        let refreshed = db::Media {
            relations: Some(vec![(
                db::MediaRelation {
                    left_media_id: item.id,
                    right_media_id: actor.id,
                    role: Some(db::RelationRole::Actor),
                    ..Default::default()
                },
                actor,
            )]),
            ..item
        };
        save_pending_relations(ctx, &[refreshed]).await;

        let relations = db::MediaRelation::get_by_left_ids(&ctx.db, &[item_id])
            .await
            .unwrap();
        assert!(
            relations
                .iter()
                .any(|relation| relation.right_media_id == genre_id)
        );
    }

    #[test]
    fn recognized_manifest_media_kind_drops_unrecognized_custom_type() {
        assert_eq!(
            recognized_manifest_media_kind(sdks::stremio::MediaType::Other(
                "anime".to_string()
            )),
            None
        );
        assert_eq!(
            recognized_manifest_media_kind(sdks::stremio::MediaType::Series),
            Some(sdks::remux::MediaKind::Series)
        );
        assert_eq!(
            recognized_manifest_media_kind(sdks::stremio::MediaType::Other(
                "episode".to_string()
            )),
            Some(sdks::remux::MediaKind::Episode)
        );
    }

    // --- get_direct_children resource guard ---

    struct SpyTree {
        called: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl TreeAddon for SpyTree {
        fn supports(&self, root: &db::Media) -> bool {
            matches!(root.kind, db::MediaKind::Series)
        }

        async fn get_children(
            &self,
            _root: &db::Media,
            _ctx: &AppContext,
        ) -> anyhow::Result<Option<Vec<db::Media>>> {
            self.called
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(vec![]))
        }
    }

    fn stream_only_runtime_with_tree(
        called: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> AddonRuntime {
        let now = chrono::Utc::now().naive_utc();
        AddonRuntime {
            row: addon::Addon {
                id: uuid::Uuid::new_v4(),
                name: "stream-only".into(),
                preset: AddonPresetRef {
                    kind: "scripted".into(),
                    config: serde_json::Value::Null.into(),
                },
                resources: vec![ResourceType::Stream], // no Meta
                types: vec![],
                enabled: true,
                priority: 0,
                created_at: now,
                updated_at: now,
                system: false,
                is_default: false,
                http_redirect_stream: false,
                service_filter: vec![],
            },
            caps: AddonCapabilities {
                tree: Some(std::sync::Arc::new(SpyTree { called })),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn get_direct_children_skips_addons_without_meta_resource() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = stream_only_runtime_with_tree(called.clone());

        let service = AddonService {
            inner: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(vec![runtime])),
        };

        let series = db::Media {
            kind: db::MediaKind::Series,
            ..Default::default()
        };

        // AppContext is required by the trait signature but is never reached when the
        // addon is filtered out before get_children is called. Use new_test_server so
        // port binding is handled correctly (same as all other integration tests).
        let (_, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = guard
            .0
            .clone();

        let children = service
            .get_direct_children(&series, &ctx)
            .await;

        assert!(
            children.is_empty(),
            "stream-only addon should not contribute children"
        );
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "stream-only addon's get_children should never be called"
        );
    }
}
