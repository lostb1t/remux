use crate::flv::flv_tag_header::FlvTagHeader;
use crate::flv::{FLV_TAG_HEADER_LENGTH, PREVIOUS_TAG_SIZE_LENGTH};

#[derive(Debug, Clone)]
pub struct FlvTag {
    pub header: FlvTagHeader,
    pub data: bytes::Bytes,       // Tag data
    pub previous_tag_size: u32, // PreviousTagSize
}

impl FlvTag {
    // Convert the FLV Tag to a byte array [u8], including the header, data, and PreviousTagSize
    pub fn as_bytes_with_previous_tag_size(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FLV_TAG_HEADER_LENGTH + self.data.len() + PREVIOUS_TAG_SIZE_LENGTH);

        // Convert header to bytes
        bytes.extend_from_slice(&self.header.as_bytes());

        // Add data
        bytes.extend_from_slice(&self.data);

        // Add PreviousTagSize (4 bytes)
        bytes.extend_from_slice(&self.previous_tag_size.to_be_bytes());

        bytes
    }

    pub fn is_audio(&self) -> bool {
        self.header.is_audio()
    }

    pub fn is_video(&self) -> bool {
        self.header.is_video()
    }

    pub fn is_script_data(&self) -> bool {
        self.header.is_script_data()
    }

    pub fn is_video_sequence_header(&self) -> bool {
        if !self.is_video() {
            return false;
        }
        // This is assuming h264.
        self.data.len() >= 2 && self.data[0] == 0x17 && self.data[1] == 0x00
    }

    pub fn is_audio_sequence_header(&self) -> bool {
        if !self.is_audio() {
            return false;
        }
        // This is assuming aac.
        self.data.len() >= 2 && self.data[0] == 0xaf && self.data[1] == 0x00
    }

    pub fn is_video_keyframe(&self) -> bool {
        if !self.is_video() {
            return false;
        }
        // Assuming h264.
        self.data.len() >= 2 && self.data[0] == 0x17 && self.data[1] != 0x00 // 0x00 is the sequence header, don't count that for now
    }
}