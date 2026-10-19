//! Unreal player characters (for example a Fortnite body, head and hat) as
//! input for [`jka_assets::source1_rig::build_rig`], so they are rebound onto
//! the JKA humanoid exactly like Source models and play every JKA animation.
//!
//! Each part is an exported `*.skeletal.json` (CUE4Parse reference skeleton,
//! mesh-space bind vertices and per-section materials). In Unreal the parts
//! share the body's pose through a leader pose component, so they are merged
//! here into one skeleton by bone name; a part whose bind pose for a shared bone
//! differs from the body's is re-placed into the body's bind pose first.
use crate::transform::local;
use glam::{Mat4, Vec3};
use jka_assets::source1::{SourceBone, SourceSkeleton};
use jka_assets::source1_rig::{RigDraw, RigGeometry, RigSurfaceSpec, RigVertex};
use serde_json::Value;
use std::collections::HashMap;

/// One exported skeletal mesh and the shader for each of its material sections.
pub struct CharacterPart<'a> {
    pub name: &'a str,
    pub mesh: &'a [u8],
    pub shaders: &'a [String],
    /// A rigid accessory (a helmet with only a `root` bone) is attached to a
    /// socket instead of sharing bones by name.
    pub attach: Option<&'a Socket>,
}

/// A skeleton socket: bone plus relative UE location (cm) and rotator
/// (pitch, yaw, roll in degrees).
#[derive(Clone, Debug)]
pub struct Socket {
    pub bone: String,
    pub location: [f32; 3],
    pub rotation: [f32; 3],
}

impl Socket {
    /// UE `FRotator::Quaternion`, then the usual handedness reflection.
    fn local(&self) -> Result<Mat4, String> {
        let half = |d: f32| (d.to_radians() * 0.5).sin_cos();
        let ((sp, cp), (sy, cy), (sr, cr)) = (
            half(self.rotation[0]),
            half(self.rotation[1]),
            half(self.rotation[2]),
        );
        let q = serde_json::json!({
            "X": cr * sp * sy - sr * cp * cy,
            "Y": -cr * sp * cy - sr * cp * sy,
            "Z": cr * cp * sy - sr * sp * cy,
            "W": cr * cp * cy + sr * sp * sy,
        });
        let [x, y, z] = self.location;
        local(&serde_json::json!({"Rotation": q, "Translation": {"X": x, "Y": y, "Z": z}}))
    }
}

const MAX_PART_BONES: usize = 2048;
const MAX_PART_VERTICES: usize = 2_000_000;

/// Unreal mannequin (Fortnite player skeleton) name to its ValveBiped
/// counterpart, which `build_rig` already maps onto `_humanoid`. Spine bones
/// are matched positionally there, so only their order matters. Metacarpals,
/// twist, deform and dynamic bones follow their nearest mapped ancestor.
fn valvebiped_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let (stem, side) = match lower.strip_suffix("_l") {
        Some(stem) => (stem, "L"),
        None => match lower.strip_suffix("_r") {
            Some(stem) => (stem, "R"),
            None => (lower.as_str(), ""),
        },
    };
    let center = match stem {
        "pelvis" => Some("Pelvis"),
        "spine_01" => Some("Spine"),
        "spine_02" => Some("Spine1"),
        "spine_03" => Some("Spine2"),
        "spine_04" => Some("Spine3"),
        "spine_05" => Some("Spine4"),
        "neck_01" => Some("Neck1"),
        "neck_02" => Some("Neck2"),
        "head" => Some("Head1"),
        _ => None,
    };
    if side.is_empty() {
        return center.map(|bone| format!("ValveBiped.Bip01_{bone}"));
    }
    let limb = match stem {
        "clavicle" => "Clavicle",
        "upperarm" => "UpperArm",
        "lowerarm" => "Forearm",
        "hand" => "Hand",
        "thigh" => "Thigh",
        "calf" => "Calf",
        "foot" => "Foot",
        "ball" => "Toe0",
        // ValveBiped numbers digits thumb..pinky as Finger0..4, then 0n/0nn.
        _ => {
            let (finger, joint) = stem.rsplit_once('_')?;
            let digit = ["thumb", "index", "middle", "ring", "pinky"]
                .iter()
                .position(|f| *f == finger)?;
            // Finger1 (knuckle), Finger11, Finger12 for the index, and so on.
            let joint = match joint {
                "01" => "",
                "02" => "1",
                "03" => "2",
                _ => return None,
            };
            return Some(format!("ValveBiped.Bip01_{side}_Finger{digit}{joint}"));
        }
    };
    Some(format!("ValveBiped.Bip01_{side}_{limb}"))
}

struct PartBones {
    names: Vec<String>,
    parents: Vec<Option<usize>>,
    local: Vec<Mat4>,
    world: Vec<Mat4>,
}

fn part_bones(bones: &Value, part: &str) -> Result<PartBones, String> {
    let rows = bones
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= MAX_PART_BONES)
        .ok_or_else(|| format!("{part}: invalid bone list"))?;
    let mut out = PartBones {
        names: Vec::new(),
        parents: Vec::new(),
        local: Vec::new(),
        world: Vec::new(),
    };
    for (i, bone) in rows.iter().enumerate() {
        let name = bone["Name"]
            .as_str()
            .ok_or_else(|| format!("{part}: unnamed bone"))?
            .to_owned();
        let parent = match bone["ParentIndex"].as_i64() {
            Some(p) if p >= 0 && (p as usize) < i => Some(p as usize),
            Some(p) if p < 0 => None,
            _ => {
                return Err(format!(
                    "{part}: bone {name} is not stored after its parent"
                ))
            }
        };
        let local = local(&bone["Transform"]).map_err(|e| format!("{part}: bone {name}: {e}"))?;
        let world = parent.map_or(local, |p| out.world[p] * local);
        out.names.push(name);
        out.parents.push(parent);
        out.local.push(local);
        out.world.push(world);
    }
    Ok(out)
}

fn ue_vector(value: &Value) -> Option<Vec3> {
    let v = Vec3::new(
        value["X"].as_f64()? as f32,
        -(value["Y"].as_f64()? as f32),
        value["Z"].as_f64()? as f32,
    );
    v.is_finite().then_some(v)
}

fn rows3(m: &Mat4) -> [[f32; 4]; 3] {
    let c = m.to_cols_array_2d();
    [0, 1, 2].map(|r| [c[0][r], c[1][r], c[2][r], c[3][r]])
}

/// [`character_rig_input_mapped`] for the standard UE mannequin skeleton.
pub fn character_rig_input(
    model_name: &str,
    parts: &[CharacterPart],
) -> Result<(SourceSkeleton, RigGeometry), String> {
    character_rig_input_mapped(model_name, parts, &HashMap::new())
}

/// Merge `parts` (the first defines shared bones) into a rig skeleton and a
/// single-LOD geometry whose surface keys are (part, 0, section).
///
/// Bones are matched onto the JKA humanoid by name: `bone_map` (lower-case
/// source bone name to a ValveBiped or `_humanoid` bone name) first, then the
/// built-in UE mannequin names (`pelvis`, `spine_01`, `upperarm_l`, ...). Bones
/// matching neither inherit from their nearest mapped ancestor, so a game with
/// its own skeleton only needs to name its main limb and spine bones.
pub fn character_rig_input_mapped(
    model_name: &str,
    parts: &[CharacterPart],
    bone_map: &HashMap<String, String>,
) -> Result<(SourceSkeleton, RigGeometry), String> {
    let mut names: Vec<String> = Vec::new();
    let mut parents: Vec<Option<usize>> = Vec::new();
    let mut worlds: Vec<Mat4> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut geometry = RigGeometry::default();
    let mut pool: Vec<RigVertex> = Vec::new();
    let mut draws = Vec::new();
    for (part_index, part) in parts.iter().enumerate() {
        let mesh: Value =
            serde_json::from_slice(part.mesh).map_err(|e| format!("{}: {e}", part.name))?;
        let bones = part_bones(&mesh["bones"], part.name)?;
        // Merge by name; a new bone's parent was merged before it. An attached
        // part keeps its own bones, its root parented to the socket bone.
        let socket = match part.attach {
            Some(socket) => {
                let bone = *index_of
                    .get(&socket.bone.to_ascii_lowercase())
                    .ok_or_else(|| {
                        format!(
                            "{}: socket bone {} is not in the merged skeleton",
                            part.name, socket.bone
                        )
                    })?;
                Some((
                    bone,
                    socket
                        .local()
                        .map_err(|e| format!("{}: socket: {e}", part.name))?,
                ))
            }
            None => None,
        };
        let mut merged = Vec::with_capacity(bones.names.len());
        for i in 0..bones.names.len() {
            if let Some((socket_bone, socket_local)) = socket {
                let (parent, local) = match bones.parents[i] {
                    Some(p) => (merged[p], bones.local[i]),
                    None => (socket_bone, socket_local * bones.local[i]),
                };
                names.push(format!("{}_{}", part.name, bones.names[i]));
                parents.push(Some(parent));
                worlds.push(worlds[parent] * local);
                merged.push(names.len() - 1);
                continue;
            }
            let key = bones.names[i].to_ascii_lowercase();
            let index = match index_of.get(&key) {
                Some(&found) => found,
                None => {
                    let parent = bones.parents[i].map(|p| merged[p]);
                    let world =
                        parent.map_or(bones.local[i], |p: usize| worlds[p] * bones.local[i]);
                    names.push(bones.names[i].clone());
                    parents.push(parent);
                    worlds.push(world);
                    index_of.insert(key, names.len() - 1);
                    names.len() - 1
                }
            };
            merged.push(index);
        }
        // Bind-pose correction from this part's skeleton into the merged one.
        let correction: Vec<Mat4> = (0..bones.names.len())
            .map(|i| worlds[merged[i]] * bones.world[i].inverse())
            .collect();
        let rows = mesh["vertices"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= MAX_PART_VERTICES)
            .ok_or_else(|| format!("{}: invalid vertex list", part.name))?;
        let base = pool.len();
        for (v, row) in rows.iter().enumerate() {
            let position = ue_vector(&row["Position"])
                .ok_or_else(|| format!("{}: vertex {v} position", part.name))?;
            let normal = ue_vector(&row["Normal"]).unwrap_or(Vec3::Z);
            let mut influences = Vec::new();
            let mut blend = Mat4::ZERO;
            for influence in row["Influences"].as_array().into_iter().flatten() {
                let (Some(bone), Some(weight)) =
                    (influence["Bone"].as_u64(), influence["Weight"].as_f64())
                else {
                    continue;
                };
                let (bone, weight) = (bone as usize, weight as f32);
                if weight <= 0. || !weight.is_finite() {
                    continue;
                }
                let Some(&target) = merged.get(bone) else {
                    return Err(format!(
                        "{}: vertex {v} references bone {bone} outside its skeleton",
                        part.name
                    ));
                };
                influences.push((target, weight));
                blend += correction[bone] * weight;
            }
            let total: f32 = influences.iter().map(|(_, w)| w).sum();
            if total <= 1e-6 {
                return Err(format!("{}: vertex {v} has no skinning weights", part.name));
            }
            let blend = blend * (1. / total);
            let uv = &row["Uv"];
            pool.push(RigVertex {
                position: blend.transform_point3(position).to_array(),
                normal: blend
                    .transform_vector3(normal)
                    .normalize_or(Vec3::Z)
                    .to_array(),
                uv: [
                    uv["U"].as_f64().unwrap_or(0.) as f32,
                    uv["V"].as_f64().unwrap_or(0.) as f32,
                ],
                influences,
            });
        }
        let indices: Vec<u32> = mesh["indices"]
            .as_array()
            .ok_or_else(|| format!("{}: no indices", part.name))?
            .iter()
            .map(|i| {
                i.as_u64()
                    .filter(|&i| (i as usize) < rows.len())
                    .map(|i| (i as usize + base) as u32)
            })
            .collect::<Option<_>>()
            .ok_or_else(|| format!("{}: index outside vertices", part.name))?;
        for (section_index, section) in mesh["sections"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let first = section["first_index"].as_u64().unwrap_or(0) as usize;
            let end = first + 3 * section["triangles"].as_u64().unwrap_or(0) as usize;
            let Some(slice) = indices.get(first..end) else {
                return Err(format!(
                    "{}: section {section_index} outside indices",
                    part.name
                ));
            };
            let Some(shader) = part.shaders.get(section_index).filter(|s| !s.is_empty()) else {
                continue;
            };
            // The Y reflection mirrors handedness, so winding is reversed too.
            let mut tris = slice.to_vec();
            for tri in tris.chunks_exact_mut(3) {
                tri.swap(1, 2);
            }
            let key = (part_index, 0, section_index);
            geometry.surfaces.push(RigSurfaceSpec {
                key,
                name: format!("{}_{section_index}", part.name),
                shader: shader.clone(),
                vmt_candidates: Vec::new(),
            });
            draws.push(RigDraw { key, indices: tris });
        }
    }
    if draws.is_empty() {
        return Err(format!("{model_name}: no drawable sections"));
    }
    geometry.lods.push((pool, draws));
    // The rig skeleton is rigid. A scaled bone (Fortnite heads scale
    // `faceAttach`) still moves its children, so rigid world frames are taken
    // from the scaled hierarchy and the locals rebuilt between them.
    let rigid: Vec<Mat4> = worlds
        .iter()
        .map(|w| {
            let (_, rotation, translation) = w.to_scale_rotation_translation();
            Mat4::from_rotation_translation(rotation, translation)
        })
        .collect();
    let bones = (0..names.len())
        .map(|i| {
            let local = parents[i].map_or(rigid[i], |p| rigid[p].inverse() * rigid[i]);
            let (_, rotation, translation) = local.to_scale_rotation_translation();
            SourceBone {
                name: bone_map
                    .get(&names[i].to_ascii_lowercase())
                    .cloned()
                    .or_else(|| valvebiped_name(&names[i]))
                    .unwrap_or_else(|| names[i].clone()),
                parent: parents[i],
                bind_position: translation.to_array(),
                bind_quaternion: rotation.to_array(),
                pose_to_bone: rows3(&rigid[i].inverse()),
                jiggle: None,
            }
        })
        .collect();
    Ok((
        SourceSkeleton {
            model_name: model_name.to_owned(),
            bones,
        },
        geometry,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one_triangle_character() -> Vec<u8> {
        let bone = |name: &str, parent: i64, y: f64| json!({"Name": name, "ParentIndex": parent, "Transform": {"Translation": {"X": 0.0, "Y": y, "Z": 10.0}}});
        let vertex = |x: f64| json!({"Position": {"X": x, "Y": 0.0, "Z": 0.0}, "Normal": {"X": 0.0, "Y": 0.0, "Z": 1.0}, "Uv": {"U": 0.0, "V": 0.0}, "Influences": [{"Bone": 1, "Weight": 1.0}]});
        serde_json::to_vec(&json!({
            "bones": [bone("Hips", -1, 0.0), bone("Chest", 0, 5.0)],
            "vertices": [vertex(0.0), vertex(1.0), vertex(2.0)],
            "indices": [0, 1, 2],
            "sections": [{"first_index": 0, "triangles": 1, "material": null}],
        }))
        .unwrap()
    }

    #[test]
    fn a_bone_map_names_skeletons_that_do_not_use_the_mannequin_names() {
        let mesh = one_triangle_character();
        let shaders = vec!["skin".to_owned()];
        let part = CharacterPart {
            name: "body",
            mesh: &mesh,
            shaders: &shaders,
            attach: None,
        };

        // Without a map the custom names stay as they are...
        let (plain, _) = character_rig_input("t", std::slice::from_ref(&part)).unwrap();
        assert_eq!(
            plain
                .bones
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>(),
            ["Hips", "Chest"]
        );

        // ...and a map (case-insensitive) renames them for the rig fit.
        let map = HashMap::from([
            ("hips".to_owned(), "ValveBiped.Bip01_Pelvis".to_owned()),
            ("chest".to_owned(), "ValveBiped.Bip01_Spine2".to_owned()),
        ]);
        let (mapped, _) = character_rig_input_mapped("t", &[part], &map).unwrap();
        assert_eq!(
            mapped
                .bones
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>(),
            ["ValveBiped.Bip01_Pelvis", "ValveBiped.Bip01_Spine2"]
        );
    }

    use super::valvebiped_name;
    #[test]
    fn fortnite_bones_take_valvebiped_names() {
        assert_eq!(
            valvebiped_name("upperarm_l").as_deref(),
            Some("ValveBiped.Bip01_L_UpperArm")
        );
        assert_eq!(
            valvebiped_name("ball_r").as_deref(),
            Some("ValveBiped.Bip01_R_Toe0")
        );
        assert_eq!(
            valvebiped_name("spine_03").as_deref(),
            Some("ValveBiped.Bip01_Spine2")
        );
        assert_eq!(
            valvebiped_name("head").as_deref(),
            Some("ValveBiped.Bip01_Head1")
        );
        assert_eq!(
            valvebiped_name("thumb_01_l").as_deref(),
            Some("ValveBiped.Bip01_L_Finger0")
        );
        assert_eq!(
            valvebiped_name("index_02_r").as_deref(),
            Some("ValveBiped.Bip01_R_Finger11")
        );
        assert_eq!(
            valvebiped_name("pinky_03_l").as_deref(),
            Some("ValveBiped.Bip01_L_Finger42")
        );
        assert_eq!(valvebiped_name("index_metacarpal_l"), None);
        assert_eq!(valvebiped_name("dyn_tags"), None);
    }
}
