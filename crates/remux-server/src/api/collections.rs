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
use remux_sdks::CommaSeparatedList;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AppState, api, db, db::auth::AdminSession};

// ---------------------------------------------------------------------------
// POST /collections
// ---------------------------------------------------------------------------

/// Jellyfin collection creation request. The official client sends this as
/// query parameters, e.g. `POST /Collections?Name=...&Ids=...&IsLocked=true`.
#[query]
#[derive(Debug)]
pub struct CreateCollectionQuery {
    pub name: String,
    #[serde(default)]
    pub ids: CommaSeparatedList<Uuid>,
    pub parent_id: Option<Uuid>,
    pub is_locked: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionCreationResult {
    pub id: Uuid,
}

#[post("/collections")]
pub async fn create_collection(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(q): Query<CreateCollectionQuery>,
) -> Result<Json<CollectionCreationResult>> {
    if let Some(parent_id) = q.parent_id {
        let parent = db::Media::get_by_id(
            &state
                .ctx
                .db,
            &parent_id,
        )
        .await?
        .context_not_found("Parent collection not found")?;
        if !parent.is_group_container() {
            return Err(anyhow::anyhow!("parent is not a collection group"))
                .context_bad_request("ParentId must be a collection group");
        }
    }

    let mut collection = db::Media {
        title: q.name,
        kind: db::MediaKind::Collection,
        collection_kind: Some(db::CollectionKind::Manual),
        collection_media_kind: Some(db::CollectionMediaKind::Mixed),
        parent_id: q.parent_id,
        is_locked: q
            .is_locked
            .unwrap_or(false),
        ..Default::default()
    };
    collection
        .save(
            &state
                .ctx
                .db,
        )
        .await
        .context_bad_request("Failed to create collection")?;

    if !q
        .ids
        .is_empty()
    {
        let item_ids =
            crate::services::MediaResolveService::resolve_ids(&q.ids, &state.ctx).await;
        db::MediaRelation::add_collection_items(
            &state
                .ctx
                .db,
            &collection.id,
            &item_ids,
        )
        .await
        .context_bad_request("Failed to add collection items")?;
    }

    Ok(Json(CollectionCreationResult { id: collection.id }))
}

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
    pub ids: CommaSeparatedList<Uuid>,
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

    if collection.collection_kind == Some(db::CollectionKind::Smart) {
        return Err(anyhow::anyhow!("smart collection"))
            .context_bad_request("Cannot add items to a smart collection");
    }

    let media_ids: Vec<Uuid> = q
        .ids
        .to_vec();

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
    pub ids: CommaSeparatedList<Uuid>,
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

    if collection.collection_kind == Some(db::CollectionKind::Smart) {
        return Err(anyhow::anyhow!("smart collection"))
            .context_bad_request("Cannot remove items from a smart collection");
    }

    let ids: Vec<Uuid> = q
        .ids
        .to_vec();

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
        db::MediaRelation::delete_by_relation_ids(
            &state
                .ctx
                .db,
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
    // Catalog items are stubs (an opendal title comes from the filename) and
    // their id can differ from the stored row for the same external id.
    // Upserting such a stub doesn't insert it: the target-less `ON CONFLICT
    // DO UPDATE` lands on the stored row and overwrites its title. Adopt
    // stored rows first, like `import_catalog_items`, and only write new ones.
    db::Media::adopt_existing_rows(
        &state
            .ctx
            .db,
        &mut items,
    )
    .await;
    let media_ids: Vec<Uuid> = items
        .iter()
        .map(|m| m.id)
        .collect();
    let mut stored_ids: std::collections::HashSet<Uuid> =
        std::collections::HashSet::new();
    for chunk in media_ids.chunks(500) {
        let mut qb = sqlx::QueryBuilder::new("SELECT id FROM media WHERE id IN (");
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(id);
        }
        qb.push(")");
        let rows: Vec<Uuid> = qb
            .build_query_scalar()
            .fetch_all(
                &state
                    .ctx
                    .db,
            )
            .await?;
        stored_ids.extend(rows);
    }
    let new_items: Vec<db::Media> = items
        .into_iter()
        .filter(|m| !stored_ids.contains(&m.id))
        .collect();

    // Upsert only the new items so they exist in the DB.
    if !new_items.is_empty() {
        db::Media::upsert(
            &state
                .ctx
                .db,
            &new_items,
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
    use uuid::Uuid;

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

    #[tokio::test]
    async fn create_collection_matches_jellyfin_query_shape() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let first = insert_movie(db, "First", "tt9990002").await;
        let second = insert_movie(db, "Second", "tt9990003").await;

        let response = server
            .post("/collections")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[
                ("Name", "nana"),
                ("IsLocked", "true"),
                (
                    "Ids",
                    &format!(
                        "{},{}",
                        first
                            .id
                            .simple(),
                        second
                            .id
                            .simple()
                    ),
                ),
            ])
            .await;
        response.assert_status_ok();
        let body: serde_json::Value = response.json();
        let collection_id: Uuid = body["Id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let collection = db::Media::get_by_id(db, &collection_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(collection.title, "nana");
        assert_eq!(collection.kind, db::MediaKind::Collection);
        assert_eq!(collection.collection_kind, Some(db::CollectionKind::Manual));
        assert_eq!(
            collection.collection_media_kind,
            Some(db::CollectionMediaKind::Mixed)
        );
        assert!(collection.is_locked);

        let relations = db::MediaRelation::get_collection_items(db, &collection_id)
            .await
            .unwrap();
        assert_eq!(relations.len(), 2);
        assert_eq!(relations[0].right_media_id, first.id);
        assert_eq!(relations[1].right_media_id, second.id);
    }

    /// `importcatalog` must not rewrite an existing item from the catalog's
    /// stub. Here the catalog lists a movie under a stub title (as a
    /// filename-parsed opendal item would) with a stub id that differs from
    /// the stored row's id but the same IMDb id. Before the fix, the stub was
    /// upserted as-is: SQLite's target-less `ON CONFLICT DO UPDATE` redirected
    /// it onto the stored row (unique external-id index), overwrote the
    /// stored title, and the request then failed with 400 because the stub
    /// id never became a row.
    #[tokio::test]
    async fn import_catalog_does_not_overwrite_existing_item_from_stub() {
        use futures::FutureExt;

        let (server, guard, token) = authenticated_server().await;
        let ctx = &guard.0;
        let auth = auth_header_with_token(&token);

        let mock = httpmock::MockServer::start();
        mock.mock(|when, then| {
            when.path("/manifest.json");
            then.status(200)
                .json_body(serde_json::json!({
                    "id": "stub-catalog",
                    "name": "Stub catalog",
                    "version": "1.0.0",
                    "resources": ["catalog"],
                    "types": ["movie"],
                    "catalogs": [{"type": "movie", "id": "files", "name": "files"}]
                }));
        });
        mock.mock(|when, then| {
            when.path("/catalog/movie/files.json");
            then.status(200)
                .json_body(serde_json::json!({
                    "metas": [{"id": "tt0088763", "type": "movie", "name": "Back To The Future"}]
                }));
        });

        let now = Utc::now().naive_utc();
        let addon_id = uuid::Uuid::new_v4();
        crate::addons::Addon {
            id: addon_id,
            name: "stub-catalog".to_string(),
            preset: crate::addons::AddonPresetRef {
                kind: "stremio".to_string(),
                config: serde_json::json!({
                    "manifest_url": format!("{}/manifest.json", mock.base_url())
                })
                .into(),
            },
            resources: vec![crate::addons::ResourceType::Catalog],
            types: vec![],
            enabled: true,
            priority: 0,
            system: false,
            is_default: true,
            http_redirect_stream: false,
            service_filter: vec![],
            created_at: now,
            updated_at: now,
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        ctx.addons
            .reload(&ctx.db, &ctx.config)
            .await
            .unwrap();

        // The stored row, as a TMDB-sourced import would have created it.
        // Its id derives from the TMDB id, not the IMDb id the catalog stub's
        // id derives from -- two ids, one IMDb id.
        let mut stored = db::Media {
            id: crate::common::stable_media_uuid(&db::MediaKind::Movie, "tmdb:105"),
            title: "Back to the Future".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: ExternalIds {
                imdb: Some(NonEmptyString::try_new("tt0088763".to_string()).unwrap()),
                tmdb: Some(105),
                ..Default::default()
            },
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        stored
            .save(&ctx.db)
            .await
            .unwrap();
        let collection = insert_manual_collection(&ctx.db, "Imported").await;

        let resp = std::panic::AssertUnwindSafe(
            server
                .post(&format!("/remux/collections/{}/importcatalog", collection.id))
                .add_header(
                    http::header::AUTHORIZATION,
                    HeaderValue::from_str(&auth).unwrap(),
                )
                .json(&serde_json::json!({"addon_id": addon_id, "catalog_id": "movie:files"}))
                .into_future(),
        )
        .catch_unwind()
        .await;

        let after = db::Media::get_by_id(&ctx.db, &stored.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after.title, "Back to the Future",
            "importing a catalog must not overwrite an existing item's title with the stub's"
        );
        assert!(resp.is_ok(), "importcatalog must succeed (2xx)");
        let members: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT right_media_id FROM media_relations WHERE left_media_id = ? AND role = 'collection'",
        )
        .bind(collection.id)
        .fetch_all(&ctx.db)
        .await
        .unwrap();
        assert_eq!(
            members,
            vec![stored.id],
            "the collection must point at the stored row"
        );
    }
}
