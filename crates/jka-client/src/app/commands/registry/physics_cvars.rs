//! Physics cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_R_CLOTHAIRRESISTANCE {
        runtime: CvarDef::new(
                "r_clothAirResistance",
                "",
                "0..4",
                "Cloth airflow strength. Zero disables aerodynamic drag; one uses normal fabric pressure.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_clothairresistance(app)),
                |app, value| {
                    App::cvar_set_r_clothairresistance(
                        app,
                        APP_R_CLOTHAIRRESISTANCE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_clothairresistance, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.cloth_air_resistance = v.clamp(0.0, 4.0);
                    }
                }),
                archive: Some((197, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloth_air_resistance)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_clothairresistance(&self) -> String {
                    self.video.cloth_air_resistance.to_string()
                }
                fn cvar_set_r_clothairresistance(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_clothairresistance".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_clothbodyclearance" => {
                                self.video.cloth_body_clearance = amount.clamp(0.0, 4.0)
                            }
                            "r_clothairresistance" => {
                                self.video.cloth_air_resistance = amount.clamp(0.0, 4.0)
                            }
                            "r_clothturnresponse" => {
                                self.video.cloth_turn_response = amount.clamp(0.0, 4.0)
                            }
                            "r_clothanimationinfluence" => {
                                self.video.cloth_animation_influence = amount.clamp(0.0, 1.0)
                            }
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

    APP_R_CLOTHANIMATIONINFLUENCE {
        runtime: CvarDef::new("r_clothAnimationInfluence", "", "0..1", "Animation influence on free fabric: zero follows sewn attachments; one follows full skeletal animation.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_clothanimationinfluence(app)), |app, value| App::cvar_set_r_clothanimationinfluence(app, APP_R_CLOTHANIMATIONINFLUENCE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_clothanimationinfluence, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.cloth_animation_influence = v.clamp(0.0, 1.0);
                    }
                }),
                archive: Some((199, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloth_animation_influence)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_clothanimationinfluence(&self) -> String {
                    self.video.cloth_animation_influence.to_string()
                }
                fn cvar_set_r_clothanimationinfluence(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_clothanimationinfluence".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_clothbodyclearance" => {
                                self.video.cloth_body_clearance = amount.clamp(0.0, 4.0)
                            }
                            "r_clothairresistance" => {
                                self.video.cloth_air_resistance = amount.clamp(0.0, 4.0)
                            }
                            "r_clothturnresponse" => {
                                self.video.cloth_turn_response = amount.clamp(0.0, 4.0)
                            }
                            "r_clothanimationinfluence" => {
                                self.video.cloth_animation_influence = amount.clamp(0.0, 1.0)
                            }
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

    APP_R_CLOTHBODYCLEARANCE {
        runtime: CvarDef::new(
                "r_clothBodyClearance",
                "",
                "0..4",
                "Additional cloth/body separation in JKA units. Large values inflate the garment.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_clothbodyclearance(app)),
                |app, value| {
                    App::cvar_set_r_clothbodyclearance(
                        app,
                        APP_R_CLOTHBODYCLEARANCE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_clothbodyclearance, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.cloth_body_clearance = v.clamp(0.0, 4.0);
                    }
                }),
                archive: Some((196, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloth_body_clearance)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_clothbodyclearance(&self) -> String {
                    self.video.cloth_body_clearance.to_string()
                }
                fn cvar_set_r_clothbodyclearance(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_clothbodyclearance".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_clothbodyclearance" => {
                                self.video.cloth_body_clearance = amount.clamp(0.0, 4.0)
                            }
                            "r_clothairresistance" => {
                                self.video.cloth_air_resistance = amount.clamp(0.0, 4.0)
                            }
                            "r_clothturnresponse" => {
                                self.video.cloth_turn_response = amount.clamp(0.0, 4.0)
                            }
                            "r_clothanimationinfluence" => {
                                self.video.cloth_animation_influence = amount.clamp(0.0, 1.0)
                            }
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

    APP_R_CLOTHBODYCOLLISION {
        runtime: CvarDef::new(
                "r_clothBodyCollision",
                "",
                "0..1",
                "Collide experimental player cloth with animated head and body capsules.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_clothbodycollision(app)),
                |app, value| {
                    App::cvar_set_r_clothbodycollision(
                        app,
                        APP_R_CLOTHBODYCOLLISION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_clothbodycollision, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.cloth_body_collision =
                        parse_bool(value).unwrap_or(settings.cloth_body_collision);
                }),
                archive: Some((215, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloth_body_collision))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_clothbodycollision(&self) -> String {
                    bool_value(self.video.cloth_body_collision)
                }
                fn cvar_set_r_clothbodycollision(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_clothbodycollision".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOTHPHYSICS {
        runtime: CvarDef::new(
                "r_clothPhysics",
                "",
                "0..1",
                "Experimental: add cloth sway and momentum to authored cape/cloak/robe surfaces.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_clothphysics(app)),
                |app, value| {
                    App::cvar_set_r_clothphysics(app, APP_R_CLOTHPHYSICS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_clothphysics, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.cloth_physics = parse_bool(value).unwrap_or(settings.cloth_physics);
                }),
                archive: Some((214, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloth_physics))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_clothphysics(&self) -> String {
                    bool_value(self.video.cloth_physics)
                }
                fn cvar_set_r_clothphysics(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_clothphysics".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOTHTURNRESPONSE {
        runtime: CvarDef::new(
                "r_clothTurnResponse",
                "",
                "0..4",
                "Cloth turning inertia multiplier. One uses the same response as translation.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_clothturnresponse(app)),
                |app, value| {
                    App::cvar_set_r_clothturnresponse(
                        app,
                        APP_R_CLOTHTURNRESPONSE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_clothturnresponse, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.cloth_turn_response = v.clamp(0.0, 4.0);
                    }
                }),
                archive: Some((198, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloth_turn_response)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_clothturnresponse(&self) -> String {
                    self.video.cloth_turn_response.to_string()
                }
                fn cvar_set_r_clothturnresponse(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_clothturnresponse".to_owned();
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a finite number"))?;
                        if !amount.is_finite() {
                            return Err(format!("{name}: expected a finite number"));
                        }
                        match lower.as_str() {
                            "r_clothbodyclearance" => {
                                self.video.cloth_body_clearance = amount.clamp(0.0, 4.0)
                            }
                            "r_clothairresistance" => {
                                self.video.cloth_air_resistance = amount.clamp(0.0, 4.0)
                            }
                            "r_clothturnresponse" => {
                                self.video.cloth_turn_response = amount.clamp(0.0, 4.0)
                            }
                            "r_clothanimationinfluence" => {
                                self.video.cloth_animation_influence = amount.clamp(0.0, 1.0)
                            }
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

    APP_R_CLOTHWIND {
        runtime: CvarDef::new(
                "r_clothWind",
                "",
                "0..1",
                "Use shared r_weatherWind direction and gusts for cloth.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_clothwind(app)),
                |app, value| App::cvar_set_r_clothwind(app, APP_R_CLOTHWIND.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_clothwind, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.cloth_wind = parse_bool(value).unwrap_or(settings.cloth_wind)
                }),
                archive: Some((200, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloth_wind))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_clothwind(&self) -> String {
                    bool_value(self.video.cloth_wind)
                }
                fn cvar_set_r_clothwind(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_clothwind".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DISMEMBERLIFETIME {
        runtime: CvarDef::new(
                "r_dismemberLifetime",
                "",
                "1..300 seconds",
                "Maximum client Rapier lifetime of a detached limb.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_dismemberlifetime(app)),
                |app, value| {
                    App::cvar_set_r_dismemberlifetime(
                        app,
                        APP_R_DISMEMBERLIFETIME.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_dismemberlifetime, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(value) = value.parse::<f32>().ok().filter(|value| value.is_finite()) {
                        settings.dismember_lifetime = value.clamp(1.0, 300.0);
                    }
                }),
                archive: Some((203, |state| {
                    let settings = state.video;
                    format!("{:.1}", settings.dismember_lifetime)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dismemberlifetime(&self) -> String {
                    format!("{:.1}", self.video.dismember_lifetime)
                }
                fn cvar_set_r_dismemberlifetime(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| "r_dismemberLifetime expects seconds".to_owned())?;
                        if !requested.is_finite() {
                            return Err("r_dismemberLifetime expects a finite number".to_owned());
                        }
                        self.video.dismember_lifetime = requested.clamp(1.0, 300.0);
                        self.physics_menu_changed();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_DISMEMBERMAX {
        runtime: CvarDef::new(
                "r_dismemberMax",
                "",
                "1..128",
                "Maximum detached limb rigid bodies kept by client physics.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_dismembermax(app)),
                |app, value| {
                    App::cvar_set_r_dismembermax(app, APP_R_DISMEMBERMAX.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_dismembermax, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(value) = value.parse::<u32>() {
                        settings.dismember_max = value.clamp(1, 128);
                    }
                }),
                archive: Some((202, |state| {
                    let settings = state.video;
                    format!("{}", settings.dismember_max)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dismembermax(&self) -> String {
                    self.video.dismember_max.to_string()
                }
                fn cvar_set_r_dismembermax(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| "r_dismemberMax expects an integer".to_owned())?;
                        self.video.dismember_max = requested.clamp(1, 128);
                        self.physics_menu_changed();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_PHYSICS {
        runtime: CvarDef::new(
                "r_physics",
                "",
                "0..1",
                "Master switch for client-side visual physics (ragdolls, cloth, jiggle, props).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physics(app)),
                |app, value| App::cvar_set_r_physics(app, APP_R_PHYSICS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_physics, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.client_physics = parse_bool(value).unwrap_or(settings.client_physics);
                }),
                archive: Some((204, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.client_physics))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physics(&self) -> String {
                    bool_value(self.video.client_physics)
                }
                fn cvar_set_r_physics(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_physics".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSCCD {
        runtime: CvarDef::new(
                "r_physicsCCD",
                "",
                "0..1",
                "Enable continuous collision detection for eligible visual physics bodies.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsccd(app)),
                |app, value| {
                    App::cvar_set_r_physicsccd(app, APP_R_PHYSICSCCD.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_physicsccd, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.client_physics_ccd = parse_bool(value).unwrap_or(settings.client_physics_ccd);
                }),
                archive: Some((207, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.client_physics_ccd))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsccd(&self) -> String {
                    bool_value(self.video.client_physics_ccd)
                }
                fn cvar_set_r_physicsccd(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_physicsccd".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSDEBRIS {
        runtime: CvarDef::new(
                "r_physicsDebris",
                "",
                "0..1",
                "Enable rigid-body simulation for client-spawned debris.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsdebris(app)),
                |app, value| {
                    App::cvar_set_r_physicsdebris(app, APP_R_PHYSICSDEBRIS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_physicsdebris, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.physics_debris = parse_bool(value).unwrap_or(settings.physics_debris)
                    }
                }),
                archive: Some((218, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.physics_debris))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsdebris(&self) -> String {
                    bool_value(self.video.physics_debris)
                }
                fn cvar_set_r_physicsdebris(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_physicsdebris".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSDEBRISLIFETIME {
        runtime: CvarDef::new(
                "r_physicsDebrisLifetime",
                "",
                "2|5|10|20|30 seconds",
                "Lifetime of client physics debris.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsdebrislifetime(app)),
                |app, value| {
                    App::cvar_set_r_physicsdebrislifetime(
                        app,
                        APP_R_PHYSICSDEBRISLIFETIME.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_physicsdebrislifetime, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(requested) = value.parse::<u32>() {
                        const VALUES: [u32; 5] = [2, 5, 10, 20, 30];
                        settings.physics_debris_lifetime = *VALUES
                            .iter()
                            .min_by_key(|&&v| v.abs_diff(requested))
                            .unwrap_or(&10) as f32;
                    }
                }),
                archive: Some((220, |state| {
                    let settings = state.video;
                    format!("{:.1}", settings.physics_debris_lifetime)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsdebrislifetime(&self) -> String {
                    format!("{:.1}", self.video.physics_debris_lifetime)
                }
                fn cvar_set_r_physicsdebrislifetime(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let requested = Self::parse_cvar_finite(name, value)
                            .ok()
                            .filter(|seconds| seconds.fract() == 0.0)
                            .map(|seconds| seconds as u32)
                            .ok_or_else(|| format!("{name}: expected 2, 5, 10, 20, or 30"))?;
                        if ![2_u32, 5, 10, 20, 30].contains(&requested) {
                            return Err(format!("{name}: expected 2, 5, 10, 20, or 30"));
                        }
                        self.video.physics_debris_lifetime = requested as f32;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSDEBRISMAX {
        runtime: CvarDef::new(
                "r_physicsDebrisMax",
                "",
                "64|128|192|384|768",
                "Maximum active client debris rigid bodies.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsdebrismax(app)),
                |app, value| {
                    App::cvar_set_r_physicsdebrismax(
                        app,
                        APP_R_PHYSICSDEBRISMAX.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_physicsdebrismax, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(requested) = value.parse::<u32>() {
                        const VALUES: [u32; 5] = [64, 128, 192, 384, 768];
                        settings.physics_debris_max = *VALUES
                            .iter()
                            .min_by_key(|&&v| v.abs_diff(requested))
                            .unwrap_or(&192);
                    }
                }),
                archive: Some((219, |state| {
                    let settings = state.video;
                    format!("{}", settings.physics_debris_max)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsdebrismax(&self) -> String {
                    self.video.physics_debris_max.to_string()
                }
                fn cvar_set_r_physicsdebrismax(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 64, 128, 192, 384, or 768"))?;
                        if ![64_u32, 128, 192, 384, 768].contains(&requested) {
                            return Err(format!("{name}: expected 64, 128, 192, 384, or 768"));
                        }
                        self.video.physics_debris_max = requested;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSDEBUG {
        runtime: CvarDef::new(
                "r_physicsDebug",
                "",
                "0..1",
                "Print Rapier initialization and ragdoll candidate diagnostics.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsdebug(app)),
                |app, value| {
                    App::cvar_set_r_physicsdebug(app, APP_R_PHYSICSDEBUG.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_physicsdebug, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.physics_debug_draw =
                            parse_bool(value).unwrap_or(settings.physics_debug_draw)
                    }
                }),
                archive: Some((225, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.physics_debug_draw))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsdebug(&self) -> String {
                    bool_value(self.video.physics_debug_draw)
                }
                fn cvar_set_r_physicsdebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_physicsdebug".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSEXPLOSIONIMPULSES {
        runtime: CvarDef::new(
                "r_physicsExplosionImpulses",
                "",
                "0..1",
                "Apply observed explosion impulses to client physics bodies.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsexplosionimpulses(app)),
                |app, value| {
                    App::cvar_set_r_physicsexplosionimpulses(
                        app,
                        APP_R_PHYSICSEXPLOSIONIMPULSES.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_physicsexplosionimpulses, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.physics_explosion_impulses =
                            parse_bool(value).unwrap_or(settings.physics_explosion_impulses)
                    }
                }),
                archive: Some((223, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.physics_explosion_impulses))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsexplosionimpulses(&self) -> String {
                    bool_value(self.video.physics_explosion_impulses)
                }
                fn cvar_set_r_physicsexplosionimpulses(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_physicsexplosionimpulses".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSFORCEIMPULSES {
        runtime: CvarDef::new(
                "r_physicsForceImpulses",
                "",
                "0..1",
                "Apply observable Force-power impulses to client physics bodies.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsforceimpulses(app)),
                |app, value| {
                    App::cvar_set_r_physicsforceimpulses(
                        app,
                        APP_R_PHYSICSFORCEIMPULSES.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_physicsforceimpulses, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.physics_force_impulses =
                            parse_bool(value).unwrap_or(settings.physics_force_impulses)
                    }
                }),
                archive: Some((224, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.physics_force_impulses))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsforceimpulses(&self) -> String {
                    bool_value(self.video.physics_force_impulses)
                }
                fn cvar_set_r_physicsforceimpulses(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_physicsforceimpulses".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSHZ {
        runtime: CvarDef::new(
                "r_physicsHz",
                "",
                "30|60|120|240",
                "Fixed timestep for client-side visual physics.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicshz(app)),
                |app, value| App::cvar_set_r_physicshz(app, APP_R_PHYSICSHZ.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_physicshz, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(requested) = value.parse::<u32>() {
                        const VALUES: [u32; 4] = [30, 60, 120, 240];
                        settings.client_physics_hz = *VALUES
                            .iter()
                            .min_by_key(|&&candidate| candidate.abs_diff(requested))
                            .unwrap_or(&60);
                    }
                }),
                archive: Some((205, |state| {
                    let settings = state.video;
                    format!("{}", settings.client_physics_hz)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicshz(&self) -> String {
                    self.video.client_physics_hz.to_string()
                }
                fn cvar_set_r_physicshz(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 30, 60, 120, or 240"))?;
                        if ![30_u32, 60, 120, 240].contains(&requested) {
                            return Err(format!("{name}: expected 30, 60, 120, or 240"));
                        }
                        self.video.client_physics_hz = requested;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSMAXSUBSTEPS {
        runtime: CvarDef::new(
                "r_physicsMaxSubsteps",
                "",
                "1|2|4|8",
                "Maximum visual-physics catch-up steps after a slow frame.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsmaxsubsteps(app)),
                |app, value| {
                    App::cvar_set_r_physicsmaxsubsteps(
                        app,
                        APP_R_PHYSICSMAXSUBSTEPS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_physicsmaxsubsteps, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(requested) = value.parse::<u32>() {
                        const VALUES: [u32; 4] = [1, 2, 4, 8];
                        settings.client_physics_max_substeps = *VALUES
                            .iter()
                            .min_by_key(|&&candidate| candidate.abs_diff(requested))
                            .unwrap_or(&4);
                    }
                }),
                archive: Some((206, |state| {
                    let settings = state.video;
                    format!("{}", settings.client_physics_max_substeps)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsmaxsubsteps(&self) -> String {
                    self.video.client_physics_max_substeps.to_string()
                }
                fn cvar_set_r_physicsmaxsubsteps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 1, 2, 4, or 8"))?;
                        if ![1_u32, 2, 4, 8].contains(&requested) {
                            return Err(format!("{name}: expected 1, 2, 4, or 8"));
                        }
                        self.video.client_physics_max_substeps = requested;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSPLAYERPUSH {
        runtime: CvarDef::new(
                "r_physicsPlayerPush",
                "",
                "0..1",
                "Allow player capsules to push visual props one-way.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsplayerpush(app)),
                |app, value| {
                    App::cvar_set_r_physicsplayerpush(
                        app,
                        APP_R_PHYSICSPLAYERPUSH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_physicsplayerpush, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.physics_player_push =
                            parse_bool(value).unwrap_or(settings.physics_player_push)
                    }
                }),
                archive: Some((221, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.physics_player_push))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsplayerpush(&self) -> String {
                    bool_value(self.video.physics_player_push)
                }
                fn cvar_set_r_physicsplayerpush(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_physicsplayerpush".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSPROPMAX {
        runtime: CvarDef::new(
                "r_physicsPropMax",
                "",
                "32|64|96|192|384",
                "Maximum active client physics props.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicspropmax(app)),
                |app, value| {
                    App::cvar_set_r_physicspropmax(app, APP_R_PHYSICSPROPMAX.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_physicspropmax, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(requested) = value.parse::<u32>() {
                        const VALUES: [u32; 5] = [32, 64, 96, 192, 384];
                        settings.physics_prop_max = *VALUES
                            .iter()
                            .min_by_key(|&&v| v.abs_diff(requested))
                            .unwrap_or(&96);
                    }
                }),
                archive: Some((217, |state| {
                    let settings = state.video;
                    format!("{}", settings.physics_prop_max)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicspropmax(&self) -> String {
                    self.video.physics_prop_max.to_string()
                }
                fn cvar_set_r_physicspropmax(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 32, 64, 96, 192, or 384"))?;
                        if ![32_u32, 64, 96, 192, 384].contains(&requested) {
                            return Err(format!("{name}: expected 32, 64, 96, 192, or 384"));
                        }
                        self.video.physics_prop_max = requested;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSPROPS {
        runtime: CvarDef::new(
                "r_physicsProps",
                "",
                "0..1",
                "Enable client-only dynamic visual props.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsprops(app)),
                |app, value| {
                    App::cvar_set_r_physicsprops(app, APP_R_PHYSICSPROPS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_physicsprops, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.physics_props = parse_bool(value).unwrap_or(settings.physics_props)
                    }
                }),
                archive: Some((216, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.physics_props))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsprops(&self) -> String {
                    bool_value(self.video.physics_props)
                }
                fn cvar_set_r_physicsprops(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_physicsprops".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSSLEEPING {
        runtime: CvarDef::new(
                "r_physicsSleeping",
                "",
                "0..1",
                "Allow inactive Rapier bodies to sleep.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicssleeping(app)),
                |app, value| {
                    App::cvar_set_r_physicssleeping(
                        app,
                        APP_R_PHYSICSSLEEPING.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_physicssleeping, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.client_physics_sleeping =
                        parse_bool(value).unwrap_or(settings.client_physics_sleeping);
                }),
                archive: Some((208, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.client_physics_sleeping))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicssleeping(&self) -> String {
                    bool_value(self.video.client_physics_sleeping)
                }
                fn cvar_set_r_physicssleeping(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_physicssleeping".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSSTATS {
        runtime: CvarDef::new(
                "r_physicsStats",
                "",
                "0..1",
                "Print client physics counters and fixed-step state once per second.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsstats(app)),
                |app, value| {
                    App::cvar_set_r_physicsstats(app, APP_R_PHYSICSSTATS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_physicsstats, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.physics_stats = parse_bool(value).unwrap_or(settings.physics_stats)
                    }
                }),
                archive: Some((226, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.physics_stats))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsstats(&self) -> String {
                    bool_value(self.video.physics_stats)
                }
                fn cvar_set_r_physicsstats(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_physicsstats".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PHYSICSWEAPONIMPULSES {
        runtime: CvarDef::new(
                "r_physicsWeaponImpulses",
                "",
                "0..1",
                "Projectile impact ragdolls (server knockback flag required locally; no saber hits).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_physicsweaponimpulses(app)),
                |app, value| {
                    App::cvar_set_r_physicsweaponimpulses(
                        app,
                        APP_R_PHYSICSWEAPONIMPULSES.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_physicsweaponimpulses, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.physics_weapon_impulses =
                            parse_bool(value).unwrap_or(settings.physics_weapon_impulses)
                    }
                }),
                archive: Some((222, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.physics_weapon_impulses))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_physicsweaponimpulses(&self) -> String {
                    bool_value(self.video.physics_weapon_impulses)
                }
                fn cvar_set_r_physicsweaponimpulses(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_physicsweaponimpulses".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_RAGDOLLLIFETIME {
        runtime: CvarDef::new(
                "r_ragdollLifetime",
                "",
                "5|10|20|30|60 seconds",
                "Lifetime of simulated client ragdolls.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ragdolllifetime(app)),
                |app, value| {
                    App::cvar_set_r_ragdolllifetime(
                        app,
                        APP_R_RAGDOLLLIFETIME.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_ragdolllifetime, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(requested) = value.parse::<u32>() {
                        const VALUES: [u32; 5] = [5, 10, 20, 30, 60];
                        settings.ragdoll_lifetime = *VALUES
                            .iter()
                            .min_by_key(|&&v| v.abs_diff(requested))
                            .unwrap_or(&20) as f32;
                    }
                }),
                archive: Some((211, |state| {
                    let settings = state.video;
                    format!("{:.1}", settings.ragdoll_lifetime)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ragdolllifetime(&self) -> String {
                    format!("{:.1}", self.video.ragdoll_lifetime)
                }
                fn cvar_set_r_ragdolllifetime(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        // Written as "20.0" by the config, so accept whole-valued floats.
                        let requested = Self::parse_cvar_finite(name, value)
                            .ok()
                            .filter(|seconds| seconds.fract() == 0.0)
                            .map(|seconds| seconds as u32)
                            .ok_or_else(|| format!("{name}: expected 5, 10, 20, 30, or 60"))?;
                        if ![5_u32, 10, 20, 30, 60].contains(&requested) {
                            return Err(format!("{name}: expected 5, 10, 20, 30, or 60"));
                        }
                        self.video.ragdoll_lifetime = requested as f32;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_RAGDOLLMAX {
        runtime: CvarDef::new(
                "r_ragdollMax",
                "",
                "2|4|8|16|32",
                "Maximum active client ragdolls.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ragdollmax(app)),
                |app, value| {
                    App::cvar_set_r_ragdollmax(app, APP_R_RAGDOLLMAX.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_ragdollmax, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(requested) = value.parse::<u32>() {
                        const VALUES: [u32; 5] = [2, 4, 8, 16, 32];
                        settings.ragdoll_max = *VALUES
                            .iter()
                            .min_by_key(|&&v| v.abs_diff(requested))
                            .unwrap_or(&8);
                    }
                }),
                archive: Some((210, |state| {
                    let settings = state.video;
                    format!("{}", settings.ragdoll_max)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ragdollmax(&self) -> String {
                    self.video.ragdoll_max.to_string()
                }
                fn cvar_set_r_ragdollmax(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 2, 4, 8, 16, or 32"))?;
                        if ![2_u32, 4, 8, 16, 32].contains(&requested) {
                            return Err(format!("{name}: expected 2, 4, 8, 16, or 32"));
                        }
                        self.video.ragdoll_max = requested;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_RAGDOLLS {
        runtime: CvarDef::new(
                "r_ragdolls",
                "",
                "0..1",
                "Enable client-side visual ragdolls.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ragdolls(app)),
                |app, value| App::cvar_set_r_ragdolls(app, APP_R_RAGDOLLS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_ragdolls, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ragdolls = parse_bool(value).unwrap_or(settings.ragdolls)
                }),
                archive: Some((209, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ragdolls))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ragdolls(&self) -> String {
                    bool_value(self.video.ragdolls)
                }
                fn cvar_set_r_ragdolls(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_ragdolls".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_RAGDOLLJOINTMODEL {
        runtime: CvarDef::new(
            "r_ragdollJointModel", "0", "0..1",
            "Ragdoll joints: 0 classic hard limits; 1 experimental bounded soft limits + bind-informed neutral tension. Switching modes re-seeds simulated ragdolls.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(App::cvar_get_r_ragdolljointmodel(app)),
            |app, value| App::cvar_set_r_ragdolljointmodel(
                app, APP_R_RAGDOLLJOINTMODEL.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_ragdolljointmodel, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    if mode <= 1 { settings.ragdoll_joint_model = mode; }
                }
            }),
            archive: Some((36374, |state| state.video.ragdoll_joint_model.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_r_ragdolljointmodel(&self) -> String {
                    self.video.ragdoll_joint_model.to_string()
                }
                fn cvar_set_r_ragdolljointmodel(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = value.trim().parse::<u8>()
                        .map_err(|_| format!("{name}: expected 0 or 1"))?;
                    if mode > 1 { return Err(format!("{name}: expected 0 or 1")); }
                    if self.video.ragdoll_joint_model == mode { return Ok(()); }
                    self.video.ragdoll_joint_model = mode;
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_RAGDOLLSELFCOLLISION {
        runtime: CvarDef::new(
                "r_ragdollSelfCollision",
                "",
                "0..1",
                "Allow limbs on one ragdoll to collide with each other.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ragdollselfcollision(app)),
                |app, value| {
                    App::cvar_set_r_ragdollselfcollision(
                        app,
                        APP_R_RAGDOLLSELFCOLLISION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_ragdollselfcollision, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ragdoll_self_collision =
                        parse_bool(value).unwrap_or(settings.ragdoll_self_collision);
                }),
                archive: Some((212, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ragdoll_self_collision))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ragdollselfcollision(&self) -> String {
                    bool_value(self.video.ragdoll_self_collision)
                }
                fn cvar_set_r_ragdollselfcollision(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_ragdollselfcollision".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_physics" => self.video.client_physics = enabled,
                            "r_physicsccd" => self.video.client_physics_ccd = enabled,
                            "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                            "r_ragdolls" => self.video.ragdolls = enabled,
                            "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                            "r_clothphysics" => self.video.cloth_physics = enabled,
                            "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                            "r_clothwind" => self.video.cloth_wind = enabled,
                            "r_physicsprops" => self.video.physics_props = enabled,
                            "r_physicsdebris" => self.video.physics_debris = enabled,
                            "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                            "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                            "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                            "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                            "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                            "r_physicsstats" => self.video.physics_stats = enabled,
                            _ => unreachable!(),
                        }
                        self.mark_config_dirty();
                        if lower == "r_physics" {
                            self.ensure_map_physics_mesh();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }
}
