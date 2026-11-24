use super::cvar_prelude::*;
use crate::console::cvars::CvarScalar;

video_scalar_cvars! { SCALAR_DEFINITIONS, commit_river_settings;
    APP_R_RIVERNINJADISTANCEMULT {
        name: "r_riverNinjaDistanceMult", default: "",
        range: "0..0.6", description: "Fluid Ninja underwater DistanceMult for new rivers. Lower values allow greater visibility.",
        kind: CvarType::Float { min: 0.0, max: 0.6 }, field: river_settings.ninja_underwater.distance_mult, archive: 29135
    }
    APP_R_RIVERNINJADISTORTION {
        name: "r_riverNinjaDistortion", default: "",
        range: "0..0.05", description: "Fluid Ninja underwater VisualDistortIntensity for new rivers.",
        kind: CvarType::Float { min: 0.0, max: 0.05 }, field: river_settings.ninja_underwater.distortion, archive: 29136
    }
    APP_R_RIVERNINJABRIGHTNESS {
        name: "r_riverNinjaBrightness", default: "",
        range: "0..2", description: "Fluid Ninja underwater GlobalBrightness for new rivers.",
        kind: CvarType::Float { min: 0.0, max: 2.0 }, field: river_settings.ninja_underwater.brightness, archive: 29137
    }
    APP_R_RIVERNINJACAUSTICS {
        name: "r_riverNinjaCaustics", default: "",
        range: "0..200", description: "Fluid Ninja underwater CausticsMult for new rivers.",
        kind: CvarType::Float { min: 0.0, max: 200.0 }, field: river_settings.ninja_underwater.caustics_mult, archive: 29138
    }
}

fn color_text([r, g, b]: [f32; 3]) -> String {
    format!("{r} {g} {b}")
}

app_cvars! {
    APP_R_RIVERNINJADISTANCECOLOR {
        runtime: CvarDef::new("r_riverNinjaDistanceColor", "", "r g b (0..1)",
            "Fluid Ninja underwater DistanceColor for new rivers (linear RGB).",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(color_text(app.video.river_settings.ninja_underwater.distance_color)),
            |app, value| App::cvar_set_river_ninja_distance_color(app, "r_riverNinjaDistanceColor", &value.to_string()),
        ).with_text_setter(App::cvar_set_river_ninja_distance_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _, value, _| {
                if let Some(color) = parse_vec3(value).filter(|rgb| rgb.iter().all(|v| v.is_finite())) {
                    settings.river_settings.ninja_underwater.distance_color = color.map(|v| v.clamp(0.0, 1.0));
                }
            }),
            archive: Some((29139, |s| color_text(s.video.river_settings.ninja_underwater.distance_color))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_river_ninja_distance_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                let color: [f32; 3] = Self::parse_cvar_floats(name, value)?;
                self.video.river_settings.ninja_underwater.distance_color = color.map(|v| v.clamp(0.0, 1.0));
                self.commit_river_settings();
                Ok(())
            }
        }]
    }
    APP_R_RIVERNINJACAUSTICSCOLOR {
        runtime: CvarDef::new("r_riverNinjaCausticsColor", "", "r g b (0..1)",
            "Fluid Ninja underwater CausticsColor for new rivers (linear RGB).",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(color_text(app.video.river_settings.ninja_underwater.caustics_color)),
            |app, value| App::cvar_set_river_ninja_caustics_color(app, "r_riverNinjaCausticsColor", &value.to_string()),
        ).with_text_setter(App::cvar_set_river_ninja_caustics_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _, value, _| {
                if let Some(color) = parse_vec3(value).filter(|rgb| rgb.iter().all(|v| v.is_finite())) {
                    settings.river_settings.ninja_underwater.caustics_color = color.map(|v| v.clamp(0.0, 1.0));
                }
            }),
            archive: Some((29140, |s| color_text(s.video.river_settings.ninja_underwater.caustics_color))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_river_ninja_caustics_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                let color: [f32; 3] = Self::parse_cvar_floats(name, value)?;
                self.video.river_settings.ninja_underwater.caustics_color = color.map(|v| v.clamp(0.0, 1.0));
                self.commit_river_settings();
                Ok(())
            }
        }]
    }
}
