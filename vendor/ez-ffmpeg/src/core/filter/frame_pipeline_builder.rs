use crate::core::filter::frame_filter::FrameFilter;
use crate::filter::frame_pipeline::FramePipeline;
use ffmpeg_sys_next::AVMediaType;

/// A builder for constructing [`FramePipeline`] instances.
///
/// # Example
/// ```rust
/// let pipeline = FramePipelineBuilder::new(AVMediaType::AVMEDIA_TYPE_VIDEO)
///     .filter("opengl", Box::new(OpenGLFrameFilter::new())) // Add an OpenGL filter
///     .build();
/// ```
pub struct FramePipelineBuilder {
    /// The index of the stream being processed.
    ///
    /// This value corresponds to the `stream_index` of an input or output stream in FFmpeg.
    /// It is used to identify which stream the pipeline applies to.
    pub(crate) stream_index: Option<usize>,

    /// The type of media this pipeline is processing.
    ///
    /// This field determines whether the pipeline is handling **video**, **audio**, or **subtitle** data.
    /// It is represented using `AVMediaType`, which can take values such as:
    /// - `AVMEDIA_TYPE_VIDEO` for video frames.
    /// - `AVMEDIA_TYPE_AUDIO` for audio frames.
    /// - `AVMEDIA_TYPE_SUBTITLE` for subtitle processing.
    pub(crate) media_type: AVMediaType,

    /// A list of filters to be applied in sequence.
    ///
    /// Each filter is represented by a tuple containing:
    /// - A `String` name that identifies the filter.
    /// - A `Box<dyn FrameFilter>` that holds the filter implementation.
    ///
    /// These filters will be applied to the media frames in the order they are added.
    pub(crate) filters: Vec<(String, Box<dyn FrameFilter>)>,
}

impl FramePipelineBuilder {
    /// Creates a new `FramePipelineBuilder` instance for a specific media type.
    ///
    /// This initializes a builder that can be configured with stream index, link label, and filters.
    ///
    /// # Arguments
    /// - `media_type` - The type of media being processed (`AVMEDIA_TYPE_VIDEO`, `AVMEDIA_TYPE_AUDIO`, etc.).
    ///
    /// # Returns
    /// A new `FramePipelineBuilder` instance with the given `media_type`.
    ///
    /// # Example
    /// ```rust
    /// let builder = FramePipelineBuilder::new(AVMEDIA_TYPE_VIDEO);
    /// ```
    pub fn new(media_type: AVMediaType) -> Self {
        Self {
            stream_index: None,
            media_type,
            filters: vec![],
        }
    }

    /// Sets the stream index for this pipeline.
    ///
    /// The stream index is used to specify which input or output stream this pipeline applies to.
    /// This should match the `stream_index` of the corresponding FFmpeg stream.
    ///
    /// # Arguments
    /// - `stream_index` - The index of the media stream in the input or output file.
    ///
    /// # Returns
    /// The modified `FramePipelineBuilder` instance, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let builder = FramePipelineBuilder::new(AVMEDIA_TYPE_VIDEO)
    ///     .set_stream_index(0);
    /// ```
    pub fn set_stream_index(mut self, stream_index: usize) -> Self {
        self.stream_index = Some(stream_index);
        self
    }

    /// Adds a filter to the pipeline.
    ///
    /// This method registers a filter to be applied to the media frames in the pipeline.
    /// Filters are applied in the order they are added.
    ///
    /// # Arguments
    /// - `name` - The name of the filter, which serves as an identifier.
    /// - `filter` - A boxed instance of a filter that implements `FrameFilter`.
    ///
    /// # Returns
    /// The modified `FramePipelineBuilder` instance, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let filter = Box::new(MyCustomFilter {});
    /// let builder = FramePipelineBuilder::new(AVMEDIA_TYPE_VIDEO)
    ///     .filter("scale", filter);
    /// ```
    pub fn filter(mut self, name: &str, filter: Box<dyn FrameFilter>) -> Self {
        assert_eq!(self.media_type, filter.media_type());
        self.filters.push((name.to_string(), filter));
        self
    }

    /// Builds the `FramePipeline` instance.
    ///
    /// # Arguments
    /// - `stream_index`: The final determined stream index.
    ///
    /// # Returns
    /// A reference-counted `FramePipeline` instance.
    ///
    /// # Example
    /// ```rust
    /// let pipeline = builder.build();
    /// ```
    ///
    pub fn build(self) -> FramePipeline {
        let mut frame_pipeline = FramePipeline::new(self.media_type, self.stream_index);

        for (name, filter) in self.filters.into_iter() {
            frame_pipeline.add_filter(name, filter);
        }

        frame_pipeline
    }
}

impl From<AVMediaType> for FramePipelineBuilder {
    fn from(media_type: AVMediaType) -> Self {
        Self::new(media_type)
    }
}
