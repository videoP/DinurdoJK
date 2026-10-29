//! Jiggle cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_R_JIGGLEBREASTSTRENGTH {
        runtime: CvarDef::new(
                "r_jiggleBreastStrength",
                "",
                "0..2",
                "Chest-region soft-tissue displacement multiplier.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jigglebreaststrength(app)),
                |app, value| {
                    App::cvar_set_r_jigglebreaststrength(
                        app,
                        APP_R_JIGGLEBREASTSTRENGTH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_jigglebreaststrength, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_breast_strength = v.clamp(0.0, 2.0);
                    }
                }),
                archive: Some((185, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_breast_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglebreaststrength(&self) -> String {
                    format!("{:.3}", self.video.jiggle_breast_strength)
                }
                fn cvar_set_r_jigglebreaststrength(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_jigglebreaststrength".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEDAMPING {
        runtime: CvarDef::new(
                "r_jiggleDamping",
                "",
                "0..3",
                "Multiplier for KawaiiPhysics-style velocity damping.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jiggledamping(app)),
                |app, value| {
                    App::cvar_set_r_jiggledamping(app, APP_R_JIGGLEDAMPING.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_jiggledamping, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_damping = v.clamp(0.0, 3.0);
                    }
                }),
                archive: Some((188, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_damping)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jiggledamping(&self) -> String {
                    format!("{:.3}", self.video.jiggle_damping)
                }
                fn cvar_set_r_jiggledamping(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jiggledamping".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEGLUTECOVERAGEHEIGHT {
        runtime: CvarDef::new(
                "r_jiggleGluteCoverageHeight",
                "",
                "0..1",
                "Deprecated normalized remesh control retained for config compatibility; use the explicit breast/glute remesh Z bounds.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jiggleglutecoverageheight(app)),
                |app, value| {
                    App::cvar_set_r_jiggleglutecoverageheight(
                        app,
                        APP_R_JIGGLEGLUTECOVERAGEHEIGHT.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_jiggleglutecoverageheight, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_glute_coverage_height = v.clamp(0.0, 1.0);
                    }
                }),
                archive: Some((418, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_glute_coverage_height)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jiggleglutecoverageheight(&self) -> String {
                    format!("{:.3}", self.video.jiggle_glute_coverage_height)
                }
                fn cvar_set_r_jiggleglutecoverageheight(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let amount = value
                        .trim()
                        .parse::<f32>()
                        .map_err(|_| format!("{name}: expected a finite number"))?;
                    if !amount.is_finite() {
                        return Err(format!("{name}: expected a finite number"));
                    }
                    self.video.jiggle_glute_coverage_height = amount.clamp(0.0, 1.0);
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEGLUTELIFT {
        runtime: CvarDef::new(
                "r_jiggleGluteLift",
                "",
                "-0.4..0.6",
                "Deprecated normalized glute trim retained for config compatibility; use r_jiggleGluteJiggleBottomZ/r_jiggleGluteJiggleTopZ.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jiggleglutelift(app)),
                |app, value| {
                    App::cvar_set_r_jiggleglutelift(
                        app,
                        APP_R_JIGGLEGLUTELIFT.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_jiggleglutelift, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_glute_lift = v.clamp(-0.4, 0.6);
                    }
                }),
                archive: Some((189, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_glute_lift)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jiggleglutelift(&self) -> String {
                    format!("{:.3}", self.video.jiggle_glute_lift)
                }
                fn cvar_set_r_jiggleglutelift(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jiggleglutelift".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEGLUTESTRENGTH {
        runtime: CvarDef::new(
                "r_jiggleGluteStrength",
                "",
                "0..2",
                "Glute-region soft-tissue displacement multiplier.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jiggleglutestrength(app)),
                |app, value| {
                    App::cvar_set_r_jiggleglutestrength(
                        app,
                        APP_R_JIGGLEGLUTESTRENGTH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_jiggleglutestrength, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_glute_strength = v.clamp(0.0, 2.0);
                    }
                }),
                archive: Some((186, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_glute_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jiggleglutestrength(&self) -> String {
                    format!("{:.3}", self.video.jiggle_glute_strength)
                }
                fn cvar_set_r_jiggleglutestrength(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_jiggleglutestrength".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEJPAIRDRAG {
        runtime: CvarDef::new(
                "r_jiggleJPAirDrag",
                "",
                "0..1",
                "naelstrof/JigglePhysics world-motion air drag.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jigglejpairdrag(app)),
                |app, value| {
                    App::cvar_set_r_jigglejpairdrag(
                        app,
                        APP_R_JIGGLEJPAIRDRAG.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_jigglejpairdrag, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_jp_air_drag = v.clamp(0.0, 1.0);
                    }
                }),
                archive: Some((192, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_jp_air_drag)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglejpairdrag(&self) -> String {
                    format!("{:.3}", self.video.jiggle_jp_air_drag)
                }
                fn cvar_set_r_jigglejpairdrag(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jigglejpairdrag".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEJPDRAG {
        runtime: CvarDef::new(
                "r_jiggleJPDrag",
                "",
                "0..1",
                "naelstrof/JigglePhysics mechanical/local-space drag.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jigglejpdrag(app)),
                |app, value| {
                    App::cvar_set_r_jigglejpdrag(app, APP_R_JIGGLEJPDRAG.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_jigglejpdrag, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_jp_drag = v.clamp(0.0, 1.0);
                    }
                }),
                archive: Some((191, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_jp_drag)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglejpdrag(&self) -> String {
                    format!("{:.3}", self.video.jiggle_jp_drag)
                }
                fn cvar_set_r_jigglejpdrag(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jigglejpdrag".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEJPGRAVITY {
        runtime: CvarDef::new("r_jiggleJPGravity", "", "0..2", "naelstrof/JigglePhysics gravity multiplier for soft-region points; 0 is this client's body-tissue preset.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_jigglejpgravity(app)), |app, value| App::cvar_set_r_jigglejpgravity(app, APP_R_JIGGLEJPGRAVITY.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_jigglejpgravity, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_jp_gravity = v.clamp(0.0, 2.0);
                    }
                }),
                archive: Some((195, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_jp_gravity)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglejpgravity(&self) -> String {
                    format!("{:.3}", self.video.jiggle_jp_gravity)
                }
                fn cvar_set_r_jigglejpgravity(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jigglejpgravity".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEJPSOFTEN {
        runtime: CvarDef::new(
                "r_jiggleJPSoften",
                "",
                "0..1",
                "naelstrof/JigglePhysics stiffness softening near the animated rest pose.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jigglejpsoften(app)),
                |app, value| {
                    App::cvar_set_r_jigglejpsoften(app, APP_R_JIGGLEJPSOFTEN.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_jigglejpsoften, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_jp_soften = v.clamp(0.0, 1.0);
                    }
                }),
                archive: Some((194, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_jp_soften)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglejpsoften(&self) -> String {
                    format!("{:.3}", self.video.jiggle_jp_soften)
                }
                fn cvar_set_r_jigglejpsoften(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jigglejpsoften".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEJPSTIFFNESS {
        runtime: CvarDef::new(
                "r_jiggleJPStiffness",
                "",
                "0..1",
                "naelstrof/JigglePhysics stiffness/angle elasticity for the virtual soft-region child.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jigglejpstiffness(app)),
                |app, value| {
                    App::cvar_set_r_jigglejpstiffness(
                        app,
                        APP_R_JIGGLEJPSTIFFNESS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_jigglejpstiffness, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_jp_stiffness = v.clamp(0.0, 1.0);
                    }
                }),
                archive: Some((190, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_jp_stiffness)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglejpstiffness(&self) -> String {
                    format!("{:.3}", self.video.jiggle_jp_stiffness)
                }
                fn cvar_set_r_jigglejpstiffness(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jigglejpstiffness".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEJPSTRETCH {
        runtime: CvarDef::new("r_jiggleJPStretch", "", "0..1", "naelstrof/JigglePhysics squash/stretch control; 0 rigid length, 1 length follows stiffness.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_jigglejpstretch(app)), |app, value| App::cvar_set_r_jigglejpstretch(app, APP_R_JIGGLEJPSTRETCH.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_jigglejpstretch, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_jp_stretch = v.clamp(0.0, 1.0);
                    }
                }),
                archive: Some((193, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_jp_stretch)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglejpstretch(&self) -> String {
                    format!("{:.3}", self.video.jiggle_jp_stretch)
                }
                fn cvar_set_r_jigglejpstretch(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jigglejpstretch".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLEPHYSICS {
        runtime: CvarDef::new("r_jigglePhysics", "", "0|1|2|3", "Soft-tissue targets by reported player sex: 0 Off, 1 Female, 2 Male, 3 Both.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_jigglephysics(app)), |app, value| App::cvar_set_r_jigglephysics(app, APP_R_JIGGLEPHYSICS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_jigglephysics, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.jiggle_physics = VideoSettings::parse_jiggle_mode(value).unwrap_or(settings.jiggle_physics);
                }),
                archive: Some((213, |state| {
                    let settings = state.video;
                    settings.jiggle_physics.to_string()
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglephysics(&self) -> String {
                    self.video.jiggle_physics.to_string()
                }
                fn cvar_set_r_jigglephysics(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.jiggle_physics = VideoSettings::parse_jiggle_mode(value)
                        .ok_or_else(|| format!("{name}: expected 0 Off, 1 Female, 2 Male, or 3 Both"))?;
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HAIRPHYSICS {
        runtime: CvarDef::new(
            "cg_hairPhysics", "", "0|1|2",
            "Procedural hair: 0 Off; 1 authored Source jigglebones; 2 authored + auto hair* on native GLMs.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.hair_physics.to_string()),
            |app, value| App::cvar_set_cg_hairphysics(app, APP_CG_HAIRPHYSICS.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_cg_hairphysics, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(mode) = value.trim().parse::<u8>().ok().filter(|&mode| mode <= 2) {
                    settings.hair_physics = mode;
                }
            }),
            archive: Some((28100, |state| state.video.hair_physics.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_cg_hairphysics(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = value.trim().parse::<u8>()
                        .ok().filter(|&mode| mode <= 2)
                        .ok_or_else(|| format!("{name}: expected 0 Off, 1 Authored, or 2 Authored + GLM Hair"))?;
                    if self.video.hair_physics != mode {
                        self.video.hair_physics = mode;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLESOLVER {
        runtime: CvarDef::new("r_jiggleSolver", "", "0|1", "Soft-tissue solver: 0 KawaiiPhysics-derived point solve, 1 naelstrof/JigglePhysics Verlet/constraint solve.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_jigglesolver(app)), |app, value| App::cvar_set_r_jigglesolver(app, APP_R_JIGGLESOLVER.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_jigglesolver, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    let normalized = value.trim().to_ascii_lowercase();
                    settings.jiggle_solver = match normalized.as_str() {
                        "0" | "kawaii" | "kawaiiphysics" => 0,
                        "1" | "jiggle" | "jigglephysics" | "naelstrof" => 1,
                        _ => settings.jiggle_solver,
                    };
                }),
                archive: Some((183, |state| {
                    let settings = state.video;
                    format!("{}", settings.jiggle_solver)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglesolver(&self) -> String {
                    self.video.jiggle_solver.to_string()
                }
                fn cvar_set_r_jigglesolver(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let normalized = value.trim().to_ascii_lowercase();
                        self.video.jiggle_solver = match normalized.as_str() {
                            "0" | "kawaii" | "kawaiiphysics" => 0,
                            "1" | "jiggle" | "jigglephysics" | "naelstrof" => 1,
                            _ => {
                                return Err(format!(
                                    "{name}: expected 0/KawaiiPhysics or 1/JigglePhysics"
                                ))
                            }
                        };
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLESTIFFNESS {
        runtime: CvarDef::new(
                "r_jiggleStiffness",
                "",
                "0..3",
                "Multiplier for KawaiiPhysics-style pose stiffness.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jigglestiffness(app)),
                |app, value| {
                    App::cvar_set_r_jigglestiffness(
                        app,
                        APP_R_JIGGLESTIFFNESS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_jigglestiffness, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_stiffness = v.clamp(0.0, 3.0);
                    }
                }),
                archive: Some((187, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_stiffness)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglestiffness(&self) -> String {
                    format!("{:.3}", self.video.jiggle_stiffness)
                }
                fn cvar_set_r_jigglestiffness(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jigglestiffness".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JIGGLESTRENGTH {
        runtime: CvarDef::new(
                "r_jiggleStrength",
                "",
                "0..2",
                "Overall soft-tissue displacement multiplier.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_jigglestrength(app)),
                |app, value| {
                    App::cvar_set_r_jigglestrength(app, APP_R_JIGGLESTRENGTH.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_jigglestrength, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.jiggle_strength = v.clamp(0.0, 2.0);
                    }
                }),
                archive: Some((184, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.jiggle_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jigglestrength(&self) -> String {
                    format!("{:.3}", self.video.jiggle_strength)
                }
                fn cvar_set_r_jigglestrength(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_jigglestrength".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                            "r_jigglebreaststrength" => {
                                self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jiggleglutestrength" => {
                                self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                            }
                            "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                            "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                            "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                            "r_jigglejpstiffness" => {
                                self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0)
                            }
                            "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                            "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                            "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                            "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }
}
