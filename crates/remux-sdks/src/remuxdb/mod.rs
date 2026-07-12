use serde::Serialize;
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize)]
pub struct ExternalIds {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kitsu_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaInfo {
    pub client_id: String,
    pub kind: String,
    pub filename: String,
    pub torrent_info_hash: Option<String>,
    pub torrent_file_idx: Option<i32>,
    pub container: String,
    pub size: i64,
    pub duration: f64,
    pub bitrate: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub is_hearing_impaired: bool,
    pub is_interlaced: bool,
    pub hdr10_plus_present: bool,
    pub dv_profile: Option<i32>,
    pub dv_level: Option<i32>,
    pub dv_version_major: Option<i32>,
    pub dv_version_minor: Option<i32>,
    pub dv_bl_signal_compat_id: Option<i32>,
    pub dv_rpu_present: bool,
    pub dv_bl_present: bool,
    pub dv_el_present: bool,
    pub is_anamorphic: bool,
    pub level: Option<i32>,
    pub ref_frames: Option<i32>,
}

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
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub is_hearing_impaired: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubtitleTrackPayload {
    pub idx: i32,
    pub codec: Option<String>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub comment: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub is_hearing_impaired: bool,
}

impl MediaInfo {
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
