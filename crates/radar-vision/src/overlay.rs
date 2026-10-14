//! What on the minimap does NOT move with the map, and how to find it.
//!
//! The AION 2 minimap is north-up and centred on the player: the map scrolls
//! under a fixed arrow. Fixed to the screen are the arrow in its light disk,
//! the view cone (a light wedge with two bright edge lines that turns with
//! the camera), a cyan range ring, the search icon (top right) and the clock
//! (bottom right). White and red dots (players, NPCs) move on their own.
//! Layout facts and the constants below follow madara134/aion2-cube-finder
//! (MIT), measured there on real minimap captures.

use crate::plane::{Bgra, Mask, Plane};

/// Masked disk around the pivot (arrow + light disk), px at UI scale 1.
pub const DISK_R: f64 = 28.0;
/// View cone half-angle: its edges are 113–114° apart.
pub const CONE_HALF_DEG: f64 = 57.0;
/// Masked half-width (degrees) of each bright cone edge line.
pub const CONE_EDGE_DEG: f64 = 5.0;
/// Masked half-width (px) of the cyan range ring.
pub const RING_HALF_W: f64 = 4.0;
/// Fixed HUD icons, as fractions of the map rectangle (x0, y0, x1, y1).
pub const HUD_BOXES: [(f64, f64, f64, f64); 2] = [(0.86, 0.0, 1.0, 0.24), (0.70, 0.84, 1.0, 1.0)];
/// Border pixels left out.
pub const EDGE: usize = 3;
/// min(R,G,B) at or above = white icon / party dot.
pub const ICON_MIN: u8 = 205;
/// More than this share of icon pixels is bright terrain, not icons.
pub const ICON_MAX_FRACTION: f64 = 0.25;
/// Cone edge strength that counts as a clear cone.
pub const CONE_CLEAR: f64 = 0.3;

/// Smallest angle between two compass directions (degrees, 0..180).
pub fn circ_diff(a: f64, b: f64) -> f64 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

/// Compass angle (0 = up, 90 = right) of a screen offset.
pub fn screen_bearing(dx: f64, dy: f64) -> f64 {
    dx.atan2(-dy).to_degrees().rem_euclid(360.0)
}

/// Pixels that move with the map: not the arrow disk, the range ring, the
/// HUD icons or the border; with `cone`, also not the two cone edge lines
/// (`cone_edge` degrees either side). The cone's interior is only a tint,
/// which the band-pass removes, so it stays usable.
#[allow(clippy::too_many_arguments)]
pub fn overlay_mask(
    w: usize,
    h: usize,
    pivot: (f64, f64),
    cone: Option<f64>,
    ring_r: Option<f64>,
    hud: bool,
    disk_r: f64,
    edge: usize,
    cone_edge: f64,
) -> Mask {
    let mut m = Mask::full(w, h);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f64 - pivot.0, y as f64 - pivot.1);
            let r = dx.hypot(dy);
            let mut ok = r > disk_r;
            if let Some(c) = cone {
                let d = circ_diff(screen_bearing(dx, dy), c);
                ok &= (d - CONE_HALF_DEG).abs() > cone_edge;
            }
            if let Some(rr) = ring_r {
                ok &= (r - rr).abs() > RING_HALF_W;
            }
            m.data[y * w + x] = ok;
        }
    }
    if hud {
        for (x0, y0, x1, y1) in HUD_BOXES {
            let (ya, yb) = (
                (y0 * h as f64) as usize,
                ((y1 * h as f64).ceil() as usize).min(h),
            );
            let (xa, xb) = (
                (x0 * w as f64) as usize,
                ((x1 * w as f64).ceil() as usize).min(w),
            );
            for y in ya..yb {
                for x in xa..xb {
                    m.data[y * w + x] = false;
                }
            }
        }
    }
    for e in 0..edge.min(h / 2) {
        for x in 0..w {
            m.data[e * w + x] = false;
            m.data[(h - 1 - e) * w + x] = false;
        }
    }
    for e in 0..edge.min(w / 2) {
        for y in 0..h {
            m.data[y * w + e] = false;
            m.data[y * w + w - 1 - e] = false;
        }
    }
    m
}

/// White icons and party dots, and red markers, sampled every `ds` pixels:
/// they can move on their own. Empty when "icons" cover too much (bright
/// terrain).
pub fn icon_mask(frame: &Bgra, ds: usize, w: usize, h: usize) -> Mask {
    let mut m = Mask {
        w,
        h,
        data: vec![false; w * h],
    };
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = frame.rgb((x * ds).min(frame.w - 1), (y * ds).min(frame.h - 1));
            let white = r.min(g).min(b) >= ICON_MIN;
            let red = r > 150 && g < 90 && b < 90;
            m.data[y * w + x] = white || red;
        }
    }
    let m = m.dilate(2);
    if m.fraction() > ICON_MAX_FRACTION {
        return Mask {
            w,
            h,
            data: vec![false; w * h],
        };
    }
    m
}

/// (centre, strength) of the view cone from its two bright edge lines: the
/// angular profile of thin bright ridges between radii `r0` and `r1`, best
/// pair of peaks 2·CONE_HALF_DEG (±6°) apart. Strength is the mean ridge
/// fraction of the two edges (0..1; ≥ CONE_CLEAR is a clear cone). The cone
/// is the camera direction, not the character's facing.
pub fn cone_angle(gray: &Plane, center: (f64, f64), r0: f64, r1: f64) -> (f64, f64) {
    const LOCAL: isize = 6;
    let radii: Vec<f64> = (0..)
        .map(|i| r0 + i as f64)
        .take_while(|r| *r < r1)
        .collect();
    let n = radii.len();
    if n == 0 {
        return (0.0, 0.0);
    }
    // v[a][r]: brightness along each ray (NaN outside the image).
    let mut v = vec![f64::NAN; 360 * n];
    for a in 0..360 {
        let (s, c) = (a as f64).to_radians().sin_cos();
        for (j, r) in radii.iter().enumerate() {
            if let Some(p) = gray.sample(center.0 + s * r, center.1 - c * r) {
                v[a * n + j] = p;
            }
        }
    }
    let mut ridge = vec![0.0f64; 360];
    let mut window = Vec::with_capacity((2 * LOCAL + 1) as usize);
    for a in 0..360isize {
        let mut hits = 0usize;
        for j in 0..n {
            let x = v[a as usize * n + j];
            if x.is_nan() {
                continue;
            }
            window.clear();
            for k in -LOCAL..=LOCAL {
                let y = v[(a + k).rem_euclid(360) as usize * n + j];
                if !y.is_nan() {
                    window.push(y);
                }
            }
            window.sort_by(f64::total_cmp);
            let local = window[window.len() / 2];
            if x - local > 25.0 {
                hits += 1;
            }
        }
        // Rays leaving the image count their outside part as "no ridge",
        // as the thresholds were tuned that way.
        ridge[a as usize] = hits as f64 / n as f64;
    }
    let widened: Vec<f64> = (0..360)
        .map(|a| {
            ridge[a]
                .max(ridge[(a + 1) % 360])
                .max(ridge[(a + 359) % 360])
        })
        .collect();
    let span = (2.0 * CONE_HALF_DEG).round() as usize;
    let mut best = (-1.0f64, 0.0f64);
    for s in span - 6..=span + 6 {
        for a in 0..360 {
            let pair = 0.5 * (widened[a] + widened[(a + s) % 360]);
            if pair > best.0 {
                best = (pair, (a as f64 + s as f64 / 2.0).rem_euclid(360.0));
            }
        }
    }
    (best.1, best.0.max(0.0))
}

/// Cyan-ness of each pixel: (G − R) + (B − R), clipped at 0.
pub fn cyan(frame: &Bgra) -> Plane {
    let mut p = Plane::new(frame.w, frame.h);
    for y in 0..frame.h {
        for x in 0..frame.w {
            let (r, g, b) = frame.rgb(x, y);
            p.data[y * frame.w + x] = ((g as f64 - r as f64) + (b as f64 - r as f64)).max(0.0);
        }
    }
    p
}

/// Radius of the cyan range ring around the pivot from a few cyan planes
/// (per-pixel median), or `None` when no ring stands out.
pub fn ring_radius(planes: &[Plane], pivot: (f64, f64), r_min: f64) -> Option<f64> {
    let first = planes.first()?;
    let (w, h) = (first.w, first.h);
    let mut med = Plane::new(w, h);
    let mut vals = Vec::with_capacity(planes.len());
    for i in 0..w * h {
        vals.clear();
        vals.extend(planes.iter().map(|p| p.data[i]));
        vals.sort_by(f64::total_cmp);
        med.data[i] = vals[vals.len() / 2];
    }
    let r_max = 0.5 * w.min(h) as f64;
    let mut prof = Vec::new();
    let mut r = r_min;
    while r < r_max {
        let mut s = 0.0;
        let mut n = 0.0;
        for k in 0..180 {
            let (sn, cs) = ((k as f64 * 2.0).to_radians()).sin_cos();
            if let Some(v) = med.sample(pivot.0 + sn * r, pivot.1 - cs * r) {
                s += v;
                n += 1.0;
            }
        }
        prof.push((r, if n > 0.0 { s / n } else { 0.0 }));
        r += 0.5;
    }
    if prof.is_empty() {
        return None;
    }
    let (best_r, best_v) = prof.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1))?;
    let mut sorted: Vec<f64> = prof.iter().map(|p| p.1).collect();
    sorted.sort_by(f64::total_cmp);
    let base = sorted[sorted.len() / 2];
    (best_v - base >= 8.0).then_some(best_r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_parts() {
        let (w, h) = (311, 206);
        let pivot = (155.5, 103.0);
        let m = overlay_mask(
            w,
            h,
            pivot,
            Some(0.0),
            Some(67.0),
            true,
            DISK_R,
            EDGE,
            CONE_EDGE_DEG,
        );
        assert!(!m.at(155, 103) && !m.at(155, 103 - 20)); // arrow disk
                                                          // A point on the cone's right edge line (57° from up).
        let a = 57f64.to_radians();
        let (ex, ey) = (pivot.0 + a.sin() * 50.0, pivot.1 - a.cos() * 50.0);
        assert!(!m.at(ex.round() as usize, ey.round() as usize));
        assert!(m.at(155, 103 - 50)); // cone interior is usable
        assert!(!m.at(155 + 67, 103)); // range ring
        assert!(!m.at(0, 0) && !m.at(w - 1, h - 1)); // border
        assert!(m.at(40, 180)); // plain map
                                // HUD boxes.
        for (x0, y0, x1, y1) in HUD_BOXES {
            let x = ((x0 + x1) / 2.0 * w as f64) as usize;
            let y = ((y0 + y1) / 2.0 * h as f64) as usize;
            assert!(!m.at(x, y));
        }
    }

    #[test]
    fn bearings() {
        assert_eq!(screen_bearing(0.0, -1.0), 0.0);
        assert!((screen_bearing(1.0, 0.0) - 90.0).abs() < 1e-9);
        assert!((screen_bearing(0.0, 1.0) - 180.0).abs() < 1e-9);
        assert!((circ_diff(350.0, 10.0) - 20.0).abs() < 1e-9);
    }
}
