//! Runtime overrides of authored map illumination.
use crate::app::egui_settings::{
    segmented_row, theme, ui, App, DynamicShadowsMode, EntityAmbientLightingMode, VideoSection, VideoSettings,
};

impl App {
    /// A baked-lighting slider. Double-click the slider or its label to restore
    /// `default`.
    #[allow(clippy::too_many_arguments)]
    fn baked_slider(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        reset: theme::Reset,
        range: std::ops::RangeInclusive<f32>,
        default: f32,
        field: fn(&mut VideoSettings) -> &mut f32,
    ) -> bool {
        let mut changed = false;
        theme::row(ui, label, tip, reset, |ui| {
            let mut value = *field(&mut self.video);
            let readout = format!("{value:.2}");
            if theme::slider_default(ui, &mut value, range, &readout, default) {
                *field(&mut self.video) = value;
                self.sync_post_effects();
                self.mark_config_dirty();
                changed = true;
            }
        });
        changed
    }

    pub(in crate::app::egui_settings) fn egui_relighting(&mut self, ui: &mut egui::Ui) {
        let defaults = VideoSettings::default();
        theme::row(ui, "Relight map", "Enable runtime changes to authored map illumination. Off restores the original lighting contribution while preserving your edits. Sun and moon position remain on Sky. Does not modify the BSP or write lightmaps to disk.", theme::Reset::Environment(ui::ENV_ROW_SUN_SOURCE), |ui| {
            if let Some(enabled) = theme::switch(ui, self.video.sun_override) {
                self.set_sun_override(enabled, enabled);
            }
        });
        if theme::ghost_button(ui, "OPEN SKY / SUN & MOON").clicked() {
            self.video_section = VideoSection::Sun;
        }
        if theme::ghost_button(ui, "RESET ALL RELIGHTING SETTINGS").clicked() {
            self.reset_relighting_page();
        }
        if !self.video.sun_override {
            theme::banner(ui, "Relighting is off. Your settings are retained; the map uses its authored lighting. Lamp shadows remain available in Lighting.", theme::TEXT_DIM);
        }
        ui.add_enabled_ui(self.video.sun_override, |ui| {
            let authored = self.map_sun_editor_values();
            theme::section(ui, "SUN REPLACEMENT", "Sun direction, color and intensity are edited on Sky. Subtraction removes the old baked sun; live sunlight adds the current sun separately.");
            theme::row(ui, "Add live world sunlight", "Add the current sun to world surfaces. Enable Cascaded Shadow Maps under Lighting for moving casters; turn on static-world CSM below if you also want BSP casters. Subtract baked sunlight below to reduce duplicate sunlight.", theme::Reset::None, |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.sun_lighting) {
                    let _ = self.set_console_cvar("r_sunLighting", if enabled { "1" } else { "0" });
                }
            });
            let entity_csm = self.video.dynamic_shadows == DynamicShadowsMode::CascadedShadowMapsEntities;
            ui.add_enabled_ui(entity_csm, |ui| {
                theme::row(
                    ui,
                    "CSM for static world",
                    "Include static BSP geometry, map props and static occluders in the sun's cascades, rather than using only baked static shadows. Requires Lighting > Cascaded Shadow Maps and Relight map. More depth rendering and GPU cost; independent of local lamp shadow cubemaps.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(enabled) = theme::switch(ui, self.video.static_world_csm) {
                            let _ = self.set_console_cvar("r_staticWorldCsm", if enabled { "1" } else { "0" });
                        }
                    },
                );
            });
            if !entity_csm {
                ui.small("Enable Cascaded Shadow Maps in Lighting to use static-world CSM.");
            }
            const ENTITY_SUN: [(bool, &str); 2] = [(false, "Baked"), (true, "Runtime sun")];
            ui.add_enabled_ui(
                self.video.entity_ambient_lighting == EntityAmbientLightingMode::BspLightgridClassic,
                |ui| {
                    if let Some(target) = segmented_row(
                        ui,
                        "Entity sun",
                        "Baked keeps the stock lightgrid on players and models. Runtime sun estimates how much of each lightgrid probe is the map's baked sun, removes it, and lights entities with the runtime sun instead (color, intensity and direction, including the sun configured on Sky). Indoor and torch-lit probes are left alone. Requires Entity ambient lighting = BSP lightgrid.",
                        theme::Reset::Environment(ui::ENV_ROW_ENTITY_SUN_LIGHTING),
                        self.video.entity_sun_lighting,
                        &ENTITY_SUN,
                    ) {
                        self.environment_selected = ui::ENV_ROW_ENTITY_SUN_LIGHTING;
                        self.video.entity_sun_lighting = target;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                    }
                },
            );
            // ------------------------------------------- baked lighting --
            // This is a subtraction from authored baked light and is independent
            // of the edited runtime sun direction and intensity.
            let baked_sun_ready = authored.is_some();
            theme::section(
                ui,
                "BAKED LIGHTING",
                "Approximate removal of the map's baked direct sun. This control never adds a replacement light.",
            );
            if authored.is_none() {
                theme::banner(
                    ui,
                    "Inactive: this map has no authored sun to remove from its baked lighting.",
                    theme::WARNING,
                );
            } else {
                theme::banner(
                    ui,
                    "When compiler settings are available, removal projects the authored sun into the lightmap and subtracts before exposure and filtering. Other maps use an estimated fallback.",
                    theme::TEXT_DIM,
                );
                if let Some((yaw, pitch, intensity, color)) = authored {
                    theme::banner(
                        ui,
                        &format!(
                            "MAP SUN  azimuth {yaw:.1}°  elevation {pitch:.1}°  intensity {intensity:.0}  RGB {:.2} {:.2} {:.2}  | live beam uses this direction and hue",
                            color[0], color[1], color[2]
                        ),
                        theme::TEXT_FAINT,
                    );
                }
            }
            if authored.is_some() && !self.video.baked_sun_shadow_map {
                theme::banner(
                    ui,
                    "Legacy fallback ignores occlusion and can subtract inside baked shadows on uncalibrated surfaces. Choose Shadow map for occlusion in the fallback.",
                    theme::WARNING,
                );
            }
            ui.add_enabled_ui(baked_sun_ready, |ui| {
                if self.baked_slider(
                    ui,
                    "Subtract baked sunlight",
                    "0 leaves baked lighting unchanged. 1 subtracts the full direct-sun estimate. Known compiler settings use the authored sun strength, color, direction and shadow sampling in the original lightmap UVs. Baked edge samples refine the projected soft shadow. Other surfaces use a measured fit or the conservative fallback below. This never adds the runtime sun; its intensity and CSM setting do not affect removal.",
                    theme::Reset::Environment(ui::ENV_ROW_BAKED_SUN_REMOVAL),
                    0.0..=1.0,
                    defaults.baked_sun_removal,
                    |v| &mut v.baked_sun_removal,
                ) {
                    self.environment_selected = ui::ENV_ROW_BAKED_SUN_REMOVAL;
                    self.touch_baked_sun_debug();
                }
            });
            theme::section(ui, "REMAINING BAKED LIGHT", "Adjust residual baked illumination; these controls do not change texture colors or output gamma.");
            self.baked_slider(
                ui,
                "Baked indirect brightness",
                "Brightness multiplier for residual baked illumination only, separate from gamma and runtime sun. Default 1.0. Does not change gamma.",
                theme::Reset::None,
                0.0..=2.0,
                defaults.baked_indirect_scale,
                |v| &mut v.baked_indirect_scale,
            );
            self.baked_slider(
                ui,
                "Baked indirect saturation",
                "Saturation multiplier for residual baked illumination only, not textures or dynamic sunlight. Default 1.0. Does not change gamma.",
                theme::Reset::None,
                0.0..=2.0,
                defaults.baked_indirect_saturation,
                |v| &mut v.baked_indirect_saturation,
            );
            self.baked_slider(
                ui,
                "Baked indirect power",
                "Nonlinear night-mode darkening of remaining baked light: 1.0 unchanged, 2.0 squared, 3.0 cubed. Preserves RGB hue by shaping linear luminance, not individual channels. Try 2.5 with brightness 1.0.",
                theme::Reset::None,
                1.0..=4.0,
                defaults.baked_indirect_power,
                |v| &mut v.baked_indirect_power,
            );
            self.baked_slider(
                ui,
                "Baked highlight preservation",
                "0 darkens all baked light with the power curve. 1 preserves high-intensity baked light pools (including likely lamps). This is luminance-based, not actual spotlight separation.",
                theme::Reset::None,
                0.0..=1.0,
                defaults.baked_indirect_highlight_preserve,
                |v| &mut v.baked_indirect_highlight_preserve,
            );

            ui.collapsing("Advanced sun removal / diagnostics", |ui| {
                const BAKED_PREVIEW: [(u8, &str); 3] = [
                    (0, "Auto on drag"),
                    (1, "Removed amount"),
                    (2, "Sun visibility"),
                ];
                ui.add_enabled_ui(baked_sun_ready, |ui| {
                    if let Some(target) = segmented_row(
                        ui,
                        "Live preview",
                        "Auto shows the removal heatmap while you drag the subtraction slider. Removed amount: black means none, bright red means more light removed. Sun visibility: green means the authored sun reaches the surface, red means blocked, blue means the map is not ready or its projection failed, amber means Legacy mode has no occlusion data. The colored beam shows the authored map-sun direction and hue.",
                        theme::Reset::None,
                        self.baked_sun_debug_view,
                        &BAKED_PREVIEW,
                    ) {
                        self.baked_sun_debug_view = target;
                        self.sun_ray_until = None;
                        self.sync_sun_ray();
                    }
                });
                const BAKED_VISIBILITY: [(bool, &str); 2] = [(false, "Legacy"), (true, "Shadow map")];
                if let Some(target) = segmented_row(
                    ui,
                    "Old sun visibility",
                    "Controls fallback visibility on surfaces without a measured fit. Legacy uses surface facing only. Shadow map uses a cached BSP shadow map and sky-admission map from the authored q3map_sun direction. Paired fits follow the original lightmap edges. This does not require Dynamic shadows (CSM).",
                    theme::Reset::None,
                    self.video.baked_sun_shadow_map,
                    &BAKED_VISIBILITY,
                ) {
                    self.video.baked_sun_shadow_map = target;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.touch_baked_sun_debug();
                }
                if self.baked_slider(
                    ui,
                    "Old shadow edge softness",
                    "Fallback shadow-map filter radius in texels. Larger values soften fallback visibility but may blur detail. Paired surface fits retain the original lightmap edges. Default 2.5. Does not change gamma.",
                    theme::Reset::None,
                    0.0..=24.0,
                    defaults.baked_sun_edge_softness,
                    |v| &mut v.baked_sun_edge_softness,
                ) {
                    self.touch_baked_sun_debug();
                }
                if self.baked_slider(
                    ui,
                    "Fallback baked-light fraction",
                    "Safety cap for surfaces without a reliable sunlit/shadowed fit. 0.48 limits the fallback estimate to 48% of a lightmap sample. Paired surfaces use their measured shadow baseline and direct-light bound, limited to available RGB light. Default 0.48.",
                    theme::Reset::None,
                    0.0..=0.9,
                    defaults.baked_sun_direct_limit,
                    |v| &mut v.baked_sun_direct_limit,
                ) {
                    self.touch_baked_sun_debug();
                }
            });
            theme::section(ui, "SKY ILLUMINATION", "Blue Water ambient and sun/moon illumination follows the positions and atmosphere configured on Sky.");
            ui.add_enabled_ui(self.video.sky_backend == 1, |ui| {
                let mut value = self.video.blue_water_environment.world_lighting;
                theme::row(ui, "Sky world illumination", "Blend Blue Water sky ambient and sun/moon lighting over the baked map. 0 preserves the original contribution. This scales baked light and does not reconstruct indoor bounced light.", theme::Reset::None, |ui| {
                    let default = defaults.blue_water_environment.world_lighting;
                    let readout = format!("{value:.2}");
                    if theme::slider_default(ui, &mut value, 0.0..=1.0, &readout, default) {
                        self.video.blue_water_environment.world_lighting = value;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                    }
                });
            });
            theme::section(ui, "ENTITY FIXTURE EXPERIMENT", "Compare existing model lighting with a restrained per-pixel accent from authored map fixtures. Original world lightmaps stay intact; world accents below are independent.");
            if let Some(target) = segmented_row(ui, "Fixture lighting on models", "Session-only. Requires Relight map and Cascaded Shadow Maps. B preserves baked entity lighting and adds individually shadowed fixture accents on players and lit models. Hardware RT retains its existing model receiver. Wait for pipeline compilation before comparing.", theme::Reset::None,
                self.video.entity_fixture_lighting_ab, &[(false,"A: Existing"),(true,"B: Fixtures")]) {
                let _ = self.set_console_cvar("r_entityFixtureLightingAB", if target {"1"} else {"0"});
            }
            if self.video.entity_fixture_lighting_ab && !self.video.dynamic_shadows.is_csm() {
                ui.small("Select Cascaded Shadow Maps in Lighting to activate B.");
            }
            if self.video.entity_fixture_lighting_ab && self.video.dynamic_lights == crate::ui::DynamicLightsMode::RayTracedHardware {
                ui.small("Hardware ray tracing keeps its existing model lighting; this experiment applies to raster lighting.");
            }
            theme::section(ui, "ADDED MAP LIGHTS", "Each source switch independently controls runtime map illumination and its optional lightmap reconstruction. Lamp-shadow source selections in Lighting never add illumination by themselves.");
            self.egui_toggle_row(
                ui,
                "Shader source: glow stages",
                "Promote surfaces with the authored JKA shader-stage `glow` marker. Texture colour/energy and actual BSP surface geometry determine the emitted light.",
                ui::VIDEO_ROW_PROMOTE_SHADER_GLOW,
                self.video.promote_shader_glow,
            );
            self.egui_toggle_row(
                ui,
                "Shader source: additive stages",
                "Promote visible additive emission stages (GL_ONE GL_ONE or SRC_ALPHA ONE). This is broader than `glow` and is useful for finding legacy emissive materials that were never tagged for Dynamic Glow.",
                ui::VIDEO_ROW_PROMOTE_SHADER_ADDITIVE,
                self.video.promote_shader_additive,
            );
            self.egui_toggle_row(
                ui,
                "Shader source: emissive maps",
                "Promote modern _emissive/_emission/_emit/_glow companion textures. Mostly-black masks are energy/coverage weighted so the entire surface does not behave like a full-white emitter.",
                ui::VIDEO_ROW_PROMOTE_SHADER_EMISSIVE,
                self.video.promote_shader_emissive,
            );

            self.egui_toggle_row(
                ui,
                "Authored surface / area lights",
                "Runtime area emitters from mapper-authored q3map_surfacelight metadata. Shader-driven glow/additive/emissive promotion is controlled separately above, so the same surface is never emitted twice.",
                ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS,
                self.video.emissive_area_lights,
            );

        });
        if self.sun_ray_until.is_some()
            && ui.ctx().dragged_id().is_some()
            && self.environment_selected == ui::ENV_ROW_BAKED_SUN_REMOVAL
        {
            self.touch_baked_sun_debug();
        }
    }

    fn reset_relighting_page(&mut self) {
        let d = VideoSettings::default();
        self.video.entity_sun_lighting = d.entity_sun_lighting;
        self.video.baked_sun_shadow_map = d.baked_sun_shadow_map;
        self.video.baked_sun_removal = d.baked_sun_removal;
        self.video.baked_sun_edge_softness = d.baked_sun_edge_softness;
        self.video.baked_sun_direct_limit = d.baked_sun_direct_limit;
        self.video.baked_indirect_scale = d.baked_indirect_scale;
        self.video.baked_indirect_saturation = d.baked_indirect_saturation;
        self.video.baked_indirect_power = d.baked_indirect_power;
        self.video.baked_indirect_highlight_preserve = d.baked_indirect_highlight_preserve;
        self.video.sun_lighting = d.sun_lighting;
        self.video.static_world_csm = d.static_world_csm;
        self.video.promote_shader_glow = d.promote_shader_glow;
        self.video.promote_shader_additive = d.promote_shader_additive;
        self.video.promote_shader_emissive = d.promote_shader_emissive;
        self.video.emissive_area_lights = d.emissive_area_lights;
        self.video.entity_fixture_lighting_ab = false;
        self.render_command(crate::renderer::RenderCommand::SetEntityFixtureLightingAB(false));
        self.video.blue_water_environment.world_lighting = d.blue_water_environment.world_lighting;
        self.baked_sun_debug_view = 0;
        self.sun_ray_until = None;
        self.set_sun_override(false, false);
    }
}
