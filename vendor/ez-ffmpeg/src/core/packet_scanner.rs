use ffmpeg_sys_next::{
    av_packet_alloc, av_packet_free, av_packet_unref, av_read_frame,
    avformat_seek_file, AVPacket, AVERROR, EAGAIN,
    AV_PKT_FLAG_CORRUPT, AV_PKT_FLAG_KEY,
};

use std::iter::FusedIterator;

use crate::core::context::AVFormatContextBox;
use crate::core::stream_info::StreamInfo;
use crate::error::{DemuxingError, OpenInputError, PacketScannerError, Result};

/// Read-only metadata extracted from a single demuxed packet.
///
/// `PacketInfo` contains scalar values copied out of an `AVPacket` together with
/// stream-type flags looked up at read time, so it has no lifetime ties to the
/// scanner. It is cheap to clone and store.
///
/// # Defensive fields
///
/// `stream_index` and `size` are clamped to non-negative values before storage.
/// FFmpeg's internal asserts guarantee valid ranges in practice, so the clamping
/// is purely defensive and not expected to trigger.
#[derive(Debug, Clone)]
pub struct PacketInfo {
    stream_index: usize,
    pts: Option<i64>,
    dts: Option<i64>,
    duration: i64,
    size: usize,
    pos: i64,
    is_keyframe: bool,
    is_corrupt: bool,
    is_video: bool,
    is_audio: bool,
}

impl PacketInfo {
    /// The index of the stream this packet belongs to.
    pub fn stream_index(&self) -> usize {
        self.stream_index
    }

    /// Presentation timestamp in stream time-base units, if available.
    pub fn pts(&self) -> Option<i64> {
        self.pts
    }

    /// Decompression timestamp in stream time-base units, if available.
    pub fn dts(&self) -> Option<i64> {
        self.dts
    }

    /// Duration of this packet in stream time-base units.
    pub fn duration(&self) -> i64 {
        self.duration
    }

    /// Size of the packet data in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Byte position of this packet in the input file, or -1 if unknown.
    pub fn pos(&self) -> i64 {
        self.pos
    }

    /// Whether this packet contains a keyframe.
    pub fn is_keyframe(&self) -> bool {
        self.is_keyframe
    }

    /// Whether this packet is flagged as corrupt.
    pub fn is_corrupt(&self) -> bool {
        self.is_corrupt
    }

    /// Whether this packet belongs to a video stream.
    pub fn is_video(&self) -> bool {
        self.is_video
    }

    /// Whether this packet belongs to an audio stream.
    pub fn is_audio(&self) -> bool {
        self.is_audio
    }
}

/// A stateful packet-level scanner for media files.
///
/// `PacketScanner` opens a media file (or URL) and iterates over demuxed packets
/// without decoding. This is useful for inspecting packet metadata such as
/// timestamps, keyframe flags, sizes, and stream indices.
///
/// # Example
///
/// ```rust,ignore
/// use ez_ffmpeg::packet_scanner::PacketScanner;
///
/// let mut scanner = PacketScanner::open("test.mp4")?;
/// for packet in scanner.packets() {
///     let packet = packet?;
///     println!(
///         "stream={} pts={:?} size={} keyframe={}",
///         packet.stream_index(),
///         packet.pts(),
///         packet.size(),
///         packet.is_keyframe(),
///     );
/// }
/// ```
pub struct PacketScanner {
    fmt_ctx_box: AVFormatContextBox,
    pkt: *mut AVPacket,
    streams: Vec<StreamInfo>,
}

// SAFETY: PacketScanner owns its AVFormatContext and AVPacket exclusively.
// It is moved between threads, never shared. No thread-affine callbacks are registered.
// This is safe only because `open()` does not expose custom AVIO or interrupt callbacks.
// If custom callbacks are added in the future, this impl must be re-evaluated.
// This matches the safety reasoning of AVFormatContextBox's own `unsafe impl Send`.
unsafe impl Send for PacketScanner {}

impl PacketScanner {
    /// Open a media file or URL for packet scanning.
    ///
    /// Stream information is extracted and cached at open time so that
    /// [`streams`](Self::streams), [`video_stream`](Self::video_stream),
    /// [`audio_stream`](Self::audio_stream), and
    /// [`stream_for_packet`](Self::stream_for_packet) are available
    /// immediately without additional I/O.
    pub fn open(url: impl Into<String>) -> Result<Self> {
        let fmt_ctx_box = crate::core::stream_info::init_format_context(url)?;
        // SAFETY: fmt_ctx_box is fully initialized by init_format_context.
        let streams = unsafe { crate::core::stream_info::extract_stream_infos(&fmt_ctx_box)? };

        // SAFETY: av_packet_alloc returns a valid packet or null.
        // Null is checked immediately; the packet is freed in Drop.
        unsafe {
            let pkt = av_packet_alloc();
            if pkt.is_null() {
                return Err(OpenInputError::OutOfMemory.into());
            }

            Ok(Self { fmt_ctx_box, pkt, streams })
        }
    }

    /// Seek to a timestamp in microseconds.
    ///
    /// Seeks to the nearest keyframe before the given timestamp.
    /// Can be called repeatedly for jump-reading patterns.
    ///
    /// On failure you may continue reading or attempt another seek, though
    /// the exact read position is not guaranteed to be unchanged.
    pub fn seek(&mut self, timestamp_us: i64) -> Result<()> {
        // SAFETY: fmt_ctx is valid for the lifetime of self. avformat_seek_file
        // accepts any timestamp and returns a negative value on failure.
        unsafe {
            let ret = avformat_seek_file(
                self.fmt_ctx_box.fmt_ctx,
                -1,
                i64::MIN,
                timestamp_us,
                timestamp_us,
                0,
            );
            if ret < 0 {
                return Err(
                    PacketScannerError::SeekError(DemuxingError::from(ret)).into()
                );
            }
        }
        Ok(())
    }

    /// Read the next packet's info. Returns `None` at EOF.
    ///
    /// If the underlying demuxer returns `EAGAIN` (common with network streams),
    /// this method retries with a 10 ms sleep up to 500 times (~5 seconds).
    /// After exhausting retries it returns an error.
    pub fn next_packet(&mut self) -> Result<Option<PacketInfo>> {
        const MAX_EAGAIN_RETRIES: u32 = 500;

        // SAFETY: self.pkt is a valid, non-null AVPacket allocated in open().
        // av_packet_unref resets the packet for reuse; av_read_frame fills it.
        // We read only scalar fields from the filled packet.
        unsafe {
            av_packet_unref(self.pkt);

            let mut eagain_retries: u32 = 0;
            loop {
                let ret = av_read_frame(self.fmt_ctx_box.fmt_ctx, self.pkt);
                if ret == AVERROR(EAGAIN) {
                    eagain_retries += 1;
                    if eagain_retries > MAX_EAGAIN_RETRIES {
                        return Err(
                            PacketScannerError::ReadError(DemuxingError::from(ret)).into()
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                if ret < 0 {
                    if ret == ffmpeg_sys_next::AVERROR_EOF {
                        return Ok(None);
                    }
                    return Err(
                        PacketScannerError::ReadError(DemuxingError::from(ret)).into()
                    );
                }
                break;
            }

            let pkt = &*self.pkt;
            let pts = if pkt.pts == ffmpeg_sys_next::AV_NOPTS_VALUE {
                None
            } else {
                Some(pkt.pts)
            };
            let dts = if pkt.dts == ffmpeg_sys_next::AV_NOPTS_VALUE {
                None
            } else {
                Some(pkt.dts)
            };

            // FFmpeg guarantees via av_assert0 in handle_new_packet() (demux.c:571)
            // that stream_index is in [0, nb_streams). The .max(0) and .unwrap_or()
            // are purely defensive and not expected to trigger in practice.
            let stream_index = pkt.stream_index.max(0) as usize;
            let (is_video, is_audio) = self.streams.get(stream_index)
                .map(|s| (s.is_video(), s.is_audio()))
                .unwrap_or((false, false));

            Ok(Some(PacketInfo {
                stream_index,
                pts,
                dts,
                duration: pkt.duration,
                // FFmpeg does not document negative size; clamp to 0 defensively.
                size: { debug_assert!(pkt.size >= 0, "negative pkt.size: {}", pkt.size); pkt.size.max(0) as usize },
                pos: pkt.pos,
                is_keyframe: (pkt.flags & AV_PKT_FLAG_KEY) != 0,
                is_corrupt: (pkt.flags & AV_PKT_FLAG_CORRUPT) != 0,
                is_video,
                is_audio,
            }))
        }
    }

    /// Returns all stream information cached at open time.
    pub fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    /// Returns the first video stream, if any.
    pub fn video_stream(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.is_video())
    }

    /// Returns the first audio stream, if any.
    pub fn audio_stream(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.is_audio())
    }

    /// Returns the stream information for the given packet, if the stream
    /// index is within bounds.
    pub fn stream_for_packet(&self, packet: &PacketInfo) -> Option<&StreamInfo> {
        self.streams.get(packet.stream_index())
    }

    /// Returns an iterator for convenient `for packet in scanner.packets()` usage.
    ///
    /// Each call creates a fresh iterator, so you can `seek()` and then call
    /// `packets()` again to iterate from the new position.
    ///
    /// The iterator is fused: once it yields `None` (EOF) or an `Err`, all
    /// subsequent calls to `next()` return `None`.
    pub fn packets(&mut self) -> PacketIter<'_> {
        PacketIter { scanner: self, done: false }
    }
}

impl Drop for PacketScanner {
    fn drop(&mut self) {
        // SAFETY: pkt was allocated by av_packet_alloc in open().
        // av_packet_free handles null gracefully, but we check anyway.
        unsafe {
            if !self.pkt.is_null() {
                av_packet_free(&mut self.pkt);
            }
        }
        // AVFormatContextBox handles closing the format context
    }
}

/// Iterator wrapper for [`PacketScanner`].
///
/// Yields `Result<PacketInfo>` for each packet until EOF or an error occurs.
/// The iterator is fused: after returning `None` or `Err`, it always returns `None`.
pub struct PacketIter<'a> {
    scanner: &'a mut PacketScanner,
    done: bool,
}

impl<'a> Iterator for PacketIter<'a> {
    type Item = Result<PacketInfo>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.scanner.next_packet() {
            Ok(Some(info)) => Some(Ok(info)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

impl<'a> FusedIterator for PacketIter<'a> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_not_found() {
        let result = PacketScanner::open("not_found.mp4");
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_packets() {
        let mut scanner = PacketScanner::open("test.mp4").unwrap();
        let mut count = 0;
        let mut keyframes = 0;
        for packet in scanner.packets() {
            let info = packet.unwrap();
            count += 1;
            if info.is_keyframe() {
                keyframes += 1;
            }
        }
        assert!(count > 0, "expected at least one packet");
        assert!(keyframes > 0, "expected at least one keyframe");
        println!("total packets: {}, keyframes: {}", count, keyframes);
    }

    #[test]
    fn test_seek_and_read() {
        let mut scanner = PacketScanner::open("test.mp4").unwrap();
        // Seek to 1 second (1_000_000 microseconds)
        scanner.seek(1_000_000).unwrap();
        let packet = scanner.next_packet().unwrap();
        assert!(packet.is_some(), "expected a packet after seeking");
    }

    #[test]
    fn test_streams() {
        let scanner = PacketScanner::open("test.mp4").unwrap();
        let streams = scanner.streams();
        assert!(!streams.is_empty(), "expected at least one stream");
        assert_eq!(streams.len(), 2, "test.mp4 should have 2 streams (video + audio)");
    }

    #[test]
    fn test_video_stream() {
        let scanner = PacketScanner::open("test.mp4").unwrap();
        let video = scanner.video_stream();
        assert!(video.is_some(), "expected a video stream");
        assert!(video.unwrap().is_video());
    }

    #[test]
    fn test_audio_stream() {
        let scanner = PacketScanner::open("test.mp4").unwrap();
        let audio = scanner.audio_stream();
        assert!(audio.is_some(), "expected an audio stream");
        assert!(audio.unwrap().is_audio());
    }

    #[test]
    fn test_stream_for_packet() {
        let mut scanner = PacketScanner::open("test.mp4").unwrap();
        let packet = scanner.next_packet().unwrap();
        assert!(packet.is_some(), "expected at least one packet");
        let info = packet.unwrap();
        let stream = scanner.stream_for_packet(&info);
        assert!(stream.is_some(), "stream_for_packet should return Some for valid packet");
    }
}
