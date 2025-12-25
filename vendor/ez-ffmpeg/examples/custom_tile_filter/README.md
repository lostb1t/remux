# ez-ffmpeg Example: Custom Tile Filter (2x2)

This example demonstrates how to create a custom video filter that transforms each video frame into a 2x2 tiled layout.

## Overview

The Tile2x2Filter takes an input frame and creates an output frame with doubled dimensions, containing 4 copies of the input arranged in a 2x2 grid.

```
Input: 320x240              Output: 640x480

    +----+                  +----+----+
    | A  |        ->        | A  | A  |
    +----+                  +----+----+
                            | A  | A  |
                            +----+----+
```

## Requirements

- Only supports YUV420P pixel format
- Input dimensions must be even numbers

## Usage

```bash
cargo run --example custom_tile_filter
```

The example reads `test.mp4` from the project root and outputs `output.mp4` with 2x2 tiled frames.

### Two Usage Patterns

**Pattern 1: Format Conversion Required (current test.mp4)**

When input video is not YUV420P (e.g., test.mp4 is YUV444P), use filter_desc for format conversion:

```rust
FfmpegContext::builder()
    .input("test.mp4")
    .filter_desc("format=yuv420p")  // Convert to YUV420P in FFmpeg's filtergraph
    .output(Output::from("output.mp4").add_frame_pipeline(frame_pipeline_builder))
    .build()
```

**Pattern 2: Direct Pipeline on Input (when video is already YUV420P)**

If input video is already YUV420P, attach the pipeline directly to Input:

```rust
let input = Input::from("video.mp4")
    .add_frame_pipeline(frame_pipeline_builder);

FfmpegContext::builder()
    .input(input)
    .output("output.mp4")
    .build()
```

The filter will validate pixel format and fail immediately if input is not YUV420P.

## How It Works

1. **Validation**: Checks that input is YUV420P format with even dimensions
2. **Frame Creation**: Creates output frame with doubled width and height
3. **Plane Processing**: Processes each YUV420P plane separately:
   - Y plane (luminance): Full resolution
   - U and V planes (chrominance): Quarter resolution (subsampled)
4. **Tiling**: Copies each plane to 4 positions in a 2x2 grid using efficient memory operations

## Extending

To support different tile layouts (3x3, 4x4):
- Modify `copy_plane_to_tiles` to accept grid dimensions
- Adjust output dimensions: `input_width * grid_size`
