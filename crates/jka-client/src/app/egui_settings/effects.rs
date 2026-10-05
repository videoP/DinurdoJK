//! Effects.
use crate::app::egui_settings::{
    index_of, segmented_row, theme, ui, App, FxGeometryMode, Ghoul2SkinningMode,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_effects(&mut self, ui: &mut egui::Ui) {
        theme::row(
            ui,
            "FX FPS",
            "Fixed sampling rate for continuous projectile/trail EFX. This removes the stock JKA \
             render-FPS dependency without changing authored EFX count/life/delay values. Drag to \
             the far-right Legacy JKA endpoint to restore one PlayEffect call per presentation frame. \
             0 in cg_fxFPS also selects Legacy JKA.",
            theme::Reset::Video(ui::VIDEO_ROW_FX_FPS),
            |ui| {
                const LEGACY_SLIDER_VALUE: f32 = crate::fx::FX_FPS_MAX as f32 + 1.0;
                let mut slider_value = if self.video.fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
                    LEGACY_SLIDER_VALUE
                } else {
                    self.video.fx_fps.clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX) as f32
                };
                let readout = if self.video.fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
                    "Legacy JKA".to_owned()
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

        ui.add_enabled_ui(self.video.ghoul2_skinning == Ghoul2SkinningMode::Gpu, |ui| {
        if let Some(value) = segmented_row(
            ui,
            "Burn marks",
            "cg_ghoul2Marks. Scorch marks left on player models by blaster, rocket and thermal hits, kept per model (jaPRO defaults to 16). They follow the animation and fade after 10-20 seconds. Needs GPU Ghoul2 skinning (Video > Models).",
            theme::Reset::None,
            self.audio.game.g2_marks,
            &[(0, "Off"), (4, "4"), (16, "16"), (32, "32")],
        ) {
            let _ = self.set_console_cvar("cg_ghoul2Marks", &value.to_string());
        }
        });

        const FX_PHYSICS: [(u32, &str); 3] = [
            (crate::fx::FX_PHYSICS_OFF, "Off"),
            (crate::fx::FX_PHYSICS_AUTHORED, "Authored"),
            (crate::fx::FX_PHYSICS_ALL, "All"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "FX physics",
            "World collision for bouncing EFX debris (fx_physics, as in TaystJK). Particles, tails and \
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

        const FX_GEOMETRY: [(FxGeometryMode, &str); 3] = [
            (FxGeometryMode::Cpu, "CPU"),
            (FxGeometryMode::CpuWorkers, "CPU workers"),
            (FxGeometryMode::Gpu, "GPU"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "FX geometry",
            "View-dependent EFX geometry. CPU is the exact current reference path. CPU workers uses the same authored particles/materials and exact same CPU geometry, but parallelizes tessellation over Rayon. GPU keeps FX simulation on the dedicated jka-fx CPU thread and moves EFX Particle billboard expansion/submission to compact WGPU instances; non-sprite primitives remain on CPU. Hardware RT lighting/shadows currently fall back to CPU workers for this mode.",
            theme::Reset::Video(ui::VIDEO_ROW_FX_GEOMETRY),
            self.video.fx_geometry,
            &FX_GEOMETRY,
        ) {
            let current = FxGeometryMode::ALL
                .iter()
                .position(|mode| *mode == self.video.fx_geometry)
                .unwrap_or(0);
            let next = FxGeometryMode::ALL
                .iter()
                .position(|mode| *mode == target)
                .unwrap_or(current);
            self.video_selected = ui::VIDEO_ROW_FX_GEOMETRY;
            self.change_video_setting(next as i32 - current as i32);
        }

        ui.add_enabled_ui(self.video.fx_geometry == FxGeometryMode::Gpu, |ui| {
        self.egui_toggle_row(
            ui,
            "FX zero-alpha discard",
            "A/B diagnostic for r_fxGeometry GPU. Discards only fragments whose final source alpha is exactly zero on source-alpha EFX sprite blend modes, before fog/blending. Non-zero-alpha edges are unchanged; GL_ONE/additive-one and modulation modes are intentionally untouched.",
            ui::VIDEO_ROW_FX_ZERO_ALPHA_DISCARD,
            self.video.fx_zero_alpha_discard,
        );
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

        self.egui_toggle_row(
            ui,
            "Flares",
            "Screen-space flare overlays such as the OpenJK saber clash flash. Off suppresses only \
             the flare overlay; saber impact particles, sounds, dynamic lights and marks are unchanged.",
            ui::VIDEO_ROW_FLARES,
            self.video.flares,
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
    }
}
