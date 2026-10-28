use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, MediaKind, ResourceType, StreamAddon,
    stremio::StremioManifestUrl,
};
use crate::{
    AppContext, db,
    stream::{StreamDescriptor, StreamInfo},
};

/// Eclipse manifests are not Stremio manifests: `resources` also lists
/// Eclipse-only entries (`settings`, `isrc`, `resolve`, ...) that the Stremio
/// parser rejects, so read only what we need and ignore the rest.
#[derive(Deserialize)]
struct EclipseManifest {
    #[serde(default)]
    resources: Vec<String>,
    #[serde(default)]
    types: Vec<String>,
}

impl EclipseManifest {
    fn info(&self) -> super::ManifestInfo {
        let resources = self
            .resources
            .iter()
            .filter_map(|r| {
                r.parse::<ResourceType>()
                    .ok()
            })
            .filter(|r| matches!(r, ResourceType::Stream | ResourceType::Search))
            .map(|name| remux_sdks::stremio::ResourceRef {
                name,
                types: self
                    .types
                    .clone(),
                id_prefixes: None,
            })
            .collect();
        let types = self
            .types
            .iter()
            .map(|t| {
                serde_json::from_value(serde_json::Value::String(t.clone()))
                    .unwrap_or(remux_sdks::stremio::MediaType::Other(t.clone()))
            })
            .collect();
        (resources, types)
    }
}

#[derive(Deserialize)]
struct EclipseSearchResponse {
    tracks: Vec<EclipseTrack>,
}

#[derive(Deserialize)]
struct EclipseTrack {
    id: String,
    title: String,
    artist: String,
    album: String,
    duration: i64,
}

#[derive(Deserialize)]
struct EclipseStreamResponse {
    url: String,
    quality: String,
}

/// Eclipse request timeout: covers connecting, the whole response and its
/// body, so a provider that stalls cannot hang startup or an addon reload.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn eclipse_from_cfg(cfg: &serde_json::Value) -> Result<AddonCapabilities> {
    let raw_url = cfg
        .get("manifest_url")
        .and_then(|v| v.as_str())
        .filter(|s| {
            !s.trim()
                .is_empty()
        })
        .ok_or_else(|| anyhow!("manifest_url is required"))?
        .to_string();
    let manifest_url = StremioManifestUrl::try_new(raw_url)
        .map_err(|e| anyhow!("Invalid manifest_url: {e}"))?;
    let client = reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    let addon = Arc::new(EclipseAddon {
        manifest_url,
        client,
        manifest: Default::default(),
    });
    Ok(AddonCapabilities {
        kind: Some(addon.clone()),
        stream: Some(addon),
        ..Default::default()
    })
}

/// `base` with `segments` appended to its path. The query (an addon token may
/// live there) is kept and `query` is added to it; a trailing slash on the
/// base does not produce an empty segment.
fn endpoint(base: &str, segments: &[&str], query: &[(&str, &str)]) -> Result<url::Url> {
    let mut url = url::Url::parse(base)?;
    url.path_segments_mut()
        .map_err(|_| anyhow!("manifest_url cannot be a base"))?
        .pop_if_empty()
        .extend(segments);
    if !query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(query);
    }
    Ok(url)
}

/// Generic Eclipse addon: the user pastes their own manifest URL.
pub struct EclipsePreset;

inventory::submit! {
    AddonPresetRegistration(|| Box::new(EclipsePreset))
}

impl AddonPreset for EclipsePreset {
    fn id(&self) -> &'static str {
        "eclipse"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "eclipse".to_string(),
            display_name: "Eclipse".to_string(),
            description: "Search and stream music from an Eclipse addon manifest"
                .to_string(),
            icon: None,
            supported_resources: vec![
                AddonMetadata::simple_resource(ResourceType::Stream),
                AddonMetadata::simple_resource(ResourceType::Search),
            ],
            supported_types: vec![
                MediaKind::Track,
                MediaKind::Album,
                MediaKind::Artist,
            ],
            supported_resources_user: vec![ResourceType::Stream, ResourceType::Search],
            supported_types_user: vec![
                MediaKind::Track,
                MediaKind::Album,
                MediaKind::Artist,
            ],
            options: vec![AddonOption {
                id: "manifest_url".to_string(),
                name: "Manifest URL".to_string(),
                description: Some(
                    "The manifest.json URL of your Eclipse addon".to_string(),
                ),
                required: true,
                default: None,
                kind: AddonOptionType::Url,
            }],
        }
    }

    fn from_cfg(
        &self,
        _id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        eclipse_from_cfg(cfg)
    }
}

pub struct EclipseAddon {
    manifest_url: StremioManifestUrl,
    client: reqwest::Client,
    manifest: super::ManifestCache,
}

impl EclipseAddon {
    fn base_url(&self) -> &str {
        self.manifest_url
            .as_ref()
    }
}

#[async_trait]
impl AddonKind for EclipseAddon {
    fn id(&self) -> &'static str {
        "eclipse"
    }

    async fn available_info(
        &self,
    ) -> Result<
        Option<(
            Vec<remux_sdks::stremio::ResourceRef>,
            Vec<remux_sdks::stremio::MediaType>,
        )>,
    > {
        let info = self
            .manifest
            .get_or_fetch(|| async {
                let manifest = self
                    .client
                    .get(endpoint(self.base_url(), &["manifest.json"], &[])?)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<EclipseManifest>()
                    .await?;
                Ok(manifest.info())
            })
            .await?;
        Ok(Some(info))
    }
}

#[async_trait]
impl StreamAddon for EclipseAddon {
    fn supports(&self, media: &db::Media) -> bool {
        matches!(
            media.kind,
            db::MediaKind::Track | db::MediaKind::Album | db::MediaKind::Artist
        )
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        _id_prefixes: Option<&[String]>,
    ) -> Result<Vec<StreamInfo>> {
        eclipse_streams(&self.client, self.base_url(), media, ctx).await
    }
}

async fn eclipse_streams(
    client: &reqwest::Client,
    base_url: &str,
    media: &db::Media,
    ctx: &AppContext,
) -> Result<Vec<StreamInfo>> {
    // Build query: include artist name when available. Prefer the artist row
    // (grandparent for tracks); playlist imports have no artist row, so fall
    // back to the flat artist name stored on the track itself.
    let gp_title = match media.grandparent_id {
        Some(gp_id) => db::Media::get_by_id(&ctx.db, &gp_id)
            .await
            .ok()
            .flatten()
            .map(|m| m.title),
        None => None,
    };
    let query = media.track_search_query_from(gp_title.as_deref());

    let search_url = endpoint(base_url, &["search"], &[("q", &query)])?;
    let resp = client
        .get(search_url)
        .send()
        .await?
        .error_for_status()?
        .json::<EclipseSearchResponse>()
        .await?;

    if resp
        .tracks
        .is_empty()
    {
        return Ok(vec![]);
    }

    // Pick the first result whose title matches (case-insensitive), else fall back to first.
    let title_lower = media
        .title
        .to_lowercase();
    let track = resp
        .tracks
        .iter()
        .find(|t| {
            t.title
                .to_lowercase()
                == title_lower
        })
        .unwrap_or(&resp.tracks[0]);

    let stream_url = endpoint(base_url, &["stream", &track.id], &[])?;
    let stream_resp = client
        .get(stream_url)
        .send()
        .await?
        .error_for_status()?
        .json::<EclipseStreamResponse>()
        .await?;

    Ok(vec![StreamInfo {
        descriptor: StreamDescriptor::http(stream_resp.url),
        name: Some(format!("Eclipse · {}", stream_resp.quality)),
        description: Some(format!("{} · {}", track.artist, track.album)),
        duration: Some(track.duration),
        ..Default::default()
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn generic_preset_requires_a_manifest_url() {
        assert!(eclipse_from_cfg(&json!({})).is_err());
        assert!(eclipse_from_cfg(&json!({ "manifest_url": "  " })).is_err());
        assert!(
            eclipse_from_cfg(
                &json!({ "manifest_url": "https://example.com/abc/manifest.json" })
            )
            .is_ok()
        );
    }

    #[test]
    fn manifest_with_eclipse_only_resources_parses() {
        let manifest: EclipseManifest = serde_json::from_value(json!({
            "id": "x", "name": "x", "version": "1.0.0",
            "types": ["track", "album"],
            "resources": ["search", "stream", "settings", "catalog", "isrc"],
        }))
        .unwrap();
        let (resources, types) = manifest.info();
        let names: Vec<_> = resources
            .iter()
            .map(|r| {
                r.name
                    .clone()
            })
            .collect();
        assert_eq!(names, vec![ResourceType::Search, ResourceType::Stream]);
        assert_eq!(types.len(), 2);
    }

    #[test]
    fn endpoint_keeps_the_query_and_never_doubles_slashes() {
        let url = |base, seg: &[&str], q: &[(&str, &str)]| {
            endpoint(base, seg, q)
                .unwrap()
                .to_string()
        };
        assert_eq!(
            url("https://h/addon?token=abc", &["manifest.json"], &[]),
            "https://h/addon/manifest.json?token=abc"
        );
        assert_eq!(
            url("https://h/", &["search"], &[("q", "a b")]),
            "https://h/search?q=a+b"
        );
        assert_eq!(
            url("https://h/a/b/", &["stream", "x/y"], &[]),
            "https://h/a/b/stream/x%2Fy"
        );
    }

    #[test]
    fn the_manifest_url_option_is_required() {
        assert!(
            EclipsePreset
                .metadata()
                .options[0]
                .required
        );
    }
}
