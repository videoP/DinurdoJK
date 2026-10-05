//! Clouds.
use crate::app::egui_settings::{
    index_of, percent, quality_table_row, segmented_row, theme, ui, App, CloudRenderResolution,
    CloudType,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_clouds(&mut self, ui: &mut egui::Ui) {
        self.egui_environment_toggle(
            ui,
            "Volumetric clouds",
            "Replaces the flat skybox clouds with a raymarched cloud deck that \
             has real depth, lighting and motion.",
            ui::ENV_ROW_CLOUDS,
            self.video.clouds,
        );

        ui.add_enabled_ui(self.video.clouds, |ui| {
            const TYPES: [(CloudType, &str); 3] = [
                (CloudType::Cumulus, "Cumulus"),
                (CloudType::Stratus, "Stratus"),
                (CloudType::Storm, "Storm"),
            ];
            if let Some(target) = segmented_row(
                ui,
                "Cloud type",
                "Shape preset for the deck: puffy cumulus, flat stratus sheet, or a \
             tall dark storm front.",
                theme::Reset::Environment(ui::ENV_ROW_CLOUD_TYPE),
                self.video.cloud_type,
                &TYPES,
            ) {
                let current = index_of(&TYPES, self.video.cloud_type, 0);
                let next = index_of(&TYPES, target, current);
                self.environment_selected = ui::ENV_ROW_CLOUD_TYPE;
                self.change_environment_setting(next as i32 - current as i32);
            }

            theme::row(
                ui,
                "Raymarch quality",
                "Steps taken through the cloud volume. The single biggest cost in \
             this section: lower it first if clouds are expensive.",
                theme::Reset::Environment(ui::ENV_ROW_CLOUD_QUALITY),
                |ui| {
                    let mut value = self.video.cloud_quality;
                    let readout = percent(value);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        self.set_cloud_quality(value);
                    }
                },
            );
            theme::row(
                ui,
                "Coverage",
                "Fraction of the sky the deck fills, from scattered wisps to solid \
             overcast.",
                theme::Reset::Environment(ui::ENV_ROW_CLOUD_COVERAGE),
                |ui| {
                    let mut value = self.video.cloud_coverage;
                    let readout = percent(value);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        self.set_cloud_coverage(value);
                    }
                },
            );
            theme::row(
                ui,
                "Base height",
                "World height of the bottom of the deck, in map units.",
                theme::Reset::Environment(ui::ENV_ROW_CLOUD_HEIGHT),
                |ui| {
                    let mut value = self.video.cloud_height;
                    let readout = format!("{value:.0} u");
                    if theme::slider(
                        ui,
                        &mut value,
                        ui::CLOUD_HEIGHT_MIN..=ui::CLOUD_HEIGHT_MAX,
                        &readout,
                    ) {
                        self.set_cloud_height(value);
                    }
                },
            );
            theme::row(
                ui,
                "Thickness",
                "How tall the deck is above its base. Thicker decks are darker \
             underneath and cost more to march.",
                theme::Reset::Environment(ui::ENV_ROW_CLOUD_THICKNESS),
                |ui| {
                    let mut value = self.video.cloud_thickness;
                    let readout = format!("{value:.0} u");
                    if theme::slider(
                        ui,
                        &mut value,
                        ui::CLOUD_THICKNESS_MIN..=ui::CLOUD_THICKNESS_MAX,
                        &readout,
                    ) {
                        self.set_cloud_thickness(value);
                    }
                },
            );

            theme::section(ui, "RENDER COST", "");
            self.egui_environment_toggle(
                ui,
                "Cloud shadows on map",
                "Projects the deck's shadow onto the world, so cloud cover sweeps \
             across the ground as it drifts.",
                ui::ENV_ROW_CLOUD_SHADOWS,
                self.video.cloud_shadows,
            );
            const CLOUD_RES: [(CloudRenderResolution, &str); 4] = [
                (CloudRenderResolution::Quarter, "25%"),
                (CloudRenderResolution::Half, "50%"),
                (CloudRenderResolution::ThreeQuarter, "75%"),
                (CloudRenderResolution::Full, "100%"),
            ];
            if let Some(target) = quality_table_row(
                ui,
                "Render resolution",
                "Fraction of the screen resolution the clouds are marched at before \
             being upscaled. 50% is roughly four times cheaper than full.",
                theme::Reset::Environment(ui::ENV_ROW_CLOUD_RENDER_RESOLUTION),
                self.video.cloud_render_resolution,
                &CLOUD_RES,
                1,
            ) {
                self.set_environment_quality_segment(ui::ENV_ROW_CLOUD_RENDER_RESOLUTION, target);
            }
            self.egui_environment_toggle(
                ui,
                "Temporal interleave",
                "Marches part of the cloud buffer each frame and reuses the rest. \
             Much cheaper, at the cost of some smearing when the camera whips \
             around.",
                ui::ENV_ROW_CLOUD_TEMPORAL,
                self.video.cloud_temporal,
            );

            theme::section(ui, "TUNING", "Fine control over the cloud model.");
            self.egui_cloud_tuning(ui);
        });
    }

    pub(in crate::app::egui_settings) fn egui_cloud_tuning(&mut self, ui: &mut egui::Ui) {
        theme::row(
            ui,
            "Temporal depth fix",
            "Rejects reprojected cloud samples whose depth no longer matches, \
             which removes ghosting behind moving geometry.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX),
            |ui| {
                if theme::switch(ui, self.video.cloud_temporal_depth_fix).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX, true, None);
                }
            },
        );
        theme::row(
            ui,
            "Wind shear",
            "How much faster the top of the deck moves than the bottom, which \
             tilts and stretches the clouds.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_SHEAR),
            |ui| {
                let mut value = self.video.cloud_shear;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_SHEAR, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Base height variation",
            "Raises and lowers the base across the deck so its underside is not \
             one flat plane.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_BASE_VARIATION),
            |ui| {
                let mut value = self.video.cloud_base_variation;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_BASE_VARIATION, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Cloud size",
            "Scales cloud masses in all directions. Their width already \
             tracks the thickness slider so their shape stays constant; \
             this is the multiplier on top.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_SIZE),
            |ui| {
                let mut value = self.video.cloud_size;
                // Readout is the world multiplier the shader applies, not the
                // raw slider position, so the number means something.
                // Log mapping, matching cloud_size_scale() in the shader.
                let readout = format!("{:.2}x", 0.5 * 8.0f32.powf(value));
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_SIZE, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Thickness variation",
            "How much the deck varies in depth. At zero every column fills the \
             whole thickness; higher leaves some shallow and some towering.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_THICKNESS_VARIATION),
            |ui| {
                let mut value = self.video.cloud_thickness_variation;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_THICKNESS_VARIATION, false, Some(value));
                }
            },
        );

        theme::section(
            ui,
            "DYNAMICS / A/B",
            "Independent experimental gates. All are off by default.",
        );
        theme::row(
            ui,
            "Shape evolution",
            "Lets the high-frequency erosion field drift through the macro weather \
             field so cloud edges and billows slowly reform.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_SHAPE_EVOLUTION),
            |ui| {
                if theme::switch(ui, self.video.cloud_shape_evolution).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_SHAPE_EVOLUTION, true, None);
                }
            },
        );
        theme::row(
            ui,
            "Terrain interaction",
            "Uses the shared map weather heightfield to lift low cloud over terrain \
             and large solid geometry. Has no effect when the deck is well above \
             the map.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_TERRAIN_INTERACTION),
            |ui| {
                if theme::switch(ui, self.video.cloud_terrain_interaction).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_TERRAIN_INTERACTION, true, None);
                }
            },
        );
        theme::row(
            ui,
            "Empty-space skip",
            "A/B optimization: takes larger primary-ray steps only when the macro \
             cloud field is safely below the coverage threshold, then falls back \
             to fine stepping near potential cloud.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_EMPTY_SKIP),
            |ui| {
                if theme::switch(ui, self.video.cloud_empty_skip).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_EMPTY_SKIP, true, None);
                }
            },
        );
        theme::row(
            ui,
            "Aerial perspective",
            "Blends distant clouds toward the atmosphere colour so the deck \
             reads as receding into haze.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_AERIAL),
            |ui| {
                let mut value = self.video.cloud_aerial;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_AERIAL, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Sky-tinted ambient",
            "Lights the shadowed side of the clouds with the sky colour instead \
             of flat grey.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_SKY_AMBIENT),
            |ui| {
                if theme::switch(ui, self.video.cloud_sky_ambient).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_SKY_AMBIENT, true, None);
                }
            },
        );
        theme::row(
            ui,
            "History blend",
            "How much of the previous frame each temporal sample keeps. Higher is smoother and cheaper to converge, but holds onto mistakes for more frames.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_HISTORY_BLEND),
            |ui| {
                let mut value = self.video.cloud_history_blend;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=0.98, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_HISTORY_BLEND, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Motion reject",
            "How strongly camera movement throws history away. At 100% any real movement falls back to a full march, which is what the reference implementation does and cannot smear; lower keeps the interleave running while you move, at the cost of trails.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_MOTION_REJECT),
            |ui| {
                let mut value = self.video.cloud_motion_reject;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_MOTION_REJECT, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "History depth reject",
            "Discards history on pixels where solid geometry sits in front of the cloud layer, which stops world geometry and the player model dragging their silhouettes across the sky.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_HISTORY_DEPTH_REJECT),
            |ui| {
                if theme::switch(ui, self.video.cloud_history_depth_reject).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_HISTORY_DEPTH_REJECT, true, None);
                }
            },
        );
    }
}
