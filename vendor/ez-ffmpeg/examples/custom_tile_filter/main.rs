mod tile_filter;

use crate::tile_filter::Tile2x2Filter;
use ez_ffmpeg::filter::frame_pipeline_builder::FramePipelineBuilder;
use ez_ffmpeg::{FfmpegContext, Output};
use ffmpeg_sys_next::AVMediaType;

fn main() {
    // Initialize logger to see filter initialization and processing messages
    // Set RUST_LOG=debug to see detailed frame processing logs
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    println!("=== Tile2x2Filter Example ===");
    println!("Input:  test.mp4 (will be converted to YUV420P if needed)");
    println!("Output: output.mp4 (2x2 tiled, doubled dimensions)");
    println!();

    // Create a FramePipelineBuilder for processing video frames
    let frame_pipeline_builder: FramePipelineBuilder = AVMediaType::AVMEDIA_TYPE_VIDEO.into();

    // Create an instance of the custom Tile2x2Filter
    let tile_filter = Tile2x2Filter::new();

    // Add the custom tile filter to the frame pipeline builder
    // This filter will be applied to each video frame
    let frame_pipeline_builder = frame_pipeline_builder.filter("tile_2x2", Box::new(tile_filter));

    // Build and start the FFmpeg context
    // The pipeline processes: test.mp4 → format conversion → Tile2x2Filter → output.mp4
    match FfmpegContext::builder()
        .input("test.mp4") // Input video file (320x240, yuv444p in this case)
        .filter_desc("format=yuv420p") // Convert to YUV420P format (required by Tile2x2Filter)
        .output(
            Output::from("output.mp4")
                .add_frame_pipeline(frame_pipeline_builder) // Apply the tile filter
        )
        .build()
    {
        Ok(context) => {
            println!("Starting video processing...");
            match context.start() {
                Ok(scheduler) => {
                    // Wait for the processing to complete
                    match scheduler.wait() {
                        Ok(_) => {
                            println!();
                            println!("Processing completed successfully!");
                            println!("Output file: output.mp4");
                            println!();
                            println!("To verify the output:");
                            println!("  ffprobe -v error -select_streams v:0 -show_entries stream=width,height,pix_fmt output.mp4");
                        }
                        Err(e) => {
                            eprintln!("Error during processing: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error starting scheduler: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Error building FFmpeg context: {}", e);
            eprintln!();
            eprintln!("Common issues:");
            eprintln!("  - Make sure test.mp4 exists in the project root directory");
            eprintln!("  - Check that the input file is a valid video file");
            std::process::exit(1);
        }
    }
}
