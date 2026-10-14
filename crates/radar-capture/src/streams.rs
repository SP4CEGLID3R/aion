//! Finds the game's connection among everything captured, and frames it.
//!
//! Every TCP direction is a flow. A new flow is *probed*: its bytes are kept
//! from the first one, and once it carries two heartbeats within three
//! seconds it is the game's. Flows that start like TLS, or show no heartbeat
//! within 15 s, are ignored. Detection by content means no server address or
//! port has to be known, and traffic through a local proxy is found too.
//!
//! The game opens a new connection per map/dungeon, and entity ids are per
//! connection (Aether's finding), so a newly recognised game connection that
//! replaces another is reported as a zone change.

use std::collections::HashMap;
use std::net::SocketAddr;

use crate::framing::{count_heartbeats, Frame, Framer, FramingStats};
use crate::net::{tcp_flags, TcpSegment};
use crate::reassembly::{Chunk, Reassembler};

const PROBE_BYTES: usize = 256 * 1024;
const PROBE_TIMEOUT_MS: f64 = 15_000.0;
const HEARTBEAT_WINDOW_MS: f64 = 3_000.0;
const IDLE_MS: f64 = 60_000.0;
const MAX_FLOWS: usize = 4_096;

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// A flow was recognised as a game stream.
    GameStream {
        conn: u64,
        flow: String,
        /// The previous game connection this one replaces (a zone change).
        replaced: Option<u64>,
    },
    Frame {
        conn: u64,
        t_ms: f64,
        frame: Frame,
    },
    /// Bytes of a game stream were lost: whatever they said is unknown.
    Gap {
        conn: u64,
        t_ms: f64,
    },
    /// A frame the game client sent to the server (the other direction of a
    /// game connection). Framed by chain sync; it may carry the player's own
    /// movement.
    ClientFrame {
        conn: u64,
        t_ms: f64,
        frame: Frame,
    },
}

/// What is known about a flow, for deciding what to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    Probing,
    /// This direction, or the other direction of the same connection, is
    /// the game's.
    Game,
    Ignored,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FlowKey {
    src: SocketAddr,
    dst: SocketAddr,
}

impl FlowKey {
    /// Both directions of a connection share this.
    fn connection(&self) -> (SocketAddr, SocketAddr) {
        if self.src <= self.dst {
            (self.src, self.dst)
        } else {
            (self.dst, self.src)
        }
    }
}

enum State {
    Probing {
        buf: Vec<u8>,
        first_t: f64,
        heartbeats: usize,
        first_hb_t: Option<f64>,
    },
    Game {
        conn: u64,
        framer: Framer,
    },
    /// The client→server direction of a recognised game connection.
    Client {
        conn: u64,
        framer: Framer,
        started: bool,
    },
    Ignored,
}

struct Flow {
    reasm: Reassembler,
    state: State,
    last_t: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamStats {
    pub segments: u64,
    pub flows: usize,
    pub game_streams: u64,
    pub gaps: u64,
    /// Frames read from the client's own direction.
    pub client_frames: u64,
    pub framing: FramingStats,
    pub current_stream: Option<String>,
}

#[derive(Default)]
pub struct GameStreams {
    flows: HashMap<FlowKey, Flow>,
    next_conn: u64,
    current: Option<(u64, (SocketAddr, SocketAddr))>,
    stats: StreamStats,
    /// Framing stats of streams that have ended.
    retired: FramingStats,
}

impl GameStreams {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> StreamStats {
        let mut s = self.stats.clone();
        s.flows = self.flows.len();
        let mut f = self.retired;
        for flow in self.flows.values() {
            if let State::Game { framer, .. } = &flow.state {
                add(&mut f, framer.stats());
            }
        }
        s.framing = f;
        s
    }

    pub fn kind(&self, src: SocketAddr, dst: SocketAddr) -> FlowKind {
        let of = |key: FlowKey| {
            self.flows.get(&key).map(|f| match f.state {
                State::Probing { .. } => FlowKind::Probing,
                State::Game { .. } | State::Client { .. } => FlowKind::Game,
                State::Ignored => FlowKind::Ignored,
            })
        };
        let fwd = of(FlowKey { src, dst });
        if fwd == Some(FlowKind::Game) || of(FlowKey { src: dst, dst: src }) == Some(FlowKind::Game)
        {
            return FlowKind::Game;
        }
        fwd.unwrap_or(FlowKind::Unknown)
    }

    pub fn push(&mut self, seg: &TcpSegment<'_>, t_ms: f64) -> Vec<StreamEvent> {
        self.stats.segments += 1;
        let mut out = Vec::new();
        let key = FlowKey {
            src: seg.src,
            dst: seg.dst,
        };
        if !self.flows.contains_key(&key) {
            if self.flows.len() >= MAX_FLOWS {
                self.prune(t_ms, true);
            }
            let reverse = FlowKey {
                src: key.dst,
                dst: key.src,
            };
            let state = match self.flows.get(&reverse).map(|f| &f.state) {
                Some(State::Game { conn, .. }) => State::Client {
                    conn: *conn,
                    framer: Framer::chain_synced(),
                    started: false,
                },
                _ => State::Probing {
                    buf: Vec::new(),
                    first_t: t_ms,
                    heartbeats: 0,
                    first_hb_t: None,
                },
            };
            self.flows.insert(
                key,
                Flow {
                    reasm: Reassembler::new(),
                    state,
                    last_t: t_ms,
                },
            );
        }
        let flow = self.flows.get_mut(&key).expect("inserted above");
        flow.last_t = t_ms;
        if matches!(flow.state, State::Ignored) {
            return out;
        }

        let chunks = flow
            .reasm
            .push(seg.seq, seg.has(tcp_flags::SYN), seg.payload, t_ms);
        let mut recognised = false;
        for chunk in chunks {
            match (&mut flow.state, chunk) {
                (State::Game { conn, framer }, Chunk::Data(d)) => {
                    let conn = *conn;
                    out.extend(
                        framer
                            .feed(&d, t_ms)
                            .into_iter()
                            .map(|frame| StreamEvent::Frame { conn, t_ms, frame }),
                    );
                }
                (State::Game { conn, framer }, Chunk::Gap(_)) => {
                    self.stats.gaps += 1;
                    framer.desync();
                    out.push(StreamEvent::Gap { conn: *conn, t_ms });
                }
                (
                    State::Client {
                        conn,
                        framer,
                        started,
                    },
                    Chunk::Data(d),
                ) => {
                    let conn = *conn;
                    let frames = if *started {
                        framer.feed(&d, t_ms)
                    } else {
                        *started = true;
                        framer.start_chain(&d, flow.reasm.from_start, t_ms)
                    };
                    self.stats.client_frames += frames.len() as u64;
                    out.extend(frames.into_iter().map(|frame| StreamEvent::ClientFrame {
                        conn,
                        t_ms,
                        frame,
                    }));
                }
                (
                    State::Client {
                        framer, started, ..
                    },
                    Chunk::Gap(_),
                ) => {
                    *started = true;
                    framer.desync();
                }
                (
                    State::Probing {
                        buf,
                        heartbeats,
                        first_hb_t,
                        ..
                    },
                    chunk,
                ) => {
                    let d = match chunk {
                        Chunk::Data(d) => d,
                        Chunk::Gap(_) => {
                            // Not from the start any more; keep probing what follows.
                            flow.reasm.from_start = false;
                            buf.clear();
                            continue;
                        }
                    };
                    if buf.is_empty() && d.starts_with(&[0x16, 0x03]) {
                        flow.state = State::Ignored; // TLS handshake
                        break;
                    }
                    // Count heartbeats including any spanning the old/new boundary.
                    let before = count_heartbeats(buf);
                    buf.extend_from_slice(&d);
                    if buf.len() > PROBE_BYTES {
                        let cut = buf.len() - PROBE_BYTES;
                        buf.drain(..cut);
                        flow.reasm.from_start = false;
                        *heartbeats = count_heartbeats(buf);
                    } else {
                        *heartbeats += count_heartbeats(buf) - before;
                    }
                    if *heartbeats > 0 {
                        let first = *first_hb_t.get_or_insert(t_ms);
                        if *heartbeats >= 2 && t_ms - first <= HEARTBEAT_WINDOW_MS {
                            recognised = true;
                        } else if t_ms - first > HEARTBEAT_WINDOW_MS {
                            // One heartbeat long ago does not count; start over.
                            *heartbeats = 1;
                            *first_hb_t = Some(t_ms);
                        }
                    }
                }
                (State::Ignored, _) => break,
            }
        }

        if recognised {
            let State::Probing { buf, .. } = std::mem::replace(&mut flow.state, State::Ignored)
            else {
                unreachable!()
            };
            self.next_conn += 1;
            let conn = self.next_conn;
            let mut framer = Framer::new();
            let frames = framer.start(&buf, flow.reasm.from_start, t_ms);
            flow.state = State::Game { conn, framer };
            self.stats.game_streams += 1;

            let pair = key.connection();
            let replaced = match self.current {
                Some((old, old_pair)) if old_pair != pair => Some(old),
                _ => None,
            };
            if self.current.is_none_or(|(_, p)| p != pair) {
                self.current = Some((conn, pair));
            }
            let name = format!("{} → {}", key.src, key.dst);
            self.stats.current_stream = Some(name.clone());
            out.insert(
                0,
                StreamEvent::GameStream {
                    conn,
                    flow: name,
                    replaced,
                },
            );
            out.extend(
                frames
                    .into_iter()
                    .map(|frame| StreamEvent::Frame { conn, t_ms, frame }),
            );
            out.extend(self.adopt_client_direction(key, conn, t_ms));
        } else if let State::Probing {
            first_t,
            heartbeats: 0,
            ..
        } = flow.state
        {
            if t_ms - first_t > PROBE_TIMEOUT_MS {
                flow.state = State::Ignored;
            }
        }

        if seg.has(tcp_flags::FIN) || seg.has(tcp_flags::RST) {
            self.retire(&key);
        }
        if self.stats.segments.is_multiple_of(4096) {
            self.prune(t_ms, false);
        }
        out
    }

    /// The game's server direction was just recognised: frame the client's
    /// direction of the same connection too.
    fn adopt_client_direction(
        &mut self,
        server: FlowKey,
        conn: u64,
        t_ms: f64,
    ) -> Vec<StreamEvent> {
        let reverse = FlowKey {
            src: server.dst,
            dst: server.src,
        };
        let Some(flow) = self.flows.get_mut(&reverse) else {
            return Vec::new(); // created as a client flow on its first packet
        };
        let mut framer = Framer::chain_synced();
        let frames = match std::mem::replace(&mut flow.state, State::Ignored) {
            State::Probing { buf, .. } => framer.start_chain(&buf, flow.reasm.from_start, t_ms),
            _ => framer.start_chain(&[], false, t_ms),
        };
        flow.state = State::Client {
            conn,
            framer,
            started: true,
        };
        self.stats.client_frames += frames.len() as u64;
        frames
            .into_iter()
            .map(|frame| StreamEvent::ClientFrame { conn, t_ms, frame })
            .collect()
    }

    fn retire(&mut self, key: &FlowKey) {
        if let Some(Flow {
            state: State::Game { framer, .. },
            ..
        }) = self.flows.remove(key)
        {
            add(&mut self.retired, framer.stats());
        }
    }

    fn prune(&mut self, now: f64, force: bool) {
        let stale: Vec<FlowKey> = self
            .flows
            .iter()
            .filter(|(_, f)| {
                now - f.last_t > IDLE_MS || (force && matches!(f.state, State::Ignored))
            })
            .map(|(k, _)| *k)
            .collect();
        for k in stale {
            self.retire(&k);
            self.flows.remove(&k);
        }
    }
}

fn add(into: &mut FramingStats, s: FramingStats) {
    into.frames += s.frames;
    into.bundles += s.bundles;
    into.bad_bundles += s.bad_bundles;
    into.resyncs += s.resyncs;
    into.dropped_bytes += s.dropped_bytes;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::encode_frame;

    fn hb() -> Vec<u8> {
        encode_frame([0x00, 0x36], &[0; 8])
    }

    fn seg<'a>(src: &str, dst: &str, seq: u32, flags: u8, payload: &'a [u8]) -> TcpSegment<'a> {
        TcpSegment {
            src: src.parse().unwrap(),
            dst: dst.parse().unwrap(),
            seq,
            flags,
            payload,
        }
    }

    fn frames(events: &[StreamEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Frame { frame, .. } => Some(frame.opcode_hex()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn recognises_a_game_stream_and_reads_from_its_first_byte() {
        let mut g = GameStreams::new();
        let s = "203.0.113.5:13328";
        let c = "192.168.1.2:50000";
        assert!(g.push(&seg(s, c, 0, tcp_flags::SYN, b""), 0.0).is_empty());
        // Zone data before the first heartbeat must not be lost.
        let zone = encode_frame([0x33, 0x36], &[1, 2, 3]);
        let mut seq = 1;
        let mut all = g.push(&seg(s, c, seq, 0, &zone), 10.0);
        seq += zone.len() as u32;
        let h = hb();
        all.extend(g.push(&seg(s, c, seq, 0, &h), 20.0));
        seq += h.len() as u32;
        assert!(frames(&all).is_empty(), "one heartbeat is not enough");
        all.extend(g.push(&seg(s, c, seq, 0, &h), 70.0));
        assert!(matches!(
            all[0],
            StreamEvent::GameStream {
                conn: 1,
                replaced: None,
                ..
            }
        ));
        assert_eq!(frames(&all), vec!["33 36", "00 36", "00 36"]);
    }

    #[test]
    fn ignores_tls_and_silent_flows() {
        let mut g = GameStreams::new();
        g.push(
            &seg(
                "1.1.1.1:443",
                "10.0.0.2:5000",
                0,
                0,
                &[0x16, 0x03, 0x01, 0, 5],
            ),
            0.0,
        );
        let h = hb();
        let out = g.push(
            &seg(
                "1.1.1.1:443",
                "10.0.0.2:5000",
                5,
                0,
                &[h.clone(), h].concat(),
            ),
            1.0,
        );
        assert!(out.is_empty());
        assert_eq!(g.stats().game_streams, 0);
    }

    #[test]
    fn a_new_game_connection_is_a_zone_change() {
        let mut g = GameStreams::new();
        let two = [hb(), hb()].concat();
        let first = g.push(
            &seg("203.0.113.5:13328", "192.168.1.2:50000", 100, 0, &two),
            0.0,
        );
        assert!(matches!(
            first[0],
            StreamEvent::GameStream {
                conn: 1,
                replaced: None,
                ..
            }
        ));
        let second = g.push(
            &seg("203.0.113.9:13328", "192.168.1.2:50001", 500, 0, &two),
            5_000.0,
        );
        assert!(matches!(
            second[0],
            StreamEvent::GameStream {
                conn: 2,
                replaced: Some(1),
                ..
            }
        ));
    }

    #[test]
    fn heartbeats_split_across_segments_still_count() {
        let mut g = GameStreams::new();
        let two = [hb(), hb()].concat();
        let (a, b) = two.split_at(12); // second heartbeat split after its first byte
        g.push(&seg("203.0.113.5:1", "10.0.0.2:2", 0, 0, a), 0.0);
        let out = g.push(
            &seg("203.0.113.5:1", "10.0.0.2:2", a.len() as u32, 0, b),
            50.0,
        );
        assert!(matches!(out.first(), Some(StreamEvent::GameStream { .. })));
    }
}
