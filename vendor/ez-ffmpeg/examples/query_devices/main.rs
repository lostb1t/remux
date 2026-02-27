use ez_ffmpeg::device::{get_input_audio_devices, get_input_video_devices};

fn main() {
    // Fetch and print all available video input devices (e.g., cameras)
    // These devices are typically used to capture video input from physical devices like webcams.
    get_input_video_devices().unwrap().iter().for_each(|device| println!("camera: {device}"));

    // Fetch and print all available audio input devices (e.g., microphones)
    // These devices are typically used to capture audio input from physical devices like microphones.
    get_input_audio_devices().unwrap().iter().for_each(|device| println!("microphone: {device}"));
}
