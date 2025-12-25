use crate::filter::frame_pipeline::FramePipeline;
use ffmpeg_sys_next::{AVRational, AVSampleFormat};
use std::collections::HashMap;

unsafe impl Send for Output {}

pub struct Output {
    /// The URL of the output destination.
    ///
    /// This specifies where the output stream will be written. It can be:
    /// - A local file path (e.g., `file:///path/to/output.mp4`).
    /// - A network destination (e.g., `rtmp://example.com/live/stream`).
    /// - Any other URL supported by FFmpeg (e.g., `udp://...`, `http://...`).
    ///
    /// The URL must be valid. If the URL is invalid or unsupported, the library will
    /// return an error when attempting to initialize the output stream.
    pub(crate) url: Option<String>,

    /// A callback function for custom data writing.
    ///
    /// The `write_callback` function allows you to provide custom logic for handling data
    /// output from the encoding process. This is useful for scenarios where the output is
    /// not written directly to a standard destination (like a file or URL), but instead to
    /// a custom data sink, such as an in-memory buffer or a custom network destination.
    ///
    /// The callback receives a buffer of encoded data (`buf: &[u8]`) and should return the number of bytes
    /// successfully written. If an error occurs, a negative value should be returned. For example:
    /// - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)` indicates an I/O error.
    ///
    /// ### Parameters:
    /// - `buf: &[u8]`: A buffer containing the encoded data to be written.
    ///
    /// ### Return Value:
    /// - **Positive Value**: The number of bytes successfully written.
    /// - **Negative Value**: Indicates an error occurred, such as:
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)`: General I/O error.
    ///   - Custom-defined error codes depending on your implementation.
    ///
    /// ### Example:
    /// ```rust
    /// fn custom_write_callback(buf: &[u8]) -> i32 {
    ///     println!("Writing data: {} bytes", buf.len());
    ///     buf.len() as i32 // Return the number of bytes successfully written
    /// }
    /// ```
    /// ### Note:
    /// It is recommended to set the `format` field to the desired output format (e.g., `mp4`, `flv`, etc.)
    /// when using a custom `write_callback`. The `format` ensures that FFmpeg processes the output
    /// correctly for the specified format.
    pub(crate) write_callback: Option<Box<dyn FnMut(&[u8]) -> i32>>,

    /// A callback function for custom seeking within the output stream.
    ///
    /// The `seek_callback` function allows custom logic for adjusting the write position in
    /// the output stream. This is essential for formats that require seeking, such as `mp4`
    /// and `mkv`, where metadata or index information must be updated at specific positions.
    ///
    /// If the output format requires seeking but no `seek_callback` is provided, the operation
    /// may fail, resulting in errors such as:
    /// ```text
    /// [mp4 @ 0x...] muxer does not support non seekable output
    /// ```
    ///
    /// **FFmpeg may invoke `seek_callback` from different threads, so thread safety is required.**
    /// If the destination is a `File`, **wrap it in `Arc<Mutex<File>>`** to ensure safe access.
    ///
    /// ### Parameters:
    /// - `offset: i64`: The target position in the output stream where seeking should occur.
    /// - `whence: i32`: The seek mode, which determines how `offset` should be interpreted:
    ///   - `ffmpeg_sys_next::SEEK_SET` (0) - Seek to an absolute position.
    ///   - `ffmpeg_sys_next::SEEK_CUR` (1) - Seek relative to the current position.
    ///   - `ffmpeg_sys_next::SEEK_END` (2) - Seek relative to the end of the output.
    ///   - `ffmpeg_sys_next::AVSEEK_FLAG_BYTE` (2) - Seek using **byte offset** instead of timestamps.
    ///   - `ffmpeg_sys_next::AVSEEK_SIZE` (65536) - Query the **total size** of the stream.
    ///   - `ffmpeg_sys_next::AVSEEK_FORCE` (131072) - **Force seeking, even if normally restricted.**
    ///
    /// ### Return Value:
    /// - **Positive Value**: The new offset position after seeking.
    /// - **Negative Value**: An error occurred. Common errors include:
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE)`: Seek is not supported.
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)`: General I/O error.
    ///
    /// ### Example (Thread-safe seek callback using `Arc<Mutex<File>>`):
    /// Since `FFmpeg` may call `write_callback` and `seek_callback` from different threads,
    /// **use `Arc<Mutex<File>>` to ensure safe concurrent access.**
    ///
    /// ```rust
    /// use std::fs::File;
    /// use std::io::{Seek, SeekFrom};
    /// use std::sync::{Arc, Mutex};
    ///
    /// let file = Arc::new(Mutex::new(File::create("output.mp4").expect("Failed to create file")));
    ///
    /// let seek_callback = {
    ///     let file = Arc::clone(&file);
    ///     Box::new(move |offset: i64, whence: i32| -> i64 {
    ///         let mut file = file.lock().unwrap();
    ///
    ///         // ✅ Handle AVSEEK_SIZE: Return total file size
    ///         if whence == ffmpeg_sys_next::AVSEEK_SIZE {
    ///             if let Ok(size) = file.metadata().map(|m| m.len() as i64) {
    ///                 println!("FFmpeg requested stream size: {}", size);
    ///                 return size;
    ///             }
    ///             return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
    ///         }
    ///
    ///         // ✅ Ignore AVSEEK_FORCE flag
    ///         let actual_whence = whence & !ffmpeg_sys_next::AVSEEK_FORCE;
    ///
    ///         // ✅ Handle AVSEEK_FLAG_BYTE: Perform byte-based seek
    ///         if actual_whence & ffmpeg_sys_next::AVSEEK_FLAG_BYTE != 0 {
    ///             println!("FFmpeg requested byte-based seeking. Seeking to byte offset: {}", offset);
    ///             if let Ok(new_pos) = file.seek(SeekFrom::Start(offset as u64)) {
    ///                 return new_pos as i64;
    ///             }
    ///             return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
    ///         }
    ///
    ///         // ✅ Standard seek modes
    ///         let seek_result = match actual_whence {
    ///             ffmpeg_sys_next::SEEK_SET => file.seek(SeekFrom::Start(offset as u64)),
    ///             ffmpeg_sys_next::SEEK_CUR => file.seek(SeekFrom::Current(offset)),
    ///             ffmpeg_sys_next::SEEK_END => file.seek(SeekFrom::End(offset)),
    ///             _ => {
    ///                 println!("Unsupported seek mode: {}", whence);
    ///                 return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE) as i64;
    ///             }
    ///         };
    ///
    ///         match seek_result {
    ///             Ok(new_pos) => {
    ///                 println!("Seek successful, new position: {}", new_pos);
    ///                 new_pos as i64
    ///             }
    ///             Err(e) => {
    ///                 println!("Seek failed: {}", e);
    ///                 ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64
    ///             }
    ///         }
    ///     })
    /// };
    /// ```
    pub(crate) seek_callback: Option<Box<dyn FnMut(i64, i32) -> i64>>,

    /// A pipeline specifying how frames will be processed **before encoding**.
    ///
    /// Once input data is decoded into [`Frame`]s, these frames pass through
    /// this pipeline on their way to the encoder. The pipeline is composed of
    /// one or more [`FrameFilter`]s, each providing a specific transformation,
    /// effect, or filter (e.g., resizing, color correction, OpenGL shader
    /// effects, etc.).
    ///
    /// If set to [`None`], no additional processing is applied — frames
    /// are sent to the encoder as they are.
    pub(crate) frame_pipelines: Option<Vec<FramePipeline>>,

    /// Unparsed stream map specifications (user input stage)
    /// These get parsed and expanded into stream_maps during outputs_bind()
    pub(crate) stream_map_specs: Vec<StreamMapSpec>,

    /// Expanded stream maps (FFmpeg-compatible, ready for use)
    /// Each entry maps exactly one input stream to one output stream
    pub(crate) stream_maps: Vec<StreamMap>,

    /// The output format for the container.
    ///
    /// This field specifies the desired output format, such as `mp4`, `flv`, or `mkv`. If `None`, FFmpeg
    /// will attempt to automatically detect the format based on the output URL or filename extension.
    ///
    /// The format can be specified explicitly for scenarios where the format detection is insufficient or
    /// where you want to force a particular container format regardless of the URL or extension.
    pub(crate) format: Option<String>,

    /// The codec to be used for **video** encoding.
    ///
    /// If this field is `None`, FFmpeg will try to select an appropriate video codec based on the
    /// output format or other settings. By setting this field to a specific codec (e.g., `"h264"`, `"hevc"`, etc.),
    /// you can override FFmpeg’s default codec selection. If the specified codec is not available
    /// in your FFmpeg build, an error will be returned during initialization.
    pub(crate) video_codec: Option<String>,

    /// The codec to be used for **audio** encoding.
    ///
    /// If this field is `None`, FFmpeg will try to select an appropriate audio codec based on the
    /// output format or other settings. By providing a value (e.g., `"aac"`, `"mp3"`, etc.),
    /// you override FFmpeg’s default codec choice. If the specified codec is not available
    /// in your FFmpeg build, an error will be returned during initialization.
    pub(crate) audio_codec: Option<String>,

    /// The codec to be used for **subtitle** encoding.
    ///
    /// If this field is `None`, FFmpeg will try to select an appropriate subtitle codec based on
    /// the output format or other settings. Setting this field (e.g., `"mov_text"` for MP4 subtitles)
    /// forces FFmpeg to use the specified codec. If the chosen codec is not supported by your build of FFmpeg,
    /// an error will be returned during initialization.
    pub(crate) subtitle_codec: Option<String>,
    pub(crate) start_time_us: Option<i64>,
    pub(crate) recording_time_us: Option<i64>,
    pub(crate) stop_time_us: Option<i64>,
    pub(crate) framerate: Option<AVRational>,
    pub(crate) vsync_method: VSyncMethod,
    pub(crate) bits_per_raw_sample: Option<i32>,
    pub(crate) audio_sample_rate: Option<i32>,
    pub(crate) audio_channels: Option<i32>,
    pub(crate) audio_sample_fmt: Option<AVSampleFormat>,

    // -q:v
    // use fixed quality scale (VBR)
    pub(crate) video_qscale: Option<i32>,

    // -q:a
    // set audio quality (codec-specific)
    pub(crate) audio_qscale: Option<i32>,

    /// Maximum number of **video** frames to encode (equivalent to `-frames:v` in FFmpeg).
    ///
    /// This option limits the number of **video** frames processed by the encoder.
    ///
    /// **Equivalent FFmpeg Command:**
    /// ```sh
    /// ffmpeg -i input.mp4 -frames:v 100 output.mp4
    /// ```
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_max_video_frames(300);
    /// ```
    pub(crate) max_video_frames: Option<i64>,

    /// Maximum number of **audio** frames to encode (equivalent to `-frames:a` in FFmpeg).
    ///
    /// This option limits the number of **audio** frames processed by the encoder.
    ///
    /// **Equivalent FFmpeg Command:**
    /// ```sh
    /// ffmpeg -i input.mp4 -frames:a 500 output.mp4
    /// ```
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_max_audio_frames(500);
    /// ```
    pub(crate) max_audio_frames: Option<i64>,

    /// Maximum number of **subtitle** frames to encode (equivalent to `-frames:s` in FFmpeg).
    ///
    /// This option limits the number of **subtitle** frames processed by the encoder.
    ///
    /// **Equivalent FFmpeg Command:**
    /// ```sh
    /// ffmpeg -i input.mp4 -frames:s 200 output.mp4
    /// ```
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_max_subtitle_frames(200);
    /// ```
    pub(crate) max_subtitle_frames: Option<i64>,

    /// Video encoder-specific options.
    ///
    /// This field stores key-value pairs for configuring the **video encoder**.
    /// These options are passed to the video encoder before encoding begins.
    ///
    /// **Common Examples:**
    /// - `crf=0` (for lossless quality in x264/x265)
    /// - `preset=ultrafast` (for faster encoding speed in H.264)
    /// - `tune=zerolatency` (for real-time streaming)
    pub(crate) video_codec_opts: Option<HashMap<String, String>>,

    /// Audio encoder-specific options.
    ///
    /// This field stores key-value pairs for configuring the **audio encoder**.
    /// These options are passed to the audio encoder before encoding begins.
    ///
    /// **Common Examples:**
    /// - `b=192k` (for setting bitrate in AAC/MP3)
    /// - `ar=44100` (for setting sample rate)
    pub(crate) audio_codec_opts: Option<HashMap<String, String>>,

    /// Subtitle encoder-specific options.
    ///
    /// This field stores key-value pairs for configuring the **subtitle encoder**.
    /// These options are passed to the subtitle encoder before encoding begins.
    ///
    /// **Common Examples:**
    /// - `mov_text` (for MP4 subtitles)
    /// - `srt` (for subtitle format)
    pub(crate) subtitle_codec_opts: Option<HashMap<String, String>>,

    /// The output format options for the container.
    ///
    /// This field stores additional format-specific options that are passed to the FFmpeg muxer.
    /// It is a collection of key-value pairs that can modify the behavior of the output format.
    ///
    /// Common examples include:
    /// - `movflags=faststart` (for MP4 files)
    /// - `flvflags=no_duration_filesize` (for FLV files)
    ///
    /// These options are used when initializing the FFmpeg output format.
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_format_opt("movflags", "faststart");
    /// ```
    pub(crate) format_opts: Option<HashMap<String, String>>,

    // ========== Metadata Fields ==========
    /// Global metadata for the entire output file
    pub(crate) global_metadata: Option<HashMap<String, String>>,

    /// Stream-specific metadata with stream specifiers
    /// Key: stream specifier string (e.g., "v:0", "a", "s:0")
    /// Value: metadata key-value pairs for matching streams
    /// During output initialization, each specifier is matched against actual streams
    pub(crate) stream_metadata: Vec<(String, String, String)>, // (spec, key, value) tuples

    /// Chapter-specific metadata, indexed by chapter index
    pub(crate) chapter_metadata: HashMap<usize, HashMap<String, String>>,

    /// Program-specific metadata, indexed by program index
    pub(crate) program_metadata: HashMap<usize, HashMap<String, String>>,

    /// Metadata mappings from input files
    pub(crate) metadata_map: Vec<crate::core::metadata::MetadataMapping>,

    /// Whether to automatically copy metadata from input files (default: true)
    /// Replicates FFmpeg's default behavior of copying global and stream metadata
    pub(crate) auto_copy_metadata: bool,
}

#[derive(Copy, Clone, PartialEq)]
pub enum VSyncMethod {
    VsyncAuto,
    VsyncCfr,
    VsyncVfr,
    VsyncPassthrough,
    VsyncVscfr,
}

impl Output {
    pub fn new(url: impl Into<String>) -> Self {
        url.into().into()
    }

    /// Creates a new `Output` instance with a custom write callback and format string.
    ///
    /// This method initializes an `Output` object that uses a provided `write_callback` function
    /// to handle the encoded data being written to the output stream. You can optionally specify
    /// the desired output format via the `format` method.
    ///
    /// ### Parameters:
    /// - `write_callback: fn(buf: &[u8]) -> i32`: A function that processes the provided buffer of
    ///   encoded data and writes it to the destination. The function should return the number of bytes
    ///   successfully written (positive value) or a negative value in case of error.
    ///
    /// ### Return Value:
    /// - Returns a new `Output` instance configured with the specified `write_callback` function.
    ///
    /// ### Behavior of `write_callback`:
    /// - **Positive Value**: Indicates the number of bytes successfully written.
    /// - **Negative Value**: Indicates an error occurred. For example:
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)`: Represents an input/output error.
    ///   - Other custom-defined error codes can also be returned to signal specific issues.
    ///
    /// ### Example:
    /// ```rust
    /// let output = Output::new_by_write_callback(move |buf| {
    ///     println!("Processing {} bytes of data for output", buf.len());
    ///     buf.len() as i32 // Return the number of bytes processed
    /// })
    /// .set_format("mp4");
    /// ```
    pub fn new_by_write_callback<F>(write_callback: F) -> Self
    where
        F: FnMut(&[u8]) -> i32 + 'static,
    {
        (Box::new(write_callback) as Box<dyn FnMut(&[u8]) -> i32>).into()
    }

    /// Sets a custom seek callback for the output stream.
    ///
    /// This function assigns a user-defined function that handles seeking within the output stream.
    /// Seeking is required for certain formats (e.g., `mp4`, `mkv`) where metadata or index information
    /// needs to be updated at specific positions in the file.
    ///
    /// **Why is `seek_callback` necessary?**
    /// - Some formats (e.g., MP4) require `seek` operations to update metadata (`moov`, `mdat`).
    /// - If no `seek_callback` is provided for formats that require seeking, FFmpeg will fail with:
    ///   ```text
    ///   [mp4 @ 0x...] muxer does not support non seekable output
    ///   ```
    /// - For streaming formats (`flv`, `ts`, `rtmp`, `hls`), seeking is **not required**.
    ///
    /// **FFmpeg may invoke `seek_callback` from different threads.**
    /// - If using a `File` as the output, **wrap it in `Arc<Mutex<File>>`** to ensure thread-safe access.
    ///
    /// ### Parameters:
    /// - `seek_callback: FnMut(i64, i32) -> i64`
    ///   - `offset: i64`: The target seek position in the stream.
    ///   - `whence: i32`: The seek mode determining how `offset` should be interpreted:
    ///     - `ffmpeg_sys_next::SEEK_SET` (0): Seek to an absolute position.
    ///     - `ffmpeg_sys_next::SEEK_CUR` (1): Seek relative to the current position.
    ///     - `ffmpeg_sys_next::SEEK_END` (2): Seek relative to the end of the output.
    ///     - `ffmpeg_sys_next::AVSEEK_FLAG_BYTE` (2): Seek using **byte offset** instead of timestamps.
    ///     - `ffmpeg_sys_next::AVSEEK_SIZE` (65536): Query the **total size** of the stream.
    ///     - `ffmpeg_sys_next::AVSEEK_FORCE` (131072): **Force seeking, even if normally restricted.**
    ///
    /// ### Return Value:
    /// - **Positive Value**: The new offset position after seeking.
    /// - **Negative Value**: An error occurred. Common errors include:
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE)`: Seek is not supported.
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)`: General I/O error.
    ///
    /// ### Example (Thread-safe seek callback using `Arc<Mutex<File>>`):
    /// Since `FFmpeg` may call `write_callback` and `seek_callback` from different threads,
    /// **use `Arc<Mutex<File>>` to ensure safe concurrent access.**
    ///
    /// ```rust
    /// use std::fs::File;
    /// use std::io::{Seek, SeekFrom, Write};
    /// use std::sync::{Arc, Mutex};
    ///
    /// // ✅ Create a thread-safe file handle
    /// let file = Arc::new(Mutex::new(File::create("output.mp4").expect("Failed to create file")));
    ///
    /// // ✅ Define the write callback (data writing logic)
    /// let write_callback = {
    ///     let file = Arc::clone(&file);
    ///     move |buf: &[u8]| -> i32 {
    ///         let mut file = file.lock().unwrap();
    ///         match file.write_all(buf) {
    ///             Ok(_) => buf.len() as i32,
    ///             Err(e) => {
    ///                 println!("Write error: {}", e);
    ///                 ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i32
    ///             }
    ///         }
    ///     }
    /// };
    ///
    /// // ✅ Define the seek callback (position adjustment logic)
    /// let seek_callback = {
    ///     let file = Arc::clone(&file);
    ///     Box::new(move |offset: i64, whence: i32| -> i64 {
    ///         let mut file = file.lock().unwrap();
    ///
    ///         match whence {
    ///             // ✅ Handle AVSEEK_SIZE: Return total file size
    ///             ffmpeg_sys_next::AVSEEK_SIZE => {
    ///                 if let Ok(size) = file.metadata().map(|m| m.len() as i64) {
    ///                     println!("FFmpeg requested stream size: {}", size);
    ///                     return size;
    ///                 }
    ///                 return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
    ///             }
    ///
    ///             // ✅ Handle AVSEEK_FLAG_BYTE: Seek using byte offset
    ///             ffmpeg_sys_next::AVSEEK_FLAG_BYTE => {
    ///                 println!("FFmpeg requested byte-based seeking. Seeking to byte offset: {}", offset);
    ///                 if let Ok(new_pos) = file.seek(SeekFrom::Start(offset as u64)) {
    ///                     return new_pos as i64;
    ///                 }
    ///                 return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
    ///             }
    ///
    ///             // ✅ Standard seek modes
    ///             ffmpeg_sys_next::SEEK_SET => file.seek(SeekFrom::Start(offset as u64)),
    ///             ffmpeg_sys_next::SEEK_CUR => file.seek(SeekFrom::Current(offset)),
    ///             ffmpeg_sys_next::SEEK_END => file.seek(SeekFrom::End(offset)),
    ///             _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Unsupported seek mode")),
    ///         }.map_or(ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64, |pos| pos as i64)
    ///     })
    /// };
    ///
    /// // ✅ Create an output with both callbacks
    /// let output = Output::new_by_write_callback(write_callback, "mp4")
    ///     .set_seek_callback(seek_callback);
    /// ```
    pub fn set_seek_callback<F>(mut self, seek_callback: F) -> Self
    where
        F: FnMut(i64, i32) -> i64 + 'static,
    {
        self.seek_callback = Some(Box::new(seek_callback) as Box<dyn FnMut(i64, i32) -> i64>);
        self
    }

    /// Sets the output format for the container.
    ///
    /// This method allows you to specify the output format for the container. If no format is specified,
    /// FFmpeg will attempt to detect it automatically based on the file extension or output URL.
    ///
    /// ### Parameters:
    /// - `format: &str`: A string specifying the desired output format (e.g., `mp4`, `flv`, `mkv`).
    ///
    /// ### Return Value:
    /// - Returns the `Output` instance with the newly set format.
    pub fn set_format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    /// Sets the **video codec** to be used for encoding.
    ///
    /// # Arguments
    /// * `video_codec` - A string slice representing the desired video codec (e.g., `"h264"`, `"hevc"`).
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Output` struct, allowing for method chaining.
    ///
    /// # Examples
    /// ```rust
    /// let output = Output::from("rtmp://localhost/live/stream")
    ///     .set_video_codec("h264");
    /// ```
    pub fn set_video_codec(mut self, video_codec: impl Into<String>) -> Self {
        self.video_codec = Some(video_codec.into());
        self
    }

    /// Sets the **audio codec** to be used for encoding.
    ///
    /// # Arguments
    /// * `audio_codec` - A string slice representing the desired audio codec (e.g., `"aac"`, `"mp3"`).
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Output` struct, allowing for method chaining.
    ///
    /// # Examples
    /// ```rust
    /// let output = Output::from("rtmp://localhost/live/stream")
    ///     .set_audio_codec("aac");
    /// ```
    pub fn set_audio_codec(mut self, audio_codec: impl Into<String>) -> Self {
        self.audio_codec = Some(audio_codec.into());
        self
    }

    /// Sets the **subtitle codec** to be used for encoding.
    ///
    /// # Arguments
    /// * `subtitle_codec` - A string slice representing the desired subtitle codec
    ///   (e.g., `"mov_text"`, `"webvtt"`).
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Output` struct, allowing for method chaining.
    ///
    /// # Examples
    /// ```rust
    /// let output = Output::from("rtmp://localhost/live/stream")
    ///     .set_subtitle_codec("mov_text");
    /// ```
    pub fn set_subtitle_codec(mut self, subtitle_codec: impl Into<String>) -> Self {
        self.subtitle_codec = Some(subtitle_codec.into());
        self
    }

    /// Replaces the entire frame-processing pipeline with a new sequence
    /// of transformations for **pre-encoding** frames on this `Output`.
    ///
    /// This method clears any previously set pipelines and replaces them with the provided list.
    ///
    /// # Parameters
    /// * `frame_pipelines` - A list of [`FramePipeline`] instances defining the
    ///   transformations to apply before encoding.
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Output`, enabling method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_frame_pipelines(vec![
    ///         FramePipelineBuilder::new(AVMediaType::AVMEDIA_TYPE_VIDEO).filter("opengl", Box::new(my_filter)),
    ///         // Additional pipelines...
    ///     ]);
    /// ```
    pub fn set_frame_pipelines(mut self, frame_pipelines: Vec<impl Into<FramePipeline>>) -> Self {
        self.frame_pipelines = Some(
            frame_pipelines
                .into_iter()
                .map(|frame_pipeline| frame_pipeline.into())
                .collect(),
        );
        self
    }

    /// Adds a single [`FramePipeline`] to the existing pipeline list.
    ///
    /// If no pipelines are currently defined, this method creates a new pipeline list.
    /// Otherwise, it appends the provided pipeline to the existing transformations.
    ///
    /// # Parameters
    /// * `frame_pipeline` - A [`FramePipeline`] defining a transformation.
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Output`, enabling method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .add_frame_pipeline(FramePipelineBuilder::new(AVMediaType::AVMEDIA_TYPE_VIDEO).filter("opengl", Box::new(my_filter)).build())
    ///     .add_frame_pipeline(FramePipelineBuilder::new(AVMediaType::AVMEDIA_TYPE_AUDIO).filter("my_custom_filter1", Box::new(...)).filter("my_custom_filter2", Box::new(...)));
    /// ```
    pub fn add_frame_pipeline(mut self, frame_pipeline: impl Into<FramePipeline>) -> Self {
        if self.frame_pipelines.is_none() {
            self.frame_pipelines = Some(vec![frame_pipeline.into()]);
        } else {
            self.frame_pipelines
                .as_mut()
                .unwrap()
                .push(frame_pipeline.into());
        }
        self
    }

    /// Adds a **stream mapping** for a specific stream or stream type,
    /// **re-encoding** it according to this output’s codec settings.
    ///
    /// # Linklabel (FFmpeg-like Specifier)
    ///
    /// This string typically follows `"<input_index>:<media_type>"` syntax:
    /// - **`"0:v"`** – the video stream(s) from input #0.
    /// - **`"1:a?"`** – audio from input #1, **ignore** if none present (due to `?`).
    /// - Other possibilities include `"0:s"`, `"0:d"`, etc. for subtitles/data, optionally with `?`.
    ///
    /// By calling `add_stream_map`, **you force re-encoding** of the chosen stream(s).
    /// If the user wants a bit-for-bit copy, see [`add_stream_map_with_copy`](Self::add_stream_map_with_copy).
    ///
    /// # Parameters
    /// - `linklabel`: An FFmpeg-style specifier referencing the desired input index and
    ///   media type, like `"0:v"`, `"1:a?"`, etc.
    ///
    /// # Returns
    /// * `Self` - for chained method calls.
    ///
    /// # Example
    /// ```rust
    /// // Re-encode the video stream from input #0 (fail if no video).
    /// let output = Output::from("output.mp4")
    ///     .add_stream_map("0:v");
    /// ```
    pub fn add_stream_map(mut self, linklabel: impl Into<String>) -> Self {
        self.stream_map_specs.push(linklabel.into().into());
        self
    }

    /// Adds a **stream mapping** for a specific stream or stream type,
    /// **copying** it bit-for-bit from the source without re-encoding.
    ///
    /// # Linklabel (FFmpeg-like Specifier)
    ///
    /// Follows the same `"<input_index>:<media_type>"` pattern as [`add_stream_map`](Self::add_stream_map):
    /// - **`"0:a"`** – audio stream(s) from input #0.
    /// - **`"0:a?"`** – same, but ignore errors if no audio exists.
    /// - And so on for video (`v`), subtitles (`s`), attachments (`t`), etc.
    ///
    /// # Copy vs. Re-encode
    ///
    /// Here, `copy = true` by default, meaning the chosen stream(s) are passed through
    /// **without** decoding/encoding. This generally **only** works if the source’s codec
    /// is compatible with the container/format you’re outputting to.
    /// If you require re-encoding (e.g., to ensure compatibility or apply filters),
    /// use [`add_stream_map`](Self::add_stream_map).
    ///
    /// # Parameters
    /// - `linklabel`: An FFmpeg-style specifier referencing the desired input index and
    ///   media type, like `"0:v?"`.
    ///
    /// # Returns
    /// * `Self` - for chained method calls.
    ///
    /// # Example
    /// ```rust
    /// // Copy the audio stream(s) from input #0 if present, no re-encode:
    /// let output = Output::from("output.mkv")
    ///     .add_stream_map_with_copy("0:a?");
    /// ```
    pub fn add_stream_map_with_copy(mut self, linklabel: impl Into<String>) -> Self {
        self.stream_map_specs.push(StreamMapSpec {
            linklabel: linklabel.into(),
            copy: true,
        });
        self
    }

    /// Sets the **start time** (in microseconds) for output encoding.
    ///
    /// If this is set, FFmpeg will attempt to start encoding from the specified
    /// timestamp in the input stream. This can be used to skip initial content.
    ///
    /// # Parameters
    /// * `start_time_us` - The start time in microseconds.
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("output.mp4")
    ///     .set_start_time_us(2_000_000); // Start at 2 seconds
    /// ```
    pub fn set_start_time_us(mut self, start_time_us: i64) -> Self {
        self.start_time_us = Some(start_time_us);
        self
    }

    /// Sets the **recording time** (in microseconds) for output encoding.
    ///
    /// This indicates how many microseconds of data should be processed
    /// (i.e., maximum duration to encode). Once this time is reached,
    /// FFmpeg will stop encoding.
    ///
    /// # Parameters
    /// * `recording_time_us` - The maximum duration (in microseconds) to process.
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("output.mp4")
    ///     .set_recording_time_us(5_000_000); // Record for 5 seconds
    /// ```
    pub fn set_recording_time_us(mut self, recording_time_us: i64) -> Self {
        self.recording_time_us = Some(recording_time_us);
        self
    }

    /// Sets a **stop time** (in microseconds) for output encoding.
    ///
    /// If set, FFmpeg will stop encoding once the input’s timestamp
    /// surpasses this value. Effectively, encoding ends at this timestamp
    /// regardless of remaining data.
    ///
    /// # Parameters
    /// * `stop_time_us` - The timestamp (in microseconds) at which to stop.
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("output.mp4")
    ///     .set_stop_time_us(10_000_000); // Stop at 10 seconds
    /// ```
    pub fn set_stop_time_us(mut self, stop_time_us: i64) -> Self {
        self.stop_time_us = Some(stop_time_us);
        self
    }

    /// Sets a **target frame rate** (`AVRational`) for output encoding.
    ///
    /// This can force the output to use a specific frame rate (e.g., 30/1 for 30 FPS).
    /// If unset, FFmpeg typically preserves the source frame rate or uses defaults
    /// based on the selected codec/container.
    ///
    /// # Parameters
    /// * `framerate` - An `AVRational` representing the desired frame rate
    ///   numerator/denominator (e.g., `AVRational { num: 30, den: 1 }` for 30fps).
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// use ffmpeg_sys_next::AVRational;
    /// let output = Output::from("output.mp4")
    ///     .set_framerate(AVRational { num: 30, den: 1 });
    /// ```
    pub fn set_framerate(mut self, framerate: AVRational) -> Self {
        self.framerate = Some(framerate);
        self
    }

    /// Sets the **video sync method** to be used during encoding.
    ///
    /// FFmpeg uses a variety of vsync policies to handle frame presentation times,
    /// dropping/duplicating frames as needed. Adjusting this can be useful when
    /// you need strict CFR (constant frame rate), or to pass frames through
    /// without modification (`VsyncPassthrough`).
    ///
    /// # Parameters
    /// * `method` - A variant of [`VSyncMethod`], such as `VsyncCfr` or `VsyncVfr`.
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("output.mp4")
    ///     .set_vsync_method(VSyncMethod::VsyncCfr);
    /// ```
    pub fn set_vsync_method(mut self, method: VSyncMethod) -> Self {
        self.vsync_method = method;
        self
    }

    /// Sets the **bits per raw sample** for video encoding.
    ///
    /// This value can influence quality or color depth when dealing with
    /// certain pixel formats. Commonly used for high-bit-depth workflows
    /// or specialized encoding scenarios.
    ///
    /// # Parameters
    /// * `bits` - The bits per raw sample (e.g., 8, 10, 12).
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("output.mkv")
    ///     .set_bits_per_raw_sample(10); // e.g., 10-bit
    /// ```
    pub fn set_bits_per_raw_sample(mut self, bits: i32) -> Self {
        self.bits_per_raw_sample = Some(bits);
        self
    }

    /// Sets the **audio sample rate** (in Hz) for output encoding.
    ///
    /// This method allows you to specify the desired audio sample rate for the output.
    /// Common values include 44100 (CD quality), 48000 (standard for digital video),
    /// and 22050 or 16000 (for lower bitrate applications).
    ///
    /// # Parameters
    /// * `audio_sample_rate` - The sample rate in Hertz (e.g., 44100, 48000).
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("output.mp4")
    ///     .set_audio_sample_rate(48000); // Set to 48kHz
    /// ```
    pub fn set_audio_sample_rate(mut self, audio_sample_rate: i32) -> Self {
        self.audio_sample_rate = Some(audio_sample_rate);
        self
    }

    /// Sets the number of **audio channels** for output encoding.
    ///
    /// Common values include 1 (mono), 2 (stereo), 5.1 (6 channels), and 7.1 (8 channels).
    /// This setting affects the spatial audio characteristics of the output.
    ///
    /// # Parameters
    /// * `audio_channels` - The number of audio channels (e.g., 1 for mono, 2 for stereo).
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let output = Output::from("output.mp4")
    ///     .set_audio_channels(2); // Set to stereo
    /// ```
    pub fn set_audio_channels(mut self, audio_channels: i32) -> Self {
        self.audio_channels = Some(audio_channels);
        self
    }

    /// Sets the **audio sample format** for output encoding.
    ///
    /// This method allows you to specify the audio sample format, which affects
    /// how audio samples are represented. Common formats include:
    /// - `AV_SAMPLE_FMT_S16` (signed 16-bit)
    /// - `AV_SAMPLE_FMT_S32` (signed 32-bit)
    /// - `AV_SAMPLE_FMT_FLT` (32-bit float)
    /// - `AV_SAMPLE_FMT_FLTP` (32-bit float, planar)
    ///
    /// The format choice can impact quality, processing requirements, and compatibility.
    ///
    /// # Parameters
    /// * `sample_fmt` - An `AVSampleFormat` enum value specifying the desired sample format.
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// use ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_S16;
    ///
    /// let output = Output::from("output.mp4")
    ///     .set_audio_sample_fmt(AV_SAMPLE_FMT_S16); // Set to signed 16-bit
    /// ```
    pub fn set_audio_sample_fmt(mut self, sample_fmt: AVSampleFormat) -> Self {
        self.audio_sample_fmt = Some(sample_fmt);
        self
    }

    /// Sets the **video quality scale** (VBR) for encoding.
    ///
    /// This method configures a fixed quality scale for variable bitrate (VBR) video encoding.
    /// Lower values result in higher quality but larger file sizes, while higher values
    /// produce lower quality with smaller file sizes.
    ///
    /// # Note on Modern Usage
    /// While still supported, using fixed quality scale (`-q:v`) is generally not recommended
    /// for modern video encoding workflows with codecs like H.264 and H.265. Instead, consider:
    /// * For H.264/H.265: Use CRF (Constant Rate Factor) via `-crf` parameter
    /// * For two-pass encoding: Use target bitrate settings
    ///
    /// This parameter is primarily useful for older codecs or specific scenarios where
    /// direct quality scale control is needed.
    ///
    /// # Quality Scale Ranges by Codec
    /// * **H.264/H.265**: 0-51 (if needed: 17-28)
    ///   - 17-18: Visually lossless
    ///   - 23: High quality
    ///   - 28: Good quality with reasonable file size
    /// * **MPEG-4/MPEG-2**: 2-31 (recommended: 2-6)
    ///   - Lower values = higher quality
    /// * **VP9**: 0-63 (if needed: 15-35)
    ///
    /// # Parameters
    /// * `video_qscale` - The quality scale value for video encoding.
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// // For MJPEG encoding of image sequences
    /// let output = Output::from("output.jpg")
    ///     .set_video_qscale(2);  // High quality JPEG images
    ///
    /// // For legacy image format conversion
    /// let output = Output::from("output.png")
    ///     .set_video_qscale(3);  // Controls compression level
    /// ```
    pub fn set_video_qscale(mut self, video_qscale: i32) -> Self {
        self.video_qscale = Some(video_qscale);
        self
    }

    /// Sets the **audio quality scale** for encoding.
    ///
    /// This method configures codec-specific audio quality settings. The range, behavior,
    /// and optimal values depend entirely on the audio codec being used.
    ///
    /// # Quality Scale Ranges by Codec
    /// * **MP3 (libmp3lame)**: 0-9 (recommended: 2-5)
    ///   - 0: Highest quality
    ///   - 2: Near-transparent quality (~190-200 kbps)
    ///   - 5: Good quality (~130 kbps)
    ///   - 9: Lowest quality
    /// * **AAC**: 0.1-255 (recommended: 1-5)
    ///   - 1: Highest quality (~250 kbps)
    ///   - 3: Good quality (~160 kbps)
    ///   - 5: Medium quality (~100 kbps)
    /// * **Vorbis**: -1 to 10 (recommended: 3-8)
    ///   - 10: Highest quality
    ///   - 5: Good quality
    ///   - 3: Medium quality
    ///
    /// # Parameters
    /// * `audio_qscale` - The quality scale value for audio encoding.
    ///
    /// # Returns
    /// * `Self` - The modified `Output`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// // For MP3 encoding at high quality
    /// let output = Output::from("output.mp3")
    ///     .set_audio_codec("libmp3lame")
    ///     .set_audio_qscale(2);
    ///
    /// // For AAC encoding at good quality
    /// let output = Output::from("output.m4a")
    ///     .set_audio_codec("aac")
    ///     .set_audio_qscale(3);
    ///
    /// // For Vorbis encoding at high quality
    /// let output = Output::from("output.ogg")
    ///     .set_audio_codec("libvorbis")
    ///     .set_audio_qscale(7);
    /// ```
    pub fn set_audio_qscale(mut self, audio_qscale: i32) -> Self {
        self.audio_qscale = Some(audio_qscale);
        self
    }

    /// **Sets the maximum number of video frames to encode (`-frames:v`).**
    ///
    /// **Equivalent FFmpeg Command:**
    /// ```sh
    /// ffmpeg -i input.mp4 -frames:v 100 output.mp4
    /// ```
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_max_video_frames(500);
    /// ```
    pub fn set_max_video_frames(mut self, max_frames: impl Into<Option<i64>>) -> Self {
        self.max_video_frames = max_frames.into();
        self
    }

    /// **Sets the maximum number of audio frames to encode (`-frames:a`).**
    ///
    /// **Equivalent FFmpeg Command:**
    /// ```sh
    /// ffmpeg -i input.mp4 -frames:a 500 output.mp4
    /// ```
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_max_audio_frames(500);
    /// ```
    pub fn set_max_audio_frames(mut self, max_frames: impl Into<Option<i64>>) -> Self {
        self.max_audio_frames = max_frames.into();
        self
    }

    /// **Sets the maximum number of subtitle frames to encode (`-frames:s`).**
    ///
    /// **Equivalent FFmpeg Command:**
    /// ```sh
    /// ffmpeg -i input.mp4 -frames:s 200 output.mp4
    /// ```
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_max_subtitle_frames(200);
    /// ```
    pub fn set_max_subtitle_frames(mut self, max_frames: impl Into<Option<i64>>) -> Self {
        self.max_subtitle_frames = max_frames.into();
        self
    }

    /// Sets a **video codec-specific option**.
    ///
    /// These options control **video encoding parameters** such as compression, quality, and speed.
    ///
    /// **Supported Parameters:**
    /// | Parameter | Description |
    /// |-----------|-------------|
    /// | `crf=0-51` | Quality level for x264/x265, lower means higher quality (`0` is lossless) |
    /// | `preset=ultrafast, superfast, fast, medium, slow, veryslow` | Encoding speed, affects compression efficiency |
    /// | `tune=film, animation, grain, stillimage, fastdecode, zerolatency` | Optimizations for specific types of content |
    /// | `b=4M` | Bitrate (e.g., `4M` for 4 Mbps) |
    /// | `g=50` | GOP (Group of Pictures) size, affects keyframe frequency |
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_video_codec_opt("crf", "18")
    ///     .set_video_codec_opt("preset", "fast");
    /// ```
    pub fn set_video_codec_opt(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        if let Some(ref mut opts) = self.video_codec_opts {
            opts.insert(key.into(), value.into());
        } else {
            let mut opts = HashMap::new();
            opts.insert(key.into(), value.into());
            self.video_codec_opts = Some(opts);
        }
        self
    }

    /// **Sets multiple video codec options at once.**
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_video_codec_opts(vec![
    ///         ("crf", "18"),
    ///         ("preset", "fast")
    ///     ]);
    /// ```
    pub fn set_video_codec_opts(
        mut self,
        opts: Vec<(impl Into<String>, impl Into<String>)>,
    ) -> Self {
        let video_opts = self.video_codec_opts.get_or_insert_with(HashMap::new);
        for (key, value) in opts {
            video_opts.insert(key.into(), value.into());
        }
        self
    }

    /// Sets a **audio codec-specific option**.
    ///
    /// These options control **audio encoding parameters** such as bitrate, sample rate, and format.
    ///
    /// **Supported Parameters:**
    /// | Parameter | Description |
    /// |-----------|-------------|
    /// | `b=192k` | Bitrate (e.g., `128k` for 128 Kbps, `320k` for 320 Kbps) |
    /// | `compression_level=0-12` | Compression efficiency for formats like FLAC |
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_audio_codec_opt("b", "320k")
    ///     .set_audio_codec_opt("compression_level", "6");
    /// ```
    pub fn set_audio_codec_opt(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        if let Some(ref mut opts) = self.audio_codec_opts {
            opts.insert(key.into(), value.into());
        } else {
            let mut opts = HashMap::new();
            opts.insert(key.into(), value.into());
            self.audio_codec_opts = Some(opts);
        }
        self
    }

    /// **Sets multiple audio codec options at once.**
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_audio_codec_opts(vec![
    ///         ("b", "320k"),
    ///         ("compression_level", "6")
    ///     ]);
    /// ```
    pub fn set_audio_codec_opts(
        mut self,
        opts: Vec<(impl Into<String>, impl Into<String>)>,
    ) -> Self {
        let audio_opts = self.audio_codec_opts.get_or_insert_with(HashMap::new);
        for (key, value) in opts {
            audio_opts.insert(key.into(), value.into());
        }
        self
    }

    /// Sets a **subtitle codec-specific option**.
    ///
    /// These options control **subtitle encoding parameters** such as format and character encoding.
    ///
    /// **Supported Parameters:**
    /// | Parameter | Description |
    /// |-----------|-------------|
    /// | `mov_text` | Subtitle format for MP4 files |
    /// | `srt` | Subtitle format for `.srt` files |
    /// | `ass` | Advanced SubStation Alpha (ASS) subtitle format |
    /// | `forced_subs=1` | Forces the subtitles to always be displayed |
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_subtitle_codec_opt("mov_text", "");
    /// ```
    pub fn set_subtitle_codec_opt(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        if let Some(ref mut opts) = self.subtitle_codec_opts {
            opts.insert(key.into(), value.into());
        } else {
            let mut opts = HashMap::new();
            opts.insert(key.into(), value.into());
            self.subtitle_codec_opts = Some(opts);
        }
        self
    }

    /// **Sets multiple subtitle codec options at once.**
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_subtitle_codec_opts(vec![
    ///         ("mov_text", ""),
    ///         ("forced_subs", "1")
    ///     ]);
    /// ```
    pub fn set_subtitle_codec_opts(
        mut self,
        opts: Vec<(impl Into<String>, impl Into<String>)>,
    ) -> Self {
        let subtitle_opts = self.subtitle_codec_opts.get_or_insert_with(HashMap::new);
        for (key, value) in opts {
            subtitle_opts.insert(key.into(), value.into());
        }
        self
    }

    /// Sets a format-specific option for the output container.
    ///
    /// FFmpeg supports various format-specific options that can be passed to the muxer.
    /// These options allow fine-tuning of the output container’s behavior.
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_format_opt("movflags", "faststart")
    ///     .set_format_opt("flvflags", "no_duration_filesize");
    /// ```
    ///
    /// ### Common Format Options:
    /// | Format | Option | Description |
    /// |--------|--------|-------------|
    /// | `mp4`  | `movflags=faststart` | Moves moov atom to the beginning of the file for faster playback start |
    /// | `flv`  | `flvflags=no_duration_filesize` | Removes duration/size metadata for live streaming |
    ///
    /// **Parameters:**
    /// - `key`: The format option name (e.g., `"movflags"`, `"flvflags"`).
    /// - `value`: The value to set (e.g., `"faststart"`, `"no_duration_filesize"`).
    ///
    /// Returns the modified `Output` struct for chaining.
    pub fn set_format_opt(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        if let Some(ref mut opts) = self.format_opts {
            opts.insert(key.into(), value.into());
        } else {
            let mut opts = HashMap::new();
            opts.insert(key.into(), value.into());
            self.format_opts = Some(opts);
        }
        self
    }

    /// Sets multiple format-specific options at once.
    ///
    /// This method allows setting multiple format options in a single call.
    ///
    /// **Example Usage:**
    /// ```rust
    /// let output = Output::from("some_url")
    ///     .set_format_opts(vec![
    ///         ("movflags", "faststart"),
    ///         ("flvflags", "no_duration_filesize")
    ///     ]);
    /// ```
    ///
    /// **Parameters:**
    /// - `opts`: A vector of key-value pairs representing format options.
    ///
    /// Returns the modified `Output` struct for chaining.
    pub fn set_format_opts(mut self, opts: Vec<(impl Into<String>, impl Into<String>)>) -> Self {
        if let Some(ref mut format_opts) = self.format_opts {
            for (key, value) in opts {
                format_opts.insert(key.into(), value.into());
            }
        } else {
            let mut format_opts = HashMap::new();
            for (key, value) in opts {
                format_opts.insert(key.into(), value.into());
            }
            self.format_opts = Some(format_opts);
        }
        self
    }

    // ========== Metadata API Methods ==========
    // The following helpers mirror FFmpeg's command-line metadata options as implemented in
    // fftools/ffmpeg_opt.c (opt_metadata / opt_map_metadata) and the automatic propagation rules
    // in fftools/ffmpeg_mux_init.c:3050-3070. Each method references the corresponding FFmpeg
    // behavior so callers can cross-check the C implementation when needed.

    /// Add or update global metadata for the output file.
    ///
    /// If value is empty string, the key will be removed (FFmpeg behavior).
    /// Replicates FFmpeg's `-metadata key=value` option.
    ///
    /// FFmpeg reference: fftools/ffmpeg_opt.c (`opt_metadata()` handles `-metadata key=value`).
    ///
    /// # Examples
    /// ```rust,ignore
    /// let output = Output::from("output.mp4")
    ///     .add_metadata("title", "My Video")
    ///     .add_metadata("author", "John Doe");
    /// ```
    pub fn add_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        let value = value.into();

        if value.is_empty() {
            // Empty value means remove the key (FFmpeg behavior)
            if let Some(ref mut metadata) = self.global_metadata {
                metadata.remove(&key);
            }
        } else {
            self.global_metadata
                .get_or_insert_with(HashMap::new)
                .insert(key, value);
        }
        self
    }

    /// Add multiple global metadata entries at once.
    ///
    /// FFmpeg reference: fftools/ffmpeg_opt.c (consecutive `-metadata` invocations append to the
    /// same dictionary; this helper simply batches the calls on the Rust side).
    ///
    /// # Examples
    /// ```rust,ignore
    /// let mut metadata = HashMap::new();
    /// metadata.insert("title".to_string(), "My Video".to_string());
    /// metadata.insert("author".to_string(), "John Doe".to_string());
    ///
    /// let output = Output::from("output.mp4")
    ///     .add_metadata_map(metadata);
    /// ```
    pub fn add_metadata_map(mut self, metadata: HashMap<String, String>) -> Self {
        for (key, value) in metadata {
            self = self.add_metadata(key, value);
        }
        self
    }

    /// Remove a global metadata key.
    ///
    /// FFmpeg reference: fftools/ffmpeg_opt.c (`-metadata key=` deletes the key when value is
    /// empty; we follow the same rule by interpreting an empty string as removal).
    ///
    /// # Examples
    /// ```rust,ignore
    /// let output = Output::from("output.mp4")
    ///     .add_metadata("title", "My Video")
    ///     .remove_metadata("title");  // Remove the title
    /// ```
    pub fn remove_metadata(mut self, key: &str) -> Self {
        if let Some(ref mut metadata) = self.global_metadata {
            metadata.remove(key);
        }
        self
    }

    /// Clear all metadata (global, stream, chapter, program) and mappings.
    ///
    /// Useful when you want to start fresh without any metadata.
    ///
    /// FFmpeg reference: fftools/ffmpeg_opt.c (users typically issue `-map_metadata -1` and then
    /// reapply `-metadata` options; this helper emulates that workflow programmatically).
    ///
    /// # Examples
    /// ```rust,ignore
    /// let output = Output::from("output.mp4")
    ///     .add_metadata("title", "My Video")
    ///     .clear_all_metadata();  // Remove all metadata
    /// ```
    pub fn clear_all_metadata(mut self) -> Self {
        self.global_metadata = None;
        self.stream_metadata.clear();
        self.chapter_metadata.clear();
        self.program_metadata.clear();
        self.metadata_map.clear();
        self
    }


    /// Disable automatic metadata copying from input files.
    ///
    /// By default, FFmpeg automatically copies global and stream metadata
    /// from input files to output. This method disables that behavior,
    /// similar to FFmpeg's `-map_metadata -1` option.
    /// FFmpeg reference: ffmpeg_mux_init.c (`copy_meta()` sets metadata_global_manual when
    /// `-map_metadata -1` is used; `auto_copy_metadata` mirrors the same flag).
    ///
    /// # Examples
    /// ```rust,ignore
    /// let output = Output::from("output.mp4")
    ///     .disable_auto_copy_metadata()  // Don't copy any metadata from input
    ///     .add_metadata("title", "New Title");  // Only use explicitly set metadata
    /// ```
    pub fn disable_auto_copy_metadata(mut self) -> Self {
        self.auto_copy_metadata = false;
        self
    }

    /// Add or update stream-specific metadata.
    ///
    /// Uses FFmpeg's stream specifier syntax to identify target streams.
    /// If value is empty string, the key will be removed (FFmpeg behavior).
    /// Replicates FFmpeg's `-metadata:s:spec key=value` option.
    ///
    /// FFmpeg reference: fftools/ffmpeg_opt.c (`opt_metadata()` with stream specifiers, lines
    /// 2465-2520 in FFmpeg 7.x).
    ///
    /// # Stream Specifier Syntax
    /// - `"v:0"` - First video stream
    /// - `"a:1"` - Second audio stream
    /// - `"s"` - All subtitle streams
    /// - `"v"` - All video streams
    /// - `"p:0:v"` - Video streams in program 0
    /// - `"#0x100"` or `"i:256"` - Stream with specific ID
    /// - `"m:language:eng"` - Streams with metadata language=eng
    /// - `"u"` - Usable streams only
    /// - `"disp:default"` - Streams with default disposition
    ///
    /// # Examples
    /// ```rust,ignore
    /// let output = Output::from("output.mp4")
    ///     .add_stream_metadata("v:0", "language", "eng")
    ///     .add_stream_metadata("a:0", "title", "Main Audio");
    /// ```
    ///
    /// # Errors
    /// Returns error if the stream specifier syntax is invalid.
    pub fn add_stream_metadata(
        mut self,
        stream_spec: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, String> {
        use crate::core::metadata::StreamSpecifier;

        let stream_spec_str = stream_spec.into();
        let key = key.into();
        let value = value.into();

        // Parse and validate stream specifier
        let _specifier = StreamSpecifier::parse(&stream_spec_str)?;

        // Store as (spec, key, value) tuple
        // During output initialization, this will be matched against actual streams
        // using StreamSpecifier::matches and applied to all matching streams
        // Replicates FFmpeg's of_add_metadata behavior
        self.stream_metadata.push((stream_spec_str, key, value));

        Ok(self)
    }

    /// Add or update chapter-specific metadata.
    ///
    /// Chapters are used for DVD-like navigation points in media files.
    /// If value is empty string, the key will be removed (FFmpeg behavior).
    /// Replicates FFmpeg's `-metadata:c:N key=value` option.
    /// FFmpeg reference: fftools/ffmpeg_opt.c (`opt_metadata()` handles the `c:` target selector).
    ///
    /// # Examples
    /// ```rust,ignore
    /// let output = Output::from("output.mp4")
    ///     .add_chapter_metadata(0, "title", "Introduction")
    ///     .add_chapter_metadata(1, "title", "Main Content");
    /// ```
    pub fn add_chapter_metadata(
        mut self,
        chapter_index: usize,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let key = key.into();
        let value = value.into();

        if value.is_empty() {
            // Empty value means remove the key (FFmpeg behavior)
            if let Some(metadata) = self.chapter_metadata.get_mut(&chapter_index) {
                metadata.remove(&key);
            }
        } else {
            self.chapter_metadata
                .entry(chapter_index)
                .or_insert_with(HashMap::new)
                .insert(key, value);
        }
        self
    }

    /// Add or update program-specific metadata.
    ///
    /// Programs are used in multi-program transport streams (e.g., MPEG-TS).
    /// If value is empty string, the key will be removed (FFmpeg behavior).
    /// Replicates FFmpeg's `-metadata:p:N key=value` option.
    /// FFmpeg reference: fftools/ffmpeg_opt.c (`opt_metadata()` with `p:` selector).
    ///
    /// # Examples
    /// ```rust,ignore
    /// let output = Output::from("output.ts")
    ///     .add_program_metadata(0, "service_name", "Channel 1")
    ///     .add_program_metadata(1, "service_name", "Channel 2");
    /// ```
    pub fn add_program_metadata(
        mut self,
        program_index: usize,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let key = key.into();
        let value = value.into();

        if value.is_empty() {
            // Empty value means remove the key (FFmpeg behavior)
            if let Some(metadata) = self.program_metadata.get_mut(&program_index) {
                metadata.remove(&key);
            }
        } else {
            self.program_metadata
                .entry(program_index)
                .or_insert_with(HashMap::new)
                .insert(key, value);
        }
        self
    }

    /// Map metadata from an input file to this output.
    ///
    /// Replicates FFmpeg's `-map_metadata [src_file_idx]:src_type:dst_type` option.
    /// This allows copying metadata from specific locations in input files to
    /// specific locations in the output file.
    ///
    /// # Type Specifiers
    /// - `"g"` or `""` - Global metadata
    /// - `"s"` or `"s:spec"` - Stream metadata (with optional stream specifier)
    /// - `"c:N"` - Chapter N metadata
    /// - `"p:N"` - Program N metadata
    ///
    /// # Examples
    /// ```rust,ignore
    /// use ez_ffmpeg::core::metadata::{MetadataType, MetadataMapping};
    ///
    /// let output = Output::from("output.mp4")
    ///     // Copy global metadata from input 0 to output global
    ///     .map_metadata_from_input(0, "g", "g")?
    ///     // Copy first video stream metadata from input 1 to output first video stream
    ///     .map_metadata_from_input(1, "s:v:0", "s:v:0")?;
    /// ```
    ///
    /// # Errors
    /// Returns error if the type specifier syntax is invalid.
    /// FFmpeg reference: fftools/ffmpeg_opt.c (`opt_map_metadata()` parses the same
    /// `[file][:type]` triplet and feeds it into `MetadataMapping`).
    pub fn map_metadata_from_input(
        mut self,
        input_index: usize,
        src_type_spec: impl Into<String>,
        dst_type_spec: impl Into<String>,
    ) -> Result<Self, String> {
        use crate::core::metadata::{MetadataMapping, MetadataType};

        let src_type = MetadataType::parse(&src_type_spec.into())?;
        let dst_type = MetadataType::parse(&dst_type_spec.into())?;

        self.metadata_map.push(MetadataMapping {
            src_type,
            dst_type,
            input_index,
        });

        Ok(self)
    }


}

impl From<Box<dyn FnMut(&[u8]) -> i32>> for Output {
    fn from(write_callback_and_format: Box<dyn FnMut(&[u8]) -> i32>) -> Self {
        Self {
            url: None,
            write_callback: Some(write_callback_and_format),
            seek_callback: None,
            frame_pipelines: None,
            stream_map_specs: vec![],
            stream_maps: vec![],
            format: None,
            video_codec: None,
            audio_codec: None,
            subtitle_codec: None,
            start_time_us: None,
            recording_time_us: None,
            stop_time_us: None,
            framerate: None,
            vsync_method: VSyncMethod::VsyncAuto,
            bits_per_raw_sample: None,
            audio_sample_rate: None,
            audio_channels: None,
            audio_sample_fmt: None,
            video_qscale: None,
            audio_qscale: None,
            max_video_frames: None,
            max_audio_frames: None,
            max_subtitle_frames: None,
            video_codec_opts: None,
            audio_codec_opts: None,
            subtitle_codec_opts: None,
            format_opts: None,
            // Metadata fields - initialized with defaults
            global_metadata: None,
            stream_metadata: Vec::new(),
            chapter_metadata: HashMap::new(),
            program_metadata: HashMap::new(),
            metadata_map: Vec::new(),
            auto_copy_metadata: true, // FFmpeg default: auto-copy enabled
        }
    }
}

impl From<String> for Output {
    fn from(url: String) -> Self {
        Self {
            url: Some(url),
            write_callback: None,
            seek_callback: None,
            frame_pipelines: None,
            stream_map_specs: vec![],
            stream_maps: vec![],
            format: None,
            video_codec: None,
            audio_codec: None,
            subtitle_codec: None,
            start_time_us: None,
            recording_time_us: None,
            stop_time_us: None,
            framerate: None,
            vsync_method: VSyncMethod::VsyncAuto,
            bits_per_raw_sample: None,
            audio_sample_rate: None,
            audio_channels: None,
            audio_sample_fmt: None,
            video_qscale: None,
            audio_qscale: None,
            max_video_frames: None,
            max_audio_frames: None,
            max_subtitle_frames: None,
            video_codec_opts: None,
            audio_codec_opts: None,
            subtitle_codec_opts: None,
            format_opts: None,
            // Metadata fields - initialized with defaults
            global_metadata: None,
            stream_metadata: Vec::new(),
            chapter_metadata: HashMap::new(),
            program_metadata: HashMap::new(),
            metadata_map: Vec::new(),
            auto_copy_metadata: true, // FFmpeg default: auto-copy enabled
        }
    }
}

impl From<&str> for Output {
    fn from(url: &str) -> Self {
        Self::from(String::from(url))
    }
}

/// Temporary storage for unparsed stream map specifications (user input stage)
/// Equivalent to FFmpeg's command-line parsing before opt_map() expansion
#[derive(Debug, Clone)]
pub(crate) struct StreamMapSpec {
    /// Stream specifier string: "0:v", "1:a:0", "0:v?", "[label]", etc.
    pub(crate) linklabel: String,
    /// Stream copy flag (-c copy)
    pub(crate) copy: bool,
}

impl<T: Into<String>> From<T> for StreamMapSpec {
    fn from(linklabel: T) -> Self {
        Self {
            linklabel: linklabel.into(),
            copy: false,
        }
    }
}

/// Final expanded stream map (matches FFmpeg's StreamMap structure)
/// Created after parsing and expansion in outputs_bind()
/// FFmpeg reference: fftools/ffmpeg.h:134-141
#[derive(Debug, Clone)]
pub(crate) struct StreamMap {
    /// 1 if this mapping is disabled by a negative map (-map -0:v)
    pub(crate) disabled: bool,
    /// Input file index
    pub(crate) file_index: usize,
    /// Input stream index within the file
    pub(crate) stream_index: usize,
    /// Name of an output link, for mapping lavfi outputs (e.g., "[v]", "myout")
    pub(crate) linklabel: Option<String>,
    /// Stream copy flag (-c copy)
    pub(crate) copy: bool,
}
