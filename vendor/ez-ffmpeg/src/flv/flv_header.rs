use crate::flv::{FLV_HEADER_LENGTH, FLV_TAG_HEADER_LENGTH, PREVIOUS_TAG_SIZE_LENGTH};

#[derive(Debug)]
pub struct FlvHeader {
    pub(crate) flags: u8, // Only store the flags, other fields are fixed
}

impl FlvHeader {
    // Method to convert FlvHeader to a byte array [u8], borrowing self instead of taking ownership
    pub fn as_bytes(&self) -> [u8; FLV_HEADER_LENGTH] {
        [
            0x46, 0x4C, 0x56,       // "FLV" signature
            1,          // version
            self.flags, // flags
            0x00, 0x00, 0x00, 0x09, // data_offset (fixed as 9)
        ]
    }

    // Convert the FlvHeader to a byte array [u8], including the header and PreviousTagSize
    pub fn as_bytes_with_previous_tag_size(
        &self,
    ) -> [u8; FLV_HEADER_LENGTH + PREVIOUS_TAG_SIZE_LENGTH] {
        let mut bytes = [0u8; FLV_HEADER_LENGTH + PREVIOUS_TAG_SIZE_LENGTH];

        bytes[0..FLV_HEADER_LENGTH].copy_from_slice(&self.as_bytes());

        bytes
    }

    // Getter for flags
    pub fn flags(&self) -> u8 {
        self.flags
    }

    // Check if the FLV file contains audio
    pub fn has_audio(&self) -> bool {
        (self.flags  & 0x04) != 0
    }

    // Check if the FLV file contains video
    pub fn has_video(&self) -> bool {
        (self.flags  & 0x01) != 0
    }
}
