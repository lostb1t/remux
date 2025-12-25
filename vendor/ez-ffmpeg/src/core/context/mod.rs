use crate::error::AllocFrameError;
use ffmpeg_sys_next::AVMediaType::{
    AVMEDIA_TYPE_ATTACHMENT, AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_DATA, AVMEDIA_TYPE_SUBTITLE,
    AVMEDIA_TYPE_VIDEO,
};
use ffmpeg_sys_next::{
    av_freep, avcodec_free_context, avformat_close_input, avformat_free_context, avio_closep,
    avio_context_free, AVCodecContext, AVCodecParameters, AVFormatContext, AVIOContext,
    AVMediaType, AVRational, AVStream, AVFMT_NOFILE,
};
use std::ffi::c_void;
use std::ptr::null_mut;


/// The **ffmpeg_context** module is responsible for assembling FFmpeg’s configuration:
/// inputs, outputs, codecs, filters, and other parameters needed to construct a
/// complete media processing pipeline.
///
/// # Example
/// ```rust
///
/// // Build an FFmpeg context with one input, some filter settings, and one output
/// let context = FfmpegContext::builder()
///     .input("test.mp4")
///     .filter_desc("hue=s=0")
///     .output("output.mp4")
///     .build()
///     .unwrap();
/// // The context now holds all info needed for an FFmpeg job.
/// ```
pub mod ffmpeg_context;

/// The **ffmpeg_context_builder** module defines the builder pattern for creating
/// [`FfmpegContext`](ffmpeg_context::FfmpegContext) objects.
///
/// It exposes the [`FfmpegContextBuilder`](ffmpeg_context_builder::FfmpegContextBuilder) struct, which allows you to:
/// - Configure multiple [`Input`](input::Input) and
///   [`Output`](output::Output) streams.
/// - Attach filter descriptions via [`FilterComplex`](crate::core::context::filter_complex::FilterComplex)
///   or inline strings (e.g., `"scale=1280:720"`, `"hue=s=0"`).
/// - Produce a finished `FfmpegContext` that can then be executed by
///   [`FfmpegScheduler`](crate::core::scheduler::ffmpeg_scheduler::FfmpegScheduler).
///
/// # Examples
///
/// ```rust
/// // 1. Create a builder (usually via FfmpegContext::builder())
/// let builder = FfmpegContext::builder();
///
/// // 2. Add inputs, outputs, and filters
/// let ffmpeg_context = builder
///     .input("input.mp4")
///     .filter_desc("hue=s=0")
///     .output("output.mp4")
///     .build()
///     .expect("Failed to build FfmpegContext");
///
/// // 3. Use `ffmpeg_context` with FfmpegScheduler (e.g., `.start()` and `.wait()`).
/// ```
pub mod ffmpeg_context_builder;

/// The **input** module defines the [`Input`](crate::core::context::input::Input) struct,
/// representing an FFmpeg input source. An input can be:
/// - A file path or URL (e.g., `"video.mp4"`, `rtmp://example.com/live/stream`).
/// - A **custom data source** via a `read_callback` (and optionally `seek_callback`) for
///   advanced scenarios like in-memory buffers or network protocols.
///
/// You can also specify **frame pipelines** to apply custom [`FrameFilter`](crate::core::filter::frame_filter::FrameFilter)
/// transformations **after decoding** but **before** the frames move on to the rest of the pipeline.
///
/// # Example
///
/// ```rust
/// use ez_ffmpeg::core::context::input::Input;
///
/// // Basic file or network URL:
/// let file_input: Input = "example.mp4".into();
///
/// // Or a custom read callback:
/// let custom_input = Input::new_by_read_callback(|buf| {
///     // Fill `buf` with data from your source
///     // Return the number of bytes read, or negative for errors
///     0
/// });
/// ```
pub mod input;

/// The **output** module defines the [`Output`](crate::core::context::output::Output) struct,
/// representing an FFmpeg output destination. An output may be:
/// - A file path or URL (e.g., `"output.mp4"`, `rtmp://...`).
/// - A **custom write callback** that processes encoded data (e.g., storing it
///   in-memory or sending it over a custom network protocol).
///
/// You can specify additional details such as:
/// - **Container format** (e.g., `"mp4"`, `"flv"`, `"mkv"`).
/// - **Video/Audio/Subtitle codecs** (e.g., `"h264"`, `"aac"`, `"mov_text"`).
/// - **Frame pipelines** to apply [`FrameFilter`](crate::core::filter::frame_filter::FrameFilter)
///   transformations **before encoding**.
///
/// # Example
///
/// ```rust
/// use ez_ffmpeg::core::context::output::Output;
///
/// // Basic file/URL output:
/// let file_output: Output = "output.mp4".into();
///
/// // Or a custom write callback:
/// let custom_output = Output::new_by_write_callback(|encoded_data| {
///     // Write `encoded_data` somewhere
///     encoded_data.len() as i32
/// }).set_format("mp4");
/// ```
pub mod output;

/// The **filter_complex** module defines the [`FilterComplex`](crate::core::context::filter_complex::FilterComplex)
/// struct, which encapsulates one or more FFmpeg filter descriptions (e.g., `"scale=1280:720"`,
/// `"hue=s=0"`, etc.). You can use `FilterComplex` to construct more advanced or multi-step
/// filter graphs than simple inline strings allow.
///
/// `FilterComplex` can also associate a particular hardware device (e.g., for GPU-based
/// filtering) via `hw_device`.
///
/// # Example
///
/// ```rust
/// use ez_ffmpeg::core::context::filter_complex::FilterComplex;
///
/// // Build a FilterComplex from a string:
/// let my_filters = FilterComplex::from("scale=1280:720");
///
/// // Optionally specify a hardware device (e.g., "cuda"):
/// // my_filters.set_hw_device("cuda");
/// ```
pub mod filter_complex;


pub(super) mod decoder_stream;
pub(super) mod demuxer;
pub(super) mod encoder_stream;
pub(super) mod filter_graph;
pub(super) mod input_filter;
pub(super) mod muxer;
pub(super) mod obj_pool;
pub(super) mod output_filter;

/// The **null_output** module provides a custom null output implementation for FFmpeg
/// that discards all data while supporting seeking.
///
/// It exposes the [`create_null_output`](null_output::create_null_output) function, which returns an
/// [`Output`](ez_ffmpeg::Output) object configured to:
/// - Discard all written data, behaving like `/dev/null`.
/// - Maintain a seekable position state using atomic operations for thread-safe, high-performance access.
/// - Support scenarios such as testing or processing streaming inputs (e.g., RTMP) where no output file is needed.
///
/// # Usage Scenario
/// This module is useful when processing FFmpeg input streams without generating an output file, such as
/// when handling RTMP streams that require a seekable output format like MP4, even if the output is discarded.
///
/// # Examples
///
/// ```rust
/// use ez_ffmpeg::Output;
/// let output: Output = create_null_output();
/// // Pass `output` to an FFmpeg context for processing
/// ```
///
/// # Performance
/// - Utilizes `AtomicU64` with `Relaxed` ordering for lock-free position tracking, ensuring efficient concurrent access.
/// - Write and seek operations are optimized to minimize overhead by avoiding locks.
///
/// # Notes
/// - The default output format is "mp4", but this can be modified using `set_format` as needed.
/// - Write operations assume individual buffers do not exceed `i32::MAX` bytes, which aligns with typical FFmpeg usage.
pub mod null_output;

pub(crate) struct CodecContext {
    inner: *mut AVCodecContext,
}

unsafe impl Send for CodecContext {}
unsafe impl Sync for CodecContext {}

impl CodecContext {
    pub(crate) fn new(avcodec_context: *mut AVCodecContext) -> Self {
        Self {
            inner: avcodec_context,
        }
    }

    pub(crate) fn replace(&mut self, avcodec_context: *mut AVCodecContext) -> *mut AVCodecContext {
        let mut tmp = self.inner;
        if !tmp.is_null() {
            unsafe {
                avcodec_free_context(&mut tmp);
            }
        }
        self.inner = avcodec_context;
        tmp
    }

    pub(crate) fn null() -> Self {
        Self { inner: null_mut() }
    }

    pub(crate) fn as_mut_ptr(&self) -> *mut AVCodecContext {
        self.inner
    }

    pub(crate) fn as_ptr(&self) -> *const AVCodecContext {
        self.inner as *const AVCodecContext
    }
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        unsafe {
            avcodec_free_context(&mut self.inner);
        }
    }
}

#[derive(Copy, Clone)]
pub(crate) struct Stream {
    pub(crate) inner: *mut AVStream,
}

unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}

pub(crate) struct FrameBox {
    pub(crate) frame: ffmpeg_next::Frame,
    // stream copy or filtergraph
    pub(crate) frame_data: FrameData,
}

unsafe impl Send for FrameBox {}
unsafe impl Sync for FrameBox {}

pub fn frame_alloc() -> crate::error::Result<ffmpeg_next::Frame> {
    unsafe {
        let frame = ffmpeg_next::Frame::empty();
        if frame.as_ptr().is_null() {
            return Err(AllocFrameError::OutOfMemory.into());
        }
        Ok(frame)
    }
}

pub fn null_frame() -> ffmpeg_next::Frame {
    unsafe { ffmpeg_next::Frame::wrap(null_mut()) }
}

#[derive(Clone)]
pub(crate) struct FrameData {
    pub(crate) framerate: Option<AVRational>,
    pub(crate) bits_per_raw_sample: i32,
    pub(crate) input_stream_width: i32,
    pub(crate) input_stream_height: i32,
    pub(crate) subtitle_header_size: i32,
    pub(crate) subtitle_header: *mut u8,

    pub(crate) fg_input_index: usize,
}

unsafe impl Send for FrameData {}
unsafe impl Sync for FrameData {}

pub(crate) struct PacketBox {
    pub(crate) packet: ffmpeg_next::Packet,
    pub(crate) packet_data: PacketData,
}

unsafe impl Send for PacketBox {}
unsafe impl Sync for PacketBox {}

// optionally attached as opaque_ref to decoded AVFrames
#[derive(Clone)]
pub(crate) struct PacketData {
    // demuxer-estimated dts in AV_TIME_BASE_Q,
    // to be used when real dts is missing
    pub(crate) dts_est: i64,
    pub(crate) codec_type: AVMediaType,
    pub(crate) output_stream_index: i32,
    pub(crate) is_copy: bool,
    pub(crate) codecpar: *mut AVCodecParameters,
}

unsafe impl Send for PacketData {}
unsafe impl Sync for PacketData {}

pub(crate) struct AVFormatContextBox {
    pub(crate) fmt_ctx: *mut AVFormatContext,
    pub(crate) is_input: bool,
    pub(crate) is_set_callback: bool,
}
unsafe impl Send for AVFormatContextBox {}
unsafe impl Sync for AVFormatContextBox {}

impl AVFormatContextBox {
    pub(crate) fn new(
        fmt_ctx: *mut AVFormatContext,
        is_input: bool,
        is_set_callback: bool,
    ) -> Self {
        Self {
            fmt_ctx,
            is_input,
            is_set_callback,
        }
    }
}

impl Drop for AVFormatContextBox {
    fn drop(&mut self) {
        if self.fmt_ctx.is_null() {
            return;
        }
        if self.is_input {
            in_fmt_ctx_free(self.fmt_ctx, self.is_set_callback)
        } else {
            out_fmt_ctx_free(self.fmt_ctx, self.is_set_callback)
        }
    }
}

pub(crate) fn out_fmt_ctx_free(out_fmt_ctx: *mut AVFormatContext, is_set_write_callback: bool) {
    if out_fmt_ctx.is_null() {
        return;
    }
    unsafe {
        if is_set_write_callback {
            free_output_opaque((*out_fmt_ctx).pb);
        } else if (*out_fmt_ctx).flags & AVFMT_NOFILE == 0 {
            let mut pb = (*out_fmt_ctx).pb;
            if !pb.is_null() {
                avio_closep(&mut pb);
            }
        }
        avformat_free_context(out_fmt_ctx);
    }
}

unsafe fn free_output_opaque(mut avio_ctx: *mut AVIOContext) {
    if avio_ctx.is_null() {
        return;
    }
    if !(*avio_ctx).buffer.is_null() {
        av_freep(&mut (*avio_ctx).buffer as *mut _ as *mut c_void);
    }
    let opaque_ptr = (*avio_ctx).opaque as *mut Box<dyn FnMut(&[u8]) -> i32>;
    if !opaque_ptr.is_null() {
        let _ = Box::from_raw(opaque_ptr);
    }
    avio_context_free(&mut avio_ctx);
}

pub(crate) fn in_fmt_ctx_free(mut in_fmt_ctx: *mut AVFormatContext, is_set_read_callback: bool) {
    if in_fmt_ctx.is_null() {
        return;
    }
    if is_set_read_callback {
        unsafe {
            free_input_opaque((*in_fmt_ctx).pb);
        }
    }
    unsafe {
        avformat_close_input(&mut in_fmt_ctx);
    }
}

unsafe fn free_input_opaque(mut avio_ctx: *mut AVIOContext) {
    if !avio_ctx.is_null() {
        let opaque_ptr = (*avio_ctx).opaque as *mut Box<dyn FnMut(&mut [u8]) -> i32>;
        if !opaque_ptr.is_null() {
            let _ = Box::from_raw(opaque_ptr);
        }
        av_freep(&mut (*avio_ctx).buffer as *mut _ as *mut c_void);
        avio_context_free(&mut avio_ctx);
    }
}

#[allow(dead_code)]
pub(crate) fn type_to_linklabel(media_type: AVMediaType, index: usize) -> Option<String> {
    match media_type {
        AVMediaType::AVMEDIA_TYPE_UNKNOWN => None,
        AVMEDIA_TYPE_VIDEO => Some(format!("{index}:v")),
        AVMEDIA_TYPE_AUDIO => Some(format!("{index}:a")),
        AVMEDIA_TYPE_DATA => Some(format!("{index}:d")),
        AVMEDIA_TYPE_SUBTITLE => Some(format!("{index}:s")),
        AVMEDIA_TYPE_ATTACHMENT => Some(format!("{index}:t")),
        AVMediaType::AVMEDIA_TYPE_NB => None,
    }
}
