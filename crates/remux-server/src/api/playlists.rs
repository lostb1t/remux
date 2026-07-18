use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{delete, get, post, query};
use remux_sdks::CommaSeparatedList;
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt, api, common::get_uuid, db, db::auth,
};
use axum_anyhow::ApiResult as Result;

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
    /// Jellyfin filters playlist contents by item type. Clients (e.g. Finamp)
    /// request `IncludeItemTypes=Audio` when building a play queue and hard-throw
    /// if a non-audio member (a playlist may contain a MusicArtist, MusicAlbum,
    /// etc.) leaks through. Empty means "no type filter".
    #[serde(default)]
    pub include_item_types: CommaSeparatedList<api::MediaType>,
}

pub(super) async fn filter_relations_by_item_types(
    db: &sqlx::SqlitePool,
    relations: Vec<db::MediaRelation>,
    item_types: &[api::MediaType],
) -> Result<Vec<db::MediaRelation>> {
    let allowed_kinds: Vec<db::MediaKind> = item_types
        .iter()
        .filter_map(|item_type| db::MediaKind::try_from(item_type.clone()).ok())
        .collect();
    if allowed_kinds.is_empty() || relations.is_empty() {
        return Ok(relations);
    }

    let mut qb = sqlx::QueryBuilder::new("SELECT id, kind FROM media WHERE id IN (");
    let mut sep = qb.separated(", ");
    for relation in &relations {
        sep.push_bind(relation.right_media_id);
    }
    qb.push(")");
    let kinds: std::collections::HashMap<Uuid, db::MediaKind> = qb
        .build_query_as::<(Uuid, db::MediaKind)>()
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();

    Ok(relations
        .into_iter()
        .filter(|relation| {
            kinds
                .get(&relation.right_media_id)
                .is_some_and(|kind| allowed_kinds.contains(kind))
        })
        .collect())
}

pub(super) async fn relations_to_items(
    db: &sqlx::SqlitePool,
    relations: &[db::MediaRelation],
    hide_sources: bool,
) -> Result<Vec<api::BaseItemDto>> {
    let mut relation_ids = Vec::with_capacity(relations.len());
    let mut media = Vec::with_capacity(relations.len());
    for relation in relations {
        if let Some(item) = db::Media::get_by_id(db, &relation.right_media_id).await? {
            relation_ids.push(relation.relation_id);
            media.push(item);
        }
    }

    db::Media::preload_parents(db, &mut media).await;
    Ok(media
        .into_iter()
        .zip(relation_ids)
        .map(|(item, relation_id)| {
            let mut dto = api::db_media_to_item(item, hide_sources);
            dto.playlist_item_id = Some(relation_id.to_string());
            dto
        })
        .collect())
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

    // Apply IncludeItemTypes to the playlist's contents before paginating, the
    // way Jellyfin does. A playlist may contain non-audio members (a MusicArtist,
    // a MusicAlbum, ...); without this filter a client's typed query (e.g.
    // Finamp's `IncludeItemTypes=Audio` play-queue build) receives the stray
    // member and throws `Wrong BaseItemDto type`. We resolve the members' kinds
    // in one batch query and keep only the requested ones.
    let relations = filter_relations_by_item_types(
        &state
            .ctx
            .db,
        relations,
        &q.include_item_types,
    )
    .await?;

    let total = relations.len() as i64;

    let start = q
        .start_index
        .unwrap_or(0) as usize;
    let remaining = relations
        .len()
        .saturating_sub(start);
    let slice = match q.limit {
        Some(limit) => {
            &relations[start.min(relations.len())..][..(limit as usize).min(remaining)]
        }
        None => &relations[start.min(relations.len())..],
    };

    let items = relations_to_items(
        &state
            .ctx
            .db,
        slice,
        false,
    )
    .await?;

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
