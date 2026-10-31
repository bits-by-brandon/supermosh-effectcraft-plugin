//! The glide: Supermosh's block-motion feedback loop (`approximate` in Supermosh's 2021
//! `core/src/index.ts`), made stateless.
//!
//! Supermosh splits the frame into `size × size` blocks, gives each block a shift `(sx, sy)` and
//! then, once per output frame, replaces the picture with
//!
//! ```text
//! next(x, y) = previous((x + sx) mod width, (y + sy) mod height)   // (sx, sy) of the block of (x, y)
//! ```
//!
//! feeding each result back in, so the image smears further every frame. An EffectCraft plug-in
//! has no memory of earlier frames, but it doesn't need one: after `n` steps every output pixel is
//! a source pixel, `out_n(p) = src(M_1(M_2(…M_n(p))))`, where `M_k` is the coordinate map of step
//! `k`. So we build that composite coordinate map directly (`C ← C ∘ M_k`, by repeated squaring
//! for runs of steps that share one motion field: `O(pixels · log n)`) and sample the incoming
//! frame once.

use crate::params::{MAX_SHIFTS, Values};

/// A coordinate that left the frame (Edges: Transparent).
const NONE: u32 = u32::MAX;

/// Luma difference below which Luma Gradient treats a block as flat.
const GRADIENT_FLOOR: f64 = 0.002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Random,
    LumaGradient,
    HueDirection,
    Uniform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edges {
    Wrap,
    Clamp,
    Mirror,
    Transparent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Result,
    MotionField,
    ResultGrid,
}

/// Every parameter, decoded.
#[derive(Clone, Debug)]
pub struct Settings {
    pub manual_steps: bool,
    pub start: f64,
    pub length: f64,
    pub hold_after: bool,
    pub rate: f64,
    pub steps: f64,
    pub blend: bool,
    pub max_steps: f64,
    pub block_w: f64,
    pub block_h: f64,
    /// Buffer pixels.
    pub origin: [f64; 2],
    /// Layer pixels.
    pub shifts: Vec<f64>,
    pub x_amount: f64,
    pub y_amount: f64,
    pub motion: Motion,
    pub seed: u64,
    pub reseed: u64,
    /// Degrees, clockwise from up.
    pub direction: f64,
    /// Layer pixels.
    pub distance: f64,
    pub snap: bool,
    pub jitter: f64,
    pub statics: f64,
    pub edges: Edges,
    pub force_opaque: bool,
    pub mix: f64,
    pub view: View,
    pub grid_color: [f32; 4],
}

impl Settings {
    pub fn from_values(v: &Values) -> Self {
        let count = (v.f("shift_count").round() as usize).clamp(1, MAX_SHIFTS);
        let shifts = (1..=count).map(|i| v.f(&format!("shift_{i}")).round()).collect();
        let gc = v.color("grid_color");
        Settings {
            manual_steps: v.index("step_source") == 1,
            start: v.f("start"),
            length: v.f("length").max(0.0),
            hold_after: v.index("after") == 0,
            rate: v.f("rate").max(0.0),
            steps: v.f("steps").max(0.0),
            blend: v.b("blend"),
            max_steps: v.f("max_steps").max(0.0).floor(),
            block_w: v.f("block_w").max(1.0),
            block_h: v.f("block_h").max(1.0),
            origin: v.point("grid_origin"),
            shifts,
            x_amount: v.f("x_amount") / 100.0,
            y_amount: v.f("y_amount") / 100.0,
            motion: match v.index("motion") {
                1 => Motion::LumaGradient,
                2 => Motion::HueDirection,
                3 => Motion::Uniform,
                _ => Motion::Random,
            },
            seed: v.f("seed").max(0.0) as u64,
            reseed: v.f("reseed").max(0.0) as u64,
            direction: v.f("direction"),
            distance: v.f("distance"),
            snap: v.b("snap"),
            jitter: (v.f("jitter") / 100.0).clamp(0.0, 1.0),
            statics: (v.f("static") / 100.0).clamp(0.0, 1.0),
            edges: match v.index("edges") {
                1 => Edges::Clamp,
                2 => Edges::Mirror,
                3 => Edges::Transparent,
                _ => Edges::Wrap,
            },
            force_opaque: v.b("force_opaque"),
            mix: (v.f("mix") / 100.0).clamp(0.0, 1.0),
            view: match v.index("view") {
                1 => View::MotionField,
                2 => View::ResultGrid,
                _ => View::Result,
            },
            grid_color: [gc[0] as f32, gc[1] as f32, gc[2] as f32, gc[3] as f32],
        }
    }

    /// How far into the glide `time` is, in steps (fractional), capped at Max Steps.
    pub fn step_position(&self, time: f64) -> f64 {
        let s = if self.manual_steps {
            self.steps
        } else {
            let mut local = time - self.start;
            if local < 0.0 {
                return 0.0;
            }
            if self.length > 0.0 && local > self.length {
                if !self.hold_after {
                    return 0.0;
                }
                local = self.length;
            }
            local * self.rate
        };
        // `1/30 s × 30` is 0.9999…: nudge so frame-aligned times land on whole steps.
        (s + 1e-6).clamp(0.0, self.max_steps.max(0.0))
    }
}

/// The block grid over a `w × h` buffer.
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub bw: i64,
    pub bh: i64,
    ox: i64,
    oy: i64,
    /// Absolute grid column / row of the first block touching the buffer.
    bx0: i64,
    by0: i64,
    pub nbx: usize,
    pub nby: usize,
    /// The block column of each buffer column, the block row of each buffer row.
    col: Vec<u32>,
    row: Vec<u32>,
    /// The buffer column where each block column ends (exclusive, clipped).
    col_end: Vec<usize>,
}

impl Grid {
    pub fn new(w: usize, h: usize, s: &Settings, scale: f64) -> Grid {
        let bw = ((s.block_w * scale).round() as i64).max(1);
        let bh = ((s.block_h * scale).round() as i64).max(1);
        let ox = s.origin[0].round() as i64;
        let oy = s.origin[1].round() as i64;
        let bx0 = (-ox).div_euclid(bw);
        let by0 = (-oy).div_euclid(bh);
        let nbx = ((w as i64 - 1 - ox).div_euclid(bw) - bx0 + 1).max(1) as usize;
        let nby = ((h as i64 - 1 - oy).div_euclid(bh) - by0 + 1).max(1) as usize;
        let col = (0..w as i64).map(|x| ((x - ox).div_euclid(bw) - bx0) as u32).collect();
        let row = (0..h as i64).map(|y| ((y - oy).div_euclid(bh) - by0) as u32).collect();
        let col_end = (0..nbx as i64).map(|i| ((bx0 + i + 1) * bw + ox).clamp(0, w as i64) as usize).collect();
        Grid { w, h, bw, bh, ox, oy, bx0, by0, nbx, nby, col, row, col_end }
    }

    /// The buffer rectangle of block `(i, j)`, clipped: `x0..x1, y0..y1`.
    fn rect(&self, i: usize, j: usize) -> (usize, usize, usize, usize) {
        let x0 = (self.bx0 + i as i64) * self.bw + self.ox;
        let y0 = (self.by0 + j as i64) * self.bh + self.oy;
        let clip = |v: i64, max: usize| v.clamp(0, max as i64) as usize;
        (clip(x0, self.w), clip(x0 + self.bw, self.w), clip(y0, self.h), clip(y0 + self.bh, self.h))
    }

    fn is_line(&self, x: usize, y: usize) -> bool {
        (x as i64 - self.ox).rem_euclid(self.bw) == 0 || (y as i64 - self.oy).rem_euclid(self.bh) == 0
    }
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A deterministic random number for one block of one epoch.
fn block_hash(seed: u64, epoch: u64, bx: i64, by: i64) -> u64 {
    splitmix(
        seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ epoch.wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ (bx as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
            ^ (by as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93),
    )
}

/// A probability as a threshold on 53 random bits.
fn threshold(p: f64) -> u64 {
    (p.clamp(0.0, 1.0) * (1u64 << 53) as f64) as u64
}

/// Rotate `(x, y)` (y down) clockwise on screen by `deg`.
fn rotate(x: f64, y: f64, deg: f64) -> (f64, f64) {
    let (s, c) = deg.to_radians().sin_cos();
    (x * c - y * s, x * s + y * c)
}

/// A shift in layer pixels to buffer pixels. A shift that is non-zero at full resolution stays
/// at least one pixel at preview resolutions, so previews move like the final render.
fn to_buffer(v: f64, scale: f64) -> i32 {
    let full = v.round();
    if full == 0.0 {
        return 0;
    }
    let b = (v * scale).round();
    let b = if b == 0.0 { full.signum() } else { b };
    b.clamp(-1_048_576.0, 1_048_576.0) as i32
}

fn luma(p: &[f32; 4]) -> f64 {
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

/// The per-block shifts the image-driven and uniform modes start from, in layer pixels (a shift
/// is where a pixel reads from, so content moves the opposite way). Empty for Random.
fn base_field(src: &[[f32; 4]], g: &Grid, s: &Settings) -> Vec<(f64, f64)> {
    let along = |dx: f64, dy: f64| (-dx * s.distance, -dy * s.distance);
    let mut field = Vec::with_capacity(g.nbx * g.nby);
    match s.motion {
        Motion::Random => {}
        Motion::Uniform => {
            let (dx, dy) = rotate(0.0, -1.0, s.direction);
            field.resize(g.nbx * g.nby, along(dx, dy));
        }
        Motion::LumaGradient => {
            let mut grads = Vec::with_capacity(g.nbx * g.nby);
            for j in 0..g.nby {
                for i in 0..g.nbx {
                    let (x0, x1, y0, y1) = g.rect(i, j);
                    let (xm, ym) = ((x0 + x1) / 2, (y0 + y1) / 2);
                    let mut sum = [0.0f64; 4]; // left, right, top, bottom
                    let mut cnt = [0usize; 4];
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let l = luma(&src[y * g.w + x]);
                            let h = if x < xm { 0 } else { 1 };
                            let v = if y < ym { 2 } else { 3 };
                            sum[h] += l;
                            cnt[h] += 1;
                            sum[v] += l;
                            cnt[v] += 1;
                        }
                    }
                    let mean = |k: usize| if cnt[k] > 0 { sum[k] / cnt[k] as f64 } else { 0.0 };
                    let gx = if cnt[0] > 0 && cnt[1] > 0 { mean(1) - mean(0) } else { 0.0 };
                    let gy = if cnt[2] > 0 && cnt[3] > 0 { mean(3) - mean(2) } else { 0.0 };
                    grads.push((gx, gy));
                }
            }
            for (gx, gy) in grads {
                // Every block with a visible slope moves the full distance along it; flat
                // blocks (below ~half an 8-bit level) stay put.
                let m = gx.hypot(gy);
                let (nx, ny) = if m > GRADIENT_FLOOR { (gx / m, gy / m) } else { (0.0, 0.0) };
                let (dx, dy) = rotate(nx, ny, s.direction);
                field.push(along(dx, dy));
            }
        }
        Motion::HueDirection => {
            for j in 0..g.nby {
                for i in 0..g.nbx {
                    let (x0, x1, y0, y1) = g.rect(i, j);
                    let mut sum = [0.0f64; 4];
                    for y in y0..y1 {
                        for x in x0..x1 {
                            for (k, v) in src[y * g.w + x].iter().enumerate() {
                                sum[k] += *v as f64;
                            }
                        }
                    }
                    // Mean colour, un-premultiplied.
                    let (r, gr, b) = if sum[3] > 0.0 { (sum[0] / sum[3], sum[1] / sum[3], sum[2] / sum[3]) } else { (0.0, 0.0, 0.0) };
                    let (hue, sat) = hue_sat(r, gr, b);
                    let (dx, dy) = rotate(0.0, -sat, hue + s.direction);
                    field.push(along(dx, dy));
                }
            }
        }
    }
    field
}

/// HSV hue (degrees, 0 = red) and saturation of an RGB colour.
fn hue_sat(r: f64, g: f64, b: f64) -> (f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let c = max - min;
    if max <= 0.0 || c <= 0.0 {
        return (0.0, 0.0);
    }
    let h = if max == r {
        60.0 * ((g - b) / c).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / c + 2.0)
    } else {
        60.0 * ((r - g) / c + 4.0)
    };
    (h, (c / max).min(1.0))
}

fn snap(v: f64, list: &[f64]) -> f64 {
    list.iter().copied().min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs())).unwrap_or(v)
}

/// A shift reduced to at most one frame size `n` without changing where any pixel reads from:
/// Wrap repeats every `n` pixels, Mirror every `2n`, and Clamp and Transparent saturate past `n`.
pub(crate) fn reduce(v: i32, n: usize, edges: Edges) -> i32 {
    let n = n.min(1 << 24) as i64;
    let v = v as i64;
    let r = match edges {
        Edges::Wrap => {
            let m = v.rem_euclid(n);
            if m > n / 2 { m - n } else { m }
        }
        Edges::Mirror => {
            let m = v.rem_euclid(2 * n);
            if m > n { m - 2 * n } else { m }
        }
        Edges::Clamp | Edges::Transparent => v.clamp(-n, n),
    };
    r as i32
}

/// Builds the per-block motion field of each epoch (a run of `reseed` steps), in buffer pixels.
/// Everything that doesn't change between epochs is worked out once per frame.
pub struct FieldGen {
    random: bool,
    /// The shift list in buffer pixels, with the horizontal / vertical amounts applied.
    lx: Vec<i32>,
    ly: Vec<i32>,
    /// Image-driven and uniform modes: each block's shift (snapped, scaled), in buffer pixels.
    base: Vec<(i32, i32)>,
    seed: u64,
    jitter: u64,
    statics: u64,
}

impl FieldGen {
    pub fn new(src: &[[f32; 4]], g: &Grid, s: &Settings, scale: f64) -> FieldGen {
        let list = &s.shifts;
        let (rx, ry) = (|v| reduce(v, g.w, s.edges), |v| reduce(v, g.h, s.edges));
        let lx = list.iter().map(|v| rx(to_buffer(v * s.x_amount, scale))).collect();
        let ly = list.iter().map(|v| ry(to_buffer(v * s.y_amount, scale))).collect();
        let base = base_field(src, g, s)
            .into_iter()
            .map(|(mut vx, mut vy)| {
                if s.snap {
                    vx = snap(vx, list);
                    vy = snap(vy, list);
                }
                (rx(to_buffer(vx * s.x_amount, scale)), ry(to_buffer(vy * s.y_amount, scale)))
            })
            .collect();
        FieldGen {
            random: s.motion == Motion::Random,
            lx,
            ly,
            base,
            seed: s.seed,
            jitter: if s.jitter > 0.0 { threshold(s.jitter) } else { 0 },
            statics: if s.statics > 0.0 { threshold(s.statics) } else { 0 },
        }
    }

    /// The largest horizontal and vertical shift any block can get, in buffer pixels.
    pub fn max_shift(&self) -> (u32, u32) {
        let fx = self.lx.iter().chain(self.base.iter().map(|b| &b.0)).map(|v| v.unsigned_abs()).max().unwrap_or(0);
        let fy = self.ly.iter().chain(self.base.iter().map(|b| &b.1)).map(|v| v.unsigned_abs()).max().unwrap_or(0);
        (fx, fy)
    }

    /// The field of epoch `epoch`.
    pub fn epoch(&self, g: &Grid, epoch: u64) -> Vec<(i32, i32)> {
        let n = self.lx.len().max(1) as u64;
        let pick = |h: u64| {
            let (i, j) = ((h % n) as usize, ((h >> 32) % n) as usize);
            (self.lx.get(i).copied().unwrap_or(0), self.ly.get(j).copied().unwrap_or(0))
        };
        let mut out = Vec::with_capacity(g.nbx * g.nby);
        for j in 0..g.nby {
            for i in 0..g.nbx {
                let h = block_hash(self.seed, epoch, g.bx0 + i as i64, g.by0 + j as i64);
                // Supermosh picks each block's x and y shift from `xyShifts`.
                let mut v = if self.random { pick(h) } else { self.base.get(j * g.nbx + i).copied().unwrap_or((0, 0)) };
                if self.jitter > 0 || self.statics > 0 {
                    let h2 = splitmix(h ^ 0xA5A5_A5A5_A5A5_A5A5);
                    if (h2 >> 11) < self.jitter {
                        v = pick(splitmix(h2));
                    }
                    let h3 = splitmix(h ^ 0x5A5A_5A5A_5A5A_5A5A);
                    if (h3 >> 11) < self.statics {
                        v = (0, 0);
                    }
                }
                out.push(v);
            }
        }
        out
    }
}

pub(crate) fn edge(v: i64, n: usize, e: Edges) -> Option<usize> {
    let n = n as i64;
    if (0..n).contains(&v) {
        return Some(v as usize);
    }
    match e {
        // Shifts are reduced to at most one frame (see `reduce`): skip the division then.
        Edges::Wrap if v < 0 && v >= -n => Some((v + n) as usize),
        Edges::Wrap if v >= n && v < 2 * n => Some((v - n) as usize),
        Edges::Wrap => Some(v.rem_euclid(n) as usize),
        Edges::Clamp => Some(v.clamp(0, n - 1) as usize),
        Edges::Mirror if v < 0 && v >= -n => Some((-1 - v) as usize),
        Edges::Mirror if v >= n && v < 2 * n => Some((2 * n - 1 - v) as usize),
        Edges::Mirror => {
            let m = v.rem_euclid(2 * n);
            Some(if m >= n { 2 * n - 1 - m } else { m } as usize)
        }
        Edges::Transparent => None,
    }
}

/// One step's coordinate map: where each buffer pixel reads from.
pub fn step_map(g: &Grid, field: &[(i32, i32)], edges: Edges) -> Vec<u32> {
    let identity: Vec<u32> = (0..(g.w * g.h) as u32).collect();
    let mut t = Vec::with_capacity(g.w * g.h);
    gather_step(&identity, &mut t, g, field, edges);
    t
}

/// `out = c ∘ M`, where `M` is the step map of `field`: `out[p] = c[M(p)]`.
///
/// Hot loop: wasmi charges fuel per structured block, so the loop bodies stay small and the
/// frame-edge handling lives in a cold helper. Wide blocks copy each block row's contiguous
/// source run as a slice; narrow blocks go pixel by pixel.
fn gather_step(c: &[u32], out: &mut Vec<u32>, g: &Grid, field: &[(i32, i32)], edges: Edges) {
    let (w, h) = (g.w, g.h);
    out.clear();
    out.resize(w * h, NONE);
    let narrow = g.bw < 6;
    for y in 0..h {
        let rowb = g.row[y] as usize * g.nbx;
        let orow = &mut out[y * w..(y + 1) * w];
        if narrow {
            for (x, o) in orow.iter_mut().enumerate() {
                let (sx, sy) = field[rowb + g.col[x] as usize];
                let qx = x.wrapping_add(sx as isize as usize);
                let qy = y.wrapping_add(sy as isize as usize);
                *o = if qx < w && qy < h { c[qy * w + qx] } else { edge_pixel(c, g, x, y, sx, sy, edges) };
            }
            continue;
        }
        let mut x0 = 0;
        while x0 < w {
            let bi = g.col[x0] as usize;
            let x1 = g.col_end[bi];
            let (sx, sy) = field[rowb + bi];
            block_row(c, orow, g, x0, x1, y, sx, sy, edges);
            x0 = x1;
        }
    }
}

/// One block's row: `orow[x0..x1]` from `c`, shifted by `(sx, sy)`. The part whose source is
/// inside the frame is one slice copy; only the pixels past the frame edge go one by one.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn block_row(c: &[u32], orow: &mut [u32], g: &Grid, x0: usize, x1: usize, y: usize, sx: i32, sy: i32, edges: Edges) {
    let (w, h) = (g.w, g.h);
    let qy = y.wrapping_add(sy as isize as usize);
    let qy = if qy < h {
        qy
    } else {
        match edge_cold(y as i64 + sy as i64, h, edges) {
            Some(q) => q,
            None => {
                orow[x0..x1].fill(NONE);
                return;
            }
        }
    };
    let crow = &c[qy * w..(qy + 1) * w];
    let sx = sx as isize;
    // Output columns whose source column x + sx is inside the frame.
    let lo = (-sx).clamp(x0 as isize, x1 as isize) as usize;
    let hi = (w as isize - sx).clamp(x0 as isize, x1 as isize) as usize;
    if lo < hi {
        let from = (lo as isize + sx) as usize;
        orow[lo..hi].copy_from_slice(&crow[from..from + (hi - lo)]);
    }
    if x0 < lo || hi < x1 {
        edge_columns(crow, orow, g.w, x0, lo, hi, x1, sx as i64, edges);
    }
}

/// The columns of a block row whose source is past the left or right frame edge.
#[cold]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn edge_columns(crow: &[u32], orow: &mut [u32], w: usize, x0: usize, lo: usize, hi: usize, x1: usize, sx: i64, edges: Edges) {
    for x in (x0..lo).chain(hi.max(x0)..x1) {
        orow[x] = edge(x as i64 + sx, w, edges).map_or(NONE, |q| crow[q]);
    }
}

#[cold]
#[inline(never)]
fn edge_cold(v: i64, n: usize, e: Edges) -> Option<usize> {
    edge(v, n, e)
}

#[cold]
#[inline(never)]
fn edge_pixel(c: &[u32], g: &Grid, x: usize, y: usize, sx: i32, sy: i32, edges: Edges) -> u32 {
    match (edge(x as i64 + sx as i64, g.w, edges), edge(y as i64 + sy as i64, g.h, edges)) {
        (Some(qx), Some(qy)) => c[qy * g.w + qx],
        _ => NONE,
    }
}

/// `out = c ∘ t`: `out[p] = c[t[p]]`.
fn gather_table(c: &[u32], out: &mut Vec<u32>, t: &[u32]) {
    out.clear();
    out.extend(t.iter().map(|&q| if q == NONE { NONE } else { c[q as usize] }));
}

/// Fuel (WebAssembly instructions, roughly) per pixel, measured with `tools/fuel-probe` on
/// EffectCraft's wasmi. They plan each frame so it stays inside the host's budget.
mod cost {
    /// The host's budget per pixel (EffectCraft: `FUEL_PER_PIXEL`).
    pub const HOST_BUDGET: f64 = 20_000.0;
    /// The share of it a frame plans to use, leaving headroom for what the model leaves out.
    pub const BUDGET: f64 = HOST_BUDGET * 0.8;
    /// Copying the frame, sampling, mixing and the rest that runs once.
    pub const FIXED: f64 = 400.0;
    /// Luma Gradient / Hue Direction reading every pixel once, plus a per-block summary.
    pub fn image_field(block_area: f64) -> f64 {
        100.0 + 2500.0 / block_area
    }
    /// Building one epoch's field, per block.
    pub const FIELD_PER_BLOCK: f64 = 350.0;
    /// Composing two coordinate tables.
    pub const TABLE: f64 = 45.0;
    /// A pixel whose source is past the frame edge (the slow path), for narrow / wide blocks.
    pub const EDGE_PIXEL_NARROW: f64 = 450.0;
    pub const EDGE_PIXEL_WIDE: f64 = 300.0;
    /// One direct step: narrow blocks pixel by pixel, wider ones one slice copy per block row,
    /// plus the share of pixels (up to `edge_share`) whose source crosses the frame edge.
    pub fn direct(block_w: i64, edge_share: f64) -> f64 {
        if block_w < 6 { 110.0 + EDGE_PIXEL_NARROW * edge_share } else { 6.0 + 220.0 / block_w as f64 + EDGE_PIXEL_WIDE * edge_share }
    }
}

/// How one run of `k` steps sharing a field is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    /// `k` gathers straight from the field.
    Direct,
    /// One step table, then squaring: `log2 k` squarings and `popcount k` gathers.
    Squaring,
}

fn run_cost(k: u64, direct: f64) -> (f64, Method) {
    let by_direct = k as f64 * direct;
    let by_squaring = if k > 1 { direct + cost::TABLE * ((63 - k.leading_zeros()) + k.count_ones()) as f64 } else { f64::INFINITY };
    if by_direct <= by_squaring { (by_direct, Method::Direct) } else { (by_squaring, Method::Squaring) }
}

/// The runs of steps a frame applies: `(epoch, steps, method)`, in order.
pub struct Plan {
    runs: Vec<(u64, u64, Method)>,
    /// The steps the plan covers: `n`, or fewer when `n` steps would overrun the host's budget.
    pub steps: u64,
}

/// Plan `n` steps (plus `extra` for blending), cutting the glide short where the frame would run
/// out of fuel: the host would otherwise abort the frame and show it unprocessed.
pub fn plan(g: &Grid, s: &Settings, fields: &FieldGen, n: u64, extra: u64) -> Plan {
    let (mx, my) = fields.max_shift();
    let edge_share = (mx as f64 / g.w as f64 + my as f64 / g.h as f64).min(1.0);
    let direct = cost::direct(g.bw, edge_share);
    let field = cost::FIELD_PER_BLOCK * (g.nbx * g.nby) as f64 / (g.w * g.h).max(1) as f64;
    let image = match s.motion {
        Motion::LumaGradient | Motion::HueDirection => cost::image_field((g.bw * g.bh) as f64),
        _ => 0.0,
    };
    let mut spent = cost::FIXED + image + extra as f64 * (direct + field);
    let mut runs = vec![];
    let mut steps = 0;
    let r = if s.reseed == 0 { n.max(1) } else { s.reseed };
    let mut e = 0;
    while steps < n {
        let k = r.min(n - steps);
        let (c, m) = run_cost(k, direct);
        if spent + field + c <= cost::BUDGET {
            spent += field + c;
            runs.push((epoch_id(s, e), k, m));
            steps += k;
            e += 1;
            continue;
        }
        // The largest part of this run that still fits (the cost grows with k).
        let (mut lo, mut hi) = (0, k);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if spent + field + run_cost(mid, direct).0 <= cost::BUDGET { lo = mid } else { hi = mid - 1 }
        }
        if lo > 0 {
            runs.push((epoch_id(s, e), lo, run_cost(lo, direct).1));
            steps += lo;
        }
        break;
    }
    Plan { runs, steps }
}

/// The composite map of a plan: `M_1 ∘ M_2 ∘ … ∘ M_n`. Built left to right, `C ← C ∘ M_k`, so
/// each pass is a gather over known output coordinates.
pub fn composite(g: &Grid, s: &Settings, fields: &FieldGen, plan: &Plan) -> Vec<u32> {
    let mut c: Vec<u32> = (0..(g.w * g.h) as u32).collect();
    let mut out: Vec<u32> = Vec::with_capacity(c.len());
    for &(epoch, k, method) in &plan.runs {
        let field = fields.epoch(g, epoch);
        if method == Method::Direct {
            for _ in 0..k {
                gather_step(&c, &mut out, g, &field, s.edges);
                std::mem::swap(&mut c, &mut out);
            }
            continue;
        }
        // C ← C ∘ T^k, by squaring T (powers of one map commute).
        let mut pow = step_map(g, &field, s.edges);
        let mut tmp: Vec<u32> = Vec::with_capacity(c.len());
        let mut k = k;
        loop {
            if k & 1 == 1 {
                gather_table(&c, &mut out, &pow);
                std::mem::swap(&mut c, &mut out);
            }
            k >>= 1;
            if k == 0 {
                break;
            }
            gather_table(&pow, &mut tmp, &pow);
            std::mem::swap(&mut pow, &mut tmp);
        }
    }
    c
}

/// The epoch number used for hashing (always 0 when the field never reseeds).
fn epoch_id(s: &Settings, e: u64) -> u64 {
    if s.reseed == 0 { 0 } else { e }
}

/// The epoch step `k` (1-based) belongs to.
fn epoch_of(s: &Settings, k: u64) -> u64 {
    (k.max(1) - 1).checked_div(s.reseed).unwrap_or(0)
}

fn lerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

fn hsv(h: f64, v: f64) -> [f32; 4] {
    let h = h.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    [(r * v) as f32, (g * v) as f32, (b * v) as f32, 1.0]
}

/// Render one frame in place.
pub fn render(px: &mut [[f32; 4]], w: usize, h: usize, s: &Settings, time: f64, scale: f64) {
    if w == 0 || h == 0 || px.len() < w * h {
        return;
    }
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let pos = s.step_position(time);
    let mut n = pos.floor() as u64;
    let mut frac = if s.blend && (pos - n as f64) > 1e-5 && (n as f64) < s.max_steps { (pos - n as f64) as f32 } else { 0.0 };
    let g = Grid::new(w, h, s, scale);
    let src = px[..w * h].to_vec();
    let fields = FieldGen::new(&src, &g, s, scale);

    if s.view == View::MotionField {
        let field = fields.epoch(&g, epoch_id(s, epoch_of(s, n.max(1))));
        let max = field.iter().map(|&(x, y)| (x as f64).hypot(y as f64)).fold(0.0, f64::max);
        // Hue from the direction content moves (against the shift; 0° = up, clockwise),
        // brightness from speed. One colour per block.
        let colours: Vec<[f32; 4]> = field
            .iter()
            .map(|&(sx, sy)| {
                let (dx, dy) = (-sx as f64, -sy as f64);
                let mag = dx.hypot(dy);
                if max > 0.0 && mag > 0.0 { hsv(dx.atan2(-dy).to_degrees(), mag / max) } else { [0.0, 0.0, 0.0, 1.0] }
            })
            .collect();
        for y in 0..h {
            let rowb = g.row[y] as usize * g.nbx;
            for x in 0..w {
                px[y * w + x] = colours[rowb + g.col[x] as usize];
            }
        }
        return;
    }

    if n > 0 || frac > 0.0 {
        let plan = plan(&g, s, &fields, n, u64::from(frac > 0.0));
        if plan.steps < n {
            // Out of budget: the glide stops where the frame can still afford it.
            n = plan.steps;
            frac = 0.0;
        }
        let c = composite(&g, s, &fields, &plan);
        let at = |q: u32| if q == NONE { [0.0; 4] } else { src[q as usize] };
        // `C_{n+1} = C_n ∘ M_{n+1}`, for blending toward the next step.
        let next = (frac > 0.0).then(|| {
            let field = fields.epoch(&g, epoch_id(s, epoch_of(s, n + 1)));
            let mut out = vec![];
            gather_step(&c, &mut out, &g, &field, s.edges);
            out
        });
        let mix = s.mix as f32;
        for p in 0..w * h {
            let mut v = at(c[p]);
            if let Some(c1) = &next {
                v = lerp(v, at(c1[p]), frac);
            }
            px[p] = lerp(src[p], v, mix);
        }
    }

    if s.force_opaque {
        for p in px[..w * h].iter_mut() {
            p[3] = 1.0;
        }
    }
    if s.view == View::ResultGrid {
        let [r, gg, b, a] = s.grid_color;
        for y in 0..h {
            for x in 0..w {
                if g.is_line(x, y) {
                    let p = &mut px[y * w + x];
                    *p = [r * a + p[0] * (1.0 - a), gg * a + p[1] * (1.0 - a), b * a + p[2] * (1.0 - a), a + p[3] * (1.0 - a)];
                }
            }
        }
    }
}
