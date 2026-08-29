use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, AddonSelectOption, MediaKind, ResourceType,
};
use crate::{AppContext, db};

pub struct BetterPostersPreset;

impl AddonPreset for BetterPostersPreset {
    fn id(&self) -> &'static str {
        "betterposters"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "betterposters".to_string(),
            display_name: "BetterPosters".to_string(),
            description: "Poster images with stylized versions from btttr.cc. Supports genre banners, rating overlays, quality tags, age ratings, and trend tags.".to_string(),
            icon: None,
            supported_resources: vec![AddonMetadata::simple_resource(ResourceType::Meta)],
            supported_types: vec![
                MediaKind::Movie,
                MediaKind::Series,
            ],
            supported_resources_user: vec![ResourceType::Meta],
            supported_types_user: vec![
                MediaKind::Movie,
                MediaKind::Series,
            ],
            options: vec![
                AddonOption {
                    id: "genre".to_string(),
                    name: "Genre Banner".to_string(),
                    description: Some("Show a genre banner on the poster.".to_string()),
                    required: false,
                    default: Some(serde_json::Value::Bool(true)),
                    kind: AddonOptionType::Boolean,
                },
                AddonOption {
                    id: "rating".to_string(),
                    name: "Rating Overlay".to_string(),
                    description: Some("Show a rating overlay on the poster.".to_string()),
                    required: false,
                    default: Some(serde_json::Value::Bool(true)),
                    kind: AddonOptionType::Boolean,
                },
                AddonOption {
                    id: "quality".to_string(),
                    name: "Quality Tags".to_string(),
                    description: Some("Show quality tags (4K, HDR, etc.) on the poster.".to_string()),
                    required: false,
                    default: Some(serde_json::Value::Bool(false)),
                    kind: AddonOptionType::Boolean,
                },
                AddonOption {
                    id: "age_rating".to_string(),
                    name: "Age Rating".to_string(),
                    description: Some("Show the age/content rating on the poster.".to_string()),
                    required: false,
                    default: Some(serde_json::Value::Bool(false)),
                    kind: AddonOptionType::Boolean,
                },
                AddonOption {
                    id: "trend_tags".to_string(),
                    name: "Trend Tags".to_string(),
                    description: Some("Show trend tags (trending, new, etc.) on the poster.".to_string()),
                    required: false,
                    default: Some(serde_json::Value::Bool(true)),
                    kind: AddonOptionType::Boolean,
                },
                AddonOption {
                    id: "language".to_string(),
                    name: "Language".to_string(),
                    description: Some("Language for text overlays on the poster.".to_string()),
                    required: false,
                    default: None,
                    kind: AddonOptionType::Select {
                        options: language_options(),
                    },
                },
                AddonOption {
                    id: "rating_source".to_string(),
                    name: "Rating Source".to_string(),
                    description: Some("Which rating source to display. Only applies when Rating Overlay is enabled.".to_string()),
                    required: false,
                    default: None,
                    kind: AddonOptionType::Select {
                        options: rating_source_options(),
                    },
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let addon = Arc::new(BetterPostersAddon {
            genre: cfg_bool(cfg, "genre", true),
            rating: cfg_bool(cfg, "rating", true),
            quality: cfg_bool(cfg, "quality", false),
            age_rating: cfg_bool(cfg, "age_rating", false),
            trend_tags: cfg_bool(cfg, "trend_tags", true),
            language: cfg_str(cfg, "language"),
            rating_source: cfg_str(cfg, "rating_source"),
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
    genre: bool,
    rating: bool,
    quality: bool,
    age_rating: bool,
    trend_tags: bool,
    language: Option<String>,
    rating_source: Option<String>,
}

fn cfg_bool(cfg: &serde_json::Value, key: &str, default: bool) -> bool {
    cfg.get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

fn cfg_str(cfg: &serde_json::Value, key: &str) -> Option<String> {
    cfg.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Compute the btttr.cc path segment from the four boolean flags.
///
/// Table (Genre × Rating × Quality × Age):
///   G R  → poster
///   G ¬R → poster-g
///   ¬G R → poster-r
///   ¬G ¬R→ poster-n
/// Quality ON appends 'q', Age ON appends 'a'.
fn build_path(genre: bool, rating: bool, quality: bool, age_rating: bool) -> String {
    let base = match (genre, rating) {
        (true, true) => "poster",
        (true, false) => "poster-g",
        (false, true) => "poster-r",
        (false, false) => "poster-n",
    };
    let mut path = base.to_string();
    if quality {
        path.push('q');
    }
    if age_rating {
        path.push('a');
    }
    path
}

fn build_url(imdb_id: &str, addon: &BetterPostersAddon) -> String {
    let path = build_path(addon.genre, addon.rating, addon.quality, addon.age_rating);
    let base = format!(
        "https://btttr.cc/{}/imdb/poster-default/{}.jpg",
        path, imdb_id
    );

    let mut params: Vec<String> = Vec::new();

    if !addon.trend_tags {
        params.push("tag=none".to_string());
    }
    if let Some(lang) = &addon.language {
        params.push(format!("lang={}", lang));
    }
    if addon.rating {
        if let Some(rs) = &addon.rating_source {
            params.push(format!("rs={}", rs));
        }
    }

    if params.is_empty() {
        base
    } else {
        format!("{}?{}", base, params.join("&"))
    }
}

fn language_options() -> Vec<AddonSelectOption> {
    vec![
        AddonSelectOption {
            label: "English".to_string(),
            value: "".to_string(),
        },
        AddonSelectOption {
            label: "Arabic".to_string(),
            value: "ar".to_string(),
        },
        AddonSelectOption {
            label: "Bulgarian".to_string(),
            value: "bg".to_string(),
        },
        AddonSelectOption {
            label: "Czech".to_string(),
            value: "cs".to_string(),
        },
        AddonSelectOption {
            label: "Danish".to_string(),
            value: "da".to_string(),
        },
        AddonSelectOption {
            label: "German".to_string(),
            value: "de".to_string(),
        },
        AddonSelectOption {
            label: "Greek".to_string(),
            value: "el".to_string(),
        },
        AddonSelectOption {
            label: "Spanish".to_string(),
            value: "es".to_string(),
        },
        AddonSelectOption {
            label: "Finnish".to_string(),
            value: "fi".to_string(),
        },
        AddonSelectOption {
            label: "French".to_string(),
            value: "fr".to_string(),
        },
        AddonSelectOption {
            label: "Hebrew".to_string(),
            value: "he".to_string(),
        },
        AddonSelectOption {
            label: "Hungarian".to_string(),
            value: "hu".to_string(),
        },
        AddonSelectOption {
            label: "Italian".to_string(),
            value: "it".to_string(),
        },
        AddonSelectOption {
            label: "Japanese".to_string(),
            value: "ja".to_string(),
        },
        AddonSelectOption {
            label: "Korean".to_string(),
            value: "ko".to_string(),
        },
        AddonSelectOption {
            label: "Dutch".to_string(),
            value: "nl".to_string(),
        },
        AddonSelectOption {
            label: "Norwegian".to_string(),
            value: "no".to_string(),
        },
        AddonSelectOption {
            label: "Polish".to_string(),
            value: "pl".to_string(),
        },
        AddonSelectOption {
            label: "Portuguese".to_string(),
            value: "pt".to_string(),
        },
        AddonSelectOption {
            label: "Romanian".to_string(),
            value: "ro".to_string(),
        },
        AddonSelectOption {
            label: "Russian".to_string(),
            value: "ru".to_string(),
        },
        AddonSelectOption {
            label: "Swedish".to_string(),
            value: "sv".to_string(),
        },
        AddonSelectOption {
            label: "Thai".to_string(),
            value: "th".to_string(),
        },
        AddonSelectOption {
            label: "Turkish".to_string(),
            value: "tr".to_string(),
        },
        AddonSelectOption {
            label: "Ukrainian".to_string(),
            value: "uk".to_string(),
        },
        AddonSelectOption {
            label: "Chinese (Simplified)".to_string(),
            value: "zh".to_string(),
        },
    ]
}

fn rating_source_options() -> Vec<AddonSelectOption> {
    vec![
        AddonSelectOption {
            label: "Average".to_string(),
            value: "".to_string(),
        },
        AddonSelectOption {
            label: "IMDb".to_string(),
            value: "IM".to_string(),
        },
        AddonSelectOption {
            label: "TMDB".to_string(),
            value: "TM".to_string(),
        },
        AddonSelectOption {
            label: "Rotten Tomatoes".to_string(),
            value: "RT".to_string(),
        },
        AddonSelectOption {
            label: "Metacritic".to_string(),
            value: "MC".to_string(),
        },
        AddonSelectOption {
            label: "Trakt".to_string(),
            value: "TR".to_string(),
        },
        AddonSelectOption {
            label: "Letterboxd".to_string(),
            value: "LB".to_string(),
        },
        AddonSelectOption {
            label: "Roger Ebert".to_string(),
            value: "RE".to_string(),
        },
    ]
}

#[async_trait]
impl AddonKind for BetterPostersAddon {
    fn id(&self) -> &'static str {
        "betterposters"
    }
}

#[async_trait]
impl super::MetaAddon for BetterPostersAddon {
    async fn supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series)
            && media
                .external_ids
                .imdb
                .is_some()
    }

    async fn meta_fetch(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
        _config: &crate::api::ServerConfiguration,
    ) -> Result<Option<db::Media>> {
        let Some(ref imdb_id) = media
            .external_ids
            .imdb
        else {
            return Ok(None);
        };
        let url = build_url(imdb_id, self);
        let mut patch = db::Media {
            id: media.id,
            kind: media
                .kind
                .clone(),
            ..Default::default()
        };
        patch.set_image(db::ImageKind::Primary, url);
        Ok(Some(patch))
    }

    async fn images_fetch(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_path_variants() {
        assert_eq!(build_path(true, true, false, false), "poster");
        assert_eq!(build_path(true, false, false, false), "poster-g");
        assert_eq!(build_path(false, true, false, false), "poster-r");
        assert_eq!(build_path(false, false, false, false), "poster-n");
        assert_eq!(build_path(true, true, true, false), "posterq");
        assert_eq!(build_path(true, true, false, true), "postera");
        assert_eq!(build_path(true, true, true, true), "posterqa");
        assert_eq!(build_path(false, false, true, true), "poster-nqa");
    }

    #[test]
    fn build_url_no_params() {
        let addon = BetterPostersAddon {
            genre: true,
            rating: true,
            quality: false,
            age_rating: false,
            trend_tags: true,
            language: None,
            rating_source: None,
        };
        assert_eq!(
            build_url("tt0111161", &addon),
            "https://btttr.cc/poster/imdb/poster-default/tt0111161.jpg"
        );
    }

    #[test]
    fn build_url_with_all_params() {
        let addon = BetterPostersAddon {
            genre: false,
            rating: true,
            quality: true,
            age_rating: false,
            trend_tags: false,
            language: Some("de".to_string()),
            rating_source: Some("IM".to_string()),
        };
        let url = build_url("tt0111161", &addon);
        assert_eq!(
            url,
            "https://btttr.cc/poster-rq/imdb/poster-default/tt0111161.jpg?tag=none&lang=de&rs=IM"
        );
    }

    #[test]
    fn rating_source_ignored_when_rating_off() {
        let addon = BetterPostersAddon {
            genre: true,
            rating: false,
            quality: false,
            age_rating: false,
            trend_tags: true,
            language: None,
            rating_source: Some("IM".to_string()),
        };
        let url = build_url("tt0111161", &addon);
        // rs= must not appear
        assert!(!url.contains("rs="));
    }
}
