use remux_sdks::remux::{EncodingOptions, HardwareAccelerationType};

pub trait Accelerator: Send {
    fn has_av1_decode(&self) -> bool {
        false
    }

    /// True for VAAPI and QSV — they support hardware VPP tone mapping.
    fn supports_vpp_tonemap(&self) -> bool {
        false
    }

    /// True for VideoToolbox — it has no hardware tone mapper, so HDR always
    /// falls back to the CPU tonemapx filter.
    fn prefers_sw_tonemap(&self) -> bool {
        false
    }

    /// False only for `NoAccel` — used to decide whether hardware codec names
    /// and filter chains should be applied.
    fn is_hw(&self) -> bool {
        true
    }

    /// Short encoder suffix, e.g. `"_nvenc"`.  `None` means software encode.
    fn encoder_suffix(&self) -> Option<&str> {
        None
    }

    /// Map a software encoder name to the hardware equivalent using
    /// `encoder_suffix`.  Returns `base` unchanged for "copy", VP9, or when
    /// there is no suffix.
    fn encoder_name(&self, base: &str) -> String {
        let Some(sfx) = self.encoder_suffix() else {
            return base.to_string();
        };
        match base {
            "libx264" => format!("h264{sfx}"),
            "libx265" => format!("hevc{sfx}"),
            other => other.to_string(),
        }
    }

    /// Hardware-decode flags placed **before** `-i`.  The default (no args)
    /// means software decode.
    fn input_args(&self) -> Vec<String> {
        vec![]
    }

    /// Filter steps appended after the scale filter to move frames onto the
    /// hardware device for the encoder (e.g. `hwupload` for VAAPI).
    fn filter_suffix(&self) -> Option<String> {
        None
    }

    /// Environment variable overrides for the ffmpeg child process.
    fn env_overrides(&self) -> Vec<(&'static str, String)> {
        vec![]
    }

    /// True when the codec/HDR combination requires software decode even though
    /// hardware encoding is still used.
    fn requires_software_decode(
        &self,
        _source_codec: Option<&str>,
        _hdr: bool,
    ) -> bool {
        false
    }

    /// Hardware-decode args that account for codec/HDR context.  Callers pass
    /// this instead of `input_args()` directly.
    ///
    /// The default skips `input_args()` when `requires_software_decode` is set.
    /// QSV overrides to emit device-init-only args when HDR needs SW decode but
    /// the QSV encoder still needs the device chain in place.
    fn decode_input_args(
        &self,
        source_codec: Option<&str>,
        hdr: bool,
        _do_vpp_tonemap: bool,
    ) -> Vec<String> {
        if self.requires_software_decode(source_codec, hdr) {
            vec![]
        } else {
            self.input_args()
        }
    }

    /// Hardware filter suffix accounting for HDR and VPP tonemapping mode.
    /// Callers use this instead of `filter_suffix()` directly.
    ///
    /// The default is just `filter_suffix()`.  QSV overrides to swap in the
    /// VPP tonemap chain or a bare `format=nv12` depending on the decode path.
    fn hw_filter_suffix(&self, _hdr: bool, _do_vpp_tonemap: bool) -> Option<String> {
        self.filter_suffix()
    }

    fn as_type(&self) -> HardwareAccelerationType;
}

pub struct NoAccel;
pub struct Nvenc;
pub struct Vaapi {
    pub device: String,
    pub driver: String,
}
pub struct Qsv {
    pub vaapi_device: String,
    pub vaapi_driver: String,
}
pub struct VideoToolbox {
    pub av1_hw_decode: bool,
}
pub struct Amf;
pub struct V4l2m2m;
pub struct Rkmpp;

impl Accelerator for NoAccel {
    fn is_hw(&self) -> bool {
        false
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::None
    }
}

impl Accelerator for Nvenc {
    fn encoder_suffix(&self) -> Option<&str> {
        Some("_nvenc")
    }

    fn input_args(&self) -> Vec<String> {
        vec!["-hwaccel".into(), "cuda".into()]
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::Nvenc
    }
}

impl Accelerator for Vaapi {
    fn encoder_suffix(&self) -> Option<&str> {
        Some("_vaapi")
    }

    fn input_args(&self) -> Vec<String> {
        let driver_opt = if self
            .driver
            .is_empty()
        {
            String::new()
        } else {
            format!(",driver={}", self.driver)
        };
        vec![
            "-init_hw_device".into(),
            format!("vaapi=va:{}{}", self.device, driver_opt),
            "-filter_hw_device".into(),
            "va".into(),
        ]
    }

    fn filter_suffix(&self) -> Option<String> {
        Some("format=nv12,hwupload".to_string())
    }

    fn env_overrides(&self) -> Vec<(&'static str, String)> {
        if self.driver == "i965" {
            vec![("LIBVA_DRIVER_NAME", "i965".to_string())]
        } else {
            vec![]
        }
    }

    fn supports_vpp_tonemap(&self) -> bool {
        true
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::Vaapi
    }
}

impl Qsv {
    // Device-init args without the hwaccel decode flags — used when HDR source
    // needs software decode but the QSV encoder still requires the device chain.
    fn init_only_args(&self) -> Vec<String> {
        let driver = if self
            .vaapi_driver
            .is_empty()
        {
            "iHD".to_string()
        } else {
            self.vaapi_driver
                .clone()
        };
        let driver_opt = if driver.is_empty() {
            String::new()
        } else {
            format!(",driver={driver}")
        };
        vec![
            "-init_hw_device".into(),
            format!("vaapi=va:{}{}", self.vaapi_device, driver_opt),
            "-init_hw_device".into(),
            "qsv=qs@va".into(),
            "-filter_hw_device".into(),
            "qs".into(),
        ]
    }
}

impl Accelerator for Qsv {
    fn encoder_suffix(&self) -> Option<&str> {
        Some("_qsv")
    }

    fn input_args(&self) -> Vec<String> {
        let mut args = self.init_only_args();
        args.extend([
            "-hwaccel".into(),
            "vaapi".into(),
            "-hwaccel_output_format".into(),
            "vaapi".into(),
        ]);
        args
    }

    fn filter_suffix(&self) -> Option<String> {
        Some("hwmap=derive_device=qsv,format=qsv".to_string())
    }

    fn env_overrides(&self) -> Vec<(&'static str, String)> {
        if self.vaapi_driver == "i965" {
            vec![("LIBVA_DRIVER_NAME", "i965".to_string())]
        } else {
            vec![]
        }
    }

    fn supports_vpp_tonemap(&self) -> bool {
        true
    }

    fn decode_input_args(
        &self,
        _source_codec: Option<&str>,
        hdr: bool,
        do_vpp_tonemap: bool,
    ) -> Vec<String> {
        if hdr && !do_vpp_tonemap {
            // SW decode so CPU filters can run; device chain still needed for
            // the QSV encoder.
            self.init_only_args()
        } else {
            self.input_args()
        }
    }

    fn hw_filter_suffix(&self, hdr: bool, do_vpp_tonemap: bool) -> Option<String> {
        if do_vpp_tonemap {
            Some(
                "tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709:extra_hw_frames=32,\
                 hwmap=derive_device=qsv,format=qsv"
                    .to_string(),
            )
        } else if hdr {
            // SW-decode path — frames are in CPU memory; format=nv12 before encoder.
            Some("format=nv12".to_string())
        } else {
            self.filter_suffix()
        }
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::Qsv
    }
}

impl Accelerator for VideoToolbox {
    fn encoder_suffix(&self) -> Option<&str> {
        Some("_videotoolbox")
    }

    fn input_args(&self) -> Vec<String> {
        vec!["-hwaccel".into(), "videotoolbox".into()]
    }

    fn has_av1_decode(&self) -> bool {
        self.av1_hw_decode
    }

    fn prefers_sw_tonemap(&self) -> bool {
        true
    }

    fn requires_software_decode(&self, source_codec: Option<&str>, hdr: bool) -> bool {
        hdr || (source_codec.is_some_and(|c| c.eq_ignore_ascii_case("av1"))
            && !self.av1_hw_decode)
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::VideoToolbox
    }
}

impl Accelerator for Amf {
    fn encoder_suffix(&self) -> Option<&str> {
        Some("_amf")
    }

    fn input_args(&self) -> Vec<String> {
        vec!["-hwaccel".into(), "d3d11va".into()]
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::Amf
    }
}

impl Accelerator for V4l2m2m {
    fn encoder_suffix(&self) -> Option<&str> {
        Some("_v4l2m2m")
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::V4l2m2m
    }
}

impl Accelerator for Rkmpp {
    fn encoder_suffix(&self) -> Option<&str> {
        Some("_rkmpp")
    }

    fn input_args(&self) -> Vec<String> {
        vec!["-hwaccel".into(), "rkmpp".into()]
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::Rkmpp
    }
}

/// Query whether this Mac's VideoToolbox hardware can decode AV1.
/// Result is cached after the first call — hardware doesn't change at runtime.
#[cfg(target_os = "macos")]
fn videotoolbox_av1_hw_decode_supported() -> bool {
    use std::sync::OnceLock;
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        const AV1_VIDEO_CODEC_TYPE: u32 = u32::from_be_bytes(*b"av01");
        #[link(name = "VideoToolbox", kind = "framework")]
        unsafe extern "C" {
            fn VTIsHardwareDecodeSupported(codec_type: u32) -> u8;
        }
        let supported =
            unsafe { VTIsHardwareDecodeSupported(AV1_VIDEO_CODEC_TYPE) != 0 };
        tracing::debug!(supported, "VideoToolbox AV1 hardware decode capability");
        supported
    })
}

#[cfg(not(target_os = "macos"))]
fn videotoolbox_av1_hw_decode_supported() -> bool {
    false
}

/// Build a runtime `Accelerator` from the persisted encoding configuration.
pub fn from_encoding_opts(opts: &EncodingOptions) -> Box<dyn Accelerator> {
    let accel_type = opts
        .hardware_acceleration_type
        .unwrap_or_default();
    let device = opts
        .vaapi_device
        .clone()
        .unwrap_or_else(|| "/dev/dri/renderD128".to_string());
    let driver = opts
        .vaapi_driver
        .clone()
        .unwrap_or_default();

    match accel_type {
        HardwareAccelerationType::None => Box::new(NoAccel),
        HardwareAccelerationType::Nvenc => Box::new(Nvenc),
        HardwareAccelerationType::Vaapi => Box::new(Vaapi { device, driver }),
        HardwareAccelerationType::Qsv => Box::new(Qsv {
            vaapi_device: device,
            vaapi_driver: driver,
        }),
        HardwareAccelerationType::VideoToolbox => Box::new(VideoToolbox {
            av1_hw_decode: videotoolbox_av1_hw_decode_supported(),
        }),
        HardwareAccelerationType::Amf => Box::new(Amf),
        HardwareAccelerationType::V4l2m2m => Box::new(V4l2m2m),
        HardwareAccelerationType::Rkmpp => Box::new(Rkmpp),
    }
}
