//! Material shading and PBR compatibility profile.
use crate::app::egui_settings::{theme, ui, App};

impl App {
    pub(in crate::app::egui_settings) fn egui_material_rendering(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "MATERIAL SHADING", "PBR material evaluation, generated normals and directional baked-material response.");
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

    }
}
