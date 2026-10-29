//! WaterInfoMerge.usf, WaterInfoFinalize.usf and WaterDataFunctions.ush.
//! Source-depth inputs are from matching point-sampled capture pixels. These
//! functions do not manufacture water-body meshes, spline data or discharge.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
use super::boundary::WaterInfo;

pub const SHADER: &str = include_str!("water_info.wgsl");
pub const MAX_FLOW_VELOCITY: f32 = 1024.0;

#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub water_min: f32,
    pub water_max: f32,
    pub ground_min: f32,
    pub capture_z: f32,
    pub underground_offset: f32,
}

impl Zone {
    pub fn validate(self) -> Result<(), &'static str> {
        if [
            self.water_min,
            self.water_max,
            self.ground_min,
            self.capture_z,
            self.underground_offset,
            self.water_max - self.water_min,
            self.water_max - self.ground_min,
        ]
        .iter()
        .any(|v| !v.is_finite())
            || self.water_min > self.water_max
            || self.ground_min > self.water_min
            || self.underground_offset < 0.0
        {
            return Err("invalid native WaterInfo height ranges or capture");
        }
        Ok(())
    }

    pub fn merge(self, input: Pixel) -> [f32; 4] {
        let ground = (self.capture_z - input.ground_depth).max(self.ground_min);
        let water = self.capture_z - input.water_depth;
        let mut dilation = self.capture_z - input.dilation_depth;
        let has_dilation = dilation <= self.water_max && dilation >= self.water_min;
        let mut valid_water = water >= self.water_min;
        // Source shader hardcodes 128, despite declaring a separately supplied
        // DilationOverwriteMinimumDistance uniform. Do not substitute that cvar.
        if has_dilation && water + 128.0 < ground {
            valid_water = false;
        }
        let g = normalize(ground, self.ground_min, self.water_max);
        if valid_water {
            [
                input.encoded_velocity[0],
                input.encoded_velocity[1],
                normalize(water, self.water_min, self.water_max),
                g,
            ]
        } else if has_dilation {
            if dilation > ground {
                dilation = ground - self.underground_offset;
            }
            [
                0.0,
                0.0,
                normalize(dilation, self.water_min, self.water_max),
                g,
            ]
        } else {
            [0.0, 0.0, 0.0, g]
        }
    }

    /// Finalize keeps heights and averages encoded velocity only at neighbours
    /// whose decoded water height is strictly above ground. Out-of-range Load
    /// returns zero; zero padding can count as wet for some zone ranges.
    pub fn finalize(
        self,
        dimensions: [usize; 2],
        merged: &[[f32; 4]],
        radius: i32,
    ) -> Result<Vec<[f32; 4]>, &'static str> {
        let [width, height] = dimensions;
        if width == 0
            || height == 0
            || width.checked_mul(height) != Some(merged.len())
            || width > i32::MAX as usize
            || height > i32::MAX as usize
        {
            return Err("invalid WaterInfo finalize field");
        }
        let radius = radius.clamp(0, 16);
        let mut output = Vec::with_capacity(merged.len());
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let mut sum = [0.0; 2];
                let mut count = 0.0;
                for dx in -radius..=radius {
                    for dy in -radius..=radius {
                        let xx = x + dx;
                        let yy = y + dy;
                        let v = if xx >= 0 && yy >= 0 && xx < width as i32 && yy < height as i32 {
                            merged[yy as usize * width + xx as usize]
                        } else {
                            [0.0; 4]
                        };
                        let water = v[2] * (self.water_max - self.water_min) + self.water_min;
                        let ground = v[3] * (self.water_max - self.ground_min) + self.ground_min;
                        if water > ground {
                            sum[0] += v[0];
                            sum[1] += v[1];
                            count += 1.0;
                        }
                    }
                }
                if count > 0.0 {
                    sum[0] /= count;
                    sum[1] /= count;
                }
                let v = merged[y as usize * width + x as usize];
                output.push([sum[0], sum[1], v[2], v[3]]);
            }
        }
        Ok(output)
    }

    pub fn decode(self, encoded: [f32; 4], origin_z: f32) -> WaterInfo {
        let ground = encoded[3] * (self.water_max - self.ground_min) + self.ground_min;
        let mut water = encoded[2] * (self.water_max - self.water_min) + self.water_min;
        if encoded[2] <= 0.0 {
            water = ground - 256.0;
        }
        WaterInfo {
            height: water - origin_z,
            depth: water - ground,
            flow_velocity: [
                (encoded[0] - 0.5) * 2.0 * MAX_FLOW_VELOCITY,
                (encoded[1] - 0.5) * 2.0 * MAX_FLOW_VELOCITY,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Pixel {
    pub ground_depth: f32,
    pub water_depth: f32,
    pub dilation_depth: f32,
    /// RG straight from the water colour pass; these are already encoded.
    pub encoded_velocity: [f32; 2],
}

fn normalize(height: f32, min: f32, max: f32) -> f32 {
    if min == max {
        1.0
    } else {
        ((height - min) / (max - min)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn zone() -> Zone {
        Zone {
            water_min: 0.0,
            water_max: 1000.0,
            ground_min: -1000.0,
            capture_z: 2000.0,
            underground_offset: 64.0,
        }
    }
    #[test]
    fn merge_preserves_underground_dilation_rule_and_missing_water_decodes_below_ground() {
        let z = zone();
        let merged = z.merge(Pixel {
            ground_depth: 1500.0,
            water_depth: 1900.0,
            dilation_depth: 1400.0,
            encoded_velocity: [0.75, 0.25],
        });
        // Water 100+128<ground 500: reject water; squash dilation 600 to 436.
        assert_eq!(merged, [0.0, 0.0, 0.436, 0.75]);
        let dry = z.decode([0.0, 0.0, 0.0, 0.75], 100.0);
        assert_eq!(
            (dry.height, dry.depth, dry.flow_velocity),
            (144.0, -256.0, [-1024.0, -1024.0])
        );
    }
    #[test]
    fn finalize_preserves_zero_load_edge_semantics_and_strict_wet_test() {
        let z = zone();
        // Zero outside decodes to water=0, ground=-1000, so it counts as wet.
        let output = z.finalize([1, 1], &[[0.75, 0.25, 0.5, 0.5]], 1).unwrap();
        assert_eq!(output[0], [0.75 / 9.0, 0.25 / 9.0, 0.5, 0.5]);
        let equal = z.finalize([1, 1], &[[0.75, 0.25, 0.0, 0.5]], 0).unwrap();
        assert_eq!(equal[0], [0.0, 0.0, 0.0, 0.5]);
    }
    #[test]
    fn constant_height_range_uses_one_and_velocity_is_not_reinterpreted_as_discharge() {
        assert_eq!(normalize(999.0, 10.0, 10.0), 1.0);
        let wet = zone().decode([0.75, 0.25, 0.5, 0.5], 50.0);
        assert_eq!(
            (wet.height, wet.depth, wet.flow_velocity),
            (450.0, 500.0, [512.0, -512.0])
        );
    }
}
