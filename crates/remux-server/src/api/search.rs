use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use chrono::Datelike;
use itertools::Itertools;
use remux_macros::get;
use std::time::Duration;

use crate::AppState;
use crate::api;
use crate::db;
use crate::db::auth;
use axum_anyhow::{ApiResult as Result, IntoApiError};

#[get("/search/hints")]
pub async fn search_hints(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::SearchHintsQuery>,
) -> Result<impl IntoResponse> {
    let term = q.search_term.unwrap_or_default();
    if term.is_empty() {
        return Ok(Json(api::SearchHintResult {
            search_hints: vec![],
            total_record_count: 0,
        }));
    }
    let limit = q.limit.unwrap_or(20) as usize;

    // Determine which media types the caller wants.
    // An empty filter means "everything".
    let requested_types = q.include_item_types.unwrap_or_default();
    let wants_video = requested_types.is_empty()
        || requested_types.contains(&api::MediaType::Movie)
        || requested_types.contains(&api::MediaType::Series);
    let wants_music = requested_types.is_empty()
        || requested_types.contains(&api::MediaType::Audio)
        || requested_types.contains(&api::MediaType::MusicAlbum)
        || requested_types.contains(&api::MediaType::MusicArtist);

    tracing::info!(
        term,
        limit,
        wants_video,
        wants_music,
        ?requested_types,
        "search_hints request"
    );

    let mut results: Vec<db::Media> = vec![];

    if wants_video {
        // DB title-match search (fast path)
        let db_results = db::Media::get_by_filter(
            &state.ctx.db,
            &db::MediaFilter {
                title_contains: Some(term.clone()),
                kind: Some(vec![db::MediaKind::Movie, db::MediaKind::Series]),
                limit: Some(limit as u32),
                ..Default::default()
            },
        )
        .await?
        .records;

        tracing::info!(count = db_results.len(), "search_hints DB results");

        // Live search via addon registry if DB returns nothing.
        let video_results = if db_results.is_empty() {
            let movie_fut = state.ctx.addons.search(
                &db::MediaKind::Movie,
                &term,
                limit,
                &state.ctx,
            );
            let series_fut = state.ctx.addons.search(
                &db::MediaKind::Series,
                &term,
                limit,
                &state.ctx,
            );
            let (movie_res, series_res) = tokio::join!(movie_fut, series_fut);
            let mut combined = series_res.unwrap_or_default();
            combined.extend(movie_res.unwrap_or_default());
            combined
        } else {
            db_results
        };

        results.extend(video_results);
    }

    if wants_music {
        tracing::info!(term, "search_hints: querying music via addon registry");
        match state
            .ctx
            .addons
            .search(&db::MediaKind::Track, &term, limit, &state.ctx)
            .await
        {
            Ok(tracks) => {
                tracing::info!(count = tracks.len(), "search_hints: music results");
                results.extend(tracks);
            }
            Err(e) => {
                tracing::warn!(error = %e, term, "search_hints: music search failed");
            }
        }
    }

    let hints = results
        .into_iter()
        .map(|m| api::SearchHint {
            item_id: m.id,
            name: Some(m.title.clone()),
            type_: api::db_media_kind_to_type(m.kind.clone()),
            primary_image_tag: m
                .images
                .get(db::ImageKind::Primary)
                .map(|i| i.id.to_string()),
            production_year: m.released_at.map(|d| d.year() as i64),
            run_time_ticks: m.runtime.map(|r| r * 10_000_000),
            is_folder: Some(matches!(
                m.kind,
                db::MediaKind::Series | db::MediaKind::Season
            )),
            media_type: match m.kind {
                db::MediaKind::Movie | db::MediaKind::Episode => {
                    Some("Video".to_string())
                }
                db::MediaKind::Track => Some("Audio".to_string()),
                _ => None,
            },
            series_id: m.grandparent_id,
            ..Default::default()
        })
        .collect::<Vec<_>>();

    tracing::info!(total = hints.len(), "search_hints: returning results");

    let total = hints.len() as i64;
    Ok(Json(api::SearchHintResult {
        search_hints: hints,
        total_record_count: total,
    }))
}
