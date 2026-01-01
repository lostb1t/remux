// Metadata module for FFmpeg metadata operations
//
// FFmpeg source references:
// - stream_specifier.rs: cmdutils.c (stream_specifier_parse, check_stream_specifier)
// - metadata_type.rs: ffmpeg_opt.c (parse_meta_type, get_metadata_dict)
// - operations.rs: ffmpeg_mux_init.c (of_add_metadata, copy_metadata, copy_dict_with_filter)
// - default_behavior.rs: ffmpeg_mux_init.c (init_muxer - metadata section)

pub mod default_behavior;
pub mod metadata_type;
pub mod operations;
pub mod stream_specifier;

// Re-export main types and functions for convenience
pub use default_behavior::{copy_chapters_from_input, copy_metadata_default};
pub use metadata_type::{MetadataMapping, MetadataType};
pub use operations::{copy_metadata, of_add_metadata};
pub use stream_specifier::StreamSpecifier;
