use crate::core::context::demuxer::Demuxer;
use crate::core::context::ffmpeg_context_builder::FfmpegContextBuilder;
use crate::core::context::filter_complex::FilterComplex;
use crate::core::context::filter_graph::FilterGraph;
use crate::core::context::input::Input;
use crate::core::context::input_filter::{InputFilter, IFILTER_FLAG_AUTOROTATE};
use crate::core::context::muxer::Muxer;
use crate::core::context::output::{Output, StreamMap};
use crate::core::metadata::StreamSpecifier;
use crate::core::context::output_filter::{
    OutputFilter, OFILTER_FLAG_AUDIO_24BIT, OFILTER_FLAG_AUTOSCALE, OFILTER_FLAG_DISABLE_CONVERT,
};
use crate::core::context::{frame_alloc, CodecContext};
use crate::core::scheduler::ffmpeg_scheduler;
use crate::core::scheduler::ffmpeg_scheduler::{FfmpegScheduler, Initialization};
#[cfg(not(feature = "docs-rs"))]
use crate::core::scheduler::filter_task::graph_opts_apply;
use crate::core::scheduler::input_controller::SchNode;
use crate::error::Error::{
    FileSameAsInput, FilterDescUtf8, FilterNameUtf8, FilterZeroOutputs,
    FrameFilterStreamTypeNoMatched, FrameFilterTypeNoMatched, ParseInteger,
};
use crate::error::FilterGraphParseError::{
    InvalidFileIndexInFg, InvalidFilterSpecifier, OutputUnconnected,
};
use crate::error::{
    AllocOutputContextError, FilterGraphParseError, FindStreamError, OpenInputError,
    OpenOutputError,
};
use crate::error::{Error, Result};
use crate::filter::frame_pipeline::FramePipeline;
use crate::util::ffmpeg_utils::hashmap_to_avdictionary;
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC;
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::AVCodecConfig::*;
use ffmpeg_sys_next::AVCodecID::{AV_CODEC_ID_AC3, AV_CODEC_ID_MP3, AV_CODEC_ID_NONE};
use ffmpeg_sys_next::AVColorRange::AVCOL_RANGE_UNSPECIFIED;
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_UNSPECIFIED;
use ffmpeg_sys_next::AVMediaType::{
    AVMEDIA_TYPE_ATTACHMENT, AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_DATA, AVMEDIA_TYPE_SUBTITLE,
    AVMEDIA_TYPE_VIDEO,
};
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NONE;
use ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_NONE;
use ffmpeg_sys_next::{
    av_add_q, av_channel_layout_default, av_codec_get_id, av_codec_get_tag2, av_dict_free,
    av_freep, av_get_exact_bits_per_sample, av_guess_codec, av_guess_format, av_guess_frame_rate,
    av_inv_q, av_malloc, av_rescale_q, av_seek_frame, avcodec_alloc_context3,
    avcodec_descriptor_get, avcodec_descriptor_get_by_name, avcodec_find_encoder,
    avcodec_find_encoder_by_name, avcodec_get_name, avcodec_parameters_from_context,
    avcodec_parameters_to_context, avfilter_graph_alloc, avfilter_graph_free, avfilter_inout_free,
    avfilter_pad_get_name, avfilter_pad_get_type, avformat_alloc_context,
    avformat_alloc_output_context2, avformat_close_input, avformat_find_stream_info,
    avformat_flush, avformat_free_context, avformat_open_input, avio_alloc_context,
    avio_context_free, avio_open, AVCodec, AVCodecID, AVColorRange, AVColorSpace, AVFilterContext,
    AVFilterInOut, AVFilterPad, AVFormatContext, AVMediaType, AVOutputFormat, AVPixelFormat,
    AVRational, AVSampleFormat, AVStream, AVERROR_ENCODER_NOT_FOUND, AVFMT_FLAG_CUSTOM_IO,
    AVFMT_GLOBALHEADER, AVFMT_NOBINSEARCH, AVFMT_NOFILE, AVFMT_NOGENSEARCH, AVFMT_NOSTREAMS,
    AVIO_FLAG_WRITE, AVSEEK_FLAG_BACKWARD, AV_CODEC_PROP_BITMAP_SUB, AV_CODEC_PROP_TEXT_SUB,
    AV_TIME_BASE,
};
#[cfg(not(feature = "docs-rs"))]
use ffmpeg_sys_next::{
    av_channel_layout_copy, av_packet_side_data_new, avcodec_get_supported_config,
    avfilter_graph_segment_apply, avfilter_graph_segment_create_filters,
    avfilter_graph_segment_free, avfilter_graph_segment_parse, AVChannelLayout,
};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::ffi::{c_uint, c_void, CStr, CString};
use std::ptr::{null, null_mut};
use std::sync::Arc;

pub struct FfmpegContext {
    pub(crate) independent_readrate: bool,
    pub(crate) demuxs: Vec<Demuxer>,
    pub(crate) filter_graphs: Vec<FilterGraph>,
    pub(crate) muxs: Vec<Muxer>,
}

unsafe impl Send for FfmpegContext {}
unsafe impl Sync for FfmpegContext {}

impl FfmpegContext {
    /// Creates a new [`FfmpegContextBuilder`] which allows you to configure
    /// and construct an [`FfmpegContext`] with custom inputs, outputs, filters,
    /// and other parameters.
    ///
    /// # Examples
    /// ```rust
    /// let context = FfmpegContext::builder()
    ///     .input("input.mp4")
    ///     .output("output.mp4")
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn builder() -> FfmpegContextBuilder {
        FfmpegContextBuilder::new()
    }

    /// Consumes this [`FfmpegContext`] and starts an FFmpeg job, returning
    /// an [`FfmpegScheduler<ffmpeg_scheduler::Running>`] for further management.
    ///
    /// Internally, this method creates an [`FfmpegScheduler`] from the context
    /// and immediately calls [`FfmpegScheduler::start()`].
    ///
    /// # Returns
    /// - `Ok(FfmpegScheduler<Running>)` if the scheduling process started successfully.
    /// - `Err(...)` if there was an error initializing or starting FFmpeg.
    ///
    /// # Example
    /// ```rust
    /// let context = FfmpegContext::builder()
    ///     .input("input.mp4")
    ///     .output("output.mp4")
    ///     .build()
    ///     .unwrap();
    ///
    /// // Start the FFmpeg job and get a scheduler to manage it
    /// let scheduler = context.start().expect("Failed to start Ffmpeg job");
    ///
    /// // Optionally, wait for it to finish
    /// let result = scheduler.wait();
    /// assert!(result.is_ok());
    /// ```
    pub fn start(self) -> Result<FfmpegScheduler<ffmpeg_scheduler::Running>> {
        let ffmpeg_scheduler = FfmpegScheduler::new(self);
        ffmpeg_scheduler.start()
    }

    #[allow(dead_code)]
    pub(crate) fn new(
        inputs: Vec<Input>,
        filter_complexs: Vec<FilterComplex>,
        outputs: Vec<Output>,
    ) -> Result<FfmpegContext> {
        Self::new_with_options(false, inputs, filter_complexs, outputs, false)
    }

    pub(crate) fn new_with_options(
        mut independent_readrate: bool,
        mut inputs: Vec<Input>,
        filter_complexs: Vec<FilterComplex>,
        mut outputs: Vec<Output>,
        copy_ts: bool,
    ) -> Result<FfmpegContext> {
        check_duplicate_inputs_outputs(&inputs, &outputs)?;

        crate::core::initialize_ffmpeg();

        let mut demuxs = open_input_files(&mut inputs, copy_ts)?;

        if demuxs.len() <= 1 {
            independent_readrate = false;
        }

        let mut filter_graphs = if !filter_complexs.is_empty() {
            let mut filter_graphs = init_filter_graphs(filter_complexs)?;
            fg_bind_inputs(&mut filter_graphs, &mut demuxs)?;
            filter_graphs
        } else {
            Vec::new()
        };

        let mut muxs = open_output_files(&mut outputs, copy_ts)?;

        outputs_bind(&mut muxs, &mut filter_graphs, &mut demuxs)?;

        correct_input_start_times(&mut demuxs, copy_ts);

        check_output_streams(&muxs)?;

        check_fg_bindings(&filter_graphs)?;

        check_frame_filter_pipeline(&muxs, &demuxs)?;

        Ok(Self {
            independent_readrate,
            demuxs,
            filter_graphs,
            muxs,
        })
    }
}

const START_AT_ZERO: bool = false;

fn correct_input_start_times(demuxs: &mut Vec<Demuxer>, copy_ts: bool) {
    for (i, demux) in demuxs.iter_mut().enumerate() {
        unsafe {
            let is = demux.in_fmt_ctx;

            demux.start_time_effective = (*is).start_time;
            if (*is).start_time == ffmpeg_sys_next::AV_NOPTS_VALUE
                || (*(*is).iformat).flags & ffmpeg_sys_next::AVFMT_TS_DISCONT == 0
            {
                continue;
            }

            let mut new_start_time = i64::MAX;
            let stream_count = (*is).nb_streams;
            for j in 0..stream_count {
                let st = *(*is).streams.add(j as usize);
                if (*st).discard == ffmpeg_sys_next::AVDiscard::AVDISCARD_ALL
                    || (*st).start_time == ffmpeg_sys_next::AV_NOPTS_VALUE
                {
                    continue;
                }
                new_start_time = std::cmp::min(
                    new_start_time,
                    av_rescale_q(
                        (*st).start_time,
                        (*st).time_base,
                        ffmpeg_sys_next::AV_TIME_BASE_Q,
                    ),
                );
            }
            let diff = new_start_time - (*is).start_time;
            if diff != 0 {
                debug!("Correcting start time of Input #{i} by {diff}us.");
                demux.start_time_effective = new_start_time;
                if copy_ts && START_AT_ZERO {
                    demux.ts_offset = -new_start_time;
                } else if !copy_ts {
                    let abs_start_seek = (*is).start_time + demux.start_time_us.unwrap_or(0);
                    demux.ts_offset = if abs_start_seek > new_start_time {
                        -abs_start_seek
                    } else {
                        -new_start_time
                    };
                } else if copy_ts {
                    demux.ts_offset = 0;
                }

                // demux.ts_offset += demux.input_ts_offset;
            }
        }
    }
}

fn check_pipeline<T>(
    frame_pipelines: Option<&Vec<FramePipeline>>,
    streams: &[T],
    tag: &str,
    get_stream_index: impl Fn(&T) -> usize,
    get_codec_type: impl Fn(&T) -> &AVMediaType,
) -> Result<()> {
    let tag_cap = {
        let mut chars = tag.chars();
        match chars.next() {
            None => String::new(),
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        }
    };

    frame_pipelines
        .into_iter()
        .flat_map(|pipelines| pipelines.iter())
        .try_for_each(|pipeline| {
            if let Some(idx) = pipeline.stream_index {
                streams
                    .iter()
                    .any(|s| {
                        get_stream_index(s) == idx && get_codec_type(s) == &pipeline.media_type
                    })
                    .then(|| ())
                    .ok_or_else(|| {
                        Into::<crate::error::Error>::into(FrameFilterStreamTypeNoMatched(
                            tag_cap.clone(),
                            idx,
                            format!("{:?}", pipeline.media_type),
                        ))
                    })
            } else {
                streams
                    .iter()
                    .any(|s| get_codec_type(s) == &pipeline.media_type)
                    .then(|| ())
                    .ok_or_else(|| {
                        FrameFilterTypeNoMatched(tag.into(), format!("{:?}", pipeline.media_type))
                            .into()
                    })
            }
        })?;
    Ok(())
}

fn check_frame_filter_pipeline(muxs: &[Muxer], demuxs: &[Demuxer]) -> Result<()> {
    muxs.iter().try_for_each(|mux| {
        check_pipeline(
            mux.frame_pipelines.as_ref(),
            mux.get_streams(),
            "output",
            |s| s.stream_index,
            |s| &s.codec_type,
        )
    })?;
    demuxs.iter().try_for_each(|demux| {
        check_pipeline(
            demux.frame_pipelines.as_ref(),
            demux.get_streams(),
            "input",
            |s| s.stream_index,
            |s| &s.codec_type,
        )
    })?;
    Ok(())
}

fn check_fg_bindings(filter_graphs: &Vec<FilterGraph>) -> Result<()> {
    // check that all outputs were bound
    for filter_graph in filter_graphs {
        for (i, output_filter) in filter_graph.outputs.iter().enumerate() {
            if !output_filter.has_dst() {
                let linklabel = if output_filter.linklabel.is_empty() {
                    "unlabeled".to_string()
                } else {
                    output_filter.linklabel.clone()
                };
                return Err(OutputUnconnected(output_filter.name.clone(), i, linklabel).into());
            }
        }
    }
    Ok(())
}

impl Into<FfmpegScheduler<Initialization>> for FfmpegContext {
    fn into(self) -> FfmpegScheduler<Initialization> {
        FfmpegScheduler::new(self)
    }
}

fn check_output_streams(muxs: &Vec<Muxer>) -> Result<()> {
    for mux in muxs {
        unsafe {
            let oformat = (*mux.out_fmt_ctx).oformat;
            if !mux.has_src() && (*oformat).flags & AVFMT_NOSTREAMS == 0 {
                warn!("Output file does not contain any stream");
                return Err(OpenOutputError::NotContainStream.into());
            }
        }
    }
    Ok(())
}

/// Process metadata for output file
///
/// Mirrors FFmpeg's `copy_meta()` flow (ffmpeg_mux_init.c:3050-3070):
/// 1. Metadata mappings (`copy_metadata`)
/// 2. Chapter auto-copy (`copy_chapters`)
/// 3. Default auto-copy (`copy_metadata_default`)
/// 4. User-specified metadata (`of_add_metadata`)
unsafe fn process_metadata(mux: &Muxer, demuxs: &Vec<Demuxer>) -> Result<()> {
    use crate::core::metadata::MetadataType;
    use crate::core::metadata::{
        copy_chapters_from_input, copy_metadata, copy_metadata_default, of_add_metadata,
    };

    // Collect input format contexts for metadata copying
    let input_ctxs: Vec<*const AVFormatContext> = demuxs
        .iter()
        .map(|d| d.in_fmt_ctx as *const AVFormatContext)
        .collect();

    let mut metadata_global_manual = false;
    let mut metadata_streams_manual = false;
    let mut metadata_chapters_manual = false;

    // Step 1: Process explicit metadata mappings from user (-map_metadata option)
    // FFmpeg: ffmpeg_mux_init.c:3052-3058
    let mut mark_manual = |meta_type: &MetadataType| -> () {
        match meta_type {
            MetadataType::Global => metadata_global_manual = true,
            MetadataType::Stream(_) => metadata_streams_manual = true,
            MetadataType::Chapter(_) => metadata_chapters_manual = true,
            MetadataType::Program(_) => {}
        }
    };

    for mapping in &mux.metadata_map {
        mark_manual(&mapping.src_type);
        mark_manual(&mapping.dst_type);

        if mapping.input_index >= input_ctxs.len() {
            log::warn!(
                "Metadata mapping references non-existent input file index {}",
                mapping.input_index
            );
            continue;
        }

        let input_ctx = input_ctxs[mapping.input_index];
        if let Err(e) = copy_metadata(
            input_ctx,
            mux.out_fmt_ctx,
            &mapping.src_type,
            &mapping.dst_type,
        ) {
            log::warn!("Failed to copy metadata from mapping: {}", e);
        }
    }

    // Step 2: Copy chapters from first input with chapters (if auto copy enabled)
    if mux.auto_copy_metadata && !metadata_chapters_manual {
        if let Some(source_demux) = demuxs
            .iter()
            .find(|d| unsafe { !d.in_fmt_ctx.is_null() && (*d.in_fmt_ctx).nb_chapters > 0 })
        {
            if let Err(e) = copy_chapters_from_input(
                source_demux.in_fmt_ctx,
                source_demux.ts_offset,
                mux.out_fmt_ctx,
                mux.start_time_us,
                mux.recording_time_us,
                true,
            ) {
                log::warn!("Failed to copy chapters: {}", e);
            }
        }
    }

    // Step 3: Apply FFmpeg's automatic metadata copying (if not disabled by user)
    // FFmpeg: ffmpeg_mux_init.c:3064-3067
    let stream_input_mapping = mux.stream_input_mapping();
    let encoding_streams = mux.encoding_streams();

    if mux.auto_copy_metadata {
        if let Err(e) = copy_metadata_default(
            &input_ctxs,
            mux.out_fmt_ctx,
            mux.nb_streams,
            &stream_input_mapping,
            &encoding_streams,
            mux.recording_time_us.is_some(),
            mux.auto_copy_metadata,
            metadata_global_manual,
            metadata_streams_manual,
        ) {
            log::warn!("Failed to apply default metadata behavior: {}", e);
        }
    }

    // Step 4: Apply user-specified metadata values (-metadata option)
    // FFmpeg: ffmpeg_mux_init.c:3060-3062 (invoked after copy_meta in original flow)
    if let Err(e) = of_add_metadata(
        mux.out_fmt_ctx,
        &mux.global_metadata,
        &mux.stream_metadata,
        &mux.chapter_metadata,
        &mux.program_metadata,
    ) {
        log::warn!("Failed to add user metadata: {}", e);
    }

    Ok(())
}

/// Check if linklabel refers to a filter graph output
/// FFmpeg reference: ffmpeg_opt.c:493 - if (arg[0] == '[')
fn is_filter_output_linklabel(linklabel: &str) -> bool {
    linklabel.starts_with('[')
}

/// Parse and expand stream map specifications
/// This mimics FFmpeg's opt_map() behavior: parse once, expand immediately
/// FFmpeg reference: ffmpeg_opt.c:478-596
unsafe fn expand_stream_maps(
    mux: &mut Muxer,
    demuxs: &[Demuxer],
) -> Result<()> {
    let stream_map_specs = std::mem::take(&mut mux.stream_map_specs);

    for spec in stream_map_specs {
        // FFmpeg reference: opt_map line 488-491 - check for negative map
        let (linklabel, is_negative) = if spec.linklabel.starts_with('-') {
            (&spec.linklabel[1..], true)
        } else {
            (spec.linklabel.as_str(), false)
        };

        // FFmpeg reference: opt_map line 493 - check if this mapping refers to lavfi output
        if is_filter_output_linklabel(linklabel) {
            // FFmpeg reference: opt_map line 494-507 - extract linklabel from brackets
            let pure_linklabel = if linklabel.starts_with('[') {
                // Extract content between '[' and ']'
                if let Some(end_pos) = linklabel.find(']') {
                    &linklabel[1..end_pos]
                } else {
                    warn!("Invalid output link label: {}.", linklabel);
                    return Err(Error::OpenOutput(OpenOutputError::InvalidArgument));
                }
            } else {
                linklabel
            };

            // FFmpeg reference: opt_map line 504 - validate non-empty
            if pure_linklabel.is_empty() {
                warn!("Invalid output link label: {}.", linklabel);
                return Err(Error::OpenOutput(OpenOutputError::InvalidArgument));
            }

            // Store pure linklabel (without brackets) for later matching in map_manual()
            mux.stream_maps.push(StreamMap {
                file_index: 0,  // Not used for filter outputs
                stream_index: 0,  // Not used for filter outputs
                linklabel: Some(pure_linklabel.to_string()),
                copy: spec.copy,
                disabled: false,
            });
            continue;
        }

        // FFmpeg reference: opt_map line 512 - parse file index using strtol
        // Try to parse as file stream; if it fails, treat as filter output
        let parse_result = strtol(linklabel);
        if parse_result.is_err() {
            // Failed to parse file index - treat as filter output linklabel
            // This allows bare filter output names like "my-out" (without brackets)
            mux.stream_maps.push(StreamMap {
                file_index: 0,
                stream_index: 0,
                linklabel: Some(linklabel.to_string()),
                copy: spec.copy,
                disabled: false,
            });
            continue;
        }

        let (file_idx, remainder) = parse_result.unwrap();

        // FFmpeg reference: opt_map line 513-517 - validate file index
        if file_idx < 0 || file_idx as usize >= demuxs.len() {
            warn!("Invalid input file index: {}.", file_idx);
            return Err(Error::OpenOutput(OpenOutputError::InvalidArgument));
        }
        let file_idx = file_idx as usize;

        // FFmpeg reference: opt_map line 520 - parse stream specifier
        // FFmpeg reference: opt_map line 533 - handle '?' suffix for allow_unused
        let (spec_str, allow_unused) = if remainder.ends_with('?') {
            (&remainder[..remainder.len()-1], true)
        } else {
            (remainder, false)
        };

        let stream_spec = if spec_str.is_empty() {
            // Empty specifier matches all streams (FFmpeg behavior)
            StreamSpecifier::default()
        } else {
            // Strip leading ':' and parse stream specifier
            let spec_str = if spec_str.starts_with(':') {
                &spec_str[1..]
            } else {
                spec_str
            };

            StreamSpecifier::parse(spec_str).map_err(|e| {
                warn!("Invalid stream specifier in '{}': {}", linklabel, e);
                Error::OpenOutput(OpenOutputError::InvalidArgument)
            })?
        };

        // FFmpeg reference: opt_map line 543-553 - negative map: disable matching streams
        if is_negative {
            for existing_map in &mut mux.stream_maps {
                // Only process file-based maps (not filter outputs)
                if existing_map.linklabel.is_none() &&
                   existing_map.file_index == file_idx {
                    // Check if stream specifier matches
                    let demux = &demuxs[file_idx];
                    let fmt_ctx = demux.in_fmt_ctx;
                    let avstream = *(*fmt_ctx).streams.add(existing_map.stream_index);

                    if stream_spec.matches(fmt_ctx, avstream) {
                        existing_map.disabled = true;
                    }
                }
            }
            // Negative map doesn't add new entries, just disables existing ones
            continue;
        }

        // FFmpeg reference: opt_map line 555-574 - expand to one StreamMap per matched stream
        let demux = &demuxs[file_idx];
        let fmt_ctx = demux.in_fmt_ctx;
        let mut matched_count = 0;

        for (stream_idx, _) in demux.get_streams().iter().enumerate() {
            let avstream = *(*fmt_ctx).streams.add(stream_idx);

            // FFmpeg reference: opt_map line 556 - stream_specifier_match
            if stream_spec.matches(fmt_ctx, avstream) {
                mux.stream_maps.push(StreamMap {
                    file_index: file_idx,
                    stream_index: stream_idx,
                    linklabel: None,
                    copy: spec.copy,
                    disabled: false,
                });
                matched_count += 1;
            }
        }

        // FFmpeg reference: opt_map lines 577-590 - error handling for no matches
        if matched_count == 0 {
            if allow_unused {
                // FFmpeg line 579: verbose log for optional mappings
                info!(
                    "Stream map '{}' matches no streams; ignoring.",
                    linklabel
                );
            } else {
                // FFmpeg line 586-587: fatal error with hint about '?' suffix
                warn!(
                    "Stream map '{}' matches no streams.\n\
                     To ignore this, add a trailing '?' to the map.",
                    linklabel
                );
                return Err(Error::OpenOutput(
                    OpenOutputError::MatchesNoStreams(linklabel.to_string())
                ));
            }
        }
    }

    Ok(())
}

fn outputs_bind(
    muxs: &mut Vec<Muxer>,
    filter_graphs: &mut Vec<FilterGraph>,
    demuxs: &mut Vec<Demuxer>,
) -> Result<()> {
    // FFmpeg reference: ffmpeg.c calls opt_map during command-line parsing
    // We parse and expand stream maps early, before processing individual streams
    // This must happen AFTER demuxers are opened (need stream info for matching)
    // but BEFORE map_manual() is called (which uses the expanded StreamMap entries)
    unsafe {
        for mux in muxs.iter_mut() {
            if !mux.stream_map_specs.is_empty() {
                expand_stream_maps(mux, demuxs)?;
            }
        }
    }

    for (i, mux) in muxs.iter_mut().enumerate() {
        if mux.stream_maps.is_empty() {
            let mut auto_disable = 0;
            output_bind_by_unlabeled_filter(i, mux, filter_graphs, &mut auto_disable)?;
            /* pick the first stream of each type */
            map_auto_streams(i, mux, demuxs, filter_graphs, auto_disable)?;
        } else {
            for stream_map in mux.stream_maps.clone() {
                map_manual(i, mux, &stream_map, filter_graphs, demuxs)?;
            }
        }

        //TODO add_attachments

        // Process metadata
        unsafe {
            process_metadata(mux, demuxs)?;
        }
    }

    Ok(())
}

fn map_manual(
    index: usize,
    mux: &mut Muxer,
    stream_map: &StreamMap,
    filter_graphs: &mut Vec<FilterGraph>,
    demuxs: &mut Vec<Demuxer>,
) -> Result<()> {
    // FFmpeg reference: ffmpeg_mux_init.c:1720-1721 - check disabled flag
    if stream_map.disabled {
        return Ok(());
    }

    // FFmpeg reference: ffmpeg_mux_init.c:1723 - check for filter output
    if let Some(linklabel) = &stream_map.linklabel {
        // This is a filter graph output - match by linklabel
        for filter_graph in filter_graphs.iter_mut() {
            for i in 0..filter_graph.outputs.len() {
                let option = {
                    let output_filter = &filter_graph.outputs[i];
                    if output_filter.has_dst()
                        || output_filter.linklabel.is_empty()
                        || &output_filter.linklabel != linklabel
                    {
                        continue;
                    }

                    choose_encoder(mux, output_filter.media_type)?
                };

                match option {
                    None => {
                        warn!(
                            "An unexpected media_type {:?} appears in output_filter",
                            filter_graph.outputs[i].media_type
                        );
                    }
                    Some((codec_id, enc)) => {
                        return ofilter_bind_ost(index, mux, filter_graph, i, codec_id, enc, None)
                            .map(|_| ());
                    }
                }
            }
        }

        // FFmpeg reference: ffmpeg_mux_init.c:1740-1742 - filter output not found error
        warn!(
            "Output with label '{}' does not exist in any defined filter graph, \
             or was already used elsewhere.",
            linklabel
        );
        return Err(OpenOutputError::InvalidArgument.into());
    }

    // This is an input file stream - use pre-parsed file_index and stream_index
    // These were already validated and expanded in expand_stream_maps()
    let demux_idx = stream_map.file_index;
    let stream_index = stream_map.stream_index;

    let demux = &mut demuxs[demux_idx];

    // Get immutable data first to avoid borrow checker issues
    let demux_node = demux.node.clone();
    let (media_type, input_stream_duration, input_stream_time_base) = {
        let input_stream = demux.get_stream_mut(stream_index);
        (input_stream.codec_type, input_stream.duration, input_stream.time_base)
    };

    info!(
        "Binding output stream to input {}:{} ({})",
        demux_idx, stream_index,
        match media_type {
            AVMEDIA_TYPE_VIDEO => "video",
            AVMEDIA_TYPE_AUDIO => "audio",
            AVMEDIA_TYPE_SUBTITLE => "subtitle",
            AVMEDIA_TYPE_DATA => "data",
            AVMEDIA_TYPE_ATTACHMENT => "attachment",
            _ => "unknown",
        }
    );

    let option = choose_encoder(mux, media_type)?;

    match option {
        None => {
            // copy
            let (packet_sender, _st, output_stream_index) = mux.new_stream(demux_node)?;
            demux.add_packet_dst(packet_sender, stream_index, output_stream_index);
            mux.register_stream_source(output_stream_index, demux_idx, stream_index, false);

            unsafe {
                streamcopy_init(
                    mux,
                    *(*demux.in_fmt_ctx).streams.add(stream_index),
                    *(*mux.out_fmt_ctx).streams.add(output_stream_index),
                )?;
                rescale_duration(
                    input_stream_duration,
                    input_stream_time_base,
                    *(*mux.out_fmt_ctx).streams.add(output_stream_index),
                );
                mux.stream_ready()
            }
        }
        Some((codec_id, enc)) => {
            // connect input_stream to output
            if stream_map.copy {
                // copy
                let (packet_sender, _st, output_stream_index) = mux.new_stream(demux_node)?;
                demux.add_packet_dst(packet_sender, stream_index, output_stream_index);
                mux.register_stream_source(output_stream_index, demux_idx, stream_index, false);

                unsafe {
                    streamcopy_init(
                        mux,
                        *(*demux.in_fmt_ctx).streams.add(stream_index),
                        *(*mux.out_fmt_ctx).streams.add(output_stream_index),
                    )?;
                    rescale_duration(
                        input_stream_duration,
                        input_stream_time_base,
                        *(*mux.out_fmt_ctx).streams.add(output_stream_index),
                    );
                    mux.stream_ready()
                }
            } else {
                if media_type == AVMEDIA_TYPE_VIDEO || media_type == AVMEDIA_TYPE_AUDIO {
                    init_simple_filtergraph(
                        demux,
                        stream_index,
                        codec_id,
                        enc,
                        index,
                        mux,
                        filter_graphs,
                        demux_idx,
                    )?;
                } else {
                    let (frame_sender, output_stream_index) =
                        mux.add_enc_stream(media_type, enc, demux_node)?;
                    let input_stream = demux.get_stream_mut(stream_index);
                    input_stream.add_dst(frame_sender);
                    demux.connect_stream(stream_index);
                    mux.register_stream_source(output_stream_index, demux_idx, stream_index, true);

                    unsafe {
                        rescale_duration(
                            input_stream_duration,
                            input_stream_time_base,
                            *(*mux.out_fmt_ctx).streams.add(output_stream_index),
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
fn set_channel_layout(
    ch_layout: &mut AVChannelLayout,
    ch_layouts: &Option<Vec<AVChannelLayout>>,
    layout_requested: &AVChannelLayout,
) -> Result<()> {
    unsafe {
        // Scenario 1: If the requested layout has a specified order (not UNSPEC), copy it directly
        if layout_requested.order != AV_CHANNEL_ORDER_UNSPEC {
            let ret = av_channel_layout_copy(ch_layout, layout_requested);
            if ret < 0 {
                return Err(OpenOutputError::from(ret).into());
            }
            return Ok(());
        }

        // Scenario 2: Requested layout is UNSPEC and no encoder-supported layouts available
        // Use default layout based on channel count
        if ch_layouts.is_none() {
            av_channel_layout_default(ch_layout, layout_requested.nb_channels);
            return Ok(());
        }

        // Scenario 3: Try to match channel count from encoder's supported layouts
        if let Some(layouts) = ch_layouts {
            for layout in layouts {
                if layout.nb_channels == layout_requested.nb_channels {
                    let ret = av_channel_layout_copy(ch_layout, layout);
                    if ret < 0 {
                        return Err(OpenOutputError::from(ret).into());
                    }
                    return Ok(());
                }
            }
        }

        // Scenario 4: No matching channel count found, use default layout
        av_channel_layout_default(ch_layout, layout_requested.nb_channels);
        Ok(())
    }
}

#[cfg(feature = "docs-rs")]
fn configure_output_filter_opts(
    index: usize,
    mux: &mut Muxer,
    output_filter: &mut OutputFilter,
    codec_id: AVCodecID,
    enc: *const AVCodec,
    output_stream_index: usize,
) -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
fn configure_output_filter_opts(
    index: usize,
    mux: &mut Muxer,
    output_filter: &mut OutputFilter,
    codec_id: AVCodecID,
    enc: *const AVCodec,
    output_stream_index: usize,
) -> Result<()> {
    unsafe {
        output_filter.opts.name = format!("#{index}:{output_stream_index}");
        output_filter.opts.enc = enc;
        output_filter.opts.trim_start_us = mux.start_time_us;
        output_filter.opts.trim_duration_us = mux.recording_time_us;
        output_filter.opts.ts_offset = mux.start_time_us;

        output_filter.opts.flags = OFILTER_FLAG_DISABLE_CONVERT
            | OFILTER_FLAG_AUTOSCALE
            | if av_get_exact_bits_per_sample(codec_id) == 24 {
                OFILTER_FLAG_AUDIO_24BIT
            } else {
                0
            };

        let enc_ctx = avcodec_alloc_context3(enc);
        if enc_ctx.is_null() {
            return Err(OpenOutputError::OutOfMemory.into());
        }
        let _codec_ctx = CodecContext::new(enc_ctx);

        (*enc_ctx).thread_count = 0;

        if output_filter.media_type == AVMEDIA_TYPE_VIDEO {
            // formats
            let mut formats: *const AVPixelFormat = null();
            let mut ret = avcodec_get_supported_config(
                enc_ctx,
                null(),
                AV_CODEC_CONFIG_PIX_FORMAT,
                0,
                &mut formats as *mut _ as *mut *const libc::c_void,
                null_mut(),
            );
            if ret < 0 {
                return Err(OpenOutputError::from(ret).into());
            }

            let mut current = formats;
            let mut format_list = Vec::new();
            let mut count = 0;
            const MAX_FORMATS: usize = 512;
            while !current.is_null() && *current != AV_PIX_FMT_NONE && count < MAX_FORMATS {
                format_list.push(*current);
                current = current.add(1);
                count += 1;
            }
            if count >= MAX_FORMATS {
                warn!("Reached maximum format limit");
            }
            output_filter.opts.formats = Some(format_list);

            // framerates
            let mut framerates: *const AVRational = null();
            ret = avcodec_get_supported_config(
                enc_ctx,
                null(),
                AV_CODEC_CONFIG_FRAME_RATE,
                0,
                &mut framerates as *mut _ as *mut *const libc::c_void,
                null_mut(),
            );
            if ret < 0 {
                return Err(OpenOutputError::from(ret).into());
            }
            let mut framerate_list = Vec::new();
            let mut current = framerates;
            let mut count = 0;
            const MAX_FRAMERATES: usize = 64;
            while !current.is_null()
                && (*current).num != 0
                && (*current).den != 0
                && count < MAX_FRAMERATES
            {
                framerate_list.push(*current);
                current = current.add(1);
                count += 1;
            }
            if count >= MAX_FRAMERATES {
                warn!("Reached maximum framerate limit");
            }
            output_filter.opts.framerates = Some(framerate_list);

            if let Some(framerate) = mux.framerate {
                output_filter.opts.framerate = framerate;
            }

            // color_spaces
            let mut color_spaces: *const AVColorSpace = null();
            ret = avcodec_get_supported_config(
                enc_ctx,
                null(),
                AV_CODEC_CONFIG_COLOR_SPACE,
                0,
                &mut color_spaces as *mut _ as *mut *const libc::c_void,
                null_mut(),
            );
            if ret < 0 {
                return Err(OpenOutputError::from(ret).into());
            }
            let mut color_space_list = Vec::new();
            let mut current = color_spaces;
            let mut count = 0;
            const MAX_COLOR_SPACES: usize = 128;
            while !current.is_null()
                && *current != AVCOL_SPC_UNSPECIFIED
                && count < MAX_COLOR_SPACES
            {
                color_space_list.push(*current);
                current = current.add(1);
                count += 1;
            }
            if count >= MAX_COLOR_SPACES {
                warn!("Reached maximum color space limit");
            }
            output_filter.opts.color_spaces = Some(color_space_list);

            //color_ranges
            let mut color_ranges: *const AVColorRange = null();
            ret = avcodec_get_supported_config(
                enc_ctx,
                null(),
                AV_CODEC_CONFIG_COLOR_RANGE,
                0,
                &mut color_ranges as *mut _ as *mut *const libc::c_void,
                null_mut(),
            );
            if ret < 0 {
                return Err(OpenOutputError::from(ret).into());
            }
            let mut color_range_list = Vec::new();
            let mut current = color_ranges;
            let mut count = 0;
            const MAX_COLOR_RANGES: usize = 64;
            while !current.is_null()
                && *current != AVCOL_RANGE_UNSPECIFIED
                && count < MAX_COLOR_RANGES
            {
                color_range_list.push(*current);
                current = current.add(1);
                count += 1;
            }
            if count >= MAX_COLOR_RANGES {
                warn!("Reached maximum color range limit");
            }
            output_filter.opts.color_ranges = Some(color_range_list);

            let stream = &mux.get_streams()[output_stream_index];
            output_filter.opts.vsync_method = stream.vsync_method;
        } else {
            if let Some(sample_fmt) = &mux.audio_sample_fmt {
                output_filter.opts.audio_format = *sample_fmt;
            }
            // audio formats
            let mut audio_formats: *const AVSampleFormat = null();
            let mut ret = avcodec_get_supported_config(
                enc_ctx,
                null(),
                AV_CODEC_CONFIG_SAMPLE_FORMAT,
                0,
                &mut audio_formats as *mut _ as *mut _,
                null_mut(),
            );
            if ret < 0 {
                return Err(OpenOutputError::from(ret).into());
            }

            let mut current = audio_formats;
            let mut audio_format_list = Vec::new();
            let mut count = 0;
            const MAX_AUDIO_FORMATS: usize = 32;
            while !current.is_null() && *current != AV_SAMPLE_FMT_NONE && count < MAX_AUDIO_FORMATS
            {
                audio_format_list.push(*current);
                current = current.add(1);
                count += 1;
            }
            if count >= MAX_AUDIO_FORMATS {
                warn!("Reached maximum audio format limit");
            }
            output_filter.opts.audio_formats = Some(audio_format_list);

            if let Some(audio_sample_rate) = &mux.audio_sample_rate {
                output_filter.opts.sample_rate = *audio_sample_rate;
            }
            // sample_rates
            let mut rates: *const i32 = null();
            ret = avcodec_get_supported_config(
                enc_ctx,
                null(),
                AV_CODEC_CONFIG_SAMPLE_RATE,
                0,
                &mut rates as *mut _ as *mut _,
                null_mut(),
            );
            if ret < 0 {
                return Err(OpenOutputError::from(ret).into());
            }
            let mut rate_list = Vec::new();
            let mut current = rates;
            let mut count = 0;
            const MAX_SAMPLE_RATES: usize = 64;
            while !current.is_null() && *current != 0 && count < MAX_SAMPLE_RATES {
                rate_list.push(*current);
                current = current.add(1);
                count += 1;
            }
            if count >= MAX_SAMPLE_RATES {
                warn!("Reached maximum sample rate limit");
            }
            output_filter.opts.sample_rates = Some(rate_list);

            if let Some(channels) = &mux.audio_channels {
                output_filter.opts.ch_layout.nb_channels = *channels;
            }
            // channel_layouts
            let mut layouts: *const AVChannelLayout = null();
            ret = avcodec_get_supported_config(
                enc_ctx,
                null(),
                AV_CODEC_CONFIG_CHANNEL_LAYOUT,
                0,
                &mut layouts as *mut _ as *mut _,
                null_mut(),
            );
            if ret < 0 {
                return Err(OpenOutputError::from(ret).into());
            }
            let mut layout_list = Vec::new();
            let mut current = layouts;
            let mut count = 0;
            const MAX_CHANNEL_LAYOUTS: usize = 128;
            while !current.is_null()
                && (*current).order != AV_CHANNEL_ORDER_UNSPEC
                && count < MAX_CHANNEL_LAYOUTS
            {
                layout_list.push(*current);
                current = current.add(1);
                count += 1;
            }
            if count >= MAX_CHANNEL_LAYOUTS {
                warn!("Reached maximum channel layout limit");
            }
            output_filter.opts.ch_layouts = Some(layout_list);

            // Call set_channel_layout to resolve UNSPEC layouts to proper defaults
            // Corresponds to FFmpeg ffmpeg_filter.c:879-882
            if output_filter.opts.ch_layout.nb_channels > 0 {
                let layout_requested = output_filter.opts.ch_layout.clone();
                set_channel_layout(
                    &mut output_filter.opts.ch_layout,
                    &output_filter.opts.ch_layouts,
                    &layout_requested,
                )?;
            }
        }
    };
    Ok(())
}

fn map_auto_streams(
    mux_index: usize,
    mux: &mut Muxer,
    demuxs: &mut Vec<Demuxer>,
    filter_graphs: &mut Vec<FilterGraph>,
    auto_disable: i32,
) -> Result<()> {
    unsafe {
        let oformat = (*mux.out_fmt_ctx).oformat;
        map_auto_stream(
            mux_index,
            mux,
            demuxs,
            oformat,
            AVMEDIA_TYPE_VIDEO,
            filter_graphs,
            auto_disable,
        )?;
        map_auto_stream(
            mux_index,
            mux,
            demuxs,
            oformat,
            AVMEDIA_TYPE_AUDIO,
            filter_graphs,
            auto_disable,
        )?;
        map_auto_subtitle(mux, demuxs, oformat, auto_disable)?;
        map_auto_data(mux, demuxs, oformat, auto_disable)?;
    }
    Ok(())
}

#[cfg(feature = "docs-rs")]
unsafe fn map_auto_subtitle(
    mux: &mut Muxer,
    demuxs: &mut Vec<Demuxer>,
    oformat: *const AVOutputFormat,
    auto_disable: i32,
) -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn map_auto_subtitle(
    mux: &mut Muxer,
    demuxs: &mut Vec<Demuxer>,
    oformat: *const AVOutputFormat,
    auto_disable: i32,
) -> Result<()> {
    if auto_disable & (1 << AVMEDIA_TYPE_SUBTITLE as i32) != 0 {
        return Ok(());
    }

    let output_codec = avcodec_find_encoder((*oformat).subtitle_codec);
    if output_codec.is_null() {
        return Ok(());
    }
    let output_descriptor = avcodec_descriptor_get((*output_codec).id);

    for (demux_idx, demux) in demuxs.iter_mut().enumerate() {
        let option = demux
            .get_streams()
            .iter()
            .enumerate()
            .find_map(|(index, input_stream)| {
                if input_stream.codec_type == AVMEDIA_TYPE_SUBTITLE {
                    Some(index)
                } else {
                    None
                }
            });

        if option.is_none() {
            continue;
        }

        let stream_index = option.unwrap();

        let input_descriptor =
            avcodec_descriptor_get((*demux.get_stream(stream_index).codec_parameters).codec_id);
        let mut input_props = 0;
        if !input_descriptor.is_null() {
            input_props =
                (*input_descriptor).props & (AV_CODEC_PROP_TEXT_SUB | AV_CODEC_PROP_BITMAP_SUB);
        }
        let mut output_props = 0;
        if !output_descriptor.is_null() {
            output_props =
                (*output_descriptor).props & (AV_CODEC_PROP_TEXT_SUB | AV_CODEC_PROP_BITMAP_SUB);
        }

        if input_props & output_props != 0 ||
            // Map dvb teletext which has neither property to any output subtitle encoder
            !input_descriptor.is_null() && !output_descriptor.is_null() &&
                ((*input_descriptor).props == 0 || (*output_descriptor).props == 0)
        {
            let option = choose_encoder(mux, AVMEDIA_TYPE_SUBTITLE)?;

            if let Some((_codec_id, enc)) = option {
                let (frame_sender, output_stream_index) =
                    mux.add_enc_stream(AVMEDIA_TYPE_SUBTITLE, enc, demux.node.clone())?;
                demux.get_stream_mut(stream_index).add_dst(frame_sender);
                demux.connect_stream(stream_index);
                mux.register_stream_source(output_stream_index, demux_idx, stream_index, true);
                let input_stream = demux.get_stream(stream_index);
                unsafe {
                    rescale_duration(
                        input_stream.duration,
                        input_stream.time_base,
                        *(*mux.out_fmt_ctx).streams.add(output_stream_index),
                    );
                }
            } else {
                error!("Error selecting an encoder(subtitle)");
                return Err(OpenOutputError::from(AVERROR_ENCODER_NOT_FOUND).into());
            }
        }
        break;
    }

    Ok(())
}

#[cfg(feature = "docs-rs")]
unsafe fn map_auto_data(
    mux: &mut Muxer,
    demuxs: &mut Vec<Demuxer>,
    oformat: *const AVOutputFormat,
    auto_disable: i32,
) -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn map_auto_data(
    mux: &mut Muxer,
    demuxs: &mut Vec<Demuxer>,
    oformat: *const AVOutputFormat,
    auto_disable: i32,
) -> Result<()> {
    if auto_disable & (1 << AVMEDIA_TYPE_DATA as i32) != 0 {
        return Ok(());
    }

    /* Data only if codec id match */
    let codec_id = av_guess_codec(
        oformat,
        null(),
        (*mux.out_fmt_ctx).url,
        null(),
        AVMEDIA_TYPE_DATA,
    );

    if codec_id == AV_CODEC_ID_NONE {
        return Ok(());
    }

    for (demux_idx, demux) in demuxs.iter_mut().enumerate() {
        let option = demux
            .get_streams()
            .iter()
            .enumerate()
            .find_map(|(index, input_stream)| {
                if input_stream.codec_type == AVMEDIA_TYPE_DATA
                    && (*input_stream.codec_parameters).codec_id == codec_id
                {
                    Some(index)
                } else {
                    None
                }
            });

        if option.is_none() {
            continue;
        }

        let stream_index = option.unwrap();
        let option = choose_encoder(mux, AVMEDIA_TYPE_DATA)?;

        if let Some((_codec_id, enc)) = option {
            let (frame_sender, output_stream_index) =
                mux.add_enc_stream(AVMEDIA_TYPE_DATA, enc, demux.node.clone())?;
            demux.get_stream_mut(stream_index).add_dst(frame_sender);
            demux.connect_stream(stream_index);
            mux.register_stream_source(output_stream_index, demux_idx, stream_index, true);
            let input_stream = demux.get_stream(stream_index);
            unsafe {
                rescale_duration(
                    input_stream.duration,
                    input_stream.time_base,
                    *(*mux.out_fmt_ctx).streams.add(output_stream_index),
                );
            }
        } else {
            error!("Error selecting an encoder(data)");
            return Err(OpenOutputError::from(AVERROR_ENCODER_NOT_FOUND).into());
        }

        break;
    }

    Ok(())
}

#[cfg(feature = "docs-rs")]
unsafe fn map_auto_stream(
    mux_index: usize,
    mux: &mut Muxer,
    demuxs: &mut Vec<Demuxer>,
    oformat: *const AVOutputFormat,
    media_type: AVMediaType,
    filter_graphs: &mut Vec<FilterGraph>,
    auto_disable: i32,
) -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn map_auto_stream(
    mux_index: usize,
    mux: &mut Muxer,
    demuxs: &mut Vec<Demuxer>,
    oformat: *const AVOutputFormat,
    media_type: AVMediaType,
    filter_graphs: &mut Vec<FilterGraph>,
    auto_disable: i32,
) -> Result<()> {
    if auto_disable & (1 << media_type as i32) != 0 {
        return Ok(());
    }
    if media_type == AVMEDIA_TYPE_VIDEO
        || media_type == AVMEDIA_TYPE_AUDIO
        || media_type == AVMEDIA_TYPE_DATA
    {
        if av_guess_codec(oformat, null(), (*mux.out_fmt_ctx).url, null(), media_type)
            == AV_CODEC_ID_NONE
        {
            return Ok(());
        }
    }

    for (demux_idx, demux) in demuxs.iter_mut().enumerate() {
        let option = demux
            .get_streams()
            .iter()
            .enumerate()
            .find_map(|(index, input_stream)| {
                if input_stream.codec_type == media_type {
                    Some(index)
                } else {
                    None
                }
            });

        if option.is_none() {
            continue;
        }

        let stream_index = option.unwrap();
        let input_file_idx = demux_idx;
        let option = choose_encoder(mux, media_type)?;

        if let Some((codec_id, enc)) = option {
            if media_type == AVMEDIA_TYPE_VIDEO || media_type == AVMEDIA_TYPE_AUDIO {
                init_simple_filtergraph(
                    demux,
                    stream_index,
                    codec_id,
                    enc,
                    mux_index,
                    mux,
                    filter_graphs,
                    input_file_idx,
                )?;
            } else {
                let (frame_sender, output_stream_index) =
                    mux.add_enc_stream(media_type, enc, demux.node.clone())?;
                demux.get_stream_mut(stream_index).add_dst(frame_sender);
                demux.connect_stream(stream_index);
                mux.register_stream_source(output_stream_index, input_file_idx, stream_index, true);
                let input_stream = demux.get_stream(stream_index);
                unsafe {
                    rescale_duration(
                        input_stream.duration,
                        input_stream.time_base,
                        *(*mux.out_fmt_ctx).streams.add(output_stream_index),
                    );
                }
            }

            return Ok(());
        }

        // copy
        let input_stream = demux.get_stream(stream_index);
        let input_stream_duration = input_stream.duration;
        let input_stream_time_base = input_stream.time_base;

        let (packet_sender, _st, output_stream_index) = mux.new_stream(demux.node.clone())?;
        demux.add_packet_dst(packet_sender, stream_index, output_stream_index);
        mux.register_stream_source(output_stream_index, input_file_idx, stream_index, false);

        unsafe {
            streamcopy_init(
                mux,
                *(*demux.in_fmt_ctx).streams.add(stream_index),
                *(*mux.out_fmt_ctx).streams.add(output_stream_index),
            )?;
            rescale_duration(
                input_stream_duration,
                input_stream_time_base,
                *(*mux.out_fmt_ctx).streams.add(output_stream_index),
            );
            mux.stream_ready()
        }
    }

    Ok(())
}

fn init_simple_filtergraph(
    demux: &mut Demuxer,
    stream_index: usize,
    codec_id: AVCodecID,
    enc: *const AVCodec,
    mux_index: usize,
    mux: &mut Muxer,
    filter_graphs: &mut Vec<FilterGraph>,
    input_file_idx: usize,
) -> Result<()> {
    let codec_type = demux.get_stream(stream_index).codec_type;

    let filter_desc = if codec_type == AVMEDIA_TYPE_VIDEO {
        "null"
    } else {
        "anull"
    };
    let mut filter_graph = init_filter_graph(filter_graphs.len(), filter_desc, None)?;

    // filter_graph.inputs[0].media_type = codec_type;
    // filter_graph.outputs[0].media_type = codec_type;

    ifilter_bind_ist(&mut filter_graph, 0, stream_index, demux)?;
    ofilter_bind_ost(
        mux_index,
        mux,
        &mut filter_graph,
        0,
        codec_id,
        enc,
        Some((input_file_idx, stream_index)),
    )?;

    filter_graphs.push(filter_graph);

    Ok(())
}

unsafe fn rescale_duration(src_duration: i64, src_time_base: AVRational, stream: *mut AVStream) {
    (*stream).duration = av_rescale_q(src_duration, src_time_base, (*stream).time_base);
}

#[cfg(feature = "docs-rs")]
fn streamcopy_init(
    mux: &mut Muxer,
    input_stream: *mut AVStream,
    output_stream: *mut AVStream,
) -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
fn streamcopy_init(
    mux: &mut Muxer,
    input_stream: *mut AVStream,
    output_stream: *mut AVStream,
) -> Result<()> {
    unsafe {
        let codec_ctx = avcodec_alloc_context3(null_mut());
        if codec_ctx.is_null() {
            return Err(OpenOutputError::OutOfMemory.into());
        }
        let _codec_context = CodecContext::new(codec_ctx);

        let mut ret = avcodec_parameters_to_context(codec_ctx, (*input_stream).codecpar);
        if ret < 0 {
            error!("Error setting up codec context options.");
            return Err(OpenOutputError::from(ret).into());
        }

        ret = avcodec_parameters_from_context((*output_stream).codecpar, codec_ctx);
        if ret < 0 {
            error!("Error getting reference codec parameters.");
            return Err(OpenOutputError::from(ret).into());
        }

        let mut codec_tag = (*(*output_stream).codecpar).codec_tag;
        if codec_tag == 0 {
            let ct = (*(*mux.out_fmt_ctx).oformat).codec_tag;
            let mut codec_tag_tmp = 0;
            if ct.is_null()
                || av_codec_get_id(ct, (*(*output_stream).codecpar).codec_tag)
                    == (*(*output_stream).codecpar).codec_id
                || av_codec_get_tag2(
                    ct,
                    (*(*output_stream).codecpar).codec_id,
                    &mut codec_tag_tmp,
                ) == 0
            {
                codec_tag = (*(*output_stream).codecpar).codec_tag;
            }
        }
        (*(*output_stream).codecpar).codec_tag = codec_tag;

        let mut fr = (*output_stream).r_frame_rate;
        if fr.num == 0 {
            fr = (*input_stream).r_frame_rate;
        }

        if fr.num != 0 {
            (*output_stream).avg_frame_rate = fr;
        } else {
            (*output_stream).avg_frame_rate = (*input_stream).avg_frame_rate;
        }

        // copy timebase while removing common factors
        if (*output_stream).time_base.num <= 0 || (*output_stream).time_base.den <= 0 {
            if fr.num != 0 {
                (*output_stream).time_base = av_inv_q(fr);
            } else {
                (*output_stream).time_base =
                    av_add_q((*input_stream).time_base, AVRational { num: 0, den: 1 });
            }
        }

        for i in 0..(*(*input_stream).codecpar).nb_coded_side_data {
            let sd_src = (*(*input_stream).codecpar)
                .coded_side_data
                .offset(i as isize);

            let sd_dst = av_packet_side_data_new(
                &mut (*(*output_stream).codecpar).coded_side_data,
                &mut (*(*output_stream).codecpar).nb_coded_side_data,
                (*sd_src).type_,
                (*sd_src).size,
                0,
            );
            if sd_dst.is_null() {
                return Err(OpenOutputError::OutOfMemory.into());
            }
            std::ptr::copy_nonoverlapping(
                (*sd_src).data as *const u8,
                (*sd_dst).data,
                (*sd_src).size,
            );
        }

        match (*(*output_stream).codecpar).codec_type {
            AVMEDIA_TYPE_AUDIO => {
                if ((*(*output_stream).codecpar).block_align == 1
                    || (*(*output_stream).codecpar).block_align == 1152
                    || (*(*output_stream).codecpar).block_align == 576)
                    && (*(*output_stream).codecpar).codec_id == AV_CODEC_ID_MP3
                {
                    (*(*output_stream).codecpar).block_align = 0;
                }
                if (*(*output_stream).codecpar).codec_id == AV_CODEC_ID_AC3 {
                    (*(*output_stream).codecpar).block_align = 0;
                }
            }
            AVMEDIA_TYPE_VIDEO => {
                let sar = if (*input_stream).sample_aspect_ratio.num != 0 {
                    (*input_stream).sample_aspect_ratio
                } else {
                    (*(*output_stream).codecpar).sample_aspect_ratio
                };
                (*output_stream).sample_aspect_ratio = sar;
                (*(*output_stream).codecpar).sample_aspect_ratio = sar;
                (*output_stream).r_frame_rate = (*input_stream).r_frame_rate;
            }
            _ => {}
        }
    };
    Ok(())
}

fn output_bind_by_unlabeled_filter(
    index: usize,
    mux: &mut Muxer,
    filter_graphs: &mut Vec<FilterGraph>,
    auto_disable: &mut i32,
) -> Result<()> {
    let fg_len = filter_graphs.len();

    for i in 0..fg_len {
        let filter_graph = &mut filter_graphs[i];

        for i in 0..filter_graph.outputs.len() {
            let option = {
                let output_filter = &filter_graph.outputs[i];
                if (!output_filter.linklabel.is_empty() && output_filter.linklabel != "out")
                    || output_filter.has_dst()
                {
                    continue;
                }

                choose_encoder(mux, output_filter.media_type)?
            };

            let media_type = filter_graph.outputs[i].media_type;

            match option {
                None => {
                    warn!(
                        "An unexpected media_type {:?} appears in output_filter",
                        media_type
                    );
                }
                Some((codec_id, enc)) => {
                    *auto_disable |= 1 << media_type as i32;
                    ofilter_bind_ost(index, mux, filter_graph, i, codec_id, enc, None)?;
                }
            }
        }
    }

    Ok(())
}

fn ofilter_bind_ost(
    index: usize,
    mux: &mut Muxer,
    filter_graph: &mut FilterGraph,
    output_filter_index: usize,
    codec_id: AVCodecID,
    enc: *const AVCodec,
    stream_source: Option<(usize, usize)>,
) -> Result<usize> {
    let output_filter = &mut filter_graph.outputs[output_filter_index];
    let (frame_sender, output_stream_index) =
        mux.add_enc_stream(output_filter.media_type, enc, filter_graph.node.clone())?;
    output_filter.set_dst(frame_sender);

    if let Some((file_idx, stream_idx)) = stream_source {
        mux.register_stream_source(output_stream_index, file_idx, stream_idx, true);
    }

    configure_output_filter_opts(
        index,
        mux,
        output_filter,
        codec_id,
        enc,
        output_stream_index,
    )?;
    Ok(output_stream_index)
}

fn choose_encoder(
    mux: &Muxer,
    media_type: AVMediaType,
) -> Result<Option<(AVCodecID, *const AVCodec)>> {
    let media_codec = match media_type {
        AVMEDIA_TYPE_VIDEO => mux.video_codec.clone(),
        AVMEDIA_TYPE_AUDIO => mux.audio_codec.clone(),
        AVMEDIA_TYPE_SUBTITLE => mux.subtitle_codec.clone(),
        _ => return Ok(None),
    };

    match media_codec {
        None => {
            let url = CString::new(&*mux.url).unwrap();
            unsafe {
                let codec_id = av_guess_codec(
                    (*mux.out_fmt_ctx).oformat,
                    null(),
                    url.as_ptr(),
                    null(),
                    media_type,
                );
                let enc = avcodec_find_encoder(codec_id);
                if enc.is_null() {
                    let format_name = (*(*mux.out_fmt_ctx).oformat).name;
                    let format_name = CStr::from_ptr(format_name).to_str();
                    let codec_name = avcodec_get_name(codec_id);
                    let codec_name = CStr::from_ptr(codec_name).to_str();
                    if let (Ok(format_name), Ok(codec_name)) = (format_name, codec_name) {
                        error!("Automatic encoder selection failed Default encoder for format {format_name} (codec {codec_name}) is probably disabled. Please choose an encoder manually.");
                    }
                    return Err(OpenOutputError::from(AVERROR_ENCODER_NOT_FOUND).into());
                }

                return Ok(Some((codec_id, enc)));
            }
        }
        Some(media_codec) if media_codec != "copy" => unsafe {
            let media_codec_cstr = CString::new(media_codec.clone())?;

            let mut enc = avcodec_find_encoder_by_name(media_codec_cstr.as_ptr());
            let desc = avcodec_descriptor_get_by_name(media_codec_cstr.as_ptr());

            if enc.is_null() && !desc.is_null() {
                enc = avcodec_find_encoder((*desc).id);
                if !enc.is_null() {
                    let codec_name = (*enc).name;
                    let codec_name = CStr::from_ptr(codec_name).to_str();
                    let desc_name = (*desc).name;
                    let desc_name = CStr::from_ptr(desc_name).to_str();
                    if let (Ok(codec_name), Ok(desc_name)) = (codec_name, desc_name) {
                        debug!("Matched encoder '{codec_name}' for codec '{desc_name}'.");
                    }
                }
            }

            if enc.is_null() {
                error!("Unknown encoder '{media_codec}'");
                return Err(OpenOutputError::from(AVERROR_ENCODER_NOT_FOUND).into());
            }

            if (*enc).type_ != media_type {
                error!("Invalid encoder type '{media_codec}'");
                return Err(OpenOutputError::InvalidArgument.into());
            }
            let codec_id = (*enc).id;
            return Ok(Some((codec_id, enc)));
        },
        _ => {}
    };

    Ok(None)
}

fn check_duplicate_inputs_outputs(inputs: &[Input], outputs: &[Output]) -> Result<()> {
    for output in outputs {
        if let Some(output_url) = &output.url {
            for input in inputs {
                if let Some(input_url) = &input.url {
                    if input_url == output_url {
                        return Err(FileSameAsInput(input_url.clone()));
                    }
                }
            }
        }
    }
    Ok(())
}

fn open_output_files(outputs: &mut Vec<Output>, copy_ts: bool) -> Result<Vec<Muxer>> {
    let mut muxs = Vec::new();

    for (i, output) in outputs.iter_mut().enumerate() {
        unsafe {
            let result = open_output_file(i, output, copy_ts);
            if let Err(e) = result {
                free_output_av_format_context(muxs);
                return Err(e);
            }
            let mux = result.unwrap();
            muxs.push(mux)
        }
    }
    Ok(muxs)
}

unsafe fn free_output_av_format_context(muxs: Vec<Muxer>) {
    for mut mux in muxs {
        avformat_close_input(&mut mux.out_fmt_ctx);
    }
}

#[cfg(feature = "docs-rs")]
unsafe fn open_output_file(index: usize, output: &mut Output, copy_ts: bool) -> Result<Muxer> {
    Err(Error::Bug)
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn open_output_file(index: usize, output: &mut Output, copy_ts: bool) -> Result<Muxer> {
    let mut out_fmt_ctx = null_mut();
    let format = get_format(&output.format)?;
    match &output.url {
        None => {
            if output.write_callback.is_none() {
                error!("input url and write_callback is none.");
                return Err(OpenOutputError::InvalidSink.into());
            }

            let write_callback = output.write_callback.take().unwrap();

            let avio_ctx_buffer_size = 1024 * 64;
            let mut avio_ctx_buffer = av_malloc(avio_ctx_buffer_size);
            if avio_ctx_buffer.is_null() {
                return Err(OpenOutputError::OutOfMemory.into());
            }

            let have_seek_callback = output.seek_callback.is_some();
            let input_opaque = Box::new(OutputOpaque {
                write: write_callback,
                seek: output.seek_callback.take(),
            });
            let opaque = Box::into_raw(input_opaque) as *mut libc::c_void;

            let mut avio_ctx = avio_alloc_context(
                avio_ctx_buffer as *mut libc::c_uchar,
                avio_ctx_buffer_size as i32,
                1,
                opaque,
                None,
                Some(write_packet_wrapper),
                if have_seek_callback {
                    Some(seek_packet_wrapper)
                } else {
                    None
                },
            );
            if avio_ctx.is_null() {
                av_freep(&mut avio_ctx_buffer as *mut _ as *mut c_void);
                return Err(OpenOutputError::OutOfMemory.into());
            }

            let ret = avformat_alloc_output_context2(&mut out_fmt_ctx, format, null(), null());
            if out_fmt_ctx.is_null() {
                warn!("Error initializing the muxer for write_callback");
                av_freep(&mut (*avio_ctx).buffer as *mut _ as *mut c_void);
                avio_context_free(&mut avio_ctx);
                return Err(AllocOutputContextError::from(ret).into());
            }

            if !have_seek_callback && output_requires_seek(out_fmt_ctx) {
                av_freep(&mut (*avio_ctx).buffer as *mut _ as *mut c_void);
                avio_context_free(&mut avio_ctx);
                avformat_free_context(out_fmt_ctx);
                warn!("The output format supports seeking, but no seek callback is provided. This may cause issues.");
                return Err(OpenOutputError::SeekFunctionMissing.into());
            }

            (*out_fmt_ctx).pb = avio_ctx;
            (*out_fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO;
        }
        Some(url) => {
            let url_cstr = if url == "-" {
                CString::new("pipe:")?
            } else {
                CString::new(url.as_str())?
            };
            let ret =
                avformat_alloc_output_context2(&mut out_fmt_ctx, format, null(), url_cstr.as_ptr());
            if out_fmt_ctx.is_null() {
                warn!("Error initializing the muxer for {url}");
                return Err(AllocOutputContextError::from(ret).into());
            }

            let output_format = (*out_fmt_ctx).oformat;
            if (*output_format).flags & AVFMT_NOFILE == 0 {
                let ret = avio_open(&mut (*out_fmt_ctx).pb, url_cstr.as_ptr(), AVIO_FLAG_WRITE);
                if ret < 0 {
                    warn!("Error opening output {url}");
                    return Err(OpenOutputError::from(ret).into());
                }
            }
        }
    }

    let recording_time_us = match output.stop_time_us {
        None => output.recording_time_us,
        Some(stop_time_us) => {
            let start_time_us = output.start_time_us.unwrap_or_else(|| 0);
            if stop_time_us <= start_time_us {
                error!("stop_time_us value smaller than start_time_us; aborting.");
                return Err(OpenOutputError::InvalidArgument.into());
            } else {
                Some(stop_time_us - start_time_us)
            }
        }
    };

    let url = output
        .url
        .clone()
        .unwrap_or_else(|| format!("write_callback[{index}]"));

    let video_codec_opts = convert_options(output.video_codec_opts.clone())?;
    let audio_codec_opts = convert_options(output.audio_codec_opts.clone())?;
    let subtitle_codec_opts = convert_options(output.subtitle_codec_opts.clone())?;
    let format_opts = convert_options(output.format_opts.clone())?;

    let mux = Muxer::new(
        url,
        output.url.is_none(),
        out_fmt_ctx,
        output.frame_pipelines.take(),
        output.stream_map_specs.clone(),
        output.stream_maps.clone(),
        output.video_codec.clone(),
        output.audio_codec.clone(),
        output.subtitle_codec.clone(),
        output.start_time_us,
        recording_time_us,
        output.framerate,
        output.vsync_method,
        output.bits_per_raw_sample,
        output.audio_sample_rate,
        output.audio_channels,
        output.audio_sample_fmt,
        output.video_qscale,
        output.audio_qscale,
        output.max_video_frames,
        output.max_audio_frames,
        output.max_subtitle_frames,
        video_codec_opts,
        audio_codec_opts,
        subtitle_codec_opts,
        format_opts,
        copy_ts,
        output.global_metadata.clone(),
        output.stream_metadata.clone(),
        output.chapter_metadata.clone(),
        output.program_metadata.clone(),
        output.metadata_map.clone(),
        output.auto_copy_metadata,
    );

    Ok(mux)
}

fn get_format(format_option: &Option<String>) -> Result<*const AVOutputFormat> {
    match format_option {
        None => Ok(null()),
        Some(format_str) => unsafe {
            let mut format_cstr = CString::new(format_str.to_string())?;
            let mut format = av_guess_format(format_cstr.as_ptr(), null(), null());
            if format.is_null() {
                format_cstr = CString::new(format!("tmp.{format_str}"))?;
                format = av_guess_format(null(), format_cstr.as_ptr(), null());
            }
            if format.is_null() {
                return Err(OpenOutputError::FormatUnsupported(format_str.to_string()).into());
            }
            Ok(format)
        },
    }
}

unsafe fn output_requires_seek(fmt_ctx: *mut AVFormatContext) -> bool {
    if fmt_ctx.is_null() {
        return false;
    }

    let mut format_name = "unknown".to_string();

    if !(*fmt_ctx).oformat.is_null() {
        let oformat = (*fmt_ctx).oformat;
        format_name = CStr::from_ptr((*oformat).name)
            .to_string_lossy()
            .into_owned();
        let flags = (*oformat).flags;
        let no_file = flags & AVFMT_NOFILE as i32 != 0;
        let global_header = flags & AVFMT_GLOBALHEADER as i32 != 0;

        log::debug!(
            "Output format '{format_name}' - No file: {}, Global header: {}",
            if no_file { "True" } else { "False" },
            if global_header { "True" } else { "False" }
        );

        // List of formats that typically require seeking
        let format_names: Vec<&str> = format_name.split(',').collect();
        if format_names
            .iter()
            .any(|&f| matches!(f, "mp4" | "mov" | "mkv" | "avi" | "flac" | "ogg" | "webm"))
        {
            log::debug!("Output format '{format_name}' typically requires seeking.");
            return true;
        }

        // List of streaming formats that do not require seeking
        if format_names.iter().any(|&f| {
            matches!(
                f,
                "mpegts" | "hls" | "m3u8" | "udp" | "rtp" | "rtp_mpegts" | "http" | "srt"
            )
        }) {
            log::debug!("Output format '{format_name}' does not typically require seeking.");
            return false;
        }

        // Special handling for FLV format
        if format_name == "flv" {
            log::debug!("Output format 'flv' detected. It is highly recommended to set `seek_callback()` to avoid potential issues with 'Failed to update header with correct duration' and 'Failed to update header with correct filesize'.");
            return false;
        }

        // If AVFMT_NOFILE is set, the format does not use standard file I/O and may not need seeking
        if no_file {
            log::debug!(
                "Output format '{format_name}' uses AVFMT_NOFILE. Seeking is likely unnecessary."
            );
            return false;
        }

        // If the format uses global headers, it typically means the codec requires a separate metadata section
        if global_header {
            log::debug!(
                "Output format '{format_name}' uses AVFMT_GLOBALHEADER. Seeking may be required."
            );
            return true;
        }
    } else {
        log::debug!("Output format is null. Cannot determine if seeking is required.");
    }

    // Default case: assume seeking is not required
    log::debug!("Output format '{format_name}' does not match any known rules. Assuming seeking is not required.");
    false
}

struct InputOpaque {
    read: Box<dyn FnMut(&mut [u8]) -> i32>,
    seek: Option<Box<dyn FnMut(i64, i32) -> i64>>,
}

#[allow(dead_code)]
struct OutputOpaque {
    write: Box<dyn FnMut(&[u8]) -> i32>,
    seek: Option<Box<dyn FnMut(i64, i32) -> i64>>,
}

unsafe extern "C" fn write_packet_wrapper(
    opaque: *mut libc::c_void,
    buf: *const u8,
    buf_size: libc::c_int,
) -> libc::c_int {
    if buf.is_null() {
        return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO);
    }
    let closure = &mut *(opaque as *mut Box<dyn FnMut(&[u8]) -> i32>);

    let slice = std::slice::from_raw_parts(buf, buf_size as usize);

    (*closure)(slice)
}

unsafe extern "C" fn read_packet_wrapper(
    opaque: *mut libc::c_void,
    buf: *mut u8,
    buf_size: libc::c_int,
) -> libc::c_int {
    if buf.is_null() {
        return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO);
    }

    let context = &mut *(opaque as *mut InputOpaque);

    let slice = std::slice::from_raw_parts_mut(buf, buf_size as usize);

    (context.read)(slice)
}

unsafe extern "C" fn seek_packet_wrapper(
    opaque: *mut libc::c_void,
    offset: i64,
    whence: libc::c_int,
) -> i64 {
    let context = &mut *(opaque as *mut InputOpaque);

    if let Some(seek_func) = &mut context.seek {
        (*seek_func)(offset, whence)
    } else {
        ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE) as i64
    }
}

fn fg_bind_inputs(filter_graphs: &mut Vec<FilterGraph>, demuxs: &mut Vec<Demuxer>) -> Result<()> {
    if filter_graphs.is_empty() {
        return Ok(());
    }
    bind_fg_inputs_by_fg(filter_graphs)?;

    for filter_graph in filter_graphs.iter_mut() {
        for i in 0..filter_graph.inputs.len() {
            fg_complex_bind_input(filter_graph, i, demuxs)?;
        }
    }

    Ok(())
}

struct FilterLabel {
    linklabel: String,
    media_type: AVMediaType,
}

fn bind_fg_inputs_by_fg(filter_graphs: &mut Vec<FilterGraph>) -> Result<()> {
    let fg_labels = filter_graphs
        .iter()
        .map(|filter_graph| {
            let inputs = filter_graph
                .inputs
                .iter()
                .map(|input| FilterLabel {
                    linklabel: input.linklabel.clone(),
                    media_type: input.media_type,
                })
                .collect::<Vec<_>>();
            let outputs = filter_graph
                .outputs
                .iter()
                .map(|output| FilterLabel {
                    linklabel: output.linklabel.clone(),
                    media_type: output.media_type,
                })
                .collect::<Vec<_>>();
            (inputs, outputs)
        })
        .collect::<Vec<_>>();

    for (i, (inputs, _outputs)) in fg_labels.iter().enumerate() {
        for input_filter_label in inputs.iter() {
            if input_filter_label.linklabel.is_empty() {
                continue;
            }

            'outer: for (j, (_inputs, outputs)) in fg_labels.iter().enumerate() {
                if i == j {
                    continue;
                }

                for (output_idx, output_filter_label) in outputs.iter().enumerate() {
                    if output_filter_label.linklabel != input_filter_label.linklabel {
                        continue;
                    }
                    if output_filter_label.media_type != input_filter_label.media_type {
                        warn!(
                            "Tried to connect {:?} output to {:?} input",
                            output_filter_label.media_type, input_filter_label.media_type
                        );
                        return Err(FilterGraphParseError::InvalidArgument.into());
                    }

                    {
                        let filter_graph = &filter_graphs[j];
                        let output_filter = &filter_graph.outputs[output_idx];
                        if output_filter.has_dst() {
                            continue;
                        }
                    }

                    let (sender, finished_flag_list) = {
                        let filter_graph = &mut filter_graphs[i];
                        filter_graph.get_src_sender()
                    };

                    {
                        let filter_graph = &mut filter_graphs[j];
                        filter_graph.outputs[output_idx].set_dst(sender);
                        filter_graph.outputs[output_idx].fg_input_index = i;
                        filter_graph.outputs[output_idx].finished_flag_list = finished_flag_list;
                    }

                    break 'outer;
                }
            }
        }
    }
    Ok(())
}

fn fg_complex_bind_input(
    filter_graph: &mut FilterGraph,
    input_filter_index: usize,
    demuxs: &mut Vec<Demuxer>,
) -> Result<()> {
    let graph_desc = &filter_graph.graph_desc;
    let input_filter = &mut filter_graph.inputs[input_filter_index];
    let (demux_idx, stream_idx) = if !input_filter.linklabel.is_empty()
        && input_filter.linklabel != "in"
    {
        let (demux_idx, stream_idx) = fg_find_input_idx_by_linklabel(
            &input_filter.linklabel,
            input_filter.media_type,
            demuxs,
            graph_desc,
        )?;

        info!(
            "Binding filter input with label '{}' to input stream {stream_idx}:{demux_idx}",
            input_filter.linklabel
        );
        (demux_idx, stream_idx)
    } else {
        let mut demux_idx = -1i32;
        let mut stream_idx = 0;
        for (d_idx, demux) in demuxs.iter().enumerate() {
            for (st_idx, intput_stream) in demux.get_streams().iter().enumerate() {
                if intput_stream.is_used() {
                    continue;
                }
                if intput_stream.codec_type == input_filter.media_type {
                    demux_idx = d_idx as i32;
                    stream_idx = st_idx;
                    break;
                }
            }
            if demux_idx >= 0 {
                break;
            }
        }

        if demux_idx < 0 {
            warn!(
                "Cannot find a matching stream for unlabeled input pad {}",
                input_filter.name
            );
            return Err(FilterGraphParseError::InvalidArgument.into());
        }

        debug!("FilterGraph binding unlabeled input {input_filter_index} to input stream {stream_idx}:{demux_idx}");

        (demux_idx as usize, stream_idx)
    };

    let demux = &mut demuxs[demux_idx];

    ifilter_bind_ist(filter_graph, input_filter_index, stream_idx, demux)
}

#[cfg(feature = "docs-rs")]
fn ifilter_bind_ist(
    filter_graph: &mut FilterGraph,
    input_index: usize,
    stream_idx: usize,
    demux: &mut Demuxer,
) -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "docs-rs"))]
fn ifilter_bind_ist(
    filter_graph: &mut FilterGraph,
    input_index: usize,
    stream_idx: usize,
    demux: &mut Demuxer,
) -> Result<()> {
    unsafe {
        let input_filter = &mut filter_graph.inputs[input_index];
        let ist = *(*demux.in_fmt_ctx).streams.add(stream_idx);
        let par = (*ist).codecpar;
        if (*par).codec_type == AVMEDIA_TYPE_VIDEO {
            let framerate = av_guess_frame_rate(demux.in_fmt_ctx, ist, null_mut());
            input_filter.opts.framerate = framerate;
        } else if (*par).codec_type == AVMEDIA_TYPE_SUBTITLE {
            input_filter.opts.sub2video_width = (*par).width;
            input_filter.opts.sub2video_height = (*par).height;

            if input_filter.opts.sub2video_width <= 0 || input_filter.opts.sub2video_height <= 0 {
                let nb_streams = (*demux.in_fmt_ctx).nb_streams;
                for j in 0..nb_streams {
                    let par1 = (**(*demux.in_fmt_ctx).streams.add(j as usize)).codecpar;
                    if (*par1).codec_type == AVMEDIA_TYPE_VIDEO {
                        input_filter.opts.sub2video_width =
                            std::cmp::max(input_filter.opts.sub2video_width, (*par1).width);
                        input_filter.opts.sub2video_height =
                            std::cmp::max(input_filter.opts.sub2video_height, (*par1).height);
                    }
                }
            }

            if input_filter.opts.sub2video_width <= 0 || input_filter.opts.sub2video_height <= 0 {
                input_filter.opts.sub2video_width =
                    std::cmp::max(input_filter.opts.sub2video_width, 720);
                input_filter.opts.sub2video_height =
                    std::cmp::max(input_filter.opts.sub2video_height, 576);
            }

            demux.get_stream_mut(stream_idx).have_sub2video = true;
        }

        let dec_ctx = {
            let input_stream = demux.get_stream_mut(stream_idx);
            avcodec_alloc_context3(input_stream.codec.as_ptr())
        };
        if dec_ctx.is_null() {
            return Err(FilterGraphParseError::OutOfMemory.into());
        }
        let _codec_ctx = CodecContext::new(dec_ctx);

        let fallback = input_filter.opts.fallback.as_mut_ptr();
        if (*dec_ctx).codec_type == AVMEDIA_TYPE_AUDIO {
            (*fallback).format = (*dec_ctx).sample_fmt as i32;
            (*fallback).sample_rate = (*dec_ctx).sample_rate;

            let ret = av_channel_layout_copy(&mut (*fallback).ch_layout, &(*dec_ctx).ch_layout);
            if ret < 0 {
                return Err(FilterGraphParseError::from(ret).into());
            }
        } else if (*dec_ctx).codec_type == AVMEDIA_TYPE_VIDEO {
            (*fallback).format = (*dec_ctx).pix_fmt as i32;
            (*fallback).width = (*dec_ctx).width;
            (*fallback).height = (*dec_ctx).height;
            (*fallback).sample_aspect_ratio = (*dec_ctx).sample_aspect_ratio;
            (*fallback).colorspace = (*dec_ctx).colorspace;
            (*fallback).color_range = (*dec_ctx).color_range;
        }
        (*fallback).time_base = (*dec_ctx).pkt_timebase;

        //TODO Set this flag according to the input stream parameters
        input_filter.opts.flags |= IFILTER_FLAG_AUTOROTATE;

        let tsoffset = if demux.copy_ts {
            let mut tsoffset = if demux.start_time_us.is_some() {
                demux.start_time_us.unwrap()
            } else {
                0
            };
            if (*demux.in_fmt_ctx).start_time != ffmpeg_sys_next::AV_NOPTS_VALUE {
                tsoffset += (*demux.in_fmt_ctx).start_time
            }
            tsoffset
        } else {
            0
        };
        if demux.start_time_us.is_some() {
            input_filter.opts.trim_start_us = Some(tsoffset);
        }
        input_filter.opts.trim_end_us = demux.recording_time_us;

        let (sender, finished_flag_list) = filter_graph.get_src_sender();
        {
            let input_stream = demux.get_stream_mut(stream_idx);
            input_stream.add_fg_dst(sender, input_index, finished_flag_list);
        };

        let node = Arc::make_mut(&mut filter_graph.node);
        let SchNode::Filter { inputs, .. } = node else {
            unreachable!()
        };
        inputs.insert(input_index, demux.node.clone());

        demux.connect_stream(stream_idx);
        Ok(())
    }
}

/// Find input stream index by filter graph linklabel
/// FFmpeg reference: ffmpeg_filter.c - fg_create logic for parsing filter input specifiers
/// Uses StreamSpecifier for complete stream specifier parsing
fn fg_find_input_idx_by_linklabel(
    linklabel: &str,
    filter_media_type: AVMediaType,
    demuxs: &mut Vec<Demuxer>,
    desc: &str,
) -> Result<(usize, usize)> {
    // Remove brackets if present
    let new_linklabel = if linklabel.starts_with("[") && linklabel.ends_with("]") {
        if linklabel.len() <= 2 {
            warn!("Filter linklabel is empty");
            return Err(InvalidFilterSpecifier(desc.to_string()).into());
        } else {
            &linklabel[1..linklabel.len() - 1]
        }
    } else {
        linklabel
    };

    // Parse file index using strtol (FFmpeg reference: ffmpeg_opt.c:512)
    let (file_idx, remainder) = strtol(new_linklabel).map_err(|_| {
        FilterGraphParseError::InvalidArgument
    })?;

    if file_idx < 0 || file_idx as usize >= demuxs.len() {
        return Err(InvalidFileIndexInFg(file_idx as usize, desc.to_string()).into());
    }
    let file_idx = file_idx as usize;

    // Parse stream specifier using StreamSpecifier
    let spec_str = if remainder.is_empty() {
        // No specifier - will match by media type
        ""
    } else if remainder.starts_with(':') {
        &remainder[1..]
    } else {
        remainder
    };

    let stream_spec = if spec_str.is_empty() {
        // No specifier: create one matching the filter's media type
        let mut spec = StreamSpecifier::default();
        spec.media_type = Some(filter_media_type);
        spec
    } else {
        // Parse the specifier
        StreamSpecifier::parse(spec_str).map_err(|e| {
            warn!("Invalid stream specifier in filter linklabel '{}': {}", linklabel, e);
            FilterGraphParseError::InvalidArgument
        })?
    };

    // Find first matching stream
    let demux = &demuxs[file_idx];
    unsafe {
        let fmt_ctx = demux.in_fmt_ctx;

        for (idx, _) in demux.get_streams().iter().enumerate() {
            let avstream = *(*fmt_ctx).streams.add(idx);

            if stream_spec.matches(fmt_ctx, avstream) {
                // Additional check: must match filter's media type
                let codec_type = (*avstream).codecpar.as_ref().unwrap().codec_type;
                if codec_type == filter_media_type {
                    return Ok((file_idx, idx));
                }
            }
        }
    }

    // No matching stream found
    warn!(
        "Stream specifier '{}' in filtergraph description {} matches no streams.",
        remainder, desc
    );
    Err(FilterGraphParseError::InvalidArgument.into())
}

/// Similar to strtol() in C
/// FFmpeg reference: ffmpeg_opt.c:512 - strtol(arg, &endptr, 0)
/// Used for parsing file indices and other integers in stream specifiers
fn strtol(input: &str) -> Result<(i64, &str)> {
    let mut chars = input.chars().peekable();
    let mut negative = false;

    if let Some(&ch) = chars.peek() {
        if ch == '-' {
            negative = true;
            chars.next();
        } else if !ch.is_digit(10) {
            return Err(ParseInteger);
        }
    }

    let number_start = input.len() - chars.clone().collect::<String>().len();

    let number_str: String = chars.by_ref().take_while(|ch| ch.is_digit(10)).collect();

    if number_str.is_empty() {
        return Err(ParseInteger);
    }

    let number: i64 = number_str.parse().map_err(|_| ParseInteger)?;

    let remainder_index = number_start + number_str.len();
    let remainder = &input[remainder_index..];

    if negative {
        Ok((-number, remainder))
    } else {
        Ok((number, remainder))
    }
}

fn init_filter_graphs(filter_complexs: Vec<FilterComplex>) -> Result<Vec<FilterGraph>> {
    let mut filter_graphs = Vec::with_capacity(filter_complexs.len());
    for (i, filter) in filter_complexs.iter().enumerate() {
        let filter_graph = init_filter_graph(i, &filter.filter_descs, filter.hw_device.clone())?;
        filter_graphs.push(filter_graph);
    }
    Ok(filter_graphs)
}

#[cfg(feature = "docs-rs")]
fn init_filter_graph(
    fg_index: usize,
    filter_desc: &str,
    hw_device: Option<String>,
) -> Result<FilterGraph> {
    Err(Error::Bug)
}

#[cfg(not(feature = "docs-rs"))]
fn init_filter_graph(
    fg_index: usize,
    filter_desc: &str,
    hw_device: Option<String>,
) -> Result<FilterGraph> {
    let desc_cstr = CString::new(filter_desc)?;

    unsafe {
        /* this graph is only used for determining the kinds of inputs
        and outputs we have, and is discarded on exit from this function */
        let mut graph = avfilter_graph_alloc();
        (*graph).nb_threads = 1;

        let mut seg = null_mut();
        let mut ret = avfilter_graph_segment_parse(graph, desc_cstr.as_ptr(), 0, &mut seg);
        if ret < 0 {
            avfilter_graph_free(&mut graph);
            return Err(FilterGraphParseError::from(ret).into());
        }

        ret = avfilter_graph_segment_create_filters(seg, 0);
        if ret < 0 {
            avfilter_graph_free(&mut graph);
            avfilter_graph_segment_free(&mut seg);
            return Err(FilterGraphParseError::from(ret).into());
        }

        #[cfg(not(feature = "docs-rs"))]
        {
            ret = graph_opts_apply(seg);
        }
        if ret < 0 {
            avfilter_graph_segment_free(&mut seg);
            avfilter_graph_free(&mut graph);
            return Err(FilterGraphParseError::from(ret).into());
        }

        let mut inputs = null_mut();
        let mut outputs = null_mut();
        ret = avfilter_graph_segment_apply(seg, 0, &mut inputs, &mut outputs);
        avfilter_graph_segment_free(&mut seg);

        if ret < 0 {
            avfilter_inout_free(&mut inputs);
            avfilter_inout_free(&mut outputs);
            avfilter_graph_free(&mut graph);
            return Err(FilterGraphParseError::from(ret).into());
        }

        let input_filters = inouts_to_input_filters(fg_index, inputs)?;
        let output_filters = inouts_to_output_filters(outputs)?;

        if output_filters.len() == 0 {
            avfilter_inout_free(&mut inputs);
            avfilter_inout_free(&mut outputs);
            avfilter_graph_free(&mut graph);
            return Err(FilterZeroOutputs);
        }

        let filter_graph = FilterGraph::new(
            filter_desc.to_string(),
            hw_device,
            input_filters,
            output_filters,
        );

        avfilter_inout_free(&mut inputs);
        avfilter_inout_free(&mut outputs);
        avfilter_graph_free(&mut graph);

        Ok(filter_graph)
    }
}

unsafe fn inouts_to_input_filters(
    fg_index: usize,
    inouts: *mut AVFilterInOut,
) -> Result<Vec<InputFilter>> {
    let mut cur = inouts;
    let mut filterinouts = Vec::new();
    let mut filter_index = 0;
    while !cur.is_null() {
        let linklabel = if (*cur).name.is_null() {
            ""
        } else {
            let linklabel = CStr::from_ptr((*cur).name);
            let result = linklabel.to_str();
            if let Err(_) = result {
                return Err(FilterDescUtf8);
            }
            result.unwrap()
        };

        let filter_ctx = (*cur).filter_ctx;
        let media_type = avfilter_pad_get_type((*filter_ctx).input_pads, (*cur).pad_idx);

        let pads = (*filter_ctx).input_pads;
        let nb_pads = (*filter_ctx).nb_inputs;

        let name = describe_filter_link(cur, filter_ctx, pads, nb_pads)?;

        let fallback = frame_alloc()?;

        let mut filter = InputFilter::new(linklabel.to_string(), media_type, name, fallback);
        filter.opts.name = format!("fg:{fg_index}:{filter_index}");
        filterinouts.push(filter);

        cur = (*cur).next;
        filter_index += 1;
    }
    Ok(filterinouts)
}

unsafe fn inouts_to_output_filters(inouts: *mut AVFilterInOut) -> Result<Vec<OutputFilter>> {
    let mut cur = inouts;
    let mut output_filters = Vec::new();
    while !cur.is_null() {
        let linklabel = if (*cur).name.is_null() {
            ""
        } else {
            let linklabel = CStr::from_ptr((*cur).name);
            let result = linklabel.to_str();
            if let Err(_) = result {
                return Err(FilterDescUtf8);
            }
            result.unwrap()
        };

        let filter_ctx = (*cur).filter_ctx;
        let media_type = avfilter_pad_get_type((*filter_ctx).output_pads, (*cur).pad_idx);

        let pads = (*filter_ctx).output_pads;
        let nb_pads = (*filter_ctx).nb_outputs;

        let name = describe_filter_link(cur, filter_ctx, pads, nb_pads)?;

        let filter = OutputFilter::new(linklabel.to_string(), media_type, name);
        output_filters.push(filter);

        cur = (*cur).next;
    }
    Ok(output_filters)
}

unsafe fn describe_filter_link(
    cur: *mut AVFilterInOut,
    filter_ctx: *mut AVFilterContext,
    pads: *mut AVFilterPad,
    nb_pads: c_uint,
) -> Result<String> {
    let filter = (*filter_ctx).filter;
    let name = (*filter).name;
    let name = CStr::from_ptr(name);
    let result = name.to_str();
    if let Err(_) = result {
        return Err(FilterNameUtf8);
    }
    let name = result.unwrap();

    let name = if nb_pads > 1 {
        name.to_string()
    } else {
        let pad_name = avfilter_pad_get_name(pads, (*cur).pad_idx);
        let pad_name = CStr::from_ptr(pad_name);
        let result = pad_name.to_str();
        if let Err(_) = result {
            return Err(FilterNameUtf8);
        }
        let pad_name = result.unwrap();
        format!("{name}:{pad_name}")
    };
    Ok(name)
}

fn open_input_files(inputs: &mut Vec<Input>, copy_ts: bool) -> Result<Vec<Demuxer>> {
    let mut demuxs = Vec::new();
    for (i, input) in inputs.iter_mut().enumerate() {
        unsafe {
            let result = open_input_file(i, input, copy_ts);
            if let Err(e) = result {
                free_input_av_format_context(demuxs);
                return Err(e);
            }
            let demux = result.unwrap();
            demuxs.push(demux)
        }
    }
    Ok(demuxs)
}

unsafe fn free_input_av_format_context(demuxs: Vec<Demuxer>) {
    for mut demux in demuxs {
        avformat_close_input(&mut demux.in_fmt_ctx);
    }
}

#[cfg(feature = "docs-rs")]
unsafe fn open_input_file(index: usize, input: &mut Input, copy_ts: bool) -> Result<Demuxer> {
    Err(Error::Bug)
}

#[cfg(not(feature = "docs-rs"))]
unsafe fn open_input_file(index: usize, input: &mut Input, copy_ts: bool) -> Result<Demuxer> {
    let mut in_fmt_ctx = avformat_alloc_context();
    if in_fmt_ctx.is_null() {
        return Err(OpenInputError::OutOfMemory.into());
    }

    let recording_time_us = match input.stop_time_us {
        None => input.recording_time_us,
        Some(stop_time_us) => {
            let start_time_us = input.start_time_us.unwrap_or_else(|| 0);
            if stop_time_us <= start_time_us {
                error!("stop_time_us value smaller than start_time_us; aborting.");
                return Err(OpenOutputError::InvalidArgument.into());
            } else {
                Some(stop_time_us - start_time_us)
            }
        }
    };

    let file_iformat = if let Some(format) = &input.format {
        let format_cstr = CString::new(format.clone())?;

        let file_iformat = ffmpeg_sys_next::av_find_input_format(format_cstr.as_ptr());
        if file_iformat.is_null() {
            error!("Unknown input format: '{format}'");
            return Err(OpenInputError::InvalidFormat(format.clone()).into());
        }
        file_iformat
    } else {
        null()
    };

    let input_opts = convert_options(input.input_opts.clone())?;
    let mut input_opts = hashmap_to_avdictionary(&input_opts);

    match &input.url {
        None => {
            if input.read_callback.is_none() {
                error!("input url and read_callback is none.");
                return Err(OpenInputError::InvalidSource.into());
            }

            let avio_ctx_buffer_size = 1024 * 64;
            let mut avio_ctx_buffer = av_malloc(avio_ctx_buffer_size);
            if avio_ctx_buffer.is_null() {
                avformat_close_input(&mut in_fmt_ctx);
                return Err(OpenInputError::OutOfMemory.into());
            }

            let have_seek_callback = input.seek_callback.is_some();
            let input_opaque = Box::new(InputOpaque {
                read: input.read_callback.take().unwrap(),
                seek: input.seek_callback.take(),
            });
            let opaque = Box::into_raw(input_opaque) as *mut libc::c_void;

            let mut avio_ctx = avio_alloc_context(
                avio_ctx_buffer as *mut libc::c_uchar,
                avio_ctx_buffer_size as i32,
                0,
                opaque,
                Some(read_packet_wrapper),
                None,
                if have_seek_callback {
                    Some(seek_packet_wrapper)
                } else {
                    None
                },
            );
            if avio_ctx.is_null() {
                av_freep(&mut avio_ctx_buffer as *mut _ as *mut c_void);
                avformat_close_input(&mut in_fmt_ctx);
                return Err(OpenInputError::OutOfMemory.into());
            }

            (*in_fmt_ctx).pb = avio_ctx;
            (*in_fmt_ctx).flags = AVFMT_FLAG_CUSTOM_IO;

            let ret = avformat_open_input(&mut in_fmt_ctx, null(), file_iformat, &mut input_opts);
            if ret < 0 {
                av_freep(&mut (*avio_ctx).buffer as *mut _ as *mut c_void);
                avio_context_free(&mut avio_ctx);
                avformat_close_input(&mut in_fmt_ctx);
                return Err(OpenInputError::from(ret).into());
            }

            let ret = avformat_find_stream_info(in_fmt_ctx, null_mut());
            if ret < 0 {
                av_freep(&mut (*avio_ctx).buffer as *mut _ as *mut c_void);
                avio_context_free(&mut avio_ctx);
                avformat_close_input(&mut in_fmt_ctx);
                return Err(FindStreamError::from(ret).into());
            }

            if !have_seek_callback && input_requires_seek(in_fmt_ctx) {
                av_freep(&mut (*avio_ctx).buffer as *mut _ as *mut c_void);
                avio_context_free(&mut avio_ctx);
                avformat_close_input(&mut in_fmt_ctx);
                warn!("The input format supports seeking, but no seek callback is provided. This may cause issues.");
                return Err(OpenInputError::SeekFunctionMissing.into());
            }
        }
        Some(url) => {
            let url_cstr = CString::new(url.as_str())?;

            let scan_all_pmts_key = CString::new("scan_all_pmts")?;
            if ffmpeg_sys_next::av_dict_get(
                input_opts,
                scan_all_pmts_key.as_ptr(),
                null(),
                ffmpeg_sys_next::AV_DICT_MATCH_CASE,
            )
            .is_null()
            {
                let scan_all_pmts_value = CString::new("1")?;
                ffmpeg_sys_next::av_dict_set(
                    &mut input_opts,
                    scan_all_pmts_key.as_ptr(),
                    scan_all_pmts_value.as_ptr(),
                    ffmpeg_sys_next::AV_DICT_DONT_OVERWRITE,
                );
            };
            (*in_fmt_ctx).flags |= ffmpeg_sys_next::AVFMT_FLAG_NONBLOCK;

            let mut ret = avformat_open_input(
                &mut in_fmt_ctx,
                url_cstr.as_ptr(),
                file_iformat,
                &mut input_opts,
            );
            av_dict_free(&mut input_opts);
            if ret < 0 {
                avformat_close_input(&mut in_fmt_ctx);
                return Err(OpenInputError::from(ret).into());
            }

            ret = avformat_find_stream_info(in_fmt_ctx, null_mut());
            if ret < 0 {
                avformat_close_input(&mut in_fmt_ctx);
                return Err(FindStreamError::from(ret).into());
            }
        }
    }

    let mut timestamp = input.start_time_us.unwrap_or(0);
    /* add the stream start time */
    if (*in_fmt_ctx).start_time != ffmpeg_sys_next::AV_NOPTS_VALUE {
        timestamp += (*in_fmt_ctx).start_time;
    }

    /* if seeking requested, we execute it */
    if let Some(start_time_us) = input.start_time_us {
        let mut seek_timestamp = timestamp;

        if (*(*in_fmt_ctx).iformat).flags & ffmpeg_sys_next::AVFMT_SEEK_TO_PTS == 0 {
            let mut dts_heuristic = false;
            let stream_count = (*in_fmt_ctx).nb_streams;

            for i in 0..stream_count {
                let stream = *(*in_fmt_ctx).streams.add(i as usize);
                let par = (*stream).codecpar;
                if (*par).video_delay != 0 {
                    dts_heuristic = true;
                    break;
                }
            }
            if dts_heuristic {
                seek_timestamp -= 3 * AV_TIME_BASE as i64 / 23;
            }
        }
        let ret = ffmpeg_sys_next::avformat_seek_file(
            in_fmt_ctx,
            -1,
            i64::MIN,
            seek_timestamp,
            seek_timestamp,
            0,
        );
        if ret < 0 {
            warn!(
                "could not seek to position {:.3}",
                start_time_us as f64 / AV_TIME_BASE as f64
            );
        }
    }

    let url = input
        .url
        .clone()
        .unwrap_or_else(|| format!("read_callback[{index}]"));

    let demux = Demuxer::new(
        url,
        input.url.is_none(),
        in_fmt_ctx,
        0 - if copy_ts { 0 } else { timestamp },
        input.frame_pipelines.take(),
        input.video_codec.clone(),
        input.audio_codec.clone(),
        input.subtitle_codec.clone(),
        input.readrate,
        input.start_time_us,
        recording_time_us,
        input.exit_on_error,
        input.stream_loop,
        input.hwaccel.clone(),
        input.hwaccel_device.clone(),
        input.hwaccel_output_format.clone(),
        copy_ts,
    )?;

    Ok(demux)
}

fn convert_options(
    opts: Option<HashMap<String, String>>,
) -> Result<Option<HashMap<CString, CString>>> {
    if opts.is_none() {
        return Ok(None);
    }

    let converted = opts.map(|map| {
        map.into_iter()
            .map(|(k, v)| Ok((CString::new(k)?, CString::new(v)?)))
            .collect::<Result<HashMap<CString, CString>, _>>() // Collect into a HashMap
    });

    converted.transpose() // Convert `Result<Option<T>>` into `Option<Result<T>>`
}

unsafe fn input_requires_seek(fmt_ctx: *mut AVFormatContext) -> bool {
    if fmt_ctx.is_null() {
        return false;
    }

    let mut format_name = "unknown".to_string();
    let mut format_names: Vec<&str> = Vec::with_capacity(0);

    if !(*fmt_ctx).iformat.is_null() {
        let iformat = (*fmt_ctx).iformat;
        format_name = CStr::from_ptr((*iformat).name)
            .to_string_lossy()
            .into_owned();
        let flags = (*iformat).flags;
        let no_binsearch = flags & AVFMT_NOBINSEARCH as i32 != 0;
        let no_gensearch = flags & AVFMT_NOGENSEARCH as i32 != 0;

        log::debug!(
            "Input format '{format_name}' - Binary search: {}, Generic search: {}",
            if no_binsearch { "Disabled" } else { "Enabled" },
            if no_gensearch { "Disabled" } else { "Enabled" }
        );

        format_names = format_name.split(',').collect();

        if format_names.iter().any(|&f| {
            matches!(
                f,
                "mp4" | "mkv" | "avi" | "mov" | "flac" | "wav" | "aac" | "ogg" | "mp3" | "webm"
            )
        }) {
            if !no_binsearch && !no_gensearch {
                return true;
            }
        }

        if format_names.iter().any(|&f| {
            matches!(
                f,
                "hls" | "m3u8" | "mpegts" | "mms" | "udp" | "rtp" | "rtp_mpegts" | "http" | "srt"
            )
        }) {
            log::debug!("Live stream detected ({format_name}). Seeking is not possible.");
            return false;
        }

        if no_binsearch && no_gensearch {
            log::debug!("Input format '{format_name}' has both NOBINSEARCH and NOGENSEARCH set. Seeking is likely restricted.");
        }
    }

    let format_duration = (*fmt_ctx).duration;

    if format_names.iter().any(|&f| f == "flv") {
        if format_duration <= 0 {
            log::debug!(
                "Input format 'flv' detected with no valid duration. Seeking is not possible."
            );
        } else {
            log::warn!("Input format 'flv' detected with a valid duration. While seeking may still be possible, it is highly recommended to add a `seek_callback()` for optimal input handling, especially when seeking or random access to specific segments is required.");
        }
        return false;
    }

    if format_duration > 0 {
        log::debug!("Format '{format_name}' has a duration of {format_duration}. Seeking is likely possible.");
        return true;
    }

    let mut video_stream_index = -1;
    for i in 0..(*fmt_ctx).nb_streams {
        let stream = *(*fmt_ctx).streams.offset(i as isize);
        if (*stream).codecpar.is_null() {
            continue;
        }
        if (*(*stream).codecpar).codec_type == AVMEDIA_TYPE_VIDEO {
            video_stream_index = i as i32;
            break;
        }
    }

    let stream_index = if video_stream_index >= 0 {
        video_stream_index
    } else {
        -1
    };

    let original_pos = if !(*fmt_ctx).pb.is_null() {
        (*(*fmt_ctx).pb).pos
    } else {
        -1
    };

    if original_pos >= 0 {
        let seek_target = 1 * AV_TIME_BASE as i64;
        let seek_result = av_seek_frame(fmt_ctx, stream_index, seek_target, AVSEEK_FLAG_BACKWARD);

        if seek_result >= 0 {
            log::debug!("Seek test successful.");

            (*(*fmt_ctx).pb).pos = original_pos;
            avformat_flush(fmt_ctx);
            log::debug!("Restored fmt_ctx.pb.pos to {original_pos} and flushed format context.",);
            return true;
        } else {
            log::debug!("Seek test failed (return code {seek_result}). This format likely does not support seeking.");
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};
    use std::ptr::null_mut;

    use crate::core::context::ffmpeg_context::{strtol, FfmpegContext, Output};
    use ffmpeg_sys_next::{
        avfilter_graph_alloc, avfilter_graph_free, avfilter_graph_parse_ptr, avfilter_inout_free,
    };

    #[test]
    fn test_filter() {
        let desc_cstr = CString::new("[1:v][2:v]concat=n=2:v=1:a=0[vout]").unwrap();
        // let desc_cstr = CString::new("fps=15").unwrap();

        unsafe {
            let mut graph = avfilter_graph_alloc();
            let mut inputs = null_mut();
            let mut outputs = null_mut();

            let ret = avfilter_graph_parse_ptr(
                graph,
                desc_cstr.as_ptr(),
                &mut inputs,
                &mut outputs,
                null_mut(),
            );
            if ret < 0 {
                avfilter_inout_free(&mut inputs);
                avfilter_inout_free(&mut outputs);
                avfilter_graph_free(&mut graph);
                println!("err ret:{}", crate::util::ffmpeg_utils::av_err2str(ret));
                return;
            }

            println!("inputs.is_null:{}", inputs.is_null());
            println!("outputs.is_null:{}", outputs.is_null());

            let mut cur = inputs;
            while !cur.is_null() {
                let input_name = CStr::from_ptr((*cur).name);
                println!("Input name: {}", input_name.to_str().unwrap());
                cur = (*cur).next;
            }

            let output_name = CStr::from_ptr((*outputs).name);
            println!("Output name: {}", output_name.to_str().unwrap());

            let filter_ctx = (*outputs).filter_ctx;
            avfilter_inout_free(&mut outputs);
            println!("filter_ctx.is_null:{}", filter_ctx.is_null());
        }
    }

    #[test]
    fn test_new() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .is_test(true)
            .try_init();
        let _ffmpeg_context = FfmpegContext::new(
            vec!["test.mp4".to_string().into()],
            vec!["hue=s=0".to_string().into()],
            vec!["output.mp4".to_string().into()],
        )
        .unwrap();
        let _ffmpeg_context = FfmpegContext::new(
            vec!["test.mp4".into()],
            vec!["[0:v]hue=s=0".into()],
            vec!["output.mp4".to_string().into()],
        )
        .unwrap();
        let _ffmpeg_context = FfmpegContext::new(
            vec!["test.mp4".into()],
            vec!["hue=s=0[my-out]".into()],
            vec![Output::from("output.mp4").add_stream_map("my-out")],
        )
        .unwrap();
        let result = FfmpegContext::new(
            vec!["test.mp4".into()],
            vec!["hue=s=0".into()],
            vec![Output::from("output.mp4").add_stream_map("0:v?")],
        );
        assert!(result.is_err());
        let result = FfmpegContext::new(
            vec!["test.mp4".into()],
            vec!["hue=s=0".into()],
            vec![Output::from("output.mp4").add_stream_map_with_copy("1:v?")],
        );
        assert!(result.is_err());
        let result = FfmpegContext::new(
            vec!["test.mp4".into()],
            vec!["hue=s=0[fg-out]".into()],
            vec![
                Output::from("output.mp4").add_stream_map("my-out?"),
                Output::from("output.mp4").add_stream_map("fg-out"),
            ],
        );
        assert!(result.is_err());
        // ignore filter
        let result = FfmpegContext::new(
            vec!["test.mp4".into()],
            vec!["hue=s=0".into()],
            vec![Output::from("output.mp4").add_stream_map_with_copy("1:v")],
        );
        assert!(result.is_err());
        let result = FfmpegContext::new(
            vec!["test.mp4".into()],
            vec!["hue=s=0[fg-out]".into()],
            vec![Output::from("output.mp4").add_stream_map("fg-out?")],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_builder() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .is_test(true)
            .try_init();

        let _context1 = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output("output.mp4")
            .build()
            .unwrap();

        let _context2 = FfmpegContext::builder()
            .inputs(vec!["test.mp4"])
            .filter_descs(vec!["hue=s=0"])
            .outputs(vec!["output.mp4"])
            .build()
            .unwrap();
    }

    #[test]
    fn test_strtol() {
        let input = "-123---abc";
        let result = strtol(input);
        assert_eq!(result.unwrap(), (-123, "---abc"));

        let input = "123---abc";
        let result = strtol(input);
        assert_eq!(result.unwrap(), (123, "---abc"));

        let input = "-123aa";
        let result = strtol(input);
        assert_eq!(result.unwrap(), (-123, "aa"));

        let input = "-aa";
        let result = strtol(input);
        assert!(result.is_err());

        let input = "abc";
        let result = strtol(input);
        assert!(result.is_err())
    }
}
