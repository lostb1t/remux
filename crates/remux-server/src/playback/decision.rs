use crate::{
    api, db,
    device_profile::{DeviceProfileExt, SubtitleCodec, subtitle_codec_matches_profile},
};
use remux_sdks::remux::{EmbeddedSubtitleHandling, EncodingOptions};
use uuid::Uuid;

/// Per-request config shared across all streams in the playback loop.
pub(crate) struct PlaybackConfig {
    pub encoding_cfg: EncodingOptions,
    pub device_profile: Option<api::DeviceProfile>,
    pub max_bitrate: Option<i64>,
    pub play_session_id: String,
    pub item_id: Uuid,
    pub subtitle_mode: EmbeddedSubtitleHandling,
}

pub(crate) struct TranscodeOutcome {
    pub url: String,
    pub container: String,
    pub sub_protocol: String,
}

impl TranscodeOutcome {
    pub(crate) fn apply_to(self, source: &mut api::MediaSourceInfo) {
        source.supports_transcoding = true;
        source.transcoding_url = Some(self.url);
        source.transcoding_container = Some(self.container);
        source.transcoding_sub_protocol = self.sub_protocol;
        source.supports_direct_play = false;
        source.supports_direct_stream = false;
    }
}

/// The outcome of the transcode-vs-direct-play decision for one stream.
pub(crate) enum TranscodeDecision {
    /// Client can play directly; no transcode URL needed.
    DirectPlay,
    /// Video transcode is required but the user/config disallows it; drop this source.
    Skip,
    /// Transcode URL built; apply to the source.
    Transcode(TranscodeOutcome),
}

pub(crate) fn build_transcode_decision(
    source: &api::MediaSourceInfo,
    reasons: &api::TranscodeReasons,
    effective_sub_idx: Option<i64>,
    q: &api::PlaybackInfoQuery,
    session: &db::auth::AuthSession,
    cfg: &PlaybackConfig,
) -> TranscodeDecision {
    let transcode_required = !reasons.is_empty()
        || !q
            .enable_direct_play
            .unwrap_or(true)
        || !q
            .enable_direct_stream
            .unwrap_or(true);
    if !transcode_required
        || !q
            .enable_transcoding
            .unwrap_or(true)
    {
        return TranscodeDecision::DirectPlay;
    }

    if source
        .video_stream()
        .is_none()
    {
        return TranscodeDecision::Transcode(build_audio_transcode(
            source, q, session, cfg,
        ));
    }
    build_video_transcode(source, reasons, effective_sub_idx, q, session, cfg)
}

fn build_audio_transcode(
    source: &api::MediaSourceInfo,
    q: &api::PlaybackInfoQuery,
    session: &db::auth::AuthSession,
    cfg: &PlaybackConfig,
) -> TranscodeOutcome {
    let trans_profile = cfg
        .device_profile
        .as_ref()
        .and_then(|p| p.audio_transcoding_profile());
    let container = trans_profile
        .and_then(|p| {
            p.container
                .clone()
        })
        .unwrap_or_else(|| "mp3".to_string());
    let audio_codec = trans_profile
        .and_then(|p| {
            p.audio_codec
                .as_deref()
        })
        .and_then(|c| {
            c.split(',')
                .next()
        })
        .map(|c| {
            c.trim()
                .to_string()
        })
        .unwrap_or_else(|| "aac".to_string());
    let start_time = q
        .start_time_ticks
        .map(|t| format!("&StartTimeTicks={t}"))
        .unwrap_or_default();

    TranscodeOutcome {
        url: format!(
            "/videos/{}/stream.{}?MediaSourceId={}&AudioCodec={}{}&ApiKey={}",
            cfg.item_id,
            container,
            source.id,
            audio_codec,
            start_time,
            session
                .device
                .access_token,
        ),
        container,
        sub_protocol: "http".to_string(),
    }
}

fn build_video_transcode(
    source: &api::MediaSourceInfo,
    reasons: &api::TranscodeReasons,
    effective_sub_idx: Option<i64>,
    q: &api::PlaybackInfoQuery,
    session: &db::auth::AuthSession,
    cfg: &PlaybackConfig,
) -> TranscodeDecision {
    let trans_profile = cfg
        .device_profile
        .as_ref()
        .and_then(|p| p.video_transcoding_profile());
    let (container, protocol) = trans_profile
        .map(|p| {
            (
                p.container
                    .clone()
                    .unwrap_or_else(|| "ts".to_string()),
                p.protocol
                    .clone()
                    .unwrap_or_else(|| "hls".to_string()),
            )
        })
        .unwrap_or_else(|| ("ts".to_string(), "hls".to_string()));

    let needs_video_transcode = reasons
        .contains(&api::TranscodeReason::VideoCodecNotSupported(String::new()))
        || reasons.contains(&api::TranscodeReason::ContainerBitrateExceedsLimit)
        || reasons.contains(&api::TranscodeReason::VideoRangeTypeNotSupported(
            String::new(),
        ));

    let video_transcode_allowed = cfg
        .encoding_cfg
        .enable_video_transcoding
        .unwrap_or(true)
        && session
            .user
            .policy
            .as_ref()
            .map(|p| p.enable_video_playback_transcoding)
            .unwrap_or(true);

    if needs_video_transcode && !video_transcode_allowed {
        return TranscodeDecision::Skip;
    }

    let mut video_codec = if needs_video_transcode {
        "h264"
    } else {
        "copy"
    }
    .to_string();
    let needs_audio_transcode =
        reasons.contains(&api::TranscodeReason::AudioCodecNotSupported(String::new()));
    let audio_codec = if needs_audio_transcode { "aac" } else { "copy" }.to_string();

    let subtitle_method = subtitle_burn_method(
        source,
        effective_sub_idx,
        &cfg.subtitle_mode,
        &cfg.device_profile,
    );
    if subtitle_method == Some(api::SubtitleDeliveryMethod::Encode) {
        video_codec = "h264".to_string();
    }

    let bitrate = cfg
        .max_bitrate
        .map(|b| format!("&MaxStreamingBitrate={b}"))
        .unwrap_or_default();
    let reasons_param = reasons
        .to_query_value()
        .map(|v| format!("&TranscodeReasons={v}"))
        .unwrap_or_default();
    let audio_idx = q
        .audio_stream_index
        .or(source.default_audio_stream_index)
        .map(|i| format!("&AudioStreamIndex={i}"))
        .unwrap_or_default();
    let sub_idx = effective_sub_idx
        .map(|i| format!("&SubtitleStreamIndex={i}"))
        .unwrap_or_default();
    let sub_method = subtitle_method
        .map(|m| format!("&SubtitleMethod={m}"))
        .unwrap_or_default();
    let start_time = q
        .start_time_ticks
        .map(|t| format!("&StartTimeTicks={t}"))
        .unwrap_or_default();

    let url = if protocol.eq_ignore_ascii_case("hls") {
        format!(
            "/videos/{}/master.m3u8?PlaySessionId={}&MediaSourceId={}&VideoCodec={}&AudioCodec={}{}{}{}{}{}{}&ApiKey={}",
            cfg.item_id,
            cfg.play_session_id,
            source.id,
            video_codec,
            audio_codec,
            bitrate,
            reasons_param,
            audio_idx,
            sub_idx,
            sub_method,
            start_time,
            session
                .device
                .access_token,
        )
    } else {
        format!(
            "/videos/{}/stream.{}?PlaySessionId={}&MediaSourceId={}&VideoCodec={}&AudioCodec={}{}{}{}{}{}{}&ApiKey={}",
            cfg.item_id,
            container,
            cfg.play_session_id,
            source.id,
            video_codec,
            audio_codec,
            bitrate,
            reasons_param,
            audio_idx,
            sub_idx,
            sub_method,
            start_time,
            session
                .device
                .access_token,
        )
    };

    TranscodeDecision::Transcode(TranscodeOutcome {
        url,
        container,
        sub_protocol: protocol,
    })
}

/// Determines if a subtitle stream should be burned in by FFmpeg.
fn subtitle_burn_method(
    source: &api::MediaSourceInfo,
    effective_sub_idx: Option<i64>,
    subtitle_mode: &EmbeddedSubtitleHandling,
    device_profile: &Option<api::DeviceProfile>,
) -> Option<api::SubtitleDeliveryMethod> {
    let stream = effective_sub_idx.and_then(|idx| {
        source
            .media_streams
            .iter()
            .find(|s| {
                s.index == idx
                    && matches!(s.type_, Some(api::MediaStreamType::Subtitle))
            })
    })?;

    if stream.is_external
        || stream.is_text_subtitle_stream
        || *subtitle_mode != EmbeddedSubtitleHandling::Burn
    {
        return None;
    }

    let codec = stream
        .codec
        .as_deref()
        .unwrap_or("");
    let not_in_profile = !device_profile
        .as_ref()
        .map(|dp| {
            dp.subtitle_profiles
                .iter()
                .filter_map(|p| {
                    p.format
                        .as_deref()
                })
                .any(|f| subtitle_codec_matches_profile(codec, f))
        })
        .unwrap_or(false);

    if not_in_profile {
        Some(api::SubtitleDeliveryMethod::Encode)
    } else {
        None
    }
}

/// Assigns delivery URLs and methods to all subtitle streams in `source`.
pub(crate) fn apply_subtitle_delivery(
    source: &mut api::MediaSourceInfo,
    item_id: Uuid,
    access_token: &str,
    device_profile: &Option<api::DeviceProfile>,
    subtitle_mode: EmbeddedSubtitleHandling,
) {
    let source_id = source.id;
    for stream in source
        .media_streams
        .iter_mut()
    {
        if stream.type_ != Some(api::MediaStreamType::Subtitle) {
            continue;
        }
        let codec = stream
            .codec
            .as_deref()
            .unwrap_or_default();
        let profile_supports = |c: SubtitleCodec| -> bool {
            device_profile
                .as_ref()
                .map(|dp| {
                    dp.subtitle_profiles
                        .iter()
                        .filter_map(|p| {
                            p.format
                                .as_deref()
                        })
                        .any(|f| {
                            f.parse::<SubtitleCodec>()
                                .ok()
                                .as_ref()
                                == Some(&c)
                        })
                })
                .unwrap_or(false)
        };
        let profile_embeds = |c: SubtitleCodec| -> bool {
            device_profile
                .as_ref()
                .map(|dp| {
                    dp.subtitle_profiles
                        .iter()
                        .any(|p| {
                            p.method == Some(api::SubtitleDeliveryMethod::Embed)
                                && p.format
                                    .as_deref()
                                    .and_then(|f| {
                                        f.parse::<SubtitleCodec>()
                                            .ok()
                                    })
                                    .as_ref()
                                    == Some(&c)
                        })
                })
                .unwrap_or(false)
        };
        let parsed_codec = codec
            .parse::<SubtitleCodec>()
            .ok();
        let is_image_sub = parsed_codec
            .as_ref()
            .map(SubtitleCodec::is_image)
            .unwrap_or(false);
        let format = if stream.is_text_subtitle_stream {
            if parsed_codec == Some(SubtitleCodec::Ass)
                && profile_supports(SubtitleCodec::Ass)
            {
                "ass"
            } else {
                "vtt"
            }
        } else if profile_supports(SubtitleCodec::Pgs) {
            "sup"
        } else {
            "vtt"
        };
        let client_can_handle_image = is_image_sub
            && parsed_codec
                .as_ref()
                .map(|c| profile_supports(c.clone()) || profile_embeds(c.clone()))
                .unwrap_or(false);
        if !stream.is_external
            && parsed_codec
                .as_ref()
                .map(|c| profile_embeds(c.clone()))
                .unwrap_or(false)
        {
            stream.delivery_method = Some(api::SubtitleDeliveryMethod::Embed);
        } else if !stream.is_external
            && is_image_sub
            && !client_can_handle_image
            && subtitle_mode == EmbeddedSubtitleHandling::Burn
        {
            stream.delivery_method = Some(api::SubtitleDeliveryMethod::Encode);
        } else {
            let idx = stream.index;
            stream.delivery_url = Some(format!(
                "/Videos/{item_id}/{source_id}/Subtitles/{idx}/0/Stream.{format}?ApiKey={access_token}",
            ));
            stream.delivery_method = Some(api::SubtitleDeliveryMethod::External);
            stream.is_external_url = Some(false);
            stream.is_external = false;
        }
    }
}
