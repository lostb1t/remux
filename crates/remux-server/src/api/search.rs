use axum::{Json, extract::State, response::IntoResponse};
use axum_extra::extract::Query;
use remux_macros::get;

use crate::{AppState, api, db::auth};
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
        exclude_item_types: q.exclude_item_types,
        media_types: q.media_types,
        parent_id: q.parent_id,
        ..Default::default()
    };

    let result = super::items::get_items(state, session, items_query, true)
        .await?
        .with_client_patches()
        .build();

    let total = result.total_count;
    let hints: Vec<api::SearchHint> = result
        .items
        .into_iter()
        .map(item_to_hint)
        .collect();

    Ok(Json(api::SearchHintResult {
        search_hints: hints,
        total_record_count: total,
    }))
}

fn item_to_hint(item: api::BaseItemDto) -> api::SearchHint {
    let thumb_tag = item
        .image_tags
        .as_ref()
        .and_then(|t| {
            t.thumb
                .clone()
        })
        .or_else(|| {
            item.parent_thumb_image_tag
                .clone()
        });
    let thumb_item_id = item.parent_thumb_item_id;

    let backdrop_tag = item
        .backdrop_image_tags
        .first()
        .cloned()
        .or_else(|| {
            item.parent_backdrop_image_tags
                .as_ref()
                .and_then(|t| {
                    t.first()
                        .cloned()
                })
        });
    let backdrop_item_id = item.parent_backdrop_item_id;

    let primary_image_tag = item
        .image_tags
        .and_then(|t| t.primary);

    api::SearchHint {
        id: item.id,
        item_id: item.id,
        name: item.name,
        type_: item.type_,
        primary_image_tag,
        production_year: item.production_year,
        run_time_ticks: item.run_time_ticks,
        is_folder: Some(item.is_folder),
        media_type: Some(
            item.media_type
                .to_string(),
        ),
        series_id: item.series_id,
        series_name: item.series_name,
        index_number: item.index_number,
        parent_index_number: item.parent_index_number,
        thumb_image_tag: thumb_tag,
        thumb_image_item_id: thumb_item_id,
        backdrop_image_tag: backdrop_tag,
        backdrop_image_item_id: backdrop_item_id,
        album: item.album,
        album_id: item.album_id,
        album_artist: item.album_artist,
        artists: item.artists,
        ..Default::default()
    }
}
