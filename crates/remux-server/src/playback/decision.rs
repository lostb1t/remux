use crate::{
    api, db,
    device_profile::{
        DeviceProfileExt, SubtitleCodec, VideoCodec, subtitle_codec_matches_profile,
    },
};
use remux_sdks::remux::{
    DlnaProfileType, EmbeddedSubtitleHandling, EncodingOptions, PlayMethod,
    TranscodingProtocol,
};
use uuid::Uuid;

/// Effective playback-processing permissions for a request.
///
/// Server-wide encoding settings and per-user policy are deliberately combined
/// here so playback negotiation and the endpoints that actually start FFmpeg
/// cannot disagree about what is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlaybackPermissions {
    pub remuxing: bool,
    pub video_transcoding: bool,
    pub audio_transcoding: bool,
}

impl PlaybackPermissions {
    pub(crate) fn for_user(
        encoding: &EncodingOptions,
        user: Option<&db::User>,
    ) -> Self {
        let policy = user.and_then(|user| {
            user.policy
                .as_ref()
        });

        Self {
            remuxing: encoding
                .enable_remuxing
                .unwrap_or(true)
                && policy
                    .map(|p| p.enable_playback_remuxing)
                    .unwrap_or(true),
            video_transcoding: encoding
                .enable_video_transcoding
                .unwrap_or(true)
                && policy
                    .map(|p| p.enable_video_playback_transcoding)
                    .unwrap_or(true),
            audio_transcoding: encoding
                .enable_audio_transcoding
                .unwrap_or(true)
                && policy
                    .map(|p| p.enable_audio_playback_transcoding)
                    .unwrap_or(true),
        }
    }

    /// `audio_passthrough`: the requested audio codec equals the source's and
    /// no downmix or bitrate reduction is needed (see `audio_is_passthrough`).
    /// Resolve requested codecs and subtitle burn-in through the server/user
    /// permissions. Video resolves to `copy` or `h264` (burn-in forces a
    /// re-encode); disabled encoders become stream-copy requests. If that
    /// leaves a pure remux while remuxing is disabled, the caller must serve
    /// the source via direct play instead of starting FFmpeg.
    pub(crate) fn resolve_codecs(
        self,
        requested_video: &str,
        requested_audio: &str,
        audio_passthrough: bool,
        subtitle_burn_requested: bool,
    ) -> ResolvedPlaybackCodecs {
        let burn_subtitle = subtitle_burn_requested && self.video_transcoding;
        let video = if burn_subtitle
            || (!codec_is_copy(requested_video) && self.video_transcoding)
        {
            "h264"
        } else {
            "copy"
        }
        .to_string();
        let audio = if codec_is_copy(requested_audio) || !self.audio_transcoding {
            "copy".to_string()
        } else {
            requested_audio.to_string()
        };
        // Re-encoding audio into the codec it already has, without reshaping
        // it, is a pure remux in effect; the codec string above is left alone
        // so a genuine AAC downmix still reaches FFmpeg when remuxing is on.
        let direct_play_only = codec_is_copy(&video)
            && (codec_is_copy(&audio) || audio_passthrough)
            && !self.remuxing;

        ResolvedPlaybackCodecs {
            video,
            audio,
            direct_play_only,
            burn_subtitle,
        }
    }

    pub(crate) fn processing_available(self) -> bool {
        self.remuxing || self.video_transcoding || self.audio_transcoding
    }

    /// Constrain a client-reported method to one the server is allowed to use.
    /// The stream endpoint records the exact effective method when it runs;
    /// this is the fallback for clients that report playback before requesting
    /// the media URL, or whose stream URL carries no PlaySessionId.
    /// `DirectStream` is stored as reported, like Jellyfin does.
    pub(crate) fn constrain_reported_method(self, method: PlayMethod) -> PlayMethod {
        match method {
            PlayMethod::Transcode if !self.processing_available() => {
                PlayMethod::DirectPlay
            }
            method => method,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedPlaybackCodecs {
    pub video: String,
    pub audio: String,
    pub direct_play_only: bool,
    pub burn_subtitle: bool,
}

/// The audio track FFmpeg will map: the explicitly selected stream index,
/// otherwise the first audio track.
pub(crate) fn selected_audio_stream(
    source: &api::MediaSourceInfo,
    audio_stream_index: Option<i64>,
) -> Option<&api::MediaStream> {
    audio_stream_index
        .filter(|index| *index >= 0)
        .and_then(|index| {
            source
                .media_streams
                .iter()
                .find(|s| {
                    s.index == index
                        && matches!(s.type_, Some(api::MediaStreamType::Audio))
                })
        })
        .or_else(|| source.audio_stream())
}

/// True when encoding `requested_codec` would reproduce the source audio
/// as-is: same codec, and neither the channel nor bitrate cap cuts into it.
pub(crate) fn audio_is_passthrough(
    requested_codec: &str,
    source_codec: Option<&str>,
    source_channels: Option<i64>,
    source_bitrate: Option<i64>,
    max_channels: Option<i64>,
    max_bitrate: Option<i64>,
) -> bool {
    let Some(source_codec) = source_codec else {
        return false;
    };
    let exceeds = |cap: Option<i64>, source: Option<i64>| match (cap, source) {
        (Some(cap), Some(source)) => source > cap,
        (Some(_), None) => true,
        _ => false,
    };
    requested_codec.eq_ignore_ascii_case(source_codec)
        && !exceeds(max_channels, source_channels)
        && !exceeds(max_bitrate, source_bitrate)
}

fn codec_is_copy(codec: &str) -> bool {
    codec.eq_ignore_ascii_case("copy")
}

/// Per-request config shared across all streams in the playback loop.
pub(crate) struct PlaybackConfig {
    pub encoding_cfg: EncodingOptions,
    pub device_profile: Option<api::DeviceProfile>,
    pub max_bitrate: Option<i64>,
    pub play_session_id: String,
    pub item_id: Uuid,
    pub subtitle_mode: EmbeddedSubtitleHandling,
    pub is_live: bool,
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
    allow_subtitle_extraction: bool,
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

    let permissions =
        PlaybackPermissions::for_user(&cfg.encoding_cfg, Some(&session.user));

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
        // Without audio transcoding this would be a pure remux.
        if !permissions.audio_transcoding && !permissions.remuxing {
            return TranscodeDecision::DirectPlay;
        }
        return TranscodeDecision::Transcode(build_audio_transcode(
            source,
            q,
            session,
            cfg,
            permissions.audio_transcoding,
        ));
    }
    build_video_transcode(
        source,
        reasons,
        effective_sub_idx,
        q,
        session,
        cfg,
        permissions,
        allow_subtitle_extraction,
    )
}

fn build_audio_transcode(
    source: &api::MediaSourceInfo,
    q: &api::PlaybackInfoQuery,
    session: &db::auth::AuthSession,
    cfg: &PlaybackConfig,
    audio_transcoding: bool,
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
    let audio_codec = if audio_transcoding {
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
    permissions: PlaybackPermissions,
    allow_subtitle_extraction: bool,
) -> TranscodeDecision {
    let needs_video_transcode = reasons
        .0
        .iter()
        .any(api::TranscodeReason::is_video)
        || reasons.contains(&api::TranscodeReason::ContainerBitrateExceedsLimit);
    let needs_audio_transcode = reasons
        .0
        .iter()
        .any(api::TranscodeReason::is_audio);
    let subtitle_method = subtitle_burn_method(
        source,
        effective_sub_idx,
        &cfg.subtitle_mode,
        &cfg.device_profile,
        allow_subtitle_extraction,
    );

    // When an encoder is not allowed (server setting or user policy), fall
    // through with copy — remux the container and transcode the other stream
    // as needed rather than dropping the source entirely.
    let codecs = permissions.resolve_codecs(
        if needs_video_transcode {
            "h264"
        } else {
            "copy"
        },
        if needs_audio_transcode { "aac" } else { "copy" },
        false,
        subtitle_method == Some(api::SubtitleDeliveryMethod::Encode),
    );
    if codecs.direct_play_only {
        return TranscodeDecision::DirectPlay;
    }
    // Burn-in requires video re-encoding; drop it when encoding is disabled.
    let subtitle_method = subtitle_method
        .filter(|m| *m != api::SubtitleDeliveryMethod::Encode || codecs.burn_subtitle);
    let (video_codec, audio_codec) = (codecs.video, codecs.audio);

    // Jellyfin Web advertises both fMP4 and MPEG-TS HLS profiles. Jellyfin
    // selects TS for live H.264; match the profile to our HLS output, which
    // is TS except when copying HEVC as fMP4/CMAF.
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
    allow_extraction: bool,
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

    // Same rule `subtitle_burn_reason` and `apply_subtitle_delivery` use, so
    // the URL asks for burn-in exactly when the stream is marked `Encode`.
    let codec = stream
        .codec
        .as_deref()
        .unwrap_or("");
    let not_in_profile = !crate::device_profile::subtitle_codec_deliverable(
        &SubtitleCodec::from_codec_name(codec),
        device_profile.as_ref(),
        allow_extraction,
    );

    if not_in_profile {
        Some(api::SubtitleDeliveryMethod::Encode)
    } else {
        None
    }
}

/// Assigns delivery URLs and methods to all subtitle streams in `source`.
// TODO: Embed is chosen from the profile alone. For a transcoded/HLS source,
// verify that the selected output actually carries the embedded track before
// advertising Embed. This is a generic delivery issue, not client-specific.
pub(crate) fn apply_subtitle_delivery(
    source: &mut api::MediaSourceInfo,
    item_id: Uuid,
    access_token: &str,
    device_profile: &Option<api::DeviceProfile>,
    subtitle_mode: EmbeddedSubtitleHandling,
    allow_extraction: bool,
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
            crate::device_profile::profile_declares_subtitle_codec(
                device_profile.as_ref(),
                &c,
                None,
            )
        };
        let profile_embeds = |c: SubtitleCodec| -> bool {
            crate::device_profile::profile_embeds_subtitle_codec(
                device_profile.as_ref(),
                &c,
            )
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
        let idx = stream.index;
        let external_delivery = |stream: &mut api::MediaStream| {
            stream.delivery_url = Some(format!(
                "/Videos/{item_id}/{source_id}/Subtitles/{idx}/0/Stream.{format}?ApiKey={access_token}",
            ));
            stream.delivery_method = Some(api::SubtitleDeliveryMethod::External);
            stream.is_external_url = Some(false);
            stream.is_external = false;
        };
        if stream.is_external {
            // Already an external URL — no extraction involved, feasibility
            // is irrelevant. Matches this function's pre-existing behavior
            // for genuinely external streams (e.g. ones inserted directly,
            // not via append_external_subtitles).
            external_delivery(stream);
        } else if parsed_codec
            .as_ref()
            .map(|c| profile_embeds(c.clone()))
            .unwrap_or(false)
        {
            stream.delivery_method = Some(api::SubtitleDeliveryMethod::Embed);
        } else if parsed_codec
            .as_ref()
            .is_some_and(|c| {
                crate::device_profile::subtitle_codec_deliverable(
                    c,
                    device_profile.as_ref(),
                    allow_extraction,
                )
            })
        {
            // Extraction feasible (local source, or remote with the setting
            // on) — always prefer it over burning in: lossless, and doesn't
            // force a video re-encode. Same rule the PlaybackInfo filter and
            // the burn-in reason use (`subtitle_codec_deliverable`), so a PGS
            // track is only offered as a `.sup` when the profile declares
            // External PGS support; otherwise it falls through to burn-in.
            external_delivery(stream);
        } else if is_image_sub && subtitle_mode == EmbeddedSubtitleHandling::Burn {
            // Extraction infeasible (or the client can't consume PGS
            // externally): an image subtitle can still be burned in. Text
            // subtitles have no burn-in path in the transcode pipeline, so
            // they fall through to the last branch below regardless of
            // subtitle_mode — same as a stream that survived the upstream
            // feasibility filter only because the client's own profile
            // claims some support for it.
            stream.delivery_method = Some(api::SubtitleDeliveryMethod::Encode);
        }
        // Otherwise: nothing we can do — extraction isn't feasible and it's
        // either a text subtitle (no burn-in path) or subtitle_mode is
        // Strip. Should be rare in practice: the upstream feasibility filter
        // (api/playback.rs, before resolve_default_streams) already drops a
        // stream in this state unless the client's own profile explicitly
        // claims some support for it — that one survives here with no
        // delivery method rather than an External URL that would just fail
        // when fetched.
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
            is_live: false,
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
            true,
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
            is_live: false,
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
        let source = make_video_source(VideoContainer::Mkv);
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::ContainerNotSupported(
            "mkv".to_string(),
        ));
        let decision = build_transcode_decision(
            &source,
            &reasons,
            None,
            &force_transcode_query(),
            &session,
            &base_cfg(EncodingOptions::default()),
            true,
        );
        assert!(
            matches!(decision, TranscodeDecision::DirectPlay),
            "a forbidden pure remux should fall back to direct play"
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
        let source = make_video_source(VideoContainer::Mkv);
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::ContainerNotSupported(
            "mkv".to_string(),
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
            true,
        );
        assert!(
            matches!(decision, TranscodeDecision::DirectPlay),
            "a globally forbidden pure remux should fall back to direct play"
        );
    }

    #[test]
    fn remuxing_disabled_does_not_block_a_real_video_transcode() {
        let mut policy = remux_sdks::remux::UserPolicy::default();
        policy.enable_playback_remuxing = false;
        let session = make_session_with_policy(policy);
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
            true,
        ) else {
            panic!("video transcoding remains allowed when only remuxing is disabled");
        };
        assert!(
            outcome
                .url
                .contains("VideoCodec=h264"),
            "{}",
            outcome.url
        );
    }

    #[test]
    fn all_processing_disabled_resolves_requested_codecs_to_copy() {
        let mut policy = remux_sdks::remux::UserPolicy::default();
        policy.enable_playback_remuxing = false;
        policy.enable_video_playback_transcoding = false;
        policy.enable_audio_playback_transcoding = false;
        let session = make_session_with_policy(policy);
        let permissions = PlaybackPermissions::for_user(
            &EncodingOptions::default(),
            Some(&session.user),
        );

        let codecs = permissions.resolve_codecs("h264", "aac", false, false);

        assert_eq!(codecs.video, "copy");
        assert_eq!(codecs.audio, "copy");
        assert!(codecs.direct_play_only);
        assert!(!permissions.processing_available());
    }

    #[test]
    fn same_codec_audio_without_reshaping_is_a_remux_when_remuxing_is_off() {
        let permissions = PlaybackPermissions {
            remuxing: false,
            video_transcoding: true,
            audio_transcoding: true,
        };
        let passthrough = audio_is_passthrough(
            "aac",
            Some("AAC"),
            Some(2),
            Some(192_000),
            None,
            None,
        );
        assert!(passthrough);
        let codecs = permissions.resolve_codecs("copy", "aac", passthrough, false);
        assert!(codecs.direct_play_only);
        assert_eq!(codecs.audio, "aac");

        let downmix =
            audio_is_passthrough("aac", Some("aac"), Some(6), None, Some(2), None);
        assert!(!downmix);
        assert!(
            !permissions
                .resolve_codecs("copy", "aac", downmix, false)
                .direct_play_only
        );
        assert!(!audio_is_passthrough(
            "aac",
            Some("ac3"),
            None,
            None,
            None,
            None
        ));
        assert!(!audio_is_passthrough("aac", None, None, None, None, None));
        assert!(!audio_is_passthrough(
            "aac",
            Some("aac"),
            Some(2),
            None,
            None,
            Some(128_000)
        ));
    }

    #[test]
    fn selected_audio_stream_follows_the_requested_index() {
        let audio = |index, codec: &str| api::MediaStream {
            type_: Some(api::MediaStreamType::Audio),
            index,
            codec: Some(codec.to_string()),
            ..Default::default()
        };
        let source = api::MediaSourceInfo {
            media_streams: vec![audio(1, "truehd"), audio(2, "ac3")],
            ..Default::default()
        };
        let codec = |idx| {
            selected_audio_stream(&source, idx).and_then(|s| {
                s.codec
                    .as_deref()
            })
        };
        assert_eq!(codec(None), Some("truehd"));
        assert_eq!(codec(Some(2)), Some("ac3"));
        assert_eq!(codec(Some(9)), Some("truehd"));
    }

    #[test]
    fn subtitle_burn_respects_global_and_user_video_transcoding_permissions() {
        let mut encoding = EncodingOptions::default();
        encoding.enable_video_transcoding = Some(false);
        let globally_disabled = PlaybackPermissions::for_user(&encoding, None);

        let mut policy = remux_sdks::remux::UserPolicy::default();
        policy.enable_video_playback_transcoding = false;
        let session = make_session_with_policy(policy);
        let user_disabled = PlaybackPermissions::for_user(
            &EncodingOptions::default(),
            Some(&session.user),
        );

        for permissions in [globally_disabled, user_disabled] {
            let codecs = permissions.resolve_codecs("copy", "copy", false, true);
            assert_eq!(codecs.video, "copy");
            assert!(!codecs.burn_subtitle);
        }

        let allowed = PlaybackPermissions::for_user(&EncodingOptions::default(), None)
            .resolve_codecs("copy", "copy", false, true);
        assert_eq!(allowed.video, "h264");
        assert!(allowed.burn_subtitle);
    }

    #[test]
    fn client_direct_stream_report_is_kept_as_reported() {
        let permissions = PlaybackPermissions {
            remuxing: false,
            video_transcoding: true,
            audio_transcoding: true,
        };

        assert_eq!(
            permissions.constrain_reported_method(PlayMethod::DirectStream),
            PlayMethod::DirectStream
        );
        assert_eq!(
            permissions.constrain_reported_method(PlayMethod::Transcode),
            PlayMethod::Transcode
        );
    }

    #[test]
    fn client_transcode_report_is_constrained_when_no_processing_is_allowed() {
        let permissions = PlaybackPermissions {
            remuxing: false,
            video_transcoding: false,
            audio_transcoding: false,
        };

        assert_eq!(
            permissions.constrain_reported_method(PlayMethod::Transcode),
            PlayMethod::DirectPlay
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
            true,
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
            true,
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
            true,
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
            true,
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
            true,
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
            true,
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
            true,
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
    fn burn_in_url_follows_extraction_feasibility_for_external_only_pgs() {
        let session =
            make_session_with_policy(remux_sdks::remux::UserPolicy::default());
        let mut source = make_video_source(VideoContainer::Mkv);
        let stream = |codec: &str, type_, index| api::MediaStream {
            codec: Some(codec.to_string()),
            type_: Some(type_),
            index,
            ..Default::default()
        };
        source.media_streams = vec![
            stream("h264", api::MediaStreamType::Video, 0),
            stream("aac", api::MediaStreamType::Audio, 1),
            stream("hdmv_pgs_subtitle", api::MediaStreamType::Subtitle, 2),
        ];
        let mut cfg = base_cfg(EncodingOptions::default());
        cfg.device_profile = Some(api::DeviceProfile {
            subtitle_profiles: vec![remux_sdks::remux::SubtitleProfile {
                format: Some("pgs".to_string()),
                method: Some(remux_sdks::remux::SubtitleDeliveryMethod::External),
            }],
            ..Default::default()
        });
        cfg.subtitle_mode = EmbeddedSubtitleHandling::Burn;
        let mut reasons = api::TranscodeReasons::default();
        reasons.insert(api::TranscodeReason::SubtitleCodecNotSupported(
            "hdmv_pgs_subtitle".to_string(),
        ));
        let url = |allow_extraction| {
            let TranscodeDecision::Transcode(outcome) = build_transcode_decision(
                &source,
                &reasons,
                Some(2),
                &api::PlaybackInfoQuery::default(),
                &session,
                &cfg,
                allow_extraction,
            ) else {
                panic!("expected a transcode");
            };
            outcome.url
        };

        assert!(url(false).contains("SubtitleMethod=Encode"));
        assert!(!url(true).contains("SubtitleMethod=Encode"));
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
        let decision = build_transcode_decision(
            &source, &reasons, None, &query, &session, &cfg, true,
        );
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
            true,
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

    fn make_subtitle_source(
        text_codec: &str,
        image_codec: &str,
    ) -> api::MediaSourceInfo {
        api::MediaSourceInfo {
            id: Uuid::new_v4(),
            media_streams: vec![
                api::MediaStream {
                    codec: Some(text_codec.to_string()),
                    type_: Some(api::MediaStreamType::Subtitle),
                    is_text_subtitle_stream: true,
                    index: 0,
                    ..Default::default()
                },
                api::MediaStream {
                    codec: Some(image_codec.to_string()),
                    type_: Some(api::MediaStreamType::Subtitle),
                    is_text_subtitle_stream: false,
                    index: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn extraction_feasible_offers_external_delivery_for_text_and_pgs_capable_image_subs()
     {
        // local source, or remote with the setting on: allow_extraction = true.
        // Both text and image subtitles should be offered as External, backed
        // by extraction, regardless of subtitle_mode — but only once the
        // client's profile actually declares it can consume the image
        // format externally (raw PGS; there's no OCR/text conversion for it).
        let mut source = make_subtitle_source("subrip", "hdmv_pgs_subtitle");
        let profile = api::DeviceProfile {
            subtitle_profiles: vec![api::SubtitleProfile {
                format: Some("pgs".to_string()),
                method: Some(api::SubtitleDeliveryMethod::External),
            }],
            ..Default::default()
        };
        apply_subtitle_delivery(
            &mut source,
            Uuid::new_v4(),
            "tok",
            &Some(profile),
            EmbeddedSubtitleHandling::Burn,
            true,
        );
        for stream in &source.media_streams {
            assert_eq!(
                stream.delivery_method,
                Some(api::SubtitleDeliveryMethod::External),
                "stream {:?} should be delivered externally when extraction is feasible \
                 and the client's profile can consume it",
                stream.codec
            );
        }
    }

    #[test]
    fn extraction_feasible_but_client_cant_consume_pgs_falls_back_to_burn() {
        // Regression test: allow_extraction = true but no profile declares
        // PGS support. The image subtitle must NOT be advertised as
        // External — that would produce a .vtt URL the extraction endpoint
        // can't satisfy (no OCR/text conversion for image subtitles) — it
        // should burn in instead when the mode allows it. The text
        // subtitle's extraction never depended on PGS support, so it's
        // unaffected.
        let mut source = make_subtitle_source("subrip", "hdmv_pgs_subtitle");
        apply_subtitle_delivery(
            &mut source,
            Uuid::new_v4(),
            "tok",
            &None,
            EmbeddedSubtitleHandling::Burn,
            true,
        );
        let text = &source.media_streams[0];
        let image = &source.media_streams[1];
        assert_eq!(
            text.delivery_method,
            Some(api::SubtitleDeliveryMethod::External),
            "text subtitle extraction doesn't depend on PGS support"
        );
        assert_eq!(
            image.delivery_method,
            Some(api::SubtitleDeliveryMethod::Encode),
            "image subtitle the client can't consume externally must burn in, \
             not get an unusable vtt URL"
        );
    }

    #[test]
    fn extraction_infeasible_burn_mode_falls_back_to_encode_for_image_only() {
        // remote source, setting off: allow_extraction = false. Burn mode
        // should burn in the image subtitle (Encode) but has no burn-in path
        // for text subtitles, so those get no delivery method at all.
        let mut source = make_subtitle_source("subrip", "hdmv_pgs_subtitle");
        apply_subtitle_delivery(
            &mut source,
            Uuid::new_v4(),
            "tok",
            &None,
            EmbeddedSubtitleHandling::Burn,
            false,
        );
        let text = &source.media_streams[0];
        let image = &source.media_streams[1];
        assert_eq!(
            text.delivery_method, None,
            "text subtitle has no burn-in path and extraction is infeasible"
        );
        assert_eq!(
            image.delivery_method,
            Some(api::SubtitleDeliveryMethod::Encode),
            "image subtitle should burn in when extraction is infeasible"
        );
    }

    #[test]
    fn extraction_infeasible_strip_mode_delivers_neither_stream() {
        let mut source = make_subtitle_source("subrip", "hdmv_pgs_subtitle");
        apply_subtitle_delivery(
            &mut source,
            Uuid::new_v4(),
            "tok",
            &None,
            EmbeddedSubtitleHandling::Strip,
            false,
        );
        for stream in &source.media_streams {
            assert_eq!(
                stream.delivery_method, None,
                "stream {:?} should not be deliverable in strip mode when extraction is infeasible",
                stream.codec
            );
        }
    }

    #[test]
    fn already_external_stream_ignores_extraction_feasibility() {
        // A stream that's already external (e.g. via append_external_subtitles)
        // must keep getting a delivery URL whether or not extraction is
        // feasible — it never needed extraction in the first place.
        let mut source = make_subtitle_source("subrip", "hdmv_pgs_subtitle");
        source.media_streams[0].is_external = true;
        apply_subtitle_delivery(
            &mut source,
            Uuid::new_v4(),
            "real-token",
            &None,
            EmbeddedSubtitleHandling::Strip,
            false,
        );
        let text = &source.media_streams[0];
        assert_eq!(
            text.delivery_method,
            Some(api::SubtitleDeliveryMethod::External)
        );
        assert!(
            text.delivery_url
                .as_deref()
                .unwrap_or_default()
                .contains("ApiKey=real-token"),
            "external stream should still carry a working delivery URL"
        );
    }
}
