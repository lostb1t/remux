use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use tracing::{info, warn};

use remux_macros::{delete, get, post};
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt,
    addons::{
        Addon, AddonCapabilities, AddonCatalogDto, AddonDto, AddonMetadata,
        AddonPreset, AddonService, CreateAddonRequest, UpdateAddonCatalogRequest,
        UpdateAddonRequest, registered_presets, set_user_addon_override,
        user_addon_override,
    },
    db::{MediaKind as DbMediaKind, auth},
};
use axum_anyhow::ApiResult as Result;

type CapabilitySnapshot = (Vec<remux_sdks::stremio::ResourceType>, Vec<DbMediaKind>);

/// Whether a resource is switched on when an addon's resources are derived
/// from what it offers. Seek previews are not: they cost a second read of a
/// source, so they stay off until an admin turns them on.
fn on_by_default(resource: &remux_sdks::stremio::ResourceType) -> bool {
    *resource != remux_sdks::stremio::ResourceType::SeekPreviews
}

fn preset_capability_snapshot(preset: &dyn AddonPreset) -> CapabilitySnapshot {
    let metadata = preset.metadata();
    (
        metadata
            .supported_resources
            .into_iter()
            .map(|resource| resource.name)
            .filter(on_by_default)
            .collect(),
        metadata
            .supported_types
            .into_iter()
            .map(DbMediaKind::from)
            .collect(),
    )
}

async fn capability_snapshot(
    preset: &dyn AddonPreset,
    caps: &AddonCapabilities,
) -> anyhow::Result<CapabilitySnapshot> {
    let Some(kind) = caps
        .kind
        .as_deref()
    else {
        return Ok(preset_capability_snapshot(preset));
    };
    let Some((resources, types)) = kind
        .available_info()
        .await?
    else {
        return Ok(preset_capability_snapshot(preset));
    };
    let resources = resources
        .into_iter()
        .map(|resource| resource.name)
        .filter(on_by_default)
        .collect();
    let types: Vec<_> = types
        .into_iter()
        .filter_map(crate::addons::recognized_manifest_media_kind)
        .map(DbMediaKind::from)
        .collect();
    let types = if types.is_empty() {
        preset_capability_snapshot(preset).1
    } else {
        types
    };
    Ok((resources, types))
}

fn addon_to_dto(addon: Addon, addons: &AddonService) -> AddonDto {
    let preset = registered_presets()
        .into_iter()
        .find(|p| {
            p.id()
                == addon
                    .preset
                    .kind
        });

    let mut manifest_unreachable = false;
    let (
        supported_resources,
        supported_types,
        supported_resources_user,
        supported_types_user,
    ) = if let Some(ref p) = preset {
        // Runtime metadata was resolved when the addon was loaded. Listing
        // addons must not make another remote manifest request: one stalled
        // provider would otherwise hold the entire dashboard response open.
        let loaded = addons.list();
        let runtime = loaded
            .iter()
            .find(|r| {
                r.row
                    .id
                    == addon.id
            });
        manifest_unreachable = runtime.is_some_and(|r| {
            r.caps
                .manifest_unreachable
        });
        let meta = runtime
            .map(|r| {
                r.caps
                    .metadata
                    .clone()
            })
            .unwrap_or_else(|| p.metadata());
        let resources_user = meta
            .supported_resources_user
            .clone();
        let types_user = meta
            .supported_types_user
            .clone();
        let mut resources: Vec<_> = meta
            .supported_resources
            .into_iter()
            .map(|r| r.name)
            .collect();
        let mut types = meta.supported_types;
        // Disabled addons have no loaded runtime, so keep whatever was already
        // enabled for them selectable instead of showing only the preset defaults.
        if runtime.is_none() {
            for r in &addon.resources {
                if !resources.contains(r) {
                    resources.push(r.clone());
                }
            }
            for t in addon
                .types
                .iter()
                .flatten()
                .cloned()
                .map(Into::into)
            {
                if !types.contains(&t) {
                    types.push(t);
                }
            }
        }
        (resources, types, resources_user, types_user)
    } else {
        (vec![], vec![], vec![], vec![])
    };

    AddonDto {
        id: addon.id,
        kind: addon
            .preset
            .kind,
        name: addon.name,
        config: addon
            .preset
            .config
            .into_inner(),
        resources: addon.resources,
        types: addon
            .types
            .map(|types| {
                types
                    .into_iter()
                    .map(Into::into)
                    .collect()
            }),
        enabled: addon.enabled,
        supported_resources,
        supported_types,
        supported_resources_user,
        supported_types_user,
        priority: addon.priority,
        system: addon.system,
        is_default: addon.is_default,
        http_redirect_stream: addon.http_redirect_stream,
        subtitle_extraction: addon.subtitle_extraction,
        probe_on_scan: addon.probe_on_scan,
        service_filter: addon.service_filter,
        description: preset.map(|p| {
            p.metadata()
                .description
        }),
        manifest_unreachable,
        created_at: addon.created_at,
        updated_at: addon.updated_at,
    }
}

/// List metadata for every registered addon kind. Drives the dashboard's
/// "add addon" picker and form renderer.
#[get("/addon-kinds")]
pub async fn list_addon_kinds(
    State(_state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<Json<Vec<AddonMetadata>>> {
    Ok(Json(
        registered_presets()
            .iter()
            .map(|p| p.metadata())
            .collect(),
    ))
}

/// List all configured addon instances.
#[get("/addons")]
pub async fn list_addons(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<Json<Vec<AddonDto>>> {
    let addons = Addon::list(
        &state
            .ctx
            .db,
    )
    .await?;
    let dtos = addons
        .into_iter()
        .map(|addon| {
            addon_to_dto(
                addon,
                &state
                    .ctx
                    .addons,
            )
        })
        .collect();
    Ok(Json(dtos))
}

/// Get a single addon instance by ID.
#[get("/addons/{id}")]
pub async fn get_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<Json<AddonDto>> {
    let addon = Addon::get(
        &state
            .ctx
            .db,
        id,
    )
    .await?
    .context_not_found("Addon not found")?;
    Ok(Json(addon_to_dto(
        addon,
        &state
            .ctx
            .addons,
    )))
}

/// Create a new addon instance.
#[post("/addons")]
pub async fn create_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(mut payload): Json<CreateAddonRequest>,
) -> Result<(StatusCode, Json<AddonDto>)> {
    let presets = registered_presets();
    let preset = presets
        .iter()
        .find(|p| {
            p.id()
                == payload
                    .preset
                    .kind
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown addon kind: {}",
                payload
                    .preset
                    .kind
            )
        })
        .context_bad_request("Unknown addon kind")?;
    let addon_id = Uuid::new_v4();
    let normalized_config = preset
        .normalize_cfg(
            payload
                .preset
                .config
                .into_inner(),
            &state
                .ctx
                .config,
        )
        .context_bad_request("Invalid addon configuration")?;
    payload
        .preset
        .config = normalized_config.into();
    let caps = preset
        .from_cfg(
            addon_id,
            payload
                .preset
                .config
                .expose(),
            &state
                .ctx
                .config,
        )
        .context_bad_request("Invalid addon configuration")?;
    let (derived_resources, derived_types) =
        capability_snapshot(preset.as_ref(), &caps)
            .await
            .context_not_reachable()?;

    let resources: Vec<remux_sdks::stremio::ResourceType> = if payload
        .resources
        .is_empty()
    {
        derived_resources
    } else {
        payload.resources
    };
    let types = if payload
        .types
        .is_empty()
    {
        // Every type the addon supports; an empty list would mean none.
        (!derived_types.is_empty()).then_some(derived_types)
    } else {
        Some(
            payload
                .types
                .into_iter()
                .map(DbMediaKind::from)
                .collect(),
        )
    };

    let now = Utc::now().naive_utc();
    let addon = Addon {
        id: addon_id,
        preset: payload.preset,
        name: payload.name,
        resources,
        types,
        enabled: true,
        priority: payload.priority,
        created_at: now,
        updated_at: now,
        system: false,
        is_default: payload.is_default,
        http_redirect_stream: false,
        subtitle_extraction: false,
        probe_on_scan: false,
        service_filter: vec![],
    };

    addon
        .insert(
            &state
                .ctx
                .db,
        )
        .await?;
    // Before the reload, so a reload that fails leaves no stale rule.
    state
        .ctx
        .sessions
        .trickplay
        .set_addon_rule(&addon);
    state
        .ctx
        .addons
        .reload(
            &state
                .ctx
                .db,
            &state
                .ctx
                .config,
        )
        .await?;
    crate::trickplay::refresh_addon_rules(&state.ctx).await;
    Ok((
        StatusCode::CREATED,
        Json(addon_to_dto(
            addon,
            &state
                .ctx
                .addons,
        )),
    ))
}

/// Update an existing addon instance. Any field omitted is left unchanged.
#[post("/addons/{id}")]
pub async fn update_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateAddonRequest>,
) -> Result<Json<AddonDto>> {
    let UpdateAddonRequest {
        name,
        config,
        resources,
        types,
        enabled,
        priority,
        is_default,
        http_redirect_stream,
        subtitle_extraction,
        probe_on_scan,
        service_filter,
    } = payload;
    let mut addon = Addon::get(
        &state
            .ctx
            .db,
        id,
    )
    .await?
    .context_not_found("Addon not found")?;
    let was_previews_only = crate::trickplay::previews_only(&addon.resources);

    // Keep these Options available below: explicit fields take precedence over
    // capability values derived when the config changes.
    if let Some(resources) = &resources {
        if addon.system {
            if resources != &addon.resources {
                warn!("ignoring resources change on system addon {}", addon.id);
            }
        } else {
            addon.resources = resources.clone();
        }
    }
    if let Some(types) = &types {
        let types = Some(
            types
                .iter()
                .cloned()
                .map(DbMediaKind::from)
                .collect(),
        );
        if addon.system {
            if types != addon.types {
                warn!("ignoring types change on system addon {}", addon.id);
            }
        } else {
            addon.types = types;
        }
    }
    if let Some(name) = name {
        addon.name = name;
    }
    if let Some(enabled) = enabled {
        addon.enabled = enabled;
    }
    if let Some(priority) = priority {
        addon.priority = priority;
    }
    if let Some(is_default) = is_default {
        addon.is_default = is_default;
    }
    if let Some(http_redirect_stream) = http_redirect_stream {
        addon.http_redirect_stream = http_redirect_stream;
    }
    if let Some(probe_on_scan) = probe_on_scan {
        addon.probe_on_scan = probe_on_scan;
    }
    if let Some(subtitle_extraction) = subtitle_extraction {
        addon.subtitle_extraction = subtitle_extraction;
    }
    if let Some(service_filter) = service_filter {
        addon.service_filter = service_filter;
    }
    addon.updated_at = Utc::now().naive_utc();

    let presets = registered_presets();
    let preset = presets
        .iter()
        .find(|p| {
            p.id()
                == addon
                    .preset
                    .kind
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown addon kind: {}",
                addon
                    .preset
                    .kind
            )
        })
        .context_bad_request("Unknown addon kind")?;
    let config_changed = config.is_some();
    if let Some(config) = config {
        addon
            .preset
            .config = preset
            .normalize_cfg(
                config,
                &state
                    .ctx
                    .config,
            )
            .context_bad_request("Invalid addon configuration")?
            .into();
    }
    let caps = preset
        .from_cfg(
            addon.id,
            addon
                .preset
                .config
                .expose(),
            &state
                .ctx
                .config,
        )
        .context_bad_request("Invalid addon configuration")?;

    if config_changed && !addon.system {
        let (derived_resources, derived_types) =
            match capability_snapshot(preset.as_ref(), &caps).await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    warn!(
                        addon_id = %addon.id,
                        %error,
                        "failed to refresh addon capabilities; using preset metadata"
                    );
                    preset_capability_snapshot(preset.as_ref())
                }
            };
        if resources.is_none() {
            // Derived resources leave seek previews off; a reset keeps them
            // as they were.
            let previews = remux_sdks::stremio::ResourceType::SeekPreviews;
            let had_previews = addon
                .resources
                .contains(&previews);
            addon.resources = derived_resources;
            if had_previews
                && !addon
                    .resources
                    .contains(&previews)
            {
                addon
                    .resources
                    .push(previews);
            }
        }
        if types.is_none() {
            addon.types = (!derived_types.is_empty()).then_some(derived_types);
        }
    }

    addon
        .update(
            &state
                .ctx
                .db,
        )
        .await?;
    // Before the reload, so a reload that fails leaves no stale rule.
    state
        .ctx
        .sessions
        .trickplay
        .set_addon_rule(&addon);
    state
        .ctx
        .addons
        .reload(
            &state
                .ctx
                .db,
            &state
                .ctx
                .config,
        )
        .await?;
    crate::trickplay::refresh_addon_rules(&state.ctx).await;
    // After the reload, so no lookup asks the addon for streams once they are
    // deleted. On every save that leaves it so, so a failed delete is done by
    // the next save.
    if crate::trickplay::previews_only(&addon.resources) {
        let db = state
            .ctx
            .db
            .clone();
        let removed = delete_streams_of(&db, addon.id).await?;
        if removed > 0 || !was_previews_only {
            info!(
                addon = %addon.name,
                removed,
                "addon gives seek previews only now: removed the streams it gave"
            );
        }
        sweep_streams_later(db, addon.id);
    }
    Ok(Json(addon_to_dto(
        addon,
        &state
            .ctx
            .addons,
    )))
}

/// Deletes the stream rows an addon gave, once it gives seek previews only:
/// no viewer may be offered its streams from then on, and a play in progress
/// from one of them ends. A version it shared with an addon of higher
/// priority carries that addon's id and stays. The addon's file index is not
/// touched.
async fn delete_streams_of(
    db: &sqlx::SqlitePool,
    addon_id: Uuid,
) -> anyhow::Result<u64> {
    let done = sqlx::query(
        "DELETE FROM media \
         WHERE kind = 'stream' AND json_extract(stream_info, '$.addon_id') = ?1",
    )
    .bind(addon_id.to_string())
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// How long after an addon turned previews-only its streams are deleted
/// once more: a lookup that asked it before the save may write them after.
const STREAM_SWEEP_DELAY: std::time::Duration = std::time::Duration::from_secs(120);

/// Deletes an addon's stream rows again after [`STREAM_SWEEP_DELAY`], if it
/// still gives seek previews only then.
fn sweep_streams_later(db: sqlx::SqlitePool, addon_id: Uuid) {
    tokio::spawn(async move {
        tokio::time::sleep(STREAM_SWEEP_DELAY).await;
        let still = Addon::get(&db, addon_id)
            .await
            .ok()
            .flatten()
            .is_some_and(|addon| crate::trickplay::previews_only(&addon.resources));
        if !still {
            return;
        }
        match delete_streams_of(&db, addon_id).await {
            Ok(0) => {}
            Ok(removed) => {
                info!(
                    %addon_id,
                    removed,
                    "removed streams a previews-only addon gave late"
                );
            }
            Err(error) => {
                warn!(
                    %addon_id,
                    %error,
                    "could not remove a previews-only addon's streams"
                );
            }
        }
    });
}

/// Delete an addon instance.
#[delete("/addons/{id}")]
pub async fn delete_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let addon_row = Addon::get(
        &state
            .ctx
            .db,
        id,
    )
    .await?
    .context_not_found("Addon not found")?;

    if addon_row.system {
        return Err(anyhow::anyhow!("Cannot delete a system addon")
            .context_forbidden("Forbidden"));
    }

    // Purge the addon's index (removes e.g. IPTV channels) before deleting.
    if let Some(runtime) = state
        .ctx
        .addons
        .get(id)
    {
        if let Some(index) = &runtime.index {
            if let Err(e) = index
                .purge_index(&state.ctx, &addon_row)
                .await
            {
                warn!(addon = %id, error = %e, "purge_index failed on addon delete");
            }
        }
    }

    Addon::delete(
        &state
            .ctx
            .db,
        id,
    )
    .await?;
    // Before the reload, so a reload that fails leaves no stale rule.
    state
        .ctx
        .sessions
        .trickplay
        .drop_addon_rule(id);
    state
        .ctx
        .addons
        .reload(
            &state
                .ctx
                .db,
            &state
                .ctx
                .config,
        )
        .await?;
    crate::trickplay::refresh_addon_rules(&state.ctx).await;

    Ok(StatusCode::NO_CONTENT)
}

/// List catalogs for an addon merged with their config state.
/// Catalogs are disabled by default until explicitly enabled.
#[get("/addons/{id}/catalogs")]
pub async fn get_addon_catalogs(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<AddonCatalogDto>>> {
    let runtime = state
        .ctx
        .addons
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("addon not instantiated"))
        .context_bad_request("Addon could not be instantiated")?;

    let resolved = runtime
        .resolve_catalogs(&state.ctx)
        .await
        .context_not_reachable()?;

    let result = resolved
        .into_iter()
        .map(|c| AddonCatalogDto {
            catalog_id: c.catalog_id,
            name: c.name,
            enabled: c.enabled,
            max_items: c.max_items,
            tags: c.tags,
            collection_id: Some(c.collection_id),
        })
        .collect();

    Ok(Json(result))
}

/// Batch-update enabled/max_items for an addon's catalogs.
#[post("/addons/{id}/catalogs")]
pub async fn update_addon_catalogs(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<Vec<UpdateAddonCatalogRequest>>,
) -> Result<StatusCode> {
    let mut addon = Addon::get(
        &state
            .ctx
            .db,
        id,
    )
    .await?
    .context_not_found("Addon not found")?;

    let prefix = format!("addon:{id}:");
    let mut states = addon.catalog_states();

    for req in &payload {
        let local_id = req
            .catalog_id
            .strip_prefix(&prefix)
            .unwrap_or(&req.catalog_id)
            .to_string();
        let new_tags = req
            .tags
            .clone()
            .unwrap_or_else(|| {
                states
                    .get(&local_id)
                    .map(|s| {
                        s.tags
                            .clone()
                    })
                    .unwrap_or_default()
            });
        states.insert(
            local_id.clone(),
            crate::addons::CatalogState {
                enabled: req.enabled,
                max_items: req.max_items,
                tags: new_tags.clone(),
            },
        );

        // Apply tags immediately to all media already in this catalog.
        let collection_id = Uuid::new_v5(&id, local_id.as_bytes());
        for tag in &new_tags {
            if let Err(e) = sqlx::query(
                "INSERT OR IGNORE INTO media_tags (media_id, tag) \
                 SELECT mr.right_media_id, ? FROM media_relations mr \
                 WHERE mr.left_media_id = ? AND mr.role = 'catalog'",
            )
            .bind(tag)
            .bind(collection_id)
            .execute(
                &state
                    .ctx
                    .db,
            )
            .await
            {
                warn!(addon = %id, catalog = %local_id, tag = %tag, error = %e, "failed to apply catalog tag");
            }
        }
    }

    addon.set_catalog_states(states);
    addon
        .update(
            &state
                .ctx
                .db,
        )
        .await?;
    state
        .ctx
        .addons
        .reload(
            &state
                .ctx
                .db,
            &state
                .ctx
                .config,
        )
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Get a user's addon override list (ordered by priority).
/// Returns an empty array when the user has no override (uses the default list).
#[get("/users/{user_id}/addons")]
pub async fn get_user_addons(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let ids = user_addon_override(
        &state
            .ctx
            .db,
        user_id,
    )
    .await?
    .unwrap_or_default();
    Ok(Json(ids))
}

/// Set a user's addon override list. Send an empty array to clear the override
/// and fall back to the default addon list.
#[post("/users/{user_id}/addons")]
pub async fn set_user_addons(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
    Json(addon_ids): Json<Vec<Uuid>>,
) -> Result<StatusCode> {
    set_user_addon_override(
        &state
            .ctx
            .db,
        user_id,
        &addon_ids,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::integration_test::{auth_header_with_token, authenticated_server};
    use async_trait::async_trait;
    use http::header::HeaderValue;
    use remux_sdks::remux::MediaKind;
    use serde_json::json;
    use std::sync::Arc;

    struct ManifestKind(Vec<remux_sdks::stremio::MediaType>);

    #[async_trait]
    impl crate::addons::AddonKind for ManifestKind {
        fn id(&self) -> &'static str {
            "test"
        }

        async fn available_info(
            &self,
        ) -> anyhow::Result<
            Option<(
                Vec<remux_sdks::stremio::ResourceRef>,
                Vec<remux_sdks::stremio::MediaType>,
            )>,
        > {
            Ok(Some((
                vec![],
                self.0
                    .clone(),
            )))
        }
    }

    fn auth(token: &str) -> (http::header::HeaderName, HeaderValue) {
        (
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&auth_header_with_token(token)).unwrap(),
        )
    }

    #[tokio::test]
    async fn capability_snapshot_uses_shared_mapping_and_falls_back_for_unknown_types()
    {
        let preset = registered_presets()
            .into_iter()
            .find(|preset| preset.id() == "stremio")
            .unwrap();

        let genre = capability_snapshot(
            preset.as_ref(),
            &AddonCapabilities {
                kind: Some(Arc::new(ManifestKind(vec![
                    remux_sdks::stremio::MediaType::Other("genre".to_string()),
                ]))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(genre.1, vec![DbMediaKind::Genre]);

        let unknown = capability_snapshot(
            preset.as_ref(),
            &AddonCapabilities {
                kind: Some(Arc::new(ManifestKind(vec![
                    remux_sdks::stremio::MediaType::Other("anime".to_string()),
                ]))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            !unknown
                .1
                .is_empty()
        );
    }

    #[tokio::test]
    async fn list_addon_kinds_includes_stremio() {
        let (server, _ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        let resp = server
            .get("/addon-kinds")
            .add_header(h, v)
            .await;
        resp.assert_status_ok();

        let kinds: Vec<AddonMetadata> = resp.json();
        assert!(
            kinds
                .iter()
                .any(|k| k.id == "stremio"),
            "stremio kind should be registered"
        );
        let stremio = kinds
            .iter()
            .find(|k| k.id == "stremio")
            .unwrap();
        assert_eq!(
            stremio
                .options
                .len(),
            1
        );
        assert_eq!(stremio.options[0].id, "manifest_url");
    }

    #[tokio::test]
    async fn create_list_delete_addon_roundtrip() {
        let (server, ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        // Record baseline count (migrations may seed default addons).
        let initial: Vec<AddonDto> = server
            .get("/addons")
            .add_header(h.clone(), v.clone())
            .await
            .json();
        let initial_count = initial.len();

        let create_resp = server
            .post("/addons")
            .add_header(h.clone(), v.clone())
            .json(&json!({
                "preset": {
                    "kind": "stremio",
                    "config": { "manifest_url": "https://v3-cinemeta.strem.io/manifest.json" }
                },
                "name": "Test Cinemeta",
                "resources": ["catalog"],
            }))
            .await;
        create_resp.assert_status(http::StatusCode::CREATED);

        let created: AddonDto = create_resp.json();
        assert_eq!(created.kind, "stremio");
        assert_eq!(created.name, "Test Cinemeta");

        // Registry should reflect the new addon immediately.
        assert!(
            ctx.0
                .addons
                .get(created.id)
                .is_some(),
            "registry did not pick up the new addon"
        );

        // List shows exactly one more than baseline.
        let list_resp = server
            .get("/addons")
            .add_header(h.clone(), v.clone())
            .await;
        list_resp.assert_status_ok();
        let list: Vec<AddonDto> = list_resp.json();
        assert_eq!(list.len(), initial_count + 1);
        assert!(
            list.iter()
                .any(|a| a.id == created.id)
        );

        // Delete.
        let del_resp = server
            .delete(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .await;
        del_resp.assert_status(http::StatusCode::NO_CONTENT);

        let list_after: Vec<AddonDto> = server
            .get("/addons")
            .add_header(h, v)
            .await
            .json();
        assert_eq!(
            list_after.len(),
            initial_count,
            "addon should be gone after delete"
        );
        assert!(
            ctx.0
                .addons
                .get(created.id)
                .is_none(),
            "registry should have dropped the deleted addon"
        );
    }

    /// A local addon in the temp directory, created with its resources
    /// derived from what it offers.
    async fn local_addon(server: &axum_test::TestServer, token: &str) -> AddonDto {
        let (h, v) = auth(token);
        let dir = std::env::temp_dir()
            .to_string_lossy()
            .to_string();
        server
            .post("/addons")
            .add_header(h, v)
            .json(&json!({
                "preset": {
                    "kind": "opendal-local",
                    "config": { "paths": [dir], "media_kind": "movie" }
                },
                "name": "Local files"
            }))
            .await
            .json()
    }

    #[tokio::test]
    async fn seek_previews_are_offered_off_and_a_reset_keeps_them_as_they_were() {
        use remux_sdks::stremio::ResourceType;
        let (server, ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let created = local_addon(&server, &token).await;
        assert!(
            created
                .supported_resources
                .contains(&ResourceType::SeekPreviews),
            "offered"
        );
        assert!(
            created
                .resources
                .contains(&ResourceType::Stream)
        );
        assert!(
            !created
                .resources
                .contains(&ResourceType::SeekPreviews),
            "off until turned on"
        );
        let rule = |ctx: &crate::integration_test::TestGuard| {
            ctx.0
                .sessions
                .trickplay
                .addon_rules()
                .get(&created.id)
                .cloned()
                .expect("the snapshot has the addon")
        };
        assert!(!rule(&ctx).reads);

        let on: AddonDto = server
            .post(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&json!({ "resources": ["stream", "seekPreviews"] }))
            .await
            .json();
        assert_eq!(
            on.resources,
            vec![ResourceType::Stream, ResourceType::SeekPreviews]
        );
        assert!(rule(&ctx).reads);
        assert!(!rule(&ctx).preview_only);

        // A config change sent without resources resets them to what the
        // addon offers: seek previews stay as they were.
        let config = created
            .config
            .clone();
        let reset: AddonDto = server
            .post(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&json!({ "config": config }))
            .await
            .json();
        assert!(
            reset
                .resources
                .contains(&ResourceType::SeekPreviews)
        );
        assert!(
            reset
                .resources
                .contains(&ResourceType::Stream)
        );

        let off: AddonDto = server
            .post(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&json!({ "resources": ["stream"] }))
            .await
            .json();
        assert!(
            !off.resources
                .contains(&ResourceType::SeekPreviews)
        );
        let reset: AddonDto = server
            .post(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&json!({ "config": config }))
            .await
            .json();
        assert!(
            !reset
                .resources
                .contains(&ResourceType::SeekPreviews)
        );
        assert!(!rule(&ctx).reads);

        let del_resp = server
            .delete(&format!("/addons/{}", created.id))
            .add_header(h, v)
            .await;
        del_resp.assert_status(http::StatusCode::NO_CONTENT);
        assert!(
            ctx.0
                .sessions
                .trickplay
                .addon_rules()
                .get(&created.id)
                .is_none(),
            "the snapshot dropped the deleted addon"
        );
    }

    #[tokio::test]
    async fn an_addon_turned_previews_only_loses_the_streams_it_gave() {
        let (server, ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let created = local_addon(&server, &token).await;
        let db = &ctx
            .0
            .db;
        let mut film = crate::db::Media {
            title: "Invented Film".to_string(),
            kind: DbMediaKind::Movie,
            external_ids: crate::db::ExternalIds {
                tmdb: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        film.save(db)
            .await
            .unwrap();
        let stream = |addon_id: Uuid, name: &str| crate::db::Media {
            title: name.to_string(),
            kind: DbMediaKind::Stream,
            parent_id: Some(film.id),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    format!("/media/{name}.mkv").into(),
                ),
                addon_id: Some(addon_id),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut own_a = stream(created.id, "own-a");
        let mut own_b = stream(created.id, "own-b");
        let mut shared = stream(Uuid::new_v4(), "other-addon");
        for row in [&mut own_a, &mut own_b, &mut shared] {
            row.save(db)
                .await
                .unwrap();
        }
        let exists = |id: Uuid| async move {
            crate::db::Media::get_by_id(db, &id)
                .await
                .unwrap()
                .is_some()
        };

        // A reorder, or seek previews turned on beside streams, removes
        // nothing.
        server
            .post(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&json!({ "priority": 5 }))
            .await
            .assert_status_ok();
        server
            .post(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&json!({ "resources": ["stream", "seekPreviews"] }))
            .await
            .assert_status_ok();
        assert!(exists(own_a.id).await);
        assert!(exists(own_b.id).await);

        server
            .post(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&json!({ "resources": ["seekPreviews"] }))
            .await
            .assert_status_ok();
        assert!(!exists(own_a.id).await, "not playable by its id");
        assert!(!exists(own_b.id).await);
        assert!(exists(shared.id).await, "another addon's version stays");
        let mut title = crate::db::Media::get_by_id(db, &film.id)
            .await
            .unwrap()
            .unwrap();
        let listed: Vec<Uuid> = title
            .streams(db)
            .await
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(listed, vec![shared.id]);
        assert!(
            ctx.0
                .sessions
                .trickplay
                .addon_rules()
                .get(&created.id)
                .is_some_and(|rule| rule.preview_only)
        );
    }

    #[tokio::test]
    async fn create_addon_rejects_unknown_kind() {
        let (server, _ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        let resp = server
            .post("/addons")
            .add_header(h, v)
            .expect_failure()
            .json(&json!({
                "preset": { "kind": "no-such-kind", "config": {} },
                "name": "Bad",
            }))
            .await;
        resp.assert_status(http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_addon_rejects_missing_required_config() {
        let (server, _ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        // Stremio requires manifest_url; omitting it should fail validation
        // because from_cfg() returns Err.
        let resp = server
            .post("/addons")
            .add_header(h, v)
            .expect_failure()
            .json(&json!({
                "preset": { "kind": "stremio", "config": {} },
                "name": "Missing URL",
            }))
            .await;
        resp.assert_status(http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn update_config_refreshes_derived_capabilities() {
        let (server, _ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let dir = std::env::temp_dir()
            .to_string_lossy()
            .to_string();

        let created: AddonDto = server
            .post("/addons")
            .add_header(h.clone(), v.clone())
            .json(&json!({
                "preset": {
                    "kind": "opendal-local",
                    "config": { "paths": [dir.clone()], "media_kind": "movie" }
                },
                "name": "Local files"
            }))
            .await
            .json();
        assert!(
            created
                .types
                .as_deref()
                .unwrap_or_default()
                .contains(&MediaKind::Movie)
        );

        let updated: AddonDto = server
            .post(&format!("/addons/{}", created.id))
            .add_header(h, v)
            .json(&json!({
                "config": { "paths": [dir], "media_kind": "episode" }
            }))
            .await
            .json();

        assert!(
            updated
                .types
                .as_deref()
                .unwrap_or_default()
                .contains(&MediaKind::Series)
        );
        assert!(
            !updated
                .types
                .as_deref()
                .unwrap_or_default()
                .contains(&MediaKind::Movie)
        );

        let (h, v) = auth(&token);
        let explicit: AddonDto = server
            .post(&format!("/addons/{}", created.id))
            .add_header(h, v)
            .json(&json!({
                "config": { "paths": [std::env::temp_dir()], "media_kind": "movie" },
                "resources": ["stream"],
                "types": ["series"]
            }))
            .await
            .json();
        assert_eq!(
            explicit.resources,
            vec![remux_sdks::stremio::ResourceType::Stream]
        );
        assert_eq!(explicit.types, Some(vec![MediaKind::Series]));

        let (h, v) = auth(&token);
        let explicit_empty: AddonDto = server
            .post(&format!("/addons/{}", created.id))
            .add_header(h, v)
            .json(&json!({
                "config": { "paths": [std::env::temp_dir()], "media_kind": "episode" },
                "resources": [],
                "types": []
            }))
            .await
            .json();
        assert!(
            explicit_empty
                .resources
                .is_empty()
        );
        // Unticking every type is kept: none, not all.
        assert_eq!(explicit_empty.types, Some(vec![]));
    }

    #[tokio::test]
    async fn update_config_survives_unreachable_capability_manifest() {
        let (server, ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let now = Utc::now().naive_utc();
        let mut addon = Addon {
            id: Uuid::new_v4(),
            name: "Unavailable manifest".to_string(),
            preset: crate::addons::AddonPresetRef {
                kind: "stremio".to_string(),
                config: json!({ "manifest_url": "http://127.0.0.1:1/manifest.json" })
                    .into(),
            },
            resources: vec![],
            types: None,
            enabled: false,
            priority: 0,
            created_at: now,
            updated_at: now,
            system: false,
            is_default: false,
            http_redirect_stream: false,
            subtitle_extraction: false,
            probe_on_scan: false,
            service_filter: vec![],
        };
        addon
            .insert(
                &ctx.0
                    .db,
            )
            .await
            .unwrap();

        let response = server
            .post(&format!("/addons/{}", addon.id))
            .add_header(h, v)
            .json(&json!({
                "name": "Updated while unavailable",
                "config": { "manifest_url": "http://127.0.0.1:1/manifest.json" }
            }))
            .await;
        response.assert_status_ok();

        addon = Addon::get(
            &ctx.0
                .db,
            addon.id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(addon.name, "Updated while unavailable");
        assert!(
            !addon
                .resources
                .is_empty()
        );
        assert!(
            addon
                .types
                .is_some_and(|types| !types.is_empty())
        );
    }

    #[tokio::test]
    async fn list_addons_does_not_refetch_unreachable_manifest() {
        let (server, ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let manifest = httpmock::MockServer::start();
        // The SDK response cache is process-wide and keyed by URL, and httpmock
        // reuses pooled servers, so this test needs a path no other test uses;
        // otherwise a manifest another test cached would answer the probe.
        let hits = manifest.mock(|when, then| {
            when.path("/unreachable-manifest/manifest.json");
            then.status(404);
        });
        let now = Utc::now().naive_utc();
        let addon = Addon {
            id: Uuid::new_v4(),
            name: "Unreachable manifest".to_string(),
            preset: crate::addons::AddonPresetRef {
                kind: "stremio".to_string(),
                config: json!({ "manifest_url": manifest.url("/unreachable-manifest/manifest.json") })
                    .into(),
            },
            resources: vec![],
            types: None,
            enabled: true,
            priority: 0,
            created_at: now,
            updated_at: now,
            system: false,
            is_default: false,
            http_redirect_stream: false,
            subtitle_extraction: false,
            probe_on_scan: false,
            service_filter: vec![],
        };
        addon
            .insert(
                &ctx.0
                    .db,
            )
            .await
            .unwrap();
        ctx.0
            .addons
            .reload(
                &ctx.0
                    .db,
                &ctx.0
                    .config,
            )
            .await
            .unwrap();
        let hits_after_load = hits.hits();
        assert!(hits_after_load >= 1, "load should have probed the manifest");

        for _ in 0..2 {
            let list: Vec<AddonDto> = server
                .get("/addons")
                .add_header(h.clone(), v.clone())
                .await
                .json();
            let dto = list
                .iter()
                .find(|a| a.id == addon.id)
                .expect("addon listed despite unreachable manifest");
            assert!(dto.manifest_unreachable);
            assert!(
                !dto.supported_resources
                    .is_empty()
            );
        }
        assert_eq!(
            hits.hits(),
            hits_after_load,
            "listing addons must not issue manifest requests"
        );
    }
}
