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
        theme::section(ui, "SURFACE DETAIL", "Flow-advected wave detail.");
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
