use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Datelike;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, MediaKind, TrackingAddon, TrackingCtx,
};
use crate::{
    db,
    tracking::{TrackingEvent, TrackingEventKind},
};

// ---------------------------------------------------------------------------
// Preset
// ---------------------------------------------------------------------------

pub struct YamtrackPreset;

impl AddonPreset for YamtrackPreset {
    fn id(&self) -> &'static str {
        "yamtrack"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "yamtrack".to_string(),
            display_name: "Yamtrack".to_string(),
            description:
                "Yamtrack — self-hosted media tracker. Scrobbles playback via webhooks."
                    .to_string(),
            icon: None,
            supported_resources: vec![],
            supported_types: vec![MediaKind::Movie, MediaKind::Series],
            supported_resources_user: vec![],
            supported_types_user: vec![MediaKind::Movie, MediaKind::Series],
            options: vec![AddonOption {
                id: "base_url".to_string(),
                name: "Yamtrack URL".to_string(),
                description: Some(
                    "Base URL of your Yamtrack instance (e.g. https://yamtrack.example.com)."
                        .to_string(),
                ),
                required: true,
                default: None,
                kind: AddonOptionType::Url,
            }],
            user_options: vec![AddonOption {
                id: "token".to_string(),
                name: "Webhook Token".to_string(),
                description: Some(
                    "Your Yamtrack webhook token (Settings → Integrations → Jellyfin)."
                        .to_string(),
                ),
                required: true,
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
        let base_url = cfg
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_default();

        let addon = Arc::new(YamtrackAddon { base_url });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            tracking: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(YamtrackPreset))
}

// ---------------------------------------------------------------------------
// Addon
// ---------------------------------------------------------------------------

pub struct YamtrackAddon {
    base_url: String,
}

impl AddonKind for YamtrackAddon {
    fn id(&self) -> &'static str {
        "yamtrack"
    }
}

#[async_trait]
impl TrackingAddon for YamtrackAddon {
    fn event_filter(&self) -> Option<Vec<TrackingEventKind>> {
        Some(vec![
            TrackingEventKind::PlaybackStop,
            TrackingEventKind::MarkPlayed,
            TrackingEventKind::MarkUnplayed,
        ])
    }

    async fn on_event(
        &self,
        event: &TrackingEvent,
        _user: &db::User,
        media: &db::Media,
        user_config: &Value,
        ctx: &TrackingCtx,
    ) -> Result<()> {
        let token = user_config
            .get("token")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .context("yamtrack: user token not configured")?;

        if self
            .base_url
            .is_empty()
        {
            anyhow::bail!("yamtrack: base_url not configured");
        }

        // For episodes, resolve the series-level external IDs (TMDB/TVDB/IMDB).
        // Yamtrack needs series IDs, not episode IDs.
        let series = if media.kind == db::MediaKind::Episode {
            let ancestors = db::Media::get_ancestors(&ctx.db, &media.id).await?;
            ancestors
                .into_iter()
                .find(|a| a.kind == db::MediaKind::Series)
        } else {
            None
        };

        let payload = build_payload(event, media, series.as_ref())?;
        let url = format!(
            "{}/webhook/jellyfin/{}",
            self.base_url
                .trim_end_matches('/'),
            token
        );

        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .context("yamtrack: webhook POST failed")?;

        if !resp
            .status()
            .is_success()
        {
            anyhow::bail!("yamtrack: webhook returned {} for {}", resp.status(), url);
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Payload construction
// ---------------------------------------------------------------------------

fn jellyfin_event_name(event: &TrackingEvent) -> &'static str {
    match event {
        TrackingEvent::PlaybackStop { .. } => "PlaybackStop",
        TrackingEvent::MarkPlayed { .. } => "MarkPlayed",
        TrackingEvent::MarkUnplayed { .. } => "MarkUnplayed",
        _ => "Unknown",
    }
}

fn build_payload(
    event: &TrackingEvent,
    media: &db::Media,
    series: Option<&db::Media>,
) -> Result<Value> {
    let played = matches!(
        event,
        TrackingEvent::PlaybackStop { .. } | TrackingEvent::MarkPlayed { .. }
    );
    let position_ticks: i64 = match event {
        TrackingEvent::PlaybackStop { position_ticks, .. } => *position_ticks,
        _ => 0,
    };

    // For episodes, use series-level provider IDs so Yamtrack can match the show.
    let ext_ids = series
        .map(|s| &s.external_ids)
        .unwrap_or(&media.external_ids);
    let provider_ids = build_provider_ids(ext_ids);
    let item = build_item(media, series, provider_ids);

    Ok(json!({
        "Event": jellyfin_event_name(event),
        "Item": item,
        "UserData": {
            "Played": played,
            "PlaybackPositionTicks": position_ticks,
            "PlayCount": if played { 1 } else { 0 },
        }
    }))
}

fn build_provider_ids(ext: &db::ExternalIds) -> Value {
    let mut map = serde_json::Map::new();
    if let Some(tmdb) = ext.tmdb {
        map.insert("Tmdb".to_string(), json!(tmdb.to_string()));
    }
    if let Some(ref imdb) = ext.imdb {
        map.insert("Imdb".to_string(), json!(imdb.as_str()));
    }
    if let Some(tvdb) = ext.tvdb {
        map.insert("Tvdb".to_string(), json!(tvdb.to_string()));
    }
    Value::Object(map)
}

fn build_item(
    media: &db::Media,
    series: Option<&db::Media>,
    provider_ids: Value,
) -> Value {
    let year = |m: &db::Media| {
        m.released_at
            .map(|d| {
                d.and_utc()
                    .year()
            })
    };

    match media.kind {
        db::MediaKind::Episode => {
            let series_name = series
                .map(|s| {
                    s.title
                        .as_str()
                })
                .unwrap_or(&media.title);
            let production_year = year(series.unwrap_or(media));
            json!({
                "Type": "Episode",
                "Name": media.title,
                "SeriesName": series_name,
                "ParentIndexNumber": media.parent_idx.unwrap_or(1),
                "IndexNumber": media.idx.unwrap_or(1),
                "ProductionYear": production_year,
                "ProviderIds": provider_ids,
            })
        }
        db::MediaKind::Movie => {
            json!({
                "Type": "Movie",
                "Name": media.title,
                "ProductionYear": year(media),
                "ProviderIds": provider_ids,
            })
        }
        _ => {
            json!({
                "Type": "Movie",
                "Name": media.title,
                "ProviderIds": provider_ids,
            })
        }
    }
}
