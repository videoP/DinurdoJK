//! Offline-authored Anny deformation grids for an exact native GLM revision.
//!
//! Shape interpolation happens during asset registration. Mesh topology, UVs,
//! skin weights, animation definitions and gameplay bounds are retained.
use crate::ghoul2::{GlaAnimation, GlmModel, Matrix3x4};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyShapeParams {
    pub muscle: f32,
    pub weight: f32,
    pub proportions: f32,
    /// Normalized within the age range recorded in the exported bundle.
    pub age: f32,
}

impl Default for BodyShapeParams {
    fn default() -> Self {
        Self {
            muscle: 0.5,
            weight: 0.5,
            proportions: 0.5,
            age: 0.5,
        }
    }
}

impl BodyShapeParams {
    pub fn clamped(self) -> Self {
        let defaults = Self::default().values();
        let values = std::array::from_fn::<_, 4, _>(|i| {
            let value = self.values()[i];
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                defaults[i]
            }
        });
        Self {
            muscle: values[0],
            weight: values[1],
            proportions: values[2],
            age: values[3],
        }
    }

    pub fn is_neutral(self) -> bool {
        self.clamped() == Self::default()
    }

    fn values(self) -> [f32; 4] {
        [self.muscle, self.weight, self.proportions, self.age]
    }
}

/// FNV-1a over the original model bytes; rejects data from another GLM revision.
pub fn model_fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

pub fn binding_fingerprint(glm: &GlmModel) -> u64 {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(glm.num_bones as u32).to_le_bytes());
    bytes.extend_from_slice(&(glm.lods.len() as u32).to_le_bytes());
    for lod in &glm.lods {
        bytes.extend_from_slice(&(lod.surfaces.len() as u32).to_le_bytes());
        for surface in &lod.surfaces {
            for n in [
                surface.surface_index,
                surface.vertices.len(),
                surface.triangles.len(),
            ] {
                bytes.extend_from_slice(&(n as u32).to_le_bytes());
            }
            for v in &surface.vertices {
                for x in v.position.into_iter().chain(v.normal) {
                    bytes.extend_from_slice(&x.to_le_bytes());
                }
            }
            for face in &surface.triangles {
                for i in face {
                    bytes.extend_from_slice(&i.to_le_bytes());
                }
            }
        }
    }
    model_fingerprint(&bytes)
}

pub struct BodyShapeGrid {
    knots: [Vec<f32>; 4],
    bones: usize,
    points_per_sample: usize,
    samples: Vec<[f32; 3]>,
    pub adult_only: bool,
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let end = self
            .offset
            .checked_add(N)
            .ok_or("body-shape offset overflow")?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or("truncated body-shape data")?;
        self.offset = end;
        Ok(slice.try_into().expect("fixed-size slice"))
    }
    fn count(&mut self, max: usize) -> Result<usize, String> {
        let value = u32::from_le_bytes(self.take()?) as usize;
        if value > max {
            return Err("body-shape count exceeds limit".into());
        }
        Ok(value)
    }
    fn float(&mut self) -> Result<f32, String> {
        let value = f32::from_le_bytes(self.take()?);
        if !value.is_finite() {
            return Err("non-finite body-shape value".into());
        }
        Ok(value)
    }
}

impl BodyShapeGrid {
    pub fn parse(bytes: &[u8], glm: &GlmModel, fingerprint: u64) -> Result<Self, String> {
        let mut r = Reader { bytes, offset: 0 };
        if &r.take::<8>()? != b"JKBODY2\0" {
            return Err("unsupported body-shape format".into());
        }
        if u64::from_le_bytes(r.take()?) != fingerprint {
            return Err("body-shape GLM fingerprint mismatch".into());
        }
        if u64::from_le_bytes(r.take()?) != binding_fingerprint(glm) {
            return Err("body-shape bind geometry mismatch".into());
        }
        let bones = r.count(4096)?;
        let lods = r.count(32)?;
        let sample_count = r.count(512)?;
        let flags = r.count(1)?;
        if bones != glm.num_bones || lods != glm.lods.len() {
            return Err("body-shape rig/LOD mismatch".into());
        }
        let mut knots = std::array::from_fn(|_| Vec::new());
        let mut expected_samples = 1usize;
        for axis in &mut knots {
            let n = r.count(16)?;
            if n < 2 {
                return Err("body-shape axis requires at least two anchors".into());
            }
            *axis = (0..n).map(|_| r.float()).collect::<Result<_, _>>()?;
            if axis[0] != 0.0 || axis[n - 1] != 1.0 || axis.windows(2).any(|w| w[0] >= w[1]) {
                return Err("body-shape anchors must increase from 0 to 1".into());
            }
            expected_samples = expected_samples
                .checked_mul(n)
                .ok_or("body-shape grid overflow")?;
        }
        if sample_count != expected_samples {
            return Err("body-shape sample count mismatch".into());
        }
        let mut points_per_sample = bones;
        for lod in &glm.lods {
            if r.count(65536)? != lod.surfaces.len() {
                return Err("body-shape surface count mismatch".into());
            }
            for surface in &lod.surfaces {
                let index = r.count(65536)?;
                let vertices = r.count(1_000_000)?;
                if index != surface.surface_index || vertices != surface.vertices.len() {
                    return Err("body-shape surface layout mismatch".into());
                }
                points_per_sample = points_per_sample
                    .checked_add(vertices)
                    .ok_or("body-shape vertex count overflow")?;
            }
        }
        let total = points_per_sample
            .checked_mul(sample_count)
            .ok_or("body-shape data size overflow")?;
        let remaining = total
            .checked_mul(12)
            .ok_or("body-shape byte size overflow")?;
        if bytes.len().checked_sub(r.offset) != Some(remaining) {
            return Err("body-shape payload size mismatch".into());
        }
        let samples = (0..total)
            .map(|_| Ok([r.float()?, r.float()?, r.float()?]))
            .collect::<Result<_, String>>()?;
        Ok(Self {
            knots,
            bones,
            points_per_sample,
            samples,
            adult_only: flags != 0,
        })
    }

    fn corners(&self, params: BodyShapeParams) -> Vec<(usize, f32)> {
        let mut corners = vec![(0, 1.0)];
        for (knots, value) in self.knots.iter().zip(params.clamped().values()) {
            let hi = knots
                .partition_point(|&k| k < value)
                .clamp(1, knots.len() - 1);
            let alpha = (value - knots[hi - 1]) / (knots[hi] - knots[hi - 1]);
            let mut next = Vec::with_capacity(corners.len() * 2);
            for (index, weight) in corners {
                if alpha < 1.0 {
                    next.push((index * knots.len() + hi - 1, weight * (1.0 - alpha)));
                }
                if alpha > 0.0 {
                    next.push((index * knots.len() + hi, weight * alpha));
                }
            }
            corners = next;
        }
        corners
    }

    /// Returns the new joint pivots for the existing Ghoul2 pose rebasing path.
    /// Neutral is an exact no-op, including authored vertex normals.
    pub fn apply(
        &self,
        glm: &mut GlmModel,
        gla: &GlaAnimation,
        params: BodyShapeParams,
        native_pivots: Option<&[[f32; 3]]>,
    ) -> Result<Option<Vec<[f32; 3]>>, String> {
        if params.is_neutral() {
            return Ok(None);
        }
        if gla.skeleton.len() != self.bones {
            return Err("body-shape bind skeleton mismatch".into());
        }
        if native_pivots.is_some_and(|p| p.len() != self.bones) {
            return Err("body-shape native pivot mismatch".into());
        }
        let corners = self.corners(params);
        let neutral = self.corners(BodyShapeParams::default());
        let mut delta = vec![[0.0; 3]; self.points_per_sample];
        // Subtract the interpolated neutral, so float32 export/rounding never
        // introduces identity drift or seams around an otherwise neutral field.
        for (sign, samples) in [(1.0, corners), (-1.0, neutral)] {
            for (sample, weight) in samples {
                let offset = sample * self.points_per_sample;
                for (i, out) in delta.iter_mut().enumerate() {
                    for k in 0..3 {
                        out[k] += sign * weight * self.samples[offset + i][k];
                    }
                }
            }
        }
        let pivots = gla
            .skeleton
            .iter()
            .zip(&delta)
            .enumerate()
            .map(|(i, (bone, d))| {
                std::array::from_fn(|k| {
                    native_pivots.map_or(bone.base_pose[k][3], |p| p[i][k]) + d[k]
                })
            })
            .collect();
        let mut offset = self.bones;
        for lod in &mut glm.lods {
            let mut welded = HashMap::<[u32; 3], usize>::new();
            let mut groups = Vec::new();
            let mut before = Vec::<[f32; 3]>::new();
            let mut after = Vec::<[f32; 3]>::new();
            let mut bind_positions = Vec::new();
            for surface in &mut lod.surfaces {
                let mut ids = Vec::with_capacity(surface.vertices.len());
                for v in &mut surface.vertices {
                    // Exact position weld shares a field across material seams.
                    let key = v.position.map(|x| if x == 0.0 { 0 } else { x.to_bits() });
                    let next = welded.len();
                    let group = *welded.entry(key).or_insert(next);
                    if group == before.len() {
                        before.push([0.0; 3]);
                        after.push([0.0; 3]);
                    }
                    ids.push(group);
                    bind_positions.push(v.position);
                    for k in 0..3 {
                        v.position[k] += delta[offset][k];
                    }
                    offset += 1;
                }
                groups.push(ids);
            }
            let mut base = 0;
            for (surface, ids) in lod.surfaces.iter().zip(&groups) {
                for face in &surface.triangles {
                    let [a, b, c] = face.map(|v| v as usize);
                    if [a, b, c].iter().any(|&v| v >= ids.len()) {
                        return Err("invalid body-shape triangle".into());
                    }
                    let old = cross(
                        sub(bind_positions[base + b], bind_positions[base + a]),
                        sub(bind_positions[base + c], bind_positions[base + a]),
                    );
                    let new = cross(
                        sub(surface.vertices[b].position, surface.vertices[a].position),
                        sub(surface.vertices[c].position, surface.vertices[a].position),
                    );
                    for v in [a, b, c] {
                        for k in 0..3 {
                            before[ids[v]][k] += old[k];
                            after[ids[v]][k] += new[k];
                        }
                    }
                }
                base += surface.vertices.len();
            }
            for (surface, ids) in lod.surfaces.iter_mut().zip(groups) {
                for (v, id) in surface.vertices.iter_mut().zip(ids) {
                    let old = normalize(before[id]);
                    let new = normalize(after[id]);
                    // GLM winding can oppose authored normals. Preserve their
                    // orientation/detail rather than replacing them wholesale.
                    let sign = if dot(old, v.normal) < 0.0 { -1.0 } else { 1.0 };
                    v.normal = normalize(std::array::from_fn(|k| {
                        v.normal[k] + sign * (new[k] - old[k])
                    }));
                }
            }
        }
        Ok(Some(pivots))
    }
}

/// Retarget animated joint links to the shaped rest proportions. Source's
/// baseline rebind is separate: carrying its unscaled translation residuals
/// into a child rig produces elongated necks and limbs.
pub struct BodyShapeBinding {
    original: Vec<[f32; 3]>,
    shaped: Vec<[f32; 3]>,
    parents: Vec<Option<usize>>,
    source_baseline: bool,
}

impl BodyShapeBinding {
    pub fn new(
        gla: &GlaAnimation,
        original: Option<&[[f32; 3]]>,
        shaped: &[[f32; 3]],
    ) -> Result<Self, String> {
        let n = gla.skeleton.len();
        if shaped.len() != n || original.is_some_and(|p| p.len() != n) {
            return Err("body-shape binding / skeleton count mismatch".into());
        }
        let rest = original.map_or_else(
            || {
                gla.skeleton
                    .iter()
                    .map(|b| std::array::from_fn(|k| b.base_pose[k][3]))
                    .collect()
            },
            |p| p.to_vec(),
        );
        Ok(Self {
            original: rest,
            shaped: shaped.to_vec(),
            source_baseline: original.is_some(),
            parents: (0..n)
                .map(|i| crate::skeleton::anatomical_parent(&gla.skeleton, i))
                .collect(),
        })
    }

    pub fn retarget(
        &self,
        gla: &GlaAnimation,
        stock: &[Matrix3x4],
    ) -> Result<Vec<Matrix3x4>, String> {
        let baseline;
        let stock = if self.source_baseline {
            baseline = crate::source1_rig::rebase_source_ghoul2_pose(gla, stock, &self.original)?;
            baseline.as_slice()
        } else {
            stock
        };
        let n = self.original.len();
        if stock.len() != n {
            return Err("body-shape pose count mismatch".into());
        }
        // Tissue controls do not alter joints. Keep the original matrices exact.
        if self.original == self.shaped {
            return Ok(stock.to_vec());
        }
        let old_joints: Vec<_> = stock
            .iter()
            .zip(&self.original)
            .map(|(m, p)| transform(m, *p))
            .collect();
        let mut joints = vec![[0.; 3]; n];
        let mut output = stock.to_vec();
        let mut state = vec![0u8; n];
        fn visit(
            j: usize,
            binding: &BodyShapeBinding,
            stock: &[Matrix3x4],
            old: &[[f32; 3]],
            joints: &mut [[f32; 3]],
            output: &mut [Matrix3x4],
            state: &mut [u8],
        ) -> Result<(), String> {
            if state[j] == 2 {
                return Ok(());
            }
            if state[j] == 1 {
                return Err("cyclic body-shape ancestry".into());
            }
            state[j] = 1;
            if let Some(p) = binding.parents[j] {
                if p >= stock.len() {
                    return Err("invalid body-shape parent".into());
                }
                visit(p, binding, stock, old, joints, output, state)?;
                let link = sub(old[j], old[p]);
                // Bring the complete animated link (including animation
                // translation) into its parent's neutral model coordinates.
                let local = inverse_rotate(&stock[p], link)?;
                let before = sub(binding.original[j], binding.original[p]);
                let after = sub(binding.shaped[j], binding.shaped[p]);
                let mapped = if dot(before, before) < 1e-12 {
                    // Face/helper bones can share a pivot while still carrying
                    // animated translation. Inherit the nearest physical
                    // segment's proportions instead of keeping adult offsets.
                    let mut ancestor = p;
                    let mut scaled = local;
                    for _ in 0..binding.parents.len() {
                        let Some(parent) = binding.parents[ancestor] else {
                            break;
                        };
                        let a = sub(binding.original[ancestor], binding.original[parent]);
                        let b = sub(binding.shaped[ancestor], binding.shaped[parent]);
                        if dot(a, a) >= 1e-12 {
                            scaled = map_link(local, a, b);
                            break;
                        }
                        ancestor = parent;
                    }
                    std::array::from_fn(|k| scaled[k] + after[k])
                } else {
                    map_link(local, before, after)
                };
                let rotated = rotate(&stock[p], mapped);
                joints[j] = std::array::from_fn(|k| joints[p][k] + rotated[k]);
            } else {
                joints[j] = transform(&stock[j], binding.shaped[j]);
            }
            let rotated = rotate(&stock[j], binding.shaped[j]);
            for k in 0..3 {
                output[j][k][3] = joints[j][k] - rotated[k];
            }
            state[j] = 2;
            Ok(())
        }
        for j in 0..n {
            visit(
                j,
                self,
                stock,
                &old_joints,
                &mut joints,
                &mut output,
                &mut state,
            )?;
        }
        Ok(output)
    }
}

fn rotate(m: &Matrix3x4, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| (0..3).map(|i| m[k][i] * p[i]).sum())
}
fn transform(m: &Matrix3x4, p: [f32; 3]) -> [f32; 3] {
    let r = rotate(m, p);
    std::array::from_fn(|k| r[k] + m[k][3])
}
fn inverse_rotate(m: &Matrix3x4, p: [f32; 3]) -> Result<[f32; 3], String> {
    // Quantized GLA rotations are not perfectly orthonormal. Transposition
    // introduces measurable link-length drift even on an unchanged skeleton.
    let columns: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|k| m[k][i]));
    let rows = [
        cross(columns[1], columns[2]),
        cross(columns[2], columns[0]),
        cross(columns[0], columns[1]),
    ];
    let det = dot(columns[0], rows[0]);
    if det.abs() < 1e-8 {
        return Err("singular body-shape parent pose".into());
    }
    Ok(rows.map(|row| dot(row, p) / det))
}
fn map_link(link: [f32; 3], before: [f32; 3], after: [f32; 3]) -> [f32; 3] {
    if before == after {
        return link;
    }
    let old_length = dot(before, before).sqrt();
    let new_length = dot(after, after).sqrt();
    if old_length < 1e-6 {
        return std::array::from_fn(|k| link[k] + after[k] - before[k]);
    }
    if new_length < 1e-6 {
        return [0.; 3];
    }
    let a = before.map(|x| x / old_length);
    let b = after.map(|x| x / new_length);
    let c = dot(a, b).clamp(-1., 1.);
    let axis = cross(a, b);
    let turned = if c < -0.99999 {
        let reference = if a[0].abs() < 0.8 {
            [1., 0., 0.]
        } else {
            [0., 1., 0.]
        };
        let perpendicular = normalize(cross(a, reference));
        std::array::from_fn(|k| 2. * perpendicular[k] * dot(perpendicular, link) - link[k])
    } else {
        let first = cross(axis, link);
        let second = cross(axis, first);
        std::array::from_fn(|k| link[k] + first[k] + second[k] / (1. + c))
    };
    turned.map(|x| x * new_length / old_length)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| a[k] - b[k])
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| a[k] * b[k]).sum()
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length > 1e-12 {
        v.map(|x| x / length)
    } else {
        [0.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonfinite_controls_reset_and_fingerprint_is_stable() {
        let p = BodyShapeParams {
            muscle: f32::NAN,
            weight: f32::INFINITY,
            proportions: -2.0,
            age: 4.0,
        }
        .clamped();
        assert_eq!(p.values(), [0.5, 0.5, 0.0, 1.0]);
        assert_eq!(model_fingerprint(b"hello"), 0xa430d84680aabd0b);
    }

    #[test]
    fn interpolation_keeps_cross_axis_interactions() {
        let grid = BodyShapeGrid {
            knots: std::array::from_fn(|_| vec![0.0, 1.0]),
            bones: 0,
            points_per_sample: 0,
            samples: vec![],
            adult_only: true,
        };
        let corners = grid.corners(BodyShapeParams {
            muscle: 0.3,
            weight: 0.8,
            proportions: 0.4,
            age: 0.2,
        });
        assert!((corners.iter().map(|x| x.1).sum::<f32>() - 1.0).abs() < 1e-6);
        // Sample values for a multilinear function include a muscle*weight term.
        let result: f32 = corners
            .iter()
            .map(|&(i, w)| w * (((i >> 3) & 1) * ((i >> 2) & 1)) as f32)
            .sum();
        assert!((result - 0.24).abs() < 1e-6);
    }

    #[test]
    fn malformed_data_is_rejected_before_allocation() {
        let glm = GlmModel {
            name: String::new(),
            anim_name: String::new(),
            num_bones: 0,
            hierarchy: vec![],
            lods: vec![],
        };
        for bytes in [vec![], b"JKBODY1\0".to_vec(), vec![255; 128]] {
            assert!(BodyShapeGrid::parse(&bytes, &glm, 0).is_err());
        }
    }

    #[test]
    fn proportion_binding_scales_animation_translation_and_quantized_rotations() {
        let gla = crate::skeleton::humanoid_reference(crate::skeleton::HumanoidRig::Jka53);
        let original: Vec<[f32; 3]> = gla
            .skeleton
            .iter()
            .map(|b| std::array::from_fn(|k| b.base_pose[k][3]))
            .collect();
        let shaped: Vec<_> = original.iter().map(|p| p.map(|x| x * 0.7)).collect();
        let mut pose =
            vec![[[1.0004, 0., 0., 0.], [0., 0.9997, 0., 0.], [0., 0., 1., 0.]]; original.len()];
        // Animated link translations differ from the rest links. Keeping
        // adult translation residuals used to elongate children's necks.
        for (i, m) in pose.iter_mut().enumerate() {
            for k in 0..3 {
                m[k][3] = original[i][k] * 0.13 + (i % 3) as f32 * 0.1;
            }
        }
        let binding = BodyShapeBinding::new(&gla, None, &shaped).unwrap();
        let output = binding.retarget(&gla, &pose).unwrap();
        for j in 0..original.len() {
            if let Some(p) = crate::skeleton::anatomical_parent(&gla.skeleton, j) {
                let old = sub(
                    transform(&pose[j], original[j]),
                    transform(&pose[p], original[p]),
                );
                let new = sub(
                    transform(&output[j], shaped[j]),
                    transform(&output[p], shaped[p]),
                );
                assert!(
                    (dot(new, new).sqrt() - 0.7 * dot(old, old).sqrt()).abs() < 2e-5,
                    "bone {j}"
                );
            }
        }
        let unchanged = BodyShapeBinding::new(&gla, None, &original).unwrap();
        assert_eq!(unchanged.retarget(&gla, &pose).unwrap(), pose);
    }

    /// Generate fixtures with tools/jiggle-shape-preview/anny/validate_profile.py.
    /// Uses installed retail assets; deliberately excluded from portable CI.
    #[test]
    #[ignore = "requires local retail PK3s and generated Anny shape/oracle fixtures"]
    fn prepared_shapes_match_recipe_and_animate_native_ghoul2() {
        use crate::ghoul2::{
            parse_gla, parse_glm_native, skin_glm_surface, Ghoul2Animator,
            BONE_ANIM_OVERRIDE_FREEZE,
        };
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = root.join("tools/jiggle-shape-preview/anny/output");
        let mut assets =
            crate::pk3::AssetSearchPath::open_game(&root.join("target/release/base"), None)
                .unwrap();
        for model in ["jedi_hf", "jedi_hm", "jedi_zf"] {
            let raw = assets
                .read(
                    &format!("models/players/{model}/model.glm"),
                    32 * 1024 * 1024,
                )
                .unwrap()
                .unwrap()
                .bytes;
            let original = parse_glm_native(&raw).unwrap();
            let anim = assets
                .read(&format!("{}.gla", original.anim_name), 128 * 1024 * 1024)
                .unwrap()
                .unwrap()
                .bytes;
            let gla = parse_gla(&anim).unwrap();
            let bundle = std::fs::read(output.join(format!("{model}.bodyshape"))).unwrap();
            let grid = BodyShapeGrid::parse(&bundle, &original, model_fingerprint(&raw)).unwrap();
            assert!(BodyShapeGrid::parse(&bundle, &original, 0).is_err());
            let oracle = std::fs::read(output.join(format!("{model}_oracle.bin"))).unwrap();
            let mut r = Reader {
                bytes: &oracle,
                offset: 0,
            };
            assert_eq!(&r.take::<8>().unwrap(), b"JKORACLE");
            let cases = r.count(100).unwrap();
            assert_eq!(r.count(1_000_000).unwrap(), grid.points_per_sample);
            let mut worst_error = 0.0f32;
            for _ in 0..cases {
                let values: [f32; 4] = std::array::from_fn(|_| r.float().unwrap());
                let params = BodyShapeParams {
                    muscle: values[0],
                    weight: values[1],
                    proportions: values[2],
                    age: values[3],
                };
                let expected: Vec<[f32; 3]> = (0..grid.points_per_sample)
                    .map(|_| std::array::from_fn(|_| r.float().unwrap()))
                    .collect();
                let mut shaped = original.clone();
                let pivots = grid.apply(&mut shaped, &gla, params, None).unwrap();
                if params.is_neutral() {
                    assert!(pivots.is_none());
                }
                if let Some(pivots) = &pivots {
                    for (i, p) in pivots.iter().enumerate() {
                        for k in 0..3 {
                            worst_error = worst_error.max(
                                (p[k] - gla.skeleton[i].base_pose[k][3] - expected[i][k]).abs(),
                            );
                        }
                    }
                }
                let mut offset = grid.bones;
                for (before, after) in original.lods.iter().zip(&shaped.lods) {
                    for (a, b) in before.surfaces.iter().zip(&after.surfaces) {
                        assert_eq!(a.triangles, b.triangles);
                        assert_eq!(a.texcoords, b.texcoords);
                        assert_eq!(a.bone_references, b.bone_references);
                        for (v, w) in a.vertices.iter().zip(&b.vertices) {
                            assert_eq!(v.weights, w.weights);
                            if params.is_neutral() {
                                assert_eq!(v.position, w.position);
                                assert_eq!(v.normal, w.normal);
                            }
                            for k in 0..3 {
                                worst_error = worst_error.max(
                                    (w.position[k] - v.position[k] - expected[offset][k]).abs(),
                                );
                            }
                            assert!(w.normal.iter().all(|x| x.is_finite()));
                            offset += 1;
                        }
                    }
                }
                for frame in [14651, 20447, 2947] {
                    let mut animator = Ghoul2Animator::new(&gla);
                    animator
                        .set_bone_anim(
                            &gla,
                            "model_root",
                            0,
                            (gla.num_frames - 1) as i32,
                            BONE_ANIM_OVERRIDE_FREEZE,
                            1.0,
                            0,
                            Some(frame as f32),
                            0,
                        )
                        .unwrap();
                    let mut pose = animator.evaluate_pose_openjk_root(&gla, 0).unwrap();
                    if let Some(pivots) = &pivots {
                        pose = BodyShapeBinding::new(&gla, None, pivots)
                            .unwrap()
                            .retarget(&gla, &pose)
                            .unwrap();
                    }
                    for lod in &shaped.lods {
                        for surface in &lod.surfaces {
                            let skinned = skin_glm_surface(surface, &pose).unwrap();
                            assert!(skinned.vertices.iter().all(|v| v
                                .position
                                .iter()
                                .chain(&v.normal)
                                .all(|x| x.is_finite())));
                        }
                    }
                }
            }
            assert!(
                worst_error < 2e-5,
                "{model}: recipe parity error {worst_error}"
            );
            eprintln!("{model}: {cases} recipes, all LODs, stand/walk/crouch frames; worst position/pivot error {worst_error}");
        }
    }

    #[test]
    #[ignore = "requires local Source Juliet assets and generated Anny fixtures"]
    fn prepared_source_shape_keeps_native_pivots_and_attachments() {
        use crate::{ghoul2::*, skeleton::*, source1::*, source1_geometry::*, source1_rig::*};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = root.join("tools/jiggle-shape-preview/anny/output");
        let mut assets =
            crate::pk3::AssetSearchPath::open_game(&root.join("target/release/base"), None)
                .unwrap();
        let mut read = |path: &str| assets.read(path, 128 * 1024 * 1024).unwrap().unwrap().bytes;
        let stem = "models/player/fwhykay/lollipop_chainsaw/juliet_starling_cine/juliet_starling_american_casual/juliet_starling_american_casual";
        let mdl = read(&format!("{stem}.mdl"));
        let vvd = parse_source_vvd(&read(&format!("{stem}.vvd"))).unwrap();
        let vtx = parse_source_vtx(&read(&format!("{stem}.dx90.vtx"))).unwrap();
        let skeleton = parse_source1_skeleton(&mdl).unwrap();
        let materials = parse_source1_materials(&mdl).unwrap();
        let gla = humanoid_reference(HumanoidRig::Jk2_72);
        let original = build_source_rig(
            stem,
            &mdl,
            &vvd,
            &vtx,
            &skeleton,
            &materials,
            &gla,
            &SourceRigOptions::default(),
        )
        .unwrap();
        let bundle = std::fs::read(output.join("temp.bodyshape")).unwrap();
        let grid = BodyShapeGrid::parse(&bundle, &original.glm, model_fingerprint(&mdl)).unwrap();
        let oracle = std::fs::read(output.join("temp_oracle.bin")).unwrap();
        let mut r = Reader {
            bytes: &oracle,
            offset: 8,
        };
        let cases = r.count(100).unwrap();
        assert_eq!(r.count(1_000_000).unwrap(), grid.points_per_sample);
        let donor = parse_glm_native(&read("models/players/kyle/model.glm")).unwrap();
        let donor_gla = humanoid_reference(HumanoidRig::Jka53);
        let animation = parse_gla(&read("models/players/_humanoid/_humanoid.gla")).unwrap();
        let retargeter = PoseRetargeter::new(&animation.skeleton, &gla.skeleton).unwrap();
        let mut worst = 0.0f32;
        for _ in 0..cases {
            let values: [f32; 4] = std::array::from_fn(|_| r.float().unwrap());
            let params = BodyShapeParams {
                muscle: values[0],
                weight: values[1],
                proportions: values[2],
                age: values[3],
            };
            let expected: Vec<[f32; 3]> = (0..grid.points_per_sample)
                .map(|_| std::array::from_fn(|_| r.float().unwrap()))
                .collect();
            let mut rig = build_source_rig(
                stem,
                &mdl,
                &vvd,
                &vtx,
                &skeleton,
                &materials,
                &gla,
                &SourceRigOptions::default(),
            )
            .unwrap();
            let baseline_pivots = rig.native_pivots.clone();
            if let Some(p) = grid
                .apply(&mut rig.glm, &gla, params, rig.native_pivots.as_deref())
                .unwrap()
            {
                rig.native_pivots = Some(p);
            }
            let original_pivots = original.native_pivots.as_ref().unwrap();
            for (i, point) in rig.native_pivots.as_ref().unwrap().iter().enumerate() {
                for k in 0..3 {
                    worst = worst.max((point[k] - original_pivots[i][k] - expected[i][k]).abs());
                }
            }
            let mut offset = grid.bones;
            for (a, b) in original.glm.lods.iter().zip(&rig.glm.lods) {
                for (a, b) in a.surfaces.iter().zip(&b.surfaces) {
                    for (v, w) in a.vertices.iter().zip(&b.vertices) {
                        assert_eq!(v.weights, w.weights);
                        for k in 0..3 {
                            worst = worst
                                .max((w.position[k] - v.position[k] - expected[offset][k]).abs());
                        }
                        offset += 1;
                    }
                }
            }
            assert!(
                attach_humanoid_bolts_between_rigs(&mut rig, &donor, &donor_gla, &gla).unwrap() > 0
            );
            for frame in [14651, 20447, 2947] {
                let mut animator = Ghoul2Animator::new(&animation);
                animator
                    .set_bone_anim(
                        &animation,
                        "model_root",
                        0,
                        (animation.num_frames - 1) as i32,
                        BONE_ANIM_OVERRIDE_FREEZE,
                        1.0,
                        0,
                        Some(frame as f32),
                        0,
                    )
                    .unwrap();
                let stock = animator.evaluate_pose_openjk_root(&animation, 0).unwrap();
                let pose = retargeter
                    .retarget(PoseInput::Ghoul2SkinMatrices(&stock))
                    .unwrap();
                let pose = BodyShapeBinding::new(
                    &gla,
                    baseline_pivots.as_deref(),
                    rig.native_pivots.as_ref().unwrap(),
                )
                .unwrap()
                .retarget(&gla, &pose)
                .unwrap();
                for surface in &rig.glm.lods[0].surfaces {
                    let skinned = skin_glm_surface(surface, &pose).unwrap();
                    assert!(skinned
                        .vertices
                        .iter()
                        .all(|v| v.position.iter().all(|x| x.is_finite())));
                }
                assert!(model_bolt_matrix(&rig.glm, &gla, &pose, "*r_hand")
                    .unwrap()
                    .is_some());
            }
        }
        assert!(worst < 2e-5, "Source shape parity error {worst}");
        eprintln!("Source Juliet: {cases} recipes, 72-bone pivots, retargeted gameplay frames and donor attachments; worst error {worst}");
    }
}
