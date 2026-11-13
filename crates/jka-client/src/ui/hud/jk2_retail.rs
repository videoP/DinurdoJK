//! Native Jedi Outcast (JK2) multiplayer status HUD.
//!
//! Source authority: Raven's CODE-mp/cgame/cg_draw.c (`CG_DrawHUD`,
//! `CG_DrawArmor`, `CG_DrawHealth`, `CG_DrawAmmo`, `CG_DrawForcePower`),
//! cg_main.c (forceTicPos/ammoTicPos/media paths), and cg_drawtools.c
//! (`CG_DrawNumField`). No JKA HUD art or scripted extensions are involved.
//! Coordinates are 640x480 virtual units; the clusters are independently editable.
use crate::ui::{
    hud_element_rect, textured_rect_with_source, HudElementId, HudRectContext,
    UiSnapshot, UiVertex,
};
use super::extensions::{
    EXT_IMAGE_SHADER_SOURCE_BASE, EXT_IMAGE_TEXTURE_SOURCE,
};


// JK2 source: code/cgame/cg_weapons.cpp CG_DrawIconBackground (single-player)
// and CODE-mp/cgame/cg_main.c media registration. The JK2 MP renderer left
// these actual draw calls commented, but the authored textures and the 130 ms
// interpolation remain present in the original single-player renderer.
const SELECT_BG_WEAPON: &str = "gfx/hud/background";
const SELECT_BG_FORCE: &str = "gfx/hud/background_f";
const SELECT_BG_ITEM: &str = "gfx/hud/background_i";
const SELECT_PRONG_WEAPON: &str = "gfx/hud/prong_on_w";
const SELECT_PRONG_FORCE: &str = "gfx/hud/prong_on_f";
const SELECT_PRONG_ITEM: &str = "gfx/hud/prong_on_i";
const SELECT_PRONG_OFF: &str = "gfx/hud/prong_off";
const SELECT_ART: [&str; 7] = [
    SELECT_BG_WEAPON, SELECT_BG_FORCE, SELECT_BG_ITEM,
    SELECT_PRONG_WEAPON, SELECT_PRONG_FORCE, SELECT_PRONG_ITEM, SELECT_PRONG_OFF,
];
// Raven CODE-mp/cgame/cg_main.c weaponIcons / weaponIcons_NA (JK2 only).
const JK2_WEAPON_ICONS: [&str; 14] = [
    "", "gfx/hud/w_icon_stunbaton", "gfx/hud/w_icon_lightsaber",
    "gfx/hud/w_icon_briar", "gfx/hud/w_icon_blaster", "gfx/hud/w_icon_disruptor",
    "gfx/hud/w_icon_bowcaster", "gfx/hud/w_icon_repeater",
    "gfx/hud/w_icon_demp2", "gfx/hud/w_icon_flechette",
    "gfx/hud/w_icon_merrsonn", "gfx/hud/w_icon_thermal",
    "gfx/hud/w_icon_tripmine", "gfx/hud/w_icon_detpack",
];
const JK2_WEAPON_ICONS_EMPTY: [&str; 14] = [
    "", "gfx/hud/w_icon_stunbaton_na", "gfx/hud/w_icon_lightsaber_na",
    "gfx/hud/w_icon_briar_na", "gfx/hud/w_icon_blaster_na",
    "gfx/hud/w_icon_disruptor_na", "gfx/hud/w_icon_bowcaster_na",
    "gfx/hud/w_icon_repeater_na", "gfx/hud/w_icon_demp2_na",
    "gfx/hud/w_icon_flechette_na", "gfx/hud/w_icon_merrsonn_na",
    "gfx/hud/w_icon_thermal_na", "gfx/hud/w_icon_tripmine_na",
    "gfx/hud/w_icon_detpack_na",
];
const JK2_WEAPON_NAMES: [&str; 14] = [
    "", "Stun Baton", "Lightsaber", "Bryar Pistol", "E-11 Blaster Rifle",
    "Disruptor Rifle", "Bowcaster", "Heavy Repeater", "DEMP2",
    "Flechette", "Rocket Launcher", "Thermal Detonator",
    "Trip Mine", "Det Pack",
];
// CODE-mp/game/bg_misc.c bg_itemlist for the six JK2 holdable tags.
const JK2_ITEM_ICONS: [&str; 7] = [
    "", "gfx/hud/i_icon_seeker", "gfx/hud/i_icon_shieldwall",
    "gfx/hud/i_icon_bacta", "", "gfx/hud/i_icon_zoom",
    "gfx/hud/i_icon_sentrygun",
];
const JK2_ITEM_NAMES: [&str; 7] = [
    "", "Seeker Drone", "Forcefield", "Bacta Canister", "Datapad",
    "Binoculars", "Sentry Gun",
];

// Bucky MVSDK cg_draw.c: the enhanced flag status uses these exact media.
const JK2_FLAG_ICONS: [&str; 3] = [
    "gfx/hud/mpi_rflag", "gfx/hud/mpi_bflag", "icons/iconf_neutral1",
];

/// The exact carried-flag icon used by the JK2 flag-status HUD and the JKA
/// Siege scoreboard. Input is the flag's team, *not* its carrier's team.
pub(crate) fn flag_carrier_icon(flag_team: i32) -> Option<&'static str> {
    match flag_team {
        1 => Some(JK2_FLAG_ICONS[0]),
        2 => Some(JK2_FLAG_ICONS[1]),
        0 => Some(JK2_FLAG_ICONS[2]),
        _ => None,
    }
}
const JK2_FLAG_TAKEN: [&str; 3] = [
    "gfx/hud/mpi_rflag_x", "gfx/hud/mpi_bflag_x", "icons/iconf_neutral1_x",
];

/// Exact MVSDK enhanced-flag-status imagery. `status=1` means taken,
/// `status=4` means dropped; never substitute a fabricated flag glyph.
pub(crate) fn flag_status_icon(team: i32, status: i32) -> Option<&'static str> {
    let index = match team { 1 => 0, 2 => 1, 0 => 2, _ => return None };
    Some(if status == 1 { JK2_FLAG_TAKEN[index] } else { JK2_FLAG_ICONS[index] })
}
const WHITE: [f32; 4] = [1.0; 4];
const GREEN: [f32; 4] = [0.0, 0.613, 0.097, 1.0];
const RED: [f32; 4] = [0.835, 0.015, 0.015, 1.0];
const ORANGE: [f32; 4] = [1.0, 0.658, 0.062, 1.0];
const LEFT_INNER: &str = "gfx/hud/hudleft_innerframe";
const LEFT_FRAME: &str = "gfx/hud/hudleft";
const RIGHT_INNER: &str = "gfx/hud/hudright_innerframe";
const RIGHT_FRAME: &str = "gfx/hud/hudrightframe";
const ARMOR_OUTER: &str = "gfx/hud/armor1";
const ARMOR_INNER: &str = "gfx/hud/armor2";
const ARMOR_TIC: &str = "gfx/hud/armor_tic";
const HEALTH: &str = "gfx/hud/health";
const HEALTH_TIC: &str = "gfx/hud/health_tic";
const SABER_STYLES: [&str; 3] = [
    "gfx/hud/saber_stylesFast", "gfx/hud/saber_stylesMed", "gfx/hud/saber_stylesStrong",
];
// CODE-mp/game/bg_weapons.h + bg_weapons.c. JK2 and JKA weapon IDs are
// NOT interchangeable (JK2 WP_SABER is 2, not JKA's 3).
pub(crate) fn jk2_ammo_index(weapon: i32) -> usize {
    match weapon {
        3 | 4 => 2,            // Bryar pistol, E-11: AMMO_BLASTER
        5 | 6 | 8 => 3,        // Disruptor, bowcaster, DEMP2: AMMO_POWERCELL
        7 | 9 => 4,            // Repeater, flechette: AMMO_METAL_BOLTS
        10 => 5,               // Rocket launcher: AMMO_ROCKETS
        11 => 7,               // Thermal detonator
        12 => 8,               // Trip mine
        13 => 9,               // Det pack
        _ => 0,
    }
}
// CODE-mp/game/bg_weapons.c ammoData[AMMO_MAX], not JKA's ammo table.
const JK2_AMMO_MAX: [i32; 10] = [0, 100, 300, 300, 300, 25, 800, 10, 10, 10];

const NUMBERS: [&str; 11] = [
    "gfx/2d/numbers/t_zero", "gfx/2d/numbers/t_one",
    "gfx/2d/numbers/t_two", "gfx/2d/numbers/t_three",
    "gfx/2d/numbers/t_four", "gfx/2d/numbers/t_five",
    "gfx/2d/numbers/t_six", "gfx/2d/numbers/t_seven",
    "gfx/2d/numbers/t_eight", "gfx/2d/numbers/t_nine",
    "gfx/2d/numbers/t_minus",
];

// JK2 CODE-mp/cgame/cg_main.c: forceTicPos and ammoTicPos, in source order.
// A negative width is an intentionally mirrored (right-hand) tick.
type Piece = (&'static str, f32, f32, f32, f32);
const FORCE_TICS: [Piece; 14] = [
    ("gfx/hud/force_tick1", 11.0, 41.0, 20.0, 10.0),
    ("gfx/hud/force_tick2", 12.0, 45.0, 20.0, 10.0),
    ("gfx/hud/force_tick3", 14.0, 49.0, 20.0, 10.0),
    ("gfx/hud/force_tick4", 17.0, 52.0, 20.0, 10.0),
    ("gfx/hud/force_tick5", 22.0, 55.0, 10.0, 10.0),
    ("gfx/hud/force_tick6", 28.0, 57.0, 10.0, 20.0),
    ("gfx/hud/force_tick7", 34.0, 59.0, 10.0, 10.0),
    ("gfx/hud/force_tick7", 46.0, 59.0, -10.0, 10.0),
    ("gfx/hud/force_tick6", 52.0, 57.0, -10.0, 20.0),
    ("gfx/hud/force_tick5", 58.0, 55.0, -10.0, 10.0),
    ("gfx/hud/force_tick4", 63.0, 52.0, -20.0, 10.0),
    ("gfx/hud/force_tick3", 66.0, 49.0, -20.0, 10.0),
    ("gfx/hud/force_tick2", 68.0, 45.0, -20.0, 10.0),
    ("gfx/hud/force_tick1", 69.0, 41.0, -20.0, 10.0),
];
const AMMO_TICS: [Piece; 14] = [
    ("gfx/hud/ammo_tick7", 12.0, 34.0, 10.0, 10.0),
    ("gfx/hud/ammo_tick6", 13.0, 28.0, 10.0, 10.0),
    ("gfx/hud/ammo_tick5", 15.0, 23.0, 10.0, 10.0),
    ("gfx/hud/ammo_tick4", 19.0, 19.0, 10.0, 10.0),
    ("gfx/hud/ammo_tick3", 23.0, 15.0, 10.0, 10.0),
    ("gfx/hud/ammo_tick2", 29.0, 12.0, 10.0, 10.0),
    ("gfx/hud/ammo_tick1", 34.0, 11.0, 10.0, 10.0),
    ("gfx/hud/ammo_tick1", 47.0, 11.0, -10.0, 10.0),
    ("gfx/hud/ammo_tick2", 52.0, 12.0, -10.0, 10.0),
    ("gfx/hud/ammo_tick3", 58.0, 15.0, -10.0, 10.0),
    ("gfx/hud/ammo_tick4", 62.0, 19.0, -10.0, 10.0),
    ("gfx/hud/ammo_tick5", 66.0, 23.0, -10.0, 10.0),
    ("gfx/hud/ammo_tick6", 68.0, 28.0, -10.0, 10.0),
    ("gfx/hud/ammo_tick7", 69.0, 34.0, -10.0, 10.0),
];

/// Appends only the assets for the active game; extension image slots retain their order.
/// As with the JKA native HUD, loading/decoding stays on the existing image worker.
/// JKA reuses the stock red/blue flag-status shaders without registering JK2
/// inventory and weapon art in the JKA atlas.
pub(crate) fn append_flag_status_images(images: &mut Vec<String>) {
    for name in JK2_FLAG_ICONS[..2].iter() {
        if !images.iter().any(|present| present == name) {
            images.push((*name).to_owned());
        }
    }
}

pub(crate) fn append_jk2_ctf_images(images: &mut Vec<String>) {
    for name in JK2_FLAG_ICONS.into_iter().chain(JK2_FLAG_TAKEN)
        .chain(JK2_ITEM_ICONS.into_iter().filter(|name| !name.is_empty()))
        .chain([JK2_WEAPON_ICONS[12]])
    {
        if !images.iter().any(|present| present == name) {
            images.push(name.to_owned());
        }
    }
}

pub(crate) fn append_jk2_hud_images(images: &mut Vec<String>, style: u8) {
    // Stock JK2 weapon/inventory selection is independent of the status HUD
    // style: custom panels still need the native JK2 icons and ID ordering.
    // Preserve the existing registration order in retail style so cached
    // extension atlas indices are stable when that style is selected.
    if style == 1 {
        let retail = [
            LEFT_INNER, LEFT_FRAME, RIGHT_INNER, RIGHT_FRAME,
            ARMOR_OUTER, ARMOR_INNER, ARMOR_TIC, HEALTH, HEALTH_TIC,
        ].into_iter()
            .chain(SABER_STYLES)
            .chain(FORCE_TICS.iter().map(|piece| piece.0))
            .chain(AMMO_TICS.iter().map(|piece| piece.0))
            .chain(NUMBERS)
            .chain(SELECT_ART);
        for name in retail {
            if !images.iter().any(|present| present == name) {
                images.push(name.to_owned());
            }
        }
    }
    let selectors = JK2_WEAPON_ICONS.iter().copied().filter(|name| !name.is_empty())
        .chain(JK2_WEAPON_ICONS_EMPTY.iter().copied().filter(|name| !name.is_empty()))
        .chain(JK2_ITEM_ICONS.iter().copied().filter(|name| !name.is_empty()));
    for name in selectors {
        if !images.iter().any(|present| present == name) {
            images.push(name.to_owned());
        }
    }
}

struct Painter<'a> {
    out: &'a mut Vec<UiVertex>,
    images: &'a [String],
    shader_flags: &'a [bool],
    uvs: &'a [([f32; 2], [f32; 2])],
    screen: [u32; 2],
    origin: [f32; 2],
    /// Uniform 640x480-to-pixel factor. HUD artwork must never stretch on widescreen.
    unit: f32,
}

impl Painter<'_> {
    fn image(&mut self, name: &str, x: f32, y: f32, width: f32, height: f32, color: [f32; 4]) {
        self.image_pixels(
            name, self.origin[0] + x * self.unit, self.origin[1] + y * self.unit,
            width * self.unit, height * self.unit, [0.0, 1.0], color,
        );
    }

    /// Draw artwork in real pixels, optionally sampling a horizontal region.
    /// The selector's long background can grow through its featureless centre,
    /// without stretching the actual artwork (or any circular HUD component).
    #[allow(clippy::too_many_arguments)]
    fn image_pixels(
        &mut self, name: &str, x: f32, y: f32, width: f32, height: f32,
        u_range: [f32; 2], color: [f32; 4],
    ) {
        let Some(index) = self.images.iter().position(|image| image == name) else { return; };
        let (mut uv0, mut uv1) = if self.shader_flags.get(index).copied().unwrap_or(false) {
            ([0.0, 0.0], [1.0, 1.0])
        } else {
            self.uvs.get(index).copied().unwrap_or(([0.0, 0.0], [0.0, 0.0]))
        };
        let u_start = uv0[0];
        let u_span = uv1[0] - uv0[0];
        uv0[0] = u_start + u_span * u_range[0];
        uv1[0] = u_start + u_span * u_range[1];
        if width < 0.0 { std::mem::swap(&mut uv0[0], &mut uv1[0]); }
        if height < 0.0 { std::mem::swap(&mut uv0[1], &mut uv1[1]); }
        textured_rect_with_source(
            self.out,
            x.min(x + width), y.min(y + height), width.abs(), height.abs(),
            uv0, uv1, color,
            if self.shader_flags.get(index).copied().unwrap_or(false) {
                EXT_IMAGE_SHADER_SOURCE_BASE + index as f32
            } else { EXT_IMAGE_TEXTURE_SOURCE },
            self.screen[0], self.screen[1],
        );
    }

    fn frame(&mut self, name: &str, color: [f32; 4]) {
        self.image(name, 0.0, 0.0, 80.0, 80.0, color);
    }

    /// Raven CG_DrawNumField(NUM_FONT_SMALL): x+2+charWidth*(3-digits),
    /// with a one-virtual-pixel gap *after* each six-pixel digit.
    fn number(&mut self, value: i32, x: f32, y: f32, color: [f32; 4]) {
        let digits = value.clamp(-99, 999).to_string();
        let mut x = x + 2.0 + 6.0 * (3 - digits.len()) as f32;
        for ch in digits.chars() {
            let index = if ch == '-' { 10 } else { ch.to_digit(10).unwrap_or(0) as usize };
            self.image(NUMBERS[index], x, y, 6.0, 12.0, color);
            x += 7.0;
        }
    }

    /// CODE-mp draws all 14 segments, including solid-black empty ticks.
    /// The reference reduces the remaining value for each tick in reverse order.
    fn ring(&mut self, pieces: &[Piece; 14], value: i32, maximum: i32) {
        let increment = maximum.max(1) as f32 / 14.0;
        let mut remaining = value as f32;
        for &(name, x, y, w, h) in pieces.iter().rev() {
            let brightness = (remaining / increment).clamp(0.0, 1.0);
            self.image(name, x, y, w, h, [brightness, brightness, brightness, 1.0]);
            remaining -= increment;
        }
    }
}

fn dim(color: [f32; 4], level: f32) -> [f32; 4] {
    let value = level.clamp(0.0, 1.0);
    [color[0] * value, color[1] * value, color[2] * value, color[3]]
}

pub(in crate::ui) fn build(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(hud) = ui.hud else { return; };
    if hud.spectating { return; }
    let ctx = HudRectContext::from_snapshot(ui);
    let tint = super::draw::native_hud_tint(&hud, ui.video.tint_hud);
    // Match the aspect-safe HUD convention used by lagometer, JA+ and JKA:
    // cgs.widthRatioCoef cancels horizontal widescreen stretch. Keep the
    // original 640x480 artwork proportions, but anchor panels to real edges.
    let unit = super::layout::hud_art_scale(w, h);
    let images = &ui.extension_images;
    let shaders = &ui.extension_shader_flags;
    if !ui.native_replaced(crate::extensions::NativeHud::Health) {
        let left = hud_element_rect(HudElementId::Health, ui.hud_layout.health, &ctx, w, h);
        let mut painter = Painter {
            out, images, shader_flags: shaders, uvs: &ui.extension_image_uvs, screen: [w, h],
            origin: [left.x, left.y], unit: unit * ui.hud_layout.health.scale.clamp(0.5, 2.0),
        };
        // Original layering: inner frame, armor rings and tic, health ring
        // and tic, small numbers, outer frame. No JKA stock HUD calls.
        painter.frame(LEFT_INNER, tint);
        let half_max = (hud.max_health.max(1) / 2).max(1) as f32;
        let outer_armor = (hud.armor.min(hud.max_health) as f32 - half_max) / half_max;
        let inner_armor = if outer_armor > 0.0 { 1.0 } else { hud.armor as f32 / half_max };
        painter.frame(ARMOR_OUTER, dim(GREEN, outer_armor));
        painter.frame(ARMOR_INNER, dim(GREEN, inner_armor));
        // Raven toggles these indicators on a 100 ms shared timer below
        // 25% armor / 20% health; the stateless UI uses game-time phase.
        let flashing = (hud.time_ms / 100).rem_euclid(2) == 0;
        if hud.armor != 0 && (inner_armor >= 0.5 || flashing) {
            painter.frame(ARMOR_TIC, GREEN);
        }
        let health_fraction = hud.health.min(hud.max_health) as f32 / hud.max_health.max(1) as f32;
        painter.frame(HEALTH, dim(RED, health_fraction));
        if health_fraction > 0.2 || flashing {
            painter.frame(HEALTH_TIC, if hud.jk2_has_bacta { [0.1, 0.3, 1.0, 1.0] } else { RED });
        }
        painter.number(hud.armor, 32.0, 54.0, GREEN);
        painter.number(hud.health, 16.0, 40.0, RED);
        painter.frame(LEFT_FRAME, tint);
    }
    if !ui.native_replaced(crate::extensions::NativeHud::Ammo) {
        let right = hud_element_rect(HudElementId::Ammo, ui.hud_layout.ammo, &ctx, w, h);
        let mut painter = Painter {
            out, images, shader_flags: shaders, uvs: &ui.extension_image_uvs, screen: [w, h],
            origin: [right.x, right.y], unit: unit * ui.hud_layout.ammo.scale.clamp(0.5, 2.0),
        };
        painter.frame(RIGHT_INNER, tint);
        if let Some(force) = hud.force_power {
            // JK2 CG_DrawForcePower uses a fixed 100 point capacity.
            painter.ring(&FORCE_TICS, force, 100);
        }
        if hud.weapon == 2 {
            // JK2 weapon WP_SABER: style is drawn across the *top* half
            // of the right cluster, rather than using JKA's separate icons.
            if (1..=3).contains(&hud.saber_style) {
                painter.image(SABER_STYLES[(hud.saber_style - 1) as usize],
                    0.0, 0.0, 80.0, 40.0, WHITE);
            }
        } else if hud.weapon > 0 {
            if let Some(ammo) = hud.ammo.filter(|ammo| *ammo >= 0) {
                painter.number(ammo, 30.0, 26.0, ORANGE);
                let maximum = JK2_AMMO_MAX.get(hud.ammo_index).copied().unwrap_or(0);
                painter.ring(&AMMO_TICS, ammo, maximum);
            }
        }
        painter.frame(RIGHT_FRAME, tint);
    }
}


/// Bucky MVSDK `cg_enhancedFlagStatus`: carried flags show the carrier,
/// dropped flags show *only* the flag icon. This overlay is native JK2 and
/// independent of the selectable DinurdoJK vs retail status-panel style.
pub(in crate::ui) fn build_enhanced_flag_status(
    out: &mut Vec<UiVertex>, ui: &UiSnapshot,
    font: Option<&crate::ui::ProportionalFont>, w: u32, h: u32,
) {
    // cg_specHud 2/3/4 draws these flags inside its player-card stacks.
    // Do not retain the old free-floating MVSDK overlay under broadcast HUD.
    if ui.spectator_broadcast.is_some()
        || ui.video.jk2_enhanced_flag_status == 0 || ui.jk2_flags.is_empty() { return; }
    let scale = h as f32 / 480.0;
    // Also show the at-base flags as editing placeholders, so the element
    // remains visible and draggable even when no player has taken a flag.
    let edit_preview = ui.mode == crate::ui::OverlayMode::HudEdit;
    for (row, flag) in ui.jk2_flags.iter().filter(|flag| matches!(flag.status, 1 | 4)
        || (edit_preview && flag.status == 0)).enumerate()
    {
        // JKA reuses its known scoreboard icons. JK2 uses MVSDK's carried
        // *_x variant when the flag is taken.
        let Some(icon) = (if ui.jk2_mode { flag_status_icon(flag.team, flag.status) }
            else { flag_carrier_icon(flag.team) }) else { continue };
        let y = 66.0 + 38.0 * row as f32;
        {
            let mut painter = Painter {
                out, images: &ui.extension_images, shader_flags: &ui.extension_shader_flags,
                uvs: &ui.extension_image_uvs, screen: [w, h], origin: [0.0, 0.0], unit: scale,
            };
            painter.image(icon, 3.0, y, 32.0, 32.0, WHITE);
        }
        // The mod's team colors are red, blue and yellow (TEAM_FREE).
        let color = match flag.team {
            1 => [1.0, 0.25, 0.25, 1.0],
            2 => [0.4, 0.6, 1.0, 1.0],
            _ => [1.0, 0.85, 0.2, 1.0],
        };
        // Dropped means *no name*, even if a stale entity still carries
        // the powerup bit for a single interpolation frame.
        if flag.status != 1 { continue; }
        let Some(carrier) = flag.carrier.as_ref() else { continue; };
        // Avoid an arbitrarily long/colored client name taking over the HUD.
        let name: String = carrier.chars().take(26).collect();
        let mut line = name;
        if let Some((hp, armor)) = flag.health_armor {
            line.push_str(&format!("  {} / {}", hp.max(0), armor.max(0)));
        }
        if ui.video.jk2_enhanced_flag_status >= 2 && flag.held_ms > 0 {
            let secs = flag.held_ms / 1000;
            line.push_str(&format!("  {}:{:02}", secs / 60, secs % 60));
        }
        let x = 39.0 * scale;
        let y = (y + 10.0) * scale;
        if let Some(font) = font {
            crate::ui::proportional_text_uniform(out, &line, font, x, y, 0.65,
                color, true, w, h);
        } else {
            crate::ui::fixed_charset_text(out, &line, x, y,
                7.0 * scale, 12.0 * scale, 7.0 * scale, color, true, w, h);
        }
    }
}

/// MVSDK `cg_drawInventory` is the persistent item stack, not the temporary
/// inventory selection carousel. Does not require the retail JK2 HUD style.
pub(in crate::ui) fn build_persistent_inventory(
    out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32,
) {
    if !ui.video.jk2_draw_inventory { return; }
    let Some(inventory) = ui.jk2_inventory else { return; };
    let scale = h as f32 / 480.0;
    let mut painter = Painter {
        out, images: &ui.extension_images, shader_flags: &ui.extension_shader_flags,
        uvs: &ui.extension_image_uvs, screen: [w, h],
        origin: [w as f32 - 42.0 * scale, 0.0], unit: scale,
    };
    let mut y = 80.0;
    for (tag, icon) in JK2_ITEM_ICONS.iter().enumerate().skip(1) {
        if icon.is_empty() || inventory.owned_bits & (1_u32 << tag) == 0 {
            continue;
        }
        painter.image(icon, 0.0, y, 32.0, 32.0, WHITE);
        y += 35.0;
    }
    if inventory.trip_mines > 0 {
        painter.image(JK2_WEAPON_ICONS[12], 0.0, y, 32.0, 32.0, WHITE);
    }
}

/// Draw the original JK2 460x60 selection art with fixed-size endcaps.
/// Only a narrow centre texel is extended across extra widescreen space;
/// circular frames, icons, and animated prongs remain aspect-correct.
fn select_background(
    painter: &mut Painter<'_>, name: &str, y: f32, height: f32, screen_width: f32,
    tint: [f32; 4],
) {
    let unit = painter.unit;
    let left = 90.0 * unit;
    let half = 230.0 * unit;
    let right = screen_width - (90.0 + 230.0) * unit;
    let y = y * unit;
    let height = height * unit;
    if right <= left + half {
        // 4:3 and narrower: the authored image already fits at uniform scale.
        painter.image_pixels(name, left, y, 460.0 * unit, height, [0.0, 1.0], tint);
    } else {
        painter.image_pixels(name, left, y, half, height, [0.0, 0.5], tint);
        painter.image_pixels(name, left + half, y, right - left - half, height,
            [0.499, 0.501], tint);
        painter.image_pixels(name, right, y, half, height, [0.5, 1.0], tint);
    }
}

/// The authored selector advances in integer-pixel steps. Keep this key shared
/// by the painter and the transient-update dirty check: animation only rebuilds
/// the *stable* gameplay HUD when its visible chrome actually changes.
/// Kind: 0 = idle, 1 = weapon, 2 = Force, 3 = inventory.
pub(crate) fn chrome_key(ui: &UiSnapshot) -> Option<(u8, i32, i32, bool)> {
    if ui.native_replaced(crate::extensions::NativeHud::Jk2Prongs)
        || !ui.jk2_mode || ui.video.hud_style != 1 || !ui.video.draw_hud
        || ui.hud.is_some_and(|hud| hud.spectating)
    {
        return None;
    }
    let selection = [
        ui.weapon_select.map(|value| (1, value.remaining_ms)),
        ui.force_select.map(|value| (2, value.remaining_ms)),
        ui.inventory_select.map(|value| (3, value.remaining_ms)),
    ].into_iter().flatten().find(|(_, remaining)| *remaining > 0);
    let Some((kind, remaining)) = selection else { return Some((0, 0, 0, false)); };
    let percent = if remaining > 1400 {
        ((1530 - remaining.min(1530)) as f32 / 130.0).clamp(0.0, 1.0)
    } else if remaining <= 130 {
        (remaining as f32 / 130.0).clamp(0.0, 1.0)
    } else { 1.0 };
    Some((kind, (8.0 * percent) as i32, (60.0 * percent) as i32, remaining <= 130))
}

/// Replays JK2 single-player's expanding `CG_DrawIconBackground` artwork.
/// This is part of the stable HUD batch, *before* the left and right clusters,
/// so the prongs are correctly behind their frames instead of drawn over them
/// by the later, transient selector-icon batch. The quantized animation key
/// triggers only the necessary stable rebuilds while selection is moving.
pub(in crate::ui) fn build_select_chrome(
    out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32,
) {
    if w == 0 || h == 0 { return; }
    let Some((kind, x_add, span, closing)) = chrome_key(ui) else { return; };
    // Only the idle prongs follow cg_tintHud/team. During selection, use
    // the authored colors of the on-prongs and holographic background; their
    // colored textures must not be multiplied by the team's status-HUD tint.
    let idle_tint = ui.hud.as_ref().map_or(WHITE, |hud|
        super::draw::native_hud_tint(hud, ui.video.tint_hud));
    let tint = if kind == 0 { idle_tint } else { WHITE };
    let unit = super::layout::hud_art_scale(w, h);
    let mut painter = Painter {
        out, images: &ui.extension_images, shader_flags: &ui.extension_shader_flags,
        uvs: &ui.extension_image_uvs, screen: [w, h], origin: [0.0, 0.0], unit,
    };
    let prongs = |painter: &mut Painter<'_>, name: &str, x_add: f32| {
        painter.image_pixels(name, (67.0 + x_add) * unit, 400.0 * unit,
            40.0 * unit, 80.0 * unit, [0.0, 1.0], tint);
        painter.image_pixels(name, w as f32 - (66.0 + x_add) * unit, 400.0 * unit,
            -40.0 * unit, 80.0 * unit, [0.0, 1.0], tint);
    };
    if kind == 0 {
        prongs(&mut painter, SELECT_PRONG_OFF, 0.0);
        return;
    }
    let (background, prong) = match kind {
        1 => (SELECT_BG_WEAPON, SELECT_PRONG_WEAPON),
        2 => (SELECT_BG_FORCE, SELECT_PRONG_FORCE),
        _ => (SELECT_BG_ITEM, SELECT_PRONG_ITEM),
    };
    if span > 0 {
        select_background(&mut painter, background, 440.0, -(span as f32), w as f32, tint);
        select_background(&mut painter, background, 438.0, span as f32, w as f32, tint);
    }
    prongs(&mut painter, if closing { SELECT_PRONG_OFF } else { prong }, x_add as f32);
}

fn select_label(
    out: &mut Vec<UiVertex>, label: &str,
    color: [f32; 4], font: Option<&crate::ui::ProportionalFont>, w: u32, h: u32,
) {
    let scale = h as f32 / 480.0;
    let x = w as f32 * 0.5;
    if let Some(font) = font {
        let size = 0.55;
        let width = crate::ui::proportional_text_width(label, font, size) * scale;
        crate::ui::proportional_text_uniform(out, label, font, x - width * 0.5,
            455.0 * scale, size, color, true, w, h);
    } else {
        let width = 8.0 * scale;
        crate::ui::fixed_charset_text(out, label, x - label.len() as f32 * width * 0.5,
            455.0 * scale, width, 12.0 * scale, width, color, true, w, h);
    }
}

/// Dedicated JK2 weapon carousel. JKA IDs (saber=3, concussion=15) and
/// JKA's icon atlas are intentionally not used for this protocol.
pub(in crate::ui) fn build_weapon_select(
    out: &mut Vec<UiVertex>, ui: &UiSnapshot,
    font: Option<&crate::ui::ProportionalFont>, w: u32, h: u32,
) {
    if ui.native_replaced(crate::extensions::NativeHud::WeaponSelect) { return; }
    let Some(selection) = ui.weapon_select else { return; };
    if selection.remaining_ms <= 130 || !(1..=13).contains(&selection.selected) { return; }
    // JK2 displays every owned weapon; weapons without ammo use their
    // authored `_na` icon instead of disappearing from the carousel.
    let visible: Vec<usize> = (1..=13)
        .filter(|id| selection.owned_bits & (1_u32 << *id) != 0)
        .collect();
    let Some(center) = visible.iter().position(|id| *id == selection.selected as usize) else { return; };
    let side = if visible.len() > 6 { (3, 3) } else {
        ((visible.len()-1)/2, visible.len()/2)
    };
    let scale = h as f32 / 480.0;
    let mut painter = Painter {
        out, images: &ui.extension_images, shader_flags: &ui.extension_shader_flags, uvs: &ui.extension_image_uvs,
        screen: [w, h], origin: [w as f32 * 0.5 - 320.0 * scale, 0.0],
        unit: scale,
    };
    let mut draw = |weapon: usize, x: f32, y: f32, size: f32| {
        let no_ammo = selection.ammo_available_bits & (1 << weapon) == 0;
        painter.image(if no_ammo { JK2_WEAPON_ICONS_EMPTY[weapon] }
            else { JK2_WEAPON_ICONS[weapon] }, x, y, size, size, WHITE);
    };
    for i in 1..=side.0 {
        draw(visible[(center + visible.len() - i) % visible.len()],
            228.0 - (i as f32 - 1.0) * 52.0, 420.0, 40.0);
    }
    draw(selection.selected as usize, 280.0, 400.0, 80.0);
    for i in 1..=side.1 {
        draw(visible[(center + i) % visible.len()],
            372.0 + (i as f32 - 1.0) * 52.0, 420.0, 40.0);
    }
    select_label(out, JK2_WEAPON_NAMES[selection.selected as usize],
        [0.875, 0.718, 0.121, 1.0], font, w, h);
}

/// Native JK2 `HI_*` inventory tags / icons (JKA's tags are different).
pub(in crate::ui) fn build_inventory_select(
    out: &mut Vec<UiVertex>, ui: &UiSnapshot,
    font: Option<&crate::ui::ProportionalFont>, w: u32, h: u32,
) {
    if ui.native_replaced(crate::extensions::NativeHud::InventorySelect) { return; }
    let Some(selection) = ui.inventory_select else { return; };
    if selection.remaining_ms <= 130 || !(1..=6).contains(&selection.selected) { return; }
    let visible: Vec<usize> = (1..=6).filter(|id| {
        JK2_ITEM_ICONS[*id].len() > 0 && selection.owned_bits & (1_u32 << *id) != 0
    }).collect();
    let Some(center) = visible.iter().position(|id| *id == selection.selected as usize) else { return; };
    let left = (visible.len()-1).min(6) / 2;
    let right = (visible.len()-1).min(6) - left;
    let scale = h as f32 / 480.0;
    let mut painter = Painter {
        out, images: &ui.extension_images, shader_flags: &ui.extension_shader_flags, uvs: &ui.extension_image_uvs,
        screen: [w, h], origin: [w as f32 * 0.5 - 320.0 * scale, 0.0],
        unit: scale,
    };
    for i in 1..=left {
        painter.image(JK2_ITEM_ICONS[visible[(center + visible.len()-i) % visible.len()]],
            224.0 - (i as f32 - 1.0)*56.0, 420.0, 40.0, 40.0, WHITE);
    }
    painter.image(JK2_ITEM_ICONS[selection.selected as usize], 280.0, 400.0, 80.0, 80.0, WHITE);
    for i in 1..=right {
        painter.image(JK2_ITEM_ICONS[visible[(center+i) % visible.len()]],
            376.0 + (i as f32 - 1.0)*56.0, 420.0, 40.0, 40.0, WHITE);
    }
    select_label(out, JK2_ITEM_NAMES[selection.selected as usize],
        [0.312, 0.75, 0.621, 1.0], font, w, h);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jk2_native_assets_have_unique_source_paths() {
        let mut images = vec!["my/custom/hud".to_owned()];
        append_jk2_hud_images(&mut images, 0);
        assert_eq!(images.len(), 32); // custom HUD still needs 31 native selector icons
        assert!(images.iter().any(|name| name == "gfx/hud/w_icon_lightsaber"));
        assert!(images.iter().any(|name| name == "gfx/hud/w_icon_tripmine"));
        assert!(!images.iter().any(|name| name == LEFT_FRAME));
        append_jk2_hud_images(&mut images, 1);
        assert_eq!(images.len(), 76); // unique JK2 art, including carousel backgrounds and prongs
        assert_eq!(images[0], "my/custom/hud");
        append_jk2_hud_images(&mut images, 1);
        assert_eq!(images.len(), 76);
    }

    #[test]
    fn jk2_selector_chrome_uses_the_original_prongs_and_open_close_timing() {
        let mut ui = UiSnapshot::default();
        ui.jk2_mode = true;
        ui.video.hud_style = 1;
        ui.video.draw_hud = true;
        append_jk2_hud_images(&mut ui.extension_images, 1);
        let mut out = Vec::new();
        ui.force_select = Some(crate::ui::UiForceSelect {
            selected: 3, known_bits: 1 << 3, remaining_ms: 1530,
        });
        build_select_chrome(&mut out, &ui, 1280, 720);
        assert_eq!(out.len(), 12); // two prongs, collapsed background
        out.clear();
        ui.force_select.as_mut().unwrap().remaining_ms = 1465;
        build_select_chrome(&mut out, &ui, 1280, 720);
        assert_eq!(out.len(), 48); // six background slices + two prongs
        out.clear();
        ui.force_select.as_mut().unwrap().remaining_ms = 65;
        build_select_chrome(&mut out, &ui, 1280, 720);
        assert_eq!(out.len(), 48); // six background slices + two off prongs
    }

    #[test]
    fn jk2_selector_chrome_uses_authored_colors_when_active() {
        let mut ui = UiSnapshot::default();
        ui.jk2_mode = true;
        ui.video.hud_style = 1;
        ui.video.draw_hud = true;
        ui.video.tint_hud = true;
        append_jk2_hud_images(&mut ui.extension_images, 1);
        ui.hud = Some(crate::ui::HudState {
            gametype: crate::cgame::GT_TEAM,
            team: crate::cgame::TEAM_RED,
            ..Default::default()
        });
        let mut vertices = Vec::new();
        // Both idle/off prongs are team tinted.
        build_select_chrome(&mut vertices, &ui, 1280, 720);
        assert_eq!(vertices.len(), 12);
        assert!(vertices.iter().all(|v| v.color == [255, 128, 128, 255]));

        // Opening prongs AND the stretched holographic backing must keep
        // their original colors, even in red/blue team games.
        ui.weapon_select = Some(crate::ui::UiWeaponSelect {
            selected: 2, owned_bits: 1 << 2, selectable_bits: 1 << 2,
            ammo_available_bits: 1 << 2, saber_style: 0, remaining_ms: 1465,
        });
        vertices.clear();
        build_select_chrome(&mut vertices, &ui, 1280, 720);
        assert_eq!(vertices.len(), 48);
        assert!(vertices.iter().all(|v| v.color == [255; 4]));

        ui.hud.as_mut().unwrap().team = crate::cgame::TEAM_BLUE;
        vertices.clear();
        build_select_chrome(&mut vertices, &ui, 1280, 720);
        assert!(vertices.iter().all(|v| v.color == [255; 4]));

        // Closing still has a visible background and uses the off-prong
        // image, but stays untinted until the selection is fully idle.
        ui.weapon_select.as_mut().unwrap().remaining_ms = 65;
        vertices.clear();
        build_select_chrome(&mut vertices, &ui, 1280, 720);
        assert_eq!(vertices.len(), 48);
        assert!(vertices.iter().all(|v| v.color == [255; 4]));

        // Once idle, team-tinted off prongs return; cg_tintHud off and
        // non-team gametypes keep the original white modulation.
        ui.weapon_select = None;
        vertices.clear();
        build_select_chrome(&mut vertices, &ui, 1280, 720);
        assert!(vertices.iter().all(|v| v.color == [128, 128, 255, 255]));
        ui.video.tint_hud = false;
        vertices.clear();
        build_select_chrome(&mut vertices, &ui, 1280, 720);
        assert!(vertices.iter().all(|v| v.color == [255; 4]));
        ui.video.tint_hud = true;
        ui.hud.as_mut().unwrap().gametype = 0; // GT_FFA
        vertices.clear();
        build_select_chrome(&mut vertices, &ui, 1280, 720);
        assert!(vertices.iter().all(|v| v.color == [255; 4]));
    }

    #[test]
    fn jk2_left_frame_uses_the_existing_hudleft_artwork() {
        // gfx/hud/hudleft.tga is loaded by the implicit-image qpath below.
        // Do not register the unrelated static_test instead.
        assert_eq!(LEFT_FRAME, "gfx/hud/hudleft");
        let mut images = Vec::new();
        append_jk2_hud_images(&mut images, 1);
        assert!(images.iter().any(|name| name == "gfx/hud/hudleft"));
        assert!(!images.iter().any(|name| name == "gfx/hud/static_test"));
    }

    #[test]
    fn jk2_chrome_key_tracks_visible_animation_steps_only() {
        let mut ui = UiSnapshot::default();
        ui.jk2_mode = true;
        ui.video.hud_style = 1;
        ui.video.draw_hud = true;
        assert_eq!(chrome_key(&ui), Some((0, 0, 0, false)));
        ui.force_select = Some(crate::ui::UiForceSelect {
            selected: 3, known_bits: 1 << 3, remaining_ms: 1530,
        });
        assert_eq!(chrome_key(&ui), Some((2, 0, 0, false)));
        ui.force_select.as_mut().unwrap().remaining_ms = 1529;
        assert_eq!(chrome_key(&ui), Some((2, 0, 0, false)));
        ui.force_select.as_mut().unwrap().remaining_ms = 1400;
        assert_eq!(chrome_key(&ui), Some((2, 8, 60, false)));
        ui.force_select.as_mut().unwrap().remaining_ms = 130;
        assert_eq!(chrome_key(&ui), Some((2, 8, 60, true)));
        ui.force_select.as_mut().unwrap().remaining_ms = 0;
        assert_eq!(chrome_key(&ui), Some((0, 0, 0, false)));
    }

    #[test]
    fn jk2_status_frames_are_drawn_after_the_prongs_in_the_same_batch() {
        let mut ui = UiSnapshot::default();
        ui.jk2_mode = true;
        ui.video.hud_style = 1;
        ui.video.draw_hud = true;
        ui.video.draw_2d = true;
        ui.hud = Some(crate::ui::HudState {
            health: 100, max_health: 100, armor: 50,
            weapon: 2, saber_style: 2, force_power: Some(75),
            ..Default::default()
        });
        append_jk2_hud_images(&mut ui.extension_images, 1);
        let mut chrome = Vec::new();
        let mut status = Vec::new();
        let mut combined = Vec::new();
        build_select_chrome(&mut chrome, &ui, 1280, 720);
        build(&mut status, &ui, 1280, 720);
        crate::ui::compose::build_dynamic_static_vertices(&mut combined, &ui, 1280, 720);
        assert_eq!(chrome.len(), 12);
        assert!(!status.is_empty());
        assert_eq!(combined.len(), chrome.len() + status.len());
        assert_eq!(combined[0].position, chrome[0].position);
        assert_eq!(combined[chrome.len()].position, status[0].position);
    }

    #[test]
    fn extension_can_hide_jk2_prongs_without_hiding_selection_icons() {
        let mut ui = UiSnapshot::default();
        ui.video.draw_hud = true;
        ui.jk2_mode = true;
        ui.video.hud_style = 1;
        append_jk2_hud_images(&mut ui.extension_images, 1);
        ui.weapon_select = Some(crate::ui::UiWeaponSelect {
            selected: 2, owned_bits: 1 << 2, selectable_bits: 1 << 2,
            ammo_available_bits: 1 << 2, saber_style: 0, remaining_ms: 1000,
        });
        let mut chrome = Vec::new();
        let mut icons = Vec::new();
        build_select_chrome(&mut chrome, &ui, 1280, 720);
        build_weapon_select(&mut icons, &ui, None, 1280, 720);
        assert!(!chrome.is_empty() && !icons.is_empty());

        ui.native_hidden = crate::extensions::NativeHudMask::from_names(
            &["jk2_prongs".to_string()]).unwrap();
        chrome.clear();
        icons.clear();
        build_select_chrome(&mut chrome, &ui, 1280, 720);
        build_weapon_select(&mut icons, &ui, None, 1280, 720);
        assert!(chrome.is_empty());
        assert!(!icons.is_empty());

        ui.native_hidden = crate::extensions::NativeHudMask::from_names(
            &["weapon_select".to_string()]).unwrap();
        chrome.clear();
        icons.clear();
        build_select_chrome(&mut chrome, &ui, 1280, 720);
        build_weapon_select(&mut icons, &ui, None, 1280, 720);
        assert!(!chrome.is_empty());
        assert!(icons.is_empty());

        ui.native_hidden = crate::extensions::NativeHudMask::from_names(
            &["weapon_select".to_string(), "jk2_prongs".to_string()]).unwrap();
        chrome.clear();
        icons.clear();
        build_select_chrome(&mut chrome, &ui, 1280, 720);
        build_weapon_select(&mut icons, &ui, None, 1280, 720);
        assert!(chrome.is_empty() && icons.is_empty());
        // With the extension hidden by cg_drawHud 0, the weapon selector
        // returns to native behavior (but JK2's prongs obey cg_drawHud).
        ui.video.draw_hud = false;
        build_weapon_select(&mut icons, &ui, None, 1280, 720);
        assert!(!icons.is_empty());
    }

    #[test]
    fn jk2_hud_frames_and_selector_prongs_do_not_stretch_on_widescreen() {
        // 4:3, 16:9, the reported 1649x928 screenshot, and ultrawide.
        // Native HUD frames and selector prongs use the same height-based unit;
        // only their positioning is anchored to opposite screen edges.
        for (w, h) in [(640, 480), (1280, 720), (1649, 928), (3440, 1440)] {
            let mut ui = UiSnapshot::default();
            ui.jk2_mode = true;
            ui.video.hud_style = 1;
            ui.video.draw_hud = true;
            append_jk2_hud_images(&mut ui.extension_images, 1);
            let context = HudRectContext::from_snapshot(&ui);
            let left = hud_element_rect(HudElementId::Health, ui.hud_layout.health, &context, w, h);
            let right = hud_element_rect(HudElementId::Ammo, ui.hud_layout.ammo, &context, w, h);
            let unit = super::super::layout::hud_art_scale(w, h);
            let size = 80.0 * unit;
            assert!((left.x).abs() < 0.01);
            assert!((left.width - size).abs() < 0.01);
            assert!((left.height - size).abs() < 0.01);
            assert!((right.x - (w as f32 - size)).abs() < 0.01);
            assert!((right.width - right.height).abs() < 0.01);

            let px = |ndc: f32| (ndc + 1.0) * w as f32 * 0.5;
            let py = |ndc: f32| (1.0 - ndc) * h as f32 * 0.5;
            let mut out = Vec::new();
            build_select_chrome(&mut out, &ui, w, h);
            assert_eq!(out.len(), 12); // left and right prongs, six vertices each
            assert!((px(out[0].position[0]) - 67.0 * unit).abs() < 0.01);
            assert!((px(out[6].position[0]) - (w as f32 - 106.0 * unit)).abs() < 0.01);
            assert!((px(out[2].position[0]) - px(out[0].position[0]) - 40.0 * unit).abs() < 0.01);
            assert!((px(out[8].position[0]) - px(out[6].position[0]) - 40.0 * unit).abs() < 0.01);
            assert!((py(out[0].position[1]) - (h as f32 - size)).abs() < 0.01);
            assert!((py(out[2].position[1]) - py(out[0].position[1]) - size).abs() < 0.01);
            assert!(px(out[0].position[0]) < left.x + left.width);
            assert!(px(out[6].position[0]) < right.x);

            ui.force_select = Some(crate::ui::UiForceSelect {
                selected: 3, known_bits: 1 << 3, remaining_ms: 1400,
            });
            out.clear();
            build_select_chrome(&mut out, &ui, w, h);
            assert_eq!(out.len(), if w as f32 > 640.0 * unit { 48 } else { 24 });
            assert!((px(out[0].position[0]) - 90.0 * unit).abs() < 0.01);
        }
    }

    #[test]
    fn jk2_weapon_labels_match_native_ids() {
        assert_eq!(JK2_WEAPON_NAMES[2], "Lightsaber");
        assert_eq!(JK2_WEAPON_NAMES[11], "Thermal Detonator");
        assert_eq!(JK2_WEAPON_NAMES[12], "Trip Mine");
        assert_eq!(JK2_WEAPON_ICONS[2], "gfx/hud/w_icon_lightsaber");
        assert_eq!(JK2_WEAPON_ICONS[12], "gfx/hud/w_icon_tripmine");
    }

    #[test]
    fn jk2_native_weapon_and_inventory_selectors_are_not_jka_ids() {
        let mut ui = UiSnapshot::default();
        ui.jk2_mode = true;
        ui.video.hud_style = 1;
        append_jk2_hud_images(&mut ui.extension_images, 1);
        let mut out = Vec::new();
        ui.weapon_select = Some(crate::ui::UiWeaponSelect {
            selected: 2, owned_bits: 1 << 2, selectable_bits: 1 << 2,
            ammo_available_bits: 1 << 2, saber_style: 0, remaining_ms: 1000,
        });
        build_weapon_select(&mut out, &ui, None, 1280, 720);
        assert!(!out.is_empty());
        out.clear();
        ui.inventory_select = Some(crate::ui::UiInventorySelect {
            selected: 3, owned_bits: 1 << 3, remaining_ms: 1000,
        });
        build_inventory_select(&mut out, &ui, None, 1280, 720);
        assert!(!out.is_empty());
    }

    #[test]
    fn jk2_weapon_ids_do_not_use_jka_ammo_mapping() {
        assert_eq!(jk2_ammo_index(2), 0); // JK2 saber
        assert_eq!(jk2_ammo_index(3), 2); // JK2 Bryar
        assert_eq!(jk2_ammo_index(5), 3); // Disruptor
        assert_eq!(jk2_ammo_index(7), 4); // Repeater
        assert_eq!(jk2_ammo_index(10), 5); // Rocket launcher
        assert_eq!(jk2_ammo_index(13), 9); // Det pack
    }

    #[test]
    fn jk2_hud_emits_native_geometry() {
        let mut ui = UiSnapshot::default();
        ui.video.hud_style = 1;
        ui.jk2_mode = true;
        ui.hud = Some(crate::ui::HudState {
            health: 100, max_health: 100, armor: 50,
            weapon: 2, saber_style: 2, force_power: Some(75),
            ..Default::default()
        });
        append_jk2_hud_images(&mut ui.extension_images, 1);
        let mut out = Vec::new();
        build(&mut out, &ui, 1280, 720);
        assert!(!out.is_empty());
    }
}
