use crate::{
    ClientError, Endpoint, RestClient,
    remux::{
        MediaSourceInfo, MediaStream, MediaStreamType, VideoRange, VideoRangeType,
    },
};
use http::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use tracing::{debug, warn};

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct ExternalIds {
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub kitsu_id: Option<i64>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct NzbSubmission {
    pub indexer: String,
    pub indexer_guid: String,
    pub title: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct MediaInfoPayload {
    pub client_id: Option<String>,
    pub kind: String,
    pub filename: String,
    pub torrent_info_hash: Option<String>,
    pub torrent_file_idx: Option<i32>,
    pub nzb: Option<NzbSubmission>,
    pub container: String,
    pub size: i64,
    pub duration: f64,
    pub bitrate: Option<i64>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub external_ids: Option<ExternalIds>,
    pub tracks: Vec<TrackPayload>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TrackPayload {
    Video(VideoTrackPayload),
    Audio(AudioTrackPayload),
    Subtitle(SubtitleTrackPayload),
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct VideoTrackPayload {
    pub idx: i32,
    pub codec: String,
    pub width: i32,
    pub height: i32,
    pub fps: Option<f64>,
    pub avg_fps: Option<f64>,
    pub bit_rate: Option<i64>,
    pub bit_depth: Option<i32>,
    pub pixel_format: Option<String>,
    pub profile: Option<String>,
    pub codec_tag: Option<String>,
    pub comment: Option<String>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub color_primaries: Option<String>,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub aspect_ratio: Option<String>,
    pub rotation: Option<i32>,
    pub is_default: Option<bool>,
    pub is_forced: Option<bool>,
    pub is_external: Option<bool>,
    pub is_hearing_impaired: Option<bool>,
    pub is_interlaced: Option<bool>,
    pub is_anamorphic: Option<bool>,
    pub hdr10_plus_present: Option<bool>,
    pub dv_profile: Option<i32>,
    pub dv_level: Option<i32>,
    pub dv_version_major: Option<i32>,
    pub dv_version_minor: Option<i32>,
    pub dv_bl_signal_compat_id: Option<i32>,
    pub dv_rpu_present: Option<bool>,
    pub dv_bl_present: Option<bool>,
    pub dv_el_present: Option<bool>,
    pub level: Option<i32>,
    pub ref_frames: Option<i32>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct AudioTrackPayload {
    pub idx: i32,
    pub codec: String,
    pub channels: i32,
    pub sample_rate: i32,
    pub bit_rate: Option<i64>,
    pub bit_depth: Option<i32>,
    pub channel_layout: Option<String>,
    pub profile: Option<String>,
    pub codec_tag: Option<String>,
    pub comment: Option<String>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub is_default: Option<bool>,
    pub is_forced: Option<bool>,
    pub is_external: Option<bool>,
    pub is_hearing_impaired: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct SubtitleTrackPayload {
    pub idx: i32,
    pub codec: Option<String>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub comment: Option<String>,
    pub is_default: Option<bool>,
    pub is_forced: Option<bool>,
    pub is_external: Option<bool>,
    pub is_hearing_impaired: Option<bool>,
}

impl MediaInfoPayload {
    pub async fn submit(self, base_url: String, token: Option<String>) {
        let url = format!("{}/api/mediainfo", base_url.trim_end_matches('/'));
        let body = match serde_json::to_string(&self) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "remuxdb: failed to serialize payload");
                return;
            }
        };
        debug!(url, "remuxdb: sending");
        let mut req = reqwest::Client::new()
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body);
        if let Some(t) = token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        match req
            .send()
            .await
        {
            Ok(resp)
                if resp
                    .status()
                    .is_success() =>
            {
                debug!(url, "remuxdb: mediainfo submitted ok");
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp
                    .text()
                    .await
                    .unwrap_or_default();
                warn!(url, %status, body, "remuxdb mediainfo submission failed");
            }
            Err(e) => {
                warn!(url, error = %e, "remuxdb mediainfo submission error");
            }
        }
    }
}

/// Flat track returned by `GET /api/media/info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackDetail {
    pub kind: String,
    pub idx: i32,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub is_forced: bool,
    #[serde(default)]
    pub is_hearing_impaired: bool,
    #[serde(default)]
    pub is_external: bool,
    #[serde(default)]
    pub is_anamorphic: bool,
    #[serde(default)]
    pub hdr10_plus_present: bool,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub bit_rate: Option<i64>,
    pub bit_depth: Option<i32>,
    pub pixel_format: Option<String>,
    pub profile: Option<String>,
    pub level: Option<i32>,
    pub ref_frames: Option<i32>,
    // video
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub fps: Option<f64>,
    pub aspect_ratio: Option<String>,
    pub rotation: Option<i32>,
    pub color_primaries: Option<String>,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub dv_profile: Option<i32>,
    // audio
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channel_layout: Option<String>,
}

/// A source (torrent or NZB) within a MediaInfo group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeSource {
    pub kind: String,
    pub filename: Option<String>,
    pub indexer: Option<String>,
    pub indexer_guid: Option<String>,
    pub torrent_info_hash: Option<String>,
    pub torrent_file_idx: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterDetail {
    pub id: Option<i32>,
    pub title: Option<String>,
    pub start_time: Option<f64>,
    pub end_time: Option<f64>,
}

/// One probe result returned by `GET /api/media/info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    pub content_hash: Option<String>,
    pub container: Option<String>,
    pub duration: Option<f64>,
    pub size: Option<i64>,
    pub bitrate: Option<i64>,
    #[serde(default)]
    pub virtual_chapters: bool,
    #[serde(default)]
    pub chapters: Vec<ChapterDetail>,
    pub sources: Vec<ProbeSource>,
    pub tracks: Vec<TrackDetail>,
}

#[derive(Clone)]
struct MediaInfoEndpoint {
    imdb_id: String,
    season: Option<i32>,
    episode: Option<i32>,
    token: Option<String>,
    client_id: Option<String>,
}

impl Endpoint for MediaInfoEndpoint {
    type Output = Vec<MediaInfo>;

    fn path(&self) -> String {
        let mut path = format!("/api/media/info?imdb_id={}", self.imdb_id);
        if let Some(s) = self.season {
            path.push_str(&format!("&season={s}"));
        }
        if let Some(e) = self.episode {
            path.push_str(&format!("&episode={e}"));
        }
        path
    }

    fn headers(&self) -> HeaderMap {
        let mut map = HeaderMap::new();
        if let Some(t) = &self.token {
            if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
                map.insert(http::header::AUTHORIZATION, v);
            }
        }
        if let Some(id) = &self.client_id {
            if let Ok(v) = HeaderValue::from_str(id) {
                map.insert("x-client-id", v);
            }
        }
        map
    }
}

/// Fetch probe versions for a media title from RemuxDB.
/// Returns `None` on 404 or any error (failures are logged at debug level).
pub async fn fetch_probe(
    base_url: &str,
    token: Option<&str>,
    client_id: Option<&str>,
    imdb_id: &str,
    season: Option<i32>,
    episode: Option<i32>,
) -> Option<Vec<MediaInfo>> {
    let client = match RestClient::new(base_url.trim_end_matches('/')) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "remuxdb: invalid base url");
            return None;
        }
    };
    let ep = MediaInfoEndpoint {
        imdb_id: imdb_id.to_string(),
        season,
        episode,
        token: token.map(|s| s.to_string()),
        client_id: client_id.map(|s| s.to_string()),
    };
    match client
        .execute(ep)
        .await
    {
        Ok(versions) => {
            debug!(count = versions.len(), "remuxdb: probe versions fetched");
            Some(versions)
        }
        Err(ClientError::Http { status: 404, .. }) => None,
        Err(e) => {
            warn!(error = %e, "remuxdb: probe fetch failed");
            None
        }
    }
}

impl TryFrom<&MediaStream> for TrackPayload {
    type Error = ();

    fn try_from(ms: &MediaStream) -> Result<Self, ()> {
        match ms
            .type_
            .ok_or(())?
        {
            MediaStreamType::Video => Ok(TrackPayload::Video(VideoTrackPayload {
                idx: ms.index as i32,
                codec: ms
                    .codec
                    .clone()
                    .unwrap_or_default(),
                width: ms
                    .width
                    .unwrap_or(0) as i32,
                height: ms
                    .height
                    .unwrap_or(0) as i32,
                fps: ms
                    .real_frame_rate
                    .map(|f| f as f64),
                avg_fps: ms
                    .average_frame_rate
                    .map(|f| f as f64),
                bit_rate: ms.bit_rate,
                bit_depth: ms
                    .bit_depth
                    .map(|d| d as i32),
                pixel_format: ms
                    .pixel_format
                    .clone(),
                profile: ms
                    .profile
                    .clone(),
                codec_tag: ms
                    .codec_tag
                    .clone(),
                comment: ms
                    .comment
                    .clone(),
                title: ms
                    .title
                    .clone(),
                language: ms
                    .language
                    .clone(),
                color_primaries: ms
                    .color_primaries
                    .clone(),
                color_range: ms
                    .color_range
                    .clone(),
                color_space: ms
                    .color_space
                    .clone(),
                color_transfer: ms
                    .color_transfer
                    .clone(),
                aspect_ratio: ms
                    .aspect_ratio
                    .clone(),
                rotation: ms
                    .rotation
                    .map(|r| r as i32),
                is_default: ms.is_default,
                is_forced: Some(ms.is_forced),
                is_external: Some(ms.is_external),
                is_hearing_impaired: Some(ms.is_hearing_impaired),
                is_interlaced: Some(ms.is_interlaced),
                is_anamorphic: ms.is_anamorphic,
                hdr10_plus_present: Some(matches!(
                    ms.video_range_type,
                    Some(VideoRangeType::Hdr10Plus)
                )),
                dv_profile: ms
                    .dv_profile
                    .map(|v| v as i32),
                dv_level: ms
                    .dv_level
                    .map(|v| v as i32),
                dv_version_major: ms
                    .dv_version_major
                    .map(|v| v as i32),
                dv_version_minor: ms
                    .dv_version_minor
                    .map(|v| v as i32),
                dv_bl_signal_compat_id: ms
                    .dv_bl_signal_compatibility_id
                    .map(|v| v as i32),
                dv_rpu_present: ms
                    .rpu_present_flag
                    .map(|v| v != 0),
                dv_bl_present: ms
                    .bl_present_flag
                    .map(|v| v != 0),
                dv_el_present: ms
                    .el_present_flag
                    .map(|v| v != 0),
                level: ms
                    .level
                    .map(|v| v as i32),
                ref_frames: ms
                    .ref_frames
                    .map(|v| v as i32),
            })),
            MediaStreamType::Audio => Ok(TrackPayload::Audio(AudioTrackPayload {
                idx: ms.index as i32,
                codec: ms
                    .codec
                    .clone()
                    .unwrap_or_default(),
                channels: ms
                    .channels
                    .unwrap_or(0) as i32,
                sample_rate: ms
                    .sample_rate
                    .unwrap_or(0) as i32,
                bit_rate: ms.bit_rate,
                bit_depth: ms
                    .bit_depth
                    .map(|d| d as i32),
                channel_layout: ms
                    .channel_layout
                    .clone(),
                profile: ms
                    .profile
                    .clone(),
                codec_tag: ms
                    .codec_tag
                    .clone(),
                comment: ms
                    .comment
                    .clone(),
                title: ms
                    .title
                    .clone(),
                language: ms
                    .language
                    .clone(),
                is_default: ms.is_default,
                is_forced: Some(ms.is_forced),
                is_external: Some(ms.is_external),
                is_hearing_impaired: Some(ms.is_hearing_impaired),
            })),
            MediaStreamType::Subtitle => {
                Ok(TrackPayload::Subtitle(SubtitleTrackPayload {
                    idx: ms.index as i32,
                    codec: ms
                        .codec
                        .clone(),
                    title: ms
                        .title
                        .clone(),
                    language: ms
                        .language
                        .clone(),
                    comment: ms
                        .comment
                        .clone(),
                    is_default: ms.is_default,
                    is_forced: Some(ms.is_forced),
                    is_external: Some(ms.is_external),
                    is_hearing_impaired: Some(ms.is_hearing_impaired),
                }))
            }
            _ => Err(()),
        }
    }
}

impl From<&TrackDetail> for MediaStream {
    fn from(t: &TrackDetail) -> Self {
        let type_ = match t
            .kind
            .as_str()
        {
            "video" => Some(MediaStreamType::Video),
            "audio" => Some(MediaStreamType::Audio),
            "subtitle" => Some(MediaStreamType::Subtitle),
            _ => None,
        };
        let range_type = if let Some(dv) = t.dv_profile {
            if dv > 0 {
                VideoRangeType::Dovi
            } else {
                VideoRangeType::Sdr
            }
        } else if t.hdr10_plus_present {
            VideoRangeType::Hdr10Plus
        } else {
            match t
                .color_transfer
                .as_deref()
            {
                Some("smpte2084") => VideoRangeType::Hdr10,
                Some("arib-std-b67") => VideoRangeType::Hlg,
                _ => VideoRangeType::Sdr,
            }
        };
        let video_range = match range_type {
            VideoRangeType::Sdr | VideoRangeType::Unknown => VideoRange::Sdr,
            _ => VideoRange::Hdr,
        };
        MediaStream {
            index: t.idx as i64,
            type_,
            codec: t
                .codec
                .clone(),
            bit_rate: t.bit_rate,
            bit_depth: t
                .bit_depth
                .map(|v| v as i64),
            pixel_format: t
                .pixel_format
                .clone(),
            profile: t
                .profile
                .clone(),
            title: t
                .title
                .clone(),
            language: t
                .language
                .clone(),
            is_default: Some(t.is_default),
            is_forced: t.is_forced,
            is_external: t.is_external,
            is_hearing_impaired: t.is_hearing_impaired,
            width: t
                .width
                .map(|v| v as i64),
            height: t
                .height
                .map(|v| v as i64),
            real_frame_rate: t
                .fps
                .map(|v| v as f32),
            color_primaries: t
                .color_primaries
                .clone(),
            color_range: t
                .color_range
                .clone(),
            color_space: t
                .color_space
                .clone(),
            color_transfer: t
                .color_transfer
                .clone(),
            aspect_ratio: t
                .aspect_ratio
                .clone(),
            rotation: t
                .rotation
                .map(|v| v as i64),
            video_range: Some(video_range),
            video_range_type: Some(range_type),
            dv_profile: t
                .dv_profile
                .map(|v| v as i64),
            is_anamorphic: Some(t.is_anamorphic),
            level: t
                .level
                .map(|v| v as f64),
            ref_frames: t
                .ref_frames
                .map(|v| v as i64),
            channels: t
                .channels
                .map(|v| v as i64),
            sample_rate: t
                .sample_rate
                .map(|v| v as i64),
            channel_layout: t
                .channel_layout
                .clone(),
            ..Default::default()
        }
    }
}

impl From<&MediaInfo> for MediaSourceInfo {
    fn from(version: &MediaInfo) -> Self {
        MediaSourceInfo {
            container: version
                .container
                .clone(),
            size: version.size,
            run_time_ticks: version
                .duration
                .map(|d| (d * 10_000_000.0).round() as i64),
            bitrate: version.bitrate,
            media_streams: version
                .tracks
                .iter()
                .map(MediaStream::from)
                .collect(),
            ..Default::default()
        }
    }
}
