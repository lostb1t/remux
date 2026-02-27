# ez-ffmpeg Example: Camera and Microphone Capture

This example demonstrates how to capture video from a camera and audio from a microphone using `ez-ffmpeg`, saving the output to "capture.mp4". The code is cross-platform, supporting macOS, Windows, and Linux, with platform-specific input configurations.

## Purpose
This example is useful for:
- Recording video calls or screencasts with audio.
- Testing camera and microphone setups.
- Understanding how to handle platform-specific input devices in `ez-ffmpeg`.

## Usage
1. Ensure FFmpeg is installed and accessible on your system (refer to the top-level README for installation details).
2. Run the example using `cargo run`. The program will capture video and audio for 10 seconds and then stop automatically.
3. The output file "capture.mp4" will be saved in the current directory.

## Platform-Specific Notes
- **macOS**:
    - Uses `avfoundation` format.
    - Input can be specified by device index (e.g., "0:0" for video device 0 and audio device 0) or by device name (e.g., "FaceTime HD Camera:Built-in Microphone").
    - To list available devices, use `ez_ffmpeg::device::get_input_video_devices()` and `ez_ffmpeg::device::get_input_audio_devices()`.

- **Windows**:
    - Uses `dshow` format.
    - Input is specified by device names, e.g., "video=Integrated Webcam:audio=Microphone".
    - Device names can be found using `ez_ffmpeg::device::get_input_video_devices()` and `ez_ffmpeg::device::get_input_audio_devices()`.

- **Linux**:
    - Uses `v4l2` for video and `alsa` for audio.
    - Video input is specified by device path (e.g., "/dev/video0"), and audio by ALSA device (e.g., "hw:0").
    - List video devices with `ez_ffmpeg::device::get_input_video_devices()` and audio devices with `ez_ffmpeg::device::get_input_audio_devices()`.

## Code Explanation
- The code uses platform-specific macros (`#[cfg(target_os = "os_name")]`) to handle different input configurations for macOS, Windows, and Linux.
- For each platform, the input is configured accordingly, and the output is set to "capture.mp4" with H.264 video and AAC audio.
- The capture runs for 10 seconds before being aborted programmatically using `scheduler.abort()`.

## Potential Issues
- **Device Availability**: Ensure the specified devices are available and not in use by other applications.
- **Permissions**: On some platforms, you may need to grant permissions to access the camera and microphone.
- **Device Names/Indices**: If the default device names or indices do not match your system, adjust them based on the output from the device listing functions.
- **FFmpeg Support**: Ensure your FFmpeg installation supports the required input formats (e.g., `avfoundation`, `dshow`, `v4l2`, `alsa`).

## Additional Notes
- The code includes commented-out sections showing alternative ways to specify inputs (e.g., using device names on macOS).
- The `ez_ffmpeg::device` module provides functions to list available input devices, which can be used to dynamically select devices.