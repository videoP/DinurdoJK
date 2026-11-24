//! Effects.
use crate::app::egui_settings::{
    index_of, segmented_row, theme, ui, App, FootprintMode, Ghoul2SkinningMode,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_effects(&mut self, ui: &mut egui::Ui) {
        if let Some(game) = segmented_row(
            ui, "FX FPS game",
            "Apply the fixed FX cadence to JKA, JK2 or both. The excluded game uses the existing legacy per-frame FX path without changing your saved rate or scope.",
            theme::Reset::Video(ui::VIDEO_ROW_FX_FPS_GAME), self.video.fx_fps_game,
            &[(crate::fx::FX_FPS_GAME_JKA, "JKA only"), (crate::fx::FX_FPS_GAME_JK2, "JK2 only"), (crate::fx::FX_FPS_GAME_BOTH, "Both")],
        ) {
            let _ = self.set_console_cvar("cg_fxFPSGame", &game.to_string());
        }
        // Keep both saved values visible but inactive in an excluded game.
        let fixed_fx_available = crate::fx::fx_fps_game_enabled(
            self.video.fx_fps_game, self.fs_roots.active_jk2.is_some(),
        );
        ui.add_enabled_ui(fixed_fx_available, |ui| {
        theme::row(
            ui,
            "FX FPS",
            "Fixed FX worker update rate and projectile/trail emission sampling. Worker snapshots \
             are prepared asynchronously while the renderer reuses the last completed FX \
             visuals; authored EFX lifetimes and counts are preserved. Drag to the far-right \
             Legacy endpoint for synchronous per-frame updates (cg_fxFPS 0).",
            theme::Reset::Video(ui::VIDEO_ROW_FX_FPS),
            |ui| {
                const LEGACY_SLIDER_VALUE: f32 = crate::fx::FX_FPS_MAX as f32 + 1.0;
                let mut slider_value = if self.video.fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
                    LEGACY_SLIDER_VALUE
                } else {
                    self.video.fx_fps.clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX) as f32
                };
                let readout = if self.video.fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
                    "Legacy (per-frame)".to_owned()
                } else {
                    format!("{} Hz", self.video.fx_fps)
                };
                if theme::slider(
                    ui,
                    &mut slider_value,
                    crate::fx::FX_FPS_MIN as f32..=LEGACY_SLIDER_VALUE,
                    &readout,
                ) {
                    let value = if slider_value >= LEGACY_SLIDER_VALUE {
                        crate::fx::FX_FPS_LEGACY_JKA
                    } else {
                        (slider_value.round() as u32)
                            .clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX)
                    };
                    let _ = self.set_console_cvar("cg_fxFPS", &value.to_string());
                }
            },
        );

        const FX_FPS_SCOPE: [(u32, &str); 2] = [
            (crate::fx::FX_FPS_SCOPE_CONTINUOUS, "Continuous"),
            (crate::fx::FX_FPS_SCOPE_FRAME_DRIVEN, "All frame-driven"),
        ];
        if let Some(scope) = segmented_row(
            ui,
            "FX FPS scope",
            "Continuous (default) only resamples projectile/trail EFX whose stock density accidentally follows render FPS. All frame-driven also applies cg_fxFPS to eligible stock presentation-frame effects such as legacy saber wall sparks and mark sampling. Authored .efx count/life/delay values are still respected.",
            theme::Reset::Video(ui::VIDEO_ROW_FX_FPS_SCOPE),
            self.video.fx_fps_scope,
            &FX_FPS_SCOPE,
        ) {
            let _ = self.set_console_cvar("cg_fxFPSScope", &scope.to_string());
        }

        });

        ui.add_enabled_ui(
            self.video.ghoul2_skinning == Ghoul2SkinningMode::Gpu && self.audio.game.blood == 0,
            |ui| {
                if let Some(value) = segmented_row(
                    ui,
                    "Legacy burn marks",
                    "cg_ghoul2Marks limits the legacy scorch marks kept per model when Blood effects are off. Requires GPU Ghoul2 skinning (Video > Models).",
                    theme::Reset::None,
                    self.audio.game.g2_marks,
                    &[(0, "Off"), (4, "4"), (16, "16"), (32, "32")],
                ) {
                    let _ = self.set_console_cvar("cg_ghoul2Marks", &value.to_string());
                }
            },
        );

        const FX_PHYSICS: [(u32, &str); 3] = [
            (crate::fx::FX_PHYSICS_OFF, "Off"),
            (crate::fx::FX_PHYSICS_AUTHORED, "Authored"),
            (crate::fx::FX_PHYSICS_ALL, "All"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "FX physics",
            "World collision for bouncing EFX debris (fx_physics). Particles, tails and \
             emitters bounce off the map using the exact OpenJK collision world, spawn their authored \
             impact effects and stop when they settle. Off lets them fly through walls. Authored (default) \
             traces only primitives the effect author marked expensivePhysics (rock falls, dust, \
             debris). All forces the trace on every primitive that has physics enabled (glass, sparks), which costs one collision trace per such particle per frame on the FX thread. \
             Stock level 1 behaves like Off.",
            theme::Reset::Video(ui::VIDEO_ROW_FX_PHYSICS),
            self.video.fx_physics,
            &FX_PHYSICS,
        ) {
            let index_of_mode = |mode: u32| FX_PHYSICS.iter().position(|(value, _)| *value == mode).unwrap_or(1);
            self.video_selected = ui::VIDEO_ROW_FX_PHYSICS;
            self.change_video_setting(
                index_of_mode(target) as i32 - index_of_mode(self.video.fx_physics) as i32,
            );
        }

        const FX_LOD: [(u32, &str); 3] = [
            (crate::fx::FX_LOD_OFF, "Off"),
            (crate::fx::FX_LOD_AUTHORED, "Authored"),
            (crate::fx::FX_LOD_ADAPTIVE, "Adaptive"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "FX LOD",
            "Spawn-time EFX level of detail (fx_lod); live particles are never thinned. Off is stock. Authored honors the cullRange that effect authors put on smoke, sparks, fire and impacts (stock OpenJK ignores it), skipping those primitives when the effect is farther than the authored range. Adaptive (default) also spawns fewer particles/tails for populations whose individual elements project to only a few pixels; near or large effects stay exactly as authored. Sounds, camera shakes, lights and single sparks are never reduced.",
            theme::Reset::Video(ui::VIDEO_ROW_FX_LOD),
            self.video.fx_lod,
            &FX_LOD,
        ) {
            let index_of_mode = |mode: u32| FX_LOD.iter().position(|(value, _)| *value == mode).unwrap_or(2);
            self.video_selected = ui::VIDEO_ROW_FX_LOD;
            self.change_video_setting(
                index_of_mode(target) as i32 - index_of_mode(self.video.fx_lod) as i32,
            );
        }

        ui.add_enabled_ui(self.video.fx_lod != crate::fx::FX_LOD_OFF, |ui| {
        theme::row(
            ui,
            "FX LOD scale",
            "r_fxLodScale: multiplies every authored EFX cullRange, like r_lodscale does for models (default 5; 1 is the raw authored range, which is short). Also shifts where Adaptive FX LOD starts thinning particles. Needs FX LOD set to Authored or Adaptive.",
            theme::Reset::None,
            |ui| {
                let mut scale = self.video.fx_lod_scale;
                if ui
                    .add(egui::DragValue::new(&mut scale).range(0.5..=20.0).speed(0.1).update_while_editing(false))
                    .changed()
                {
                    let _ = self.set_console_cvar("r_fxLodScale", &scale.to_string());
                }
            },
        );
        });

        theme::row(ui, "Wall mark count",
            "r_markcount: maximum retained authored EFX wall marks, default 100. Zero disables them. BSP marks appear only on markable surfaces.",
            theme::Reset::None, |ui| {
                let mut count = self.video.mark_count;
                if ui.add(egui::DragValue::new(&mut count).range(0..=4096).speed(1.0)).changed() {
                    let _ = self.set_console_cvar("r_markcount", &count.to_string());
                }
            });

        self.egui_toggle_row(
            ui,
            "Modern saber rendering",
            "Uses a continuous view-facing glow ribbon with the stock saber shaders instead of \
             OpenJK's chain of glow sprites. The authored line/glow textures and exact additive \
             blend rules are preserved. Off is the OpenJK-compatible presentation.",
            ui::VIDEO_ROW_MODERN_SABERS,
            self.video.modern_sabers,
        );

        ui.add_enabled_ui(!self.video.modern_sabers, |ui| {
            const CORE_MODES: [(u32, &str); 4] = [
                (0, "Original"),
                (1, "SBAK"),
                (2, "Both"),
                (3, "Glow only"),
            ];
            if let Some(mode) = segmented_row(
                ui,
                "Legacy saber core",
                "r_saberCoreMode: Original = normal OpenJK core only; SBAK = thin pulsing \
                 0.8..0.9-unit core only; Both = normal plus thin pulsing core; Glow only = \
                 neither core. The legacy glow remains in every mode. Does not affect Modern sabers.",
                theme::Reset::Video(ui::VIDEO_ROW_SABER_CORE_MODE),
                self.video.saber_core_mode,
                &CORE_MODES,
            ) {
                let _ = self.set_console_cvar("r_saberCoreMode", &mode.to_string());
            }
        });

        const FLARE_MODES: [(u8, &str); 3] = [
            (0, "Off"), (1, "Legacy"), (2, "Enhanced"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Map flares",
            "r_flares: Off hides authored BSP world flares; Legacy preserves the original \
             textured shader sprites; Enhanced replaces their appearance with a \
             mu6k/ShaderToy-style procedural lens flare. The authored flare position, \
             BSP tint, chosen shader image/color, occlusion and fog remain in use. \
             Saber clash flares are separate.",
            theme::Reset::Video(ui::VIDEO_ROW_FLARES),
            self.video.world_flare_mode(),
            &FLARE_MODES,
        ) {
            self.video_selected = ui::VIDEO_ROW_FLARES;
            let _ = self.set_console_cvar("r_flares", &target.to_string());
        }

        ui.add_enabled_ui(self.video.world_flare_mode() == 2, |ui| {
            theme::row(
                ui,
                "Enhanced flare intensity",
                "r_flareEnhancedIntensity: final brightness of Enhanced map lens flares. \
                 0% hides lens light, 12% is the balanced default, 100% is the unscaled \
                 contribution. Multiplies authored tint after the lens equations, preserving \
                 shape and color. Legacy and saber flares are unaffected.",
                theme::Reset::None,
                |ui| {
                    let mut percent = self.video.flare_enhanced_intensity * 100.0;
                    let readout = format!("{:.0}%", percent);
                    if theme::slider(ui, &mut percent, 0.0..=100.0, &readout) {
                        let value = (percent / 100.0).clamp(0.0, 1.0);
                        let _ = self.set_console_cvar("r_flareEnhancedIntensity", &format!("{value:.3}"));
                    }
                },
            );
            theme::row(
                ui,
                "Enhanced flare source strength",
                "r_flareEnhancedCore: scales only the narrow bright peak at the authored \
                 light source. It is independent of the broad wash, so you can make \
                 the origin bright without filling the screen. 1.0x = original \
                 source peak; 3.0x = default. Legacy is unaffected.",
                theme::Reset::None,
                |ui| {
                    let mut core = self.video.flare_enhanced_core;
                    let readout = format!("{core:.1}x");
                    if theme::slider(ui, &mut core, 0.0..=8.0, &readout) {
                        let _ = self.set_console_cvar("r_flareEnhancedCore", &format!("{core:.3}"));
                    }
                },
            );
            theme::row(
                ui,
                "Enhanced flare wash",
                "r_flareEnhancedWash: strength of the broad screen-filling haze component                  of the Enhanced lens flare. 0% keeps the localized lens/ghost features                  while removing the veil; 10% is the balanced default; 100% preserves                  the fully ported wash strength.",
                theme::Reset::None,
                |ui| {
                    let mut percent = self.video.flare_enhanced_wash * 100.0;
                    let readout = format!("{:.0}%", percent);
                    if theme::slider(ui, &mut percent, 0.0..=100.0, &readout) {
                        let value = (percent / 100.0).clamp(0.0, 1.0);
                        let _ = self.set_console_cvar("r_flareEnhancedWash", &format!("{value:.3}"));
                    }
                },
            );
            theme::row(
                ui,
                "Enhanced flare localization",
                "r_flareEnhancedLocality: screen-distance falloff for Enhanced flares.                  0% keeps the fully broad Shadertoy footprint; 70% is the balanced default;                  100% confines both the lens and wash much closer to the authored source.",
                theme::Reset::None,
                |ui| {
                    let mut percent = self.video.flare_enhanced_locality * 100.0;
                    let readout = format!("{:.0}%", percent);
                    if theme::slider(ui, &mut percent, 0.0..=100.0, &readout) {
                        let value = (percent / 100.0).clamp(0.0, 1.0);
                        let _ = self.set_console_cvar("r_flareEnhancedLocality", &format!("{value:.3}"));
                    }
                },
            );
        });

        self.egui_toggle_row(
            ui,
            "Saber flares (r_saberFlares)",
            "Only the screen-space saber clash flash; other impact EFX, sounds, and marks are unaffected.",
            ui::VIDEO_ROW_SABER_FLARES,
            self.video.saber_flares,
        );

        self.egui_toggle_row(
            ui,
            "Saber impact effects",
            "Authored OpenJK saber hit/block EFX (sparks, smoke and any EFX-owned lights). \
             Off hides the complete tagged impact-effect tree while leaving saber blades, flares, \
             sounds and saber marks alone. Existing live impact primitives are retained, so this can \
             be toggled while a demo is paused to A/B the exact same frozen FX population.",
            ui::VIDEO_ROW_SABER_IMPACT_FX,
            self.video.saber_impact_fx,
        );

        const SABER_MARK_OPTIONS: [(ui::SaberMarkMode, &str); 3] = [
            (ui::SaberMarkMode::Off, "Off"),
            (ui::SaberMarkMode::Legacy, "Legacy"),
            (ui::SaberMarkMode::Enhanced, "Enhanced"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Saber marks",
            "Saber/world contact presentation. Legacy follows OpenJK: contact is evaluated once per \
             presentation frame, the stock burn/glow mark lasts 10 seconds, and saberhitwall audio \
             uses the original 100 ms debounce. Enhanced is a separate smooth molten-surface effect: \
             movement lays a spatially sampled curved melt path, while holding the blade in one place \
             accumulates heat, widens/raises the molten lips, makes the material sag under gravity, \
             and grows smooth sludge/drips before cooling to a dark scar. cg_fxFPS does not control \
             saber/world marks. This is presentation-only.",
            theme::Reset::Video(ui::VIDEO_ROW_SABER_MARKS),
            self.video.saber_marks,
            &SABER_MARK_OPTIONS,
        ) {
            let current = index_of(&SABER_MARK_OPTIONS, self.video.saber_marks, 1);
            let next = index_of(&SABER_MARK_OPTIONS, target, current);
            self.video_selected = ui::VIDEO_ROW_SABER_MARKS;
            self.change_video_setting(next as i32 - current as i32);
        }
        self.egui_water_wake(ui);
        self.egui_ninja_effects(ui);
    }

    fn egui_water_wake(&mut self, ui: &mut egui::Ui) {
        // Global player wake, independent of the water renderer: one setting
        // for stock JKA water, promoted oceans, lakes and the shared Fluid
        // Ninja river/ocean wake.
        theme::section(ui, "WATER WAKE", "Player wakes on every water type. Console: cg_waterwake.");
        theme::row(ui, "Water wake",
            "Off disables all player wakes, including the Fluid Ninja river and Ninja ocean wake. Regular uses the original JKA wake on unpromoted water and the shared Fluid Ninja wake on Ninja rivers and the Ninja ocean. Enhanced also animates the JKA wake and enables the promoted-water deformation below (Godot, Blue Water, Hybrid and lakes). This does not change ship wakes or ambient wave foam.",
            theme::Reset::None, |ui| {
                for (mode, label) in [(0u8, "Off"), (1, "Regular"), (2, "Enhanced")] {
                    if theme::chip(ui, label, self.video.water_wake == mode).clicked()
                        && self.video.water_wake != mode
                    {
                        let _ = self.set_console_cvar("cg_waterwake", &mode.to_string());
                    }
                    ui.add_space(3.0);
                }
            });

        let mut interaction = self.video.ocean_settings.interaction;
        let mut strength = self.video.ocean_settings.interaction_strength;
        let mut splash_mode = self.video.ocean_settings.interaction_splash_mode;
        let toggled = ui.checkbox(&mut interaction, "Player water deformation").on_hover_text(
            "Global player displacement for Godot, Blue Water, Hybrid and supported lake surfaces. Requires Enhanced wake.").changed();
        let adjusted = ui.horizontal(|ui| {
            ui.label("Deformation strength (all water renderers)");
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
    }

    fn egui_ninja_effects(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "FLUID NINJA", "Fire, ground fog, blood, and snow presentation. These use the existing Fluid Ninja implementation and its console commands, not a second solver. Diagnostic emitters are local to the current map/session.");
        if !jka_ninja::INCLUDED {
            theme::hint(ui, "The private Fluid Ninja crate is not included in this build. Fire, fog and blood diagnostics require that implementation.", theme::TEXT_FAINT());
        }
        ui.add_enabled_ui(jka_ninja::INCLUDED, |ui| {
            theme::section(ui, "FIRE", "Place bonfires, candles, torches or log fires along your view, or ignite the player for the Burning Man effect.");
            {
                self.ninja_toggle(ui, "r_ninjaFirePromote", crate::ninja_fire_debug::promote(),
                    "Promote authored fire effects",
                    "Replace supported stationary ET_FX flames with Ninja fire and Burning Man sparks. The closest four eligible flames are simulated.");
                self.ninja_toggle(ui, "r_ninjaSmokePromote", crate::ninja_fire_debug::promote_smoke(),
                    "Promote authored smoke effects",
                    "Replace upright particle smoke and steam from the two nearest eligible continuous map emitters with Ninja smoke once their volumes are ready.");
                theme::row(
                    ui,
                    "Smoke lifetime",
                    "How long promoted EFX smoke lingers (0.25x–8x, default 1x).",
                    theme::Reset::None,
                    |ui| {
                        let mut scale = self.video.fx_smoke_duration_scale;
                        let readout = format!("{scale:.2}x");
                        let changed = theme::slider(ui, &mut scale, crate::fx::SMOKE_DURATION_SCALE_MIN..=crate::fx::SMOKE_DURATION_SCALE_MAX, &readout);
                        // Double-click the label or the slider: the cvar's default (1x).
                        let reset = theme::row_label_double_clicked(ui)
                            || ui.ctx().input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary))
                                && ui.min_rect().contains(ui.ctx().input(|i| i.pointer.interact_pos()).unwrap_or_default());
                        if reset {
                            let default = crate::app::commands::registry::cvar_registry().find("r_fxSmokeDurationScale")
                                .map_or_else(|| "1".to_owned(), |def| def.entry.default.to_string());
                            let _ = self.set_console_cvar("r_fxSmokeDurationScale", &default);
                        } else if changed {
                            let _ = self.set_console_cvar("r_fxSmokeDurationScale", &format!("{scale:.2}"));
                        }
                    },
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label("Spawn at view:");
                    for (name, command) in [
                        ("Bonfire", "ninjafire bonfire"), ("Candle", "ninjafire candle"),
                        ("Torch", "ninjafire torch"), ("Log fire", "ninjafire logfire"),
                    ] {
                        if ui.button(name).on_hover_text(format!("Run /{command} (96 units in front of the view)")).clicked() {
                            self.execute_command_line(command);
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Burning Man ON").on_hover_text("Run /ninjafireon on the presented player").clicked() {
                        self.execute_command_line("ninjafireon");
                    }
                    if ui.button("Smoking Man ON").on_hover_text("Run /ninjafireon smokingman: the Fire Smoke demo's key-2 pawn (volume smoke)").clicked() {
                        self.execute_command_line("ninjafireon smokingman");
                    }
                    if ui.button("Smoking Man (always) ON").on_hover_text("Run /ninjafireon smokingman2: Smoking Man that keeps smoking while you stand still").clicked() {
                        self.execute_command_line("ninjafireon smokingman2");
                    }
                    if ui.button("Burning + Smoking Man ON").on_hover_text("Run /ninjafireon burningman3: the Fire Smoke demo's key-3 pawn (flame plus volume smoke)").clicked() {
                        self.execute_command_line("ninjafireon burningman3");
                    }
                    if ui.button("Burning + Smoking Man (always) ON").on_hover_text("Run /ninjafireon burningman3b: Burning + Smoking Man whose smoke keeps going while you stand still").clicked() {
                        self.execute_command_line("ninjafireon burningman3b");
                    }
                    if ui.button("Blue Fist ON").on_hover_text("Run /ninjafireon fist: Stage 11 Flaming Fist B (blue smoke from the left hand)").clicked() {
                        self.execute_command_line("ninjafireon fist");
                    }
                    if ui.button("Player effect OFF").clicked() { self.execute_command_line("ninjafireoff"); }
                    if ui.button("Clear flames").clicked() { self.execute_command_line("ninjafireclear"); }
                });
                self.ninja_toggle(ui, "r_ninjaFireFlicker", crate::ninja_fire_debug::flicker(), "Flickering fire light", "Animate the existing Burning Man light's intensity; off restores steady intensity.");
                self.ninja_toggle(ui, "r_ninjaFireAirflow", crate::ninja_fire_debug::airflow_response(), "Airflow response", "Thin the outer flame in fast air and flare as the airflow subsides.");
                self.ninja_toggle(ui, "r_ninjaFireCollision", crate::ninja_fire_debug::collision(), "Brush collision", "Use Fluid Ninja MeshField collision against solid map brushes.");
                self.ninja_toggle(ui, "r_ninjaFireEmbers", crate::ninja_fire_debug::embers(), "Fire embers", "Fire Smoke flamethrower-turret embers from every Ninja fire source: bright streaks and soft glowing dots that fall, bounce and settle on the floor.");
            }

            theme::section(ui, "FOG", "Fog Mist follows the player and reacts to movement.");
            self.ninja_toggle(ui, "r_ninjaFog", crate::ninja_fog_debug::enabled(), "Fluid Ninja fog", "Enable test fog. Disabling removes the active volume.");
            ui.add_enabled_ui(crate::ninja_fog_debug::enabled(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Fog Mist (follows player)").on_hover_text("Run /ninjafog mist").clicked() {
                        self.execute_command_line("ninjafog mist");
                    }
                    if ui.button("Clear fog").clicked() { self.execute_command_line("ninjafogclear"); }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Fixed test volumes");
                    for (name, command) in [
                        ("Low lying", "ninjafog lowlying"), ("Plastic", "ninjafog plastic"),
                        ("Scattered", "ninjafog scattered"),
                    ] {
                        if ui.button(name).on_hover_text(format!("Run /{command}")).clicked() {
                            self.execute_command_line(command);
                        }
                    }
                });
            });

            theme::section(ui, "BLOOD", "Animated saber spurts and interactive surface blood.");
            if let Some(mode) = segmented_row(
                ui, "Blood effects",
                "cg_blood 1: animated Ninja blood, surface pools, SkinGore and a skull/brain gib. cg_blood 2: full TaystJK gib models with Ninja blood droplets/marks instead of legacy bloodTrail/bloodMark. SkinGore requires GPU Ghoul2 skinning; gib MD3 assets must be available.",
                theme::Reset::None, self.audio.game.blood,
                &[(0, "Off"), (1, "Blood"), (2, "Blood + gibs")],
            ) {
                let _ = self.set_console_cvar("cg_blood", &mode.to_string());
            }
            ui.add_enabled_ui(crate::ninja_blood::enabled(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Test:");
                    for (label, command) in [
                        ("Spurt", "ninjablood spurt"), ("Leak", "ninjablood leak"),
                        ("Gush", "ninjablood gush"), ("Impact", "ninjablood hit"),
                    ] {
                        if ui.button(label).on_hover_text(format!("Run /{command}")).clicked() {
                            self.execute_command_line(command);
                        }
                    }
                    if ui.button("Stop bleeding").on_hover_text("Stop the local leak or gush").clicked() {
                        self.execute_command_line("ninjablood");
                    }
                });
            });
        });
        theme::section(ui, "SNOW", "The existing footprint modes; Ninja composite selects the Fluid Ninja painter and composite presentation when available.");
        const FOOTPRINTS: [(FootprintMode, &str); 5] = [
            (FootprintMode::Off, "Off"), (FootprintMode::TwoD, "2D"),
            (FootprintMode::ThreeD, "3D"), (FootprintMode::Ninja, "Ninja"),
            (FootprintMode::NinjaComposite, "Ninja composite"),
        ];
        if let Some(mode) = segmented_row(ui, "Snow footprints", "r_footprints: same choice as the surface settings page. Controls snow and other footprint media using the existing renderer modes.", theme::Reset::None, self.video.footprints, &FOOTPRINTS) {
            let _ = self.set_console_cvar("r_footprints", mode.config_value());
        }
        ui.horizontal(|ui| {
            if ui.button("Reset Ninja diagnostics")
                .on_hover_text("Restore the registered session defaults for Ninja fire and fog")
                .clicked()
            {
                for name in NINJA_CVARS {
                    if let Some(def) = crate::app::commands::registry::cvar_registry().find(name) {
                        let default = def.entry.default.to_owned();
                        let _ = self.set_console_cvar(name, &default);
                    }
                }
            }
        });
    }

    fn ninja_toggle(&mut self, ui: &mut egui::Ui, name: &str, current: bool, label: &str, tip: &str) {
        let value = theme::row(ui, label, tip, theme::Reset::None, |ui| {
            let mut value = current;
            let response = ui.checkbox(&mut value, "Enabled");
            if theme::row_label_double_clicked(ui) {
                crate::app::commands::registry::cvar_registry().find(name)
                    .map(|def| def.entry.default == "1")
            } else if response.changed() { Some(value) } else { None }
        });
        if let Some(value) = value {
            let _ = self.set_console_cvar(name, if value { "1" } else { "0" });
        }
    }
}

const NINJA_CVARS: &[&str] = &[
    "r_ninjaFirePromote", "r_ninjaSmokePromote", "r_ninjaFireFlicker", "r_ninjaFireAirflow",
    "r_ninjaFireCollision", "r_ninjaFireEmbers",
    "r_ninjaFog",
];
