# ez-ffmpeg Example: Packet Scanner

This example demonstrates how to use `PacketScanner` to iterate over demuxed packet metadata from a media file without decoding. It is useful for inspecting timestamps, keyframe locations, packet sizes, and stream indices at the packet level.

## Features

- **Sequential Scan**: Iterate over all packets in a file using the `packets()` iterator.
- **Keyframe Detection**: Use `is_keyframe()` to identify keyframe packets and their positions.
- **Per-Stream Statistics**: Aggregate packet counts, keyframe counts, and byte sizes per stream.
- **Seek**: Jump to an arbitrary position (in microseconds) with `seek()` and read packets from there.
- **Jump Reading**: Perform multiple seeks to different timestamps for random-access scanning.
- **Corrupt Detection**: Use `is_corrupt()` to find packets flagged as corrupt.
- **Stream Info**: Access cached stream information (video, audio, etc.) at open time.
- **Stream-Aware Processing**: Correlate packets with their stream metadata for type-aware processing.

## Key Methods

1. **`PacketScanner::open(url)`**: Opens a media file or URL for packet scanning. Stream information is extracted and cached at open time.

2. **`scanner.packets()`**: Returns an iterator that yields `Result<PacketInfo>` for each packet until EOF.

3. **`scanner.seek(timestamp_us)`**: Seeks to the nearest keyframe before the given timestamp (in microseconds). Can be called repeatedly for jump-reading patterns.

4. **`scanner.next_packet()`**: Reads the next packet. Returns `Ok(Some(PacketInfo))` for a packet, `Ok(None)` at EOF.

5. **`scanner.streams()`**: Returns all stream information (`&[StreamInfo]`) cached at open time.

6. **`scanner.video_stream()`**: Returns the first video stream info, if any.

7. **`scanner.audio_stream()`**: Returns the first audio stream info, if any.

8. **`scanner.stream_for_packet(pkt)`**: Returns the stream info for a given packet by its stream index.

9. **`PacketInfo` getters**:
   - `stream_index()` — Which stream this packet belongs to.
   - `pts()` / `dts()` — Presentation / decompression timestamps (`Option<i64>`).
   - `duration()` — Packet duration in stream time-base units.
   - `size()` — Packet data size in bytes.
   - `pos()` — Byte position in the input file.
   - `is_keyframe()` — Whether the packet contains a keyframe.
   - `is_corrupt()` — Whether the packet is flagged as corrupt.
   - `is_video()` / `is_audio()` — Whether this packet belongs to a video or audio stream.

10. **`StreamInfo` helpers**:
    - `is_video()` / `is_audio()` — Check stream type.
    - `index()` — Stream index within the media file.
    - `stream_type()` — Human-readable type label (`"Video"`, `"Audio"`, etc.).

## Example Overview

The following scenarios are demonstrated:

1. **Sequential Scan**: Iterate all packets and count totals.
2. **Keyframe Detection**: Filter keyframes and print their metadata.
3. **Per-Stream Statistics**: Aggregate packet/keyframe/byte counts per stream index.
4. **Seek to Position**: Seek to 1 second and read the next 5 packets.
5. **Multiple Seeks**: Jump to several timestamps (0s, 0.5s, 1s, 2s, 3s) and read one packet each.
6. **Corrupt Packet Detection**: Scan for packets with the corrupt flag set.
7. **First Keyframe Per Stream**: Find and print the first keyframe in each stream.
8. **Stream Info Overview**: List all streams with their type, codec, and key parameters.
9. **Video & Audio Stream Access**: Use `video_stream()` and `audio_stream()` for quick access.
10. **Stream-Aware Packet Processing**: Classify packets by stream type using `PacketInfo::is_video()`/`is_audio()`.
11. **stream_for_packet Usage**: Use `PacketInfo::is_video()`/`is_audio()` for quick checks and `stream_for_packet()` for full stream details.

## When to Use

- **Media analysis tools**: Quickly scan packet metadata without the cost of decoding.
- **Keyframe indexing**: Build a keyframe index for seeking or thumbnail generation.
- **Stream inspection**: Understand the packet structure and interleaving of a media file.
- **Integrity checks**: Detect corrupt packets in a media file.
- **Random-access patterns**: Seek to specific positions and inspect nearby packets.
- **Stream-aware processing**: Filter or process packets based on their stream type (video, audio, etc.).
