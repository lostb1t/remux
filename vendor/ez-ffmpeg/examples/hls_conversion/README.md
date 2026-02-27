# ez-ffmpeg Example: HLS Conversion Example

This example demonstrates how to convert a video file into HTTP Live Streaming (HLS) format using ez-ffmpeg.

## What is HLS?

HTTP Live Streaming (HLS) is an adaptive streaming protocol developed by Apple. It works by:

1. Dividing video content into small segments (typically .ts files)
2. Creating a manifest file (.m3u8) that contains metadata and references to the segments
3. Allowing clients to switch between different quality levels based on network conditions

HLS has become one of the most widely supported streaming formats across devices and platforms due to its reliability and adaptability.

## Use Cases

HLS is ideal for:
- Video-on-demand (VOD) services
- Live event broadcasting
- Multi-device content delivery
- Bandwidth-constrained environments
- Scenarios requiring adaptive bitrate streaming

## Implementation Details

### Required Options
- `.set_format("hls")` - Sets the output format to HLS

### Optional Options
- `.set_format_opt("hls_time", "5")` - Duration of each segment in seconds
- `.set_format_opt("hls_playlist_type", "vod")` - Specifies this is a video-on-demand playlist
- `.set_format_opt("hls_segment_filename", "...")` - Pattern for segment filenames
- `.set_video_codec("libx264")` - Video codec (H.264 is widely supported)
- `.set_audio_codec("aac")` - Audio codec (AAC is recommended for HLS)
- `.set_video_codec_opt("crf", "23")` - Controls quality (Constant Rate Factor)

## Technical Considerations

- **Segment Length**: Shorter segments (2-6 seconds) allow for faster quality switching but create more files and requests
- **Playlist Types**:
    - `vod` - For complete, unchanging content
    - `event` - For live events that will be available as VOD later
    - Omitting this parameter creates a regular playlist for truly live content
- **Multi-bitrate Streaming**: For full adaptive streaming, you would typically create multiple renditions at different quality levels and a master playlist

## Compatibility

HLS content can be played on:
- iOS/iPadOS/macOS devices natively
- Android devices (with appropriate player)
- Modern web browsers (using MSE-based players like hls.js)
- Smart TVs and streaming devices
- Desktop media players like VLC