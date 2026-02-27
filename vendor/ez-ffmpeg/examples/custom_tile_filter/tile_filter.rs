use ez_ffmpeg::filter::frame_filter::FrameFilter;
use ez_ffmpeg::filter::frame_filter_context::FrameFilterContext;
use ffmpeg_next::Frame;
use ffmpeg_sys_next::{av_frame_copy_props, av_frame_get_buffer, AVMediaType, AVPixelFormat};
use ez_ffmpeg::util::ffmpeg_utils::av_err2str;

/// Tile2x2Filter: A custom video filter that creates a 2x2 tiled output.
///
/// This filter takes an input video frame and creates an output frame that is
/// twice the size in both dimensions, containing 4 copies of the input frame
/// arranged in a 2x2 grid pattern.
///
/// **Strict Requirements:**
/// - Only supports YUV420P pixel format
/// - Input width and height must be even numbers
/// - Frame must be valid and non-empty
///
/// **Example:**
/// Input: 320x240 → Output: 640x480
/// ```
/// ┌────┬────┐
/// │ A  │ A  │  Where A is the original frame
/// ├────┼────┤
/// │ A  │ A  │
/// └────┴────┘
/// ```
pub struct Tile2x2Filter;

impl Tile2x2Filter {
    /// Creates a new Tile2x2Filter instance.
    pub fn new() -> Self {
        Self
    }

    /// Validates that the frame meets all requirements.
    ///
    /// # Arguments
    /// * `frame` - The input frame to validate
    ///
    /// # Returns
    /// * `Ok(())` if all validations pass
    /// * `Err(String)` with detailed error message if validation fails
    fn validate_frame(&self, frame: &Frame) -> Result<(), String> {
        unsafe {
            let width = (*frame.as_ptr()).width;
            let height = (*frame.as_ptr()).height;
            let format: AVPixelFormat = std::mem::transmute((*frame.as_ptr()).format);

            // Validation 1: Pixel format must be YUV420P
            if format != AVPixelFormat::AV_PIX_FMT_YUV420P {
                return Err(format!(
                    "Unsupported pixel format: {:?}. This filter only supports YUV420P format. \
                     Hint: Add .filter_desc(\"format=yuv420p\") before this filter in your pipeline.",
                    format
                ));
            }

            // Validation 2: Width and height must be even numbers
            // This is required because YUV420P's U and V planes are half the size of Y plane
            if width % 2 != 0 || height % 2 != 0 {
                return Err(format!(
                    "Width and height must be even numbers for YUV420P format. \
                     Got: {}x{}. Please ensure input dimensions are even.",
                    width, height
                ));
            }

            // Validation 3: Dimensions must be positive
            if width <= 0 || height <= 0 {
                return Err(format!("Invalid dimensions: {}x{}", width, height));
            }
        }

        Ok(())
    }

    /// Copies a plane of the input frame to 4 positions in the output frame.
    ///
    /// # Arguments
    /// * `src_data` - Pointer to source plane data
    /// * `src_linesize` - Source plane linesize (bytes per row, may include padding)
    /// * `dst_data` - Pointer to destination plane data
    /// * `dst_linesize` - Destination plane linesize
    /// * `plane_width` - Width of the plane in bytes
    /// * `plane_height` - Height of the plane in rows
    ///
    /// # Safety
    /// This function uses unsafe pointer operations. Caller must ensure:
    /// - All pointers are valid and properly aligned
    /// - Buffer sizes are sufficient for the copy operations
    unsafe fn copy_plane_to_tiles(
        &self,
        src_data: *const u8,
        src_linesize: i32,
        dst_data: *mut u8,
        dst_linesize: i32,
        plane_width: usize,
        plane_height: usize,
    ) {
        // Copy to 4 positions in a 2x2 grid
        for tile_y in 0..2 {
            for tile_x in 0..2 {
                let dst_offset_x = tile_x * plane_width;
                let dst_offset_y = tile_y * plane_height;

                // Copy row by row to handle linesize (which may include padding)
                for row in 0..plane_height {
                    let src_row_ptr = src_data.add(row * src_linesize as usize);
                    let dst_row_ptr = dst_data.add(
                        (dst_offset_y + row) * dst_linesize as usize + dst_offset_x
                    );

                    // Use copy_nonoverlapping for efficient memory copy
                    std::ptr::copy_nonoverlapping(src_row_ptr, dst_row_ptr, plane_width);
                }
            }
        }
    }
}

impl FrameFilter for Tile2x2Filter {
    fn media_type(&self) -> AVMediaType {
        AVMediaType::AVMEDIA_TYPE_VIDEO
    }

    fn init(&mut self, ctx: &FrameFilterContext) -> Result<(), String> {
        log::info!("Initializing Tile2x2Filter: {}", ctx.name());
        log::info!("This filter creates a 2x2 tiled output (doubles width and height)");
        log::info!("Requirements: YUV420P format, even dimensions");
        Ok(())
    }

    fn filter_frame(
        &mut self,
        frame: Frame,
        _ctx: &FrameFilterContext,
    ) -> Result<Option<Frame>, String> {
        unsafe {
            // If frame is null or empty, return it as-is (this can happen at end of stream)
            if frame.as_ptr().is_null() || frame.is_empty() {
                return Ok(Some(frame));
            }
        }

        // Validate the input frame
        self.validate_frame(&frame)?;

        unsafe {
            let input_width = (*frame.as_ptr()).width;
            let input_height = (*frame.as_ptr()).height;

            log::debug!(
                "Processing frame: {}x{} → {}x{}",
                input_width,
                input_height,
                input_width * 2,
                input_height * 2
            );

            // Create output frame with doubled dimensions
            let mut output_frame = Frame::empty();
            if output_frame.as_ptr().is_null() {
                return Err("Failed to create output frame: Out of memory".to_string());
            }

            // Set output frame properties
            (*output_frame.as_mut_ptr()).width = input_width * 2;
            (*output_frame.as_mut_ptr()).height = input_height * 2;
            (*output_frame.as_mut_ptr()).format = (*frame.as_ptr()).format;

            // Allocate buffer for output frame
            let ret = av_frame_get_buffer(output_frame.as_mut_ptr(), 0);
            if ret < 0 {
                return Err(format!(
                    "Failed to allocate buffer for output frame: {}",
                    av_err2str(ret)
                ));
            }

            // Copy frame properties (pts, time_base, color info, etc.)
            let ret = av_frame_copy_props(output_frame.as_mut_ptr(), frame.as_ptr());
            if ret < 0 {
                return Err(format!(
                    "Failed to copy frame properties: {}",
                    av_err2str(ret)
                ));
            }

            // Process each plane separately
            // YUV420P has 3 planes:
            // - Plane 0 (Y): Full resolution (width x height)
            // - Plane 1 (U): Quarter resolution (width/2 x height/2)
            // - Plane 2 (V): Quarter resolution (width/2 x height/2)

            // Copy Y plane (full resolution)
            self.copy_plane_to_tiles(
                (*frame.as_ptr()).data[0],
                (*frame.as_ptr()).linesize[0],
                (*output_frame.as_mut_ptr()).data[0],
                (*output_frame.as_mut_ptr()).linesize[0],
                input_width as usize,        // Y plane width in bytes
                input_height as usize,       // Y plane height
            );

            // Copy U plane (half resolution)
            self.copy_plane_to_tiles(
                (*frame.as_ptr()).data[1],
                (*frame.as_ptr()).linesize[1],
                (*output_frame.as_mut_ptr()).data[1],
                (*output_frame.as_mut_ptr()).linesize[1],
                (input_width / 2) as usize,  // U plane width in bytes
                (input_height / 2) as usize, // U plane height
            );

            // Copy V plane (half resolution)
            self.copy_plane_to_tiles(
                (*frame.as_ptr()).data[2],
                (*frame.as_ptr()).linesize[2],
                (*output_frame.as_mut_ptr()).data[2],
                (*output_frame.as_mut_ptr()).linesize[2],
                (input_width / 2) as usize,  // V plane width in bytes
                (input_height / 2) as usize, // V plane height
            );

            Ok(Some(output_frame))
        }
    }

    fn uninit(&mut self, ctx: &FrameFilterContext) {
        log::info!("Uninitializing Tile2x2Filter: {}", ctx.name());
    }
}
