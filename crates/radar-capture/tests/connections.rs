//! Several game-framed connections at once, as seen on a player's PC (two
//! game servers and a local service on 127.0.0.1, recognised in any order),
//! and a teleport to a new connection. Messages are built from the
//! documented layouts (`synthetic::aion2_layout`).

use std::net::SocketAddr;

use radar_capture::net::{build_ipv4_tcp, linktype, tcp_flags};
use radar_capture::pcapfile::Record;
use radar_capture::source::process_records;
use radar_capture::synthetic::aion2_layout::{heartbeat, map_load, self_buff, self_info, spawn};
use radar_capture::{Aion2Config, DecoderChoice};
use radar_core::{CoordinateTransform, CubeState, StaleDisplay, Tracker, TrackerConfig};
use radar_ingest::{EventAdapter, WireEvent};

const T0: f64 = 1_791_884_800_000.0;
const ME: [f32; 3] = [-45_000.0, 120_000.0, 15_000.0];
const THERE: [f32; 3] = [30_000.0, -80_000.0, 2_000.0];
const CUBE_A: [f32; 3] = [-50_205.0, 126_283.0, 15_449.0];
const CUBE_B: [f32; 3] = [31_000.0, -78_000.0, 2_100.0];

/// One server→client TCP direction.
struct Flow {
    server: SocketAddr,
    client: SocketAddr,
    seq: u32,
    records: Vec<Record>,
}

impl Flow {
    fn new(server: &str, client: &str, t: f64) -> Flow {
        let server: SocketAddr = server.parse().unwrap();
        let client: SocketAddr = client.parse().unwrap();
        let syn = build_ipv4_tcp(server, client, 999, tcp_flags::SYN, &[]);
        Flow {
            server,
            client,
            seq: 1_000,
            records: vec![Record {
                t_ms: t,
                linktype: linktype::RAW,
                data: syn,
            }],
        }
    }

    fn send(&mut self, t: f64, bytes: Vec<u8>) {
        let pkt = build_ipv4_tcp(self.server, self.client, self.seq, 0, &bytes);
        self.seq = self.seq.wrapping_add(bytes.len() as u32);
        self.records.push(Record {
            t_ms: t,
            linktype: linktype::RAW,
            data: pkt,
        });
    }
}

/// Heartbeats every 100 ms from `from` to `to` (ms since T0), plus whatever
/// `extra` adds at each step.
fn stream(flow: &mut Flow, from: f64, to: f64, mut extra: impl FnMut(f64) -> Vec<u8>) {
    let mut rel = from;
    while rel < to {
        let mut batch = heartbeat(T0 + rel);
        batch.extend(extra(rel));
        flow.send(T0 + rel, batch);
        rel += 100.0;
    }
}

fn merge(flows: Vec<Flow>) -> Vec<Record> {
    let mut all: Vec<Record> = flows.into_iter().flat_map(|f| f.records).collect();
    all.sort_by(|a, b| a.t_ms.total_cmp(&b.t_ms));
    all
}

struct Run {
    events: Vec<WireEvent>,
    /// (ms since T0, live cube ids) after each batch of events.
    live: Vec<(f64, Vec<String>)>,
}

fn run(records: &[Record]) -> Run {
    let off = process_records(
        records,
        DecoderChoice::Aion2(Box::new(Aion2Config::builtin())),
        false,
    )
    .unwrap();
    let adapter = EventAdapter::new(CoordinateTransform::default()).unwrap();
    let mut tracker = Tracker::new(TrackerConfig::default());
    let mut live = Vec::new();
    for (i, ev) in off.events.iter().enumerate() {
        let now = ev.time().unwrap();
        tracker.apply(adapter.convert(ev.clone()).unwrap(), now);
        if off.events.get(i + 1).and_then(|e| e.time()) == Some(now) {
            continue;
        }
        tracker.tick(now);
        let f = tracker.frame(now, StaleDisplay::Hide);
        let ids = f
            .cubes
            .iter()
            .filter(|c| c.state == CubeState::Spawned)
            .map(|c| c.id.clone())
            .collect();
        live.push((now as f64 - T0, ids));
    }
    Run {
        events: off.events,
        live,
    }
}

impl Run {
    fn live_at(&self, rel: f64) -> Vec<String> {
        self.live
            .iter()
            .rev()
            .find(|(t, _)| *t <= rel)
            .map(|(_, ids)| ids.clone())
            .unwrap_or_default()
    }

    /// Own positions (game units back from metres, north-up) between two
    /// times (ms since T0).
    fn players(&self, from: f64, to: f64) -> Vec<(f64, [f64; 3])> {
        self.events
            .iter()
            .filter_map(|e| match e {
                WireEvent::Player { t, pos, .. } => Some((*t as f64 - T0, *pos)),
                _ => None,
            })
            .filter(|(t, _)| (from..to).contains(t))
            .collect()
    }

    fn zone_changes(&self) -> Vec<f64> {
        self.events
            .iter()
            .filter_map(|e| match e {
                WireEvent::ZoneChanged { t, .. } => Some(*t as f64 - T0),
                _ => None,
            })
            .collect()
    }
}

/// The map connection: a self-buff every 5 s, cube A seen at 5 s.
fn map_stream(flow: &mut Flow, to: f64) {
    stream(flow, 0.0, to, |rel| {
        let mut b = Vec::new();
        if (rel as u64).is_multiple_of(5_000) {
            b.extend(self_buff(ME));
        }
        if rel == 5_000.0 {
            b.extend(spawn(65_509, 1_200_039, CUBE_A));
        }
        b
    });
}

#[test]
fn extra_connections_do_not_reset_the_radar() {
    // As in the player's log: the map server, a local service on 127.0.0.1
    // and another server, the last one recognised only after 20 s.
    let mut game = Flow::new("66.206.3.146:39713", "192.168.0.164:58149", T0);
    map_stream(&mut game, 60_000.0);
    let mut local = Flow::new("127.0.0.1:55643", "127.0.0.1:58148", T0);
    stream(&mut local, 0.0, 60_000.0, |_| Vec::new());
    let mut other = Flow::new("188.122.80.85:10896", "192.168.0.164:58150", T0 + 20_000.0);
    stream(&mut other, 20_000.0, 60_000.0, |_| Vec::new());
    let r = run(&merge(vec![game, local, other]));

    // One zone: the map connection, from its first map data.
    assert_eq!(r.zone_changes().len(), 1, "{:?}", r.zone_changes());
    // The cube stays live throughout, also after the late connection.
    for rel in [6_000.0, 19_000.0, 21_000.0, 40_000.0, 59_000.0] {
        assert_eq!(r.live_at(rel), vec!["65509"], "at {rel}");
    }
    // Our position keeps coming with every self-buff, also right after.
    for (from, to) in [
        (20_000.0, 20_900.0),
        (25_000.0, 25_900.0),
        (55_000.0, 55_900.0),
    ] {
        assert!(!r.players(from, to).is_empty(), "no position {from}..{to}");
    }
}

#[test]
fn a_copy_of_the_map_data_on_a_local_connection_changes_nothing() {
    // A local relay sending the same map data as the server.
    let mut game = Flow::new("66.206.3.146:39713", "192.168.0.164:58149", T0);
    map_stream(&mut game, 40_000.0);
    let mut copy = Flow::new("127.0.0.1:55643", "127.0.0.1:58148", T0 + 50.0);
    stream(&mut copy, 50.0, 40_050.0, |rel| {
        let rel = rel - 50.0;
        let mut b = Vec::new();
        if (rel as u64).is_multiple_of(5_000) {
            b.extend(self_buff(ME));
        }
        if rel == 5_000.0 {
            b.extend(spawn(65_509, 1_200_039, CUBE_A));
        }
        b
    });
    let r = run(&merge(vec![game, copy]));
    assert_eq!(r.zone_changes().len(), 1, "{:?}", r.zone_changes());
    assert_eq!(r.live_at(39_000.0), vec!["65509"]);
    assert!(!r.players(35_000.0, 36_000.0).is_empty());
}

#[test]
fn a_teleport_to_a_new_connection_is_followed() {
    // The map connection ends at 30 s; after a 3 s loading screen the new
    // map's connection loads the map (with our position), says who we are,
    // and shows cube B.
    // The other server keeps its connection throughout.
    let mut old = Flow::new("66.206.3.146:39713", "192.168.0.164:58149", T0);
    map_stream(&mut old, 30_000.0);
    let mut other = Flow::new("188.122.80.85:10896", "192.168.0.164:58150", T0);
    stream(&mut other, 0.0, 60_000.0, |_| Vec::new());
    let mut new = Flow::new("66.206.3.150:41000", "192.168.0.164:58160", T0 + 33_000.0);
    stream(&mut new, 33_000.0, 60_000.0, |rel| {
        let mut b = Vec::new();
        if rel == 33_100.0 {
            b.extend(map_load(2, THERE));
        }
        if rel == 33_500.0 {
            b.extend(self_info());
        }
        if (rel as u64).is_multiple_of(5_000) || rel == 33_500.0 {
            b.extend(self_buff(THERE));
        }
        if rel == 36_000.0 {
            b.extend(spawn(70_001, 1_200_039, CUBE_B));
        }
        b
    });
    let r = run(&merge(vec![old, other, new]));

    let zones = r.zone_changes();
    assert_eq!(zones.len(), 2, "{zones:?}");
    assert!((33_000.0..34_000.0).contains(&zones[1]), "{zones:?}");
    assert_eq!(r.live_at(29_000.0), vec!["65509"]);
    // After the teleport: the old cube is gone, the new one live.
    assert_eq!(r.live_at(40_000.0), vec!["70001"]);
    assert_eq!(r.live_at(59_000.0), vec!["70001"]);
    // Our position at the new place right away (metres, north-up).
    let after = r.players(33_000.0, 34_000.0);
    assert!(!after.is_empty(), "no position right after the teleport");
    let (_, p) = after[0];
    assert!(
        (p[0] - 300.0).abs() < 1.0 && (p[1] - 800.0).abs() < 1.0,
        "{p:?}"
    );
}

#[test]
fn back_to_a_connection_that_stayed_open() {
    // A dungeon: its connection (newer) loads the map at 20 s and the first
    // one goes quiet; at 40 s the first one, still open, loads the map
    // again (back outside). Both switches happen at once.
    let mut field = Flow::new("66.206.3.146:39713", "192.168.0.164:58149", T0);
    stream(&mut field, 0.0, 60_000.0, |rel| {
        let mut b = Vec::new();
        if rel < 19_000.0 && (rel as u64).is_multiple_of(5_000) {
            b.extend(self_buff(ME));
        }
        if rel == 5_000.0 {
            b.extend(spawn(65_509, 1_200_039, CUBE_A));
        }
        if rel == 40_000.0 {
            b.extend(map_load(1, ME));
        }
        b
    });
    let mut dungeon = Flow::new("66.206.3.150:41000", "192.168.0.164:58160", T0 + 19_500.0);
    stream(&mut dungeon, 19_500.0, 39_000.0, |rel| {
        let mut b = Vec::new();
        if rel == 20_000.0 {
            b.extend(map_load(7, THERE));
        }
        if rel == 22_000.0 {
            b.extend(spawn(70_001, 1_200_039, CUBE_B));
        }
        b
    });
    let r = run(&merge(vec![field, dungeon]));
    let zones = r.zone_changes();
    assert!(
        zones.iter().any(|z| (20_000.0..20_200.0).contains(z)),
        "{zones:?}"
    );
    assert!(
        zones.iter().any(|z| (40_000.0..40_200.0).contains(z)),
        "{zones:?}"
    );
    assert_eq!(r.live_at(30_000.0), vec!["70001"]);
    assert!(r.live_at(45_000.0).is_empty());
    // Our position: in the dungeon at 20 s, outside again at 40 s.
    let (_, p) = r.players(20_000.0, 20_500.0)[0];
    assert!((p[0] - 300.0).abs() < 1.0, "{p:?}");
    let (_, p) = r.players(40_000.0, 40_500.0)[0];
    assert!((p[0] + 450.0).abs() < 1.0, "{p:?}");
}

#[test]
fn a_stray_message_on_a_local_connection_does_not_steal_the_radar() {
    // The local service sends, now and then, a message with the type of an
    // object spawn that does not decode as one.
    let mut game = Flow::new("66.206.3.146:39713", "192.168.0.164:58149", T0);
    map_stream(&mut game, 40_000.0);
    let mut local = Flow::new("127.0.0.1:55643", "127.0.0.1:58148", T0);
    stream(&mut local, 0.0, 40_000.0, |rel| {
        if (rel as u64).is_multiple_of(2_000) {
            radar_capture::framing::encode_frame([0x34, 0x36], &[1, 2, 3])
        } else {
            Vec::new()
        }
    });
    let r = run(&merge(vec![local, game]));
    assert_eq!(r.zone_changes().len(), 1, "{:?}", r.zone_changes());
    assert_eq!(r.live_at(39_000.0), vec!["65509"]);
    assert!(!r.players(35_000.0, 36_000.0).is_empty());
}
