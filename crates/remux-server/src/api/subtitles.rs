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
    db::auth, playback::engine::ffmpeg_reconnect_args,
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

/// One subtitle output to pull out of a batch extraction — everything
/// `extract_subtitles_to_cache` needs to place it at its own cache path.
/// A single stream can produce more than one of these (see
/// `extractable_subtitles`: a native-ASS stream gets both an ASS and an SRT
/// output), so `stream_index`/`map_spec` are not unique across the list.
#[derive(Debug)]
struct ExtractableSubtitle {
    stream_index: i64,
    map_spec: String,
    cache_codec: api::SubtitleCodec,
    ffmpeg_codec: String,
}

/// Process-wide single-flight lock, keyed by (item_id, media_source_id).
/// A source is only ever read once concurrently — every subtitle stream on
/// it is extracted together in one ffmpeg pass (see
/// `extract_subtitles_to_cache`), so the lock only needs to be per-source,
/// not per-stream. Entries are never evicted: one small `(Uuid, Uuid)` key
/// plus an empty mutex per source ever requested is negligible over a
/// process lifetime.
static SUBTITLE_EXTRACTION_LOCKS: std::sync::LazyLock<
    tokio::sync::Mutex<
        std::collections::HashMap<(Uuid, Uuid), std::sync::Arc<tokio::sync::Mutex<()>>>,
    >,
> = std::sync::LazyLock::new(|| {
    tokio::sync::Mutex::new(std::collections::HashMap::new())
});

async fn subtitle_extraction_lock(
    item_id: Uuid,
    media_source_id: Uuid,
) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    SUBTITLE_EXTRACTION_LOCKS
        .lock()
        .await
        .entry((item_id, media_source_id))
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Builds the list of every extractable (embedded, non-external, text)
/// subtitle stream on `probe`, in container order, with the ffmpeg `-map`
/// spec and cache codec(s) each one needs. Image/bitmap subtitles (PGS,
/// DVD/DVB sub) are never included — ffmpeg can't convert them to a text
/// codec, and one such stream in the same batch as text streams would fail
/// the whole ffmpeg invocation (they're served on-the-fly instead, see the
/// `is_binary` branch in `subtitles_stream_inner`).
///
/// Mirrors Jellyfin's own rule: a native ASS/SSA stream stays ASS
/// (stream-copied) — decided per stream from its *own* codec, not whatever
/// format a client happens to be asking for right now, since this batch also
/// caches every other subtitle track on the source for later requests. A
/// native-ASS stream additionally gets a plain SRT cache alongside its ASS
/// one: VTT/SRT/JSON requests are always served from the SRT cache (see
/// `subtitle_cache_codec`), and without it those requests would 404 against
/// a stream that only ever produced a `.ass` file.
fn extractable_subtitles(probe: &api::MediaSourceInfo) -> Vec<ExtractableSubtitle> {
    let mut indexes: Vec<i64> = probe
        .media_streams
        .iter()
        .filter(|s| {
            matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                && !s.is_external
                && s.is_text_subtitle_stream()
        })
        .map(|s| s.index)
        .collect();
    indexes.sort_unstable();
    indexes
        .iter()
        .enumerate()
        .filter_map(|(ordinal, &stream_index)| {
            let stream = probe
                .media_streams
                .iter()
                .find(|s| s.index == stream_index)?;
            let source_codec = stream
                .codec
                .as_deref();
            let map_spec = format!("0:s:{ordinal}");
            let cache_codec = subtitle_cache_codec(source_codec.unwrap_or(""));
            let ffmpeg_codec = subtitle_cache_ffmpeg_codec(&cache_codec, source_codec);
            let mut outputs = vec![ExtractableSubtitle {
                stream_index,
                map_spec: map_spec.clone(),
                cache_codec: cache_codec.clone(),
                ffmpeg_codec,
            }];
            if cache_codec == api::SubtitleCodec::Ass {
                outputs.push(ExtractableSubtitle {
                    stream_index,
                    map_spec,
                    cache_codec: api::SubtitleCodec::Srt,
                    ffmpeg_codec: subtitle_cache_ffmpeg_codec(
                        &api::SubtitleCodec::Srt,
                        source_codec,
                    ),
                });
            }
            Some(outputs)
        })
        .flatten()
        .collect()
}

/// The path ffmpeg actually writes to for a given final cache path — never
/// the final path itself. `extract_subtitles_to_cache` only renames a temp
/// file into place after ffmpeg has fully closed it, so a reader can never
/// observe a partially-written file at the path it checks for a cache hit
/// (see `ensure_subtitle_cached`'s pre-lock fast path).
fn subtitle_cache_tmp_path(final_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = final_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    final_path.with_file_name(name)
}

/// Builds the ffmpeg argument list (minus the binary name) for extracting
/// every stream in `streams` from `input_url` in one pass, plus the
/// (tmp_path, final_path) pair each one is written to. ffmpeg writes to
/// `tmp_path`; the caller renames to `final_path` only once extraction has
/// fully succeeded. Pure and side-effect free so it's unit-testable without
/// actually running ffmpeg — mirrors how `build_hls_args` in
/// `playback::engine` is tested.
fn build_subtitle_extraction_args(
    data_dir: &std::path::Path,
    input_url: &str,
    item_id: Uuid,
    streams: &[ExtractableSubtitle],
) -> anyhow::Result<(Vec<String>, Vec<(std::path::PathBuf, std::path::PathBuf)>)> {
    let mut args = vec![
        "-y".to_string(),
        "-nostdin".to_string(),
        "-copyts".to_string(),
    ];
    args.extend(
        ffmpeg_reconnect_args(input_url)
            .iter()
            .map(|s| s.to_string()),
    );
    args.push("-i".to_string());
    args.push(input_url.to_string());
    let mut cache_paths = Vec::with_capacity(streams.len());
    for s in streams {
        let final_path =
            subtitle_cache_path(data_dir, item_id, s.stream_index, &s.cache_codec);
        let tmp_path = subtitle_cache_tmp_path(&final_path);
        args.extend([
            "-map".to_string(),
            s.map_spec
                .clone(),
            "-an".to_string(),
            "-vn".to_string(),
            "-c:s".to_string(),
            s.ffmpeg_codec
                .clone(),
            "-f".to_string(),
            s.cache_codec
                .to_string(),
        ]);
        args.push(
            tmp_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid cache path"))?
                .to_string(),
        );
        cache_paths.push((tmp_path, final_path));
    }
    Ok((args, cache_paths))
}

/// Extracts every subtitle in `streams` from `input_url` in a single ffmpeg
/// pass — one `-i`, one `-map`/`-c:s` output per stream — so a remote
/// source is read once for its subtitles no matter how many tracks it has,
/// not once per requested track. On timeout or failure every output in this
/// batch is deleted, matching Jellyfin's own behavior (confirmed against
/// its `SubtitleEncoder.cs`: it doesn't keep partial output either).
///
/// ffmpeg writes to a `.tmp` path and each output is only renamed into its
/// real cache path after the whole batch succeeds — a concurrent reader
/// checking the real path (outside the extraction lock, see
/// `ensure_subtitle_cached`) can therefore never observe a partially-written
/// file.
async fn extract_subtitles_to_cache(
    data_dir: &std::path::Path,
    input_url: &str,
    item_id: Uuid,
    streams: &[ExtractableSubtitle],
    timeout_seconds: i64,
) -> anyhow::Result<()> {
    if streams.is_empty() {
        return Ok(());
    }
    let cache_dir = data_dir.join("subtitle-cache");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| anyhow!("failed to create subtitle cache dir: {e}"))?;

    let (args, cache_paths) =
        build_subtitle_extraction_args(data_dir, input_url, item_id, streams)?;
    let mut cmd = tokio::process::Command::new(ffmpeg_bin());
    cmd.hide_console();
    cmd.kill_on_drop(true);
    cmd.args(&args);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());

    let delete_tmp_outputs = |paths: Vec<(std::path::PathBuf, std::path::PathBuf)>| {
        tokio::spawn(async move {
            for (tmp, _final) in paths {
                let _ = tokio::fs::remove_file(tmp).await;
            }
        });
    };

    let timeout_secs = timeout_seconds.max(1) as u64;
    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        cmd.output(),
    )
    .await
    {
        Err(_) => {
            delete_tmp_outputs(cache_paths);
            anyhow::bail!("subtitle extraction timed out");
        }
        Ok(Err(e)) => anyhow::bail!("failed to run ffmpeg: {e}"),
        Ok(Ok(output)) => output,
    };

    if !output
        .status
        .success()
    {
        let stderr = String::from_utf8_lossy(&output.stderr);
        delete_tmp_outputs(cache_paths);
        anyhow::bail!("ffmpeg subtitle extraction failed: {stderr}");
    }

    // A stream ffmpeg couldn't actually produce (e.g. an empty track) leaves
    // an empty or missing tmp file — drop just that one rather than failing
    // the whole batch, so the other tracks it successfully extracted still
    // cache. Everything else is only now moved into its real cache path.
    for (tmp, final_path) in &cache_paths {
        let empty = tokio::fs::read(tmp)
            .await
            .map(|b| {
                b.iter()
                    .all(|b| b.is_ascii_whitespace())
            })
            .unwrap_or(true);
        if empty {
            let _ = tokio::fs::remove_file(tmp).await;
        } else if let Err(e) = tokio::fs::rename(tmp, final_path).await {
            warn!(error = %e, tmp = %tmp.display(), "failed to move extracted subtitle into cache");
            let _ = tokio::fs::remove_file(tmp).await;
        }
    }

    Ok(())
}

/// Ensures `stream_index`'s subtitle is cached, batch-extracting every
/// not-yet-cached extractable stream on the source in one pass if needed
/// (see `extract_subtitles_to_cache`). Single-flight per source: concurrent
/// or player-retried requests for different tracks on the same source wait
/// on the one running extraction instead of each starting their own full
/// read of a possibly-remote file.
async fn ensure_subtitle_cached(
    data_dir: &std::path::Path,
    input_url: &str,
    probe: &api::MediaSourceInfo,
    item_id: Uuid,
    media_source_id: Uuid,
    stream_index: i64,
    requested_cache_codec: api::SubtitleCodec,
    timeout_seconds: i64,
) -> anyhow::Result<std::path::PathBuf> {
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
    let requested_path =
        subtitle_cache_path(data_dir, item_id, stream_index, &requested_cache_codec);
    if is_cached(&requested_path) {
        return Ok(requested_path);
    }

    let lock = subtitle_extraction_lock(item_id, media_source_id).await;
    let _guard = lock
        .lock()
        .await;

    // Re-check now that we hold the lock: a concurrent request for a
    // different track on this source may have just extracted this one too.
    if is_cached(&requested_path) {
        return Ok(requested_path);
    }

    let missing: Vec<ExtractableSubtitle> = extractable_subtitles(probe)
        .into_iter()
        .filter(|s| {
            !is_cached(&subtitle_cache_path(
                data_dir,
                item_id,
                s.stream_index,
                &s.cache_codec,
            ))
        })
        .collect();
    extract_subtitles_to_cache(data_dir, input_url, item_id, &missing, timeout_seconds)
        .await?;

    if !is_cached(&requested_path) {
        anyhow::bail!(
            "subtitle extraction produced no output for stream {stream_index}"
        );
    }
    Ok(requested_path)
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
    if !resp
        .status()
        .is_success()
    {
        return Err(anyhow!("upstream subtitle status {}", resp.status()));
    }
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .map_err(|e| anyhow!("read subtitle bytes: {e}"))
}

fn external_subtitle_response(
    bytes: axum::body::Bytes,
    output_format: &str,
) -> Response<Body> {
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let (converted, content_type) = match output_format {
        "vtt" | "webvtt" => (
            crate::conversions::srt_to_vtt(&body),
            "text/vtt; charset=utf-8",
        ),
        "js" | "json" => (
            crate::conversions::srt_to_jellyfin_json(&body),
            "application/json; charset=utf-8",
        ),
        _ => (body, "text/plain; charset=utf-8"),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "public, max-age=3600")
        .header("Access-Control-Allow-Origin", "*")
        .body(Body::from(converted))
        .unwrap()
}

#[derive(Clone)]
pub(crate) struct SidecarSubtitleRoute {
    index: i64,
    subtitle: crate::addons::SubtitleInfo,
}

fn sidecar_subtitle_routes_key(
    device_id: &str,
    item_id: Uuid,
    media_source_id: Uuid,
) -> String {
    format!("sidecar-subtitle-routes:{device_id}:{item_id}:{media_source_id}")
}

fn next_external_subtitle_index(
    source_indices: impl IntoIterator<Item = i64>,
    sidecar_indices: impl IntoIterator<Item = i64>,
) -> i64 {
    source_indices
        .into_iter()
        .chain(sidecar_indices)
        .max()
        .map_or(0, |index| index + 1)
}

pub(crate) fn inject_sidecar_subtitles(
    source: &mut api::MediaSourceInfo,
    subtitles: Vec<crate::addons::SubtitleInfo>,
) -> Vec<SidecarSubtitleRoute> {
    let next_idx = source
        .media_streams
        .iter()
        .map(|stream| stream.index)
        .max()
        .map_or(0, |index| index + 1);

    subtitles
        .into_iter()
        .enumerate()
        .map(|(offset, subtitle)| {
            let index = next_idx + offset as i64;
            let mut stream = crate::conversions::subtitle_to_media_stream(&subtitle);
            stream.index = index;
            source
                .media_streams
                .push(stream);
            SidecarSubtitleRoute { index, subtitle }
        })
        .collect()
}

pub(crate) fn save_sidecar_subtitle_routes(
    ctx: &crate::AppContext,
    device_id: &str,
    item_id: Uuid,
    media_source_id: Uuid,
    routes: Vec<SidecarSubtitleRoute>,
) {
    if routes.is_empty() {
        return;
    }
    ctx.store
        .save(
            sidecar_subtitle_routes_key(device_id, item_id, media_source_id),
            routes,
            std::time::Duration::from_secs(6 * 60 * 60),
        );
}

fn load_sidecar_subtitle_routes(
    ctx: &crate::AppContext,
    device_id: &str,
    item_id: Uuid,
    media_source_id: Uuid,
) -> Option<std::sync::Arc<Vec<SidecarSubtitleRoute>>> {
    ctx.store
        .get::<Vec<SidecarSubtitleRoute>>(&sidecar_subtitle_routes_key(
            device_id,
            item_id,
            media_source_id,
        ))
}

async fn sidecar_subtitle_response(
    state: &AppState,
    routes: Option<&[SidecarSubtitleRoute]>,
    item_id: Uuid,
    media_source_id: Uuid,
    stream_index: i64,
    format: &str,
) -> Option<Response<Body>> {
    let route = routes?
        .iter()
        .find(|route| route.index == stream_index)?;
    let descriptor = route
        .subtitle
        .url
        .as_ref()?;

    Some(
        match fetch_external_subtitle_bytes(state, descriptor).await {
            Ok(bytes) => {
                external_subtitle_response(bytes, &format.to_ascii_lowercase())
            }
            Err(error) => {
                warn!(%error, %item_id, %media_source_id, stream_index,
                "sidecar subtitle unavailable");
                (StatusCode::NOT_FOUND, "subtitle unavailable").into_response()
            }
        },
    )
}

async fn subtitles_stream_inner(
    state: AppState,
    session: auth::AuthSession,
    item_id: Uuid,
    media_source_id: Uuid,
    stream_index: i64,
    format: String,
) -> Result<impl IntoResponse> {
    let sidecar_routes = load_sidecar_subtitle_routes(
        &state.ctx,
        &session
            .device
            .id,
        item_id,
        media_source_id,
    );
    if let Some(response) = sidecar_subtitle_response(
        &state,
        sidecar_routes
            .as_ref()
            .map(|routes| routes.as_slice()),
        item_id,
        media_source_id,
        stream_index,
        &format,
    )
    .await
    {
        return Ok(response);
    }

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
            // Sidecars are inserted before add-on subtitles in PlaybackInfo,
            // but are not present in the source's probe data. Include their
            // advertised indexes so the requested add-on matches the selected
            // menu entry.
            let next_idx = next_external_subtitle_index(
                embedded_indices
                    .iter()
                    .copied(),
                sidecar_routes
                    .as_deref()
                    .into_iter()
                    .flatten()
                    .map(|entry| entry.index),
            );
            let i = stream_index - next_idx;
            // Only attempt external resolution if the index is not an embedded stream.
            if i >= 0 && !embedded_indices.contains(&stream_index) {
                let server_cfg = db::Settings::get_config_or_default(
                    &state
                        .ctx
                        .db,
                )
                .await;
                let sub_langs = server_cfg
                    .subtitle_languages
                    .clone()
                    .unwrap_or_default();
                let dedup = SubtitleDedupSettings::from_config(&server_cfg);
                let subs = state
                    .ctx
                    .addons
                    .fetch_subtitles(
                        &mut item_media,
                        &state.ctx,
                        true,
                        Some(
                            session
                                .user
                                .id,
                        ),
                    )
                    .await;
                // Match append_external_subtitles' filtering exactly, or the
                // index a client requests (from the menu it was shown) won't
                // line up with this reconstructed list. That means resolving
                // delivery_method the same way PlaybackInfo did first: raw
                // probe data leaves text subtitles' delivery_method unset
                // (None), which would bypass has_supported_embedded_subtitle's
                // Embed-only guard and let a stale/best-effort profile
                // reach its codec fallback instead of agreeing with what was
                // actually advertised.
                let device_profile =
                    crate::jellyfin_client::merge_device_profile_subtitles(
                        &session.device,
                        session
                            .device
                            .parsed_device_profile(),
                    );
                let encoding_cfg = db::Settings::get_encoding_config(
                    &state
                        .ctx
                        .db,
                )
                .await
                .unwrap_or_default();
                let subtitle_mode = encoding_cfg
                    .subtitle_mode
                    .unwrap_or_default();
                let allow_subtitle_extraction = source
                    .stream_info
                    .as_ref()
                    .is_some_and(|si| {
                        si.descriptor
                            .is_local()
                    })
                    || encoding_cfg
                        .allow_remote_subtitle_extraction
                        .unwrap_or(false);
                let resolved_probe = source
                    .probe_data
                    .clone()
                    .map(|mut probe| {
                        for stream in &mut probe.media_streams {
                            if matches!(
                                stream.type_,
                                Some(api::MediaStreamType::Subtitle)
                            ) {
                                stream.is_text_subtitle_stream =
                                    stream.is_text_subtitle_stream();
                            }
                        }
                        crate::playback::decision::apply_subtitle_delivery(
                            &mut probe,
                            item_id,
                            session
                                .device
                                .access_token
                                .expose(),
                            &device_profile,
                            subtitle_mode,
                            allow_subtitle_extraction,
                        );
                        probe
                    });
                let scored: Vec<_> =
                    crate::subtitle_selection::select_external_subtitles(
                        &subs,
                        &sub_langs,
                        source
                            .stream_info
                            .as_ref()
                            .and_then(|info| {
                                info.filename
                                    .as_deref()
                            }),
                        dedup.max_external_per_language(),
                    )
                    .into_iter()
                    .filter(|sub| {
                        !dedup.enabled
                            || !resolved_probe
                                .as_ref()
                                .is_some_and(|probe| {
                                    has_supported_embedded_subtitle(
                                        probe,
                                        sub,
                                        device_profile.as_ref(),
                                    )
                                })
                    })
                    .collect();
                if let Some(sub) = scored.get(i as usize) {
                    if let Some(ref descriptor) = sub.url {
                        let output_format = format.to_ascii_lowercase();
                        match fetch_external_subtitle_bytes(&state, descriptor).await {
                            Ok(bytes) => {
                                return Ok(external_subtitle_response(
                                    bytes,
                                    &output_format,
                                ));
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

    // Defense in depth: a client can hit this endpoint directly with any
    // stream_index, regardless of what delivery_method PlaybackInfo actually
    // advertised (`apply_subtitle_delivery`'s upstream feasibility check).
    // Never start an extraction — reading the whole remote file once — for a
    // remote source unless the admin opted in; local files are unaffected.
    let is_local = media
        .stream_info
        .as_ref()
        .is_some_and(|si| {
            si.descriptor
                .is_local()
        });
    let encoding_cfg = db::Settings::get_encoding_config(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();
    let allow_subtitle_extraction = is_local
        || encoding_cfg
            .allow_remote_subtitle_extraction
            .unwrap_or(false);
    if !allow_subtitle_extraction {
        return Ok((
            StatusCode::NOT_FOUND,
            "subtitle extraction is disabled for remote sources",
        )
            .into_response());
    }

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

    let is_binary = matches!(output_format.as_str(), "sup" | "pgssub");

    // Binary formats (PGS/SUP): extract on-the-fly as raw bytes.
    if is_binary {
        let mut cmd = tokio::process::Command::new(ffmpeg_bin());
        cmd.hide_console();
        cmd.kill_on_drop(true);
        cmd.args(["-copyts"]);
        cmd.args(ffmpeg_reconnect_args(&url));
        cmd.args(["-i", &url]);
        cmd.args([
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
        let timeout_secs = encoding_cfg
            .subtitle_extraction_timeout_seconds
            .unwrap_or(1800)
            .max(1) as u64;
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            cmd.output(),
        )
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
    if cache_file.exists()
        && std::fs::read(&cache_file)
            .ok()
            .map(|b| {
                !String::from_utf8_lossy(&b)
                    .trim()
                    .is_empty()
            })
            .unwrap_or(false)
    {
        debug!(%item_id, stream_index, "subtitle cache hit");
    } else {
        info!(%item_id, stream_index, %map_spec, "subtitle cache miss — extracting on-demand");
    }
    let probe = media
        .probe_data
        .clone()
        .unwrap_or_default();
    let cache_path = match ensure_subtitle_cached(
        &state
            .ctx
            .config
            .data_dir,
        &url,
        &probe,
        item_id,
        media_source_id,
        stream_index,
        cache_codec,
        encoding_cfg
            .subtitle_extraction_timeout_seconds
            .unwrap_or(1800),
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
        Some(crate::stream::StreamDescriptor::Local(path)) => path
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

/// `ServerConfiguration.deduplicate_subtitle_tracks` /
/// `max_external_subtitles_per_language`, resolved once per request so
/// `append_external_subtitles` and the download endpoint's index
/// reconstruction always agree on what was advertised.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SubtitleDedupSettings {
    /// When true: at most one subtitle per language, and a language already
    /// covered by a supported embedded track gets no external entry at all.
    /// When false: up to `max_external_per_language_when_disabled` external
    /// candidates per language, regardless of embedded coverage.
    pub enabled: bool,
    pub max_external_per_language_when_disabled: i64,
}

impl SubtitleDedupSettings {
    pub(crate) fn from_config(cfg: &api::ServerConfiguration) -> Self {
        Self {
            enabled: cfg
                .deduplicate_subtitle_tracks
                .unwrap_or(true),
            max_external_per_language_when_disabled: cfg
                .max_external_subtitles_per_language
                .unwrap_or(1),
        }
    }

    fn max_external_per_language(&self) -> usize {
        if self.enabled {
            1
        } else {
            self.max_external_per_language_when_disabled
                .max(0) as usize
        }
    }
}

/// Add prefetched subtitles to real-probed sources. Only called from
/// PlaybackInfo — Items detail deliberately never shows subtitle tracks (see
/// the stripping step in `items.rs`), so index alignment only has to hold
/// within a single PlaybackInfo response and the download endpoint that
/// follows it.
pub(crate) fn append_external_subtitles(
    media_sources: &mut [api::MediaSourceInfo],
    subs: &[crate::addons::SubtitleInfo],
    sub_langs: &[String],
    device_profile: Option<&api::DeviceProfile>,
    item_id: Uuid,
    api_key: &str,
    dedup: SubtitleDedupSettings,
) {
    for source in media_sources.iter_mut() {
        if !has_real_probe_data(source) {
            continue;
        }
        let next_idx = source
            .media_streams
            .iter()
            .map(|s| s.index)
            .max()
            .map_or(0, |m| m + 1);

        let source_filename = source
            .remux
            .as_ref()
            .and_then(|remux| {
                remux
                    .provider_info
                    .as_ref()
            })
            .and_then(|info| info.get("filename"))
            .and_then(serde_json::Value::as_str);
        // With dedup on, a language already covered by a supported embedded
        // track is dropped entirely, not replaced with a different (e.g.
        // forced/HI) external variant of the same language — offering a
        // second track for a language the device can already play embedded
        // is exactly the redundant duplicate this filter exists to avoid.
        // With dedup off, every external candidate up to the configured cap
        // is kept regardless of embedded coverage — the user asked to see
        // everything.
        let scored: Vec<_> = crate::subtitle_selection::select_external_subtitles(
            &subs,
            sub_langs,
            source_filename,
            dedup.max_external_per_language(),
        )
        .into_iter()
        .filter(|sub| {
            !dedup.enabled
                || !has_supported_embedded_subtitle(source, sub, device_profile)
        })
        .collect();
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

fn has_real_probe_data(source: &api::MediaSourceInfo) -> bool {
    matches!(
        source
            .remux
            .as_ref()
            .and_then(|remux| remux.source),
        Some(api::ProbeOrigin::Ffprobe | api::ProbeOrigin::RemuxDb)
    ) && !source
        .media_streams
        .is_empty()
}

fn has_supported_embedded_subtitle(
    source: &api::MediaSourceInfo,
    external: &crate::addons::SubtitleInfo,
    device_profile: Option<&api::DeviceProfile>,
) -> bool {
    let Some(profile) = device_profile else {
        return false;
    };
    let external_language = crate::subtitle_selection::normalized_language(
        external
            .lang
            .as_deref(),
    );
    if external_language == "und" || external_language.is_empty() {
        return false;
    }

    source
        .media_streams
        .iter()
        .any(|stream| {
            if stream.type_ != Some(api::MediaStreamType::Subtitle)
                || stream.is_external
                || stream
                    .delivery_method
                    .as_ref()
                    .is_some_and(|method| method != &api::SubtitleDeliveryMethod::Embed)
                || stream.is_forced != external.is_forced
                || stream.is_hearing_impaired != external.is_hi
                || crate::subtitle_selection::normalized_language(
                    stream
                        .language
                        .as_deref(),
                ) != external_language
            {
                return false;
            }
            let Some(codec) = stream
                .codec
                .as_deref()
            else {
                return false;
            };
            profile
                .subtitle_profiles
                .iter()
                .any(|supported| {
                    supported.method == Some(api::SubtitleDeliveryMethod::Embed)
                        && supported
                            .format
                            .as_deref()
                            .is_some_and(|format| {
                                crate::device_profile::subtitle_codec_matches_profile(
                                    codec, format,
                                )
                            })
                })
        })
}

/// Drops embedded subtitle streams the device can't play embedded when a
/// matching addon external subtitle already covers them (same language,
/// forced flag, and hearing-impaired flag — the same "is this actually the
/// same track" bar `has_supported_embedded_subtitle` already uses in the
/// opposite direction).
///
/// Extracting an unsupported embedded subtitle on demand is a slow HTTP
/// round trip against the source file (the download endpoint has to seek
/// into it with ffmpeg) — there's no reason to offer that when a fast,
/// already-fetched external copy exists.
///
/// Must run before `resolve_default_streams`/`compute_transcode_reasons` in
/// playback.rs's per-source loop — those, not `apply_subtitle_delivery`, are
/// what actually pick the default subtitle and decide whether a burn-in
/// transcode is needed. Removing the stream from `media_streams` here is
/// what keeps them from ever considering it; this function doesn't (and
/// shouldn't need to) touch their logic.
pub(crate) fn drop_unsupported_embedded_subtitles_with_external_match(
    source: &mut api::MediaSourceInfo,
    external_subtitles: &[crate::addons::SubtitleInfo],
    device_profile: Option<&api::DeviceProfile>,
) {
    source
        .media_streams
        .retain(|stream| {
            if stream.type_ != Some(api::MediaStreamType::Subtitle)
                || stream.is_external
            {
                return true;
            }
            let Some(codec) = stream
                .codec
                .as_deref()
                .and_then(|c| {
                    c.parse::<crate::device_profile::SubtitleCodec>()
                        .ok()
                })
            else {
                return true;
            };
            if crate::device_profile::profile_embeds_subtitle_codec(
                device_profile,
                &codec,
            ) {
                // Would be Embed delivery anyway — free, keep it.
                return true;
            }
            let stream_language = crate::subtitle_selection::normalized_language(
                stream
                    .language
                    .as_deref(),
            );
            let has_replacement = external_subtitles
                .iter()
                .any(|ext| {
                    ext.is_forced == stream.is_forced
                        && ext.is_hi == stream.is_hearing_impaired
                        && crate::subtitle_selection::normalized_language(
                            ext.lang
                                .as_deref(),
                        ) == stream_language
                });
            !has_replacement
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::HeaderValue;

    use crate::integration_test::{auth_header_with_token, authenticated_server};

    const DEDUP_ON: SubtitleDedupSettings = SubtitleDedupSettings {
        enabled: true,
        max_external_per_language_when_disabled: 1,
    };

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

    #[test]
    fn sidecars_follow_existing_stream_indexes() {
        let mut source = api::MediaSourceInfo {
            media_streams: vec![
                api::MediaStream {
                    index: 0,
                    type_: Some(api::MediaStreamType::Video),
                    ..Default::default()
                },
                api::MediaStream {
                    index: 2,
                    type_: Some(api::MediaStreamType::Audio),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let subtitles = vec![crate::addons::SubtitleInfo {
            id: "sidecar-4".to_string(),
            url: Some(crate::stream::StreamDescriptor::http(
                "https://example.com/Movie.en.srt",
            )),
            lang: Some("en".to_string()),
            is_forced: false,
            is_hi: false,
            filename: None,
            from_trusted: None,
            ai_translated: None,
        }];

        let routes = inject_sidecar_subtitles(&mut source, subtitles);

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].index, 3);
        assert_eq!(source.media_streams[2].index, 3);
        assert_eq!(
            source.media_streams[2]
                .codec
                .as_deref(),
            Some("subrip")
        );
    }

    #[test]
    fn external_subtitles_follow_sidecars() {
        assert_eq!(next_external_subtitle_index([0, 1], [2]), 3);
    }

    #[test]
    fn external_subtitles_only_follow_real_probe_streams() {
        let item_id = Uuid::new_v4();
        let existing = vec![
            api::MediaStream {
                index: 0,
                type_: Some(api::MediaStreamType::Video),
                ..Default::default()
            },
            api::MediaStream {
                index: 2,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("subrip".into()),
                ..Default::default()
            },
        ];
        let mut sources = vec![
            api::MediaSourceInfo {
                id: Uuid::new_v4(),
                media_streams: existing.clone(),
                remux: Some(api::MediaSourceRemuxInfo {
                    source: Some(api::ProbeOrigin::Ffprobe),
                    provider_info: Some(
                        serde_json::json!({"filename": "Movie.2026.mkv"}),
                    ),
                }),
                ..Default::default()
            },
            api::MediaSourceInfo {
                id: Uuid::new_v4(),
                media_streams: existing.clone(),
                remux: Some(api::MediaSourceRemuxInfo {
                    source: Some(api::ProbeOrigin::FilenameGuess),
                    ..Default::default()
                }),
                ..Default::default()
            },
            api::MediaSourceInfo {
                id: Uuid::new_v4(),
                ..Default::default()
            },
            api::MediaSourceInfo {
                id: Uuid::new_v4(),
                media_streams: existing,
                ..Default::default()
            },
        ];
        let subs = vec![crate::addons::SubtitleInfo {
            id: "external".into(),
            url: Some(crate::stream::StreamDescriptor::http(
                "https://example.com/Movie.2026.srt",
            )),
            lang: Some("eng".into()),
            is_forced: false,
            is_hi: false,
            filename: Some("Movie.2026.srt".into()),
            from_trusted: None,
            ai_translated: None,
        }];

        append_external_subtitles(
            &mut sources,
            &subs,
            &[],
            None,
            item_id,
            "test-key",
            DEDUP_ON,
        );

        assert_eq!(
            sources[0]
                .media_streams
                .len(),
            3
        );
        assert_eq!(sources[0].media_streams[2].index, 3);
        assert_eq!(sources[0].media_streams[2].is_external, true);
        assert_eq!(
            sources[1]
                .media_streams
                .len(),
            2
        );
        assert!(
            sources[2]
                .media_streams
                .is_empty()
        );
        assert_eq!(
            sources[3]
                .media_streams
                .len(),
            2
        );
    }

    #[test]
    fn supported_embedded_subtitle_replaces_only_the_matching_external_variant() {
        let item_id = Uuid::new_v4();
        let source = api::MediaSourceInfo {
            id: Uuid::new_v4(),
            media_streams: vec![api::MediaStream {
                index: 2,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("pgssub".into()),
                language: Some("eng".into()),
                ..Default::default()
            }],
            remux: Some(api::MediaSourceRemuxInfo {
                source: Some(api::ProbeOrigin::Ffprobe),
                ..Default::default()
            }),
            ..Default::default()
        };
        let external = crate::addons::SubtitleInfo {
            id: "regular".into(),
            url: None,
            lang: Some("en".into()),
            is_forced: false,
            is_hi: false,
            filename: None,
            from_trusted: None,
            ai_translated: None,
        };
        let embed_profile = api::DeviceProfile {
            subtitle_profiles: vec![api::SubtitleProfile {
                format: Some("pgs".into()),
                method: Some(api::SubtitleDeliveryMethod::Embed),
            }],
            ..Default::default()
        };
        let external_profile = api::DeviceProfile {
            subtitle_profiles: vec![api::SubtitleProfile {
                format: Some("pgs".into()),
                method: Some(api::SubtitleDeliveryMethod::External),
            }],
            ..Default::default()
        };

        let mut sources = vec![source.clone()];
        append_external_subtitles(
            &mut sources,
            &[external.clone()],
            &[],
            Some(&embed_profile),
            item_id,
            "test-key",
            DEDUP_ON,
        );
        assert_eq!(
            sources[0]
                .media_streams
                .len(),
            1
        );

        for profile in [None, Some(&external_profile)] {
            let mut sources = vec![source.clone()];
            append_external_subtitles(
                &mut sources,
                &[external.clone()],
                &[],
                profile,
                item_id,
                "test-key",
                DEDUP_ON,
            );
            assert_eq!(
                sources[0]
                    .media_streams
                    .len(),
                2
            );
        }

        let mut forced = external.clone();
        forced.is_forced = true;
        let mut sources = vec![source.clone()];
        append_external_subtitles(
            &mut sources,
            &[forced],
            &[],
            Some(&embed_profile),
            item_id,
            "test-key",
            DEDUP_ON,
        );
        assert_eq!(
            sources[0]
                .media_streams
                .len(),
            2
        );

        let mut external_source = source;
        external_source.media_streams[0].delivery_method =
            Some(api::SubtitleDeliveryMethod::External);
        let mut sources = vec![external_source];
        append_external_subtitles(
            &mut sources,
            &[external],
            &[],
            Some(&embed_profile),
            item_id,
            "test-key",
            DEDUP_ON,
        );
        assert_eq!(
            sources[0]
                .media_streams
                .len(),
            2
        );
    }

    #[test]
    fn dutch_bibliographic_code_does_not_duplicate_embedded_subtitle() {
        let source = api::MediaSourceInfo {
            id: Uuid::new_v4(),
            media_streams: vec![api::MediaStream {
                index: 2,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("PGSSUB".into()),
                language: Some("nld".into()),
                ..Default::default()
            }],
            remux: Some(api::MediaSourceRemuxInfo {
                source: Some(api::ProbeOrigin::RemuxDb),
                ..Default::default()
            }),
            ..Default::default()
        };
        let external = crate::addons::SubtitleInfo {
            id: "dutch".into(),
            url: None,
            lang: Some("dut".into()),
            is_forced: false,
            is_hi: false,
            filename: None,
            from_trusted: None,
            ai_translated: None,
        };
        let profile = api::DeviceProfile {
            subtitle_profiles: vec![api::SubtitleProfile {
                format: Some("pgs".into()),
                method: Some(api::SubtitleDeliveryMethod::Embed),
            }],
            ..Default::default()
        };

        let mut sources = vec![source];
        append_external_subtitles(
            &mut sources,
            &[external],
            &[],
            Some(&profile),
            Uuid::new_v4(),
            "test-key",
            DEDUP_ON,
        );
        assert_eq!(
            sources[0]
                .media_streams
                .len(),
            1
        );
    }

    #[test]
    fn dedup_disabled_keeps_multiple_external_candidates_up_to_the_cap() {
        let source = api::MediaSourceInfo {
            id: Uuid::new_v4(),
            media_streams: vec![api::MediaStream {
                index: 2,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("pgssub".into()),
                language: Some("eng".into()),
                ..Default::default()
            }],
            remux: Some(api::MediaSourceRemuxInfo {
                source: Some(api::ProbeOrigin::Ffprobe),
                ..Default::default()
            }),
            ..Default::default()
        };
        let embed_profile = api::DeviceProfile {
            subtitle_profiles: vec![api::SubtitleProfile {
                format: Some("pgs".into()),
                method: Some(api::SubtitleDeliveryMethod::Embed),
            }],
            ..Default::default()
        };
        let regular = crate::addons::SubtitleInfo {
            id: "regular".into(),
            url: None,
            lang: Some("en".into()),
            is_forced: false,
            is_hi: false,
            filename: None,
            from_trusted: None,
            ai_translated: None,
        };
        let mut forced = regular.clone();
        forced.id = "forced".into();
        forced.is_forced = true;

        // Dedup on: the embedded English track (Embed-supported) suppresses
        // both external candidates for English.
        let mut sources = vec![source.clone()];
        append_external_subtitles(
            &mut sources,
            &[regular.clone(), forced.clone()],
            &[],
            Some(&embed_profile),
            Uuid::new_v4(),
            "test-key",
            DEDUP_ON,
        );
        assert_eq!(
            sources[0]
                .media_streams
                .len(),
            1,
            "dedup on: embedded already covers English, no external added"
        );

        // Dedup off with a cap of 2: both externals show up alongside the
        // embedded track, even though the embedded one would otherwise
        // suppress a same-flavor external.
        let mut sources = vec![source];
        append_external_subtitles(
            &mut sources,
            &[regular, forced],
            &[],
            Some(&embed_profile),
            Uuid::new_v4(),
            "test-key",
            SubtitleDedupSettings {
                enabled: false,
                max_external_per_language_when_disabled: 2,
            },
        );
        assert_eq!(
            sources[0]
                .media_streams
                .len(),
            3,
            "dedup off: embedded track plus both external candidates, up to the cap"
        );
    }

    fn dutch_pgs_source() -> api::MediaSourceInfo {
        api::MediaSourceInfo {
            id: Uuid::new_v4(),
            media_streams: vec![api::MediaStream {
                index: 2,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("pgssub".into()),
                language: Some("dut".into()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn no_embed_profile() -> api::DeviceProfile {
        // Supports the codec for some non-Embed method (or not at all) —
        // either way, profile_embeds_subtitle_codec must say no.
        api::DeviceProfile::default()
    }

    #[test]
    fn drops_unsupported_embedded_subtitle_with_a_matching_external() {
        let mut source = dutch_pgs_source();
        let external = crate::addons::SubtitleInfo {
            id: "dutch".into(),
            url: None,
            lang: Some("dut".into()),
            is_forced: false,
            is_hi: false,
            filename: None,
            from_trusted: None,
            ai_translated: None,
        };
        drop_unsupported_embedded_subtitles_with_external_match(
            &mut source,
            &[external],
            Some(&no_embed_profile()),
        );
        assert!(
            source
                .media_streams
                .is_empty(),
            "unsupported embedded subtitle with a matching external must be dropped"
        );
    }

    #[test]
    fn keeps_unsupported_embedded_subtitle_without_a_language_match() {
        let mut source = dutch_pgs_source();
        // Different language — not a replacement for the Dutch track.
        let external = crate::addons::SubtitleInfo {
            id: "english".into(),
            url: None,
            lang: Some("eng".into()),
            is_forced: false,
            is_hi: false,
            filename: None,
            from_trusted: None,
            ai_translated: None,
        };
        drop_unsupported_embedded_subtitles_with_external_match(
            &mut source,
            &[external],
            Some(&no_embed_profile()),
        );
        assert_eq!(
            source
                .media_streams
                .len(),
            1,
            "a different-language external must not hide the embedded track"
        );
    }

    #[test]
    fn keeps_embed_supported_subtitle_even_with_a_matching_external() {
        let mut source = dutch_pgs_source();
        let external = crate::addons::SubtitleInfo {
            id: "dutch".into(),
            url: None,
            lang: Some("dut".into()),
            is_forced: false,
            is_hi: false,
            filename: None,
            from_trusted: None,
            ai_translated: None,
        };
        let profile = api::DeviceProfile {
            subtitle_profiles: vec![api::SubtitleProfile {
                format: Some("pgs".into()),
                method: Some(api::SubtitleDeliveryMethod::Embed),
            }],
            ..Default::default()
        };
        drop_unsupported_embedded_subtitles_with_external_match(
            &mut source,
            &[external],
            Some(&profile),
        );
        assert_eq!(
            source
                .media_streams
                .len(),
            1,
            "an embed-supported subtitle is free to deliver — never drop it in favor of external"
        );
    }

    fn source_with_subtitles(streams: Vec<api::MediaStream>) -> api::MediaSourceInfo {
        api::MediaSourceInfo {
            id: Uuid::new_v4(),
            media_streams: streams,
            ..Default::default()
        }
    }

    #[test]
    fn extractable_subtitles_skips_external_and_image_streams_and_orders_by_index() {
        let source = source_with_subtitles(vec![
            api::MediaStream {
                index: 5,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("subrip".into()),
                is_external: false,
                ..Default::default()
            },
            api::MediaStream {
                index: 1,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("mov_text".into()),
                is_external: false,
                ..Default::default()
            },
            api::MediaStream {
                index: 3,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("subrip".into()),
                is_external: true,
                ..Default::default()
            },
            api::MediaStream {
                index: 2,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("hdmv_pgs_subtitle".into()),
                is_external: false,
                ..Default::default()
            },
            api::MediaStream {
                index: 0,
                type_: Some(api::MediaStreamType::Audio),
                codec: Some("aac".into()),
                ..Default::default()
            },
        ]);

        let extractable = extractable_subtitles(&source);
        let indexes: Vec<i64> = extractable
            .iter()
            .map(|s| s.stream_index)
            .collect();
        assert_eq!(
            indexes,
            vec![1, 5],
            "external, non-subtitle, and image/bitmap subtitle streams must be \
             excluded (ffmpeg can't convert PGS/VobSub to a text codec — one in \
             the batch would fail the whole ffmpeg invocation), remaining ones \
             sorted by index"
        );
        // map_spec is the ordinal among *extracted text* streams, not the raw index.
        assert_eq!(extractable[0].map_spec, "0:s:0");
        assert_eq!(extractable[1].map_spec, "0:s:1");
        assert_eq!(extractable[0].cache_codec, api::SubtitleCodec::Srt);
        assert_eq!(extractable[1].cache_codec, api::SubtitleCodec::Srt);
    }

    #[test]
    fn extractable_subtitles_gives_native_ass_a_second_srt_output() {
        // A VTT/SRT/JSON request is always served from the SRT cache. Without
        // this second output, a stream whose native codec is ASS would only
        // ever get a .ass file, and such a request would 404 forever.
        let source = source_with_subtitles(vec![api::MediaStream {
            index: 0,
            type_: Some(api::MediaStreamType::Subtitle),
            codec: Some("ass".into()),
            ..Default::default()
        }]);

        let extractable = extractable_subtitles(&source);
        assert_eq!(
            extractable.len(),
            2,
            "a native ASS stream must produce both an .ass and an .srt output"
        );
        assert!(
            extractable
                .iter()
                .all(|s| s.stream_index == 0 && s.map_spec == "0:s:0"),
            "both outputs come from the same mapped input stream: {extractable:?}"
        );
        assert!(
            extractable
                .iter()
                .any(|s| s.cache_codec == api::SubtitleCodec::Ass)
                && extractable
                    .iter()
                    .any(|s| s.cache_codec == api::SubtitleCodec::Srt),
            "expected one native-ASS (copy) output and one SRT-converted output: {extractable:?}"
        );
        let ass_output = extractable
            .iter()
            .find(|s| s.cache_codec == api::SubtitleCodec::Ass)
            .unwrap();
        assert_eq!(
            ass_output.ffmpeg_codec, "copy",
            "native ASS should be stream-copied, not re-encoded"
        );
        let srt_output = extractable
            .iter()
            .find(|s| s.cache_codec == api::SubtitleCodec::Srt)
            .unwrap();
        assert_eq!(srt_output.ffmpeg_codec, "srt");
    }

    #[test]
    fn build_subtitle_extraction_args_has_one_input_and_one_map_pair_per_output() {
        let data_dir = std::path::Path::new("/tmp/remux-subtitle-test");
        let item_id = Uuid::new_v4();
        // subrip -> 1 SRT output. ass -> 2 outputs (ass copy + srt convert),
        // both mapped from the same input stream. PGS is excluded entirely.
        let source = source_with_subtitles(vec![
            api::MediaStream {
                index: 0,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("subrip".into()),
                ..Default::default()
            },
            api::MediaStream {
                index: 1,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("ass".into()),
                ..Default::default()
            },
            api::MediaStream {
                index: 2,
                type_: Some(api::MediaStreamType::Subtitle),
                codec: Some("hdmv_pgs_subtitle".into()),
                ..Default::default()
            },
        ]);
        let streams = extractable_subtitles(&source);
        assert_eq!(streams.len(), 3, "1 (subrip) + 2 (ass) + 0 (pgs, excluded)");

        let (args, cache_paths) = build_subtitle_extraction_args(
            data_dir,
            "http://example.com/stream.mkv",
            item_id,
            &streams,
        )
        .expect("arg construction should not fail for a valid path");

        // Exactly one -i (single ffmpeg process reads the file once), and it
        // comes before every -map (single input feeding every output).
        let input_positions: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == "-i")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            input_positions.len(),
            1,
            "expected exactly one -i: {args:?}"
        );
        let input_pos = input_positions[0];
        assert_eq!(args[input_pos + 1], "http://example.com/stream.mkv");
        let first_map_pos = args
            .iter()
            .position(|a| a == "-map")
            .expect("expected at least one -map");
        assert!(
            input_pos < first_map_pos,
            "-i must precede every -map: {args:?}"
        );

        // http source should get reconnect args.
        assert!(
            args.contains(&"-reconnect".to_string()),
            "http input should carry reconnect args: {args:?}"
        );

        // One -map/-c:s pair per output, and one (tmp, final) path pair per output.
        let map_count = args
            .iter()
            .filter(|a| a.as_str() == "-map")
            .count();
        let codec_count = args
            .iter()
            .filter(|a| a.as_str() == "-c:s")
            .count();
        assert_eq!(map_count, 3, "expected one -map per output: {args:?}");
        assert_eq!(codec_count, 3, "expected one -c:s per output: {args:?}");
        assert_eq!(cache_paths.len(), 3);

        // subrip (ordinal 0) maps once; ass (ordinal 1) maps twice — once for
        // each of its two outputs — from the same input stream.
        let map_specs: Vec<&String> = args
            .windows(2)
            .filter(|w| w[0] == "-map")
            .map(|w| &w[1])
            .collect();
        assert_eq!(map_specs, vec!["0:s:0", "0:s:1", "0:s:1"]);

        // ffmpeg is only ever pointed at .tmp output paths — the final cache
        // path is populated by an atomic rename after success, never written
        // to directly, so a concurrent reader can never see a partial file.
        for (tmp, final_path) in &cache_paths {
            assert!(
                tmp.to_str()
                    .unwrap()
                    .ends_with(".tmp"),
                "ffmpeg output path should be a .tmp path: {tmp:?}"
            );
            assert_ne!(tmp, final_path);
            assert!(
                args.contains(
                    &tmp.to_str()
                        .unwrap()
                        .to_string()
                ),
                "the .tmp path must be what's actually passed to ffmpeg: {args:?}"
            );
            assert!(
                !args.contains(
                    &final_path
                        .to_str()
                        .unwrap()
                        .to_string()
                ),
                "the final cache path must never be passed to ffmpeg directly: {args:?}"
            );
        }
    }

    #[test]
    fn build_subtitle_extraction_args_skips_reconnect_for_local_files() {
        let data_dir = std::path::Path::new("/tmp/remux-subtitle-test");
        let source = source_with_subtitles(vec![api::MediaStream {
            index: 0,
            type_: Some(api::MediaStreamType::Subtitle),
            codec: Some("subrip".into()),
            ..Default::default()
        }]);
        let streams = extractable_subtitles(&source);

        let (args, _) = build_subtitle_extraction_args(
            data_dir,
            "/mnt/media/movie.mkv",
            Uuid::new_v4(),
            &streams,
        )
        .expect("arg construction should not fail for a valid path");

        assert!(
            !args.contains(&"-reconnect".to_string()),
            "a local path should not get http reconnect args: {args:?}"
        );
    }
}
