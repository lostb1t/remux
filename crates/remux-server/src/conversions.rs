use crate::{
    addons::SubtitleInfo,
    api, common,
    common::{ToRunTimeTicks, get_uuid},
    db,
    playback::probe::{
        StreamMeta, display_title_audio, display_title_subtitle, display_title_video,
    },
    sdks::stremio,
    stream::StreamDescriptor,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};
use tracing::warn;

// Heuristic metadata fallback for remote source URLs when ffprobe metadata is
// unavailable. This keeps clients functional (stream selection/transcode
// decisions) instead of exposing empty stream lists.
fn infer_container_from_url(url: &str) -> Option<remux_sdks::remux::VideoContainer> {
    let path = url::Url::parse(url)
        .ok()
        .map(|u| {
            u.path()
                .to_string()
        })
        .unwrap_or_else(|| url.to_string());
    let filename = path
        .rsplit('/')
        .next()
        .unwrap_or(path.as_str());
    let ext = filename
        .rsplit('.')
        .next()?
        .to_ascii_lowercase();
    if ext == "m3u8" {
        return Some(remux_sdks::remux::VideoContainer::Other("hls".to_string()));
    }
    remux_sdks::remux::VideoContainer::parse_known(&ext).map(|c| c.canonical())
}

/// Best-effort media-info guess parsed from a release filename (e.g.
/// "Movie.2023.2160p.UHD.BluRay.x265.DTS-HD.MA.7.1-GROUP.mkv") via the
/// `hunch` crate. Used only to fill in MediaStreams/container on the item
/// details response when neither RemuxDB nor a prior ffprobe has real data —
/// never persisted, never consulted for playback/transcode decisions.
pub(crate) struct FilenameProbeGuess {
    pub container: Option<api::VideoContainer>,
    pub media_streams: Vec<api::MediaStream>,
}

fn map_hunch_video_codec(s: &str) -> Option<String> {
    remux_sdks::remux::VideoCodec::parse_known(s).map(|c| c.to_string())
}

fn map_hunch_audio_codec(s: &str) -> Option<String> {
    remux_sdks::remux::AudioCodec::parse_known(s).map(|c| c.to_string())
}

fn screen_size_to_dimensions(s: &str) -> Option<(i64, i64)> {
    match s {
        "2160p" => Some((3840, 2160)),
        "1080p" => Some((1920, 1080)),
        "720p" => Some((1280, 720)),
        "480p" => Some((854, 480)),
        "360p" => Some((640, 360)),
        _ => None,
    }
}

/// Parses hunch's "5.1" / "7.1" / "2.0" audio_channels token into a channel
/// count (front + LFE/rear, e.g. "5.1" -> 6, "7.1" -> 8).
fn parse_channel_count(s: &str) -> Option<i64> {
    let (front, rear) = s.split_once('.')?;
    Some(
        front
            .parse::<i64>()
            .ok()?
            + rear
                .parse::<i64>()
                .ok()?,
    )
}

fn hunch_video_range_type(other: &[&str]) -> Option<api::VideoRangeType> {
    other
        .iter()
        .find_map(|&o| match o {
            "Dolby Vision" => Some(api::VideoRangeType::Dovi),
            "HDR10+" => Some(api::VideoRangeType::Hdr10Plus),
            "HDR10" => Some(api::VideoRangeType::Hdr10),
            _ => None,
        })
}

/// Coarse `VideoRange` companion for `VideoRangeType`, mirroring how the real
/// ffprobe path (playback::probe) always derives both together. We only ever
/// assert `Hdr` on positive filename evidence — absence of an HDR tag doesn't
/// mean SDR, it just means hunch found nothing, so it stays `None` (unknown).
fn video_range_from_type(t: Option<&api::VideoRangeType>) -> Option<api::VideoRange> {
    match t? {
        api::VideoRangeType::Sdr => Some(api::VideoRange::Sdr),
        api::VideoRangeType::Other => Some(api::VideoRange::Other),
        _ => Some(api::VideoRange::Hdr),
    }
}

pub(crate) fn guess_media_source_from_filename(filename: &str) -> FilenameProbeGuess {
    let parsed = hunch::hunch(filename);
    let mut media_streams = Vec::new();

    let video_codec = parsed
        .video_codec()
        .and_then(map_hunch_video_codec);
    let (width, height) = crate::db::min_screen_size(&parsed)
        .and_then(screen_size_to_dimensions)
        .map(|(w, h)| (Some(w), Some(h)))
        .unwrap_or((None, None));
    let video_range_type = hunch_video_range_type(&parsed.other());
    let bit_depth = parsed
        .color_depth()
        .and_then(|s| {
            s.trim_end_matches("-bit")
                .parse::<i64>()
                .ok()
        });

    if video_codec.is_some()
        || width.is_some()
        || video_range_type.is_some()
        || bit_depth.is_some()
    {
        let video_range = video_range_from_type(video_range_type.as_ref());
        let display_title = display_title_video(&StreamMeta {
            codec: video_codec.as_deref(),
            width,
            height,
            video_range: video_range.as_ref(),
            ..Default::default()
        });
        media_streams.push(api::MediaStream {
            index: 0,
            type_: Some(api::MediaStreamType::Video),
            codec: video_codec,
            width,
            height,
            video_range,
            video_range_type,
            bit_depth,
            display_title,
            // Never flag a filename guess as the container's default stream —
            // resolve_default_streams() falls back to the first is_default
            // stream, which would otherwise stamp DefaultAudioStreamIndex from
            // pure guesswork.
            is_default: Some(false),
            ..Default::default()
        });
    }

    let audio_codec = parsed
        .audio_codec()
        .and_then(map_hunch_audio_codec);
    let channels = parsed
        .audio_channels()
        .and_then(parse_channel_count);
    if audio_codec.is_some() || channels.is_some() {
        let display_title = display_title_audio(&StreamMeta {
            codec: audio_codec.as_deref(),
            channels,
            ..Default::default()
        });
        media_streams.push(api::MediaStream {
            index: media_streams.len() as i64,
            type_: Some(api::MediaStreamType::Audio),
            codec: audio_codec,
            channels,
            display_title,
            is_default: Some(false),
            ..Default::default()
        });
    }

    let container = parsed
        .container()
        .and_then(api::VideoContainer::parse_known)
        .map(|c| c.canonical());

    FilenameProbeGuess {
        container,
        media_streams,
    }
}

/// Fills in item-details MediaSources whose `media_streams` came back empty
/// (RemuxDB miss and never probed) with best-effort facts: codec/resolution/etc.
/// guessed from the release filename, plus an overall bitrate derived from the
/// addon-reported file size and the item's known runtime (bitrate = size*8 /
/// duration — no filename parsing involved for that part). Persists the
/// result into `db::Media.probe_data` tagged `ProbeOrigin::FilenameGuess` so
/// later requests don't re-guess — but that tag is exactly what keeps this
/// out of the playback/transcode path: `MediaSourceInfo::is_filename_guess()`
/// is checked everywhere a real ffprobe/RemuxDB result would otherwise be
/// trusted (see `probe_stream` in playback::probe), so a guess here can never
/// block or stand in for a real probe.
pub(crate) async fn apply_filename_probe_fallback(
    base_item: &mut api::BaseItemDto,
    sources: &[db::Media],
    db: &sqlx::SqlitePool,
) {
    let Some(infos) = base_item
        .media_sources
        .as_mut()
    else {
        return;
    };
    for (info, source) in infos
        .iter_mut()
        .zip(sources.iter())
    {
        if !info
            .media_streams
            .is_empty()
        {
            continue;
        }
        let mut estimated = false;

        if let Some(filename) = source
            .stream_info
            .as_ref()
            .and_then(|si| {
                si.filename
                    .as_deref()
            })
        {
            let guess = guess_media_source_from_filename(filename);
            if !guess
                .media_streams
                .is_empty()
            {
                info.media_streams = guess.media_streams;
                if info
                    .container
                    .is_none()
                {
                    info.container = guess.container;
                }
                estimated = true;
            }
        }

        if info
            .bitrate
            .is_none()
        {
            let size = source
                .stream_info
                .as_ref()
                .and_then(|si| si.size)
                .or(info.size);
            if let (Some(size), Some(ticks)) = (size, info.run_time_ticks) {
                let secs = common::ticks_to_seconds(ticks);
                if secs > 0.0 {
                    info.size
                        .get_or_insert(size);
                    info.bitrate = Some(((size as f64 * 8.0) / secs).round() as i64);
                    estimated = true;
                }
            }
        }

        if !estimated {
            continue;
        }
        info.remux
            .get_or_insert_with(Default::default)
            .source = Some(api::ProbeOrigin::FilenameGuess);

        let persisted = api::MediaSourceInfo {
            media_streams: info
                .media_streams
                .clone(),
            container: info
                .container
                .clone(),
            bitrate: info.bitrate,
            size: info.size,
            run_time_ticks: info.run_time_ticks,
            remux: Some(api::MediaSourceRemuxInfo {
                source: Some(api::ProbeOrigin::FilenameGuess),
                ..Default::default()
            }),
            ..Default::default()
        };
        if let Err(e) = db::Media::save_probe_data(db, &source.id, &persisted).await {
            warn!(id = %source.id, error = %e, "failed to persist filename-guessed probe data");
        }
    }
}

impl From<db::Media> for api::MediaSourceInfo {
    fn from(source: db::Media) -> Self {
        let descriptor = source
            .stream_info
            .as_ref()
            .map(|si| &si.descriptor);
        let is_stub = descriptor
            .and_then(|d| d.as_http_url())
            .is_none();
        let container = source
            .probe_data
            .as_ref()
            .and_then(|p| {
                p.container
                    .clone()
            })
            .or_else(|| {
                descriptor
                    .and_then(|d| d.as_http_url())
                    .and_then(infer_container_from_url)
            });

        // Carry the persisted probe-source tag forward — without this, a
        // FilenameGuess (or Ffprobe/RemuxDb) tag stored on probe_data would
        // silently vanish from the response every time this From impl
        // rebuilds `remux`, making the estimate indistinguishable from real
        // data to any client checking it.
        let probe_source = source
            .probe_data
            .as_ref()
            .and_then(|p| {
                p.remux
                    .as_ref()
            })
            .and_then(|r| r.source);
        let remux = Some(api::MediaSourceRemuxInfo {
            provider_info: source
                .stream_info
                .as_ref()
                .and_then(|si| serde_json::to_value(si).ok()),
            source: probe_source,
        });

        let path = Some({
            let stem = source
                .stream_info
                .as_ref()
                .and_then(|si| {
                    si.filename
                        .as_deref()
                })
                .and_then(|f| {
                    std::path::Path::new(f)
                        .file_stem()
                        .and_then(|s| s.to_str())
                });
            match stem {
                Some(s) => format!("/remux/{}/{}", source.id, s),
                None => format!("/remux/{}", source.id),
            }
        });
        let is_remote = false;
        let protocol = api::MediaProtocol::File;

        let client_id = source
            .group_id
            .unwrap_or(source.id);
        let probe_ticks = source
            .probe_data
            .as_ref()
            .and_then(|p| p.run_time_ticks);
        let meta_ticks = source
            .runtime
            .and_then(|r| r.to_ticks(common::TickUnit::Seconds));
        let run_time_ticks = probe_ticks.or(meta_ticks);
        let probe_bitrate = source
            .probe_data
            .as_ref()
            .and_then(|p| p.bitrate);
        let probe_size = source
            .probe_data
            .as_ref()
            .and_then(|p| p.size);
        let mut media_streams = source
            .probe_data
            .map(|mut p| {
                for s in &mut p.media_streams {
                    if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                        s.is_text_subtitle_stream = s.is_text_subtitle_stream();
                    }
                }
                p.media_streams
            })
            .unwrap_or_default();

        // Derive display_title for any stream that doesn't have one yet.
        // This covers streams loaded from RemuxDB probe data where only raw
        // track facts are stored; FFprobe-sourced streams may already have it.
        for stream in &mut media_streams {
            if stream
                .display_title
                .is_some()
            {
                continue;
            }
            let meta = StreamMeta {
                language: stream
                    .language
                    .as_deref(),
                codec: stream
                    .codec
                    .as_deref(),
                profile: stream
                    .profile
                    .as_deref(),
                channels: stream.channels,
                channel_layout: stream
                    .channel_layout
                    .as_deref(),
                width: stream.width,
                height: stream.height,
                video_range: None,
                is_default: stream
                    .is_default
                    .unwrap_or(false),
                is_forced: stream.is_forced,
                is_external: stream.is_external,
                is_hearing_impaired: stream.is_hearing_impaired,
                title: stream
                    .title
                    .as_deref(),
            };
            stream.display_title = match stream.type_ {
                Some(api::MediaStreamType::Video) => display_title_video(&meta),
                Some(api::MediaStreamType::Audio) => display_title_audio(&meta),
                Some(api::MediaStreamType::Subtitle) => display_title_subtitle(&meta),
                _ => None,
            };
        }

        // Clients that use /Items/{id}/File for direct playback inspect
        // MediaStreams before deciding to play. Synthesize a stub so they
        // don't reject unprobed tracks outright.
        if source.kind == db::MediaKind::Track && media_streams.is_empty() {
            media_streams = vec![api::MediaStream {
                type_: Some(api::MediaStreamType::Audio),
                codec: Some("aac".to_string()),
                channels: Some(2),
                is_default: Some(true),
                display_title: Some("Audio".to_string()),
                index: 0,
                ..Default::default()
            }];
        }
        api::MediaSourceInfo {
            id: client_id,
            e_tag: client_id,
            path,
            protocol,
            is_remote,
            name: Some(
                source
                    .title
                    .clone(),
            ),
            container,
            remux,
            has_segments: !is_stub,
            formats: Some(vec![]),
            required_http_headers: Some(HashMap::new()),
            run_time_ticks,
            bitrate: probe_bitrate,
            size: probe_size,
            media_streams,
            // `default_audio_stream_index` / `default_subtitle_stream_index` are
            // derived per request via `MediaSourceInfo::resolve_default_streams`;
            // stored probe data never carries them.
            ..Default::default()
        }
    }
}
impl From<api::DisplayPreferencesDto> for db::JellyfinDisplayPrefsData {
    fn from(dto: api::DisplayPreferencesDto) -> Self {
        Self {
            view_type: dto.view_type,
            sort_by: dto.sort_by,
            index_by: dto.index_by,
            remember_indexing: dto.remember_indexing,
            primary_image_height: dto.primary_image_height,
            primary_image_width: dto.primary_image_width,
            custom_prefs: dto.custom_prefs,
            scroll_direction: dto.scroll_direction,
            show_backdrop: dto.show_backdrop,
            remember_sorting: dto.remember_sorting,
            sort_order: dto.sort_order,
            show_sidebar: dto.show_sidebar,
            home_sections: None,
        }
    }
}

impl TryFrom<stremio::Episode> for db::Media {
    type Error = anyhow::Error;
    fn try_from(meta: stremio::Episode) -> Result<db::Media> {
        let mut media = db::Media {
            title: meta
                .get_name()
                .unwrap_or_default(),
            kind: db::MediaKind::Episode,
            released_at: meta
                .released
                .map(|x| x.naive_utc()),
            runtime: meta
                .runtime
                .map(|d| d.num_seconds()),
            description: meta
                .overview
                .or(meta.description),
            rating_audience: meta.rating,
            ..Default::default()
        };
        if let Some(url) = meta.thumbnail {
            media.set_image(db::ImageKind::Primary, url);
        }
        Ok(media)
    }
}

pub fn subtitle_to_media_stream(sub: &SubtitleInfo) -> api::MediaStream {
    let path_hint = match &sub.url {
        Some(StreamDescriptor::Http { url, .. }) => url.as_str(),
        Some(StreamDescriptor::Local(p)) => p
            .to_str()
            .unwrap_or(""),
        Some(StreamDescriptor::Torrent {
            file_hint: Some(path),
            ..
        }) => path.as_str(),
        Some(StreamDescriptor::Opendal { path, .. }) => path.as_str(),
        _ => "",
    };
    let lc = path_hint.to_ascii_lowercase();
    let codec = if lc.ends_with(".vtt") {
        "webvtt"
    } else if lc.ends_with(".srt") {
        "subrip"
    } else if lc.ends_with(".ass") || lc.ends_with(".ssa") {
        "ass"
    } else {
        "webvtt"
    };
    api::MediaStream {
        index: 0,
        type_: Some(api::MediaStreamType::Subtitle),
        codec: Some(codec.to_string()),
        language: sub
            .lang
            .clone(),
        display_title: Some({
            let lang = sub
                .lang
                .clone()
                .unwrap_or_else(|| "und".into());
            format!("{} - {} - External", lang, codec.to_uppercase())
        }),
        is_default: Some(false),
        is_forced: sub.is_forced,
        is_hearing_impaired: sub.is_hi,
        is_external: true,
        is_text_subtitle_stream: true,
        supports_external_stream: true,
        delivery_method: Some(api::SubtitleDeliveryMethod::External),
        is_external_url: Some(false),
        audio_spatial_format: Some("None".to_string()),
        video_range: Some(api::VideoRange::Other),
        video_range_type: Some(api::VideoRangeType::Other),
        localized_undefined: Some("Undefined".to_string()),
        localized_default: Some("Default".to_string()),
        localized_forced: Some("Forced".to_string()),
        localized_external: Some("External".to_string()),
        localized_hearing_impaired: Some("Hearing Impaired".to_string()),
        ..Default::default()
    }
}

pub fn stream_into_media_source_info(
    id: String,
    jellyfin_media_type: api::MediaType,
    stream: stremio::Stream,
) -> api::MediaSourceInfo {
    let id = get_uuid();
    api::MediaSourceInfo {
        id: id.clone(),
        e_tag: id.clone(),
        path: stream.url,
        protocol: api::MediaProtocol::File,
        supports_transcoding: false,
        supports_direct_stream: true,
        supports_direct_play: true,
        is_remote: false,
        name: stream
            .name
            .clone(),
        ..Default::default()
    }
}

fn to_option_bool(flag: i64) -> Option<bool> {
    match flag {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

// --- Subtitle text conversion ---
//
// Jellyfin-web fetches subtitles as either JSON TrackEvents (Stream.js)
// or WebVTT (Stream.vtt).  We extract to SRT via ffmpeg and convert.

/// Convert SRT to WebVTT. Existing VTT is retained with its timestamps normalized.
pub fn srt_to_vtt(input: &str) -> String {
    let input = input.trim_start_matches('\u{FEFF}');
    let normalized = input
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let input = normalized.as_str();
    if input
        .trim_start()
        .starts_with("WEBVTT")
    {
        // If there's a second WEBVTT header mid-file (e.g. OpenSubtitles metadata
        // block), drop everything before it — the real cues start there.
        let second = input
            .find("WEBVTT")
            .and_then(|first| {
                input[first + 6..]
                    .find("WEBVTT")
                    .map(|off| first + 6 + off)
            });
        if let Some(pos) = second {
            return normalize_webvtt_timestamps(
                input[pos..].trim_start_matches('\u{FEFF}'),
            );
        }
        return normalize_webvtt_timestamps(input);
    }
    let mut out = String::from("WEBVTT\n\n");
    for block in input
        .trim()
        .split("\n\n")
    {
        let lines: Vec<&str> = block
            .lines()
            .collect();
        if lines.len() < 2 {
            continue;
        }
        let rest = if lines[0]
            .trim()
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            &lines[1..]
        } else {
            &lines[..]
        };
        if rest.is_empty() {
            continue;
        }
        let timecode = rest[0].replace(',', ".");
        out.push_str(&timecode);
        out.push('\n');
        for line in &rest[1..] {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Some subtitle providers prepend a `WEBVTT` header to otherwise-SRT content.
/// Browsers reject comma-separated milliseconds in WebVTT timing lines, so
/// normalize just the timestamp tokens while preserving cue settings and text.
fn normalize_webvtt_timestamps(input: &str) -> String {
    input
        .lines()
        .map(|line| {
            let Some((start, end_and_settings)) = line.split_once("-->") else {
                return line.to_string();
            };
            let end_and_settings = end_and_settings.trim();
            let (end, settings) = end_and_settings
                .split_once(char::is_whitespace)
                .unwrap_or((end_and_settings, ""));
            let settings = settings.trim_start();
            if settings.is_empty() {
                format!(
                    "{} --> {}",
                    start
                        .trim()
                        .replace(',', "."),
                    end.replace(',', ".")
                )
            } else {
                format!(
                    "{} --> {} {}",
                    start
                        .trim()
                        .replace(',', "."),
                    end.replace(',', "."),
                    settings
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert SRT to Jellyfin JSON TrackEvents format (1 tick = 100 ns).
pub fn srt_to_jellyfin_json(input: &str) -> String {
    let normalized = input
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut events: Vec<serde_json::Value> = Vec::new();
    for block in normalized
        .trim()
        .split("\n\n")
    {
        let lines: Vec<&str> = block
            .lines()
            .collect();
        if lines.len() < 2 {
            continue;
        }
        let content = if lines[0]
            .trim()
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            &lines[1..]
        } else {
            &lines[..]
        };
        if content.is_empty() {
            continue;
        }
        let parts: Vec<&str> = content[0]
            .split("-->")
            .collect();
        if parts.len() < 2 {
            continue;
        }
        let start = srt_timestamp_to_ticks(parts[0].trim());
        let end = srt_timestamp_to_ticks(parts[1].trim());
        let text = content[1..].join("\n");
        if let (Some(s), Some(e)) = (start, end) {
            events.push(serde_json::json!({
                "Id": events.len().to_string(),
                "Text": text,
                "StartPositionTicks": s,
                "EndPositionTicks": e,
            }));
        }
    }
    serde_json::json!({ "TrackEvents": events }).to_string()
}

fn srt_timestamp_to_ticks(ts: &str) -> Option<i64> {
    let cleaned = ts.replace(',', ".");
    let parts: Vec<&str> = cleaned
        .split(':')
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let h: i64 = parts[0]
        .parse()
        .ok()?;
    let m: i64 = parts[1]
        .parse()
        .ok()?;
    let sp: Vec<&str> = parts[2]
        .split('.')
        .collect();
    let s: i64 = sp[0]
        .parse()
        .ok()?;
    let ms: i64 = if sp.len() > 1 {
        let padded = format!("{:0<3}", sp[1]);
        padded[..3]
            .parse()
            .ok()?
    } else {
        0
    };
    Some(((h * 3600 + m * 60 + s) * 1000 + ms) * 10_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guess_media_source_from_filename_parses_full_release_tags() {
        let guess = guess_media_source_from_filename(
            "Movie.2023.2160p.UHD.BluRay.x265.10bit.HDR10.DTS-HD.MA.7.1-GROUP.mkv",
        );

        assert_eq!(guess.container, Some(api::VideoContainer::Mkv));
        assert_eq!(
            guess
                .media_streams
                .len(),
            2
        );

        let video = &guess.media_streams[0];
        assert_eq!(video.type_, Some(api::MediaStreamType::Video));
        assert_eq!(
            video
                .codec
                .as_deref(),
            Some("hevc")
        );
        assert_eq!(video.width, Some(3840));
        assert_eq!(video.height, Some(2160));
        assert_eq!(video.bit_depth, Some(10));
        assert_eq!(video.video_range_type, Some(api::VideoRangeType::Hdr10));
        assert_eq!(video.video_range, Some(api::VideoRange::Hdr));
        assert_eq!(
            video.is_default,
            Some(false),
            "a filename guess must never be flagged as the container default"
        );
        assert_eq!(
            video
                .display_title
                .as_deref(),
            Some("4K HEVC Hdr")
        );

        let audio = &guess.media_streams[1];
        assert_eq!(audio.type_, Some(api::MediaStreamType::Audio));
        assert_eq!(
            audio
                .codec
                .as_deref(),
            Some("dts")
        );
        assert_eq!(audio.channels, Some(8));
        assert_eq!(
            audio.is_default,
            Some(false),
            "a filename guess must never be flagged as the container default"
        );
        assert_eq!(
            audio
                .display_title
                .as_deref(),
            Some("DTS - 7.1")
        );
    }

    #[test]
    fn guessed_streams_never_produce_a_resolved_default_audio_index() {
        // resolve_default_streams()'s last-resort fallback picks whichever
        // stream is flagged is_default. Filename guesses must not win that
        // fallback, or DefaultAudioStreamIndex gets stamped from guesswork.
        let guess = guess_media_source_from_filename(
            "Movie.2023.1080p.WEB-DL.x264.AAC.5.1-GROUP.mp4",
        );
        let mut info = api::MediaSourceInfo {
            media_streams: guess.media_streams,
            ..Default::default()
        };
        info.resolve_default_streams(
            &remux_sdks::remux::UserConfiguration::default(),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(info.default_audio_stream_index, None);
    }

    #[test]
    fn guess_media_source_from_filename_no_technical_tags_yields_empty_streams() {
        let guess = guess_media_source_from_filename("Some Movie (2023).mkv");

        assert!(
            guess
                .media_streams
                .is_empty()
        );
    }

    #[test]
    fn guess_media_source_from_filename_no_hdr_evidence_leaves_video_range_unknown() {
        // Absence of an HDR tag doesn't mean SDR — it just means hunch found
        // nothing, so video_range must stay None (unknown), not assert Sdr.
        let guess = guess_media_source_from_filename(
            "Movie.2023.1080p.WEB-DL.x264.AAC.5.1-GROUP.mp4",
        );

        let video = &guess.media_streams[0];
        assert_eq!(video.video_range_type, None);
        assert_eq!(video.video_range, None);
    }

    #[tokio::test]
    async fn apply_filename_probe_fallback_fills_empty_source_from_filename() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let source = db::Media {
            id: uuid::Uuid::new_v4(),
            kind: db::MediaKind::Stream,
            stream_info: Some(crate::stream::StreamInfo {
                filename: Some(
                    "Movie.2023.1080p.WEB-DL.x264.AAC.5.1-GROUP.mp4".to_string(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        };
        db::Media::upsert(db, &[source.clone()])
            .await
            .unwrap();
        let mut base_item = api::BaseItemDto {
            media_sources: Some(vec![api::MediaSourceInfo::default()]),
            ..Default::default()
        };

        apply_filename_probe_fallback(&mut base_item, &[source.clone()], db).await;

        let info = &base_item
            .media_sources
            .unwrap()[0];
        assert!(
            !info
                .media_streams
                .is_empty()
        );
        assert_eq!(info.container, Some(api::VideoContainer::Mp4));
        assert_eq!(
            info.remux
                .as_ref()
                .and_then(|r| r.source),
            Some(api::ProbeOrigin::FilenameGuess)
        );

        // The guess must have been persisted, tagged, so a later request
        // doesn't re-guess and so playback-side gates can see it's not real.
        let saved = db::Media::get_by_id(db, &source.id)
            .await
            .unwrap()
            .unwrap();
        let saved_probe = saved
            .probe_data
            .expect("probe_data should be persisted");
        assert!(
            !saved_probe
                .media_streams
                .is_empty()
        );
        assert!(saved_probe.is_filename_guess());
    }

    #[tokio::test]
    async fn apply_filename_probe_fallback_never_overrides_real_probe_data() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let source = db::Media {
            id: uuid::Uuid::new_v4(),
            kind: db::MediaKind::Stream,
            stream_info: Some(crate::stream::StreamInfo {
                filename: Some(
                    "Movie.2023.1080p.WEB-DL.x264.AAC.5.1-GROUP.mp4".to_string(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        };
        let real_stream = api::MediaStream {
            type_: Some(api::MediaStreamType::Video),
            codec: Some("av1".to_string()),
            ..Default::default()
        };
        let mut base_item = api::BaseItemDto {
            media_sources: Some(vec![api::MediaSourceInfo {
                media_streams: vec![real_stream.clone()],
                ..Default::default()
            }]),
            ..Default::default()
        };

        apply_filename_probe_fallback(&mut base_item, &[source], db).await;

        let info = &base_item
            .media_sources
            .unwrap()[0];
        assert_eq!(info.media_streams, vec![real_stream]);
        assert_eq!(
            info.remux
                .as_ref()
                .and_then(|r| r.source),
            None
        );
    }

    #[tokio::test]
    async fn apply_filename_probe_fallback_skips_source_with_no_filename() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let source = db::Media {
            id: uuid::Uuid::new_v4(),
            stream_info: Some(crate::stream::StreamInfo::default()),
            ..Default::default()
        };
        let mut base_item = api::BaseItemDto {
            media_sources: Some(vec![api::MediaSourceInfo::default()]),
            ..Default::default()
        };

        apply_filename_probe_fallback(&mut base_item, &[source], db).await;

        let info = &base_item
            .media_sources
            .unwrap()[0];
        assert!(
            info.media_streams
                .is_empty()
        );
    }

    #[tokio::test]
    async fn apply_filename_probe_fallback_derives_bitrate_from_size_and_runtime() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        // 900 MB over 1 hour -> 2 Mbps.
        let source = db::Media {
            id: uuid::Uuid::new_v4(),
            kind: db::MediaKind::Stream,
            stream_info: Some(crate::stream::StreamInfo {
                size: Some(900_000_000),
                ..Default::default()
            }),
            ..Default::default()
        };
        db::Media::upsert(db, &[source.clone()])
            .await
            .unwrap();
        let mut base_item = api::BaseItemDto {
            media_sources: Some(vec![api::MediaSourceInfo {
                run_time_ticks: Some(3600 * 10_000_000),
                ..Default::default()
            }]),
            ..Default::default()
        };

        apply_filename_probe_fallback(&mut base_item, &[source.clone()], db).await;

        let info = &base_item
            .media_sources
            .unwrap()[0];
        assert_eq!(info.bitrate, Some(2_000_000));
        assert_eq!(info.size, Some(900_000_000));
        assert_eq!(
            info.remux
                .as_ref()
                .and_then(|r| r.source),
            Some(api::ProbeOrigin::FilenameGuess)
        );

        let saved = db::Media::get_by_id(db, &source.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            saved
                .probe_data
                .and_then(|p| p.bitrate),
            Some(2_000_000)
        );
    }

    #[tokio::test]
    async fn apply_filename_probe_fallback_never_overrides_real_bitrate() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let source = db::Media {
            id: uuid::Uuid::new_v4(),
            stream_info: Some(crate::stream::StreamInfo {
                size: Some(900_000_000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut base_item = api::BaseItemDto {
            media_sources: Some(vec![api::MediaSourceInfo {
                run_time_ticks: Some(3600 * 10_000_000),
                bitrate: Some(8_000_000),
                ..Default::default()
            }]),
            ..Default::default()
        };

        apply_filename_probe_fallback(&mut base_item, &[source], db).await;

        let info = &base_item
            .media_sources
            .unwrap()[0];
        assert_eq!(info.bitrate, Some(8_000_000));
    }

    #[test]
    fn srt_to_vtt_normalizes_hybrid_webvtt_timestamps() {
        let input = "WEBVTT\n\n1\n00:00:47,791 --> 00:00:49,791\nHello\n\n2\n00:00:50,000 --> 00:00:52,000 line:90%,start\nWorld\n";

        let output = srt_to_vtt(input);

        assert!(output.contains("00:00:47.791 --> 00:00:49.791"));
        assert!(output.contains("00:00:50.000 --> 00:00:52.000 line:90%,start"));
        assert!(!output.contains("00:00:47,791"));
    }

    #[test]
    fn srt_to_vtt_keeps_valid_webvtt_timestamps() {
        let input = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000 align:start\nHello\n";

        let output = srt_to_vtt(input);

        assert!(output.contains("00:00:01.000 --> 00:00:02.000 align:start"));
    }

    #[test]
    fn srt_to_vtt_converts_crlf_separated_cues() {
        let input = "1\r\n00:00:47,791 --> 00:00:49,791\r\nHello\r\n\r\n2\r\n00:00:50,000 --> 00:00:52,000\r\nWorld\r\n";

        let output = srt_to_vtt(input);

        assert!(output.contains("00:00:47.791 --> 00:00:49.791"));
        assert!(output.contains("00:00:50.000 --> 00:00:52.000"));
        assert!(!output.contains("00:00:47,791"));
    }

    #[test]
    fn srt_to_jellyfin_json_converts_crlf_separated_cues() {
        let input = "1\r\n00:00:01,000 --> 00:00:02,000\r\nHello\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nWorld\r\n";

        let output: serde_json::Value =
            serde_json::from_str(&srt_to_jellyfin_json(input)).unwrap();

        assert_eq!(
            output["TrackEvents"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(output["TrackEvents"][1]["Text"], "World");
    }
}
