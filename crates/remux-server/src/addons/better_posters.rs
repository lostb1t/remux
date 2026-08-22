use anyhow::Result;
use async_trait::async_trait;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, AddonSelectOption, MediaKind, MetaAddon,
    PrimaryPosterOverride, ResourceType,
};
use crate::{AppContext, api, db};

const BASE_URL: &str = "https://btttr.cc";

static BETTER_POSTERS_CONFIG_REVISION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn config_revision() -> u64 {
    BETTER_POSTERS_CONFIG_REVISION.load(Ordering::Relaxed)
}

fn publish_config_revision(cfg: &serde_json::Value) {
    let mut hasher = DefaultHasher::new();
    cfg.to_string().hash(&mut hasher);
    BETTER_POSTERS_CONFIG_REVISION.store(hasher.finish(), Ordering::Relaxed);
}

pub struct BetterPostersPreset;

impl AddonPreset for BetterPostersPreset {
    fn id(&self) -> &'static str {
        "better-posters"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "better-posters".to_string(),
            display_name: "BetterPosters".to_string(),
            description: "Use btttr.cc as the effective primary poster provider for movies and series.".to_string(),
            icon: None,
            supported_resources: vec![AddonMetadata::simple_resource(ResourceType::Meta)],
            supported_types: vec![MediaKind::Movie, MediaKind::Series],
            supported_resources_user: vec![],
            supported_types_user: vec![],
            options: vec![
                bool_option(
                    "trend_tags",
                    "Trend Tags",
                    "Trending, New, IMDb ranking and similar BetterPosters trend tags.",
                    false,
                ),
                bool_option(
                    "quality_tags",
                    "Quality Tags",
                    "Show quality badges such as 4K, Dolby Vision and Atmos.",
                    false,
                ),
                bool_option(
                    "genre",
                    "Genre",
                    "Show the genre label at the bottom of the poster.",
                    false,
                ),
                bool_option(
                    "rating",
                    "Rating",
                    "Show the rating at the bottom of the poster.",
                    false,
                ),
                select_option(
                    "rating_source",
                    "Rating Source",
                    "Rating source used when Rating is enabled.",
                    "average",
                    &[
                        ("Average", "average"),
                        ("IMDb", "imdb"),
                        ("TMDB", "tmdb"),
                        ("Rotten Tomatoes", "rotten_tomatoes"),
                        ("Metacritic", "metacritic"),
                        ("Trakt", "trakt"),
                        ("Letterboxd", "letterboxd"),
                        ("Roger Ebert", "roger_ebert"),
                    ],
                ),
                bool_option(
                    "age_rating",
                    "Age Rating",
                    "Show age/certification badges such as PG-13, TV-MA and R.",
                    false,
                ),
                bool_option(
                    "watch_progress",
                    "Watch Progress",
                    "Show watched/episode progress using the authenticated Jellyfin user's local Remux watch state. No Trakt or PublicMetaDB account is required.",
                    false,
                ),
                select_option(
                    "language",
                    "Language",
                    "Language used by BetterPosters for poster labels.",
                    "en",
                    &[
                        ("English", "en"),
                        ("Spanish", "es"),
                        ("French", "fr"),
                        ("German", "de"),
                        ("Portuguese (Brazil)", "pt-BR"),
                        ("Portuguese (Portugal)", "pt-PT"),
                        ("Italian", "it"),
                        ("Dutch", "nl"),
                        ("Polish", "pl"),
                        ("Russian", "ru"),
                        ("Turkish", "tr"),
                        ("Arabic", "ar"),
                        ("Japanese", "ja"),
                        ("Korean", "ko"),
                        ("Chinese", "zh"),
                        ("Hindi", "hi"),
                        ("Swedish", "sv"),
                        ("Czech", "cs"),
                    ],
                ),
            ],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        publish_config_revision(cfg);
        let addon = Arc::new(BetterPostersAddon {
            trend_tags: cfg_bool(cfg, "trend_tags", false),
            quality_tags: cfg_bool(cfg, "quality_tags", false),
            genre: cfg_bool(cfg, "genre", false),
            rating: cfg_bool(cfg, "rating", false),
            rating_source: cfg_string(cfg, "rating_source", "average"),
            age_rating: cfg_bool(cfg, "age_rating", false),
            watch_progress: cfg_bool(cfg, "watch_progress", false),
            language: cfg_string(cfg, "language", "en"),
        });

        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            meta: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(BetterPostersPreset))
}

pub struct BetterPostersAddon {
    trend_tags: bool,
    quality_tags: bool,
    genre: bool,
    rating: bool,
    rating_source: String,
    age_rating: bool,
    watch_progress: bool,
    language: String,
}

#[async_trait]
impl AddonKind for BetterPostersAddon {
    fn id(&self) -> &'static str {
        "better-posters"
    }
}

#[async_trait]
impl MetaAddon for BetterPostersAddon {
    async fn supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series)
    }

    async fn meta_fetch(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
        _config: &api::ServerConfiguration,
    ) -> Result<Option<db::Media>> {
        Ok(None)
    }

    async fn images_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<api::RemoteImageInfo>> {
        let Some(imdb_id) = self
            .resolve_imdb_id(media, ctx)
            .await?
        else {
            return Ok(vec![]);
        };
        let url = self.standard_url(&imdb_id);
        Ok(vec![api::RemoteImageInfo {
            provider_name: Some("BetterPosters".to_string()),
            url: Some(url.clone()),
            thumbnail_url: Some(url),
            type_: Some("Primary".to_string()),
            width: None,
            height: None,
        }])
    }

    async fn primary_poster_override(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        user: Option<&db::User>,
    ) -> Result<Option<PrimaryPosterOverride>> {
        let Some(imdb_id) = self
            .resolve_imdb_id(media, ctx)
            .await?
        else {
            return Ok(None);
        };

        let standard_url = self.standard_url(&imdb_id);
        if !self.watch_progress {
            return Ok(Some(PrimaryPosterOverride {
                url: standard_url,
                fallback_url: None,
                private_cache: false,
            }));
        }

        let Some(user) = user else {
            return Ok(Some(PrimaryPosterOverride {
                url: standard_url,
                fallback_url: None,
                private_cache: false,
            }));
        };

        let filename = self
            .progress_filename(media, ctx, user)
            .await?;
        if filename == "poster" {
            return Ok(Some(PrimaryPosterOverride {
                url: standard_url,
                fallback_url: None,
                private_cache: true,
            }));
        }

        Ok(Some(PrimaryPosterOverride {
            url: self.progress_url(media, &imdb_id, &filename),
            fallback_url: Some(standard_url),
            private_cache: true,
        }))
    }
}

impl BetterPostersAddon {
    async fn resolve_imdb_id(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<String>> {
        if let Some(imdb) = media
            .external_ids
            .imdb
            .as_ref()
        {
            return Ok(Some(
                imdb.as_str()
                    .to_string(),
            ));
        }

        let client = super::tmdb::tmdb_client_from_ctx(ctx).await?;
        Ok(super::tmdb::resolve_imdb_from_ids(
            &media.external_ids,
            media.kind == db::MediaKind::Series,
            &client,
        )
        .await
        .map(|id| {
            id.as_str()
                .to_string()
        }))
    }

    async fn progress_filename(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        user: &db::User,
    ) -> Result<String> {
        match media.kind {
            db::MediaKind::Movie => {
                let played =
                    db::UserMediaState::get_by_user_and_media(&ctx.db, user, media)
                        .await?
                        .is_some_and(|s| s.play_count > 0);
                Ok(if played { "auto~w" } else { "poster" }.to_string())
            }
            db::MediaKind::Series => {
                let (total, watched): (i64, i64) = sqlx::query_as(
                    r#"
                    SELECT COUNT(*) AS total,
                           COALESCE(SUM(CASE WHEN ums.play_count > 0 THEN 1 ELSE 0 END), 0) AS watched
                    FROM media e
                    LEFT JOIN user_media_state ums
                      ON ums.media_id = e.id AND ums.user_id = ?1
                    WHERE e.kind = 'episode' AND e.grandparent_id = ?2
                    "#,
                )
                .bind(user.id)
                .bind(media.id)
                .fetch_one(&ctx.db)
                .await?;

                Ok(match (watched, total) {
                    (_, 0) | (0, _) => "poster".to_string(),
                    (w, t) if w >= t => "auto~w".to_string(),
                    (w, t) => format!("auto~s{w}o{t}"),
                })
            }
            _ => Ok("poster".to_string()),
        }
    }

    fn standard_url(&self, imdb_id: &str) -> String {
        let mut url = format!(
            "{BASE_URL}/{}/imdb/poster-default/{imdb_id}.jpg",
            self.standard_path()
        );
        self.append_query(&mut url);
        url
    }

    fn progress_url(&self, media: &db::Media, imdb_id: &str, filename: &str) -> String {
        let media_type = match media.kind {
            db::MediaKind::Series => "series",
            _ => "movie",
        };
        let mut url = format!(
            "{BASE_URL}/{}/{media_type}/{imdb_id}/{filename}.jpg",
            self.dynamic_overlay_path()
        );
        self.append_query(&mut url);
        url
    }

    fn standard_path(&self) -> String {
        let mut suffix = match (self.genre, self.rating) {
            (true, true) => String::new(),
            (false, true) => "r".to_string(),
            (true, false) => "g".to_string(),
            (false, false) => "n".to_string(),
        };
        if self.quality_tags {
            suffix.push('q');
        }
        if self.age_rating {
            suffix.push('a');
        }
        if suffix.is_empty() {
            "poster".to_string()
        } else {
            format!("poster-{suffix}")
        }
    }

    fn dynamic_overlay_path(&self) -> String {
        let mut suffix = String::new();
        if self.genre {
            suffix.push('g');
        }
        if self.rating {
            suffix.push('r');
        }
        if self.quality_tags {
            suffix.push('q');
        }
        if self.age_rating {
            suffix.push('a');
        }
        if suffix.is_empty() {
            "poster".to_string()
        } else {
            format!("poster-{suffix}")
        }
    }

    fn append_query(&self, url: &mut String) {
        let mut params: Vec<String> = Vec::new();
        if !self.trend_tags {
            params.push("tag=none".to_string());
        }
        if self.language != "en"
            && !self
                .language
                .is_empty()
        {
            params.push(format!("lang={}", self.language));
        }
        if self.rating {
            if let Some(code) = rating_source_code(&self.rating_source) {
                params.push(format!("rs={code}"));
            }
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
    }
}

fn cfg_bool(cfg: &serde_json::Value, key: &str, default: bool) -> bool {
    cfg.get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

fn cfg_string(cfg: &serde_json::Value, key: &str, default: &str) -> String {
    cfg.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn bool_option(id: &str, name: &str, description: &str, default: bool) -> AddonOption {
    AddonOption {
        id: id.to_string(),
        name: name.to_string(),
        description: Some(description.to_string()),
        required: false,
        default: Some(serde_json::Value::Bool(default)),
        kind: AddonOptionType::Boolean,
    }
}

fn select_option(
    id: &str,
    name: &str,
    description: &str,
    default: &str,
    options: &[(&str, &str)],
) -> AddonOption {
    AddonOption {
        id: id.to_string(),
        name: name.to_string(),
        description: Some(description.to_string()),
        required: false,
        default: Some(serde_json::Value::String(default.to_string())),
        kind: AddonOptionType::Select {
            options: options
                .iter()
                .map(|(label, value)| AddonSelectOption {
                    label: (*label).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
        },
    }
}

fn rating_source_code(source: &str) -> Option<&'static str> {
    match source {
        "imdb" => Some("IM"),
        "tmdb" => Some("TM"),
        "rotten_tomatoes" => Some("RT"),
        "metacritic" => Some("MC"),
        "trakt" => Some("TR"),
        "letterboxd" => Some("LB"),
        "roger_ebert" => Some("RE"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addon() -> BetterPostersAddon {
        BetterPostersAddon {
            trend_tags: true,
            quality_tags: false,
            genre: true,
            rating: true,
            rating_source: "average".to_string(),
            age_rating: false,
            watch_progress: true,
            language: "en".to_string(),
        }
    }

    #[test]
    fn official_default_url_matches_current_builder() {
        assert_eq!(
            addon().standard_url("tt0133093"),
            "https://btttr.cc/poster/imdb/poster-default/tt0133093.jpg"
        );
    }

    #[test]
    fn official_query_options_are_encoded_in_expected_shape() {
        let mut a = addon();
        a.trend_tags = false;
        a.language = "de".to_string();
        a.rating_source = "letterboxd".to_string();
        assert_eq!(
            a.standard_url("tt0133093"),
            "https://btttr.cc/poster/imdb/poster-default/tt0133093.jpg?tag=none&lang=de&rs=LB"
        );
    }

    #[test]
    fn dynamic_progress_overlay_uses_local_progress_protocol() {
        let a = addon();
        let media = db::Media {
            kind: db::MediaKind::Series,
            ..Default::default()
        };
        assert_eq!(
            a.progress_url(&media, "tt0903747", "auto~s3o5"),
            "https://btttr.cc/poster-gr/series/tt0903747/auto~s3o5.jpg"
        );
    }

    #[test]
    fn path_variants_match_official_stable_builder() {
        let mut a = addon();
        a.genre = false;
        a.rating = false;
        a.quality_tags = true;
        a.age_rating = true;
        assert_eq!(a.standard_path(), "poster-nqa");
    }
}
