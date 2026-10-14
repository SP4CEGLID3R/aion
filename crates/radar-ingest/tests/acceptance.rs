//! End-to-end acceptance tests: protocol text or synthetic bytes → parser →
//! adapter → tracker → frame, with a deterministic clock.

use std::collections::BTreeSet;

use radar_core::{
    CoordinateTransform, CubeState, RadarSettings, StaleDisplay, Tracker, TrackerConfig,
};
use radar_ingest::simulator::{SimConfig, Simulator};
use radar_ingest::{parse_line, srf, EventAdapter, WireEvent};

fn adapter() -> EventAdapter {
    EventAdapter::new(CoordinateTransform::default()).unwrap()
}

fn tracker() -> Tracker {
    Tracker::new(TrackerConfig {
        stale_timeout_ms: 10_000,
        ..TrackerConfig::default()
    })
}

fn marker_ids(t: &Tracker, now: i64) -> BTreeSet<String> {
    t.frame(now, StaleDisplay::Hide)
        .cubes
        .into_iter()
        .inspect(|c| assert_eq!(c.state, CubeState::Spawned))
        .map(|c| c.id)
        .collect()
}

#[test]
fn acceptance_scenario_fixture() {
    let text = include_str!("../fixtures/acceptance.jsonl");
    let adapter = adapter();
    let mut tracker = tracker();
    let mut now = 0;
    let mut checkpoints = 0;

    for (n, line) in text.lines().enumerate() {
        let lineno = n + 1;
        if let Some(directive) = line.strip_prefix("#!") {
            let directive = directive.trim();
            if let Some(ids) = directive.strip_prefix("markers:") {
                let expected: BTreeSet<String> =
                    ids.split_whitespace().map(str::to_owned).collect();
                assert_eq!(
                    marker_ids(&tracker, now),
                    expected,
                    "checkpoint at line {lineno}"
                );
                checkpoints += 1;
            } else if let Some(ms) = directive.strip_prefix("advance:") {
                now += ms.trim().parse::<i64>().unwrap();
                tracker.tick(now);
            } else {
                panic!("unknown directive at line {lineno}: {directive}");
            }
            continue;
        }
        let Some(ev) = parse_line(line).unwrap_or_else(|e| panic!("line {lineno}: {e}")) else {
            continue;
        };
        // The clock follows the stream, as it would live; it never runs backwards.
        if let Some(t) = ev.time() {
            now = now.max(t);
        }
        tracker.apply(adapter.convert(ev).unwrap(), now);
    }
    assert_eq!(checkpoints, 11);

    let d = tracker.diagnostics();
    assert_eq!(d.not_live_by_provenance["static_spawn_point"], 1);
    assert_eq!(d.not_live_by_provenance["historical"], 2);
    assert_eq!(d.rejected["missing_id"], 1);
    assert_eq!(d.rejected["out_of_order"], 1);
}

#[test]
fn synthetic_byte_stream_fixture() {
    let bytes =
        srf::parse_hex_fixture(include_str!("../fixtures/synthetic-session.srf.hex")).unwrap();
    let adapter = adapter();
    let mut tracker = tracker();
    let mut decoder = srf::FrameDecoder::new();
    let mut seen_markers = BTreeSet::new();

    // Feed the bytes in awkward 5-byte chunks, as a socket might deliver them.
    for chunk in bytes.chunks(5) {
        decoder.push(chunk);
        while let Some(frame) = decoder.next_frame() {
            let ev = frame.expect("fixture frames are valid");
            let now = ev.time().unwrap();
            tracker.apply(adapter.convert(ev).unwrap(), now);
            let ids = marker_ids(&tracker, now);
            assert!(!ids.contains("900"), "a static spawn point became a marker");
            assert!(ids.len() <= 2);
            seen_markers.extend(ids);
        }
    }
    assert_eq!(decoder.stats().frames, 11);
    assert_eq!(decoder.stats().errors, 0);
    assert_eq!(
        seen_markers,
        BTreeSet::from(["1".to_owned(), "2".to_owned()])
    );
    // Collected and despawned by the end.
    assert!(marker_ids(&tracker, 1_760_000_003_000).is_empty());
}

/// Every marker the radar shows must be a cube that really exists in the
/// simulated world, at its real position — and every live cube is shown —
/// despite duplicate spawns, static spawn points and historical locations
/// mixed into the stream.
#[test]
fn simulator_markers_always_match_ground_truth() {
    let adapter = adapter();
    let mut tracker = tracker();
    let mut sim = Simulator::new(
        SimConfig {
            seed: 42,
            cube_count: 8,
            noise: true,
            ..SimConfig::default()
        },
        1_760_000_000_000,
    );
    let mut removals = 0;
    for _ in 0..(5 * 60 * 1000 / 50) {
        for ev in sim.step() {
            if matches!(
                ev,
                WireEvent::CubeCollected { .. } | WireEvent::CubeDespawned { .. }
            ) {
                removals += 1;
            }
            tracker.apply(adapter.convert(ev).unwrap(), sim.now());
        }
        tracker.tick(sim.now());

        let frame = tracker.frame(sim.now(), StaleDisplay::MarkStale);
        let truth: BTreeSet<(String, [i64; 3])> = sim
            .live_cubes()
            .map(|(id, p)| (id.to_owned(), p.map(|v| (v * 10.0).round() as i64)))
            .collect();
        let shown: BTreeSet<(String, [i64; 3])> = frame
            .cubes
            .iter()
            .map(|c| {
                assert_eq!(
                    c.state,
                    CubeState::Spawned,
                    "{} shown as {:?}",
                    c.id,
                    c.state
                );
                let p = [c.pos.x, c.pos.y, c.pos.z];
                (c.id.clone(), p.map(|v| (v * 10.0).round() as i64))
            })
            .collect();
        assert_eq!(shown, truth, "at t={}", sim.now());
    }
    assert!(
        removals > 10,
        "the run should exercise despawns and collections"
    );
    let d = tracker.diagnostics();
    assert!(d.duplicates > 0);
    assert!(d.not_live_by_provenance["static_spawn_point"] > 0);
    assert!(d.not_live_by_provenance["historical"] > 0);
}

/// When the source goes silent, nothing keeps looking live.
#[test]
fn source_outage_turns_every_marker_stale() {
    let adapter = adapter();
    let mut tracker = Tracker::new(RadarSettings::default().tracker_config());
    let mut sim = Simulator::new(SimConfig::default(), 1_760_000_000_000);
    for _ in 0..100 {
        for ev in sim.step() {
            tracker.apply(adapter.convert(ev).unwrap(), sim.now());
        }
    }
    let live = marker_ids(&tracker, sim.now()).len();
    assert_eq!(live, 6);

    // Ten seconds and a bit of silence.
    let later = sim.now() + 10_001;
    tracker.tick(later);
    assert!(marker_ids(&tracker, later).is_empty());
    let marked = tracker.frame(later, StaleDisplay::MarkStale);
    assert_eq!(marked.cubes.len(), 6);
    assert!(marked.cubes.iter().all(|c| c.state == CubeState::Stale));
    assert!(marked.player.unwrap().stale);
}
