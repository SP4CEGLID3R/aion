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
            let _ = rect;
            Err(CaptureError::Unsupported)
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
            let _ = out;
            Err(CaptureError::Unsupported)
        }
    }
}

/// A one-off capture of any rectangle (for picking the minimap), BGRX rows.
pub fn grab_once(rect: ScreenRect) -> Result<Vec<u8>, CaptureError> {
    let mut g = RegionGrabber::new(rect)?;
    let mut out = Vec::new();
    g.grab(&mut out)?;
    Ok(out)
}

/// BGRX → RGBA in place (alpha 255), for showing a capture in a canvas.
pub fn bgrx_to_rgba(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
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
