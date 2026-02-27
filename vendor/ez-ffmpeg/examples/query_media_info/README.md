# ez-ffmpeg Example: Query Media Info

This example demonstrates how to query various media information from a media file, including its duration, format, metadata, and details about its streams (video, audio, etc.). It uses the `ez-ffmpeg` crate's container and stream info utilities.

## Features

- **Duration**: Get the duration of the media file in microseconds.
- **Format**: Get the format of the media file (e.g., mp4, mkv).
- **Metadata**: Retrieve metadata like title, author, and other information embedded in the media file.
- **Stream Information**: Retrieve detailed information about the media streams, such as video and audio streams.

## Example Overview

The following steps are performed in the example:

1. **Get Duration**: Retrieves the total duration of the media file in microseconds.
2. **Get Format**: Retrieves the format (e.g., `mp4`, `mkv`) of the media file.
3. **Get Metadata**: Extracts metadata (e.g., title, author) embedded in the media file.
4. **Get Stream Info**:
    - Retrieves the information about the first video stream in the media file.
    - Retrieves the information about the first audio stream.
    - Retrieves information about all streams (video, audio, subtitles, etc.) in the file.
