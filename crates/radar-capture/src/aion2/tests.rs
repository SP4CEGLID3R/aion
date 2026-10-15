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

/// A config with one extra, unconfirmed cube type.
fn with_hypothesis(on: bool) -> Aion2Config {
    let mut cfg = Aion2Config::builtin();
    cfg.cube_templates.push(CubeTemplate {
        template: 1_399_999,
        zone: "Elsewhere?".into(),
        status: TemplateStatus::Hypothesis,
        evidence: "test".into(),
    });
    cfg.use_hypotheses = on;
    cfg
}

#[test]
fn builtin_config_is_valid_and_knows_verteron_and_altgard() {
    let d = dec();
    assert!(d.is_cube_template(1_200_039));
    assert!(d.is_cube_template(1_200_041));
    assert!(d.is_cube_template(1_301_199), "Altgard, confirmed in game");
    let off = Aion2Decoder::new(with_hypothesis(false)).unwrap();
    assert!(
        !off.is_cube_template(1_399_999),
        "hypotheses are off by default"
    );
    let on = Aion2Decoder::new(with_hypothesis(true)).unwrap();
    assert!(on.is_cube_template(1_399_999));
}

#[test]
fn altgard_cubes_are_labelled_as_confirmed() {
    let mut d = dec();
    let ev = d.on_frame(&f([0x34, 0x36], spawn_body(5, 1_301_199, CUBE, 0)), 0);
    let [WireEvent::CubeSpawned { label, .. }] = ev.as_slice() else {
        panic!("{ev:?}")
    };
    assert_eq!(label.as_deref(), Some("Hidden Cube (Altgard)"));
}

#[test]
fn unconfirmed_types_are_shown_only_when_asked_and_say_so() {
    let mut d = Aion2Decoder::new(with_hypothesis(true)).unwrap();
    let ev = d.on_frame(&f([0x34, 0x36], spawn_body(5, 1_399_999, CUBE, 0)), 0);
    let [WireEvent::CubeSpawned { label, .. }] = ev.as_slice() else {
        panic!("{ev:?}")
    };
    assert_eq!(
        label.as_deref(),
        Some("Hidden Cube? (Elsewhere, unconfirmed type)")
    );

    let mut off = Aion2Decoder::new(with_hypothesis(false)).unwrap();
    assert!(off
        .on_frame(&f([0x34, 0x36], spawn_body(5, 1_399_999, CUBE, 0)), 0)
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

fn identified(id: u64) -> Aion2Decoder {
    let mut d = dec();
    let mut info = varint_bytes(id);
    info.extend([0u8; 8]);
    d.on_frame(&f([0x33, 0x36], info), 1);
    d
}

/// Walking east at 5 m/s from ME.
fn east(t: Millis) -> [f32; 3] {
    [ME[0] + 0.5 * t as f32, ME[1], ME[2]]
}

#[test]
fn between_server_fixes_the_position_is_estimated_from_objects_entering_range() {
    let mut d = identified(9180);
    let ev = d.on_frame(&f([0x2a, 0x38], buff_a_body(9180, ME)), 10);
    assert!(matches!(ev[..], [WireEvent::Player { accuracy: None, .. }]));
    assert_eq!(d.status(10).position_source, "server");

    // Objects of a template with a known 50 m enter radius appear ahead of
    // us, 50 m from where we really are, as we walk east.
    let mut estimates = Vec::new();
    for k in 0..24u64 {
        let t = 500 + k as Millis * 250;
        let me = east(t);
        let a = (k as f32 * 37.0 % 120.0 - 60.0).to_radians(); // ahead, ±60°
        let q = [me[0] + 5_000.0 * a.cos(), me[1] + 5_000.0 * a.sin(), me[2]];
        let ev = d.on_frame(&f([0x34, 0x36], spawn_body(1_000 + k, 1_500_004, q, 0)), t);
        for e in ev {
            if let WireEvent::Player {
                pos,
                accuracy,
                heading,
                ..
            } = e
            {
                estimates.push((t, pos, accuracy, heading));
            }
        }
    }
    assert!(estimates.len() > 10, "{estimates:?}");
    let (t, pos, accuracy, heading) = *estimates.last().unwrap();
    let truth = d.to_world(east(t));
    let err = (pos[0] - truth[0]).hypot(pos[1] - truth[1]);
    assert!(err < 6.0, "error {err} m at {t}: {pos:?} vs {truth:?}");
    assert!(accuracy.is_some_and(|a| a < 15.0), "{accuracy:?}");
    // Moving east: the heading comes from the direction of travel.
    let h = heading.expect("heading from motion");
    assert!((h - 90.0).abs() < 25.0, "{h}");
    let st = d.status(t);
    assert_eq!(st.position_source, "estimate");
    assert!(st.estimate.range_updates > 10);
    // Between events, a tick moves the estimate on.
    assert!(d.tick(t + 400).iter().any(|e| matches!(
        e,
        WireEvent::Player {
            accuracy: Some(_),
            ..
        }
    )));
}

#[test]
fn freshly_created_objects_say_nothing_about_where_we_are() {
    let mut d = identified(9180);
    // Server clock from a heartbeat.
    let now_ms: i64 = 1_791_883_623_000;
    d.on_frame(&f([0x00, 0x36], now_ms.to_le_bytes().to_vec()), 0);
    d.on_frame(&f([0x2a, 0x38], buff_a_body(9180, ME)), 10);
    for k in 0..10u64 {
        let t = 500 + k as Millis * 250;
        let q = [ME[0] + 5_000.0, ME[1] + k as f32 * 300.0, ME[2]];
        // Created 1 s ago: it appeared, it did not come into range.
        let created = now_ms + t - 1_000;
        let ev = d.on_frame(
            &f([0x34, 0x36], spawn_body(2_000 + k, 1_500_004, q, created)),
            t,
        );
        assert!(ev.is_empty(), "{ev:?}");
    }
    assert_eq!(d.status(3_000).estimate.range_updates, 0);
}

#[test]
fn client_movement_reports_drive_the_position_once_learned() {
    let mut cfg = Aion2Config::builtin();
    cfg.own_movement = Some(MoveSource {
        opcode: "557a".into(),
        anchor: Anchor::End,
        offset: 14,
    });
    let mut d = Aion2Decoder::new(cfg).unwrap();
    let mut payload = vec![0x55, 0x7a, 0x01];
    payload.extend(xyz(east(1_000)));
    payload.extend([0x10, 0x00]);
    let frame = Frame {
        opcode: [0x55, 0x7a],
        payload,
        bundled: false,
    };
    let ev = d.on_client_frame(&frame, 1_000);
    let [WireEvent::Player {
        pos,
        accuracy: None,
        ..
    }] = ev.as_slice()
    else {
        panic!("{ev:?}")
    };
    assert_eq!(*pos, d.to_world(east(1_000)));
    let st = d.status(1_000);
    assert_eq!(st.position_source, "client");
    assert_eq!(st.movement.state, "found");
}

#[test]
fn learned_radii_and_movement_are_kept_for_the_next_session() {
    let mut d = identified(9180);
    // Fixes every 250 ms while objects of an unknown template enter 42 m away.
    for k in 0..6u64 {
        let t = 1_000 + k as Millis * 250;
        let me = east(t);
        d.on_frame(&f([0x2a, 0x38], buff_a_body(9180, me)), t);
        let q = [me[0], me[1] + 4_200.0, me[2]];
        d.on_frame(&f([0x34, 0x36], spawn_body(3_000 + k, 1_234_567, q, 0)), t);
    }
    let learned = d.learned();
    let r = learned.object_radii["1234567"];
    assert!((r.enter.unwrap() - 4_200.0).abs() < 1.0, "{r:?}");
    // The built-in seeds are not written out as if measured.
    assert!(!learned.object_radii.contains_key("1500004"));

    let mut cfg = Aion2Config::builtin();
    cfg.apply_learned(&learned);
    assert_eq!(cfg.object_radii["1234567"].enter, r.enter);
    let text = serde_json::to_string(&learned).unwrap();
    assert_eq!(
        serde_json::from_str::<Aion2Learned>(&text).unwrap(),
        learned
    );
}

fn minimap_sample(t: Millis, x: f64, y: f64) -> crate::minimap::MinimapSample {
    crate::minimap::MinimapSample {
        t,
        x,
        y,
        seg: 0,
        good: true,
        brk: false,
        black: false,
        frozen: false,
        peak: 0.9,
        roundness: 0.5,
        ridge: 0.065,
        cone: Some(90.0), // camera looking east on the (north-up) minimap
        cone_strength: 0.7,
        ring_r: None,
        ui_scale: 1.0,
    }
}

#[test]
fn the_minimap_moves_the_player_between_server_fixes() {
    let mut d = identified(9180);
    // Minimap frames every 100 ms; the player walks east at 5 m/s:
    // 500 units/s × 0.0103 px/unit ≈ 5.15 px/s on the minimap (+x).
    let k = minimap::SEED_K;
    let mut player_events = Vec::new();
    for i in 0..60i64 {
        let t = 1_000 + i * 100;
        let me = east(t);
        let px = (me[0] - ME[0]) as f64 * k;
        for e in d.on_minimap(&minimap_sample(t, 10.0 + px, 20.0)) {
            player_events.push((t, e));
        }
        if i == 2 {
            // One wing opening anchors the track.
            let ev = d.on_frame(&f([0x2a, 0x38], buff_a_body(9180, east(t + 50))), t + 50);
            // The server's own report: its height is measured.
            assert!(matches!(
                ev[..],
                [WireEvent::Player {
                    height_accuracy: None,
                    ..
                }]
            ));
        }
    }
    let st = d.status(7_000);
    assert_eq!(st.minimap.state, "tracking", "{:?}", st.minimap);
    assert_eq!(st.position_source, "minimap");
    // Positions follow the walk, carry an accuracy and the camera heading.
    let (t, last) = player_events.last().unwrap();
    let WireEvent::Player {
        pos,
        heading,
        accuracy,
        height_accuracy,
        ..
    } = last
    else {
        panic!("{last:?}")
    };
    let truth = d.to_world(east(*t));
    assert!(
        (pos[0] - truth[0]).abs() < 0.5 && (pos[1] - truth[1]).abs() < 0.5,
        "{pos:?} vs {truth:?}"
    );
    assert!(accuracy.is_some_and(|a| a < 5.0), "{accuracy:?}");
    assert_eq!(*heading, Some(90.0));
    // The height is still the fix's: the further from it, the less sure.
    let fix = d.to_world(east(1_250));
    let moved = (truth[0] - fix[0]).hypot(truth[1] - fix[1]);
    assert!(moved > 20.0, "{moved}");
    assert!(
        height_accuracy.is_some_and(|h| (h - (0.5 + 0.2 * moved)).abs() < 0.5),
        "{height_accuracy:?} after {moved} m"
    );
    // Never back in time.
    let ts: Vec<Millis> = player_events
        .iter()
        .map(|(_, e)| match e {
            WireEvent::Player { t, .. } => *t,
            _ => 0,
        })
        .collect();
    assert!(ts.windows(2).all(|w| w[0] <= w[1]), "{ts:?}");
}

#[test]
fn minimap_calibration_is_kept_for_the_next_session() {
    let mut d = identified(9180);
    let k = minimap::SEED_K;
    // Fixes every 3 s while walking east: legs calibrate the scale.
    for i in 0..400i64 {
        let t = 1_000 + i * 100;
        let px = (east(t)[0] - ME[0]) as f64 * k;
        d.on_minimap(&minimap_sample(t, px, 0.0));
        if i % 30 == 5 {
            d.on_frame(&f([0x2a, 0x38], buff_a_body(9180, east(t + 50))), t + 50);
        }
    }
    let learned = d.learned();
    let cal = learned.minimap.clone().expect("calibrated");
    assert!(cal.legs >= 3, "{cal:?}");
    assert!((cal.k / k - 1.0).abs() < 0.02, "{cal:?}");
    let mut cfg = Aion2Config::builtin();
    cfg.apply_learned(&learned);
    assert_eq!(cfg.minimap_hint, Some(cal));
}

#[test]
fn between_fixes_the_height_follows_the_ground_around_us() {
    let mut d = identified(9180);
    let k = minimap::SEED_K;
    // A hillside rising 1 m per 4 m to the east; things stand around on it.
    let ground = |x: f32| ME[2] + (x - ME[0]) * 0.25;
    let mut id = 70_000;
    for i in 0..20 {
        for j in -3..=3 {
            let x = ME[0] - 2_000.0 + i as f32 * 400.0 + (j * 37) as f32;
            let y = ME[1] + j as f32 * 500.0;
            let wobble = ((i * 7 + j * 3) % 5) as f32 * 8.0 - 16.0;
            id += 1;
            d.on_frame(
                &f(
                    [0x34, 0x36],
                    spawn_body(id, 1_500_041, [x, y, ground(x) + wobble], 0),
                ),
                900,
            );
        }
    }
    let on_hill = |t: Millis| {
        let p = east(t);
        [p[0], p[1], ground(p[0])]
    };
    let mut last = None;
    for i in 0..60i64 {
        let t = 1_000 + i * 100;
        let px = (on_hill(t)[0] - ME[0]) as f64 * k;
        for e in d.on_minimap(&minimap_sample(t, 10.0 + px, 20.0)) {
            last = Some((t, e));
        }
        if i == 2 || i == 12 {
            // Two buffs on the way: the model is checked against them.
            d.on_frame(&f([0x2a, 0x38], buff_a_body(9180, on_hill(t + 50))), t + 50);
        }
    }
    let (
        t,
        WireEvent::Player {
            pos,
            height_accuracy,
            ..
        },
    ) = last.unwrap()
    else {
        panic!()
    };
    // 23 m on from the last buff, 6 m higher: the height came along.
    let truth = d.to_world(on_hill(t));
    assert!(
        (pos[2] - truth[2]).abs() < 0.6,
        "{} vs {}",
        pos[2],
        truth[2]
    );
    assert!(truth[2] - d.to_world(on_hill(2_250))[2] > 5.0);
    assert!(
        height_accuracy.is_some_and(|h| h < 1.5),
        "{height_accuracy:?}"
    );
    assert_eq!(d.status(t).ground.checks, 2);
}

#[test]
fn a_removal_captured_just_before_a_presence_tick_still_removes_the_cube() {
    use radar_core::{CoordinateTransform, StaleDisplay, Tracker, TrackerConfig};
    use radar_ingest::EventAdapter;
    let adapter = EventAdapter::new(CoordinateTransform::default()).unwrap();
    let mut tracker = Tracker::new(TrackerConfig::default());
    let mut feed = |evs: Vec<WireEvent>, now: Millis| {
        for e in evs {
            tracker.apply(adapter.convert(e).unwrap(), now);
        }
    };
    let mut d = dec();
    let base: Millis = 1_791_000_000_000;
    feed(
        d.on_frame(
            &f([0x34, 0x36], spawn_body(7, 1_200_039, CUBE, 0)),
            base + 1_000,
        ),
        base + 1_000,
    );
    feed(
        d.on_frame(&f([0x00, 0x36], vec![0; 8]), base + 2_000),
        base + 2_000,
    );
    // The removal was captured at 2 490 but is still in the capture buffer
    // when the loop's presence tick runs at 2 500 (wall clock).
    feed(d.tick(base + 2_500), base + 2_500);
    feed(
        d.on_frame(&f([0x35, 0x36], removed_body(7, 0x04)), base + 2_490),
        base + 2_510,
    );
    let frame = tracker.frame(base + 2_520, StaleDisplay::Hide);
    assert!(frame.cubes.is_empty(), "still shown: {:?}", frame.cubes);
}
