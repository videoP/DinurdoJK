//! zulubo/SkinGore (MIT): CPU translation of DamageBaker, Dilate and AddBlit.
//!
//! This is deliberately a **data-stage** port, not a simulated decal.  The
//! output is a persistent UV-space RGBA8 damage map per GLM surface.  JKA has
//! only UV0, so the original Unity fallback (UV1 when UV2 is absent) applies;
//! overlapping UV islands retain upstream's documented bleeding limitation.
//!
//! Upstream: Assets/SkinGore/{SkinGoreRenderer.cs,Resources/*.shader}.
use std::{collections::HashMap, sync::Arc};

use super::player_gore::{skin_vertex, GpuSnap};

pub(crate) const RESOLUTION: usize = 64; // SkinGoreRenderer.goreMapResolution
const PIXELS: usize = RESOLUTION * RESOLUTION;

#[derive(Clone)]
pub(crate) struct DamageMap {
    /// RGBA8, matching Unity RenderTexture's default 8-bit format. The alpha
    /// channel participates in AddBlit's `Blend One One` equation.
    pub pixels: Arc<Vec<u8>>,
    pub revision: u64,
}
impl DamageMap {
    fn clear() -> Self { Self { pixels: Arc::new(vec![0; PIXELS * 4]), revision: 0 } }
}

/// Per-life and per-surface masks. Never create a new surface for every hit.
/// The renderer can key its 64x64 GPU texture by (entity, mesh key) and upload
/// only when the revision changes, avoiding an ever-growing texture cache.
#[derive(Default)]
pub(crate) struct SkinGoreMaps {
    masks: HashMap<(u16, Arc<str>), DamageMap>,
}
impl SkinGoreMaps {
    pub fn clear(&mut self) { self.masks.clear(); }
    pub fn clear_entity(&mut self, entity: u16) { self.masks.retain(|(id, _), _| *id != entity); }
    /// OpenJK CG_BodyQueueCopy duplicates the wounded Ghoul2 instance into
    /// the corpse slot. The source player starts a fresh life on respawn.
    pub fn body_queue_copy(&mut self, source: u16, corpse: u16) {
        let copied: Vec<_> = self.masks.iter()
            .filter(|((entity, _), _)| *entity == source)
            .map(|((_, surface), mask)| ((corpse, Arc::clone(surface)), mask.clone()))
            .collect();
        self.clear_entity(corpse);
        self.masks.extend(copied);
        self.clear_entity(source);
    }
    pub fn get(&self, entity: u16, mesh_key: &str) -> Option<&DamageMap> {
        self.masks.get(&(entity, Arc::from(mesh_key)))
    }
    pub fn retain_entity_surfaces(&mut self, entity: u16, snaps: &[GpuSnap]) {
        // Model/skin changes must not retain marks on unrelated surfaces.
        self.masks.retain(|(id, key), _| *id != entity || snaps.iter().any(|s| s.mesh_key == *key));
    }

    /// Translates SkinGoreRenderer.AddDamage: per-fragment UV-space distance
    /// from the current skinned model to the impact center, then Dilate and
    /// AddBlit. World-to-root is unnecessary here because the same rigid
    /// transform applies to both the transformed hit and the skinned points.
    pub fn add_damage(&mut self, entity: u16, snaps: &[GpuSnap], position: [f32; 3], radius: f32, amount: f32) {
        if !radius.is_finite() || radius <= 0.0 || !amount.is_finite() || amount <= 0.0 { return; }
        for snap in snaps {
            let baker = bake(snap, position, radius, amount);
            if !baker.iter().any(|p| p[0] != 0) { continue; }
            let dilated = dilate(&baker);
            // At most one mask per surface per entity. Dynamic joins/restarts
            // discard their maps in the presenter, not on an arbitrary timer.
            let map = self.masks.entry((entity, Arc::clone(&snap.mesh_key))).or_insert_with(DamageMap::clear);
            let pixels = Arc::make_mut(&mut map.pixels);
            for (old, add) in pixels.chunks_exact_mut(4).zip(dilated.iter()) {
                let a = f32::from(add[3]) / 255.0;
                // AddBlit: col = tex2D(_MainTex, uv) * _Color; return col * col.a;
                // Default _Color = (1,1,1,1), fixed4 saturates on RT write.
                for k in 0..4 {
                    let value = (f32::from(add[k]) * a).round() as u8;
                    old[k] = old[k].saturating_add(value);
                }
            }
            map.revision = map.revision.wrapping_add(1);
        }
    }
}

/// Rasterize bind-pose UV triangles at texel centers, interpolating the *current
/// posed positions* as Unity's DamageBaker does. Values use the original
/// `saturate(length(p-hit)/(radius*1.5))`, `(1-dist*dist)*amount` equations.
fn bake(snap: &GpuSnap, position: [f32;3], radius: f32, amount: f32) -> Vec<[u8;4]> {
    let points: Vec<[f32;3]> = snap.vertices.iter().map(|v| skin_vertex(v, snap).0).collect();
    let mut out = vec![[0u8;4]; PIXELS];
    for tri in snap.indices.chunks_exact(3) {
        let [a,b,c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (Some(va),Some(vb),Some(vc)) = (snap.vertices.get(a), snap.vertices.get(b), snap.vertices.get(c)) else { continue; };
        let (Some(pa),Some(pb),Some(pc)) = (points.get(a), points.get(b), points.get(c)) else { continue; };
        let (u0,u1,u2) = (va.uv,vb.uv,vc.uv);
        // The texture upload and the WGSL sampler both address row zero at
        // v=0. Bake into the same UV coordinates that the body draw samples.
        let xy = |uv: [f32;2]| [uv[0] * RESOLUTION as f32, uv[1] * RESOLUTION as f32];
        let (p0,p1,p2) = (xy(u0),xy(u1),xy(u2));
        let cross = |u: [f32;2],v: [f32;2]| u[0]*v[1]-u[1]*v[0];
        let sub = |u: [f32;2],v: [f32;2]| [u[0]-v[0],u[1]-v[1]];
        let den = cross(sub(p1,p0),sub(p2,p0));
        if den.abs() < 1.0e-7 { continue; }
        let x0 = p0[0].min(p1[0]).min(p2[0]).floor().max(0.0) as usize;
        let y0 = p0[1].min(p1[1]).min(p2[1]).floor().max(0.0) as usize;
        let x1 = (p0[0].max(p1[0]).max(p2[0]).ceil() as usize).min(RESOLUTION);
        let y1 = (p0[1].max(p1[1]).max(p2[1]).ceil() as usize).min(RESOLUTION);
        for y in y0..y1 { for x in x0..x1 {
            let p = [x as f32+0.5,y as f32+0.5];
            let b = cross(sub(p,p0),sub(p2,p0))/den;
            let c = cross(sub(p1,p0),sub(p,p0))/den;
            let a = 1.0-b-c;
            if a < -1.0e-6 || b < -1.0e-6 || c < -1.0e-6 { continue; }
            let world: [f32;3] = std::array::from_fn(|i| pa[i]*a+pb[i]*b+pc[i]*c);
            let dist = (world.iter().zip(position).map(|(v,h)|(v-h)*(v-h)).sum::<f32>().sqrt()/(radius*1.5)).clamp(0.0,1.0);
            let red = ((1.0-dist*dist)*amount).clamp(0.0,1.0);
            let value = (red*255.0).round() as u8;
            out[y*RESOLUTION+x] = [value,value,value,255];
        }}
    }
    out
}

/// Hidden/Dilate.shader: exact eight-neighbour *overwrite* order. This is not
/// a max-filter: the last valid neighbour wins, as in the upstream shader.
fn dilate(src: &[[u8;4]]) -> Vec<[u8;4]> {
    let mut out = src.to_vec();
    for y in 0..RESOLUTION { for x in 0..RESOLUTION {
        let idx = y*RESOLUTION+x;
        if src[idx][3] > 127 { continue; }
        for (dx,dy) in [(1,-1),(1,0),(1,1),(0,1),(-1,1),(-1,0),(-1,-1),(0,-1)] {
            let nx = (x as isize+dx).clamp(0,RESOLUTION as isize-1) as usize;
            let ny = (y as isize+dy).clamp(0,RESOLUTION as isize-1) as usize;
            let pixel = src[ny*RESOLUTION+nx];
            if pixel[3] > 127 { out[idx] = pixel; }
        }
    }}
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn dilate_respects_upstream_last_sample_order() {
        let mut input = vec![[0;4]; PIXELS];
        let at = |x: usize, y: usize| y*RESOLUTION+x;
        input[at(10,9)] = [80,80,80,255]; // (0,-1), last in order
        input[at(11,10)] = [220,220,220,255]; // (1,0), earlier
        assert_eq!(dilate(&input)[at(10,10)], [80,80,80,255]);
    }
    #[test] fn blank_map_and_entity_reset() {
        let mut maps = SkinGoreMaps::default();
        maps.masks.insert((3, Arc::from("x")), DamageMap::clear());
        maps.masks.insert((4, Arc::from("y")), DamageMap::clear());
        maps.clear_entity(3);
        assert!(maps.get(3,"x").is_none());
        assert!(maps.get(4,"y").is_some());
        assert_eq!(maps.get(4,"y").unwrap().pixels.len(), RESOLUTION*RESOLUTION*4);
    }
}

/// Original Unity SkinGoreBase material, loaded once from the JKA virtual
/// filesystem (including PK3 archives) when Enhanced is first used. The
/// three wound maps come from the SkinGore PK3. The original detail map is
/// bundled to work with older packages that shipped the alternate noise map.
#[derive(Clone)]
pub(crate) struct SkinGoreAssets {
    pub albedo: Arc<crate::materials::TextureData>,
    pub normal: Arc<crate::materials::TextureData>,
    pub metallic: Arc<crate::materials::TextureData>,
    pub detail: Arc<crate::materials::TextureData>,
}
const ASSET_ROOT: &str = "textures/japro";
static ASSETS: std::sync::OnceLock<Option<SkinGoreAssets>> = std::sync::OnceLock::new();

fn decode(assets: &mut jka_assets::pk3::AssetSearchPath, name: &str, srgb: bool)
    -> Option<Arc<crate::materials::TextureData>>
{
    let qpath = format!("{ASSET_ROOT}/{name}");
    let file = match assets.read(&qpath, 16 * 1024 * 1024) {
        Ok(Some(file)) => file,
        Ok(None) => {
            eprintln!("[SKINGORE] missing {qpath}; install the SkinGore PK3 in GameData/base");
            return None;
        }
        Err(error) => {
            eprintln!("[SKINGORE] could not read {qpath}: {error}");
            return None;
        }
    };
    decode_texture(qpath, &file.bytes, srgb)
}

/// Like `decode`, but a missing file is silent (caller has a fallback).
fn decode_quiet(assets: &mut jka_assets::pk3::AssetSearchPath, name: &str, srgb: bool)
    -> Option<Arc<crate::materials::TextureData>>
{
    let qpath = format!("{ASSET_ROOT}/{name}");
    let file = assets.read(&qpath, 16 * 1024 * 1024).ok()??;
    decode_texture(qpath, &file.bytes, srgb)
}

fn decode_texture(label: String, bytes: &[u8], srgb: bool)
    -> Option<Arc<crate::materials::TextureData>>
{
    let image = match image::load_from_memory(bytes) {
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            eprintln!("[SKINGORE] could not decode {label}: {error}");
            return None;
        }
    };
    let (width, height) = image.dimensions();
    let (rgba, mip_level_count) = crate::materials::build_mip_chain(width, height, image.into_raw());
    Some(Arc::new(crate::materials::TextureData {
        label,
        source: None, width, height, rgba: rgba.into(),
        rgba16f: None, mip_level_count, clamp: false, srgb,
    }))
}
/// Called on the cgame thread; PK3 lookup is lazy and cached. A failed lookup
/// is also cached for this process, so missing archives do not trigger VFS
/// reads on every rendered frame.
pub(crate) fn ensure_assets(search: &mut jka_assets::pk3::AssetSearchPath)
    -> Option<&'static SkinGoreAssets>
{
    ASSETS.get_or_init(|| Some(SkinGoreAssets {
        albedo: decode(search, "GoreTileAlbedo.png", true)?,
        normal: decode(search, "GoreTileNormal.png", false)?,
        // Match the Unity importer: the metallic red channel is sRGB, and
        // the smoothness alpha channel is read linearly from the same image.
        metallic: decode(search, "GoreTileSmoothness.png", true)?,
        // Prefer the PK3's detail map; fall back to the bundled original.
        detail: decode_quiet(search, "GoreDetailNoise.png", true)
            .or_else(|| decode_texture(
                format!("{ASSET_ROOT}/GoreDetailNoise.png"),
                include_bytes!("skin_gore/detailNoise.jpg"),
                true,
            ))?,
    })).as_ref()
}
/// Renderer thread only accesses images already loaded on the cgame thread.
pub(crate) fn assets() -> Option<&'static SkinGoreAssets> {
    ASSETS.get().and_then(Option::as_ref)
}
