use ez_ffmpeg::FfmpegContext;
use ez_ffmpeg::Input;
use ez_ffmpeg::Output;
use std::thread::sleep;
use std::time::Duration;

fn main() {
    // Set up common output configuration for all platforms
    let output = Output::from("capture.mp4")
        .set_video_codec("libx264")
        .set_video_codec_opt("preset", "ultrafast")
        .set_video_codec_opt("crf", "23")
        .set_audio_codec("aac")
        .set_audio_codec_opt("b", "128k");

    // List video devices with: let video_devices = ez_ffmpeg::device::get_input_video_devices();
    // List audio devices with: let audio_devices = ez_ffmpeg::device::get_input_audio_devices();

    // Platform-specific input setup using cfg macros
    #[cfg(target_os = "macos")]
    {
        // macOS: avfoundation supports both device index (e.g., "0:0") and device name (e.g., "FaceTime HD Camera:Built-in Microphone")
        // Using device index
        let input = Input::from("0:0") // video device 0 and audio device 0
            .set_format("avfoundation")
            .set_input_opt("framerate", "30")
            .set_input_opt("video_size", "1280x720");
        // Using device name (commented out as an alternative)
        // let input = Input::from("FaceTime HD Camera:Built-in Microphone")
        //     .set_format("avfoundation")
        //     .set_input_opt("framerate", "30")
        //     .set_input_opt("video_size", "1280x720");

        let scheduler = FfmpegContext::builder()
            .input(input)
            .output(output)
            .build()
            .unwrap()
            .start()
            .unwrap();

        sleep(Duration::from_secs(10));
        scheduler.abort();
        sleep(Duration::from_secs(1));
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: dshow uses device names, e.g., "video=Integrated Webcam:audio=Microphone"
        let input = Input::from("video=Integrated Webcam:audio=Microphone")
            .set_format("dshow")
            .set_input_opt("framerate", "30")
            .set_input_opt("video_size", "1280x720");

        let scheduler = FfmpegContext::builder()
            .input(input)
            .output(output)
            .build()
            .unwrap()
            .start()
            .unwrap();

        sleep(Duration::from_secs(10));
        scheduler.abort();
        sleep(Duration::from_secs(1));
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: separate inputs for video (v4l2) and audio (alsa)
        let video_input = Input::from("/dev/video0")
            .set_format("v4l2")
            .set_input_opt("framerate", "30")
            .set_input_opt("video_size", "1280x720");
        let audio_input = Input::from("hw:0").set_format("alsa");

        let scheduler = FfmpegContext::builder()
            .input(video_input)
            .input(audio_input)
            .output(output)
            .build()
            .unwrap()
            .start()
            .unwrap();

        sleep(Duration::from_secs(10));
        scheduler.abort();
        sleep(Duration::from_secs(1));
    }
}
