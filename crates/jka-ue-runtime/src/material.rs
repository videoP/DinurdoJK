use glam::{Vec3, Vec4};
use jka_ue_assets::material::{Op, Program};
pub struct Sample {
    pub seconds: f32,
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: [f32; 2],
    pub eye: Vec3,
}
pub fn evaluate(program: &Program, sample: &Sample) -> Result<[f32; 4], String> {
    let mut values = [Vec4::ZERO; 256];
    if program.nodes.len() > 256 {
        return Err("material program exceeds evaluator limit".into());
    }
    for (index, node) in program.nodes.iter().enumerate() {
        let a = node
            .inputs
            .first()
            .map(|n| values[*n])
            .unwrap_or(Vec4::ZERO);
        let b = node.inputs.get(1).map(|n| values[*n]).unwrap_or(Vec4::ZERO);
        let c = node.inputs.get(2).map(|n| values[*n]).unwrap_or(Vec4::ZERO);
        let value = match &node.op {
            Op::Constant(v) => *v,
            Op::Time => Vec4::splat(sample.seconds),
            Op::Position => sample.position.extend(1.),
            Op::Normal => sample.normal.normalize_or_zero().extend(0.),
            Op::Uv => Vec4::new(sample.uv[0], sample.uv[1], 0., 0.),
            Op::Fresnel(exponent) => Vec4::splat(
                (1. - sample
                    .normal
                    .normalize_or_zero()
                    .dot((sample.eye - sample.position).normalize_or_zero())
                    .abs())
                .clamp(0., 1.)
                .powf(*exponent),
            ),
            Op::Add => a + b,
            Op::Subtract => a - b,
            Op::Multiply => a * b,
            Op::Divide => Vec4::from_array(std::array::from_fn(|i| {
                a[i] / if b[i].abs() < 1e-8 { 1e-8 } else { b[i] }
            })),
            Op::Min => a.min(b),
            Op::Max => a.max(b),
            Op::Power => Vec4::from_array(std::array::from_fn(|i| a[i].max(0.).powf(b[i]))),
            Op::Sine => Vec4::from_array(a.to_array().map(f32::sin)),
            Op::Cosine => Vec4::from_array(a.to_array().map(f32::cos)),
            Op::Frac => a - a.floor(),
            Op::Saturate => a.clamp(Vec4::ZERO, Vec4::ONE),
            Op::Lerp => a + (b - a) * c,
            Op::Dot => Vec4::splat(a.dot(b)),
            Op::Normalize => a.normalize_or_zero(),
            Op::Swizzle(channels) => Vec4::from_array(channels.map(|n| a[n])),
        };
        if !value.is_finite() {
            return Err("non-finite material result".into());
        }
        values[index] = value;
    }
    Ok(values[program.output].to_array())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn program_preserves_authored_blue_and_changes_with_time() {
        let p=Program::parse(&serde_json::to_vec(&json!({"version":1,"unlit":true,"output":4,"nodes":[{"op":"constant","value":[0.009131,0.310453,0.913099,1.]},{"op":"time"},{"op":"sine","inputs":[1]},{"op":"saturate","inputs":[2]},{"op":"multiply","inputs":[0,3]}]})).unwrap()).unwrap();
        let mut s = Sample {
            seconds: std::f32::consts::FRAC_PI_2,
            position: Vec3::ZERO,
            normal: Vec3::Z,
            uv: [0.; 2],
            eye: Vec3::Z,
        };
        let blue = evaluate(&p, &s).unwrap();
        assert!(blue[2] > blue[1] && blue[1] > blue[0]);
        s.seconds = 0.;
        assert_eq!(evaluate(&p, &s).unwrap(), [0.; 4]);
    }
    #[test]
    fn cyclic_graphs_and_unknown_expressions_fail_at_load() {
        assert!(Program::parse(
            br#"{"version":1,"output":0,"nodes":[{"op":"add","inputs":[0,0]}]}"#
        )
        .is_err());
        assert!(
            Program::parse(br#"{"version":1,"output":0,"nodes":[{"op":"custom_hlsl"}]}"#).is_err()
        );
    }
}
