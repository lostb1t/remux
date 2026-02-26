# ez-ffmpeg Example: Query Devices

This example demonstrates how to query and list the available input devices for audio and video. It utilizes the `get_input_audio_devices` and `get_input_video_devices` functions from the `ez-ffmpeg` crate to fetch all the supported input devices for capturing audio (e.g., microphones) and video (e.g., cameras).

## Overview

- **Input Video Devices**: These devices are used to capture video input, such as webcams, cameras, and other video capture devices.
- **Input Audio Devices**: These devices are used to capture audio input, such as microphones, sound cards, or other audio capture hardware.

The example will print the list of available audio and video input devices, helping you identify the devices available for video and audio streaming or recording.

## How It Works

1. **Get Video Devices**: The program queries the available video input devices by calling `get_input_video_devices()`.
2. **Get Audio Devices**: It then queries the available audio input devices by calling `get_input_audio_devices()`.
3. **Print Results**: The program prints out the names of the video and audio devices to the console.

### Example Output:

```plaintext
camera: /dev/video0
camera: /dev/video1
microphone: hw:0,0
microphone: hw:1,0
```

This will list all the video and audio input devices detected by your FFmpeg build on your system. The exact names and paths will vary depending on the hardware available.

### Useful for:

- Identifying available audio/video capture devices.
- Streaming or recording audio and video from physical devices.
- Troubleshooting device availability issues.