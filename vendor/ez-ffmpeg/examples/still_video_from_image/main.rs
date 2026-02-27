// ez-ffmpeg Example: Create a Still Video from an Image

use ez_ffmpeg::{FfmpegContext, Input, Output};

fn main() {
    // Create an ffmpeg context
    FfmpegContext::builder()
        .input(
            Input::from("logo.jpg")
                // Set the "loop" format option to 1 to repeat the image
                .set_input_opt("loop", "1")
        )
        // Optionally scale the image to the desired output resolution
        .filter_desc("scale=1280:720")
        .output(
            Output::from("output.mp4")
                // Set the total duration of the output video to 10 seconds
                .set_recording_time_us(10 * 1_000_000)
        )
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();
}
