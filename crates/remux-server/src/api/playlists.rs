use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{delete, get, post, query};
use remux_sdks::{CommaSeparatedList, remux::deserialize_separated_str};
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt, api, common::get_uuid, db, db::auth,
};
use axum_anyhow::ApiResult as Result;
use tracing::warn;

fn require_playlist_read(media: &db::Media, session: &auth::AuthSession) -> Result<()> {
    if !media.public
        && media.user_id
            != Some(
                session
                    .user
                    .id,
            )
        && !session
            .user
            .is_admin
    {
        return Err(anyhow::anyhow!("Access denied").context_forbidden("Access denied"));
    }
    Ok(())
}

fn require_playlist_write(
    media: &db::Media,
    session: &auth::AuthSession,
) -> Result<()> {
    if media.user_id
        != Some(
            session
                .user
                .id,
        )
        && !session
            .user
            .is_admin
    {
        return Err(anyhow::anyhow!("Access denied").context_forbidden("Access denied"));
    }
    Ok(())
}

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
    session: auth::AuthSession,
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
        user_id: Some(
            session
                .user
                .id,
        ),
        public: body
            .is_public
            .unwrap_or(false),
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
    session: auth::AuthSession,
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

    if !media.public
        && media.user_id
            != Some(
                session
                    .user
                    .id,
            )
        && !session
            .user
            .is_admin
    {
        return Err(anyhow::anyhow!("Access denied").context_forbidden("Access denied"));
    }

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
        "OpenAccess": media.public,
        "Shares": [],
        "ItemIds": item_ids,
    })))
}

#[post("/playlists/{id}")]
pub async fn update_playlist(
    State(state): State<AppState>,
    session: auth::AuthSession,
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

    if media.user_id
        != Some(
            session
                .user
                .id,
        )
        && !session
            .user
            .is_admin
    {
        return Err(anyhow::anyhow!("Access denied").context_forbidden("Access denied"));
    }

    if let Some(name) = body.name {
        media.title = name;
    }
    if let Some(is_public) = body.is_public {
        media.public = is_public;
    }
    media
        .save(
            &state
                .ctx
                .db,
        )
        .await?;

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
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<PlaylistItemsQuery>,
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
    require_playlist_read(&media, &session)?;

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
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<AddItemsQuery>,
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
    require_playlist_write(&media, &session)?;

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

/// GET /Playlists/{id}/Users
/// Returns the share list — always empty since we don't support per-user shares.
/// Owner-only.
#[get("/playlists/{id}/users")]
pub async fn get_playlist_users(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .filter(|m| m.kind == db::MediaKind::Playlist)
    .context_not_found("Playlist not found")?;

    if media.user_id
        != Some(
            session
                .user
                .id,
        )
        && !session
            .user
            .is_admin
    {
        return Err(anyhow::anyhow!("Access denied").context_forbidden("Access denied"));
    }

    Ok(Json(serde_json::json!([])))
}

/// GET /Playlists/{id}/Users/{userId}
#[get("/playlists/{id}/users/{user_id}")]
pub async fn get_playlist_user(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .filter(|m| m.kind == db::MediaKind::Playlist)
    .context_not_found("Playlist not found")?;

    let is_owner = media.user_id
        == Some(
            session
                .user
                .id,
        )
        || session
            .user
            .is_admin;
    let can_edit = is_owner
        && user_id
            == session
                .user
                .id;

    Ok(Json(serde_json::json!({
        "UserId": user_id,
        "CanEdit": can_edit
    })))
}

/// POST /Playlists/{id}/Users/{userId} — stub; we don't implement per-user shares.
#[post("/playlists/{id}/users/{user_id}")]
pub async fn update_playlist_user(
    _state: State<AppState>,
    _session: auth::AuthSession,
    Path((_id, _user_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /Playlists/{id}/Users/{userId} — stub; we don't implement per-user shares.
#[delete("/playlists/{id}/users/{user_id}")]
pub async fn remove_playlist_user(
    _state: State<AppState>,
    _session: auth::AuthSession,
    Path((_id, _user_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT)
}

#[delete("/playlists/{id}/items")]
pub async fn remove_playlist_items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<RemoveItemsQuery>,
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
    require_playlist_write(&media, &session)?;

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
    session: auth::AuthSession,
    Path((id, item_id, new_index)): Path<(Uuid, Uuid, usize)>,
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
    require_playlist_write(&media, &session)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn test_db() -> SqlitePool {
        let db = db::connect("sqlite::memory:", 10_000)
            .await
            .unwrap();
        db::migrate(&db)
            .await
            .unwrap();
        db
    }

    async fn insert_user(db: &SqlitePool) -> db::User {
        let mut user = db::User {
            id: uuid::Uuid::new_v4(),
            username: format!("user_{}", uuid::Uuid::new_v4()),
            password_hash: "test"
                .to_string()
                .into(),
            ..Default::default()
        };
        user.save(db)
            .await
            .unwrap();
        user
    }

    async fn insert_playlist(
        db: &SqlitePool,
        owner: &db::User,
        public: bool,
    ) -> db::Media {
        let mut pl = db::Media {
            id: uuid::Uuid::new_v4(),
            title: "Test Playlist".to_string(),
            kind: db::MediaKind::Playlist,
            user_id: Some(owner.id),
            public,
            ..Default::default()
        };
        pl.save(db)
            .await
            .unwrap();
        pl
    }

    async fn visible_playlist_ids(
        db: &SqlitePool,
        user_id: uuid::Uuid,
    ) -> Vec<uuid::Uuid> {
        db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Playlist]),
                user_id: Some(user_id),
                total_count: false,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .records
        .into_iter()
        .map(|m| m.id)
        .collect()
    }

    #[tokio::test]
    async fn owner_sees_their_private_playlist() {
        let db = test_db().await;
        let owner = insert_user(&db).await;
        let pl = insert_playlist(&db, &owner, false).await;

        assert!(
            visible_playlist_ids(&db, owner.id)
                .await
                .contains(&pl.id),
            "owner should see their own private playlist"
        );
    }

    #[tokio::test]
    async fn other_user_cannot_see_private_playlist() {
        let db = test_db().await;
        let owner = insert_user(&db).await;
        let other = insert_user(&db).await;
        let pl = insert_playlist(&db, &owner, false).await;

        assert!(
            !visible_playlist_ids(&db, other.id)
                .await
                .contains(&pl.id),
            "other user must not see private playlist"
        );
    }

    #[tokio::test]
    async fn public_playlist_visible_to_all_users() {
        let db = test_db().await;
        let owner = insert_user(&db).await;
        let other = insert_user(&db).await;
        let pl = insert_playlist(&db, &owner, true).await;

        assert!(
            visible_playlist_ids(&db, owner.id)
                .await
                .contains(&pl.id),
            "owner should see public playlist"
        );
        assert!(
            visible_playlist_ids(&db, other.id)
                .await
                .contains(&pl.id),
            "other user should see public playlist"
        );
    }

    #[tokio::test]
    async fn stored_playlist_has_owner_and_privacy() {
        let db = test_db().await;
        let owner = insert_user(&db).await;
        let pl = insert_playlist(&db, &owner, false).await;

        let stored = db::Media::get_by_id(&db, &pl.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.user_id, Some(owner.id));
        assert!(!stored.public);
    }

    #[tokio::test]
    async fn making_playlist_public_exposes_it_to_others() {
        let db = test_db().await;
        let owner = insert_user(&db).await;
        let other = insert_user(&db).await;
        let mut pl = insert_playlist(&db, &owner, false).await;

        assert!(
            !visible_playlist_ids(&db, other.id)
                .await
                .contains(&pl.id)
        );

        pl.public = true;
        pl.save(&db)
            .await
            .unwrap();

        assert!(
            visible_playlist_ids(&db, other.id)
                .await
                .contains(&pl.id)
        );
    }
}
