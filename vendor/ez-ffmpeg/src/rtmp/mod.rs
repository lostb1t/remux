//! The **RTMP** module includes an embedded RTMP server (`EmbedRtmpServer`) that can receive
//! data directly from memory—bypassing actual network sockets—to facilitate local or test
//! streaming scenarios. It is inspired by `rml_rtmp`’s threaded RTMP server.
//!
//! > **Not recommended** for high-concurrency (e.g., more than 1000 connections) use cases.
//!
//! # Example
//!
//! ```rust
//! // 1. Create and start an embedded RTMP server on "localhost:1935"
//! let mut embed_rtmp_server = EmbedRtmpServer::new("localhost:1935")
//!          .start().unwrap();
//!
//! // 2. Create an RTMP “input”: (app_name="my-app", stream_key="my-stream")
//! //    This returns an `Output` for FFmpeg to push data into.
//! let output = embed_rtmp_server
//!     .create_rtmp_input("my-app", "my-stream")
//!     .unwrap();
//!
//! // 3. Prepare an `Input` pointing to a local file, for example "test.mp4"
//! let mut input: Input = "test.mp4".into();
//! input.readrate = Some(1.0); // optional: limit reading speed
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
mod rtmp_connection;
mod gop;