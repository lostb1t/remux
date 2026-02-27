use ffmpeg_sys_next::AVRational;
use ez_ffmpeg::{FfmpegContext, Output};

fn main() {
    // Method 1: Set framerate using Output's `set_framerate` method
    // This method directly sets the framerate on the output.
    FfmpegContext::builder()
        .input("test.mp4") // Input file: test.mp4
        // Set framerate to 30 FPS using AVRational for the output
        .output(Output::from("output.mp4").set_framerate(AVRational { num: 30, den: 1 }))
        .build().unwrap() // Build the context
        .start().unwrap() // Start the transcoding process
        .wait().unwrap(); // Wait for the process to complete

    // Method 2: Use FFmpeg filter to set framerate
    // This method applies a filter on the input stream to set the framerate.
    FfmpegContext::builder()
        .input("test.mp4") // Input file: test.mp4
        // Use the "fps" filter to modify the framerate
        .filter_desc("fps=30")
        .output(Output::from("output.mp4")) // Output file: output.mp4
        .build().unwrap() // Build the context
        .start().unwrap() // Start the transcoding process
        .wait().unwrap(); // Wait for the process to complete
}
