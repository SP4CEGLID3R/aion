//! How high the ground is where the player stands, between server fixes.
//!
//! The minimap only says where we are on the map. The server, though, keeps
//! sending the positions of everything around us — monsters, NPCs,
//! gathering spots, other players — and those mostly stand on the ground.
//! Their heights near the player, fitted with a tilted plane that ignores
//! the odd flier or balcony, give the ground height under the player at
//! every minimap reading.
//!
//! Checked like every other estimate here: each own server fix (exact
//! height) is compared with what the model predicted there. The typical
//! miss becomes part of the stated uncertainty, and a fix well above the
//! modelled ground (opening the wings mid-air) means we are flying, so the
//! ground model is not used until the next fix.
//!
//! Units are game units (cm) throughout.

use std::collections::{HashMap, VecDeque};

use radar_core::Millis;

/// Samples within this distance (horizontal) of the query count.
const RADIUS: f64 = 2_500.0;
/// …or within this one when there are too few close by.
const RADIUS_WIDE: f64 = 4_000.0;
/// Fewer samples than this: no estimate.
const MIN_SAMPLES: usize = 4;
/// One sample per cell of this size (the latest), so one monster pacing
/// around does not outvote everything else.
const CELL: f64 = 200.0;
const MAX_CELLS: usize = 20_000;
/// Steeper than this (rise over run) is a cliff or two levels: no estimate.
const MAX_SLOPE: f64 = 1.2;
/// Entity heights scatter at least this much around the ground (origins).
const MIN_SPREAD: f64 = 30.0;
/// Until the model has been checked against fixes, assume it misses by this;
/// after one check, by half again that miss but at least this much.
const PRIOR_MISS: f64 = 300.0;
const ONE_CHECK_MISS: f64 = 150.0;
/// A fix this far above the modelled ground: we are in the air.
const AIRBORNE: f64 = 300.0;
const CHECKS_KEPT: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ground {
    /// Ground height under the query point.
    pub z: f64,
    /// 1-sigma uncertainty of `z`.
    pub sigma: f64,
    /// Samples used.
    pub samples: usize,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    p: [f64; 3],
    t: Millis,
}

#[derive(Debug, Default)]
pub struct GroundModel {
    cells: HashMap<(i64, i64), Sample>,
    /// Signed misses (fix height − predicted ground) at recent own fixes.
    checks: VecDeque<f64>,
    /// The last own fix was well above the ground: flying or gliding.
    airborne: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundStatus {
    pub samples: usize,
    pub checks: usize,
    /// Typical miss at own fixes, metres (once checked).
    pub miss_m: Option<f64>,
    pub airborne: bool,
}

impl GroundModel {
    pub fn new() -> GroundModel {
        GroundModel::default()
    }

    /// Everything is forgotten (another map).
    pub fn clear(&mut self) {
        self.cells.clear();
        self.checks.clear();
        self.airborne = false;
    }

    /// Something stands at `p` (not a cube, not us).
    pub fn add(&mut self, p: [f32; 3], t: Millis) {
        let p = [p[0] as f64, p[1] as f64, p[2] as f64];
        if !p.iter().all(|v| v.is_finite()) {
            return;
        }
        if self.cells.len() >= MAX_CELLS {
            // Drop the oldest half; rare, and keeps memory bounded.
            let mut ts: Vec<Millis> = self.cells.values().map(|s| s.t).collect();
            ts.sort_unstable();
            let cut = ts[ts.len() / 2];
            self.cells.retain(|_, s| s.t > cut);
        }
        let key = ((p[0] / CELL).floor() as i64, (p[1] / CELL).floor() as i64);
        self.cells.insert(key, Sample { p, t });
    }

    /// The ground under `q` from the samples alone, without the checks.
    pub fn fit(&self, q: [f32; 3]) -> Option<Ground> {
        let (qx, qy) = (q[0] as f64, q[1] as f64);
        let near = |r: f64| -> Vec<Sample> {
            self.cells
                .values()
                .filter(|s| (s.p[0] - qx).hypot(s.p[1] - qy) <= r)
                .copied()
                .collect()
        };
        let mut pts = near(RADIUS);
        if pts.len() < MIN_SAMPLES {
            pts = near(RADIUS_WIDE);
        }
        if pts.len() < MIN_SAMPLES {
            return None;
        }
        fit_plane(&pts, qx, qy)
    }

    /// The model's typical miss at own fixes (one check is weak evidence,
    /// none is none).
    fn miss(&self) -> f64 {
        match self.checks.len() {
            0 => PRIOR_MISS,
            1 => (1.5 * self.checks[0].abs()).max(ONE_CHECK_MISS),
            _ => median(self.checks.iter().map(|c| c.abs()).collect()),
        }
    }

    /// Entities' origins vs ours: the typical signed miss.
    fn bias(&self) -> f64 {
        if self.checks.len() >= 2 {
            let b = median(self.checks.iter().copied().collect());
            if b.abs() <= AIRBORNE {
                return b;
            }
        }
        0.0
    }

    /// An own server fix: exact height. Checks the model, and tells whether
    /// we are on the ground.
    pub fn on_fix(&mut self, p: [f32; 3]) {
        let Some(g) = self.fit(p) else {
            // Nothing to compare with; assume walking.
            self.airborne = false;
            return;
        };
        let miss = p[2] as f64 - g.z;
        self.airborne = miss - self.bias() > AIRBORNE;
        // Flying says nothing about the ground model (and while flying it
        // is not used); a fix below the modelled ground is a real miss.
        if !self.airborne {
            self.checks.push_back(miss);
            while self.checks.len() > CHECKS_KEPT {
                self.checks.pop_front();
            }
        }
    }

    /// Our height at `q` if we are on the ground there: the fitted ground
    /// plus the usual offset, with its 1-sigma uncertainty (fit spread and
    /// the misses seen at fixes). `None` while flying or without samples.
    pub fn own_height(&self, q: [f32; 3]) -> Option<(f64, f64)> {
        if self.airborne {
            return None;
        }
        let g = self.fit(q)?;
        Some((g.z + self.bias(), g.sigma.hypot(self.miss())))
    }

    pub fn status(&self) -> GroundStatus {
        GroundStatus {
            samples: self.cells.len(),
            checks: self.checks.len(),
            miss_m: (!self.checks.is_empty()).then(|| self.miss() / 100.0),
            airborne: self.airborne,
        }
    }
}

/// Robust, distance-weighted plane through the samples, evaluated at
/// (qx, qy): a median start, then a few Huber-reweighted least-squares
/// rounds. Level (no slope) when the samples do not spread out enough in
/// both directions to tell a slope; then the distance from them to the
/// query adds to the uncertainty.
fn fit_plane(pts: &[Sample], qx: f64, qy: f64) -> Option<Ground> {
    let n = pts.len();
    // Metres from the query point; heights stay in game units.
    let dx: Vec<f64> = pts.iter().map(|s| (s.p[0] - qx) / 100.0).collect();
    let dy: Vec<f64> = pts.iter().map(|s| (s.p[1] - qy) / 100.0).collect();
    let z: Vec<f64> = pts.iter().map(|s| s.p[2]).collect();
    // Closer samples count more (10 m away: half).
    let wd: Vec<f64> = (0..n)
        .map(|i| 1.0 / (1.0 + (dx[i] * dx[i] + dy[i] * dy[i]) / 100.0))
        .collect();
    let sw: f64 = wd.iter().sum();
    let (mx, my) = (
        (0..n).map(|i| wd[i] * dx[i]).sum::<f64>() / sw,
        (0..n).map(|i| wd[i] * dy[i]).sum::<f64>() / sw,
    );
    let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (a, b) = (dx[i] - mx, dy[i] - my);
        cxx += wd[i] * a * a / sw;
        cyy += wd[i] * b * b / sw;
        cxy += wd[i] * a * b / sw;
    }
    let narrow = (cxx + cyy) / 2.0 - (((cxx - cyy) / 2.0).powi(2) + cxy * cxy).sqrt();
    // A slope needs samples at least ~3 m apart across both directions.
    let sloped = narrow >= 9.0;

    let mut coef = [weighted_median(&z, &wd), 0.0, 0.0];
    let mut spread = MIN_SPREAD;
    for _ in 0..5 {
        let abs: Vec<f64> = (0..n)
            .map(|i| (z[i] - (coef[0] + coef[1] * dx[i] + coef[2] * dy[i])).abs())
            .collect();
        spread = (1.4826 * weighted_median(&abs, &wd)).max(MIN_SPREAD);
        let w: Vec<f64> = (0..n)
            .map(|i| wd[i] * (1.5 * spread / abs[i].max(1e-9)).min(1.0))
            .collect();
        coef = match sloped.then(|| solve_plane(&dx, &dy, &z, &w)).flatten() {
            Some(c) => c,
            None => [weighted_median(&z, &w), 0.0, 0.0],
        };
    }
    // Rise over run (heights in cm, run in m).
    if coef[1].hypot(coef[2]) / 100.0 > MAX_SLOPE {
        return None;
    }
    let sw2: f64 = wd.iter().map(|w| w * w).sum();
    let n_eff = (sw * sw / sw2).max(1.0);
    let mut sigma = spread * (1.0 + 1.0 / n_eff).sqrt();
    if !sloped {
        // An unseen slope of up to ~1 in 7 between the samples and here.
        sigma = sigma.hypot(15.0 * mx.hypot(my));
    }
    Some(Ground {
        z: coef[0],
        sigma,
        samples: n,
    })
}

/// Weighted least squares for z = a + b·dx + c·dy; None when the points
/// do not spread out in two directions.
fn solve_plane(dx: &[f64], dy: &[f64], z: &[f64], w: &[f64]) -> Option<[f64; 3]> {
    let mut m = [[0.0f64; 3]; 3];
    let mut v = [0.0f64; 3];
    for i in 0..z.len() {
        let row = [1.0, dx[i], dy[i]];
        for a in 0..3 {
            for b in 0..3 {
                m[a][b] += w[i] * row[a] * row[b];
            }
            v[a] += w[i] * row[a] * z[i];
        }
    }
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    if d.abs() < 1e-9 {
        return None;
    }
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let mut mk = m;
        for r in 0..3 {
            mk[r][k] = v[r];
        }
        *o = det(&mk) / d;
    }
    out.iter().all(|c| c.is_finite()).then_some(out)
}

fn weighted_median(v: &[f64], w: &[f64]) -> f64 {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&a, &b| v[a].total_cmp(&v[b]));
    let total: f64 = w.iter().sum();
    let mut acc = 0.0;
    for &i in &idx {
        acc += w[i];
        if acc >= total / 2.0 {
            return v[i];
        }
    }
    v[idx[idx.len() - 1]]
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hillside rising 1 m per 4 m to the north (+y), with things standing
    /// on it in a loose grid, a few of them flying 15 m up.
    fn hillside(m: &mut GroundModel, cx: f32, cy: f32) {
        let mut k = 0u32;
        for i in -5..=5 {
            for j in -5..=5 {
                k += 1;
                // Not quite a grid: a pseudo-random wobble.
                let wob = (k.wrapping_mul(2_654_435_761) % 997) as f32 / 997.0 - 0.5;
                let x = cx + i as f32 * 700.0 + wob * 300.0;
                let y = cy + j as f32 * 700.0 - wob * 250.0;
                let mut z = 1_000.0 + (y - cy) * 0.25 + wob * 40.0;
                if k.is_multiple_of(13) {
                    z += 1_500.0;
                }
                m.add([x, y, z], k as Millis);
            }
        }
    }

    #[test]
    fn finds_the_ground_on_a_slope_despite_fliers() {
        let mut m = GroundModel::new();
        hillside(&mut m, 0.0, 0.0);
        for (x, y) in [(0.0, 0.0), (1_200.0, -900.0), (-1_700.0, 2_000.0)] {
            let g = m.fit([x, y, 0.0]).expect("enough samples");
            let truth = 1_000.0 + y as f64 * 0.25;
            assert!(
                (g.z - truth).abs() < 60.0,
                "at ({x}, {y}): {} vs {truth}",
                g.z
            );
            assert!(g.sigma < 80.0, "{g:?}");
        }
    }

    #[test]
    fn says_nothing_without_samples_or_on_two_levels() {
        let mut m = GroundModel::new();
        assert!(m.fit([0.0, 0.0, 0.0]).is_none());
        m.add([0.0, 0.0, 0.0], 1);
        m.add([500.0, 0.0, 0.0], 1);
        assert!(m.fit([0.0, 0.0, 0.0]).is_none());
        // A bridge over a river: half the samples 8 m above the others.
        let mut two = GroundModel::new();
        for i in 0..40 {
            let x = (i % 8) as f32 * 400.0 - 1_400.0;
            let y = (i / 8) as f32 * 400.0 - 800.0;
            two.add([x, y, if i % 2 == 0 { 0.0 } else { 800.0 }], i);
        }
        let g = two.fit([0.0, 0.0, 0.0]).unwrap();
        assert!(g.sigma > 250.0, "two levels must look unsure: {g:?}");
    }

    #[test]
    fn checks_against_fixes_set_the_uncertainty_and_spot_flying() {
        let mut m = GroundModel::new();
        hillside(&mut m, 0.0, 0.0);
        // Before any check: the prior miss dominates.
        let (_, s0) = m.own_height([0.0, 0.0, 0.0]).unwrap();
        assert!(s0 >= PRIOR_MISS, "{s0}");
        // Two fixes standing on the slope (our origin 20 cm above theirs);
        // the first already makes it surer.
        m.on_fix([300.0, 400.0, 1_000.0 + 100.0 + 20.0]);
        let (_, s1) = m.own_height([0.0, 0.0, 0.0]).unwrap();
        assert!(s1 < 200.0, "{s1}");
        m.on_fix([-600.0, -800.0, 1_000.0 - 200.0 + 20.0]);
        let (z, s) = m.own_height([0.0, 1_200.0, 0.0]).unwrap();
        assert!((z - (1_000.0 + 300.0 + 20.0)).abs() < 60.0, "{z}");
        assert!(s < 100.0, "{s}");
        // Opening the wings 20 m above the slope: flying, no ground height.
        m.on_fix([0.0, 0.0, 1_000.0 + 2_000.0]);
        assert!(m.status().airborne);
        assert!(m.own_height([0.0, 0.0, 0.0]).is_none());
        // Landing and a buff on the ground: back on the ground.
        m.on_fix([0.0, 0.0, 1_000.0 + 20.0]);
        assert!(!m.status().airborne);
        assert!(m.own_height([0.0, 0.0, 0.0]).is_some());
        m.clear();
        assert_eq!(m.status(), GroundStatus::default());
    }
}
