//! Native WaterBodyRiverComponent ribbon/dilation construction and flow packing.
//! The caller supplies evaluated native spline samples (local cm). This does
//! not infer a spline from endpoint markers or replace Unreal spline sampling.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub distance: f32,
    pub position: [f64; 3],
    pub tangent: [f64; 3],
    pub up: [f64; 3],
    pub width: f32,
    pub velocity: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub dilation: f32,
    pub relative_yaw_degrees: f64,
    pub maximum_velocity: f32,
}

#[derive(Default)]
pub struct Mesh {
    pub positions: Vec<[f64; 3]>,
    /// RGBA in shader channel order, after the native static-mesh colour build.
    pub colors: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
}

pub struct Meshes {
    pub surface: Mesh,
    pub dilation: Mesh,
}

fn safe_normal(v: [f64; 3]) -> [f64; 3] {
    let squared = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if squared == 1.0 {
        return v;
    }
    if squared < 1e-8 {
        return [0.0; 3];
    }
    v.map(|x| x * (1.0 / squared.sqrt()))
}
fn scaled(v: [f64; 3], scale: f64) -> [f64; 3] {
    v.map(|x| x * scale)
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn basis(s: Sample) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let tangent = safe_normal(s.tangent);
    let up = safe_normal(s.up);
    let normal = safe_normal([
        tangent[1] * up[2] - tangent[2] * up[1],
        tangent[2] * up[0] - tangent[0] * up[2],
        tangent[0] * up[1] - tangent[1] * up[0],
    ]);
    let mut outward = scaled(normal, f64::from(s.width / 2.0));
    outward[2] = 0.0; // Only outward width is flattened, not dilation offset.
    (tangent, normal, outward)
}

impl Mesh {
    fn append(&mut self, positions: &[[f64; 3]], color: [u8; 4]) -> u32 {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(positions);
        self.colors
            .extend(std::iter::repeat_n(color, positions.len()));
        base
    }
    fn triangles(&mut self, triangles: &[[u32; 3]]) {
        self.indices.extend(triangles.iter().flatten());
    }
}

/// Equivalent to the native geometry functions AFTER spline evaluation. The
/// sample distances must begin at zero and include the end of the spline.
/// Native ConvertSplineToPolyLineWithDistances uses a squared 5 cm tolerance;
/// its evaluator and adaptive subdivision are a separate, still required port.
pub fn from_evaluated_samples(
    samples: &[Sample],
    settings: Settings,
) -> Result<Meshes, &'static str> {
    if samples.len() < 2
        || samples.len() > (u32::MAX as usize / 4).saturating_sub(2)
        || samples[0].distance != 0.0
        || !settings.dilation.is_finite()
        || settings.dilation < 0.0
        || !settings.relative_yaw_degrees.is_finite()
        || settings.relative_yaw_degrees.abs() > 360.0
        || !settings.maximum_velocity.is_finite()
        || settings.maximum_velocity <= 0.0
        || samples.iter().enumerate().any(|(i, s)| {
            !s.distance.is_finite()
                || (i > 0 && s.distance <= samples[i - 1].distance)
                || !s.width.is_finite()
                || s.width <= 0.0
                || !s.velocity.is_finite()
                || s.position
                    .iter()
                    .chain(s.tangent.iter())
                    .chain(s.up.iter())
                    .any(|x| !x.is_finite())
                || s.tangent
                    .iter()
                    .chain(s.up.iter())
                    .any(|x| !(x * x).is_finite())
        })
    {
        return Err("invalid evaluated native water-body samples/settings");
    }
    let mut out = Meshes {
        surface: Mesh::default(),
        dilation: Mesh::default(),
    };
    if settings.dilation > 0.0 {
        terminal(&mut out.dilation, samples[0], settings.dilation, false);
    }
    for &s in samples {
        let (tangent, normal, outward) = basis(s);
        let mut direction =
            (tangent[1].atan2(tangent[0]) + settings.relative_yaw_degrees.to_radians()) as f32;
        while direction > std::f32::consts::PI {
            direction -= std::f32::consts::TAU;
        }
        while direction < -std::f32::consts::PI {
            direction += std::f32::consts::TAU;
        }
        if direction < 0.0 {
            direction += std::f32::consts::TAU;
        }
        let mut velocity = s.velocity;
        if velocity < 0.0 {
            velocity *= -1.0;
            direction = (std::f32::consts::PI + direction) % std::f32::consts::TAU;
        }
        let flow = [
            (velocity / settings.maximum_velocity).clamp(0.0, 1.0),
            (direction / std::f32::consts::TAU).clamp(0.0, 1.0),
        ];
        let color = [static_mesh_color(flow[0]), static_mesh_color(flow[1]), 0, 0];
        let left = add(s.position, scaled(outward, -1.0));
        let right = add(s.position, outward);
        let b = out.surface.append(&[left, right], color);
        if b != 0 {
            out.surface
                .triangles(&[[b - 2, b + 1, b - 1], [b - 2, b, b + 1]]);
        }
        if settings.dilation > 0.0 {
            let offset = scaled(normal, f64::from(settings.dilation));
            let b = out.dilation.append(
                &[
                    add(left, scaled(offset, -1.0)),
                    left,
                    right,
                    add(right, offset),
                ],
                [0; 4],
            );
            if s.distance == 0.0 {
                out.dilation
                    .triangles(&[[b - 3, b + 2, b - 2], [b - 3, b + 1, b + 2]]);
            }
            if b != 0 {
                out.dilation.triangles(&[
                    [b - 4, b + 1, b - 3],
                    [b - 4, b, b + 1],
                    [b - 2, b + 2, b + 3],
                    [b - 2, b + 3, b - 1],
                ]);
            }
        }
    }
    if settings.dilation > 0.0 {
        terminal(
            &mut out.dilation,
            *samples.last().unwrap(),
            settings.dilation,
            true,
        );
    }
    if out
        .surface
        .positions
        .iter()
        .chain(out.dilation.positions.iter())
        .flatten()
        .any(|x| !x.is_finite())
    {
        return Err("native water-body mesh overflow");
    }
    Ok(out)
}

fn terminal(mesh: &mut Mesh, s: Sample, dilation: f32, end: bool) {
    let (tangent, normal, outward) = basis(s);
    let offset = scaled(normal, f64::from(dilation));
    let mut along = scaled(
        tangent,
        if end {
            f64::from(dilation)
        } else {
            -f64::from(dilation)
        },
    );
    along[2] = 0.0;
    let left = add(add(s.position, scaled(outward, -1.0)), along);
    let right = add(add(s.position, outward), along);
    let b = mesh.append(
        &[
            add(left, scaled(offset, -1.0)),
            left,
            right,
            add(right, offset),
        ],
        [0; 4],
    );
    if end {
        mesh.triangles(&[
            [b - 4, b, b + 1],
            [b - 4, b + 1, b - 3],
            [b - 3, b + 1, b + 2],
            [b - 3, b + 2, b - 2],
            [b - 2, b + 2, b + 3],
            [b - 2, b + 3, b - 1],
        ]);
    }
}

/// Native MeshConversion's SRGBToLinear, followed by StaticMeshBuilder's
/// ToFColor(true), then the local vertex factory's UNorm8 fetch. Do not upload
/// the original packed float directly and bypass the static-mesh colour write.
fn static_mesh_color(value: f32) -> u8 {
    let linear = if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    };
    let bits = linear
        .clamp(f32::from_bits(114 << 23), f32::from_bits(0x3f7fffff))
        .to_bits();
    let table = VERTEX_COLOR_TABLE[((bits - (114 << 23)) >> 20) as usize];
    let bias = (table >> 16) << 9;
    let scale = table & 0xffff;
    let t = (bits >> 12) & 0xff;
    ((bias + scale * t) >> 16) as u8
}

include!("vertex_color_table.rs");

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(x: f64) -> Sample {
        Sample {
            distance: x as f32,
            position: [x, 0.0, 128.0],
            tangent: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            width: 4.0,
            velocity: 1024.0,
        }
    }
    fn settings() -> Settings {
        Settings {
            dilation: 1.0,
            relative_yaw_degrees: 0.0,
            maximum_velocity: 1024.0,
        }
    }
    #[test]
    fn ribbon_and_hollow_dilation_preserve_native_topology_and_caps() {
        let m = from_evaluated_samples(&[sample(0.0), sample(8.0)], settings()).unwrap();
        assert_eq!(
            m.surface.positions,
            vec![
                [0.0, 2.0, 128.0],
                [0.0, -2.0, 128.0],
                [8.0, 2.0, 128.0],
                [8.0, -2.0, 128.0]
            ]
        );
        assert_eq!(m.surface.indices, vec![0, 3, 1, 0, 2, 3]);
        assert_eq!(m.dilation.positions.len(), 16);
        assert_eq!(m.dilation.indices.len(), 16 * 3);
        assert_eq!(m.dilation.positions[0], [-1.0, 3.0, 128.0]);
        assert_eq!(m.dilation.positions[15], [9.0, -3.0, 128.0]);
        assert!(m.dilation.indices.iter().all(|i| *i < 16));
        assert_eq!(m.surface.colors, vec![[255, 0, 0, 0]; 4]);
    }
    #[test]
    fn signed_velocity_and_static_mesh_byte_encoding_are_retained() {
        let mut a = sample(0.0);
        a.velocity = -512.0;
        let mut b = sample(8.0);
        b.velocity = 4096.0;
        let m = from_evaluated_samples(
            &[a, b],
            Settings {
                dilation: 0.0,
                ..settings()
            },
        )
        .unwrap();
        // Native inverse-gamma/fast-LUT chain puts exact .5 in byte 127,
        // rather than the 128 produced by replacing it with round(x*255).
        assert_eq!(m.surface.colors[0], [127, 127, 0, 0]);
        assert_eq!(m.surface.colors[2], [255, 0, 0, 0]);
        assert!(m.dilation.positions.is_empty());
        for byte in 0..=255 {
            assert_eq!(static_mesh_color(byte as f32 / 255.0), byte);
        }
    }
    #[test]
    fn roll_flattens_width_but_dilation_preserves_its_vertical_component() {
        let mut a = sample(0.0);
        a.up = [0.0, 0.6, 0.8];
        let mut b = sample(8.0);
        b.up = a.up;
        let m = from_evaluated_samples(&[a, b], settings()).unwrap();
        assert!(m.surface.positions.iter().all(|p| p[2] == 128.0));
        assert!((m.dilation.positions[0][2] - 127.4).abs() < 1e-10);
        assert!((m.dilation.positions[3][2] - 128.6).abs() < 1e-10);
    }
}
