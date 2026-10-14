//! Estimating the own position from objects entering and leaving the
//! server's send range.
//!
//! The server reports our position only with events (buffs, gliding, being
//! hit, loading screens). It does, however, add objects (`34 36`) as they come
//! within a fixed distance of us and remove them (`35 36` reason 01) as they
//! fall behind, and it tells us where each object is. aion2-cube-radar
//! measured those distances: about 100 m for cubes, about 50 m enter / 65 m
//! leave for many other objects, a fixed value per template (FINDINGS B8).
//! So every such event says "you are about R from this point right now".
//!
//! This module turns that into a position:
//!
//! * **Radii are learned, not assumed.** For each template and edge (enter /
//!   leave) the distance to a server-confirmed own position at the same
//!   moment is collected; the median is used once three samples agree.
//! * **A small Kalman filter** (position and velocity) takes the range
//!   events and the server's own-position fixes. Outliers are gated.
//! * **It checks itself.** Every server fix is compared with what the filter
//!   predicted just before. If the recent median error is large, estimates
//!   are not shown at all; the status reports the measured error either way.
//!
//! Nothing here is shown as a report: estimates carry their uncertainty, and
//! the radar draws it.

use std::collections::{HashMap, VecDeque};

use radar_core::Millis;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Enter,
    Leave,
}

/// Known send radii of one template, game units (cm).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiusSeed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enter: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leave: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EstimateStatus {
    /// Templates with a usable enter or leave radius.
    pub radii_known: usize,
    /// Range events used for the position so far.
    pub range_updates: u64,
    /// Range events rejected as inconsistent.
    pub range_rejected: u64,
    /// Median distance between the estimate and the next server fix, metres
    /// (last few checks). `None` until checked.
    pub check_error_m: Option<f64>,
    pub checks: usize,
    /// Whether estimates are being shown.
    pub trusted: bool,
}

const MAX_SAMPLES: usize = 25;
const MIN_SAMPLES: usize = 3;
/// A truth this close in time to an event is used directly.
const NEAR_MS: Millis = 400;
/// Two truths at most this far apart bracket an event for interpolation.
const BRACKET_MS: Millis = 3_000;
/// Events this long after a load or teleport are not edges.
const QUIET_MS: Millis = 6_000;
/// Distances beyond this are not a send radius (teleports, bulk removals).
const MAX_RADIUS: f32 = 30_000.0;
/// Spawns created this recently (server clock) appeared, they did not enter.
const FRESH_MS: i64 = 5_000;
/// Emit estimates only while 1-sigma is below this (units).
const MAX_SIGMA: f64 = 2_500.0;
/// Estimates are not shown when recent checks are worse than this (units).
const TRUST_ERROR: f64 = 2_500.0;
/// Velocity random walk, units/s per sqrt(s).
const ACCEL_NOISE: f64 = 500.0;
/// 1-sigma of the unknown velocity at a fix, units/s (running ≈ 600–800,
/// mounted or gliding faster).
const INIT_SPEED: f64 = 1_000.0;
/// Velocity fades with this time constant (players stop and turn).
const VEL_TAU_S: f64 = 6.0;

#[derive(Debug, Default)]
struct Radius {
    samples: VecDeque<f32>,
    seed: Option<f32>,
}

impl Radius {
    /// (radius, 1-sigma) when known.
    fn value(&self) -> Option<(f64, f64)> {
        if self.samples.len() >= MIN_SAMPLES {
            let mut v: Vec<f32> = self.samples.iter().copied().collect();
            v.sort_by(f32::total_cmp);
            let med = v[v.len() / 2];
            let mut dev: Vec<f32> = v.iter().map(|x| (x - med).abs()).collect();
            dev.sort_by(f32::total_cmp);
            let mad = dev[dev.len() / 2] as f64 * 1.48;
            if mad > 1_500.0 {
                return None; // not one fixed radius
            }
            return Some((med as f64, mad.max(200.0)));
        }
        self.seed.map(|s| (s as f64, 700.0))
    }
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    t: Millis,
    q: [f32; 3],
    template: u32,
    edge: Edge,
    learned: bool,
}

/// Position-velocity Kalman filter in the horizontal plane.
#[derive(Debug, Clone, Copy)]
struct Filter {
    x: [f64; 4],
    p: [[f64; 4]; 4],
    t: Millis,
    z: f32,
}

// Small fixed-size matrix arithmetic reads best with indices.
#[allow(clippy::needless_range_loop)]
impl Filter {
    fn at(p: [f32; 3], sigma: f64, t: Millis) -> Filter {
        let mut f = Filter {
            x: [p[0] as f64, p[1] as f64, 0.0, 0.0],
            p: [[0.0; 4]; 4],
            t,
            z: p[2],
        };
        f.p[0][0] = sigma * sigma;
        f.p[1][1] = sigma * sigma;
        f.p[2][2] = INIT_SPEED * INIT_SPEED;
        f.p[3][3] = INIT_SPEED * INIT_SPEED;
        f
    }

    fn predict(&mut self, t: Millis) {
        let dt = (t - self.t).max(0) as f64 / 1000.0;
        if dt == 0.0 {
            return;
        }
        self.t = t;
        let k = (-dt / VEL_TAU_S).exp();
        // Integral of the fading velocity over dt.
        let g = VEL_TAU_S * (1.0 - k);
        let f = [
            [1.0, 0.0, g, 0.0],
            [0.0, 1.0, 0.0, g],
            [0.0, 0.0, k, 0.0],
            [0.0, 0.0, 0.0, k],
        ];
        let x = self.x;
        for (i, row) in f.iter().enumerate() {
            self.x[i] = (0..4).map(|j| row[j] * x[j]).sum();
        }
        let mut fp = [[0.0; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                fp[i][j] = (0..4).map(|m| f[i][m] * self.p[m][j]).sum();
            }
        }
        for i in 0..4 {
            for j in 0..4 {
                self.p[i][j] = (0..4).map(|m| fp[i][m] * f[j][m]).sum();
            }
        }
        let q = ACCEL_NOISE * ACCEL_NOISE * dt;
        self.p[2][2] += q;
        self.p[3][3] += q;
        self.p[0][0] += q * dt * dt / 3.0;
        self.p[1][1] += q * dt * dt / 3.0;
    }

    /// Scalar update with measurement row `h`, innovation `y`, variance `r`.
    /// Returns false (and changes nothing) when gated out.
    fn update(&mut self, h: [f64; 4], y: f64, r: f64, gate: f64) -> bool {
        let ph: Vec<f64> = (0..4)
            .map(|i| (0..4).map(|j| self.p[i][j] * h[j]).sum())
            .collect();
        let s: f64 = (0..4).map(|i| h[i] * ph[i]).sum::<f64>() + r;
        if s.is_nan() || s <= 0.0 || y * y > gate * gate * s {
            return false;
        }
        let k: Vec<f64> = ph.iter().map(|v| v / s).collect();
        for i in 0..4 {
            self.x[i] += k[i] * y;
        }
        let p = self.p;
        for i in 0..4 {
            for j in 0..4 {
                self.p[i][j] = p[i][j] - k[i] * ph[j];
            }
        }
        true
    }

    fn fix(&mut self, p: [f32; 3], sigma: f64) {
        let r = sigma * sigma;
        let x = self.x;
        // Position updates are never gated: the server is the reference.
        self.update([1.0, 0.0, 0.0, 0.0], p[0] as f64 - x[0], r, f64::INFINITY);
        let x = self.x;
        self.update([0.0, 1.0, 0.0, 0.0], p[1] as f64 - x[1], r, f64::INFINITY);
        self.z = p[2];
    }

    /// "We are `radius` from `q`".
    fn range(&mut self, q: [f32; 3], radius: f64, sigma: f64) -> bool {
        let dx = self.x[0] - q[0] as f64;
        let dy = self.x[1] - q[1] as f64;
        let d = dx.hypot(dy);
        if d < 1.0 {
            return false;
        }
        let h = [dx / d, dy / d, 0.0, 0.0];
        self.update(h, radius - d, sigma * sigma, 3.0)
    }

    fn sigma(&self) -> f64 {
        ((self.p[0][0] + self.p[1][1]) / 2.0).max(0.0).sqrt()
    }

    fn pos(&self) -> [f32; 3] {
        [self.x[0] as f32, self.x[1] as f32, self.z]
    }

    fn velocity(&self) -> (f64, f64) {
        (self.x[2], self.x[3])
    }
}

/// The own position at `t` from nearby truths: the closest one within
/// `NEAR_MS`, or linear between two that bracket `t` closely enough.
fn truth_at(truths: &VecDeque<([f32; 3], Millis)>, t: Millis) -> Option<[f32; 3]> {
    if let Some((p, _)) = truths
        .iter()
        .filter(|(_, ft)| (t - ft).abs() <= NEAR_MS)
        .min_by_key(|(_, ft)| (t - ft).abs())
    {
        return Some(*p);
    }
    let before = truths.iter().rev().find(|(_, ft)| *ft <= t)?;
    let after = truths.iter().find(|(_, ft)| *ft >= t)?;
    if after.1 - before.1 > BRACKET_MS || after.1 == before.1 {
        return None;
    }
    let a = (t - before.1) as f32 / (after.1 - before.1) as f32;
    let l = |i: usize| before.0[i] + (after.0[i] - before.0[i]) * a;
    Some([l(0), l(1), l(2)])
}

pub struct EdgeEstimator {
    radii: HashMap<(u32, Edge), Radius>,
    /// Objects in the send range now: we are within their leave radius.
    inside: HashMap<u64, (u32, [f32; 3])>,
    /// Objects that left recently: we are beyond their enter radius.
    outside: HashMap<u64, (u32, [f32; 3], Millis)>,
    truths: VecDeque<([f32; 3], Millis)>,
    pending: VecDeque<Pending>,
    filter: Option<Filter>,
    /// Range updates since the last truth (a check needs some).
    since_truth: u32,
    quiet_until: Millis,
    errors: VecDeque<f64>,
    range_updates: u64,
    range_rejected: u64,
}

impl EdgeEstimator {
    pub fn new(seeds: &std::collections::BTreeMap<String, RadiusSeed>) -> Self {
        let mut radii: HashMap<(u32, Edge), Radius> = HashMap::new();
        for (k, s) in seeds {
            let Ok(t) = k.trim().parse::<u32>() else {
                continue;
            };
            for (edge, v) in [(Edge::Enter, s.enter), (Edge::Leave, s.leave)] {
                if let Some(v) = v.filter(|v| v.is_finite() && *v > 0.0 && *v < MAX_RADIUS) {
                    radii.entry((t, edge)).or_default().seed = Some(v);
                }
            }
        }
        EdgeEstimator {
            radii,
            inside: HashMap::new(),
            outside: HashMap::new(),
            truths: VecDeque::new(),
            pending: VecDeque::new(),
            filter: None,
            since_truth: 0,
            quiet_until: 0,
            errors: VecDeque::new(),
            range_updates: 0,
            range_rejected: 0,
        }
    }

    /// Map load, teleport or new connection: the next object bursts are not
    /// range edges, and the old position no longer applies.
    pub fn quiet(&mut self, t: Millis) {
        self.quiet_until = t + QUIET_MS;
        self.pending.clear();
    }

    pub fn on_new_connection(&mut self, t: Millis) {
        self.quiet(t);
        self.inside.clear();
        self.outside.clear();
        self.truths.clear();
        self.filter = None;
        self.since_truth = 0;
    }

    /// A position known to be right: a server fix (`check` = compare it with
    /// the estimate first) or the client's own movement report.
    pub fn on_truth(&mut self, p: [f32; 3], t: Millis, sigma: f64, check: bool) {
        if let Some(f) = &mut self.filter {
            if check && self.since_truth >= 2 && t >= f.t {
                let mut g = *f;
                g.predict(t);
                let e = (g.x[0] - p[0] as f64).hypot(g.x[1] - p[1] as f64);
                self.errors.push_back(e);
                while self.errors.len() > 7 {
                    self.errors.pop_front();
                }
            }
            if t >= f.t {
                f.predict(t);
                f.fix(p, sigma);
            } else {
                *f = Filter::at(p, sigma, t);
            }
        } else {
            self.filter = Some(Filter::at(p, sigma, t));
        }
        self.since_truth = 0;
        self.truths.push_back((p, t));
        while self.truths.len() > 64 || self.truths.front().is_some_and(|(_, ft)| t - ft > 30_000) {
            self.truths.pop_front();
        }
        self.learn_pending(t);
    }

    fn learn_pending(&mut self, now: Millis) {
        let mut learned = Vec::new();
        for e in self.pending.iter_mut().filter(|e| !e.learned) {
            if let Some(p) = truth_at(&self.truths, e.t) {
                e.learned = true;
                let d = (p[0] - e.q[0]).hypot(p[1] - e.q[1]);
                if d < MAX_RADIUS {
                    learned.push(((e.template, e.edge), d));
                }
            }
        }
        for (key, d) in learned {
            let r = self.radii.entry(key).or_default();
            r.samples.push_back(d);
            while r.samples.len() > MAX_SAMPLES {
                r.samples.pop_front();
            }
        }
        while self
            .pending
            .front()
            .is_some_and(|e| now - e.t > BRACKET_MS + NEAR_MS)
        {
            self.pending.pop_front();
        }
    }

    /// An object entered or left the send range. `created_age_ms` is how long
    /// ago the server created it (spawns only), when known. Returns a new
    /// estimate `(position, sigma, velocity)` when one should be shown.
    pub fn on_edge(
        &mut self,
        id: u64,
        template: u32,
        q: [f32; 3],
        edge: Edge,
        created_age_ms: Option<i64>,
        t: Millis,
    ) -> Option<([f32; 3], f64, (f64, f64))> {
        match edge {
            Edge::Enter => {
                if self.inside.len() >= 4_000 {
                    self.inside.clear();
                }
                self.inside.insert(id, (template, q));
                self.outside.remove(&id);
            }
            Edge::Leave => {
                self.inside.remove(&id);
                if self.outside.len() >= 4_000 {
                    self.outside.retain(|_, (_, _, lt)| t - *lt < 120_000);
                }
                self.outside.insert(id, (template, q, t));
            }
        }
        if t < self.quiet_until || created_age_ms.is_some_and(|a| a < FRESH_MS) {
            return None;
        }
        self.pending.push_back(Pending {
            t,
            q,
            template,
            edge,
            learned: false,
        });
        while self.pending.len() > 256 {
            self.pending.pop_front();
        }
        self.learn_pending(t);
        if truth_at(&self.truths, t).is_some() {
            return None; // already known exactly
        }
        let (radius, rs) = self.radii.get(&(template, edge))?.value()?;
        let f = self.filter.as_mut()?;
        if t < f.t {
            return None;
        }
        f.predict(t);
        // Lag between the server's range check and the report.
        let sigma = rs + 300.0;
        if f.range(q, radius, sigma) {
            self.range_updates += 1;
            self.since_truth += 1;
        } else {
            self.range_rejected += 1;
            return None;
        }
        self.constrain();
        self.current()
    }

    /// The object was removed for another reason (opened, destroyed): it
    /// says nothing about where we are any more.
    pub fn forget(&mut self, id: u64) {
        self.inside.remove(&id);
        self.outside.remove(&id);
    }

    /// Objects still in range must be within their leave radius, objects
    /// that left must be beyond their enter radius. The worst violations
    /// pull the estimate back.
    fn constrain(&mut self) {
        let Some(f) = self.filter.as_mut() else {
            return;
        };
        let (x, y) = (f.x[0], f.x[1]);
        let mut v: Vec<(f64, [f32; 3], f64, f64)> = Vec::new();
        for (tpl, q) in self.inside.values() {
            let Some((r, rs)) = self.radii.get(&(*tpl, Edge::Leave)).and_then(Radius::value) else {
                continue;
            };
            let d = (x - q[0] as f64).hypot(y - q[1] as f64);
            if d > r + rs {
                v.push((d - r, *q, r, rs));
            }
        }
        for (tpl, q, _) in self.outside.values() {
            let Some((r, rs)) = self.radii.get(&(*tpl, Edge::Enter)).and_then(Radius::value) else {
                continue;
            };
            let d = (x - q[0] as f64).hypot(y - q[1] as f64);
            if d < r - rs {
                v.push((r - d, *q, r, rs));
            }
        }
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, q, r, rs) in v.into_iter().take(3) {
            // Gated loosely: a violated bound is real information, but an
            // object that moved or a wrong radius must not drag us far.
            let d = (f.x[0] - q[0] as f64).hypot(f.x[1] - q[1] as f64);
            if d < 1.0 {
                continue;
            }
            let h = [
                (f.x[0] - q[0] as f64) / d,
                (f.x[1] - q[1] as f64) / d,
                0.0,
                0.0,
            ];
            f.update(h, r - d, (rs + 300.0).powi(2), 4.0);
        }
    }

    /// The estimate moved forward to `t` (for smooth display between events).
    pub fn predict(&mut self, t: Millis) -> Option<([f32; 3], f64, (f64, f64))> {
        if self.since_truth == 0 {
            return None;
        }
        let f = self.filter.as_mut()?;
        if t <= f.t {
            return None;
        }
        f.predict(t);
        self.constrain();
        self.current()
    }

    fn current(&self) -> Option<([f32; 3], f64, (f64, f64))> {
        let f = self.filter.as_ref()?;
        let s = f.sigma();
        (self.trusted() && s <= MAX_SIGMA && self.since_truth > 0)
            .then(|| (f.pos(), s, f.velocity()))
    }

    fn check_error(&self) -> Option<f64> {
        if self.errors.is_empty() {
            return None;
        }
        let mut v: Vec<f64> = self.errors.iter().copied().collect();
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    }

    pub fn trusted(&self) -> bool {
        self.errors.len() < 3 || self.check_error().is_some_and(|e| e <= TRUST_ERROR)
    }

    /// Learned radii, for saving across sessions (template → seed).
    pub fn learned_radii(&self) -> std::collections::BTreeMap<String, RadiusSeed> {
        let mut out: std::collections::BTreeMap<String, RadiusSeed> = Default::default();
        for ((t, edge), r) in &self.radii {
            if r.samples.len() < MIN_SAMPLES {
                continue;
            }
            let Some((v, _)) = r.value() else { continue };
            let e = out.entry(t.to_string()).or_default();
            match edge {
                Edge::Enter => e.enter = Some(v as f32),
                Edge::Leave => e.leave = Some(v as f32),
            }
        }
        out
    }

    pub fn status(&self, units_per_meter: f64) -> EstimateStatus {
        let mut templates: Vec<u32> = self
            .radii
            .iter()
            .filter(|(_, r)| r.value().is_some())
            .map(|((t, _), _)| *t)
            .collect();
        templates.sort_unstable();
        templates.dedup();
        EstimateStatus {
            radii_known: templates.len(),
            range_updates: self.range_updates,
            range_rejected: self.range_rejected,
            check_error_m: self.check_error().map(|e| e / units_per_meter),
            checks: self.errors.len(),
            trusted: self.trusted(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A walk with turns and a 20 s stop, in game units.
    fn walk(t: Millis) -> [f32; 3] {
        let s = t as f32 / 1000.0;
        let s = if s < 70.0 {
            s
        } else if s < 90.0 {
            70.0
        } else {
            s - 20.0
        };
        let x = -20_000.0 + 450.0 * s;
        let y = 50_000.0 + 3_000.0 * (s / 15.0).sin() * 4.0;
        [x, y, 1_000.0]
    }

    /// Objects on a grid; template by cell parity, radii 5000 / 6500 and
    /// 10000 / 11900.
    fn world(spacing: f32) -> Vec<(u32, [f32; 3])> {
        let mut v = Vec::new();
        let mut n = 0u32;
        for i in -10..60 {
            for j in -20..20 {
                // Pseudo-random jitter so the layout is not a lattice.
                let jx = ((i * 7919 + j * 104_729) % 1_700) as f32;
                let jy = ((i * 15_485 + j * 32_452) % 1_900) as f32;
                let tpl = if n.is_multiple_of(3) {
                    1_500_000
                } else {
                    1_500_004
                };
                v.push((
                    tpl,
                    [
                        i as f32 * spacing + jx - 20_000.0,
                        j as f32 * spacing + jy + 50_000.0,
                        1_000.0,
                    ],
                ));
                n += 1;
            }
        }
        v
    }

    fn radius(tpl: u32, edge: Edge) -> f32 {
        match (tpl, edge) {
            (1_500_000, Edge::Enter) => 10_000.0,
            (1_500_000, Edge::Leave) => 11_900.0,
            (_, Edge::Enter) => 5_000.0,
            (_, Edge::Leave) => 6_500.0,
        }
    }

    /// Simulates the server's interest management at 4 Hz with the given
    /// own-fix interval, and returns the estimator plus the worst error of
    /// shown estimates after `warmup` ms.
    fn run(
        spacing: f32,
        fix_every: Millis,
        secs: i64,
        warmup: Millis,
        seeds: BTreeMap<String, RadiusSeed>,
    ) -> (EdgeEstimator, f64, usize) {
        let objs = world(spacing);
        let mut inside = vec![false; objs.len()];
        let mut est = EdgeEstimator::new(&seeds);
        let mut worst = 0.0f64;
        let mut shown = 0;
        // The objects around the start are a load burst, not edges.
        est.quiet_until = 1;
        for step in 0..(secs * 4) {
            let t = step * 250;
            let me = walk(t);
            if t % fix_every == 0 {
                est.on_truth(me, t, 150.0, true);
            }
            for (k, (tpl, q)) in objs.iter().enumerate() {
                let d = (me[0] - q[0]).hypot(me[1] - q[1]);
                let ev = if !inside[k] && d <= radius(*tpl, Edge::Enter) {
                    inside[k] = true;
                    Some(Edge::Enter)
                } else if inside[k] && d > radius(*tpl, Edge::Leave) {
                    inside[k] = false;
                    Some(Edge::Leave)
                } else {
                    None
                };
                if let Some(edge) = ev {
                    if let Some((p, _s, _v)) = est.on_edge(k as u64, *tpl, *q, edge, None, t) {
                        if t >= warmup {
                            let e = ((p[0] - me[0]).hypot(p[1] - me[1])) as f64;
                            worst = worst.max(e);
                            shown += 1;
                        }
                    }
                }
            }
            if t % 1_000 == 500 && t >= warmup {
                if let Some((p, _s, _v)) = est.predict(t) {
                    let e = ((p[0] - me[0]).hypot(p[1] - me[1])) as f64;
                    worst = worst.max(e);
                    shown += 1;
                }
            }
        }
        (est, worst, shown)
    }

    /// Learns radii with frequent fixes (combat buffs), then follows a walk
    /// with fixes only every 60 s (opening the wings now and then).
    /// Returns (worst error, median check error) in metres.
    fn learn_then_track(spacing: f32) -> (f64, f64) {
        let (est, _, _) = run(spacing, 1_000, 90, i64::MAX, BTreeMap::new());
        let radii = est.learned_radii();
        let r = &radii["1500004"];
        assert!((r.enter.unwrap() - 5_000.0).abs() < 400.0, "{r:?}");
        assert!((r.leave.unwrap() - 6_500.0).abs() < 400.0, "{r:?}");

        let (est, worst, shown) = run(spacing, 60_000, 180, 5_000, radii);
        let st = est.status(100.0);
        eprintln!(
            "spacing {spacing}: shown {shown}, worst {:.1} m, {st:?}",
            worst / 100.0
        );
        assert!(shown > 100, "{shown}");
        assert!(st.trusted);
        (worst / 100.0, st.check_error_m.unwrap())
    }

    #[test]
    fn radii_are_learned_and_a_walk_is_followed_in_a_dense_field() {
        let (worst, check) = learn_then_track(1_800.0);
        assert!(worst < 10.0 && check < 5.0, "{worst} {check}");
    }

    #[test]
    fn a_walk_is_followed_in_a_sparse_field() {
        let (worst, check) = learn_then_track(4_500.0);
        assert!(worst < 20.0 && check < 20.0, "{worst} {check}");
    }

    #[test]
    fn nothing_is_estimated_without_radii_or_without_an_anchor() {
        let mut est = EdgeEstimator::new(&BTreeMap::new());
        assert!(est
            .on_edge(7, 1, [0.0, 50_000.0, 0.0], Edge::Enter, None, 10_000)
            .is_none());
        let mut seeds = BTreeMap::new();
        seeds.insert(
            "1".to_string(),
            RadiusSeed {
                enter: Some(5_000.0),
                leave: None,
            },
        );
        let mut est = EdgeEstimator::new(&seeds);
        // A radius but no anchor yet.
        assert!(est
            .on_edge(7, 1, [0.0, 50_000.0, 0.0], Edge::Enter, None, 10_000)
            .is_none());
    }

    #[test]
    fn freshly_created_objects_and_load_bursts_are_not_edges() {
        let mut seeds = BTreeMap::new();
        seeds.insert(
            "1".to_string(),
            RadiusSeed {
                enter: Some(5_000.0),
                leave: None,
            },
        );
        let mut est = EdgeEstimator::new(&seeds);
        est.on_truth([0.0, 50_000.0, 0.0], 0, 150.0, true);
        assert!(est
            .on_edge(
                1,
                1,
                [4_000.0, 50_000.0, 0.0],
                Edge::Enter,
                Some(100),
                2_000
            )
            .is_none());
        est.quiet(3_000);
        assert!(est
            .on_edge(2, 1, [4_000.0, 50_000.0, 0.0], Edge::Enter, None, 5_000)
            .is_none());
        assert_eq!(est.status(100.0).range_updates, 0);
        // After the quiet period, with a recent anchor, it is an edge.
        est.on_truth([0.0, 50_000.0, 0.0], 8_000, 150.0, true);
        assert!(est
            .on_edge(3, 1, [4_000.0, 54_000.0, 0.0], Edge::Enter, None, 9_500)
            .is_some());
        assert_eq!(est.status(100.0).range_updates, 1);
    }

    #[test]
    fn a_wrong_radius_is_caught_by_the_checks_or_relearned() {
        // Seeds 40 % too large.
        let mut seeds = BTreeMap::new();
        for t in [1_500_000u32, 1_500_004] {
            seeds.insert(
                t.to_string(),
                RadiusSeed {
                    enter: Some(radius(t, Edge::Enter) * 1.4),
                    leave: Some(radius(t, Edge::Leave) * 1.4),
                },
            );
        }
        let (est, _, _) = run(1_800.0, 7_000, 200, 0, seeds);
        let st = est.status(100.0);
        eprintln!("{st:?}");
        assert!(st.checks >= 3);
        // Never trusted with a large error.
        assert!(!st.trusted || st.check_error_m.unwrap() < 25.0, "{st:?}");
    }
}
