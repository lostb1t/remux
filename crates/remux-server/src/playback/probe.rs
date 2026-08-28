use crate::{
    IntoApiError, ResultExt, api,
    common::{HideConsole, TickUnit, ToRunTimeTicks},
    db,
    device_profile::{AudioCodec, SubtitleCodec, VideoCodec},
};
use anyhow::{Result, anyhow};
use isolang::Language;
use remux_sdks::remux::{MediaSegmentType, MediaSegments, Segment};
use serde::Deserialize;
use std::{collections::HashMap, str::FromStr};
use tracing::{debug, info, warn};
use uuid::Uuid;

fn ffprobe_bin() -> String {
    std::env::var("FFPROBE_PATH").unwrap_or_else(|_| "ffprobe".into())
}

fn nonzero<T: Default + PartialOrd>(v: T) -> Option<T> {
    if v > T::default() { Some(v) } else { None }
}

fn normalize_lang(code: &str) -> &str {
    match code {
        "alb" => "sqi",
        "arm" => "hye",
        "baq" => "eus",
        "bur" => "mya",
        "chi" => "zho",
        "cze" => "ces",
        "dut" => "nld",
        "fre" => "fra",
        "geo" => "kat",
        "ger" => "deu",
        "gre" => "ell",
        "ice" => "isl",
        "mac" => "mkd",
        "mao" => "mri",
        "may" => "msa",
        "per" => "fas",
        "rum" => "ron",
        "slo" => "slk",
        "tib" => "bod",
        "wel" => "cym",
        other => other,
    }
}

fn first_to_upper(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => {
            f.to_uppercase()
                .collect::<String>()
                + c.as_str()
        }
    }
}

fn video_resolution_text(width: Option<i64>, height: Option<i64>) -> Option<String> {
    match (width, height) {
        (Some(w), _) if w >= 3840 => Some("4K".into()),
        (_, Some(h)) if h >= 2160 => Some("4K".into()),
        (Some(w), _) if w >= 1920 => Some("1080p".into()),
        (_, Some(h)) if h >= 1080 => Some("1080p".into()),
        (Some(w), _) if w >= 1280 => Some("720p".into()),
        (_, Some(h)) if h >= 720 => Some("720p".into()),
        (Some(w), _) if w >= 720 => Some("480p".into()),
        (_, Some(h)) if h >= 480 => Some("480p".into()),
        (Some(_), _) | (_, Some(_)) => Some("SD".into()),
        _ => None,
    }
}

fn append_tags_to_title(title: &str, tags: &[String]) -> String {
    let mut result = title.to_string();
    for tag in tags {
        if !title
            .to_ascii_lowercase()
            .contains(&tag.to_ascii_lowercase())
        {
            result.push_str(" - ");
            result.push_str(tag);
        }
    }
    result
}

pub(crate) struct StreamMeta<'a> {
    pub language: Option<&'a str>,
    pub codec: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub channels: Option<i64>,
    pub channel_layout: Option<&'a str>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub video_range: Option<&'a api::VideoRange>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub is_hearing_impaired: bool,
    pub title: Option<&'a str>,
}

pub(crate) fn display_title_audio(m: &StreamMeta) -> Option<String> {
    let mut attrs: Vec<String> = vec![];

    if let Some(lang) = m.language {
        let special = ["und", "mis", "zxx", "mul"];
        if !special.contains(
            &lang
                .to_ascii_lowercase()
                .as_str(),
        ) {
            let name = Language::from_str(lang)
                .ok()
                .map(|l| {
                    l.to_name()
                        .to_string()
                })
                .unwrap_or_else(|| lang.to_string());
            attrs.push(first_to_upper(&name));
        }
    }

    let profile_lc = m
        .profile
        .map(|p| p.to_ascii_lowercase());
    if let Some(ref p) = profile_lc {
        if p != "lc" {
            attrs.push(
                m.profile
                    .unwrap()
                    .to_string(),
            );
        }
    } else if let Some(codec) = m.codec {
        attrs.push(
            codec
                .parse::<AudioCodec>()
                .unwrap()
                .friendly_name()
                .to_string(),
        );
    }

    if let Some(layout) = m.channel_layout {
        attrs.push(first_to_upper(layout));
    } else if let Some(ch) = m.channels {
        attrs.push(format!("{} ch", ch));
    }

    if m.is_default {
        attrs.push("Default".into());
    }
    if m.is_external {
        attrs.push("External".into());
    }

    if let Some(title) = m.title {
        return Some(append_tags_to_title(title, &attrs));
    }
    if attrs.is_empty() {
        None
    } else {
        Some(attrs.join(" - "))
    }
}

pub(crate) fn display_title_video(m: &StreamMeta) -> Option<String> {
    let mut attrs: Vec<String> = vec![];

    if let Some(res) = video_resolution_text(m.width, m.height) {
        attrs.push(res);
    }
    if let Some(codec) = m.codec {
        attrs.push(codec.to_ascii_uppercase());
    }
    if let Some(range) = m.video_range {
        if *range != api::VideoRange::Other {
            attrs.push(format!("{:?}", range));
        }
    }

    if let Some(title) = m.title {
        return Some(append_tags_to_title(title, &attrs));
    }
    if attrs.is_empty() {
        None
    } else {
        Some(attrs.join(" "))
    }
}

pub(crate) fn display_title_subtitle(m: &StreamMeta) -> Option<String> {
    let mut attrs: Vec<String> = vec![];

    if let Some(lang) = m.language {
        let name = Language::from_str(lang)
            .ok()
            .map(|l| {
                l.to_name()
                    .to_string()
            })
            .unwrap_or_else(|| lang.to_string());
        attrs.push(first_to_upper(&name));
    } else {
        attrs.push("Und".into());
    }

    if m.is_hearing_impaired {
        attrs.push("Hearing Impaired".into());
    }
    if m.is_forced {
        attrs.push("Forced".into());
    }
    if let Some(codec) = m.codec {
        let display = codec
            .parse::<SubtitleCodec>()
            .map(|c| c.to_string())
            .unwrap_or_else(|_| codec.to_string());
        attrs.push(display.to_ascii_uppercase());
    }
    if m.is_external {
        attrs.push("External".into());
    }

    if attrs.is_empty() {
        None
    } else {
        Some(attrs.join(" - "))
    }
}

#[derive(Deserialize)]
struct FfprobeChapter {
    start_time: String,
    end_time: String,
    #[serde(default)]
    tags: HashMap<String, String>,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: FfprobeFormat,
    #[serde(default)]
    chapters: Vec<FfprobeChapter>,
}

#[derive(Deserialize, Default)]
struct FfprobeDisposition {
    #[serde(default)]
    default: i64,
    #[serde(default)]
    forced: i64,
    #[serde(default)]
    hearing_impaired: i64,
    #[serde(default)]
    attached_pic: i64,
}

#[derive(Deserialize)]
struct FfprobeStream {
    index: i64,
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_tag_string: Option<String>,
    profile: Option<String>,
    level: Option<f64>,
    width: Option<i64>,
    height: Option<i64>,
    bit_rate: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    color_transfer: Option<String>,
    channels: Option<i64>,
    channel_layout: Option<String>,
    sample_rate: Option<String>,
    pix_fmt: Option<String>,
    bits_per_raw_sample: Option<String>,
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    disposition: FfprobeDisposition,
}

/// Derive bit depth from a pixel format string (e.g. "yuv420p10le" → 10, "yuv420p" → 8).
fn bit_depth_from_pix_fmt(pix_fmt: &str) -> Option<i64> {
    // Check for explicit bit-depth suffixes: 9, 10, 12, 14, 16
    for depth in [16u8, 14, 12, 10, 9] {
        if pix_fmt.contains(&depth.to_string()) {
            return Some(depth as i64);
        }
    }
    // Standard 8-bit formats have no numeric suffix
    if !pix_fmt.is_empty() { Some(8) } else { None }
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    format_name: Option<String>,
    bit_rate: Option<String>,
    size: Option<String>,
}

fn parse_frame_rate(s: &str) -> Option<f64> {
    let mut parts = s.splitn(2, '/');
    let num: f64 = parts
        .next()?
        .parse()
        .ok()?;
    let den: f64 = parts
        .next()
        .unwrap_or("1")
        .parse()
        .ok()?;
    if den == 0.0 {
        return None;
    }
    let fps = num / den;
    if fps > 0.0 { Some(fps) } else { None }
}

fn chapter_title_to_type(title: &str) -> Option<MediaSegmentType> {
    let t = title.to_ascii_lowercase();
    if t.contains("intro") {
        Some(MediaSegmentType::Intro)
    } else if t.contains("recap") {
        Some(MediaSegmentType::Recap)
    } else if t.contains("credits") || t.contains("outro") {
        Some(MediaSegmentType::Outro)
    } else if t.contains("preview") {
        Some(MediaSegmentType::Preview)
    } else if t.contains("commercial") || t.contains(" ad ") || t.eq("ad") {
        Some(MediaSegmentType::Commercial)
    } else {
        None
    }
}

fn chapters_to_segments(chapters: &[FfprobeChapter]) -> MediaSegments {
    let mut segs = MediaSegments::default();
    for ch in chapters {
        let title = ch
            .tags
            .get("title")
            .map(|s| s.as_str())
            .unwrap_or("");
        let Some(kind) = chapter_title_to_type(title) else {
            continue;
        };
        let Some(start) = ch
            .start_time
            .to_ticks(TickUnit::Seconds)
        else {
            continue;
        };
        let Some(end) = ch
            .end_time
            .to_ticks(TickUnit::Seconds)
        else {
            continue;
        };
        let seg = Segment {
            start_ticks: start,
            end_ticks: end,
        };
        match kind {
            MediaSegmentType::Intro
                if segs
                    .intro
                    .is_none() =>
            {
                segs.intro = Some(seg)
            }
            MediaSegmentType::Outro
                if segs
                    .outro
                    .is_none() =>
            {
                segs.outro = Some(seg)
            }
            MediaSegmentType::Recap
                if segs
                    .recap
                    .is_none() =>
            {
                segs.recap = Some(seg)
            }
            MediaSegmentType::Preview
                if segs
                    .preview
                    .is_none() =>
            {
                segs.preview = Some(seg)
            }
            MediaSegmentType::Commercial
                if segs
                    .commercial
                    .is_none() =>
            {
                segs.commercial = Some(seg)
            }
            _ => {}
        }
    }
    segs
}

/// When a single video stream has no per-stream bitrate but all audio streams do,
/// estimate it as container total minus summed audio bitrates (mirrors Jellyfin).
fn apply_video_bitrate_fallback(
    streams: &mut Vec<api::MediaStream>,
    total_bitrate: Option<i64>,
) {
    let Some(total) = total_bitrate else { return };
    let video_indices: Vec<usize> = streams
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            matches!(s.type_, Some(api::MediaStreamType::Video))
                && s.bit_rate
                    .is_none()
        })
        .map(|(i, _)| i)
        .collect();
    if video_indices.len() != 1 {
        return;
    }
    // Mirror Jellyfin: subtract all non-video, non-external streams (not just audio).
    // audioBitratesKnown check: all audio streams must have a known bitrate.
    let non_video: Vec<&api::MediaStream> = streams
        .iter()
        .filter(|s| {
            !matches!(s.type_, Some(api::MediaStreamType::Video)) && !s.is_external
        })
        .collect();
    let audio_all_known = non_video
        .iter()
        .filter(|s| matches!(s.type_, Some(api::MediaStreamType::Audio)))
        .all(|s| {
            s.bit_rate
                .is_some()
        });
    if audio_all_known {
        let other_sum: i64 = non_video
            .iter()
            .map(|s| {
                s.bit_rate
                    .unwrap_or(0)
            })
            .sum();
        let estimated = total - other_sum;
        if estimated > 0 {
            streams[video_indices[0]].bit_rate = Some(estimated);
        }
    }
}

/// Probe a media URL with ffprobe and return a Jellyfin `MediaSourceInfo`
/// alongside any chapter-derived `MediaSegments`.
pub fn probe_media(url: &str) -> Result<(api::MediaSourceInfo, MediaSegments)> {
    debug!(url, "probing media");

    let output = std::process::Command::new(ffprobe_bin())
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_chapters",
            "-show_streams",
            "-show_format",
            url,
        ])
        .hide_console()
        .output()
        .map_err(|e| anyhow!("Failed to run ffprobe: {}", e))?;

    if !output
        .status
        .success()
    {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffprobe failed for {}: {}", url, stderr));
    }

    let probe: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow!("Failed to parse ffprobe output: {}", e))?;

    let run_time_ticks = probe
        .format
        .duration
        .as_deref()
        .and_then(|s| {
            s.parse::<f64>()
                .ok()
        })
        .map(|secs| (secs * 1_000_000.0) as i64) // → µs
        .and_then(nonzero)
        .map(|us| us * 10); // µs → 100ns ticks

    let container = probe
        .format
        .format_name
        .as_deref()
        .map(remux_utils::normalize_container);

    let overall_bitrate = probe
        .format
        .bit_rate
        .as_deref()
        .and_then(|s| {
            s.parse::<i64>()
                .ok()
        })
        .and_then(nonzero);

    let file_size = probe
        .format
        .size
        .as_deref()
        .and_then(|s| {
            s.parse::<i64>()
                .ok()
        })
        .and_then(nonzero);

    debug!(?run_time_ticks, ?container, "probe container info");

    let mut streams: Vec<api::MediaStream> = Vec::new();
    let mut video_idx: i64 = 0;
    let mut audio_idx: i64 = 0;
    let mut sub_idx: i64 = 0;

    for s in &probe.streams {
        let codec_type = s
            .codec_type
            .as_deref()
            .unwrap_or("");
        let language = s
            .tags
            .get("language")
            .map(|s| normalize_lang(s.as_str()));
        let title = s
            .tags
            .get("title")
            .cloned();

        match codec_type {
            "video" => {
                // Skip attached pictures (embedded cover art). They are not playable
                // video streams, and having two Type:Video entries confuses clients
                // that look for the primary video stream.
                if s.disposition
                    .attached_pic
                    != 0
                {
                    continue;
                }
                let bitrate = s
                    .bit_rate
                    .as_deref()
                    .and_then(|b| {
                        b.parse::<i64>()
                            .ok()
                    })
                    .and_then(nonzero);
                // Prefer r_frame_rate (exact) then avg_frame_rate for display/segment math.
                let fps = s
                    .r_frame_rate
                    .as_deref()
                    .and_then(parse_frame_rate)
                    .or_else(|| {
                        s.avg_frame_rate
                            .as_deref()
                            .and_then(parse_frame_rate)
                    });
                let raw_codec = s
                    .codec_name
                    .clone()
                    .unwrap_or_default();
                let codec = raw_codec
                    .parse::<VideoCodec>()
                    .unwrap()
                    .to_string();
                let is_default = video_idx == 0;
                let is_forced = s
                    .disposition
                    .forced
                    != 0;

                // Detect HDR type from color_transfer reported by ffprobe.
                let (video_range, video_range_type) = match s
                    .color_transfer
                    .as_deref()
                {
                    Some("smpte2084") => {
                        (api::VideoRange::Hdr, api::VideoRangeType::Hdr10)
                    }
                    Some("arib-std-b67") => {
                        (api::VideoRange::Hdr, api::VideoRangeType::Hlg)
                    }
                    _ => (api::VideoRange::Sdr, api::VideoRangeType::Sdr),
                };

                let meta = StreamMeta {
                    language,
                    codec: Some(&codec),
                    profile: s
                        .profile
                        .as_deref(),
                    channels: None,
                    channel_layout: None,
                    width: s
                        .width
                        .and_then(nonzero),
                    height: s
                        .height
                        .and_then(nonzero),
                    video_range: Some(&video_range),
                    is_default,
                    is_forced,
                    is_external: false,
                    is_hearing_impaired: false,
                    title: title.as_deref(),
                };

                let bit_depth = s
                    .bits_per_raw_sample
                    .as_deref()
                    .and_then(|b| {
                        b.parse::<i64>()
                            .ok()
                    })
                    .filter(|&b| b > 0)
                    .or_else(|| {
                        s.pix_fmt
                            .as_deref()
                            .and_then(bit_depth_from_pix_fmt)
                    });
                streams.push(api::MediaStream {
                    type_: Some(api::MediaStreamType::Video),
                    index: s.index,
                    codec: Some(codec.clone()),
                    codec_tag: s
                        .codec_tag_string
                        .clone(),
                    profile: s
                        .profile
                        .clone(),
                    level: s.level,
                    width: meta.width,
                    height: meta.height,
                    bit_rate: bitrate,
                    bit_depth,
                    average_frame_rate: fps
                        .map(|f| f as f32)
                        .and_then(nonzero),
                    real_frame_rate: fps
                        .map(|f| f as f32)
                        .and_then(nonzero),
                    is_default: Some(is_default),
                    is_forced,
                    is_avc: Some(false),
                    time_base: Some("1/1000".to_string()),
                    audio_spatial_format: Some("None".to_string()),
                    video_range: Some(video_range),
                    video_range_type: Some(video_range_type),
                    display_title: display_title_video(&meta),
                    language: language.map(str::to_string),
                    title,
                    ..Default::default()
                });
                video_idx += 1;
            }
            "audio" => {
                let bitrate = s
                    .bit_rate
                    .as_deref()
                    .and_then(|b| {
                        b.parse::<i64>()
                            .ok()
                    })
                    .and_then(nonzero);
                let channels = s
                    .channels
                    .and_then(nonzero);
                let sample_rate = s
                    .sample_rate
                    .as_deref()
                    .and_then(|sr| {
                        sr.parse::<i64>()
                            .ok()
                    })
                    .and_then(nonzero);
                let raw_codec = s
                    .codec_name
                    .clone()
                    .unwrap_or_default();
                let codec = raw_codec
                    .parse::<AudioCodec>()
                    .unwrap()
                    .to_string();
                // Only the container's real disposition flag counts as a default;
                // no first-of-type synthesis (an unflagged track has no default).
                let is_default = s
                    .disposition
                    .default
                    != 0;
                let is_forced = s
                    .disposition
                    .forced
                    != 0;
                let channel_layout = s
                    .channel_layout
                    .as_deref();

                let meta = StreamMeta {
                    language,
                    codec: Some(&codec),
                    profile: s
                        .profile
                        .as_deref(),
                    channels,
                    channel_layout,
                    width: None,
                    height: None,
                    video_range: None,
                    is_default,
                    is_forced,
                    is_external: false,
                    is_hearing_impaired: false,
                    title: title.as_deref(),
                };

                streams.push(api::MediaStream {
                    type_: Some(api::MediaStreamType::Audio),
                    index: s.index,
                    codec: Some(codec.clone()),
                    channels,
                    channel_layout: channel_layout.map(str::to_string),
                    sample_rate,
                    bit_rate: bitrate,
                    is_default: Some(is_default),
                    is_forced,
                    is_avc: Some(false),
                    time_base: Some("1/1000".to_string()),
                    video_range: Some(api::VideoRange::Other),
                    video_range_type: Some(api::VideoRangeType::Other),
                    audio_spatial_format: Some("None".to_string()),
                    localized_default: Some("Default".to_string()),
                    localized_external: Some("External".to_string()),
                    display_title: display_title_audio(&meta),
                    language: language.map(str::to_string),
                    title,
                    ..Default::default()
                });
                audio_idx += 1;
            }
            "subtitle" => {
                let raw_codec = s
                    .codec_name
                    .clone()
                    .unwrap_or_default();
                let parsed_codec = raw_codec
                    .parse::<SubtitleCodec>()
                    .ok();
                let codec = parsed_codec
                    .as_ref()
                    .map(|c| c.to_string())
                    .unwrap_or(raw_codec);
                let is_image = parsed_codec
                    .as_ref()
                    .map(SubtitleCodec::is_image)
                    .unwrap_or(false);
                let delivery_method = if is_image {
                    Some(api::SubtitleDeliveryMethod::Embed)
                } else {
                    None
                };
                // Only the container's real disposition flag counts as a default;
                // no first-of-type synthesis (an unflagged track has no default).
                let is_default = s
                    .disposition
                    .default
                    != 0;
                let is_forced = s
                    .disposition
                    .forced
                    != 0;
                let is_hearing_impaired = s
                    .disposition
                    .hearing_impaired
                    != 0;

                let meta = StreamMeta {
                    language,
                    codec: Some(&codec),
                    profile: None,
                    channels: None,
                    channel_layout: None,
                    width: None,
                    height: None,
                    video_range: None,
                    is_default,
                    is_forced,
                    is_external: false,
                    is_hearing_impaired,
                    title: None, // don't use raw stream title; build purely from attributes
                };

                let mut stream = api::MediaStream {
                    type_: Some(api::MediaStreamType::Subtitle),
                    index: s.index,
                    codec: Some(codec.clone()),
                    is_default: Some(is_default),
                    is_forced,
                    is_hearing_impaired,
                    is_avc: Some(false),
                    time_base: Some("1/1000".to_string()),
                    video_range: Some(api::VideoRange::Other),
                    video_range_type: Some(api::VideoRangeType::Other),
                    audio_spatial_format: Some("None".to_string()),
                    localized_undefined: Some("Undefined".to_string()),
                    localized_default: Some("Default".to_string()),
                    localized_forced: Some("Forced".to_string()),
                    localized_external: Some("External".to_string()),
                    localized_hearing_impaired: Some("Hearing Impaired".to_string()),
                    display_title: display_title_subtitle(&meta),
                    language: language.map(str::to_string),
                    title,
                    supports_external_stream: true,
                    delivery_method,
                    ..Default::default()
                };
                stream.is_text_subtitle_stream = stream.is_text_subtitle_stream();
                streams.push(stream);
                sub_idx += 1;
            }
            _ => {}
        }
    }

    apply_video_bitrate_fallback(&mut streams, overall_bitrate);

    // NOTE: `default_audio_stream_index` / `default_subtitle_stream_index` are
    // deliberately NOT computed here. They are derived, per-request API values
    // (see `MediaSourceInfo::resolve_default_streams`) and must not be persisted
    // in probe data. The per-stream `is_default` flags above are the container
    // facts that derivation builds on.

    let segments = chapters_to_segments(&probe.chapters);

    Ok((
        api::MediaSourceInfo {
            media_streams: streams,
            container,
            run_time_ticks,
            bitrate: overall_bitrate,
            size: file_size,
            ..Default::default()
        },
        segments,
    ))
}

/// Returns the top-level Movie/Episode/Track to use for stream enumeration.
///
/// When `media_source_id` points to a Stream child record, `resolver.stream`
/// resolves to that child (kind=Stream), not the parent episode. Enumerating
/// streams from a Stream record yields only 1 result — the bug that produced
/// "tried 1 of 1 streams" even though the episode had many. This function
/// always returns a top-level item, loading by `id` when `media` is a child.
pub(crate) async fn resolve_stream_root(
    media: &db::Media,
    id: Uuid,
    db: &sqlx::SqlitePool,
) -> db::Media {
    if matches!(
        media.kind,
        db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
    ) {
        media.clone()
    } else {
        db::Media::get_by_id(db, &id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| media.clone())
    }
}

/// Resolve probe data for a single source: cache hit → skip → live probe with fallback.
/// Uncached P2P sources return inferred stream metadata immediately so
/// `PlaybackInfo` is not blocked waiting for torrent pieces; ordinary network
/// sources still use authoritative probing for codec decisions.
pub(crate) async fn probe_stream(
    stream: &db::Media,
    url_opt: Option<String>,
    skip_probe: bool,
    timeout_secs: u64,
    auto_next_stream: bool,
    max_retries: usize,
    probe_pool: &[db::Media],
    restrict_resolution: bool,
    port: u16,
    db: &sqlx::SqlitePool,
) -> axum_anyhow::ApiResult<(api::MediaSourceInfo, db::Media)> {
    if skip_probe {
        return Ok((api::MediaSourceInfo::from(stream.clone()), stream.clone()));
    }
    if let Some(cached) = &stream.probe_data {
        if cached
            .video_stream()
            .is_some()
        {
            let alive = match stream
                .stream_info
                .as_ref()
                .map(|si| &si.descriptor)
            {
                Some(d) => {
                    d.is_alive()
                        .await
                }
                None => true,
            };
            if alive {
                debug!(id = %stream.id, "probe cache hit, url alive");
                let mut info = api::MediaSourceInfo::from(stream.clone());
                apply_video_bitrate_fallback(&mut info.media_streams, info.bitrate);
                return Ok((info, stream.clone()));
            }
            debug!(id = %stream.id, "probe cache hit but url dead, falling through to fallback");
        } else {
            debug!(id = %stream.id, "probe cache stale (no video stream), re-probing");
        }
    } else if stream
        .stream_info
        .as_ref()
        .is_some_and(|stream_info| stream_info.is_p2p())
    {
        // Probing a torrent URL requests media pieces. PlaybackInfo and version
        // dropdowns must remain metadata-only, so defer authoritative probing
        // until the playback-owned HLS path starts the selected source.
        debug!(id = %stream.id, "uncached P2P source — returning inferred metadata without downloading pieces");
        return Ok((api::MediaSourceInfo::from(stream.clone()), stream.clone()));
    }
    probe_with_fallback(
        stream.clone(),
        url_opt,
        timeout_secs,
        auto_next_stream,
        max_retries,
        probe_pool,
        restrict_resolution,
        port,
        db,
        |url| probe_media(&url),
    )
    .await
}

fn select_candidates(
    primary: &db::Media,
    probe_pool: &[db::Media],
    auto_next_stream: bool,
    max_retries: usize,
    restrict_resolution: bool,
    port: u16,
) -> Vec<(db::Media, String)> {
    if !auto_next_stream {
        return vec![];
    }
    let pri_p2p = primary
        .stream_info
        .as_ref()
        .map_or(false, |si| si.is_p2p());
    let pri_res = primary
        .stream_info
        .as_ref()
        .and_then(|si| si.resolution_tag());
    probe_pool
        .iter()
        .filter(|c| {
            if c.id == primary.id {
                return false;
            }
            let c_p2p = c
                .stream_info
                .as_ref()
                .map_or(false, |si| si.is_p2p());
            if c_p2p != pri_p2p {
                return false;
            }
            if restrict_resolution {
                let c_res = c
                    .stream_info
                    .as_ref()
                    .and_then(|si| si.resolution_tag());
                if c_res != pri_res {
                    return false;
                }
            }
            true
        })
        // In group-cascade mode (restrict_resolution=false) try all candidates;
        // otherwise honour the configured retry cap.
        .take(if restrict_resolution {
            max_retries
        } else {
            usize::MAX
        })
        .filter_map(|c| {
            let url = c
                .stream_info
                .as_ref()?
                .descriptor
                .server_input(c.id, port);
            Some((c.clone(), url))
        })
        .collect()
}

fn probe_attempt_timeout(primary: &db::Media, configured_secs: u64) -> u64 {
    if primary
        .stream_info
        .as_ref()
        .is_some_and(|info| info.is_p2p())
    {
        configured_secs.min(30)
    } else {
        configured_secs
    }
}

fn probe_overall_deadline(
    primary: &db::Media,
    attempt_timeout_secs: u64,
    candidate_count: usize,
) -> Option<std::time::Duration> {
    primary
        .stream_info
        .as_ref()
        .is_some_and(|info| info.is_p2p())
        .then(|| {
            std::time::Duration::from_secs(
                attempt_timeout_secs * (candidate_count as u64).min(4) + 5,
            )
        })
}

/// Probe a stream URL, retrying with the next matching candidate on failure.
///
/// Returns a 500 error if all candidates fail to probe.
///
/// Probes are sequential, but both the per-source timeout and total fallback
/// duration are bounded so a large candidate pool cannot multiply startup
/// latency without limit.
async fn probe_with_fallback<F>(
    primary: db::Media,
    primary_url: Option<String>,
    timeout_secs: u64,
    auto_next_stream: bool,
    max_retries: usize,
    probe_pool: &[db::Media],
    restrict_resolution: bool,
    port: u16,
    db: &sqlx::SqlitePool,
    probe_fn: F,
) -> axum_anyhow::ApiResult<(api::MediaSourceInfo, db::Media)>
where
    F: Fn(
            String,
        )
            -> anyhow::Result<(api::MediaSourceInfo, remux_sdks::remux::MediaSegments)>
        + Clone
        + Send
        + 'static,
{
    let candidates = select_candidates(
        &primary,
        probe_pool,
        auto_next_stream,
        max_retries,
        restrict_resolution,
        port,
    );

    let all_to_try: Vec<_> = std::iter::once((primary.clone(), primary_url))
        .chain(
            candidates
                .into_iter()
                .map(|(m, u)| (m, Some(u))),
        )
        .collect();
    let total_available = probe_pool.len();
    let mut attempts = 0usize;
    // Bound P2P probing so one unreachable swarm cannot dominate the fallback
    // sequence. Local and HTTP/debrid sources retain the administrator's
    // configured timeout.
    let effective_timeout = probe_attempt_timeout(&primary, timeout_secs);
    // P2P fallback probing is bounded across the candidate pool. Direct HTTP,
    // debrid, and local sources retain their existing fallback behavior.
    let overall_deadline =
        probe_overall_deadline(&primary, effective_timeout, all_to_try.len());
    let overall_start = std::time::Instant::now();

    for (stream, url_opt) in all_to_try {
        if overall_deadline.is_some_and(|deadline| overall_start.elapsed() >= deadline)
        {
            warn!(
                timeout = ?overall_deadline,
                attempts, "probe overall deadline exceeded"
            );
            break;
        }
        let is_retry = stream.id != primary.id;
        let url = match url_opt {
            Some(u) => u,
            None => {
                warn!(id = %stream.id, "skipping stream with no URL");
                continue;
            }
        };
        attempts += 1;
        if is_retry {
            debug!(
                failed_id = %primary.id,
                next_id = %stream.id,
                next_url = %url,
                "probe failed, trying next matching stream"
            );
        }
        let url2 = url.clone();
        let stream2 = stream.clone();
        let db2 = db.clone();
        let f = probe_fn.clone();
        let probe_result = tokio::time::timeout(
            std::time::Duration::from_secs(effective_timeout),
            tokio::task::spawn_blocking(move || f(url2)),
        )
        .await;

        match probe_result {
            Ok(Ok(Ok((mut probed, segments)))) => {
                // Reject streams whose probed duration is suspiciously short
                // relative to the known metadata runtime (or absolutely < 3 min
                // when unknown) — these are typically error/copyright-strike
                // placeholder videos, not real content. Skip for audio-only
                // streams since short songs are legitimate.
                if probed
                    .video_stream()
                    .is_some()
                    && let Some(probed_ticks) = probed.run_time_ticks
                {
                    let max_threshold = 5_i64
                        .to_ticks(TickUnit::Minutes)
                        .unwrap_or(0);
                    let threshold_ticks = match stream.runtime {
                        Some(known_secs) => known_secs
                            .to_ticks(TickUnit::Seconds)
                            .map(|t| (t / 2).min(max_threshold))
                            .unwrap_or(max_threshold),
                        None => 3_i64
                            .to_ticks(TickUnit::Minutes)
                            .unwrap_or(0),
                    };
                    if probed_ticks < threshold_ticks {
                        warn!(
                            id = %stream.id,
                            url = %url,
                            probed_ticks,
                            threshold_ticks,
                            known_runtime_secs = ?stream.runtime,
                            "stream is suspiciously short, treating as probe failure"
                        );
                        continue;
                    }
                }

                if probed
                    .video_stream()
                    .is_some()
                    || probed
                        .audio_stream()
                        .is_some()
                {
                    if !segments.is_empty() {
                        probed.segments = Some(segments);
                    }
                    if let Err(e) =
                        db::Media::save_probe_data(&db2, &stream2.id, &probed).await
                    {
                        warn!(id = %stream2.id, error = %e, "failed to save probe data");
                    }
                } else {
                    warn!(id = %stream2.id, "probe returned no audio or video stream, not caching");
                }
                if is_retry {
                    info!(
                        fallback_url = %url,
                        attempt = attempts,
                        "probe succeeded on fallback stream"
                    );
                }
                return Ok((probed, stream));
            }
            Ok(Ok(Err(e))) => {
                warn!(url = %url, error = %e, "probe failed");
            }
            Ok(Err(e)) => {
                warn!(url = %url, error = %e, "probe task panicked");
            }
            Err(_) => {
                warn!(url = %url, timeout = effective_timeout, "probe timed out");
            }
        }
    }

    Err(anyhow!(
        "all probe attempts failed for '{}' (tried {attempts} of {total_available} streams)",
        primary.full_title()
    ))
    .context_internal("stream probe failed — no usable streams found")
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    #[test]
    fn bit_depth_from_pix_fmt_known_formats() {
        assert_eq!(bit_depth_from_pix_fmt("yuv420p10le"), Some(10));
        assert_eq!(bit_depth_from_pix_fmt("yuv420p12le"), Some(12));
        assert_eq!(bit_depth_from_pix_fmt("yuv444p16le"), Some(16));
        assert_eq!(bit_depth_from_pix_fmt("yuv420p"), Some(8));
        assert_eq!(bit_depth_from_pix_fmt("yuvj420p"), Some(8));
        assert_eq!(bit_depth_from_pix_fmt(""), None);
    }
    use crate::stream::{StreamDescriptor, StreamInfo};
    use remux_sdks::remux::{MediaSegments, MediaStream, MediaStreamType};
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    fn http_media(url: &str) -> db::Media {
        db::Media {
            stream_info: Some(StreamInfo {
                descriptor: StreamDescriptor::http(url),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn http_media_with_filename(url: &str, filename: &str) -> db::Media {
        db::Media {
            stream_info: Some(StreamInfo {
                descriptor: StreamDescriptor::http(url),
                filename: Some(filename.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn p2p_media() -> db::Media {
        db::Media {
            stream_info: Some(StreamInfo {
                descriptor: StreamDescriptor::Torrent {
                    info_hash: "abc123".to_string(),
                    file_hint: None,
                    file_idx: None,
                    trackers: vec![],
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn video_probe() -> anyhow::Result<(api::MediaSourceInfo, MediaSegments)> {
        Ok((
            api::MediaSourceInfo {
                media_streams: vec![MediaStream {
                    type_: Some(MediaStreamType::Video),
                    ..Default::default()
                }],
                ..Default::default()
            },
            Default::default(),
        ))
    }

    fn queued_probe(
        results: Vec<anyhow::Result<(api::MediaSourceInfo, MediaSegments)>>,
    ) -> impl Fn(String) -> anyhow::Result<(api::MediaSourceInfo, MediaSegments)>
    + Clone
    + Send
    + 'static {
        let q = Arc::new(Mutex::new(VecDeque::from(results)));
        move |_url: String| {
            q.lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("no more probe results")))
        }
    }

    async fn test_db() -> sqlx::SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 5_000)
            .await
            .unwrap();
        crate::db::migrate(&db)
            .await
            .unwrap();
        db
    }

    #[test]
    fn no_fallback_when_auto_next_stream_false() {
        let primary = http_media("http://a.example.com");
        let other = http_media("http://b.example.com");
        let result = select_candidates(
            &primary,
            &[primary.clone(), other],
            false,
            3,
            false,
            3000,
        );
        assert!(result.is_empty());
    }

    #[test]
    fn excludes_primary_from_candidates() {
        let primary = http_media("http://a.example.com");
        let other = http_media("http://b.example.com");
        let all = vec![primary.clone(), other];
        let result = select_candidates(&primary, &all, true, 10, false, 3000);
        assert!(
            result
                .iter()
                .all(|(m, _)| m.id != primary.id)
        );
    }

    #[test]
    fn p2p_mismatch_excluded() {
        let primary = http_media("http://a.example.com");
        let p2p = p2p_media();
        let all = vec![primary.clone(), p2p];
        let result = select_candidates(&primary, &all, true, 10, false, 3000);
        assert!(result.is_empty());
    }

    #[test]
    fn probe_timeout_cap_does_not_override_non_p2p_configuration() {
        assert_eq!(
            probe_attempt_timeout(&http_media("http://example.com"), 75),
            75
        );
        assert_eq!(probe_attempt_timeout(&p2p_media(), 75), 30);
        assert_eq!(
            probe_overall_deadline(&http_media("http://example.com"), 75, 10),
            None
        );
        assert_eq!(
            probe_overall_deadline(&p2p_media(), 30, 10),
            Some(std::time::Duration::from_secs(125))
        );
    }

    #[test]
    fn resolution_mismatch_excluded_when_restricted() {
        let primary =
            http_media_with_filename("http://a.example.com", "Movie.1080p.BluRay.mkv");
        let other =
            http_media_with_filename("http://b.example.com", "Movie.720p.BluRay.mkv");
        let all = vec![primary.clone(), other];
        let result = select_candidates(&primary, &all, true, 10, true, 3000);
        assert!(result.is_empty());
    }

    #[test]
    fn resolution_mismatch_allowed_in_cascade() {
        let primary =
            http_media_with_filename("http://a.example.com", "Movie.1080p.BluRay.mkv");
        let other =
            http_media_with_filename("http://b.example.com", "Movie.720p.BluRay.mkv");
        let all = vec![primary.clone(), other];
        let result = select_candidates(&primary, &all, true, 10, false, 3000);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn max_retries_caps_result() {
        let primary = http_media("http://a.example.com");
        let others: Vec<_> = (0..5)
            .map(|i| http_media(&format!("http://s{i}.example.com")))
            .collect();
        let all: Vec<_> = std::iter::once(primary.clone())
            .chain(others)
            .collect();
        let result = select_candidates(&primary, &all, true, 2, true, 3000);
        assert!(result.len() <= 2);
    }

    #[test]
    fn cascade_mode_ignores_retry_cap() {
        let primary = http_media("http://a.example.com");
        let others: Vec<_> = (0..5)
            .map(|i| http_media(&format!("http://s{i}.example.com")))
            .collect();
        let all: Vec<_> = std::iter::once(primary.clone())
            .chain(others)
            .collect();
        let result = select_candidates(&primary, &all, true, 1, false, 3000);
        assert_eq!(result.len(), 5);
    }

    #[tokio::test]
    async fn primary_success() {
        let db = test_db().await;
        let primary = http_media("http://a.example.com");
        let primary_id = primary.id;
        let all = vec![primary.clone()];
        let result = probe_with_fallback(
            primary,
            Some("http://a.example.com".to_string()),
            10,
            false,
            0,
            &all,
            false,
            3000,
            &db,
            queued_probe(vec![video_probe()]),
        )
        .await;
        let (_, effective) = result.unwrap();
        assert_eq!(
            effective.id, primary_id,
            "primary succeeded — effective stream must be the primary"
        );
    }

    #[tokio::test]
    async fn primary_fails_fallback_succeeds() {
        let db = test_db().await;
        let primary = http_media("http://a.example.com");
        let fallback = http_media("http://b.example.com");
        let fallback_id = fallback.id;
        let all = vec![primary.clone(), fallback];
        let result = probe_with_fallback(
            primary,
            Some("http://a.example.com".to_string()),
            10,
            true,
            5,
            &all,
            false,
            3000,
            &db,
            queued_probe(vec![Err(anyhow!("primary failed")), video_probe()]),
        )
        .await;
        let (_, effective) = result.unwrap();
        assert_eq!(
            effective.id, fallback_id,
            "fallback succeeded — effective stream must be the fallback, not the primary"
        );
    }

    #[tokio::test]
    async fn all_fail_returns_error_with_counts() {
        let db = test_db().await;
        let primary = http_media("http://a.example.com");
        let fallback = http_media("http://b.example.com");
        let all = vec![primary.clone(), fallback];
        let result = probe_with_fallback(
            primary,
            Some("http://a.example.com".to_string()),
            10,
            true,
            5,
            &all,
            false,
            3000,
            &db,
            queued_probe(vec![Err(anyhow!("fail")), Err(anyhow!("fail"))]),
        )
        .await;
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("tried 2 of 2 streams"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn short_duration_skips_to_next() {
        let db = test_db().await;
        // 1 minute — well below the 3-minute threshold when runtime is unknown
        let short_ticks = 60_i64 * 10_000_000;
        let short_probe = Ok((
            api::MediaSourceInfo {
                run_time_ticks: Some(short_ticks),
                media_streams: vec![MediaStream {
                    type_: Some(MediaStreamType::Video),
                    ..Default::default()
                }],
                ..Default::default()
            },
            MediaSegments::default(),
        ));
        let primary = http_media("http://a.example.com");
        let fallback = http_media("http://b.example.com");
        let fallback_id = fallback.id;
        let all = vec![primary.clone(), fallback];
        let result = probe_with_fallback(
            primary,
            Some("http://a.example.com".to_string()),
            10,
            true,
            5,
            &all,
            false,
            3000,
            &db,
            queued_probe(vec![short_probe, video_probe()]),
        )
        .await;
        let (_, effective) = result.unwrap();
        assert_eq!(
            effective.id, fallback_id,
            "short primary skipped — effective stream must be the fallback"
        );
    }

    #[tokio::test]
    async fn no_url_stream_not_counted_as_attempt() {
        let db = test_db().await;
        let primary = http_media("http://a.example.com");
        let all = vec![primary.clone()];
        let result = probe_with_fallback(
            primary,
            None, // no URL — will be skipped without incrementing attempts
            10,
            false,
            0,
            &all,
            false,
            3000,
            &db,
            queued_probe(vec![]),
        )
        .await;
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("tried 0 of 1 streams"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn cascade_mode_tries_all_streams() {
        let db = test_db().await;
        let sources: Vec<_> = (0..4)
            .map(|i| http_media(&format!("http://s{i}.example.com")))
            .collect();
        let primary = sources[0].clone();
        let result = probe_with_fallback(
            primary,
            Some("http://s0.example.com".to_string()),
            10,
            true,
            1, // max_retries=1, but restrict_resolution=false → cascade
            &sources,
            false,
            3000,
            &db,
            queued_probe(vec![
                Err(anyhow!("fail")),
                Err(anyhow!("fail")),
                Err(anyhow!("fail")),
                Err(anyhow!("fail")),
            ]),
        )
        .await;
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("tried 4 of 4 streams"),
            "unexpected error: {err}"
        );
    }

    // Regression: items_playbackinfo was building probe_pool = candidate_streams,
    // which is narrowed to 1 item when a specific media_source_id is requested.
    // The fallback pool must be the full filtered list, not the selection.
    #[tokio::test]
    async fn specific_stream_request_falls_back_through_all_streams() {
        let db = test_db().await;
        let primary = http_media("http://a.example.com");
        let other_a = http_media("http://b.example.com");
        let other_b = http_media("http://c.example.com");

        // probe_pool is the full pool (as items_playbackinfo now provides via fallback_streams).
        let probe_pool = vec![primary.clone(), other_a, other_b];

        let result = probe_with_fallback(
            primary,
            Some("http://a.example.com".into()),
            10,
            true,
            5,
            &probe_pool,
            false,
            3000,
            &db,
            queued_probe(vec![
                Err(anyhow!("fail")),
                Err(anyhow!("fail")),
                Err(anyhow!("fail")),
            ]),
        )
        .await;
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("tried 3 of 3 streams"),
            "expected all 3 streams tried, got: {err}"
        );
    }

    // Regression coverage for "tried 1 of 1 streams" when media_source_id is
    // a stream UUID: resolver.stream resolves to a Stream child record, not the
    // episode, so all_streams was built from the wrong starting point.

    #[tokio::test]
    async fn stream_root_passes_through_when_media_is_top_level() {
        let db = test_db().await;
        let movie = db::Media {
            kind: db::MediaKind::Movie,
            ..Default::default()
        };
        let id = movie.id;
        let root = resolve_stream_root(&movie, id, &db).await;
        assert_eq!(root.id, id, "Movie kind must pass through unchanged");
    }

    #[tokio::test]
    async fn stream_root_passes_through_for_episode_and_track() {
        let db = test_db().await;
        for kind in [db::MediaKind::Episode, db::MediaKind::Track] {
            let media = db::Media {
                kind: kind.clone(),
                ..Default::default()
            };
            let id = media.id;
            let root = resolve_stream_root(&media, id, &db).await;
            assert_eq!(
                root.id, id,
                "{kind:?} must pass through without a DB lookup"
            );
        }
    }

    /// Build a Movie with the deterministic UUID required by validate().
    fn test_movie(stremio_id: &str, title: &str) -> db::Media {
        let external_ids = db::ExternalIds {
            custom_stremio_id: Some(stremio_id.to_string()),
            ..Default::default()
        };
        let id = crate::common::stable_media_uuid(&db::MediaKind::Movie, stremio_id);
        db::Media {
            id,
            kind: db::MediaKind::Movie,
            title: title.into(),
            external_ids,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn stream_root_loads_parent_when_media_is_stream_record() {
        // Reproduces Bug 3: client sends media_source_id = stream_uuid, so
        // resolver returns the Stream child as `media` instead of the Movie.
        let db = test_db().await;
        let movie = test_movie("test:src_root:1", "Test Movie");
        let movie_id = movie.id;
        db::Media::insert(&db, &[movie])
            .await
            .unwrap();

        // Simulate what resolver.stream returns when media_source_id = stream_uuid
        let stream_record = db::Media {
            kind: db::MediaKind::Stream,
            parent_id: Some(movie_id),
            title: "stream child".into(),
            stream_info: Some(StreamInfo {
                descriptor: StreamDescriptor::http("http://s1.example.com"),
                ..Default::default()
            }),
            ..Default::default()
        };

        // `id` in the handler is always the top-level movie UUID from the URL path.
        let root = resolve_stream_root(&stream_record, movie_id, &db).await;
        assert_eq!(
            root.id, movie_id,
            "stream record must resolve to its parent movie, not itself"
        );
        assert!(
            matches!(root.kind, db::MediaKind::Movie),
            "stream_root.kind must be Movie, got {:?}",
            root.kind
        );
        assert_ne!(root.id, stream_record.id);
    }

    #[tokio::test]
    async fn stream_kind_yields_full_sibling_pool() {
        // End-to-end regression: when media_source_id is a stream UUID the
        // fallback pool must contain ALL sibling streams, not just the one
        // that was requested. Before the stream_root fix this returned 1 →
        // "tried 1 of 1 streams" even when the movie had many streams.
        let db = test_db().await;
        let movie = test_movie("test:src_root:2", "Test Movie");
        let movie_id = movie.id;
        db::Media::insert(&db, &[movie])
            .await
            .unwrap();

        let siblings: Vec<db::Media> = (0..3)
            .map(|i| db::Media {
                kind: db::MediaKind::Stream,
                parent_id: Some(movie_id),
                title: format!("Stream {i}"),
                stream_info: Some(StreamInfo {
                    descriptor: StreamDescriptor::http(format!(
                        "http://s{i}.example.com"
                    )),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect();
        db::Media::insert(&db, &siblings)
            .await
            .unwrap();

        // Simulate the handler receiving media = siblings[0] (resolver returned stream child)
        let media = siblings[0].clone();
        let mut root = resolve_stream_root(&media, movie_id, &db).await;
        let pool = root
            .streams(&db)
            .await
            .unwrap();

        assert_eq!(
            pool.len(),
            3,
            "all 3 sibling streams must be in the fallback pool; \
             if this is 1 the stream_root fix was reverted and \
             'tried 1 of 1 streams' returns when media_source_id is a stream UUID"
        );
        for sibling in &siblings {
            assert!(
                pool.iter()
                    .any(|s| s.id == sibling.id),
                "stream {} missing from pool",
                sibling.id
            );
        }
    }
}
