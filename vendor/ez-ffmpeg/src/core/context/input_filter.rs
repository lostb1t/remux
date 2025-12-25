use crate::core::context::null_frame;
use ffmpeg_sys_next::{AVMediaType, AVRational};
use ffmpeg_next::Frame;

pub(crate) struct InputFilter {
    pub(crate) linklabel: String,
    pub(crate) media_type: AVMediaType,
    pub(crate) name: String,
    pub(crate) opts: InputFilterOptions,
}

impl InputFilter {
    pub(crate) fn new(linklabel: String, media_type: AVMediaType, name: String, fallback: Frame) -> Self {
        Self {
            linklabel,
            media_type,
            name,
            opts: InputFilterOptions::new(fallback),
        }
    }
}

pub(crate) const IFILTER_FLAG_AUTOROTATE: u32 = 1 << 0;
#[allow(dead_code)]
pub(crate) const IFILTER_FLAG_REINIT: u32 = 1 << 1;
#[allow(dead_code)]
pub(crate) const IFILTER_FLAG_CFR: u32 = 1 << 2;
#[allow(dead_code)]
pub(crate) const IFILTER_FLAG_CROP: u32 = 1 << 3;

pub(crate) struct InputFilterOptions {
    pub(crate) trim_start_us: Option<i64>,
    pub(crate) trim_end_us: Option<i64>,
    
    pub(crate) name: String,
    pub(crate) framerate: AVRational,
    #[allow(dead_code)]
    pub(crate) crop_top: u32,
    #[allow(dead_code)]
    pub(crate) crop_bottom: u32,
    #[allow(dead_code)]
    pub(crate) crop_left: u32,
    #[allow(dead_code)]
    pub(crate) crop_right: u32,

    pub(crate) sub2video_width: i32,
    pub(crate) sub2video_height: i32,

    pub(crate) flags: u32,

    pub(crate) fallback:Frame,
}

impl InputFilterOptions {
    pub(crate) fn new(fallback: Frame) -> Self {
        Self {
            trim_start_us: None,
            trim_end_us: None,
            name: "".to_string(),
            framerate: AVRational { num: 0, den: 0 },
            crop_top: 0,
            crop_bottom: 0,
            crop_left: 0,
            crop_right: 0,
            sub2video_width: 0,
            sub2video_height: 0,
            flags: 0,
            fallback,
        }
    }
    pub(crate) fn empty() -> Self {
        Self {
            trim_start_us: None,
            trim_end_us: None,
            name: "".to_string(),
            framerate: AVRational { num: 0, den: 0 },
            crop_top: 0,
            crop_bottom: 0,
            crop_left: 0,
            crop_right: 0,
            sub2video_width: 0,
            sub2video_height: 0,
            flags: 0,
            fallback:null_frame(),
        }
    }
}
