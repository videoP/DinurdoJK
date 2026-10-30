//! Small vector glyphs drawn from triangles: the leader's crown and the
//! client-side mute marker. No atlas art exists for either in the base game,
//! and geometry keeps them crisp at every resolution.
use crate::palette::hud as color;
use crate::ui::flex::{Container, Flex};
use crate::ui::{rect, UiVertex};
use taffy::prelude::AlignItems;

fn triangle(out: &mut Vec<UiVertex>, points: [[f32; 2]; 3], color: [f32; 4], width: u32, height: u32) {
    out.extend(points.map(|[x, y]| UiVertex {
        position: [x / width.max(1) as f32 * 2.0 - 1.0, 1.0 - y / height.max(1) as f32 * 2.0],
        uv: [0.0; 2],
        color,
        textured: 0.0,
    }));
}

/// A straight line of `thickness` px as two triangles.
fn segment(
    out: &mut Vec<UiVertex>,
    from: [f32; 2],
    to: [f32; 2],
    thickness: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let length = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (nx, ny) = (-dy / length * thickness * 0.5, dx / length * thickness * 0.5);
    let a = [from[0] + nx, from[1] + ny];
    let b = [from[0] - nx, from[1] - ny];
    let c = [to[0] - nx, to[1] - ny];
    let d = [to[0] + nx, to[1] + ny];
    triangle(out, [a, b, c], color, width, height);
    triangle(out, [a, c, d], color, width, height);
}

/// Three-point crown filling `[x, y, w, h]`: a band along the bottom third
/// and three spikes, the middle one tallest, each tipped with a jewel.
pub(in crate::ui) fn crown(out: &mut Vec<UiVertex>, bounds: [f32; 4], alpha: f32, width: u32, height: u32) {
    let [x, y, w, h] = bounds;
    if w < 2.0 || h < 2.0 || alpha <= 0.0 {
        return;
    }
    let gold = color::fade(color::GOLD, alpha);
    let shade = [0.0, 0.0, 0.0, 0.45 * alpha];
    let band_h = (h * 0.32).max(1.0);
    let base = y + h - band_h;
    let spike = |out: &mut Vec<UiVertex>, ox: f32, oy: f32, fill: [f32; 4]| {
        // Outer spikes reach 70% of the height, the centre spike all of it.
        let tips = [
            ([x + ox, base + oy], [x + w * 0.30 + ox, base + oy], [x + w * 0.04 + ox, y + h * 0.30 + oy]),
            ([x + w * 0.28 + ox, base + oy], [x + w * 0.72 + ox, base + oy], [x + w * 0.5 + ox, y + oy]),
            ([x + w * 0.70 + ox, base + oy], [x + w + ox, base + oy], [x + w * 0.96 + ox, y + h * 0.30 + oy]),
        ];
        for (a, b, tip) in tips {
            triangle(out, [a, b, tip], fill, width, height);
        }
        rect(out, x + ox, base + oy, w, band_h, fill, width, height);
    };
    // One-pixel drop shadow keeps the gold legible over bright skies.
    spike(out, 1.0, 1.0, shade);
    spike(out, 0.0, 0.0, gold);
    let jewel = (h * 0.16).max(1.5);
    for (cx, cy) in [(x + w * 0.04, y + h * 0.30), (x + w * 0.5, y), (x + w * 0.96, y + h * 0.30)] {
        rect(
            out,
            cx - jewel * 0.5,
            cy - jewel * 0.5,
            jewel,
            jewel,
            color::fade(color::TEXT, alpha),
            width,
            height,
        );
    }
    // Darker inset line separates the band from the spikes.
    rect(out, x, base, w, (h * 0.07).max(1.0), [0.0, 0.0, 0.0, 0.25 * alpha], width, height);
}

/// Where a crown sits for a name run whose cap box is `text` (`[x, y, w, h]`,
/// top of capitals to baseline). The crown is a third of the name's width,
/// centred, in its own box directly above the text with a small gap, so it
/// never overlaps a glyph.
pub(in crate::ui) fn crown_over(text: [f32; 4], scale: f32) -> [f32; 4] {
    let crown_w = (text[2] / 3.0).clamp(15.0 * scale, 46.0 * scale);
    let crown_h = (crown_w * 0.42).clamp(5.0 * scale, 12.0 * scale);
    let gap = (2.0 * scale).max(1.0);
    let mut flex = Flex::new();
    let crown = flex.leaf(crown_w, crown_h);
    let name = flex.leaf(text[2], text[3]);
    let stack = flex.node(Container::column().align(AlignItems::CENTER).gap(gap), &[crown, name]);
    flex.compute(stack, text[2].max(crown_w), crown_h + gap + text[3]);
    let [cx, cy, cw, ch] = flex.bounds(crown);
    let [nx, ny, _, _] = flex.bounds(name);
    // Re-anchor so the name box lands exactly on `text`.
    [text[0] + cx - nx, text[1] + cy - ny, cw, ch]
}

/// Speaker with a diagonal slash: this player is muted on this client.
pub(in crate::ui) fn muted(out: &mut Vec<UiVertex>, bounds: [f32; 4], alpha: f32, width: u32, height: u32) {
    let [x, y, w, h] = bounds;
    let ink = color::fade(color::ROSE_LIGHT, alpha);
    let body_w = w * 0.26;
    let body_h = h * 0.38;
    let cy = y + h * 0.5;
    rect(out, x, cy - body_h * 0.5, body_w, body_h, ink, width, height);
    triangle(
        out,
        [[x + body_w, cy - body_h * 0.5], [x + w * 0.62, y + h * 0.08], [x + w * 0.62, y + h * 0.92]],
        ink,
        width,
        height,
    );
    triangle(
        out,
        [[x + body_w, cy - body_h * 0.5], [x + w * 0.62, y + h * 0.92], [x + body_w, cy + body_h * 0.5]],
        ink,
        width,
        height,
    );
    let stroke = (h * 0.12).max(1.2);
    segment(out, [x + w * 0.02, y + h * 0.95], [x + w * 0.98, y + h * 0.05], stroke * 1.8, [0.0, 0.0, 0.0, 0.6 * alpha], width, height);
    segment(out, [x + w * 0.02, y + h * 0.95], [x + w * 0.98, y + h * 0.05], stroke, ink, width, height);
}

/// A lightsaber from `hilt` (pommel end) to `tip`: a dark ribbed hilt over the
/// first fifth, then a glowing blade in `blade` with a white-hot core.
pub(in crate::ui) fn lightsaber(
    out: &mut Vec<UiVertex>,
    hilt: [f32; 2],
    tip: [f32; 2],
    thickness: f32,
    blade: [f32; 4],
    width: u32,
    height: u32,
) {
    let along = |t: f32| [hilt[0] + (tip[0] - hilt[0]) * t, hilt[1] + (tip[1] - hilt[1]) * t];
    let emitter = along(0.22);
    // Glow, then core: two widths of the same segment read as light, not paint.
    // The core is only tinted toward white, so the blade keeps its colour.
    let core = [0.5 + blade[0] * 0.5, 0.5 + blade[1] * 0.5, 0.5 + blade[2] * 0.5, 0.95 * blade[3]];
    segment(out, emitter, tip, thickness * 3.4, [blade[0], blade[1], blade[2], 0.28 * blade[3]], width, height);
    segment(out, emitter, tip, thickness * 2.0, [blade[0], blade[1], blade[2], 0.80 * blade[3]], width, height);
    segment(out, emitter, along(0.985), thickness * 0.6, core, width, height);
    // Hilt: steel body, darker grip bands, a bright emitter ring.
    segment(out, hilt, emitter, thickness * 1.25, [0.62, 0.62, 0.64, blade[3]], width, height);
    for band in [0.05, 0.10, 0.15] {
        segment(out, along(band), along(band + 0.025), thickness * 1.35, [0.12, 0.11, 0.10, blade[3]], width, height);
    }
    segment(out, along(0.205), emitter, thickness * 1.55, [0.85, 0.85, 0.86, blade[3]], width, height);
}

/// Two crossed sabers filling `bounds`: red rising left to right, blue the
/// other way. Stands between the two team scores.
pub(in crate::ui) fn crossed_sabers(
    out: &mut Vec<UiVertex>,
    bounds: [f32; 4],
    red: [f32; 4],
    blue: [f32; 4],
    width: u32,
    height: u32,
) {
    let [x, y, w, h] = bounds;
    let thickness = (h * 0.06).max(1.5);
    lightsaber(out, [x + w * 0.08, y + h * 0.92], [x + w * 0.92, y + h * 0.08], thickness, red, width, height);
    lightsaber(out, [x + w * 0.92, y + h * 0.92], [x + w * 0.08, y + h * 0.08], thickness, blue, width, height);
}

/// Texture source of the avatar atlas (`ui.wgsl` binding 9).
pub(in crate::ui) const AVATAR_SOURCE: f32 = 9.0;

/// A player avatar as a soft disc: full strength in the middle, fading out
/// toward the rim, so the square icon blends into the board behind it.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn avatar(
    out: &mut Vec<UiVertex>,
    center: [f32; 2],
    radius: f32,
    uv0: [f32; 2],
    uv1: [f32; 2],
    alpha: f32,
    width: u32,
    height: u32,
) {
    const SEGMENTS: usize = 28;
    let inner = 0.58;
    let vertex = |x: f32, y: f32, a: f32| {
        // Map the point inside the icon's square to its atlas UV.
        let u = uv0[0] + (uv1[0] - uv0[0]) * ((x - center[0]) / (radius * 2.0) + 0.5);
        let v = uv0[1] + (uv1[1] - uv0[1]) * ((y - center[1]) / (radius * 2.0) + 0.5);
        UiVertex {
            position: [x / width.max(1) as f32 * 2.0 - 1.0, 1.0 - y / height.max(1) as f32 * 2.0],
            uv: [u, v],
            color: [1.0, 1.0, 1.0, a],
            textured: AVATAR_SOURCE,
        }
    };
    let ring = |scale: f32, a: f32| -> Vec<UiVertex> {
        (0..=SEGMENTS)
            .map(|i| {
                let angle = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
                vertex(center[0] + angle.cos() * radius * scale, center[1] + angle.sin() * radius * scale, a)
            })
            .collect()
    };
    let middle = vertex(center[0], center[1], alpha);
    let inner_ring = ring(inner, alpha);
    let outer_ring = ring(1.0, 0.0);
    for i in 0..SEGMENTS {
        out.extend_from_slice(&[middle, inner_ring[i], inner_ring[i + 1]]);
        out.extend_from_slice(&[inner_ring[i], outer_ring[i], outer_ring[i + 1]]);
        out.extend_from_slice(&[inner_ring[i], outer_ring[i + 1], inner_ring[i + 1]]);
    }
}

/// A small golden cup in `[x, y, w, h]`: bowl, handles, stem and foot.
pub(in crate::ui) fn cup(out: &mut Vec<UiVertex>, bounds: [f32; 4], alpha: f32, width: u32, height: u32) {
    let [x, y, w, h] = bounds;
    let gold = color::fade(color::GOLD, alpha);
    let shade = [0.0, 0.0, 0.0, 0.4 * alpha];
    let draw = |out: &mut Vec<UiVertex>, ox: f32, oy: f32, fill: [f32; 4]| {
        let (x, y) = (x + ox, y + oy);
        // Bowl: wide at the rim, narrowing to the stem.
        triangle(out, [[x + w * 0.18, y], [x + w * 0.82, y], [x + w * 0.68, y + h * 0.48]], fill, width, height);
        triangle(out, [[x + w * 0.18, y], [x + w * 0.68, y + h * 0.48], [x + w * 0.32, y + h * 0.48]], fill, width, height);
        // Handles.
        segment(out, [x + w * 0.18, y + h * 0.08], [x + w * 0.04, y + h * 0.22], (w * 0.09).max(1.0), fill, width, height);
        segment(out, [x + w * 0.04, y + h * 0.22], [x + w * 0.26, y + h * 0.40], (w * 0.09).max(1.0), fill, width, height);
        segment(out, [x + w * 0.82, y + h * 0.08], [x + w * 0.96, y + h * 0.22], (w * 0.09).max(1.0), fill, width, height);
        segment(out, [x + w * 0.96, y + h * 0.22], [x + w * 0.74, y + h * 0.40], (w * 0.09).max(1.0), fill, width, height);
        // Stem and foot.
        rect(out, x + w * 0.44, y + h * 0.48, w * 0.12, h * 0.30, fill, width, height);
        rect(out, x + w * 0.26, y + h * 0.80, w * 0.48, h * 0.16, fill, width, height);
    };
    draw(out, 1.0, 1.0, shade);
    draw(out, 0.0, 0.0, gold);
}

#[cfg(test)]
mod tests {
    use super::crown_over;

    #[test]
    fn crown_is_a_third_of_the_name_centred_above_it() {
        let text = [100.0, 50.0, 90.0, 12.0];
        let [x, y, w, h] = crown_over(text, 1.0);
        assert!((w - 30.0).abs() < 0.01);
        assert!((x + w * 0.5 - (text[0] + text[2] * 0.5)).abs() < 0.01);
        assert!(y + h < text[1], "crown must end above the cap line");
    }

    #[test]
    fn crown_stays_visible_for_short_names() {
        let [_, _, w, h] = crown_over([0.0, 20.0, 12.0, 10.0], 1.0);
        assert!(w >= 10.0 && h >= 5.0);
    }
}
