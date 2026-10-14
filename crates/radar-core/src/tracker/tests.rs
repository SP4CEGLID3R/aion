use super::*;
use crate::model::{CubeEvent, CubeEventKind, PlayerUpdate, Provenance, RadarEvent};

const T0: Millis = 1_000_000;

fn tracker() -> Tracker {
    Tracker::new(TrackerConfig {
        stale_timeout_ms: 10_000,
        stale_retain_ms: 5_000,
        tombstone_ttl_ms: 60_000,
        max_clock_skew_ms: 1_000,
        history_capacity: 100,
    })
}

fn cube(kind: CubeEventKind, id: &str, at: Millis) -> CubeEvent {
    CubeEvent {
        id: Some(id.to_owned()),
        kind,
        pos: Some(WorldPos::new(10.0, 20.0, 0.0)),
        label: Some(format!("Cube {id}")),
        zone: None,
        provenance: Provenance::Live,
        observed_at: at,
    }
}

fn spawn(id: &str, at: Millis) -> RadarEvent {
    RadarEvent::Cube(cube(CubeEventKind::Spawned, id, at))
}

fn seen(id: &str, at: Millis) -> RadarEvent {
    RadarEvent::Cube(CubeEvent {
        pos: None,
        ..cube(CubeEventKind::Seen, id, at)
    })
}

fn despawn(id: &str, at: Millis) -> RadarEvent {
    RadarEvent::Cube(CubeEvent {
        pos: None,
        ..cube(CubeEventKind::Despawned, id, at)
    })
}

fn collect(id: &str, at: Millis) -> RadarEvent {
    RadarEvent::Cube(CubeEvent {
        pos: None,
        ..cube(CubeEventKind::Collected, id, at)
    })
}

fn player(x: f64, y: f64, at: Millis) -> RadarEvent {
    RadarEvent::Player(PlayerUpdate {
        accuracy_m: None,
        pos: WorldPos::new(x, y, 0.0),
        heading_deg: Some(0.0),
        zone: None,
        observed_at: at,
    })
}

fn markers(t: &Tracker, now: Millis) -> Vec<String> {
    t.frame(now, StaleDisplay::Hide)
        .cubes
        .into_iter()
        .map(|c| c.id)
        .collect()
}

// ---------------------------------------------------------------------------
// Acceptance tests (requirement 7)
// ---------------------------------------------------------------------------

#[test]
fn acceptance_1_confirmed_spawn_adds_one_marker() {
    let mut t = tracker();
    assert_eq!(t.apply(spawn("a", T0), T0), Outcome::Applied);
    let frame = t.frame(T0, StaleDisplay::Hide);
    assert_eq!(frame.cubes.len(), 1);
    assert_eq!(frame.cubes[0].id, "a");
    assert_eq!(frame.cubes[0].state, CubeState::Spawned);
    assert_eq!(frame.stats.spawned, 1);
}

#[test]
fn acceptance_2_despawn_or_collection_removes_that_marker() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    t.apply(spawn("b", T0), T0);

    assert_eq!(t.apply(despawn("a", T0 + 100), T0 + 100), Outcome::Applied);
    assert_eq!(markers(&t, T0 + 100), vec!["b"]);
    assert_eq!(t.cube("a").unwrap().state, CubeState::Despawned);

    assert_eq!(t.apply(collect("b", T0 + 200), T0 + 200), Outcome::Applied);
    assert!(markers(&t, T0 + 200).is_empty());
    assert_eq!(t.cube("b").unwrap().state, CubeState::Collected);

    // Removed cubes stay removed even when stale cubes are displayed.
    assert!(t.frame(T0 + 200, StaleDisplay::MarkStale).cubes.is_empty());
}

#[test]
fn acceptance_3_static_spawn_point_never_creates_a_marker() {
    let mut t = tracker();
    let ev = RadarEvent::Cube(CubeEvent {
        provenance: Provenance::StaticSpawnPoint,
        ..cube(CubeEventKind::Spawned, "sp-1", T0)
    });
    assert_eq!(t.apply(ev, T0), Outcome::Rejected(RejectReason::NotLive));
    assert!(markers(&t, T0).is_empty());
    assert!(t.cube("sp-1").is_none());
    assert_eq!(
        t.diagnostics().not_live_by_provenance["static_spawn_point"],
        1
    );

    // Nor can a later "seen" for that id turn it into a marker.
    t.apply(seen("sp-1", T0 + 1), T0 + 1);
    assert!(markers(&t, T0 + 1).is_empty());
}

#[test]
fn acceptance_4_historical_location_never_creates_a_live_marker() {
    let mut t = tracker();
    // Labelled historical, even with a fresh timestamp.
    let historical = RadarEvent::Cube(CubeEvent {
        provenance: Provenance::Historical,
        ..cube(CubeEventKind::Spawned, "h-1", T0)
    });
    assert_eq!(
        t.apply(historical, T0),
        Outcome::Rejected(RejectReason::NotLive)
    );

    // Labelled live, but observed long ago: it cannot vouch for the present.
    let old = spawn("h-2", T0 - 60_000);
    assert_eq!(t.apply(old, T0), Outcome::Rejected(RejectReason::TooOld));

    // Predicted locations are not observations either.
    let predicted = RadarEvent::Cube(CubeEvent {
        provenance: Provenance::Predicted,
        ..cube(CubeEventKind::Spawned, "p-1", T0)
    });
    assert_eq!(
        t.apply(predicted, T0),
        Outcome::Rejected(RejectReason::NotLive)
    );

    assert!(t.frame(T0, StaleDisplay::MarkStale).cubes.is_empty());
    // They are still visible in the debug history.
    assert!(t.history().any(|h| h.id.as_deref() == Some("h-1")));
}

#[test]
fn acceptance_5_duplicate_events_do_not_create_duplicate_markers() {
    let mut t = tracker();
    assert_eq!(t.apply(spawn("a", T0), T0), Outcome::Applied);
    assert_eq!(t.apply(spawn("a", T0), T0), Outcome::Duplicate);
    assert_eq!(t.apply(spawn("a", T0 + 50), T0 + 50), Outcome::Duplicate);
    assert_eq!(t.apply(seen("a", T0 + 60), T0 + 60), Outcome::Duplicate);
    assert_eq!(markers(&t, T0 + 60), vec!["a"]);

    t.apply(despawn("a", T0 + 70), T0 + 70);
    assert_eq!(t.apply(despawn("a", T0 + 70), T0 + 70), Outcome::Duplicate);
    assert!(markers(&t, T0 + 70).is_empty());
    assert_eq!(t.diagnostics().duplicates, 4);
}

#[test]
fn acceptance_6_missing_or_stale_state_is_never_presented_as_spawned() {
    let mut t = tracker();

    // Missing: a cube that is only "seen", never confirmed spawned.
    t.apply(seen("unconfirmed", T0), T0);
    assert_eq!(t.cube("unconfirmed").unwrap().state, CubeState::Unknown);
    assert!(markers(&t, T0).is_empty());

    // Missing: no stable id, or no position.
    let no_id = RadarEvent::Cube(CubeEvent {
        id: None,
        ..cube(CubeEventKind::Spawned, "", T0)
    });
    assert_eq!(
        t.apply(no_id, T0),
        Outcome::Rejected(RejectReason::MissingId)
    );
    let no_pos = RadarEvent::Cube(CubeEvent {
        pos: None,
        ..cube(CubeEventKind::Spawned, "nopos", T0)
    });
    assert_eq!(
        t.apply(no_pos, T0),
        Outcome::Rejected(RejectReason::MissingPosition)
    );
    assert!(markers(&t, T0).is_empty());

    // Stale: spawned, then nothing for longer than the timeout.
    t.apply(spawn("a", T0), T0);
    assert_eq!(markers(&t, T0 + 10_000), vec!["a"]);
    // Even without tick(), the frame refuses to call it spawned.
    let late = T0 + 10_001;
    assert!(markers(&t, late).is_empty());
    let shown = t.frame(late, StaleDisplay::MarkStale);
    assert_eq!(shown.cubes.len(), 1);
    assert_eq!(shown.cubes[0].state, CubeState::Stale);
    assert_eq!(shown.stats.spawned, 0);

    t.tick(late);
    assert_eq!(t.cube("a").unwrap().state, CubeState::Stale);
    // Retained as stale for a while, then forgotten.
    t.tick(late + 5_001);
    assert!(t.cube("a").is_none());
}

// ---------------------------------------------------------------------------
// Further behaviour
// ---------------------------------------------------------------------------

#[test]
fn seen_refreshes_a_spawned_cube_and_revives_a_stale_one() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    t.apply(seen("a", T0 + 9_000), T0 + 9_000);
    t.tick(T0 + 15_000);
    assert_eq!(markers(&t, T0 + 15_000), vec!["a"]);

    t.tick(T0 + 19_001);
    assert_eq!(t.cube("a").unwrap().state, CubeState::Stale);
    assert_eq!(
        t.apply(seen("a", T0 + 19_500), T0 + 19_500),
        Outcome::Applied
    );
    assert_eq!(t.cube("a").unwrap().state, CubeState::Spawned);
    assert_eq!(markers(&t, T0 + 19_500), vec!["a"]);
}

#[test]
fn late_older_spawn_cannot_resurrect_a_despawned_cube() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    t.apply(despawn("a", T0 + 500), T0 + 500);
    assert_eq!(
        t.apply(spawn("a", T0 + 400), T0 + 600),
        Outcome::Rejected(RejectReason::OutOfOrder)
    );
    assert!(markers(&t, T0 + 600).is_empty());
}

#[test]
fn despawn_before_spawn_leaves_a_tombstone() {
    let mut t = tracker();
    t.apply(despawn("a", T0 + 500), T0 + 500);
    assert_eq!(
        t.apply(spawn("a", T0 + 100), T0 + 600),
        Outcome::Rejected(RejectReason::OutOfOrder)
    );
    assert!(markers(&t, T0 + 600).is_empty());
}

#[test]
fn a_newer_spawn_after_despawn_is_a_respawn() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    t.apply(despawn("a", T0 + 100), T0 + 100);
    assert_eq!(t.apply(spawn("a", T0 + 200), T0 + 200), Outcome::Applied);
    assert_eq!(markers(&t, T0 + 200), vec!["a"]);
}

#[test]
fn seen_after_despawn_without_respawn_is_unknown_not_spawned() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    t.apply(despawn("a", T0 + 100), T0 + 100);
    t.apply(seen("a", T0 + 200), T0 + 200);
    assert_eq!(t.cube("a").unwrap().state, CubeState::Unknown);
    assert!(markers(&t, T0 + 200).is_empty());
}

#[test]
fn future_and_non_finite_events_are_rejected() {
    let mut t = tracker();
    assert_eq!(
        t.apply(spawn("a", T0 + 5_000), T0),
        Outcome::Rejected(RejectReason::FutureTimestamp)
    );
    let nan = RadarEvent::Cube(CubeEvent {
        pos: Some(WorldPos::new(f64::NAN, 0.0, 0.0)),
        ..cube(CubeEventKind::Spawned, "n", T0)
    });
    assert_eq!(
        t.apply(nan, T0),
        Outcome::Rejected(RejectReason::NonFinitePosition)
    );
    let bad_player = RadarEvent::Player(PlayerUpdate {
        accuracy_m: None,
        pos: WorldPos::new(0.0, f64::INFINITY, 0.0),
        heading_deg: None,
        zone: None,
        observed_at: T0,
    });
    assert_eq!(
        t.apply(bad_player, T0),
        Outcome::Rejected(RejectReason::NonFinitePosition)
    );
    assert!(markers(&t, T0).is_empty());
}

#[test]
fn zone_change_drops_every_cube_and_wrong_zone_events_are_rejected() {
    let mut t = tracker();
    t.apply(
        RadarEvent::ZoneChanged {
            zone: "town".into(),
            observed_at: T0,
        },
        T0,
    );
    t.apply(spawn("a", T0), T0);
    assert_eq!(markers(&t, T0), vec!["a"]);

    t.apply(
        RadarEvent::ZoneChanged {
            zone: "dungeon".into(),
            observed_at: T0 + 10,
        },
        T0 + 10,
    );
    assert!(markers(&t, T0 + 10).is_empty());
    assert!(t.cube("a").is_none());

    let wrong = RadarEvent::Cube(CubeEvent {
        zone: Some("town".into()),
        ..cube(CubeEventKind::Spawned, "b", T0 + 20)
    });
    assert_eq!(
        t.apply(wrong, T0 + 20),
        Outcome::Rejected(RejectReason::WrongZone)
    );
}

#[test]
fn a_player_in_a_new_zone_implies_a_zone_change() {
    let mut t = tracker();
    let p = |zone: &str, at| {
        RadarEvent::Player(PlayerUpdate {
            accuracy_m: None,
            pos: WorldPos::new(0.0, 0.0, 0.0),
            heading_deg: None,
            zone: Some(zone.into()),
            observed_at: at,
        })
    };
    t.apply(p("town", T0), T0);
    t.apply(spawn("a", T0), T0);
    t.apply(p("dungeon", T0 + 10), T0 + 10);
    assert_eq!(t.zone(), Some("dungeon"));
    assert!(markers(&t, T0 + 10).is_empty());
}

#[test]
fn frame_reports_distance_and_bearing_from_the_player() {
    let mut t = tracker();
    t.apply(player(10.0, 10.0, T0), T0);
    t.apply(spawn("a", T0), T0); // cube at (10, 20): 10 units due north
    let frame = t.frame(T0, StaleDisplay::Hide);
    let c = &frame.cubes[0];
    assert!((c.distance.unwrap() - 10.0).abs() < 1e-9);
    assert!(c.bearing_deg.unwrap().abs() < 1e-9);
    assert!(!frame.player.unwrap().stale);
}

#[test]
fn frame_without_player_has_no_distances_and_flags_stale_player() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    let frame = t.frame(T0, StaleDisplay::Hide);
    assert!(frame.player.is_none());
    assert_eq!(frame.cubes[0].distance, None);

    t.apply(player(0.0, 0.0, T0), T0);
    let later = t.frame(T0 + 10_001, StaleDisplay::Hide);
    assert!(later.player.unwrap().stale);
}

#[test]
fn out_of_order_player_updates_are_ignored() {
    let mut t = tracker();
    t.apply(player(5.0, 5.0, T0 + 100), T0 + 100);
    assert_eq!(
        t.apply(player(0.0, 0.0, T0), T0 + 100),
        Outcome::Rejected(RejectReason::OutOfOrder)
    );
    let p = t.frame(T0 + 100, StaleDisplay::Hide).player.unwrap();
    assert_eq!(p.pos, WorldPos::new(5.0, 5.0, 0.0));
}

#[test]
fn tombstones_and_unknowns_expire() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    t.apply(despawn("a", T0 + 1), T0 + 1);
    t.apply(seen("u", T0), T0);
    t.tick(T0 + 10_001);
    assert!(t.cube("u").is_none());
    assert!(t.cube("a").is_some());
    t.tick(T0 + 60_002);
    assert!(t.cube("a").is_none());
}

#[test]
fn history_is_bounded_and_records_rejections() {
    let mut t = Tracker::new(TrackerConfig {
        history_capacity: 3,
        ..TrackerConfig::default()
    });
    for i in 0..10 {
        t.apply(spawn(&format!("c{i}"), T0), T0);
    }
    assert_eq!(t.history().count(), 3);
    assert_eq!(t.history().last().unwrap().id.as_deref(), Some("c9"));

    let mut t = tracker();
    t.apply(
        RadarEvent::Cube(CubeEvent {
            provenance: Provenance::StaticSpawnPoint,
            ..cube(CubeEventKind::Spawned, "sp", T0)
        }),
        T0,
    );
    let h = t.history().last().unwrap();
    assert_eq!(h.outcome, "rejected:not_live");
    assert_eq!(h.provenance, Some(Provenance::StaticSpawnPoint));
}

#[test]
fn frame_serialises_with_camel_case_and_screaming_states() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    let json = serde_json::to_string(&t.frame(T0, StaleDisplay::Hide)).unwrap();
    assert!(json.contains("\"generatedAt\""));
    assert!(json.contains("\"state\":\"SPAWNED\""));
    assert!(json.contains("\"confirmedAgeMs\""));
}

#[test]
fn a_lost_cube_is_stale_at_once_never_live_and_a_new_spawn_revives_it() {
    let mut t = tracker();
    t.apply(spawn("a", T0), T0);
    let lost = RadarEvent::Cube(CubeEvent {
        pos: None,
        ..cube(CubeEventKind::Lost, "a", T0 + 100)
    });
    assert_eq!(t.apply(lost, T0 + 100), Outcome::Applied);
    assert!(markers(&t, T0 + 100).is_empty());
    let shown = t.frame(T0 + 100, StaleDisplay::MarkStale);
    assert_eq!(shown.cubes[0].state, CubeState::Stale);

    // Back in range: the source reports the spawn again.
    assert_eq!(t.apply(spawn("a", T0 + 200), T0 + 200), Outcome::Applied);
    assert_eq!(markers(&t, T0 + 200), vec!["a"]);

    // Losing an unknown cube changes nothing.
    let unknown = RadarEvent::Cube(CubeEvent {
        pos: None,
        ..cube(CubeEventKind::Lost, "zz", T0 + 300)
    });
    assert_eq!(t.apply(unknown, T0 + 300), Outcome::Duplicate);
    assert!(t.cube("zz").is_none());
}
