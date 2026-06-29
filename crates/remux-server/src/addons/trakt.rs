use anyhow::Result;
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, MediaKind, MetricSnapshot, MetricValue, MetricsAddon,
    MetricsCtx, ResourceType,
};
use crate::{db, sdks};

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
            metrics: Some(Arc::new(TraktAddon {
                client_id,
                cache: Mutex::new(None),
            })),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(TraktPreset))
}

pub struct TraktAddon {
    client_id: Option<String>,
    // (date, movie_ceiling, show_ceiling) — refreshed daily from #1 popular item stats
    cache: Mutex<Option<(chrono::NaiveDate, f64, f64)>>,
}

impl TraktAddon {
    async fn ceilings(&self, ctx: &MetricsCtx) -> Option<(f64, f64)> {
        let client_id = self
            .client_id
            .as_deref()?;
        let today = chrono::Utc::now().date_naive();

        let mut cache = self
            .cache
            .lock()
            .await;
        if cache
            .as_ref()
            .map_or(true, |(d, _, _)| *d != today)
        {
            let client = sdks::trakt::trakt_client(
                client_id,
                &ctx.config
                    .trakt_base_url,
            )
            .ok()?;

            let movie_ceiling = async {
                let top = client
                    .execute(sdks::trakt::MoviePopularEndpoint { limit: 1 })
                    .await
                    .ok()?
                    .into_iter()
                    .next()?;
                let imdb_id = top
                    .ids
                    .imdb?;
                let stats = client
                    .execute(sdks::trakt::MovieStatsEndpoint { imdb_id })
                    .await
                    .ok()?;
                Some(
                    stats
                        .raw_score()
                        .max(1.0),
                )
            }
            .await
            .unwrap_or(500_000.0);

            let show_ceiling = async {
                let top = client
                    .execute(sdks::trakt::ShowPopularEndpoint { limit: 1 })
                    .await
                    .ok()?
                    .into_iter()
                    .next()?;
                let imdb_id = top
                    .ids
                    .imdb?;
                let stats = client
                    .execute(sdks::trakt::ShowStatsEndpoint { imdb_id })
                    .await
                    .ok()?;
                Some(
                    stats
                        .raw_score()
                        .max(1.0),
                )
            }
            .await
            .unwrap_or(500_000.0);

            *cache = Some((today, movie_ceiling, show_ceiling));
        }

        cache
            .as_ref()
            .map(|(_, m, s)| (*m, *s))
    }
}

#[async_trait]
impl MetricsAddon for TraktAddon {
    async fn metric(
        &self,
        media: &db::Media,
        ctx: &MetricsCtx,
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
        let Some(client_id) = self
            .client_id
            .as_deref()
        else {
            return Ok(None);
        };

        let Some((movie_ceiling, show_ceiling)) = self
            .ceilings(ctx)
            .await
        else {
            return Ok(None);
        };

        let client = sdks::trakt::trakt_client(
            client_id,
            &ctx.config
                .trakt_base_url,
        )?;
        let today = chrono::Utc::now().date_naive();

        let stats = loop {
            let result = match media.kind {
                db::MediaKind::Movie => {
                    client
                        .execute(sdks::trakt::MovieStatsEndpoint {
                            imdb_id: imdb_id.to_string(),
                        })
                        .await
                }
                db::MediaKind::Series => {
                    client
                        .execute(sdks::trakt::ShowStatsEndpoint {
                            imdb_id: imdb_id.to_string(),
                        })
                        .await
                }
                _ => return Ok(None),
            };
            match result {
                Ok(s) => break s,
                Err(sdks::ClientError::RateLimited { retry_after_secs }) => {
                    tokio::time::sleep(Duration::from_secs(retry_after_secs)).await;
                }
                Err(_) => return Ok(None),
            }
        };

        let ceiling = match media.kind {
            db::MediaKind::Movie => movie_ceiling,
            _ => show_ceiling,
        };

        let age_years = media
            .released_at
            .map(|d| {
                let days = (chrono::Utc::now().naive_utc() - d).num_days();
                days as f64 / 365.25
            })
            .unwrap_or(5.0)
            .max(0.0);

        let raw = stats.raw_score();
        let base = (raw / ceiling * 100.0).clamp(0.0, 100.0);
        let decay = 1.0 / (1.0 + age_years * 0.1);
        let score = base * decay;

        let external_id = format!("trakt:{}", imdb_id);
        Ok(Some(MetricSnapshot {
            source: "trakt".to_string(),
            media_id: Some(media.id),
            media_raw: Some(external_id.clone()),
            external_id,
            value: MetricValue::from_normalized(score),
            date: today,
        }))
    }
}
