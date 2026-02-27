//! A high-performance ring buffer implementation designed for FLV (Flash Video) data processing.
//!
//! This buffer is optimized for:
//! - Minimizing memory fragmentation by using a pre-allocated ring buffer
//! - Reducing memory allocation/deallocation overhead
//! - High-performance memory operations using unsafe `copy_nonoverlapping`
//!
//! The buffer size is always a power of two to enable efficient modulo operations
//! using bitwise AND operations.
//!
//! # Memory Management
//! - Uses a ring buffer to cache data and prevent frequent memory allocations
//! - Only resizes when absolutely necessary to maintain performance
//! - All memory operations are performed using `copy_nonoverlapping` for maximum efficiency
//!
//! # Safety
//! While the implementation uses unsafe code for performance optimization,
//! all unsafe operations are carefully bounded and checked to maintain memory safety.
//!
use crate::flv::flv_header::FlvHeader;
use crate::flv::flv_tag::FlvTag;
use crate::flv::flv_tag_header::FlvTagHeader;
use crate::flv::{FLV_HEADER_LENGTH, FLV_TAG_HEADER_LENGTH, PREVIOUS_TAG_SIZE_LENGTH};
use byteorder::{BigEndian, ReadBytesExt};
use log::{debug, warn};
use std::io;
use std::io::Cursor;

#[derive(Debug)]
pub struct FlvBuffer {
    buffer: Vec<u8>,               // Internal buffer
    head: usize,                   // Index of the first valid byte in the buffer
    tail: usize,                   // Index where new data will be written
    flv_header: Option<FlvHeader>, // Store the parsed FLV header (if available)
    header_parsed: bool,           // Flag to track if FLV file header has been parsed
    initial_capacity: usize,       // Initial buffer capacity
}

impl FlvBuffer {
    /// Creates a new FlvBuffer with default capacity (1M).
    pub fn new() -> Self {
        Self::with_capacity(1024 * 1024)
    }

    /// Creates a new FlvBuffer with the specified capacity.
    /// The actual capacity will be rounded up to the next power of two.
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = if capacity.is_power_of_two() {
            capacity
        } else {
            capacity.checked_next_power_of_two().unwrap_or(usize::MAX)
        };

        FlvBuffer {
            buffer: vec![0; capacity],
            head: 0,
            tail: 0,
            flv_header: None,
            header_parsed: false, // Initially, the header is not parsed
            initial_capacity: capacity,
        }
    }

    /// Calculates the current length of valid data in the buffer.
    /// Uses efficient bitwise operations for modulo calculation.
    #[inline]
    fn len(&self) -> usize {
        /*if self.tail >= self.head {
            self.tail - self.head
        } else {
            self.buffer.len() - self.head + self.tail
        }*/
        (self.tail.wrapping_sub(self.head)) & (self.buffer.len() - 1)
    }

    /// Calculates the available space in the buffer.
    /// Uses efficient bitwise operations for modulo calculation.
    #[inline]
    fn available_space(&self) -> usize {
        // self.buffer.len() - self.len() - 1
        (self.head.wrapping_sub(self.tail).wrapping_sub(1)) & (self.buffer.len() - 1)
    }

    /// Writes data into the ring buffer.
    ///
    /// This method handles:
    /// - Buffer resizing if needed
    /// - Wrapping around the buffer end
    /// - High-performance memory copying using `copy_nonoverlapping`
    ///
    /// # Performance
    /// Uses unsafe `copy_nonoverlapping` for optimal memory copying performance,
    /// avoiding bounds checks and overlapping memory verification.
    pub fn write_data(&mut self, data: &[u8]) {
        let data_len = data.len();
        if data_len == 0 {
            return;
        }

        // Resize the buffer if needed to accommodate the new data.
        if data_len > self.available_space() {
            self.resize_buffer(self.len() + data_len + 1);
        }

        // Write data to the buffer, handling the wrap-around if necessary.
        if self.tail >= self.head {
            let available_at_end = self.buffer.len() - self.tail;

            if data_len <= available_at_end {
                // Can write the entire chunk in one go
                // self.buffer[self.tail..self.tail + data_len].copy_from_slice(data);
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        self.buffer.as_mut_ptr().add(self.tail),
                        data_len,
                    );
                }

                self.tail += data_len;
            } else {
                // Need to wrap around.
                if available_at_end > 0 {
                    // self.buffer[self.tail..].copy_from_slice(&data[..available_at_end]);
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            data.as_ptr(),
                            self.buffer.as_mut_ptr().add(self.tail),
                            available_at_end,
                        );
                    }
                }
                // self.buffer[..data_len - available_at_end].copy_from_slice(&data[available_at_end..]);
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr().add(available_at_end),
                        self.buffer.as_mut_ptr(),
                        data_len - available_at_end,
                    );
                }

                self.tail = data_len - available_at_end; //Tail is now at start of buffer.
            }
        } else {
            // Head is after tail - just write to the end.
            // self.buffer[self.tail..self.tail + data_len].copy_from_slice(data);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    self.buffer.as_mut_ptr().add(self.tail),
                    data_len,
                );
            }

            self.tail += data_len;
        }

        // Wrap around the tail if it reaches the end of the buffer.
        if self.tail == self.buffer.len() {
            self.tail = 0;
        }
    }

    /// Resizes the buffer to accommodate more data.
    ///
    /// # Notes
    /// - New capacity is always a power of two
    /// - Maintains data continuity during resize
    /// - Uses high-performance memory copying
    fn resize_buffer(&mut self, new_capacity: usize) {
        let new_capacity = new_capacity
            .checked_next_power_of_two()
            .unwrap_or(usize::MAX)
            .max(self.initial_capacity);

        let mut new_buffer = vec![0; new_capacity];

        // Calculate the current data length BEFORE replacing the buffer
        let current_len = self.len();

        // Copy existing data directly to new buffer
        if self.tail > self.head {
            new_buffer[..current_len].copy_from_slice(&self.buffer[self.head..self.tail]);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buffer.as_ptr().add(self.head),
                    new_buffer.as_mut_ptr(),
                    current_len,
                );
            }

        } else if current_len > 0 {
            let first_part = self.buffer.len() - self.head;
            // new_buffer[..first_part].copy_from_slice(&self.buffer[self.head..]);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buffer.as_ptr().add(self.head),
                    new_buffer.as_mut_ptr(),
                    first_part,
                );
            }

            // new_buffer[first_part..current_len].copy_from_slice(&self.buffer[..self.tail]);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buffer.as_ptr(),
                    new_buffer.as_mut_ptr().add(first_part),
                    current_len - first_part,
                );
            }
        }

        self.buffer = new_buffer;
        self.head = 0;
        self.tail = current_len;
    }

    /// Skips the PreviousTagSize field (4 bytes) after reading a tag.
    #[inline]
    fn skip_previous_tag_size(&mut self) {
        self.head += PREVIOUS_TAG_SIZE_LENGTH;
        if self.head >= self.buffer.len() {
            self.head -= self.buffer.len(); // Handle wrap-around.
        }
    }

    /// Attempts to parse the FLV file header from the buffer.
    ///
    /// # Returns
    /// - `Ok(())` if parsing succeeds or needs to be deferred
    /// - `Err` if an IO error occurs during parsing
    fn parse_flv_header(&mut self) -> io::Result<()> {
        if self.header_parsed {
            return Ok(()); // Header already parsed
        }

        // Check if there is enough data in buffer for an FLV header.
        if self.len() < FLV_HEADER_LENGTH {
            return Ok(()); // Not enough data, skip parsing.
        }

        let mut temp_buffer = [0u8; FLV_HEADER_LENGTH];
        self.read_data(self.head, &mut temp_buffer);

        let mut reader = Cursor::new(&temp_buffer);

        // Check if the file starts with "FLV"
        let flv_signature = reader.read_u24::<BigEndian>()?;
        debug!("FLV Signature: {:#X}", flv_signature);
        if flv_signature != 0x464C56 {
            // "FLV" in ASCII
            self.skip_previous_tag_size();
            self.header_parsed = true;
            return Ok(()); // Skip files that don't start with "FLV"
        }

        // Read the version (should be 1)
        let version = reader.read_u8()?;
        debug!("FLV Version: {}", version);
        if version != 1 {
            self.skip_previous_tag_size();
            self.header_parsed = true;
            return Ok(()); // Skip if the version is not 1
        }

        // Read the flags (should be 0x05 for audio and video presence)
        let flags = reader.read_u8()?;
        debug!("FLV Flags: {:#X}", flags);
        match flags {
            0x01 => debug!("Audio: No, Video: Yes"),
            0x04 => debug!("Audio: Yes, Video: No"),
            0x05 => debug!("Audio: Yes, Video: Yes"),
            _ => {
                self.skip_previous_tag_size();
                self.header_parsed = true;
                return Ok(());
            } // Skip invalid flags
        }

        // Read the data offset (indicates where the data starts)
        let data_offset = reader.read_u32::<BigEndian>()?;
        if data_offset != 9 {
            self.skip_previous_tag_size();
            self.header_parsed = true;
            return Ok(());
        }
        debug!("FLV Data Offset: {}", data_offset);

        // Store the header information
        self.flv_header = Some(FlvHeader { flags });

        // Mark the header as parsed
        self.header_parsed = true;

        // Advance cursor past the FLV header and the subsequent PreviousTagSize
        self.head += FLV_HEADER_LENGTH;
        self.skip_previous_tag_size();

        Ok(())
    }

    /// Returns a reference to the parsed FLV header, if available.
    pub fn get_flv_header(&self) -> Option<&FlvHeader> {
        self.flv_header.as_ref()
    }

    /// Attempts to parse and return a complete FLV tag from the buffer.
    ///
    /// # Returns
    /// - `Some(FlvTag)` if a complete tag is available
    /// - `None` if there isn't enough data for a complete tag
    pub fn get_flv_tag(&mut self) -> Option<FlvTag> {
        // Check if there's enough data to read a complete FLV Tag
        if self.len() < FLV_TAG_HEADER_LENGTH {
            return None; // Not enough data to read a tag header
        }

        // Ensure the FLV file header is parsed or skip if not valid
        if let Err(e) = self.parse_flv_header() {
            warn!("Failed parsing FLV header: {}", e);
            return None; // Return None if header parsing fails
        }

        // Create a reader that can handle buffer wrap-around
        let mut header_reader = CursorRing::new(&self.buffer, self.head, self.buffer.len());

        // Parse the FLV Tag Header
        let tag_type = header_reader.read_u8().ok()?;
        let data_size = header_reader.read_u24::<BigEndian>().ok()?;
        let timestamp = header_reader.read_u24::<BigEndian>().ok()?;
        let timestamp_ext = header_reader.read_u8().ok()?;
        let _stream_id = header_reader.read_u24::<BigEndian>().ok()?;

        // Calculate the total size of the tag, including the header and PreviousTagSize
        let total_tag_size = FLV_TAG_HEADER_LENGTH + data_size as usize + PREVIOUS_TAG_SIZE_LENGTH;

        // Check if there's enough data to read the full tag data
        if self.len() < total_tag_size {
            return None; // Not enough data to read the tag data and PreviousTagSize
        }

        // Read the tag data into a temporary buffer
        let mut data = vec![0u8; data_size as usize];
        let data_start = self.head + FLV_TAG_HEADER_LENGTH;
        self.read_data(data_start, &mut data);

        // Create the FLV Tag
        let flv_tag = FlvTag {
            header: FlvTagHeader {
                tag_type,
                data_size,
                timestamp,
                timestamp_ext,
                stream_id: 0, // Always 0
            },
            data: bytes::Bytes::from(data),
            previous_tag_size: (FLV_TAG_HEADER_LENGTH + data_size as usize) as u32, // Store PreviousTagSize
        };

        // Advance the head past the entire tag (header + data + PreviousTagSize)
        self.head += total_tag_size;

        // Handle wrap-around for head.
        while self.head >= self.buffer.len() {
            self.head -= self.buffer.len();
        }

        Some(flv_tag)
    }

    /// Reads data from the ring buffer, safely handling wrap-around.
    ///
    /// # Performance
    /// Uses unsafe `copy_nonoverlapping` for optimal memory copying,
    /// with careful bounds checking to ensure safety.
    fn read_data(&self, start: usize, buffer: &mut [u8]) {
        let buffer_size = self.buffer.len();
        if buffer_size == 0 || buffer.is_empty() {
            return;
        }

        let normalized_start = start % buffer_size;
        let request_len = buffer.len();
        let safe_len = request_len.min(buffer_size);

        let (first_len, second_len) = {
            let virtual_end = normalized_start + safe_len;
            if virtual_end <= buffer_size {
                (safe_len, 0)
            } else {
                (
                    buffer_size - normalized_start,
                    safe_len - (buffer_size - normalized_start),
                )
            }
        };

        // buffer[..first_len].copy_from_slice(&self.buffer[normalized_start..normalized_start  + first_len]);
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.buffer.as_ptr().add(normalized_start),
                buffer.as_mut_ptr(),
                first_len,
            );
        }

        if second_len > 0 {
            // buffer[first_len..first_len + second_len].copy_from_slice(&self.buffer[..second_len]);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buffer.as_ptr(),
                    buffer.as_mut_ptr().add(first_len),
                    second_len
                );
            }
        }
    }
}


/// A cursor implementation that handles ring buffer wrap-around.
/// Used for reading FLV tag headers efficiently.
struct CursorRing<'a> {
    buffer: &'a [u8],
    position: usize,
    buffer_size: usize,
}

impl<'a> CursorRing<'a> {
    fn new(buffer: &'a [u8], start: usize, buffer_size: usize) -> Self {
        Self {
            buffer,
            position: start,
            buffer_size,
        }
    }
}

impl<'a> io::Read for CursorRing<'a> {
    #[inline(always)]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut bytes_read = 0;
        let buf_len = buf.len();
        let buffer_end = self.buffer_size;

        let wrap_around = self.position + buf_len > buffer_end;

        if wrap_around {
            let first_part_len = buffer_end - self.position;
            // buf[..first_part_len].copy_from_slice(&self.buffer[self.position..]);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buffer.as_ptr().add(self.position),
                    buf.as_mut_ptr(),
                    first_part_len,
                );
            }
            self.position = 0;
            bytes_read += first_part_len;
        }

        let remaining_len = buf_len - bytes_read;
        if remaining_len > 0 {
            // buf[bytes_read..].copy_from_slice(&self.buffer[self.position..self.position + remaining_len]);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buffer.as_ptr().add(self.position),
                    buf.as_mut_ptr().add(bytes_read),
                    remaining_len,
                );
            }
            self.position += remaining_len;
            bytes_read += remaining_len;
        }

        if self.position == buffer_end {
            self.position = 0;
        }

        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_len() {
        assert_eq!(base_len(0, 10, 16), len(0, 10, 16));
        assert_eq!(base_len(1, 3, 16), len(1, 3, 16));
        assert_eq!(base_len(3, 5, 16), len(3, 5, 16));
        assert_eq!(base_len(4, 4, 16), len(4, 4, 16));
        assert_eq!(base_len(10, 2, 16), len(10, 2, 16));
        assert_eq!(base_len(8, 3, 16), len(8, 3, 16));
        assert_eq!(base_len(9, 0, 16), len(9, 0, 16));
    }

    fn len(head: usize, tail: usize, buffer_len: usize) -> usize {
        (tail.wrapping_sub(head)) & (buffer_len - 1)
    }

    fn base_len(head: usize, tail: usize, buffer_len: usize) -> usize {
        if tail >= head {
            tail - head
        } else {
            buffer_len - head + tail
        }
    }

    #[test]
    fn test_available_space() {
        assert_eq!(base_available_space(0, 10, 16), available_space(0, 10, 16));
        assert_eq!(base_available_space(1, 3, 16), available_space(1, 3, 16));
        assert_eq!(base_available_space(3, 5, 16), available_space(3, 5, 16));
        assert_eq!(base_available_space(4, 5, 16), available_space(4, 5, 16));
        assert_eq!(base_available_space(10, 2, 16), available_space(10, 2, 16));
        assert_eq!(base_available_space(8, 3, 16), available_space(8, 3, 16));
        assert_eq!(base_available_space(9, 0, 16), available_space(9, 0, 16));
    }

    fn base_available_space(head: usize, tail: usize, buffer_len: usize) -> usize {
        buffer_len - len(head, tail, buffer_len) - 1
    }

    fn available_space(head: usize, tail: usize, buffer_len: usize) -> usize {
        (head.wrapping_sub(tail).wrapping_sub(1)) & (buffer_len - 1)
    }
}
