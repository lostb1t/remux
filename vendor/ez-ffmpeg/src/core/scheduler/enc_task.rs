use crate::core::context::encoder_stream::EncoderStream;
use crate::core::context::obj_pool::ObjPool;
use crate::core::context::{CodecContext, FrameBox, PacketBox, PacketData};
use crate::error::Error::{Encoding, OpenEncoder};
use crate::error::{AllocPacketError, EncodeSubtitleError, EncodingError, EncodingOperationError, OpenEncoderError, OpenEncoderOperationError, OpenOutputError};
use crate::core::scheduler::ffmpeg_scheduler::{
    frame_is_null, is_stopping, packet_is_null, set_scheduler_error, wait_until_not_paused, STATUS_ABORT,
};
use crate::hwaccel::hw_device_get_by_type;
use crate::util::ffmpeg_utils::{av_err2str, hashmap_to_avdictionary};
use crossbeam_channel::{Receiver, RecvTimeoutError, SendError, Sender};
use ffmpeg_next::packet::Mut;
use ffmpeg_next::{Frame, Packet};
use ffmpeg_sys_next::AVCodecID::{
    AV_CODEC_ID_ASS, AV_CODEC_ID_CODEC2, AV_CODEC_ID_DVB_SUBTITLE, AV_CODEC_ID_MJPEG,
};
use ffmpeg_sys_next::AVFieldOrder::{
    AV_FIELD_BB, AV_FIELD_BT, AV_FIELD_PROGRESSIVE, AV_FIELD_TB, AV_FIELD_TT,
};
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_SUBTITLE, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NONE;
use ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_NONE;
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::AVSideDataProps::AV_SIDE_DATA_PROP_GLOBAL;
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::{av_channel_layout_copy, av_frame_side_data_clone, av_frame_side_data_desc, AV_CODEC_FLAG_COPY_OPAQUE, AV_CODEC_FLAG_FRAME_DURATION, AV_FRAME_FLAG_INTERLACED, AV_FRAME_FLAG_TOP_FIELD_FIRST, AV_FRAME_SIDE_DATA_FLAG_UNIQUE};
use ffmpeg_sys_next::{av_add_q, av_buffer_ref, av_compare_ts, av_cpu_max_align, av_frame_copy_props, av_frame_get_buffer, av_frame_ref, av_get_bytes_per_sample, av_get_pix_fmt_name, av_opt_set_dict2, av_pix_fmt_desc_get, av_rescale_q, av_sample_fmt_is_planar, av_samples_copy, av_shrink_packet, avcodec_alloc_context3, avcodec_encode_subtitle, avcodec_get_hw_config, avcodec_open2, avcodec_parameters_from_context, avcodec_receive_packet, avcodec_send_frame, AVBufferRef, AVCodecContext, AVFrame, AVHWFramesContext, AVMediaType, AVRational, AVStream, AVSubtitle, AVERROR, AVERROR_EOF, AVERROR_EXPERIMENTAL, AV_CODEC_CAP_ENCODER_REORDERED_OPAQUE, AV_CODEC_CAP_PARAM_CHANGE, AV_CODEC_FLAG_INTERLACED_DCT, AV_CODEC_FLAG_INTERLACED_ME, AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX, AV_CODEC_HW_CONFIG_METHOD_HW_FRAMES_CTX, AV_NOPTS_VALUE, AV_OPT_SEARCH_CHILDREN, AV_PKT_FLAG_TRUSTED, AV_TIME_BASE_Q, EAGAIN};
use ffmpeg_sys_next::av_mallocz;
use log::{debug, error, info, trace, warn};
use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

#[cfg(feature = "docs-rs")]
pub(crate) fn enc_init(
    mux_idx: usize,
    mut enc_stream: EncoderStream,
    ready_sender: Option<Sender<i32>>,
    start_time_us: Option<i64>,
    recording_time_us: Option<i64>,
    bits_per_raw_sample: Option<i32>,
    max_video_frames: Option<i64>,
    max_audio_frames: Option<i64>,
    max_subtitle_frames: Option<i64>,
    video_codec_opts: &Option<HashMap<CString, CString>>,
    audio_codec_opts: &Option<HashMap<CString, CString>>,
    subtitle_codec_opts: &Option<HashMap<CString, CString>>,
    oformat_flags: i32,
    frame_pool: ObjPool<Frame>,
    packet_pool: ObjPool<Packet>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
pub(crate) fn enc_init(
    mux_idx: usize,
    mut enc_stream: EncoderStream,
    ready_sender: Option<Sender<i32>>,
    start_time_us: Option<i64>,
    recording_time_us: Option<i64>,
    bits_per_raw_sample: Option<i32>,
    max_video_frames: Option<i64>,
    max_audio_frames: Option<i64>,
    max_subtitle_frames: Option<i64>,
    video_codec_opts: &Option<HashMap<CString, CString>>,
    audio_codec_opts: &Option<HashMap<CString, CString>>,
    subtitle_codec_opts: &Option<HashMap<CString, CString>>,
    oformat_flags: i32,
    frame_pool: ObjPool<Frame>,
    packet_pool: ObjPool<Packet>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    let enc_ctx = unsafe { avcodec_alloc_context3(enc_stream.encoder) };
    if enc_ctx.is_null() {
        return Err(OpenEncoder(
            OpenEncoderOperationError::ContextAllocationError(OpenEncoderError::OutOfMemory),
        ));
    }

    if let Some(qscale) = enc_stream.qscale {
        if qscale > 0 {
            unsafe {
                (*enc_ctx).flags |= ffmpeg_sys_next::AV_CODEC_FLAG_QSCALE as i32;
                (*enc_ctx).global_quality = ffmpeg_sys_next::FF_QP2LAMBDA * qscale;
            }
        }
    }

    if oformat_flags & ffmpeg_sys_next::AVFMT_GLOBALHEADER != 0 {
       unsafe { (*enc_ctx).flags |= ffmpeg_sys_next::AV_CODEC_FLAG_GLOBAL_HEADER as i32; }
    }
    let enc_ctx_box = CodecContext::new(enc_ctx);

    let max_frames = get_max_frames(enc_stream.codec_type, max_video_frames, max_audio_frames, max_subtitle_frames);

    set_encoder_opts(&enc_stream, video_codec_opts, audio_codec_opts, subtitle_codec_opts, &enc_ctx_box)?;

    let receiver = enc_stream.take_src();
    let pkt_sender = enc_stream.take_dst();
    let pre_pkt_sender = enc_stream.take_dst_pre();
    let mux_started = enc_stream.take_mux_started();

    let stream_box = enc_stream.stream;
    let stream_index = enc_stream.stream_index;

    let encoder_name = unsafe {std::str::from_utf8_unchecked(CStr::from_ptr((*enc_stream.encoder).name).to_bytes())};

    let result = std::thread::Builder::new().name(format!("encoder{stream_index}:{mux_idx}:{encoder_name}")).spawn(move || unsafe {
        let enc_ctx_box = enc_ctx_box;
        let stream_box = stream_box;

        let mut opened = false;
        let mut finished = false;
        let mut frames_sent = 0;
        let mut samples_sent = 0;

        // audio
        let mut frame_samples = 0;
        let mut align_mask = 0;
        let mut samples_queued = 0;
        let mut audio_frame_queue: VecDeque<FrameBox> = VecDeque::new();
        let mut is_finished = false;

        loop {
            let sync_frame = receive_frame(&mut opened, &receiver, &frame_pool, enc_ctx_box.as_mut_ptr(), stream_box.inner,
                                               &ready_sender, &bits_per_raw_sample, &mut frame_samples, &mut align_mask, &mut samples_queued, &mut audio_frame_queue,
                                               &mut samples_sent, &mut frames_sent, &mut is_finished,
                                               &scheduler_status, &scheduler_result);

            let mut receive_frame_box = match sync_frame {
                SyncFrame::FrameBox(frame_box) => frame_box,
                SyncFrame::Continue => continue,
                SyncFrame::Break => break
            };

            let result = frame_encode(
                enc_ctx_box.as_mut_ptr(),
                receive_frame_box.frame.as_mut_ptr(),
                start_time_us,
                recording_time_us,
                &pkt_sender,
                &pre_pkt_sender,
                &mux_started,
                stream_box.inner,
                &packet_pool,
            );
            frame_pool.release(receive_frame_box.frame);
            if let Err(e) = result {
                error!("Error encoding a frame: {}", e);
                set_scheduler_error(&scheduler_status, &scheduler_result, e);
                break;
            }

            if let Some(max_frames) = max_frames.as_ref() {
                if frames_sent >= *max_frames {
                    debug!("sq: {stream_index} frames_max {max_frames} reached");
                    finished = true;
                    break;
                }
            }

            finished = result.unwrap();
            if finished {
                trace!("Encoder returned EOF, finishing");
                break;
            }
        }

        // Encoder flush logic - handles two different exit scenarios:
        //
        // SCENARIO 1: finished=true (normal completion)
        //   - Encoder received NULL frame from upstream (frame_encode() processed it)
        //   - encode_frame() already called avcodec_send_frame(NULL)
        //   - All buffered packets (B-frames, etc.) have been drained via receive_packet() loop
        //   - Encoder is completely flushed, no additional flush needed
        //
        // SCENARIO 2: finished=false (early exit: abort or disconnect)
        //   - Encoder did NOT receive NULL frame (thread exited early via abort() or Disconnected)
        //   - Encoder buffer may still contain unencoded frames (B-frames waiting for future frames)
        //   - If NOT abort: actively flush encoder to drain buffered frames
        //   - If abort: skip flush (user doesn't care about incomplete output)
        //
        if !finished {
            let status = scheduler_status.load(Ordering::Acquire);

            if status != STATUS_ABORT {
                // Actively flush encoder: send NULL frame and drain all buffered packets
                let enc_ctx = enc_ctx_box.as_mut_ptr();
                let stream = stream_box.inner;

                // Send NULL frame to signal EOF
                let ret = avcodec_send_frame(enc_ctx, null_mut());
                if ret < 0 && ret != AVERROR_EOF {
                    error!("Error flushing encoder: {}", av_err2str(ret));
                } else {
                    // Drain all buffered packets (B-frames, delayed frames, etc.)
                    // Note: These are real packets with data, NOT empty marker packets
                    loop {
                        let packet_result = packet_pool.get();
                        if packet_result.is_err() {
                            error!("Failed to allocate packet for flushing");
                            break;
                        }
                        let mut packet = packet_result.unwrap();
                        let pkt = packet.as_mut_ptr();

                        let ret = avcodec_receive_packet(enc_ctx, pkt);
                        if ret == AVERROR_EOF {
                            trace!("Encoder flushing completed");
                            packet_pool.release(packet);
                            break;
                        }
                        if ret == AVERROR(EAGAIN) {
                            packet_pool.release(packet);
                            break;
                        }
                        if ret < 0 {
                            error!("Error receiving flushed packet: {}", av_err2str(ret));
                            packet_pool.release(packet);
                            break;
                        }

                        (*pkt).time_base = (*enc_ctx).time_base;
                        (*pkt).flags |= AV_PKT_FLAG_TRUSTED;

                        // Send flushed packet to mux (real data packet, not empty marker)
                        if let Err(_) = send_to_mux(PacketBox {
                            packet,
                            packet_data: PacketData {
                                dts_est: 0,
                                codec_type: (*enc_ctx).codec_type,
                                output_stream_index: (*stream).index,
                                is_copy: false,
                                codecpar: (*stream).codecpar,
                            },
                        }, &pkt_sender, &pre_pkt_sender, &mux_started) {
                            error!("send flushed packet failed, mux already finished");
                            break;
                        }
                    }
                }
            } else {
                debug!("Encoder detected abort, skipping flush for stream {}", stream_index);
            }
        }

        // Send empty packet marker to signal stream end to muxer
        //
        // IMPORTANT: This empty packet is REQUIRED in ALL scenarios:
        // - SCENARIO 1 (finished=true): After normal encoding completion
        // - SCENARIO 2 (finished=false, no abort): After flushing buffered frames
        //
        // Why needed after flush? Because flushed packets contain REAL DATA (B-frames, etc.),
        // not empty markers. Muxer uses `packet.is_empty()` to detect stream completion,
        // so we must send an explicit empty packet to signal "no more data from this encoder".
        {
            let enc_ctx = enc_ctx_box.as_mut_ptr();
            let stream = stream_box.inner;

            let mut packet = Packet::empty();
            (*packet.as_mut_ptr()).stream_index = stream_index as i32;
            if let Err(e) = send_to_mux(PacketBox {
                packet,
                packet_data: PacketData {
                    dts_est: 0,
                    codec_type: (*enc_ctx).codec_type,
                    output_stream_index: (*stream).index,
                    is_copy: false,
                    codecpar: (*stream).codecpar,
                },
            }, &pkt_sender, &pre_pkt_sender, &mux_started) {
                error!("Error sending end packet: {}", e);
                set_scheduler_error(
                    &scheduler_status,
                    &scheduler_result,
                    Encoding(EncodingOperationError::MuxerFinished),
                );
            }
        }

        debug!("Encoder finished.");
    });
    if let Err(e) = result {
        error!("Encoder thread exited with error: {e}");
        return Err(OpenEncoderOperationError::ThreadExited.into())
    }

    Ok(())
}

enum SyncFrame {
    FrameBox(FrameBox),
    Continue,
    Break,
}

fn receive_from(
    receiver: &Receiver<FrameBox>,
    scheduler_status: &Arc<AtomicUsize>,
) -> Result<FrameBox, SyncFrame> {
    match receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(frame_box) => {
            if is_stopping(wait_until_not_paused(scheduler_status)) {
                info!("Encoder receiver end command, finishing.");
                return Err(SyncFrame::Break);
            }
            Ok(frame_box)
        },
        Err(e) if e == RecvTimeoutError::Disconnected => {
            debug!("Source[decoder/filtergraph/pipeline] thread exit.");
            Err(SyncFrame::Break)
        }
        Err(_) => Err(SyncFrame::Continue),
    }
}

fn process_audio_queue(
    frame_samples: i32,
    samples_queued: &mut i32,
    audio_frame_queue: &mut VecDeque<FrameBox>,
    frame_pool: &ObjPool<Frame>,
    align_mask: usize,
    samples_sent: &mut i64,
    frames_sent: &mut i64,
    is_finished: &mut bool,
    scheduler_status: &Arc<AtomicUsize>,
    scheduler_result: &Arc<Mutex<Option<crate::error::Result<()>>>>
) -> Result<Option<FrameBox>, ()> {
    if let Some(peek) = audio_frame_queue.front() {
        if frame_samples <= *samples_queued || *is_finished {
            let nb_samples = if *is_finished {
                std::cmp::min(frame_samples, *samples_queued)
            } else {
                frame_samples
            };
            *samples_sent += nb_samples as i64;
            *frames_sent = *samples_sent / frame_samples as i64;

            return if *is_finished && frame_is_null(&peek.frame) {
                Ok(audio_frame_queue.pop_front())
            } else {
                unsafe {
                    if nb_samples != (*peek.frame.as_ptr()).nb_samples
                        || !frame_is_aligned(align_mask, peek.frame.as_ptr())
                    {
                        match receive_samples(
                            samples_queued,
                            audio_frame_queue,
                            nb_samples,
                            frame_pool,
                            align_mask,
                        ) {
                            Err(ret) => {
                                error!("Error receive audio frame: {}", av_err2str(ret));
                                set_scheduler_error(
                                    scheduler_status,
                                    scheduler_result,
                                    Encoding(EncodingOperationError::ReceiveAudioError(
                                        crate::error::EncodingError::from(ret),
                                    )),
                                );
                                Err(())
                            }
                            Ok(frame) => Ok(Some(frame)),
                        }
                    } else {
                        *samples_queued -= (*peek.frame.as_ptr()).nb_samples;
                        Ok(audio_frame_queue.pop_front())
                    }
                }
            }
        }
    }
    Ok(None)
}

fn receive_frame(
    opened: &mut bool,
    receiver: &Receiver<FrameBox>,
    frame_pool: &ObjPool<Frame>,
    enc_ctx: *mut AVCodecContext,
    stream: *mut AVStream,
    ready_sender: &Option<Sender<i32>>,
    bits_per_raw_sample: &Option<i32>,
    frame_samples: &mut i32,
    align_mask: &mut usize,
    samples_queued: &mut i32,
    audio_frame_queue: &mut VecDeque<FrameBox>,
    samples_sent: &mut i64,
    frames_sent: &mut i64,
    is_finished: &mut bool,
    scheduler_status: &Arc<AtomicUsize>,
    scheduler_result: &Arc<Mutex<Option<crate::error::Result<()>>>>
) -> SyncFrame {
    let mut frame_box = if !*opened {
        let mut frame_box = match receive_from(receiver, scheduler_status) {
            Ok(frame) => frame,
            Err(sync) => return sync,
        };

        if frame_is_null(&frame_box.frame) {
            frame_pool.release(frame_box.frame);
            error!("Receive frame is null on open encoder");
            return SyncFrame::Break;
        }

        if let Err(e) = enc_open(enc_ctx, stream, &mut frame_box, ready_sender.clone(), bits_per_raw_sample.clone()) {
            frame_pool.release(frame_box.frame);
            error!("Open encoder error: {e}");
            set_scheduler_error(scheduler_status, scheduler_result, e);
            return SyncFrame::Break;
        }
        *opened = true;
        unsafe {
            if (*enc_ctx).codec_type == AVMEDIA_TYPE_AUDIO {
                *frame_samples = (*enc_ctx).frame_size;
                *align_mask = av_cpu_max_align() - 1;
            }
        }

        frame_box
    } else {
        if *frame_samples > 0 && !audio_frame_queue.is_empty() {
            match process_audio_queue(
                *frame_samples,
                samples_queued,
                audio_frame_queue,
                frame_pool,
                *align_mask,
                samples_sent,
                frames_sent,
                is_finished,
                scheduler_status,
                scheduler_result,
            ) {
                Ok(Some(frame)) => return SyncFrame::FrameBox(frame),
                Err(_) => return SyncFrame::Break,
                _ => {}
            }
        }

        let frame_box = match receive_from(receiver, scheduler_status) {
            Ok(frame) => frame,
            Err(sync) => return sync,
        };

        frame_box
    };

    if *frame_samples > 0 {
        *is_finished = frame_is_null(&frame_box.frame);
        if !*is_finished {
            unsafe {
                (*frame_box.frame.as_mut_ptr()).duration = av_rescale_q(
                    (*frame_box.frame.as_ptr()).nb_samples as i64,
                    AVRational {
                        num: 1,
                        den: (*frame_box.frame.as_ptr()).sample_rate,
                    },
                    (*frame_box.frame.as_ptr()).time_base,
                );
                *samples_queued += (*frame_box.frame.as_ptr()).nb_samples;
            }
        }
        audio_frame_queue.push_back(frame_box);

        if *samples_queued < *frame_samples && !*is_finished {
            return SyncFrame::Continue;
        }
        match process_audio_queue(
            *frame_samples,
            samples_queued,
            audio_frame_queue,
            frame_pool,
            *align_mask,
            samples_sent,
            frames_sent,
            is_finished,
            scheduler_status,
            &scheduler_result,
        ) {
            Ok(Some(frame)) => SyncFrame::FrameBox(frame),
            Ok(None) => SyncFrame::Continue,
            Err(_) => SyncFrame::Break,
        }
    } else {
        *frames_sent += 1;
        SyncFrame::FrameBox(frame_box)
    }
}

fn set_encoder_opts(enc_stream: &EncoderStream, video_codec_opts: &Option<HashMap<CString, CString>>, audio_codec_opts: &Option<HashMap<CString, CString>>, subtitle_codec_opts: &Option<HashMap<CString, CString>>, enc_ctx_box: &CodecContext) -> crate::error::Result<()> {
    let mut encoder_opts = if enc_stream.codec_type == AVMEDIA_TYPE_VIDEO {
        hashmap_to_avdictionary(video_codec_opts)
    } else if enc_stream.codec_type == AVMEDIA_TYPE_AUDIO {
        hashmap_to_avdictionary(audio_codec_opts)
    } else if enc_stream.codec_type == AVMEDIA_TYPE_SUBTITLE {
        hashmap_to_avdictionary(subtitle_codec_opts)
    } else {
        null_mut()
    };
    if !encoder_opts.is_null() {
        let ret = unsafe {
            av_opt_set_dict2(
                enc_ctx_box.as_mut_ptr() as *mut libc::c_void,
                &mut encoder_opts,
                AV_OPT_SEARCH_CHILDREN,
            )
        };
        if ret < 0 {
            error!("Error applying encoder options: {}", av_err2str(ret));
            return Err(OpenEncoder(
                OpenEncoderOperationError::ContextAllocationError(OpenEncoderError::from(ret)),
            ));
        }
    }
    Ok(())
}

fn get_max_frames(codec_type: AVMediaType, max_video_frames: Option<i64>, max_audio_frames: Option<i64>, max_subtitle_frames: Option<i64>) -> Option<i64> {
    if codec_type == AVMEDIA_TYPE_VIDEO {
        max_video_frames
    } else if codec_type == AVMEDIA_TYPE_AUDIO {
        max_audio_frames
    } else if codec_type == AVMEDIA_TYPE_SUBTITLE {
        max_subtitle_frames
    } else {
        None
    }
}

#[cfg(feature = "docs-rs")]
unsafe fn receive_samples(
    samples_queued: &mut i32,
    audio_frame_queue: &mut VecDeque<FrameBox>,
    nb_samples: i32,
    frame_pool: &ObjPool<Frame>,
    align_mask: usize,
) -> Result<FrameBox, i32> {
    Err(ffmpeg_sys_next::AVERROR_BUG)
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn receive_samples(
    samples_queued: &mut i32,
    audio_frame_queue: &mut VecDeque<FrameBox>,
    nb_samples: i32,
    frame_pool: &ObjPool<Frame>,
    align_mask: usize,
) -> Result<FrameBox, i32> {
    assert!(*samples_queued >= nb_samples);

    let Ok(mut dst) = frame_pool.get() else {
        return Err(AVERROR(ffmpeg_sys_next::ENOMEM));
    };

    let mut src_box = audio_frame_queue.front_mut().unwrap();
    let src = &mut src_box.frame;

    if (*src.as_ptr()).nb_samples > nb_samples && frame_is_aligned(align_mask, src.as_ptr()) {
        let ret = av_frame_ref(dst.as_mut_ptr(), src.as_ptr());
        if ret < 0 {
            frame_pool.release(dst);
            return Err(ret);
        }

        (*dst.as_mut_ptr()).nb_samples = nb_samples;
        offset_audio(src.as_mut_ptr(), nb_samples);
        *samples_queued -= nb_samples;

        (*dst.as_mut_ptr()).duration = av_rescale_q(
            nb_samples as i64,
            AVRational {
                num: 1,
                den: (*dst.as_ptr()).sample_rate,
            },
            (*dst.as_ptr()).time_base,
        );

        let frame_data = src_box.frame_data.clone();
        return Ok(FrameBox {
            frame: dst,
            frame_data,
        });
    }

    // otherwise allocate a new frame and copy the data
    let mut ret = av_channel_layout_copy(
        &mut (*dst.as_mut_ptr()).ch_layout,
        &(*src.as_ptr()).ch_layout,
    );
    if ret < 0 {
        frame_pool.release(dst);
        return Err(ret);
    }
    (*dst.as_mut_ptr()).format = (*src.as_ptr()).format;
    (*dst.as_mut_ptr()).nb_samples = nb_samples;

    ret = av_frame_get_buffer(dst.as_mut_ptr(), 0);
    if ret < 0 {
        frame_pool.release(dst);
        return Err(ret);
    }

    ret = av_frame_copy_props(dst.as_mut_ptr(), src.as_ptr());
    if ret < 0 {
        frame_pool.release(dst);
        return Err(ret);
    }

    let frame_data = src_box.frame_data.clone();

    (*dst.as_mut_ptr()).nb_samples = 0;
    while (*dst.as_ptr()).nb_samples < nb_samples {
        src_box = audio_frame_queue.front_mut().unwrap();
        let src = &mut src_box.frame;

        let to_copy = std::cmp::min(
            nb_samples - (*dst.as_ptr()).nb_samples,
            (*src.as_ptr()).nb_samples,
        );

        av_samples_copy(
            (*dst.as_ptr()).extended_data,
            (*src.as_ptr()).extended_data,
            (*dst.as_ptr()).nb_samples,
            0,
            to_copy,
            (*dst.as_ptr()).ch_layout.nb_channels,
            std::mem::transmute((*dst.as_ptr()).format),
        );

        if to_copy < (*src.as_ptr()).nb_samples {
            offset_audio(src.as_mut_ptr(), to_copy);
        } else {
            audio_frame_queue.pop_front();
        }

        *samples_queued -= to_copy;
        (*dst.as_mut_ptr()).nb_samples += to_copy;
    }

    (*dst.as_mut_ptr()).duration = av_rescale_q(
        nb_samples as i64,
        AVRational {
            num: 1,
            den: (*dst.as_ptr()).sample_rate,
        },
        (*dst.as_ptr()).time_base,
    );

    Ok(FrameBox {
        frame: dst,
        frame_data,
    })
}

#[cfg(feature = "docs-rs")]
fn enc_open(
    enc_ctx: *mut AVCodecContext,
    stream: *mut AVStream,
    frame_box: &mut FrameBox,
    ready_sender: Option<Sender<i32>>,
    bits_per_raw_sample: Option<i32>,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
fn enc_open(
    enc_ctx: *mut AVCodecContext,
    stream: *mut AVStream,
    frame_box: &mut FrameBox,
    ready_sender: Option<Sender<i32>>,
    bits_per_raw_sample: Option<i32>,
) -> crate::error::Result<()> {
    unsafe {
        let enc = (*enc_ctx).codec;

        let frame = frame_box.frame.as_mut_ptr();
        if (*enc_ctx).codec_type == AVMEDIA_TYPE_VIDEO
            || (*enc_ctx).codec_type == AVMEDIA_TYPE_AUDIO
        {
            for i in 0..(*frame).nb_side_data {
                let desc = av_frame_side_data_desc((**(*frame).side_data.offset(i as isize)).type_);

                if ((*desc).props & AV_SIDE_DATA_PROP_GLOBAL as u32) == 0 {
                    continue;
                }

                let ret = av_frame_side_data_clone(
                    &mut (*enc_ctx).decoded_side_data,
                    &mut (*enc_ctx).nb_decoded_side_data,
                    *(*frame).side_data.offset(i as isize),
                    AV_FRAME_SIDE_DATA_FLAG_UNIQUE as u32,
                );
                if ret < 0 {
                    return Err(OpenEncoder(
                        OpenEncoderOperationError::FrameSideDataCloneError(
                            OpenEncoderError::OutOfMemory,
                        ),
                    ));
                }
            }

            (*enc_ctx).time_base = (*frame).time_base;
            if let Some(framerate) = frame_box.frame_data.framerate {
                (*enc_ctx).framerate = framerate;
                (*stream).avg_frame_rate = framerate;
            }
        }

        match (*enc_ctx).codec_type {
            AVMEDIA_TYPE_AUDIO => {
                assert!(
                    (*frame).format != AV_SAMPLE_FMT_NONE as i32
                        && (*frame).sample_rate > 0
                        && (*frame).ch_layout.nb_channels > 0
                );

                if (*frame).format == -1 {
                    return Err(OpenOutputError::UnknownFrameFormat.into());
                }
                (*enc_ctx).sample_fmt = std::mem::transmute((*frame).format);
                (*enc_ctx).sample_rate = (*frame).sample_rate;
                let ret = av_channel_layout_copy(&mut (*enc_ctx).ch_layout, &(*frame).ch_layout);
                if ret < 0 {
                    return Err(OpenEncoder(
                        OpenEncoderOperationError::ChannelLayoutCopyError(
                            OpenEncoderError::OutOfMemory,
                        ),
                    ));
                }

                if let Some(bits_per_raw_sample) = bits_per_raw_sample {
                    (*enc_ctx).bits_per_raw_sample = bits_per_raw_sample;
                } else {
                    (*enc_ctx).bits_per_raw_sample = std::cmp::min(
                        frame_box.frame_data.bits_per_raw_sample,
                        av_get_bytes_per_sample((*enc_ctx).sample_fmt) << 3,
                    );
                }
            }
            AVMEDIA_TYPE_VIDEO => {
                assert!(
                    (*frame).format != AV_SAMPLE_FMT_NONE as i32
                        && (*frame).width > 0
                        && (*frame).height > 0
                );

                (*enc_ctx).width = (*frame).width;
                (*enc_ctx).height = (*frame).height;
                (*enc_ctx).sample_aspect_ratio = (*frame).sample_aspect_ratio;
                (*stream).sample_aspect_ratio = (*frame).sample_aspect_ratio;

                if (*frame).format == -1 {
                    return Err(OpenEncoder(OpenEncoderOperationError::UnknownFrameFormat));
                }
                (*enc_ctx).pix_fmt = std::mem::transmute((*frame).format);

                if let Some(bits_per_raw_sample) = bits_per_raw_sample {
                    (*enc_ctx).bits_per_raw_sample = bits_per_raw_sample;
                } else {
                    (*enc_ctx).bits_per_raw_sample = std::cmp::min(
                        frame_box.frame_data.bits_per_raw_sample,
                        (*av_pix_fmt_desc_get((*enc_ctx).pix_fmt)).comp[0].depth,
                    );
                }

                (*enc_ctx).color_range = (*frame).color_range;
                (*enc_ctx).color_primaries = (*frame).color_primaries;
                (*enc_ctx).color_trc = (*frame).color_trc;
                (*enc_ctx).colorspace = (*frame).colorspace;
                (*enc_ctx).chroma_sample_location = (*frame).chroma_location;

                if ((*enc_ctx).flags as u32
                    & (AV_CODEC_FLAG_INTERLACED_DCT | AV_CODEC_FLAG_INTERLACED_ME))
                    != 0
                    || ((*frame).flags & AV_FRAME_FLAG_INTERLACED) != 0
                {
                    let top_field_first = if ((*frame).flags & AV_FRAME_FLAG_TOP_FIELD_FIRST) != 0 {
                        true
                    } else {
                        false
                    };

                    if (*enc).id == AV_CODEC_ID_MJPEG {
                        (*enc_ctx).field_order = if top_field_first {
                            AV_FIELD_TT
                        } else {
                            AV_FIELD_BB
                        };
                    } else {
                        (*enc_ctx).field_order = if top_field_first {
                            AV_FIELD_TB
                        } else {
                            AV_FIELD_BT
                        };
                    }
                } else {
                    (*enc_ctx).field_order = AV_FIELD_PROGRESSIVE;
                }
            }
            AVMEDIA_TYPE_SUBTITLE => {
                (*enc_ctx).time_base = AV_TIME_BASE_Q;

                if (*enc_ctx).width == 0 {
                    (*enc_ctx).width = frame_box.frame_data.input_stream_width;
                    (*enc_ctx).height = frame_box.frame_data.input_stream_height;
                }

                if !frame_box.frame_data.subtitle_header.is_null() {
                    /* ASS code assumes this buffer is null terminated so add extra byte. */
                    let size = (frame_box.frame_data.subtitle_header_size + 1) as usize;
                    let subtitle_header = av_mallocz(size) as *mut u8;
                    (*enc_ctx).subtitle_header = subtitle_header;
                    if (*enc_ctx).subtitle_header.is_null() {
                        return Err(OpenEncoder(
                            OpenEncoderOperationError::SettingSubtitleError(
                                OpenEncoderError::OutOfMemory,
                            ),
                        ));
                    }
                    std::ptr::copy_nonoverlapping(
                        frame_box.frame_data.subtitle_header as *const u8,
                        (*enc_ctx).subtitle_header,
                        frame_box.frame_data.subtitle_header_size as usize,
                    );
                    (*enc_ctx).subtitle_header_size = frame_box.frame_data.subtitle_header_size;
                }
            }
            _ => panic!("Unsupported codec type"),
        }

        if (*enc).capabilities as u32 & AV_CODEC_CAP_ENCODER_REORDERED_OPAQUE != 0 {
            (*enc_ctx).flags |= AV_CODEC_FLAG_COPY_OPAQUE as i32;
        }
        (*enc_ctx).flags |= AV_CODEC_FLAG_FRAME_DURATION as i32;

        let frames_ref = if (*enc_ctx).codec_type == AVMEDIA_TYPE_VIDEO
            || (*enc_ctx).codec_type == AVMEDIA_TYPE_AUDIO
        {
            (*frame).hw_frames_ctx
        } else {
            null_mut()
        };
        let ret = hw_device_setup_for_encode(enc_ctx, frames_ref);
        if ret < 0 {
            error!("Encoding hardware device setup failed");
            return Err(OpenEncoder(OpenEncoderOperationError::HwSetupError(
                OpenEncoderError::OutOfMemory,
            )));
        }

        let ret = avcodec_open2(enc_ctx, enc, null_mut());
        if ret < 0 {
            if ret != AVERROR_EXPERIMENTAL {
                error!("Error while opening encoder - maybe incorrect parameters such as bit_rate, rate, width or height.");
            }
            return Err(OpenEncoder(OpenEncoderOperationError::CodecOpenError(
                OpenEncoderError::OutOfMemory,
            )));
        }

        if (*enc_ctx).bit_rate != 0
            && (*enc_ctx).bit_rate < 1000
            && (*enc_ctx).codec_id != AV_CODEC_ID_CODEC2
        /* don't complain about 700 bit/s modes */
        {
            warn!("The bitrate parameter is set too low. It takes bits/s as argument, not kbits/s");
        }

        let ret = avcodec_parameters_from_context((*stream).codecpar, enc_ctx);
        if ret < 0 {
            error!("Error initializing the output stream codec context.");
            return Err(OpenEncoder(
                OpenEncoderOperationError::CodecParametersError(OpenEncoderError::OutOfMemory),
            ));
        }

        // copy timebase while removing common factors
        if (*stream).time_base.num <= 0 || (*stream).time_base.den <= 0 {
            (*stream).time_base = av_add_q((*enc_ctx).time_base, AVRational { num: 0, den: 1 });
        }

        if let Some(ready_sender) = ready_sender {
            let _ = ready_sender.send((*stream).index);
        }
    }

    Ok(())
}

unsafe fn hw_device_setup_for_encode(
    enc_ctx: *mut AVCodecContext,
    mut frames_ref: *mut AVBufferRef,
) -> i32 {
    let mut dev = None;

    if !frames_ref.is_null()
        && (*((*frames_ref).data as *mut AVHWFramesContext)).format == (*enc_ctx).pix_fmt
    {
        // Matching format, will try to use hw_frames_ctx.
    } else {
        frames_ref = null_mut();
    }

    let mut i = 0;
    loop {
        let config = avcodec_get_hw_config((*enc_ctx).codec, i);
        if config.is_null() {
            break;
        }

        if !frames_ref.is_null()
            && (*config).methods & AV_CODEC_HW_CONFIG_METHOD_HW_FRAMES_CTX as i32 != 0
            && ((*config).pix_fmt == AV_PIX_FMT_NONE || (*config).pix_fmt == (*enc_ctx).pix_fmt)
        {
            trace!(
                "Using input frames context (format {}) with {} encoder.",
                CStr::from_ptr(av_get_pix_fmt_name((*enc_ctx).pix_fmt))
                    .to_str()
                    .unwrap_or("[unknow / Invalid UTF-8]"),
                CStr::from_ptr((*(*enc_ctx).codec).name)
                    .to_str()
                    .unwrap_or("[unknow codec / Invalid UTF-8]")
            );
            (*enc_ctx).hw_frames_ctx = av_buffer_ref(frames_ref);
            if (*enc_ctx).hw_frames_ctx.is_null() {
                return AVERROR(ffmpeg_sys_next::ENOMEM);
            }
            return 0;
        }

        if dev.is_none() && (*config).methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
        {
            dev = hw_device_get_by_type((*config).device_type);
        }

        i += 1;
    }

    if let Some(dev) = dev {
        trace!(
            "Using device %s (type {}) with {} encoder.",
            dev.name,
            CStr::from_ptr((*(*enc_ctx).codec).name)
                .to_str()
                .unwrap_or("[unknow codec / Invalid UTF-8]")
        );
        (*enc_ctx).hw_device_ctx = av_buffer_ref(dev.device_ref);
        if (*enc_ctx).hw_device_ctx.is_null() {
            return AVERROR(ffmpeg_sys_next::ENOMEM);
        }
    }

    0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn offset_audio(f: *mut AVFrame, nb_samples: i32) {
    let planar = av_sample_fmt_is_planar(std::mem::transmute((*f).format));
    let planes = if planar != 0 {
        (*f).ch_layout.nb_channels
    } else {
        1
    };
    let bps = av_get_bytes_per_sample(std::mem::transmute((*f).format));
    let offset = (nb_samples
        * bps
        * if planar != 0 {
            1
        } else {
            (*f).ch_layout.nb_channels
        }) as usize;

    assert!(bps > 0);
    assert!(nb_samples < (*f).nb_samples);

    for i in 0..planes as usize {
        std::ptr::write(
            (*f).extended_data.add(i),
            (*(*f).extended_data.add(i)).add(offset),
        );
        if i < (*f).data.len() {
            *(*f).data.get_unchecked_mut(i) = *(*f).extended_data.add(i);
        }
    }

    (*f).linesize[0] -= offset as i32;
    (*f).nb_samples -= nb_samples;

    (*f).duration = av_rescale_q(
        (*f).nb_samples as i64,
        AVRational {
            num: 1,
            den: (*f).sample_rate,
        },
        (*f).time_base,
    );

    (*f).pts += av_rescale_q(
        nb_samples as i64,
        AVRational {
            num: 1,
            den: (*f).sample_rate,
        },
        (*f).time_base,
    );
}

unsafe fn frame_is_aligned(align_mask: usize, frame: *const AVFrame) -> bool {
    assert!((*frame).nb_samples > 0);
    assert!(align_mask > 0);

    let data_ptr = (*frame).data[0] as usize;
    let linesize = (*frame).linesize[0] as usize;

    if (data_ptr & align_mask) == 0 && (linesize & align_mask) == 0 && linesize > align_mask {
        return true;
    }

    false
}

#[cfg(not(feature = "docs-rs"))]
fn frame_encode(
    enc_ctx: *mut AVCodecContext,
    frame: *mut AVFrame,
    start_time_us: Option<i64>,
    recording_time_us: Option<i64>,
    pkt_sender: &Sender<PacketBox>,
    pre_pkt_sender: &Sender<PacketBox>,
    mux_started: &Arc<AtomicBool>,
    stream: *mut AVStream,
    packet_pool: &ObjPool<Packet>,
) -> crate::error::Result<bool> {
    unsafe {
        if (*enc_ctx).codec_type == AVMEDIA_TYPE_SUBTITLE {
            let subtitle = if !frame.is_null() && !(*frame).buf[0].is_null() {
                (*(*frame).buf[0]).data as *const AVSubtitle
            } else {
                null()
            };

            return if !subtitle.is_null() && (*subtitle).num_rects != 0 {
                do_subtitle_out(
                    enc_ctx,
                    subtitle,
                    start_time_us,
                    recording_time_us,
                    pkt_sender,
                    pre_pkt_sender,
                    mux_started,
                    stream,
                )
            } else {
                Ok(false)
            };
        }

        if !frame.is_null() {
            if let Some(recording_time_us) = recording_time_us {
                if av_compare_ts(
                    (*frame).pts,
                    (*frame).time_base,
                    recording_time_us,
                    AV_TIME_BASE_Q,
                ) >= 0
                {
                    debug!("Reached the target time: {recording_time_us} us, frame time: {} us. Ending the recording.", (*frame).pts);
                    return Ok(true);
                }
            }

            if (*enc_ctx).codec_type == AVMEDIA_TYPE_VIDEO {
                (*frame).quality = (*enc_ctx).global_quality;
                (*frame).pict_type = AV_PICTURE_TYPE_NONE;
            } else {
                if (*(*enc_ctx).codec).capabilities & AV_CODEC_CAP_PARAM_CHANGE as i32 == 0
                    && (*enc_ctx).ch_layout.nb_channels != (*frame).ch_layout.nb_channels
                {
                    error!("Audio channel count changed and encoder does not support parameter changes");
                    return Ok(false);
                }
            }
        }
        encode_frame(enc_ctx, frame, pkt_sender,  pre_pkt_sender, mux_started, stream, packet_pool)
    }
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn do_subtitle_out(
    enc_ctx: *mut AVCodecContext,
    sub: *const AVSubtitle,
    start_time_us: Option<i64>,
    recording_time_us: Option<i64>,
    pkt_sender: &Sender<PacketBox>,
    pre_pkt_sender: &Sender<PacketBox>,
    mux_started: &Arc<AtomicBool>,
    stream: *mut AVStream,
) -> crate::error::Result<bool> {
    let subtitle_out_max_size = 1024 * 1024;
    if (*sub).pts == AV_NOPTS_VALUE {
        return Err(Encoding(EncodingOperationError::SubtitleNotPts));
    }
    if let Some(start_time_us) = start_time_us {
        if (*sub).pts < start_time_us {
            return Ok(false);
        }
    }

    let nb = if (*enc_ctx).codec_id == AV_CODEC_ID_DVB_SUBTITLE {
        2
    } else if (*enc_ctx).codec_id == AV_CODEC_ID_ASS {
        std::cmp::max((*sub).num_rects, 1)
    } else {
        1
    };

    let mut pts = (*sub).pts;
    if let Some(start_time_us) = start_time_us {
        pts -= start_time_us;
    }
    for i in 0..nb {
        let mut local_sub = *sub;
        if let Some(recording_time_us) = recording_time_us {
            if av_compare_ts(pts, AV_TIME_BASE_Q, recording_time_us, AV_TIME_BASE_Q) >= 0 {
                return Ok(true);
            }
        }

        let mut packet = Packet::new(subtitle_out_max_size);
        if packet_is_null(&packet) {
            return Err(Encoding(EncodingOperationError::AllocPacket(
                AllocPacketError::OutOfMemory,
            )));
        }
        let pkt = packet.as_mut_ptr();

        local_sub.pts = pts;
        // start_display_time is required to be 0
        local_sub.pts += av_rescale_q(
            (*sub).start_display_time as i64,
            AVRational { num: 1, den: 1000 },
            AV_TIME_BASE_Q,
        );
        local_sub.end_display_time -= (*sub).start_display_time;
        local_sub.start_display_time = 0;

        if (*enc_ctx).codec_id == AV_CODEC_ID_DVB_SUBTITLE && i == 1 {
            local_sub.num_rects = 0;
        } else if (*enc_ctx).codec_id == AV_CODEC_ID_ASS && (*sub).num_rects > 0 {
            local_sub.num_rects = 1;
            local_sub.rects = local_sub.rects.add(i as usize);
        }

        let subtitle_out_size =
            avcodec_encode_subtitle(enc_ctx, (*pkt).data, (*pkt).size, &local_sub);
        if subtitle_out_size < 0 {
            error!("Subtitle encoding failed");
            return Err(Encoding(EncodingOperationError::EncodeSubtitle(
                EncodeSubtitleError::from(subtitle_out_size),
            )));
        }

        av_shrink_packet(pkt, subtitle_out_size);

        (*pkt).time_base = AV_TIME_BASE_Q;
        (*pkt).pts = (*sub).pts;
        (*pkt).duration = av_rescale_q(
            (*sub).end_display_time as i64,
            AVRational { num: 1, den: 1000 },
            (*pkt).time_base,
        );
        if (*enc_ctx).codec_id == AV_CODEC_ID_DVB_SUBTITLE {
            /* XXX: the pts correction is handled here. Maybe handling
            it in the codec would be better */
            if i == 0 {
                (*pkt).pts += av_rescale_q(
                    (*sub).start_display_time as i64,
                    AVRational { num: 1, den: 1000 },
                    (*pkt).time_base,
                );
            } else {
                (*pkt).pts += av_rescale_q(
                    (*sub).end_display_time as i64,
                    AVRational { num: 1, den: 1000 },
                    (*pkt).time_base,
                );
            }
        }
        (*pkt).dts = (*pkt).pts;

        if let Err(_) = send_to_mux(PacketBox {
            packet,
            packet_data: PacketData {
                dts_est: 0,
                codec_type: (*enc_ctx).codec_type,
                output_stream_index: (*stream).index,
                is_copy: false,
                codecpar: (*stream).codecpar,
            },
        }, pkt_sender, pre_pkt_sender, mux_started) {
            error!("send subtitle packet failed, mux already finished");
            return Err(Encoding(EncodingOperationError::MuxerFinished));
        }
    }

    Ok(false)
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn encode_frame(
    enc_ctx: *mut AVCodecContext,
    frame: *mut AVFrame,
    pkt_sender: &Sender<PacketBox>,
    pre_pkt_sender: &Sender<PacketBox>,
    mux_started: &Arc<AtomicBool>,
    stream: *mut AVStream,
    packet_pool: &ObjPool<Packet>,
) -> crate::error::Result<bool> {
    if !frame.is_null() {
        if (*frame).sample_aspect_ratio.num != 0 {
            (*enc_ctx).sample_aspect_ratio = (*frame).sample_aspect_ratio;
        }
    }

    let ret = avcodec_send_frame(enc_ctx, frame);
    if ret < 0 && !(ret == AVERROR_EOF && frame.is_null()) {
        if ret == AVERROR_EOF && frame.is_null(){
            trace!("EOF reached, no more frames to encode.");
        } else {
            error!(
            "Error submitting {:?} frame to the encoder",
            (*enc_ctx).codec_type
        );
            return Err(Encoding(EncodingOperationError::SendFrameError(
                EncodingError::from(ret),
            )));
        }
    }

    loop {
        let mut packet = packet_pool.get()?;
        let pkt = packet.as_mut_ptr();

        let ret = avcodec_receive_packet(enc_ctx, pkt);

        (*pkt).time_base = (*enc_ctx).time_base;

        if ret == AVERROR(EAGAIN) {
            return Ok(false);
        } else if ret < 0 {
            if ret == AVERROR_EOF {
                trace!("EOF reached. No more packets to receive.");
                return Ok(true);
            }
            error!("{:?} encoding failed", (*enc_ctx).codec_type);
            return Err(Encoding(EncodingOperationError::ReceivePacketError(
                EncodingError::from(ret),
            )));
        }

        (*pkt).flags |= AV_PKT_FLAG_TRUSTED;

        if let Err(_) = send_to_mux(PacketBox {
            packet,
            packet_data: PacketData {
                dts_est: 0,
                codec_type: (*enc_ctx).codec_type,
                output_stream_index: (*stream).index,
                is_copy: false,
                codecpar: (*stream).codecpar,
            },
        }, pkt_sender, pre_pkt_sender, mux_started) {
            error!("send packet failed, mux already finished");
            return Err(Encoding(EncodingOperationError::MuxerFinished));
        }
    }
}


fn send_to_mux(packet_box: PacketBox, pkt_sender: &Sender<PacketBox>, pre_pkt_sender: &Sender<PacketBox>, mux_started:&Arc<AtomicBool>) -> Result<(), SendError<PacketBox>> {
    if mux_started.load(Ordering::Acquire) {
        pkt_sender.send(packet_box)
    } else {
        if let Err(e) = pre_pkt_sender.send(packet_box) {
            for _ in 0..64 {
                if mux_started.load(Ordering::Acquire) {
                    return pkt_sender.send(e.0);
                }
                sleep(Duration::from_millis(100));
            }
            return Err(e);
        }
        Ok(())
    }
}