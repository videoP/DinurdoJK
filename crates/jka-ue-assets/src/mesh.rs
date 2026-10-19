//! Validated native skeletal prop geometry, shared by compiler and renderer.
use serde_json::Value;
#[derive(Debug)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub bones: [usize; 4],
    pub weights: [f32; 4],
}
#[derive(Debug)]
pub struct Section {
    pub first: usize,
    pub end: usize,
    pub shader: String,
}
#[derive(Debug)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub sections: Vec<Section>,
    pub bones: Vec<String>,
}
impl Mesh {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if v["version"] != 1 {
            return Err("unsupported native prop version".into());
        }
        let names = v["bones"].as_array().ok_or("no prop skeleton")?;
        if names.is_empty() || names.len() > 4096 {
            return Err("invalid prop bone count".into());
        }
        let bones = names
            .iter()
            .map(|n| {
                n.as_str()
                    .map(str::to_owned)
                    .ok_or("invalid prop bone name".into())
            })
            .collect::<Result<Vec<_>, String>>()?;
        let rows = v["vertices"].as_array().ok_or("no prop vertices")?;
        if rows.is_empty() || rows.len() > 2_000_000 {
            return Err("invalid prop vertex count".into());
        }
        fn floats<const N: usize>(v: &Value) -> Result<[f32; N], String> {
            let a = v
                .as_array()
                .filter(|a| a.len() == N)
                .ok_or("invalid prop vector")?;
            let mut out = [0.; N];
            for i in 0..N {
                out[i] = a[i].as_f64().ok_or("invalid prop scalar")? as f32;
                if !out[i].is_finite() {
                    return Err("non-finite prop scalar".into());
                }
            }
            Ok(out)
        }
        let mut vertices = Vec::with_capacity(rows.len());
        for row in rows {
            let weights = floats::<4>(&row["weights"])?;
            if weights.iter().any(|&w| w < 0.) || (weights.iter().sum::<f32>() - 1.).abs() > 0.002 {
                return Err("invalid prop weights".into());
            }
            let refs = row["bones"]
                .as_array()
                .filter(|a| a.len() == 4)
                .ok_or("invalid prop bone references")?;
            let mut refs_out = [0; 4];
            for i in 0..4 {
                refs_out[i] = refs[i]
                    .as_u64()
                    .filter(|&n| n < bones.len() as u64)
                    .ok_or("prop influence outside skeleton")?
                    as usize;
            }
            vertices.push(Vertex {
                position: floats(&row["position"])?,
                normal: floats(&row["normal"])?,
                uv: floats(&row["uv"])?,
                bones: refs_out,
                weights,
            });
        }
        let indices = v["indices"]
            .as_array()
            .ok_or("no prop triangles")?
            .iter()
            .map(|i| {
                i.as_u64()
                    .filter(|&n| n < vertices.len() as u64)
                    .map(|n| n as u32)
                    .ok_or("prop triangle outside vertices".into())
            })
            .collect::<Result<Vec<_>, String>>()?;
        if indices.len() > 12_000_000 || indices.len() % 3 != 0 {
            return Err("invalid prop triangles".into());
        }
        let sections = v["sections"]
            .as_array()
            .ok_or("no prop surfaces")?
            .iter()
            .map(|s| {
                let first = s["first"].as_u64().ok_or("invalid prop section")? as usize;
                let end = s["end"].as_u64().ok_or("invalid prop section")? as usize;
                if first > end || end > indices.len() || first % 3 != 0 || end % 3 != 0 {
                    return Err("prop section outside indices".into());
                }
                Ok(Section {
                    first,
                    end,
                    shader: qpath(s["shader"].as_str().ok_or("no prop material")?)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if sections.len() > 1024 {
            return Err("too many prop surfaces".into());
        }
        Ok(Self {
            vertices,
            indices,
            sections,
            bones,
        })
    }
}

fn qpath(path: &str) -> Result<String, String> {
    if path.is_empty()
        || path.len() > 256
        || path.contains(':')
        || path.contains('\\')
        || path.contains('\0')
    {
        return Err("invalid material path".into());
    }
    if path
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("invalid material path".into());
    }
    Ok(path.to_owned())
}
