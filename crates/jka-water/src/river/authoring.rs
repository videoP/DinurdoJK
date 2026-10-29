//! WaterAdvanced ShallowWaterRiverActor::Rebuild boundary construction.
//! World positions/tangents use double precision as Unreal's FVector does;
//! emitted Niagara sizes/angles and local solver positions are floats.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
use super::{boundary::SourceBox, control::Grid};

pub const SOURCE_SIZE_CM: f32 = 1000.0;

#[derive(Clone, Copy, Debug)]
pub struct Domain {
    pub origin: [f64; 3],
    pub vertical_extent: f64,
    pub grid: Grid,
}

impl Domain {
    /// Rebuild places the system at the XY bounds centre and the bottom Z,
    /// rather than at the 3D bounds centre. Grid padding follows Niagara.
    pub fn from_bounds(min: [f64; 3], max: [f64; 3], max_axis: u32) -> Result<Self, &'static str> {
        if (0..3).any(|i| !min[i].is_finite() || !max[i].is_finite() || max[i] <= min[i]) {
            return Err("invalid river authoring bounds");
        }
        let size: [f64; 3] = std::array::from_fn(|i| max[i] - min[i]);
        let origin = [min[0] + size[0] * 0.5, min[1] + size[1] * 0.5, min[2]];
        if origin.iter().any(|v| !v.is_finite()) || size.iter().any(|v| !v.is_finite()) {
            return Err("river authoring bounds overflow");
        }
        Ok(Self {
            origin,
            vertical_extent: size[2] * 0.5,
            grid: Grid::max_axis([size[0] as f32, size[1] as f32], max_axis)?,
        })
    }

    pub fn grid_min(self) -> [f32; 3] {
        [
            -self.grid.world_size[0] * 0.5,
            -self.grid.world_size[1] * 0.5,
            0.0,
        ]
    }

    pub fn boundary(
        self,
        endpoint: Endpoint,
        sink: bool,
        source_size: f32,
    ) -> Result<SourceBox, &'static str> {
        if endpoint
            .position
            .iter()
            .chain(endpoint.tangent.iter())
            .any(|v| !v.is_finite())
            || !endpoint.width.is_finite()
            || endpoint.width <= 0.0
            || !source_size.is_finite()
            || source_size <= 0.0
        {
            return Err("invalid river endpoint position, tangent, width or source size");
        }
        let squared =
            endpoint.tangent[0] * endpoint.tangent[0] + endpoint.tangent[1] * endpoint.tangent[1];
        if !squared.is_finite() || !self.vertical_extent.is_finite() || self.vertical_extent <= 0.0
        {
            return Err("river endpoint tangent or vertical extent overflow");
        }
        let height = if sink {
            100000.0
        } else {
            (2.0 * self.vertical_extent + 1000.0) as f32
        };
        let mut position = endpoint.position;
        if !sink {
            // Deliberately BEFORE direction normalization, just as Rebuild.
            // A spline tangent's magnitude affects this source offset.
            position[0] += endpoint.tangent[0] * 0.5 * f64::from(source_size);
            position[1] += endpoint.tangent[1] * 0.5 * f64::from(source_size);
            position[2] -= 0.5 * f64::from(height);
        }
        // FVector::Normalize leaves the vector unchanged when SquareSum does
        // not exceed UE_SMALL_NUMBER, including the zero-tangent case.
        let scale = if squared > 1e-8 {
            1.0 / squared.sqrt()
        } else {
            1.0
        };
        let direction = [endpoint.tangent[0] * scale, endpoint.tangent[1] * scale];
        let mut angle = direction[1].clamp(-1.0, 1.0).acos();
        // BaseVector=(0,1,0); normalized cross product has Z sign -Dir.X.
        if direction[0] > 0.0 {
            angle *= -1.0;
        }
        let position = std::array::from_fn(|i| (position[i] - self.origin[i]) as f32);
        if !height.is_finite() || position.iter().any(|v| !v.is_finite()) {
            return Err("river endpoint local coordinates overflow");
        }
        Ok(SourceBox {
            position,
            size: [endpoint.width, source_size, height],
            angle: angle as f32,
            sink,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Endpoint {
    pub position: [f64; 3],
    pub tangent: [f64; 3],
    pub width: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actor_origin_is_bottom_and_source_preserves_raw_tangent_offset() {
        let domain = Domain::from_bounds([100.0, 200.0, -50.0], [300.0, 280.0, 150.0], 16).unwrap();
        assert_eq!(domain.origin, [200.0, 240.0, -50.0]);
        let endpoint = Endpoint {
            position: [150.0, 220.0, 100.0],
            tangent: [2.0, 0.0, 999.0],
            width: 30.0,
        };
        let source = domain.boundary(endpoint, false, 1000.0).unwrap();
        assert_eq!(source.position, [950.0, -20.0, -450.0]);
        assert_eq!(source.size, [30.0, 1000.0, 1200.0]);
        assert_eq!(source.angle, -std::f32::consts::FRAC_PI_2);
        assert_eq!(source.position[2] + source.size[2] * 0.5, 150.0);
        let sink = domain.boundary(endpoint, true, 1000.0).unwrap();
        assert_eq!(sink.position, [-50.0, -20.0, 150.0]);
        assert_eq!(sink.size, [30.0, 1000.0, 100000.0]);
    }
    #[test]
    fn endpoint_translation_keeps_local_precision_and_direction_is_from_positive_y() {
        let mut a = Domain::from_bounds([0.0; 3], [100.0; 3], 16).unwrap();
        let mut p = Endpoint {
            position: [35.0, 40.0, 80.0],
            tangent: [-1.0, 0.0, 0.0],
            width: 20.0,
        };
        let expected = a.boundary(p, false, 1000.0).unwrap();
        assert_eq!(expected.angle, std::f32::consts::FRAC_PI_2);
        for i in 0..3 {
            a.origin[i] += 1e9;
            p.position[i] += 1e9;
        }
        assert_eq!(
            a.boundary(p, false, 1000.0).unwrap().position,
            expected.position
        );
        p.tangent = [0.0; 3];
        assert_eq!(
            a.boundary(p, false, 1000.0).unwrap().angle,
            std::f32::consts::FRAC_PI_2
        );
    }
}
