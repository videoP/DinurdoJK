//! RBSP version 1 data layout, checked against OpenJK's codemp/qcommon/qfiles.h.
//! Coordinates stay in JKA units with Z up. No GPU or protocol types appear here.
use std::fmt;
use std::ops::Range;
mod collision;
mod visibility;
pub use collision::{CollisionLeaf, CollisionNode, CollisionTree};
pub use visibility::Visibility;

/// Maximum accepted RBSP size. Large community maps can legitimately exceed 128 MiB
/// (especially with embedded lightmaps), while keeping a finite cap still protects
/// package reads from accidental/corrupt oversized entries.
pub const MAX_FILE_BYTES: usize = 512 * 1024 * 1024;
// JKA codemp/game/surfaceflags.h. The low five bits are MATERIAL_*;
// Quake 3's 0x4 SKY / 0x80 NODRAW values do NOT apply to RBSP.
pub const MATERIAL_MASK: u32 = 0x0000_001f;
pub const MATERIAL_SNOW: u32 = 14;
pub const CONTENTS_SOLID: u32 = 0x0000_0001;
pub const CONTENTS_TERRAIN: u32 = 0x0000_1000;
pub const SURF_SKY: u32 = 0x0000_2000;
pub const SURF_NODRAW: u32 = 0x0020_0000;
const HEADER_BYTES: usize = 152;
const MAX_MESH_VERTICES: usize = 1_048_576;
const MAX_MESH_INDICES: usize = 6_291_456;
const MAX_ACOUSTIC_VERTICES: usize = 4_194_304;
const MAX_ACOUSTIC_TRIANGLES: usize = 8_388_608;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RBSP: {}", self.0)
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;

fn invalid(message: impl Into<String>) -> Error {
    Error(message.into())
}

// All fixed-offset reads below operate on a validated header or exact-size record.
fn int(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn number(data: &[u8], offset: usize) -> Result<f32> {
    let value = f32::from_bits(int(data, offset) as u32);
    if !value.is_finite() {
        return Err(invalid("non-finite coordinate or attribute"));
    }
    Ok(value)
}

fn floats<const N: usize>(data: &[u8], offset: usize) -> Result<[f32; N]> {
    let mut values = [0.0; N];
    for (i, value) in values.iter_mut().enumerate() {
        *value = number(data, offset + i * 4)?;
    }
    Ok(values)
}

fn range(first: i32, count: i32, limit: usize, context: &str) -> Result<Range<usize>> {
    let start =
        usize::try_from(first).map_err(|_| invalid(format!("negative {context} offset")))?;
    let count = usize::try_from(count).map_err(|_| invalid(format!("negative {context} count")))?;
    let end = start
        .checked_add(count)
        .filter(|&end| end <= limit)
        .ok_or_else(|| invalid(format!("{context} range exceeds {limit}")))?;
    Ok(start..end)
}

fn index(value: i32, limit: usize, context: &str) -> Result<usize> {
    Ok(range(value, 1, limit, context)?.start)
}

#[derive(Debug, Clone)]
pub struct Shader {
    pub name: Vec<u8>,
    pub surface_flags: u32,
    pub contents: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    pub position: [f32; 3],
    pub texcoord: [f32; 2],
    pub lightmap_uv: [[f32; 2]; 4],
    pub normal: [f32; 3],
    pub color: [[u8; 4]; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Planar,
    Patch,
    Triangles,
    Flare,
}

#[derive(Debug, Clone)]
pub struct Surface {
    pub shader: usize,
    /// BSP fog volume assigned by the surface record, or -1 when unassigned.
    pub fog_num: i32,
    pub kind: SurfaceKind,
    pub vertices: Range<usize>,
    pub indices: Range<usize>,
    pub lightmaps: [i32; 4],
    pub lightmap_styles: [u8; 4],
    pub vertex_styles: [u8; 4],
    pub patch_size: [usize; 2],
}

#[derive(Debug, Clone)]
pub struct FogVolume {
    pub shader: Vec<u8>,
    pub brush_num: i32,
    pub visible_side: i32,
}

#[derive(Debug, Clone)]
pub struct Model {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub surfaces: Range<usize>,
    pub brushes: Range<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub normal: [f32; 3],
    pub distance: f32,
}

#[derive(Debug, Clone)]
pub struct Brush {
    pub sides: Range<usize>,
    pub shader: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct BrushSide {
    pub plane: usize,
    pub shader: usize,
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub properties: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Entity {
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.properties
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_slice())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Spawn {
    pub origin: [f32; 3],
    pub yaw: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct LightGridSample {
    pub ambient_light: [[u8; 3]; 4],
    pub direct_light: [[u8; 3]; 4],
    pub styles: [u8; 4],
    /// JKA/OpenJK longitude/latitude bytes as stored by dgrid_t.
    pub lat_long: [u8; 2],
}

#[derive(Debug, Clone)]
pub struct LightGrid {
    /// JKA-space grid origin.
    pub origin: [f32; 3],
    /// JKA-space spacing between samples.
    pub size: [f32; 3],
    /// Expanded X/Y/Z grid dimensions.
    pub bounds: [usize; 3],
    /// Deduplicated dgrid_t records from LUMP_LIGHTGRID.
    pub samples: Vec<LightGridSample>,
    /// One sample index per expanded grid cell from LUMP_LIGHTARRAY.
    pub cell_samples: Vec<u16>,
}

impl LightGrid {
    pub fn cell_count(&self) -> usize {
        self.bounds[0] * self.bounds[1] * self.bounds[2]
    }

    pub fn sample_for_cell(&self, cell: usize) -> Option<&LightGridSample> {
        let sample = usize::from(*self.cell_samples.get(cell)?);
        self.samples.get(sample)
    }
}

#[derive(Debug)]
pub struct Bsp {
    pub collision: Option<CollisionTree>,
    pub visibility: Option<Visibility>,
    pub shaders: Vec<Shader>,
    pub fogs: Vec<FogVolume>,
    pub vertices: Vec<Vertex>,
    /// Surface-local indices, as stored on disk.
    pub indices: Vec<u32>,
    pub surfaces: Vec<Surface>,
    pub models: Vec<Model>,
    pub planes: Vec<Plane>,
    pub brushes: Vec<Brush>,
    pub brush_sides: Vec<BrushSide>,
    pub entities: Vec<Entity>,
    /// 128 by 128 RGB pages; four style slots are retained on each surface.
    pub lightmaps: Vec<u8>,
    /// JKA RBSP lightgrid + light-array indirection used by Rend2-style
    /// directional/static lighting enhancements.
    pub light_grid: Option<LightGrid>,
}

impl Bsp {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < HEADER_BYTES || data.len() > MAX_FILE_BYTES {
            return Err(invalid(format!(
                "file size outside {HEADER_BYTES} bytes..{} MiB",
                MAX_FILE_BYTES / (1024 * 1024)
            )));
        }
        if &data[..4] != b"RBSP" || int(data, 4) != 1 {
            return Err(invalid("expected RBSP version 1"));
        }
        let mut lumps = Vec::with_capacity(18);
        for n in 0..18 {
            let span = range(
                int(data, 8 + n * 8),
                int(data, 12 + n * 8),
                data.len(),
                "lump",
            )?;
            if !span.is_empty() && span.start < HEADER_BYTES {
                return Err(invalid(format!("lump {n} overlaps header")));
            }
            for previous in &lumps {
                let previous: &Range<usize> = previous;
                if !span.is_empty()
                    && !previous.is_empty()
                    && span.start < previous.end
                    && previous.start < span.end
                {
                    return Err(invalid(format!("lump {n} overlaps another lump")));
                }
            }
            lumps.push(span);
        }
        // The validated file-size ceiling already bounds every lump allocation.
        // Do not impose old q3map2/OpenJK compile-time record-count limits here:
        // extended compilers such as WzMap can emit perfectly valid RBSP files
        // with more planes/brushes/brush sides than those historical constants.
        // Keep the on-disk record-size check and the detailed index/range checks
        // below; those are the actual format/safety requirements for this loader.
        let records = |lump: usize, stride: usize| -> Result<std::slice::ChunksExact<'_, u8>> {
            let bytes = &data[lumps[lump].clone()];
            if !bytes.len().is_multiple_of(stride) {
                return Err(invalid(format!("lump {lump}: invalid record size")));
            }
            Ok(bytes.chunks_exact(stride))
        };
        let mut shaders = Vec::new();
        for row in records(1, 72)? {
            let end = row[..64]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| invalid("unterminated shader name"))?;
            shaders.push(Shader {
                name: row[..end].to_vec(),
                surface_flags: int(row, 64) as u32,
                contents: int(row, 68) as u32,
            });
        }
        let mut fogs = Vec::new();
        for row in records(12, 72)? {
            let end = row[..64]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| invalid("unterminated fog shader name"))?;
            fogs.push(FogVolume {
                shader: row[..end].to_vec(),
                brush_num: int(row, 64),
                visible_side: int(row, 68),
            });
        }
        let mut vertices = Vec::new();
        for row in records(10, 80)? {
            let mut lightmap_uv = [[0.0; 2]; 4];
            let mut color = [[0; 4]; 4];
            for slot in 0..4 {
                // Unused style slots in retail maps contain arbitrary bits, even NaNs.
                // Validate these per surface once its active styles are known.
                lightmap_uv[slot] = std::array::from_fn(|axis| {
                    f32::from_bits(int(row, 20 + slot * 8 + axis * 4) as u32)
                });
                color[slot].copy_from_slice(&row[64 + slot * 4..68 + slot * 4]);
            }
            vertices.push(Vertex {
                position: floats(row, 0)?,
                texcoord: floats(row, 12)?,
                lightmap_uv,
                normal: floats(row, 52)?,
                color,
            });
        }
        let mut indices = Vec::new();
        for row in records(11, 4)? {
            indices.push(u32::try_from(int(row, 0)).map_err(|_| invalid("negative draw index"))?);
        }
        let lightmap_data = &data[lumps[14].clone()];
        // OpenJK's MAX_MAP_LIGHTING (8 MiB) is a historical map/compiler design
        // bound, not an RBSP runtime-format limit. The stock renderer derives the
        // number of pages directly from the lump length, and community maps can
        // legitimately exceed that old constant. Keep the actual format check:
        // embedded lightmaps must be whole 128x128 RGB pages.
        if !lightmap_data.len().is_multiple_of(128 * 128 * 3) {
            return Err(invalid("invalid lightmap page count"));
        }
        let lightmap_count = lightmap_data.len() / (128 * 128 * 3);
        let mut surfaces = Vec::new();
        for row in records(13, 148)? {
            let kind = match int(row, 8) {
                1 => SurfaceKind::Planar,
                2 => SurfaceKind::Patch,
                3 => SurfaceKind::Triangles,
                4 => SurfaceKind::Flare,
                value => return Err(invalid(format!("unsupported surface kind {value}"))),
            };
            let surface_vertices = range(
                int(row, 12),
                int(row, 16),
                vertices.len(),
                "surface vertices",
            )?;
            let surface_indices =
                range(int(row, 20), int(row, 24), indices.len(), "surface indices")?;
            if surface_indices.len() % 3 != 0
                || indices[surface_indices.clone()]
                    .iter()
                    .any(|&i| i as usize >= surface_vertices.len())
            {
                return Err(invalid("invalid surface triangle indices"));
            }
            let mut lightmaps = [0; 4];
            for (slot, lightmap) in lightmaps.iter_mut().enumerate() {
                *lightmap = int(row, 36 + slot * 4);
                if *lightmap >= 0 {
                    // A zero-length RBSP lightmap lump is valid for maps compiled
                    // with q3map2 external lightmaps. In that mode surface lightmap
                    // indices name maps/<map>/lm_%04d.tga pages instead of embedded
                    // 128x128 pages. Keep strict range validation whenever embedded
                    // pages are actually present.
                    if lightmap_count > 0 {
                        index(*lightmap, lightmap_count, "surface lightmap")?;
                    }
                    if row[28 + slot] < 254
                        && vertices[surface_vertices.clone()].iter().any(|vertex| {
                            vertex.lightmap_uv[slot]
                                .iter()
                                .any(|value| !value.is_finite())
                        })
                    {
                        return Err(invalid("non-finite active lightmap coordinate"));
                    }
                }
            }
            let patch_size = if kind == SurfaceKind::Patch {
                let width = int(row, 140);
                let height = int(row, 144);
                if !(3..=31).contains(&width)
                    || !(3..=31).contains(&height)
                    || width % 2 != 1
                    || height % 2 != 1
                    || (width * height) as usize != surface_vertices.len()
                {
                    return Err(invalid("invalid quadratic patch control grid"));
                }
                [width as usize, height as usize]
            } else {
                [0, 0]
            };
            surfaces.push(Surface {
                shader: index(int(row, 0), shaders.len(), "surface shader")?,
                fog_num: int(row, 4),
                kind,
                vertices: surface_vertices,
                indices: surface_indices,
                lightmaps,
                lightmap_styles: row[28..32].try_into().unwrap(),
                vertex_styles: row[32..36].try_into().unwrap(),
                patch_size,
            });
        }
        let mut planes = Vec::new();
        for row in records(2, 16)? {
            planes.push(Plane {
                normal: floats(row, 0)?,
                distance: number(row, 12)?,
            });
        }
        let mut brush_sides = Vec::new();
        for row in records(9, 12)? {
            brush_sides.push(BrushSide {
                plane: index(int(row, 0), planes.len(), "brush plane")?,
                shader: index(int(row, 4), shaders.len(), "brush side shader")?,
            });
        }
        let mut brushes = Vec::new();
        for row in records(8, 12)? {
            brushes.push(Brush {
                sides: range(int(row, 0), int(row, 4), brush_sides.len(), "brush sides")?,
                shader: index(int(row, 8), shaders.len(), "brush shader")?,
            });
        }
        let mut models = Vec::new();
        for row in records(7, 40)? {
            let mins = floats::<3>(row, 0)?;
            let maxs = floats::<3>(row, 12)?;
            if mins.iter().zip(maxs).any(|(&min, max)| min > max) {
                return Err(invalid("reversed model bounds"));
            }
            models.push(Model {
                mins,
                maxs,
                surfaces: range(int(row, 24), int(row, 28), surfaces.len(), "model surfaces")?,
                brushes: range(int(row, 32), int(row, 36), brushes.len(), "model brushes")?,
            });
        }
        if models.is_empty() {
            return Err(invalid("missing world model"));
        }
        let entities = parse_entities(&data[lumps[0].clone()])?;
        let light_grid = parse_light_grid(
            &data[lumps[15].clone()],
            &data[lumps[17].clone()],
            &entities,
            &models[0],
        )?;
        let collision = CollisionTree::parse(
            &data[lumps[3].clone()],
            &data[lumps[4].clone()],
            &data[lumps[5].clone()],
            &data[lumps[6].clone()],
            planes.len(),
            surfaces.len(),
            brushes.len(),
        )?;
        let visibility = Visibility::parse(
            &data[lumps[3].clone()],
            &data[lumps[4].clone()],
            &data[lumps[5].clone()],
            &data[lumps[16].clone()],
            &planes,
            surfaces.len(),
        )?;
        Ok(Self {
            collision,
            visibility,
            shaders,
            fogs,
            vertices,
            indices,
            surfaces,
            models,
            planes,
            brushes,
            brush_sides,
            entities,
            lightmaps: lightmap_data.to_vec(),
            light_grid,
        })
    }

    /// Initial FFA spawn candidates, without game-side spawn selection rules.
    pub fn deathmatch_spawns(&self) -> Vec<Spawn> {
        self.entities
            .iter()
            .filter_map(|entity| {
                if entity.get(b"classname")? != b"info_player_deathmatch" {
                    return None;
                }
                let origin = std::str::from_utf8(entity.get(b"origin")?).ok()?;
                let coordinates: Vec<f32> = origin
                    .split_whitespace()
                    .map(str::parse)
                    .collect::<std::result::Result<_, _>>()
                    .ok()?;
                let origin: [f32; 3] = coordinates.try_into().ok()?;
                let yaw: f32 = match entity.get(b"angle") {
                    Some(value) => std::str::from_utf8(value).ok()?.parse().ok()?,
                    None => 0.0,
                };
                (origin.iter().all(|x| x.is_finite()) && yaw.is_finite())
                    .then_some(Spawn { origin, yaw })
            })
            .collect()
    }

    /// Geometry for a development renderer. Fixed patch subdivision is visual only;
    /// this does not implement OpenJK patch collision or its adaptive LOD/stitching.
    pub fn world_mesh(&self, patch_subdivisions: usize) -> Result<Mesh> {
        let mut mesh = Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            batches: Vec::new(),
        };
        self.append_model_mesh(&mut mesh, 0, patch_subdivisions)?;
        Ok(mesh)
    }

    /// Drawable geometry of every inline model (`*1`..`*N`: doors, lifts,
    /// breakables), tessellated exactly like the world. The second vector maps
    /// each mesh batch to its inline model index. Vertices stay at their
    /// compiled positions, which for an origin-brush model are relative to the
    /// entity origin, as OpenJK's R_AddBrushModelSurfaces expects.
    pub fn inline_models_mesh(&self, patch_subdivisions: usize) -> Result<(Mesh, Vec<u32>)> {
        let mut mesh = Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            batches: Vec::new(),
        };
        let mut batch_models = Vec::new();
        for model in 1..self.models.len() {
            self.append_model_mesh(&mut mesh, model, patch_subdivisions)?;
            batch_models.resize(mesh.batches.len(), model as u32);
        }
        Ok((mesh, batch_models))
    }

    /// Static world geometry for acoustic ray tracing.
    ///
    /// This is intentionally independent of [`Self::world_mesh`]. The render
    /// mesh omits `SURF_NODRAW`, while an invisible caulk/nodraw brush can still
    /// be a physically solid wall and must therefore block sound. Structural
    /// brush faces are reconstructed directly from the RBSP brush planes and
    /// carry the JKA material id from each individual brush side. Curved patch
    /// and triangle-soup collision surfaces are appended separately.
    pub fn world_acoustic_mesh(&self, patch_subdivisions: usize) -> Result<AcousticMesh> {
        if !(1..=16).contains(&patch_subdivisions) {
            return Err(invalid("patch subdivisions must be 1..16"));
        }
        let world = self
            .models
            .first()
            .ok_or_else(|| invalid("RBSP has no world model"))?;
        let mut mesh = AcousticMesh::default();
        // The initial clipping quad must cover worlds that are offset far away
        // from the JKA origin as well as worlds centered around it.
        let extent = world
            .mins
            .iter()
            .chain(world.maxs.iter())
            .map(|value| value.abs())
            .fold(0.0_f32, f32::max)
            .max(1024.0)
            * 2.0;

        for brush_index in world.brushes.clone() {
            let brush = &self.brushes[brush_index];
            let brush_shader = self
                .shaders
                .get(brush.shader)
                .ok_or_else(|| invalid("brush shader index out of range"))?;
            if brush_shader.contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) == 0 {
                continue;
            }
            for side_index in brush.sides.clone() {
                let side = self
                    .brush_sides
                    .get(side_index)
                    .ok_or_else(|| invalid("brush side index out of range"))?;
                let polygon = brush_side_polygon(self, brush, side_index, extent)?;
                if polygon.len() < 3 {
                    continue;
                }
                mesh.reserve(polygon.len(), polygon.len().saturating_sub(2))?;
                let base = u32::try_from(mesh.vertices.len())
                    .map_err(|_| invalid("acoustic mesh vertex index overflow"))?;
                mesh.vertices.extend_from_slice(&polygon);
                let material = self
                    .shaders
                    .get(side.shader)
                    .ok_or_else(|| invalid("brush-side shader index out of range"))?
                    .surface_flags
                    & MATERIAL_MASK;
                for index in 1..polygon.len() - 1 {
                    mesh.triangles.push([
                        base,
                        base + index as u32,
                        base + index as u32 + 1,
                    ]);
                    mesh.materials.push(material as u8);
                }
            }
        }

        // Patches and triangle soups are not necessarily backed by convex BSP
        // brushes. Include solid/terrain authored surfaces so curved architecture
        // and terrain participate in reflection/occlusion baking as well.
        for surface_index in world.surfaces.clone() {
            let surface = &self.surfaces[surface_index];
            if !matches!(surface.kind, SurfaceKind::Patch | SurfaceKind::Triangles) {
                continue;
            }
            let shader = self
                .shaders
                .get(surface.shader)
                .ok_or_else(|| invalid("surface shader index out of range"))?;
            if shader.contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) == 0 {
                continue;
            }
            let material = (shader.surface_flags & MATERIAL_MASK) as u8;
            match surface.kind {
                SurfaceKind::Patch => {
                    let [width, height] = surface.patch_size;
                    let controls = &self.vertices[surface.vertices.clone()];
                    let n = patch_subdivisions;
                    for y in (0..height - 2).step_by(2) {
                        for x in (0..width - 2).step_by(2) {
                            mesh.reserve((n + 1) * (n + 1), n * n * 2)?;
                            let base = u32::try_from(mesh.vertices.len())
                                .map_err(|_| invalid("acoustic mesh vertex index overflow"))?;
                            for v in 0..=n {
                                for u in 0..=n {
                                    mesh.vertices.push(
                                        patch_vertex(
                                            controls,
                                            width,
                                            x,
                                            y,
                                            u as f32 / n as f32,
                                            v as f32 / n as f32,
                                        )
                                        .position,
                                    );
                                }
                            }
                            for v in 0..n {
                                for u in 0..n {
                                    let a = base + (v * (n + 1) + u) as u32;
                                    let b = a + 1;
                                    let c = a + (n + 1) as u32;
                                    mesh.triangles.extend_from_slice(&[[a, c, b], [b, c, c + 1]]);
                                    mesh.materials.extend_from_slice(&[material, material]);
                                }
                            }
                        }
                    }
                }
                SurfaceKind::Triangles => {
                    mesh.reserve(surface.vertices.len(), surface.indices.len() / 3)?;
                    let base = u32::try_from(mesh.vertices.len())
                        .map_err(|_| invalid("acoustic mesh vertex index overflow"))?;
                    mesh.vertices.extend(
                        self.vertices[surface.vertices.clone()]
                            .iter()
                            .map(|vertex| vertex.position),
                    );
                    let indices = &self.indices[surface.indices.clone()];
                    for triangle in indices.chunks_exact(3) {
                        mesh.triangles.push([
                            base + triangle[0],
                            base + triangle[1],
                            base + triangle[2],
                        ]);
                        mesh.materials.push(material);
                    }
                }
                _ => unreachable!(),
            }
        }

        debug_assert_eq!(mesh.triangles.len(), mesh.materials.len());
        Ok(mesh)
    }

    fn append_model_mesh(&self, mesh: &mut Mesh, model: usize, patch_subdivisions: usize) -> Result<()> {
        if !(1..=16).contains(&patch_subdivisions) {
            return Err(invalid("patch subdivisions must be 1..16"));
        }
        let surfaces = self
            .models
            .get(model)
            .ok_or_else(|| invalid("inline model index out of range"))?
            .surfaces
            .clone();
        for surface_index in surfaces {
            let surface = &self.surfaces[surface_index];
            if self.shaders[surface.shader].surface_flags & SURF_NODRAW != 0
                || surface.kind == SurfaceKind::Flare
            {
                continue;
            }
            let start = mesh.indices.len();
            if surface.kind == SurfaceKind::Patch {
                let [width, height] = surface.patch_size;
                let controls = &self.vertices[surface.vertices.clone()];
                let n = patch_subdivisions;
                for y in (0..height - 2).step_by(2) {
                    for x in (0..width - 2).step_by(2) {
                        mesh.reserve((n + 1) * (n + 1), n * n * 6)?;
                        let base = mesh.vertices.len() as u32;
                        for v in 0..=n {
                            for u in 0..=n {
                                mesh.vertices.push(render_vertex(
                                    patch_vertex(
                                        controls,
                                        width,
                                        x,
                                        y,
                                        u as f32 / n as f32,
                                        v as f32 / n as f32,
                                    ),
                                    surface,
                                ));
                            }
                        }
                        for v in 0..n {
                            for u in 0..n {
                                let a = base + (v * (n + 1) + u) as u32;
                                let b = a + 1;
                                let c = a + (n + 1) as u32;
                                // Same control-grid winding as the OpenJK surface tessellator.
                                mesh.indices.extend_from_slice(&[a, c, b, b, c, c + 1]);
                            }
                        }
                    }
                }
            } else {
                mesh.reserve(surface.vertices.len(), surface.indices.len())?;
                let base = mesh.vertices.len() as u32;
                mesh.vertices.extend(
                    self.vertices[surface.vertices.clone()]
                        .iter()
                        .map(|&vertex| render_vertex(vertex, surface)),
                );
                mesh.indices.extend(
                    self.indices[surface.indices.clone()]
                        .iter()
                        .map(|i| base + i),
                );
            }
            if mesh.indices.len() > start {
                mesh.batches.push(DrawBatch {
                    surface: surface_index,
                    shader: surface.shader,
                    lightmaps: surface.lightmaps,
                    lightmap_styles: surface.lightmap_styles,
                    indices: start..mesh.indices.len(),
                });
            }
        }
        Ok(())
    }
}

fn brush_side_polygon(
    bsp: &Bsp,
    brush: &Brush,
    side_index: usize,
    extent: f32,
) -> Result<Vec<[f32; 3]>> {
    let side = bsp
        .brush_sides
        .get(side_index)
        .ok_or_else(|| invalid("brush side index out of range"))?;
    let plane = *bsp
        .planes
        .get(side.plane)
        .ok_or_else(|| invalid("brush-side plane index out of range"))?;
    let n = plane.normal;
    let length_sq = n.iter().map(|value| value * value).sum::<f32>();
    if length_sq < 0.5 {
        return Ok(Vec::new());
    }
    let helper = if n[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let tangent = normalize3(cross3(helper, n));
    let bitangent = normalize3(cross3(n, tangent));
    let center = n.map(|value| value * plane.distance / length_sq);
    let add = |a: [f32; 3], b: [f32; 3], scale: f32| {
        std::array::from_fn(|axis| a[axis] + b[axis] * scale)
    };
    let mut polygon = vec![
        add(add(center, tangent, -extent), bitangent, -extent),
        add(add(center, tangent, extent), bitangent, -extent),
        add(add(center, tangent, extent), bitangent, extent),
        add(add(center, tangent, -extent), bitangent, extent),
    ];

    // RBSP brush planes point outward. Keep the half-space on or behind each
    // plane (dot(normal, point) <= distance), matching CM brush construction.
    for other_side_index in brush.sides.clone() {
        if other_side_index == side_index || polygon.len() < 3 {
            continue;
        }
        let other_side = &bsp.brush_sides[other_side_index];
        let other = bsp.planes[other_side.plane];
        polygon = clip_polygon_to_plane(&polygon, other, 0.05);
    }
    Ok(polygon)
}

fn clip_polygon_to_plane(points: &[[f32; 3]], plane: Plane, epsilon: f32) -> Vec<[f32; 3]> {
    let mut result = Vec::with_capacity(points.len() + 4);
    for index in 0..points.len() {
        let current = points[index];
        let next = points[(index + 1) % points.len()];
        let current_distance = dot3(plane.normal, current) - plane.distance;
        let next_distance = dot3(plane.normal, next) - plane.distance;
        let current_inside = current_distance <= epsilon;
        let next_inside = next_distance <= epsilon;
        if current_inside {
            result.push(current);
        }
        if current_inside != next_inside {
            let denominator = current_distance - next_distance;
            if denominator.abs() > 1.0e-8 {
                let t = (current_distance / denominator).clamp(0.0, 1.0);
                result.push(std::array::from_fn(|axis| {
                    current[axis] + (next[axis] - current[axis]) * t
                }));
            }
        }
    }
    result
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(value: [f32; 3]) -> [f32; 3] {
    let length = value.iter().map(|value| value * value).sum::<f32>().sqrt();
    if length <= 1.0e-8 {
        [0.0; 3]
    } else {
        value.map(|value| value / length)
    }
}

#[derive(Debug)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub batches: Vec<DrawBatch>,
}

/// Static acoustic triangle soup in native JKA coordinates. `materials` is
/// parallel to `triangles` and stores the low-five-bit `MATERIAL_*` id from
/// OpenJK/JKA `surfaceflags.h` for later conversion to Steam Audio materials.
#[derive(Debug, Clone, Default)]
pub struct AcousticMesh {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub materials: Vec<u8>,
}

impl AcousticMesh {
    fn reserve(&mut self, vertices: usize, triangles: usize) -> Result<()> {
        if vertices > MAX_ACOUSTIC_VERTICES.saturating_sub(self.vertices.len())
            || triangles > MAX_ACOUSTIC_TRIANGLES.saturating_sub(self.triangles.len())
        {
            return Err(invalid("acoustic mesh exceeds safety limit"));
        }
        self.vertices.reserve(vertices);
        self.triangles.reserve(triangles);
        self.materials.reserve(triangles);
        Ok(())
    }
}

impl Mesh {
    fn reserve(&mut self, vertices: usize, indices: usize) -> Result<()> {
        if vertices > MAX_MESH_VERTICES - self.vertices.len()
            || indices > MAX_MESH_INDICES - self.indices.len()
        {
            return Err(invalid(
                "tessellated mesh exceeds development memory budget",
            ));
        }
        self.vertices.reserve(vertices);
        self.indices.reserve(indices);
        Ok(())
    }
}

#[derive(Debug)]
pub struct DrawBatch {
    pub surface: usize,
    pub shader: usize,
    pub lightmaps: [i32; 4],
    pub lightmap_styles: [u8; 4],
    pub indices: Range<usize>,
}

fn render_vertex(mut vertex: Vertex, surface: &Surface) -> Vertex {
    for slot in 0..4 {
        if surface.lightmaps[slot] < 0 || surface.lightmap_styles[slot] >= 254 {
            vertex.lightmap_uv[slot] = [0.0; 2];
        }
    }
    vertex
}

fn patch_vertex(controls: &[Vertex], width: usize, x: usize, y: usize, u: f32, v: f32) -> Vertex {
    let basis = |t: f32| [(1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t), t * t];
    let mut result = Vertex {
        position: [0.0; 3],
        texcoord: [0.0; 2],
        lightmap_uv: [[0.0; 2]; 4],
        normal: [0.0; 3],
        color: [[0; 4]; 4],
    };
    let mut colors = [[0.0; 4]; 4];
    for (dy, wy) in basis(v).into_iter().enumerate() {
        for (dx, wx) in basis(u).into_iter().enumerate() {
            let weight = wx * wy;
            let vertex = &controls[(y + dy) * width + x + dx];
            for axis in 0..3 {
                result.position[axis] += vertex.position[axis] * weight;
                result.normal[axis] += vertex.normal[axis] * weight;
            }
            for axis in 0..2 {
                result.texcoord[axis] += vertex.texcoord[axis] * weight;
            }
            for (slot, color) in colors.iter_mut().enumerate() {
                for axis in 0..2 {
                    result.lightmap_uv[slot][axis] += vertex.lightmap_uv[slot][axis] * weight;
                }
                for (channel, value) in color.iter_mut().enumerate() {
                    *value += f32::from(vertex.color[slot][channel]) * weight;
                }
            }
        }
    }
    let length = result.normal.iter().map(|n| n * n).sum::<f32>().sqrt();
    if length > 0.0 {
        for value in &mut result.normal {
            *value /= length;
        }
    }
    for (slot, color) in colors.iter().enumerate() {
        for (channel, value) in color.iter().enumerate() {
            result.color[slot][channel] = value.round().clamp(0.0, 255.0) as u8;
        }
    }
    result
}

fn parse_light_grid(
    grid_bytes: &[u8],
    array_bytes: &[u8],
    entities: &[Entity],
    world: &Model,
) -> Result<Option<LightGrid>> {
    if grid_bytes.is_empty() {
        return Ok(None);
    }
    if !grid_bytes.len().is_multiple_of(30) {
        return Err(invalid("light grid lump has invalid dgrid_t record size"));
    }

    let size = entities
        .first()
        .and_then(|entity| entity.get(b"gridsize"))
        .and_then(parse_grid_size)
        .unwrap_or([64.0, 64.0, 128.0]);
    let mut origin = [0.0; 3];
    let mut bounds = [0usize; 3];
    for axis in 0..3 {
        origin[axis] = size[axis] * (world.mins[axis] / size[axis]).ceil();
        let maximum = size[axis] * (world.maxs[axis] / size[axis]).floor();
        let span = ((maximum - origin[axis]) / size[axis]).round();
        if !span.is_finite() || span < 0.0 {
            return Err(invalid("invalid light grid bounds"));
        }
        bounds[axis] = span as usize + 1;
    }
    let cell_count = bounds
        .into_iter()
        .try_fold(1usize, |count, axis| count.checked_mul(axis))
        .ok_or_else(|| invalid("light grid cell count overflow"))?;

    let mut samples = Vec::with_capacity(grid_bytes.len() / 30);
    for row in grid_bytes.chunks_exact(30) {
        let mut ambient_light = [[0u8; 3]; 4];
        let mut direct_light = [[0u8; 3]; 4];
        for style in 0..4 {
            ambient_light[style].copy_from_slice(&row[style * 3..style * 3 + 3]);
            direct_light[style].copy_from_slice(&row[12 + style * 3..15 + style * 3]);
        }
        samples.push(LightGridSample {
            ambient_light,
            direct_light,
            styles: row[24..28].try_into().unwrap(),
            lat_long: row[28..30].try_into().unwrap(),
        });
    }
    if samples.len() > u16::MAX as usize + 1 {
        return Err(invalid(
            "light grid sample count exceeds JKA u16 light-array range",
        ));
    }

    let cell_samples = if array_bytes.is_empty() {
        if samples.len() != cell_count {
            return Err(invalid(format!(
                "light grid array missing: {} cells but {} samples",
                cell_count,
                samples.len()
            )));
        }
        (0..cell_count).map(|index| index as u16).collect()
    } else {
        if array_bytes.len() != cell_count * 2 {
            return Err(invalid(format!(
                "light grid array mismatch: {} bytes for {} cells",
                array_bytes.len(),
                cell_count
            )));
        }
        let mut indices = Vec::with_capacity(cell_count);
        for row in array_bytes.chunks_exact(2) {
            let sample = u16::from_le_bytes(row.try_into().unwrap());
            if usize::from(sample) >= samples.len() {
                return Err(invalid(
                    "light grid array references missing dgrid_t sample",
                ));
            }
            indices.push(sample);
        }
        indices
    };

    Ok(Some(LightGrid {
        origin,
        size,
        bounds,
        samples,
        cell_samples,
    }))
}

fn parse_grid_size(bytes: &[u8]) -> Option<[f32; 3]> {
    let text = std::str::from_utf8(bytes).ok()?;
    let values = text
        .split_whitespace()
        .map(str::parse::<f32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()?;
    if values.len() != 3
        || values
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return None;
    }
    Some([values[0], values[1], values[2]])
}

fn parse_entities(data: &[u8]) -> Result<Vec<Entity>> {
    if data.len() > 0x40000 {
        return Err(invalid("entity text exceeds limit"));
    }
    let mut cursor = 0;
    let mut entities = Vec::new();
    loop {
        skip_space(data, &mut cursor);
        if cursor == data.len() || data[cursor..].iter().all(|&b| b == 0) {
            break;
        }
        if data[cursor] != b'{' {
            return Err(invalid("expected entity opening brace"));
        }
        cursor += 1;
        if entities.len() >= 2048 {
            return Err(invalid("too many entities"));
        }
        let mut properties = Vec::new();
        loop {
            skip_space(data, &mut cursor);
            if data.get(cursor) == Some(&b'}') {
                cursor += 1;
                break;
            }
            let key = quoted(data, &mut cursor, 64)?;
            skip_space(data, &mut cursor);
            let value = quoted(data, &mut cursor, 4096)?;
            if properties.len() >= 256 {
                return Err(invalid("too many entity properties"));
            }
            properties.push((key, value));
        }
        entities.push(Entity { properties });
    }
    Ok(entities)
}

fn skip_space(data: &[u8], cursor: &mut usize) {
    loop {
        while data.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
            *cursor += 1;
        }
        if data.get(*cursor..*cursor + 2) != Some(b"//") {
            break;
        }
        while data.get(*cursor).is_some_and(|&b| b != b'\n') {
            *cursor += 1;
        }
    }
}

fn quoted(data: &[u8], cursor: &mut usize, limit: usize) -> Result<Vec<u8>> {
    if data.get(*cursor) != Some(&b'"') {
        return Err(invalid("expected quoted entity key/value"));
    }
    *cursor += 1;
    let start = *cursor;
    while let Some(&byte) = data.get(*cursor) {
        if byte == b'"' {
            let value = data[start..*cursor].to_vec();
            *cursor += 1;
            return Ok(value);
        }
        if byte == 0 || *cursor - start >= limit {
            return Err(invalid("invalid entity key/value length"));
        }
        *cursor += 1;
    }
    Err(invalid("unterminated entity key/value"))
}


#[cfg(test)]
mod acoustic_tests {
    use super::*;

    fn cube_bsp(surface_flags: u32, contents: u32) -> Bsp {
        let planes = vec![
            Plane { normal: [1.0, 0.0, 0.0], distance: 1.0 },
            Plane { normal: [-1.0, 0.0, 0.0], distance: 1.0 },
            Plane { normal: [0.0, 1.0, 0.0], distance: 1.0 },
            Plane { normal: [0.0, -1.0, 0.0], distance: 1.0 },
            Plane { normal: [0.0, 0.0, 1.0], distance: 1.0 },
            Plane { normal: [0.0, 0.0, -1.0], distance: 1.0 },
        ];
        let brush_sides = (0..6)
            .map(|plane| BrushSide { plane, shader: 0 })
            .collect();
        Bsp {
            collision: None,
            visibility: None,
            shaders: vec![Shader {
                name: b"textures/test/acoustic_cube".to_vec(),
                surface_flags,
                contents,
            }],
            fogs: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            surfaces: Vec::new(),
            models: vec![Model {
                mins: [-1.0; 3],
                maxs: [1.0; 3],
                surfaces: 0..0,
                brushes: 0..1,
            }],
            planes,
            brushes: vec![Brush { sides: 0..6, shader: 0 }],
            brush_sides,
            entities: Vec::new(),
            lightmaps: Vec::new(),
            light_grid: None,
        }
    }

    #[test]
    fn acoustic_mesh_keeps_nodraw_solid_brushes() {
        let material = 7;
        let bsp = cube_bsp(SURF_NODRAW | material, CONTENTS_SOLID);
        let mesh = bsp.world_acoustic_mesh(4).expect("acoustic cube");
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.triangles.len(), 12);
        assert_eq!(mesh.materials, vec![material as u8; 12]);
    }

    #[test]
    fn acoustic_mesh_ignores_non_solid_brushes() {
        let bsp = cube_bsp(7, 0);
        let mesh = bsp.world_acoustic_mesh(4).expect("non-solid cube");
        assert!(mesh.vertices.is_empty());
        assert!(mesh.triangles.is_empty());
        assert!(mesh.materials.is_empty());
    }
}
