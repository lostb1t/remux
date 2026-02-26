mod progress_filter;

use std::sync::Arc;

use crate::progress_filter::ProgressCallBackFilter;
use ez_ffmpeg::container_info::get_duration_us;
use ez_ffmpeg::filter::frame_pipeline_builder::FramePipelineBuilder;
use ez_ffmpeg::{FfmpegContext, Output};
use ffmpeg_sys_next::AVMediaType;
use progress_filter::ProgressCallBacker;

fn main() {
    let frame_pipeline_builder: FramePipelineBuilder = AVMediaType::AVMEDIA_TYPE_AUDIO.into();

    let mut progress_callbacker = ProgressCallBacker::new();
    progress_callbacker.total_duration = get_duration_us("test.mp4").unwrap();
    println!("Duration: {} us", progress_callbacker.total_duration);
    
    // Retrieve the audio stream information
    let audio_info = ez_ffmpeg::stream_info::find_audio_stream_info("test.mp4").unwrap();
    if let Some(audio_info) = audio_info {
        match audio_info {
            ez_ffmpeg::stream_info::StreamInfo::Audio { time_base, .. } => {
                progress_callbacker.time_base = time_base;
                println!("Audio time base: {}/{}", time_base.num, time_base.den);
            }
            _ => {}
        }
    } else {
        println!("Warning: Audio stream information not found");
    }


    let progress_filter = ProgressCallBackFilter::new(Arc::new(progress_callbacker));
    let frame_pipeline_builder =
        frame_pipeline_builder.filter("progress", Box::new(progress_filter));

    FfmpegContext::builder()
        .input("test.mp4") // Input video file (audio will be processed)
        .output(Output::from("output.mp4").add_frame_pipeline(frame_pipeline_builder)) // Output file with applied filter
        .build()
        .unwrap() // Build the FFmpeg context
        .start()
        .unwrap() // Start the FFmpeg processing
        .wait()
        .unwrap(); // Wait for the process to finish
}
