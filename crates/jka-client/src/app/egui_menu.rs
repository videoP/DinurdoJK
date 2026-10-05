//! app::egui_menu module facade. Implementations are grouped by responsibility.
mod assets;
mod audio;
mod catalogs;
mod chat_logs;
mod controls;
mod demos;
mod dialogs;
mod editors;
mod frontend_pages;
mod maps;
mod profile;
mod resume;
mod screenshots;
mod servers;
mod shell;
mod solo;

// The whole in-game menu, drawn with egui.
//
// Everything the player sees once ESC is pressed lives here: the global top
// bar, the Setup tab strip, the Video section rail, the non-Video pages and
// the modal video confirmation. Laying the chrome out with real egui panels
// (rather than free-floating areas over renderer-drawn text) is what keeps
// the tab strip clickable instead of buried under the settings panel.
//
// The renderer-drawn overlay still owns the HUD, chat, console, crosshair and
// loading screen, which are in-world surfaces rather than menu chrome.

use super::egui_theme as theme;
use super::map_editor::MapEditTool;
use super::ui_catalog::CatalogPayload;
use super::*;

const TOP_ITEMS: [&str; 7] = [
    "GAME", "SERVERS", "PROFILE", "CONTROLS", "SETUP", "VOTE", "MOD",
];
const SETUP_TABS: [&str; 6] = ["GAME", "CAMERA", "VIDEO", "AUDIO", "NETWORK", "INTERFACE"];

const TOP_RESUME: usize = 0;
const TOP_PROFILE: usize = 2;
const TOP_CONTROLS: usize = 3;
pub(super) const TOP_SETUP: usize = 4;

const SETUP_TAB_GAME: usize = 0;
pub(super) const SETUP_TAB_CAMERA: usize = 1;
const SETUP_TAB_VIDEO: usize = 2;
const SETUP_TAB_AUDIO: usize = 3;
const SETUP_TAB_NETWORK: usize = 4;
const SETUP_TAB_INTERFACE: usize = 5;

/// Widest the settings/page column is allowed to get. Sized to just fit a
/// label, a full-width meter and its readout: on the Video page the world
/// behind the menu is deliberately left un-blurred so the effect of a setting
/// is visible, and every extra point of panel width hides more of it.
const CONTENT_MAX_W: f32 = 720.0;
/// Left inset shared by the tab strip, the rail and the page column so all
/// three read off the same vertical line.
const GUTTER: f32 = 24.0;

/// Keep the renderer hidden until the frontend BSP is actually ready, then
/// reveal it gently behind the menu. This also replaces the renderer's empty
/// world/fog clear color with intentional black during startup.
const FRONTEND_SCENE_FADE_IN_SECS: f32 = 4.0;

const PROFILE_SECTIONS: [&str; 5] = ["IDENTITY", "MODEL", "FORCE", "SABER", "COSMETICS"];
const PROFILE_IDENTITY: usize = 0;
const PROFILE_MODEL: usize = 1;
const PROFILE_FORCE: usize = 2;
const PROFILE_SABER: usize = 3;
const PROFILE_COSMETICS: usize = 4;

// OpenJK bg_misc.c / TaystJK ui_force.c. Keep Profile force editing on the
// protocol-26 rank-side-18digits representation rather than inventing a new
// loadout format.
const FORCE_MASTERY_POINTS: [i32; 8] = [0, 5, 10, 20, 30, 50, 75, 100];
const FORCE_MASTERY_NAMES: [&str; 8] = [
    "Uninitiated",
    "Initiate",
    "Padawan",
    "Jedi",
    "Jedi Adept",
    "Jedi Guardian",
    "Jedi Knight",
    "Jedi Master",
];
const FORCE_COSTS: [[i32; 4]; 18] = [
    [0, 2, 4, 6],
    [0, 0, 2, 6],
    [0, 2, 4, 6],
    [0, 1, 3, 6],
    [0, 1, 3, 6],
    [0, 4, 6, 8],
    [0, 1, 3, 6],
    [0, 2, 5, 8],
    [0, 4, 6, 8],
    [0, 2, 5, 8],
    [0, 1, 3, 6],
    [0, 1, 3, 6],
    [0, 1, 3, 6],
    [0, 2, 4, 6],
    [0, 2, 5, 8],
    [0, 1, 5, 8],
    [0, 1, 5, 8],
    [0, 4, 6, 8],
];
const FORCE_SIDES: [i32; 18] = [1, 0, 0, 0, 0, 1, 2, 2, 2, 1, 1, 1, 2, 2, 0, 0, 0, 0];
const FORCE_NAMES: [&str; 18] = [
    "Heal",
    "Jump",
    "Speed",
    "Push",
    "Pull",
    "Mind Trick",
    "Grip",
    "Lightning",
    "Rage",
    "Protect",
    "Absorb",
    "Team Heal",
    "Team Force",
    "Drain",
    "Seeing",
    "Saber Attack",
    "Saber Defense",
    "Saber Throw",
];
// Keep neutral powers visually stable at the top, then append only the chosen
// alignment. This mirrors the stock allocation rules while making the modern
// editor much easier to scan.
const FORCE_NEUTRAL_ORDER: [usize; 8] = [1, 2, 3, 4, 14, 15, 16, 17];
const FORCE_LIGHT_ORDER: [usize; 5] = [0, 5, 9, 10, 11];
const FORCE_DARK_ORDER: [usize; 5] = [6, 7, 8, 13, 12];

#[derive(Clone, Copy)]
struct ProfileForceConfig {
    rank: u8,
    side: u8,
    powers: [u8; 18],
}

impl ProfileForceConfig {
    fn parse(value: &str) -> Self {
        let mut out = Self {
            rank: 7,
            side: 1,
            powers: [0; 18],
        };
        let mut parts = value.trim().splitn(3, '-');
        out.rank = parts
            .next()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(7)
            .min(7);
        out.side = match parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(1) {
            2 => 2,
            _ => 1,
        };
        if let Some(powers) = parts.next() {
            for (index, byte) in powers.bytes().take(18).enumerate() {
                if byte.is_ascii_digit() {
                    out.powers[index] = (byte - b'0').min(3);
                }
            }
        }
        // ui_force.c always gives the player the free first Jump level.
        out.powers[1] = out.powers[1].max(1);
        out
    }

    fn serialize(self) -> String {
        let powers = self
            .powers
            .iter()
            .map(|level| char::from(b'0' + (*level).min(3)))
            .collect::<String>();
        format!("{}-{}-{powers}", self.rank, self.side)
    }

    fn budget(self) -> i32 {
        FORCE_MASTERY_POINTS[self.rank.min(7) as usize]
    }

    fn used(self, free_saber: bool) -> i32 {
        let mut total = 0;
        for (power, &level) in self.powers.iter().enumerate() {
            for rank in 1..=level.min(3) as usize {
                if (power == 1 && rank == 1)
                    || (free_saber && (power == 15 || power == 16) && rank == 1)
                {
                    continue;
                }
                total += FORCE_COSTS[power][rank];
            }
        }
        total
    }

    fn normalize(&mut self, max_rank: u8, disabled: u32, gametype: i32, free_saber: bool) {
        self.rank = self.rank.min(max_rank.min(7));
        if self.side != 1 && self.side != 2 {
            self.side = 2;
        }

        for power in 0..18 {
            self.powers[power] = self.powers[power].min(3);
            if self.powers[power] != 0
                && FORCE_SIDES[power] != 0
                && FORCE_SIDES[power] != self.side as i32
            {
                self.powers[power] = 0;
            }
            if self.powers[power] != 0 && disabled & (1u32 << power) != 0 {
                self.powers[power] = 0;
            }
        }
        if gametype < 6 {
            self.powers[11] = 0;
            self.powers[12] = 0;
        }

        // Port BG_LegalizedForcePowers' over-budget reduction order. It drains
        // lower-ranked powers first, preserving higher investments, with the
        // saber attack/defense/throw dependency handled exactly like OpenJK.
        let allowed = self.budget();
        let mut used = self.used(free_saber);
        if used > allowed {
            let min_saber = u8::from(free_saber);
            let mut attempted_cycles = 0;
            let mut power_cycle = 2u8;
            while used > allowed {
                for power in 0..18 {
                    if used <= allowed {
                        break;
                    }
                    if self.powers[power] != 0 && self.powers[power] < power_cycle {
                        if power == 15 && (self.powers[16] > min_saber || self.powers[17] > 0) {
                            let which = if self.powers[17] != 0 { 17 } else { 16 };
                            while self.powers[which] > 0 && used > allowed {
                                let level = self.powers[which] as usize;
                                if self.powers[which] > 1
                                    || ((which != 15 || !free_saber)
                                        && (which != 16 || !free_saber))
                                {
                                    used -= FORCE_COSTS[which][level];
                                    self.powers[which] -= 1;
                                } else {
                                    break;
                                }
                            }
                        } else {
                            while self.powers[power] > 0 && used > allowed {
                                let level = self.powers[power] as usize;
                                if self.powers[power] > 1
                                    || (power != 1
                                        && (power != 15 || !free_saber)
                                        && (power != 16 || !free_saber))
                                {
                                    used -= FORCE_COSTS[power][level];
                                    self.powers[power] -= 1;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                }
                power_cycle = power_cycle.saturating_add(1);
                attempted_cycles += 1;
                if attempted_cycles > 18 {
                    break;
                }
            }
            if used > allowed {
                self.powers = [0; 18];
            }
        }

        if free_saber {
            self.powers[15] = self.powers[15].max(1);
            self.powers[16] = self.powers[16].max(1);
        }
        self.powers[1] = self.powers[1].max(1);

        // BG_LegalizedForcePowers has deliberate special handling for these
        // three disabled powers (all-force-disabled servers depend on it).
        if disabled & (1u32 << 1) != 0 {
            self.powers[1] = 1;
        }
        if disabled & (1u32 << 15) != 0 {
            self.powers[15] = 3;
        }
        if disabled & (1u32 << 16) != 0 {
            self.powers[16] = 3;
        }
        if self.powers[15] == 0 {
            self.powers[16] = 0;
            self.powers[17] = 0;
        }
    }
}

/// One entry in the Video page's left rail. Splitting the old single scrolling
/// wall of settings into addressable sections is what makes the page skimmable;
/// the RENDERING/ENVIRONMENT split becomes a group heading in the rail instead
/// of a pair of buttons floating in the panel header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum VideoSection {
    Display,
    ImageQuality,
    Visibility,
    Models,
    Lighting,
    Effects,
    Shadows,
    Reflections,
    PostProcessing,
    Color,
    DebugTools,
    Physics,
    Sun,
    Clouds,
    Weather,
    Surface,
    Water,
}

impl VideoSection {
    const RENDERING: [(Self, &'static str); 11] = [
        (Self::Display, "Display"),
        (Self::ImageQuality, "Image quality"),
        (Self::Visibility, "Visibility"),
        (Self::Models, "Models"),
        (Self::Lighting, "Lighting"),
        (Self::Effects, "Effects"),
        (Self::Shadows, "Shadows"),
        (Self::Reflections, "Reflections"),
        (Self::PostProcessing, "Post processing"),
        (Self::Color, "Color"),
        (Self::Physics, "Physics"),
    ];
    const ENVIRONMENT: [(Self, &'static str); 5] = [
        (Self::Sun, "Sun"),
        (Self::Clouds, "Clouds"),
        (Self::Weather, "Weather"),
        (Self::Surface, "Surface"),
        (Self::Water, "Water"),
    ];
    const TOOLS: [(Self, &'static str); 1] = [(Self::DebugTools, "Debug & tools")];
}

// ------------------------------------------------------------- free widgets --

/// The legacy control table stores SHOUTED labels; the menu reads them back in
/// sentence case so it matches every other page.
pub(super) fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start_of_word = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            start_of_word = true;
            out.push(ch);
        } else if start_of_word {
            start_of_word = false;
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

/// Group heading in the rail. Deliberately unlike a rail item: larger, bold,
/// bright, and closed off with a rule, because at the same weight as the
/// entries beneath it people read it as one more thing to click.
fn rail_group(ui: &mut egui::Ui, title: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    theme::glow_text(
        &painter,
        egui::pos2(rect.left() + GUTTER - 12.0, rect.center().y + 2.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(14.0),
        theme::TEXT,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + GUTTER - 12.0, rect.bottom() - 1.0),
            egui::vec2(rect.width() - (GUTTER - 12.0) - 12.0, 1.0),
        ),
        egui::CornerRadius::ZERO,
        theme::LINE,
    );
    ui.add_space(4.0);
}

fn refresh_icon(ui: &mut egui::Ui, tooltip: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let painter = ui.painter().clone();
    painter.rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        if response.hovered() {
            theme::CONTROL_HOVER
        } else {
            theme::CONTROL
        },
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(
            1.0_f32,
            if response.hovered() {
                theme::ACCENT
            } else {
                theme::LINE_STRONG
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "↻",
        egui::FontId::proportional(16.0),
        if response.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    response.on_hover_text(tooltip).clicked()
}

fn chat_log_duration_label(start_ms: u64, end_ms: u64) -> String {
    let seconds = end_ms.saturating_sub(start_ms) / 1000;
    let hours = seconds / 3600;
    let minutes = (seconds / 60) % 60;
    let seconds = seconds % 60;
    if hours != 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Compact affordance for the existing `fs_refresh` path. Asset Viewer,
/// source .map browsing, and Solo Game all use the same VFS invalidation and
/// retry behavior rather than maintaining separate partial refresh routines.
fn filesystem_refresh_icon(ui: &mut egui::Ui) -> bool {
    refresh_icon(ui, "Refresh filesystem (fs_refresh)")
}

fn jka_ui_color(code: char, fallback: egui::Color32) -> egui::Color32 {
    match code {
        '0' => egui::Color32::from_rgb(0x24, 0x28, 0x2E),
        '1' => egui::Color32::from_rgb(0xFF, 0x5C, 0x5C),
        '2' => egui::Color32::from_rgb(0x70, 0xD8, 0x78),
        '3' => egui::Color32::from_rgb(0xF2, 0xD5, 0x62),
        '4' => egui::Color32::from_rgb(0x6E, 0x9C, 0xFF),
        '5' => egui::Color32::from_rgb(0x63, 0xD7, 0xE8),
        '6' => egui::Color32::from_rgb(0xD9, 0x78, 0xE8),
        '7' => theme::TEXT,
        '8' => egui::Color32::from_rgb(0xFF, 0x9A, 0x45),
        '9' => egui::Color32::from_rgb(0xB8, 0xBE, 0xC8),
        _ => fallback,
    }
}

/// Render JKA `^0`..`^9` color escapes directly in egui. Server/player names
/// keep their authored colors while searching and sorting continue to use the
/// color-stripped text.
pub(super) fn jka_colored_text(
    text: &str,
    size: f32,
    fallback: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut color = fallback;
    let mut run = String::new();
    let flush = |job: &mut egui::text::LayoutJob, run: &mut String, color: egui::Color32| {
        if run.is_empty() {
            return;
        }
        job.append(
            run,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(size),
                color,
                ..Default::default()
            },
        );
        run.clear();
    };

    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    flush(&mut job, &mut run, color);
                    chars.next();
                    color = jka_ui_color(next, fallback);
                    continue;
                }
            }
        }
        run.push(ch);
    }
    flush(&mut job, &mut run, color);
    job
}

/// Build an editor galley with the same character count as the stored name.
/// Color escapes become zero-width placeholders, preserving cursor movement
/// and backspace semantics while the surrounding runs keep their JKA colors.
fn profile_name_layout(raw: &str, size: f32, fallback: egui::Color32) -> egui::text::LayoutJob {
    const HIDDEN_CHAR: char = '\u{2060}';

    let mut job = egui::text::LayoutJob::default();
    let mut color = fallback;
    let mut run = String::new();
    let flush = |job: &mut egui::text::LayoutJob, run: &mut String, color: egui::Color32| {
        if run.is_empty() {
            return;
        }
        job.append(
            run,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(size),
                color,
                ..Default::default()
            },
        );
        run.clear();
    };

    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    flush(&mut job, &mut run, color);
                    chars.next();
                    let hidden = format!("{HIDDEN_CHAR}{HIDDEN_CHAR}");
                    job.append(
                        &hidden,
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(size),
                            color: egui::Color32::TRANSPARENT,
                            ..Default::default()
                        },
                    );
                    color = jka_ui_color(next, fallback);
                    continue;
                }
            }
        }
        run.push(ch);
    }
    flush(&mut job, &mut run, color);
    job
}

fn asset_size_label(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.2} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as usize)
    }
}

fn rail_item(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 27.0), egui::Sense::click());
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, theme::CONTROL_SELECTED);
        painter.rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
            egui::CornerRadius::ZERO,
            theme::ACCENT,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, theme::CONTROL);
    }
    painter.text(
        egui::pos2(rect.left() + GUTTER, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        if selected {
            theme::TEXT
        } else if response.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    response
}

/// A wide, flat action row used by the Resume page.
fn menu_action(ui: &mut egui::Ui, label: &str, detail: &str, enabled: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 48.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let painter = ui.painter().clone();
    let hovered = enabled && response.hovered();
    painter.rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        if hovered {
            theme::CONTROL_HOVER
        } else {
            theme::CONTROL
        },
    );
    if hovered {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
            egui::CornerRadius::ZERO,
            theme::ACCENT,
        );
    }
    painter.text(
        egui::pos2(rect.left() + 16.0, rect.top() + 15.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.5),
        if enabled {
            theme::TEXT
        } else {
            theme::TEXT_DISABLED
        },
    );
    painter.text(
        egui::pos2(rect.left() + 16.0, rect.top() + 33.0),
        egui::Align2::LEFT_CENTER,
        detail,
        egui::FontId::proportional(11.5),
        theme::TEXT_FAINT,
    );
    ui.add_space(6.0);
    enabled && response.clicked()
}

pub(super) fn binding_slot(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    waiting: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(190.0, 24.0), egui::Sense::click_and_drag());
    let painter = ui.painter().clone();
    let fill = if waiting {
        theme::CONTROL_SELECTED
    } else if response.hovered() {
        theme::CONTROL_HOVER
    } else {
        theme::CONTROL
    };
    painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, if waiting { theme::WARNING } else { theme::LINE }),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(12.5),
        color,
    );
    response
}

fn confirmation_dialog(ctx: &egui::Context, seconds: u32) -> Option<bool> {
    let mut result = None;
    let screen = ctx.content_rect();
    // Painted straight onto the background layer: an `Area` clips to its own
    // (zero-sized) content rect, so a full-screen scrim drawn inside one never
    // shows up.
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("jka_video_confirmation_dim"),
    ))
    .rect_filled(
        screen,
        egui::CornerRadius::ZERO,
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 170),
    );
    egui::Area::new(egui::Id::new("jka_video_confirmation"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(
            screen.center().x - 250.0,
            screen.center().y - 90.0,
        ))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0_f32, theme::LINE_STRONG))
                .inner_margin(egui::Margin::same(22))
                .show(ui, |ui| {
                    ui.set_width(456.0);
                    theme::label(
                        ui,
                        theme::plain("KEEP THESE DISPLAY SETTINGS?", 17.0, theme::TEXT).strong(),
                    );
                    ui.add_space(8.0);
                    theme::label(
                        ui,
                        theme::plain(
                            "The previous display mode is restored automatically if you do nothing.",
                            12.5,
                            theme::TEXT_DIM,
                        ),
                    );
                    ui.add_space(12.0);
                    theme::banner(
                        ui,
                        &format!("Reverting in {seconds} second{}", if seconds == 1 { "" } else { "s" }),
                        theme::WARNING,
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "KEEP  (ENTER)").clicked() {
                            result = Some(true);
                        }
                        ui.add_space(10.0);
                        if theme::ghost_button(ui, "REVERT  (ESC)").clicked() {
                            result = Some(false);
                        }
                    });
                });
        });
    result
}
