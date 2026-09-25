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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OceanWind {
    pub speed: f32,
    pub direction: f32,
    pub gust: f32,
    pub shift: f32,
}

impl Default for OceanWind {
    fn default() -> Self { Self { speed: 180.0, direction: 0.0, gust: 0.2, shift: 0.0 } }
}

impl OceanWind {
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

// JAPRO bg_public.h: CS_BSP_MODELS (1612) + 32 sub-BSPs + 16 runtime BSPs.
pub const CS_OCEANS: u16 = 1660;
pub const CS_WEATHER: u16 = CS_OCEANS + 8;

#[derive(Clone, Debug, PartialEq)]
pub struct AuthoredOcean {
    pub index: usize,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub height: f32,
    pub current_scale: f32,
    pub waves: OceanAuthoring,
    pub weather: usize,
    pub wind: OceanWind,
    pub rain: f32,
    pub wave_model: u32,
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
            let weather_index = number(e,"weather",0.0).clamp(0.0,1.0) as usize;
            let mut ocean = Self {
                index: result.len(), mins, maxs, height: number(e,"waterHeight",maxs[2]),
                current_scale:number(e,"currentScale",0.25), weather:weather_index,
                wind:OceanWind::default(), rain:0.0, wave_model:number(e,"waveModel",1.0) as u32,
                wave_count:number(e,"gerstnerWaveCount",8.0) as u32, horizontal_scale:number(e,"gerstnerHorizontalScale",1.0),
                waves: OceanAuthoring {
                    amplitude:number(e,"amplitude",128.0), wavelength:number(e,"wavelength",1400.0),
                    speed:number(e,"speed",180.0), direction:number(e,"waveAngle",0.0),
                    steepness:number(e,"steepness",0.65), slosh:number(e,"slosh",0.0),
                    seed:e.get(b"waveSeed").and_then(|s| std::str::from_utf8(s).ok()).and_then(|s| s.parse().ok()).unwrap_or(85619173),
                    wind_chop:number(e,"windChop",1.0),foam:number(e,"foamAmount",1.0),
                    foam_lifetime:number(e,"foamLifetime",2.0),spray:number(e,"sprayAmount",1.0),
                }.sanitize(),
            };
            if let Some(w) = weather.get(weather_index) {
                ocean.wind = OceanWind {speed:number(w,"windSpeed",180.0), direction:number(w,"windAngle",0.0),
                    gust:number(w,"windGust",0.2),shift:number(w,"windShift",0.0)}.sanitize();
                ocean.rain=number(w,"rainIntensity",0.0).clamp(0.0,1.0);
            }
            result.push(ocean);
        }
        result
    }

    /// One development format; incomplete, non-finite and old strings are rejected.
    pub fn parse(index: usize, bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let words: Vec<_> = text.split_whitespace().collect();
        if words.len() != 24 { return None; }
        let v: Vec<f32> = words.iter().map(|v| v.parse::<f32>()).collect::<Result<_, _>>().ok()?;
        if v.iter().any(|v| !v.is_finite()) { return None; }
        let weather = words[19].parse::<usize>().ok()?;
        if weather >= 2 || (0..3).any(|i| v[i] >= v[i+3]) { return None; }
        Some(Self {
            index, mins: [v[0],v[1],v[2]], maxs: [v[3],v[4],v[5]], height: v[6],
            current_scale: v[7].clamp(-2.0,2.0),
            waves: OceanAuthoring {
                amplitude: v[8], steepness: v[9], wavelength: v[10], slosh: v[11], speed: v[12],
                direction: v[14].atan2(v[13]).to_degrees(), seed: words[15].parse().ok()?,
                wind_chop: v[20], foam: v[21], foam_lifetime: v[22], spray: v[23],
            }.sanitize(),
            wave_count: words[16].parse::<u32>().ok()?.clamp(2, 12),
            horizontal_scale: v[17].clamp(0.0,1.0), wave_model: words[18].parse().ok()?,
            weather, wind: OceanWind::default(), rain: 0.0,
        })
    }

    pub fn apply_weather(&mut self, bytes: &[u8]) -> Option<()> {
        let v: Vec<f32> = std::str::from_utf8(bytes).ok()?.split_whitespace()
            .map(str::parse).collect::<Result<_,_>>().ok()?;
        if v.len() != 12 || v.iter().any(|v| !v.is_finite()) { return None; }
        self.wind = OceanWind { direction: v[1].atan2(v[0]).to_degrees(), speed: v[2], gust: v[3], shift: v[4] }.sanitize();
        self.rain = v[11].clamp(0.0,1.0);
        Some(())
    }

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
        let wind = OceanWind { speed: 0.0, gust: 1.0, shift: 180.0, ..Default::default() };
        assert_eq!(wind.at(123.0), [0.0, 0.0]);
        let ocean = OceanAuthoring { amplitude: f32::NAN, wavelength: 0.0, ..Default::default() }.sanitize();
        assert_eq!(ocean.amplitude, 128.0);
        assert_eq!(ocean.wavelength, 64.0);
    }
    #[test]
    fn ocean_wire_is_strict_and_wind_does_not_change_swell() {
        let wire = b"-100 -200 -50 100 200 0 0 0.25 128 0.65 1400 1 180 1 0 4294967295 8 1 1 0 1 1 2 1";
        let mut ocean = AuthoredOcean::parse(0, wire).unwrap();
        assert_eq!(ocean.waves.seed, u32::MAX);
        let waves = ocean.waves;
        ocean.apply_weather(b"0 1 800 0.4 15 0.5 0.5 1 2 6000 14000 0.7").unwrap();
        assert_eq!(ocean.waves, waves);
        assert_eq!(ocean.wind.direction, 90.0);
        assert!(AuthoredOcean::parse(0, b"1 0 180 0.2 0 0.25 128 0.65 1400 0 180 0 1 0").is_none());
        assert!(ocean.apply_weather(b"1 0 NaN 0 0 0 0 0 0 0 0 0").is_none());
    }
}
