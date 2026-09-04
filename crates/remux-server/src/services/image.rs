use std::{collections::HashSet, io::Cursor, path::PathBuf};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use futures::StreamExt;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use uuid::Uuid;

use crate::{
    api::image::detect_content_type,
    db,
    db::ImageKind,
    sdks::remux::{
        CollectionFontFamily, CollectionFontWeight, CollectionImageConfig,
        CollectionOverlay, CollectionPosterLayout,
    },
};

/// Width/height of generated library placeholder images (16:9).
const OUT_W: u32 = 960;
const OUT_H: u32 = 540;

/// Black overlay opacity — matches Jellyfin's `0x78` (≈ 47%).
const OVERLAY_ALPHA: f32 = 0x78 as f32 / 255.0;

/// Maximum text width as a fraction of image width before scaling down.
const MAX_TEXT_FRACTION: f32 = 0.90;

/// TMDB image base URL for logo downloads.
const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w300";

/// Standard poster dimensions (2:3 ratio) used in fan layout.
const POSTER_W: u32 = 190;
const POSTER_H: u32 = 285;

/// A four-by-four poster sheet gives the grid enough depth to overflow the frame.
const GRID_POSTER_LIMIT: usize = 16;
/// A grid cell should use its own source poster whenever the collection has
/// enough artwork. Repeating posters makes the projected sheet look synthetic.
const GRID_SOURCE_POSTER_LIMIT: usize = GRID_POSTER_LIMIT;
const GRID_COLUMNS: usize = 4;
const GRID_POSTER_W: u32 = 161;
const GRID_POSTER_H: u32 = 241;
const GRID_GUTTER: u32 = 10;

/// Inset for text overlays, so copy does not sit flush with the image edge.
const TEXT_LEFT_MARGIN: i32 = 100;

/// Side of the square buffer used for rotation (must exceed sqrt(POSTER_W²+POSTER_H²) ≈ 342).
const ROTATION_CANVAS: u32 = 380;

/// Rounded-corner radius on each poster (pixels).
const CORNER_RADIUS: u32 = 10;

/// Left text area: left edge of posters starts here (fan occupies right ~55% of canvas).
const TEXT_AREA_END: u32 = (OUT_W as f32 * 0.42) as u32;

static FONT_BOLD: &[u8] = include_bytes!("../../assets/fonts/LiberationSans-Bold.ttf");
static FONT_ROBOTO: &[u8] = include_bytes!("../../assets/fonts/Roboto.ttf");
static FONT_OPEN_SANS: &[u8] = include_bytes!("../../assets/fonts/OpenSans.ttf");
static FONT_LATO: &[u8] = include_bytes!("../../assets/fonts/Lato.ttf");
static FONT_MONTSERRAT: &[u8] = include_bytes!("../../assets/fonts/Montserrat.ttf");
static FONT_POPPINS: &[u8] = include_bytes!("../../assets/fonts/Poppins.ttf");
static FONT_OSWALD: &[u8] = include_bytes!("../../assets/fonts/Oswald.ttf");
static FONT_RALEWAY: &[u8] = include_bytes!("../../assets/fonts/Raleway.ttf");
static FONT_MERRIWEATHER: &[u8] = include_bytes!("../../assets/fonts/Merriweather.ttf");
static FONT_PLAYFAIR_DISPLAY: &[u8] =
    include_bytes!("../../assets/fonts/PlayfairDisplay.ttf");
static FONT_BEBAS_NEUE: &[u8] = include_bytes!("../../assets/fonts/BebasNeue.ttf");

#[allow(clippy::incompatible_msrv)]
static HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build image http client")
    });

fn collection_font_bytes(font: CollectionFontFamily) -> &'static [u8] {
    match font {
        CollectionFontFamily::Roboto => FONT_ROBOTO,
        CollectionFontFamily::OpenSans => FONT_OPEN_SANS,
        CollectionFontFamily::Lato => FONT_LATO,
        CollectionFontFamily::Montserrat => FONT_MONTSERRAT,
        CollectionFontFamily::Poppins => FONT_POPPINS,
        CollectionFontFamily::Oswald => FONT_OSWALD,
        CollectionFontFamily::Raleway => FONT_RALEWAY,
        CollectionFontFamily::Merriweather => FONT_MERRIWEATHER,
        CollectionFontFamily::PlayfairDisplay => FONT_PLAYFAIR_DISPLAY,
        CollectionFontFamily::BebasNeue => FONT_BEBAS_NEUE,
    }
}

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
            "v2|{}|fw={:?}|fh={:?}|w={:?}|h={:?}|mw={:?}|mh={:?}|q={:?}|bl={:?}|bg={:?}|fmt={:?}",
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

    /// Returns the local path for a specific image type with the given extension.
    pub fn image_path(
        data_dir: &std::path::Path,
        id: Uuid,
        image_type: &str,
        ext: &str,
    ) -> PathBuf {
        Self::image_dir(data_dir, id).join(format!(
            "{}.{}",
            image_type.to_lowercase(),
            ext
        ))
    }

    /// Generate the library placeholder image, write it to disk, save the path
    /// to `media_images` in the DB, and return the bytes.
    pub async fn library_image(
        data_dir: &std::path::Path,
        id: Uuid,
        name: &str,
        db: &sqlx::SqlitePool,
    ) -> anyhow::Result<Vec<u8>> {
        // Check for an existing generated file (either jpg or png).
        for ext in ["jpg", "png"] {
            let p = Self::image_path(data_dir, id, "primary", ext);
            if p.exists() {
                return Ok(tokio::fs::read(&p).await?);
            }
        }

        let bytes = Self::generate(id, name, db).await?;
        let ct = detect_content_type(&bytes);
        let ext = ext_for_content_type(ct);
        let path = Self::image_path(data_dir, id, "primary", ext);
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
        let ext = ext_for_content_type(detect_content_type(bytes));
        let path = Self::image_path(data_dir, id, &kind.to_string(), ext);
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
        // Look up the stored path from the DB rather than reconstructing it —
        // the extension varies by format (gif, png, webp, jpg).
        if let Ok(images) = db::MediaImage::get_for_media(db, &id).await {
            if let Some(img) = images.get(kind) {
                if img
                    .path
                    .starts_with('/')
                {
                    let _ =
                        tokio::fs::remove_file(std::path::Path::new(&img.path)).await;
                }
            }
        }
        // Sweep all known extensions for this kind — previous uploads of different
        // formats leave orphan files behind, and the generated placeholder is always
        // primary.jpg regardless of what the DB row points to.
        for ext in ["jpg", "png", "gif", "webp"] {
            let _ = tokio::fs::remove_file(Self::image_path(
                data_dir,
                id,
                &kind.to_string(),
                ext,
            ))
            .await;
        }
        db::MediaImage::delete_for_type(db, id, kind)
            .await
            .map_err(anyhow::Error::from)?;
        // Touch updated_at so the synthetic image tag (derived from it) changes,
        // busting client caches that keyed on the old tag.
        let _ = sqlx::query("UPDATE media SET updated_at = ? WHERE id = ?")
            .bind(chrono::Utc::now().naive_utc())
            .bind(id)
            .execute(db)
            .await;
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

        // GIFs are always served as-is to preserve animation.
        if detect_content_type(&bytes) == "image/gif" {
            return Ok((bytes, "image/gif"));
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

    /// Regenerate the collection image: delete cached file and DB row, then
    /// re-generate on next request. Call after `collection_image_config` changes.
    pub async fn invalidate_collection_image(
        data_dir: &std::path::Path,
        id: Uuid,
        db: &sqlx::SqlitePool,
    ) -> anyhow::Result<()> {
        Self::delete_image(data_dir, id, ImageKind::Primary, db).await
    }

    async fn generate(
        id: Uuid,
        name: &str,
        db: &sqlx::SqlitePool,
    ) -> anyhow::Result<Vec<u8>> {
        let collection = db::Media::get_by_id(db, &id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("collection {id} not found"))?;
        let config = collection
            .collection_image_config
            .clone();

        // Resolve a custom background before looking up the poster grid. It is
        // the entire composition when present, so poster downloads would be
        // wasted work.
        let custom_background_path = config
            .as_ref()
            .and_then(|_| {
                collection
                    .images
                    .get(ImageKind::Backdrop)
                    .map(|image| {
                        image
                            .path
                            .clone()
                    })
            });
        let custom_background = if let Some(path) = custom_background_path {
            let result = if path.contains("://") {
                Self::fetch_rgba(&path).await
            } else {
                Self::read_rgba(&path).await
            };
            result.ok()
        } else {
            None
        };

        let poster_limit = config
            .as_ref()
            .map(|c| {
                if c.layout == CollectionPosterLayout::Grid {
                    GRID_SOURCE_POSTER_LIMIT as u32
                } else {
                    4u32
                }
            })
            .unwrap_or(4);
        let poster_urls = if custom_background.is_none() {
            collect_poster_urls(id, &collection, db, poster_limit).await
        } else {
            Vec::new()
        };

        if poster_urls.is_empty() && config.is_none() {
            // Fallback: single backdrop with label (original behaviour).
            let bg = match find_backdrop_url_from_collection(&collection, db).await {
                Ok(src) => {
                    let result = if src.contains("://") {
                        Self::fetch_rgb(&src).await
                    } else {
                        Self::read_rgb(&src).await
                    };
                    match result {
                        Ok(img) => {
                            let resized = image::imageops::resize(
                                &img,
                                OUT_W,
                                OUT_H,
                                image::imageops::FilterType::Lanczos3,
                            );
                            apply_dark_overlay(resized)
                        }
                        Err(_) => solid_background(),
                    }
                }
                Err(_) => solid_background(),
            };
            let img = draw_label(bg, name)?;
            return encode_jpeg(img);
        }

        let posters =
            Self::load_poster_images(&poster_urls, poster_limit as usize).await;

        // Pre-fetch streaming logo (async) so the blocking closure has it ready.
        let logo_image: Option<RgbaImage> = match config
            .as_ref()
            .map(|c| &c.overlay)
        {
            Some(CollectionOverlay::StreamingLogo {
                logo_path: Some(path),
                ..
            }) => {
                let url = format!("{TMDB_IMAGE_BASE}{path}");
                async {
                    let bytes = HTTP_CLIENT
                        .get(&url)
                        .send()
                        .await?
                        .bytes()
                        .await?;
                    anyhow::Ok(image::load_from_memory(&bytes)?.into_rgba8())
                }
                .await
                .ok()
            }
            _ => None,
        };

        // CPU-bound fan composition + PNG encode runs on the blocking pool.
        let name_owned = name.to_string();
        let config_owned = config;
        let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
            let has_custom_background = custom_background.is_some();
            let mut canvas = custom_background
                .map(|image| {
                    DynamicImage::ImageRgba8(image)
                        .resize_to_fill(
                            OUT_W,
                            OUT_H,
                            image::imageops::FilterType::Lanczos3,
                        )
                        .into_rgba8()
                })
                .unwrap_or_else(|| {
                    RgbaImage::from_pixel(OUT_W, OUT_H, Rgba([18, 18, 22, 255]))
                });

            let layout = config_owned
                .as_ref()
                .map(|c| c.layout)
                .unwrap_or_default();
            let max_n = if layout == CollectionPosterLayout::Grid {
                GRID_POSTER_LIMIT
            } else {
                4
            };
            let n = posters
                .len()
                .min(max_n);
            if n > 0 && !has_custom_background {
                if layout == CollectionPosterLayout::Grid {
                    stamp_grid(&mut canvas, &posters[..n]);
                } else {
                    let positions = layout_positions(layout, n);
                    for idx in (0..n).rev() {
                        let (cx, cy, angle_deg) = positions[idx];
                        let resized = image::imageops::resize(
                            &posters[idx],
                            POSTER_W,
                            POSTER_H,
                            image::imageops::FilterType::Lanczos3,
                        );
                        rotate_and_stamp(&mut canvas, resized, cx, cy, angle_deg);
                    }
                }
            }

            let overlay = config_owned
                .as_ref()
                .map(|c| &c.overlay)
                .unwrap_or(&CollectionOverlay::None);
            apply_overlay_sync(&mut canvas, overlay, &name_owned, logo_image)?;

            encode_jpeg_rgba(canvas)
        })
        .await??;

        Ok(bytes)
    }

    async fn load_poster_images(urls: &[String], limit: usize) -> Vec<RgbaImage> {
        futures::stream::iter(
            urls.iter()
                .take(limit)
                .cloned()
                .map(|url| async move {
                    let img = if url.contains("://") {
                        Self::fetch_rgba(&url).await
                    } else {
                        Self::read_rgba(&url).await
                    };
                    img
                }),
        )
        // Keep source order stable while downloading up to eight images at
        // once. The old serial loop made a single generated image wait for
        // every remote poster in sequence.
        .buffered(8)
        .filter_map(|image| async move { image.ok() })
        .collect()
        .await
    }

    async fn fetch_rgb(url: &str) -> anyhow::Result<RgbImage> {
        let bytes = HTTP_CLIENT
            .get(url)
            .send()
            .await?
            .bytes()
            .await?;
        Ok(image::load_from_memory(&bytes)?.into_rgb8())
    }

    async fn read_rgb(path: &str) -> anyhow::Result<RgbImage> {
        let bytes = tokio::fs::read(path).await?;
        Ok(image::load_from_memory(&bytes)?.into_rgb8())
    }

    async fn fetch_rgba(url: &str) -> anyhow::Result<RgbaImage> {
        let bytes = HTTP_CLIENT
            .get(url)
            .send()
            .await?
            .bytes()
            .await?;
        Ok(image::load_from_memory(&bytes)?.into_rgba8())
    }

    async fn read_rgba(path: &str) -> anyhow::Result<RgbaImage> {
        let bytes = tokio::fs::read(path).await?;
        Ok(image::load_from_memory(&bytes)?.into_rgba8())
    }

    async fn fetch_and_resize(url: &str) -> anyhow::Result<RgbImage> {
        let img = Self::fetch_rgb(url).await?;
        Ok(image::imageops::resize(
            &img,
            OUT_W,
            OUT_H,
            image::imageops::FilterType::Lanczos3,
        ))
    }

    async fn read_local_and_resize(path: &str) -> anyhow::Result<RgbImage> {
        let img = Self::read_rgb(path).await?;
        Ok(image::imageops::resize(
            &img,
            OUT_W,
            OUT_H,
            image::imageops::FilterType::Lanczos3,
        ))
    }
}

fn collection_item_filter(collection: &db::Media) -> db::MediaFilter {
    match &collection.kind {
        db::MediaKind::Collection => {
            let kinds = match &collection.collection_media_kind {
                Some(db::CollectionMediaKind::Movie) => vec![db::MediaKind::Movie],
                Some(db::CollectionMediaKind::Series) => vec![db::MediaKind::Series],
                Some(db::CollectionMediaKind::Mixed) => {
                    vec![db::MediaKind::Movie, db::MediaKind::Series]
                }
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
        _ => db::MediaFilter {
            parent_id: Some(collection.id),
            limit: Some(8),
            ..Default::default()
        },
    }
}

/// Collect distinct poster image paths/URLs from collection items.
async fn collect_poster_urls(
    _id: Uuid,
    collection: &db::Media,
    db: &sqlx::SqlitePool,
    limit: u32,
) -> Vec<String> {
    // Inspect extra records so duplicate artwork does not consume a grid cell.
    let filter = db::MediaFilter {
        limit: Some(limit.saturating_mul(2)),
        ..collection_item_filter(collection)
    };
    let Ok(result) = db::Media::get_by_filter(db, &filter).await else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    result
        .records
        .iter()
        .filter_map(|m| {
            m.images
                .get(ImageKind::Primary)
                .map(|i| {
                    i.path
                        .clone()
                })
                .or_else(|| {
                    m.images
                        .get(ImageKind::Backdrop)
                        .map(|i| {
                            i.path
                                .clone()
                        })
                })
        })
        .filter(|path| seen.insert(path.clone()))
        .take(limit as usize)
        .collect()
}

/// Find a backdrop or poster URL from collection items (for the legacy single-image path).
async fn find_backdrop_url_from_collection(
    collection: &db::Media,
    db: &sqlx::SqlitePool,
) -> anyhow::Result<String> {
    let filter = db::MediaFilter {
        limit: Some(8),
        ..collection_item_filter(collection)
    };
    let items = db::Media::get_by_filter(db, &filter)
        .await?
        .records;

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
            anyhow::anyhow!("no image found for collection {}", collection.id)
        })
}

/// Poster positions (canvas center_x, center_y, clockwise angle°) for a given layout and n posters.
fn layout_positions(layout: CollectionPosterLayout, n: usize) -> Vec<(i64, i64, f32)> {
    match layout {
        // Grid is rendered as one transformed 4×4 plane by `stamp_grid`.
        CollectionPosterLayout::Grid => Vec::new(),
        // Clean horizontal shelf, barely overlapping.
        CollectionPosterLayout::Row => match n {
            1 => vec![(720, 270, 0.0)],
            2 => vec![(645, 270, 0.0), (795, 270, 0.0)],
            3 => vec![(570, 270, 0.0), (715, 270, 0.0), (860, 270, 0.0)],
            _ => vec![
                (515, 270, 0.0),
                (638, 270, 0.0),
                (762, 270, 0.0),
                (885, 270, 0.0),
            ],
        },
        // Wide artistic scatter with bold angles and height variation.
        CollectionPosterLayout::Scatter => match n {
            1 => vec![(720, 270, 0.0)],
            2 => vec![(628, 288, -14.0), (808, 248, 16.0)],
            3 => vec![(570, 298, -17.0), (718, 245, 10.0), (850, 285, -11.0)],
            _ => vec![
                (545, 302, -19.0),
                (668, 240, 13.0),
                (778, 296, -9.0),
                (882, 244, 17.0),
            ],
        },
    }
}

/// Apply rounded corners (CORNER_RADIUS px) to an RGBA image in-place.
fn apply_rounded_corners(img: &mut RgbaImage) {
    let w = img.width();
    let h = img.height();
    let r = CORNER_RADIUS as f32;
    for py in 0..h {
        for px in 0..w {
            let fx = px as f32;
            let fy = py as f32;
            let in_corner = (fx < r && fy < r)
                || (fx > w as f32 - r - 1.0 && fy < r)
                || (fx < r && fy > h as f32 - r - 1.0)
                || (fx > w as f32 - r - 1.0 && fy > h as f32 - r - 1.0);
            if in_corner {
                let cx = if fx < r { r } else { w as f32 - r - 1.0 };
                let cy = if fy < r { r } else { h as f32 - r - 1.0 };
                if ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() > r {
                    img.get_pixel_mut(px, py)[3] = 0;
                }
            }
        }
    }
}

/// Build a non-overlapping 4×4 poster sheet, then project it as a single
/// skewed plane. The plane starts beyond the text area and is intentionally
/// larger than the output canvas, so it reads as a background element.
fn stamp_grid(canvas: &mut RgbaImage, posters: &[RgbaImage]) {
    let (grid_w, grid_h) = grid_dimensions();
    let mut grid = RgbaImage::new(grid_w, grid_h);

    let prepared_posters: Vec<RgbaImage> = posters
        .iter()
        .map(|poster| {
            let mut resized = image::imageops::resize(
                poster,
                GRID_POSTER_W,
                GRID_POSTER_H,
                image::imageops::FilterType::Lanczos3,
            );
            apply_rounded_corners(&mut resized);
            resized
        })
        .collect();

    // Smaller collections still need a complete background plane. Reuse their
    // prepared posters cyclically instead of resizing the same image per cell.
    for idx in 0..GRID_POSTER_LIMIT {
        let poster = &prepared_posters[idx % prepared_posters.len()];
        let col = idx % GRID_COLUMNS;
        let row = idx / GRID_COLUMNS;
        let x = col as u32 * (GRID_POSTER_W + GRID_GUTTER);
        let y = row as u32 * (GRID_POSTER_H + GRID_GUTTER);
        image::imageops::overlay(&mut grid, poster, x.into(), y.into());
    }

    // An affine projection with a subtle clockwise rotation and horizontal
    // shear. Its centre sits off the right edge; most of the 4×4 sheet is
    // therefore clipped, leaving roughly the right third of the image visible.
    let (a, b, c, d) = (0.78f32, -0.12f32, 0.12f32, 0.75f32);
    let determinant = a * d - b * c;
    let centre_x = 875.0;
    let centre_y = 270.0;
    let half_w = grid_w as f32 / 2.0;
    let half_h = grid_h as f32 / 2.0;
    let mut projected = RgbaImage::new(OUT_W, OUT_H);

    for y in 0..OUT_H {
        for x in 0..OUT_W {
            let dx = x as f32 - centre_x;
            let dy = y as f32 - centre_y;
            let src_x = (d * dx - b * dy) / determinant + half_w;
            let src_y = (-c * dx + a * dy) / determinant + half_h;
            if src_x < 0.0
                || src_y < 0.0
                || src_x >= (grid_w - 1) as f32
                || src_y >= (grid_h - 1) as f32
            {
                continue;
            }

            let x0 = src_x as u32;
            let y0 = src_y as u32;
            let x1 = x0 + 1;
            let y1 = y0 + 1;
            let tx = src_x.fract();
            let ty = src_y.fract();
            let interpolate = |p00: u8, p10: u8, p01: u8, p11: u8| {
                let top = p00 as f32 + (p10 as f32 - p00 as f32) * tx;
                let bottom = p01 as f32 + (p11 as f32 - p01 as f32) * tx;
                (top + (bottom - top) * ty) as u8
            };
            let p00 = grid
                .get_pixel(x0, y0)
                .0;
            let p10 = grid
                .get_pixel(x1, y0)
                .0;
            let p01 = grid
                .get_pixel(x0, y1)
                .0;
            let p11 = grid
                .get_pixel(x1, y1)
                .0;
            projected.put_pixel(
                x,
                y,
                Rgba([
                    interpolate(p00[0], p10[0], p01[0], p11[0]),
                    interpolate(p00[1], p10[1], p01[1], p11[1]),
                    interpolate(p00[2], p10[2], p01[2], p11[2]),
                    interpolate(p00[3], p10[3], p01[3], p11[3]),
                ]),
            );
        }
    }

    image::imageops::overlay(canvas, &projected, 0, 0);
}

fn grid_dimensions() -> (u32, u32) {
    let rows = GRID_POSTER_LIMIT.div_ceil(GRID_COLUMNS) as u32;
    (
        GRID_COLUMNS as u32 * GRID_POSTER_W + (GRID_COLUMNS as u32 - 1) * GRID_GUTTER,
        rows * GRID_POSTER_H + (rows - 1) * GRID_GUTTER,
    )
}

/// Rotate `poster` by `angle_deg` (clockwise) and stamp it onto `canvas`
/// so its centre lands at (`cx`, `cy`).
fn rotate_and_stamp(
    canvas: &mut RgbaImage,
    mut poster: RgbaImage,
    cx: i64,
    cy: i64,
    angle_deg: f32,
) {
    apply_rounded_corners(&mut poster);

    let pw = poster.width();
    let ph = poster.height();

    // Embed the poster centred in a square big enough that rotation never clips it.
    let buf_size = ROTATION_CANVAS;
    let ox = ((buf_size - pw) / 2) as i64;
    let oy = ((buf_size - ph) / 2) as i64;
    let mut buf = RgbaImage::new(buf_size, buf_size);
    image::imageops::overlay(&mut buf, &poster, ox, oy);

    // Bilinear inverse-mapping rotation (no external imageproc dep needed).
    let bx = buf_size as i32;
    let by = buf_size as i32;
    let half_x = buf_size as f32 / 2.0;
    let half_y = buf_size as f32 / 2.0;
    let theta = angle_deg * std::f32::consts::PI / 180.0;
    let cos_a = theta.cos();
    let sin_a = theta.sin();
    let mut rotated = RgbaImage::new(buf_size, buf_size);
    for oy in 0..buf_size {
        for ox in 0..buf_size {
            let fx = ox as f32 - half_x;
            let fy = oy as f32 - half_y;
            // Inverse rotation (CW angle → CCW inverse)
            let src_x = fx * cos_a + fy * sin_a + half_x;
            let src_y = -fx * sin_a + fy * cos_a + half_y;
            if src_x >= 0.0
                && src_x < (bx - 1) as f32
                && src_y >= 0.0
                && src_y < (by - 1) as f32
            {
                let x0 = src_x as u32;
                let y0 = src_y as u32;
                let x1 = (x0 + 1).min(buf_size - 1);
                let y1 = (y0 + 1).min(buf_size - 1);
                let fx2 = src_x.fract();
                let fy2 = src_y.fract();
                let p00 = buf
                    .get_pixel(x0, y0)
                    .0;
                let p10 = buf
                    .get_pixel(x1, y0)
                    .0;
                let p01 = buf
                    .get_pixel(x0, y1)
                    .0;
                let p11 = buf
                    .get_pixel(x1, y1)
                    .0;
                let interp = |a: u8, b: u8, c: u8, d: u8| -> u8 {
                    let top = a as f32 + (b as f32 - a as f32) * fx2;
                    let bot = c as f32 + (d as f32 - c as f32) * fx2;
                    (top + (bot - top) * fy2) as u8
                };
                rotated.put_pixel(
                    ox,
                    oy,
                    Rgba([
                        interp(p00[0], p10[0], p01[0], p11[0]),
                        interp(p00[1], p10[1], p01[1], p11[1]),
                        interp(p00[2], p10[2], p01[2], p11[2]),
                        interp(p00[3], p10[3], p01[3], p11[3]),
                    ]),
                );
            }
        }
    }

    let stamp_x = cx - buf_size as i64 / 2;
    let stamp_y = cy - buf_size as i64 / 2;
    image::imageops::overlay(canvas, &rotated, stamp_x, stamp_y);
}

/// Sync overlay renderer — called from inside `spawn_blocking`.
/// `logo_image` is the already-downloaded streaming logo (if any).
fn apply_overlay_sync(
    canvas: &mut RgbaImage,
    overlay: &CollectionOverlay,
    fallback_name: &str,
    logo_image: Option<RgbaImage>,
) -> anyhow::Result<()> {
    match overlay {
        CollectionOverlay::None => {}
        CollectionOverlay::Text {
            text,
            font_size,
            font_family,
            font_weight,
        } => {
            let label = text
                .as_deref()
                .unwrap_or(fallback_name);
            let size = (*font_size).unwrap_or(90) as f32;
            let font_bytes = collection_font_bytes(font_family.unwrap_or_default());
            let mut font = FontRef::try_from_slice(font_bytes)
                .map_err(|e| anyhow::anyhow!("font load: {e:?}"))?;
            use ab_glyph::VariableFont;
            let weight = match font_weight.unwrap_or_default() {
                CollectionFontWeight::Regular => 400.0,
                CollectionFontWeight::Bold => 700.0,
            };
            let _ = font.set_variation(b"wght", weight);

            let max_w = TEXT_AREA_END as f32 - TEXT_LEFT_MARGIN as f32;
            let scale = PxScale::from(size);

            let lines = wrap_words(&font, scale, label, max_w);

            let sf = font.as_scaled(scale);
            let line_h = sf.ascent() - sf.descent();
            let line_gap = line_h * 0.18;
            let step = line_h + line_gap;
            let total_h = step * lines.len() as f32 - line_gap;
            let start_y = ((OUT_H as f32 - total_h) / 2.0) as i32;

            for (i, line) in lines
                .iter()
                .enumerate()
            {
                let y = start_y + (i as f32 * step) as i32;
                draw_text_mut(
                    canvas,
                    Rgba([255, 255, 255, 255]),
                    TEXT_LEFT_MARGIN,
                    y,
                    scale,
                    &font,
                    line,
                );
            }
        }
        CollectionOverlay::StreamingLogo { provider_name, .. } => {
            if let Some(logo) = logo_image {
                let max_logo_h = OUT_H / 3;
                let max_logo_w = OUT_W / 3;
                let scale = (max_logo_h as f32 / logo.height() as f32)
                    .min(max_logo_w as f32 / logo.width() as f32);
                let lw = (logo.width() as f32 * scale) as u32;
                let lh = (logo.height() as f32 * scale) as u32;
                let scaled = image::imageops::resize(
                    &logo,
                    lw,
                    lh,
                    image::imageops::FilterType::Lanczos3,
                );
                let margin = 20i64;
                let lx = OUT_W as i64 - lw as i64 - margin;
                let ly = OUT_H as i64 - lh as i64 - margin;
                image::imageops::overlay(canvas, &scaled, lx, ly);
            } else {
                let label = provider_name
                    .as_deref()
                    .unwrap_or("Streaming");
                let font = FontRef::try_from_slice(FONT_BOLD)
                    .map_err(|e| anyhow::anyhow!("font: {e:?}"))?;
                let scale = PxScale::from(60.0);
                let tw = measure_text_width(&font, scale, label);
                let sf = font.as_scaled(scale);
                let th = sf.ascent() - sf.descent();
                let x = ((OUT_W as f32 - tw) / 2.0) as i32;
                let y = ((OUT_H as f32 - th) / 2.0) as i32;
                draw_text_mut(
                    canvas,
                    Rgba([255, 255, 255, 255]),
                    x,
                    y,
                    scale,
                    &font,
                    label,
                );
            }
        }
    }
    Ok(())
}

/// Break `text` into lines where each line fits within `max_w` pixels at `scale`.
fn wrap_words(
    font: &FontRef<'_>,
    scale: PxScale,
    text: &str,
    max_w: f32,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && measure_text_width(font, scale, &candidate) > max_w {
            lines.push(current);
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(text.to_string());
    }
    lines
}

fn solid_background() -> RgbImage {
    RgbImage::from_pixel(OUT_W, OUT_H, Rgb([30, 30, 30]))
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
    let font = FontRef::try_from_slice(FONT_BOLD)
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

fn ext_for_content_type(ct: &str) -> &'static str {
    match ct {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpg",
    }
}

fn encode_jpeg(img: RgbImage) -> anyhow::Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Jpeg)?;
    Ok(buf.into_inner())
}

/// Generated collection art is fully opaque photographic content. JPEG avoids
/// the disproportionately expensive lossless PNG compression of a poster grid.
fn encode_jpeg_rgba(img: RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    let rgb = DynamicImage::ImageRgba8(img).into_rgb8();
    use image::codecs::jpeg::JpegEncoder;
    JpegEncoder::new_with_quality(&mut buf, 90)
        .encode_image(&DynamicImage::ImageRgb8(rgb))?;
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

    // Priority 1: fill — cover the requested box, including upscaling local
    // generated art when the client asks for a larger rendition. With both
    // dimensions supplied this deliberately crops the excess so the response
    // has the exact dimensions requested by Jellyfin clients.
    if opts
        .fill_width
        .is_some()
        || opts
            .fill_height
            .is_some()
    {
        return match (opts.fill_width, opts.fill_height) {
            (Some(width), Some(height)) => {
                img.resize_to_fill(width, height, image::imageops::FilterType::Lanczos3)
            }
            (Some(width), None) => {
                img.resize(width, u32::MAX, image::imageops::FilterType::Lanczos3)
            }
            (None, Some(height)) => {
                img.resize(u32::MAX, height, image::imageops::FilterType::Lanczos3)
            }
            (None, None) => unreachable!("fill sizing requires a dimension"),
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_is_four_by_four_with_gutters() {
        assert_eq!(
            grid_dimensions(),
            (
                4 * GRID_POSTER_W + 3 * GRID_GUTTER,
                4 * GRID_POSTER_H + 3 * GRID_GUTTER,
            )
        );
    }

    #[test]
    fn fill_dimensions_upscale_and_crop_to_the_requested_size() {
        let source = DynamicImage::new_rgba8(960, 540);
        let opts = ImageProcessOptions {
            fill_width: Some(1170),
            fill_height: Some(657),
            ..Default::default()
        };

        let result = apply_sizing(source, &opts);

        assert_eq!((result.width(), result.height()), (1170, 657));
    }
}
