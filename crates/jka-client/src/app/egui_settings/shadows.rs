//! Shadows.
use crate::app::egui_settings::{
    index_of, mode_index, mode_row, segmented_row, theme, ui, App, DynamicLightsMode,
    DynamicShadowsMode, EntityShadowLight,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_shadows(&mut self, ui: &mut egui::Ui) {
        const SHADOW_OPTIONS: [(DynamicShadowsMode, &str, &str); 4] = [
            (
                DynamicShadowsMode::Off,
                "Off",
                "No directional shadow maps. Player foot-contact blobs are controlled separately.",
            ),
            (
                DynamicShadowsMode::Blob,
                "Blob",
                "OpenJK cg_shadows 1 drop shadow: the stock markShadow decal projected onto the floor beneath players and NPCs.",
            ),
            (
                DynamicShadowsMode::EntityMap,
                "Entity map",
                "One small shadow map that only draws players, NPCs and models; the map is not re-rendered, so it costs a few depth-only draws. It is aimed by the baked lightgrid's dominant light direction at the camera (or the runtime sun), and fades out where the baked light is mostly ambient.",
            ),
            (
                DynamicShadowsMode::CascadedShadowMaps,
                "Cascaded maps (CSM)",
                "Bevy 0.19.1 directional-light CSM: four exponential cascades, stable texel snapping, reverse-Z and Bevy shadow filtering.",
            ),
        ];
        if let Some(target) = mode_row(
            ui,
            "Sun & entity shadow method",
            "Shadowing for sunlight and legacy entity drop-shadows. These are shadow techniques, not additional static light sources.",
            theme::Reset::Video(ui::VIDEO_ROW_DYNAMIC_SHADOWS),
            self.video.dynamic_shadows,
            &SHADOW_OPTIONS,
        ) {
            let current = mode_index(&SHADOW_OPTIONS, self.video.dynamic_shadows);
            self.video_selected = ui::VIDEO_ROW_DYNAMIC_SHADOWS;
            self.change_video_setting(target as i32 - current as i32);
        }
        let mut contact = self.video.player_contact_shadows;
        if ui.checkbox(&mut contact, "Player foot-contact blobs").on_hover_text(
            "Subtle ground-contact darkening under players and NPCs, independent of CSM, RT and lamp shadows. Reuses the projected blob system, follows the floor, and fades with height. Classic Blob mode retains its full-size drop shadow."
        ).changed() {
            let _ = self.set_console_cvar("r_playerContactShadows", if contact { "1" } else { "0" });
        }
        ui.add_enabled_ui(self.video.dynamic_shadows == DynamicShadowsMode::EntityMap, |ui| {
            const ENTITY_SHADOW_LIGHT_OPTIONS: [(EntityShadowLight, &str); 2] = [
                (EntityShadowLight::Lightgrid, "Lightgrid"),
                (EntityShadowLight::Authored, "Authored light"),
            ];
            if let Some(target) = segmented_row(
                ui,
                "Entity shadow light",
                "Where the Entity map aims its light. Lightgrid uses the baked dominant direction \
                 at the player: cheap and always available, but it is an average of every lamp in \
                 range, so several lamps give a compromise direction. Authored light picks the \
                 strongest map light that agrees with the lightgrid and shadows from its real \
                 position. Both cast a single shadow. Only used by Dynamic shadows = Entity map.",
                theme::Reset::Video(ui::VIDEO_ROW_ENTITY_SHADOW_LIGHT),
                self.video.entity_shadow_light,
                &ENTITY_SHADOW_LIGHT_OPTIONS,
            ) {
                let current = index_of(&ENTITY_SHADOW_LIGHT_OPTIONS, self.video.entity_shadow_light, 0);
                let next = index_of(&ENTITY_SHADOW_LIGHT_OPTIONS, target, current);
                self.video_selected = ui::VIDEO_ROW_ENTITY_SHADOW_LIGHT;
                self.change_video_setting(next as i32 - current as i32);
            }
        });
        // RT always shadows transient lights; this toggle still controls the
        // cached raster shadows of static map lights in that mode.
        let local_shadows_applies = matches!(self.video.dynamic_lights,
            DynamicLightsMode::PerPixelForwardPlus | DynamicLightsMode::RayTracedHardware);
        let local_shadows_note = match self.video.dynamic_lights {
            DynamicLightsMode::PerPixelForwardPlus => "",
            DynamicLightsMode::RayTracedHardware => {
                "

In Ray traced mode, this controls static map-light cubemap shadows. Transient lights always use RT shadows."
            }
            _ => {
                "

Not used: this dynamic-lights mode never shadows local lights. Choose Forward+ to use it."
            }
        };
        ui.add_enabled_ui(local_shadows_applies, |ui| {
            self.egui_toggle_row(
                ui,
                "Local light shadows",
                &format!(
                    "Enables cached cubemap shadows for static map lights. Transient lights use RT visibility when hardware RT is active; Ray traced dynamic lights always include their transient shadows.{local_shadows_note}"
                ),
                ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS,
                self.video.local_light_shadows,
            );
        });
        self.egui_toggle_row(
            ui,
            "Sun screen-space contact shadows",
            "Marches a short ray from each pixel toward the map's sun through the \
             depth buffer and darkens pixels where a nearby on-screen surface blocks \
             it, filling in the fine darkening where objects meet the ground. \
             Screen-space: it only sees what is on screen, so occluders past the \
             screen edge cast nothing. Needs a map with an authored sun.",
            ui::VIDEO_ROW_CONTACT_SHADOWS,
            self.video.contact_shadows,
        );

        // Grey out (rather than hide) the RT tuning rows when nothing ray-traced
        // is active, matching the other dependent rows on this page.
        let rt_lights = self.video.dynamic_lights == DynamicLightsMode::RayTracedHardware;
        const RT_TRACE_RATE: [(u32, &str, &str); 3] = [
            (1, "Full", "Fresh dynamic-light shadow rays every frame."),
            (2, "Half", "Solari importance resampling: trace one local-light sample per two candidates. More noise."),
            (4, "Quarter", "Solari importance resampling: trace one local-light sample per four candidates. More noise."),
        ];
        ui.add_enabled_ui(rt_lights, |ui| {
            let note = if rt_lights { "" } else { "\n\nNeeds ray-traced dynamic lights." };
            if let Some(index) = mode_row(ui, "RT ray budget", &format!("Solari weighted light sampling reduces shadow rays at the material receiver, without a visibility-history pass. Full traces every candidate; lower budgets add sampling noise. Dynamic lights have a one-sample floor, so a lone point light cannot become cheaper. RT samples sets the starting budget; temporal filtering can smooth noise. FPS gains depend on the scene.{note}"),
                theme::Reset::None, self.video.rt_trace_rate, &RT_TRACE_RATE) {
                let _ = self.set_console_cvar("r_rtTraceRate", &RT_TRACE_RATE[index].0.to_string());
            }
        });
        const RT_SAMPLES: [(u32, &str, &str); 3] = [
            (1, "1", "Current ray budget. Fastest; more visible grain."),
            (
                2,
                "2",
                "Two samples per soft source. Less grain, higher GPU cost.",
            ),
            (
                4,
                "4",
                "Four samples per soft source. Highest quality and GPU cost.",
            ),
        ];

        const RT_TAA: [(u32, &str, &str); 3] = [
            (0, "Off", "No regional temporal resolve."),
            (1, "On", "Temporal filtering inside runtime-light influence volumes. Retains MSAA; no extra shadow rays. Also filters textures and FX inside those regions."),
            (2, "Mask", "Green marks conservative RT light influence volumes; black stays unfiltered. Includes surfaces behind shadow blockers."),
        ];
        let rt_taa_note = if self.video.taa {
            "\n\nInactive while global TAA is enabled: global TAA already filters the scene."
        } else if !rt_lights {
            "\n\nNeeds ray-traced local lights."
        } else {
            ""
        };
        ui.add_enabled_ui(rt_lights && !self.video.taa, |ui| {
            if let Some(index) = mode_row(ui, "RT regional TAA",
                &format!("Adds temporal filtering to the final scene color inside RT light volumes. Retains MSAA; enabling disables FXAA and SMAA. Uses full-screen history and depth/motion data, so savings depend on scene coverage.{rt_taa_note}"),
                theme::Reset::None, self.video.rt_taa, &RT_TAA) {
                let _ = self.set_console_cvar("r_rtTaa", &RT_TAA[index].0.to_string());
            }
        });

        ui.add_enabled_ui(rt_lights, |ui| {
            let note = if rt_lights { "" } else { "\n\nNot used: needs ray-traced dynamic lights." };
            if let Some(index) = mode_row(ui, "RT samples", &format!("Samples per soft dynamic light. Point lights stay at one ray. TAA can further reduce noise.{note}"),
                theme::Reset::None, self.video.rt_samples, &RT_SAMPLES) {
                let _ = self.set_console_cvar("r_rtSamples", &RT_SAMPLES[index].0.to_string());
            }
        });
    }
}
