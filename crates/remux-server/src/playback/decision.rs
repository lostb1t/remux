use crate::{
    api, db,
    device_profile::{
        DeviceProfileExt, SubtitleCodec, VideoCodec, subtitle_codec_matches_profile,
    },
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

    let remuxing_allowed = cfg
        .encoding_cfg
        .enable_remuxing
        .unwrap_or(true)
        && session
            .user
            .policy
            .as_ref()
            .map(|p| p.enable_playback_remuxing)
            .unwrap_or(true);
    if !remuxing_allowed {
        return TranscodeDecision::DirectPlay;
    }

    // Only take the audio path when the source explicitly has audio streams
    // but NO video stream. An empty media_streams (unprobed skip-probe
    // candidate) should default to the video path — the item being played
    // is a movie/episode, not a music track.
    let has_audio = source
        .audio_stream()
        .is_some();
    let has_video = source
        .video_stream()
        .is_some();
    if has_audio && !has_video {
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
                .as_ref()
                .map(|c| c.to_string())
        })
        .unwrap_or_else(|| "mp3".to_string());
    let audio_transcode_allowed = cfg
        .encoding_cfg
        .enable_audio_transcoding
        .unwrap_or(true)
        && session
            .user
            .policy
            .as_ref()
            .map(|p| p.enable_audio_playback_transcoding)
            .unwrap_or(true);
    let audio_codec = if audio_transcode_allowed {
        trans_profile
            .and_then(|p| {
                p.audio_codec
                    .as_ref()
            })
            .and_then(|c| c.first())
            .map(|c| c.to_string())
            .unwrap_or_else(|| "aac".to_string())
    } else {
        "copy".to_string()
    };
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
                .access_token
                .expose(),
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
                    .as_ref()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "ts".to_string()),
                p.protocol
                    .as_ref()
                    .map(|pr| pr.to_string())
                    .unwrap_or_else(|| "hls".to_string()),
            )
        })
        .unwrap_or_else(|| ("ts".to_string(), "hls".to_string()));

    let needs_video_transcode = reasons
        .contains(&api::TranscodeReason::VideoCodecNotSupported(String::new()))
        || reasons.contains(&api::TranscodeReason::ContainerBitrateExceedsLimit)
        || reasons.contains(&api::TranscodeReason::VideoRangeTypeNotSupported(
            String::new(),
        ))
        || reasons.contains(&api::TranscodeReason::VideoProfileNotSupported(
            String::new(),
        ))
        || reasons.contains(&api::TranscodeReason::VideoBitDepthNotSupported(
            String::new(),
        ));

    // When video re-encoding is not allowed (server setting or user policy),
    // fall through with video=copy — remux the container and transcode audio
    // as needed rather than dropping the source entirely.
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

    let mut video_codec = if needs_video_transcode && video_transcode_allowed {
        "h264"
    } else {
        "copy"
    }
    .to_string();
    let needs_audio_transcode =
        reasons.contains(&api::TranscodeReason::AudioCodecNotSupported(String::new()));
    let audio_transcode_allowed = cfg
        .encoding_cfg
        .enable_audio_transcoding
        .unwrap_or(true)
        && session
            .user
            .policy
            .as_ref()
            .map(|p| p.enable_audio_playback_transcoding)
            .unwrap_or(true);
    let audio_codec = if needs_audio_transcode && audio_transcode_allowed {
        "aac"
    } else {
        "copy"
    }
    .to_string();

    let subtitle_method = {
        let method = subtitle_burn_method(
            source,
            effective_sub_idx,
            &cfg.subtitle_mode,
            &cfg.device_profile,
        );
        if method == Some(api::SubtitleDeliveryMethod::Encode) {
            if video_transcode_allowed {
                video_codec = "h264".to_string();
                method
            } else {
                // Burn-in requires video re-encoding; drop it when encoding is disabled.
                None
            }
        } else {
            method
        }
    };

    // If policy constraints reduced both codecs to copy, this would be a no-op
    // remux. If the source container already matches the transcoding target
    // there is nothing to do — upgrade to direct play.
    let needs_hevc_retag = reasons.contains(
        &api::TranscodeReason::VideoCodecTagNotSupported(String::new()),
    );
    if video_codec == "copy" && audio_codec == "copy" && !needs_hevc_retag {
        let src = source
            .container
            .as_ref()
            .map(|c| c.to_string())
            .unwrap_or_default()
            .to_lowercase();
        if src == container.to_lowercase() {
            return TranscodeDecision::DirectPlay;
        }
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
    // The DeviceProfile only exists on this request, but the tag is needed
    // later when the HLS session builds its ffmpeg args — carry the resolved
    // value on the URL, like every other per-playback decision above. Only a
    // stream copy of HEVC has a sample entry to write, so nothing else carries
    // the parameter.
    let is_hevc_copy = video_codec == "copy"
        && source
            .video_stream()
            .and_then(|s| {
                s.codec
                    .as_deref()
            })
            .and_then(|c| {
                c.parse::<VideoCodec>()
                    .ok()
            })
            .map(|c| c.is_hevc())
            .unwrap_or(false);
    let video_codec_tag = q
        .device_profile
        .as_ref()
        .filter(|_| is_hevc_copy)
        .map(|p| format!("&VideoCodecTag={}", p.hevc_copy_tag(source)))
        .unwrap_or_default();

    let url = if protocol.eq_ignore_ascii_case("hls") {
        format!(
            "/videos/{}/master.m3u8?PlaySessionId={}&MediaSourceId={}&VideoCodec={}&AudioCodec={}{}{}{}{}{}{}{}&ApiKey={}",
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
            video_codec_tag,
            session
                .device
                .access_token
                .expose(),
        )
    } else {
        format!(
            "/videos/{}/stream.{}?PlaySessionId={}&MediaSourceId={}&VideoCodec={}&AudioCodec={}{}{}{}{}{}{}{}&ApiKey={}",
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
            video_codec_tag,
            session
                .device
                .access_token
                .expose(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use remux_sdks::remux::VideoContainer;

    /// A source with no video stream routes to its own transcode branch, which
    /// builds its own URL. That URL is what the client fetches next, so it has
    /// to carry the real session token rather than the `Secret`'s redaction.
    #[test]
    fn audio_only_transcode_url_carries_the_real_token() {
        let session = db::auth::AuthSession {
            device: db::auth::Device {
                access_token: "real-token"
                    .to_string()
                    .into(),
                ..Default::default()
            },
            user: db::User::default(),
        };
        let source = api::MediaSourceInfo {
            id: Uuid::new_v4(),
            media_streams: vec![api::MediaStream {
                codec: Some("flac".to_string()),
                type_: Some(api::MediaStreamType::Audio),
                index: 0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let cfg = PlaybackConfig {
            encoding_cfg: EncodingOptions::default(),
            device_profile: None,
            max_bitrate: None,
            play_session_id: "play-session".to_string(),
            item_id: Uuid::new_v4(),
            subtitle_mode: EmbeddedSubtitleHandling::default(),
        };
        // Direct play off is what forces the decision down a transcode branch.
        let q = api::PlaybackInfoQuery {
            enable_direct_play: Some(false),
            ..Default::default()
        };

        let decision = build_transcode_decision(
            &source,
            &api::TranscodeReasons::default(),
            None,
            &q,
            &session,
            &cfg,
        );

        let TranscodeDecision::Transcode(outcome) = decision else {
            panic!("a source with no video stream should transcode");
        };
        assert!(
            outcome
                .url
                .contains("ApiKey=real-token"),
            "audio transcode URL should carry the session token: {}",
            outcome.url
        );
    }

    fn make_video_source(container: VideoContainer) -> api::MediaSourceInfo {
        api::MediaSourceInfo {
            id: Uuid::new_v4(),
            container: Some(container),
            media_streams: vec![
                api::MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(api::MediaStreamType::Video),
                    index: 0,
                    ..Default::default()
                },
                api::MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(api::MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    fn make_session_with_policy(
        policy: remux_sdks::remux::UserPolicy,
    ) -> db::auth::AuthSession {
        db::auth::AuthSession {
            device: db::auth::Device {
                access_token: "tok"
                    .to_string()
                    .into(),
                ..Default::default()
            },
            user: db::User {
                policy: Some(sqlx::types::Json(policy)),
                ..Default::default()
            },
        }
    }

    fn base_cfg(encoding_cfg: EncodingOptions) -> PlaybackConfig {
        PlaybackConfig {
            encoding_cfg,
            device_profile: None,
            max_bitrate: None,
            play_session_id: "s".to_string(),
            item_id: Uuid::new_v4(),
            subtitle_mode: EmbeddedSubtitleHandling::default(),
        }
    }

    fn force_transcode_query() -> api::PlaybackInfoQuery {
        api::PlaybackInfoQuery {
            enable_direct_play: Some(false),
            ..Default::default()
        }
    }

    #[test]
    fn remuxing_disabled_by_policy_returns_direct_play() {
        let mut policy = remux_sdks::remux::UserPolicy::default();
        policy.enable_playback_remuxing = false;
        let session = make_session_with_policy(policy);
        let source = make_video_source(VideoContainer::Ts);
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::VideoCodecNotSupported(
            "hevc".to_string(),
        ));
        let decision = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(EncodingOptions::default()),
        );
        assert!(
            matches!(decision, TranscodeDecision::DirectPlay),
            "remuxing disabled should force direct play"
        );
    }

    #[test]
    fn remuxing_disabled_globally_returns_direct_play() {
        let session = db::auth::AuthSession {
            device: db::auth::Device {
                access_token: "tok"
                    .to_string()
                    .into(),
                ..Default::default()
            },
            user: db::User::default(),
        };
        let source = make_video_source(VideoContainer::Ts);
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::VideoCodecNotSupported(
            "hevc".to_string(),
        ));
        let mut enc = EncodingOptions::default();
        enc.enable_remuxing = Some(false);
        let decision = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(enc),
        );
        assert!(
            matches!(decision, TranscodeDecision::DirectPlay),
            "global remuxing disabled should force direct play"
        );
    }

    #[test]
    fn audio_transcode_disabled_by_policy_forces_copy() {
        let mut policy = remux_sdks::remux::UserPolicy::default();
        policy.enable_audio_playback_transcoding = false;
        let session = make_session_with_policy(policy);
        let source = make_video_source(VideoContainer::Ts);
        // Both video AND audio need transcoding so the result is a real transcode
        // URL (video=h264). The audio codec must be copy despite needing a transcode.
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::VideoCodecNotSupported(
            "hevc".to_string(),
        ));
        reasons.insert(api::TranscodeReason::AudioCodecNotSupported(
            "ac3".to_string(),
        ));
        let TranscodeDecision::Transcode(outcome) = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(EncodingOptions::default()),
        ) else {
            panic!("expected transcode outcome");
        };
        assert!(
            outcome
                .url
                .contains("AudioCodec=copy"),
            "audio codec should be copy when transcoding is disabled: {}",
            outcome.url
        );
    }

    #[test]
    fn audio_transcode_disabled_globally_forces_copy() {
        let session = db::auth::AuthSession {
            device: db::auth::Device {
                access_token: "tok"
                    .to_string()
                    .into(),
                ..Default::default()
            },
            user: db::User::default(),
        };
        let source = make_video_source(VideoContainer::Ts);
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::VideoCodecNotSupported(
            "hevc".to_string(),
        ));
        reasons.insert(api::TranscodeReason::AudioCodecNotSupported(
            "ac3".to_string(),
        ));
        let mut enc = EncodingOptions::default();
        enc.enable_audio_transcoding = Some(false);
        let TranscodeDecision::Transcode(outcome) = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(enc),
        ) else {
            panic!("expected transcode outcome");
        };
        assert!(
            outcome
                .url
                .contains("AudioCodec=copy"),
            "global audio transcoding disabled should force copy: {}",
            outcome.url
        );
    }

    #[test]
    fn both_copy_same_container_returns_direct_play() {
        // video transcode disabled + audio doesn't need transcoding + container matches
        // → nothing would change → direct play
        let mut policy = remux_sdks::remux::UserPolicy::default();
        policy.enable_video_playback_transcoding = false;
        let session = make_session_with_policy(policy);
        let source = make_video_source(VideoContainer::Ts);
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::VideoCodecNotSupported(
            "hevc".to_string(),
        ));
        // No audio transcode reason — audio codec would be copy already.
        // With video also forced to copy and same container (ts) the result is a no-op.
        let decision = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(EncodingOptions::default()),
        );
        assert!(
            matches!(decision, TranscodeDecision::DirectPlay),
            "no-op remux should be upgraded to direct play"
        );
    }

    #[test]
    fn hevc_tag_mismatch_same_container_still_remuxes() {
        // Rewriting hvc1/hev1 is the sole purpose of this remux. Returning
        // direct play merely because the container already matches would leave
        // the incompatible sample entry untouched.
        let session =
            make_session_with_policy(remux_sdks::remux::UserPolicy::default());
        let mut source = make_video_source(VideoContainer::Ts);
        source.media_streams[0].codec = Some("hevc".to_string());
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::VideoCodecTagNotSupported(
            "hev1".to_string(),
        ));

        let TranscodeDecision::Transcode(outcome) = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(EncodingOptions::default()),
        ) else {
            panic!("an HEVC sample-entry mismatch must remux, not direct play");
        };
        assert!(
            outcome
                .url
                .contains("VideoCodec=copy")
        );
    }

    /// An HEVC stream copy out of MKV, so the target container differs and the
    /// decision reaches the URL builder.
    fn hevc_copy_outcome(
        codec: &str,
        codec_tag: Option<&str>,
    ) -> super::TranscodeOutcome {
        let session =
            make_session_with_policy(remux_sdks::remux::UserPolicy::default());
        let mut source = make_video_source(VideoContainer::Mkv);
        source.media_streams[0].codec = Some(codec.to_string());
        source.media_streams[0].codec_tag = codec_tag.map(str::to_string);
        let mut q = force_transcode_query();
        q.device_profile = Some(api::DeviceProfile::default());

        let TranscodeDecision::Transcode(outcome) = build_transcode_decision(
            &source,
            &api::TranscodeReasons::default(),
            None,
            &q,
            &session,
            &base_cfg(EncodingOptions::default()),
        ) else {
            panic!("expected a remux");
        };
        outcome
    }

    #[test]
    fn hevc_copy_url_carries_the_resolved_sample_entry_tag() {
        let url = hevc_copy_outcome("hevc", Some("hev1")).url;
        assert!(url.contains("&VideoCodecTag=hev1"), "{url}");
    }

    #[test]
    fn hevc_copy_url_keeps_hvc1_for_an_ordinary_source() {
        let url = hevc_copy_outcome("hevc", Some("hvc1")).url;
        assert!(url.contains("&VideoCodecTag=hvc1"), "{url}");
    }

    #[test]
    fn non_hevc_copy_url_omits_the_sample_entry_tag() {
        // Only HEVC has a sample entry to rewrite; on anything else the
        // parameter is noise the session would carry around for nothing.
        let url = hevc_copy_outcome("h264", Some("avc1")).url;
        assert!(!url.contains("VideoCodecTag"), "{url}");
    }

    #[test]
    fn both_copy_different_container_returns_transcode() {
        // video transcode disabled + audio copy + but container needs to change → remux URL
        let mut policy = remux_sdks::remux::UserPolicy::default();
        policy.enable_video_playback_transcoding = false;
        let session = make_session_with_policy(policy);
        // Source is mkv, transcoding profile will pick ts → remux is needed
        let source = make_video_source(VideoContainer::Mkv);
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::VideoCodecNotSupported(
            "hevc".to_string(),
        ));
        let TranscodeDecision::Transcode(outcome) = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(EncodingOptions::default()),
        ) else {
            panic!("expected transcode outcome for container remux");
        };
        assert_eq!(outcome.container, "ts");
    }

    #[test]
    fn video_profile_not_supported_forces_a_real_video_transcode() {
        // Detecting the reason is not enough — build_video_transcode must also
        // act on it, or a device-profile rejection (e.g. Hi10p) would still
        // direct-play as VideoCodec=copy despite being reported as unsupported.
        let session =
            make_session_with_policy(remux_sdks::remux::UserPolicy::default());
        let source = make_video_source(VideoContainer::Mkv);
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::VideoProfileNotSupported(
            "High 10".to_string(),
        ));
        let TranscodeDecision::Transcode(outcome) = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(EncodingOptions::default()),
        ) else {
            panic!(
                "VideoProfileNotSupported must trigger a transcode, not direct play"
            );
        };
        assert!(
            outcome
                .url
                .contains("VideoCodec=h264"),
            "VideoProfileNotSupported must force real video re-encoding, not copy: {}",
            outcome.url
        );
    }

    #[test]
    fn test_burn_mode_transcodes_only_when_subtitle_actively_selected() {
        let session =
            make_session_with_policy(remux_sdks::remux::UserPolicy::default());
        let mut source = make_video_source(VideoContainer::Mkv);
        source.media_streams = vec![
            api::MediaStream {
                codec: Some("h264".to_string()),
                type_: Some(api::MediaStreamType::Video),
                index: 0,
                ..Default::default()
            },
            api::MediaStream {
                codec: Some("aac".to_string()),
                type_: Some(api::MediaStreamType::Audio),
                index: 1,
                ..Default::default()
            },
            api::MediaStream {
                codec: Some("hdmv_pgs_subtitle".to_string()),
                type_: Some(api::MediaStreamType::Subtitle),
                index: 2,
                ..Default::default()
            },
        ];

        let profile = api::DeviceProfile {
            direct_play_profiles: vec![remux_sdks::remux::DirectPlayProfile {
                container: Some(vec![remux_sdks::remux::VideoContainer::Mkv]),
                video_codec: Some(vec![remux_sdks::remux::VideoCodec::H264]),
                audio_codec: Some(vec![remux_sdks::remux::AudioCodec::Aac]),
                type_: Some(remux_sdks::remux::DlnaProfileType::Video),
            }],
            subtitle_profiles: vec![remux_sdks::remux::SubtitleProfile {
                format: Some("vtt".to_string()),
                method: Some(remux_sdks::remux::SubtitleDeliveryMethod::External),
            }],
            ..Default::default()
        };

        let mut cfg = base_cfg(EncodingOptions::default());
        cfg.device_profile = Some(profile);
        cfg.subtitle_mode = EmbeddedSubtitleHandling::Burn;

        // When NO subtitle is selected (None): DirectPlay (no transcode reasons)
        let reasons = api::TranscodeReasons::default();
        let query = api::PlaybackInfoQuery::default();
        let decision =
            build_transcode_decision(&source, &reasons, None, &query, &session, &cfg);
        assert!(
            matches!(decision, TranscodeDecision::DirectPlay),
            "no subtitle selected in burn mode should remain direct play"
        );

        // When PGS subtitle (index 2) is actively selected: Transcode with video re-encoding
        let mut burn_reasons = api::TranscodeReasons::default();
        burn_reasons.insert(api::TranscodeReason::SubtitleCodecNotSupported(
            "hdmv_pgs_subtitle".to_string(),
        ));
        let decision_sub = build_transcode_decision(
            &source,
            &burn_reasons,
            Some(2),
            &query,
            &session,
            &cfg,
        );
        match decision_sub {
            TranscodeDecision::Transcode(outcome) => {
                assert!(
                    outcome
                        .url
                        .contains("VideoCodec=h264"),
                    "PGS burn-in must trigger video transcoding: {}",
                    outcome.url
                );
                assert!(
                    outcome
                        .url
                        .contains("SubtitleMethod=Encode"),
                    "PGS burn-in must set SubtitleMethod=Encode: {}",
                    outcome.url
                );
            }
            TranscodeDecision::DirectPlay => {
                panic!("PGS subtitle selected in burn mode must trigger transcoding");
            }
        }
    }
}
