//! One minimap frame at a time: how far the map has scrolled (the player's
//! movement in minimap pixels) and which way the camera looks.
//!
//! Keyframe registration: each frame is matched against a keyframe (refreshed
//! every second, after a large move, or when the match weakens), so small
//! per-frame errors do not pile up. If the keyframe match fails, the previous
//! frame is tried; if that fails too, the track breaks and a new segment
//! starts (positions of different segments are not comparable). This follows
//! the design of madara134/aion2-cube-finder's minimap tracker (MIT).

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use crate::ncc::{Correlator, Spectra};
use crate::overlay::{
    cone_angle, cyan, icon_mask, overlay_mask, ring_radius, CONE_CLEAR, CONE_EDGE_DEG, DISK_R, EDGE,
};
use crate::plane::{Bgra, Mask, Plane};

/// Milliseconds, the same clock as the packet capture (unix time).
pub type Millis = i64;

/// Frames smaller than this (pixels, on the short side scaled to the default
/// aspect) are tracked at full resolution; larger ones at half.
const SMALL_RECT_PX: f64 = 300.0;
/// A keyframe is replaced after this much movement (share of the short side).
const KEY_SHARE: f64 = 0.15;
/// ...or when the match with it drops below this.
const KEY_PEAK: f64 = 0.7;
/// The largest shift searched (share of the short side).
const SHIFT_SHARE: f64 = 0.3;
/// Mean and spread below which a frame is black (capture blocked).
const BLACK_MEAN: f64 = 12.0;
const BLACK_STD: f64 = 6.0;
/// Identical frames for this long: the capture is probably frozen.
const FROZEN_MS: Millis = 2_000;
/// Matches with a peak less round than this measure movement along a road
/// or coast badly (synthetic tests: ≤ 0.055 when wrong, ≥ 0.08 when right).
pub const RIDGE: f64 = 0.065;
/// Range ring: sampled every 10th frame, measured every 50th.
const RING_SAMPLE_EVERY: u64 = 10;
const RING_EVERY: u64 = 50;

#[derive(Debug, Clone)]
pub struct TrackerConfig {
    /// The player arrow in the frame (full-resolution px); default: centre.
    pub pivot: Option<(f64, f64)>,
    /// Size of the minimap's arrow and markers relative to the default UI at
    /// 2560×1440 (≈ screen height / 1440).
    pub ui_scale: f64,
    /// 1 or 2; `None` picks from the frame size.
    pub downsample: Option<usize>,
    /// Keyframe refresh interval (fewer refreshes = less drift; 3 s measured
    /// best on synthetic walks).
    pub key_ms: Millis,
    /// Measure the view cone every N frames.
    pub cone_every: u64,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        TrackerConfig {
            pivot: None,
            ui_scale: 1.0,
            downsample: None,
            key_ms: 3_000,
            cone_every: 5,
        }
    }
}

/// The result for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub t: Millis,
    /// Player position in minimap pixels (full resolution), integrated since
    /// the segment started; NaN when unknown.
    pub x: f64,
    pub y: f64,
    /// Track segment; positions are comparable only within one segment.
    pub seg: i64,
    /// A translation was measured and accepted.
    pub good: bool,
    /// The track broke here (weak match, black frame).
    pub brk: bool,
    pub peak: f64,
    /// How round the match peak was (1 = round; below [`RIDGE`] the view is
    /// dominated by one straight feature and movement along it is unsure).
    pub roundness: f64,
    /// View cone direction (screen compass degrees, 0 = up): the camera.
    pub cone: Option<f64>,
    pub cone_strength: f64,
    pub black: bool,
    pub frozen: bool,
    /// A new range-ring measurement on this frame (rare).
    pub ring_r: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct TrackerStats {
    pub frames: u64,
    pub black: u64,
    pub steps: u64,
    pub good: u64,
    pub fallback: u64,
    pub breaks: u64,
    pub cone_checks: u64,
    pub cone_clear: u64,
    /// Time the last frame took, milliseconds.
    pub last_ms: f64,
}

struct Key {
    spec: Rc<Spectra>,
    pos: (f64, f64),
    t: Millis,
}

pub struct MinimapTracker {
    full_w: usize,
    full_h: usize,
    ds: usize,
    w: usize,
    h: usize,
    pivot: (f64, f64),
    pv: (f64, f64),
    scale: f64,
    cfg: TrackerConfig,
    cor: Correlator,
    key_px: f64,
    ring_r: Option<f64>,
    base: Option<Mask>,
    cone_masks: Vec<((i64, bool), Mask)>,
    ring_planes: VecDeque<Plane>,
    n: u64,
    seg: i64,
    key: Option<Key>,
    prev: Option<Key>,
    last: Option<(f64, f64)>,
    cone: Option<f64>,
    cone_s: f64,
    last_print: Option<(u64, Millis)>,
    same_since: Option<Millis>,
    stats: TrackerStats,
}

/// Downsampling for a minimap rectangle: full resolution for small (1080p
/// and below) rectangles, half for large ones.
pub fn choose_downsample(w: usize, h: usize) -> usize {
    if (w as f64).min(h as f64 * 1.5) < SMALL_RECT_PX {
        1
    } else {
        2
    }
}

impl MinimapTracker {
    pub fn new(full_w: usize, full_h: usize, cfg: TrackerConfig) -> MinimapTracker {
        let ds = cfg
            .downsample
            .filter(|d| *d == 1 || *d == 2)
            .unwrap_or_else(|| choose_downsample(full_w, full_h));
        let (w, h) = (full_w / ds, full_h / ds);
        let pivot = cfg
            .pivot
            .unwrap_or((full_w as f64 / 2.0, full_h as f64 / 2.0));
        let pv = (
            (pivot.0 - (ds as f64 - 1.0) / 2.0) / ds as f64,
            (pivot.1 - (ds as f64 - 1.0) / 2.0) / ds as f64,
        );
        let short = w.min(h) as f64;
        let scale = if cfg.ui_scale.is_finite() && cfg.ui_scale > 0.2 {
            cfg.ui_scale
        } else {
            1.0
        };
        MinimapTracker {
            full_w,
            full_h,
            ds,
            w,
            h,
            pivot,
            pv,
            scale,
            cor: Correlator::new(w, h, ((SHIFT_SHARE * short) as usize).max(4)),
            key_px: KEY_SHARE * short,
            ring_r: None,
            base: None,
            cone_masks: Vec::new(),
            ring_planes: VecDeque::new(),
            n: 0,
            seg: -1,
            key: None,
            prev: None,
            last: None,
            cone: None,
            cone_s: 0.0,
            last_print: None,
            same_since: None,
            stats: TrackerStats::default(),
            cfg,
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.full_w, self.full_h)
    }

    pub fn pivot(&self) -> (f64, f64) {
        self.pivot
    }

    pub fn downsample(&self) -> usize {
        self.ds
    }

    pub fn ring_radius(&self) -> Option<f64> {
        self.ring_r
    }

    pub fn stats(&self) -> &TrackerStats {
        &self.stats
    }

    fn base_mask(&mut self) -> Mask {
        if self.base.is_none() {
            let ring = self.ring_r.map(|r| r / self.ds as f64);
            self.base = Some(overlay_mask(
                self.w,
                self.h,
                self.pv,
                None,
                ring,
                true,
                DISK_R * self.scale / self.ds as f64 + 2.0,
                EDGE,
                CONE_EDGE_DEG,
            ));
        }
        self.base.clone().expect("just built")
    }

    fn cone_mask(&mut self, cone: f64, wide: bool) -> Mask {
        let key = ((cone / 2.0).round() as i64 % 180, wide);
        if let Some((_, m)) = self.cone_masks.iter().find(|(k, _)| *k == key) {
            return m.clone();
        }
        if self.cone_masks.len() > 64 {
            self.cone_masks.clear();
        }
        let m = overlay_mask(
            self.w,
            self.h,
            self.pv,
            Some(key.0 as f64 * 2.0),
            None,
            false,
            0.0,
            0,
            CONE_EDGE_DEG * if wide { 3.0 } else { 1.0 },
        );
        self.cone_masks.push((key, m.clone()));
        m
    }

    fn restart(&mut self, spec: Rc<Spectra>, t: Millis) -> (f64, f64) {
        self.seg += 1;
        let pos = self.last.unwrap_or((0.0, 0.0));
        self.key = Some(Key {
            spec: spec.clone(),
            pos,
            t,
        });
        self.prev = Some(Key { spec, pos, t });
        self.last = Some(pos);
        pos
    }

    fn step_result(
        &self,
        t: Millis,
        pos: Option<(f64, f64)>,
        good: bool,
        brk: bool,
        peak: f64,
    ) -> Step {
        let (x, y) = pos.unwrap_or((f64::NAN, f64::NAN));
        Step {
            t,
            x,
            y,
            seg: self.seg,
            good,
            brk,
            peak,
            roundness: 1.0,
            cone: self.cone,
            cone_strength: self.cone_s,
            black: false,
            frozen: false,
            ring_r: None,
        }
    }

    /// Track one frame (`t` in the packet clock).
    pub fn step(&mut self, t: Millis, frame: &Bgra) -> Step {
        let started = Instant::now();
        let mut out = self.step_inner(t, frame);
        self.stats.last_ms = started.elapsed().as_secs_f64() * 1000.0;
        // Frozen capture: identical pixels for a while.
        let print = frame.fingerprint();
        match self.last_print {
            Some((p, prev_t)) if p == print => {
                // Unchanged since the previous frame: frozen once that has
                // lasted FROZEN_MS.
                let since = *self.same_since.get_or_insert(prev_t);
                out.frozen = t - since >= FROZEN_MS;
            }
            _ => self.same_since = None,
        }
        self.last_print = Some((print, t));
        out
    }

    fn step_inner(&mut self, t: Millis, frame: &Bgra) -> Step {
        self.n += 1;
        self.stats.frames += 1;
        if frame.w != self.full_w || frame.h != self.full_h {
            // A different size is a different minimap: start over.
            self.key = None;
            self.prev = None;
            let mut s = self.step_result(t, None, false, true, 0.0);
            s.black = true;
            return s;
        }
        let (mean, std) = frame.stats();
        if mean < BLACK_MEAN && std < BLACK_STD {
            self.stats.black += 1;
            self.key = None;
            self.prev = None;
            let mut s = self.step_result(t, None, false, true, 0.0);
            s.black = true;
            return s;
        }
        let gray = frame.gray();
        if self.cone.is_none() || self.n.is_multiple_of(self.cfg.cone_every.max(1)) {
            let (c, s) = cone_angle(
                &gray,
                self.pivot,
                DISK_R * self.scale + 6.0,
                95.0 * self.scale,
            );
            self.stats.cone_checks += 1;
            if s >= CONE_CLEAR {
                self.stats.cone_clear += 1;
            }
            self.cone = Some(c);
            self.cone_s = s;
        }
        let mut ring_new = None;
        if self.n == 1 || self.n.is_multiple_of(RING_SAMPLE_EVERY) {
            self.ring_planes.push_back(cyan(frame));
            while self.ring_planes.len() > 5 {
                self.ring_planes.pop_front();
            }
        }
        // The ring is fixed on screen: left unmasked it pins small minimaps
        // to "no movement", so look for it on the first frame already.
        if self.n == 1 || (self.n.is_multiple_of(RING_EVERY) && self.ring_planes.len() >= 3) {
            let planes: Vec<Plane> = self.ring_planes.iter().cloned().collect();
            ring_new = ring_radius(&planes, self.pivot, DISK_R * self.scale + 6.0);
            if let Some(r) = ring_new {
                if self.ring_r.is_none_or(|old| (old - r).abs() > 1.0) {
                    self.ring_r = Some(r);
                    self.base = None;
                }
            }
        }
        let small = if self.ds == 2 { gray.half() } else { gray };
        let small = crop(&small, self.w, self.h);
        let mut mask = self.base_mask();
        if let Some(c) = self.cone {
            let clear = self.cone_s >= CONE_CLEAR;
            mask = mask.and(&self.cone_mask(c, !clear));
        }
        mask = mask
            .and_not(&icon_mask(frame, self.ds, self.w, self.h))
            .erode(1);
        let spec = Rc::new(self.cor.spectra(&small, &mask));

        let Some(key) = self.key.as_ref() else {
            let pos = self.restart(spec, t);
            let mut s = self.step_result(t, Some(pos), false, false, 1.0);
            s.ring_r = ring_new;
            return s;
        };
        self.stats.steps += 1;
        let mut shift = self.cor.shift(&key.spec, &spec);
        let mut base = (key.pos, key.t, key.spec.clone());
        if !shift.good() {
            if let Some(prev) = self.prev.as_ref() {
                if !Rc::ptr_eq(&prev.spec, &key.spec) {
                    let s2 = self.cor.shift(&prev.spec, &spec);
                    if s2.good() {
                        self.stats.fallback += 1;
                        shift = s2;
                        base = (prev.pos, prev.t, prev.spec.clone());
                        self.key = Some(Key {
                            spec: prev.spec.clone(),
                            pos: prev.pos,
                            t: prev.t,
                        });
                    }
                }
            }
        }
        if !shift.good() {
            self.stats.breaks += 1;
            let pos = self.restart(spec, t);
            let mut s = self.step_result(t, Some(pos), false, true, shift.peak);
            s.ring_r = ring_new;
            return s;
        }
        self.stats.good += 1;
        let ds = self.ds as f64;
        let pos = (base.0 .0 - shift.dx * ds, base.0 .1 - shift.dy * ds);
        self.last = Some(pos);
        self.prev = Some(Key {
            spec: spec.clone(),
            pos,
            t,
        });
        if shift.dx.hypot(shift.dy) > self.key_px
            || t - base.1 >= self.cfg.key_ms
            || shift.peak < KEY_PEAK
        {
            self.key = Some(Key { spec, pos, t });
        }
        let mut s = self.step_result(t, Some(pos), true, false, shift.peak);
        s.ring_r = ring_new;
        s.roundness = shift.roundness;
        s
    }
}

fn crop(p: &Plane, w: usize, h: usize) -> Plane {
    if p.w == w && p.h == h {
        return p.clone();
    }
    let mut out = Plane::new(w, h);
    for y in 0..h {
        out.data[y * w..(y + 1) * w].copy_from_slice(&p.data[y * p.w..y * p.w + w]);
    }
    out
}
