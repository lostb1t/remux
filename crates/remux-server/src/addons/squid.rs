use anyhow::Result;
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use sqlx::types::Json;
use std::sync::Arc;

use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration, MediaKind,
    ResourceType,
};
use crate::{AppContext, api, db};

const TIDAL_CLIENT: &str = "BiniLossless/v3.4";

const INSTANCES: &[&str] = &[
    "https://tidal-api.binimum.org",
    "https://eu-central.monochrome.tf",
    "https://frankfurt-2.monochrome.tf",
    "https://us-west.monochrome.tf",
    "https://arran.monochrome.tf",
    "https://api.monochrome.tf",
    "https://monochrome-api.samidy.com",
    "https://triton.squid.wtf",
    "https://vogel.qqdl.site",
    "https://katze.qqdl.site",
    "https://hund.qqdl.site",
    "https://wolf.qqdl.site",
    "https://maus.qqdl.site",
    "https://tidal.kinoplus.online",
    "https://hifi-one.spotisaver.net",
    "https://hifi-two.spotisaver.net",
    "https://hifi.geeked.wtf",
    "https://hfapi.dyamuh.dev",
    "https://hfapi.aluratech.org",
    "https://api.studentsneed.help",
];

// --- Deserialization types ---

#[derive(Deserialize)]
#[serde(untagged)]
enum TrackOuter {
    Wrapped { data: TrackInner },
    Flat(TrackInner),
}

impl TrackOuter {
    fn into_inner(self) -> TrackInner {
        match self {
            Self::Wrapped { data } => data,
            Self::Flat(inner) => inner,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackInner {
    manifest: Option<String>,
    #[serde(alias = "manifestMimeType", default)]
    manifest_mime_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecodedManifest {
    urls: Vec<String>,
    mime_type: String,
    #[serde(default)]
    codecs: Option<String>,
    #[serde(default)]
    sample_rate: Option<i64>,
}

// --- Helpers ---

fn normalize_codec(codec: &str) -> &str {
    if codec.starts_with("mp4a") {
        "aac"
    } else {
        codec
    }
}

fn mime_to_container(mime: &str) -> Option<String> {
    if mime.contains("flac") {
        Some("flac".to_string())
    } else if mime.contains("mp4") || mime.contains("m4a") {
        Some("mp4".to_string())
    } else if mime.contains("webm") || mime.contains("opus") {
        Some("webm".to_string())
    } else if mime.contains("mpeg") || mime.contains("mp3") {
        Some("mp3".to_string())
    } else {
        None
    }
}

fn build_query(media: &db::Media) -> String {
    let artist = media
        .description
        .as_deref()
        .and_then(|d| d.strip_prefix("by "))
        .unwrap_or("");
    if artist.is_empty() {
        media.title.clone()
    } else {
        format!("{} {}", media.title, artist)
    }
}

async fn try_instance(
    client: &reqwest::Client,
    base: &str,
    query: &str,
    parent: &db::Media,
) -> Result<Option<crate::stream::StreamInfo>> {
    let search_url = format!("{}/search/?s={}", base, urlencoding::encode(query));
    let resp = client
        .get(&search_url)
        .header("x-client", TIDAL_CLIENT)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("search HTTP {} — {}", status, body);
    }

    let body: serde_json::Value = resp.json().await?;
    let track_id = body
        .pointer("/data/items/0/id")
        .or_else(|| body.pointer("/data/tracks/items/0/id"))
        .or_else(|| body.pointer("/tracks/items/0/id"))
        .and_then(|v| v.as_u64())
        .map(|id| id.to_string());

    let track_id = match track_id {
        Some(id) => id,
        None => {
            tracing::debug!(query, base, "squid: no tracks in search result");
            return Ok(None);
        }
    };

    tracing::debug!(track_id, "squid: fetching manifest");

    let manifest_url = format!(
        "{}/track/?id={}&quality=LOSSLESS",
        base,
        urlencoding::encode(&track_id)
    );
    let resp = client
        .get(&manifest_url)
        .header("x-client", TIDAL_CLIENT)
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("manifest HTTP {}", resp.status());
    }

    let track = resp.json::<TrackOuter>().await?.into_inner();
    let manifest_b64 = match track.manifest {
        Some(m) => m,
        None => return Ok(None),
    };

    if track.manifest_mime_type.contains("dash")
        || manifest_b64.trim_start().starts_with('<')
    {
        tracing::debug!(track_id, "squid: DASH manifest, skipping");
        return Ok(None);
    }

    let decoded =
        base64::engine::general_purpose::STANDARD.decode(manifest_b64.trim())?;
    let manifest: DecodedManifest = serde_json::from_slice(&decoded)?;

    let url = match manifest.urls.into_iter().next() {
        Some(u) => u,
        None => return Ok(None),
    };

    let label = if manifest.mime_type.contains("flac") {
        "FLAC"
    } else if manifest.mime_type.contains("mp4") {
        "AAC"
    } else {
        "Audio"
    };

    let codec = manifest
        .codecs
        .as_deref()
        .map(normalize_codec)
        .map(str::to_string);
    let display_title = match codec.as_deref() {
        Some(c) => format!("{} - 2ch", c.to_uppercase()),
        None => label.to_string(),
    };

    Ok(Some(crate::stream::StreamInfo {
        descriptor: crate::stream::StreamDescriptor::http(url),
        name: Some(label.to_string()),
        probe_data: Some(api::MediaSourceInfo {
            container: mime_to_container(&manifest.mime_type),
            run_time_ticks: parent.runtime.map(|r| r * 10_000_000),
            media_streams: vec![api::MediaStream {
                index: 0,
                type_: Some(api::MediaStreamType::Audio),
                codec,
                channels: Some(2),
                sample_rate: manifest.sample_rate,
                is_default: Some(true),
                display_title: Some(display_title),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }))
}

// --- AddonPreset ---

pub struct SquidPreset;

impl AddonPreset for SquidPreset {
    fn id(&self) -> &'static str {
        "squid"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "squid".to_string(),
            display_name: "Squid (Tidal)".to_string(),
            description: "Lossless music streams via community Tidal proxy instances."
                .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Stream],
            supported_types: vec![MediaKind::Track],
            options: vec![],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        _cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<Arc<dyn AddonKind>> {
        let client = reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .timeout(std::time::Duration::from_secs(8))
            .build()?;
        Ok(Arc::new(SquidAddon { client }))
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(SquidPreset))
}

// --- Addon instance ---

pub struct SquidAddon {
    client: reqwest::Client,
}

#[async_trait]
impl AddonKind for SquidAddon {
    fn id(&self) -> &'static str {
        "squid"
    }

    fn stream_supports(&self, media: &db::Media) -> bool {
        media.kind == db::MediaKind::Track
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let query = build_query(media);
        tracing::debug!(query, title = %media.title, "squid stream lookup");

        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<(crate::stream::StreamInfo, String)>(1);
        let mut handles = Vec::with_capacity(INSTANCES.len());

        for base in INSTANCES {
            let tx = tx.clone();
            let query = query.clone();
            let client = self.client.clone();
            let parent = media.clone();
            let base = base.to_string();
            handles.push(tokio::spawn(async move {
                match try_instance(&client, &base, &query, &parent).await {
                    Ok(Some(source)) => {
                        let _ = tx.send((source, base)).await;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::debug!(base, error = %e, "squid: instance failed")
                    }
                }
            }));
        }
        drop(tx);

        let result = if let Some((source, base)) = rx.recv().await {
            tracing::debug!(query, base, label = ?source.name, "squid: stream resolved");
            Ok(vec![source])
        } else {
            tracing::warn!(query, "squid: all instances failed");
            Ok(vec![])
        };

        for h in handles {
            h.abort();
        }

        result
    }
}
