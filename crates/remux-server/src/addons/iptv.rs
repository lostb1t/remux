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

const IPTV_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

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
            supported_resources: vec![
                AddonMetadata::simple_resource(ResourceType::Stream),
                AddonMetadata::simple_resource(ResourceType::Catalog),
            ],
            supported_types: vec![MediaKind::TvChannel],
            supported_resources_user: vec![],
            supported_types_user: vec![],
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
                enable_new_channels_option(),
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
        let enable_new_channels = cfg["enable_new_channels"]
            .as_bool()
            .unwrap_or(false);

        let addon = Arc::new(IptvAddon {
            addon_id,
            m3u_url: url,
            epg_url,
            xtream_username: None,
            xtream_password: None,
            is_xtream: false,
            sync_vod: false,
            sync_series: false,
            enable_new_channels,
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
            supported_resources: vec![AddonMetadata::simple_resource(ResourceType::Stream), AddonMetadata::simple_resource(ResourceType::Catalog)],
            supported_types: vec![MediaKind::TvChannel],
            supported_resources_user: vec![],
            supported_types_user: vec![],
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
                enable_new_channels_option(),
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
        let enable_new_channels = cfg["enable_new_channels"]
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
            enable_new_channels,
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

fn enable_new_channels_option() -> AddonOption {
    AddonOption {
        id: "enable_new_channels".to_string(),
        name: "Enable new channels".to_string(),
        description: Some(
            "Import new channels as enabled instead of disabled. Channels that \
             are already imported keep their current state."
                .to_string(),
        ),
        required: false,
        default: Some(serde_json::Value::Bool(false)),
        kind: AddonOptionType::Boolean,
    }
}

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
    /// Save live channels as enabled the first time they are imported.
    pub(crate) enable_new_channels: bool,
}

impl IptvAddon {
    pub(crate) fn source_id(&self) -> String {
        self.addon_id
            .simple()
            .to_string()
    }
}

/// `enabled` only applies to a channel seen for the first time: on later
/// imports `db::Media::upsert` keeps the stored value, so an admin's choice
/// sticks.
fn channel_to_media(
    ch: &iptv::M3uChannel,
    addon_id: Uuid,
    source_id: &str,
    enabled: bool,
) -> db::Media {
    let id = Uuid::new_v5(
        &addon_id,
        ch.url
            .as_bytes(),
    );
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
        enabled,
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
    ) -> Result<Option<(Vec<remux_sdks::stremio::ResourceRef>, Vec<StremioMediaType>)>>
    {
        let make_ref = |name| remux_sdks::stremio::ResourceRef {
            name,
            types: vec![],
            id_prefixes: None,
        };
        Ok(Some((
            vec![
                make_ref(ResourceType::Stream),
                make_ref(ResourceType::Catalog),
            ],
            vec![StremioMediaType::Tv],
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

        let client = reqwest::Client::builder()
            .user_agent(IPTV_USER_AGENT)
            .build()?;
        let source_id = self.source_id();
        let addon_id = self.addon_id;
        let enabled = self.enable_new_channels;

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
                    .map(|ch| channel_to_media(ch, addon_id, &source_id, enabled))
                    .collect(),
                Err(e) => {
                    warn!(error = %e, "failed to fetch Xtream channels");
                    return Err(e);
                }
            }
        } else {
            debug!(url = %crate::playback::probe::redact_url(&self.m3u_url), "fetching M3U playlist");
            let resp = client
                .get(&self.m3u_url)
                .send()
                .await?;
            let channels = iptv::parse_m3u_stream(resp).await?;
            channels
                .iter()
                .map(|ch| channel_to_media(ch, addon_id, &source_id, enabled))
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
        _id_prefixes: Option<&[String]>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    #[test]
    fn both_presets_offer_the_option_off_by_default() {
        for meta in [IptvM3uPreset.metadata(), IptvXstreamPreset.metadata()] {
            let option = meta
                .options
                .iter()
                .find(|o| o.id == "enable_new_channels")
                .unwrap_or_else(|| {
                    panic!("{} has no enable_new_channels option", meta.id)
                });
            assert!(matches!(option.kind, AddonOptionType::Boolean));
            assert_eq!(option.default, Some(serde_json::Value::Bool(false)));
            assert!(!option.required);
        }
    }

    #[tokio::test]
    async fn fetched_channels_follow_the_option() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let provider = httpmock::MockServer::start();
        provider.mock(|when, then| {
            when.path("/list.m3u");
            then.status(200)
                .body(
                    "#EXTM3U\n#EXTINF:-1 tvg-id=\"ch1\",Channel 1\nhttp://stream/1\n",
                );
        });
        provider.mock(|when, then| {
            when.path("/player_api.php")
                .query_param("action", "get_live_categories");
            then.status(200)
                .json_body(json!([]));
        });
        provider.mock(|when, then| {
            when.path("/player_api.php")
                .query_param("action", "get_live_streams");
            then.status(200)
                .json_body(json!([{ "name": "Channel 1", "stream_id": 1 }]));
        });
        let config = crate::Config::default();
        let with_option = |mut cfg: serde_json::Value, option: Option<bool>| {
            if let Some(on) = option {
                cfg["enable_new_channels"] = json!(on);
            }
            cfg
        };
        let m3u = |option: Option<bool>| {
            let cfg = json!({ "url": format!("{}/list.m3u", provider.base_url()) });
            IptvM3uPreset
                .from_cfg(Uuid::new_v4(), &with_option(cfg, option), &config)
                .unwrap()
        };
        let xtream = |option: Option<bool>| {
            let cfg = json!({
                "server_url": provider.base_url(),
                "username": "user",
                "password": "pass",
            });
            IptvXstreamPreset
                .from_cfg(Uuid::new_v4(), &with_option(cfg, option), &config)
                .unwrap()
        };
        let enabled = |caps: AddonCapabilities| async move {
            caps.catalog
                .unwrap()
                .catalog_stream(ctx, "all")
                .await
                .unwrap()
                .unwrap()
                .map(|m| m.enabled)
                .collect::<Vec<_>>()
                .await
        };

        // Add-ons saved before the option existed have no key: still disabled.
        assert_eq!(enabled(m3u(None)).await, vec![false]);
        assert_eq!(enabled(xtream(None)).await, vec![false]);
        assert_eq!(enabled(m3u(Some(false))).await, vec![false]);
        assert_eq!(enabled(xtream(Some(false))).await, vec![false]);
        assert_eq!(enabled(m3u(Some(true))).await, vec![true]);
        assert_eq!(enabled(xtream(Some(true))).await, vec![true]);
    }

    #[tokio::test]
    async fn a_refresh_keeps_a_channel_the_admin_disabled() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon_id = Uuid::new_v4();
        let source_id = addon_id
            .simple()
            .to_string();
        let collection_id = Uuid::new_v5(&addon_id, b"all");
        let catalog = crate::addons::ResolvedCatalog {
            provider_catalog_id: "all".into(),
            catalog_id: format!("addon:{addon_id}:all"),
            collection_id,
            name: "All Channels".into(),
            media_kind: Some(db::MediaKind::TvChannel),
            collection_media_kind: None,
            enabled: true,
            max_items: None,
            tags: vec![],
        };
        db::Media {
            id: collection_id,
            kind: db::MediaKind::Collection,
            title: "All Channels".into(),
            ..Default::default()
        }
        .save(&ctx.db)
        .await
        .unwrap();
        let progress = ProgressReporter::new(Default::default());
        let url = |n: u32| format!("http://stream/{n}");
        let id = |n: u32| Uuid::new_v5(&addon_id, url(n).as_bytes());
        let import = |numbers: &[u32], enable_new_channels: bool| {
            let items: Vec<db::Media> = numbers
                .iter()
                .map(|&n| {
                    let ch = iptv::M3uChannel {
                        name: format!("Channel {n}"),
                        url: url(n),
                        ..Default::default()
                    };
                    channel_to_media(&ch, addon_id, &source_id, enable_new_channels)
                })
                .collect();
            let (catalog, progress) = (&catalog, &progress);
            async move {
                crate::tasks::catalog_import_shared::import_catalog_items(
                    ctx,
                    catalog,
                    &catalog.catalog_id,
                    100,
                    crate::addons::CatalogStream::complete(futures::stream::iter(
                        items,
                    )),
                    progress,
                )
                .await
                .unwrap();
            }
        };
        let enabled = |n: u32| async move {
            sqlx::query_scalar::<_, bool>("SELECT enabled FROM media WHERE id = ?")
                .bind(id(n))
                .fetch_one(&ctx.db)
                .await
                .unwrap()
        };

        import(&[1, 2], true).await;
        assert!(enabled(1).await);
        assert!(enabled(2).await);

        // Turn channel 1 off, as PATCH /remux/iptv/channels/{id} does with `enabled`.
        sqlx::query("UPDATE media SET enabled = 0 WHERE id = ?")
            .bind(id(1))
            .execute(&ctx.db)
            .await
            .unwrap();
        import(&[1, 2], true).await;
        assert!(
            !enabled(1).await,
            "a refresh turned a disabled channel back on"
        );
        assert!(enabled(2).await);

        // With the option off, a new channel arrives disabled as before, and the
        // stored channels keep their state.
        import(&[1, 2, 3], false).await;
        assert!(!enabled(3).await);
        assert!(!enabled(1).await);
        assert!(enabled(2).await);
    }
}
