use ez_ffmpeg::FfmpegContext;

fn main() {
    // Create a new FFmpeg context with a builder pattern.
    // Specify three input files that will be merged into one output.
    FfmpegContext::builder()
        .input("test.mp4") // First input file
        .input("test.mp4") // Second input file
        .input("test.mp4") // Third input file
        // Define the filter description: Concatenate 3 video and audio streams
        // n=3 indicates 3 input files, v=1 for video, a=1 for audio
        .filter_desc("concat=n=3:v=1:a=1")
        // Define the output file
        .output("output.mp4")
        // Build and run the FFmpeg job
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();
}
