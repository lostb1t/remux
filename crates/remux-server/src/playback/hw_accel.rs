use std::sync::atomic::{AtomicBool, Ordering};

use remux_sdks::remux::{EncodingOptions, HardwareAccelerationType};

/// Whether ffmpeg can derive an OpenCL device from the VAAPI device and has
/// `tonemap_opencl`. A runtime property of the host, probed once at startup
/// and kept out of the persisted encoding settings so a dashboard save cannot
/// clear it.
static OPENCL_TONEMAP_AVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn set_opencl_tonemap_available(available: bool) {
    OPENCL_TONEMAP_AVAILABLE.store(available, Ordering::Relaxed);
}

fn opencl_tonemap_available() -> bool {
    OPENCL_TONEMAP_AVAILABLE.load(Ordering::Relaxed)
}

/// User tone-mapping settings that GPU tone mappers take as filter options.
#[derive(Debug, Clone, Copy)]
pub struct TonemapOptions<'a> {
    pub algorithm: &'a str,
    pub desat: f32,
    pub peak: f32,
}

impl TonemapOptions<'_> {
    /// `tonemap_opencl` has no `bt2446a`; BT.2390 is the closest curve and
    /// Jellyfin's default.
    fn opencl_algorithm(&self) -> &str {
        match self.algorithm {
            "none" | "linear" | "gamma" | "clip" | "reinhard" | "hable" | "mobius"
            | "bt2390" => self.algorithm,
            _ => "bt2390",
        }
    }
}

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
    /// Tone mapping on the GPU through OpenCL (`tonemap_opencl`), with frames
    /// mapped VAAPI → OpenCL → VAAPI without leaving video memory. Takes the
    /// software tone-mapping settings (algorithm, peak, desat).
    OclTonemap,
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
            && accel.supports_ocl_tonemap()
            && !vpp_blocked_by_overlay
        {
            Self::OclTonemap
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

    /// True when the accelerator can run `tonemap_opencl` on its decoded
    /// surfaces through VAAPI–OpenCL interop.
    fn supports_ocl_tonemap(&self) -> bool {
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
    /// `needs_cpu_overlay` is true when a subtitle burn-in will run through
    /// the CPU `overlay` filter (i.e. `!supports_gpu_resident_overlay()`).
    /// That filter reads `0:v:0` directly with no `hwdownload`, so an
    /// accelerator whose `input_args()` requests GPU-resident frames (QSV,
    /// VAAPI) must fall back to a software decode whenever this is true,
    /// regardless of `treatment` — `HdrTreatment::for_source` only routes
    /// *tone-mapping* treatments away from GPU residency for this case
    /// (`SwTonemap`), not `Sdr`/`Clamp`, which have nothing to do with tone
    /// mapping but still end up on the same CPU overlay path.
    ///
    /// The default skips `input_args()` when `requires_software_decode` is
    /// set. QSV/VAAPI override to also emit device-init-only args when
    /// software tone mapping (or this CPU-overlay case) needs CPU frames but
    /// the encoder still requires the device chain.
    fn decode_input_args(
        &self,
        source_codec: Option<&str>,
        treatment: HdrTreatment,
        _needs_cpu_overlay: bool,
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
    fn hw_filter_suffix(
        &self,
        _treatment: HdrTreatment,
        _tonemap: &TonemapOptions,
    ) -> Option<String> {
        self.filter_suffix()
    }

    fn as_type(&self) -> HardwareAccelerationType;
}

pub struct NoAccel;
pub struct Nvenc;
pub struct Vaapi {
    pub device: String,
    pub driver: String,
    /// OpenCL tone mapping is usable on this host.
    pub opencl: bool,
}
pub struct Qsv {
    pub vaapi_device: String,
    pub vaapi_driver: String,
    /// OpenCL tone mapping is usable on this host.
    pub opencl: bool,
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

impl Vaapi {
    // Device-init args without the hwaccel decode flags — used when HDR source
    // needs software decode but the VAAPI encoder still requires the device chain.
    fn init_only_args(&self) -> Vec<String> {
        self.device_args(false)
    }

    // With `opencl`, an OpenCL device is derived from the VAAPI one (which
    // enables media sharing for zero-copy `hwmap`) and becomes the filter
    // device, mirroring Jellyfin's VAAPI + OpenCL pipeline.
    fn device_args(&self, opencl: bool) -> Vec<String> {
        let driver_opt = if self
            .driver
            .is_empty()
        {
            String::new()
        } else {
            format!(",driver={}", self.driver)
        };
        let mut args = vec![
            "-init_hw_device".into(),
            format!("vaapi=va:{}{}", self.device, driver_opt),
        ];
        if opencl {
            args.extend([
                "-init_hw_device".into(),
                "opencl=ocl@va".into(),
                "-filter_hw_device".into(),
                "ocl".into(),
            ]);
        } else {
            args.extend(["-filter_hw_device".into(), "va".into()]);
        }
        args
    }

    fn hw_decode_args(&self, opencl: bool) -> Vec<String> {
        let mut args = self.device_args(opencl);
        args.extend([
            "-hwaccel".into(),
            "vaapi".into(),
            "-hwaccel_output_format".into(),
            "vaapi".into(),
        ]);
        args
    }
}

impl Accelerator for Vaapi {
    fn encoder_suffix(&self) -> Option<&str> {
        Some("_vaapi")
    }

    fn input_args(&self) -> Vec<String> {
        self.hw_decode_args(false)
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

    fn supports_ocl_tonemap(&self) -> bool {
        self.opencl
    }

    fn decode_input_args(
        &self,
        _source_codec: Option<&str>,
        treatment: HdrTreatment,
        needs_cpu_overlay: bool,
    ) -> Vec<String> {
        // The CPU `overlay` filter reads `0:v:0` directly with no
        // `hwdownload`, so an accelerator without a GPU-resident overlay
        // (VAAPI has none — see `supports_gpu_resident_overlay`) must fall
        // back to software decode whenever a burn-in needs it, regardless of
        // treatment: `Sdr`/`Clamp` would otherwise still request
        // VAAPI-resident frames here.
        if needs_cpu_overlay && !self.supports_gpu_resident_overlay() {
            return self.init_only_args();
        }
        match treatment {
            // tonemapx needs CPU frames; device chain still needed for the
            // VAAPI encoder.
            HdrTreatment::SwTonemap => self.init_only_args(),
            HdrTreatment::OclTonemap => self.hw_decode_args(true),
            _ => self.input_args(),
        }
    }

    fn hw_filter_suffix(
        &self,
        treatment: HdrTreatment,
        tonemap: &TonemapOptions,
    ) -> Option<String> {
        match treatment {
            // Frames are already VAAPI-format post-decode; a native
            // h264_vaapi/hevc_vaapi encoder needs no hop at all (unlike QSV,
            // which always needs a final hwmap into QSV memory).
            HdrTreatment::Sdr => None,
            // Same brightness lift as QSV's VPP path; no trailing hop needed.
            HdrTreatment::VppTonemap => Some(
                "procamp_vaapi=b=16,\
                 tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709:extra_hw_frames=32"
                    .to_string(),
            ),
            // P010 VAAPI surface → OpenCL (read) → tone map → back into the
            // same VAAPI surface pool (reverse map) — one hop, since the
            // encoder is already native VAAPI (Jellyfin's `isVaInVaOut` case).
            HdrTreatment::OclTonemap => Some(format!(
                "hwmap=derive_device=opencl:mode=read,\
                 tonemap_opencl=format=nv12:p=bt709:t=bt709:m=bt709:\
                 tonemap={algo}:peak={peak}:desat={desat},\
                 hwmap=derive_device=vaapi:mode=write:reverse=1,format=vaapi",
                algo = tonemap.opencl_algorithm(),
                peak = tonemap.peak,
                desat = tonemap.desat,
            )),
            // Frames are already NV12 after scale_vaapi; setparams is
            // metadata-only and passes VAAPI surfaces through untouched.
            HdrTreatment::Clamp => Some(
                "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709"
                    .to_string(),
            ),
            // CPU frames after tonemapx; upload for the VAAPI encoder.
            HdrTreatment::SwTonemap => Some("format=nv12,hwupload".to_string()),
        }
    }

    fn as_type(&self) -> HardwareAccelerationType {
        HardwareAccelerationType::Vaapi
    }
}

impl Qsv {
    // Device-init args without the hwaccel decode flags — used when HDR source
    // needs software decode but the QSV encoder still requires the device chain.
    fn init_only_args(&self) -> Vec<String> {
        self.device_args(false)
    }

    // With `opencl`, an OpenCL device is derived from the VAAPI one (which
    // enables media sharing for zero-copy `hwmap`) and becomes the filter
    // device, as in Jellyfin's QSV + OpenCL pipeline.
    fn device_args(&self, opencl: bool) -> Vec<String> {
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
        let mut args = vec![
            "-init_hw_device".into(),
            format!("vaapi=va:{}{}", self.vaapi_device, driver_opt),
            "-init_hw_device".into(),
            "qsv=qs@va".into(),
        ];
        if opencl {
            args.extend([
                "-init_hw_device".into(),
                "opencl=ocl@va".into(),
                "-filter_hw_device".into(),
                "ocl".into(),
            ]);
        } else {
            args.extend(["-filter_hw_device".into(), "qs".into()]);
        }
        args
    }

    fn hw_decode_args(&self, opencl: bool) -> Vec<String> {
        let mut args = self.device_args(opencl);
        args.extend([
            "-hwaccel".into(),
            "vaapi".into(),
            "-hwaccel_output_format".into(),
            "vaapi".into(),
        ]);
        args
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
        self.hw_decode_args(false)
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

    fn supports_ocl_tonemap(&self) -> bool {
        self.opencl
    }

    fn decode_input_args(
        &self,
        _source_codec: Option<&str>,
        treatment: HdrTreatment,
        needs_cpu_overlay: bool,
    ) -> Vec<String> {
        // QSV has `overlay_qsv`, a GPU-resident overlay, so a burn-in never
        // forces software decode here the way it does for plain VAAPI — kept
        // for symmetry with `Vaapi::decode_input_args` and so a future
        // accelerator without a GPU-resident overlay gets this for free.
        if needs_cpu_overlay && !self.supports_gpu_resident_overlay() {
            return self.init_only_args();
        }
        match treatment {
            // tonemapx needs CPU frames; device chain still needed for the
            // QSV encoder.
            HdrTreatment::SwTonemap => self.init_only_args(),
            HdrTreatment::OclTonemap => self.hw_decode_args(true),
            _ => self.input_args(),
        }
    }

    fn hw_filter_suffix(
        &self,
        treatment: HdrTreatment,
        tonemap: &TonemapOptions,
    ) -> Option<String> {
        match treatment {
            HdrTreatment::Sdr => self.filter_suffix(),
            // The VPP curve maps the whole mastering range into SDR and leaves
            // midtones dark; Jellyfin lifts them with its default
            // `VppTonemappingBrightness` of 16 ahead of the tone map.
            HdrTreatment::VppTonemap => Some(
                "procamp_vaapi=b=16,\
                 tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709:extra_hw_frames=32,\
                 hwmap=derive_device=qsv,format=qsv"
                    .to_string(),
            ),
            // P010 VAAPI surface → OpenCL (read) → tone map → back into QSV
            // memory in one reverse map, as Jellyfin's QSV+OpenCL pipeline
            // does. `extra_hw_frames=16` on the reverse map — not the tone
            // map itself, which has no such option — follows Jellyfin's own
            // comment: without it hevc_qsv can fail to allocate memory.
            HdrTreatment::OclTonemap => Some(format!(
                "hwmap=derive_device=opencl:mode=read,\
                 tonemap_opencl=format=nv12:p=bt709:t=bt709:m=bt709:\
                 tonemap={algo}:peak={peak}:desat={desat},\
                 hwmap=derive_device=qsv:mode=write:reverse=1:extra_hw_frames=16,\
                 format=qsv",
                algo = tonemap.opencl_algorithm(),
                peak = tonemap.peak,
                desat = tonemap.desat,
            )),
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
        HardwareAccelerationType::Vaapi => Box::new(Vaapi {
            device,
            driver,
            opencl: opencl_tonemap_available(),
        }),
        HardwareAccelerationType::Qsv => Box::new(Qsv {
            vaapi_device: device,
            vaapi_driver: driver,
            opencl: opencl_tonemap_available(),
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
            opencl: false,
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
            opencl: false,
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
            let args = q.decode_input_args(None, t, false);
            assert!(
                args.iter()
                    .any(|a| a == "-hwaccel_output_format"),
                "{t:?} should hw-decode: {args:?}"
            );
        }
        let args = q.decode_input_args(None, HdrTreatment::SwTonemap, false);
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

    fn qsv_opencl() -> Qsv {
        Qsv {
            opencl: true,
            ..qsv()
        }
    }

    #[test]
    fn qsv_tone_mapping_runs_on_opencl_when_available() {
        assert_eq!(
            HdrTreatment::for_source(true, &qsv_opencl(), true, false, false),
            HdrTreatment::OclTonemap
        );
        // overlay_qsv composites after the tone map, so burn-in keeps OpenCL.
        assert_eq!(
            HdrTreatment::for_source(true, &qsv_opencl(), true, false, true),
            HdrTreatment::OclTonemap
        );
        assert_eq!(
            HdrTreatment::for_source(true, &qsv(), true, false, false),
            HdrTreatment::SwTonemap,
            "no OpenCL runtime: fall back to tonemapx"
        );
        assert_eq!(
            HdrTreatment::for_source(true, &qsv_opencl(), true, true, false),
            HdrTreatment::VppTonemap,
            "VPP wins when both are enabled, as in Jellyfin"
        );
    }

    #[test]
    fn qsv_opencl_decodes_on_gpu_with_opencl_filter_device() {
        let args =
            qsv_opencl().decode_input_args(None, HdrTreatment::OclTonemap, false);
        let pair = |k: &str, v: &str| {
            args.windows(2)
                .any(|w| w[0] == k && w[1] == v)
        };
        assert!(pair("-init_hw_device", "opencl=ocl@va"), "{args:?}");
        assert!(pair("-filter_hw_device", "ocl"), "{args:?}");
        assert!(pair("-hwaccel_output_format", "vaapi"), "{args:?}");
        assert!(!pair("-filter_hw_device", "qs"), "{args:?}");
    }

    #[test]
    fn qsv_opencl_suffix_maps_through_opencl_and_back() {
        let bt2390 = TonemapOptions {
            algorithm: "bt2390",
            desat: 0.0,
            peak: 100.0,
        };
        let s = qsv_opencl()
            .hw_filter_suffix(HdrTreatment::OclTonemap, &bt2390)
            .unwrap();
        assert!(
            s.starts_with("hwmap=derive_device=opencl:mode=read,"),
            "{s}"
        );
        assert!(
            s.contains("tonemap_opencl=format=nv12:p=bt709:t=bt709:m=bt709:tonemap=bt2390:peak=100:desat=0"),
            "{s}"
        );
        assert!(
            s.ends_with(
                "hwmap=derive_device=qsv:mode=write:reverse=1:extra_hw_frames=16,format=qsv"
            ),
            "{s}"
        );
        let unsupported = TonemapOptions {
            algorithm: "bt2446a",
            ..bt2390
        };
        let s = qsv_opencl()
            .hw_filter_suffix(HdrTreatment::OclTonemap, &unsupported)
            .unwrap();
        assert!(
            s.contains("tonemap=bt2390"),
            "bt2446a has no OpenCL kernel: {s}"
        );
    }

    const HABLE: TonemapOptions<'static> = TonemapOptions {
        algorithm: "hable",
        desat: 0.0,
        peak: 0.0,
    };

    #[test]
    fn qsv_suffix_per_treatment() {
        let q = qsv();
        assert_eq!(
            q.hw_filter_suffix(HdrTreatment::Sdr, &HABLE)
                .unwrap(),
            "hwmap=derive_device=qsv,format=qsv"
        );
        let clamp = q
            .hw_filter_suffix(HdrTreatment::Clamp, &HABLE)
            .unwrap();
        assert!(clamp.starts_with("hwmap=derive_device=qsv,format=qsv,setparams="));
        let vpp = q
            .hw_filter_suffix(HdrTreatment::VppTonemap, &HABLE)
            .unwrap();
        assert!(
            vpp.starts_with("procamp_vaapi=b=16,tonemap_vaapi=")
                && vpp.ends_with("format=qsv")
        );
        assert_eq!(
            q.hw_filter_suffix(HdrTreatment::SwTonemap, &HABLE)
                .unwrap(),
            "format=nv12"
        );
    }

    fn vaapi() -> Vaapi {
        Vaapi {
            device: "/dev/dri/renderD128".into(),
            driver: "iHD".into(),
            opencl: false,
        }
    }

    fn vaapi_opencl() -> Vaapi {
        Vaapi {
            opencl: true,
            ..vaapi()
        }
    }

    #[test]
    fn vaapi_tone_mapping_runs_on_opencl_when_available() {
        assert_eq!(
            HdrTreatment::for_source(true, &vaapi_opencl(), true, false, false),
            HdrTreatment::OclTonemap
        );
        // VAAPI has no GPU-resident overlay, so unlike QSV a burn-in still
        // blocks OpenCL tone mapping and falls back to software.
        assert_eq!(
            HdrTreatment::for_source(true, &vaapi_opencl(), true, false, true),
            HdrTreatment::SwTonemap
        );
        assert_eq!(
            HdrTreatment::for_source(true, &vaapi(), true, false, false),
            HdrTreatment::SwTonemap,
            "no OpenCL runtime: fall back to tonemapx"
        );
        assert_eq!(
            HdrTreatment::for_source(true, &vaapi_opencl(), true, true, false),
            HdrTreatment::VppTonemap,
            "VPP wins when both are enabled, as in Jellyfin"
        );
    }

    #[test]
    fn vaapi_opencl_decodes_on_gpu_with_opencl_filter_device() {
        let args =
            vaapi_opencl().decode_input_args(None, HdrTreatment::OclTonemap, false);
        let pair = |k: &str, v: &str| {
            args.windows(2)
                .any(|w| w[0] == k && w[1] == v)
        };
        assert!(pair("-init_hw_device", "opencl=ocl@va"), "{args:?}");
        assert!(pair("-filter_hw_device", "ocl"), "{args:?}");
        assert!(pair("-hwaccel_output_format", "vaapi"), "{args:?}");
        assert!(!pair("-filter_hw_device", "va"), "{args:?}");
    }

    #[test]
    fn vaapi_opencl_suffix_maps_through_opencl_and_back() {
        let bt2390 = TonemapOptions {
            algorithm: "bt2390",
            desat: 0.0,
            peak: 100.0,
        };
        let s = vaapi_opencl()
            .hw_filter_suffix(HdrTreatment::OclTonemap, &bt2390)
            .unwrap();
        assert!(
            s.starts_with("hwmap=derive_device=opencl:mode=read,"),
            "{s}"
        );
        assert!(
            s.contains("tonemap_opencl=format=nv12:p=bt709:t=bt709:m=bt709:tonemap=bt2390:peak=100:desat=0"),
            "{s}"
        );
        assert!(
            s.ends_with("hwmap=derive_device=vaapi:mode=write:reverse=1,format=vaapi"),
            "{s}"
        );
    }

    #[test]
    fn vaapi_suffix_per_treatment() {
        let v = vaapi();
        assert_eq!(v.hw_filter_suffix(HdrTreatment::Sdr, &HABLE), None);
        let clamp = v
            .hw_filter_suffix(HdrTreatment::Clamp, &HABLE)
            .unwrap();
        assert_eq!(
            clamp,
            "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709"
        );
        let vpp = v
            .hw_filter_suffix(HdrTreatment::VppTonemap, &HABLE)
            .unwrap();
        assert_eq!(
            vpp,
            "procamp_vaapi=b=16,tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709:extra_hw_frames=32"
        );
        assert_eq!(
            v.hw_filter_suffix(HdrTreatment::SwTonemap, &HABLE)
                .unwrap(),
            "format=nv12,hwupload"
        );
    }
}
