//! The hand-over point between the screen reader (minimap tracker, in the
//! app) and the packet decoder (here): minimap measurements go in, the
//! calibration the decoder learns comes back out for drawing.

use std::collections::VecDeque;
use std::sync::Mutex;

use radar_core::Millis;
use serde::Serialize;

/// One minimap frame, as measured by the screen tracker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapSample {
    /// Same clock as the packet capture (unix ms).
    pub t: Millis,
    /// Player position in minimap pixels, integrated by the tracker; NaN
    /// when unknown. Only differences within one `seg` mean anything.
    pub x: f64,
    pub y: f64,
    pub seg: i64,
    pub good: bool,
    /// The track broke on this frame (weak match).
    pub brk: bool,
    /// The capture is black (minimap hidden, exclusive fullscreen, loading).
    pub black: bool,
    /// The capture has not changed for a while (blocked or paused game).
    pub frozen: bool,
    pub peak: f64,
    /// Roundness of the match peak; below `ridge` the view is one straight
    /// feature (road, coast) and movement along it is unsure.
    pub roundness: f64,
    /// The tracker's threshold for `roundness`.
    pub ridge: f64,
    /// View cone (camera) direction, screen compass degrees.
    pub cone: Option<f64>,
    pub cone_strength: f64,
    /// A new range-ring radius measurement (px), rarely.
    pub ring_r: Option<f64>,
    /// The tracker moved its pivot onto the arrow: ring sizes measured
    /// before are not comparable with later ones.
    pub recentred: bool,
    /// Size of the minimap UI relative to the default at 2560×1440.
    pub ui_scale: f64,
}

/// What the decoder knows about the minimap's scale, for drawing it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinimapView {
    /// Minimap pixels per metre for drawing it (seeded until calibrated;
    /// right after a zoom change, scaled by the range ring's change).
    pub px_per_m: Option<f64>,
    pub calibrated: bool,
    /// "tracking", "uncertain", "recalibrating", "no fix" or "off".
    pub state: String,
    /// Why, when not tracking.
    pub reason: String,
    /// Camera direction (world compass degrees) from the view cone.
    pub camera_deg: Option<f64>,
    /// Calibration legs so far (0 = seeded scale only).
    pub legs: u32,
    /// The minimap picture is the one being followed, at the known scale:
    /// worth drawing. Not while the track is broken (the big map or a dialog
    /// over the minimap) or a zoom change is being checked.
    pub picture_ok: bool,
}

const MAX_QUEUED: usize = 256;

/// Shared between the screen-reader thread and the capture thread.
#[derive(Debug, Default)]
pub struct MinimapLink {
    samples: Mutex<VecDeque<MinimapSample>>,
    view: Mutex<MinimapView>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl MinimapLink {
    pub fn new() -> MinimapLink {
        MinimapLink::default()
    }

    /// Queues a measurement (the oldest are dropped if nobody reads them).
    pub fn push(&self, s: MinimapSample) {
        let mut q = lock(&self.samples);
        if q.len() >= MAX_QUEUED {
            q.pop_front();
        }
        q.push_back(s);
    }

    pub fn drain(&self) -> Vec<MinimapSample> {
        lock(&self.samples).drain(..).collect()
    }

    pub fn set_view(&self, v: MinimapView) {
        *lock(&self.view) = v;
    }

    pub fn view(&self) -> MinimapView {
        lock(&self.view).clone()
    }
}
