use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;

use super::{StreamOption, StreamService};

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
];

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
}

impl Default for SquidStreamService {
    fn default() -> Self {
        Self {
            client: build_client(),
        }
    }
}

impl SquidStreamService {
    fn get_instances(&self) -> Vec<String> {
        INSTANCES.iter().map(|s| s.to_string()).collect()
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
}

async fn try_instance(
    client: &reqwest::Client,
    base: &str,
    query: &str,
) -> Result<Option<StreamOption>> {
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
            tracing::debug!(query, base, "squid/tidal: no tracks in search result");
            return Ok(None);
        }
    };

    tracing::debug!(track_id, "squid/tidal: fetching manifest");

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
        tracing::debug!(track_id, "squid/tidal: DASH manifest, skipping");
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
        .map(super::normalize_codec)
        .map(str::to_string);

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

#[async_trait]
impl StreamService for SquidStreamService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Track]
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<StreamOption>> {
        let query = Self::build_query(media);
        tracing::debug!(query, title = %media.title, "squid/tidal stream lookup");

        let instances = self.get_instances();

        // Race all instances in parallel — first successful result wins.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(StreamOption, String)>(1);
        let mut handles = Vec::with_capacity(instances.len());

        for base in instances {
            let tx = tx.clone();
            let query = query.clone();
            let client = self.client.clone();
            let handle = tokio::spawn(async move {
                match try_instance(&client, &base, &query).await {
                    Ok(Some(stream)) => {
                        let _ = tx.send((stream, base)).await;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::debug!(base, error = %e, "squid/tidal: instance failed");
                    }
                }
            });
            handles.push(handle);
        }
        drop(tx); // rx.recv() returns None once all tasks finish without sending

        let result = if let Some((stream, base)) = rx.recv().await {
            tracing::debug!(query, base, label = %stream.label, "squid/tidal: stream resolved");
            Ok(vec![stream])
        } else {
            tracing::warn!(query, "squid/tidal: all instances failed");
            Ok(vec![])
        };

        // Cancel tasks that are still running.
        for h in handles {
            h.abort();
        }

        result
    }
}
