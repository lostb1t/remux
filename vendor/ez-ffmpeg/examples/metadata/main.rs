// Metadata Examples
// Demonstrates various metadata operations in ez-ffmpeg
//
// FFmpeg references:
// - fftools/ffmpeg_opt.c: opt_metadata(), opt_map_metadata() for option parsing
// - fftools/ffmpeg_mux_init.c: copy_meta(), of_add_metadata() for metadata processing
// - libavutil/dict.c: av_dict_set() for dictionary operations
//
// Run with: cargo run --example metadata [function_name]
// Available functions: basic, stream, copy, remove, complete

use ez_ffmpeg::{FfmpegContext, Output};
use std::collections::HashMap;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let example_name = args.get(1).map(|s| s.as_str()).unwrap_or("basic");

    match example_name {
        "basic" => basic_metadata(),
        "stream" => stream_metadata(),
        "copy" => copy_metadata(),
        "remove" => remove_metadata(),
        "complete" => complete_workflow(),
        _ => {
            println!("Available examples:");
            println!("  basic    - Add global metadata");
            println!("  stream   - Add stream-specific metadata");
            println!("  copy     - Copy metadata from input");
            println!("  remove   - Remove/disable metadata copying");
            println!("  complete - Comprehensive workflow");
            println!("\nUsage: cargo run --example metadata [example_name]");
        }
    }
}

/// Example 1: Basic global metadata
/// Equivalent FFmpeg command:
/// ffmpeg -i input.mp4 -metadata title="My Video" -metadata author="John Doe" output.mp4
///
/// FFmpeg reference: fftools/ffmpeg_opt.c (`opt_metadata()` handles `-metadata key=value`)
/// This replicates FFmpeg's metadata option processing where each -metadata flag
/// adds a key-value pair to the output file's global metadata dictionary.
fn basic_metadata() {
    println!("=== Basic Global Metadata Example ===\n");

    // FFmpeg reference: fftools/ffmpeg_opt.c:opt_metadata() processes each -metadata option
    // and stores it in OptionsContext->metadata for later application by of_add_metadata()
    let output = Output::from("output.mp4")
        .add_metadata("title", "My Video")
        .add_metadata("author", "John Doe")
        .add_metadata("comment", "Created with ez-ffmpeg")
        .add_metadata("year", "2024");

    FfmpegContext::builder()
        .input("input.mp4")
        .output(output)
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    println!("Video processed with metadata successfully!");
    println!("Use 'ffprobe -show_format output.mp4' to verify metadata\n");
}

/// Example 2: Stream-specific metadata
/// Equivalent FFmpeg command:
/// ffmpeg -i input.mp4 -metadata:s:v:0 language="eng" -metadata:s:a:0 language="eng" \
///        -metadata:s:a:0 title="English Audio" output.mp4
///
/// FFmpeg reference: fftools/ffmpeg_opt.c (`opt_metadata()` with stream specifiers, lines 2465-2520)
/// Stream specifier syntax is parsed by check_stream_specifier() and avformat_match_stream_specifier()
/// to identify which streams should receive the metadata.
fn stream_metadata() {
    println!("=== Stream-Specific Metadata Example ===\n");

    // FFmpeg reference: fftools/ffmpeg_opt.c:opt_metadata() parses stream specifiers like "v:0", "a:0"
    // The specifier is matched against each output stream in init_muxer() -> of_add_metadata()
    let output = Output::from("output.mp4")
        // Add metadata to first video stream
        .add_stream_metadata("v:0", "language", "eng").unwrap()
        .add_stream_metadata("v:0", "title", "Main Video").unwrap()
        // Add metadata to first audio stream
        .add_stream_metadata("a:0", "language", "eng").unwrap()
        .add_stream_metadata("a:0", "title", "English Audio").unwrap()
        // Add metadata to all subtitle streams
        .add_stream_metadata("s", "language", "eng").unwrap();

    FfmpegContext::builder()
        .input("input.mp4")
        .output(output)
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    println!("Video processed with stream metadata successfully!");
    println!("Use 'ffprobe -show_streams output.mp4' to verify stream metadata\n");
}

/// Example 3: Copying metadata from input files
/// Equivalent FFmpeg command:
/// ffmpeg -i input.mp4 -map_metadata 0 output.mp4
///
/// FFmpeg reference: fftools/ffmpeg_opt.c (`opt_map_metadata()` handles `-map_metadata`)
/// and fftools/ffmpeg_mux_init.c:copy_meta() implements the actual metadata copying logic.
/// By default, FFmpeg automatically copies global and stream metadata from the first input.
fn copy_metadata() {
    println!("=== Copy Metadata Example ===\n");

    // By default, metadata is automatically copied from the first input
    // (replicates FFmpeg's default behavior in ffmpeg_mux_init.c:copy_metadata_default())
    //
    // You can also explicitly map metadata from specific input locations:
    // FFmpeg reference: fftools/ffmpeg_opt.c:opt_map_metadata() parses the [file][:type] syntax
    // where type can be 'g' (global), 's:spec' (stream), 'c:N' (chapter), 'p:N' (program)
    let output = Output::from("output.mp4")
        // Copy global metadata from input file 0 to output global metadata
        .map_metadata_from_input(0, "g", "g").unwrap()
        // Copy metadata from first video stream of input 0 to first video stream of output
        .map_metadata_from_input(0, "s:v:0", "s:v:0").unwrap()
        // Add additional custom metadata
        .add_metadata("encoder", "ez-ffmpeg");

    FfmpegContext::builder()
        .input("input.mp4")
        .output(output)
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    println!("Video processed with copied metadata successfully!");
    println!("Use 'ffprobe -show_format -show_streams output.mp4' to verify metadata\n");
}

/// Example 4: Removing or disabling metadata
/// Equivalent FFmpeg command:
/// ffmpeg -i input.mp4 -map_metadata -1 -metadata title="New Title" output.mp4
///
/// FFmpeg reference: fftools/ffmpeg_opt.c:opt_map_metadata() with argument "-1"
/// This sets metadata_global_manual flag in copy_meta(), disabling automatic metadata copying.
/// When -map_metadata -1 is used, only explicitly specified metadata is included in output.
fn remove_metadata() {
    println!("=== Remove/Disable Metadata Example ===\n");

    // FFmpeg reference: fftools/ffmpeg_mux_init.c:copy_meta() checks metadata_global_manual
    // When true (set by -map_metadata -1), automatic metadata copying is disabled
    let output = Output::from("output.mp4")
        // Disable automatic metadata copying (like FFmpeg's -map_metadata -1)
        .disable_auto_copy_metadata()
        // Add only the metadata we want
        .add_metadata("title", "New Title")
        .add_metadata("author", "New Author");
        // Note: To remove a specific key while keeping others, use empty value:
        // .add_metadata("unwanted_key", "")  // This removes the key
        // FFmpeg reference: libavutil/dict.c:av_dict_set() removes entry when value is NULL/empty

    FfmpegContext::builder()
        .input("input.mp4")
        .output(output)
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    println!("Video processed without auto-copied metadata!");
    println!("Use 'ffprobe -show_format output.mp4' to verify only new metadata exists\n");
}

/// Example 5: Complete metadata workflow
/// Equivalent FFmpeg command:
/// ffmpeg -i input.mp4 \
///        -metadata title="Complete Example" -metadata artist="ez-ffmpeg" \
///        -metadata:s:v:0 language="eng" -metadata:s:a:0 language="eng" \
///        -map_metadata 0:s:v:0:s:v:0 \
///        -c:v libx264 -preset medium -crf 23 -c:a aac -b:a 192k \
///        output.mp4
///
/// FFmpeg reference: This example combines multiple metadata operations as processed in:
/// - fftools/ffmpeg_opt.c: opt_metadata(), opt_map_metadata() for parsing options
/// - fftools/ffmpeg_mux_init.c: copy_meta(), of_add_metadata() for applying metadata
/// The processing order mirrors FFmpeg's: map_metadata first, then user-specified metadata
fn complete_workflow() {
    println!("=== Complete Metadata Workflow Example ===\n");

    // Prepare batch metadata
    // FFmpeg reference: All -metadata options are collected and applied in order
    let mut global_metadata = HashMap::new();
    global_metadata.insert("title".to_string(), "Complete Example".to_string());
    global_metadata.insert("artist".to_string(), "ez-ffmpeg".to_string());
    global_metadata.insert("album".to_string(), "Examples Collection".to_string());
    global_metadata.insert("year".to_string(), "2024".to_string());
    global_metadata.insert("genre".to_string(), "Educational".to_string());
    global_metadata.insert("copyright".to_string(), "MIT License".to_string());

    // FFmpeg reference: Metadata processing order in ffmpeg_mux_init.c:init_muxer():
    // 1. Process -map_metadata (copy_meta())
    // 2. Apply user metadata (of_add_metadata())
    // 3. Handle stream-specific metadata for each output stream
    let output = Output::from("output.mp4")
        // Add batch global metadata
        .add_metadata_map(global_metadata)
        // Add individual metadata
        .add_metadata("encoder", "ez-ffmpeg metadata example")
        .add_metadata("comment", "This video demonstrates complete metadata workflow")

        // Add stream-specific metadata
        // FFmpeg reference: Stream metadata is matched and applied per-stream in init_muxer()
        .add_stream_metadata("v:0", "language", "eng").unwrap()
        .add_stream_metadata("v:0", "title", "HD Video Track").unwrap()
        .add_stream_metadata("v:0", "handler_name", "VideoHandler").unwrap()

        .add_stream_metadata("a:0", "language", "eng").unwrap()
        .add_stream_metadata("a:0", "title", "Stereo Audio Track").unwrap()
        .add_stream_metadata("a:0", "handler_name", "AudioHandler").unwrap()

        // Map metadata from input (this will be processed first before user metadata)
        // FFmpeg reference: copy_meta() processes -map_metadata before of_add_metadata()
        .map_metadata_from_input(0, "s:v:0", "s:v:0").unwrap()

        // Configure video codec with metadata-preserving settings
        .set_video_codec("libx264")
        .set_video_codec_opt("preset", "medium")
        .set_video_codec_opt("crf", "23")

        // Configure audio codec
        .set_audio_codec("aac")
        .set_audio_codec_opt("b", "192k");

    FfmpegContext::builder()
        .input("input.mp4")
        .output(output)
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    println!("Video processed with complete metadata workflow!");
    println!("\nVerify the metadata:");
    println!("  Global:  ffprobe -show_format output.mp4");
    println!("  Streams: ffprobe -show_streams output.mp4");
    println!("  All:     ffprobe -show_format -show_streams output.mp4\n");
}
