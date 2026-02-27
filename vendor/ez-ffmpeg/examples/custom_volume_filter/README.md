# ez-ffmpeg Example:  Custom Volume Filter with SIMD Acceleration

This example demonstrates how to create a custom audio filter in Rust, allowing you to adjust the volume of audio frames. It also shows how to leverage SIMD (Single Instruction, Multiple Data) instructions for performance optimization.

### Key Features:
- **Custom Filter Creation**: Learn how to define and implement your own filters by extending the `FrameFilter` trait.
- **SIMD Acceleration**: Use SIMD for fast audio processing. The example includes support for multiple architectures, including x86 and ARM (aarch64).
- **Flexible Audio Processing**: The filter can be easily customized to handle different audio formats, sample rates, and channel layouts.

### How It Works:
1. **Define the Filter**: Implement the filter by defining a struct (e.g., `VolumeFilter`) and a `filter_frame` method that processes the audio frames.
2. **Simd-Based Volume Adjustment**: The `adjust_volume_f32` method adjusts the volume of audio channels using SIMD instructions for efficient processing.
3. **Resampling**: If necessary, the filter automatically resamples the audio to match the expected format and channel layout.
4. **Extend or Modify**: You can extend this example to create other types of filters (e.g., equalizers, pitch adjusters) by following the same structure.

### Usage:
This example is designed to show how you can create and use custom filters within your own audio processing pipeline. Simply adjust the filter parameters (like volume) and add your own logic to modify the behavior.
