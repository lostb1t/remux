use ez_ffmpeg::{FfmpegContext, Input, Output};

fn main() {
    // Create input and output stream objects from files
    let mut input: Input = "test.mp4".into();
    let mut output: Output = "output.mp4".into();

    // macOS Hardware Acceleration: Using VideoToolbox for video decoding and encoding
    input = input.set_hwaccel("videotoolbox");  // Set hardware acceleration for macOS
    output = output.set_video_codec("h264_videotoolbox");  // Set video codec to h264 with VideoToolbox

    // Windows Hardware Acceleration: Using Direct3D 12 Video Acceleration (d3d12va)
    // for decoding and Media Foundation for encoding
    input = input.set_hwaccel("d3d12va");  // Set hardware acceleration for Windows (Direct3D 12)
    output = output.set_video_codec("h264_mf");  // Use Media Foundation codec for encoding

    // Nvidia Hardware Acceleration: Using CUDA for decoding and NVENC for encoding
    input = input.set_hwaccel("cuda").set_video_codec("h264_cuvid");  // CUDA for decoding and cuvid codec
    output = output.set_video_codec("h264_nvenc");  // Use NVENC for encoding

    // Intel Hardware Acceleration: Using Quick Sync Video (QSV) for decoding and encoding
    input = input.set_hwaccel("qsv").set_video_codec("h264_qsv");  // Set QSV for Intel hardware
    output = output.set_video_codec("h264_qsv");  // Use QSV for encoding

    // AMD Hardware Acceleration: Using Vulkan for decoding and AMF for encoding
    input = input.set_hwaccel("vulkan").set_video_codec("h264_amf");  // Vulkan for decoding
    output = output.set_video_codec("h264_amf");  // Use AMF for encoding

    // Build the FFMPEG context, configure input and output, then start the process
    FfmpegContext::builder()
        .input(input)  // Set input stream
        .output(output)  // Set output stream
        .build().unwrap()  // Build context
        .start().unwrap()  // Start the process
        .wait().unwrap();  // Wait for the process to finish
}
