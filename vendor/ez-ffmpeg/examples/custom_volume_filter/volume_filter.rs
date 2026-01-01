use ffmpeg_next::Frame;
use ffmpeg_sys_next::{av_frame_copy_props, av_frame_get_buffer, AVMediaType};
use ez_ffmpeg::filter::frame_filter::FrameFilter;
use ez_ffmpeg::filter::frame_filter_context::FrameFilterContext;

pub struct VolumeFilter {
    volume: f32, // The volume gain factor (e.g., 1.0 for no change, 2.0 for doubling the volume)
    resampler: Option<ffmpeg_next::software::resampling::Context>, // Optional resampler if format/channel layout needs to be changed
}

impl VolumeFilter {
    pub fn new(volume: f32) -> Self {
        Self { volume, resampler: None }
    }
}

impl FrameFilter for VolumeFilter {
    fn media_type(&self) -> AVMediaType {
        AVMediaType::AVMEDIA_TYPE_AUDIO // This filter operates on audio frames
    }

    fn filter_frame(
        &mut self,
        mut frame: Frame, // The input audio frame to be processed
        _ctx: &FrameFilterContext, // Context of the filter (not used here)
    ) -> Result<Option<Frame>, String> {
        unsafe {
            // Ensure the frame is valid and not empty
            if frame.as_ptr().is_null() || frame.is_empty() {
                return Ok(Some(frame)); // If invalid, simply return the frame as-is
            }
        }

        // Extract audio format, channel layout, sample rate, and number of samples from the frame
        let format: ffmpeg_sys_next::AVSampleFormat =
            unsafe { std::mem::transmute((*frame.as_ptr()).format) };
        let ch_layout = unsafe { (*frame.as_ptr()).ch_layout };
        let sample_rate = unsafe { (*frame.as_ptr()).sample_rate };
        let nb_samples = unsafe { (*frame.as_ptr()).nb_samples };
        let nb_channels = ch_layout.nb_channels;

        // If the audio format and layout do not match expected format, initialize resampler
        if format == ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_FLTP && nb_channels == 2 {
            self.resampler = None; // No resampling needed for stereo float planar format
        } else {
            let resampler = ffmpeg_next::software::resampling::Context::get(
                format.into(),
                ch_layout.into(),
                sample_rate as u32,
                ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_FLTP.into(),
                ffmpeg_next::util::channel_layout::ChannelLayout::STEREO,
                sample_rate as u32,
            )
                .map_err(|e| format!("failed to create resampler: {:?}", e))?;
            self.resampler = Some(resampler);
        }

        // Resample frame if necessary
        let frame = match self.resampler.as_mut() {
            None => frame,
            Some(resampler) => {
                let mut resample_frame = unsafe { Frame::empty() };
                unsafe {
                    if resample_frame.as_ptr().is_null() {
                        return Err("failed to create resample_frame frame: Out of memory.".to_string());
                    }

                    let mut ret = av_frame_copy_props(resample_frame.as_mut_ptr(), frame.as_ptr());
                    if ret < 0 {
                        return Err(format!("failed to copy properties. ret:{}", av_err2str(ret)));
                    }

                    (*resample_frame.as_mut_ptr()).sample_rate = sample_rate;
                    (*resample_frame.as_mut_ptr()).format = ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_FLTP as i32;
                    (*resample_frame.as_mut_ptr()).nb_samples = nb_samples;
                    (*resample_frame.as_mut_ptr()).ch_layout = ffmpeg_next::util::channel_layout::ChannelLayout::STEREO.into();

                    ret = av_frame_get_buffer(resample_frame.as_mut_ptr(), 0);
                    if ret < 0 {
                        return Err(format!("failed to allocate buffer for resample_frame. {}", av_err2str(ret)));
                    }

                    // Perform resampling
                    ret = ffmpeg_sys_next::swr_convert(
                        resampler.as_mut_ptr(),
                        (*resample_frame.as_mut_ptr()).data.as_ptr(),
                        (*resample_frame.as_ptr()).nb_samples,
                        (*frame.as_ptr()).data.as_ptr() as *const *const _,
                        (*frame.as_ptr()).nb_samples,
                    );
                    if ret < 0 {
                        return Err(format!(
                            "failed to resample frame[format:{}] to dst_frame[format:{}]: {}",
                            (*frame.as_ptr()).format,
                            (*resample_frame.as_ptr()).format,
                            ez_ffmpeg::util::ffmpeg_utils::av_err2str(ret)
                        ));
                    }
                    resample_frame
                }
            }
        };

        // Apply volume adjustment on both channels
        let input_channel_0 = plane(&frame, 0);
        let output_channel_0 = plane_mut(&frame, 0);
        adjust_volume_f32(output_channel_0, input_channel_0, self.volume);

        let input_channel_1 = plane(&frame, 1);
        let output_channel_1 = plane_mut(&frame, 1);
        adjust_volume_f32(output_channel_1, input_channel_1, self.volume);

        Ok(Some(frame)) // Return the processed frame
    }
}

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use std::arch::x86_64::*;
use ez_ffmpeg::util::ffmpeg_utils::av_err2str;

// SIMD-based volume adjustment
#[inline]
pub fn adjust_volume_f32(output: &mut [f32], input: &[f32], gain: f32) {
    assert_eq!(output.len(), input.len());
    let len = output.len();
    let (simd_len, _) = calculate_simd_params(len);

    // --- x86/x86_64 ---
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            for i in (0..simd_len).step_by(8) {
                let g = _mm256_set1_ps(gain);
                let in_v = _mm256_loadu_ps(input.as_ptr().add(i));
                let scaled = _mm256_mul_ps(in_v, g);
                let clamped_min = _mm256_max_ps(scaled, _mm256_set1_ps(-1.0));
                let clamped = _mm256_min_ps(clamped_min, _mm256_set1_ps(1.0));
                _mm256_storeu_ps(output.as_mut_ptr().add(i), clamped);
            }
        } else if is_x86_feature_detected!("sse4.1") {
            for i in (0..simd_len).step_by(4) {
                let g = _mm_set1_ps(gain);
                let in_v = _mm_loadu_ps(input.as_ptr().add(i));
                let scaled = _mm_mul_ps(in_v, g);
                let clamped_min = _mm_max_ps(scaled, _mm_set1_ps(-1.0));
                let clamped = _mm_min_ps(clamped_min, _mm_set1_ps(1.0));
                _mm_storeu_ps(output.as_mut_ptr().add(i), clamped);
            }
        }
    }

    // --- aarch64 ---
    #[cfg(target_arch = "aarch64")]
    unsafe {
        for i in (0..simd_len).step_by(4) {
            let g = vdupq_n_f32(gain);
            let in_v = vld1q_f32(input.as_ptr().add(i));
            let scaled = vmulq_f32(in_v, g);
            let clamped_min = vminq_f32(scaled, vdupq_n_f32(1.0));
            let clamped = vmaxq_f32(clamped_min, vdupq_n_f32(-1.0));
            vst1q_f32(output.as_mut_ptr().add(i), clamped);
        }
    }

    for i in simd_len..len {
        output[i] = (input[i] * gain).clamp(-1.0, 1.0);
    }
}

fn calculate_simd_params(len: usize) -> (usize, usize) {
    let simd_width: usize = {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if is_x86_feature_detected!("avx2") {
                8
            } else if is_x86_feature_detected!("sse4.1") {
                4
            } else if is_x86_feature_detected!("sse2") {
                2
            } else {
                1
            }
        }
        #[cfg(all(target_arch = "aarch64", target_feature = "sve"))]
        {
            16
        }
        #[cfg(all(target_arch = "aarch64", not(target_feature = "sve")))]
        {
            4
        }
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
        {
            1
        }
    };

    let simd_len = len / simd_width * simd_width;
    let remainder = len % simd_width;
    (simd_len, remainder)
}

// Helper function to extract a specific audio channel's data
#[inline]
pub fn plane(frame: &Frame, channel: usize) -> &[f32] {
    let num_samples = unsafe { (*frame.as_ptr()).nb_samples } as usize;
    unsafe { std::slice::from_raw_parts((*frame.as_ptr()).data[channel] as *mut f32, num_samples) }
}

// Helper function to get mutable access to a specific audio channel's data
#[inline]
pub fn plane_mut(frame: &Frame, channel: usize) -> &mut [f32] {
    let num_samples = unsafe { (*frame.as_ptr()).nb_samples } as usize;
    unsafe { std::slice::from_raw_parts_mut((*frame.as_ptr()).data[channel] as *mut f32, num_samples) }
}
