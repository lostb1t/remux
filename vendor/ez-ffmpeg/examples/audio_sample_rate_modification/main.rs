use ez_ffmpeg::{FfmpegContext, Output};

fn main() {
    // Build the FFmpeg context with the input file
    FfmpegContext::builder()
        // Specify the input video file
        .input("test.mp4")
        // Specify the output file and configure audio codec options
        .output(
            Output::from("output.wav")
                // Set the number of audio channels (1 = mono)
                .set_audio_channels(1)
                // Set the audio sample rate (16000 Hz)
                .set_audio_sample_rate(16000)
                // Set the audio codec to pcm_s16le (16-bit signed little-endian PCM)
                .set_audio_codec("pcm_s16le"),
        )
        // Build the context, start the process and wait for completion
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();
}
