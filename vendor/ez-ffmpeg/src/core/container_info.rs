use ffmpeg_next::format;

/// Gets the duration of a media file in microseconds.
///
/// # Arguments
/// - `input`: The path to the input file (e.g., `"video.mp4"`).
///
/// # Returns
/// - `Result<i64, ffmpeg_next::Error>`: Returns the duration as an `i64` value in microseconds.
///   If an error occurs, it returns an `ffmpeg_next::Error`.
///
/// # Example
/// ```rust
/// let duration = get_duration_us("video.mp4").unwrap();
/// println!("Duration: {} us", duration);
/// ```
pub fn get_duration_us(input: impl Into<String>) -> Result<i64, ffmpeg_next::Error> {
    // Open the media file using `format::input` and get the `FormatContext`
    let format_context = format::input(&input.into())?;

    // Get the duration of the media file in microseconds
    let duration = format_context.duration();

    // Return the duration
    Ok(duration)
}

/// Gets the format name of a media file (e.g., "mp4", "avi").
///
/// # Arguments
/// - `input`: The path to the input file (e.g., `"video.mp4"`).
///
/// # Returns
/// - `Result<String, ffmpeg_next::Error>`: Returns a string representing the format of the media file.
///   If an error occurs, it returns an `ffmpeg_next::Error`.
///
/// # Example
/// ```rust
/// let format = get_format("video.mp4").unwrap();
/// println!("Format: {}", format);
/// ```
pub fn get_format(input: impl Into<String>) -> Result<String, ffmpeg_next::Error> {
    // Open the media file using `format::input` and get the `FormatContext`
    let format_context = format::input(&input.into())?;

    // Get the format name of the media file and return it as a string
    Ok(format_context.format().name().to_string())
}

/// Gets the metadata of a media file (e.g., title, artist).
///
/// # Arguments
/// - `input`: The path to the input file (e.g., `"video.mp4"`).
///
/// # Returns
/// - `Result<Vec<(String, String)>, ffmpeg_next::Error>`: Returns a vector of key-value pairs representing the metadata.
///   Each key is a metadata field (e.g., `"title"`, `"artist"`) and each value is the corresponding value.
///   If an error occurs, it returns an `ffmpeg_next::Error`.
///
/// # Example
/// ```rust
/// let metadata = get_metadata("video.mp4").unwrap();
/// for (key, value) in metadata {
///     println!("{}: {}", key, value);
/// }
/// ```
pub fn get_metadata(input: impl Into<String>) -> Result<Vec<(String, String)>, ffmpeg_next::Error> {
    // Open the media file using `format::input` and get the `FormatContext`
    let format_context = format::input(&input.into())?;

    // Get the metadata and convert it to a vector of key-value pairs
    Ok(format_context
        .metadata()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

/// Gets the metadata of a specific chapter in a media file.
///
/// FFmpeg reference: AVChapter->metadata in libavformat/avformat.h:1253
/// Accesses chapter metadata dictionary similar to AVFormatContext->metadata
///
/// # Arguments
/// - `input`: The path to the input file (e.g., `"video.mp4"`).
/// - `chapter_index`: The index of the chapter (0-based, not the chapter ID).
///
/// # Returns
/// - `Result<Vec<(String, String)>, ffmpeg_next::Error>`: Returns a vector of key-value pairs representing the chapter metadata.
///   Each key is a metadata field and each value is the corresponding value.
///   If the chapter doesn't exist or an error occurs, it returns an `ffmpeg_next::Error`.
///
/// # Example
/// ```rust
/// let metadata = get_chapter_metadata("video.mp4", 0).unwrap();
/// for (key, value) in metadata {
///     println!("{}: {}", key, value);
/// }
/// ```
pub fn get_chapter_metadata(
    input: impl Into<String>,
    chapter_index: usize,
) -> Result<Vec<(String, String)>, ffmpeg_next::Error> {
    // Open the media file using `format::input` and get the `FormatContext`
    let format_context = format::input(&input.into())?;

    // Get the chapter at the specified index
    // FFmpeg reference: AVFormatContext->chapters[chapter_index] in avformat.h:1390
    // FFmpeg behavior: functions such as avformat_seek_file()/copy_chapters() return AVERROR(EINVAL)
    // for an out-of-range chapter index. We mirror that by mapping the absence of the chapter to
    // ffmpeg_next::Error::InvalidData so callers can treat it the same way as FFmpeg C APIs.
    let chapter = format_context
        .chapter(chapter_index)
        .ok_or_else(|| ffmpeg_next::Error::InvalidData)?;

    // Get the chapter metadata and convert it to a vector of key-value pairs
    // FFmpeg reference: AVChapter->metadata (AVDictionary) in avformat.h:1253
    Ok(chapter
        .metadata()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

/// Gets the metadata of a specific stream in a media file.
///
/// FFmpeg reference: AVStream->metadata in libavformat/avformat.h:1310
/// Accesses stream metadata dictionary similar to AVFormatContext->metadata
///
/// # Arguments
/// - `input`: The path to the input file (e.g., `"video.mp4"`).
/// - `stream_index`: The index of the stream (0-based).
///
/// # Returns
/// - `Result<Vec<(String, String)>, ffmpeg_next::Error>`: Returns a vector of key-value pairs representing the stream metadata.
///   Each key is a metadata field and each value is the corresponding value.
///   If the stream doesn't exist or an error occurs, it returns an `ffmpeg_next::Error`.
///
/// # Example
/// ```rust
/// let metadata = get_stream_metadata("video.mp4", 0).unwrap();
/// for (key, value) in metadata {
///     println!("{}: {}", key, value);
/// }
/// ```
pub fn get_stream_metadata(
    input: impl Into<String>,
    stream_index: usize,
) -> Result<Vec<(String, String)>, ffmpeg_next::Error> {
    // Open the media file using `format::input` and get the `FormatContext`
    let format_context = format::input(&input.into())?;

    // Get the stream at the specified index
    // FFmpeg reference: AVFormatContext->streams[stream_index] in avformat.h:1476
    let stream = format_context
        .stream(stream_index)
        .ok_or_else(|| ffmpeg_next::Error::StreamNotFound)?;

    // Get the stream metadata and convert it to a vector of key-value pairs
    // FFmpeg reference: AVStream->metadata (AVDictionary) in avformat.h:1310
    Ok(stream
        .metadata()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Integration tests rely on the sample `test.mp4` stored at the repository root.
    const TEST_VIDEO_PATH: &str = "test.mp4";

    fn require_test_asset() {
        assert!(
            Path::new(TEST_VIDEO_PATH).exists(),
            "Expected '{}' to exist in the repo root for container_info tests",
            TEST_VIDEO_PATH
        );
    }

    #[test]
    fn test_get_chapter_metadata_returns_invalid_when_missing() {
        require_test_asset();
        let result = get_chapter_metadata(TEST_VIDEO_PATH, 0);
        assert!(matches!(result, Err(ffmpeg_next::Error::InvalidData)));
    }

    #[test]
    fn test_get_chapter_metadata_invalid_index() {
        require_test_asset();
        let result = get_chapter_metadata(TEST_VIDEO_PATH, 999);
        assert!(matches!(result, Err(ffmpeg_next::Error::InvalidData)));
    }

    #[test]
    fn test_get_stream_metadata_video_stream() {
        require_test_asset();
        let metadata = get_stream_metadata(TEST_VIDEO_PATH, 0).unwrap();
        assert!(!metadata.is_empty());
    }

    #[test]
    fn test_get_stream_metadata_audio_stream() {
        require_test_asset();
        let metadata = get_stream_metadata(TEST_VIDEO_PATH, 1).unwrap();
        assert!(!metadata.is_empty());
    }

    #[test]
    fn test_get_stream_metadata_invalid_index() {
        require_test_asset();
        let result = get_stream_metadata(TEST_VIDEO_PATH, 999);
        assert!(matches!(result, Err(ffmpeg_next::Error::StreamNotFound)));
    }

    #[test]
    fn test_get_metadata_returns_global_entries() {
        require_test_asset();
        let metadata = get_metadata(TEST_VIDEO_PATH).unwrap();
        assert!(!metadata.is_empty());
    }
}
