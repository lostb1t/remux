# ez-ffmpeg Example: Generate Black Video with Silent Audio

## Functionality

This example demonstrates how to generate a black video with silent audio using Rust.

## Equivalent FFmpeg Command

```bash
ffmpeg -f lavfi -i color=c=black:s=1280x720:r=30 -f lavfi -i anullsrc=r=44100:cl=stereo -t 10 output.mp4
```

## How to Run

1. Navigate to the `examples/generate_black_video` directory.
2. Run the command `cargo run` to execute the Rust code.
3. The output video file will be generated in the current directory.

## Code Explanation

- The code in `main.rs` implements a simple video generator using Rust.
- It creates a completely black video frame and adds silent audio.
