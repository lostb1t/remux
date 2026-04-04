use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::response::Redirect;
use axum_extra::extract::Query;
use remux_macros::get;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::jellyfin;
use axum_anyhow::{ApiResult as Result, OptionExt};

async fn items_images_inner(
    state: AppState,
    id: Uuid,
    image_type: jellyfin::ImageType,
    q: jellyfin::ImageQuery,
) -> Result<Redirect> {
    if let Some(url) = q.tag {
        return Ok(Redirect::temporary(url.as_str()));
    }

    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("Not Found", "media not found")?;

    let url = match image_type {
        jellyfin::ImageType::Primary => media.poster,
        jellyfin::ImageType::Backdrop => media.backdrop,
        jellyfin::ImageType::Logo | jellyfin::ImageType::LogoImageAspectRatio => media.logo,
        jellyfin::ImageType::Thumb => media.poster,
    }
    .unwrap_or_else(|| "https://placehold.co/600x400".to_string());

    Ok(Redirect::temporary(url.as_str()))
}

#[get("/items/{id}/images/{image_type}")]
pub async fn items_images(
    State(state): State<AppState>,
    Path((id, image_type)): Path<(Uuid, jellyfin::ImageType)>,
    Query(q): Query<jellyfin::ImageQuery>,
) -> Result<impl IntoResponse> {
    items_images_inner(state, id, image_type, q).await
}

#[get("/items/{id}/images/{image_type}/{index}")]
pub async fn items_images_indexed(
    State(state): State<AppState>,
    Path((id, image_type, _index)): Path<(Uuid, jellyfin::ImageType, usize)>,
    Query(q): Query<jellyfin::ImageQuery>,
) -> Result<impl IntoResponse> {
    items_images_inner(state, id, image_type, q).await
}
