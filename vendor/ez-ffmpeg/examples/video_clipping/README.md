# ez-ffmpeg Example: Video Clipping with Time Parameters

This example demonstrates how to clip a video file by specifying different time parameters using `ez-ffmpeg`. You can define the start time, recording duration, and stop time to clip or trim the video.

## Key Methods for Video Clipping

1. **`set_start_time_us`**: Sets the **start time** (in microseconds) from which to begin reading the input video. This is useful for skipping a portion of the video at the beginning.

2. **`set_recording_time_us`**: Specifies the **duration** (in microseconds) for which the input should be processed. FFmpeg will stop processing after this time, effectively trimming the video to this length.

3. **`set_stop_time_us`**: Defines an absolute **stop time** (in microseconds) at which FFmpeg will stop reading the input. This timestamp is referenced from the start of the video stream.

You can use these methods either on the `Input` object or the `Output` object depending on the use case.

## Code Explanation

### Example 1: Set Start Time

This example shows how to specify the start time (2 seconds in this case). FFmpeg will begin processing the video from this timestamp.

`.set_start_time_us(2000_000)`  - Start at 2 seconds

### Example 2: Set Recording Duration

This example limits the recording time to 1 second, so only the first second of the video will be processed.

`.set_recording_time_us(1000_000)`  - Process only the first second

### Example 3: Set Stop Time

This example stops processing at 1 second, meaning FFmpeg will read the video until this timestamp and then stop.

`.set_stop_time_us(1000_000)`  - Stop at 1 second

### Example 4: Set Start Time on Output

In this example, the start time is set via the `Output` object, effectively starting the processing from 1 second of the video.

`.set_start_time_us(1000_000)`  - Start at 1 second

### Example 5: Set Recording Duration on Output

This example shows how to limit the recording duration via the `Output` object, so only 1 second of the video will be processed.

`.set_recording_time_us(1000_000)`  - Process only the first second

### Example 6: Set Stop Time on Output

In this case, the stop time is specified via the `Output` object, stopping the video processing at 1 second.

`.set_stop_time_us(1000_000)`  - Stop at 1 second

## When to Use

- **Use `set_start_time_us`** to skip a portion of the video at the beginning and start processing from a later point.
- **Use `set_recording_time_us`** to limit the amount of video to process, cutting off after a specified duration.
- **Use `set_stop_time_us`** to define an absolute endpoint for video processing.
