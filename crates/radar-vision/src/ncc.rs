//! Masked normalised cross-correlation (Padfield, "Masked object
//! registration in the Fourier domain", 2012) between two frames, for every
//! shift up to `max_shift` pixels, via FFTs.
//!
//! Masked pixels (overlays that do not move with the map) take no part, and
//! the score is normalised over the overlap actually compared, so a shift
//! that only overlaps a few pixels cannot win by luck.

use std::sync::Arc;

use rustfft::num_complex::Complex64;
use rustfft::{Fft, FftPlanner};

use crate::plane::{bandpass, Mask, Plane};

/// Translation accepted when the NCC peak and its margin over the best score
/// more than 4 px away reach these.
pub const PEAK_MIN: f64 = 0.45;
pub const MARGIN_MIN: f64 = 0.04;
/// Minimum overlap for a shift, as a share of the smaller mask.
pub const OVERLAP_MIN: f64 = 0.3;

/// The smallest n' >= n whose only prime factors are 2, 3 and 5.
pub fn next_fast(mut n: usize) -> usize {
    loop {
        let mut m = n;
        for p in [2, 3, 5] {
            while m.is_multiple_of(p) {
                m /= p;
            }
        }
        if m == 1 {
            return n;
        }
        n += 1;
    }
}

/// 2-D complex FFT of a fixed size (rows then columns), unnormalised.
struct Fft2 {
    h: usize,
    w: usize,
    row_fwd: Arc<dyn Fft<f64>>,
    col_fwd: Arc<dyn Fft<f64>>,
    row_inv: Arc<dyn Fft<f64>>,
    col_inv: Arc<dyn Fft<f64>>,
}

impl Fft2 {
    fn new(h: usize, w: usize) -> Fft2 {
        let mut planner = FftPlanner::new();
        Fft2 {
            h,
            w,
            row_fwd: planner.plan_fft_forward(w),
            col_fwd: planner.plan_fft_forward(h),
            row_inv: planner.plan_fft_inverse(w),
            col_inv: planner.plan_fft_inverse(h),
        }
    }

    fn run(&self, data: &mut [Complex64], inverse: bool) {
        let (row, col) = if inverse {
            (&self.row_inv, &self.col_inv)
        } else {
            (&self.row_fwd, &self.col_fwd)
        };
        row.process(data); // every row of length w
        let mut column = vec![Complex64::new(0.0, 0.0); self.h];
        for x in 0..self.w {
            for y in 0..self.h {
                column[y] = data[y * self.w + x];
            }
            col.process(&mut column);
            for y in 0..self.h {
                data[y * self.w + x] = column[y];
            }
        }
    }
}

/// The spectra of one frame: mask, masked band-passed image and its square.
pub struct Spectra {
    m: Vec<Complex64>,
    g: Vec<Complex64>,
    g2: Vec<Complex64>,
    /// Number of usable pixels.
    pub count: f64,
}

/// A measured translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shift {
    /// The map content moved by (dx, dy) px from frame a to frame b.
    pub dx: f64,
    pub dy: f64,
    /// NCC at the peak (-1..1).
    pub peak: f64,
    /// Peak minus the best NCC more than 4 px away.
    pub margin: f64,
    /// Pixels compared at the peak.
    pub overlap: f64,
    /// Peak curvature in its flattest direction over that in its sharpest
    /// (0..1). Near 0 the peak is a ridge: a straight road or coast fills
    /// the view and movement along it cannot be measured.
    pub roundness: f64,
}

impl Shift {
    pub fn good(&self) -> bool {
        self.peak >= PEAK_MIN && self.margin >= MARGIN_MIN
    }
}

/// Per-size FFT plans and the NCC between spectra of that size.
pub struct Correlator {
    h: usize,
    w: usize,
    s: usize,
    ph: usize,
    pw: usize,
    fft: Fft2,
}

impl Correlator {
    /// For frames of `w`×`h` and shifts up to `max_shift` px either way.
    pub fn new(w: usize, h: usize, max_shift: usize) -> Correlator {
        let ph = next_fast(h + max_shift + 1);
        let pw = next_fast(w + max_shift + 1);
        Correlator {
            h,
            w,
            s: max_shift,
            ph,
            pw,
            fft: Fft2::new(ph, pw),
        }
    }

    pub fn max_shift(&self) -> usize {
        self.s
    }

    fn transform(&self, values: impl Fn(usize) -> f64) -> Vec<Complex64> {
        let mut buf = vec![Complex64::new(0.0, 0.0); self.ph * self.pw];
        for y in 0..self.h {
            for x in 0..self.w {
                buf[y * self.pw + x] = Complex64::new(values(y * self.w + x), 0.0);
            }
        }
        self.fft.run(&mut buf, false);
        buf
    }

    /// Spectra of a frame under its mask.
    pub fn spectra(&self, gray: &Plane, mask: &Mask) -> Spectra {
        debug_assert_eq!((gray.w, gray.h), (self.w, self.h));
        let bp = bandpass(gray, mask);
        let m = self.transform(|i| if mask.data[i] { 1.0 } else { 0.0 });
        let g = self.transform(|i| bp.data[i]);
        let g2 = self.transform(|i| bp.data[i] * bp.data[i]);
        Spectra {
            m,
            g,
            g2,
            count: mask.count() as f64,
        }
    }

    /// IFFT of `p + i q` where both IFFTs are real: returns (ifft p, ifft q)
    /// restricted to the shift window.
    fn pair(&self, p: &[Complex64], q: &[Complex64]) -> (Vec<f64>, Vec<f64>) {
        let mut buf: Vec<Complex64> = p
            .iter()
            .zip(q)
            .map(|(a, b)| a + Complex64::new(0.0, 1.0) * b)
            .collect();
        self.fft.run(&mut buf, true);
        let norm = 1.0 / (self.ph * self.pw) as f64;
        let side = 2 * self.s + 1;
        let (mut re, mut im) = (vec![0.0; side * side], vec![0.0; side * side]);
        for (j, dy) in (-(self.s as isize)..=self.s as isize).enumerate() {
            let y = dy.rem_euclid(self.ph as isize) as usize;
            for (i, dx) in (-(self.s as isize)..=self.s as isize).enumerate() {
                let x = dx.rem_euclid(self.pw as isize) as usize;
                let v = buf[y * self.pw + x] * norm;
                re[j * side + i] = v.re;
                im[j * side + i] = v.im;
            }
        }
        (re, im)
    }

    /// NCC for every shift in the window (row-major, (2s+1)²) and the
    /// overlap counts.
    pub fn ncc(&self, a: &Spectra, b: &Spectra) -> (Vec<f64>, Vec<f64>) {
        let conj_mul = |x: &[Complex64], y: &[Complex64]| -> Vec<Complex64> {
            x.iter().zip(y).map(|(u, v)| u.conj() * v).collect()
        };
        // Correlations Σ_x a(x)·b(x+d) for the six products.
        let (n, sa) = self.pair(&conj_mul(&a.m, &b.m), &conj_mul(&a.g, &b.m));
        let (sb, sab) = self.pair(&conj_mul(&a.m, &b.g), &conj_mul(&a.g, &b.g));
        let (saa, sbb) = self.pair(&conj_mul(&a.g2, &b.m), &conj_mul(&a.m, &b.g2));
        let min_overlap = OVERLAP_MIN * a.count.min(b.count);
        let mut out = vec![-1.0; n.len()];
        let mut counts = vec![0.0; n.len()];
        for i in 0..n.len() {
            let c = n[i].round();
            counts[i] = c;
            if c < min_overlap || c < 1.0 {
                continue;
            }
            let num = sab[i] - sa[i] * sb[i] / c;
            let den = ((saa[i] - sa[i] * sa[i] / c).max(0.0)
                * (sbb[i] - sb[i] * sb[i] / c).max(0.0))
            .sqrt();
            out[i] = if den > 1e-9 { num / den } else { 0.0 };
        }
        (out, counts)
    }

    /// The best translation from `a` to `b`, with a sub-pixel estimate.
    pub fn shift(&self, a: &Spectra, b: &Spectra) -> Shift {
        let (out, counts) = self.ncc(a, b);
        let side = 2 * self.s + 1;
        let k = out
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let (iy, ix) = (k / side, k % side);
        let peak = out[k];
        let at = |y: usize, x: usize| out[y * side + x];
        let (fx, fy, roundness) = if ix > 0 && ix + 1 < side && iy > 0 && iy + 1 < side {
            let mut n = [[0.0; 3]; 3];
            for (j, row) in n.iter_mut().enumerate() {
                for (i, v) in row.iter_mut().enumerate() {
                    *v = at(iy + j - 1, ix + i - 1);
                }
            }
            let (fx, fy) = refine_peak(&n);
            (fx, fy, roundness(&n))
        } else {
            (0.0, 0.0, 0.0)
        };
        let mut second = -1.0f64;
        for y in 0..side {
            for x in 0..side {
                let d2 = (y as isize - iy as isize).pow(2) + (x as isize - ix as isize).pow(2);
                if d2 > 16 {
                    second = second.max(at(y, x));
                }
            }
        }
        Shift {
            dx: ix as f64 - self.s as f64 + fx,
            dy: iy as f64 - self.s as f64 + fy,
            peak,
            margin: peak - second,
            overlap: counts[k],
            roundness,
        }
    }
}

/// Sub-pixel peak in a 3×3 neighbourhood (`n[y][x]`): parabolas along the
/// row and the column through the current estimate, alternately. Plain
/// row/column parabolas through the integer peak go wrong on tilted
/// (elongated) peaks, i.e. on diagonal sub-pixel shifts; a quadratic surface
/// over all nine points is biased on sharp peaks. This is neither.
fn refine_peak(n: &[[f64; 3]; 3]) -> (f64, f64) {
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let row_at = |fy: f64| -> [f64; 3] {
        let (j, t) = if fy >= 0.0 { (2, fy) } else { (0, -fy) };
        [
            lerp(n[1][0], n[j][0], t),
            lerp(n[1][1], n[j][1], t),
            lerp(n[1][2], n[j][2], t),
        ]
    };
    let col_at = |fx: f64| -> [f64; 3] {
        let (i, t) = if fx >= 0.0 { (2, fx) } else { (0, -fx) };
        [
            lerp(n[0][1], n[0][i], t),
            lerp(n[1][1], n[1][i], t),
            lerp(n[2][1], n[2][i], t),
        ]
    };
    let mut fy = parab(n[0][1], n[1][1], n[2][1]);
    let mut fx = 0.0;
    for _ in 0..3 {
        let r = row_at(fy);
        fx = parab(r[0], r[1], r[2]);
        let c = col_at(fx);
        fy = parab(c[0], c[1], c[2]);
    }
    (fx, fy)
}

/// Ratio of the weaker to the stronger principal curvature of the 3×3
/// neighbourhood around a peak (1 = round, 0 = ridge or not a peak).
fn roundness(n: &[[f64; 3]; 3]) -> f64 {
    let c = n[1][1];
    let dxx = n[1][0] - 2.0 * c + n[1][2];
    let dyy = n[0][1] - 2.0 * c + n[2][1];
    let dxy = (n[2][2] - n[2][0] - n[0][2] + n[0][0]) / 4.0;
    // Eigenvalues of [[dxx, dxy], [dxy, dyy]]; both negative at a peak.
    let mean = 0.5 * (dxx + dyy);
    let diff = (0.25 * (dxx - dyy).powi(2) + dxy * dxy).sqrt();
    let (weak, strong) = (mean + diff, mean - diff);
    if strong >= 0.0 || weak >= 0.0 {
        return 0.0;
    }
    weak / strong
}

/// Vertex of the parabola through (-1, m1), (0, c), (1, p1), clamped to ±0.5.
fn parab(m1: f64, c: f64, p1: f64) -> f64 {
    let den = m1 - 2.0 * c + p1;
    if den >= 0.0 {
        return 0.0;
    }
    (0.5 * (m1 - p1) / den).clamp(-0.5, 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grey view of a synthetic (non-periodic) map at an offset.
    fn texture(w: usize, h: usize, ox: f64, oy: f64) -> Plane {
        use std::sync::OnceLock;
        static MAP: OnceLock<crate::synth::SynthMap> = OnceLock::new();
        let map = MAP.get_or_init(|| crate::synth::SynthMap::new(400, 400, 11));
        crate::synth::view_gray(map, 200.0 + ox, 200.0 + oy, w, h)
    }

    #[test]
    fn next_fast_sizes() {
        assert_eq!(next_fast(134), 135);
        assert_eq!(next_fast(186), 192);
        assert_eq!(next_fast(7), 8);
    }

    #[test]
    fn recovers_integer_and_subpixel_shifts() {
        let (w, h) = (120, 90);
        let cor = Correlator::new(w, h, 20);
        let mask = Mask::full(w, h).erode(0);
        let a = cor.spectra(&texture(w, h, 0.0, 0.0), &mask);
        for (dx, dy) in [(5.0, -3.0), (-7.0, 11.0), (2.4, -1.7), (0.0, 0.0)] {
            // Content moved by (dx, dy): frame b at offset (-dx, -dy).
            let b = cor.spectra(&texture(w, h, -dx, -dy), &mask);
            let s = cor.shift(&a, &b);
            assert!(s.good(), "{s:?}");
            assert!(
                (s.dx - dx).abs() < 0.3 && (s.dy - dy).abs() < 0.3,
                "{dx} {dy}: {s:?}"
            );
        }
    }

    #[test]
    fn masked_overlay_does_not_pin_the_shift() {
        // A static bright square in both frames: masked, it must not win.
        let (w, h) = (120, 90);
        let cor = Correlator::new(w, h, 20);
        let mut mask = Mask::full(w, h);
        let mut a = texture(w, h, 0.0, 0.0);
        let mut b = texture(w, h, -6.0, -4.0);
        for y in 30..60 {
            for x in 40..80 {
                a.data[y * w + x] = 255.0;
                b.data[y * w + x] = 255.0;
                mask.data[y * w + x] = false;
            }
        }
        let s = cor.shift(&cor.spectra(&a, &mask), &cor.spectra(&b, &mask));
        assert!(
            (s.dx - 6.0).abs() < 0.3 && (s.dy - 4.0).abs() < 0.3,
            "{s:?}"
        );
    }
}
