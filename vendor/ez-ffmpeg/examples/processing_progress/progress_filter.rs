use std::sync::Arc;

use ez_ffmpeg::filter::frame_filter::FrameFilter;
use ez_ffmpeg::filter::frame_filter_context::FrameFilterContext;
use ez_ffmpeg::AVMediaType;
use ez_ffmpeg::Frame;

pub struct ProgressCallBacker {
    pub total_duration: i64,
    pub time_base: ez_ffmpeg::AVRational,
}

impl ProgressCallBacker {
    pub fn new() -> Self {
        Self {
            total_duration: 0,
            time_base: ez_ffmpeg::AVRational { num: 0, den: 0 },
        }
    }

    pub fn print_progress(&self, frame: &Frame) {
        if let Some(pts) = frame.pts() {
            // Check if the time base is valid.
            if self.time_base.den == 0 {
                println!(
                    "Warning: The time base denominator is 0, and the time cannot be calculated."
                );
                return;
            }

            // Get the timestamp of the frame (in seconds).
            let time_in_stream = pts as f64 * self.time_base.num as f64 / self.time_base.den as f64;

            // Total duration (in seconds)
            let total_duration_sec = self.total_duration as f64 / 1_000_000.0;

            // Calculate the progress percentage
            let progress = (time_in_stream / total_duration_sec) * 100.0;
            let clamped_progress = progress.min(100.0).max(0.0);

            println!(
                "Progress: {:.2}% (Current: {:.3}s / Total: {:.2}s, PTS: {})",
                clamped_progress, time_in_stream, total_duration_sec, pts
            );
        }
    }
}

pub struct ProgressCallBackFilter {
    progress_callback: Arc<ProgressCallBacker>,
}

impl ProgressCallBackFilter {
    pub fn new(progress_callback: Arc<ProgressCallBacker>) -> Self {
        Self { progress_callback }
    }
}

impl FrameFilter for ProgressCallBackFilter {
    fn media_type(&self) -> AVMediaType {
        AVMediaType::AVMEDIA_TYPE_AUDIO // Process audio frames
    }

    fn filter_frame(
        &mut self,
        frame: Frame,
        _ctx: &FrameFilterContext,
    ) -> Result<Option<Frame>, String> {
        unsafe {
            // Ensure the frame is valid and not empty
            if frame.as_ptr().is_null() || frame.is_empty() {
                return Ok(Some(frame)); // If invalid, simply return the frame as-is
            }
        }
        self.progress_callback.print_progress(&frame);
        Ok(Some(frame))
    }
}
