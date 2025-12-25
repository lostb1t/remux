//! # ez-ffmpeg
//!
//! **ez-ffmpeg** provides a safe and ergonomic Rust interface for [FFmpeg](https://ffmpeg.org)
//! integration. By abstracting away much of the raw C API complexity,
//! It abstracts the complexity of the raw C API, allowing you to configure media pipelines,
//! perform transcoding and filtering, and inspect streams with ease.
//!
//! ## Crate Layout
//!
//! - **`core`**: The foundational module that contains the main building blocks for configuring
//!   and running FFmpeg pipelines. This includes:
//!   - `Input` / `Output`: Descriptors for where media data comes from and goes to (files, URLs,
//!     custom I/O callbacks, etc.).
//!   - `FilterComplex` and [`FrameFilter`](filter::frame_filter::FrameFilter): Mechanisms for applying FFmpeg filter graphs or
//!     custom transformations.
//!   - `container_info`: Utilities to extract information about the container, such as duration and format details.
//!   - `stream_info`: Utilities to query media metadata (duration, codecs, etc.).
//!   - `hwaccel`: Helpers for enumerating and configuring hardware-accelerated video codecs
//!     (CUDA, VAAPI, VideoToolbox, etc.).
//!   - `codec`: Tools to list and inspect available encoders/decoders.
//!   - `device`: Utilities to discover system cameras, microphones, and other input devices.
//!   - `filter`: Query FFmpeg's built-in filters and infrastructure for building custom frame-processing filters.
//!   - `context`: Houses [`FfmpegContext`] for assembling an FFmpeg job.
//!   - `scheduler`: Provides [`FfmpegScheduler`] which manages the lifecycle of that job.
//!
//! - **`opengl`** (feature `"opengl"`): Offers GPU-accelerated OpenGL filters, letting you
//!   provide a fragment shader and apply effects to video frames in a high-performance way
//!   without manually managing the GL context.
//!
//! - **`rtmp`** (feature `"rtmp"`): Includes an embedded RTMP server, `EmbedRtmpServer`, which
//!   can receive data directly from memory instead of over a network. Handy for local tests
//!   or simple streaming scenarios.
//!
//! - **`flv`** (feature `"flv"`): Provides data structures and helpers for handling FLV
//!   containers, useful if you’re working with RTMP or other FLV-based workflows.
//!
//! ## Basic Usage
//!
//! For a simple pipeline, you typically do the following:
//!
//! 1. Build a [`FfmpegContext`] by specifying at least one [input](Input)
//!    and one [output](Output). Optionally, add filter descriptions
//!    (`filter_desc`) or attach [`FrameFilter`](filter::frame_filter::FrameFilter) pipelines at either the input (post-decode)
//!    or the output (pre-encode) stage.
//! 2. Create an [`FfmpegScheduler`] from that context, then call `start()` and `wait()` (or `.await`
//!    if you enable the `"async"` feature) to run the job.
//!
//! ```rust
//! use ez_ffmpeg::FfmpegContext;
//! use ez_ffmpeg::FfmpegScheduler;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 1. Build the FFmpeg context
//!     let context = FfmpegContext::builder()
//!         .input("input.mp4")
//!         .filter_desc("hue=s=0") // Example filter: desaturate
//!         .output("output.mov")
//!         .build()?;
//!
//!     // 2. Run it via FfmpegScheduler (sync mode)
//!     let result = FfmpegScheduler::new(context)
//!         .start()?
//!         .wait();
//!     result?; // If any error occurred, propagate it
//!     Ok(())
//! }
//! ```
//!
//! ## Feature Flags
//!
//! **`ez-ffmpeg`** uses Cargo features to provide optional functionality. By default, no optional
//! features are enabled, allowing you to keep dependencies minimal. You can enable features as needed
//! in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies.ez-ffmpeg]
//! version = "*"
//! features = ["opengl", "rtmp", "flv", "async"]
//! ```
//!
//! ### Core Features
//!
//! - **`opengl`**: Enables OpenGL-based filters for GPU-accelerated processing.
//! - **`rtmp`**: Enables an embedded RTMP server for local streaming scenarios.
//! - **`flv`**: Adds FLV container parsing and handling.
//! - **`async`**: Makes the [`FfmpegScheduler`] wait method asynchronous (you can `.await` it).
//! - **`static`**: Uses static linking for FFmpeg libraries (via `ffmpeg-next/static`).
//!
//! ## License Notice
//!
//! ez-ffmpeg is distributed under the WTFPL (Do What The F*ck You Want To Public License).
//! You are free to use, modify, and distribute this software.
//!
//! **Note:** FFmpeg itself is subject to its own licensing terms. When enabling features that incorporate FFmpeg components,
//! please ensure that your usage complies with FFmpeg's license.

pub mod core;
pub mod util;
pub mod error;

pub use self::core::context::ffmpeg_context::FfmpegContext;
pub use self::core::context::input::Input;
pub use self::core::context::output::Output;
pub use self::core::scheduler::ffmpeg_scheduler::FfmpegScheduler;
pub use self::core::container_info;
pub use self::core::stream_info;
pub use self::core::device;
pub use self::core::hwaccel;
pub use self::core::codec;
pub use self::core::filter;

pub use ffmpeg_sys_next::AVRational;
pub use ffmpeg_sys_next::AVMediaType;
pub use ffmpeg_next::Frame;


#[cfg(feature = "opengl")]
pub mod opengl;
#[cfg(feature = "opengl")]
use surfman::declare_surfman;
#[cfg(feature = "opengl")]
declare_surfman!();

#[cfg(feature = "rtmp")]
pub mod rtmp;

#[cfg(feature = "flv")]
pub mod flv;