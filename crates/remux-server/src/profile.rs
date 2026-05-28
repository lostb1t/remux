use remux_sdks::remux::{
    CodecProfile, DeviceProfile, DirectPlayProfile, MediaSourceInfo, MediaStream,
    MediaStreamType, ProfileCondition, SubtitleDeliveryMethod, TranscodeReason,
    TranscodeReasons, TranscodingProfile,
};

pub trait DeviceProfileExt {
    fn video_transcoding_profile(&self) -> Option<&TranscodingProfile>;
    fn audio_transcoding_profile(&self) -> Option<&TranscodingProfile>;
    fn subtitle_delivery_method(&self, codec: &str) -> Option<SubtitleDeliveryMethod>;
    fn supports_direct_play(&self, media_source: &MediaSourceInfo) -> bool;
    fn check_direct_play(&self, media_source: &MediaSourceInfo) -> TranscodeReasons;
}

impl DeviceProfileExt for DeviceProfile {
    fn video_transcoding_profile(&self) -> Option<&TranscodingProfile> {
        let is_video = |p: &&TranscodingProfile| {
            p.type_
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("Video"))
                .unwrap_or(false)
        };
        // Prefer HTTP progressive over HLS: clients like Streamyfin hardcode
        // contentType "video/mp4", so an HLS URL causes the Chromecast to reject.
        self.transcoding_profiles
            .iter()
            .find(|p| {
                is_video(p)
                    && p.protocol
                        .as_deref()
                        .map(|pr| pr.eq_ignore_ascii_case("http"))
                        .unwrap_or(false)
            })
            .or_else(|| self.transcoding_profiles.iter().find(|p| is_video(p)))
    }

    fn audio_transcoding_profile(&self) -> Option<&TranscodingProfile> {
        self.transcoding_profiles.iter().find(|p| {
            p.type_
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("Audio"))
                .unwrap_or(false)
        })
    }

    fn subtitle_delivery_method(&self, codec: &str) -> Option<SubtitleDeliveryMethod> {
        self.subtitle_profiles
            .iter()
            .find(|p| {
                p.format
                    .as_deref()
                    .map(|f| f.eq_ignore_ascii_case(codec))
                    .unwrap_or(false)
            })
            .and_then(|p| p.method.clone())
    }

    fn supports_direct_play(&self, media_source: &MediaSourceInfo) -> bool {
        self.check_direct_play(media_source).is_empty()
    }

    fn check_direct_play(&self, media_source: &MediaSourceInfo) -> TranscodeReasons {
        let source_has_video = media_source.video_stream().is_some();
        let mut best: Option<TranscodeReasons> = None;
        for profile in &self.direct_play_profiles {
            if let Some(t) = &profile.type_ {
                if t.eq_ignore_ascii_case("Video") && !source_has_video {
                    continue;
                }
                if t.eq_ignore_ascii_case("Audio") && source_has_video {
                    continue;
                }
            }
            let reasons = profile.check_reasons(media_source);
            if reasons.is_empty() {
                return reasons;
            }
            best = Some(match best {
                None => reasons,
                Some(prev) => {
                    if reasons.0.len() < prev.0.len() {
                        reasons
                    } else {
                        prev
                    }
                }
            });
        }
        let mut reasons = best.unwrap_or_else(|| {
            let mut r = TranscodeReasons::default();
            r.insert(TranscodeReason::ContainerNotSupported(
                "no matching profile".into(),
            ));
            r
        });

        check_codec_profiles(self, media_source, &mut reasons);
        check_subtitle_codec(self, media_source, &mut reasons);

        reasons
    }
}

fn check_codec_profiles(
    profile: &DeviceProfile,
    media_source: &MediaSourceInfo,
    reasons: &mut TranscodeReasons,
) {
    for cp in &profile.codec_profiles {
        let type_ = cp.type_.as_deref().unwrap_or("");
        if type_.eq_ignore_ascii_case("Video") {
            if let Some(stream) = media_source.video_stream() {
                let codec = stream.codec.as_deref().unwrap_or("");
                if cp.applies_to_codec(codec) {
                    for r in cp.check_reasons(stream).0 {
                        reasons.insert(r);
                    }
                }
            }
        } else if type_.eq_ignore_ascii_case("Audio") {
            if let Some(stream) = media_source.audio_stream() {
                let codec = stream.codec.as_deref().unwrap_or("");
                if cp.applies_to_codec(codec) {
                    for r in cp.check_reasons(stream).0 {
                        reasons.insert(r);
                    }
                }
            }
        }
    }
}

fn check_subtitle_codec(
    profile: &DeviceProfile,
    media_source: &MediaSourceInfo,
    reasons: &mut TranscodeReasons,
) {
    let sub_idx = match media_source.default_subtitle_stream_index {
        Some(idx) => idx,
        None => return,
    };
    let sub_stream = media_source.media_streams.iter().find(|s| {
        s.index == sub_idx && matches!(s.type_, Some(MediaStreamType::Subtitle))
    });
    let sub_codec = match sub_stream.and_then(|s| s.codec.as_deref()) {
        Some(c) => c,
        None => return,
    };

    // Drop/External/Embed are passthrough-compatible; Encode and Hls require transcoding.
    let supported = profile.subtitle_profiles.iter().any(|p| {
        let format_matches = p
            .format
            .as_deref()
            .map(|f| f.eq_ignore_ascii_case(sub_codec))
            .unwrap_or(false);
        if !format_matches {
            return false;
        }
        matches!(
            p.method,
            Some(
                SubtitleDeliveryMethod::Drop
                    | SubtitleDeliveryMethod::External
                    | SubtitleDeliveryMethod::Embed
            )
        )
    });

    if !supported {
        reasons.insert(TranscodeReason::SubtitleCodecNotSupported(
            sub_codec.to_string(),
        ));
    }
}

pub trait DirectPlayProfileExt {
    fn supports_media_source(&self, media_source: &MediaSourceInfo) -> bool;
    fn check_reasons(&self, media_source: &MediaSourceInfo) -> TranscodeReasons;
    fn supports_container(&self, container: &str) -> bool;
    fn supports_video_codec(&self, codec: &str) -> bool;
    fn supports_audio_codec(&self, codec: &str) -> bool;
}

impl DirectPlayProfileExt for DirectPlayProfile {
    fn supports_media_source(&self, media_source: &MediaSourceInfo) -> bool {
        self.check_reasons(media_source).is_empty()
    }

    fn check_reasons(&self, media_source: &MediaSourceInfo) -> TranscodeReasons {
        let mut reasons = TranscodeReasons::default();

        match (&self.container, &media_source.container) {
            (Some(profile_container), None) => {
                reasons.insert(TranscodeReason::ContainerNotSupported(format!(
                    "profile={profile_container} source=(none)"
                )));
            }
            (Some(profile_container), Some(source_container)) => {
                if !self.supports_container(source_container) {
                    reasons.insert(TranscodeReason::ContainerNotSupported(format!(
                        "profile={profile_container} source={source_container}"
                    )));
                }
            }
            _ => {}
        }

        if let (Some(profile_codec), Some(video_stream)) =
            (&self.video_codec, media_source.video_stream())
        {
            if let Some(video_codec) = &video_stream.codec {
                if !self.supports_video_codec(video_codec) {
                    reasons.insert(TranscodeReason::VideoCodecNotSupported(format!(
                        "profile={profile_codec} source={video_codec}"
                    )));
                }
            }
        }

        if let (Some(profile_codec), Some(audio_stream)) =
            (&self.audio_codec, media_source.audio_stream())
        {
            if let Some(audio_codec) = &audio_stream.codec {
                if !self.supports_audio_codec(audio_codec) {
                    reasons.insert(TranscodeReason::AudioCodecNotSupported(format!(
                        "profile={profile_codec} source={audio_codec}"
                    )));
                }
            }
        }

        reasons
    }

    fn supports_container(&self, container: &str) -> bool {
        // Normalize aliases: mp4 and m4a are the same format.
        let aliases: &[&str] = match container.to_ascii_lowercase().as_str() {
            "mp4" => &["mp4", "m4a"],
            "m4a" => &["m4a", "mp4"],
            _ => &[],
        };
        self.container
            .as_ref()
            .map(|c| {
                c == "*"
                    || c.split(',').any(|c| {
                        let c = c.trim();
                        c.eq_ignore_ascii_case(container)
                            || aliases.iter().any(|a| c.eq_ignore_ascii_case(a))
                    })
            })
            .unwrap_or(true)
    }

    fn supports_video_codec(&self, codec: &str) -> bool {
        self.video_codec
            .as_ref()
            .map(|v| {
                v == "*" || v.split(',').any(|v| v.trim().eq_ignore_ascii_case(codec))
            })
            .unwrap_or(true)
    }

    fn supports_audio_codec(&self, codec: &str) -> bool {
        self.audio_codec
            .as_ref()
            .map(|a| {
                a == "*" || a.split(',').any(|a| a.trim().eq_ignore_ascii_case(codec))
            })
            .unwrap_or(true)
    }
}

pub trait CodecProfileExt {
    fn applies_to_codec(&self, codec: &str) -> bool;
    fn check_reasons(&self, stream: &MediaStream) -> TranscodeReasons;
}

impl CodecProfileExt for CodecProfile {
    fn applies_to_codec(&self, codec: &str) -> bool {
        self.codec
            .as_deref()
            .map(|c| {
                c == "*" || c.split(',').any(|v| v.trim().eq_ignore_ascii_case(codec))
            })
            .unwrap_or(true)
    }

    fn check_reasons(&self, stream: &MediaStream) -> TranscodeReasons {
        let mut reasons = TranscodeReasons::default();
        for cond in &self.conditions {
            let property = match cond.property.as_deref() {
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
                    cond.condition.as_deref().unwrap_or(""),
                    cond.value.as_deref().unwrap_or(""),
                    actual.as_deref().unwrap_or("(unknown)"),
                );
                let reason = match property {
                    "VideoRangeType" => {
                        TranscodeReason::VideoRangeTypeNotSupported(detail)
                    }
                    "VideoCodecTag" => {
                        TranscodeReason::VideoCodecTagNotSupported(detail)
                    }
                    _ => TranscodeReason::VideoCodecNotSupported(detail),
                };
                reasons.insert(reason);
            }
        }
        reasons
    }
}

fn stream_property_value(stream: &MediaStream, property: &str) -> Option<String> {
    match property {
        "VideoRangeType" => stream
            .video_range_type
            .as_ref()
            .map(|v| v.as_str().to_string()),
        "VideoCodecTag" => stream.codec_tag.clone(),
        "IsAnamorphic" => Some(stream.is_anamorphic.unwrap_or(false).to_string()),
        "IsInterlaced" => Some(stream.is_interlaced.to_string()),
        "IsAVC" | "IsAvc" => Some(stream.is_avc.unwrap_or(false).to_string()),
        "BitDepth" => stream.bit_depth.map(|v| v.to_string()),
        "RefFrames" => stream.ref_frames.map(|v| v.to_string()),
        "NumAudioStreams" | "NumVideoStreams" => None,
        "VideoLevel" | "Level" => stream.level.map(|v| v.to_string()),
        "VideoProfile" | "Profile" => stream.profile.clone(),
        "Height" => stream.height.map(|v| v.to_string()),
        "Width" => stream.width.map(|v| v.to_string()),
        "VideoFramerate" | "Framerate" => stream.real_frame_rate.map(|v| v.to_string()),
        "VideoBitrate" | "Bitrate" | "AudioBitrate" => {
            stream.bit_rate.map(|v| v.to_string())
        }
        "AudioChannels" => stream.channels.map(|v| v.to_string()),
        "AudioSampleRate" => stream.sample_rate.map(|v| v.to_string()),
        _ => None,
    }
}

pub trait ProfileConditionExt {
    fn is_satisfied_opt(&self, actual: Option<&str>) -> bool;
}

impl ProfileConditionExt for ProfileCondition {
    fn is_satisfied_opt(&self, actual: Option<&str>) -> bool {
        let cond = match self.condition.as_deref() {
            Some(c) => c,
            None => return true,
        };
        let actual = match actual {
            Some(v) if !v.is_empty() => v,
            _ => return !self.is_required.unwrap_or(true),
        };
        let expected = self.value.as_deref().unwrap_or("");

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
