use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{delete, get, post, query};
use remux_sdks::CommaSeparatedList;
use remux_sdks::remux::deserialize_separated_str;
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt, api, common::get_uuid, db, db::auth,
};
use axum_anyhow::ApiResult as Result;
use tracing::warn;

/// Restrict playlist membership to directly-playable leaf items: tracks,
/// movies, episodes and TV channels. Containers (albums, artists, series,
/// seasons, collections, ...) have no playable content of their own and would
/// otherwise be stored as empty playlist items.
async fn retain_playlist_items(
    db: &sqlx::SqlitePool,
    resolved: Vec<Uuid>,
) -> Result<Vec<Uuid>> {
    if resolved.is_empty() {
        return Ok(resolved);
    }
    let by_kind: std::collections::HashMap<Uuid, db::MediaKind> =
        db::Media::get_by_ids(db, &resolved)
            .await?
            .into_iter()
            .map(|m| (m.id, m.kind))
            .collect();
    Ok(resolved
        .into_iter()
        .filter(|id| match by_kind.get(id) {
            Some(kind) if kind.is_playable_leaf() => true,
            Some(kind) => {
                warn!(?id, ?kind, "rejecting non-playable media in playlist");
                false
            }
            None => {
                warn!(?id, "playlist add: media not found, dropping");
                false
            }
        })
        .collect())
}

#[query]
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
    let body = body
        .map(|b| b.0)
        .unwrap_or_default();
    let name = q
        .name
        .or(body.name)
        .unwrap_or_else(|| "New Playlist".into());
    let ids: Vec<Uuid> = if !q
        .ids
        .is_empty()
    {
        q.ids
            .to_vec()
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
        .save(
            &state
                .ctx
                .db,
        )
        .await
        .context_bad_request("Failed to create playlist")?;

    if !ids.is_empty() {
        let resolved =
            crate::services::MediaResolveService::resolve_ids(&ids, &state.ctx).await;
        let resolved = retain_playlist_items(
            &state
                .ctx
                .db,
            resolved,
        )
        .await?;
        if !resolved.is_empty() {
            db::MediaRelation::add_playlist_items(
                &state
                    .ctx
                    .db,
                &media.id,
                &resolved,
            )
            .await
            .ok();
        }
    }

    Ok(Json(api::PlaylistCreationResult {
        id: media
            .id
            .to_string(),
    }))
}

#[get("/playlists/{id}")]
pub async fn get_playlist(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await
    .context_bad_request("DB error")?
    .filter(|m| m.kind == db::MediaKind::Playlist)
    .context_not_found("Playlist not found")?;

    let rels = db::MediaRelation::get_playlist_items(
        &state
            .ctx
            .db,
        &media.id,
    )
    .await?;
    let item_ids: Vec<Uuid> = rels
        .iter()
        .map(|r| r.right_media_id)
        .collect();

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
    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await
    .context_bad_request("DB error")?
    .filter(|m| m.kind == db::MediaKind::Playlist)
    .context_not_found("Playlist not found")?;

    if let Some(name) = body.name {
        media.title = name;
        media
            .save(
                &state
                    .ctx
                    .db,
            )
            .await?;
    }

    if let Some(ids) = body.ids {
        sqlx::query(
            "DELETE FROM media_relations WHERE left_media_id = ? AND role = 'playlist'",
        )
        .bind(media.id)
        .execute(
            &state
                .ctx
                .db,
        )
        .await?;
        db::MediaRelation::add_playlist_items(
            &state
                .ctx
                .db,
            &media.id,
            &ids,
        )
        .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[query]
#[derive(Default)]
pub struct PlaylistItemsQuery {
    pub start_index: Option<u32>,
    pub limit: Option<u32>,
    /// Jellyfin `IncludeItemTypes` filter, applied before pagination.
    #[serde(default, deserialize_with = "deserialize_separated_str")]
    pub include_item_types: Option<Vec<api::MediaType>>,
}

#[get("/playlists/{id}/items")]
pub async fn get_playlist_items(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<PlaylistItemsQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await
    .context_bad_request("DB error")?
    .filter(|m| m.kind == db::MediaKind::Playlist)
    .context_not_found("Playlist not found")?;

    let relations = db::MediaRelation::get_playlist_items(
        &state
            .ctx
            .db,
        &id,
    )
    .await?;

    let item_ids: Vec<Uuid> = relations
        .iter()
        .map(|r| r.right_media_id)
        .collect();
    let mut by_id: std::collections::HashMap<Uuid, db::Media> = if item_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        db::Media::get_by_ids(
            &state
                .ctx
                .db,
            &item_ids,
        )
        .await?
        .into_iter()
        .map(|m| (m.id, m))
        .collect()
    };

    let mut ordered: Vec<(Uuid, db::Media)> = relations
        .into_iter()
        .filter_map(|rel| {
            by_id
                .remove(&rel.right_media_id)
                .map(|m| (rel.relation_id, m))
        })
        .collect();

    if let Some(types) = &q.include_item_types {
        let kinds: Vec<db::MediaKind> = types
            .iter()
            .filter_map(|t| db::MediaKind::try_from(t.clone()).ok())
            .collect();
        ordered.retain(|(_, m)| kinds.contains(&m.kind));
    }

    let total = ordered.len() as i64;

    let start = q
        .start_index
        .unwrap_or(0) as usize;
    let start = start.min(ordered.len());
    let slice = match q.limit {
        Some(limit) => &ordered[start..][..(limit as usize).min(ordered.len() - start)],
        None => &ordered[start..],
    };

    let items: Vec<api::BaseItemDto> = slice
        .iter()
        .map(|(relation_id, media)| {
            let mut dto = api::db_media_to_item(media.clone(), false);
            dto.playlist_item_id = Some(relation_id.to_string());
            dto
        })
        .collect();

    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        start_index: q
            .start_index
            .unwrap_or(0),
    }))
}

#[query]
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
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await
    .context_bad_request("DB error")?
    .filter(|m| m.kind == db::MediaKind::Playlist)
    .context_not_found("Playlist not found")?;

    let resolved =
        crate::services::MediaResolveService::resolve_ids(&q.ids, &state.ctx).await;
    let resolved = retain_playlist_items(
        &state
            .ctx
            .db,
        resolved,
    )
    .await?;
    db::MediaRelation::add_playlist_items(
        &state
            .ctx
            .db,
        &id,
        &resolved,
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[query]
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
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
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
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await
    .context_bad_request("DB error")?
    .filter(|m| m.kind == db::MediaKind::Playlist)
    .context_not_found("Playlist not found")?;

    db::MediaRelation::delete_by_relation_ids(
        &state
            .ctx
            .db,
        &q.entry_ids,
    )
    .await?;
    db::sync_playlist_media_kind(
        &state
            .ctx
            .db,
        &id,
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

#[post("/playlists/{id}/items/{item_id}/move/{new_index}")]
pub async fn move_playlist_item(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((id, item_id, new_index)): Path<(Uuid, Uuid, usize)>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await
    .context_bad_request("DB error")?
    .filter(|m| m.kind == db::MediaKind::Playlist)
    .context_not_found("Playlist not found")?;

    db::MediaRelation::move_playlist_item(
        &state
            .ctx
            .db,
        &id,
        &item_id,
        new_index,
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
