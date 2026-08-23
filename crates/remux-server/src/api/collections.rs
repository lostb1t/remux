use crate::{OptionExt, ResultExt};
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query;
use futures::StreamExt;
use http::StatusCode;
use remux_macros::{delete, get, post, query};
use serde::Deserialize;
use uuid::Uuid;

use crate::{AppState, api, db, db::auth::AdminSession};

// ---------------------------------------------------------------------------
// GET /collections/{id}/items
// ---------------------------------------------------------------------------

#[query]
#[derive(Debug)]
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
    let collection = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .filter(|m| m.kind == db::MediaKind::Collection)
    .context_not_found("Collection not found")?;

    let relations = db::MediaRelation::get_collection_items(
        &state
            .ctx
            .db,
        &id,
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

    let mut items = Vec::with_capacity(slice.len());
    for rel in slice {
        if let Some(media) = db::Media::get_by_id(
            &state
                .ctx
                .db,
            &rel.right_media_id,
        )
        .await?
        {
            let mut dto = api::db_media_to_item(media, false);
            dto.playlist_item_id = Some(
                rel.relation_id
                    .to_string(),
            );
            items.push(dto);
        }
    }

    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        start_index: q
            .start_index
            .unwrap_or(0),
        ..Default::default()
    }))
}

// ---------------------------------------------------------------------------
// POST /collections/{id}/items  (add items by id list)
// ---------------------------------------------------------------------------

#[query]
#[derive(Debug)]
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
    let collection = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .filter(|m| m.kind == db::MediaKind::Collection)
    .context_not_found("Collection not found")?;

    let media_ids: Vec<Uuid> = q
        .ids
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| Uuid::parse_str(s.trim()).ok())
        .collect();

    if collection.is_group_container() {
        let collection_ids: Vec<Uuid> = db::Media::get_by_ids(
            &state
                .ctx
                .db,
            &media_ids,
        )
        .await?
        .into_iter()
        .filter(|m| m.kind == db::MediaKind::Collection)
        .map(|m| m.id)
        .collect();
        db::Media::set_parent_id(
            &state
                .ctx
                .db,
            &collection_ids,
            Some(id),
        )
        .await
        .context_bad_request("failed to add items")?;
    } else {
        db::MediaRelation::add_collection_items(
            &state
                .ctx
                .db,
            &id,
            &media_ids,
        )
        .await
        .context_bad_request("failed to add items")?;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// DELETE /collections/{id}/items  (?ids=media_id,...)
// ---------------------------------------------------------------------------

#[query]
#[derive(Debug)]
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
    let collection = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .filter(|m| m.kind == db::MediaKind::Collection)
    .context_not_found("Collection not found")?;

    let ids: Vec<Uuid> = q
        .ids
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| Uuid::parse_str(s.trim()).ok())
        .collect();

    if collection.is_group_container() {
        db::Media::clear_parent_id_scoped(
            &state
                .ctx
                .db,
            &ids,
            &id,
        )
        .await
        .context_bad_request("failed to remove items")?;
    } else {
        db::MediaRelation::delete_collection_items_by_media_ids(
            &state
                .ctx
                .db,
            &id,
            &ids,
        )
        .await
        .context_bad_request("failed to remove items")?;
    }

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
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .filter(|m| m.kind == db::MediaKind::Collection)
    .context_not_found("Collection not found")?;

    db::MediaRelation::move_collection_item(
        &state
            .ctx
            .db,
        &id,
        &item_id,
        new_index,
    )
    .await
    .context_bad_request("failed to move item")?;

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
    let mut collection = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .filter(|m| m.kind == db::MediaKind::Collection)
    .context_not_found("Collection not found")?;

    let addon = state
        .ctx
        .addons
        .get_catalog(body.addon_id)
        .context_not_found("Addon not found or has no catalog")?;

    let stream = addon
        .catalog_stream(&state.ctx, &body.catalog_id)
        .await
        .context_bad_request("addon catalog_stream failed")?
        .context_not_found("Catalog not found in addon")?;

    let mut items: Vec<db::Media> = Vec::new();
    let mut stream = stream;
    while let Some(item) = stream
        .next()
        .await
    {
        items.push(item);
    }
    let media_ids: Vec<Uuid> = items
        .iter()
        .map(|m| m.id)
        .collect();

    // Upsert the items so they exist in the DB.
    if !items.is_empty() {
        db::Media::upsert(
            &state
                .ctx
                .db,
            &items,
        )
        .await?;
    }

    db::MediaRelation::replace_collection_items(
        &state
            .ctx
            .db,
        &id,
        &media_ids,
    )
    .await
    .context_bad_request("failed to replace collection items")?;

    // Ensure collection_kind is Manual.
    if collection.collection_kind != Some(db::CollectionKind::Manual) {
        collection.collection_kind = Some(db::CollectionKind::Manual);
        collection
            .save(
                &state
                    .ctx
                    .db,
            )
            .await
            .context_bad_request("failed to update collection kind")?;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use http::header::HeaderValue;

    use crate::{
        db,
        db::{ExternalIds, MediaIdRaw, NonEmptyString},
        integration_test::{auth_header_with_token, authenticated_server},
    };

    async fn get_user_id(server: &axum_test::TestServer, auth: &str) -> String {
        let resp: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(auth).unwrap(),
            )
            .await
            .json();
        resp["Id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn insert_group_container(db: &sqlx::SqlitePool, title: &str) -> db::Media {
        let now = Utc::now().naive_utc();
        let mut c = db::Media {
            title: title.to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Manual),
            collection_media_kind: Some(db::CollectionMediaKind::Collection),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        c.save(db)
            .await
            .expect("insert_group_container failed");
        c
    }

    // Smart collection — matches the child type expected by group-container browse.
    async fn insert_smart_collection(db: &sqlx::SqlitePool, title: &str) -> db::Media {
        let now = Utc::now().naive_utc();
        let mut c = db::Media {
            title: title.to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Smart),
            collection_media_kind: Some(db::CollectionMediaKind::Movie),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        c.save(db)
            .await
            .expect("insert_smart_collection failed");
        c
    }

    // Manual (non-group) collection — used for the regular relation path.
    async fn insert_manual_collection(db: &sqlx::SqlitePool, title: &str) -> db::Media {
        let now = Utc::now().naive_utc();
        let mut c = db::Media {
            title: title.to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Manual),
            collection_media_kind: Some(db::CollectionMediaKind::Movie),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        c.save(db)
            .await
            .expect("insert_manual_collection failed");
        c
    }

    async fn insert_movie(db: &sqlx::SqlitePool, title: &str, imdb: &str) -> db::Media {
        let now = Utc::now().naive_utc();
        let ext = ExternalIds {
            imdb: Some(NonEmptyString::try_new(imdb.to_string()).unwrap()),
            ..Default::default()
        };
        let id = uuid::Uuid::from(&MediaIdRaw {
            kind: db::MediaKind::Movie,
            external_ids: ext.clone(),
            season: None,
            episode: None,
        });
        let mut m = db::Media {
            id,
            title: title.to_string(),
            kind: db::MediaKind::Movie,
            external_ids: ext,
            created_at: now,
            updated_at: now,
            released_at: Some(now - chrono::Duration::days(365)),
            ..Default::default()
        };
        m.save(db)
            .await
            .expect("insert_movie failed");
        m
    }

    #[tokio::test]
    async fn add_to_group_container_sets_parent_id() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth).await;

        let group = insert_group_container(db, "Group").await;
        let child = insert_smart_collection(db, "Child").await;

        server
            .post(&format!("/collections/{}/items", group.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "ids",
                child
                    .id
                    .to_string(),
            )])
            .await;

        let updated = db::Media::get_by_id(db, &child.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.parent_id, Some(group.id));

        let body: serde_json::Value = server
            .get(&format!("/users/{user_id}/items"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "parentId",
                group
                    .id
                    .to_string(),
            )])
            .await
            .json();
        let ids: Vec<String> = body["Items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| {
                i["Id"]
                    .as_str()
                    .map(|s| s.to_string())
            })
            .collect();
        let child_id_no_hyphens = child
            .id
            .to_string()
            .replace('-', "");
        assert!(ids.contains(&child_id_no_hyphens));
    }

    #[tokio::test]
    async fn remove_from_group_container_clears_parent_id() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth).await;

        let group = insert_group_container(db, "Group").await;
        let child = insert_smart_collection(db, "Child").await;

        db::Media::set_parent_id(db, &[child.id], Some(group.id))
            .await
            .unwrap();

        server
            .delete(&format!("/collections/{}/items", group.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "ids",
                child
                    .id
                    .to_string(),
            )])
            .await;

        let updated = db::Media::get_by_id(db, &child.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.parent_id, None);

        let body: serde_json::Value = server
            .get(&format!("/users/{user_id}/items"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "parentId",
                group
                    .id
                    .to_string(),
            )])
            .await
            .json();
        assert_eq!(
            body["Items"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "group should be empty after remove"
        );
    }

    #[tokio::test]
    async fn add_to_regular_collection_creates_relation() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let movie = insert_movie(db, "Movie A", "tt9990001").await;
        let col = insert_manual_collection(db, "Movies").await;

        server
            .post(&format!("/collections/{}/items", col.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "ids",
                movie
                    .id
                    .to_string(),
            )])
            .await;

        let relations = db::MediaRelation::get_collection_items(db, &col.id)
            .await
            .unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].right_media_id, movie.id);

        let movie_after = db::Media::get_by_id(db, &movie.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            movie_after.parent_id, None,
            "parent_id must not be set for non-group collections"
        );
    }
}
