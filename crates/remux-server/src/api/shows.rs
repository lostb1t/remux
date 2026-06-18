use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use remux_macros::{api_query, get};
use uuid::Uuid;

use crate::{AppState, OptionExt, api, db, db::auth};
use axum_anyhow::ApiResult as Result;

use super::items::get_items;

pub fn livetv_view_id() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, b"remux-livetv-view")
}

pub fn livetv_view_item() -> api::BaseItemDto {
    api::BaseItemDto {
        id: livetv_view_id(),
        name: Some("Live TV".to_string()),
        type_: api::MediaType::UserView,
        collection_type: Some(api::CollectionType::Livetv),
        is_folder: true,
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
    if q.sort_by
        .is_none()
    {
        q.sort_by = Some(vec![api::ItemSortBy::IndexNumber]);
        q.sort_order = Some(vec![api::SortOrder::Ascending]);
    }
    let items = get_items(state, session.clone(), q.clone(), true)
        .await?
        .with_permissions()
        .with_client_patches()
        .build();

    Ok(Json(api::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0),
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
    if q.season_id
        .is_none()
    {
        q.series_id = Some(id);
    }
    q.include_item_types = Some(vec![api::MediaType::Episode]);
    if q.sort_by
        .is_none()
    {
        q.sort_by = Some(vec![
            api::ItemSortBy::ParentIndexNumber,
            api::ItemSortBy::IndexNumber,
        ]);
        q.sort_order = Some(vec![api::SortOrder::Ascending]);
    }
    if let Some(start_id) = q
        .start_item_id
        .take()
    {
        if q.start_index
            .is_none()
        {
            let mut all_q = q.clone();
            all_q.limit = None;
            all_q.start_index = None;
            let all = get_items(state.clone(), session.clone(), all_q, false)
                .await?
                .with_client_patches()
                .build();
            if let Some(pos) = all
                .items
                .iter()
                .position(|i| i.id == start_id)
            {
                q.start_index = Some(pos as u32);
            }
        }
    }
    let items = get_items(state, session.clone(), q.clone(), true)
        .await?
        .with_permissions()
        .with_client_patches()
        .build();

    Ok(Json(api::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0),
        ..Default::default()
    }))
}

#[get("/shows/nextup")]
pub async fn shows_nextup(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // Home-screen call: no seriesId — return one next-up episode per in-progress series
    if q.series_id
        .is_none()
    {
        return shows_nextup_all(state, session, q)
            .await
            .map(IntoResponse::into_response);
    }
    let grandparent_id = q
        .series_id
        .unwrap();

    let disable_first = q
        .disable_first_episode
        .unwrap_or(false);
    let enable_resumable = q
        .enable_resumable
        .unwrap_or(true);
    let user_id = session
        .user
        .id;

    // All episodes for the series in watch order (season asc, episode asc)
    let episodes: Vec<db::Media> = sqlx::query_as(
        "SELECT * FROM media \
         WHERE grandparent_id = ? AND kind = 'episode' \
         ORDER BY COALESCE(parent_idx, 9999) ASC, COALESCE(idx, 9999) ASC",
    )
    .bind(grandparent_id)
    .fetch_all(
        &state
            .ctx
            .db,
    )
    .await?;

    if episodes.is_empty() {
        return Ok(Json(api::BaseItemDtoQueryResult::default()).into_response());
    }

    let media_ids: Vec<Uuid> = episodes
        .iter()
        .map(|e| e.id)
        .collect();

    let states: HashMap<Uuid, db::UserMediaState> = if media_ids.is_empty() {
        HashMap::new()
    } else {
        db::UserMediaState::get_by_filter(
            &state
                .ctx
                .db,
            &db::UserMediaStateFilter {
                user_id: Some(user_id),
                media_id: Some(media_ids),
                ..Default::default()
            },
        )
        .await?
        .records
        .into_iter()
        .map(|s| (s.media_id, s))
        .collect()
    };

    let state_for =
        |e: &db::Media| -> Option<&db::UserMediaState> { states.get(&e.id) };

    // 1. Resumable: partially-watched episode
    let mut next_ep: Option<&db::Media> = None;
    if enable_resumable {
        next_ep = episodes
            .iter()
            .find(|e| {
                state_for(e)
                    .map_or(false, |s| s.play_count == 0 && s.playback_position > 0)
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
                .find(|e| {
                    e.parent_idx
                        .map_or(true, |s| s > 0)
                })
                .or_else(|| episodes.first())
        } else {
            None
        };
    }

    let Some(ep) = next_ep else {
        return Ok(Json(api::BaseItemDtoQueryResult::default()).into_response());
    };

    let mut enriched = vec![ep.clone()];
    db::Media::preload_parents(
        &state
            .ctx
            .db,
        &mut enriched,
    )
    .await;
    let mut ep = enriched.remove(0);
    ep.images = db::MediaImage::get_for_media(
        &state
            .ctx
            .db,
        &ep.id,
    )
    .await
    .unwrap_or_default();

    let mut item = api::db_media_to_item(ep.clone(), false);
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
    let user_id = session
        .user
        .id;
    let limit = q
        .limit
        .unwrap_or(50) as i64;
    let enable_resumable = q
        .enable_resumable
        .unwrap_or(true);

    // Inner UNION selects last_played_at/played_at directly from idx_ums_user_play_state
    // (covering) so no second join to user_media_state is needed. UNION ALL is safe
    // because the two legs are mutually exclusive (play_count > 0 vs play_count = 0).
    let date_cutoff = q
        .next_up_date_cutoff
        .clone()
        .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
    let active_series: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT m.grandparent_id \
         FROM ( \
           SELECT media_id, last_played_at, played_at \
           FROM user_media_state WHERE user_id = ? AND play_count > 0 \
           UNION ALL \
           SELECT media_id, last_played_at, played_at \
           FROM user_media_state WHERE user_id = ? AND play_count = 0 AND playback_position > 0 \
         ) AS active \
         JOIN media m ON m.id = active.media_id \
         WHERE m.kind = 'episode' \
         AND m.grandparent_id IS NOT NULL \
         GROUP BY m.grandparent_id \
         HAVING MAX(COALESCE(active.last_played_at, active.played_at)) >= ? \
         ORDER BY MAX(COALESCE(active.last_played_at, active.played_at)) DESC \
         LIMIT ?",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(&date_cutoff)
    .bind(limit)
    .fetch_all(&state.ctx.db)
    .await?;

    if active_series.is_empty() {
        return Ok(Json(api::BaseItemDtoQueryResult::default()).into_response());
    }

    let series_ids: Vec<Uuid> = active_series
        .into_iter()
        .map(|(id,)| id)
        .collect();

    let mut ep_qb =
        sqlx::QueryBuilder::new("SELECT * FROM media WHERE grandparent_id IN (");
    {
        let mut sep = ep_qb.separated(", ");
        for id in &series_ids {
            sep.push_bind(id);
        }
    }
    ep_qb.push(
        ") AND kind = 'episode' \
         ORDER BY grandparent_id, COALESCE(parent_idx, 9999) ASC, COALESCE(idx, 9999) ASC",
    );
    let all_episodes: Vec<db::Media> = ep_qb
        .build_query_as()
        .fetch_all(
            &state
                .ctx
                .db,
        )
        .await?;

    let all_ep_ids: Vec<Uuid> = all_episodes
        .iter()
        .map(|e| e.id)
        .collect();
    let mut states_map: HashMap<Uuid, db::UserMediaState> = HashMap::new();
    for chunk in all_ep_ids.chunks(900) {
        let mut s_qb =
            sqlx::QueryBuilder::new("SELECT * FROM user_media_state WHERE user_id = ");
        s_qb.push_bind(user_id);
        s_qb.push(" AND media_id IN (");
        let mut sep = s_qb.separated(", ");
        for id in chunk {
            sep.push_bind(id);
        }
        s_qb.push(")");
        let chunk_states: Vec<db::UserMediaState> = s_qb
            .build_query_as()
            .fetch_all(
                &state
                    .ctx
                    .db,
            )
            .await?;
        states_map.extend(
            chunk_states
                .into_iter()
                .map(|s| (s.media_id, s)),
        );
    }

    // Group episodes by grandparent_id (order within each group preserved from query).
    let mut episodes_by_series: HashMap<Uuid, Vec<db::Media>> = HashMap::new();
    for ep in all_episodes {
        if let Some(gid) = ep.grandparent_id {
            episodes_by_series
                .entry(gid)
                .or_default()
                .push(ep);
        }
    }

    // Find the next episode per series in memory — same logic as the single-series path.
    let mut next_eps: Vec<db::Media> = Vec::new();
    for series_id in &series_ids {
        let Some(episodes) = episodes_by_series.get(series_id) else {
            continue;
        };

        let state_for = |e: &db::Media| states_map.get(&e.id);

        let mut next_ep: Option<&db::Media> = None;
        if enable_resumable {
            next_ep = episodes
                .iter()
                .find(|e| {
                    state_for(e)
                        .map_or(false, |s| s.play_count == 0 && s.playback_position > 0)
                });
        }
        if next_ep.is_none() {
            let last_played_pos = episodes
                .iter()
                .rposition(|e| state_for(e).map_or(false, |s| s.play_count > 0));
            if let Some(pos) = last_played_pos {
                next_ep = episodes.get(pos + 1);
            }
        }

        if let Some(ep) = next_ep {
            next_eps.push(ep.clone());
        }
    }

    if next_eps.is_empty() {
        return Ok(Json(api::BaseItemDtoQueryResult::default()).into_response());
    }

    db::Media::preload_parents(
        &state
            .ctx
            .db,
        &mut next_eps,
    )
    .await;
    let next_ep_ids: Vec<Uuid> = next_eps
        .iter()
        .map(|e| e.id)
        .collect();
    let mut images_map = db::MediaImage::get_for_media_ids(
        &state
            .ctx
            .db,
        &next_ep_ids,
    )
    .await
    .unwrap_or_default();
    for ep in &mut next_eps {
        ep.images = images_map
            .remove(&ep.id)
            .unwrap_or_default();
    }

    let items: Vec<api::BaseItemDto> = next_eps
        .into_iter()
        .map(|ep| {
            let mut item = api::db_media_to_item(ep.clone(), false);
            if let Some(s) = states_map.get(&ep.id) {
                item.user_data = Some(api::db_state_to_dto(s.clone(), &ep));
            }
            item
        })
        .collect();

    let total = items.len() as i64;
    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        start_index: q
            .start_index
            .unwrap_or(0),
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

// --------------------------------------------------------------------------
// GET /shows/recommendations
// --------------------------------------------------------------------------

#[api_query]
#[derive(Debug, Default)]
pub struct GetShowRecommendationsQuery {
    pub user_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub category_limit: Option<u32>,
    pub item_limit: Option<u32>,
}

#[get("/shows/recommendations")]
pub async fn shows_recommendations(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<GetShowRecommendationsQuery>,
) -> Result<impl IntoResponse> {
    let user_id = q
        .user_id
        .unwrap_or(
            session
                .user
                .id,
        );
    let categories = super::movies::build_recommendations(
        &state
            .ctx
            .db,
        user_id,
        q.parent_id,
        db::MediaKind::Series,
        q.category_limit
            .unwrap_or(5) as usize,
        q.item_limit
            .unwrap_or(8),
    )
    .await?;
    Ok(Json(categories))
}

#[cfg(test)]
mod test {
    use super::*;
    use chrono::{NaiveDate, NaiveDateTime};
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

    async fn insert_series_with_episodes(
        db: &SqlitePool,
        series_title: &str,
        episode_titles: &[&str],
    ) -> (db::Media, Vec<db::Media>) {
        let series_imdb = db::NonEmptyString::try_new(format!(
            "tt{}",
            series_title
                .bytes()
                .fold(0_u32, |acc, byte| acc
                    .wrapping_mul(31)
                    .wrapping_add(byte as u32))
        ))
        .unwrap();
        let mut series = db::Media {
            id: Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: series_title.to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let mut season = db::Media {
            id: Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: None,
            }),
            title: format!("{series_title} Season 1"),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        let mut episodes = Vec::with_capacity(episode_titles.len());
        for (idx, title) in episode_titles
            .iter()
            .enumerate()
        {
            let mut episode = db::Media {
                id: Uuid::from(&db::MediaIdRaw {
                    kind: db::MediaKind::Episode,
                    external_ids: db::ExternalIds {
                        series_imdb: Some(series_imdb.clone()),
                        ..Default::default()
                    },
                    season: Some(1),
                    episode: Some(idx as i64 + 1),
                }),
                title: (*title).to_string(),
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                grandparent_id: Some(series.id),
                parent_id: Some(season.id),
                parent_idx: Some(1),
                idx: Some(idx as i64 + 1),
                ..Default::default()
            };
            episode
                .save(db)
                .await
                .unwrap();
            episodes.push(episode);
        }

        (series, episodes)
    }

    async fn insert_user(db: &SqlitePool, username: &str) -> db::User {
        let mut user = db::User {
            username: username.to_string(),
            password_hash: "test".to_string(),
            ..Default::default()
        };
        user.save(db)
            .await
            .unwrap();
        user
    }

    async fn insert_state(
        db: &SqlitePool,
        user_id: Uuid,
        media_id: Uuid,
        play_count: i64,
        playback_position: i64,
        played_at: Option<NaiveDateTime>,
        last_played_at: Option<NaiveDateTime>,
    ) {
        sqlx::query(
            r#"
            INSERT INTO user_media_state (
                user_id,
                media_id,
                media_raw,
                stream_id,
                favorite,
                play_count,
                played_at,
                playback_position,
                last_played_at,
                subtitle_idx,
                audio_idx
            )
            VALUES (?1, ?2, NULL, NULL, 0, ?3, ?4, ?5, ?6, NULL, NULL)
            ON CONFLICT(user_id, media_id)
            DO UPDATE SET
                play_count = excluded.play_count,
                played_at = excluded.played_at,
                playback_position = excluded.playback_position,
                last_played_at = excluded.last_played_at
            "#,
        )
        .bind(user_id)
        .bind(media_id)
        .bind(play_count)
        .bind(played_at)
        .bind(playback_position)
        .bind(last_played_at)
        .execute(db)
        .await
        .unwrap();
    }

    async fn active_series_ids(
        db: &SqlitePool,
        user_id: Uuid,
        cutoff: Option<&str>,
    ) -> Vec<Uuid> {
        let date_cutoff = cutoff
            .map(api::normalize_next_up_date_cutoff)
            .transpose()
            .unwrap()
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
        let active_series: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT m.grandparent_id \
             FROM ( \
               SELECT media_id, last_played_at, played_at \
               FROM user_media_state WHERE user_id = ? AND play_count > 0 \
               UNION ALL \
               SELECT media_id, last_played_at, played_at \
               FROM user_media_state WHERE user_id = ? AND play_count = 0 AND playback_position > 0 \
             ) AS active \
             JOIN media m ON m.id = active.media_id \
             WHERE m.kind = 'episode' \
             AND m.grandparent_id IS NOT NULL \
             GROUP BY m.grandparent_id \
             HAVING MAX(COALESCE(active.last_played_at, active.played_at)) >= ? \
             ORDER BY MAX(COALESCE(active.last_played_at, active.played_at)) DESC \
             LIMIT ?",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(&date_cutoff)
        .bind(50_i64)
        .fetch_all(db)
        .await
        .unwrap();

        active_series
            .into_iter()
            .map(|(id,)| id)
            .collect()
    }

    #[tokio::test]
    async fn shows_nextup_orders_by_last_played_desc() {
        let db = test_db().await;
        let user = insert_user(&db, "test").await;

        let (series_a, episodes_a) = insert_series_with_episodes(
            &db,
            "Series A",
            &["A Episode 1", "A Episode 2"],
        )
        .await;
        let (series_b, episodes_b) = insert_series_with_episodes(
            &db,
            "Series B",
            &["B Episode 1", "B Episode 2"],
        )
        .await;

        insert_state(
            &db,
            user.id,
            episodes_a[0].id,
            1,
            0,
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 16)
                    .unwrap()
                    .and_hms_opt(8, 0, 0)
                    .unwrap(),
            ),
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 16)
                    .unwrap()
                    .and_hms_opt(8, 0, 0)
                    .unwrap(),
            ),
        )
        .await;
        insert_state(
            &db,
            user.id,
            episodes_b[0].id,
            1,
            0,
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 17)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 17)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
        )
        .await;

        assert_eq!(
            active_series_ids(&db, user.id, None).await,
            vec![series_b.id, series_a.id],
        );
    }

    #[tokio::test]
    async fn shows_nextup_accepts_rfc3339_cutoff() {
        let db = test_db().await;
        let user = insert_user(&db, "test").await;

        let (_series_old, old_episodes) = insert_series_with_episodes(
            &db,
            "Old Series",
            &["Old Episode 1", "Old Episode 2"],
        )
        .await;
        let (new_series, new_episodes) = insert_series_with_episodes(
            &db,
            "New Series",
            &["New Episode 1", "New Episode 2"],
        )
        .await;

        insert_state(
            &db,
            user.id,
            old_episodes[0].id,
            1,
            0,
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 17)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 17)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
        )
        .await;
        insert_state(
            &db,
            user.id,
            new_episodes[0].id,
            1,
            0,
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 18)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 18)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
        )
        .await;

        assert_eq!(
            active_series_ids(&db, user.id, Some("2026-06-17T23:00:00Z")).await,
            vec![new_series.id],
        );
    }

    #[tokio::test]
    async fn shows_nextup_falls_back_to_played_at_when_last_played_at_is_null() {
        let db = test_db().await;
        let user = insert_user(&db, "test").await;

        let (legacy_series, legacy_episodes) = insert_series_with_episodes(
            &db,
            "Legacy Series",
            &["Legacy Episode 1", "Legacy Episode 2"],
        )
        .await;
        let (_newer_series, newer_episodes) = insert_series_with_episodes(
            &db,
            "Newer Series",
            &["Newer Episode 1", "Newer Episode 2"],
        )
        .await;

        insert_state(
            &db,
            user.id,
            legacy_episodes[0].id,
            1,
            0,
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 18)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
            None,
        )
        .await;
        insert_state(
            &db,
            user.id,
            newer_episodes[0].id,
            1,
            0,
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 17)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
            Some(
                NaiveDate::from_ymd_opt(2026, 6, 17)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
        )
        .await;

        assert_eq!(
            active_series_ids(&db, user.id, Some("2026-06-18")).await,
            vec![legacy_series.id],
        );
    }
}
