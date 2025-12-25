use crate::core::context::output::VSyncMethod;
use crate::core::context::FrameBox;
use crossbeam_channel::Sender;
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::{AVChannelLayout, AVChannelLayout__bindgen_ty_1, AVChannelOrder};
use ffmpeg_sys_next::{AVCodec, AVColorRange, AVColorSpace, AVMediaType, AVPixelFormat, AVRational, AVSampleFormat};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Clone)]
pub(crate) struct OutputFilter {
    pub(crate) linklabel: String,
    pub(crate) media_type: AVMediaType,
    pub(crate) name: String,
    pub(crate) opts: OutputFilterOptions,
    pub(crate) fg_input_index: usize,
    pub(crate) finished_flag_list: Arc<[AtomicBool]>,
    dst: Option<Sender<FrameBox>>,
}

impl OutputFilter {
    pub(crate) fn new(linklabel: String, media_type: AVMediaType, name: String) -> Self {
        Self {
            linklabel,
            media_type,
            name,
            opts: OutputFilterOptions::new(),
            fg_input_index: 0,
            finished_flag_list: Arc::new([]),
            dst: None,
        }
    }

    pub(crate) fn set_dst(&mut self, sender: Sender<FrameBox>) {
        self.dst = Some(sender);
    }

    pub(crate) fn has_dst(&self) -> bool {
        self.dst.is_some()
    }

    pub(crate) fn take_dst(&mut self) -> Option<Sender<FrameBox>> {
        self.dst.take()
    }
}

pub(crate) const OFILTER_FLAG_DISABLE_CONVERT: u32 = 1 << 0;
pub(crate) const OFILTER_FLAG_AUDIO_24BIT: u32 = 1 << 1;
pub(crate) const OFILTER_FLAG_AUTOSCALE: u32 = 1 << 2;

#[derive(Clone)]
pub(crate) struct OutputFilterOptions {
    pub(crate) name: String,
    pub(crate) enc: *const AVCodec,
    #[allow(dead_code)]
    pub(crate) format: AVPixelFormat,
    pub(crate) formats: Option<Vec<AVPixelFormat>>,
    pub(crate) audio_format: AVSampleFormat,
    pub(crate) audio_formats: Option<Vec<AVSampleFormat>>,
    pub(crate) framerate: AVRational,
    pub(crate) framerates: Option<Vec<AVRational>>,
    #[allow(dead_code)]
    pub(crate) color_space: AVColorSpace,
    pub(crate) color_spaces: Option<Vec<AVColorSpace>>,
    #[allow(dead_code)]
    pub(crate) color_range: AVColorRange,
    pub(crate) color_ranges: Option<Vec<AVColorRange>>,
    pub(crate) vsync_method: Option<VSyncMethod>,
    pub(crate) sample_rate: i32,
    pub(crate) sample_rates: Option<Vec<i32>>,
    #[cfg(not(feature = "docs-rs"))]
    pub(crate) ch_layout: AVChannelLayout,
    #[cfg(not(feature = "docs-rs"))]
    pub(crate) ch_layouts: Option<Vec<AVChannelLayout>>,
    pub(crate) trim_start_us: Option<i64>,
    pub(crate) trim_duration_us: Option<i64>,
    pub(crate) ts_offset: Option<i64>,
    pub(crate) flags: u32,
}
unsafe impl Send for OutputFilterOptions {}
unsafe impl Sync for OutputFilterOptions {}

impl OutputFilterOptions {
    pub(crate) fn new() -> Self {
        Self {
            name: "".to_string(),
            enc: null(),
            format: AVPixelFormat::AV_PIX_FMT_NONE,
            formats: None,
            audio_format: AVSampleFormat::AV_SAMPLE_FMT_NONE,
            audio_formats: None,
            framerate: AVRational { num: 0, den: 0 },
            framerates: None,
            color_space: AVColorSpace::AVCOL_SPC_UNSPECIFIED,
            color_spaces: None,
            color_range: AVColorRange::AVCOL_RANGE_UNSPECIFIED,
            color_ranges: None,
            vsync_method: None,
            sample_rate: 0,
            sample_rates: None,
            #[cfg(not(feature = "docs-rs"))]
            ch_layout: AVChannelLayout {
                order: AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC,
                nb_channels: 0,
                u: AVChannelLayout__bindgen_ty_1 { mask: 0 },
                opaque: null_mut(),
            },
            #[cfg(not(feature = "docs-rs"))]
            ch_layouts: None,
            trim_start_us: None,
            trim_duration_us: None,
            ts_offset: None,
            flags: 0,
        }
    }
}
