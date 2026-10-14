//! Opcode census: every packet type seen, how often, how big.
//!
//! This is the first thing to look at on a new game patch: it shows whether
//! the framing holds (a sane list of opcodes, not noise) and which packet
//! types exist to investigate.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::framing::Frame;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpcodeRow {
    pub opcode: String,
    pub count: u64,
    pub min_len: usize,
    pub max_len: usize,
    pub bundled: u64,
    pub first_ms: f64,
    pub last_ms: f64,
    /// The profile decodes this opcode.
    pub decoded: bool,
}

#[derive(Debug, Default)]
pub struct Census {
    rows: BTreeMap<[u8; 2], OpcodeRow>,
}

impl Census {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, frame: &Frame, t_ms: f64) {
        let len = frame.payload.len();
        let row = self.rows.entry(frame.opcode).or_insert_with(|| OpcodeRow {
            opcode: frame.opcode_hex(),
            count: 0,
            min_len: len,
            max_len: len,
            bundled: 0,
            first_ms: t_ms,
            last_ms: t_ms,
            decoded: false,
        });
        row.count += 1;
        row.min_len = row.min_len.min(len);
        row.max_len = row.max_len.max(len);
        row.bundled += u64::from(frame.bundled);
        row.last_ms = t_ms;
    }

    pub fn mark_decoded(&mut self, opcode: [u8; 2]) {
        if let Some(r) = self.rows.get_mut(&opcode) {
            r.decoded = true;
        }
    }

    /// Rows, most frequent first.
    pub fn rows(&self) -> Vec<OpcodeRow> {
        let mut v: Vec<_> = self.rows.values().cloned().collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then(a.opcode.cmp(&b.opcode)));
        v
    }

    pub fn total(&self) -> u64 {
        self.rows.values().map(|r| r.count).sum()
    }

    /// A fixed-width text table.
    pub fn table(&self, limit: usize) -> String {
        let total = self.total().max(1) as f64;
        let mut s = format!(
            "{:<7} {:>9} {:>7} {:>13} {:>8}  {}\n",
            "opcode", "count", "share", "size", "bundled", "decoded"
        );
        for r in self.rows().into_iter().take(limit) {
            s += &format!(
                "{:<7} {:>9} {:>6.2}% {:>6}-{:<6} {:>8}  {}\n",
                r.opcode,
                r.count,
                r.count as f64 * 100.0 / total,
                r.min_len,
                r.max_len,
                r.bundled,
                if r.decoded { "yes" } else { "" }
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(op: [u8; 2], len: usize, bundled: bool) -> Frame {
        let mut payload = vec![0; len];
        payload[..2].copy_from_slice(&op);
        Frame {
            opcode: op,
            payload,
            bundled,
        }
    }

    #[test]
    fn counts_sizes_and_orders_by_frequency() {
        let mut c = Census::new();
        c.observe(&f([0, 0x36], 10, false), 1.0);
        c.observe(&f([0x41, 0x36], 40, false), 2.0);
        c.observe(&f([0x41, 0x36], 90, true), 3.0);
        let rows = c.rows();
        assert_eq!(rows[0].opcode, "41 36");
        assert_eq!(
            (
                rows[0].count,
                rows[0].min_len,
                rows[0].max_len,
                rows[0].bundled
            ),
            (2, 40, 90, 1)
        );
        assert_eq!(c.total(), 3);
        assert!(c.table(10).contains("41 36"));
    }
}
