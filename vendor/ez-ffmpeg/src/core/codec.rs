use ffmpeg_sys_next::{
    av_codec_is_decoder, av_codec_is_encoder, av_codec_iterate, avcodec_descriptor_next,
    AVCodecDescriptor, AVCodecID, AVMediaType,
};
use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut};

#[derive(Clone)]
pub(crate) struct Codec {
    inner: *const ffmpeg_sys_next::AVCodec,
}
unsafe impl Send for Codec {}
unsafe impl Sync for Codec {}

impl Codec {
    pub(crate) fn null() -> Self {
        Self { inner: null() }
    }
    pub(crate) fn is_null(&self) -> bool {
        self.inner.is_null()
    }

    pub(crate) fn new(avcodec: *const ffmpeg_sys_next::AVCodec) -> Self {
        Self { inner: avcodec }
    }

    pub(crate) fn as_ptr(&self) -> *const ffmpeg_sys_next::AVCodec {
        self.inner
    }
}

/// Holds metadata about a specific codec recognized by FFmpeg.
///
/// This struct consolidates information from both [`AVCodec`](ffmpeg_sys_next::AVCodec) (via `codec_name`, `codec_long_name`,
/// etc.) and the [`AVCodecDescriptor`] (via `desc_name`) into a single, user-friendly format. It
/// can be used to inspect properties of encoders or decoders available in your FFmpeg build.
///
/// # Fields
/// * `codec_name` - The short name of the codec (e.g., `"h264"`, `"aac"`).
/// * `codec_long_name` - A more descriptive name of the codec (e.g., `"H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10"`).
/// * `desc_name` - The descriptor’s name, typically very similar or identical to `codec_name`.
/// * `media_type` - The media category (`AVMEDIA_TYPE_AUDIO`, `AVMEDIA_TYPE_VIDEO`, etc.).
/// * `codec_id` - The internal FFmpeg `AVCodecID`, an enum identifying the codec.
/// * `codec_capabilities` - A bitmask indicating codec capabilities (e.g., intra-only, lossless).
#[derive(Clone, Debug)]
pub struct CodecInfo {
    /// The short name from `AVCodec.name`.
    pub codec_name: String,
    /// The long name from `AVCodec.long_name`.
    pub codec_long_name: String,
    /// The descriptor’s name from `AVCodecDescriptor.name`.
    pub desc_name: String,

    /// The media type (e.g., audio, video, subtitle).
    pub media_type: AVMediaType,
    /// The numeric codec ID (e.g., `AV_CODEC_ID_H264`).
    pub codec_id: AVCodecID,
    /// A bitmask of capabilities from `AVCodec.capabilities`.
    pub codec_capabilities: i32,
}

/// Retrieves a list of **all encoders** (e.g., for H.264, AAC, etc.) recognized by FFmpeg.
///
/// Each returned [`CodecInfo`] contains metadata such as the short codec name, long codec name,
/// descriptor name, media type, and capabilities. This function helps you discover which encoders
/// are compiled into your FFmpeg build.
///
/// # Example
///
/// ```rust
/// let encoders = get_encoders();
/// for encoder in encoders {
///     println!("Encoder: {} ({})", encoder.codec_name, encoder.codec_long_name);
/// }
/// ```
pub fn get_encoders() -> Vec<CodecInfo> {
    get_codec_infos(1)
}

/// Retrieves a list of **all decoders** (e.g., for H.264, AAC, etc.) recognized by FFmpeg.
///
/// Each returned [`CodecInfo`] contains metadata such as the short codec name, long codec name,
/// descriptor name, media type, and capabilities. This function helps you discover which decoders
/// are compiled into your FFmpeg build.
///
/// # Example
///
/// ```rust
/// let decoders = get_decoders();
/// for decoder in decoders {
///     println!("Decoder: {} ({})", decoder.codec_name, decoder.codec_long_name);
/// }
/// ```
pub fn get_decoders() -> Vec<CodecInfo> {
    get_codec_infos(0)
}

fn get_codec_infos(encoder: i32) -> Vec<CodecInfo> {
    let mut codec_infos = Vec::new();
    let descs = get_codecs_sorted();
    for desc in descs {
        let mut iter = null_mut();
        unsafe {
            loop {
                let codec = next_codec_for_id((*desc).id, &mut iter, encoder);
                if codec.is_null() {
                    break;
                }

                let codec_name = (*codec.inner).name;
                let codec_name = CStr::from_ptr(codec_name).to_str();
                let codec_long_name = (*codec.inner).long_name;
                let codec_long_name = CStr::from_ptr(codec_long_name).to_str();
                let desc_name = (*desc).name;
                let desc_name = CStr::from_ptr(desc_name).to_str();

                if let (Ok(codec_name), Ok(codec_long_name), Ok(desc_name)) =
                    (codec_name, codec_long_name, desc_name)
                {
                    let codec_info = CodecInfo {
                        codec_name: codec_name.to_string(),
                        codec_long_name: codec_long_name.to_string(),
                        desc_name: desc_name.to_string(),
                        media_type: (*codec.inner).type_,
                        codec_id: (*codec.inner).id,
                        codec_capabilities: (*codec.inner).capabilities,
                    };
                    codec_infos.push(codec_info);
                }
            }
        }
    }
    codec_infos
}

fn get_codecs_sorted() -> Vec<*const AVCodecDescriptor> {
    let mut desc = null();
    let mut codecs = Vec::new();

    unsafe {
        loop {
            desc = avcodec_descriptor_next(desc);
            if desc.is_null() {
                break;
            }
            codecs.push(desc)
        }
    }
    if !codecs.is_empty() {
        codecs.sort_by(|&a, &b| compare_codec_desc(a, b));
    }

    codecs
}

fn compare_codec_desc(
    a: *const AVCodecDescriptor,
    b: *const AVCodecDescriptor,
) -> std::cmp::Ordering {
    unsafe {
        match ((*a).type_ as i32).cmp(&((*b).type_ as i32)) {
            std::cmp::Ordering::Equal => (*a).name.cmp(&(*b).name),
            other => other,
        }
    }
}

fn next_codec_for_id(id: AVCodecID, iter: *mut *mut c_void, encoder: i32) -> Codec {
    loop {
        unsafe {
            let c = av_codec_iterate(iter);
            if c.is_null() {
                break;
            }

            if (*c).id == id {
                let result = if encoder != 0 {
                    av_codec_is_encoder(c)
                } else {
                    av_codec_is_decoder(c)
                };
                if result != 0 {
                    return Codec::new(c);
                }
            }
        }
    }

    Codec::null()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_codes() {
        let encoders = get_encoders();
        for encoder in encoders {
            println!("{:?}", encoder);
        }

        println!("----------------------");

        let decoders = get_decoders();
        for decoder in decoders {
            println!("{:?}", decoder);
        }
    }
}
