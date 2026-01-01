use crate::core::context::decoder_stream::DecoderStream;
use crate::core::context::obj_pool::ObjPool;
use crate::core::context::{null_frame, CodecContext, FrameBox, FrameData, PacketBox};
use crate::core::scheduler::ffmpeg_scheduler::{
    is_stopping, packet_is_null, set_scheduler_error, wait_until_not_paused,
};
use crate::error::DecodingOperationError::DecodeSubtitleError;
use crate::error::Error::{Bug, Decoding, OpenDecoder};
use crate::error::{
    DecodingError, DecodingOperationError, Error, OpenDecoderError, OpenDecoderOperationError,
};
use crate::hwaccel::HWAccelID::{HwaccelAuto, HwaccelGeneric};
use crate::hwaccel::{
    hw_device_get_by_name, hw_device_get_by_type, hw_device_init_from_type,
    hw_device_match_by_codec, HWAccelID,
};
use crate::util::ffmpeg_utils::av_rescale_q_rnd;
use crossbeam_channel::{RecvTimeoutError, Sender};
use ffmpeg_next::packet::{Mut, Ref};
use ffmpeg_next::{Frame, Packet};
use ffmpeg_sys_next::AVHWDeviceType::AV_HWDEVICE_TYPE_QSV;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_SUBTITLE, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::AVRounding::AV_ROUND_UP;
use ffmpeg_sys_next::AVSubtitleType::SUBTITLE_BITMAP;
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::{av_channel_layout_copy, AV_CODEC_FLAG_COPY_OPAQUE};
use ffmpeg_sys_next::{av_buffer_create, av_buffer_ref, av_calloc, av_dict_set, av_frame_apply_cropping, av_frame_copy_props, av_frame_move_ref, av_frame_ref, av_frame_unref, av_free, av_freep, av_gcd, av_hwdevice_get_type_name, av_hwframe_transfer_data, av_inv_q, av_mallocz, av_memdup, av_mul_q, av_opt_set_dict2, av_pix_fmt_desc_get, av_rescale_delta, av_rescale_q, av_strdup, avcodec_alloc_context3, avcodec_decode_subtitle2, avcodec_default_get_buffer2, avcodec_flush_buffers, avcodec_free_context, avcodec_get_hw_config, avcodec_open2, avcodec_parameters_to_context, avcodec_receive_frame, avcodec_send_packet, avsubtitle_free, AVCodec, AVCodecContext, AVDictionary, AVFrame, AVHWDeviceType, AVMediaType, AVPixelFormat, AVRational, AVSubtitle, AVSubtitleRect, AVERROR, AVERROR_EOF, AVPALETTE_SIZE, AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX, AV_FRAME_CROP_UNALIGNED, AV_FRAME_FLAG_CORRUPT, AV_NOPTS_VALUE, AV_PIX_FMT_FLAG_HWACCEL, AV_TIME_BASE_Q, EAGAIN, EINVAL, ENOMEM};
use log::{debug, error, info, trace, warn};
use std::ffi::{c_void, CStr, CString};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use crate::util::ffmpeg_utils::av_err2str;

#[cfg(feature = "docs-rs")]
pub(crate) fn dec_init(
    demux_idx: usize,
    dec_stream: &mut DecoderStream,
    exit_on_error: Option<bool>,
    frame_pool: ObjPool<Frame>,
    packet_pool: ObjPool<Packet>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
pub(crate) fn dec_init(
    demux_idx: usize,
    dec_stream: &mut DecoderStream,
    exit_on_error: Option<bool>,
    frame_pool: ObjPool<Frame>,
    packet_pool: ObjPool<Packet>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    let receiver = dec_stream.take_src();
    
    let codec_ptr = dec_stream.codec.as_ptr();
    if codec_ptr.is_null() {
        error!("Decoder codec pointer is null for stream {}", dec_stream.stream_index);
        return Err(OpenDecoder(OpenDecoderOperationError::ContextAllocationError(OpenDecoderError::OutOfMemory)));
    }
    
    let codec_name_ptr = unsafe { (*codec_ptr).name };
    if codec_name_ptr.is_null() {
        error!("Decoder codec name pointer is null for stream {}", dec_stream.stream_index);
        return Err(OpenDecoder(OpenDecoderOperationError::ContextAllocationError(OpenDecoderError::OutOfMemory)));
    }
    
    let decoder_name = unsafe {
        std::str::from_utf8_unchecked(CStr::from_ptr(codec_name_ptr).to_bytes())
    };

    if receiver.is_none() {
        debug!(
            "Demuxer:{demux_idx}:{decoder_name} stream:[{}] not be used. skip.",
            dec_stream.stream_index
        );
        return Ok(());
    }
    let receiver = receiver.unwrap();

    let dp = DecoderParameter::new(dec_stream);
    let dp_arc = Arc::new(Mutex::new(dp));
    dec_open(dp_arc.clone(), dec_stream, null_mut())?;

    let senders = dec_stream.take_dsts();
    let exit_on_error = exit_on_error.unwrap_or(false);


    let dp_arc = dp_arc.clone();
    let result = std::thread::Builder::new()
        .name(format!(
            "decoder{}:{demux_idx}:{decoder_name}",
            dec_stream.stream_index,
        ))
        .spawn(move || {
            let dp_arc = dp_arc;
            let input_status = false;
            let ret = 0;
            let mut err_exit = false;

            loop {
                if input_status {
                    break;
                }

                let result = receiver.recv_timeout(Duration::from_millis(100));

                if is_stopping(wait_until_not_paused(&scheduler_status)) {
                    info!("Decoder receiver end command, finishing.");
                    break;
                }

                if let Err(e) = result {
                    if e == RecvTimeoutError::Disconnected {
                        debug!("Demuxer thread exit.");
                        // input_status = true;
                        break;
                    }
                    continue;
                }

                let packet_box = result.unwrap();

                unsafe {
                    let have_data = !input_status
                        && !packet_is_null(&packet_box.packet)
                        && (!(*packet_box.packet.as_ptr()).buf.is_null()
                            || (*packet_box.packet.as_ptr()).side_data_elems != 0
                            || (*packet_box.packet.as_ptr()).opaque as usize as i32
                                == PacketOpaque::PktOpaqueSubHeartbeat as i32
                            || (*packet_box.packet.as_ptr()).opaque as usize as i32
                                == PacketOpaque::PktOpaqueFixSubDuration as i32);

                    let mut flush_buffers = !input_status && !have_data;
                    if !have_data {
                        match flush_buffers {
                            true => trace!("Decoder thread received flush packet"),
                            false => trace!("Decoder thread received EOF packet"),
                        }
                    }

                    if let Err(e) = packet_decode(
                        &dp_arc,
                        exit_on_error,
                        packet_box,
                        &packet_pool,
                        &frame_pool,
                        &senders,
                    ) {
                        if e == Error::Exit {
                            flush_buffers = false;
                        }

                        if e == Error::Exit || e == Error::EOF {
                            trace!(
                                "Decoder returned EOF, {}",
                                if flush_buffers {
                                    "resetting"
                                } else {
                                    "finishing"
                                }
                            );
                            if !flush_buffers {
                                break;
                            }

                            /* report last frame duration to the scheduler */
                            let dp = dp_arc.clone();
                            let dp = dp.lock().unwrap();

                            avcodec_flush_buffers(dp.dec_ctx.as_mut_ptr());
                        } else {
                            err_exit = true;
                            set_scheduler_error(&scheduler_status, &scheduler_result, e);
                            error!("Error processing packet in decoder: {}", av_err2str(ret));
                            break;
                        }
                    }
                }
            }

            // on success send EOF timestamp to our downstreams
            if !err_exit {
                let mut frame = frame_pool.get().unwrap();
                unsafe {
                    {
                        let dp = dp_arc.clone();
                        let dp = dp.lock().unwrap();
                        (*frame.as_mut_ptr()).opaque =
                            FrameOpaque::FrameOpaqueEof as i32 as *mut c_void;
                        (*frame.as_mut_ptr()).pts = if dp.last_frame_pts == AV_NOPTS_VALUE {
                            AV_NOPTS_VALUE
                        } else {
                            dp.last_frame_pts + dp.last_frame_duration_est
                        };
                        (*frame.as_mut_ptr()).time_base = dp.last_frame_tb;
                    }
                    let frame_box = dec_frame_to_box(dp_arc.clone(), frame);
                    if let Err(e) = dec_send(frame_box, &frame_pool, &senders) {
                        if e != Error::EOF {
                            error!("Error signalling EOF: {e}");
                            set_scheduler_error(&scheduler_status, &scheduler_result, e);
                            return;
                        }
                    }

                    let dp = dp_arc.clone();
                    let dp = dp.lock().unwrap();
                    let err_rate = if dp.dec.frames_decoded != 0 || dp.dec.decode_errors != 0 {
                        dp.dec.decode_errors as f64
                            / (dp.dec.frames_decoded + dp.dec.decode_errors) as f64
                    } else {
                        0.0
                    };
                    let max_error_rate = 2.0 / 3.0;
                    if err_rate > max_error_rate {
                        error!("Decoder error rate {err_rate} exceeds maximum {max_error_rate}");
                        // ret = FFMPEG_ERROR_RATE_EXCEEDED;
                    } else if err_rate != 0.0 {
                        debug!("Decoder error rate {err_rate}");
                    }
                }
            }

            dec_done(&dp_arc, &senders);

            dp_arc.lock().unwrap().drop_opaque_ptr();
            debug!("Decoder finished.");
        });
    if let Err(e) = result {
        error!("Decoder thread exited with error: {e}");
        return Err(OpenDecoderOperationError::ThreadExited.into());
    }

    Ok(())
}

#[cfg(feature = "docs-rs")]
unsafe fn transcode_subtitles(
    dp_arc: Arc<Mutex<DecoderParameter>>,
    exit_on_error: bool,
    packet_box: PacketBox,
    packet_pool: &ObjPool<Packet>,
    frame_pool: &ObjPool<Frame>,
    senders: &Vec<Sender<FrameBox>>,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn transcode_subtitles(
    dp_arc: Arc<Mutex<DecoderParameter>>,
    exit_on_error: bool,
    mut packet_box: PacketBox,
    packet_pool: &ObjPool<Packet>,
    frame_pool: &ObjPool<Frame>,
    senders: &Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>,
) -> crate::error::Result<()> {
    if !packet_is_null(&packet_box.packet)
        && (*packet_box.packet.as_ptr()).stream_index >= 0
        && (*packet_box.packet.as_ptr()).opaque as usize as i32
            == PacketOpaque::PktOpaqueSubHeartbeat as i32
    {
        let Ok(mut frame) = frame_pool.get() else {
            return Err(Decoding(DecodingOperationError::FrameAllocationError(
                DecodingError::OutOfMemory,
            )));
        };
        (*frame.as_mut_ptr()).pts = (*packet_box.packet.as_ptr()).pts;
        (*frame.as_mut_ptr()).time_base = (*packet_box.packet.as_ptr()).time_base;
        (*frame.as_mut_ptr()).opaque = PacketOpaque::PktOpaqueSubHeartbeat as i32 as *mut c_void;

        let frame_box = dec_frame_to_box(dp_arc, frame);

        let result = dec_send(frame_box, frame_pool, senders);
        packet_pool.release(packet_box.packet);
        return match result {
            Ok(_) => Ok(()),
            Err(e) => {
                if e == Error::EOF {
                    Err(Error::Exit)
                } else {
                    Err(e)
                }
            }
        };
    } else if !packet_is_null(&packet_box.packet)
        && (*packet_box.packet.as_ptr()).stream_index >= 0
        && (*packet_box.packet.as_ptr()).opaque as usize as i32
            == PacketOpaque::PktOpaqueFixSubDuration as i32
    {
        //TODO
        let _ret = fix_sub_duration_heartbeat(
            dp_arc,
            av_rescale_q(
                (*packet_box.packet.as_ptr()).pts,
                (*packet_box.packet.as_ptr()).time_base,
                AV_TIME_BASE_Q,
            ),
        );
        packet_pool.release(packet_box.packet);
        // return ret;
        return Ok(());
    }

    let mut packet_is_eof = false;
    if packet_is_null(&packet_box.packet) || (*packet_box.packet.as_ptr()).stream_index >= 0 {
        let Ok(packet) = packet_pool.get() else {
            return Err(Decoding(DecodingOperationError::PacketAllocationError(
                DecodingError::OutOfMemory,
            )));
        };
        packet_box.packet = packet;
        packet_is_eof = true;
    };

    let dp = dp_arc.clone();
    let mut dp = dp.lock().unwrap();
    let mut subtitle = AVSubtitle {
        format: 0,
        start_display_time: 0,
        end_display_time: 0,
        num_rects: 0,
        rects: null_mut(),
        pts: 0,
    };
    let mut got_output: libc::c_int = 0;
    let ret = avcodec_decode_subtitle2(
        dp.dec_ctx.as_mut_ptr(),
        &mut subtitle,
        &mut got_output,
        packet_box.packet.as_mut_ptr(),
    );
    packet_pool.release(packet_box.packet);
    if ret < 0 {
        error!("Error decoding subtitles: {}", av_err2str(ret));
        dp.dec.decode_errors = dp.dec.decode_errors + 1;
        return if exit_on_error {
            Err(Decoding(DecodeSubtitleError(DecodingError::from(ret))))
        } else {
            Ok(())
        };
    }

    if got_output == 0 {
        return if !packet_is_eof {
            Ok(())
        } else {
            Err(Error::EOF)
        };
    }

    dp.dec.frames_decoded = dp.dec.frames_decoded + 1;

    let Ok(mut frame) = frame_pool.get() else {
        return Err(Decoding(DecodingOperationError::FrameAllocationError(
            DecodingError::OutOfMemory,
        )));
    };
    // XXX the queue for transferring data to consumers runs
    // on AVFrames, so we wrap AVSubtitle in an AVBufferRef and put that
    // inside the frame
    // eventually, subtitles should be switched to use AVFrames natively
    if let Err(e) = subtitle_wrap_frame(frame.as_mut_ptr(), &mut subtitle, false) {
        avsubtitle_free(&mut subtitle);
        return Err(e);
    }

    (*frame.as_mut_ptr()).width = (*dp.dec_ctx.as_ptr()).width;
    (*frame.as_mut_ptr()).height = (*dp.dec_ctx.as_ptr()).height;
    std::mem::drop(dp);

    process_subtitle(dp_arc, frame, frame_pool, senders)
}

unsafe fn process_subtitle(
    dp_arc: Arc<Mutex<DecoderParameter>>,
    frame: Frame,
    frame_pool: &ObjPool<Frame>,
    senders: &Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>,
) -> crate::error::Result<()> {
    let frame_ptr = frame.as_ptr();
    if (*frame_ptr).buf[0].is_null() {
        return Ok(());
    }

    let subtitle = (*(*frame_ptr).buf[0]).data as *mut AVSubtitle;

    //TODO
    // if (dp->flags & DECODER_FLAG_FIX_SUB_DURATION)

    if subtitle.is_null() {
        return Ok(());
    }

    let frame_box = dec_frame_to_box(dp_arc, frame);

    match dec_send(frame_box, frame_pool, senders) {
        Ok(_) => Ok(()),
        Err(e) => {
            if e == Error::EOF {
                Err(Error::Exit)
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(not(feature = "docs-rs"))]
fn subtitle_wrap_frame(
    frame: *mut AVFrame,
    subtitle: *mut AVSubtitle,
    copy: bool,
) -> crate::error::Result<()> {
    unsafe {
        let mut sub: *mut AVSubtitle;

        if copy {
            sub = av_mallocz(size_of::<AVSubtitle>()) as *mut AVSubtitle;
            if sub.is_null() {
                return Err(Decoding(DecodingOperationError::SubtitleAllocationError(
                    DecodingError::OutOfMemory,
                )));
            }

            let ret = copy_av_subtitle(sub, subtitle);
            if ret < 0 {
                av_freep(&mut sub as *mut _ as *mut c_void);
                return Err(Decoding(DecodingOperationError::CopySubtitleError(
                    DecodingError::from(ret),
                )));
            }
        } else {
            sub = av_memdup(subtitle as *const c_void, std::mem::size_of::<AVSubtitle>())
                as *mut AVSubtitle;
            if sub.is_null() {
                return Err(Decoding(DecodingOperationError::SubtitleAllocationError(
                    DecodingError::OutOfMemory,
                )));
            }
            std::ptr::write(subtitle, std::mem::zeroed()); // Clear the source subtitle
        }

        let buf = av_buffer_create(
            sub as *mut u8,
            std::mem::size_of::<AVSubtitle>(),
            Some(subtitle_free),
            null_mut(),
            0,
        );

        if buf.is_null() {
            avsubtitle_free(sub);
            av_freep(&mut sub as *mut _ as *mut c_void);
            return Err(Decoding(DecodingOperationError::SubtitleAllocationError(
                DecodingError::OutOfMemory,
            )));
        }

        (*frame).buf[0] = buf;

        Ok(())
    }
}

unsafe extern "C" fn subtitle_free(_: *mut c_void, data: *mut u8) {
    let sub = data as *mut AVSubtitle;
    avsubtitle_free(sub);
    av_free(sub as *mut c_void);
}

unsafe fn copy_av_subtitle(dst: *mut AVSubtitle, src: *const AVSubtitle) -> i32 {
    let mut tmp = AVSubtitle {
        format: (*src).format,
        start_display_time: (*src).start_display_time,
        end_display_time: (*src).end_display_time,
        num_rects: 0,
        rects: null_mut(),
        pts: (*src).pts,
    };

    if (*src).num_rects == 0 {
        *dst = tmp;
        return 0; // Success
    }

    tmp.rects = av_calloc(
        (*src).num_rects as usize,
        std::mem::size_of::<*mut AVSubtitleRect>(),
    ) as *mut *mut AVSubtitleRect;
    if tmp.rects.is_null() {
        return AVERROR(ENOMEM);
    }

    for i in 0..(*src).num_rects as usize {
        let src_rect = *(*src).rects.add(i);
        let dst_rect: *mut AVSubtitleRect =
            av_mallocz(std::mem::size_of::<AVSubtitleRect>()) as *mut AVSubtitleRect;
        if dst_rect.is_null() {
            avsubtitle_free(&mut tmp);
            return AVERROR(ENOMEM);
        }

        tmp.rects.add(i).write(dst_rect);
        (*dst_rect).type_ = (*src_rect).type_;
        (*dst_rect).flags = (*src_rect).flags;
        (*dst_rect).x = (*src_rect).x;
        (*dst_rect).y = (*src_rect).y;
        (*dst_rect).w = (*src_rect).w;
        (*dst_rect).h = (*src_rect).h;
        (*dst_rect).nb_colors = (*src_rect).nb_colors;

        // Deep copy text
        if !(*src_rect).text.is_null() {
            (*dst_rect).text = av_strdup((*src_rect).text);
            if (*dst_rect).text.is_null() {
                avsubtitle_free(&mut tmp);
                return AVERROR(ENOMEM);
            }
        }

        // Deep copy ASS
        if !(*src_rect).ass.is_null() {
            (*dst_rect).ass = av_strdup((*src_rect).ass);
            if (*dst_rect).ass.is_null() {
                avsubtitle_free(&mut tmp);
                return AVERROR(ENOMEM);
            }
        }

        // Deep copy data
        for j in 0..4 {
            let buf_size = if (*src_rect).type_ == SUBTITLE_BITMAP && j == 1 {
                AVPALETTE_SIZE
            } else {
                ((*src_rect).h * (*src_rect).linesize[j as usize]) as i32
            };

            if !(*src_rect).data[j as usize].is_null() {
                (*dst_rect).data[j as usize] = av_memdup(
                    (*src_rect).data[j as usize] as *const c_void,
                    buf_size as usize,
                ) as *mut u8;
                if (*dst_rect).data[j as usize].is_null() {
                    avsubtitle_free(&mut tmp);
                    return AVERROR(ENOMEM);
                }
                (*dst_rect).linesize[j as usize] = (*src_rect).linesize[j as usize];
            }
        }

        tmp.num_rects += 1;
    }

    *dst = tmp;
    0 // Success
}

fn fix_sub_duration_heartbeat(_dp_arc: Arc<Mutex<DecoderParameter>>, _signal_pts: i64) -> i32 {
    0
}

unsafe fn dec_send(
    mut frame_box: FrameBox,
    frame_pool: &ObjPool<Frame>,
    senders: &Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>,
) -> crate::error::Result<()> {
    let mut nb_done = 0;
    for (i, (sender, fg_input_index, finished_flag_list)) in senders.iter().enumerate() {
        if !finished_flag_list.is_empty() && *fg_input_index < finished_flag_list.len() {
            if finished_flag_list[*fg_input_index].load(Ordering::Acquire) {
                nb_done += 1;
                continue;
            }
        }
        if i < senders.len() - 1 {
            let Ok(mut to_send) = frame_pool.get() else {
                return Err(Decoding(DecodingOperationError::FrameAllocationError(
                    DecodingError::OutOfMemory,
                )));
            };

            let mut frame_data = frame_box.frame_data.clone();
            frame_data.fg_input_index = *fg_input_index;

            // frame may sometimes contain props only,
            // e.g. to signal EOF timestamp
            if !(*frame_box.frame.as_ptr()).buf[0].is_null() {
                let ret = av_frame_ref(to_send.as_mut_ptr(), frame_box.frame.as_ptr());
                if ret < 0 {
                    return Err(Decoding(DecodingOperationError::FrameRefError(
                        DecodingError::OutOfMemory,
                    )));
                }
            } else {
                let ret = av_frame_copy_props(to_send.as_mut_ptr(), frame_box.frame.as_ptr());
                if ret < 0 {
                    return Err(Decoding(DecodingOperationError::FrameCopyPropsError(
                        DecodingError::OutOfMemory,
                    )));
                }
            };

            let frame_box = FrameBox {
                frame: to_send,
                frame_data,
            };
            if let Err(_) = sender.send(frame_box) {
                debug!("Decoder send frame failed, destination already finished");
                nb_done += 1;
                continue;
            }
        } else {
            frame_box.frame_data.fg_input_index = *fg_input_index;

            if let Err(_) = sender.send(frame_box) {
                debug!("Decoder send frame failed, destination already finished");
                nb_done += 1;
            }
            break;
        }
    }

    if nb_done == senders.len() {
        Err(Error::EOF)
    } else {
        Ok(())
    }
}

unsafe fn dec_frame_to_box(dp_arc: Arc<Mutex<DecoderParameter>>, frame: Frame) -> FrameBox {
    let dp = dp_arc.lock().unwrap();
    let dec_ctx = dp.dec_ctx.as_ptr();
    let frame_box = FrameBox {
        frame,
        frame_data: FrameData {
            framerate: Some((*dec_ctx).framerate),
            bits_per_raw_sample: (*dec_ctx).bits_per_raw_sample,
            input_stream_width: (*dec_ctx).width,
            input_stream_height: (*dec_ctx).height,
            subtitle_header_size: (*dec_ctx).subtitle_header_size,
            subtitle_header: (*dec_ctx).subtitle_header,
            fg_input_index: usize::MAX,
        },
    };
    frame_box
}

#[repr(i32)]
enum PacketOpaque {
    PktOpaqueSubHeartbeat = 1,
    PktOpaqueFixSubDuration,
}

#[cfg(feature = "docs-rs")]
fn dec_open(
    dp_arc: Arc<Mutex<DecoderParameter>>,
    dec_stream: &DecoderStream,
    param_out: *mut AVFrame,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
fn dec_open(
    dp_arc: Arc<Mutex<DecoderParameter>>,
    dec_stream: &DecoderStream,
    param_out: *mut AVFrame,
) -> crate::error::Result<()> {
    unsafe {
        let mut dec_ctx = avcodec_alloc_context3(dec_stream.codec.as_ptr());
        if dec_ctx.is_null() {
            return Err(OpenDecoder(
                OpenDecoderOperationError::ContextAllocationError(OpenDecoderError::OutOfMemory),
            ));
        }

        let mut ret = avcodec_parameters_to_context(
            dec_ctx,
            (*dec_stream.stream.inner).codecpar,
        );
        if ret < 0 {
            avcodec_free_context(&mut dec_ctx);
            error!("Error initializing the decoder context.");
            return Err(OpenDecoder(
                OpenDecoderOperationError::ParameterApplicationError(OpenDecoderError::from(ret)),
            ));
        }

        let dp_ptr = Arc::into_raw(dp_arc.clone());
        // Set `opaque` to point to the boxed `DecoderParameter`.
        (*dec_ctx).opaque = dp_ptr as *mut libc::c_void;

        (*dec_ctx).get_format = Some(get_format_callback);
        (*dec_ctx).get_buffer2 = Some(get_buffer_callback);
        (*dec_ctx).pkt_timebase = dec_stream.time_base;

        let mut dec_opts: *mut AVDictionary = std::ptr::null_mut();
        let opt_key = CString::new("threads".to_string()).unwrap();
        let opt_val = CString::new("auto".to_string()).unwrap();
        av_dict_set(&mut dec_opts, opt_key.as_ptr(), opt_val.as_ptr(), 0);

        {
            let dp_arc_clone = dp_arc.clone();
            let mut dp = dp_arc_clone.lock().unwrap();
            ret = hw_device_setup_for_decode(&mut dp, dec_stream.codec.as_ptr(), dec_ctx);
            if ret < 0 {
                avcodec_free_context(&mut dec_ctx);
                error!("Hardware device setup failed for decoder: {}", av_err2str(ret));
                return Err(OpenDecoder(OpenDecoderOperationError::HwSetupError(
                    OpenDecoderError::from(ret),
                )));
            }
        }

        ret = av_opt_set_dict2(dec_ctx as *mut c_void, &mut dec_opts, ffmpeg_sys_next::AV_OPT_SEARCH_CHILDREN);
        if ret < 0 {
            avcodec_free_context(&mut dec_ctx);
            error!("Error applying decoder options: {}", av_err2str(ret));
            return Err(OpenDecoder(
                OpenDecoderOperationError::ParameterApplicationError(OpenDecoderError::from(ret)),
            ));
        }

        (*dec_ctx).flags |= AV_CODEC_FLAG_COPY_OPAQUE as i32;
        // we apply cropping outselves
        {
            let dp_arc_clone = dp_arc.clone();
            let mut dp = dp_arc_clone.lock().unwrap();
            dp.apply_cropping = (*dec_ctx).apply_cropping;
        }
        (*dec_ctx).apply_cropping = 0;

        ret = avcodec_open2(dec_ctx, dec_stream.codec.as_ptr(), null_mut());
        if ret < 0 {
            avcodec_free_context(&mut dec_ctx);
            error!("Error while opening decoder: {}", av_err2str(ret));
            return Err(OpenDecoder(OpenDecoderOperationError::DecoderOpenError(
                OpenDecoderError::from(ret),
            )));
        }

        if !(*dec_ctx).hw_device_ctx.is_null() {
            // Update decoder extra_hw_frames option to account for the
            // frames held in queues inside the ffmpeg utility.  This is
            // called after avcodec_open2() because the user-set value of
            // extra_hw_frames becomes valid in there, and we need to add
            // this on top of it.

            // DEFAULT_FRAME_THREAD_QUEUE_SIZE = 8
            let extra_frames = 8;
            if (*dec_ctx).extra_hw_frames >= 0 {
                (*dec_ctx).extra_hw_frames += extra_frames;
            } else {
                (*dec_ctx).extra_hw_frames = extra_frames;
            }
        }

        {
            let dp_arc_clone = dp_arc.clone();
            let mut dp = dp_arc_clone.lock().unwrap();
            dp.dec.subtitle_header = (*dec_ctx).subtitle_header;
            dp.dec.subtitle_header_size = (*dec_ctx).subtitle_header_size;
        }

        if !param_out.is_null() {
            if (*dec_ctx).codec_type == AVMEDIA_TYPE_AUDIO {
                (*param_out).format = (*dec_ctx).sample_fmt as i32;
                (*param_out).sample_rate = (*dec_ctx).sample_rate;

                ret =
                    av_channel_layout_copy(&mut (*param_out).ch_layout, &mut (*dec_ctx).ch_layout);
                if ret < 0 {
                    return Err(OpenDecoder(
                        OpenDecoderOperationError::ChannelLayoutCopyError(OpenDecoderError::from(
                            ret,
                        )),
                    ));
                }
            } else if (*dec_ctx).codec_type == AVMEDIA_TYPE_VIDEO {
                (*param_out).format = (*dec_ctx).pix_fmt as i32;
                (*param_out).width = (*dec_ctx).width;
                (*param_out).height = (*dec_ctx).height;
                (*param_out).sample_aspect_ratio = (*dec_ctx).sample_aspect_ratio;
                (*param_out).colorspace = (*dec_ctx).colorspace;
                (*param_out).color_range = (*dec_ctx).color_range;
            }

            (*param_out).time_base = (*dec_ctx).pkt_timebase;
        }

        let mut dp = dp_arc.lock().unwrap();
        dp.dec_ctx.replace(dec_ctx);
    }

    Ok(())
}

fn hw_device_setup_for_decode(
    dp: &mut MutexGuard<DecoderParameter>,
    codec: *const AVCodec,
    dec_ctx: *mut AVCodecContext,
) -> i32 {
    let mut dev = None;
    let mut err = 0;
    let mut auto_device = false;
    let mut device_type = AVHWDeviceType::AV_HWDEVICE_TYPE_NONE;

    let hwaccel_device = dp.hwaccel_device.clone();
    if let Some(hwaccel_device) = &hwaccel_device {
        dev = hw_device_get_by_name(hwaccel_device);
        match &dev {
            None => {
                if dp.hwaccel_id == HWAccelID::HwaccelAuto {
                    auto_device = true;
                } else if dp.hwaccel_id == HWAccelID::HwaccelGeneric {
                    device_type = dp.hwaccel_device_type;
                    (err, dev) =
                        hw_device_init_from_type(device_type, Some(hwaccel_device.clone()));
                } else {
                    // This will be dealt with by API-specific initialisation
                    // (using hwaccel_device), so nothing further needed here.
                    return 0;
                }
            }
            Some(dev) => {
                if dp.hwaccel_id == HWAccelID::HwaccelAuto {
                    dp.hwaccel_device_type = dev.device_type;
                } else if dp.hwaccel_device_type != dev.device_type {
                    unsafe {
                        let dev_device_name = av_hwdevice_get_type_name(dev.device_type);
                        let dev_device_name = CStr::from_ptr(dev_device_name).to_str();
                        let dp_device_name = av_hwdevice_get_type_name(dp.hwaccel_device_type);
                        let dp_device_name = CStr::from_ptr(dp_device_name).to_str();
                        if let (Ok(dev_device_name), Ok(dp_device_name)) =
                            (dev_device_name, dp_device_name)
                        {
                            error!("Invalid hwaccel device specified for decoder: device {} of type {} is not usable with hwaccel {}.",
                        dev.name, dp_device_name, dev_device_name);
                        }
                    }

                    return AVERROR(EINVAL);
                }
            }
        }
    } else {
        if dp.hwaccel_id == HWAccelID::HwaccelAuto {
            auto_device = true;
        } else if dp.hwaccel_id == HWAccelID::HwaccelGeneric {
            device_type = dp.hwaccel_device_type;
            dev = hw_device_get_by_type(device_type);

            // When "-qsv_device device" is used, an internal QSV device named
            // as "__qsv_device" is created. Another QSV device is created too
            // if "-init_hw_device qsv=name:device" is used. There are 2 QSV devices
            // if both "-qsv_device device" and "-init_hw_device qsv=name:device"
            // are used, hw_device_get_by_type(AV_HWDEVICE_TYPE_QSV) returns NULL.
            // To keep back-compatibility with the removed ad-hoc libmfx setup code,
            // call hw_device_get_by_name("__qsv_device") to select the internal QSV
            // device.
            if dev.is_none() && device_type == AV_HWDEVICE_TYPE_QSV {
                dev = hw_device_get_by_name("__qsv_device");
            }

            if dev.is_none() {
                (err, dev) = hw_device_init_from_type(device_type, None);
            };
        } else {
            dev = hw_device_match_by_codec(codec);
            if dev.is_none() {
                // No device for this codec, but not using generic hwaccel
                // and therefore may well not need one - ignore.
                return 0;
            }
        }
    }

    if auto_device {
        if unsafe { avcodec_get_hw_config(codec, 0).is_null() } {
            // Decoder does not support any hardware devices.
            return 0;
        }

        let mut i = 0;
        loop {
            let config = unsafe { avcodec_get_hw_config(codec, i) };
            if config.is_null() {
                break;
            }

            device_type = unsafe { (*config).device_type };
            dev = hw_device_get_by_type(device_type);
            if let Some(dev) = &dev {
                unsafe {
                    let dev_device_type = av_hwdevice_get_type_name(device_type);
                    if let Ok(dev_device_type) = CStr::from_ptr(dev_device_type).to_str() {
                        info!(
                            "Using auto hwaccel type {dev_device_type} with existing device {}.",
                            dev.name
                        );
                    }
                }
                break;
            }

            i += 1;
        }

        i = 0;
        loop {
            let config = unsafe { avcodec_get_hw_config(codec, i) };
            if config.is_null() {
                break;
            }

            device_type = unsafe { (*config).device_type };
            // Try to make a new device of this type.
            (err, dev) = hw_device_init_from_type(device_type, dp.hwaccel_device.clone());
            if err < 0 {
                // Can't make a device of this type.
                i += 1;
                continue;
            }

            unsafe {
                let dev_device_type = av_hwdevice_get_type_name(device_type);
                if let Ok(dev_device_type) = CStr::from_ptr(dev_device_type).to_str() {
                    match &dp.hwaccel_device {
                        Some(hwaccel_device) => {
                            info!("Using auto hwaccel type {dev_device_type} with new device created from {hwaccel_device}.");
                        }
                        None => {
                            info!("Using auto hwaccel type {dev_device_type} with new default device.");
                        }
                    }
                }
            }
            break;
        }
        if dev.is_some() {
            dp.hwaccel_device_type = device_type;
        } else {
            info!("Auto hwaccel disabled: no device found.");
            dp.hwaccel_id = HWAccelID::HwaccelNone;
            return 0;
        }
    }

    match dev {
        None => {
            unsafe {
                let dev_device_type = av_hwdevice_get_type_name(device_type);
                let dev_device_type = CStr::from_ptr(dev_device_type).to_str();
                let codec_name = (*codec).name;
                let codec_name = CStr::from_ptr(codec_name).to_str();
                if let (Ok(dev_device_type), Ok(codec_name)) = (dev_device_type, codec_name) {
                    info!("No device available for decoder: device type {dev_device_type} needed for codec {codec_name}.");
                }
            }
            err
        }
        Some(dev) => unsafe {
            (*dec_ctx).hw_device_ctx = av_buffer_ref(dev.device_ref);
            if (*dec_ctx).hw_device_ctx.is_null() {
                return AVERROR(ENOMEM);
            }
            0
        },
    }
}

unsafe extern "C" fn get_format_callback(
    s: *mut AVCodecContext,
    pix_fmts: *const AVPixelFormat,
) -> AVPixelFormat {
    if s.is_null() || pix_fmts.is_null() {
        trace!("get pixel format: none");
        return AVPixelFormat::AV_PIX_FMT_NONE;
    }

    // Retrieve `DecoderParameter` from `opaque`
    let dp_ptr = (*s).opaque as *const Mutex<DecoderParameter>;
    let dp_arc = Arc::from_raw(dp_ptr);
    // Create a new Arc to extend the lifetime
    let dp_ptr = Arc::into_raw(dp_arc.clone());
    (*s).opaque = dp_ptr as *mut libc::c_void;

    let mut dp = dp_arc.lock().unwrap();

    let mut i = 0;
    while *pix_fmts.add(i) != AVPixelFormat::AV_PIX_FMT_NONE {
        let mut config = null();
        let desc = av_pix_fmt_desc_get(*pix_fmts.add(i));

        if desc.is_null() || (*desc).flags & AV_PIX_FMT_FLAG_HWACCEL as u64 == 0 {
            break;
        }
        if dp.hwaccel_id == HwaccelGeneric || dp.hwaccel_id == HwaccelAuto {
            let mut j = 0;
            loop {
                config = avcodec_get_hw_config((*s).codec, j);
                if config.is_null() {
                    break;
                }
                if (*config).methods as u32 & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as u32 == 0 {
                    j += 1;
                    continue;
                }
                if (*config).pix_fmt == *pix_fmts.add(i) {
                    break;
                }
                j += 1;
            }
        }

        if !config.is_null() && (*config).device_type == dp.hwaccel_device_type {
            dp.hwaccel_pix_fmt = *pix_fmts.add(i);
            break;
        }

        i += 1;
    }

    let format = *pix_fmts.add(i);
    trace!("get pixel format: {:?}", format);

    format
}

unsafe extern "C" fn get_buffer_callback(
    dec_ctx: *mut AVCodecContext,
    frame: *mut AVFrame,
    flags: libc::c_int,
) -> libc::c_int {
    if dec_ctx.is_null() || frame.is_null() {
        return AVERROR(EINVAL);
    }

    /*let dp_ptr = (*dec_ctx).opaque as *const Arc<Mutex<DecoderParameter>>;
    let dp_arc = Arc::clone(&*dp_ptr);

    let mut dp = dp_arc.lock().unwrap();*/

    // for multiview video, store the output mask in frame opaque
    /*if dp.view_map.len() > 0 {
        let sd = av_frame_get_side_data(frame, AV_FRAME_DATA_VIEW_ID);
        let view_id = if !sd.is_null() {
            *(sd.as_ref().unwrap().data as *const i32)
        } else {
            0
        };

        for i in 0..dp.view_map.len() {
            if dp.view_map[i].id == view_id {
                (*frame).opaque = dp.view_map[i].out_mask as *mut c_void;
                break;
            }
        }
    }*/

    avcodec_default_get_buffer2(dec_ctx, frame, flags)
}

struct DecoderParameter {
    dec: Decoder,

    dec_ctx: CodecContext,
    // dec_ctx: *mut AVCodecContext,

    // override output video sample aspect ratio with this value
    sar_override: AVRational,
    framerate_in: AVRational,

    apply_cropping: i32,

    hwaccel_id: HWAccelID,
    hwaccel_device_type: AVHWDeviceType,
    hwaccel_device: Option<String>,
    hwaccel_output_format: AVPixelFormat,
    hwaccel_pix_fmt: AVPixelFormat,

    // pts/estimated duration of the last decoded frame
    // * in decoder timebase for video,
    // * in last_frame_tb (may change during decoding) for audio
    last_frame_pts: i64,
    last_frame_duration_est: i64,
    last_frame_tb: AVRational,
    last_filter_in_rescale_delta: i64,
    last_frame_sample_rate: i32,
    // view_map: Vec<ViewMap>,

}

unsafe impl Send for DecoderParameter {}
unsafe impl Sync for DecoderParameter {}

/*struct ViewMap {
    id: i32,
    out_mask: i32,
}*/

impl DecoderParameter {
    fn drop_opaque_ptr(&self) {
        let dec_ctx = self.dec_ctx.as_ptr();
        if !dec_ctx.is_null() {
            unsafe {
                let dp_ptr = (*dec_ctx).opaque as *const Mutex<DecoderParameter>;
                if !dp_ptr.is_null() {
                    let _dp_arc = Arc::from_raw(dp_ptr);
                }
            }
        }
    }

    fn new(dec_stream: &mut DecoderStream) -> Self {
        Self {
            dec: Decoder {
                media_type: dec_stream.codec_type,
                subtitle_header_size: 0,
                subtitle_header: null_mut(),
                frames_decoded: 0,
                samples_decoded: 0,
                decode_errors: 0,
            },
            dec_ctx: CodecContext::null(),

            sar_override: unsafe { (*(*dec_stream.stream.inner).codecpar).sample_aspect_ratio },
            framerate_in: dec_stream.avg_framerate,
            apply_cropping: 0,
            hwaccel_id: dec_stream.hwaccel_id,
            hwaccel_device_type: dec_stream.hwaccel_device_type,
            hwaccel_device: dec_stream.hwaccel_device.clone(),
            hwaccel_output_format: dec_stream.hwaccel_output_format,
            hwaccel_pix_fmt: AVPixelFormat::AV_PIX_FMT_NONE,
            last_frame_pts: 0,
            last_frame_duration_est: 0,
            last_frame_tb: AVRational { num: 1, den: 1 },
            last_filter_in_rescale_delta: 0,
            last_frame_sample_rate: 0,

            // view_map: vec![],
        }
    }
}

struct Decoder {
    #[allow(dead_code)]
    media_type: AVMediaType,

    subtitle_header_size: i32,
    subtitle_header: *mut u8,

    // number of frames/samples retrieved from the decoder
    frames_decoded: u64,
    samples_decoded: u64,
    decode_errors: u64,
}

fn dec_done(dp_arc: &Arc<Mutex<DecoderParameter>>, senders: &Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>) {
    for (sender, fg_input_index, finished_flag_list) in senders {
        if !finished_flag_list.is_empty() && *fg_input_index < finished_flag_list.len() {
            if finished_flag_list[*fg_input_index].load(Ordering::Acquire) {
                continue;
            }
        }

        let mut frame_box = unsafe { dec_frame_to_box(dp_arc.clone(), null_frame()) };
        frame_box.frame_data.fg_input_index = *fg_input_index;
        if let Err(_) = sender.send(frame_box) {
            debug!("Decoder send EOF failed, destination already finished");
        }
    }
}

/*const fn fferrtag(a: u8, b: u8, c: u8, d: u8) -> i32 {
    -(((a as i32) & 0xFF) << 24
        | ((b as i32) & 0xFF) << 16
        | ((c as i32) & 0xFF) << 8
        | ((d as i32) & 0xFF))
}

const FFMPEG_ERROR_RATE_EXCEEDED: i32 = fferrtag(b'E', b'R', b'E', b'D');*/

#[repr(i32)]
enum FrameOpaque {
    #[allow(dead_code)]
    FrameOpaqueSubHeartbeat = 1,
    FrameOpaqueEof,
    #[allow(dead_code)]
    FrameOpaqueSendCommand,
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn packet_decode(
    dp_arc: &Arc<Mutex<DecoderParameter>>,
    exit_on_error: bool,
    packet_box: PacketBox,
    packet_pool: &ObjPool<Packet>,
    frame_pool: &ObjPool<Frame>,
    senders: &Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>,
) -> crate::error::Result<()> {
    let dec_ctx = {
        let dp = dp_arc.clone();
        let dp = dp.lock().unwrap();
        dp.dec_ctx.as_mut_ptr()
    };

    if !dec_ctx.is_null() && (*dec_ctx).codec_type == AVMEDIA_TYPE_SUBTITLE {
        return transcode_subtitles(
            dp_arc.clone(),
            exit_on_error,
            packet_box,
            packet_pool,
            frame_pool,
            senders,
        );
    }

    // With fate-indeo3-2, we're getting 0-sized packets before EOF for some
    // reason. This seems like a semi-critical bug. Don't trigger EOF, and
    // skip the packet.
    if !packet_is_null(&packet_box.packet)
        && (*packet_box.packet.as_ptr()).stream_index >= 0
        && packet_box.packet.is_empty()
    {
        return Ok(());
    }

    //TODO DECODER_FLAG_TS_UNRELIABLE

    let mut ret = if (*packet_box.packet.as_ptr()).stream_index < 0 {
        avcodec_send_packet(dec_ctx, null())
    } else {
        avcodec_send_packet(dec_ctx, packet_box.packet.as_ptr())
    };
    if ret < 0 && !(ret == AVERROR_EOF && (*packet_box.packet.as_ptr()).stream_index < 0) {
        // In particular, we don't expect AVERROR(EAGAIN), because we read all
        // decoded frames with avcodec_receive_frame() until done.
        if ret == AVERROR(EAGAIN) {
            error!("A decoder returned an unexpected error code. This is a bug, please report it.");
            packet_pool.release(packet_box.packet);
            return Err(Bug);
        }
        error!(
            "Error submitting {} to decoder: {}",
            if (*packet_box.packet.as_ptr()).stream_index < 0 {
                "EOF"
            } else {
                "packet"
            },
            av_err2str(ret)
        );

        packet_pool.release(packet_box.packet);
        if ret != AVERROR_EOF {
            let dp = dp_arc.clone();
            let mut dp = dp.lock().unwrap();
            dp.dec.decode_errors += 1;
            if !exit_on_error {
                return Ok(());
            };
        }

        return Err(Decoding(DecodingOperationError::SendPacketError(
            DecodingError::from(ret),
        )));
    }

    packet_pool.release(packet_box.packet);

    loop {
        let mut outputs_mask = 1;

        let Ok(mut frame) = frame_pool.get() else {
            return Err(Decoding(DecodingOperationError::FrameAllocationError(
                DecodingError::OutOfMemory,
            )));
        };

        ret = avcodec_receive_frame(dec_ctx, frame.as_mut_ptr());
        if ret == AVERROR(EAGAIN) {
            return Ok(());
        } else if ret == AVERROR_EOF {
            return Err(Error::EOF);
        } else if ret < 0 {
            error!("Decoding error: {}", av_err2str(ret));
            let dp = dp_arc.clone();
            let mut dp = dp.lock().unwrap();
            dp.dec.decode_errors += 1;

            if exit_on_error {
                return Err(Decoding(DecodingOperationError::ReceiveFrameError(
                    DecodingError::from(ret),
                )));
            };

            continue;
        }

        if (*frame.as_ptr()).decode_error_flags != 0
            || ((*frame.as_ptr()).flags & AV_FRAME_FLAG_CORRUPT != 0)
        {
            if exit_on_error {
                error!("corrupt decoded frame");
                return Err(Decoding(DecodingOperationError::CorruptFrame));
            } else {
                warn!("corrupt decoded frame");
            }
        }

        let mut frame_box = dec_frame_to_box(dp_arc.clone(), frame);
        // fdemux_paramter.dec.pts                 = (*frame).pts;
        // fdemux_paramter.dec.tb                  = dec->pkt_timebase;
        // fdemux_paramter.dec.frame_num           = dec->frame_num - 1;

        (*frame_box.frame.as_mut_ptr()).time_base = (*dec_ctx).pkt_timebase;

        if (*dec_ctx).codec_type == AVMEDIA_TYPE_AUDIO {
            let dp = dp_arc.clone();
            let mut dp = dp.lock().unwrap();
            dp.dec.samples_decoded += (*frame_box.frame.as_ptr()).nb_samples as u64;

            audio_ts_process(dp, frame_box.frame.as_mut_ptr());
        } else {
            if let Err(e) = video_frame_process(
                dp_arc.clone(),
                frame_box.frame.as_mut_ptr(),
                &mut outputs_mask,
                frame_pool,
            ) {
                error!("Error while processing the decoded data");
                return Err(e);
            }
        }

        {
            let dp = dp_arc.clone();
            let mut dp = dp.lock().unwrap();
            dp.dec.frames_decoded += 1;
        }

        if let Err(e) = dec_send(frame_box, &frame_pool, &senders) {
            return if e == Error::EOF {
                Err(Error::Exit)
            } else {
                Err(e)
            };
        }
    }
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn video_frame_process(
    dp_arc: Arc<Mutex<DecoderParameter>>,
    frame: *mut AVFrame,
    outputs_mask: &mut usize,
    frame_pool: &ObjPool<Frame>,
) -> crate::error::Result<()> {
    let mut dp = dp_arc.lock().unwrap();

    if (*frame).format == dp.hwaccel_pix_fmt as i32 {
        let err = hwaccel_retrieve_data(&dp, frame, frame_pool);
        if err < 0 {
            return Err(Decoding(DecodingOperationError::HWRetrieveDataError(
                DecodingError::from(err),
            )));
        }
    }

    (*frame).pts = (*frame).best_effort_timestamp;

    //TODO forced fixed framerate

    // no timestamp available - extrapolate from previous frame duration
    if (*frame).pts == AV_NOPTS_VALUE {
        (*frame).pts = if dp.last_frame_pts == AV_NOPTS_VALUE {
            0
        } else {
            dp.last_frame_pts + dp.last_frame_duration_est
        }
    }

    // update timestamp history
    dp.last_frame_duration_est = video_duration_estimate(&dp, frame);
    dp.last_frame_pts = (*frame).pts;
    dp.last_frame_tb = (*frame).time_base;

    if dp.sar_override.num != 0 {
        (*frame).sample_aspect_ratio = dp.sar_override;
    }

    if dp.apply_cropping != 0 {
        let ret = av_frame_apply_cropping(frame, AV_FRAME_CROP_UNALIGNED as i32);
        if ret < 0 {
            error!("Error applying decoder cropping");
            return Err(Decoding(DecodingOperationError::CroppingError(
                DecodingError::from(ret),
            )));
        }
    }

    if !(*frame).opaque.is_null() {
        *outputs_mask = (*frame).opaque as usize;
    }

    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn video_duration_estimate(dp: &MutexGuard<DecoderParameter>, frame: *mut AVFrame) -> i64 {
    let mut codec_duration = 0;
    // difference between this and last frame's timestamps
    let ts_diff: i64 = if (*frame).pts != AV_NOPTS_VALUE && dp.last_frame_pts != AV_NOPTS_VALUE {
        (*frame).pts - dp.last_frame_pts
    } else {
        -1
    };

    // XXX lavf currently makes up frame durations when they are not provided by
    // the container. As there is no way to reliably distinguish real container
    // durations from the fake made-up ones, we use heuristics based on whether
    // the container has timestamps. Eventually lavf should stop making up
    // durations, then this should be simplified.

    // frame duration is unreliable (typically guessed by lavf) when it is equal
    // to 1 and the actual duration of the last frame is more than 2x larger
    let duration_unreliable = (*frame).duration == 1 && ts_diff > 2 * (*frame).duration;

    // prefer frame duration for containers with timestamps
    if (*frame).duration > 0 && !duration_unreliable {
        return (*frame).duration;
    }

    if (*dp.dec_ctx.as_ptr()).framerate.den != 0 && (*dp.dec_ctx.as_ptr()).framerate.num != 0 {
        let fields = (*frame).repeat_pict + 2;
        let field_rate = av_mul_q(
            (*dp.dec_ctx.as_ptr()).framerate,
            AVRational { num: 2, den: 1 },
        );
        codec_duration = av_rescale_q(fields as i64, av_inv_q(field_rate), (*frame).time_base);
    }

    // when timestamps are available, repeat last frame's actual duration
    // (i.e. pts difference between this and last frame)
    if ts_diff > 0 {
        return ts_diff;
    }

    // try frame/codec duration
    if (*frame).duration > 0 {
        return (*frame).duration;
    }
    if codec_duration > 0 {
        return codec_duration;
    }

    // try average framerate
    if dp.framerate_in.num != 0 && dp.framerate_in.den != 0 {
        let d = av_rescale_q(1, av_inv_q(dp.framerate_in), (*frame).time_base);
        if d > 0 {
            return d;
        }
    }

    // last resort is last frame's estimated duration, and 1
    std::cmp::max(dp.last_frame_duration_est, 1)
}

unsafe fn hwaccel_retrieve_data(
    dp: &MutexGuard<DecoderParameter>,
    input: *mut AVFrame,
    frame_pool: &ObjPool<Frame>,
) -> i32 {
    let output_format = dp.hwaccel_output_format;

    if (*input).format == output_format as i32 {
        // Nothing to do.
        return 0;
    }

    let Ok(mut output) = frame_pool.get() else {
        return AVERROR(ffmpeg_sys_next::ENOMEM);
    };

    (*output.as_mut_ptr()).format = output_format as i32;

    let mut err = av_hwframe_transfer_data(output.as_mut_ptr(), input, 0);
    if err < 0 {
        error!("Failed to transfer data to output frame: {err}");
        frame_pool.release(output);
        return err;
    }

    err = av_frame_copy_props(output.as_mut_ptr(), input);
    if err < 0 {
        frame_pool.release(output);
        return err;
    }

    av_frame_unref(input);
    av_frame_move_ref(input, output.as_mut_ptr());
    frame_pool.release(output);

    0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn audio_ts_process(mut dp: MutexGuard<DecoderParameter>, frame: *mut AVFrame) {
    let tb_filter = AVRational {
        num: 1,
        den: (*frame).sample_rate,
    };

    // on samplerate change, choose a new internal timebase for timestamp
    // generation that can represent timestamps from all the samplerates
    // seen so far
    let tb = audio_samplerate_update(&mut dp, frame);
    let pts_pred = if dp.last_frame_pts == AV_NOPTS_VALUE {
        0
    } else {
        dp.last_frame_pts + dp.last_frame_duration_est
    };

    if (*frame).pts == AV_NOPTS_VALUE {
        (*frame).pts = pts_pred;
        (*frame).time_base = tb;
    } else if dp.last_frame_pts != AV_NOPTS_VALUE
        && (*frame).pts > av_rescale_q_rnd(pts_pred, tb, (*frame).time_base, AV_ROUND_UP as u32)
    {
        // there was a gap in timestamps, reset conversion state
        dp.last_filter_in_rescale_delta = AV_NOPTS_VALUE;
    }

    (*frame).pts = av_rescale_delta(
        (*frame).time_base,
        (*frame).pts,
        tb,
        (*frame).nb_samples,
        &mut dp.last_filter_in_rescale_delta,
        tb,
    );

    dp.last_frame_pts = (*frame).pts;
    dp.last_frame_duration_est = av_rescale_q((*frame).nb_samples as i64, tb_filter, tb);

    // finally convert to filtering timebase
    (*frame).pts = av_rescale_q((*frame).pts, tb, tb_filter);
    (*frame).duration = (*frame).nb_samples as i64;
    (*frame).time_base = tb_filter;
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn audio_samplerate_update(
    dp: &mut MutexGuard<DecoderParameter>,
    frame: *mut AVFrame,
) -> AVRational {
    let prev = dp.last_frame_tb.den;
    let sr = (*frame).sample_rate;

    if (*frame).sample_rate == dp.last_frame_sample_rate {
        return dp.last_frame_tb;
    }

    let gcd = av_gcd(prev as i64, sr as i64);

    let mut tb_new = if prev as i64 / gcd >= (i32::MAX / sr) as i64 {
        warn!("Audio timestamps cannot be represented exactly after sample rate change: {prev} -> {sr}");

        // LCM of 192000, 44100, allows to represent all common samplerates
        AVRational {
            num: 1,
            den: 28224000,
        }
    } else {
        AVRational {
            num: 1,
            den: (prev as i64 / gcd) as i32 * sr,
        }
    };

    // keep the frame timebase if it is strictly better than
    // the samplerate-defined one
    if (*frame).time_base.num == 1
        && (*frame).time_base.den > tb_new.den
        && (*frame).time_base.den % tb_new.den == 0
    {
        tb_new = (*frame).time_base;
    }

    if dp.last_frame_pts != AV_NOPTS_VALUE {
        dp.last_frame_pts = av_rescale_q(dp.last_frame_pts, dp.last_frame_tb, tb_new);
    }
    dp.last_frame_duration_est = av_rescale_q(dp.last_frame_duration_est, dp.last_frame_tb, tb_new);

    dp.last_frame_tb = tb_new;
    dp.last_frame_sample_rate = (*frame).sample_rate;

    dp.last_frame_tb
}
