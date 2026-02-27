# ez-ffmpeg Example: Hardware Acceleration

This example demonstrates how to use hardware acceleration for video decoding and encoding on different platforms (macOS, Windows, Nvidia, Intel, and AMD) using the `ez-ffmpeg` crate.

### Features:
- Support for hardware-accelerated video decoding and encoding on multiple platforms.
- Efficient video conversion using GPU resources to offload work from the CPU.
- Provides support for various hardware acceleration technologies such as VideoToolbox (macOS), D3D12VA (Windows), CUDA (Nvidia), QSV (Intel), and Vulkan/AMF (AMD).

### Prerequisites:
Before using this example, you need to have FFmpeg installed with hardware acceleration enabled.

#### macOS
On macOS, you can easily install FFmpeg with hardware acceleration support via Homebrew.

1. Install FFmpeg using Homebrew:
   ```sh
   brew install ffmpeg
   ```

2. No additional steps are required. FFmpeg should automatically be built with support for VideoToolbox hardware acceleration.

#### Windows
On Windows, you need to install FFmpeg through `vcpkg` and manually enable the necessary features. To install FFmpeg with the required features, use the following steps:

1. Install `vcpkg` (if you haven't already):
   Follow the instructions at [https://github.com/microsoft/vcpkg](https://github.com/microsoft/vcpkg).

2. Set the VCPKG_ROOT environment variable: After installing vcpkg, you need to set the VCPKG_ROOT environment variable so that vcpkg can be used by Rust and other tools

3. Install FFmpeg with all required features:
   ```sh
   vcpkg install ffmpeg[core,avcodec,avformat,avfilter,avdevice,swresample,swscale,nvcodec,qsv,amf,x264]:x64-windows
   ```

#### Static Compilation for Windows (Optional)
If you need to statically compile `ffmpeg` with hardware acceleration, use the following command to install the static version of FFmpeg via `vcpkg`:
```bash
vcpkg install ffmpeg[core,avcodec,avformat,avfilter,avdevice,swresample,swscale,nvcodec,qsv,amf,x264]:x64-windows-static-md
```
Ensure that you enable the `static` feature in `ez-ffmpeg` if you want to perform static linking:
```toml
[dependencies]
ez-ffmpeg = { version = "x.x.x", features = ["static"] }
```

#### Linux (Optional)
If you are using Linux, you may need to install additional libraries for hardware acceleration, such as `vdpau`, `vaapi`, or `cuda` support, depending on your hardware.

---

### Hardware Acceleration Technologies:
- **macOS**: VideoToolbox (`videotoolbox`)
- **Windows**: Direct3D 12 Video Acceleration (`d3d12va`) and Media Foundation codec (`h264_mf`)
- **Nvidia**: CUDA (`cuda`) and NVENC (`h264_nvenc`)
- **Intel**: Quick Sync Video (`qsv`)
- **AMD**: Vulkan (`vulkan`) and AMF (`h264_amf`)

Make sure to only configure one of the hardware acceleration methods based on your environment and hardware. The example provides pre-configured settings for macOS, Windows, Nvidia, Intel, and AMD.

---

### Additional Notes:
- On **macOS**, hardware acceleration is supported out of the box by `FFmpeg` with the `videotoolbox` codec.
- On **Windows**, the hardware acceleration settings require `vcpkg` to install FFmpeg with the appropriate codecs.
- On **Linux**, ensure you have the correct dependencies installed for CUDA, QSV, or Vulkan-based acceleration.
