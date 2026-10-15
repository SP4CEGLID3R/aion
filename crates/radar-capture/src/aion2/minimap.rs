//! Own position between server fixes from the minimap's movement.
//!
//! The screen tracker reports how far the minimap has scrolled (minimap
//! pixels). Here that becomes a world position: snap to every server fix,
//! and in between add the minimap displacement since that fix, converted
//! with the minimap's scale and axis mapping.
//!
//! The scale is seeded (≈ 1.04 px/m at the default UI on 2560×1440) and then
//! learned online from fix-to-fix "legs": the world distance between two
//! server fixes against the minimap distance over the same time. Zoom changes
//! show as legs that disagree (or a changed range ring) and trigger a re-fit.
//! The axis mapping (which screen direction is world +X/+Y) is seeded with
//! "right = +X, down = +Y" and re-fitted from leg directions. The design,
//! including its constants, follows madara134/aion2-cube-finder's
//! minimap fusion (MIT).

use std::collections::{HashMap, VecDeque};

use radar_core::Millis;
use serde::{Deserialize, Serialize};

use crate::minimap::MinimapSample;

/// Minimap px per game unit at UI scale 1 (2560×1440 default UI).
pub const SEED_K: f64 = 0.0103;
const LEG_MIN_M: f64 = 10.0;
const LEG_MAX_M: f64 = 300.0;
const LEG_MAX_MS: Millis = 60_000;
const LEG_MIN_PX: f64 = 3.0;
const N_LEGS: usize = 9;
/// A leg off by more than this suggests a zoom change.
const JUMP: f64 = 0.25;
/// Two re-fit legs must agree this well.
const AGREE: f64 = 0.15;
const RING_JUMP: f64 = 0.08;
/// Plausible scale, × the seed.
const K_RANGE: (f64, f64) = (0.25, 4.0);
const DIR_TOL_DEG: f64 = 35.0;
const STALE_FRAME_MS: Millis = 1_500;
const TRACK_KEEP: usize = 3_000;
/// A track break shorter than this keeps the position chain.
const BRIDGE_MS: Millis = 1_000;
/// Segment chains are pruned past this many, keeping those seen within this
/// long (well beyond the tracker's 2 minutes for picking a segment up again).
const SEG_CHAINS_KEPT: usize = 256;
const SEG_CHAIN_KEEP_MS: Millis = 300_000;
/// Cone strength that counts as a clear camera direction.
const CONE_CLEAR: f64 = 0.3;
/// A plain view (one road, coast) for this long makes the position unsure.
const RIDGE_UNSURE_MS: Millis = 3_000;

/// The 8 signed axis permutations: world (x, y) → screen (right, down).
pub const AXIS_MAPS: [(&str, [[f64; 2]; 2]); 8] = [
    ("right=+x, down=+y", [[1.0, 0.0], [0.0, 1.0]]),
    ("right=+x, down=-y", [[1.0, 0.0], [0.0, -1.0]]),
    ("right=-x, down=+y", [[-1.0, 0.0], [0.0, 1.0]]),
    ("right=-x, down=-y", [[-1.0, 0.0], [0.0, -1.0]]),
    ("right=+y, down=+x", [[0.0, 1.0], [1.0, 0.0]]),
    ("right=+y, down=-x", [[0.0, 1.0], [-1.0, 0.0]]),
    ("right=-y, down=+x", [[0.0, -1.0], [1.0, 0.0]]),
    ("right=-y, down=-x", [[0.0, -1.0], [-1.0, 0.0]]),
];

fn axis_index(name: &str) -> Option<usize> {
    AXIS_MAPS.iter().position(|(n, _)| *n == name)
}

/// Calibration kept between sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinimapCalibration {
    /// Minimap px per game unit (full-resolution px).
    pub k: f64,
    pub axis: String,
    #[serde(default)]
    pub ring_r: Option<f64>,
    #[serde(default)]
    pub legs: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinimapStatus {
    /// "off", "no frames", "no fix", "tracking", "uncertain", "recalibrating".
    pub state: String,
    pub reason: String,
    pub px_per_m: Option<f64>,
    pub axis: String,
    /// Legs that have confirmed the scale (0 = seed only).
    pub legs: u32,
    pub axis_confirmed: bool,
    pub frames: u64,
    pub breaks: u64,
    pub last_frame_age_ms: Option<Millis>,
    /// Camera direction from the view cone (screen compass degrees).
    pub camera_deg: Option<f64>,
    pub black: bool,
    pub frozen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LocKind {
    /// A position: fresh fix + minimap movement.
    Live,
    /// No reliable position from the minimap right now.
    Uncertain,
    /// The scale is being re-fitted (zoom change?).
    Recal,
    /// No server fix yet.
    NoFix,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    pub kind: LocKind,
    /// Game units (cm).
    pub pos: Option<[f32; 3]>,
    /// 1-sigma, game units.
    pub sigma: f64,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct TrackPt {
    t: Millis,
    x: f64,
    y: f64,
    chain: u64,
    seg: i64,
    /// Continues the previous point's coordinates: the same segment, or a
    /// fresh one bridged on from it (a resumed segment's coordinates jump).
    cont: bool,
}

/// A point on the minimap track: comparable with others of the same chain.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TrackRef {
    x: f64,
    y: f64,
    chain: u64,
    /// The tracker segment it was measured in.
    seg: i64,
}

#[derive(Debug, Clone, Copy)]
struct Anchor {
    t: Millis,
    pos: [f32; 3],
    zone: bool,
    /// Track position at the fix, if the track covered it.
    p: Option<TrackRef>,
    /// Track breaks before this fix (for uncertainty).
    breaks: u64,
    /// Plain-view time before this fix (for uncertainty).
    ridge_ms: Millis,
}

#[derive(Debug, Clone, Copy)]
struct Leg {
    w: (f64, f64),
    d: (f64, f64),
    k: f64,
}

#[derive(Debug, Clone, PartialEq)]
enum Recal {
    /// Legs disagree with the scale: candidates so far.
    Legs(Vec<f64>),
    /// The range ring changed size: waiting for legs to confirm.
    Ring(Vec<f64>),
}

pub struct MinimapFusion {
    k: f64,
    k_seed: f64,
    seeded: bool,
    axis: usize,
    axis_ok: bool,
    legs_total: u32,
    legs: VecDeque<Leg>,
    recal: Option<Recal>,
    k_before: f64,
    ring_base: Option<f64>,
    ring_now: Option<f64>,
    ring_odd: u32,
    track: VecDeque<TrackPt>,
    chain: u64,
    next_chain: u64,
    /// The chain each recent tracker segment belongs to, and when it was
    /// last seen: a segment the tracker picks up again (after the minimap
    /// was covered) returns to its chain.
    seg_chain: HashMap<i64, (u64, Millis)>,
    last_seg: Option<i64>,
    last_good_t: Option<Millis>,
    breaks: u64,
    pending: Vec<Anchor>,
    anchor: Option<Anchor>,
    leg_start: Option<Anchor>,
    map_still: bool,
    last_frame_t: Option<Millis>,
    black: bool,
    frozen: bool,
    cone: Option<(f64, f64, Millis)>,
    frames: u64,
    dirty: bool,
    units_per_m: f64,
    /// Time the view was a plain ridge (elongated peaks count in part:
    /// see `on_frame`), in total and in the current run.
    ridge_ms: Millis,
    ridge_run: Millis,
    last_frame_dt: Millis,
}

/// Angle (degrees) between a leg's minimap displacement and its world
/// displacement mapped by `m`.
fn leg_dir_err(leg: &Leg, m: &[[f64; 2]; 2]) -> f64 {
    let p = (
        m[0][0] * leg.w.0 + m[0][1] * leg.w.1,
        m[1][0] * leg.w.0 + m[1][1] * leg.w.1,
    );
    let cross = p.0 * leg.d.1 - p.1 * leg.d.0;
    let dot = p.0 * leg.d.0 + p.1 * leg.d.1;
    cross.atan2(dot).to_degrees().abs()
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

impl MinimapFusion {
    pub fn new(units_per_m: f64, saved: Option<&MinimapCalibration>) -> MinimapFusion {
        let mut f = MinimapFusion {
            k: SEED_K,
            k_seed: SEED_K,
            seeded: false,
            axis: 0,
            axis_ok: false,
            legs_total: 0,
            legs: VecDeque::new(),
            recal: None,
            k_before: SEED_K,
            ring_base: None,
            ring_now: None,
            ring_odd: 0,
            track: VecDeque::new(),
            chain: 0,
            next_chain: 1,
            seg_chain: HashMap::new(),
            last_seg: None,
            last_good_t: None,
            breaks: 0,
            pending: Vec::new(),
            anchor: None,
            leg_start: None,
            map_still: false,
            last_frame_t: None,
            black: false,
            frozen: false,
            cone: None,
            frames: 0,
            dirty: false,
            units_per_m,
            ridge_ms: 0,
            ridge_run: 0,
            last_frame_dt: 0,
        };
        if let Some(c) = saved.filter(|c| c.k.is_finite() && c.k > 0.0) {
            f.k = c.k;
            f.seeded = true;
            f.legs_total = c.legs;
            if let Some(i) = axis_index(&c.axis) {
                f.axis = i;
                f.axis_ok = c.legs >= 4;
            }
            f.ring_base = c.ring_r;
        }
        f
    }

    fn m(&self) -> &[[f64; 2]; 2] {
        &AXIS_MAPS[self.axis].1
    }

    /// Screen pixels → game units, for a displacement.
    fn to_world(&self, dx: f64, dy: f64) -> (f64, f64) {
        let m = self.m();
        // m maps world → screen and is a signed permutation: inverse = transpose.
        (
            (m[0][0] * dx + m[1][0] * dy) / self.k,
            (m[0][1] * dx + m[1][1] * dy) / self.k,
        )
    }

    /// Screen compass direction → game-unit direction vector.
    pub fn screen_dir_to_world(&self, deg: f64) -> (f64, f64) {
        let a = deg.to_radians();
        let (dx, dy) = (a.sin(), -a.cos());
        let m = self.m();
        (m[0][0] * dx + m[1][0] * dy, m[0][1] * dx + m[1][1] * dy)
    }

    pub fn on_new_connection(&mut self) {
        self.pending.clear();
        self.anchor = None;
        self.leg_start = None;
    }

    /// A minimap measurement.
    pub fn on_frame(&mut self, s: &MinimapSample) {
        self.frames += 1;
        self.last_frame_dt = self
            .last_frame_t
            .map(|f| (s.t - f).clamp(0, 1_000))
            .unwrap_or(0);
        self.last_frame_t = Some(s.t);
        self.black = s.black;
        self.frozen = s.frozen;
        if !self.seeded && s.ui_scale.is_finite() && s.ui_scale > 0.2 {
            self.k_seed = SEED_K * s.ui_scale;
            self.k = self.k_seed;
            self.k_before = self.k;
            self.seeded = true;
        } else if s.ui_scale.is_finite() && s.ui_scale > 0.2 {
            self.k_seed = SEED_K * s.ui_scale;
        }
        if let Some(c) = s.cone {
            self.cone = Some((c, s.cone_strength, s.t));
        }
        if s.recentred {
            // The next ring measurement is the new reference, not a zoom
            // change.
            self.ring_base = None;
            self.ring_odd = 0;
        }
        if let Some(r) = s.ring_r {
            self.on_ring(r);
        }
        if s.black {
            // The minimap is gone (loading screen, big map): the chain ends,
            // unless the tracker later picks the same segment up again.
            self.new_chain();
            self.last_seg = None;
            for mut a in std::mem::take(&mut self.pending) {
                a.p = None;
                self.apply(a);
            }
            return;
        }
        if s.x.is_nan() {
            return;
        }
        // After a black frame nothing continues; within a segment everything.
        let mut cont = self.last_seg.is_some();
        if self.last_seg != Some(s.seg) {
            let known = self.seg_chain.get(&s.seg).map(|(c, _)| *c);
            if self.last_seg.is_some() {
                self.breaks += 1;
            }
            match known {
                // Picked up again: back to that segment's chain.
                Some(c) => {
                    self.resume_segment(c, s);
                    cont = false;
                }
                // A new segment starts where the last one ended, so it
                // continues the chain if the break was short.
                None => {
                    if self.last_seg.is_some()
                        && self.last_good_t.is_none_or(|g| s.t - g > BRIDGE_MS)
                    {
                        self.new_chain();
                        cont = false;
                    }
                }
            }
        }
        self.seg_chain.insert(s.seg, (self.chain, s.t));
        if self.seg_chain.len() > SEG_CHAINS_KEPT {
            // Keep every segment the tracker may still pick up again.
            self.seg_chain
                .retain(|_, (_, seen)| s.t - *seen <= SEG_CHAIN_KEEP_MS);
        }
        self.last_seg = Some(s.seg);
        if s.good || !s.brk {
            self.last_good_t = Some(s.t);
        }
        // How much of this frame's movement may have gone unseen along a
        // road or coast: none for a round match peak, all of it for a flat
        // one, in between for an elongated one. (A cliff edge at the
        // threshold made a view just under it cost as much as a featureless
        // road, so ordinary scenes looked unsure within seconds.)
        if s.good {
            let severity = if s.ridge > 0.0 {
                ((s.ridge - s.roundness) / s.ridge).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if severity > 0.0 {
                let dt = (self.last_frame_dt as f64 * severity).round() as Millis;
                self.ridge_ms += dt;
                self.ridge_run += dt;
            } else {
                self.ridge_run = 0;
            }
        }
        let prev = self.track.back().copied();
        let pt = TrackPt {
            t: s.t,
            x: s.x,
            y: s.y,
            chain: self.chain,
            seg: s.seg,
            cont,
        };
        self.track.push_back(pt);
        while self.track.len() > TRACK_KEEP {
            self.track.pop_front();
        }
        let mut keep = Vec::new();
        for mut a in std::mem::take(&mut self.pending) {
            if a.t <= s.t {
                a.p = interp(prev, pt, a.t);
                self.apply(a);
            } else {
                keep.push(a);
            }
        }
        self.pending = keep;
    }

    fn new_chain(&mut self) {
        self.chain = self.next_chain;
        self.next_chain += 1;
    }

    /// The tracker picked an earlier segment up again, so positions are that
    /// segment's (in its chain `c`) from here on. A fix taken during the
    /// segment being left comes along: the offset between the two is what
    /// the last frame of one and this frame of the other show (0.1 s apart).
    fn resume_segment(&mut self, c: u64, s: &MinimapSample) {
        let from = self.last_seg;
        if let Some(last) = self
            .track
            .back()
            .copied()
            .filter(|l| Some(l.seg) == from && s.t - l.t <= 300)
        {
            let (ox, oy) = (s.x - last.x, s.y - last.y);
            for a in [self.anchor.as_mut(), self.leg_start.as_mut()]
                .into_iter()
                .flatten()
            {
                if let Some(p) = a.p.filter(|p| p.seg == last.seg) {
                    a.p = Some(TrackRef {
                        x: p.x + ox,
                        y: p.y + oy,
                        chain: c,
                        seg: s.seg,
                    });
                }
            }
        }
        self.chain = c;
    }

    /// A server fix of the own position (`zone`: map load / teleport, no leg
    /// across it). `now` is the newest time seen (packet clock).
    pub fn on_fix(&mut self, t: Millis, pos: [f32; 3], zone: bool) {
        let mut a = Anchor {
            t,
            pos,
            zone,
            p: None,
            breaks: self.breaks,
            ridge_ms: self.ridge_ms,
        };
        if self.track.back().is_some_and(|b| b.t >= t) {
            a.p = self.track_at(t);
            self.apply(a);
        } else if self.last_frame_t.is_none_or(|f| t - f > STALE_FRAME_MS) {
            self.apply(a); // no live minimap: plain fix
        } else {
            self.pending.push(a); // resolved with the next frame
        }
    }

    fn track_at(&self, t: Millis) -> Option<TrackRef> {
        let last = *self.track.back()?;
        if t >= last.t {
            return (t - last.t <= STALE_FRAME_MS).then_some(last.at());
        }
        for i in (1..self.track.len()).rev() {
            if self.track[i - 1].t <= t {
                return interp(Some(self.track[i - 1]), self.track[i], t);
            }
        }
        None
    }

    fn apply(&mut self, a: Anchor) {
        let ls = if a.zone { None } else { self.leg_start };
        match (ls, a.p) {
            (Some(ls), Some(p))
                if ls.p.is_some_and(|q| q.chain == p.chain)
                    && a.t > ls.t
                    && a.t - ls.t <= LEG_MAX_MS =>
            {
                let q = ls.p.expect("checked");
                let w = ((a.pos[0] - ls.pos[0]) as f64, (a.pos[1] - ls.pos[1]) as f64);
                let wm = w.0.hypot(w.1) / self.units_per_m;
                if wm >= LEG_MIN_M {
                    if wm <= LEG_MAX_M {
                        let d = (p.x - q.x, p.y - q.y);
                        let k = d.0.hypot(d.1) / w.0.hypot(w.1).max(1e-9);
                        self.leg(Leg { w, d, k });
                    }
                    self.leg_start = Some(a);
                }
                // Shorter: keep the start, the leg grows with the next fix.
            }
            _ => self.leg_start = if a.p.is_some() { Some(a) } else { None },
        }
        self.anchor = Some(a);
    }

    fn leg(&mut self, leg: Leg) {
        if leg.d.0.hypot(leg.d.1) < LEG_MIN_PX {
            // The server says we moved, the minimap did not: wrong region,
            // covered minimap or a frozen capture.
            self.map_still = true;
            return;
        }
        self.map_still = false;
        if !(K_RANGE.0 * self.k_seed..=K_RANGE.1 * self.k_seed).contains(&leg.k) {
            return;
        }
        if self.axis_ok && leg_dir_err(&leg, self.m()) > DIR_TOL_DEG {
            return;
        }
        match self.recal.take() {
            Some(Recal::Legs(mut c)) => {
                if c.len() == 1 && (leg.k / self.k_before - 1.0).abs() <= JUMP {
                    // One odd leg: the scale is unchanged.
                    self.k = self.k_before;
                    self.push_leg(leg);
                    return;
                }
                c.push(leg.k);
                self.finish_recal(c, leg, false);
            }
            Some(Recal::Ring(mut c)) => {
                if c.is_empty() && (leg.k / self.k_before - 1.0).abs() <= AGREE {
                    // The ring was misread; a leg confirms the scale.
                    self.k = self.k_before;
                    self.push_leg(leg);
                    return;
                }
                c.push(leg.k);
                self.finish_recal(c, leg, true);
            }
            None => {
                if (leg.k / self.k - 1.0).abs() > JUMP {
                    self.k_before = self.k;
                    self.recal = Some(Recal::Legs(vec![leg.k]));
                    return;
                }
                self.push_leg(leg);
                if self.legs.len() >= 3 {
                    let new = median(self.legs.iter().map(|l| l.k).collect());
                    if (new / self.k - 1.0).abs() > 0.002 {
                        self.k = new;
                        self.dirty = true;
                    }
                }
                self.check_axis();
            }
        }
    }

    /// Two agreeing candidate legs set the new scale.
    fn finish_recal(&mut self, c: Vec<f64>, leg: Leg, ring: bool) {
        if c.len() >= 2 {
            let (a, b) = (c[c.len() - 2], c[c.len() - 1]);
            if (a / b - 1.0).abs() <= AGREE {
                log::info!(
                    "minimap: zoom re-fitted, {:.3} -> {:.3} px/m",
                    self.k_before * self.units_per_m,
                    0.5 * (a + b) * self.units_per_m
                );
                self.k = 0.5 * (a + b);
                self.legs.clear();
                self.push_leg(leg);
                if let Some(r) = self.ring_now {
                    self.ring_base = Some(r);
                }
                self.dirty = true;
                return;
            }
        }
        self.recal = Some(if ring { Recal::Ring(c) } else { Recal::Legs(c) });
    }

    fn push_leg(&mut self, leg: Leg) {
        self.legs.push_back(leg);
        while self.legs.len() > N_LEGS {
            self.legs.pop_front();
        }
        self.legs_total += 1;
        self.dirty = true;
    }

    fn check_axis(&mut self) {
        if self.legs.len() < 4 {
            return;
        }
        let errs: Vec<f64> = AXIS_MAPS
            .iter()
            .map(|(_, m)| median(self.legs.iter().map(|l| leg_dir_err(l, m)).collect()))
            .collect();
        let best = (0..errs.len())
            .min_by(|a, b| errs[*a].total_cmp(&errs[*b]))
            .unwrap_or(0);
        if best != self.axis && errs[best] < 20.0 && errs[self.axis] > 45.0 {
            log::info!(
                "minimap: axis map re-fitted {} -> {}",
                AXIS_MAPS[self.axis].0,
                AXIS_MAPS[best].0
            );
            self.axis = best;
            self.dirty = true;
        }
        self.axis_ok = errs[self.axis] < 20.0;
    }

    fn on_ring(&mut self, r: f64) {
        self.ring_now = Some(r);
        let Some(base) = self.ring_base else {
            self.ring_base = Some(r);
            self.dirty = true;
            return;
        };
        if (r / base - 1.0).abs() > RING_JUMP {
            self.ring_odd += 1;
            if self.ring_odd >= 2 && self.recal.is_none() {
                log::info!("minimap: range ring {base:.1} -> {r:.1} px (zoom change?)");
                self.k_before = self.k;
                self.recal = Some(Recal::Ring(Vec::new()));
                self.ring_odd = 0;
            }
        } else {
            self.ring_odd = 0;
            if matches!(&self.recal, Some(Recal::Ring(c)) if c.is_empty()) {
                self.recal = None; // back to normal
            }
        }
    }

    /// Where the player is at `t` according to the last fix and the minimap.
    pub fn locate(&self, t: Millis) -> Located {
        let none = |kind, reason| Located {
            kind,
            pos: None,
            sigma: 0.0,
            reason,
        };
        if let Some(a) = self.pending.last() {
            return Located {
                kind: LocKind::Live,
                pos: Some(a.pos),
                sigma: 150.0,
                reason: "fresh fix",
            };
        }
        let Some(a) = self.anchor else {
            return none(LocKind::NoFix, "no server fix yet — open your wings once");
        };
        match &self.recal {
            Some(Recal::Ring(c)) if c.is_empty() => {
                return none(LocKind::Uncertain, "minimap zoom changed? checking");
            }
            Some(_) => return none(LocKind::Recal, "re-measuring the minimap scale"),
            None => {}
        }
        if self.map_still {
            return none(LocKind::Uncertain, "the minimap does not move with you");
        }
        if self.last_frame_t.is_none_or(|f| t - f > STALE_FRAME_MS) {
            return none(LocKind::Uncertain, "no minimap frames");
        }
        if self.black {
            return none(LocKind::Uncertain, "minimap hidden");
        }
        if self.frozen {
            return none(LocKind::Uncertain, "minimap capture frozen");
        }
        if self.ridge_run >= RIDGE_UNSURE_MS {
            return none(
                LocKind::Uncertain,
                "the minimap shows too little here (one road or coast)",
            );
        }
        let Some(p) = a.p else {
            return none(
                LocKind::Uncertain,
                "waiting for the next fix on the minimap track",
            );
        };
        let Some(cur) = self.track_at(t) else {
            return none(LocKind::Uncertain, "minimap track broken");
        };
        if cur.chain != p.chain {
            return none(LocKind::Uncertain, "minimap track broken");
        }
        let (wx, wy) = self.to_world(cur.x - p.x, cur.y - p.y);
        // Uncertainty: fix error, scale error over the distance moved, and a
        // little per track break bridged.
        let dist = wx.hypot(wy);
        let scale_err = if self.legs_total >= 3 { 0.03 } else { 0.12 };
        // Plain stretches: movement along the road may have been missed;
        // allow for running speed (~7 m/s) over that time.
        let ridge = (self.ridge_ms - a.ridge_ms) as f64 / 1000.0 * 700.0;
        let sigma = 150.0 + scale_err * dist + 200.0 * (self.breaks - a.breaks) as f64 + ridge;
        Located {
            kind: LocKind::Live,
            pos: Some([a.pos[0] + wx as f32, a.pos[1] + wy as f32, a.pos[2]]),
            sigma,
            reason: "",
        }
    }

    /// Camera direction (screen compass degrees) from the view cone, if
    /// clear and recent.
    pub fn camera(&self, t: Millis) -> Option<f64> {
        let (c, s, ct) = self.cone?;
        (s >= CONE_CLEAR && t - ct <= STALE_FRAME_MS).then_some(c)
    }

    pub fn px_per_m(&self) -> f64 {
        self.k * self.units_per_m
    }

    pub fn calibrated(&self) -> bool {
        self.legs_total >= 3
    }

    /// The calibration, when it has changed and is worth keeping.
    pub fn take_dirty(&mut self) -> Option<MinimapCalibration> {
        if !self.dirty || self.legs_total == 0 {
            return None;
        }
        self.dirty = false;
        Some(self.calibration())
    }

    pub fn calibration(&self) -> MinimapCalibration {
        MinimapCalibration {
            k: self.k,
            axis: AXIS_MAPS[self.axis].0.to_string(),
            ring_r: self.ring_base,
            legs: self.legs_total,
        }
    }

    pub fn status(&self, now: Millis) -> MinimapStatus {
        let loc = self.locate(now);
        let state = if self.frames == 0 {
            "no frames"
        } else {
            match loc.kind {
                LocKind::Live => "tracking",
                LocKind::Uncertain => "uncertain",
                LocKind::Recal => "recalibrating",
                LocKind::NoFix => "no fix",
            }
        };
        MinimapStatus {
            state: state.into(),
            reason: loc.reason.into(),
            px_per_m: Some(self.px_per_m()),
            axis: AXIS_MAPS[self.axis].0.into(),
            legs: self.legs_total,
            axis_confirmed: self.axis_ok,
            frames: self.frames,
            breaks: self.breaks,
            last_frame_age_ms: self.last_frame_t.map(|f| now - f),
            camera_deg: self.camera(now),
            black: self.black,
            frozen: self.frozen,
        }
    }
}

impl TrackPt {
    fn at(&self) -> TrackRef {
        TrackRef {
            x: self.x,
            y: self.y,
            chain: self.chain,
            seg: self.seg,
        }
    }
}

/// Track position at `t` between two consecutive samples whose coordinates
/// continue (not across a resumed segment), or the newer sample when `t` is
/// within 150 ms of it.
fn interp(a: Option<TrackPt>, b: TrackPt, t: Millis) -> Option<TrackRef> {
    match a {
        Some(a) if a.chain == b.chain && b.cont && b.t - a.t <= 1_000 && t >= a.t => {
            let f = (t - a.t) as f64 / (b.t - a.t).max(1) as f64;
            Some(TrackRef {
                x: a.x + f * (b.x - a.x),
                y: a.y + f * (b.y - a.y),
                ..b.at()
            })
        }
        _ => ((b.t - t).abs() <= 150).then_some(b.at()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UPM: f64 = 100.0;

    /// A walk in game units, its minimap track under scale `k` and axis map
    /// `m` (world → screen), sampled at 10 Hz.
    struct World {
        k: f64,
        m: [[f64; 2]; 2],
        origin: (f64, f64),
    }

    impl World {
        fn pos(&self, t: Millis) -> [f32; 3] {
            let s = t as f64 / 1000.0;
            // 6 m/s around a 60 m circle, so legs point every way.
            let x = self.origin.0 + 6_000.0 * (s / 10.0).cos();
            let y = self.origin.1 + 6_000.0 * (s / 10.0).sin();
            [x as f32, y as f32, 1000.0]
        }

        fn sample(&self, t: Millis) -> MinimapSample {
            let p = self.pos(t);
            let (dx, dy) = (p[0] as f64 - self.origin.0, p[1] as f64 - self.origin.1);
            let sx = self.k * (self.m[0][0] * dx + self.m[0][1] * dy);
            let sy = self.k * (self.m[1][0] * dx + self.m[1][1] * dy);
            MinimapSample {
                t,
                x: 50.0 + sx,
                y: -20.0 + sy,
                seg: 0,
                good: true,
                brk: false,
                black: false,
                frozen: false,
                peak: 0.9,
                roundness: 0.5,
                ridge: 0.065,
                cone: Some(45.0),
                cone_strength: 0.6,
                ring_r: None,
                recentred: false,
                ui_scale: 1.0,
            }
        }
    }

    fn run(world: &World, fix_every: Millis, secs: i64, f: &mut MinimapFusion) -> f64 {
        let mut worst: f64 = 0.0;
        for i in 0..secs * 10 {
            let t = i * 100;
            f.on_frame(&world.sample(t));
            // Fixes arrive between frames, as they do in the game.
            if t % fix_every == 0 {
                f.on_fix(t + 50, world.pos(t + 50), false);
            }
            if t > 20_000 {
                let loc = f.locate(t);
                if let Some(p) = loc.pos {
                    let q = world.pos(t);
                    worst = worst.max(((p[0] - q[0]) as f64).hypot((p[1] - q[1]) as f64));
                }
            }
        }
        worst / UPM
    }

    #[test]
    fn with_the_seed_scale_it_tracks_between_fixes() {
        let w = World {
            k: SEED_K,
            m: AXIS_MAPS[0].1,
            origin: (-40_000.0, 70_000.0),
        };
        let mut f = MinimapFusion::new(UPM, None);
        // Fixes every 30 s only.
        let worst = run(&w, 30_000, 120, &mut f);
        assert!(worst < 1.0, "worst {worst} m");
        assert_eq!(f.status(120_000).state, "tracking");
    }

    #[test]
    fn learns_a_different_scale_and_axis_from_legs() {
        // UI scale 0.75 (1080p) but a zoomed minimap and a mirrored axis.
        let w = World {
            k: SEED_K * 1.6,
            m: AXIS_MAPS[1].1, // right=+x, down=-y
            origin: (10_000.0, -5_000.0),
        };
        let mut f = MinimapFusion::new(UPM, None);
        // Frequent fixes while learning (combat buffs).
        run(&w, 5_000, 120, &mut f);
        let cal = f.calibration();
        assert!((cal.k / (SEED_K * 1.6) - 1.0).abs() < 0.02, "{cal:?}");
        assert_eq!(cal.axis, "right=+x, down=-y");
        // Then with rare fixes it stays accurate.
        let mut g = MinimapFusion::new(UPM, Some(&cal));
        let worst = run(&w, 40_000, 120, &mut g);
        assert!(worst < 1.0, "worst {worst} m");
    }

    #[test]
    fn elongated_views_cost_in_proportion_and_only_flat_ones_make_it_unsure() {
        let w = World {
            k: SEED_K,
            m: AXIS_MAPS[0].1,
            origin: (0.0, 50_000.0),
        };
        let sigma_after = |roundness: f64, secs: i64| {
            let mut f = MinimapFusion::new(UPM, None);
            f.on_frame(&w.sample(0));
            f.on_fix(50, w.pos(50), false);
            for i in 1..=secs * 10 {
                let mut s = w.sample(i * 100);
                s.roundness = roundness;
                f.on_frame(&s);
            }
            let loc = f.locate(secs * 1000);
            (loc.kind, loc.sigma / UPM)
        };
        // A round peak: fix error plus 12 % (unconfirmed scale) of the
        // ~100 m from the fix.
        let (kind, round) = sigma_after(0.5, 20);
        assert_eq!(kind, LocKind::Live);
        assert!(round < 16.0, "{round}");
        // Just under the ridge threshold (common in ordinary scenes): about
        // the same, not +7 m every second.
        let (kind, near) = sigma_after(0.06, 20);
        assert_eq!(kind, LocKind::Live);
        assert!(near - round < 12.0, "{near} vs {round}");
        // A flat peak (a featureless road): unsure after 3 s.
        let (kind, _) = sigma_after(0.0, 4);
        assert_eq!(kind, LocKind::Uncertain);
    }

    #[test]
    fn a_fix_waits_for_the_next_frame_and_breaks_are_bridged() {
        let w = World {
            k: SEED_K,
            m: AXIS_MAPS[0].1,
            origin: (0.0, 50_000.0),
        };
        let mut f = MinimapFusion::new(UPM, None);
        f.on_frame(&w.sample(0));
        // The fix arrives between frames.
        f.on_fix(50, w.pos(50), false);
        assert_eq!(f.locate(60).reason, "fresh fix");
        f.on_frame(&w.sample(100));
        assert_eq!(f.locate(100).kind, LocKind::Live);
        // A one-frame break (new tracker segment) is bridged.
        let mut s = w.sample(200);
        s.seg = 1;
        f.on_frame(&s);
        let loc = f.locate(200);
        assert_eq!(loc.kind, LocKind::Live);
        assert!(loc.sigma > 150.0);
        // A black frame (minimap hidden) ends the chain.
        let mut b = w.sample(300);
        b.black = true;
        b.x = f64::NAN;
        f.on_frame(&b);
        assert_eq!(f.locate(300).kind, LocKind::Uncertain);
    }

    #[test]
    fn a_segment_picked_up_again_carries_on_without_a_new_fix() {
        let w = World {
            k: SEED_K,
            m: AXIS_MAPS[0].1,
            origin: (0.0, 50_000.0),
        };
        for covered_by_black in [true, false] {
            let mut f = MinimapFusion::new(UPM, None);
            f.on_frame(&w.sample(0));
            f.on_fix(50, w.pos(50), false);
            for i in 1..50 {
                f.on_frame(&w.sample(i * 100));
            }
            assert_eq!(f.locate(4_900).kind, LocKind::Live);
            // 3 s covered: black frames, or the big map tracked as a
            // segment of its own (a still picture: the position stops where
            // it was, while we walk on).
            let held = w.sample(4_900);
            for i in 50..80 {
                let mut s = w.sample(i * 100);
                if covered_by_black {
                    s.black = true;
                    s.x = f64::NAN;
                } else {
                    s.seg = 7;
                    (s.x, s.y) = (held.x, held.y);
                }
                f.on_frame(&s);
            }
            if covered_by_black {
                assert_eq!(f.locate(7_900).kind, LocKind::Uncertain);
            }
            // The tracker picks segment 0 up again: positions are right
            // again without opening the wings.
            for i in 80..100 {
                f.on_frame(&w.sample(i * 100));
            }
            let loc = f.locate(9_900);
            assert_eq!(loc.kind, LocKind::Live, "black={covered_by_black}");
            let (p, q) = (loc.pos.unwrap(), w.pos(9_900));
            let err = ((p[0] - q[0]) as f64).hypot((p[1] - q[1]) as f64) / UPM;
            assert!(err < 0.5, "black={covered_by_black}: {err} m");
        }
    }

    #[test]
    fn a_fix_during_a_bridged_break_counts_at_low_frame_rates() {
        // 4 readings per second: frames 250 ms apart.
        let w = World {
            k: SEED_K,
            m: AXIS_MAPS[0].1,
            origin: (0.0, 50_000.0),
        };
        let mut f = MinimapFusion::new(UPM, None);
        for i in 0..8 {
            f.on_frame(&w.sample(i * 250));
        }
        // The fix arrives just after the last frame of segment 0 and is
        // resolved with the first frame of the fresh segment 1, 200 ms on.
        f.on_fix(1_800, w.pos(1_800), false);
        for i in 8..16 {
            let mut s = w.sample(i * 250);
            s.seg = 1;
            f.on_frame(&s);
        }
        let loc = f.locate(3_750);
        assert_eq!(loc.kind, LocKind::Live, "{}", loc.reason);
        let (p, q) = (loc.pos.unwrap(), w.pos(3_750));
        let err = ((p[0] - q[0]) as f64).hypot((p[1] - q[1]) as f64) / UPM;
        assert!(err < 1.0, "{err} m");
    }

    #[test]
    fn a_long_flicker_does_not_make_it_forget_the_segment_to_resume() {
        let w = World {
            k: SEED_K,
            m: AXIS_MAPS[0].1,
            origin: (0.0, 50_000.0),
        };
        let mut f = MinimapFusion::new(UPM, None);
        f.on_frame(&w.sample(0));
        f.on_fix(50, w.pos(50), false);
        for i in 1..30 {
            f.on_frame(&w.sample(i * 100));
        }
        // An animated cover: a new tracker segment on every frame, 6 s long.
        let held = w.sample(2_900);
        for i in 30..90 {
            let mut s = w.sample(i * 100);
            s.seg = 100 + i;
            s.good = false;
            s.brk = true;
            (s.x, s.y) = (held.x, held.y);
            f.on_frame(&s);
        }
        // Back to segment 0's place.
        for i in 90..100 {
            f.on_frame(&w.sample(i * 100));
        }
        let loc = f.locate(9_900);
        assert_eq!(loc.kind, LocKind::Live, "{}", loc.reason);
        let (p, q) = (loc.pos.unwrap(), w.pos(9_900));
        let err = ((p[0] - q[0]) as f64).hypot((p[1] - q[1]) as f64) / UPM;
        assert!(err < 0.5, "{err} m");
    }

    #[test]
    fn a_fix_during_the_cover_moves_with_the_resumed_segment() {
        let w = World {
            k: SEED_K,
            m: AXIS_MAPS[0].1,
            origin: (0.0, 50_000.0),
        };
        let mut f = MinimapFusion::new(UPM, None);
        for i in 0..30 {
            f.on_frame(&w.sample(i * 100));
        }
        // A new segment (seg 3) tracks properly for 2 s, offset by 40 px…
        for i in 30..50 {
            let mut s = w.sample(i * 100);
            s.seg = 3;
            s.x += 40.0;
            f.on_frame(&s);
            if i == 40 {
                f.on_fix(i * 100 + 50, w.pos(i * 100 + 50), false); // …and anchors
            }
        }
        // …then the tracker realises it is segment 0's place after all.
        for i in 50..70 {
            f.on_frame(&w.sample(i * 100));
        }
        let loc = f.locate(6_900);
        assert_eq!(loc.kind, LocKind::Live);
        let (p, q) = (loc.pos.unwrap(), w.pos(6_900));
        let err = ((p[0] - q[0]) as f64).hypot((p[1] - q[1]) as f64) / UPM;
        assert!(err < 1.0, "{err} m");
    }

    #[test]
    fn a_minimap_that_does_not_move_is_reported() {
        let w = World {
            k: SEED_K,
            m: AXIS_MAPS[0].1,
            origin: (0.0, 50_000.0),
        };
        let mut f = MinimapFusion::new(UPM, None);
        for i in 0..200 {
            let t = i * 100;
            let mut s = w.sample(0); // frozen picture
            s.t = t;
            f.on_frame(&s);
            if i % 50 == 0 {
                f.on_fix(t, w.pos(t), false);
            }
        }
        let st = f.status(20_000);
        assert_eq!(st.state, "uncertain");
        assert!(st.reason.contains("does not move"), "{st:?}");
    }

    #[test]
    fn camera_direction_maps_to_the_world() {
        let f = MinimapFusion::new(UPM, None);
        // Seed axis right=+x, down=+y: screen up (0°) is world −Y.
        let (x, y) = f.screen_dir_to_world(0.0);
        assert!(x.abs() < 1e-9 && (y + 1.0).abs() < 1e-9);
        let (x, y) = f.screen_dir_to_world(90.0);
        assert!((x - 1.0).abs() < 1e-9 && y.abs() < 1e-9);
    }
}
