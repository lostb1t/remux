use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use remux_macros::{delete, get, post};
use uuid::Uuid;

use crate::{
    AppState, OptionExt, ResultExt, api, db,
    db::{ImageKind, auth},
    services::image::{ImageProcessOptions, ImageService},
};
use axum_anyhow::ApiResult as Result;

static IMAGE_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("failed to build image proxy client")
    });

/// Fetch an upstream URL, returning (bytes, content_type).
async fn fetch_upstream(url: &str) -> anyhow::Result<(Vec<u8>, String)> {
    let resp = remux_utils::retry! {
        attempts: 3,
        delay: 500,
        { IMAGE_CLIENT.get(url).send().await }
    }
    .map_err(|e| anyhow::anyhow!("image fetch failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("upstream image returned {status}");
    }
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| {
            v.to_str()
                .ok()
        })
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("image read failed: {e}"))?
        .to_vec();
    Ok((bytes, ct))
}

async fn items_images_inner(
    state: AppState,
    id: Uuid,
    image_type: api::ImageType,
    q: api::ImageQuery,
) -> Result<impl IntoResponse> {
    let opts = ImageProcessOptions {
        fill_width: q.fill_width,
        fill_height: q.fill_height,
        width: q.width,
        height: q.height,
        max_width: q.max_width,
        max_height: q.max_height,
        quality: q.quality,
        blur: q.blur,
        background_color: q
            .background_color
            .clone(),
        format: q
            .format
            .clone(),
    };

    // Resolve to (raw_bytes, content_type, source_key_for_cache, is_remote).
    // is_remote=true means the bytes came from an external URL and must not be
    // re-encoded — proxy them as-is.
    let (bytes, raw_ct, source_key, is_remote): (Vec<u8>, String, String, bool) =
        if let Some(url) = q
            .tag
            .as_ref()
            .filter(|t| t.contains("://"))
        {
            let (b, ct) = fetch_upstream(url)
                .await
                .context_not_found("image fetch failed")?;
            (b, ct, url.clone(), true)
        } else {
            let key = id.to_string();
            if let Some(media) = db::Media::get_by_id(
                &state
                    .ctx
                    .db,
                &id,
            )
            .await?
            {
                let kind: ImageKind = image_type
                    .to_string()
                    .parse()
                    .unwrap_or(ImageKind::Primary);
                // If Thumb is requested but not stored, fall back to Primary.
                let img_row = media
                    .images
                    .get(kind)
                    .or_else(|| {
                        if kind == ImageKind::Thumb {
                            media
                                .images
                                .get(ImageKind::Primary)
                        } else {
                            None
                        }
                    });

                if let Some(img) = img_row {
                    let source_key = img
                        .id
                        .to_string();
                    if img
                        .path
                        .starts_with('/')
                    {
                        let path = std::path::PathBuf::from(&img.path);
                        let (b, ct) = ImageService::serve_local(&path)
                            .await
                            .context_not_found("image file not found")?;
                        (b, ct.to_string(), source_key, false)
                    } else {
                        // Always proxy external URLs rather than redirecting — some clients
                        // (e.g. Infuse) do not follow redirects for image requests.
                        let (b, ct) = fetch_upstream(&img.path)
                            .await
                            .context_not_found("image fetch failed")?;
                        (b, ct, source_key, true)
                    }
                } else if matches!(
                    image_type,
                    api::ImageType::Primary | api::ImageType::Thumb
                ) && matches!(
                    media.kind,
                    db::MediaKind::Collection | db::MediaKind::Folder
                ) {
                    let b = ImageService::library_image(
                        &state
                            .ctx
                            .config
                            .data_dir,
                        id,
                        &media.title,
                        &state
                            .ctx
                            .db,
                    )
                    .await
                    .context_not_found("no backdrop available for library")?;
                    // Reload the newly-inserted image row to get its stable UUID
                    let img_row = db::MediaImage::get_for_media(
                        &state
                            .ctx
                            .db,
                        &id,
                    )
                    .await
                    .unwrap_or_default()
                    .primary
                    .into_iter()
                    .find(|i| i.image_index == 0);
                    let source_key = img_row
                        .map(|i| {
                            i.id.to_string()
                        })
                        .unwrap_or_else(|| format!("placeholder:{id}"));
                    (b, "image/jpeg".to_string(), source_key, false)
                } else {
                    return Err(anyhow::anyhow!("image not found"))
                        .context_not_found("image not found");
                }
            } else {
                // Not in DB — cached search result.
                let url = state
                    .ctx
                    .store
                    .get::<db::Media>(key.clone())
                    .and_then(|m| match image_type {
                        api::ImageType::Primary | api::ImageType::Thumb => m
                            .get_image(ImageKind::Primary)
                            .map(str::to_owned),
                        api::ImageType::Backdrop => m
                            .get_image(ImageKind::Backdrop)
                            .map(str::to_owned),
                        api::ImageType::Logo | api::ImageType::LogoImageAspectRatio => {
                            m.get_image(ImageKind::Logo)
                                .map(str::to_owned)
                        }
                    });
                let url = url.context_not_found("media image not found")?;
                let (b, ct) = fetch_upstream(&url)
                    .await
                    .context_not_found("image fetch failed")?;
                (b, ct, url, true)
            }
        };

    // Apply resize/quality/blur/format transforms (cached) for local images only.
    // Remote images are proxied as-is — re-encoding adds latency with no benefit.
    let (final_bytes, content_type): (Vec<u8>, String) = if is_remote {
        (bytes, raw_ct)
    } else {
        let (b, ct) = ImageService::process_image(
            &state
                .ctx
                .config
                .data_dir,
            bytes,
            &opts,
            &source_key,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context_internal("image processing failed")?;
        (b, ct.to_string())
    };

    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CACHE_CONTROL, "max-age=86400".to_string()),
        ],
        final_bytes,
    )
        .into_response())
}

// --- GET ---

#[get("/items/{id}/images")]
pub async fn get_item_image_infos(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct ImageInfo {
        image_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        image_index: Option<i64>,
        image_tag: String,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        height: Option<i64>,
        size: u64,
    }

    let images = db::MediaImage::get_for_media(
        &state
            .ctx
            .db,
        &id,
    )
    .await?;

    let infos: Vec<ImageInfo> = images
        .into_iter()
        .map(|img| {
            let size = if img
                .path
                .starts_with('/')
            {
                std::fs::metadata(&img.path)
                    .map(|m| m.len())
                    .unwrap_or(0)
            } else {
                0
            };
            let image_type = match img
                .image_type
                .as_str()
            {
                "primary" => "Primary",
                "backdrop" => "Backdrop",
                "logo" => "Logo",
                "thumb" => "Thumb",
                other => other,
            }
            .to_owned();
            ImageInfo {
                image_type,
                image_index: if img.image_index == 0 {
                    None
                } else {
                    Some(img.image_index)
                },
                image_tag: img
                    .id
                    .simple()
                    .to_string(),
                path: img.path,
                width: img.width,
                height: img.height,
                size,
            }
        })
        .collect();

    Ok(axum::Json(infos))
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

// --- POST (upload) ---

async fn upload_item_image_inner(
    state: AppState,
    id: Uuid,
    kind: ImageKind,
    image: api::image::JellyfinImage,
) -> Result<impl IntoResponse> {
    ImageService::save_image(
        &state
            .ctx
            .config
            .data_dir,
        id,
        kind,
        &image.bytes,
        &state
            .ctx
            .db,
    )
    .await
    .context_internal("failed to save image")?;
    Ok(StatusCode::NO_CONTENT)
}

#[post("/items/{id}/images/{image_type}")]
pub async fn upload_item_image(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((id, image_type)): Path<(Uuid, String)>,
    image: api::image::JellyfinImage,
) -> Result<impl IntoResponse> {
    upload_item_image_inner(state, id, parse_image_kind(&image_type), image).await
}

#[post("/items/{id}/images/{image_type}/{index}")]
pub async fn upload_item_image_indexed(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((id, image_type, _index)): Path<(Uuid, String, usize)>,
    image: api::image::JellyfinImage,
) -> Result<impl IntoResponse> {
    upload_item_image_inner(state, id, parse_image_kind(&image_type), image).await
}

// --- DELETE ---

async fn delete_item_image_inner(
    state: AppState,
    id: Uuid,
    kind: ImageKind,
) -> Result<impl IntoResponse> {
    ImageService::delete_image(
        &state
            .ctx
            .config
            .data_dir,
        id,
        kind,
        &state
            .ctx
            .db,
    )
    .await
    .context_internal("failed to delete image")?;
    Ok(StatusCode::NO_CONTENT)
}

#[delete("/items/{id}/images/{image_type}")]
pub async fn delete_item_image(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((id, image_type)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse> {
    delete_item_image_inner(state, id, parse_image_kind(&image_type)).await
}

#[delete("/items/{id}/images/{image_type}/{index}")]
pub async fn delete_item_image_indexed(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((id, image_type, _index)): Path<(Uuid, String, usize)>,
) -> Result<impl IntoResponse> {
    delete_item_image_inner(state, id, parse_image_kind(&image_type)).await
}

/// Convert a Jellyfin URL path segment (e.g. "Thumb", "Primary") to `ImageKind`.
/// Routes through `api::ImageType` so Thumb→Primary and LogoImageAspectRatio→Logo
/// semantics are preserved.
fn parse_image_kind(s: &str) -> ImageKind {
    s.parse::<api::ImageType>()
        .map(|t| {
            t.to_string()
                .parse()
                .unwrap_or(ImageKind::Primary)
        })
        .unwrap_or(ImageKind::Primary)
}
