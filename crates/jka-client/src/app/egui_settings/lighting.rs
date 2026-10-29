//! Lighting.
use crate::app::egui_settings::{
    mode_index, mode_row, quality_row, segmented_row, theme, ui, App, DynamicLightsMode,
    EntityAmbientLightingMode,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_lighting(&mut self, ui: &mut egui::Ui) {
        const AMBIENT_OPTIONS: [(EntityAmbientLightingMode, &str, &str); 3] = [
            (
                EntityAmbientLightingMode::Off,
                "Off",
                "Dynamic objects receive a uniform, constant ambient color with no environmental lighting.",
            ),
            (
                EntityAmbientLightingMode::BspLightgridClassic,
                "BSP lightgrid",
                "OpenJK-style entity lighting from the map's BSP lightgrid: trilinear ambient + directed light and direction, sampled at the entity origin.",
            ),
            (
                EntityAmbientLightingMode::BevyIrradianceVolume,
                "Irradiance volume",
                "Bevy irradiance-volume runtime over the BSP lightgrid: bounded LightProbe volume, Valve ambient cubes, hardware trilinear interpolation, N² weighting and diffuse-indirect PBR integration.",
            ),
        ];
        if let Some(target) = mode_row(
            ui,
            "Entity ambient lighting",
            "How players, NPCs and moveable objects pick up the room's ambient \
             light as they move through the map.",
            theme::Reset::Video(ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING),
            self.video.entity_ambient_lighting,
            &AMBIENT_OPTIONS,
        ) {
            let current = mode_index(&AMBIENT_OPTIONS, self.video.entity_ambient_lighting);
            self.video_selected = ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING;
            self.change_video_setting(target as i32 - current as i32);
        }

        // Classic world lighting is one three-state control. `Off` maps to
        // r_fullbright 1; once enabled, the meter chooses between the cheap
        // r_vertexLight path and normal authored BSP lightmaps.
        let world_lighting = if !self.video.world_lighting {
            0
        } else if self.video.vertex_lighting {
            1
        } else {
            2
        };
        const WORLD_LIGHTING: [&str; 3] = ["Fullbright", "Vertex light", "BSP lightmaps"];
        if let Some(target) = quality_row(
            ui,
            "World lighting",
            "Master for classic BSP world lighting. Fullbright is vanilla r_fullbright 1: every surface is drawn at full brightness with no lighting, so the map looks flatter and brighter, not darker. \
             When enabled, Vertex light uses BSP vertex colors (r_vertexLight 1); \
             BSP lightmaps uses the normal authored baked-lightmap path (r_vertexLight 0).",
            theme::Reset::Video(ui::VIDEO_ROW_WORLD_LIGHTING),
            world_lighting,
            &WORLD_LIGHTING,
        ) {
            self.video_selected = ui::VIDEO_ROW_WORLD_LIGHTING;
            self.set_world_lighting_quality(target);
        }

        const LIGHT_OPTIONS: [(DynamicLightsMode, &str, &str); 6] = [
            (
                DynamicLightsMode::Off,
                "Off",
                "Runtime-authored dynamic lights are disabled.",
            ),
            (
                DynamicLightsMode::Vertex,
                "Vertex",
                "Cheapest authored dynamic-light path: evaluate runtime lights at BSP vertices and interpolate them. This is separate from static r_vertexLight.",
            ),
            (
                DynamicLightsMode::Legacy,
                "Legacy",
                "Low-cost JKA-style authored dynamic lights with a smooth radial falloff and no shadow pass.",
            ),
            (
                DynamicLightsMode::ClusteredLite,
                "Clustered lite",
                "Transient authored lights only through GPU cluster lists, using the cheap Lambert model and no local-light shadows.",
            ),
            (
                DynamicLightsMode::PerPixelForwardPlus,
                "Forward+",
                "Full clustered per-pixel local lighting, including the modern map-light path.",
            ),
            (
                DynamicLightsMode::RayTracedHardware,
                "Ray traced",
                "Ray-traced shadows for transient lights such as sabers, blasters and explosions. Static map lights use raster lighting and optional cached shadow cubemaps. Sun shadows are separate. Falls back to Forward+ when RT is unavailable.",
            ),
        ];
        if let Some(target) = mode_row(
            ui,
            "Dynamic lights",
            "Technique used for light emitted by moving things: blaster bolts, \
             sabers, explosions and flickering map lights.",
            theme::Reset::Video(ui::VIDEO_ROW_DYNAMIC_LIGHTS),
            self.video.dynamic_lights,
            &LIGHT_OPTIONS,
        ) {
            let current = mode_index(&LIGHT_OPTIONS, self.video.dynamic_lights);
            self.video_selected = ui::VIDEO_ROW_DYNAMIC_LIGHTS;
            self.change_video_setting(target as i32 - current as i32);
        }

        // A source-.map editor preview, not a BSP relighting feature. This
        // remains available even when "Relight map" is disabled.
        theme::section(ui, "SOURCE .MAP PREVIEW", "Lighting entities from an editable .map file; never adds lamps to compiled BSP maps.");
        self.egui_toggle_row(
            ui,
            ".map light simulation",
            "When viewing a source .map, approximate authored light and lightJunior entities with clustered lighting. Ignored on compiled BSP worlds; independent of Relight map.",
            ui::VIDEO_ROW_MAP_LIGHT_SIMULATION,
            self.video.map_light_simulation,
        );

        theme::section(ui, "SUN & ENTITY SHADOWS", "Cascaded Shadow Maps is the highest setting for entity sun shadows. Static BSP caster shadows can optionally be enabled in Relighting.");
        self.egui_shadows(ui);

        self.egui_toggle_row(
            ui,
            "Dynamic light debug view",
            "Diagnostic BSP receiver view for active direct-local lights. World surfaces become neutral dark gray and normal BSP textures, baked lightmaps/static vertex light, sun, reflections, GI, emissive shading and in-surface weather/fog are bypassed so RGB/intensity footprints are easy to inspect. Runtime FX, enabled map point lights, authored q3map_surfacelight emitters and shader-promoted emitters are shown according to their normal source toggles. Session-only; does not alter light data.",
            ui::VIDEO_ROW_DYNAMIC_LIGHT_DEBUG,
            self.video.dynamic_light_debug,
        );

        theme::section(ui, "LAMP SHADOWS", "No master switch: inferred moving-object shadows follow the individual lamp-source toggles. Only CSM uses them; original map lightmaps remain intact.");
        let lamp_shadow_mode = self.video.dynamic_shadows.is_csm();
        ui.add_enabled_ui(lamp_shadow_mode, |ui| {
            self.egui_toggle_row(ui, "Source: glow stages", "Use static shader glow emitters as shadow sources.", ui::VIDEO_ROW_BAKED_LOCAL_GLOW, self.video.baked_local_sources & 1 != 0);
            self.egui_toggle_row(ui, "Source: additive stages", "Infer lamps from additive stages; broad inference, disabled by default.", ui::VIDEO_ROW_BAKED_LOCAL_ADDITIVE, self.video.baked_local_sources & 2 != 0);
            self.egui_toggle_row(ui, "Source: emissive maps", "Use emissive companion textures as shadow sources.", ui::VIDEO_ROW_BAKED_LOCAL_EMISSIVE, self.video.baked_local_sources & 4 != 0);
            self.egui_toggle_row(ui, "Source: authored surfacelight", "Use authored q3map_surfacelight lamps as shadow sources; this switch alone adds no light.", ui::VIDEO_ROW_BAKED_LOCAL_SURFACELIGHT, self.video.baked_local_sources & 8 != 0);
            self.egui_toggle_row(ui, "Source: fixed continuous EFX", "Use fixed continuously active EFX lamps. Moving, triggered and intermittent effects are excluded.", ui::VIDEO_ROW_BAKED_LOCAL_FX, self.video.baked_local_sources & 16 != 0);
        });
        if !lamp_shadow_mode {
            ui.small("Select Cascaded Shadow Maps to use these lamp shadows.");
        }
        const LAMP_SHADOW_AB: [(u8, &str, &str); 5] = [
            (0, "A: Cubes", "Original six-face cubemap generation and sampling (reference)."),
            (1, "B: Receiver", "Conservative per-octant caster test before sampling dynamic shadow cubemaps."),
            (2, "C: Casters", "Pre-filter models and BSP movers against each lamp before testing the six faces."),
            (3, "B+C", "Apply both experimental optimizations; shadows still update at frame rate."),
            (4, "Receiver early-out", "B plus a conservative per-pixel precheck: skip all inferred-lamp weight/PCF evaluation if no selected moving caster can shadow that pixel."),
        ];
        ui.add_enabled_ui(lamp_shadow_mode && self.video.baked_local_sources != 0, |ui| {
            if let Some(target) = mode_row(ui, "Lamp shadow A/B experiment",
                "Session-only. GPU timings/perfcapture; no update-rate cap. Octahedral shadow maps are NOT implemented by these variants.",
                theme::Reset::None, self.video.lamp_shadow_ab, &LAMP_SHADOW_AB) {
                let _ = self.set_console_cvar("r_lampShadowAB", &LAMP_SHADOW_AB[target].0.to_string());
            }
        });
        const FX_OPTIONS: [(u8, &str, &str); 3] = [
            (0, "Off", "Authored FX lights and faithful missile/entity lights still work."),
            (1, "Conservative", "Existing EFX semantic/material classifier; no additional live projectile lights."),
            (2, "Aggressive", "Broader EFX promotion, live luminous projectile lights, and short-lived light retention."),
        ];
        if let Some(target) = mode_row(
            ui,
            "Promote emissive FX to lights",
            "0 off / 1 conservative / 2 aggressive. Preserves authored Light{} stages and uses existing point, spot and segment lights.",
            theme::Reset::Video(ui::VIDEO_ROW_PROMOTE_FX_LIGHTS),
            self.video.promote_fx_lights,
            &FX_OPTIONS,
        ) {
            let current = mode_index(&FX_OPTIONS, self.video.promote_fx_lights);
            self.video_selected = ui::VIDEO_ROW_PROMOTE_FX_LIGHTS;
            self.change_video_setting(target as i32 - current as i32);
        }

        let ao_mode = if self.video.ssao {
            1usize
        } else if self.video.static_bsp_ao {
            2
        } else {
            0
        };
        const AO_OPTIONS: [(usize, &str); 3] = [(0, "Off"), (1, "Realtime"), (2, "Baked")];
        if let Some(target) = segmented_row(
            ui,
            "Ambient occlusion",
            "Contact darkening where surfaces meet. Realtime computes it every \
             frame from depth; Baked precomputes it per map and costs nothing \
             at runtime but misses moving objects.",
            theme::Reset::Video(ui::VIDEO_ROW_AMBIENT_OCCLUSION),
            ao_mode,
            &AO_OPTIONS,
        ) {
            self.video_selected = ui::VIDEO_ROW_AMBIENT_OCCLUSION;
            self.change_video_setting(target as i32 - ao_mode as i32);
        }
        if ao_mode == 2 {
            self.egui_baked_ao(ui);
        }
        self.egui_toggle_row(
            ui,
            "Voxel / probe GI",
            "Bounced indirect light gathered into a voxel or probe volume, so \
             lit surfaces spill colour onto their surroundings. Enabling it on a \
             map prepared without GI needs Apply Video Settings, which reloads the map.",
            ui::VIDEO_ROW_VOXEL_PROBE_GI,
            self.video.voxel_probe_gi,
        );
    }
}
