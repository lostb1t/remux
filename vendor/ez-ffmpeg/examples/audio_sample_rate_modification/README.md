# ez-ffmpeg Example: Audio Sample Rate Modification

This example demonstrates how to modify the audio sample rate of a video file and convert it to an audio-only WAV file using `ez-ffmpeg`.

## Code Explanation

- **Input Video:** `test.mp4`
- **Audio Configuration:**
    - **Audio Channels:** The output audio is configured to have 1 channel (mono) by using the `ac` option.
    - **Audio Sample Rate:** The sample rate is set to `16000` Hz using the `ar` option.
    - **Audio Codec:** The codec is set to `pcm_s16le`, which is 16-bit signed little-endian PCM audio format.

- **Output File:** The resulting audio is saved as `output.wav`.

### Key FFmpeg Options:

- `ac=1`: Set audio channels to 1 (mono).
- `ar=16000`: Set audio sample rate to 16000 Hz.
- `pcm_s16le`: Specifies the audio codec as 16-bit signed little-endian PCM.

## When to Use

- **Use this method** when you need to modify the audio sample rate and format of a video, converting it to a WAV file or other audio-only formats.
