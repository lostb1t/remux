// Metadata operations - writing and copying
// Based on FFmpeg ffmpeg_mux_init.c

use super::metadata_type::MetadataType;
use super::stream_specifier::StreamSpecifier;
use ffmpeg_sys_next::{
    av_dict_copy, av_dict_set, AVDictionary, AVFormatContext, AV_DICT_DONT_OVERWRITE,
};
use std::ffi::CString;

/// Add metadata to output file
///
/// FFmpeg reference: ffmpeg_mux_init.c:2726 of_add_metadata
///
/// # Safety
/// This function is unsafe because it dereferences raw FFmpeg pointers.
/// The caller must ensure that:
/// - `oc` is a valid pointer to an initialized AVFormatContext
/// - The AVFormatContext remains valid for the duration of this call
/// - All streams, chapters, and programs in the context are properly initialized
pub unsafe fn of_add_metadata(
    oc: *mut AVFormatContext,
    global_metadata: &Option<std::collections::HashMap<String, String>>,
    stream_metadata: &[(String, String, String)], // (spec, key, value) tuples
    chapter_metadata: &std::collections::HashMap<usize, std::collections::HashMap<String, String>>,
    program_metadata: &std::collections::HashMap<usize, std::collections::HashMap<String, String>>,
) -> Result<(), String> {
    if oc.is_null() {
        return Err("AVFormatContext is null".to_string());
    }

    let oc_ref = &mut *oc;

    // 1. Add global metadata
    if let Some(metadata) = global_metadata {
        for (key, value) in metadata {
            let c_key =
                CString::new(key.as_str()).map_err(|e| format!("Invalid key '{}': {}", key, e))?;

            let result = if value.is_empty() {
                // Empty value means delete the key (FFmpeg behavior)
                av_dict_set(&mut oc_ref.metadata, c_key.as_ptr(), std::ptr::null(), 0)
            } else {
                let c_value = CString::new(value.as_str())
                    .map_err(|e| format!("Invalid value for key '{}': {}", key, e))?;
                av_dict_set(&mut oc_ref.metadata, c_key.as_ptr(), c_value.as_ptr(), 0)
            };

            if result < 0 {
                log::warn!(
                    "Failed to set global metadata key '{}': error code {}",
                    key,
                    result
                );
            }
        }
    }

    // 2. Add stream metadata
    // For each (spec, key, value) tuple, match against all streams and apply
    for (spec_str, key, value) in stream_metadata {
        // Parse stream specifier
        let specifier = StreamSpecifier::parse(spec_str)
            .map_err(|e| format!("Invalid stream specifier '{}': {}", spec_str, e))?;

        // Iterate through all streams and apply to matching ones
        let streams = std::slice::from_raw_parts(oc_ref.streams, oc_ref.nb_streams as usize);

        for stream_ptr in streams {
            if stream_ptr.is_null() {
                continue;
            }

            // Check if this stream matches the specifier
            if specifier.matches(oc, *stream_ptr) {
                let stream_ref = &mut **stream_ptr;

                let c_key = CString::new(key.as_str())
                    .map_err(|e| format!("Invalid key '{}': {}", key, e))?;

                let result = if value.is_empty() {
                    // Empty value means delete the key
                    av_dict_set(
                        &mut stream_ref.metadata,
                        c_key.as_ptr(),
                        std::ptr::null(),
                        0,
                    )
                } else {
                    let c_value = CString::new(value.as_str())
                        .map_err(|e| format!("Invalid value for key '{}': {}", key, e))?;
                    av_dict_set(
                        &mut stream_ref.metadata,
                        c_key.as_ptr(),
                        c_value.as_ptr(),
                        0,
                    )
                };

                if result < 0 {
                    log::warn!(
                        "Failed to set stream metadata key '{}': error code {}",
                        key,
                        result
                    );
                }
            }
        }
    }

    // 3. Add chapter metadata
    for (chapter_idx, metadata) in chapter_metadata {
        let chapter_idx = *chapter_idx;

        if chapter_idx >= oc_ref.nb_chapters as usize {
            log::warn!(
                "Invalid chapter index {}: only {} chapters exist",
                chapter_idx,
                oc_ref.nb_chapters
            );
            continue;
        }

        let chapters = std::slice::from_raw_parts(oc_ref.chapters, oc_ref.nb_chapters as usize);
        let chapter_ptr = chapters[chapter_idx];

        if chapter_ptr.is_null() {
            log::warn!("Chapter {} pointer is null", chapter_idx);
            continue;
        }

        let chapter_ref = &mut *chapter_ptr;

        for (key, value) in metadata {
            let c_key =
                CString::new(key.as_str()).map_err(|e| format!("Invalid key '{}': {}", key, e))?;

            let result = if value.is_empty() {
                av_dict_set(
                    &mut chapter_ref.metadata,
                    c_key.as_ptr(),
                    std::ptr::null(),
                    0,
                )
            } else {
                let c_value = CString::new(value.as_str())
                    .map_err(|e| format!("Invalid value for key '{}': {}", key, e))?;
                av_dict_set(
                    &mut chapter_ref.metadata,
                    c_key.as_ptr(),
                    c_value.as_ptr(),
                    0,
                )
            };

            if result < 0 {
                log::warn!(
                    "Failed to set chapter {} metadata key '{}': error code {}",
                    chapter_idx,
                    key,
                    result
                );
            }
        }
    }

    // 4. Add program metadata
    for (program_idx, metadata) in program_metadata {
        let program_idx = *program_idx;

        if program_idx >= oc_ref.nb_programs as usize {
            log::warn!(
                "Invalid program index {}: only {} programs exist",
                program_idx,
                oc_ref.nb_programs
            );
            continue;
        }

        let programs = std::slice::from_raw_parts(oc_ref.programs, oc_ref.nb_programs as usize);
        let program_ptr = programs[program_idx];

        if program_ptr.is_null() {
            log::warn!("Program {} pointer is null", program_idx);
            continue;
        }

        let program_ref = &mut *program_ptr;

        for (key, value) in metadata {
            let c_key =
                CString::new(key.as_str()).map_err(|e| format!("Invalid key '{}': {}", key, e))?;

            let result = if value.is_empty() {
                av_dict_set(
                    &mut program_ref.metadata,
                    c_key.as_ptr(),
                    std::ptr::null(),
                    0,
                )
            } else {
                let c_value = CString::new(value.as_str())
                    .map_err(|e| format!("Invalid value for key '{}': {}", key, e))?;
                av_dict_set(
                    &mut program_ref.metadata,
                    c_key.as_ptr(),
                    c_value.as_ptr(),
                    0,
                )
            };

            if result < 0 {
                log::warn!(
                    "Failed to set program {} metadata key '{}': error code {}",
                    program_idx,
                    key,
                    result
                );
            }
        }
    }

    Ok(())
}

/// Copy metadata from input to output based on metadata mapping
///
/// FFmpeg reference: ffmpeg_mux_init.c:2826 copy_metadata
///
/// # Safety
/// This function is unsafe because it dereferences raw FFmpeg pointers.
/// The caller must ensure that:
/// - `input_ctx` and `output_ctx` are valid pointers to initialized AVFormatContext structures
/// - Both contexts remain valid for the duration of this call
/// - All streams, chapters, and programs are properly initialized
pub unsafe fn copy_metadata(
    input_ctx: *const AVFormatContext,
    output_ctx: *mut AVFormatContext,
    src_type: &MetadataType,
    dst_type: &MetadataType,
) -> Result<(), String> {
    if input_ctx.is_null() {
        return Err("Input AVFormatContext is null".to_string());
    }
    if output_ctx.is_null() {
        return Err("Output AVFormatContext is null".to_string());
    }

    let input_ref = &*input_ctx;
    let output_ref = &mut *output_ctx;

    // Get source metadata dictionary pointer
    let src_metadata_ptr = match src_type {
        MetadataType::Global => &input_ref.metadata as *const *mut AVDictionary,

        MetadataType::Stream(specifier) => {
            // Find first matching input stream
            let streams =
                std::slice::from_raw_parts(input_ref.streams, input_ref.nb_streams as usize);
            let mut found_ptr: Option<*const *mut AVDictionary> = None;

            for stream_ptr in streams {
                if !stream_ptr.is_null() && specifier.matches(input_ctx, *stream_ptr) {
                    found_ptr = Some(&(**stream_ptr).metadata as *const *mut AVDictionary);
                    break;
                }
            }

            match found_ptr {
                Some(ptr) => ptr,
                None => {
                    log::warn!("No matching input stream found for specifier");
                    return Ok(()); // Not an error, just no match
                }
            }
        }

        MetadataType::Chapter(idx) => {
            let idx = *idx;
            if idx >= input_ref.nb_chapters as usize {
                return Err(format!(
                    "Invalid source chapter index {}: only {} chapters exist",
                    idx, input_ref.nb_chapters
                ));
            }
            let chapters =
                std::slice::from_raw_parts(input_ref.chapters, input_ref.nb_chapters as usize);
            &(*chapters[idx]).metadata as *const *mut AVDictionary
        }

        MetadataType::Program(idx) => {
            let idx = *idx;
            if idx >= input_ref.nb_programs as usize {
                return Err(format!(
                    "Invalid source program index {}: only {} programs exist",
                    idx, input_ref.nb_programs
                ));
            }
            let programs =
                std::slice::from_raw_parts(input_ref.programs, input_ref.nb_programs as usize);
            &(*programs[idx]).metadata as *const *mut AVDictionary
        }
    };

    // Get source metadata
    let src_dict = *src_metadata_ptr;

    // Copy to destination based on type
    match dst_type {
        MetadataType::Global => {
            av_dict_copy(
                &mut output_ref.metadata,
                src_dict,
                AV_DICT_DONT_OVERWRITE as i32,
            );
        }

        MetadataType::Stream(specifier) => {
            // Apply to all matching output streams
            let streams =
                std::slice::from_raw_parts_mut(output_ref.streams, output_ref.nb_streams as usize);

            for stream_ptr in streams {
                if !stream_ptr.is_null() && specifier.matches(output_ctx, *stream_ptr) {
                    let stream_ref = &mut **stream_ptr;
                    av_dict_copy(
                        &mut stream_ref.metadata,
                        src_dict,
                        AV_DICT_DONT_OVERWRITE as i32,
                    );
                }
            }
        }

        MetadataType::Chapter(idx) => {
            let idx = *idx;
            if idx >= output_ref.nb_chapters as usize {
                return Err(format!(
                    "Invalid destination chapter index {}: only {} chapters exist",
                    idx, output_ref.nb_chapters
                ));
            }
            let chapters =
                std::slice::from_raw_parts(output_ref.chapters, output_ref.nb_chapters as usize);
            let chapter_ref = &mut *chapters[idx];
            av_dict_copy(
                &mut chapter_ref.metadata,
                src_dict,
                AV_DICT_DONT_OVERWRITE as i32,
            );
        }

        MetadataType::Program(idx) => {
            let idx = *idx;
            if idx >= output_ref.nb_programs as usize {
                return Err(format!(
                    "Invalid destination program index {}: only {} programs exist",
                    idx, output_ref.nb_programs
                ));
            }
            let programs =
                std::slice::from_raw_parts(output_ref.programs, output_ref.nb_programs as usize);
            let program_ref = &mut *programs[idx];
            av_dict_copy(
                &mut program_ref.metadata,
                src_dict,
                AV_DICT_DONT_OVERWRITE as i32,
            );
        }
    }

    Ok(())
}
