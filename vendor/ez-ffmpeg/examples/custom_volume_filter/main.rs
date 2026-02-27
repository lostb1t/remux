mod volume_filter;

use crate::volume_filter::VolumeFilter;
use ez_ffmpeg::filter::frame_pipeline_builder::FramePipelineBuilder;
use ez_ffmpeg::{FfmpegContext, Output};
use ffmpeg_sys_next::AVMediaType;

fn main() {
    // Create a FramePipelineBuilder for processing audio frames
    let frame_pipeline_builder: FramePipelineBuilder = AVMediaType::AVMEDIA_TYPE_AUDIO.into();

    // Create an instance of the custom volume filter with a gain of 0.1 (reducing volume)
    let volume_filter = VolumeFilter::new(0.1);

    // Add the custom volume filter to the frame pipeline builder
    let frame_pipeline_builder = frame_pipeline_builder.filter("volume", Box::new(volume_filter));

    // Build and start the FFmpeg context, which will process the video and apply the custom filter
    FfmpegContext::builder()
        .input("test.mp4") // Input video file (audio will be processed)
        .output(Output::from("output.mp4").add_frame_pipeline(frame_pipeline_builder)) // Output file with applied filter
        .build().unwrap() // Build the FFmpeg context
        .start().unwrap() // Start the FFmpeg processing
        .wait().unwrap(); // Wait for the process to finish
}
