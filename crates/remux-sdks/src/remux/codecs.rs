#[derive(
    Debug, Clone, PartialEq, Eq, strum_macros::EnumString, strum_macros::Display,
)]
#[strum(ascii_case_insensitive)]
pub enum SubtitleCodec {
    // Canonical: "pgssub". Aliases: pgs, hdmv_pgs_subtitle, sup.
    #[strum(
        to_string = "pgssub",
        serialize = "pgssub",
        serialize = "pgs",
        serialize = "hdmv_pgs_subtitle",
        serialize = "sup"
    )]
    Pgs,
    // Canonical: "srt". Aliases: subrip.
    #[strum(to_string = "srt", serialize = "srt", serialize = "subrip")]
    Srt,
    // Canonical: "dvdsub". Aliases: dvd_subtitle.
    #[strum(to_string = "dvdsub", serialize = "dvdsub", serialize = "dvd_subtitle")]
    DvdSub,
    // Canonical: "dvbsub". Aliases: dvb_subtitle.
    #[strum(to_string = "dvbsub", serialize = "dvbsub", serialize = "dvb_subtitle")]
    DvbSub,
    // Canonical: "ass". Aliases: ssa.
    #[strum(to_string = "ass", serialize = "ass", serialize = "ssa")]
    Ass,
    // Canonical: "vtt". Aliases: webvtt.
    #[strum(to_string = "vtt", serialize = "vtt", serialize = "webvtt")]
    WebVtt,
    // Canonical: "tx3g". Aliases: mov_text.
    #[strum(to_string = "tx3g", serialize = "tx3g", serialize = "mov_text")]
    MovText,
    #[strum(default, to_string = "{0}")]
    Other(String),
}

impl SubtitleCodec {
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Pgs | Self::DvdSub | Self::DvbSub)
    }

    pub fn is_text(&self) -> bool {
        // Unknown codecs are assumed to be text (image codecs are a known closed set).
        !self.is_image()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, strum_macros::EnumString, strum_macros::Display,
)]
#[strum(ascii_case_insensitive)]
pub enum VideoCodec {
    #[strum(
        to_string = "h264",
        serialize = "h264",
        serialize = "avc",
        serialize = "avc1",
        // hunch (filename release-tag parser) label.
        serialize = "H.264"
    )]
    H264,
    #[strum(
        to_string = "hevc",
        serialize = "hevc",
        serialize = "h265",
        serialize = "hvc1",
        serialize = "hev1",
        // hunch label.
        serialize = "H.265"
    )]
    Hevc,
    #[strum(
        to_string = "av1",
        serialize = "av1",
        serialize = "libaom-av1",
        serialize = "libsvtav1"
    )]
    Av1,
    #[strum(to_string = "vp9", serialize = "vp9", serialize = "libvpx-vp9")]
    Vp9,
    #[strum(to_string = "vp8", serialize = "vp8", serialize = "libvpx")]
    Vp8,
    #[strum(to_string = "mpeg4", serialize = "mpeg4")]
    Mpeg4,
    #[strum(
        to_string = "mpeg2video",
        serialize = "mpeg2video",
        serialize = "mpeg2",
        // hunch label.
        serialize = "MPEG-2"
    )]
    Mpeg2,
    #[strum(default, to_string = "{0}")]
    Other(String),
}

impl VideoCodec {
    pub fn is_hevc(&self) -> bool {
        matches!(self, Self::Hevc)
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Other(_))
    }

    /// Parse and return `Some` only for a recognized variant (never `Other`).
    pub fn parse_known(s: &str) -> Option<Self> {
        <Self as std::str::FromStr>::from_str(s)
            .ok()
            .filter(|c| c.is_known())
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, strum_macros::EnumString, strum_macros::Display,
)]
#[strum(ascii_case_insensitive)]
pub enum AudioCodec {
    #[strum(
        to_string = "aac",
        serialize = "aac",
        serialize = "aac_fixed",
        serialize = "aac_latm"
    )]
    Aac,
    #[strum(
        to_string = "ac3",
        serialize = "ac3",
        serialize = "a52",
        // hunch label.
        serialize = "Dolby Digital"
    )]
    Ac3,
    #[strum(
        to_string = "eac3",
        serialize = "eac3",
        serialize = "ec3",
        // hunch label.
        serialize = "Dolby Digital Plus"
    )]
    Eac3,
    #[strum(
        to_string = "truehd",
        serialize = "truehd",
        // hunch label.
        serialize = "Dolby TrueHD"
    )]
    TrueHd,
    #[strum(
        to_string = "dts",
        serialize = "dts",
        serialize = "dca",
        // hunch labels — DTS:X and DTS-HD are extensions of base DTS.
        serialize = "DTS:X",
        serialize = "DTS-HD"
    )]
    Dts,
    #[strum(to_string = "flac", serialize = "flac")]
    Flac,
    #[strum(to_string = "mp3", serialize = "mp3", serialize = "mp3float")]
    Mp3,
    #[strum(to_string = "opus", serialize = "opus", serialize = "libopus")]
    Opus,
    #[strum(to_string = "vorbis", serialize = "vorbis")]
    Vorbis,
    #[strum(to_string = "alac", serialize = "alac")]
    Alac,
    #[strum(
        to_string = "pcm",
        serialize = "pcm",
        serialize = "pcm_s16le",
        serialize = "pcm_s24le",
        serialize = "pcm_s32le",
        serialize = "pcm_f32le",
        serialize = "pcm_s16be",
        serialize = "pcm_u8",
        // hunch label.
        serialize = "LPCM"
    )]
    Pcm,
    #[strum(default, to_string = "{0}")]
    Other(String),
}

#[derive(
    Debug, Clone, PartialEq, Eq, strum_macros::EnumString, strum_macros::Display,
)]
#[strum(ascii_case_insensitive)]
pub enum AudioContainer {
    #[strum(to_string = "mp3", serialize = "mp3")]
    Mp3,
    #[strum(to_string = "flac", serialize = "flac")]
    Flac,
    #[strum(to_string = "m4a", serialize = "m4a")]
    M4a,
    #[strum(to_string = "ogg", serialize = "ogg")]
    Ogg,
    #[strum(to_string = "opus", serialize = "opus")]
    Opus,
    #[strum(to_string = "wav", serialize = "wav")]
    Wav,
    #[strum(to_string = "aac", serialize = "aac")]
    Aac,
    #[strum(to_string = "wv", serialize = "wv")]
    Wv,
    #[strum(default, to_string = "{0}")]
    Other(String),
}

impl AudioContainer {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Flac => "audio/flac",
            Self::M4a => "audio/mp4",
            Self::Ogg => "audio/ogg",
            Self::Opus => "audio/opus",
            Self::Wav => "audio/wav",
            Self::Aac => "audio/aac",
            Self::Wv => "audio/x-wavpack",
            Self::Other(_) => "audio/octet-stream",
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Other(_))
    }

    pub fn parse_known(ext: &str) -> Option<Self> {
        <Self as std::str::FromStr>::from_str(ext)
            .ok()
            .filter(|c| c.is_known())
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, strum_macros::EnumString, strum_macros::Display,
)]
#[strum(ascii_case_insensitive)]
pub enum VideoContainer {
    #[strum(to_string = "mkv", serialize = "mkv", serialize = "matroska")]
    Mkv,
    #[strum(to_string = "mp4", serialize = "mp4", serialize = "m4a")]
    Mp4,
    #[strum(to_string = "m4v", serialize = "m4v")]
    M4v,
    #[strum(to_string = "mov", serialize = "mov")]
    Mov,
    #[strum(to_string = "avi", serialize = "avi")]
    Avi,
    #[strum(to_string = "webm", serialize = "webm")]
    Webm,
    #[strum(
        to_string = "ts",
        serialize = "ts",
        serialize = "m2ts",
        serialize = "mpegts"
    )]
    Ts,
    #[strum(to_string = "wmv", serialize = "wmv")]
    Wmv,
    #[strum(
        to_string = "mpeg",
        serialize = "mpeg",
        serialize = "mpg",
        serialize = "mpeg2"
    )]
    Mpeg,
    #[strum(default, to_string = "{0}")]
    Other(String),
}

impl VideoContainer {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp4 | Self::M4v => "video/mp4",
            Self::Mkv => "video/x-matroska",
            Self::Avi => "video/x-msvideo",
            Self::Mov => "video/quicktime",
            Self::Webm => "video/webm",
            Self::Ts => "video/mp2t",
            Self::Wmv => "video/x-ms-wmv",
            Self::Mpeg => "video/mpeg",
            Self::Other(_) => "video/octet-stream",
        }
    }

    pub fn canonical(&self) -> Self {
        match self {
            Self::M4v | Self::Mov => Self::Mp4,
            other => other.clone(),
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Other(_))
    }

    /// Parse a file extension and return `Some` only for known variants.
    pub fn parse_known(ext: &str) -> Option<Self> {
        <Self as std::str::FromStr>::from_str(ext)
            .ok()
            .filter(|c| c.is_known())
    }

    /// True when the container string represents an HLS playlist source (m3u8),
    /// which ffprobe labels as "hls" — a transport, not a real container format.
    pub fn is_hls_input(&self) -> bool {
        matches!(self, Self::Other(s) if s.eq_ignore_ascii_case("hls"))
    }
}

impl serde::Serialize for VideoContainer {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for VideoContainer {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse()
            .map_err(serde::de::Error::custom)
    }
}

impl AudioCodec {
    pub fn friendly_name(&self) -> &str {
        match self {
            Self::Aac => "AAC",
            Self::Ac3 => "Dolby Digital",
            Self::Eac3 => "Dolby Digital Plus",
            Self::TrueHd => "TrueHD",
            Self::Dts => "DTS",
            Self::Flac => "FLAC",
            Self::Mp3 => "MP3",
            Self::Opus => "Opus",
            Self::Vorbis => "Vorbis",
            Self::Alac => "ALAC",
            Self::Pcm => "PCM",
            Self::Other(s) => s.as_str(),
        }
    }

    pub fn needs_adts_reframe(&self) -> bool {
        matches!(self, Self::Aac)
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Other(_))
    }

    /// Parse and return `Some` only for a recognized variant (never `Other`).
    pub fn parse_known(s: &str) -> Option<Self> {
        <Self as std::str::FromStr>::from_str(s)
            .ok()
            .filter(|c| c.is_known())
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, strum_macros::EnumString, strum_macros::Display,
)]
#[strum(ascii_case_insensitive)]
pub enum TranscodingProtocol {
    #[strum(to_string = "http")]
    Http,
    #[strum(to_string = "hls")]
    Hls,
    #[strum(default, to_string = "{0}")]
    Other(String),
}

impl serde::Serialize for TranscodingProtocol {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for TranscodingProtocol {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(s.parse()
            .unwrap_or_else(|_| Self::Other(s)))
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, strum_macros::EnumString, strum_macros::Display,
)]
#[strum(ascii_case_insensitive)]
pub enum DlnaProfileType {
    Video,
    Audio,
    Photo,
    #[strum(default, to_string = "{0}")]
    Other(String),
}

impl serde::Serialize for DlnaProfileType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for DlnaProfileType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(s.parse()
            .unwrap_or_else(|_| Self::Other(s)))
    }
}
