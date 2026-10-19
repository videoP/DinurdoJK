//! ActorX (`.psa`) animation reader for exports from CUE4Parse / UModel.
//! Decode CUE4Parse transforms into right-handed coordinates before FK.
use glam::{Quat, Vec3};
use std::collections::{HashMap, HashSet};

fn mirror_position(p: Vec3) -> Vec3 {
    Vec3::new(p.x, -p.y, p.z)
}
fn mirror_rotation(q: Quat) -> Quat {
    Quat::from_xyzw(-q.x, q.y, -q.z, q.w)
}

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    pub rest_rotation: Quat,
    pub rest_position: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(out: &mut Vec<u8>, id: &str, size: u32, count: u32, data: &[u8]) {
        let at = out.len();
        out.resize(at + 32, 0);
        out[at..at + id.len()].copy_from_slice(id.as_bytes());
        out[at + 24..at + 28].copy_from_slice(&size.to_le_bytes());
        out[at + 28..at + 32].copy_from_slice(&count.to_le_bytes());
        out.extend_from_slice(data);
    }
    fn fixture() -> Vec<u8> {
        let mut bytes = Vec::new();
        chunk(&mut bytes, "ANIMHEAD", 0, 0, &[]);
        let mut bone = vec![0u8; 120];
        bone[..4].copy_from_slice(b"Root");
        bone[72..76].copy_from_slice(&(-1i32).to_le_bytes());
        bone[88..92].copy_from_slice(&1f32.to_le_bytes());
        chunk(&mut bytes, "BONENAMES", 120, 1, &bone);
        let mut info = vec![0; 168];
        info[128..132].copy_from_slice(&1u32.to_le_bytes());
        info[152..156].copy_from_slice(&30f32.to_le_bytes());
        info[164..168].copy_from_slice(&1u32.to_le_bytes());
        chunk(&mut bytes, "ANIMINFO", 168, 1, &info);
        let mut key = vec![0; 32];
        key[4..8].copy_from_slice(&(-2f32).to_le_bytes());
        key[24..28].copy_from_slice(&(-1f32).to_le_bytes());
        chunk(&mut bytes, "ANIMKEYS", 32, 1, &key);
        let mut scale = Vec::new();
        for n in [1f32, 1.2, 1.0, 1.0] {
            scale.extend_from_slice(&n.to_le_bytes());
        }
        chunk(&mut bytes, "SCALEKEYS", 16, 1, &scale);
        bytes
    }
    #[test]
    fn cue4parse_keys_and_reference_use_the_same_right_handed_basis() {
        let psa = Psa::parse(&fixture()).unwrap();
        assert_eq!(psa.fps, 30.0);
        assert_eq!(psa.keys[0].position.y, -2.0);
        assert_eq!(psa.keys[0].rotation, Quat::IDENTITY);
        assert_eq!(psa.scales[0].y, 1.2);
    }
    #[test]
    fn reflected_rotation_rotates_reflected_points_in_the_same_direction() {
        let q = Quat::from_rotation_x(0.7) * Quat::from_rotation_z(-0.4);
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert!((mirror_rotation(q) * mirror_position(p) - mirror_position(q * p)).length() < 1e-5);
    }
    #[test]
    fn truncated_and_overflowing_chunks_fail() {
        let bytes = fixture();
        assert!(Psa::parse(&bytes[..bytes.len() - 1]).is_err());
        let mut bytes = fixture();
        bytes[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Psa::parse(&bytes).is_err());
        assert!(Psa::parse(b"not a PSA").is_err());
    }
}
/// Local-space reference pose (UE convention: translation, XYZW rotation,
/// scale) by bone name, the base of `AAT_LocalSpaceBase` additive clips.
pub type RefPose = std::collections::HashMap<String, (Vec3, Quat, Vec3)>;

/// The source skeleton's reference pose, needed to resolve local-space additive
/// clips. It is read from the JSON file named by `DJK_UE_REFPOSE`
/// (`DJK_FN_REFPOSE` is accepted for older setups):
/// `{"skeleton": {bone: {"t": [..3], "r": [..4], "s": [..3]}}}`, written from
/// your game's exported `USkeleton` (UE convention: translation, XYZW rotation,
/// scale). Only additive clips need it.
pub fn reference_pose() -> Result<&'static RefPose, String> {
    static POSE: std::sync::OnceLock<Result<RefPose, String>> = std::sync::OnceLock::new();
    POSE.get_or_init(|| {
        let path = std::env::var("DJK_UE_REFPOSE")
            .or_else(|_| std::env::var("DJK_FN_REFPOSE"))
            .map_err(|_| {
                "additive clip needs DJK_UE_REFPOSE (source skeleton reference pose JSON)"
                    .to_owned()
            })?;
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?)
                .map_err(|e| e.to_string())?;
        let bones = value["skeleton"]
            .as_object()
            .ok_or("reference pose JSON has no skeleton")?;
        let f = |v: &serde_json::Value, i: usize| v[i].as_f64().unwrap_or(0.) as f32;
        Ok(bones
            .iter()
            .map(|(name, b)| {
                (
                    name.to_ascii_lowercase(),
                    (
                        Vec3::new(f(&b["t"], 0), f(&b["t"], 1), f(&b["t"], 2)),
                        Quat::from_xyzw(f(&b["r"], 0), f(&b["r"], 1), f(&b["r"], 2), f(&b["r"], 3))
                            .normalize(),
                        Vec3::new(f(&b["s"], 0), f(&b["s"], 1), f(&b["s"], 2)),
                    ),
                )
            })
            .collect())
    })
    .as_ref()
    .map_err(Clone::clone)
}

enum AdditiveBase {
    Clip(std::path::PathBuf),
    ReferencePose,
}

#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub rotation: Quat,
    pub position: Vec3,
}
#[derive(Debug)]
pub struct Psa {
    pub bones: Vec<Bone>,
    pub fps: f32,
    pub frames: usize,
    pub keys: Vec<Key>,
    pub scales: Vec<Vec3>,
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        data.get(at..at + 4)
            .ok_or("truncated PSA field")?
            .try_into()
            .unwrap(),
    ))
}
fn float(data: &[u8], at: usize) -> Result<f32, String> {
    let n = f32::from_bits(u32_at(data, at)?);
    if !n.is_finite() {
        return Err("non-finite PSA transform".into());
    }
    Ok(n)
}
fn vector(data: &[u8], at: usize) -> Result<Vec3, String> {
    Ok(Vec3::new(
        float(data, at)?,
        float(data, at + 4)?,
        float(data, at + 8)?,
    ))
}
fn quaternion(data: &[u8], at: usize) -> Result<Quat, String> {
    let q = Quat::from_xyzw(
        float(data, at)?,
        float(data, at + 4)?,
        float(data, at + 8)?,
        float(data, at + 12)?,
    );
    if q.length_squared() < 0.0001 {
        return Err("zero PSA quaternion".into());
    }
    Ok(q.normalize())
}
fn name(data: &[u8]) -> Result<String, String> {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    std::str::from_utf8(&data[..end])
        .map(str::to_owned)
        .map_err(|_| "non-UTF8 PSA name".into())
}

impl Psa {
    /// Parse a PSA file. A clip whose exported metadata (`Json/<stem>.json`
    /// beside it) marks it `AAT_LocalSpaceBase` stores per-bone offsets from
    /// the reference pose, not poses: it is composed onto that pose here so
    /// every consumer sees an absolute animation.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut psa = Self::parse(&bytes)?;
        // Many clips flagged additive already export usable poses (their
        // recipes worked unchanged), so only clips listed in
        // DJK_FN_ADDITIVE_BASES are composed.
        if Self::is_local_additive(path) {
            match Self::additive_base(path) {
                // ABPT_AnimScaled clips (the female `_CMF` variants) are offsets
                // from the male clip of the same emote, time-scaled to this length.
                Some(AdditiveBase::Clip(base)) => psa.apply_additive_clip(&Self::parse(
                    &std::fs::read(&base).map_err(|e| format!("{}: {e}", base.display()))?,
                )?),
                Some(AdditiveBase::ReferencePose) => psa.apply_local_additive(reference_pose()?),
                None => {}
            }
        }
        Ok(psa)
    }

    /// `DJK_FN_ADDITIVE_BASES/<stem>.psa` is the exported base sequence of an
    /// additive clip (its RefPoseSeq, or the male `_CMM` clip when the export
    /// stripped it); an empty `<stem>.refpose` marker composes onto the
    /// skeleton reference pose instead.
    fn additive_base(path: &std::path::Path) -> Option<AdditiveBase> {
        let dir = std::path::PathBuf::from(std::env::var_os("DJK_FN_ADDITIVE_BASES")?);
        let base = dir.join(path.file_name()?);
        if base.is_file() {
            return Some(AdditiveBase::Clip(base));
        }
        dir.join(path.file_stem()?)
            .with_extension("refpose")
            .is_file()
            .then_some(AdditiveBase::ReferencePose)
    }

    /// Accumulate onto another clip, sampled at the same normalized time
    /// (nearest frame), bones matched by name. Both clips are already mirrored.
    pub fn apply_additive_clip(&mut self, base: &Psa) {
        let bones = self.bones.len().max(1);
        let base_bones = base.bones.len().max(1);
        let index: Vec<Option<usize>> = self
            .bones
            .iter()
            .map(|b| {
                base.bones
                    .iter()
                    .position(|o| o.name.eq_ignore_ascii_case(&b.name))
            })
            .collect();
        let span = self.frames.saturating_sub(1).max(1) as f32;
        let base_last = base.frames.saturating_sub(1);
        for (i, key) in self.keys.iter_mut().enumerate() {
            let Some(Some(other)) = index.get(i % bones) else {
                continue;
            };
            let frame = ((i / bones) as f32 / span * base_last as f32).round() as usize;
            let at = frame.min(base_last) * base_bones + other;
            let Some(b) = base.keys.get(at) else { continue };
            key.rotation = (key.rotation * b.rotation).normalize();
            key.position += b.position;
            if let Some(scale) = self.scales.get_mut(i) {
                let s = base.scales.get(at).copied().unwrap_or(Vec3::ONE);
                *scale = if *scale == Vec3::ONE {
                    s
                } else {
                    s * (Vec3::ONE + *scale)
                };
            }
        }
    }

    fn is_local_additive(path: &std::path::Path) -> bool {
        let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else {
            return false;
        };
        let meta = dir.join("Json").join(stem).with_extension("json");
        std::fs::read_to_string(meta).is_ok_and(|text| text.contains("AAT_LocalSpaceBase"))
    }

    /// UE additive accumulate onto `base` (mirrored like parsed keys):
    /// rotation = delta * base, translation = base + delta and
    /// scale = base * (1 + delta). Exported "no scale track" keys were already
    /// turned into 1 by `parse` and keep the base scale.
    pub fn apply_local_additive(&mut self, base: &RefPose) {
        let bones = self.bones.len().max(1);
        for (i, key) in self.keys.iter_mut().enumerate() {
            let Some(bone) = self.bones.get(i % bones) else {
                continue;
            };
            let Some(&(t, r, s)) = base.get(&bone.name.to_ascii_lowercase()) else {
                continue;
            };
            key.rotation = (key.rotation * mirror_rotation(r)).normalize();
            key.position += mirror_position(t);
            if let Some(scale) = self.scales.get_mut(i) {
                *scale = if *scale == Vec3::ONE {
                    s
                } else {
                    s * (Vec3::ONE + *scale)
                };
            }
        }
    }

    pub fn parse(data: &[u8]) -> Result<Self, String> {
        if data.len() > 128 * 1024 * 1024 {
            return Err("PSA exceeds 128 MiB".into());
        }
        let mut chunks = HashMap::new();
        let mut at = 0;
        while at < data.len() {
            let header = data.get(at..at + 32).ok_or("truncated PSA chunk header")?;
            let id = name(&header[..20])?;
            let size = u32_at(header, 24)? as usize;
            let count = u32_at(header, 28)? as usize;
            let len = size.checked_mul(count).ok_or("PSA chunk length overflow")?;
            let end = (at + 32)
                .checked_add(len)
                .filter(|&n| n <= data.len())
                .ok_or("PSA chunk outside file")?;
            if chunks
                .insert(id, (size, count, &data[at + 32..end]))
                .is_some()
            {
                return Err("duplicate PSA chunk".into());
            }
            at = end;
        }
        if !chunks.contains_key("ANIMHEAD") {
            return Err("missing PSA ANIMHEAD".into());
        }
        let &(size, count, records) = chunks.get("BONENAMES").ok_or("missing BONENAMES")?;
        if size != 120 || !(1..=4096).contains(&count) {
            return Err("unsupported PSA bone record layout/count".into());
        }
        let mut bones = Vec::with_capacity(count);
        let mut seen = HashSet::new();
        for i in 0..count {
            let b = &records[i * size..(i + 1) * size];
            let bone_name = name(&b[..64])?;
            if !seen.insert(bone_name.to_ascii_lowercase()) {
                return Err("duplicate PSA bone name".into());
            }
            let parent = u32_at(b, 72)? as i32;
            let parent = if parent < 0 || (i == 0 && parent == 0) {
                None
            } else {
                let parent = parent as usize;
                if parent >= i {
                    return Err("PSA bones must be in parent-first order".into());
                }
                Some(parent)
            };
            // BONENAMES are written without the ANIMKEYS mirror/root changes.
            bones.push(Bone {
                name: bone_name,
                parent,
                rest_rotation: mirror_rotation(quaternion(b, 76)?),
                rest_position: mirror_position(vector(b, 92)?),
            });
        }
        let &(size, count, info) = chunks.get("ANIMINFO").ok_or("missing ANIMINFO")?;
        if size != 168 || count != 1 {
            return Err("expected one 168-byte PSA animation info record".into());
        }
        if u32_at(info, 128)? as usize != bones.len() || u32_at(info, 160)? != 0 {
            return Err("unsupported partial PSA clip".into());
        }
        let fps = float(info, 152)?;
        let frames = u32_at(info, 164)? as usize;
        if !(1.0..=240.0).contains(&fps) || frames == 0 || frames > 1_000_000 {
            return Err("invalid PSA sample rate/frame count".into());
        }
        let expected = frames
            .checked_mul(bones.len())
            .ok_or("PSA key count overflow")?;
        let &(size, count, raw) = chunks.get("ANIMKEYS").ok_or("missing ANIMKEYS")?;
        if size != 32 || count != expected {
            return Err("PSA key layout/count mismatch".into());
        }
        let mut keys = Vec::with_capacity(count);
        for i in 0..count {
            let b = &raw[i * size..(i + 1) * size];
            let mut position = vector(b, 0)?;
            let mut rotation = quaternion(b, 12)?;
            position.y = -position.y;
            rotation.y = -rotation.y;
            if i % bones.len() == 0 {
                rotation.w = -rotation.w;
            }
            // Undo ActorX's key-only changes, then reflect BOTH rest and keys
            // from Unreal's left-handed basis. A rotation alone cannot do this.
            keys.push(Key {
                rotation: mirror_rotation(rotation),
                position: mirror_position(position),
            });
        }
        let scales = if let Some(&(size, count, raw)) = chunks.get("SCALEKEYS") {
            if size != 16 || count != expected {
                return Err("PSA scale key layout/count mismatch".into());
            }
            let mut scales = (0..count)
                .map(|i| vector(raw, i * size))
                .collect::<Result<Vec<_>, _>>()?;
            // Exporters write (0,0,0) for bones without a scale track. A bone
            // that is zero in every frame has no track (scale 1); a bone that
            // is zero only in some frames is an intentional hide and is kept.
            let bone_count = bones.len().max(1);
            for bone in 0..bone_count {
                let all_zero = (0..frames).all(|f| {
                    scales
                        .get(f * bone_count + bone)
                        .is_some_and(|v| *v == Vec3::ZERO)
                });
                if all_zero {
                    for f in 0..frames {
                        if let Some(v) = scales.get_mut(f * bone_count + bone) {
                            *v = Vec3::ONE;
                        }
                    }
                }
            }
            scales
        } else {
            vec![Vec3::ONE; expected]
        };
        Ok(Self {
            bones,
            fps,
            frames,
            keys,
            scales,
        })
    }

    pub fn world_rotations(&self, frame: Option<usize>) -> Vec<Quat> {
        let mut world = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let local = frame.map_or(b.rest_rotation, |f| {
                self.keys[f * self.bones.len() + i].rotation
            });
            world.push(b.parent.map_or(local, |p| world[p] * local));
        }
        world
    }
}
