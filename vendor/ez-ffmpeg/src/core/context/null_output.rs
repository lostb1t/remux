use std::sync::atomic::{AtomicU64, Ordering};
use crate::Output;
use ffmpeg_sys_next;

/// Represents the state for the null output, tracking current and maximum positions.
///
/// This struct uses atomic types for lock-free, high-performance position management,
/// making it suitable for concurrent access by FFmpeg threads.
struct NullState {
    current_position: AtomicU64,
    max_position: AtomicU64,
}

/// Creates a high-performance, thread-safe null output for FFmpeg that discards all data while supporting seeking.
pub fn create_null_output() -> Output {
    let null_state = std::sync::Arc::new(NullState {
        current_position: AtomicU64::new(0),
        max_position: AtomicU64::new(0),
    });

    // Write callback: discards data and updates position atomically
    let write_callback: Box<dyn FnMut(&[u8]) -> i32> = {
        let state = std::sync::Arc::clone(&null_state);
        Box::new(move |buf: &[u8]| -> i32 {
            let len = buf.len() as u64;
            let new_pos = state.current_position.fetch_add(len, Ordering::Relaxed) + len;
            state.max_position.fetch_max(new_pos, Ordering::Relaxed);
            len as i32
        })
    };

    // Seek callback: handles positioning and size queries atomically
    let seek_callback: Box<dyn FnMut(i64, i32) -> i64> = {
        let state = std::sync::Arc::clone(&null_state);
        Box::new(move |offset: i64, whence: i32| -> i64 {
            match whence {
                ffmpeg_sys_next::AVSEEK_SIZE => state.max_position.load(Ordering::Relaxed) as i64,
                ffmpeg_sys_next::SEEK_SET => {
                    if offset < 0 {
                        return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EINVAL) as i64;
                    }
                    let new_pos = offset as u64;
                    state.current_position.store(new_pos, Ordering::Relaxed);
                    state.max_position.fetch_max(new_pos, Ordering::Relaxed);
                    new_pos as i64
                }
                ffmpeg_sys_next::SEEK_CUR => {
                    let current = state.current_position.load(Ordering::Relaxed);
                    let new_pos = if offset >= 0 {
                        current.saturating_add(offset as u64)
                    } else {
                        current.saturating_sub((-offset) as u64)
                    };
                    state.current_position.store(new_pos, Ordering::Relaxed);
                    state.max_position.fetch_max(new_pos, Ordering::Relaxed);
                    new_pos as i64
                }
                ffmpeg_sys_next::SEEK_END => {
                    let max = state.max_position.load(Ordering::Relaxed);
                    let new_pos = if offset >= 0 {
                        max.saturating_add(offset as u64)
                    } else {
                        max.saturating_sub((-offset) as u64)
                    };
                    state.current_position.store(new_pos, Ordering::Relaxed);
                    state.max_position.fetch_max(new_pos, Ordering::Relaxed);
                    new_pos as i64
                }
                _ => ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EINVAL) as i64,
            }
        })
    };

    // Create and configure the output
    let mut output: Output = write_callback.into();
    output = output.set_seek_callback(seek_callback)
        .set_format("mp4"); // Default format, adjustable as needed
    output
}