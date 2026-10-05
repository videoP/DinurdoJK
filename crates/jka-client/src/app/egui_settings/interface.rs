//! Interface.
use crate::app::egui_settings::{
    draw_crosshair_glyph, percent, segmented_row, theme, ui_catalog, App, OverlayMode,
};

impl App {
    pub(in crate::app) fn egui_interface_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "INTERFACE / HUD",
            "HUD presentation. This is the home for crosshair, movement helpers and the future drag/snap HUD editor.",
        );

        egui::ScrollArea::vertical()
            .id_salt("jka_interface_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::section(ui, "HUD", "Heads-up display controls.");
                theme::row(
                    ui,
                    "HUD layout",
                    "Open HUD Edit Mode. Drag Health, Shield, Ammo and Force directly over the game; positions/scales are cached as typed runtime data and archived through cg_hud* cvars.",
                    theme::Reset::None,
                    |ui| {
                        let response = ui.add_enabled(!self.front_end, egui::Button::new("EDIT HUD"));
                        if response.clicked() {
                            self.hud_edit_selected = None;
                            self.hud_edit_drag_origin = None;
                            self.hud_edit_drag_delta = [0.0, 0.0];
                            self.hud_edit_chat_resize_origin = None;
                            self.hud_edit_chat_resize_corner = None;
                            self.set_overlay(OverlayMode::HudEdit);
                        }
                        if self.front_end {
                            response.on_hover_text("Enter a game before editing the HUD.");
                        } else {
                            response.on_hover_text("Also available as /hudedit.");
                        }
                    },
                );
                self.egui_japro_hud_settings(ui);

                theme::section(
                    ui,
                    "CROSSHAIR",
                    "Click a crosshair to use it; click it again to turn the crosshair off.",
                );

                if self.strafe_helper.flags & crate::ui::SHELPER_CROSSHAIR != 0 {
                    theme::banner(
                        ui,
                        "cg_strafeHelper bit 16384 is set: jaPRO's line crosshair replaces these settings.",
                        theme::WARNING,
                    );
                }

                self.draw_crosshair_picker(ui);

                if self.crosshair.style == crate::ui::CROSSHAIR_STYLE_LINE && self.crosshair.image == 0 {
                    theme::row(
                        ui,
                        "Line width",
                        "cg_strafeHelperLineWidth, in 640x480 units. jaPRO draws its line crosshair with the strafehelper line width, so this also sets the strafehelper lines. Size stretches the line's length.",
                        theme::Reset::None,
                        |ui| {
                            let mut value = self.strafe_helper.line_width;
                            let readout = format!("{value:.2}");
                            if theme::slider(ui, &mut value, 0.25..=5.0, &readout) {
                                let _ = self.set_console_cvar("cg_strafeHelperLineWidth", &value.to_string());
                            }
                        },
                    );
                }

                theme::row(
                    ui,
                    "Size",
                    "cg_crosshairSize. Uses JKA's stock default of 24. Shapes and images are sized in pixels, compensating for the transparent padding in the original artwork; the line stretches with it.",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.crosshair.size;
                        let readout = format!("{value:.0}");
                        if theme::slider(ui, &mut value, 4.0..=96.0, &readout) {
                            let _ = self.set_console_cvar("cg_crosshairSize", &value.to_string());
                        }
                    },
                );
                theme::row(
                    ui,
                    "Strength",
                    "cg_crosshairStrength. 100% is the crosshair as authored. Below that it fades; above it the faint stock image crosshairs are drawn stronger (shapes are already solid, so only fade).",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.crosshair.strength;
                        let readout = percent(value);
                        if theme::slider(ui, &mut value, 0.0..=crate::ui::CROSSHAIR_STRENGTH_MAX, &readout) {
                            let _ = self.set_console_cvar("cg_crosshairStrength", &value.to_string());
                        }
                    },
                );

                theme::row(
                    ui,
                    "Color",
                    "cg_crosshairColor. RGB picker; the archived cvar keeps TaystJK's R G B A 0..255 format.",
                    theme::Reset::None,
                    |ui| {
                        let mut rgb = [
                            f32::from(self.crosshair.color[0]) / 255.0,
                            f32::from(self.crosshair.color[1]) / 255.0,
                            f32::from(self.crosshair.color[2]) / 255.0,
                        ];
                        if ui.color_edit_button_rgb(&mut rgb).changed() {
                            let to_u8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                            let [_, _, _, alpha] = self.crosshair.color;
                            let color = [to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), alpha];
                            let value = format!(
                                "{} {} {} {}",
                                color[0], color[1], color[2], color[3]
                            );
                            let _ = self.set_console_cvar("cg_crosshairColor", &value);
                        }
                        ui.add_space(10.0);
                        theme::glow_label(
                            ui,
                            &format!(
                                "{}  {}  {}",
                                self.crosshair.color[0],
                                self.crosshair.color[1],
                                self.crosshair.color[2]
                            ),
                            12.5,
                            theme::TEXT_FAINT,
                        );
                    },
                );
                const DYNAMIC_CROSSHAIR: [(u8, &str); 3] = [(0, "Off"), (1, "Always"), (2, "Smart")];
                if let Some(mode) = segmented_row(
                    ui,
                    "Dynamic crosshair",
                    "TaystJK cg_dynamicCrosshair. Off keeps the crosshair at screen center. Always traces from the weapon/player muzzle. Smart uses TaystJK's static overrides for saber/melee, race mode and the applicable strafehelper modes.",
                    theme::Reset::None,
                    self.crosshair.dynamic,
                    &DYNAMIC_CROSSHAIR,
                ) {
                    let _ = self.set_console_cvar("cg_dynamicCrosshair", &mode.to_string());
                }

                theme::row(
                    ui,
                    "Color by target",
                    "cg_crosshairIdentifyTarget. jaPRO: the crosshair turns red on enemies, green on teammates, yellow on neutral objects and grey on other duelists. Off keeps the color above.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(enabled) = theme::switch(ui, self.crosshair.identify_target) {
                            let _ = self.set_console_cvar("cg_crosshairIdentifyTarget", if enabled { "1" } else { "0" });
                        }
                    },
                );
                const NAMES: [(f32, &str); 5] =
                    [(0.0, "Off"), (-1.0, "While aimed"), (1.0, "1 s"), (3.0, "3 s"), (5.0, "5 s")];
                if let Some(names) = segmented_row(
                    ui,
                    "Player names",
                    "cg_drawCrosshairNames. Off; only while the crosshair is on a player; or for that many seconds after. The console takes any value (negative = while aimed).",
                    theme::Reset::None,
                    self.crosshair.names,
                    &NAMES,
                ) {
                    let _ = self.set_console_cvar("cg_drawCrosshairNames", &names.to_string());
                }
                theme::row(
                    ui,
                    "Name colors",
                    "cg_drawCrosshairNamesColours. On draws the name with its own color codes; off strips them and colors it red or green by friend or foe.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(enabled) = theme::switch(ui, self.crosshair.names_colours) {
                            let _ = self.set_console_cvar("cg_drawCrosshairNamesColours", if enabled { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Name opacity",
                    "cg_drawCrosshairNamesOpacity.",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.crosshair.names_opacity;
                        let readout = format!("{:.0}%", value * 100.0);
                        if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                            let _ = self.set_console_cvar("cg_drawCrosshairNamesOpacity", &value.to_string());
                        }
                    },
                );

                theme::section(
                    ui,
                    "MOVEMENT KEYS",
                    "TaystJK-compatible cg_movementKeys overlay controls. Position is handled by HUD Edit Mode.",
                );
                const MOVEMENT_KEY_MODES: [(u8, &str); 5] = [
                    (0, "Off"), (1, "Original"), (2, "+ Attack"), (3, "Compact"), (4, "Movable"),
                ];
                if let Some(mode) = segmented_row(
                    ui, "Mode", "cg_movementKeys. Uses TaystJK's 0..4 display modes.",
                    theme::Reset::None, self.movement_keys_hud.mode, &MOVEMENT_KEY_MODES,
                ) {
                    let _ = self.set_console_cvar("cg_movementKeys", &mode.to_string());
                }
                theme::row(ui, "Walk key", "cg_movementKeysWalk. Include the walk/run state in the overlay.", theme::Reset::None, |ui| {
                    if let Some(value) = theme::switch(ui, self.movement_keys_hud.walk) {
                        let _ = self.set_console_cvar("cg_movementKeysWalk", if value { "1" } else { "0" });
                    }
                });

                theme::section(
                    ui,
                    "STRAFEHELPER",
                    "TaystJK CGAZ/Strafehelper controls. Direction bits remain available through cg_strafeHelper.",
                );
                const STRAFE_STYLES: [(u8, &str); 5] = [
                    (0, "Off"), (1, "Original"), (2, "Updated"), (3, "CGAZ"), (4, "Cinematic"),
                ];
                let style = if self.strafe_helper.flags & crate::ui::SHELPER_CINEMATIC != 0 { 4 }
                    else if self.strafe_helper.flags & crate::ui::SHELPER_CGAZ != 0 { 3 }
                    else if self.strafe_helper.flags & crate::ui::SHELPER_UPDATED != 0 { 2 }
                    else if self.strafe_helper.flags & crate::ui::SHELPER_ORIGINAL != 0 { 1 }
                    else { 0 };
                if let Some(selected) = segmented_row(
                    ui, "Style", "Visual style inside cg_strafeHelper. Cinematic keeps TaystJK strafe math but presents it as depth-tested glowing lines in 3D world space.",
                    theme::Reset::None, style, &STRAFE_STYLES,
                ) {
                    let style_bit = match selected {
                        1 => crate::ui::SHELPER_ORIGINAL,
                        2 => crate::ui::SHELPER_UPDATED,
                        3 => crate::ui::SHELPER_CGAZ,
                        4 => crate::ui::SHELPER_CINEMATIC,
                        _ => 0,
                    };
                    let flags = (self.strafe_helper.flags & !crate::ui::SHELPER_STYLE_MASK) | style_bit;
                    let _ = self.set_console_cvar("cg_strafeHelper", &flags.to_string());
                }
                theme::row(ui, "Directions", "Direction bits inside cg_strafeHelper. Defaults match TaystJK: WA, WD, A, D and Center.", theme::Reset::None, |ui| {
                    let mut flags = self.strafe_helper.flags;
                    for (bit, label) in [
                        (crate::ui::SHELPER_W, "W"),
                        (crate::ui::SHELPER_WA, "WA"),
                        (crate::ui::SHELPER_WD, "WD"),
                        (crate::ui::SHELPER_A, "A"),
                        (crate::ui::SHELPER_D, "D"),
                        (crate::ui::SHELPER_CENTER, "Center"),
                    ] {
                        let enabled = flags & bit != 0;
                        if theme::chip(ui, label, enabled).clicked() { flags ^= bit; }
                        ui.add_space(3.0);
                    }
                    if flags != self.strafe_helper.flags {
                        let _ = self.set_console_cvar("cg_strafeHelper", &flags.to_string());
                    }
                });
                theme::row(ui, "Offset", "cg_strafeHelperOffset. TaystJK stores hundredths of a degree; default 75 = 0.75 degrees.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.offset;
                    let readout = format!("{:.2}°", value * 0.01);
                    if theme::slider(ui, &mut value, -500.0..=500.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperOffset", &value.to_string());
                    }
                });
                theme::row(ui, "Line width", "cg_strafeHelperLineWidth.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.line_width;
                    let readout = format!("{value:.2}");
                    if theme::slider(ui, &mut value, 0.25..=5.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperLineWidth", &value.to_string());
                    }
                });
                theme::row(ui, "Precision", "cg_strafeHelperPrecision. TaystJK uses this as the world-space distance before projecting a guide. Cinematic keeps that distance directly as its 3D line length.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.precision as f32;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, 100.0..=10000.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperPrecision", &(value.round() as u32).to_string());
                    }
                });
                theme::row(ui, "Physics FPS", "cg_strafeHelper_FPS. Zero follows com_maxfps like TaystJK; uncapped falls back to 125.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.fps;
                    let readout = if value < 1.0 { "Auto (125)".to_owned() } else { format!("{value:.0}") };
                    if theme::slider(ui, &mut value, 0.0..=1000.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelper_FPS", &value.to_string());
                    }
                });
                theme::row(ui, "Cutoff", "cg_strafeHelperCutoff. Controls 2D style clipping; Cinematic uses Precision for its 3D length.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.cutoff;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, 0.0..=480.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperCutoff", &value.to_string());
                    }
                });
                theme::row(ui, "Inactive alpha", "cg_strafeHelperInactiveAlpha.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.inactive_alpha as f32;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, 0.0..=255.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperInactiveAlpha", &(value.round() as u8).to_string());
                    }
                });
                theme::row(ui, "Active color", "cg_strafeHelperActiveColor.", theme::Reset::None, |ui| {
                    let mut rgb = [
                        f32::from(self.strafe_helper.active_color[0]) / 255.0,
                        f32::from(self.strafe_helper.active_color[1]) / 255.0,
                        f32::from(self.strafe_helper.active_color[2]) / 255.0,
                    ];
                    if ui.color_edit_button_rgb(&mut rgb).changed() {
                        let to_u8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                        let a = self.strafe_helper.active_color[3];
                        let value = format!("{} {} {} {}", to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), a);
                        let _ = self.set_console_cvar("cg_strafeHelperActiveColor", &value);
                    }
                });

                theme::section(ui, "CHAT", "The say / say_team input line.");
                theme::row(
                    ui,
                    "Name completion",
                    "cg_chatboxCompletion. Tab completes the current word to a player name (colours ignored). Repeated Tab cycles matches, preferring exact names and names that start with what you typed.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.chatbox_completion) {
                            let _ = self.set_console_cvar("cg_chatboxCompletion", if value { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Chat logging",
                    "cl_chatLog. Saves live server chat as a self-contained HTML session under the active fs_game/chatlogs directory. A dedicated writer thread batches user-space flushes for up to 10 seconds; demo playback is not logged.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.chat_log_enabled) {
                            let _ = self.set_console_cvar("cl_chatLog", if value { "1" } else { "0" });
                        }
                    },
                );

                theme::section(ui, "CONSOLE", "The ` console. Also switchable from the SUGGEST chip in the console header.");
                theme::row(
                    ui,
                    "Live suggestions",
                    "con_suggest. Filter commands and cvars as you type: Up/Down pick, Tab completes, Esc hides the list. Off keeps the classic Tab-lists-matches behaviour.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.console_suggest) {
                            let _ = self.set_console_cvar("con_suggest", if value { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Timestamps",
                    "con_timestamps. Prefix console lines with local HH:MM:SS.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.console_timestamps) {
                            let _ = self.set_console_cvar("con_timestamps", if value { "1" } else { "0" });
                        }
                    },
                );
            });
    }
}

// ---------------------------------------------------------------- resets --

impl App {
    pub(in crate::app::egui_settings) fn reset_ocean_cascade(&mut self, cascade: usize, field: u8) {
        if cascade >= crate::ocean::OCEAN_CASCADES {
            return;
        }
        let defaults = crate::ocean::OceanSettings::default();
        let source = defaults.cascades[cascade];
        let target = &mut self.video.ocean_settings.cascades[cascade];
        match field {
            0 => target.tile_length[0] = source.tile_length[0],
            1 => target.tile_length[1] = source.tile_length[1],
            2 => target.displacement_scale = source.displacement_scale,
            3 => target.normal_scale = source.normal_scale,
            4 => target.wind_speed = source.wind_speed,
            5 => target.wind_direction = source.wind_direction,
            6 => target.fetch_length = source.fetch_length,
            7 => target.swell = source.swell,
            8 => target.spread = source.spread,
            9 => target.detail = source.detail,
            10 => target.whitecap = source.whitecap,
            11 => target.foam_amount = source.foam_amount,
            _ => return,
        }
        self.commit_ocean_settings();
    }
}

impl App {
    pub(in crate::app) fn crosshair_image_key(index: u8) -> String {
        format!("crosshair-image:{index}")
    }

    /// Thumbnail of image crosshair `index` (1..=10). The first call for an image
    /// asks the catalog worker to read it off-thread; it shows up on a later frame.
    pub(in crate::app::egui_settings) fn crosshair_image_texture(
        &mut self,
        index: u8,
    ) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.crosshair_image_textures.get(&index) {
            return Some(texture.clone());
        }
        let key = Self::crosshair_image_key(index);
        if self.ui_catalog.icons_missing.contains(&key)
            || self.ui_catalog.icons_inflight.contains(&key)
        {
            return None;
        }
        self.ui_catalog.icons_inflight.insert(key);
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::CrosshairImage { index },
        );
        None
    }

    /// One row of tiles: the stock `gfx/2d/crosshair{a..j}` images, then the
    /// built-in shapes. Each tile draws the crosshair itself in the current color,
    /// so there is no preview pane and no labels; hover names a tile. Clicking the
    /// selected tile turns the crosshair off.
    pub(in crate::app::egui_settings) fn draw_crosshair_picker(&mut self, ui: &mut egui::Ui) {
        #[derive(Clone, Copy, PartialEq)]
        enum Pick {
            Off,
            Image(u8),
            Shape(u8),
        }
        // Box (6) stays an accepted cg_drawCrosshair value but is not offered, so
        // every tile fits on one row.
        const SHAPES: [(u8, &str); 6] = [
            (1, "Classic"),
            (2, "Dot"),
            (3, "Plus"),
            (4, "Plus with dot"),
            (5, "Brackets"),
            (crate::ui::CROSSHAIR_STYLE_LINE, "Line"),
        ];
        const GAP: f32 = 6.0;

        let style = self.crosshair.style;
        let image = self.crosshair.image;
        let current = if style == 0 {
            Pick::Off
        } else if image != 0 {
            Pick::Image(image)
        } else {
            Pick::Shape(style)
        };

        // J only ships in japro-assets.pk3; drop tiles for art that is not there.
        let mut tiles = Vec::new();
        for index in 1..=crate::ui::CROSSHAIR_IMAGE_COUNT {
            if !self
                .ui_catalog
                .icons_missing
                .contains(&Self::crosshair_image_key(index))
            {
                tiles.push((
                    Pick::Image(index),
                    format!("Stock crosshair {}", char::from(b'A' + index - 1)),
                ));
            }
        }
        tiles.extend(
            SHAPES
                .iter()
                .map(|&(id, name)| (Pick::Shape(id), name.to_owned())),
        );

        let count = tiles.len() as f32;
        let tile = ((ui.available_width() - GAP * (count - 1.0)) / count).clamp(34.0, 52.0);
        let mut picked = None;
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
            for (pick, name) in &tiles {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(tile, tile), egui::Sense::click());
                let selected = *pick == current;
                ui.painter()
                    .rect_filled(rect, egui::CornerRadius::ZERO, theme::INSET);
                let (stroke_width, stroke_color) = if selected {
                    (2.0_f32, theme::ACCENT)
                } else if response.hovered() {
                    (1.0_f32, theme::TEXT_FAINT)
                } else {
                    (1.0_f32, theme::LINE)
                };
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::ZERO,
                    egui::Stroke::new(stroke_width, stroke_color),
                    egui::StrokeKind::Inside,
                );
                match *pick {
                    Pick::Off => {}
                    Pick::Image(index) => {
                        let texture = self.crosshair_image_texture(index);
                        let sample = crate::ui::CrosshairSettings {
                            style: style.max(1),
                            image: index,
                            size: 40.0,
                            ..self.crosshair
                        };
                        draw_crosshair_glyph(
                            &ui.painter().clone(),
                            rect.center(),
                            tile - 6.0,
                            sample,
                            texture.as_ref(),
                            self.strafe_helper.line_width,
                        );
                    }
                    Pick::Shape(id) => {
                        let sample = crate::ui::CrosshairSettings {
                            style: id,
                            image: 0,
                            size: 40.0,
                            ..self.crosshair
                        };
                        draw_crosshair_glyph(
                            &ui.painter().clone(),
                            rect.center(),
                            tile - 6.0,
                            sample,
                            None,
                            self.strafe_helper.line_width,
                        );
                    }
                }
                let response = response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(name.as_str());
                if response.clicked() {
                    // Clicking the active crosshair turns it off.
                    picked = Some(if selected { Pick::Off } else { *pick });
                }
            }
        });
        ui.add_space(4.0);

        let Some(pick) = picked else { return };
        // cg_drawCrosshair stays the on/off switch in every mode, so picking
        // anything while Off turns it back on.
        let (next_style, next_image) = match pick {
            Pick::Off => (0, image),
            Pick::Image(index) => (style.max(1), index),
            Pick::Shape(id) => (id, 0),
        };
        if next_style != style {
            let _ = self.set_console_cvar("cg_drawCrosshair", &next_style.to_string());
        }
        if next_image != image {
            let _ = self.set_console_cvar("cg_crosshairImage", &next_image.to_string());
        }
    }
}
