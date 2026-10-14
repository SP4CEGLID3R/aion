//! Synthetic minimap frames for tests and demos: a procedural map texture
//! seen through a window that moves with the player, with the minimap's
//! screen-fixed overlays drawn on top (view cone, range ring, arrow disk,
//! HUD icons) and dots that move on their own. Nothing here comes from the
//! game.

/// An RGB texture.
pub struct SynthMap {
    pub w: usize,
    pub h: usize,
    rgb: Vec<[f32; 3]>,
}

/// Deterministic pseudo-random numbers (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise on a `cell`-pixel lattice.
fn value_noise(w: usize, h: usize, cell: usize, rng: &mut Rng) -> Vec<f64> {
    let (gw, gh) = (w / cell + 2, h / cell + 2);
    let grid: Vec<f64> = (0..gw * gh).map(|_| rng.next()).collect();
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        let (gy, fy) = (y / cell, smooth((y % cell) as f64 / cell as f64));
        for x in 0..w {
            let (gx, fx) = (x / cell, smooth((x % cell) as f64 / cell as f64));
            let g = |i: usize, j: usize| grid[j * gw + i];
            let top = g(gx, gy) * (1.0 - fx) + g(gx + 1, gy) * fx;
            let bot = g(gx, gy + 1) * (1.0 - fx) + g(gx + 1, gy + 1) * fx;
            out[y * w + x] = top * (1.0 - fy) + bot * fy;
        }
    }
    out
}

impl SynthMap {
    /// A map-like texture: forest/grass/rock colours from layered noise,
    /// with a few roads and a river.
    pub fn new(w: usize, h: usize, seed: u64) -> SynthMap {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let layers: Vec<(Vec<f64>, f64)> = [(64, 0.45), (24, 0.3), (9, 0.17), (4, 0.08)]
            .iter()
            .map(|(cell, wt)| (value_noise(w, h, *cell, &mut rng), *wt))
            .collect();
        let moisture = value_noise(w, h, 90, &mut rng);
        let mut rgb = vec![[0.0f32; 3]; w * h];
        for i in 0..w * h {
            let v: f64 = layers.iter().map(|(l, wt)| l[i] * wt).sum();
            let m = moisture[i];
            let forest = [70.0 + 60.0 * v, 95.0 + 70.0 * v, 50.0 + 30.0 * v];
            let field = [150.0 + 70.0 * v, 140.0 + 60.0 * v, 80.0 + 40.0 * v];
            let c: Vec<f64> = (0..3)
                .map(|k| forest[k] * m + field[k] * (1.0 - m))
                .collect();
            rgb[i] = [c[0] as f32, c[1] as f32, c[2] as f32];
        }
        let mut map = SynthMap { w, h, rgb };
        // Roads: a few straight segments; a winding river.
        for _ in 0..6 {
            let (x0, y0) = (rng.next() * w as f64, rng.next() * h as f64);
            let (x1, y1) = (rng.next() * w as f64, rng.next() * h as f64);
            map.line(x0, y0, x1, y1, 2.5, [205.0, 185.0, 140.0]);
        }
        let mut y = rng.next() * h as f64;
        let mut x = 0.0;
        while x < w as f64 {
            let ny = y + (rng.next() - 0.5) * 30.0;
            map.line(x, y, x + 25.0, ny, 4.0, [60.0, 110.0, 150.0]);
            x += 25.0;
            y = ny;
        }
        map
    }

    fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, width: f64, c: [f64; 3]) {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = (dx * dx + dy * dy).max(1e-9);
        let (xa, xb) = (x0.min(x1) - width - 1.0, x0.max(x1) + width + 1.0);
        let (ya, yb) = (y0.min(y1) - width - 1.0, y0.max(y1) + width + 1.0);
        for y in (ya.max(0.0) as usize)..(yb.min(self.h as f64) as usize) {
            for x in (xa.max(0.0) as usize)..(xb.min(self.w as f64) as usize) {
                let t = (((x as f64 - x0) * dx + (y as f64 - y0) * dy) / len2).clamp(0.0, 1.0);
                let d = (x as f64 - (x0 + t * dx)).hypot(y as f64 - (y0 + t * dy));
                if d < width {
                    let a = (1.0 - (d / width).powi(2)).clamp(0.0, 1.0) as f32;
                    let p = &mut self.rgb[y * self.w + x];
                    for k in 0..3 {
                        p[k] = p[k] * (1.0 - a) + c[k] as f32 * a;
                    }
                }
            }
        }
    }

    fn sample(&self, x: f64, y: f64) -> [f64; 3] {
        let x = x.clamp(0.0, (self.w - 2) as f64);
        let y = y.clamp(0.0, (self.h - 2) as f64);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let p = |i: usize, j: usize| self.rgb[j * self.w + i];
        let mut out = [0.0; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let top = p(x0, y0)[k] as f64 * (1.0 - fx) + p(x0 + 1, y0)[k] as f64 * fx;
            let bot = p(x0, y0 + 1)[k] as f64 * (1.0 - fx) + p(x0 + 1, y0 + 1)[k] as f64 * fx;
            *o = top * (1.0 - fy) + bot * fy;
        }
        out
    }
}

/// The bare map (no overlays) around (`cx`, `cy`), grey.
pub fn view_gray(map: &SynthMap, cx: f64, cy: f64, w: usize, h: usize) -> crate::plane::Plane {
    let mut p = crate::plane::Plane::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = map.sample(
                cx + x as f64 - w as f64 / 2.0,
                cy + y as f64 - h as f64 / 2.0,
            );
            p.data[y * w + x] = c[0] * 0.299 + c[1] * 0.587 + c[2] * 0.114;
        }
    }
    p
}

/// What to draw on top of the map in one frame.
#[derive(Debug, Clone)]
pub struct Overlay {
    /// Camera (view cone) direction, screen compass degrees.
    pub cone: f64,
    /// Character facing (arrow), screen compass degrees.
    pub facing: f64,
    pub ring_r: f64,
    /// Independent movers, in frame pixels.
    pub dots: Vec<(f64, f64)>,
    /// Size of the arrow disk etc. (1.0 = default UI at 1440p).
    pub ui_scale: f64,
}

impl Default for Overlay {
    fn default() -> Self {
        Overlay {
            cone: 0.0,
            facing: 0.0,
            ring_r: 67.0,
            dots: Vec::new(),
            ui_scale: 1.0,
        }
    }
}

/// A `w`×`h` BGRA frame showing the map around (`cx`, `cy`) (texture px),
/// the player at the frame centre.
pub fn frame(map: &SynthMap, cx: f64, cy: f64, w: usize, h: usize, ov: &Overlay) -> Vec<u8> {
    let (px, py) = (w as f64 / 2.0, h as f64 / 2.0);
    let s = ov.ui_scale;
    let cone_half = crate::overlay::CONE_HALF_DEG;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f64 - px, y as f64 - py);
            let mut c = map.sample(cx + dx, cy + dy);
            let r = dx.hypot(dy);
            let ang = crate::overlay::screen_bearing(dx, dy);
            let d = crate::overlay::circ_diff(ang, ov.cone);
            if d < cone_half && r > 20.0 * s {
                for (k, v) in c.iter_mut().enumerate() {
                    *v = *v * 0.6 + [235.0, 222.0, 170.0][k] * 0.4;
                }
            }
            if ((d - cone_half).abs().to_radians() * r) < 1.2 && r > 20.0 * s {
                for v in c.iter_mut() {
                    *v = *v * 0.3 + 255.0 * 0.7;
                }
            }
            if (r - ov.ring_r * s).abs() < 1.2 {
                for (k, v) in c.iter_mut().enumerate() {
                    *v = *v * 0.5 + [60.0, 200.0, 200.0][k] * 0.5;
                }
            }
            if r <= 24.0 * s {
                // Light disk with a dark arrow pointing at `facing`.
                c = [235.0, 228.0, 205.0];
                let rel = crate::overlay::circ_diff(ang, ov.facing);
                let along = r * rel.to_radians().cos();
                let across = (r * rel.to_radians().sin()).abs();
                if along > -6.0 * s && along < 11.0 * s && across < (11.0 * s - along) * 0.45 {
                    c = [60.0, 70.0, 90.0];
                }
            }
            for (qx, qy) in &ov.dots {
                let dd = (x as f64 - qx).hypot(y as f64 - qy);
                if dd <= 3.5 {
                    c = [245.0; 3];
                } else if dd <= 4.5 {
                    c = [40.0; 3];
                }
            }
            let i = (y * w + x) * 4;
            out[i] = c[2].clamp(0.0, 255.0) as u8;
            out[i + 1] = c[1].clamp(0.0, 255.0) as u8;
            out[i + 2] = c[0].clamp(0.0, 255.0) as u8;
            out[i + 3] = 255;
        }
    }
    // Screen-fixed HUD icons: a search glass top right, a clock bottom right.
    for (fx, fy, rad) in [(0.93, 0.1, 9.0), (0.85, 0.92, 6.0)] {
        let (ix, iy) = (fx * w as f64, fy * h as f64);
        for y in 0..h {
            for x in 0..w {
                let d = (x as f64 - ix).hypot(y as f64 - iy);
                if (d - rad * s).abs() < 1.5 {
                    let i = (y * w + x) * 4;
                    out[i..i + 3].copy_from_slice(&[235, 235, 235]);
                }
            }
        }
    }
    out
}
