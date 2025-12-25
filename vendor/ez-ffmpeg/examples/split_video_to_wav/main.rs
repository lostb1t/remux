use ez_ffmpeg::FfmpegContext;
use ez_ffmpeg::Input;
use ez_ffmpeg::Output;

/// Splits the audio from a video file into multiple WAV segments, each 10 seconds long.
///
/// This example demonstrates how to use `ez-ffmpeg` to extract audio from a video
/// and split it into WAV files using the `segment` format.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define the input video file
    let input = Input::from("input.mp4");

    // Configure the output to split audio into WAV files every 10 seconds
    // The `%03d` in the filename creates numbered files like output_000.wav, output_001.wav, etc.
    let output = Output::from("output_%03d.wav")
        .set_format("segment")
        .set_format_opt("segment_time", "10");

    // Build the FFmpeg context with the specified input and output
    let context = FfmpegContext::builder()
        .input(input)
        .output(output)
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    Ok(())
}