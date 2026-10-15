//! End-to-end tests of the tracker on synthetic minimap frames.

use crate::overlay::circ_diff;
use crate::plane::Bgra;
use crate::synth::{frame, Overlay, SynthMap};
use crate::tracker::{MinimapTracker, Step, TrackerConfig};

/// Player path in texture pixels at time `t` (ms): walk, turn, stop, run.
fn path(t: i64) -> (f64, f64) {
    let s = t as f64 / 1000.0;
    // 6 px/s walking with a slow turn, a 6 s stop, then 14 px/s (gliding).
    let walk = |s: f64| (600.0 + 6.0 * s, 500.0 + 25.0 * (s / 8.0).sin());
    if s < 30.0 {
        walk(s)
    } else if s < 36.0 {
        walk(30.0)
    } else {
        let (x, y) = walk(30.0);
        let u = s - 36.0;
        (x + 14.0 * u * 0.8, y - 14.0 * u * 0.6)
    }
}

struct Run {
    steps: Vec<Step>,
    truth: Vec<(f64, f64)>,
    ms_per_frame: f64,
}

fn run(
    w: usize,
    h: usize,
    seconds: i64,
    fps: i64,
    start: (f64, f64),
    path: impl Fn(i64) -> (f64, f64),
) -> Run {
    run_ui(w, h, 1.0, 7, seconds, fps, start, path)
}

#[allow(clippy::too_many_arguments)]
fn run_ui(
    w: usize,
    h: usize,
    ui: f64,
    seed: u64,
    seconds: i64,
    fps: i64,
    start: (f64, f64),
    path: impl Fn(i64) -> (f64, f64),
) -> Run {
    let map = SynthMap::new(1600, 1300, seed);
    let cfg = TrackerConfig {
        ui_scale: ui,
        ..TrackerConfig::default()
    };
    let mut tr = MinimapTracker::new(w, h, cfg);
    let mut steps = Vec::new();
    let mut truth = Vec::new();
    let mut total_ms = 0.0;
    let n = seconds * fps;
    for i in 0..n {
        let t = 1_790_000_000_000 + i * 1000 / fps;
        let (x, y) = path(i * 1000 / fps);
        let s = i as f64 / fps as f64;
        let ov = Overlay {
            cone: (20.0 + 40.0 * (s / 5.0).sin()).rem_euclid(360.0),
            facing: (90.0 + 30.0 * (s / 3.0).cos()).rem_euclid(360.0),
            dots: vec![
                (60.0 + 25.0 * (s * 0.7).sin(), 50.0 + 12.0 * (s * 0.9).cos()),
                (w as f64 * 0.7 + 10.0 * s.sin(), h as f64 * 0.6),
            ],
            ui_scale: ui,
            ..Overlay::default()
        };
        let px = frame(&map, x + start.0, y + start.1, w, h, &ov);
        let f = Bgra::packed(w, h, &px).unwrap();
        let st = tr.step(t, &f);
        total_ms += tr.stats().last_ms;
        steps.push(st);
        truth.push((x, y));
    }
    Run {
        steps,
        truth,
        ms_per_frame: total_ms / n as f64,
    }
}

#[test]
fn follows_a_walk_with_turns_stops_and_gliding() {
    // 2560×1440 default UI, and 1920×1080 (everything 0.75×).
    for (w, h, ui, seed) in [(311, 206, 1.0, 7), (234, 155, 0.75, 3)] {
        let r = run_ui(w, h, ui, seed, 60, 10, (0.0, 0.0), path);
        let breaks = r.steps.iter().filter(|s| s.brk).count();
        assert_eq!(breaks, 0, "{w}x{h}: breaks");
        let (x0, y0) = (r.steps[0].x, r.steps[0].y);
        let (tx0, ty0) = r.truth[0];
        let mut worst: f64 = 0.0;
        for (s, (tx, ty)) in r.steps.iter().zip(&r.truth) {
            let ex = (s.x - x0) - (tx - tx0);
            let ey = (s.y - y0) - (ty - ty0);
            worst = worst.max(ex.hypot(ey));
        }
        let travelled: f64 = r
            .truth
            .windows(2)
            .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
            .sum();
        eprintln!(
            "{w}x{h}: travelled {travelled:.0} px, worst error {worst:.2} px, {:.1} ms/frame",
            r.ms_per_frame
        );
        // At ~1 px per metre: a few metres over a minute of travel.
        assert!(
            worst < 6.0,
            "{w}x{h}: worst error {worst:.2} px over {travelled:.0} px"
        );
    }
}

#[test]
fn standing_still_while_the_camera_turns_reports_no_movement() {
    let r = run(311, 206, 20, 10, (0.0, 0.0), |_| (700.0, 600.0));
    let (x0, y0) = (r.steps[0].x, r.steps[0].y);
    for s in &r.steps {
        assert!(!s.brk);
        assert!(
            (s.x - x0).hypot(s.y - y0) < 0.5,
            "drift {:?}",
            (s.x - x0, s.y - y0)
        );
    }
}

#[test]
fn a_teleport_breaks_the_track() {
    let r = run(311, 206, 6, 10, (0.0, 0.0), |t| {
        if t < 3_000 {
            (600.0 + t as f64 * 0.005, 500.0)
        } else {
            (1_100.0, 900.0) // far away: nothing overlaps
        }
    });
    let before = r.steps[25].seg;
    let after = r.steps.last().unwrap().seg;
    assert!(r.steps.iter().any(|s| s.brk), "no break");
    assert_ne!(before, after);
}

#[test]
fn the_view_cone_gives_the_camera_direction() {
    let map = SynthMap::new(900, 800, 3);
    let mut worst: f64 = 0.0;
    for cone in [0.0, 37.0, 90.0, 145.0, 200.0, 271.0, 333.0] {
        let ov = Overlay {
            cone,
            facing: cone + 25.0,
            ..Overlay::default()
        };
        let px = frame(&map, 450.0, 400.0, 311, 206, &ov);
        let f = Bgra::packed(311, 206, &px).unwrap();
        let g = f.gray();
        let (c, strength) = crate::overlay::cone_angle(&g, (155.5, 103.0), 34.0, 95.0);
        assert!(
            strength >= crate::overlay::CONE_CLEAR,
            "{cone}: strength {strength}"
        );
        worst = worst.max(circ_diff(c, cone));
    }
    assert!(worst <= 3.0, "worst cone error {worst}");
}

#[test]
fn black_and_frozen_captures_are_flagged() {
    let (w, h) = (311, 206);
    let mut tr = MinimapTracker::new(w, h, TrackerConfig::default());
    let black = vec![0u8; w * h * 4];
    let s = tr.step(0, &Bgra::packed(w, h, &black).unwrap());
    assert!(s.black && s.brk && s.x.is_nan());

    let map = SynthMap::new(700, 600, 5);
    let px = frame(&map, 350.0, 300.0, w, h, &Overlay::default());
    let f = Bgra::packed(w, h, &px).unwrap();
    let mut frozen_at = None;
    for i in 0..30 {
        let s = tr.step(1_000 + i * 100, &f);
        assert!(!s.black);
        if s.frozen && frozen_at.is_none() {
            frozen_at = Some(i);
        }
    }
    // Identical frames for 2 s.
    assert_eq!(frozen_at, Some(20));
}

/// Real minimap screenshots (madara134/aion2-cube-finder's MIT test
/// fixtures), when available locally: `MINIMAP_ASSETS=<dir> cargo test -p
/// radar-vision -- --ignored`. Not shipped with this repository.
fn load_png(path: &std::path::Path) -> (usize, usize, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = info.color_type.samples();
    let mut bgra = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let p = &buf[i * ch..i * ch + ch.min(3)];
        bgra[i * 4] = p[2];
        bgra[i * 4 + 1] = p[1];
        bgra[i * 4 + 2] = p[0];
        bgra[i * 4 + 3] = 255;
    }
    (w, h, bgra)
}

#[test]
#[ignore]
fn real_screenshots_view_cone() {
    let Ok(dir) = std::env::var("MINIMAP_ASSETS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    // The map area starts below the 33 px title bar.
    for (name, lo, hi) in [
        ("tolbas_facing_up.png", 300.0, 360.0),
        ("tolbas_facing_right.png", 0.0, 360.0),
    ] {
        let (w, h, px) = load_png(&dir.join(name));
        let top = 33;
        let crop = &px[top * w * 4..];
        let f = Bgra::packed(w, h - top, crop).unwrap();
        let g = f.gray();
        let pivot = (w as f64 / 2.0, (h - top) as f64 / 2.0);
        let (c, strength) = crate::overlay::cone_angle(&g, pivot, 34.0, 95.0);
        eprintln!("{name}: cone {c:.1} strength {strength:.2}");
        assert!(strength >= crate::overlay::CONE_CLEAR, "{name}: {strength}");
        assert!((lo..=hi).contains(&c) || hi - lo >= 360.0, "{name}: {c}");
    }
}

#[test]
#[ignore]
fn real_texture_walk() {
    let Ok(dir) = std::env::var("MINIMAP_ASSETS") else {
        return;
    };
    let (w0, h0, px) = load_png(&std::path::Path::new(&dir).join("aullaeu_panel.png"));
    // Mirror the panel to 2×2 like the reference project, as a large texture.
    let (tw, th) = (w0 * 2, h0 * 2);
    let mut tex = vec![0u8; tw * th * 4];
    for y in 0..th {
        for x in 0..tw {
            let sx = if x < w0 { x } else { tw - 1 - x };
            let sy = if y < h0 { y } else { th - 1 - y };
            let (si, di) = ((sy * w0 + sx) * 4, (y * tw + x) * 4);
            tex[di..di + 4].copy_from_slice(&px[si..si + 4]);
        }
    }
    let (w, h) = (311usize, 206usize);
    let mut tr = MinimapTracker::new(w, h, TrackerConfig::default());
    let mut worst: f64 = 0.0;
    let mut first = None;
    let mut frame_buf = vec![0u8; w * h * 4];
    for i in 0..400i64 {
        let s = i as f64 / 10.0;
        // A loop well inside the texture, integer steps (no resampling).
        let cx = 330.0 + 120.0 * (s / 7.0).sin();
        let cy = 330.0 + 90.0 * (s / 5.0).cos();
        let (ix, iy) = (cx.round() as usize, cy.round() as usize);
        for y in 0..h {
            let src = ((iy - h / 2 + y) * tw + ix - w / 2) * 4;
            frame_buf[y * w * 4..(y + 1) * w * 4].copy_from_slice(&tex[src..src + w * 4]);
        }
        let st = tr.step(i * 100, &Bgra::packed(w, h, &frame_buf).unwrap());
        let f0 = *first.get_or_insert((st.x, st.y, ix as f64, iy as f64));
        let e = (
            (st.x - f0.0) - (ix as f64 - f0.2),
            (st.y - f0.1) - (iy as f64 - f0.3),
        );
        worst = worst.max(e.0.hypot(e.1));
    }
    eprintln!("real texture: worst {worst:.2} px, {:?}", tr.stats());
    assert!(worst < 4.0, "{worst}");
}

#[test]
fn a_view_with_only_a_straight_road_is_flagged() {
    // Flat grass with one long road: movement along it cannot be measured.
    let (w, h) = (311usize, 206usize);
    let mut px_map = vec![[120.0f32, 150.0, 90.0]; 1200 * 600];
    for y in 0..600 {
        for x in 0..1200 {
            // Mild noise so the grass is not perfectly flat.
            let n = (((x * 7919 + y * 104_729) % 97) as f32 / 97.0 - 0.5) * 3.0;
            let p = &mut px_map[y * 1200 + x];
            p[0] += n;
            p[1] += n;
            if (296..304).contains(&y) {
                *p = [205.0, 185.0, 140.0];
            }
        }
    }
    let mut tr = MinimapTracker::new(w, h, TrackerConfig::default());
    let mut ridged = 0;
    for i in 0..40i64 {
        let cx = 300 + i as usize * 2; // running along the road
        let mut frame = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let p = px_map[(300 - h / 2 + y) * 1200 + cx - w / 2 + x];
                let o = (y * w + x) * 4;
                frame[o] = p[2] as u8;
                frame[o + 1] = p[1] as u8;
                frame[o + 2] = p[0] as u8;
                frame[o + 3] = 255;
            }
        }
        let s = tr.step(i * 100, &Bgra::packed(w, h, &frame).unwrap());
        if !s.good || s.roundness < crate::tracker::RIDGE {
            ridged += 1;
        }
    }
    // Nearly every frame is either rejected or flagged as a ridge.
    assert!(ridged >= 35, "{ridged}");
}

#[test]
fn picks_the_track_up_again_after_the_minimap_was_covered() {
    let (w, h) = (311usize, 206usize);
    let map = SynthMap::new(1600, 1300, 7);
    // The big map or a dialog: a different picture altogether.
    let other = SynthMap::new(700, 700, 99);
    let at = |t: i64| (600.0 + 0.004 * t as f64, 500.0 + 0.002 * t as f64);
    for black in [true, false] {
        let mut tr = MinimapTracker::new(w, h, TrackerConfig::default());
        let ov = Overlay::default();
        let mut reference = None;
        let mut worst: f64 = 0.0;
        for i in 0..160i64 {
            let t = i * 100;
            // Covered for 4 s while walking on (18 px).
            let covered = (80..120).contains(&i);
            let px = match (covered, black) {
                (true, true) => vec![0u8; w * h * 4],
                (true, false) => frame(&other, 350.0, 350.0, w, h, &ov),
                _ => {
                    let (x, y) = at(t);
                    frame(&map, x, y, w, h, &ov)
                }
            };
            let s = tr.step(1_790_000_000_000 + t, &Bgra::packed(w, h, &px).unwrap());
            if i == 79 {
                reference = Some((s.seg, s.x, s.y, at(t)));
            }
            if black && i == 120 {
                // Straight back on the first frame after black ones.
                assert_eq!(s.seg, reference.unwrap().0, "first frame back");
                assert!(s.good);
            }
            if i >= 122 {
                let (seg, x0, y0, (tx0, ty0)) = reference.unwrap();
                assert_eq!(s.seg, seg, "black={black} i={i}: not picked up again");
                let (tx, ty) = at(t);
                worst = worst.max(((s.x - x0) - (tx - tx0)).hypot((s.y - y0) - (ty - ty0)));
            }
        }
        assert!(worst < 2.0, "black={black}: worst {worst:.2} px");
        assert!(tr.stats().resumed >= 1);
    }
}

/// Tracks `seconds` of `path` with the arrow at `pivot` and the pivot
/// finder on; returns the run, the frame the pivot moved on, and the tracker.
fn run_off_centre(
    (w, h, ui, seed): (usize, usize, f64, u64),
    pivot: (f64, f64),
    seconds: i64,
    path: impl Fn(i64) -> (f64, f64),
) -> (Run, Option<usize>, MinimapTracker) {
    let map = SynthMap::new(1600, 1300, seed);
    let cfg = TrackerConfig {
        ui_scale: ui,
        locate_pivot: true,
        ..TrackerConfig::default()
    };
    let mut tr = MinimapTracker::new(w, h, cfg);
    let (mut steps, mut truth, mut moved) = (Vec::new(), Vec::new(), None);
    let fps = 10;
    for i in 0..seconds * fps {
        let t = 1_790_000_000_000 + i * 1000 / fps;
        let (x, y) = path(i * 1000 / fps);
        let s = i as f64 / fps as f64;
        let ov = Overlay {
            cone: (20.0 + 40.0 * (s / 5.0).sin()).rem_euclid(360.0),
            facing: (90.0 + 30.0 * (s / 3.0).cos()).rem_euclid(360.0),
            dots: vec![(60.0 + 25.0 * (s * 0.7).sin(), 50.0 + 12.0 * (s * 0.9).cos())],
            ui_scale: ui,
            pivot: Some(pivot),
            ..Overlay::default()
        };
        let px = frame(&map, x, y, w, h, &ov);
        let st = tr.step(t, &Bgra::packed(w, h, &px).unwrap());
        if st.recentred {
            assert!(moved.is_none(), "the pivot moves once");
            moved = Some(i as usize);
        }
        steps.push(st);
        truth.push((x, y));
    }
    let run = Run {
        steps,
        truth,
        ms_per_frame: 0.0,
    };
    (run, moved, tr)
}

#[test]
fn finds_the_arrow_when_the_rectangle_is_not_centred_on_it() {
    // The default UI at 1440p and at 1080p, the arrow off by up to ~10 px
    // (as seen on a player's screen), and an exactly centred one.
    for (cfg, off) in [
        ((311, 206, 1.0, 7), (9.0, -6.0)),
        ((234, 155, 0.75, 3), (-7.0, 5.0)),
        ((311, 206, 1.0, 5), (0.0, 0.0)),
    ] {
        let (w, h) = (cfg.0 as f64, cfg.1 as f64);
        let pivot = (w / 2.0 + off.0, h / 2.0 + off.1);
        let (r, moved, tr) = run_off_centre(cfg, pivot, 50, path);
        let at = moved.unwrap_or_else(|| panic!("{cfg:?}: arrow not found"));
        let p = tr.pivot();
        let err = (p.0 - pivot.0).hypot(p.1 - pivot.1);
        eprintln!(
            "{cfg:?}: arrow at {pivot:?} found at {p:?} (off {err:.2} px) after {:.1} s",
            at as f64 / 10.0
        );
        assert!(err <= 1.5, "{cfg:?}: found {p:?}, arrow at {pivot:?}");
        assert!(at <= 300, "{cfg:?}: found only after {at} frames");
        // Moving the pivot does not disturb the tracking.
        assert_eq!(
            r.steps.iter().filter(|s| s.brk).count(),
            0,
            "{cfg:?}: breaks"
        );
        let (x0, y0) = (r.steps[0].x, r.steps[0].y);
        let (tx0, ty0) = r.truth[0];
        let worst = r
            .steps
            .iter()
            .zip(&r.truth)
            .map(|(s, (tx, ty))| ((s.x - x0) - (tx - tx0)).hypot((s.y - y0) - (ty - ty0)))
            .fold(0.0, f64::max);
        assert!(worst < 6.0, "{cfg:?}: worst error {worst:.2} px");
    }
}

#[test]
fn standing_still_never_moves_the_pivot() {
    let cfg = (311, 206, 1.0, 7);
    let (_, moved, tr) = run_off_centre(cfg, (164.5, 97.0), 30, |_| (700.0, 600.0));
    assert_eq!(moved, None);
    assert!(!tr.pivot_found());
    assert_eq!(tr.pivot(), (155.5, 103.0));
}
