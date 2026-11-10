//! Fast, read-only Rhai HUD draw-command consumer. Shares the existing UI pass.
use crate::extensions::{hud_widget_origin, HudCommand, NativeHud, NativeHudMask, PlacedHudWidget, MAX_HUD_IMAGES, MAX_HUD_COMMANDS};
use crate::ui::{disc, rect, text, textured_rect_with_source, HudElementId, UiSnapshot, UiVertex};

impl HudElementId {
    /// The extension-replaceable group this element belongs to, if any.
    pub(in crate::ui) fn native(self) -> Option<NativeHud> {
        match self {
            Self::Health => Some(NativeHud::Health),
            Self::Shield => Some(NativeHud::Shield),
            Self::Ammo => Some(NativeHud::Ammo),
            Self::Force => Some(NativeHud::Force),
            Self::Speedometer | Self::SpeedometerJumps | Self::SpeedGraph
            | Self::AccelMeter | Self::GroundSpeed | Self::JumpHeight | Self::JumpDistance
            | Self::VerticalSpeed | Self::YawSpeed => Some(NativeHud::Speedometer),
            _ => None,
        }
    }
}

impl UiSnapshot {
    /// True when a running extension has replaced this native element. Extension
    /// widgets only draw under `cg_drawHud`, so the native element comes back
    /// with it: the user's setting never ends up hiding both.
    pub(in crate::ui) fn native_replaced(&self, element: NativeHud) -> bool {
        self.video.draw_hud
            && (self.native_hidden.contains(element) || self.broadcast_replaces(element))
    }

    /// cg_specHud observer card: it carries the observed player's health,
    /// armor, ammo and Force, so the native corner meters step aside.
    pub(in crate::ui) fn broadcast_replaces(&self, element: NativeHud) -> bool {
        matches!(element, NativeHud::Health | NativeHud::Shield | NativeHud::Ammo | NativeHud::Force)
            && self.spectator_broadcast.as_ref().is_some_and(|b| b.replaces_native_vitals())
    }

}

/// `native_replaced` for callers that only hold the mask and `cg_drawHud` (HUD Edit).
pub(crate) fn hud_element_replaced(mask: NativeHudMask, draw_hud: bool, id: HudElementId) -> bool {
    draw_hud && id.native().is_some_and(|element| mask.contains(element))
}

/// `ui.wgsl` texture source of the extension image atlas (binding 7).
pub const EXT_IMAGE_TEXTURE_SOURCE: f32 = 7.0;
/// Same atlas, read with textureLoad for crisp pixel-art sprite regions.
pub const EXT_IMAGE_REGION_TEXTURE_SOURCE: f32 = 7.25;
// Actual source-size UV rectangles come from the same variable-size packer used
// by native HUD weapon icons, not from a 256px grid.
/// Texture-source marker for an authored HUD image, distinct from native icons.
pub(crate) const EXT_IMAGE_SHADER_SOURCE_BASE: f32 = 8192.0;
/// Unique painter-order tag for each shaded image draw across widgets.
pub(crate) const EXT_USER_SHADER_SOURCE_BASE: f32 = 16384.0;

pub(in crate::ui) fn build_extension_hud(
    ui: &UiSnapshot, out: &mut Vec<UiVertex>, widgets: &[PlacedHudWidget], images: &[String],
    shader_flags: &[bool], image_dimensions: &[Option<[u32; 2]>],
    image_uvs: &[([f32; 2], [f32; 2])], width: u32, height: u32,
) {
    if width == 0 || height == 0 { return; }
    // Reserve space in the preallocated UI dynamic buffer for the stock HUD.
    // Hard ceiling is 4096 extension vertices (1365 triangles), total.
    const MAX_EXTENSION_VERTICES: usize = 4095;
    const UI_DYNAMIC_BUFFER_BYTES: usize = 512 * 1024;
    let extension_start = out.len();
    let gpu_capacity = UI_DYNAMIC_BUFFER_BYTES / std::mem::size_of::<UiVertex>();
    let available = gpu_capacity.saturating_sub(extension_start);
    let limit = MAX_EXTENSION_VERTICES.min(available - available % 3);
    for (widget_index, placed) in widgets.iter().take(64).enumerate() {
        if out.len() - extension_start >= limit { break; }
        let p = placed.placement;
        if !p.visible || !p.scale.is_finite() || p.scale <= 0.0 { continue; }
        let widget = &placed.widget;
        let max_x = widget.width;
        let max_y = widget.height;
        let start = out.len();
        for (command_index, command) in widget.commands.iter().enumerate() {
            if out.len() - extension_start >= limit { break; }
            match command {
                HudCommand::Text { text: value, x, y, size, color } => {
                    if *x < 0.0 || *y < 0.0 || *x >= max_x || *y + 8.0 * size > max_y { continue; }
                    // The HUD atlas is a 6px monospace font. Clip text by chars
                    // before emitting geometry, including UTF-8/JKA safety.
                    let max_chars = ((max_x - x) / (6.0 * size)).max(0.0) as usize;
                    if max_chars == 0 { continue; }
                    let clipped: String = value.chars().take(max_chars.min(256)).collect();
                    text(out, &clipped, *x, *y, *size, *color, width, height);
                }
                HudCommand::Rect { x, y, w, h, color } => {
                    let x0 = x.max(0.0); let y0 = y.max(0.0);
                    let x1 = (*x + *w).min(max_x); let y1 = (*y + *h).min(max_y);
                    if x1 > x0 && y1 > y0 { rect(out, x0, y0, x1-x0, y1-y0, *color, width, height); }
                }
                HudCommand::Line { x1, y1, x2, y2, thickness, color } => {
                    // Clamp endpoints to the widget rectangle. A true segment
                    // clip is not required for the v1 bounded line primitives.
                    let (x1, y1, x2, y2) = (
                        x1.clamp(0.0, max_x), y1.clamp(0.0, max_y),
                        x2.clamp(0.0, max_x), y2.clamp(0.0, max_y),
                    );
                    let dx=x2-x1; let dy=y2-y1;
                    let len=(dx*dx + dy*dy).sqrt();
                    if len < 0.001 { continue; }
                    let nx = -dy / len * thickness * 0.5;
                    let ny = dx / len * thickness * 0.5;
                    let point = |x: f32,y: f32| UiVertex {
                        position: [x / width as f32 * 2.0 - 1.0, 1.0 - y / height as f32 * 2.0],
                        uv: [0.0, 0.0], color: UiVertex::pack_color(*color), textured: 0.0,
                    };
                    let a=point(x1+nx,y1+ny); let b=point(x1-nx,y1-ny);
                    let c=point(x2-nx,y2-ny); let d=point(x2+nx,y2+ny);
                    out.extend_from_slice(&[a,b,c,a,c,d]);
                }
                HudCommand::Image { name, x, y, w, h, color }
                | HudCommand::ShaderImage { name, x, y, w, h, color, .. }
                | HudCommand::ImageRegion { name, x, y, w, h, color, .. }
                | HudCommand::NativeIcon { category: name, id: _, x, y, w, h, color } => {
                    let native_cell = if let HudCommand::NativeIcon { category, id, .. } = command {
                        crate::ui::native_hud_icon_index(category, i64::from(*id))
                    } else { None };
                    let image_index = if native_cell.is_none() {
                        images.iter().position(|slot| slot == name)
                    } else { None };
                    if native_cell.is_none() && image_index.is_none() { continue; }
                    // A negative size mirrors the image, as menu rects do.
                    let (rx, flip_x) = if *w < 0.0 { (*x + *w, true) } else { (*x, false) };
                    let (ry, flip_y) = if *h < 0.0 { (*y + *h, true) } else { (*y, false) };
                    let (rw, rh) = (w.abs(), h.abs());
                    let (x0, y0) = (rx.max(0.0), ry.max(0.0));
                    let (x1, y1) = ((rx + rw).min(max_x), (ry + rh).min(max_y));
                    if x1 <= x0 || y1 <= y0 { continue; }
                    let (mut cell0, mut cell1) = if let Some(cell) = native_cell {
                        ui.icon_uv(cell)
                    } else if image_index.is_some_and(|i| shader_flags.get(i).copied().unwrap_or(false)) {
                        // Authored materials render standalone, in their own 0..1 UV space.
                        ([0.0, 0.0], [1.0, 1.0])
                    } else {
                        image_index.and_then(|i| image_uvs.get(i).copied())
                            .unwrap_or(([0.0, 0.0], [0.0, 0.0]))
                    };
                    if let HudCommand::ImageRegion { source, .. } = command {
                        let [sx, sy, sw, sh] = *source;
                        let Some([image_w, image_h]) = image_index.and_then(|i| image_dimensions.get(i)).and_then(|v| *v) else { continue };
                        if image_w == 0 || image_h == 0
                            || image_index.is_some_and(|i| shader_flags.get(i).copied().unwrap_or(false))
                            || sx + sw > image_w as f32 || sy + sh > image_h as f32 { continue; }
                        let old0 = cell0;
                        let old1 = cell1;
                        let width_uv = old1[0] - old0[0];
                        let height_uv = old1[1] - old0[1];
                        cell0 = [old0[0] + width_uv * sx / image_w as f32,
                                 old0[1] + height_uv * sy / image_h as f32];
                        cell1 = [old0[0] + width_uv * (sx + sw) / image_w as f32,
                                 old0[1] + height_uv * (sy + sh) / image_h as f32];
                    }
                    let axis = |lo: f32, hi: f32, origin: f32, size: f32, flip: bool, c0: f32, c1: f32| {
                        let (a, b) = ((lo - origin) / size, (hi - origin) / size);
                        let (a, b) = if flip { (1.0 - a, 1.0 - b) } else { (a, b) };
                        (c0 + (c1 - c0) * a, c0 + (c1 - c0) * b)
                    };
                    let (u0, u1) = axis(x0, x1, rx, rw, flip_x, cell0[0], cell1[0]);
                    let (v0, v1) = axis(y0, y1, ry, rh, flip_y, cell0[1], cell1[1]);
                    textured_rect_with_source(
                        out, x0, y0, x1 - x0, y1 - y0, [u0, v0], [u1, v1], *color,
                        if let Some(cell) = native_cell {
                            ui.icon_texture_source(cell)
                        } else if matches!(command, HudCommand::ShaderImage { .. }) {
                            EXT_USER_SHADER_SOURCE_BASE
                                + (widget_index * MAX_HUD_COMMANDS + command_index) as f32
                        } else if matches!(command, HudCommand::ImageRegion { .. }) {
                            EXT_IMAGE_REGION_TEXTURE_SOURCE
                        } else if image_index.is_some_and(|i| shader_flags.get(i).copied().unwrap_or(false)) {
                            EXT_IMAGE_SHADER_SOURCE_BASE + image_index.unwrap_or(0) as f32
                        } else { EXT_IMAGE_TEXTURE_SOURCE },
                        width, height,
                    );
                }
                HudCommand::Circle { x, y, radius, color } => {
                    if *x - *radius < 0.0 || *x + *radius > max_x || *y - *radius < 0.0 || *y + *radius > max_y {
                        continue; // no unbounded circle geometry outside the widget
                    }
                    disc(out, *x, *y, *radius, 32, *color, width, height);
                }
            }
        }
        // Do not permit an extension to grow the dynamic HUD buffer without
        // bound, even when it fills every primitive slot with circles/text.
        let remaining = limit.saturating_sub(start - extension_start);
        let after = (out.len() - start).min(remaining);
        out.truncate(start + after - after % 3);
        // Widget-local coordinates -> framebuffer pixels -> NDC.
        let wf=width as f32; let hf=height as f32;
        let (ox, oy) = hud_widget_origin(widget.anchor, p, widget.width, widget.height, wf, hf);
        for v in &mut out[start..] {
            let x=(v.position[0]+1.0)*0.5*wf;
            let y=(1.0-v.position[1])*0.5*hf;
            v.position=[(ox+x*p.scale)/wf*2.0-1.0, 1.0-(oy+y*p.scale)/hf*2.0];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{HudPlacement, HudWidget};
    #[test]
    fn widget_rect_uses_existing_ui_triangles() {
        let widget = PlacedHudWidget {
            widget: HudWidget { key: "test/rect".into(), name: "Rect".into(),
                width: 50.0, height: 40.0, anchor: Default::default(),
                commands: vec![HudCommand::Rect { x: 2.0, y: 3.0, w: 20.0, h: 10.0, color: [1.0; 4] }] },
            placement: HudPlacement { x: 30.0, y: 25.0, scale: 2.0, visible: true },
        };
        let mut out=Vec::new();
        build_extension_hud(&UiSnapshot::default(), &mut out, &[widget], &[], &[], &[], &[], 640, 480);
        assert_eq!(out.len(), 6);
        assert!((out[0].position[0] - ((34.0 / 640.0) * 2.0 - 1.0)).abs() < 0.0001);
    }

    fn image_widget(commands: Vec<HudCommand>, anchor: crate::extensions::HudAnchor) -> PlacedHudWidget {
        PlacedHudWidget {
            widget: HudWidget { key: "t/i".into(), name: "I".into(), width: 100.0, height: 50.0,
                anchor, commands },
            placement: HudPlacement::for_anchor(anchor),
        }
    }

    fn image(name: &str, x: f32, y: f32, w: f32, h: f32) -> HudCommand {
        HudCommand::Image { name: name.into(), x, y, w, h, color: [1.0; 4] }
    }

    #[test]
    fn native_icons_use_shared_atlas_even_with_zero_extension_images() {
        let mut ui = UiSnapshot::default();
        ui.icon_atlas_uvs = vec![([0.2, 0.3], [0.4, 0.5]); crate::ui::ICON_NAMES.len()];
        let widget = image_widget(vec![HudCommand::NativeIcon {
            category: "force".into(), id: 2, x: 0.0, y: 0.0,
            w: 20.0, h: 20.0, color: [1.0; 4],
        }], crate::extensions::HudAnchor::TopLeft);
        let mut out = Vec::new();
        build_extension_hud(&ui, &mut out, &[widget], &[], &[], &[], &[], 640, 480);
        assert_eq!(out.len(), 6);
        assert_eq!(out[0].uv, [0.2, 0.3]);
        assert_eq!(out[0].textured, crate::ui::hud::movement::ICON_TEXTURE_SOURCE);
    }

    #[test]
    fn extension_wgsl_tags_are_stable_and_distinct_from_static_images() {
        let shaded = HudCommand::ShaderImage {
            name: "gfx/noise".into(), shader: "test/liquid".into(),
            x: 1.0, y: 2.0, w: 20.0, h: 20.0, color: [1.0; 4],
            params: [0.75, 0.0, 0.0, 0.0],
        };
        let widget = image_widget(vec![image("gfx/noise", 0.0, 0.0, 5.0, 5.0), shaded],
            crate::extensions::HudAnchor::TopLeft);
        let mut out = Vec::new();
        build_extension_hud(&UiSnapshot::default(), &mut out, &[widget], &["gfx/noise".into()], &[], &[], &[([0.1, 0.2], [0.3, 0.4])], 640, 480);
        assert_eq!(out.len(), 12);
        assert_eq!(out[0].textured, EXT_IMAGE_TEXTURE_SOURCE);
        assert_eq!(out[6].textured, EXT_USER_SHADER_SOURCE_BASE + 1.0);
    }

    #[test]
    fn images_use_the_extension_atlas_and_sorted_cells() {
        let widget = image_widget(
            vec![image("gfx/b", 0.0, 0.0, 10.0, 10.0), image("gfx/a", 20.0, 0.0, 10.0, 10.0)],
            crate::extensions::HudAnchor::TopLeft);
        let mut out = Vec::new();
        let slots = ["gfx/a".to_owned(), "gfx/b".to_owned()];
        build_extension_hud(&UiSnapshot::default(), &mut out, &[PlacedHudWidget { placement: HudPlacement { x: 0.0, y: 0.0, ..widget.placement }, ..widget }], &slots, &[], &[], &[([0.1, 0.2], [0.3, 0.4]), ([0.5, 0.6], [0.8, 0.9])], 640, 480);
        assert_eq!(out.len(), 12);
        assert!(out.iter().all(|v| v.textured == EXT_IMAGE_TEXTURE_SOURCE));
        // Lookups must use each source-native rectangle, never fixed grid math.
        assert_eq!(out[0].uv, [0.5, 0.6]);
        assert_eq!(out[6].uv, [0.1, 0.2]);
        let mut more = slots.to_vec();
        more.push("gfx/c".to_owned());
        let mut again = Vec::new();
        let w2 = image_widget(vec![image("gfx/b", 0.0, 0.0, 10.0, 10.0)], crate::extensions::HudAnchor::TopLeft);
        build_extension_hud(&UiSnapshot::default(), &mut again, &[PlacedHudWidget { placement: HudPlacement { x: 0.0, y: 0.0, ..w2.placement }, ..w2 }], &more, &[], &[], &[([0.1, 0.2], [0.3, 0.4]), ([0.5, 0.6], [0.8, 0.9])], 640, 480);
        assert_eq!(again[0].uv, out[0].uv);
    }

    #[test]
    fn authored_shader_marks_only_its_own_image() {
        let widget = image_widget(
            vec![image("gfx/a", 0.0, 0.0, 10.0, 10.0),
                 image("gfx/b", 15.0, 0.0, 10.0, 10.0)],
            crate::extensions::HudAnchor::TopLeft,
        );
        let mut out = Vec::new();
        build_extension_hud(&UiSnapshot::default(), &mut out, &[widget],
            &["gfx/a".into(), "gfx/b".into()], &[true, false], &[],
            &[([0.12, 0.23], [0.34, 0.45]), ([0.56, 0.67], [0.78, 0.89])], 640, 480);
        assert!(out[0..6].iter().all(|v| v.textured == EXT_IMAGE_SHADER_SOURCE_BASE));
        assert!(out[6..12].iter().all(|v| v.textured == EXT_IMAGE_TEXTURE_SOURCE));
        let (uv0, uv1) = ([0.0, 0.0], [1.0, 1.0]);
        assert_eq!(out[0].uv, uv0);
        assert_eq!(out[2].uv, uv1);
    }

    #[test]
    fn mirrored_and_clipped_images_crop_and_flip_uvs() {
        let mut out = Vec::new();
        // Negative width mirrors; the 100-unit widget clips the right 20 units.
        let widget = image_widget(vec![image("gfx/a", 80.0, 0.0, -40.0, 10.0)], crate::extensions::HudAnchor::TopLeft);
        build_extension_hud(&UiSnapshot::default(), &mut out, &[PlacedHudWidget { placement: HudPlacement { x: 0.0, y: 0.0, ..widget.placement }, ..widget }], &["gfx/a".to_owned()], &[], &[], &[([0.2, 0.3], [0.6, 0.7])], 640, 480);
        assert_eq!(out.len(), 6);
        let (u_left, u_right) = (out[0].uv[0], out[2].uv[0]);
        assert!(u_left > u_right, "mirrored image must run right-to-left: {u_left} {u_right}");
        let width_px = (out[2].position[0] - out[0].position[0]) * 640.0 / 2.0;
        assert!((width_px - 40.0).abs() < 0.01, "{width_px}");
    }

    #[test]
    fn bottom_right_widgets_sit_in_the_corner() {
        let mut out = Vec::new();
        let widget = image_widget(vec![image("gfx/a", 0.0, 0.0, 100.0, 50.0)], crate::extensions::HudAnchor::BottomRight);
        build_extension_hud(&UiSnapshot::default(), &mut out, &[widget], &["gfx/a".to_owned()], &[], &[], &[([0.2, 0.3], [0.6, 0.7])], 640, 480);
        let right_px = (out[2].position[0] + 1.0) * 0.5 * 640.0;
        let bottom_px = (1.0 - out[2].position[1]) * 0.5 * 480.0;
        assert!((right_px - 640.0).abs() < 0.01 && (bottom_px - 480.0).abs() < 0.01, "{right_px} {bottom_px}");
    }
}
