use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, MediaKind, MetricSnapshot, MetricValue, MetricsAddon,
    ResourceType,
};
use crate::{AppContext, db, sdks};

pub struct TraktPreset;

impl AddonPreset for TraktPreset {
    fn id(&self) -> &'static str {
        "trakt"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "trakt".to_string(),
            display_name: "Trakt".to_string(),
            description: "Trakt — crowd-sourced ratings for movies and shows.".to_string(),
            icon: None,
            supported_resources: vec![AddonMetadata::simple_resource(ResourceType::Metrics)],
            supported_types: vec![MediaKind::Movie, MediaKind::Series],
            options: vec![AddonOption {
                id: "client_id".to_string(),
                name: "Trakt Client ID".to_string(),
                description: Some(
                    "Your Trakt application Client ID. Register a free app at trakt.tv/oauth/applications."
                        .to_string(),
                ),
                required: false,
                default: None,
                kind: AddonOptionType::Password,
            }],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let client_id = cfg
            .get("client_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(AddonCapabilities {
            metrics: Some(Arc::new(TraktAddon { client_id })),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(TraktPreset))
}

pub struct TraktAddon {
    client_id: Option<String>,
}

const TRAKT_WATCHERS_MAX: f64 = 700_000.0;

#[async_trait]
impl MetricsAddon for TraktAddon {
    async fn metric(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MetricSnapshot>> {
        if !matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series) {
            return Ok(None);
        }
        let Some(imdb_id) = media
            .external_ids
            .imdb
            .as_deref()
        else {
            return Ok(None);
        };
        let Some(ref client_id) = self.client_id else {
            return Ok(None);
        };

        let client = sdks::trakt::trakt_client(
            client_id,
            &ctx.config
                .trakt_base_url,
        )
        .map_err(anyhow::Error::from)?;
        let today = chrono::Utc::now().date_naive();

        let watchers: Option<u64> = match media.kind {
            db::MediaKind::Movie => client
                .execute(sdks::trakt::MovieStatsEndpoint {
                    imdb_id: imdb_id.to_string(),
                })
                .await
                .ok()
                .map(|r| r.watchers),
            db::MediaKind::Series => client
                .execute(sdks::trakt::ShowStatsEndpoint {
                    imdb_id: imdb_id.to_string(),
                })
                .await
                .ok()
                .map(|r| r.watchers),
            _ => return Ok(None),
        };

        let external_id = format!("trakt:{}", imdb_id);
        Ok(watchers.map(|w| MetricSnapshot {
            source: "trakt".to_string(),
            media_id: Some(media.id),
            media_raw: Some(external_id.clone()),
            external_id,
            value: MetricValue::from_raw(w as f64, TRAKT_WATCHERS_MAX),
            date: today,
        }))
    }
}
