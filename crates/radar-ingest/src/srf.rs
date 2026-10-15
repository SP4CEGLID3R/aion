//! SRF — **Synthetic** Radar Frames.
//!
//! A small binary framing format invented for this project so that the
//! byte-stream → parser → structured event → adapter → tracker chain can be
//! developed and tested without capturing any game traffic.
//!
//! **SRF is not AION 2's protocol and says nothing about it.** It exists only
//! to exercise the same problems a real stream parser has: partial reads,
//! corrupted bytes, resynchronisation, unknown opcodes and length checks.
//!
//! ```text
//! offset size  field
//! 0      2     magic "SR" (0x53 0x52)
//! 2      1     version (1)
//! 3      1     opcode
//! 4      2     payload length N, u16 little-endian, N <= 512
//! 6      8     timestamp, i64 little-endian, ms since the Unix epoch
//! 14     N     payload
//! ```
//!
//! | opcode | event              | payload                                   |
//! |--------|--------------------|-------------------------------------------|
//! | 0x01   | player             | x, y, z, heading: f32 LE (heading NaN = unknown) |
//! | 0x10   | cube spawned       | id u32, x, y, z f32                        |
//! | 0x11   | cube seen          | id u32                                     |
//! | 0x12   | cube removed       | id u32, reason u8 (0 despawned, 1 collected) |
//! | 0x20   | zone changed       | UTF-8 zone name, 1–64 bytes                |
//! | 0x30   | static spawn point | id u32, x, y, z f32                        |

use thiserror::Error;

use crate::wire::{WireEvent, WireId, WireProvenance};

pub const MAGIC: [u8; 2] = *b"SR";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 14;
pub const MAX_PAYLOAD: usize = 512;
/// The decoder never buffers more than this; beyond it, input is discarded.
const MAX_BUFFER: usize = 64 * 1024;

pub mod opcode {
    pub const PLAYER: u8 = 0x01;
    pub const CUBE_SPAWNED: u8 = 0x10;
    pub const CUBE_SEEN: u8 = 0x11;
    pub const CUBE_REMOVED: u8 = 0x12;
    pub const ZONE_CHANGED: u8 = 0x20;
    pub const SPAWN_POINT: u8 = 0x30;
}

#[derive(Debug, Error, PartialEq)]
pub enum DecodeError {
    #[error("unsupported SRF version {0}")]
    Version(u8),
    #[error("payload length {0} exceeds {MAX_PAYLOAD}")]
    Oversized(usize),
    #[error("opcode 0x{opcode:02x} expects {expected} payload bytes, got {got}")]
    BadLength {
        opcode: u8,
        expected: usize,
        got: usize,
    },
    #[error("unknown opcode 0x{0:02x}")]
    UnknownOpcode(u8),
    #[error("removal reason {0} is not 0 or 1")]
    BadReason(u8),
    #[error("zone name is empty, too long, or not UTF-8")]
    BadZone,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DecoderStats {
    pub frames: u64,
    pub errors: u64,
    /// Bytes thrown away while looking for the next frame.
    pub skipped_bytes: u64,
    pub unknown_opcodes: u64,
}

/// Incremental decoder: feed it bytes in any chunking, pull frames out.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    stats: DecoderStats,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> &DecoderStats {
        &self.stats
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > MAX_BUFFER {
            // A stream that never frames: keep only the tail, which may hold
            // the start of a frame.
            let drop = self.buf.len() - HEADER_LEN.max(1);
            self.buf.drain(..drop);
            self.stats.skipped_bytes += drop as u64;
        }
    }

    /// Returns the next complete frame, `None` when more bytes are needed.
    pub fn next_frame(&mut self) -> Option<Result<WireEvent, DecodeError>> {
        loop {
            self.resync();
            if self.buf.len() < HEADER_LEN {
                return None;
            }
            let version = self.buf[2];
            let op = self.buf[3];
            let len = u16::from_le_bytes([self.buf[4], self.buf[5]]) as usize;
            if version != VERSION {
                self.skip(1);
                self.stats.errors += 1;
                return Some(Err(DecodeError::Version(version)));
            }
            if len > MAX_PAYLOAD {
                // The length is garbage, so this is not a frame start.
                self.skip(1);
                self.stats.errors += 1;
                return Some(Err(DecodeError::Oversized(len)));
            }
            if self.buf.len() < HEADER_LEN + len {
                return None;
            }
            let t = i64::from_le_bytes(self.buf[6..14].try_into().expect("8 bytes"));
            let payload: Vec<u8> = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
            self.buf.drain(..HEADER_LEN + len);

            let result = decode_payload(op, t, &payload);
            match &result {
                Ok(_) => self.stats.frames += 1,
                Err(DecodeError::UnknownOpcode(_)) => {
                    self.stats.unknown_opcodes += 1;
                    continue; // like an opcode census: count it and move on
                }
                Err(_) => self.stats.errors += 1,
            }
            return Some(result);
        }
    }

    /// Drops bytes until the buffer starts with the magic (or could, given
    /// one more byte).
    fn resync(&mut self) {
        if self.buf.starts_with(&MAGIC) {
            return;
        }
        let found = self.buf.windows(2).position(|w| w == MAGIC);
        let drop = match found {
            Some(i) => i,
            // Keep a trailing 'S': it may be the first half of the magic.
            None if self.buf.last() == Some(&MAGIC[0]) => self.buf.len() - 1,
            None => self.buf.len(),
        };
        self.skip(drop);
    }

    fn skip(&mut self, n: usize) {
        self.buf.drain(..n);
        self.stats.skipped_bytes += n as u64;
    }
}

/// Decodes a complete byte buffer.
pub fn decode_all(bytes: &[u8]) -> (Vec<Result<WireEvent, DecodeError>>, DecoderStats) {
    let mut d = FrameDecoder::new();
    d.push(bytes);
    let mut out = Vec::new();
    while let Some(f) = d.next_frame() {
        out.push(f);
    }
    (out, d.stats.clone())
}

fn f32_at(p: &[u8], i: usize) -> f64 {
    f32::from_le_bytes(p[i..i + 4].try_into().expect("4 bytes")) as f64
}

fn u32_at(p: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(p[i..i + 4].try_into().expect("4 bytes"))
}

fn expect_len(op: u8, p: &[u8], expected: usize) -> Result<(), DecodeError> {
    if p.len() != expected {
        return Err(DecodeError::BadLength {
            opcode: op,
            expected,
            got: p.len(),
        });
    }
    Ok(())
}

fn decode_payload(op: u8, t: i64, p: &[u8]) -> Result<WireEvent, DecodeError> {
    let id = |p: &[u8]| Some(WireId::Num(u32_at(p, 0) as u64));
    let xyz = |p: &[u8], at: usize| [f32_at(p, at), f32_at(p, at + 4), f32_at(p, at + 8)];
    match op {
        opcode::PLAYER => {
            expect_len(op, p, 16)?;
            let heading = f32_at(p, 12);
            Ok(WireEvent::Player {
                accuracy: None,
                height_accuracy: None,
                t,
                pos: xyz(p, 0),
                heading: (!heading.is_nan()).then_some(heading),
                zone: None,
            })
        }
        opcode::CUBE_SPAWNED => {
            expect_len(op, p, 16)?;
            Ok(WireEvent::CubeSpawned {
                t,
                id: id(p),
                pos: xyz(p, 4),
                label: None,
                zone: None,
                provenance: WireProvenance::Live,
            })
        }
        opcode::CUBE_SEEN => {
            expect_len(op, p, 4)?;
            Ok(WireEvent::CubeSeen {
                t,
                id: id(p),
                pos: None,
                zone: None,
                provenance: WireProvenance::Live,
            })
        }
        opcode::CUBE_REMOVED => {
            expect_len(op, p, 5)?;
            match p[4] {
                0 => Ok(WireEvent::CubeDespawned {
                    t,
                    id: id(p),
                    zone: None,
                }),
                1 => Ok(WireEvent::CubeCollected {
                    t,
                    id: id(p),
                    zone: None,
                }),
                r => Err(DecodeError::BadReason(r)),
            }
        }
        opcode::ZONE_CHANGED => {
            let zone = std::str::from_utf8(p).map_err(|_| DecodeError::BadZone)?;
            if zone.is_empty() || zone.len() > 64 {
                return Err(DecodeError::BadZone);
            }
            Ok(WireEvent::ZoneChanged {
                t,
                zone: zone.to_owned(),
            })
        }
        opcode::SPAWN_POINT => {
            expect_len(op, p, 16)?;
            Ok(WireEvent::SpawnPoint {
                t: Some(t),
                id: id(p),
                pos: xyz(p, 4),
                label: None,
                zone: None,
            })
        }
        other => Err(DecodeError::UnknownOpcode(other)),
    }
}

/// Builds one frame. Used by tests and by the fixture generator.
pub fn encode(op: u8, t: i64, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= MAX_PAYLOAD, "payload too large");
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(op);
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(&t.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Payload helpers for building frames.
pub mod build {
    use super::*;

    fn xyz(out: &mut Vec<u8>, pos: [f32; 3]) {
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    pub fn player(t: i64, pos: [f32; 3], heading: Option<f32>) -> Vec<u8> {
        let mut p = Vec::new();
        xyz(&mut p, pos);
        p.extend_from_slice(&heading.unwrap_or(f32::NAN).to_le_bytes());
        encode(opcode::PLAYER, t, &p)
    }

    pub fn cube_spawned(t: i64, id: u32, pos: [f32; 3]) -> Vec<u8> {
        let mut p = id.to_le_bytes().to_vec();
        xyz(&mut p, pos);
        encode(opcode::CUBE_SPAWNED, t, &p)
    }

    pub fn cube_seen(t: i64, id: u32) -> Vec<u8> {
        encode(opcode::CUBE_SEEN, t, &id.to_le_bytes())
    }

    pub fn cube_removed(t: i64, id: u32, collected: bool) -> Vec<u8> {
        let mut p = id.to_le_bytes().to_vec();
        p.push(collected as u8);
        encode(opcode::CUBE_REMOVED, t, &p)
    }

    pub fn zone_changed(t: i64, zone: &str) -> Vec<u8> {
        encode(opcode::ZONE_CHANGED, t, zone.as_bytes())
    }

    pub fn spawn_point(t: i64, id: u32, pos: [f32; 3]) -> Vec<u8> {
        let mut p = id.to_le_bytes().to_vec();
        xyz(&mut p, pos);
        encode(opcode::SPAWN_POINT, t, &p)
    }
}

/// Parses the `.srf.hex` fixture format: hex bytes, any whitespace, `#`
/// comments to end of line.
pub fn parse_hex_fixture(text: &str) -> Result<Vec<u8>, String> {
    let mut digits = String::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        digits.extend(line.chars().filter(|c| !c.is_whitespace()));
    }
    if !digits.len().is_multiple_of(2) {
        return Err("odd number of hex digits".into());
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&digits[i..i + 2], 16)
                .map_err(|_| format!("bad hex at digit {i}: {:?}", &digits[i..i + 2]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> Vec<u8> {
        [
            build::zone_changed(1_000, "synthetic-zone"),
            build::player(1_000, [0.0, 0.0, 0.0], Some(90.0)),
            build::cube_spawned(1_010, 7, [10.0, 20.0, 1.5]),
            build::cube_seen(2_000, 7),
            build::spawn_point(2_000, 99, [5.0, 5.0, 0.0]),
            build::cube_removed(3_000, 7, true),
        ]
        .concat()
    }

    #[test]
    fn decodes_a_clean_stream() {
        let (frames, stats) = decode_all(&stream());
        let frames: Vec<_> = frames.into_iter().map(Result::unwrap).collect();
        assert_eq!(frames.len(), 6);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.skipped_bytes, 0);
        assert_eq!(
            frames[2],
            WireEvent::CubeSpawned {
                t: 1_010,
                id: Some(WireId::Num(7)),
                pos: [10.0, 20.0, 1.5],
                label: None,
                zone: None,
                provenance: WireProvenance::Live,
            }
        );
        assert!(matches!(frames[4], WireEvent::SpawnPoint { .. }));
        assert!(matches!(frames[5], WireEvent::CubeCollected { .. }));
        let WireEvent::Player { heading, .. } = &frames[1] else {
            panic!()
        };
        assert_eq!(*heading, Some(90.0));
    }

    #[test]
    fn any_chunking_gives_the_same_frames() {
        let bytes = stream();
        let (expected, _) = decode_all(&bytes);
        for chunk in [1, 2, 3, 7, 13, 64] {
            let mut d = FrameDecoder::new();
            let mut got = Vec::new();
            for piece in bytes.chunks(chunk) {
                d.push(piece);
                while let Some(f) = d.next_frame() {
                    got.push(f);
                }
            }
            assert_eq!(got, expected, "chunk size {chunk}");
        }
    }

    #[test]
    fn resyncs_after_garbage_and_skips_unknown_opcodes() {
        let mut bytes = vec![0xde, 0xad, b'S', 0x00, 0xbe, 0xef];
        bytes.extend(build::cube_seen(5, 1));
        bytes.extend(encode(0x7f, 5, &[1, 2, 3])); // unknown opcode
        bytes.extend([b'S', b'R', 9, 0x11, 4, 0]); // wrong version, truncated
        bytes.extend(build::cube_seen(6, 2));
        let (frames, stats) = decode_all(&bytes);
        let ok: Vec<_> = frames.iter().filter_map(|f| f.as_ref().ok()).collect();
        assert_eq!(ok.len(), 2);
        assert_eq!(stats.unknown_opcodes, 1);
        assert!(frames.contains(&Err(DecodeError::Version(9))));
        assert!(stats.skipped_bytes >= 6);
    }

    #[test]
    fn rejects_bad_lengths_reasons_and_zones() {
        let wrong_len = encode(opcode::CUBE_SEEN, 1, &[1, 2]);
        let bad_reason = encode(opcode::CUBE_REMOVED, 1, &[1, 0, 0, 0, 5]);
        let empty_zone = encode(opcode::ZONE_CHANGED, 1, &[]);
        let not_utf8 = encode(opcode::ZONE_CHANGED, 1, &[0xff, 0xfe]);
        let (frames, stats) = decode_all(&[wrong_len, bad_reason, empty_zone, not_utf8].concat());
        assert_eq!(
            frames,
            vec![
                Err(DecodeError::BadLength {
                    opcode: opcode::CUBE_SEEN,
                    expected: 4,
                    got: 2
                }),
                Err(DecodeError::BadReason(5)),
                Err(DecodeError::BadZone),
                Err(DecodeError::BadZone),
            ]
        );
        assert_eq!(stats.errors, 4);
    }

    #[test]
    fn oversized_length_is_treated_as_noise() {
        let mut bytes = vec![b'S', b'R', VERSION, 0x10, 0xff, 0xff];
        bytes.extend([0u8; 8]);
        bytes.extend(build::cube_seen(1, 3));
        let (frames, _) = decode_all(&bytes);
        assert_eq!(frames[0], Err(DecodeError::Oversized(0xffff)));
        assert!(matches!(
            frames.last(),
            Some(Ok(WireEvent::CubeSeen { .. }))
        ));
    }

    #[test]
    fn hex_fixture_parsing() {
        assert_eq!(
            parse_hex_fixture("53 52 # magic\n01").unwrap(),
            vec![0x53, 0x52, 0x01]
        );
        assert!(parse_hex_fixture("5").is_err());
        assert!(parse_hex_fixture("zz").is_err());
    }
}
