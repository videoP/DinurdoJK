//! Rust port of Anny's phenotype.py and barycentric retopology blendshape sampling.
//! Authored targets are exported unchanged from full_model.py by export_runtime.py.
//! Copyright (C) 2025 NAVER Corp. Anny portions: Apache-2.0 (license in archive).
use super::{cross, dot, normalize, sub, BodyShapeParams};
use crate::ghoul2::{GlaAnimation, GlmModel};
use serde::Deserialize;
use std::{
    collections::HashMap,
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
    let pivots = super::procedural::apply(glm, gla, params, detail_pivots.as_deref().or(native))?;
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
    fixed_params: [f32; 5],
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
/// Asset-pack paths of the lossless runtime deformation data.
pub const RUNTIME_METADATA: &str = "customization/body/model.json";
pub const RUNTIME_ARRAYS: &str = "customization/body/arrays.bin";
static MODEL: OnceLock<Model> = OnceLock::new();
static ASSETS: Mutex<Option<crate::pk3::AssetSearchPath>> = Mutex::new(None);

/// Supply the presenter's VFS without decoding the donor until first use.
/// The immutable donor and its derived mapping caches are shared by presenters.
pub fn configure_assets(assets: &crate::pk3::AssetSearchPath) {
    if MODEL.get().is_none() {
        if let Ok(mut source) = ASSETS.lock() {
            if source.is_none() {
                *source = Some(assets.fork());
            }
        }
    }
}

fn model() -> Result<&'static Model, String> {
    if let Some(data) = MODEL.get() {
        return Ok(data);
    }
    let mut source = ASSETS.lock().map_err(|_| "Anny asset reader poisoned")?;
    if let Some(data) = MODEL.get() {
        return Ok(data);
    }
    #[cfg(test)]
    if source.is_none() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        *source = Some(crate::pk3::AssetSearchPath::open(&root).map_err(|e| e.to_string())?);
    }
    let assets = source
        .as_mut()
        .ok_or("Anny asset search path is not configured")?;
    let mut read = |name, limit| {
        assets
            .read(name, limit)
            .map_err(|e| format!("Anny asset: {e}"))?
            .map(|asset| asset.bytes)
            .ok_or_else(|| format!("Missing {name}; install dinurdo-assets.pk3 in base"))
    };
    let json = read(RUNTIME_METADATA, 2 * 1024 * 1024)?;
    let bytes = read(RUNTIME_ARRAYS, 32 * 1024 * 1024)?;
    let data = Model::parse(&json, bytes)?;
    let _ = MODEL.set(data);
    *source = None;
    MODEL
        .get()
        .ok_or_else(|| "Anny initialization failed".into())
}

impl Model {
    fn parse(json: &[u8], mut bytes: Vec<u8>) -> Result<Self, String> {
        if json.len() > 2 * 1024 * 1024 {
            return Err("Anny metadata exceeds limit".into());
        }
        let meta: Metadata = serde_json::from_slice(json).map_err(|e| e.to_string())?;
        if meta.version != 2
            || meta.fixed_params != [0.5; 5]
            || meta.controls.len() != LOCAL_COUNT
            || meta.facial.len() != FACIAL_COUNT
            || meta
                .controls
                .iter()
                .zip(LOCAL_CONTROLS)
                .any(|(c, expected)| c.label != expected.0)
            || meta
                .facial
                .iter()
                .zip(FACIAL_CONTROLS)
                .any(|(c, expected)| c.label != expected)
        {
            return Err("Unsupported Anny runtime data/catalog".into());
        }
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("Anny arrays exceed limit".into());
        }
        let valid = |[offset, count]: [usize; 2]| {
            offset
                .checked_add(count.saturating_mul(4))
                .is_some_and(|end| end <= bytes.len())
        };
        if meta.blocks.iter().any(|&spec| !valid(spec))
            || !valid(meta.vertices)
            || !valid(meta.faces)
            || meta.vertices[1] == 0
            || meta.vertices[1] % 3 != 0
            || meta.faces[1] % 3 != 0
        {
            return Err("Invalid Anny array bounds".into());
        }
        for sparse in meta
            .targets
            .iter()
            .map(|t| &t.sparse)
            .chain(meta.controls.iter().flat_map(|c| c.sides.iter()))
            .chain(meta.facial.iter().map(|f| &f.sparse))
        {
            if !valid(sparse.indices)
                || !valid(sparse.delta)
                || sparse.indices[1].checked_mul(3) != Some(sparse.delta[1])
            {
                return Err("Invalid Anny target bounds".into());
            }
        }
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
        out.validate()?;
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
        Ok(out)
    }
}
impl Model {
    fn validate(&self) -> Result<(), String> {
        let n = self.template.len();
        let bones = self.meta.bones.len();
        if bones == 0
            || bones > 256
            || self.meta.weights.len() != n
            || self.template.iter().flatten().any(|x| !x.is_finite())
            || self.faces.iter().flatten().any(|&i| i >= n)
            || self.meta.bones.iter().any(|b| {
                b.head.is_empty()
                    || b.tail.is_empty()
                    || b.head.iter().chain(&b.tail).any(|&i| i >= n)
            })
            || self
                .meta
                .weights
                .iter()
                .flatten()
                .any(|&(i, w)| i >= bones || !w.is_finite())
        {
            return Err("Invalid Anny reference geometry/rig".into());
        }
        for t in &self.meta.targets {
            for (axis, anchor) in &t.terms {
                let count = match axis.as_str() {
                    "race" | "cupsize" | "firmness" | "muscle" | "weight" => 3,
                    "gender" | "height" | "proportions" => 2,
                    "age" => 5,
                    _ => return Err("Invalid Anny target axis".into()),
                };
                if *anchor >= count {
                    return Err("Invalid Anny target anchor".into());
                }
            }
        }
        for s in self
            .meta
            .targets
            .iter()
            .map(|t| &t.sparse)
            .chain(self.meta.controls.iter().flat_map(|c| c.sides.iter()))
            .chain(self.meta.facial.iter().map(|f| &f.sparse))
        {
            for v in self.bytes[s.indices[0]..s.indices[0] + s.indices[1] * 4].chunks_exact(4) {
                if u32::from_le_bytes(v.try_into().unwrap()) as usize >= n {
                    return Err("Invalid Anny target vertex".into());
                }
            }
            for v in self.bytes[s.delta[0]..s.delta[0] + s.delta[1] * 4].chunks_exact(4) {
                if !f32::from_le_bytes(v.try_into().unwrap()).is_finite() {
                    return Err("Invalid Anny target displacement".into());
                }
            }
        }
        Ok(())
    }
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
