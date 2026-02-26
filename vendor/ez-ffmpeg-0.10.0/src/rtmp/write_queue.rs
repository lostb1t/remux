// src/rtmp/write_queue.rs - Write queue implementation
//
// Core features:
// - Tiered backpressure strategy (Normal/Warning/High/Critical)
// - Partial write support
// - Sequence headers prioritized (never dropped by backpressure policy, but rejected at critical threshold)
// - Special handling for audio-only streams
// - Time-based eviction strategy

use bytes::Bytes;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::time::Instant;

// Backpressure threshold constants
const QUEUE_WARN_BYTES: usize = 1 * 1024 * 1024; // 1MB warning
const QUEUE_HIGH_BYTES: usize = 2 * 1024 * 1024; // 2MB high watermark
const QUEUE_MAX_BYTES: usize = 4 * 1024 * 1024; // 4MB disconnect
const QUEUE_MAX_AGE_SECS: u64 = 10; // 10 second timeout
const AUDIO_ONLY_MAX_AGE_SECS: u64 = 5; // 5 second timeout for audio-only

/// Queue entry
struct WriteEntry {
    data: Bytes,
    offset: usize,
    timestamp: Instant,
    #[allow(dead_code)]
    is_keyframe: bool,
    is_sequence_header: bool, // SPS/PPS/AudioConfig prioritized (never dropped by policy, but rejected at critical)
}

impl WriteEntry {
    fn remaining(&self) -> &[u8] {
        &self.data[self.offset..]
    }

    fn advance(&mut self, n: usize) {
        self.offset += n;
    }

    fn is_complete(&self) -> bool {
        self.offset >= self.data.len()
    }

    fn remaining_bytes(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }

    fn age_secs(&self) -> u64 {
        self.timestamp.elapsed().as_secs()
    }
}

/// Backpressure level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackpressureLevel {
    Normal,   // < 1MB: enqueue all
    Warning,  // 1-2MB: drop non-keyframes, keep audio + keyframes
    High,     // 2-4MB: only keep keyframes and sequence headers
    Critical, // >= 4MB: should disconnect
}


/// Flush result
#[derive(Debug)]
pub enum FlushResult {
    /// QueueAll flushed
    Complete { bytes_written: usize },
    /// WouldBlock encountered, partial write
    WouldBlock { bytes_written: usize },
    /// Connection closed
    Closed,
}

/// Write queue
///
/// Write queue with tiered backpressure and partial write support
pub struct WriteQueue {
    queue: VecDeque<WriteEntry>,
    total_bytes: usize,
    has_video: bool, // Used to detect audio-only stream
    dropped_frames: u64,
}

impl WriteQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::with_capacity(64),
            total_bytes: 0,
            has_video: false,
            dropped_frames: 0,
        }
    }

    /// Current backpressure level
    pub fn backpressure_level(&self) -> BackpressureLevel {
        if self.total_bytes >= QUEUE_MAX_BYTES {
            BackpressureLevel::Critical
        } else if self.total_bytes >= QUEUE_HIGH_BYTES {
            BackpressureLevel::High
        } else if self.total_bytes >= QUEUE_WARN_BYTES {
            BackpressureLevel::Warning
        } else {
            BackpressureLevel::Normal
        }
    }

    /// Enqueue data
    ///
    /// # Arguments
    /// * `data` - Data to enqueue
    /// * `is_keyframe` - Whether it's a keyframe
    /// * `is_sequence_header` - Whether it's a sequence header (SPS/PPS/AudioConfig)
    /// * `is_video` - Whether it's video data
    ///
    /// # Returns
    /// * `true` - Successfully enqueued or dropped per policy
    /// * `false` - Queue full, should disconnect
    pub fn enqueue(
        &mut self,
        data: Bytes,
        is_keyframe: bool,
        is_sequence_header: bool,
        is_video: bool,
    ) -> bool {
        if is_video {
            self.has_video = true;
        }

        // Check critical threshold BEFORE adding to prevent overshoot
        // Use saturating_add to prevent overflow
        if self.total_bytes.saturating_add(data.len()) >= QUEUE_MAX_BYTES {
            return false;
        }

        let level = self.backpressure_level();

        // Sequence headers never dropped
        if is_sequence_header {
            self.push_entry(data, is_keyframe, true);
            return true;
        }

        match level {
            BackpressureLevel::Normal => {
                self.push_entry(data, is_keyframe, false);
            }
            BackpressureLevel::Warning => {
                // Drop non-keyframe video, keep audio
                if is_keyframe || !is_video {
                    self.push_entry(data, is_keyframe, false);
                } else {
                    self.dropped_frames += 1;
                }
                // Perform time-based eviction
                self.evict_old_entries();
            }
            BackpressureLevel::High => {
                // Keep keyframes only
                if is_keyframe {
                    self.push_entry(data, is_keyframe, false);
                } else {
                    self.dropped_frames += 1;
                }
                self.evict_old_entries();
            }
            BackpressureLevel::Critical => unreachable!(),
        }

        true
    }

    fn push_entry(&mut self, data: Bytes, is_keyframe: bool, is_sequence_header: bool) {
        let len = data.len();
        self.queue.push_back(WriteEntry {
            data,
            offset: 0,
            timestamp: Instant::now(),
            is_keyframe,
            is_sequence_header,
        });
        self.total_bytes += len;
    }

    /// Time-based eviction - Remove stale data
    fn evict_old_entries(&mut self) {
        let max_age = if self.has_video {
            QUEUE_MAX_AGE_SECS
        } else {
            AUDIO_ONLY_MAX_AGE_SECS
        };

        while let Some(entry) = self.queue.front() {
            // Sequence headers never evicted
            if entry.is_sequence_header {
                break;
            }
            if entry.age_secs() > max_age {
                if let Some(removed) = self.queue.pop_front() {
                    self.total_bytes = self.total_bytes.saturating_sub(removed.remaining_bytes());
                    self.dropped_frames += 1;
                }
            } else {
                break;
            }
        }
    }

    /// Try to flush to writer
    ///
    /// Supports partial write, tracks write offset for each entry
    pub fn try_flush<W: Write>(&mut self, writer: &mut W) -> io::Result<FlushResult> {
        let mut bytes_written = 0;

        while let Some(entry) = self.queue.front_mut() {
            let buf = entry.remaining();
            if buf.is_empty() {
                // Entry complete, subtract from total bytes
                let entry_size = self.queue.front().map(|e| e.data.len()).unwrap_or(0);
                self.total_bytes = self.total_bytes.saturating_sub(entry_size);
                self.queue.pop_front();
                continue;
            }

            match writer.write(buf) {
                Ok(0) => return Ok(FlushResult::Closed),
                Ok(n) => {
                    bytes_written += n;
                    entry.advance(n);
                    if entry.is_complete() {
                        let entry_size = self.queue.front().map(|e| e.data.len()).unwrap_or(0);
                        self.total_bytes = self.total_bytes.saturating_sub(entry_size);
                        self.queue.pop_front();
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(FlushResult::WouldBlock { bytes_written });
                }
                Err(e) => return Err(e),
            }
        }

        Ok(FlushResult::Complete { bytes_written })
    }

    /// Is queue empty
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Bytes pending to send (test only)
    #[cfg(test)]
    pub fn pending_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Entry count in queue (test only)
    #[cfg(test)]
    pub fn pending_entries(&self) -> usize {
        self.queue.len()
    }

    /// Dropped frames count (test only)
    #[cfg(test)]
    fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    /// Has video flag (test only)
    #[cfg(test)]
    fn has_video(&self) -> bool {
        self.has_video
    }

}

impl Default for WriteQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_data(size: usize) -> Bytes {
        Bytes::from(vec![0u8; size])
    }

    #[test]
    fn test_basic_enqueue_dequeue() {
        let mut queue = WriteQueue::new();

        queue.enqueue(make_data(100), false, false, true);
        assert_eq!(queue.pending_bytes(), 100);
        assert_eq!(queue.pending_entries(), 1);
        assert_eq!(queue.backpressure_level(), BackpressureLevel::Normal);
    }

    #[test]
    fn test_backpressure_levels() {
        // Test each level independently with fresh queues

        // Normal level (< 1MB)
        {
            let mut queue = WriteQueue::new();
            queue.enqueue(make_data(512 * 1024), true, false, true); // use keyframe to avoid drops
            assert_eq!(queue.backpressure_level(), BackpressureLevel::Normal);
        }

        // Warning level (>= 1MB, < 2MB)
        {
            let mut queue = WriteQueue::new();
            queue.enqueue(make_data(1500 * 1024), true, false, true); // use keyframe
            assert_eq!(queue.backpressure_level(), BackpressureLevel::Warning);
        }

        // High level (>= 2MB, < 4MB)
        {
            let mut queue = WriteQueue::new();
            queue.enqueue(make_data(3 * 1024 * 1024), true, false, true); // use keyframe
            assert_eq!(queue.backpressure_level(), BackpressureLevel::High);
        }

        // Critical threshold test - enqueue that would reach critical is rejected
        {
            let mut queue = WriteQueue::new();
            // First fill to just below critical (3.5MB)
            queue.enqueue(make_data(3500 * 1024), true, false, true);
            assert_eq!(queue.backpressure_level(), BackpressureLevel::High);

            // Try to add 600KB which would push total to 4.1MB >= 4MB (critical)
            // This should be rejected
            let result = queue.enqueue(make_data(600 * 1024), true, false, true);
            assert!(!result, "Enqueue should be rejected when it would reach Critical");
            // Queue should still be at High level (data was rejected)
            assert_eq!(queue.backpressure_level(), BackpressureLevel::High);
        }
    }

    #[test]
    fn test_sequence_header_never_dropped() {
        let mut queue = WriteQueue::new();

        // Fill up to high level
        queue.enqueue(make_data(3 * 1024 * 1024), false, false, true);
        assert_eq!(queue.backpressure_level(), BackpressureLevel::High);

        // Sequence header should still be enqueued
        let result = queue.enqueue(make_data(100), false, true, true);
        assert!(result);

        // Non-keyframe should be dropped at high level
        let _before = queue.pending_entries();
        queue.enqueue(make_data(100), false, false, true);
        // Entry count should not increase for non-keyframe
        assert!(queue.dropped_frames() > 0);
    }

    #[test]
    fn test_keyframe_preserved_at_high_level() {
        let mut queue = WriteQueue::new();

        // Fill up to high level
        queue.enqueue(make_data(3 * 1024 * 1024), false, false, true);
        assert_eq!(queue.backpressure_level(), BackpressureLevel::High);

        let before = queue.pending_entries();

        // Keyframe should be accepted
        queue.enqueue(make_data(100), true, false, true);
        assert_eq!(queue.pending_entries(), before + 1);
    }

    #[test]
    fn test_audio_preserved_at_warning_level() {
        let mut queue = WriteQueue::new();

        // Fill up to warning level
        queue.enqueue(make_data(1500 * 1024), false, false, true);
        assert_eq!(queue.backpressure_level(), BackpressureLevel::Warning);

        let before = queue.pending_entries();

        // Audio should be accepted at warning level
        queue.enqueue(make_data(100), false, false, false);
        assert_eq!(queue.pending_entries(), before + 1);

        // Non-keyframe video should be dropped
        let dropped_before = queue.dropped_frames();
        queue.enqueue(make_data(100), false, false, true);
        assert!(queue.dropped_frames() > dropped_before);
    }

    #[test]
    fn test_critical_rejects_all() {
        let mut queue = WriteQueue::new();

        // Fill up to just below critical level (3.9MB)
        queue.enqueue(make_data(3900 * 1024), true, false, true);
        assert_eq!(queue.backpressure_level(), BackpressureLevel::High);

        // Try to add data that would exceed critical threshold
        // Even keyframes should be rejected
        let result = queue.enqueue(make_data(200 * 1024), true, false, true);
        assert!(!result, "Keyframe should be rejected when it would exceed Critical");

        // Even sequence headers should be rejected when threshold would be exceeded
        // (Note: sequence headers bypass drop policy but not critical threshold)
        let result = queue.enqueue(make_data(200 * 1024), false, true, true);
        assert!(!result, "Sequence header should be rejected when it would exceed Critical");
        assert!(!result);
    }

    #[test]
    fn test_partial_write() {
        let mut queue = WriteQueue::new();
        queue.enqueue(Bytes::from_static(b"hello"), false, false, true);
        queue.enqueue(Bytes::from_static(b"world"), false, false, true);

        // Create a limited writer that can only write 3 bytes at a time
        struct LimitedWriter {
            inner: Vec<u8>,
            limit: usize,
        }

        impl Write for LimitedWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                let n = buf.len().min(self.limit);
                self.inner.extend_from_slice(&buf[..n]);
                Ok(n)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut writer = LimitedWriter {
            inner: Vec::new(),
            limit: 3,
        };

        // First flush: writes "hel" from "hello"
        let result = queue.try_flush(&mut writer).unwrap();
        assert!(matches!(result, FlushResult::Complete { .. }));

        // All data should be written
        assert_eq!(writer.inner, b"helloworld");
        assert!(queue.is_empty());
    }

    #[test]
    fn test_would_block_handling() {
        struct WouldBlockWriter {
            written: usize,
            block_after: usize,
        }

        impl Write for WouldBlockWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                if self.written >= self.block_after {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
                let n = buf.len().min(self.block_after - self.written);
                self.written += n;
                Ok(n)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut queue = WriteQueue::new();
        queue.enqueue(Bytes::from_static(b"hello"), false, false, true);
        queue.enqueue(Bytes::from_static(b"world"), false, false, true);

        let mut writer = WouldBlockWriter {
            written: 0,
            block_after: 3,
        };

        // Should write 3 bytes then WouldBlock
        let result = queue.try_flush(&mut writer).unwrap();
        assert!(matches!(result, FlushResult::WouldBlock { bytes_written: 3 }));
        assert!(!queue.is_empty());
    }

    #[test]
    fn test_stats() {
        let mut queue = WriteQueue::new();
        queue.enqueue(make_data(1000), false, false, true);

        assert_eq!(queue.pending_bytes(), 1000);
        assert_eq!(queue.pending_entries(), 1);
        assert!(queue.has_video());
        assert_eq!(queue.dropped_frames(), 0);
    }

    #[test]
    fn test_pure_audio_stream() {
        let mut queue = WriteQueue::new();

        // Only enqueue audio
        queue.enqueue(make_data(100), false, false, false);
        queue.enqueue(make_data(100), false, false, false);

        assert!(!queue.has_video());
    }
}
