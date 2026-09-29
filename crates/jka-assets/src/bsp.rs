//! RBSP version 1 data layout, checked against OpenJK's codemp/qcommon/qfiles.h.
//! Coordinates stay in JKA units with Z up. No GPU or protocol types appear here.
use std::fmt;
use std::ops::Range;
mod collision;
mod visibility;
pub use collision::{CollisionLeaf, CollisionNode, CollisionTree};
pub use visibility::{AreaLocator, Visibility};

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
pub const SURF_NODLIGHT: u32 = 0x0080_0000;
const HEADER_BYTES: usize = 152;
const MAX_MESH_VERTICES: usize = 2_097_152;
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

    /// Geometry for the renderer. Patch control grids are pre-tessellated with
    /// OpenJK's curvature-error subdivision rule (`r_subdivisions` semantics).
    /// Runtime distance LOD/stitching is not represented by this flattened mesh.
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
                let grid = subdivide_patch_to_grid(
                    width,
                    height,
                    controls,
                    patch_subdivisions as f32,
                )?;
                let index_count = (grid.width - 1)
                    .checked_mul(grid.height - 1)
                    .and_then(|quads| quads.checked_mul(6))
                    .ok_or_else(|| invalid("patch index count overflow"))?;
                mesh.reserve_with_context(
                    grid.vertices.len(),
                    index_count,
                    &format!(
                        "surface {surface_index} Patch shader={} controls={}x{} -> grid={}x{}",
                        surface.shader, width, height, grid.width, grid.height
                    ),
                )?;
                let base = u32::try_from(mesh.vertices.len())
                    .map_err(|_| invalid("tessellated mesh vertex index overflow"))?;
                mesh.vertices.extend(
                    grid.vertices
                        .iter()
                        .copied()
                        .map(|vertex| render_vertex(vertex, surface)),
                );
                for y in 0..grid.height - 1 {
                    for x in 0..grid.width - 1 {
                        let a = base + (y * grid.width + x) as u32;
                        let b = a + 1;
                        let c = a + grid.width as u32;
                        // OpenJK RB_SurfaceGrid winding: top-left, bottom-left, top-right.
                        mesh.indices.extend_from_slice(&[a, c, b, b, c, c + 1]);
                    }
                }
            } else {
                mesh.reserve_with_context(
                    surface.vertices.len(),
                    surface.indices.len(),
                    &format!(
                        "surface {surface_index} {:?} shader={} source_vertices={} source_indices={}",
                        surface.kind,
                        surface.shader,
                        surface.vertices.len(),
                        surface.indices.len()
                    ),
                )?;
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

/// CONTENTS_PLAYERCLIP / MONSTERCLIP / BOTCLIP / SHOTCLIP.
pub const CONTENTS_CLIP_MASK: u32 = 0x0000_00f0;

/// Flat brush-volume geometry for debug overlays, in BSP (JKA) coordinates.
/// Vertices are not shared between faces; each face is a triangle fan plus a
/// closed outline loop so the renderer needs exactly two static draws.
#[derive(Debug, Clone, Default)]
pub struct DebugVolumeMesh {
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 4]>,
    pub triangle_indices: Vec<u32>,
    pub line_indices: Vec<u32>,
}

impl DebugVolumeMesh {
    pub fn is_empty(&self) -> bool {
        self.triangle_indices.is_empty()
    }

    fn push_polygon(&mut self, polygon: &[[f32; 3]], color: [u8; 4]) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(polygon);
        self.colors.extend(std::iter::repeat(color).take(polygon.len()));
        let count = polygon.len() as u32;
        for i in 1..count - 1 {
            self.triangle_indices.extend_from_slice(&[base, base + i, base + i + 1]);
        }
        for i in 0..count {
            self.line_indices.extend_from_slice(&[base + i, base + (i + 1) % count]);
        }
    }
}

/// Trigger volumes and clip-only brushes of one map.
#[derive(Debug, Clone, Default)]
pub struct DebugVolumes {
    pub triggers: DebugVolumeMesh,
    pub clips: DebugVolumeMesh,
}

fn trigger_color(classname: &[u8]) -> [u8; 4] {
    match classname {
        b"trigger_push" => [64, 255, 96, 255],
        b"trigger_teleport" => [190, 96, 255, 255],
        b"trigger_hurt" => [255, 64, 64, 255],
        b"trigger_multiple" => [64, 160, 255, 255],
        b"trigger_once" => [64, 255, 255, 255],
        b"trigger_always" => [170, 170, 170, 255],
        _ => [255, 220, 64, 255],
    }
}

fn clip_color(contents: u32) -> [u8; 4] {
    if contents & 0x10 != 0 {
        [255, 140, 40, 255] // player clip
    } else if contents & 0x80 != 0 {
        [255, 64, 200, 255] // shot clip
    } else {
        [255, 240, 80, 255] // monster / bot clip
    }
}

impl Bsp {
    /// Builds the trigger-volume and clip-brush overlay meshes once per map.
    ///
    /// Triggers are the brush models of entities whose classname starts with
    /// `trigger_`; clip brushes are any brush whose contents carry a clip flag.
    /// Only those brushes are triangulated, so the cost scales with the
    /// handful of clip/trigger brushes rather than the whole map.
    pub fn debug_volumes(&self) -> Result<DebugVolumes> {
        let world = self
            .models
            .first()
            .ok_or_else(|| invalid("RBSP has no world model"))?;
        let extent = world
            .mins
            .iter()
            .chain(world.maxs.iter())
            .map(|value| value.abs())
            .fold(0.0_f32, f32::max)
            .max(1024.0)
            * 2.0;
        let mut volumes = DebugVolumes::default();

        for entity in &self.entities {
            let Some(classname) = entity.get(b"classname") else { continue };
            if !classname.starts_with(b"trigger_") {
                continue;
            }
            let Some(model) = entity
                .get(b"model")
                .and_then(|value| value.strip_prefix(b"*"))
                .and_then(|digits| std::str::from_utf8(digits).ok())
                .and_then(|digits| digits.parse::<usize>().ok())
                .filter(|&index| index > 0)
                .and_then(|index| self.models.get(index))
            else {
                continue;
            };
            let color = trigger_color(classname);
            for brush_index in model.brushes.clone() {
                self.append_brush_debug_volume(&mut volumes.triggers, brush_index, extent, color)?;
            }
        }

        for (brush_index, brush) in self.brushes.iter().enumerate() {
            let Some(shader) = self.shaders.get(brush.shader) else { continue };
            if shader.contents & CONTENTS_CLIP_MASK == 0 || shader.contents & CONTENTS_TERRAIN != 0 {
                continue;
            }
            self.append_brush_debug_volume(
                &mut volumes.clips,
                brush_index,
                extent,
                clip_color(shader.contents),
            )?;
        }
        Ok(volumes)
    }

    fn append_brush_debug_volume(
        &self,
        mesh: &mut DebugVolumeMesh,
        brush_index: usize,
        extent: f32,
        color: [u8; 4],
    ) -> Result<()> {
        let brush = self
            .brushes
            .get(brush_index)
            .ok_or_else(|| invalid("brush index out of range"))?;
        for side_index in brush.sides.clone() {
            let polygon = brush_side_polygon(self, brush, side_index, extent)?;
            if polygon.len() >= 3 {
                mesh.push_polygon(&polygon, color);
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
    fn reserve_with_context(&mut self, vertices: usize, indices: usize, context: &str) -> Result<()> {
        let current_vertices = self.vertices.len();
        let current_indices = self.indices.len();
        let vertex_overflow = vertices > MAX_MESH_VERTICES - current_vertices;
        let index_overflow = indices > MAX_MESH_INDICES - current_indices;
        if vertex_overflow || index_overflow {
            return Err(invalid(format!(
                "tessellated mesh exceeds development memory budget ({context}; current={current_vertices} vertices/{current_indices} indices; add={vertices} vertices/{indices} indices; limits={MAX_MESH_VERTICES} vertices/{MAX_MESH_INDICES} indices; exceeded={})",
                match (vertex_overflow, index_overflow) {
                    (true, true) => "vertices+indices",
                    (true, false) => "vertices",
                    (false, true) => "indices",
                    (false, false) => unreachable!(),
                }
            )));
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


#[derive(Debug)]
struct PatchGrid {
    width: usize,
    height: usize,
    vertices: Vec<Vertex>,
}

const OPENJK_MAX_GRID_SIZE: usize = 65;

/// Port of OpenJK `R_SubdividePatchToGrid`'s load-time tessellation.
///
/// OpenJK treats `r_subdivisions` as a maximum geometric deviation in map
/// units, inserts control columns/rows only when the quadratic curve exceeds
/// that error, then removes collinear controls and places the remaining points
/// on the curve. The optional final transpose in OpenJK only improves legacy
/// triangle-strip length, so it is intentionally omitted for our triangle-list
/// renderer.
fn subdivide_patch_to_grid(
    width: usize,
    height: usize,
    controls: &[Vertex],
    subdivision_error: f32,
) -> Result<PatchGrid> {
    if width < 3
        || height < 3
        || width > 31
        || height > 31
        || width % 2 == 0
        || height % 2 == 0
        || controls.len() != width * height
        || !subdivision_error.is_finite()
        || subdivision_error < 0.0
    {
        return Err(invalid("invalid quadratic patch control grid"));
    }

    let mut ctrl: Vec<Vec<Vertex>> = controls
        .chunks_exact(width)
        .map(|row| row.to_vec())
        .collect();
    let mut grid_width = width;
    let mut grid_height = height;
    let mut error_table = [
        vec![0.0_f32; OPENJK_MAX_GRID_SIZE],
        vec![0.0_f32; OPENJK_MAX_GRID_SIZE],
    ];

    for dir in 0..2 {
        error_table[dir].fill(0.0);
        let mut column = 0usize;
        while column + 2 < grid_width {
            let mut max_len_sq = 0.0_f32;
            for row in ctrl.iter().take(grid_height) {
                let p0 = row[column].position;
                let p1 = row[column + 1].position;
                let p2 = row[column + 2].position;
                let curve_mid: [f32; 3] = std::array::from_fn(|axis| {
                    (p0[axis] + 2.0 * p1[axis] + p2[axis]) * 0.25
                });
                let from_start: [f32; 3] =
                    std::array::from_fn(|axis| curve_mid[axis] - p0[axis]);
                let line = normalize3(std::array::from_fn(|axis| p2[axis] - p0[axis]));
                let projection = dot3(from_start, line);
                let perpendicular: [f32; 3] = std::array::from_fn(|axis| {
                    from_start[axis] - line[axis] * projection
                });
                max_len_sq = max_len_sq.max(dot3(perpendicular, perpendicular));
            }

            let max_len = max_len_sq.sqrt();
            if max_len < 0.1 {
                error_table[dir][column + 1] = 999.0;
                column += 2;
                continue;
            }
            if grid_width + 2 > OPENJK_MAX_GRID_SIZE {
                error_table[dir][column + 1] = 1.0 / max_len;
                column += 2;
                continue;
            }
            if max_len <= subdivision_error {
                error_table[dir][column + 1] = 1.0 / max_len;
                column += 2;
                continue;
            }

            error_table[dir][column + 2] = 1.0 / max_len;
            for row in ctrl.iter_mut().take(grid_height) {
                let previous = lerp_draw_vertex(row[column], row[column + 1]);
                let next = lerp_draw_vertex(row[column + 1], row[column + 2]);
                let mid = lerp_draw_vertex(previous, next);
                row.splice(column + 1..column + 2, [previous, mid, next]);
            }
            grid_width += 2;
            // OpenJK backs up and rechecks the same quadratic span after insertion.
        }

        ctrl = transpose_patch_grid(&ctrl, grid_width, grid_height);
        std::mem::swap(&mut grid_width, &mut grid_height);
    }

    put_patch_points_on_curve(&mut ctrl, grid_width, grid_height);

    // Cull control rows/columns OpenJK marked as completely collinear.
    let mut column = 1usize;
    while column + 1 < grid_width {
        if error_table[0][column] == 999.0 {
            for row in ctrl.iter_mut().take(grid_height) {
                row.remove(column);
            }
            for index in column + 1..grid_width {
                error_table[0][index - 1] = error_table[0][index];
            }
            grid_width -= 1;
        }
        column += 1;
    }
    let mut row = 1usize;
    while row + 1 < grid_height {
        if error_table[1][row] == 999.0 {
            ctrl.remove(row);
            for index in row + 1..grid_height {
                error_table[1][index - 1] = error_table[1][index];
            }
            grid_height -= 1;
        }
        row += 1;
    }

    make_patch_normals(&mut ctrl, grid_width, grid_height);

    let mut vertices = Vec::with_capacity(grid_width * grid_height);
    for row in ctrl.into_iter().take(grid_height) {
        vertices.extend(row.into_iter().take(grid_width));
    }
    Ok(PatchGrid {
        width: grid_width,
        height: grid_height,
        vertices,
    })
}

fn lerp_draw_vertex(a: Vertex, b: Vertex) -> Vertex {
    let mut out = a;
    out.position = std::array::from_fn(|axis| 0.5 * (a.position[axis] + b.position[axis]));
    out.texcoord = std::array::from_fn(|axis| 0.5 * (a.texcoord[axis] + b.texcoord[axis]));
    out.normal = std::array::from_fn(|axis| 0.5 * (a.normal[axis] + b.normal[axis]));
    for slot in 0..4 {
        out.lightmap_uv[slot] = std::array::from_fn(|axis| {
            0.5 * (a.lightmap_uv[slot][axis] + b.lightmap_uv[slot][axis])
        });
        out.color[slot] = std::array::from_fn(|channel| {
            ((u16::from(a.color[slot][channel]) + u16::from(b.color[slot][channel])) >> 1) as u8
        });
    }
    out
}

fn transpose_patch_grid(ctrl: &[Vec<Vertex>], width: usize, height: usize) -> Vec<Vec<Vertex>> {
    let mut transposed = Vec::with_capacity(width);
    for x in 0..width {
        let mut row = Vec::with_capacity(height);
        for source_row in ctrl.iter().take(height) {
            row.push(source_row[x]);
        }
        transposed.push(row);
    }
    transposed
}

fn put_patch_points_on_curve(ctrl: &mut [Vec<Vertex>], width: usize, height: usize) {
    for x in 0..width {
        for y in (1..height - 1).step_by(2) {
            let current = ctrl[y][x];
            let previous = lerp_draw_vertex(current, ctrl[y + 1][x]);
            let next = lerp_draw_vertex(current, ctrl[y - 1][x]);
            ctrl[y][x] = lerp_draw_vertex(previous, next);
        }
    }
    for y in 0..height {
        for x in (1..width - 1).step_by(2) {
            let current = ctrl[y][x];
            let previous = lerp_draw_vertex(current, ctrl[y][x + 1]);
            let next = lerp_draw_vertex(current, ctrl[y][x - 1]);
            ctrl[y][x] = lerp_draw_vertex(previous, next);
        }
    }
}

fn make_patch_normals(ctrl: &mut [Vec<Vertex>], width: usize, height: usize) {
    const NEIGHBORS: [[isize; 2]; 8] = [
        [0, 1],
        [1, 1],
        [1, 0],
        [1, -1],
        [0, -1],
        [-1, -1],
        [-1, 0],
        [-1, 1],
    ];

    let wrap_width = (0..height).all(|y| {
        let delta: [f32; 3] = std::array::from_fn(|axis| {
            ctrl[y][0].position[axis] - ctrl[y][width - 1].position[axis]
        });
        dot3(delta, delta) <= 1.0
    });
    let wrap_height = (0..width).all(|x| {
        let delta: [f32; 3] = std::array::from_fn(|axis| {
            ctrl[0][x].position[axis] - ctrl[height - 1][x].position[axis]
        });
        dot3(delta, delta) <= 1.0
    });

    let positions: Vec<Vec<[f32; 3]>> = ctrl
        .iter()
        .take(height)
        .map(|row| row.iter().take(width).map(|vertex| vertex.position).collect())
        .collect();

    for x in 0..width {
        for y in 0..height {
            let base = positions[y][x];
            let mut around = [[0.0_f32; 3]; 8];
            let mut good = [false; 8];

            for (neighbor_index, [dx, dy]) in NEIGHBORS.into_iter().enumerate() {
                for distance in 1..=3isize {
                    let mut nx = x as isize + dx * distance;
                    let mut ny = y as isize + dy * distance;
                    if wrap_width {
                        if nx < 0 {
                            nx = width as isize - 1 + nx;
                        } else if nx >= width as isize {
                            nx = 1 + nx - width as isize;
                        }
                    }
                    if wrap_height {
                        if ny < 0 {
                            ny = height as isize - 1 + ny;
                        } else if ny >= height as isize {
                            ny = 1 + ny - height as isize;
                        }
                    }
                    if nx < 0 || nx >= width as isize || ny < 0 || ny >= height as isize {
                        break;
                    }
                    let delta: [f32; 3] = std::array::from_fn(|axis| {
                        positions[ny as usize][nx as usize][axis] - base[axis]
                    });
                    let normalized = normalize3(delta);
                    if dot3(normalized, normalized) == 0.0 {
                        continue;
                    }
                    good[neighbor_index] = true;
                    around[neighbor_index] = normalized;
                    break;
                }
            }

            let mut sum = [0.0_f32; 3];
            for index in 0..8 {
                if !good[index] || !good[(index + 1) & 7] {
                    continue;
                }
                let normal = normalize3(cross3(around[(index + 1) & 7], around[index]));
                if dot3(normal, normal) == 0.0 {
                    continue;
                }
                for axis in 0..3 {
                    sum[axis] += normal[axis];
                }
            }
            ctrl[y][x].normal = normalize3(sum);
        }
    }
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
    fn debug_volumes_pick_clip_brushes_and_trigger_models() {
        // PLAYERCLIP cube: six quads, 12 triangles, 24 outline segments.
        let bsp = cube_bsp(0, 0x10);
        let volumes = bsp.debug_volumes().expect("clip cube");
        assert_eq!(volumes.clips.positions.len(), 24);
        assert_eq!(volumes.clips.triangle_indices.len(), 12 * 3);
        assert_eq!(volumes.clips.line_indices.len(), 24 * 2);
        assert!(volumes.triggers.is_empty());

        // Solid brushes without a clip flag are not clip volumes.
        assert!(cube_bsp(0, CONTENTS_SOLID).debug_volumes().unwrap().clips.is_empty());

        // A trigger_* entity pointing at *1 draws that model's brushes.
        let mut bsp = cube_bsp(0, 0);
        bsp.models.push(Model {
            mins: [-1.0; 3],
            maxs: [1.0; 3],
            surfaces: 0..0,
            brushes: 0..1,
        });
        bsp.entities.push(Entity {
            properties: vec![
                (b"classname".to_vec(), b"trigger_push".to_vec()),
                (b"model".to_vec(), b"*1".to_vec()),
            ],
        });
        let volumes = bsp.debug_volumes().expect("trigger cube");
        assert_eq!(volumes.triggers.triangle_indices.len(), 12 * 3);
        assert_eq!(volumes.triggers.colors[0], [64, 255, 96, 255]);
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
