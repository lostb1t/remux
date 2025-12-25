use crate::core::context::encoder_stream::EncoderStream;
use crate::core::context::output::{StreamMap, VSyncMethod};
use crate::core::context::{FrameBox, PacketBox};
use crate::core::filter::frame_pipeline::FramePipeline;
use crate::core::scheduler::input_controller::SchNode;
use crate::error::OpenOutputError;
use crossbeam_channel::{Receiver, Sender};
use ffmpeg_sys_next::{
    avformat_new_stream, AVCodec, AVFormatContext, AVMediaType, AVRational, AVSampleFormat,
    AVStream, AVFMT_NOTIMESTAMPS, AVFMT_VARIABLE_FPS,
};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::ptr::null;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Tracks which input stream produced each output stream.
/// Mirrors FFmpeg's `OutputStream->ist` bookkeeping in
/// `fftools/ffmpeg_mux_init.c`, enabling accurate metadata propagation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamSource {
    pub(crate) input_file_index: usize,
    pub(crate) input_stream_index: usize,
    pub(crate) encoded: bool,
}

/// Registry for per-output stream source information (FFmpeg reference:
/// `OutputStream->ist` and related logic in `ffmpeg_mux_init.c`).
#[derive(Default, Clone)]
pub(crate) struct StreamSourceRegistry {
    entries: Vec<Option<StreamSource>>,
}

impl StreamSourceRegistry {
    pub(crate) fn register(
        &mut self,
        output_idx: usize,
        input_file_index: usize,
        input_stream_index: usize,
        encoded: bool,
    ) {
        if self.entries.len() <= output_idx {
            self.entries.resize(output_idx + 1, None);
        }
        self.entries[output_idx] = Some(StreamSource {
            input_file_index,
            input_stream_index,
            encoded,
        });
    }

    pub(crate) fn stream_input_mapping(&self) -> Vec<(usize, (usize, usize))> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                entry
                    .as_ref()
                    .map(|source| (idx, (source.input_file_index, source.input_stream_index)))
            })
            .collect()
    }

    pub(crate) fn encoding_streams(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| match entry {
                Some(source) if source.encoded => Some(idx),
                _ => None,
            })
            .collect()
    }
}

pub(crate) struct Muxer {
    pub(crate) url: String,
    pub(crate) is_set_write_callback: bool,

    pub(crate) out_fmt_ctx: *mut AVFormatContext,
    pub(crate) oformat_flags: i32,
    pub(crate) frame_pipelines: Option<Vec<FramePipeline>>,

    pub(crate) stream_map_specs: Vec<crate::core::context::output::StreamMapSpec>,
    pub(crate) stream_maps: Vec<StreamMap>,
    pub(crate) video_codec: Option<String>,
    pub(crate) audio_codec: Option<String>,
    pub(crate) subtitle_codec: Option<String>,
    pub(crate) start_time_us: Option<i64>,
    pub(crate) recording_time_us: Option<i64>,
    pub(crate) framerate: Option<AVRational>,
    pub(crate) vsync_method: VSyncMethod,
    pub(crate) bits_per_raw_sample: Option<i32>,
    pub(crate) audio_sample_rate: Option<i32>,
    pub(crate) audio_channels: Option<i32>,
    pub(crate) audio_sample_fmt: Option<AVSampleFormat>,

    pub(crate) video_qscale: Option<i32>,
    pub(crate) audio_qscale: Option<i32>,

    pub(crate) max_video_frames: Option<i64>,
    pub(crate) max_audio_frames: Option<i64>,
    pub(crate) max_subtitle_frames: Option<i64>,

    pub(crate) video_codec_opts: Option<HashMap<CString, CString>>,
    pub(crate) audio_codec_opts: Option<HashMap<CString, CString>>,
    pub(crate) subtitle_codec_opts: Option<HashMap<CString, CString>>,
    pub(crate) format_opts: Option<HashMap<CString, CString>>,

    pub(crate) copy_ts: bool,

    // Metadata fields
    pub(crate) global_metadata: Option<HashMap<String, String>>,
    pub(crate) stream_metadata: Vec<(String, String, String)>, // (spec, key, value)
    pub(crate) chapter_metadata: HashMap<usize, HashMap<String, String>>,
    pub(crate) program_metadata: HashMap<usize, HashMap<String, String>>,
    pub(crate) metadata_map: Vec<crate::core::metadata::MetadataMapping>,
    pub(crate) auto_copy_metadata: bool,
    pub(crate) stream_sources: StreamSourceRegistry,

    streams: Vec<EncoderStream>,
    queue: Option<(Sender<PacketBox>, Receiver<PacketBox>)>,
    src_pre_receivers: Vec<Receiver<PacketBox>>,
    is_started: Arc<AtomicBool>,

    pub(crate) nb_streams: usize,
    pub(crate) nb_streams_ready: Arc<AtomicUsize>,

    pub(crate) mux_stream_nodes: Vec<Arc<SchNode>>,
}

unsafe impl Send for Muxer {}
unsafe impl Sync for Muxer {}

impl Muxer {
    pub(crate) fn new(
        url: String,
        is_set_write_callback: bool,
        out_fmt_ctx: *mut AVFormatContext,
        frame_pipelines: Option<Vec<FramePipeline>>,
        stream_map_specs: Vec<crate::core::context::output::StreamMapSpec>,
        stream_maps: Vec<StreamMap>,
        video_codec: Option<String>,
        audio_codec: Option<String>,
        subtitle_codec: Option<String>,
        start_time_us: Option<i64>,
        recording_time_us: Option<i64>,
        framerate: Option<AVRational>,
        vsync_method: VSyncMethod,
        bits_per_raw_sample: Option<i32>,
        audio_sample_rate: Option<i32>,
        audio_channels: Option<i32>,
        audio_sample_fmt: Option<AVSampleFormat>,
        video_qscale: Option<i32>,
        audio_qscale: Option<i32>,
        max_video_frames: Option<i64>,
        max_audio_frames: Option<i64>,
        max_subtitle_frames: Option<i64>,
        video_codec_opts: Option<HashMap<CString, CString>>,
        audio_codec_opts: Option<HashMap<CString, CString>>,
        subtitle_codec_opts: Option<HashMap<CString, CString>>,
        format_opts: Option<HashMap<CString, CString>>,
        copy_ts: bool,
        global_metadata: Option<HashMap<String, String>>,
        stream_metadata: Vec<(String, String, String)>,
        chapter_metadata: HashMap<usize, HashMap<String, String>>,
        program_metadata: HashMap<usize, HashMap<String, String>>,
        metadata_map: Vec<crate::core::metadata::MetadataMapping>,
        auto_copy_metadata: bool,
    ) -> Self {
        Self {
            url,
            frame_pipelines,
            out_fmt_ctx,
            oformat_flags: unsafe { (*(*out_fmt_ctx).oformat).flags },
            stream_map_specs,
            stream_maps,
            video_codec,
            audio_codec,
            subtitle_codec,
            start_time_us,
            recording_time_us,
            framerate,
            vsync_method,
            bits_per_raw_sample,
            audio_sample_rate,
            audio_channels,
            audio_sample_fmt,
            video_qscale,
            audio_qscale,
            max_video_frames,
            max_audio_frames,
            max_subtitle_frames,
            video_codec_opts,
            audio_codec_opts,
            subtitle_codec_opts,
            format_opts,
            copy_ts,
            streams: vec![],
            queue: None,
            src_pre_receivers: vec![],
            is_started: Arc::new(Default::default()),
            nb_streams: 0,
            nb_streams_ready: Arc::new(Default::default()),
            is_set_write_callback,
            mux_stream_nodes: vec![],
            global_metadata,
            stream_metadata,
            chapter_metadata,
            program_metadata,
            metadata_map,
            auto_copy_metadata,
            stream_sources: StreamSourceRegistry::default(),
        }
    }

    pub(crate) fn register_stream_source(
        &mut self,
        output_stream_index: usize,
        input_file_index: usize,
        input_stream_index: usize,
        encoded: bool,
    ) {
        self.stream_sources.register(
            output_stream_index,
            input_file_index,
            input_stream_index,
            encoded,
        );
    }

    pub(crate) fn stream_input_mapping(&self) -> Vec<(usize, (usize, usize))> {
        self.stream_sources.stream_input_mapping()
    }

    pub(crate) fn encoding_streams(&self) -> Vec<usize> {
        self.stream_sources.encoding_streams()
    }

    pub(crate) fn add_enc_stream(
        &mut self,
        media_type: AVMediaType,
        enc: *const AVCodec,
        src_node: Arc<SchNode>,
    ) -> crate::error::Result<(Sender<FrameBox>, usize)> {
        let (packet_sender, st, stream_index) = self.new_stream(src_node)?;
        let (frame_sender, frame_receiver) = crossbeam_channel::bounded(8);

        let vsync_method = if media_type == AVMediaType::AVMEDIA_TYPE_VIDEO {
            Some(unsafe {
                determine_vsync_method(
                    self.vsync_method,
                    self.framerate,
                    self.out_fmt_ctx,
                    self.copy_ts,
                )
            })
        } else {
            None
        };

        let qscale = if media_type == AVMediaType::AVMEDIA_TYPE_VIDEO {
            self.video_qscale
        } else if media_type == AVMediaType::AVMEDIA_TYPE_AUDIO {
            self.audio_qscale
        } else {
            None
        };

        let (pre_packet_sender, pre_packet_receiver) = crossbeam_channel::bounded(65536);
        self.src_pre_receivers.push(pre_packet_receiver);

        let stream = EncoderStream::new(
            stream_index,
            st,
            media_type,
            enc,
            vsync_method,
            qscale,
            frame_receiver,
            packet_sender,
            pre_packet_sender,
            self.is_started.clone(),
        );
        self.streams.push(stream);
        Ok((frame_sender, stream_index))
    }

    pub(crate) fn new_stream(
        &mut self,
        src: Arc<SchNode>,
    ) -> crate::error::Result<(Sender<PacketBox>, *mut AVStream, usize)> {
        let packet_sender = match &self.queue {
            None => {
                let (packet_sender, packet_receiver) = crossbeam_channel::bounded(8);
                self.queue = Some((packet_sender.clone(), packet_receiver));
                packet_sender
            }
            Some((packet_sender, _packet_receiver)) => packet_sender.clone(),
        };

        let index = self.nb_streams;
        self.mux_stream_nodes.insert(
            index,
            Arc::new(SchNode::MuxStream {
                src,
                last_dts: Arc::new(AtomicI64::new(0)),
                source_finished: Arc::new(AtomicBool::new(false)),
            }),
        );

        self.nb_streams += 1;
        unsafe {
            let st = avformat_new_stream(self.out_fmt_ctx, null());
            if st.is_null() {
                return Err(OpenOutputError::OutOfMemory.into());
            }
            Ok((packet_sender, st, index))
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.nb_streams == self.nb_streams_ready.load(Ordering::Acquire)
    }

    pub(crate) fn stream_ready(&self) {
        self.nb_streams_ready.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn stream_count(&self) -> usize {
        self.nb_streams
    }

    pub(crate) fn has_src(&self) -> bool {
        self.queue.is_some()
    }

    pub(crate) fn take_queue(&mut self) -> Option<(Sender<PacketBox>, Receiver<PacketBox>)> {
        self.queue.take()
    }

    pub(crate) fn take_src_pre_recvs(&mut self) -> Vec<Receiver<PacketBox>> {
        std::mem::take(&mut self.src_pre_receivers)
    }

    pub(crate) fn get_streams(&self) -> &Vec<EncoderStream> {
        &self.streams
    }

    pub(crate) fn get_streams_mut(&mut self) -> &mut Vec<EncoderStream> {
        &mut self.streams
    }

    pub(crate) fn take_streams_mut(&mut self) -> Vec<EncoderStream> {
        std::mem::take(&mut self.streams)
    }

    pub(crate) fn get_is_started(&self) -> Arc<AtomicBool> {
        self.is_started.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::StreamSourceRegistry;

    #[test]
    fn test_stream_source_registry() {
        let mut registry = StreamSourceRegistry::default();
        registry.register(2, 0, 1, false);
        registry.register(0, 1, 3, true);

        assert_eq!(
            registry.stream_input_mapping(),
            vec![(0, (1, 3)), (2, (0, 1)),]
        );

        assert_eq!(registry.encoding_streams(), vec![0]);

        registry.register(0, 1, 3, false);
        assert!(registry.encoding_streams().is_empty());
    }
}

unsafe fn determine_vsync_method(
    vsync_method: VSyncMethod,
    framerate: Option<AVRational>,
    out_fmt_ctx: *mut AVFormatContext,
    copy_ts: bool,
) -> VSyncMethod {
    if vsync_method != VSyncMethod::VsyncAuto {
        return vsync_method;
    }

    // 1. Check if frame rate is set
    let mut vsync_method = if framerate.map_or(false, |fr| fr.num != 0) {
        VSyncMethod::VsyncCfr
    }
    // 2. If output format is "avi", set VSYNC_VFR
    else if match CStr::from_ptr((*(*out_fmt_ctx).oformat).name).to_str() {
        Ok(s) => s == "avi",
        Err(_) => false,
    } {
        VSyncMethod::VsyncVfr
    }
    // 3. Otherwise, check the format flags
    else {
        let oformat = (*out_fmt_ctx).oformat;
        if (*oformat).flags & AVFMT_VARIABLE_FPS != 0 {
            if (*oformat).flags & AVFMT_NOTIMESTAMPS != 0 {
                VSyncMethod::VsyncPassthrough
            } else {
                VSyncMethod::VsyncVfr
            }
        } else {
            VSyncMethod::VsyncCfr
        }
    };

    // 4. If input stream exists and VSYNC_CFR is selected, check additional conditions
    if vsync_method == VSyncMethod::VsyncCfr && copy_ts {
        vsync_method = VSyncMethod::VsyncVscfr;
    }

    vsync_method
    // TODO 5. If VSYNC_CFR and copy_ts is true, change to VSYNC_VSCFR
}
