use crate::api;
use anyhow::{Result, anyhow};
use isolang::Language;
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;

fn ffprobe_bin() -> String {
    std::env::var("FFPROBE_PATH").unwrap_or_else(|_| "ffprobe".into())
}

fn nonzero<T: Default + PartialOrd>(v: T) -> Option<T> {
    if v > T::default() { Some(v) } else { None }
}

fn first_to_upper(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn subtitle_codec_display(codec: &str) -> &str {
    match codec {
        "hdmv_pgs_subtitle" => "pgssub",
        "dvd_subtitle" => "dvdsub",
        other => other,
    }
}

fn audio_codec_friendly(codec: &str) -> &str {
    match codec {
        "aac" => "AAC",
        "ac3" | "a52" => "Dolby Digital",
        "eac3" => "Dolby Digital Plus",
        "truehd" => "TrueHD",
        "dca" | "dts" => "DTS",
        "flac" => "FLAC",
        "mp3" => "MP3",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "pcm_s16le" | "pcm_s24le" | "pcm_s32le" | "pcm_f32le" => "PCM",
        "alac" => "ALAC",
        other => other,
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
        if !title.to_ascii_lowercase().contains(&tag.to_ascii_lowercase()) {
            result.push_str(" - ");
            result.push_str(tag);
        }
    }
    result
}

struct StreamMeta<'a> {
    language: Option<&'a str>,
    codec: Option<&'a str>,
    profile: Option<&'a str>,
    channels: Option<i64>,
    channel_layout: Option<&'a str>,
    width: Option<i64>,
    height: Option<i64>,
    video_range: Option<&'a api::VideoRange>,
    is_default: bool,
    is_forced: bool,
    is_external: bool,
    is_hearing_impaired: bool,
    title: Option<&'a str>,
}

fn display_title_audio(m: &StreamMeta) -> Option<String> {
    let mut attrs: Vec<String> = vec![];

    if let Some(lang) = m.language {
        let special = ["und", "mis", "zxx", "mul"];
        if !special.contains(&lang.to_ascii_lowercase().as_str()) {
            let name = Language::from_str(lang)
                .ok()
                .map(|l| l.to_name().to_string())
                .unwrap_or_else(|| lang.to_string());
            attrs.push(first_to_upper(&name));
        }
    }

    let profile_lc = m.profile.map(|p| p.to_ascii_lowercase());
    if let Some(ref p) = profile_lc {
        if p != "lc" {
            attrs.push(m.profile.unwrap().to_string());
        }
    } else if let Some(codec) = m.codec {
        attrs.push(audio_codec_friendly(codec).to_string());
    }

    if let Some(layout) = m.channel_layout {
        attrs.push(first_to_upper(layout));
    } else if let Some(ch) = m.channels {
        attrs.push(format!("{} ch", ch));
    }

    if m.is_default { attrs.push("Default".into()); }
    if m.is_external { attrs.push("External".into()); }

    if let Some(title) = m.title {
        return Some(append_tags_to_title(title, &attrs));
    }
    if attrs.is_empty() { None } else { Some(attrs.join(" - ")) }
}

fn display_title_video(m: &StreamMeta) -> Option<String> {
    let mut attrs: Vec<String> = vec![];

    if let Some(res) = video_resolution_text(m.width, m.height) {
        attrs.push(res);
    }
    if let Some(codec) = m.codec {
        attrs.push(codec.to_ascii_uppercase());
    }
    if let Some(range) = m.video_range {
        if *range != api::VideoRange::Unknown {
            attrs.push(format!("{:?}", range));
        }
    }

    if let Some(title) = m.title {
        return Some(append_tags_to_title(title, &attrs));
    }
    if attrs.is_empty() { None } else { Some(attrs.join(" ")) }
}

fn display_title_subtitle(m: &StreamMeta) -> Option<String> {
    let mut attrs: Vec<String> = vec![];

    if let Some(lang) = m.language {
        let name = Language::from_str(lang)
            .ok()
            .map(|l| l.to_name().to_string())
            .unwrap_or_else(|| lang.to_string());
        attrs.push(first_to_upper(&name));
    } else {
        attrs.push("Und".into());
    }

    if m.is_hearing_impaired { attrs.push("Hearing Impaired".into()); }
    if m.is_default { attrs.push("Default".into()); }
    if m.is_forced { attrs.push("Forced".into()); }
    if let Some(codec) = m.codec {
        attrs.push(subtitle_codec_display(codec).to_ascii_uppercase());
    }
    if m.is_external { attrs.push("External".into()); }

    if attrs.is_empty() { None } else { Some(attrs.join(" - ")) }
}

#[derive(Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: FfprobeFormat,
}

#[derive(Deserialize, Default)]
struct FfprobeDisposition {
    #[serde(default)]
    default: i64,
    #[serde(default)]
    forced: i64,
    #[serde(default)]
    hearing_impaired: i64,
}

#[derive(Deserialize)]
struct FfprobeStream {
    index: i64,
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    bit_rate: Option<String>,
    avg_frame_rate: Option<String>,
    channels: Option<i64>,
    channel_layout: Option<String>,
    sample_rate: Option<String>,
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    disposition: FfprobeDisposition,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    format_name: Option<String>,
    bit_rate: Option<String>,
}

fn parse_frame_rate(s: &str) -> Option<f64> {
    let mut parts = s.splitn(2, '/');
    let num: f64 = parts.next()?.parse().ok()?;
    let den: f64 = parts.next().unwrap_or("1").parse().ok()?;
    if den == 0.0 {
        return None;
    }
    let fps = num / den;
    if fps > 0.0 { Some(fps) } else { None }
}

/// Probe a media URL with ffprobe and return a Jellyfin MediaSourceInfo.
pub fn probe_media(url: &str) -> Result<api::MediaSourceInfo> {
    tracing::debug!(url, "probing media");

    let output = std::process::Command::new(ffprobe_bin())
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            url,
        ])
        .output()
        .map_err(|e| anyhow!("Failed to run ffprobe: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffprobe failed for {}: {}", url, stderr));
    }

    let probe: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow!("Failed to parse ffprobe output: {}", e))?;

    let run_time_ticks = probe
        .format
        .duration
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|secs| (secs * 1_000_000.0) as i64) // → µs
        .and_then(nonzero)
        .map(|us| us * 10); // µs → 100ns ticks

    let container = probe.format.format_name.as_deref().map(|f| {
        let base = f.split(',').next().unwrap_or(f);
        match base {
            "matroska" => "mkv".to_string(),
            "mov" => "mp4".to_string(),
            "mpegts" => "ts".to_string(),
            other => other.to_string(),
        }
    });

    let overall_bitrate = probe
        .format
        .bit_rate
        .as_deref()
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(nonzero);

    tracing::debug!(?run_time_ticks, ?container, "probe container info");

    let mut streams: Vec<api::MediaStream> = Vec::new();
    let mut video_idx: i64 = 0;
    let mut audio_idx: i64 = 0;
    let mut sub_idx: i64 = 0;

    for s in &probe.streams {
        let codec_type = s.codec_type.as_deref().unwrap_or("");
        let language = s.tags.get("language").map(|s| s.as_str());
        let title = s.tags.get("title").cloned();

        match codec_type {
            "video" => {
                let bitrate = s
                    .bit_rate
                    .as_deref()
                    .and_then(|b| b.parse::<i64>().ok())
                    .and_then(nonzero);
                let fps = s.avg_frame_rate.as_deref().and_then(parse_frame_rate);
                let codec = s.codec_name.clone().unwrap_or_default();
                let is_default = video_idx == 0;
                let is_forced = s.disposition.forced != 0;

                let meta = StreamMeta {
                    language,
                    codec: Some(&codec),
                    profile: s.profile.as_deref(),
                    channels: None,
                    channel_layout: None,
                    width: s.width.and_then(nonzero),
                    height: s.height.and_then(nonzero),
                    video_range: None,
                    is_default,
                    is_forced,
                    is_external: false,
                    is_hearing_impaired: false,
                    title: title.as_deref(),
                };

                streams.push(api::MediaStream {
                    type_: Some(api::MediaStreamType::Video),
                    index: s.index,
                    codec: Some(codec.clone()),
                    width: meta.width,
                    height: meta.height,
                    bit_rate: bitrate,
                    average_frame_rate: fps.map(|f| f as f32).and_then(nonzero),
                    real_frame_rate: fps.map(|f| f as f32).and_then(nonzero),
                    is_default: Some(is_default),
                    is_forced,
                    is_avc: Some(false),
                    time_base: Some("1/1000".to_string()),
                    audio_spatial_format: Some("None".to_string()),
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
                    .and_then(|b| b.parse::<i64>().ok())
                    .and_then(nonzero);
                let channels = s.channels.and_then(nonzero);
                let sample_rate = s
                    .sample_rate
                    .as_deref()
                    .and_then(|sr| sr.parse::<i64>().ok())
                    .and_then(nonzero);
                let codec = s.codec_name.clone().unwrap_or_default();
                let is_default = audio_idx == 0;
                let is_forced = s.disposition.forced != 0;
                let channel_layout = s.channel_layout.as_deref();

                let meta = StreamMeta {
                    language,
                    codec: Some(&codec),
                    profile: s.profile.as_deref(),
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
                    video_range: Some(api::VideoRange::Unknown),
                    video_range_type: Some(api::VideoRangeType::Unknown),
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
                let codec = s.codec_name.clone().unwrap_or_default();
                let is_text = matches!(
                    codec.as_str(),
                    "ass" | "ssa" | "subrip" | "webvtt" | "mov_text" | "text"
                );
                let is_image = matches!(
                    codec.as_str(),
                    "pgssub" | "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvdsub"
                );
                let delivery_method = if is_image {
                    Some(api::SubtitleDeliveryMethod::Embed)
                } else {
                    None
                };
                let is_default = sub_idx == 0;
                let is_forced = s.disposition.forced != 0;
                let is_hearing_impaired = s.disposition.hearing_impaired != 0;

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

                streams.push(api::MediaStream {
                    type_: Some(api::MediaStreamType::Subtitle),
                    index: s.index,
                    codec: Some(codec.clone()),
                    is_default: Some(is_default),
                    is_forced,
                    is_hearing_impaired,
                    is_avc: Some(false),
                    time_base: Some("1/1000".to_string()),
                    video_range: Some(api::VideoRange::Unknown),
                    video_range_type: Some(api::VideoRangeType::Unknown),
                    audio_spatial_format: Some("None".to_string()),
                    localized_undefined: Some("Undefined".to_string()),
                    localized_default: Some("Default".to_string()),
                    localized_forced: Some("Forced".to_string()),
                    localized_external: Some("External".to_string()),
                    localized_hearing_impaired: Some("Hearing Impaired".to_string()),
                    display_title: display_title_subtitle(&meta),
                    language: language.map(str::to_string),
                    title,
                    is_text_subtitle_stream: is_text,
                    supports_external_stream: true,
                    delivery_method,
                    ..Default::default()
                });
                sub_idx += 1;
            }
            _ => {}
        }
    }

    let default_audio_stream_index = streams
        .iter()
        .find(|s| matches!(s.type_, Some(api::MediaStreamType::Audio)))
        .map(|s| s.index);
    let default_subtitle_stream_index = streams
        .iter()
        .find(|s| matches!(s.type_, Some(api::MediaStreamType::Subtitle)))
        .map(|s| s.index);

    Ok(api::MediaSourceInfo {
        media_streams: streams,
        container,
        run_time_ticks,
        bitrate: overall_bitrate,
        default_audio_stream_index,
        default_subtitle_stream_index,
        ..Default::default()
    })
}
