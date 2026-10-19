//! Blue Water backend selection and the controls currently consumed by the port.
use crate::app::egui_settings::{theme, App};

impl App {
    pub(in crate::app::egui_settings) fn egui_lakes(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "BLUE WATER LAKES & PONDS", "Flat water with wind ripples, sky reflections, sun glitter, shallow transmission and soft shore edges. Each pool keeps its map height and outline.");
        let mut settings = self.video.ocean_settings;
        let d = crate::ocean::LakeSettings::default();
        let mut changed = false;
        super::ocean_slider_d(ui, "Lake wind speed (m/s)", "Controls analytic ripples and reflection roughness; no ocean FFT is simulated.", &mut settings.lake.wind_speed, d.wind_speed, 0.0..=40.0, &mut changed);
        super::ocean_slider_d(ui, "Lake wind direction", "Direction ripples travel, in degrees from +X toward world +Z.", &mut settings.lake.direction, d.direction, -180.0..=180.0, &mut changed);
        super::ocean_slider_d(ui, "Lake transparency", "Optical clarity multiplier. 1 preserves the original lake coefficients.", &mut settings.lake.transparency, d.transparency, 0.1..=10.0, &mut changed);
        theme::row(ui, "Lake water", "Freshwater is brown-green; lagoons and salt ponds use the original turquoise optics. This applies to all promoted pools for the A/B.", theme::Reset::None, |ui| {
            for (salt,label) in [(false,"Freshwater"),(true,"Lagoon / salt pond")] {
                if theme::chip(ui,label,settings.lake.salt == salt).clicked() { settings.lake.salt=salt;changed=true; }
            }
        });
        if changed {self.video.ocean_settings=settings;self.commit_ocean_settings();}
    }

    pub(in crate::app::egui_settings) fn egui_blue_water(&mut self, ui: &mut egui::Ui) {
        let hybrid = self.video.ocean_settings.backend == 2;
        theme::section(
            ui,
            if hybrid { "HYBRID OCEAN" } else { "BLUE WATER OCEAN" },
            if hybrid { "Native Blue Water surface above; JKA/Godot optics and surface while submerged." }
            else { "Original Blue Water ocean surface, waves and optical material." },
        );
        let mut ocean = self.video.ocean_settings;
        let d = crate::ocean::OceanSettings::default();
        let mut changed = false;
        theme::section(ui, "WAVES", "Native Blue Water parameters, shared across promoted pools. Changes apply live.");
        super::ocean_slider_d(ui, "Wave height", if hybrid {
            "Master wave height: controls Blue Water FFT and proportionally scales the Hybrid Gerstner swell. Zero removes both wave contributions. Gerstner height still sets relative strength."
        } else {
            "Significant wave height in metres (4 times the surface height standard deviation). Zero is flat."
        }, &mut ocean.blue_water_waves.height, d.blue_water_waves.height, 0.0..=60.0, &mut changed);
        super::ocean_slider_d(ui, "Swell direction", "Direction waves travel, in degrees from +X toward world +Z.", &mut ocean.blue_water_waves.direction, d.blue_water_waves.direction, -180.0..=180.0, &mut changed);
        super::ocean_slider_d(ui, "Peak wavelength (m)", "Dominant crest spacing in metres. Controls wave speed through the native dispersion relation.", &mut ocean.blue_water_waves.wavelength, d.blue_water_waves.wavelength, 2.0..=512.0, &mut changed);
        super::ocean_slider_d(ui, "Choppiness", "Horizontal displacement strength. Zero keeps only vertical waves.", &mut ocean.blue_water_waves.choppiness, d.blue_water_waves.choppiness, 0.0..=4.0, &mut changed);
        super::ocean_slider_d(ui, "Spectrum wind speed", "Reference wind speed in metres per second. Changes the native spectral shape and fine surface roughness; independent of weather wind.", &mut ocean.blue_water_waves.wind_speed, d.blue_water_waves.wind_speed, 0.1..=40.0, &mut changed);
        if hybrid {
            theme::section(ui, "GERSTNER / FFT MIX", "Mix Blue Water's spectral waves with the authored directional swell. The FFT simulation stays resident and unchanged.");
            super::ocean_slider_d(ui, "Cross-sea / FFT blend",
                "1 = pure Blue Water FFT (original). 0 = authored Gerstner swell. Between them, both displacements contribute. Hybrid only; changing this does not rebuild the FFT.",
                &mut ocean.hybrid_fft_blend, d.hybrid_fft_blend, 0.0..=1.0, &mut changed);
            if ocean.hybrid_fft_blend < 1.0 {
                // Hybrid has independent authored wave controls; no Godot map
                // wave parameters or linked controls are modified.
                super::ocean_slider_d(ui, "Gerstner height (units)",
                    "Base swell amplitude at the default Wave height (40 JKA units = 1 metre). Actual Gerstner displacement also follows Wave height and the FFT blend.",
                    &mut ocean.hybrid_swell.amplitude, d.hybrid_swell.amplitude, 0.0..=1024.0, &mut changed);
                super::ocean_slider_metres_d(ui, "Gerstner wavelength (m)", "Distance between crests, in metres (40 JKA units = 1 metre).", &mut ocean.hybrid_swell.wavelength, d.hybrid_swell.wavelength, 256.0, 1.6, &mut changed);
                super::ocean_slider_d(ui, "Gerstner speed (units/s)", "Travel speed in JKA units per second; negative reverses travel.", &mut ocean.hybrid_swell.speed, d.hybrid_swell.speed, -4096.0..=4096.0, &mut changed);
                super::ocean_slider_d(ui, "Gerstner steepness", "Controls the authored swell's horizontal choppiness.", &mut ocean.hybrid_swell.steepness, d.hybrid_swell.steepness, 0.0..=1.0, &mut changed);
            }
            if theme::ghost_button(ui, "RESET GERSTNER & BLEND").clicked() {
                ocean.hybrid_fft_blend = 1.0;
                ocean.hybrid_swell = crate::ocean::OceanAuthoring::default();
                changed = true;
            }
        }
        if theme::ghost_button(ui, "RESET WAVES").clicked() {
            ocean.blue_water_waves = crate::ocean::BlueWaterWaves::default();
            changed = true;
        }
        theme::row(
            ui,
            "FFT update rate",
            "How often the Blue Water wave field advances. Zero removes the rate cap.",
            theme::Reset::None,
            |ui| {
                let mut value = ocean.updates_per_second;
                let readout = if value == 0.0 {
                    "Uncapped".to_owned()
                } else {
                    format!("{value:.0} Hz")
                };
                if theme::slider_default(ui, &mut value, 0.0..=60.0, &readout, d.updates_per_second) {
                    ocean.updates_per_second = value;
                    changed = true;
                }
            },
        );
        theme::section(ui,"COASTAL WAVES & SURF (A/B)","Blue Water's original coast model. Your separate JKA shallow-bank solver remains available through r_coastal.");
        theme::row(ui,"Upstream coastal waves","Builds per-pool shore depth and wind-fetch maps in the background. Waves shrink in shallow water and behind land; moving surf bands follow the bank. Off restores open-ocean waves. Native / Hybrid only.",theme::Reset::None,|ui| {
            if ui.checkbox(&mut ocean.blue_water_coast,"Blue Water coast & surf").changed() {changed=true;}
        });
        if ocean.blue_water_coast {
            theme::row(ui,"Physical land shelter (advanced)","Optional upstream wind-fetch and swell shelter. Low wind with long authored waves can become almost flat behind land. Leave off for authored incoming ocean waves with depth breaking and surf at real banks.",theme::Reset::None,|ui| {
                if ui.checkbox(&mut ocean.blue_water_coast_shelter,"Wind / swell shelter").changed() {changed=true;}
            });
            let status=self.prepared_map_cache.as_ref().and_then(|c|c.map.coastal_bathymetry.as_ref())
                .map_or("Load a BSP map with water promotion enabled to prepare its shores.",|j|j.upstream_shore.status());
            theme::banner(ui,status,theme::TEXT_DIM);
        }
        theme::section(ui, "REFLECTIONS & OPTICS", "Screen-space reflections, caustics, refraction and underwater effects. Each change applies live.");
        theme::row(ui, "Screen-space reflections", "Trace nearby scene reflections. Environment and sky remain when disabled.", theme::Reset::None, |ui| {
            let value = if hybrid { &mut ocean.hybrid_ssr } else { &mut ocean.blue_water_ssr };
            if ui.checkbox(value, "Trace scene reflections").changed() { changed = true; }
        });
        theme::row(ui, "Caustics", "Godot carries the inexpensive optics caustics into BlueWater transmission; native modes use its separate receiver pass.", theme::Reset::None, |ui| {
            for &(mode, name) in if hybrid { &[(3, "Godot"), (4, "Animated"), (0, "Native"), (1, "Native FFT only"), (2, "Off")][..] } else { &[(0, "Native"), (1, "Native FFT only"), (2, "Off")][..] } {
                if theme::chip(ui, name, (if hybrid { ocean.hybrid_caustics_mode } else { ocean.blue_water_caustics_mode }) == mode).clicked() {
                    if hybrid { ocean.hybrid_caustics_mode = mode; } else { ocean.blue_water_caustics_mode = mode; } changed = true;
                }
            }
        });
        theme::row(ui, "Refraction method", "Screen offset restores seabed bending with the inexpensive Godot approach. Ray trace is the more expensive physical scene trace.", theme::Reset::None, |ui| {
            for &(mode, name) in if hybrid { &[(1, "Screen offset"), (2, "Ray trace"), (0, "Off")][..] } else { &[(2, "Ray trace"), (1, "Screen offset"), (0, "Off")][..] } {
                if theme::chip(ui, name, (if hybrid { ocean.hybrid_refraction_mode } else { ocean.blue_water_refraction_mode }) == mode).clicked() {
                    if hybrid { ocean.hybrid_refraction_mode = mode; } else { ocean.blue_water_refraction_mode = mode; } changed = true;
                }
            }
        });
        theme::row(ui, "Underwater effects", "Hybrid uses the Godot surface and underwater optics while submerged; native modes keep Blue Water scattering and waterline. Above water, Hybrid keeps the Blue Water surface.", theme::Reset::None, |ui| {
            for &(mode, name) in if hybrid { &[(4, "Godot submerged"), (0, "Native"), (3, "Native submerged"), (1, "No motes"), (2, "Off/test")][..] } else { &[(0, "Native"), (3, "Native submerged"), (1, "No motes"), (2, "Off/test")][..] } {
                if theme::chip(ui, name, (if hybrid { ocean.hybrid_underwater_mode } else { ocean.blue_water_underwater_mode }) == mode).clicked() {
                    if hybrid { ocean.hybrid_underwater_mode = mode; } else { ocean.blue_water_underwater_mode = mode; } changed = true;
                }
            }
        });
        theme::section(ui, "SHIP WAKE & SPRAY (A/B)", "Blue Water's own wake, impact rings and spindrift, ported beside JKA's water interaction for comparison. Console: oceanwake ship | strike | clear | reset.");
        theme::row(ui, "Ship wake", "Off keeps JKA's own water interaction only. On binds Blue Water's dispersive wake field, trail foam and slick, bow waves, hull contact foam and impact rings to this surface. It needs hulls: sail a demo with 'oceanwake ship destroyer', or drop an impact with 'oceanwake strike'.", theme::Reset::None, |ui| {
            let mut on = ocean.blue_wake != 0;
            if ui.checkbox(&mut on, "Blue Water wake").changed() { ocean.blue_wake = u8::from(on); changed = true; }
        });
        theme::row(ui, "Spindrift", "Wisps of spray torn off breaking crests, from a gale's wind (13 m/s; raise Spectrum wind speed above). The surface's spray haze needs 17 m/s.", theme::Reset::None, |ui| {
            let mut on = ocean.spindrift != 0;
            if ui.checkbox(&mut on, "Blue Water spindrift").changed() { ocean.spindrift = u8::from(on); changed = true; }
        });
        theme::section(ui, "WATER APPEARANCE", "Changes apply live above and below the surface.");
        theme::row(ui, "Transparency", if hybrid {
            "Hybrid clarity multiplier: controls both native surface transmission and Godot underwater fog distance. 1x uses stock Godot fog distance, independent of the Godot preset."
        } else {
            "Higher values let light travel farther through water. 1x preserves native calibration."
        }, theme::Reset::None, |ui| {
            let mut value = if hybrid { ocean.hybrid_transparency } else { ocean.blue_water_optics.transparency };
            let readout = format!("{value:.2}x");
            if theme::slider_default(ui, &mut value, 0.1..=10.0, &readout, if hybrid { d.hybrid_transparency } else { d.blue_water_optics.transparency }) {
                if hybrid { ocean.hybrid_transparency = value; } else { ocean.blue_water_optics.transparency = value; }
                changed = true;
            }
        });
        if hybrid {
            super::color_row(ui, "Water color", "Hybrid surface body tint; does not modify native Blue Water's saved color.", theme::Reset::None,
                &mut ocean.hybrid_water_color, &mut changed);
            super::color_row(ui, "Underwater fog color", "Hybrid color for both Godot submerged optics and Blue Water scattering; independent of the Godot and native presets.", theme::Reset::None,
                &mut ocean.hybrid_fog_color, &mut changed);
        } else {
            super::color_row(ui, "Water color", "Tint of the native water body and crest transmission.", theme::Reset::None,
                &mut ocean.blue_water_optics.water_color, &mut changed);
            super::color_row(ui, "Underwater fog color", "Color of native Blue Water scattered light below the surface.", theme::Reset::None,
                &mut ocean.blue_water_optics.fog_color, &mut changed);
        }
        if theme::ghost_button(ui, "RESET WATER APPEARANCE").clicked() {
            if hybrid {
                ocean.hybrid_transparency = 1.0;
                ocean.hybrid_fog_color = crate::ocean::optics::Settings::default().fog_color;
                ocean.hybrid_water_color = crate::ocean::BlueWaterOptics::default().water_color;
            } else {
                ocean.blue_water_optics = crate::ocean::BlueWaterOptics::default();
            }
            changed = true;
        }
        if hybrid && ocean.hybrid_caustics_mode == 4 {
            theme::section(ui, "ANIMATED CAUSTICS", "Independent procedural light networks that deform, split and merge over time. GPU pattern is generated once, not re-simulated per frame. No ocean FFT reads per receiver pixel.");
            super::ocean_slider_d(ui, "Pattern scale", "Larger values make finer light webs, without increasing texture resolution or FFT cost.", &mut ocean.hybrid_procedural_scale, d.hybrid_procedural_scale, 0.25..=4.0, &mut changed);
            super::ocean_slider_d(ui, "Animation speed", "Pattern evolution rate; zero freezes the field for performance/visual A/B. Wave roughness also influences the animation.", &mut ocean.hybrid_procedural_speed, d.hybrid_procedural_speed, 0.0..=3.0, &mut changed);
        }
        if hybrid && ocean.hybrid_caustics_mode == 3 {
            theme::section(ui, "HYBRID CAUSTIC SHAPE", "The 181m Blue Water mid-cascade makes larger patterns than Godot's 57m cascade. These controls only affect Hybrid's Godot receiver caustics (mode: Godot). No extra passes or textures.");
            super::ocean_slider_d(ui, "Pattern density", "Higher values create smaller caustic markings by tiling the existing Blue Water slope texture. 3x roughly compensates for the 181m vs 57m wave tile lengths.", &mut ocean.hybrid_caustics_scale, d.hybrid_caustics_scale, 1.0..=6.0, &mut changed);
            super::ocean_slider_d(ui, "Focus gain", "Additional Hybrid-only brightness/contrast after wave curvature calculation. Godot caustic strength is still the master control.", &mut ocean.hybrid_caustics_gain, d.hybrid_caustics_gain, 0.0..=4.0, &mut changed);
        }
        super::color_row(
            ui,
            "Foam color",
            "Tint for the original Blue Water crest and surface foam.",
            theme::Reset::None,
            &mut ocean.foam_color,
            &mut changed,
        );
        super::ocean_slider_d(ui, "Surface refraction", "Wave distortion used by the original surface's scene transmission.", &mut ocean.blue_water_refraction, d.blue_water_refraction, 0.0..=0.15, &mut changed);
        super::ocean_slider_d(ui, "Caustic strength", "Strength of the selected Godot or native Blue Water caustics.", &mut ocean.optics.caustics, d.optics.caustics, 0.0..=4.0, &mut changed);
        if hybrid {
            super::ocean_slider_d(ui, "Caustic view distance", "Godot-style Hybrid caustics: skip distant terrain receivers and fade the last 25% of range, in metres. 0 disables culling. Native caustics use their own height gate.", &mut ocean.optics.caustic_max_distance_m, d.optics.caustic_max_distance_m, 0.0..=500.0, &mut changed);
        }
        if changed {
            self.video.ocean_settings = ocean;
            self.commit_ocean_settings();
        }

        theme::banner(ui, "Use render_perf_capture for ocean GPU pass timings. r_showtris 32 draws this backend's actual mesh. Sky/cloud backends live in Clouds.", theme::TEXT_DIM);
    }
}
