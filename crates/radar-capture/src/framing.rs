//! AION 2 stream framing.
//!
//! **Source and confidence.** These rules are taken from the documentation of
//! the Aether DPS meter (v3.1.0, `capture/processor.rs` doc comments and
//! `docs/AION2_PACKET_PROTOCOL_ANALYSIS.zh-CN.md`), which states they were
//! checked against recorded global-service sessions. This project has not
//! verified them against live traffic itself; the opcode census exists to
//! show quickly whether they hold. The implementation is independent (Aether
//! is GPL-3.0; no code was copied).
//!
//! ```text
//! frame    = length-varint [flag 0xF0..=0xFE] opcode(2) body
//! length   = varint value + varint byte count - 4      (whole frame, in bytes)
//! padding  = 0x00 bytes may sit between frames
//! bundle   = a frame whose opcode is FF FF:
//!            FF FF, decompressed size (u32 LE), LZ4 block of further frames
//! heartbeat= 0E 00 36  (an 11-byte frame, ~20 per second from the server)
//! ```
//!
//! When the position in the stream is lost (a gap, or a length that makes no
//! sense), parsing resumes at the next heartbeat.

use lz4_flex::block::decompress;

pub const HEARTBEAT: [u8; 3] = [0x0E, 0x00, 0x36];
const MAX_FRAME: usize = 256 * 1024;
const MAX_BUNDLE: usize = 5_000_000;
const MAX_BUFFER: usize = 4 * 1024 * 1024;
const MAX_BUNDLE_DEPTH: u8 = 4;
/// A frame still incomplete this long after it started, while data keeps
/// arriving, had a bogus length.
const STALLED_MS: f64 = 2_000.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub opcode: [u8; 2],
    /// The packet from its opcode onward.
    pub payload: Vec<u8>,
    /// It came out of a compressed bundle.
    pub bundled: bool,
}

impl Frame {
    pub fn opcode_hex(&self) -> String {
        format!("{:02X} {:02X}", self.opcode[0], self.opcode[1])
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FramingStats {
    pub frames: u64,
    pub bundles: u64,
    pub bad_bundles: u64,
    pub resyncs: u64,
    pub dropped_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Head {
    Sized { len: usize, header: usize },
    NeedMore,
    Invalid,
}

fn read_head(buf: &[u8]) -> Head {
    let mut value: u64 = 0;
    for i in 0..5 {
        let Some(&b) = buf.get(i) else {
            return Head::NeedMore;
        };
        value |= u64::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            let header = i + 1;
            let len = (value + header as u64).saturating_sub(4) as usize;
            if len < header + 2 || len > MAX_FRAME {
                return Head::Invalid;
            }
            return Head::Sized { len, header };
        }
    }
    Head::Invalid
}

/// Encodes a frame the way the documentation says the game does. Used by
/// tests and synthetic fixtures.
pub fn encode_frame(opcode: [u8; 2], body: &[u8]) -> Vec<u8> {
    let inner = 2 + body.len();
    for header in 1..=5usize {
        // len = value + header - 4, and len = header + inner.
        let value = (inner + 4) as u64;
        let mut v = Vec::new();
        let mut rest = value;
        loop {
            let byte = (rest & 0x7F) as u8;
            rest >>= 7;
            if rest == 0 {
                v.push(byte);
                break;
            }
            v.push(byte | 0x80);
        }
        if v.len() == header {
            let mut out = v;
            out.extend_from_slice(&opcode);
            out.extend_from_slice(body);
            return out;
        }
    }
    unreachable!("frame too large to encode");
}

/// Wraps already-encoded frames into a compressed bundle frame.
pub fn encode_bundle(frames: &[u8]) -> Vec<u8> {
    let mut body = (frames.len() as u32).to_le_bytes().to_vec();
    body.extend(lz4_flex::block::compress(frames));
    encode_frame([0xFF, 0xFF], &body)
}

/// Splits one direction of a game connection into frames.
#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<u8>,
    synced: bool,
    waiting_since: Option<f64>,
    stats: FramingStats,
}

impl Framer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> FramingStats {
        self.stats
    }

    /// Starts on bytes captured before the stream was recognised. When they
    /// are the connection's first bytes and frame cleanly up to the first
    /// heartbeat, they are parsed too (zone data arrives before heartbeats).
    pub fn start(&mut self, pending: &[u8], from_start: bool, t_ms: f64) -> Vec<Frame> {
        let first_hb = find(pending, 0);
        let begin = match first_hb {
            Some(hb) if from_start && chains_to(pending, hb) => 0,
            Some(hb) => hb,
            None => 0,
        };
        self.stats.dropped_bytes += begin as u64;
        self.synced = first_hb.is_some();
        self.feed(&pending[begin..], t_ms)
    }

    /// Bytes were lost under the parser.
    pub fn desync(&mut self) {
        self.synced = false;
        self.waiting_since = None;
    }

    pub fn feed(&mut self, data: &[u8], t_ms: f64) -> Vec<Frame> {
        let mut out = Vec::new();
        self.buf.extend_from_slice(data);
        if self.buf.len() > MAX_BUFFER {
            self.stats.dropped_bytes += self.buf.len() as u64;
            self.buf.clear();
            self.desync();
            return out;
        }
        let buf = std::mem::take(&mut self.buf);
        let mut pos = 0;
        loop {
            if !self.synced {
                match find(&buf, pos) {
                    Some(hb) => {
                        self.stats.dropped_bytes += (hb - pos) as u64;
                        self.stats.resyncs += 1;
                        pos = hb;
                        self.synced = true;
                    }
                    None => {
                        let keep = buf.len().saturating_sub(HEARTBEAT.len() - 1).max(pos);
                        self.stats.dropped_bytes += (keep - pos) as u64;
                        pos = keep;
                        break;
                    }
                }
            }
            while buf.get(pos) == Some(&0) {
                pos += 1;
            }
            if pos >= buf.len() {
                break;
            }
            match read_head(&buf[pos..]) {
                Head::Sized { len, header } => {
                    if pos + len > buf.len() {
                        let since = *self.waiting_since.get_or_insert(t_ms);
                        if t_ms - since > STALLED_MS {
                            self.lose(&mut pos);
                            continue;
                        }
                        break;
                    }
                    self.waiting_since = None;
                    self.stats.frames += 1;
                    self.emit(&buf[pos..pos + len], header, false, 0, &mut out);
                    pos += len;
                }
                Head::NeedMore => break,
                Head::Invalid => self.lose(&mut pos),
            }
        }
        let mut buf = buf;
        buf.drain(..pos.min(buf.len()));
        self.buf = buf;
        out
    }

    fn lose(&mut self, pos: &mut usize) {
        self.synced = false;
        self.waiting_since = None;
        self.stats.dropped_bytes += 1;
        *pos += 1;
    }

    fn emit(
        &mut self,
        frame: &[u8],
        header: usize,
        bundled: bool,
        depth: u8,
        out: &mut Vec<Frame>,
    ) {
        let Some(&first) = frame.get(header) else {
            return;
        };
        let off = header + usize::from((0xF0..0xFF).contains(&first));
        let Some(payload) = frame.get(off..).filter(|p| p.len() >= 2) else {
            return;
        };
        if payload[..2] == [0xFF, 0xFF] {
            self.unbundle(payload, depth, out);
        } else {
            out.push(Frame {
                opcode: [payload[0], payload[1]],
                payload: payload.to_vec(),
                bundled,
            });
        }
    }

    fn unbundle(&mut self, payload: &[u8], depth: u8, out: &mut Vec<Frame>) {
        self.stats.bundles += 1;
        if payload.len() < 7 || depth >= MAX_BUNDLE_DEPTH {
            self.stats.bad_bundles += 1;
            return;
        }
        let size = u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]) as usize;
        let inner = match (size > 0 && size <= MAX_BUNDLE).then(|| decompress(&payload[6..], size))
        {
            Some(Ok(inner)) => inner,
            _ => {
                self.stats.bad_bundles += 1;
                return;
            }
        };
        let mut pos = 0;
        while pos < inner.len() {
            if inner[pos] == 0 {
                pos += 1;
                continue;
            }
            match read_head(&inner[pos..]) {
                Head::Sized { len, header } if pos + len <= inner.len() => {
                    self.emit(&inner[pos..pos + len], header, true, depth + 1, out);
                    pos += len;
                }
                _ => {
                    self.stats.bad_bundles += 1;
                    return;
                }
            }
        }
    }
}

/// The next heartbeat at or after `from`.
pub fn find(buf: &[u8], from: usize) -> Option<usize> {
    buf.get(from..)?
        .windows(HEARTBEAT.len())
        .position(|w| w == HEARTBEAT)
        .map(|i| from + i)
}

/// Counts heartbeats in `buf`.
pub fn count_heartbeats(buf: &[u8]) -> usize {
    buf.windows(HEARTBEAT.len())
        .filter(|w| *w == HEARTBEAT)
        .count()
}

/// Whether frames read from 0 land exactly on `target`.
fn chains_to(buf: &[u8], target: usize) -> bool {
    let mut pos = 0;
    while pos < target {
        if buf[pos] == 0 {
            pos += 1;
            continue;
        }
        match read_head(&buf[pos..]) {
            Head::Sized { len, .. } => pos += len,
            _ => return false,
        }
    }
    pos == target
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heartbeat() -> Vec<u8> {
        encode_frame([0x00, 0x36], &[0u8; 8])
    }

    #[test]
    fn the_documented_heartbeat_is_an_11_byte_frame() {
        let hb = heartbeat();
        assert_eq!(hb.len(), 11);
        assert_eq!(hb[..3], HEARTBEAT);
        assert_eq!(read_head(&hb), Head::Sized { len: 11, header: 1 });
    }

    #[test]
    fn lengths_use_the_documented_formula_for_long_varints() {
        let f = encode_frame([0x12, 0x34], &[7u8; 300]);
        assert_eq!(f.len(), 2 + 2 + 300); // two-byte varint
        assert_eq!(
            read_head(&f),
            Head::Sized {
                len: 304,
                header: 2
            }
        );
    }

    #[test]
    fn splits_frames_skips_padding_and_flags() {
        let mut stream = heartbeat();
        stream.extend([0, 0]);
        // Flag byte F5, then opcode 41 36: the flag counts toward the length.
        stream.extend(encode_frame([0xF5, 0x41], &[0x36, 1, 2, 3]));
        stream.extend(encode_frame([0x04, 0x38], &[9; 200]));
        let mut f = Framer::new();
        let frames = f.start(&stream, true, 0.0);
        let ops: Vec<_> = frames.iter().map(Frame::opcode_hex).collect();
        assert_eq!(ops, vec!["00 36", "41 36", "04 38"]);
        assert_eq!(frames[1].payload, vec![0x41, 0x36, 1, 2, 3]);
    }

    #[test]
    fn unpacks_nested_compressed_bundles() {
        let inner = [
            encode_frame([0x41, 0x36], &[1, 2, 3, 4]),
            encode_frame([0x04, 0x38], &[5; 40]),
        ]
        .concat();
        let nested = encode_bundle(&[encode_bundle(&inner), heartbeat()].concat());
        let mut f = Framer::new();
        let frames = f.start(&[heartbeat(), nested].concat(), true, 0.0);
        let ops: Vec<_> = frames
            .iter()
            .map(|fr| (fr.opcode_hex(), fr.bundled))
            .collect();
        assert_eq!(
            ops,
            vec![
                ("00 36".into(), false),
                ("41 36".into(), true),
                ("04 38".into(), true),
                ("00 36".into(), true)
            ]
        );
        assert_eq!(f.stats().bad_bundles, 0);
    }

    #[test]
    fn any_chunking_gives_the_same_frames() {
        let stream = [
            heartbeat(),
            encode_frame([0x41, 0x36], &[1; 150]),
            encode_bundle(&encode_frame([0x04, 0x38], &[2; 90])),
            heartbeat(),
        ]
        .concat();
        let expected = Framer::new().start(&stream, true, 0.0);
        for size in [1, 2, 5, 17, 64] {
            let mut f = Framer::new();
            let mut got = f.start(&[], true, 0.0);
            for c in stream.chunks(size) {
                got.extend(f.feed(c, 0.0));
            }
            assert_eq!(got, expected, "chunk size {size}");
        }
    }

    #[test]
    fn joins_mid_stream_and_recovers_after_a_gap_at_the_next_heartbeat() {
        let tail_of_something = vec![0x99, 0x98, 0x97];
        let stream = [
            tail_of_something,
            heartbeat(),
            encode_frame([0x41, 0x36], &[1]),
        ]
        .concat();
        let mut f = Framer::new();
        let frames = f.start(&stream, false, 0.0);
        assert_eq!(frames.len(), 2);

        f.desync();
        let after_gap = [
            vec![0x55; 10],
            heartbeat(),
            encode_frame([0x04, 0x38], &[3]),
        ]
        .concat();
        let frames = f.feed(&after_gap, 10.0);
        let ops: Vec<_> = frames.iter().map(Frame::opcode_hex).collect();
        assert_eq!(ops, vec!["00 36", "04 38"]);
        assert!(f.stats().resyncs >= 1);
    }

    #[test]
    fn a_stalled_frame_loses_position_instead_of_waiting_forever() {
        let mut f = Framer::new();
        f.start(&heartbeat(), true, 0.0);
        // A length promising 200 bytes that never come.
        let mut bogus = encode_frame([0x41, 0x36], &[0; 200]);
        bogus.truncate(20);
        assert!(f.feed(&bogus, 0.0).is_empty());
        let frames = f.feed(&[heartbeat(), heartbeat()].concat(), 3_000.0);
        assert!(!frames.is_empty());
    }
}
