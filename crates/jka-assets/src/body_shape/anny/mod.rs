//! Rust port of Anny's phenotype.py and barycentric retopology blendshape sampling.
//! Authored targets are exported unchanged from full_model.py by export_runtime.py.
//! Copyright (C) 2025 NAVER Corp. Anny portions: Apache-2.0 (license in archive).
use super::{cross, dot, normalize, sub, BodyShapeParams};
use crate::ghoul2::{GlaAnimation, GlmModel};
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::{Cursor, Read},
    sync::{Arc, Mutex, OnceLock},
};
mod catalog;
pub use catalog::*;
mod transfer;
pub use transfer::{apply, region, regions, supports};
pub use transfer::{closest_triangle, SurfaceTree};

/// Keep Profile's established whole-body deformation. The Anny donor supplies
/// the added detail controls; it must not replace the model's own height/age
/// deformation from universal-body-shaping.patch.
pub fn apply_profile(
    glm: &mut GlmModel,
    gla: &GlaAnimation,
    params: BodyShapeParams,
    details: Details,
    native: Option<&[[f32; 3]]>,
) -> Result<Option<Vec<[f32; 3]>>, String> {
    let detail_pivots = if details.is_neutral() {
        None
    } else {
        apply(glm, gla, BodyShapeParams::default(), details, native)?
    };
    let pivots = super::procedural::apply(
        glm, gla, params, detail_pivots.as_deref().or(native),
    )?;
    Ok(pivots.or(detail_pivots))
}

pub const EXTRA_CONTROLS: [&str; 6] = [
    "gender",
    "cupsize",
    "firmness",
    "african",
    "asian",
    "caucasian",
];
pub const DETAIL_COUNT: usize = 6 + LOCAL_COUNT + FACIAL_COUNT;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Details(pub [f32; DETAIL_COUNT]);
impl Default for Details {
    fn default() -> Self {
        let mut values = [0.; DETAIL_COUNT];
        values[..6].fill(0.5);
        Self(values)
    }
}
impl Details {
    pub fn name(index: usize) -> &'static str {
        if index < 6 {
            EXTRA_CONTROLS[index]
        } else if index < 6 + LOCAL_COUNT {
            LOCAL_CONTROLS[index - 6].0
        } else {
            FACIAL_CONTROLS[index - 6 - LOCAL_COUNT]
        }
    }
    pub fn range(index: usize) -> std::ops::RangeInclusive<f32> {
        if (6..6 + LOCAL_COUNT).contains(&index) {
            -1.0..=1.0
        } else {
            0.0..=1.0
        }
    }
    pub fn clamped(mut self) -> Self {
        let default = Self::default();
        for (i, value) in self.0.iter_mut().enumerate() {
            let range = Self::range(i);
            *value = if value.is_finite() {
                value.clamp(*range.start(), *range.end())
            } else {
                default.0[i]
            };
        }
        self
    }
    pub fn encode(self) -> String {
        let default = Self::default();
        self.clamped()
            .0
            .iter()
            .enumerate()
            .filter(|(i, v)| **v != default.0[*i])
            .map(|(i, v)| format!("{}={v:.3}", Self::name(i)))
            .collect::<Vec<_>>()
            .join(",")
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut out = Self::default();
        for term in text.split(',').filter(|s| !s.trim().is_empty()) {
            let (name, value) = term
                .trim()
                .split_once('=')
                .ok_or("expected Anny label=value pairs")?;
            let i = (0..DETAIL_COUNT)
                .find(|&i| Self::name(i) == name)
                .ok_or_else(|| format!("unknown Anny control: {name}"))?;
            let value: f32 = value
                .parse()
                .map_err(|_| "expected finite Anny control value")?;
            if !value.is_finite() {
                return Err("expected finite Anny control value".into());
            }
            out.0[i] = value;
        }
        Ok(out.clamped())
    }
    pub fn is_neutral(self) -> bool {
        self.clamped() == Self::default()
    }
}

pub fn fingerprint(params: BodyShapeParams, details: Details) -> u64 {
    let p = params.clamped();
    let bytes: Vec<_> = [p.muscle, p.weight, p.height, p.proportions, p.age]
        .into_iter()
        .chain(details.clamped().0)
        .flat_map(f32::to_le_bytes)
        .collect();
    super::model_fingerprint(&bytes)
}

#[derive(Deserialize)]
struct Bone {
    name: String,
    target: String,
    parent: String,
    head: Vec<usize>,
    tail: Vec<usize>,
}
#[derive(Deserialize)]
struct Sparse {
    indices: [usize; 2],
    delta: [usize; 2],
}
#[derive(Deserialize)]
struct Target {
    terms: Vec<(String, usize)>,
    #[serde(flatten)]
    sparse: Sparse,
}
#[derive(Deserialize)]
struct Local {
    label: String,
    sides: [Sparse; 2],
}
#[derive(Deserialize)]
struct Facial {
    label: String,
    #[serde(flatten)]
    sparse: Sparse,
}
#[derive(Deserialize)]
struct Metadata {
    version: u32,
    vertices: [usize; 2],
    faces: [usize; 2],
    blocks: Vec<[usize; 2]>,
    bones: Vec<Bone>,
    weights: Vec<Vec<(usize, f32)>>,
    targets: Vec<Target>,
    controls: Vec<Local>,
    facial: Vec<Facial>,
    #[cfg(test)]
    oracle: Vec<Oracle>,
}
#[cfg(test)]
#[derive(Deserialize)]
struct Oracle {
    params: [f32; 5],
    details: String,
    expected: [usize; 2],
}
struct Model {
    meta: Metadata,
    bytes: Vec<u8>,
    template: Vec<[f32; 3]>,
    faces: Vec<[usize; 3]>,
    neutral: Vec<[f32; 3]>,
    heads: Vec<[f32; 3]>,
    tails: Vec<[f32; 3]>,
}

#[cfg(test)]
mod tests;
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| a[k] + b[k])
}
fn mul(a: [f32; 3], b: f32) -> [f32; 3] {
    a.map(|x| x * b)
}
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn mean(points: &[[f32; 3]], ids: &[usize]) -> [f32; 3] {
    ids.iter().fold([0.; 3], |v, &i| {
        add(v, mul(points[i], 1. / ids.len() as f32))
    })
}
fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| {
        let mut zip = zip::ZipArchive::new(Cursor::new(include_bytes!("anny_targets.zip")))
            .expect("bundled Anny archive");
        let meta: Metadata = serde_json::from_reader(zip.by_name("model.json").unwrap())
            .expect("bundled Anny metadata");
        assert_eq!(meta.version, 1);
        assert_eq!(meta.controls.len(), LOCAL_COUNT);
        assert_eq!(meta.facial.len(), FACIAL_COUNT);
        for (i, c) in meta.controls.iter().enumerate() {
            assert_eq!(c.label, LOCAL_CONTROLS[i].0);
        }
        for (i, c) in meta.facial.iter().enumerate() {
            assert_eq!(c.label, FACIAL_CONTROLS[i]);
        }
        let mut bytes = Vec::new();
        zip.by_name("arrays.bin")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        for &[offset, count] in &meta.blocks {
            let shuffled = bytes[offset..offset + count * 4].to_vec();
            for i in 0..count {
                for k in 0..4 {
                    bytes[offset + i * 4 + k] = shuffled[k * count + i];
                }
            }
        }
        let floats: Vec<_> = bytes[meta.vertices[0]..meta.vertices[0] + meta.vertices[1] * 4]
            .chunks_exact(4)
            .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
            .collect();
        let template = floats.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect();
        let ids: Vec<_> = bytes[meta.faces[0]..meta.faces[0] + meta.faces[1] * 4]
            .chunks_exact(4)
            .map(|v| u32::from_le_bytes(v.try_into().unwrap()) as usize)
            .collect();
        let faces = ids.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect();
        let mut out = Model {
            meta,
            bytes,
            template,
            faces,
            neutral: vec![],
            heads: vec![],
            tails: vec![],
        };
        out.neutral = out.evaluate(BodyShapeParams::default(), Details::default());
        out.heads = out
            .meta
            .bones
            .iter()
            .map(|b| mean(&out.neutral, &b.head))
            .collect();
        out.tails = out
            .meta
            .bones
            .iter()
            .map(|b| mean(&out.neutral, &b.tail))
            .collect();
        out
    })
}
impl Model {
    fn accumulate(&self, out: &mut [[f32; 3]], sparse: &Sparse, weight: f32) {
        if weight == 0. {
            return;
        }
        for i in 0..sparse.indices[1] {
            let at = sparse.indices[0] + i * 4;
            let vertex = u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap()) as usize;
            for k in 0..3 {
                let at = sparse.delta[0] + (i * 3 + k) * 4;
                out[vertex][k] +=
                    weight * f32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap());
            }
        }
    }
    fn evaluate(&self, params: BodyShapeParams, details: Details) -> Vec<[f32; 3]> {
        let p = params.clamped();
        let d = details.clamped();
        let mut out = self.template.clone();
        // phenotype.py: piecewise linear anchor interpolation; race weights normalize.
        let race_sum = d.0[3..6].iter().sum::<f32>();
        for t in &self.meta.targets {
            let mut coefficient = 1.;
            for (axis, anchor) in &t.terms {
                if axis == "race" {
                    coefficient *= if race_sum > 0. {
                        d.0[3 + anchor] / race_sum
                    } else {
                        1. / 3.
                    };
                    continue;
                }
                let (value, count, min) = match axis.as_str() {
                    "gender" => (d.0[0], 2, 0.),
                    "cupsize" => (d.0[1], 3, 0.),
                    "firmness" => (d.0[2], 3, 0.),
                    "age" => (p.age, 5, -1. / 3.),
                    "muscle" => (p.muscle, 3, 0.),
                    "weight" => (p.weight, 3, 0.),
                    "height" => (p.height, 2, 0.),
                    "proportions" => (p.proportions, 2, 0.),
                    _ => unreachable!("authored phenotype axis"),
                };
                let x = ((value - min) / (1. - min)) * (count - 1) as f32;
                coefficient *= (1. - (x - *anchor as f32).abs()).max(0.);
            }
            self.accumulate(&mut out, &t.sparse, coefficient);
        }
        // phenotype.py: positive = relu(value), negative = relu(-value).
        for (i, local) in self.meta.controls.iter().enumerate() {
            let value = d.0[6 + i];
            self.accumulate(&mut out, &local.sides[usize::from(value < 0.)], value.abs());
        }
        for (i, facial) in self.meta.facial.iter().enumerate() {
            self.accumulate(&mut out, &facial.sparse, d.0[6 + LOCAL_COUNT + i]);
        }
        out
    }
}
