use crate::flv::FLV_TAG_HEADER_LENGTH;

#[derive(Debug, Clone)]
pub struct FlvTagHeader {
    pub tag_type: u8,        // Tag Type (0x08: Audio, 0x09: Video, 0x12: Script Data)
    pub data_size: u32,      // Size of the Tag Data
    pub timestamp: u32,      // Timestamp
    pub timestamp_ext: u8,   // Extended Timestamp
    pub stream_id: u32,      // Always 0
}

impl FlvTagHeader {
    // Method to convert FlvTagHeader to a byte array [u8]
    pub fn as_bytes(&self) -> [u8; FLV_TAG_HEADER_LENGTH] {
        let mut bytes = [0u8; FLV_TAG_HEADER_LENGTH];

        // Add tag_type (u8)
        bytes[0] = self.tag_type;
        // Add data_size (u24, 3 bytes)
        bytes[1..4].copy_from_slice(&self.data_size.to_be_bytes()[1..]);
        // Add timestamp (u24, 3 bytes)
        bytes[4..7].copy_from_slice(&self.timestamp.to_be_bytes()[1..]);
        // Add timestamp_ext (u8)
        bytes[7] = self.timestamp_ext;
        // Add stream_id (u24, 3 bytes)
        bytes[8..11].copy_from_slice(&(self.stream_id.to_be_bytes()[1..]));

        bytes
    }

    pub fn is_audio(&self) -> bool {
        self.tag_type == 0x08
    }

    pub fn is_video(&self) -> bool {
        self.tag_type == 0x09
    }

    pub fn is_script_data(&self) -> bool {
        self.tag_type == 0x12
    }
}