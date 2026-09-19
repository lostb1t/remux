pub(crate) use remux_sdks::remux::{AudioCodec, SubtitleCodec, VideoCodec};
use remux_sdks::remux::{
    CodecProfile, DeviceProfile, DirectPlayProfile, DlnaProfileType, MediaSourceInfo,
    MediaStream, MediaStreamType, ProfileCondition, SortMediaSourcesMode,
    SubtitleDeliveryMethod, TranscodeReason, TranscodeReasons, TranscodingProfile,
    TranscodingProtocol, VideoContainer, VideoRangeType,
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
            self.codec_profiles
                .iter()
                .filter(|cp| matches!(cp.type_, Some(DlnaProfileType::Video)))
                .filter(|cp| cp.applies_to_codec("hevc"))
                .flat_map(|cp| &cp.conditions)
                .filter(|cond| {
                    cond.property
                        .as_deref()
                        == Some("VideoCodecTag")
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

/// Individual quality/compatibility signals for one `MediaSourceInfo`, from
/// which a `SortMediaSourcesMode`-specific sort key is built via `.key()`.
/// Every field is "higher is better".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaSourceRank {
    transcode_cost_tier: u8,
    resolution_tier: u8,
    hdr_tier: u8,
    bit_depth: i64,
    quality_source_tier: u8,
    audio_tier: u8,
    audio_channels: i64,
    bitrate: i64,
}

impl MediaSourceRank {
    /// Sort key for `mode` — a *greater* value is a *better* match. Compared
    /// lexicographically, most significant tier first.
    ///
    /// `Compatibility` puts `transcode_cost_tier` first: a version that direct
    /// plays (or only needs a cheap remux) always outranks one that needs a
    /// real re-encode, no matter how much better it looks on paper.
    /// `Quality` drops cost from the key entirely — best quality wins even if
    /// it means transcoding. `Disabled` has no key; callers must not sort.
    ///
    /// There's no separate subtitle field: whether the resolved default
    /// subtitle needs burning in is already reflected in
    /// `transcode_cost_tier` via `TranscodeReason::SubtitleCodecNotSupported`
    /// (inserted in `api/playback.rs`, scoped to the one subtitle stream that
    /// will actually be used and to `EmbeddedSubtitleHandling::Burn` mode) —
    /// re-deriving it here from every embedded subtitle stream would ignore
    /// which one is actually selected and double-count the same fact.
    pub fn key(
        &self,
        mode: SortMediaSourcesMode,
    ) -> (u8, u8, u8, i64, u8, u8, i64, i64) {
        let cost = match mode {
            SortMediaSourcesMode::Compatibility => self.transcode_cost_tier,
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
        (
            cost,
            self.resolution_tier,
            self.hdr_tier,
            self.bit_depth,
            self.quality_source_tier,
            self.audio_tier,
            self.audio_channels,
            self.bitrate,
        )
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
    // incompatible video codec/profile/range/bit depth.
    let needs_video_reencode = reasons
        .0
        .iter()
        .any(|r| {
            matches!(
                r,
                TranscodeReason::VideoCodecNotSupported(_)
                    | TranscodeReason::VideoRangeTypeNotSupported(_)
                    | TranscodeReason::VideoProfileNotSupported(_)
                    | TranscodeReason::VideoBitDepthNotSupported(_)
                    | TranscodeReason::SubtitleCodecNotSupported(_)
            )
        });
    if needs_video_reencode {
        return 1;
    }
    let needs_audio_reencode = reasons
        .0
        .iter()
        .any(|r| matches!(r, TranscodeReason::AudioCodecNotSupported(_)));
    if needs_audio_reencode {
        return 2;
    }
    // Only cheap reasons left: ContainerNotSupported, VideoCodecTagNotSupported,
    // ContainerBitrateExceedsLimit — a remux, not a re-encode (video and audio
    // both copied).
    3
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
        if !matches!(cp.type_, None | Some(DlnaProfileType::Video)) {
            continue;
        }
        let is_hevc = codec_list_contains(&cp.codec, &["hevc", "h265"]);
        let is_av1 = codec_list_contains(&cp.codec, &["av1"]);
        for cond in &cp.conditions {
            let Some(property) = cond
                .property
                .as_deref()
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
                "Width" => value >= 3840,
                "Height" => value >= 2160,
                "VideoLevel" | "Level" => {
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

fn default_audio_stream(source: &MediaSourceInfo) -> Option<&MediaStream> {
    if let Some(idx) = source.default_audio_stream_index {
        if let Some(s) = source
            .media_streams
            .iter()
            .find(|s| s.index == idx)
        {
            return Some(s);
        }
    }
    source
        .media_streams
        .iter()
        .find(|s| matches!(s.type_, Some(MediaStreamType::Audio)))
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
/// `source.capability_rank(profile)` and sort with
/// `sort_by_key(|s| Reverse(s.capability_rank(profile).key(mode)))`, higher is
/// better.
pub trait MediaSourceCapabilityExt {
    fn capability_rank(&self, profile: Option<&DeviceProfile>) -> MediaSourceRank;
}

/// Release-source weight (remux > BluRay > WEB-DL > WEBRip > ...), parsed from
/// the original release filename the same way pre-probe candidate ordering
/// does. `MediaSourceInfo` has no filename field of its own, but
/// `conversions.rs` always stamps the original `StreamInfo` (as JSON) into
/// `remux.provider_info`, so that's recovered here instead of threading a
/// second parameter through every call site.
fn quality_source_tier(source: &MediaSourceInfo) -> u8 {
    let stream_info: Option<crate::stream::StreamInfo> = source
        .remux
        .as_ref()
        .and_then(|r| {
            r.provider_info
                .as_ref()
        })
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    crate::db::detect_source_quality_weight(stream_info.as_ref())
}

impl MediaSourceCapabilityExt for MediaSourceInfo {
    /// When `profile` is `None`, every source ties except on
    /// `transcode_cost_tier`, which reflects whatever transcode reasons are
    /// already on the source.
    fn capability_rank(&self, profile: Option<&DeviceProfile>) -> MediaSourceRank {
        let video = primary_video_stream(self);
        let audio = default_audio_stream(self);

        let resolution_tier = match profile {
            Some(p) if confident_4k_capable(p) => {
                let is_4k = video
                    .and_then(|s| s.width)
                    .is_some_and(|w| w >= 3840);
                u8::from(is_4k)
            }
            _ => 0,
        };

        MediaSourceRank {
            transcode_cost_tier: transcode_cost_tier(&self.transcoding_reasons),
            resolution_tier,
            hdr_tier: hdr_tier(video),
            bit_depth: video
                .and_then(|s| s.bit_depth)
                .unwrap_or(0),
            quality_source_tier: quality_source_tier(self),
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
        DeviceProfileExt, MediaSourceCapabilityExt, MediaSourceRank,
        transcode_cost_tier,
    };
    use remux_sdks::remux::{
        AudioCodec, CodecProfile, DeviceProfile, DirectPlayProfile, DlnaProfileType,
        MediaSourceInfo, MediaStream, MediaStreamType, ProfileCondition,
        SortMediaSourcesMode, SubtitleDeliveryMethod, SubtitleProfile, TranscodeReason,
        TranscodeReasons, VideoCodec, VideoContainer, VideoRangeType,
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

    fn hevc_tag_condition(condition: &str, value: &str) -> DeviceProfile {
        DeviceProfile {
            codec_profiles: vec![CodecProfile {
                type_: Some(DlnaProfileType::Video),
                codec: Some(vec!["hevc".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some(condition.to_string()),
                    property: Some("VideoCodecTag".to_string()),
                    value: Some(value.to_string()),
                    is_required: Some(true),
                }],
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
                type_: Some(DlnaProfileType::Video),
                codec: Some(vec!["hevc".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some("EqualsAny".to_string()),
                    property: Some("VideoProfile".to_string()),
                    value: Some("main|main 10".to_string()),
                    is_required: Some(false),
                }],
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
                type_: Some(DlnaProfileType::Video),
                codec: Some(vec!["h264".to_string()]),
                conditions: vec![ProfileCondition {
                    condition: Some("EqualsAny".to_string()),
                    property: Some("VideoCodecTag".to_string()),
                    value: Some("avc1".to_string()),
                    is_required: Some(true),
                }],
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
                type_: Some(DlnaProfileType::Video),
                codec: Some(vec!["hevc".to_string(), "h265".to_string()]),
                conditions: vec![
                    ProfileCondition {
                        condition: Some("LessThanEqual".to_string()),
                        property: Some("VideoLevel".to_string()),
                        value: Some("153".to_string()),
                        is_required: Some(false),
                    },
                    ProfileCondition {
                        condition: Some("NotEquals".to_string()),
                        property: Some("VideoRangeType".to_string()),
                        value: Some("DOVI".to_string()),
                        is_required: Some(true),
                    },
                ],
            }],
            ..Default::default()
        }
    }

    fn compat(rank: MediaSourceRank) -> (u8, u8, u8, i64, u8, u8, i64, i64) {
        rank.key(SortMediaSourcesMode::Compatibility)
    }

    fn quality(rank: MediaSourceRank) -> (u8, u8, u8, i64, u8, u8, i64, i64) {
        rank.key(SortMediaSourcesMode::Quality)
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
            compat(compatible_1080p.capability_rank(Some(&profile)))
                > compat(incompatible_4k_hdr.capability_rank(Some(&profile))),
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
            quality(incompatible_4k_hdr.capability_rank(Some(&profile)))
                > quality(compatible_1080p.capability_rank(Some(&profile))),
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
            compat(source_4k.capability_rank(Some(&profile)))
                > compat(source_1080p.capability_rank(Some(&profile))),
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
            source_4k.capability_rank(Some(&profile)),
            source_1080p.capability_rank(Some(&profile)),
            "without a confident 4K signal, resolution must not affect ranking"
        );
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
                .capability_rank(None),
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
                .capability_rank(None),
            )
        };
        assert!(rank_for("truehd") > rank_for("eac3"));
        assert!(rank_for("eac3") > rank_for("ac3"));
        assert!(rank_for("ac3") > rank_for("aac"));
    }

    #[test]
    fn subtitle_burn_in_reason_costs_as_much_as_a_video_reencode() {
        // SubtitleCodecNotSupported is only ever inserted (by api/playback.rs)
        // for the resolved default subtitle when EmbeddedSubtitleHandling is
        // Burn — at that point burning the text in means re-encoding the
        // video, so it must rank the same as an incompatible video codec, not
        // as a cheap remux.
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
            compat(compatible.capability_rank(None))
                > compat(incompatible_but_better_looking.capability_rank(None))
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
        });
        source
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
            compat(remux.capability_rank(None)) > compat(bluray.capability_rank(None))
        );
        assert!(
            compat(bluray.capability_rank(None)) > compat(webdl.capability_rank(None))
        );
    }

    #[test]
    fn bitrate_is_the_final_tiebreaker_at_equal_everything_else() {
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
            compat(higher_bitrate.capability_rank(None))
                > compat(lower_bitrate.capability_rank(None))
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
            compat(on_streamyfin.capability_rank(Some(&streamyfin_mpv_real_profile())))
                > compat(
                    on_jellyfin_web.capability_rank(Some(&jellyfin_web_real_profile()))
                )
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
            std::cmp::Reverse(compat(s.capability_rank(Some(profile))))
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
