//! What a transcode does with each stream, as types instead of codec strings.
//!
//! Two different things used to share the same strings: what the client asked
//! for (copy the stream, or transcode it to some codec) and the ffmpeg encoder
//! the argument builders pick from that. [`CodecTarget`] is the first,
//! [`VideoEncoder`] the second.

use std::fmt;

/// What a client asked for one stream: keep it as is, or transcode it to the
/// named codec (as the client spelled it, e.g. `h264` or `aac`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CodecTarget {
    #[default]
    Copy,
    Transcode(String),
}

impl CodecTarget {
    pub fn is_copy(&self) -> bool {
        matches!(self, Self::Copy)
    }
}

impl From<&str> for CodecTarget {
    fn from(codec: &str) -> Self {
        if codec.eq_ignore_ascii_case("copy") {
            Self::Copy
        } else {
            Self::Transcode(codec.to_string())
        }
    }
}

impl From<String> for CodecTarget {
    fn from(codec: String) -> Self {
        Self::from(codec.as_str())
    }
}

impl fmt::Display for CodecTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Copy => f.write_str("copy"),
            Self::Transcode(codec) => f.write_str(codec),
        }
    }
}

/// The ffmpeg video encoder a transcode runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoEncoder {
    /// Stream copy.
    Copy,
    /// Software H.264.
    Libx264,
    /// A hardware H.264 encoder, by its ffmpeg name (`h264_nvenc`, …).
    Hardware(String),
}

impl VideoEncoder {
    /// Every transcode encodes to H.264: in software, or with the hardware
    /// encoder when acceleration is on.
    pub fn for_target(
        target: &CodecTarget,
        accel: &dyn crate::playback::hw_accel::Accelerator,
    ) -> Self {
        match target {
            CodecTarget::Copy => Self::Copy,
            CodecTarget::Transcode(_) if accel.is_hw() => {
                Self::Hardware(accel.encoder_name("libx264"))
            }
            CodecTarget::Transcode(_) => Self::Libx264,
        }
    }

    pub fn is_copy(&self) -> bool {
        matches!(self, Self::Copy)
    }

    /// The name ffmpeg knows this encoder by (`-c:v`).
    pub fn ffmpeg_name(&self) -> &str {
        match self {
            Self::Copy => "copy",
            Self::Libx264 => "libx264",
            Self::Hardware(name) => name,
        }
    }
}

impl fmt::Display for VideoEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.ffmpeg_name())
    }
}

/// Quality settings for an encode.
pub struct EncodeQuality {
    pub preset: String,
    pub h264_crf: u32,
    pub video_bitrate: Option<u32>,
}

/// `-c:v` onwards for an encode (not for a copy, whose flags depend on the
/// output). `low_latency` tunes x264 for live segmenting (HLS).
pub fn video_encode_args(
    encoder: &VideoEncoder,
    quality: &EncodeQuality,
    low_latency: bool,
) -> Vec<String> {
    let mut args = vec!["-c:v".to_string(), encoder.to_string()];
    match encoder {
        VideoEncoder::Copy => {}
        VideoEncoder::Hardware(name) => {
            // HW encoders use bitrate control; CRF/preset/profile flags don't
            // apply. h264_nvenc needs an explicit 8-bit output: 10-bit frames
            // fail, and CUDA decode leaves frames in software memory so FFmpeg
            // can convert them to yuv420p before the encoder. The option
            // applies with or without a bitrate.
            if name == "h264_nvenc" {
                args.extend(["-pix_fmt".into(), "yuv420p".into()]);
            }
            if let Some(bitrate) = quality.video_bitrate {
                args.extend(["-b:v".into(), bitrate.to_string()]);
            }
        }
        VideoEncoder::Libx264 => {
            args.extend([
                "-profile:v".into(),
                "high".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-crf".into(),
                quality
                    .h264_crf
                    .to_string(),
                "-preset".into(),
                quality
                    .preset
                    .clone(),
            ]);
            if low_latency {
                args.extend(["-tune".into(), "zerolatency".into()]);
            }
            // The client's max bitrate is a ceiling, not a CBR target.
            if let Some(bitrate) = quality.video_bitrate {
                args.extend([
                    "-maxrate".into(),
                    bitrate.to_string(),
                    "-bufsize".into(),
                    (bitrate * 2).to_string(),
                ]);
            }
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_is_recognised_in_any_case() {
        assert!(CodecTarget::from("copy").is_copy());
        assert!(CodecTarget::from("Copy").is_copy());
        assert_eq!(
            CodecTarget::from("h264"),
            CodecTarget::Transcode("h264".into())
        );
        assert_eq!(CodecTarget::from("h264").to_string(), "h264");
    }

    #[test]
    fn only_hls_tunes_x264_for_low_latency() {
        let quality = EncodeQuality {
            preset: "veryfast".into(),
            h264_crf: 23,
            video_bitrate: Some(4_000_000),
        };
        let hls = video_encode_args(&VideoEncoder::Libx264, &quality, true);
        let progressive = video_encode_args(&VideoEncoder::Libx264, &quality, false);
        assert!(
            hls.windows(2)
                .any(|w| w == ["-tune", "zerolatency"])
        );
        assert!(!progressive.contains(&"-tune".to_string()));
        assert!(
            progressive
                .windows(2)
                .any(|w| w == ["-maxrate", "4000000"])
        );
    }

    #[test]
    fn nvenc_gets_an_8_bit_output() {
        let quality = EncodeQuality {
            preset: String::new(),
            h264_crf: 23,
            video_bitrate: None,
        };
        let args = video_encode_args(
            &VideoEncoder::Hardware("h264_nvenc".into()),
            &quality,
            true,
        );
        assert_eq!(args, ["-c:v", "h264_nvenc", "-pix_fmt", "yuv420p"]);
    }
}
