//! Classic motion-vector datamosh, as a prepass for EffectCraft.
//!
//! A datamosh drops a keyframe, so the decoder keeps predicting from a stale picture: every
//! P-frame's block motion vectors move the old pixels around and nothing new arrives. This crate
//! does that on decoded frames. [`estimate`] finds a motion vector per block between two frames
//! (block matching, as an encoder's motion search does, down to quarter pixels), and [`warp`]
//! moves the moshed picture with those vectors and no residual.
//!
//! EffectCraft's plug-in API v1 renders each frame on its own, so a plug-in can't carry a picture
//! from frame to frame. This runs before EffectCraft instead; import its output as footage.

/// One channel of a frame, `f32` 0–1, row major.
#[derive(Clone, Debug)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Plane {
    /// Rec. 709 luma of packed RGB8.
    pub fn luma(rgb: &[u8], w: usize, h: usize) -> Plane {
        let px = rgb.as_chunks::<3>().0.iter().take(w * h).map(|c| (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0).collect();
        Plane { w, h, px }
    }

    /// The pixel at `(x, y)`, with the edges extended (unrestricted motion vectors).
    fn at(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        self.px[y * self.w + x]
    }

    fn bilinear(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = (x - x0, y - y0);
        let (xi, yi) = (x0 as i32, y0 as i32);
        let top = self.at(xi, yi) * (1.0 - tx) + self.at(xi + 1, yi) * tx;
        let bottom = self.at(xi, yi + 1) * (1.0 - tx) + self.at(xi + 1, yi + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// Half the size, 2×2 box filtered.
    fn half(&self) -> Plane {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let (sx, sy) = (x * 2, y * 2);
                px.push((self.at(sx, sy) + self.at(sx + 1, sy) + self.at(sx, sy + 1) + self.at(sx + 1, sy + 1)) * 0.25);
            }
        }
        Plane { w, h, px }
    }
}

/// A block's motion: the block's content in the current frame is found at `+ (x, y)` in the
/// previous frame, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mv {
    pub x: f32,
    pub y: f32,
}

/// Motion search settings.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Block size in pixels (16 = H.264 macroblocks).
    pub block: usize,
    /// Largest motion searched, in pixels.
    pub search: usize,
    /// Sub-pixel precision: 1 (whole pixels), 2 (half) or 4 (quarter, as H.264).
    pub subpel: u32,
    /// Cost of motion per pixel of vector length, in mean luma difference: higher values keep
    /// flat areas (sky, water) still instead of letting noise pick a vector.
    pub lambda: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { block: 16, search: 32, subpel: 4, lambda: 0.0015 }
    }
}

/// One motion vector per block, row major.
#[derive(Clone, Debug)]
pub struct Field {
    pub block: usize,
    pub cols: usize,
    pub rows: usize,
    pub mvs: Vec<Mv>,
}

impl Field {
    pub fn get(&self, x: usize, y: usize) -> Mv {
        self.mvs[(y / self.block) * self.cols + x / self.block]
    }
}

/// Mean absolute difference between the block of `cur` at `(x0, y0)`, `n`×`n` (clipped to the
/// frame), and `prev` sampled at an integer offset.
fn cost_int(prev: &Plane, cur: &Plane, x0: usize, y0: usize, n: usize, dx: i32, dy: i32) -> f32 {
    let (x1, y1) = ((x0 + n).min(cur.w), (y0 + n).min(cur.h));
    let mut sum = 0.0;
    for y in y0..y1 {
        for (x, &c) in cur.px[y * cur.w + x0..y * cur.w + x1].iter().enumerate() {
            sum += (c - prev.at((x0 + x) as i32 + dx, y as i32 + dy)).abs();
        }
    }
    sum / ((x1 - x0) * (y1 - y0)).max(1) as f32
}

fn cost_sub(prev: &Plane, cur: &Plane, x0: usize, y0: usize, n: usize, mv: Mv) -> f32 {
    let (x1, y1) = ((x0 + n).min(cur.w), (y0 + n).min(cur.h));
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += (cur.px[y * cur.w + x] - prev.bilinear(x as f32 + mv.x, y as f32 + mv.y)).abs();
        }
    }
    sum / ((x1 - x0) * (y1 - y0)).max(1) as f32
}

/// Candidates kept from one pyramid level to the next.
const KEEP: usize = 4;

/// The motion of one block: a full search at the coarsest pyramid level, then at each finer level
/// a ±2 pixel refinement around the best few candidates so far and around no motion (a coarse
/// level sees too little of a small block to be trusted alone), then half and quarter pixels.
fn search_block(prev: &[Plane], cur: &[Plane], bx: usize, by: usize, s: &Settings) -> Mv {
    let levels = prev.len();
    let mut kept: Vec<(i32, i32)> = vec![(0, 0)];
    for l in (0..levels).rev() {
        let n = (s.block >> l).max(1);
        let (x0, y0) = ((bx * s.block) >> l, (by * s.block) >> l);
        let unit = (1 << l) as f32;
        let mut centres: Vec<((i32, i32), i32)> = if l == levels - 1 {
            vec![((0, 0), ((s.search >> l) as i32).max(1))]
        } else {
            kept.iter().map(|&(x, y)| ((x * 2, y * 2), 2)).chain([((0, 0), 2)]).collect()
        };
        centres.dedup();
        // Only overlapping refinement windows can repeat a candidate (not the coarse full search).
        let overlap = centres.len() > 1;
        let mut scored: Vec<(f32, i32, i32)> = Vec::new();
        for ((cx, cy), r) in centres {
            for dy in cy - r..=cy + r {
                for dx in cx - r..=cx + r {
                    if overlap && scored.iter().any(|&(_, x, y)| (x, y) == (dx, dy)) {
                        continue;
                    }
                    let c = cost_int(&prev[l], &cur[l], x0, y0, n, dx, dy) + s.lambda * unit * (dx.abs() + dy.abs()) as f32;
                    scored.push((c, dx, dy));
                }
            }
        }
        // Ties go to the shorter vector, so the search is deterministic and prefers stillness.
        scored.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1.abs() + a.2.abs()).cmp(&(b.1.abs() + b.2.abs()))).then((a.1, a.2).cmp(&(b.1, b.2))));
        kept = scored.iter().take(KEEP).map(|&(_, x, y)| (x, y)).collect();
    }
    let best = kept[0];
    let mut mv = Mv { x: best.0 as f32, y: best.1 as f32 };
    let (x0, y0) = (bx * s.block, by * s.block);
    let mut best_cost = cost_sub(&prev[0], &cur[0], x0, y0, s.block, mv) + s.lambda * (mv.x.abs() + mv.y.abs());
    let mut step = 0.5;
    while step * s.subpel as f32 >= 1.0 {
        let centre = mv;
        for (sx, sy) in [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)] {
            let cand = Mv { x: centre.x + sx * step, y: centre.y + sy * step };
            let c = cost_sub(&prev[0], &cur[0], x0, y0, s.block, cand) + s.lambda * (cand.x.abs() + cand.y.abs());
            if c < best_cost {
                best_cost = c;
                mv = cand;
            }
        }
        step *= 0.5;
    }
    mv
}

fn pyramid(p: &Plane, s: &Settings) -> Vec<Plane> {
    let mut levels = vec![p.clone()];
    while levels.len() < 3 && (s.block >> levels.len()) >= 2 && (s.search >> levels.len()) >= 2 {
        let next = levels.last().unwrap().half();
        levels.push(next);
    }
    levels
}

/// Rows of `0..n` split across the machine's threads.
fn parallel_rows<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, |t| t.get()).min(n.max(1));
    let chunk = n.div_ceil(threads.max(1)).max(1);
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..n).step_by(chunk).map(|start| scope.spawn(move || (start..(start + chunk).min(n)).map(f).collect::<Vec<T>>())).collect();
        handles.into_iter().flat_map(|h| h.join().expect("worker panicked")).collect()
    })
}

/// The block motion from `prev` to `cur` (both the same size).
pub fn estimate(prev: &Plane, cur: &Plane, s: &Settings) -> Field {
    assert!(prev.w == cur.w && prev.h == cur.h, "frames differ in size");
    let s = Settings { block: s.block.max(2), search: s.search.max(1), ..*s };
    let (pp, cp) = (pyramid(prev, &s), pyramid(cur, &s));
    let (cols, rows) = (cur.w.div_ceil(s.block), cur.h.div_ceil(s.block));
    let mvs = parallel_rows(rows, |by| (0..cols).map(|bx| search_block(&pp, &cp, bx, by, &s)).collect::<Vec<_>>()).concat();
    Field { block: s.block, cols, rows, mvs }
}

/// The moshed picture after one more frame: every pixel of `img` (packed RGB, `f32` 0–255) is
/// taken from where its block's vector, scaled by `gain`, points. No residual is added.
pub fn warp(img: &[f32], w: usize, h: usize, field: &Field, gain: f32) -> Vec<f32> {
    let sample = |c: usize, x: f32, y: f32| -> f32 {
        let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let p = |x: usize, y: usize| img[(y * w + x) * 3 + c];
        (p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx) * (1.0 - ty) + (p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx) * ty
    };
    parallel_rows(h, |y| {
        let mut row = Vec::with_capacity(w * 3);
        for x in 0..w {
            let mv = field.get(x, y);
            let (sx, sy) = (x as f32 + mv.x * gain, y as f32 + mv.y * gain);
            for c in 0..3 {
                row.push(sample(c, sx, sy));
            }
        }
        row
    })
    .concat()
}

#[cfg(test)]
mod tests;
