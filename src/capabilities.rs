//use dioxus::web::document;
use crate::sdks;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, error, info, trace, warn, Level};
use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct Capabilities {
    pub supportsH264: bool,
    pub supportsVP9: bool,
    pub supportsAV1: bool,
    pub supportsHEVC: bool,
    pub supportsEAC3: bool,
    pub supportsFLAC: bool,
    pub supportsAAC: bool,
    pub supportsOpus: bool,
    pub supportsMP3: bool,
    pub supportsHLS: bool,
    pub supportsWebVTT: bool,
    pub userAgent: String,
}

impl Capabilities {
    /// Detects browser media capabilities from JS
    pub async fn detect_browser_capabilities() -> Option<Self> {
        let js = r#"

                const video = document.createElement('video');
                const audio = document.createElement('audio');

                function canPlayAny(typeList) {
                    if (!video.canPlayType) return false;
                    return typeList.some(type => video.canPlayType(type).replace(/no/, '') !== '');
                }

                const hevcTypes = [
                    'video/mp4; codecs="hvc1.1.L120"',
                    'video/mp4; codecs="hev1.1.L120"',
                    'video/mp4; codecs="hvc1.1.0.L120"',
                    'video/mp4; codecs="hev1.1.0.L120"',
                ];

                const av1Types = [
                    'video/mp4; codecs="av01.0.05M.08"',
                    'video/mp4; codecs="av01.0.08M.08"',
                ];

                const h264Types = [
                    'video/mp4; codecs="avc1.42E01E, mp4a.40.2"',
                    'video/mp4; codecs="avc1.4d401e"',
                    'video/mp4; codecs="avc1.640028"',
                ];

                const vp9Types = [
                    'video/webm; codecs="vp9, opus"',
                    'video/webm; codecs="vp9"',
                ];

                const eac3Types = [
                    'audio/mp4; codecs="ec-3"',
                    'audio/eac3',
                ];

                const flacTypes = [
                    'audio/flac',
                    'audio/x-flac',
                    'audio/webm; codecs="flac"',
                ];

                const aacTypes = [
                    'audio/mp4; codecs="mp4a.40.2"',
                    'audio/aac',
                ];

                const opusTypes = [
                    'audio/webm; codecs="opus"',
                    'audio/ogg; codecs="opus"',
                ];

                const mp3Types = [
                    'audio/mpeg',
                    'audio/mp3',
                ];

                return {
                    supportsH264: canPlayAny(h264Types),
                    supportsVP9: canPlayAny(vp9Types),
                    supportsAV1: canPlayAny(av1Types),
                    supportsHEVC: canPlayAny(hevcTypes),
                    supportsEAC3: canPlayAny(eac3Types),
                    supportsFLAC: canPlayAny(flacTypes),
                    supportsAAC: canPlayAny(aacTypes),
                    supportsOpus: canPlayAny(opusTypes),
                    supportsMP3: canPlayAny(mp3Types),
                    supportsHLS: !!video.canPlayType('application/vnd.apple.mpegURL'),
                    supportsWebVTT: 'track' in document.createElement('track'),
                    userAgent: navigator.userAgent
                };
            
        "#;

        match document::eval(js).await {
            Ok(json) => {
                //info!("{:?}", json);
                // dbg!(&json);
                serde_json::from_value(json).ok()
            }
            Err(err) => {
                error!("Media capability detection failed: {err}");
                None
            }
        }
    }

    /// Converts detected capabilities into a Jellyfin DeviceProfile
    pub fn to_device_profile(&self) -> sdks::jellyfin::DeviceProfile {
        let mut direct_play_profiles = vec![];
        debug!("{:?}", &self);
        // Video
        if self.supportsH264 {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "mp4".into(),
                type_: "Video".into(),
                video_codec: Some("h264".into()),
                audio_codec: Some("aac".into()),
                protocol: Some("http".into()),
            });
        }
        if self.supportsVP9 {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "webm".into(),
                type_: "Video".into(),
                video_codec: Some("vp9".into()),
                audio_codec: Some("opus".into()),
                protocol: Some("http".into()),
            });
        }
        if self.supportsAV1 {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "mp4".into(),
                type_: "Video".into(),
                video_codec: Some("av1".into()),
                audio_codec: Some("aac".into()),
                protocol: Some("http".into()),
            });
        }
        if self.supportsHEVC {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "mp4".into(),
                type_: "Video".into(),
                video_codec: Some("hevc".into()),
                audio_codec: Some("aac".into()),
                protocol: Some("http".into()),
            });
        }

        // Audio
        if self.supportsAAC {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "mp4".into(),
                type_: "Audio".into(),
                audio_codec: Some("aac".into()),
                video_codec: None,
                protocol: Some("http".into()),
            });
        }
        if self.supportsOpus {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "webm".into(),
                type_: "Audio".into(),
                audio_codec: Some("opus".into()),
                video_codec: None,
                protocol: Some("http".into()),
            });
        }
        if self.supportsMP3 {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "mp3".into(),
                type_: "Audio".into(),
                audio_codec: Some("mp3".into()),
                video_codec: None,
                protocol: Some("http".into()),
            });
        }
        if self.supportsFLAC {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "webm".into(),
                type_: "Audio".into(),
                audio_codec: Some("flac".into()),
                video_codec: None,
                protocol: Some("http".into()),
            });
        }
        if self.supportsEAC3 {
            direct_play_profiles.push(sdks::jellyfin::DirectPlayProfile {
                container: "mp4".into(),
                type_: "Audio".into(),
                audio_codec: Some("eac3".into()),
                video_codec: None,
                protocol: Some("http".into()),
            });
        }

        // Subtitles
        let subtitle_profiles = build_all_subtitle_profiles();

        let transcoding_profiles = vec![sdks::jellyfin::TranscodingProfile {
            container: "ts".into(),
            type_: "Video".into(),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            protocol: Some("hls".into()),
            context: Some("Streaming".into()),
            enable_subtitles_in_manifest: Some(true),
            ..Default::default()
        }];

        //let container_profiles = vec![
        //     sdks::jellyfin::ContainerProfile {
        //         type_: "Subtitle".into(),
        //         containers: Some(vec!["vtt". by into()]),
        //         ..Default::default()
        //     }
        //];

        sdks::jellyfin::DeviceProfile {
            name: Some(self.userAgent.clone()),
            max_streaming_bitrate: Some(20_000_000),
            direct_play_profiles: Some(direct_play_profiles),
            subtitle_profiles: Some(subtitle_profiles),
            transcoding_profiles: Some(transcoding_profiles),
            //container_profiles: Some(container_profiles),
            codec_profiles: Some(vec![]),
            ..Default::default()
        }
    }
}

fn build_all_subtitle_profiles() -> Vec<sdks::jellyfin::SubtitleProfile> {
    let formats = [
        "vtt",
        "webvtt",
        "srt",
        "subrip",
        "ttml",
        "dvbsub",
        "ass",
        "idx",
        "pgs",
        "pgssub",
        "ssa",

        // Other formats
        "microdvd",
        "mov_text",
        "mpl2",
        "pjs",
        "realtext",
        "scc",
        "smi",
        "stl",
        "sub",
        "subviewer",
        "teletext",
        "text",
        "vplayer",
        "xsub",
    ];

    let methods = [
        // Method::Embed,
        sdks::jellyfin::SubtitleDeliveryMethod::Hls,
        sdks::jellyfin::SubtitleDeliveryMethod::External,
        sdks::jellyfin::SubtitleDeliveryMethod::Encode,
    ];

    formats
        .iter()
        .flat_map(|&format| {
            methods
                .iter()
                .map(move |method| sdks::jellyfin::SubtitleProfile {
                    format: Some(format.to_string()),
                    method: Some(method.clone()),
                    ..Default::default()
                })
        })
        .collect()
}
