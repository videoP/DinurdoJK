//! Validated readers for Jedi Academy Ghoul2 GLM/GLA version 6 assets.
//!
//! Layout and decode rules are ported from OpenJK's `mdx_format.h`,
//! `tr_model.cpp`, and `MC_UnCompressQuat` in `qcommon/matcomp.cpp`.

const MDXM_VERSION: i32 = 6;
const MDXA_VERSION: i32 = 6;
const MDXM_HEADER_SIZE: usize = 164;
const MDXA_HEADER_SIZE: usize = 100;
const MDXM_SURFACE_SIZE: usize = 40;
const MDXM_VERTEX_SIZE: usize = 32;
const MDXM_HIERARCHY_PREFIX: usize = 144;
const MDXA_SKEL_PREFIX: usize = 172;
pub const MDXA_COMP_QUAT_BONE_SIZE: usize = 14;
const MAX_G2_BONEWEIGHTS_PER_VERT: usize = 4;
const G2_BITS_PER_BONEREF: u32 = 5;
const G2_BONEWEIGHT_RECIPROCAL_MULT: f32 = 1.0 / 1023.0;
const G2_BONEWEIGHT_TOPBITS_SHIFT: u32 =
    G2_BITS_PER_BONEREF * MAX_G2_BONEWEIGHTS_PER_VERT as u32 - 8;
const G2_BONEWEIGHT_TOPBITS_AND: u32 = 0x300;

// OpenJK/JaPRo `OldToNewRemapTable` from `tr_ghoul2.cpp`: Jedi Outcast's
// 72-bone humanoid meshes use Jedi Academy's 53-bone animation skeleton.
// Removed toe, metacarpal and finger bones map to their surviving counterparts.
const JK2_TO_JKA_HUMANOID_BONES: [usize; 72] = [
    0, 1, 2, 3, 4, 5, 6, 6, 7, 8, 9, 10, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
    24, 25, 26, 27, 28, 29, 29, 34, 35, 35, 30, 31, 31, 32, 33, 33, 32, 33, 33, 34, 35, 35, 36, 37,
    38, 39, 40, 41, 42, 42, 43, 44, 44, 43, 44, 44, 45, 46, 46, 45, 46, 46, 47, 48, 48, 52,
];
const JKA_HUMANOID_BONE_COUNT: usize = 53;

pub type Matrix3x4 = [[f32; 4]; 3];


/// OpenJK's renderer special-cases `*default.gla`: it is not read from a PK3.
/// `RE_RegisterModels_GetDiskFile` returns this built-in one-bone GLA instead.
/// Simple Ghoul2 models such as saber hilts reference it from their GLM header.
pub const OPENJK_DEFAULT_GLA_NAME: &str = "*default.gla";

// Exact `FakeGLAFile` payload from OpenJK `codemp/rd-vanilla/tr_model.cpp`.
// Keeping the canonical bytes lets the normal validated GLA parser/evaluator
// exercise the same data path as disk-backed GLA files.
const OPENJK_DEFAULT_GLA_FILE: [u8; 294] = [
    0x32, 0x4C, 0x47, 0x41, 0x06, 0x00, 0x00, 0x00, 0x2A, 0x64, 0x65, 0x66, 0x61, 0x75, 0x6C, 0x74,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3F, 0x01, 0x00, 0x00, 0x00,
    0x14, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x18, 0x01, 0x00, 0x00, 0x68, 0x00, 0x00, 0x00,
    0x26, 0x01, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x4D, 0x6F, 0x64, 0x56, 0x69, 0x65, 0x77, 0x20,
    0x69, 0x6E, 0x74, 0x65, 0x72, 0x6E, 0x61, 0x6C, 0x20, 0x64, 0x65, 0x66, 0x61, 0x75, 0x6C, 0x74,
    0x00, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD,
    0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD,
    0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0xCD, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF,
    0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFD, 0xBF, 0xFE, 0x7F, 0xFE, 0x7F, 0xFE, 0x7F,
    0x00, 0x80, 0x00, 0x80, 0x00, 0x80,
];

#[derive(Debug, Clone)]
pub struct GlmModel {
    pub name: String,
    /// OpenJK stores this without the `.gla` suffix and registers `<anim_name>.gla`.
    pub anim_name: String,
    /// Required animation-skeleton size after legacy humanoid remapping.
    /// A Jedi Outcast 72-bone humanoid GLM is normalized to 53 bones.
    pub num_bones: usize,
    pub hierarchy: Vec<GlmSurfaceHierarchy>,
    pub lods: Vec<GlmLod>,
}

#[derive(Debug, Clone)]
pub struct GlmSurfaceHierarchy {
    pub name: String,
    pub flags: u32,
    pub shader: String,
    pub parent_index: i32,
    pub children: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct GlmLod {
    pub surfaces: Vec<GlmSurface>,
}

#[derive(Debug, Clone)]
pub struct GlmSurface {
    pub surface_index: usize,
    pub vertices: Vec<GlmVertex>,
    pub texcoords: Vec<[f32; 2]>,
    pub triangles: Vec<[u32; 3]>,
    /// Maps the packed per-vertex local bone indices to GLA skeleton bone indices.
    pub bone_references: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct GlmVertex {
    pub normal: [f32; 3],
    pub position: [f32; 3],
    pub weights: Vec<GlmWeight>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlmWeight {
    /// Index into `GlmSurface::bone_references`, exactly as stored by Ghoul2.
    pub local_bone_index: usize,
    pub weight: f32,
}

#[derive(Debug, Clone)]
pub struct GlaAnimation {
    pub name: String,
    pub scale: f32,
    pub num_frames: usize,
    pub skeleton: Vec<GlaBone>,
    frame_indices: Vec<u32>,
    compressed_bones: Vec<[u8; MDXA_COMP_QUAT_BONE_SIZE]>,
}

#[derive(Debug, Clone)]
pub struct GlaBone {
    pub name: String,
    pub flags: u32,
    pub parent: i32,
    pub base_pose: Matrix3x4,
    pub base_pose_inv: Matrix3x4,
    pub children: Vec<usize>,
}

fn checked_end(start: usize, len: usize, limit: usize, what: &str) -> Result<usize, String> {
    start
        .checked_add(len)
        .filter(|&end| end <= limit)
        .ok_or_else(|| format!("Ghoul2 {what} outside file"))
}

fn bytes_at<'a>(data: &'a [u8], offset: usize, len: usize, what: &str) -> Result<&'a [u8], String> {
    let end = checked_end(offset, len, data.len(), what)?;
    Ok(&data[offset..end])
}

fn u32_at(data: &[u8], offset: usize, what: &str) -> Result<u32, String> {
    let b = bytes_at(data, offset, 4, what)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn i32_at(data: &[u8], offset: usize, what: &str) -> Result<i32, String> {
    Ok(u32_at(data, offset, what)? as i32)
}

fn f32_at(data: &[u8], offset: usize, what: &str) -> Result<f32, String> {
    let value = f32::from_bits(u32_at(data, offset, what)?);
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("non-finite Ghoul2 float in {what}"))
    }
}

fn usize_i32(value: i32, what: &str) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| format!("negative Ghoul2 {what}: {value}"))
}

fn bounded_count(value: i32, max: usize, what: &str) -> Result<usize, String> {
    let value = usize_i32(value, what)?;
    if value > max {
        return Err(format!("Ghoul2 {what} {value} exceeds safety limit {max}"));
    }
    Ok(value)
}

fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).replace('\\', "/")
}

fn matrix_at(data: &[u8], offset: usize, what: &str) -> Result<Matrix3x4, String> {
    let mut out = [[0.0; 4]; 3];
    for row in 0..3 {
        for col in 0..4 {
            out[row][col] = f32_at(data, offset + (row * 4 + col) * 4, what)?;
        }
    }
    Ok(out)
}

fn add_relative(base: usize, relative: i32, limit: usize, what: &str) -> Result<usize, String> {
    let relative = usize_i32(relative, what)?;
    base.checked_add(relative)
        .filter(|&value| value <= limit)
        .ok_or_else(|| format!("Ghoul2 {what} offset outside file"))
}

/// Read only the GLM animation-skeleton reference from the MDXM header.
pub fn glm_animation_name(data: &[u8]) -> Result<String, String> {
    if data.len() < MDXM_HEADER_SIZE || &data[0..4] != b"2LGM" {
        return Err("expected Ghoul2 GLM/MDXM file".into());
    }
    if i32_at(data, 4, "GLM version")? != MDXM_VERSION {
        return Err(format!("expected GLM version {MDXM_VERSION}"));
    }
    Ok(cstr(bytes_at(data, 72, 64, "GLM animation name")?))
}

/// Parse a Ghoul2 GLM/MDXM version 6 mesh, remapping legacy Jedi Outcast
/// humanoid bone references as OpenJK's `R_LoadMDXM` does.
pub fn parse_glm(data: &[u8]) -> Result<GlmModel, String> {
    if data.len() < MDXM_HEADER_SIZE || &data[0..4] != b"2LGM" {
        return Err("expected Ghoul2 GLM/MDXM file".into());
    }
    if i32_at(data, 4, "GLM version")? != MDXM_VERSION {
        return Err(format!("expected GLM version {MDXM_VERSION}"));
    }

    let name = cstr(bytes_at(data, 8, 64, "GLM name")?);
    let anim_name = glm_animation_name(data)?;
    let num_bones = bounded_count(i32_at(data, 140, "GLM bone count")?, 4096, "GLM bone count")?;
    let legacy_humanoid =
        num_bones == JK2_TO_JKA_HUMANOID_BONES.len() && anim_name.contains("_humanoid");
    let num_lods = bounded_count(i32_at(data, 144, "GLM LOD count")?, 64, "GLM LOD count")?;
    let ofs_lods = usize_i32(i32_at(data, 148, "GLM LOD offset")?, "GLM LOD offset")?;
    let num_surfaces = bounded_count(
        i32_at(data, 152, "GLM surface count")?,
        16_384,
        "GLM surface count",
    )?;
    let ofs_hierarchy = usize_i32(
        i32_at(data, 156, "GLM hierarchy offset")?,
        "GLM hierarchy offset",
    )?;
    let ofs_end = usize_i32(i32_at(data, 160, "GLM end offset")?, "GLM end offset")?;
    if ofs_end < MDXM_HEADER_SIZE || ofs_end > data.len() {
        return Err("GLM end offset outside file".into());
    }
    let data = &data[..ofs_end];
    if ofs_lods < MDXM_HEADER_SIZE || ofs_lods >= data.len() {
        return Err("GLM LOD offset outside file".into());
    }
    if ofs_hierarchy < MDXM_HEADER_SIZE || ofs_hierarchy >= data.len() {
        return Err("GLM hierarchy offset outside file".into());
    }

    // OpenJK walks hierarchy records consecutively from ofsSurfHierarchy. The offset table
    // directly after mdxmHeader_t exists for random access but is not needed to decode them.
    checked_end(
        MDXM_HEADER_SIZE,
        num_surfaces.checked_mul(4).ok_or("GLM hierarchy offset table overflow")?,
        data.len(),
        "GLM hierarchy offset table",
    )?;
    let mut hierarchy = Vec::with_capacity(num_surfaces);
    let mut cursor = ofs_hierarchy;
    for surface_index in 0..num_surfaces {
        checked_end(cursor, MDXM_HIERARCHY_PREFIX, data.len(), "GLM surface hierarchy")?;
        let surface_name = cstr(bytes_at(data, cursor, 64, "GLM hierarchy name")?);
        let flags = u32_at(data, cursor + 64, "GLM hierarchy flags")?;
        let shader = cstr(bytes_at(data, cursor + 68, 64, "GLM hierarchy shader")?);
        let parent_index = i32_at(data, cursor + 136, "GLM hierarchy parent")?;
        if parent_index < -1 || parent_index >= num_surfaces as i32 {
            return Err(format!(
                "GLM hierarchy surface {surface_index} has invalid parent {parent_index}"
            ));
        }
        let child_count = bounded_count(
            i32_at(data, cursor + 140, "GLM child count")?,
            num_surfaces,
            "GLM child count",
        )?;
        let children_bytes = child_count.checked_mul(4).ok_or("GLM child list overflow")?;
        let record_end = checked_end(
            cursor,
            MDXM_HIERARCHY_PREFIX + children_bytes,
            data.len(),
            "GLM child list",
        )?;
        let mut children = Vec::with_capacity(child_count);
        for child in 0..child_count {
            let index = usize_i32(
                i32_at(data, cursor + MDXM_HIERARCHY_PREFIX + child * 4, "GLM child index")?,
                "GLM child index",
            )?;
            if index >= num_surfaces {
                return Err(format!(
                    "GLM hierarchy surface {surface_index} has invalid child {index}"
                ));
            }
            children.push(index);
        }
        hierarchy.push(GlmSurfaceHierarchy {
            name: surface_name,
            flags,
            shader,
            parent_index,
            children,
        });
        cursor = record_end;
    }

    let mut lods = Vec::with_capacity(num_lods);
    let mut lod_cursor = ofs_lods;
    for lod_index in 0..num_lods {
        checked_end(lod_cursor, 4, data.len(), "GLM LOD header")?;
        let lod_size = usize_i32(i32_at(data, lod_cursor, "GLM LOD end")?, "GLM LOD end")?;
        if lod_size < 4 + num_surfaces * 4 {
            return Err(format!("GLM LOD {lod_index} is smaller than its offset table"));
        }
        let lod_end = checked_end(lod_cursor, lod_size, data.len(), "GLM LOD")?;
        let offsets_start = lod_cursor + 4;
        checked_end(offsets_start, num_surfaces * 4, lod_end, "GLM LOD surface offsets")?;

        // OpenJK's loader walks surfaces consecutively immediately after the offset table.
        let mut surface_cursor = offsets_start + num_surfaces * 4;
        let mut surfaces = Vec::with_capacity(num_surfaces);
        for file_order in 0..num_surfaces {
            checked_end(
                surface_cursor,
                MDXM_SURFACE_SIZE,
                lod_end,
                "GLM surface header",
            )?;
            let surface_index = usize_i32(
                i32_at(data, surface_cursor + 4, "GLM surface index")?,
                "GLM surface index",
            )?;
            if surface_index >= num_surfaces {
                return Err(format!(
                    "GLM LOD {lod_index} surface {file_order} has invalid surface index {surface_index}"
                ));
            }
            // OpenJK G2_FindSurface starts at `(byte *)lod + sizeof(mdxmLOD_t)`,
            // i.e. the beginning of mdxmLODSurfOffset_t, and adds offsets[index]
            // from there.  The offsets are therefore relative to the surface-offset
            // table, not to mdxmLOD_t itself.  (For surface 0 this is commonly the
            // size of the complete offset array.)
            let table_offset = usize_i32(
                i32_at(data, offsets_start + surface_index * 4, "GLM surface table offset")?,
                "GLM surface table offset",
            )?;
            if offsets_start.checked_add(table_offset) != Some(surface_cursor) {
                return Err(format!(
                    "GLM LOD {lod_index} surface {surface_index} offset table disagrees with file layout"
                ));
            }

            let num_verts = bounded_count(
                i32_at(data, surface_cursor + 12, "GLM vertex count")?,
                1_000_000,
                "GLM vertex count",
            )?;
            let ofs_verts = i32_at(data, surface_cursor + 16, "GLM vertex offset")?;
            let num_triangles = bounded_count(
                i32_at(data, surface_cursor + 20, "GLM triangle count")?,
                2_000_000,
                "GLM triangle count",
            )?;
            let ofs_triangles = i32_at(data, surface_cursor + 24, "GLM triangle offset")?;
            let num_bone_refs = bounded_count(
                i32_at(data, surface_cursor + 28, "GLM bone reference count")?,
                32,
                "GLM bone reference count",
            )?;
            let ofs_bone_refs = i32_at(data, surface_cursor + 32, "GLM bone reference offset")?;
            let surface_size = usize_i32(
                i32_at(data, surface_cursor + 36, "GLM surface end")?,
                "GLM surface end",
            )?;
            if surface_size < MDXM_SURFACE_SIZE {
                return Err(format!("GLM LOD {lod_index} surface {surface_index} has invalid size"));
            }
            let surface_end = checked_end(surface_cursor, surface_size, lod_end, "GLM surface")?;

            let verts_start = add_relative(surface_cursor, ofs_verts, surface_end, "GLM vertices")?;
            let verts_bytes = num_verts
                .checked_mul(MDXM_VERTEX_SIZE)
                .ok_or("GLM vertex array overflow")?;
            let texcoords_start = checked_end(verts_start, verts_bytes, surface_end, "GLM vertices")?;
            checked_end(
                texcoords_start,
                num_verts.checked_mul(8).ok_or("GLM texcoord array overflow")?,
                surface_end,
                "GLM texcoords",
            )?;

            let triangle_start = add_relative(
                surface_cursor,
                ofs_triangles,
                surface_end,
                "GLM triangles",
            )?;
            checked_end(
                triangle_start,
                num_triangles
                    .checked_mul(12)
                    .ok_or("GLM triangle array overflow")?,
                surface_end,
                "GLM triangles",
            )?;

            let bone_refs_start = add_relative(
                surface_cursor,
                ofs_bone_refs,
                surface_end,
                "GLM bone references",
            )?;
            checked_end(
                bone_refs_start,
                num_bone_refs
                    .checked_mul(4)
                    .ok_or("GLM bone reference array overflow")?,
                surface_end,
                "GLM bone references",
            )?;

            let mut bone_references = Vec::with_capacity(num_bone_refs);
            for bone_ref in 0..num_bone_refs {
                let bone = usize_i32(
                    i32_at(data, bone_refs_start + bone_ref * 4, "GLM bone reference")?,
                    "GLM bone reference",
                )?;
                if bone >= num_bones {
                    return Err(format!(
                        "GLM LOD {lod_index} surface {surface_index} references bone {bone}, but mesh declares {num_bones}"
                    ));
                }
                // Validate against the authored count before remapping. Vertex
                // weights still index this local table, so neither their indices
                // nor their weights change, including when two bones collapse.
                bone_references.push(if legacy_humanoid {
                    JK2_TO_JKA_HUMANOID_BONES[bone]
                } else {
                    bone
                });
            }

            let mut vertices = Vec::with_capacity(num_verts);
            let mut texcoords = Vec::with_capacity(num_verts);
            for vertex_index in 0..num_verts {
                let vertex_offset = verts_start + vertex_index * MDXM_VERTEX_SIZE;
                let normal = [
                    f32_at(data, vertex_offset, "GLM vertex normal")?,
                    f32_at(data, vertex_offset + 4, "GLM vertex normal")?,
                    f32_at(data, vertex_offset + 8, "GLM vertex normal")?,
                ];
                let position = [
                    f32_at(data, vertex_offset + 12, "GLM vertex position")?,
                    f32_at(data, vertex_offset + 16, "GLM vertex position")?,
                    f32_at(data, vertex_offset + 20, "GLM vertex position")?,
                ];
                let packed = u32_at(data, vertex_offset + 24, "GLM packed weights")?;
                let raw_weights = bytes_at(data, vertex_offset + 28, 4, "GLM weights")?;
                let weight_count = ((packed >> 30) + 1) as usize;
                if !(1..=MAX_G2_BONEWEIGHTS_PER_VERT).contains(&weight_count) {
                    return Err("invalid GLM vertex weight count".into());
                }
                let mut total_weight = 0.0f32;
                let mut weights = Vec::with_capacity(weight_count);
                for weight_index in 0..weight_count {
                    let local_bone_index = ((packed
                        >> (G2_BITS_PER_BONEREF * weight_index as u32))
                        & ((1 << G2_BITS_PER_BONEREF) - 1))
                        as usize;
                    if local_bone_index >= bone_references.len() {
                        return Err(format!(
                            "GLM LOD {lod_index} surface {surface_index} vertex {vertex_index} weight {weight_index} references local bone {local_bone_index}, but surface has {} bone references",
                            bone_references.len()
                        ));
                    }
                    let weight = if weight_index == weight_count - 1 {
                        1.0 - total_weight
                    } else {
                        let mut quantized = raw_weights[weight_index] as u32;
                        quantized |= (packed
                            >> (G2_BONEWEIGHT_TOPBITS_SHIFT + weight_index as u32 * 2))
                            & G2_BONEWEIGHT_TOPBITS_AND;
                        let value = quantized as f32 * G2_BONEWEIGHT_RECIPROCAL_MULT;
                        total_weight += value;
                        value
                    };
                    // OpenJK uses the residual for the final weight. Allow tiny quantization noise,
                    // but reject a genuinely invalid negative result.
                    if !weight.is_finite() || weight < -0.002 || weight > 1.002 {
                        return Err(format!(
                            "GLM LOD {lod_index} surface {surface_index} vertex {vertex_index} has invalid weight {weight}"
                        ));
                    }
                    weights.push(GlmWeight {
                        local_bone_index,
                        weight,
                    });
                }
                vertices.push(GlmVertex {
                    normal,
                    position,
                    weights,
                });
                let texcoord_offset = texcoords_start + vertex_index * 8;
                texcoords.push([
                    f32_at(data, texcoord_offset, "GLM texcoord")?,
                    f32_at(data, texcoord_offset + 4, "GLM texcoord")?,
                ]);
            }

            let mut triangles = Vec::with_capacity(num_triangles);
            for triangle_index in 0..num_triangles {
                let offset = triangle_start + triangle_index * 12;
                let triangle = [
                    u32_at(data, offset, "GLM triangle index")?,
                    u32_at(data, offset + 4, "GLM triangle index")?,
                    u32_at(data, offset + 8, "GLM triangle index")?,
                ];
                if triangle.iter().any(|&index| index as usize >= num_verts) {
                    return Err(format!(
                        "GLM LOD {lod_index} surface {surface_index} triangle {triangle_index} has out-of-range vertex"
                    ));
                }
                triangles.push(triangle);
            }

            surfaces.push(GlmSurface {
                surface_index,
                vertices,
                texcoords,
                triangles,
                bone_references,
            });
            surface_cursor = surface_end;
        }
        if surface_cursor > lod_end {
            return Err(format!("GLM LOD {lod_index} surface data overruns LOD"));
        }
        lods.push(GlmLod { surfaces });
        lod_cursor = lod_end;
    }

    Ok(GlmModel {
        name,
        anim_name,
        num_bones: if legacy_humanoid {
            JKA_HUMANOID_BONE_COUNT
        } else {
            num_bones
        },
        hierarchy,
        lods,
    })
}

/// Parse a Ghoul2 GLA/MDXA version 6 animation/skeleton file.
pub fn parse_gla(data: &[u8]) -> Result<GlaAnimation, String> {
    if data.len() < MDXA_HEADER_SIZE || &data[0..4] != b"2LGA" {
        return Err("expected Ghoul2 GLA/MDXA file".into());
    }
    if i32_at(data, 4, "GLA version")? != MDXA_VERSION {
        return Err(format!("expected GLA version {MDXA_VERSION}"));
    }

    let name = cstr(bytes_at(data, 8, 64, "GLA name")?);
    let scale = f32_at(data, 72, "GLA scale")?;
    let num_frames = bounded_count(i32_at(data, 76, "GLA frame count")?, 1_000_000, "GLA frame count")?;
    if num_frames == 0 {
        return Err("GLA has no frames".into());
    }
    let ofs_frames = usize_i32(i32_at(data, 80, "GLA frame offset")?, "GLA frame offset")?;
    let num_bones = bounded_count(i32_at(data, 84, "GLA bone count")?, 4096, "GLA bone count")?;
    if num_bones == 0 {
        return Err("GLA has no bones".into());
    }
    let ofs_comp_pool = usize_i32(
        i32_at(data, 88, "GLA compressed bone pool offset")?,
        "GLA compressed bone pool offset",
    )?;
    let ofs_skel = usize_i32(i32_at(data, 92, "GLA skeleton offset")?, "GLA skeleton offset")?;
    let ofs_end = usize_i32(i32_at(data, 96, "GLA end offset")?, "GLA end offset")?;
    if ofs_end < MDXA_HEADER_SIZE || ofs_end > data.len() {
        return Err("GLA end offset outside file".into());
    }
    let data = &data[..ofs_end];

    let offsets_bytes = num_bones.checked_mul(4).ok_or("GLA skeleton offset table overflow")?;
    checked_end(
        MDXA_HEADER_SIZE,
        offsets_bytes,
        data.len(),
        "GLA skeleton offset table",
    )?;
    if ofs_skel >= data.len() || ofs_frames >= data.len() || ofs_comp_pool >= data.len() {
        return Err("GLA section offset outside file".into());
    }

    // OpenJK addresses each bone as header + sizeof(mdxaHeader_t) + offsets[i].
    let mut skeleton = Vec::with_capacity(num_bones);
    for bone_index in 0..num_bones {
        let relative = usize_i32(
            i32_at(data, MDXA_HEADER_SIZE + bone_index * 4, "GLA skeleton record offset")?,
            "GLA skeleton record offset",
        )?;
        let bone_offset = MDXA_HEADER_SIZE
            .checked_add(relative)
            .filter(|&offset| offset < data.len())
            .ok_or("GLA skeleton record offset outside file")?;
        checked_end(bone_offset, MDXA_SKEL_PREFIX, data.len(), "GLA skeleton record")?;

        let bone_name = cstr(bytes_at(data, bone_offset, 64, "GLA bone name")?);
        let flags = u32_at(data, bone_offset + 64, "GLA bone flags")?;
        let parent = i32_at(data, bone_offset + 68, "GLA bone parent")?;
        if parent < -1 || parent >= num_bones as i32 || parent == bone_index as i32 {
            return Err(format!("GLA bone {bone_index} has invalid parent {parent}"));
        }
        let base_pose = matrix_at(data, bone_offset + 72, "GLA base pose")?;
        let base_pose_inv = matrix_at(data, bone_offset + 120, "GLA inverse base pose")?;
        let child_count = bounded_count(
            i32_at(data, bone_offset + 168, "GLA child count")?,
            num_bones,
            "GLA child count",
        )?;
        checked_end(
            bone_offset,
            MDXA_SKEL_PREFIX + child_count * 4,
            data.len(),
            "GLA children",
        )?;
        let mut children = Vec::with_capacity(child_count);
        for child_index in 0..child_count {
            let child = usize_i32(
                i32_at(data, bone_offset + MDXA_SKEL_PREFIX + child_index * 4, "GLA child")?,
                "GLA child",
            )?;
            if child >= num_bones || child == bone_index {
                return Err(format!("GLA bone {bone_index} has invalid child {child}"));
            }
            children.push(child);
        }
        skeleton.push(GlaBone {
            name: bone_name,
            flags,
            parent,
            base_pose,
            base_pose_inv,
            children,
        });
    }

    let index_count = num_frames
        .checked_mul(num_bones)
        .ok_or("GLA frame/bone index count overflow")?;
    let index_bytes = index_count.checked_mul(3).ok_or("GLA frame index table overflow")?;
    checked_end(ofs_frames, index_bytes, data.len(), "GLA frame indices")?;
    let mut frame_indices = Vec::with_capacity(index_count);
    let mut max_index = 0u32;
    for index in 0..index_count {
        let offset = ofs_frames + index * 3;
        let value = data[offset] as u32
            | ((data[offset + 1] as u32) << 8)
            | ((data[offset + 2] as u32) << 16);
        max_index = max_index.max(value);
        frame_indices.push(value);
    }

    // OpenJK does not store a pool count. It scans all frame indices and swaps entries
    // 0..=maxIndex; therefore that exact required range must be present.
    let pool_count = max_index as usize + 1;
    checked_end(
        ofs_comp_pool,
        pool_count
            .checked_mul(MDXA_COMP_QUAT_BONE_SIZE)
            .ok_or("GLA compressed bone pool overflow")?,
        data.len(),
        "GLA compressed bone pool",
    )?;
    let mut compressed_bones = Vec::with_capacity(pool_count);
    for pool_index in 0..pool_count {
        let offset = ofs_comp_pool + pool_index * MDXA_COMP_QUAT_BONE_SIZE;
        let mut compressed = [0u8; MDXA_COMP_QUAT_BONE_SIZE];
        compressed.copy_from_slice(bytes_at(
            data,
            offset,
            MDXA_COMP_QUAT_BONE_SIZE,
            "GLA compressed bone",
        )?);
        compressed_bones.push(compressed);
    }

    Ok(GlaAnimation {
        name,
        scale,
        num_frames,
        skeleton,
        frame_indices,
        compressed_bones,
    })
}


/// Parse the exact synthetic one-bone GLA OpenJK supplies for `*default.gla`.
/// This name is intentionally virtual and must never be looked up in the PK3 VFS.
pub fn openjk_default_gla() -> Result<GlaAnimation, String> {
    parse_gla(&OPENJK_DEFAULT_GLA_FILE)
}


// Ghoul2 bone flags from OpenJK `ghoul2/G2.h`.
pub const BONE_ANGLES_POSTMULT: u32 = 0x0002;

// Ghoul2 bone-animation flags from OpenJK `ghoul2/G2.h`.
pub const BONE_ANIM_OVERRIDE: u32 = 0x0008;
pub const BONE_ANIM_OVERRIDE_LOOP: u32 = 0x0010;
pub const BONE_ANIM_OVERRIDE_FREEZE: u32 = 0x0040 + BONE_ANIM_OVERRIDE;
pub const BONE_ANIM_BLEND: u32 = 0x0080;
pub const BONE_ANIM_TOTAL: u32 =
    BONE_ANIM_OVERRIDE | BONE_ANIM_OVERRIDE_LOOP | BONE_ANIM_OVERRIDE_FREEZE | BONE_ANIM_BLEND;

/// Root matrix used by OpenJK Ghoul2 when no GHOUL2_NEWORIGIN override is active.
/// This is intentionally not a conventional identity matrix; it is the exact
/// `identityMatrix` from `tr_ghoul2.cpp`.
pub const OPENJK_GHOUL2_ROOT_MATRIX: Matrix3x4 = [
    [0.0, -1.0, 0.0, 0.0],
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghoul2BoneAnim {
    pub bone_index: usize,
    pub start_frame: i32,
    pub end_frame: i32,
    pub flags: u32,
    pub anim_speed: f32,
    pub start_time: i32,
    pub pause_time: i32,
    pub blend_frame: f32,
    pub blend_lerp_frame: i32,
    pub blend_time: i32,
    pub blend_start: i32,
}

impl Ghoul2BoneAnim {
    fn inactive(bone_index: usize) -> Self {
        Self {
            bone_index,
            start_frame: 0,
            end_frame: 1,
            flags: 0,
            anim_speed: 0.0,
            start_time: 0,
            pause_time: 0,
            blend_frame: 0.0,
            blend_lerp_frame: 0,
            blend_time: 0,
            blend_start: 0,
        }
    }

    pub fn is_animating(&self) -> bool {
        self.flags & (BONE_ANIM_OVERRIDE_LOOP | BONE_ANIM_OVERRIDE) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghoul2BoneFrame {
    pub current_frame: i32,
    pub new_frame: i32,
    /// OpenJK calls this `lerp` in `G2_TimingModel` and stores it as
    /// `SBoneCalc::backlerp`; the final local matrix is
    /// `backlerp * new + (1-backlerp) * current`.
    pub backlerp: f32,
}

impl Default for Ghoul2BoneFrame {
    fn default() -> Self {
        Self {
            current_frame: 0,
            new_frame: 0,
            backlerp: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Ghoul2BoneCalc {
    frame: Ghoul2BoneFrame,
    blend_frame: f32,
    blend_old_frame: i32,
    blend_mode: bool,
    blend_lerp: f32,
}

impl Default for Ghoul2BoneCalc {
    fn default() -> Self {
        Self {
            frame: Ghoul2BoneFrame::default(),
            blend_frame: 0.0,
            blend_old_frame: 0,
            blend_mode: false,
            blend_lerp: 0.0,
        }
    }
}

/// CPU result of OpenJK `RB_SurfaceGhoul` for one GLM surface.
///
/// This is deliberately renderer-neutral: it preserves JKA model-space coordinates and UVs.
/// Entity/world transforms and material selection belong to the cgame/renderer presentation layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghoul2SkinnedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ghoul2SkinnedSurface {
    pub surface_index: usize,
    pub vertices: Vec<Ghoul2SkinnedVertex>,
    pub indices: Vec<u32>,
}

/// Port of the active (non-`JK2_MODE`) vertex deformation in OpenJK `RB_SurfaceGhoul`.
///
/// Important fidelity detail: the stock fast-normal path transforms `v->normal` with only the
/// first bone matrix even when position has multiple weights. Positions use the packed GLM
/// weights, with the final weight represented as the residual `1 - sum(previous)` exactly as
/// decoded by [`parse_glm`].
pub fn skin_glm_surface(
    surface: &GlmSurface,
    bone_matrices: &[Matrix3x4],
) -> Result<Ghoul2SkinnedSurface, String> {
    if surface.vertices.len() != surface.texcoords.len() {
        return Err(format!(
            "GLM surface {} has {} vertices but {} texcoords",
            surface.surface_index,
            surface.vertices.len(),
            surface.texcoords.len()
        ));
    }

    let mut vertices = Vec::with_capacity(surface.vertices.len());
    for (vertex_index, (vertex, &uv)) in surface
        .vertices
        .iter()
        .zip(surface.texcoords.iter())
        .enumerate()
    {
        let first_weight = vertex.weights.first().ok_or_else(|| {
            format!(
                "GLM surface {} vertex {} has no bone weights",
                surface.surface_index, vertex_index
            )
        })?;
        let first_bone = surface_bone_matrix(
            surface,
            bone_matrices,
            first_weight.local_bone_index,
            vertex_index,
        )?;

        // Active OpenJK RB_SurfaceGhoul fast-normal path (tr_ghoul2.cpp):
        // normals use EvalRender() for weight 0 only, even for multi-weight vertices.
        let normal = transform_normal(first_bone, vertex.normal);

        let position = match vertex.weights.as_slice() {
            [only] => {
                let bone = surface_bone_matrix(
                    surface,
                    bone_matrices,
                    only.local_bone_index,
                    vertex_index,
                )?;
                transform_point(bone, vertex.position)
            }
            [first, second] => {
                // OpenJK special-cases two weights as w * (first - second) + second.
                let first_matrix = surface_bone_matrix(
                    surface,
                    bone_matrices,
                    first.local_bone_index,
                    vertex_index,
                )?;
                let second_matrix = surface_bone_matrix(
                    surface,
                    bone_matrices,
                    second.local_bone_index,
                    vertex_index,
                )?;
                let a = transform_point(first_matrix, vertex.position);
                let b = transform_point(second_matrix, vertex.position);
                [
                    first.weight * (a[0] - b[0]) + b[0],
                    first.weight * (a[1] - b[1]) + b[1],
                    first.weight * (a[2] - b[2]) + b[2],
                ]
            }
            weights => {
                // OpenJK accumulates every stored weight except the last and gives the final
                // bone the exact residual. parse_glm already records that same residual, but
                // recompute it here to retain RB_SurfaceGhoul's arithmetic/control flow.
                let mut out = [0.0f32; 3];
                let mut total_weight = 0.0f32;
                for weight in &weights[..weights.len() - 1] {
                    let bone = surface_bone_matrix(
                        surface,
                        bone_matrices,
                        weight.local_bone_index,
                        vertex_index,
                    )?;
                    let point = transform_point(bone, vertex.position);
                    out[0] += weight.weight * point[0];
                    out[1] += weight.weight * point[1];
                    out[2] += weight.weight * point[2];
                    total_weight += weight.weight;
                }
                let last = weights.last().expect("multi-weight slice is non-empty");
                let bone = surface_bone_matrix(
                    surface,
                    bone_matrices,
                    last.local_bone_index,
                    vertex_index,
                )?;
                let point = transform_point(bone, vertex.position);
                let last_weight = 1.0 - total_weight;
                out[0] += last_weight * point[0];
                out[1] += last_weight * point[1];
                out[2] += last_weight * point[2];
                out
            }
        };

        vertices.push(Ghoul2SkinnedVertex {
            position,
            normal,
            uv,
        });
    }

    let mut indices = Vec::with_capacity(surface.triangles.len() * 3);
    for (triangle_index, triangle) in surface.triangles.iter().enumerate() {
        if triangle
            .iter()
            .any(|&index| index as usize >= surface.vertices.len())
        {
            return Err(format!(
                "GLM surface {} triangle {} has out-of-range vertex",
                surface.surface_index, triangle_index
            ));
        }
        indices.extend_from_slice(triangle);
    }

    Ok(Ghoul2SkinnedSurface {
        surface_index: surface.surface_index,
        vertices,
        indices,
    })
}

fn surface_bone_matrix<'a>(
    surface: &GlmSurface,
    bone_matrices: &'a [Matrix3x4],
    local_bone_index: usize,
    vertex_index: usize,
) -> Result<&'a Matrix3x4, String> {
    let skeleton_bone = *surface.bone_references.get(local_bone_index).ok_or_else(|| {
        format!(
            "GLM surface {} vertex {} local bone {} outside {} bone references",
            surface.surface_index,
            vertex_index,
            local_bone_index,
            surface.bone_references.len()
        )
    })?;
    bone_matrices.get(skeleton_bone).ok_or_else(|| {
        format!(
            "GLM surface {} vertex {} skeleton bone {} outside pose of {} bones",
            surface.surface_index,
            vertex_index,
            skeleton_bone,
            bone_matrices.len()
        )
    })
}


/// Ghoul2 animation + POSTMULT player-angle override state matching the active
/// OpenJK `boneInfo_v` paths used by `G2_Set_Bone_Anim*`, `G2_TimingModel`, and
/// `BG_G2PlayerAngles`. Ragdoll/IK remain separate presentation layers.
#[derive(Debug, Clone)]
pub struct Ghoul2Animator {
    bone_anims: Vec<Ghoul2BoneAnim>,
    bone_angle_postmult: Vec<Option<Matrix3x4>>,
}

impl Ghoul2Animator {
    pub fn new(gla: &GlaAnimation) -> Self {
        Self {
            bone_anims: (0..gla.num_bones()).map(Ghoul2BoneAnim::inactive).collect(),
            bone_angle_postmult: vec![None; gla.num_bones()],
        }
    }

    pub fn bone_index(gla: &GlaAnimation, bone_name: &str) -> Option<usize> {
        gla.skeleton
            .iter()
            .position(|bone| bone.name.eq_ignore_ascii_case(bone_name))
    }

    pub fn bone_anim(&self, bone_index: usize) -> Option<&Ghoul2BoneAnim> {
        self.bone_anims.get(bone_index)
    }


    /// Port of the `BONE_ANGLES_POSTMULT` path in OpenJK
    /// `G2_Set_Bone_Angles` / `G2_Generate_Matrix`. `blendTime` is zero for
    /// `BG_G2PlayerAngles`, so no angle-blend state is required here.
    pub fn set_bone_angles_postmult(
        &mut self,
        gla: &GlaAnimation,
        bone_name: &str,
        angles: [f32; 3],
        up: i32,
        left: i32,
        forward: i32,
    ) -> Result<(), String> {
        let bone_index = Self::bone_index(gla, bone_name)
            .ok_or_else(|| format!("Ghoul2 bone {bone_name:?} not found"))?;
        let generated = g2_generate_postmult_matrix(
            angles,
            up,
            left,
            forward,
            &gla.skeleton[bone_index].base_pose,
            &gla.skeleton[bone_index].base_pose_inv,
        )?;
        self.bone_angle_postmult[bone_index] = Some(generated);
        Ok(())
    }

    pub fn clear_bone_angle_overrides(&mut self) {
        self.bone_angle_postmult.fill(None);
    }

    /// Port of OpenJK `G2_Get_Bone_Anim_Index` for the frame value used by
    /// cgame when resuming an animation at a different speed.
    pub fn bone_frame(
        &mut self,
        gla: &GlaAnimation,
        bone_index: usize,
        current_time: i32,
    ) -> Result<Option<f32>, String> {
        let num_frames = i32::try_from(gla.num_frames).map_err(|_| "GLA frame count exceeds i32")?;
        let bone = self
            .bone_anims
            .get_mut(bone_index)
            .ok_or_else(|| format!("Ghoul2 bone index {bone_index} out of range"))?;
        if !bone.is_animating() {
            return Ok(None);
        }
        let mut frame = Ghoul2BoneFrame::default();
        g2_timing_model(bone, current_time, num_frames, &mut frame)?;
        if !bone.is_animating() {
            return Ok(None);
        }
        Ok(Some(frame.current_frame as f32 + frame.backlerp))
    }

    /// Port of `G2_Set_Bone_Anim_Index` with the stock blend multiplier of 1.0.
    /// `set_frame=None` is OpenJK's `-1` sentinel.
    pub fn set_bone_anim_index(
        &mut self,
        gla: &GlaAnimation,
        bone_index: usize,
        start_frame: i32,
        end_frame: i32,
        flags: u32,
        anim_speed: f32,
        current_time: i32,
        set_frame: Option<f32>,
        blend_time: i32,
    ) -> Result<(), String> {
        let num_frames = i32::try_from(gla.num_frames).map_err(|_| "GLA frame count exceeds i32")?;
        if bone_index >= self.bone_anims.len() {
            return Err(format!("Ghoul2 bone index {bone_index} out of range"));
        }
        validate_anim_range(start_frame, end_frame, num_frames)?;
        if !anim_speed.is_finite() {
            return Err("Ghoul2 bone animation speed is non-finite".into());
        }
        if let Some(set_frame) = set_frame {
            if !set_frame.is_finite() {
                return Err("Ghoul2 setFrame is non-finite".into());
            }
            let valid = (set_frame >= start_frame as f32 && set_frame < end_frame as f32)
                || (set_frame > end_frame as f32 && set_frame <= (start_frame + 1) as f32);
            if !valid {
                return Err(format!(
                    "Ghoul2 setFrame {set_frame} outside animation range {start_frame}..{end_frame}"
                ));
            }
            if anim_speed == 0.0 {
                return Err("Ghoul2 setFrame cannot be used with zero animation speed".into());
            }
        }

        let mut mod_flags = flags;
        if mod_flags & BONE_ANIM_BLEND != 0 {
            let previous = self.bone_frame(gla, bone_index, current_time)?;
            let bone = &mut self.bone_anims[bone_index];
            if let Some(current_frame) = previous {
                if bone.blend_start == current_time {
                    // Replacing a blend which has not begun yet: OpenJK only changes duration.
                    bone.blend_time = blend_time;
                } else {
                    if bone.anim_speed < 0.0 {
                        let frame = current_frame.floor();
                        bone.blend_frame = frame;
                        bone.blend_lerp_frame = frame as i32;
                    } else {
                        bone.blend_frame = current_frame;
                        bone.blend_lerp_frame = current_frame as i32 + 1;
                        if bone.blend_frame >= bone.end_frame as f32 {
                            bone.blend_frame = if bone.flags & BONE_ANIM_OVERRIDE_LOOP != 0 {
                                bone.start_frame as f32
                            } else {
                                (bone.end_frame - 1) as f32
                            };
                        }
                        if bone.blend_lerp_frame >= bone.end_frame {
                            bone.blend_lerp_frame = if bone.flags & BONE_ANIM_OVERRIDE_LOOP != 0 {
                                bone.start_frame
                            } else {
                                bone.end_frame - 1
                            };
                        }
                    }
                    bone.blend_time = blend_time;
                    bone.blend_start = current_time;
                }
            } else {
                let bone = &mut self.bone_anims[bone_index];
                bone.blend_frame = 0.0;
                bone.blend_lerp_frame = 0;
                bone.blend_time = 0;
                mod_flags &= !BONE_ANIM_BLEND;
            }
        } else {
            let bone = &mut self.bone_anims[bone_index];
            bone.blend_frame = 0.0;
            bone.blend_lerp_frame = 0;
            bone.blend_time = 0;
            bone.blend_start = 0;
            mod_flags &= !BONE_ANIM_BLEND;
        }

        let bone = &mut self.bone_anims[bone_index];
        bone.end_frame = end_frame;
        bone.start_frame = start_frame;
        bone.anim_speed = anim_speed;
        bone.pause_time = 0;
        bone.start_time = if let Some(set_frame) = set_frame {
            (current_time as f32 - (((set_frame - start_frame as f32) * 50.0) / anim_speed)) as i32
        } else {
            current_time
        };
        bone.flags &= !BONE_ANIM_TOTAL;
        bone.flags |= mod_flags;

        validate_frame_index(bone.blend_frame as i32, num_frames, "blendFrame")?;
        validate_frame_index(bone.blend_lerp_frame, num_frames, "blendLerpFrame")?;
        Ok(())
    }

    pub fn set_bone_anim(
        &mut self,
        gla: &GlaAnimation,
        bone_name: &str,
        start_frame: i32,
        end_frame: i32,
        flags: u32,
        anim_speed: f32,
        current_time: i32,
        set_frame: Option<f32>,
        blend_time: i32,
    ) -> Result<(), String> {
        let bone_index = Self::bone_index(gla, bone_name)
            .ok_or_else(|| format!("Ghoul2 bone {bone_name:?} not found"))?;
        self.set_bone_anim_index(
            gla,
            bone_index,
            start_frame,
            end_frame,
            flags,
            anim_speed,
            current_time,
            set_frame,
            blend_time,
        )
    }

    /// Animation-only port of `G2_TransformGhoulBones` / `G2_TransformBone`.
    /// It preserves OpenJK's parent inheritance, matrix-component lerps, and
    /// outgoing-animation blend behavior. Bone-angle overrides, IK, ragdoll,
    /// smoothing, and unsquash are later independent layers in OpenJK and are
    /// intentionally not approximated here.
    pub fn evaluate_pose(
        &mut self,
        gla: &GlaAnimation,
        current_time: i32,
        root_matrix: Matrix3x4,
    ) -> Result<Vec<Matrix3x4>, String> {
        if self.bone_anims.len() != gla.num_bones() {
            return Err("Ghoul2 animator does not match GLA skeleton".into());
        }
        let mut out = vec![[[0.0; 4]; 3]; gla.num_bones()];
        let mut calcs = vec![Ghoul2BoneCalc::default(); gla.num_bones()];
        let mut complete = vec![false; gla.num_bones()];
        let mut visiting = vec![false; gla.num_bones()];
        for bone in 0..gla.num_bones() {
            self.evaluate_bone_recursive(
                gla,
                current_time,
                root_matrix,
                bone,
                &mut out,
                &mut calcs,
                &mut complete,
                &mut visiting,
            )?;
        }
        Ok(out)
    }

    /// Evaluate one Ghoul2 bone plus only the ancestor chain required to
    /// produce it. This mirrors OpenJK's lazy CBoneCache::Eval/EvalLow shape:
    /// requesting a bolt/helper bone does not require materializing every bone
    /// in the skeleton first.
    pub fn evaluate_bone(
        &mut self,
        gla: &GlaAnimation,
        current_time: i32,
        root_matrix: Matrix3x4,
        bone_index: usize,
    ) -> Result<Matrix3x4, String> {
        if self.bone_anims.len() != gla.num_bones() {
            return Err("Ghoul2 animator does not match GLA skeleton".into());
        }
        if bone_index >= gla.num_bones() {
            return Err(format!("Ghoul2 bone index {bone_index} is out of range"));
        }
        let mut out = vec![[[0.0; 4]; 3]; gla.num_bones()];
        let mut calcs = vec![Ghoul2BoneCalc::default(); gla.num_bones()];
        let mut complete = vec![false; gla.num_bones()];
        let mut visiting = vec![false; gla.num_bones()];
        self.evaluate_bone_recursive(
            gla,
            current_time,
            root_matrix,
            bone_index,
            &mut out,
            &mut calcs,
            &mut complete,
            &mut visiting,
        )?;
        Ok(out[bone_index])
    }

    pub fn evaluate_bone_openjk_root(
        &mut self,
        gla: &GlaAnimation,
        current_time: i32,
        bone_index: usize,
    ) -> Result<Matrix3x4, String> {
        self.evaluate_bone(
            gla,
            current_time,
            OPENJK_GHOUL2_ROOT_MATRIX,
            bone_index,
        )
    }

    pub fn evaluate_pose_openjk_root(
        &mut self,
        gla: &GlaAnimation,
        current_time: i32,
    ) -> Result<Vec<Matrix3x4>, String> {
        self.evaluate_pose(gla, current_time, OPENJK_GHOUL2_ROOT_MATRIX)
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_bone_recursive(
        &mut self,
        gla: &GlaAnimation,
        current_time: i32,
        root_matrix: Matrix3x4,
        bone_index: usize,
        out: &mut [Matrix3x4],
        calcs: &mut [Ghoul2BoneCalc],
        complete: &mut [bool],
        visiting: &mut [bool],
    ) -> Result<(), String> {
        if complete[bone_index] {
            return Ok(());
        }
        if visiting[bone_index] {
            return Err(format!("GLA skeleton cycle at bone {bone_index}"));
        }
        visiting[bone_index] = true;

        let parent = gla.skeleton[bone_index].parent;
        let mut calc = if parent >= 0 {
            let parent_index = parent as usize;
            self.evaluate_bone_recursive(
                gla,
                current_time,
                root_matrix,
                parent_index,
                out,
                calcs,
                complete,
                visiting,
            )?;
            // Exact `CBoneCache::EvalLow`: children inherit all animation timing
            // and blend state from the parent before applying a local override.
            calcs[parent_index]
        } else {
            Ghoul2BoneCalc::default()
        };

        let num_frames = i32::try_from(gla.num_frames).map_err(|_| "GLA frame count exceeds i32")?;
        let bone_anim = &mut self.bone_anims[bone_index];
        if bone_anim.flags & BONE_ANIM_BLEND != 0 {
            let elapsed = current_time - bone_anim.blend_start;
            if elapsed >= 0 && elapsed < bone_anim.blend_time {
                calc.blend_frame = bone_anim.blend_frame;
                calc.blend_old_frame = bone_anim.blend_lerp_frame;
                calc.blend_lerp = elapsed as f32 / bone_anim.blend_time as f32;
                calc.blend_mode = true;
            } else {
                calc.blend_mode = false;
            }
        } else if bone_anim.flags & (BONE_ANIM_OVERRIDE_LOOP | BONE_ANIM_OVERRIDE) != 0 {
            calc.blend_mode = false;
        }

        if bone_anim.is_animating() {
            g2_timing_model(bone_anim, current_time, num_frames, &mut calc.frame)?;
        }

        validate_frame_index(calc.frame.new_frame, num_frames, "newFrame")?;
        validate_frame_index(calc.frame.current_frame, num_frames, "currentFrame")?;
        if calc.blend_frame < 0.0 || calc.blend_frame >= (num_frames + 1) as f32 {
            return Err(format!("Ghoul2 blendFrame {} out of range", calc.blend_frame));
        }
        validate_frame_index(calc.blend_old_frame, num_frames, "blendOldFrame")?;

        let mut old_blend_matrix = [[0.0; 4]; 3];
        if calc.blend_mode {
            let old_backlerp = calc.blend_frame - calc.blend_frame.trunc();
            let old_frontlerp = 1.0 - old_backlerp;
            let blend_a = gla.decompress_bone(calc.blend_frame as usize, bone_index)?;
            let blend_b = gla.decompress_bone(calc.blend_old_frame as usize, bone_index)?;
            old_blend_matrix = lerp_matrix(blend_a, blend_b, old_backlerp, old_frontlerp);
        }

        let mut local = if calc.frame.backlerp == 0.0 {
            gla.decompress_bone(calc.frame.current_frame as usize, bone_index)?
        } else {
            let frontlerp = 1.0 - calc.frame.backlerp;
            let new_matrix = gla.decompress_bone(calc.frame.new_frame as usize, bone_index)?;
            let current_matrix = gla.decompress_bone(calc.frame.current_frame as usize, bone_index)?;
            lerp_matrix(new_matrix, current_matrix, calc.frame.backlerp, frontlerp)
        };

        if calc.blend_mode {
            let blend_frontlerp = 1.0 - calc.blend_lerp;
            local = lerp_matrix(local, old_blend_matrix, calc.blend_lerp, blend_frontlerp);
        }

        out[bone_index] = if parent < 0 {
            multiply_3x4(&root_matrix, &local)
        } else {
            multiply_3x4(&out[parent as usize], &local)
        };
        // Exact OpenJK `G2_TransformBone` POSTMULT ordering: the normal
        // animation/parent result is post-multiplied by the BasePose-conjugated
        // angle override. Children therefore inherit this overridden parent.
        if let Some(override_matrix) = self.bone_angle_postmult[bone_index] {
            out[bone_index] = multiply_3x4(&out[bone_index], &override_matrix);
        }
        calcs[bone_index] = calc;
        visiting[bone_index] = false;
        complete[bone_index] = true;
        Ok(())
    }
}

fn validate_anim_range(start_frame: i32, end_frame: i32, num_frames: i32) -> Result<(), String> {
    if start_frame < 0 || start_frame > num_frames || end_frame < 0 || end_frame > num_frames {
        return Err(format!(
            "Ghoul2 animation range {start_frame}..{end_frame} outside 0..{num_frames}"
        ));
    }
    Ok(())
}

fn validate_frame_index(frame: i32, num_frames: i32, name: &str) -> Result<(), String> {
    if frame < 0 || frame >= num_frames {
        Err(format!("Ghoul2 {name} {frame} outside 0..{num_frames}"))
    } else {
        Ok(())
    }
}

fn fmod(value: f32, divisor: i32) -> f32 {
    value % divisor as f32
}

/// Exact control flow of OpenJK `G2_TimingModel`. Unlike a conventional
/// animation sampler, ending a non-freezing override clears the animation flag
/// and deliberately leaves the inherited parent frame unchanged.
fn g2_timing_model(
    bone: &mut Ghoul2BoneAnim,
    current_time: i32,
    num_frames: i32,
    inherited: &mut Ghoul2BoneFrame,
) -> Result<(), String> {
    validate_anim_range(bone.start_frame, bone.end_frame, num_frames)?;
    let anim_speed = bone.anim_speed;
    let mut time = if bone.pause_time != 0 {
        (bone.pause_time - bone.start_time) as f32 / 50.0
    } else {
        (current_time - bone.start_time) as f32 / 50.0
    };
    if time < 0.0 {
        time = 0.0;
    }
    let mut new_frame_g = bone.start_frame as f32 + time * anim_speed;
    let anim_size = bone.end_frame - bone.start_frame;
    let end_frame = bone.end_frame as f32;

    if anim_size != 0 {
        let ran_off = (anim_speed > 0.0 && new_frame_g > end_frame - 1.0)
            || (anim_speed < 0.0 && new_frame_g < end_frame + 1.0);
        if ran_off {
            if bone.flags & BONE_ANIM_OVERRIDE_LOOP != 0 {
                if anim_speed < 0.0 {
                    if new_frame_g < end_frame + 1.0 && new_frame_g >= end_frame {
                        inherited.backlerp = end_frame + 1.0 - new_frame_g;
                        inherited.current_frame = bone.end_frame;
                        inherited.new_frame = bone.start_frame;
                    } else {
                        if new_frame_g <= end_frame + 1.0 {
                            new_frame_g = end_frame
                                + fmod(new_frame_g - end_frame, anim_size)
                                - anim_size as f32;
                        }
                        inherited.backlerp = new_frame_g.ceil() - new_frame_g;
                        inherited.current_frame = new_frame_g.ceil() as i32;
                        inherited.new_frame = if inherited.current_frame <= bone.end_frame + 1 {
                            bone.start_frame
                        } else {
                            inherited.current_frame - 1
                        };
                    }
                } else if new_frame_g > end_frame - 1.0 && new_frame_g < end_frame {
                    inherited.backlerp = new_frame_g - new_frame_g.trunc();
                    inherited.current_frame = new_frame_g as i32;
                    inherited.new_frame = bone.start_frame;
                } else {
                    if new_frame_g >= end_frame {
                        new_frame_g = end_frame
                            + fmod(new_frame_g - end_frame, anim_size)
                            - anim_size as f32;
                    }
                    inherited.backlerp = new_frame_g - new_frame_g.trunc();
                    inherited.current_frame = new_frame_g as i32;
                    inherited.new_frame = if new_frame_g >= end_frame - 1.0 {
                        bone.start_frame
                    } else {
                        inherited.current_frame + 1
                    };
                }
            } else if bone.flags & BONE_ANIM_OVERRIDE_FREEZE == BONE_ANIM_OVERRIDE_FREEZE {
                inherited.current_frame = if anim_speed > 0.0 {
                    bone.end_frame - 1
                } else {
                    bone.end_frame + 1
                };
                inherited.new_frame = inherited.current_frame;
                inherited.backlerp = 0.0;
            } else {
                bone.flags &= !BONE_ANIM_TOTAL;
                return Ok(());
            }
        } else if anim_speed > 0.0 {
            inherited.current_frame = new_frame_g as i32;
            inherited.backlerp = new_frame_g - inherited.current_frame as f32;
            inherited.new_frame = inherited.current_frame + 1;
            if inherited.new_frame >= bone.end_frame {
                inherited.new_frame = if bone.flags & BONE_ANIM_OVERRIDE_LOOP != 0 {
                    bone.start_frame
                } else {
                    bone.end_frame - 1
                };
            }
        } else {
            inherited.backlerp = new_frame_g.ceil() - new_frame_g;
            inherited.current_frame = new_frame_g.ceil() as i32;
            if inherited.current_frame > bone.start_frame {
                inherited.current_frame = bone.start_frame;
                inherited.new_frame = inherited.current_frame;
                inherited.backlerp = 0.0;
            } else {
                inherited.new_frame = inherited.current_frame - 1;
                if inherited.new_frame < bone.end_frame + 1 {
                    inherited.new_frame = if bone.flags & BONE_ANIM_OVERRIDE_LOOP != 0 {
                        bone.start_frame
                    } else {
                        bone.end_frame + 1
                    };
                }
            }
        }
    } else {
        inherited.current_frame = if anim_speed < 0.0 {
            bone.end_frame + 1
        } else {
            bone.end_frame - 1
        };
        if inherited.current_frame < 0 {
            inherited.current_frame = 0;
        }
        inherited.new_frame = inherited.current_frame;
        inherited.backlerp = 0.0;
    }

    validate_frame_index(inherited.current_frame, num_frames, "currentFrame")?;
    validate_frame_index(inherited.new_frame, num_frames, "newFrame")?;
    Ok(())
}

fn lerp_matrix(a: Matrix3x4, b: Matrix3x4, a_weight: f32, b_weight: f32) -> Matrix3x4 {
    let mut out = [[0.0; 4]; 3];
    for row in 0..3 {
        for col in 0..4 {
            out[row][col] = a_weight * a[row][col] + b_weight * b[row][col];
        }
    }
    out
}

impl GlaAnimation {
    pub fn num_bones(&self) -> usize {
        self.skeleton.len()
    }

    /// Port of OpenJK's three-byte `mdxaIndex_t` lookup followed by
    /// `MC_UnCompressQuat` from the shared compressed-bone pool.
    pub fn decompress_bone(&self, frame: usize, bone: usize) -> Result<Matrix3x4, String> {
        if frame >= self.num_frames {
            return Err(format!("GLA frame {frame} out of range {}", self.num_frames));
        }
        if bone >= self.num_bones() {
            return Err(format!("GLA bone {bone} out of range {}", self.num_bones()));
        }
        let frame_bone = frame
            .checked_mul(self.num_bones())
            .and_then(|base| base.checked_add(bone))
            .ok_or("GLA frame/bone index overflow")?;
        let pool_index = self.frame_indices[frame_bone] as usize;
        let compressed = self
            .compressed_bones
            .get(pool_index)
            .ok_or_else(|| format!("GLA compressed bone index {pool_index} out of range"))?;
        Ok(uncompress_quat(compressed))
    }

    /// Evaluate one animation frame into the same parent-composed 3x4 bone matrices
    /// that Ghoul2 uses before bone-angle overrides. OpenJK composes root bones directly
    /// and child bones as `parent * local_animation_matrix`.
    pub fn compose_frame(&self, frame: usize) -> Result<Vec<Matrix3x4>, String> {
        let mut out = vec![[[0.0; 4]; 3]; self.num_bones()];
        let mut complete = vec![false; self.num_bones()];
        let mut visiting = vec![false; self.num_bones()];
        for bone in 0..self.num_bones() {
            self.compose_bone_recursive(frame, bone, &mut out, &mut complete, &mut visiting)?;
        }
        Ok(out)
    }

    fn compose_bone_recursive(
        &self,
        frame: usize,
        bone: usize,
        out: &mut [Matrix3x4],
        complete: &mut [bool],
        visiting: &mut [bool],
    ) -> Result<(), String> {
        if complete[bone] {
            return Ok(());
        }
        if visiting[bone] {
            return Err(format!("GLA skeleton cycle at bone {bone}"));
        }
        visiting[bone] = true;
        let local = self.decompress_bone(frame, bone)?;
        let parent = self.skeleton[bone].parent;
        out[bone] = if parent < 0 {
            local
        } else {
            let parent = parent as usize;
            self.compose_bone_recursive(frame, parent, out, complete, visiting)?;
            multiply_3x4(&out[parent], &local)
        };
        visiting[bone] = false;
        complete[bone] = true;
        Ok(())
    }
}

/// Exact arithmetic used by OpenJK's `MC_UnCompressQuat` for a 14-byte
/// `mdxaCompQuatBone_t`.
pub fn uncompress_quat(comp: &[u8; MDXA_COMP_QUAT_BONE_SIZE]) -> Matrix3x4 {
    let read = |index: usize| -> f32 {
        u16::from_le_bytes([comp[index * 2], comp[index * 2 + 1]]) as f32
    };
    let w = read(0) / 16383.0 - 2.0;
    let x = read(1) / 16383.0 - 2.0;
    let y = read(2) / 16383.0 - 2.0;
    let z = read(3) / 16383.0 - 2.0;

    let tx = 2.0 * x;
    let ty = 2.0 * y;
    let tz = 2.0 * z;
    let twx = tx * w;
    let twy = ty * w;
    let twz = tz * w;
    let txx = tx * x;
    let txy = ty * x;
    let txz = tz * x;
    let tyy = ty * y;
    let tyz = tz * y;
    let tzz = tz * z;

    [
        [
            1.0 - (tyy + tzz),
            txy - twz,
            txz + twy,
            read(4) / 64.0 - 512.0,
        ],
        [
            txy + twz,
            1.0 - (txx + tzz),
            tyz - twx,
            read(5) / 64.0 - 512.0,
        ],
        [
            txz - twy,
            tyz + twx,
            1.0 - (txx + tyy),
            read(6) / 64.0 - 512.0,
        ],
    ]
}

// OpenJK `Eorientations` values from `q_shared.h`.
const G2_POSITIVE_X: i32 = 1;
const G2_POSITIVE_Z: i32 = 2;
const G2_POSITIVE_Y: i32 = 3;
const G2_NEGATIVE_X: i32 = 4;
const G2_NEGATIVE_Z: i32 = 5;
const G2_NEGATIVE_Y: i32 = 6;

/// Direct port of the PRE/POSTMULT branch of OpenJK `G2_Generate_Matrix`.
fn g2_generate_postmult_matrix(
    angles: [f32; 3],
    up: i32,
    left: i32,
    forward: i32,
    base_pose: &Matrix3x4,
    base_pose_inv: &Matrix3x4,
) -> Result<Matrix3x4, String> {
    let mut new_angles = [0.0; 3];
    new_angles[1] = match up {
        G2_NEGATIVE_X => angles[2] + 180.0,
        G2_POSITIVE_X => angles[2],
        G2_NEGATIVE_Y | G2_POSITIVE_Y => angles[0],
        G2_NEGATIVE_Z => angles[1] + 180.0,
        G2_POSITIVE_Z => angles[1],
        _ => return Err(format!("unsupported Ghoul2 up orientation {up}")),
    };
    new_angles[0] = match left {
        G2_NEGATIVE_X => angles[2],
        G2_POSITIVE_X => angles[2] + 180.0,
        G2_NEGATIVE_Y => angles[0],
        G2_POSITIVE_Y => angles[0] + 180.0,
        G2_NEGATIVE_Z | G2_POSITIVE_Z => angles[1],
        _ => return Err(format!("unsupported Ghoul2 left orientation {left}")),
    };
    new_angles[2] = match forward {
        G2_NEGATIVE_X | G2_POSITIVE_X => angles[2],
        G2_NEGATIVE_Y => angles[0],
        G2_POSITIVE_Y => angles[0] + 180.0,
        G2_NEGATIVE_Z => angles[1],
        G2_POSITIVE_Z => angles[1] + 180.0,
        _ => return Err(format!("unsupported Ghoul2 forward orientation {forward}")),
    };

    // OpenJK `Create_Matrix`: AnglesToAxis followed by writing the axis vectors
    // into the matrix columns.
    let axis = q3_angles_to_axis(new_angles);
    let generated = [
        [axis[0][0], axis[1][0], axis[2][0], 0.0],
        [axis[0][1], axis[1][1], axis[2][1], 0.0],
        [axis[0][2], axis[1][2], axis[2][2], 0.0],
    ];
    let temp = multiply_3x4(&generated, base_pose_inv);
    Ok(multiply_3x4(base_pose, &temp))
}

fn q3_angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [
        -sr * sp * cy + cr * sy,
        -sr * sp * sy - cr * cy,
        -sr * cp,
    ];
    let up = [
        cr * sp * cy + sr * sy,
        cr * sp * sy - sr * cy,
        cr * cp,
    ];
    [forward, [-right[0], -right[1], -right[2]], up]
}

/// Row-major affine 3x4 multiply matching OpenJK's `Multiply_3x4Matrix` use
/// in Ghoul2 bone composition: `out = parent * local`.
pub fn multiply_3x4(parent: &Matrix3x4, local: &Matrix3x4) -> Matrix3x4 {
    let mut out = [[0.0; 4]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = parent[row][0] * local[0][col]
                + parent[row][1] * local[1][col]
                + parent[row][2] * local[2][col];
        }
        out[row][3] = parent[row][0] * local[0][3]
            + parent[row][1] * local[1][3]
            + parent[row][2] * local[2][3]
            + parent[row][3];
    }
    out
}

/// Port of jaPRO `CBoneCache::SmoothLow` (`r_ghoul2animsmooth`): blend each
/// composed bone matrix in `current` against its previous-frame filtered
/// value in `previous` (`factor` weights `previous`), then remove the
/// shear/scale drift that linearly blending rotation matrices introduces by
/// renormalizing the result through the bone's bind pose. `factor` of `0.0`
/// returns `current` unchanged. `previous` and `current` must be the same
/// length (one entry per GLA bone); mismatched lengths are a caller bug, not
/// a discontinuity to smooth over.
pub fn smooth_ghoul2_pose(
    gla: &GlaAnimation,
    previous: &[Matrix3x4],
    current: &[Matrix3x4],
    factor: f32,
) -> Vec<Matrix3x4> {
    assert_eq!(previous.len(), current.len(), "smooth_ghoul2_pose: pose length mismatch");
    current
        .iter()
        .enumerate()
        .map(|(index, current_matrix)| {
            let mut blended = [[0.0_f32; 4]; 3];
            for row in 0..3 {
                for col in 0..4 {
                    blended[row][col] = factor * previous[index][row][col]
                        + (1.0 - factor) * current_matrix[row][col];
                }
            }
            let base_pose = &gla.skeleton[index].base_pose;
            let base_pose_inv = &gla.skeleton[index].base_pose_inv;
            let mut temp = multiply_3x4(&blended, base_pose);
            let maxl = (base_pose[0][0] * base_pose[0][0]
                + base_pose[0][1] * base_pose[0][1]
                + base_pose[0][2] * base_pose[0][2])
                .sqrt();
            for row in temp.iter_mut() {
                let len = (row[0] * row[0] + row[1] * row[1] + row[2] * row[2]).sqrt();
                if len > f32::EPSILON {
                    let scale = maxl / len;
                    row[0] *= scale;
                    row[1] *= scale;
                    row[2] *= scale;
                }
            }
            multiply_3x4(&temp, base_pose_inv)
        })
        .collect()
}

pub fn transform_point(matrix: &Matrix3x4, point: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * point[0]
            + matrix[0][1] * point[1]
            + matrix[0][2] * point[2]
            + matrix[0][3],
        matrix[1][0] * point[0]
            + matrix[1][1] * point[1]
            + matrix[1][2] * point[2]
            + matrix[1][3],
        matrix[2][0] * point[0]
            + matrix[2][1] * point[1]
            + matrix[2][2] * point[2]
            + matrix[2][3],
    ]
}

pub fn transform_normal(matrix: &Matrix3x4, normal: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * normal[0] + matrix[0][1] * normal[1] + matrix[0][2] * normal[2],
        matrix[1][0] * normal[0] + matrix[1][1] * normal[1] + matrix[1][2] * normal[2],
        matrix[2][0] * normal[0] + matrix[2][1] * normal[1] + matrix[2][2] * normal[2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_quat(values: [f32; 4], translation: [f32; 3]) -> [u8; 14] {
        let mut out = [0u8; 14];
        for (index, value) in values.into_iter().enumerate() {
            let encoded = ((value + 2.0) * 16383.0).round() as u16;
            out[index * 2..index * 2 + 2].copy_from_slice(&encoded.to_le_bytes());
        }
        for (index, value) in translation.into_iter().enumerate() {
            let encoded = ((value + 512.0) * 64.0).round() as u16;
            let offset = (index + 4) * 2;
            out[offset..offset + 2].copy_from_slice(&encoded.to_le_bytes());
        }
        out
    }

    #[test]
    fn uncompress_quat_identity_translation() {
        let matrix = uncompress_quat(&pack_quat([1.0, 0.0, 0.0, 0.0], [4.0, -8.0, 12.5]));
        assert!((matrix[0][0] - 1.0).abs() < 0.001);
        assert!((matrix[1][1] - 1.0).abs() < 0.001);
        assert!((matrix[2][2] - 1.0).abs() < 0.001);
        assert!(matrix[0][1].abs() < 0.001);
        assert!(matrix[0][2].abs() < 0.001);
        assert!((matrix[0][3] - 4.0).abs() < 0.001);
        assert!((matrix[1][3] + 8.0).abs() < 0.001);
        assert!((matrix[2][3] - 12.5).abs() < 0.001);
    }

    #[test]
    fn affine_multiply_composes_translation() {
        let parent = [
            [1.0, 0.0, 0.0, 10.0],
            [0.0, 1.0, 0.0, 20.0],
            [0.0, 0.0, 1.0, 30.0],
        ];
        let child = [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 2.0],
            [0.0, 0.0, 1.0, 3.0],
        ];
        let out = multiply_3x4(&parent, &child);
        assert_eq!(transform_point(&out, [0.0, 0.0, 0.0]), [11.0, 22.0, 33.0]);
    }

    #[test]
    fn g2_timing_model_positive_loop_matches_openjk() {
        let mut bone = Ghoul2BoneAnim {
            bone_index: 0,
            start_frame: 10,
            end_frame: 14,
            flags: BONE_ANIM_OVERRIDE_LOOP,
            anim_speed: 1.0,
            start_time: 1000,
            pause_time: 0,
            blend_frame: 0.0,
            blend_lerp_frame: 0,
            blend_time: 0,
            blend_start: 0,
        };
        let mut frame = Ghoul2BoneFrame::default();
        g2_timing_model(&mut bone, 1025, 32, &mut frame).unwrap();
        assert_eq!(frame.current_frame, 10);
        assert_eq!(frame.new_frame, 11);
        assert!((frame.backlerp - 0.5).abs() < 0.0001);

        g2_timing_model(&mut bone, 1200, 32, &mut frame).unwrap();
        assert_eq!(frame.current_frame, 10);
        assert_eq!(frame.new_frame, 11);
        assert!(frame.backlerp.abs() < 0.0001);
    }

    #[test]
    fn g2_timing_model_freezes_on_last_frame() {
        let mut bone = Ghoul2BoneAnim {
            bone_index: 0,
            start_frame: 4,
            end_frame: 8,
            flags: BONE_ANIM_OVERRIDE_FREEZE,
            anim_speed: 1.0,
            start_time: 0,
            pause_time: 0,
            blend_frame: 0.0,
            blend_lerp_frame: 0,
            blend_time: 0,
            blend_start: 0,
        };
        let mut frame = Ghoul2BoneFrame::default();
        g2_timing_model(&mut bone, 1000, 32, &mut frame).unwrap();
        assert_eq!(frame.current_frame, 7);
        assert_eq!(frame.new_frame, 7);
        assert_eq!(frame.backlerp, 0.0);
        assert!(bone.is_animating());
    }

    #[test]
    fn g2_timing_model_ended_override_falls_back_to_parent_state() {
        let mut bone = Ghoul2BoneAnim {
            bone_index: 0,
            start_frame: 4,
            end_frame: 8,
            flags: BONE_ANIM_OVERRIDE,
            anim_speed: 1.0,
            start_time: 0,
            pause_time: 0,
            blend_frame: 0.0,
            blend_lerp_frame: 0,
            blend_time: 0,
            blend_start: 0,
        };
        let mut frame = Ghoul2BoneFrame { current_frame: 2, new_frame: 3, backlerp: 0.25 };
        g2_timing_model(&mut bone, 1000, 32, &mut frame).unwrap();
        assert_eq!(frame, Ghoul2BoneFrame { current_frame: 2, new_frame: 3, backlerp: 0.25 });
        assert!(!bone.is_animating());
    }

    #[test]
    fn rb_surface_ghoul_two_weight_position_and_first_bone_normal() {
        let surface = GlmSurface {
            surface_index: 3,
            vertices: vec![GlmVertex {
                normal: [1.0, 1.0, 1.0],
                position: [1.0, 2.0, 3.0],
                weights: vec![
                    GlmWeight { local_bone_index: 0, weight: 0.25 },
                    GlmWeight { local_bone_index: 1, weight: 0.75 },
                ],
            }],
            texcoords: vec![[0.25, 0.75]],
            triangles: vec![[0, 0, 0]],
            bone_references: vec![0, 1],
        };
        let pose = vec![
            [
                [2.0, 0.0, 0.0, 10.0],
                [0.0, 3.0, 0.0, 0.0],
                [0.0, 0.0, 4.0, 0.0],
            ],
            [
                [1.0, 0.0, 0.0, 20.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        ];
        let skinned = skin_glm_surface(&surface, &pose).unwrap();
        let vertex = skinned.vertices[0];
        // RB_SurfaceGhoul's two-weight branch: w * (bone0 - bone1) + bone1.
        assert_eq!(vertex.position, [18.75, 3.0, 5.25]);
        // The active fast-normal path uses only the first bone, with no position weighting.
        assert_eq!(vertex.normal, [2.0, 3.0, 4.0]);
        assert_eq!(vertex.uv, [0.25, 0.75]);
        assert_eq!(skinned.indices, vec![0, 0, 0]);
    }

    #[test]
    fn rb_surface_ghoul_multi_weight_uses_last_residual() {
        let surface = GlmSurface {
            surface_index: 0,
            vertices: vec![GlmVertex {
                normal: [0.0, 0.0, 1.0],
                position: [0.0, 0.0, 0.0],
                weights: vec![
                    GlmWeight { local_bone_index: 0, weight: 0.2 },
                    GlmWeight { local_bone_index: 1, weight: 0.3 },
                    // Intentionally bogus stored final value: RB_SurfaceGhoul recomputes it.
                    GlmWeight { local_bone_index: 2, weight: 0.9 },
                ],
            }],
            texcoords: vec![[0.0, 0.0]],
            triangles: vec![],
            bone_references: vec![0, 1, 2],
        };
        let translated = |x: f32| [
            [1.0, 0.0, 0.0, x],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let skinned = skin_glm_surface(
            &surface,
            &[translated(10.0), translated(20.0), translated(30.0)],
        )
        .unwrap();
        assert!((skinned.vertices[0].position[0] - 23.0).abs() < 0.0001);
    }

    #[test]
    fn openjk_default_gla_matches_renderer_fake_asset() {
        let gla = openjk_default_gla().unwrap();
        assert_eq!(gla.name, "*default");
        assert_eq!(gla.num_frames, 1);
        assert_eq!(gla.skeleton.len(), 1);
        assert_eq!(gla.skeleton[0].name, "ModView internal default");
        assert_eq!(gla.skeleton[0].parent, -1);
        assert_eq!(gla.skeleton[0].base_pose, [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ]);
    }

    // Synthetic MDXM bytes keep these regression tests independent of PK3s.
    // Two LODs and multiple surfaces exercise every registration/skinning path.
    fn humanoid_glm_fixture(num_bones: i32, anim_name: &str, references: &[i32]) -> Vec<u8> {
        fn put_i32(data: &mut [u8], offset: usize, value: i32) {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let surface_count = references.chunks(8).len();
        let hierarchy_start = MDXM_HEADER_SIZE + surface_count * 4;
        let lods_start = hierarchy_start + surface_count * MDXM_HIERARCHY_PREFIX;
        let mut data = vec![0; lods_start];
        data[..4].copy_from_slice(b"2LGM");
        put_i32(&mut data, 4, MDXM_VERSION);
        data[72..72 + anim_name.len()].copy_from_slice(anim_name.as_bytes());
        put_i32(&mut data, 140, num_bones);
        put_i32(&mut data, 144, 2);
        put_i32(&mut data, 148, lods_start as i32);
        put_i32(&mut data, 152, surface_count as i32);
        put_i32(&mut data, 156, hierarchy_start as i32);
        for index in 0..surface_count {
            let start = hierarchy_start + index * MDXM_HIERARCHY_PREFIX;
            put_i32(
                &mut data,
                MDXM_HEADER_SIZE + index * 4,
                (start - MDXM_HEADER_SIZE) as i32,
            );
            put_i32(&mut data, start + 136, -1);
        }
        for _ in 0..2 {
            let lod_start = data.len();
            let offsets_start = lod_start + 4;
            data.resize(offsets_start + surface_count * 4, 0);
            for (surface_index, bones) in references.chunks(8).enumerate() {
                let start = data.len();
                let verts_offset = MDXM_SURFACE_SIZE;
                let refs_offset = verts_offset + bones.len() * (MDXM_VERTEX_SIZE + 8);
                let surface_size = refs_offset + bones.len() * 4;
                data.resize(start + surface_size, 0);
                put_i32(
                    &mut data,
                    offsets_start + surface_index * 4,
                    (start - offsets_start) as i32,
                );
                put_i32(&mut data, start + 4, surface_index as i32);
                put_i32(&mut data, start + 8, -(start as i32));
                put_i32(&mut data, start + 12, bones.len() as i32);
                put_i32(&mut data, start + 16, verts_offset as i32);
                put_i32(&mut data, start + 24, refs_offset as i32);
                put_i32(&mut data, start + 28, bones.len() as i32);
                put_i32(&mut data, start + 32, refs_offset as i32);
                put_i32(&mut data, start + 36, surface_size as i32);
                for (local, &bone) in bones.iter().enumerate() {
                    let vertex = start + verts_offset + local * MDXM_VERTEX_SIZE;
                    for (offset, value) in [(8, 1.0f32), (12, 1.0), (16, 2.0), (20, 3.0)] {
                        data[vertex + offset..vertex + offset + 4]
                            .copy_from_slice(&value.to_le_bytes());
                    }
                    // One weight, using its original local table index.
                    put_i32(&mut data, vertex + 24, local as i32);
                    put_i32(&mut data, start + refs_offset + local * 4, bone);
                }
            }
            let lod_size = data.len() - lod_start;
            put_i32(&mut data, lod_start, lod_size as i32);
        }
        let file_size = data.len();
        put_i32(&mut data, 160, file_size as i32);
        data
    }

    #[test]
    fn legacy_humanoid_skins_with_jka_pose_across_surfaces_and_lods() {
        // JK2 toes, spine, hands, removed finger bones, hand tag and face.
        let old = [7, 8, 13, 31, 32, 33, 48, 54, 55, 56, 62, 68, 71];
        let expected = [6, 7, 11, 29, 29, 34, 36, 42, 42, 43, 45, 47, 52];
        let model = parse_glm(&humanoid_glm_fixture(
            72,
            "models/players/_humanoid/_humanoid",
            &old,
        ))
        .unwrap();
        assert_eq!(model.num_bones, 53);
        let pose = (0..53)
            .map(|bone| {
                [
                    [1.0, 0.0, 0.0, bone as f32],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                ]
            })
            .collect::<Vec<_>>();
        assert_eq!(model.lods.len(), 2);
        for lod in &model.lods {
            assert_eq!(lod.surfaces.len(), 2);
            for (surface, expected_bones) in lod.surfaces.iter().zip(expected.chunks(8)) {
                assert_eq!(surface.bone_references, expected_bones);
                let skinned = skin_glm_surface(surface, &pose).unwrap();
                for (local, (&bone, vertex)) in
                    expected_bones.iter().zip(&skinned.vertices).enumerate()
                {
                    assert_eq!(
                        surface.vertices[local].weights,
                        vec![GlmWeight {
                            local_bone_index: local,
                            weight: 1.0
                        }]
                    );
                    assert_eq!(vertex.position, [1.0 + bone as f32, 2.0, 3.0]);
                    assert_eq!(vertex.normal, [0.0, 0.0, 1.0]);
                }
            }
        }
    }

    #[test]
    fn native_and_non_humanoid_meshes_keep_authored_bone_references() {
        for (count, anim_name, references) in [
            (
                53,
                "models/players/_humanoid/_humanoid",
                vec![7, 8, 13, 32, 48, 52],
            ),
            (
                72,
                "models/players/custom/custom",
                vec![7, 8, 13, 54, 68, 71],
            ),
            (1, "*default", vec![0]),
        ] {
            let model = parse_glm(&humanoid_glm_fixture(count, anim_name, &references)).unwrap();
            assert_eq!(model.num_bones, count as usize);
            for lod in &model.lods {
                assert_eq!(
                    lod.surfaces[0].bone_references,
                    references
                        .iter()
                        .map(|&bone| bone as usize)
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn legacy_humanoid_rejects_invalid_authored_bone_references() {
        for (bone, message) in [
            (-1, "negative Ghoul2 GLM bone reference"),
            (72, "references bone 72, but mesh declares 72"),
        ] {
            let bytes = humanoid_glm_fixture(72, "models/players/_humanoid/_humanoid", &[bone]);
            assert!(parse_glm(&bytes).unwrap_err().contains(message));
        }
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock assets and the JAWA skin packs"]
    fn legacy_jawa_models_skin_with_stock_animation() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = crate::pk3::AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let gla_bytes = assets
            .read("models/players/_humanoid/_humanoid.gla", 128 * 1024 * 1024)
            .unwrap()
            .expect("stock humanoid GLA")
            .bytes;
        let gla = parse_gla(&gla_bytes).unwrap();
        assert_eq!(gla.num_bones(), 53);
        let poses = [0, 100, gla.num_frames - 1].map(|frame| gla.compose_frame(frame).unwrap());
        for name in [
            "jawa_loki_4lom",
            "jawa_loki_tc14",
            "jawa_loki_c3po",
            "jawa_maw",
        ] {
            let qpath = format!("models/players/{name}/model.glm");
            let bytes = assets
                .read(&qpath, 16 * 1024 * 1024)
                .unwrap()
                .expect(&qpath)
                .bytes;
            assert_eq!(i32_at(&bytes, 140, "bone count").unwrap(), 72);
            let model = parse_glm(&bytes).unwrap();
            assert_eq!(model.num_bones, gla.num_bones());
            let mut vertex_count = 0;
            for lod in &model.lods {
                for surface in &lod.surfaces {
                    assert!(surface
                        .bone_references
                        .iter()
                        .all(|&bone| bone < gla.num_bones()));
                    for pose in &poses {
                        let skinned = skin_glm_surface(surface, pose).unwrap();
                        assert!(skinned.vertices.iter().all(|v| v
                            .position
                            .iter()
                            .chain(&v.normal)
                            .all(|x| x.is_finite())));
                    }
                    vertex_count += surface.vertices.len();
                }
            }
            assert!(vertex_count > 0);
            println!(
                "{qpath}: 72 -> 53 bones, {} LODs, {vertex_count} vertices skinned at 3 frames",
                model.lods.len()
            );
        }
    }

    #[test]
    fn rejects_wrong_formats() {
        assert!(parse_glm(b"not a glm").is_err());
        assert!(parse_gla(b"not a gla").is_err());
    }
}

/// Reconstruct an original Ghoul2 surface/tag bolt matrix from an already
/// evaluated pose.  This is the normal-model-tag branch of OpenJK
/// `G2_ProcessSurfaceBolt2`: it transforms the first three tag vertices, uses
/// side 0/side 2 as the authored long/short axes, and uses vertex 2 as the tag
/// origin.  The returned matrix is the internal/model-space "low" bolt matrix
/// used to attach another Ghoul2 model; the public G2 API applies its historical
/// 90-degree column fix after multiplying this into entity world space.
pub fn surface_bolt_matrix(
    glm: &GlmModel,
    pose: &[Matrix3x4],
    surface_name: &str,
) -> Result<Option<Matrix3x4>, String> {
    let Some(surface_index) = glm
        .hierarchy
        .iter()
        .position(|hierarchy| hierarchy.name.eq_ignore_ascii_case(surface_name))
    else {
        return Ok(None);
    };
    let Some(lod) = glm.lods.first() else {
        return Ok(None);
    };
    let Some(surface) = lod
        .surfaces
        .iter()
        .find(|surface| surface.surface_index == surface_index)
    else {
        return Ok(None);
    };
    if surface.vertices.len() < 3 {
        return Err(format!(
            "Ghoul2 bolt surface {surface_name:?} has {} vertices, expected at least 3",
            surface.vertices.len()
        ));
    }

    // `G2_ProcessSurfaceBolt2` walks the first three mdxmVertex_t records, not
    // the surface triangle index list.
    let skinned = skin_glm_surface(surface, pose)?;
    let p = [
        skinned.vertices[0].position,
        skinned.vertices[1].position,
        skinned.vertices[2].position,
    ];
    let sides = [
        sub3(p[1], p[0]),
        sub3(p[2], p[1]),
        sub3(p[0], p[2]),
    ];
    const TRISIDE_LONGEST: usize = 0;
    const TRISIDE_SHORTEST: usize = 2;
    const TAG_ORIGIN: usize = 2;

    let mut axis0 = normalize3(sides[TRISIDE_LONGEST]);
    let axis1 = normalize3(sides[TRISIDE_SHORTEST]);
    let d = dot3(axis0, axis1);
    axis0 = normalize3([
        axis0[0] - d * axis1[0],
        axis0[1] - d * axis1[1],
        axis0[2] - d * axis1[2],
    ]);
    let axis2 = normalize3(cross3(
        sides[TRISIDE_LONGEST],
        sides[TRISIDE_SHORTEST],
    ));
    if length_sq3(axis0) <= f32::EPSILON
        || length_sq3(axis1) <= f32::EPSILON
        || length_sq3(axis2) <= f32::EPSILON
    {
        return Err(format!("Ghoul2 bolt surface {surface_name:?} is degenerate"));
    }

    let origin = p[TAG_ORIGIN];
    Ok(Some([
        [axis1[0], axis0[0], -axis2[0], origin[0]],
        [axis1[1], axis0[1], -axis2[1], origin[1]],
        [axis1[2], axis0[2], -axis2[2], origin[2]],
    ]))
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length_sq3(v: [f32; 3]) -> f32 {
    dot3(v, v)
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let length_sq = length_sq3(v);
    if length_sq <= f32::EPSILON {
        return [0.0; 3];
    }
    let inv = length_sq.sqrt().recip();
    [v[0] * inv, v[1] * inv, v[2] * inv]
}

/// OpenJK `G2_Add_Bolt` searches surface tags first and bones second.  Return
/// the same internal/model-space bolt transform for a named attachment point.
pub fn model_bolt_matrix(
    glm: &GlmModel,
    gla: &GlaAnimation,
    pose: &[Matrix3x4],
    name: &str,
) -> Result<Option<Matrix3x4>, String> {
    if let Some(surface) = surface_bolt_matrix(glm, pose, name)? {
        return Ok(Some(surface));
    }
    let Some(bone_index) = Ghoul2Animator::bone_index(gla, name) else {
        return Ok(None);
    };
    let animated = pose
        .get(bone_index)
        .ok_or_else(|| format!("Ghoul2 bolt bone {name:?} outside evaluated pose"))?;
    let base = &gla
        .skeleton
        .get(bone_index)
        .ok_or_else(|| format!("Ghoul2 bolt bone {name:?} outside GLA skeleton"))?
        .base_pose;
    Ok(Some(multiply_3x4(animated, base)))
}
