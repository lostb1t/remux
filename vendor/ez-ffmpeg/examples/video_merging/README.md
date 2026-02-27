# ez-ffmpeg Example: Video Merging

This example demonstrates how to merge multiple video files into a single output file using the `ez-ffmpeg` library.

## Key Concepts

In this example, we use the `concat` filter to merge three video files. The `concat` filter is used to join multiple input video/audio streams into a single stream.

### Filter Parameters:

- **`n=3`**: This parameter defines the number of input files to be merged. In this case, we have 3 input files.
- **`v=1`**: This indicates that there is 1 video stream in the output.
- **`a=1`**: This indicates that there is 1 audio stream in the output.

### Code Breakdown

1. **Input files**: Three video files (`test.mp4`) are specified. These files will be concatenated into a single output.
2. **Filter Description**: The `concat=n=3:v=1:a=1` filter concatenates the three input files into one video and audio stream.
3. **Output file**: The result is written to `output.mp4`.

## Usage

This example is useful when you want to merge multiple video files into a single file. You can adjust the number of inputs or the filter settings to suit your needs (e.g., merging only audio or video streams).
