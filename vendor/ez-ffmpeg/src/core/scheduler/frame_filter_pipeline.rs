use crate::core::context::decoder_stream::DecoderStream;
use crate::core::context::encoder_stream::EncoderStream;
use crate::core::context::obj_pool::ObjPool;
use crate::core::context::{FrameBox, FrameData};
use crate::core::scheduler::type_to_symbol;
use crate::error::Error::{
    FrameFilterInit, FrameFilterProcess, FrameFilterRequest, FrameFilterSendOOM,
    FrameFilterStreamTypeNoMatched, FrameFilterThreadExited, FrameFilterTypeNoMatched,
};
use crate::filter::frame_pipeline::FramePipeline;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ffmpeg_next::Frame;
use ffmpeg_sys_next::{av_frame_copy_props, av_frame_ref};
use log::{debug, error, info, warn};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

pub(crate) fn input_pipeline_init(
    demux_idx: usize,
    pipeline: FramePipeline,
    decoder_streams: &mut Vec<DecoderStream>,
    frame_pool: ObjPool<Frame>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    if pipeline.filters.is_empty() {
        warn!("pipeline filters is empty");
        return Ok(());
    }

    // Match type to find index and linklabel.
    let (stream_index, encoder_frame_receiver, pipeline_frame_senders) =
        match_decoder_stream(&pipeline, decoder_streams)?;

    pipeline_init(
        true,
        demux_idx,
        pipeline,
        stream_index,
        encoder_frame_receiver,
        pipeline_frame_senders,
        frame_pool,
        scheduler_status,
        scheduler_result,
    )
}
pub(crate) fn output_pipeline_init(
    mux_idx: usize,
    pipeline: FramePipeline,
    encoder_streams: &mut Vec<EncoderStream>,
    frame_pool: ObjPool<Frame>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    if pipeline.filters.is_empty() {
        warn!("pipeline filters is empty");
        return Ok(());
    }

    // Match type to find index and linklabel.
    let (stream_index, encoder_frame_receiver, pipeline_frame_sender) =
        match_encoder_stream(&pipeline, encoder_streams)?;

    pipeline_init(
        false,
        mux_idx,
        pipeline,
        stream_index,
        encoder_frame_receiver,
        vec![(pipeline_frame_sender, usize::MAX, Arc::new([]))],
        frame_pool,
        scheduler_status,
        scheduler_result,
    )
}

fn match_decoder_stream(
    pipeline: &FramePipeline,
    decoder_streams: &mut Vec<DecoderStream>,
) -> crate::error::Result<(usize, Receiver<FrameBox>, Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>)> {
    let (stream_index, pipeline_frame_receiver, decoder_frame_senders) =
        match pipeline.stream_index {
            Some(stream_index) => {
                match decoder_streams
                    .iter_mut()
                    .find(|decoder_stream| decoder_stream.stream_index == stream_index)
                {
                    None => {
                        return Err(FrameFilterStreamTypeNoMatched(
                            "Input".to_string(),
                            stream_index,
                            format!("{:?}", pipeline.media_type),
                        ))
                    }
                    Some(decoder_stream) => {
                        let (pipeline_frame_sender, pipeline_frame_receiver) =
                            crossbeam_channel::bounded(8);
                        let decoder_frame_senders =
                            decoder_stream.replace_dsts(pipeline_frame_sender, usize::MAX, Arc::new([]));

                        (
                            stream_index,
                            pipeline_frame_receiver,
                            decoder_frame_senders,
                        )
                    }
                }
            }
            None => match decoder_streams
                .iter_mut()
                .find(|decoder_stream| decoder_stream.codec_type == pipeline.media_type)
            {
                None => {
                    return Err(FrameFilterTypeNoMatched(
                        "input".to_string(),
                        format!("{:?}", pipeline.media_type),
                    ))
                }
                Some(decoder_stream) => {
                    let (pipeline_frame_sender, pipeline_frame_receiver) =
                        crossbeam_channel::bounded(8);
                    let decoder_frame_senders = decoder_stream.replace_dsts(pipeline_frame_sender, usize::MAX, Arc::new([]));
                    (
                        decoder_stream.stream_index,
                        pipeline_frame_receiver,
                        decoder_frame_senders
                    )
                }
            },
        };
    Ok((
        stream_index,
        pipeline_frame_receiver,
        decoder_frame_senders
    ))
}

fn match_encoder_stream(
    pipeline: &FramePipeline,
    encoder_streams: &mut Vec<EncoderStream>,
) -> crate::error::Result<(usize, Receiver<FrameBox>, Sender<FrameBox>)> {
    let (stream_index, encoder_frame_receiver, pipeline_frame_sender) = match pipeline
        .stream_index
    {
        Some(stream_index) => {
            match encoder_streams
                .iter_mut()
                .find(|encoder_stream| encoder_stream.stream_index == stream_index)
            {
                None => {
                    return Err(FrameFilterStreamTypeNoMatched(
                        "Output".to_string(),
                        stream_index,
                        format!("{:?}", pipeline.media_type),
                    ))
                }
                Some(encoder_stream) => {
                    let (pipeline_frame_sender, pipeline_frame_receiver) =
                        crossbeam_channel::bounded(8);
                    let encoder_frame_receiver =
                        encoder_stream.replace_src(pipeline_frame_receiver);

                    (
                        stream_index,
                        encoder_frame_receiver,
                        pipeline_frame_sender,
                    )
                }
            }
        }
        None => match encoder_streams
            .iter_mut()
            .find(|encoder_stream| encoder_stream.codec_type == pipeline.media_type)
        {
            None => {
                return Err(FrameFilterTypeNoMatched(
                    "output".to_string(),
                    format!("{:?}", pipeline.media_type),
                ))
            }
            Some(encoder_stream) => {
                let (pipeline_frame_sender, pipeline_frame_receiver) =
                    crossbeam_channel::bounded(8);
                let encoder_frame_receiver = encoder_stream.replace_src(pipeline_frame_receiver);

                (
                    encoder_stream.stream_index,
                    encoder_frame_receiver,
                    pipeline_frame_sender,
                )
            }
        },
    };
    Ok((
        stream_index,
        encoder_frame_receiver,
        pipeline_frame_sender,
    ))
}

fn pipeline_init(
    is_input: bool,
    demux_mux_idx: usize,
    mut pipeline: FramePipeline,
    stream_index: usize,
    frame_receiver: Receiver<FrameBox>,
    frame_senders: Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>,
    frame_pool: ObjPool<Frame>,
    scheduler_status: Arc<AtomicUsize>,
    scheduler_result: Arc<Mutex<Option<crate::error::Result<()>>>>,
) -> crate::error::Result<()> {
    let pipeline_name = if is_input {
        "input-frame-pipeline".to_string()
    } else {
        "output-frame-pipeline".to_string()
    };
    let result = std::thread::Builder::new()
        .name(format!(
            "{pipeline_name}:{}:{stream_index}:{demux_mux_idx}",
            type_to_symbol(pipeline.media_type),
        ))
        .spawn(move || {
            if let Err(e) = frame_filter_init(&mut pipeline) {
                pipeline_uninit(&mut pipeline);
                crate::core::scheduler::ffmpeg_scheduler::set_scheduler_error(
                    &scheduler_status,
                    &scheduler_result,
                    e,
                );
                return;
            }

            if let Err(e) = run_pipeline(
                &mut pipeline,
                frame_receiver,
                frame_senders,
                &frame_pool,
                &scheduler_status,
            ) {
                crate::core::scheduler::ffmpeg_scheduler::set_scheduler_error(
                    &scheduler_status,
                    &scheduler_result,
                    e,
                );
            }

            pipeline_uninit(&mut pipeline);
        });

    if let Err(e) = result {
        error!("Pipeline thread exited with error: {e}");
        return Err(FrameFilterThreadExited);
    }

    Ok(())
}

fn run_pipeline(
    pipeline: &mut FramePipeline,
    frame_receiver: Receiver<FrameBox>,
    mut frame_senders: Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>,
    frame_pool: &ObjPool<Frame>,
    scheduler_status: &Arc<AtomicUsize>,
) -> crate::error::Result<()> {
    let mut src_finished_flag = false;

    loop {
        if crate::core::scheduler::ffmpeg_scheduler::wait_until_not_paused(&scheduler_status)
            == crate::core::scheduler::ffmpeg_scheduler::STATUS_END
        {
            info!("Receiver end command, finishing.");
            return Ok(());
        }

        if !src_finished_flag {
            let result = frame_receiver.recv_timeout(Duration::from_millis(1));
            match result {
                Err(e) => {
                    if e == RecvTimeoutError::Disconnected {
                        src_finished_flag = true;
                        debug!("Source[decoder/filtergraph] thread exit.");
                        continue;
                    }
                }
                Ok(frame_box) => {
                    // filter frame
                    match pipeline.run_filters(frame_box.frame) {
                        Ok(tmp_frame) => send_frame(
                            pipeline,
                            &mut frame_senders,
                            frame_pool,
                            tmp_frame,
                        )?,
                        Err(e) => {
                            error!(
                                "Pipeline [index:{}] failed, during filter frame. error: {e}",
                                pipeline.stream_index.unwrap_or(usize::MAX),
                            );
                            return Err(FrameFilterProcess(e));
                        }
                    };

                    if frame_senders.len() == 0 {
                        debug!("All frame sender finished, finishing.");
                        return Ok(());
                    }
                }
            }
        } else {
            sleep(Duration::from_millis(1))
        }

        // request frame
        for i in 0..pipeline.filter_len() {
            loop {
                let result = pipeline.request_frame(i);
                if let Err(e) = result {
                    error!(
                        "Pipeline [index:{}] failed, during request frame.",
                        pipeline.stream_index.unwrap_or(usize::MAX)
                    );
                    return Err(FrameFilterRequest(e));
                }

                let tmp_frame = result.unwrap();
                if tmp_frame.is_none() {
                    break;
                }

                match pipeline.run_filters_from(i + 1, tmp_frame.unwrap()) {
                    Ok(tmp_frame) => send_frame(
                        pipeline,
                        &mut frame_senders,
                        frame_pool,
                        tmp_frame,
                    )?,
                    Err(e) => {
                        error!(
                            "Pipeline [index:{}] failed, during filter frame. error: {e}",
                            pipeline.stream_index.unwrap_or(usize::MAX)
                        );
                        return Err(FrameFilterProcess(e));
                    }
                };
            }
        }

        if frame_senders.len() == 0 {
            debug!("All frame sender finished, finishing.");
            return Ok(());
        }
    }
}

fn send_frame(
    pipeline: &mut FramePipeline,
    frame_senders: &mut Vec<(Sender<FrameBox>, usize, Arc<[AtomicBool]>)>,
    frame_pool: &ObjPool<Frame>,
    tmp_frame: Option<Frame>,
) -> crate::error::Result<()> {
    if let Some(frame) = tmp_frame {
        let mut frame_box = FrameBox {
            frame,
            frame_data: FrameData {
                framerate: None,
                bits_per_raw_sample: 0,
                input_stream_width: 0,
                input_stream_height: 0,
                subtitle_header_size: 0,
                subtitle_header: null_mut(),
                fg_input_index: usize::MAX,
            },
        };

        let mut finished_senders = Vec::new();
        for (i, (sender, fg_input_index, finished_flag_list)) in frame_senders.iter().enumerate() {
            if !finished_flag_list.is_empty() && *fg_input_index < finished_flag_list.len() {
                if finished_flag_list[*fg_input_index].load(Ordering::Acquire) {
                    finished_senders.push(i);
                    continue;
                }
            }
            if i < frame_senders.len() - 1 {
                let mut to_send = frame_pool.get()?;

                // frame may sometimes contain props only,
                // e.g. to signal EOF timestamp
                unsafe {
                    if !(*frame_box.frame.as_ptr()).buf[0].is_null() {
                        let ret = av_frame_ref(to_send.as_mut_ptr(), frame_box.frame.as_ptr());
                        if ret < 0 {
                            return Err(FrameFilterSendOOM);
                        }
                    } else {
                        let ret =
                            av_frame_copy_props(to_send.as_mut_ptr(), frame_box.frame.as_ptr());
                        if ret < 0 {
                            return Err(FrameFilterSendOOM);
                        }
                    };
                }
                let mut frame_data = frame_box.frame_data.clone();
                frame_data.fg_input_index = *fg_input_index;
                let frame_box = FrameBox {
                    frame: to_send,
                    frame_data,
                };
                if let Err(_) = sender.send(frame_box) {
                    debug!(
                        "Pipeline [index:{}] send frame failed, destination already finished",
                        pipeline.stream_index.unwrap_or(usize::MAX),
                    );
                    finished_senders.push(i);
                    continue;
                }
            } else {
                frame_box.frame_data.fg_input_index = *fg_input_index;
                if let Err(_) = sender.send(frame_box) {
                    debug!("Pipeline [index:{}] send frame failed, destination already finished",
                        pipeline.stream_index.unwrap_or(usize::MAX)
                    );
                    finished_senders.push(i);
                }
                break;
            }
        }

        for i in finished_senders {
            frame_senders.remove(i);
        }
    }

    Ok(())
}

fn pipeline_uninit(pipeline: &mut FramePipeline) {
    pipeline.uninit_filters()
}

fn frame_filter_init(pipeline: &mut FramePipeline) -> crate::error::Result<()> {
    if let Err(e) = pipeline.init_filters() {
        return Err(FrameFilterInit(e));
    };
    Ok(())
}
