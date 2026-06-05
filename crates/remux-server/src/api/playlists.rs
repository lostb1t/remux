use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{api_query, delete, get, post};
use remux_sdks::CommaSeparatedList;
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::common::get_uuid;
use crate::db;
use crate::db::auth;
use crate::{IntoApiError, OptionExt, ResultExt};
use axum_anyhow::ApiResult as Result;

#[api_query]
pub struct CreatePlaylistQuery {
    pub name: Option<String>,
    #[serde(default)]
    pub ids: CommaSeparatedList<Uuid>,
    pub user_id: Option<Uuid>,
}

#[post("/playlists")]
pub async fn create_playlist(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<CreatePlaylistQuery>,
    body: Option<Json<api::CreatePlaylistDto>>,
) -> Result<impl IntoResponse> {
    let body = body.map(|b| b.0).unwrap_or_default();
    let name = q
        .name
        .or(body.name)
        .unwrap_or_else(|| "New Playlist".into());
    let ids: Vec<Uuid> = if !q.ids.is_empty() {
        q.ids.to_vec()
    } else {
        body.ids
    };

    let mut media = db::Media {
        id: get_uuid(),
        title: name,
        kind: db::MediaKind::Playlist,
        ..Default::default()
    };
    media
        .save(&state.ctx.db)
        .await
        .context_bad_request("Failed to create playlist")?;

    if !ids.is_empty() {
        let resolved = crate::services::resolve::resolve_ids(&ids, &state.ctx).await;
        if !resolved.is_empty() {
            db::MediaRelation::add_playlist_items(&state.ctx.db, &media.id, &resolved)
                .await
                .ok();
        }
    }

    Ok(Json(api::PlaylistCreationResult {
        id: media.id.to_string(),
    }))
}

#[get("/playlists/{id}")]
pub async fn get_playlist(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await
        .context_bad_request("DB error")?
        .filter(|m| m.kind == db::MediaKind::Playlist)
        .context_not_found("Playlist not found")?;

    let rels = db::MediaRelation::get_playlist_items(&state.ctx.db, &media.id).await?;
    let item_ids: Vec<Uuid> = rels.iter().map(|r| r.right_media_id).collect();

    Ok(Json(serde_json::json!({
        "OpenAccess": true,
        "Shares": [],
        "ItemIds": item_ids,
    })))
}

#[post("/playlists/{id}")]
pub async fn update_playlist(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Json(body): Json<api::UpdatePlaylistDto>,
) -> Result<impl IntoResponse> {
    let mut media = db::Media::get_by_id(&state.ctx.db, &id)
        .await
        .context_bad_request("DB error")?
        .filter(|m| m.kind == db::MediaKind::Playlist)
        .context_not_found("Playlist not found")?;

    if let Some(name) = body.name {
        media.title = name;
        media.save(&state.ctx.db).await?;
    }

    if let Some(ids) = body.ids {
        sqlx::query(
            "DELETE FROM media_relations WHERE left_media_id = ? AND role = 'playlist'",
        )
        .bind(media.id)
        .execute(&state.ctx.db)
        .await?;
        db::MediaRelation::add_playlist_items(&state.ctx.db, &media.id, &ids).await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[api_query]
#[derive(Default)]
pub struct PlaylistItemsQuery {
    pub start_index: Option<u32>,
    pub limit: Option<u32>,
}

#[get("/playlists/{id}/items")]
pub async fn get_playlist_items(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<PlaylistItemsQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await
        .context_bad_request("DB error")?
        .filter(|m| m.kind == db::MediaKind::Playlist)
        .context_not_found("Playlist not found")?;

    let relations = db::MediaRelation::get_playlist_items(&state.ctx.db, &id).await?;
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
    }))
}

#[api_query]
pub struct AddItemsQuery {
    #[serde(default)]
    pub ids: CommaSeparatedList<Uuid>,
}

#[post("/playlists/{id}/items")]
pub async fn add_playlist_items(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<AddItemsQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await
        .context_bad_request("DB error")?
        .filter(|m| m.kind == db::MediaKind::Playlist)
        .context_not_found("Playlist not found")?;

    let resolved = crate::services::resolve::resolve_ids(&q.ids, &state.ctx).await;
    db::MediaRelation::add_playlist_items(&state.ctx.db, &id, &resolved).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[api_query]
pub struct RemoveItemsQuery {
    #[serde(default)]
    pub entry_ids: CommaSeparatedList<Uuid>,
}

/// GET /Playlists/{id}/Users/{userId}
/// Returns edit permissions for the given user on this playlist.
/// remux grants CanEdit to every authenticated user for now.
#[get("/playlists/{id}/users/{user_id}")]
pub async fn get_playlist_user(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .filter(|m| m.kind == db::MediaKind::Playlist)
        .context_not_found("Playlist not found")?;

    Ok(Json(serde_json::json!({
        "UserId": user_id.to_string(),
        "CanEdit": true
    })))
}

#[delete("/playlists/{id}/items")]
pub async fn remove_playlist_items(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<RemoveItemsQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await
        .context_bad_request("DB error")?
        .filter(|m| m.kind == db::MediaKind::Playlist)
        .context_not_found("Playlist not found")?;

    db::MediaRelation::delete_by_relation_ids(&state.ctx.db, &q.entry_ids).await?;
    db::sync_playlist_media_kind(&state.ctx.db, &id).await;

    Ok(StatusCode::NO_CONTENT)
}

#[post("/playlists/{id}/items/{item_id}/move/{new_index}")]
pub async fn move_playlist_item(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((id, item_id, new_index)): Path<(Uuid, Uuid, usize)>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(&state.ctx.db, &id)
        .await
        .context_bad_request("DB error")?
        .filter(|m| m.kind == db::MediaKind::Playlist)
        .context_not_found("Playlist not found")?;

    db::MediaRelation::move_playlist_item(&state.ctx.db, &id, &item_id, new_index)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
