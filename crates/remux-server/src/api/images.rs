use axum::extract::{Path, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use remux_macros::get;
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::db;
use crate::providers::search::MusicSearchResult;
use crate::sdks;
use axum_anyhow::{ApiResult as Result, OptionExt};

static IMAGE_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .build()
            .expect("failed to build image proxy client")
    });

async fn items_images_inner(
    state: AppState,
    id: Uuid,
    image_type: api::ImageType,
    q: api::ImageQuery,
) -> Result<impl IntoResponse> {
    let url = if let Some(tag_url) = q.tag {
        tag_url
    } else {
        let key = id.to_string();
        if let Some(media) = db::Media::get_by_id(&state.ctx.db, &id).await? {
            match image_type {
                api::ImageType::Primary | api::ImageType::Thumb => media.poster,
                api::ImageType::Backdrop => media.backdrop,
                api::ImageType::Logo | api::ImageType::LogoImageAspectRatio => {
                    media.logo
                }
            }
            .unwrap_or_else(|| "https://placehold.co/600x400".to_string())
        } else {
            // Not in DB yet — item is a cached search result. Pull poster from store.
            let poster = state
                .ctx
                .store
                .get::<MusicSearchResult>(key.clone())
                .and_then(|r| r.media.poster.clone())
                .or_else(|| {
                    state
                        .ctx
                        .store
                        .get::<sdks::aio::Meta>(key)
                        .and_then(|m| m.poster.clone())
                });
            poster.context_not_found("Not Found", "media not found")?
        }
    };

    let upstream = IMAGE_CLIENT
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("image fetch failed: {e}"))?;

    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    let bytes = upstream
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("image read failed: {e}"))?;

    Ok(([(header::CONTENT_TYPE, content_type)], bytes))
}

#[get("/items/{id}/images/{image_type}")]
pub async fn items_images(
    State(state): State<AppState>,
    Path((id, image_type)): Path<(Uuid, api::ImageType)>,
    Query(q): Query<api::ImageQuery>,
) -> Result<impl IntoResponse> {
    items_images_inner(state, id, image_type, q).await
}

#[get("/items/{id}/images/{image_type}/{index}")]
pub async fn items_images_indexed(
    State(state): State<AppState>,
    Path((id, image_type, _index)): Path<(Uuid, api::ImageType, usize)>,
    Query(q): Query<api::ImageQuery>,
) -> Result<impl IntoResponse> {
    items_images_inner(state, id, image_type, q).await
}
