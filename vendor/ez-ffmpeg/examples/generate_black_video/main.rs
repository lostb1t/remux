use ez_ffmpeg::Input;
use ez_ffmpeg::Output;
use ez_ffmpeg::FfmpegContext;

/// Generates a black video with silent audio using ez-ffmpeg.
///
/// This example creates a 10-second MP4 video with a black screen (1280x720, 30 fps)
/// and silent stereo audio (44100 Hz). It is equivalent to the following FFmpeg command:
///
/// ```sh
/// ffmpeg -f lavfi -i color=c=black:s=1280x720:r=30 -f lavfi -i anullsrc=r=44100:cl=stereo -t 10 output.mp4
/// ```
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define the video input: black color, 1280x720, 30 fps
    let video_input = Input::from("color=c=black:s=1280x720:r=30")
        .set_format("lavfi");

    // Define the audio input: silent audio, 44100 Hz, stereo
    let audio_input = Input::from("anullsrc=r=44100:cl=stereo")
        .set_format("lavfi");

    // Define the output: MP4 file with 10 seconds duration
    let output = Output::from("output.mp4")
        .set_recording_time_us(10_000_000); // 10 seconds

    // Build the FFmpeg context with two inputs and one output
    FfmpegContext::builder()
        .input(video_input)
        .input(audio_input)
        .output(output)
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();

    println!("Black video with silent audio generated successfully.");

    Ok(())
}