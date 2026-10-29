//! CPU reference for the actual Niagara stencil/sampling operations in stages
//! 8, 9, 10, 13 and 14. Original algorithms: Copyright Epic Games, Inc.
//! Integer loads use zero outside the grid; filtered samples clamp to edges.

use super::{Geometry, Parameters, Presentation, Transport, Water};

pub struct Field<'a> {
    pub width: usize,
    pub height: usize,
    water: &'a [Water],
    geometry: &'a [Geometry],
    filtered_sample: Option<&'a dyn Fn([f32; 2], usize) -> f32>,
}

impl<'a> Field<'a> {
    pub fn new(
        width: usize,
        height: usize,
        water: &'a [Water],
        geometry: &'a [Geometry],
    ) -> Result<Self, &'static str> {
        let count = width
            .checked_mul(height)
            .ok_or("river grid size overflow")?;
        if width == 0
            || height == 0
            || width > i32::MAX as usize
            || height > i32::MAX as usize
            || water.len() != count
            || geometry.len() != count
        {
            return Err("invalid river reference grid dimensions or channel lengths");
        }
        Ok(Self {
            width,
            height,
            water,
            geometry,
            filtered_sample: None,
        })
    }

    /// Supply hardware-filtered samples for GPU conformance checks. The default
    /// CPU reference uses ideal f32 bilinear arithmetic; texture filtering has
    /// device-specific precision and must not be emulated by guessed rounding.
    /// Coordinates are the source's original normalized UV, before GridPos.
    pub fn with_filtered_sampler(mut self, sampler: &'a dyn Fn([f32; 2], usize) -> f32) -> Self {
        self.filtered_sample = Some(sampler);
        self
    }

    fn filtered(&self, unit: [f32; 2], at: [f32; 2], channel: usize) -> f32 {
        match self.filtered_sample {
            Some(sample) => sample(unit, channel),
            None => self.bilinear(at, channel),
        }
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height)
            .then(|| y as usize * self.width + x as usize)
    }

    pub fn water(&self, x: i32, y: i32) -> Water {
        self.index(x, y).map(|i| self.water[i]).unwrap_or_default()
    }

    pub fn geometry(&self, x: i32, y: i32) -> Geometry {
        self.index(x, y)
            .map(|i| self.geometry[i])
            .unwrap_or_default()
    }

    fn clamp(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x.clamp(0, self.width as i32 - 1),
            y.clamp(0, self.height as i32 - 1),
        )
    }

    fn channel(&self, x: i32, y: i32, channel: usize) -> f32 {
        let w = self.water(x, y);
        [w.height, w.depth, w.velocity[0], w.velocity[1]][channel]
    }

    /// Basic2DCubic from NiagaraDataInterfaceGrid2DCollection. Interpolation
    /// overshoot or a boundary stencil reverts to bilinear, independently for
    /// each attribute. This is not a generic Catmull-Rom replacement.
    pub fn sample(&self, at: [f32; 2], channel: usize) -> f32 {
        // The cached advection shader calls IndexToUnit before Basic2DCubic
        // converts UV back to GridPos. Preserve both f32 operations: cancelling
        // this round trip changes sampling at integer-cell boundaries.
        let unit = [
            (at[0] + 0.5) * (1.0 / self.width as f32),
            (at[1] + 0.5) * (1.0 / self.height as f32),
        ];
        let at = [
            unit[0] * self.width as f32 - 0.5,
            unit[1] * self.height as f32 - 0.5,
        ];
        let x = at[0].floor() as i32;
        let y = at[1].floor() as i32;
        if x < 2 || y < 2 || x >= self.width as i32 - 3 || y >= self.height as i32 - 3 {
            return self.filtered(unit, at, channel);
        }
        let mut rows = [0.0; 4];
        let mut minimum = f32::MAX;
        let mut maximum = -f32::MAX;
        for j in 0..4 {
            let mut values = [0.0; 4];
            for i in 0..4 {
                let value = self.channel(x + i - 1, y + j - 1, channel);
                values[i as usize] = value;
                minimum = minimum.min(value);
                maximum = maximum.max(value);
            }
            rows[j as usize] = basic_cubic(values, at[0] - at[0].floor());
        }
        let value = basic_cubic(rows, at[1] - at[1].floor());
        if value < minimum || value > maximum {
            self.filtered(unit, at, channel)
        } else {
            value
        }
    }

    pub fn bilinear(&self, at: [f32; 2], channel: usize) -> f32 {
        let at = [
            at[0].clamp(0.0, self.width as f32 - 1.0),
            at[1].clamp(0.0, self.height as f32 - 1.0),
        ];
        let x = at[0].floor() as i32;
        let y = at[1].floor() as i32;
        let (right, up) = self.clamp(x + 1, y + 1);
        let a = lerp(
            self.channel(x, y, channel),
            self.channel(right, y, channel),
            at[0] - x as f32,
        );
        let b = lerp(
            self.channel(x, up, channel),
            self.channel(right, up, channel),
            at[0] - x as f32,
        );
        lerp(a, b, at[1] - y as f32)
    }

    pub fn extrapolate_velocity(&self, x: i32, y: i32, settings: Transport) -> Water {
        let mut w = self.water(x, y);
        if (w.depth < 1e-5 && settings.extrapolate_into_dry)
            || self.geometry(x, y).collider > 1.0 - 1e-5
        {
            let mut distance = 65500.0;
            let radius = settings.extrapolation_half_width as i32;
            for dx in -radius..=radius {
                for dy in -radius..=radius {
                    let neighbor = self.water(x + dx, y + dy);
                    let d = ((dx * dx + dy * dy) as f32).sqrt();
                    if d < distance
                        && neighbor.depth > 1e-5
                        && neighbor.height > w.height
                        && length(neighbor.velocity) > 1e-5
                    {
                        w.velocity = neighbor.velocity;
                        distance = d;
                    }
                }
            }
        }
        w
    }

    pub fn advect(
        &self,
        x: i32,
        y: i32,
        previous_bottom: f32,
        p: Parameters,
        settings: Transport,
    ) -> Water {
        let input = self.water(x, y);
        let geo = self.geometry(x, y);
        let mut w = input;
        if geo.collider < 1.0 - 1e-5 && input.depth > settings.minimum_advection_depth {
            let scale = p.dt_over_dx() * settings.advection_multiplier;
            let vy = 0.25
                * (self.water(x, y + 1).velocity[1]
                    + self.water(x - 1, y + 1).velocity[1]
                    + self.water(x - 1, y).velocity[1]
                    + input.velocity[1]);
            w.velocity[0] = self.sample(
                [x as f32 - scale * input.velocity[0], y as f32 - scale * vy],
                2,
            );
            let vx = 0.25
                * (self.water(x + 1, y).velocity[0]
                    + self.water(x + 1, y - 1).velocity[0]
                    + self.water(x, y - 1).velocity[0]
                    + input.velocity[0]);
            w.velocity[1] = self.sample(
                [x as f32 - scale * vx, y as f32 - scale * input.velocity[1]],
                3,
            );
            let velocity = [
                0.5 * (self.water(x + 1, y).velocity[0] + input.velocity[0]),
                0.5 * (self.water(x, y + 1).velocity[1] + input.velocity[1]),
            ];
            w.depth = self.sample(
                [
                    x as f32 - scale * velocity[0],
                    y as f32 - scale * velocity[1],
                ],
                1,
            );
        }
        w.height = geo.bottom + w.depth;
        // This scratch function receives the newly advected depth, not the
        // stage's pre-advection input depth.
        if (geo.bottom - previous_bottom).abs() > p.dx && w.depth > 1e-5 {
            w = Water {
                height: geo.bottom,
                ..Water::default()
            };
        }
        w
    }

    pub fn extrapolate_height(&self, x: i32, y: i32, settings: Transport) -> Water {
        let mut w = self.water(x, y);
        let geo = self.geometry(x, y);
        if geo.collider > 1.0 - 1e-5 {
            let mut distance = 65500.0;
            let radius = settings.extrapolation_half_width as i32;
            for dx in -radius..=radius {
                for dy in -radius..=radius {
                    let neighbor = self.water(x + dx, y + dy);
                    let d = ((dx * dx + dy * dy) as f32).sqrt();
                    if d < distance
                        && self.geometry(x + dx, y + dy).collider < 1.0 - 1e-5
                        && neighbor.depth > 1e-5
                    {
                        w.height = neighbor.height;
                        distance = d;
                    }
                }
            }
            w.depth = w.height - geo.bottom;
        }
        w
    }

    /// Stage 13 writes smoothed HEIGHT back to WaterGrid, but the extrapolated
    /// DEPTH and blurred VELOCITY only to SimGridRT. Never feed those render
    /// channels back into the hydraulic state.
    pub fn present(&self, x: i32, y: i32, settings: Presentation) -> (Water, Water) {
        let input = self.water(x, y);
        let border = settings.border_width as i32;
        let is_border = x < border
            || y < border
            || x > self.width as i32 - 1 - border
            || y > self.height as i32 - 1 - border;
        let extrap =
            !is_border && (self.geometry(x, y).collider > 1.0 - 1e-5 || input.depth < 1e-5);
        let mut height = if extrap { 0.0 } else { input.height };
        let mut depth = if extrap { 0.0 } else { input.depth };
        let mut weight_height = if extrap { 0.0 } else { 1.0 };
        let mut weight_depth = weight_height;
        let radius = settings.half_width as i32;
        let depth_radius = (radius - 2).max(1);
        let method = if settings.height_cutoff < 1e-8 {
            3
        } else {
            settings.smoothing_method
        };
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                // Preserve the source's AND, which excludes the entire center row/column.
                if dx == 0 || dy == 0 {
                    continue;
                }
                let (nx, ny) = self.clamp(x + dx, y + dy);
                let w = self.water(nx, ny);
                if self.geometry(nx, ny).collider >= 1.0 - 1e-5 || w.depth <= 1e-5 {
                    continue;
                }
                let distance = (((nx - x).pow(2) + (ny - y).pow(2)) as f32).sqrt();
                let mut weight = 0.0;
                if extrap {
                    weight = distance;
                } else if method != 0 {
                    weight = 1.0 - smoothstep(radius as f32, distance);
                    if method == 1 {
                        weight *= 1.0
                            - smoothstep(settings.height_cutoff, (w.height - input.height).abs());
                    } else if method == 2 {
                        weight *=
                            smoothstep(settings.height_cutoff, (w.height - input.height).abs());
                    }
                }
                height += w.height * weight;
                weight_height += weight;
                if dx.abs() <= depth_radius && dy.abs() <= depth_radius {
                    depth += w.depth * weight;
                    weight_depth += weight;
                }
            }
        }
        height = if weight_height > 1e-8 {
            height / weight_height
        } else if extrap {
            input.height
        } else {
            height
        };
        depth = if weight_depth > 1e-8 {
            depth / weight_depth
        } else if extrap {
            input.depth
        } else {
            depth
        };
        if is_border {
            depth = 0.0;
        }
        let mut velocity = [0.0; 2];
        let mut weight = 0.0;
        for dx in -1..=1 {
            for dy in -1..=1 {
                let (nx, ny) = self.clamp(x + dx, y + dy);
                let kernel = length([1.0 - (dx as f32).abs() / 2.0, 1.0 - (dy as f32).abs() / 2.0]);
                for (out, value) in velocity.iter_mut().zip(self.water(nx, ny).velocity) {
                    *out += value * kernel;
                }
                weight += kernel;
            }
        }
        for value in &mut velocity {
            *value /= weight;
        }
        (
            Water { height, ..input },
            Water {
                height,
                depth,
                velocity,
            },
        )
    }

    /// Stage 14 reads the temporary render height, emits a normal, and restores
    /// the hydraulic height to bottom + original depth.
    pub fn normal_and_reset(&self, x: i32, y: i32, dx: f32) -> (Water, [f32; 4]) {
        let input = self.water(x, y);
        let mut normal = [0.0, 0.0, 1.0, 0.0];
        if input.depth > 1e-5 {
            let cross = [
                -dx * (self.water(x + 1, y).height - input.height),
                -dx * (self.water(x, y + 1).height - input.height),
                dx * dx,
            ];
            let length = (cross[0].powi(2) + cross[1].powi(2) + cross[2].powi(2)).sqrt();
            if length >= 1e-5 {
                for axis in 0..3 {
                    normal[axis] = cross[axis] / length;
                }
            }
        }
        (
            Water {
                height: self.geometry(x, y).bottom + input.depth,
                ..input
            },
            normal,
        )
    }
}

fn basic_cubic([fm1, f0, f1, f2]: [f32; 4], t: f32) -> f32 {
    if (f1 - f0).abs() < f32::MIN_POSITIVE {
        return f0;
    }
    let a1 = -1.0 / 3.0 * fm1 - 0.5 * f0 + f1 - 1.0 / 6.0 * f2;
    let a2 = 0.5 * fm1 - f0 + 0.5 * f1;
    let a3 = -1.0 / 6.0 * fm1 + 0.5 * f0 - 0.5 * f1 + 1.0 / 6.0 * f2;
    a3 * t * t * t + a2 * t * t + a1 * t + f0
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}
fn length(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}
fn smoothstep(edge: f32, value: f32) -> f32 {
    let t = (value / edge).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubic_reproduces_affine_fields_and_filtered_edges_clamp_instead_of_zero() {
        let n = 10;
        let water: Vec<_> = (0..n * n)
            .map(|i| Water {
                height: (i % n) as f32 * 3.0 + (i / n) as f32 * 5.0,
                ..Water::default()
            })
            .collect();
        let geo = vec![Geometry::default(); n * n];
        let field = Field::new(n, n, &water, &geo).unwrap();
        assert!((field.sample([3.25, 4.75], 0) - 33.5).abs() < 1e-5);
        assert!((field.sample([-100.0, 4.75], 0) - 23.75).abs() < 1e-5);
        assert_eq!(field.water(-1, 4), Water::default());
    }

    #[test]
    fn cubic_overshoot_reverts_to_bilinear() {
        let n = 10;
        let water: Vec<_> = (0..n * n)
            .map(|i| Water {
                depth: match i % n {
                    3 => 1.0,
                    4 => 0.9,
                    _ => 0.0,
                },
                ..Water::default()
            })
            .collect();
        let geo = vec![Geometry::default(); n * n];
        let field = Field::new(n, n, &water, &geo).unwrap();
        assert!(basic_cubic([0.0, 1.0, 0.9, 0.0], 0.3) > 1.0);
        assert!((field.sample([3.3, 4.5], 1) - 0.97).abs() < 1e-6);
    }

    #[test]
    fn staggered_advection_translates_a_linear_depth_field_and_suppresses_bed_changes() {
        let n = 10;
        let water: Vec<_> = (0..n * n)
            .map(|i| {
                let depth = 100.0 + (i % n) as f32 * 2.0 + (i / n) as f32 * 3.0;
                Water {
                    height: depth,
                    depth,
                    velocity: [40.0, 20.0],
                }
            })
            .collect();
        let geo = vec![Geometry::default(); n * n];
        let field = Field::new(n, n, &water, &geo).unwrap();
        let p = Parameters::cached_river_variant(20.0, 0.25, 980.0, 1000.0, 1000.0);
        let next = field.advect(4, 4, 0.0, p, Transport::default());
        assert!((next.depth - (water[4 * n + 4].depth - 1.75)).abs() < 1e-4);
        assert_eq!(next.velocity, [40.0, 20.0]);
        assert_eq!(
            field.advect(4, 4, -50.0, p, Transport::default()),
            Water::default()
        );
    }

    #[test]
    fn velocity_extrapolation_preserves_source_tie_order_and_does_not_filter_neighbor_colliders() {
        let n = 5;
        let mut water = vec![Water::default(); n * n];
        let mut geo = vec![Geometry::default(); n * n];
        geo[2 * n + 2].collider = 1.0;
        geo[2 * n + 1].collider = 1.0;
        water[2 * n + 1] = Water {
            height: 10.0,
            depth: 10.0,
            velocity: [1.0, 0.0],
        };
        water[2 * n + 3] = Water {
            height: 10.0,
            depth: 10.0,
            velocity: [9.0, 0.0],
        };
        let field = Field::new(n, n, &water, &geo).unwrap();
        assert_eq!(
            field
                .extrapolate_velocity(2, 2, Transport::default())
                .velocity,
            [1.0, 0.0]
        );
        assert_eq!(
            field.extrapolate_velocity(2, 1, Transport::default()),
            Water::default()
        );
    }

    #[test]
    fn height_extrapolation_keeps_negative_depth_when_neighbor_surface_is_below_collider_bed() {
        let n = 5;
        let mut water = vec![Water::default(); n * n];
        let mut geo = vec![Geometry::default(); n * n];
        geo[2 * n + 2] = Geometry {
            bottom: 20.0,
            collider: 1.0,
            ..Geometry::default()
        };
        water[2 * n + 1] = Water {
            height: 10.0,
            depth: 10.0,
            ..Water::default()
        };
        let field = Field::new(n, n, &water, &geo).unwrap();
        let result = field.extrapolate_height(2, 2, Transport::default());
        assert_eq!((result.height, result.depth), (10.0, -10.0));
    }

    #[test]
    fn presentation_preserves_source_diagonal_predicate_and_separates_render_depth() {
        let n = 9;
        let mut water = vec![Water::default(); n * n];
        let geo = vec![Geometry::default(); n * n];
        for (x, y, h) in [(3, 3, 10.0), (2, 2, 20.0), (3, 4, 999.0)] {
            water[y * n + x] = Water {
                height: h,
                depth: h,
                ..Water::default()
            };
        }
        let field = Field::new(n, n, &water, &geo).unwrap();
        let (hydraulic, rendered) = field.present(
            4,
            4,
            Presentation {
                half_width: 3,
                ..Presentation::default()
            },
        );
        assert!((hydraulic.height - 50.0 / 3.0).abs() < 1e-5);
        assert_eq!(hydraulic.depth, 0.0);
        assert_eq!(rendered.depth, 10.0);
        assert_eq!(field.present(0, 0, Presentation::default()).1.depth, 0.0);
    }

    #[test]
    fn normals_follow_smoothed_height_then_hydraulic_height_is_restored() {
        let n = 5;
        let water: Vec<_> = (0..n * n)
            .map(|i| Water {
                height: (i % n) as f32 * 3.0 + (i / n) as f32 * 4.0,
                depth: 2.0,
                velocity: [3.0, 4.0],
            })
            .collect();
        let geo = vec![Geometry::default(); n * n];
        let field = Field::new(n, n, &water, &geo).unwrap();
        let (reset, normal) = field.normal_and_reset(2, 2, 2.0);
        assert_eq!(reset.height, 2.0);
        assert_eq!(reset.velocity, [3.0, 4.0]);
        for (actual, expected) in normal[..3].iter().zip([-3.0, -4.0, 2.0]) {
            assert!((*actual - expected / 29.0_f32.sqrt()).abs() < 1e-6);
        }
    }
}
