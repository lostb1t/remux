use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;

use remux_macros::{delete, get, post};
use uuid::Uuid;

use crate::AppState;
use crate::addons::{
    Addon, AddonCatalogDto, AddonDto, AddonMetadata, CreateAddonRequest,
    UpdateAddonCatalogRequest, UpdateAddonRequest, make_media_id, registered_presets,
};
use crate::db::{MediaKind as DbMediaKind, auth};
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};
use remux_sdks::remux::MediaKind;

async fn addon_to_dto(addon: Addon) -> AddonDto {
    let preset = registered_presets()
        .into_iter()
        .find(|p| p.id() == addon.preset.kind);

    let (supported_resources, supported_types) = if let Some(ref p) = preset {
        let meta = p.metadata();
        match p.from_cfg(addon.id, &addon.preset.config) {
            Ok(kind) => {
                let (resources, raw_types) = kind.available_info().await;
                let types: Vec<MediaKind> = raw_types
                    .into_iter()
                    .filter_map(|t| DbMediaKind::try_from(t).ok().map(Into::into))
                    .collect();
                if !resources.is_empty() {
                    (resources, types)
                } else {
                    (meta.supported_resources, meta.supported_types)
                }
            }
            Err(_) => (meta.supported_resources, meta.supported_types),
        }
    } else {
        (vec![], vec![])
    };

    AddonDto {
        id: addon.id,
        kind: addon.preset.kind,
        name: addon.name,
        config: addon.preset.config,
        resources: addon.resources,
        types: addon.types.iter().cloned().map(Into::into).collect(),
        enabled: addon.enabled,
        supported_resources,
        supported_types,
        priority: addon.priority,
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
        registered_presets().iter().map(|p| p.metadata()).collect(),
    ))
}

/// List all configured addon instances.
#[get("/addons")]
pub async fn list_addons(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<Json<Vec<AddonDto>>> {
    let addons = Addon::list(&state.ctx.db).await?;
    let dtos = futures::future::join_all(addons.into_iter().map(addon_to_dto)).await;
    Ok(Json(dtos))
}

/// Get a single addon instance by ID.
#[get("/addons/{id}")]
pub async fn get_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<Json<AddonDto>> {
    let addon = Addon::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;
    Ok(Json(addon_to_dto(addon).await))
}

/// Create a new addon instance.
#[post("/addons")]
pub async fn create_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(payload): Json<CreateAddonRequest>,
) -> Result<(StatusCode, Json<AddonDto>)> {
    let presets = registered_presets();
    let preset = presets
        .iter()
        .find(|p| p.id() == payload.preset.kind)
        .ok_or_else(|| anyhow::anyhow!("unknown addon kind: {}", payload.preset.kind))
        .context_bad_request("Bad Request", "Unknown addon kind")?;

    let addon_id = Uuid::new_v4();
    let kind = preset
        .from_cfg(addon_id, &payload.preset.config)
        .context_bad_request("Bad Request", "Invalid addon configuration")?;

    // Default resources/types to the live available set (e.g. upstream manifest for
    // Stremio), falling back to the preset's static metadata if unavailable.
    let metadata = preset.metadata();
    let avail_resources = kind.available_resources().await;
    let avail_types = kind.available_types().await;

    let resources = if payload.resources.is_empty() {
        if !avail_resources.is_empty() {
            avail_resources
        } else {
            metadata.supported_resources
        }
    } else {
        payload.resources
    };
    let types: Vec<DbMediaKind> = if payload.types.is_empty() {
        if !avail_types.is_empty() {
            avail_types
                .into_iter()
                .filter_map(|t| DbMediaKind::try_from(t).ok())
                .collect()
        } else {
            metadata
                .supported_types
                .into_iter()
                .map(DbMediaKind::from)
                .collect()
        }
    } else {
        payload.types.into_iter().map(DbMediaKind::from).collect()
    };

    let now = Utc::now().naive_utc();
    let mut addon = Addon {
        id: addon_id,
        preset: payload.preset,
        name: payload.name,
        resources,
        types,
        enabled: true,
        priority: payload.priority,
        created_at: now,
        updated_at: now,
    };

    addon.insert(&state.ctx.db).await?;
    state.ctx.addons.reload(&state.ctx.db).await?;
    Ok((StatusCode::CREATED, Json(addon_to_dto(addon).await)))
}

/// Update an existing addon instance. Any field omitted is left unchanged.
#[post("/addons/{id}")]
pub async fn update_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateAddonRequest>,
) -> Result<Json<AddonDto>> {
    let mut addon = Addon::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;

    if let Some(name) = payload.name {
        addon.name = name;
    }
    if let Some(config) = payload.config {
        addon.preset.config = config;
    }
    if let Some(resources) = payload.resources {
        addon.resources = resources;
    }
    if let Some(types) = payload.types {
        addon.types = types.into_iter().map(DbMediaKind::from).collect();
    }
    if let Some(enabled) = payload.enabled {
        addon.enabled = enabled;
    }
    if let Some(priority) = payload.priority {
        addon.priority = priority;
    }
    addon.updated_at = Utc::now().naive_utc();

    let presets = registered_presets();
    let preset = presets
        .iter()
        .find(|p| p.id() == addon.preset.kind)
        .ok_or_else(|| anyhow::anyhow!("unknown addon kind: {}", addon.preset.kind))
        .context_bad_request("Bad Request", "Unknown addon kind")?;
    preset
        .from_cfg(addon.id, &addon.preset.config)
        .context_bad_request("Bad Request", "Invalid addon configuration")?;

    addon.update(&state.ctx.db).await?;
    state.ctx.addons.reload(&state.ctx.db).await?;
    Ok(Json(addon_to_dto(addon).await))
}

/// Delete an addon instance.
#[delete("/addons/{id}")]
pub async fn delete_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let addon_row = Addon::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;

    // Purge the addon's index (removes e.g. IPTV channels) before deleting.
    if let Some(runtime) = state.ctx.addons.get(id).await {
        if let Err(e) = runtime.kind.purge_index(&state.ctx, &addon_row).await {
            tracing::warn!(addon = %id, error = %e, "purge_index failed on addon delete");
        }
    }

    Addon::delete(&state.ctx.db, id).await?;
    state.ctx.addons.reload(&state.ctx.db).await?;

    // Remove catalog memberships for this addon so items are no longer
    // associated with catalogs that no longer exist.
    if let Err(e) = sqlx::query("DELETE FROM media_catalog_items WHERE addon_id = ?")
        .bind(id.to_string())
        .execute(&state.ctx.db)
        .await
    {
        tracing::warn!(addon = %id, error = %e, "failed to clean up catalog memberships on addon delete");
    }

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
    let addon_row = Addon::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;

    let catalog = state
        .ctx
        .addons
        .get_catalog(id)
        .await
        .ok_or_else(|| anyhow::anyhow!("addon not instantiated"))
        .context_bad_request("Bad Request", "Addon could not be instantiated")?;

    let available = catalog
        .catalog_list(&state.ctx)
        .await
        .context_internal("Catalog Error", "Failed to list addon catalogs")?;

    let states = addon_row.catalog_states();
    let prefix = format!("addon:{id}:");

    let result = available
        .iter()
        .map(|cat_info| {
            let full_id = make_media_id(id, &cat_info.provider_catalog_id);
            let local_id = full_id.strip_prefix(&prefix).unwrap_or(&full_id);
            let state_entry = states.get(local_id).cloned().unwrap_or_else(|| {
                crate::addons::CatalogState {
                    enabled: cat_info.default_enabled,
                    max_items: cat_info.default_max_items,
                }
            });
            AddonCatalogDto {
                catalog_id: full_id,
                name: cat_info.name.clone(),
                enabled: state_entry.enabled,
                max_items: state_entry.max_items,
            }
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
    let mut addon = Addon::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;

    let prefix = format!("addon:{id}:");
    let mut states = addon.catalog_states();

    for req in &payload {
        let local_id = req
            .catalog_id
            .strip_prefix(&prefix)
            .unwrap_or(&req.catalog_id)
            .to_string();
        states.insert(
            local_id,
            crate::addons::CatalogState {
                enabled: req.enabled,
                max_items: req.max_items,
            },
        );
    }

    addon.set_catalog_states(states);
    addon.update(&state.ctx.db).await?;
    state.ctx.addons.reload(&state.ctx.db).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::integration_test::{auth_header_with_token, authenticated_server};
    use http::header::HeaderValue;
    use serde_json::json;

    fn auth(token: &str) -> (http::header::HeaderName, HeaderValue) {
        (
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&auth_header_with_token(token)).unwrap(),
        )
    }

    #[tokio::test]
    async fn list_addon_kinds_includes_stremio() {
        let (server, _ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        let resp = server.get("/addon-kinds").add_header(h, v).await;
        resp.assert_status_ok();

        let kinds: Vec<AddonMetadata> = resp.json();
        assert!(
            kinds.iter().any(|k| k.id == "stremio"),
            "stremio kind should be registered"
        );
        let stremio = kinds.iter().find(|k| k.id == "stremio").unwrap();
        assert_eq!(stremio.options.len(), 1);
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
            ctx.0.addons.get(created.id).await.is_some(),
            "registry did not pick up the new addon"
        );

        // List shows exactly one more than baseline.
        let list_resp = server.get("/addons").add_header(h.clone(), v.clone()).await;
        list_resp.assert_status_ok();
        let list: Vec<AddonDto> = list_resp.json();
        assert_eq!(list.len(), initial_count + 1);
        assert!(list.iter().any(|a| a.id == created.id));

        // Delete.
        let del_resp = server
            .delete(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .await;
        del_resp.assert_status(http::StatusCode::NO_CONTENT);

        let list_after: Vec<AddonDto> =
            server.get("/addons").add_header(h, v).await.json();
        assert_eq!(
            list_after.len(),
            initial_count,
            "addon should be gone after delete"
        );
        assert!(
            ctx.0.addons.get(created.id).await.is_none(),
            "registry should have dropped the deleted addon"
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
}
