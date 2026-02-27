// src/rtmp/gop.rs - Optimized version (Zero-Copy GOP)
//
// Optimizations:
// 1. FrozenGop uses Arc<[FrameData]> for O(1) clone
// 2. Separates frozen (completed) and current (writing) GOP
// 3. Keeps old API compatibility layer, marked deprecated

use bytes::Bytes;
use rml_rtmp::time::RtmpTimestamp;
use std::collections::VecDeque;
use std::sync::Arc;

/// Frame data - Structure unchanged
/// ⚠️ Note: Keyframe detection is caller's responsibility, not implemented here
#[derive(Clone)]
pub(crate) enum FrameData {
    Video {
        timestamp: RtmpTimestamp,
        data: Bytes,
    },
    Audio {
        timestamp: RtmpTimestamp,
        data: Bytes,
    },
}

/// Frozen GOP - Immutable, O(1) clone
///
/// Uses `Arc<[FrameData]>` instead of `Arc<Vec<FrameData>>` to reduce indirection
#[derive(Clone)]
pub struct FrozenGop {
    frames: Arc<[FrameData]>,
}

impl FrozenGop {
    fn new(frames: Vec<FrameData>) -> Self {
        Self {
            frames: Arc::from(frames.into_boxed_slice()),
        }
    }

    /// Get frame data slice - zero-copy
    pub fn frames(&self) -> &[FrameData] {
        &self.frames
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Get Arc strong reference count (for testing zero-copy)
    #[cfg(test)]
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.frames)
    }
}

/// Optimized GOP manager
///
/// Core improvements:
/// - frozen: Completed GOPs, using FrozenGop for zero-copy sharing
/// - current: Currently writing GOP
pub struct Gops {
    frozen: VecDeque<FrozenGop>,
    current: Vec<FrameData>,
    max_gops: usize,
}

impl Default for Gops {
    fn default() -> Self {
        Self::new(1)
    }
}

impl Clone for Gops {
    fn clone(&self) -> Self {
        Self {
            frozen: self.frozen.clone(), // FrozenGop clone is O(1)
            current: self.current.clone(),
            max_gops: self.max_gops,
        }
    }
}

impl Gops {
    pub fn new(max_gops: usize) -> Self {
        Self {
            frozen: VecDeque::with_capacity(max_gops),
            current: Vec::with_capacity(256),
            max_gops,
        }
    }

    /// Save frame data
    ///
    /// # Arguments
    /// * `data` - Frame data
    /// * `is_key_frame` - Whether it's a keyframe (determined by caller from video data)
    ///
    /// ⚠️ Keyframe detection must be correctly implemented by caller, wrong detection breaks GOP boundaries
    pub fn save_frame_data(&mut self, data: FrameData, is_key_frame: bool) {
        if self.max_gops == 0 {
            return;
        }

        // When keyframe encountered, freeze current GOP
        if is_key_frame && !self.current.is_empty() {
            // Take current frames and create frozen GOP
            let frames = std::mem::take(&mut self.current);
            let frozen = FrozenGop::new(frames);

            // If limit exceeded, remove oldest GOP
            if self.frozen.len() >= self.max_gops {
                self.frozen.pop_front();
            }
            self.frozen.push_back(frozen);

            // Re-preallocate capacity
            self.current.reserve(256);
        }

        self.current.push(data);
    }

    /// Get reference iterator for all frozen GOPs (test only)
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn frozen_gops(&self) -> impl Iterator<Item = &FrozenGop> {
        self.frozen.iter()
    }

    /// Get clone iterator for frozen GOPs
    ///
    /// Each FrozenGop clone is O(1), only increments Arc reference count
    pub fn get_frozen_gops(&self) -> impl Iterator<Item = FrozenGop> + '_ {
        self.frozen.iter().cloned()
    }

    /// Get frames of currently writing GOP (test only)
    #[cfg(test)]
    pub fn get_current_frames(&self) -> &[FrameData] {
        &self.current
    }

    /// Whether GOP caching is enabled
    pub fn is_enabled(&self) -> bool {
        self.max_gops > 0
    }

    /// Frozen GOP count (test only)
    #[cfg(test)]
    pub fn frozen_count(&self) -> usize {
        self.frozen.len()
    }

    /// Frozen frames count (test only)
    #[cfg(test)]
    fn frozen_frame_count(&self) -> usize {
        self.frozen.iter().map(|g| g.len()).sum()
    }

    /// Current GOP frame count (test only)
    #[cfg(test)]
    fn current_frame_count(&self) -> usize {
        self.current.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_video_frame(ts: u32, data: &[u8]) -> FrameData {
        FrameData::Video {
            timestamp: RtmpTimestamp { value: ts },
            data: Bytes::copy_from_slice(data),
        }
    }

    fn make_audio_frame(ts: u32, data: &[u8]) -> FrameData {
        FrameData::Audio {
            timestamp: RtmpTimestamp { value: ts },
            data: Bytes::copy_from_slice(data),
        }
    }

    #[test]
    fn test_frozen_gop_zero_copy() {
        let mut gops = Gops::new(2);

        // Add first keyframe and some frames
        gops.save_frame_data(make_video_frame(0, b"keyframe1"), true);
        gops.save_frame_data(make_video_frame(33, b"frame2"), false);
        gops.save_frame_data(make_audio_frame(40, b"audio1"), false);

        // New keyframe triggers freeze
        gops.save_frame_data(make_video_frame(66, b"keyframe2"), true);

        // Should have one frozen GOP
        let frozen: Vec<_> = gops.get_frozen_gops().collect();
        assert_eq!(frozen.len(), 1);
        assert_eq!(frozen[0].len(), 3); // keyframe1, frame2, audio1

        // Verify zero-copy
        let gop1 = frozen[0].clone();
        let gop2 = gop1.clone();

        // Arc reference count should increase
        assert!(gop1.strong_count() >= 2);
        assert_eq!(gop1.strong_count(), gop2.strong_count());
    }

    #[test]
    fn test_gop_boundary_correctness() {
        let mut gops = Gops::new(3);

        // GOP 1
        gops.save_frame_data(make_video_frame(0, b"k1"), true);
        gops.save_frame_data(make_video_frame(33, b"p1"), false);

        // GOP 2
        gops.save_frame_data(make_video_frame(66, b"k2"), true);
        gops.save_frame_data(make_video_frame(100, b"p2"), false);

        // GOP 3
        gops.save_frame_data(make_video_frame(133, b"k3"), true);

        // Should have 2 frozen GOPs
        assert_eq!(gops.frozen_count(), 2);
        assert_eq!(gops.frozen_frame_count(), 4); // (k1+p1) + (k2+p2)
        assert_eq!(gops.current_frame_count(), 1); // k3
    }

    #[test]
    fn test_max_gops_limit() {
        let mut gops = Gops::new(2);

        // Create 3 GOPs, should only keep 2
        gops.save_frame_data(make_video_frame(0, b"k1"), true);
        gops.save_frame_data(make_video_frame(33, b"k2"), true);
        gops.save_frame_data(make_video_frame(66, b"k3"), true);
        gops.save_frame_data(make_video_frame(100, b"k4"), true);

        // Oldest GOP should be removed
        assert_eq!(gops.frozen_count(), 2);
    }

    #[test]
    fn test_repeated_keyframes() {
        let mut gops = Gops::new(3);

        // Consecutive keyframes
        gops.save_frame_data(make_video_frame(0, b"k1"), true);
        gops.save_frame_data(make_video_frame(33, b"k2"), true); // Triggers freeze
        gops.save_frame_data(make_video_frame(66, b"k3"), true); // Triggers freeze

        // Should have 2 frozen GOPs, each with only 1 frame
        assert_eq!(gops.frozen_count(), 2);

        let frozen: Vec<_> = gops.get_frozen_gops().collect();
        assert_eq!(frozen[0].len(), 1);
        assert_eq!(frozen[1].len(), 1);
    }

    #[test]
    fn test_disabled_gop_cache() {
        let mut gops = Gops::new(0);

        gops.save_frame_data(make_video_frame(0, b"k1"), true);
        gops.save_frame_data(make_video_frame(33, b"k2"), true);

        // Should not save any frames when disabled
        assert_eq!(gops.frozen_count(), 0);
        assert!(!gops.is_enabled());
    }

    #[test]
    fn test_empty_current_gop_on_first_keyframe() {
        let mut gops = Gops::new(2);

        // First keyframe, current is empty, should not create empty frozen GOP
        gops.save_frame_data(make_video_frame(0, b"k1"), true);

        assert_eq!(gops.frozen_count(), 0);
        assert_eq!(gops.get_current_frames().len(), 1);
    }
}
