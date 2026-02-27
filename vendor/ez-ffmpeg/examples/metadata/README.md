# ez-ffmpeg Example: Metadata Operations

This example demonstrates how to manage metadata (tags) in video and audio files using the `ez-ffmpeg` library. It covers adding, copying, mapping, and removing metadata for both global file properties and individual streams.

## Key Concepts

Metadata in media files includes information like title, author, year, language, and other descriptive tags. This example shows how to:

- **Add global metadata**: Set file-level tags (title, author, year, etc.)
- **Add stream-specific metadata**: Set tags for individual video/audio streams (language, title, etc.)
- **Copy metadata**: Copy metadata from input files to output
- **Map metadata**: Map metadata from specific input locations to output locations
- **Remove metadata**: Disable automatic metadata copying

### Stream Specifiers

Stream-specific metadata uses FFmpeg's stream specifier syntax:
- `v:0` - First video stream
- `a:0` - First audio stream
- `s` - All subtitle streams
- `v` - All video streams

## Available Examples

This example includes 5 different metadata operations. Run them individually using:

```bash
# Basic global metadata
cargo run --example metadata basic

# Stream-specific metadata
cargo run --example metadata stream

# Copy metadata from input
cargo run --example metadata copy

# Remove/disable metadata copying
cargo run --example metadata remove

# Complete workflow (combines all operations)
cargo run --example metadata complete
```

## Verifying Metadata

After running the examples, you can verify the metadata using `ffprobe`:

```bash
# View global metadata
ffprobe -show_format output.mp4

# View stream metadata
ffprobe -show_streams output.mp4

# View all metadata
ffprobe -show_format -show_streams output.mp4
```

## FFmpeg Command Equivalents

Each example function includes comments showing the equivalent FFmpeg command. For example:

- **basic**: `ffmpeg -i input.mp4 -metadata title="My Video" output.mp4`
- **stream**: `ffmpeg -i input.mp4 -metadata:s:v:0 language="eng" output.mp4`
- **copy**: `ffmpeg -i input.mp4 -map_metadata 0 output.mp4`
- **remove**: `ffmpeg -i input.mp4 -map_metadata -1 output.mp4`

## Usage

This example is useful when you need to:
- Add descriptive information to media files (title, author, copyright, etc.)
- Set language tags for video/audio/subtitle streams
- Preserve or remove metadata during transcoding
- Map metadata from specific input streams to output streams
