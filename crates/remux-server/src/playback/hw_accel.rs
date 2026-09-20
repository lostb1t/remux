use remux_sdks::remux::{EncodingOptions, HardwareAccelerationType};

/// How the colour range of the source is handled on the way to an SDR output.
///
/// Derived once per transcode by [`HdrTreatment::for_source`] and threaded
/// through the accelerator and filter-graph builders, so decode flags, filter
/// chains and overlay placement all agree on where frames live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrTreatment {
    /// Source is SDR; no colour handling needed.
    Sdr,
    /// HDR source, no tone mapping: colour metadata is relabelled BT.709 and
    /// the frame is truncated to 8-bit. Cheap but washed out.
    Clamp,
    /// Hardware tone mapping (`tonemap_vaapi`) on GPU-resident frames.
    VppTonemap,
    /// Software tone mapping (`tonemapx`) on CPU frames. Forces software
    /// decode on accelerators whose filter chain would otherwise stay on GPU.
    SwTonemap,
}

impl HdrTreatment {
    /// `needs_cpu_overlay` is true when a subtitle burn-in is requested and
    /// the accelerator has no GPU-resident overlay (only QSV's `overlay_qsv`
    /// qualifies — see [`Accelerator::supports_gpu_resident_overlay`]). In
    /// that case VPP tonemap is unusable: `tonemap_vaapi` requires
    /// VAAPI-resident frames, but the CPU `overlay` filter that composites
    /// the subtitle outputs plain `yuv420p` CPU frames (issue #378). Falling
    /// through to `VppTonemap` anyway would silently skip tone mapping
    /// (the CPU overlay path only reads `SwTonemap` to decide whether to run
    /// `tonemapx`) and wash out the picture instead of erroring — so this
    /// case must be routed to `SwTonemap` explicitly.
    pub fn for_source(
        hdr: bool,
        accel: &dyn Accelerator,
        enable_tonemapping: bool,
        enable_vpp_tonemapping: bool,
        needs_cpu_overlay: bool,
    ) -> Self {
        let vpp_blocked_by_overlay =
            needs_cpu_overlay && !accel.supports_gpu_resident_overlay();
        if !hdr {
            Self::Sdr
        } else if enable_vpp_tonemapping
            && accel.supports_vpp_tonemap()
            && !vpp_blocked_by_overlay
        {
            Self::VppTonemap
        } else if enable_tonemapping
            || accel.prefers_sw_tonemap()
            // A CPU overlay blocking VPP still means the user asked for *some*
            // tone mapping — fall back to software rather than silently
            // clamping, regardless of whether this accelerator could ever
            // have done VPP tonemap in the first place (e.g. NVENC).
            || (enable_vpp_tonemapping && vpp_blocked_by_overlay)
        {
            Self::SwTonemap
        } else {
            Self::Clamp
        }
    }

    pub fn is_hdr(self) -> bool {
        self != Self::Sdr
    }
}

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

    /// True only for QSV: its `overlay_qsv` filter composites a subtitle onto
    /// GPU-resident frames, so a burn-in doesn't force the video through the
    /// CPU `overlay` filter. VAAPI (the other VPP-tonemap-capable
    /// accelerator) has no such filter — its burn-in path always runs on the
    /// CPU — so it must not be treated as GPU-resident here.
    fn supports_gpu_resident_overlay(&self) -> bool {
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
    /// QSV overrides to emit device-init-only args when software tone mapping
    /// needs CPU frames but the QSV encoder still needs the device chain.
    fn decode_input_args(
        &self,
        source_codec: Option<&str>,
        treatment: HdrTreatment,
    ) -> Vec<String> {
        if self.requires_software_decode(source_codec, treatment.is_hdr()) {
            vec![]
        } else {
            self.input_args()
        }
    }

    /// Hardware filter suffix accounting for the HDR treatment.  Callers use
    /// this instead of `filter_suffix()` directly.
    ///
    /// The default is just `filter_suffix()`.  QSV overrides to swap in the
    /// VPP tonemap chain, the BT.709 relabel, or a bare `format=nv12`
    /// depending on where frames live.
    fn hw_filter_suffix(&self, _treatment: HdrTreatment) -> Option<String> {
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

    fn supports_gpu_resident_overlay(&self) -> bool {
        true
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
        treatment: HdrTreatment,
    ) -> Vec<String> {
        match treatment {
            // tonemapx needs CPU frames; device chain still needed for the
            // QSV encoder.
            HdrTreatment::SwTonemap => self.init_only_args(),
            _ => self.input_args(),
        }
    }

    fn hw_filter_suffix(&self, treatment: HdrTreatment) -> Option<String> {
        match treatment {
            HdrTreatment::Sdr => self.filter_suffix(),
            HdrTreatment::VppTonemap => Some(
                "tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709:extra_hw_frames=32,\
                 hwmap=derive_device=qsv,format=qsv"
                    .to_string(),
            ),
            // Frames are already NV12 after scale_vaapi; setparams is
            // metadata-only and passes QSV surfaces through untouched.
            HdrTreatment::Clamp => Some(
                "hwmap=derive_device=qsv,format=qsv,\
                 setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709"
                    .to_string(),
            ),
            // CPU frames after tonemapx; format=nv12 before the encoder.
            HdrTreatment::SwTonemap => Some("format=nv12".to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn qsv() -> Qsv {
        Qsv {
            vaapi_device: "/dev/dri/renderD128".into(),
            vaapi_driver: "iHD".into(),
        }
    }

    #[test]
    fn sdr_source_ignores_tonemap_toggles() {
        assert_eq!(
            HdrTreatment::for_source(false, &qsv(), true, true, false),
            HdrTreatment::Sdr
        );
    }

    #[test]
    fn vpp_wins_over_sw_when_accelerator_supports_it() {
        assert_eq!(
            HdrTreatment::for_source(true, &qsv(), true, true, false),
            HdrTreatment::VppTonemap
        );
        assert_eq!(
            HdrTreatment::for_source(true, &qsv(), false, true, false),
            HdrTreatment::VppTonemap
        );
    }

    #[test]
    fn vpp_toggle_falls_back_to_clamp_without_hw_tonemap() {
        // NVENC has no VPP tone mapper; with only the VPP toggle on nothing
        // tone-maps and the source is clamped.
        assert_eq!(
            HdrTreatment::for_source(true, &Nvenc, false, true, false),
            HdrTreatment::Clamp
        );
        assert_eq!(
            HdrTreatment::for_source(true, &Nvenc, true, true, false),
            HdrTreatment::SwTonemap
        );
    }

    #[test]
    fn vpp_toggle_still_sw_tonemaps_on_non_vpp_accelerator_when_burning_a_subtitle() {
        // NVENC never supports VPP tonemap at all, but a burn-in still means
        // the user asked for *some* tone mapping — must not silently clamp
        // just because this accelerator could never have done VPP anyway.
        assert_eq!(
            HdrTreatment::for_source(true, &Nvenc, false, true, true),
            HdrTreatment::SwTonemap
        );
    }

    #[test]
    fn explicit_sw_tonemap_and_clamp() {
        assert_eq!(
            HdrTreatment::for_source(true, &qsv(), true, false, false),
            HdrTreatment::SwTonemap
        );
        assert_eq!(
            HdrTreatment::for_source(true, &qsv(), false, false, false),
            HdrTreatment::Clamp
        );
    }

    #[test]
    fn qsv_vpp_tonemap_survives_subtitle_burn_in() {
        // QSV's overlay_qsv composites onto GPU-resident frames, so a
        // burn-in doesn't block VPP tone mapping.
        assert_eq!(
            HdrTreatment::for_source(true, &qsv(), false, true, true),
            HdrTreatment::VppTonemap
        );
    }

    #[test]
    fn vaapi_vpp_tonemap_falls_back_to_sw_when_burning_a_subtitle() {
        // VAAPI has no GPU-resident overlay: tonemap_vaapi output can't
        // feed the CPU `overlay` filter (issue #378), so a burn-in must
        // force software tone mapping instead of silently dropping it.
        let vaapi = Vaapi {
            device: "/dev/dri/renderD128".into(),
            driver: "iHD".into(),
        };
        assert_eq!(
            HdrTreatment::for_source(true, &vaapi, false, true, true),
            HdrTreatment::SwTonemap
        );
        // Without a burn-in, VAAPI still gets VPP tone mapping.
        assert_eq!(
            HdrTreatment::for_source(true, &vaapi, false, true, false),
            HdrTreatment::VppTonemap
        );
    }

    #[test]
    fn videotoolbox_always_prefers_sw_tonemap() {
        let vt = VideoToolbox {
            av1_hw_decode: false,
        };
        assert_eq!(
            HdrTreatment::for_source(true, &vt, false, false, false),
            HdrTreatment::SwTonemap
        );
        assert_eq!(
            HdrTreatment::for_source(true, &vt, false, true, false),
            HdrTreatment::SwTonemap
        );
    }

    #[test]
    fn qsv_only_sw_tonemap_decodes_in_software() {
        let q = qsv();
        for t in [
            HdrTreatment::Sdr,
            HdrTreatment::Clamp,
            HdrTreatment::VppTonemap,
        ] {
            let args = q.decode_input_args(None, t);
            assert!(
                args.iter()
                    .any(|a| a == "-hwaccel_output_format"),
                "{t:?} should hw-decode: {args:?}"
            );
        }
        let args = q.decode_input_args(None, HdrTreatment::SwTonemap);
        assert!(
            !args
                .iter()
                .any(|a| a == "-hwaccel"),
            "{args:?}"
        );
        assert!(
            args.iter()
                .any(|a| a == "qsv=qs@va"),
            "{args:?}"
        );
    }

    #[test]
    fn qsv_suffix_per_treatment() {
        let q = qsv();
        assert_eq!(
            q.hw_filter_suffix(HdrTreatment::Sdr)
                .unwrap(),
            "hwmap=derive_device=qsv,format=qsv"
        );
        let clamp = q
            .hw_filter_suffix(HdrTreatment::Clamp)
            .unwrap();
        assert!(clamp.starts_with("hwmap=derive_device=qsv,format=qsv,setparams="));
        let vpp = q
            .hw_filter_suffix(HdrTreatment::VppTonemap)
            .unwrap();
        assert!(vpp.starts_with("tonemap_vaapi=") && vpp.ends_with("format=qsv"));
        assert_eq!(
            q.hw_filter_suffix(HdrTreatment::SwTonemap)
                .unwrap(),
            "format=nv12"
        );
    }
}
