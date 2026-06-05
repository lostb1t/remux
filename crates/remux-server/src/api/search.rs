use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use remux_macros::get;

use crate::AppState;
use crate::api;
use crate::db::auth;
use axum_anyhow::ApiResult as Result;

#[get("/search/hints")]
pub async fn search_hints(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::SearchHintsQuery>,
) -> Result<impl IntoResponse> {
    let term = q
        .search_term
        .unwrap_or_default();
    if term.is_empty() {
        return Ok(Json(api::SearchHintResult {
            search_hints: vec![],
            total_record_count: 0,
        }));
    }

    let items_query = api::GetItemsQuery {
        search_term: Some(term),
        limit: q
            .limit
            .or(Some(20)),
        start_index: q.start_index,
        include_item_types: q.include_item_types,
        ..Default::default()
    };

    let result = super::items::get_items(state, session, items_query, false).await?;

    let hints: Vec<api::SearchHint> = result
        .items
        .into_iter()
        .map(item_to_hint)
        .collect();
    let total = hints.len() as i64;

    Ok(Json(api::SearchHintResult {
        search_hints: hints,
        total_record_count: total,
    }))
}

fn item_to_hint(item: api::BaseItemDto) -> api::SearchHint {
    api::SearchHint {
        item_id: item.id,
        name: item.name,
        type_: item.type_,
        primary_image_tag: item
            .image_tags
            .and_then(|t| t.primary),
        production_year: item.production_year,
        run_time_ticks: item.run_time_ticks,
        is_folder: Some(item.is_folder),
        media_type: Some(
            item.media_type
                .to_string(),
        ),
        series_id: item.series_id,
        series_name: item.series_name,
        ..Default::default()
    }
}
