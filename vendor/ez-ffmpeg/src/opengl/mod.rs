//! The **OpenGL** module provides a simplified approach to apply OpenGL-based filters/effects
//! to video frames without manually dealing with OpenGL context creation or lifecycle management.
//! Simply provide a fragment shader, and this module handles the details for you.
//!
//! # Example
//!
//! ```rust
//! // Suppose we have some fragment shader code in `fragment_shader`
//! let fragment_shader = r#"
//!     #version 330 core
//!     // ... your shader code ...
//! "#;
//!
//! // 1. Build a pipeline for VIDEO frames
//! let frame_pipeline_builder: FramePipelineBuilder = AVMediaType::AVMEDIA_TYPE_VIDEO.into();
//!
//! // 2. Create an OpenGL filter with the provided fragment shader
//! let filter = OpenGLFrameFilter::new_simple(fragment_shader)
//!     .expect("Failed to create OpenGL frame filter");
//!
//! // 3. Attach it to the pipeline
//! let frame_pipeline_builder = frame_pipeline_builder.filter("opengl", Box::new(filter));
//!
//! // 4. Assign it to your output (e.g., `output.add_frame_pipeline(frame_pipeline_builder)`)
//! //    so that when FFmpeg processes frames, they pass through this OpenGL filter.
//! ```
//!
//! **Feature Flag**: Only available when the `opengl` feature is enabled.

pub mod opengl_frame_filter;