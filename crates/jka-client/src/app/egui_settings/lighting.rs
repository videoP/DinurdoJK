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

        self.egui_toggle_row(
            ui,
            "Physically based rendering (PBR)",
            "Master switch for the complete Rend2-style PBR material profile. \
             Disabling it turns off PBR companion-map shading, parallax occlusion, \
             PBR lightgrid/probe response, dynamic-light PBR BRDF shading and \
             emissive companion shading together.",
            ui::VIDEO_ROW_PBR,
            self.video.pbr,
        );
        self.egui_toggle_row(
            ui,
            "Allow asset overrides",
            "JKA/OpenJK VFS compatibility policy. When enabled, higher-priority addon PK3s \
             and loose files may replace assets from retail assets0.pk3 through assets3.pk3. \
             When disabled, ordinary lookups protect those retail qpaths. Active PBR .mtr \
             materials may still load their explicitly referenced base/normal/RMO/etc. \
             textures from the same package as the material. While a map is loaded, Apply Video Settings reloads it with the new setting.",
            ui::VIDEO_ROW_ASSET_OVERRIDES,
            self.video.allow_asset_overrides,
        );
        self.egui_toggle_row(
            ui,
            "Generate normal maps",
            "Rend2 compatibility fallback. Synthesize a normal map from diffuse luminance \
             only when no authored normal map exists; with a map loaded, Apply Video \
             Settings reloads it with the new setting. Useful for old assets, \
             but authored normals are better.",
            ui::VIDEO_ROW_GEN_NORMAL_MAPS,
            self.video.gen_normal_maps,
        );
        self.egui_toggle_row(
            ui,
            "Deluxe mapping",
            "Use q3map2 directional lightmaps for normal/specular response when \
             present. Maps without deluxemaps retain the existing BSP-lightgrid \
             directional fallback.",
            ui::VIDEO_ROW_DELUXE_MAPPING,
            self.video.deluxe_mapping,
        );
        theme::row(
            ui,
            "Deluxe specular",
            "Scales only the specular lobe produced by directional baked lighting. \
             0 disables baked-direction specular while keeping diffuse deluxe relighting.",
            theme::Reset::Video(ui::VIDEO_ROW_DELUXE_SPECULAR),
            |ui| {
                let mut value = self.video.deluxe_specular;
                let readout = format!("{value:.2}×");
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.video.deluxe_specular = value;
                    self.sync_pbr();
                    self.mark_config_dirty();
                }
            },
        );

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
                "Direct local lighting with hardware ray-traced shadows, independent of sun shadows. Falls back to Forward+ when RT is unavailable.",
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

        self.egui_toggle_row(
            ui,
            ".map light simulation",
            "Source .map preview only. Approximates a compiled light stage from authored light/lightJunior entities, including color, strength and q3map-style falloff. It uses the modern clustered renderer and leaves compiled BSP lighting unchanged.",
            ui::VIDEO_ROW_MAP_LIGHT_SIMULATION,
            self.video.map_light_simulation,
        );

        self.egui_toggle_row(
            ui,
            "Emissive / area lights",
            "Lets glowing surfaces act as light sources with a shape, instead of \
             only looking bright themselves.",
            ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS,
            self.video.emissive_area_lights,
        );

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
