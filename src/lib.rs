//! Supermosh Glide: an EffectCraft effect plug-in (WebAssembly, plug-in API v1) porting the
//! block-motion "glide" datamosh of Supermosh (<https://github.com/supermosh/supermosh.github.io>,
//! `core/src/index.ts`, 2021) by Nino Filiu.
//!
//! Build with `cargo build --release --target wasm32-unknown-unknown`, then load
//! `target/wasm32-unknown-unknown/release/supermosh_glide.wasm` with Effect ▸ Load Effect
//! Plug-in… in EffectCraft, or copy it into the `Plug-ins` folder next to EffectCraft's settings.
//!
//! Copyright (C) 2021-2026 Nino Filiu (Supermosh) and the supermosh-effectcraft-plugin
//! contributors. This program is free software: you can redistribute it and/or modify it under
//! the terms of the GNU General Public License as published by the Free Software Foundation,
//! either version 3 of the License, or (at your option) any later version. See `LICENSE`.

pub mod glide;
pub mod params;

pub const ID: &str = "io.github.bits-by-brandon.supermosh-glide";
pub const NAME: &str = "Supermosh Glide";
pub const CATEGORY: &str = "Stylize";
pub const AUTHOR: &str = "Supermosh by Nino Filiu; EffectCraft port by bits-by-brandon";
pub const DESCRIPTION: &str =
    "Datamosh glide from Supermosh: blocks of the frame are shifted again and again, so the picture smears along a block motion field.";

/// Render with flattened parameter values, as the host sends them.
pub fn render_flat(px: &mut [[f32; 4]], width: usize, height: usize, flat: &[f64], time: f64, scale: f64) -> Result<(), &'static str> {
    let values = params::Values::new(flat).ok_or("wrong parameter count")?;
    let settings = glide::Settings::from_values(&values);
    glide::render(px, width, height, &settings, time, scale);
    Ok(())
}

#[cfg(target_arch = "wasm32")]
mod abi {
    use std::cell::RefCell;
    use std::sync::OnceLock;

    fn manifest() -> &'static str {
        static M: OnceLock<String> = OnceLock::new();
        M.get_or_init(crate::params::manifest_json)
    }

    thread_local! {
        static BUF: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn ec_api_version() -> i32 {
        1
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn ec_manifest_ptr() -> i32 {
        manifest().as_ptr() as i32
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn ec_manifest_len() -> i32 {
        manifest().len() as i32
    }

    /// One reusable, 8-byte-aligned buffer for the parameters and pixels.
    #[unsafe(no_mangle)]
    pub extern "C" fn ec_alloc(bytes: i32) -> i32 {
        BUF.with(|b| {
            let mut b = b.borrow_mut();
            let words = (bytes.max(0) as usize).div_ceil(8);
            if b.len() < words {
                b.resize(words, 0);
            }
            b.as_mut_ptr() as i32
        })
    }

    /// # Safety
    /// The host passes pointers into the buffer `ec_alloc` returned.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ec_render(pixels: i32, width: i32, height: i32, params: i32, nparams: i32, time: f64, scale: f64) -> i32 {
        if width <= 0 || height <= 0 || nparams < 0 {
            return 2;
        }
        let (w, h) = (width as usize, height as usize);
        let p = unsafe { std::slice::from_raw_parts(params as *const f64, nparams as usize) };
        let px = unsafe { std::slice::from_raw_parts_mut(pixels as *mut [f32; 4], w * h) };
        match crate::render_flat(px, w, h, p, time, scale) {
            Ok(()) => 0,
            Err(_) => 1,
        }
    }
}

#[cfg(test)]
mod tests;
