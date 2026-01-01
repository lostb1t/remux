# ez-ffmpeg Example: Create a Still Video from an Image

This example demonstrates how to use `ez-ffmpeg` to generate a still video from a single image, such as a logo or poster, by looping the image for a fixed duration.

## Code Explanation

- **Input Image:** `logo.jpg`
- **Looping:** The input is looped using the `loop=1` format option.
- **Video Filter:**
  - The image is scaled to `1280x720` using the `scale` filter.
- **Output Video:**
  - Duration: 10 seconds
  - Output format: `mp4`
  - Output file: `output.mp4`

### Key FFmpeg Options:

- `loop=1`: Tells FFmpeg to loop the image input.
- `-t 10`: Specifies the total duration of the output video.
- `-vf scale=1280:720`: Scales the input image to 1280x720 resolution.

## Equivalent FFmpeg Command

```bash
ffmpeg -loop 1 -i logo.jpg -t 10 -vf scale=1280:720 output.mp4
