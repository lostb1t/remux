# ez-ffmpeg Example: Thumbnail Extraction

This example demonstrates how to extract thumbnail images from a video using `ez-ffmpeg`. Two examples are provided: one for generating a single thumbnail and another for generating multiple thumbnails.

## Examples

### Example 1: Single Thumbnail Extraction

- **Input Video:** `test.mp4`
- **Thumbnail Extraction:**
  - The `scale` filter resizes the video to a width of `160`, while automatically adjusting the height to maintain the aspect ratio (using `-1`).
  - The expression `min(160, iw)` ensures that the width does not exceed 160 pixels.
- **Output:** The first frame of the video is extracted and saved as `output.jpg`.
- **FFmpeg Option:**
  - `set_max_video_frames(1)`: Limits the output to 1 video frame, effectively extracting only a single frame (the thumbnail).

### Example 2: Multiple Thumbnails Extraction

- **Input Video:** `test.mp4`
- **Thumbnail Extraction:**
  - The same `scale` filter is applied to resize the video.
- **Output:** Multiple frames are extracted and saved using a `%03d` pattern in the filename (e.g., `output_001.jpg`, `output_002.jpg`, etc.).
- **FFmpeg Option:**
  - `set_max_video_frames(5)`: Limits the output to 5 video frames, generating multiple thumbnails.

## When to Use

- **Single Thumbnail Extraction:** Use this method when you need one preview image from the video.
- **Multiple Thumbnails Extraction:** Use this method when you want several preview images at different intervals in the video.

