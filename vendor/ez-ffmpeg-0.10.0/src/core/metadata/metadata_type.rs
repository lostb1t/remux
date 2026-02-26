// Metadata type and mapping structures
// Based on FFmpeg ffmpeg_mux_init.c

use super::stream_specifier::StreamSpecifier;

/// Metadata type - identifies which metadata dictionary to operate on
/// From FFmpeg's metadata type system
#[derive(Debug, Clone)]
pub enum MetadataType {
    /// Global metadata (type 'g')
    Global,
    /// Stream metadata (type 's') with stream specifier
    Stream(StreamSpecifier),
    /// Chapter metadata (type 'c') with chapter index
    Chapter(usize),
    /// Program metadata (type 'p') with program index
    Program(usize),
}

impl MetadataType {
    /// Parse a metadata type specifier string.
    ///
    /// Supported syntax:
    /// - `"g"` or empty - Global metadata
    /// - `"s"` or `"s:spec"` - Stream metadata with optional stream specifier
    /// - `"c:N"` - Chapter N metadata
    /// - `"p:N"` - Program N metadata
    ///
    /// FFmpeg reference: ffmpeg_mux_init.c:2696 parse_meta_type
    ///
    /// # Examples
    /// ```ignore
    /// use ez_ffmpeg::core::metadata::MetadataType;
    ///
    /// // Global metadata
    /// let meta = MetadataType::parse("g").unwrap();
    /// let meta = MetadataType::parse("").unwrap(); // defaults to global
    ///
    /// // Stream metadata
    /// let meta = MetadataType::parse("s").unwrap();
    /// let meta = MetadataType::parse("s:v:0").unwrap(); // first video stream
    /// let meta = MetadataType::parse("s:a").unwrap(); // all audio streams
    ///
    /// // Chapter metadata
    /// let meta = MetadataType::parse("c:0").unwrap(); // chapter 0
    ///
    /// // Program metadata
    /// let meta = MetadataType::parse("p:1").unwrap(); // program 1
    /// ```
    pub fn parse(spec: &str) -> Result<Self, String> {
        if spec.is_empty() {
            return Ok(MetadataType::Global);
        }

        let mut chars = spec.chars();
        let type_char = chars.next().unwrap();

        match type_char {
            'g' => {
                // Global metadata - should not have additional content
                if chars.next().is_some() {
                    return Err(format!("Invalid metadata specifier: {}", spec));
                }
                Ok(MetadataType::Global)
            }

            's' => {
                // Stream metadata - optional stream specifier after ':'
                let remainder: String = chars.collect();

                if remainder.is_empty() {
                    // "s" - all streams
                    Ok(MetadataType::Stream(StreamSpecifier::default()))
                } else if let Some(stream_spec) = remainder.strip_prefix(':') {
                    // "s:spec" - specific streams
                    // skip the ':'
                    let specifier = if stream_spec.is_empty() {
                        StreamSpecifier::default()
                    } else {
                        StreamSpecifier::parse(stream_spec)?
                    };
                    Ok(MetadataType::Stream(specifier))
                } else {
                    // Invalid - 's' must be followed by ':' or nothing
                    Err(format!("Invalid metadata specifier: {}", spec))
                }
            }

            'c' => {
                // Chapter metadata - must have ':N' format
                let remainder: String = chars.collect();
                if !remainder.starts_with(':') {
                    return Err(format!("Invalid metadata specifier: {}", spec));
                }

                let index_str = &remainder[1..]; // skip the ':'
                let index = index_str
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid chapter index: {}", index_str))?;

                Ok(MetadataType::Chapter(index))
            }

            'p' => {
                // Program metadata - must have ':N' format
                let remainder: String = chars.collect();
                if !remainder.starts_with(':') {
                    return Err(format!("Invalid metadata specifier: {}", spec));
                }

                let index_str = &remainder[1..]; // skip the ':'
                let index = index_str
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid program index: {}", index_str))?;

                Ok(MetadataType::Program(index))
            }

            _ => Err(format!("Invalid metadata type: {}", type_char)),
        }
    }
}

/// Metadata mapping - specifies how to copy metadata from input to output
/// Metadata mapping structure for input→output metadata transfer
#[derive(Debug, Clone)]
pub struct MetadataMapping {
    /// Source metadata type
    pub src_type: MetadataType,
    /// Destination metadata type
    pub dst_type: MetadataType,
    /// Input file index
    pub input_index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::metadata::stream_specifier::StreamListType;
    use ffmpeg_sys_next::AVMediaType;

    #[test]
    fn test_parse_global_metadata() {
        // Global metadata - "g" or empty string
        assert!(matches!(
            MetadataType::parse("g").unwrap(),
            MetadataType::Global
        ));

        assert!(matches!(
            MetadataType::parse("").unwrap(),
            MetadataType::Global
        ));
    }

    #[test]
    fn test_parse_stream_metadata() {
        // Simple stream specifier "s"
        let result = MetadataType::parse("s").unwrap();
        assert!(matches!(result, MetadataType::Stream(_)));

        // Stream with type "s:v"
        let result = MetadataType::parse("s:v").unwrap();
        if let MetadataType::Stream(spec) = result {
            assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        } else {
            panic!("Expected Stream variant");
        }

        // Stream with index "s:a:0"
        let result = MetadataType::parse("s:a:0").unwrap();
        if let MetadataType::Stream(spec) = result {
            assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_AUDIO));
            assert_eq!(spec.idx, Some(0));
        } else {
            panic!("Expected Stream variant");
        }

        // Stream with metadata filter "s:m:language:eng"
        let result = MetadataType::parse("s:m:language:eng").unwrap();
        if let MetadataType::Stream(spec) = result {
            assert_eq!(spec.meta_key, Some("language".to_string()));
            assert_eq!(spec.meta_val, Some("eng".to_string()));
        } else {
            panic!("Expected Stream variant");
        }
    }

    #[test]
    fn test_parse_chapter_metadata() {
        // Chapter with index "c:0"
        let result = MetadataType::parse("c:0").unwrap();
        assert!(matches!(result, MetadataType::Chapter(0)));

        // Chapter with larger index "c:10"
        let result = MetadataType::parse("c:10").unwrap();
        assert!(matches!(result, MetadataType::Chapter(10)));
    }

    #[test]
    fn test_parse_program_metadata() {
        // Program with index "p:0"
        let result = MetadataType::parse("p:0").unwrap();
        assert!(matches!(result, MetadataType::Program(0)));

        // Program with larger index "p:5"
        let result = MetadataType::parse("p:5").unwrap();
        assert!(matches!(result, MetadataType::Program(5)));
    }

    #[test]
    fn test_parse_errors() {
        // Invalid specifier type
        assert!(MetadataType::parse("x").is_err());

        // Chapter without index
        assert!(MetadataType::parse("c").is_err());

        // Chapter with invalid index
        assert!(MetadataType::parse("c:abc").is_err());

        // Program without index
        assert!(MetadataType::parse("p").is_err());

        // Program with invalid index
        assert!(MetadataType::parse("p:xyz").is_err());

        // Invalid stream specifier
        assert!(MetadataType::parse("s:invalid").is_err());
    }

    #[test]
    fn test_parse_boundary_values() {
        // Large chapter index
        let result = MetadataType::parse("c:999").unwrap();
        assert!(matches!(result, MetadataType::Chapter(999)));

        // Large program index
        let result = MetadataType::parse("p:999").unwrap();
        assert!(matches!(result, MetadataType::Program(999)));

        // Zero indices
        let result = MetadataType::parse("c:0").unwrap();
        assert!(matches!(result, MetadataType::Chapter(0)));

        let result = MetadataType::parse("p:0").unwrap();
        assert!(matches!(result, MetadataType::Program(0)));
    }

    #[test]
    fn test_parse_complex_stream_specifiers() {
        // Stream with disposition
        let result = MetadataType::parse("s:disp:default:v").unwrap();
        if let MetadataType::Stream(spec) = result {
            assert_eq!(spec.disposition, Some("default".to_string()));
            assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        } else {
            panic!("Expected Stream variant");
        }

        // Stream with usable flag (usable terminates, so must be last)
        let result = MetadataType::parse("s:v:u").unwrap();
        if let MetadataType::Stream(spec) = result {
            assert_eq!(spec.usable_only, true);
            assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        } else {
            panic!("Expected Stream variant");
        }

        // Stream with program specifier
        let result = MetadataType::parse("s:p:10:v:0").unwrap();
        if let MetadataType::Stream(spec) = result {
            assert_eq!(spec.list_type, StreamListType::Program);
            assert_eq!(spec.list_id, 10);
            assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
            assert_eq!(spec.idx, Some(0));
        } else {
            panic!("Expected Stream variant");
        }
    }

    #[test]
    fn test_parse_special_characters() {
        // Stream metadata with special characters in key/value
        let result = MetadataType::parse("s:m:my_key:my-value").unwrap();
        if let MetadataType::Stream(spec) = result {
            assert_eq!(spec.meta_key, Some("my_key".to_string()));
            assert_eq!(spec.meta_val, Some("my-value".to_string()));
        } else {
            panic!("Expected Stream variant");
        }

        // Metadata value with spaces
        let result = MetadataType::parse("s:m:title:Test Video").unwrap();
        if let MetadataType::Stream(spec) = result {
            assert_eq!(spec.meta_key, Some("title".to_string()));
            assert_eq!(spec.meta_val, Some("Test Video".to_string()));
        } else {
            panic!("Expected Stream variant");
        }
    }
}
