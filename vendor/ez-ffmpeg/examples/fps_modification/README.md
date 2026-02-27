# ez-ffmpeg Example: FPS Modification

This example shows two methods for modifying the framerate of a video to 30 FPS using `ez-ffmpeg`.

## Methods

### Method 1: Using `set_framerate` on Output
Set the framerate directly on the output video using the `set_framerate` method.

- **Best for:** Simple framerate adjustment without additional filtering.

### Method 2: Using the `fps` Filter
Apply FFmpeg’s `fps` filter to modify the input video’s framerate.

- **Best for:** More complex workflows that involve input stream manipulation or additional filters.

## When to Use Each Method

- **Method 1:** Quick and efficient when only output framerate needs modification.
- **Method 2:** Use when more control over the input stream or additional filters is required.
