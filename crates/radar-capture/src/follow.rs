//! Which game connection the decoder follows.
//!
//! The client keeps several connections in the game's framing open at once
//! (seen on a player's PC: two to game servers and one to a local service on
//! 127.0.0.1), and opens a new one per map. Entity ids are per connection, so
//! mixing them confuses who the player is. Only the current map's connection
//! sends map data (positions, objects, map loads); that one is followed and
//! the others are ignored. What counts as map data is decided by decoding it
//! (`Aion2Decoder::frame_kind`): a message type alone is not enough.
//!
//! * The first connection to send map data is followed.
//! * A map load or teleport (with a world position) on another connection
//!   is the next map: followed at once, whichever connection it is (a new
//!   one, or one open all along, e.g. back from a dungeon).
//! * Otherwise another connection takes over only after sending
//!   [`TAKEOVER_FRAMES`] pieces of map data while the followed one sent none
//!   for [`TAKEOVER_QUIET_MS`]. A copy of the same data on another
//!   connection never does, as the followed one keeps sending too.
//!
//! Messages of connections that are not followed ("who we are", map data)
//! are held, a few per connection, and replayed when that connection is
//! followed: the login message or the first objects may come before the
//! switch. A connection merely being recognised changes nothing.

use std::collections::{HashMap, VecDeque};

use radar_core::Millis;

use crate::framing::Frame;

/// The followed connection must have sent no map data for this long…
pub const TAKEOVER_QUIET_MS: f64 = 2_000.0;
/// …while another one sent this many pieces of it, for that one to take over.
pub const TAKEOVER_FRAMES: u32 = 3;
/// Messages held per connection that is not followed.
const HELD_PER_CONNECTION: usize = 64;
/// Connections whose messages are held at most.
const HELD_CONNECTIONS: usize = 8;

/// What a frame is, for following.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A map load or teleport with a world position: this connection is the
    /// current map's from now on.
    Arrival,
    /// Positions and objects that decode: only the current map's connection
    /// sends them.
    Map,
    /// Who we are (by entity id): meaningful only on the followed connection.
    Identity,
    /// Heartbeats and the rest.
    Other,
}

/// Frames to hand to the decoder, in order.
#[derive(Debug, Default)]
pub struct Routed {
    /// From now on this connection is followed.
    pub switched: bool,
    /// The connection followed before, if any: the decoder's
    /// per-connection state (entity ids) is to be reset first.
    pub previous: Option<u64>,
    pub frames: Vec<(Frame, Millis)>,
}

#[derive(Debug, Default)]
pub struct MapConnection {
    current: Option<u64>,
    last_map_t: f64,
    /// Another connection sending map data while the followed one is quiet:
    /// (connection, pieces since the followed one's last).
    rival: Option<(u64, u32)>,
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
        if mine {
            if matches!(kind, Kind::Map | Kind::Arrival) {
                self.last_map_t = t_ms;
                self.rival = None;
            }
            return Routed {
                frames: vec![(frame, t)],
                ..Routed::default()
            };
        }
        match kind {
            Kind::Other if self.current.is_none() => Routed {
                frames: vec![(frame, t)],
                ..Routed::default()
            },
            Kind::Other => Routed::default(),
            Kind::Identity => {
                self.hold(conn, frame, t);
                Routed::default()
            }
            Kind::Arrival => self.take(conn, frame, t_ms, t),
            Kind::Map if self.current.is_none() => self.take(conn, frame, t_ms, t),
            Kind::Map => {
                let n = match self.rival {
                    Some((c, n)) if c == conn => n + 1,
                    _ => 1,
                };
                self.rival = Some((conn, n));
                if n >= TAKEOVER_FRAMES && t_ms - self.last_map_t >= TAKEOVER_QUIET_MS {
                    self.take(conn, frame, t_ms, t)
                } else {
                    self.hold(conn, frame, t);
                    Routed::default()
                }
            }
        }
    }

    /// Gaps and client frames count only for the followed connection.
    pub fn follows(&self, conn: u64) -> bool {
        self.current == Some(conn)
    }

    fn hold(&mut self, conn: u64, frame: Frame, t: Millis) {
        if !self.held.contains_key(&conn) && self.held.len() >= HELD_CONNECTIONS {
            self.held.clear();
        }
        let held = self.held.entry(conn).or_default();
        held.push_back((frame, t));
        if held.len() > HELD_PER_CONNECTION {
            held.pop_front();
        }
    }

    fn take(&mut self, conn: u64, frame: Frame, t_ms: f64, t: Millis) -> Routed {
        let previous = self.current;
        log::info!(
            "following game connection {conn} (map data){}",
            previous
                .map(|c| format!(", instead of {c}"))
                .unwrap_or_default()
        );
        self.current = Some(conn);
        self.last_map_t = t_ms;
        self.rival = None;
        let mut frames: Vec<(Frame, Millis)> =
            self.held.remove(&conn).map(Vec::from).unwrap_or_default();
        self.held.clear();
        frames.push((frame, t));
        Routed {
            switched: true,
            previous,
            frames,
        }
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

    fn at(m: &mut MapConnection, conn: u64, kind: Kind, op: u8, t: f64) -> Routed {
        m.route(conn, kind, f(op), t, t as Millis)
    }

    #[test]
    fn the_first_connection_with_map_data_is_followed() {
        let mut m = MapConnection::new();
        // Heartbeats from three connections: nothing chosen, all decoded.
        for c in [1, 2, 3] {
            let r = at(&mut m, c, Kind::Other, 0, 0.0);
            assert!(!r.switched && r.frames.len() == 1);
        }
        assert_eq!(m.current(), None);
        assert!(!m.follows(1));
        // Connection 3 sends map data: followed, nothing to reset before.
        let r = at(&mut m, 3, Kind::Map, 0x34, 10.0);
        assert!(r.switched && r.previous.is_none());
        assert_eq!(ops(&r), vec![0x34]);
        // The others are ignored from now on.
        assert!(at(&mut m, 1, Kind::Other, 0, 20.0).frames.is_empty());
        let r = at(&mut m, 2, Kind::Map, 0x34, 30.0);
        assert!(!r.switched && r.frames.is_empty());
        assert!(m.follows(3) && !m.follows(1));
    }

    #[test]
    fn who_we_are_counts_only_from_the_followed_connection() {
        let mut m = MapConnection::new();
        // Before any map data: held per connection, not decoded.
        assert!(at(&mut m, 1, Kind::Identity, 0x4a, 0.0).frames.is_empty());
        assert!(at(&mut m, 2, Kind::Identity, 0x33, 5.0).frames.is_empty());
        assert!(at(&mut m, 2, Kind::Identity, 0x44, 6.0).frames.is_empty());
        // Connection 2 sends map data: its held messages come first, in order.
        let r = at(&mut m, 2, Kind::Map, 0x21, 10.0);
        assert_eq!(ops(&r), vec![0x33, 0x44, 0x21]);
        // Later ones: from 2 decoded, from 1 held (not decoded).
        assert_eq!(ops(&at(&mut m, 2, Kind::Identity, 0x4a, 20.0)), vec![0x4a]);
        assert!(at(&mut m, 1, Kind::Identity, 0x4a, 21.0).frames.is_empty());
    }

    #[test]
    fn a_map_load_on_another_connection_is_followed_at_once() {
        let mut m = MapConnection::new();
        at(&mut m, 5, Kind::Map, 0x34, 0.0);
        // The new map's connection says who we are, then loads the map
        // while the old one is still talking: followed at once.
        at(&mut m, 7, Kind::Identity, 0x33, 1_000.0);
        at(&mut m, 5, Kind::Map, 0x34, 1_050.0);
        let r = at(&mut m, 7, Kind::Arrival, 0x21, 1_100.0);
        assert_eq!((r.switched, r.previous), (true, Some(5)));
        assert_eq!(ops(&r), vec![0x33, 0x21]);
        // Back to an older connection that stayed open (out of a dungeon):
        // its map load is followed at once too, whatever its id.
        let r = at(&mut m, 2, Kind::Arrival, 0x21, 60_000.0);
        assert_eq!((r.switched, r.previous), (true, Some(7)));
        assert!(at(&mut m, 7, Kind::Map, 0x34, 60_100.0).frames.is_empty());
    }

    #[test]
    fn a_copy_of_the_same_data_on_another_connection_never_takes_over() {
        for copy_first in [false, true] {
            let mut m = MapConnection::new();
            at(&mut m, 2, Kind::Map, 0x34, 0.0);
            // Connection 5 sends the same map data, just before or just
            // after the followed one, with quiet spells in between.
            let mut t = 0.0;
            for i in 1..300 {
                t += if i % 10 == 0 { 3_000.0 } else { 100.0 };
                let (a, b) = if copy_first { (5, 2) } else { (2, 5) };
                assert!(!at(&mut m, a, Kind::Map, 0x34, t).switched, "{i}");
                assert!(!at(&mut m, b, Kind::Map, 0x34, t + 5.0).switched, "{i}");
            }
            assert_eq!(m.current(), Some(2));
        }
    }

    #[test]
    fn one_stray_message_on_another_connection_changes_nothing() {
        let mut m = MapConnection::new();
        at(&mut m, 2, Kind::Map, 0x34, 0.0);
        // Long quiet on the followed connection, then single stray pieces
        // of map data from elsewhere, far apart.
        for (c, t) in [(9, 10_000.0), (8, 20_000.0), (9, 30_000.0)] {
            assert!(!at(&mut m, c, Kind::Map, 0x34, t).switched);
            at(&mut m, 2, Kind::Map, 0x34, t + 100.0);
        }
        assert_eq!(m.current(), Some(2));
    }

    #[test]
    fn a_new_map_without_a_map_load_takes_over_after_a_few_objects() {
        let mut m = MapConnection::new();
        at(&mut m, 1, Kind::Map, 0x34, 0.0);
        // The followed connection ends; after a 3 s loading screen another
        // connection sends objects: the third takes over, with all three.
        assert!(!at(&mut m, 4, Kind::Map, 0x34, 3_000.0).switched);
        assert!(!at(&mut m, 4, Kind::Map, 0x35, 3_010.0).switched);
        let r = at(&mut m, 4, Kind::Map, 0x36, 3_020.0);
        assert_eq!((r.switched, r.previous), (true, Some(1)));
        assert_eq!(ops(&r), vec![0x34, 0x35, 0x36]);
    }
}
