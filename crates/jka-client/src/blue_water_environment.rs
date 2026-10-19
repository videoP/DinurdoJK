//! Native atmosphere settings shared by menus, config and the render thread.
use blue_water_sky::{CloudGenus, HighCloudGenus, HighClouds, SkyQuality, SkyScene};
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub quality: u8,
    pub genus: u8,
    pub altitude: f32,
    pub thickness: f32,
    pub horizon: f32,
    pub ambient: f32,
    pub base_shadow: f32,
    pub high_genus: u8,
    pub high_amount: f32,
    pub high_altitude: f32,
    pub moon_phase: f32,
    pub moon_custom: bool,
    pub moon_elevation: f32,
    pub moon_azimuth: f32,
    pub rayleigh: f32,
    pub turbidity: f32,
    pub mie: f32,
    pub mie_g: f32,
    pub multiple: f32,
    /// 0 host weather, 1 original Blue Water rain/lightning, 2 no weather.
    pub weather_backend: u8,
    pub rain: f32,
    pub rain_speed: f32,
    pub lightning: f32,
    /// Temporary diagnostic, deliberately not archived.
    pub strike_preview: bool,
    pub fog: f32,
    pub visibility: f32,
    pub world_lighting: bool,
    pub filmic: bool,
    /// The dome's night glow and floor as a share of the authored level (1 upstream, 0 black). Dome only: the
    /// environment bake and the world's night light keep the authored glow.
    pub night_sky: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            quality: 2,
            genus: 0,
            altitude: 1700.0,
            thickness: 2400.0,
            horizon: 0.06,
            ambient: 1.1,
            base_shadow: 0.2,
            high_genus: 0,
            high_amount: 0.2,
            high_altitude: 8000.0,
            moon_phase: 0.5,
            moon_custom: false,
            moon_elevation: 35.0,
            moon_azimuth: 180.0,
            rayleigh: 0.38,
            turbidity: 2.2,
            mie: 0.225,
            mie_g: 0.72,
            multiple: 1.0,
            weather_backend: 0,
            rain: 0.3,
            rain_speed: 1.0,
            lightning: 2.0,
            strike_preview: false,
            fog: 0.0,
            visibility: 17000.0,
            world_lighting: false,
            filmic: true,
            night_sky: 1.0,
        }
    }
}
impl Settings {
    pub fn sanitize(mut self) -> Self {
        let d = Self::default();
        macro_rules! limit {
            ($f:ident,$a:expr,$b:expr) => {
                self.$f = if self.$f.is_finite() {
                    self.$f.clamp($a, $b)
                } else {
                    d.$f
                };
            };
        }
        self.quality = self.quality.min(3);
        self.genus = self.genus.min(7);
        self.high_genus = self.high_genus.min(3);
        self.weather_backend = self.weather_backend.min(2);
        limit!(altitude, -1000.0, 15000.0);
        limit!(thickness, 50.0, 12000.0);
        limit!(horizon, 0.0, 1.0);
        limit!(ambient, 0.0, 3.0);
        limit!(base_shadow, 0.0, 1.0);
        limit!(high_amount, 0.0, 1.0);
        limit!(high_altitude, 2000.0, 20000.0);
        limit!(moon_phase, 0.0, 1.0);
        limit!(moon_elevation, -90.0, 90.0);
        limit!(moon_azimuth, 0.0, 360.0);
        limit!(rayleigh, 0.0, 1.5);
        limit!(turbidity, 1.0, 10.0);
        limit!(mie, 0.0, 2.0);
        limit!(mie_g, 0.0, 0.95);
        limit!(multiple, 0.0, 3.0);
        limit!(rain, 0.0, 1.0);
        limit!(rain_speed, 0.25, 4.0);
        limit!(lightning, 0.0, 30.0);
        limit!(fog, 0.0, 1.0);
        limit!(visibility, 200.0, 100000.0);
        limit!(night_sky, 0.0, 1.0);
        self
    }
    pub fn quality(self) -> SkyQuality {
        match self.quality {
            0 => SkyQuality::Low,
            1 => SkyQuality::Medium,
            3 => SkyQuality::Ultra,
            _ => SkyQuality::High,
        }
    }
    pub fn scene(
        self,
        elevation: f64,
        azimuth: f64,
        intensity: f32,
        coverage: f32,
        wind: [f32; 2],
        clouds: bool,
    ) -> SkyScene {
        let s = self.sanitize();
        let mut scene = SkyScene::north_atlantic(elevation, azimuth);
        // The existing sun editor stores q3 light units; 255 is one daylight scale.
        scene.sun_intensity = f64::from(intensity.max(0.0) / 255.0) * 6.6;
        scene.moon_phase = f64::from(s.moon_phase);
        if s.moon_custom {
            scene.moon = Some(blue_water_sky::SkyAngles {
                elevation: f64::from(s.moon_elevation),
                azimuth: f64::from(s.moon_azimuth),
            });
        }
        scene.atmosphere = blue_water_sky::model::AuthoredAtmosphere {
            rayleigh: s.rayleigh as f64,
            turbidity: s.turbidity as f64,
            mie: s.mie as f64,
            mie_g: s.mie_g as f64,
            multiple: s.multiple as f64,
        };
        scene.clouds.coverage = coverage as f64;
        scene.clouds.altitude = s.altitude as f64;
        scene.clouds.thickness = s.thickness as f64;
        scene.clouds.base_chosen = true;
        scene.clouds.depth_chosen = true;
        scene.clouds.horizon_coverage = s.horizon as f64;
        scene.clouds.ambient = s.ambient as f64;
        scene.clouds.base_shadow = s.base_shadow as f64;
        scene.clouds.genus = match s.genus {
            1 => Some(CloudGenus::Cumulus),
            2 => Some(CloudGenus::Stratus),
            3 => Some(CloudGenus::Cumulonimbus),
            4 => Some(CloudGenus::Stratocumulus),
            5 => Some(CloudGenus::Nimbostratus),
            6 => Some(CloudGenus::Altocumulus),
            7 => Some(CloudGenus::Altostratus),
            _ => None,
        };
        scene.clouds.high = match s.high_genus {
            0 => None,
            v => Some(HighClouds {
                genus: match v {
                    2 => HighCloudGenus::Cirrostratus,
                    3 => HighCloudGenus::Cirrocumulus,
                    _ => HighCloudGenus::Cirrus,
                },
                amount: s.high_amount as f64,
                altitude: s.high_altitude as f64,
            }),
        };
        scene.clouds.wind_speed = f64::from(wind[0] / crate::ocean::OCEAN_UNITS_PER_METER);
        scene.clouds.wind_heading = wind[1] as f64;
        if s.weather_backend == 1 {
            scene.weather.precipitation = s.rain as f64;
            scene.weather.lightning = s.lightning as f64;
            if s.strike_preview { scene.weather.lightning = 0.0; }
            scene.clouds.fog = s.fog as f64;
        }
        if !clouds {
            scene.clouds.coverage = 0.0;
            scene.clouds.high = None;
            scene.clouds.fog = 0.0;
        }
        scene
    }
}
