//! One direction of a TCP connection, put back in order.
//!
//! Handles out-of-order segments, retransmissions, overlaps and sequence
//! wrap-around. When a segment never arrives (a capture drop), the stream
//! waits a little, then skips the hole and reports a [`Chunk::Gap`] so the
//! framer above knows its position is lost.

/// Out-of-order data kept while waiting for a missing segment.
const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;
/// How long a hole is waited for before it is skipped.
const GAP_TIMEOUT_MS: f64 = 1_500.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    Data(Vec<u8>),
    /// Bytes were lost; whatever follows does not continue what came before.
    Gap(u32),
}

#[derive(Debug, Default)]
pub struct Reassembler {
    next: Option<u32>,
    pending: Vec<(u32, Vec<u8>)>,
    pending_bytes: usize,
    hole_since: Option<f64>,
    /// The stream was seen from its SYN, i.e. from its first byte.
    pub from_start: bool,
}

/// `a - b` in sequence space (wrap-around safe).
fn seq_diff(a: u32, b: u32) -> i64 {
    a.wrapping_sub(b) as i32 as i64
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a segment captured at `t_ms`; returns what is now contiguous.
    pub fn push(&mut self, seq: u32, syn: bool, payload: &[u8], t_ms: f64) -> Vec<Chunk> {
        let mut out = Vec::new();
        let seq = if syn {
            if self.next.is_none() {
                self.from_start = true;
            }
            seq.wrapping_add(1)
        } else {
            seq
        };
        let next = *self.next.get_or_insert(seq);
        if payload.is_empty() {
            return out;
        }

        let d = seq_diff(seq, next);
        if d + payload.len() as i64 <= 0 {
            return out; // pure retransmission
        }
        if d <= 0 {
            let fresh = &payload[(-d) as usize..];
            self.next = Some(next.wrapping_add(fresh.len() as u32));
            out.push(Chunk::Data(fresh.to_vec()));
            self.drain_pending(&mut out);
        } else {
            self.pending_bytes += payload.len();
            self.pending.push((seq, payload.to_vec()));
            self.hole_since.get_or_insert(t_ms);
        }

        let waited_too_long = self
            .hole_since
            .is_some_and(|since| t_ms - since > GAP_TIMEOUT_MS);
        if !self.pending.is_empty() && (waited_too_long || self.pending_bytes > MAX_PENDING_BYTES) {
            self.skip_hole(&mut out);
        }
        if self.pending.is_empty() {
            self.hole_since = None;
        }
        out
    }

    fn drain_pending(&mut self, out: &mut Vec<Chunk>) {
        loop {
            let next = self.next.expect("set before draining");
            let mut progressed = false;
            let mut i = 0;
            while i < self.pending.len() {
                let (seq, ref data) = self.pending[i];
                let d = seq_diff(seq, next);
                if d + data.len() as i64 <= 0 {
                    // Entirely old: drop.
                    self.pending_bytes -= data.len();
                    self.pending.swap_remove(i);
                } else if d <= 0 {
                    let (_, data) = self.pending.swap_remove(i);
                    self.pending_bytes -= data.len();
                    let fresh = &data[(-d) as usize..];
                    self.next = Some(next.wrapping_add(fresh.len() as u32));
                    out.push(Chunk::Data(fresh.to_vec()));
                    progressed = true;
                    break;
                } else {
                    i += 1;
                }
            }
            if !progressed {
                return;
            }
        }
    }

    fn skip_hole(&mut self, out: &mut Vec<Chunk>) {
        let next = self.next.expect("set before skipping");
        let earliest = self
            .pending
            .iter()
            .map(|(s, _)| *s)
            .min_by_key(|s| seq_diff(*s, next))
            .expect("pending is not empty");
        out.push(Chunk::Gap(earliest.wrapping_sub(next)));
        self.next = Some(earliest);
        self.hole_since = None;
        self.drain_pending(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(chunks: &[Chunk]) -> Vec<u8> {
        chunks
            .iter()
            .flat_map(|c| match c {
                Chunk::Data(d) => d.clone(),
                Chunk::Gap(_) => b"|".to_vec(),
            })
            .collect()
    }

    #[test]
    fn in_order_and_syn() {
        let mut r = Reassembler::new();
        assert!(r.push(999, true, b"", 0.0).is_empty());
        assert!(r.from_start);
        assert_eq!(data(&r.push(1000, false, b"abc", 0.0)), b"abc");
        assert_eq!(data(&r.push(1003, false, b"def", 0.0)), b"def");
    }

    #[test]
    fn reorders_and_drops_retransmissions_and_overlaps() {
        let mut r = Reassembler::new();
        let mut all = Vec::new();
        all.extend(r.push(100, false, b"ab", 0.0));
        all.extend(r.push(104, false, b"ef", 0.0)); // early
        all.extend(r.push(100, false, b"ab", 0.0)); // retransmit
        all.extend(r.push(101, false, b"bcd", 0.0)); // overlap, fills the hole
        all.extend(r.push(105, false, b"fgh", 0.0)); // overlap past the end
        assert_eq!(data(&all), b"abcdefgh");
    }

    #[test]
    fn wraps_around_the_sequence_space() {
        let mut r = Reassembler::new();
        let start = u32::MAX - 1;
        let mut all = r.push(start, false, b"xy", 0.0);
        all.extend(r.push(start.wrapping_add(4), false, b"!!", 0.0));
        all.extend(r.push(start.wrapping_add(2), false, b"z1", 0.0));
        assert_eq!(data(&all), b"xyz1!!");
    }

    #[test]
    fn a_lost_segment_becomes_a_gap_after_the_timeout() {
        let mut r = Reassembler::new();
        r.push(0, false, b"aa", 0.0);
        assert!(r.push(10, false, b"bb", 100.0).is_empty());
        let out = r.push(12, false, b"cc", 2_000.0);
        assert_eq!(out[0], Chunk::Gap(8));
        assert_eq!(data(&out), b"|bbcc");
    }
}
