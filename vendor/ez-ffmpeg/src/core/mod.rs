//! The **core** module provides the foundational building blocks for configuring and running FFmpeg
//! pipelines. It encompasses:
//!
//! - **Input & Output Handling** (in [`context`]): Structures and logic (`Input`, `Output`) for
//!   specifying where media data originates and where it should be written.
//! - **Filter Descriptions**: Define filter graphs with `FilterComplex` or attach custom [`FrameFilter`](filter::frame_filter::FrameFilter)
//!   implementations at the input/output stage.
//! - **Stream and Device Queries** (in [`stream_info`] and [`device`]): Utilities for retrieving
//!   information about media streams and available input devices.
//! - **Hardware Acceleration** (in [`hwaccel`]): Enumerate/configure GPU-accelerated codecs (CUDA, VAAPI, etc.).
//! - **Codec Discovery** (in [`codec`]): List encoders/decoders supported by FFmpeg.
//! - **Custom Filters** (in [`filter`]): Implement user-defined [`FrameFilter`](filter::frame_filter::FrameFilter) logic for frames.
//! - **Lifecycle Orchestration** (in [`scheduler`]): [`FfmpegScheduler`](scheduler::ffmpeg_scheduler::FfmpegScheduler) that runs the configured pipeline
//!   (synchronously or asynchronously if the `async` feature is enabled).
//!
//! # Submodules
//!
//! - [`context`]: Houses [`FfmpegContext`](context::ffmpeg_context::FfmpegContext)—the central struct for assembling inputs, outputs, and filters.
//! - [`scheduler`]: Defines [`FfmpegScheduler`](scheduler::ffmpeg_scheduler::FfmpegScheduler), managing the execution of an `FfmpegContext` pipeline.
//! - [`container_info`]: Utilities to extract information about the container, such as duration and format details.
//! - [`stream_info`]: Inspect media streams (e.g., find video/audio streams in a file).
//! - [`device`]: Query audio/video input devices (cameras, microphones, etc.) on various platforms.
//! - [`hwaccel`]: Helpers for hardware-accelerated encoding/decoding setup.
//! - [`codec`]: Tools to discover which encoders/decoders your FFmpeg build supports.
//! - [`filter`]: Query FFmpeg's built-in filters and infrastructure for building custom frame-processing filters.

pub mod context;
pub mod scheduler;
pub mod container_info;
pub mod stream_info;
pub mod device;
pub mod hwaccel;
pub mod codec;
pub mod filter;
pub(crate) mod metadata;

static INIT_FFMPEG: std::sync::Once = std::sync::Once::new();

extern "C" fn cleanup() {
    let _ = std::panic::catch_unwind(|| {
        unsafe {
            hwaccel::hw_device_free_all();
            ffmpeg_sys_next::avformat_network_deinit();
        }

        log::debug!("FFmpeg cleaned up");
    });
}

// -----------------------------------------------------------------------------
// va_list ABI glue
// -----------------------------------------------------------------------------
//
// Your bindgen output on aarch64-linux is emitting signatures like:
//
//   av_log_format_line(..., args: [u64; 4], ...)
//   av_log_set_callback(Some(fn(..., args: [u64; 4])))
//
// So we must match that EXACTLY on that target.
//
// On other targets we keep the old mappings.

#[cfg(all(
    target_arch = "aarch64",
    not(target_vendor = "apple"),
    not(target_os = "uefi"),
    not(windows),
))]
type VaListType = [u64; 4];

#[cfg(any(
    all(
        not(target_arch = "aarch64"),
        not(target_arch = "powerpc"),
        not(target_arch = "s390x"),
        not(target_arch = "x86_64")
    ),
    all(target_arch = "aarch64", target_vendor = "apple"),
    target_family = "wasm",
    target_os = "uefi",
    windows,
))]
type VaListType = *mut libc::c_char;

#[cfg(all(target_arch = "x86_64", not(target_os = "uefi"), not(windows)))]
type VaListType = *mut ffmpeg_sys_next::__va_list_tag;

#[cfg(all(target_arch = "powerpc", not(target_os = "uefi"), not(windows)))]
type VaListType = *mut ffmpeg_sys_next::__va_list_tag_powerpc;

#[cfg(target_arch = "s390x")]
type VaListType = *mut ffmpeg_sys_next::__va_list_tag_s390x;

// -----------------------------------------------------------------------------
// FFmpeg log callback
// -----------------------------------------------------------------------------
//
// Also important: your bindgen output uses `u8` for C `char` (fmt pointer and buffers),
// so we MUST use `*const u8` for `fmt` and pass `*mut u8` buffers.

unsafe extern "C" fn ffmpeg_log_callback(
    ptr: *mut libc::c_void,
    level: libc::c_int,
    fmt: *const u8,
    args: VaListType,
) {
    let mut buffer = [0u8; 1024];
    let mut print_prefix: libc::c_int = 1;

    // Signature in your bindings:
    //   av_log_format_line(ptr, level, fmt, args, line, line_size, print_prefix)
    ffmpeg_sys_next::av_log_format_line(
        ptr,
        level,
        fmt,
        args,
        buffer.as_mut_ptr(),
        buffer.len() as libc::c_int,
        &mut print_prefix,
    );

    // Convert to &str (buffer is NUL-terminated C string).
    if let Ok(msg) = std::ffi::CStr::from_ptr(buffer.as_ptr() as *const libc::c_char).to_str() {
        let trimmed = msg.trim_end_matches(|c| c == '\n' || c == '\r');

        if level <= ffmpeg_sys_next::AV_LOG_ERROR {
            log::error!("FFmpeg: {}", trimmed);
        } else if level <= ffmpeg_sys_next::AV_LOG_WARNING {
            log::warn!("FFmpeg: {}", trimmed);
        } else if level <= ffmpeg_sys_next::AV_LOG_INFO {
            log::info!("FFmpeg: {}", trimmed);
        } else {
            log::debug!("FFmpeg: {}", trimmed);
        }
    }
}

pub(crate) fn initialize_ffmpeg() {
    INIT_FFMPEG.call_once(|| {
        unsafe {
            libc::atexit(cleanup as extern "C" fn());
            ffmpeg_sys_next::avdevice_register_all();
            ffmpeg_sys_next::avformat_network_init();
            ffmpeg_sys_next::av_log_set_callback(Some(ffmpeg_log_callback));
        }

        log::info!("FFmpeg initialized.");
    });
}