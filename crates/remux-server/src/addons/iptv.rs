use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use remux_sdks::stremio::MediaType as StremioMediaType;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, CatalogInfo, MediaKind, ProgressReporter, ResourceType,
};
use crate::addons::Addon;
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
    ) -> Result<Arc<dyn AddonKind>> {
        let url = cfg["url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("iptv-m3u: url is required"))?
            .to_string();
        let epg_url = cfg["epg_url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        Ok(Arc::new(IptvAddon {
            addon_id,
            m3u_url: url,
            epg_url,
            xtream_username: None,
            xtream_password: None,
            is_xtream: false,
        }))
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
            ],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
    ) -> Result<Arc<dyn AddonKind>> {
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

        Ok(Arc::new(IptvAddon {
            addon_id,
            m3u_url: server_url,
            epg_url: None,
            xtream_username: Some(username),
            xtream_password: Some(password),
            is_xtream: true,
        }))
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(IptvXstreamPreset))
}

// ---------------------------------------------------------------------------
// Shared runtime
// ---------------------------------------------------------------------------

struct IptvAddon {
    addon_id: Uuid,
    /// M3U URL for M3U addons; Xtream server base URL for Xtream addons.
    m3u_url: String,
    epg_url: Option<String>,
    xtream_username: Option<String>,
    xtream_password: Option<String>,
    is_xtream: bool,
}

#[async_trait]
impl AddonKind for IptvAddon {
    fn id(&self) -> &'static str {
        "iptv"
    }

    async fn available_info(&self) -> (Vec<ResourceType>, Vec<StremioMediaType>) {
        (
            vec![ResourceType::Stream, ResourceType::Catalog],
            vec![StremioMediaType::Tv],
        )
    }

    async fn catalog_list(&self, ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        let source_id = self.addon_id.simple().to_string();
        let groups: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT json_extract(external_ids, '$.iptv_group') AS grp \
             FROM media \
             WHERE kind = 'tv_channel' \
               AND json_extract(external_ids, '$.iptv_source_id') = ? \
               AND grp IS NOT NULL AND grp != '' \
             ORDER BY grp",
        )
        .bind(&source_id)
        .fetch_all(&ctx.db)
        .await?;

        let mut catalogs = vec![CatalogInfo {
            provider_catalog_id: "all".to_string(),
            name: "All Channels".to_string(),
            default_enabled: true,
            default_max_items: Some(999999999),
        }];

        for group in groups {
            catalogs.push(CatalogInfo {
                provider_catalog_id: format!("group:{}", group),
                name: group,
                default_enabled: false,
                default_max_items: Some(999999999),
            });
        }

        Ok(catalogs)
    }

    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        let source_id = self.addon_id.simple().to_string();

        let items: Vec<db::Media> = if let Some(group) = local_id.strip_prefix("group:")
        {
            sqlx::query_as::<_, db::Media>(
                "SELECT * FROM media \
                 WHERE kind = 'tv_channel' \
                   AND json_extract(external_ids, '$.iptv_source_id') = ? \
                   AND json_extract(external_ids, '$.iptv_group') = ? \
                 ORDER BY (sort_order IS NULL), COALESCE(sort_order, channel_number, 999999), title COLLATE NOCASE",
            )
            .bind(&source_id)
            .bind(group)
            .fetch_all(&ctx.db)
            .await?
        } else if local_id == "all" {
            sqlx::query_as::<_, db::Media>(
                "SELECT * FROM media \
                 WHERE kind = 'tv_channel' \
                   AND json_extract(external_ids, '$.iptv_source_id') = ? \
                 ORDER BY (sort_order IS NULL), COALESCE(sort_order, channel_number, 999999), title COLLATE NOCASE",
            )
            .bind(&source_id)
            .fetch_all(&ctx.db)
            .await?
        } else {
            return Ok(None);
        };

        Ok(Some(Box::pin(futures::stream::iter(items))))
    }

    fn stream_supports(&self, media: &db::Media) -> bool {
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

    async fn refresh_index(
        &self,
        ctx: &AppContext,
        _addon: &Addon,
        progress: ProgressReporter,
    ) -> Result<()> {
        let start = Instant::now();
        let client = reqwest::Client::new();
        let source_id = self.addon_id.simple().to_string();

        let channels_parsed = if self.is_xtream {
            let user = self.xtream_username.as_deref().unwrap_or("");
            let pass = self.xtream_password.as_deref().unwrap_or("");
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
                Ok(ch) => ch,
                Err(e) => {
                    warn!(error = %e, "failed to fetch Xtream channels");
                    return Err(e);
                }
            }
        } else {
            debug!(url = %self.m3u_url, "fetching M3U playlist");
            let resp = client.get(&self.m3u_url).send().await?;
            iptv::parse_m3u_stream(resp).await?
        };

        let channel_count = channels_parsed.len();
        let mut channel_refs: Vec<(Uuid, Option<String>)> =
            Vec::with_capacity(channel_count);

        for chunk in channels_parsed.chunks(1000) {
            let media_chunk: Vec<db::Media> = chunk
                .iter()
                .map(|ch| {
                    let tvg_key = ch.tvg_id.as_deref().unwrap_or(&ch.name);
                    let channel_id = Uuid::new_v5(&self.addon_id, tvg_key.as_bytes());
                    let mut media = db::Media {
                        id: channel_id,
                        title: ch.name.clone(),
                        kind: db::MediaKind::TvChannel,
                        stream_info: Some(crate::stream::StreamInfo {
                            descriptor: crate::stream::StreamDescriptor::http(
                                ch.url.clone(),
                            ),
                            ..Default::default()
                        }),
                        tvg_id: ch.tvg_id.clone(),
                        channel_number: ch.channel_number,
                        external_ids: db::ExternalIds {
                            iptv_source_id: Some(source_id.clone()),
                            iptv_group: ch.group.clone(),
                            ..Default::default()
                        },
                        enabled: false,
                        program_kind: ch.program_kind.clone(),
                        ..Default::default()
                    };
                    if let Some(url) = ch.logo.clone() {
                        media.set_image(db::ImageKind::Primary, url);
                    }
                    media
                })
                .collect();

            db::Media::upsert(&ctx.db, &media_chunk).await?;
            channel_refs.extend(media_chunk.iter().map(|c| (c.id, c.tvg_id.clone())));
        }
        drop(channels_parsed);

        progress.set(60.0);

        // Prune stale channels for this addon only.
        if !channel_refs.is_empty() {
            let mut tx = ctx.db.begin().await?;
            sqlx::query(
                "CREATE TEMPORARY TABLE IF NOT EXISTS _iptv_kept (id BLOB NOT NULL PRIMARY KEY)",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM _iptv_kept")
                .execute(&mut *tx)
                .await?;

            for chunk in channel_refs.chunks(500) {
                let mut qb =
                    sqlx::QueryBuilder::new("INSERT OR IGNORE INTO _iptv_kept (id) ");
                qb.push_values(chunk.iter(), |mut b, (id, _)| {
                    b.push_bind(*id);
                });
                qb.build().execute(&mut *tx).await?;
            }

            sqlx::query(
                "DELETE FROM media \
                 WHERE kind = 'tv_program' \
                   AND parent_id IN ( \
                       SELECT id FROM media \
                       WHERE kind = 'tv_channel' \
                         AND json_extract(external_ids, '$.iptv_source_id') = ? \
                         AND id NOT IN (SELECT id FROM _iptv_kept) \
                   )",
            )
            .bind(&source_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "DELETE FROM media \
                 WHERE kind = 'tv_channel' \
                   AND json_extract(external_ids, '$.iptv_source_id') = ? \
                   AND id NOT IN (SELECT id FROM _iptv_kept)",
            )
            .bind(&source_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
        }

        progress.set(70.0);

        // Fetch EPG.
        let mut epg_programs = 0usize;

        let epg_url = if self.is_xtream {
            // Xtream auto-derives the EPG URL.
            let user = self.xtream_username.as_deref().unwrap_or("");
            let pass = self.xtream_password.as_deref().unwrap_or("");
            let base = self.m3u_url.trim_end_matches('/');
            Some(format!(
                "{}/xmltv.php?username={}&password={}",
                base, user, pass
            ))
        } else {
            self.epg_url.clone()
        };

        if let Some(url) = epg_url {
            match iptv::stream_import_epg(&client, &url, &channel_refs, ctx).await {
                Ok(count) => epg_programs = count,
                Err(e) => warn!(error = %e, "failed to import EPG"),
            }
        }

        info!(
            channels = channel_count,
            programs = epg_programs,
            elapsed_s = start.elapsed().as_secs(),
            "IPTV addon refresh complete"
        );

        progress.set(100.0);
        Ok(())
    }

    async fn purge_index(&self, ctx: &AppContext, _addon: &Addon) -> Result<()> {
        let source_id = self.addon_id.simple().to_string();
        sqlx::query(
            "DELETE FROM media \
             WHERE kind = 'tv_program' \
               AND parent_id IN ( \
                   SELECT id FROM media \
                   WHERE kind = 'tv_channel' \
                     AND json_extract(external_ids, '$.iptv_source_id') = ? \
               )",
        )
        .bind(&source_id)
        .execute(&ctx.db)
        .await?;

        sqlx::query(
            "DELETE FROM media \
             WHERE kind = 'tv_channel' \
               AND json_extract(external_ids, '$.iptv_source_id') = ?",
        )
        .bind(&source_id)
        .execute(&ctx.db)
        .await?;

        Ok(())
    }
}
