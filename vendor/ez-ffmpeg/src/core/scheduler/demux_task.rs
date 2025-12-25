use std::ffi::CStr;
use crate::core::context::decoder_stream::DecoderStream;
use crate::core::context::demuxer::Demuxer;
use crate::core::context::obj_pool::ObjPool;
use crate::core::context::{AVFormatContextBox, PacketBox, PacketData};
use crate::core::scheduler::ffmpeg_scheduler::{
    is_stopping, packet_is_null, set_scheduler_error, wait_until_not_paused,
};
use crate::error::Error::Demuxing;
use crate::error::{DemuxingError, DemuxingOperationError};
use crate::util::ffmpeg_utils::av_rescale_q_rnd;
use crossbeam_channel::Sender;
use ffmpeg_next::packet::{Mut, Ref};
use ffmpeg_next::Packet;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::AVRounding::AV_ROUND_NEAR_INF;
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::AV_CODEC_PROP_FIELDS;
use ffmpeg_sys_next::{
    av_compare_ts, av_gettime_relative, av_inv_q, av_mul_q, av_packet_ref, av_q2d, av_read_frame,
    av_rescale, av_rescale_q, av_stream_get_parser, av_usleep,
    avformat_seek_file, AVCodecDescriptor, AVCodecParameters, AVFormatContext, AVMediaType,
    AVPacket, AVRational, AVStream, AVERROR, AVERROR_EOF, AVFMT_TS_DISCONT,
    AV_NOPTS_VALUE, AV_PKT_FLAG_CORRUPT, AV_TIME_BASE, AV_TIME_BASE_Q,
    EAGAIN,
};
use libc::{c_int, c_uint};
use log::{debug, error, info, warn};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use crate::core::scheduler::input_controller::SchNode;
use crate::util::ffmpeg_utils::av_err2str;

#[cfg(feature = "docs-rs")]
pub(crate) fn demux_init(
    demux_idx: usize,
    demux: &mut Demuxer,
    independent_readrate: bool,
    packet_pool: ObjPool<Packet>,
    demux_node: Arc<SchNode>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
pub(crate) fn demux_init(
    demux_idx: usize,
    demux: &mut Demuxer,
    independent_readrate: bool,
    packet_pool: ObjPool<Packet>,
    demux_node: Arc<SchNode>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    if demux.destination_is_empty() {
        warn!(
            "The input:{} does not need to be sent to the destination, skip",
            demux.url
        );
        return Ok(());
    }

    let copy_ts = demux.copy_ts;
    let mut demux_paramter = DemuxerParamter::new(demux);

    let in_fmt_ctx = demux.in_fmt_ctx;
    demux.in_fmt_ctx = null_mut();
    let in_fmt_ctx_box = AVFormatContextBox::new(in_fmt_ctx, true, demux.is_set_read_callback);

    #[cfg(windows)]
    let hwaccel = { demux.hwaccel.take() };

    let format_name = unsafe {std::str::from_utf8_unchecked(CStr::from_ptr((*(*in_fmt_ctx).iformat).name).to_bytes())};

    let result = std::thread::Builder::new()
        .name(format!("demuxer{demux_idx}:{format_name}"))
        .spawn(move || {
            let in_fmt_ctx_box = in_fmt_ctx_box;
            let mut is_started = false;
            demux_paramter.wallclock_start = unsafe { av_gettime_relative() };

            loop {
                let mut send_flags = 0usize;
                let mut packet = match packet_pool.get() {
                    Ok(packet) => packet,
                    Err(e) => {
                        error!("get packet error on demuxing: {e}");
                        break;
                    }
                };

                unsafe {
                    let mut ret = av_read_frame(in_fmt_ctx_box.fmt_ctx, packet.as_mut_ptr());
                    if ret == AVERROR(EAGAIN) {
                        if is_stopping(wait_until_not_paused(&scheduler_status)) {
                            info!("Demuxer receiver end command, finishing.");
                            break;
                        }
                        packet_pool.release(packet);
                        av_usleep(10000);
                        continue;
                    }

                    if is_stopping(wait_until_not_paused(&scheduler_status)) {
                        info!("Demuxer receiver end command, finishing.");
                        break;
                    }

                    if ret < 0 {
                        if ret == AVERROR_EOF {
                            debug!("EOF while reading input");
                        } else {
                            error!("Error during demuxing: {}", av_err2str(ret));
                            ret = if !is_started || demux_paramter.exit_on_error {
                                ret
                            } else {
                                0
                            };
                        }

                        if ret == AVERROR_EOF {
                            ret = 0;
                        }

                        if demux_paramter.stream_loop != 0 {
                            // Windows-specific CUDA handling logic
                            #[cfg(windows)]
                            let should_skip_packet_send = hwaccel.as_deref() == Some("cuda");

                            // On non-Windows platforms, always send the packet
                            #[cfg(not(windows))]
                            let should_skip_packet_send = false;

                            // Selectively bypass packet sending based on platform and acceleration
                            let mut ret = if should_skip_packet_send {
                                // Skip sending the flush packet when using CUDA on Windows
                                // This avoids the "cuvid decode callback error" issue that occurs during loop iterations
                                // Testing showed that after the third loop iteration, avcodec_receive_frame would consistently
                                // return AVERROR_EXTERNAL with the internal error "cuvid decode callback error"
                                0 // Assume success
                            } else {
                                /* signal looping to our consumers by setting stream_index to -1 (flush packet) */
                                (*packet.as_mut_ptr()).stream_index = -1;
                                let packet_box = PacketBox {
                                    packet,
                                    packet_data: PacketData {
                                        dts_est: 0,
                                        codec_type: AVMediaType::AVMEDIA_TYPE_UNKNOWN,
                                        output_stream_index: 0,
                                        is_copy: false,
                                        codecpar: null_mut(),
                                    },
                                };
                                demux_send(&mut demux_paramter, packet_box, &packet_pool, 0, &demux_node, &scheduler_status, independent_readrate)
                            };

                            // Common seek operation for both cases
                            if ret >= 0 {
                                ret = seek_to_start(&mut demux_paramter, in_fmt_ctx_box.fmt_ctx);
                                if ret >= 0 {
                                    continue;
                                }
                            }
                            /* fallthrough to the error path */
                        }

                        if ret != 0 {
                            set_scheduler_error(
                                &scheduler_status,
                                &scheduler_result,
                                Demuxing(DemuxingOperationError::ReadFrameError(
                                    DemuxingError::from(ret),
                                )),
                            );
                        }

                        break;
                    }

                    demux_paramter.end_pts = Timestamp {
                        ts: (*packet.as_ptr()).pts,
                        tb: (*packet.as_ptr()).time_base,
                    };

                    if (*packet.as_ptr()).flags & AV_PKT_FLAG_CORRUPT != 0 {
                        if demux_paramter.exit_on_error {
                            error!(
                                "corrupt input packet in stream {}",
                                (*packet.as_ptr()).stream_index
                            );
                            packet_pool.release(packet);
                            // ret = AVERROR_INVALIDDATA;
                            break;
                        } else {
                            warn!(
                                "corrupt input packet in stream {}",
                                (*packet.as_ptr()).stream_index
                            );
                        }
                    }

                    if demux_paramter.demux_streams.len()
                        <= (*packet.as_ptr()).stream_index as usize
                    {
                        warn!("Incorrect stream id:{}", (*packet.as_ptr()).stream_index);
                        continue;
                    }

                    is_started = true;
                    ret = input_packet_process(
                        &mut demux_paramter,
                        in_fmt_ctx_box.fmt_ctx,
                        packet.as_mut_ptr(),
                        &mut send_flags,
                        copy_ts,
                    );
                    if ret < 0 {
                        break;
                    }

                    if let Some(readrate) = demux_paramter.readrate {
                        if readrate != 0.0 {
                            readrate_sleep(
                                &demux_paramter,
                                (*in_fmt_ctx_box.fmt_ctx).nb_streams,
                                readrate,
                            );
                        }
                    }

                    {
                        let ds = demux_paramter
                            .demux_streams
                            .get_mut((*packet.as_ptr()).stream_index as usize)
                            .unwrap();
                        let packet_box = PacketBox {
                            packet,
                            packet_data: PacketData {
                                dts_est: ds.dts,
                                codec_type: ds.codec_type,
                                output_stream_index: 0,
                                is_copy: false,
                                codecpar: ds.codecpar,
                            },
                        };
                        ret = demux_send(&mut demux_paramter, packet_box, &packet_pool, send_flags, &demux_node, &scheduler_status, independent_readrate);

                        if ret < 0 {
                            break;
                        }
                    }
                }
            }

            if is_started {
                demux_done(&mut demux_paramter, &packet_pool, &scheduler_status);
            }

            let node = demux_node.as_ref();
            let SchNode::Demux {
                waiter: _, task_exited
            } = node else { unreachable!() };
            task_exited.store(true, Ordering::Release);
            debug!("Demuxer finished.");
        });
    if let Err(e) = result {
        error!("Demuxer thread exited with error: {e}");
        return Err(DemuxingOperationError::ThreadExited.into());
    }

    Ok(())
}

fn demux_done(demux_paramter: &mut DemuxerParamter, packet_pool: &ObjPool<Packet>, scheduler_status: &Arc<AtomicUsize>) {
    for ds in &demux_paramter.demux_streams {
        for (i, (packet_dst, input_stream_index, output_stream_index)) in
            demux_paramter.dsts.iter().enumerate()
        {
            let dst_finished = &mut demux_paramter.dsts_finished[i];

            if ds.stream_index != *input_stream_index {
                continue;
            }

            let result = packet_pool.get();
            if let Err(e) = result {
                warn!("Demuxer done alloc packet failed: {}", e);
                continue;
            }
            let mut packet = result.unwrap();
            unsafe { (*packet.as_mut_ptr()).stream_index = -1 };

            let packet_box = PacketBox {
                packet,
                packet_data: PacketData {
                    dts_est: ds.dts,
                    codec_type: ds.codec_type,
                    output_stream_index: 0,
                    is_copy: false,
                    codecpar: ds.codecpar,
                },
            };

            let _ret = unsafe {
                demux_stream_send_to_dst(
                    packet_box,
                    packet_dst,
                    output_stream_index,
                    dst_finished,
                    0,
                    scheduler_status,
                )
            };
        }
    }
}

const READRATE_INITIAL_BURST: f32 = 0.5;
unsafe fn readrate_sleep(demux_paramter: &DemuxerParamter, nb_streams: c_uint, readrate: f32) {
    let file_start = 0;
    let burst_until = (AV_TIME_BASE as f32 * READRATE_INITIAL_BURST) as i64;

    for i in 0..nb_streams {
        let option = demux_paramter.demux_streams.get(i as usize);
        if let Some(ds) = option {
            let mut stream_ts_offset = if ds.first_dts != AV_NOPTS_VALUE {
                ds.first_dts
            } else {
                0
            };
            stream_ts_offset = std::cmp::max(stream_ts_offset, file_start);
            let pts = av_rescale(ds.dts, 1000000, AV_TIME_BASE as i64);
            let now = ((av_gettime_relative() - demux_paramter.wallclock_start) as f32 * readrate)
                as i64
                + stream_ts_offset;
            if pts - burst_until > now {
                av_usleep((pts - burst_until - now) as u32);
            }
        }
    }
}

unsafe fn input_packet_process(
    demux_paramter: &mut DemuxerParamter,
    in_fmt_ctx: *mut AVFormatContext,
    pkt: *mut AVPacket,
    send_flags: &mut usize,
    copy_ts: bool,
) -> c_int {
    ts_fixup(demux_paramter, in_fmt_ctx, pkt, copy_ts);

    if let Some(recording_time_us) = demux_paramter.recording_time_us {
        if recording_time_us != i64::MAX {
            let mut start_time = 0;
            if copy_ts {
                start_time += demux_paramter.start_time_us.unwrap_or(0);
            }
            let ds = demux_paramter
                .demux_streams
                .get_mut((*pkt).stream_index as usize)
                .unwrap();
            if ds.dts >= recording_time_us + start_time {
                *send_flags |= DEMUX_SEND_STREAMCOPY_EOF;
            }
        }
    }

    // ds->data_size += pkt->size;
    // ds->nb_packets++;

    0
}

#[cfg(feature = "docs-rs")]
unsafe fn ts_fixup(
    demux_paramter: &mut DemuxerParamter,
    in_fmt_ctx: *mut AVFormatContext,
    pkt: *mut AVPacket,
    copy_ts: bool,
) {}

#[cfg(not(feature = "docs-rs"))]
unsafe fn ts_fixup(
    demux_paramter: &mut DemuxerParamter,
    in_fmt_ctx: *mut AVFormatContext,
    pkt: *mut AVPacket,
    copy_ts: bool,
) {
    let streams = (*in_fmt_ctx).streams;
    let ist = *streams.offset((*pkt).stream_index as isize);
    let start_time = demux_paramter.start_time_effective;
    (*pkt).time_base = (*ist).time_base;

    {
        let ds = demux_paramter
            .demux_streams
            .get_mut((*pkt).stream_index as usize)
            .unwrap();

        if !ds.wrap_correction_done
            && start_time != AV_NOPTS_VALUE
            && (*ist).pts_wrap_bits < 64
        {
            let stime = av_rescale_q(start_time, AV_TIME_BASE_Q, (*pkt).time_base);
            let stime2 = stime + (1u64 << (*ist).pts_wrap_bits) as i64;
            ds.wrap_correction_done = true;

            if stime2 > stime
                && (*pkt).dts != AV_NOPTS_VALUE
                && (*pkt).dts > stime + (1i64 << ((*ist).pts_wrap_bits - 1))
            {
                (*pkt).dts -= (1u64 << (*ist).pts_wrap_bits) as i64;
                ds.wrap_correction_done = false;
            }
            if stime2 > stime
                && (*pkt).pts != AV_NOPTS_VALUE
                && (*pkt).pts > stime + (1i64 << ((*ist).pts_wrap_bits - 1))
            {
                (*pkt).pts -= (1u64 << (*ist).pts_wrap_bits) as i64;
                ds.wrap_correction_done = false;
            }
        }
    }

    if (*pkt).dts != AV_NOPTS_VALUE {
        (*pkt).dts += av_rescale_q(demux_paramter.ts_offset, AV_TIME_BASE_Q, (*pkt).time_base);
    }
    if (*pkt).pts != AV_NOPTS_VALUE {
        (*pkt).pts += av_rescale_q(demux_paramter.ts_offset, AV_TIME_BASE_Q, (*pkt).time_base);
    }

    // TODO ts_scale
    /*if ((*pkt).pts != AV_NOPTS_VALUE)
    (*pkt).pts *= ds.ts_scale;
    if ((*pkt).dts != AV_NOPTS_VALUE)
    (*pkt).dts *= ds.ts_scale;*/

    let duration = av_rescale_q(
        demux_paramter.duration.ts,
        demux_paramter.duration.tb,
        (*pkt).time_base,
    );

    if (*pkt).pts != AV_NOPTS_VALUE {
        // audio decoders take precedence for estimating total file duration
        let pkt_duration = if demux_paramter.have_audio_dec {
            0
        } else {
            (*pkt).duration
        };

        (*pkt).pts += duration;

        // update max/min pts that will be used to compute total file duration
        // when using -stream_loop
        if demux_paramter.max_pts.ts == AV_NOPTS_VALUE
            || av_compare_ts(
                demux_paramter.max_pts.ts,
                demux_paramter.max_pts.tb,
                (*pkt).pts + pkt_duration,
                (*pkt).time_base,
            ) < 0
        {
            demux_paramter.max_pts = Timestamp {
                ts: (*pkt).pts + pkt_duration,
                tb: (*pkt).time_base,
            };
        }
        if demux_paramter.min_pts.ts == AV_NOPTS_VALUE
            || av_compare_ts(
                demux_paramter.min_pts.ts,
                demux_paramter.min_pts.tb,
                (*pkt).pts,
                (*pkt).time_base,
            ) > 0
        {
            demux_paramter.min_pts = Timestamp {
                ts: (*pkt).pts,
                tb: (*pkt).time_base,
            };
        }
    }

    if (*pkt).dts != AV_NOPTS_VALUE {
        (*pkt).dts += duration;
    }

    // detect and try to correct for timestamp discontinuities
    ts_discontinuity_process(demux_paramter, in_fmt_ctx, ist, pkt, copy_ts);

    // update estimated/predicted dts
    ist_dts_update(demux_paramter, ist, pkt);
}

#[cfg(feature = "docs-rs")]
unsafe fn ist_dts_update(
    demux_paramter: &mut DemuxerParamter,
    ist: *mut AVStream,
    pkt: *mut AVPacket,
) {}

#[cfg(not(feature = "docs-rs"))]
unsafe fn ist_dts_update(
    demux_paramter: &mut DemuxerParamter,
    ist: *mut AVStream,
    pkt: *mut AVPacket,
) {
    let ds = demux_paramter
        .demux_streams
        .get_mut((*pkt).stream_index as usize)
        .unwrap();

    let par = (*ist).codecpar;

    if !ds.saw_first_ts {
        ds.dts = if (*ist).avg_frame_rate.num != 0 {
            (((-(*par).video_delay) * AV_TIME_BASE) as f64 / av_q2d((*ist).avg_frame_rate)) as i64
        } else {
            0
        };
        ds.first_dts = ds.dts;

        if (*pkt).pts != AV_NOPTS_VALUE {
            ds.dts += av_rescale_q((*pkt).pts, (*pkt).time_base, AV_TIME_BASE_Q);
            ds.first_dts = ds.dts;
        }
        ds.saw_first_ts = true;
    }

    if ds.next_dts == AV_NOPTS_VALUE {
        ds.next_dts = ds.dts;
    }

    if (*pkt).dts != AV_NOPTS_VALUE {
        ds.dts = av_rescale_q((*pkt).dts, (*pkt).time_base, AV_TIME_BASE_Q);
        ds.next_dts = ds.dts;
    }

    ds.dts = ds.next_dts;
    match (*par).codec_type {
        AVMEDIA_TYPE_AUDIO => {
            if (*par).sample_rate != 0 {
                ds.next_dts +=
                    (AV_TIME_BASE as i64 * (*par).frame_size as i64) / (*par).sample_rate as i64;
            } else {
                ds.next_dts += av_rescale_q((*pkt).duration, (*pkt).time_base, AV_TIME_BASE_Q);
            }
        }
        AVMEDIA_TYPE_VIDEO => {
            if (*ist).avg_frame_rate.num != 0 {
                // TODO: Remove work-around for c99-to-c89 issue 7
                let time_base_q = AV_TIME_BASE_Q;
                let next_dts =
                    av_rescale_q(ds.next_dts, time_base_q, av_inv_q((*ist).avg_frame_rate));
                ds.next_dts =
                    av_rescale_q(next_dts + 1, av_inv_q((*ist).avg_frame_rate), time_base_q);
            } else if (*pkt).duration != 0 {
                ds.next_dts += av_rescale_q((*pkt).duration, (*pkt).time_base, AV_TIME_BASE_Q);
            } else if (*par).framerate.num != 0 {
                let field_rate = av_mul_q((*par).framerate, AVRational { num: 2, den: 1 });
                let mut fields = 2;

                if !ds.codec_desc.is_null()
                    && ((*ds.codec_desc).props != 0 & AV_CODEC_PROP_FIELDS)
                    && !av_stream_get_parser(ist).is_null()
                {
                    fields = 1 + (*av_stream_get_parser(ist)).repeat_pict;
                }

                ds.next_dts += av_rescale_q(fields as i64, av_inv_q(field_rate), AV_TIME_BASE_Q);
            }
        }
        _ => {}
    }

    //TODO
    // fd -> dts_est = ds.dts;
}

#[cfg(feature = "docs-rs")]
unsafe fn ts_discontinuity_process(
    demux_paramter: &mut DemuxerParamter,
    in_fmt_ctx: *mut AVFormatContext,
    ist: *mut AVStream,
    pkt: *mut AVPacket,
) {}

#[cfg(not(feature = "docs-rs"))]
unsafe fn ts_discontinuity_process(
    demux_paramter: &mut DemuxerParamter,
    in_fmt_ctx: *mut AVFormatContext,
    ist: *mut AVStream,
    pkt: *mut AVPacket,
    copy_ts: bool,
) {
    let offset = av_rescale_q(
        demux_paramter.ts_offset_discont,
        AV_TIME_BASE_Q,
        (*pkt).time_base,
    );

    // apply previously-detected timestamp-discontinuity offset
    // (to all streams, not just audio/video)
    if (*pkt).dts != AV_NOPTS_VALUE {
        (*pkt).dts += offset;
    }
    if (*pkt).pts != AV_NOPTS_VALUE {
        (*pkt).pts += offset;
    }

    // detect timestamp discontinuities for audio/video
    if ((*(*ist).codecpar).codec_type == AVMEDIA_TYPE_VIDEO
        || (*(*ist).codecpar).codec_type == AVMEDIA_TYPE_AUDIO)
        && (*pkt).dts != AV_NOPTS_VALUE
    {
        ts_discontinuity_detect(demux_paramter, in_fmt_ctx, ist, pkt, copy_ts);
    }
}

#[cfg(feature = "docs-rs")]
unsafe fn ts_discontinuity_detect(
    demux_paramter: &mut DemuxerParamter,
    in_fmt_ctx: *mut AVFormatContext,
    ist: *mut AVStream,
    pkt: *mut AVPacket,
) {}

#[cfg(not(feature = "docs-rs"))]
unsafe fn ts_discontinuity_detect(
    demux_paramter: &mut DemuxerParamter,
    in_fmt_ctx: *mut AVFormatContext,
    ist: *mut AVStream,
    pkt: *mut AVPacket,
    copy_ts: bool,
) {
    let ds = demux_paramter
        .demux_streams
        .get_mut((*pkt).stream_index as usize)
        .unwrap();

    let fmt_is_discont = (*(*in_fmt_ctx).iformat).flags & AVFMT_TS_DISCONT;

    let mut disable_discontinuity_correction = copy_ts;
    let pkt_dts = av_rescale_q_rnd(
        (*pkt).dts,
        (*pkt).time_base,
        AV_TIME_BASE_Q,
        AV_ROUND_NEAR_INF as u32,
    );

    if copy_ts && ds.next_dts != AV_NOPTS_VALUE && fmt_is_discont != 0 && (*ist).pts_wrap_bits < 60
    {
        let wrap_dts = av_rescale_q_rnd(
            (*pkt).dts + (1i64 << (*ist).pts_wrap_bits),
            (*pkt).time_base,
            AV_TIME_BASE_Q,
            AV_ROUND_NEAR_INF as u32,
        );
        if (wrap_dts - ds.next_dts).abs() < (pkt_dts - ds.next_dts).abs() / 10 {
            disable_discontinuity_correction = false;
        }
    }

    const DTS_DELTA_THRESHOLD: i64 = 10;
    if ds.next_dts != AV_NOPTS_VALUE && !disable_discontinuity_correction {
        let mut delta = pkt_dts - ds.next_dts;
        if fmt_is_discont != 0 {
            if delta.abs() > 1i64 * DTS_DELTA_THRESHOLD * AV_TIME_BASE as i64
                || (pkt_dts + (AV_TIME_BASE / 10) as i64) < ds.dts
            {
                (*demux_paramter).ts_offset_discont -= delta;
                warn!(
                    "timestamp discontinuity (stream id={}): {}, new offset= {}",
                    (*ist).id,
                    delta,
                    (*demux_paramter).ts_offset_discont
                );
                (*pkt).dts -= av_rescale_q(delta, AV_TIME_BASE_Q, (*pkt).time_base);
                if (*pkt).pts != AV_NOPTS_VALUE {
                    (*pkt).pts -= av_rescale_q(delta, AV_TIME_BASE_Q, (*pkt).time_base);
                }
            }
        } else {
            const DTS_ERROR_THRESHOLD: i64 = 108000;
            if delta.abs() > 1i64 * DTS_ERROR_THRESHOLD * AV_TIME_BASE as i64 {
                warn!(
                    "DTS {}, next:{} st:{} invalid dropping",
                    (*pkt).dts,
                    ds.next_dts,
                    (*pkt).stream_index
                );
                (*pkt).dts = AV_NOPTS_VALUE;
            }
            if (*pkt).pts != AV_NOPTS_VALUE {
                let pkt_pts = av_rescale_q((*pkt).pts, (*pkt).time_base, AV_TIME_BASE_Q);
                delta = pkt_pts - ds.next_dts;
                if delta.abs() > 1i64 * DTS_ERROR_THRESHOLD * AV_TIME_BASE as i64 {
                    warn!(
                        "PTS {}, next:{} invalid dropping st:{}",
                        (*pkt).pts,
                        ds.next_dts,
                        (*pkt).stream_index
                    );
                    (*pkt).pts = AV_NOPTS_VALUE;
                }
            }
        }
    } else if ds.next_dts == AV_NOPTS_VALUE
        && !copy_ts
        && fmt_is_discont != 0
        && (*demux_paramter).last_ts != AV_NOPTS_VALUE
    {
        let delta = pkt_dts - (*demux_paramter).last_ts;
        if delta.abs() > 1i64 * DTS_DELTA_THRESHOLD * AV_TIME_BASE as i64 {
            (*demux_paramter).ts_offset_discont -= delta;
            debug!(
                "Inter stream timestamp discontinuity {}, new offset= {}",
                delta,
                (*demux_paramter).ts_offset_discont
            );
            (*pkt).dts -= av_rescale_q(delta, AV_TIME_BASE_Q, (*pkt).time_base);
            if (*pkt).pts != AV_NOPTS_VALUE {
                (*pkt).pts -= av_rescale_q(delta, AV_TIME_BASE_Q, (*pkt).time_base);
            }
        }
    }

    (*demux_paramter).last_ts = av_rescale_q((*pkt).dts, (*pkt).time_base, AV_TIME_BASE_Q);
}

struct DemuxStreamParamter {
    codec_type: AVMediaType,
    stream_index: usize,
    codecpar: *mut AVCodecParameters,
    codec_desc: *const AVCodecDescriptor,

    wrap_correction_done: bool,
    saw_first_ts: bool,
    ///< dts of the first packet read for this stream (in AV_TIME_BASE units)
    first_dts: i64,

    next_dts: i64,
    ///< dts of the last packet read for this stream (in AV_TIME_BASE units)
    dts: i64,
}

unsafe impl Send for DemuxStreamParamter {}
unsafe impl Sync for DemuxStreamParamter {}
impl DemuxStreamParamter {
    fn new(ds: &DecoderStream) -> Self {
        Self {
            codec_type: ds.codec_type,
            stream_index: ds.stream_index,
            codecpar: ds.codec_parameters,
            codec_desc: ds.codec_desc,
            wrap_correction_done: false,
            saw_first_ts: false,
            first_dts: AV_NOPTS_VALUE,
            next_dts: AV_NOPTS_VALUE,
            dts: 0,
        }
    }
}
struct DemuxerParamter {
    dsts_finished: Vec<bool>,
    have_audio_dec: bool,

    wallclock_start: i64,
    /**
     * Extra timestamp offset added by discontinuity handling.
     */
    ts_offset_discont: i64,
    last_ts: i64,

    start_time_effective: i64,
    ts_offset: i64,

    readrate: Option<f32>,
    start_time_us: Option<i64>,
    recording_time_us: Option<i64>,
    exit_on_error: bool,
    stream_loop: i32,

    end_pts: Timestamp,

    /* duration of the looped segment of the input file */
    duration: Timestamp,
    /* pts with the smallest/largest values ever seen */
    min_pts: Timestamp,
    max_pts: Timestamp,

    demux_streams: Vec<DemuxStreamParamter>,

    dsts: Vec<(Sender<PacketBox>, usize, Option<usize>)>,
}
unsafe impl Send for DemuxerParamter {}
unsafe impl Sync for DemuxerParamter {}
impl DemuxerParamter {
    fn new(demux: &mut Demuxer) -> Self {
        let dsts = demux.take_dsts();
        let dsts_finished = vec![false; dsts.len()];

        let mut have_audio_dec = false;
        for (_packet_dst, input_stream_index, _output_stream_index) in &dsts {
            let stream = demux.get_stream(*input_stream_index);
            if stream.codec_type == AVMEDIA_TYPE_AUDIO {
                have_audio_dec = true;
            }
        }

        let nb_streams = unsafe { (*demux.in_fmt_ctx).nb_streams };
        let mut demux_streams: Vec<DemuxStreamParamter> = Vec::with_capacity(nb_streams as usize);
        for i in 0..nb_streams {
            let stream = demux.get_stream(i as usize);
            demux_streams.push(DemuxStreamParamter::new(stream))
        }


        Self {
            dsts_finished,
            have_audio_dec,
            wallclock_start: 0,
            ts_offset_discont: 0,
            last_ts: 0,
            start_time_effective: demux.start_time_effective,
            ts_offset: demux.ts_offset,
            readrate: demux.readrate,
            start_time_us: demux.start_time_us,
            recording_time_us: demux.recording_time_us,
            exit_on_error: demux.exit_on_error.unwrap_or(false),
            stream_loop: demux.stream_loop.unwrap_or(0),

            end_pts: Default::default(),

            duration: Timestamp {
                ts: 0,
                tb: AVRational { num: 1, den: 1 },
            },
            min_pts: Default::default(),
            max_pts: Default::default(),
            demux_streams,
            dsts,
        }
    }
}

#[derive(Clone)]
struct Timestamp {
    ts: i64,
    tb: AVRational,
}

impl Default for Timestamp {
    fn default() -> Self {
        Self {
            ts: AV_NOPTS_VALUE,
            tb: AVRational { num: 1, den: 1 },
        }
    }
}

unsafe fn seek_to_start(
    demux_paramter: &mut DemuxerParamter,
    in_fmt_ctx: *mut AVFormatContext,
) -> i32 {
    let start_time = demux_paramter.start_time_us.unwrap_or(0);
    let ret = avformat_seek_file(in_fmt_ctx, -1, i64::MIN, start_time, start_time, 0);
    if ret < 0 {
        return ret;
    }

    if demux_paramter.end_pts.ts != AV_NOPTS_VALUE && demux_paramter.max_pts.ts == AV_NOPTS_VALUE
        || av_compare_ts(
            demux_paramter.max_pts.ts,
            demux_paramter.max_pts.tb,
            demux_paramter.end_pts.ts,
            demux_paramter.end_pts.tb,
        ) < 0
    {
        demux_paramter.max_pts = demux_paramter.end_pts.clone();
    }

    if demux_paramter.max_pts.ts != AV_NOPTS_VALUE {
        let min_pts = if demux_paramter.min_pts.ts == AV_NOPTS_VALUE {
            0
        } else {
            demux_paramter.min_pts.ts
        };
        demux_paramter.duration.ts = demux_paramter.max_pts.ts
            - av_rescale_q(
                min_pts,
                demux_paramter.min_pts.tb,
                demux_paramter.max_pts.tb,
            );
    }
    demux_paramter.duration.tb = demux_paramter.max_pts.tb;

    if demux_paramter.stream_loop > 0 {
        demux_paramter.stream_loop -= 1;
    }

    let loop_status = if demux_paramter.stream_loop > 0 {
        format!("Remaining loops: {}", demux_paramter.stream_loop)
    } else if demux_paramter.stream_loop == 0 {
        "Last loop".to_string()
    } else {
        "Infinite loop mode".to_string()
    };

    debug!("Repositioning stream to starting point: position={start_time}μs, {loop_status}");

    ret
}

unsafe fn demux_send(
    demux_paramter: &mut DemuxerParamter,
    packet_box: PacketBox,
    packet_pool: &ObjPool<Packet>,
    flags: usize,
    demux_node: &Arc<SchNode>,
    scheduler_status: &Arc<AtomicUsize>,
    independent_readrate: bool,
) -> i32 {
    let node = demux_node.as_ref();
    let SchNode::Demux {
        waiter, ..
    } = node else { unreachable!() };
    let wait_time = waiter.wait_with_scheduler_status(scheduler_status, independent_readrate);
    if is_stopping(wait_until_not_paused(scheduler_status)) {
        return ffmpeg_sys_next::AVERROR_EXIT;
    }
    if independent_readrate && wait_time != 0 {
        if let Some(readrate) = demux_paramter.readrate {
            if readrate != 0.0 {
                let fix_wallclock_start = demux_paramter.wallclock_start + wait_time;
                debug!("FFmpeg on-demand scheduling caused the initial wallclock_start to not meet the specified readrate:{readrate}. Adjusting wallclock_start from {} to {fix_wallclock_start}",
                    demux_paramter.wallclock_start);
                demux_paramter.wallclock_start = fix_wallclock_start;
            }
        }
    }

    // flush the downstreams after seek
    if (*packet_box.packet.as_ptr()).stream_index == -1 {
        packet_pool.release(packet_box.packet);
        return demux_flush(packet_pool, &demux_paramter.dsts);
    }

    demux_send_for_stream(demux_paramter, packet_box, packet_pool, flags, scheduler_status)
}

unsafe fn demux_send_for_stream(
    demux_paramter: &mut DemuxerParamter,
    packet_box: PacketBox,
    packet_pool: &ObjPool<Packet>,
    flags: usize,
    scheduler_status: &Arc<AtomicUsize>
) -> i32 {
    let stream_index = (*packet_box.packet.as_ptr()).stream_index;

    let send_dsts = demux_paramter
        .dsts
        .iter()
        .enumerate()
        .filter(
            |(_i, (_packet_dst, input_stream_index, _output_stream_index))| {
                *input_stream_index == stream_index as usize
            },
        )
        .collect::<Vec<_>>();

    let mut nb_done = 0;

    for (i, (dst_i, (packet_dst, _, output_stream_index))) in send_dsts.iter().enumerate() {
        let dst_finished = &mut demux_paramter.dsts_finished[*dst_i];

        if i < send_dsts.len() - 1 {
            let Ok(mut to_send) = packet_pool.get() else {
                return AVERROR(ffmpeg_sys_next::ENOMEM);
            };

            let packet_data = packet_box.packet_data.clone();

            let mut ret = av_packet_ref(to_send.as_mut_ptr(), packet_box.packet.as_ptr());
            if ret < 0 {
                return ret;
            }

            let packet_box = PacketBox {
                packet: to_send,
                packet_data,
            };

            ret = demux_stream_send_to_dst(
                packet_box,
                packet_dst,
                output_stream_index,
                dst_finished,
                flags,
                scheduler_status
            );
            if ret == AVERROR_EOF {
                nb_done += 1;
            } else if ret < 0 {
                return ret;
            }
        } else {
            let ret = demux_stream_send_to_dst(
                packet_box,
                packet_dst,
                output_stream_index,
                dst_finished,
                flags,
                scheduler_status
            );
            if ret == AVERROR_EOF {
                nb_done += 1;
            } else if ret < 0 {
                return ret;
            }
            break;
        }
    }

    if nb_done == demux_paramter.dsts.len() {
        AVERROR_EOF
    } else {
        0
    }
}

const DEMUX_SEND_STREAMCOPY_EOF: usize = 1 << 0;

unsafe fn demux_stream_send_to_dst(
    mut packet_box: PacketBox,
    packet_dst: &Sender<PacketBox>,
    output_stream_index: &Option<usize>,
    dst_finished: &mut bool,
    flags: usize,
    scheduler_status: &Arc<AtomicUsize>
) -> i32 {
    if *dst_finished {
        return AVERROR_EOF;
    }

    if !packet_is_null(&packet_box.packet)
        && output_stream_index.is_some()
        && (flags & DEMUX_SEND_STREAMCOPY_EOF) != 0
    {
        unsafe {
            (*packet_box.packet.as_mut_ptr()).stream_index = -1;
        }
        *dst_finished = true;
    }

    if let Some(output_stream_index) = output_stream_index {
        (*packet_box.packet.as_mut_ptr()).stream_index = *output_stream_index as i32;
        packet_box.packet_data.output_stream_index = *output_stream_index as i32;
        packet_box.packet_data.is_copy = true;
    }

    if *dst_finished {
        if let Err(_) = packet_dst.send(packet_box) {
            if !is_stopping(wait_until_not_paused(scheduler_status)) {
                error!("Demuxer send packet failed, destination already finished");
            }
        }

        return AVERROR_EOF;
    }

    if let Err(_) = packet_dst.send(packet_box) {
        if !is_stopping(wait_until_not_paused(scheduler_status)) {
            error!("Demuxer send packet failed, destination already finished");
        }

        *dst_finished = true;
        return AVERROR_EOF;
    }

    0
}

unsafe fn demux_flush(
    packet_pool: &ObjPool<Packet>,
    dsts: &Vec<(Sender<PacketBox>, usize, Option<usize>)>,
) -> i32 {
    // let ts = AV_NOPTS_VALUE;
    // let tb = AVRational{ num: 0, den: 0 };
    // let max_end_ts = Timestamp { ts: AV_NOPTS_VALUE, tb: AVRational { num: 0, den: 0 } };

    for (packet_dst, _input_stream_index, output_stream_index) in dsts {
        //only send to decoder
        if output_stream_index.is_some() {
            continue;
        }

        let Ok(mut packet) = packet_pool.get() else {
            return AVERROR(ffmpeg_sys_next::ENOMEM);
        };
        (*packet.as_mut_ptr()).stream_index = -1;

        let packet_box = PacketBox {
            packet,
            packet_data: PacketData {
                dts_est: 0,
                codec_type: AVMediaType::AVMEDIA_TYPE_UNKNOWN,
                output_stream_index: 0,
                is_copy: false,
                codecpar: null_mut(),
            },
        };

        if let Err(_) = packet_dst.send(packet_box) {
            error!("Demuxer send packet failed, destination already finished");
            return AVERROR_EOF;
        }

        //TODO max_end_ts
    }

    0
}
