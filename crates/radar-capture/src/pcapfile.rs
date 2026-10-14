//! Reading and writing capture files.
//!
//! * Writes classic `.pcap` (little-endian, microseconds). Wireshark opens it.
//! * Reads classic `.pcap` (either byte order, µs or ns) and `.pcapng`, the
//!   format Wireshark saves by default, so a session recorded in Wireshark can
//!   be analysed and replayed here.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// Capture time, ms since the Unix epoch.
    pub t_ms: f64,
    pub linktype: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum PcapError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("not a pcap or pcapng file")]
    UnknownFormat,
    #[error("corrupt capture file: {0}")]
    Corrupt(String),
}

/// Reads every record of a `.pcap` or `.pcapng` file. A truncated last record
/// is treated as the end of the file.
pub fn read_file(path: &Path) -> Result<Vec<Record>, PcapError> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    read_bytes(&bytes)
}

pub fn read_bytes(b: &[u8]) -> Result<Vec<Record>, PcapError> {
    match b.get(..4) {
        Some([0xD4, 0xC3, 0xB2, 0xA1]) => read_classic(b, false, 1e-3),
        Some([0xA1, 0xB2, 0xC3, 0xD4]) => read_classic(b, true, 1e-3),
        Some([0x4D, 0x3C, 0xB2, 0xA1]) => read_classic(b, false, 1e-6),
        Some([0xA1, 0xB2, 0x3C, 0x4D]) => read_classic(b, true, 1e-6),
        Some([0x0A, 0x0D, 0x0D, 0x0A]) => read_pcapng(b),
        _ => Err(PcapError::UnknownFormat),
    }
}

fn u16_at(b: &[u8], at: usize, be: bool) -> Option<u16> {
    let v: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
    Some(if be {
        u16::from_be_bytes(v)
    } else {
        u16::from_le_bytes(v)
    })
}

fn u32_at(b: &[u8], at: usize, be: bool) -> Option<u32> {
    let v: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(if be {
        u32::from_be_bytes(v)
    } else {
        u32::from_le_bytes(v)
    })
}

/// `frac_to_ms`: 1e-3 for microsecond files, 1e-6 for nanosecond ones.
fn read_classic(b: &[u8], be: bool, frac_to_ms: f64) -> Result<Vec<Record>, PcapError> {
    let linktype = u32_at(b, 20, be).ok_or_else(|| PcapError::Corrupt("short header".into()))?;
    let mut out = Vec::new();
    let mut at = 24;
    while at + 16 <= b.len() {
        let sec = u32_at(b, at, be).unwrap_or(0) as f64;
        let frac = u32_at(b, at + 4, be).unwrap_or(0) as f64;
        let caplen = u32_at(b, at + 8, be).unwrap_or(0) as usize;
        let Some(data) = b.get(at + 16..at + 16 + caplen) else {
            break; // truncated tail
        };
        out.push(Record {
            t_ms: sec * 1000.0 + frac * frac_to_ms,
            linktype: linktype & 0x0FFF_FFFF,
            data: data.to_vec(),
        });
        at += 16 + caplen;
    }
    Ok(out)
}

fn read_pcapng(b: &[u8]) -> Result<Vec<Record>, PcapError> {
    struct Iface {
        linktype: u32,
        /// Seconds per timestamp unit.
        unit: f64,
    }
    let mut out = Vec::new();
    let mut ifaces: Vec<Iface> = Vec::new();
    let mut be = false;
    let mut at = 0;
    while at + 12 <= b.len() {
        let block_type = u32_at(b, at, be).unwrap_or(0);
        if block_type == 0x0A0D_0D0A {
            // Section header: its byte-order magic decides endianness.
            be = match b.get(at + 8..at + 12) {
                Some([0x1A, 0x2B, 0x3C, 0x4D]) => true,
                Some([0x4D, 0x3C, 0x2B, 0x1A]) => false,
                _ => return Err(PcapError::Corrupt("bad section byte order".into())),
            };
            ifaces.clear();
        }
        let len = u32_at(b, at + 4, be).unwrap_or(0) as usize;
        if len < 12 || at + len > b.len() {
            break; // truncated tail
        }
        let block = &b[at..at + len];
        match block_type {
            1 => {
                let linktype = u16_at(block, 8, be).unwrap_or(0) as u32;
                let mut unit = 1e-6;
                // Options start at 16: look for if_tsresol (code 9).
                let mut o = 16;
                while o + 4 <= len - 4 {
                    let code = u16_at(block, o, be).unwrap_or(0);
                    let olen = u16_at(block, o + 2, be).unwrap_or(0) as usize;
                    if code == 0 {
                        break;
                    }
                    if code == 9 && olen >= 1 {
                        let r = block[o + 4];
                        unit = if r & 0x80 != 0 {
                            2f64.powi(-((r & 0x7F) as i32))
                        } else {
                            10f64.powi(-(r as i32))
                        };
                    }
                    o += 4 + olen.div_ceil(4) * 4;
                }
                ifaces.push(Iface { linktype, unit });
            }
            6 => {
                let iface = u32_at(block, 8, be).unwrap_or(0) as usize;
                let hi = u32_at(block, 12, be).unwrap_or(0) as u64;
                let lo = u32_at(block, 16, be).unwrap_or(0) as u64;
                let caplen = u32_at(block, 20, be).unwrap_or(0) as usize;
                let Some(i) = ifaces.get(iface) else {
                    return Err(PcapError::Corrupt(format!(
                        "packet for unknown interface {iface}"
                    )));
                };
                if let Some(data) = block.get(28..28 + caplen) {
                    out.push(Record {
                        t_ms: ((hi << 32) | lo) as f64 * i.unit * 1000.0,
                        linktype: i.linktype,
                        data: data.to_vec(),
                    });
                }
            }
            3 => {
                // Simple packet block: no timestamp, interface 0.
                let caplen = u32_at(block, 8, be).unwrap_or(0) as usize;
                if let (Some(i), Some(data)) =
                    (ifaces.first(), block.get(12..(12 + caplen).min(len - 4)))
                {
                    out.push(Record {
                        t_ms: 0.0,
                        linktype: i.linktype,
                        data: data.to_vec(),
                    });
                }
            }
            _ => {}
        }
        at += len;
    }
    Ok(out)
}

/// Streams records into a classic `.pcap` file.
pub struct PcapWriter<W: Write> {
    out: W,
    bytes: u64,
}

impl PcapWriter<BufWriter<File>> {
    pub fn create(path: &Path, linktype: u32) -> io::Result<Self> {
        PcapWriter::new(BufWriter::new(File::create(path)?), linktype)
    }
}

impl<W: Write> PcapWriter<W> {
    pub fn new(mut out: W, linktype: u32) -> io::Result<Self> {
        out.write_all(&0xA1B2_C3D4u32.to_le_bytes())?;
        out.write_all(&2u16.to_le_bytes())?;
        out.write_all(&4u16.to_le_bytes())?;
        out.write_all(&[0u8; 8])?; // thiszone, sigfigs
        out.write_all(&262_144u32.to_le_bytes())?; // snaplen
        out.write_all(&linktype.to_le_bytes())?;
        Ok(Self { out, bytes: 24 })
    }

    pub fn write(&mut self, t_ms: f64, data: &[u8]) -> io::Result<()> {
        let us = (t_ms * 1000.0).max(0.0) as u64;
        self.out
            .write_all(&((us / 1_000_000) as u32).to_le_bytes())?;
        self.out
            .write_all(&((us % 1_000_000) as u32).to_le_bytes())?;
        self.out.write_all(&(data.len() as u32).to_le_bytes())?;
        self.out.write_all(&(data.len() as u32).to_le_bytes())?;
        self.out.write_all(data)?;
        self.bytes += 16 + data.len() as u64;
        Ok(())
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::linktype;

    #[test]
    fn classic_round_trip() {
        let mut buf = Vec::new();
        let mut w = PcapWriter::new(&mut buf, linktype::RAW).unwrap();
        w.write(1_760_000_000_123.5, b"abc").unwrap();
        w.write(1_760_000_001_000.0, b"").unwrap();
        w.flush().unwrap();
        let recs = read_bytes(&buf).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].data, b"abc");
        assert_eq!(recs[0].linktype, linktype::RAW);
        assert!((recs[0].t_ms - 1_760_000_000_123.5).abs() < 0.01);
    }

    #[test]
    fn truncated_tail_is_the_end() {
        let mut buf = Vec::new();
        let mut w = PcapWriter::new(&mut buf, 1).unwrap();
        w.write(0.0, b"abcdef").unwrap();
        w.write(0.0, b"ghijkl").unwrap();
        buf.truncate(buf.len() - 3);
        assert_eq!(read_bytes(&buf).unwrap().len(), 1);
    }

    #[test]
    fn rejects_unknown_formats() {
        assert!(matches!(
            read_bytes(b"hello world"),
            Err(PcapError::UnknownFormat)
        ));
    }

    fn le(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    #[test]
    fn reads_pcapng_with_nanosecond_resolution() {
        let mut b = Vec::new();
        // Section header block (28 bytes).
        b.extend(le(0x0A0D_0D0A));
        b.extend(le(28));
        b.extend(le(0x1A2B_3C4D));
        b.extend([1, 0, 0, 0]);
        b.extend([0xFF; 8]);
        b.extend(le(28));
        // Interface description: Ethernet, if_tsresol = 9 (ns).
        b.extend(le(1));
        b.extend(le(32));
        b.extend([1, 0, 0, 0]);
        b.extend(le(0));
        b.extend([9, 0, 1, 0, 9, 0, 0, 0]);
        b.extend([0, 0, 0, 0]); // opt_endofopt
        b.extend(le(32));
        // Enhanced packet block with 5 bytes of data (padded to 8).
        let ts: u64 = 1_760_000_000_500_000_000; // ns
        b.extend(le(6));
        b.extend(le(40));
        b.extend(le(0));
        b.extend(le((ts >> 32) as u32));
        b.extend(le(ts as u32));
        b.extend(le(5));
        b.extend(le(5));
        b.extend(*b"hello\0\0\0");
        b.extend(le(40));

        let recs = read_bytes(&b).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].data, b"hello");
        assert_eq!(recs[0].linktype, 1);
        assert!(
            (recs[0].t_ms - 1_760_000_000_500.0).abs() < 0.01,
            "{}",
            recs[0].t_ms
        );
    }
}
