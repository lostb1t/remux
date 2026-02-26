use ez_ffmpeg::container_info::{get_duration_us, get_format, get_metadata};
use ez_ffmpeg::stream_info::{find_all_stream_infos, find_audio_stream_info, find_video_stream_info};

fn main() {
    // Retrieve the duration in microseconds for the media file "test.mp4"
    let duration = get_duration_us("test.mp4").unwrap();
    println!("Duration: {} us", duration);

    // Retrieve the format name for "test.mp4"
    let format = get_format("test.mp4").unwrap();
    println!("Format: {}", format);

    // Retrieve the metadata for "test.mp4"
    let metadata = get_metadata("test.mp4").unwrap();
    for (key, value) in metadata {
        println!("{}: {}", key, value);
    }

    // Retrieve information about the first video stream in "test.mp4"
    let maybe_video_info = find_video_stream_info("test.mp4").unwrap();
    if let Some(video_info) = maybe_video_info {
        println!("Found video stream: {:?}", video_info);
    } else {
        println!("No video stream found.");
    }

    // Retrieve information about the first audio stream in "test.mp4"
    let maybe_audio_info = find_audio_stream_info("test.mp4").unwrap();
    if let Some(audio_info) = maybe_audio_info {
        println!("Found audio stream: {:?}", audio_info);
    } else {
        println!("No audio stream found.");
    }

    // Retrieve information about all streams (video, audio, etc.) in "test.mp4"
    let all_infos = find_all_stream_infos("test.mp4").unwrap();
    println!("Total streams found: {}", all_infos.len());
    for info in all_infos {
        println!("{:?}", info);
    }
}
