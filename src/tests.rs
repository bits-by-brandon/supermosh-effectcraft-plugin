use crate::glide::*;
use crate::params::{self, PARAMS};

fn defaults() -> Vec<f64> {
    params::default_values()
}

fn set(v: &mut [f64], id: &str, x: f64) {
    v[params::offset(id).unwrap_or_else(|| panic!("no param {id}"))] = x;
}

fn settings(v: &[f64]) -> Settings {
    Settings::from_values(&params::Values::new(v).expect("param count"))
}

/// A deterministic test picture with structure in every block.
fn picture(w: usize, h: usize) -> Vec<[f32; 4]> {
    (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            [(x * 0.37).sin() * 0.5 + 0.5, (y * 0.21).cos() * 0.5 + 0.5, ((x + y) * 0.05).fract(), 1.0]
        })
        .collect()
}

/// Supermosh's `approximate` (core/src/index.ts), generalised to the edge modes: one step.
fn naive_step(prev: &[[f32; 4]], g: &Grid, field: &[(i32, i32)], bw: usize, bh: usize, edges: Edges) -> Vec<[f32; 4]> {
    let (w, h) = (g.w, g.h);
    let e = |v: i64, n: usize| -> Option<usize> {
        let n = n as i64;
        match edges {
            Edges::Wrap => Some(((v + n) % n + n) as usize % n as usize),
            Edges::Clamp => Some(v.clamp(0, n - 1) as usize),
            Edges::Mirror => {
                let m = v.rem_euclid(2 * n);
                Some(if m >= n { 2 * n - 1 - m } else { m } as usize)
            }
            Edges::Transparent => (0..n).contains(&v).then_some(v as usize),
        }
    };
    let mut out = vec![[0.0; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = field[(y / bh) * g.nbx + x / bw];
            out[y * w + x] = match (e(x as i64 + sx as i64, w), e(y as i64 + sy as i64, h)) {
                (Some(qx), Some(qy)) => prev[qy * w + qx],
                _ => [0.0; 4],
            };
        }
    }
    out
}

fn close(a: &[[f32; 4]], b: &[[f32; 4]]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| p.iter().zip(q).all(|(x, y)| (x - y).abs() < 1e-5))
}

#[test]
fn manifest_is_valid() {
    let m: serde_json::Value = serde_json::from_str(&params::manifest_json()).expect("manifest JSON");
    assert_eq!(m["api"], 1);
    let ps = m["params"].as_array().expect("params");
    assert_eq!(ps.len(), PARAMS.len());
    let mut ids = std::collections::HashSet::new();
    for p in ps {
        let id = p["id"].as_str().expect("id");
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "{id}");
        assert!(ids.insert(id.to_string()), "duplicate {id}");
        if p["type"] == "slider" {
            let f = |k: &str| p[k].as_f64().expect(k);
            assert!(f("min") <= f("default") && f("default") <= f("max"), "{id}");
            assert!(f("sliderMin") <= f("sliderMax"), "{id}");
        }
        if p["type"] == "popup" {
            assert!((p["default"].as_u64().expect("default") as usize) < p["options"].as_array().expect("options").len());
        }
    }
    assert!(params::manifest_json().len() < 1 << 20);
    assert_eq!(defaults().len(), params::flat_len());
}

#[test]
fn defaults_match_supermosh_config() {
    let s = settings(&defaults());
    assert_eq!(s.rate, 30.0); // config.fps
    assert_eq!((s.block_w, s.block_h), (16.0, 16.0)); // config.size
    assert_eq!(s.shifts, vec![0.0, 1.0, -1.0, 2.0, -2.0, 4.0, -4.0, 8.0, -8.0]); // config.xyShifts
    assert_eq!(s.motion, Motion::Random);
    assert_eq!(s.edges, Edges::Wrap);
}

#[test]
fn matches_supermosh_glide_iteration() {
    let (w, h) = (70, 45); // partial blocks on the right and bottom, as in Supermosh
    let mut v = defaults();
    set(&mut v, "step_source", 1.0);
    for n in [1u64, 2, 3, 7, 30, 64] {
        set(&mut v, "steps", n as f64);
        let s = settings(&v);
        let src = picture(w, h);
        let g = Grid::new(w, h, &s, 1.0);
        let field = FieldGen::new(&src, &g, &s, 1.0).epoch(&g, 0);
        let mut want = src.clone();
        for _ in 0..n {
            want = naive_step(&want, &g, &field, 16, 16, Edges::Wrap);
        }
        let mut got = src.clone();
        render(&mut got, w, h, &s, 0.0, 1.0);
        assert!(close(&got, &want), "n = {n}");
        assert!(!close(&got, &src), "n = {n} should change the picture");
    }
}

#[test]
fn every_edge_mode_and_reseeding_match_step_by_step() {
    let (w, h) = (41, 33);
    for (ei, edges) in [Edges::Wrap, Edges::Clamp, Edges::Mirror, Edges::Transparent].into_iter().enumerate() {
        for reseed in [0u64, 1, 3, 4] {
            for motion in 0..4 {
                let mut v = defaults();
                set(&mut v, "step_source", 1.0);
                set(&mut v, "steps", 10.0);
                set(&mut v, "edges", ei as f64);
                set(&mut v, "reseed", reseed as f64);
                set(&mut v, "motion", motion as f64);
                set(&mut v, "block_w", 8.0);
                set(&mut v, "block_h", 6.0);
                set(&mut v, "jitter", 30.0);
                set(&mut v, "static", 20.0);
                set(&mut v, "distance", 5.0);
                set(&mut v, "direction", 30.0);
                let s = settings(&v);
                let src = picture(w, h);
                let g = Grid::new(w, h, &s, 1.0);
                let fields = FieldGen::new(&src, &g, &s, 1.0);
                let mut want = src.clone();
                for k in 1..=10u64 {
                    let epoch = (k - 1).checked_div(reseed).unwrap_or(0);
                    let field = fields.epoch(&g, epoch);
                    want = naive_step(&want, &g, &field, 8, 6, edges);
                }
                // The naive loop applies step 1 first to the picture: out_k = out_{k-1}∘M_k,
                // which is exactly the composite M_1∘…∘M_n the plug-in builds.
                let mut got = src.clone();
                render(&mut got, w, h, &s, 0.0, 1.0);
                assert!(close(&got, &want), "edges {edges:?} reseed {reseed} motion {motion}");
            }
        }
    }
}

#[test]
fn timing_start_length_and_after() {
    let (w, h) = (32, 32);
    let src = picture(w, h);
    let mut v = defaults();
    set(&mut v, "start", 1.0);
    set(&mut v, "length", 0.5);
    let run = |v: &[f64], t: f64| {
        let mut p = src.clone();
        render(&mut p, w, h, &settings(v), t, 1.0);
        p
    };
    assert!(close(&run(&v, 0.5), &src), "before the start: untouched");
    assert!(close(&run(&v, 1.0), &src), "step 0 at the start");
    assert!(!close(&run(&v, 1.2), &src));
    assert!(close(&run(&v, 3.0), &run(&v, 1.5)), "Hold Last Step holds");
    set(&mut v, "after", 1.0);
    assert!(close(&run(&v, 3.0), &src), "Show Source after the glide");
    // Frame-aligned times give whole steps.
    let s = settings(&defaults());
    assert_eq!(s.step_position(1.0 / 30.0).floor(), 1.0);
    assert_eq!(s.step_position(10.0 / 30.0).floor(), 10.0);
}

#[test]
fn max_steps_caps_the_glide() {
    let mut v = defaults();
    set(&mut v, "max_steps", 12.0);
    assert_eq!(settings(&v).step_position(100.0), 12.0);
}

#[test]
fn blend_is_between_neighbouring_steps() {
    let (w, h) = (32, 24);
    let src = picture(w, h);
    let mut v = defaults();
    set(&mut v, "step_source", 1.0);
    let run = |v: &[f64]| {
        let mut p = src.clone();
        render(&mut p, w, h, &settings(v), 0.0, 1.0);
        p
    };
    set(&mut v, "steps", 4.0);
    let a = run(&v);
    set(&mut v, "steps", 5.0);
    let b = run(&v);
    set(&mut v, "steps", 4.25);
    assert!(close(&run(&v), &a), "no blend: whole steps");
    set(&mut v, "blend", 1.0);
    let mid = run(&v);
    let want: Vec<[f32; 4]> = a.iter().zip(&b).map(|(p, q)| std::array::from_fn(|i| p[i] * 0.75 + q[i] * 0.25)).collect();
    assert!(close(&mid, &want));
}

#[test]
fn mix_zero_and_force_opaque() {
    let (w, h) = (20, 20);
    let mut src = picture(w, h);
    for p in src.iter_mut() {
        *p = [p[0] * 0.5, p[1] * 0.5, p[2] * 0.5, 0.5];
    }
    let mut v = defaults();
    set(&mut v, "step_source", 1.0);
    set(&mut v, "steps", 9.0);
    set(&mut v, "mix", 0.0);
    let mut p = src.clone();
    render(&mut p, w, h, &settings(&v), 0.0, 1.0);
    assert!(close(&p, &src));
    set(&mut v, "force_opaque", 1.0);
    render(&mut p, w, h, &settings(&v), 0.0, 1.0);
    assert!(p.iter().all(|q| q[3] == 1.0));
}

#[test]
fn uniform_direction_moves_content_that_way() {
    // A single bright pixel, uniform motion 90° (right), 2 px per step, 3 steps → 6 px right.
    let (w, h) = (32, 8);
    let mut src = vec![[0.0, 0.0, 0.0, 1.0]; w * h];
    src[4 * w + 5] = [1.0; 4];
    let mut v = defaults();
    set(&mut v, "step_source", 1.0);
    set(&mut v, "steps", 3.0);
    set(&mut v, "motion", 3.0);
    set(&mut v, "direction", 90.0);
    set(&mut v, "distance", 2.0);
    let mut p = src.clone();
    render(&mut p, w, h, &settings(&v), 0.0, 1.0);
    assert_eq!(p[4 * w + 11], [1.0; 4]);
    assert_eq!(p[4 * w + 5], [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn preview_scale_keeps_small_shifts() {
    let mut v = defaults();
    set(&mut v, "motion", 3.0);
    set(&mut v, "direction", 90.0);
    set(&mut v, "distance", 1.0);
    let s = settings(&v);
    let g = Grid::new(16, 16, &s, 0.25);
    let f = FieldGen::new(&[[0.0; 4]; 256], &g, &s, 0.25).epoch(&g, 0);
    assert!(f.iter().all(|&(x, y)| x == -1 && y == 0), "{f:?}");
}

#[test]
fn deterministic_and_seeded() {
    let (w, h) = (48, 48);
    let src = picture(w, h);
    let mut v = defaults();
    set(&mut v, "step_source", 1.0);
    set(&mut v, "steps", 20.0);
    let run = |v: &[f64]| {
        let mut p = src.clone();
        render(&mut p, w, h, &settings(v), 0.0, 1.0);
        p
    };
    let a = run(&v);
    assert!(close(&a, &run(&v)));
    set(&mut v, "seed", 7.0);
    assert!(!close(&a, &run(&v)));
}

#[test]
fn views_render() {
    let (w, h) = (40, 30);
    let src = picture(w, h);
    let mut v = defaults();
    set(&mut v, "view", 1.0);
    let mut p = src.clone();
    render(&mut p, w, h, &settings(&v), 0.0, 1.0);
    assert!(p.iter().all(|q| q[3] == 1.0));
    set(&mut v, "view", 2.0);
    let mut p = src.clone();
    render(&mut p, w, h, &settings(&v), 0.0, 1.0);
    assert_eq!(p[0], [1.0, 0.0, 1.0, 1.0], "grid line at the origin");
    assert_eq!(p[w + 1], src[w + 1], "inside a block: untouched at step 0");
}

#[test]
fn wrong_param_count_is_an_error() {
    let mut p = picture(4, 4);
    assert!(crate::render_flat(&mut p, 4, 4, &[0.0; 3], 0.0, 1.0).is_err());
}

#[test]
fn shift_reduction_reads_the_same_pixels() {
    // Shifts far larger than the frame are reduced once per frame; every pixel must still read
    // from the same place, in every edge mode.
    for edges in [Edges::Wrap, Edges::Clamp, Edges::Mirror, Edges::Transparent] {
        for n in [1usize, 2, 7, 16, 23] {
            for v in -3000..3000 {
                let r = reduce(v, n, edges);
                assert!(r.unsigned_abs() as usize <= n, "{edges:?} n={n} v={v} → {r}");
                for x in 0..n as i64 {
                    assert_eq!(edge(x + v as i64, n, edges), edge(x + r as i64, n, edges), "{edges:?} n={n} v={v} x={x}");
                }
            }
        }
    }
}

#[test]
fn huge_shifts_render_like_small_equivalents() {
    let (w, h) = (23, 17);
    let src = picture(w, h);
    for e in 0..4 {
        let mut a = defaults();
        set(&mut a, "step_source", 1.0);
        set(&mut a, "steps", 6.0);
        set(&mut a, "edges", e as f64);
        set(&mut a, "block_w", 5.0);
        set(&mut a, "shift_count", 2.0);
        let mut b = a.clone();
        // Wrap repeats every 23 / 17 px, Mirror every 46 / 34, Clamp and Transparent saturate.
        let (big, small) = match e {
            0 => (3.0 + 23.0 * 17.0 * 4.0, 3.0),
            2 => (3.0 + 46.0 * 34.0 * 2.0, 3.0),
            _ => (5000.0, 23.0),
        };
        set(&mut a, "shift_1", big);
        set(&mut a, "shift_2", -big);
        set(&mut b, "shift_1", small);
        set(&mut b, "shift_2", -small);
        let (mut pa, mut pb) = (src.clone(), src.clone());
        render(&mut pa, w, h, &settings(&a), 0.0, 1.0);
        render(&mut pb, w, h, &settings(&b), 0.0, 1.0);
        assert!(close(&pa, &pb), "edges {e}");
    }
}

#[test]
fn budget_caps_heavy_glides_instead_of_failing() {
    // Reseeding every step with 1 px blocks can't fit 100 000 steps in the host's fuel budget:
    // the plan stops early (the glide holds) rather than letting the host abort the frame.
    let mut v = defaults();
    set(&mut v, "step_source", 1.0);
    set(&mut v, "steps", 99_999.0);
    set(&mut v, "max_steps", 100_000.0);
    set(&mut v, "reseed", 1.0);
    set(&mut v, "block_w", 1.0);
    set(&mut v, "block_h", 1.0);
    let s = settings(&v);
    let (w, h) = (64, 36);
    let src = picture(w, h);
    let g = Grid::new(w, h, &s, 1.0);
    let fields = FieldGen::new(&src, &g, &s, 1.0);
    let p = plan(&g, &s, &fields, 99_999, 0);
    assert!(p.steps > 10 && p.steps < 200, "{}", p.steps);
    // Without reseeding, squaring fits the whole glide.
    set(&mut v, "reseed", 0.0);
    let s = settings(&v);
    assert_eq!(plan(&g, &s, &fields, 99_999, 0).steps, 99_999);
}
