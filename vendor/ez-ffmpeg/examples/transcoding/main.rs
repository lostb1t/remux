use ez_ffmpeg::FfmpegContext;

fn main() {
    // Build the FFmpeg context
    FfmpegContext::builder()
        // Set input file (e.g., "test.mp4")
        .input("test.mp4")
        // Set output file (e.g., "output.mov")
        .output("output.mov")
        // Build the context and return
        .build().unwrap()
        // Start the FFmpeg transcoding job
        .start().unwrap()
        // Wait for the transcoding job to complete
        .wait().unwrap();
}
