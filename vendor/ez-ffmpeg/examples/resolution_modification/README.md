# ez-ffmpeg Example: Resolution Modification

This example demonstrates how to modify the resolution of a video using the `scale` filter in `ez-ffmpeg`.

## Code Explanation

- **Input Video:** `test.mp4`
- **Resolution Change:** The `scale` filter is used to resize the video to a width of `1280` while keeping the aspect ratio (height is automatically adjusted using `-1`).
- **Output Video:** The result is saved to `output.mp4`.

### Key FFmpeg Filter:
- `scale=1280:-1`: This scales the video to a width of `1280`, and the height is calculated automatically to preserve the aspect ratio.

## When to Use

- **Use this method** when you need to adjust the resolution of a video without manually calculating the new height to maintain the aspect ratio.
