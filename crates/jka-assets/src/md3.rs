//! Validated MD3 loader retaining every animation frame and tag.
//!
//! Earlier DinurdoJK code intentionally kept only frame 0.  CGame entity
//! presentation needs the actual MD3 frame/tag data, so this representation is
//! still renderer-friendly while preserving the complete protocol-visible model.
use std::f32::consts::TAU;

#[derive(Debug, Clone)]
pub struct Model {
    pub frames: Vec<Frame>,
    /// Tags are indexed `[frame][tag]` exactly as stored by MD3.
    pub tags: Vec<Vec<Tag>>,
    pub surfaces: Vec<Surface>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub local_origin: [f32; 3],
    pub radius: f32,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Tag {
    pub name: String,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
}

#[derive(Debug, Clone)]
pub struct Surface {
    pub name: String,
    pub shader: String,
    /// Backwards-compatible frame-0 view used by older callers.
    pub vertices: Vec<Vertex>,
    /// Complete MD3 vertex animation, indexed `[frame][vertex]`.
    pub frames: Vec<Vec<Vertex>>,
    pub indices: Vec<u32>,
}

impl Surface {
    pub fn frame_vertices(&self, frame: usize) -> Option<&[Vertex]> {
        self.frames.get(frame).map(Vec::as_slice)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

fn i32_at(d: &[u8], o: usize) -> Result<i32, String> {
    d.get(o..o + 4)
        .ok_or_else(|| "truncated MD3".to_string())
        .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}

fn f32_at(d: &[u8], o: usize) -> Result<f32, String> {
    let v = f32::from_bits(i32_at(d, o)? as u32);
    if v.is_finite() {
        Ok(v)
    } else {
        Err("non-finite MD3 float".into())
    }
}

fn vec3_at(d: &[u8], o: usize) -> Result<[f32; 3], String> {
    Ok([f32_at(d, o)?, f32_at(d, o + 4)?, f32_at(d, o + 8)?])
}

fn usize_i(v: i32, what: &str) -> Result<usize, String> {
    usize::try_from(v).map_err(|_| format!("negative MD3 {what}"))
}

fn cstr(d: &[u8]) -> String {
    let n = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..n]).into_owned()
}

fn qpath_cstr(d: &[u8]) -> String {
    cstr(d).replace('\\', "/").to_ascii_lowercase()
}

fn span(
    base: usize,
    off: i32,
    len: usize,
    limit: usize,
    what: &str,
) -> Result<std::ops::Range<usize>, String> {
    let s = base
        .checked_add(usize_i(off, what)?)
        .ok_or("MD3 offset overflow")?;
    let e = s
        .checked_add(len)
        .filter(|&e| e <= limit)
        .ok_or_else(|| format!("MD3 {what} outside file"))?;
    Ok(s..e)
}

fn decode_normal(packed: u16) -> [f32; 3] {
    let lat = ((packed >> 8) & 255) as f32 * TAU / 255.0;
    let lng = (packed & 255) as f32 * TAU / 255.0;
    [lat.cos() * lng.sin(), lat.sin() * lng.sin(), lng.cos()]
}

pub fn parse(data: &[u8]) -> Result<Model, String> {
    if data.len() < 108 || &data[..4] != b"IDP3" || i32_at(data, 4)? != 15 {
        return Err("expected MD3 version 15".into());
    }

    let num_frames = usize_i(i32_at(data, 76)?, "frame count")?;
    let num_tags = usize_i(i32_at(data, 80)?, "tag count")?;
    let num_surfaces = usize_i(i32_at(data, 84)?, "surface count")?;
    if num_frames == 0 || num_frames > 4096 || num_tags > 4096 || num_surfaces > 1024 {
        return Err("MD3 dimensions outside limits".into());
    }

    let end = usize_i(i32_at(data, 104)?, "end offset")?.min(data.len());
    if end < 108 {
        return Err("MD3 end before header".into());
    }

    let frame_range = span(0, i32_at(data, 92)?, num_frames * 56, end, "frames")?;
    let tag_range = span(
        0,
        i32_at(data, 96)?,
        num_frames
            .checked_mul(num_tags)
            .and_then(|count| count.checked_mul(112))
            .ok_or("MD3 tag size overflow")?,
        end,
        "tags",
    )?;

    let mut frames = Vec::with_capacity(num_frames);
    for frame_index in 0..num_frames {
        let o = frame_range.start + frame_index * 56;
        frames.push(Frame {
            mins: vec3_at(data, o)?,
            maxs: vec3_at(data, o + 12)?,
            local_origin: vec3_at(data, o + 24)?,
            radius: f32_at(data, o + 36)?,
            name: cstr(&data[o + 40..o + 56]),
        });
    }

    let mut tags = Vec::with_capacity(num_frames);
    for frame_index in 0..num_frames {
        let mut frame_tags = Vec::with_capacity(num_tags);
        for tag_index in 0..num_tags {
            let o = tag_range.start + (frame_index * num_tags + tag_index) * 112;
            let mut axis = [[0.0; 3]; 3];
            for row in 0..3 {
                axis[row] = vec3_at(data, o + 76 + row * 12)?;
            }
            frame_tags.push(Tag {
                name: cstr(&data[o..o + 64]),
                origin: vec3_at(data, o + 64)?,
                axis,
            });
        }
        tags.push(frame_tags);
    }

    let mut cursor = usize_i(i32_at(data, 100)?, "surface offset")?;
    let mut out = Vec::with_capacity(num_surfaces);
    for _ in 0..num_surfaces {
        if cursor + 108 > end || &data[cursor..cursor + 4] != b"IDP3" {
            return Err("invalid MD3 surface".into());
        }
        let name = cstr(&data[cursor + 4..cursor + 68]);
        let surface_frames = usize_i(i32_at(data, cursor + 72)?, "surface frame count")?;
        let shaders = usize_i(i32_at(data, cursor + 76)?, "shader count")?;
        let verts = usize_i(i32_at(data, cursor + 80)?, "vertex count")?;
        let tris = usize_i(i32_at(data, cursor + 84)?, "triangle count")?;
        if surface_frames != num_frames {
            return Err(format!(
                "MD3 surface {name} has {surface_frames} frames, model has {num_frames}"
            ));
        }
        if verts > 1_000_000 || tris > 2_000_000 {
            return Err("MD3 surface dimensions outside limits".into());
        }
        let surf_end = usize_i(i32_at(data, cursor + 104)?, "surface end")?;
        let surface_limit = cursor
            .checked_add(surf_end)
            .filter(|&e| e <= end && e >= cursor + 108)
            .ok_or("MD3 surface end outside file")?;
        let tr = span(
            cursor,
            i32_at(data, cursor + 88)?,
            tris * 12,
            surface_limit,
            "triangles",
        )?;
        let sh = span(
            cursor,
            i32_at(data, cursor + 92)?,
            shaders * 68,
            surface_limit,
            "shaders",
        )?;
        let st = span(
            cursor,
            i32_at(data, cursor + 96)?,
            verts * 8,
            surface_limit,
            "texcoords",
        )?;
        let xyz = span(
            cursor,
            i32_at(data, cursor + 100)?,
            surface_frames
                .checked_mul(verts)
                .and_then(|count| count.checked_mul(8))
                .ok_or("MD3 vertex size overflow")?,
            surface_limit,
            "vertices",
        )?;
        let shader = if shaders > 0 {
            qpath_cstr(&data[sh.start..sh.start + 64])
        } else {
            name.replace('\\', "/").to_ascii_lowercase()
        };

        let mut uvs = Vec::with_capacity(verts);
        for v in 0..verts {
            let so = st.start + v * 8;
            uvs.push([f32_at(data, so)?, f32_at(data, so + 4)?]);
        }

        let mut vertex_frames = Vec::with_capacity(surface_frames);
        for frame_index in 0..surface_frames {
            let mut vertices = Vec::with_capacity(verts);
            for v in 0..verts {
                let xo = xyz.start + (frame_index * verts + v) * 8;
                let coord = |n: usize| {
                    i16::from_le_bytes(data[xo + n * 2..xo + n * 2 + 2].try_into().unwrap())
                        as f32
                        / 64.0
                };
                let packed = u16::from_le_bytes(data[xo + 6..xo + 8].try_into().unwrap());
                vertices.push(Vertex {
                    position: [coord(0), coord(1), coord(2)],
                    normal: decode_normal(packed),
                    uv: uvs[v],
                });
            }
            vertex_frames.push(vertices);
        }

        let mut indices = Vec::with_capacity(tris * 3);
        for t in 0..tris * 3 {
            let i = usize_i(i32_at(data, tr.start + t * 4)?, "triangle index")?;
            if i >= verts {
                return Err("MD3 triangle index out of range".into());
            }
            indices.push(i as u32);
        }

        let vertices = vertex_frames.first().cloned().unwrap_or_default();
        out.push(Surface {
            name,
            shader,
            vertices,
            frames: vertex_frames,
            indices,
        });
        cursor = surface_limit;
    }

    Ok(Model {
        frames,
        tags,
        surfaces: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_normal_is_unit_length() {
        let normal = decode_normal(0x4080);
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        assert!((length - 1.0).abs() < 1.0e-5);
    }
}
