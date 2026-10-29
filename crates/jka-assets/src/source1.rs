//! Source 1 (StudioMDL v48/v49) skeleton and material-table importer.
//!
//! This does NOT pretend that ValveBiped transforms are interchangeable with
//! Ghoul2's `_humanoid.gla`: `source1_rig` performs an explicit rebind.


/// Source SDK `STUDIO_PROC_JIGGLE` (5) and `mstudiojigglebone_t` flags.
/// These are authored physics constraints, not animation keyframes.
pub const STUDIO_PROC_JIGGLE: i32 = 5;
pub const JIGGLE_IS_FLEXIBLE: u32 = 0x01;
pub const JIGGLE_IS_RIGID: u32 = 0x02;
pub const JIGGLE_HAS_YAW_CONSTRAINT: u32 = 0x04;
pub const JIGGLE_HAS_PITCH_CONSTRAINT: u32 = 0x08;
pub const JIGGLE_HAS_ANGLE_CONSTRAINT: u32 = 0x10;
pub const JIGGLE_HAS_LENGTH_CONSTRAINT: u32 = 0x20;
pub const JIGGLE_HAS_BASE_SPRING: u32 = 0x40;
pub const JIGGLE_IS_BOING: u32 = 0x80;

/// The actual Source 1 binary jiggle rule (35 four-byte fields; v48/v49).
/// Values are kept in Source's authored units. The secondary-motion solver
/// converts length into model units after rig fitting.
#[derive(Debug, Clone, Copy, Default)]
pub struct SourceJiggleParams {
    pub flags: u32,
    pub length: f32,
    pub tip_mass: f32,
    pub yaw_stiffness: f32,
    pub yaw_damping: f32,
    pub pitch_stiffness: f32,
    pub pitch_damping: f32,
    pub along_stiffness: f32,
    pub along_damping: f32,
    pub angle_limit: f32,
    pub min_yaw: f32,
    pub max_yaw: f32,
    pub yaw_friction: f32,
    pub yaw_bounce: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,
    pub pitch_friction: f32,
    pub pitch_bounce: f32,
    pub base_mass: f32,
    pub base_stiffness: f32,
    pub base_damping: f32,
    pub base_min_left: f32,
    pub base_max_left: f32,
    pub base_left_friction: f32,
    pub base_min_up: f32,
    pub base_max_up: f32,
    pub base_up_friction: f32,
    pub base_min_forward: f32,
    pub base_max_forward: f32,
    pub base_forward_friction: f32,
    pub boing_impact_speed: f32,
    pub boing_impact_angle: f32,
    pub boing_damping_rate: f32,
    pub boing_frequency: f32,
    pub boing_amplitude: f32,
}

fn parse_jiggle_params(bytes: &[u8], offset: usize) -> Result<SourceJiggleParams, String> {
    // All 35 fields, including fields we don't yet simulate, must be in the
    // declared MDL; unchecked procindex offsets are not trustworthy.
    bytes.get(offset..offset.checked_add(140).ok_or("jiggle offset overflow")?)
        .ok_or("jiggle rule outside MDL")?;
    let flags = i32_at(bytes, offset)? as u32;
    let field = |index: usize| f32_at(bytes, offset + index * 4);
    Ok(SourceJiggleParams {
        flags, length: field(1)?, tip_mass: field(2)?,
        yaw_stiffness: field(3)?, yaw_damping: field(4)?,
        pitch_stiffness: field(5)?, pitch_damping: field(6)?,
        along_stiffness: field(7)?, along_damping: field(8)?,
        angle_limit: field(9)?,
        min_yaw: field(10)?, max_yaw: field(11)?,
        yaw_friction: field(12)?, yaw_bounce: field(13)?,
        min_pitch: field(14)?, max_pitch: field(15)?,
        pitch_friction: field(16)?, pitch_bounce: field(17)?,
        base_mass: field(18)?, base_stiffness: field(19)?,
        base_damping: field(20)?, base_min_left: field(21)?,
        base_max_left: field(22)?, base_left_friction: field(23)?,
        base_min_up: field(24)?, base_max_up: field(25)?,
        base_up_friction: field(26)?, base_min_forward: field(27)?,
        base_max_forward: field(28)?, base_forward_friction: field(29)?,
        boing_impact_speed: field(30)?, boing_impact_angle: field(31)?,
        boing_damping_rate: field(32)?, boing_frequency: field(33)?,
        boing_amplitude: field(34)?,
    })
}

#[derive(Debug, Clone)]
pub struct SourceBone {
    pub name: String,
    pub parent: Option<usize>,
    pub jiggle: Option<SourceJiggleParams>,
    pub bind_position: [f32; 3],
    pub bind_quaternion: [f32; 4],
    /// StudioMDL's authored inverse bind matrix (`mstudiobone_t::poseToBone`).
    /// Kept so the rig can cross-check its own derived inverse against it.
    pub pose_to_bone: [[f32; 4]; 3],
}

#[derive(Debug, Clone)]
pub struct SourceSkeleton {
    pub model_name: String,
    pub bones: Vec<SourceBone>,
}

/// The `$texture`/`$cdmaterials`/`$texturegroup` tables. `mstudiomesh_t::material`
/// is a *skin reference*, so it must be resolved through a skin family before it
/// names a texture.
#[derive(Debug, Clone)]
pub struct SourceMaterialTable {
    /// Material base names, without directory or `.vmt`.
    pub textures: Vec<String>,
    /// `$cdmaterials` search directories, slash-separated, each with a trailing `/`.
    pub directories: Vec<String>,
    /// `skin_families[family][skin_ref]` is an index into `textures`.
    pub skin_families: Vec<Vec<usize>>,
}

impl SourceMaterialTable {
    /// Resolve a mesh's `skin_ref` through one skin family. Out-of-range
    /// families fall back to family 0, matching the engine's clamp.
    pub fn texture_index(&self, family: usize, skin_ref: i32) -> Option<usize> {
        let table = self
            .skin_families
            .get(family)
            .or_else(|| self.skin_families.first())?;
        let slot = usize::try_from(skin_ref).ok()?;
        // A model with no texturegroup still has an identity family; a skin_ref
        // past its end is addressed directly, exactly as the engine does.
        table.get(slot).copied().or(Some(slot))
    }

    /// Candidate `materials/...vmt` qpaths for a texture slot, in search order.
    pub fn vmt_candidates(&self, texture_index: usize) -> Vec<String> {
        let Some(name) = self.textures.get(texture_index) else {
            return Vec::new();
        };
        // An absolute material name ignores $cdmaterials, as in the engine.
        if name.contains('/') {
            return vec![format!("materials/{name}.vmt")];
        }
        self.directories
            .iter()
            .map(|dir| format!("materials/{dir}{name}.vmt"))
            .collect()
    }
}

fn i32_at(bytes: &[u8], offset: usize) -> Result<i32, String> {
    let raw = bytes
        .get(offset..offset.checked_add(4).ok_or("offset overflow")?)
        .ok_or_else(|| format!("Source MDL truncated at byte {offset}"))?;
    Ok(i32::from_le_bytes(raw.try_into().unwrap()))
}
fn f32_at(bytes: &[u8], offset: usize) -> Result<f32, String> {
    let value = f32::from_bits(i32_at(bytes, offset)? as u32);
    if !value.is_finite() {
        return Err(format!("non-finite Source bone component at {offset}"));
    }
    Ok(value)
}
fn string_at(bytes: &[u8], start: usize, max: usize) -> Result<String, String> {
    let end_bound = start.checked_add(max).ok_or("string offset overflow")?;
    let slice = bytes
        .get(start..end_bound.min(bytes.len()))
        .ok_or("invalid string offset")?;
    let end = slice
        .iter()
        .position(|v| *v == 0)
        .ok_or("unterminated Source string")?;
    Ok(String::from_utf8_lossy(&slice[..end]).into_owned())
}
fn usize_at(bytes: &[u8], offset: usize, what: &str) -> Result<usize, String> {
    usize::try_from(i32_at(bytes, offset)?).map_err(|_| format!("negative Source {what}"))
}

/// StudioMDL versions whose field layout matches the one this importer reads.
///
/// 49 is accepted on evidence, not on the assumption that it is "close enough":
/// `w3_geralt_bear_v5.mdl` is a real v49 file whose MDL/VVD/VTX checksums agree
/// and whose per-mesh `numLODVertexes` - read at the far end of the
/// bodypart -> model -> mesh chain - reconcile exactly with the VVD's LOD0 total
/// of 114993, which could not happen if any offset in that chain had moved.
/// `mstudiobone_t` is likewise unchanged: its 71 bone names and parents decode
/// at the v48 stride, and `build_source_rig` cross-checks every authored
/// `poseToBone` against the inverse bind derived from the chain.
///
/// Do not add a version here without running `tools/source1/geometry_probe.py`
/// over a real file of that version first; the probe checks exactly the
/// invariants that a moved field would break.
pub const SUPPORTED_MDL_VERSIONS: [i32; 2] = [48, 49];

fn check_supported_version(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.len() < 240 || bytes.get(..4) != Some(b"IDST") {
        return Err("not a Source StudioMDL file".into());
    }
    let version = i32_at(bytes, 4)?;
    if !SUPPORTED_MDL_VERSIONS.contains(&version) {
        return Err(format!(
            "unsupported Source MDL version {version} (supported: {SUPPORTED_MDL_VERSIONS:?})"
        ));
    }
    let length = i32_at(bytes, 76)?;
    if length < 240 || length as usize > bytes.len() {
        return Err("invalid MDL declared length".into());
    }
    Ok(&bytes[..length as usize])
}

/// Decode the bone table and rest pose from a supported Source 1 StudioMDL file.
/// Unsupported versions are rejected instead of guessing field layouts.
pub fn parse_source1_skeleton(bytes: &[u8]) -> Result<SourceSkeleton, String> {
    let bytes = check_supported_version(bytes)?;
    let name = string_at(bytes, 12, 64)?;
    let count = usize_at(bytes, 156, "bone count")?;
    let offset = usize_at(bytes, 160, "bone offset")?;
    const BONE_SIZE: usize = 216;
    if count == 0
        || count > 512
        || offset
            .checked_add(count * BONE_SIZE)
            .is_none_or(|end| end > bytes.len())
    {
        return Err("invalid Source MDL bone table".into());
    }
    let mut bones = Vec::with_capacity(count);
    for index in 0..count {
        let base = offset + index * BONE_SIZE;
        let rel_name = usize_at(bytes, base, "bone name offset")?;
        let parent = i32_at(bytes, base + 4)?;
        if parent < -1 || (parent >= 0 && parent as usize >= index) {
            return Err(format!(
                "Source bone {index} has invalid parent {parent}"
            ));
        }
        let start = base.checked_add(rel_name).ok_or("bone name offset overflow")?;
        let name = string_at(bytes, start, 256)?;
        if name.is_empty() {
            return Err(format!("Source bone {index} has empty name"));
        }
        let mut bind_position = [0.0; 3];
        let mut bind_quaternion = [0.0; 4];
        let mut pose_to_bone = [[0.0; 4]; 3];
        for (j, v) in bind_position.iter_mut().enumerate() {
            *v = f32_at(bytes, base + 32 + 4 * j)?;
        }
        for (j, v) in bind_quaternion.iter_mut().enumerate() {
            *v = f32_at(bytes, base + 44 + 4 * j)?;
        }
        for (j, row) in pose_to_bone.iter_mut().enumerate() {
            for (k, v) in row.iter_mut().enumerate() {
                *v = f32_at(bytes, base + 96 + (j * 4 + k) * 4)?;
            }
        }
        // `mstudiobone_t` v48/v49: proctype at +164, procindex at +168.
        // The index is bone-relative, not file-relative.
        let proctype = i32_at(bytes, base + 164)?;
        let procindex = i32_at(bytes, base + 168)?;
        let jiggle = if proctype == STUDIO_PROC_JIGGLE {
            if procindex <= 0 {
                return Err(format!("Source jiggle bone {name} has invalid procindex {procindex}"));
            }
            let at = base.checked_add(procindex as usize)
                .ok_or("jiggle rule offset overflow")?;
            Some(parse_jiggle_params(bytes, at)
                .map_err(|error| format!("Source jiggle bone {name}: {error}"))?)
        } else {
            None
        };
        bones.push(SourceBone {
            name,
            jiggle,
            parent: (parent >= 0).then_some(parent as usize),
            bind_position,
            bind_quaternion,
            pose_to_bone,
        });
    }
    Ok(SourceSkeleton {
        model_name: name,
        bones,
    })
}

/// Decode `$texture`, `$cdmaterials` and the `$texturegroup` skin table.
pub fn parse_source1_materials(bytes: &[u8]) -> Result<SourceMaterialTable, String> {
    let bytes = check_supported_version(bytes)?;
    let texture_count = usize_at(bytes, 204, "texture count")?.min(2048);
    let texture_at = usize_at(bytes, 208, "texture offset")?;
    let mut textures = Vec::with_capacity(texture_count);
    for i in 0..texture_count {
        let at = texture_at
            .checked_add(i * 64)
            .ok_or("MDL texture table overflow")?;
        let rel = usize_at(bytes, at, "texture name offset")?;
        let start = at.checked_add(rel).ok_or("texture name overflow")?;
        textures.push(string_at(bytes, start, 256)?.replace('\\', "/"));
    }

    let dir_count = usize_at(bytes, 212, "cdmaterials count")?.min(128);
    let dir_at = usize_at(bytes, 216, "cdmaterials offset")?;
    let mut directories = Vec::with_capacity(dir_count);
    for i in 0..dir_count {
        let slot = dir_at
            .checked_add(i * 4)
            .ok_or("MDL cdmaterials table overflow")?;
        let mut dir = string_at(bytes, usize_at(bytes, slot, "cdmaterials offset")?, 256)?
            .replace('\\', "/");
        if !dir.is_empty() && !dir.ends_with('/') {
            dir.push('/');
        }
        directories.push(dir);
    }
    if directories.is_empty() {
        // `$cdmaterials` is optional; the engine then looks beside `materials/`.
        directories.push(String::new());
    }

    let skin_refs = usize_at(bytes, 220, "skinref count")?.min(4096);
    let families = usize_at(bytes, 224, "skinfamily count")?.min(256);
    let skin_at = usize_at(bytes, 228, "skin table offset")?;
    let mut skin_families = Vec::with_capacity(families);
    for family in 0..families {
        let mut row = Vec::with_capacity(skin_refs);
        for slot in 0..skin_refs {
            let at = skin_at
                .checked_add((family * skin_refs + slot) * 2)
                .ok_or("MDL skin table overflow")?;
            let raw = bytes
                .get(at..at + 2)
                .ok_or("MDL skin table outside file")?;
            let index = i16::from_le_bytes(raw.try_into().unwrap());
            row.push(usize::try_from(index).map_err(|_| "negative MDL skin reference")?);
        }
        skin_families.push(row);
    }
    if skin_families.is_empty() {
        skin_families.push((0..textures.len()).collect());
    }
    Ok(SourceMaterialTable {
        textures,
        directories,
        skin_families,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v48_stub(len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        bytes[..4].copy_from_slice(b"IDST");
        bytes[4..8].copy_from_slice(&48i32.to_le_bytes());
        bytes[76..80].copy_from_slice(&(len as i32).to_le_bytes());
        bytes
    }

    #[test]
    fn refuses_non_source_data() {
        assert!(parse_source1_skeleton(b"not a model").is_err());
    }

    #[test]
    fn refuses_unsupported_version() {
        // 44 is the Orange Box layout. It is plausibly close, which is exactly
        // why it stays rejected until a real file of that version has been probed.
        let mut bytes = vec![0; 256];
        bytes[..4].copy_from_slice(b"IDST");
        bytes[4..8].copy_from_slice(&44i32.to_le_bytes());
        assert!(parse_source1_skeleton(&bytes)
            .unwrap_err()
            .contains("version 44"));
    }

    #[test]
    fn accepts_every_supported_version() {
        for version in SUPPORTED_MDL_VERSIONS {
            let mut bytes = v48_stub(256);
            bytes[4..8].copy_from_slice(&version.to_le_bytes());
            let error = parse_source1_skeleton(&bytes).err().unwrap_or_default();
            assert!(
                !error.contains("unsupported Source MDL version"),
                "version {version} was rejected by the gate: {error}"
            );
        }
    }

    #[test]
    fn refuses_invalid_bone_table() {
        let mut bytes = v48_stub(256);
        bytes[156..160].copy_from_slice(&600i32.to_le_bytes());
        assert!(parse_source1_skeleton(&bytes)
            .unwrap_err()
            .contains("bone table"));
    }

    #[test]
    fn parses_compiled_jigglebone_rule() {
        let mut mdl = v48_stub(640);
        mdl[156..160].copy_from_slice(&1i32.to_le_bytes());
        mdl[160..164].copy_from_slice(&256i32.to_le_bytes());
        mdl[256..260].copy_from_slice(&216i32.to_le_bytes());
        mdl[472..477].copy_from_slice(b"Hair\0");
        mdl[256 + 164..256 + 168].copy_from_slice(&STUDIO_PROC_JIGGLE.to_le_bytes());
        mdl[256 + 168..256 + 172].copy_from_slice(&240i32.to_le_bytes());
        let rule = 256 + 240;
        mdl[rule..rule + 4].copy_from_slice(&(JIGGLE_IS_FLEXIBLE | JIGGLE_HAS_ANGLE_CONSTRAINT).to_le_bytes());
        mdl[rule + 4..rule + 8].copy_from_slice(&12.5f32.to_le_bytes());
        mdl[rule + 36..rule + 40].copy_from_slice(&0.4f32.to_le_bytes());
        let bone = &parse_source1_skeleton(&mdl).unwrap().bones[0];
        let jiggle = bone.jiggle.unwrap();
        assert_eq!(bone.name, "Hair");
        assert_eq!(jiggle.length, 12.5);
        assert_eq!(jiggle.angle_limit, 0.4);
        assert!(jiggle.flags & JIGGLE_IS_FLEXIBLE != 0);
    }

    #[test]
    fn refuses_out_of_bounds_jiggle_rule() {
        let mut mdl = v48_stub(490);
        mdl[156..160].copy_from_slice(&1i32.to_le_bytes());
        mdl[160..164].copy_from_slice(&256i32.to_le_bytes());
        mdl[256..260].copy_from_slice(&216i32.to_le_bytes());
        mdl[472..477].copy_from_slice(b"Hair\0");
        mdl[256 + 164..256 + 168].copy_from_slice(&STUDIO_PROC_JIGGLE.to_le_bytes());
        mdl[256 + 168..256 + 172].copy_from_slice(&230i32.to_le_bytes());
        assert!(parse_source1_skeleton(&mdl).unwrap_err().contains("outside MDL"));
    }

    #[test]
    fn skin_family_resolves_through_the_table() {
        let table = SourceMaterialTable {
            textures: vec!["a".into(), "b".into(), "c".into()],
            directories: vec!["models/x/".into()],
            // Family 1 swaps slot 0 to texture 2, as a $texturegroup does.
            skin_families: vec![vec![0, 1, 2], vec![2, 1, 0]],
        };
        assert_eq!(table.texture_index(0, 0), Some(0));
        assert_eq!(table.texture_index(1, 0), Some(2));
        // An out-of-range family clamps to family 0 instead of vanishing.
        assert_eq!(table.texture_index(9, 0), Some(0));
        assert_eq!(
            table.vmt_candidates(2),
            vec!["materials/models/x/c.vmt".to_owned()]
        );
    }
}

#[cfg(test)]
pub(crate) fn test_parse_jiggle_for_animator(bytes: &[u8]) -> SourceJiggleParams {
    parse_jiggle_params(bytes, 0).unwrap()
}
