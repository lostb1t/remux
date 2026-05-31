use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};
use axum_extra::extract::Query;
use futures::StreamExt;
use http::StatusCode;
use remux_macros::{delete, get, post};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::db;
use crate::db::auth::AdminSession;

// ---------------------------------------------------------------------------
// GET /collections/{id}/items
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionItemsQuery {
    pub start_index: Option<u32>,
    pub limit: Option<u32>,
}

#[get("/collections/{id}/items")]
pub async fn get_collection_items(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<Uuid>,
    Query(q): Query<CollectionItemsQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .filter(|m| m.kind == db::MediaKind::Collection)
        .context_not_found("Not Found", "Collection not found")?;

    let relations = db::MediaRelation::get_collection_items(&state.ctx.db, &id).await?;
    let total = relations.len() as i64;
    let start = q.start_index.unwrap_or(0) as usize;
    let remaining = relations.len().saturating_sub(start);
    let slice = match q.limit {
        Some(limit) => {
            &relations[start.min(relations.len())..][..(limit as usize).min(remaining)]
        }
        None => &relations[start.min(relations.len())..],
    };

    let mut items = Vec::with_capacity(slice.len());
    for rel in slice {
        if let Some(media) =
            db::Media::get_by_id(&state.ctx.db, &rel.right_media_id).await?
        {
            let mut dto = api::db_media_to_item(media);
            dto.playlist_item_id = Some(rel.relation_id.to_string());
            items.push(dto);
        }
    }

    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        start_index: q.start_index.unwrap_or(0),
        ..Default::default()
    }))
}

// ---------------------------------------------------------------------------
// POST /collections/{id}/items  (add items by id list)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AddCollectionItemsQuery {
    pub ids: Option<String>,
}

#[post("/collections/{id}/items")]
pub async fn add_collection_items(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<Uuid>,
    Query(q): Query<AddCollectionItemsQuery>,
) -> Result<StatusCode> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .filter(|m| m.kind == db::MediaKind::Collection)
        .context_not_found("Not Found", "Collection not found")?;

    let media_ids: Vec<Uuid> = q
        .ids
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| Uuid::parse_str(s.trim()).ok())
        .collect();

    db::MediaRelation::add_collection_items(&state.ctx.db, &id, &media_ids)
        .await
        .context_bad_request("collections", "failed to add items")?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// DELETE /collections/{id}/items  (?ids=relation_id,...)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoveCollectionItemsQuery {
    pub ids: Option<String>,
}

#[delete("/collections/{id}/items")]
pub async fn remove_collection_items(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<Uuid>,
    Query(q): Query<RemoveCollectionItemsQuery>,
) -> Result<StatusCode> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .filter(|m| m.kind == db::MediaKind::Collection)
        .context_not_found("Not Found", "Collection not found")?;

    let relation_ids: Vec<Uuid> = q
        .ids
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| Uuid::parse_str(s.trim()).ok())
        .collect();

    db::MediaRelation::delete_by_relation_ids(&state.ctx.db, &relation_ids)
        .await
        .context_bad_request("collections", "failed to remove items")?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /collections/{id}/items/{item_id}/move/{new_index}
// ---------------------------------------------------------------------------

#[post("/collections/{id}/items/{item_id}/move/{new_index}")]
pub async fn move_collection_item(
    State(state): State<AppState>,
    _session: AdminSession,
    Path((id, item_id, new_index)): Path<(Uuid, Uuid, usize)>,
) -> Result<StatusCode> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .filter(|m| m.kind == db::MediaKind::Collection)
        .context_not_found("Not Found", "Collection not found")?;

    db::MediaRelation::move_collection_item(&state.ctx.db, &id, &item_id, new_index)
        .await
        .context_bad_request("collections", "failed to move item")?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /remux/collections/{id}/importcatalog
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ImportCatalogBody {
    pub addon_id: Uuid,
    pub catalog_id: String,
}

#[post("/remux/collections/{id}/importcatalog")]
pub async fn import_catalog(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<Uuid>,
    Json(body): Json<ImportCatalogBody>,
) -> Result<StatusCode> {
    let mut collection = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .filter(|m| m.kind == db::MediaKind::Collection)
        .context_not_found("Not Found", "Collection not found")?;

    let addon = state
        .ctx
        .addons
        .get_catalog(body.addon_id)
        .await
        .context_not_found("Not Found", "Addon not found or has no catalog")?;

    let stream = addon
        .catalog_stream(&state.ctx, &body.catalog_id)
        .await
        .context_bad_request("collections", "addon catalog_stream failed")?
        .context_not_found("Not Found", "Catalog not found in addon")?;

    let mut items: Vec<db::Media> = Vec::new();
    let mut stream = stream;
    while let Some(item) = stream.next().await {
        items.push(item);
    }
    let media_ids: Vec<Uuid> = items.iter().map(|m| m.id).collect();

    // Upsert the items so they exist in the DB.
    if !items.is_empty() {
        db::Media::upsert(&state.ctx.db, &items).await?;
    }

    db::MediaRelation::replace_collection_items(&state.ctx.db, &id, &media_ids)
        .await
        .context_bad_request("collections", "failed to replace collection items")?;

    // Ensure collection_kind is Manual.
    if collection.collection_kind != Some(db::CollectionKind::Manual) {
        collection.collection_kind = Some(db::CollectionKind::Manual);
        collection
            .save(&state.ctx.db)
            .await
            .context_bad_request("collections", "failed to update collection kind")?;
    }

    Ok(StatusCode::NO_CONTENT)
}
