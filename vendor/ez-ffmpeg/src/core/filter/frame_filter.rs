use crate::core::filter::frame_filter_context::FrameFilterContext;
use ffmpeg_sys_next::AVMediaType;
use ffmpeg_next::Frame;

pub trait FrameFilter: Send {
    /// Returns the media type this filter operates on.
    ///
    /// This is used to determine whether the filter is compatible with a specific media type
    /// (e.g., video, audio, etc.). Each filter should define the media type it supports.
    fn media_type(&self) -> AVMediaType;

    /// Initializes the filter.
    ///
    /// This method is called once when the filter is added to the pipeline and prepares
    /// the filter for processing. The `ctx` provides access to the `FrameFilterContext`,
    /// which includes the filter's name and its associated pipeline. The pipeline allows
    /// filters to set or retrieve attributes, enabling the sharing of information across
    /// filters dynamically.
    ///
    /// # Parameters
    /// - `ctx`: The context that provides metadata and dynamic modification capabilities.
    ///
    /// # Returns
    /// - `Ok(())` if initialization succeeds.
    /// - `Err(String)` if there is an error during initialization.
    fn init(&mut self, ctx: &FrameFilterContext) -> Result<(), String> {
        log::debug!("Initializing filter:{}", ctx.name());
        Ok(())
    }

    /// Processes a single frame through the filter.
    ///
    /// This method applies the filter's logic to a given frame and optionally produces
    /// a new frame. The `ctx` provides access to the filter's metadata and allows dynamic
    /// pipeline modifications if needed. The pipeline allows filters to set or retrieve
    /// attributes, enabling the sharing of information across filters during processing.
    ///
    /// # Parameters
    /// - `frame`: The input frame to be processed.
    /// - `ctx`: The context that provides metadata and dynamic modification capabilities.
    ///
    /// # Returns
    /// - `Ok(Some(frame))` if the filter produces a new frame.
    /// - `Ok(None)` if no frame is produced.
    /// - `Err(String)` if there is an error during processing.
    fn filter_frame(
        &mut self,
        _frame: Frame,
        _ctx: &FrameFilterContext,
    ) -> Result<Option<Frame>, String> {
        Ok(None)
    }

    /// Requests a frame from the filter.
    ///
    /// This method is used to pull frames from the filter when needed. For example,
    /// some filters might generate frames independently of input frames. The context
    /// provides access to the pipeline, allowing filters to set or retrieve attributes
    /// dynamically during frame requests.
    ///
    /// # Parameters
    /// - `ctx`: The context that provides metadata and dynamic modification capabilities.
    ///
    /// # Returns
    /// - `Ok(Some(frame))` if the filter produces a frame.
    /// - `Ok(None)` if no frame is produced.
    /// - `Err(String)` if there is an error during processing.
    fn request_frame(&mut self, _ctx: &FrameFilterContext) -> Result<Option<Frame>, String> {
        Ok(None)
    }

    /// Cleans up the filter.
    ///
    /// This method is called when the filter is removed from the pipeline or when
    /// the pipeline is terminated. It allows the filter to release resources or perform
    /// any necessary cleanup. The context provides access to the pipeline, allowing
    /// filters to set or retrieve attributes for final updates or cleanup of shared state.
    ///
    /// # Parameters
    /// - `ctx`: The context that provides metadata and dynamic modification capabilities.
    fn uninit(&mut self, ctx: &FrameFilterContext) {
        log::debug!("Uninitialized filter:{}", ctx.name());
    }
}



pub struct NoopFilter {
    media_type: AVMediaType
}

impl NoopFilter {
    pub fn new(media_type: AVMediaType) -> Self {
        Self { media_type }
    }
}

impl FrameFilter for NoopFilter {
    fn media_type(&self) -> AVMediaType {
        self.media_type
    }

    fn filter_frame(&mut self, frame: Frame, _ctx: &FrameFilterContext) -> Result<Option<Frame>, String> {
        Ok(Some(frame))
    }
}