//! CPU culling of world geometry that water absorption already hides. This is
//! the JAPRO behaviour (`R_UnderwaterSightDistance` in tr_main.cpp and
//! `R_SailingSeabedCulled` in tr_world.cpp), ported to per-surface bounds.
//!
//! The caller supplies a cutoff matched to its active optical model: Unified
//! uses its transparency distance and `r_oceanUnderwaterCull` multiplier, while
//! Blue Water derives the cutoff from native RGB absorption. This keeps the
//! cull tied to the attenuation that actually hides the scene.
//!
//! * **Submerged** (eye inside water): water extinction is isotropic, so a radial cut
//!   measures the distance to the nearest visible surface in every direction at once.
//!   Everything at least `max(distance, 256)` away is dropped. (JAPRO's far plane sits
//!   at `max(distance, 1024) * 1.732`; with murky water that leaves geometry visible
//!   long after absorption has hidden it, so the radius here is the distance itself.)
//! * **Above water**: only rays that pierce the surface are attenuated, and what hides
//!   submerged geometry is the water column over it, which depends on depth alone. A
//!   surface whose top is more than `distance` below the water plane, and which lies
//!   wholly over the water, is dropped regardless of where the viewer stands.
use crate::ocean::{OceanMasks, WaterVolume};

/// Never cull submerged geometry closer than this, however murky the water is set
/// (JAPRO floors at 1024; murky water here is allowed to cull sooner).
pub const SUBMERGED_MIN_CULL: f32 = 256.0;

/// Radius of the submerged radial cut, or `None` when the eye is not in water.
///
/// `surface_offset` is how far the real (animated) surface over the eye sits above
/// the volume's flat top, in render units. The eye is under water exactly when it is
/// under `top + surface_offset` and above the fixed authored floor.
/// Pass 0 for a flat plane. Distances are measured from the true eye.
pub fn submerged_cull_radius(
    eye: [f32; 3], volumes: &[WaterVolume], masks: &OceanMasks, distance: f32, surface_offset: f32,
) -> Option<f32> {
    if !(distance > 0.0) {
        return None;
    }
    volumes
        .iter()
        .any(|v| eye[1] < v.maximum[1] + surface_offset && v.contains_displaced(eye, masks, surface_offset))
        .then(|| distance.max(SUBMERGED_MIN_CULL))
}

pub struct WaterCull<'a> {
    eye: [f32; 3],
    /// Squared radial cut while submerged; `None` above water.
    submerged_radius_sq: Option<f32>,
    /// Depth below the plane that hides a surface from above.
    depth: f32,
    masks: &'a OceanMasks,
    volumes: Vec<WaterVolume>,
}

impl<'a> WaterCull<'a> {
    /// `None` when culling is disabled or there is no water to reason about.
    pub fn new(
        eye: [f32; 3], volumes: &[WaterVolume], masks: &'a OceanMasks, distance: f32, surface_offset: f32,
    ) -> Option<Self> {
        if !(distance > 0.0) || volumes.is_empty() {
            return None;
        }
        Some(Self {
            eye,
            submerged_radius_sq: submerged_cull_radius(eye, volumes, masks, distance, surface_offset).map(|r| r * r),
            depth: distance,
            masks,
            volumes: volumes.to_vec(),
        })
    }

    /// Whether a world surface with these render-space bounds may be skipped.
    pub fn culls(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        if let Some(radius_sq) = self.submerged_radius_sq {
            let distance_sq: f32 = (0..3).map(|i| (self.eye[i] - self.eye[i].clamp(min[i], max[i])).powi(2)).sum();
            return distance_sq >= radius_sq;
        }
        self.volumes.iter().any(|v| {
            v.maximum[1] - max[1] > self.depth
                && min[1] >= v.minimum[1]
                && [0, 2].iter().all(|&i| min[i] >= v.minimum[i] && max[i] <= v.maximum[i])
                && v.mask_slot.map_or(true, |slot| {
                    self.masks.rect_inside(usize::from(slot), [min[0], min[2]], [max[0], max[2]])
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sea() -> WaterVolume {
        WaterVolume { minimum: [-10000.0, -65536.0, -10000.0], maximum: [10000.0, 0.0, 10000.0], mask_slot: None }
    }
    fn cull<'a>(eye: [f32; 3], volume: WaterVolume, masks: &'a OceanMasks, distance: f32) -> WaterCull<'a> {
        WaterCull::new(eye, &[volume], masks, distance, 0.0).unwrap()
    }
    fn strip() -> (OceanMasks, WaterVolume) {
        let mut masks = OceanMasks::default();
        let triangles = [
            [[0.0, 0.0], [6000.0, 0.0], [6000.0, 20000.0]],
            [[0.0, 0.0], [6000.0, 20000.0], [0.0, 20000.0]],
        ];
        assert!(masks.push_surface(0, &triangles));
        let volume = WaterVolume { minimum: [0.0, -20000.0, 0.0], maximum: [20000.0, 0.0, 20000.0], mask_slot: Some(0) };
        (masks, volume)
    }

    #[test]
    fn disabled_or_waterless_never_culls() {
        let masks = OceanMasks::default();
        assert!(WaterCull::new([0.0; 3], &[sea()], &masks, 0.0, 0.0).is_none());
        assert!(WaterCull::new([0.0; 3], &[], &masks, 3600.0, 0.0).is_none());
    }

    #[test]
    fn wave_offset_moves_the_waterline_with_the_real_surface() {
        let masks = OceanMasks::default();
        // Plane at 0, a crest of +60 over the eye: an eye at +30 is under water...
        let in_crest = WaterCull::new([0.0, 30.0, 0.0], &[sea()], &masks, 2000.0, 60.0).unwrap();
        assert!(in_crest.culls([2100.0, -50.0, 0.0], [2200.0, 50.0, 100.0]));
        // ...and with a flat plane the same eye is above water and does not radial-cull.
        let flat = WaterCull::new([0.0, 30.0, 0.0], &[sea()], &masks, 2000.0, 0.0).unwrap();
        assert!(!flat.culls([2100.0, -50.0, 0.0], [2200.0, 50.0, 100.0]));
        // A trough of -60: an eye at -30 is below the plane but in air.
        assert!(submerged_cull_radius([0.0, -30.0, 0.0], &[sea()], &masks, 2000.0, -60.0).is_none());
        assert!(submerged_cull_radius([0.0, -30.0, 0.0], &[sea()], &masks, 2000.0, 0.0).is_some());
    }

    #[test]
    fn authored_floor_stays_fixed_under_crests_and_troughs() {
        let masks = OceanMasks::default();
        let shallow = WaterVolume { minimum:[-100.0,-56.0,-100.0],
            maximum:[100.0,0.0,100.0], mask_slot:None };
        assert!(submerged_cull_radius([0.0,-80.0,0.0], &[shallow], &masks, 2000.0, -40.0).is_none());
        assert!(submerged_cull_radius([0.0,-40.0,0.0], &[shallow], &masks, 2000.0, 40.0).is_some());
        assert!(submerged_cull_radius([0.0,-3273.0,0.0], &[shallow], &masks, 2000.0, 0.0).is_none());
    }

    #[test]
    fn submerged_cuts_radially_at_the_scaled_distance_in_every_direction() {
        let masks = OceanMasks::default();
        // Distance 2000 -> radius 2000. A swimmer just under the surface counts.
        let swimmer = cull([0.0, -50.0, 0.0], sea(), &masks, 2000.0);
        assert!(swimmer.culls([6000.0, 100.0, 0.0], [6400.0, 900.0, 300.0])); // coast above water
        assert!(swimmer.culls([0.0, -9000.0, 0.0], [100.0, -8000.0, 100.0])); // deep seabed
        assert!(swimmer.culls([2100.0, -50.0, 0.0], [2200.0, 50.0, 100.0]));
        assert!(!swimmer.culls([1900.0, -50.0, 0.0], [1990.0, 50.0, 100.0])); // inside 2000
        assert!(!swimmer.culls([100.0, -100.0, 100.0], [300.0, 100.0, 300.0]));
    }

    #[test]
    fn submerged_has_a_small_floor() {
        let masks = OceanMasks::default();
        // A tiny absorption distance still keeps 256 units around the viewer.
        let murky = cull([0.0, -50.0, 0.0], sea(), &masks, 0.3);
        assert!(!murky.culls([200.0, -50.0, 0.0], [250.0, 50.0, 100.0]));
        assert!(murky.culls([300.0, -50.0, 0.0], [400.0, 50.0, 100.0]));
    }

    #[test]
    fn above_water_culls_by_depth_below_the_surface_only() {
        let masks = OceanMasks::default();
        let above = cull([0.0, 400.0, 0.0], sea(), &masks, 2000.0);
        // Wholly over the water and its top is 2500 below the plane: hidden, near or far.
        assert!(above.culls([100.0, -5200.0, 100.0], [300.0, -2500.0, 300.0]));
        // Its top is only 1500 down: kept.
        assert!(!above.culls([100.0, -5200.0, 100.0], [300.0, -1500.0, 300.0]));
        // Far above-water geometry is never touched from above.
        assert!(!above.culls([6000.0, 100.0, 0.0], [6400.0, 900.0, 300.0]));
        // Spills past the footprint (could be dry ground): kept.
        assert!(!above.culls([9000.0, -5200.0, 100.0], [11000.0, -5000.0, 300.0]));
    }

    #[test]
    fn masked_water_only_culls_geometry_wholly_over_its_outline() {
        let (masks, volume) = strip();
        let above = cull([3000.0, 400.0, 10000.0], volume, &masks, 2000.0);
        assert!(above.culls([100.0, -5200.0, 100.0], [300.0, -5000.0, 300.0]));
        assert!(!above.culls([5900.0, -5200.0, 100.0], [6500.0, -5000.0, 300.0])); // crosses the rim
        assert!(!above.culls([7000.0, -5200.0, 100.0], [7300.0, -5000.0, 300.0])); // dry
    }

    #[test]
    fn submerged_in_masked_water_requires_being_on_the_outline() {
        let (masks, volume) = strip();
        // In the box but off the outline (dry): not submerged, so no radial cut.
        let dry = cull([12000.0, -50.0, 10000.0], volume, &masks, 2000.0);
        assert!(!dry.culls([0.0, 100.0, 19000.0], [200.0, 900.0, 19200.0]));
        // On the outline: radial cut.
        let wet = cull([3000.0, -50.0, 10000.0], volume, &masks, 2000.0);
        assert!(wet.culls([3000.0, 100.0, 19000.0], [3200.0, 900.0, 19200.0]));
    }
}
