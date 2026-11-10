//! jaPRO sailing state for the jaPRO prediction backend (`native/japro_sailing.c`).
//!
//! Predicted swimming must run the server's own analytic ocean, never the
//! rendered FFT. Static data comes from the client's BSP entity lump, exactly as
//! the jaPRO cgame reads it; per-ocean runtime state from `CS_SAILING_OCEANS`.
use crate::ffi::{self, NativeGuard};
use std::ffi::{c_char, CString};

/// One BSP entity, in entity-lump order, with its inline brush model bounds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SailingSpawn {
    pub pairs: Vec<(String, String)>,
    pub model_bounds: Option<([f32; 3], [f32; 3])>,
}

impl SailingSpawn {
    /// Whether the jaPRO cgame would hand this entity to the sailing BG state.
    pub fn is_sailing_marker(&self) -> bool {
        self.pairs.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("classname")
                && ["misc_sailing_ocean", "misc_sailing_shore", "misc_sailing_current", "misc_dynamic_weather"]
                    .iter()
                    .any(|c| v.eq_ignore_ascii_case(c))
        })
    }
}

/// Drop every ocean, shore and current (map change or disconnect).
pub fn sailing_reset() {
    let _guard = NativeGuard::new();
    // SAFETY: serialized by the native lock; clears static BG tables.
    unsafe { ffi::jka_sailing_reset() };
}

/// Replace the sailing BG state with this map's markers. Ocean indices follow
/// the entity order, which is the server's spawn order for the same BSP.
pub fn sailing_load(entities: &[SailingSpawn]) {
    let _guard = NativeGuard::new();
    // SAFETY: serialized by the native lock.
    unsafe { ffi::jka_sailing_reset() };
    for entity in entities.iter().filter(|e| e.is_sailing_marker()) {
        let owned: Vec<(CString, CString)> = entity
            .pairs
            .iter()
            .filter_map(|(k, v)| Some((CString::new(k.as_str()).ok()?, CString::new(v.as_str()).ok()?)))
            .collect();
        let keys: Vec<*const c_char> = owned.iter().map(|(k, _)| k.as_ptr()).collect();
        let values: Vec<*const c_char> = owned.iter().map(|(_, v)| v.as_ptr()).collect();
        let (mins, maxs) = entity
            .model_bounds
            .as_ref()
            .map_or((std::ptr::null(), std::ptr::null()), |(a, b)| (a.as_ptr(), b.as_ptr()));
        // SAFETY: the key/value pointers live in `owned` for the call; the native
        // side copies what it keeps into BG structs before returning.
        unsafe { ffi::jka_sailing_spawn(keys.as_ptr(), values.as_ptr(), keys.len() as i32, mins, maxs) };
    }
}

/// Apply `CS_SAILING_OCEANS + index`. False when rejected (no such ocean, or
/// a malformed string), in which case the ocean keeps its previous state.
pub fn sailing_apply_ocean_state(index: usize, state: &[u8]) -> bool {
    let Ok(state) = CString::new(state) else { return false };
    let _guard = NativeGuard::new();
    // SAFETY: serialized by the native lock; `state` outlives the call.
    unsafe { ffi::jka_sailing_apply_ocean_state(index as i32, state.as_ptr()) != 0 }
}

/// Apply `CS_DYNAMIC_WEATHER + index` (the map's wind, which the sea's
/// spectrum reads). False when rejected.
pub fn sailing_apply_weather_state(index: usize, state: &[u8]) -> bool {
    let Ok(state) = CString::new(state) else { return false };
    let _guard = NativeGuard::new();
    // SAFETY: serialized by the native lock; `state` outlives the call.
    unsafe { ffi::jka_sailing_apply_weather_state(index as i32, state.as_ptr()) != 0 }
}

/// `SAILING_SEA_MAX_WAVES`: two physics tiles of 128 waves.
pub const SAILING_SEA_MAX_WAVES: usize = 256;

/// One physics wave as an FFT lattice entry: the texel at signed index
/// (`nx`, `nz`) of tile `cascade` holds (`re`, `im`, `m`), its twin at
/// (-`nx`, -`nz`) the conjugate and -`m`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SailingSeaWave {
    pub cascade: u32,
    pub nx: i32,
    pub nz: i32,
    pub re: f32,
    pub im: f32,
    pub m: f32,
}

/// `BG_SailingSea`: the sea physics floats in, which the Hybrid ocean draws.
/// Lengths in metres in Blue Water's frame except the swell (map units).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SailingSea {
    pub significant_height: f32,
    pub direction_degrees: f32,
    pub peak_wavelength: f32,
    pub choppiness: f32,
    pub wind_speed: f32,
    pub tile_sizes: [f32; 3],
    pub fft_blend: f32,
    pub swell_amplitude: f32,
    pub swell_wavelength: f32,
    pub swell_speed: f32,
    pub swell_steepness: f32,
    pub waves: Vec<SailingSeaWave>,
}

/// Ocean `index`'s sea, built (or rebuilt after an ocean/weather change) by
/// the native BG code, or None without that ocean.
pub fn sailing_sea(index: usize) -> Option<SailingSea> {
    let mut params = [0.0f32; 13];
    let mut raw = vec![0.0f32; SAILING_SEA_MAX_WAVES * 6];
    let _guard = NativeGuard::new();
    // SAFETY: serialized by the native lock; writes 13 params and at most
    // SAILING_SEA_MAX_WAVES * 6 floats.
    let count = unsafe {
        ffi::jka_sailing_sea(index as i32, params.as_mut_ptr(), raw.as_mut_ptr(), SAILING_SEA_MAX_WAVES as i32)
    };
    if count < 0 {
        return None;
    }
    let waves = raw.chunks_exact(6).take(count as usize).map(|w| SailingSeaWave {
        cascade: w[0] as u32, nx: w[1] as i32, nz: w[2] as i32, re: w[3], im: w[4], m: w[5],
    }).collect();
    Some(SailingSea {
        significant_height: params[0], direction_degrees: params[1], peak_wavelength: params[2],
        choppiness: params[3], wind_speed: params[4], tile_sizes: [params[5], params[6], params[7]],
        fft_blend: params[8], swell_amplitude: params[9], swell_wavelength: params[10],
        swell_speed: params[11], swell_steepness: params[12], waves,
    })
}

/// The predicted analytic surface height at (x, y) and server time `time_ms`,
/// or `None` outside every ocean volume.
pub fn sailing_wave_height(x: f32, y: f32, time_ms: i32) -> Option<f32> {
    let mut height = 0.0f32;
    let _guard = NativeGuard::new();
    // SAFETY: serialized by the native lock; writes one float.
    (unsafe { ffi::jka_sailing_wave_height(x, y, time_ms, &mut height) } != 0).then_some(height)
}
