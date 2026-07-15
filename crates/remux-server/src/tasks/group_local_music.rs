use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{AppContext, common, db};

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
            "format_tags=artist,album_artist,albumartist,album,title,track",
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
    Derived {
        track_id,
        title,
        artist_name,
        album_name,
        track_no,
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
        let base = "SELECT f.id, f.path, f.track_number, f.title \
                    FROM opendal_files f JOIN media m ON m.id = f.id \
                    WHERE f.media_kind = 'track' AND m.kind = 'track' AND m.parent_id IS NULL";
        let sql = match limit {
            Some(n) => format!("{base} LIMIT {n}"),
            None => base.to_string(),
        };
        let rows: Vec<(Uuid, String, Option<i64>, Option<String>)> =
            sqlx::query_as(&sql)
                .fetch_all(&ctx.db)
                .await?;
        let total = rows.len();
        info!("GroupLocalMusic: {total} orphaned local tracks to group");
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
            "SELECT id, title, grandparent_id FROM media WHERE kind = 'album'",
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
                    let tags = tokio::task::spawn_blocking(move || ffprobe_tags(&p))
                        .await
                        .unwrap_or_default();
                    derive(id, &path, track_no, opendal_title, &tags, &roots)
                }
            })
            .buffer_unordered(8)
            .collect()
            .await;

        // 4. Synthesize artist/album rows (deduped) and relink tracks.
        let mut new_artists: Vec<db::Media> = Vec::new();
        let mut new_albums: Vec<db::Media> = Vec::new();
        let mut track_updates: Vec<db::Media> = Vec::with_capacity(derived.len());

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
                    new_albums.push(db::Media {
                        id,
                        title: d
                            .album_name
                            .trim()
                            .to_string(),
                        kind: db::MediaKind::Album,
                        grandparent_id: Some(artist_id),
                        external_ids: db::ExternalIds {
                            custom_stremio_id: Some(format!(
                                "localalbum:{norm_artist}:{norm_album}"
                            )),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
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
                parent_idx: d.track_no,
                external_ids: db::ExternalIds {
                    // Preserve the opendal marker so Track validation passes and
                    // the source keeps resolving.
                    custom_stremio_id: Some(format!("opendal:{}", d.track_id)),
                    ..Default::default()
                },
                ..Default::default()
            });
        }

        info!(
            "GroupLocalMusic: {} new artists, {} new albums, {} tracks relinked",
            new_artists.len(),
            new_albums.len(),
            track_updates.len()
        );

        // 5. Persist parents before children (parent_id/grandparent_id FKs).
        db::Media::upsert(&ctx.db, &new_artists).await?;
        db::Media::upsert(&ctx.db, &new_albums).await?;
        for (i, chunk) in track_updates
            .chunks(500)
            .enumerate()
        {
            db::Media::upsert(&ctx.db, chunk).await?;
            progress.report((i + 1) * 500, total.max(1));
        }
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
