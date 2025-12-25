use ez_ffmpeg::{FfmpegContext, Output};

fn main() {
    // 1. Build an FFmpeg context for processing the video
    FfmpegContext::builder()
        // 2. Specify the input video file
        .input("test.mp4") // The main video
        // 3. Specify the watermark image file
        .input("logo.jpg") // The watermark logo
        // 4. Apply the watermarking filter:
        //    - Scale the watermark image (logo) to fit within the video.
        //    - Format the watermark image to have a transparent background.
        //    - Apply a transparency effect (adjust the alpha channel).
        //    - Overlay the watermark image onto the video at the position (10, 10).
        .filter_desc("[1:v]scale=100:-1,format=rgba,lut=a=val*0.7[wm];[0:v][wm]overlay=10:10")
        // 5. Specify the output video file where the result will be saved
        .output(Output::from("output.mp4"))
        .build().unwrap() // Build the context
        .start().unwrap() // Start the transcoding process
        .wait().unwrap(); // Wait for the process to complete
}
