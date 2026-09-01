use std::collections::HashMap;

use axum::{Json, extract::State, response::IntoResponse};
use axum_extra::extract::Query;
use remux_macros::{get, query};
use uuid::Uuid;

use crate::{
    AppState, OptionExt, api, db,
    db::{auth, media::push_release_date_filter},
    services::resolve::ResolvedItem,
};
use axum_anyhow::ApiResult as Result;

use super::items::get_items;

pub fn livetv_view_id() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, b"remux-livetv-view")
}

pub fn livetv_view_item() -> api::BaseItemDto {
    api::BaseItemDto {
        id: livetv_view_id(),
        name: Some("Live TV".to_string()),
        server_id: crate::common::server_id(),
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
    ResolvedItem(item): ResolvedItem,
    Query(mut q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.parent_id = Some(item.id);
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
    ResolvedItem(item): ResolvedItem,
    Query(mut q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // Some Jellyfin clients accidentally pass the season ID as the show ID in the path.
    // If season_id is given, it's sufficient on its own (maps to parent_id in get_items),
    // so skip setting series_id to avoid filtering by the wrong ID.
    if q.season_id
        .is_none()
    {
        q.series_id = Some(item.id);
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

    let server_config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let release_threshold = server_config.release_date_threshold();

    // All released episodes for the series in watch order (season asc, episode asc)
    let mut ep_qb =
        sqlx::QueryBuilder::new("SELECT * FROM media WHERE grandparent_id = ");
    ep_qb
        .push_bind(grandparent_id)
        .push(" AND kind = 'episode'");
    if let Some(t) = release_threshold {
        push_release_date_filter(&mut ep_qb, "media", t, true);
    }
    ep_qb.push(" ORDER BY COALESCE(parent_idx, 9999) ASC, COALESCE(idx, 9999) ASC");
    let episodes: Vec<db::Media> = ep_qb
        .build_query_as()
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
        .map(|l| l as usize);
    let start_index = q
        .start_index
        .unwrap_or(0) as usize;
    let enable_resumable = q
        .enable_resumable
        .unwrap_or(true);

    let server_config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let release_threshold = server_config.release_date_threshold();

    // Inner UNION selects last_played_at/played_at directly from idx_ums_user_play_state
    // (covering) so no second join to user_media_state is needed. UNION ALL is safe
    // because the two legs are mutually exclusive (play_count > 0 vs play_count = 0).
    // CROSS JOIN pins SQLite to start from the small active set and then PK-lookup
    // media rows, avoiding a slow scan over the full episode index.
    // No series-count LIMIT here — we apply the page limit to the final episode list
    // (matching Jellyfin's approach: consider all active series, paginate results).
    let date_cutoff = q
        .next_up_date_cutoff
        .clone()
        .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
    let active_series: Vec<(Uuid, Option<chrono::NaiveDateTime>)> = sqlx::query_as(
        "SELECT m.grandparent_id, \
                MAX(COALESCE(active.last_played_at, active.played_at, '1970-01-01 00:00:00')) AS last_activity \
         FROM ( \
           SELECT media_id, last_played_at, played_at \
           FROM user_media_state WHERE user_id = ? AND play_count > 0 \
           UNION ALL \
           SELECT media_id, last_played_at, played_at \
           FROM user_media_state WHERE user_id = ? AND play_count = 0 AND playback_position > 0 \
         ) AS active \
         CROSS JOIN media m ON m.id = active.media_id \
         WHERE m.kind = 'episode' \
         AND m.grandparent_id IS NOT NULL \
         GROUP BY m.grandparent_id \
         HAVING last_activity >= ? \
         ORDER BY last_activity DESC",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(&date_cutoff)
    .fetch_all(&state.ctx.db)
    .await?;

    if active_series.is_empty() {
        return Ok(Json(api::BaseItemDtoQueryResult::default()).into_response());
    }

    let mut series_ids: Vec<Uuid> = Vec::with_capacity(active_series.len());
    let mut series_last_activity: HashMap<Uuid, chrono::NaiveDateTime> =
        HashMap::with_capacity(active_series.len());
    for (id, last_activity) in active_series {
        series_ids.push(id);
        if let Some(ts) = last_activity {
            series_last_activity.insert(id, ts);
        }
    }

    let mut ep_qb =
        sqlx::QueryBuilder::new("SELECT * FROM media WHERE grandparent_id IN (");
    {
        let mut sep = ep_qb.separated(", ");
        for id in &series_ids {
            sep.push_bind(id);
        }
    }
    ep_qb.push(") AND kind = 'episode'");
    if let Some(t) = release_threshold {
        push_release_date_filter(&mut ep_qb, "media", t, true);
    }
    ep_qb.push(
        " ORDER BY grandparent_id, COALESCE(parent_idx, 9999) ASC, COALESCE(idx, 9999) ASC",
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
                let candidate = episodes.get(pos + 1);
                // Mirror Jellyfin: when EnableResumable=false, skip a candidate that
                // is already in-progress — the user should resume or mark it watched.
                next_ep = if !enable_resumable {
                    candidate.filter(|e| {
                        !state_for(e).map_or(false, |s| s.playback_position > 0)
                    })
                } else {
                    candidate
                };
            }
        }

        if let Some(ep) = next_ep {
            next_eps.push(ep.clone());
        }
    }

    // Re-sort: if next ep released more recently than the user's last watch,
    // use the release date as the effective key so fresh episodes surface first.
    let epoch = chrono::NaiveDateTime::parse_from_str(
        "1970-01-01 00:00:00",
        "%Y-%m-%d %H:%M:%S",
    )
    .unwrap();
    next_eps.sort_by(|a, b| {
        let key = |ep: &db::Media| {
            let release = ep
                .digital_released_at
                .or(ep.released_at)
                .unwrap_or(epoch);
            let activity = ep
                .grandparent_id
                .and_then(|gid| {
                    series_last_activity
                        .get(&gid)
                        .copied()
                })
                .unwrap_or(epoch);
            release.max(activity)
        };
        key(b).cmp(&key(a))
    });

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

    let total = next_eps.len() as i64;
    let items: Vec<api::BaseItemDto> = next_eps
        .into_iter()
        .skip(start_index)
        .take(limit.unwrap_or(usize::MAX))
        .map(|ep| {
            let mut item = api::db_media_to_item(ep.clone(), false);
            if let Some(s) = states_map.get(&ep.id) {
                item.user_data = Some(api::db_state_to_dto(s.clone(), &ep));
            }
            item
        })
        .collect();

    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        start_index: start_index as u32,
        ..Default::default()
    })
    .into_response())
}

/// Upcoming episodes sorted by premiere date, soonest first.
/// The digital_released_before filter is intentionally skipped so episodes
/// that have aired but are not yet digitally available are included.
#[get("/shows/upcoming")]
pub async fn shows_upcoming(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let user_id = q
        .user_id
        .unwrap_or(
            session
                .user
                .id,
        );

    // Use start-of-today so episodes stored as midnight UTC on today's date are included.
    let today = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap();

    // Resolve the ParentId if provided. Clients often send the TV library view UUID
    // (a Collection/Folder). We map it to the right filter field:
    //   Manual collection → fetch series via media_relations (role='collection') → grandparent_ids
    //   Series            → grandparent_id
    //   Season            → parent_id
    //   not in DB (virtual view UUID) → no filter
    let mut episode_parent_id: Option<uuid::Uuid> = None;
    let mut episode_grandparent_id: Option<uuid::Uuid> = None;
    let mut episode_grandparent_ids: Option<Vec<uuid::Uuid>> = None;
    if let Some(pid) = q.parent_id {
        use crate::services::resolve::MediaResolveService;
        if let Some(parent) = MediaResolveService::resolve_item(pid, &state.ctx).await?
        {
            match parent.kind {
                db::MediaKind::Series => episode_grandparent_id = Some(parent.id),
                db::MediaKind::Season => episode_parent_id = Some(parent.id),
                _ => {
                    // Collection/Folder: get the series it contains and scope episodes to those.
                    let series = db::MediaRelation::get_collection_items(
                        &state
                            .ctx
                            .db,
                        &parent.id,
                    )
                    .await?;
                    let ids: Vec<uuid::Uuid> = series
                        .into_iter()
                        .map(|s| s.right_media_id)
                        .collect();
                    if !ids.is_empty() {
                        episode_grandparent_ids = Some(ids);
                    }
                }
            }
        }
    }

    let policy = session
        .user
        .policy
        .as_ref();
    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Episode]),
            parent_id: episode_parent_id,
            grandparent_id: episode_grandparent_id,
            grandparent_ids: episode_grandparent_ids,
            released_after: Some(today),
            digital_released_before: None,
            limit: q.limit,
            offset: q.start_index,
            include_user_state: true,
            user_id: Some(user_id),
            sort_by: vec![api::ItemSortBy::PremiereDate],
            sort_order: vec![api::SortOrder::Ascending],
            total_count: q
                .enable_total_record_count
                .unwrap_or(true),
            max_parental_rating: policy.and_then(|p| p.max_parental_rating),
            blocked_tags: policy
                .map(|p| {
                    p.blocked_tags
                        .clone()
                })
                .filter(|v| !v.is_empty()),
            allowed_tags: policy
                .map(|p| {
                    p.allowed_tags
                        .clone()
                })
                .filter(|v| !v.is_empty()),
            policy_filter: policy
                .and_then(|p| {
                    p.filter_rules
                        .as_ref()
                })
                .cloned(),
            ..Default::default()
        },
    )
    .await?;

    let items = result
        .records
        .into_iter()
        .map(|m| api::db_media_to_item(m, false))
        .collect::<Vec<_>>();

    Ok(Json(api::BaseItemDtoQueryResult {
        total_record_count: result.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0),
        items,
        ..Default::default()
    }))
}

// --------------------------------------------------------------------------
// GET /shows/recommendations
// --------------------------------------------------------------------------

#[query]
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
        let imdb = db::NonEmptyString::try_new(format!(
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
                    imdb: Some(imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: series_title.to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(imdb),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let season_id = crate::common::stable_media_uuid(
            &db::MediaKind::Season,
            &format!("{}:1", series.id),
        );
        let mut season = db::Media {
            id: season_id,
            title: format!("{series_title} Season 1"),
            kind: db::MediaKind::Season,
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
            let ep_num = idx as i64 + 1;
            let mut episode = db::Media {
                id: crate::common::stable_media_uuid(
                    &db::MediaKind::Episode,
                    &format!("{}:{ep_num}", season_id),
                ),
                title: (*title).to_string(),
                kind: db::MediaKind::Episode,
                grandparent_id: Some(series.id),
                parent_id: Some(season.id),
                parent_idx: Some(1),
                idx: Some(ep_num),
                digital_released_at: Some(
                    NaiveDate::from_ymd_opt(2020, 1, 1)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                ),
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

    #[tokio::test]
    async fn shows_nextup_new_episode_bubbles_to_top() {
        // Series A: user watched finale 3 months ago; next ep released yesterday → should surface first.
        // Series B: user watched last week; next ep has no release date → stays in normal position.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::{Duration, Utc};
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let now = Utc::now().naive_utc();

        // Series A — finished 3 months ago, new ep released yesterday
        let (series_a, mut eps_a) =
            insert_series_with_episodes(db, "Series A Bubble", &["A Ep 1", "A Ep 2"])
                .await;

        // Series B — watched last week, no release date on next ep
        let (series_b, eps_b) =
            insert_series_with_episodes(db, "Series B Bubble", &["B Ep 1", "B Ep 2"])
                .await;

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Mark A E1 played 3 months ago
        insert_state(
            db,
            user.id,
            eps_a[0].id,
            1,
            0,
            Some(now - Duration::days(90)),
            Some(now - Duration::days(90)),
        )
        .await;
        // Set A E2 (next ep) digital_released_at to yesterday
        eps_a[1].digital_released_at = Some(now - Duration::days(1));
        eps_a[1]
            .save(db)
            .await
            .unwrap();

        // Mark B E1 played last week
        insert_state(
            db,
            user.id,
            eps_b[0].id,
            1,
            0,
            Some(now - Duration::days(7)),
            Some(now - Duration::days(7)),
        )
        .await;
        // Clear release date on B E2 so effective key falls back to last_activity
        let mut ep_b2 = eps_b[1].clone();
        ep_b2.digital_released_at = None;
        ep_b2.released_at = None;
        ep_b2
            .save(db)
            .await
            .unwrap();

        let resp = server
            .get("/shows/nextup")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        let id_strs: Vec<&str> = items
            .iter()
            .map(|i| {
                i["Id"]
                    .as_str()
                    .unwrap()
            })
            .collect();

        let ep_a2_str = eps_a[1]
            .id
            .simple()
            .to_string();
        let ep_b2_str = ep_b2
            .id
            .simple()
            .to_string();

        let pos_a = id_strs
            .iter()
            .position(|&s| s == ep_a2_str);
        let pos_b = id_strs
            .iter()
            .position(|&s| s == ep_b2_str);

        assert!(pos_a.is_some(), "Series A next ep missing from NextUp");
        assert!(pos_b.is_some(), "Series B next ep missing from NextUp");
        assert!(
            pos_a < pos_b,
            "Series A (new ep released yesterday) should appear before Series B (watched last week), \
             got pos_a={pos_a:?} pos_b={pos_b:?}. series_a={}, series_b={}",
            series_a.id,
            series_b.id,
        );
    }

    // -----------------------------------------------------------------------
    // Integration tests: full HTTP handler (requires test server + real auth)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn nextup_returns_episode_after_last_played_default_enable_resumable() {
        // Reproduces the "NextUp returns no results but should" bug.
        // A user has fully watched episode 1 of a series. NextUp (with the
        // default EnableResumable=true) must return episode 2.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let now = Utc::now().naive_utc();

        let (series, episodes) =
            insert_series_with_episodes(db, "TestSeries", &["Ep1", "Ep2", "Ep3"]).await;

        // Identify the authed user so we can insert state for them.
        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Episode 1 was fully watched.
        insert_state(
            db,
            user.id,
            episodes[0].id,
            1, // play_count = 1
            0, // position = 0 (reset after completion)
            Some(now),
            Some(now),
        )
        .await;

        // Default path: no EnableResumable param → defaults to true.
        let resp = server
            .get("/shows/nextup")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        // Must return exactly one item: episode 2 of this series.
        assert_eq!(items.len(), 1, "NextUp should return episode 2");
        assert_eq!(
            items[0]["Id"]
                .as_str()
                .unwrap(),
            episodes[1]
                .id
                .simple()
                .to_string(),
            "NextUp should return episode 2, not {:?}",
            items[0]["Name"]
        );

        // Sanity-check: the series UUID should be reachable via the result.
        let _ = series.id;
    }

    #[tokio::test]
    async fn nextup_enable_resumable_false_returns_episode_after_last_played() {
        // Companion to the above: explicit EnableResumable=false must also find
        // episode 2 after episode 1 is played.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let now = Utc::now().naive_utc();

        let (_series, episodes) =
            insert_series_with_episodes(db, "TestSeries2", &["Ep1", "Ep2", "Ep3"])
                .await;

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        insert_state(db, user.id, episodes[0].id, 1, 0, Some(now), Some(now)).await;

        let resp = server
            .get("/shows/nextup?EnableResumable=false")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        assert_eq!(
            items.len(),
            1,
            "NextUp (EnableResumable=false) should return episode 2"
        );
        assert_eq!(
            items[0]["Id"]
                .as_str()
                .unwrap(),
            episodes[1]
                .id
                .simple()
                .to_string(),
        );
    }

    #[tokio::test]
    async fn nextup_in_progress_episode_returned_when_enable_resumable_true() {
        // When the user has only started (not completed) episode 1 and
        // EnableResumable=true (default), NextUp should return episode 1.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let now = Utc::now().naive_utc();

        let (_series, episodes) =
            insert_series_with_episodes(db, "TestSeries3", &["Ep1", "Ep2"]).await;

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Episode 1 is in-progress (started but not completed).
        insert_state(
            db,
            user.id,
            episodes[0].id,
            0,    // play_count = 0 (not finished)
            1800, // position = 30 min in
            None,
            Some(now),
        )
        .await;

        let resp = server
            .get("/shows/nextup")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0]["Id"]
                .as_str()
                .unwrap(),
            episodes[0]
                .id
                .simple()
                .to_string(),
            "In-progress episode 1 should be returned as NextUp when EnableResumable=true"
        );
    }

    #[tokio::test]
    async fn nextup_with_null_play_dates_still_returns_results() {
        // Reproduces the core bug: play state imported from Jellyfin may have
        // play_count=1 but both played_at and last_played_at as NULL (Jellyfin
        // doesn't always export LastPlayedDate). The active_series SQL query uses
        // HAVING MAX(COALESCE(last_played_at, played_at)) >= ?, and NULL >= ?
        // is always false in SQLite, so those series are silently dropped.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let (_series, episodes) =
            insert_series_with_episodes(db, "NullDateSeries", &["Ep1", "Ep2", "Ep3"])
                .await;

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Episode 1 played but with NULL dates (e.g. imported from Jellyfin without LastPlayedDate).
        insert_state(
            db,
            user.id,
            episodes[0].id,
            1,    // play_count = 1
            0,    // position = 0
            None, // played_at = NULL
            None, // last_played_at = NULL
        )
        .await;

        let resp = server
            .get("/shows/nextup")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        // Should return episode 2 — the null dates must not silently drop the series.
        assert_eq!(
            items.len(),
            1,
            "NextUp must return results even when play dates are NULL (Jellyfin import case)"
        );
        assert_eq!(
            items[0]["Id"]
                .as_str()
                .unwrap(),
            episodes[1]
                .id
                .simple()
                .to_string()
        );
    }

    #[tokio::test]
    async fn nextup_skips_unreleased_episodes_when_release_date_filter_enabled() {
        // Regression for #35: episodes with a future digital_released_at must not
        // appear in Next Up even when the user has finished everything released so far.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        // Enable the release-date filter with a 0-day buffer (strict: only past/today).
        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let future = now + chrono::Duration::days(30);

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: db::NonEmptyString::try_new("tt9999991".to_string()).ok(),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "FutureSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt9999991".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let season_id = crate::common::stable_media_uuid(
            &db::MediaKind::Season,
            &format!("{}:1", series.id),
        );
        let mut season = db::Media {
            id: season_id,
            title: "FutureSeries Season 1".to_string(),
            kind: db::MediaKind::Season,
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        // Ep1: already released and played.
        let mut ep1 = db::Media {
            id: crate::common::stable_media_uuid(
                &db::MediaKind::Episode,
                &format!("{}:1", season_id),
            ),
            title: "Ep1".to_string(),
            kind: db::MediaKind::Episode,
            grandparent_id: Some(series.id),
            parent_id: Some(season.id),
            parent_idx: Some(1),
            idx: Some(1),
            digital_released_at: Some(now - chrono::Duration::days(7)),
            ..Default::default()
        };
        ep1.save(db)
            .await
            .unwrap();

        // Ep2: not yet released — must be hidden from Next Up.
        let mut ep2 = db::Media {
            id: crate::common::stable_media_uuid(
                &db::MediaKind::Episode,
                &format!("{}:2", season_id),
            ),
            title: "Ep2 (unreleased)".to_string(),
            kind: db::MediaKind::Episode,
            grandparent_id: Some(series.id),
            parent_id: Some(season.id),
            parent_idx: Some(1),
            idx: Some(2),
            digital_released_at: Some(future),
            ..Default::default()
        };
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        insert_state(db, user.id, ep1.id, 1, 0, Some(now), Some(now)).await;

        let resp = server
            .get("/shows/nextup")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        assert_eq!(
            items.len(),
            0,
            "NextUp must not return unreleased episode Ep2; got: {:?}",
            items
        );
    }

    /// An episode with no air date (`digital_released_at = NULL`, `released_at = NULL`)
    /// must not appear in Next Up for its series. Anime series on TVDB often have upcoming
    /// seasons with no scheduled air date. Currently fails because `push_episode_date_filter`
    /// falls back to `'1900-01-01'` for NULL dates, treating them as already released.
    #[tokio::test]
    async fn nextup_excludes_null_air_date_episode() {
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let now = Utc::now().naive_utc();

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: db::NonEmptyString::try_new("tt_null_nup_001".to_string())
                        .ok(),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "NullDateNextUpSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt_null_nup_001".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let season_id = crate::common::stable_media_uuid(
            &db::MediaKind::Season,
            &format!("{}:1", series.id),
        );
        let mut season = db::Media {
            id: season_id,
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        // ep1: released and played
        let mut ep1 = db::Media {
            id: crate::common::stable_media_uuid(
                &db::MediaKind::Episode,
                &format!("{}:1", season_id),
            ),
            title: "Ep1".to_string(),
            kind: db::MediaKind::Episode,
            grandparent_id: Some(series.id),
            parent_id: Some(season.id),
            parent_idx: Some(1),
            idx: Some(1),
            digital_released_at: Some(now - chrono::Duration::days(7)),
            ..Default::default()
        };
        ep1.save(db)
            .await
            .unwrap();

        // ep2: no air date — upcoming anime episode with no scheduled release
        let mut ep2 = db::Media {
            id: crate::common::stable_media_uuid(
                &db::MediaKind::Episode,
                &format!("{}:2", season_id),
            ),
            title: "Ep2 (no air date)".to_string(),
            kind: db::MediaKind::Episode,
            grandparent_id: Some(series.id),
            parent_id: Some(season.id),
            parent_idx: Some(1),
            idx: Some(2),
            digital_released_at: None,
            released_at: None,
            ..Default::default()
        };
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Mark ep1 as fully watched.
        insert_state(db, user.id, ep1.id, 1, 0, Some(now), Some(now)).await;

        // Query series-scoped Next Up — ep2 must not appear (NULL date = unreleased).
        let resp = server
            .get(&format!("/shows/nextup?SeriesId={}", series.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        assert_eq!(
            items.len(),
            0,
            "null-date episode must not appear in Next Up; got: {:?}",
            items
        );
    }

    // -----------------------------------------------------------------------
    // Integration tests: /shows/upcoming
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn upcoming_returns_episodes_with_released_at_today_or_future() {
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let now = Utc::now().naive_utc();
        let today_midnight = now
            .date()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let future = today_midnight + chrono::Duration::days(7);
        let past = today_midnight - chrono::Duration::days(7);

        let (_, episodes_today) =
            insert_series_with_episodes(db, "Upcoming Today Series", &["Ep Today"])
                .await;
        let (_, episodes_future) =
            insert_series_with_episodes(db, "Upcoming Future Series", &["Ep Future"])
                .await;
        let (_, episodes_past) =
            insert_series_with_episodes(db, "Past Series", &["Ep Past"]).await;

        // Set released_at directly (insert_series_with_episodes sets digital_released_at to 2020-01-01).
        // Override released_at for our test episodes.
        sqlx::query("UPDATE media SET released_at = ? WHERE id = ?")
            .bind(today_midnight)
            .bind(episodes_today[0].id)
            .execute(db)
            .await
            .unwrap();
        sqlx::query("UPDATE media SET released_at = ? WHERE id = ?")
            .bind(future)
            .bind(episodes_future[0].id)
            .execute(db)
            .await
            .unwrap();
        sqlx::query("UPDATE media SET released_at = ? WHERE id = ?")
            .bind(past)
            .bind(episodes_past[0].id)
            .execute(db)
            .await
            .unwrap();

        let resp = server
            .get("/shows/upcoming")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();
        let ids: Vec<&str> = items
            .iter()
            .map(|i| {
                i["Id"]
                    .as_str()
                    .unwrap()
            })
            .collect();

        let today_id = episodes_today[0]
            .id
            .simple()
            .to_string();
        let future_id = episodes_future[0]
            .id
            .simple()
            .to_string();
        let past_id = episodes_past[0]
            .id
            .simple()
            .to_string();

        assert!(
            ids.contains(&today_id.as_str()),
            "today's episode must appear in /shows/upcoming; got: {:?}",
            ids
        );
        assert!(
            ids.contains(&future_id.as_str()),
            "future episode must appear in /shows/upcoming; got: {:?}",
            ids
        );
        assert!(
            !ids.contains(&past_id.as_str()),
            "past episode must NOT appear in /shows/upcoming; got: {:?}",
            ids
        );
    }

    #[tokio::test]
    async fn upcoming_virtual_parent_id_returns_all_not_empty() {
        // Clients send ParentId = TV library view UUID (not in DB). Must return episodes,
        // not empty — the virtual UUID must not be used as a filter.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let now = Utc::now().naive_utc();
        let today = now
            .date()
            .and_hms_opt(0, 0, 0)
            .unwrap();

        let (_, episodes) =
            insert_series_with_episodes(db, "Upcoming Virtual Parent Test", &["Ep1"])
                .await;
        sqlx::query("UPDATE media SET released_at = ? WHERE id = ?")
            .bind(today)
            .bind(episodes[0].id)
            .execute(db)
            .await
            .unwrap();

        // This UUID does not exist in the DB — it's the virtual TV library view.
        let virtual_uuid =
            uuid::Uuid::parse_str("a1b2c3d4-0000-4000-8000-000000000002").unwrap();

        let resp = server
            .get(&format!("/shows/upcoming?ParentId={virtual_uuid}"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();
        let ids: Vec<&str> = items
            .iter()
            .map(|i| {
                i["Id"]
                    .as_str()
                    .unwrap()
            })
            .collect();
        let ep_id = episodes[0]
            .id
            .simple()
            .to_string();

        assert!(
            ids.contains(&ep_id.as_str()),
            "/shows/upcoming with virtual ParentId must not return empty; got: {:?}",
            ids
        );
    }

    #[tokio::test]
    async fn upcoming_collection_parent_id_scopes_to_collection_series() {
        // When ParentId is a real collection, only return episodes from series in that collection.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let now = Utc::now().naive_utc();
        let today = now
            .date()
            .and_hms_opt(0, 0, 0)
            .unwrap();

        let (series_in, eps_in) =
            insert_series_with_episodes(db, "In Collection Series", &["In Ep"]).await;
        let (_, eps_out) =
            insert_series_with_episodes(db, "Not In Collection Series", &["Out Ep"])
                .await;

        // Set both episodes to today.
        for ep in [&eps_in[0], &eps_out[0]] {
            sqlx::query("UPDATE media SET released_at = ? WHERE id = ?")
                .bind(today)
                .bind(ep.id)
                .execute(db)
                .await
                .unwrap();
        }

        // Create a collection and add only series_in to it.
        let collection_id = uuid::Uuid::new_v4();
        let mut collection = db::Media {
            id: collection_id,
            title: "Test Collection".to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Manual),
            ..Default::default()
        };
        collection
            .save(db)
            .await
            .unwrap();
        db::MediaRelation::add_collection_items(db, &collection_id, &[series_in.id])
            .await
            .unwrap();

        let resp = server
            .get(&format!("/shows/upcoming?ParentId={collection_id}"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();
        let ids: Vec<&str> = items
            .iter()
            .map(|i| {
                i["Id"]
                    .as_str()
                    .unwrap()
            })
            .collect();

        let ep_in_id = eps_in[0]
            .id
            .simple()
            .to_string();
        let ep_out_id = eps_out[0]
            .id
            .simple()
            .to_string();

        assert!(
            ids.contains(&ep_in_id.as_str()),
            "episode from collection series must appear; got: {:?}",
            ids
        );
        assert!(
            !ids.contains(&ep_out_id.as_str()),
            "episode NOT in collection must not appear; got: {:?}",
            ids
        );
    }

    #[tokio::test]
    async fn upcoming_skips_digital_released_before_filter() {
        // Even when the release-date gate is on, /shows/upcoming must not hide
        // episodes that have aired but aren't yet digitally available.
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        // Enable the release-date filter.
        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let today = now
            .date()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let future_digital = today + chrono::Duration::days(14); // digital release in 2 weeks

        let (_, episodes) =
            insert_series_with_episodes(db, "Aired Not Digital Series", &["Ep Aired"])
                .await;

        // released_at = today (aired), but digital_released_at = 2 weeks away.
        sqlx::query(
            "UPDATE media SET released_at = ?, digital_released_at = ? WHERE id = ?",
        )
        .bind(today)
        .bind(future_digital)
        .bind(episodes[0].id)
        .execute(db)
        .await
        .unwrap();

        let resp = server
            .get("/shows/upcoming")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();
        let ids: Vec<&str> = items
            .iter()
            .map(|i| {
                i["Id"]
                    .as_str()
                    .unwrap()
            })
            .collect();
        let ep_id = episodes[0]
            .id
            .simple()
            .to_string();

        assert!(
            ids.contains(&ep_id.as_str()),
            "/shows/upcoming must include aired episode even when digital_released_at is in the future; got: {:?}",
            ids
        );
    }

    /// Season with a future `digital_released_at` set directly on the season row.
    #[tokio::test]
    async fn seasons_hides_unreleased_season() {
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();
        let now = Utc::now().naive_utc();
        let past = now - chrono::Duration::days(30);
        let future = now + chrono::Duration::days(30);

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: db::NonEmptyString::try_new(
                        "tt_seasons_unreleased_001".to_string(),
                    )
                    .ok(),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "TestSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new(
                    "tt_seasons_unreleased_001".to_string(),
                )
                .ok(),
                ..Default::default()
            },
            digital_released_at: Some(past),
            released_at: Some(past),
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        // Season 1: already released.
        let mut season1 = db::Media {
            id: crate::common::stable_media_uuid(
                &db::MediaKind::Season,
                &format!("{}:1", series.id),
            ),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            digital_released_at: Some(past),
            released_at: Some(past),
            ..Default::default()
        };
        season1
            .save(db)
            .await
            .unwrap();

        // Season 2: premiere in the future — must be hidden.
        let mut season2 = db::Media {
            id: crate::common::stable_media_uuid(
                &db::MediaKind::Season,
                &format!("{}:2", series.id),
            ),
            title: "Season 2".to_string(),
            kind: db::MediaKind::Season,
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(2),
            digital_released_at: Some(future),
            released_at: Some(future),
            ..Default::default()
        };
        season2
            .save(db)
            .await
            .unwrap();

        let resp = server
            .get(&format!(
                "/shows/{}/seasons?userId={}",
                series.id, series.id
            ))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        assert_eq!(
            items.len(),
            1,
            "only the released Season 1 should appear; got: {:?}",
            items
        );
        assert_eq!(
            items[0]["IndexNumber"]
                .as_i64()
                .unwrap(),
            1,
            "the returned season must be Season 1"
        );
    }

    /// Season with NULL dates and no released episodes must be hidden (upcoming
    /// TVDB season with no scheduled date should not inherit the series premiere).
    #[tokio::test]
    async fn seasons_hides_null_date_season_with_no_released_episodes() {
        use crate::integration_test::{auth_header_with_token, authenticated_server};
        use chrono::Utc;
        use http::header::HeaderValue;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let past = now - chrono::Duration::days(365);

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: db::NonEmptyString::try_new(
                        "tt_seasons_null_tvdb_001".to_string(),
                    )
                    .ok(),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "NullTvdbSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new(
                    "tt_seasons_null_tvdb_001".to_string(),
                )
                .ok(),
                ..Default::default()
            },
            digital_released_at: Some(past),
            released_at: Some(past),
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let mut season1 = db::Media {
            id: crate::common::stable_media_uuid(
                &db::MediaKind::Season,
                &format!("{}:1", series.id),
            ),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            digital_released_at: Some(past),
            released_at: Some(past),
            ..Default::default()
        };
        season1
            .save(db)
            .await
            .unwrap();

        // Season 2: TVDB knows it exists but has no air date yet.
        let mut season2 = db::Media {
            id: crate::common::stable_media_uuid(
                &db::MediaKind::Season,
                &format!("{}:2", series.id),
            ),
            title: "Season 2".to_string(),
            kind: db::MediaKind::Season,
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(2),
            digital_released_at: None,
            released_at: None,
            ..Default::default()
        };
        season2
            .save(db)
            .await
            .unwrap();

        let resp = server
            .get(&format!("/shows/{}/seasons", series.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        assert_eq!(
            items.len(),
            1,
            "Season 2 has no dates — must not appear; got: {:?}",
            items
        );
        assert_eq!(
            items[0]["IndexNumber"]
                .as_i64()
                .unwrap(),
            1
        );
    }
}
