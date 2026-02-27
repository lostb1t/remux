use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use crate::core::codec::Codec;
use crate::core::context::{FrameBox, PacketBox, Stream};
use crate::core::hwaccel::HWAccelID;
use crossbeam_channel::{Receiver, Sender};
use ffmpeg_sys_next::{
    AVCodec, AVCodecDescriptor, AVCodecParameters, AVHWDeviceType, AVMediaType,
    AVPixelFormat, AVRational, AVStream,
};

#[derive(Clone)]
pub(crate) struct DecoderStream {
    pub(crate) stream_index: usize,
    pub(crate) stream: Stream,
    pub(crate) codec_parameters: *mut AVCodecParameters,
    pub(crate) codec_type: AVMediaType,
    pub(crate) codec: Codec,
    pub(crate) codec_desc: *const AVCodecDescriptor,
    pub(crate) duration: i64,
    pub(crate) time_base: AVRational,
    pub(crate) avg_framerate: AVRational,
    pub(crate) have_sub2video: bool,

    pub(crate) hwaccel_id: HWAccelID,
    pub(crate) hwaccel_device_type: AVHWDeviceType,
    pub(crate) hwaccel_device: Option<String>,
    pub(crate) hwaccel_output_format: AVPixelFormat,

    src: Option<Receiver<PacketBox>>,
    dsts: Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>,
}

impl DecoderStream {
    pub(crate) fn new(
        stream_index: usize,
        stream: *mut AVStream,
        codec_parameters: *mut AVCodecParameters,
        codec_type: AVMediaType,
        codec: *const AVCodec,
        codec_desc: *const AVCodecDescriptor,
        duration: i64,
        time_base: AVRational,
        avg_framerate: AVRational,
        hwaccel_id: HWAccelID,
        hwaccel_device_type: AVHWDeviceType,
        hwaccel_device: Option<String>,
        hwaccel_output_format: AVPixelFormat,
    ) -> Self {
        Self {
            stream_index,
            stream: Stream { inner: stream },
            codec_parameters,
            codec_type,
            codec: Codec::new(codec),
            codec_desc,
            duration,
            time_base,
            avg_framerate,
            have_sub2video: false,
            hwaccel_id,
            hwaccel_device_type,
            hwaccel_device,
            hwaccel_output_format,
            src: None,
            dsts: vec![],
        }
    }

    pub(crate) fn is_used(&self) -> bool {
        self.src.is_some()
    }

    pub(crate) fn set_src(&mut self, src: Receiver<PacketBox>) {
        self.src = Some(src);
    }

    pub(crate) fn add_dst(&mut self, frame_dst: Sender<FrameBox>) {
        self.add_fg_dst(frame_dst, usize::MAX, Arc::new([]));
    }

    pub(crate) fn add_fg_dst(&mut self, frame_dst: Sender<FrameBox>, fg_input_index: usize, finished_flag_list: Arc<[AtomicBool]>) {
        self.dsts.push((frame_dst, fg_input_index, finished_flag_list));
    }

    pub(crate) fn take_src(&mut self) -> Option<Receiver<PacketBox>> {
        self.src.take()
    }

    pub(crate) fn take_dsts(&mut self) -> Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)> {
        std::mem::take(&mut self.dsts)
    }

    pub fn replace_dsts(&mut self, new_dsts: Sender<FrameBox>, fg_input_index: usize, finished_flag_list: Arc<[AtomicBool]>) -> Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)> {
        std::mem::replace(&mut self.dsts, vec![(new_dsts, fg_input_index, finished_flag_list)])
    }
}
