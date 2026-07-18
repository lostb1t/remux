use crate::{
    addons::SubtitleInfo,
    api, common,
    common::{ToRunTimeTicks, get_uuid},
    db,
    sdks::stremio,
    stream::StreamDescriptor,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};

// Heuristic metadata fallback for remote source URLs when ffprobe metadata is
// unavailable. This keeps clients functional (stream selection/transcode
// decisions) instead of exposing empty stream lists.
fn infer_container_from_url(url: &str) -> Option<String> {
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
    match ext.as_str() {
        "matroska" | "mkv" => Some("mkv".to_string()),
        "mp4" | "m4v" | "mov" => Some("mp4".to_string()),
        "webm" => Some("webm".to_string()),
        "avi" => Some("avi".to_string()),
        "m2ts" | "ts" => Some("ts".to_string()),
        "m3u8" => Some("ts".to_string()),
        "mp3" => Some("mp3".to_string()),
        "m4a" => Some("m4a".to_string()),
        "flac" => Some("flac".to_string()),
        "ogg" | "oga" => Some("ogg".to_string()),
        "opus" => Some("opus".to_string()),
        "wav" => Some("wav".to_string()),
        "aac" => Some("aac".to_string()),
        _ => None,
    }
}

fn fallback_container_for_media_kind(kind: db::MediaKind) -> String {
    match kind {
        db::MediaKind::Track => "mp3",
        db::MediaKind::TvChannel | db::MediaKind::TvProgram => "ts",
        _ => "mp4",
    }
    .to_string()
}

fn infer_video_codec(text: &str) -> Option<String> {
    if text.contains("hevc") || text.contains("h265") || text.contains("x265") {
        Some("hevc".to_string())
    } else if text.contains("av1") {
        Some("av1".to_string())
    } else if text.contains("vp9") {
        Some("vp9".to_string())
    } else if text.contains("h264") || text.contains("x264") || text.contains("avc") {
        Some("h264".to_string())
    } else {
        None
    }
}

fn infer_audio_codec(text: &str) -> Option<String> {
    if text.contains("truehd") {
        Some("truehd".to_string())
    } else if text.contains("dts") || text.contains("dca") {
        Some("dts".to_string())
    } else if text.contains("eac3") || text.contains("ddp") {
        Some("eac3".to_string())
    } else if text.contains("ac3") {
        Some("ac3".to_string())
    } else if text.contains("aac") {
        Some("aac".to_string())
    } else {
        None
    }
}

fn infer_audio_channels(text: &str) -> Option<i64> {
    if text.contains("7.1") {
        Some(8)
    } else if text.contains("5.1") {
        Some(6)
    } else if text.contains("2.0") || text.contains("stereo") {
        Some(2)
    } else {
        None
    }
}

fn fallback_media_streams(source: &db::Media) -> Vec<api::MediaStream> {
    // Only synthesize streams for remote source entries.
    if source.kind != db::MediaKind::Stream {
        return Vec::new();
    }

    if !source.is_remote_url() {
        return Vec::new();
    }

    let text = source
        .title
        .to_ascii_lowercase();
    let video_codec = infer_video_codec(&text);
    let audio_codec = infer_audio_codec(&text);
    let channels = infer_audio_channels(&text);

    let video_title = video_codec
        .as_ref()
        .map(|c| format!("{} - Fallback", c.to_uppercase()))
        .unwrap_or_else(|| "Video - Fallback".to_string());
    let audio_title = match (&audio_codec, channels) {
        (Some(c), Some(8)) => format!("{} - 7.1 - Fallback", c.to_uppercase()),
        (Some(c), Some(6)) => format!("{} - 5.1 - Fallback", c.to_uppercase()),
        (Some(c), Some(2)) => format!("{} - Stereo - Fallback", c.to_uppercase()),
        (Some(c), _) => format!("{} - Fallback", c.to_uppercase()),
        (None, Some(8)) => "Audio - 7.1 - Fallback".to_string(),
        (None, Some(6)) => "Audio - 5.1 - Fallback".to_string(),
        (None, Some(2)) => "Audio - Stereo - Fallback".to_string(),
        (None, _) => "Audio - Fallback".to_string(),
    };

    vec![
        api::MediaStream {
            type_: Some(api::MediaStreamType::Video),
            codec: video_codec,
            is_default: Some(true),
            display_title: Some(video_title),
            ..Default::default()
        },
        api::MediaStream {
            type_: Some(api::MediaStreamType::Audio),
            codec: audio_codec,
            channels,
            is_default: Some(true),
            display_title: Some(audio_title),
            ..Default::default()
        },
    ]
}

impl From<db::Media> for api::MediaSourceInfo {
    fn from(source: db::Media) -> Self {
        // VideoType is a video concept; audio (Track) sources omit it. Captured
        // up front because `source.kind` is moved later.
        let is_track = matches!(source.kind, db::MediaKind::Track);
        let descriptor = source
            .stream_info
            .as_ref()
            .map(|si| &si.descriptor);
        let is_stub = descriptor
            .and_then(|d| d.as_http_url())
            .is_none();
        let inferred_container = descriptor
            .and_then(|d| d.as_http_url())
            .and_then(infer_container_from_url);
        let container = source
            .probe_data
            .as_ref()
            .and_then(|p| {
                p.container
                    .clone()
            })
            .or(inferred_container)
            .unwrap_or_else(|| fallback_container_for_media_kind(source.kind));

        let remux = Some(api::MediaSourceRemuxInfo {
            provider_info: source
                .stream_info
                .as_ref()
                .and_then(|si| serde_json::to_value(si).ok()),
        });

        // The file name without extension. Jellyfin labels a MediaSource with the
        // file stem (e.g. "Chief Keef - Bang - 01 - Fuck Niggas (intro)"), not the
        // track title, so reuse it for both `Path` and `Name`. Streaming sources
        // carry it in `stream_info.filename`; local tracks (no `stream_info`) stash
        // it on `probe_data.name` at probe time (see GroupLocalMusic).
        let file_stem: Option<String> = source
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
                    .map(|s| s.to_string())
            })
            .or_else(|| {
                source
                    .probe_data
                    .as_ref()
                    .and_then(|p| {
                        p.name
                            .clone()
                    })
            });
        let path = Some(match &file_stem {
            Some(s) => format!("/remux/{}/{}", source.id, s),
            None => format!("/remux/{}", source.id),
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
        // Carry overall bitrate/size through from the probe so the browse
        // MediaSource matches Jellyfin (previously dropped).
        let probe_bitrate = source
            .probe_data
            .as_ref()
            .and_then(|p| p.bitrate);
        let probe_size = source
            .probe_data
            .as_ref()
            .and_then(|p| p.size);
        let video_type = (!is_track).then_some(api::VideoType::VideoFile);
        let meta_ticks = source
            .runtime
            .and_then(|r| r.to_ticks(common::TickUnit::Seconds));
        let run_time_ticks = probe_ticks.or(meta_ticks);
        let (media_streams, default_audio_stream_index, default_subtitle_stream_index) =
            source
                .probe_data
                .map(|p| {
                    (
                        p.media_streams,
                        p.default_audio_stream_index,
                        p.default_subtitle_stream_index,
                    )
                })
                .unwrap_or_default();
        // Never emit an empty MediaStreams array. Some clients (e.g. Finamp's
        // download size estimate) call `.first` on MediaStreams without guarding
        // for empty and throw `Bad state: No element`, white-screening the
        // download dialog — whereas a null/absent array is handled safely. When a
        // track has not been probed yet, synthesize a minimal audio stream from
        // the container so the array always carries at least one element.
        let media_streams = if media_streams.is_empty() {
            vec![api::MediaStream {
                type_: Some(api::MediaStreamType::Audio),
                index: 0,
                codec: Some(container.clone()),
                is_default: Some(true),
                ..Default::default()
            }]
        } else {
            media_streams
        };
        api::MediaSourceInfo {
            id: client_id,
            e_tag: client_id,
            path,
            protocol,
            is_remote,
            name: Some(file_stem.unwrap_or_else(|| {
                source
                    .title
                    .clone()
            })),
            container: Some(container),
            bitrate: probe_bitrate,
            size: probe_size,
            video_type,
            remux,
            has_segments: !is_stub,
            formats: vec![],
            required_http_headers: HashMap::new(),
            run_time_ticks,
            media_streams,
            default_audio_stream_index,
            default_subtitle_stream_index,
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
        video_range: Some(api::VideoRange::Unknown),
        video_range_type: Some(api::VideoRangeType::Unknown),
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
    let container = stream
        .url
        .as_deref()
        .and_then(infer_container_from_url)
        .unwrap_or_else(|| "mp4".to_string());
    api::MediaSourceInfo {
        id: id.clone(),
        e_tag: id.clone(),
        path: stream.url,
        container: Some(container),
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

/// Convert SRT to WebVTT. Already-valid VTT is passed through unchanged.
pub fn srt_to_vtt(input: &str) -> String {
    let input = input.trim_start_matches('\u{FEFF}');
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
            return input[pos..]
                .trim_start_matches('\u{FEFF}')
                .to_string();
        }
        return input.to_string();
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

/// Convert SRT to Jellyfin JSON TrackEvents format (1 tick = 100 ns).
pub fn srt_to_jellyfin_json(input: &str) -> String {
    let mut events: Vec<serde_json::Value> = Vec::new();
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
mod parity_tests {
    use super::*;

    /// A local audio Track must serialize a MediaSource matching Jellyfin's
    /// audio shape: File protocol, not remote, no VideoType, and Container /
    /// Bitrate / Size carried through from the probe.
    #[test]
    fn track_media_source_matches_jellyfin_audio_shape() {
        let mut media = db::Media {
            kind: db::MediaKind::Track,
            title: "Fuck Niggas (intro)".to_string(),
            ..Default::default()
        };
        media.probe_data = Some(api::MediaSourceInfo {
            container: Some("flac".to_string()),
            bitrate: Some(1_017_103),
            size: Some(10_763_303),
            // Local tracks stash the file stem here (see GroupLocalMusic); the
            // MediaSource Name must be the file stem, not the track title.
            name: Some("Chief Keef - Bang - 01 - Fuck Niggas (intro)".to_string()),
            media_streams: vec![api::MediaStream {
                type_: Some(api::MediaStreamType::Audio),
                codec: Some("flac".to_string()),
                bit_rate: Some(1_017_103),
                ..Default::default()
            }],
            ..Default::default()
        });

        let source: api::MediaSourceInfo = media.into();

        assert_eq!(source.video_type, None, "audio must omit VideoType");
        assert!(!source.is_remote, "local source must not be remote");
        assert_eq!(source.protocol, api::MediaProtocol::File);
        assert_eq!(
            source
                .container
                .as_deref(),
            Some("flac")
        );
        assert_eq!(source.bitrate, Some(1_017_103));
        assert_eq!(source.size, Some(10_763_303));
        assert!(
            !source
                .media_streams
                .is_empty()
        );
        assert_eq!(
            source
                .name
                .as_deref(),
            Some("Chief Keef - Bang - 01 - Fuck Niggas (intro)"),
            "MediaSource Name must be the file stem, not the track title"
        );
    }

    /// The whole-double serializer must render `MediaStream.Level` as Jellyfin's
    /// `.NET` writer does: whole values with no decimal, fractional values intact.
    #[test]
    fn level_serializes_as_whole_number_like_jellyfin() {
        let stream = api::MediaStream {
            type_: Some(api::MediaStreamType::Audio),
            level: Some(0.0),
            ..Default::default()
        };
        let v = serde_json::to_value(&stream).unwrap();
        // `0.0` must serialize as `0`, matching Jellyfin byte-for-byte.
        assert_eq!(v.get("Level"), Some(&serde_json::json!(0)));

        let fractional = api::MediaStream {
            type_: Some(api::MediaStreamType::Video),
            level: Some(4.1),
            ..Default::default()
        };
        let v2 = serde_json::to_value(&fractional).unwrap();
        assert_eq!(v2.get("Level"), Some(&serde_json::json!(4.1)));
    }

    /// A video source keeps VideoType (regression guard for the Option change).
    #[test]
    fn video_source_retains_video_type() {
        let media = db::Media {
            kind: db::MediaKind::Movie,
            ..Default::default()
        };
        let source: api::MediaSourceInfo = media.into();
        assert_eq!(source.video_type, Some(api::VideoType::VideoFile));
    }
}

#[cfg(test)]
mod subtitle_conversion_tests {
    use super::*;

    #[test]
    fn srt_converts_index_and_comma_timecodes() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello world\n\n\
                   2\n00:00:05,500 --> 00:00:08,000\nSecond line\n";
        assert_eq!(
            srt_to_vtt(srt),
            "WEBVTT\n\n00:00:01.000 --> 00:00:04.000\nHello world\n\n\
             00:00:05.500 --> 00:00:08.000\nSecond line\n\n",
        );
    }

    #[test]
    fn srt_to_vtt_strips_bom_before_converting() {
        let srt = "\u{FEFF}1\n00:00:01,000 --> 00:00:02,000\nHi";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(vtt.contains("00:00:01.000 --> 00:00:02.000"));
        assert!(!vtt.contains('\u{FEFF}'));
    }

    #[test]
    fn already_webvtt_is_passed_through() {
        let already_vtt = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHi\n";
        assert_eq!(srt_to_vtt(already_vtt), already_vtt);
    }

    #[test]
    fn double_webvtt_header_keeps_only_the_real_cues() {
        // OpenSubtitles sometimes prepends a metadata WEBVTT block; the real cues
        // start at the second header.
        let doubled_header = "WEBVTT\n\nNOTE opensubtitles metadata\n\n\
                     WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nReal cue\n";
        let deduped = srt_to_vtt(doubled_header);
        assert_eq!(
            deduped,
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nReal cue\n"
        );
        assert!(!deduped.contains("opensubtitles"));
    }
}

#[cfg(test)]
mod inference_tests {
    use super::*;

    #[test]
    fn infers_container_from_url_extensions() {
        assert_eq!(
            infer_container_from_url("https://x.tld/a/b/video.mkv"),
            Some("mkv".to_string())
        );
        // Query string is stripped; extension is case-insensitive.
        assert_eq!(
            infer_container_from_url("https://x.tld/v.MP4?token=1"),
            Some("mp4".to_string())
        );
        assert_eq!(
            infer_container_from_url("https://x.tld/live/stream.m3u8"),
            Some("ts".to_string())
        );
        assert_eq!(
            infer_container_from_url("https://x.tld/clip.mov"),
            Some("mp4".to_string())
        );
        // Bare (non-URL) paths still resolve by extension.
        assert_eq!(
            infer_container_from_url("song.flac"),
            Some("flac".to_string())
        );
        // No extension / unknown extension → no guess.
        assert_eq!(infer_container_from_url("https://x.tld/noext"), None);
        assert_eq!(infer_container_from_url("https://x.tld/file.xyz"), None);
    }

    #[test]
    fn infers_video_codec_with_hevc_priority() {
        assert_eq!(
            infer_video_codec("movie.2160p.x265.mkv"),
            Some("hevc".to_string())
        );
        assert_eq!(infer_video_codec("clip h264 web"), Some("h264".to_string()));
        assert_eq!(
            infer_video_codec("something av1 test"),
            Some("av1".to_string())
        );
        // hevc is checked before the avc/h264 branch.
        assert_eq!(infer_video_codec("hevc and avc"), Some("hevc".to_string()));
        assert_eq!(infer_video_codec("mystery codec"), None);
    }

    #[test]
    fn infers_audio_codec_and_channels() {
        assert_eq!(
            infer_audio_codec("dolby truehd atmos"),
            Some("truehd".to_string())
        );
        assert_eq!(infer_audio_codec("dts-hd ma"), Some("dts".to_string()));
        assert_eq!(infer_audio_codec("ddp 5.1"), Some("eac3".to_string()));
        assert_eq!(infer_audio_codec("plain aac"), Some("aac".to_string()));
        assert_eq!(infer_audio_codec("silence"), None);

        assert_eq!(infer_audio_channels("movie 7.1 atmos"), Some(8));
        assert_eq!(infer_audio_channels("show 5.1"), Some(6));
        assert_eq!(infer_audio_channels("stereo mix"), Some(2));
        assert_eq!(infer_audio_channels("mono-ish"), None);
    }

    #[test]
    fn srt_to_jellyfin_json_emits_tick_timed_events() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello";
        let json: serde_json::Value =
            serde_json::from_str(&srt_to_jellyfin_json(srt)).unwrap();
        let events = json["TrackEvents"]
            .as_array()
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["Text"], "Hello");
        // 1 tick = 100 ns → 1 s = 10_000_000 ticks.
        assert_eq!(events[0]["StartPositionTicks"], 10_000_000_i64);
        assert_eq!(events[0]["EndPositionTicks"], 40_000_000_i64);
    }
}
