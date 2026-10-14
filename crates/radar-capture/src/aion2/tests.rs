//! Tests build messages from the documented layouts (see the module docs).
//! The bytes are constructed here, not taken from anyone's captures.

use super::*;

fn varint_bytes(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

fn f(op: [u8; 2], body: Vec<u8>) -> Frame {
    let mut payload = op.to_vec();
    payload.extend(body);
    Frame {
        opcode: op,
        payload,
        bundled: false,
    }
}

fn xyz(p: [f32; 3]) -> Vec<u8> {
    p.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub(crate) fn spawn_body(id: u64, template: u32, p: [f32; 3], created: i64) -> Vec<u8> {
    let mut b = varint_bytes(id);
    b.extend([0x04, 0x00]);
    b.extend(template.to_le_bytes());
    b.extend(xyz(p));
    b.extend(0f32.to_le_bytes());
    b.extend(90f32.to_le_bytes());
    b.extend([1, 1, 1]);
    b.extend(created.to_le_bytes());
    b.extend([0u8; 10]);
    b
}

fn removed_body(id: u64, reason: u8) -> Vec<u8> {
    let mut b = varint_bytes(id);
    b.extend([reason, 0x03, 0x00]);
    b
}

fn buff_a_body(caster: u64, p: [f32; 3]) -> Vec<u8> {
    let mut b = varint_bytes(caster);
    b.extend([0x01, 0x11, 0x90, 0x03, 0xe8, 0x03, 0, 0, 0, 0, 0, 0, 0, 0]);
    b.extend(1_791_883_623_000i64.to_le_bytes());
    b.extend(varint_bytes(caster));
    b.extend([0x01, 0x00]);
    b.extend(xyz(p));
    b
}

fn map_load_body(map: u32, p: [f32; 3]) -> Vec<u8> {
    let mut b = 11u32.to_le_bytes().to_vec();
    b.extend(map.to_le_bytes());
    b.extend([0x3a, 0x03, 0x87, 0x00, 0, 0, 0, 0]);
    b.extend(xyz(p));
    b.extend(180f32.to_le_bytes());
    b.extend([2, 0, 0, 0]);
    b
}

fn dec() -> Aion2Decoder {
    Aion2Decoder::new(Aion2Config::builtin()).unwrap()
}

const CUBE: [f32; 3] = [-50205.0, 126283.0, 15449.0];
const ME: [f32; 3] = [-45000.0, 120000.0, 15000.0];

#[test]
fn builtin_config_is_valid_and_skips_hypotheses_by_default() {
    let d = dec();
    assert!(d.is_cube_template(1_200_039));
    assert!(d.is_cube_template(1_200_041));
    assert!(
        !d.is_cube_template(1_301_199),
        "hypothesis templates are off by default"
    );
    let mut cfg = Aion2Config::builtin();
    cfg.use_hypotheses = true;
    assert!(Aion2Decoder::new(cfg).unwrap().is_cube_template(1_301_199));
}

#[test]
fn unconfirmed_types_are_shown_only_when_asked_and_say_so() {
    let mut cfg = Aion2Config::builtin();
    cfg.use_hypotheses = true;
    let mut d = Aion2Decoder::new(cfg).unwrap();
    let ev = d.on_frame(&f([0x34, 0x36], spawn_body(5, 1_301_199, CUBE, 0)), 0);
    let [WireEvent::CubeSpawned { label, .. }] = ev.as_slice() else {
        panic!("{ev:?}")
    };
    assert_eq!(
        label.as_deref(),
        Some("Hidden Cube? (Altgard, unconfirmed type)")
    );

    let mut off = dec();
    assert!(off
        .on_frame(&f([0x34, 0x36], spawn_body(5, 1_301_199, CUBE, 0)), 0)
        .is_empty());
}

#[test]
fn parses_the_documented_spawn_layout() {
    let s = parse_spawn(&spawn_body(65_509, 1_200_039, CUBE, 1_791_884_649_000)).unwrap();
    assert_eq!((s.id, s.kind, s.template), (65_509, 4, 1_200_039));
    assert_eq!(s.pos, CUBE);
    assert_eq!(s.heading, 90.0);
    assert_eq!(s.created_ms, Some(1_791_884_649_000));
    assert!(parse_spawn(&[0x05, 0x04]).is_none());
}

#[test]
fn a_cube_spawn_becomes_a_marker_event_in_metres_with_north_up() {
    let mut d = dec();
    let ev = d.on_frame(
        &f([0x34, 0x36], spawn_body(65_509, 1_200_039, CUBE, 0)),
        1_000,
    );
    let [WireEvent::CubeSpawned {
        id,
        pos,
        label,
        provenance,
        ..
    }] = ev.as_slice()
    else {
        panic!("{ev:?}")
    };
    assert_eq!(*id, Some(WireId::Num(65_509)));
    // cm → m; north = −Y.
    assert_eq!(*pos, [-502.05, -1262.83, 154.49]);
    assert_eq!(label.as_deref(), Some("Hidden Cube (Verteron)"));
    assert_eq!(*provenance, WireProvenance::Live);
}

#[test]
fn other_objects_are_not_cubes() {
    let mut d = dec();
    let ev = d.on_frame(&f([0x34, 0x36], spawn_body(1, 1_500_041, CUBE, 0)), 1_000);
    assert!(ev.is_empty());
}

#[test]
fn removal_reasons_map_to_collected_lost_and_despawned() {
    let mut d = dec();
    for id in [1, 2, 3] {
        d.on_frame(&f([0x34, 0x36], spawn_body(id, 1_200_039, CUBE, 0)), 0);
    }
    let r = |d: &mut Aion2Decoder, id, reason| {
        d.on_frame(&f([0x35, 0x36], removed_body(id, reason)), 10)
    };
    assert!(matches!(
        r(&mut d, 1, 0x04)[..],
        [WireEvent::CubeCollected { .. }]
    ));
    assert!(matches!(
        r(&mut d, 2, 0x01)[..],
        [WireEvent::CubeLost { .. }]
    ));
    assert!(matches!(
        r(&mut d, 3, 0x07)[..],
        [WireEvent::CubeDespawned { .. }]
    ));
    // Removing a non-cube object emits nothing.
    assert!(r(&mut d, 99, 0x04).is_empty());
}

#[test]
fn presence_is_confirmed_only_while_the_stream_is_alive() {
    let mut d = dec();
    d.on_frame(&f([0x34, 0x36], spawn_body(7, 1_200_039, CUBE, 0)), 0);
    assert!(d.tick(2_500).is_empty(), "no heartbeat seen yet");
    d.on_frame(&f([0x00, 0x36], vec![0; 8]), 2_000);
    assert_eq!(d.tick(2_500).len(), 1);
    assert!(d.tick(3_000).is_empty(), "not again before the interval");
    // Heartbeats stop: no more confirmations, so the radar lets it go stale.
    assert!(d.tick(6_000).is_empty());
}

#[test]
fn a_gap_loses_every_cube() {
    let mut d = dec();
    d.on_frame(&f([0x34, 0x36], spawn_body(7, 1_200_039, CUBE, 0)), 0);
    d.on_frame(&f([0x34, 0x36], spawn_body(8, 1_200_041, CUBE, 0)), 0);
    let lost = d.on_gap(5);
    assert_eq!(lost.len(), 2);
    assert!(lost.iter().all(|e| matches!(e, WireEvent::CubeLost { .. })));
    d.on_frame(&f([0x00, 0x36], vec![0; 8]), 6);
    assert!(d.tick(3_000).is_empty());
}

#[test]
fn own_position_from_self_info_and_buffs() {
    let mut d = dec();
    // Someone else's buff: no player event.
    assert!(d
        .on_frame(&f([0x2a, 0x38], buff_a_body(5_471, ME)), 0)
        .is_empty());
    // 33 36 names us (first varint).
    let mut info = varint_bytes(9_180);
    info.extend([0x07, b'X']);
    d.on_frame(&f([0x33, 0x36], info), 1);
    let ev = d.on_frame(&f([0x2a, 0x38], buff_a_body(9_180, ME)), 2);
    let [WireEvent::Player { pos, heading, .. }] = ev.as_slice() else {
        panic!("{ev:?}")
    };
    assert_eq!(*pos, [-450.0, -1200.0, 150.0]);
    assert_eq!(*heading, None);
}

#[test]
fn self_buffs_identify_us_without_self_info() {
    let mut d = dec();
    let mut events = Vec::new();
    for i in 0..4 {
        events.extend(d.on_frame(&f([0x2a, 0x38], buff_a_body(9_180, ME)), i));
    }
    assert_eq!(d.status(10).self_id, Some(9_180));
    assert!(events.iter().any(|e| matches!(e, WireEvent::Player { .. })));
}

#[test]
fn map_load_gives_our_position_and_a_new_map_is_a_zone_change() {
    let mut d = dec();
    let ev = d.on_frame(&f([0x21, 0x36], map_load_body(1010, ME)), 0);
    assert!(matches!(ev[..], [WireEvent::Player { .. }]));
    assert_eq!(d.status(0).zone.as_deref(), Some("Verteron (1010)"));
    // Same map again (a teleport inside the zone): no zone change.
    let ev = d.on_frame(&f([0x21, 0x36], map_load_body(1010, ME)), 1);
    assert!(!ev
        .iter()
        .any(|e| matches!(e, WireEvent::ZoneChanged { .. })));
    let ev = d.on_frame(&f([0x21, 0x36], map_load_body(2020, ME)), 2);
    assert!(ev
        .iter()
        .any(|e| matches!(e, WireEvent::ZoneChanged { .. })));
}

#[test]
fn positions_near_the_origin_or_out_of_bounds_are_rejected() {
    assert!(!is_world_position([-108.0, -31.0, -95.0]));
    assert!(!is_world_position([500_000.0, 0.0, 0.0]));
    assert!(!is_world_position([f32::NAN, 5_000.0, 0.0]));
    assert!(is_world_position(ME));
}

#[test]
fn opening_with_the_key_teaches_a_new_cube_template() {
    let mut d = dec();
    d.on_frame(&f([0x34, 0x36], spawn_body(42, 1_399_999, CUBE, 0)), 0);
    let mut done = varint_bytes(42);
    done.extend(varint_bytes(9_180));
    done.extend(1u32.to_le_bytes());
    done.extend(630_900_003u32.to_le_bytes());
    d.on_frame(&f([0x3a, 0x36], done), 1);
    assert!(d.is_cube_template(1_399_999));
    assert_eq!(d.status(1).learned_templates, vec![1_399_999]);
    // The next one of that template is shown.
    let ev = d.on_frame(&f([0x34, 0x36], spawn_body(43, 1_399_999, CUBE, 0)), 2);
    assert!(matches!(ev[..], [WireEvent::CubeSpawned { .. }]));
}

#[test]
fn a_new_connection_forgets_ids() {
    let mut d = dec();
    d.on_frame(&f([0x34, 0x36], spawn_body(7, 1_200_039, CUBE, 0)), 0);
    d.on_new_connection();
    assert_eq!(d.status(0).cubes_live, 0);
    assert_eq!(d.status(0).self_id, None);
}

#[test]
fn config_with_a_bad_opcode_is_refused() {
    let bad = DEFAULT_CONFIG_JSON.replace("\"objectSpawn\": \"3436\"", "\"objectSpawn\": \"34\"");
    assert!(Aion2Config::from_json(&bad).is_err());
    assert!(Aion2Config::from_json(DEFAULT_CONFIG_JSON).is_ok());
}
