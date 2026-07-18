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

async fn unavailable_artwork(
    id: Uuid,
    source_key: String,
) -> Result<(Vec<u8>, String, String, bool)> {
    tracing::warn!(%id, "artwork source was unavailable; serving generated fallback");
    Ok((
        ImageService::unavailable_artwork().await?,
        "image/jpeg".to_string(),
        format!("unavailable-artwork:{source_key}"),
        false,
    ))
}

async fn inherited_track_image(
    db: &sqlx::SqlitePool,
    media: &db::Media,
) -> Result<Option<db::MediaImage>> {
    if media.kind != db::MediaKind::Track {
        return Ok(None);
    }

    for parent_id in [media.parent_id, media.grandparent_id]
        .into_iter()
        .flatten()
    {
        if let Some(image) = db::MediaImage::get_for_media(db, &parent_id)
            .await?
            .get(ImageKind::Primary)
            .cloned()
        {
            return Ok(Some(image));
        }
    }

    Ok(None)
}

fn image_at(
    images: &db::MediaImages,
    kind: ImageKind,
    index: i64,
) -> Option<&db::MediaImage> {
    images
        .iter()
        .find(|image| {
            image.image_type == kind.to_string() && image.image_index == index
        })
}

async fn items_images_inner(
    state: AppState,
    id: Uuid,
    image_type: api::ImageType,
    image_index: Option<i64>,
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
            match fetch_upstream(url).await {
                Ok((b, ct)) => (b, ct, url.clone(), true),
                Err(_) => unavailable_artwork(id, url.clone()).await?,
            }
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
                let image_index = image_index.unwrap_or(0);
                let img_row = image_at(&media.images, kind, image_index)
                    .or_else(|| {
                        if kind == ImageKind::Thumb && image_index == 0 {
                            image_at(&media.images, ImageKind::Primary, 0)
                        } else {
                            None
                        }
                    })
                    .cloned();
                let img_row = if img_row.is_none()
                    && image_index == 0
                    && matches!(kind, ImageKind::Primary | ImageKind::Thumb)
                {
                    inherited_track_image(
                        &state
                            .ctx
                            .db,
                        &media,
                    )
                    .await?
                } else {
                    img_row
                };

                if let Some(img) = img_row {
                    let source_key = img
                        .id
                        .to_string();
                    if img
                        .path
                        .starts_with('/')
                    {
                        let path = std::path::PathBuf::from(&img.path);
                        match ImageService::serve_local(&path).await {
                            Ok((b, ct)) => (b, ct.to_string(), source_key, false),
                            Err(_) => unavailable_artwork(id, source_key).await?,
                        }
                    } else {
                        // Always proxy external URLs rather than redirecting — some clients
                        // (e.g. Infuse) do not follow redirects for image requests.
                        match fetch_upstream(&img.path).await {
                            Ok((b, ct)) => (b, ct, source_key, true),
                            Err(_) => unavailable_artwork(id, source_key).await?,
                        }
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
                match fetch_upstream(&url).await {
                    Ok((b, ct)) => (b, ct, url, true),
                    Err(_) => unavailable_artwork(id, url).await?,
                }
            }
        };

    // Jellyfin clients use the image query parameters for layout sizing. Remote
    // providers (e.g. Deezer covers) must honour them too: returning an original
    // 1000px image for `fillWidth=380` breaks clients that budget decoding and
    // layout based on the requested size. Processed results are cached by the
    // upstream URL and options, so this only fetches and re-encodes once per
    // variant.
    // Remote artwork used to be cached as its raw upstream bytes. Version its
    // source key so those incompatible cache entries cannot survive the switch
    // to transformed remote images.
    let processing_source_key =
        is_remote.then(|| format!("remote-art-v2:{source_key}"));
    let processing_source_key = processing_source_key
        .as_deref()
        .unwrap_or(&source_key);

    let (final_bytes, content_type): (Vec<u8>, String) = if is_remote {
        if opts.needs_processing() {
            let (b, ct) = ImageService::process_image(
                &state
                    .ctx
                    .config
                    .data_dir,
                bytes,
                &opts,
                processing_source_key,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context_internal("image processing failed")?;
            (b, ct.to_string())
        } else {
            (bytes, raw_ct)
        }
    } else {
        let (b, ct) = ImageService::process_image(
            &state
                .ctx
                .config
                .data_dir,
            bytes,
            &opts,
            processing_source_key,
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

    let mut images = db::MediaImage::get_for_media(
        &state
            .ctx
            .db,
        &id,
    )
    .await?;

    // Audio inherits album/artist primary art. The item DTO advertises that
    // image under ImageTags.Primary, so ImageInfos must report the same image
    // and tag for clients that discover artwork through this endpoint first.
    if images
        .get(ImageKind::Primary)
        .is_none()
    {
        if let Some(media) = db::Media::get_by_id(
            &state
                .ctx
                .db,
            &id,
        )
        .await?
        {
            if let Some(image) = inherited_track_image(
                &state
                    .ctx
                    .db,
                &media,
            )
            .await?
            {
                images
                    .primary
                    .push(image);
            }
        }
    }

    // Stat every local image on a blocking thread. This ran as a synchronous
    // `std::fs::metadata` per image row inside the map below, stalling the
    // async worker once per image — which matters most exactly when many
    // requests are in flight. Order is preserved by zipping against the same
    // Vec the sizes were computed from.
    let imgs: Vec<_> = images
        .into_iter()
        .collect();
    let paths: Vec<String> = imgs
        .iter()
        .map(|img| {
            img.path
                .clone()
        })
        .collect();
    let sizes: Vec<u64> = tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|path| {
                if path.starts_with('/') {
                    std::fs::metadata(&path)
                        .map(|m| m.len())
                        .unwrap_or(0)
                } else {
                    0
                }
            })
            .collect()
    })
    .await
    .unwrap_or_else(|_| vec![0; imgs.len()]);

    let infos: Vec<ImageInfo> = imgs
        .into_iter()
        .zip(sizes)
        .map(|(img, size)| {
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
                image_tag: crate::api::models::image_tag_for_id(img.id),
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
    items_images_inner(state, id, image_type, None, q).await
}

#[get("/items/{id}/images/{image_type}/{index}")]
pub async fn items_images_indexed(
    State(state): State<AppState>,
    Path((id, image_type, index)): Path<(Uuid, api::ImageType, usize)>,
    Query(q): Query<api::ImageQuery>,
) -> Result<impl IntoResponse> {
    items_images_inner(state, id, image_type, Some(index as i64), q).await
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
