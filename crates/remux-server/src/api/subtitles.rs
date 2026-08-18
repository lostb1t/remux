use anyhow::anyhow;
use axum::{
    body::Body,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use http::{Response, StatusCode};
use remux_macros::get;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt, api, common::HideConsole, db,
    db::auth,
};

fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into())
}

/// The cache storage codec for a requested text subtitle format: ASS/SSA requests
/// get a native ASS cache (styled dialogue preserved), everything else the SRT
/// cache that VTT/JSON conversions are built on.
fn subtitle_cache_codec(output_format: &str) -> api::SubtitleCodec {
    if matches!(
        output_format.parse::<api::SubtitleCodec>(),
        Ok(api::SubtitleCodec::Ass)
    ) {
        api::SubtitleCodec::Ass
    } else {
        api::SubtitleCodec::Srt
    }
}

/// ffmpeg `-c:s` for the cache: stream-copy native ASS/SSA when the cache wants
/// ASS, otherwise re-encode to the cache codec.
fn subtitle_cache_ffmpeg_codec(
    cache_codec: &api::SubtitleCodec,
    source_codec: Option<&str>,
) -> String {
    if *cache_codec == api::SubtitleCodec::Ass
        && matches!(
            source_codec.and_then(|c| c
                .parse::<api::SubtitleCodec>()
                .ok()),
            Some(api::SubtitleCodec::Ass)
        )
    {
        "copy".to_string()
    } else {
        cache_codec.to_string()
    }
}

fn subtitle_cache_path(
    data_dir: &std::path::Path,
    item_id: Uuid,
    stream_index: i64,
    cache_codec: &api::SubtitleCodec,
) -> std::path::PathBuf {
    data_dir
        .join("subtitle-cache")
        .join(format!(
            "{item_id}_{stream_index}.{}",
            cache_codec.to_string()
        ))
}

/// Extract an embedded text subtitle stream to the requested cache format.
/// The cache key is `{data_dir}/subtitle-cache/{item_id}_{stream_index}.{format}`.
/// Returns immediately if the cache already exists and is non-empty.
async fn extract_subtitle_to_cache(
    data_dir: &std::path::Path,
    input_url: &str,
    map_spec: &str,
    item_id: uuid::Uuid,
    stream_index: i64,
    cache_codec: api::SubtitleCodec,
    source_codec: Option<&str>,
) -> anyhow::Result<std::path::PathBuf> {
    let cache_dir = data_dir.join("subtitle-cache");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| anyhow!("failed to create subtitle cache dir: {e}"))?;
    let cache_path = subtitle_cache_path(data_dir, item_id, stream_index, &cache_codec);

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

    let ffmpeg_codec = subtitle_cache_ffmpeg_codec(&cache_codec, source_codec);
    let ffmpeg_format = cache_codec.to_string();
    let mut cmd = tokio::process::Command::new(ffmpeg_bin());
    cmd.hide_console();
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
        &ffmpeg_codec,
        "-f",
        &ffmpeg_format,
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

/// Subtitle extraction endpoint - extracts a subtitle stream from a media source
/// and optionally converts it to the requested format (vtt, srt, ass).
// Jellyfin clients include a start-position-ticks segment in the path.
#[get(
    "/videos/{item_id}/{media_source_id}/subtitles/{stream_index}/{start_ticks}/stream.{format}"
)]
pub async fn subtitles_stream(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((item_id, media_source_id, stream_index, _start_ticks, format)): Path<(
        Uuid,
        Uuid,
        i64,
        String,
        String,
    )>,
) -> Result<impl IntoResponse> {
    subtitles_stream_inner(
        state,
        session,
        item_id,
        media_source_id,
        stream_index,
        format,
    )
    .await
}

/// Jellyfin also accepts the tickless subtitle route (defaults the start-position
/// ticks segment to 0) — Moonfin for webOS uses it.
/// https://github.com/jellyfin/jellyfin/blob/master/Jellyfin.Api/Controllers/SubtitleController.cs
#[get("/videos/{item_id}/{media_source_id}/subtitles/{stream_index}/stream.{format}")]
pub async fn subtitles_stream_tickless(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((item_id, media_source_id, stream_index, format)): Path<(
        Uuid,
        Uuid,
        i64,
        String,
    )>,
) -> Result<impl IntoResponse> {
    subtitles_stream_inner(
        state,
        session,
        item_id,
        media_source_id,
        stream_index,
        format,
    )
    .await
}

/// Fetch the raw bytes of an external subtitle URL through our stream proxy.
/// A refused/corrupt upstream response (flaky subtitle CDN) is treated as
/// "subtitle unavailable", not a server error: the caller logs it and answers
/// 404 to the client instead of aborting the request with a 500.
async fn fetch_external_subtitle_bytes(
    state: &AppState,
    descriptor: &crate::stream::StreamDescriptor,
) -> anyhow::Result<axum::body::Bytes> {
    let resp = match descriptor {
        crate::stream::StreamDescriptor::Opendal { addon_id, .. } => {
            let addon = state
                .ctx
                .addons
                .get(*addon_id)
                .ok_or_else(|| anyhow!("addon not found for subtitle"))?;
            let stream_cap = addon
                .stream
                .as_ref()
                .ok_or_else(|| anyhow!("addon has no stream capability"))?;
            stream_cap
                .serve_stream(descriptor, &axum::http::HeaderMap::new())
                .await
                .map_err(|e| anyhow!("upstream serve failed: {e:?}"))?
        }
        _ => descriptor
            .clone()
            .into_source()
            .serve(state, &axum::http::HeaderMap::new())
            .await
            .map_err(|e| anyhow!("upstream serve failed: {e:?}"))?,
    };
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .map_err(|e| anyhow!("read subtitle bytes: {e}"))
}

async fn subtitles_stream_inner(
    state: AppState,
    session: auth::AuthSession,
    item_id: Uuid,
    media_source_id: Uuid,
    stream_index: i64,
    format: String,
) -> Result<impl IntoResponse> {
    // Try to resolve as an external subtitle injected during PlaybackInfo.
    // fetch_subtitles is cached (24h Stremio / SQLite Opendal) so this is cheap.
    if let Some(mut item_media) = db::Media::get_by_id(
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
            Some(
                session
                    .user
                    .id,
            ),
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
                        &mut item_media,
                        &state
                            .ctx
                            .db,
                        true,
                        Some(
                            session
                                .user
                                .id,
                        ),
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
                        match fetch_external_subtitle_bytes(&state, descriptor).await {
                            Ok(bytes) => {
                                let body = String::from_utf8_lossy(&bytes).into_owned();
                                let (converted, content_type) = match output_format
                                    .as_str()
                                {
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
                            Err(e) => {
                                warn!(error = %e, item_id = %item_id, stream_index,
                                    "external subtitle unavailable");
                                return Ok((
                                    StatusCode::NOT_FOUND,
                                    "subtitle unavailable",
                                )
                                    .into_response());
                            }
                        }
                    }
                }
            }
        }
    }

    let Ok(media) = crate::services::StreamService::lookup(
        &state.ctx,
        item_id,
        Some(media_source_id),
        None,
        Some(
            session
                .user
                .id,
        ),
    )
    .await
    else {
        return Ok((StatusCode::NOT_FOUND, "stream not found").into_response());
    };

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

    let map_spec = media
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
                .map(|ordinal| format!("0:s:{}", ordinal))
        })
        .context_not_found("subtitle stream not found")?;

    let source_codec = media
        .probe_data
        .as_ref()
        .and_then(|probe| {
            probe
                .media_streams
                .iter()
                .find(|stream| {
                    stream.index == stream_index
                        && matches!(stream.type_, Some(api::MediaStreamType::Subtitle))
                })
        })
        .and_then(|stream| {
            stream
                .codec
                .as_deref()
        });

    let is_binary = matches!(output_format.as_str(), "sup" | "pgssub");

    // Binary formats (PGS/SUP): extract on-the-fly as raw bytes.
    if is_binary {
        let mut cmd = tokio::process::Command::new(ffmpeg_bin());
        cmd.hide_console();
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

    // VTT/SRT/JSON requests use the SRT cache populated at PlaybackInfo time.
    // ASS/SSA requests use a separate native ASS cache so styled subtitle data is
    // never replaced by SRT bytes under an .ass URL.
    let cache_codec = subtitle_cache_codec(&output_format);
    let cache_file = subtitle_cache_path(
        &state
            .ctx
            .config
            .data_dir,
        item_id,
        stream_index,
        &cache_codec,
    );
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
        info!(%item_id, stream_index, %map_spec, "subtitle cache miss — extracting on-demand");
    }
    let cache_path = match extract_subtitle_to_cache(
        &state
            .ctx
            .config
            .data_dir,
        &url,
        &map_spec,
        item_id,
        stream_index,
        cache_codec,
        source_codec,
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

    let body = if is_json {
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

pub(crate) use remux_sdks::remux::lang_to_two_letter;

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
    subtitle_media: &mut crate::db::Media,
    media_sources: &mut Vec<api::MediaSourceInfo>,
    item_id: Uuid,
    api_key: &str,
    sub_langs: Vec<String>,
    user_id: Option<uuid::Uuid>,
) {
    let subs = ctx
        .addons
        .fetch_subtitles(subtitle_media, &ctx.db, false, user_id)
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
mod tests {
    use super::*;
    use http::header::HeaderValue;

    use crate::integration_test::{auth_header_with_token, authenticated_server};

    /// Jellyfin's tickless subtitle route (`.../Subtitles/{index}/Stream.{format}`,
    /// no start-position-ticks segment) must dispatch to the same handler as the
    /// canonical route. With a non-existent item both produce the identical
    /// handler response — an unregistered route would yield axum's bare 404.
    #[tokio::test]
    async fn tickless_subtitle_route_dispatches_to_handler() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let bogus = "00000000-0000-0000-0000-000000000000";
        let _ = &guard;

        let canonical = server
            .get(&format!("/videos/{bogus}/{bogus}/subtitles/2/0/stream.ass"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .expect_failure()
            .await;
        let tickless = server
            .get(&format!("/videos/{bogus}/{bogus}/subtitles/2/stream.ass"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .expect_failure()
            .await;

        assert_eq!(
            canonical.status_code(),
            tickless.status_code(),
            "both subtitle route forms must reach the same handler"
        );
        assert!(
            !tickless
                .text()
                .is_empty(),
            "tickless route must dispatch to the subtitle handler, not a bare route-miss 404"
        );
    }

    #[test]
    fn ass_requests_use_a_native_cache_separate_from_srt() {
        let data_dir = std::path::Path::new("/data");
        let item_id = Uuid::nil();

        let srt = subtitle_cache_path(data_dir, item_id, 2, &api::SubtitleCodec::Srt);
        let ass = subtitle_cache_path(data_dir, item_id, 2, &api::SubtitleCodec::Ass);

        assert_eq!(
            srt,
            data_dir
                .join("subtitle-cache")
                .join(format!("{item_id}_2.srt"))
        );
        assert_eq!(
            ass,
            data_dir
                .join("subtitle-cache")
                .join(format!("{item_id}_2.ass"))
        );
        assert_ne!(srt, ass);
    }

    #[test]
    fn native_ass_extraction_preserves_the_original_stream() {
        let cache = subtitle_cache_codec("ass");

        assert_eq!(cache, api::SubtitleCodec::Ass);
        assert_eq!(subtitle_cache_ffmpeg_codec(&cache, Some("ass")), "copy");
        assert_eq!(subtitle_cache_ffmpeg_codec(&cache, Some("SSA")), "copy");
        assert_eq!(cache.to_string(), "ass");
    }

    #[test]
    fn non_ass_source_is_converted_when_ass_is_requested() {
        let cache = subtitle_cache_codec("ssa");

        assert_eq!(cache, api::SubtitleCodec::Ass);
        assert_eq!(subtitle_cache_ffmpeg_codec(&cache, Some("subrip")), "ass");
    }
}
