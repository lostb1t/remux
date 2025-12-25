pub struct FilterComplex {
    pub(crate) filter_descs: String,
    pub(crate) hw_device: Option<String>,

}

impl FilterComplex {

    /// Assigns a hardware device for this filter complex, enabling GPU-accelerated
    /// or device-specific filtering.
    ///
    /// # Parameters
    /// * `hw_device` - A `String` specifying the hardware device name or identifier
    ///   recognized by FFmpeg (e.g., `"cuda"`, `"vaapi"`, `"dxva2"`, etc.).
    ///
    /// # Returns
    /// A modified `FilterComplex` that includes the specified hardware device.
    ///
    /// # Example
    /// ```rust
    /// use ez_ffmpeg::core::context::filter_complex::FilterComplex;
    ///
    /// // Create a FilterComplex for scaling, then set a CUDA device
    /// let mut fc = FilterComplex::from("scale=1280:720")
    ///     .set_hw_device("cuda");
    /// ```
    pub fn set_hw_device(mut self, hw_device: impl Into<String>) -> Self {
        self.hw_device = Some(hw_device.into());
        self
    }
}

impl From<String> for FilterComplex {
    fn from(filter_descs: String) -> Self {
        Self { filter_descs, hw_device: None }
    }
}

impl From<&str> for FilterComplex {
    fn from(filter_descs: &str) -> Self {
        Self { filter_descs: filter_descs.to_string(), hw_device: None }
    }
}