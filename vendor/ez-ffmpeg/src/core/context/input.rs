use std::collections::HashMap;
use crate::filter::frame_pipeline::FramePipeline;

unsafe impl Send for Input {}

pub struct Input {
    /// The URL of the input source.
    ///
    /// This specifies the source from which the input stream is obtained. It can be:
    /// - A local file path (e.g., `file:///path/to/video.mp4`).
    /// - A network stream (e.g., `rtmp://example.com/live/stream`).
    /// - Any other URL supported by FFmpeg (e.g., `http://example.com/video.mp4`, `udp://...`).
    ///
    /// The URL must be valid. If the URL is invalid or unsupported,
    /// the library will return an error when attempting to open the input stream.
    pub(crate) url: Option<String>,

    /// A callback function for custom data reading.
    ///
    /// The `read_callback` function allows you to provide custom logic for feeding data into
    /// the input stream. This is useful for scenarios where the input does not come directly
    /// from a standard source (like a file or URL), but instead from a custom data source,
    /// such as an in-memory buffer or a custom network stream.
    ///
    /// ### Parameters:
    /// - `buf: &mut [u8]`: A mutable buffer into which the data should be written.
    ///   The callback should fill this buffer with as much data as possible, up to its length.
    ///
    /// ### Return Value:
    /// - **Positive Value**: The number of bytes successfully read into `buf`.
    /// - **`ffmpeg_sys_next::AVERROR_EOF`**: Indicates the end of the input stream. No more data will be read.
    /// - **Negative Value**: Indicates an error occurred, such as:
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)`: General I/O error.
    ///   - Custom-defined error codes depending on your implementation.
    ///
    /// ### Example:
    /// ```rust
    /// fn custom_read_callback(buf: &mut [u8]) -> i32 {
    ///     let data = b"example data stream";
    ///     let len = data.len().min(buf.len());
    ///     buf[..len].copy_from_slice(&data[..len]);
    ///     len as i32 // Return the number of bytes written into the buffer
    /// }
    /// ```
    pub(crate) read_callback: Option<Box<dyn FnMut(&mut [u8]) -> i32>>,

    /// A callback function for custom seeking within the input stream.
    ///
    /// The `seek_callback` function allows defining custom seeking behavior.
    /// This is useful for data sources that support seeking, such as files or memory-mapped data.
    /// For non-seekable streams (e.g., live network streams), this function may return an error.
    ///
    /// **FFmpeg may invoke `seek_callback` from multiple threads, so thread safety is required.**
    /// When using a `File` as an input source, **use `Arc<Mutex<File>>` to ensure safe access.**
    ///
    /// ### Parameters:
    /// - `offset: i64`: The target position in the stream for seeking.
    /// - `whence: i32`: The seek mode defining how the `offset` should be interpreted:
    ///   - `ffmpeg_sys_next::SEEK_SET` (0): Seek to an absolute position.
    ///   - `ffmpeg_sys_next::SEEK_CUR` (1): Seek relative to the current position.
    ///   - `ffmpeg_sys_next::SEEK_END` (2): Seek relative to the end of the stream.
    ///   - `ffmpeg_sys_next::SEEK_HOLE` (3): Find the next file hole (sparse file support).
    ///   - `ffmpeg_sys_next::SEEK_DATA` (4): Find the next data block (sparse file support).
    ///   - `ffmpeg_sys_next::AVSEEK_FLAG_BYTE` (2): Seek using **byte offsets** instead of timestamps.
    ///   - `ffmpeg_sys_next::AVSEEK_SIZE` (65536): Query the **total size** of the stream.
    ///   - `ffmpeg_sys_next::AVSEEK_FORCE` (131072): **Force seeking even if normally restricted.**
    ///
    /// ### Return Value:
    /// - **Positive Value**: The new offset position after seeking.
    /// - **Negative Value**: An error occurred. Common errors include:
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE)`: Seek is not supported.
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)`: General I/O error.
    ///
    /// ### Example (Handling multi-threaded access safely with `Arc<Mutex<File>>`):
    /// Since FFmpeg may call `read_callback` and `seek_callback` from different threads,
    /// **`Arc<Mutex<File>>` is used to ensure safe access across threads.**
    ///
    /// ```rust
    /// use std::fs::File;
    /// use std::io::{Seek, SeekFrom};
    /// use std::sync::{Arc, Mutex};
    ///
    /// let file = Arc::new(Mutex::new(File::open("test.mp4").expect("Failed to open file")));
    ///
    /// let seek_callback = {
    ///     let file = Arc::clone(&file);
    ///     Box::new(move |offset: i64, whence: i32| -> i64 {
    ///         let mut file = file.lock().unwrap(); // Acquire lock
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
    ///         // ✅ Handle AVSEEK_FORCE: Ignore this flag when processing seek
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
    ///         // ✅ Handle SEEK_HOLE and SEEK_DATA (Linux only)
    ///         #[cfg(target_os = "linux")]
    ///         if actual_whence == ffmpeg_sys_next::SEEK_HOLE {
    ///             println!("FFmpeg requested SEEK_HOLE, but Rust std::fs does not support it.");
    ///             return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE) as i64;
    ///         }
    ///         #[cfg(target_os = "linux")]
    ///         if actual_whence == ffmpeg_sys_next::SEEK_DATA {
    ///             println!("FFmpeg requested SEEK_DATA, but Rust std::fs does not support it.");
    ///             return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE) as i64;
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

    /// The pipeline that provides custom processing for decoded frames.
    ///
    /// After the input data is decoded into `Frame` objects, these frames
    /// are passed through the `frame_pipeline`. Each frame goes through
    /// a series of `FrameFilter` objects in the pipeline, allowing for
    /// customized processing (e.g., filtering, transformation, etc.).
    ///
    /// If `None`, no processing pipeline is applied to the decoded frames.
    pub(crate) frame_pipelines: Option<Vec<FramePipeline>>,

    /// The input format for the source.
    ///
    /// This field specifies which container or device format FFmpeg should use to read the input.
    /// If `None`, FFmpeg will attempt to automatically detect the format based on the source URL,
    /// file extension, or stream data.
    ///
    /// You might need to specify a format explicitly in cases where automatic detection fails or
    /// when you must force a particular format. For example:
    /// - When capturing from a specific device on macOS (using `avfoundation`).
    /// - When capturing on Windows devices (using `dshow`).
    /// - When dealing with raw streams or unusual data sources.
    pub(crate) format: Option<String>,

    /// The codec to be used for **video** decoding.
    ///
    /// If set, this forces FFmpeg to use the specified video codec for decoding.
    /// Otherwise, FFmpeg will attempt to auto-detect the best available codec.
    pub(crate) video_codec: Option<String>,

    /// The codec to be used for **audio** decoding.
    ///
    /// If set, this forces FFmpeg to use the specified audio codec for decoding.
    /// Otherwise, FFmpeg will attempt to auto-detect the best available codec.
    pub(crate) audio_codec: Option<String>,

    /// The codec to be used for **subtitle** decoding.
    ///
    /// If set, this forces FFmpeg to use the specified subtitle codec for decoding.
    /// Otherwise, FFmpeg will attempt to auto-detect the best available codec.
    pub(crate) subtitle_codec: Option<String>,

    pub(crate) exit_on_error: Option<bool>,

    /// read input at specified rate.
    /// when set 1. read input at native frame rate.
    pub(crate) readrate: Option<f32>,
    pub(crate) start_time_us: Option<i64>,
    pub(crate) recording_time_us: Option<i64>,
    pub(crate) stop_time_us: Option<i64>,

    /// set number of times input stream shall be looped
    pub(crate) stream_loop: Option<i32>,

    /// Hardware Acceleration name
    /// use Hardware accelerated decoding
    pub(crate) hwaccel: Option<String>,
    /// select a device for HW acceleration
    pub(crate) hwaccel_device: Option<String>,
    /// select output format used with HW accelerated decoding
    pub(crate) hwaccel_output_format: Option<String>,

    /// Input options for avformat_open_input.
    ///
    /// This field stores options that are passed to FFmpeg's `avformat_open_input()` function.
    /// These options can affect different layers of the input processing pipeline:
    ///
    /// **Format/Demuxer options:**
    /// - `probesize` - Maximum data to probe for format detection
    /// - `analyzeduration` - Duration to analyze for stream info
    /// - `fflags` - Format flags (e.g., "+genpts")
    ///
    /// **Protocol options:**
    /// - `user_agent` - HTTP User-Agent header
    /// - `timeout` - Network timeout in microseconds
    /// - `headers` - Custom HTTP headers
    ///
    /// **Device options:**
    /// - `framerate` - Input framerate (for avfoundation, dshow, etc.)
    /// - `video_size` - Input video resolution
    /// - `pixel_format` - Input pixel format
    ///
    /// **General input options:**
    /// - `thread_queue_size` - Input thread queue size
    /// - `re` - Read input at native frame rate
    ///
    /// These options allow fine-tuning of input behavior across different components
    /// of the FFmpeg input pipeline.
    pub(crate) input_opts: Option<HashMap<String, String>>,
}

impl Input {
    pub fn new(url: impl Into<String>) -> Self {
        url.into().into()
    }

    /// Creates a new `Input` instance with a custom read callback.
    ///
    /// This method initializes an `Input` object that uses a provided `read_callback` function
    /// to supply data to the input stream. This is particularly useful for custom data sources
    /// such as in-memory buffers, network streams, or other non-standard input mechanisms.
    ///
    /// ### Parameters:
    /// - `read_callback: fn(buf: &mut [u8]) -> i32`: A function pointer that fills the provided
    ///   mutable buffer with data and returns the number of bytes read.
    ///
    /// ### Return Value:
    /// - Returns a new `Input` instance configured with the specified `read_callback`.
    ///
    /// ### Behavior of `read_callback`:
    /// - **Positive Value**: Indicates the number of bytes successfully read.
    /// - **`ffmpeg_sys_next::AVERROR_EOF`**: Indicates the end of the stream. The library will stop requesting data.
    /// - **Negative Value**: Indicates an error occurred. For example:
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)`: Represents an input/output error.
    ///   - Other custom-defined error codes can also be returned to signal specific issues.
    ///
    /// ### Example:
    /// ```rust
    /// let input = Input::new_by_read_callback(move |buf| {
    ///     let data = b"example custom data source";
    ///     let len = data.len().min(buf.len());
    ///     buf[..len].copy_from_slice(&data[..len]);
    ///     len as i32 // Return the number of bytes written
    /// });
    /// ```
    pub fn new_by_read_callback<F>(read_callback: F) -> Self
    where
        F: FnMut(&mut [u8]) -> i32 + 'static,
    {
        (Box::new(read_callback) as Box<dyn FnMut(&mut [u8]) -> i32>).into()
    }

    /// Sets a custom seek callback for the input stream.
    ///
    /// This function assigns a user-defined function that handles seeking within the input stream.
    /// It is required when using custom data sources that support random access, such as files,
    /// memory-mapped buffers, or seekable network streams.
    ///
    /// **FFmpeg may invoke `seek_callback` from different threads.**
    /// If using a `File` as the data source, **wrap it in `Arc<Mutex<File>>`** to ensure
    /// thread-safe access across multiple threads.
    ///
    /// ### Parameters:
    /// - `seek_callback: FnMut(i64, i32) -> i64`: A function that handles seek operations.
    ///   - `offset: i64`: The target seek position in the stream.
    ///   - `whence: i32`: The seek mode, which determines how `offset` should be interpreted:
    ///     - `ffmpeg_sys_next::SEEK_SET` (0) - Seek to an absolute position.
    ///     - `ffmpeg_sys_next::SEEK_CUR` (1) - Seek relative to the current position.
    ///     - `ffmpeg_sys_next::SEEK_END` (2) - Seek relative to the end of the stream.
    ///     - `ffmpeg_sys_next::SEEK_HOLE` (3) - Find the next hole in a sparse file (Linux only).
    ///     - `ffmpeg_sys_next::SEEK_DATA` (4) - Find the next data block in a sparse file (Linux only).
    ///     - `ffmpeg_sys_next::AVSEEK_FLAG_BYTE` (2) - Seek using byte offset instead of timestamps.
    ///     - `ffmpeg_sys_next::AVSEEK_SIZE` (65536) - Query the total size of the stream.
    ///     - `ffmpeg_sys_next::AVSEEK_FORCE` (131072) - Force seeking, even if normally restricted.
    ///
    /// ### Return Value:
    /// - Returns `Self`, allowing for method chaining.
    ///
    /// ### Behavior of `seek_callback`:
    /// - **Positive Value**: The new offset position after seeking.
    /// - **Negative Value**: An error occurred, such as:
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE)`: Seek is not supported.
    ///   - `ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)`: General I/O error.
    ///
    /// ### Example (Thread-safe seek callback using `Arc<Mutex<File>>`):
    /// Since `FFmpeg` may call `read_callback` and `seek_callback` from different threads,
    /// **use `Arc<Mutex<File>>` to ensure safe concurrent access.**
    ///
    /// ```rust
    /// use std::fs::File;
    /// use std::io::{Read, Seek, SeekFrom};
    /// use std::sync::{Arc, Mutex};
    ///
    /// // ✅ Wrap the file in Arc<Mutex<>> for safe shared access
    /// let file = Arc::new(Mutex::new(File::open("test.mp4").expect("Failed to open file")));
    ///
    /// // ✅ Thread-safe read callback
    /// let read_callback = {
    ///     let file = Arc::clone(&file);
    ///     move |buf: &mut [u8]| -> i32 {
    ///         let mut file = file.lock().unwrap();
    ///         match file.read(buf) {
    ///             Ok(0) => {
    ///                 println!("Read EOF");
    ///                 ffmpeg_sys_next::AVERROR_EOF
    ///             }
    ///             Ok(bytes_read) => bytes_read as i32,
    ///             Err(e) => {
    ///                 println!("Read error: {}", e);
    ///                 ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO)
    ///             }
    ///         }
    ///     }
    /// };
    ///
    /// // ✅ Thread-safe seek callback
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
    ///         // ✅ Handle SEEK_HOLE and SEEK_DATA (Linux only)
    ///         #[cfg(target_os = "linux")]
    ///         if actual_whence == ffmpeg_sys_next::SEEK_HOLE {
    ///             println!("FFmpeg requested SEEK_HOLE, but Rust std::fs does not support it.");
    ///             return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE) as i64;
    ///         }
    ///         #[cfg(target_os = "linux")]
    ///         if actual_whence == ffmpeg_sys_next::SEEK_DATA {
    ///             println!("FFmpeg requested SEEK_DATA, but Rust std::fs does not support it.");
    ///             return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE) as i64;
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
    ///
    /// let input = Input::new_by_read_callback(read_callback).set_seek_callback(seek_callback);
    /// ```
    pub fn set_seek_callback<F>(mut self, seek_callback: F) -> Self
    where
        F: FnMut(i64, i32) -> i64 + 'static,
    {
        self.seek_callback = Some(Box::new(seek_callback) as Box<dyn FnMut(i64, i32) -> i64>);
        self
    }

    /// Replaces the entire frame-processing pipeline with a new sequence
    /// of transformations for **post-decoding** frames on this `Input`.
    ///
    /// This method clears any previously set pipelines and replaces them with the provided list.
    ///
    /// # Parameters
    /// * `frame_pipelines` - A list of [`FramePipeline`] instances defining the
    ///   transformations to apply to decoded frames.
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Input`, enabling method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("my_video.mp4")
    ///     .set_frame_pipelines(vec![
    ///         FramePipelineBuilder::new(AVMediaType::AVMEDIA_TYPE_VIDEO).filter("opengl", Box::new(my_filter)),
    ///         // Additional pipelines...
    ///     ]);
    /// ```
    pub fn set_frame_pipelines(mut self, frame_pipelines: Vec<impl Into<FramePipeline>>) -> Self {
        self.frame_pipelines = Some(frame_pipelines.into_iter().map(|frame_pipeline| frame_pipeline.into()).collect());
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
    /// * `Self` - Returns the modified `Input`, enabling method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("my_video.mp4")
    ///     .add_frame_pipeline(FramePipelineBuilder::new(AVMediaType::AVMEDIA_TYPE_VIDEO).filter("opengl", Box::new(my_filter)).build())
    ///     .add_frame_pipeline(FramePipelineBuilder::new(AVMediaType::AVMEDIA_TYPE_AUDIO).filter("my_custom_filter1", Box::new(...)).filter("my_custom_filter2", Box::new(...)).build());
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

    /// Sets the input format for the container or device.
    ///
    /// By default, if no format is specified,
    /// FFmpeg will attempt to detect the format automatically. However, certain
    /// use cases require specifying the format explicitly:
    /// - Using device-specific inputs (e.g., `avfoundation` on macOS, `dshow` on Windows).
    /// - Handling raw streams or formats that FFmpeg may not detect automatically.
    ///
    /// ### Parameters:
    /// - `format`: A string specifying the desired input format (e.g., `mp4`, `flv`, `avfoundation`).
    ///
    /// ### Return Value:
    /// - Returns the `Input` instance with the newly set format.
    pub fn set_format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    /// Sets the **video codec** to be used for decoding.
    ///
    /// By default, FFmpeg will automatically select an appropriate video codec
    /// based on the input format and available decoders. However, this method
    /// allows you to override that selection and force a specific codec.
    ///
    /// # Common Video Codecs:
    /// | Codec | Description |
    /// |-------|-------------|
    /// | `h264` | H.264 (AVC), widely supported and efficient |
    /// | `hevc` | H.265 (HEVC), better compression at higher complexity |
    /// | `vp9` | VP9, open-source alternative to H.265 |
    /// | `av1` | AV1, newer open-source codec with improved compression |
    /// | `mpeg4` | MPEG-4 Part 2, older but still used in some cases |
    ///
    /// # Arguments
    /// * `video_codec` - A string representing the desired video codec (e.g., `"h264"`, `"hevc"`).
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Input` struct, allowing for method chaining.
    ///
    /// # Example:
    /// ```rust
    /// let input = Input::from("video.mp4").set_video_codec("h264");
    /// ```
    pub fn set_video_codec(mut self, video_codec: impl Into<String>) -> Self {
        self.video_codec = Some(video_codec.into());
        self
    }

    /// Sets the **audio codec** to be used for decoding.
    ///
    /// By default, FFmpeg will automatically select an appropriate audio codec
    /// based on the input format and available decoders. However, this method
    /// allows you to specify a preferred codec.
    ///
    /// # Common Audio Codecs:
    /// | Codec | Description |
    /// |-------|-------------|
    /// | `aac` | AAC, commonly used for MP4 and streaming |
    /// | `mp3` | MP3, widely supported but lower efficiency |
    /// | `opus` | Opus, high-quality open-source codec |
    /// | `vorbis` | Vorbis, used in Ogg containers |
    /// | `flac` | FLAC, lossless audio format |
    ///
    /// # Arguments
    /// * `audio_codec` - A string representing the desired audio codec (e.g., `"aac"`, `"mp3"`).
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Input` struct, allowing for method chaining.
    ///
    /// # Example:
    /// ```rust
    /// let input = Input::from("audio.mp3").set_audio_codec("aac");
    /// ```
    pub fn set_audio_codec(mut self, audio_codec: impl Into<String>) -> Self {
        self.audio_codec = Some(audio_codec.into());
        self
    }

    /// Sets the **subtitle codec** to be used for decoding.
    ///
    /// By default, FFmpeg will automatically select an appropriate subtitle codec
    /// based on the input format and available decoders. This method lets you specify
    /// a particular subtitle codec.
    ///
    /// # Common Subtitle Codecs:
    /// | Codec | Description |
    /// |-------|-------------|
    /// | `ass` | Advanced SubStation Alpha (ASS) subtitles |
    /// | `srt` | SubRip Subtitle format (SRT) |
    /// | `mov_text` | Subtitles in MP4 containers |
    /// | `subrip` | Plain-text subtitle format |
    ///
    /// # Arguments
    /// * `subtitle_codec` - A string representing the desired subtitle codec (e.g., `"mov_text"`, `"ass"`, `"srt"`).
    ///
    /// # Returns
    /// * `Self` - Returns the modified `Input` struct, allowing for method chaining.
    ///
    /// # Example:
    /// ```rust
    /// let input = Input::from("movie.mkv").set_subtitle_codec("ass");
    /// ```
    pub fn set_subtitle_codec(mut self, subtitle_codec: impl Into<String>) -> Self {
        self.subtitle_codec = Some(subtitle_codec.into());
        self
    }

    /// Enables or disables **exit on error** behavior for the input.
    ///
    /// If set to `true`, FFmpeg will exit (stop processing) if it encounters any
    /// decoding or demuxing error on this input. If set to `false` (the default),
    /// FFmpeg may attempt to continue despite errors, skipping damaged portions.
    ///
    /// # Parameters
    /// - `exit_on_error`: `true` to stop on errors, `false` to keep going.
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("test.mp4")
    ///     .set_exit_on_error(true);
    /// ```
    pub fn set_exit_on_error(mut self, exit_on_error: bool) -> Self {
        self.exit_on_error = Some(exit_on_error);
        self
    }

    /// Sets a **read rate** for this input, controlling how quickly frames are read.
    ///
    /// - If set to `1.0`, frames are read at their native frame rate.
    /// - If set to another value (e.g., `0.5` or `2.0`), FFmpeg may attempt to read
    ///   slower or faster, simulating changes in real-time playback speed.
    ///
    /// # Parameters
    /// - `rate`: A floating-point value indicating the read rate multiplier.
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("video.mp4")
    ///     .set_readrate(0.5); // read at half speed
    /// ```
    pub fn set_readrate(mut self, rate: f32) -> Self {
        self.readrate = Some(rate);
        self
    }

    /// Sets the **start time** (in microseconds) from which to begin reading.
    ///
    /// FFmpeg will skip all data before this timestamp. This can be used to
    /// implement “input seeking” or to only process a portion of the input.
    ///
    /// # Parameters
    /// - `start_time_us`: The timestamp (in microseconds) at which to start reading.
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("long_clip.mp4")
    ///     .set_start_time_us(2_000_000); // Start at 2 seconds
    /// ```
    pub fn set_start_time_us(mut self, start_time_us: i64) -> Self {
        self.start_time_us = Some(start_time_us);
        self
    }

    /// Sets the **recording time** (in microseconds) for this input.
    ///
    /// FFmpeg will only read for the specified duration, ignoring data past this
    /// limit. This can be used to trim or limit how much of the input is processed.
    ///
    /// # Parameters
    /// - `recording_time_us`: The number of microseconds to read from the input.
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("long_clip.mp4")
    ///     .set_recording_time_us(5_000_000); // Only read 5 seconds
    /// ```
    pub fn set_recording_time_us(mut self, recording_time_us: i64) -> Self {
        self.recording_time_us = Some(recording_time_us);
        self
    }

    /// Sets a **stop time** (in microseconds) beyond which input data will be ignored.
    ///
    /// This is similar to [`set_recording_time_us`](Self::set_recording_time_us) but
    /// specifically references an absolute timestamp in the stream. Once this timestamp
    /// is reached, FFmpeg stops reading.
    ///
    /// # Parameters
    /// - `stop_time_us`: The absolute timestamp (in microseconds) at which to stop reading.
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("long_clip.mp4")
    ///     .set_stop_time_us(10_000_000); // Stop reading at 10 seconds
    /// ```
    pub fn set_stop_time_us(mut self, stop_time_us: i64) -> Self {
        self.stop_time_us = Some(stop_time_us);
        self
    }

    /// Sets the number of **loops** to perform on this input stream.
    ///
    /// If FFmpeg reaches the end of the input, it can loop back and start from the
    /// beginning, effectively repeating the content `stream_loop` times.
    /// A negative value may indicate infinite looping (depending on FFmpeg’s actual behavior).
    ///
    /// # Parameters
    /// - `count`: How many times to loop (e.g. `1` means one loop, `-1` might mean infinite).
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("music.mp3")
    ///     .set_stream_loop(2); // play the input 2 extra times
    /// ```
    pub fn set_stream_loop(mut self, count: i32) -> Self {
        self.stream_loop = Some(count);
        self
    }

    /// Specifies a **hardware acceleration** name for decoding this input.
    ///
    /// Common values might include `"cuda"`, `"vaapi"`, `"dxva2"`, `"videotoolbox"`, etc.
    /// Whether it works depends on your FFmpeg build and the hardware you have available.
    ///
    /// # Parameters
    /// - `hwaccel_name`: A string naming the hardware accel to use.
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("video.mp4")
    ///     .set_hwaccel("cuda");
    /// ```
    pub fn set_hwaccel(mut self, hwaccel_name: impl Into<String>) -> Self {
        self.hwaccel = Some(hwaccel_name.into());
        self
    }

    /// Selects a **hardware acceleration device** for decoding.
    ///
    /// For example, if you have multiple GPUs or want to specify a device node (like
    /// `"/dev/dri/renderD128"` on Linux for VAAPI), you can pass it here. This option
    /// must match the hardware accel you set via [`set_hwaccel`](Self::set_hwaccel) if
    /// you expect decoding to succeed.
    ///
    /// # Parameters
    /// - `device`: A string indicating the device path or identifier.
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("video.mp4")
    ///     .set_hwaccel("vaapi")
    ///     .set_hwaccel_device("/dev/dri/renderD128");
    /// ```
    pub fn set_hwaccel_device(mut self, device: impl Into<String>) -> Self {
        self.hwaccel_device = Some(device.into());
        self
    }

    /// Sets the **output pixel format** to be used with hardware-accelerated decoding.
    ///
    /// Certain hardware decoders can produce various output pixel formats. This option
    /// lets you specify which format (e.g., `"nv12"`, `"vaapi"`, etc.) is used during
    /// the decode process.
    /// Must be compatible with the chosen hardware accel and device.
    ///
    /// # Parameters
    /// - `format`: A string naming the desired output pixel format (e.g. `"nv12"`).
    ///
    /// # Returns
    /// * `Self` - allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let input = Input::from("video.mp4")
    ///     .set_hwaccel("cuda")
    ///     .set_hwaccel_output_format("cuda");
    /// ```
    pub fn set_hwaccel_output_format(mut self, format: impl Into<String>) -> Self {
        self.hwaccel_output_format = Some(format.into());
        self
    }

    /// Sets a single input option for avformat_open_input.
    ///
    /// This method configures options that will be passed to FFmpeg's `avformat_open_input()`
    /// function. The options can control behavior at different levels including format detection,
    /// protocol handling, device configuration, and general input processing.
    ///
    /// **Example Usage:**
    /// ```rust
    /// let input = Input::new("avfoundation:0")
    ///     .set_input_opt("framerate", "30")
    ///     .set_input_opt("probesize", "5000000");
    /// ```
    ///
    /// ### Parameters:
    /// - `key`: The option name (e.g., `"framerate"`, `"probesize"`, `"timeout"`).
    /// - `value`: The option value (e.g., `"30"`, `"5000000"`, `"10000000"`).
    ///
    /// ### Return Value:
    /// - Returns the modified `Input` instance for method chaining.
    pub fn set_input_opt(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        if let Some(ref mut opts) = self.input_opts {
            opts.insert(key.into(), value.into());
        } else {
            let mut opts = HashMap::new();
            opts.insert(key.into(), value.into());
            self.input_opts = Some(opts);
        }
        self
    }

    /// Sets multiple input options at once for avformat_open_input.
    ///
    /// This method allows setting multiple options in a single call, which will all be
    /// passed to FFmpeg's `avformat_open_input()` function. Each key-value pair will be
    /// inserted into the options map, overwriting any existing keys with the same name.
    ///
    /// **Example Usage:**
    /// ```rust
    /// let input = Input::new("http://example.com/stream.m3u8")
    ///     .set_input_opts(vec![
    ///         ("user_agent", "MyApp/1.0"),
    ///         ("timeout", "10000000"),
    ///         ("probesize", "5000000"),
    ///     ]);
    /// ```
    ///
    /// ### Parameters:
    /// - `opts`: A vector of key-value pairs representing input options.
    ///
    /// ### Return Value:
    /// - Returns the modified `Input` instance for method chaining.
    pub fn set_input_opts(mut self, opts: Vec<(impl Into<String>, impl Into<String>)>) -> Self {
        if let Some(ref mut input_opts) = self.input_opts {
            for (key, value) in opts {
                input_opts.insert(key.into(), value.into());
            }
        } else {
            let mut input_opts = HashMap::new();
            for (key, value) in opts {
                input_opts.insert(key.into(), value.into());
            }
            self.input_opts = Some(input_opts);
        }
        self
    }

}

impl From<Box<dyn FnMut(&mut [u8]) -> i32>> for Input {
    fn from(read_callback: Box<dyn FnMut(&mut [u8]) -> i32>) -> Self {
        Self {
            url: None,
            read_callback: Some(read_callback),
            seek_callback: None,
            frame_pipelines: None,
            format: None,
            video_codec: None,
            audio_codec: None,
            subtitle_codec: None,
            exit_on_error: None,
            readrate: None,
            start_time_us: None,
            recording_time_us: None,
            stop_time_us: None,
            stream_loop: None,
            hwaccel: None,
            hwaccel_device: None,
            hwaccel_output_format: None,
            input_opts: None,
        }
    }
}

impl From<String> for Input {
    fn from(url: String) -> Self {
        Self {
            url: Some(url),
            read_callback: None,
            seek_callback: None,
            frame_pipelines: None,
            format: None,
            video_codec: None,
            audio_codec: None,
            subtitle_codec: None,
            exit_on_error: None,
            readrate: None,
            start_time_us: None,
            recording_time_us: None,
            stop_time_us: None,
            stream_loop: None,
            hwaccel: None,
            hwaccel_device: None,
            hwaccel_output_format: None,
            input_opts: None,
        }
    }
}

impl From<&str> for Input {
    fn from(url: &str) -> Self {
        Self::from(String::from(url))
    }
}


#[cfg(test)]
mod tests {
    use crate::core::context::input::Input;

    #[test]
    fn test_new_by_read_callback() {
        let data_source = b"example custom data source".to_vec();
        let _input = Input::new_by_read_callback(move |buf| {
            let len = data_source.len().min(buf.len());
            buf[..len].copy_from_slice(&data_source[..len]);
            len as i32 // Return the number of bytes written
        });

        let data_source2 = b"example custom data source2".to_vec();
        let _input = Input::new_by_read_callback(move |buf2| {
            let len = data_source2.len().min(buf2.len());
            buf2[..len].copy_from_slice(&data_source2[..len]);
            len as i32 // Return the number of bytes written
        });
    }
}
