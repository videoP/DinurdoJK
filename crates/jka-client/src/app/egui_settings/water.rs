//! Water.
use crate::app::egui_settings::{
    color_row, linked_ocean_slider, ocean_slider, ocean_slider_dd, percent, quality_table_row, theme, ui, App, FootprintMode,
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
        // Off / native JKA surfaceSprites / promoted to Godot procedural grass
        // (r_surfaceSprites + r_grass). Godot replaces the sprite emitters it
        // can promote; anything it cannot keeps its JKA sprites.
        const GRASS: [(u8, &str); 3] = [(0, "Off"), (1, "JKA sprites"), (2, "Godot grass")];
        let grass_mode = if self.video.grass { 2 } else { u8::from(self.video.surface_sprites) };
        if let Some(target) = quality_table_row(
            ui,
            "Grass",
            "Ground vegetation the map marks with surfaceSprites. JKA sprites are the \
             original flat cards; Godot grass promotes them to GPU-generated blades that \
             react to wind and to players moving through them.",
            theme::Reset::Environment(ui::ENV_ROW_GRASS),
            grass_mode,
            &GRASS,
            1,
        ) {
            let sprites = target != 0;
            if sprites != self.video.surface_sprites {
                let _ = self.set_console_cvar("r_surfaceSprites", if sprites { "1" } else { "0" });
            }
            if (target == 2) != self.video.grass {
                self.environment_selected = ui::ENV_ROW_GRASS;
                self.change_environment_setting(1);
            }
        }
        if self.video.grass && !self.applied_grass {
            theme::banner(
                ui,
                "Grass needs Apply: this map was prepared without grass resources.",
                theme::WARNING,
            );
        }
    }

    pub(in crate::app::egui_settings) fn egui_water(&mut self, ui: &mut egui::Ui) {
        let current = self.ocean_type();
        theme::section(ui, "WATER PROMOTION", "Choose how map water is promoted. Off keeps stock JKA water.");
        theme::row(ui, "Water renderer", "Lakes / ponds uses subtle noise relief, wind ripples and editable water colors, without ocean FFT or an infinite horizon. Gerstner is Hybrid with godot-water-shader's waves, colour and persistent foam, lit by Blue Water. Switch live to compare. Sky and clouds are independent.", theme::Reset::None, |ui| {
            for (mode, label) in [(0, "Off / JKA"), (1, "Godot"), (2, "Blue Water"), (3, "Hybrid"), (5, "Gerstner"), (4, "Lakes / ponds")] {
                if theme::chip(ui, label, mode == current).clicked() && mode != current {
                    self.set_ocean_type(mode);
                }
                ui.add_space(3.0);
            }
        });
        if !self.video.ocean {
            theme::banner(ui, "Stock JKA water is active; promoted-ocean simulation and shading are disabled.", theme::TEXT_DIM);
            return;
        }
        if self.video.ocean && !self.applied_ocean {
            theme::banner(ui, "Ocean resources need Apply because this renderer started without them.", theme::WARNING);
        }

        if self.video.ocean_settings.backend == 3 {
            self.egui_lakes(ui);
            return;
        }
        let mut wake_mode = self.video.water_wake;
        ui.horizontal(|ui| {
            ui.label("Water Wake");
            egui::ComboBox::from_id_salt("cg_waterwake_mode")
                .selected_text(match wake_mode {
                    0 => "Off", 1 => "JKA", _ => "Enhanced",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut wake_mode, 0, "Off");
                    ui.selectable_value(&mut wake_mode, 1, "JKA");
                    ui.selectable_value(&mut wake_mode, 2, "Enhanced");
                });
        }).response.on_hover_text("cg_waterwake: original wake shader on vanilla water, or enhanced movement-driven wake plus the existing Godot ocean wake in mode 2.");
        if wake_mode != self.video.water_wake {
            self.video.water_wake = wake_mode;
            if let Some(session) = self.game_session.as_mut() {
                session.water_wake_mode = wake_mode;
            }
            self.commit_ocean_settings();
        }
        let mut interaction = self.video.ocean_settings.interaction;
        let mut strength = self.video.ocean_settings.interaction_strength;
        let mut splash_mode = self.video.ocean_settings.interaction_splash_mode;
        let toggled = ui.checkbox(&mut interaction, "Player water deformation").on_hover_text(
            "Nearby ripples from moving players. Uses the ocean update rate and fades after contact stops.").changed();
        let adjusted = interaction && ui.horizontal(|ui| {
            ui.label("Deformation strength");
            ui.add(egui::DragValue::new(&mut strength).speed(0.5).range(0.0..=f32::MAX)).changed()
        }).inner;
        let mut splash_changed = false;
        ui.horizontal(|ui| {
            ui.label("Interaction FX A/B");
            egui::ComboBox::from_id_salt("ocean_interaction_splash_mode")
                .selected_text(match splash_mode {
                    0 => "Off",
                    1 => "A: Entry droplets",
                    2 => "B: Wake spray",
                    3 => "A + B",
                    4 => "Entry jets",
                    _ => "All FX",
                })
                .show_ui(ui, |ui| {
                    splash_changed |= ui.selectable_value(&mut splash_mode, 0, "Off").changed();
                    splash_changed |= ui.selectable_value(&mut splash_mode, 1, "A: Entry droplets").changed();
                    splash_changed |= ui.selectable_value(&mut splash_mode, 2, "B: Wake spray").changed();
                    splash_changed |= ui.selectable_value(&mut splash_mode, 3, "A + B").changed();
                    splash_changed |= ui.selectable_value(&mut splash_mode, 4, "Entry jets").changed();
                    splash_changed |= ui.selectable_value(&mut splash_mode, 5, "All FX").changed();
                });
        }).response.on_hover_text(
            "Compare entry droplets with spray emitted along the moving wake. Count and size follow speed and deformation strength; Entry jets and All FX add upward jets.");
        if toggled || adjusted || splash_changed {
            self.video.ocean_settings.interaction = interaction;
            self.video.ocean_settings.interaction_strength = strength;
            self.video.ocean_settings.interaction_splash_mode = splash_mode;
            self.commit_ocean_settings();
        }
        if self.video.ocean_settings.backend != 0 {
            self.egui_blue_water(ui);
            return;
        }
        // Godot-only controls begin below. Native Blue Water and Hybrid do not
        // display values that they never consume.
        let mut target = self.ocean_settings_target;
        let target_label = if target >= 0 {
            self.promoted_oceans.iter().find(|p| p.index == target as usize).map(|p| p.label())
                .unwrap_or_else(|| "Shared simulation / optics".into())
        } else if target <= -2 {
            self.authored_oceans.get((-target-2) as usize)
                .map(|o| format!("Authored ocean {}",o.index))
                .unwrap_or_else(|| "Shared simulation / optics".into())
        } else { "Shared simulation / optics".into() };
        theme::row(ui, "Ocean settings target",
            "Choose a pool by texture and JKA coordinates. Pool tuning is local to this map; FFT quality and underwater optics are shared.",
            theme::Reset::None, |ui| {
                egui::ComboBox::from_id_salt("ocean_settings_target").selected_text(target_label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut target, -1, "Shared simulation / optics");
                    for pool in &self.promoted_oceans {
                        ui.selectable_value(&mut target, pool.index as i32, pool.label());
                    }
                    for (i,ocean) in self.authored_oceans.iter().enumerate() {
                        ui.selectable_value(&mut target, -2-i as i32, format!("Authored ocean {}", ocean.index));
                    }
                });
            });
        if target != self.ocean_settings_target {
            self.ocean_settings_target = target;
            self.authored_ocean_preview = false;
            if target <= -2 { self.authored_ocean_selected = (-target-2) as usize; }
            self.publish_authored_oceans();
        }
        let promoted_index = self.promoted_oceans.iter().position(|p| self.ocean_settings_target == p.index as i32);
        let reset = |value| if promoted_index.is_some() { theme::Reset::None } else { value };
        if let Some(index) = promoted_index {
            let pool = &self.promoted_oceans[index];
            theme::banner(ui, &format!("Local pool tuning / image variation {:.0}% / amplitude default {:.1}u. Manual changes override these estimates.",
                pool.heuristic.variation*100.0, pool.heuristic.amplitude), theme::TEXT_DIM);
        } else {
            theme::banner(ui, "FFT spectrum, quality and underwater optics are shared. Select a pool to edit its wave height, chop, foam and surface appearance.", theme::TEXT_DIM);
        }
        if self.ocean_settings_target <= -2 && !self.authored_oceans.is_empty() {
            self.authored_ocean_selected = self.authored_ocean_selected.min(self.authored_oceans.len()-1);
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
        let mut settings = if let Some(index) = promoted_index {
            let pool = &self.promoted_oceans[index];
            let local = self.promoted_ocean_overrides.iter().find(|(slot,_)| *slot == pool.index).map(|(_,s)| *s);
            crate::ocean::appearance::surface_settings(self.video.ocean_settings, pool, local)
        } else {
            let mut shared = self.video.ocean_settings;
            if self.ocean_settings_target > -2 { shared.apply_links(false); }
            shared
        };
        // Locks live on the shared settings; a lock toggle edits this copy and
        // is written back with the rest of the change below.
        let mut links = self.video.ocean_settings.links;
        let da = crate::ocean::OceanSettings::default().authored;
        let dopt = crate::ocean::OceanSettings::default().optics;
        let mut changed = false;

        theme::section(
            ui,
            "SIMULATION",
            "Map-facing swell controls. Distances and speeds use JKA map units (40 units = 1 metre).",
        );
        ui.add_enabled_ui(
            promoted_index.is_some() || (self.ocean_settings_target <= -2 && self.authored_ocean_preview),
            |ui| {
                let a = &mut settings.authored;
                ocean_slider_dd(
                    ui,
                    "Swell amplitude",
                    "Master sea state, in map units. Wavelength, speed, choppiness, wind chop, foam, spray and roughness follow it while their padlocks are closed. Promoted pools retain a small nonzero ripple.",
                    theme::Reset::None,
                    &mut a.amplitude, da.amplitude,
                    if promoted_index.is_some() { crate::ocean::appearance::MIN_PROMOTED_AMPLITUDE..=5000.0 } else { 0.0..=5000.0 },
                    &mut changed,
                );
                linked_ocean_slider(
                    ui,
                    "Swell wavelength",
                    "Distance between authored swell crests, in map units. Follows the swell amplitude while locked; unlock to set it yourself.",
                    &mut a.wavelength, da.wavelength,
                    64.0..=32768.0,
                    crate::ocean::links::WAVELENGTH,
                    &mut links,
                    &mut changed,
                );
                ocean_slider_dd(
                    ui,
                    "Swell travel direction",
                    "Heading of the authored swell in degrees.",
                    theme::Reset::None,
                    &mut a.direction, da.direction,
                    -180.0..=180.0,
                    &mut changed,
                );
                linked_ocean_slider(
                    ui,
                    "Primary wave speed",
                    "Travel speed of the authored swell in map units per second. Follows the swell amplitude while locked; unlock to set it yourself.",
                    &mut a.speed, da.speed,
                    -4096.0..=4096.0,
                    crate::ocean::links::SPEED,
                    &mut links,
                    &mut changed,
                );
                linked_ocean_slider(
                    ui,
                    "Base choppiness",
                    "Horizontal steepness of the authored swell. Follows the swell amplitude while locked; unlock to set it yourself.",
                    &mut a.steepness, da.steepness,
                    0.0..=1.0,
                    crate::ocean::links::STEEPNESS,
                    &mut links,
                    &mut changed,
                );
                ocean_slider_dd(
                    ui,
                    "Cross-sea / FFT blend",
                    "How strongly the first two FFT cascades roughen and cross the authored swell.",
                    theme::Reset::None,
                    &mut a.slosh, da.slosh,
                    0.0..=1.0,
                    &mut changed,
                );
                linked_ocean_slider(
                    ui,
                    "Wind chop",
                    "Scales the weather-driven fine chop cascade. Follows the swell amplitude while locked; unlock to set it yourself.",
                    &mut a.wind_chop, da.wind_chop,
                    0.0..=4.0,
                    crate::ocean::links::WIND_CHOP,
                    &mut links,
                    &mut changed,
                );
                linked_ocean_slider(
                    ui,
                    "Foam amount",
                    "Foam strength for the selected ocean. Follows the swell amplitude while locked; unlock to set it yourself.",
                    &mut a.foam, da.foam,
                    0.0..=4.0,
                    crate::ocean::links::FOAM,
                    &mut links,
                    &mut changed,
                );
                linked_ocean_slider(
                    ui,
                    "Spray amount",
                    "Spray strength for the selected ocean. Follows the swell amplitude while locked; unlock to set it yourself.",
                    &mut a.spray, da.spray,
                    0.0..=4.0,
                    crate::ocean::links::SPRAY,
                    &mut links,
                    &mut changed,
                );

            },
        );

        ui.add_enabled_ui(promoted_index.is_none(), |ui| {
            ocean_slider_dd(
                ui,
                "Foam lifetime",
                "How long accumulated crest foam persists before decaying.",
                theme::Reset::None,
                &mut settings.authored.foam_lifetime, da.foam_lifetime,
                0.1..=30.0,
                &mut changed,
            );
        });

        ui.add_enabled_ui(promoted_index.is_none(), |ui| {
            theme::row(
                ui,
                "Wave seed",
                "Random seed used to generate the repeatable FFT spectrum phases.",
                theme::Reset::None,
                |ui| {
                    changed |= ui
                        .add(egui::DragValue::new(&mut settings.authored.seed).speed(1.0))
                        .changed();
                },
            );
        });


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
        theme::banner(ui, "FFT resolution, mesh quality and update rate apply to all oceans.", theme::TEXT_FAINT);
        ui.add_enabled_ui(promoted_index.is_none(), |ui| {
        const MAP_SIZES: [(u32, &str); 4] =
            [(128, "128"), (256, "256"), (512, "512"), (1024, "1024")];
        if let Some(target) = quality_table_row(
            ui,
            "FFT resolution",
            "Resolution of each cascade's displacement/normal/foam simulation \
             textures and FFT input. This is NOT the water mesh grid; Mesh \
             quality below controls geometry. Higher resolves finer ripples; \
             memory cost grows with N^2 and FFT work roughly with N^2 log N.",
            reset(theme::Reset::Ocean(OCEAN_MAP_SIZE)),
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
            reset(theme::Reset::Ocean(OCEAN_MESH_QUALITY)),
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
            reset(theme::Reset::Ocean(OCEAN_UPDATES)),
            |ui| {
                let mut value = settings.updates_per_second;
                let readout = format!("{value:.0} Hz");
                if theme::slider(ui, &mut value, 0.0..=60.0, &readout) {
                    settings.updates_per_second = value;
                    changed = true;
                }
            },
        );
        });
        theme::row(
            ui,
            "Sea spray",
            "GodotOceanWaves-style crest spray. Samples the same FFT foam/normal field and emits only from breaking whitecaps.",
            reset(theme::Reset::Ocean(OCEAN_SEA_SPRAY)),
            |ui| {
                let preview = theme::preview_toggle_value(ui.ctx(), reset(theme::Reset::Ocean(OCEAN_SEA_SPRAY)), settings.sea_spray);
                if let Some(value) = theme::switch(ui, preview) {
                    settings.sea_spray = value;
                    changed = true;
                }
            },
        );

        theme::row(
            ui,
            "Wind foam streaks",
            "Severe-wind surface foam transport. Reuses real FFT whitecaps, advects them with weather wind, and concentrates them into long wind-aligned foam streaks for gale conditions.",
            reset(theme::Reset::Ocean(OCEAN_WIND_FOAM)),
            |ui| {
                let preview = theme::preview_toggle_value(ui.ctx(), reset(theme::Reset::Ocean(OCEAN_WIND_FOAM)), settings.wind_foam_streaks);
                if let Some(value) = theme::switch(ui, preview) {
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
             high is a dull matte sheet. Locked, it follows the swell amplitude              (calm water glossy, a full sea at the upstream 0.65); unlock to set it yourself.",
            reset(theme::Reset::Ocean(OCEAN_ROUGHNESS)),
            |ui| {
                let locked = links & crate::ocean::links::ROUGHNESS != 0;
                let mut value = settings.roughness;
                let readout = percent(value);
                let moved = ui
                    .add_enabled_ui(!locked, |ui| theme::slider(ui, &mut value, 0.0..=1.0, &readout))
                    .inner;
                ui.add_space(8.0);
                if let Some(now_locked) = theme::lock_toggle(ui, locked) {
                    if now_locked { links |= crate::ocean::links::ROUGHNESS } else { links &= !crate::ocean::links::ROUGHNESS }
                    changed = true;
                } else if moved {
                    settings.roughness = value;
                    changed = true;
                }
            },
        );
        theme::row(
            ui,
            "Normal strength",
            "How strongly the simulated ripple normals perturb the lighting.",
            reset(theme::Reset::Ocean(OCEAN_NORMAL_STRENGTH)),
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
            reset(theme::Reset::Ocean(OCEAN_WATER_COLOR)),
            &mut settings.water_color,
            &mut changed,
        );
        color_row(
            ui,
            "Foam color",
            "Colour of whitecaps and the foam trail behind breaking waves.",
            reset(theme::Reset::Ocean(OCEAN_FOAM_COLOR)),
            &mut settings.foam_color,
            &mut changed,
        );

        ui.add_enabled_ui(promoted_index.is_none(), |ui| {
        theme::section(ui, "WATER OPTICS", "");
        color_row(
            ui,
            "Fog color",
            "Scattered underwater light; converted to linear color.",
            reset(theme::Reset::Ocean(9)),
            &mut settings.optics.fog_color,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Transparency",
            "How far light travels through the water before it is absorbed, in game units. Lower is murkier, above and below the surface.",
            reset(theme::Reset::Ocean(10)),
            &mut settings.optics.transparency,
            1.0..=30000.0,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Depth darkening",
            "Sunlight penetration multiplier; larger values stay bright deeper.",
            reset(theme::Reset::Ocean(12)),
            &mut settings.optics.depth_darkening,
            0.01..=8.0,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Refraction",
            "Wave distortion of objects viewed through water. Zero disables distortion.",
            reset(theme::Reset::Ocean(13)),
            &mut settings.optics.refraction,
            0.0..=0.15,
            &mut changed,
        );
        ocean_slider(
            ui,
            "Caustics",
            "Experimental wave-curvature sunlight focusing. Not shadow-aware yet; zero disables.",
            reset(theme::Reset::Ocean(14)),
            &mut settings.optics.caustics,
            0.0..=4.0,
            &mut changed,
        );
        ocean_slider_dd(ui, "Caustic view distance", "Camera-to-terrain receiver range in metres. Caustics fade over the final 25%; 0 disables distance culling. Does not change fog/refraction.", theme::Reset::None, &mut settings.optics.caustic_max_distance_m, dopt.caustic_max_distance_m, 0.0..=500.0, &mut changed);
        ocean_slider(ui, "Underwater cull", "Skips geometry the water already hides, as a multiple of the absorption distance (the transparency distance; never below 2, where 0.2% of the light is left). Underwater: everything beyond it (never closer than 256), plus the far ocean sheet. From above: anything lying under that much water. Murkier water culls more. Zero disables.", reset(theme::Reset::Ocean(15)), &mut settings.optics.underwater_cull, 0.0..=8.0, &mut changed);

        });
        ui.add_enabled_ui(promoted_index.is_none(), |ui| {
        theme::section(
            ui,
            "ADVANCED SPECTRUM",
            "Shared by all pools. Three layered FFT spectra reduce tiling. These controls use the native GodotOceanWaves units: metres, m/s and km.",
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
                    ocean_slider(ui, "Tile length X", "Repeat size of this FFT cascade along X, in metres. Larger values carry broader waves and repeat less often.", reset(theme::Reset::OceanCascade(cascade_index as u8, 0)), &mut cascade.tile_length[0], 1.0..=2000.0, &mut changed);
                    ocean_slider(ui, "Tile length Y", "Repeat size of this FFT cascade along Y, in metres. Keeping X/Y different can make repetition less obvious.", reset(theme::Reset::OceanCascade(cascade_index as u8, 1)), &mut cascade.tile_length[1], 1.0..=2000.0, &mut changed);
                    ocean_slider(ui, "Displacement scale", "Multiplier for this cascade's geometric displacement. Reduce it on higher-frequency cascades to avoid over-busy silhouettes.", reset(theme::Reset::OceanCascade(cascade_index as u8, 2)), &mut cascade.displacement_scale, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Normal scale", "Multiplier for this cascade's shading normals. It can add small-scale sparkle/chop without adding the same amount of geometry displacement.", reset(theme::Reset::OceanCascade(cascade_index as u8, 3)), &mut cascade.normal_scale, 0.0..=2.0, &mut changed);
                    if cascade_index < 2 {
                        ocean_slider(ui, "Spectrum wind speed", "Reference wind speed for this TMA/JONSWAP spectrum, in m/s. It changes spectrum energy and peak frequency; it is separate from live weather wind.", reset(theme::Reset::OceanCascade(cascade_index as u8, 4)), &mut cascade.wind_speed, 0.0001..=60.0, &mut changed);
                        ocean_slider(ui, "Spectrum angle offset", "Directional offset from the authored swell heading, in degrees. This lets the cascades cross rather than stack in exactly one direction.", reset(theme::Reset::OceanCascade(cascade_index as u8, 5)), &mut cascade.wind_direction, -360.0..=360.0, &mut changed);
                    } else {
                        theme::banner(
                            ui,
                            "This cascade follows the weather wind speed and direction.",
                            theme::TEXT_FAINT,
                        );
                    }
                    ocean_slider(ui, "Fetch length", "Distance from shoreline / wind fetch, in kilometres. Longer fetch shifts the TMA/JONSWAP sea state toward more developed waves.", reset(theme::Reset::OceanCascade(cascade_index as u8, 6)), &mut cascade.fetch_length, 0.1..=1000.0, &mut changed);
                    ocean_slider(ui, "Directional concentration", "GodotOceanWaves calls this swell. Higher values elongate/concentrate wave energy around the preferred heading; it is not your authored swell height.", reset(theme::Reset::OceanCascade(cascade_index as u8, 7)), &mut cascade.swell, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Spread", "Mix between strongly directional and flatter/isotropic wave energy. Higher values allow more energy away from the preferred heading.", reset(theme::Reset::OceanCascade(cascade_index as u8, 8)), &mut cascade.spread, 0.0..=1.0, &mut changed);
                    ocean_slider(ui, "Detail", "Small-wave suppression control. Lower values attenuate high-frequency waves; 1 keeps the full high-frequency tail.", reset(theme::Reset::OceanCascade(cascade_index as u8, 9)), &mut cascade.detail, 0.0..=1.0, &mut changed);
                    ocean_slider(ui, "Whitecap threshold", "Controls how steep/compressed a crest must be before foam accumulates. Higher values make whitecaps trigger more readily in this implementation.", reset(theme::Reset::OceanCascade(cascade_index as u8, 10)), &mut cascade.whitecap, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Foam amount", "Per-cascade foam growth multiplier. Lifetime/decay is controlled by the main Foam lifetime setting above.", reset(theme::Reset::OceanCascade(cascade_index as u8, 11)), &mut cascade.foam_amount, 0.0..=10.0, &mut changed);
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

        });
        ui.add_space(10.0);
        if theme::ghost_button(ui, "RESET OCEAN DEFAULTS").clicked() {
            if let Some(index) = promoted_index {
                let slot = self.promoted_oceans[index].index;
                self.promoted_ocean_overrides.retain(|(i, _)| *i != slot);
                self.render_command(crate::renderer::RenderCommand::SetPromotedOceanSettings(self.promoted_ocean_overrides.clone()));
                self.publish_ui();
                return;
            }
            settings = crate::ocean::OceanSettings::default();
            changed = true;
        }

        if changed {
            settings.links = links;
            if links != self.video.ocean_settings.links {
                self.video.ocean_settings.links = links;
                if promoted_index.is_some() { self.commit_ocean_settings(); }
            }
            if let Some(index) = promoted_index {
                let slot = self.promoted_oceans[index].index;
                self.promoted_ocean_overrides.retain(|(i,_)| *i != slot);
                self.promoted_ocean_overrides.push((slot, settings));
                self.render_command(crate::renderer::RenderCommand::SetPromotedOceanSettings(self.promoted_ocean_overrides.clone()));
                self.publish_ui();
            } else {
                self.video.ocean_settings = settings;
                self.commit_ocean_settings();
            }
        }
    }
}
