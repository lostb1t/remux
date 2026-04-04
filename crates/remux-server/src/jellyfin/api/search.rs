use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use chrono::Datelike;
use itertools::Itertools;
use remux_macros::get;
use std::time::Duration;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
use axum_anyhow::{ApiResult as Result, IntoApiError};

#[get("/search/hints")]
pub async fn search_hints(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<jellyfin::SearchHintsQuery>,
) -> Result<impl IntoResponse> {
    let term = q.search_term.unwrap_or_default();
    if term.is_empty() {
        return Ok(Json(jellyfin::SearchHintResult {
            search_hints: vec![],
            total_record_count: 0,
        }));
    }
    let limit = q.limit.unwrap_or(20);

    // DB title-match search (fast path)
    let db_results = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            title_contains: Some(term.clone()),
            kind: Some(vec![
                db::MediaKind::Movie,
                db::MediaKind::Series,
            ]),
            limit: Some(limit),
            ..Default::default()
        },
    )
    .await?
    .records;

    // AIO live search if DB returns nothing
    let results = if db_results.is_empty() {
        if let Ok(aio) = crate::aio::AioService::from_settings(&state.ctx.db).await {
            let movie_fut = aio.search(crate::sdks::aio::MediaType::Movie, term.clone());
            let series_fut = aio.search(crate::sdks::aio::MediaType::Series, term.clone());
            let (movie_res, series_res) = tokio::join!(movie_fut, series_fut);

            let mut combined = series_res.unwrap_or_default();
            combined.extend(movie_res.unwrap_or_default());

            combined
                .into_iter()
                // Deduplicate by IMDb ID if available, otherwise by AIO ID
                .unique_by(|meta| meta.imdb_id.clone().unwrap_or_else(|| meta.id.clone()))
                .filter_map(|meta| {
                    let m = db::Media::try_from(meta.clone()).ok()?;
                    state.ctx.store.insert(m.id, meta, Duration::from_secs(3600));
                    Some(m)
                })
                .collect()
        } else {
            vec![]
        }
    } else {
        db_results
    };

    let hints = results
        .into_iter()
        .map(|m| jellyfin::SearchHint {
            item_id: m.id,
            name: Some(m.title.clone()),
            type_: jellyfin::db_media_kind_to_type(m.kind.clone()),
            primary_image_tag: m.poster.clone(),
            production_year: m.released_at.map(|d| d.year() as i64),
            run_time_ticks: m.runtime.map(|r| r * 10_000_000),
            is_folder: Some(matches!(
                m.kind,
                db::MediaKind::Series | db::MediaKind::Season
            )),
            media_type: match m.kind {
                db::MediaKind::Movie | db::MediaKind::Episode => Some("Video".to_string()),
                _ => None,
            },
            series_id: m.series_id,
            ..Default::default()
        })
        .collect::<Vec<_>>();

    let total = hints.len() as i64;
    Ok(Json(jellyfin::SearchHintResult {
        search_hints: hints,
        total_record_count: total,
    }))
}
