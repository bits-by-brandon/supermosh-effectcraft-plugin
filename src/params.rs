//! The effect's parameters: one table ([`PARAMS`]) that both the JSON manifest and the value
//! parser are built from, so the order the host flattens values in can't drift from the order we
//! read them in.

use std::fmt::Write as _;

pub enum Kind {
    Slider { default: f64, min: f64, max: f64, slider_min: f64, slider_max: f64, decimals: u8 },
    Angle { default: f64 },
    Checkbox { default: bool },
    Popup { options: &'static [&'static str], default: u32 },
    Point { default: [f64; 2] },
    Color { default: [f64; 4] },
}

impl Kind {
    /// How many `f64`s the host flattens this parameter to.
    pub const fn width(&self) -> usize {
        match self {
            Kind::Point { .. } => 2,
            Kind::Color { .. } => 4,
            _ => 1,
        }
    }
}

pub struct Param {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
}

#[allow(clippy::too_many_arguments)]
const fn slider(id: &'static str, name: &'static str, default: f64, min: f64, max: f64, slider_min: f64, slider_max: f64, decimals: u8) -> Param {
    Param { id, name, kind: Kind::Slider { default, min, max, slider_min, slider_max, decimals } }
}
const fn popup(id: &'static str, name: &'static str, options: &'static [&'static str], default: u32) -> Param {
    Param { id, name, kind: Kind::Popup { options, default } }
}
const fn checkbox(id: &'static str, name: &'static str, default: bool) -> Param {
    Param { id, name, kind: Kind::Checkbox { default } }
}

/// The most entries the shift list can have.
pub const MAX_SHIFTS: usize = 16;

/// Supermosh's default `xyShifts` (`[0, 1, -1, 2, -2, 4, -4, 8, -8]`), then more values for
/// longer lists.
pub const DEFAULT_SHIFTS: [f64; MAX_SHIFTS] = [0.0, 1.0, -1.0, 2.0, -2.0, 4.0, -4.0, 8.0, -8.0, 16.0, -16.0, 32.0, -32.0, 3.0, -3.0, 6.0];

macro_rules! shift {
    ($n:literal) => {
        slider(concat!("shift_", $n), concat!("Shift ", $n), DEFAULT_SHIFTS[$n - 1], -1024.0, 1024.0, -64.0, 64.0, 0)
    };
}

pub const STEP_SOURCES: &[&str] = &["Time", "Steps Slider"];
pub const AFTER_GLIDE: &[&str] = &["Hold Last Step", "Show Source"];
pub const MOTION_SOURCES: &[&str] = &["Random (Shift List)", "Luma Gradient", "Hue Direction", "Uniform Direction"];
pub const EDGES: &[&str] = &["Wrap", "Clamp", "Mirror", "Transparent"];
pub const VIEWS: &[&str] = &["Result", "Motion Field", "Result + Grid"];

pub const PARAMS: &[Param] = &[
    // Timing (Supermosh: a glide segment's `time` and `length`, and `config.fps`).
    popup("step_source", "Steps From", STEP_SOURCES, 0),
    slider("start", "Glide Start (s)", 0.0, -86400.0, 86400.0, 0.0, 30.0, 2),
    slider("length", "Glide Length (s, 0 = endless)", 0.0, 0.0, 86400.0, 0.0, 30.0, 2),
    popup("after", "After Glide", AFTER_GLIDE, 0),
    slider("rate", "Steps per Second", 30.0, 0.0, 1000.0, 0.0, 120.0, 2),
    slider("steps", "Steps", 0.0, 0.0, 100000.0, 0.0, 300.0, 2),
    checkbox("blend", "Blend Between Steps", false),
    slider("max_steps", "Max Steps", 1000.0, 1.0, 100000.0, 1.0, 5000.0, 0),
    // Blocks (Supermosh: `config.size`).
    slider("block_w", "Block Width", 16.0, 1.0, 1024.0, 1.0, 128.0, 0),
    slider("block_h", "Block Height", 16.0, 1.0, 1024.0, 1.0, 128.0, 0),
    Param { id: "grid_origin", name: "Grid Origin", kind: Kind::Point { default: [0.0, 0.0] } },
    // Shift list (Supermosh: `config.xyShifts`).
    slider("shift_count", "Shift List Length", 9.0, 1.0, MAX_SHIFTS as f64, 1.0, MAX_SHIFTS as f64, 0),
    shift!(1),
    shift!(2),
    shift!(3),
    shift!(4),
    shift!(5),
    shift!(6),
    shift!(7),
    shift!(8),
    shift!(9),
    shift!(10),
    shift!(11),
    shift!(12),
    shift!(13),
    shift!(14),
    shift!(15),
    shift!(16),
    slider("x_amount", "Horizontal Amount (%)", 100.0, -1000.0, 1000.0, -200.0, 200.0, 1),
    slider("y_amount", "Vertical Amount (%)", 100.0, -1000.0, 1000.0, -200.0, 200.0, 1),
    // Motion field.
    popup("motion", "Motion Source", MOTION_SOURCES, 0),
    slider("seed", "Random Seed", 0.0, 0.0, 1_000_000.0, 0.0, 1000.0, 0),
    slider("reseed", "Reseed Every N Steps (0 = never)", 0.0, 0.0, 100000.0, 0.0, 120.0, 0),
    Param { id: "direction", name: "Direction", kind: Kind::Angle { default: 0.0 } },
    slider("distance", "Distance (px)", 4.0, -1024.0, 1024.0, -32.0, 32.0, 1),
    checkbox("snap", "Snap to Shift List", false),
    slider("jitter", "Random Jitter (%)", 0.0, 0.0, 100.0, 0.0, 100.0, 1),
    slider("static", "Static Blocks (%)", 0.0, 0.0, 100.0, 0.0, 100.0, 1),
    // Output.
    popup("edges", "Edges", EDGES, 0),
    checkbox("force_opaque", "Force Opaque", false),
    slider("mix", "Mix (%)", 100.0, 0.0, 100.0, 0.0, 100.0, 1),
    popup("view", "View", VIEWS, 0),
    Param { id: "grid_color", name: "Grid Color", kind: Kind::Color { default: [1.0, 0.0, 1.0, 1.0] } },
];

/// Total flattened parameter count.
pub fn flat_len() -> usize {
    PARAMS.iter().map(|p| p.kind.width()).sum()
}

fn num(out: &mut String, v: f64) {
    // `{}` on an f64 prints `16` for 16.0 and never uses exponents for these magnitudes.
    let _ = write!(out, "{v}");
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The plug-in manifest (API v1) as JSON.
pub fn manifest_json() -> String {
    let mut o = String::with_capacity(8 * 1024);
    o.push_str("{\"api\":1,\"id\":");
    json_str(&mut o, crate::ID);
    o.push_str(",\"name\":");
    json_str(&mut o, crate::NAME);
    o.push_str(",\"category\":");
    json_str(&mut o, crate::CATEGORY);
    o.push_str(",\"version\":");
    json_str(&mut o, env!("CARGO_PKG_VERSION"));
    o.push_str(",\"author\":");
    json_str(&mut o, crate::AUTHOR);
    o.push_str(",\"description\":");
    json_str(&mut o, crate::DESCRIPTION);
    o.push_str(",\"params\":[");
    for (i, p) in PARAMS.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str("{\"id\":");
        json_str(&mut o, p.id);
        o.push_str(",\"name\":");
        json_str(&mut o, p.name);
        match &p.kind {
            Kind::Slider { default, min, max, slider_min, slider_max, decimals } => {
                o.push_str(",\"type\":\"slider\",\"default\":");
                num(&mut o, *default);
                o.push_str(",\"min\":");
                num(&mut o, *min);
                o.push_str(",\"max\":");
                num(&mut o, *max);
                o.push_str(",\"sliderMin\":");
                num(&mut o, *slider_min);
                o.push_str(",\"sliderMax\":");
                num(&mut o, *slider_max);
                let _ = write!(o, ",\"decimals\":{decimals}");
            }
            Kind::Angle { default } => {
                o.push_str(",\"type\":\"angle\",\"default\":");
                num(&mut o, *default);
            }
            Kind::Checkbox { default } => {
                let _ = write!(o, ",\"type\":\"checkbox\",\"default\":{default}");
            }
            Kind::Popup { options, default } => {
                o.push_str(",\"type\":\"popup\",\"options\":[");
                for (k, opt) in options.iter().enumerate() {
                    if k > 0 {
                        o.push(',');
                    }
                    json_str(&mut o, opt);
                }
                let _ = write!(o, "],\"default\":{default}");
            }
            Kind::Point { default } => {
                o.push_str(",\"type\":\"point\",\"default\":[");
                num(&mut o, default[0]);
                o.push(',');
                num(&mut o, default[1]);
                o.push(']');
            }
            Kind::Color { default } => {
                o.push_str(",\"type\":\"color\",\"default\":[");
                for (k, v) in default.iter().enumerate() {
                    if k > 0 {
                        o.push(',');
                    }
                    num(&mut o, *v);
                }
                o.push(']');
            }
        }
        o.push('}');
    }
    o.push_str("]}");
    o
}

/// Parameter values read from the host's flattened `f64`s, by id.
pub struct Values<'a> {
    flat: &'a [f64],
}

impl<'a> Values<'a> {
    pub fn new(flat: &'a [f64]) -> Option<Self> {
        (flat.len() == flat_len()).then_some(Values { flat })
    }

    fn slot(&self, id: &str) -> &'a [f64] {
        let mut at = 0;
        for p in PARAMS {
            let w = p.kind.width();
            if p.id == id {
                return &self.flat[at..at + w];
            }
            at += w;
        }
        // Ids are compile-time constants checked by the tests; a typo reads as zero.
        &[]
    }

    pub fn f(&self, id: &str) -> f64 {
        let v = self.slot(id).first().copied().unwrap_or(0.0);
        if v.is_finite() { v } else { 0.0 }
    }
    pub fn b(&self, id: &str) -> bool {
        self.f(id) != 0.0
    }
    pub fn index(&self, id: &str) -> usize {
        self.f(id).max(0.0) as usize
    }
    pub fn point(&self, id: &str) -> [f64; 2] {
        match self.slot(id) {
            [x, y] => [*x, *y],
            _ => [0.0; 2],
        }
    }
    pub fn color(&self, id: &str) -> [f64; 4] {
        match self.slot(id) {
            [r, g, b, a] => [*r, *g, *b, *a],
            _ => [1.0; 4],
        }
    }
}

/// The defaults, flattened, as the host would send them (points at the buffer's top-left).
pub fn default_values() -> Vec<f64> {
    let mut v = vec![];
    for p in PARAMS {
        match &p.kind {
            Kind::Slider { default, .. } | Kind::Angle { default } => v.push(*default),
            Kind::Checkbox { default } => v.push(if *default { 1.0 } else { 0.0 }),
            Kind::Popup { default, .. } => v.push(*default as f64),
            Kind::Point { default } => v.extend_from_slice(default),
            Kind::Color { default } => v.extend_from_slice(default),
        }
    }
    v
}

/// Index of parameter `id`'s first value in the flattened list (tests and tools).
pub fn offset(id: &str) -> Option<usize> {
    let mut at = 0;
    for p in PARAMS {
        if p.id == id {
            return Some(at);
        }
        at += p.kind.width();
    }
    None
}
