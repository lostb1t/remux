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
        Addon, AddonCapabilities, AddonCatalogDto, AddonDto, AddonMetadata,
        AddonPreset, AddonService, CreateAddonRequest, UpdateAddonCatalogRequest,
        UpdateAddonRequest, registered_presets, set_user_addon_override,
        user_addon_override,
    },
    db::{MediaKind as DbMediaKind, auth},
};
use axum_anyhow::ApiResult as Result;
use remux_sdks::remux::MediaKind;

type CapabilitySnapshot = (Vec<remux_sdks::stremio::ResourceType>, Vec<DbMediaKind>);

fn preset_capability_snapshot(preset: &dyn AddonPreset) -> CapabilitySnapshot {
    let metadata = preset.metadata();
    (
        metadata
            .supported_resources
            .into_iter()
            .map(|resource| resource.name)
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

    let (
        supported_resources,
        supported_types,
        supported_resources_user,
        supported_types_user,
    ) = if let Some(ref p) = preset {
        // Runtime metadata was resolved when the addon was loaded. Listing
        // addons must not make another remote manifest request: one stalled
        // provider would otherwise hold the entire dashboard response open.
        let meta = addons
            .get(addon.id)
            .map(|runtime| {
                runtime
                    .caps
                    .metadata
            })
            .unwrap_or_else(|| p.metadata());
        let resources_user = meta
            .supported_resources_user
            .clone();
        let types_user = meta
            .supported_types_user
            .clone();
        (
            meta.supported_resources
                .into_iter()
                .map(|r| r.name)
                .collect(),
            meta.supported_types,
            resources_user,
            types_user,
        )
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
            .iter()
            .cloned()
            .map(Into::into)
            .collect(),
        enabled: addon.enabled,
        supported_resources,
        supported_types,
        supported_resources_user,
        supported_types_user,
        priority: addon.priority,
        system: addon.system,
        is_default: addon.is_default,
        http_redirect_stream: addon.http_redirect_stream,
        service_filter: addon.service_filter,
        description: preset.map(|p| {
            p.metadata()
                .description
        }),
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
    let types: Vec<DbMediaKind> = if payload
        .types
        .is_empty()
    {
        derived_types
    } else {
        payload
            .types
            .into_iter()
            .map(DbMediaKind::from)
            .collect()
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
        service_filter: vec![],
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
        let types: Vec<_> = types
            .iter()
            .cloned()
            .map(DbMediaKind::from)
            .collect();
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
            addon.resources = derived_resources;
        }
        if types.is_none() {
            addon.types = derived_types;
        }
    }

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
    Ok(Json(addon_to_dto(
        addon,
        &state
            .ctx
            .addons,
    )))
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
                .contains(&MediaKind::Series)
        );
        assert!(
            !updated
                .types
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
        assert_eq!(explicit.types, vec![MediaKind::Series]);

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
        assert!(
            explicit_empty
                .types
                .is_empty()
        );
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
            types: vec![],
            enabled: false,
            priority: 0,
            created_at: now,
            updated_at: now,
            system: false,
            is_default: false,
            http_redirect_stream: false,
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
            !addon
                .types
                .is_empty()
        );
    }
}
