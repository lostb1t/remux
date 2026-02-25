use anyhow::Context;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use remux_macros::get;
use axum_extra::extract::Query;
use http::StatusCode;
use std::time::Duration;
use tracing::trace;
use tracing::warn;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
use crate::sdks;
use crate::utils::IntoVec;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};
use chrono::Datelike;

use super::{mock_items, stub_json};

pub struct ItemsQueryResult {
    pub items: Vec<jellyfin::BaseItemDto>,
    pub total_count: i64,
}

pub async fn get_items(
    state: AppState,
    session: auth::AuthSession,
    mut q: jellyfin::GetItemsQuery,
    _count: bool,
) -> Result<ItemsQueryResult> {
    //trace!(?q, "get_items");
    let aio = session.aio;

    let parent = if let Some(parent_id) = q.parent_id.clone() {
        db::Media::get_by_id(&state.ctx.db, &parent_id).await?
    } else {
        None
    };

    let search = q.search_term.clone().or(q.name_starts_with.clone());
    let skip = q.start_index.unwrap_or(0) as u32;

    //  trace!(?q, "get_items");

    // only support Movie, Series, and Episode for search and catalogs
    if search.is_some()
        || parent
            .clone()
            .map_or(false, |p| p.kind == db::MediaKind::Catalog)
    {
        let types = q.get_requested_item_types();
        // if types.len() != 0 {
        if types.len() == 0
            || ![jellyfin::MediaType::Movie, jellyfin::MediaType::Series, jellyfin::MediaType::Episode]
                .contains(&types[0])
        {
            return Ok(ItemsQueryResult {
                items: vec![],
                total_count: 0,
            });
        }

        if let Some(s) = search {
            // todo: need to to make parallel request for types

            let items = aio
                .search(types[0].clone().into(), s)
                .await?
                .into_iter()
                .filter_map(|meta| match db::Media::try_from(meta.clone()) {
                    Ok(media) => {
                        state.ctx.store.save(
                            media.id.clone(),
                            meta.clone(),
                            Duration::from_secs(360),
                        );
                        Some(jellyfin::BaseItemDto::from(media))
                    }
                    Err(e) => {
                        warn!("Failed to convert item to Media: {}", e);
                        None
                    }
                })
                .collect::<Vec<_>>();

            //let items = vec![];
            return Ok(ItemsQueryResult {
                items: items,
                total_count: 9999,
            });
        }
        //  }
    }

    // if q.filters.is_some() {
    //     return Ok(ItemsQueryResult {
    //         items: vec![],
    //         total_count: 0,
    //     });
    // }

    let manifest = aio.get_manifest().await?;

    if let Some(parent) = &parent {
        if parent.id == db::collection_uuid() {
            let result = db::Media::get_by_filter(
                &state.ctx.db,
                &db::MediaFilter {
                    kind: Some(vec![db::MediaKind::Catalog]),
                    //promoted: Some(true),
                    ..Default::default()
                },
            )
            .await?;

            return Ok(ItemsQueryResult {
                total_count: result.total_count as i64,
                items: result.records.into_vec(),
            });
        }

        // catalog get
        if parent.kind == db::MediaKind::Catalog {
            // cataloga

            // if parent.promoted {
            q.parent_id = None;

            if let Some(kind) = parent.catalog_media_kind.clone() {
                q.include_item_types = Some(vec![kind.into()]);
            } else {
                q.include_item_types = Some(vec![
                    db::MediaKind::Movie.into(),
                    db::MediaKind::Series.into(),
                ]);
            }
            //             q.include_item_types = Some(vec![jellyfin::MediaType::Movie]);
            // trace!(?q, "CATALOG");
            if q.limit.is_none() {
                q.limit = Some(250);
            }

            q.user_id = Some(session.user.id.clone());

            let mut result =
                db::Media::get_by_jellyfin_filter(&state.ctx.db, &q, true).await?;

            return Ok(ItemsQueryResult {
                total_count: result.total_count as i64,
                items: result.records.into_vec(),
            });
        }

        //  }
    }
    //trace!(?q, "get_items");
    let mut result =
        db::Media::get_by_jellyfin_filter(&state.ctx.db, &q, false).await?;

    // handle details request
    if let Some(ids) = &q.ids {
        if ids.len() == 1 {
            let mut media: Option<db::Media> =
                if let Some(media) = result.records.get(0) {
                    Some(media.clone())
                } else {
                    if let Some(meta) =
                        state.ctx.store.get::<sdks::aio::Meta>(*ids.get(0).unwrap())
                    {
                        let mut media: db::Media = aio
                            .get_meta(meta.media_type.clone(), meta.id.clone())
                            .await?
                            .try_into()?;

                        // web client makes 2 simultenious request. So we get race conditions.
                        if let Err(err) = media.save(&state.ctx.db).await {
                            media = db::Media::get_by_filter(
                                &state.ctx.db,
                                &db::MediaFilter {
                                    aio_id: media.aio_id.clone(),

                                    ..Default::default()
                                },
                            )
                            .await?
                            .records
                            .get(0)
                            .unwrap()
                            .clone();
                        }

                        Some(media)
                    } else {
                        None
                    }
                };
            if let Some(media) = media.as_mut() {
                if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode)
                    && (q.fields.is_none()
                        || q.fields.as_ref().map_or(false, |f| {
                            f.contains(&jellyfin::ItemFields::MediaSources)
                        }))
                {
                    media.refresh_sources(&state.ctx.db, &state.ctx.aio).await?;
                    media.sources(&state.ctx.db).await?;
                    // always load state for single
                    media.user_state(&state.ctx.db, &session.user).await?;

                    if let Some(sources) = &media.sources {
                        trace!(streams_len = sources.len(), "sources");
                    }
                }

                media.load_relations(&state.ctx.db).await?;

                return Ok(ItemsQueryResult {
                    items: vec![media.clone().into()],
                    total_count: 1,
                });
            }
        }
    }

    Ok(ItemsQueryResult {
        items: result.records.into_vec(),
        total_count: 999_999,
    })
}

#[get("/items/latest")]
pub async fn items_flat(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state, session, q, false).await?;
    Ok(Json::<Vec<jellyfin::BaseItemDto>>(items.items))
}

#[get("/items")]
pub async fn items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    //trace!(?q);
    let items = get_items(state, session, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q.start_index.unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

/// Get item counts
#[get("/items/counts")]
pub async fn items_counts(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    // Get counts for different media types from the database
    let movie_count = db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Movie).await? as i32;
    let series_count = db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Series).await? as i32;
    let episode_count = db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Episode).await? as i32;
    
    // For now, return hardcoded values for other types since we don't have them in the database yet
    // In a real implementation, you would query the actual counts
    let item_counts = jellyfin::ItemCounts {
        movie_count,
        series_count,
        episode_count,
        artist_count: 0, // TODO: Implement artist counting
        program_count: 0, // TODO: Implement program counting
        trailer_count: 0, // TODO: Implement trailer counting
        song_count: 0, // TODO: Implement song counting
        album_count: 0, // TODO: Implement album counting
        music_video_count: 0, // TODO: Implement music video counting
        box_set_count: 0, // TODO: Implement box set counting
        book_count: 0, // TODO: Implement book counting
        item_count: movie_count + series_count + episode_count, // Total of counted items
    };

    Ok(Json(item_counts))
}

pub async fn item(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
) -> Result<Option<jellyfin::BaseItemDto>> {
    let manifest = session.aio.get_manifest().await?;
    // let libraries = super::get_virtual_folders(&state).await?;

    // if let Some(library) = libraries.into_iter().find(|x| x.id == id) {
    //     return Ok(Some(library));
    // }

    let q = jellyfin::GetItemsQuery {
        ids: vec![id].into(),
        ..Default::default()
    };
    return Ok(get_items(state, session, q, false)
        .await?
        .items
        .first()
        .cloned());
}

#[get("/items/{id}")]
pub async fn items_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, session, id).await?).into_response());
}

#[get("/items/suggestions")]
pub async fn items_suggestions(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    //let b = state.tmdb.movie_popular_list().send().await.unwrap()
    //.into_inner()
    //.results
    //.map(|c| {
    //  jellyfin::BaseItemDto {
    //     name: c.title,
    //     ..Default::default()
    //   }
    //}
    //);
    //let tmdb_items = state.tmdb.movie_now_playing().send().await;
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: vec![],
        ..Default::default()
    }))
}

#[get("/persons")]
pub async fn persons(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: vec![],
        ..Default::default()
    }))
}

#[get("/items/filters")]
pub async fn items_filters(State(state): State<AppState>) -> Result<impl IntoResponse> {
    /// genres is actually tags?
    use strum::IntoEnumIterator;
    // let genres = db::Genre::iter().map(|g| g.to_string()).collect();
    let current_year = chrono::Utc::now().year() as i64;
    //let years = (1900..=current_year).collect();

    Ok(Json(jellyfin::QueryFiltersLegacy {
        //  genres: Some(genres),
        //  years: Some(years),
        genres: None,
        years: None,
        ..Default::default()
    }))
}

#[get("/library/virtualfolders")]
pub async fn library_virtualfolders(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let manifest = session.aio.get_manifest().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
    // Ok(Json(json!(
    //     crate::jellyfin::get_virtual_folders(&state).await?
    // )))
}

#[get("/items/{id}/similar")]
pub async fn items_similar(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/items/{id}/thememedia")]
pub async fn items_thememedia(State(state): State<AppState>) -> Result<impl IntoResponse> {
    stub_json(State(state)).await
}
