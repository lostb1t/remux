use anyhow::anyhow;
use axum::{
    body::Body,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use http::{Response, StatusCode};
use remux_macros::get;
use tracing::error;
use uuid::Uuid;

use crate::{AppState, IntoApiError, OptionExt, ResultExt, api, db, db::auth};

fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into())
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
        let source_media = {
            let sm = db::Media::get_by_id(
                &state
                    .ctx
                    .db,
                &media_source_id,
            )
            .await
            .ok()
            .flatten();
            if let Some(mut m) = sm {
                if matches!(
                    m.kind,
                    db::MediaKind::Movie
                        | db::MediaKind::Episode
                        | db::MediaKind::Track
                ) {
                    m.streams(
                        &state
                            .ctx
                            .db,
                    )
                    .await
                    .ok()
                    .and_then(|v| {
                        v.into_iter()
                            .next()
                    })
                } else {
                    Some(m)
                }
            } else {
                None
            }
        };
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

    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &media_source_id,
    )
    .await?
    .context_not_found("media source not found")?;

    if matches!(
        media.kind,
        db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
    ) {
        media = media
            .streams(
                &state
                    .ctx
                    .db,
            )
            .await?
            .get(0)
            .context_not_found("no sources found")?
            .clone();
    }

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
        .unwrap_or_else(|| format!("0:{stream_index}"));

    let is_passthrough =
        matches!(output_format.as_str(), "ass" | "ssa" | "sup" | "pgssub");
    let is_binary = matches!(output_format.as_str(), "sup" | "pgssub");

    // Binary formats (PGS/SUP): extract on-the-fly as raw bytes.
    if is_binary {
        let mut cmd = tokio::process::Command::new(ffmpeg_bin());
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
        let output = cmd
            .output()
            .await
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

    // Text formats: cache SRT, convert on-the-fly.
    let (ext_codec, ext_fmt, ext) = if is_passthrough {
        ("copy", "ass", "ass")
    } else {
        ("srt", "srt", "srt")
    };

    let cache_dir = state
        .ctx
        .config
        .data_dir
        .join("subtitle-cache");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| anyhow!("failed to create subtitle cache dir: {e}"))?;
    let cache_path = cache_dir.join(format!("{media_source_id}_{stream_index}.{ext}"));

    let mut cached = if cache_path.exists() {
        tokio::fs::read(&cache_path)
            .await
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .filter(|s| {
                !s.trim()
                    .is_empty()
            })
            .unwrap_or_default()
    } else {
        String::new()
    };

    if cached.is_empty() {
        let mut cmd = tokio::process::Command::new(ffmpeg_bin());
        cmd.args([
            "-copyts",
            "-i",
            &url,
            "-map",
            &map_spec,
            "-an",
            "-vn",
            "-c:s",
            ext_codec,
            "-f",
            ext_fmt,
            cache_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid cache path"))?,
        ]);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());
        let output = cmd
            .output()
            .await
            .map_err(|e| anyhow!("failed to run ffmpeg: {e}"))?;
        if !output
            .status
            .success()
        {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(%media_source_id, stream_index, %map_spec, "ffmpeg subtitle extraction failed: {stderr}");
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("subtitle extraction failed"))
                .unwrap());
        }
        cached = String::from_utf8_lossy(
            &tokio::fs::read(&cache_path)
                .await
                .map_err(|e| anyhow!("failed to read cached subtitle: {e}"))?,
        )
        .into_owned();
        if cached
            .trim()
            .is_empty()
        {
            let _ = tokio::fs::remove_file(&cache_path).await;
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("subtitle extraction failed"))
                .unwrap());
        }
    }

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
