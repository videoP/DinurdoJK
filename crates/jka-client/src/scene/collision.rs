//! Collision.
use crate::scene::{
    map_asset_name, AssetSearchPath, Bsp, HashMap, MapSource, Path, SurfaceKind, MAX_FILE_BYTES,
};

/// Build only the Rapier static-world mesh for an already-running BSP map.
///
/// Client physics is enabled after load far more often than it changes the
/// map's render data, and the mesh depends on nothing but the BSP surfaces, so
/// enabling it must not force a renderer restart or a full map re-prepare.
pub fn prepare_physics_collision(
    root: &Path,
    game: Option<&Path>,
    source: &MapSource,
    allow_asset_overrides: bool,
) -> Result<crate::cgame::ragdoll::PhysicsMapMesh, String> {
    let MapSource::Bsp(name) = source else {
        // Source-map preparation never produced a physics mesh either.
        return Ok(crate::cgame::ragdoll::PhysicsMapMesh::default());
    };
    let asset_name = map_asset_name(name, "bsp")?;
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    assets.set_allow_asset_overrides(allow_asset_overrides);
    let asset = assets
        .read(&asset_name, MAX_FILE_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Map {asset_name} not found on the game/base asset search path"))?;
    let bsp = Bsp::parse(&asset.bytes).map_err(|error| error.to_string())?;
    let mesh = bsp.world_mesh(4).map_err(|error| error.to_string())?;
    Ok(bsp_physics_collision_mesh(&bsp, &mesh))
}

pub(in crate::scene) fn bsp_physics_collision_mesh(
    bsp: &Bsp,
    mesh: &jka_assets::bsp::Mesh,
) -> crate::cgame::ragdoll::PhysicsMapMesh {
    const CONTENTS_SOLID: u32 = 0x0000_0001;
    const CONTENTS_TERRAIN: u32 = 0x0000_1000;

    // Rapier needs a triangle mesh, while authoritative player movement keeps
    // using OpenJK CM brushes/patch collision. Build and preprocess the visual-
    // physics shape here on the existing map worker, never during render/upload.
    let mut vertices = Vec::<[f32; 3]>::new();
    let mut triangles = Vec::<[u32; 3]>::new();
    let mut remap = HashMap::<u32, u32>::new();
    for batch in &mesh.batches {
        let Some(surface) = bsp.surfaces.get(batch.surface) else {
            continue;
        };
        if surface.kind == SurfaceKind::Flare {
            continue;
        }
        let Some(shader) = bsp.shaders.get(batch.shader) else {
            continue;
        };
        if shader.contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) == 0 {
            continue;
        }
        for triangle in mesh.indices[batch.indices.clone()].chunks_exact(3) {
            let mut compact = [0_u32; 3];
            let mut valid = true;
            for (slot, &source_index) in triangle.iter().enumerate() {
                if mesh.vertices.get(source_index as usize).is_none() {
                    valid = false;
                    break;
                }
                compact[slot] = *remap.entry(source_index).or_insert_with(|| {
                    let index = vertices.len() as u32;
                    vertices.push(mesh.vertices[source_index as usize].position);
                    index
                });
            }
            if valid
                && compact[0] != compact[1]
                && compact[1] != compact[2]
                && compact[2] != compact[0]
            {
                triangles.push(compact);
            }
        }
    }

    match crate::cgame::ragdoll::PhysicsMapMesh::from_jka_mesh(vertices, triangles) {
        Ok(mesh) => mesh,
        Err(error) => {
            eprintln!("RAPIER MAP PREP WARNING: {error}");
            crate::cgame::ragdoll::PhysicsMapMesh::default()
        }
    }
}
