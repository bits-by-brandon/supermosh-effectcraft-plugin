//! `fuel-probe WASM WIDTH HEIGHT [id=value ...]`: render one frame of a test pattern with the
//! plug-in the way EffectCraft's host does (wasmi, fuel metering) and print the fuel it used,
//! per pixel, next to the host's budget (50 M + 20 000 per pixel).

use wasmi::{Config, Engine, Linker, Module, Store};

const FUEL_BASE: u64 = 50_000_000;
const FUEL_PER_PIXEL: u64 = 20_000;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: fuel-probe WASM WIDTH HEIGHT [id=value ...]");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[0]).expect("read wasm");
    let (w, h): (usize, usize) = (args[1].parse().expect("width"), args[2].parse().expect("height"));
    let mut params = supermosh_glide::params::default_values();
    for kv in &args[3..] {
        let (k, v) = kv.split_once('=').expect("id=value");
        let at = supermosh_glide::params::offset(k).unwrap_or_else(|| panic!("no parameter `{k}`"));
        params[at] = v.parse().expect("number");
    }
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, &bytes).expect("module");
    let mut store = Store::new(&engine, ());
    let inst = Linker::<()>::new(&engine).instantiate_and_start(&mut store, &module).expect("instantiate");
    let memory = inst.get_memory(&store, "memory").expect("memory");
    let alloc = inst.get_typed_func::<i32, i32>(&store, "ec_alloc").expect("ec_alloc");
    let render = inst.get_typed_func::<(i32, i32, i32, i32, i32, f64, f64), i32>(&store, "ec_render").expect("ec_render");
    let n = w * h;
    let total = params.len() * 8 + n * 16;
    store.set_fuel(u64::MAX / 2).expect("fuel");
    let base = alloc.call(&mut store, total as i32).expect("alloc") as usize;
    let mut buf = Vec::with_capacity(total);
    for v in &params {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    for i in 0..n {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        for c in [(x * 0.37).sin() * 0.5 + 0.5, (y * 0.21).cos() * 0.5 + 0.5, ((x + y) * 0.05).fract(), 1.0f32] {
            buf.extend_from_slice(&c.to_le_bytes());
        }
    }
    memory.write(&mut store, base, &buf).expect("write");
    let before = store.get_fuel().expect("fuel");
    let t = std::time::Instant::now();
    let r = render.call(&mut store, ((base + params.len() * 8) as i32, w as i32, h as i32, base as i32, params.len() as i32, 0.0, 1.0)).expect("render");
    let used = before - store.get_fuel().expect("fuel");
    let budget = FUEL_BASE + FUEL_PER_PIXEL * n as u64;
    println!(
        "{w}x{h} result={r} fuel={used} per_px={:.1} budget_per_px={:.1} {} {:.2}s",
        used as f64 / n as f64,
        budget as f64 / n as f64,
        if used <= budget { "OK" } else { "OVER" },
        t.elapsed().as_secs_f64()
    );
}
