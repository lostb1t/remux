use std::io::Cursor;
use std::path::PathBuf;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use imageproc::drawing::draw_text_mut;
use uuid::Uuid;

use crate::api::image::detect_content_type;
use crate::db;
use crate::db::ImageKind;

/// Width/height of generated library placeholder images (16:9).
const OUT_W: u32 = 960;
const OUT_H: u32 = 540;

/// Black overlay opacity — matches Jellyfin's `0x78` (≈ 47%).
const OVERLAY_ALPHA: f32 = 0x78 as f32 / 255.0;

/// Maximum text width as a fraction of image width before scaling down.
const MAX_TEXT_FRACTION: f32 = 0.90;

static FONT_DATA: &[u8] = include_bytes!("../../assets/fonts/LiberationSans-Bold.ttf");

static HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build image http client")
    });

// ---------------------------------------------------------------------------
// Image processing options
// ---------------------------------------------------------------------------

/// Parameters controlling server-side image transformation.
#[derive(Debug, Clone, Default)]
pub struct ImageProcessOptions {
    pub fill_width: Option<u32>,
    pub fill_height: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    /// JPEG encode quality (0–100). `None` → default 90.
    pub quality: Option<u8>,
    /// Gaussian blur sigma in pixels.
    pub blur: Option<u32>,
    pub background_color: Option<String>,
    /// "jpg" / "jpeg" / "png". `None` → jpeg.
    pub format: Option<String>,
}

impl ImageProcessOptions {
    /// Returns true when any transformation is requested.
    pub fn needs_processing(&self) -> bool {
        self.fill_width
            .is_some()
            || self
                .fill_height
                .is_some()
            || self
                .width
                .is_some()
            || self
                .height
                .is_some()
            || self
                .max_width
                .is_some()
            || self
                .max_height
                .is_some()
            || self
                .quality
                .is_some()
            || self
                .blur
                .is_some()
            || self
                .background_color
                .is_some()
            || self
                .format
                .is_some()
    }

    fn output_format(&self) -> ImageFormat {
        match self
            .format
            .as_deref()
        {
            Some("png") => ImageFormat::Png,
            _ => ImageFormat::Jpeg,
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self
            .format
            .as_deref()
        {
            Some("png") => "image/png",
            _ => "image/jpeg",
        }
    }

    /// Stable cache key derived from source identifier + all transform params.
    fn cache_key(&self, source: &str) -> String {
        let key_data = format!(
            "{}|fw={:?}|fh={:?}|w={:?}|h={:?}|mw={:?}|mh={:?}|q={:?}|bl={:?}|bg={:?}|fmt={:?}",
            source,
            self.fill_width,
            self.fill_height,
            self.width,
            self.height,
            self.max_width,
            self.max_height,
            self.quality,
            self.blur,
            self.background_color,
            self.format,
        );
        Uuid::new_v5(&Uuid::NAMESPACE_URL, key_data.as_bytes()).to_string()
    }
}

// ---------------------------------------------------------------------------
// ImageService
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ImageService;

impl ImageService {
    /// Returns the directory for a library item's local images.
    pub fn image_dir(data_dir: &std::path::Path, id: Uuid) -> PathBuf {
        data_dir
            .join("meta")
            .join("library")
            .join(id.to_string())
    }

    /// Returns the local path for a specific image type (e.g. `primary.jpg`).
    pub fn image_path(
        data_dir: &std::path::Path,
        id: Uuid,
        image_type: &str,
    ) -> PathBuf {
        Self::image_dir(data_dir, id).join(format!("{}.jpg", image_type.to_lowercase()))
    }

    /// Generate the library placeholder image, write it to disk, save the path
    /// to `media_images` in the DB, and return the JPEG bytes.
    pub async fn library_image(
        data_dir: &std::path::Path,
        id: Uuid,
        name: &str,
        db: &sqlx::SqlitePool,
    ) -> anyhow::Result<Vec<u8>> {
        let path = Self::image_path(data_dir, id, "primary");

        if path.exists() {
            return Ok(tokio::fs::read(&path).await?);
        }

        let bytes = Self::generate(id, name, db).await?;
        Self::write_image_to_disk(&path, &bytes).await?;
        // INSERT OR IGNORE — don't replace if already exists (stable UUID for cache)
        sqlx::query(
            "INSERT OR IGNORE INTO media_images (id, media_id, image_type, image_index, path, width, height) VALUES (?, ?, 'primary', 0, ?, ?, ?)"
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(path.to_string_lossy().as_ref())
        .bind(OUT_W as i64)
        .bind(OUT_H as i64)
        .execute(db)
        .await?;

        Ok(bytes)
    }

    /// Save an uploaded image for `id`/`image_type`, write to disk, update DB.
    pub async fn save_image(
        data_dir: &std::path::Path,
        id: Uuid,
        kind: ImageKind,
        bytes: &[u8],
        db: &sqlx::SqlitePool,
    ) -> anyhow::Result<()> {
        let path = Self::image_path(data_dir, id, &kind.to_string());
        Self::write_image_to_disk(&path, bytes).await?;
        let (img_w, img_h) = image::load_from_memory(bytes)
            .map(|img| (img.width() as i64, img.height() as i64))
            .ok()
            .unzip();
        db::MediaImage::save(
            db,
            id,
            kind,
            path.to_string_lossy()
                .as_ref(),
            img_w,
            img_h,
        )
        .await
        .map_err(anyhow::Error::from)?;
        Ok(())
    }

    /// Delete the local image for `id`/`kind` and remove from media_images.
    pub async fn delete_image(
        data_dir: &std::path::Path,
        id: Uuid,
        kind: ImageKind,
        db: &sqlx::SqlitePool,
    ) -> anyhow::Result<()> {
        let path = Self::image_path(data_dir, id, &kind.to_string());
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        db::MediaImage::delete_for_type(db, id, kind)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(())
    }

    /// Serve a locally-stored image file, returning (bytes, content_type).
    pub async fn serve_local(
        path: &PathBuf,
    ) -> anyhow::Result<(Vec<u8>, &'static str)> {
        let bytes = tokio::fs::read(path).await?;
        let ct = detect_content_type(&bytes);
        Ok((bytes, ct))
    }

    /// Directory for processed image cache.
    pub fn cache_dir(data_dir: &std::path::Path) -> PathBuf {
        data_dir
            .join("cache")
            .join("images")
    }

    /// Apply image transformations described by `opts`, returning (bytes, content_type).
    ///
    /// * If no processing is needed, the raw bytes are returned as-is.
    /// * Processed results are cached at `cache_dir()/{uuid_key}.{ext}`.
    pub async fn process_image(
        data_dir: &std::path::Path,
        bytes: Vec<u8>,
        opts: &ImageProcessOptions,
        source_key: &str,
    ) -> anyhow::Result<(Vec<u8>, &'static str)> {
        if !opts.needs_processing() {
            let ct = detect_content_type(&bytes);
            return Ok((bytes, ct));
        }

        let cache_key = opts.cache_key(source_key);
        let cache_dir = Self::cache_dir(data_dir);

        // Check both extensions — alpha auto-detection may produce PNG even when opts say JPEG.
        for (ext, ct) in [("png", "image/png"), ("jpg", "image/jpeg")] {
            let path = cache_dir.join(format!("{cache_key}.{ext}"));
            if path.exists() {
                let cached = tokio::fs::read(&path).await?;
                return Ok((cached, ct));
            }
        }

        let opts_clone = opts.clone();
        let (processed, content_type) = tokio::task::spawn_blocking(move || {
            process_image_sync(&bytes, &opts_clone)
        })
        .await??;

        let ext = if content_type == "image/png" {
            "png"
        } else {
            "jpg"
        };
        tokio::fs::create_dir_all(&cache_dir).await?;
        tokio::fs::write(cache_dir.join(format!("{cache_key}.{ext}")), &processed)
            .await?;

        Ok((processed, content_type))
    }

    async fn write_image_to_disk(path: &PathBuf, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }

    async fn generate(
        id: Uuid,
        name: &str,
        db: &sqlx::SqlitePool,
    ) -> anyhow::Result<Vec<u8>> {
        let bg = match find_backdrop_url(id, db).await {
            Err(_) => solid_background(),
            Ok(src) => {
                let result = if src.contains("://") {
                    Self::fetch_and_resize(&src).await
                } else {
                    Self::read_local_and_resize(&src).await
                };
                match result {
                    Ok(img) => apply_dark_overlay(img),
                    Err(_) => solid_background(),
                }
            }
        };
        let img = draw_label(bg, name)?;
        encode_jpeg(img)
    }

    async fn fetch_and_resize(url: &str) -> anyhow::Result<RgbImage> {
        let resp = HTTP_CLIENT
            .get(url)
            .send()
            .await?;
        let bytes = resp
            .bytes()
            .await?;
        let decoded = image::load_from_memory(&bytes)?.into_rgb8();
        let resized = image::imageops::resize(
            &decoded,
            OUT_W,
            OUT_H,
            image::imageops::FilterType::Lanczos3,
        );
        Ok(resized)
    }

    async fn read_local_and_resize(path: &str) -> anyhow::Result<RgbImage> {
        let bytes = tokio::fs::read(path).await?;
        let decoded = image::load_from_memory(&bytes)?.into_rgb8();
        let resized = image::imageops::resize(
            &decoded,
            OUT_W,
            OUT_H,
            image::imageops::FilterType::Lanczos3,
        );
        Ok(resized)
    }
}

/// Query up to 8 items belonging to the collection and return the first
/// backdrop/poster URL found (backdrops preferred).
///
/// Smart collections (kind = Collection) contain items matched by
/// `collection_media_kind`, not by `parent_id`. Folder items use `parent_id`.
async fn find_backdrop_url(
    collection_id: Uuid,
    db: &sqlx::SqlitePool,
) -> anyhow::Result<String> {
    // Look up the collection itself to understand its kind.
    let collection = db::Media::get_by_id(db, &collection_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("collection {collection_id} not found"))?;

    let filter = match &collection.kind {
        db::MediaKind::Collection => {
            // Smart collection — items are matched by their media kind.
            let kinds = match &collection.collection_media_kind {
                Some(db::CollectionMediaKind::Movie) => vec![db::MediaKind::Movie],
                Some(db::CollectionMediaKind::Series) => vec![db::MediaKind::Series],
                Some(db::CollectionMediaKind::Music) => {
                    vec![db::MediaKind::Album, db::MediaKind::Artist]
                }
                Some(db::CollectionMediaKind::Collection) => {
                    vec![db::MediaKind::Collection]
                }
                Some(db::CollectionMediaKind::Playlist) => {
                    vec![db::MediaKind::Playlist]
                }
                None => vec![db::MediaKind::Movie, db::MediaKind::Series],
            };
            db::MediaFilter {
                kind: Some(kinds),
                limit: Some(8),
                ..Default::default()
            }
        }
        _ => {
            // Folder or other container — items have parent_id = collection_id.
            db::MediaFilter {
                parent_id: Some(collection_id),
                limit: Some(8),
                ..Default::default()
            }
        }
    };

    let items = db::Media::get_by_filter(db, &filter)
        .await?
        .records;

    // Prefer backdrop, then fall back to primary (poster).
    items
        .iter()
        .find_map(|m| {
            m.images
                .get(ImageKind::Backdrop)
                .map(|i| {
                    i.path
                        .clone()
                })
        })
        .or_else(|| {
            items
                .iter()
                .find_map(|m| {
                    m.images
                        .get(ImageKind::Primary)
                        .map(|i| {
                            i.path
                                .clone()
                        })
                })
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no backdrop or poster found for collection {collection_id}"
            )
        })
}

fn solid_background() -> RgbImage {
    RgbImage::from_pixel(OUT_W, OUT_H, image::Rgb([30, 30, 30]))
}

/// Blend a semi-transparent black overlay over the image to darken it,
/// matching Jellyfin's `SKColors.Black.WithAlpha(0x78)` step.
fn apply_dark_overlay(mut img: RgbImage) -> RgbImage {
    let inv = 1.0 - OVERLAY_ALPHA;
    for pixel in img.pixels_mut() {
        pixel[0] = (pixel[0] as f32 * inv) as u8;
        pixel[1] = (pixel[1] as f32 * inv) as u8;
        pixel[2] = (pixel[2] as f32 * inv) as u8;
    }
    img
}

/// Render the library name centered on the image in white text.
fn draw_label(mut img: RgbImage, name: &str) -> anyhow::Result<RgbImage> {
    let font = FontRef::try_from_slice(FONT_DATA)
        .map_err(|e| anyhow::anyhow!("font load failed: {e:?}"))?;

    // Start at ~20% of image height and scale down until it fits within 90% width.
    let mut scale = PxScale::from(OUT_H as f32 * 0.20);
    let max_width = OUT_W as f32 * MAX_TEXT_FRACTION;
    let mut tw = measure_text_width(&font, scale, name);
    if tw > max_width {
        scale = PxScale::from(scale.x * max_width / tw);
        tw = measure_text_width(&font, scale, name);
    }

    let text_height = {
        let sf = font.as_scaled(scale);
        sf.ascent() - sf.descent()
    };

    let x = ((OUT_W as f32 - tw) / 2.0) as i32;
    let y = ((OUT_H as f32 - text_height) / 2.0) as i32;

    draw_text_mut(&mut img, Rgb([255, 255, 255]), x, y, scale, &font, name);

    Ok(img)
}

fn measure_text_width(font: &FontRef<'_>, scale: PxScale, text: &str) -> f32 {
    let scaled = font.as_scaled(scale);
    let mut width = 0.0f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let glyph_id = scaled.glyph_id(c);
        if let Some(p) = prev {
            width += scaled.kern(p, glyph_id);
        }
        width += scaled.h_advance(glyph_id);
        prev = Some(glyph_id);
    }
    width
}

fn encode_jpeg(img: RgbImage) -> anyhow::Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Jpeg)?;
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------------------
// Image processing helpers (sync — run in spawn_blocking)
// ---------------------------------------------------------------------------

fn process_image_sync(
    bytes: &[u8],
    opts: &ImageProcessOptions,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let img = image::load_from_memory(bytes)?;
    let has_alpha = img
        .color()
        .has_alpha();
    let img = apply_sizing(img, opts);
    let img = if let Some(sigma) = opts.blur {
        img.blur(sigma as f32)
    } else {
        img
    };

    // Auto-preserve transparency: use PNG when the source has alpha and the caller
    // didn't explicitly request a lossy format.
    let use_png = matches!(
        opts.format
            .as_deref(),
        Some("png")
    ) || (has_alpha
        && !matches!(
            opts.format
                .as_deref(),
            Some("jpeg" | "jpg")
        ));

    let quality = opts
        .quality
        .unwrap_or(90);
    let mut buf = Cursor::new(Vec::<u8>::new());
    if use_png {
        img.write_to(&mut buf, ImageFormat::Png)?;
        Ok((buf.into_inner(), "image/png"))
    } else {
        use image::codecs::jpeg::JpegEncoder;
        img.write_with_encoder(JpegEncoder::new_with_quality(&mut buf, quality))?;
        Ok((buf.into_inner(), "image/jpeg"))
    }
}

/// Resize `img` according to sizing params (fill → exact → max priority order).
fn apply_sizing(img: DynamicImage, opts: &ImageProcessOptions) -> DynamicImage {
    let orig_w = img.width();
    let orig_h = img.height();

    // Priority 1: fill — scale down to fit inside box, no upscale, maintain AR.
    if opts
        .fill_width
        .is_some()
        || opts
            .fill_height
            .is_some()
    {
        let scale_x = opts
            .fill_width
            .map(|fw| fw as f32 / orig_w as f32)
            .unwrap_or(f32::MAX);
        let scale_y = opts
            .fill_height
            .map(|fh| fh as f32 / orig_h as f32)
            .unwrap_or(f32::MAX);
        let scale = scale_x
            .min(scale_y)
            .min(1.0);
        if scale < 1.0 {
            let nw = ((orig_w as f32 * scale) as u32).max(1);
            let nh = ((orig_h as f32 * scale) as u32).max(1);
            return img.resize(nw, nh, image::imageops::FilterType::Lanczos3);
        }
        return img;
    }

    // Priority 2: exact width / height (missing dimension maintains AR).
    if opts
        .width
        .is_some()
        || opts
            .height
            .is_some()
    {
        let nw = opts
            .width
            .unwrap_or(u32::MAX);
        let nh = opts
            .height
            .unwrap_or(u32::MAX);
        return img.resize(nw, nh, image::imageops::FilterType::Lanczos3);
    }

    // Priority 3: max — cap size, scale down only.
    let cap_w = opts
        .max_width
        .unwrap_or(u32::MAX);
    let cap_h = opts
        .max_height
        .unwrap_or(u32::MAX);
    if orig_w > cap_w || orig_h > cap_h {
        return img.resize(cap_w, cap_h, image::imageops::FilterType::Lanczos3);
    }

    img
}
