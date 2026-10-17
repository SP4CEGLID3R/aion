//! Which game connection the decoder follows.
//!
//! The client keeps several connections in the game's framing open at once
//! (seen on a player's PC: two to game servers and one to a local service on
//! 127.0.0.1), and opens a new one per map. Entity ids are per connection, so
//! mixing them confuses who the player is. Only the current map's connection
//! sends map data (positions, objects, map loads); that one is followed and
//! the others are ignored:
//!
//! * The first connection to send map data is followed.
//! * A newer connection sending map data while the followed one has been
//!   quiet for a moment is the next map (a teleport, a dungeon): follow it.
//! * If the followed one has sent no map data for a long time, any
//!   connection that does takes over.
//!
//! "Who we are" messages count only from the followed connection. Those of
//! other connections are held (a few per connection) and replayed when that
//! connection is followed: the login message, or the new map's, may come
//! before its first map data. A connection merely being recognised changes
//! nothing.

use std::collections::{HashMap, VecDeque};

use radar_core::Millis;

use crate::framing::Frame;

/// The followed connection must have been this quiet before a newer one
/// takes over (a teleport's loading screen is longer; a copy of the same
/// data on another connection is not).
const NEWER_AFTER_QUIET_MS: f64 = 1_000.0;
/// After this long without map data on the followed connection, any
/// connection with map data takes over.
const ANY_AFTER_QUIET_MS: f64 = 30_000.0;
/// "Who we are" messages held per connection that is not followed.
const HELD_PER_CONNECTION: usize = 16;

/// What a frame is, for following (see `Aion2Decoder::frame_kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Positions, objects, map loads: only the current map's connection.
    Map,
    /// Who we are (by entity id): meaningful only on the followed connection.
    Identity,
    /// Heartbeats and the rest.
    Other,
}

/// Frames to hand to the decoder, in order.
#[derive(Debug, Default)]
pub struct Routed {
    /// A new connection is followed from now on: reset the decoder's
    /// per-connection state before decoding `frames`.
    pub switched: bool,
    pub frames: Vec<(Frame, Millis)>,
}

#[derive(Debug, Default)]
pub struct MapConnection {
    current: Option<u64>,
    last_map_t: f64,
    held: HashMap<u64, VecDeque<(Frame, Millis)>>,
}

impl MapConnection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<u64> {
        self.current
    }

    /// A frame of connection `conn` at `t_ms` (event time `t`).
    pub fn route(&mut self, conn: u64, kind: Kind, frame: Frame, t_ms: f64, t: Millis) -> Routed {
        let mine = self.current == Some(conn);
        let decode = |frame| Routed {
            switched: false,
            frames: vec![(frame, t)],
        };
        match kind {
            Kind::Other if self.current.is_none() || mine => decode(frame),
            Kind::Identity if mine => decode(frame),
            Kind::Identity => {
                let held = self.held.entry(conn).or_default();
                held.push_back((frame, t));
                if held.len() > HELD_PER_CONNECTION {
                    held.pop_front();
                }
                Routed::default()
            }
            Kind::Map if mine => {
                self.last_map_t = t_ms;
                decode(frame)
            }
            Kind::Map => {
                let quiet = t_ms - self.last_map_t;
                let take = match self.current {
                    None => true,
                    Some(c) => {
                        (conn > c && quiet >= NEWER_AFTER_QUIET_MS) || quiet >= ANY_AFTER_QUIET_MS
                    }
                };
                if !take {
                    return Routed::default();
                }
                log::info!(
                    "following game connection {conn} (map data){}",
                    self.current
                        .map(|c| format!(", instead of {c}"))
                        .unwrap_or_default()
                );
                self.current = Some(conn);
                self.last_map_t = t_ms;
                let mut frames: Vec<(Frame, Millis)> =
                    self.held.remove(&conn).map(Vec::from).unwrap_or_default();
                self.held.clear();
                frames.push((frame, t));
                Routed {
                    switched: true,
                    frames,
                }
            }
            Kind::Other => Routed::default(),
        }
    }

    /// Gaps and client frames count only for the followed connection.
    pub fn follows(&self, conn: u64) -> bool {
        self.current.is_none_or(|c| c == conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(op: u8) -> Frame {
        Frame {
            opcode: [op, 0x36],
            payload: vec![op, 0x36],
            bundled: false,
        }
    }

    fn ops(r: &Routed) -> Vec<u8> {
        r.frames.iter().map(|(f, _)| f.opcode[0]).collect()
    }

    #[test]
    fn the_first_connection_with_map_data_is_followed() {
        let mut m = MapConnection::new();
        // Heartbeats from three connections: nothing chosen, all decoded.
        for c in [1, 2, 3] {
            let r = m.route(c, Kind::Other, f(0), 0.0, 0);
            assert!(!r.switched && r.frames.len() == 1);
        }
        assert_eq!(m.current(), None);
        // Connection 3 sends map data: followed.
        let r = m.route(3, Kind::Map, f(0x34), 10.0, 10);
        assert!(r.switched);
        assert_eq!(ops(&r), vec![0x34]);
        // The others are ignored from now on, map data or not.
        assert!(m.route(1, Kind::Other, f(0), 20.0, 20).frames.is_empty());
        let r = m.route(2, Kind::Map, f(0x34), 30.0, 30);
        assert!(!r.switched && r.frames.is_empty());
        assert!(m.follows(3) && !m.follows(1));
    }

    #[test]
    fn who_we_are_counts_only_from_the_followed_connection() {
        let mut m = MapConnection::new();
        // Before any map data: held per connection, not decoded.
        assert!(m
            .route(1, Kind::Identity, f(0x4a), 0.0, 0)
            .frames
            .is_empty());
        assert!(m
            .route(2, Kind::Identity, f(0x33), 5.0, 5)
            .frames
            .is_empty());
        assert!(m
            .route(2, Kind::Identity, f(0x44), 6.0, 6)
            .frames
            .is_empty());
        // Connection 2 sends map data: its held messages come first, in order.
        let r = m.route(2, Kind::Map, f(0x21), 10.0, 10);
        assert!(r.switched);
        assert_eq!(ops(&r), vec![0x33, 0x44, 0x21]);
        // Later ones: from 2 decoded, from 1 held (not decoded).
        assert_eq!(
            ops(&m.route(2, Kind::Identity, f(0x4a), 20.0, 20)),
            vec![0x4a]
        );
        assert!(m
            .route(1, Kind::Identity, f(0x4a), 21.0, 21)
            .frames
            .is_empty());
        // A teleport: the new map's connection says who we are before its
        // first map data; that message comes with the switch.
        m.route(2, Kind::Map, f(0x34), 30_000.0, 30_000);
        let r = m.route(7, Kind::Identity, f(0x33), 33_000.0, 33_000);
        assert!(r.frames.is_empty());
        let r = m.route(7, Kind::Map, f(0x21), 33_100.0, 33_100);
        assert!(r.switched);
        assert_eq!(ops(&r), vec![0x33, 0x21]);
    }

    #[test]
    fn a_copy_of_the_same_data_on_another_connection_never_takes_over() {
        let mut m = MapConnection::new();
        m.route(2, Kind::Map, f(0x34), 0.0, 0);
        // Connection 5 (newer) sends the same map data at the same moments.
        for i in 1..600 {
            let t = i as f64 * 100.0;
            m.route(2, Kind::Map, f(0x34), t, t as Millis);
            assert!(
                !m.route(5, Kind::Map, f(0x34), t + 5.0, t as Millis)
                    .switched
            );
        }
        // An older copy never takes over either, even after a quiet spell.
        let mut m = MapConnection::new();
        m.route(5, Kind::Map, f(0x34), 0.0, 0);
        assert!(!m.route(2, Kind::Map, f(0x34), 5_000.0, 5_000).switched);
    }

    #[test]
    fn a_teleport_to_a_new_connection_is_followed_at_once() {
        let mut m = MapConnection::new();
        m.route(1, Kind::Map, f(0x34), 0.0, 0);
        m.route(1, Kind::Map, f(0x34), 60_000.0, 60_000);
        // Loading screen: 3 s of nothing, then the new map's connection.
        assert!(m.route(4, Kind::Map, f(0x21), 63_000.0, 63_000).switched);
        // The old connection's last words are ignored.
        assert!(m
            .route(1, Kind::Map, f(0x35), 63_100.0, 63_100)
            .frames
            .is_empty());
    }

    #[test]
    fn a_long_silent_connection_is_given_up_for_any_other() {
        let mut m = MapConnection::new();
        m.route(4, Kind::Map, f(0x34), 0.0, 0);
        assert!(!m.route(2, Kind::Map, f(0x34), 29_000.0, 29_000).switched);
        assert!(m.route(2, Kind::Map, f(0x34), 31_000.0, 31_000).switched);
    }
}
