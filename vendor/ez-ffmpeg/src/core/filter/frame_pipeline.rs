use crate::core::filter::frame_filter::FrameFilter;
use crate::core::filter::frame_filter_context::FrameFilterContext;
use ffmpeg_sys_next::AVMediaType;
use std::any::Any;
use std::collections::HashMap;
use crate::filter::frame_pipeline_builder::FramePipelineBuilder;

/// Internally, we store each filter along with its name in a holder.
pub(crate) struct FilterHolder {
    name: String,
    filter: Box<dyn FrameFilter>,
}

/// A pipeline that processes frames by passing them through all filters in order.
/// It also stores an attribute map that filters can access/modify via `FrameFilterContext`.
pub struct FramePipeline {
    pub(crate) media_type: AVMediaType,
    pub(crate) stream_index: Option<usize>,

    pub(crate) filters: Vec<FilterHolder>,

    // Shared data among all filters
    attribute_map: HashMap<String, Box<dyn Any + Send>>,
}

impl FramePipeline {
    /// Creates a new pipeline for a given media type.
    /// All filters must match this type.
    pub fn new(media_type: AVMediaType, stream_index: Option<usize>) -> Self {
        Self {
            media_type,
            stream_index,
            filters: Vec::new(),
            attribute_map: HashMap::new(),
        }
    }

    /// Adds a filter to the pipeline. No dynamic removal is provided in this simplified approach.
    pub fn add_filter(&mut self, name: impl Into<String>, filter: Box<dyn FrameFilter>) {
        assert_eq!(self.media_type, filter.media_type());
        self.filters.push(FilterHolder {
            name: name.into(),
            filter,
        });
    }

    /// Allows external code to directly set an attribute. (Optional convenience)
    pub fn set_attribute<T: 'static + std::marker::Send>(&mut self, key: impl Into<String>, value: T) {
        self.attribute_map.insert(key.into(), Box::new(value));
    }

    /// Allows external code to retrieve an attribute by key.
    pub fn get_attribute<T: 'static>(&self, key: &str) -> Option<&T> {
        self.attribute_map
            .get(key)
            .and_then(|v| v.downcast_ref::<T>())
    }

    /// Initializes all filters in order.
    pub(crate) fn init_filters(&mut self) -> Result<(), String> {
        for holder in &mut self.filters {
            let mut ctx = FrameFilterContext::new(&holder.name, &mut self.attribute_map);
            holder.filter.init(&mut ctx)?;
        }
        Ok(())
    }

    /// Calls `uninit` on all filters (in the same order).
    /// (You can reverse the order if needed, but typically it's not strict.)
    pub(crate) fn uninit_filters(&mut self) {
        for holder in &mut self.filters {
            let mut ctx = FrameFilterContext::new(&holder.name, &mut self.attribute_map);
            holder.filter.uninit(&mut ctx);
        }
    }

    /// Pushes a frame through each filter in order. If any filter returns `None`,
    /// the frame is dropped. Otherwise, the final `Some(frame)` is returned.
    pub(crate) fn run_filters(&mut self, mut frame: ffmpeg_next::Frame) -> Result<Option<ffmpeg_next::Frame>, String> {
        for holder in &mut self.filters {
            let mut ctx = FrameFilterContext::new(&holder.name, &mut self.attribute_map);
            match holder.filter.filter_frame(frame, &mut ctx)? {
                Some(f) => {
                    frame = f;
                }
                None => {
                    return Ok(None);
                }
            }
        }
        Ok(Some(frame))
    }

    pub(crate) fn filter_len(&self) -> usize {
        self.filters.len()
    }

    pub(crate) fn request_frame(&mut self, index: usize) -> Result<Option<ffmpeg_next::Frame>, String> {
        assert!(index < self.filters.len());
        let holder = &mut self.filters[index];
        let mut ctx = FrameFilterContext::new(&holder.name, &mut self.attribute_map);
        holder.filter.request_frame(&mut ctx)
    }

    /// Passes the given `frame` through the filters starting at `start_index`.
    ///
    /// For example, if `start_index` is 2, we will call `filter_frame` on the 2nd filter,
    /// then the 3rd, and so on, up to the last filter in the pipeline. If any filter
    /// returns `None`, the frame is discarded and no further filters are called.
    ///
    /// # Parameters
    /// - `start_index`: The zero-based index of the filter from which to begin processing.
    /// - `frame`: The FFmpeg `Frame` to be processed.
    ///
    /// # Returns
    /// - `Ok(Some(frame))` if the frame is successfully processed by all remaining filters.
    /// - `Ok(None)` if any filter discards the frame by returning `None`.
    /// - `Err(String)` if an error occurs in any filter.
    pub(crate) fn run_filters_from(
        &mut self,
        start_index: usize,
        mut frame: ffmpeg_next::Frame,
    ) -> Result<Option<ffmpeg_next::Frame>, String> {
        // If start_index is out of bounds, we can either return an error
        // or treat it as "no filters to run." Here we choose to check bounds explicitly.
        if start_index >= self.filters.len() {
            // No filters to run, so the frame passes through unchanged.
            return Ok(Some(frame));
        }

        // Iterate from `start_index` to the end of `self.filters`.
        for i in start_index..self.filters.len() {
            let holder = &mut self.filters[i];

            // Build a temporary context, giving the filter its name and the attribute map.
            let mut ctx = FrameFilterContext::new(&holder.name, &mut self.attribute_map);

            // Call `filter_frame` on the filter. If `None`, discard the frame and stop.
            match holder.filter.filter_frame(frame, &mut ctx)? {
                Some(f) => {
                    frame = f; // Continue to the next filter
                }
                None => {
                    // The filter has dropped this frame
                    return Ok(None);
                }
            }
        }

        // If we reach here, all remaining filters have produced Some(frame).
        Ok(Some(frame))
    }
}

impl From<FramePipelineBuilder> for FramePipeline {
    fn from(pipeline: FramePipelineBuilder) -> Self {
        pipeline.build()
    }
}