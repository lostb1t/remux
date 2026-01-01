<p align="center">
  <img src="https://raw.githubusercontent.com/YeautyYE/ez-ffmpeg/main/logo.jpg" alt="Logo" width="300">
</p>

<div align="center">

[![Crates.io](https://img.shields.io/crates/v/ez-ffmpeg.svg)](https://crates.io/crates/ez-ffmpeg)
[![Documentation](https://img.shields.io/badge/docs.rs-ez--ffmpeg-blue)](https://docs.rs/ez-ffmpeg)
[![License: MIT/Apache-2.0/MPL-2.0](https://img.shields.io/badge/License-MIT%2FApache--2.0%2FMPL--2.0-brightgreen.svg)](https://github.com/YeautyYE/ez-ffmpeg/blob/main/LICENSE-APACHE)
[![Rust](https://img.shields.io/badge/Rust-%3E=1.80.0-orange)](https://www.rust-lang.org/)
[![FFmpeg](https://img.shields.io/badge/FFmpeg-%3E=7.0-blue)](https://ffmpeg.org)

</div>


## Overview

**`ez-ffmpeg`** provides a **safe and ergonomic Rust interface for FFmpeg integration**, offering a familiar API that closely follows FFmpeg’s original logic and parameter structures.

This library:
- Ensures full safety without using `unsafe` code
- Keeps the execution logic and parameter conventions as close to FFmpeg as possible
- Provides an intuitive and user-friendly API for media processing
- Supports custom Rust filters and flexible input/output handling
- Offers optional RTMP and OpenGL integration

By abstracting the complexity of the raw C API, `ez-ffmpeg` simplifies configuring media pipelines, performing transcoding and filtering, and inspecting media streams.

## Version Requirements

- **Rust:** Version 1.80.0 or higher.
- **FFmpeg:** Version 7.0 or higher. 

## Documentation

More information about this crate can be found in the [crate documentation](https://docs.rs/ez-ffmpeg).

## Quick Start

### Installation Prerequisites

#### macOS
```bash
brew install ffmpeg
```

#### Windows
```bash
# For dynamic linking
vcpkg install ffmpeg

# For static linking (requires 'static' feature)
vcpkg install ffmpeg:x64-windows-static-md

# Set VCPKG_ROOT environment variable
```

### Adding the Dependency

Add **ez-ffmpeg** to your project by including it in your `Cargo.toml`:

```toml
[dependencies]
ez-ffmpeg = "*"
```

### Basic Usage

Below is a basic example to get you started. Create or update your `main.rs` with the following code:

```rust
use ez_ffmpeg::FfmpegContext;
use ez_ffmpeg::FfmpegScheduler;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Build the FFmpeg context
    let context = FfmpegContext::builder()
        .input("input.mp4")
        .filter_desc("hue=s=0") // Example filter: desaturate (optional)
        .output("output.mov")
        .build()?;

    // 2. Run it via FfmpegScheduler (synchronous mode)
    let result = FfmpegScheduler::new(context)
        .start()?
        .wait();
    result?; // Propagate any errors that occur
    Ok(())
}
```
More examples can be found [here][examples].

[examples]: https://github.com/YeautyYE/ez-ffmpeg/tree/master/examples

## Features

**ez-ffmpeg** offers several optional features that can be enabled in your `Cargo.toml` as needed:

- **opengl:** Enables GPU-accelerated OpenGL filters for high-performance video processing.
- **rtmp:** Includes an embedded RTMP server for local streaming scenarios.
- **flv:** Provides support for FLV container parsing and handling.
- **async:** Adds asynchronous functionality (allowing you to `.await` operations).
- **static:** Enables static linking for FFmpeg libraries (via `ffmpeg-next/static`).

## License

ez-ffmpeg is licensed under your choice of the MIT, Apache-2.0, or MPL-2.0 licenses. You may select the license that best fits your needs.
**Important:** While ez-ffmpeg is freely usable, FFmpeg has its own licensing terms. Ensure that your use of its components complies with FFmpeg's license.