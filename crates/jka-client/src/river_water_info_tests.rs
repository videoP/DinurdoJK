use crate::river_water_info::{self as gpu, Precision};
use jka_water::river::water_info::{Pixel, Zone};

pub(super) fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    for precision in [Precision::Full, Precision::Half] {
        // Dyadic normalized heights/velocities make the Half case exact through
        // both typed stores. Full additionally exercises nonuniform velocity.
        let zone = Zone {
            water_min: -256.0,
            water_max: 256.0,
            ground_min: -256.0,
            capture_z: 1024.0,
            underground_offset: 64.0,
        };
        let pixels: Vec<_> = (0..12)
            .map(|i| {
                let (ground, water, dilation) = match i {
                    0 => (0.0, -2000.0, -2000.0),
                    1 => (0.0, -200.0, 100.0),
                    2 => (200.0, -200.0, 100.0),
                    _ => (0.0, 128.0, -2000.0),
                };
                Pixel {
                    ground_depth: 1024.0 - ground,
                    water_depth: 1024.0 - water,
                    dilation_depth: 1024.0 - dilation,
                    encoded_velocity: if matches!(precision, Precision::Half) {
                        [0.75, 0.25]
                    } else if i == 11 {
                        [1.25, -0.25] // Merge/decode do not clamp encoded flow.
                    } else {
                        [
                            0.125 + (i % 4) as f32 * 0.125,
                            0.875 - (i % 3) as f32 * 0.125,
                        ]
                    },
                }
            })
            .collect();
        let merged: Vec<_> = pixels.iter().map(|v| zone.merge(*v)).collect();
        let finalized = zone.finalize([4, 3], &merged, 1).unwrap();
        let expected: Vec<_> = finalized
            .iter()
            .map(|v| {
                let w = zone.decode(*v, 32.0);
                [w.height, w.depth, w.flow_velocity[0], w.flow_velocity[1]]
            })
            .collect();
        for wrap in [false, true] {
            let settings = gpu::Settings {
                dimensions: [4, 3],
                simulation_dimensions: [4, 3],
                zone,
                origin_z: 32.0,
                grid_min: if wrap { [4.0, 3.0] } else { [0.0; 2] },
                grid_size: [4.0, 3.0],
                zone_min: [0.0; 2],
                zone_size: [4.0, 3.0],
                blur_radius: 1,
                precision,
            };
            let mut encoder = device.create_command_encoder(&Default::default());
            let field = gpu::encode(device, &mut encoder, settings, &pixels).unwrap();
            queue.submit(Some(encoder.finish()));
            super::compare(
                &super::read_buffer(device, queue, field.buffer()),
                &expected,
                &format!("native WaterInfo {precision:?} wrap={wrap}"),
            );
        }
    }
    check_half_store_rounding(device, queue);
    check_scene_captures(device, queue);
}

fn check_scene_captures(device: &wgpu::Device, queue: &wgpu::Queue) {
    use crate::river_water_info_capture::{self as capture, Body, Ground};
    use jka_water::river::water_body::{self, Sample};
    let zone = Zone {
        water_min: -256.0,
        water_max: 256.0,
        ground_min: -256.0,
        capture_z: 1024.0,
        underground_offset: 64.0,
    };
    let settings = gpu::Settings {
        dimensions: [16, 8],
        simulation_dimensions: [16, 8],
        zone,
        origin_z: 0.0,
        grid_min: [-4.0, -4.0],
        grid_size: [16.0, 8.0],
        zone_min: [-4.0, -4.0],
        zone_size: [16.0, 8.0],
        blur_radius: 0,
        precision: Precision::Full,
    };
    let body_settings = water_body::Settings {
        dilation: 1.0,
        relative_yaw_degrees: 0.0,
        maximum_velocity: 1024.0,
    };
    let sample = |x, z, distance, width, velocity| Sample {
        position: [x, 0.0, z],
        distance,
        width,
        velocity,
        tangent: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    let ground = [
        [-4.0, -4.0, 0.0],
        [12.0, -4.0, 0.0],
        [12.0, 4.0, 0.0],
        [-4.0, 4.0, 0.0],
        [4.0, -4.0, 400.0],
        [6.0, -4.0, 400.0],
        [6.0, 4.0, 400.0],
        [4.0, 4.0, 400.0],
    ];
    let indices = [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
    for wrap_direction in [false, true] {
        let world_origin: [f64; 3] = if wrap_direction {
            [10000.0, -20000.0, 300.0]
        } else {
            [0.0; 3]
        };
        let origin_z = world_origin[2] as f32;
        let zone = Zone {
            water_min: zone.water_min + origin_z,
            water_max: zone.water_max + origin_z,
            ground_min: zone.ground_min + origin_z,
            capture_z: zone.capture_z + origin_z,
            ..zone
        };
        let settings = gpu::Settings {
            zone,
            origin_z,
            grid_min: std::array::from_fn(|i| settings.grid_min[i] + world_origin[i] as f32),
            zone_min: std::array::from_fn(|i| settings.zone_min[i] + world_origin[i] as f32),
            ..settings
        };
        let mut base = water_body::from_evaluated_samples(
            &[
                sample(0.0, 128.0, 0.0, 4.0, 1024.0),
                sample(8.0, 128.0, 8.0, 4.0, 1024.0),
            ],
            body_settings,
        )
        .unwrap();
        if wrap_direction {
            // Stored static-mesh fixture: nearly 2PI and nearly zero. The
            // material must interpolate converted Cartesian flow across this
            // seam, not interpolate these packed angles in the fragment shader.
            base.surface.colors = vec![
                [255, 254, 0, 0],
                [255, 254, 0, 0],
                [255, 1, 0, 0],
                [255, 1, 0, 0],
            ];
        }
        let upper = water_body::from_evaluated_samples(
            &[
                sample(2.0, 192.0, 0.0, 2.0, -512.0),
                sample(6.0, 192.0, 4.0, 2.0, -512.0),
            ],
            water_body::Settings {
                dilation: 0.0,
                ..body_settings
            },
        )
        .unwrap();
        let bodies = [
            Body {
                mesh: &base,
                local_to_capture: if wrap_direction {
                    [
                        [-1.0, 0.0, 0.0, 8.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                    ]
                } else {
                    capture::IDENTITY
                },
            },
            Body {
                mesh: &upper,
                local_to_capture: capture::IDENTITY,
            },
        ];
        let mut pixels = Vec::new();
        for y in 0..8 {
            for x in 0..16 {
                let point = [x as f64 - 3.5, y as f64 - 3.5];
                let ground_z = if point[0] > 4.0 && point[0] < 6.0 {
                    400.0
                } else {
                    0.0
                };
                let mut water = None;
                let mut dilation = None;
                for b in &bodies {
                    // The seam case also reflects the first body about X=4.
                    // Evaluate its native mesh at the inverse-transformed point.
                    let local_point = if b.local_to_capture[0][0] < 0.0 {
                        [8.0 - point[0], point[1]]
                    } else {
                        point
                    };
                    if let Some(hit) = raster_reference(&b.mesh.surface, local_point) {
                        if water
                            .as_ref()
                            .is_none_or(|old: &(f32, [f32; 2])| hit.0 >= old.0)
                        {
                            water = Some(hit);
                        }
                    }
                    if let Some(hit) = raster_reference(&b.mesh.dilation, local_point) {
                        if dilation.is_none_or(|old: f32| hit.0 >= old) {
                            dilation = Some(hit.0);
                        }
                    }
                }
                pixels.push(Pixel {
                    ground_depth: 1024.0 - ground_z,
                    water_depth: water.map_or(capture::FAR_CM, |v| 1024.0 - v.0),
                    dilation_depth: dilation.map_or(capture::FAR_CM, |z| 1024.0 - z),
                    encoded_velocity: water.map_or([0.0; 2], |v| v.1),
                });
            }
        }
        let merged: Vec<_> = pixels.iter().map(|p| zone.merge(*p)).collect();
        // Under raised ground the regular water survives merge because the
        // native dilation mesh is hollow. Finalize then clears this cell's
        // flow because its water is below ground; do not expect retained flow
        // in the final decoded field at this location.
        assert!(raster_reference(&base.dilation, [4.5, 1.5]).is_none());
        assert!(merged[5 * 16 + 8][0] > 0.99);
        let final_pixels = zone.finalize([16, 8], &merged, 0).unwrap();
        let expected: Vec<_> = final_pixels
            .iter()
            .map(|p| {
                let w = zone.decode(*p, origin_z);
                [w.height, w.depth, w.flow_velocity[0], w.flow_velocity[1]]
            })
            .collect();
        let mut encoder = device.create_command_encoder(&Default::default());
        let field = capture::encode(
            device,
            &mut encoder,
            settings,
            world_origin,
            &[Ground {
                positions: &ground,
                indices: &indices,
                sides: crate::river_capture::Sides::CounterClockwise,
            }],
            &bodies,
        )
        .unwrap();
        queue.submit(Some(encoder.finish()));
        super::compare(
            &super::read_buffer(device, queue, field.buffer()),
            &expected,
            &format!("native WaterInfo scene captures, angle seam={wrap_direction}"),
        );
        assert!(expected
            .iter()
            .any(|p| p[0] == 128.0 && p[1] == -128.0 && p[2] == -1024.0)); // Ground clamps at water_max; finalize drops below-ground flow.
        assert!(expected.iter().any(|p| p[0] == 192.0 && p[2] < -500.0)); // Highest overlapping body.
        assert!(expected.iter().any(|p| p[0] == -64.0 && p[1] == -64.0)); // Squashed edge dilation.
        assert!(expected.iter().any(|p| p[1] == -256.0)); // Native background depth resolves to far.
    }
}

/// Independent pixel-centre barycentric reference over the source-derived mesh.
/// Captures are orthographic, so flow is affine in screen XY after its vertex
/// material evaluation. This is test-only; production never rasterizes on CPU.
fn raster_reference(
    mesh: &jka_water::river::water_body::Mesh,
    p: [f64; 2],
) -> Option<(f32, [f32; 2])> {
    let mut hit = None;
    for t in mesh.indices.chunks_exact(3) {
        let [a, b, c] = [
            mesh.positions[t[0] as usize],
            mesh.positions[t[1] as usize],
            mesh.positions[t[2] as usize],
        ];
        let denominator = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if denominator == 0.0 {
            continue;
        }
        let u = ((b[1] - c[1]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[1] - c[1])) / denominator;
        let v = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / denominator;
        let weights = [u, v, 1.0 - u - v];
        if weights.iter().any(|w| *w < -1e-10) {
            continue;
        }
        let z = (weights[0] * a[2] + weights[1] * b[2] + weights[2] * c[2]) as f32;
        let mut flow = [0.0; 2];
        for i in 0..3 {
            let color = mesh.colors[t[i] as usize];
            let magnitude = (color[0] as f32 / 255.0) * 1024.0;
            let direction = (color[1] as f32 / 255.0) * 2.0 * std::f32::consts::PI;
            flow[0] += (magnitude * direction.cos() / (2.0 * 1024.0) + 0.5) * weights[i] as f32;
            flow[1] += (magnitude * direction.sin() / (2.0 * 1024.0) + 0.5) * weights[i] as f32;
        }
        if hit.as_ref().is_none_or(|old: &(f32, [f32; 2])| z >= old.0) {
            hit = Some((z, flow));
        }
    }
    hit
}

/// Check each typed-write boundary without imposing a CPU half-conversion mode
/// on the GPU. The first changes the wet predicate; the second snaps a blur
/// result to the half lattice. No cross-backend bitwise-rounding requirement.
fn check_half_store_rounding(device: &wgpu::Device, queue: &wgpu::Queue) {
    let zone = Zone {
        water_min: -256.0,
        water_max: 256.0,
        ground_min: -256.0,
        capture_z: 1024.0,
        underground_offset: 64.0,
    };
    let settings = gpu::Settings {
        dimensions: [4, 1],
        simulation_dimensions: [4, 1],
        zone,
        origin_z: 32.0,
        grid_min: [0.0; 2],
        grid_size: [4.0, 1.0],
        zone_min: [0.0; 2],
        zone_size: [4.0, 1.0],
        blur_radius: 1,
        precision: Precision::Half,
    };
    for wet in [false, true] {
        let pixels: Vec<_> = [0.5, 0.5, 0.50048828125, 0.50048828125]
            .into_iter()
            .map(|x| Pixel {
                // These close heights both become .75 at the first store.
                // Before that store water>ground; afterwards water==ground.
                ground_depth: 1024.0 - ((if wet { 0.5 } else { 0.7501 }) * 512.0 - 256.0),
                water_depth: 1024.0 - (0.7502 * 512.0 - 256.0),
                dilation_depth: 3024.0,
                encoded_velocity: [x, 0.5],
            })
            .collect();
        let mut encoder = device.create_command_encoder(&Default::default());
        let field = gpu::encode(device, &mut encoder, settings, &pixels).unwrap();
        queue.submit(Some(encoder.finish()));
        let actual = super::read_buffer(device, queue, field.buffer());
        let mut expected = [[96.0, 0.0, -1024.0, -1024.0]; 4];
        if wet {
            // Exact half inputs blur to velocities [0, 1/3, 2/3, 1] in f32.
            // The final half store yields integer velocities here. Accept the
            // adjacent half values at 2/3; do not force a device rounding mode.
            assert!(actual[2][2] == 0.0 || actual[2][2] == 1.0);
            expected = [
                [96.0, 128.0, 0.0, 0.0],
                [96.0, 128.0, 0.0, 0.0],
                [96.0, 128.0, actual[2][2], 0.0],
                [96.0, 128.0, 1.0, 0.0],
            ];
        }
        super::compare(&actual, &expected, "native WaterInfo half write boundaries");
    }
}
