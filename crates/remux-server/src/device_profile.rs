pub(crate) use remux_sdks::remux::{AudioCodec, SubtitleCodec, VideoCodec};
use remux_sdks::remux::{
    CodecProfile, CodecProfileType, DeviceProfile, DirectPlayProfile, DlnaProfileType,
    EmbeddedSubtitleHandling, MediaSourceInfo, MediaStream, MediaStreamType,
    ProfileCondition, ProfileConditionProperty, ProfileConditionType,
    SortMediaSourcesMode, SubtitleDeliveryMethod, TranscodeReason, TranscodeReasons,
    TranscodingProfile, TranscodingProtocol, VideoContainer, VideoRangeType,
};

pub trait DeviceProfileExt {
    fn video_transcoding_profile(&self) -> Option<&TranscodingProfile>;
    fn audio_transcoding_profile(&self) -> Option<&TranscodingProfile>;
    fn subtitle_delivery_method(&self, codec: &str) -> Option<SubtitleDeliveryMethod>;
    fn supports_direct_play(&self, media_source: &MediaSourceInfo) -> bool;
    fn check_direct_play(&self, media_source: &MediaSourceInfo) -> TranscodeReasons;
    fn hevc_copy_tag(&self, media_source: &MediaSourceInfo) -> &'static str;
}

/// Sample-entry fourcc for HEVC in an MP4-family container. `hvc1` asserts the
/// `hvcC` carries VPS/SPS/PPS out-of-band; `hev1` also permits them in-band.
pub const HEVC_TAG_HVC1: &str = "hvc1";
pub const HEVC_TAG_HEV1: &str = "hev1";

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
                    let reasons_cost = transcode_cost_tier(&reasons);
                    let previous_cost = transcode_cost_tier(&prev);
                    if reasons_cost > previous_cost
                        || (reasons_cost == previous_cost
                            && reasons
                                .0
                                .len()
                                < prev
                                    .0
                                    .len())
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

    /// Which HEVC sample-entry tag to write when we stream-copy HEVC into fMP4.
    ///
    /// Two independent questions decide this, and `hvc1` — today's behaviour,
    /// and what Apple's HLS authoring spec mandates — wins unless both come
    /// back clean:
    ///
    /// 1. *What will the client accept?* Apple clients advertise a
    ///    `VideoCodecTag` condition on their hevc codec profile (Safari sends
    ///    `EqualsAny hvc1|dvh1`); most clients omit it entirely. Asked through
    ///    the client's own conditions, so `Equals`/`NotEquals`/`EqualsAny` are
    ///    all honoured without re-implementing them here.
    /// 2. *Is `hvc1` true for this file?* `hvc1` promises the `hvcC` carries
    ///    VPS/SPS/PPS out-of-band, which is a lie for sources that keep
    ///    parameter sets in-band (some WEB-DL repackages). ffmpeg copies the
    ///    header-only record through verbatim and the resulting empty `hvcC`
    ///    leaves ExoPlayer unable to initialise a decoder. A muxer that put the
    ///    parameter sets in-band said so in its own sample entry, so the
    ///    source's fourcc is the signal.
    ///
    /// A silent client on an ordinary `hvc1` source therefore stays on `hvc1`;
    /// only a source that is itself `hev1` moves, and only when the client
    /// hasn't ruled `hev1` out.
    fn hevc_copy_tag(&self, media_source: &MediaSourceInfo) -> &'static str {
        let client_rejects = |tag: &str| {
            let Some(video_stream) = media_source.video_stream() else {
                return false;
            };
            self.codec_profiles
                .iter()
                .filter(|cp| matches!(cp.type_, None | Some(CodecProfileType::Video)))
                .filter(|cp| cp.applies_to_media(media_source, video_stream, "hevc"))
                .flat_map(|cp| &cp.conditions)
                .filter(|cond| {
                    cond.property
                        .as_ref()
                        == Some(&ProfileConditionProperty::VideoCodecTag)
                })
                .any(|cond| !cond.is_satisfied_opt(Some(tag)))
        };

        // A declared constraint is the client telling us outright. Checked
        // hvc1-first so a contradictory profile that rejects both still lands
        // on today's behaviour.
        if client_rejects(HEVC_TAG_HEV1) {
            return HEVC_TAG_HVC1;
        }
        if client_rejects(HEVC_TAG_HVC1) {
            return HEVC_TAG_HEV1;
        }

        // The client takes either, so keep hvc1 unless the source itself says
        // its parameter sets are in-band.
        let source_is_hev1 = media_source
            .video_stream()
            .and_then(|s| {
                s.codec_tag
                    .as_deref()
            })
            .is_some_and(|tag| tag.eq_ignore_ascii_case(HEVC_TAG_HEV1));

        if source_is_hev1 {
            HEVC_TAG_HEV1
        } else {
            HEVC_TAG_HVC1
        }
    }
}

fn check_codec_profiles(
    profile: &DeviceProfile,
    media_source: &MediaSourceInfo,
    reasons: &mut TranscodeReasons,
) {
    let has_video = media_source
        .video_stream()
        .is_some();
    for cp in &profile.codec_profiles {
        let stream = match &cp.type_ {
            None | Some(CodecProfileType::Video) => media_source.video_stream(),
            Some(CodecProfileType::VideoAudio) if has_video => {
                selected_audio_stream(media_source)
            }
            Some(CodecProfileType::Audio) if !has_video => {
                selected_audio_stream(media_source)
            }
            _ => None,
        };
        let Some(stream) = stream else {
            continue;
        };
        let codec = stream
            .codec
            .as_deref()
            .unwrap_or("");
        if cp.applies_to_media(media_source, stream, codec) {
            for reason in cp
                .check_reasons(media_source, stream)
                .0
            {
                reasons.insert(reason);
            }
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

        if let Some(audio_stream) = selected_audio_stream(media_source) {
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
    fn applies_to_media(
        &self,
        media_source: &MediaSourceInfo,
        stream: &MediaStream,
        codec: &str,
    ) -> bool;
    fn check_reasons(
        &self,
        media_source: &MediaSourceInfo,
        stream: &MediaStream,
    ) -> TranscodeReasons;
}

impl CodecProfileExt for CodecProfile {
    fn applies_to_codec(&self, codec: &str) -> bool {
        let Some(list) = &self.codec else {
            return true; // None = applies to all codecs
        };
        list.iter()
            .any(|entry| any_codec_matches(entry, codec))
    }

    fn applies_to_media(
        &self,
        media_source: &MediaSourceInfo,
        stream: &MediaStream,
        codec: &str,
    ) -> bool {
        if !self.applies_to_codec(codec) {
            return false;
        }
        if let Some(containers) = self
            .container
            .as_deref()
            .filter(|value| {
                !value
                    .trim()
                    .is_empty()
            })
        {
            let Some(source_container) = media_source
                .container
                .as_ref()
            else {
                return false;
            };
            if !containers
                .split(',')
                .map(str::trim)
                .any(|candidate| {
                    candidate == "*"
                        || container_name_matches(candidate, source_container)
                })
            {
                return false;
            }
        }
        self.apply_conditions
            .iter()
            .all(|condition| condition_satisfied(condition, media_source, stream))
    }

    fn check_reasons(
        &self,
        media_source: &MediaSourceInfo,
        stream: &MediaStream,
    ) -> TranscodeReasons {
        let mut reasons = TranscodeReasons::default();
        for cond in &self.conditions {
            let property = match cond
                .property
                .as_ref()
            {
                Some(p) => p,
                None => continue,
            };
            let actual = condition_property_value(media_source, stream, property);
            if !condition_satisfied_for_value(cond, property, &actual) {
                let condition = cond
                    .condition
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                let detail = format!(
                    "property={property} condition={} value={} actual={}",
                    condition,
                    cond.value
                        .as_deref()
                        .unwrap_or(""),
                    actual.detail(),
                );
                if let Some(reason) = failed_condition_reason(property, stream, detail)
                {
                    reasons.insert(reason);
                }
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

fn container_name_matches(candidate: &str, source: &VideoContainer) -> bool {
    let parsed = candidate
        .parse::<VideoContainer>()
        .unwrap_or_else(|_| VideoContainer::Other(candidate.to_string()));
    match (&parsed, source) {
        (VideoContainer::Other(a), VideoContainer::Other(b)) => {
            a.eq_ignore_ascii_case(b)
        }
        _ => &parsed == source,
    }
}

fn condition_satisfied(
    condition: &ProfileCondition,
    media_source: &MediaSourceInfo,
    stream: &MediaStream,
) -> bool {
    let Some(property) = condition
        .property
        .as_ref()
    else {
        return true;
    };
    let actual = condition_property_value(media_source, stream, property);
    condition_satisfied_for_value(condition, property, &actual)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConditionValue {
    Known(String),
    Missing,
    Unsupported,
}

impl ConditionValue {
    fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Known(value) => Some(value),
            Self::Missing | Self::Unsupported => None,
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Known(value) => value,
            Self::Missing => "(unknown)",
            Self::Unsupported => "(unsupported)",
        }
    }
}

fn optional_condition_value<T: ToString>(value: Option<T>) -> ConditionValue {
    value
        .map(|value| ConditionValue::Known(value.to_string()))
        .unwrap_or(ConditionValue::Missing)
}

fn condition_satisfied_for_value(
    condition: &ProfileCondition,
    property: &ProfileConditionProperty,
    actual: &ConditionValue,
) -> bool {
    if matches!(actual, ConditionValue::Unsupported) {
        return true;
    }
    let actual = actual.as_deref();
    // Jellyfin treats HDR10+ as satisfying HDR10 compatibility constraints.
    // Keep this rule shared by both Conditions and ApplyConditions.
    if property == &ProfileConditionProperty::VideoRangeType
        && actual.is_some_and(|value| value.eq_ignore_ascii_case("HDR10Plus"))
        && condition.is_satisfied_opt(Some("HDR10"))
    {
        return true;
    }
    condition.is_satisfied_opt(actual)
}

fn condition_property_value(
    media_source: &MediaSourceInfo,
    stream: &MediaStream,
    property: &ProfileConditionProperty,
) -> ConditionValue {
    match property {
        ProfileConditionProperty::VideoRangeType => optional_condition_value(
            stream
                .video_range_type
                .as_ref()
                .map(|value| value.as_str()),
        ),
        ProfileConditionProperty::VideoCodecTag => optional_condition_value(
            stream
                .codec_tag
                .as_deref(),
        ),
        ProfileConditionProperty::IsAnamorphic => {
            optional_condition_value(stream.is_anamorphic)
        }
        ProfileConditionProperty::IsInterlaced => {
            optional_condition_value(Some(stream.is_interlaced))
        }
        ProfileConditionProperty::IsAvc => optional_condition_value(stream.is_avc),
        ProfileConditionProperty::VideoBitDepth
        | ProfileConditionProperty::BitDepth
        | ProfileConditionProperty::AudioBitDepth => {
            optional_condition_value(stream.bit_depth)
        }
        ProfileConditionProperty::RefFrames => optional_condition_value(
            stream
                .ref_frames
                .filter(|frames| *frames > 0),
        ),
        ProfileConditionProperty::NumStreams => ConditionValue::Known(
            media_source
                .media_streams
                .len()
                .to_string(),
        ),
        ProfileConditionProperty::NumAudioStreams => ConditionValue::Known(
            media_source
                .media_streams
                .iter()
                .filter(|stream| matches!(stream.type_, Some(MediaStreamType::Audio)))
                .count()
                .to_string(),
        ),
        ProfileConditionProperty::NumVideoStreams => ConditionValue::Known(
            media_source
                .media_streams
                .iter()
                .filter(|stream| matches!(stream.type_, Some(MediaStreamType::Video)))
                .count()
                .to_string(),
        ),
        ProfileConditionProperty::VideoLevel | ProfileConditionProperty::Level => {
            optional_condition_value(stream.level)
        }
        ProfileConditionProperty::VideoProfile | ProfileConditionProperty::Profile => {
            optional_condition_value(
                stream
                    .profile
                    .as_deref(),
            )
        }
        ProfileConditionProperty::Height => optional_condition_value(stream.height),
        ProfileConditionProperty::Width => optional_condition_value(stream.width),
        ProfileConditionProperty::VideoFramerate
        | ProfileConditionProperty::Framerate => {
            optional_condition_value(stream.real_frame_rate)
        }
        ProfileConditionProperty::VideoRotation => {
            optional_condition_value(stream.rotation)
        }
        ProfileConditionProperty::VideoBitrate
        | ProfileConditionProperty::Bitrate
        | ProfileConditionProperty::AudioBitrate => {
            optional_condition_value(stream.bit_rate)
        }
        ProfileConditionProperty::AudioChannels => {
            optional_condition_value(stream.channels)
        }
        ProfileConditionProperty::AudioProfile => optional_condition_value(
            stream
                .profile
                .as_deref(),
        ),
        ProfileConditionProperty::AudioSampleRate => {
            optional_condition_value(stream.sample_rate)
        }
        ProfileConditionProperty::PacketLength => {
            optional_condition_value(stream.packet_length)
        }
        ProfileConditionProperty::IsSecondaryAudio => {
            optional_condition_value(is_secondary_audio(media_source, stream))
        }
        ProfileConditionProperty::Has64BitOffsets
        | ProfileConditionProperty::VideoTimestamp
        | ProfileConditionProperty::Other(_) => ConditionValue::Unsupported,
    }
}

fn failed_condition_reason(
    property: &ProfileConditionProperty,
    stream: &MediaStream,
    detail: String,
) -> Option<TranscodeReason> {
    match property {
        ProfileConditionProperty::VideoRangeType => {
            Some(TranscodeReason::VideoRangeTypeNotSupported(detail))
        }
        ProfileConditionProperty::VideoCodecTag => {
            Some(TranscodeReason::VideoCodecTagNotSupported(detail))
        }
        ProfileConditionProperty::VideoProfile | ProfileConditionProperty::Profile => {
            Some(TranscodeReason::VideoProfileNotSupported(detail))
        }
        ProfileConditionProperty::VideoBitDepth
        | ProfileConditionProperty::BitDepth => {
            Some(TranscodeReason::VideoBitDepthNotSupported(detail))
        }
        ProfileConditionProperty::VideoLevel | ProfileConditionProperty::Level => {
            Some(TranscodeReason::VideoLevelNotSupported(detail))
        }
        ProfileConditionProperty::Width | ProfileConditionProperty::Height => {
            Some(TranscodeReason::VideoResolutionNotSupported(detail))
        }
        ProfileConditionProperty::VideoFramerate
        | ProfileConditionProperty::Framerate => {
            Some(TranscodeReason::VideoFramerateNotSupported(detail))
        }
        ProfileConditionProperty::VideoRotation => {
            Some(TranscodeReason::VideoRotationNotSupported(detail))
        }
        ProfileConditionProperty::VideoBitrate => {
            Some(TranscodeReason::VideoBitrateNotSupported(detail))
        }
        ProfileConditionProperty::RefFrames => {
            Some(TranscodeReason::RefFramesNotSupported(detail))
        }
        ProfileConditionProperty::IsAnamorphic => {
            Some(TranscodeReason::AnamorphicVideoNotSupported(detail))
        }
        ProfileConditionProperty::IsInterlaced => {
            Some(TranscodeReason::InterlacedVideoNotSupported(detail))
        }
        ProfileConditionProperty::AudioChannels => {
            Some(TranscodeReason::AudioChannelsNotSupported(detail))
        }
        ProfileConditionProperty::AudioProfile => {
            Some(TranscodeReason::AudioProfileNotSupported(detail))
        }
        ProfileConditionProperty::AudioSampleRate => {
            Some(TranscodeReason::AudioSampleRateNotSupported(detail))
        }
        ProfileConditionProperty::AudioBitDepth => {
            Some(TranscodeReason::AudioBitDepthNotSupported(detail))
        }
        ProfileConditionProperty::AudioBitrate => {
            Some(TranscodeReason::AudioBitrateNotSupported(detail))
        }
        ProfileConditionProperty::IsSecondaryAudio => {
            Some(TranscodeReason::SecondaryAudioNotSupported(detail))
        }
        ProfileConditionProperty::NumStreams => {
            Some(TranscodeReason::StreamCountExceedsLimit(detail))
        }
        ProfileConditionProperty::Bitrate => {
            if matches!(stream.type_, Some(MediaStreamType::Audio)) {
                Some(TranscodeReason::AudioBitrateNotSupported(detail))
            } else {
                Some(TranscodeReason::VideoBitrateNotSupported(detail))
            }
        }
        ProfileConditionProperty::Has64BitOffsets
        | ProfileConditionProperty::PacketLength
        | ProfileConditionProperty::VideoTimestamp
        | ProfileConditionProperty::IsAvc
        | ProfileConditionProperty::NumAudioStreams
        | ProfileConditionProperty::NumVideoStreams => None,
        ProfileConditionProperty::Other(_) => None,
    }
}

fn selected_audio_stream(source: &MediaSourceInfo) -> Option<&MediaStream> {
    source
        .default_audio_stream_index
        .and_then(|index| {
            source
                .media_streams
                .iter()
                .find(|stream| {
                    stream.index == index
                        && matches!(stream.type_, Some(MediaStreamType::Audio))
                })
        })
        .or_else(|| source.audio_stream())
}

fn is_secondary_audio(source: &MediaSourceInfo, stream: &MediaStream) -> Option<bool> {
    if stream.is_external {
        return Some(false);
    }
    source
        .media_streams
        .iter()
        .find(|candidate| {
            matches!(candidate.type_, Some(MediaStreamType::Audio))
                && !candidate.is_external
        })
        .map(|primary| primary.index != stream.index)
}

pub trait ProfileConditionExt {
    fn is_satisfied_opt(&self, actual: Option<&str>) -> bool;
}

impl ProfileConditionExt for ProfileCondition {
    fn is_satisfied_opt(&self, actual: Option<&str>) -> bool {
        let cond = match self
            .condition
            .as_ref()
        {
            Some(c) => c,
            None => return true,
        };
        let actual = match actual {
            Some(v) if !v.is_empty() => v,
            _ => {
                return !self
                    .is_required
                    .unwrap_or(false);
            }
        };
        let expected = self
            .value
            .as_deref()
            .unwrap_or("");

        match cond {
            ProfileConditionType::Equals => actual.eq_ignore_ascii_case(expected),
            ProfileConditionType::NotEquals => !actual.eq_ignore_ascii_case(expected),
            ProfileConditionType::EqualsAny => expected
                .split('|')
                .any(|v| actual.eq_ignore_ascii_case(v.trim())),
            ProfileConditionType::LessThanEqual => {
                if let (Ok(a), Ok(e)) = (actual.parse::<f64>(), expected.parse::<f64>())
                {
                    a <= e
                } else {
                    false
                }
            }
            ProfileConditionType::GreaterThanEqual => {
                if let (Ok(a), Ok(e)) = (actual.parse::<f64>(), expected.parse::<f64>())
                {
                    a >= e
                } else {
                    false
                }
            }
            _ => true,
        }
    }
}

/// Individual quality/compatibility signals for one `MediaSourceInfo`, from
/// which a `SortMediaSourcesMode`-specific sort key is built via `.key()`.
/// Every field is "higher is better".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaSourceRank {
    cached: bool,
    bitrate_plausibility_tier: u8,
    transcode_cost_tier: u8,
    resolution_tier: u8,
    hdr_class: u8,
    hdr_tier: u8,
    bit_depth: i64,
    quality_source_tier: u8,
    audio_tier: u8,
    audio_channels: i64,
    bitrate: i64,
}

/// Compared lexicographically. Confirmed cache availability precedes quality
/// and playback cost. Plausibility precedes cost in Best/Quality mode, but
/// follows it in Compatibility mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MediaSourceSortKey {
    cached: bool,
    plausibility_before_cost: u8,
    cost: u8,
    plausibility_after_cost: u8,
    resolution: u8,
    hdr_class: u8,
    release_quality: u8,
    bitrate: i64,
    bit_depth: i64,
    audio_tier: u8,
    audio_channels: i64,
    hdr_variant: u8,
}

/// All request-independent inputs used to rank sources. Bitrate limits are
/// intentionally absent: they are live playback policy, not a durable device
/// capability and therefore must never affect source ordering.
#[derive(Debug, Clone, Copy)]
pub struct SourceRankingContext<'a> {
    pub mode: SortMediaSourcesMode,
    pub device_profile: Option<&'a DeviceProfile>,
    pub subtitle_mode: EmbeddedSubtitleHandling,
    pub explicit_subtitle_index: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SourceAssessment {
    pub reasons: TranscodeReasons,
    rank: MediaSourceRank,
}

impl SourceAssessment {
    pub fn key(&self, mode: SortMediaSourcesMode) -> MediaSourceSortKey {
        self.rank
            .key(mode)
    }

    pub fn playback_label(&self) -> &'static str {
        playback_decision_label(&self.reasons)
    }
}

impl SourceRankingContext<'_> {
    pub fn assess(&self, source: &MediaSourceInfo) -> SourceAssessment {
        let reasons = compute_transcode_reasons(
            source,
            self.device_profile,
            self.subtitle_mode,
            self.explicit_subtitle_index,
            None,
        );
        let rank = source.capability_rank(self.device_profile, &reasons);
        SourceAssessment { reasons, rank }
    }

    pub fn key(&self, source: &MediaSourceInfo) -> MediaSourceSortKey {
        self.assess(source)
            .key(self.mode)
    }
}

impl MediaSourceRank {
    /// Sort key for `mode` — a *greater* value is a *better* match. Compared
    /// lexicographically, most significant tier first.
    ///
    /// A confirmed upstream cache hit always wins. Then `Compatibility` puts
    /// `transcode_cost_tier` first: a version that direct
    /// plays (or only needs a cheap remux) always outranks one that needs a
    /// real re-encode, no matter how much better it looks on paper. `Best`
    /// and `Quality` first demote only severely under-bitrated video sources.
    /// `Quality` drops cost from the key entirely — best quality wins even if
    /// it means transcoding. `Disabled` has no key; callers must not sort.
    /// All HDR formats share a primary tier; release quality and bitrate
    /// outrank HDR subtype, which is only a late tie-breaker.
    ///
    /// There's no separate subtitle field: whether the resolved default
    /// subtitle needs burning in is already reflected in
    /// `transcode_cost_tier` via `TranscodeReason::SubtitleCodecNotSupported`
    /// (inserted in `api/playback.rs`, scoped to the one subtitle stream that
    /// will actually be used and to `EmbeddedSubtitleHandling::Burn` mode) —
    /// re-deriving it here from every embedded subtitle stream would ignore
    /// which one is actually selected and double-count the same fact.
    pub fn key(&self, mode: SortMediaSourcesMode) -> MediaSourceSortKey {
        let cost = match mode {
            SortMediaSourcesMode::Compatibility => self.transcode_cost_tier,
            // Collapse Direct Play (4) and Direct Stream (3) into one tier —
            // both are low-overhead, so quality (the rest of the tuple)
            // picks the winner between them. Audio (2) and video (1)
            // re-encodes stay distinct and still rank below both.
            SortMediaSourcesMode::Best => match self.transcode_cost_tier {
                4 | 3 => 2,
                2 => 1,
                _ => 0,
            },
            // Every source ties on this field, so the rest of the tuple
            // (pure quality) decides the order.
            SortMediaSourcesMode::Quality => 0,
            SortMediaSourcesMode::Disabled => {
                debug_assert!(
                    false,
                    "capability_rank().key() called with mode Disabled"
                );
                0
            }
        };
        let (plausibility_before_cost, plausibility_after_cost) = match mode {
            SortMediaSourcesMode::Compatibility => (1, self.bitrate_plausibility_tier),
            _ => (self.bitrate_plausibility_tier, 0),
        };
        MediaSourceSortKey {
            cached: self.cached,
            plausibility_before_cost,
            cost,
            plausibility_after_cost,
            resolution: self.resolution_tier,
            hdr_class: self.hdr_class,
            release_quality: self.quality_source_tier,
            bitrate: self.bitrate,
            bit_depth: self.bit_depth,
            audio_tier: self.audio_tier,
            audio_channels: self.audio_channels,
            hdr_variant: self.hdr_tier,
        }
    }
}

/// How expensive the transcode implied by `reasons` is — higher is cheaper.
/// Grouped by which part of the pipeline actually has to do work: a
/// container/tag/subtitle mismatch is a remux (near-free, video and audio both
/// copied); an audio-codec mismatch means re-encoding just the audio track
/// (video still copied); anything touching the video codec/profile/HDR
/// range/bit depth forces a full video re-encode — by far the most expensive
/// and quality-lossy case, and the one "Compatibility" mode exists to avoid.
fn transcode_cost_tier(reasons: &TranscodeReasons) -> u8 {
    if reasons.is_empty() {
        return 4;
    }
    // SubtitleCodecNotSupported is only ever inserted when the resolved
    // default subtitle must be burned in (see api/playback.rs) — burning
    // text into frames means re-encoding the video, same cost as an
    // incompatible video codec/profile/range/bit depth. ContainerBitrateExceedsLimit
    // belongs here too: `build_video_transcode` (playback/decision.rs) treats it
    // as needing a video re-encode (drops to H.264), not a plain remux.
    let needs_video_reencode = reasons
        .0
        .iter()
        .any(TranscodeReason::is_video)
        || reasons
            .0
            .iter()
            .any(|reason| {
                matches!(
                    reason,
                    TranscodeReason::SubtitleCodecNotSupported(_)
                        | TranscodeReason::ContainerBitrateExceedsLimit
                )
            });
    if needs_video_reencode {
        return 1;
    }
    let needs_audio_reencode = reasons
        .0
        .iter()
        .any(TranscodeReason::is_audio);
    if needs_audio_reencode {
        return 2;
    }
    // Only cheap reasons left: ContainerNotSupported, VideoCodecTagNotSupported —
    // a remux, not a re-encode (video and audio both copied).
    3
}

/// Whether the subtitle stream that will actually be used needs to be burned
/// into the video for `device_profile` — shared by `PlaybackInfo` (the real
/// transcode decision) and item-details ranking, so both agree on the same
/// fact instead of one of them silently ignoring subtitles.
///
/// Only ever fires in `EmbeddedSubtitleHandling::Burn`: `Extract` converts
/// the subtitle instead (OCR/sidecar) and `Strip` drops it, so neither ever
/// forces a transcode. `explicit_subtitle_index` (a client's requested
/// `SubtitleStreamIndex`, if any) takes priority over
/// `source.default_subtitle_stream_index` — call `resolve_default_streams`
/// first so that fallback reflects the real selection, not container order.
pub fn subtitle_burn_reason(
    source: &MediaSourceInfo,
    device_profile: Option<&DeviceProfile>,
    subtitle_mode: EmbeddedSubtitleHandling,
    explicit_subtitle_index: Option<i64>,
) -> Option<TranscodeReason> {
    if subtitle_mode != EmbeddedSubtitleHandling::Burn {
        return None;
    }
    let idx = explicit_subtitle_index.or(source.default_subtitle_stream_index)?;
    let stream = source
        .media_streams
        .iter()
        .find(|s| {
            s.index == idx && matches!(s.type_, Some(MediaStreamType::Subtitle))
        })?;
    if stream.is_external || stream.is_text_subtitle_stream() {
        return None;
    }
    let codec = stream
        .codec
        .as_deref()?;
    let supported = device_profile
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
    if supported {
        None
    } else {
        Some(TranscodeReason::SubtitleCodecNotSupported(
            codec.to_string(),
        ))
    }
}

/// The full set of device-profile-driven transcode reasons for `source`:
/// container/codec incompatibility (`check_direct_play`), the bitrate cap,
/// and (in Burn mode) the resolved subtitle needing to be burned in. This is
/// the single source of truth both `PlaybackInfo` and item-details ranking
/// build from, so neither one silently ignores a reason the other applies —
/// call `resolve_default_streams` on `source` first so the subtitle check
/// judges the real selection, not container order.
///
/// Deliberately does *not* include transport-specific cases like "RTSP can
/// never direct play" — those depend on the raw stream descriptor, not
/// anything `MediaSourceInfo` carries, and only matter to the real playback
/// decision, not to ranking.
pub fn compute_transcode_reasons(
    source: &MediaSourceInfo,
    device_profile: Option<&DeviceProfile>,
    subtitle_mode: EmbeddedSubtitleHandling,
    explicit_subtitle_index: Option<i64>,
    max_bitrate: Option<i64>,
) -> TranscodeReasons {
    let mut reasons = device_profile
        .map(|profile| profile.check_direct_play(source))
        .unwrap_or_default();
    // Only flag bitrate exceeded when the source bitrate is known and
    // actually exceeds the cap. An unknown bitrate is treated as within
    // limits so that clients with a high/unlimited cap aren't forced into
    // transcoding unnecessarily.
    let bitrate_exceeded = max_bitrate.is_some_and(|max| {
        source
            .bitrate
            .is_some_and(|b| b > max)
    });
    if bitrate_exceeded {
        reasons.insert(TranscodeReason::ContainerBitrateExceedsLimit);
    }
    if let Some(reason) = subtitle_burn_reason(
        source,
        device_profile,
        subtitle_mode,
        explicit_subtitle_index,
    ) {
        reasons.insert(reason);
    }
    reasons
}

/// Jellyfin's own three-way playback decision, derived from the same cost
/// tiering used for ranking: tier 4 (no reasons) is Direct Play, tier 3
/// (remux only — video and audio both copied) is Direct Stream, anything
/// below that (an actual audio or video re-encode) is Transcode.
pub fn playback_decision_label(reasons: &TranscodeReasons) -> &'static str {
    match transcode_cost_tier(reasons) {
        4 => "Direct Play",
        3 => "Direct Stream",
        _ => "Transcode",
    }
}

/// Appends "<bitrate> Mbps (Direct Play|Direct Stream|Transcode)" to a video
/// stream's `DisplayTitle` — the bitrate first, decision label last.
/// `source_bitrate` is the MediaSource's overall bitrate, used when the video
/// stream itself doesn't report its own (common: many releases only carry an
/// overall container bitrate, dominated by video, not a per-stream one).
pub fn annotate_video_display_title(
    video: &mut MediaStream,
    source_bitrate: Option<i64>,
    reasons: &TranscodeReasons,
) {
    let label = playback_decision_label(reasons);
    let bitrate_str = video
        .bit_rate
        .or(source_bitrate)
        .filter(|b| *b > 0)
        .map(|b| format!("{:.1} Mbps ", b as f64 / 1_000_000.0))
        .unwrap_or_default();
    let title = video
        .display_title
        .get_or_insert_with(String::new);
    *title = format!("{title} {bitrate_str}({label})");
}

/// True only when the profile gives an explicit, numeric signal that the
/// device can handle 4K — an HEVC/AV1 `VideoLevel` condition at or above the
/// 4K tier, or a `Width`/`Height` condition capping at/above 3840x2160.
/// Absence of such a condition means "unknown", not "no" — callers must not
/// treat that as a reason to rank a source lower, only as a reason not to
/// let resolution drive ranking at all.
fn confident_4k_capable(profile: &DeviceProfile) -> bool {
    const HEVC_4K_LEVEL: i64 = 150; // HEVC Level 5.0
    const AV1_4K_LEVEL: i64 = 13; // AV1 Level 5.0

    for cp in &profile.codec_profiles {
        if !matches!(cp.type_, None | Some(CodecProfileType::Video)) {
            continue;
        }
        let is_hevc = codec_list_contains(&cp.codec, &["hevc", "h265"]);
        let is_av1 = codec_list_contains(&cp.codec, &["av1"]);
        for cond in &cp.conditions {
            let Some(property) = cond
                .property
                .as_ref()
            else {
                continue;
            };
            let Some(value) = cond
                .value
                .as_deref()
                .and_then(|v| {
                    v.parse::<i64>()
                        .ok()
                })
            else {
                continue;
            };
            let confident = match property {
                ProfileConditionProperty::Width => value >= 3840,
                ProfileConditionProperty::Height => value >= 2160,
                ProfileConditionProperty::VideoLevel
                | ProfileConditionProperty::Level => {
                    (is_hevc && value >= HEVC_4K_LEVEL)
                        || (is_av1 && value >= AV1_4K_LEVEL)
                }
                _ => false,
            };
            if confident {
                return true;
            }
        }
    }
    false
}

fn codec_list_contains(list: &Option<Vec<String>>, wanted: &[&str]) -> bool {
    let Some(list) = list else {
        return false;
    };
    list.iter()
        .any(|c| {
            wanted
                .iter()
                .any(|w| c.eq_ignore_ascii_case(w))
        })
}

fn primary_video_stream(source: &MediaSourceInfo) -> Option<&MediaStream> {
    source
        .media_streams
        .iter()
        .find(|s| matches!(s.type_, Some(MediaStreamType::Video)))
}

fn hdr_tier(stream: Option<&MediaStream>) -> u8 {
    match stream.and_then(|s| {
        s.video_range_type
            .as_ref()
    }) {
        Some(VideoRangeType::Dovi) | Some(VideoRangeType::DoviWithHdr10) => 5,
        Some(VideoRangeType::Hdr10Plus) => 4,
        Some(VideoRangeType::Hdr10) => 3,
        Some(VideoRangeType::Hlg) | Some(VideoRangeType::DoviWithHlg) => 2,
        Some(VideoRangeType::Sdr)
        | Some(VideoRangeType::DoviWithSdr)
        | Some(VideoRangeType::Other) => 1,
        None => 0,
    }
}

fn audio_codec_tier(stream: Option<&MediaStream>) -> u8 {
    let Some(codec) = stream.and_then(|s| {
        s.codec
            .as_deref()
    }) else {
        return 0;
    };
    match codec.parse::<AudioCodec>() {
        Ok(AudioCodec::TrueHd) | Ok(AudioCodec::Dts) => 3,
        Ok(AudioCodec::Eac3) => 2,
        Ok(AudioCodec::Ac3) => 1,
        _ => 0,
    }
}

/// Rank a `MediaSourceInfo` against a device's capabilities — call as
/// `source.capability_rank(profile, reasons)` and sort with
/// `sort_by_key(|s| Reverse(s.capability_rank(profile, reasons).key(mode)))`,
/// higher is better.
///
/// `reasons` is taken as a parameter rather than read from
/// `self.transcoding_reasons` on purpose: the profile used for *ranking* can
/// differ from the one used for the actual transcode decision (a request
/// missing a live DeviceProfile falls back to a persisted one for sorting
/// only, to avoid a stale profile causing a wrong transcode action) — pass
/// `compute_transcode_reasons(source, profile, ...)` built against whichever
/// profile you're ranking with.
pub trait MediaSourceCapabilityExt {
    fn capability_rank(
        &self,
        profile: Option<&DeviceProfile>,
        reasons: &TranscodeReasons,
    ) -> MediaSourceRank;
}

/// Release-source weight (remux > BluRay > WEB-DL > WEBRip > ...), parsed from
/// the original release filename the same way pre-probe candidate ordering
/// does. `MediaSourceInfo` has no filename field of its own, but
/// `conversions.rs` always stamps the original `StreamInfo` (as JSON) into
/// `remux.provider_info`, so that's recovered here instead of threading a
/// second parameter through every call site.
fn source_stream_info(source: &MediaSourceInfo) -> Option<crate::stream::StreamInfo> {
    source
        .remux
        .as_ref()
        .and_then(|r| {
            r.provider_info
                .as_ref()
        })
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// Only demote extreme bitrate/resolution mismatches. One bit per four pixels
/// per second corresponds to about 0.5 Mbps at 1080p or 2 Mbps at 4K; this
/// is a plausibility check, not an estimate of visual quality. Below 100 Kbps
/// is suspect even without dimensions; otherwise missing facts stay neutral.
fn bitrate_plausibility_tier(
    source: &MediaSourceInfo,
    video: Option<&MediaStream>,
) -> u8 {
    let Some(bitrate) = source
        .bitrate
        .filter(|bps| *bps > 0)
    else {
        return 1;
    };
    if bitrate < 100_000 {
        return 0;
    }
    let Some((width, height)) = video.and_then(|stream| {
        Some((
            stream
                .width
                .filter(|v| *v > 0)?,
            stream
                .height
                .filter(|v| *v > 0)?,
        ))
    }) else {
        return 1;
    };
    u8::from((bitrate as i128) * 4 >= (width as i128) * (height as i128))
}

impl MediaSourceCapabilityExt for MediaSourceInfo {
    /// A missing profile disables capability-specific checks but still leaves
    /// the intrinsic quality fields available as deterministic tie-breakers.
    fn capability_rank(
        &self,
        profile: Option<&DeviceProfile>,
        reasons: &TranscodeReasons,
    ) -> MediaSourceRank {
        let video = primary_video_stream(self);
        let audio = selected_audio_stream(self);
        let stream_info = source_stream_info(self);

        let (width, height) = video
            .map(|stream| {
                (
                    stream
                        .width
                        .unwrap_or(0),
                    stream
                        .height
                        .unwrap_or(0),
                )
            })
            .unwrap_or_default();
        // Release dimensions are often cropped (e.g. 3836x2072 or
        // 1920x800). Bucket by the longer side instead of exact display
        // dimensions so these remain 4K and 1080p respectively.
        let long_side = width.max(height);
        let raw_resolution_tier = if long_side >= 3200 {
            5 // 4K
        } else if long_side >= 2300 {
            4 // 1440p
        } else if long_side >= 1600 {
            3 // 1080p
        } else if long_side >= 1100 {
            2
        } else if long_side > 0 {
            1
        } else {
            0
        };
        // Without an explicit 4K signal, treat 4K as tied with 1080p rather
        // than promoting it. Lower resolution tiers remain distinct so a
        // 720p source cannot beat 1080p merely because of a later tie-breaker.
        let resolution_tier =
            if raw_resolution_tier == 5 && !profile.is_some_and(confident_4k_capable) {
                3
            } else {
                raw_resolution_tier
            };
        // An absent video-range tag is not evidence of quality below SDR.
        // Treat it as the SDR baseline for ordering, without claiming the
        // source was actually probed as SDR.
        let hdr_tier = hdr_tier(video).max(1);

        MediaSourceRank {
            cached: stream_info
                .as_ref()
                .and_then(|info| info.service_cached)
                == Some(true),
            bitrate_plausibility_tier: bitrate_plausibility_tier(self, video),
            transcode_cost_tier: transcode_cost_tier(reasons),
            resolution_tier,
            // HDR vs SDR matters; Dolby Vision vs HDR10 only breaks otherwise
            // equal ranks after release quality, bitrate, bit depth and audio.
            hdr_class: if hdr_tier > 1 { 2 } else { 1 },
            hdr_tier,
            // A missing BitDepth is common for remote (RemuxDB-sourced)
            // probe data that never explicitly set it — fall back to
            // deriving it from PixelFormat (e.g. "yuv420p10le" -> 10), the
            // same way a local ffprobe conversion already does. Only when
            // neither is present do we assume 8-bit, the near-universal
            // baseline — that "unknown" default must never outrank a
            // genuinely-known higher bit depth, but also must not make an
            // unknown stream look worse than an ordinary 8-bit one.
            bit_depth: video
                .and_then(|s| {
                    s.bit_depth
                        .or_else(|| {
                            s.pixel_format
                                .as_deref()
                                .and_then(
                                    crate::playback::probe::bit_depth_from_pix_fmt,
                                )
                        })
                })
                .unwrap_or(8),
            quality_source_tier: crate::db::detect_source_quality_weight(
                stream_info.as_ref(),
            ),
            audio_tier: audio_codec_tier(audio),
            audio_channels: audio
                .and_then(|s| s.channels)
                .unwrap_or(0),
            bitrate: self
                .bitrate
                .unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CodecProfileExt, DeviceProfileExt, MediaSourceCapabilityExt, MediaSourceRank,
        MediaSourceSortKey, SourceRankingContext, failed_condition_reason,
        playback_decision_label, primary_video_stream, subtitle_burn_reason,
        transcode_cost_tier,
    };
    use remux_sdks::remux::{
        AudioCodec, CodecProfile, CodecProfileType, DeviceProfile, DirectPlayProfile,
        DlnaProfileType, EmbeddedSubtitleHandling, MediaSourceInfo, MediaStream,
        MediaStreamType, ProfileCondition, ProfileConditionProperty,
        ProfileConditionType, SortMediaSourcesMode, SubtitleDeliveryMethod,
        SubtitleProfile, TranscodeReason, TranscodeReasons, VideoCodec, VideoContainer,
        VideoRangeType,
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
                type_: Some(CodecProfileType::Video),
                codec: Some(vec!["h264".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some(ProfileConditionType::EqualsAny),
                    property: Some(ProfileConditionProperty::VideoProfile),
                    value: Some("high|main|baseline|constrained baseline".to_string()),
                    is_required: Some(false),
                }],
                ..Default::default()
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

    fn moonfin_ref_frames_profile() -> DeviceProfile {
        let codec_profiles: Vec<CodecProfile> =
            serde_json::from_value(serde_json::json!([
                {
                    "Type": "Video",
                    "Codec": "h264",
                    "Container": "mkv",
                    "Conditions": [{
                        "Condition": "LessThanEqual",
                        "Property": "RefFrames",
                        "Value": "12",
                        "IsRequired": true
                    }],
                    "ApplyConditions": [{
                        "Condition": "GreaterThanEqual",
                        "Property": "Width",
                        "Value": "1200",
                        "IsRequired": true
                    }]
                },
                {
                    "Type": "Video",
                    "Codec": "h264",
                    "Container": "mkv",
                    "Conditions": [{
                        "Condition": "LessThanEqual",
                        "Property": "RefFrames",
                        "Value": "4",
                        "IsRequired": true
                    }],
                    "ApplyConditions": [{
                        "Condition": "GreaterThanEqual",
                        "Property": "Width",
                        "Value": "1900",
                        "IsRequired": true
                    }]
                }
            ]))
            .expect("Moonfin codec profiles deserialize");
        DeviceProfile {
            direct_play_profiles: vec![DirectPlayProfile {
                container: Some(vec![VideoContainer::Mkv]),
                video_codec: Some(vec![VideoCodec::H264]),
                audio_codec: Some(vec![AudioCodec::Aac]),
                type_: Some(DlnaProfileType::Video),
            }],
            codec_profiles,
            ..Default::default()
        }
    }

    fn h264_ref_frames_source(width: i64, ref_frames: Option<i64>) -> MediaSourceInfo {
        let mut source = h264_source("High");
        let video = source
            .media_streams
            .iter_mut()
            .find(|stream| matches!(stream.type_, Some(MediaStreamType::Video)))
            .expect("video stream");
        video.width = Some(width);
        video.ref_frames = ref_frames;
        source
    }

    #[test]
    fn moonfin_apply_conditions_scope_ref_frame_limits() {
        let profile = moonfin_ref_frames_profile();

        let reasons = profile.check_direct_play(&h264_ref_frames_source(1280, Some(5)));
        assert!(
            reasons.is_empty(),
            "the <=4 rule is only applicable at widths >=1900: {reasons:?}"
        );

        let reasons = profile.check_direct_play(&h264_ref_frames_source(1920, Some(5)));
        assert!(
            reasons.contains(&TranscodeReason::RefFramesNotSupported(String::new())),
            "a 1920-wide stream with five refs must fail with the precise reason: {reasons:?}"
        );
        assert!(
            !reasons.contains(&TranscodeReason::VideoCodecNotSupported(String::new()))
        );
    }

    #[test]
    fn required_missing_ref_frames_has_precise_reason() {
        let reasons = moonfin_ref_frames_profile()
            .check_direct_play(&h264_ref_frames_source(1920, None));
        assert!(
            reasons.contains(&TranscodeReason::RefFramesNotSupported(String::new()))
        );
    }

    #[test]
    fn required_zero_ref_frames_has_precise_reason() {
        let reasons = moonfin_ref_frames_profile()
            .check_direct_play(&h264_ref_frames_source(1920, Some(0)));
        assert!(
            reasons.contains(&TranscodeReason::RefFramesNotSupported(String::new()))
        );
    }

    #[test]
    fn codec_profile_container_limits_are_honoured() {
        let mut profile = moonfin_ref_frames_profile();
        for codec_profile in &mut profile.codec_profiles {
            codec_profile.container = Some("mp4".to_string());
        }
        let reasons =
            profile.check_direct_play(&h264_ref_frames_source(1920, Some(99)));
        assert!(
            reasons.is_empty(),
            "MP4-only codec restrictions must not apply to MKV: {reasons:?}"
        );
    }

    #[test]
    fn jellyfin_video_bit_depth_property_deserializes() {
        let condition: ProfileCondition = serde_json::from_value(serde_json::json!({
            "Condition": "LessThanEqual",
            "Property": "VideoBitDepth",
            "Value": "10"
        }))
        .expect("VideoBitDepth condition");
        assert_eq!(
            condition.property,
            Some(ProfileConditionProperty::VideoBitDepth)
        );
    }

    #[test]
    fn total_stream_count_conditions_use_media_source_context() {
        let profile = DeviceProfile {
            direct_play_profiles: moonfin_ref_frames_profile().direct_play_profiles,
            codec_profiles: vec![CodecProfile {
                type_: Some(CodecProfileType::Video),
                codec: Some(vec!["h264".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some(ProfileConditionType::LessThanEqual),
                    property: Some(ProfileConditionProperty::NumStreams),
                    value: Some("2".to_string()),
                    is_required: Some(true),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut source = h264_ref_frames_source(1920, Some(1));
        source
            .media_streams
            .push(MediaStream {
                codec: Some("aac".to_string()),
                type_: Some(MediaStreamType::Audio),
                index: 2,
                ..Default::default()
            });
        let reasons = profile.check_direct_play(&source);
        assert!(
            reasons.contains(&TranscodeReason::StreamCountExceedsLimit(String::new()))
        );
    }

    #[test]
    fn absent_is_required_defaults_to_jellyfin_false() {
        let profile = DeviceProfile {
            direct_play_profiles: moonfin_ref_frames_profile().direct_play_profiles,
            codec_profiles: vec![CodecProfile {
                type_: Some(CodecProfileType::Video),
                codec: Some(vec!["h264".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some(ProfileConditionType::LessThanEqual),
                    property: Some(ProfileConditionProperty::RefFrames),
                    value: Some("4".to_string()),
                    is_required: None,
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            profile
                .check_direct_play(&h264_ref_frames_source(1920, None))
                .is_empty()
        );
    }

    #[test]
    fn hdr10_plus_satisfies_hdr10_apply_condition() {
        let source = MediaSourceInfo {
            media_streams: vec![MediaStream {
                codec: Some("hevc".to_string()),
                type_: Some(MediaStreamType::Video),
                video_range_type: Some(VideoRangeType::Hdr10Plus),
                ..Default::default()
            }],
            ..Default::default()
        };
        let profile = CodecProfile {
            type_: Some(CodecProfileType::Video),
            codec: Some(vec!["hevc".to_string()]),
            apply_conditions: vec![ProfileCondition {
                condition: Some(ProfileConditionType::Equals),
                property: Some(ProfileConditionProperty::VideoRangeType),
                value: Some("HDR10".to_string()),
                is_required: Some(true),
            }],
            ..Default::default()
        };
        let stream = source
            .video_stream()
            .expect("video stream");

        assert!(profile.applies_to_media(&source, stream, "hevc"));
    }

    #[test]
    fn failed_conditions_map_to_precise_transcode_reasons() {
        let video = MediaStream {
            type_: Some(MediaStreamType::Video),
            ..Default::default()
        };
        let audio = MediaStream {
            type_: Some(MediaStreamType::Audio),
            ..Default::default()
        };
        let cases = [
            (
                ProfileConditionProperty::VideoLevel,
                &video,
                "VideoLevelNotSupported",
            ),
            (
                ProfileConditionProperty::Width,
                &video,
                "VideoResolutionNotSupported",
            ),
            (
                ProfileConditionProperty::VideoFramerate,
                &video,
                "VideoFramerateNotSupported",
            ),
            (
                ProfileConditionProperty::VideoRotation,
                &video,
                "VideoRotationNotSupported",
            ),
            (
                ProfileConditionProperty::VideoBitrate,
                &video,
                "VideoBitrateNotSupported",
            ),
            (
                ProfileConditionProperty::RefFrames,
                &video,
                "RefFramesNotSupported",
            ),
            (
                ProfileConditionProperty::IsAnamorphic,
                &video,
                "AnamorphicVideoNotSupported",
            ),
            (
                ProfileConditionProperty::IsInterlaced,
                &video,
                "InterlacedVideoNotSupported",
            ),
            (
                ProfileConditionProperty::AudioChannels,
                &audio,
                "AudioChannelsNotSupported",
            ),
            (
                ProfileConditionProperty::AudioProfile,
                &audio,
                "AudioProfileNotSupported",
            ),
            (
                ProfileConditionProperty::AudioSampleRate,
                &audio,
                "AudioSampleRateNotSupported",
            ),
            (
                ProfileConditionProperty::AudioBitDepth,
                &audio,
                "AudioBitDepthNotSupported",
            ),
            (
                ProfileConditionProperty::AudioBitrate,
                &audio,
                "AudioBitrateNotSupported",
            ),
            (
                ProfileConditionProperty::IsSecondaryAudio,
                &audio,
                "SecondaryAudioNotSupported",
            ),
            (
                ProfileConditionProperty::NumStreams,
                &video,
                "StreamCountExceedsLimit",
            ),
        ];

        for (property, stream, expected) in cases {
            let reason = failed_condition_reason(&property, stream, "test".to_string())
                .expect("known condition must have a reason");
            assert_eq!(reason.name(), expected, "property {property}");
        }
        for property in [
            ProfileConditionProperty::NumAudioStreams,
            ProfileConditionProperty::NumVideoStreams,
            ProfileConditionProperty::IsAvc,
            ProfileConditionProperty::PacketLength,
            ProfileConditionProperty::Has64BitOffsets,
            ProfileConditionProperty::VideoTimestamp,
            ProfileConditionProperty::Other("FutureProperty".to_string()),
        ] {
            assert!(
                failed_condition_reason(&property, &video, "test".to_string())
                    .is_none(),
                "property {property} must match Jellyfin's no-reason behavior"
            );
        }
    }

    #[test]
    fn unsupported_conditions_do_not_scope_or_reject_codec_profiles() {
        let profile = DeviceProfile {
            direct_play_profiles: moonfin_ref_frames_profile().direct_play_profiles,
            codec_profiles: vec![CodecProfile {
                type_: Some(CodecProfileType::Video),
                codec: Some(vec!["h264".to_string()]),
                apply_conditions: vec![ProfileCondition {
                    condition: Some(ProfileConditionType::Equals),
                    property: Some(ProfileConditionProperty::Has64BitOffsets),
                    value: Some("true".to_string()),
                    is_required: Some(true),
                }],
                conditions: vec![ProfileCondition {
                    condition: Some(ProfileConditionType::LessThanEqual),
                    property: Some(ProfileConditionProperty::RefFrames),
                    value: Some("4".to_string()),
                    is_required: Some(true),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let reasons = profile.check_direct_play(&h264_ref_frames_source(1920, Some(5)));
        assert!(
            reasons.contains(&TranscodeReason::RefFramesNotSupported(String::new())),
            "the unsupported apply condition must not hide supported conditions: {reasons:?}"
        );
    }

    #[test]
    fn direct_play_profile_uses_selected_audio_stream() {
        let profile = DeviceProfile {
            direct_play_profiles: vec![DirectPlayProfile {
                container: Some(vec![VideoContainer::Mkv]),
                video_codec: Some(vec![VideoCodec::H264]),
                audio_codec: Some(vec![AudioCodec::Aac]),
                type_: Some(DlnaProfileType::Video),
            }],
            ..Default::default()
        };
        let mut source = h264_source("High");
        source.media_streams[1].codec = Some("ac3".to_string());
        source
            .media_streams
            .push(MediaStream {
                codec: Some("aac".to_string()),
                type_: Some(MediaStreamType::Audio),
                index: 2,
                ..Default::default()
            });
        source.default_audio_stream_index = Some(2);

        assert!(
            profile
                .check_direct_play(&source)
                .is_empty()
        );

        source.default_audio_stream_index = Some(1);
        assert!(
            profile
                .check_direct_play(&source)
                .contains(&TranscodeReason::AudioCodecNotSupported(String::new()))
        );
    }

    #[test]
    fn video_audio_profile_uses_selected_secondary_audio_stream() {
        let profile: CodecProfile = serde_json::from_value(serde_json::json!({
            "Type": "VideoAudio",
            "Codec": "aac",
            "Conditions": [{
                "Condition": "Equals",
                "Property": "IsSecondaryAudio",
                "Value": "false",
                "IsRequired": true
            }]
        }))
        .expect("VideoAudio codec profile");
        assert_eq!(profile.type_, Some(CodecProfileType::VideoAudio));

        let device_profile = DeviceProfile {
            direct_play_profiles: moonfin_ref_frames_profile().direct_play_profiles,
            codec_profiles: vec![profile],
            ..Default::default()
        };
        let mut source = h264_ref_frames_source(1920, Some(1));
        source
            .media_streams
            .push(MediaStream {
                codec: Some("aac".to_string()),
                type_: Some(MediaStreamType::Audio),
                index: 2,
                ..Default::default()
            });
        source.default_audio_stream_index = Some(2);

        let reasons = device_profile.check_direct_play(&source);
        assert!(
            reasons
                .contains(&TranscodeReason::SecondaryAudioNotSupported(String::new())),
            "selected second internal track must be secondary: {reasons:?}"
        );
    }

    #[test]
    fn audio_channel_limit_does_not_force_video_reencode() {
        // A device that only supports 2-channel audio should trigger an audio
        // transcode (AudioChannelsNotSupported), not a video re-encode. Before the
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
                type_: Some(CodecProfileType::VideoAudio),
                codec: Some(vec!["aac".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some(ProfileConditionType::LessThanEqual),
                    property: Some(ProfileConditionProperty::AudioChannels),
                    value: Some("2".to_string()),
                    is_required: Some(false),
                }],
                ..Default::default()
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
            reasons
                .contains(&TranscodeReason::AudioChannelsNotSupported(String::new())),
            "a 5.1 channel limit violation should produce AudioChannelsNotSupported: {reasons:?}"
        );
        assert!(
            !reasons.contains(&TranscodeReason::VideoCodecNotSupported(String::new())),
            "an audio-only constraint must not produce VideoCodecNotSupported: {reasons:?}"
        );
    }

    fn hevc_tag_condition(condition: &str, value: &str) -> DeviceProfile {
        DeviceProfile {
            codec_profiles: vec![CodecProfile {
                type_: Some(CodecProfileType::Video),
                codec: Some(vec!["hevc".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some(
                        condition
                            .parse()
                            .expect("known profile condition"),
                    ),
                    property: Some(ProfileConditionProperty::VideoCodecTag),
                    value: Some(value.to_string()),
                    is_required: Some(true),
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// An HEVC source whose sample entry is `tag` (`None` = the container
    /// reports no fourcc, as MKV does).
    fn hevc_source(tag: Option<&str>) -> MediaSourceInfo {
        MediaSourceInfo {
            container: Some(VideoContainer::Mp4),
            media_streams: vec![MediaStream {
                codec: Some("hevc".to_string()),
                codec_tag: tag.map(str::to_string),
                type_: Some(MediaStreamType::Video),
                index: 0,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn hevc_copy_tag_is_hvc1_for_safaris_declared_condition() {
        // Verbatim from Jellyfin's own Safari test profile
        // (tests/Jellyfin.Model.Tests/Test Data/DeviceProfile-SafariNext.json).
        // Declared constraints outrank the source: even an hev1 source has to
        // be retagged for a client that only accepts hvc1.
        let profile = hevc_tag_condition("EqualsAny", "hvc1|dvh1");
        assert_eq!(profile.hevc_copy_tag(&hevc_source(Some("hev1"))), "hvc1");
    }

    #[test]
    fn hevc_copy_tag_honours_type_omitted_codec_profiles() {
        let mut profile = hevc_tag_condition("EqualsAny", "hvc1|dvh1");
        profile.codec_profiles[0].type_ = None;
        assert_eq!(profile.hevc_copy_tag(&hevc_source(Some("hev1"))), "hvc1");
    }

    #[test]
    fn hevc_copy_tag_honours_not_equals_conditions() {
        // NotEquals hev1 means the client refuses hev1 -> must send hvc1.
        let profile = hevc_tag_condition("NotEquals", "hev1");
        assert_eq!(profile.hevc_copy_tag(&hevc_source(Some("hev1"))), "hvc1");
    }

    #[test]
    fn hevc_copy_tag_is_hev1_when_the_client_rules_hvc1_out() {
        let profile = hevc_tag_condition("Equals", "hev1");
        assert_eq!(profile.hevc_copy_tag(&hevc_source(Some("hvc1"))), "hev1");
    }

    #[test]
    fn hevc_copy_tag_follows_the_source_when_the_client_is_silent() {
        // No VideoCodecTag condition anywhere: an hev1 source keeps hev1,
        // because its parameter sets are in-band and hvc1 would be a lie.
        let silent = DeviceProfile {
            codec_profiles: vec![CodecProfile {
                type_: Some(CodecProfileType::Video),
                codec: Some(vec!["hevc".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some(ProfileConditionType::EqualsAny),
                    property: Some(ProfileConditionProperty::VideoProfile),
                    value: Some("main|main 10".to_string()),
                    is_required: Some(false),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(silent.hevc_copy_tag(&hevc_source(Some("hev1"))), "hev1");
    }

    #[test]
    fn hevc_copy_tag_stays_hvc1_for_ordinary_sources() {
        // The regression guard: a silent client on anything that isn't
        // positively hev1 keeps today's behaviour. An absent fourcc is not
        // evidence — MKV reports none yet keeps parameter sets out-of-band in
        // CodecPrivate.
        for tag in [None, Some("hvc1")] {
            assert_eq!(
                DeviceProfile::default().hevc_copy_tag(&hevc_source(tag)),
                "hvc1",
                "tag={tag:?}"
            );
        }
    }

    #[test]
    fn hevc_copy_tag_ignores_tag_conditions_scoped_to_other_codecs() {
        let profile = DeviceProfile {
            codec_profiles: vec![CodecProfile {
                type_: Some(CodecProfileType::Video),
                codec: Some(vec!["h264".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some(ProfileConditionType::EqualsAny),
                    property: Some(ProfileConditionProperty::VideoCodecTag),
                    value: Some("avc1".to_string()),
                    is_required: Some(true),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        // The h264 constraint must not decide anything, leaving the source to.
        assert_eq!(profile.hevc_copy_tag(&hevc_source(Some("hev1"))), "hev1");
        assert_eq!(profile.hevc_copy_tag(&hevc_source(Some("hvc1"))), "hvc1");
    }

    #[test]
    fn hevc_copy_tag_is_hvc1_when_the_source_has_no_video_stream() {
        let empty = MediaSourceInfo {
            container: Some(VideoContainer::Mp4),
            ..Default::default()
        };
        assert_eq!(DeviceProfile::default().hevc_copy_tag(&empty), "hvc1");
    }

    // --- capability_rank / MediaSourceRank -----------------------------

    fn video_stream(
        width: i64,
        video_range_type: Option<VideoRangeType>,
    ) -> MediaStream {
        MediaStream {
            type_: Some(MediaStreamType::Video),
            index: 0,
            width: Some(width),
            video_range_type,
            ..Default::default()
        }
    }

    fn audio_stream(codec: &str, channels: i64) -> MediaStream {
        MediaStream {
            type_: Some(MediaStreamType::Audio),
            index: 1,
            codec: Some(codec.to_string()),
            channels: Some(channels),
            ..Default::default()
        }
    }

    fn source_with(
        video: MediaStream,
        audio: MediaStream,
        direct_playable: bool,
    ) -> MediaSourceInfo {
        let mut reasons = TranscodeReasons::default();
        if !direct_playable {
            reasons.insert(TranscodeReason::VideoCodecNotSupported("test".to_string()));
        }
        MediaSourceInfo {
            default_audio_stream_index: Some(audio.index),
            media_streams: vec![video, audio],
            transcoding_reasons: reasons,
            ..Default::default()
        }
    }

    /// Real Streamyfin/MPV profile: HEVC up to Level 153 (5.1, the 4K tier),
    /// DOVI excluded. No Width/Height condition anywhere.
    fn streamyfin_mpv_profile() -> DeviceProfile {
        DeviceProfile {
            codec_profiles: vec![CodecProfile {
                type_: Some(CodecProfileType::Video),
                codec: Some(vec!["hevc".to_string(), "h265".to_string()]),
                conditions: vec![
                    ProfileCondition {
                        condition: Some(ProfileConditionType::LessThanEqual),
                        property: Some(ProfileConditionProperty::VideoLevel),
                        value: Some("153".to_string()),
                        is_required: Some(false),
                    },
                    ProfileCondition {
                        condition: Some(ProfileConditionType::NotEquals),
                        property: Some(ProfileConditionProperty::VideoRangeType),
                        value: Some("DOVI".to_string()),
                        is_required: Some(true),
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn compat(rank: MediaSourceRank) -> MediaSourceSortKey {
        rank.key(SortMediaSourcesMode::Compatibility)
    }

    fn quality(rank: MediaSourceRank) -> MediaSourceSortKey {
        rank.key(SortMediaSourcesMode::Quality)
    }

    fn best(rank: MediaSourceRank) -> MediaSourceSortKey {
        rank.key(SortMediaSourcesMode::Best)
    }

    fn source_with_reasons(
        video: MediaStream,
        audio: MediaStream,
        reasons: &[TranscodeReason],
    ) -> MediaSourceInfo {
        let mut transcoding_reasons = TranscodeReasons::default();
        for reason in reasons {
            transcoding_reasons.insert(reason.clone());
        }
        MediaSourceInfo {
            default_audio_stream_index: Some(audio.index),
            media_streams: vec![video, audio],
            transcoding_reasons,
            ..Default::default()
        }
    }

    #[test]
    fn best_mode_lets_a_higher_bitrate_remux_beat_a_lower_bitrate_direct_play() {
        let direct_play_low_bitrate = with_release(
            source_with_reasons(
                video_stream(1920, Some(VideoRangeType::Sdr)),
                audio_stream("aac", 2),
                &[],
            ),
            "Movie.2024.1080p.WEB-DL.mkv",
            5_000_000,
        );
        let direct_stream_high_bitrate = with_release(
            source_with_reasons(
                video_stream(1920, Some(VideoRangeType::Sdr)),
                audio_stream("aac", 2),
                &[TranscodeReason::ContainerNotSupported("mkv".to_string())],
            ),
            "Movie.2024.1080p.BluRay.REMUX.mkv",
            25_000_000,
        );
        assert!(
            compat(
                direct_play_low_bitrate.capability_rank(
                    None,
                    &direct_play_low_bitrate.transcoding_reasons
                )
            ) > compat(direct_stream_high_bitrate.capability_rank(
                None,
                &direct_stream_high_bitrate.transcoding_reasons
            )),
            "Compatibility must still prefer the true direct play"
        );
        assert!(
            best(direct_stream_high_bitrate.capability_rank(
                None,
                &direct_stream_high_bitrate.transcoding_reasons
            )) > best(
                direct_play_low_bitrate.capability_rank(
                    None,
                    &direct_play_low_bitrate.transcoding_reasons
                )
            ),
            "Best must treat direct play and direct stream as equal, letting \
             the higher-bitrate remux win"
        );
    }

    #[test]
    fn best_mode_still_ranks_a_real_transcode_below_direct_play_and_direct_stream() {
        let direct_stream = source_with_reasons(
            video_stream(1920, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            &[TranscodeReason::ContainerNotSupported("mkv".to_string())],
        );
        let needs_video_reencode = source_with_reasons(
            video_stream(3840, Some(VideoRangeType::Dovi)),
            audio_stream("truehd", 8),
            &[TranscodeReason::VideoCodecNotSupported("test".to_string())],
        );
        assert!(
            best(
                direct_stream.capability_rank(None, &direct_stream.transcoding_reasons)
            ) > best(
                needs_video_reencode
                    .capability_rank(None, &needs_video_reencode.transcoding_reasons)
            ),
            "a real re-encode must still rank below a container-only remux, \
             even though it looks better on paper"
        );
    }

    #[test]
    fn compatibility_mode_prefers_direct_playable_over_everything_else() {
        let compatible_1080p = source_with(
            video_stream(1920, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        let incompatible_4k_hdr = source_with(
            video_stream(3840, Some(VideoRangeType::Dovi)),
            audio_stream("truehd", 8),
            false,
        );
        let profile = streamyfin_mpv_profile();
        assert!(
            compat(compatible_1080p.capability_rank(
                Some(&profile),
                &compatible_1080p.transcoding_reasons
            )) > compat(incompatible_4k_hdr.capability_rank(
                Some(&profile),
                &incompatible_4k_hdr.transcoding_reasons
            )),
            "a fully direct-playable 1080p source must outrank a 4K/HDR source that needs a transcode"
        );
    }

    #[test]
    fn quality_mode_prefers_better_quality_even_if_it_needs_a_transcode() {
        let compatible_1080p = source_with(
            video_stream(1920, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        let incompatible_4k_hdr = source_with(
            video_stream(3840, Some(VideoRangeType::Dovi)),
            audio_stream("truehd", 8),
            false,
        );
        let profile = streamyfin_mpv_profile();
        assert!(
            quality(incompatible_4k_hdr.capability_rank(
                Some(&profile),
                &incompatible_4k_hdr.transcoding_reasons
            )) > quality(compatible_1080p.capability_rank(
                Some(&profile),
                &compatible_1080p.transcoding_reasons
            )),
            "Quality mode must ignore transcode cost and rank by HDR/audio quality alone"
        );
    }

    #[test]
    fn transcode_cost_tier_distinguishes_remux_from_audio_from_video_reencode() {
        let direct_play = TranscodeReasons::default();
        let mut remux_only = TranscodeReasons::default();
        remux_only.insert(TranscodeReason::ContainerNotSupported("test".to_string()));
        let mut audio_reencode = TranscodeReasons::default();
        audio_reencode
            .insert(TranscodeReason::AudioCodecNotSupported("test".to_string()));
        let mut video_reencode = TranscodeReasons::default();
        video_reencode
            .insert(TranscodeReason::VideoCodecNotSupported("test".to_string()));

        assert!(transcode_cost_tier(&direct_play) > transcode_cost_tier(&remux_only));
        assert!(
            transcode_cost_tier(&remux_only) > transcode_cost_tier(&audio_reencode)
        );
        assert!(
            transcode_cost_tier(&audio_reencode) > transcode_cost_tier(&video_reencode)
        );
    }

    #[test]
    fn direct_play_profile_selection_prefers_cheaper_work_over_fewer_reasons() {
        let profile = DeviceProfile {
            direct_play_profiles: vec![
                DirectPlayProfile {
                    container: Some(vec![VideoContainer::Mkv]),
                    video_codec: Some(vec![VideoCodec::Hevc]),
                    audio_codec: Some(vec![AudioCodec::Aac]),
                    type_: Some(DlnaProfileType::Video),
                },
                DirectPlayProfile {
                    container: Some(vec![VideoContainer::Mp4]),
                    video_codec: Some(vec![VideoCodec::H264]),
                    audio_codec: Some(vec![AudioCodec::Ac3]),
                    type_: Some(DlnaProfileType::Video),
                },
            ],
            ..Default::default()
        };

        let reasons = profile.check_direct_play(&h264_source("High"));
        assert_eq!(transcode_cost_tier(&reasons), 2);
        assert!(
            reasons.contains(&TranscodeReason::ContainerNotSupported(String::new()))
        );
        assert!(
            reasons.contains(&TranscodeReason::AudioCodecNotSupported(String::new()))
        );
        assert!(
            !reasons.contains(&TranscodeReason::VideoCodecNotSupported(String::new()))
        );
    }

    #[test]
    fn confident_4k_profile_prefers_4k_when_both_direct_playable() {
        let source_1080p = source_with(
            video_stream(1920, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        let source_4k = source_with(
            video_stream(3840, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        let profile = streamyfin_mpv_profile();
        assert!(
            compat(
                source_4k
                    .capability_rank(Some(&profile), &source_4k.transcoding_reasons)
            ) > compat(
                source_1080p
                    .capability_rank(Some(&profile), &source_1080p.transcoding_reasons)
            ),
            "profile has an HEVC Level 153 (4K-tier) condition, so 4K should outrank 1080p"
        );
    }

    #[test]
    fn unknown_4k_capability_does_not_favor_resolution() {
        // No codec profiles at all — no numeric signal to be confident about.
        let profile = DeviceProfile::default();
        let source_1080p = source_with(
            video_stream(1920, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        let source_4k = source_with(
            video_stream(3840, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        assert_eq!(
            source_4k.capability_rank(Some(&profile), &source_4k.transcoding_reasons),
            source_1080p
                .capability_rank(Some(&profile), &source_1080p.transcoding_reasons),
            "without a confident 4K signal, resolution must not affect ranking"
        );
    }

    #[test]
    fn resolution_tiers_keep_1080p_above_720p() {
        let source_1080p = source_with(
            video_stream(1920, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        let source_720p = source_with(
            video_stream(1280, Some(VideoRangeType::Sdr)),
            audio_stream("truehd", 8),
            true,
        );

        assert!(
            best(source_1080p.capability_rank(None, &TranscodeReasons::default()))
                > best(source_720p.capability_rank(None, &TranscodeReasons::default())),
            "resolution must be decided before audio quality"
        );
    }

    #[test]
    fn cropped_dimensions_use_coarse_resolution_buckets() {
        let profile = streamyfin_mpv_profile();
        let tier = |width, height| {
            let mut video = video_stream(width, Some(VideoRangeType::Sdr));
            video.height = Some(height);
            source_with(video, audio_stream("aac", 2), true)
                .capability_rank(Some(&profile), &TranscodeReasons::default())
                .resolution_tier
        };

        assert_eq!(tier(3840, 2160), tier(3836, 2072));
        assert_eq!(tier(3840, 1600), 5);
        assert_eq!(tier(2560, 1440), 4);
        assert_eq!(tier(1920, 800), 3);
        assert_eq!(tier(1280, 536), 2);
        assert_eq!(tier(1024, 554), 1);
    }

    #[test]
    fn hdr_variant_only_breaks_ties_after_bitrate() {
        let hdr10_high_bitrate = with_release(
            source_with(
                video_stream(3840, Some(VideoRangeType::Hdr10)),
                audio_stream("aac", 2),
                true,
            ),
            "Movie.2024.2160p.WEB-DL.mkv",
            90_000_000,
        );
        let dovi_low_bitrate = with_release(
            source_with(
                video_stream(3840, Some(VideoRangeType::Dovi)),
                audio_stream("aac", 2),
                true,
            ),
            "Movie.2024.2160p.WEB-DL.mkv",
            30_000_000,
        );
        assert!(
            best(
                hdr10_high_bitrate.capability_rank(None, &TranscodeReasons::default())
            ) > best(
                dovi_low_bitrate.capability_rank(None, &TranscodeReasons::default())
            )
        );

        let mut dovi_equal_bitrate = dovi_low_bitrate;
        dovi_equal_bitrate.bitrate = Some(90_000_000);
        assert!(
            best(
                dovi_equal_bitrate.capability_rank(None, &TranscodeReasons::default())
            ) > best(
                hdr10_high_bitrate.capability_rank(None, &TranscodeReasons::default())
            )
        );
    }

    #[test]
    fn unknown_video_range_uses_sdr_baseline_for_ranking() {
        let rank_for = |range| {
            best(
                source_with(video_stream(1920, range), audio_stream("aac", 2), true)
                    .capability_rank(None, &TranscodeReasons::default()),
            )
        };
        assert_eq!(rank_for(None), rank_for(Some(VideoRangeType::Sdr)));
        assert!(rank_for(Some(VideoRangeType::Hdr10)) > rank_for(None));
    }

    #[test]
    fn severe_bitrate_mismatch_loses_in_best_but_not_compatibility() {
        let mut tiny_4k_video = video_stream(3840, Some(VideoRangeType::Dovi));
        tiny_4k_video.height = Some(2160);
        let tiny_4k = with_release(
            source_with(tiny_4k_video, audio_stream("aac", 2), true),
            "Movie.2024.2160p.WEB-DL.mkv",
            798_266,
        );
        let mut healthy_1080p_video = video_stream(1920, Some(VideoRangeType::Sdr));
        healthy_1080p_video.height = Some(1080);
        let healthy_1080p = with_release(
            source_with_reasons(
                healthy_1080p_video,
                audio_stream("aac", 2),
                &[TranscodeReason::VideoCodecNotSupported("test".to_string())],
            ),
            "Movie.2024.1080p.WEB-DL.mkv",
            3_000_000,
        );
        let tiny_rank = tiny_4k.capability_rank(None, &tiny_4k.transcoding_reasons);
        let healthy_rank =
            healthy_1080p.capability_rank(None, &healthy_1080p.transcoding_reasons);

        assert_eq!(tiny_rank.bitrate_plausibility_tier, 0);
        assert_eq!(healthy_rank.bitrate_plausibility_tier, 1);
        assert!(best(healthy_rank) > best(tiny_rank));
        assert!(quality(healthy_rank) > quality(tiny_rank));
        assert!(compat(tiny_rank) > compat(healthy_rank));
    }

    #[test]
    fn bitrate_plausibility_uses_pixel_count_and_ignores_unknowns() {
        let mut video_4k = video_stream(3840, Some(VideoRangeType::Hdr10));
        video_4k.height = Some(2160);
        let mut plausible_4k =
            source_with(video_4k.clone(), audio_stream("aac", 2), true);
        plausible_4k.bitrate = Some(30_000_000);
        assert_eq!(
            plausible_4k
                .capability_rank(None, &TranscodeReasons::default())
                .bitrate_plausibility_tier,
            1
        );

        let mut tiny_4k = plausible_4k.clone();
        tiny_4k.bitrate = Some(798_266);
        assert_eq!(
            tiny_4k
                .capability_rank(None, &TranscodeReasons::default())
                .bitrate_plausibility_tier,
            0
        );

        let mut unknown_bitrate = plausible_4k;
        unknown_bitrate.bitrate = None;
        assert_eq!(
            unknown_bitrate
                .capability_rank(None, &TranscodeReasons::default())
                .bitrate_plausibility_tier,
            1
        );
        let mut unknown_height = tiny_4k;
        unknown_height.media_streams[0].height = None;
        assert_eq!(
            unknown_height
                .capability_rank(None, &TranscodeReasons::default())
                .bitrate_plausibility_tier,
            1
        );
        unknown_height.bitrate = Some(34_000);
        assert_eq!(
            unknown_height
                .capability_rank(None, &TranscodeReasons::default())
                .bitrate_plausibility_tier,
            0
        );
    }

    #[test]
    fn ranking_context_ignores_profile_bitrate_limits() {
        let profile = DeviceProfile {
            max_streaming_bitrate: Some(1),
            direct_play_profiles: vec![DirectPlayProfile {
                container: None,
                video_codec: None,
                audio_codec: None,
                type_: Some(DlnaProfileType::Video),
            }],
            ..Default::default()
        };
        let mut source = source_with(
            video_stream(1920, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        source.bitrate = Some(50_000_000);

        let assessment = super::SourceRankingContext {
            mode: SortMediaSourcesMode::Best,
            device_profile: Some(&profile),
            subtitle_mode: EmbeddedSubtitleHandling::default(),
            explicit_subtitle_index: None,
        }
        .assess(&source);

        assert!(
            !assessment
                .reasons
                .contains(&TranscodeReason::ContainerBitrateExceedsLimit)
        );
        assert_eq!(assessment.playback_label(), "Direct Play");
    }

    #[test]
    fn hdr_tier_orders_dovi_above_hdr10plus_above_hdr10_above_hlg_above_sdr() {
        let rank_for = |range: VideoRangeType| {
            compat(
                source_with(
                    video_stream(1920, Some(range)),
                    audio_stream("aac", 2),
                    true,
                )
                .capability_rank(None, &TranscodeReasons::default()),
            )
        };
        assert!(rank_for(VideoRangeType::Dovi) > rank_for(VideoRangeType::Hdr10Plus));
        assert!(rank_for(VideoRangeType::Hdr10Plus) > rank_for(VideoRangeType::Hdr10));
        assert!(rank_for(VideoRangeType::Hdr10) > rank_for(VideoRangeType::Hlg));
        assert!(rank_for(VideoRangeType::Hlg) > rank_for(VideoRangeType::Sdr));
    }

    #[test]
    fn audio_tier_prefers_lossless_over_lossy() {
        let rank_for = |codec: &str| {
            compat(
                source_with(
                    video_stream(1920, Some(VideoRangeType::Sdr)),
                    audio_stream(codec, 2),
                    true,
                )
                .capability_rank(None, &TranscodeReasons::default()),
            )
        };
        assert!(rank_for("truehd") > rank_for("eac3"));
        assert!(rank_for("eac3") > rank_for("ac3"));
        assert!(rank_for("ac3") > rank_for("aac"));
    }

    #[test]
    fn subtitle_burn_in_reason_costs_as_much_as_a_video_reencode() {
        // SubtitleCodecNotSupported is only ever inserted by subtitle_burn_reason
        // (shared by playback.rs and item-details ranking) when
        // EmbeddedSubtitleHandling is Burn — at that point burning the text in
        // means re-encoding the video, so it must rank the same as an
        // incompatible video codec, not as a cheap remux.
        let mut needs_subtitle_burn = TranscodeReasons::default();
        needs_subtitle_burn.insert(TranscodeReason::SubtitleCodecNotSupported(
            "pgssub".to_string(),
        ));
        let mut needs_video_reencode = TranscodeReasons::default();
        needs_video_reencode
            .insert(TranscodeReason::VideoCodecNotSupported("hevc".to_string()));

        assert_eq!(
            transcode_cost_tier(&needs_subtitle_burn),
            transcode_cost_tier(&needs_video_reencode)
        );
        let mut remux_only = TranscodeReasons::default();
        remux_only.insert(TranscodeReason::ContainerNotSupported("mkv".to_string()));
        assert!(
            transcode_cost_tier(&remux_only)
                > transcode_cost_tier(&needs_subtitle_burn)
        );
    }

    fn source_with_subtitle(
        codec: &str,
        is_external: bool,
        index: i64,
    ) -> MediaSourceInfo {
        MediaSourceInfo {
            default_subtitle_stream_index: Some(index),
            media_streams: vec![MediaStream {
                type_: Some(MediaStreamType::Subtitle),
                index,
                codec: Some(codec.to_string()),
                is_external,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn subtitle_burn_reason_only_fires_in_burn_mode() {
        let source = source_with_subtitle("pgssub", false, 0);
        for mode in [
            EmbeddedSubtitleHandling::Strip,
            EmbeddedSubtitleHandling::Extract,
        ] {
            assert!(
                subtitle_burn_reason(&source, None, mode, None).is_none(),
                "{mode:?} must never force a transcode for subtitles"
            );
        }
        assert!(
            subtitle_burn_reason(&source, None, EmbeddedSubtitleHandling::Burn, None)
                .is_some()
        );
    }

    #[test]
    fn subtitle_burn_reason_ignores_text_and_external_subtitles() {
        let text = source_with_subtitle("subrip", false, 0);
        assert!(
            subtitle_burn_reason(&text, None, EmbeddedSubtitleHandling::Burn, None)
                .is_none(),
            "a text subtitle never needs burning in"
        );

        let external_image = source_with_subtitle("pgssub", true, 0);
        assert!(
            subtitle_burn_reason(
                &external_image,
                None,
                EmbeddedSubtitleHandling::Burn,
                None
            )
            .is_none(),
            "an external subtitle is delivered separately, never burned in"
        );
    }

    #[test]
    fn subtitle_burn_reason_respects_a_profile_that_accepts_the_image_format() {
        let source = source_with_subtitle("pgssub", false, 0);
        let profile = DeviceProfile {
            subtitle_profiles: vec![SubtitleProfile {
                format: Some("pgssub".to_string()),
                method: Some(SubtitleDeliveryMethod::Embed),
            }],
            ..Default::default()
        };
        assert!(
            subtitle_burn_reason(
                &source,
                Some(&profile),
                EmbeddedSubtitleHandling::Burn,
                None
            )
            .is_none(),
            "a profile that lists the image format at all needs no burn-in"
        );
    }

    #[test]
    fn subtitle_burn_reason_prefers_the_explicit_index_over_the_resolved_default() {
        let mut source = source_with_subtitle("subrip", false, 0);
        source
            .media_streams
            .push(MediaStream {
                type_: Some(MediaStreamType::Subtitle),
                index: 1,
                codec: Some("pgssub".to_string()),
                ..Default::default()
            });
        // default_subtitle_stream_index (0, text) would need no burn, but the
        // client explicitly asked for index 1 (image) instead.
        assert!(
            subtitle_burn_reason(
                &source,
                None,
                EmbeddedSubtitleHandling::Burn,
                Some(1)
            )
            .is_some()
        );
    }

    #[test]
    fn no_profile_only_transcode_cost_matters() {
        let compatible = source_with(
            video_stream(1920, Some(VideoRangeType::Sdr)),
            audio_stream("aac", 2),
            true,
        );
        let incompatible_but_better_looking = source_with(
            video_stream(3840, Some(VideoRangeType::Dovi)),
            audio_stream("truehd", 8),
            false,
        );
        assert!(
            compat(compatible.capability_rank(None, &compatible.transcoding_reasons))
                > compat(incompatible_but_better_looking.capability_rank(
                    None,
                    &incompatible_but_better_looking.transcoding_reasons
                ))
        );
    }

    fn with_release(
        mut source: MediaSourceInfo,
        filename: &str,
        bitrate: i64,
    ) -> MediaSourceInfo {
        source.bitrate = Some(bitrate);
        source.remux = Some(remux_sdks::remux::MediaSourceRemuxInfo {
            provider_info: serde_json::to_value(crate::stream::StreamInfo {
                filename: Some(filename.to_string()),
                ..Default::default()
            })
            .ok(),
            source: None,
            ..Default::default()
        });
        source
    }

    #[test]
    fn confirmed_cache_hit_precedes_quality_and_compatibility_in_every_sort_mode() {
        let source = || {
            source_with(
                video_stream(1920, Some(VideoRangeType::Sdr)),
                audio_stream("ac3", 6),
                true,
            )
        };
        let mut cached = with_release(source(), "Movie.1080p.WEBRip.mkv", 1_000_000);
        cached
            .remux
            .as_mut()
            .unwrap()
            .provider_info = serde_json::to_value(crate::stream::StreamInfo {
            filename: Some("Movie.1080p.WEBRip.mkv".to_string()),
            service_cached: Some(true),
            ..Default::default()
        })
        .ok();
        cached
            .transcoding_reasons
            .insert(TranscodeReason::VideoCodecNotSupported("test".to_string()));
        let uncached = with_release(source(), "Movie.2160p.BDRemux.mkv", 90_000_000);
        let mut explicitly_uncached = uncached.clone();
        explicitly_uncached
            .remux
            .as_mut()
            .unwrap()
            .provider_info = serde_json::to_value(crate::stream::StreamInfo {
            filename: Some("Movie.2160p.BDRemux.mkv".to_string()),
            service_cached: Some(false),
            ..Default::default()
        })
        .ok();

        let cached_rank = cached.capability_rank(None, &cached.transcoding_reasons);
        for other in [&uncached, &explicitly_uncached] {
            let other_rank = other.capability_rank(None, &other.transcoding_reasons);
            for mode in [
                SortMediaSourcesMode::Best,
                SortMediaSourcesMode::Quality,
                SortMediaSourcesMode::Compatibility,
            ] {
                assert!(
                    cached_rank.key(mode) > other_rank.key(mode),
                    "mode={mode:?}"
                );
            }
        }
    }

    #[test]
    fn quality_source_tier_prefers_remux_over_bluray_over_webdl_at_equal_technical_quality()
     {
        let base = || {
            source_with(
                video_stream(1920, Some(VideoRangeType::Sdr)),
                audio_stream("ac3", 6),
                true,
            )
        };
        let remux = with_release(
            base(),
            "Movie.2024.1080p.BluRay.REMUX.AVC.DD5.1-GROUP.mkv",
            20_000_000,
        );
        let bluray = with_release(
            base(),
            "Movie.2024.1080p.BluRay.DD5.1-GROUP.mkv",
            15_000_000,
        );
        let webdl =
            with_release(base(), "Movie.2024.1080p.WEB-DL.DD5.1-GROUP.mkv", 8_000_000);

        assert!(
            compat(remux.capability_rank(None, &remux.transcoding_reasons))
                > compat(bluray.capability_rank(None, &bluray.transcoding_reasons))
        );
        assert!(
            compat(bluray.capability_rank(None, &bluray.transcoding_reasons))
                > compat(webdl.capability_rank(None, &webdl.transcoding_reasons))
        );
    }

    #[test]
    fn bitrate_breaks_ties_at_equal_release_quality() {
        let base = || {
            source_with(
                video_stream(1920, Some(VideoRangeType::Sdr)),
                audio_stream("ac3", 6),
                true,
            )
        };
        let higher_bitrate = with_release(
            base(),
            "Movie.2024.1080p.BluRay.REMUX.AVC.DD5.1-GROUP.mkv",
            30_000_000,
        );
        let lower_bitrate = with_release(
            base(),
            "Movie.2024.1080p.BluRay.REMUX.AVC.DD5.1-OTHER.mkv",
            10_000_000,
        );
        assert!(
            compat(
                higher_bitrate
                    .capability_rank(None, &higher_bitrate.transcoding_reasons)
            ) > compat(
                lower_bitrate.capability_rank(None, &lower_bitrate.transcoding_reasons)
            )
        );
    }

    // --- Real-data regression: an actual probed MediaSourceInfo (Jurassic
    // World Fallen Kingdom's "BLURAY REMUX ... DTS:X" release, captured from
    // a live server's ffprobe result) against two real client DeviceProfiles.
    // Locks in the exact TranscodeReasons/cost tier observed in production so
    // a future change to condition-matching can't silently regress either
    // client without a test noticing.

    fn jurassic_world_remux_source() -> MediaSourceInfo {
        serde_json::from_str(include_str!(
            "testdata/jurassic_world_remux_probe_data.json"
        ))
        .expect("fixture must deserialize")
    }

    fn streamyfin_mpv_real_profile() -> DeviceProfile {
        serde_json::from_str(include_str!(
            "testdata/streamyfin_mpv_device_profile.json"
        ))
        .expect("fixture must deserialize")
    }

    fn jellyfin_web_real_profile() -> DeviceProfile {
        serde_json::from_str(include_str!("testdata/jellyfin_web_device_profile.json"))
            .expect("fixture must deserialize")
    }

    #[derive(serde::Deserialize)]
    struct BackroomsSourceFixture {
        name: String,
        filename: String,
        #[serde(default)]
        service_cached: Option<bool>,
        probe_data: MediaSourceInfo,
    }

    /// The saved Backrooms probes exercise the real client ranking path. The
    /// expected order is a separate JSON fixture so changes to any position
    /// show up as a reviewable test failure, not just changed console output.
    #[test]
    fn backrooms_web_best_mode_ranking() {
        let fixtures: Vec<BackroomsSourceFixture> =
            serde_json::from_str(include_str!("testdata/backrooms_web_sources.json"))
                .expect("Backrooms fixture must deserialize");
        assert_eq!(fixtures.len(), 28);
        let expected_order: Vec<String> = serde_json::from_str(include_str!(
            "testdata/backrooms_web_best_order.json"
        ))
        .expect("Backrooms expected order must deserialize");
        assert_eq!(expected_order.len(), fixtures.len());

        let profile = jellyfin_web_real_profile();
        let ranking = SourceRankingContext {
            mode: SortMediaSourcesMode::Best,
            device_profile: Some(&profile),
            subtitle_mode: EmbeddedSubtitleHandling::default(),
            explicit_subtitle_index: None,
        };
        let mut ranked: Vec<_> = fixtures
            .into_iter()
            .map(|fixture| {
                let media = crate::db::Media {
                    title: fixture
                        .name
                        .clone(),
                    stream_info: Some(crate::stream::StreamInfo {
                        filename: Some(
                            fixture
                                .filename
                                .clone(),
                        ),
                        service_cached: fixture.service_cached,
                        ..Default::default()
                    }),
                    probe_data: Some(fixture.probe_data),
                    ..Default::default()
                };
                let mut source = crate::api::MediaSourceInfo::from(media.clone());
                crate::conversions::apply_filename_guess(&mut source, &media);
                source.resolve_default_streams(
                    &crate::api::UserConfiguration::default(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                );
                let assessment = ranking.assess(&source);
                (fixture.filename, fixture.name, source, assessment)
            })
            .collect();
        ranked.sort_by_cached_key(|(_, _, _, assessment)| {
            std::cmp::Reverse(assessment.key(SortMediaSourcesMode::Best))
        });

        let actual_order: Vec<&str> = ranked
            .iter()
            .map(|(filename, _, _, _)| filename.as_str())
            .collect();
        let expected_order: Vec<&str> = expected_order
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(actual_order, expected_order);

        assert!(
            ranked[0]
                .1
                .contains("3.2 Mbps"),
            "the confirmed cached source must rank before better uncached sources"
        );
        let position_of = |needle: &str| {
            ranked
                .iter()
                .position(|(_, name, _, _)| name.contains(needle))
                .expect("source must be in the Backrooms fixture")
        };
        assert!(
            position_of("34.2 Mbps") < position_of("4.38 Mbps"),
            "unknown video range must not rank below SDR before bitrate is considered"
        );
        assert!(
            position_of("798 Kbps") > position_of("3.34 Mbps"),
            "the severely under-bitrated 4K source should lose to a credible transcoding source"
        );
        assert!(
            position_of("43.2 Kbps") > position_of("3.34 Mbps"),
            "the severely under-bitrated 1080p source should lose to a credible transcoding source"
        );
        assert!(
            position_of("34 Kbps") > position_of("3.34 Mbps"),
            "missing dimensions must not exempt an extremely low bitrate"
        );

        println!("\n--- Backrooms / Jellyfin Web / Best mode ---");
        for (position, (_, name, source, assessment)) in ranked
            .iter()
            .enumerate()
        {
            let video = source
                .media_streams
                .iter()
                .find(|stream| stream.type_ == Some(MediaStreamType::Video));
            println!(
                "#{:02} key={:?} decision={} video={:?} {}x{} bitrate={:?} reasons={:?} name={name}",
                position + 1,
                assessment.key(SortMediaSourcesMode::Best),
                assessment.playback_label(),
                video.and_then(|v| v
                    .codec
                    .as_deref()),
                video
                    .and_then(|v| v.width)
                    .unwrap_or(0),
                video
                    .and_then(|v| v.height)
                    .unwrap_or(0),
                source.bitrate,
                assessment
                    .reasons
                    .0
                    .iter()
                    .map(TranscodeReason::name)
                    .collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn real_remux_source_direct_plays_on_lenient_streamyfin_profile() {
        let profile = streamyfin_mpv_real_profile();
        let mut source = jurassic_world_remux_source();
        source.transcoding_reasons = profile.check_direct_play(&source);

        assert!(
            source
                .transcoding_reasons
                .is_empty(),
            "mkv/h264/dts/pgssub is fully within Streamyfin's DirectPlayProfiles \
             and SubtitleProfiles — got {:?}",
            source.transcoding_reasons
        );
        assert_eq!(transcode_cost_tier(&source.transcoding_reasons), 4);
    }

    #[test]
    fn real_remux_source_needs_container_audio_and_subtitle_work_on_jellyfin_web() {
        let profile = jellyfin_web_real_profile();
        let mut source = jurassic_world_remux_source();
        source.transcoding_reasons = profile.check_direct_play(&source);

        // mkv isn't in any of Jellyfin Web's video DirectPlayProfiles (mp4/m4v,
        // mov, webm only); dts isn't in any accepted AudioCodec list.
        // Subtitle compatibility for a *specific* requested track is decided
        // separately by `apply_subtitle_delivery` during real playback (using
        // `SubtitleStreamIndex`), not by `check_direct_play` — so it never
        // shows up here in isolation.
        let names: Vec<&str> = source
            .transcoding_reasons
            .0
            .iter()
            .map(TranscodeReason::name)
            .collect();
        for expected in ["ContainerNotSupported", "AudioCodecNotSupported"] {
            assert!(
                names.contains(&expected),
                "expected {expected} in {names:?}"
            );
        }
        assert!(
            !names.contains(&"SubtitleCodecNotSupported"),
            "check_direct_play alone shouldn't evaluate subtitle codecs: {names:?}"
        );
        // The video stream itself (h264, level 41, High profile, SDR) is
        // otherwise compatible, so despite three reasons this is an
        // audio-reencode-tier cost, not a full video re-encode.
        assert_eq!(transcode_cost_tier(&source.transcoding_reasons), 2);
    }

    #[test]
    fn real_remux_source_ranks_better_on_compatible_profile_than_strict_one() {
        // Same file, scored against two real profiles — the ranking itself
        // doesn't compare across profiles (that would be meaningless), but
        // this locks in that transcode_cost_tier correctly differentiates a
        // clean direct-play match from a three-reason one for identical input.
        let mut on_streamyfin = jurassic_world_remux_source();
        on_streamyfin.transcoding_reasons =
            streamyfin_mpv_real_profile().check_direct_play(&on_streamyfin);
        let mut on_jellyfin_web = jurassic_world_remux_source();
        on_jellyfin_web.transcoding_reasons =
            jellyfin_web_real_profile().check_direct_play(&on_jellyfin_web);

        assert!(
            compat(on_streamyfin.capability_rank(
                Some(&streamyfin_mpv_real_profile()),
                &on_streamyfin.transcoding_reasons
            )) > compat(on_jellyfin_web.capability_rank(
                Some(&jellyfin_web_real_profile()),
                &on_jellyfin_web.transcoding_reasons
            ))
        );
    }

    // --- Real-data regression: Project Hail Mary's actual version spread
    // (captured from the live server, including the exact 81GB 4K Remux
    // release that produced the original "far from playable" report — real
    // bitrate 72878866) against both real client profiles. Prints the
    // resulting Compatibility-mode order (run with `-- --nocapture` to see it).

    fn hail_mary_4k_remux() -> MediaSourceInfo {
        serde_json::from_str(include_str!(
            "testdata/hail_mary_4k_remux_probe_data.json"
        ))
        .expect("fixture must deserialize")
    }

    fn hail_mary_1080p_webdl_h264() -> MediaSourceInfo {
        serde_json::from_str(include_str!(
            "testdata/hail_mary_1080p_webdl_h264_probe_data.json"
        ))
        .expect("fixture must deserialize")
    }

    fn hail_mary_1080p_webrip_mp4() -> MediaSourceInfo {
        serde_json::from_str(include_str!(
            "testdata/hail_mary_1080p_webrip_mp4_probe_data.json"
        ))
        .expect("fixture must deserialize")
    }

    fn ranked_for_profile(
        profile: &DeviceProfile,
        label: &str,
    ) -> Vec<(&'static str, MediaSourceInfo)> {
        let mut sources: Vec<(&'static str, MediaSourceInfo)> = vec![
            (
                "4K Remux (hevc/hdr10/truehd 8ch, mkv)",
                hail_mary_4k_remux(),
            ),
            (
                "1080p WEB-DL (h264/sdr/ac3 6ch, mkv)",
                hail_mary_1080p_webdl_h264(),
            ),
            (
                "1080p WEBRip (h264/sdr/aac 6ch, mp4)",
                hail_mary_1080p_webrip_mp4(),
            ),
        ];
        for (_, source) in &mut sources {
            source.transcoding_reasons = profile.check_direct_play(source);
        }
        sources.sort_by_key(|(_, s)| {
            std::cmp::Reverse(compat(
                s.capability_rank(Some(profile), &s.transcoding_reasons),
            ))
        });
        println!("\n--- Compatibility-mode order for {label} ---");
        for (name, s) in &sources {
            println!(
                "  {name}: reasons={:?} tier={}",
                s.transcoding_reasons
                    .0
                    .iter()
                    .map(TranscodeReason::name)
                    .collect::<Vec<_>>(),
                transcode_cost_tier(&s.transcoding_reasons)
            );
        }
        sources
    }

    #[test]
    fn hail_mary_real_versions_rank_compatible_mp4_above_bigger_incompatible_remux_on_jellyfin_web()
     {
        let profile = jellyfin_web_real_profile();
        let sources = ranked_for_profile(&profile, "Jellyfin Web");
        assert_eq!(
            sources[0].0, "1080p WEBRip (h264/sdr/aac 6ch, mp4)",
            "mp4/h264/aac is the only one of the three actually in Jellyfin \
             Web's DirectPlayProfiles — it must rank first even though it's \
             by far the smallest/lowest-bitrate file"
        );
    }

    #[test]
    fn hail_mary_real_versions_on_lenient_streamyfin_profile() {
        let profile = streamyfin_mpv_real_profile();
        ranked_for_profile(&profile, "Streamyfin MPV");
    }
}
