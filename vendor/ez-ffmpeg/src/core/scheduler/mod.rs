use ffmpeg_sys_next::AVMediaType;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_ATTACHMENT, AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_DATA, AVMEDIA_TYPE_SUBTITLE, AVMEDIA_TYPE_VIDEO};

/// The **ffmpeg_scheduler** module provides the orchestrator that actually runs the
/// configured FFmpeg job. It handles the lifecycle of the FFmpeg process, manages
/// threading (or subprocess execution) as appropriate, and returns the final results.
/// You can optionally run this job synchronously or, if the `async` feature is enabled,
/// as an async task that you `.await`.
///
/// # Synchronous Example
///
/// ```rust
/// // Assume we've already built an FfmpegContext
/// let context = FfmpegContext::builder()
///     .input("test.mp4")
///     .filter_desc("hue=s=0")
///     .output("output.mp4")
///     .build()
///     .unwrap();
///
/// // Create a scheduler, start the job, then block until it's finished
/// let result = FfmpegScheduler::new(context)
///     .start()
///     .unwrap()
///     .wait();
///
/// assert!(result.is_ok(), "FFmpeg job failed unexpectedly");
/// ```
///
/// # Asynchronous Example (requires `async` feature)
///
/// ```rust,ignore
/// // Note: you need to enable the "async" feature in Cargo.toml:
/// // [dependencies.ez_ffmpeg]
/// // features = ["async"]
///
/// #[tokio::main]
/// async fn main() {
///     let context = FfmpegContext::builder()
///         .input("test.mp4")
///         .filter_desc("hue=s=0")
///         .output("output.mp4")
///         .build()
///         .unwrap();
///
///     let mut scheduler = FfmpegScheduler::new(context)
///         .start()
///         .expect("Failed to start FFmpeg job");
///
///     // Asynchronous wait
///     scheduler.await.expect("FFmpeg job failed unexpectedly");
/// }
/// ```
pub mod ffmpeg_scheduler;
mod frame_filter_pipeline;
mod mux_task;
mod enc_task;
pub(crate) mod filter_task;
mod dec_task;
mod demux_task;
pub(crate) mod input_controller;

pub(crate) fn type_to_symbol(media_type: AVMediaType) -> String {
    match media_type {
        AVMediaType::AVMEDIA_TYPE_UNKNOWN => "unknown".to_string(),
        AVMEDIA_TYPE_VIDEO => "v".to_string(),
        AVMEDIA_TYPE_AUDIO => "a".to_string(),
        AVMEDIA_TYPE_DATA => "d".to_string(),
        AVMEDIA_TYPE_SUBTITLE => "s".to_string(),
        AVMEDIA_TYPE_ATTACHMENT => "t".to_string(),
        AVMediaType::AVMEDIA_TYPE_NB => "nb".to_string(),
    }
}