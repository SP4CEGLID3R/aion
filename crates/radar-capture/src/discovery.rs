//! Tools for finding which packets carry cube spawns, and their layout.
//!
//! Nobody has published which AION 2 packet (if any) announces a cube. The
//! workflow here finds out from your own recordings:
//!
//! 1. Record while playing, and press a marker hotkey each time a cube
//!    appears on screen or you collect one ([`Marker`]).
//! 2. [`correlate`] ranks opcodes by how consistently they occur right around
//!    those moments compared with their usual rate.
//! 3. [`inspect`] looks at one opcode's payloads for fields that behave like
//!    coordinates (plausible floats) or ids (values that differ per packet).
//! 4. The findings go into a decoder profile ([`crate::profile`]), which is
//!    checked by replaying recordings before it is marked validated.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// ms since the Unix epoch, on the same clock as the capture.
    pub t: f64,
    /// `cube_visible`, `cube_collected`, or anything else you choose.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub fn append_marker(path: &Path, marker: &Marker) -> io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(
        f,
        "{}",
        serde_json::to_string(marker).map_err(io::Error::other)?
    )
}

pub fn read_markers(path: &Path) -> io::Result<Vec<Marker>> {
    let mut out = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Marker>(&line) {
            Ok(m) => out.push(m),
            Err(e) => log::warn!("skipping marker line {line:?}: {e}"),
        }
    }
    Ok(out)
}

/// One decoded packet from a recording.
#[derive(Debug, Clone)]
pub struct Sample {
    pub t_ms: f64,
    pub conn: u64,
    pub opcode: [u8; 2],
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub opcode: String,
    pub total: usize,
    /// Markers with at least one of these packets in their window.
    pub markers_hit: usize,
    pub hit_ratio: f64,
    /// Packets inside any marker window.
    pub in_windows: usize,
    /// How many would fall inside the windows by chance at the average rate.
    pub expected: f64,
    pub lift: f64,
    pub score: f64,
}

/// How samples are grouped for correlation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    /// By opcode alone.
    Opcode,
    /// By opcode plus the first byte after it: one opcode often carries
    /// several kinds of message (spawn, update, remove), and a cube spawn can
    /// drown among them at the opcode level.
    Subtype,
}

fn group_key(s: &Sample, g: Grouping) -> String {
    let op = format!("{:02X} {:02X}", s.opcode[0], s.opcode[1]);
    match (g, s.payload.get(2)) {
        (Grouping::Subtype, Some(b)) => format!("{op} / {b:02X}"),
        _ => op,
    }
}

/// Ranks packet groups by association with marker times. A good cube-spawn
/// candidate shows up near (almost) every "cube visible" marker and is rare
/// elsewhere: score = hit ratio × lift.
pub fn correlate(
    samples: &[Sample],
    markers: &[f64],
    before_ms: f64,
    after_ms: f64,
) -> Vec<Candidate> {
    correlate_by(samples, markers, before_ms, after_ms, Grouping::Opcode)
}

pub fn correlate_by(
    samples: &[Sample],
    markers: &[f64],
    before_ms: f64,
    after_ms: f64,
    g: Grouping,
) -> Vec<Candidate> {
    if samples.is_empty() || markers.is_empty() {
        return Vec::new();
    }
    let t0 = samples.iter().map(|s| s.t_ms).fold(f64::INFINITY, f64::min);
    let t1 = samples
        .iter()
        .map(|s| s.t_ms)
        .fold(f64::NEG_INFINITY, f64::max);
    let duration = (t1 - t0).max(1.0);
    let window_share = ((before_ms + after_ms) * markers.len() as f64 / duration).min(1.0);

    let mut groups: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for s in samples {
        groups.entry(group_key(s, g)).or_default().push(s.t_ms);
    }
    let in_window = |t: f64, m: f64| t >= m - before_ms && t <= m + after_ms;

    let mut out: Vec<Candidate> = groups
        .into_iter()
        .map(|(key, times)| {
            let markers_hit = markers
                .iter()
                .filter(|&&m| times.iter().any(|&t| in_window(t, m)))
                .count();
            let in_windows = times
                .iter()
                .filter(|&&t| markers.iter().any(|&m| in_window(t, m)))
                .count();
            let expected = times.len() as f64 * window_share;
            let hit_ratio = markers_hit as f64 / markers.len() as f64;
            let lift = in_windows as f64 / expected.max(1e-9);
            Candidate {
                opcode: key,
                total: times.len(),
                markers_hit,
                hit_ratio,
                in_windows,
                expected,
                lift,
                score: hit_ratio * lift,
            }
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.total.cmp(&b.total)));
    out
}

pub fn correlation_table(cands: &[Candidate], limit: usize) -> String {
    let mut s = format!(
        "{:<12} {:>8} {:>8} {:>7} {:>10} {:>7} {:>7}\n",
        "packet", "total", "markers", "hit %", "in window", "lift", "score"
    );
    for c in cands.iter().take(limit) {
        s += &format!(
            "{:<12} {:>8} {:>8} {:>6.0}% {:>10} {:>7.1} {:>7.1}\n",
            c.opcode,
            c.total,
            c.markers_hit,
            c.hit_ratio * 100.0,
            c.in_windows,
            c.lift,
            c.score
        );
    }
    s
}

/// Reads an unsigned LEB128 varint at `at`: (value, length).
pub fn read_varint(p: &[u8], at: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for i in 0..10 {
        let b = *p.get(at + i)?;
        v |= u64::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

fn plausible_coord(v: f32) -> bool {
    v.is_finite() && (v == 0.0 || (v.abs() > 1e-2 && v.abs() < 1e6))
}

/// Looks for fields in one opcode's payloads: constant bytes, offsets that
/// read as plausible float coordinates, and values that vary per packet (ids).
pub fn inspect(payloads: &[&[u8]]) -> String {
    let mut s = String::new();
    if payloads.is_empty() {
        return "no packets with this opcode\n".into();
    }
    let n = payloads.len();
    let mut lens: BTreeMap<usize, usize> = BTreeMap::new();
    for p in payloads {
        *lens.entry(p.len()).or_default() += 1;
    }
    s += &format!("{n} packets; lengths (bytes: count): {lens:?}\n");
    let min_len = *lens.keys().next().unwrap_or(&0);

    let constant: Vec<usize> = (0..min_len)
        .filter(|&i| payloads.iter().all(|p| p[i] == payloads[0][i]))
        .collect();
    s += &format!(
        "constant bytes at offsets: {}\n",
        constant
            .iter()
            .map(|i| format!("{i}={:02X}", payloads[0][*i]))
            .collect::<Vec<_>>()
            .join(" ")
    );

    if let Some((_, l)) = read_varint(payloads[0], 2) {
        let ids: BTreeSet<u64> = payloads
            .iter()
            .filter_map(|p| read_varint(p, 2).map(|v| v.0))
            .collect();
        s += &format!(
            "varint after the opcode (offset 2, {l} bytes in the first packet): {} distinct values in {n}\n",
            ids.len()
        );
    }

    s += "f32 LE offsets that look like coordinates in >= 90% of packets:\n";
    for off in 2..min_len.saturating_sub(3) {
        let vals: Vec<f32> = payloads
            .iter()
            .map(|p| f32::from_le_bytes(p[off..off + 4].try_into().unwrap()))
            .collect();
        let good = vals.iter().filter(|v| plausible_coord(**v)).count();
        let nonzero = vals.iter().filter(|v| **v != 0.0).count();
        if good * 10 >= n * 9 && nonzero * 2 >= n {
            let lo = vals.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = vals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let distinct: BTreeSet<u32> = vals.iter().map(|v| v.to_bits()).collect();
            s += &format!(
                "  offset {off:>4}: {lo:>12.2} .. {hi:<12.2} ({} distinct)\n",
                distinct.len()
            );
        }
    }

    s += "u32 LE offsets whose value differs in most packets (id candidates):\n";
    for off in 2..min_len.saturating_sub(3) {
        let distinct: HashMap<u32, ()> = payloads
            .iter()
            .map(|p| (u32::from_le_bytes(p[off..off + 4].try_into().unwrap()), ()))
            .collect();
        if n >= 4 && distinct.len() * 10 >= n * 8 {
            s += &format!("  offset {off:>4}: {} distinct of {n}\n", distinct.len());
        }
    }
    s
}

/// Hex dump of a payload, 16 bytes per line with offsets.
pub fn hexdump(p: &[u8]) -> String {
    p.chunks(16)
        .enumerate()
        .map(|(i, c)| {
            let hex: Vec<String> = c.iter().map(|b| format!("{b:02X}")).collect();
            format!("  {:04}: {}\n", i * 16, hex.join(" "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: f64, op: [u8; 2]) -> Sample {
        Sample {
            t_ms: t,
            conn: 1,
            opcode: op,
            payload: op.to_vec(),
        }
    }

    #[test]
    fn correlation_finds_the_packet_that_follows_markers() {
        let mut samples = Vec::new();
        // Background: a frequent packet every 100 ms for 10 minutes.
        for i in 0..6_000 {
            samples.push(sample(i as f64 * 100.0, [0x04, 0x38]));
        }
        // A rare one shortly after each marker, plus one stray.
        let markers: Vec<f64> = (1..=8).map(|i| i as f64 * 60_000.0).collect();
        for m in &markers {
            samples.push(sample(m + 300.0, [0x77, 0x36]));
        }
        samples.push(sample(5_000.0, [0x77, 0x36]));
        let ranked = correlate(&samples, &markers, 1_000.0, 2_000.0);
        assert_eq!(ranked[0].opcode, "77 36");
        assert_eq!(ranked[0].markers_hit, 8);
        assert!(ranked[0].lift > 10.0, "{}", ranked[0].lift);
        // The background packet also hits every window, but without lift.
        assert!(ranked[1].lift < 2.0);
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn inspect_finds_float_coordinates_and_ids() {
        let payloads: Vec<Vec<u8>> = (0..20u32)
            .map(|i| {
                let mut p = vec![0x77, 0x36];
                p.push((i + 1) as u8); // varint id
                p.extend((1000 + i * 7).to_le_bytes()); // u32 id
                p.extend((1234.5f32 + i as f32 * 3.0).to_le_bytes());
                p.extend((-560.25f32 - i as f32).to_le_bytes());
                p.extend([0xAA, 0xBB]);
                p
            })
            .collect();
        let refs: Vec<&[u8]> = payloads.iter().map(Vec::as_slice).collect();
        let report = inspect(&refs);
        assert!(report.contains("offset    7"), "{report}");
        assert!(report.contains("offset   11"), "{report}");
        assert!(report.contains("20 distinct values in 20"), "{report}");
        assert!(report.contains("15=AA"), "{report}");
    }

    #[test]
    fn markers_round_trip_through_a_file() {
        let dir = std::env::temp_dir().join(format!("radar-markers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.jsonl");
        let _ = std::fs::remove_file(&path);
        append_marker(
            &path,
            &Marker {
                t: 1.0,
                kind: "cube_visible".into(),
                note: None,
            },
        )
        .unwrap();
        append_marker(
            &path,
            &Marker {
                t: 2.0,
                kind: "cube_collected".into(),
                note: Some("x".into()),
            },
        )
        .unwrap();
        let m = read_markers(&path).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].note.as_deref(), Some("x"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
