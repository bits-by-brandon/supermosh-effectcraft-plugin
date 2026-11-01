use super::*;

/// Smooth value noise with no repeats (a periodic texture would match at several offsets).
fn noise(x: f32, y: f32) -> f32 {
    let hash = |x: i32, y: i32| {
        let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        (h >> 8) as f32 / (1u32 << 24) as f32
    };
    let (x, y) = (x / 5.0, y / 5.0);
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let (xi, yi) = (x0 as i32, y0 as i32);
    let top = hash(xi, yi) * (1.0 - tx) + hash(xi + 1, yi) * tx;
    let bottom = hash(xi, yi + 1) * (1.0 - tx) + hash(xi + 1, yi + 1) * tx;
    top * (1.0 - ty) + bottom * ty
}

/// A deterministic textured picture, sampled at `(x - ox, y - oy)`: content moved by `(ox, oy)`.
fn texture(w: usize, h: usize, ox: f32, oy: f32) -> Plane {
    let px = (0..w * h).map(|i| noise((i % w) as f32 - ox + 100.0, (i / w) as f32 - oy + 100.0)).collect();
    Plane { w, h, px }
}

fn interior(field: &Field) -> impl Iterator<Item = Mv> + '_ {
    (1..field.rows - 1).flat_map(move |by| (1..field.cols - 1).map(move |bx| field.mvs[by * field.cols + bx]))
}

#[test]
fn still_frames_have_no_motion() {
    let p = texture(96, 64, 0.0, 0.0);
    let field = estimate(&p, &p, &Settings::default());
    assert!(field.mvs.iter().all(|m| *m == Mv::default()));
}

#[test]
fn finds_whole_pixel_motion() {
    // Content moving right 5 and up 3: each block of the new frame is found 5 left and 3 down.
    let (prev, cur) = (texture(128, 96, 0.0, 0.0), texture(128, 96, 5.0, -3.0));
    let field = estimate(&prev, &cur, &Settings::default());
    for mv in interior(&field) {
        assert_eq!(mv, Mv { x: -5.0, y: 3.0 });
    }
}

#[test]
fn finds_motion_past_the_finest_search() {
    // 20 px is beyond the ±2 refinement, so it has to come from the coarse levels.
    let (prev, cur) = (texture(160, 96, 0.0, 0.0), texture(160, 96, -20.0, 0.0));
    let field = estimate(&prev, &cur, &Settings { lambda: 0.0, ..Settings::default() });
    for mv in interior(&field) {
        assert_eq!(mv, Mv { x: 20.0, y: 0.0 });
    }
}

#[test]
fn finds_quarter_pixel_motion() {
    let (prev, cur) = (texture(128, 96, 0.0, 0.0), texture(128, 96, 2.25, 0.5));
    let field = estimate(&prev, &cur, &Settings { lambda: 0.0, ..Settings::default() });
    for mv in interior(&field) {
        assert!((mv.x + 2.25).abs() < 0.13 && (mv.y + 0.5).abs() < 0.13, "{mv:?}");
    }
}

#[test]
fn whole_pixel_mode_rounds() {
    let (prev, cur) = (texture(128, 96, 0.0, 0.0), texture(128, 96, 2.25, 0.0));
    let field = estimate(&prev, &cur, &Settings { subpel: 1, lambda: 0.0, ..Settings::default() });
    for mv in interior(&field) {
        assert_eq!(mv, Mv { x: -2.0, y: 0.0 });
    }
}

#[test]
fn warp_moves_the_stale_picture_with_the_motion() {
    // A stale picture with one bright pixel, and a field saying everything moved right by 4.
    let (w, h) = (32, 32);
    let mut img = vec![0.0; w * h * 3];
    img[(10 * w + 10) * 3] = 255.0;
    let field = Field { block: 16, cols: 2, rows: 2, mvs: vec![Mv { x: -4.0, y: 0.0 }; 4] };
    let out = warp(&img, w, h, &field, 1.0);
    assert_eq!(out[(10 * w + 14) * 3], 255.0);
    assert_eq!(out[(10 * w + 10) * 3], 0.0);
    // Gain scales the vectors.
    let out = warp(&img, w, h, &field, 0.5);
    assert_eq!(out[(10 * w + 12) * 3], 255.0);
}

#[test]
fn zero_motion_keeps_the_picture() {
    let (w, h) = (40, 24);
    let img: Vec<f32> = (0..w * h * 3).map(|i| (i % 251) as f32).collect();
    let field = Field { block: 16, cols: 3, rows: 2, mvs: vec![Mv::default(); 6] };
    assert_eq!(warp(&img, w, h, &field, 1.0), img);
}

#[test]
fn estimation_is_deterministic() {
    let (prev, cur) = (texture(96, 80, 0.0, 0.0), texture(96, 80, 1.5, 3.25));
    let s = Settings::default();
    assert_eq!(estimate(&prev, &cur, &s).mvs, estimate(&prev, &cur, &s).mvs);
}
