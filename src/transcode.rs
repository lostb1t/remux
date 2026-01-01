pub struct TranscodeSession {
  id: Uuid
}

pub async fn master_hls(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    use crate::ez_ffmpeg::{FfmpegContext, Output};

    // let stream = auth.user.get_stream().await?;

    // Create directory for output files if it doesn't exist
    let output_dir = "hls_output";
    std::fs::create_dir_all(output_dir)?;

    let output_path = std::path::Path::new(output_dir).join("playlist.m3u8");

    // Build the FFmpeg context for HLS conversion
    FfmpegContext::builder()
        .input("https://stremthru.13377001.xyz/stremio/torz/eyJzdG9yZXMiOlt7ImMiOiJ0YiIsInQiOiIwNGVjODBmOS1lMzY3LTQzMWUtOWJiNy0yYWE2NDFkNzYyZWIifSx7ImMiOiJyZCIsInQiOiIyT0JQVDNUMkdBWURXQUhSTlZNN1ZKRjNDMldWQjVWQjQzVEZZWFNIV09SSUFKWkpPUFpRIn1dfQ==/_/strem/tt32537226/tb/bf533ea59e97a37a58e6a5bc6f0615f10b4b9a36/0/Hunting%20Season%202025%201080p%20WEB-DL%20HEVC%20x265%205.1%20BONE.mkv")
        .output(Output::from(output_path.to_str().unwrap())
                    // Required options
                    .set_format("hls")

                    // Optional options - customize as needed
                    .set_format_opt("hls_time", "5")          // Optional: Segment duration in seconds
                   // .set_format_opt("hls_playlist_type", "vod") // Optional: Video on demand playlist
                    .set_format_opt("hls_segment_filename", std::path::Path::new(output_dir).join("segment_%03d.ts").to_str().unwrap()) // Optional: Custom segment filename pattern
                    .set_video_codec("libx264")               // Optional: H.264 video codec
                    .set_audio_codec("aac")                   // Optional: AAC audio codec
                 //   .set_video_codec_opt("crf", "23")         // Optional: Control quality (lower is better)
        )
        .build()?
        .start()?
        .wait()?;

    println!(
        "Conversion complete. HLS files are in the '{}' directory",
        output_dir
    );
    println!(
        "You can play the HLS stream using a compatible player with: {}",
        output_path.display()
    );

    Ok(StatusCode::NO_CONTENT.into_response())
}