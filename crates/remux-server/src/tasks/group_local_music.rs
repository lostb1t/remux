use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{AppContext, api, common, db, playback::probe::probe_media};

/// Reads embedded tags from local (opendal-local) audio files and groups
/// orphaned track rows (no parent album / grandparent artist) into synthesized
/// Artist and Album rows so they become browsable in Jellyfin/Finamp clients.
///
/// Local tracks are created bare by the opendal scan (no album/artist link and
/// no metadata source), and the Deezer enrichment only covers tracks it can
/// match — underground/self-released music never groups. This task fills that
/// gap using the files' own ID3/Vorbis tags (via ffprobe), falling back to the
/// folder path when a tag is missing. Synthesized artists/albums merge into any
/// existing artist/album of the same (normalized) name, so a later Deezer pass
/// can enrich them with canonical artwork.
pub struct GroupLocalMusicTask;

struct Derived {
    track_id: Uuid,
    title: String,
    artist_name: String,
    album_name: String,
    track_no: Option<i64>,
    /// Disc number (drives `ParentIndexNumber`).
    disc_no: Option<i64>,
    /// Release year (drives `ProductionYear` / `PremiereDate`).
    year: Option<i32>,
    /// Directory of the file, used to find a folder cover for the album.
    dir: String,
    /// Source file used to extract embedded album artwork when no folder cover exists.
    path: String,
    /// Duration in whole seconds (drives item-level `RunTimeTicks`).
    runtime_secs: Option<i64>,
    /// Full ffprobe result (container, bitrate, size, real media streams) —
    /// populates `MediaSources` so local tracks match Jellyfin's shape.
    probe_data: Option<api::MediaSourceInfo>,
    /// Genre names parsed from the `genre` tag; become `MusicGenre` entities +
    /// `media_relations` so `Genres`/`GenreItems` match Jellyfin.
    genres: Vec<String>,
}

/// Split a `genre` tag into individual genre names. Jellyfin treats `;`, `/`,
/// `|` and `\` as multi-value delimiters; commas are left intact because real
/// genres contain them (e.g. "Folk, World, & Country"). Trims, drops blanks,
/// de-duplicates case-insensitively while preserving first-seen casing.
fn parse_genres(tags: &HashMap<String, String>) -> Vec<String> {
    let raw = match tags.get("genre") {
        Some(v) if !v.is_empty() => v.as_str(),
        _ => return Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for part in raw.split([';', '/', '|', '\\']) {
        let name = part.trim();
        if !name.is_empty() && seen.insert(name.to_lowercase()) {
            out.push(name.to_string());
        }
    }
    out
}

/// Parse a 4-digit year from a `date`/`year` tag like "2018", "2018-05-01".
fn parse_year(tags: &HashMap<String, String>) -> Option<i32> {
    for k in ["date", "originaldate", "year"] {
        if let Some(v) = tags.get(k) {
            let digits: String = v
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if digits.len() == 4 {
                if let Ok(y) = digits.parse::<i32>() {
                    if (1000..=3000).contains(&y) {
                        return Some(y);
                    }
                }
            }
        }
    }
    None
}

fn dir_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((d, _)) => d.to_string(),
        None => String::new(),
    }
}

/// Midnight on Jan 1 of `year` — Jellyfin derives ProductionYear/PremiereDate
/// from the release date.
fn year_to_date(year: i32) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDate::from_ymd_opt(year, 1, 1)?.and_hms_opt(0, 0, 0)
}

/// Find a folder cover image next to the tracks (cover/folder/front/albumart).
fn find_folder_cover(dir: &str) -> Option<String> {
    const STEMS: &[&str] = &["cover", "folder", "front", "albumart", "album", "thumb"];
    const EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];
    for entry in std::fs::read_dir(dir)
        .ok()?
        .flatten()
    {
        let name = entry
            .file_name()
            .to_string_lossy()
            .to_lowercase();
        if let Some((stem, ext)) = name.rsplit_once('.') {
            if EXTS.contains(&ext) && STEMS.contains(&stem) {
                return Some(
                    entry
                        .path()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    None
}

fn extract_embedded_cover(
    source: &str,
    data_dir: &Path,
    album_id: Uuid,
) -> Option<String> {
    let target =
        crate::services::image::ImageService::image_path(data_dir, album_id, "primary");
    if target
        .metadata()
        .is_ok_and(|metadata| metadata.len() > 0)
    {
        return Some(
            target
                .to_string_lossy()
                .into_owned(),
        );
    }

    let parent = target.parent()?;
    std::fs::create_dir_all(parent).ok()?;
    let temporary = temporary_cover_path(&target);
    let ffmpeg = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into());
    let status = Command::new(ffmpeg)
        .args([
            "-v",
            "error",
            "-y",
            "-i",
            source,
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-an",
            "-c:v",
            "mjpeg",
        ])
        .arg(&temporary)
        .status()
        .ok()?;

    if !status.success()
        || !temporary
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 0)
    {
        let _ = std::fs::remove_file(&temporary);
        return None;
    }

    std::fs::rename(&temporary, &target).ok()?;
    Some(
        target
            .to_string_lossy()
            .into_owned(),
    )
}

fn temporary_cover_path(target: &Path) -> PathBuf {
    target.with_file_name("primary.tmp.jpg")
}

fn normalize(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strip a trailing " (YYYY)" year and " [tag]" suffix from a folder-derived
/// album name.
fn clean_album_name(s: &str) -> String {
    let mut t = s
        .trim()
        .to_string();
    if let Some(idx) = t.rfind(" [") {
        t.truncate(idx);
    }
    // Remove a trailing " (1998)"-style year group.
    let trimmed = t.trim_end();
    if trimmed.ends_with(')') {
        if let Some(open) = trimmed.rfind(" (") {
            let inner = &trimmed[open + 2..trimmed.len() - 1];
            if inner.len() == 4
                && inner
                    .chars()
                    .all(|c| c.is_ascii_digit())
            {
                t.truncate(open);
            }
        }
    }
    t.trim()
        .to_string()
}

fn ffprobe_tags(path: &str) -> HashMap<String, String> {
    let bin = std::env::var("FFPROBE_PATH").unwrap_or_else(|_| "ffprobe".into());
    let mut map = HashMap::new();
    let output = std::process::Command::new(bin)
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format_tags=artist,album_artist,albumartist,album,title,track,\
disc,disc_number,date,originaldate,year,genre",
            "-of",
            "json",
            path,
        ])
        .output();
    if let Ok(out) = output {
        if out
            .status
            .success()
        {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(tags) = v
                    .get("format")
                    .and_then(|f| f.get("tags"))
                    .and_then(|t| t.as_object())
                {
                    for (k, val) in tags {
                        if let Some(s) = val.as_str() {
                            let s = s.trim();
                            if !s.is_empty() {
                                map.insert(k.to_lowercase(), s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    map
}

fn stem(path: &str) -> String {
    let file = path
        .rsplit('/')
        .next()
        .unwrap_or(path);
    match file.rfind('.') {
        Some(i) => file[..i].to_string(),
        None => file.to_string(),
    }
}

/// Derive `(artist, album)` from a file's folder structure, used only as a
/// fallback when the file itself carries no artist/album tags.
///
/// `roots` are the configured opendal-local library roots (e.g. `/mnt/music`).
/// Stripping the matching root lets us tell a loose file directly under the
/// library root (`<root>/<folder>/track` → artist == album == folder) apart
/// from a properly nested `<root>/<artist>/<album>/track`. Roots are read from
/// the addon config at runtime — never hardcoded — so this works on any server.
fn derive_from_path(path: &str, roots: &[String]) -> (String, String) {
    let rel = roots
        .iter()
        .find(|r| path.starts_with(&format!("{r}/")))
        .map(|r| &path[r.len() + 1..])
        .unwrap_or(path);
    let mut parts: Vec<&str> = rel
        .split('/')
        .collect();
    parts.pop(); // drop the filename
    let parts: Vec<&str> = parts
        .into_iter()
        .filter(|p| {
            let pl = p.to_lowercase();
            !(pl.starts_with("digital media")
                || pl.starts_with("disc ")
                || pl.starts_with("cd "))
        })
        .collect();
    match parts.len() {
        0 => ("Unknown Artist".to_string(), "Unknown Album".to_string()),
        1 => (parts[0].to_string(), clean_album_name(parts[0])),
        _ => (parts[0].to_string(), clean_album_name(parts[1])),
    }
}

fn parse_track_no(tag: Option<&String>, fallback: Option<i64>) -> Option<i64> {
    if let Some(t) = tag {
        if let Some(first) = t
            .split('/')
            .next()
        {
            if let Ok(n) = first
                .trim()
                .parse::<i64>()
            {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    fallback.filter(|n| *n > 0)
}

fn derive(
    track_id: Uuid,
    path: &str,
    opendal_track_no: Option<i64>,
    opendal_title: Option<String>,
    tags: &HashMap<String, String>,
    roots: &[String],
) -> Derived {
    let (path_artist, path_album) = derive_from_path(path, roots);
    let artist_name = tags
        .get("album_artist")
        .or_else(|| tags.get("albumartist"))
        .or_else(|| tags.get("artist"))
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or(path_artist);
    let album_name = tags
        .get("album")
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or(path_album);
    let title = tags
        .get("title")
        .cloned()
        .filter(|s| !s.is_empty())
        .or(opendal_title)
        .unwrap_or_else(|| stem(path));
    let track_no = parse_track_no(tags.get("track"), opendal_track_no);
    let disc_no = parse_track_no(
        tags.get("disc")
            .or_else(|| tags.get("disc_number")),
        None,
    );
    Derived {
        track_id,
        title,
        artist_name,
        album_name,
        track_no,
        disc_no,
        year: parse_year(tags),
        dir: dir_of(path),
        path: path.to_string(),
        runtime_secs: None,
        probe_data: None,
        genres: parse_genres(tags),
    }
}

#[async_trait]
impl Task for GroupLocalMusicTask {
    fn key(&self) -> &str {
        "GroupLocalMusic"
    }
    fn name(&self) -> &str {
        "Group Local Music"
    }
    fn description(&self) -> &str {
        "Reads tags from local audio files and groups orphaned tracks into artists and albums."
    }
    fn short_description(&self) -> &str {
        "Groups local tracks into artists/albums"
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Library
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        // Optional cap (operator-configurable) for validating grouping on a
        // subset before a full library pass. See `Config::group_local_music_limit`.
        let limit = ctx
            .config
            .group_local_music_limit;

        // 1. Orphaned local tracks (no album parent) with their file path.
        // Process local tracks that still need grouping (no album parent) OR
        // that were grouped by an earlier run but never probed (no probe_data →
        // no duration/streams/container). Re-running is idempotent.
        let base = "SELECT f.id, f.path, f.track_number, f.title \
                    FROM opendal_files f JOIN media m ON m.id = f.id \
                    LEFT JOIN media parent ON parent.id = m.parent_id \
                    WHERE f.media_kind = 'track' AND m.kind = 'track' \
                      AND (m.parent_id IS NULL OR m.probe_data IS NULL \
                           OR (json_extract(parent.external_ids, '$.custom_stremio_id') LIKE 'localalbum:%' \
                               AND NOT EXISTS (SELECT 1 FROM media_images image \
                                               WHERE image.media_id = parent.id \
                                                 AND image.image_type = 'primary') \
                               AND f.id = (SELECT candidate.id FROM opendal_files candidate \
                                           JOIN media candidate_media ON candidate_media.id = candidate.id \
                                           WHERE candidate_media.parent_id = parent.id \
                                             AND candidate.media_kind = 'track' \
                                           LIMIT 1)))";
        let sql = match limit {
            Some(n) => format!("{base} LIMIT {n}"),
            None => base.to_string(),
        };
        let rows: Vec<(Uuid, String, Option<i64>, Option<String>)> =
            sqlx::query_as(&sql)
                .fetch_all(&ctx.db)
                .await?;
        let total = rows.len();
        info!("GroupLocalMusic: {total} local tracks to group/probe");
        if total == 0 {
            progress.set(100.0);
            return Ok(());
        }

        // Configured opendal-local library roots, read from the addon config so
        // path-based fallback grouping is never tied to a specific server layout.
        let roots: Vec<String> = sqlx::query_scalar::<_, Option<String>>(
            "SELECT json_extract(preset, '$.config.path') FROM addons \
             WHERE json_extract(preset, '$.kind') = 'opendal-local' \
               AND json_extract(preset, '$.config.media_kind') = 'track'",
        )
        .fetch_all(&ctx.db)
        .await?
        .into_iter()
        .flatten()
        .collect();
        let roots = Arc::new(roots);

        // 2. Existing artist/album name maps so local tracks merge into an
        //    existing (e.g. Deezer) artist/album rather than duplicating it.
        let mut artist_by_name: HashMap<String, Uuid> =
            sqlx::query_as::<_, (Uuid, String)>(
                "SELECT id, title FROM media WHERE kind = 'artist'",
            )
            .fetch_all(&ctx.db)
            .await?
            .into_iter()
            .map(|(id, t)| (normalize(&t), id))
            .collect();
        let mut album_by_key: HashMap<(Uuid, String), Uuid> = HashMap::new();
        for (id, title, gp) in sqlx::query_as::<_, (Uuid, String, Option<Uuid>)>(
            // Exclude our own synthesized local albums from the dedup map so a
            // re-run re-creates them (same stable id → upsert updates) and can
            // backfill folder art + release year onto existing rows.
            "SELECT id, title, grandparent_id FROM media WHERE kind = 'album' \
             AND (external_ids IS NULL \
                  OR json_extract(external_ids, '$.custom_stremio_id') NOT LIKE 'localalbum:%')",
        )
        .fetch_all(&ctx.db)
        .await?
        {
            if let Some(gp) = gp {
                album_by_key.insert((gp, normalize(&title)), id);
            }
        }

        // 3. ffprobe each file (bounded concurrency) and derive artist/album/title.
        let derived: Vec<Derived> = stream::iter(rows)
            .map(|(id, path, track_no, opendal_title)| {
                let roots = Arc::clone(&roots);
                async move {
                    let p = path.clone();
                    // One blocking hop reads tags (for grouping) and runs a full
                    // ffprobe (for duration + real media streams).
                    let (tags, probe) = tokio::task::spawn_blocking(move || {
                        let tags = ffprobe_tags(&p);
                        let probe = probe_media(&p).ok();
                        (tags, probe)
                    })
                    .await
                    .unwrap_or_default();
                    let mut d =
                        derive(id, &path, track_no, opendal_title, &tags, &roots);
                    if let Some((mut source, _segments)) = probe {
                        // 100ns ticks → whole seconds for media.runtime.
                        d.runtime_secs = source
                            .run_time_ticks
                            .map(|t| t / 10_000_000)
                            .filter(|s| *s > 0);
                        // Jellyfin names a MediaSource after the file (stem), not
                        // the track title. Local tracks have no `stream_info`, so
                        // stash the stem on the stored source for the serializer.
                        source.name = Some(stem(&path));
                        d.probe_data = Some(source);
                    }
                    d
                }
            })
            .buffer_unordered(8)
            .collect()
            .await;

        // 4. Synthesize artist/album rows (deduped) and relink tracks.
        let mut new_artists: Vec<db::Media> = Vec::new();
        let mut new_albums: Vec<db::Media> = Vec::new();
        let mut track_updates: Vec<db::Media> = Vec::with_capacity(derived.len());
        // Genre entities (deduped by stable id) + track→genre relations, so
        // `Genres`/`GenreItems` populate like a stock Jellyfin library.
        let mut new_genres: HashMap<Uuid, db::Media> = HashMap::new();
        let mut genre_relations: Vec<db::MediaRelation> = Vec::new();
        let data_dir = &ctx
            .config
            .data_dir;

        for d in &derived {
            let norm_artist = normalize(&d.artist_name);
            let norm_album = normalize(&d.album_name);

            let artist_id = *artist_by_name
                .entry(norm_artist.clone())
                .or_insert_with(|| {
                    let id = common::stable_media_uuid(
                        &db::MediaKind::Artist,
                        &format!("local:{norm_artist}"),
                    );
                    new_artists.push(db::Media {
                        id,
                        title: d
                            .artist_name
                            .trim()
                            .to_string(),
                        kind: db::MediaKind::Artist,
                        external_ids: db::ExternalIds {
                            custom_stremio_id: Some(format!(
                                "localartist:{norm_artist}"
                            )),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                    id
                });

            let album_id = *album_by_key
                .entry((artist_id, norm_album.clone()))
                .or_insert_with(|| {
                    let id = common::stable_media_uuid(
                        &db::MediaKind::Album,
                        &format!("local:{norm_artist}:{norm_album}"),
                    );
                    let mut album = db::Media {
                        id,
                        title: d
                            .album_name
                            .trim()
                            .to_string(),
                        kind: db::MediaKind::Album,
                        grandparent_id: Some(artist_id),
                        released_at: d
                            .year
                            .and_then(year_to_date),
                        external_ids: db::ExternalIds {
                            custom_stremio_id: Some(format!(
                                "localalbum:{norm_artist}:{norm_album}"
                            )),
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    // Adopt a folder cover so album+track artwork appears in
                    // clients (served directly from disk); tracks inherit it via
                    // AlbumPrimaryImageTag.
                    if let Some(cover) = find_folder_cover(&d.dir)
                        .or_else(|| extract_embedded_cover(&d.path, data_dir, id))
                    {
                        album.set_image(db::ImageKind::Primary, cover);
                    }
                    new_albums.push(album);
                    id
                });

            track_updates.push(db::Media {
                id: d.track_id,
                title: d
                    .title
                    .clone(),
                kind: db::MediaKind::Track,
                parent_id: Some(album_id),
                grandparent_id: Some(artist_id),
                // `idx` is the track's IndexNumber (track number); `parent_idx`
                // is ParentIndexNumber (disc). The track number belongs in `idx`.
                idx: d.track_no,
                parent_idx: d.disc_no,
                released_at: d
                    .year
                    .and_then(year_to_date),
                // Real duration + probed source shape so local tracks carry
                // RunTimeTicks / Container / Bitrate / Size / MediaStreams like
                // a stock Jellyfin library track.
                runtime: d.runtime_secs,
                probe_data: d
                    .probe_data
                    .clone(),
                external_ids: db::ExternalIds {
                    // Preserve the opendal marker so Track validation passes and
                    // the source keeps resolving.
                    custom_stremio_id: Some(format!("opendal:{}", d.track_id)),
                    ..Default::default()
                },
                ..Default::default()
            });

            // Track genres: reuse the shared builder so ids/kind stay identical
            // to every other genre source, then dedupe entities by id.
            for (rel, genre) in db::build_genre_relations_from_names(
                d.track_id,
                &d.genres,
                db::MediaKind::MusicGenre,
            ) {
                new_genres
                    .entry(genre.id)
                    .or_insert(genre);
                genre_relations.push(rel);
            }
        }

        info!(
            "GroupLocalMusic: {} new artists, {} new albums, {} tracks relinked, {} genres, {} genre links",
            new_artists.len(),
            new_albums.len(),
            track_updates.len(),
            new_genres.len(),
            genre_relations.len(),
        );

        // 5. Persist parents before children (parent_id/grandparent_id FKs).
        db::Media::upsert(&ctx.db, &new_artists).await?;
        db::Media::upsert(&ctx.db, &new_albums).await?;
        // Genre entities before their relations (right_media_id FK).
        let genre_rows: Vec<db::Media> = new_genres
            .into_values()
            .collect();
        db::Media::upsert(&ctx.db, &genre_rows).await?;
        for (i, chunk) in track_updates
            .chunks(500)
            .enumerate()
        {
            db::Media::upsert(&ctx.db, chunk).await?;
            progress.report((i + 1) * 500, total.max(1));
        }
        db::MediaRelation::upsert(&ctx.db, &genre_relations).await?;
        progress.set(100.0);
        info!("GroupLocalMusic: complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_case_and_whitespace() {
        assert_eq!(normalize("  9TAILS   Archive "), "9tails archive");
        assert_eq!(normalize("Daft Punk"), "daft punk");
    }

    #[test]
    fn clean_album_name_strips_year_and_tag_suffixes() {
        assert_eq!(clean_album_name("Ænima (1996)"), "Ænima");
        assert_eq!(
            clean_album_name("Donda (Deluxe) (2021) [MP3 320]"),
            "Donda (Deluxe)"
        );
        assert_eq!(clean_album_name("Nostalgy [FLAC-WEB-DEEZER]"), "Nostalgy");
        // A parenthetical that is not a 4-digit year is preserved.
        assert_eq!(
            clean_album_name("Live @ Rex Club (Paris)"),
            "Live @ Rex Club (Paris)"
        );
    }

    #[test]
    fn parse_genres_splits_dedupes_and_preserves_commas() {
        let mut tags = HashMap::new();
        tags.insert(
            "genre".to_string(),
            "Drill / Chicago Drill; Drill \\ Gangsta Rap".to_string(),
        );
        // Split on ; / | \, case-insensitive dedupe ("Drill" once), first casing kept.
        assert_eq!(
            parse_genres(&tags),
            vec!["Drill", "Chicago Drill", "Gangsta Rap"]
        );

        // A comma is part of the genre name, not a delimiter.
        tags.insert("genre".to_string(), "Folk, World, & Country".to_string());
        assert_eq!(parse_genres(&tags), vec!["Folk, World, & Country"]);

        // Missing / empty tag → no genres.
        assert!(parse_genres(&HashMap::new()).is_empty());
        tags.insert("genre".to_string(), String::new());
        assert!(parse_genres(&tags).is_empty());
    }

    #[test]
    fn parse_track_no_prefers_tag_then_fallback() {
        assert_eq!(parse_track_no(Some(&"3/12".to_string()), None), Some(3));
        assert_eq!(parse_track_no(Some(&"07".to_string()), Some(1)), Some(7));
        assert_eq!(parse_track_no(None, Some(5)), Some(5));
        assert_eq!(parse_track_no(Some(&"".to_string()), None), None);
        assert_eq!(parse_track_no(Some(&"0".to_string()), None), None);
    }

    #[test]
    fn derive_from_path_handles_artist_album_and_loose_archives() {
        // A representative library root; real roots come from the addon config.
        let roots = vec!["/mnt/music".to_string()];
        // <root>/<artist>/<album (year)>/<track>
        assert_eq!(
            derive_from_path("/mnt/music/Tool/Ænima (1996)/09 - jimmy.mp3", &roots),
            ("Tool".to_string(), "Ænima".to_string())
        );
        // "Digital Media NN" disc folders are skipped.
        assert_eq!(
            derive_from_path(
                "/mnt/music/Future/FUTURE (2017)/Digital Media 02/14 - Flip.flac",
                &roots
            ),
            ("Future".to_string(), "FUTURE".to_string())
        );
        // Loose file directly under the root → artist == album == its folder.
        assert_eq!(
            derive_from_path("/mnt/music/Some Archive/1.mp3", &roots),
            ("Some Archive".to_string(), "Some Archive".to_string())
        );
    }

    #[test]
    fn derive_prefers_tags_over_path() {
        let roots = vec!["/mnt/music".to_string()];
        let mut tags = HashMap::new();
        tags.insert("album_artist".to_string(), "Some Artist".to_string());
        tags.insert("album".to_string(), "Second Album".to_string());
        tags.insert("title".to_string(), "Night Time".to_string());
        tags.insert("track".to_string(), "4".to_string());
        let d = derive(
            Uuid::nil(),
            "/mnt/music/Some Artist/Second Album/04 night time.mp3",
            None,
            None,
            &tags,
            &roots,
        );
        assert_eq!(d.artist_name, "Some Artist");
        assert_eq!(d.album_name, "Second Album");
        assert_eq!(d.title, "Night Time");
        assert_eq!(d.track_no, Some(4));
    }

    #[test]
    fn derive_falls_back_to_path_and_filename_when_untagged() {
        let roots = vec!["/mnt/music".to_string()];
        let tags = HashMap::new();
        let d = derive(
            Uuid::nil(),
            "/mnt/music/Some Archive/loose track title.mp3",
            Some(2),
            None,
            &tags,
            &roots,
        );
        assert_eq!(d.artist_name, "Some Archive");
        assert_eq!(d.album_name, "Some Archive");
        assert_eq!(d.title, "loose track title");
        assert_eq!(d.track_no, Some(2));
    }
}
