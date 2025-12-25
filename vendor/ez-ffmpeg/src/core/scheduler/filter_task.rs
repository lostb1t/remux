use crate::core::context::filter_graph::FilterGraph;
use crate::core::context::input_filter::{InputFilterOptions, IFILTER_FLAG_AUTOROTATE};
use crate::core::context::obj_pool::ObjPool;
use crate::core::context::output::VSyncMethod;
use crate::core::context::output::VSyncMethod::{VsyncCfr, VsyncVscfr};
use crate::core::context::output_filter::OutputFilterOptions;
use crate::core::context::{null_frame, FrameBox, FrameData};
use crate::core::scheduler::ffmpeg_scheduler::{
    frame_is_null, is_stopping, set_scheduler_error, wait_until_not_paused,
};
use crate::core::scheduler::input_controller::{InputController, SchNode};
use crate::error::{Error, FilterGraphError, FilterGraphOperationError, FilterGraphParseError};
use crate::hwaccel::{hw_device_for_filter, init_filter_hw_device, HWDevice};
use crate::util::ffmpeg_utils::av_rescale_q_rnd;
use crossbeam_channel::{RecvTimeoutError, Sender};
use ffmpeg_next::Frame;
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC;
use ffmpeg_sys_next::AVColorRange::AVCOL_RANGE_UNSPECIFIED;
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_UNSPECIFIED;
use ffmpeg_sys_next::AVFrameSideDataType::AV_FRAME_DATA_DISPLAYMATRIX;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_SUBTITLE, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::AVOptionType::AV_OPT_TYPE_BINARY;
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NONE;
use ffmpeg_sys_next::AVRounding::{AV_ROUND_NEAR_INF, AV_ROUND_PASS_MINMAX};
use ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_NONE;
use ffmpeg_sys_next::{
    av_bprint_chars, av_bprint_finalize, av_bprint_init, av_bprintf, av_buffer_ref,
    av_buffer_unref, av_buffersink_get_frame_flags, av_buffersink_get_frame_rate,
    av_buffersink_get_h, av_buffersink_get_sample_aspect_ratio, av_buffersink_get_sample_rate,
    av_buffersink_get_time_base, av_buffersink_get_w, av_buffersrc_add_frame,
    av_buffersrc_add_frame_flags, av_buffersrc_close, av_buffersrc_get_nb_failed_requests,
    av_buffersrc_parameters_alloc, av_buffersrc_parameters_set, av_color_range_name,
    av_color_space_name, av_dict_free, av_frame_alloc, av_frame_free, av_frame_get_side_data,
    av_frame_ref, av_frame_remove_side_data, av_freep, av_get_pix_fmt_name, av_get_sample_fmt_name,
    av_inv_q, av_log2, av_malloc, av_opt_find, av_opt_set, av_opt_set_bin, av_opt_set_int,
    av_pix_fmt_desc_get, av_q2d, av_rescale_q, avfilter_get_by_name,
    avfilter_graph_alloc, avfilter_graph_config, avfilter_graph_create_filter, avfilter_graph_free,
    avfilter_graph_request_oldest, avfilter_inout_free, avfilter_link, avfilter_pad_get_type,
    avio_close, avio_closep, avio_open, avio_open2, avio_read, avio_read_to_bprint, avio_size,
    AVBPrint, AVBufferRef, AVColorRange, AVColorSpace, AVFilterContext, AVFilterGraph,
    AVFilterInOut, AVFrame, AVMediaType, AVPixelFormat, AVRational, AVSampleFormat, AVERROR,
    AVERROR_BUG, AVERROR_EOF, AVERROR_OPTION_NOT_FOUND, AVIO_FLAG_READ, AV_BPRINT_SIZE_AUTOMATIC,
    AV_BUFFERSINK_FLAG_NO_REQUEST, AV_BUFFERSRC_FLAG_PUSH, AV_NOPTS_VALUE, AV_OPT_SEARCH_CHILDREN,
    AV_PIX_FMT_FLAG_HWACCEL, AV_TIME_BASE_Q, EAGAIN, EIO, ENOMEM,
};
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::{
    av_buffer_replace, av_buffersink_get_ch_layout, av_buffersink_get_color_range,
    av_buffersink_get_colorspace, av_buffersink_get_format, av_channel_layout_check,
    av_channel_layout_compare, av_channel_layout_copy, av_channel_layout_describe_bprint,
    av_channel_layout_uninit, av_dict_iterate, avfilter_graph_segment_apply,
    avfilter_graph_segment_create_filters, avfilter_graph_segment_free,
    avfilter_graph_segment_parse, AVChannelLayout, AVChannelLayout__bindgen_ty_1, AVChannelOrder,
    AVFilterGraphSegment, AVFILTER_FLAG_HWDEVICE, AVFILTER_FLAG_METADATA_ONLY, AV_FRAME_FLAG_KEY,
};
use log::{debug, error, info, trace, warn};
use std::collections::VecDeque;
use std::f64::consts::PI;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::util::ffmpeg_utils::av_err2str;

pub(crate) fn filter_graph_init(
    fg_index: usize,
    filter_graph: &mut FilterGraph,
    frame_pool: ObjPool<Frame>,
    input_controller: Arc<InputController>,
    filter_node: Arc<SchNode>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    if let Some(hw_device) = &filter_graph.hw_device {
        let err = init_filter_hw_device(hw_device);
        if err < 0 {
            return Err(Error::FilterGraph(FilterGraphOperationError::ParseError(
                FilterGraphParseError::from(err),
            )));
        }
    }

    let (src, finished_flag_list) = filter_graph.take_src();

    let input_len = filter_graph.inputs.len();
    let output_len = filter_graph.outputs.len();
    let graph_desc = filter_graph.graph_desc.clone();

    let mut ifps = Vec::with_capacity(input_len);
    for i in 0..input_len {
        let opts = std::mem::replace(
            &mut filter_graph.inputs[i].opts,
            InputFilterOptions::empty(),
        );

        let input_filter_parameter = InputFilterParameter::new(
            i,
            filter_graph.inputs[i].name.clone(),
            filter_graph.inputs[i].media_type,
            opts,
        );
        ifps.push(input_filter_parameter);
    }

    let mut ofps = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let output_filter_parameter = OutputFilterParameter::new(
            filter_graph.outputs[i].media_type,
            filter_graph.outputs[i].opts.clone(),
            filter_graph.outputs[i].take_dst(),
            filter_graph.outputs[i].fg_input_index,
            filter_graph.outputs[i].finished_flag_list.clone(),
        );
        ofps.push(output_filter_parameter);
    }

    let result = std::thread::Builder::new()
        .name(format!("filtergraph{fg_index}"))
        .spawn(move || {
            let mut graph: *mut AVFilterGraph = null_mut();
            let mut fgp = FilterGraphParameter::default();
            let node = filter_node.as_ref();
            let SchNode::Filter {
                inputs: _,
                best_input,
            } = node
            else {
                unreachable!()
            };

            loop {
                // update scheduling to account for desired input stream, if it changed
                //
                // this check needs no locking because only the filtering thread
                // updates this value
                if fgp.next_in != best_input.load(Ordering::Acquire) {
                    best_input.store(fgp.next_in, Ordering::Release);
                    input_controller.update_locked(&scheduler_status);
                }

                let result = src.recv_timeout(Duration::from_millis(100));

                if is_stopping(wait_until_not_paused(&scheduler_status)) {
                    info!("Filtergraph receiver end command, finishing.");
                    break;
                }

                if let Err(e) = result {
                    if e == RecvTimeoutError::Disconnected {
                        debug!("Filtergraph all src is finished.");
                        break;
                    }
                    continue;
                }

                let frame_box = result.unwrap();
                let input_index = frame_box.frame_data.fg_input_index;

                if input_index < finished_flag_list.len() {
                    if finished_flag_list[input_index].load(Ordering::Acquire) {
                       continue;
                    }
                } else {
                    unreachable!()
                }

                unsafe {
                    if ifps[input_index].media_type == AVMEDIA_TYPE_SUBTITLE {
                        //TODO
                        // sub2video_frame
                    } else if !frame_is_null(&frame_box.frame)
                        && !(*frame_box.frame.as_ptr()).buf[0].is_null()
                    {
                        if let Err(e) = fg_send_frame(
                            fg_index,
                            &mut graph,
                            &mut fgp,
                            &graph_desc,
                            frame_box,
                            &mut ifps,
                            &mut ofps,
                            input_index,
                            &frame_pool,
                        ) {
                            match e {
                                Error::FilterGraph(FilterGraphOperationError::BufferSourceAddFrameError(FilterGraphError::EOF)) => {
                                    debug!("Input {} no longer accepts new data", input_index);
                                    filter_receive_finish(&finished_flag_list, input_index);
                                }
                                e => {
                                    error!("filter_graph send_frame error: {e}");
                                    set_scheduler_error(&scheduler_status, &scheduler_result, e);
                                    break;
                                }
                            }
                        }
                    } else {
                        if let Err(e) = fg_send_eof(
                            fg_index,
                            &mut graph,
                            &mut fgp,
                            &graph_desc,
                            frame_box,
                            &mut ifps,
                            &mut ofps,
                            input_index,
                            &frame_pool,
                        ) {
                            match e {
                                Error::FilterGraph(FilterGraphOperationError::BufferSourceAddFrameError(FilterGraphError::EOF)) => {
                                    debug!("Input {} no longer accepts new data", input_index);
                                    filter_receive_finish(&finished_flag_list, input_index);
                                }
                                e => {
                                    error!("filter_graph send_eof error: {e}");
                                    set_scheduler_error(&scheduler_status, &scheduler_result, e);
                                    break;
                                }
                            }
                        }
                    }
                }

                if is_stopping(wait_until_not_paused(&scheduler_status)) {
                    info!("Filtergraph receiver end command, finishing.");
                    break;
                }

                unsafe {
                    let ret = fg_read_frames(graph, &mut fgp, &ifps, &mut ofps, &frame_pool);
                    if ret == AVERROR_EOF {
                        trace!("Filtergraph All consumers returned EOF");
                        break;
                    }
                    if ret < 0 {
                        set_scheduler_error(
                            &scheduler_status,
                            &scheduler_result,
                            Error::FilterGraph(FilterGraphOperationError::ProcessFramesError(
                                FilterGraphError::from(ret),
                            )),
                        );
                        cleanup_filtergraph(&mut graph, &mut ifps, &mut ofps);
                        break;
                    }
                }
            }

            for ofp in &mut ofps {
                let ret = unsafe { fg_output_frame(&mut fgp, ofp, null_frame(), &frame_pool) };
                if ret < 0 {
                    set_scheduler_error(
                        &scheduler_status,
                        &scheduler_result,
                        Error::FilterGraph(FilterGraphOperationError::SendFramesError(
                            FilterGraphError::from(ret),
                        )),
                    );
                    break;
                }
            }

            cleanup_filtergraph(&mut graph, &mut ifps, &mut ofps);
            debug!("FilterGraph finished.");
        });
    if let Err(e) = result {
        error!("FilterGraph thread exited with error: {e}");
        return Err(FilterGraphOperationError::ThreadExited.into());
    }

    Ok(())
}

fn filter_receive_finish(finished_flag_list: &Arc<[AtomicBool]>, input_index: usize) {
    if input_index < finished_flag_list.len() {
        if !finished_flag_list[input_index].load(Ordering::Acquire) {
            finished_flag_list[input_index].store(true, Ordering::Release);
        }
    } else {
        unreachable!()
    }
}

unsafe fn ifilter_has_all_input_formats(ifps: &Vec<InputFilterParameter>) -> bool {
    for ifp in ifps {
        if ifp.format < 0 {
            return false;
        }
    }
    true
}

struct InputFilterParameter {
    input_filter_index: usize,
    name: String,
    media_type: AVMediaType,
    opts: InputFilterOptions,
    hw_frames_ctx: *mut AVBufferRef,
    time_base: AVRational,
    format: i32,
    width: i32,
    height: i32,
    sample_aspect_ratio: AVRational,
    color_space: AVColorSpace,
    color_range: AVColorRange,
    sample_rate: i32,
    #[cfg(not(feature = "docs-rs"))]
    ch_layout: AVChannelLayout,
    displaymatrix_present: bool,
    displaymatrix_applied: bool,
    displaymatrix: [i32; 9],
    filter: *mut AVFilterContext,

    frame_queue: VecDeque<FrameBox>,

    eof: bool,
}

impl InputFilterParameter {
    fn new(
        input_filter_index: usize,
        name: String,
        media_type: AVMediaType,
        opts: InputFilterOptions,
    ) -> Self {
        Self {
            input_filter_index,
            name,
            media_type,
            opts,
            hw_frames_ctx: null_mut(),
            time_base: AVRational { num: 0, den: 1 },
            format: -1,
            width: 0,
            height: 0,
            sample_aspect_ratio: AVRational { num: 0, den: 1 },
            color_space: AVColorSpace::AVCOL_SPC_UNSPECIFIED,
            color_range: AVColorRange::AVCOL_RANGE_UNSPECIFIED,
            sample_rate: 0,
            #[cfg(not(feature = "docs-rs"))]
            ch_layout: AVChannelLayout {
                order: AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC,
                nb_channels: 0,
                u: AVChannelLayout__bindgen_ty_1 { mask: 0 },
                opaque: null_mut(),
            },
            displaymatrix_present: false,
            displaymatrix_applied: false,
            displaymatrix: [0; 9],
            filter: null_mut(),
            frame_queue: Default::default(),
            eof: false,
        }
    }
}

unsafe impl Send for InputFilterParameter {}
unsafe impl Sync for InputFilterParameter {}

#[derive(Default)]
struct FilterGraphParameter {
    // index of the next input to request from the scheduler
    next_in: usize,
    got_frame: bool,
    nb_outputs_done: usize,
    is_meta: bool,
}

struct OutputFilterParameter {
    media_type: AVMediaType,
    opts: OutputFilterOptions,
    dst: Option<Sender<FrameBox>>,

    fg_input_index: usize,
    finished_flag_list: Arc<[AtomicBool]>,

    filter: *mut AVFilterContext,
    name: String,
    fpsconv_context: FPSConvContext,
    eof: bool,

    format: AVPixelFormat,
    width: i32,
    height: i32,
    color_space: AVColorSpace,
    color_range: AVColorRange,
    sample_aspect_ratio: AVRational,
    sample_rate: i32,

    tb_out: AVRational,

    next_pts: i64,
}

impl OutputFilterParameter {
    fn new(
        media_type: AVMediaType,
        opts: OutputFilterOptions,
        dst: Option<Sender<FrameBox>>,
        fg_input_index: usize,
        finished_flag_list: Arc<[AtomicBool]>,
    ) -> Self {
        let mut fpsconv_context = FPSConvContext::default();
        fpsconv_context.framerate = opts.framerate;
        Self {
            media_type,
            dst,
            fg_input_index,
            finished_flag_list,
            filter: null_mut(),
            name: "".to_string(),
            opts,
            fpsconv_context,
            eof: false,
            format: AVPixelFormat::AV_PIX_FMT_NONE,
            width: 0,
            height: 0,
            color_space: AVColorSpace::AVCOL_SPC_RGB,
            color_range: AVColorRange::AVCOL_RANGE_UNSPECIFIED,
            sample_aspect_ratio: AVRational { num: 0, den: 0 },
            sample_rate: 0,
            tb_out: AVRational { num: 0, den: 0 },
            next_pts: 0,
        }
    }
}

unsafe impl Send for OutputFilterParameter {}
unsafe impl Sync for OutputFilterParameter {}

#[cfg(feature = "docs-rs")]
unsafe fn fg_send_eof(
    fg_index: usize,
    graph: *mut *mut AVFilterGraph,
    fgp: &mut FilterGraphParameter,
    graph_desc: &str,
    mut frame_box: FrameBox,
    ifps: &mut Vec<InputFilterParameter>,
    ofps: &mut Vec<OutputFilterParameter>,
    input_filter_index: usize,
    frame_pool: &ObjPool<Frame>,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn fg_send_eof(
    fg_index: usize,
    graph: *mut *mut AVFilterGraph,
    fgp: &mut FilterGraphParameter,
    graph_desc: &str,
    mut frame_box: FrameBox,
    ifps: &mut Vec<InputFilterParameter>,
    ofps: &mut Vec<OutputFilterParameter>,
    input_filter_index: usize,
    frame_pool: &ObjPool<Frame>,
) -> crate::error::Result<()> {
    let ifp = &ifps[input_filter_index];
    if ifp.eof {
        frame_pool.release(frame_box.frame);
        return Ok(());
    }

    let ifp = &mut ifps[input_filter_index];
    ifp.eof = true;

    let frame = frame_box.frame.as_mut_ptr();
    if !ifp.filter.is_null() {
        let pts = av_rescale_q_rnd(
            (*frame).pts,
            (*frame).time_base,
            ifp.time_base,
            AV_ROUND_NEAR_INF as u32 | AV_ROUND_PASS_MINMAX as u32,
        );
        let ret = av_buffersrc_close(ifp.filter, pts, AV_BUFFERSRC_FLAG_PUSH as u32);
        frame_pool.release(frame_box.frame);
        if ret < 0 {
            return Err(Error::FilterGraph(
                FilterGraphOperationError::BufferSourceCloseError(FilterGraphError::from(ret)),
            ));
        }
    } else {
        if ifp.format < 0 {
            // the filtergraph was never configured, use the fallback parameters

            let ifp = &mut ifps[input_filter_index];
            ifp.format = (*ifp.opts.fallback.as_ptr()).format;
            ifp.sample_rate = (*ifp.opts.fallback.as_ptr()).sample_rate;
            ifp.width = (*ifp.opts.fallback.as_ptr()).width;
            ifp.height = (*ifp.opts.fallback.as_ptr()).height;
            ifp.sample_aspect_ratio = (*ifp.opts.fallback.as_ptr()).sample_aspect_ratio;
            ifp.color_space = (*ifp.opts.fallback.as_ptr()).colorspace;
            ifp.color_range = (*ifp.opts.fallback.as_ptr()).color_range;
            ifp.time_base = (*ifp.opts.fallback.as_ptr()).time_base;

            let ret = av_channel_layout_copy(
                &mut ifp.ch_layout,
                &(*ifp.opts.fallback.as_ptr()).ch_layout,
            );
            if ret < 0 {
                frame_pool.release(frame_box.frame);
                return Err(Error::FilterGraph(
                    FilterGraphOperationError::ChannelLayoutCopyError(FilterGraphError::from(ret)),
                ));
            }

            if ifilter_has_all_input_formats(ifps) {
                if let Err(e) = configure_filtergraph(fg_index, graph, fgp, graph_desc, ifps, ofps)
                {
                    error!("Error initializing filters! {e}");
                    frame_pool.release(frame_box.frame);
                    return Err(e);
                }
            }
        }

        let ifp = &ifps[input_filter_index];
        if ifp.format < 0 {
            error!(
                "Cannot determine format of input {} after EOF",
                ifp.opts.name
            );
            return Err(Error::FilterGraph(FilterGraphOperationError::InvalidData));
        }
    }

    Ok(())
}

const VIDEO_CHANGED: i32 = 1 << 0;
const AUDIO_CHANGED: i32 = 1 << 1;
const MATRIX_CHANGED: i32 = 1 << 2;
const HWACCEL_CHANGED: i32 = 1 << 3;
#[cfg(feature = "docs-rs")]
unsafe fn fg_send_frame(
    fg_index: usize,
    graph: *mut *mut AVFilterGraph,
    fgp: &mut FilterGraphParameter,
    graph_desc: &str,
    mut frame_box: FrameBox,
    ifps: &mut Vec<InputFilterParameter>,
    ofps: &mut Vec<OutputFilterParameter>,
    input_filter_index: usize,
    frame_pool: &ObjPool<Frame>,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn fg_send_frame(
    fg_index: usize,
    graph: *mut *mut AVFilterGraph,
    fgp: &mut FilterGraphParameter,
    graph_desc: &str,
    mut frame_box: FrameBox,
    ifps: &mut Vec<InputFilterParameter>,
    ofps: &mut Vec<OutputFilterParameter>,
    input_filter_index: usize,
    frame_pool: &ObjPool<Frame>,
) -> crate::error::Result<()> {
    let frame = frame_box.frame.as_mut_ptr();
    let mut need_reinit = 0;

    let ifp = &ifps[input_filter_index];
    let media_type = ifp.media_type;
    // determine if the parameters for this input changed
    // Check audio or video parameters
    match media_type {
        AVMEDIA_TYPE_AUDIO => {
            if ifp.format != (*frame).format
                || ifp.sample_rate != (*frame).sample_rate
                || unsafe { av_channel_layout_compare(&ifp.ch_layout, &(*frame).ch_layout) } != 0
            {
                need_reinit |= AUDIO_CHANGED;
            }
        }
        AVMEDIA_TYPE_VIDEO => {
            if ifp.format != (*frame).format
                || ifp.width != (*frame).width
                || ifp.height != (*frame).height
                || ifp.color_space != (*frame).colorspace
                || ifp.color_range != (*frame).color_range
            {
                need_reinit |= VIDEO_CHANGED;
            }
        }
        _ => {}
    }

    // Check display matrix
    let sd = av_frame_get_side_data(frame, AV_FRAME_DATA_DISPLAYMATRIX);
    if !sd.is_null() {
        if !ifp.displaymatrix_present
            || std::slice::from_raw_parts((*sd).data, size_of_val(&ifp.displaymatrix))
                != std::slice::from_raw_parts(
                    ifp.displaymatrix.as_ptr() as *const u8,
                    size_of_val(&ifp.displaymatrix),
                )
        {
            need_reinit |= MATRIX_CHANGED;
        }
    } else if ifp.displaymatrix_present {
        need_reinit |= MATRIX_CHANGED;
    }

    // Always allow reinit
    /*if !graph.is_null() && !(ifp.opts.flags & IFILTER_FLAG_REINIT){
        need_reinit = 0;
    }*/

    if (ifp.hw_frames_ctx.is_null() != (*frame).hw_frames_ctx.is_null())
        || (!ifp.hw_frames_ctx.is_null()
            && (*ifp.hw_frames_ctx).data != (*(*frame).hw_frames_ctx).data)
    {
        need_reinit |= HWACCEL_CHANGED;
    }

    // Reinitialize if needed
    if need_reinit != 0 || (*graph).is_null() {
        ifilter_parameters_from_frame(&mut ifps[input_filter_index], frame, media_type)?;

        if !ifilter_has_all_input_formats(ifps) {
            let _ = &mut ifps[input_filter_index].frame_queue.push_back(frame_box);
            return Ok(());
        }

        if !(*graph).is_null() {
            let ret = fg_read_frames(*graph, fgp, ifps, ofps, frame_pool);
            if ret < 0 {
                return Err(Error::FilterGraph(
                    FilterGraphOperationError::ProcessFramesError(FilterGraphError::from(ret)),
                ));
            }

            let mut reason: AVBPrint = std::mem::zeroed();
            av_bprint_init(&mut reason, 0, AV_BPRINT_SIZE_AUTOMATIC as u32);

            if need_reinit & AUDIO_CHANGED != 0 {
                let fmt_str = CString::new("audio parameters changed to %d Hz, \0").unwrap();
                let sample_format_name =
                    av_get_sample_fmt_name(std::mem::transmute((*frame).format));
                av_bprintf(&mut reason, fmt_str.as_ptr(), (*frame).sample_rate);
                av_channel_layout_describe_bprint(&(*frame).ch_layout, &mut reason);
                let comma_str = CString::new(", %s, \0").unwrap();
                av_bprintf(
                    &mut reason,
                    comma_str.as_ptr(),
                    unknown_if_null(sample_format_name).as_ptr(),
                );
            }

            if need_reinit & VIDEO_CHANGED != 0 {
                let pixel_format_name = av_get_pix_fmt_name(std::mem::transmute((*frame).format));
                let color_space_name = av_color_space_name((*frame).colorspace);
                let color_range_name = av_color_range_name((*frame).color_range);

                let fmt_str =
                    CString::new("video parameters changed to %s(%s, %s), %dx%d, \0").unwrap();
                av_bprintf(
                    &mut reason,
                    fmt_str.as_ptr(),
                    unknown_if_null(pixel_format_name).as_ptr(),
                    unknown_if_null(color_range_name).as_ptr(),
                    unknown_if_null(color_space_name).as_ptr(),
                    (*frame).width,
                    (*frame).height,
                );
            }

            if need_reinit & MATRIX_CHANGED != 0 {
                let matrix_changed_str = CString::new("display matrix changed, \0").unwrap();
                av_bprintf(&mut reason, matrix_changed_str.as_ptr());
            }

            if need_reinit & HWACCEL_CHANGED != 0 {
                let hwaccel_changed_str = CString::new("hwaccel changed, \0").unwrap();
                av_bprintf(&mut reason, hwaccel_changed_str.as_ptr());
            }

            // Remove the last comma if necessary
            if reason.len > 1 {
                let len = reason.len as usize;
                let reason_ptr = reason.str_ as *mut u8;
                *reason_ptr.add(len - 2) = 0; // Set the last comma to null terminator
            }

            let reason_str = CStr::from_ptr(reason.str_)
                .to_str()
                .unwrap_or("Unknown reason")
                .to_string();
            info!("Reconfiguring filter because graph {}", reason_str);
        }

        if let Err(e) = configure_filtergraph(fg_index, graph, fgp, graph_desc, ifps, ofps) {
            error!("Error reinitializing filters! {}", e);
            frame_pool.release(frame_box.frame);
            return Err(e);
        }
    }

    let ifp = &ifps[input_filter_index];

    (*frame).pts = av_rescale_q((*frame).pts, (*frame).time_base, ifp.time_base);
    (*frame).duration = av_rescale_q((*frame).duration, (*frame).time_base, ifp.time_base);
    (*frame).time_base = ifp.time_base;

    if ifp.displaymatrix_applied {
        av_frame_remove_side_data(frame, AV_FRAME_DATA_DISPLAYMATRIX);
    }

    let ret = av_buffersrc_add_frame_flags(ifp.filter, frame, AV_BUFFERSRC_FLAG_PUSH as u32 as i32);
    frame_pool.release(frame_box.frame);
    if ret < 0 {
        if ret != AVERROR_EOF {
            error!("Error while filtering: {}", av_err2str(ret));
        }
        return Err(Error::FilterGraph(
            FilterGraphOperationError::BufferSourceAddFrameError(FilterGraphError::from(ret)),
        ));
    }
    Ok(())
}

#[cfg(feature = "docs-rs")]
unsafe fn configure_filtergraph(
    fg_index: usize,
    mut graph: *mut *mut AVFilterGraph,
    fgp: &mut FilterGraphParameter,
    graph_desc: &str,
    ifps: &mut Vec<InputFilterParameter>,
    ofps: &mut Vec<OutputFilterParameter>,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn configure_filtergraph(
    fg_index: usize,
    graph: *mut *mut AVFilterGraph,
    fgp: &mut FilterGraphParameter,
    graph_desc: &str,
    ifps: &mut Vec<InputFilterParameter>,
    ofps: &mut Vec<OutputFilterParameter>,
) -> crate::error::Result<()> {
    cleanup_filtergraph(graph, ifps, ofps);
    *graph = avfilter_graph_alloc();
    (**graph).nb_threads = 0;

    let hw_device = hw_device_for_filter();

    let mut inputs = null_mut();
    let mut outputs = null_mut();
    let mut ret = graph_parse(*graph, graph_desc, &mut inputs, &mut outputs, hw_device);
    if ret < 0 {
        cleanup_filtergraph(graph, ifps, ofps);
        return Err(Error::FilterGraph(FilterGraphOperationError::ParseError(
            FilterGraphParseError::from(ret),
        )));
    }

    let mut cur = inputs;
    let mut i = 0;
    loop {
        if cur.is_null() {
            break;
        }

        let ifp = ifps.get_mut(i);
        if ifp.is_none() {
            error!("input[{i}] can't find matched InputFilterParameter");
            break;
        }

        let ret = configure_input_filter(fg_index, *graph, ifp.unwrap(), cur);
        if ret < 0 {
            avfilter_inout_free(&mut inputs);
            avfilter_inout_free(&mut outputs);
            cleanup_filtergraph(graph, ifps, ofps);
            return Err(Error::FilterGraph(FilterGraphOperationError::ParseError(
                FilterGraphParseError::from(ret),
            )));
        }

        cur = (*cur).next;
        i += 1;
    }
    avfilter_inout_free(&mut inputs);

    cur = outputs;
    i = 0;
    loop {
        if cur.is_null() {
            break;
        }

        let ofp = ofps.get_mut(i);
        if ofp.is_none() {
            error!("output[{i}] can't find matched OutputFilterParameter");
            break;
        }

        let ret = configure_output_filter(*graph, ofp.unwrap(), cur);
        if ret < 0 {
            avfilter_inout_free(&mut outputs);
            cleanup_filtergraph(graph, ifps, ofps);
            return Err(Error::FilterGraph(FilterGraphOperationError::ParseError(
                FilterGraphParseError::from(ret),
            )));
        }

        cur = (*cur).next;
        i += 1;
    }
    avfilter_inout_free(&mut outputs);

    //TODO disable_conversions

    ret = avfilter_graph_config(*graph, null_mut());
    if ret < 0 {
        cleanup_filtergraph(graph, ifps, ofps);
        return Err(Error::FilterGraph(FilterGraphOperationError::ParseError(
            FilterGraphParseError::from(ret),
        )));
    }

    fgp.is_meta = graph_is_meta(*graph);

    /* limit the lists of allowed formats to the ones selected, to
     * make sure they stay the same if the filtergraph is reconfigured later */
    for ofp in &mut *ofps {
        ofp.format = std::mem::transmute(av_buffersink_get_format(ofp.filter));
        ofp.width = av_buffersink_get_w(ofp.filter);
        ofp.height = av_buffersink_get_h(ofp.filter);
        ofp.color_space = av_buffersink_get_colorspace(ofp.filter);
        ofp.color_range = av_buffersink_get_color_range(ofp.filter);

        ofp.tb_out = av_buffersink_get_time_base(ofp.filter);

        ofp.sample_aspect_ratio = av_buffersink_get_sample_aspect_ratio(ofp.filter);

        ofp.sample_rate = av_buffersink_get_sample_rate(ofp.filter);

        av_channel_layout_uninit(&mut ofp.opts.ch_layout);
        ret = av_buffersink_get_ch_layout(ofp.filter, &mut ofp.opts.ch_layout);
        if ret < 0 {
            cleanup_filtergraph(graph, ifps, ofps);
            return Err(Error::FilterGraph(FilterGraphOperationError::ParseError(
                FilterGraphParseError::from(ret),
            )));
        }
    }

    for ifp in &mut *ifps {
        loop {
            let option = ifp.frame_queue.pop_front();
            if option.is_none() {
                break;
            }
            let tmp_frame_box = option.unwrap();
            if ifp.media_type == AVMEDIA_TYPE_SUBTITLE {
                //TODO
                // sub2video_frame(ifp, tmp_frame_box);
            } else {
                let mut tmp_frame = unsafe { av_frame_alloc() };
                if tmp_frame.is_null() {
                    return Err(Error::FilterGraph(
                        FilterGraphOperationError::BufferSourceAddFrameError(
                            FilterGraphError::OutOfMemory,
                        ),
                    ));
                }
                ret = av_frame_ref(tmp_frame, tmp_frame_box.frame.as_ptr());
                if ret < 0 {
                    return Err(Error::FilterGraph(
                        FilterGraphOperationError::BufferSourceAddFrameError(
                            FilterGraphError::from(ret),
                        ),
                    ));
                }
                ret = av_buffersrc_add_frame(ifp.filter, tmp_frame);
                if ret < 0 {
                    av_frame_free(&mut tmp_frame);
                }
            }
        }
    }

    if ret < 0 {
        cleanup_filtergraph(graph, ifps, ofps);
        return Err(Error::FilterGraph(
            FilterGraphOperationError::BufferSourceAddFrameError(FilterGraphError::from(ret)),
        ));
    }

    let mut have_input_eof = false;
    /* send the EOFs for the finished inputs */
    for ifp in &mut *ifps {
        if ifp.eof {
            ret = av_buffersrc_add_frame(ifp.filter, null_mut());
            if ret < 0 {
                cleanup_filtergraph(graph, ifps, ofps);
                return Err(Error::FilterGraph(
                    FilterGraphOperationError::BufferSourceAddFrameError(FilterGraphError::from(
                        ret,
                    )),
                ));
            }
            have_input_eof = true;
        }
    }

    if have_input_eof {
        // make sure the EOF propagates to the end of the graph
        ret = avfilter_graph_request_oldest(*graph);
        if ret < 0 && ret != AVERROR(EAGAIN) && ret != AVERROR_EOF {
            cleanup_filtergraph(graph, ifps, ofps);
            return Err(Error::FilterGraph(
                FilterGraphOperationError::RequestOldestError(FilterGraphError::from(ret)),
            ));
        }
    }

    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn graph_is_meta(graph: *mut AVFilterGraph) -> bool {
    for i in 0..(*graph).nb_filters {
        unsafe {
            let filter_context = *(*graph).filters.add(i as usize);
            let filter = (*filter_context).filter;

            if !((*filter).flags & AVFILTER_FLAG_METADATA_ONLY != 0
                || (*filter_context).nb_outputs == 0
                || filter_is_buffersrc(filter_context))
            {
                return false;
            }
        }
    }
    true
}

fn filter_is_buffersrc(f: *mut AVFilterContext) -> bool {
    unsafe {
        (*f).nb_inputs == 0
            && (CStr::from_ptr((*(*f).filter).name)
                == CStr::from_bytes_with_nul(b"buffer\0").unwrap()
                || CStr::from_ptr((*(*f).filter).name)
                    == CStr::from_bytes_with_nul(b"abuffer\0").unwrap())
    }
}

unsafe fn configure_output_filter(
    graph: *mut AVFilterGraph,
    ofp: &mut OutputFilterParameter,
    output: *mut AVFilterInOut,
) -> i32 {
    match ofp.media_type {
        AVMEDIA_TYPE_VIDEO => configure_output_video_filter(graph, ofp, output),
        AVMEDIA_TYPE_AUDIO => configure_output_audio_filter(graph, ofp, output),
        _ => {
            error!("Unexpected media type {:?}", ofp.media_type);
            0
        }
    }
}

#[cfg(feature = "docs-rs")]
unsafe fn configure_output_audio_filter(
    graph: *mut AVFilterGraph,
    ofp: &mut OutputFilterParameter,
    output: *mut AVFilterInOut,
) -> i32 {
    0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn configure_output_audio_filter(
    graph: *mut AVFilterGraph,
    ofp: &mut OutputFilterParameter,
    output: *mut AVFilterInOut,
) -> i32 {
    let mut pad_idx = (*output).pad_idx;
    let mut last_filter = (*output).filter_ctx;

    let result = CString::new(format!("out_{}", ofp.opts.name));
    if let Err(_) = result {
        return AVERROR(ENOMEM);
    }
    let name = result.unwrap();

    let abuffer_str = std::ffi::CString::new("abuffersink").unwrap();
    let abuffer_filter = avfilter_get_by_name(abuffer_str.as_ptr());

    let ret = avfilter_graph_create_filter(
        &mut ofp.filter,
        abuffer_filter,
        name.as_ptr(),
        null(),
        null_mut(),
        graph,
    );
    if ret < 0 {
        return ret;
    }

    let all_channel_counts_str = std::ffi::CString::new("all_channel_counts").unwrap();
    let mut ret = av_opt_set_int(
        ofp.filter as *mut _,
        all_channel_counts_str.as_ptr(),
        1,
        AV_OPT_SEARCH_CHILDREN,
    );
    if ret < 0 {
        return ret;
    }

    let mut bprint = AVBPrint {
        str_: null_mut(),
        len: 0,
        size: 0,
        size_max: 0,
        reserved_internal_buffer: [0; 1],
        reserved_padding: [0; 1000],
    };
    av_bprint_init(&mut bprint, 0, u32::MAX);

    choose_sample_fmts(
        &mut bprint,
        ofp.opts.audio_format,
        ofp.opts.audio_formats.clone(),
    );
    choose_sample_rates(
        &mut bprint,
        ofp.opts.sample_rate,
        ofp.opts.sample_rates.clone(),
    );
    choose_channel_layouts(&mut bprint, ofp.opts.ch_layout, ofp.opts.ch_layouts.clone());

    if bprint.len >= bprint.size {
        av_bprint_finalize(&mut bprint, null_mut());
        return AVERROR(ENOMEM);
    }

    if bprint.len > 0 {
        let mut filter = null_mut();

        let result = CString::new(format!("format_out_{}", ofp.name));
        if let Err(_) = result {
            av_bprint_finalize(&mut bprint, null_mut());
            return AVERROR(ENOMEM);
        }
        let name = result.unwrap();

        let format_out_str = CString::new("aformat").unwrap();
        let format_out_filter = avfilter_get_by_name(format_out_str.as_ptr());
        let mut ret = avfilter_graph_create_filter(
            &mut filter,
            format_out_filter,
            name.as_ptr(),
            bprint.str_,
            null_mut(),
            graph,
        );
        if ret < 0 {
            av_bprint_finalize(&mut bprint, null_mut());
            return ret;
        }

        ret = avfilter_link(last_filter, pad_idx as u32, filter, 0);
        if ret < 0 {
            av_bprint_finalize(&mut bprint, null_mut());
            return ret;
        }

        last_filter = filter;
        pad_idx = 0;
    }

    //TODO auto apad

    let name = format!("trim_out_{}", ofp.name);
    ret = insert_trim(
        ofp.opts.trim_start_us,
        ofp.opts.trim_duration_us,
        &mut last_filter,
        &mut pad_idx,
        &name,
    );
    if ret < 0 {
        return ret;
    }

    ret = avfilter_link(last_filter, pad_idx as u32, ofp.filter, 0);
    if ret < 0 {
        av_bprint_finalize(&mut bprint, null_mut());
        return ret;
    }

    0
}

fn insert_trim(
    start_time: Option<i64>,
    duration: Option<i64>,
    last_filter: *mut *mut AVFilterContext,
    pad_idx: &mut i32,
    filter_name: &str,
) -> i32 {
    if start_time.is_none() && duration.is_none() {
        return 0;
    }
    unsafe {
        let graph = (**last_filter).graph;
        let type_ = avfilter_pad_get_type((**last_filter).output_pads, *pad_idx);
        let name = if type_ == AVMEDIA_TYPE_VIDEO {
            "trim".to_string()
        } else {
            "atrim".to_string()
        };
        let mut ret = 0;

        let name_cstring = CString::new(name.clone()).unwrap();
        let trim = avfilter_get_by_name(name_cstring.as_ptr());
        if trim.is_null() {
            error!("{name} filter not present, cannot limit recording time.");
            return ffmpeg_sys_next::AVERROR_FILTER_NOT_FOUND;
        }

        let filter_name_cstring = CString::new(filter_name).unwrap();
        let ctx =
            ffmpeg_sys_next::avfilter_graph_alloc_filter(graph, trim, filter_name_cstring.as_ptr());
        if ctx.is_null() {
            return AVERROR(ENOMEM);
        }

        if let Some(duration) = duration {
            let durationi_cstring = CString::new("durationi").unwrap();

            ret = av_opt_set_int(
                ctx as *mut _,
                durationi_cstring.as_ptr(),
                duration,
                AV_OPT_SEARCH_CHILDREN,
            );
        }

        if ret >= 0 {
            if let Some(start_time) = start_time {
                let starti_cstring = CString::new("starti").unwrap();
                ret = av_opt_set_int(
                    ctx as *mut _,
                    starti_cstring.as_ptr(),
                    start_time,
                    AV_OPT_SEARCH_CHILDREN,
                );
            }
        }
        if ret < 0 {
            error!("Error configuring the {name} filter");
            return ret;
        }

        ret = ffmpeg_sys_next::avfilter_init_str(ctx, null_mut());
        if ret < 0 {
            return ret;
        }

        ret = avfilter_link(*last_filter, *pad_idx as u32, ctx, 0);
        if ret < 0 {
            return ret;
        }

        *last_filter = ctx;
        *pad_idx = 0;
    }

    0
}

fn cleanup_filtergraph(
    graph: *mut *mut AVFilterGraph,
    ifps: &mut Vec<InputFilterParameter>,
    ofps: &mut Vec<OutputFilterParameter>,
) {
    for input_filter_parameter in ifps {
        input_filter_parameter.filter = null_mut();
        if !input_filter_parameter.hw_frames_ctx.is_null() {
            unsafe { av_buffer_unref(&mut input_filter_parameter.hw_frames_ctx) };
        }
    }
    for output_filter_parameter in ofps {
        output_filter_parameter.filter = null_mut();
    }
    if graph.is_null() {
        return;
    }
    unsafe {
        avfilter_graph_free(graph);
    }
}

unsafe fn fg_read_frames(
    graph: *mut AVFilterGraph,
    fgp: &mut FilterGraphParameter,
    ifps: &Vec<InputFilterParameter>,
    ofps: &mut Vec<OutputFilterParameter>,
    frame_pool: &ObjPool<Frame>,
) -> i32 {
    // graph not configured, just select the input to request
    if graph.is_null() {
        for ifp in ifps {
            if ifp.format < 0 && !ifp.eof {
                fgp.next_in = ifp.input_filter_index;
                return 0;
            }
        }
        return AVERROR_BUG;
    }

    loop {
        if fgp.nb_outputs_done >= ofps.len() {
            break;
        }

        let ret = avfilter_graph_request_oldest(graph);
        if ret == AVERROR(EAGAIN) {
            fgp.next_in = choose_input(ifps);
            break;
        } else if ret < 0 {
            if ret == AVERROR_EOF {
                trace!("Filtergraph returned EOF, finishing");
            } else {
                error!("Error requesting a frame from the filtergraph: {}", av_err2str(ret));
            }
            return ret;
        }

        //TODO rate-control

        for ofp in &mut *ofps {
            loop {
                let ret = fg_output_step(fgp, ofp, frame_pool);
                if ret < 0 {
                    return ret;
                }
                if ret != 0 {
                    break;
                }
            }
        }
    }

    if fgp.nb_outputs_done == ofps.len() {
        AVERROR_EOF
    } else {
        0
    }
}

fn choose_input(ifps: &Vec<InputFilterParameter>) -> usize {
    let mut nb_requests_max = -1;
    let mut best_input: i32 = -1;

    for ifp in ifps {
        if ifp.eof {
            continue;
        }
        let nb_requests = unsafe { av_buffersrc_get_nb_failed_requests(ifp.filter) } as i32;
        if nb_requests > nb_requests_max {
            nb_requests_max = nb_requests;
            best_input = ifp.input_filter_index as i32;
        }
    }

    assert!(best_input >= 0);

    best_input as usize
}

#[cfg(feature = "docs-rs")]
unsafe fn fg_output_step(
    fgp: &mut FilterGraphParameter,
    ofp: &mut OutputFilterParameter,
    frame_pool: &ObjPool<Frame>,
) -> i32 {
    0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn fg_output_step(
    fgp: &mut FilterGraphParameter,
    ofp: &mut OutputFilterParameter,
    frame_pool: &ObjPool<Frame>,
) -> i32 {
    let Ok(mut frame) = frame_pool.get() else {
        return AVERROR(ffmpeg_sys_next::ENOMEM);
    };
    let mut ret = av_buffersink_get_frame_flags(
        ofp.filter,
        frame.as_mut_ptr(),
        AV_BUFFERSINK_FLAG_NO_REQUEST,
    );

    if ret == AVERROR_EOF && !ofp.eof {
        frame_pool.release(frame);
        ret = fg_output_frame(fgp, ofp, null_frame(), frame_pool);
        return if ret < 0 { ret } else { 1 };
    } else if ret == AVERROR(EAGAIN) || ret == AVERROR_EOF {
        frame_pool.release(frame);
        return 1;
    } else if ret < 0 {
        frame_pool.release(frame);
        warn!("Error in retrieving a frame from the filtergraph: {}", av_err2str(ret));
        return ret;
    }

    if ofp.eof {
        frame_pool.release(frame);
        return 0;
    }

    // Choose the output timebase the first time we get a frame.
    choose_out_timebase(ofp, &frame);
    (*frame.as_mut_ptr()).time_base = av_buffersink_get_time_base(ofp.filter);

    /*if !fgp.is_meta {

    }*/

    if ofp.media_type == AVMEDIA_TYPE_VIDEO {
        if (*frame.as_ptr()).duration == 0 {
            let fr = av_buffersink_get_frame_rate(ofp.filter);
            if fr.num > 0 && fr.den > 0 {
                (*frame.as_mut_ptr()).duration =
                    av_rescale_q(1, av_inv_q(fr), (*frame.as_ptr()).time_base);
            }
        }
    }

    ret = fg_output_frame(fgp, ofp, frame, frame_pool);
    if ret < 0 {
        return ret;
    }

    0
}

unsafe fn choose_out_timebase(ofp: &mut OutputFilterParameter, frame: &Frame) {
    let mut tb = AVRational { num: 0, den: 0 };
    if ofp.media_type == AVMEDIA_TYPE_AUDIO {
        tb = AVRational {
            num: 1,
            den: (*frame.as_ptr()).sample_rate,
        };
        ofp.tb_out = tb;
        return;
    }

    let mut fr = ofp.fpsconv_context.framerate;
    if fr.num == 0 {
        let fr_sink = av_buffersink_get_frame_rate(ofp.filter);
        if fr_sink.num > 0 && fr_sink.den > 0 {
            fr = fr_sink;
        }
    }

    if let Some(vsync_method) = ofp.opts.vsync_method {
        if vsync_method == VsyncCfr || vsync_method == VsyncVscfr {
            if fr.num == 0 && ofp.fpsconv_context.framerate_max.num == 0 {
                fr = AVRational { num: 25, den: 1 };
                warn!(
                    "No information \
                about the input framerate is available. Falling \
                back to a default value of 25fps. Use the `framerate` option \
                if you want a different framerate."
                );
            }

            if ofp.fpsconv_context.framerate_max.num != 0
                && (av_q2d(fr) > av_q2d(ofp.fpsconv_context.framerate_max) || fr.den != 0)
            {
                fr = ofp.fpsconv_context.framerate_max;
            }
        }
    }

    if !(tb.num > 0 && tb.den > 0) {
        tb = av_inv_q(fr);
    }
    if !(tb.num > 0 && tb.den > 0) {
        tb = (*frame.as_ptr()).time_base;
    }

    ofp.fpsconv_context.framerate = fr;

    ofp.tb_out = tb;
}

#[cfg(feature = "docs-rs")]
unsafe fn fg_output_frame(
    fgp: &mut FilterGraphParameter,
    ofp: &mut OutputFilterParameter,
    mut frame: Frame,
    frame_pool: &ObjPool<Frame>,
) -> i32 {
    0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn fg_output_frame(
    fgp: &mut FilterGraphParameter,
    ofp: &mut OutputFilterParameter,
    mut frame: Frame,
    frame_pool: &ObjPool<Frame>,
) -> i32 {
    if !ofp.finished_flag_list.is_empty() && ofp.fg_input_index < ofp.finished_flag_list.len() {
        if ofp.finished_flag_list[ofp.fg_input_index].load(Ordering::Acquire) {
            ofp.eof = true;
            fgp.nb_outputs_done += 1;
            return 0;
        }
    }

    let mut nb_frames = if !frame_is_null(&frame) { 1 } else { 0 };
    let mut nb_frames_prev = 0;

    if ofp.media_type == AVMEDIA_TYPE_VIDEO && (!frame_is_null(&frame) || fgp.got_frame) {
        unsafe { video_sync_process(ofp, frame.as_mut_ptr(), &mut nb_frames, &mut nb_frames_prev) };
    }

    let frame_prev = &ofp.fpsconv_context.last_frame;
    for i in 0..nb_frames {
        let frame_out = if ofp.media_type == AVMEDIA_TYPE_VIDEO {
            let frame_in = if i < nb_frames_prev && !(*frame_prev.as_ptr()).buf[0].is_null() {
                frame_prev
            } else {
                &frame
            };

            if frame_is_null(&frame_in) {
                break;
            }

            let Ok(mut out) = frame_pool.get() else {
                return AVERROR(ffmpeg_sys_next::ENOMEM);
            };
            let ret = av_frame_ref(out.as_mut_ptr(), frame_in.as_ptr());
            if ret < 0 {
                return ret;
            }

            (*out.as_mut_ptr()).pts = ofp.next_pts;

            if ofp.fpsconv_context.dropped_keyframe {
                (*out.as_mut_ptr()).flags |= AV_FRAME_FLAG_KEY;
                ofp.fpsconv_context.dropped_keyframe = false;
            }

            out
        } else {
            // let frame = frame;
            let Ok(mut out) = frame_pool.get() else {
                return AVERROR(ffmpeg_sys_next::ENOMEM);
            };
            (*frame.as_mut_ptr()).pts = if (*frame.as_ptr()).pts == AV_NOPTS_VALUE {
                ofp.next_pts
            } else {
                av_rescale_q(
                    (*frame.as_ptr()).pts,
                    (*frame.as_ptr()).time_base,
                    ofp.tb_out,
                ) - av_rescale_q(ofp.opts.ts_offset.unwrap_or(0), AV_TIME_BASE_Q, ofp.tb_out)
            };
            (*frame.as_mut_ptr()).time_base = ofp.tb_out;
            (*frame.as_mut_ptr()).duration = av_rescale_q(
                (*frame.as_ptr()).nb_samples as i64,
                AVRational {
                    num: 1,
                    den: (*frame.as_ptr()).sample_rate,
                },
                ofp.tb_out,
            );

            ofp.next_pts = (*frame.as_ptr()).pts + (*frame.as_ptr()).duration;

            let ret = av_frame_ref(out.as_mut_ptr(), frame.as_ptr());
            if ret < 0 {
                return ret;
            }
            out
        };

        // send the frame to consumers
        if let Some(dst) = ofp.dst.as_ref() {
            let framerate = if ofp.opts.framerate.den == 0 {
                None
            } else {
                Some(ofp.opts.framerate)
            };

            let frame_box = FrameBox {
                frame: frame_out,
                frame_data: FrameData {
                    framerate,
                    bits_per_raw_sample: 0,
                    input_stream_width: 0,
                    input_stream_height: 0,
                    subtitle_header_size: 0,
                    subtitle_header: null_mut(),
                    fg_input_index: ofp.fg_input_index,
                },
            };

            if let Err(_) = dst.send(frame_box) {
                if !ofp.eof {
                    ofp.eof = true;
                    fgp.nb_outputs_done += 1;
                }
                return 0;
            }
        }

        if ofp.media_type == AVMEDIA_TYPE_VIDEO {
            ofp.fpsconv_context.frame_number += 1;
            ofp.next_pts += 1;

            if i == nb_frames_prev {
                if !frame_is_null(&frame) {
                    (*frame.as_mut_ptr()).flags &= !AV_FRAME_FLAG_KEY;
                }
            }
        }

        fgp.got_frame = true;
    }

    let frame_is_null = frame_is_null(&frame);
    if !frame_is_null && ofp.media_type == AVMEDIA_TYPE_VIDEO {
        let frame_prev = std::mem::replace(&mut ofp.fpsconv_context.last_frame, frame);
        frame_pool.release(frame_prev);
    }

    if frame_is_null {
        return close_output(ofp, fgp, frame_pool);
    }

    0
}

#[cfg(feature = "docs-rs")]
unsafe fn close_output(
    ofp: &mut OutputFilterParameter,
    fgp: &mut FilterGraphParameter,
    frame_pool: &ObjPool<Frame>,
) -> i32 {
    0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn close_output(
    ofp: &mut OutputFilterParameter,
    fgp: &mut FilterGraphParameter,
    frame_pool: &ObjPool<Frame>,
) -> i32 {
    // we are finished and no frames were ever seen at this output,
    // at least initialize the encoder with a dummy frame

    if !fgp.got_frame {
        let Ok(mut frame) = frame_pool.get() else {
            return AVERROR(ffmpeg_sys_next::ENOMEM);
        };

        (*frame.as_mut_ptr()).time_base = ofp.tb_out;
        (*frame.as_mut_ptr()).format = std::mem::transmute(ofp.format);

        (*frame.as_mut_ptr()).width = ofp.width;
        (*frame.as_mut_ptr()).height = ofp.height;
        (*frame.as_mut_ptr()).sample_aspect_ratio = ofp.sample_aspect_ratio;

        (*frame.as_mut_ptr()).sample_rate = ofp.sample_rate;

        if ofp.opts.ch_layout.nb_channels != 0 {
            let ret = av_channel_layout_copy(
                &mut (*frame.as_mut_ptr()).ch_layout,
                &mut (*frame.as_mut_ptr()).ch_layout,
            );
            if ret < 0 {
                return ret;
            }
        }

        warn!("No filtered frames for output stream, trying to initialize anyway.");

        if let Some(dst) = ofp.dst.clone() {
            let framerate = if ofp.opts.framerate.den == 0 {
                None
            } else {
                Some(ofp.opts.framerate)
            };

            let frame_box = FrameBox {
                frame,
                frame_data: FrameData {
                    framerate,
                    bits_per_raw_sample: 0,
                    input_stream_width: 0,
                    input_stream_height: 0,
                    subtitle_header_size: 0,
                    subtitle_header: null_mut(),
                    fg_input_index: ofp.fg_input_index,
                },
            };

            if let Err(_) = dst.send(frame_box) {
                return AVERROR(ffmpeg_sys_next::EOF);
            }
        }
    }

    ofp.eof = true;

    if let Some(dst) = ofp.dst.clone() {
        let frame_box = FrameBox {
            frame: null_frame(),
            frame_data: FrameData {
                framerate: None,
                bits_per_raw_sample: 0,
                input_stream_width: 0,
                input_stream_height: 0,
                subtitle_header_size: 0,
                subtitle_header: null_mut(),
                fg_input_index: ofp.fg_input_index,
            },
        };

        if let Err(_) = dst.send(frame_box) {
            return AVERROR(ffmpeg_sys_next::EOF);
        }
    }

    0
}

#[cfg(feature = "docs-rs")]
unsafe fn video_sync_process(
    ofp: &mut OutputFilterParameter,
    frame: *mut AVFrame,
    nb_frames: &mut i64,
    nb_frames_prev: &mut i64,
) {
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn video_sync_process(
    ofp: &mut OutputFilterParameter,
    frame: *mut AVFrame,
    nb_frames: &mut i64,
    nb_frames_prev: &mut i64,
) {
    let Some(vsync_method) = ofp.opts.vsync_method else {
        error!("No vsync method on video sync!!!");
        return;
    };

    let fps = &mut ofp.fpsconv_context;

    if frame.is_null() {
        *nb_frames_prev = mid_pred(
            fps.frames_prev_hist[0],
            fps.frames_prev_hist[1],
            fps.frames_prev_hist[2],
        );
        *nb_frames = *nb_frames_prev;

        if *nb_frames == 0 && fps.last_dropped > 0 {
            fps.last_dropped += 1;
        }

        // Handle finish cleanup here
        fps.frames_prev_hist.copy_within(0..2, 1);
        fps.frames_prev_hist[0] = *nb_frames_prev;

        if *nb_frames_prev == 0 && fps.last_dropped > 0 {
            info!(
                "Dropping frame {} at ts {}",
                fps.frame_number,
                (*fps.last_frame.as_ptr()).pts
            );
        }

        fps.last_dropped = (nb_frames == nb_frames_prev && !frame.is_null()) as i32;

        if fps.last_dropped != 0 && (*frame).flags & AV_FRAME_FLAG_KEY != 0 {
            fps.dropped_keyframe = true;
        }

        return;
    }

    let mut duration = (*frame).duration as f64 * av_q2d((*frame).time_base) / av_q2d(ofp.tb_out);
    let mut sync_ipts =
        adjust_frame_pts_to_encoder_tb(frame, ofp.tb_out, ofp.opts.ts_offset.unwrap_or(0));
    /* delta0 is the "drift" between the input frame and
     * where it would fall in the output. */
    let mut delta0 = sync_ipts - ofp.next_pts as f64;
    let delta = delta0 + duration;

    // tracks the number of times the PREVIOUS frame should be duplicated,
    // mostly for variable framerate (VFR)
    *nb_frames_prev = 0;
    /* by default, we output a single frame */
    *nb_frames = 1;

    if delta0 < 0.0 && delta > 0.0 && vsync_method != VSyncMethod::VsyncPassthrough {
        if delta0 < -0.6 {
            debug!("Past duration {:.2} too large", -delta0);
        } else {
            debug!("Clipping frame in rate conversion by {:.6}", -delta0);
        }
        sync_ipts = ofp.next_pts as f64;
        duration += delta0;
        delta0 = 0.0;
    }

    match vsync_method {
        VSyncMethod::VsyncVscfr => {
            if fps.frame_number == 0 && delta0 >= 0.5 {
                log::debug!("Not duplicating {} initial frames", delta0 as i32);
                // delta = duration;
                // delta0 = 0.0;
                ofp.next_pts = sync_ipts.round() as i64;
            }
        }
        VSyncMethod::VsyncCfr => {
            if delta < -1.1 {
                *nb_frames = 0;
            } else if delta > 1.1 {
                *nb_frames = delta.round() as i64;
                if delta0 > 1.1 {
                    *nb_frames_prev = (delta0 - 0.6).round() as i64;
                }
            }
            (*frame).duration = 1;
        }
        VSyncMethod::VsyncVfr => {
            if delta <= -0.6 {
                *nb_frames = 0;
            } else if delta > 0.6 {
                ofp.next_pts = sync_ipts.round() as i64;
            }
            (*frame).duration = duration.round() as i64;
        }
        VSyncMethod::VsyncPassthrough => {
            ofp.next_pts = sync_ipts.round() as i64;
            (*frame).duration = duration.round() as i64;
        }
        _ => {}
    }

    // Handle finish cleanup here
    fps.frames_prev_hist.copy_within(0..2, 1);
    fps.frames_prev_hist[0] = *nb_frames_prev;

    if *nb_frames_prev == 0 && fps.last_dropped > 0 {
        info!(
            "Dropping frame {} at ts {}",
            fps.frame_number,
            (*fps.last_frame.as_ptr()).pts
        );
    }

    fps.last_dropped = (nb_frames == nb_frames_prev) as i32;

    if fps.last_dropped != 0 && (*frame).flags & AV_FRAME_FLAG_KEY != 0 {
        fps.dropped_keyframe = true;
    }
}

#[cfg(feature = "docs-rs")]
unsafe fn adjust_frame_pts_to_encoder_tb(
    frame: *mut AVFrame,
    tb_dst: AVRational,
    start_time: i64,
) -> f64 {
    0.0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn adjust_frame_pts_to_encoder_tb(
    frame: *mut AVFrame,
    tb_dst: AVRational,
    start_time: i64,
) -> f64 {
    let mut float_pts = AV_NOPTS_VALUE as f64;
    let mut tb = tb_dst;
    let filter_tb = (*frame).time_base;
    let extra_bits = av_clip(29 - av_log2(tb.den as u32), 0, 16);

    if (*frame).pts == AV_NOPTS_VALUE {
        return float_pts;
    }

    tb.den <<= extra_bits;
    float_pts = av_rescale_q((*frame).pts, filter_tb, tb) as f64
        - av_rescale_q(start_time, AV_TIME_BASE_Q, tb) as f64;
    float_pts /= (1 << extra_bits) as f64;

    if float_pts != float_pts.round() {
        float_pts += float_pts.signum() * 1.0 / (1 << 17) as f64;
    }

    (*frame).pts = av_rescale_q((*frame).pts, filter_tb, tb_dst)
        - av_rescale_q(start_time, AV_TIME_BASE_Q, tb_dst);
    (*frame).time_base = tb_dst;

    float_pts
}

fn av_clip(x: i32, min_val: i32, max_val: i32) -> i32 {
    std::cmp::max(min_val, std::cmp::min(x, max_val))
}

fn mid_pred(a: i64, b: i64, c: i64) -> i64 {
    if a > b {
        if b > c {
            b // a > b > c
        } else if a > c {
            c // a > c > b
        } else {
            a // c > a > b
        }
    } else {
        if a > c {
            a // b > a > c
        } else if b > c {
            c // b > c > a
        } else {
            b // c > b > a
        }
    }
}

fn unknown_if_null(ptr: *const c_char) -> &'static str {
    if ptr.is_null() {
        "unknown"
    } else {
        unsafe { CStr::from_ptr(ptr).to_str().unwrap_or("invalid") }
    }
}

struct FPSConvContext {
    last_frame: Frame,
    frame_number: usize,

    /* history of nb_frames_prev, i.e. the number of times the
     * previous frame was duplicated by vsync code in recent
     * do_video_out() calls */
    frames_prev_hist: [i64; 3],

    #[allow(dead_code)]
    dup_warning: u64,

    last_dropped: i32,
    dropped_keyframe: bool,

    framerate: AVRational,
    //TODO
    framerate_max: AVRational,
}

impl Default for FPSConvContext {
    fn default() -> Self {
        Self {
            last_frame: unsafe { Frame::empty() },
            frame_number: 0,
            frames_prev_hist: [0; 3],
            dup_warning: 0,
            last_dropped: 0,
            dropped_keyframe: false,
            framerate: AVRational { num: 0, den: 0 },
            framerate_max: AVRational { num: 0, den: 0 },
        }
    }
}

#[cfg(feature = "docs-rs")]
fn ifilter_parameters_from_frame(
    ifp: &mut InputFilterParameter,
    frame: *const AVFrame,
    media_type: AVMediaType,
) -> crate::error::Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
fn ifilter_parameters_from_frame(
    ifp: &mut InputFilterParameter,
    frame: *const AVFrame,
    media_type: AVMediaType,
) -> crate::error::Result<()> {
    unsafe {
        // Replace hw_frames_ctx
        let ret = av_buffer_replace(&mut ifp.hw_frames_ctx, (*frame).hw_frames_ctx);
        if ret < 0 {
            error!("Replace hw_frames_ctx error: {}", av_err2str(ret));
            return Err(Error::FilterGraph(
                FilterGraphOperationError::BufferReplaceoseError(FilterGraphError::from(ret)),
            ));
        }

        // Determine time_base
        ifp.time_base = if media_type == AVMEDIA_TYPE_AUDIO {
            AVRational {
                num: 1,
                den: (*frame).sample_rate,
            }
        } else {
            (*frame).time_base
        };

        // Set format
        ifp.format = (*frame).format;

        // Set video-specific parameters
        ifp.width = (*frame).width;
        ifp.height = (*frame).height;
        ifp.sample_aspect_ratio = (*frame).sample_aspect_ratio;
        ifp.color_space = (*frame).colorspace;
        ifp.color_range = (*frame).color_range;

        // Set audio-specific parameters
        ifp.sample_rate = (*frame).sample_rate;
        let ret = av_channel_layout_copy(&mut ifp.ch_layout, &(*frame).ch_layout);
        if ret < 0 {
            error!("layout_copy error: {}", av_err2str(ret));
            return Err(Error::FilterGraph(
                FilterGraphOperationError::ChannelLayoutCopyError(FilterGraphError::from(ret)),
            ));
        }

        // Handle display matrix side data
        let sd = av_frame_get_side_data(frame, AV_FRAME_DATA_DISPLAYMATRIX);
        if !sd.is_null() {
            std::ptr::copy_nonoverlapping(
                (*sd).data as *const i32,
                ifp.displaymatrix.as_mut_ptr(),
                9,
            );
            ifp.displaymatrix_present = true;
        } else {
            ifp.displaymatrix_present = false;
        }

        Ok(())
    }
}

fn choose_sample_fmts(
    bprint: &mut AVBPrint,
    format: AVSampleFormat,
    formats: Option<Vec<AVSampleFormat>>,
) {
    if format == AV_SAMPLE_FMT_NONE {
        if formats.is_none() || formats.as_ref().unwrap().is_empty() {
            return;
        }
    }

    unsafe {
        {
            let sample_fmts = CString::new("sample_fmts=").unwrap();
            av_bprintf(bprint, sample_fmts.as_ptr());
        }

        if format != AV_SAMPLE_FMT_NONE {
            let format_name = av_get_sample_fmt_name(format);
            if !format_name.is_null() {
                let fmt_str = CString::new("%s").unwrap();
                av_bprintf(bprint, fmt_str.as_ptr(), format_name);
            }
        } else if let Some(formats) = formats {
            for fmt in formats {
                let format_name = av_get_sample_fmt_name(fmt);
                if !format_name.is_null() {
                    let pipe_str = CString::new("%s|").unwrap();
                    av_bprintf(bprint, pipe_str.as_ptr(), format_name);
                }
            }
            if bprint.len > 0 {
                let last_char_ptr = bprint.str_.add(bprint.len as usize - 1);
                *last_char_ptr = 0; // Remove last '|'
                bprint.len -= 1;
            }
        }

        av_bprint_chars(bprint, b':' as u8, 1);
    }
}

#[cfg(feature = "docs-rs")]
fn choose_sample_rates(bprint: &mut AVBPrint, rate: i32, rates: Option<Vec<i32>>) {}

#[cfg(not(feature = "docs-rs"))]
fn choose_sample_rates(bprint: &mut AVBPrint, rate: i32, rates: Option<Vec<i32>>) {
    if rate == 0 {
        if rates.is_none() || rates.as_ref().unwrap().is_empty() {
            return;
        }
    }

    unsafe {
        {
            let sample_rates_str = CString::new("sample_rates=").unwrap();
            av_bprintf(bprint, sample_rates_str.as_ptr());
        }

        if rate != 0 {
            let fmt_specifier = CString::new("%d").unwrap();
            av_bprintf(bprint, fmt_specifier.as_ptr(), rate);
        } else if let Some(rates) = rates {
            for r in rates {
                let pipe_specifier = CString::new("%d|").unwrap();
                av_bprintf(bprint, pipe_specifier.as_ptr(), r);
            }
            if bprint.len > 0 {
                let last_char_ptr = bprint.str_.add(bprint.len as usize - 1);
                *last_char_ptr = 0; // Remove last '|'
                bprint.len -= 1;
            }
        }

        av_bprint_chars(bprint, b':' as u8, 1);
    }
}

#[cfg(not(feature = "docs-rs"))]
fn choose_channel_layouts(
    bprint: &mut AVBPrint,
    layout: AVChannelLayout,
    layouts: Option<Vec<AVChannelLayout>>,
) {
    unsafe {
        let ret = av_channel_layout_check(&layout as *const AVChannelLayout);
        if ret != 0 {
            let channel_layouts_str = CString::new("channel_layouts=").unwrap();
            av_bprintf(bprint, channel_layouts_str.as_ptr());
            av_channel_layout_describe_bprint(&layout, bprint);
        } else if let Some(layouts) = layouts {
            if layouts.is_empty() {
                return;
            }

            let channel_layouts_str = CString::new("channel_layouts=").unwrap();
            av_bprintf(bprint, channel_layouts_str.as_ptr());

            for l in layouts {
                if l.nb_channels == 0 {
                    break;
                }

                av_channel_layout_describe_bprint(&l, bprint);
                if !bprint.str_.is_null() {
                    let pipe_str = CString::new("|").unwrap();
                    av_bprintf(bprint, pipe_str.as_ptr());
                }
            }
            if bprint.len > 0 {
                let last_char_ptr = bprint.str_.add(bprint.len as usize - 1);
                *last_char_ptr = 0; // Remove last '|'
                bprint.len -= 1;
            }
        } else {
            return;
        }

        av_bprint_chars(bprint, b':' as u8, 1);
    }
}

unsafe fn configure_output_video_filter(
    graph: *mut AVFilterGraph,
    ofp: &mut OutputFilterParameter,
    output: *mut AVFilterInOut,
) -> i32 {
    let mut pad_idx = (*output).pad_idx;

    let mut last_filter = (*output).filter_ctx;

    let result = CString::new(format!("out_{}", ofp.opts.name));
    if let Err(_) = result {
        return AVERROR(ENOMEM);
    }
    let name = result.unwrap();

    let buffer_str = std::ffi::CString::new("buffersink").unwrap();
    let buffer_filter = avfilter_get_by_name(buffer_str.as_ptr());

    let mut ret = avfilter_graph_create_filter(
        &mut ofp.filter,
        buffer_filter,
        name.as_ptr(),
        null(),
        null_mut(),
        graph,
    );
    if ret < 0 {
        return ret;
    }

    let mut bprint = AVBPrint {
        str_: null_mut(),
        len: 0,
        size: 0,
        size_max: 0,
        reserved_internal_buffer: [0; 1],
        reserved_padding: [0; 1000],
    };
    av_bprint_init(&mut bprint, 0, u32::MAX);

    //TODO To support specifying the following parameters
    choose_pix_fmts(&mut bprint, AV_PIX_FMT_NONE, ofp.opts.formats.clone());
    choose_color_spaces(
        &mut bprint,
        AVCOL_SPC_UNSPECIFIED,
        ofp.opts.color_spaces.clone().unwrap_or(Vec::new()),
    );
    choose_color_ranges(
        &mut bprint,
        AVCOL_RANGE_UNSPECIFIED,
        ofp.opts.color_ranges.clone().unwrap_or(Vec::new()),
    );

    if bprint.len >= bprint.size {
        av_bprint_finalize(&mut bprint, null_mut());
        return AVERROR(ENOMEM);
    }

    if bprint.len > 0 {
        let mut filter = null_mut();

        let format_str = CString::new("format").unwrap();
        let format_filter = avfilter_get_by_name(format_str.as_ptr());

        let mut ret = avfilter_graph_create_filter(
            &mut filter,
            format_filter,
            format_str.as_ptr(),
            bprint.str_,
            null_mut(),
            graph,
        );
        if ret < 0 {
            av_bprint_finalize(&mut bprint, null_mut());
            return ret;
        }

        ret = avfilter_link(last_filter, pad_idx as u32, filter, 0);
        if ret < 0 {
            av_bprint_finalize(&mut bprint, null_mut());
            return ret;
        }

        last_filter = filter;
        pad_idx = 0;
    }

    let name = format!("trim_out_{}", ofp.name);
    ret = insert_trim(
        ofp.opts.trim_start_us,
        ofp.opts.trim_duration_us,
        &mut last_filter,
        &mut pad_idx,
        &name,
    );
    if ret < 0 {
        return ret;
    }

    ret = avfilter_link(last_filter, pad_idx as u32, ofp.filter, 0);
    if ret < 0 {
        av_bprint_finalize(&mut bprint, null_mut());
        return ret;
    }
    0
}

fn choose_pix_fmts(
    bprint: &mut AVBPrint,
    format: AVPixelFormat,
    formats: Option<Vec<AVPixelFormat>>,
) {
    if format == AV_PIX_FMT_NONE && (formats.is_none() || formats.as_ref().unwrap().is_empty()) {
        return;
    }

    unsafe {
        {
            let pix_fmts_str = CString::new("pix_fmts=").unwrap();
            av_bprintf(bprint, pix_fmts_str.as_ptr());
        }

        if format != AV_PIX_FMT_NONE {
            let format_name = av_get_pix_fmt_name(format);
            if !format_name.is_null() {
                let fmt_specifier = CString::new("%s").unwrap();
                av_bprintf(bprint, fmt_specifier.as_ptr(), format_name);
            }
        } else if let Some(formats) = formats {
            for fmt in formats {
                let format_name = av_get_pix_fmt_name(fmt);
                if !format_name.is_null() {
                    let fmt_pipe = CString::new("%s|").unwrap();
                    av_bprintf(bprint, fmt_pipe.as_ptr(), format_name);
                }
            }
            if bprint.len > 0 {
                let last_char_ptr = bprint.str_.add(bprint.len as usize - 1);
                *last_char_ptr = 0; // Remove last '|'
                bprint.len -= 1;
            }
        }

        av_bprint_chars(bprint, b':' as u8, 1);
    }
}

fn choose_color_spaces(
    bprint: &mut AVBPrint,
    color_space: AVColorSpace,
    color_spaces: Vec<AVColorSpace>,
) {
    if color_space == AVCOL_SPC_UNSPECIFIED && color_spaces.is_empty() {
        return;
    }

    unsafe {
        {
            let color_spaces_str = CString::new("color_spaces=").unwrap();
            av_bprintf(bprint, color_spaces_str.as_ptr());
        }

        if color_space != AVCOL_SPC_UNSPECIFIED {
            let color_space_name = av_color_space_name(color_space);
            if !color_space_name.is_null() {
                let fmt_str = CString::new("%s").unwrap();
                av_bprintf(bprint, fmt_str.as_ptr(), color_space_name);
            }
        } else {
            for cs in color_spaces {
                let color_space_name = av_color_space_name(cs);
                if !color_space_name.is_null() {
                    let fmt_pipe = CString::new("%s|").unwrap();
                    av_bprintf(bprint, fmt_pipe.as_ptr(), color_space_name);
                }
            }
            if bprint.len > 0 {
                let last_char_ptr = bprint.str_.add(bprint.len as usize - 1);
                *last_char_ptr = 0; // Remove last '|'
                bprint.len -= 1;
            }
        }

        av_bprint_chars(bprint, b':' as u8, 1);
    }
}

fn choose_color_ranges(
    bprint: &mut AVBPrint,
    color_range: AVColorRange,
    color_ranges: Vec<AVColorRange>,
) {
    if color_range == AVCOL_RANGE_UNSPECIFIED && color_ranges.is_empty() {
        return;
    }

    unsafe {
        {
            let color_ranges_str = CString::new("color_ranges=").unwrap();
            av_bprintf(bprint, color_ranges_str.as_ptr());
        }

        if color_range != AVCOL_RANGE_UNSPECIFIED {
            let color_range_name = av_color_range_name(color_range);
            if !color_range_name.is_null() {
                let fmt_str = CString::new("%s").unwrap();
                av_bprintf(bprint, fmt_str.as_ptr(), color_range_name);
            }
        } else {
            for cr in color_ranges {
                let color_range_name = av_color_range_name(cr);
                if !color_range_name.is_null() {
                    let fmt_pipe = CString::new("%s|").unwrap();
                    av_bprintf(bprint, fmt_pipe.as_ptr(), color_range_name);
                }
            }
            if bprint.len > 0 {
                let last_char_ptr = bprint.str_.add(bprint.len as usize - 1);
                *last_char_ptr = 0; // Remove last '|'
                bprint.len -= 1;
            }
        }

        av_bprint_chars(bprint, b':' as u8, 1);
    }
}

unsafe fn configure_input_filter(
    fg_index: usize,
    graph: *mut AVFilterGraph,
    ifp: &mut InputFilterParameter,
    input: *mut AVFilterInOut,
) -> i32 {
    match ifp.media_type {
        AVMEDIA_TYPE_VIDEO => configure_input_video_filter(fg_index, graph, ifp, input),
        AVMEDIA_TYPE_AUDIO => configure_input_audio_filter(fg_index, graph, ifp, input),
        _ => {
            error!("Unexpected media type {:?}", ifp.media_type);
            0
        }
    }
}

#[cfg(feature = "docs-rs")]
unsafe fn configure_input_audio_filter(
    fg_index: usize,
    graph: *mut AVFilterGraph,
    ifp: &mut InputFilterParameter,
    input: *mut AVFilterInOut,
) -> i32 {
    0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn configure_input_audio_filter(
    fg_index: usize,
    graph: *mut AVFilterGraph,
    ifp: &mut InputFilterParameter,
    input: *mut AVFilterInOut,
) -> i32 {
    let mut pad_idx = 0;
    let abuffer_str = std::ffi::CString::new("abuffer").unwrap();
    let abuffer_filter = avfilter_get_by_name(abuffer_str.as_ptr());

    let mut args = AVBPrint {
        str_: null_mut(),
        len: 0,
        size: 0,
        size_max: 0,
        reserved_internal_buffer: [0; 1],
        reserved_padding: [0; 1000],
    };

    av_bprint_init(&mut args, 0, AV_BPRINT_SIZE_AUTOMATIC as u32);
    let sample_fmt_name = av_get_sample_fmt_name(std::mem::transmute((*ifp).format));

    {
        let fmt_str = CString::new("time_base=%d/%d:sample_rate=%d:sample_fmt=%s").unwrap();
        av_bprintf(
            &mut args,
            fmt_str.as_ptr(),
            (*ifp).time_base.num,
            (*ifp).time_base.den,
            (*ifp).sample_rate,
            sample_fmt_name,
        );
    }

    if av_channel_layout_check(&(*ifp).ch_layout) != 0
        && (*ifp).ch_layout.order != AV_CHANNEL_ORDER_UNSPEC
    {
        let channel_layout_str = CString::new(":channel_layout=").unwrap();
        av_bprintf(&mut args, channel_layout_str.as_ptr());
        av_channel_layout_describe_bprint(&(*ifp).ch_layout, &mut args);
    } else {
        let channels_fmt = CString::new(":channels=%d").unwrap();
        av_bprintf(
            &mut args,
            channels_fmt.as_ptr(),
            (*ifp).ch_layout.nb_channels,
        );
    }

    let name = format!("graph_{fg_index}_in_{}", ifp.opts.name);
    let name = CString::new(name.as_str()).expect("CString::new failed");

    let mut ret = avfilter_graph_create_filter(
        &mut (*ifp).filter,
        abuffer_filter,
        name.as_ptr(),
        args.str_,
        null_mut(),
        graph,
    );
    if ret < 0 {
        return ret;
    }

    let mut last_filter = ifp.filter;

    let name = format!("trim_in_{}", ifp.name);
    ret = insert_trim(
        ifp.opts.trim_start_us,
        ifp.opts.trim_end_us,
        &mut last_filter,
        &mut pad_idx,
        &name,
    );
    if ret < 0 {
        return ret;
    }

    ret = avfilter_link(last_filter, 0, (*input).filter_ctx, (*input).pad_idx as u32);
    if ret < 0 {
        return ret;
    }
    0
}

unsafe fn configure_input_video_filter(
    fg_index: usize,
    graph: *mut AVFilterGraph,
    ifp: &mut InputFilterParameter,
    input: *mut AVFilterInOut,
) -> i32 {
    let mut pad_idx = 0;

    let buffer_str = std::ffi::CString::new("buffer").unwrap();
    let buffer_filter = avfilter_get_by_name(buffer_str.as_ptr());

    let mut par = av_buffersrc_parameters_alloc();
    if par.is_null() {
        return AVERROR(ENOMEM);
    }

    //TODO
    /*if (ifp.type_src == AVMEDIA_TYPE_SUBTITLE){
    sub2video_prepare(ifp);
    }*/

    let mut sar = ifp.sample_aspect_ratio;
    if sar.den == 0 {
        sar = AVRational { num: 0, den: 1 };
    }

    let mut args = format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}:colorspace={}:range={}",
        ifp.width,
        ifp.height,
        ifp.format,
        ifp.time_base.num,
        ifp.time_base.den,
        sar.num,
        sar.den,
        ifp.color_space as i32,
        ifp.color_range as i32,
    );
    if ifp.opts.framerate.num != 0 && ifp.opts.framerate.den != 0 {
        args.push_str(&format!(
            ":frame_rate={}/{}",
            ifp.opts.framerate.num, ifp.opts.framerate.den
        ));
    }

    let result = CString::new(format!(
        "graph {fg_index} input from stream {}",
        ifp.opts.name
    ));
    if let Err(_) = result {
        av_freep(&mut par as *mut _ as *mut c_void);
        return AVERROR(ENOMEM);
    }
    let name = result.unwrap();

    let args_cstr = CString::new(args).unwrap();
    let mut ret = avfilter_graph_create_filter(
        &mut ifp.filter,
        buffer_filter,
        name.as_ptr(),
        args_cstr.as_ptr(),
        null_mut(),
        graph,
    );
    if ret < 0 {
        av_freep(&mut par as *mut _ as *mut c_void);
        return ret;
    }

    (*par).hw_frames_ctx = ifp.hw_frames_ctx;
    ret = av_buffersrc_parameters_set(ifp.filter, par);
    av_freep(&mut par as *mut _ as *mut c_void);
    if ret < 0 {
        return ret;
    }

    let mut last_filter = ifp.filter;
    let desc = av_pix_fmt_desc_get(std::mem::transmute(ifp.format));

    /*if (ifp.opts.flags & IFILTER_FLAG_CROP) {
        char crop_buf[64];
        snprintf(crop_buf, sizeof(crop_buf), "w=iw-%u-%u:h=ih-%u-%u:x=%u:y=%u",
        ifp.opts.crop_left, ifp.opts.crop_right,
        ifp.opts.crop_top, ifp.opts.crop_bottom,
        ifp.opts.crop_left, ifp.opts.crop_top);
        ret = insert_filter(&last_filter, &pad_idx, "crop", crop_buf);
        if ret < 0
        return ret;
    }*/

    ifp.displaymatrix_applied = false;
    if ifp.opts.flags & IFILTER_FLAG_AUTOROTATE != 0
        && (*desc).flags & AV_PIX_FMT_FLAG_HWACCEL as u64 == 0
    {
        let displaymatrix = ifp.displaymatrix;
        let theta = get_rotation(&displaymatrix);

        if (theta - 90.0).abs() < 1.0 {
            let args = if displaymatrix[3] > 0 {
                "cclock_flip"
            } else {
                "clock"
            };
            ret = insert_filter(&mut last_filter, &mut pad_idx, "transpose", Some(args));
        } else if (theta - 180.0).abs() < 1.0 {
            if displaymatrix[0] < 0 {
                ret = insert_filter(&mut last_filter, &mut pad_idx, "hflip", None);
                if ret < 0 {
                    return ret;
                }
            }
            if displaymatrix[4] < 0 {
                ret = insert_filter(&mut last_filter, &mut pad_idx, "vflip", None);
            }
        } else if (theta - 270.0).abs() < 1.0 {
            let args = if displaymatrix[3] < 0 {
                "clock_flip"
            } else {
                "cclock"
            };
            ret = insert_filter(&mut last_filter, &mut pad_idx, "transpose", Some(args));
        } else if theta.abs() > 1.0 {
            let rotate_buf = format!("{:.6}*PI/180", theta);
            ret = insert_filter(&mut last_filter, &mut pad_idx, "rotate", Some(&rotate_buf));
        } else if theta.abs() < 1.0 {
            if displaymatrix[4] < 0 {
                ret = insert_filter(&mut last_filter, &mut pad_idx, "vflip", None);
            }
        }

        if ret < 0 {
            return ret;
        }

        ifp.displaymatrix_applied = true;
    }

    let name = format!("trim_in_{}", ifp.name);
    ret = insert_trim(
        ifp.opts.trim_start_us,
        ifp.opts.trim_end_us,
        &mut last_filter,
        &mut pad_idx,
        &name,
    );
    if ret < 0 {
        return ret;
    }

    ret = avfilter_link(last_filter, 0, (*input).filter_ctx, (*input).pad_idx as u32);
    if ret < 0 {
        return ret;
    }
    0
}

fn insert_filter(
    last_filter: &mut *mut AVFilterContext,
    pad_idx: &mut i32,
    filter_name: &str,
    args: Option<&str>,
) -> i32 {
    let graph = unsafe { (*(*last_filter)).graph };
    let filter_name_cstr = CString::new(filter_name).unwrap();
    let filter = unsafe { avfilter_get_by_name(filter_name_cstr.as_ptr()) };
    if filter.is_null() {
        return AVERROR_BUG;
    }

    let mut ctx = std::ptr::null_mut();
    let args_cstr = args.map(|a| CString::new(a).unwrap());
    let args_ptr = args_cstr
        .as_ref()
        .map_or(std::ptr::null(), |cstr| cstr.as_ptr());

    let ret = unsafe {
        let filter_name_cstr = CString::new(filter_name).unwrap();
        avfilter_graph_create_filter(
            &mut ctx,
            filter,
            filter_name_cstr.as_ptr(),
            args_ptr,
            std::ptr::null_mut(),
            graph,
        )
    };
    if ret < 0 {
        return ret;
    }

    let ret = unsafe { avfilter_link(*last_filter, *pad_idx as u32, ctx, 0) };
    if ret < 0 {
        return ret;
    }

    *last_filter = ctx;
    *pad_idx = 0;
    0
}

fn get_rotation(displaymatrix: &[i32; 9]) -> f64 {
    let mut theta = -round(display_rotation_get(displaymatrix));

    theta -= 360.0 * (theta / 360.0 + 0.9 / 360.0).floor();

    if (theta - 90.0 * (theta / 90.0).round()).abs() > 2.0 {
        warn!(
            "Odd rotation angle.\n\
            If you want to help, upload a sample \
            of this file to https://streams.videolan.org/upload/ \
            and contact the ffmpeg-devel mailing list. (ffmpeg-devel@ffmpeg.org)"
        );
    }

    theta
}

fn display_rotation_get(matrix: &[i32; 9]) -> f64 {
    let mut scale = [0.0; 2];

    scale[0] = hypot(conv_fp(matrix[0]), conv_fp(matrix[3]));
    scale[1] = hypot(conv_fp(matrix[1]), conv_fp(matrix[4]));

    if scale[0] == 0.0 || scale[1] == 0.0 {
        return f64::NAN;
    }

    let rotation =
        (conv_fp(matrix[1]) / scale[1]).atan2(conv_fp(matrix[0]) / scale[0]) * 180.0 / PI;

    -rotation
}

fn hypot(x: f64, y: f64) -> f64 {
    (x.powi(2) + y.powi(2)).sqrt()
}

fn conv_fp(value: i32) -> f64 {
    value as f64 / 1_073_741_824.0 // CONV_FP converts fixed-point values
}

fn round(value: f64) -> f64 {
    value.round()
}

#[cfg(feature = "docs-rs")]
unsafe fn graph_parse(
    graph: *mut AVFilterGraph,
    graph_desc: &str,
    inputs: *mut *mut AVFilterInOut,
    outputs: *mut *mut AVFilterInOut,
    hw_device: Option<HWDevice>,
) -> i32 {
    0
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn graph_parse(
    graph: *mut AVFilterGraph,
    graph_desc: &str,
    inputs: *mut *mut AVFilterInOut,
    outputs: *mut *mut AVFilterInOut,
    hw_device: Option<HWDevice>,
) -> i32 {
    let desc = CString::new(graph_desc).expect("CString::new failed");
    let mut seg = null_mut();
    *inputs = null_mut();
    *outputs = null_mut();
    let mut ret = avfilter_graph_segment_parse(graph, desc.as_ptr(), 0, &mut seg);
    if ret < 0 {
        return ret;
    }

    ret = avfilter_graph_segment_create_filters(seg, 0);
    if ret < 0 {
        avfilter_graph_segment_free(&mut seg);
        return ret;
    }

    if let Some(hw_device) = hw_device {
        for i in 0..(*graph).nb_filters {
            let f = *(*graph).filters.add(i as usize);

            if (*(*f).filter).flags & AVFILTER_FLAG_HWDEVICE == 0 {
                continue;
            }
            (*f).hw_device_ctx = av_buffer_ref(hw_device.device_ref);
            if (*f).hw_device_ctx.is_null() {
                avfilter_graph_segment_free(&mut seg);
                return AVERROR(ENOMEM);
            }
        }
    }

    ret = graph_opts_apply(seg);
    if ret < 0 {
        avfilter_graph_segment_free(&mut seg);
        return ret;
    }

    ret = avfilter_graph_segment_apply(seg, 0, inputs, outputs);
    avfilter_graph_segment_free(&mut seg);

    ret
}

#[cfg(not(feature = "docs-rs"))]
pub(crate) unsafe fn graph_opts_apply(seg: *mut AVFilterGraphSegment) -> i32 {
    for i in 0..(*seg).nb_chains {
        let ch = *(*seg).chains.add(i);

        for j in 0..(*ch).nb_filters {
            let p = *(*ch).filters.add(j);
            let mut e = null();

            assert!(!(*p).filter.is_null());

            loop {
                e = av_dict_iterate((*p).opts, e);
                if e.is_null() {
                    break;
                }

                let ret = filter_opt_apply((*p).filter, (*e).key, (*e).value);
                if ret < 0 {
                    return ret;
                }
            }

            av_dict_free(&mut (*p).opts);
        }
    }

    0
}

unsafe fn filter_opt_apply(f: *mut AVFilterContext, mut key: *mut c_char, val: *mut c_char) -> i32 {
    let mut ret = av_opt_set(f as *mut libc::c_void, key, val, AV_OPT_SEARCH_CHILDREN);
    if ret >= 0 {
        return 0;
    }

    let o = if ret == AVERROR_OPTION_NOT_FOUND && *key == b'/' as c_char {
        av_opt_find(
            f as *mut libc::c_void,
            key.add(1),
            null(),
            0,
            AV_OPT_SEARCH_CHILDREN,
        )
    } else {
        null()
    };
    if o.is_null() {
        error!(
            "Error applying option '{}' to filter '{}': {}",
            CStr::from_ptr(key).to_str().unwrap_or("[unknow key]"),
            CStr::from_ptr((*(*f).filter).name)
                .to_str()
                .unwrap_or("[unknow filter name]"),
            av_err2str(ret)
        );
        return ret;
    }

    key = key.add(1);

    if (*o).type_ == AV_OPT_TYPE_BINARY {
        let result = read_binary(val);
        if let Err(e) = result {
            error!(
                "Error loading value for option '{}' from file {}",
                CStr::from_ptr(key).to_str().unwrap_or("[unknow key]"),
                CStr::from_ptr(val).to_str().unwrap_or("[unknow val]")
            );
            return e;
        }
        let (data, len) = result.unwrap();

        ret = av_opt_set_bin(
            f as *mut libc::c_void,
            key,
            data as *mut u8,
            len as i32,
            AV_OPT_SEARCH_CHILDREN,
        );
        av_freep(data);
    } else {
        let data = file_read(val);
        if data.is_null() {
            error!(
                "Error loading value for option '{}' from file {}",
                CStr::from_ptr(key).to_str().unwrap_or("[unknow key]"),
                CStr::from_ptr(val).to_str().unwrap_or("[unknow val]")
            );
            return AVERROR(EIO);
        }

        ret = av_opt_set(f as *mut libc::c_void, key, data, AV_OPT_SEARCH_CHILDREN);
        av_freep(data as *mut libc::c_void);
    }
    if ret < 0 {
        error!(
            "Error applying option '{}' to filter '{}': {}",
            CStr::from_ptr(key).to_str().unwrap_or("[unknow key]"),
            CStr::from_ptr((*(*f).filter).name)
                .to_str()
                .unwrap_or("[unknow filter name]"),
            av_err2str(ret)
        );
        return ret;
    }

    0
}

unsafe fn file_read(filename: *mut c_char) -> *mut c_char {
    let mut pb = null_mut();
    let mut ret = avio_open(&mut pb, filename, AVIO_FLAG_READ);
    let bprint = null_mut();
    let mut str = null_mut();

    if ret < 0 {
        error!(
            "Error opening file {}.",
            CStr::from_ptr(filename)
                .to_str()
                .unwrap_or("[unknow filename]")
        );
        return null_mut();
    }

    av_bprint_init(bprint, 0, u32::MAX);
    ret = avio_read_to_bprint(pb, bprint, usize::MAX);
    avio_closep(&mut pb);
    if ret < 0 {
        av_bprint_finalize(bprint, null_mut());
        return null_mut();
    }
    ret = av_bprint_finalize(bprint, &mut str);
    if ret < 0 {
        return null_mut();
    }
    str
}

unsafe fn read_binary(path: *mut c_char) -> crate::error::Result<(*mut c_void, i64), i32> {
    let mut io = null_mut();

    let ret = avio_open2(&mut io, path, AVIO_FLAG_READ, null(), null_mut());
    if ret < 0 {
        error!(
            "Cannot open file '{}': {}",
            CStr::from_ptr(path).to_str().unwrap_or("[unknow path]"),
            av_err2str(ret)
        );
        return Err(ret);
    }

    let fsize = avio_size(io);
    if fsize < 0 || fsize > i64::MAX {
        error!(
            "Cannot obtain size of file '{}'",
            CStr::from_ptr(path).to_str().unwrap_or("[unknow path]")
        );
        avio_close(io);
        return Err(AVERROR(EIO));
    }

    let data = av_malloc(fsize as usize);
    if data.is_null() {
        avio_close(io);
        return Err(AVERROR(ENOMEM));
    }

    let read_size = avio_read(io, data as *mut libc::c_uchar, fsize as i32);
    if read_size != fsize as i32 {
        error!(
            "Error reading file '{}'. read_size:{read_size}",
            CStr::from_ptr(path).to_str().unwrap_or("[unknow path]")
        );
        av_freep(data);
        avio_close(io);
        return Err(if read_size < 0 {
            read_size
        } else {
            AVERROR(EIO)
        });
    }

    avio_close(io);

    Ok((data, fsize))
}
