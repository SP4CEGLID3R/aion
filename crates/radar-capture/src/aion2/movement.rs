//! Learning where the game client reports the player's own movement.
//!
//! The server only tells us our position with events (buffs, being hit,
//! loading screens), seconds or minutes apart. The client, though, must tell
//! the server where it is while it moves. Which packet and which bytes carry
//! that is not documented anywhere, so instead of guessing, this module
//! *learns* it: every client frame is scanned for float triplets that sit
//! within a few metres of a server-confirmed position taken at the same
//! moment. A candidate (opcode, anchor, offset) is adopted only after it has
//! matched several different confirmed positions with few misses, and it is
//! dropped again if later confirmed positions disagree with it.
//!
//! If the client's traffic is encrypted or uses another encoding, nothing is
//! ever adopted and the radar keeps working from server fixes alone.

use std::collections::{HashMap, HashSet, VecDeque};

use radar_core::Millis;
use serde::{Deserialize, Serialize};

use super::is_world_position;

/// Where a field is counted from: the frame's first byte (the opcode) or its
/// last byte (for layouts with variable-length fields in front).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Start,
    End,
}

/// A place in client frames that holds the own position (x y z f32).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveSource {
    /// Two bytes on the wire, hex ("1c37").
    pub opcode: String,
    pub anchor: Anchor,
    /// Byte offset of x: from the opcode (Start) or back from the frame end
    /// to the start of x (End).
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    op: [u8; 2],
    anchor: Anchor,
    offset: usize,
}

impl Key {
    fn read(&self, payload: &[u8]) -> Option<[f32; 3]> {
        let at = match self.anchor {
            Anchor::Start => self.offset,
            Anchor::End => payload.len().checked_sub(self.offset)?,
        };
        let b = payload.get(at..at + 12)?;
        let f = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().expect("4 bytes"));
        Some([f(0), f(4), f(8)])
    }

    fn to_source(self) -> MoveSource {
        MoveSource {
            opcode: format!("{:02x}{:02x}", self.op[0], self.op[1]),
            anchor: self.anchor,
            offset: self.offset,
        }
    }

    fn from_source(s: &MoveSource) -> Option<Key> {
        let b = crate::profile::parse_hex(&s.opcode).filter(|b| b.len() == 2)?;
        Some(Key {
            op: [b[0], b[1]],
            anchor: s.anchor,
            offset: s.offset,
        })
    }
}

#[derive(Debug, Default)]
struct Candidate {
    hits: u32,
    misses: u32,
    fixes: HashSet<Millis>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovementStatus {
    /// "learning", "found" or "off".
    pub state: String,
    pub source: Option<MoveSource>,
    pub updates: u64,
    pub client_frames: u64,
    /// Server-confirmed positions available for learning so far.
    pub fixes_seen: usize,
}

/// Largest client frame scanned while learning.
const MAX_SCAN_LEN: usize = 600;
const MAX_CANDIDATES: usize = 60_000;
/// A server fix this close in time to a client frame is used to learn.
const LEARN_WINDOW_MS: Millis = 1_000;
/// Hits, distinct fixes and hit share needed to adopt a candidate.
const ADOPT_HITS: u32 = 6;
const ADOPT_FIXES: usize = 4;
const ADOPT_SHARE: f64 = 0.75;
/// Consecutive disagreements with server fixes that drop an adopted source.
const DROP_AFTER: u32 = 5;
/// Fastest believable movement, units (cm) per second, plus slack.
const MAX_SPEED: f32 = 4_000.0;
const EMIT_EVERY_MS: Millis = 100;

pub struct MovementLearner {
    enabled: bool,
    pinned: bool,
    source: Option<Key>,
    candidates: HashMap<Key, Candidate>,
    fixes: VecDeque<([f32; 3], Millis)>,
    fixes_total: usize,
    last_client: Option<([f32; 3], Millis)>,
    last_emit: Option<Millis>,
    disagreements: u32,
    updates: u64,
    client_frames: u64,
}

fn planar(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

impl MovementLearner {
    /// `pinned` is used and never dropped; `hint` (learned earlier) is used
    /// at once but dropped like a learned one if the server disagrees.
    pub fn new(enabled: bool, pinned: Option<&MoveSource>, hint: Option<&MoveSource>) -> Self {
        let pinned = pinned.and_then(Key::from_source);
        let source = pinned.or_else(|| hint.filter(|_| enabled).and_then(Key::from_source));
        MovementLearner {
            enabled,
            pinned: pinned.is_some(),
            source,
            candidates: HashMap::new(),
            fixes: VecDeque::new(),
            fixes_total: 0,
            last_client: None,
            last_emit: None,
            disagreements: 0,
            updates: 0,
            client_frames: 0,
        }
    }

    /// A position confirmed by the server (event-based own fix).
    pub fn on_server_fix(&mut self, p: [f32; 3], t: Millis) {
        self.fixes.push_back((p, t));
        self.fixes_total += 1;
        while self.fixes.len() > 64 {
            self.fixes.pop_front();
        }
        // Keep an adopted source honest.
        if let (Some(_), Some((cp, ct))) = (self.source, self.last_client) {
            if (t - ct).abs() <= LEARN_WINDOW_MS {
                if planar(cp, p) > 1_500.0 + MAX_SPEED * (t - ct).abs() as f32 / 1000.0 {
                    self.disagreements += 1;
                    if self.disagreements >= DROP_AFTER && !self.pinned {
                        log::warn!("own-movement source disagrees with the server; learning again");
                        self.source = None;
                        self.candidates.clear();
                        self.last_client = None;
                        self.disagreements = 0;
                    }
                } else {
                    self.disagreements = 0;
                }
            }
        }
    }

    /// New connection: positions start over, a learned source is kept.
    pub fn on_new_connection(&mut self) {
        self.fixes.clear();
        self.last_client = None;
        self.last_emit = None;
    }

    /// A client frame (`payload` starts at the opcode). Returns a live own
    /// position (game units) when the adopted source yields one.
    pub fn on_client_frame(&mut self, op: [u8; 2], payload: &[u8], t: Millis) -> Option<[f32; 3]> {
        self.client_frames += 1;
        if let Some(key) = self.source {
            if key.op != op {
                return None;
            }
            let p = key.read(payload).filter(|p| is_world_position(*p))?;
            if let Some((lp, lt)) = self.last_client {
                let dt = ((t - lt).max(0) as f32 / 1000.0).max(0.05);
                if planar(lp, p) > 1_000.0 + MAX_SPEED * dt {
                    return None; // a jump no one can run: not trusted
                }
            }
            self.last_client = Some((p, t));
            if self.last_emit.is_some_and(|e| t - e < EMIT_EVERY_MS) {
                return None;
            }
            self.last_emit = Some(t);
            self.updates += 1;
            return Some(p);
        }
        if self.enabled {
            self.learn(op, payload, t);
        }
        None
    }

    fn learn(&mut self, op: [u8; 2], payload: &[u8], t: Millis) {
        if payload.len() < 14 || payload.len() > MAX_SCAN_LEN {
            return;
        }
        let Some(&(fix, ft)) = self
            .fixes
            .iter()
            .filter(|(_, ft)| (t - ft).abs() <= LEARN_WINDOW_MS)
            .min_by_key(|(_, ft)| (t - ft).abs())
        else {
            return;
        };
        let tol = 300.0 + MAX_SPEED * (t - ft).abs() as f32 / 1000.0;
        let len = payload.len();
        for anchor in [Anchor::Start, Anchor::End] {
            for offset in 2..=len - 12 {
                let offset = match anchor {
                    Anchor::Start => offset,
                    Anchor::End => len - offset + 2, // 14..=len back from the end
                };
                let key = Key { op, anchor, offset };
                let Some(p) = key.read(payload) else { continue };
                if !is_world_position(p) {
                    continue;
                }
                let hit = planar(p, fix) <= tol && (p[2] - fix[2]).abs() <= tol + 500.0;
                if !hit && !self.candidates.contains_key(&key) {
                    continue; // only track misses for something that ever hit
                }
                if self.candidates.len() >= MAX_CANDIDATES {
                    self.candidates.retain(|_, c| c.hits >= 2);
                }
                let c = self.candidates.entry(key).or_default();
                if hit {
                    c.hits += 1;
                    if c.fixes.len() < 64 {
                        c.fixes.insert(ft);
                    }
                } else {
                    c.misses += 1;
                }
            }
        }
        let best = self
            .candidates
            .iter()
            .filter(|(_, c)| {
                c.hits >= ADOPT_HITS
                    && c.fixes.len() >= ADOPT_FIXES
                    && c.hits as f64 >= ADOPT_SHARE * (c.hits + c.misses) as f64
            })
            .max_by_key(|(_, c)| (c.fixes.len(), c.hits))
            .map(|(k, _)| *k);
        if let Some(key) = best {
            log::info!("own movement found in client frames: {:?}", key.to_source());
            self.source = Some(key);
            self.candidates.clear();
        }
    }

    pub fn status(&self) -> MovementStatus {
        MovementStatus {
            state: match (self.source, self.enabled) {
                (Some(_), _) => "found",
                (None, true) => "learning",
                (None, false) => "off",
            }
            .into(),
            source: self.source.map(Key::to_source),
            updates: self.updates,
            client_frames: self.client_frames,
            fixes_seen: self.fixes_total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up client movement packet: opcode, a varint sequence number
    /// of varying length, flags, then x y z.
    fn move_frame(seq: u64, p: [f32; 3]) -> Vec<u8> {
        let mut b = vec![0x55, 0x7a];
        let mut v = seq;
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                b.push(byte);
                break;
            }
            b.push(byte | 0x80);
        }
        b.push(0x03);
        for c in p {
            b.extend(c.to_le_bytes());
        }
        b.extend([0x10, 0x00]);
        b
    }

    fn walk(t: Millis) -> [f32; 3] {
        let s = t as f32 / 1000.0;
        [-45_000.0 + s * 500.0, 120_000.0 - s * 300.0, 15_000.0]
    }

    #[test]
    fn learns_the_movement_field_from_server_fixes_and_then_follows_it() {
        let mut m = MovementLearner::new(true, None, None);
        let mut live = Vec::new();
        for i in 0..400u64 {
            let t = i as Millis * 100;
            // A server fix every 5 s (a self-buff).
            if t % 5_000 == 0 {
                m.on_server_fix(walk(t), t);
            }
            // Noise: another packet with a far-away float triplet.
            let mut noise = vec![0x1d, 0x37, 0x01];
            for c in [12_000.0f32, -80_000.0, 400.0] {
                noise.extend(c.to_le_bytes());
            }
            m.on_client_frame([0x1d, 0x37], &noise, t);
            // The sequence number grows past 127, shifting x by one byte:
            // only the end-anchored field stays put.
            if let Some(p) = m.on_client_frame([0x55, 0x7a], &move_frame(i * 3, walk(t)), t) {
                live.push((t, p));
            }
        }
        let st = m.status();
        assert_eq!(st.state, "found");
        let src = st.source.unwrap();
        assert_eq!(src.opcode, "557a");
        assert_eq!((src.anchor, src.offset), (Anchor::End, 14));
        // Once found: an update about every 100 ms, matching the walk.
        assert!(live.len() > 100, "{}", live.len());
        for (t, p) in &live {
            assert!(planar(*p, walk(*t)) < 1.0);
        }
    }

    #[test]
    fn nothing_is_adopted_from_unrelated_numbers() {
        let mut m = MovementLearner::new(true, None, None);
        for i in 0..400u64 {
            let t = i as Millis * 100;
            if t % 5_000 == 0 {
                m.on_server_fix(walk(t), t);
            }
            // Positions of *other* entities, never near us.
            let mut other = vec![0x1b, 0x37, 0x05];
            let q = [walk(t)[0] + 40_000.0, walk(t)[1] - 25_000.0, 15_000.0];
            for c in q {
                other.extend(c.to_le_bytes());
            }
            m.on_client_frame([0x1b, 0x37], &other, t);
        }
        assert_eq!(m.status().state, "learning");
    }

    #[test]
    fn a_source_that_stops_agreeing_is_dropped_and_a_pinned_one_is_used_at_once() {
        let pinned = MoveSource {
            opcode: "557a".into(),
            anchor: Anchor::End,
            offset: 14,
        };
        let mut m = MovementLearner::new(true, Some(&pinned), None);
        assert!(m
            .on_client_frame([0x55, 0x7a], &move_frame(1, walk(0)), 0)
            .is_some());

        let mut m = MovementLearner::new(true, None, None);
        m.source = Key::from_source(&pinned);
        for i in 0..6 {
            let t = 1_000 * i;
            m.on_client_frame([0x55, 0x7a], &move_frame(1, walk(t)), t);
            // The server says we are 100 m away from what the client field says.
            let far = [walk(t)[0] + 10_000.0, walk(t)[1], walk(t)[2]];
            m.on_server_fix(far, t);
        }
        assert_eq!(m.status().state, "learning");
    }
}
