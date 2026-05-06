use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::get;
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::db;
use crate::db::auth;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};

use super::items::get_items;

pub fn livetv_view_id() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, b"remux-livetv-view")
}

pub fn livetv_view_item() -> api::BaseItemDto {
    api::BaseItemDto {
        id: livetv_view_id(),
        name: Some("Live TV".to_string()),
        type_: api::MediaType::CollectionFolder,
        collection_type: Some(api::CollectionType::Livetv),
        ..Default::default()
    }
}

#[get("/shows/{id}/seasons")]
pub async fn shows_seasons(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(mut q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.parent_id = Some(id);
    q.include_item_types = Some(vec![api::MediaType::Season]);
    if q.sort_by.is_none() {
        q.sort_by = Some(vec![api::ItemSortBy::IndexNumber]);
        q.sort_order = Some(vec![api::SortOrder::Ascending]);
    }
    let items = get_items(state, session.clone(), q.clone(), true)
        .await?
        .with_permissions(&session);

    Ok(Json(api::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q.start_index.unwrap_or(0),
        ..Default::default()
    }))
}

#[get("/shows/{id}/episodes")]
pub async fn shows_episodes(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(mut q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // Some Jellyfin clients accidentally pass the season ID as the show ID in the path.
    // If season_id is given, it's sufficient on its own (maps to parent_id in get_items),
    // so skip setting series_id to avoid filtering by the wrong ID.
    if q.season_id.is_none() {
        q.series_id = Some(id);
    }
    q.include_item_types = Some(vec![api::MediaType::Episode]);
    if q.sort_by.is_none() {
        q.sort_by = Some(vec![
            api::ItemSortBy::ParentIndexNumber,
            api::ItemSortBy::IndexNumber,
        ]);
        q.sort_order = Some(vec![api::SortOrder::Ascending]);
    }
    if let Some(start_id) = q.start_item_id.take() {
        if q.start_index.is_none() {
            let mut all_q = q.clone();
            all_q.limit = None;
            all_q.start_index = None;
            let all = get_items(state.clone(), session.clone(), all_q, false).await?;
            if let Some(pos) = all.items.iter().position(|i| i.id == start_id) {
                q.start_index = Some(pos as u32);
            }
        }
    }
    let items = get_items(state, session.clone(), q.clone(), true)
        .await?
        .with_permissions(&session);

    Ok(Json(api::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q.start_index.unwrap_or(0),
        ..Default::default()
    }))
}

/// This sbould hold dynamic collections
#[get("/userviews")]
pub async fn userviews(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let policy_rules: Vec<remux_sdks::remux::FilterRule> = session
        .user
        .policy
        .as_ref()
        .and_then(|p| p.0.filter_rules.as_ref())
        .map(|f| f.rules.clone())
        .unwrap_or_default();

    let library_filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::Collection, db::MediaKind::Folder]),
        promoted: Some(true),
        filter_rules: policy_rules,
        include_child_count: true,
        ..Default::default()
    };
    let channel_filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvChannel]),
        enabled: Some(true),
        ..Default::default()
    };
    let (library_result, channel_result) = tokio::join!(
        db::Media::get_by_filter(&state.ctx.db, &library_filter),
        db::Media::get_by_filter(&state.ctx.db, &channel_filter),
    );

    let mut items = library_result?
        .records
        .into_iter()
        .map(api::db_media_to_item)
        .collect::<Vec<api::BaseItemDto>>();

    // Inject a synthetic Live TV view if any enabled channels exist
    if !channel_result?.records.is_empty() {
        items.push(livetv_view_item());
    }

    let count = items.len() as i64;
    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: count,
        ..Default::default()
    }))
}

#[get("/userviews/groupingoptions")]
pub async fn userviews_groupingoptions(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let policy_rules: Vec<remux_sdks::remux::FilterRule> = session
        .user
        .policy
        .as_ref()
        .and_then(|p| p.0.filter_rules.as_ref())
        .map(|f| f.rules.clone())
        .unwrap_or_default();

    let filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::Collection, db::MediaKind::Folder]),
        promoted: Some(true),
        filter_rules: policy_rules,
        ..Default::default()
    };
    let items = db::Media::get_by_filter(&state.ctx.db, &filter)
        .await?
        .records
        .into_iter()
        .map(|m| remux_sdks::remux::SpecialViewOptionDto {
            name: Some(m.title.clone()),
            id: Some(m.id.to_string()),
        })
        .collect::<Vec<_>>();

    Ok(Json(items))
}

#[get("/shows/nextup")]
pub async fn shows_nextup(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // Home-screen call: no seriesId — return one next-up episode per in-progress series
    if q.series_id.is_none() {
        return shows_nextup_all(state, session, q)
            .await
            .map(IntoResponse::into_response);
    }
    let series_id = q.series_id.unwrap();

    let disable_first = q.disable_first_episode.unwrap_or(false);
    let enable_resumable = q.enable_resumable.unwrap_or(true);
    let user_id = session.user.id;

    // All episodes for the series in watch order (season asc, episode asc)
    let episodes: Vec<db::Media> = sqlx::query_as(
        "SELECT * FROM media \
         WHERE series_id = ? AND kind = 'episode' \
         ORDER BY COALESCE(parent_idx, 9999) ASC, COALESCE(idx, 9999) ASC",
    )
    .bind(series_id)
    .fetch_all(&state.ctx.db)
    .await?;

    if episodes.is_empty() {
        return Ok(Json(api::BaseItemDtoQueryResult::default()).into_response());
    }

    // Batch-load play states for this user
    let media_keys: Vec<String> =
        episodes.iter().filter_map(|e| e.media_id.clone()).collect();

    let states: HashMap<String, db::UserMediaState> = if media_keys.is_empty() {
        HashMap::new()
    } else {
        db::UserMediaState::get_by_filter(
            &state.ctx.db,
            &db::UserMediaStateFilter {
                user_id: Some(user_id),
                media_key: Some(media_keys),
                ..Default::default()
            },
        )
        .await?
        .records
        .into_iter()
        .map(|s| (s.media_key.clone(), s))
        .collect()
    };

    let state_for = |e: &db::Media| -> Option<&db::UserMediaState> {
        e.media_id.as_ref().and_then(|k| states.get(k))
    };

    // 1. Resumable: partially-watched episode
    let mut next_ep: Option<&db::Media> = None;
    if enable_resumable {
        next_ep = episodes.iter().find(|e| {
            state_for(e).map_or(false, |s| s.play_count == 0 && s.playback_position > 0)
        });
    }

    // 2. First unplayed episode after the last fully-played episode
    if next_ep.is_none() {
        let last_played_pos = episodes
            .iter()
            .rposition(|e| state_for(e).map_or(false, |s| s.play_count > 0));

        next_ep = if let Some(pos) = last_played_pos {
            episodes.get(pos + 1)
        } else if !disable_first {
            // Nothing watched yet — show first regular (Season 1+) episode,
            // skipping Season 0 specials just like Jellyfin server does.
            episodes
                .iter()
                .find(|e| e.parent_idx.map_or(true, |s| s > 0))
                .or_else(|| episodes.first())
        } else {
            None
        };
    }

    let Some(ep) = next_ep else {
        return Ok(Json(api::BaseItemDtoQueryResult::default()).into_response());
    };

    let mut enriched = vec![ep.clone()];
    db::Media::enrich_parents(&state.ctx.db, &mut enriched).await;
    let ep = enriched.remove(0);

    let mut item = api::db_media_to_item(ep.clone());
    if let Some(s) = state_for(&ep) {
        item.user_data = Some(api::db_state_to_dto(s.clone(), &ep));
    }

    Ok(Json(api::BaseItemDtoQueryResult {
        items: vec![item],
        total_record_count: 1,
        start_index: 0,
        ..Default::default()
    })
    .into_response())
}

/// Home-screen NextUp: one next-up episode per series that the user has started watching.
/// Only returns series where at least one episode has been played or is in progress.
async fn shows_nextup_all(
    state: AppState,
    session: auth::AuthSession,
    q: api::GetItemsQuery,
) -> Result<impl IntoResponse> {
    let user_id = session.user.id;
    let limit = q.limit.unwrap_or(50) as i64;
    let enable_resumable = q.enable_resumable.unwrap_or(true);

    // Find series the user has interacted with: at least one played or in-progress episode.
    // For each such series, find the next episode to watch (same logic as per-series nextup).
    // We do this in SQL: for each series, find the first episode after the last played one.
    //
    // Step 1: Get distinct series_ids where user has play state
    let active_series: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT e.series_id \
         FROM media e \
         JOIN user_media_state ums ON ums.media_key = e.media_id \
         WHERE e.kind = 'episode' \
           AND e.series_id IS NOT NULL \
           AND ums.user_id = ? \
           AND (ums.play_count > 0 OR ums.playback_position > 0) \
         LIMIT ?",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(&state.ctx.db)
    .await?;

    if active_series.is_empty() {
        return Ok(Json(api::BaseItemDtoQueryResult::default()).into_response());
    }

    let mut items: Vec<api::BaseItemDto> = Vec::new();

    for (series_id,) in active_series {
        let episodes: Vec<db::Media> = sqlx::query_as(
            "SELECT * FROM media \
             WHERE series_id = ? AND kind = 'episode' \
             ORDER BY COALESCE(parent_idx, 9999) ASC, COALESCE(idx, 9999) ASC",
        )
        .bind(series_id)
        .fetch_all(&state.ctx.db)
        .await?;

        if episodes.is_empty() {
            continue;
        }

        let media_keys: Vec<String> =
            episodes.iter().filter_map(|e| e.media_id.clone()).collect();
        let states: HashMap<String, db::UserMediaState> = if media_keys.is_empty() {
            HashMap::new()
        } else {
            db::UserMediaState::get_by_filter(
                &state.ctx.db,
                &db::UserMediaStateFilter {
                    user_id: Some(user_id),
                    media_key: Some(media_keys),
                    ..Default::default()
                },
            )
            .await?
            .records
            .into_iter()
            .map(|s| (s.media_key.clone(), s))
            .collect()
        };

        let state_for = |e: &db::Media| -> Option<&db::UserMediaState> {
            e.media_id.as_ref().and_then(|k| states.get(k))
        };

        // Resumable first
        let mut next_ep: Option<&db::Media> = None;
        if enable_resumable {
            next_ep = episodes.iter().find(|e| {
                state_for(e)
                    .map_or(false, |s| s.play_count == 0 && s.playback_position > 0)
            });
        }
        // Then next after last played
        if next_ep.is_none() {
            let last_played_pos = episodes
                .iter()
                .rposition(|e| state_for(e).map_or(false, |s| s.play_count > 0));
            if let Some(pos) = last_played_pos {
                next_ep = episodes.get(pos + 1);
            }
        }

        if let Some(ep) = next_ep {
            let mut enriched = vec![ep.clone()];
            db::Media::enrich_parents(&state.ctx.db, &mut enriched).await;
            let ep = enriched.remove(0);
            let mut item = api::db_media_to_item(ep.clone());
            if let Some(s) = state_for(&ep) {
                item.user_data = Some(api::db_state_to_dto(s.clone(), &ep));
            }
            items.push(item);
        }
    }

    let total = items.len() as i64;
    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        start_index: q.start_index.unwrap_or(0),
        ..Default::default()
    })
    .into_response())
}

/// Upcoming episodes (live TV future airings) — not supported, return empty.
#[get("/shows/upcoming")]
pub async fn shows_upcoming(
    _state: State<AppState>,
    _session: auth::AuthSession,
) -> impl IntoResponse {
    Json(api::BaseItemDtoQueryResult::default())
}
