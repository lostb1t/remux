use anyhow::anyhow;
use axum::{
    body::Body,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use http::{Response, StatusCode};
use remux_macros::get;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{AppState, IntoApiError, OptionExt, ResultExt, api, db, db::auth};

fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into())
}

/// Tracks in-progress batch subtitle extractions. Subtitle endpoint waits on these
/// instead of launching a competing on-demand FFmpeg process.
type SubtitleExtractionKey = (Uuid, Uuid);

static BATCH_EXTRACTING: OnceLock<
    Mutex<HashMap<SubtitleExtractionKey, watch::Receiver<bool>>>,
> = OnceLock::new();

fn batch_extraction_map()
-> &'static Mutex<HashMap<SubtitleExtractionKey, watch::Receiver<bool>>> {
    BATCH_EXTRACTING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn subtitle_cache_source_id(
    media: &db::Media,
    probed_size: Option<i64>,
) -> Uuid {
    let Some(stream_info) = media
        .stream_info
        .as_ref()
    else {
        return media.id;
    };
    let normalized_filename = stream_info
        .filename
        .as_deref()
        .map(|filename| {
            filename
                .replace('\\', "/")
                .to_ascii_lowercase()
        });
    let size = probed_size.or(stream_info.size);
    let identity = match &stream_info.descriptor {
        crate::stream::StreamDescriptor::Torrent {
            info_hash,
            file_hint,
            file_idx,
            ..
        } => Some(format!(
            "torrent:{}:{}:{}",
            info_hash.to_ascii_lowercase(),
            file_idx.map_or_else(String::new, |index| index.to_string()),
            normalized_filename
                .as_deref()
                .or(file_hint.as_deref())
                .unwrap_or_default()
        )),
        crate::stream::StreamDescriptor::Http { .. } => normalized_filename
            .zip(size)
            .map(|(filename, size)| format!("http:{filename}:{size}")),
        crate::stream::StreamDescriptor::Local(path) => {
            Some(format!("local:{}", path.to_string_lossy()))
        }
        crate::stream::StreamDescriptor::Opendal { addon_id, path } => {
            Some(format!("opendal:{addon_id}:{path}"))
        }
        crate::stream::StreamDescriptor::Rtsp { .. } => None,
    };

    identity
        .map(|identity| Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()))
        .unwrap_or(media.id)
}

fn is_valid_ass_document(bytes: &[u8]) -> bool {
    let content = String::from_utf8_lossy(bytes);
    content.contains("[Script Info]")
        && content.contains("[Events]")
        && content
            .lines()
            .any(|line| {
                line.trim_start()
                    .starts_with("Dialogue:")
            })
}

#[cfg(test)]
mod tests {
    use super::{is_valid_ass_document, subtitle_cache_source_id};
    use crate::{db, stream};
    use uuid::Uuid;

    fn http_stream_media(id: Uuid, url: &str) -> db::Media {
        db::Media {
            id,
            stream_info: Some(stream::StreamInfo {
                descriptor: stream::StreamDescriptor::http(url),
                filename: Some("S01E03-Kingdom of Lies.mkv".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn ass_validation_requires_dialogue_events() {
        assert!(!is_valid_ass_document(
            b"[Script Info]\n[Events]\nFormat: Layer, Start, End, Text\n"
        ));
        assert!(is_valid_ass_document(
            b"[Script Info]\n[Events]\nDialogue: 0,0:00:00.00,0:00:01.00,Hello\n"
        ));
    }

    #[test]
    fn stable_cache_identity_ignores_refreshed_http_url() {
        let first = http_stream_media(Uuid::new_v4(), "https://debrid/first-token");
        let refreshed =
            http_stream_media(Uuid::new_v4(), "https://debrid/second-token");

        assert_eq!(
            subtitle_cache_source_id(&first, Some(6_302_221_349)),
            subtitle_cache_source_id(&refreshed, Some(6_302_221_349))
        );
        assert_ne!(
            subtitle_cache_source_id(&first, Some(6_302_221_349)),
            subtitle_cache_source_id(&refreshed, Some(1_450_000_000))
        );
    }
}

/// Extract an embedded subtitle stream to the SRT cache and return the cache path.
/// The cache key is
/// `{data_dir}/subtitle-cache/{item_id}_{cache_source_id}_{stream_index}.srt`.
/// Returns immediately if the cache already exists and is non-empty.
pub(crate) async fn extract_subtitle_to_cache(
    data_dir: &std::path::Path,
    input_url: &str,
    map_spec: &str,
    item_id: uuid::Uuid,
    cache_source_id: uuid::Uuid,
    stream_index: i64,
) -> anyhow::Result<std::path::PathBuf> {
    let cache_dir = data_dir.join("subtitle-cache");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| anyhow!("failed to create subtitle cache dir: {e}"))?;
    let cache_path =
        cache_dir.join(format!("{item_id}_{cache_source_id}_{stream_index}.srt"));

    // Return cached copy if it exists and is non-empty.
    if cache_path.exists() {
        let bytes = tokio::fs::read(&cache_path)
            .await
            .unwrap_or_default();
        let content = String::from_utf8_lossy(&bytes);
        if !content
            .trim()
            .is_empty()
        {
            return Ok(cache_path);
        }
    }

    let mut cmd = tokio::process::Command::new(ffmpeg_bin());
    cmd.kill_on_drop(true);
    cmd.args([
        "-y",
        "-nostdin",
        "-copyts",
        "-i",
        input_url,
        "-map",
        map_spec,
        "-an",
        "-vn",
        "-c:s",
        "srt",
        "-f",
        "srt",
        cache_path
            .to_str()
            .ok_or_else(|| anyhow!("invalid cache path"))?,
    ]);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());

    let output =
        tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output())
            .await
            .map_err(|_| {
                let p = cache_path.clone();
                tokio::spawn(async move {
                    let _ = tokio::fs::remove_file(p).await;
                });
                anyhow!("subtitle extraction timed out")
            })?
            .map_err(|e| anyhow!("failed to run ffmpeg: {e}"))?;

    if !output
        .status
        .success()
    {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffmpeg subtitle extraction failed: {stderr}");
    }

    let bytes = tokio::fs::read(&cache_path)
        .await
        .map_err(|e| anyhow!("failed to read cached subtitle: {e}"))?;
    if bytes
        .iter()
        .all(|b| b.is_ascii_whitespace())
    {
        let _ = tokio::fs::remove_file(&cache_path).await;
        anyhow::bail!("subtitle extraction produced empty output");
    }

    Ok(cache_path)
}

/// Pre-extract all embedded text subtitle streams for a media source in one FFmpeg pass.
/// ASS/SSA streams also retain a raw styled copy for renderer-capable clients.
/// Mirrors Jellyfin's approach: one command, multiple outputs, fire-and-forget at PlaybackInfo time.
/// The `subtitles_stream` endpoint falls back to on-demand extraction for any cache misses.
pub(crate) async fn pre_extract_all_subtitles_to_cache(
    data_dir: std::path::PathBuf,
    input_url: String,
    item_id: uuid::Uuid,
    cache_source_id: uuid::Uuid,
    subtitle_streams: Vec<(i64, bool)>,
) {
    let cache_dir = data_dir.join("subtitle-cache");
    let _ = tokio::fs::create_dir_all(&cache_dir).await;

    let cache_is_populated = |path: &std::path::Path| {
        std::fs::read(path)
            .ok()
            .map(|bytes| {
                if path
                    .to_string_lossy()
                    .contains(".ass")
                {
                    is_valid_ass_document(&bytes)
                } else {
                    !bytes.is_empty()
                        && !String::from_utf8_lossy(&bytes)
                            .trim()
                            .is_empty()
                }
            })
            .unwrap_or(false)
    };
    let mut to_extract = Vec::new();
    for (idx, preserve_ass) in &subtitle_streams {
        let srt_path = cache_dir.join(format!("{item_id}_{cache_source_id}_{idx}.srt"));
        let raw_ass_path =
            cache_dir.join(format!("{item_id}_{cache_source_id}_{idx}.ass"));
        let needs_srt = !cache_is_populated(&srt_path);
        let needs_raw_ass = *preserve_ass && !cache_is_populated(&raw_ass_path);
        if needs_srt || needs_raw_ass {
            let raw_ass_temp_path = needs_raw_ass.then(|| {
                cache_dir.join(format!(
                    "{item_id}_{cache_source_id}_{idx}_{}.ass.batch.tmp",
                    Uuid::new_v4()
                ))
            });
            to_extract.push((
                *idx,
                needs_srt.then_some(srt_path),
                raw_ass_temp_path,
                raw_ass_path,
            ));
        } else {
            debug!(%item_id, stream_index = idx, "subtitle caches hit, skipping");
        }
    }

    if to_extract.is_empty() {
        debug!(%item_id, "all {} subtitle track(s) already cached", subtitle_streams.len());
        return;
    }

    let indices: Vec<i64> = to_extract
        .iter()
        .map(|(i, _, _, _)| *i)
        .collect();
    info!(
        %item_id,
        %cache_source_id,
        ?indices,
        "pre-extracting {} subtitle track(s) in background",
        to_extract.len()
    );

    // Register in-progress signal so the subtitle endpoint can wait on us
    // instead of launching a competing FFmpeg process.
    let extraction_key = (item_id, cache_source_id);
    let (done_tx, done_rx) = watch::channel(false);
    {
        let mut extracting = batch_extraction_map()
            .lock()
            .unwrap();
        if extracting.contains_key(&extraction_key) {
            debug!(%item_id, %cache_source_id, "subtitle extraction already in progress, reusing existing work");
            return;
        }
        extracting.insert(extraction_key, done_rx);
    }

    let mut cmd = tokio::process::Command::new(ffmpeg_bin());
    cmd.kill_on_drop(true);
    // -y: overwrite without prompting (hangs forever waiting for stdin otherwise)
    // -nostdin: don't read from stdin at all
    // -c:s srt: convert to SRT so the cache is always valid SRT (not raw ASS/VTT bytes)
    cmd.args(["-y", "-nostdin", "-i", &input_url]);
    for (idx, srt_path, raw_ass_temp_path, _) in &to_extract {
        if let Some(p) = srt_path
            .as_ref()
            .and_then(|path| path.to_str())
        {
            cmd.args([
                "-map",
                &format!("0:{idx}"),
                "-an",
                "-vn",
                "-c:s",
                "srt",
                "-flush_packets",
                "1",
                p,
            ]);
        }
        if let Some(p) = raw_ass_temp_path
            .as_ref()
            .and_then(|path| path.to_str())
        {
            cmd.args([
                "-map",
                &format!("0:{idx}"),
                "-an",
                "-vn",
                "-c:s",
                "copy",
                "-f",
                "ass",
                p,
            ]);
        }
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());

    let start = std::time::Instant::now();
    match tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output()).await
    {
        Ok(Ok(output)) => {
            let elapsed = start
                .elapsed()
                .as_secs_f32();
            if output
                .status
                .success()
            {
                for (_, _, raw_ass_temp_path, raw_ass_path) in &to_extract {
                    if let Some(temp_path) = raw_ass_temp_path {
                        if cache_is_populated(temp_path) {
                            if tokio::fs::rename(temp_path, raw_ass_path)
                                .await
                                .is_err()
                            {
                                let _ = tokio::fs::remove_file(temp_path).await;
                            }
                        } else {
                            let _ = tokio::fs::remove_file(temp_path).await;
                        }
                    }
                }
                info!(%item_id, %cache_source_id, ?indices, elapsed_secs = elapsed, "batch subtitle extraction completed");
            } else {
                for (_, _, raw_ass_temp_path, _) in &to_extract {
                    if let Some(temp_path) = raw_ass_temp_path {
                        let _ = tokio::fs::remove_file(temp_path).await;
                    }
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!(%item_id, %cache_source_id, ?indices, elapsed_secs = elapsed, %stderr, "batch subtitle extraction non-zero exit");
            }
        }
        Ok(Err(e)) => {
            for (_, _, raw_ass_temp_path, _) in &to_extract {
                if let Some(temp_path) = raw_ass_temp_path {
                    let _ = tokio::fs::remove_file(temp_path).await;
                }
            }
            warn!(%item_id, %cache_source_id, ?indices, "failed to spawn ffmpeg for batch subtitle extraction: {e}");
        }
        Err(_) => {
            for (_, _, raw_ass_temp_path, _) in &to_extract {
                if let Some(temp_path) = raw_ass_temp_path {
                    let _ = tokio::fs::remove_file(temp_path).await;
                }
            }
            warn!(%item_id, %cache_source_id, ?indices, "batch subtitle extraction timed out after 120s");
        }
    }

    // Signal done and clean up (drop tx signals all receivers).
    let _ = done_tx.send(true);
    batch_extraction_map()
        .lock()
        .unwrap()
        .remove(&extraction_key);
}

/// Subtitle extraction endpoint - extracts a subtitle stream from a media source
/// and optionally converts it to the requested format (vtt, srt, ass).
// Jellyfin clients include a start-position-ticks segment in the path.
#[get(
    "/videos/{item_id}/{media_source_id}/subtitles/{stream_index}/{start_ticks}/stream.{format}"
)]
pub async fn subtitles_stream(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((item_id, media_source_id, stream_index, _start_ticks, format)): Path<(
        Uuid,
        Uuid,
        i64,
        String,
        String,
    )>,
) -> Result<impl IntoResponse> {
    // Try to resolve as an external subtitle injected during PlaybackInfo.
    // fetch_subtitles is cached (24h Stremio / SQLite Opendal) so this is cheap.
    if let Some(item_media) = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await
    .ok()
    .flatten()
    {
        let source_media = crate::services::StreamService::lookup(
            &state.ctx,
            item_id,
            Some(media_source_id),
            None,
        )
        .await
        .ok();
        if let Some(ref source) = source_media {
            let embedded_indices: std::collections::HashSet<i64> = source
                .probe_data
                .as_ref()
                .map(|p| {
                    p.media_streams
                        .iter()
                        .map(|s| s.index)
                        .collect()
                })
                .unwrap_or_default();
            let next_idx = embedded_indices
                .iter()
                .max()
                .map_or(0, |m| m + 1);
            let i = stream_index - next_idx;
            // Only attempt external resolution if the index is not an embedded stream.
            if i >= 0 && !embedded_indices.contains(&stream_index) {
                let sub_langs = db::Settings::get_config_or_default(
                    &state
                        .ctx
                        .db,
                )
                .await
                .subtitle_languages
                .unwrap_or_default();
                let subs = state
                    .ctx
                    .addons
                    .fetch_subtitles(
                        &item_media,
                        &state
                            .ctx
                            .db,
                        false,
                    )
                    .await;
                let source_info = api::MediaSourceInfo::from(source.clone());
                let scored = scored_external_subtitles(
                    &subs,
                    &sub_langs,
                    &source_info.name,
                    &source_info.path,
                );
                if let Some(sub) = scored.get(i as usize) {
                    if let Some(ref descriptor) = sub.url {
                        let output_format = format.to_ascii_lowercase();
                        let resp = match descriptor {
                            crate::stream::StreamDescriptor::Opendal {
                                addon_id,
                                ..
                            } => {
                                let addon = state
                                    .ctx
                                    .addons
                                    .get(*addon_id)
                                    .ok_or_else(|| {
                                        anyhow!("addon not found for subtitle")
                                    })?;
                                let stream_cap = addon
                                    .stream
                                    .as_ref()
                                    .ok_or_else(|| {
                                        anyhow!("addon has no stream capability")
                                    })?;
                                stream_cap
                                    .serve_stream(
                                        descriptor,
                                        &axum::http::HeaderMap::new(),
                                    )
                                    .await
                                    .map_err(|e| anyhow!("{e:?}"))?
                            }
                            _ => descriptor
                                .clone()
                                .into_source()
                                .serve(&state, &axum::http::HeaderMap::new())
                                .await
                                .map_err(|e| anyhow!("{e:?}"))?,
                        };
                        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                            .await
                            .map_err(|e| anyhow!("read subtitle bytes: {e}"))?;
                        let body = String::from_utf8_lossy(&bytes).into_owned();
                        let (converted, content_type) = match output_format.as_str() {
                            "vtt" | "webvtt" => (
                                crate::conversions::srt_to_vtt(&body),
                                "text/vtt; charset=utf-8",
                            ),
                            "js" => (
                                crate::conversions::srt_to_jellyfin_json(&body),
                                "application/json",
                            ),
                            _ => (body, "text/plain; charset=utf-8"),
                        };
                        return Ok(Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", content_type)
                            .header("Cache-Control", "public, max-age=3600")
                            .header("Access-Control-Allow-Origin", "*")
                            .body(Body::from(converted))
                            .unwrap());
                    }
                }
            }
        }
    }

    let media = crate::services::StreamService::lookup(
        &state.ctx,
        item_id,
        Some(media_source_id),
        None,
    )
    .await?;
    let cache_source_id = subtitle_cache_source_id(
        &media,
        media
            .probe_data
            .as_ref()
            .and_then(|probe| probe.size),
    );

    let url = media
        .stream_info
        .as_ref()
        .map(|si| {
            si.descriptor
                .server_input(
                    media.id,
                    state
                        .ctx
                        .config
                        .port,
                )
        })
        .context_not_found("media source has no URL")?;

    let output_format = format.to_ascii_lowercase();
    let is_json = matches!(output_format.as_str(), "js" | "json");
    let (ffmpeg_format, content_type) = match output_format.as_str() {
        "vtt" | "webvtt" => ("webvtt", "text/vtt; charset=utf-8"),
        "srt" | "subrip" => ("srt", "text/plain; charset=utf-8"),
        "ass" | "ssa" => ("ass", "text/plain; charset=utf-8"),
        "pgssub" | "sup" => ("sup", "application/octet-stream"),
        "js" | "json" => ("srt", "application/json; charset=utf-8"),
        _ => ("srt", "text/plain; charset=utf-8"),
    };

    let subtitle_ordinal = media
        .probe_data
        .as_ref()
        .and_then(|probe| {
            let mut sub_indexes: Vec<i64> = probe
                .media_streams
                .iter()
                .filter(|s| matches!(s.type_, Some(api::MediaStreamType::Subtitle)))
                .map(|s| s.index)
                .collect();
            sub_indexes.sort_unstable();
            sub_indexes
                .iter()
                .position(|idx| *idx == stream_index)
        })
        .context_not_found("subtitle stream not found")?;
    let map_spec = format!("0:s:{subtitle_ordinal}");

    let is_passthrough =
        matches!(output_format.as_str(), "ass" | "ssa" | "sup" | "pgssub");
    let is_binary = matches!(output_format.as_str(), "sup" | "pgssub");

    // ASS/SSA must bypass the SRT cache. Converting to SRT destroys styles,
    // drawings, transforms, and karaoke effects that client renderers need.
    if matches!(output_format.as_str(), "ass" | "ssa") {
        let cache_dir = state
            .ctx
            .config
            .data_dir
            .join("subtitle-cache");
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .map_err(|e| anyhow!("failed to create subtitle cache dir: {e}"))?;
        let cache_path =
            cache_dir.join(format!("{item_id}_{cache_source_id}_{stream_index}.ass"));

        let mut cached = tokio::fs::read(&cache_path)
            .await
            .ok()
            .filter(|bytes| is_valid_ass_document(bytes));
        if cached.is_none() {
            let in_progress_rx = batch_extraction_map()
                .lock()
                .unwrap()
                .get(&(item_id, cache_source_id))
                .cloned();
            if let Some(mut rx) = in_progress_rx {
                if !*rx.borrow() {
                    info!(%item_id, %media_source_id, stream_index, "batch ASS extraction in progress - waiting for it to finish");
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(120),
                        rx.changed(),
                    )
                    .await;
                    cached = tokio::fs::read(&cache_path)
                        .await
                        .ok()
                        .filter(|bytes| is_valid_ass_document(bytes));
                }
            }
        }
        let bytes = if let Some(bytes) = cached {
            debug!(%item_id, %media_source_id, stream_index, "raw ASS subtitle cache hit");
            bytes
        } else {
            let temp_path = cache_dir.join(format!(
                "{item_id}_{cache_source_id}_{stream_index}_{}.ass.tmp",
                Uuid::new_v4()
            ));
            let mut cmd = tokio::process::Command::new(ffmpeg_bin());
            cmd.kill_on_drop(true);
            cmd.args([
                "-y",
                "-nostdin",
                "-copyts",
                "-i",
                &url,
                "-map",
                &map_spec,
                "-an",
                "-vn",
                "-c:s",
                "copy",
                "-f",
                "ass",
                temp_path
                    .to_str()
                    .ok_or_else(|| anyhow!("invalid ASS cache path"))?,
            ]);
            cmd.stdin(std::process::Stdio::null());
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::piped());
            let output =
                tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output())
                    .await
                    .map_err(|_| anyhow!("ASS subtitle extraction timed out"))?
                    .map_err(|e| anyhow!("failed to run ffmpeg: {e}"))?;
            if !output
                .status
                .success()
            {
                let _ = tokio::fs::remove_file(&temp_path).await;
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!("ASS subtitle extraction failed: {stderr}").into());
            }
            let bytes = tokio::fs::read(&temp_path)
                .await
                .map_err(|e| anyhow!("failed to read ASS subtitle: {e}"))?;
            if !is_valid_ass_document(&bytes) {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(anyhow!(
                    "ASS subtitle extraction produced no dialogue events"
                )
                .into());
            }
            if tokio::fs::rename(&temp_path, &cache_path)
                .await
                .is_err()
            {
                let _ = tokio::fs::remove_file(&temp_path).await;
            }
            bytes
        };

        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Cache-Control", "public, max-age=3600")
            .header("Access-Control-Allow-Origin", "*")
            .body(Body::from(bytes))
            .unwrap());
    }

    // Binary formats (PGS/SUP): extract on-the-fly as raw bytes.
    if is_binary {
        let mut cmd = tokio::process::Command::new(ffmpeg_bin());
        cmd.kill_on_drop(true);
        cmd.args([
            "-copyts",
            "-i",
            &url,
            "-map",
            &map_spec,
            "-an",
            "-vn",
            "-c:s",
            "copy",
            "-f",
            output_format.as_str(),
            "-",
        ]);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output())
                .await
                .map_err(|_| anyhow!("subtitle extraction timed out"))?
                .map_err(|e| anyhow!("failed to run ffmpeg: {e}"))?;
        if !output
            .status
            .success()
        {
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("subtitle extraction failed"))
                .unwrap());
        }
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .body(Body::from(output.stdout))
            .unwrap());
    }

    // Text formats: serve from SRT cache (populated by pre_extract_all_subtitles_to_cache
    // at PlaybackInfo time). Falls back to on-demand extraction on cache miss.
    let cache_file = state
        .ctx
        .config
        .data_dir
        .join("subtitle-cache")
        .join(format!("{item_id}_{cache_source_id}_{stream_index}.srt"));
    let is_cached = |path: &std::path::Path| -> bool {
        path.exists()
            && std::fs::read(path)
                .ok()
                .map(|b| {
                    !String::from_utf8_lossy(&b)
                        .trim()
                        .is_empty()
                })
                .unwrap_or(false)
    };

    if is_cached(&cache_file) {
        debug!(%item_id, stream_index, "subtitle cache hit");
    } else {
        // Check if a batch extraction is in progress for this item.
        // If so, wait for it to finish rather than launching a competing FFmpeg process.
        let in_progress_rx = batch_extraction_map()
            .lock()
            .unwrap()
            .get(&(item_id, cache_source_id))
            .cloned();
        if let Some(mut rx) = in_progress_rx {
            if !*rx.borrow() {
                info!(%item_id, stream_index, "batch extraction in progress — waiting for it to finish");
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(120),
                    rx.changed(),
                )
                .await;
            }
        }

        if is_cached(&cache_file) {
            info!(%item_id, stream_index, "subtitle ready after waiting for batch extraction");
        } else {
            info!(%item_id, stream_index, %map_spec, "subtitle cache miss — extracting on-demand");
        }
    }
    let cache_path = match extract_subtitle_to_cache(
        &state
            .ctx
            .config
            .data_dir,
        &url,
        &map_spec,
        item_id,
        cache_source_id,
        stream_index,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            error!(%item_id, stream_index, %map_spec, "subtitle extraction failed: {e}");
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("subtitle extraction failed"))
                .unwrap());
        }
    };

    let cached = String::from_utf8_lossy(
        &tokio::fs::read(&cache_path)
            .await
            .map_err(|e| anyhow!("failed to read cached subtitle: {e}"))?,
    )
    .into_owned();

    let body = if is_passthrough {
        cached
    } else if is_json {
        crate::conversions::srt_to_jellyfin_json(&cached)
    } else if ffmpeg_format == "webvtt" {
        crate::conversions::srt_to_vtt(&cached)
    } else {
        cached
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "public, max-age=3600")
        .header("Access-Control-Allow-Origin", "*")
        .body(Body::from(body))
        .unwrap())
}

pub(crate) fn lang_to_two_letter(lang: &str) -> Option<String> {
    use std::str::FromStr;
    let lang = lang
        .trim()
        .to_lowercase();
    if lang.is_empty() {
        return None;
    }
    if lang.len() == 2 {
        return Some(lang);
    }
    isolang::Language::from_639_3(&lang)
        .or_else(|| isolang::Language::from_str(&lang).ok())
        .and_then(|l| l.to_639_1())
        .map(|s| s.to_string())
}

pub(crate) fn subtitle_path_hint(sub: &crate::addons::SubtitleInfo) -> &str {
    match &sub.url {
        Some(crate::stream::StreamDescriptor::Http { url, .. }) => url.as_str(),
        Some(crate::stream::StreamDescriptor::Local(p)) => p
            .to_str()
            .unwrap_or(""),
        Some(crate::stream::StreamDescriptor::Opendal { path, .. }) => path.as_str(),
        _ => "",
    }
}

pub(crate) fn descriptor_to_subtitle_url(sub: &crate::addons::SubtitleInfo) -> String {
    match &sub.url {
        Some(d) => serde_json::to_string(d).unwrap_or_default(),
        None => String::new(),
    }
}

fn score_sub_url(
    sub: &crate::addons::SubtitleInfo,
    source_name: &Option<String>,
    source_path: &Option<String>,
) -> i32 {
    fn tokens(s: &str) -> std::collections::HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 2)
            .map(|t| t.to_lowercase())
            .collect()
    }
    let hint = subtitle_path_hint(sub);
    let sub_file = hint
        .rsplit('/')
        .next()
        .unwrap_or(hint);
    let sub_tok = tokens(sub_file);
    let mut src_tok = tokens(
        source_name
            .as_deref()
            .unwrap_or(""),
    );
    src_tok.extend(tokens(
        source_path
            .as_deref()
            .unwrap_or(""),
    ));
    sub_tok
        .intersection(&src_tok)
        .count() as i32
}

/// Filter, score, sort, and deduplicate external subtitles for a single source.
/// Returns the ordered list of subtitles that will be assigned stream indices.
pub(crate) fn scored_external_subtitles<'a>(
    subs: &'a [crate::addons::SubtitleInfo],
    sub_langs: &[String],
    source_name: &Option<String>,
    source_path: &Option<String>,
) -> Vec<&'a crate::addons::SubtitleInfo> {
    let filtered: Vec<&crate::addons::SubtitleInfo> = if sub_langs.is_empty() {
        subs.iter()
            .collect()
    } else {
        subs.iter()
            .filter(|s| {
                let two = s
                    .lang
                    .as_deref()
                    .and_then(lang_to_two_letter);
                two.map_or(false, |two| {
                    sub_langs
                        .iter()
                        .any(|p| two.eq_ignore_ascii_case(p.trim()))
                })
            })
            .collect()
    };

    let mut scored: Vec<_> = filtered
        .into_iter()
        .map(|s| (score_sub_url(s, source_name, source_path), s))
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        let rank = |s: &&crate::addons::SubtitleInfo| {
            let two = s
                .lang
                .as_deref()
                .and_then(lang_to_two_letter);
            sub_langs
                .iter()
                .position(|p| {
                    two.as_deref()
                        .map_or(false, |t| t.eq_ignore_ascii_case(p.trim()))
                })
                .unwrap_or(usize::MAX)
        };
        rank(a)
            .cmp(&rank(b))
            .then(sb.cmp(sa))
    });

    let mut lang_counts: std::collections::HashMap<String, usize> = Default::default();
    scored
        .into_iter()
        .filter_map(|(_, s)| {
            let key = s
                .lang
                .clone()
                .unwrap_or_else(|| "und".to_string());
            let count = lang_counts
                .entry(key)
                .or_insert(0);
            if *count < 2 {
                *count += 1;
                Some(s)
            } else {
                None
            }
        })
        .collect()
}

/// Inject external subtitles into a list of `MediaSourceInfo` entries.
pub(crate) async fn inject_external_subtitles(
    ctx: &crate::AppContext,
    subtitle_media: &crate::db::Media,
    media_sources: &mut Vec<api::MediaSourceInfo>,
    item_id: Uuid,
    api_key: &str,
    sub_langs: Vec<String>,
) {
    let subs = ctx
        .addons
        .fetch_subtitles(subtitle_media, &ctx.db, false)
        .await;
    if subs.is_empty() {
        return;
    }

    for source in media_sources.iter_mut() {
        let next_idx = source
            .media_streams
            .iter()
            .map(|s| s.index)
            .max()
            .map_or(0, |m| m + 1);

        let scored =
            scored_external_subtitles(&subs, &sub_langs, &source.name, &source.path);

        let wants_default = !sub_langs.is_empty()
            && source
                .default_subtitle_stream_index
                .is_none();
        for (i, sub) in scored
            .into_iter()
            .enumerate()
        {
            let mut stream = crate::conversions::subtitle_to_media_stream(sub);
            let idx = next_idx + i as i64;
            stream.index = idx;
            stream.delivery_url = Some(format!(
                "/Videos/{item_id}/{source_id}/Subtitles/{idx}/0/Stream.vtt?ApiKey={api_key}",
                source_id = source.id,
            ));
            if wants_default && i == 0 {
                stream.is_default = Some(true);
                source.default_subtitle_stream_index = Some(next_idx);
            }
            source
                .media_streams
                .push(stream);
        }
    }
}

#[cfg(test)]
mod language_code_tests {
    use super::*;

    #[test]
    fn lang_to_two_letter_normalizes_codes() {
        // Already two letters: kept as-is, just trimmed and lowercased.
        assert_eq!(lang_to_two_letter("en"), Some("en".to_string()));
        assert_eq!(lang_to_two_letter("  EN "), Some("en".to_string()));
        // Three-letter ISO 639-3 codes are mapped down to two letters.
        assert_eq!(lang_to_two_letter("eng"), Some("en".to_string()));
        assert_eq!(lang_to_two_letter("spa"), Some("es".to_string()));
        // Empty or unrecognizable input gives nothing back.
        assert_eq!(lang_to_two_letter(""), None);
        assert_eq!(lang_to_two_letter("   "), None);
        assert_eq!(lang_to_two_letter("xyz"), None);
    }
}
