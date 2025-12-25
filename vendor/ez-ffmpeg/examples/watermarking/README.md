# # ez-ffmpeg Example: Watermarking

This example demonstrates how to overlay a watermark image (such as a logo) onto a video file using **FFmpeg** through the `ez-ffmpeg` crate. The watermark is applied in the following way:

1. **Input Video and Watermark**:  
   The process takes two inputs: a video file (e.g., `test.mp4`) and an image file (e.g., `logo.jpg`) to be used as the watermark.

2. **Watermarking Process**:  
   The watermark image is scaled, formatted with transparency, and its alpha channel is adjusted to create a translucent effect. This watermark is then overlaid onto the video at a specified position (e.g., 10 pixels from the top-left corner).

3. **Output**:  
   The final result is saved as a new video file with the watermark applied.
