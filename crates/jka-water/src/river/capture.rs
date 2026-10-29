//! Reconstruction of WaterAdvanced's five orthographic depth captures.
//!
//! Source: Grid2D_SW_River cached stages 5/6, ComputeBottomContour,
//! CustomHlsl0265E811... (dilation), CustomHlsl14EFA242... (bridge selection)
//! and CustomHlsl8DA87775... (border colliders). All heights are local cm.
//! Capture channels are reusable; the selected bottom depends on WaterInfo.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.

use super::Geometry;

pub const CHANNELS: usize = 5;
pub const PREPARE_SHADER: &str = include_str!("capture.wgsl");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Channel {
    Landscape = 0,
    ActorsTop = 1,
    ActorsUnder = 2,
    DilatedTop = 3,
    DilatedUnder = 4,
}

impl Channel {
    pub const ALL: [Self; CHANNELS] = [
        Self::Landscape,
        Self::ActorsTop,
        Self::ActorsUnder,
        Self::DilatedTop,
        Self::DilatedUnder,
    ];

    pub fn top_down(self) -> bool {
        matches!(self, Self::Landscape | Self::ActorsTop | Self::DilatedTop)
    }
}

/// Camera and clip planes are explicit capture inputs. Empty pixels contain
/// `far`, the linear depth at the orthographic depth buffer's clear value.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub height: f32,
    pub near: f32,
    pub far: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub dimensions: [u32; 2],
    /// Actual unit-to-world XY extent, centered at the local simulation origin.
    pub world_size: [f32; 2],
    /// The capture is square, as in ShallowWaterRiverActor::InitializeCaptureDI.
    pub capture_width: f32,
    pub dx: f32,
    pub bottom_depth: f32,
    pub dilation: f32,
    pub cameras: [Camera; CHANNELS],
}

impl Settings {
    pub fn validate(self) -> Result<(), &'static str> {
        if self.dimensions.contains(&0)
            || self.world_size.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || !self.capture_width.is_finite()
            || self.capture_width <= 0.0
            || !self.dx.is_finite()
            || self.dx <= 0.0
            || !self.bottom_depth.is_finite()
            || !self.dilation.is_finite()
            || self.dilation < 0.0
            || self.cameras.iter().any(|c| {
                !c.height.is_finite()
                    || !c.near.is_finite()
                    || !c.far.is_finite()
                    || c.far <= c.near
                    || !(c.far - c.near).is_finite()
                    || !(c.height + c.far).is_finite()
                    || !(c.height - c.far).is_finite()
            })
        {
            return Err("invalid river capture dimensions, projection or contour settings");
        }
        Ok(())
    }

    pub fn floor(self) -> f32 {
        (-self.dx).min(self.bottom_depth)
    }

    /// Niagara converts min(Dilation / dx, 40) to integer, truncating it.
    pub fn dilation_radius(self) -> i32 {
        (self.dilation / self.dx).min(40.0) as i32
    }
}

/// Projected, undilated contours. CPU padding is explicit; the GPU packs the
/// five scalars at 20 bytes/cell to stay within storage binding limits.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Contours {
    pub landscape: f32,
    pub actors_top: f32,
    pub actors_under: f32,
    pub dilated_top: f32,
    pub dilated_under: f32,
    pub padding: [f32; 3],
}

/// TextureProjectDepth uses bilinear clamp sampling and validates UV in [0,1).
/// Input is the five square, row-major linear-depth images. Rows here increase
/// with local world Y; a renderer must account for its camera/view orientation.
pub fn project(
    settings: Settings,
    resolution: u32,
    depths: [&[f32]; CHANNELS],
) -> Result<Vec<Contours>, &'static str> {
    settings.validate()?;
    let pixels = u64::from(resolution) * u64::from(resolution);
    if resolution == 0
        || depths
            .iter()
            .any(|v| v.len() as u64 != pixels || v.iter().any(|d| !d.is_finite()))
    {
        return Err("invalid river depth captures");
    }
    let count = u64::from(settings.dimensions[0]) * u64::from(settings.dimensions[1]);
    let mut output =
        Vec::with_capacity(usize::try_from(count).map_err(|_| "capture grid overflow")?);
    for y in 0..settings.dimensions[1] {
        for x in 0..settings.dimensions[0] {
            let uv: [f32; 2] = std::array::from_fn(|axis| {
                0.5 + (([x, y][axis] as f32 + 0.5) / settings.dimensions[axis] as f32 - 0.5)
                    * settings.world_size[axis]
                    / settings.capture_width
            });
            let mut values = [settings.floor(); CHANNELS];
            if uv.iter().all(|v| *v >= 0.0 && *v < 1.0) {
                let texel = uv.map(|v| v * resolution as f32 - 0.5);
                let lo = texel.map(|v| v.floor() as i32);
                let fraction = std::array::from_fn::<_, 2, _>(|i| texel[i] - lo[i] as f32);
                for channel in Channel::ALL {
                    let image = depths[channel as usize];
                    let load = |dx: i32, dy: i32| {
                        let ix = (lo[0] + dx).clamp(0, resolution as i32 - 1) as usize;
                        let iy = (lo[1] + dy).clamp(0, resolution as i32 - 1) as usize;
                        image[iy * resolution as usize + ix]
                    };
                    let a = load(0, 0) + (load(1, 0) - load(0, 0)) * fraction[0];
                    let b = load(0, 1) + (load(1, 1) - load(0, 1)) * fraction[0];
                    let depth = a + (b - a) * fraction[1];
                    let camera = settings.cameras[channel as usize];
                    let height = if channel.top_down() {
                        camera.height - depth
                    } else {
                        camera.height + depth
                    };
                    // The original also applies this MAX to underside captures.
                    values[channel as usize] = height.max(settings.floor());
                }
            }
            output.push(Contours {
                landscape: values[0],
                actors_top: values[1],
                actors_under: values[2],
                dilated_top: values[3],
                dilated_under: values[4],
                padding: [0.0; 3],
            });
        }
    }
    Ok(output)
}

/// Stages 5/6: max-dilate BOTH dilated-actor channels, merge undilated tops
/// with max and undersides with min, then select the bed using spline height.
pub fn geometry(
    settings: Settings,
    contours: &[Contours],
    water_height: &[f32],
) -> Result<Vec<Geometry>, &'static str> {
    settings.validate()?;
    let [nx, ny] = settings.dimensions.map(|n| n as usize);
    let count = nx.checked_mul(ny).ok_or("capture grid overflow")?;
    if contours.len() != count
        || water_height.len() != count
        || water_height.iter().any(|h| !h.is_finite())
        || contours.iter().any(|c| {
            [
                c.landscape,
                c.actors_top,
                c.actors_under,
                c.dilated_top,
                c.dilated_under,
            ]
            .iter()
            .any(|v| !v.is_finite())
        })
    {
        return Err("invalid projected river contour or WaterInfo fields");
    }
    let radius = settings.dilation_radius();
    let mut output = Vec::with_capacity(count);
    for y in 0..ny as i32 {
        for x in 0..nx as i32 {
            let i = y as usize * nx + x as usize;
            let cell = contours[i];
            let mut top = -100000.0f32;
            let mut under = -100000.0f32;
            for dx in -radius..=radius {
                for dy in -radius..=radius {
                    let ix = (x + dx).clamp(0, nx as i32 - 1) as usize;
                    let iy = (y + dy).clamp(0, ny as i32 - 1) as usize;
                    let sample = contours[iy * nx + ix];
                    top = top.max(sample.dilated_top);
                    under = under.max(sample.dilated_under);
                }
            }
            top = top.max(cell.actors_top);
            under = under.min(cell.actors_under);
            if water_height[i] - under <= 0.0
                && under > cell.landscape
                && top > under
                && top > cell.landscape
            {
                top = cell.landscape;
            }
            // Cached border is two cells. Terrain elevation isn't a binary
            // collider: it enters through the bottom contour instead.
            output.push(Geometry {
                bottom: top.max(cell.landscape),
                collider: if x < 2 || y < 2 || x > nx as i32 - 3 || y > ny as i32 - 3 {
                    1.0
                } else {
                    0.0
                },
                collision_velocity: [0.0; 2],
            });
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            dimensions: [8, 6],
            world_size: [8.0, 6.0],
            capture_width: 8.0,
            dx: 1.0,
            bottom_depth: -20.0,
            dilation: 0.0,
            cameras: std::array::from_fn(|i| Camera {
                height: if Channel::ALL[i].top_down() {
                    100.0
                } else {
                    -100.0
                },
                near: 0.0,
                far: 1000.0,
            }),
        }
    }

    #[test]
    fn river_capture_linear_depth_sampling_and_missing_captures() {
        let s = settings();
        let depth: Vec<_> = (0..16).map(|i| 110.0 - (i % 4) as f32 * 2.0).collect();
        let missing = vec![1000.0; 16];
        let contours = project(s, 4, [&depth, &missing, &missing, &missing, &missing]).unwrap();
        // Cell x=3 maps to texel coordinate 1.25; linear depth =107.5.
        assert_eq!(contours[3].landscape, -7.5);
        assert_eq!(contours[3].actors_top, -20.0);
        assert_eq!(contours[3].actors_under, 900.0);
        assert_eq!(contours[0].landscape, -10.0); // clamp at texture edge
        let wider = Settings {
            world_size: [16.0, 6.0],
            ..s
        };
        let out = project(wider, 4, [&depth; 5]).unwrap();
        assert_eq!(
            out[0],
            Contours {
                landscape: -20.0,
                actors_top: -20.0,
                actors_under: -20.0,
                dilated_top: -20.0,
                dilated_under: -20.0,
                padding: [0.0; 3],
            }
        );
        assert!(project(s, 0, [&[]; 5]).is_err());
        let mut ortho = s;
        ortho.cameras[0].near = -100.0;
        assert!(ortho.validate().is_ok()); // orthographic near planes can be behind the camera
    }

    #[test]
    fn river_capture_bridge_bed_depends_on_water_level_and_strict_bounds() {
        let s = settings();
        let c = Contours {
            landscape: -10.0,
            actors_top: 8.0,
            actors_under: 5.0,
            dilated_top: -20.0,
            dilated_under: 900.0,
            padding: [0.0; 3],
        };
        let contours = vec![c; 48];
        for (height, expected) in [(4.0, -10.0), (5.0, -10.0), (5.01, 8.0)] {
            let g = geometry(s, &contours, &vec![height; 48]).unwrap();
            assert!(g.iter().all(|v| v.bottom == expected));
            assert_eq!(g[2 * 8 + 2].collider, 0.0);
            assert_eq!(g[1 * 8 + 2].collider, 1.0);
            assert_eq!(g[2 * 8 + 6].collider, 1.0);
        }
        // Equal top/under isn't a bridge; preserve the source's strict >.
        let equal = vec![
            Contours {
                actors_under: 8.0,
                ..c
            };
            48
        ];
        assert_eq!(geometry(s, &equal, &vec![4.0; 48]).unwrap()[18].bottom, 8.0);
    }

    #[test]
    fn river_capture_dilates_both_channels_with_max_before_bridge_selection() {
        let s = Settings {
            dilation: 1.9,
            ..settings()
        };
        assert_eq!(s.dilation_radius(), 1);
        assert_eq!(
            Settings {
                dilation: 1000.0,
                ..s
            }
            .dilation_radius(),
            40
        );
        let mut c = vec![
            Contours {
                landscape: -50.0,
                actors_top: -50.0,
                actors_under: 1000.0,
                dilated_top: -40.0,
                dilated_under: 5.0,
                padding: [0.0; 3],
            };
            48
        ];
        c[3 * 8 + 3].dilated_top = 20.0;
        c[3 * 8 + 3].dilated_under = 10.0;
        let g = geometry(s, &c, &vec![9.0; 48]).unwrap();
        assert_eq!(g[3 * 8 + 4].bottom, -50.0); // max underside is 10: water below it
        assert_eq!(g[3 * 8 + 5].bottom, -40.0); // outside the one-cell kernel
        let g = geometry(s, &c, &vec![11.0; 48]).unwrap();
        assert_eq!(g[3 * 8 + 4].bottom, 20.0);
        let invalid = Settings {
            dilation: f32::NAN,
            ..s
        };
        assert!(geometry(invalid, &c, &vec![11.0; 48]).is_err());
    }
}
