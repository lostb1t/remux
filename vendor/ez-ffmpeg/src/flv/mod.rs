//! The **FLV** module contains data structures and tools to parse or handle FLV containers.
//! It helps convert raw media data into FLV tags, making it easier to integrate with
//! FFmpeg-based pipelines or RTMP streaming flows where FLV is the underlying format.
//!
//! **Feature Flag**: Only available when the `flv` feature is enabled.

pub mod flv_tag;
pub mod flv_tag_header;
pub mod flv_buffer;
mod flv_header;


// Define constants for commonly used lengths
pub const PREVIOUS_TAG_SIZE_LENGTH: usize = 4;
pub const FLV_HEADER_LENGTH: usize = 9;
pub const FLV_TAG_HEADER_LENGTH: usize = 11;