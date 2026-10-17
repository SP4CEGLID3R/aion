//! Reads the in-game minimap from the screen on its own thread: each frame
//! goes through the tracker (map movement, camera direction) and the result
//! to the capture decoder through a [`MinimapLink`]. The latest picture is
//! kept for the radar background and the settings preview.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use radar_capture::minimap::{MinimapLink, MinimapSample};
use radar_core::ScreenRegion;
use radar_ingest::clock::now_ms;
use radar_vision::capture::bgrx_to_rgba;
use radar_vision::{Bgra, MinimapTracker, RegionGrabber, ScreenRect, TrackerConfig, RIDGE};
use serde::Serialize;

/// The minimap's map area with the default HUD at 2560×1440: anchored to the
/// top right corner, scaled with the screen height (measured by
/// madara134/aion2-cube-finder and NoRhythmDEV/aion2-map-overlay).
const REF_HEIGHT: f64 = 1440.0;
const REF_FROM_RIGHT: f64 = 2560.0 - 2221.0;
const REF_TOP: f64 = 126.0;
const REF_SIZE: (f64, f64) = (311.0, 206.0);

/// Where the minimap sits with the default HUD on a monitor at (x, y) of
/// `w`×`h` physical pixels.
pub fn default_region(x: i32, y: i32, w: u32, h: u32) -> ScreenRegion {
    let s = h as f64 / REF_HEIGHT;
    ScreenRegion {
        x: x + (w as f64 - REF_FROM_RIGHT * s).round() as i32,
        y: y + (REF_TOP * s).round() as i32,
        width: (REF_SIZE.0 * s).round() as u32,
        height: (REF_SIZE.1 * s).round() as u32,
    }
}

/// Size of the game's UI on a monitor `h` pixels high, relative to 1440p.
pub fn ui_scale(monitor_height: u32) -> f64 {
    (monitor_height as f64 / REF_HEIGHT).clamp(0.4, 3.0)
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionStatus {
    pub running: bool,
    pub region: Option<ScreenRegion>,
    /// The region is the default spot, not one the user picked.
    pub default_region: bool,
    pub ui_scale: f64,
    pub frames: u64,
    /// Time of the last screen copy and of the tracking, milliseconds.
    pub grab_ms: f64,
    pub track_ms: f64,
    pub black: bool,
    pub frozen: bool,
    pub breaks: u64,
    /// The view cone (camera direction) is visible.
    pub cone_clear: bool,
    pub error: Option<String>,
}

pub struct LatestFrame {
    pub seq: u64,
    pub w: u32,
    pub h: u32,
    pub pivot: (f32, f32),
    /// The frame shows the minimap (its arrow disk), not something over it.
    pub disk_ok: bool,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct Shared {
    pub status: Mutex<VisionStatus>,
    pub latest: Mutex<Option<LatestFrame>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Shared {
    pub fn status(&self) -> VisionStatus {
        lock(&self.status).clone()
    }

    /// The latest picture as `[seq u64][w u32][h u32][pivot x f32][pivot y
    /// f32][px per m f32][flags u32]` + RGBA, if newer than `after`.
    pub fn frame_after(&self, after: u64, px_per_m: Option<f64>, calibrated: bool) -> Vec<u8> {
        let latest = lock(&self.latest);
        let Some(f) = latest.as_ref().filter(|f| f.seq > after) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(32 + f.rgba.len());
        out.extend(f.seq.to_le_bytes());
        out.extend(f.w.to_le_bytes());
        out.extend(f.h.to_le_bytes());
        out.extend(f.pivot.0.to_le_bytes());
        out.extend(f.pivot.1.to_le_bytes());
        // No scale = not drawn: only the minimap itself is drawn behind the radar.
        let px_per_m = px_per_m.filter(|_| f.disk_ok);
        out.extend((px_per_m.unwrap_or(0.0) as f32).to_le_bytes());
        out.extend(u32::from(calibrated).to_le_bytes());
        out.extend(&f.rgba);
        out
    }
}

pub struct MinimapService {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub shared: Arc<Shared>,
    pub link: Arc<MinimapLink>,
}

impl MinimapService {
    pub fn start(
        region: ScreenRegion,
        default_region: bool,
        ui_scale: f64,
        fps: u32,
        link: Arc<MinimapLink>,
    ) -> MinimapService {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared::default());
        {
            let mut st = lock(&shared.status);
            st.region = Some(region);
            st.default_region = default_region;
            st.ui_scale = ui_scale;
        }
        let thread = {
            let (stop, shared, link) = (stop.clone(), shared.clone(), link.clone());
            std::thread::Builder::new()
                .name("minimap".into())
                .spawn(move || run(region, ui_scale, fps, &stop, &shared, &link))
                .ok()
        };
        MinimapService {
            stop,
            thread,
            shared,
            link,
        }
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for MinimapService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    region: ScreenRegion,
    ui_scale: f64,
    fps: u32,
    stop: &AtomicBool,
    shared: &Shared,
    link: &MinimapLink,
) {
    let rect = ScreenRect {
        x: region.x,
        y: region.y,
        width: region.width,
        height: region.height,
    };
    let mut grabber = match RegionGrabber::new(rect) {
        Ok(g) => g,
        Err(e) => {
            log::warn!("minimap capture unavailable: {e}");
            lock(&shared.status).error = Some(e.to_string());
            return;
        }
    };
    let (w, h) = (region.width as usize, region.height as usize);
    let mut tracker = MinimapTracker::new(
        w,
        h,
        TrackerConfig {
            ui_scale,
            locate_pivot: true,
            ..TrackerConfig::default()
        },
    );
    let period = Duration::from_millis(1000 / u64::from(fps.max(1)));
    let mut buf = Vec::new();
    let mut seq = 0u64;
    let mut disk_ok = true;
    lock(&shared.status).running = true;
    log::info!(
        "minimap capture: {}×{} at ({}, {}), {fps}/s",
        w,
        h,
        region.x,
        region.y
    );
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        if let Err(e) = grabber.grab(&mut buf) {
            lock(&shared.status).error = Some(e.to_string());
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        let grab_ms = started.elapsed().as_secs_f64() * 1000.0;
        let t = now_ms();
        let Some(frame) = Bgra::packed(w, h, &buf) else {
            continue;
        };
        let step = tracker.step(t, &frame);
        if step.disk_ok != disk_ok {
            disk_ok = step.disk_ok;
            if disk_ok {
                log::info!("minimap picture: the minimap is back, drawn in the radar again");
            } else {
                log::info!(
                    "minimap picture: something else is on the minimap's spot (big map, dialog?), not drawn"
                );
            }
        }
        let pivot = tracker.pivot();
        if step.recentred {
            log::info!(
                "minimap: player arrow found at ({:.0}, {:.0}), {:+.0}/{:+.0} px from the centre of the rectangle",
                pivot.0,
                pivot.1,
                pivot.0 - w as f64 / 2.0,
                pivot.1 - h as f64 / 2.0
            );
        }
        link.push(MinimapSample {
            t,
            x: step.x,
            y: step.y,
            seg: step.seg,
            good: step.good,
            brk: step.brk,
            black: step.black,
            frozen: step.frozen,
            peak: step.peak,
            roundness: step.roundness,
            ridge: RIDGE,
            cone: step.cone,
            cone_strength: step.cone_strength,
            ring_r: step.ring_r,
            recentred: step.recentred,
            ui_scale,
        });
        let mut rgba = buf.clone();
        bgrx_to_rgba(&mut rgba);
        seq += 1;
        *lock(&shared.latest) = Some(LatestFrame {
            seq,
            w: w as u32,
            h: h as u32,
            pivot: (pivot.0 as f32, pivot.1 as f32),
            disk_ok: step.disk_ok,
            rgba,
        });
        {
            let stats = tracker.stats();
            let mut st = lock(&shared.status);
            st.frames = stats.frames;
            st.grab_ms = grab_ms;
            st.track_ms = stats.last_ms;
            st.black = step.black;
            st.frozen = step.frozen;
            st.breaks = stats.breaks;
            st.cone_clear = step.cone_strength >= 0.3;
            st.error = None;
        }
        if let Some(rest) = period.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    lock(&shared.status).running = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spot_matches_the_measured_hud() {
        // 2560×1440: the reference.
        assert_eq!(
            default_region(0, 0, 2560, 1440),
            ScreenRegion {
                x: 2221,
                y: 126,
                width: 311,
                height: 206
            }
        );
        // 1920×1080 on a second monitor to the right.
        let r = default_region(2560, 0, 1920, 1080);
        assert_eq!((r.x, r.y, r.width, r.height), (2560 + 1666, 95, 233, 155));
        // Ultrawide: anchored to the right edge.
        let r = default_region(0, 0, 3440, 1440);
        assert_eq!(r.x, 3440 - 339);
        assert!((ui_scale(1080) - 0.75).abs() < 1e-9);
    }
}
