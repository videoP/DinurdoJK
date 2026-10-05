//! Geometry.
use crate::ui::UiVertex;

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn glyph_quad(
    out: &mut Vec<UiVertex>,
    glyph: u8,
    x: f32,
    y: f32,
    scale: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    glyph_quad_sized(
        out,
        glyph,
        x,
        y,
        6.0 * scale,
        8.0 * scale,
        color,
        width,
        height,
    );
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn glyph_quad_sized(
    out: &mut Vec<UiVertex>,
    glyph: u8,
    x: f32,
    y: f32,
    glyph_w: f32,
    glyph_h: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    // Jedi Academy's gfx/2d/charsgrid_med is a 16x16 *logical* grid,
    // but each glyph only occupies the left 8 pixels of its 16-pixel-wide
    // slot. OpenJK therefore advances U by 1/16 per character while only
    // sampling 1/32 of the texture width; V uses the full 1/16 cell.
    // See SCR_DrawChar / SCR_DrawSmallChar in OpenJK cl_scrn.cpp.
    let cell_stride = 1.0 / 16.0;
    let glyph_u_width = 1.0 / 32.0;
    let column = (glyph & 15) as f32;
    let row = (glyph >> 4) as f32;
    let u0 = column * cell_stride;
    let v0 = row * cell_stride;
    let u1 = u0 + glyph_u_width;
    let v1 = v0 + cell_stride;
    textured_rect(
        out,
        x,
        y,
        glyph_w,
        glyph_h,
        [u0, v0],
        [u1, v1],
        color,
        width,
        height,
    );
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn textured_rect(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    textured_rect_with_source(out, x, y, w, h, uv0, uv1, color, 1.0, width, height);
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn textured_rect_with_source(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
    texture_source: f32,
    width: u32,
    height: u32,
) {
    let x0 = x / width.max(1) as f32 * 2.0 - 1.0;
    let x1 = (x + w) / width.max(1) as f32 * 2.0 - 1.0;
    let y0 = 1.0 - y / height.max(1) as f32 * 2.0;
    let y1 = 1.0 - (y + h) / height.max(1) as f32 * 2.0;
    let v = |position, uv| UiVertex {
        position,
        uv,
        color,
        textured: texture_source,
    };
    out.extend_from_slice(&[
        v([x0, y0], [uv0[0], uv0[1]]),
        v([x0, y1], [uv0[0], uv1[1]]),
        v([x1, y1], [uv1[0], uv1[1]]),
        v([x0, y0], [uv0[0], uv0[1]]),
        v([x1, y1], [uv1[0], uv1[1]]),
        v([x1, y0], [uv1[0], uv0[1]]),
    ]);
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn disc(
    out: &mut Vec<UiVertex>,
    center_x: f32,
    center_y: f32,
    radius: f32,
    segments: usize,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let to_ndc = |x: f32, y: f32| [x / width * 2.0 - 1.0, 1.0 - y / height * 2.0];
    let center = to_ndc(center_x, center_y);
    let vertex = |position| UiVertex {
        position,
        uv: [0.0, 0.0],
        color,
        textured: 0.0,
    };
    let segments = segments.max(3);
    for index in 0..segments {
        let a0 = std::f32::consts::TAU * index as f32 / segments as f32;
        let a1 = std::f32::consts::TAU * (index + 1) as f32 / segments as f32;
        let p0 = to_ndc(center_x + radius * a0.cos(), center_y + radius * a0.sin());
        let p1 = to_ndc(center_x + radius * a1.cos(), center_y + radius * a1.sin());
        out.extend_from_slice(&[vertex(center), vertex(p0), vertex(p1)]);
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn rect(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let x0 = x / width.max(1) as f32 * 2.0 - 1.0;
    let x1 = (x + w) / width.max(1) as f32 * 2.0 - 1.0;
    let y0 = 1.0 - y / height.max(1) as f32 * 2.0;
    let y1 = 1.0 - (y + h) / height.max(1) as f32 * 2.0;
    let v = |position| UiVertex {
        position,
        uv: [0.0, 0.0],
        color,
        textured: 0.0,
    };
    out.extend_from_slice(&[
        v([x0, y0]),
        v([x0, y1]),
        v([x1, y1]),
        v([x0, y0]),
        v([x1, y1]),
        v([x1, y0]),
    ]);
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn rect_outline(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    thickness: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    rect(out, x, y, w, thickness, color, width, height);
    rect(
        out,
        x,
        y + h - thickness,
        w,
        thickness,
        color,
        width,
        height,
    );
    rect(out, x, y, thickness, h, color, width, height);
    rect(
        out,
        x + w - thickness,
        y,
        thickness,
        h,
        color,
        width,
        height,
    );
}
