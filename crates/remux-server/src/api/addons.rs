use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use tracing::warn;

use remux_macros::{delete, get, post};
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt,
    addons::{
        Addon, AddonCatalogDto, AddonDto, AddonMetadata, CreateAddonRequest,
        UpdateAddonCatalogRequest, UpdateAddonRequest, registered_presets,
    },
    db::{MediaKind as DbMediaKind, auth},
};
use axum_anyhow::ApiResult as Result;
use remux_sdks::remux::MediaKind;

async fn addon_to_dto(addon: Addon, config: &crate::Config) -> AddonDto {
    let preset = registered_presets()
        .into_iter()
        .find(|p| {
            p.id()
                == addon
                    .preset
                    .kind
        });

    let (supported_resources, supported_types) = if let Some(ref p) = preset {
        let meta = p.metadata();
        match p.from_cfg(
            addon.id,
            &addon
                .preset
                .config,
            config,
        ) {
            Ok(caps) => {
                let kind = caps
                    .kind
                    .as_ref()
                    .map(|k| k.as_ref());
                let info = if let Some(k) = kind {
                    k.available_info()
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };
                match info {
                    Some((resources, raw_types, _, _, _)) => {
                        let types = raw_types
                            .into_iter()
                            .filter_map(|t| {
                                DbMediaKind::try_from(t)
                                    .ok()
                                    .map(Into::into)
                            })
                            .collect();
                        (resources, types)
                    }
                    None => (meta.supported_resources, meta.supported_types),
                }
            }
            Err(_) => (meta.supported_resources, meta.supported_types),
        }
    } else {
        (vec![], vec![])
    };

    AddonDto {
        id: addon.id,
        kind: addon
            .preset
            .kind,
        name: addon.name,
        config: addon
            .preset
            .config,
        resources: addon.resources,
        types: addon
            .types
            .iter()
            .cloned()
            .map(Into::into)
            .collect(),
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
    let dtos = futures::future::join_all(
        addons
            .into_iter()
            .map(|a| {
                addon_to_dto(
                    a,
                    &state
                        .ctx
                        .config,
                )
            }),
    )
    .await;
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
    Ok(Json(
        addon_to_dto(
            addon,
            &state
                .ctx
                .config,
        )
        .await,
    ))
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
                .config,
            &state
                .ctx
                .config,
        )
        .context_bad_request("Invalid addon configuration")?;
    payload
        .preset
        .config = normalized_config;
    let caps = preset
        .from_cfg(
            addon_id,
            &payload
                .preset
                .config,
            &state
                .ctx
                .config,
        )
        .context_bad_request("Invalid addon configuration")?;
    let kind_ref = caps
        .kind
        .as_deref();

    let metadata = preset.metadata();
    let avail_info =
        if let Some(k) = kind_ref {
            k.available_info().await.context_bad_request(
            "Could not fetch addon capabilities — is the manifest URL reachable?",
        )?
        } else {
            None
        };

    let resources = if payload
        .resources
        .is_empty()
    {
        match &avail_info {
            Some((r, _, _, _, _)) => r.clone(),
            None => metadata.supported_resources,
        }
    } else {
        payload.resources
    };
    let types: Vec<DbMediaKind> = if payload
        .types
        .is_empty()
    {
        match avail_info {
            Some((_, t, _, _, _)) => t
                .into_iter()
                .filter_map(|t| DbMediaKind::try_from(t).ok())
                .collect(),
            None => metadata
                .supported_types
                .into_iter()
                .map(DbMediaKind::from)
                .collect(),
        }
    } else {
        payload
            .types
            .into_iter()
            .map(DbMediaKind::from)
            .collect()
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

    addon
        .insert(
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
    Ok((
        StatusCode::CREATED,
        Json(
            addon_to_dto(
                addon,
                &state
                    .ctx
                    .config,
            )
            .await,
        ),
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
    let mut addon = Addon::get(
        &state
            .ctx
            .db,
        id,
    )
    .await?
    .context_not_found("Addon not found")?;

    if let Some(name) = payload.name {
        addon.name = name;
    }
    if let Some(resources) = payload.resources {
        addon.resources = resources;
    }
    if let Some(types) = payload.types {
        addon.types = types
            .into_iter()
            .map(DbMediaKind::from)
            .collect();
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
    if let Some(config) = payload.config {
        addon
            .preset
            .config = preset
            .normalize_cfg(
                config,
                &state
                    .ctx
                    .config,
            )
            .context_bad_request("Invalid addon configuration")?;
    }
    preset
        .from_cfg(
            addon.id,
            &addon
                .preset
                .config,
            &state
                .ctx
                .config,
        )
        .context_bad_request("Invalid addon configuration")?;

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
    Ok(Json(
        addon_to_dto(
            addon,
            &state
                .ctx
                .config,
        )
        .await,
    ))
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
        .context_internal("Failed to list addon catalogs")?;

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
