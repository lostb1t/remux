use std::ffi::CStr;
use std::ptr::null_mut;

pub mod frame_filter;
pub mod frame_pipeline;
pub mod frame_filter_context;
pub mod frame_pipeline_builder;

/// Retrieves a list of all filters recognized by FFmpeg.
///
/// This function iterates through all available filters in the FFmpeg filter system and collects their
/// names, descriptions, and associated flags into a vector of `FilterInfo` structs.
///
/// # Example
///
/// ```rust
/// let filters = get_filters();
/// for filter in filters {
///     println!("Filter: {} - {}", filter.name, filter.description);
/// }
/// ```
///
/// # Returns
/// A vector of `FilterInfo` structs representing all available filters.
pub fn get_filters() -> Vec<FilterInfo> {
    let mut filter_infos = Vec::new();

    // Initialize an opaque pointer for the filter iteration.
    let mut opaque = null_mut();
    loop {
        unsafe {
            // Retrieve the next filter descriptor.
            let filter = ffmpeg_sys_next::av_filter_iterate(&mut opaque);
            // If no more filters are available, break the loop.
            if filter.is_null() {
                break;
            }

            // Convert the filter's name and description from C strings to Rust strings.
            let name = std::str::from_utf8_unchecked(CStr::from_ptr((*filter).name).to_bytes());
            let description = std::str::from_utf8_unchecked(CStr::from_ptr((*filter).description).to_bytes());
            // Retrieve the filter's flags.
            let flags = ffmpeg_next::filter::Flags::from_bits_truncate((*filter).flags);

            // Push the filter's information into the vector.
            filter_infos.push(FilterInfo {
                name: name.to_string(),
                description: description.to_string(),
                flags,
            });
        }
    }

    // Return the vector containing all filter information.
    filter_infos
}

/// Represents metadata about a specific filter recognized by FFmpeg.
///
/// This struct consolidates information from the FFmpeg filter system into a single, user-friendly format.
/// It can be used to inspect properties of filters available in your FFmpeg build.
///
/// # Fields
/// * `name` - The name of the filter (e.g., `"scale"`, `"crop"`).
/// * `description` - A brief description of the filter's functionality.
#[derive(Clone, Debug)]
pub struct FilterInfo {
    /// The name of the filter.
    pub name: String,
    /// A brief description of the filter's functionality.
    pub description: String,
    /// The flags associated with the filter, indicating its capabilities and properties.
    pub flags: ffmpeg_next::filter::Flags,
}