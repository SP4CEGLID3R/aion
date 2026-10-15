//! Where the player arrow is in the minimap rectangle.
//!
//! The rectangle comes from a default spot or a box the user drew, so its
//! centre can be a few pixels off the arrow. The arrow sits in a light disk
//! that stays put on screen while the map scrolls under it: over views taken
//! at different places, the pixels around the true pivot hardly change and
//! everything else does. The finder looks for the point whose surrounding
//! ring (inside the disk, outside the arrow, which turns with the character)
//! varies least, and trusts it once two separate measurements agree.

use crate::overlay::DISK_R;
use crate::plane::Plane;

/// Views kept for one measurement.
const VIEWS: usize = 8;
/// Views must be at least this far apart (in disk radii of map movement).
const VIEW_SPACING: f64 = 0.35;
/// Ring tested around a candidate, as fractions of the disk radius: inside
/// the light disk, outside the arrow.
const RING_IN: f64 = 0.45;
const RING_OUT: f64 = 0.75;
/// Search half-width around the rectangle centre, share of its short side.
const SEARCH: f64 = 0.2;
/// The best candidate must vary this much less than a typical one…
const CONTRAST: f64 = 0.3;
/// …and the map must vary at all (grey levels; open water does not).
const MIN_TEXTURE: f64 = 4.0;
/// Candidates within this share of (typical − best) of the best are as good.
const PATCH: f64 = 0.1;
/// Two measurements this close (full-resolution px) agree.
const AGREE_PX: f64 = 2.0;

pub struct PivotFinder {
    /// Grid the views come in (the tracker's, `ds` full-resolution px each).
    w: usize,
    h: usize,
    ds: usize,
    disk_r: f64,
    views: Vec<(Plane, (f64, f64))>,
    candidate: Option<(f64, f64)>,
    found: Option<(f64, f64)>,
}

impl PivotFinder {
    /// `w`×`h` views at `ds`; `ui_scale` sizes the disk.
    pub fn new(w: usize, h: usize, ds: usize, ui_scale: f64) -> PivotFinder {
        PivotFinder {
            w,
            h,
            ds: ds.max(1),
            disk_r: DISK_R * ui_scale / ds.max(1) as f64,
            views: Vec::new(),
            candidate: None,
            found: None,
        }
    }

    /// The pivot in full-resolution px, once found.
    pub fn found(&self) -> Option<(f64, f64)> {
        self.found
    }

    /// A view taken with the map at `pos` (tracked position, full-resolution
    /// px). Returns the pivot when this view settles it.
    pub fn offer(&mut self, view: &Plane, pos: (f64, f64)) -> Option<(f64, f64)> {
        if self.found.is_some() || view.w != self.w || view.h != self.h {
            return None;
        }
        let spacing = VIEW_SPACING * self.disk_r * self.ds as f64;
        if self
            .views
            .iter()
            .any(|(_, p)| (p.0 - pos.0).hypot(p.1 - pos.1) < spacing)
        {
            return None;
        }
        self.views.push((view.clone(), pos));
        if self.views.len() < VIEWS {
            return None;
        }
        let measured = self.measure();
        // The next measurement shares half its views with this one, so the
        // two agree or not soon (about 20 s of walking).
        self.views.drain(..VIEWS / 2);
        let p = measured?;
        match self.candidate {
            Some(c) if (c.0 - p.0).hypot(c.1 - p.1) <= AGREE_PX => {
                let f = ((c.0 + p.0) / 2.0, (c.1 + p.1) / 2.0);
                self.found = Some(f);
                Some(f)
            }
            _ => {
                self.candidate = Some(p);
                None
            }
        }
    }

    /// The pivot from the current views, full-resolution px.
    fn measure(&self) -> Option<(f64, f64)> {
        let (w, h) = (self.w, self.h);
        let n = self.views.len() as f64;
        // Per-pixel spread over the views.
        let mut sd = Plane::new(w, h);
        for i in 0..w * h {
            let mean = self.views.iter().map(|(v, _)| v.data[i]).sum::<f64>() / n;
            let var = self
                .views
                .iter()
                .map(|(v, _)| (v.data[i] - mean).powi(2))
                .sum::<f64>()
                / n;
            sd.data[i] = var.sqrt();
        }
        let mut all = sd.data.clone();
        all.sort_by(f64::total_cmp);
        if all[all.len() / 2] < MIN_TEXTURE {
            return None;
        }
        let ring: Vec<(isize, isize)> = {
            let (r0, r1) = (RING_IN * self.disk_r, RING_OUT * self.disk_r);
            let m = r1.ceil() as isize;
            (-m..=m)
                .flat_map(|dy| (-m..=m).map(move |dx| (dx, dy)))
                .filter(|&(dx, dy)| {
                    let r = (dx as f64).hypot(dy as f64);
                    r >= r0 && r <= r1
                })
                .collect()
        };
        let reach = (RING_OUT * self.disk_r).ceil() as isize;
        let half = (SEARCH * w.min(h) as f64).round() as isize;
        let (cx, cy) = (w as isize / 2, h as isize / 2);
        let mut cands = Vec::new();
        for y in (cy - half).max(reach)..=(cy + half).min(h as isize - 1 - reach) {
            for x in (cx - half).max(reach)..=(cx + half).min(w as isize - 1 - reach) {
                let s = ring
                    .iter()
                    .map(|&(dx, dy)| sd.at((x + dx) as usize, (y + dy) as usize))
                    .sum::<f64>()
                    / ring.len() as f64;
                cands.push((s, x, y));
            }
        }
        if cands.is_empty() {
            return None;
        }
        let mut scores: Vec<f64> = cands.iter().map(|c| c.0).collect();
        scores.sort_by(f64::total_cmp);
        let (best, typical) = (scores[0], scores[scores.len() / 2]);
        if best > CONTRAST * typical {
            return None;
        }
        // A disk a little larger than the ring gives a small patch of equally
        // good points around the centre: take the middle of that patch.
        let near = best + PATCH * (typical - best);
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for &(s, x, y) in &cands {
            if s <= near {
                sx += x as f64;
                sy += y as f64;
                n += 1.0;
            }
        }
        // Grid to full-resolution px (inverse of the tracker's pivot → grid).
        let ds = self.ds as f64;
        let full = |g: f64| g * ds + (ds - 1.0) / 2.0;
        Some((full(sx / n), full(sy / n)))
    }
}
