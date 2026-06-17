use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use remux_sdks::stremio::MediaType as StremioMediaType;
use std::{pin::Pin, sync::Arc};
use tracing::{debug, warn};
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, CatalogAddon, CatalogInfo, MediaKind,
    ProgressReporter, ResourceType, StreamAddon,
};
use crate::{AppContext, db, iptv};

// ---------------------------------------------------------------------------
// IptvM3uPreset
// ---------------------------------------------------------------------------

pub struct IptvM3uPreset;

impl AddonPreset for IptvM3uPreset {
    fn id(&self) -> &'static str {
        "iptv-m3u"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "iptv-m3u".to_string(),
            display_name: "M3U Playlist".to_string(),
            description: "Import live TV channels from an M3U playlist URL."
                .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Stream, ResourceType::Catalog],
            supported_types: vec![MediaKind::TvChannel],
            options: vec![
                AddonOption {
                    id: "url".to_string(),
                    name: "Playlist URL".to_string(),
                    description: Some(
                        "HTTP(S) URL of the M3U playlist file.".to_string(),
                    ),
                    required: true,
                    default: None,
                    kind: AddonOptionType::Url,
                },
                AddonOption {
                    id: "epg_url".to_string(),
                    name: "EPG URL".to_string(),
                    description: Some(
                        "Optional XMLTV EPG URL for programme guide data.".to_string(),
                    ),
                    required: false,
                    default: None,
                    kind: AddonOptionType::Url,
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let url = cfg["url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("iptv-m3u: url is required"))?
            .to_string();
        let epg_url = cfg["epg_url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let addon = Arc::new(IptvAddon {
            addon_id,
            m3u_url: url,
            epg_url,
            xtream_username: None,
            xtream_password: None,
            is_xtream: false,
            sync_vod: false,
            sync_series: false,
        });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            catalog: Some(addon.clone()),
            stream: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(IptvM3uPreset))
}

// ---------------------------------------------------------------------------
// IptvXstreamPreset
// ---------------------------------------------------------------------------

pub struct IptvXstreamPreset;

impl AddonPreset for IptvXstreamPreset {
    fn id(&self) -> &'static str {
        "iptv-xtream"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "iptv-xtream".to_string(),
            display_name: "Xtream Codes".to_string(),
            description: "Import live TV channels from an Xtream Codes provider."
                .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Stream, ResourceType::Catalog],
            supported_types: vec![MediaKind::TvChannel],
            options: vec![
                AddonOption {
                    id: "server_url".to_string(),
                    name: "Server URL".to_string(),
                    description: Some(
                        "Base URL of the Xtream Codes server (e.g. http://provider.com:8080)."
                            .to_string(),
                    ),
                    required: true,
                    default: None,
                    kind: AddonOptionType::Url,
                },
                AddonOption {
                    id: "username".to_string(),
                    name: "Username".to_string(),
                    description: None,
                    required: true,
                    default: None,
                    kind: AddonOptionType::String,
                },
                AddonOption {
                    id: "password".to_string(),
                    name: "Password".to_string(),
                    description: None,
                    required: true,
                    default: None,
                    kind: AddonOptionType::Password,
                },
                AddonOption {
                    id: "sync_vod".to_string(),
                    name: "Import VOD movies".to_string(),
                    description: Some("Fetch and import VOD movies from this provider.".to_string()),
                    required: false,
                    default: Some(serde_json::Value::Bool(false)),
                    kind: AddonOptionType::Boolean,
                },
                AddonOption {
                    id: "sync_series".to_string(),
                    name: "Import TV series".to_string(),
                    description: Some("Fetch and import TV series from this provider.".to_string()),
                    required: false,
                    default: Some(serde_json::Value::Bool(false)),
                    kind: AddonOptionType::Boolean,
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let server_url = cfg["server_url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("iptv-xtream: server_url is required"))?
            .to_string();
        let username = cfg["username"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("iptv-xtream: username is required"))?
            .to_string();
        let password = cfg["password"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("iptv-xtream: password is required"))?
            .to_string();

        let sync_vod = cfg["sync_vod"]
            .as_bool()
            .unwrap_or(false);
        let sync_series = cfg["sync_series"]
            .as_bool()
            .unwrap_or(false);

        let addon = Arc::new(IptvAddon {
            addon_id,
            m3u_url: server_url,
            epg_url: None,
            xtream_username: Some(username),
            xtream_password: Some(password),
            is_xtream: true,
            sync_vod,
            sync_series,
        });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            catalog: Some(addon.clone()),
            stream: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(IptvXstreamPreset))
}

// ---------------------------------------------------------------------------
// Shared runtime
// ---------------------------------------------------------------------------

pub(crate) struct IptvAddon {
    pub(crate) addon_id: Uuid,
    /// M3U URL for M3U addons; Xtream server base URL for Xtream addons.
    pub(crate) m3u_url: String,
    pub(crate) epg_url: Option<String>,
    pub(crate) xtream_username: Option<String>,
    pub(crate) xtream_password: Option<String>,
    pub(crate) is_xtream: bool,
    pub(crate) sync_vod: bool,
    pub(crate) sync_series: bool,
}

impl IptvAddon {
    pub(crate) fn source_id(&self) -> String {
        self.addon_id
            .simple()
            .to_string()
    }
}

fn channel_to_media(
    ch: &iptv::M3uChannel,
    addon_id: Uuid,
    source_id: &str,
) -> db::Media {
    let tvg_key = ch
        .tvg_id
        .as_deref()
        .unwrap_or(&ch.name);
    let id = Uuid::new_v5(&addon_id, tvg_key.as_bytes());
    let mut media = db::Media {
        id,
        title: ch
            .name
            .clone(),
        kind: db::MediaKind::TvChannel,
        stream_info: Some(crate::stream::StreamInfo {
            descriptor: crate::stream::StreamDescriptor::http(
                ch.url
                    .clone(),
            ),
            catchup_source: ch
                .catchup_source
                .clone(),
            catchup_days: ch.catchup_days,
            ..Default::default()
        }),
        tvg_id: ch
            .tvg_id
            .clone(),
        channel_number: ch.channel_number,
        external_ids: db::ExternalIds {
            iptv_source_id: Some(source_id.to_owned()),
            iptv_group: ch
                .group
                .clone(),
            ..Default::default()
        },
        enabled: false,
        program_kind: ch
            .program_kind
            .clone(),
        ..Default::default()
    };
    if let Some(url) = ch
        .logo
        .clone()
    {
        media.set_image(db::ImageKind::Primary, url);
    }
    media
}

#[async_trait]
impl AddonKind for IptvAddon {
    fn id(&self) -> &'static str {
        "iptv"
    }

    async fn available_info(
        &self,
    ) -> Result<
        Option<(
            Vec<ResourceType>,
            Vec<StremioMediaType>,
            Option<Vec<String>>,
            Option<Vec<String>>,
            Option<Vec<String>>,
        )>,
    > {
        Ok(Some((
            vec![ResourceType::Stream, ResourceType::Catalog],
            vec![StremioMediaType::Tv],
            None,
            None,
            None,
        )))
    }
}

#[async_trait]
impl CatalogAddon for IptvAddon {
    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        Ok(vec![CatalogInfo {
            provider_catalog_id: "all".to_string(),
            name: "All Channels".to_string(),
            default_enabled: true,
            default_max_items: Some(999999999),
            collection_media_kind: None,
            media_kind: Some(db::MediaKind::TvChannel),
        }])
    }

    async fn catalog_stream(
        &self,
        _ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        if local_id != "all" {
            return Ok(None);
        }

        let client = reqwest::Client::new();
        let source_id = self.source_id();
        let addon_id = self.addon_id;

        let mut items: Vec<db::Media> = if self.is_xtream {
            let user = self
                .xtream_username
                .as_deref()
                .unwrap_or("");
            let pass = self
                .xtream_password
                .as_deref()
                .unwrap_or("");

            let category_kinds =
                iptv::fetch_xtream_categories(&client, &self.m3u_url, user, pass).await;
            debug!(
                categories = category_kinds.len(),
                "fetched Xtream categories"
            );

            match iptv::fetch_xtream_channels(
                &client,
                &self.m3u_url,
                user,
                pass,
                &category_kinds,
            )
            .await
            {
                Ok(channels) => channels
                    .iter()
                    .map(|ch| channel_to_media(ch, addon_id, &source_id))
                    .collect(),
                Err(e) => {
                    warn!(error = %e, "failed to fetch Xtream channels");
                    return Err(e);
                }
            }
        } else {
            debug!(url = %self.m3u_url, "fetching M3U playlist");
            let resp = client
                .get(&self.m3u_url)
                .send()
                .await?;
            let channels = iptv::parse_m3u_stream(resp).await?;
            channels
                .iter()
                .map(|ch| channel_to_media(ch, addon_id, &source_id))
                .collect()
        };

        // For Xtream: optionally append VOD and series as TvChannel items.
        if self.is_xtream {
            let user = self
                .xtream_username
                .as_deref()
                .unwrap_or("");
            let pass = self
                .xtream_password
                .as_deref()
                .unwrap_or("");

            if self.sync_vod {
                debug!("fetching Xtream VOD streams");
                match iptv::fetch_vod_streams(
                    &client,
                    &self.m3u_url,
                    user,
                    pass,
                    addon_id,
                    &source_id,
                )
                .await
                {
                    Ok(vod) => items.extend(vod),
                    Err(e) => warn!(error = %e, "failed to fetch Xtream VOD"),
                }
            }

            if self.sync_series {
                debug!("fetching Xtream series");
                match iptv::fetch_series_list(
                    &client,
                    &self.m3u_url,
                    user,
                    pass,
                    addon_id,
                    &source_id,
                )
                .await
                {
                    Ok(series) => items.extend(series),
                    Err(e) => warn!(error = %e, "failed to fetch Xtream series"),
                }
            }
        }

        Ok(Some(Box::pin(futures::stream::iter(items))))
    }
}

#[async_trait]
impl StreamAddon for IptvAddon {
    fn supports(&self, media: &db::Media) -> bool {
        media.kind == db::MediaKind::TvChannel
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let Some(ref si) = media.stream_info else {
            return Ok(vec![]);
        };
        Ok(vec![si.clone()])
    }
}

// ---------------------------------------------------------------------------
// Unused import suppression — ProgressReporter is used by the EPG task
// ---------------------------------------------------------------------------
const _: fn() = || {
    let _ = std::mem::size_of::<ProgressReporter>();
};
