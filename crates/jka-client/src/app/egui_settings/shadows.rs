//! Shadows.
use crate::app::egui_settings::{
    index_of, mode_index, mode_row, segmented_row, theme, ui, App, DynamicLightsMode,
    DynamicShadowsMode, EntityShadowLight,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_shadows(&mut self, ui: &mut egui::Ui) {
        const SHADOW_OPTIONS: [(DynamicShadowsMode, &str, &str); 5] = [
            (
                DynamicShadowsMode::Off,
                "Off",
                "No dynamic shadows cast by players, NPCs or moveable objects.",
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
            (
                DynamicShadowsMode::RayTraced,
                "Ray traced",
                "Hardware ray-query sun visibility using the current wgpu BLAS/TLAS API. This first port traces opaque static BSP casters; alpha-tested/translucent surfaces and moving/skinned casters are deliberately excluded rather than approximated.",
            ),
        ];
        if let Some(target) = mode_row(
            ui,
            "Dynamic shadows",
            "Real-time shadow technique layered over the map's authored/baked lighting. Hardware ray tracing currently covers opaque static BSP sun occlusion.",
            theme::Reset::Video(ui::VIDEO_ROW_DYNAMIC_SHADOWS),
            self.video.dynamic_shadows,
            &SHADOW_OPTIONS,
        ) {
            let current = mode_index(&SHADOW_OPTIONS, self.video.dynamic_shadows);
            self.video_selected = ui::VIDEO_ROW_DYNAMIC_SHADOWS;
            self.change_video_setting(target as i32 - current as i32);
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
        // Only Forward+ consults this toggle. Ray-traced lights always trace
        // local visibility, and the lower tiers never shadow local lights.
        let local_shadows_applies =
            self.video.dynamic_lights == DynamicLightsMode::PerPixelForwardPlus;
        let local_shadows_note = match self.video.dynamic_lights {
            DynamicLightsMode::PerPixelForwardPlus => "",
            DynamicLightsMode::RayTracedHardware => {
                "

Not used: Ray traced dynamic lights always trace local-light visibility."
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
                    "Adds shadows to local lights. With RT Shadows, uses hard ray-traced shadows for clustered lights, including moving FX lights. Other shadow modes use cached shadow cubemaps. Hardware RT dynamic lighting always includes local visibility, independently of this toggle.{local_shadows_note}"
                ),
                ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS,
                self.video.local_light_shadows,
            );
        });
        self.egui_toggle_row(
            ui,
            "Contact shadows",
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
        let rt_sun = self.video.dynamic_shadows == DynamicShadowsMode::RayTraced;
        let rt_lights = self.video.dynamic_lights == DynamicLightsMode::RayTracedHardware;
        const RT_RESOLUTION: [(bool, &str, &str); 2] = [
            (true, "Reduced", "Traces each pixel's sun shadow once every four frames and carries the rest forward by motion vectors. Shadow edges and newly revealed pixels still trace every frame."),
            (false, "Full", "Trace sun visibility at every shaded pixel, every frame. Reference quality; highest ray cost."),
        ];
        // The reduced schedule only exists for the sun; local lights are always traced directly.
        ui.add_enabled_ui(rt_sun, |ui| {
            let note = if rt_sun { "" } else { "\n\nNot used: set Dynamic shadows to RT Shadows." };
            if let Some(index) = mode_row(ui, "RT sun rate", &format!("How often the ray-traced sun shadow is retraced. Runs at full screen resolution either way; this trades ray count against how quickly a moving shadow caster's interior updates. Needs a depth pass.{note}"),
                theme::Reset::None, self.video.rt_half_resolution, &RT_RESOLUTION) {
                let _ = self.set_console_cvar("r_rtResolution", if RT_RESOLUTION[index].0 { "half" } else { "full" });
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
        ui.add_enabled_ui(rt_sun || rt_lights, |ui| {
            let note = if rt_sun || rt_lights { "" } else { "\n\nNot used: needs RT Shadows or Ray traced dynamic lights." };
            if let Some(index) = mode_row(ui, "RT samples", &format!("Samples per soft sun or saber light. Point lights stay at one ray. TAA can further reduce noise.{note}"),
                theme::Reset::None, self.video.rt_samples, &RT_SAMPLES) {
                let _ = self.set_console_cvar("r_rtSamples", &RT_SAMPLES[index].0.to_string());
            }
        });
    }
}
