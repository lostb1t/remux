use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use tokio::sync::OnceCell;

use super::{StreamOption, StreamService};


const INSTANCES_URL: &str = "https://monochrome.tf/instances.json";
const PRIMARY_INSTANCE: &str = "https://tidal-api.binimum.org";
const TIDAL_CLIENT: &str = "BiniLossless/v3.4";

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .expect("failed to build HTTP client")
}

// Track manifest response — may or may not have a `data` wrapper
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

// Manifest JSON (after base64 decode) uses lowercase keys
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecodedManifest {
    urls: Vec<String>,
    mime_type: String,
    #[serde(default)]
    codecs: Option<String>,
    #[serde(default)]
    bit_depth: Option<i64>,
    #[serde(default)]
    sample_rate: Option<i64>,
}

pub struct SquidStreamService {
    client: reqwest::Client,
    instances: OnceCell<Vec<String>>,
}

impl Default for SquidStreamService {
    fn default() -> Self {
        Self { client: build_client(), instances: OnceCell::new() }
    }
}

impl SquidStreamService {
    async fn get_instances(&self) -> Vec<String> {
        self.instances
            .get_or_init(|| async {
                let mut list = vec![PRIMARY_INSTANCE.to_string()];
                if let Ok(body) = self.client.get(INSTANCES_URL).send().await {
                    if let Ok(resp) = body.json::<serde_json::Value>().await {
                        if let Some(api) = resp.get("api").and_then(|v| v.as_array()) {
                            for v in api {
                                if let Some(s) = v.as_str() {
                                    let url = s.trim_end_matches('/').to_string();
                                    if url != PRIMARY_INSTANCE {
                                        list.push(url);
                                    }
                                }
                            }
                        }
                    }
                }
                tracing::debug!(count = list.len(), "squid/tidal: instances loaded");
                list
            })
            .await
            .clone()
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

    async fn try_instance(&self, base: &str, query: &str) -> Result<Option<StreamOption>> {
        // Combined search — returns tracks.items among other types
        let search_url = format!("{}/search/?s={}", base, urlencoding::encode(query));
        let resp = self
            .client
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

        // ?s= returns { data: { items: [...] } }
        // ?q= (categorized) returns { data: { tracks: { items: [...] } } }
        let track_id = body
            .pointer("/data/items/0/id")
            .or_else(|| body.pointer("/data/tracks/items/0/id"))
            .or_else(|| body.pointer("/tracks/items/0/id"))
            .and_then(|v| v.as_u64())
            .map(|id| id.to_string());

        let track_id = match track_id {
            Some(id) => id,
            None => {
                tracing::debug!(query, base, "squid/tidal: no tracks in search result");
                return Ok(None);
            }
        };

        tracing::debug!(track_id, "squid/tidal: fetching manifest");

        let manifest_url =
            format!("{}/track/?id={}&quality=LOSSLESS", base, urlencoding::encode(&track_id));
        let resp = self
            .client
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

        // Skip DASH — not supported
        if track.manifest_mime_type.contains("dash") || manifest_b64.trim_start().starts_with('<') {
            tracing::debug!(track_id, "squid/tidal: DASH manifest, skipping");
            return Ok(None);
        }

        let decoded = base64::engine::general_purpose::STANDARD.decode(manifest_b64.trim())?;
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

        let codec = manifest.codecs.as_deref().map(super::normalize_codec).map(str::to_string);

        Ok(Some(StreamOption {
            url,
            label: label.to_string(),
            mime_type: manifest.mime_type,
            is_audio_only: true,
            codec,
            sample_rate: manifest.sample_rate,
            channels: Some(2),
            ..Default::default()
        }))
    }
}

#[async_trait]
impl StreamService for SquidStreamService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Track]
    }

    async fn get_streams(&self, media: &db::Media, _ctx: &AppContext) -> Result<Vec<StreamOption>> {
        let query = Self::build_query(media);
        tracing::debug!(query, title = %media.title, "squid/tidal stream lookup");

        let instances = self.get_instances().await;

        for base in &instances {
            match self.try_instance(base, &query).await {
                Ok(Some(stream)) => {
                    tracing::info!(query, base, label = %stream.label, "squid/tidal: stream resolved");
                    return Ok(vec![stream]);
                }
                Ok(None) => return Ok(vec![]),
                Err(e) => {
                    tracing::debug!(base, error = %e, "squid/tidal: instance failed, trying next");
                }
            }
        }

        tracing::warn!(query, "squid/tidal: all instances failed");
        Ok(vec![])
    }
}
