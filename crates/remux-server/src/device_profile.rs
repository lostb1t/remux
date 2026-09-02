pub(crate) use remux_sdks::remux::{AudioCodec, SubtitleCodec, VideoCodec};
use remux_sdks::remux::{
    CodecProfile, DeviceProfile, DirectPlayProfile, DlnaProfileType, MediaSourceInfo,
    MediaStream, MediaStreamType, ProfileCondition, SubtitleDeliveryMethod,
    TranscodeReason, TranscodeReasons, TranscodingProfile, TranscodingProtocol,
    VideoContainer,
};

pub trait DeviceProfileExt {
    fn video_transcoding_profile(&self) -> Option<&TranscodingProfile>;
    fn audio_transcoding_profile(&self) -> Option<&TranscodingProfile>;
    fn subtitle_delivery_method(&self, codec: &str) -> Option<SubtitleDeliveryMethod>;
    fn supports_direct_play(&self, media_source: &MediaSourceInfo) -> bool;
    fn check_direct_play(&self, media_source: &MediaSourceInfo) -> TranscodeReasons;
}

pub(crate) fn subtitle_codec_matches_profile(
    codec: &str,
    profile_format: &str,
) -> bool {
    match (
        codec
            .trim()
            .parse::<SubtitleCodec>(),
        profile_format
            .trim()
            .parse::<SubtitleCodec>(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => codec
            .trim()
            .eq_ignore_ascii_case(profile_format.trim()),
    }
}

impl DeviceProfileExt for DeviceProfile {
    fn video_transcoding_profile(&self) -> Option<&TranscodingProfile> {
        let is_video =
            |p: &&TranscodingProfile| matches!(p.type_, Some(DlnaProfileType::Video));
        // Prefer HTTP progressive over HLS: clients like Streamyfin hardcode
        // contentType "video/mp4", so an HLS URL causes the Chromecast to reject.
        self.transcoding_profiles
            .iter()
            .find(|p| {
                is_video(p) && matches!(p.protocol, Some(TranscodingProtocol::Http))
            })
            .or_else(|| {
                self.transcoding_profiles
                    .iter()
                    .find(|p| is_video(p))
            })
    }

    fn audio_transcoding_profile(&self) -> Option<&TranscodingProfile> {
        self.transcoding_profiles
            .iter()
            .find(|p| matches!(p.type_, Some(DlnaProfileType::Audio)))
    }

    fn subtitle_delivery_method(&self, codec: &str) -> Option<SubtitleDeliveryMethod> {
        self.subtitle_profiles
            .iter()
            .find(|p| {
                p.format
                    .as_deref()
                    .map(|f| subtitle_codec_matches_profile(codec, f))
                    .unwrap_or(false)
            })
            .and_then(|p| {
                p.method
                    .clone()
            })
    }

    fn supports_direct_play(&self, media_source: &MediaSourceInfo) -> bool {
        self.check_direct_play(media_source)
            .is_empty()
    }

    fn check_direct_play(&self, media_source: &MediaSourceInfo) -> TranscodeReasons {
        let source_has_video = media_source
            .video_stream()
            .is_some();
        let source_has_audio = media_source
            .audio_stream()
            .is_some();
        // Only treat the source as audio-only when it explicitly has audio but
        // no video. An unprobed source (empty media_streams) should still be
        // matched against Video profiles — we don't know its type yet.
        let is_audio_only = source_has_audio && !source_has_video;
        let mut best: Option<TranscodeReasons> = None;
        for profile in &self.direct_play_profiles {
            if let Some(t) = &profile.type_ {
                if *t == DlnaProfileType::Video && is_audio_only {
                    continue;
                }
                if *t == DlnaProfileType::Audio && source_has_video {
                    continue;
                }
            }
            let mut reasons = profile.check_reasons(media_source);
            // Codec profile conditions (video profile, bit depth, etc.) are a
            // further restriction independent of container/codec-name matching —
            // a profile can't count as a full direct-play match until these pass
            // too. Checking them only in the "nothing matched" fallback below
            // would let e.g. Hi10p through as plain "h264" whenever some
            // direct-play profile already accepts the container and codec name.
            check_codec_profiles(self, media_source, &mut reasons);
            if reasons.is_empty() {
                return reasons;
            }
            best = Some(match best {
                None => reasons,
                Some(prev) => {
                    if reasons
                        .0
                        .len()
                        < prev
                            .0
                            .len()
                    {
                        reasons
                    } else {
                        prev
                    }
                }
            });
        }
        best.unwrap_or_else(|| {
            let mut r = TranscodeReasons::default();
            r.insert(TranscodeReason::ContainerNotSupported(
                "no matching profile".into(),
            ));
            check_codec_profiles(self, media_source, &mut r);
            r
        })
    }
}

fn check_codec_profiles(
    profile: &DeviceProfile,
    media_source: &MediaSourceInfo,
    reasons: &mut TranscodeReasons,
) {
    for cp in &profile.codec_profiles {
        match cp.type_ {
            Some(DlnaProfileType::Video) => {
                if let Some(stream) = media_source.video_stream() {
                    let codec = stream
                        .codec
                        .as_deref()
                        .unwrap_or("");
                    if cp.applies_to_codec(codec) {
                        for r in cp
                            .check_reasons(stream)
                            .0
                        {
                            reasons.insert(r);
                        }
                    }
                }
            }
            Some(DlnaProfileType::Audio) => {
                if let Some(stream) = media_source.audio_stream() {
                    let codec = stream
                        .codec
                        .as_deref()
                        .unwrap_or("");
                    if cp.applies_to_codec(codec) {
                        for r in cp
                            .check_reasons(stream)
                            .0
                        {
                            reasons.insert(r);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub trait DirectPlayProfileExt {
    fn supports_media_source(&self, media_source: &MediaSourceInfo) -> bool;
    fn check_reasons(&self, media_source: &MediaSourceInfo) -> TranscodeReasons;
    fn supports_container(&self, container: &VideoContainer) -> bool;
    fn supports_video_codec(&self, codec: &str) -> bool;
    fn supports_audio_codec(&self, codec: &str) -> bool;
}

impl DirectPlayProfileExt for DirectPlayProfile {
    fn supports_media_source(&self, media_source: &MediaSourceInfo) -> bool {
        self.check_reasons(media_source)
            .is_empty()
    }

    fn check_reasons(&self, media_source: &MediaSourceInfo) -> TranscodeReasons {
        let mut reasons = TranscodeReasons::default();

        // container = None means no restriction (wildcard / absent in profile)
        if self
            .container
            .is_some()
        {
            match &media_source.container {
                None => {
                    reasons.insert(TranscodeReason::ContainerNotSupported(
                        "source container unknown".into(),
                    ));
                }
                Some(source_container) => {
                    if !self.supports_container(source_container) {
                        reasons.insert(TranscodeReason::ContainerNotSupported(
                            format!("source={}", source_container),
                        ));
                    }
                }
            }
        }

        if let Some(video_stream) = media_source.video_stream() {
            if let Some(video_codec) = &video_stream.codec {
                if !self.supports_video_codec(video_codec) {
                    reasons.insert(TranscodeReason::VideoCodecNotSupported(format!(
                        "source={video_codec}"
                    )));
                }
            }
        }

        if let Some(audio_stream) = media_source.audio_stream() {
            if let Some(audio_codec) = &audio_stream.codec {
                if !self.supports_audio_codec(audio_codec) {
                    reasons.insert(TranscodeReason::AudioCodecNotSupported(format!(
                        "source={audio_codec}"
                    )));
                }
            }
        }

        reasons
    }

    fn supports_container(&self, source: &VideoContainer) -> bool {
        let Some(list) = &self.container else {
            return true; // None = any container
        };
        list.iter()
            .any(|c| match (c, source) {
                (VideoContainer::Other(a), VideoContainer::Other(b)) => {
                    a.eq_ignore_ascii_case(b)
                }
                _ => c == source,
            })
    }

    fn supports_video_codec(&self, source: &str) -> bool {
        let Some(list) = &self.video_codec else {
            return true; // None = any codec
        };
        let src: VideoCodec = source
            .parse()
            .unwrap_or_else(|_| VideoCodec::Other(source.to_owned()));
        list.iter()
            .any(|c| match (c, &src) {
                (VideoCodec::Other(a), VideoCodec::Other(b)) => {
                    a.eq_ignore_ascii_case(b)
                }
                _ => c == &src,
            })
    }

    fn supports_audio_codec(&self, source: &str) -> bool {
        let Some(list) = &self.audio_codec else {
            return true; // None = any codec
        };
        let src: AudioCodec = source
            .parse()
            .unwrap_or_else(|_| AudioCodec::Other(source.to_owned()));
        list.iter()
            .any(|c| match (c, &src) {
                (AudioCodec::Other(a), AudioCodec::Other(b)) => {
                    a.eq_ignore_ascii_case(b)
                }
                _ => c == &src,
            })
    }
}

pub trait CodecProfileExt {
    fn applies_to_codec(&self, codec: &str) -> bool;
    fn check_reasons(&self, stream: &MediaStream) -> TranscodeReasons;
}

impl CodecProfileExt for CodecProfile {
    fn applies_to_codec(&self, codec: &str) -> bool {
        let Some(list) = &self.codec else {
            return true; // None = applies to all codecs
        };
        list.iter()
            .any(|entry| any_codec_matches(entry, codec))
    }

    fn check_reasons(&self, stream: &MediaStream) -> TranscodeReasons {
        let mut reasons = TranscodeReasons::default();
        for cond in &self.conditions {
            let property = match cond
                .property
                .as_deref()
            {
                Some(p) => p,
                None => continue,
            };
            let actual = stream_property_value(stream, property);

            // HDR10Plus also satisfies HDR10 conditions.
            if property == "VideoRangeType" {
                if let Some(ref v) = actual {
                    if v.eq_ignore_ascii_case("HDR10Plus")
                        && cond.is_satisfied_opt(Some("HDR10"))
                    {
                        continue;
                    }
                }
            }

            if !cond.is_satisfied_opt(actual.as_deref()) {
                let detail = format!(
                    "property={property} condition={} value={} actual={}",
                    cond.condition
                        .as_deref()
                        .unwrap_or(""),
                    cond.value
                        .as_deref()
                        .unwrap_or(""),
                    actual
                        .as_deref()
                        .unwrap_or("(unknown)"),
                );
                let reason = match property {
                    "VideoRangeType" => {
                        TranscodeReason::VideoRangeTypeNotSupported(detail)
                    }
                    "VideoCodecTag" => {
                        TranscodeReason::VideoCodecTagNotSupported(detail)
                    }
                    "VideoProfile" | "Profile" => {
                        TranscodeReason::VideoProfileNotSupported(detail)
                    }
                    "BitDepth" => TranscodeReason::VideoBitDepthNotSupported(detail),
                    _ => {
                        if matches!(stream.type_, Some(MediaStreamType::Audio)) {
                            TranscodeReason::AudioCodecNotSupported(detail)
                        } else {
                            TranscodeReason::VideoCodecNotSupported(detail)
                        }
                    }
                };
                reasons.insert(reason);
            }
        }
        reasons
    }
}

fn any_codec_matches(entry: &str, source: &str) -> bool {
    // Try VideoCodec first (handles aliasing like h265→Hevc).
    let pe_v: VideoCodec = entry
        .parse()
        .unwrap_or_else(|_| VideoCodec::Other(entry.to_owned()));
    let sc_v: VideoCodec = source
        .parse()
        .unwrap_or_else(|_| VideoCodec::Other(source.to_owned()));
    let video_match = match (&pe_v, &sc_v) {
        (VideoCodec::Other(_), VideoCodec::Other(_)) => false, // defer to audio
        _ => pe_v == sc_v,
    };
    if video_match {
        return true;
    }
    // Fall back to AudioCodec (handles aliases like a52→Ac3, aac_latm→Aac).
    let pe_a: AudioCodec = entry
        .parse()
        .unwrap_or_else(|_| AudioCodec::Other(entry.to_owned()));
    let sc_a: AudioCodec = source
        .parse()
        .unwrap_or_else(|_| AudioCodec::Other(source.to_owned()));
    match (&pe_a, &sc_a) {
        (AudioCodec::Other(a), AudioCodec::Other(b)) => a.eq_ignore_ascii_case(b),
        _ => pe_a == sc_a,
    }
}

fn stream_property_value(stream: &MediaStream, property: &str) -> Option<String> {
    match property {
        "VideoRangeType" => stream
            .video_range_type
            .as_ref()
            .map(|v| {
                v.as_str()
                    .to_string()
            }),
        "VideoCodecTag" => stream
            .codec_tag
            .clone(),
        "IsAnamorphic" => Some(
            stream
                .is_anamorphic
                .unwrap_or(false)
                .to_string(),
        ),
        "IsInterlaced" => Some(
            stream
                .is_interlaced
                .to_string(),
        ),
        "IsAVC" | "IsAvc" => Some(
            stream
                .is_avc
                .unwrap_or(false)
                .to_string(),
        ),
        "BitDepth" => stream
            .bit_depth
            .map(|v| v.to_string()),
        "RefFrames" => stream
            .ref_frames
            .map(|v| v.to_string()),
        "NumAudioStreams" | "NumVideoStreams" => None,
        "VideoLevel" | "Level" => stream
            .level
            .map(|v| v.to_string()),
        "VideoProfile" | "Profile" => stream
            .profile
            .clone(),
        "Height" => stream
            .height
            .map(|v| v.to_string()),
        "Width" => stream
            .width
            .map(|v| v.to_string()),
        "VideoFramerate" | "Framerate" => stream
            .real_frame_rate
            .map(|v| v.to_string()),
        "VideoBitrate" | "Bitrate" | "AudioBitrate" => stream
            .bit_rate
            .map(|v| v.to_string()),
        "AudioChannels" => stream
            .channels
            .map(|v| v.to_string()),
        "AudioSampleRate" => stream
            .sample_rate
            .map(|v| v.to_string()),
        _ => None,
    }
}

pub trait ProfileConditionExt {
    fn is_satisfied_opt(&self, actual: Option<&str>) -> bool;
}

impl ProfileConditionExt for ProfileCondition {
    fn is_satisfied_opt(&self, actual: Option<&str>) -> bool {
        let cond = match self
            .condition
            .as_deref()
        {
            Some(c) => c,
            None => return true,
        };
        let actual = match actual {
            Some(v) if !v.is_empty() => v,
            _ => {
                return !self
                    .is_required
                    .unwrap_or(true);
            }
        };
        let expected = self
            .value
            .as_deref()
            .unwrap_or("");

        match cond {
            "Equals" => actual.eq_ignore_ascii_case(expected),
            "NotEquals" => !actual.eq_ignore_ascii_case(expected),
            "EqualsAny" => expected
                .split('|')
                .any(|v| actual.eq_ignore_ascii_case(v.trim())),
            "LessThanEqual" => {
                if let (Ok(a), Ok(e)) = (actual.parse::<f64>(), expected.parse::<f64>())
                {
                    a <= e
                } else {
                    true
                }
            }
            "GreaterThanEqual" => {
                if let (Ok(a), Ok(e)) = (actual.parse::<f64>(), expected.parse::<f64>())
                {
                    a >= e
                } else {
                    true
                }
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceProfileExt;
    use remux_sdks::remux::{
        AudioCodec, CodecProfile, DeviceProfile, DirectPlayProfile, DlnaProfileType,
        MediaSourceInfo, MediaStream, MediaStreamType, ProfileCondition,
        SubtitleDeliveryMethod, SubtitleProfile, TranscodeReason, VideoCodec,
        VideoContainer,
    };

    #[test]
    fn subtitle_delivery_method_accepts_pgs_aliases() {
        let profile = DeviceProfile {
            subtitle_profiles: vec![SubtitleProfile {
                format: Some("pgs".to_string()),
                method: Some(SubtitleDeliveryMethod::External),
            }],
            ..Default::default()
        };

        assert_eq!(
            profile.subtitle_delivery_method("hdmv_pgs_subtitle"),
            Some(SubtitleDeliveryMethod::External)
        );
    }

    #[test]
    fn direct_play_does_not_reject_aliased_subtitle_codecs() {
        let profile = DeviceProfile {
            direct_play_profiles: vec![DirectPlayProfile {
                container: Some(vec![VideoContainer::Mkv]),
                video_codec: Some(vec![VideoCodec::H264]),
                audio_codec: Some(vec![AudioCodec::Aac]),
                type_: Some(DlnaProfileType::Video),
            }],
            subtitle_profiles: vec![SubtitleProfile {
                format: Some("pgs".to_string()),
                method: Some(SubtitleDeliveryMethod::Embed),
            }],
            ..Default::default()
        };
        let media_source = MediaSourceInfo {
            container: Some(VideoContainer::Mkv),
            default_subtitle_stream_index: Some(2),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(MediaStreamType::Video),
                    index: 0,
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("hdmv_pgs_subtitle".to_string()),
                    type_: Some(MediaStreamType::Subtitle),
                    index: 2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let reasons = profile.check_direct_play(&media_source);
        assert!(
            !reasons.contains(&TranscodeReason::SubtitleCodecNotSupported(
                "hdmv_pgs_subtitle".to_string()
            )),
            "alias-matched subtitle should remain direct-play eligible: {reasons:?}"
        );
    }

    #[test]
    fn test_direct_play_mkv_with_unsupported_embedded_subtitle_codec() {
        // Device profile only supports VTT subtitles (like Roku without direct PGS)
        let profile = DeviceProfile {
            direct_play_profiles: vec![DirectPlayProfile {
                container: Some(vec![VideoContainer::Mkv]),
                video_codec: Some(vec![VideoCodec::H264]),
                audio_codec: Some(vec![AudioCodec::Aac]),
                type_: Some(DlnaProfileType::Video),
            }],
            subtitle_profiles: vec![SubtitleProfile {
                format: Some("vtt".to_string()),
                method: Some(SubtitleDeliveryMethod::External),
            }],
            ..Default::default()
        };
        let media_source = MediaSourceInfo {
            container: Some(VideoContainer::Mkv),
            default_subtitle_stream_index: Some(2),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(MediaStreamType::Video),
                    index: 0,
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("hdmv_pgs_subtitle".to_string()),
                    type_: Some(MediaStreamType::Subtitle),
                    index: 2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let reasons = profile.check_direct_play(&media_source);
        assert!(
            reasons.is_empty(),
            "direct play should be permitted for MKV even when embedded subtitle codec is not in profile: {reasons:?}"
        );
    }

    /// Mirrors a real Android TV device profile: a `DirectPlayProfile` accepts
    /// H264 by codec name alone, but a `CodecProfile` further restricts which
    /// H264 *profiles* are allowed (High/Main/Baseline — no High 10). Hi10p
    /// anime must be rejected here, not silently direct-played as plain h264.
    fn hi10p_rejecting_profile() -> DeviceProfile {
        DeviceProfile {
            direct_play_profiles: vec![DirectPlayProfile {
                container: Some(vec![VideoContainer::Mkv]),
                video_codec: Some(vec![VideoCodec::H264]),
                audio_codec: Some(vec![AudioCodec::Aac]),
                type_: Some(DlnaProfileType::Video),
            }],
            codec_profiles: vec![CodecProfile {
                type_: Some(DlnaProfileType::Video),
                codec: Some(vec!["h264".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some("EqualsAny".to_string()),
                    property: Some("VideoProfile".to_string()),
                    value: Some("high|main|baseline|constrained baseline".to_string()),
                    is_required: Some(false),
                }],
            }],
            ..Default::default()
        }
    }

    fn h264_source(profile: &str) -> MediaSourceInfo {
        MediaSourceInfo {
            container: Some(VideoContainer::Mkv),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(MediaStreamType::Video),
                    index: 0,
                    profile: Some(profile.to_string()),
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn hi10p_h264_is_rejected_despite_matching_codec_name() {
        let reasons =
            hi10p_rejecting_profile().check_direct_play(&h264_source("High 10"));
        assert!(
            reasons.contains(&TranscodeReason::VideoProfileNotSupported(String::new())),
            "Hi10p (High 10 profile) must be rejected even though the container \
             and codec name alone would pass direct play: {reasons:?}"
        );
    }

    #[test]
    fn plain_high_profile_h264_still_direct_plays() {
        // Regression guard: the CodecProfile check must not reject ordinary
        // H264 content that actually is in an allowed profile.
        let reasons = hi10p_rejecting_profile().check_direct_play(&h264_source("High"));
        assert!(
            reasons.is_empty(),
            "plain High-profile H264 should remain direct-play eligible: {reasons:?}"
        );
    }

    #[test]
    fn audio_channel_limit_does_not_force_video_reencode() {
        // A device that only supports 2-channel audio should trigger an audio
        // transcode (AudioCodecNotSupported), not a video re-encode. Before the
        // fix, the catch-all `_ => VideoCodecNotSupported` in check_reasons was
        // reached for "AudioChannels", causing a full h264 re-encode on 5.1 files.
        let profile = DeviceProfile {
            direct_play_profiles: vec![DirectPlayProfile {
                container: Some(vec![VideoContainer::Mkv]),
                video_codec: Some(vec![VideoCodec::H264]),
                audio_codec: Some(vec![AudioCodec::Aac]),
                type_: Some(DlnaProfileType::Video),
            }],
            codec_profiles: vec![CodecProfile {
                type_: Some(DlnaProfileType::Audio),
                codec: Some(vec!["aac".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some("LessThanEqual".to_string()),
                    property: Some("AudioChannels".to_string()),
                    value: Some("2".to_string()),
                    is_required: Some(false),
                }],
            }],
            ..Default::default()
        };
        let source = MediaSourceInfo {
            container: Some(VideoContainer::Mkv),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(MediaStreamType::Video),
                    index: 0,
                    profile: Some("High".to_string()),
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 1,
                    channels: Some(6),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let reasons = profile.check_direct_play(&source);
        assert!(
            reasons.contains(&TranscodeReason::AudioCodecNotSupported(String::new())),
            "a 5.1 channel limit violation should produce AudioCodecNotSupported: {reasons:?}"
        );
        assert!(
            !reasons.contains(&TranscodeReason::VideoCodecNotSupported(String::new())),
            "an audio-only constraint must not produce VideoCodecNotSupported: {reasons:?}"
        );
    }
}
