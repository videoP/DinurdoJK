//! Live river material authorship.
use super::{color_row, ocean_slider_d, theme, App};
impl App {
    pub(in crate::app) fn commit_river_settings(&mut self) {
        self.mark_config_dirty();
        self.publish_ui();
    }
    pub(in crate::app::egui_settings) fn egui_river(&mut self, ui: &mut egui::Ui) {
        self.egui_river_target(ui);
        let target = self.river_editor.selected;
        let mut s = target
            .and_then(|id| {
                self.river_editor
                    .entry_mut(id)
                    .map(|e| e.definition.appearance)
            })
            .unwrap_or(self.video.river_settings);
        let d = crate::river_settings::Settings::default();
        let mut changed = false;
        theme::section(ui, "UNDERWATER", "Hybrid-style submerged presentation for this river. r_showtris 128 shows the displaced grid, including dry cells.");
        theme::row(ui, "Underwater optics A/B", "Both modes share spectral optics: A keeps the original appearance; B adds a subtle waterline tint (default). This does not change s_underwater audio.", theme::Reset::None, |ui| {
            changed |= ui.selectable_value(&mut s.underwater_optics, 0, "A · Original").changed();
            changed |= ui.selectable_value(&mut s.underwater_optics, 1, "B · Shared").changed();
        });
        color_row(ui, "Underwater color", "Fog tint below the solved water surface (sRGB).", theme::Reset::None, &mut s.underwater_color, &mut changed);
        ocean_slider_d(ui, "Underwater visibility", "Distance in map units at which blue transmission falls to five percent. Red and green fade sooner, as in Hybrid ocean.", &mut s.underwater_visibility, d.underwater_visibility, 1.0..=65536.0, &mut changed);
        ocean_slider_d(ui, "Underwater darkening depth", "Downwelling-light depth distance in map units; larger values darken more slowly.", &mut s.underwater_darkening, d.underwater_darkening, 1.0..=65536.0, &mut changed);
        theme::section(
            ui,
            "WATER MATERIAL",
            if target.is_some() {
                "Appearance of the selected river. Changes apply immediately."
            } else {
                "Saved appearance defaults for new rivers. Existing rivers keep their own settings."
            },
        );
        {
            let mut c = [
                s.water.scattering[0],
                s.water.scattering[1],
                s.water.scattering[2],
            ];
            color_row(
                ui,
                "Scattering color",
                "Color of light scattered through the river.",
                theme::Reset::None,
                &mut c,
                &mut changed,
            );
            s.water.scattering[..3].copy_from_slice(&c);
        }
        {
            let mut c = [
                s.water.water_albedo[0],
                s.water.water_albedo[1],
                s.water.water_albedo[2],
            ];
            color_row(ui,"Surface albedo","Diffuse surface color; clear water is colored mainly by absorption and scattering.",theme::Reset::None,&mut c,&mut changed);
            s.water.water_albedo[..3].copy_from_slice(&c);
        }
        {
            let mut c = [
                s.foam.base_color[0],
                s.foam.base_color[1],
                s.foam.base_color[2],
            ];
            color_row(
                ui,
                "Foam color",
                "Base color of foam under scene lighting.",
                theme::Reset::None,
                &mut c,
                &mut changed,
            );
            s.foam.base_color[..3].copy_from_slice(&c);
        }
        ocean_slider_d(
            ui,
            "Scattering strength",
            "Amount of light scattered through the water.",
            &mut s.water.scattering[3],
            d.water.scattering[3],
            0.0..=5.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Red absorption distance",
            "Red-channel distance parameter; larger values transmit more red light.",
            &mut s.water.absorption[0],
            d.water.absorption[0],
            0.1..=200.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Green absorption distance",
            "Green-channel distance parameter; larger values transmit more green light.",
            &mut s.water.absorption[1],
            d.water.absorption[1],
            0.1..=200.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Blue absorption distance",
            "Blue-channel distance parameter; larger values transmit more blue light.",
            &mut s.water.absorption[2],
            d.water.absorption[2],
            0.1..=200.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Absorption distance scale",
            "Multiplies all three absorption distances. Higher values make the river clearer.",
            &mut s.water.absorption[3],
            d.water.absorption[3],
            0.1..=100.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Scattering direction",
            "Bias of scattered light toward forward or backward directions.",
            &mut s.water.anisotropy,
            d.water.anisotropy,
            -0.99..=0.99,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Surface roughness",
            "Width of sun highlights and environment reflections.",
            &mut s.water.water_roughness,
            d.water.water_roughness,
            0.02..=1.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Grazing roughness",
            "Surface roughness when viewing along the water.",
            &mut s.water.water_fresnel_roughness,
            d.water.water_fresnel_roughness,
            0.02..=1.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Surface specular",
            "Dielectric reflectivity; also changes the optical index of refraction.",
            &mut s.water.water_specular,
            d.water.water_specular,
            0.0..=1.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Grazing specular",
            "Material specular value at grazing view angles.",
            &mut s.water.water_fresnel_specular,
            d.water.water_fresnel_specular,
            0.0..=1.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Refraction strength",
            "Bending of the scene through the water.",
            &mut s.water.refraction,
            d.water.refraction,
            1.0..=2.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Distant refraction",
            "Refraction strength at the distant-water transition.",
            &mut s.water.refraction_far,
            d.water.refraction_far,
            1.0..=2.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Refraction transition (cm)",
            "View depth at which distant refraction is reached.",
            &mut s.water.depth_for_min_refraction,
            d.water.depth_for_min_refraction,
            1.0..=50000.0,
            &mut changed,
        );
        theme::section(ui, "POUROVERS (A/B)", "Drawn water surface over ledges and drops, for all rivers, live. The simulation is unchanged. Console: r_riverSmoothing, r_riverSmoothingCutoff, r_riverBedClamp.");
        theme::row(ui, "Surface smoothing", "Unreal's render-height smoothing. Unreal default ramps the surface across drops, so it can dip under a ledge. Edge-preserving ignores neighbours beyond the height cutoff and keeps drops sharp. None and Blur are for comparison.", theme::Reset::None, |ui| {
            let current = crate::river_runtime::smoothing_method();
            for (method, label) in [(2u8, "Unreal default"), (1, "Edge-preserving"), (3, "Blur"), (0, "None")] {
                if theme::chip(ui, label, current == method).clicked() && current != method {
                    let _ = self.set_console_cvar("r_riverSmoothing", &method.to_string());
                }
                ui.add_space(3.0);
            }
        });
        {
            let mut cutoff = crate::river_runtime::smoothing_cutoff_cm() / jka_water::river::CENTIMETRES_PER_MAP_UNIT;
            let mut cutoff_changed = false;
            ocean_slider_d(ui, "Smoothing height cutoff", "Map units (Unreal SmoothingHeightCutoff; 200 = 5 m). Edge-preserving ignores neighbours this much higher or lower; Unreal default weights them fully at this difference. Small values with Edge-preserving keep pourovers crisp.", &mut cutoff, 200.0, 0.0..=1000.0, &mut cutoff_changed);
            if cutoff_changed {
                let _ = self.set_console_cvar("r_riverSmoothingCutoff", &cutoff.to_string());
            }
        }
        {
            let mut clamp = crate::river_runtime::bed_clamp();
            if ui.checkbox(&mut clamp, "Bed clamp (pourover fix)").on_hover_text("Not an Unreal feature. Keeps the drawn water on or above the bed it covers, and lifts mesh vertices on drops steeper than 45 degrees to the upper water, so the water falls in front of a ledge instead of sliding under it.").changed() {
                let _ = self.set_console_cvar("r_riverBedClamp", if clamp { "1" } else { "0" });
            }
        }
        {
            let mut spray = crate::river_runtime::whitewater();
            if ui.checkbox(&mut spray, "Whitewater spray").on_hover_text("Unreal's river secondary emitter: spray sprites where the surface bends sharply over drops in fast water. Uses the foam minimum depth. Console: r_riverWhitewater.").changed() {
                let _ = self.set_console_cvar("r_riverWhitewater", if spray { "1" } else { "0" });
            }
        }
        theme::section(ui, "NEAR WATER MESH (A/B)", "Renderer-only mesh density; the hydraulic simulation stays at the original resolution.");
        {
            let mut adaptive = jka_river::river_blocks::adaptive();
            if ui.checkbox(&mut adaptive, "Adaptive water mesh (B)")
                .on_hover_text("Off: one vertex per solver cell everywhere. On: each 16x16-cell block gets its mesh from how turbulent its water is (surface bends, steps and flow speed), not from distance. Turbulent water keeps your tile resolution; calmer water uses fewer triangles. It never adds triangles. Banks stay at least 4-cell. Blocks hold their detail briefly so they do not flicker. No solver changes. Console: r_riverAdaptiveMesh.")
                .changed() {
                let _ = self.set_console_cvar("r_riverAdaptiveMesh", if adaptive { "1" } else { "0" });
            }
            let mut sensitivity = jka_river::river_blocks::sensitivity();
            if ui.add_enabled(adaptive, egui::Slider::new(&mut sensitivity, 0.0..=8.0).text("Turbulence sensitivity"))
                .on_hover_text("Higher: more of the river keeps the full tile resolution. Lower: more water uses the coarse meshes. Console: r_riverMeshTurbulence.")
                .changed() {
                let _ = self.set_console_cvar("r_riverMeshTurbulence", &format!("{sensitivity:.2}"));
            }
            theme::hint(ui, "Use r_showtris 128 to see the mesh and r_gpuTimings 1 for GPU comparison.", theme::TEXT_FAINT());
        }
        theme::section(ui, "SURFACE DETAIL", "Flow-advected wave detail.");
        {
            let mut mode = i32::from(crate::river_runtime::material_lod());
            ui.horizontal(|ui| {
                ui.label("Fragment detail (A/B)");
                egui::ComboBox::from_id_salt("river_fragment_detail_lod")
                    .selected_text(match mode {
                        0 => "0 — Original",
                        1 => "1 — Adaptive waves",
                        _ => "2 — Adaptive waves + foam",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut mode, 0, "0 — Original");
                        ui.selectable_value(&mut mode, 1, "1 — Adaptive waves");
                        ui.selectable_value(&mut mode, 2, "2 — Adaptive waves + foam");
                    });
            });
            if mode != i32::from(crate::river_runtime::material_lod()) {
                let _ = self.set_console_cvar("r_riverMaterialLOD", &mode.to_string());
            }
            let mut cutoff = crate::river_runtime::material_lod_cutoff();
            let slider = ui.add_enabled(
                mode != 0,
                egui::Slider::new(&mut cutoff, 0.0..=8.0).step_by(0.25).text("Cheap-path cutoff (mips)"),
            );
            if slider.on_hover_text("Lower = the cheap path starts on nearer water. 0 = all river pixels, 2 = default, 4 = original patch (far/grazing only). Watch for seams as you lower it.").changed() {
                let _ = self.set_console_cvar("r_riverMaterialLODCutoff", &format!("{cutoff}"));
            }
            ui.small("Screen-space texture-footprint LOD; fragment work only, no solver or mesh changes. Console: r_riverMaterialLOD.");
        }
        ocean_slider_d(
            ui,
            "Detail normal strength",
            "Strength of the existing flow-advected FFT detail normals.",
            &mut s.wave_amount,
            d.wave_amount,
            0.0..=5.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Wave height",
            "Multiplier of the existing flow-advected FFT displacement.",
            &mut s.wave_height,
            d.wave_height,
            0.0..=30.0,
            &mut changed,
        );
        theme::section(ui, "FOAM GENERATION", "Depth gate for new foam. Existing foam keeps advecting and fading; turbulence and speed gates still apply.");
        ocean_slider_d(ui, "Foam minimum depth", "Water-column thickness in map units below which no new foam forms. Default 60 units = 1.5 m. Full strength is calculated at 3.333 times this depth; zero removes the depth gate.", &mut s.foam_minimum_depth, d.foam_minimum_depth, 0.0..=200.0, &mut changed);
        theme::hint(ui, &format!("Full depth weight at {:.2} map units", s.foam_minimum_depth * (500.0 / 150.0)), theme::TEXT_FAINT());
        theme::section(
            ui,
            "FOAM MATERIAL",
            "Appearance of the native simulated foam.",
        );
        ocean_slider_d(
            ui,
            "Foam gain",
            "Amplifies existing simulated foam; does not generate new foam.",
            &mut s.foam.gain,
            d.foam.gain,
            0.0..=5.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Foam opacity",
            "Coverage of the water by existing foam.",
            &mut s.foam.opacity,
            d.foam.opacity,
            0.0..=1.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Foam roughness",
            "Width of highlights and reflections on foam.",
            &mut s.foam.roughness,
            d.foam.roughness,
            0.02..=1.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Foam specular",
            "Dielectric reflectivity of foam.",
            &mut s.foam.specular,
            d.foam.specular,
            0.0..=1.0,
            &mut changed,
        );
        ocean_slider_d(
            ui,
            "Foam normal flatness",
            "Flattens the foam texture normals toward the surface normal.",
            &mut s.foam.normal_flatness,
            d.foam.normal_flatness,
            0.0..=1.0,
            &mut changed,
        );
        if theme::ghost_button(ui, "RESET RIVER").clicked() {
            s = d;
            changed = true;
        }
        if changed {
            if let Some(id) = target {
                if let Some(entry) = self.river_editor.entry_mut(id) {
                    entry.definition.appearance = s;
                }
                self.render_command(crate::renderer::RenderCommand::RiverMaterial(id, s));
            } else {
                self.commit_river_defaults(s);
            }
        }
    }
    fn commit_river_defaults(&mut self, s: crate::river_settings::Settings) {
        if s.underwater_optics != self.video.river_settings.underwater_optics {
            let _ = self.set_console_cvar("r_riverUnderwaterOptics", &s.underwater_optics.to_string());
        }
        if s.foam_minimum_depth != self.video.river_settings.foam_minimum_depth {
            let _ = self.set_console_cvar("r_riverFoamMinimumDepth", &s.foam_minimum_depth.to_string());
        }
        if s.underwater_color != self.video.river_settings.underwater_color {
            let c = s.underwater_color;
            let _ = self.set_console_cvar("r_riverUnderwaterColor", &format!("{} {} {}", c[0], c[1], c[2]));
        }
        if s.underwater_visibility != self.video.river_settings.underwater_visibility {
            let _ = self.set_console_cvar("r_riverUnderwaterVisibility", &s.underwater_visibility.to_string());
        }
        if s.underwater_darkening != self.video.river_settings.underwater_darkening {
            let _ = self.set_console_cvar("r_riverUnderwaterDarkening", &s.underwater_darkening.to_string());
        }
        if s.water.scattering[..3] != self.video.river_settings.water.scattering[..3] {
            let c = s.water.scattering;
            let _ = self.set_console_cvar(
                "r_riverScatteringColor",
                &format!("{} {} {}", c[0], c[1], c[2]),
            );
        }
        if s.water.water_albedo[..3] != self.video.river_settings.water.water_albedo[..3] {
            let c = s.water.water_albedo;
            let _ =
                self.set_console_cvar("r_riverWaterAlbedo", &format!("{} {} {}", c[0], c[1], c[2]));
        }
        if s.foam.base_color[..3] != self.video.river_settings.foam.base_color[..3] {
            let c = s.foam.base_color;
            let _ =
                self.set_console_cvar("r_riverFoamColor", &format!("{} {} {}", c[0], c[1], c[2]));
        }
        if s.foam.scattering_color[..3] != self.video.river_settings.foam.scattering_color[..3] {
            let c = s.foam.scattering_color;
            let _ = self.set_console_cvar(
                "r_riverFoamScatteringColor",
                &format!("{} {} {}", c[0], c[1], c[2]),
            );
        }
        if s.water.scattering[3] != self.video.river_settings.water.scattering[3] {
            let _ = self.set_console_cvar(
                "r_riverScatteringStrength",
                &s.water.scattering[3].to_string(),
            );
        }
        if s.water.absorption[0] != self.video.river_settings.water.absorption[0] {
            let _ =
                self.set_console_cvar("r_riverAbsorptionRed", &s.water.absorption[0].to_string());
        }
        if s.water.absorption[1] != self.video.river_settings.water.absorption[1] {
            let _ =
                self.set_console_cvar("r_riverAbsorptionGreen", &s.water.absorption[1].to_string());
        }
        if s.water.absorption[2] != self.video.river_settings.water.absorption[2] {
            let _ =
                self.set_console_cvar("r_riverAbsorptionBlue", &s.water.absorption[2].to_string());
        }
        if s.water.absorption[3] != self.video.river_settings.water.absorption[3] {
            let _ =
                self.set_console_cvar("r_riverAbsorptionScale", &s.water.absorption[3].to_string());
        }
        if s.water.anisotropy != self.video.river_settings.water.anisotropy {
            let _ = self.set_console_cvar("r_riverAnisotropy", &s.water.anisotropy.to_string());
        }
        if s.water.water_roughness != self.video.river_settings.water.water_roughness {
            let _ = self.set_console_cvar("r_riverRoughness", &s.water.water_roughness.to_string());
        }
        if s.water.water_fresnel_roughness
            != self.video.river_settings.water.water_fresnel_roughness
        {
            let _ = self.set_console_cvar(
                "r_riverFresnelRoughness",
                &s.water.water_fresnel_roughness.to_string(),
            );
        }
        if s.water.water_specular != self.video.river_settings.water.water_specular {
            let _ = self.set_console_cvar("r_riverSpecular", &s.water.water_specular.to_string());
        }
        if s.water.water_fresnel_specular != self.video.river_settings.water.water_fresnel_specular
        {
            let _ = self.set_console_cvar(
                "r_riverFresnelSpecular",
                &s.water.water_fresnel_specular.to_string(),
            );
        }
        if s.water.refraction != self.video.river_settings.water.refraction {
            let _ = self.set_console_cvar("r_riverRefraction", &s.water.refraction.to_string());
        }
        if s.water.refraction_far != self.video.river_settings.water.refraction_far {
            let _ =
                self.set_console_cvar("r_riverRefractionFar", &s.water.refraction_far.to_string());
        }
        if s.water.depth_for_min_refraction
            != self.video.river_settings.water.depth_for_min_refraction
        {
            let _ = self.set_console_cvar(
                "r_riverRefractionDistance",
                &s.water.depth_for_min_refraction.to_string(),
            );
        }
        if s.wave_amount != self.video.river_settings.wave_amount {
            let _ = self.set_console_cvar("r_riverWaveAmount", &s.wave_amount.to_string());
        }
        if s.wave_height != self.video.river_settings.wave_height {
            let _ = self.set_console_cvar("r_riverWaveHeight", &s.wave_height.to_string());
        }
        if s.foam.gain != self.video.river_settings.foam.gain {
            let _ = self.set_console_cvar("r_riverFoamGain", &s.foam.gain.to_string());
        }
        if s.foam.opacity != self.video.river_settings.foam.opacity {
            let _ = self.set_console_cvar("r_riverFoamOpacity", &s.foam.opacity.to_string());
        }
        if s.foam.roughness != self.video.river_settings.foam.roughness {
            let _ = self.set_console_cvar("r_riverFoamRoughness", &s.foam.roughness.to_string());
        }
        if s.foam.specular != self.video.river_settings.foam.specular {
            let _ = self.set_console_cvar("r_riverFoamSpecular", &s.foam.specular.to_string());
        }
        if s.foam.normal_flatness != self.video.river_settings.foam.normal_flatness {
            let _ = self.set_console_cvar(
                "r_riverFoamNormalFlatness",
                &s.foam.normal_flatness.to_string(),
            );
        }
    }
}
