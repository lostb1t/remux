//! The **RTMP** module includes an embedded RTMP server (`EmbedRtmpServer`) built for
//! production-grade streaming with high concurrency support. It receives data directly from
//! memory—bypassing TCP between FFmpeg and the server. Inspired by `rml_rtmp`'s threaded RTMP server.
//!
//! # Architecture (v0.6.0+)
//!
//! The RTMP server uses a single-thread **Reactor** pattern with IO multiplexing:
//! - **Linux**: epoll (edge-triggered)
//! - **macOS/BSD**: kqueue (EV_CLEAR edge-triggered)
//! - **Windows**: WSAPoll (level-triggered)
//!
//! ## Key Components
//!
//! - `Reactor`: Single-thread event loop with IO multiplexing
//! - `Poller`: Cross-platform IO multiplexer abstraction
//! - `WriteQueue`: Per-connection write buffer with backpressure management
//! - `FrozenGop`: Zero-copy GOP (Group of Pictures) sharing via `Arc<[FrameData]>`
//!
//! ## Backpressure Management
//!
//! | Level | Threshold | Behavior |
//! |-------|-----------|----------|
//! | Normal | < 1MB | All frames enqueued |
//! | Warning | 1-2MB | Drop non-keyframe video, keep audio + keyframes |
//! | High | 2-4MB | Keep keyframes and sequence headers only |
//! | Critical | ≥ 4MB | Disconnect |
//!
//! ## Performance Characteristics
//!
//! | Metric | Value |
//! |--------|-------|
//! | Thread count | 2 (accept + reactor) |
//! | Memory per connection | Variable (8KB read buffer + write queue) |
//! | Max connections | 10,000 default on Linux/macOS (auto-adjusted by FD limit × 80%); 8,000 on Windows |
//! | GOP clone complexity | O(1) |
//!
//! ## Advantages over Traditional RTMP Servers
//!
//! | Dimension | ez-ffmpeg Embedded | nginx-rtmp |
//! |-----------|-------------------|------------|
//! | I/O Model | Native epoll/kqueue (libc FFI) | nginx event loop |
//! | Data Path | In-process ingest (no TCP between FFmpeg and server) | Network serialization |
//! | GOP Sharing | `Arc<[FrameData]>` O(1) clone | Network copy |
//! | Deployment | Zero-config, code-embedded | Separate deployment |
//!
//! # Example
//!
//! ```rust,ignore
//! // 1. Create and start an embedded RTMP server on "localhost:1935"
//! let embed_rtmp_server = EmbedRtmpServer::new("localhost:1935")
//!     .start()
//!     .unwrap();
//!
//! // 2. Create an RTMP "input": (app_name="my-app", stream_key="my-stream")
//! //    This returns an `Output` for FFmpeg to push data into.
//! let output = embed_rtmp_server
//!     .create_rtmp_input("my-app", "my-stream")
//!     .unwrap();
//!
//! // 3. Prepare an `Input` using builder pattern
//! let input = Input::from("test.mp4")
//!     .set_readrate(1.0); // optional: limit reading speed
//!
//! // 4. Build and run the FFmpeg context
//! let result = FfmpegContext::builder()
//!     .input(input)
//!     .output(output)
//!     .build().unwrap()
//!     .start().unwrap()
//!     .wait();
//!
//! if let Err(e) = result {
//!     eprintln!("FFmpeg encountered an error: {:?}", e);
//! }
//!
//! // When done, you can stop the server: `embed_rtmp_server.stop();`
//! ```
//!
//! **Feature Flag**: Only available when the `rtmp` feature is enabled.

pub mod embed_rtmp_server;
mod rtmp_scheduler;
mod gop;
mod poller;
mod write_queue;
mod reactor;