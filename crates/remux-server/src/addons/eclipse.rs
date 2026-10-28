use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, MediaKind, ResourceType, StreamAddon,
    stremio::{StremioManifestUrl, parse_manifest_info},
};
use crate::{
    AppContext, db,
    services::stremio as stremio_service,
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

fn eclipse_preset_options(
    default_url: &'static str,
    generate_url: &'static str,
) -> Vec<AddonOption> {
    vec![AddonOption {
        id: "manifest_url".to_string(),
        name: "Manifest URL".to_string(),
        description: Some(format!(
            "Optional. You can generate a new manifest URL at {generate_url}"
        )),
        required: false,
        default: Some(serde_json::Value::String(default_url.to_string())),
        kind: AddonOptionType::Url,
    }]
}

/// Builds the addon from `cfg.manifest_url`, falling back to `default_url`
/// for the legacy presets that ship one. The generic [`EclipsePreset`] has no
/// default: every user brings their own manifest.
fn eclipse_from_cfg(
    default_url: Option<&'static str>,
    cfg: &serde_json::Value,
) -> Result<AddonCapabilities> {
    let raw_url = cfg
        .get("manifest_url")
        .and_then(|v| v.as_str())
        .filter(|s| {
            !s.trim()
                .is_empty()
        })
        .or(default_url)
        .ok_or_else(|| anyhow!("manifest_url is required"))?
        .to_string();
    let manifest_url = StremioManifestUrl::try_new(raw_url)
        .map_err(|e| anyhow!("Invalid manifest_url: {e}"))?;
    let client = super::make_http_client();
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
        eclipse_from_cfg(None, cfg)
    }
}

const MONOCHROME_URL: &str = "https://monochrome1.cyrusna29.workers.dev/u/206f62ce5c9a5c710f2178a16238/manifest.json";
const MONOCHROME_GENERATE_URL: &str = "https://monochrome1.cyrusna29.workers.dev";

pub struct MonochromePreset;

inventory::submit! {
    AddonPresetRegistration(|| Box::new(MonochromePreset))
}

impl AddonPreset for MonochromePreset {
    fn deprecated(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        "monochrome"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "monochrome".to_string(),
            display_name: "Monochrome".to_string(),
            description: "Search and stream music".to_string(),
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
            options: eclipse_preset_options(MONOCHROME_URL, MONOCHROME_GENERATE_URL),
        }
    }

    fn from_cfg(
        &self,
        _id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        eclipse_from_cfg(Some(MONOCHROME_URL), cfg)
    }
}

const SPOTIFLAC_URL: &str =
    "https://spotiflac.eclipsemusic.app/5baa7290b334d6e2/manifest.json";
const SPOTIFLAC_GENERATE_URL: &str = "https://spotiflac.eclipsemusic.app";

pub struct SpotiFLACPreset;

inventory::submit! {
    AddonPresetRegistration(|| Box::new(SpotiFLACPreset))
}

impl AddonPreset for SpotiFLACPreset {
    fn deprecated(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        "eclipse_spotiflac"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "eclipse_spotiflac".to_string(),
            display_name: "SpotiFLAC".to_string(),
            description: "Search and stream music".to_string(),
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
            options: eclipse_preset_options(SPOTIFLAC_URL, SPOTIFLAC_GENERATE_URL),
        }
    }

    fn from_cfg(
        &self,
        _id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        eclipse_from_cfg(Some(SPOTIFLAC_URL), cfg)
    }
}

pub struct EclipseAddon {
    manifest_url: StremioManifestUrl,
    client: reqwest::Client,
    manifest: super::ManifestCache,
}

impl EclipseAddon {
    fn service(&self) -> Result<stremio_service::StremioService> {
        stremio_service::StremioService::from_url(&self.manifest_url)
    }

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
                    .get(format!("{}/manifest.json", self.base_url()))
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

    let search_url = format!("{}/search?q={}", base_url, urlencoding::encode(&query));
    let resp = client
        .get(&search_url)
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

    let stream_url = format!("{}/stream/{}", base_url, track.id);
    let stream_resp = client
        .get(&stream_url)
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
        assert!(eclipse_from_cfg(None, &json!({})).is_err());
        assert!(eclipse_from_cfg(None, &json!({ "manifest_url": "  " })).is_err());
        assert!(
            eclipse_from_cfg(
                None,
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
    fn legacy_presets_still_load_without_a_stored_url() {
        assert!(eclipse_from_cfg(Some(MONOCHROME_URL), &json!({})).is_ok());
        assert!(eclipse_from_cfg(Some(SPOTIFLAC_URL), &json!({})).is_ok());
    }

    #[test]
    fn only_the_generic_preset_is_offered_for_new_addons() {
        assert!(MonochromePreset.deprecated());
        assert!(SpotiFLACPreset.deprecated());
        assert!(!EclipsePreset.deprecated());
        assert!(
            EclipsePreset
                .metadata()
                .options[0]
                .required
        );
    }
}
