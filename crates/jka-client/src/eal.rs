//! Creative EAGLE Environmental Audio Library (.eal) RIFF reader.
//!
//! EAXManager/EalMan-compatible authored map parsing. Audio DSP is implemented
//! in the per-source `eal_native` scene and remains optional at runtime.
//!
//! Important: the EAGLE BSP is *not* the Quake 3 map BSP. Its split-plane
//! tolerance is -1.0 JKA world unit, following EalMan's observed EaxMan rule.

const MAX_EAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 65_536;
const MAX_NODES: usize = 1_000_000;

#[derive(Clone, Debug)]
pub struct EalPreset {
    pub name: String,
    /// Raw, little-endian 4-byte EAX fields. EAX v2 has 14 fields and EAX v3
    /// (used by Jedi Academy's EaxMan v3) has 28 fields. Keep all parameters;
    /// don't silently approximate an EAX3 preset as an EAX2 preset.
    pub eax_words: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
struct Split {
    base: [f32; 3],
    normal: [f32; 3],
    front: u32,
    back: u32,
}

#[derive(Clone, Copy, Debug)]
struct Zone {
    environment_index: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EalObstruction {
    pub level_mb: i32,
    pub lf_ratio: f32,
    pub room_ratio: f32,
    pub obstructs: bool,
}

impl EalObstruction {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 16 {
            return Err("invalid EAL obstacle size".into());
        }
        Ok(Self {
            level_mb: signed(bytes, 0)?,
            lf_ratio: scalar(bytes, 4)?,
            room_ratio: scalar(bytes, 8)?,
            obstructs: number(bytes, 12)? == 1,
        })
    }
}

pub struct EalSourceAcoustics {
    pub obstruction: EalObstruction,
    pub index: i32,
}

#[derive(Clone, Debug)]
pub struct EalMap {
    raw: std::sync::Arc<[u8]>,
    pub version: (u32, u32),
    pub default: EalPreset,
    pub environments: Vec<EalPreset>,
    pub geometry_sets: Vec<String>,
    mapped_ids: Vec<i32>,
    nodes: Vec<Split>,
    zones: Vec<Zone>,
    start: u32,
    matrix: Vec<i32>,
    obstacles: Vec<EalObstruction>,
    default_obstacle: EalObstruction,
}

/// A zone result is a reference to the actual EAX preset, not an approximation
/// using BSP contents/fog, and has the same -1 default ID as EAXManager.
#[derive(Clone, Copy, Debug)]
pub struct EnvironmentAt<'a> {
    pub id: i32,
    pub zone_index: usize,
    pub preset: &'a EalPreset,
}

fn number(b: &[u8], at: usize) -> Result<u32, String> {
    let range = b
        .get(at..at.checked_add(4).ok_or("EAL offset overflow")?)
        .ok_or_else(|| "truncated EAL integer".to_owned())?;
    Ok(u32::from_le_bytes(
        range.try_into().expect("exact four bytes"),
    ))
}
fn scalar(b: &[u8], at: usize) -> Result<f32, String> {
    let result = f32::from_bits(number(b, at)?);
    if !result.is_finite() {
        return Err("non-finite EAL coordinate".into());
    }
    Ok(result)
}
fn signed(b: &[u8], at: usize) -> Result<i32, String> {
    Ok(number(b, at)? as i32)
}
fn limit_count(count: usize, cap: usize) -> Result<usize, String> {
    if count > cap {
        return Err(format!("EAL array too large ({count})"));
    }
    Ok(count)
}
fn name(b: &[u8]) -> String {
    String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or_default()).into_owned()
}

// Some EAGLE exports (notably UT's DM-Morbias][) understate both gema and
// enclosing LIST/RIFF lengths. Derive only geometry's size from bounded counts;
// never allow the recovery to extend beyond the actual asset buffer.
fn geometry_size(bytes: &[u8]) -> Result<usize, String> {
    let sources = limit_count(number(bytes, 0)? as usize, MAX_ITEMS)?;
    let environments = limit_count(number(bytes, 4)? as usize, MAX_ITEMS)?;
    let nodes = limit_count(number(bytes, 12)? as usize, MAX_NODES)?;
    let leaves = limit_count(number(bytes, 16)? as usize, MAX_NODES)?;
    let size = 20usize
        .checked_add(sources * 16)
        .and_then(|v| v.checked_add(environments * 8))
        .and_then(|v| {
            (environments + 1)
                .checked_mul(environments + 1)
                .and_then(|n| n.checked_mul(4))
                .and_then(|n| v.checked_add(n))
        })
        .and_then(|v| v.checked_add(nodes * 44))
        .and_then(|v| v.checked_add(leaves * 8))
        .ok_or("EAL geometry size overflow")?;
    let boxes = limit_count(number(bytes, size)? as usize, MAX_ITEMS)?;
    let size = size
        .checked_add(4 + boxes * 28)
        .ok_or("EAL geometry size overflow")?;
    if size > bytes.len() {
        return Err("truncated EAL geometry".into());
    }
    Ok(size)
}
fn effective_chunk_size(tag: &[u8], declared: usize, available: &[u8]) -> Result<usize, String> {
    if tag == b"gema" {
        return Ok(declared.max(geometry_size(available)?));
    }
    if tag == b"LIST" && available.get(..4) == Some(b"gemp") {
        let mut at = 4usize;
        while at < declared {
            let header = available
                .get(at..at.checked_add(8).ok_or("EAL LIST overflow")?)
                .ok_or("short EAL LIST")?;
            let length = number(header, 4)? as usize;
            let payload = at + 8;
            if &header[..4] == b"gema" {
                let size = length.max(geometry_size(&available[payload..])?);
                return Ok(declared.max(
                    payload
                        .checked_add(size + (size & 1))
                        .ok_or("EAL LIST overflow")?,
                ));
            }
            at = payload
                .checked_add(length + (length & 1))
                .ok_or("EAL LIST overflow")?;
        }
    }
    Ok(declared)
}

/// Visit only fully-contained RIFF/LIST chunks. Chunk sizes are untrusted;
/// offsets are range-checked before advancing. The odd-byte padding is part
/// of RIFF, not part of each chunk's payload.
fn chunks(
    mut bytes: &[u8],
    mut visit: impl FnMut(&[u8; 4], &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    while !bytes.is_empty() {
        if bytes.len() < 8 {
            return Err("truncated EAL RIFF chunk header".into());
        }
        let tag: &[u8; 4] = bytes[..4].try_into().expect("four bytes");
        let len = effective_chunk_size(tag, number(bytes, 4)? as usize, &bytes[8..])?;
        let end = 8usize.checked_add(len).ok_or("EAL RIFF chunk overflow")?;
        if end > bytes.len() {
            return Err("truncated EAL RIFF chunk payload".into());
        }
        visit(tag, &bytes[8..end])?;
        let next = end
            .checked_add(len & 1)
            .ok_or("EAL chunk padding overflow")?;
        if next > bytes.len() {
            return Err("truncated EAL RIFF padding".into());
        }
        bytes = &bytes[next..];
    }
    Ok(())
}

fn parse_names(payload: &[u8], count: usize) -> Result<Vec<String>, String> {
    if payload.len() != count.checked_mul(32).ok_or("EAL name size overflow")? {
        return Err("EAL name count differs from nams size".into());
    }
    Ok(payload.chunks_exact(32).map(name).collect())
}

fn parse_presets(payload: &[u8], count: usize, stride: usize) -> Result<Vec<Vec<u32>>, String> {
    if payload.len()
        != count
            .checked_mul(stride)
            .ok_or("EAL preset size overflow")?
    {
        return Err("EAL preset count differs from lisp size".into());
    }
    Ok(payload
        .chunks_exact(stride)
        .map(|blob| {
            blob.chunks_exact(4)
                .map(|word| u32::from_le_bytes(word.try_into().expect("word")))
                .collect()
        })
        .collect())
}

fn parse_geometry(
    payload: &[u8],
) -> Result<(Vec<i32>, Vec<Split>, Vec<Zone>, u32, Vec<i32>), String> {
    if payload.len() < 24 {
        return Err("short EAL gema".into());
    }
    let sources = limit_count(number(payload, 0)? as usize, MAX_ITEMS)?;
    let envs = limit_count(number(payload, 4)? as usize, MAX_ITEMS)?;
    let start = number(payload, 8)?;
    let node_count = limit_count(number(payload, 12)? as usize, MAX_NODES)?;
    let leaf_count = limit_count(number(payload, 16)? as usize, MAX_NODES)?;
    if envs == 0 || node_count == 0 || leaf_count == 0 || leaf_count != node_count + 1 {
        return Err("invalid EAL acoustic partition counts".into());
    }
    let mut at = 20usize;
    let mut take = |bytes: usize| -> Result<usize, String> {
        let old = at;
        at = at.checked_add(bytes).ok_or("EAL gema offset overflow")?;
        if at > payload.len() {
            return Err("truncated EAL gema".into());
        }
        Ok(old)
    };
    // Source IDs, source XYZ, environment IDs, diffraction index array,
    // obstacle matrix, inner BSP, outer BSP, diffraction boxes.
    take(sources.checked_mul(16).ok_or("EAL sources size overflow")?)?;
    let ids_start = take(envs.checked_mul(4).ok_or("EAL IDs size overflow")?)?;
    let mapped_ids = (0..envs)
        .map(|i| signed(payload, ids_start + i * 4))
        .collect::<Result<Vec<_>, _>>()?;
    take(envs.checked_mul(4).ok_or("EAL diffraction size overflow")?)?;
    let matrix_count = (envs + 1)
        .checked_mul(envs + 1)
        .ok_or("EAL matrix overflow")?;
    let matrix_start = take(
        matrix_count
            .checked_mul(4)
            .ok_or("EAL matrix size overflow")?,
    )?;
    let matrix = (0..matrix_count)
        .map(|i| signed(payload, matrix_start + i * 4))
        .collect::<Result<Vec<_>, _>>()?;
    let nodes_start = take(node_count.checked_mul(44).ok_or("EAL tree size overflow")?)?;
    let mut nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let at = nodes_start + i * 44;
        nodes.push(Split {
            base: [
                scalar(payload, at)?,
                scalar(payload, at + 4)?,
                scalar(payload, at + 8)?,
            ],
            normal: [
                scalar(payload, at + 12)?,
                scalar(payload, at + 16)?,
                scalar(payload, at + 20)?,
            ],
            front: number(payload, at + 24)?,
            back: number(payload, at + 28)?,
        });
    }
    let leaves_start = take(leaf_count.checked_mul(8).ok_or("EAL leaf size overflow")?)?;
    let mut zones = Vec::with_capacity(leaf_count);
    for i in 0..leaf_count {
        let idx = signed(payload, leaves_start + i * 8 + 4)?;
        if idx < -2 || (idx >= 0 && idx as usize >= envs) {
            return Err("invalid EAL zone environment index".into());
        }
        zones.push(Zone {
            environment_index: idx,
        });
    }
    let boxes_start = take(4)?;
    let boxes = limit_count(number(payload, boxes_start)? as usize, MAX_ITEMS)?;
    take(boxes.checked_mul(28).ok_or("EAL boxes size overflow")?)?;
    if at != payload.len() {
        // EalMan accepts malformed geometry-size declarations in historical
        // files. We accept trailing bytes, but never read beyond the buffer.
    }
    let valid = |idx: u32| {
        if idx & 0x8000_0000 != 0 {
            ((idx & 0x7fff_ffff) as usize) < leaf_count
        } else {
            (idx as usize) < node_count
        }
    };
    if !valid(start)
        || nodes
            .iter()
            .any(|node| !valid(node.front) || !valid(node.back))
    {
        return Err("invalid EAL spatial-tree child index".into());
    }
    // Reject cycles at load time instead of doing up to a million iterations
    // every frame for a corrupt mod asset. Also bound authored tree depth.
    let mut marks = vec![0u8; nodes.len()];
    let mut pending = vec![(start, false, 0usize)];
    while let Some((child, leaving, depth)) = pending.pop() {
        if child & 0x8000_0000 != 0 {
            continue;
        }
        let index = child as usize;
        if leaving {
            marks[index] = 2;
            continue;
        }
        if marks[index] == 1 {
            return Err("cyclic EAL spatial tree".into());
        }
        if marks[index] == 2 {
            continue;
        }
        if depth >= 1024 {
            return Err("EAL spatial tree too deep".into());
        }
        marks[index] = 1;
        pending.push((child, true, depth));
        pending.push((nodes[index].back, false, depth + 1));
        pending.push((nodes[index].front, false, depth + 1));
    }
    Ok((mapped_ids, nodes, zones, start, matrix))
}

impl EalMap {
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        if data.len() > MAX_EAL_BYTES {
            return Err("EAL file exceeds 16 MiB limit".into());
        }
        if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"eal " {
            return Err("not a Creative EAGLE RIFF/eal file".into());
        }
        let mut riff_end = 8usize
            .checked_add(number(data, 4)? as usize)
            .ok_or("EAL RIFF overflow")?;
        if riff_end > data.len() || riff_end < 12 {
            return Err("invalid EAL RIFF size".into());
        }
        let mut at = 12usize;
        while at < riff_end {
            let header = data
                .get(at..at.checked_add(8).ok_or("EAL RIFF overflow")?)
                .ok_or("short EAL RIFF chunk")?;
            let declared = number(header, 4)? as usize;
            let size = effective_chunk_size(&header[..4], declared, &data[at + 8..])?;
            riff_end = riff_end
                .checked_add(size - declared)
                .ok_or("EAL RIFF overflow")?;
            if riff_end > data.len() {
                return Err("truncated EAL RIFF geometry".into());
            }
            at = at
                .checked_add(8 + size + (size & 1))
                .ok_or("EAL RIFF overflow")?;
        }
        let mut major = None;
        let mut minor = None;
        let mut default_bytes = None;
        let mut names = None;
        let mut presets = None;
        let mut geometry_sets = Vec::new();
        let mut geometry = None;
        let mut obstacles = Vec::new();
        let mut default_obstacle = EalObstruction::default();
        chunks(&data[12..riff_end], |tag, payload| {
            match tag {
                b"majv" => major = Some(number(payload, 0)?),
                b"minv" => minor = Some(number(payload, 0)?),
                b"denv" => default_bytes = Some(payload.to_vec()),
                b"dmat" => default_obstacle = EalObstruction::decode(payload)?,
                b"LIST" if payload.len() >= 4 && &payload[..4] == b"matp" => {
                    let mut count = None;
                    chunks(&payload[4..], |tag, content| {
                        match tag {
                            b"num " => {
                                count = Some(limit_count(number(content, 0)? as usize, MAX_ITEMS)?)
                            }
                            b"mata" => {
                                let n = count.ok_or("matp mata before num")?;
                                if content.len() != n * 16 {
                                    return Err("EAL material count mismatch".into());
                                }
                                obstacles = content
                                    .chunks_exact(16)
                                    .map(EalObstruction::decode)
                                    .collect::<Result<Vec<_>, _>>()?;
                            }
                            _ => (),
                        }
                        Ok(())
                    })?;
                }
                b"LIST" if payload.len() >= 4 && &payload[..4] == b"envp" => {
                    let mut count = None;
                    chunks(&payload[4..], |tag, content| {
                        match tag {
                            b"num " => {
                                count = Some(limit_count(number(content, 0)? as usize, MAX_ITEMS)?)
                            }
                            b"nams" => {
                                names = Some(parse_names(
                                    content,
                                    count.ok_or("envp nams before num")?,
                                )?)
                            }
                            b"lisp" => presets = Some(content.to_vec()),
                            _ => (),
                        }
                        Ok(())
                    })?;
                }
                b"LIST" if payload.len() >= 4 && &payload[..4] == b"gemp" => {
                    let mut count = None;
                    chunks(&payload[4..], |tag, content| {
                        match tag {
                            b"num " => {
                                count = Some(limit_count(number(content, 0)? as usize, MAX_ITEMS)?)
                            }
                            b"nams" => {
                                geometry_sets =
                                    parse_names(content, count.ok_or("gemp nams before num")?)?
                            }
                            b"gema" => geometry = Some(parse_geometry(content)?),
                            _ => (),
                        }
                        Ok(())
                    })?;
                }
                _ => (),
            }
            Ok(())
        })?;
        let version = (
            major.ok_or("EAL missing majv")?,
            minor.ok_or("EAL missing minv")?,
        );
        let stride = match version {
            (2, 1) => 56,
            (3, 0) => 112,
            _ => {
                return Err(format!(
                    "unsupported EAL version {}.{}",
                    version.0, version.1
                ))
            }
        };
        let default_bytes = default_bytes.ok_or("EAL missing default environment")?;
        let mut default_words = parse_presets(&default_bytes, 1, stride)?;
        let environment_names = names.ok_or("EAL missing environment names")?;
        let mut environment_words = parse_presets(
            &presets.ok_or("EAL missing lisp")?,
            environment_names.len(),
            stride,
        )?;
        let environments: Vec<EalPreset> = environment_names
            .into_iter()
            .zip(environment_words.drain(..))
            .map(|(name, eax_words)| EalPreset { name, eax_words })
            .collect();
        let (mapped_ids, nodes, zones, start, matrix) = geometry.ok_or("EAL missing geometry")?;
        if mapped_ids
            .iter()
            .any(|&id| id < -1 || (id >= 0 && id as usize >= environments.len()))
        {
            return Err("EAL geometry refers to undefined reverb preset".into());
        }
        if matrix
            .iter()
            .any(|&id| id < -2 || (id >= 0 && id as usize >= obstacles.len()))
        {
            return Err("EAL matrix refers to undefined obstacle".into());
        }
        if geometry_sets.len() != 1 {
            return Err("EAL requires exactly one geometry set (same limitation as EalMan)".into());
        }
        Ok(Self {
            raw: std::sync::Arc::from(data),
            version,
            default: EalPreset {
                name: "Default".into(),
                eax_words: default_words.remove(0),
            },
            environments,
            geometry_sets,
            mapped_ids,
            nodes,
            zones,
            start,
            matrix,
            obstacles,
            default_obstacle,
        })
    }

    pub fn raw_data(&self) -> &[u8] { &self.raw }

    /// Game-facing query. EAGLE stores X/right, Y/up, Z/depth; JKA is Z-up.
    /// Keep the raw query separate for native EalMan/oracle comparisons.
    pub fn environment_at_jka(&self, point: [f32; 3]) -> Option<EnvironmentAt<'_>> {
        self.environment_at([point[0], point[2], point[1]])
    }

    pub fn source_acoustics_jka(
        &self,
        listener: [f32; 3],
        source: [f32; 3],
    ) -> Option<EalSourceAcoustics> {
        self.source_acoustics(
            [listener[0], listener[2], listener[1]],
            [source[0], source[2], source[1]],
        )
    }

    /// Raw EAGLE-space query, matching EalMan's public coordinate convention.
    pub fn environment_at(&self, point: [f32; 3]) -> Option<EnvironmentAt<'_>> {
        if point
            .iter()
            .any(|c| !c.is_finite() || !(-32768.0..=32768.0).contains(c))
        {
            return None;
        }
        let mut child = self.start;
        // EalMan's front/back convention, including its fixed -1.0 tolerance.
        // Guard against malicious loops even after child-index validation.
        for _ in 0..=self.nodes.len() {
            if child & 0x8000_0000 != 0 {
                let zone_index = (child & 0x7fff_ffff) as usize;
                let zone = self.zones.get(zone_index)?;
                let id = if zone.environment_index < 0 {
                    -1
                } else {
                    *self.mapped_ids.get(zone.environment_index as usize)?
                };
                let preset = if id < 0 {
                    &self.default
                } else {
                    self.environments.get(id as usize)?
                };
                return Some(EnvironmentAt {
                    id,
                    zone_index,
                    preset,
                });
            }
            let node = self.nodes.get(child as usize)?;
            let signed_distance = (point[0] - node.base[0]) * node.normal[0]
                + (point[1] - node.base[1]) * node.normal[1]
                + (point[2] - node.base[2]) * node.normal[2];
            child = if signed_distance >= -1.0 {
                node.front
            } else {
                node.back
            };
        }
        None
    }
    /// EalMan GetSourceDynamicAttributes: room-to-room obstacle lookup.
    /// No diffraction is applied here; this is equivalent to
    /// EMFLAG_NODIFFRACTION. The native scene uses EalMan with diffraction
    /// enabled; this reader query is retained for validation and diagnostics.
    pub fn source_acoustics(
        &self,
        listener: [f32; 3],
        source: [f32; 3],
    ) -> Option<EalSourceAcoustics> {
        let a = self.environment_at(listener)?;
        let b = self.environment_at(source)?;
        let source_zone = self.zones.get(b.zone_index)?.environment_index.max(-1);
        let listener_zone = self.zones.get(a.zone_index)?.environment_index.max(-1);
        let n = self.mapped_ids.len() + 1;
        let idx = (listener_zone + 1) as usize * n + (source_zone + 1) as usize;
        let obstacle_index = *self.matrix.get(idx)?;
        let obstruction = match obstacle_index {
            -2 => EalObstruction::default(),
            -1 => self.default_obstacle,
            x if x >= 0 => *self.obstacles.get(x as usize)?,
            _ => return None,
        };
        Some(EalSourceAcoustics {
            obstruction,
            index: obstacle_index,
        })
    }

    pub fn preset_by_id(&self, id: i32) -> Option<&EalPreset> {
        if id == -1 {
            Some(&self.default)
        } else {
            self.environments.get(usize::try_from(id).ok()?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(tag: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::from(*tag);
        out.extend((bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
        if bytes.len() & 1 != 0 {
            out.push(0);
        }
        out
    }
    fn fixture() -> Vec<u8> {
        let mut envp = b"envp".to_vec();
        envp.extend(chunk(b"num ", &1u32.to_le_bytes()));
        let mut n = [0u8; 32];
        n[..4].copy_from_slice(b"Cave");
        envp.extend(chunk(b"nams", &n));
        envp.extend(chunk(b"lisp", &[0u8; 112]));
        let mut gema = Vec::new();
        for value in [0u32, 1, 0, 1, 2] {
            gema.extend_from_slice(&value.to_le_bytes());
        }
        gema.extend_from_slice(&0i32.to_le_bytes()); // environmental preset ID
        gema.extend_from_slice(&(-1i32).to_le_bytes()); // diffraction index
        for _ in 0..4 {
            gema.extend_from_slice(&(-2i32).to_le_bytes());
        } // obstacles matrix
          // Plane x=0, normal=(1,0,0), leaves 0 and 1.
        for f in [0f32, 0., 0., 1., 0., 0.] {
            gema.extend_from_slice(&f.to_le_bytes());
        }
        gema.extend_from_slice(&0x8000_0000u32.to_le_bytes());
        gema.extend_from_slice(&0x8000_0001u32.to_le_bytes());
        for _ in 0..3 {
            gema.extend_from_slice(&0u32.to_le_bytes());
        }
        for i in [0i32, 0i32, 0i32, -1i32] {
            gema.extend_from_slice(&i.to_le_bytes());
        }
        gema.extend_from_slice(&0u32.to_le_bytes());
        let mut gemp = b"gemp".to_vec();
        gemp.extend(chunk(b"num ", &1u32.to_le_bytes()));
        let mut geom_name = [0u8; 32];
        geom_name[..4].copy_from_slice(b"Main");
        gemp.extend(chunk(b"nams", &geom_name));
        gemp.extend(chunk(b"gema", &gema));
        let mut riff = b"eal ".to_vec();
        riff.extend(chunk(b"majv", &3u32.to_le_bytes()));
        riff.extend(chunk(b"minv", &0u32.to_le_bytes()));
        riff.extend(chunk(b"denv", &[0u8; 112]));
        riff.extend(chunk(b"LIST", &envp));
        riff.extend(chunk(b"LIST", &gemp));
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&(riff.len() as u32).to_le_bytes());
        file.extend(riff);
        file
    }
    #[test]
    fn zones_follow_eaxman_one_unit_plane_tolerance() {
        let map = EalMap::decode(&fixture()).unwrap();
        assert_eq!(map.version, (3, 0));
        assert_eq!(
            map.environment_at([2., 0., 0.]).unwrap().preset.name,
            "Cave"
        );
        assert_eq!(map.environment_at([-0.5, 0., 0.]).unwrap().id, 0);
        assert_eq!(map.environment_at([-2., 0., 0.]).unwrap().id, -1);
    }
    #[test]
    fn jka_queries_use_height_for_eagle_y_on_both_sides_of_the_obstacle_matrix() {
        let mut map = EalMap::decode(&fixture()).unwrap();
        // A horizontal acoustic partition: game height, not game depth,
        // determines the room. The matrix is deliberately asymmetric.
        map.nodes[0].normal = [0.0, 1.0, 0.0];
        map.matrix = vec![-2, -1, -2, -2];
        let upstairs = [0.0, -20.0, 20.0];
        let downstairs = [0.0, 20.0, -20.0];
        assert_eq!(map.environment_at_jka(upstairs).unwrap().id, 0);
        assert_eq!(map.environment_at_jka(downstairs).unwrap().id, -1);
        assert_eq!(map.source_acoustics_jka(downstairs, upstairs).unwrap().index, -1);
        assert_eq!(map.source_acoustics_jka(upstairs, downstairs).unwrap().index, -2);
    }
    #[test]
    fn invalid_child_reference_is_rejected() {
        let mut file = fixture();
        // Locate the chunk, then the known fixture's arrays and first node.
        // Searching for an unaligned integer also matches unrelated float bytes.
        let gema = file
            .windows(4)
            .position(|window| window == b"gema")
            .unwrap()
            + 8;
        let offset = gema + 20 + 4 + 4 + 16 + 24;
        file[offset..offset + 4].copy_from_slice(&0x8000_0099u32.to_le_bytes());
        assert!(EalMap::decode(&file).is_err());
    }
    #[test]
    fn cyclic_tree_is_rejected_at_load_time() {
        let mut file = fixture();
        let gema = file
            .windows(4)
            .position(|window| window == b"gema")
            .unwrap()
            + 8;
        let offset = gema + 20 + 4 + 4 + 16 + 24;
        file[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(EalMap::decode(&file).unwrap_err().contains("cyclic"));
    }
    #[test]
    fn historical_understated_geometry_lengths_are_recovered_within_buffer() {
        let mut file = fixture();
        let gema = file
            .windows(4)
            .position(|window| window == b"gema")
            .unwrap();
        let gemp = file
            .windows(4)
            .position(|window| window == b"gemp")
            .unwrap();
        for offset in [4, gema + 4, gemp - 4] {
            let size = number(&file, offset).unwrap();
            file[offset..offset + 4].copy_from_slice(&(size - 4).to_le_bytes());
        }
        assert!(EalMap::decode(&file).is_ok());
        file.truncate(gema + 16);
        assert!(EalMap::decode(&file).is_err());
    }
    #[test]
    fn correctly_uses_eal_riff_fourcc_and_rejects_old_typo() {
        let ok = fixture();
        assert!(EalMap::decode(&ok).is_ok());
        let mut broken = ok;
        broken[8..12].copy_from_slice(b"eam ");
        assert!(EalMap::decode(&broken).is_err());
    }
    #[test]
    fn default_obstruction_matrix_does_not_invent_occlusion() {
        let map = EalMap::decode(&fixture()).unwrap();
        let attrs = map
            .source_acoustics([2.0, 0.0, 0.0], [-2.0, 0.0, 0.0])
            .unwrap();
        assert_eq!(attrs.index, -2);
        assert_eq!(attrs.obstruction.level_mb, 0);
        assert!(!attrs.obstruction.obstructs);
    }
    #[test]
    fn truncation_is_rejected() {
        let mut file = fixture();
        file.pop();
        assert!(EalMap::decode(&file).is_err());
    }
}
