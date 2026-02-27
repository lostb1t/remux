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

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply autorotate flag based on the autorotate setting.
    /// This mirrors the logic in `ffmpeg_context.rs`:
    /// - If autorotate is true, set IFILTER_FLAG_AUTOROTATE
    /// - If autorotate is false, do not set the flag
    fn apply_autorotate_flag(flags: u32, autorotate: bool) -> u32 {
        if autorotate {
            flags | IFILTER_FLAG_AUTOROTATE
        } else {
            flags
        }
    }

    #[test]
    fn autorotate_true_sets_flag() {
        let flags = 0u32;
        let result = apply_autorotate_flag(flags, true);
        assert_eq!(result & IFILTER_FLAG_AUTOROTATE, IFILTER_FLAG_AUTOROTATE);
    }

    #[test]
    fn autorotate_false_does_not_set_flag() {
        let flags = 0u32;
        let result = apply_autorotate_flag(flags, false);
        assert_eq!(result & IFILTER_FLAG_AUTOROTATE, 0);
    }

    #[test]
    fn autorotate_preserves_other_flags() {
        // Start with other flags set
        let flags = IFILTER_FLAG_REINIT | IFILTER_FLAG_CFR;

        // autorotate=true should add AUTOROTATE without removing others
        let result = apply_autorotate_flag(flags, true);
        assert_eq!(result & IFILTER_FLAG_AUTOROTATE, IFILTER_FLAG_AUTOROTATE);
        assert_eq!(result & IFILTER_FLAG_REINIT, IFILTER_FLAG_REINIT);
        assert_eq!(result & IFILTER_FLAG_CFR, IFILTER_FLAG_CFR);

        // autorotate=false should not modify other flags
        let result = apply_autorotate_flag(flags, false);
        assert_eq!(result & IFILTER_FLAG_AUTOROTATE, 0);
        assert_eq!(result & IFILTER_FLAG_REINIT, IFILTER_FLAG_REINIT);
        assert_eq!(result & IFILTER_FLAG_CFR, IFILTER_FLAG_CFR);
    }

    #[test]
    fn autorotate_flag_is_bit_0() {
        // Verify IFILTER_FLAG_AUTOROTATE is correctly defined as bit 0
        assert_eq!(IFILTER_FLAG_AUTOROTATE, 1);
        assert_eq!(IFILTER_FLAG_AUTOROTATE, 1 << 0);
    }

    #[test]
    fn autorotate_idempotent() {
        // Applying autorotate=true multiple times should have same effect
        let flags = 0u32;
        let result1 = apply_autorotate_flag(flags, true);
        let result2 = apply_autorotate_flag(result1, true);
        assert_eq!(result1, result2);
    }

    #[test]
    fn input_filter_options_default_flags() {
        // InputFilterOptions should start with flags = 0
        let opts = InputFilterOptions::empty();
        assert_eq!(opts.flags, 0);
        assert_eq!(opts.flags & IFILTER_FLAG_AUTOROTATE, 0);
    }

    #[test]
    fn autorotate_false_preserves_existing_autorotate_flag() {
        // If AUTOROTATE is already set and autorotate=false is applied,
        // the current implementation preserves the existing flag (does not clear it).
        // This documents the actual behavior.
        let flags = IFILTER_FLAG_AUTOROTATE | IFILTER_FLAG_REINIT;

        let result = apply_autorotate_flag(flags, false);

        // autorotate=false does NOT clear the flag, it just doesn't set it
        // So if it was already set, it remains set
        assert_eq!(result & IFILTER_FLAG_AUTOROTATE, IFILTER_FLAG_AUTOROTATE);
        assert_eq!(result & IFILTER_FLAG_REINIT, IFILTER_FLAG_REINIT);
    }

    #[test]
    fn all_filter_flags_are_distinct() {
        // Verify all filter flags use different bits (no overlap)
        assert_eq!(IFILTER_FLAG_AUTOROTATE & IFILTER_FLAG_REINIT, 0);
        assert_eq!(IFILTER_FLAG_AUTOROTATE & IFILTER_FLAG_CFR, 0);
        assert_eq!(IFILTER_FLAG_AUTOROTATE & IFILTER_FLAG_CROP, 0);
        assert_eq!(IFILTER_FLAG_REINIT & IFILTER_FLAG_CFR, 0);
        assert_eq!(IFILTER_FLAG_REINIT & IFILTER_FLAG_CROP, 0);
        assert_eq!(IFILTER_FLAG_CFR & IFILTER_FLAG_CROP, 0);

        // Verify expected bit positions
        assert_eq!(IFILTER_FLAG_AUTOROTATE, 1 << 0);
        assert_eq!(IFILTER_FLAG_REINIT, 1 << 1);
        assert_eq!(IFILTER_FLAG_CFR, 1 << 2);
        assert_eq!(IFILTER_FLAG_CROP, 1 << 3);
    }

    #[test]
    fn autorotate_with_all_flags_set() {
        // Test with all flags already set
        let all_flags = IFILTER_FLAG_AUTOROTATE | IFILTER_FLAG_REINIT | IFILTER_FLAG_CFR | IFILTER_FLAG_CROP;

        // autorotate=true should not change anything (already set)
        let result = apply_autorotate_flag(all_flags, true);
        assert_eq!(result, all_flags);

        // autorotate=false should preserve all flags (including AUTOROTATE)
        let result = apply_autorotate_flag(all_flags, false);
        assert_eq!(result, all_flags);
    }
}
