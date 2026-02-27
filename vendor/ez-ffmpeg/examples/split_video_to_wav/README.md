# ez-ffmpeg Example: Split Video to WAV Segments

This example demonstrates how to use the `ez-ffmpeg` library to extract audio from a video file and split it into multiple WAV files, each 10 seconds long.

## Code Explanation

- **Input Configuration**: The input video file is specified using `Input::from("input.mp4")`.
- **Output Configuration**: The output is set to split the audio into WAV files using `Output::from("output_%03d.wav")` with the `segment` format and a `segment_time` of 10 seconds.

## Notes

- Ensure the input video contains an audio track, or no WAV files will be generated.
- The output directory must have write permissions.

## Code Explanation
This example is equivalent to the following FFmpeg command:
```sh
ffmpeg -i input.mp4 -f segment -segment_time 10 output_%03d.wav