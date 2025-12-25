# ez-ffmpeg Example: Audio and Video Stream Extraction

This example demonstrates how to extract specific streams (audio or video) from an input media file and save them to separate output files using the `ez-ffmpeg` library.

## Key Concepts

In this example, we extract the audio and video streams from a media file separately. We use the `add_stream_map` method to specify which stream to extract:

- **`0:v`**: Refers to the video stream of the first input file (`test.mp4` in this case).
- **`0:a`**: Refers to the audio stream of the first input file.

### Code Breakdown

1. **Extract Video Stream**: The first example extracts the video stream (`0:v`) from the input file and saves it to `output.mp4`.
2. **Extract Audio Stream**: The second example extracts the audio stream (`0:a`) from the input file and saves it to `output.aac`.

## Usage

This example is useful when you need to extract only specific streams from a media file, such as saving the audio to an audio file (e.g., `.aac` or `.mp3`) or saving the video to a separate video file (e.g., `.mp4`).
