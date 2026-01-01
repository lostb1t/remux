use ez_ffmpeg::{FfmpegContext, Output};

fn main() {
    // Example 1: Extract video stream from input file and save it to output.mp4
    FfmpegContext::builder()
        .input("test.mp4") // Specify the input video file
        .output(Output::from("output.mp4")
            // Map the video stream from input (stream 0:v) to output
            .add_stream_map("0:v")
        )
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    // Example 2: Extract audio stream from input file and save it to output.aac
    FfmpegContext::builder()
        .input("test.mp4") // Specify the input video file
        .output(Output::from("output.aac")
            // Map the audio stream from input (stream 0:a) to output
            .add_stream_map("0:a")
        )
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();
}
