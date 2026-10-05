//! Water.
use crate::app::egui_settings::{
    color_row, ocean_slider, percent, quality_table_row, theme, ui, App, FootprintMode,
    OCEAN_FOAM_COLOR, OCEAN_MAP_SIZE, OCEAN_MESH_QUALITY, OCEAN_NORMAL_STRENGTH, OCEAN_ROUGHNESS,
    OCEAN_SEA_SPRAY, OCEAN_UPDATES, OCEAN_WATER_COLOR, OCEAN_WIND_FOAM,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_surface(&mut self, ui: &mut egui::Ui) {
        const FOOTPRINTS: [(FootprintMode, &str); 3] = [
            (FootprintMode::Off, "Off"),
            (FootprintMode::TwoD, "2D stamp"),
            (FootprintMode::ThreeD, "3D + 2D stamp"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Footprints",
            "Tracks left in snow, sand and mud. The 2D stamp is a decal; the 3D mode \
             also displaces the local player's snow so prints have real depth.",
            theme::Reset::Environment(ui::ENV_ROW_FOOTPRINTS),
            self.video.footprints,
            &FOOTPRINTS,
            2,
        ) {
            self.set_environment_quality_segment(ui::ENV_ROW_FOOTPRINTS, target);
        }
        self.egui_environment_toggle(
            ui,
            "Procedural grass",
            "Scatters GPU-generated grass over surfaces the map marks as \
             ground. Reacts to wind and to players moving through it.",
            ui::ENV_ROW_GRASS,
            self.video.grass,
        );
        if self.video.grass && !self.applied_grass {
            theme::banner(
                ui,
                "Grass needs Apply: this map was prepared without grass resources.",
                theme::WARNING,
            );
        }
    }

    pub(in crate::app::egui_settings) fn egui_water(&mut self, ui: &mut egui::Ui) {
        self.egui_environment_toggle(
            ui,
            "Simulated ocean",
            "Replaces flat water surfaces with real rolling waves that move, \
             catch the light and form foam on their crests. Costs GPU time. \
             (An FFT wave simulation ported from GodotOceanWaves.)",
            ui::ENV_ROW_OCEAN,
            self.video.ocean,
        );
        if self.video.ocean && !self.applied_ocean {
            theme::banner(
                ui,
                "Ocean needs Apply: this renderer started without ocean resources.",
                theme::WARNING,
            );
        }

        if !self.authored_oceans.is_empty() {
            let old = self.authored_ocean_selected;
            let mut selected = old;
            theme::row(
                ui,
                "Authored ocean",
                "Select which server/map-authored ocean volume is shown in the local tuning controls.",
                theme::Reset::None,
                |ui| {
                    egui::ComboBox::from_id_salt("authored_ocean_selector")
                        .selected_text(format!("Ocean {}", self.authored_oceans[selected].index))
                        .show_ui(ui, |ui| {
                            for (index, ocean) in self.authored_oceans.iter().enumerate() {
                                ui.selectable_value(&mut selected, index, format!("Ocean {} · weather {}", ocean.index, ocean.weather));
                            }
                        });
                },
            );
            self.authored_ocean_selected = selected;
            if old != self.authored_ocean_selected {
                self.authored_ocean_preview = false;
                self.publish_authored_oceans();
            }
            if !self.authored_ocean_preview {
                let ocean = &self.authored_oceans[self.authored_ocean_selected];
                self.video.ocean_settings.authored = ocean.waves;
            }

            let mut preview = self.authored_ocean_preview;
            let mut preview_changed = false;
            theme::row(
                ui,
                "Local preview override",
                "Temporarily edit map-authored swell locally without changing the server values.",
                theme::Reset::None,
                |ui| {
                    if let Some(value) = theme::switch(ui, preview) {
                        preview = value;
                        preview_changed = true;
                    }
                },
            );
            if preview_changed {
                self.authored_ocean_preview = preview;
                self.publish_authored_oceans();
            }
            theme::row(
                ui,
                "Server ocean values",
                "Discard the local preview and restore the selected server/map-authored values.",
                theme::Reset::None,
                |ui| {
                    if theme::ghost_button(ui, "RESTORE SERVER VALUES").clicked() {
                        self.authored_ocean_preview = false;
                        self.publish_authored_oceans();
                        let ocean = &self.authored_oceans[self.authored_ocean_selected];
                        self.video.ocean_settings.authored = ocean.waves;
                    }
                },
            );
        }
        let mut settings = self.video.ocean_settings;
        let mut changed = false;

        theme::section(
            ui,
            "SIMULATION",
            "Map-facing swell controls. Distances and speeds use JKA map units (40 units = 1 metre).",
        );
        ui.add_enabled_ui(
            self.authored_oceans.is_empty() || self.authored_ocean_preview,
            |ui| {
                let a = &mut settings.authored;
                ocean_slider(
                    ui,
                    "Swell amplitude",
                    "Height scale of the authored long swell, in map units.",
                    theme::Reset::None,
                    &mut a.amplitude,
                    0.0..=5000.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Swell wavelength",
                    "Distance between authored swell crests, in map units.",
                    theme::Reset::None,
                    &mut a.wavelength,
                    64.0..=32768.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Swell travel direction",
                    "Heading of the authored swell in degrees.",
                    theme::Reset::None,
                    &mut a.direction,
                    -180.0..=180.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Primary wave speed",
                    "Travel speed of the authored swell in map units per second.",
                    theme::Reset::None,
                    &mut a.speed,
                    -4096.0..=4096.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Base choppiness",
                    "Horizontal steepness of the authored swell.",
                    theme::Reset::None,
                    &mut a.steepness,
                    0.0..=1.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Cross-sea / FFT blend",
                    "How strongly the first two FFT cascades roughen and cross the authored swell.",
                    theme::Reset::None,
                    &mut a.slosh,
                    0.0..=1.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Wind chop",
                    "Scales the weather-driven fine chop cascade.",
                    theme::Reset::None,
                    &mut a.wind_chop,
                    0.0..=4.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Foam amount",
                    "Global multiplier for crest foam generation.",
                    theme::Reset::None,
                    &mut a.foam,
                    0.0..=4.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Foam lifetime",
                    "How long accumulated crest foam persists before decaying.",
                    theme::Reset::None,
                    &mut a.foam_lifetime,
                    0.1..=30.0,
                    &mut changed,
                );
                ocean_slider(
                    ui,
                    "Spray amount",
                    "Global multiplier for crest-triggered sea spray particles.",
                    theme::Reset::None,
                    &mut a.spray,
                    0.0..=4.0,
                    &mut changed,
                );
                theme::row(
                    ui,
                    "Wave seed",
                    "Random seed used to generate the repeatable FFT spectrum phases.",
                    theme::Reset::None,
                    |ui| {
                        changed |= ui
                            .add(egui::DragValue::new(&mut a.seed).speed(1.0))
                            .changed();
                    },
                );
            },
        );

        theme::row(
            ui,
            "Mapper export",
            "Copy the current values as map entity keys.",
            theme::Reset::None,
            |ui| {
                if theme::ghost_button(ui, "COPY OCEAN KEYS").clicked() {
                    let a = settings.authored;
                    ui.ctx().copy_text(format!("\"amplitude\" \"{}\"\n\"wavelength\" \"{}\"\n\"speed\" \"{}\"\n\"waveAngle\" \"{}\"\n\"steepness\" \"{}\"\n\"slosh\" \"{}\"\n\"waveSeed\" \"{}\"\n\"waveModel\" \"1\"\n\"windChop\" \"{}\"\n\"foamAmount\" \"{}\"\n\"foamLifetime\" \"{}\"\n\"sprayAmount\" \"{}\"\n",a.amplitude,a.wavelength,a.speed,a.direction,a.steepness,a.slosh,a.seed,a.wind_chop,a.foam,a.foam_lifetime,a.spray));
                }
            },
        );
        const MAP_SIZES: [(u32, &str); 4] =
            [(128, "128"), (256, "256"), (512, "512"), (1024, "1024")];
        if let Some(target) = quality_table_row(
            ui,
            "FFT resolution",
            "Resolution of each cascade's displacement/normal/foam simulation \
             textures and FFT input. This is NOT the water mesh grid; Mesh \
             quality below controls geometry. Higher resolves finer ripples; \
             memory cost grows with N^2 and FFT work roughly with N^2 log N.",
            theme::Reset::Ocean(OCEAN_MAP_SIZE),
            settings.map_size,
            &MAP_SIZES,
            1,
        ) {
            settings.map_size = MAP_SIZES[target].0;
            changed = true;
        }
        const MESHES: [(u8, &str); 2] = [(0, "Low"), (1, "High")];
        if let Some(target) = quality_table_row(
            ui,
            "Mesh quality",
            "Tessellation density of the water surface the displacement is \
             applied to.",
            theme::Reset::Ocean(OCEAN_MESH_QUALITY),
            settings.mesh_quality,
            &MESHES,
            0,
        ) {
            settings.mesh_quality = MESHES[target].0;
            changed = true;
        }
        theme::row(
            ui,
            "Simulation updates",
            "How often the wave spectrum is stepped, independent of frame rate. \
             Lower rates save GPU time and make the motion choppier.",
            theme::Reset::Ocean(OCEAN_UPDATES),
            |ui| {
                let mut value = settings.updates_per_second;
                let readout = format!("{value:.0} Hz");
                if theme::slider(ui, &mut value, 0.0..=60.0, &readout) {
                    settings.updates_per_second = value;
                    changed = true;
                }
            },
        );

        theme::row(
            ui,
            "Sea spray",
            "GodotOceanWaves-style crest spray. Samples the same FFT foam/normal field and emits only from breaking whitecaps.",
            theme::Reset::Ocean(OCEAN_SEA_SPRAY),
            |ui| {
                if let Some(value) = theme::switch(ui, settings.sea_spray) {
                    settings.sea_spray = value;
                    changed = true;
                }
            },
        );

        theme::row(
            ui,
            "Wind foam streaks",
            "Severe-wind surface foam transport. Reuses real FFT whitecaps, advects them with weather wind, and concentrates them into long wind-aligned foam streaks for gale conditions.",
            theme::Reset::Ocean(OCEAN_WIND_FOAM),
            |ui| {
                if let Some(value) = theme::switch(ui, settings.wind_foam_streaks) {
                    settings.wind_foam_streaks = value;
                    changed = true;
                }
            },
        );

        theme::section(ui, "SURFACE", "");
        theme::row(
            ui,
            "Roughness",
            "Microfacet roughness of the water. Low is glassy and mirror-like, \
             high is a dull matte sheet.",
            theme::Reset::Ocean(OCEAN_ROUGHNESS),
            |ui| {
                let mut value = settings.roughness;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    settings.roughness = value;
                    changed = true;
                }
            },
        );
        theme::row(
            ui,
            "Normal strength",
            "How strongly the simulated ripple normals perturb the lighting.",
            theme::Reset::Ocean(OCEAN_NORMAL_STRENGTH),
            |ui| {
                let mut value = settings.normal_strength;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    settings.normal_strength = value;
                    changed = true;
                }
            },
        );
        color_row(
            ui,
            "Water color",
            "Colour of the water body itself, before reflection and foam.",
            theme::Reset::Ocean(OCEAN_WATER_COLOR),
            &mut settings.water_color,
            &mut changed,
        );
        color_row(
            ui,
            "Foam color",
            "Colour of whitecaps and the foam trail behind breaking waves.",
            theme::Reset::Ocean(OCEAN_FOAM_COLOR),
            &mut settings.foam_color,
            &mut changed,
        );

        theme::section(ui, "WATER OPTICS", "");
        color_row(
            ui,
            "Fog color",
            "Scattered underwater light; converted to linear color.",
            theme::Reset::Ocean(9),
            &mut settings.optics.fog_color,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Fog distance",
            "Base absorption distance in game units.",
            theme::Reset::Ocean(10),
            &mut settings.optics.fog_distance,
            1.0..=4000.0,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Transparency",
            "Multiplies the absorption distance above and below water.",
            theme::Reset::Ocean(11),
            &mut settings.optics.transparency,
            0.1..=16.0,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Depth darkening",
            "Sunlight penetration multiplier; larger values stay bright deeper.",
            theme::Reset::Ocean(12),
            &mut settings.optics.depth_darkening,
            0.01..=8.0,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Refraction",
            "Wave distortion of objects viewed through water. Zero disables distortion.",
            theme::Reset::Ocean(13),
            &mut settings.optics.refraction,
            0.0..=0.15,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Caustics",
            "Experimental wave-curvature sunlight focusing. Not shadow-aware yet; zero disables.",
            theme::Reset::Ocean(14),
            &mut settings.optics.caustics,
            0.0..=4.0,
            &mut changed,
        );
        ocean_slider(ui, "Underwater cull", "Absorption-distance multiple for fully submerged distant world geometry. Zero disables.", theme::Reset::Ocean(15), &mut settings.optics.underwater_cull, 0.0..=8.0, &mut changed);

        theme::section(
            ui,
            "ADVANCED SPECTRUM",
            "Three layered FFT spectra reduce tiling. These controls use the native GodotOceanWaves units: metres, m/s and km.",
        );
        for cascade_index in 0..crate::ocean::OCEAN_CASCADES {
            let (role, detail) = match cascade_index {
                0 => ("BROAD SWELL", "Largest repeating wave field; carries most large-scale displacement and foam."),
                1 => ("MID-SCALE CROSS SEA", "Secondary wave field layered over the swell to break up repetition and add crossing chop."),
                _ => ("FINE WIND CHOP", "Small-scale weather-driven detail; mainly normals/foam with only a small displacement contribution."),
            };
            let title = format!("CASCADE {}  ·  {role}", cascade_index + 1);
            egui::CollapsingHeader::new(theme::plain(&title, 13.0, theme::TEXT))
                .id_salt(("ocean_cascade", cascade_index))
                .default_open(false)
                .show(ui, |ui| {
                    theme::banner(ui, detail, theme::TEXT_DIM);
                    let cascade = &mut settings.cascades[cascade_index];
                    ocean_slider(ui, "Tile length X", "Repeat size of this FFT cascade along X, in metres. Larger values carry broader waves and repeat less often.", theme::Reset::OceanCascade(cascade_index as u8, 0), &mut cascade.tile_length[0], 1.0..=2000.0, &mut changed);
                    ocean_slider(ui, "Tile length Y", "Repeat size of this FFT cascade along Y, in metres. Keeping X/Y different can make repetition less obvious.", theme::Reset::OceanCascade(cascade_index as u8, 1), &mut cascade.tile_length[1], 1.0..=2000.0, &mut changed);
                    ocean_slider(ui, "Displacement scale", "Multiplier for this cascade's geometric displacement. Reduce it on higher-frequency cascades to avoid over-busy silhouettes.", theme::Reset::OceanCascade(cascade_index as u8, 2), &mut cascade.displacement_scale, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Normal scale", "Multiplier for this cascade's shading normals. It can add small-scale sparkle/chop without adding the same amount of geometry displacement.", theme::Reset::OceanCascade(cascade_index as u8, 3), &mut cascade.normal_scale, 0.0..=2.0, &mut changed);
                    if cascade_index < 2 {
                        ocean_slider(ui, "Spectrum wind speed", "Reference wind speed for this TMA/JONSWAP spectrum, in m/s. It changes spectrum energy and peak frequency; it is separate from live weather wind.", theme::Reset::OceanCascade(cascade_index as u8, 4), &mut cascade.wind_speed, 0.0001..=60.0, &mut changed);
                        ocean_slider(ui, "Spectrum angle offset", "Directional offset from the authored swell heading, in degrees. This lets the cascades cross rather than stack in exactly one direction.", theme::Reset::OceanCascade(cascade_index as u8, 5), &mut cascade.wind_direction, -360.0..=360.0, &mut changed);
                    } else {
                        theme::banner(
                            ui,
                            "This cascade follows the weather wind speed and direction.",
                            theme::TEXT_FAINT,
                        );
                    }
                    ocean_slider(ui, "Fetch length", "Distance from shoreline / wind fetch, in kilometres. Longer fetch shifts the TMA/JONSWAP sea state toward more developed waves.", theme::Reset::OceanCascade(cascade_index as u8, 6), &mut cascade.fetch_length, 0.1..=1000.0, &mut changed);
                    ocean_slider(ui, "Directional concentration", "GodotOceanWaves calls this swell. Higher values elongate/concentrate wave energy around the preferred heading; it is not your authored swell height.", theme::Reset::OceanCascade(cascade_index as u8, 7), &mut cascade.swell, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Spread", "Mix between strongly directional and flatter/isotropic wave energy. Higher values allow more energy away from the preferred heading.", theme::Reset::OceanCascade(cascade_index as u8, 8), &mut cascade.spread, 0.0..=1.0, &mut changed);
                    ocean_slider(ui, "Detail", "Small-wave suppression control. Lower values attenuate high-frequency waves; 1 keeps the full high-frequency tail.", theme::Reset::OceanCascade(cascade_index as u8, 9), &mut cascade.detail, 0.0..=1.0, &mut changed);
                    ocean_slider(ui, "Whitecap threshold", "Controls how steep/compressed a crest must be before foam accumulates. Higher values make whitecaps trigger more readily in this implementation.", theme::Reset::OceanCascade(cascade_index as u8, 10), &mut cascade.whitecap, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Foam amount", "Per-cascade foam growth multiplier. Lifetime/decay is controlled by the main Foam lifetime setting above.", theme::Reset::OceanCascade(cascade_index as u8, 11), &mut cascade.foam_amount, 0.0..=10.0, &mut changed);
                    if cascade.displacement_scale <= 0.001 {
                        theme::banner(
                            ui,
                            "Displacement is 0: this cascade can still affect normals/foam, but not the mesh silhouette.",
                            theme::TEXT_FAINT,
                        );
                    }
                    if cascade.foam_amount <= 0.001 {
                        theme::banner(
                            ui,
                            "Foam amount is 0: Whitecap changes in this cascade will not visibly add foam until Foam amount is raised.",
                            theme::TEXT_FAINT,
                        );
                    }
                });
        }

        ui.add_space(10.0);
        if theme::ghost_button(ui, "RESET OCEAN DEFAULTS").clicked() {
            settings = crate::ocean::OceanSettings::default();
            changed = true;
        }

        if changed {
            self.video.ocean_settings = settings;
            self.commit_ocean_settings();
        }
    }
}
