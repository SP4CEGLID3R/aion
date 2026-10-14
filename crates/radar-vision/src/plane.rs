//! Small image planes and the filters the tracker needs.
//!
//! Screen x grows to the right, y downwards. Frames arrive as 32-bit BGRA/BGRX
//! rows (what GDI hands out); everything after that works on `f64` planes.

/// A single-channel image.
#[derive(Debug, Clone, PartialEq)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f64>,
}

/// A yes/no image: `true` = usable pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<bool>,
}

/// A borrowed 32-bit frame: B, G, R, X per pixel, rows `stride` bytes apart.
#[derive(Debug, Clone, Copy)]
pub struct Bgra<'a> {
    pub w: usize,
    pub h: usize,
    pub stride: usize,
    pub data: &'a [u8],
}

impl<'a> Bgra<'a> {
    /// A tightly packed frame (`stride = 4 * w`). `None` if `data` is too short.
    pub fn packed(w: usize, h: usize, data: &'a [u8]) -> Option<Bgra<'a>> {
        (data.len() >= w * h * 4).then_some(Bgra {
            w,
            h,
            stride: w * 4,
            data,
        })
    }

    /// (r, g, b) at (x, y).
    #[inline]
    pub fn rgb(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let i = y * self.stride + x * 4;
        (self.data[i + 2], self.data[i + 1], self.data[i])
    }

    /// Luma (0.299 R + 0.587 G + 0.114 B), full resolution.
    pub fn gray(&self) -> Plane {
        let mut data = Vec::with_capacity(self.w * self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                let (r, g, b) = self.rgb(x, y);
                data.push(r as f64 * 0.299 + g as f64 * 0.587 + b as f64 * 0.114);
            }
        }
        Plane {
            w: self.w,
            h: self.h,
            data,
        }
    }

    /// Mean and standard deviation of every channel on a 4-pixel grid
    /// (cheap; enough to tell a black or blank capture).
    pub fn stats(&self) -> (f64, f64) {
        let (mut n, mut s, mut s2) = (0.0, 0.0, 0.0);
        for y in (0..self.h).step_by(4) {
            for x in (0..self.w).step_by(4) {
                let (r, g, b) = self.rgb(x, y);
                for v in [r, g, b] {
                    let v = v as f64;
                    n += 1.0;
                    s += v;
                    s2 += v * v;
                }
            }
        }
        if n == 0.0 {
            return (0.0, 0.0);
        }
        let mean = s / n;
        (mean, (s2 / n - mean * mean).max(0.0).sqrt())
    }

    /// A cheap fingerprint of the pixels, to notice frames that never change.
    pub fn fingerprint(&self) -> u64 {
        // FNV-1a over a sparse grid of pixels.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for y in (0..self.h).step_by(3) {
            for x in (0..self.w).step_by(3) {
                let (r, g, b) = self.rgb(x, y);
                for v in [r, g, b] {
                    hash ^= v as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
        hash
    }
}

impl Plane {
    pub fn new(w: usize, h: usize) -> Plane {
        Plane {
            w,
            h,
            data: vec![0.0; w * h],
        }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.w + x]
    }

    /// 2x2 average (odd last row/column dropped).
    pub fn half(&self) -> Plane {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut out = Plane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (x2, y2) = (x * 2, y * 2);
                out.data[y * w + x] = (self.at(x2, y2)
                    + self.at(x2 + 1, y2)
                    + self.at(x2, y2 + 1)
                    + self.at(x2 + 1, y2 + 1))
                    * 0.25;
            }
        }
        out
    }

    /// Mean over a `k`×`k` box (k odd), same size, edges repeated.
    pub fn box_blur(&self, k: usize) -> Plane {
        if k <= 1 {
            return self.clone();
        }
        let r = (k / 2) as isize;
        let (w, h) = (self.w, self.h);
        let mut tmp = Plane::new(w, h);
        let mut line = Vec::with_capacity(w.max(h) + k);
        // Rows: running sum over a clamped window.
        for y in 0..h {
            line.clear();
            for i in -r..(w as isize + r) {
                line.push(self.at(i.clamp(0, w as isize - 1) as usize, y));
            }
            let mut sum: f64 = line[..k].iter().sum();
            for x in 0..w {
                tmp.data[y * w + x] = sum;
                if x + k < line.len() {
                    sum += line[x + k] - line[x];
                }
            }
        }
        let mut out = Plane::new(w, h);
        let norm = 1.0 / (k * k) as f64;
        for x in 0..w {
            line.clear();
            for i in -r..(h as isize + r) {
                line.push(tmp.at(x, i.clamp(0, h as isize - 1) as usize));
            }
            let mut sum: f64 = line[..k].iter().sum();
            for y in 0..h {
                out.data[y * w + x] = sum * norm;
                if y + k < line.len() {
                    sum += line[y + k] - line[y];
                }
            }
        }
        out
    }

    /// Bilinear sample; `None` outside the image.
    pub fn sample(&self, x: f64, y: f64) -> Option<f64> {
        if !(x >= 0.0 && y >= 0.0 && x <= (self.w - 1) as f64 && y <= (self.h - 1) as f64) {
            return None;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let top = self.at(x0, y0) * (1.0 - fx) + self.at(x1, y0) * fx;
        let bottom = self.at(x0, y1) * (1.0 - fx) + self.at(x1, y1) * fx;
        Some(top * (1.0 - fy) + bottom * fy)
    }
}

/// Difference of box blurs (3 minus 13): keeps map detail, drops slow
/// shading such as the view cone's tint and vignetting. With a mask it is a
/// normalised convolution, so masked overlays do not bleed into the pixels
/// around them. The result is zero outside the mask.
pub fn bandpass(gray: &Plane, mask: &Mask) -> Plane {
    const BAND: (usize, usize) = (3, 13);
    let m = Plane {
        w: mask.w,
        h: mask.h,
        data: mask
            .data
            .iter()
            .map(|&b| if b { 1.0 } else { 0.0 })
            .collect(),
    };
    let gm = Plane {
        w: gray.w,
        h: gray.h,
        data: gray.data.iter().zip(&m.data).map(|(g, m)| g * m).collect(),
    };
    let (g1, m1) = (gm.box_blur(BAND.0), m.box_blur(BAND.0));
    let (g2, m2) = (gm.box_blur(BAND.1), m.box_blur(BAND.1));
    let mut out = Plane::new(gray.w, gray.h);
    for i in 0..out.data.len() {
        if mask.data[i] {
            out.data[i] = g1.data[i] / m1.data[i].max(1e-3) - g2.data[i] / m2.data[i].max(1e-3);
        }
    }
    out
}

impl Mask {
    pub fn full(w: usize, h: usize) -> Mask {
        Mask {
            w,
            h,
            data: vec![true; w * h],
        }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> bool {
        self.data[y * self.w + x]
    }

    pub fn and(&self, other: &Mask) -> Mask {
        Mask {
            w: self.w,
            h: self.h,
            data: self
                .data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| *a && *b)
                .collect(),
        }
    }

    pub fn and_not(&self, other: &Mask) -> Mask {
        Mask {
            w: self.w,
            h: self.h,
            data: self
                .data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| *a && !*b)
                .collect(),
        }
    }

    pub fn count(&self) -> usize {
        self.data.iter().filter(|b| **b).count()
    }

    pub fn fraction(&self) -> f64 {
        self.count() as f64 / self.data.len().max(1) as f64
    }

    /// `true` only where the whole (2r+1)² neighbourhood is `true` (edges
    /// repeated).
    pub fn erode(&self, r: usize) -> Mask {
        self.box_count(r, |c, full| c == full)
    }

    /// `true` where any pixel of the (2r+1)² neighbourhood is `true`.
    pub fn dilate(&self, r: usize) -> Mask {
        self.box_count(r, |c, _| c > 0)
    }

    fn box_count(&self, r: usize, keep: impl Fn(usize, usize) -> bool) -> Mask {
        if r == 0 {
            return self.clone();
        }
        let k = 2 * r + 1;
        let p = Plane {
            w: self.w,
            h: self.h,
            data: self
                .data
                .iter()
                .map(|&b| if b { 1.0 } else { 0.0 })
                .collect(),
        }
        .box_blur(k);
        let full = k * k;
        Mask {
            w: self.w,
            h: self.h,
            data: p
                .data
                .iter()
                .map(|v| keep((v * full as f64).round() as usize, full))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_blur_matches_a_direct_mean() {
        let mut p = Plane::new(7, 5);
        for (i, v) in p.data.iter_mut().enumerate() {
            *v = ((i * 37) % 11) as f64;
        }
        let b = p.box_blur(3);
        // An interior pixel: plain 3x3 mean.
        let mut s = 0.0;
        for y in 1..=3 {
            for x in 2..=4 {
                s += p.at(x, y);
            }
        }
        assert!((b.at(3, 2) - s / 9.0).abs() < 1e-9);
        // A corner: edges repeated.
        let c = (p.at(0, 0) * 4.0 + p.at(1, 0) * 2.0 + p.at(0, 1) * 2.0 + p.at(1, 1)) / 9.0;
        assert!((b.at(0, 0) - c).abs() < 1e-9);
    }

    #[test]
    fn erode_and_dilate() {
        let mut m = Mask::full(9, 9);
        m.data[4 * 9 + 4] = false;
        let e = m.erode(1);
        assert!(!e.at(3, 3) && !e.at(5, 5) && e.at(1, 1));
        let mut d = Mask {
            w: 9,
            h: 9,
            data: vec![false; 81],
        };
        d.data[4 * 9 + 4] = true;
        let dd = d.dilate(2);
        assert!(dd.at(2, 2) && dd.at(6, 6) && !dd.at(1, 4));
    }

    #[test]
    fn bandpass_ignores_a_flat_tint() {
        let mut g = Plane::new(40, 30);
        for y in 0..30 {
            for x in 0..40 {
                // A smooth ramp: almost nothing passes the band.
                g.data[y * 40 + x] = 100.0 + x as f64 * 0.5;
            }
        }
        let bp = bandpass(&g, &Mask::full(40, 30));
        let inner = bp.data[15 * 40 + 20];
        assert!(inner.abs() < 1e-6, "{inner}");
    }
}
