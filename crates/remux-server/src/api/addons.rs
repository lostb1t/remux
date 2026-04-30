use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use remux_macros::{delete, get, post};
use uuid::Uuid;

use crate::AppState;
use crate::addons::stremio::{init_resources_from_manifest, manifest_info_for_row};
use crate::addons::{
    AddonCatalogDto, AddonDto, AddonKindMetadata, AddonRow, CreateAddonRequest,
    UpdateAddonCatalogRequest, UpdateAddonRequest, make_media_id, registered_kinds,
};
use crate::db::auth;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};

async fn row_to_dto(row: AddonRow) -> AddonDto {
    let kind_meta = registered_kinds()
        .into_iter()
        .find(|k| k.id() == row.kind)
        .map(|k| k.metadata());

    // For Stremio addons, fetch resources/types directly from the manifest
    // (the HTTP layer already caches the response). For other kinds, use the
    // static metadata from the kind definition.
    let (supported_resources, supported_types) = if row.kind == "stremio" {
        manifest_info_for_row(&row).await.unwrap_or_else(|| {
            let meta = kind_meta.as_ref();
            (
                meta.map(|m| m.supported_resources.clone())
                    .unwrap_or_default(),
                meta.map(|m| m.supported_types.clone()).unwrap_or_default(),
            )
        })
    } else {
        let meta = kind_meta.as_ref();
        (
            meta.map(|m| m.supported_resources.clone())
                .unwrap_or_default(),
            meta.map(|m| m.supported_types.clone()).unwrap_or_default(),
        )
    };

    AddonDto {
        id: row.id,
        kind: row.kind,
        name: row.name,
        config: row.config,
        resources: row.resources,
        supported_resources,
        supported_types,
        priority: row.priority,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// List metadata for every registered addon kind. Drives the dashboard's
/// "add addon" picker and form renderer.
#[get("/addon-kinds")]
pub async fn list_addon_kinds(
    State(_state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<Json<Vec<AddonKindMetadata>>> {
    Ok(Json(
        registered_kinds().iter().map(|k| k.metadata()).collect(),
    ))
}

/// List all configured addon instances.
#[get("/addons")]
pub async fn list_addons(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<Json<Vec<AddonDto>>> {
    let rows = AddonRow::list(&state.ctx.db).await?;
    let mut dtos = Vec::with_capacity(rows.len());
    for row in rows {
        dtos.push(row_to_dto(row).await);
    }
    Ok(Json(dtos))
}

/// Get a single addon instance by ID.
#[get("/addons/{id}")]
pub async fn get_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<Json<AddonDto>> {
    let row = AddonRow::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;
    Ok(Json(row_to_dto(row).await))
}

/// Create a new addon instance.
#[post("/addons")]
pub async fn create_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(payload): Json<CreateAddonRequest>,
) -> Result<(StatusCode, Json<AddonDto>)> {
    let kinds = registered_kinds();
    let kind = kinds
        .iter()
        .find(|k| k.id() == payload.kind)
        .ok_or_else(|| anyhow::anyhow!("unknown addon kind: {}", payload.kind))
        .context_bad_request("Bad Request", "Unknown addon kind")?;

    // Default resources to the full set the kind supports if the caller didn't specify.
    let metadata = kind.metadata();
    let resources = if payload.resources.is_empty() {
        metadata.supported_resources.clone()
    } else {
        payload.resources
    };

    let now = Utc::now();
    let row = AddonRow {
        id: Uuid::new_v4(),
        kind: payload.kind,
        name: payload.name,
        config: payload.config,
        resources,
        priority: payload.priority,
        created_at: now,
        updated_at: now,
    };

    // Validate the config by attempting to instantiate before persisting.
    kind.instantiate(&row)
        .context_bad_request("Bad Request", "Invalid addon configuration")?;

    let mut row = row;
    row.insert(&state.ctx.db).await?;

    // For Stremio addons, fetch the manifest and use its resources as the
    // initial active resource set.
    if row.kind == "stremio" {
        init_resources_from_manifest(&state.ctx.db, &mut row).await;
    }

    state.ctx.addons.reload(&state.ctx.db).await?;
    Ok((StatusCode::CREATED, Json(row_to_dto(row).await)))
}

/// Update an existing addon instance. Any field omitted is left unchanged.
#[post("/addons/{id}")]
pub async fn update_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateAddonRequest>,
) -> Result<Json<AddonDto>> {
    let mut row = AddonRow::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;

    if let Some(name) = payload.name {
        row.name = name;
    }
    let config_changed = payload.config.is_some();
    if let Some(config) = payload.config {
        row.config = config;
    }
    if let Some(resources) = payload.resources {
        row.resources = resources;
    }
    if let Some(priority) = payload.priority {
        row.priority = priority;
    }
    row.updated_at = Utc::now();

    let kinds = registered_kinds();
    let kind = kinds
        .iter()
        .find(|k| k.id() == row.kind)
        .ok_or_else(|| anyhow::anyhow!("unknown addon kind: {}", row.kind))
        .context_bad_request("Bad Request", "Unknown addon kind")?;
    kind.instantiate(&row)
        .context_bad_request("Bad Request", "Invalid addon configuration")?;

    // Re-fetch manifest resources when the config changes (manifest URL may
    // have changed), so active resources reflect the new manifest.
    if row.kind == "stremio" && config_changed {
        init_resources_from_manifest(&state.ctx.db, &mut row).await;
    } else {
        row.update(&state.ctx.db).await?;
    }
    state.ctx.addons.reload(&state.ctx.db).await?;
    Ok(Json(row_to_dto(row).await))
}

/// Delete an addon instance.
#[delete("/addons/{id}")]
pub async fn delete_addon(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    AddonRow::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;
    AddonRow::delete(&state.ctx.db, id).await?;
    state.ctx.addons.reload(&state.ctx.db).await?;

    // Remove all catalog membership tags for this addon so items are no longer
    // associated with catalogs that no longer exist.
    let tag_prefix = format!("catalog:{id}:%");
    if let Err(e) = sqlx::query("DELETE FROM media_tags WHERE tag LIKE ?")
        .bind(&tag_prefix)
        .execute(&state.ctx.db)
        .await
    {
        tracing::warn!(addon = %id, error = %e, "failed to clean up catalog tags on addon delete");
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
    let row = AddonRow::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;

    let addon = state
        .ctx
        .addons
        .get_catalog(id)
        .await
        .ok_or_else(|| anyhow::anyhow!("addon not instantiated"))
        .context_bad_request("Bad Request", "Addon could not be instantiated")?;

    let available = addon
        .list(&state.ctx)
        .await
        .context_internal("Catalog Error", "Failed to list addon catalogs")?;

    let states = row.catalog_states();
    let prefix = format!("addon:{id}:");

    let result = available
        .iter()
        .map(|cat_info| {
            let full_id = make_media_id(id, &cat_info.provider_catalog_id);
            let local_id = full_id.strip_prefix(&prefix).unwrap_or(&full_id);
            let state_entry = states.get(local_id).cloned().unwrap_or_default();
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
    let mut row = AddonRow::get(&state.ctx.db, id)
        .await?
        .context_not_found("Not Found", "Addon not found")?;

    let prefix = format!("addon:{id}:");
    let mut states = row.catalog_states();

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

    row.set_catalog_states(states);
    row.update(&state.ctx.db).await?;

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

        let kinds: Vec<AddonKindMetadata> = resp.json();
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

        let create_resp = server
            .post("/addons")
            .add_header(h.clone(), v.clone())
            .json(&json!({
                "kind": "stremio",
                "name": "Test Cinemeta",
                "config": { "manifest_url": "https://v3-cinemeta.strem.io/manifest.json" },
                "resources": ["catalog"],
            }))
            .await;
        create_resp.assert_status(http::StatusCode::CREATED);

        let created: AddonDto = create_resp.json();
        assert_eq!(created.kind, "stremio");
        assert_eq!(created.name, "Test Cinemeta");

        // Registry should reflect the new addon immediately.
        assert!(
            ctx.addons.get(created.id).await.is_some(),
            "registry did not pick up the new addon"
        );

        // List shows it.
        let list_resp = server.get("/addons").add_header(h.clone(), v.clone()).await;
        list_resp.assert_status_ok();
        let list: Vec<AddonDto> = list_resp.json();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, created.id);

        // Delete.
        let del_resp = server
            .delete(&format!("/addons/{}", created.id))
            .add_header(h.clone(), v.clone())
            .await;
        del_resp.assert_status(http::StatusCode::NO_CONTENT);

        let list_after: Vec<AddonDto> =
            server.get("/addons").add_header(h, v).await.json();
        assert!(list_after.is_empty(), "addon should be gone after delete");
        assert!(
            ctx.addons.get(created.id).await.is_none(),
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
                "kind": "no-such-kind",
                "name": "Bad",
                "config": {},
            }))
            .await;
        resp.assert_status(http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_addon_rejects_missing_required_config() {
        let (server, _ctx, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        // Stremio requires manifest_url; omitting it should fail validation
        // because instantiate() returns Err.
        let resp = server
            .post("/addons")
            .add_header(h, v)
            .expect_failure()
            .json(&json!({
                "kind": "stremio",
                "name": "Missing URL",
                "config": {},
            }))
            .await;
        resp.assert_status(http::StatusCode::BAD_REQUEST);
    }
}
