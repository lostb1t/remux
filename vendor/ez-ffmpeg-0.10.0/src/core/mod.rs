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
//!
//! # Example Workflow
//!
//! 1. **Build a context** using [`FfmpegContext::builder()`](crate::core::context::ffmpeg_context::FfmpegContext::builder)
//!    specifying your input, any filters, and your output.
//! 2. **Create a scheduler** with [`FfmpegScheduler::new`](crate::core::scheduler::ffmpeg_scheduler::FfmpegScheduler::new),
//!    then call `.start()` to begin processing.
//! 3. **Wait** (or `.await` if `async` feature is enabled) for the job to complete. Use the returned
//!    `Result` to detect success or failure.
//!
//! # Example
//! ```rust,ignore
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 1. Build an FfmpegContext with an input, a simple filter, and an output
//!     let context = FfmpegContext::builder()
//!         .input("test.mp4")
//!         .filter_desc("hue=s=0") // Example: desaturate video
//!         .output("output.mp4")
//!         .build()?;
//!
//!     // 2. Create a scheduler and start the job
//!     let scheduler = FfmpegScheduler::new(context).start()?;
//!
//!     // 3. Block until it's finished
//!     scheduler.wait()?;
//!     Ok(())
//! }
//! ```

/// The **context** module provides tools for assembling an entire FFmpeg pipeline,
/// culminating in the [`FfmpegContext`](context::ffmpeg_context::FfmpegContext). This includes:
///
/// - **Inputs**: [`Input`](context::input::Input) objects representing files, URLs, or custom I/O callbacks.
/// - **Outputs**: [`Output`](context::output::Output) objects representing target files, streams, or custom sinks.
/// - **Filter Descriptions**: Simple inline filters via `filter_desc` or more complex
///   [`FilterComplex`](context::filter_complex::FilterComplex) graphs.
/// - **Builders**: e.g., [`FfmpegContextBuilder`](context::ffmpeg_context_builder::FfmpegContextBuilder) for constructing a complete context
///   with multiple inputs, outputs, and filter settings.
///
/// Once you’ve built an [`FfmpegContext`](context::ffmpeg_context::FfmpegContext), you can execute it via the [`FfmpegScheduler`](scheduler::ffmpeg_scheduler::FfmpegScheduler).
///
/// # Example
///
/// ```rust,ignore
/// // Build an FFmpeg context with one input, some filter settings, and one output.
/// let context = FfmpegContext::builder()
///     .input("test.mp4")
///     .filter_desc("hue=s=0")
///     .output("output.mp4")
///     .build()
///     .unwrap();
/// // The context now holds all info needed for an FFmpeg job.
/// ```
pub mod context;

/// The **scheduler** module orchestrates the execution of a configured [`FfmpegContext`](context::ffmpeg_context::FfmpegContext).
/// It provides the [`FfmpegScheduler`](scheduler::ffmpeg_scheduler::FfmpegScheduler) struct, which:
///
/// - **Starts** the FFmpeg pipeline via [`FfmpegScheduler::start()`](scheduler::ffmpeg_scheduler::FfmpegScheduler<crate::core::scheduler::ffmpeg_scheduler::Initialization>::start()).
/// - **Manages** thread or subprocess creation, ensuring all streams and filters run.
/// - **Waits** for completion (blocking or asynchronous, depending on whether the `async` feature is enabled).
/// - **Returns** the final result, indicating success or failure.
///
/// # Synchronous Example
///
/// ```rust,ignore
/// let context = FfmpegContext::builder()
///     .input("test.mp4")
///     .filter_desc("hue=s=0")
///     .output("output.mp4")
///     .build()
///     .unwrap();
///
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
/// #[tokio::main]
/// async fn main() {
///     let context = FfmpegContext::builder()
///         .input("test.mp4")
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
pub mod scheduler;

/// The **container_info** module provides utilities for retrieving metadata related to the media container,
/// such as duration, format, and other general properties of the media file.
///
/// This module helps to query the overall properties of a media container file (e.g., `.mp4`, `.avi`, `.mkv`)
/// without diving into individual streams (audio, video, etc.). It is useful when you need information
/// about the file as a whole, such as total duration, format type, and container-specific properties.
///
/// # Examples
///
/// ```rust,ignore
/// // Retrieve the duration in microseconds for the media file "test.mp4"
/// let duration = get_duration_us("test.mp4").unwrap();
/// println!("Duration: {} us", duration);
///
/// // Retrieve the format name for "test.mp4"
/// let format = get_format("test.mp4").unwrap();
/// println!("Format: {}", format);
///
/// // Retrieve the metadata for "test.mp4"
/// let metadata = get_metadata("test.mp4").unwrap();
/// for (key, value) in metadata {
///     println!("{}: {}", key, value);
/// }
/// ```
///
/// These helper functions return the container-level metadata, and they handle any errors that may arise
/// (e.g., if the file can't be opened or if there is an issue reading the data).
pub mod container_info;

/// The **stream_info** module provides utilities to retrieve detailed information
/// about media streams (video, audio, and more) from an input source (e.g., a local file
/// path, an RTMP URL, etc.). It queries FFmpeg for metadata regarding stream types, codec
/// parameters, duration, and other relevant details.
///
/// # Examples
///
/// ```rust,ignore
/// // Retrieve information about the first video stream in "test.mp4"
/// let maybe_video_info = find_video_stream_info("test.mp4").unwrap();
/// if let Some(video_info) = maybe_video_info {
///     println!("Found video stream: {:?}", video_info);
/// } else {
///     println!("No video stream found.");
/// }
///
/// // Retrieve information about the first audio stream in "test.mp4"
/// let maybe_audio_info = find_audio_stream_info("test.mp4").unwrap();
/// if let Some(audio_info) = maybe_audio_info {
///     println!("Found audio stream: {:?}", audio_info);
/// } else {
///     println!("No audio stream found.");
/// }
///
/// // Retrieve information about all streams (video, audio, etc.) in "test.mp4"
/// let all_infos = find_all_stream_infos("test.mp4").unwrap();
/// println!("Total streams found: {}", all_infos.len());
/// for info in all_infos {
///     println!("{:?}", info);
/// }
/// ```
///
/// These helper functions return `Result<Option<StreamInfo>, Error>` or `Result<Vec<StreamInfo>, Error>`
/// depending on the call, allowing you to differentiate between "no stream found" (returns `Ok(None)`)
/// and encountering an actual error (returns `Err(...)`).
pub mod stream_info;

/// The **packet_scanner** module provides a lightweight packet-level scanner for media files.
///
/// Unlike the full demuxing pipeline, `PacketScanner` iterates over raw demuxed packets
/// without any decoding. This is useful for inspecting packet metadata such as timestamps,
/// keyframe flags, sizes, and stream indices.
///
/// # Examples
///
/// ```rust,ignore
/// use ez_ffmpeg::packet_scanner::PacketScanner;
///
/// let mut scanner = PacketScanner::open("test.mp4")?;
/// for packet in scanner.packets() {
///     let packet = packet?;
///     println!(
///         "stream={} pts={:?} size={} keyframe={}",
///         packet.stream_index(),
///         packet.pts(),
///         packet.size(),
///         packet.is_keyframe(),
///     );
/// }
/// ```
pub mod packet_scanner;

/// The **device** module provides cross-platform methods to query available audio and video
/// input devices on the system. Depending on the target operating system, it internally
/// delegates to different platform APIs or FFmpeg’s device capabilities:
///
/// - **macOS**: Leverages AVFoundation for enumerating devices such as cameras ("vide")
///   and microphones ("soun").
/// - **Other OSes**: Uses FFmpeg’s `avdevice` to list input devices for video and audio.
///
/// These functions can be used to programmatically discover devices before choosing one
/// for capture or recording in an FFmpeg-based pipeline.
///
/// # Examples
///
/// ```rust,ignore
/// // Query video input devices (e.g., cameras)
/// let video_devices = get_input_video_devices().unwrap();
/// for device in &video_devices {
///     println!("Available video device: {}", device);
/// }
///
/// // Query audio input devices (e.g., microphones)
/// let audio_devices = get_input_audio_devices().unwrap();
/// for device in &audio_devices {
///     println!("Available audio device: {}", device);
/// }
/// ```
///
/// # Notes
///
/// - If the query process fails (e.g., missing permissions or no devices available),
///   the functions return an appropriate error from `crate::error`.
/// - On macOS, the `AVFoundation` framework is used directly. On other platforms, FFmpeg’s
///   `avdevice` functionality is used. Implementation details differ, but the returned
///   results have a uniform format: a list of human-readable device names.
/// - For more advanced device details (e.g., supported formats or resolutions), you may need
///   to perform additional FFmpeg queries or platform-specific calls.
pub mod device;
/// The **hwaccel** module provides functionality for working with hardware-accelerated
/// codecs in FFmpeg. It allows you to detect and configure various hardware devices
/// (like NVENC, VAAPI, DXVA2, or VideoToolbox) so that FFmpeg can offload encoding or
/// decoding tasks to GPU or specialized hardware.
///
/// # Public API
///
/// - [`get_hwaccels()`](hwaccel::get_hwaccels): Enumerates the hardware acceleration backends available on the
///   current system, returning a list of [`HWAccelInfo`](hwaccel::HWAccelInfo) items. Each item contains a
///   readable name (e.g., `"cuda"`, `"vaapi"`) and the corresponding `AVHWDeviceType`.
///
/// # Example
///
/// ```rust,ignore
/// // Query hardware acceleration backends
/// let hwaccels = get_hwaccels();
/// for accel in hwaccels {
///     println!("Found HW Accel: {} (type: {:?})", accel.name, accel.hw_device_type);
/// }
/// ```
///
/// # Notes
///
/// - While only [`get_hwaccels()`](hwaccel::get_hwaccels) is directly exposed, internally the module contains
///   various helpers to initialize and manage hardware devices (e.g., `hw_device_init_from_string`).
///   These are used behind the scenes or in more advanced scenarios where explicit control
///   over device creation is required.
/// - Hardware acceleration support depends on both FFmpeg’s compilation configuration
///   and the underlying system drivers/frameworks. Not all listed accelerations may be
///   fully functional on every platform.
pub mod hwaccel;

/// The **codec** module provides helpers for enumerating and querying FFmpeg’s
/// available audio/video **encoders** and **decoders**. This can be useful for
/// discovering which codecs are supported in your current FFmpeg build, along
/// with their core attributes.
///
/// # Public API
///
/// - [`get_encoders()`](codec::get_encoders): Returns a list of [`CodecInfo`](codec::CodecInfo) representing all
///   encoders (e.g., H.264, AAC) recognized by FFmpeg.
/// - [`get_decoders()`](codec::get_decoders): Returns a list of [`CodecInfo`](codec::CodecInfo) representing all
///   decoders (e.g., H.264, AAC) recognized by FFmpeg.
///
/// # Example
///
/// ```rust,ignore
/// // List all available encoders
/// let encoders = get_encoders();
/// for enc in &encoders {
///     println!("Encoder: {} - {}", enc.codec_name, enc.codec_long_name);
/// }
///
/// // List all available decoders
/// let decoders = get_decoders();
/// for dec in &decoders {
///     println!("Decoder: {} - {}", dec.codec_name, dec.codec_long_name);
/// }
/// ```
///
/// # Data Structures
///
/// - [`CodecInfo`](codec::CodecInfo): Contains user-friendly fields such as:
///   - `codec_name` / `codec_long_name`
///   - `desc_name`: The descriptor name from FFmpeg.
///   - `media_type` (audio/video/subtitle, etc.)
///   - `codec_id` (internal FFmpeg ID)
///   - `codec_capabilities` (bitmask indicating codec features)
///
/// # Notes
///
/// - The underlying [`Codec`] struct is for internal usage only, bridging to
///   the raw FFmpeg APIs. In most cases, you only need the higher-level [`CodecInfo`](codec::CodecInfo)
///   data from the public functions above.
/// - The available encoders/decoders can vary depending on your FFmpeg build
///   and any external libraries installed on the system.
pub mod codec;

/// The **filter** module provides a flexible framework for custom frame processing
/// within the FFmpeg pipeline, along with the ability to query FFmpeg's built-in filters.
/// It introduces the [`FrameFilter`](filter::frame_filter::FrameFilter) trait, which defines how to apply transformations
/// (e.g., scaling, color adjustments, GPU-accelerated effects) to decoded frames.
/// You can attach these filters to either the input or the output side
/// (depending on your desired pipeline design) so that frames are automatically
/// processed in your FFmpeg workflow.
///
/// # FFmpeg Built-in Filters
///
/// ```rust,ignore
/// use ez_ffmpeg::core::filter::get_filters;
///
/// // Query available FFmpeg filters
/// let filters = get_filters();
/// for filter in filters {
///     println!("Filter: {} - {}", filter.name, filter.description);
/// }
/// ```
///
/// # Defining and Using a Custom Filter
///
/// Below is a minimal example showing how to implement a custom filter and attach it to
/// an `Output` so that every frame is processed before encoding. You could likewise
/// attach it to an `Input` if you want the frames processed immediately after decoding.
///
/// ```rust,ignore
///
/// // 1. Define your custom filter by implementing the FrameFilter trait.
/// struct FlipFilter;
///
/// impl FrameFilter for FlipFilter {
///     fn media_type(&self) -> AVMediaType {
///         // This filter operates on video frames.
///         AVMediaType::AVMEDIA_TYPE_VIDEO
///     }
///
///     fn filter_frame(
///         &mut self,
///         mut frame: Frame,
///         _ctx: &FrameFilterContext,
///     ) -> Result<Option<Frame>, String> {
///          unsafe {
///             if frame.as_ptr().is_null() || frame.is_empty() {
///                 return Ok(Some(frame));
///             }
///          }
///
///         // Here you would implement the logic to transform the frame.
///         // As a trivial example, we just return the original frame.
///         // (Replace this with your actual transformation code.)
///
///         Ok(Some(frame))
///     }
/// }
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // 2. Create a pipeline builder for video frames.
///     let mut pipeline_builder: FramePipelineBuilder = AVMediaType::AVMEDIA_TYPE_VIDEO.into();
///
///     // 3. Add your custom filter to the pipeline, giving it a unique name.
///     pipeline_builder = pipeline_builder.filter("flip-filter", Box::new(FlipFilter));
///
///     // 4. Attach the pipeline to an Output (could also attach to an Input).
///     let mut output: Output = "output.mp4".into();
///     output.add_frame_pipeline(pipeline_builder);
///
///     // 5. Build the FFmpeg context with both input and output.
///     let context = FfmpegContext::builder()
///         .input("input.mp4")
///         .output(output)
///         .build()?;
///
///     // 6. Run the FFmpeg job via the scheduler.
///     FfmpegScheduler::new(context)
///         .start()?
///         .wait()?;
///
///     Ok(())
/// }
/// ```
///
/// In this example:
/// 1. We define a **`FlipFilter`** that implements the [`FrameFilter`](filter::frame_filter::FrameFilter) trait and specifies
///    `AVMediaType::AVMEDIA_TYPE_VIDEO`.
/// 2. We create a **`FramePipelineBuilder`** for `VIDEO` frames and add our filter to it.
/// 3. We attach that pipeline to the **`Output`** configuration, so frames will be processed
///    (in this case, “flipped”) before encoding.
/// 4. Finally, we build the FFmpeg context and run it with the **`FfmpegScheduler`**.
///
/// # More Advanced Filters
///
/// For a more complex, GPU-accelerated example, see the **OpenGL**-based filters in the
/// [`opengl` module](crate::opengl). There, you can use custom GLSL shaders to apply
/// sophisticated transformations or visual effects on video frames.
///
/// # Trait Overview
///
/// The [`FrameFilter`](filter::frame_filter::FrameFilter) trait exposes several methods you can override:
/// - [`FrameFilter::media_type()`](filter::frame_filter::FrameFilter::media_type): Indicates which media type (video, audio, etc.) this filter handles.
/// - [`FrameFilter::init()`](filter::frame_filter::FrameFilter::init): Called once when the filter is first created (e.g., allocate resources).
/// - [`FrameFilter::filter_frame()`](filter::frame_filter::FrameFilter::filter_frame): The primary method for transforming an incoming frame.
/// - [`FrameFilter::request_frame()`](filter::frame_filter::FrameFilter::request_frame): If your filter generates frames on its own, you can override this.
/// - [`FrameFilter::uninit()`](filter::frame_filter::FrameFilter::uninit): Called during cleanup when the filter is removed or the pipeline ends.
///
/// By chaining multiple filters in a pipeline, you can create sophisticated processing
/// chains for your media data.
pub mod filter;

/// The **metadata** module provides internal metadata handling for FFmpeg operations.
///
/// **Internal Use Only**: This module contains unsafe FFmpeg C API wrappers.
/// Users should use the safe public API on `Output` instead:
/// - `Output::add_metadata()` for global metadata
/// - `Output::add_stream_metadata()` for stream metadata
/// - `Output::map_metadata_from_input()` for metadata mapping
/// - `Output::disable_auto_copy_metadata()` for controlling auto-copy
///
/// # Example
/// ```rust,ignore
/// let output = Output::from("output.mp4")
///     .add_metadata("title", "My Video")
///     .add_metadata("author", "John Doe")
///     .add_stream_metadata("v:0", "language", "eng")?;
/// ```
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

// The following type definitions for `VaListType` are inspired by the Rust standard library's
// implementation of `va_list` (see std::ffi::va_list::VaListImpl). These definitions ensure compatibility
// with platform-specific ABI requirements when interfacing with C variadic functions.

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

#[cfg(all(
    target_arch = "aarch64",
    not(target_vendor = "apple"),
    not(target_os = "uefi"),
    not(windows),
))]
type VaListType = ffmpeg_sys_next::va_list;

#[cfg(all(target_arch = "powerpc", not(target_os = "uefi"), not(windows)))]
type VaListType = *mut ffmpeg_sys_next::__va_list_tag_powerpc;

#[cfg(target_arch = "s390x")]
type VaListType = *mut ffmpeg_sys_next::__va_list_tag_s390x;

unsafe extern "C" fn ffmpeg_log_callback(
    ptr: *mut libc::c_void,
    level: libc::c_int,
    fmt: *const libc::c_char,
    args: VaListType,
) {
    // Create a fixed-size buffer to hold the formatted log message.
    let mut buffer = [0u8; 1024];
    // 'print_prefix' is used internally by av_log_format_line to decide whether to print a prefix.
    let mut print_prefix = 1;

    // Call FFmpeg's av_log_format_line to format the variable arguments into the buffer.
    ffmpeg_sys_next::av_log_format_line(
        ptr,
        level,
        fmt,
        args,
        buffer.as_mut_ptr() as *mut libc::c_char,
        buffer.len() as libc::c_int,
        &mut print_prefix,
    );

    // Convert the C string in the buffer to a Rust &str.
    if let Ok(msg) = std::ffi::CStr::from_ptr(buffer.as_ptr() as *const libc::c_char).to_str() {
        // Trim any trailing newline characters (\n or \r).
        let trimmed_msg = msg.trim_end_matches(['\n', '\r']);

        // Map FFmpeg log levels to the corresponding Rust log levels.
        if level <= ffmpeg_sys_next::AV_LOG_ERROR {
            log::error!("FFmpeg: {}", trimmed_msg);
        } else if level <= ffmpeg_sys_next::AV_LOG_WARNING {
            log::warn!("FFmpeg: {}", trimmed_msg);
        } else if level <= ffmpeg_sys_next::AV_LOG_INFO {
            log::info!("FFmpeg: {}", trimmed_msg);
        }
    }
}

fn initialize_ffmpeg() {
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
