//! Map-facing ocean controls. Distances and speeds use JAPRO map units (40/m).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OceanAuthoring {
    pub amplitude: f32,
    pub wavelength: f32,
    pub speed: f32,
    pub direction: f32,
    pub steepness: f32,
    pub slosh: f32,
    pub seed: u32,
    pub wind_chop: f32,
    pub foam: f32,
    pub foam_lifetime: f32,
    pub spray: f32,
}

impl Default for OceanAuthoring {
    fn default() -> Self {
        Self { amplitude: 128.0, wavelength: 1400.0, speed: 180.0,
            direction: 0.0, steepness: 0.65, slosh: 1.0, seed: 85619173,
            wind_chop: 1.0, foam: 1.0, foam_lifetime: 2.0, spray: 1.0 }
    }
}

fn finite(value: f32, default: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() { value.clamp(min, max) } else { default }
}

impl OceanAuthoring {
    pub fn sanitize(mut self) -> Self {
        let d = Self::default();
        self.amplitude = finite(self.amplitude, d.amplitude, 0.0, 16384.0);
        self.wavelength = finite(self.wavelength, d.wavelength, 64.0, 32768.0);
        self.speed = finite(self.speed, d.speed, -4096.0, 4096.0);
        self.direction = finite(self.direction, 0.0, -360.0, 360.0);
        self.steepness = finite(self.steepness, d.steepness, 0.0, 1.0);
        self.slosh = finite(self.slosh, d.slosh, 0.0, 1.0);
        self.wind_chop = finite(self.wind_chop, 1.0, 0.0, 4.0);
        self.foam = finite(self.foam, 1.0, 0.0, 4.0);
        self.foam_lifetime = finite(self.foam_lifetime, 2.0, 0.1, 30.0);
        self.spray = finite(self.spray, 1.0, 0.0, 4.0);
        self
    }
}

/// Which swell controls follow the master amplitude. Locked controls are
/// recomputed from `derive`; unlocked ones keep their own value.
pub mod links {
    pub const WAVELENGTH: u8 = 1;
    pub const SPEED: u8 = 1 << 1;
    pub const STEEPNESS: u8 = 1 << 2;
    pub const WIND_CHOP: u8 = 1 << 3;
    pub const FOAM: u8 = 1 << 4;
    pub const SPRAY: u8 = 1 << 5;
    pub const ROUGHNESS: u8 = 1 << 6;
    pub const ALL: u8 = 0x7f;
}

/// Everything that follows the swell amplitude. At the default amplitude (128u)
/// every value equals its GodotOceanWaves default, so the default sea is the
/// upstream sea (roughness excepted, see below); smaller seas get shorter,
/// slower, gentler, less foamy and glossier waves, larger seas the opposite.
///
/// Roughness deliberately does NOT use upstream's 0.65. That value only works
/// in the demo because its normals fade out with distance; here it spreads the
/// sun's specular lobe into a whitish veil over the whole sea and starves
/// reflections (the Fresnel curve also depends on it). A resolved sea needs
/// little roughness: 0.2 at 128u, glassy for ponds, a little rougher for storms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DerivedWaves {
    pub wavelength: f32,
    pub speed: f32,
    pub steepness: f32,
    pub wind_chop: f32,
    pub foam: f32,
    pub spray: f32,
    pub roughness: f32,
}

pub fn derive_from_amplitude(amplitude: f32) -> DerivedWaves {
    let amplitude = if amplitude.is_finite() { amplitude.clamp(0.0, 16384.0) } else { 128.0 };
    let r = amplitude / 128.0;
    // Wavelength grows slower than height, so small seas are also steeper
    // relative to their size, as real chop is.
    let wavelength = (1400.0 * r.powf(0.7)).clamp(256.0, 32768.0);
    // Deep-water dispersion: phase speed grows with the square root of length.
    let speed = 180.0 * (wavelength / 1400.0).sqrt();
    let steepness = (0.65 * r.max(1.0e-3).powf(0.25)).clamp(0.2, 1.0);
    let wind_chop = r.max(1.0e-3).powf(0.25).clamp(0.3, 2.0);
    let foam = foam_for_amplitude(amplitude);
    // Preserve spray's existing response; changing foam must not silently
    // retune a different visual effect.
    let spray = (0.05 + 0.95 * (amplitude / 64.0).min(1.0)).clamp(0.0, 4.0);
    let roughness = 0.05 + 0.15 * r.clamp(0.0, 1.0) + 0.1 * ((r - 1.0) / 3.0).clamp(0.0, 1.0);
    DerivedWaves { wavelength, speed, steepness, wind_chop, foam, spray, roughness }
}

/// Continuous foam-amplitude response in JKA units: 0 is strictly foam-free,
/// even 0.001u remains positive, and the authored 128u sea remains at 1.0.
/// This is a source/visibility multiplier, not a substitute for wave breaking.
pub fn foam_for_amplitude(amplitude: f32) -> f32 {
    let amplitude = if amplitude.is_finite() { amplitude.max(0.0) } else { 0.0 };
    (1.25 * amplitude / (amplitude + 32.0)).clamp(0.0, 1.25)
}

#[cfg(test)]
mod foam_curve_tests {
    use super::foam_for_amplitude;

    #[test]
    fn no_minimum_threshold_and_original_default() {
        assert_eq!(foam_for_amplitude(0.0), 0.0);
        assert_eq!(foam_for_amplitude(-10.0), 0.0);
        assert_eq!(foam_for_amplitude(128.0), 1.0);
        assert!(foam_for_amplitude(0.001) > 0.0);
        assert!(foam_for_amplitude(0.001) < foam_for_amplitude(1.0));
        assert!(foam_for_amplitude(1.0) < foam_for_amplitude(32.0));
        assert!(foam_for_amplitude(32.0) < foam_for_amplitude(128.0));
        assert!(foam_for_amplitude(256.0) > foam_for_amplitude(128.0));
    }
}

impl OceanAuthoring {
    /// Overwrites the linked controls with their amplitude-derived values.
    pub fn apply_links(&mut self, mask: u8) {
        let d = derive_from_amplitude(self.amplitude);
        if mask & links::WAVELENGTH != 0 { self.wavelength = d.wavelength; }
        if mask & links::SPEED != 0 { self.speed = d.speed; }
        if mask & links::STEEPNESS != 0 { self.steepness = d.steepness; }
        if mask & links::WIND_CHOP != 0 { self.wind_chop = d.wind_chop; }
        if mask & links::FOAM != 0 { self.foam = d.foam; }
        if mask & links::SPRAY != 0 { self.spray = d.spray; }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtmosphericWind {
    pub speed: f32,
    pub direction: f32,
    pub gust: f32,
    pub shift: f32,
}

impl Default for AtmosphericWind {
    fn default() -> Self { Self { speed: 180.0, direction: 0.0, gust: 0.2, shift: 0.0 } }
}

impl AtmosphericWind {
    pub fn sanitize(mut self) -> Self {
        self.speed = finite(self.speed, 180.0, 0.0, 8192.0);
        self.direction = finite(self.direction, 0.0, -360.0, 360.0);
        self.gust = finite(self.gust, 0.2, 0.0, 1.0);
        self.shift = finite(self.shift, 0.0, 0.0, 180.0);
        self
    }

    /// Same continuous gust and shift functions as BG_DynamicWeatherWindVector.
    pub fn at(self, seconds: f32) -> [f32; 2] {
        let gust = (seconds * 0.83).sin() * 0.65 + (seconds * 1.71 + 1.9).sin() * 0.35;
        let shift = (seconds * 0.13).sin() * 0.7 + (seconds * 0.047 + 2.4).sin() * 0.3;
        let angle = (self.direction + self.shift * shift).to_radians();
        let speed = self.speed * (1.0 + self.gust * gust);
        [angle.cos() * speed, angle.sin() * speed]
    }
}

/// Backward-compatible name for older ocean-authoring callers. New code should
/// use `AtmosphericWind`: this wind is shared by ocean, grass, precipitation,
/// fog and wind-visualization systems.
pub type OceanWind = AtmosphericWind;

// JAPRO bg_public.h: CS_BSP_MODELS (1612) + 32 sub-BSPs + 16 runtime BSPs.
pub const CS_OCEANS: u16 = 1660;
pub const CS_WEATHER: u16 = CS_OCEANS + 8;

/// `sailingdebug` is on (the renderer then publishes `SAILING_DEBUG_DRAWN`).
pub static SAILING_DEBUG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Camera position (JKA) and the rendered water height there (probe), or None
/// when the camera is not over a Hybrid-classified pool.
pub static SAILING_DEBUG_DRAWN: std::sync::Mutex<Option<([f32; 3], Option<f32>)>> = std::sync::Mutex::new(None);
/// Blue Water's per-tile significant heights (m: big, medium, small) and the
/// Hybrid FFT blend they are drawn at, from the shared ocean.
pub static SAILING_DEBUG_TILES: std::sync::Mutex<Option<([f32; 3], f32)>> = std::sync::Mutex::new(None);

/// `SAILING_OCEAN_SPEED_AUTO`: authored speed that derives from the wavelength.
pub const OCEAN_SPEED_AUTO: f32 = -1.0;
/// `SAILING_WAVE_GRAVITY`: 9.81 m/s² at 40 map units per metre.
pub const OCEAN_WAVE_GRAVITY: f32 = 392.4;

/// `BG_SailingResolveOceanSpeed`: the phase speed an authored speed means.
pub fn resolve_ocean_speed(speed_authored: f32, wavelength: f32) -> f32 {
    if speed_authored == OCEAN_SPEED_AUTO {
        (OCEAN_WAVE_GRAVITY * wavelength.clamp(64.0, 32768.0) / std::f32::consts::TAU).sqrt()
    } else {
        speed_authored.clamp(-4096.0, 4096.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuthoredOcean {
    pub index: usize,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub height: f32,
    pub current_scale: f32,
    /// `waves.speed` is resolved; this is what was authored (-1 = auto).
    pub speed_authored: f32,
    pub waves: OceanAuthoring,
    /// Always 0: jaPRO's map wind is `misc_dynamic_weather` 0 (`CS_WEATHER`).
    pub weather: usize,
    pub wind: AtmosphericWind,
    /// `wind` is the map's (server `CS_WEATHER` or BSP `misc_dynamic_weather`),
    /// not a placeholder for the local atmospheric wind.
    pub server_wind: bool,
    pub rain: f32,
    pub wave_count: u32,
    pub horizontal_scale: f32,
}

impl AuthoredOcean {
    pub fn from_bsp(bsp: &jka_assets::bsp::Bsp) -> Vec<Self> {
        fn number(e: &jka_assets::bsp::Entity, key: &str, default: f32) -> f32 {
            e.get(key.as_bytes()).and_then(|s| std::str::from_utf8(s).ok())
                .and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(default)
        }
        fn vector(e: &jka_assets::bsp::Entity, key: &str) -> [f32;3] {
            let values: Vec<f32> = e.get(key.as_bytes()).and_then(|s| std::str::from_utf8(s).ok())
                .unwrap_or("").split_whitespace().filter_map(|s| s.parse().ok()).collect();
            if values.len() == 3 && values.iter().all(|v| v.is_finite()) { [values[0],values[1],values[2]] } else { [0.0;3] }
        }
        let weather: Vec<_> = bsp.entities.iter().filter(|e| e.get(b"classname") == Some(b"misc_dynamic_weather".as_slice())).collect();
        let mut result = Vec::new();
        for e in &bsp.entities {
            if e.get(b"classname") != Some(b"misc_sailing_ocean".as_slice()) || result.len() >= 8 { continue; }
            let Some(model) = e.get(b"model").and_then(|s| std::str::from_utf8(s).ok())
                .and_then(|s| s.strip_prefix('*')).and_then(|s| s.parse::<usize>().ok()).and_then(|i| bsp.models.get(i)) else { continue; };
            let origin = vector(e, "origin");
            let mins = std::array::from_fn(|i| model.mins[i] + origin[i]);
            let maxs = std::array::from_fn(|i| model.maxs[i] + origin[i]);
            // -1 follows the wavelength (`resolve_ocean_speed`).
            let speed_authored = number(e, "speed", 180.0).clamp(-4096.0, 4096.0);
            let wavelength = number(e, "wavelength", 1400.0).clamp(64.0, 32768.0);
            let mut ocean = Self {
                index: result.len(), mins, maxs, height: number(e,"waterHeight",maxs[2]),
                current_scale:number(e,"currentScale",0.25).clamp(-2.0,2.0), speed_authored, weather:0,
                wind:AtmosphericWind::default(), server_wind:false, rain:number(e,"rainIntensity",0.0).clamp(0.0,1.0),
                wave_count:number(e,"gerstnerWaveCount",8.0) as u32, horizontal_scale:number(e,"gerstnerHorizontalScale",1.0),
                waves: OceanAuthoring {
                    amplitude:number(e,"amplitude",128.0), wavelength,
                    speed:resolve_ocean_speed(speed_authored, wavelength), direction:number(e,"waveAngle",0.0),
                    steepness:number(e,"steepness",0.65), slosh:number(e,"slosh",0.0),
                    seed:e.get(b"waveSeed").and_then(|s| std::str::from_utf8(s).ok()).and_then(|s| s.parse().ok()).unwrap_or(85619173),
                    wind_chop:number(e,"windChop",1.0),foam:number(e,"foamAmount",1.0),
                    foam_lifetime:number(e,"foamLifetime",2.0),spray:number(e,"sprayAmount",1.0),
                }.sanitize(),
            };
            if let Some(w) = weather.first() {
                ocean.wind = AtmosphericWind {speed:number(w,"windSpeed",180.0), direction:number(w,"windAngle",0.0),
                    gust:number(w,"windGust",0.2),shift:number(w,"windShift",0.0)}.sanitize();
                ocean.server_wind = true;
            }
            result.push(ocean);
        }
        result
    }

    /// jaPRO `BG_SailingBuildOceanWeatherString`: mins[3] maxs[3] baseHeight
    /// amplitude wavelength speedAuthored steepness slosh waveDirection[2]
    /// currentScale rainIntensity waveSeed gerstnerWaveCount
    /// horizontalDisplacementScale. Any other count or a non-finite value is
    /// rejected. Wind is not here: it is `CS_WEATHER` (`apply_weather`).
    pub fn parse(index: usize, bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let words: Vec<_> = text.split_whitespace().collect();
        if words.len() != 19 { return None; }
        let float = |i: usize| words[i].parse::<f32>().ok().filter(|v| v.is_finite());
        let v: Vec<f32> = (0..16).map(float).collect::<Option<_>>()?;
        let horizontal_scale = float(18)?;
        let (mins, maxs) = ([v[0], v[1], v[2]], [v[3], v[4], v[5]]);
        if mins[0] >= maxs[0] || mins[1] >= maxs[1] || mins[2] > maxs[2] { return None; }
        let wavelength = v[8].clamp(64.0, 32768.0);
        let speed_authored = v[9].clamp(-4096.0, 4096.0);
        let amplitude = v[7];
        // The wire has no foam/spray/chop controls; derive them from the sea
        // state as the linked UI controls do (identical to defaults at 128u).
        let derived = derive_from_amplitude(amplitude);
        let seed = words[16].parse::<u32>().ok()?;
        Some(Self {
            index, mins, maxs, height: v[6],
            current_scale: v[14].clamp(-2.0, 2.0),
            speed_authored,
            waves: OceanAuthoring {
                amplitude, wavelength, speed: resolve_ocean_speed(speed_authored, wavelength),
                direction: v[13].atan2(v[12]).to_degrees(), steepness: v[10], slosh: v[11],
                seed: if seed == 0 { 85619173 } else { seed },
                wind_chop: derived.wind_chop, foam: derived.foam, foam_lifetime: 2.0, spray: derived.spray,
            }.sanitize(),
            wave_count: words[17].parse::<i32>().ok()?.clamp(2, 12) as u32,
            horizontal_scale: horizontal_scale.clamp(0.0, 1.0),
            weather: 0, wind: AtmosphericWind::default(), server_wind: false, rain: v[15].clamp(0.0, 1.0),
        })
    }

    /// Drive Hybrid (`r_oceanType 3`) from this map ocean without changing how
    /// Hybrid draws waves: its analytic swell is the map's primary Gerstner wave,
    /// `slosh` is the analytic/FFT blend (jaPRO: 0 one coherent swell, 1 the
    /// full directional spectrum), and the Blue Water FFT carries the full-
    /// spectrum height, peak wavelength, heading, chop and the map's wind.
    ///
    /// Every value comes from `sea` (jaPRO `BG_SailingSea`, the same code the
    /// server and prediction float in); the FFT's two long tiles are replaced
    /// by `sea.waves` (`RenderCommand::SetSailingSea`), so the drawn long and
    /// medium waves are the physics waves.
    pub fn apply_to_hybrid(&self, sea: &jka_movement::SailingSea, settings: &mut super::OceanSettings) {
        settings.hybrid_swell = OceanAuthoring {
            amplitude: sea.swell_amplitude,
            wavelength: sea.swell_wavelength,
            speed: sea.swell_speed,
            steepness: sea.swell_steepness,
            ..self.waves
        };
        settings.hybrid_swell_absolute = true;
        settings.hybrid_fft_blend = sea.fft_blend;
        settings.blue_water_waves = super::BlueWaterWaves {
            height: sea.significant_height,
            direction: sea.direction_degrees,
            wavelength: sea.peak_wavelength,
            choppiness: sea.choppiness,
            wind_speed: sea.wind_speed,
        };
    }

    /// jaPRO `BG_DynamicWeatherBuildString`: windDirection[2] windSpeed
    /// windGust windShift cloudCoverage cloudType cloudDensity cloudSpeedScale
    /// cloudBottom cloudTop. Only the wind is ocean state.
    pub fn apply_weather(&mut self, bytes: &[u8]) -> Option<()> {
        self.wind = parse_weather_wind(bytes)?;
        self.server_wind = true;
        Some(())
    }
}

/// jaPRO sailing markers for the prediction backend, in entity-lump order (the
/// jaPRO cgame's `CG_ParseEntitiesFromString` order). Brush bounds are the
/// inline model's, as the cgame's `R_ModelBounds` returns them.
pub fn sailing_spawns(bsp: &jka_assets::bsp::Bsp) -> Vec<jka_movement::SailingSpawn> {
    bsp.entities.iter().filter_map(|e| {
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let spawn = jka_movement::SailingSpawn {
            pairs: e.properties.iter().map(|(k, v)| (text(k), text(v))).collect(),
            model_bounds: e.get(b"model").and_then(|s| std::str::from_utf8(s).ok())
                .and_then(|s| s.strip_prefix('*')).and_then(|s| s.parse::<usize>().ok())
                .and_then(|i| bsp.models.get(i)).map(|m| (m.mins, m.maxs)),
        };
        spawn.is_sailing_marker().then_some(spawn)
    }).collect()
}

/// The wind of a jaPRO `CS_WEATHER` string (11 fields), or `None`.
pub fn parse_weather_wind(bytes: &[u8]) -> Option<AtmosphericWind> {
    let v: Vec<f32> = std::str::from_utf8(bytes).ok()?.split_whitespace()
        .map(str::parse).collect::<Result<_, _>>().ok()?;
    if v.len() != 11 || v.iter().any(|v| !v.is_finite()) { return None; }
    Some(AtmosphericWind { direction: v[1].atan2(v[0]).to_degrees(), speed: v[2], gust: v[3], shift: v[4] }.sanitize())
}

impl AuthoredOcean {

    pub fn contains_render_point(&self, p: [f32;3]) -> bool {
        p[0] >= self.mins[0] && p[0] <= self.maxs[0]
            && -p[2] >= self.mins[1] && -p[2] <= self.maxs[1]
            && (p[1] - self.height).abs() < 64.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calm_wind_stays_calm_and_invalid_controls_are_finite() {
        let wind = AtmosphericWind { speed: 0.0, gust: 1.0, shift: 180.0, ..Default::default() };
        assert_eq!(wind.at(123.0), [0.0, 0.0]);
        let ocean = OceanAuthoring { amplitude: f32::NAN, wavelength: 0.0, ..Default::default() }.sanitize();
        assert_eq!(ocean.amplitude, 128.0);
        assert_eq!(ocean.wavelength, 64.0);
    }
    #[test]
    fn ocean_wire_is_strict_and_wind_does_not_change_swell() {
        // As jaPRO's BG_SailingBuildOceanWeatherString writes it (%.9g).
        let wire = b"-100 -200 -50 100 200 0 0 128 1400 -1 0.649999976 1 0.939692616 0.342020154 0.25 0.349999994 4294967295 8 1";
        let mut ocean = AuthoredOcean::parse(0, wire).unwrap();
        assert_eq!(ocean.waves.seed, u32::MAX);
        assert_eq!((ocean.mins, ocean.maxs, ocean.height), ([-100.0, -200.0, -50.0], [100.0, 200.0, 0.0], 0.0));
        assert_eq!(ocean.speed_authored, OCEAN_SPEED_AUTO);
        // Auto speed is the deep-water phase speed of 1400u at 40 u/m.
        assert!((ocean.waves.speed - (392.4f32 * 1400.0 / std::f32::consts::TAU).sqrt()).abs() < 1e-3);
        assert!((ocean.waves.direction - 20.0).abs() < 1e-4);
        assert!((ocean.rain - 0.35).abs() < 1e-6);
        let waves = ocean.waves;
        ocean.apply_weather(b"0 1 800 0.4 15 0.5 0.5 1 2 6000 14000").unwrap();
        assert_eq!(ocean.waves, waves);
        assert_eq!(ocean.wind.direction, 90.0);
        assert_eq!(ocean.wind.speed, 800.0);
        assert!(AuthoredOcean::parse(0, &wire[..wire.len() - 2]).is_none());
        assert!(ocean.apply_weather(b"1 0 NaN 0 0 0 0 0 0 0 0").is_none());
        // A literal speed stays literal.
        let fixed = AuthoredOcean::parse(0, b"0 0 0 1 1 0 0 96 1200 220 0.7 0.7 1 0 0.1 0 1 8 1").unwrap();
        assert_eq!(fixed.waves.speed, 220.0);
    }
}
