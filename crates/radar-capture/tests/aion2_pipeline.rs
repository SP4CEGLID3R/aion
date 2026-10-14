//! The built-in AION 2 decoder end to end: TCP packets carrying frames in the
//! documented layouts → stream detection → framing → decoder → radar tracker.
//! Messages are built from the documented layouts
//! (`synthetic::aion2_layout`); no captured game data is included.

use std::net::SocketAddr;

use radar_capture::framing::encode_bundle;
use radar_capture::net::{build_ipv4_tcp, linktype, tcp_flags};
use radar_capture::pcapfile::Record;
use radar_capture::source::process_records;
use radar_capture::synthetic::aion2_layout::{heartbeat, removed, self_buff, spawn};
use radar_capture::{Aion2Config, DecoderChoice};
use radar_core::{CoordinateTransform, CubeState, StaleDisplay, Tracker, TrackerConfig};
use radar_ingest::{EventAdapter, WireEvent};

const T0: f64 = 1_791_884_800_000.0;

/// (ms since the start, live cube ids, stale cube ids) after each batch of events.
type Timeline = Vec<(f64, Vec<String>, Vec<String>)>;

struct Script {
    records: Vec<Record>,
    seq: u32,
    server: SocketAddr,
    client: SocketAddr,
}

impl Script {
    fn new() -> Self {
        let server: SocketAddr = "193.0.2.10:13328".parse().unwrap();
        let client: SocketAddr = "192.168.1.20:50123".parse().unwrap();
        let syn = build_ipv4_tcp(server, client, 999, tcp_flags::SYN, &[]);
        Script {
            records: vec![Record {
                t_ms: T0,
                linktype: linktype::RAW,
                data: syn,
            }],
            seq: 1_000,
            server,
            client,
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

const ME: [f32; 3] = [-45_000.0, 120_000.0, 15_000.0];
const CUBE_A: [f32; 3] = [-50_205.0, 126_283.0, 15_449.0];
const CUBE_B: [f32; 3] = [-40_000.0, 125_000.0, 15_100.0];

/// 60 s of play: heartbeats every 50 ms, a self-buff every 5 s, cube A seen
/// at 5 s and opened at 30 s, cube B seen at 10 s and leaving range at 40 s,
/// another object that is not a cube, and one spawn inside an LZ4 bundle.
fn session() -> Vec<Record> {
    let mut s = Script::new();
    let mut t = T0;
    while t < T0 + 60_000.0 {
        let rel = t - T0;
        let mut batch = heartbeat(t);
        if (rel as u64).is_multiple_of(5_000) {
            batch.extend(self_buff(ME));
        }
        if rel == 5_000.0 {
            batch.extend(spawn(65_509, 1_200_039, CUBE_A));
            batch.extend(spawn(45_665, 1_500_041, [12_737.3, -77_268.5, 1_302.0]));
        }
        if rel == 10_000.0 {
            batch.extend(encode_bundle(&spawn(34_438, 1_200_041, CUBE_B)));
        }
        if rel == 30_000.0 {
            batch.extend(removed(65_509, 0x04));
        }
        if rel == 40_000.0 {
            batch.extend(removed(34_438, 0x01));
        }
        s.send(t, batch);
        t += 50.0;
    }
    s.records
}

fn run(records: &[Record]) -> (Vec<WireEvent>, Tracker, Timeline) {
    let off = process_records(
        records,
        DecoderChoice::Aion2(Box::new(Aion2Config::builtin())),
        false,
    )
    .unwrap();
    let adapter = EventAdapter::new(CoordinateTransform::default()).unwrap();
    let mut tracker = Tracker::new(TrackerConfig::default());
    let mut timeline = Vec::new();
    let events = off.events.clone();
    for (i, ev) in events.iter().enumerate() {
        let now = ev.time().unwrap();
        tracker.apply(adapter.convert(ev.clone()).unwrap(), now);
        if events.get(i + 1).and_then(|e| e.time()) == Some(now) {
            continue;
        }
        tracker.tick(now);
        let f = tracker.frame(now, StaleDisplay::MarkStale);
        let live = f
            .cubes
            .iter()
            .filter(|c| c.state == CubeState::Spawned)
            .map(|c| c.id.clone())
            .collect();
        let stale = f
            .cubes
            .iter()
            .filter(|c| c.state == CubeState::Stale)
            .map(|c| c.id.clone())
            .collect();
        timeline.push((now as f64 - T0, live, stale));
    }
    (off.events, tracker, timeline)
}

fn at(timeline: &Timeline, rel: f64) -> (Vec<String>, Vec<String>) {
    let (_, live, stale) = timeline
        .iter()
        .rev()
        .find(|(t, ..)| *t <= rel)
        .expect("a frame before");
    (live.clone(), stale.clone())
}

#[test]
fn cubes_appear_stay_live_and_leave_exactly_as_the_server_reports() {
    let (events, tracker, timeline) = run(&session());

    assert_eq!(at(&timeline, 4_000.0).0, Vec::<String>::new());
    // Cube A from 5 s; the non-cube object never shows.
    assert_eq!(at(&timeline, 9_000.0).0, vec!["65509"]);
    // Cube B (in a compressed bundle) from 10 s; both stay live for 20 s with
    // no new spawn packets, because the stream keeps confirming them.
    assert_eq!(at(&timeline, 29_900.0).0, vec!["34438", "65509"]);
    // A opened at 30 s: gone, not stale.
    let (live, stale) = at(&timeline, 35_000.0);
    assert_eq!((live, stale), (vec!["34438".to_string()], vec![]));
    // B left the send range at 40 s: stale (no longer verified), never live.
    let (live, stale) = at(&timeline, 45_000.0);
    assert!(live.is_empty());
    assert_eq!(stale, vec!["34438"]);

    // Positions arrive in metres with north up (−Y).
    let spawned = events
        .iter()
        .find_map(|e| match e {
            WireEvent::CubeSpawned { pos, .. } => Some(*pos),
            _ => None,
        })
        .unwrap();
    assert_eq!(spawned, [-502.05, -1262.83, 154.49]);
    // Our own position came from the self-buffs.
    assert!(events.iter().any(|e| matches!(e, WireEvent::Player { .. })));
    assert!(
        tracker.diagnostics().rejected.is_empty(),
        "{:?}",
        tracker.diagnostics()
    );
}

#[test]
fn a_stalled_stream_stops_confirming_cubes() {
    // Same session, cut after 12 s: no more packets at all.
    let records: Vec<Record> = session()
        .into_iter()
        .filter(|r| r.t_ms < T0 + 12_000.0)
        .collect();
    let off = process_records(
        &records,
        DecoderChoice::Aion2(Box::new(Aion2Config::builtin())),
        false,
    )
    .unwrap();
    let adapter = EventAdapter::new(CoordinateTransform::default()).unwrap();
    let mut tracker = Tracker::new(TrackerConfig::default());
    for ev in off.events {
        let now = ev.time().unwrap();
        tracker.apply(adapter.convert(ev).unwrap(), now);
    }
    let later = (T0 + 12_000.0 + 10_500.0) as i64;
    tracker.tick(later);
    assert!(
        tracker.frame(later, StaleDisplay::Hide).cubes.is_empty(),
        "nothing may look live once the stream has gone quiet"
    );
}
