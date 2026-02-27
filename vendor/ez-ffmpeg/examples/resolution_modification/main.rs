use ez_ffmpeg::FfmpegContext;

fn main() {
    // Build the FFmpeg context with the input file
    FfmpegContext::builder()
        // Specify the input video file
        .input("test.mp4")
        // Apply the scale filter to change resolution
        // scale=1280:-1 keeps the aspect ratio by adjusting height automatically
        .filter_desc("scale=1280:-1")
        // Specify the output video file
        .output("output.mp4")
        // Build the context, start the process and wait for completion
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();
}
