use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use crate::core::context::output::VSyncMethod;
use crate::core::context::{FrameBox, PacketBox, Stream};
use crossbeam_channel::{Receiver, Sender};
use ffmpeg_sys_next::{AVCodec, AVMediaType, AVStream};

#[derive(Clone)]
pub(crate) struct EncoderStream {
    pub(crate) stream_index: usize,
    pub(crate) stream: Stream,
    pub(crate) codec_type: AVMediaType,
    pub(crate) encoder: *const AVCodec,
    pub(crate) vsync_method: Option<VSyncMethod>,
    pub(crate) qscale: Option<i32>,
    src: Option<Receiver<FrameBox>>,
    dst: Option<Sender<PacketBox>>,
    dst_pre: Option<Sender<PacketBox>>,
    mux_started: Option<Arc<AtomicBool>>,
}

impl EncoderStream {
    pub(crate) fn new(
        stream_index: usize,
        stream: *mut AVStream,
        codec_type: AVMediaType,
        encoder: *const AVCodec,
        vsync_method: Option<VSyncMethod>,
        qscale: Option<i32>,
        src: Receiver<FrameBox>,
        dst: Sender<PacketBox>,
        dst_pre: Sender<PacketBox>,
        mux_started: Arc<AtomicBool>,
    ) -> Self {
        Self {
            stream_index,
            stream: Stream { inner: stream },
            codec_type,
            encoder,
            vsync_method,
            qscale,
            src: Some(src),
            dst: Some(dst),
            dst_pre: Some(dst_pre),
            mux_started: Some(mux_started),
        }
    }

    pub(crate) fn take_src(&mut self) -> Receiver<FrameBox> {
        self.src.take().unwrap()
    }

    pub(crate) fn take_dst(&mut self) -> Sender<PacketBox> {
        self.dst.take().unwrap()
    }

    pub(crate) fn take_dst_pre(&mut self) -> Sender<PacketBox> {
        self.dst_pre.take().unwrap()
    }

    pub(crate) fn take_mux_started(&mut self) -> Arc<AtomicBool> {
        self.mux_started.take().unwrap()
    }

    pub fn replace_src(&mut self, new_src: Receiver<FrameBox>) -> Receiver<FrameBox> {
        let old_src = self.src.take().unwrap();
        self.src = Some(new_src);
        old_src
    }
}
