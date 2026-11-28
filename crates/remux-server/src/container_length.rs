//! The length a video file states in its own header, read from its first
//! bytes as they are relayed.
//!
//! Matroska and WebM state it in the Segment's Info element (Duration, in
//! units of TimestampScale), MP4 and MOV in the `mvhd` box inside `moov`. A
//! [`ContainerLength`] is fed a relayed body in the pieces it arrives in,
//! keeps at most [`HEADER_LIMIT`] bytes, and gives the length in ticks once
//! it has it. Nothing is allocated from a size read from the data, and no
//! input makes it panic.

/// The most bytes kept from the start of a body. A header that does not end
/// within them gives no length.
pub const HEADER_LIMIT: usize = 1024 * 1024;

/// The most elements or boxes walked before the one that states the
/// length. Real files need fewer than ten; past this a body gives none, so
/// a body of many empty elements costs little however it arrives.
const MAX_WALK: usize = 64;

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const CLUSTER: u32 = 0x1F43_B675;
const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const VOID: u32 = 0xEC;
const CRC32: u32 = 0xBF;
/// TimestampScale when the Info element has none: one millisecond.
const DEFAULT_SCALE_NS: u64 = 1_000_000;

/// Where a relayed body starts in its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyStart {
    /// At byte 0.
    File,
    /// Elsewhere: read only when it begins with an MP4 `moov` box, which is
    /// how a client fetches the index of a file that keeps it at the end.
    Range,
}

/// The container a length was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Matroska,
    Mp4,
}

impl Container {
    pub fn name(self) -> &'static str {
        match self {
            Self::Matroska => "matroska",
            Self::Mp4 => "mp4",
        }
    }
}

/// What the bytes so far say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Length(i64, Container),
    /// Not a header this reads, or one that states no usable length.
    NoLength,
    /// More bytes are needed.
    More,
}

type Parse<T> = Result<T, Step>;

/// Reads a file's stated length from the bytes of one relayed body.
#[derive(Debug)]
pub struct ContainerLength {
    start: BodyStart,
    kept: Vec<u8>,
    finished: bool,
    container: Option<Container>,
}

impl ContainerLength {
    pub fn new(start: BodyStart) -> Self {
        Self {
            start,
            kept: Vec::new(),
            finished: false,
            container: None,
        }
    }

    /// Takes the next bytes of the body. Returns the length in ticks once,
    /// when these bytes complete it; after that, or once the bytes show there
    /// is none, or [`HEADER_LIMIT`] is reached, it is finished and takes
    /// nothing more.
    pub fn feed(&mut self, bytes: &[u8]) -> Option<i64> {
        if self.finished {
            return None;
        }
        let room = HEADER_LIMIT.saturating_sub(
            self.kept
                .len(),
        );
        let taken = bytes
            .get(
                ..bytes
                    .len()
                    .min(room),
            )
            .unwrap_or_default();
        self.kept
            .extend_from_slice(taken);
        match read(&self.kept, self.start) {
            Step::Length(ticks, container) => {
                self.finish();
                self.container = Some(container);
                Some(ticks)
            }
            Step::NoLength => {
                self.finish();
                None
            }
            Step::More => {
                if self
                    .kept
                    .len()
                    >= HEADER_LIMIT
                {
                    self.finish();
                }
                None
            }
        }
    }

    /// Whether it takes no more bytes.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The container the length was read from, once it was.
    pub fn container(&self) -> Option<Container> {
        self.container
    }

    fn finish(&mut self) {
        self.finished = true;
        self.kept = Vec::new();
    }
}

fn read(bytes: &[u8], start: BodyStart) -> Step {
    let magic = EBML.to_be_bytes();
    let head = bytes
        .get(
            ..bytes
                .len()
                .min(magic.len()),
        )
        .unwrap_or_default();
    let result = if start == BodyStart::File && magic.starts_with(head) {
        if head.len() < magic.len() {
            return Step::More;
        }
        matroska(bytes).map(|ticks| (ticks, Container::Matroska))
    } else {
        mp4(bytes, start).map(|ticks| (ticks, Container::Mp4))
    };
    match result {
        Ok((ticks, container)) => Step::Length(ticks, container),
        Err(step) => step,
    }
}

/// The end of something of `size` bytes from `from`, when it is within the
/// bytes ever kept.
fn end_within(from: usize, size: u64) -> Parse<usize> {
    usize::try_from(size)
        .ok()
        .and_then(|size| from.checked_add(size))
        .filter(|end| *end <= HEADER_LIMIT)
        .ok_or(Step::NoLength)
}

/// Ticks from a length in microseconds, when it is a positive finite one.
/// It is cut to whole microseconds, as a probe of the file reports it, so
/// both give the same picture count.
fn ticks_from_us(us: f64) -> Parse<i64> {
    let us = us.trunc();
    if us.is_finite() && us >= 1.0 && us < (i64::MAX / 10) as f64 {
        Ok(us as i64 * 10)
    } else {
        Err(Step::NoLength)
    }
}

// Matroska and WebM.

/// An element id at `pos`, with its length marker, and its length in bytes.
fn ebml_id(bytes: &[u8], pos: usize) -> Parse<(u32, usize)> {
    let first = *bytes
        .get(pos)
        .ok_or(Step::More)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return Err(Step::NoLength);
    }
    let id = bytes
        .get(pos..pos + len)
        .ok_or(Step::More)?
        .iter()
        .fold(0u32, |id, b| (id << 8) | u32::from(*b));
    Ok((id, len))
}

/// An element size at `pos` (`None` when it is unknown) and its length in
/// bytes.
fn ebml_size(bytes: &[u8], pos: usize) -> Parse<(Option<u64>, usize)> {
    let first = *bytes
        .get(pos)
        .ok_or(Step::More)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return Err(Step::NoLength);
    }
    let rest = bytes
        .get(pos + 1..pos + len)
        .ok_or(Step::More)?;
    let marked = u64::from(first) & (0xFF_u64 >> len);
    let size = rest
        .iter()
        .fold(marked, |size, b| (size << 8) | u64::from(*b));
    let unknown = (1_u64 << (7 * len)) - 1;
    Ok(((size != unknown).then_some(size), len))
}

/// An element's id, size and where its data starts.
fn ebml_element(bytes: &[u8], pos: usize) -> Parse<(u32, Option<u64>, usize)> {
    let (id, id_len) = ebml_id(bytes, pos)?;
    let (size, size_len) = ebml_size(bytes, pos + id_len)?;
    Ok((id, size, pos + id_len + size_len))
}

fn matroska(bytes: &[u8]) -> Parse<i64> {
    let (id, size, data) = ebml_element(bytes, 0)?;
    if id != EBML {
        return Err(Step::NoLength);
    }
    let mut pos = end_within(data, size.ok_or(Step::NoLength)?)?;
    let mut walked = 0;
    let (segment_end, mut pos) = loop {
        walked += 1;
        if walked > MAX_WALK {
            return Err(Step::NoLength);
        }
        let (id, size, data) = ebml_element(bytes, pos)?;
        match id {
            SEGMENT => {
                let end = size.and_then(|size| {
                    usize::try_from(size)
                        .ok()
                        .and_then(|size| data.checked_add(size))
                });
                break (end, data);
            }
            VOID | CRC32 => pos = end_within(data, size.ok_or(Step::NoLength)?)?,
            _ => return Err(Step::NoLength),
        }
    };
    loop {
        walked += 1;
        if walked > MAX_WALK || segment_end.is_some_and(|end| pos >= end) {
            return Err(Step::NoLength);
        }
        let (id, size, data) = ebml_element(bytes, pos)?;
        if id == CLUSTER {
            return Err(Step::NoLength);
        }
        let end = end_within(data, size.ok_or(Step::NoLength)?)?;
        if id == INFO {
            let info = bytes
                .get(data..end)
                .ok_or(Step::More)?;
            return info_length(info);
        }
        pos = end;
    }
}

/// The length an Info element's data states.
fn info_length(info: &[u8]) -> Parse<i64> {
    let mut scale = DEFAULT_SCALE_NS;
    let mut duration = None;
    let mut pos = 0;
    while pos < info.len() {
        // The element is whole, so a child cut short is a broken one.
        let (id, size, data) = ebml_element(info, pos).map_err(|_| Step::NoLength)?;
        let end = size
            .and_then(|size| usize::try_from(size).ok())
            .and_then(|size| data.checked_add(size))
            .ok_or(Step::NoLength)?;
        let value = info
            .get(data..end)
            .ok_or(Step::NoLength)?;
        match id {
            TIMESTAMP_SCALE => {
                scale = ebml_uint(value)
                    .filter(|scale| *scale > 0)
                    .ok_or(Step::NoLength)?
            }
            DURATION => duration = Some(ebml_float(value).ok_or(Step::NoLength)?),
            _ => {}
        }
        pos = end;
    }
    let duration = duration.ok_or(Step::NoLength)?;
    // Duration in units of the scale's nanoseconds, to microseconds.
    ticks_from_us(duration * scale as f64 * 1000.0 / 1_000_000.0)
}

fn ebml_uint(value: &[u8]) -> Option<u64> {
    (1..=8)
        .contains(&value.len())
        .then(|| {
            value
                .iter()
                .fold(0u64, |n, b| (n << 8) | u64::from(*b))
        })
}

fn ebml_float(value: &[u8]) -> Option<f64> {
    match value.len() {
        4 => Some(f64::from(f32::from_be_bytes(
            value
                .try_into()
                .ok()?,
        ))),
        8 => Some(f64::from_be_bytes(
            value
                .try_into()
                .ok()?,
        )),
        _ => None,
    }
}

// MP4 and MOV.

/// A box's type, where its data starts and where it ends.
fn mp4_box(bytes: &[u8], pos: usize) -> Parse<([u8; 4], usize, usize)> {
    let head = bytes
        .get(pos..pos + 8)
        .ok_or(Step::More)?;
    let mut kind = [0u8; 4];
    kind.copy_from_slice(&head[4..8]);
    let (size, header) = match u32::from_be_bytes([head[0], head[1], head[2], head[3]])
    {
        // A 64-bit size follows the type.
        1 => {
            let large = bytes
                .get(pos + 8..pos + 16)
                .ok_or(Step::More)?;
            let mut size = [0u8; 8];
            size.copy_from_slice(large);
            (u64::from_be_bytes(size), 16)
        }
        // To the end of the file: never a box this reads.
        0 => return Err(Step::NoLength),
        size => (u64::from(size), 8),
    };
    if size < header {
        return Err(Step::NoLength);
    }
    let end = usize::try_from(size)
        .ok()
        .and_then(|size| pos.checked_add(size))
        .ok_or(Step::NoLength)?;
    Ok((kind, pos + header as usize, end))
}

fn mp4(bytes: &[u8], start: BodyStart) -> Parse<i64> {
    let mut pos = 0;
    for _ in 0..MAX_WALK {
        let (kind, data, end) = mp4_box(bytes, pos)?;
        if &kind == b"moov" {
            return moov_length(bytes, data, end);
        }
        let before_moov = matches!(&kind, b"ftyp" | b"free" | b"wide" | b"skip");
        if start == BodyStart::Range || !before_moov || end > HEADER_LIMIT {
            return Err(Step::NoLength);
        }
        pos = end;
    }
    Err(Step::NoLength)
}

/// The length the `mvhd` box inside a `moov` box whose data runs from `data`
/// to `end` states. In a fragmented file (one whose `moov` holds an `mvex`
/// box) `mvhd` states only the part in `moov`, so that gives none: the boxes
/// after `mvhd` are walked by their headers to the end of `moov`, or until
/// the next one starts past the bytes ever kept, since a `moov` that large
/// holds the whole file's tables.
fn moov_length(bytes: &[u8], data: usize, end: usize) -> Parse<i64> {
    let mut pos = data;
    let mut stated = None;
    for _ in 0..MAX_WALK {
        if pos >= end {
            return stated.ok_or(Step::NoLength);
        }
        if let Some(ticks) = stated
            && pos.saturating_add(8) > HEADER_LIMIT
        {
            return Ok(ticks);
        }
        let (kind, child_data, child_end) = mp4_box(bytes, pos)?;
        if child_end > end || (stated.is_none() && child_end > HEADER_LIMIT) {
            return Err(Step::NoLength);
        }
        match &kind {
            b"mvex" => return Err(Step::NoLength),
            b"mvhd" if stated.is_none() => {
                let body = bytes
                    .get(child_data..child_end)
                    .ok_or(Step::More)?;
                stated = Some(mvhd_length(body)?);
            }
            _ => {}
        }
        pos = child_end;
    }
    Err(Step::NoLength)
}

fn mvhd_length(body: &[u8]) -> Parse<i64> {
    let be32 = |at: usize| -> Parse<u32> {
        let b = body
            .get(at..at + 4)
            .ok_or(Step::NoLength)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (timescale, duration, unknown) = match body
        .first()
        .copied()
    {
        // Version and flags, creation and modification times, then these.
        Some(0) => (be32(12)?, u64::from(be32(16)?), u64::from(u32::MAX)),
        Some(1) => {
            let high = u64::from(be32(24)?);
            let low = u64::from(be32(28)?);
            (be32(20)?, (high << 32) | low, u64::MAX)
        }
        _ => return Err(Step::NoLength),
    };
    if timescale == 0 || duration == 0 || duration == unknown {
        return Err(Step::NoLength);
    }
    // To the nearest microsecond, as a probe of the file reports it.
    let timescale = u128::from(timescale);
    let us = (u128::from(duration) * 1_000_000 + timescale / 2) / timescale;
    i64::try_from(us)
        .ok()
        .and_then(|us| us.checked_mul(10))
        .filter(|ticks| *ticks > 0)
        .ok_or(Step::NoLength)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made-up headers, built here.

    /// An EBML element: its id's bytes, an 8-byte size, then `data`.
    fn element(id: u32, data: &[u8]) -> Vec<u8> {
        let mut out = id_bytes(id);
        out.push(0x01);
        out.extend_from_slice(&(data.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(data);
        out
    }

    /// An element with a 1-byte size (data under 127 bytes).
    fn short_element(id: u32, data: &[u8]) -> Vec<u8> {
        let mut out = id_bytes(id);
        out.push(0x80 | data.len() as u8);
        out.extend_from_slice(data);
        out
    }

    fn id_bytes(id: u32) -> Vec<u8> {
        let bytes = id.to_be_bytes();
        let skip = bytes
            .iter()
            .take_while(|b| **b == 0)
            .count();
        bytes[skip..].to_vec()
    }

    fn ebml_header() -> Vec<u8> {
        short_element(EBML, &short_element(0x4282, b"matroska"))
    }

    fn duration8(units: f64) -> Vec<u8> {
        short_element(DURATION, &units.to_be_bytes())
    }

    fn scale(ns: u32) -> Vec<u8> {
        short_element(TIMESTAMP_SCALE, &ns.to_be_bytes())
    }

    /// A Matroska file: header, then a Segment of known size holding
    /// `children`, then a few bytes of a cluster.
    fn matroska_file(children: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = children.concat();
        let mut out = ebml_header();
        out.extend(element(SEGMENT, &body));
        out.extend(short_element(CLUSTER, &[0u8; 16]));
        out
    }

    fn unknown_size_segment(children: &[Vec<u8>]) -> Vec<u8> {
        let mut out = ebml_header();
        out.extend(id_bytes(SEGMENT));
        out.push(0xFF);
        out.extend(children.concat());
        out
    }

    fn info(children: &[Vec<u8>]) -> Vec<u8> {
        element(INFO, &children.concat())
    }

    fn mp4_box_of(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = ((data.len() + 8) as u32)
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out
    }

    fn mvhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
        let mut data = vec![0u8; 4 + 8];
        data.extend(timescale.to_be_bytes());
        data.extend(duration.to_be_bytes());
        data.extend([0u8; 80]);
        mp4_box_of(b"mvhd", &data)
    }

    fn mvhd_v1(timescale: u32, duration: u64) -> Vec<u8> {
        let mut data = vec![1u8, 0, 0, 0];
        data.extend([0u8; 16]);
        data.extend(timescale.to_be_bytes());
        data.extend(duration.to_be_bytes());
        data.extend([0u8; 80]);
        mp4_box_of(b"mvhd", &data)
    }

    fn moov(mvhd: Vec<u8>) -> Vec<u8> {
        let mut data = mvhd;
        data.extend(mp4_box_of(b"trak", &[0u8; 40]));
        mp4_box_of(b"moov", &data)
    }

    fn ftyp() -> Vec<u8> {
        mp4_box_of(b"ftyp", b"isom\0\0\0\0isomiso2mp41")
    }

    fn whole(bytes: &[u8], start: BodyStart) -> Option<i64> {
        ContainerLength::new(start).feed(bytes)
    }

    fn byte_by_byte(bytes: &[u8], start: BodyStart) -> Option<i64> {
        let mut reader = ContainerLength::new(start);
        let mut found = None;
        for b in bytes {
            if let Some(ticks) = reader.feed(std::slice::from_ref(b)) {
                assert!(found.is_none(), "answered twice");
                found = Some(ticks);
            }
        }
        found
    }

    /// Fed whole and one byte at a time, the answer is the same.
    fn both(bytes: &[u8], start: BodyStart) -> Option<i64> {
        let found = whole(bytes, start);
        assert_eq!(byte_by_byte(bytes, start), found);
        found
    }

    /// Every truncation gives no answer, or the answer of the whole, and
    /// never panics.
    fn every_cut(bytes: &[u8], start: BodyStart, answer: Option<i64>) {
        for n in 0..bytes.len() {
            let got = both(&bytes[..n], start);
            assert!(got.is_none() || got == answer, "cut at {n}: {got:?}");
        }
    }

    const FILM_TICKS: i64 = 62_065_280_000;

    #[test]
    fn matroska_states_its_length_in_info() {
        let file = matroska_file(&[
            short_element(0x114D_9B74, &[0u8; 30]),
            short_element(VOID, &[0u8; 9]),
            short_element(CRC32, &[0u8; 4]),
            short_element(0x1254_C367, &[0u8; 3]),
            info(&[scale(1_000_000), duration8(6_206_528.0)]),
        ]);
        assert_eq!(both(&file, BodyStart::File), Some(FILM_TICKS));
        every_cut(&file, BodyStart::File, Some(FILM_TICKS));
        let mut reader = ContainerLength::new(BodyStart::File);
        assert_eq!(reader.feed(&file), Some(FILM_TICKS));
        assert!(reader.is_finished());
        assert_eq!(reader.container(), Some(Container::Matroska));
        assert_eq!(reader.feed(&file), None, "one answer");
    }

    #[test]
    fn a_duration_before_or_after_its_scale_and_a_default_scale() {
        // 6,206.528 s in units of 10 µs, either order.
        let after = matroska_file(&[info(&[scale(10_000), duration8(620_652_800.0)])]);
        let before = matroska_file(&[info(&[duration8(620_652_800.0), scale(10_000)])]);
        assert_eq!(both(&after, BodyStart::File), Some(FILM_TICKS));
        assert_eq!(both(&before, BodyStart::File), Some(FILM_TICKS));
        // No TimestampScale: milliseconds.
        let default = matroska_file(&[info(&[duration8(6_206_528.0)])]);
        assert_eq!(both(&default, BodyStart::File), Some(FILM_TICKS));
        // A 4-byte Duration.
        let float4 = matroska_file(&[info(&[short_element(
            DURATION,
            &6_000_000.0_f32.to_be_bytes(),
        )])]);
        assert_eq!(both(&float4, BodyStart::File), Some(60_000_000_000));
    }

    #[test]
    fn matroska_without_a_usable_duration_gives_none() {
        let cases = [
            matroska_file(&[info(&[scale(1_000_000)])]),
            matroska_file(&[info(&[duration8(0.0)])]),
            matroska_file(&[info(&[duration8(-5.0)])]),
            matroska_file(&[info(&[duration8(f64::NAN)])]),
            matroska_file(&[info(&[duration8(f64::INFINITY)])]),
            matroska_file(&[info(&[duration8(f64::MAX)])]),
            matroska_file(&[info(&[scale(0), duration8(5.0)])]),
            matroska_file(&[info(&[short_element(DURATION, &[1, 2, 3])])]),
            // A cluster before Info.
            matroska_file(&[
                short_element(CLUSTER, &[0u8; 4]),
                info(&[duration8(5.0)]),
            ]),
            // An element of unknown size before Info.
            {
                let mut file = ebml_header();
                file.extend(element(SEGMENT, &{
                    let mut body = id_bytes(0x1654_AE6B);
                    body.push(0xFF);
                    body.extend(info(&[duration8(5.0)]));
                    body
                }));
                file
            },
        ];
        for (i, file) in cases
            .iter()
            .enumerate()
        {
            assert_eq!(both(file, BodyStart::File), None, "case {i}");
            let mut reader = ContainerLength::new(BodyStart::File);
            reader.feed(file);
            assert!(reader.is_finished(), "case {i} is settled");
        }
    }

    #[test]
    fn an_unknown_size_segment_is_read() {
        let file = unknown_size_segment(&[
            short_element(VOID, &[0u8; 2]),
            info(&[scale(1_000_000), duration8(6_206_528.0)]),
        ]);
        assert_eq!(both(&file, BodyStart::File), Some(FILM_TICKS));
        every_cut(&file, BodyStart::File, Some(FILM_TICKS));
    }

    #[test]
    fn a_matroska_range_from_elsewhere_is_not_read() {
        let file = matroska_file(&[info(&[duration8(6_206_528.0)])]);
        assert_eq!(both(&file, BodyStart::Range), None);
    }

    #[test]
    fn mp4_states_its_length_in_mvhd() {
        // 6,206.528 s at 1,000 a second, in both versions.
        let mut v0 = ftyp();
        v0.extend(mp4_box_of(b"free", &[0u8; 8]));
        v0.extend(moov(mvhd_v0(1_000, 6_206_528)));
        v0.extend(mp4_box_of(b"mdat", &[0u8; 64]));
        assert_eq!(both(&v0, BodyStart::File), Some(FILM_TICKS));
        every_cut(&v0, BodyStart::File, Some(FILM_TICKS));
        let mut v1 = ftyp();
        v1.extend(mp4_box_of(b"wide", &[]));
        v1.extend(moov(mvhd_v1(90_000, 558_587_520)));
        assert_eq!(both(&v1, BodyStart::File), Some(FILM_TICKS));
        every_cut(&v1, BodyStart::File, Some(FILM_TICKS));
        let mut reader = ContainerLength::new(BodyStart::File);
        reader.feed(&v1);
        assert_eq!(reader.container(), Some(Container::Mp4));
    }

    #[test]
    fn a_range_that_begins_with_moov_is_read() {
        let index = moov(mvhd_v0(1_000, 6_206_528));
        assert_eq!(both(&index, BodyStart::Range), Some(FILM_TICKS));
        every_cut(&index, BodyStart::Range, Some(FILM_TICKS));
        // A range of the file's media data: nothing, after its first box.
        let media = mp4_box_of(b"mdat", &[0u8; 64]);
        assert_eq!(both(&media, BodyStart::Range), None);
        let mut reader = ContainerLength::new(BodyStart::Range);
        reader.feed(&media[..8]);
        assert!(reader.is_finished());
    }

    #[test]
    fn mp4_whose_index_is_at_the_end_gives_none_from_the_start() {
        let mut file = ftyp();
        file.extend(mp4_box_of(b"mdat", &[0u8; 64]));
        file.extend(moov(mvhd_v0(1_000, 6_206_528)));
        assert_eq!(both(&file, BodyStart::File), None);
    }

    #[test]
    fn mp4_without_a_usable_length_gives_none() {
        let cases = [
            moov(mvhd_v0(0, 6_206_528)),
            moov(mvhd_v0(1_000, 0)),
            moov(mvhd_v0(1_000, u32::MAX)),
            moov(mvhd_v1(1_000, u64::MAX)),
            moov(mvhd_v1(1, u64::MAX - 1)),
            moov({
                let mut b = mvhd_v0(1_000, 5);
                b[8] = 2;
                b
            }),
            moov(mp4_box_of(b"trak", &[0u8; 8])),
        ];
        for (i, index) in cases
            .iter()
            .enumerate()
        {
            let mut file = ftyp();
            file.extend(index);
            assert_eq!(both(&file, BodyStart::File), None, "case {i}");
            assert_eq!(both(index, BodyStart::Range), None, "case {i}");
        }
    }

    #[test]
    fn hostile_sizes_never_panic_or_allocate_from_the_data() {
        let mut cases: Vec<Vec<u8>> = Vec::new();
        // EBML sizes: zero, huge, all ones, past the buffer, nested past it.
        for size in [
            vec![0x80],
            vec![0x01, 0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
            vec![0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE],
            vec![0x00],
            vec![0x41, 0x00],
        ] {
            let mut file = id_bytes(EBML);
            file.extend(&size);
            file.extend([0u8; 12]);
            cases.push(file);
            let mut file = ebml_header();
            file.extend(id_bytes(SEGMENT));
            file.extend(&size);
            file.extend(id_bytes(INFO));
            file.extend(&size);
            file.extend(duration8(5.0));
            cases.push(file);
        }
        // An Info element whose child claims more than the Info holds.
        let mut file = ebml_header();
        file.extend(element(
            SEGMENT,
            &element(INFO, &{
                let mut child = id_bytes(DURATION);
                child.extend([0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00]);
                child.extend([0u8; 8]);
                child
            }),
        ));
        cases.push(file);
        // An Info element larger than the bytes ever kept.
        let mut file = ebml_header();
        file.extend(id_bytes(SEGMENT));
        file.push(0xFF);
        file.extend(id_bytes(INFO));
        file.extend([0x01, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00]);
        cases.push(file);
        // MP4 boxes: zero, under its header, 64-bit huge, past the buffer.
        for head in [
            [0u8, 0, 0, 0],
            [0, 0, 0, 4],
            [0, 0, 0, 1],
            [0xFF, 0xFF, 0xFF, 0xFF],
        ] {
            let mut file = head.to_vec();
            file.extend(b"moov");
            file.extend([0xFFu8; 8]);
            file.extend(mvhd_v0(1_000, 5));
            cases.push(file);
            let mut file = ftyp();
            file.extend(head);
            file.extend(b"free");
            file.extend([0xFFu8; 8]);
            cases.push(file);
        }
        // A moov whose child runs past the moov.
        let mut index = mp4_box_of(b"moov", &mvhd_v0(1_000, 5));
        index[3] = 20;
        cases.push(index);
        for case in &cases {
            for start in [BodyStart::File, BodyStart::Range] {
                every_cut(case, start, both(case, start));
            }
        }
    }

    #[test]
    fn other_bodies_give_none_at_once() {
        let page = b"<!DOCTYPE html><html><head><title>Not found</title></head></html>";
        let error = br#"{"error":"file not found","code":404}"#;
        for body in [&page[..], &error[..], &b""[..], &b"\x1A\x45"[..]] {
            assert_eq!(both(body, BodyStart::File), None);
            assert_eq!(both(body, BodyStart::Range), None);
        }
        let mut reader = ContainerLength::new(BodyStart::File);
        reader.feed(page);
        assert!(reader.is_finished());
    }

    #[test]
    fn it_keeps_at_most_the_limit_and_then_stops() {
        // A header whose Info starts past the limit.
        let mut file = ebml_header();
        file.extend(id_bytes(SEGMENT));
        file.push(0xFF);
        file.extend(element(0x114D_9B74, &vec![0u8; HEADER_LIMIT]));
        file.extend(info(&[duration8(5.0)]));
        let mut reader = ContainerLength::new(BodyStart::File);
        for piece in file.chunks(64 * 1024) {
            assert_eq!(reader.feed(piece), None);
        }
        assert!(reader.is_finished());

        // A body that keeps asking for more stops, at the limit at the
        // latest: empty elements (id 0x80, size 0) after one that fills most
        // of it.
        let mut file = ebml_header();
        file.extend(id_bytes(SEGMENT));
        file.push(0xFF);
        file.extend(element(0x114D_9B74, &vec![0u8; HEADER_LIMIT - 200]));
        let mut reader = ContainerLength::new(BodyStart::File);
        assert_eq!(reader.feed(&file), None);
        assert!(!reader.is_finished());
        while !reader.is_finished() {
            assert_eq!(reader.feed(&[0x80; 4096]), None);
        }
        assert!(
            reader
                .kept
                .is_empty()
        );
    }

    /// Feeds `bytes` in pieces of `piece` bytes until the reader is
    /// finished, and returns how many bytes that took.
    fn bytes_to_finish(bytes: &[u8], start: BodyStart, piece: usize) -> usize {
        let mut reader = ContainerLength::new(start);
        let mut fed = 0;
        for chunk in bytes.chunks(piece) {
            assert_eq!(reader.feed(chunk), None);
            fed += chunk.len();
            if reader.is_finished() {
                return fed;
            }
        }
        panic!("not finished after {fed} bytes");
    }

    #[test]
    fn a_body_of_many_empty_elements_ends_the_read_early() {
        // Empty elements (0x80 and Void) after a Segment of unknown size,
        // arriving two bytes at a time, as many as would fill the limit.
        for empty in [[0x80u8, 0x80], [0xEC, 0x80]] {
            let mut file = ebml_header();
            file.extend(id_bytes(SEGMENT));
            file.push(0xFF);
            while file.len() < HEADER_LIMIT {
                file.extend(empty);
            }
            let fed = bytes_to_finish(&file, BodyStart::File, 2);
            assert!(fed < 1_000, "{empty:?}: {fed} bytes");
        }
        // Empty `free` boxes after `ftyp`, eight bytes at a time.
        let mut file = ftyp();
        while file.len() < HEADER_LIMIT {
            file.extend(mp4_box_of(b"free", &[]));
        }
        let fed = bytes_to_finish(&file, BodyStart::File, 8);
        assert!(fed < 1_000, "{fed} bytes");
        // And empty boxes inside `moov`, before and after `mvhd`.
        for before in [true, false] {
            let mut data = Vec::new();
            if !before {
                data.extend(mvhd_v0(1_000, 5_000));
            }
            while data.len() < HEADER_LIMIT {
                data.extend(mp4_box_of(b"udta", &[]));
            }
            if before {
                data.extend(mvhd_v0(1_000, 5_000));
            }
            let index = mp4_box_of(b"moov", &data);
            let fed = bytes_to_finish(&index, BodyStart::Range, 8);
            assert!(fed < 1_000, "before {before}: {fed} bytes");
        }
    }

    #[test]
    fn a_fragmented_mp4_gives_none() {
        // `mvhd` states the 5 s in `moov`; `mehd` the whole 6,206 s.
        let mut mehd = vec![0u8; 4];
        mehd.extend(6_206_000u32.to_be_bytes());
        let mut data = mvhd_v0(1_000, 5_000);
        data.extend(mp4_box_of(b"trak", &[0u8; 40]));
        data.extend(mp4_box_of(b"mvex", &mp4_box_of(b"mehd", &mehd)));
        let index = mp4_box_of(b"moov", &data);
        let mut file = ftyp();
        file.extend(&index);
        file.extend(mp4_box_of(b"moof", &[0u8; 32]));
        assert_eq!(both(&file, BodyStart::File), None);
        every_cut(&file, BodyStart::File, None);
        assert_eq!(both(&index, BodyStart::Range), None);
        every_cut(&index, BodyStart::Range, None);
    }

    #[test]
    fn a_moov_larger_than_the_limit_states_its_length() {
        // `mvhd`, then a `trak` running past the bytes ever kept, then more.
        let mut data = mvhd_v0(1_000, 5_000);
        data.extend(mp4_box_of(b"trak", &vec![0u8; HEADER_LIMIT]));
        data.extend(mp4_box_of(b"udta", &[0u8; 8]));
        let mut file = ftyp();
        file.extend(mp4_box_of(b"moov", &data));
        assert_eq!(both(&file[..4096], BodyStart::File), Some(50_000_000));
        let mut reader = ContainerLength::new(BodyStart::File);
        let found: Vec<i64> = file
            .chunks(64 * 1024)
            .filter_map(|piece| reader.feed(piece))
            .collect();
        assert_eq!(found, vec![50_000_000]);
    }

    #[test]
    fn a_length_is_given_to_the_microsecond_as_a_probe_gives_it() {
        // 6,200.0000004 s: cut to 6,200 s, so 620 pictures, not 621.
        let file = matroska_file(&[info(&[duration8(6_200_000.0004)])]);
        assert_eq!(both(&file, BodyStart::File), Some(62_000_000_000));
        // MP4 rounds to the nearest microsecond.
        for (duration, ticks) in [
            (62_000_000_004_u64, 62_000_000_000),
            (62_000_000_006, 62_000_000_010),
        ] {
            let index = moov(mvhd_v1(10_000_000, duration));
            assert_eq!(both(&index, BodyStart::Range), Some(ticks), "{duration}");
        }
    }
}
