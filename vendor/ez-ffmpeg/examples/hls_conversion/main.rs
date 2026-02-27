use std::path::Path;
use ez_ffmpeg::{FfmpegContext, Output};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create directory for output files if it doesn't exist
    let output_dir = "hls_output";
    std::fs::create_dir_all(output_dir)?;

    let output_path = Path::new(output_dir).join("playlist.m3u8");

    println!("Converting input video to HLS format...");

    // Build the FFmpeg context for HLS conversion
    FfmpegContext::builder()
        .input("test.mp4")
        .output(Output::from(output_path.to_str().unwrap())
                    // Required options
                    .set_format("hls")

                    // Optional options - customize as needed
                    .set_format_opt("hls_time", "5")          // Optional: Segment duration in seconds
                    .set_format_opt("hls_playlist_type", "vod") // Optional: Video on demand playlist
                    .set_format_opt("hls_segment_filename", Path::new(output_dir).join("segment_%03d.ts").to_str().unwrap()) // Optional: Custom segment filename pattern
                    .set_video_codec("libx264")               // Optional: H.264 video codec
                    .set_audio_codec("aac")                   // Optional: AAC audio codec
                    .set_video_codec_opt("crf", "23")         // Optional: Control quality (lower is better)
        )
        .build()?
        .start()?
        .wait()?;

    println!("Conversion complete. HLS files are in the '{}' directory", output_dir);
    println!("You can play the HLS stream using a compatible player with: {}", output_path.display());

    Ok(())
}