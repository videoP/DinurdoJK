//! BSP curved-patch controls, ported from TaystJK vanilla.
use super::cvar_prelude::*;

app_cvars! {
    APP_R_SUBDIVISIONS {
        runtime: CvarDef::new("r_subdivisions", "", "0..80", "Curvature error in map units; applied on map load or vid_restart.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::MapLoadOrVidRestart, |app: &App| CvarValue::String(app.video.patch_subdivisions.to_string()), |app, value| App::cvar_set_r_subdivisions(app, APP_R_SUBDIVISIONS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_subdivisions, Some(|name, value| { let parsed = japro_float(name, value)?.clamp(0.0, 80.0); Ok(parsed.to_string()) })),
        config: CvarBinding {
            load_video: Some(|settings, name, value, _| { settings.patch_subdivisions = japro_float(name, value).unwrap_or(settings.patch_subdivisions).clamp(0.0, 80.0); }),
            archive: Some((500, |state| state.video.patch_subdivisions.to_string())),
            apply_latched: Some(|settings, value| { let name = "r_subdivisions"; settings.patch_subdivisions = japro_float(name, value).unwrap_or(settings.patch_subdivisions).clamp(0.0, 80.0); }),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_subdivisions(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.patch_subdivisions = japro_float(name, value)?.clamp(0.0, 80.0);
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
    APP_R_PATCHSTITCHING {
        runtime: CvarDef::new("r_patchStitching", "", "integer", "Stitch compatible patch boundaries on map load; shared LOD errors are always reconciled.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app: &App| CvarValue::String(bool_value(app.video.patch_stitching)), |app, value| App::cvar_set_r_patchstitching(app, APP_R_PATCHSTITCHING.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_patchstitching, Some(|name, value| { let parsed = japro_integer(name, value)? != 0; Ok(u8::from(parsed).to_string()) })),
        config: CvarBinding {
            load_video: Some(|settings, name, value, _| { settings.patch_stitching = japro_integer(name, value).map(|v| v != 0).unwrap_or(settings.patch_stitching); }),
            archive: Some((501, |state| bool_value(state.video.patch_stitching))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_patchstitching(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.patch_stitching = japro_integer(name, value)? != 0;
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
    APP_R_LODCURVEERROR {
        runtime: CvarDef::new("r_lodCurveError", "", "finite number", "Runtime patch LOD error; larger values retain more rows/columns; negative values use threshold zero.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app: &App| CvarValue::String(app.video.lod_curve_error.to_string()), |app, value| App::cvar_set_r_lodcurveerror(app, APP_R_LODCURVEERROR.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_lodcurveerror, None),
        config: CvarBinding {
            load_video: Some(|settings, name, value, _| { settings.lod_curve_error = japro_float(name, value).unwrap_or(settings.lod_curve_error); }),
            archive: Some((502, |state| state.video.lod_curve_error.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_lodcurveerror(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.lod_curve_error = japro_float(name, value)?;
                    self.render_command(RenderCommand::SetPatchLod { curve_error: self.video.lod_curve_error, no_curves: self.video.no_curves, speeds: self.video.speeds });
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
    APP_R_NOCURVES {
        runtime: CvarDef::new("r_nocurves", "", "integer", "Suppress curved patch rendering; physical map geometry remains stable.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app: &App| CvarValue::String(bool_value(app.video.no_curves)), |app, value| App::cvar_set_r_nocurves(app, APP_R_NOCURVES.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_nocurves, None),
        config: CvarBinding {
            default: Some(|state| bool_value(state.video.no_curves)),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_nocurves(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.no_curves = japro_integer(name, value)? != 0;
                    self.render_command(RenderCommand::SetPatchLod { curve_error: self.video.lod_curve_error, no_curves: self.video.no_curves, speeds: self.video.speeds });
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
    APP_R_FRAMETIMEGRAPH {
        runtime: CvarDef::new("r_frameTimeGraph", "0", "0|1", "Transparent 1-pixel/frame CPU phase graph (prepare/acquire/encode/submit/present/other), percentile-colored tips, and optional async GPU readout with r_gpuTimings 1. Updates at 30 Hz; session-only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.frame_time_graph),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { return Err("r_frameTimeGraph expects 0 or 1".into()); };
                app.frame_time_graph = enabled;
                app.render_command(RenderCommand::SetFrameTimeGraph(enabled));
                Ok(())
            }),
        config: CvarBinding {
            default: Some(|_| "0".to_string()),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_SPEEDS {
        runtime: CvarDef::new("r_speeds", "", "0..8", "TaystJK-style renderer stats: 1 world geometry, 2 culling, 3 viewcluster, 4 dynamic lights, 5 zFar, 6 flares, 7 memory; 8 extended patch LOD diagnostics.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app: &App| CvarValue::String(app.video.speeds.to_string()), |app, value| App::cvar_set_r_speeds(app, APP_R_SPEEDS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_speeds, None),
        config: CvarBinding {
            default: Some(|state| state.video.speeds.to_string()),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_speeds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.speeds = japro_integer(name, value)? as i32;
                    self.render_command(RenderCommand::SetPatchLod { curve_error: self.video.lod_curve_error, no_curves: self.video.no_curves, speeds: self.video.speeds });
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_cvars_persist_float_values_and_latch_only_load_time_controls() {
        let mut video = VideoSettings::default();
        let mut context = VideoLoadState::default();
        for (name,value) in [("r_subdivisions","1.5"),("r_patchStitching","0"),("r_lodCurveError","-25")] {
            let definition=DEFINITIONS.iter().find(|d|d.runtime.entry.name==name).unwrap();
            (definition.config.load_video.unwrap())(&mut video,name,value,&mut context);
        }
        assert_eq!(video.patch_subdivisions,1.5);
        assert!(!video.patch_stitching);
        assert_eq!(video.lod_curve_error,-25.0);
        for (name,scope) in [("r_subdivisions",LatchScope::MapLoadOrVidRestart),("r_patchStitching",LatchScope::None),("r_lodCurveError",LatchScope::None),("r_nocurves",LatchScope::None),("r_speeds",LatchScope::None)] {
            let definition=DEFINITIONS.iter().find(|d|d.runtime.entry.name==name).unwrap();
            assert_eq!(definition.runtime.entry.latch,scope);
            assert_eq!(definition.config.apply_latched.is_some(),scope!=LatchScope::None);
        }
        let sub=&DEFINITIONS[0];
        (sub.config.apply_latched.unwrap())(&mut video,"999");assert_eq!(video.patch_subdivisions,80.0);
        (sub.config.apply_latched.unwrap())(&mut video,"-1");assert_eq!(video.patch_subdivisions,0.0);
        (sub.config.apply_latched.unwrap())(&mut video,"NaN");assert_eq!(video.patch_subdivisions,0.0);
    }
}
