//! The discovery workflow end to end on the synthetic session (invented
//! opcodes): find the cube packet from markers, decode it with a profile,
//! and check the radar shows exactly the cubes that exist.

use radar_capture::discovery::{correlate, correlate_by, Grouping};
use radar_capture::profile::DecoderProfile;
use radar_capture::source::process_records;
use radar_capture::synthetic;
use radar_core::{CoordinateTransform, StaleDisplay, Tracker, TrackerConfig};
use radar_ingest::EventAdapter;

const T0: f64 = 1_760_000_000_000.0;

#[test]
fn markers_point_at_the_cube_packet() {
    let s = synthetic::session(T0, 120);
    let off = process_records(&s.records, None, true).unwrap();
    assert_eq!(off.streams.game_streams, 1);
    assert_eq!(off.streams.framing.bad_bundles, 0);

    let visible: Vec<f64> = s
        .markers
        .iter()
        .filter(|m| m.kind == "cube_visible")
        .map(|m| m.t)
        .collect();
    assert!(visible.len() >= 7);
    // By opcode alone the spawn drowns among the same opcode's frequent
    // "seen" messages; by subtype it stands out.
    let by_opcode = correlate(&off.samples, &visible, 3_000.0, 1_000.0);
    assert!(by_opcode.iter().all(|c| c.lift < 3.0), "{by_opcode:#?}");
    let ranked = correlate_by(&off.samples, &visible, 3_000.0, 1_000.0, Grouping::Subtype);
    assert_eq!(ranked[0].opcode, "C1 7E / 01", "{ranked:#?}");
    assert_eq!(ranked[0].markers_hit, visible.len());
}

#[test]
fn a_validated_profile_puts_exactly_the_live_cubes_on_the_radar() {
    let s = synthetic::session(T0, 120);
    let profile: DecoderProfile = serde_json::from_str(synthetic::PROFILE_JSON).unwrap();
    let off = process_records(&s.records, Some(profile), false).unwrap();
    assert_eq!(off.decode_errors, 0);

    let adapter = EventAdapter::new(CoordinateTransform::default()).unwrap();
    let mut tracker = Tracker::new(TrackerConfig::default());
    let mut checked = 0;
    let events = off.events;
    for (i, ev) in events.iter().enumerate() {
        let now = ev.time().unwrap();
        tracker.apply(adapter.convert(ev.clone()).unwrap(), now);
        // Check once every event stamped `now` has been applied.
        if events.get(i + 1).and_then(|e| e.time()) == Some(now) {
            continue;
        }
        tracker.tick(now);
        let shown: Vec<String> = tracker
            .frame(now, StaleDisplay::Hide)
            .cubes
            .into_iter()
            .map(|c| c.id)
            .collect();
        let truth: Vec<String> = s
            .cubes
            .iter()
            .filter(|(_, spawn, collect, _)| (*spawn as i64) <= now && now < *collect as i64)
            .map(|(id, ..)| id.to_string())
            .collect();
        assert_eq!(shown, truth, "at t={now}");
        checked += 1;
    }
    assert!(checked > 1000);
}
