//! Reading a rectangle of the screen, like a screenshot tool does.
//!
//! On Windows this copies the desktop as the compositor shows it (GDI
//! `BitBlt` from the screen DC): no handle to the game, no hooks, nothing
//! injected. Coordinates are physical pixels of the virtual screen (the app
//! is per-monitor DPI aware). Windows marked with `WDA_EXCLUDEFROMCAPTURE`,
//! like the radar itself, do not appear in the copy. Exclusive fullscreen is
//! not supported (frames come back black); use borderless or windowed mode.
//!
//! Elsewhere every call returns [`CaptureError::Unsupported`].

use std::fmt;

/// A rectangle in physical screen pixels (virtual screen; may be negative
/// on multi-monitor setups).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureError {
    Unsupported,
    Failed(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::Unsupported => {
                write!(f, "screen capture is only available on Windows")
            }
            CaptureError::Failed(e) => write!(f, "screen capture failed: {e}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Repeated captures of one rectangle, reusing the same buffers.
pub struct RegionGrabber {
    rect: ScreenRect,
    #[cfg(windows)]
    inner: gdi::Grabber,
    #[cfg(not(windows))]
    inner: dev_synth::Synth,
}

impl RegionGrabber {
    pub fn new(rect: ScreenRect) -> Result<RegionGrabber, CaptureError> {
        if rect.width == 0 || rect.height == 0 || rect.width > 4096 || rect.height > 4096 {
            return Err(CaptureError::Failed(format!(
                "invalid region {}×{}",
                rect.width, rect.height
            )));
        }
        #[cfg(windows)]
        {
            Ok(RegionGrabber {
                rect,
                inner: gdi::Grabber::new(rect.width as i32, rect.height as i32)?,
            })
        }
        #[cfg(not(windows))]
        {
            dev_synth::Synth::new(rect)
                .map(|inner| RegionGrabber { rect, inner })
                .ok_or(CaptureError::Unsupported)
        }
    }

    pub fn rect(&self) -> ScreenRect {
        self.rect
    }

    /// The region as BGRX rows (4 bytes per pixel, stride = 4 × width).
    pub fn grab(&mut self, out: &mut Vec<u8>) -> Result<(), CaptureError> {
        #[cfg(windows)]
        {
            self.inner.grab(self.rect.x, self.rect.y, out)
        }
        #[cfg(not(windows))]
        {
            self.inner.grab(out);
            Ok(())
        }
    }
}

/// A one-off capture of any rectangle (for picking the minimap), BGRX rows.
pub fn grab_once(rect: ScreenRect) -> Result<Vec<u8>, CaptureError> {
    #[cfg(not(windows))]
    if let Some(desk) = dev_synth::desktop(rect) {
        return Ok(desk);
    }
    let mut g = RegionGrabber::new(rect)?;
    let mut out = Vec::new();
    g.grab(&mut out)?;
    Ok(out)
}

/// BGRX → RGBA in place (alpha 255), for showing a capture in a canvas.
pub fn bgrx_to_rgba(buf: &mut [u8]) {
    for px in buf.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = 255;
    }
}

#[cfg(windows)]
mod gdi {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
        HDC, HGDIOBJ, SRCCOPY,
    };

    use super::CaptureError;

    fn err(what: &str) -> CaptureError {
        CaptureError::Failed(format!("{what}: {}", windows::core::Error::from_thread()))
    }

    /// A memory DC with a top-down 32-bit DIB section the screen is copied
    /// into. Create, use and drop it on one thread.
    pub struct Grabber {
        mem_dc: HDC,
        dib: HBITMAP,
        old: HGDIOBJ,
        bits: *const u8,
        width: i32,
        height: i32,
    }

    impl Grabber {
        pub fn new(width: i32, height: i32) -> Result<Grabber, CaptureError> {
            unsafe {
                let screen = GetDC(None);
                if screen.is_invalid() {
                    return Err(err("GetDC"));
                }
                let mem_dc = CreateCompatibleDC(Some(screen));
                ReleaseDC(None, screen);
                if mem_dc.is_invalid() {
                    return Err(err("CreateCompatibleDC"));
                }
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height, // negative: top-down rows
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits: *mut c_void = null_mut();
                let dib = match CreateDIBSection(
                    Some(mem_dc),
                    &bmi,
                    DIB_RGB_COLORS,
                    &mut bits,
                    None,
                    0,
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = DeleteDC(mem_dc);
                        return Err(CaptureError::Failed(format!("CreateDIBSection: {e}")));
                    }
                };
                let old = SelectObject(mem_dc, dib.into());
                Ok(Grabber {
                    mem_dc,
                    dib,
                    old,
                    bits: bits as *const u8,
                    width,
                    height,
                })
            }
        }

        pub fn grab(&mut self, left: i32, top: i32, out: &mut Vec<u8>) -> Result<(), CaptureError> {
            unsafe {
                let screen = GetDC(None);
                if screen.is_invalid() {
                    return Err(err("GetDC"));
                }
                let r = BitBlt(
                    self.mem_dc,
                    0,
                    0,
                    self.width,
                    self.height,
                    Some(screen),
                    left,
                    top,
                    SRCCOPY,
                );
                ReleaseDC(None, screen);
                r.map_err(|e| CaptureError::Failed(format!("BitBlt: {e}")))?;
                let _ = GdiFlush(); // finish GDI work before reading the bits
                let len = self.width as usize * self.height as usize * 4;
                out.clear();
                out.extend_from_slice(std::slice::from_raw_parts(self.bits, len));
            }
            Ok(())
        }
    }

    impl Drop for Grabber {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.mem_dc, self.old);
                let _ = DeleteObject(self.dib.into());
                let _ = DeleteDC(self.mem_dc);
            }
        }
    }
}

/// Development only (never on Windows): with `RADAR_VISION_SYNTH` set to the
/// unix time (ms) a synthetic game session started, captures show a
/// synthetic minimap following that session's walk
/// (`radar_capture::synthetic::aion2_layout::player`), so the whole app can be
/// tried end to end without the game. Without the variable, capture is
/// unsupported here.
#[cfg(not(windows))]
mod dev_synth {
    use super::ScreenRect;
    use crate::synth::{frame, Overlay, SynthMap};

    /// The synthetic session's walk, game units (cm) at `rel` ms.
    fn player(rel: f64) -> (f64, f64) {
        let s = rel / 1000.0;
        (
            -45_000.0 + (s * 0.05).sin() * 3_000.0,
            120_000.0 + (s * 0.04).cos() * 3_000.0,
        )
    }

    fn t0() -> Option<f64> {
        std::env::var("RADAR_VISION_SYNTH").ok()?.parse().ok()
    }

    fn now() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }

    pub struct Synth {
        map: SynthMap,
        w: usize,
        h: usize,
        t0: f64,
        /// Minimap px per game unit (seed scale for this region size).
        k: f64,
        ui: f64,
    }

    impl Synth {
        pub fn new(rect: ScreenRect) -> Option<Synth> {
            let t0 = t0()?;
            let ui = rect.height as f64 / 206.0;
            Some(Synth {
                map: SynthMap::new(1400, 1200, 5),
                w: rect.width as usize,
                h: rect.height as usize,
                t0,
                k: 0.0103 * ui,
                ui,
            })
        }

        pub fn grab(&mut self, out: &mut Vec<u8>) {
            let rel = (now() - self.t0).max(0.0);
            let (x, y) = player(rel);
            let (x0, y0) = player(0.0);
            // Right = +X, down = +Y, as on the game's minimap.
            let cx = 700.0 + (x - x0) * self.k;
            let cy = 600.0 + (y - y0) * self.k;
            let s = rel / 1000.0;
            let ov = Overlay {
                cone: (30.0 + 50.0 * (s / 6.0).sin()).rem_euclid(360.0),
                facing: (60.0 + 40.0 * (s / 4.0).sin()).rem_euclid(360.0),
                ring_r: 67.0,
                dots: vec![(self.w as f64 * 0.3 + 15.0 * s.sin(), self.h as f64 * 0.7)],
                ui_scale: self.ui,
                // Not quite centred, like a minimap rectangle that is a few
                // pixels off on a real screen.
                pivot: Some((self.w as f64 / 2.0 + 7.0, self.h as f64 / 2.0 - 5.0)),
                disk_dim: 0.0,
            };
            *out = frame(&self.map, cx, cy, self.w, self.h, &ov);
        }
    }

    /// A synthetic desktop with the minimap at its default spot.
    pub fn desktop(rect: ScreenRect) -> Option<Vec<u8>> {
        t0()?;
        let (w, h) = (rect.width as usize, rect.height as usize);
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let v = (30 + (x * 40 / w.max(1)) + (y * 30 / h.max(1))) as u8;
                out[i..i + 4].copy_from_slice(&[v + 10, v, v / 2 + 20, 255]);
            }
        }
        let s = h as f64 / 1440.0;
        let mw = (311.0 * s).round() as usize;
        let mh = (206.0 * s).round() as usize;
        let mx = (w as f64 - 339.0 * s).round() as usize;
        let my = (126.0 * s).round() as usize;
        let mut g = Synth::new(ScreenRect {
            x: 0,
            y: 0,
            width: mw as u32,
            height: mh as u32,
        })?;
        let mut mini = Vec::new();
        g.grab(&mut mini);
        for y in 0..mh.min(h.saturating_sub(my)) {
            for x in 0..mw.min(w.saturating_sub(mx)) {
                let (si, di) = ((y * mw + x) * 4, ((my + y) * w + mx + x) * 4);
                out[di..di + 4].copy_from_slice(&mini[si..si + 4]);
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_and_errors() {
        let mut px = vec![1, 2, 3, 0, 10, 20, 30, 0];
        bgrx_to_rgba(&mut px);
        assert_eq!(px, vec![3, 2, 1, 255, 30, 20, 10, 255]);
        assert!(RegionGrabber::new(ScreenRect {
            x: 0,
            y: 0,
            width: 0,
            height: 10
        })
        .is_err());
        #[cfg(not(windows))]
        assert_eq!(
            grab_once(ScreenRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10
            }),
            Err(CaptureError::Unsupported)
        );
    }
}
