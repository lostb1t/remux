use anyhow::{Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use super::session::{TranscodeSession, TranscodeState};

/// Parameters for starting a new HLS transcode job.
#[derive(Debug, Clone)]
pub struct TranscodeParams {
    pub input_url: String,
    pub output_dir: PathBuf,
    pub video_codec: String, // "copy", "libx264", "libx265"
    pub audio_codec: String, // "aac", "copy"
    pub segment_length: u32, // seconds (default 6)
    pub start_time_ticks: Option<i64>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

impl Default for TranscodeParams {
    fn default() -> Self {
        Self {
            input_url: String::new(),
            output_dir: PathBuf::new(),
            video_codec: "copy".to_string(),
            audio_codec: "aac".to_string(),
            segment_length: 6,
            start_time_ticks: None,
            max_width: None,
            max_height: None,
            video_bitrate: None,
            audio_bitrate: None,
            audio_channels: None,
            audio_stream_index: None,
            subtitle_stream_index: None,
        }
    }
}

/// Build a GStreamer pipeline description for HLS transcoding.
fn build_hls_pipeline_desc(params: &TranscodeParams) -> String {
    let playlist_path = params.output_dir.join("main.m3u8");
    let segment_pattern = params
        .output_dir
        .join("segment_%05d.ts")
        .to_string_lossy()
        .to_string();

    // Video encoding chain
    // uridecodebin3 always decodes to raw, so we always encode.
    let video_enc = {
        let mut chain = "queue ! videoconvert ! videoscale".to_string();

        // Scale filter
        match (params.max_width, params.max_height) {
            (Some(w), Some(h)) => {
                chain.push_str(&format!(
                    " ! video/x-raw,width=[1,{}],height=[1,{}]",
                    w, h
                ));
            }
            (Some(w), None) => {
                chain.push_str(&format!(" ! video/x-raw,width=[1,{}]", w));
            }
            (None, Some(h)) => {
                chain.push_str(&format!(" ! video/x-raw,height=[1,{}]", h));
            }
            _ => {}
        }

        // Encoder
        if let Some(bitrate) = params.video_bitrate {
            let bitrate_kbps = bitrate / 1000;
            chain.push_str(&format!(
                " ! x264enc bitrate={} speed-preset=fast tune=zerolatency",
                bitrate_kbps
            ));
        } else {
            chain.push_str(" ! x264enc speed-preset=fast tune=zerolatency pass=qual quantizer=23");
        }

        chain.push_str(" ! video/x-h264,profile=high ! h264parse");
        chain
    };

    // Audio encoding chain
    // uridecodebin3 always decodes to raw, so we always encode to AAC.
    let audio_enc = {
        let mut chain = "queue ! audioconvert ! audioresample".to_string();
        if let Some(channels) = params.audio_channels {
            chain.push_str(&format!(
                " ! audio/x-raw,channels={}",
                channels
            ));
        }
        let bitrate = params.audio_bitrate.unwrap_or(128_000);
        chain.push_str(&format!(
            " ! avenc_aac bitrate={} ! aacparse",
            bitrate
        ));
        chain
    };

    format!(
        "hlssink2 name=hlssink \
           location=\"{}\" \
           playlist-location=\"{}\" \
           target-duration={} \
           max-files=0 \
         uridecodebin3 uri=\"{}\" name=demux \
         demux. ! {} ! hlssink.video \
         demux. ! {} ! hlssink.audio",
        segment_pattern,
        playlist_path.to_string_lossy(),
        params.segment_length,
        params.input_url,
        video_enc,
        audio_enc,
    )
}

/// Start an HLS transcode job using a GStreamer pipeline.
///
/// This spawns the pipeline on a blocking thread (CPU-bound)
/// and updates the session state accordingly.
pub async fn start_transcode(
    session: Arc<RwLock<TranscodeSession>>,
    params: TranscodeParams,
) -> Result<()> {
    // Update state to Running
    {
        let mut s = session.write().await;
        s.state = TranscodeState::Running;
    }

    let session_clone = session.clone();
    let params_clone = params.clone();

    let handle = tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&params_clone.output_dir)?;

        let pipeline_desc = build_hls_pipeline_desc(&params_clone);
        debug!("GStreamer HLS pipeline: {}", pipeline_desc);

        let pipeline = gst::parse::launch(&pipeline_desc)
            .map_err(|e| anyhow!("Failed to create GStreamer pipeline: {}", e))?;

        let pipeline = pipeline
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| anyhow!("Element is not a Pipeline"))?;

        pipeline
            .set_state(gst::State::Paused)
            .map_err(|e| anyhow!("Failed to set pipeline to Paused: {:?}", e))?;

        // Wait for the pipeline to reach Paused so we can seek
        let state_result = pipeline.state(gst::ClockTime::from_seconds(30));
        debug!("Pipeline state after Paused wait: {:?}", state_result);

        // Get bus early to check for preroll errors
        let bus = pipeline
            .bus()
            .ok_or_else(|| anyhow!("Failed to get pipeline bus"))?;

        // Drain any errors from preroll
        while let Some(msg) = bus.pop() {
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(err) => {
                    let src = err.src().map(|s| s.path_string())
                        .unwrap_or_else(|| gst::glib::GString::from("unknown"));
                    let dbg_info = err.debug().map(|d| d.to_string()).unwrap_or_default();
                    pipeline.set_state(gst::State::Null).ok();
                    return Err(anyhow!(
                        "GStreamer preroll error from {}: {} (debug: {})",
                        src, err.error(), dbg_info
                    ));
                }
                _ => {}
            }
        }

        // Seek if start time specified
        if let Some(ticks) = params_clone.start_time_ticks {
            let nanos = ticks as u64 * 100; // ticks are 100ns units
            let seek_pos = gst::ClockTime::from_nseconds(nanos);
            pipeline
                .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, seek_pos)
                .map_err(|e| anyhow!("Failed to seek: {:?}", e))?;
        }

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| anyhow!("Failed to set pipeline to Playing: {:?}", e))?;

        info!("Starting HLS transcode job");

        loop {
            let msg = bus.timed_pop(gst::ClockTime::from_seconds(60));
            match msg {
                Some(msg) => {
                    use gst::MessageView;
                    match msg.view() {
                        MessageView::Eos(..) => {
                            info!("HLS transcode reached end of stream");
                            break;
                        }
                        MessageView::Error(err) => {
                            let src = err
                                .src()
                                .map(|s| s.path_string())
                                .unwrap_or_else(|| gst::glib::GString::from("unknown"));
                            let error = err.error();
                            let dbg_info = err.debug().map(|d| d.to_string()).unwrap_or_default();
                            pipeline.set_state(gst::State::Null).ok();
                            return Err(anyhow!(
                                "GStreamer error from {}: {} (debug: {})",
                                src,
                                error,
                                dbg_info
                            ));
                        }
                        MessageView::Warning(warn) => {
                            let dbg_info = warn.debug().map(|d| d.to_string()).unwrap_or_default();
                            tracing::warn!(
                                "GStreamer warning: {} (debug: {})",
                                warn.error(),
                                dbg_info
                            );
                        }
                        MessageView::StateChanged(sc) => {
                            if sc.src().map(|s| s == pipeline.upcast_ref::<gst::Object>()).unwrap_or(false) {
                                debug!("Pipeline state: {:?} -> {:?}", sc.old(), sc.current());
                            }
                        }
                        _ => {}
                    }
                }
                None => {
                    // Timeout — check pipeline state
                    let (_, state, _) = pipeline.state(gst::ClockTime::ZERO);
                    if state == gst::State::Null {
                        break;
                    }
                }
            }
        }

        pipeline.set_state(gst::State::Null).ok();
        info!("HLS transcode job completed");

        Ok(())
    });

    // Wait for result and update session state
    match handle.await {
        Ok(Ok(())) => {
            let mut s = session.write().await;
            s.state = TranscodeState::Complete;
            info!(session_id = %s.id, "Transcode completed successfully");
        }
        Ok(Err(e)) => {
            let mut s = session.write().await;
            let err_msg = format!("{:#}", e);
            error!(session_id = %s.id, error = %err_msg, "Transcode failed");
            s.state = TranscodeState::Error(err_msg);
        }
        Err(e) => {
            let mut s = session.write().await;
            let err_msg = format!("Task panicked: {:#}", e);
            error!(session_id = %s.id, error = %err_msg, "Transcode task panicked");
            s.state = TranscodeState::Error(err_msg);
        }
    }

    Ok(())
}

/// Parameters for a progressive (non-HLS) transcode that streams to stdout.
#[derive(Debug, Clone)]
pub struct ProgressiveTranscodeParams {
    pub input_url: String,
    pub container: String,   // "mp4", "ts", "mkv", "webm"
    pub video_codec: String, // "copy", "libx264", "libx265", "libvpx-vp9"
    pub audio_codec: String, // "copy", "aac", "libopus"
    pub start_time_ticks: Option<i64>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

/// Start a progressive transcode that returns a readable byte stream.
///
/// Uses in-process GStreamer pipeline with `appsink` to pull buffers.
pub fn start_progressive_transcode(
    params: ProgressiveTranscodeParams,
) -> Result<impl futures::Stream<Item = std::result::Result<bytes::Bytes, std::io::Error>>> {
    // Video encoding chain
    // uridecodebin3 always decodes to raw, so we always encode.
    let video_enc = {
        let mut chain = "queue ! videoconvert ! videoscale".to_string();

        match (params.max_width, params.max_height) {
            (Some(w), Some(h)) => {
                chain.push_str(&format!(
                    " ! video/x-raw,width=[1,{}],height=[1,{}]",
                    w, h
                ));
            }
            (Some(w), None) => {
                chain.push_str(&format!(" ! video/x-raw,width=[1,{}]", w));
            }
            (None, Some(h)) => {
                chain.push_str(&format!(" ! video/x-raw,height=[1,{}]", h));
            }
            _ => {}
        }

        let gst_video_codec = match params.video_codec.as_str() {
            "libx264" | "h264" | "copy" => "x264enc",
            "libx265" | "hevc" => "x265enc",
            other => other,
        };

        if let Some(bitrate) = params.video_bitrate {
            chain.push_str(&format!(" ! {} bitrate={}", gst_video_codec, bitrate / 1000));
        } else {
            chain.push_str(&format!(" ! {} speed-preset=fast", gst_video_codec));
        }

        if gst_video_codec == "x264enc" {
            chain.push_str(" ! h264parse");
        } else if gst_video_codec == "x265enc" {
            chain.push_str(" ! h265parse");
        }

        chain
    };

    // Audio encoding chain
    // uridecodebin3 always decodes to raw, so we always encode to AAC.
    let audio_enc = {
        let mut chain = "queue ! audioconvert ! audioresample".to_string();
        if let Some(channels) = params.audio_channels {
            chain.push_str(&format!(" ! audio/x-raw,channels={}", channels));
        }
        let gst_audio_codec = match params.audio_codec.as_str() {
            "aac" | "copy" => "avenc_aac",
            "libopus" | "opus" => "opusenc",
            "mp3" => "lamemp3enc",
            other => other,
        };
        let bitrate = params.audio_bitrate.unwrap_or(128_000);
        chain.push_str(&format!(" ! {} bitrate={}", gst_audio_codec, bitrate));
        chain
    };

    // Muxer
    let (muxer, extra_props) = match params.container.as_str() {
        "ts" | "mpegts" => ("mpegtsmux", ""),
        "webm" => ("webmmux", " streamable=true"),
        "mkv" | "matroska" => ("matroskamux", " streamable=true"),
        _ => ("mp4mux", " fragment-duration=1000 streamable=true"),
    };

    let pipeline_desc = format!(
        "uridecodebin3 uri=\"{}\" name=demux \
         demux. ! {} ! mux. \
         demux. ! {} ! mux. \
         {} name=mux{} ! appsink name=sink emit-signals=true sync=false",
        params.input_url,
        video_enc,
        audio_enc,
        muxer,
        extra_props,
    );

    info!("Starting progressive transcode pipeline: {}", pipeline_desc);

    let pipeline = gst::parse::launch(&pipeline_desc)
        .map_err(|e| anyhow!("Failed to create progressive pipeline: {}", e))?;

    let pipeline = pipeline
        .dynamic_cast::<gst::Pipeline>()
        .map_err(|_| anyhow!("Element is not a Pipeline"))?;

    let appsink = pipeline
        .by_name("sink")
        .ok_or_else(|| anyhow!("appsink not found in pipeline"))?
        .dynamic_cast::<gst_app::AppSink>()
        .map_err(|_| anyhow!("sink element is not an AppSink"))?;

    let (tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, std::io::Error>>(32);

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let data = bytes::Bytes::copy_from_slice(map.as_slice());
                if tx.blocking_send(Ok(data)).is_err() {
                    return Err(gst::FlowError::Error);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| anyhow!("Failed to start progressive pipeline: {:?}", e))?;

    // Monitor the pipeline bus and clean up on EOS/error
    let pipeline_weak = pipeline.downgrade();
    tokio::task::spawn_blocking(move || {
        let Some(pipeline) = pipeline_weak.upgrade() else {
            return;
        };
        let Some(bus) = pipeline.bus() else {
            return;
        };
        loop {
            let msg = bus.timed_pop(gst::ClockTime::from_seconds(60));
            match msg {
                Some(msg) => {
                    use gst::MessageView;
                    match msg.view() {
                        MessageView::Eos(..) => {
                            debug!("Progressive transcode completed");
                            break;
                        }
                        MessageView::Error(err) => {
                            error!("Progressive transcode error: {}", err.error());
                            break;
                        }
                        _ => {}
                    }
                }
                None => {
                    let (_, state, _) = pipeline.state(gst::ClockTime::ZERO);
                    if state == gst::State::Null {
                        break;
                    }
                }
            }
        }
        pipeline.set_state(gst::State::Null).ok();
    });

    Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// Generate a master HLS playlist that references the variant playlist.
/// This mimics Jellyfin's master.m3u8 format.
pub fn generate_master_playlist(session: &TranscodeSession) -> String {
    let play_session_id = &session.id;

    // Build the CODECS string for the STREAM-INF line.
    // hls.js requires this to initialize the correct MSE SourceBuffer type.
    let video_codec_str = match session.video_codec.as_str() {
        "copy" => "avc1.640028", // assume h264 copy; best effort
        "h264" | "libx264" => "avc1.640028", // h264 high profile level 4.0
        "hevc" | "libx265" => "hvc1.1.6.L150.B0",
        _ => "avc1.640028",
    };
    let audio_codec_str = match session.audio_codec.as_str() {
        "copy" | "aac" => "mp4a.40.2", // AAC-LC
        _ => "mp4a.40.2",
    };
    let codecs = format!("{},{}", video_codec_str, audio_codec_str);

    format!(
        "#EXTM3U\n\
         #EXT-X-VERSION:3\n\
         #EXT-X-STREAM-INF:BANDWIDTH=2000000,AVERAGE-BANDWIDTH=2000000,CODECS=\"{}\"\n\
         main.m3u8?PlaySessionId={}\n",
        codecs, play_session_id
    )
}
