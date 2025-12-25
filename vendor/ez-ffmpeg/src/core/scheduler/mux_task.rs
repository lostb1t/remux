use crate::core::context::muxer::Muxer;
use crate::core::context::obj_pool::ObjPool;
use crate::core::context::{AVFormatContextBox, PacketBox, PacketData};
use crate::core::scheduler::ffmpeg_scheduler::{is_stopping, packet_is_null, set_scheduler_error, wait_until_not_paused, STATUS_ABORT, STATUS_END};
use crate::core::scheduler::input_controller::{InputController, SchNode};
use crate::error::Error::Muxing;
use crate::error::{MuxingError, MuxingOperationError, WriteHeaderError};
use crate::util::ffmpeg_utils::{av_err2str, hashmap_to_avdictionary};
use crate::util::thread_synchronizer::ThreadSynchronizer;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ffmpeg_next::packet::{Mut, Ref};
use ffmpeg_next::Packet;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_SUBTITLE, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::{av_get_audio_frame_duration2, av_interleaved_write_frame, av_packet_rescale_ts, av_rescale_delta, av_rescale_q, av_write_trailer, avformat_write_header, AVFormatContext, AVPacket, AVRational, AVERROR, AVERROR_EOF, AVFMT_NOTIMESTAMPS, AVFMT_TS_NONSTRICT, AV_LOG_DEBUG, AV_LOG_WARNING, AV_NOPTS_VALUE, AV_PKT_FLAG_KEY, AV_TIME_BASE_Q, EAGAIN};
use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn mux_init(
    mux_idx: usize,
    mux: &mut Muxer,
    packet_pool: ObjPool<Packet>,
    input_controller: Arc<InputController>,
    mux_stream_nodes: Vec<Arc<SchNode>>,
    scheduler_status: Arc<AtomicUsize>,
    thread_sync: ThreadSynchronizer,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    let out_fmt_ctx = mux.out_fmt_ctx;

    mux.out_fmt_ctx = null_mut();
    mux_task_start(
        mux_idx,
        out_fmt_ctx,
        mux.is_set_write_callback,
        mux.take_queue(),
        mux.start_time_us,
        mux.recording_time_us,
        mux.stream_count(),
        mux.format_opts.clone(),
        mux.take_src_pre_recvs(),
        mux.get_is_started(),
        packet_pool,
        input_controller,
        mux_stream_nodes,
        scheduler_status,
        thread_sync,
        scheduler_result,
    )
}

pub(crate) fn ready_to_init_mux(
    mux_idx: usize,
    mux: &mut Muxer,
    packet_pool: ObjPool<Packet>,
    input_controller: Arc<InputController>,
    scheduler_status: Arc<AtomicUsize>,
    thread_sync: ThreadSynchronizer,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> Option<crossbeam_channel::Sender<i32>> {
    if !mux.is_ready() {
        let (sender, receiver) = crossbeam_channel::bounded(1);

        let out_fmt_ctx = mux.out_fmt_ctx;
        let mux_stream_nodes = mux.mux_stream_nodes.clone();
        mux.out_fmt_ctx = null_mut();
        let is_set_write_callback = mux.is_set_write_callback;
        let queue = mux.take_queue();
        let src_pre_recvs = mux.take_src_pre_recvs();
        let is_started = mux.get_is_started();
        let start_time_us = mux.start_time_us;
        let recording_time_us = mux.recording_time_us;
        let stream_count = mux.stream_count();
        let nb_streams_ready = mux.nb_streams_ready.clone();
        let format_opts = mux.format_opts.clone();

        let out_fmt_ctx_box =
            AVFormatContextBox::new(out_fmt_ctx, false, is_set_write_callback);

        let _ = std::thread::Builder::new().name(format!("ready-to-init-muxer{mux_idx}")).spawn(move || {
            let mut out_fmt_ctx_box = out_fmt_ctx_box;
            loop {
                let result = receiver.recv_timeout(Duration::from_millis(100));

                if is_stopping(wait_until_not_paused(&scheduler_status)) {
                    thread_sync.thread_done();
                    info!("Init muxer receiver end command, finishing.");
                    break;
                }

                if let Err(e) = result {
                    if e == RecvTimeoutError::Disconnected {
                        thread_sync.thread_done();
                        if thread_sync.is_all_threads_done() {
                            scheduler_status.store(STATUS_END, Ordering::Release);
                        }
                        error!("mux init thread exit");
                        break;
                    }
                    continue;
                }

                let stream_index = result.unwrap();
                debug!("output_stream: {stream_index} is readied");
                let nb_streams_ready = nb_streams_ready.fetch_add(1, Ordering::Release);
                if nb_streams_ready + 1 == stream_count {
                    let out_fmt_ctx = out_fmt_ctx_box.fmt_ctx;
                    out_fmt_ctx_box.fmt_ctx = null_mut();
                    if let Err(e) = mux_task_start(
                        mux_idx,
                        out_fmt_ctx,
                        is_set_write_callback,
                        queue,
                        start_time_us,
                        recording_time_us,
                        stream_count,
                        format_opts,
                        src_pre_recvs,
                        is_started,
                        packet_pool,
                        input_controller,
                        mux_stream_nodes,
                        scheduler_status,
                        thread_sync,
                        scheduler_result,
                    ) {
                        error!("Muxer init error: {e}");
                    }
                    break;
                }
            }
        });
        Some(sender)
    } else {
        None
    }
}

fn mux_task_start(mux_idx: usize,
                  out_fmt_ctx: *mut AVFormatContext,
                  is_set_write_callback: bool,
                  queue: Option<(Sender<PacketBox>, Receiver<PacketBox>)>,
                  start_time_us: Option<i64>,
                  recording_time_us: Option<i64>,
                  stream_count: usize,
                  format_opts: Option<HashMap<CString, CString>>,
                  src_pre_receivers: Vec<Receiver<PacketBox>>,
                  is_started: Arc<AtomicBool>,
                  packet_pool: ObjPool<Packet>,
                  input_controller: Arc<InputController>,
                  mux_stream_nodes: Vec<Arc<SchNode>>,
                  scheduler_status: Arc<AtomicUsize>,
                  thread_sync: ThreadSynchronizer,
                  scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,) -> crate::error::Result<()> {

    if queue.is_none() {
        return Ok(());
    }

    let (queue_sender, queue_receiver) = queue.unwrap();

    _mux_init(mux_idx, out_fmt_ctx, is_set_write_callback, queue_receiver, start_time_us, recording_time_us, stream_count, format_opts, packet_pool,input_controller, mux_stream_nodes, scheduler_status, thread_sync, scheduler_result)?;

    for src_pre_receiver in src_pre_receivers {
        {
            let src_pre_receiver = src_pre_receiver;
            while let Ok(packet_box) = src_pre_receiver.try_recv() {
                let _ = queue_sender.send(packet_box);
            }
        }
    }

    is_started.store(true, Ordering::Release);
    Ok(())
}

fn _mux_init(
    mux_idx: usize,
    out_fmt_ctx: *mut AVFormatContext,
    is_set_write_callback: bool,
    pkt_receiver: Receiver<PacketBox>,
    start_time_us: Option<i64>,
    recording_time_us: Option<i64>,
    stream_count: usize,
    format_opts: Option<HashMap<CString, CString>>,
    packet_pool: ObjPool<Packet>,
    input_controller: Arc<InputController>,
    mux_stream_nodes: Vec<Arc<SchNode>>,
    scheduler_status: Arc<AtomicUsize>,
    thread_sync: ThreadSynchronizer,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    let out_fmt_ctx_box = AVFormatContextBox::new(out_fmt_ctx, false, is_set_write_callback);

    let mut opts = hashmap_to_avdictionary(&format_opts);

    let ret = unsafe { avformat_write_header(out_fmt_ctx, &mut opts) };
    if ret < 0 {
        error!("Could not write header (incorrect codec parameters ?): {}", av_err2str(ret));
        thread_sync.thread_done();
        if thread_sync.is_all_threads_done() {
            scheduler_status.store(STATUS_END, Ordering::Release);
        }
        return Err(Muxing(MuxingOperationError::WriteHeader(
            WriteHeaderError::from(ret),
        )));
    }

    let oformat_flags = unsafe {
        let oformat = (*out_fmt_ctx).oformat;
        (*oformat).flags
    };

    let format_name = unsafe {std::str::from_utf8_unchecked(CStr::from_ptr((*(*out_fmt_ctx).oformat).name).to_bytes())};

    let result = std::thread::Builder::new().name(format!("muxer{mux_idx}:{format_name}")).spawn(move || {
        let out_fmt_ctx_box = out_fmt_ctx_box;
        let mut started = false;
        let mut st_rescale_delta_last_map = HashMap::new();
        let mut st_last_dts_map = HashMap::new();

        let mut nb_done = 0;

        let mut ret = 0;

        loop {
            let result = pkt_receiver.recv_timeout(Duration::from_millis(100));

            if is_stopping(wait_until_not_paused(&scheduler_status)) {
                info!("Muxer receiver end command, finishing.");
                break;
            }

            if let Err(e) = result {
                if e == RecvTimeoutError::Disconnected {
                    debug!("Encoder thread exit.");
                    break;
                }
                continue;
            }

            let mut packet_box = result.unwrap();
            let pkt = packet_box.packet.as_ptr();
            let packet_data = &packet_box.packet_data;

            let stream_index = unsafe { (*pkt).stream_index as usize };
            if stream_index >= mux_stream_nodes.len() {
                error!("Invalid stream_index: {} >= {}", stream_index, mux_stream_nodes.len());
                packet_pool.release(packet_box.packet);
                continue;
            }
            let mux_stream_node = &mux_stream_nodes[stream_index];
            unsafe {
                let has_side_data = (*packet_box.packet.as_ptr()).side_data_elems > 0;
                if packet_is_null(&packet_box.packet) || (packet_box.packet.is_empty() && !has_side_data) {
                    let current_status = scheduler_status.load(Ordering::Acquire);
                    if current_status == STATUS_ABORT {
                        debug!("Muxer detected abort from stream {}, exiting without trailer", stream_index);
                        packet_pool.release(packet_box.packet);
                        break;
                    }

                    nb_done += 1;
                    packet_pool.release(packet_box.packet);

                    let mux_stream_node = mux_stream_node.as_ref();
                    let SchNode::MuxStream { src: _, last_dts: _, source_finished } = mux_stream_node else { unreachable!() };
                    source_finished.store(true, Ordering::Release);
                    input_controller.update_locked(&scheduler_status);

                    if nb_done == stream_count {
                        trace!("All streams finished");
                        break;
                    } else {
                        continue;
                    }
                }

                update_last_dts(mux_stream_node, &input_controller, &scheduler_status, pkt);

                if !packet_is_null(&packet_box.packet) && packet_data.is_copy {
                    ret = streamcopy_rescale(
                        packet_box.packet.as_mut_ptr(),
                        &packet_data,
                        &start_time_us,
                        &recording_time_us,
                        &mut started,
                    );
                    if ret == AVERROR(EAGAIN) {
                        continue;
                    } else if ret == AVERROR_EOF {
                    }
                }

                // write
                if !packet_is_null(&packet_box.packet)
                    && (*packet_box.packet.as_ptr()).stream_index >= 0
                {
                    ret = write_packet(
                        &mut st_rescale_delta_last_map,
                        oformat_flags,
                        &mut st_last_dts_map,
                        &out_fmt_ctx_box,
                        &mut packet_box,
                    );
                    packet_pool.release(packet_box.packet);

                    if ret == AVERROR_EOF {
                        trace!("Muxer returned EOF");
                        break;
                    } else if ret < 0 {
                        error!("Error muxing a packet");
                        break;
                    }
                }
            }
        }

        if ret < 0 && ret != AVERROR_EOF {
            set_scheduler_error(
                &scheduler_status,
                &scheduler_result,
                Muxing(MuxingOperationError::InterleavedWriteError(
                    MuxingError::from(ret),
                )),
            );
        }

        // write_trailer
        let final_status = scheduler_status.load(Ordering::Acquire);
        if final_status != STATUS_ABORT {
            unsafe {
                let ret = av_write_trailer(out_fmt_ctx_box.fmt_ctx);
                if ret < 0 {
                    error!("Error writing trailer: {}", av_err2str(ret));
                    set_scheduler_error(
                        &scheduler_status,
                        &scheduler_result,
                        Muxing(MuxingOperationError::TrailerWriteError(MuxingError::from(
                            ret,
                        ))),
                    );
                }
            }
        } else {
            debug!("Muxer skipping trailer due to abort");
        }

        debug!("Muxer finished.");
        thread_sync.thread_done();

        if thread_sync.is_all_threads_done() {
            scheduler_status.store(STATUS_END, Ordering::Release);
        }
    });
    if let Err(e) = result {
        error!("Muxer thread exited with error: {e}");
        return Err(MuxingOperationError::ThreadExited.into())
    }

    Ok(())
}

unsafe fn update_last_dts(mux_stream_node: &Arc<SchNode>, input_controller: &Arc<InputController>, scheduler_status: &Arc<AtomicUsize>, pkt: *const AVPacket) {
    if (*pkt).dts != AV_NOPTS_VALUE {
        let dts = av_rescale_q((*pkt).dts + (*pkt).duration, (*pkt).time_base, AV_TIME_BASE_Q);
        let node = mux_stream_node.as_ref();
        let SchNode::MuxStream { src: _, last_dts, source_finished: _ } = node else { unreachable!() };
        last_dts.store(dts, Ordering::Release);
        input_controller.update_locked(scheduler_status);
    }
}

unsafe fn streamcopy_rescale(
    pkt: *mut AVPacket,
    packet_data: &PacketData,
    start_time_us: &Option<i64>,
    recording_time_us: &Option<i64>,
    started: &mut bool,
) -> i32 {
    if !packet_data.is_copy {
        return 0;
    }
    let dts = packet_data.dts_est;

    let start_time = start_time_us.unwrap_or(0);

    // recording_time
    if let Some(recording_time_us) = recording_time_us {
        if dts >= recording_time_us + start_time {
            return AVERROR_EOF;
        }
    }

    if !*started && (*pkt).flags & AV_PKT_FLAG_KEY == 0 {
        return AVERROR(EAGAIN);
    }

    //-ss start_time
    if !*started {
        let no_pts = (*pkt).pts == AV_NOPTS_VALUE;
        let not_start = if no_pts {
            dts < start_time
        } else {
            (*pkt).pts < av_rescale_q(start_time, AV_TIME_BASE_Q, (*pkt).time_base)
        };
        if not_start {
            return AVERROR(EAGAIN);
        }
    }

    let ts_offset = av_rescale_q(start_time, AV_TIME_BASE_Q, (*pkt).time_base);

    if (*pkt).pts != AV_NOPTS_VALUE {
        (*pkt).pts -= ts_offset;
    }

    if (*pkt).dts == AV_NOPTS_VALUE {
        (*pkt).dts = av_rescale_q(dts, AV_TIME_BASE_Q, (*pkt).time_base);
    } else if packet_data.codec_type == AVMEDIA_TYPE_AUDIO {
        (*pkt).pts = (*pkt).dts - ts_offset;
    }

    (*pkt).dts -= ts_offset;

    *started = true;
    0
}

unsafe fn write_packet(
    st_rescale_delta_last_map: &mut HashMap<i32, i64>,
    oformat_flags: i32,
    st_last_dts_map: &mut HashMap<i32, i64>,
    out_fmt_ctx_box: &AVFormatContextBox,
    mut sq_packet_box: &mut PacketBox,
) -> i32 {
    mux_fixup_ts(
        st_rescale_delta_last_map,
        oformat_flags,
        st_last_dts_map,
        &mut sq_packet_box,
        out_fmt_ctx_box.fmt_ctx,
    );

    (*sq_packet_box.packet.as_mut_ptr()).stream_index =
        sq_packet_box.packet_data.output_stream_index;
    let ret =
        av_interleaved_write_frame(out_fmt_ctx_box.fmt_ctx, sq_packet_box.packet.as_mut_ptr());
    ret
}

unsafe fn mux_fixup_ts(
    st_rescale_delta_last_map: &mut HashMap<i32, i64>,
    oformat_flags: i32,
    st_last_dts_map: &mut HashMap<i32, i64>,
    packet_box: &mut PacketBox,
    out_fmt_ctx: *mut AVFormatContext,
) {
    let pkt = packet_box.packet.as_mut_ptr();
    let packet_data = &packet_box.packet_data;
    let stream_index = packet_data.output_stream_index;

    if packet_data.codec_type == AVMEDIA_TYPE_AUDIO && packet_data.is_copy {
        let mut duration = av_get_audio_frame_duration2(packet_data.codecpar, (*pkt).size);
        if duration == 0 {
            duration = (*packet_data.codecpar).frame_size;
        }

        if !st_rescale_delta_last_map.contains_key(&stream_index) {
            st_rescale_delta_last_map.insert(stream_index, 0);
        }
        let ts_rescale_delta_last = st_rescale_delta_last_map.get_mut(&stream_index).unwrap();

        (*pkt).dts = av_rescale_delta(
            (*pkt).time_base,
            (*pkt).dts,
            AVRational {
                num: 1,
                den: (*packet_data.codecpar).sample_rate,
            },
            duration,
            ts_rescale_delta_last,
            (**(*out_fmt_ctx).streams.add(stream_index as usize)).time_base,
        );
        (*pkt).pts = (*pkt).dts;

        (*pkt).duration = av_rescale_q(
            (*pkt).duration,
            (*pkt).time_base,
            (**(*out_fmt_ctx).streams.add(stream_index as usize)).time_base,
        );
    } else {
        av_packet_rescale_ts(
            pkt,
            (*pkt).time_base,
            (**(*out_fmt_ctx).streams.add(stream_index as usize)).time_base,
        );
    }
    (*pkt).time_base = (**(*out_fmt_ctx).streams.add(stream_index as usize)).time_base;

    if !st_last_dts_map.contains_key(&stream_index) {
        st_last_dts_map.insert(stream_index, AV_NOPTS_VALUE);
    }
    let last_mux_dts = st_last_dts_map.get_mut(&stream_index).unwrap();

    if (oformat_flags & AVFMT_NOTIMESTAMPS) == 0 {
        if (*pkt).dts != AV_NOPTS_VALUE && (*pkt).pts != AV_NOPTS_VALUE && (*pkt).dts > (*pkt).pts {
            warn!(
                "Invalid DTS: {} PTS: {}, replacing by guess",
                (*pkt).dts,
                (*pkt).pts
            );
            (*pkt).pts = (*pkt).pts + (*pkt).dts + *last_mux_dts + 1
                - min3((*pkt).pts, (*pkt).dts, *last_mux_dts + 1)
                - max3((*pkt).pts, (*pkt).dts, *last_mux_dts + 1);
            (*pkt).dts = (*pkt).pts;
        }

        if (packet_data.codec_type == AVMEDIA_TYPE_AUDIO
            || packet_data.codec_type == AVMEDIA_TYPE_VIDEO
            || packet_data.codec_type == AVMEDIA_TYPE_SUBTITLE)
            && (*pkt).dts != AV_NOPTS_VALUE
            && *last_mux_dts != AV_NOPTS_VALUE
        {
            let max = *last_mux_dts + ((oformat_flags & AVFMT_TS_NONSTRICT) == 0) as i64;
            if (*pkt).dts < max {
                let loglevel = if max - (*pkt).dts > 2 || packet_data.codec_type == AVMEDIA_TYPE_VIDEO {
                    AV_LOG_WARNING
                } else {
                    AV_LOG_DEBUG
                };
                if loglevel == AV_LOG_WARNING {
                    warn!(
                        "Non-monotonic DTS; previous: {}, current: {}; ",
                        *last_mux_dts,
                        (*pkt).dts
                    );
                    warn!(
                        "changing to {}. This may result in incorrect timestamps in the output file.",
                        max
                    );
                } else {
                    debug!(
                        "Non-monotonic DTS; previous: {}, current: {}; ",
                        *last_mux_dts,
                        (*pkt).dts
                    );
                    debug!(
                        "changing to {}. This may result in incorrect timestamps in the output file.",
                        max
                    );
                }

                if (*pkt).pts >= (*pkt).dts {
                    (*pkt).pts = std::cmp::max((*pkt).pts, max);
                }
                (*pkt).dts = max;
            }
        }
    }
    *last_mux_dts = (*pkt).dts;
}



fn min3(a: i64, b: i64, c: i64) -> i64 {
    std::cmp::min(a, std::cmp::min(b, c))
}

fn max3(a: i64, b: i64, c: i64) -> i64 {
    std::cmp::max(a, std::cmp::max(b, c))
}
