//! Hud layout cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_CG_HUDAMMO {
        runtime: CvarDef::new(
                "cg_hudAmmo",
                "",
                "anchor x y scale",
                "HUD ammo panel layout used by HUD Edit Mode.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudammo(app)),
                |app, value| App::cvar_set_cg_hudammo(app, APP_CG_HUDAMMO.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_hudammo, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDAMMO.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((243, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDAMMO.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudammo(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDAMMO.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudammo(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDAMMO.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDCENTERPRINT {
        runtime: CvarDef::new(
                "cg_hudCenterPrint",
                "",
                "anchor x y scale",
                "Center-print text offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudcenterprint(app)),
                |app, value| {
                    App::cvar_set_cg_hudcenterprint(
                        app,
                        APP_CG_HUDCENTERPRINT.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_hudcenterprint, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDCENTERPRINT.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((248, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDCENTERPRINT.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudcenterprint(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDCENTERPRINT.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudcenterprint(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDCENTERPRINT.entry.name)
                            .expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDCHAT {
        runtime: CvarDef::new(
                "cg_hudChat",
                "",
                "anchor x y scale",
                "Chat history and input box offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudchat(app)),
                |app, value| App::cvar_set_cg_hudchat(app, APP_CG_HUDCHAT.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_hudchat, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDCHAT.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((247, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDCHAT.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudchat(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDCHAT.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudchat(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDCHAT.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDCROSSHAIRNAME {
        runtime: CvarDef::new(
                "cg_hudCrosshairName",
                "",
                "anchor x y scale",
                "Crosshair target name offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudcrosshairname(app)),
                |app, value| {
                    App::cvar_set_cg_hudcrosshairname(
                        app,
                        APP_CG_HUDCROSSHAIRNAME.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_hudcrosshairname, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDCROSSHAIRNAME.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((249, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDCROSSHAIRNAME.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudcrosshairname(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDCROSSHAIRNAME.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudcrosshairname(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDCROSSHAIRNAME.entry.name)
                            .expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDFOLLOW {
        runtime: CvarDef::new(
                "cg_hudFollow",
                "",
                "anchor x y scale",
                "Spectator follow name offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudfollow(app)),
                |app, value| {
                    App::cvar_set_cg_hudfollow(app, APP_CG_HUDFOLLOW.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_hudfollow, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDFOLLOW.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((250, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDFOLLOW.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudfollow(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDFOLLOW.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudfollow(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id =
                            HudElementId::from_cvar(APP_CG_HUDFOLLOW.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDFORCE {
        runtime: CvarDef::new(
                "cg_hudForce",
                "",
                "anchor x y scale",
                "HUD Force panel layout used by HUD Edit Mode.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudforce(app)),
                |app, value| App::cvar_set_cg_hudforce(app, APP_CG_HUDFORCE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_hudforce, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDFORCE.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((244, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDFORCE.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudforce(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDFORCE.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudforce(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id =
                            HudElementId::from_cvar(APP_CG_HUDFORCE.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDFPS {
        runtime: CvarDef::new(
                "cg_hudFps",
                "",
                "anchor x y scale",
                "FPS / profiler HUD offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudfps(app)),
                |app, value| App::cvar_set_cg_hudfps(app, APP_CG_HUDFPS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_hudfps, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDFPS.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((246, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDFPS.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudfps(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDFPS.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudfps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDFPS.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDGRIDSIZE {
        runtime: CvarDef::new(
                "cg_hudGridSize",
                "",
                "1..64 pixels",
                "HUD Edit Mode grid spacing in framebuffer pixels.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudgridsize(app)),
                |app, value| {
                    App::cvar_set_cg_hudgridsize(app, APP_CG_HUDGRIDSIZE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_hudgridsize, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.hud_layout.grid_size = finite()
                            .unwrap_or(settings.hud_layout.grid_size)
                            .clamp(1.0, 64.0);
                    }
                }),
                archive: Some((259, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.hud_layout.grid_size)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudgridsize(&self) -> String {
                    format!("{:.3}", self.hud_layout.grid_size)
                }
                fn cvar_set_cg_hudgridsize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.hud_layout.grid_size = Self::parse_cvar_finite(name, value)?.clamp(1.0, 64.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDHEALTH {
        runtime: CvarDef::new(
                "cg_hudHealth",
                "",
                "anchor x y scale",
                "HUD health panel layout used by HUD Edit Mode.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudhealth(app)),
                |app, value| {
                    App::cvar_set_cg_hudhealth(app, APP_CG_HUDHEALTH.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_hudhealth, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDHEALTH.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((241, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDHEALTH.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudhealth(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDHEALTH.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudhealth(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id =
                            HudElementId::from_cvar(APP_CG_HUDHEALTH.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDLAGOMETER {
        runtime: CvarDef::new(
                "cg_hudLagometer",
                "",
                "anchor x y scale",
                "Lagometer graph/numbers offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudlagometer(app)),
                |app, value| {
                    App::cvar_set_cg_hudlagometer(app, APP_CG_HUDLAGOMETER.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_hudlagometer, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDLAGOMETER.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((254, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDLAGOMETER.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudlagometer(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDLAGOMETER.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudlagometer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id =
                            HudElementId::from_cvar(APP_CG_HUDLAGOMETER.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDMOVEMENTKEYS {
        runtime: CvarDef::new(
                "cg_hudMovementKeys",
                "",
                "anchor x y scale",
                "Movement keys HUD offset/scale on top of its stock position (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudmovementkeys(app)),
                |app, value| {
                    App::cvar_set_cg_hudmovementkeys(
                        app,
                        APP_CG_HUDMOVEMENTKEYS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_hudmovementkeys, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDMOVEMENTKEYS.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((245, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDMOVEMENTKEYS.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudmovementkeys(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDMOVEMENTKEYS.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudmovementkeys(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDMOVEMENTKEYS.entry.name)
                            .expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDRACESTART {
        runtime: CvarDef::new(
                "cg_hudRaceStart",
                "",
                "anchor x y scale",
                "Race start-speed readout offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudracestart(app)),
                |app, value| {
                    App::cvar_set_cg_hudracestart(app, APP_CG_HUDRACESTART.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_hudracestart, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDRACESTART.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((253, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDRACESTART.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudracestart(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDRACESTART.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudracestart(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id =
                            HudElementId::from_cvar(APP_CG_HUDRACESTART.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDRACETIMER {
        runtime: CvarDef::new(
                "cg_hudRaceTimer",
                "",
                "anchor x y scale",
                "Race timer block offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudracetimer(app)),
                |app, value| {
                    App::cvar_set_cg_hudracetimer(app, APP_CG_HUDRACETIMER.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_hudracetimer, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDRACETIMER.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((252, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDRACETIMER.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudracetimer(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDRACETIMER.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudracetimer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id =
                            HudElementId::from_cvar(APP_CG_HUDRACETIMER.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDSHIELD {
        runtime: CvarDef::new(
                "cg_hudShield",
                "",
                "anchor x y scale",
                "HUD shield panel layout used by HUD Edit Mode.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudshield(app)),
                |app, value| {
                    App::cvar_set_cg_hudshield(app, APP_CG_HUDSHIELD.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_hudshield, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDSHIELD.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((242, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDSHIELD.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudshield(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDSHIELD.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudshield(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id =
                            HudElementId::from_cvar(APP_CG_HUDSHIELD.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDSNAP {
        runtime: CvarDef::new(
                "cg_hudSnap",
                "",
                "0..1",
                "Snap HUD Edit Mode dragging to the configured grid.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudsnap(app)),
                |app, value| App::cvar_set_cg_hudsnap(app, APP_CG_HUDSNAP.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_hudsnap, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.hud_layout.snap_to_grid =
                            parse_bool(value).unwrap_or(settings.hud_layout.snap_to_grid)
                    }
                }),
                archive: Some((258, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.hud_layout.snap_to_grid))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudsnap(&self) -> String {
                    bool_value(self.hud_layout.snap_to_grid)
                }
                fn cvar_set_cg_hudsnap(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.hud_layout.snap_to_grid = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDSPEEDGRAPH {
        runtime: CvarDef::new(
                "cg_hudSpeedGraph",
                "",
                "anchor x y scale",
                "Speedometer speed graph offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudspeedgraph(app)),
                |app, value| {
                    App::cvar_set_cg_hudspeedgraph(app, APP_CG_HUDSPEEDGRAPH.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_hudspeedgraph, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDSPEEDGRAPH.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((257, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDSPEEDGRAPH.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudspeedgraph(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDSPEEDGRAPH.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudspeedgraph(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDSPEEDGRAPH.entry.name)
                            .expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDSPEEDOMETER {
        runtime: CvarDef::new(
                "cg_hudSpeedometer",
                "",
                "anchor x y scale",
                "Speedometer main readout offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudspeedometer(app)),
                |app, value| {
                    App::cvar_set_cg_hudspeedometer(
                        app,
                        APP_CG_HUDSPEEDOMETER.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_hudspeedometer, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDSPEEDOMETER.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((255, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDSPEEDOMETER.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudspeedometer(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDSPEEDOMETER.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudspeedometer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDSPEEDOMETER.entry.name)
                            .expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDSPEEDOMETERJUMPS {
        runtime: CvarDef::new(
                "cg_hudSpeedometerJumps",
                "",
                "anchor x y scale",
                "Speedometer pre-speed jumps array offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudspeedometerjumps(app)),
                |app, value| {
                    App::cvar_set_cg_hudspeedometerjumps(
                        app,
                        APP_CG_HUDSPEEDOMETERJUMPS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_hudspeedometerjumps, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDSPEEDOMETERJUMPS.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((256, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(
                                HudElementId::from_cvar(APP_CG_HUDSPEEDOMETERJUMPS.entry.name).unwrap()
                            )
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudspeedometerjumps(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDSPEEDOMETERJUMPS.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudspeedometerjumps(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDSPEEDOMETERJUMPS.entry.name)
                            .expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDSURFACEINSPECTOR {
        runtime: CvarDef::new(
                "cg_hudSurfaceInspector",
                "tl 0 0 1",
                "anchor x y scale",
                "Trace/surface inspector panel offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudsurfaceinspector(app)),
                |app, value| {
                    App::cvar_set_cg_hudsurfaceinspector(
                        app,
                        APP_CG_HUDSURFACEINSPECTOR.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_hudsurfaceinspector, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDSURFACEINSPECTOR.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudsurfaceinspector(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDSURFACEINSPECTOR.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudsurfaceinspector(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDSURFACEINSPECTOR.entry.name)
                            .expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HUDVOTE {
        runtime: CvarDef::new(
                "cg_hudVote",
                "",
                "anchor x y scale",
                "Vote text offset/scale (HUD Edit Mode).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_hudvote(app)),
                |app, value| App::cvar_set_cg_hudvote(app, APP_CG_HUDVOTE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_hudvote, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let (Some(id), Some(layout)) = (
                        HudElementId::from_cvar(APP_CG_HUDVOTE.entry.name),
                        HudElementLayout::from_config(value),
                    ) {
                        *settings.hud_layout.element_mut(id) = layout;
                    }
                }),
                archive: Some((251, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        presentation
                            .hud_layout
                            .element(HudElementId::from_cvar(APP_CG_HUDVOTE.entry.name).unwrap())
                            .to_config()
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hudvote(&self) -> String {
                    {
                        HudElementId::from_cvar(APP_CG_HUDVOTE.entry.name)
                            .map(|id| self.hud_layout.element(id).to_config())
                            .unwrap_or_default()
                    }
                }
                fn cvar_set_cg_hudvote(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                            format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                        })?;
                        let id = HudElementId::from_cvar(APP_CG_HUDVOTE.entry.name).expect("guarded above");
                        *self.hud_layout.element_mut(id) = layout;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }
    APP_CG_HUDSTRAFEGUIDE {
        runtime: CvarDef::new("cg_hudStrafeGuide", "tl 0 0 1", "anchor x y scale", "STRAFE GUIDE layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::StrafeGuide).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::StrafeGuide) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::StrafeGuide) = layout; }
            }),
            archive: Some((800, |state| state.presentation.hud_layout.element(HudElementId::StrafeGuide).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDACCELMETER {
        runtime: CvarDef::new("cg_hudAccelMeter", "tl 0 0 1", "anchor x y scale", "ACCELERATION METER layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::AccelMeter).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::AccelMeter) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::AccelMeter) = layout; }
            }),
            archive: Some((801, |state| state.presentation.hud_layout.element(HudElementId::AccelMeter).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDVELOCITYCOMPASS {
        runtime: CvarDef::new("cg_hudVelocityCompass", "tl 0 0 1", "anchor x y scale", "VELOCITY COMPASS layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::VelocityCompass).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::VelocityCompass) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::VelocityCompass) = layout; }
            }),
            archive: Some((802, |state| state.presentation.hud_layout.element(HudElementId::VelocityCompass).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDGROUNDSPEED {
        runtime: CvarDef::new("cg_hudGroundSpeed", "tl 0 0 1", "anchor x y scale", "PRE-SPEED layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::GroundSpeed).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::GroundSpeed) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::GroundSpeed) = layout; }
            }),
            archive: Some((803, |state| state.presentation.hud_layout.element(HudElementId::GroundSpeed).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDJUMPHEIGHT {
        runtime: CvarDef::new("cg_hudJumpHeight", "tl 0 0 1", "anchor x y scale", "JUMP HEIGHT layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::JumpHeight).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::JumpHeight) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::JumpHeight) = layout; }
            }),
            archive: Some((804, |state| state.presentation.hud_layout.element(HudElementId::JumpHeight).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDJUMPDISTANCE {
        runtime: CvarDef::new("cg_hudJumpDistance", "tl 0 0 1", "anchor x y scale", "JUMP DISTANCE layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::JumpDistance).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::JumpDistance) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::JumpDistance) = layout; }
            }),
            archive: Some((805, |state| state.presentation.hud_layout.element(HudElementId::JumpDistance).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDVERTICALSPEED {
        runtime: CvarDef::new("cg_hudVerticalSpeed", "tl 0 0 1", "anchor x y scale", "VERTICAL SPEED layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::VerticalSpeed).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::VerticalSpeed) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::VerticalSpeed) = layout; }
            }),
            archive: Some((806, |state| state.presentation.hud_layout.element(HudElementId::VerticalSpeed).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDYAWSPEED {
        runtime: CvarDef::new("cg_hudYawSpeed", "tl 0 0 1", "anchor x y scale", "YAW SPEED layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::YawSpeed).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::YawSpeed) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::YawSpeed) = layout; }
            }),
            archive: Some((807, |state| state.presentation.hud_layout.element(HudElementId::YawSpeed).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }


    APP_CG_HUDSCORE {
        runtime: CvarDef::new("cg_hudScore", "tl 0 0 1", "anchor x y scale",
            "HUD editor transform for Score; independent of TaystJK visibility cvars.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::Score).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string())
                    .ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::Score) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) {
                    *settings.hud_layout.element_mut(HudElementId::Score) = layout;
                }
            }),
            archive: Some((808, |state| state.presentation.hud_layout.element(HudElementId::Score).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDMINISCORES {
        runtime: CvarDef::new("cg_hudMiniScores", "tl 0 0 1", "anchor x y scale",
            "HUD editor position and scale for cg_drawScores; separate from cg_drawScore.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::MiniScores).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string())
                    .ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::MiniScores) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) {
                    *settings.hud_layout.element_mut(HudElementId::MiniScores) = layout;
                }
            }),
            archive: Some((810, |state| state.presentation.hud_layout.element(HudElementId::MiniScores).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDENEMYINFO {
        runtime: CvarDef::new("cg_hudEnemyInfo", "tl 0 0 1", "anchor x y scale",
            "HUD editor transform for EnemyInfo; independent of TaystJK visibility cvars.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::EnemyInfo).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string())
                    .ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::EnemyInfo) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) {
                    *settings.hud_layout.element_mut(HudElementId::EnemyInfo) = layout;
                }
            }),
            archive: Some((809, |state| state.presentation.hud_layout.element(HudElementId::EnemyInfo).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_HUDKILLFEED {
        runtime: CvarDef::new("cg_hudKillFeed", "tl 0 0 1", "anchor x y scale", "KILL FEED layout (HUD Edit Mode).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.hud_layout.element(HudElementId::KillFeed).to_config()),
            |app, value| {
                let layout = HudElementLayout::from_config(&value.to_string()).ok_or_else(|| "expected anchor x y scale".to_owned())?;
                *app.hud_layout.element_mut(HudElementId::KillFeed) = layout;
                app.mark_config_dirty(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(layout) = HudElementLayout::from_config(value) { *settings.hud_layout.element_mut(HudElementId::KillFeed) = layout; }
            }),
            archive: Some((814, |state| state.presentation.hud_layout.element(HudElementId::KillFeed).to_config())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

}
