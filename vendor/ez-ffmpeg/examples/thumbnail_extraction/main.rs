use ez_ffmpeg::{FfmpegContext, Output};

fn main() {
    // Example 1: Generate a single thumbnail
    FfmpegContext::builder()
        // Specify the input video file
        .input("test.mp4")
        // Apply the scale filter to resize the video to a width of 160 (height is auto-adjusted to maintain aspect ratio)
        .filter_desc("scale='min(160,iw)':-1")
        // Set the output file to a JPEG image, limiting to 1 frame (equivalent to -vframes 1 in FFmpeg CLI)
        .output(Output::from("output.jpg")
            .set_max_video_frames(1)
            // Set the JPEG quality (lower value means higher quality, typical range: 2-31)
            .set_video_qscale(2)
        )
        // Build the context, start the process, and wait for completion
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    // Example 2: Generate multiple thumbnails using %03d pattern in the filename
    FfmpegContext::builder()
        // Specify the input video file
        .input("test.mp4")
        // Apply the same scale filter as before
        .filter_desc("scale='min(160,iw)':-1")
        // Set the output file with %03d to generate multiple images (e.g., output_001.jpg, output_002.jpg, etc.)
        .output(Output::from("output_%03d.jpg")
            // Limit the output to 5 frames; adjust as needed
            .set_max_video_frames(5)
            // Set the JPEG quality (lower value means higher quality)
            .set_video_qscale(2)
        )
        // Build the context, start the process, and wait for completion
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();
}

