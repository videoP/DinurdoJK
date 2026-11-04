//! Stock Jedi Academy multiplayer status HUD, natively emitted as UI triangles.
//!
//! Port of TaystJK `CG_DrawHUD`, `CG_DrawHealth`, `CG_DrawArmor`,
//! `CG_DrawAmmo`, `CG_DrawForcePower` and `CG_DrawSaberStyle`, using the item
//! coordinates from retail `ui/hud.menu`. No Rhai calls, menus or scripting.
//! Textures come from the mounted retail `gfx/hud` and `gfx/2d/numbers` art.
use crate::ui::{
    hud_element_rect, textured_rect_with_source, text, HudElementId, HudRectContext,
    HudState, UiSnapshot, UiVertex,
};
use super::extensions::{
    image_cell_uv_for_count, EXT_IMAGE_SHADER_SOURCE_BASE, EXT_IMAGE_TEXTURE_SOURCE,
};

// Each tuple is (qpath, x, y, width, height) in the lefthud/righthud menu.
// Righthud has negative X values, measured inward from the screen's right edge.
type Piece = (&'static str, f32, f32, f32, f32);
const HEALTH: [Piece; 4] = [
    ("gfx/hud/health_tic_1", 20.0, 24.0, 28.0, 28.0),
    ("gfx/hud/health_tic_2", 38.0, 33.0, 28.0, 28.0),
    ("gfx/hud/health_tic_3", 52.0, 47.0, 28.0, 28.0),
    ("gfx/hud/health_tic_4", 60.0, 65.0, 28.0, 28.0),
];
const ARMOR: [Piece; 4] = [
    ("gfx/hud/armor_tic_1", 9.0, -6.0, 56.0, 56.0),
    ("gfx/hud/armor_tic_2", 34.0, 3.0, 56.0, 56.0),
    ("gfx/hud/armor_tic_3", 54.0, 24.0, 56.0, 56.0),
    ("gfx/hud/armor_tic_4", 63.0, 48.0, 56.0, 56.0),
];
const AMMO: [Piece; 4] = [
    ("gfx/hud/ammo_tic_1", -48.0, 25.0, 28.0, 28.0),
    ("gfx/hud/ammo_tic_2", -66.0, 33.0, 28.0, 28.0),
    ("gfx/hud/ammo_tic_3", -80.0, 47.0, 28.0, 28.0),
    ("gfx/hud/ammo_tic_4", -88.0, 65.0, 28.0, 28.0),
];
const FORCE: [Piece; 4] = [
    ("gfx/hud/force_tic_1", -65.0, -5.0, 56.0, 56.0),
    ("gfx/hud/force_tic_2", -89.0, 4.0, 56.0, 56.0),
    ("gfx/hud/force_tic_3", -109.0, 24.0, 56.0, 56.0),
    ("gfx/hud/force_tic_4", -119.0, 48.0, 56.0, 56.0),
];
const STYLES: [Piece; 3] = [
    ("gfx/hud/saber_fast", -85.0, 69.0, 24.0, 24.0),
    ("gfx/hud/saber_med", -70.0, 43.0, 26.0, 26.0),
    ("gfx/hud/saber_strong", -49.0, 24.0, 26.0, 26.0),
];
const NUMBERS: [&str; 11] = [
    "gfx/2d/numbers/t_zero", "gfx/2d/numbers/t_one",
    "gfx/2d/numbers/t_two", "gfx/2d/numbers/t_three",
    "gfx/2d/numbers/t_four", "gfx/2d/numbers/t_five",
    "gfx/2d/numbers/t_six", "gfx/2d/numbers/t_seven",
    "gfx/2d/numbers/t_eight", "gfx/2d/numbers/t_nine",
    "gfx/2d/numbers/t_minus",
];
const AMMO_MAX: [i32; 10] = [0, 100, 300, 300, 300, 25, 800, 10, 10, 10];
const HEALTH_COLOR: [f32; 4] = [0.835, 0.015, 0.015, 1.0];
const ARMOR_COLOR: [f32; 4] = [0.0, 0.613, 0.097, 1.0];
const FORCE_COLOR: [f32; 4] = [0.359, 0.524, 0.722, 1.0];
const AMMO_COLOR: [f32; 4] = [1.0, 0.658, 0.062, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0; 4];
const BACKGROUND: &str = "gfx/hud/hudleft";

/// Called on all native UI-image registration paths (full snapshots, async
/// extension updates, HUD-editor moves). Original extension slots stay first.
/// No additional art is allocated/decoded while the native style is off.
pub(crate) fn append_retail_hud_images(images: &mut Vec<String>, style: u8) {
    if style != 1 { return; }
    let names = std::iter::once(BACKGROUND)
        .chain(HEALTH.iter().map(|p| p.0))
        .chain(ARMOR.iter().map(|p| p.0))
        .chain(AMMO.iter().map(|p| p.0))
        .chain(FORCE.iter().map(|p| p.0))
        .chain(STYLES.iter().map(|p| p.0))
        .chain(NUMBERS.iter().copied());
    for name in names {
        if !images.iter().any(|present| present == name) {
            images.push(name.to_owned());
        }
    }
}

struct Painter<'a> {
    out: &'a mut Vec<UiVertex>,
    images: &'a [String],
    shader_flags: &'a [bool],
    screen: [u32; 2],
    origin: [f32; 2],
    unit: f32,
    right: bool,
}

impl Painter<'_> {
    fn image(&mut self, name: &str, x: f32, y: f32, width: f32, height: f32, color: [f32; 4]) {
        let Some(index) = self.images.iter().position(|slot| slot == name) else { return; };
        let [sw, sh] = self.screen;
        let (mut uv0, mut uv1) = image_cell_uv_for_count(index, self.images.len());
        // The mirrored right-hand background uses a negative width in hud.menu.
        if width < 0.0 { std::mem::swap(&mut uv0[0], &mut uv1[0]); }
        if height < 0.0 { std::mem::swap(&mut uv0[1], &mut uv1[1]); }
        let x = self.origin[0] + x * self.unit;
        let y = self.origin[1] + (12.0 + y) * self.unit;
        textured_rect_with_source(
            self.out,
            x.min(x + width * self.unit), y.min(y + height * self.unit),
            width.abs() * self.unit, height.abs() * self.unit,
            uv0, uv1, color,
            if self.shader_flags.get(index).copied().unwrap_or(false) {
                EXT_IMAGE_SHADER_SOURCE_BASE + index as f32
            } else { EXT_IMAGE_TEXTURE_SOURCE },
            sw, sh,
        );
    }

    fn piece(&mut self, piece: Piece, color: [f32; 4]) {
        let (name, x, y, w, h) = piece;
        let local_x = self.local_x(x);
        self.image(name, local_x, y, w, h, color);
    }

    fn local_x(&self, x: f32) -> f32 { if self.right { 128.0 + x } else { x } }

    fn tics(&mut self, pieces: &[Piece; 4], value: i32, increment: i32,
            tint: [f32; 4], blink_last: bool) {
        let mut remaining = value;
        let increment = increment.max(1);
        for (i, &piece) in pieces.iter().enumerate().rev() {
            if remaining <= 0 { break; }
            let mut color = tint;
            if remaining < increment { color[3] *= remaining as f32 / increment as f32; }
            // TaystJK: under a quarter armor the outermost partial tick blinks.
            if !(blink_last && i == 3 && remaining < increment) {
                self.piece(piece, color);
            }
            remaining -= increment;
        }
    }

    fn number(&mut self, value: i32, x: f32, y: f32, color: [f32; 4]) {
        let value = value.clamp(-99, 999);
        let text = value.to_string();
        let mut x = x + 2.0 + 6.0 * (3 - text.len()) as f32;
        for ch in text.chars() {
            let name = if ch == '-' { NUMBERS[10] } else {
                NUMBERS[ch.to_digit(10).unwrap_or(0) as usize]
            };
            let local_x = self.local_x(x);
            self.image(name, local_x, y, 6.0, 12.0, color);
            x += 7.0;
        }
    }

    fn dashes(&mut self) {
        let scale = 2.0 * self.unit / 2.25;
        let x = self.origin[0] + self.local_x(-75.0) * self.unit;
        let y = self.origin[1] + (12.0 + 87.0) * self.unit;
        text(self.out, "--", x, y, scale, AMMO_COLOR, self.screen[0], self.screen[1]);
    }
}

pub(in crate::ui) fn build(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(hud) = ui.hud else { return; };
    if hud.spectating { return; }
    let ctx = HudRectContext::from_snapshot(ui);
    let tint = match hud.team {
        1 => [1.0, 0.5, 0.5, 1.0], // red team
        2 => [0.5, 0.5, 1.0, 1.0], // blue team
        _ => WHITE,
    };
    let max = hud.max_health.max(1);
    let s = h as f32 / 480.0;
    let images = &ui.extension_images;
    let shaders = &ui.extension_shader_flags;
    // Left cluster is driven by the Health editor handle; the right cluster
    // by Ammo. Shield and Force remain available in DinurdoJK style 0.
    if !ui.native_replaced(crate::extensions::NativeHud::Health) {
        let left = hud_element_rect(HudElementId::Health, ui.hud_layout.health, &ctx, w, h);
        let mut painter = Painter {
            out, images, shader_flags: shaders, screen: [w, h],
            origin: [left.x, left.y], unit: s * ui.hud_layout.health.scale.clamp(0.5, 2.0),
            right: false,
        };
        painter.image(BACKGROUND, 0.0, 0.0, 112.0, 112.0, tint);
        painter.tics(&HEALTH, hud.health, max / 4, tint, false);
        let blink = hud.armor > 0 && (hud.armor as f32) < max as f32 / 4.0
            && (hud.time_ms / 400).rem_euclid(2) != 0;
        painter.tics(&ARMOR, hud.armor, max / 4, tint, blink);
        painter.number(hud.armor, 85.0, 98.0, ARMOR_COLOR);
        painter.number(hud.health, 59.0, 98.0, HEALTH_COLOR);
    }
    if !ui.native_replaced(crate::extensions::NativeHud::Ammo) {
        let right = hud_element_rect(HudElementId::Ammo, ui.hud_layout.ammo, &ctx, w, h);
        let mut painter = Painter {
            out, images, shader_flags: shaders, screen: [w, h],
            origin: [right.x, right.y], unit: s * ui.hud_layout.ammo.scale.clamp(0.5, 2.0),
            right: true,
        };
        painter.image(BACKGROUND, 128.0, 0.0, -112.0, 112.0, tint);
        if let Some(force) = hud.force_power {
            painter.tics(&FORCE, force, 25, WHITE, false);
            painter.number(force, -109.0, 98.0,
                if hud.force_flash { RED } else { FORCE_COLOR });
        }
        if hud.weapon == 0 { return; }
        if (1..=3).contains(&hud.weapon) {
            painter.dashes();
            if hud.weapon == 3 && (1..=3).contains(&hud.saber_style) {
                painter.piece(STYLES[(hud.saber_style - 1) as usize], WHITE);
            }
        } else if let Some(ammo) = hud.ammo {
            let maximum = AMMO_MAX.get(hud.ammo_index).copied().unwrap_or(0);
            painter.tics(&AMMO, ammo, maximum / 4, tint, false);
            painter.number(ammo, -83.0, 98.0,
                if ammo > 0 { AMMO_COLOR } else { RED });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_hud_requires_exactly_31_names_and_preserves_extensions() {
        let mut images = vec!["gfx/custom/a".to_owned()];
        append_retail_hud_images(&mut images, 0);
        assert_eq!(images.len(), 1);
        append_retail_hud_images(&mut images, 1);
        assert_eq!(images.len(), 32);
        assert_eq!(images.first().unwrap(), "gfx/custom/a");
        append_retail_hud_images(&mut images, 1);
        assert_eq!(images.len(), 32);
    }

    #[test]
    fn classic_geometry_contains_hud_images_without_scripting() {
        let mut ui = UiSnapshot::default();
        ui.video.hud_style = 1;
        ui.hud = Some(HudState {
            health: 82, max_health: 100, armor: 63,
            ammo: Some(29), ammo_index: 3, weapon: 4,
            force_power: Some(70), ..Default::default()
        });
        append_retail_hud_images(&mut ui.extension_images, 1);
        let mut vertices = Vec::new();
        build(&mut vertices, &ui, 1920, 1080);
        assert!(vertices.len() > 6 * 10);
        assert!(vertices.iter().any(|v| v.textured == EXT_IMAGE_TEXTURE_SOURCE));
    }
}
