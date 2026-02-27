use ez_ffmpeg::{FfmpegContext, Input, Output};

fn main() {
    // Example 1: Set start time to begin processing from 2 seconds (2,000,000 microseconds)
    FfmpegContext::builder()
        .input(Input::from("test.mp4")
            // Start reading from 1 seconds in the input file
            .set_start_time_us(1000_000))
        .output("output.mp4")
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    // Example 2: Set recording time to limit reading to 1 second (1,000,000 microseconds)
    FfmpegContext::builder()
        .input(Input::from("test.mp4")
            // Process only the first 1 second of the video
            .set_recording_time_us(1000_000))
        .output("output.mp4")
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    // Example 3: Set stop time to stop processing at 1 second (1,000,000 microseconds)
    FfmpegContext::builder()
        .input(Input::from("test.mp4")
            // Stop reading at 1 second in the input file
            .set_stop_time_us(1000_000))
        .output("output.mp4")
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    // Example 4: Set start time via the Output object (starts at 1 second in the input)
    FfmpegContext::builder()
        .input(Input::from("test.mp4"))
        .output(Output::from("output.mp4")
            // Start reading from 1 second in the input file
            .set_start_time_us(2000_000))
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    // Example 5: Set recording time via the Output object (process for 1 second)
    FfmpegContext::builder()
        .input(Input::from("test.mp4"))
        .output(Output::from("output.mp4")
            // Process only the first 1 second of the video
            .set_recording_time_us(1000_000))
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    // Example 6: Set stop time via the Output object (stop processing at 1 second)
    FfmpegContext::builder()
        .input(Input::from("test.mp4"))
        .output(Output::from("output.mp4")
            // Stop reading at 1 second in the input file
            .set_stop_time_us(1000_000))
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();
}
