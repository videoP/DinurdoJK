//! Conservative receiver coverage. Each bit covers a 1/16 by 1/8 NDC tile.
use crate::renderer::{Mat4, Vec3};

pub(in crate::renderer) type ShadowCoverage = [u32; 4];
pub(in crate::renderer) const FULL_COVERAGE: ShadowCoverage = [u32::MAX; 4];

/// Project each caster separately so empty space between casters stays empty.
/// A box crossing the perspective eye plane must fail open: projecting only
/// its corners would otherwise miss part of the clipped polygon.
pub(in crate::renderer) fn add_caster(
    mask: &mut ShadowCoverage, matrix: Mat4, min: [f32; 3], max: [f32; 3], pad: f32,
) {
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    if !Vec3::from_array(min).is_finite() || !Vec3::from_array(max).is_finite() {
        *mask = FULL_COVERAGE;
        return;
    }
    for x in [min[0], max[0]] { for y in [min[1], max[1]] { for z in [min[2], max[2]] {
        let clip = matrix * Vec3::new(x, y, z).extend(1.0);
        if !clip.is_finite() || clip.w <= 0.00001 {
            *mask = FULL_COVERAGE;
            return;
        }
        let p = clip.truncate() / clip.w;
        low = low.min(p);
        high = high.max(p);
    }}}
    low -= Vec3::splat(pad);
    high += Vec3::splat(pad);
    if high.x < -1.0 || high.y < -1.0 || low.x > 1.0 || low.y > 1.0 { return; }
    let tile = |p: f32, count: u32| (((p * 0.5 + 0.5) * count as f32).floor() as i32)
        .clamp(0, count as i32 - 1) as u32;
    for y in tile(low.y, 8)..=tile(high.y, 8) {
        for x in tile(low.x, 16)..=tile(high.x, 16) {
            let bit = y * 16 + x;
            mask[(bit / 32) as usize] |= 1 << (bit % 32);
        }
    }
}

/// Live depth must be cleared once after the last caster leaves, including
/// before its first use. An already empty layer needs no further render pass.
pub(in crate::renderer) fn update_live_layer(
    initialized: &mut u32, occupied: &mut u32, layer: usize, has_casters: bool,
) -> bool {
    let bit = 1 << layer;
    let render = has_casters || *initialized & bit == 0 || *occupied & bit != 0;
    *initialized |= bit;
    *occupied = (*occupied & !bit) | if has_casters { bit } else { 0 };
    render
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contains(mask: ShadowCoverage, x: f32, y: f32) -> bool {
        let x = ((x * 0.5 + 0.5) * 16.0).floor().clamp(0.0, 15.0) as u32;
        let y = ((y * 0.5 + 0.5) * 8.0).floor().clamp(0.0, 7.0) as u32;
        let bit = y * 16 + x;
        mask[(bit / 32) as usize] & (1 << (bit % 32)) != 0
    }
    #[test]
    fn shadow_coverage_keeps_gaps_and_filter_padding() {
        let mut mask = [0; 4];
        for x in [-0.8, 0.8] {
            add_caster(&mut mask, Mat4::IDENTITY, [x - 0.02, -0.02, 0.2], [x + 0.02, 0.02, 0.3], 0.04);
            for dx in [-0.06, 0.0, 0.06] { assert!(contains(mask, x + dx, 0.0)); }
        }
        assert!(!contains(mask, 0.0, 0.0));
    }
    #[test]
    fn shadow_coverage_unknown_projection_fails_open_and_offscreen_is_empty() {
        let mut mask = [0; 4];
        add_caster(&mut mask, Mat4::IDENTITY, [2.0; 3], [3.0; 3], 0.02);
        assert_eq!(mask, [0; 4]);
        add_caster(&mut mask, Mat4::ZERO, [-1.0; 3], [1.0; 3], 0.02);
        assert_eq!(mask, FULL_COVERAGE);
        mask = [0; 4];
        add_caster(&mut mask, Mat4::IDENTITY, [f32::NAN; 3], [1.0; 3], 0.02);
        assert_eq!(mask, FULL_COVERAGE);
    }
    #[test]
    fn shadow_coverage_live_layers_clear_once_after_departure() {
        let (mut initialized, mut occupied) = (0, 0);
        for (has_casters, expected) in [(false,true),(false,false),(true,true),(true,true),(false,true),(false,false),(true,true)] {
            assert_eq!(update_live_layer(&mut initialized, &mut occupied, 2, has_casters), expected);
        }
        assert_eq!(initialized, 4);
        assert_eq!(occupied, 4);
    }
}
