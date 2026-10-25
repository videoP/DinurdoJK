//! Non-player CGame entity presentation.
//!
//! This is the Rust ownership boundary corresponding to OpenJK `CG_AddCEntity`.
//! Snapshot decoding and trajectory interpolation stay in `cgame.rs`; this module
//! dispatches already-presented entities into model/effect-specific render paths.
//! The first vertical slice intentionally renders ordinary MD3-backed
//! `ET_GENERAL` entities and non-inline MD3 `ET_MOVER` entities while keeping the
//! remaining entity kinds explicit instead of silently treating them as models.

mod turret;
use turret::TurretState;

use super::{
    md3_lod,
    item_presenter::{cg_item, item_cone_origin, ItemDraw, ItemInput, IT_WEAPON},
    weapon_fx::WeaponFx,
    player_presenter::{blend_for_alpha, openjk_vectoangles, Ghoul2PresentationView, PlayerPresenter},
    suppressed_during_intermission, ClientGameState, EntityPresentationKind, PresentedEntity,
};
use crate::{
    materials::{self, TextureData, Textures},
    renderer::{
        DynamicModelAlphaMode, DynamicModelSurface, DynamicWireframeClass, DynamicModelVertex,
        Ghoul2GpuBone, Ghoul2GpuSkinning, Ghoul2GpuVertex,
        InlineModelInstance, RayTracedRigidInstance,
    },
    scene,
};
use jka_assets::{
    bsp::StaticModels,
    md3::{self, Model as Md3Model},
    pk3::AssetSearchPath,
    shader::{AlphaGen, RgbGen, Shader, Stage, TcGen, TcMod},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

const MAX_MD3_BYTES: usize = 64 * 1024 * 1024;
const EF_NODRAW: i32 = 1 << 8;
const EF_DEAD: i32 = 1 << 1;
/// bg_public.h: authoritative spawn protection flag consumed by CG_Player.
const EF_INVULNERABLE: i32 = 1 << 27;
const EF2_HYPERSPACE: i32 = 1 << 5;
const SOLID_BMODEL: i32 = 0x00ff_ffff;
const WP_SABER: i32 = 3;
const G2_MODEL_PART: i32 = 50;
const INVULNERABILITY_MODEL: &str = "models/weaphits/testboom.md3";
const INVULNERABILITY_SHADER: &str = "powerups/invulnerabilityshell";
/// `cgs.media.halfShieldShader` (same `testboom.md3` model as the sphere above).
const SHIELD_HIT_SHADER: &str = "halfShieldShell";
/// cg_players.c MAX_SHIELD_TIME / MIN_SHIELD_TIME.
const SHIELD_MAX_TIME_MS: i32 = 2000;

#[derive(Clone)]
struct Md3SurfaceAsset {
    surface_index: usize,
    cached_indices: Arc<Vec<u32>>,
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
    authored_glow: bool,
    source_material: Option<crate::renderer::SourceMaterialParams>,
    gpu_vertices: Option<Arc<Vec<crate::renderer::Ghoul2GpuVertex>>>,
    material_trace: Arc<str>,
    /// Current effective shader has no color/depth effect. This only gates
    /// submission; the model and vertices remain cached for later remaps.
    invisible_without_override: bool,
    /// Extra authored passes, kept with the cached MD3 so remaps can replace
    /// materials without ever discarding the source geometry.
    extra_stages: Vec<Md3ExtraStage>,
    two_sided_indices: Option<Arc<Vec<u32>>>,
    authored_lighting: bool,
}

#[derive(Clone)]
struct Md3ExtraStage {
    index: usize,
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
    authored_glow: bool,
    rgb_gen: RgbGen,
    alpha_gen: AlphaGen,
    rgb_const: [f32; 3],
    alpha_const: f32,
    tc_gen: TcGen,
    tc_mods: Vec<TcMod>,
}

struct Md3Asset {
    key: Arc<str>,
    content_hash: u64,
    model: Arc<Md3Model>,
    surfaces: Vec<Md3SurfaceAsset>,
}

struct Md3Chain {
    lods: [Option<Arc<Md3Asset>>; 3],
    generated: [bool; 3],
    queued: bool,
    processed: bool,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct Md3GeometrySlot {
    model: usize,
    surface: usize,
    entity: u16,
    depth_hack: u32,
    stage: usize,
}

struct CachedMd3Geometry {
    // Prevent model-pointer recycling after a registration is replaced.
    _model: Arc<Md3Model>,
    frame: usize,
    input: [u32; 18],
    vertices: Arc<Vec<DynamicModelVertex>>,
    indices: Arc<Vec<u32>>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EntityDispatchSummary {
    pub general: usize,
    pub movers: usize,
    pub items: usize,
    pub missiles: usize,
    pub npcs: usize,
    pub fx: usize,
    pub other: usize,
    pub rendered_surfaces: usize,
}

pub struct EntityPresenter {
    assets: AssetSearchPath,
    pbr: bool,
    shaders: BTreeMap<String, Shader>,
    shader_remaps: scene::ShaderRemapState,
    textures: Textures,
    texture_arcs: HashMap<usize, Arc<TextureData>>,
    missing_texture: Arc<TextureData>,
    md3_models: HashMap<String, Arc<Md3Asset>>,
    md3_chains: HashMap<String, Md3Chain>,
    md3_worker: md3_lod::Worker,
    md3_lod_view: Option<Ghoul2PresentationView>,
    entity_pvs_enabled: bool,
    entity_area_mask: Option<[u8; 32]>,
    md3_lod_scale: f32,
    md3_lod_bias: i32,
    md3_auto_lod: bool,
    md3_lod_debug: bool,
    md3_lod_reported: HashSet<(String, usize)>,
    md3_static_report_at: Option<Instant>,
    /// Developer A/B: reuse the existing resident Ghoul2 mesh/instance renderer
    /// for repeated misc_model_static MD3s. Off is the original CPU path.
    static_md3_instances: bool,
    static_md3_meshes: HashMap<String, (Arc<Vec<Ghoul2GpuVertex>>, Arc<Vec<u32>>)>,
    static_md3_identity_bone: Arc<Vec<Ghoul2GpuBone>>,
    md3_geometry_cache_enabled: bool,
    md3_geometry: HashMap<Md3GeometrySlot, CachedMd3Geometry>,
    md3_geometry_bytes: usize,
    /// Cosmetic models that failed to load (logged once each).
    missing_cosmetics: std::collections::HashSet<String>,
    failed_models: HashSet<String>,
    logged_unsupported: HashSet<String>,
    fx_materials: HashMap<String, Vec<crate::fx::draw::FxMaterial>>,
    /// True only while the renderer is actually using hardware RT shadows.
    /// Keep RT source metadata completely off the ordinary raster presentation path.
    rt_rigid_casters_enabled: bool,
    /// r_drawMapModels: server-placed MD3 props (ET_GENERAL / non-inline
    /// ET_MOVER). Inline BSP brush movers are never affected.
    draw_map_models: bool,
    /// jaPRO `cgs.miscStaticModels` source: the map's `misc_model_static`
    /// entities. They are client-only (the game module frees them), so they
    /// come from the BSP entity string rather than from snapshots.
    static_model_source: Arc<StaticModels>,
    /// Surfaces for `static_model_source`, built on first use and rebuilt when
    /// the source changes or the entity assets are refreshed.
    static_props: Option<Vec<StaticProp>>,
    /// Every (prop, LOD, surface) of `static_props` in one draw order fixed at
    /// build time: opaque then alpha-tested, by stage layer, then texture.
    /// Emitting visible props in this order makes same-texture surfaces of
    /// different props adjacent, so the renderer merges them into one draw.
    static_draw_order: Vec<StaticDrawRef>,
    /// Per-frame LOD chosen for each static prop (`u8::MAX` = culled).
    static_lod_scratch: Vec<u8>,
    static_candidate_scratch: Vec<usize>,
    /// Immutable, map-local spatial hierarchy of misc_model_static bounds.
    /// The tree prunes whole frustum/distance/PVS groups before LOD selection;
    /// every surviving prop still runs the exact original per-prop tests.
    static_visibility_tree: StaticVisibilityTree,
    static_md3_bvh: bool,
    occlusion_stats: bool,
    /// Whether `static_props` carry RT caster sources (they must be rebuilt when
    /// hardware RT shadows are toggled).
    static_props_rt: bool,
    /// `cent->damageTime` / `damageAngles` set by `CG_PlayerShieldHit`
    /// (EV_SHIELD_HIT: armor absorbed part of a hit), keyed by client number.
    shield_hits: HashMap<u16, ShieldHit>,
    material_clock: std::time::Instant,
    imported_props: HashMap<String,Arc<jka_emotes::prop_mesh::Mesh>>,
    prop_gpu_vertices:HashMap<String,Arc<Vec<crate::renderer::Ghoul2GpuVertex>>>,
    prop_normals:HashMap<String,Option<Arc<TextureData>>>,
    ue_materials:HashMap<String,Option<Arc<jka_ue_assets::material::Program>>>,
    material_eye:[f32;3],
    turret_states: HashMap<u16, TurretState>,
}

/// `centity_t::damageTime` and `damageAngles` for the armor hit shell.
#[derive(Clone, Copy)]
struct ShieldHit {
    until: i32,
    angles: [f32; 3],
}

#[derive(Clone, Copy)]
struct StaticDrawRef {
    prop: u32,
    lod: u8,
    surface: u32,
}

/// One placed `misc_model_static` with its world-space surfaces already built.
struct StaticProp {
    surfaces: Vec<DynamicModelSurface>,
    /// Optional resident-GPU equivalents. None keeps the exact CPU material path.
    gpu_surfaces: Vec<Option<DynamicModelSurface>>,
    /// Pretransformed on model registration, switched without re-transforming
    /// thousands of misc_model_static surfaces on every camera move.
    lods: [Option<Vec<DynamicModelSurface>>; 2],
    gpu_lods: [Option<Vec<Option<DynamicModelSurface>>>; 2],
    /// Shader-stage index of each surface, per LOD (0 = `surfaces`).
    layers: [Vec<u8>; 3],
    lod_count: usize,
    lod_radius: f32,
    origin: [f32; 3],
    /// Origin the cull tests use (jaPRO adds 1 + zoffset to z).
    cull_origin: [f32; 3],
    /// PVS cluster of `cull_origin`; `None` skips the PVS test.
    cull_cluster: Option<usize>,
    /// `RadiusFromBounds` of the frame-0 bounds scaled by the model scale; 0
    /// when the model has no bounds, which disables the frustum test.
    radius: f32,
}


/// A CPU broad phase. Bounds enclose both the authored origin and the
/// zoffset-adjusted cull origin, plus the full MD3 radius. An invalid radius
/// disables group rejection, rather than allowing an invisible prop.
#[derive(Default)]
struct StaticVisibilityTree {
    nodes: Vec<StaticVisibilityNode>,
    prop_indices: Vec<usize>,
}

#[derive(Clone, Copy)]
struct StaticVisibilityNode {
    center: [f32; 3],
    radius: f32,
    /// If all children share one valid cluster, PVS can reject the group.
    shared_cluster: Option<usize>,
    start: usize,
    end: usize,
    children: Option<[usize; 2]>,
    /// Cannot reject any subtree containing a prop with unknown bounds.
    bounds_valid: bool,
}

impl StaticVisibilityTree {
    const LEAF_SIZE: usize = 12;

    fn build(props: &[StaticProp]) -> Self {
        if props.is_empty() { return Self::default(); }
        let mut result = Self {
            nodes: Vec::new(),
            prop_indices: (0..props.len()).collect(),
        };
        result.build_node(props, 0, props.len());
        result
    }

    fn build_node(&mut self, props: &[StaticProp], start: usize, end: usize) -> usize {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        let mut bounds_valid = true;
        let mut shared_cluster = props[self.prop_indices[start]].cull_cluster;
        for &prop_index in &self.prop_indices[start..end] {
            let prop = &props[prop_index];
            if !prop.radius.is_finite() || prop.radius <= 0.0
                || !prop.origin.iter().chain(prop.cull_origin.iter()).all(|v| v.is_finite()) {
                bounds_valid = false;
            }
            if shared_cluster != prop.cull_cluster { shared_cluster = None; }
            for axis in 0..3 {
                let lower = prop.origin[axis].min(prop.cull_origin[axis]) - prop.radius;
                let upper = prop.origin[axis].max(prop.cull_origin[axis]) + prop.radius;
                lo[axis] = lo[axis].min(lower);
                hi[axis] = hi[axis].max(upper);
            }
        }
        if lo.iter().chain(hi.iter()).any(|x| !x.is_finite()) { bounds_valid = false; }
        let center = std::array::from_fn(|axis| (lo[axis] * 0.5) + (hi[axis] * 0.5));
        let radius = ((0..3).map(|axis| (hi[axis] - center[axis]).powi(2)).sum::<f32>()).sqrt()
            + 1.0; // absorb rounding on the broad-phase enclosure
        if !radius.is_finite() { bounds_valid = false; }
        let node_index = self.nodes.len();
        self.nodes.push(StaticVisibilityNode {
            center, radius, shared_cluster, start, end, children: None, bounds_valid,
        });
        if end - start > Self::LEAF_SIZE {
            let axis = (0..3).max_by(|&a, &b| {
                (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b]))
            }).unwrap_or(0);
            self.prop_indices[start..end].sort_unstable_by(|&a, &b| {
                props[a].cull_origin[axis].total_cmp(&props[b].cull_origin[axis])
            });
            let mid = start + (end - start) / 2;
            let left = self.build_node(props, start, mid);
            let right = self.build_node(props, mid, end);
            self.nodes[node_index].children = Some([left, right]);
        }
        node_index
    }

    fn candidates(
        &self,
        view: Ghoul2PresentationView,
        visibility: Option<&jka_assets::bsp::Visibility>,
        view_cluster: Option<usize>,
        output: &mut Vec<usize>,
    ) -> (usize, usize) {
        // Not a visibility *decision*: returns conservative candidates for
        // the original per-prop culling. The array is never used as draw order.
        let mut checked = 0;
        let mut rejected = 0;
        if !self.nodes.is_empty() {
            self.gather(0, view, visibility, view_cluster, output, &mut checked, &mut rejected);
        }
        (checked, rejected)
    }

    #[allow(clippy::too_many_arguments)]
    fn gather(
        &self, index: usize, view: Ghoul2PresentationView,
        visibility: Option<&jka_assets::bsp::Visibility>, view_cluster: Option<usize>,
        output: &mut Vec<usize>, checked: &mut usize, rejected: &mut usize,
    ) {
        *checked += 1;
        let node = self.nodes[index];
        let outside = node.bounds_valid && (
            view.sphere_outside(node.center, node.radius)
            || (view.distance_cull() > 0.0
                && view.distance_to(node.center) - node.radius > view.distance_cull())
        );
        let pvs_hidden = node.shared_cluster.is_some_and(|cluster| {
            visibility.is_some_and(|vis| !vis.r_in_pvs(view_cluster, Some(cluster)))
        });
        if outside || pvs_hidden { *rejected += 1; return; }
        if let Some([left, right]) = node.children {
            self.gather(left, view, visibility, view_cluster, output, checked, rejected);
            self.gather(right, view, visibility, view_cluster, output, checked, rejected);
        } else {
            output.extend_from_slice(&self.prop_indices[node.start..node.end]);
        }
    }
}

/// Transform all eight corners, including rotated and negative-scale models.
fn md3_world_bounds(mins: [f32; 3], maxs: [f32; 3], origin: [f32; 3],
                    axis: [[f32; 3]; 3], scale: f32) -> ([f32; 3], [f32; 3]) {
    let mut low = [f32::INFINITY; 3];
    let mut high = [f32::NEG_INFINITY; 3];
    for corner in 0..8 {
        let local: [f32; 3] = std::array::from_fn(|i|
            if corner & (1 << i) == 0 { mins[i] * scale } else { maxs[i] * scale });
        for i in 0..3 {
            let value = origin[i] + (0..3).map(|j| axis[j][i] * local[j]).sum::<f32>();
            low[i] = low[i].min(value);
            high[i] = high[i].max(value);
        }
    }
    (low, high)
}

impl EntityPresenter {
    pub fn new(mut assets: AssetSearchPath, pbr: bool) -> Result<Self, String> {
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut assets, &mut shader_warnings, pbr)?;
        devprintln!(
            1,
            "ENTITY ASSETS: shaderDefs={} mtrDefs={}",
            diagnostics.shader_definitions, diagnostics.mtr_definitions,
        );
        for warning in shader_warnings {
            rverbose!(1, "ENTITY MATERIAL WARNING: {warning}");
        }
        Ok(Self {
            assets,
            pbr,
            shaders,
            shader_remaps: scene::ShaderRemapState::default(),
            textures: Textures::new(),
            texture_arcs: HashMap::new(),
            missing_texture: Arc::new(materials::missing_texture_data()),
            md3_models: HashMap::new(),
            md3_chains: HashMap::new(),
            md3_worker: md3_lod::Worker::new(),
            md3_lod_view: None,
            entity_pvs_enabled: true,
            entity_area_mask: None,
            md3_lod_scale: 5.0,
            md3_lod_bias: 0,
            md3_auto_lod: false,
            md3_lod_debug: false,
            md3_lod_reported: HashSet::new(),
            md3_static_report_at: None,
            static_md3_instances: false,
            static_md3_meshes: HashMap::new(),
            static_md3_identity_bone: Arc::new(vec![Ghoul2GpuBone {
                row0: [1.0, 0.0, 0.0, 0.0],
                row1: [0.0, 1.0, 0.0, 0.0],
                row2: [0.0, 0.0, 1.0, 0.0],
            }]),
            md3_geometry_cache_enabled: true,
            md3_geometry: HashMap::new(),
            md3_geometry_bytes: 0,
            missing_cosmetics: std::collections::HashSet::new(),
            failed_models: HashSet::new(),
            logged_unsupported: HashSet::new(),
            fx_materials: HashMap::new(),
            rt_rigid_casters_enabled: false,
            draw_map_models: true,
            static_model_source: Arc::default(),
            static_props: None,
            static_draw_order: Vec::new(),
            static_lod_scratch: Vec::new(),
            static_candidate_scratch: Vec::new(),
            static_visibility_tree: StaticVisibilityTree::default(),
            static_md3_bvh: false,
            occlusion_stats: false,
            static_props_rt: false,
            shield_hits: HashMap::new(),
            material_clock: std::time::Instant::now(),
            imported_props:HashMap::new(),
            prop_gpu_vertices:HashMap::new(),
            prop_normals:HashMap::new(),
            ue_materials:HashMap::new(),
            material_eye:[0.;3],
            turret_states: HashMap::new(),
        })
    }

    /// `CG_PlayerShieldHit`: EV_SHIELD_HIT carries the hit client in
    /// `otherEntityNum`, the damage direction in `eventParm` (DirToByte) and the
    /// absorbed amount in `time2`. The shell only extends an active flash.
    pub fn note_shield_hit(&mut self, event: &super::PresentationEvent) {
        if event.event != jka_protocol::entity_event::EntityEvent::EV_SHIELD_HIT {
            return;
        }
        let Ok(client) = u16::try_from(event.state.field_i32("otherEntityNum").unwrap_or(-1))
        else {
            return;
        };
        let amount = event.state.field_i32("time2").unwrap_or(0);
        let duration = if amount > SHIELD_MAX_TIME_MS {
            SHIELD_MAX_TIME_MS
        } else {
            500 + amount.saturating_mul(15)
        };
        let until = event.server_time.saturating_add(duration);
        if self.shield_hits.get(&client).is_some_and(|hit| hit.until >= until) {
            return;
        }
        let dir = jka_movement::byte_to_dir(event.parm);
        self.shield_hits.insert(
            client,
            ShieldHit {
                until,
                angles: openjk_vectoangles([-dir[0], -dir[1], -dir[2]]),
            },
        );
    }

    /// `CG_DrawPlayerShield` via `CG_PlayerHitFX`: a fading half-shield facing
    /// the hit direction. Stock skips the local player in first person (and the
    /// dead), so `hide_entity` is the viewer's own client when first person.
    pub fn present_shield_hits(
        &mut self,
        entities: &[PresentedEntity],
        followed: Option<&PresentedEntity>,
        time: i32,
        hide_entity: Option<u16>,
    ) -> Vec<DynamicModelSurface> {
        self.shield_hits
            .retain(|_, hit| hit.until > time && hit.until - time <= SHIELD_MAX_TIME_MS + 500);
        if self.shield_hits.is_empty() {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut draws = Vec::new();
        for entity in followed.into_iter().chain(entities.iter()) {
            if entity.entity_type != super::ET_PLAYER
                || Some(entity.number) == hide_entity
                || !seen.insert(entity.number)
            {
                continue;
            }
            let Some(hit) = self.shield_hits.get(&entity.number).copied() else {
                continue;
            };
            if entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0 {
                continue;
            }
            // Q_flrand(0, 1) * 16 flicker on top of the fade.
            let flicker = (time as u32)
                .wrapping_mul(0x9E37_79B1)
                .wrapping_add(u32::from(entity.number).wrapping_mul(0x85EB_CA6B))
                >> 28;
            let alpha = (255.0 * ((hit.until - time) as f32 / SHIELD_MAX_TIME_MS as f32)
                + flicker as f32)
                .min(255.0);
            // Bigger but tighter when more solid: 1.4 down to 1.0.
            let scale = 1.4 - alpha * (0.4 / 255.0);
            let mut axis = angles_to_axis(hit.angles);
            for row in &mut axis {
                for value in row.iter_mut() {
                    *value *= scale;
                }
            }
            let origin = [entity.origin[0], entity.origin[1], entity.origin[2] + 10.0];
            let level = alpha / 255.0;
            match self.present_custom_md3_stages(
                entity.number,
                INVULNERABILITY_MODEL,
                origin,
                axis,
                [level, level, level, 1.0],
                SHIELD_HIT_SHADER,
            ) {
                Ok(mut shell) => draws.append(&mut shell),
                Err(error) => {
                    if self.logged_unsupported.insert("shield-hit".to_string()) {
                        rverbose!(1, "SHIELD HIT SHELL: {error}");
                    }
                }
            }
        }
        draws
    }

    pub fn set_draw_map_models(&mut self, enabled: bool) {
        self.draw_map_models = enabled;
    }

    /// Changes only the client-side misc_model_static rendering path. Does not
    /// alter world/BSP surfaces, runtime entities or GPU Ghoul2 player settings.
    pub fn set_static_md3_instances(&mut self, enabled: bool) {
        if self.static_md3_instances != enabled {
            self.static_md3_instances = enabled;
            self.static_props = None;
        }
    }

    /// Runtime switch; the original per-prop scan is restored immediately at 0.
    /// The tree can stay cached for the next A/B switch without affecting draws.
    pub fn set_static_md3_bvh(&mut self, enabled: bool) {
        if enabled && !self.static_md3_bvh && self.static_visibility_tree.nodes.is_empty() {
            if let Some(props) = &self.static_props {
                self.static_visibility_tree = StaticVisibilityTree::build(props);
            }
        }
        self.static_md3_bvh = enabled;
    }

    pub fn set_occlusion_stats(&mut self, enabled: bool) {
        self.occlusion_stats = enabled;
        if enabled { self.md3_static_report_at = None; }
    }

    pub fn set_md3_geometry_cache(&mut self, enabled: bool) {
        if self.md3_geometry_cache_enabled != enabled {
            self.md3_geometry.clear();
            self.md3_geometry_bytes = 0;
            self.md3_geometry_cache_enabled = enabled;
        }
    }

    pub(crate) fn set_shader_remaps(&mut self, remaps: scene::ShaderRemapState) {
        if self.shader_remaps == remaps {
            return;
        }
        self.shader_remaps = remaps;
        // A server remap is reversible: keep the original MD3 models, vertex
        // buffers and cached transforms. Only the material decisions change.
        // The old implementation discarded all models here and reread them
        // from the VFS, making a clear -> visible transition unnecessarily
        // expensive (and losing any generated MD3 LODs).
        let mut replaced = HashMap::new();
        let old_models = std::mem::take(&mut self.md3_models);
        for (key, old) in old_models {
            let refreshed = Arc::new(self.refresh_md3_materials(&old));
            replaced.insert(Arc::as_ptr(&old) as usize, Arc::clone(&refreshed));
            self.md3_models.insert(key, refreshed);
        }
        let mut chains = std::mem::take(&mut self.md3_chains);
        for chain in chains.values_mut() {
            for slot in &mut chain.lods {
                let Some(old) = slot.take() else { continue };
                let id = Arc::as_ptr(&old) as usize;
                let refreshed = if let Some(existing) = replaced.get(&id) {
                    Arc::clone(existing)
                } else {
                    // Authored/generated LODs may exist only in the chain.
                    let refreshed = Arc::new(self.refresh_md3_materials(&old));
                    replaced.insert(id, Arc::clone(&refreshed));
                    refreshed
                };
                *slot = Some(refreshed);
            }
        }
        self.md3_chains = chains;
        self.fx_materials.clear();
        // Pretransformed static props contain old material/visibility decisions.
        // Rebuild once using the retained model and geometry caches.
        self.static_props = None;
    }

    /// Installs the current map's `misc_model_static` placements.
    pub fn set_static_models(&mut self, models: Arc<StaticModels>) {
        if Arc::ptr_eq(&self.static_model_source, &models) {
            return;
        }
        self.static_model_source = models;
        self.static_props = None;
        // Static GPU source data belongs to this BSP's MD3 registration set.
        // Do not retain it forever while changing maps/mods.
        self.static_md3_meshes.clear();
    }

    /// jaPRO `CG_DrawMiscStaticModels`: every static prop that survives the
    /// frustum, PVS and distance cull, as already-transformed MD3 surfaces. Without a
    /// view (first frame, teleport) nothing is culled. With hardware RT shadows
    /// the props are architecture that must keep shadowing, so culled ones stay
    /// in the list as shadow-only (`raster_visible = false`); that also keeps the
    /// RT scene signature stable while the camera turns.
    pub fn present_static_models(
        &mut self,
        view: Option<Ghoul2PresentationView>,
    ) -> Vec<DynamicModelSurface> {
        if !self.draw_map_models || self.static_model_source.placements.is_empty() {
            return Vec::new();
        }
        let present_started = self.occlusion_stats.then(Instant::now);
        if self.static_props.is_none() || self.static_props_rt != self.rt_rigid_casters_enabled {
            self.static_props_rt = self.rt_rigid_casters_enabled;
            let props = self.build_static_props();
            self.static_props = Some(props);
        }
        // R_inPVS(vieworg, cullorg): the view cluster's PVS row, areamask
        // ignored as in jaPRO. A view in solid (no cluster) keeps every prop,
        // like the world path does.
        let pvs = self.static_model_source.visibility.as_ref().filter(|_| self.entity_pvs_enabled);
        let view_cluster =
            view.and_then(|view| pvs.and_then(|vis| vis.cluster_at(view.jka_position())));
        let Some(props) = self.static_props.as_ref() else { return Vec::new() };
        let mut chosen = std::mem::take(&mut self.static_lod_scratch);
        chosen.clear();
        let mut lod_counts = [0usize; 3];
        let mut candidates = std::mem::take(&mut self.static_candidate_scratch);
        let tree_started = self.occlusion_stats.then(Instant::now);
        let (tree_tested, tree_pruned) = if self.static_md3_bvh {
            candidates.clear();
            if let Some(view) = view {
                self.static_visibility_tree.candidates(view, pvs, view_cluster, &mut candidates)
            } else {
                candidates.extend(0..props.len());
                (0, 0)
            }
        } else { (0, 0) };
        let tree_us = tree_started.map(|now| now.elapsed().as_micros()).unwrap_or(0);
        let candidate_count = if self.static_md3_bvh { candidates.len() } else { props.len() };
        let hierarchy_skipped_props = props.len().saturating_sub(candidate_count);
        let mut select_lod = |prop: &StaticProp| {
            let culled = view.is_some_and(|view| {
                let distance_cull = view.distance_cull();
                (prop.radius > 0.0 && view.sphere_outside(prop.cull_origin, prop.radius))
                    || pvs.is_some_and(|vis| !vis.r_in_pvs(view_cluster, prop.cull_cluster))
                    || (distance_cull > 0.0
                        && view.distance_to(prop.origin) - prop.radius > distance_cull)
            });
            if culled { return u8::MAX; }
            let lod = md3_lod_for_view(
                view, prop.origin, prop.lod_radius, prop.lod_count,
                self.md3_lod_scale, self.md3_lod_bias, false,
            );
            lod_counts[lod] += 1;
            if lod != 0 && prop.lods[lod - 1].is_none() { 0 } else { lod as u8 }
        };
        if self.static_md3_bvh {
            chosen.resize(props.len(), u8::MAX);
            for &prop_index in &candidates { chosen[prop_index] = select_lod(&props[prop_index]); }
        } else {
            // Original direct scan: no candidate fill or extra full-array initialization.
            for prop in props { chosen.push(select_lod(prop)); }
        }
        let mut draws = Vec::new();
        for entry in &self.static_draw_order {
            if chosen[entry.prop as usize] != entry.lod { continue; }
            let prop = &props[entry.prop as usize];
            let surfaces = match entry.lod {
                0 => &prop.surfaces,
                lod => prop.lods[usize::from(lod) - 1].as_ref().unwrap_or(&prop.surfaces),
            };
            // The resident path uses the *same* PVS, distance, frustum and LOD
            // decisions as A. It swaps only eligible source geometry for an
            // instanced transform, leaving all unusual material stages intact.
            // RT falls back to A: rigid BLAS ownership must remain unchanged.
            let gpu = if self.static_md3_instances && !self.rt_rigid_casters_enabled {
                match entry.lod {
                    0 => Some(&prop.gpu_surfaces),
                    lod => prop.gpu_lods[usize::from(lod) - 1].as_ref(),
                }.and_then(|level| level.get(entry.surface as usize))
                    .and_then(Option::as_ref)
            } else { None };
            let mut surface = gpu.unwrap_or(&surfaces[entry.surface as usize]).clone();
            if self.rt_rigid_casters_enabled && entry.lod != 0 {
                // Keep RT caster geometry at LOD0 so changing raster LOD
                // does not rebuild raytracing acceleration structures.
                surface.rt_rigid = None;
            }
            draws.push(surface);
        }
        if self.rt_rigid_casters_enabled {
            // Offscreen and lower-LOD props keep their stable authored LOD0
            // caster as shadow-only geometry.
            for (prop, &lod) in props.iter().zip(&chosen) {
                if lod == 0 { continue; }
                draws.extend(prop.surfaces.iter().cloned().map(|mut surface| {
                    surface.raster_visible = false;
                    surface
                }));
            }
        }
        self.static_lod_scratch = chosen;
        self.static_candidate_scratch = candidates;
        if (self.md3_lod_debug || self.occlusion_stats) && self.md3_static_report_at
            .is_none_or(|instant| instant.elapsed().as_secs_f32() >= 1.0) {
            println!("MD3 STATIC A/B: BVH={} total_props={} candidates={} nodes_tested={} nodes_pruned={} props_pruned={} broadphase_us={} visible_LODs=[{},{},{}] submitted_surfaces={} total_present_us={} (MD3 Hi-Z NOT wired)",
                u8::from(self.static_md3_bvh), props.len(), candidate_count,
                tree_tested, tree_pruned, hierarchy_skipped_props, tree_us,
                lod_counts[0], lod_counts[1], lod_counts[2], draws.len(),
                present_started.map(|t| t.elapsed().as_micros()).unwrap_or(0));
            self.md3_static_report_at = Some(Instant::now());
        }
        draws
    }

    fn build_static_props(&mut self) -> Vec<StaticProp> {
        let source = Arc::clone(&self.static_model_source);
        let mut props = Vec::with_capacity(source.placements.len());
        let mut surfaces_total = 0usize;
        let mut gpu_surfaces_total = 0usize;
        // Only promote repeated *model paths*; a singleton already uses the
        // original efficient indexed path and gains no instance batching.
        let mut frequencies: HashMap<String, usize> = HashMap::new();
        if self.static_md3_instances {
            for p in &source.placements {
                *frequencies.entry(p.model.to_ascii_lowercase()).or_default() += 1;
            }
        }
        for placement in source.placements.iter() {
            let mut axis = angles_to_axis(placement.angles);
            for (row, scale) in axis.iter_mut().zip(placement.scale) {
                for component in row.iter_mut() {
                    *component *= scale;
                }
            }
            let submission = ItemDraw {
                model: placement.model.clone(),
                origin: placement.origin,
                axis,
                rgba: [1.0; 4],
                custom_shader: None,
            };
            // rt_rigid is filled by present_md3_ref_entity while RT casters are on.
            let mut layers: [Vec<u8>; 3] = Default::default();
            let surfaces = match self.present_md3_ref_entity_layers(1023, &submission, None, Some(&mut layers[0])) {
                Ok(surfaces) => surfaces,
                Err(error) => {
                    if self.logged_unsupported.insert(format!("static:{}", placement.model)) {
                        println!("STATIC MODEL {}: {error}", placement.model);
                    }
                    continue;
                }
            };
            surfaces_total += surfaces.len();
            let repeated = frequencies.get(&placement.model.to_ascii_lowercase())
                .is_some_and(|count| *count >= 2);
            let gpu_surfaces = if repeated {
                self.static_md3_gpu_surfaces(&submission, None, &surfaces)
            } else { vec![None; surfaces.len()] };
            gpu_surfaces_total += gpu_surfaces.iter().filter(|s| s.is_some()).count();
            let mut gpu_lods: [Option<Vec<Option<DynamicModelSurface>>>; 2] = [None, None];
            let mut lods: [Option<Vec<DynamicModelSurface>>; 2] = [None, None];
            let lod_count = self.md3_chain_count(&placement.model);
            for level in 1..lod_count {
                let mut level_layers = Vec::new();
                if let Ok(draws) = self.present_md3_ref_entity_layers(
                    1023, &submission, Some(level), Some(&mut level_layers),
                ) {
                    if repeated {
                        let gpu = self.static_md3_gpu_surfaces(&submission, Some(level), &draws);
                        gpu_surfaces_total += gpu.iter().filter(|s| s.is_some()).count();
                        gpu_lods[level - 1] = Some(gpu);
                    }
                    lods[level - 1] = Some(draws);
                    layers[level] = level_layers;
                }
            }
            let radius = self
                .load_md3(&placement.model)
                .ok()
                .and_then(|asset| {
                    asset.model.frames.first().map(|frame| {
                        // RadiusFromBounds over the scaled frame-0 bounds.
                        let corner = |axis: usize| {
                            let scale = placement.scale[axis];
                            (frame.mins[axis] * scale).abs().max((frame.maxs[axis] * scale).abs())
                        };
                        (corner(0).powi(2) + corner(1).powi(2) + corner(2).powi(2)).sqrt()
                    })
                })
                .unwrap_or(0.0);
            let lod_radius = self.load_md3(&placement.model).ok()
                .and_then(|asset| asset.model.frames.first().map(|frame| {
                    let deltas = (0..3).map(|axis| {
                        (frame.maxs[axis] - frame.mins[axis]) * placement.scale[axis]
                    });
                    deltas.map(|d| d*d).sum::<f32>().sqrt() * 0.5
                })).unwrap_or(radius);
            let mut cull_origin = placement.origin;
            cull_origin[2] += 1.0 + placement.zoffset;
            props.push(StaticProp {
                surfaces,
                gpu_surfaces,
                lods,
                gpu_lods,
                lod_count,
                lod_radius,
                origin: placement.origin,
                cull_origin,
                cull_cluster: placement.cull_cluster,
                radius,
                layers,
            });
        }
        self.static_visibility_tree = if self.static_md3_bvh {
            StaticVisibilityTree::build(&props)
        } else { StaticVisibilityTree::default() };
        self.static_draw_order = static_draw_order(&props);
        if self.static_md3_instances {
            devprintln!(1, "STATIC MD3 INSTANCES: {} eligible surface placements (across LODs), {} unique resident meshes; RT shadows automatically use CPU baseline",
                gpu_surfaces_total, self.static_md3_meshes.len());
        }
        devprintln!(
            1,
            "STATIC MODELS: {} of {} misc_model_static placement(s) built, {} surface(s)",
            props.len(),
            source.placements.len(),
            surfaces_total,
        );
        props
    }

    /// Construct the same immutable, model-space mesh once for every repeated
    /// MD3 surface/LOD. Ghoul2's raster path already holds these meshes in
    /// persistent GPU VERTEX/INDEX buffers and batches compatible instances.
    /// No approximation of authored transforms or alpha blending is allowed.
    fn static_md3_gpu_surfaces(
        &mut self, submission: &ItemDraw, forced_lod: Option<usize>,
        reference: &[DynamicModelSurface],
    ) -> Vec<Option<DynamicModelSurface>> {
        let mut result = vec![None; reference.len()];
        let Ok(asset) = self.select_md3(
            &submission.model, submission.origin, 0, 1.0, forced_lod, false,
        ) else { return result; };
        let mut draw_index = 0;
        for surface_asset in &asset.surfaces {
            if surface_asset.invisible_without_override { continue; }
            let Some(source) = asset.model.surfaces.get(surface_asset.surface_index) else { continue; };
            if source.frame_vertices(0).is_none_or(|v| v.is_empty()) || source.indices.is_empty() {
                continue;
            }
            // Base and extra authored stages are emitted in this exact order
            // by present_md3_ref_entity_layers. Don't assume a one-to-one
            // correspondence between authored surfaces and draw calls.
            let stages = std::iter::once(None).chain(surface_asset.extra_stages.iter().map(Some));
            for stage in stages {
                let Some(draw) = reference.get(draw_index) else { return result; };
                // The two dynamic backends submit in different passes. Do not
                // separate coincident authored stages between those passes:
                // even opaque overlays may use depthEqual or exact stage order.
                let single_stage = stage.is_none() && surface_asset.extra_stages.is_empty();
                if single_stage && matches!(draw.alpha_mode,
                    DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask)
                    && source.frame_vertices(0).is_some_and(|vertices| vertices.iter().all(|v| {
                        v.normal.iter().all(|n| n.is_finite())
                            && v.normal.iter().map(|n| n * n).sum::<f32>() > 1.0e-10
                    }))
                {
                    // Geometry source ignores the shader stage, as it does not
                    // affect model-space vertices or triangle winding.
                    let key = format!("static-md3:{}:{:016x}:{}:{}",
                        asset.key, asset.content_hash, surface_asset.surface_index,
                        u8::from(surface_asset.two_sided_indices.is_some()));
                    let (vertices, indices) = self.static_md3_meshes.entry(key.clone()).or_insert_with(|| {
                        let vertices = source.frame_vertices(0).unwrap().iter().map(|v| Ghoul2GpuVertex {
                            position: v.position, normal: v.normal, uv: v.uv,
                            bone_indices: [0; 4], weights: [1.0, 0.0, 0.0, 0.0],
                            weight_count: 1, jiggle_region: u32::MAX,
                            jiggle_weight: 0.0, jiggle_coord: 4.0,
                        }).collect();
                        let indices = surface_asset.two_sided_indices.as_ref()
                            .unwrap_or(&surface_asset.cached_indices);
                        (Arc::new(vertices), Arc::clone(indices))
                    });
                    let mut gpu_draw = draw.clone();
                    gpu_draw.vertices = Arc::new(Vec::new());
                    gpu_draw.indices = Arc::new(Vec::new());
                    gpu_draw.rt_rigid = None;
                    gpu_draw.ghoul2_gpu = Some(Ghoul2GpuSkinning {
                        mesh_key: Arc::from(key),
                        vertices: Arc::clone(vertices),
                        indices: Arc::clone(indices),
                        bones: Arc::clone(&self.static_md3_identity_bone),
                        axis: submission.axis,
                        origin: submission.origin,
                        color: stage.map_or(submission.rgba, |s| md3_stage_color(s, submission.rgba)),
                        uv_xform: [1.0, 1.0, 0.0, 0.0],
                        uv_cross: [0.0; 4],
                        specular: None, bulge_height: 0.0, env_map: false,
                        jiggle_offsets: Default::default(), source_material: None,
                    });
                    result[draw_index] = Some(gpu_draw);
                }
                draw_index += 1;
            }
        }
        // If authored draw/stage enumeration changes, retain the reference
        // draw list instead of binding GPU geometry to the wrong material.
        if draw_index != reference.len() {
            return vec![None; reference.len()];
        }
        result
    }

    /// Targeted filesystem refresh for model registrations that previously
    /// failed because content was absent. Already-loaded MD3/model caches stay
    /// resident; only the VFS namespace, shader definition table and failure
    /// sentinels are refreshed.
    pub fn retry_failed_assets(&mut self) -> Result<usize, String> {
        self.assets
            .refresh()
            .map_err(|error| format!("ENTITY ASSET REFRESH ERROR: {error}"))?;
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut self.assets, &mut shader_warnings, self.pbr)?;
        self.shaders = shaders;
        for warning in shader_warnings {
            rverbose!(1, "ENTITY MATERIAL REFRESH WARNING: {warning}");
        }
        devprintln!(
            1,
            "ENTITY ASSET REFRESH: shaderDefs={} mtrDefs={}",
            diagnostics.shader_definitions, diagnostics.mtr_definitions
        );
        let retried = self.failed_models.len();
        self.failed_models.clear();
        self.fx_materials.clear();
        self.static_props = None;
        Ok(retried)
    }

    /// `ghoul2` draws Ghoul2 world models (weapon items) through the shared
    /// player/saber Ghoul2 path, so both presenters use one model cache.
    pub fn present_snapshot_entities(
        &mut self,
        entities: &[PresentedEntity],
        game: &ClientGameState,
        time: i32,
        ghoul2: &mut PlayerPresenter,
        weapon_fx: &mut WeaponFx,
        rt_rigid_casters_enabled: bool,
    ) -> (Vec<DynamicModelSurface>, Vec<InlineModelInstance>, EntityDispatchSummary) {
        self.rt_rigid_casters_enabled = rt_rigid_casters_enabled;
        let mut draws = Vec::new();
        let mut inline_models = Vec::new();
        let mut summary = EntityDispatchSummary::default();
        let intermission = game.rendering_intermission();
        // Only scan active turret slots: ordinary maps do no extra allocations
        // or target lookup work on the hot entity presentation path.
        if !self.turret_states.is_empty() {
            self.turret_states.retain(|number, _| entities.iter().any(|e|
                e.number == *number && e.state.field_i32("modelGhoul2").unwrap_or(0) != 0
                    && e.state.field_i32("bolt1").unwrap_or(0) != 0));
        }

        for entity in entities {
            // TaystJK CG_AddCEntity returns before lerp/effects/dispatch for
            // general/player/invisible entities during intermission, and for
            // vehicle NPCs. Player/NPC meshes are owned by PlayerPresenter in
            // this client, but applying the same gate here keeps generic models
            // and vehicle-side work aligned with the original dispatcher.
            if suppressed_during_intermission(intermission, entity) {
                continue;
            }
            match entity.presentation_kind() {
                EntityPresentationKind::General
                | EntityPresentationKind::Holocron => {
                    summary.general += 1;
                    self.present_configstring_model(entity, game, time, ghoul2, weapon_fx, entities, &mut draws);
                }
                EntityPresentationKind::Body => {
                    // ET_BODY is a Ghoul2 corpse copied from a client entity.
                    // OpenJK sends it through CG_G2Animated -> CG_Player, with
                    // CG_RagDoll immediately before/inside player angles.
                    // PlayerPresenter owns that same Ghoul2 presentation seam.
                    summary.general += 1;
                }
                EntityPresentationKind::Mover => {
                    summary.movers += 1;
                    self.present_mover(entity, game, &mut draws, &mut inline_models);
                }
                EntityPresentationKind::Item => {
                    summary.items += 1;
                    self.present_item(entity, game, time, ghoul2, weapon_fx, &mut draws);
                }
                EntityPresentationKind::Missile => {
                    summary.missiles += 1;
                    // CG_Missile: the trail effect plays inside WeaponFx;
                    // model-carrying missiles also submit their refEntity.
                    if let Some(model) = weapon_fx.missile(entity, game, time) {
                        let submission = ItemDraw {
                            model: model.qpath.to_owned(),
                            origin: model.origin,
                            axis: model.axis,
                            rgba: [1.0; 4],
                            custom_shader: None,
                        };
                        match self.present_md3_ref_entity(entity.number, &submission) {
                            Ok(mut surfaces) => draws.append(&mut surfaces),
                            Err(error) => self.log_entity_once(entity, &format!("missile model {}: {error}", model.qpath)),
                        }
                    }
                }
                EntityPresentationKind::Npc => {
                    // CG_G2Animated -> CG_Player: presented by PlayerPresenter
                    // with the NPC's own clientinfo.
                    summary.npcs += 1;
                }
                EntityPresentationKind::Fx => {
                    summary.fx += 1;
                    // OpenJK CG_FX. Map-authored fx_runner entities are server
                    // entities of type ET_FX; modelindex/modelindex2 carry the
                    // effect resource and off/one-shot/continuous state.
                    weapon_fx.entity_fx(entity, game);
                }
                EntityPresentationKind::Player
                | EntityPresentationKind::Invisible
                | EntityPresentationKind::PushTrigger
                | EntityPresentationKind::TeleportTrigger
                | EntityPresentationKind::Event => {}
                kind => {
                    summary.other += 1;
                    let key = format!("kind:{kind:?}");
                    if self.logged_unsupported.insert(key) {
                        devprintln!(2, "ENTITY DISPATCH TODO: {kind:?}");
                    }
                }
            }
        }

        // CG_AddLocalEntities: LE_FRAGMENT debris from EV_DEBRIS.
        for chunk in weapon_fx.chunk_models().into_iter().chain(ghoul2.physics_debris_models(time)) {
            let submission = ItemDraw {
                model: chunk.qpath.clone(),
                origin: chunk.origin,
                axis: chunk.axis,
                rgba: [1.0, 1.0, 1.0, chunk.alpha],
                custom_shader: None,
            };
            match self.present_md3_ref_entity(1023, &submission) {
                Ok(mut surfaces) => draws.append(&mut surfaces),
                Err(error) => {
                    if self.logged_unsupported.insert(format!("chunk:{}", chunk.qpath)) {
                        println!("CHUNK MODEL {}: {error}", chunk.qpath);
                    }
                }
            }
        }

        summary.rendered_surfaces = draws.len();
        (draws, inline_models, summary)
    }

    /// TaystJK/OpenJK `CG_DrawPlayerSphere` for `EF_INVULNERABLE`.
    ///
    /// This is intentionally driven only by the network `eFlags` bit. Do not
    /// infer a spawn timer locally: server mods control exactly when protection
    /// begins and ends. TaystJK registers `models/weaphits/testboom.md3`, moves
    /// it 9 units above the player origin, faces the half-shield toward
    /// `cg.refdef.vieworg`, and overrides its material with
    /// `powerups/invulnerabilityshell`.
    pub fn present_invulnerability_bubbles(
        &mut self,
        entities: &[PresentedEntity],
        followed: Option<&PresentedEntity>,
        game: &ClientGameState,
        view_origin: [f32; 3],
    ) -> Vec<DynamicModelSurface> {
        let intermission = game.rendering_intermission();
        let mut seen = HashSet::new();
        let mut draws = Vec::new();
        for entity in followed.into_iter().chain(entities.iter()) {
            if entity.entity_type != super::ET_PLAYER
                || suppressed_during_intermission(intermission, entity)
                || !seen.insert(entity.number)
            {
                continue;
            }
            let flags = entity.state.field_i32("eFlags").unwrap_or(0);
            if flags & EF_INVULNERABLE == 0 || flags & EF_DEAD != 0 {
                continue;
            }
            let Some((origin, axis)) = invulnerability_sphere_transform(entity.origin, view_origin) else {
                continue;
            };
            match self.present_custom_md3_stages(
                entity.number,
                INVULNERABILITY_MODEL,
                origin,
                axis,
                [1.0; 4],
                INVULNERABILITY_SHADER,
            ) {
                Ok(mut sphere) => draws.append(&mut sphere),
                Err(error) => {
                    let key = format!("invulnerability:{}", entity.number);
                    if self.logged_unsupported.insert(key) {
                        rverbose!(1, "INVULNERABILITY BUBBLE: {error}");
                    }
                }
            }
        }
        draws
    }

    /// A `customShader` can be multi-stage. The ordinary entity override path
    /// predates that requirement and takes its first stage; the player sphere
    /// uses this complete variant so its authored shell material is not reduced
    /// to an arbitrary single pass.
    fn present_custom_md3_stages(
        &mut self,
        entity_num: u16,
        qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        shader_name: &str,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let asset = self.load_md3(qpath)?;
        if self.is_noop_shader(shader_name) {
            return Ok(Vec::new());
        }
        let materials = self.fx_material_stages(shader_name);
        // The override shader owns cull state (halfShieldShell is `cull
        // twosided`); the model's own surface shaders do not apply here.
        let effective = self.shader_remaps.effective_name(shader_name);
        let two_sided = jka_assets::shader::find_shader(&self.shaders, &effective).is_some_and(|shader| {
            matches!(shader.cull.trim().to_ascii_lowercase().as_str(), "none" | "twosided" | "two-sided" | "disable")
        });
        let mut draws = Vec::with_capacity(asset.surfaces.len() * materials.len().max(1));
        for surface_asset in &asset.surfaces {
            let Some(surface) = asset.model.surfaces.get(surface_asset.surface_index) else { continue };
            let Some(source_vertices) = surface.frame_vertices(0) else { continue };
            if source_vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            for (stage, material) in materials.iter().enumerate() {
                let color = [
                    if material.rgb_vertex { rgba[0] } else { material.rgb_const[0] },
                    if material.rgb_vertex { rgba[1] } else { material.rgb_const[1] },
                    if material.rgb_vertex { rgba[2] } else { material.rgb_const[2] },
                    if material.alpha_vertex { rgba[3] } else { material.alpha_const },
                ];
                let (vertices, indices) = self.md3_geometry(
                    &asset, surface_asset, entity_num, stage, 0, axis, origin,
                    1.0, color, 0.0);
                let indices = md3_two_sided_indices(&indices, two_sided);
                draws.push(DynamicModelSurface {
                    geometry_source: crate::renderer::ModelGeometrySource::Md3,
                authored_lighting: false,
                    material_trace: Some(Arc::from(format!(
                        "CUSTOM SHADER OVERRIDE: {shader_name}\nCUSTOM STAGE: {stage} / {}\n{}",
                        materials.len(), surface_asset.material_trace,
                    ))),
                    entity_num,
                    wireframe_class: DynamicWireframeClass::Entity,
                    raster_visible: true,
                    vertices,
                    indices,
                    lighting_origin: Some(origin),
                    // A translucent presentation shell must never become a
                    // physical RT shadow caster.
                    rt_rigid: None,
                    rt_skinned_key: None,
                    ghoul2_gpu: None,
                    fx_gpu_sprites: None,
                    texture: material.texture.clone(),
                    alpha_mode: blend_for_alpha(material.blend.custom_shader_alpha_mode(), color[3]),

                    authored_glow: material.authored_glow,
                });
            }
        }
        Ok(draws)
    }

    fn present_configstring_model(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        time: i32,
        ghoul2: &mut PlayerPresenter,
        weapon_fx: &mut WeaponFx,
        entities: &[PresentedEntity],
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        // Holocrons are gameplay pickups that share this path, not map props.
        if !self.draw_map_models
            && !matches!(entity.presentation_kind(), EntityPresentationKind::Holocron)
        {
            return;
        }
        if entity.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW != 0 {
            return;
        }
        let model_ghoul2 = entity.state.field_i32("modelGhoul2").unwrap_or(0);
        if model_ghoul2 != 0 {
            // OpenJK uses 127 as the "Ghoul2 requested but not initialized"
            // sentinel and CG_General returns without drawing it.
            if model_ghoul2 == 127 {
                return;
            }
            // OpenJK does not present an in-flight player saber through the
            // generic ET_GENERAL model path. CG_Player owns saberEntityNum and
            // manually submits the saber Ghoul2 instance/blades. The Rust
            // PlayerPresenter mirrors that path, so do not also present saber
            // or detached Ghoul2 model-part entities as generic props.
            if matches!(entity.state.field_i32("weapon").unwrap_or(0), WP_SABER | G2_MODEL_PART) {
                return;
            }

            let model_index = entity.state.field_i32("modelindex").unwrap_or(0);
            let Some(qpath) = game.model_qpath(model_index) else {
                self.log_entity_once(entity, &format!("unresolved Ghoul2 CS_MODELS index {model_index}"));
                return;
            };
            if entity.state.field_i32("bolt1").unwrap_or(0) != 0 {
                let fireflag = entity.state.field_i32("fireflag").unwrap_or(0);
                let target = u16::try_from(entity.state.field_i32("bolt2").unwrap_or(1023))
                    .ok().filter(|&n| n < 1023)
                    .and_then(|n| entities.iter().find(|e| e.number == n))
                    .map(|e| super::entity_vec3(&e.state, "pos.trBase").unwrap_or(e.origin));
                let turret = self.turret_states.entry(entity.number).or_insert_with(||
                    TurretState::new(model_index, time));
                if turret.model_index != model_index || time < turret.last_time {
                    *turret = TurretState::new(model_index, time);
                }
                let flash = turret.update(time, entity.origin, target, fireflag);
                match ghoul2.present_turret_glm(
                    entity.number, &qpath, entity.origin, angles_to_axis(entity.angles),
                    entity_color(entity), turret.started_at, time, turret.angles, flash,
                ) {
                    Ok((mut model_draws, muzzle)) => {
                        draws.append(&mut model_draws);
                        if let Some((origin, dir)) = muzzle {
                            weapon_fx.turret_muzzle(origin, dir);
                        }
                    }
                    Err(error) => self.log_entity_once(entity, &format!("turret Ghoul2 {qpath}: {error}")),
                }
                return;
            }
            self.turret_states.remove(&entity.number);
            match ghoul2.present_static_glm(
                entity.number,
                &qpath,
                entity.origin,
                angles_to_axis(entity.angles),
                entity_color(entity),
                None,
                time,
            ) {
                Ok(mut model_draws) => draws.append(&mut model_draws),
                Err(error) => self.log_entity_once(entity, &format!("Ghoul2 {qpath}: {error}")),
            }
            return;
        }
        self.present_model_index(entity, game, entity.state.field_i32("modelindex").unwrap_or(0), draws);
    }

    /// OpenJK CG_Mover. Unlike CG_General it has no EF_NODRAW gate; only the
    /// vehicle hyperspace brush is suppressed (its in-hyperspace draw needs
    /// the predicted vehicle state, which demos of ordinary play never use).
    fn present_mover(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        draws: &mut Vec<DynamicModelSurface>,
        inline_models: &mut Vec<InlineModelInstance>,
    ) {
        if entity.state.field_i32("eFlags2").unwrap_or(0) & EF2_HYPERSPACE != 0 {
            return;
        }
        let model_index = entity.state.field_i32("modelindex").unwrap_or(0);
        if entity.state.field_i32("solid").unwrap_or(0) == SOLID_BMODEL {
            // ent.hModel = cgs.inlineDrawModel[modelindex], placed at
            // lerpOrigin with AnglesToAxis(lerpAngles).
            if let Ok(model) = u32::try_from(model_index) {
                if model > 0 {
                    inline_models.push(InlineModelInstance {
                        model,
                        origin: entity.origin,
                        axis: angles_to_axis(entity.angles),
                        // CG_AddPacketEntities(qtrue) admits only snapshot
                        // entities marked by the server's G_PortalifyEntities.
                        is_portal_entity: entity.state.field_i32("isPortalEnt").unwrap_or(0) != 0,
                    });
                }
            }
        } else if self.draw_map_models {
            self.present_model_index(entity, game, model_index, draws);
        }

        // OpenJK CG_Mover submits modelindex2 as a second model on the same
        // transform.  Supporting it here also exercises CS_MODELS lookup without
        // pretending inline BSP models are ordinary registered model qpaths.
        let model_index2 = entity.state.field_i32("modelindex2").unwrap_or(0);
        if model_index2 > 0 && self.draw_map_models {
            self.present_model_index(entity, game, model_index2, draws);
        }
    }

    /// OpenJK CG_Item: `bg_itemlist[modelindex]` placed by `cg_item`, then
    /// each submission drawn as an MD3 or Ghoul2 refEntity.
    fn present_item(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        time: i32,
        ghoul2: &mut PlayerPresenter,
        weapon_fx: &mut WeaponFx,
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        let index = entity.state.field_i32("modelindex").unwrap_or(0);
        let Some(item) = jka_movement::bg_item(index) else {
            self.log_entity_once(entity, &format!("bad item index {index} (bg_numItems {})", jka_movement::bg_item_count()));
            return;
        };
        if index > 0 && !game.server_registers_item(index as usize) {
            // OpenJK only precaches CS_ITEMS items at level load; report the
            // mismatch once but still present the entity the server sent.
            self.log_entity_once(entity, &format!("{} not in the CS_ITEMS precache", item.classname));
        }
        let weapon_midpoint = if item.item_type == IT_WEAPON
            && !item.world_model.to_ascii_lowercase().ends_with(".glm")
        {
            // R_ModelBounds of an MD3 world model: frame 0 bounds midpoint.
            self.load_md3(&item.world_model)
                .ok()
                .and_then(|asset| asset.model.frames.first().map(|frame| {
                    std::array::from_fn(|i| frame.mins[i] + 0.5 * (frame.maxs[i] - frame.mins[i]))
                }))
                .unwrap_or([0.0; 3])
        } else {
            [0.0; 3]
        };
        let input = ItemInput {
            number: entity.number,
            origin: entity.origin,
            angles: super::entity_vec3(&entity.state, "angles").unwrap_or([0.0; 3]),
            e_flags: entity.state.field_i32("eFlags").unwrap_or(0),
            time,
            misc_time: game.entity_misc_time(entity.number),
            force_side: game
                .current_snapshot()
                .and_then(|snapshot| snapshot.player_state.field_i32("fd.forceSide"))
                .unwrap_or(0),
            weapon_midpoint,
            gametype: game.gametype(),
        };
        if let Some(origin) = item_cone_origin(&item, &input) {
            weapon_fx.item_cone(entity.number, origin);
        }
        for submission in cg_item(&item, &input, angles_to_axis) {
            let result = if submission.model.to_ascii_lowercase().ends_with(".glm") {
                ghoul2.present_static_glm(
                    entity.number,
                    &submission.model,
                    submission.origin,
                    submission.axis,
                    submission.rgba,
                    submission.custom_shader,
                    time,
                )
            } else {
                self.present_md3_ref_entity(entity.number, &submission)
            };
            match result {
                Ok(mut surfaces) => draws.append(&mut surfaces),
                Err(error) => self.log_entity_once(entity, &format!("{} ({}): {error}", item.classname, submission.model)),
            }
        }
    }

    /// Player attachments: jaPRO cosmetics and carried powerup flag MD3s.
    /// They resolve through this presenter's own VFS like any other model, so
    /// they only appear when the asset search path provides them (`japro-assets.pk3` in base).
    pub fn present_cosmetics(
        &mut self,
        cosmetics: Vec<super::player_presenter::CosmeticDraw>,
    ) -> Vec<DynamicModelSurface> {
        let mut draws = Vec::new();
        for cosmetic in cosmetics {
            if cosmetic.model.ends_with(".djk_prop") {
                match self.present_imported_prop(cosmetic.entity_num,&cosmetic.model,cosmetic.origin,cosmetic.axis,cosmetic.rgba,cosmetic.skin_pose.as_deref().map(|v|v.as_slice())) {
                    Ok(mut surfaces)=>draws.append(&mut surfaces),
                    Err(error)=>{if self.missing_cosmetics.insert(cosmetic.model.clone()){println!("EMOTE PROP {}: {error}",cosmetic.model);}}
                }
                continue;
            }
            let axis = match cosmetic.face_camera {
                Some(locked) => facing_axis(cosmetic.axis, cosmetic.origin, self.material_eye, locked),
                None => cosmetic.axis,
            };
            let submission = ItemDraw {
                model: cosmetic.model.to_owned(),
                origin: cosmetic.origin,
                axis,
                rgba: cosmetic.rgba,
                custom_shader: cosmetic.custom_shader,
            };
            match self.present_md3_ref_entity(cosmetic.entity_num, &submission) {
                Ok(mut surfaces) => {
                    if cosmetic.custom_shader.is_some_and(|s| s.starts_with("fnemote/fx/")) {
                        self.fresnel_fade_fx_mesh(&mut surfaces);
                    }
                    draws.append(&mut surfaces)
                }
                Err(error) => {
                    if self.missing_cosmetics.insert(cosmetic.model.clone()) {
                        println!("COSMETIC UNAVAILABLE: {}: {error}", cosmetic.model);
                    }
                }
            }
        }
        draws
    }

    pub fn set_material_eye(&mut self,eye:[f32;3]) { self.material_eye=eye; }
    /// Translucent effect meshes (glow bodies, light cones, shells) get their
    /// opacity from a Fresnel/angle term in their UE material graph, not from
    /// particle alpha; the port keeps only a white or mask texture. Fade them
    /// where the surface faces the viewer so they read as volumes, not solids.
    fn fresnel_fade_fx_mesh(&self, surfaces: &mut [DynamicModelSurface]) {
        let eye = scene::render_position(self.material_eye);
        for surface in surfaces {
            let translucent = matches!(surface.alpha_mode, DynamicModelAlphaMode::Blend | DynamicModelAlphaMode::BlendUnlit | DynamicModelAlphaMode::Additive | DynamicModelAlphaMode::AdditiveOne);
            let flat = surface.texture.as_ref().is_some_and(|t| {
                let l = t.label.to_ascii_lowercase();
                l.contains("/fx/white") || l.contains("/fx/soft") || l.contains("/fx/hard") || l.rsplit('_').next().is_some_and(|tail| tail.starts_with('m') && tail.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
            });
            if !translucent || !flat {
                continue;
            }
            let rgb_scaled = surface.alpha_mode == DynamicModelAlphaMode::AdditiveOne;
            for v in Arc::make_mut(&mut surface.vertices) {
                let to_eye = [eye[0] - v.position[0], eye[1] - v.position[1], eye[2] - v.position[2]];
                let len = (to_eye[0] * to_eye[0] + to_eye[1] * to_eye[1] + to_eye[2] * to_eye[2]).sqrt().max(1e-4);
                let nl = (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1] + v.normal[2] * v.normal[2]).sqrt().max(1e-4);
                let facing = ((v.normal[0] * to_eye[0] + v.normal[1] * to_eye[1] + v.normal[2] * to_eye[2]) / (len * nl)).abs();
                let k = (1. - facing).powi(2);
                if rgb_scaled {
                    for c in &mut v.color[..3] {
                        *c *= k;
                    }
                } else {
                    v.color[3] *= k;
                }
            }
        }
    }
    fn ue_material(&mut self,shader:&str)->Option<Arc<jka_ue_assets::material::Program>> {
        if let Some(cached)=self.ue_materials.get(shader){return cached.clone()}
        let program=match self.assets.read(&format!("{shader}.djk_material"),256*1024){
            Ok(Some(asset))=>match jka_ue_assets::material::Program::parse(&asset.bytes){Ok(p)=>Some(Arc::new(p)),Err(error)=>{eprintln!("UE MATERIAL {shader}: {error}");None}},
            Ok(None)=>None,Err(error)=>{eprintln!("UE MATERIAL {shader}: {error}");None}
        };
        self.ue_materials.insert(shader.into(),program.clone());program
    }
    fn present_imported_prop(&mut self,entity_num:u16,path:&str,origin:[f32;3],axis:[[f32;3];3],rgba:[f32;4],pose:Option<&[jka_assets::ghoul2::Matrix3x4]>)->Result<Vec<DynamicModelSurface>,String>{
        let mesh=if let Some(mesh)=self.imported_props.get(path){mesh.clone()}else{
            let mesh=if let Some(mesh)=crate::emotes::prepared_prop_mesh(path){mesh}else{
            let bytes=self.assets.read(path,32*1024*1024).map_err(|e|e.to_string())?.ok_or("missing skeletal prop")?.bytes;
            Arc::new(jka_emotes::prop_mesh::Mesh::parse(&bytes)?)};
            if self.imported_props.values().map(|m|m.vertices.len()*std::mem::size_of::<jka_emotes::prop_mesh::Vertex>()+m.indices.len()*4).sum::<usize>()>64*1024*1024{self.imported_props.clear();self.prop_gpu_vertices.clear();self.prop_normals.clear();}
            self.imported_props.insert(path.into(),mesh.clone());mesh
        };
        if pose.is_some_and(|p|p.len()!=mesh.bones.len()){return Err("prop animation/skeleton mismatch".into());}
        let vertices=mesh.vertices.iter().map(|v|{
            let mut p=[0.;3];let mut n=[0.;3];
            for i in 0..4{if v.weights[i]==0.{continue;}let (point,normal)=if let Some(pose)=pose{let m=&pose[v.bones[i]];(std::array::from_fn(|r|m[r][0]*v.position[0]+m[r][1]*v.position[1]+m[r][2]*v.position[2]+m[r][3]),std::array::from_fn(|r|m[r][0]*v.normal[0]+m[r][1]*v.normal[1]+m[r][2]*v.normal[2]))}else{(v.position,v.normal)};for r in 0..3{p[r]+=point[r]*v.weights[i];n[r]+=normal[r]*v.weights[i];}}
            DynamicModelVertex{position:transform_model_point(p,axis,origin,1.),normal:transform_model_normal(n,axis),uv:v.uv,color:rgba,depth_hack:0.,water_wetness:0.}
        }).collect::<Vec<_>>();
        if vertices.iter().any(|v|v.position.iter().chain(&v.normal).any(|n|!n.is_finite())){return Err("non-finite prop pose".into());}
        let mut draws=Vec::new();let seconds=self.material_clock.elapsed().as_secs_f32();
        let bones=Arc::new((0..mesh.bones.len()).map(|i|{
            let m=pose.map(|p|p[i]).unwrap_or([[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]]);crate::renderer::Ghoul2GpuBone{row0:m[0],row1:m[1],row2:m[2]}
        }).collect::<Vec<_>>());
        for section in &mesh.sections {
            let program=self.ue_material(&section.shader);
            let definition=jka_assets::shader::find_shader(&self.shaders,&section.shader).cloned();
            let normal=definition.as_ref().and_then(Shader::primary).and_then(|s|s.normal_map.clone());
            let normal=normal.and_then(|name|{
                if let Some(normal)=self.prop_normals.get(&name){return normal.clone();}
                let normal=self.load_texture_arc(&name,false).map(|n|{let mut n=(*n).clone();n.srgb=false;Arc::new(n)});
                self.prop_normals.insert(name,normal.clone());normal
            });
            for (stage,material) in self.fx_material_stages(&section.shader).into_iter().enumerate(){
            let color=material.float_vertex_color(rgba);let mut tinted=vertices.clone();for v in &mut tinted{v.color=color;for modifier in &material.tc_mods{v.uv=crate::fx::draw::modified_uv(v.uv,v.position,*modifier,seconds-material.shader_time_offset);}}
            if let Some(program)=&program {
                for vertex in &mut tinted {
                    let sample=jka_ue_runtime::material::Sample{seconds,position:glam::Vec3::from_array(vertex.position)-glam::Vec3::from_array(scene::render_position(origin)),normal:glam::Vec3::from_array(vertex.normal),uv:vertex.uv,eye:glam::Vec3::from_array(scene::render_position(self.material_eye))-glam::Vec3::from_array(scene::render_position(origin))};
                    let value=jka_ue_runtime::material::evaluate(program,&sample)?;
                    vertex.color=std::array::from_fn(|i|value[i]*rgba[i]);
                }
            }
            let mut indices=Vec::new();for tri in mesh.indices[section.first..section.end].chunks_exact(3){indices.extend([tri[0],tri[2],tri[1]]);}
            if definition.as_ref().is_some_and(|s|s.cull.eq_ignore_ascii_case("none")||s.cull.eq_ignore_ascii_case("twosided")){let front=indices.clone();for tri in front.chunks_exact(3){indices.extend([tri[2],tri[1],tri[0]]);}}
            let indices=Arc::new(indices);
            let gpu=if stage==0&&material.tc_mods.is_empty()&&program.is_none(){normal.as_ref().map(|normal|{
                let vertices=self.prop_gpu_vertices.entry(path.into()).or_insert_with(||Arc::new(mesh.vertices.iter().map(|v|crate::renderer::Ghoul2GpuVertex{position:v.position,normal:v.normal,uv:v.uv,bone_indices:v.bones.map(|n|n as u32),weights:v.weights,weight_count:4,jiggle_region:u32::MAX,jiggle_weight:0.,jiggle_coord:4.}).collect())).clone();
                crate::renderer::Ghoul2GpuSkinning{mesh_key:Arc::from(format!("{path}#native{}",section.first)),vertices,indices:indices.clone(),bones:bones.clone(),axis,origin,color,uv_xform:[1.,1.,0.,0.],uv_cross:[0.;4],specular:None,bulge_height:0.,env_map:false,jiggle_offsets:Default::default(),source_material:Some(crate::renderer::SourceMaterialParams{normal_map:Some(normal.clone()),phong_boost:0.15,phong_exponent:32.,phong_fresnel:[0.,0.5,1.],rimlight_boost:0.,rimlight_exponent:1.,half_lambert:false})}
            })}else{None};
            let masked=definition.as_ref().and_then(|s|s.stages.get(stage)).is_some_and(|s|!s.alpha_test.is_empty());
            draws.push(DynamicModelSurface{geometry_source:crate::renderer::ModelGeometrySource::Md3,authored_lighting:false,material_trace:None,entity_num,wireframe_class:DynamicWireframeClass::Entity,raster_visible:true,vertices:Arc::new(tinted),indices,lighting_origin:if program.as_ref().is_some_and(|p|p.unlit){None}else{Some(origin)},rt_rigid:None,rt_skinned_key:None,ghoul2_gpu:gpu,fx_gpu_sprites:None,texture:material.texture,alpha_mode:if masked{DynamicModelAlphaMode::Mask}else{material.blend.custom_shader_alpha_mode()},authored_glow:material.authored_glow||program.as_ref().is_some_and(|p|p.unlit)});
        }}Ok(draws)
    }

    /// Retain only the latest pose for each entity/model/surface/stage slot.
    /// Changed frames/transforms/colours/depth hacks use the original CPU math.
    #[allow(clippy::too_many_arguments)]
    fn md3_geometry(
        &mut self,
        asset: &Md3Asset,
        surface_asset: &Md3SurfaceAsset,
        entity: u16,
        stage: usize,
        frame: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        scale: f32,
        color: [f32; 4],
        depth_hack: f32,
    ) -> (Arc<Vec<DynamicModelVertex>>, Arc<Vec<u32>>) {
        let slot = Md3GeometrySlot {
            model: Arc::as_ptr(&asset.model) as usize,
            surface: surface_asset.surface_index,
            entity,
            depth_hack: depth_hack.to_bits(),
            stage,
        };
        let input = md3_geometry_input(axis, origin, scale, color, depth_hack);
        if self.md3_geometry_cache_enabled {
            if let Some(cached) = self.md3_geometry.get(&slot)
                .filter(|cached| cached.frame == frame && cached.input == input)
            {
                return (Arc::clone(&cached.vertices), Arc::clone(&cached.indices));
            }
        }
        let surface = &asset.model.surfaces[surface_asset.surface_index];
        let vertices = Arc::new(surface.frame_vertices(frame).unwrap_or_default().iter()
            .map(|vertex| DynamicModelVertex {
                position: transform_model_point(vertex.position, axis, origin, scale),
                normal: transform_model_normal(vertex.normal, axis),
                uv: vertex.uv,
                color,
                depth_hack,
                water_wetness: 0.0,
            }).collect::<Vec<_>>());
        let indices = if self.md3_geometry_cache_enabled {
            Arc::clone(&surface_asset.cached_indices)
        } else {
            Arc::new(flipped_md3_indices(&surface.indices))
        };
        if self.md3_geometry_cache_enabled {
            let required = std::mem::size_of_val(vertices.as_slice());
            if let Some(old) = self.md3_geometry.remove(&slot) {
                self.md3_geometry_bytes -= std::mem::size_of_val(old.vertices.as_slice());
            }
            if self.md3_geometry_bytes.saturating_add(required) > MAX_MD3_BYTES {
                self.md3_geometry.clear();
                self.md3_geometry_bytes = 0;
            }
            if required <= MAX_MD3_BYTES {
                self.md3_geometry_bytes += required;
                self.md3_geometry.insert(slot, CachedMd3Geometry {
                    _model: Arc::clone(&asset.model), frame, input,
                    vertices: Arc::clone(&vertices), indices: Arc::clone(&indices),
                });
            }
        }
        (vertices, indices)
    }

    /// Additional authored MD3 shader stages. This is deliberately stage-driven:
    /// no model/shader-name special cases, and source vertices survive remaps.
    /// The first selected stage uses the existing primary material path.
    #[allow(clippy::too_many_arguments)]
    fn present_md3_extra_stages(
        &mut self, draws: &mut Vec<DynamicModelSurface>,
        asset: &Md3Asset, surface_asset: &Md3SurfaceAsset,
        entity_num: u16, frame: usize, axis: [[f32; 3]; 3],
        origin: [f32; 3], scale: f32, entity_color: [f32; 4], depth_hack: f32,
    ) {
        let camera = self.md3_lod_view.map(|view| view.camera_position());
        for stage in &surface_asset.extra_stages {
            let stage_color = md3_stage_color(stage, entity_color);
            let (vertices, indices) = self.md3_geometry(
                asset, surface_asset, entity_num, stage.index + 1, frame,
                axis, origin, scale, stage_color, depth_hack,
            );
            let vertices = if matches!(stage.tc_gen, TcGen::Base) && stage.tc_mods.is_empty() {
                vertices
            } else {
                Arc::new(vertices.iter().map(|vertex| {
                    let mut transformed = *vertex;
                    transformed.uv = md3_stage_uv(vertex, stage, camera);
                    transformed
                }).collect())
            };
            let indices = surface_asset.two_sided_indices.as_ref()
                .map(Arc::clone).unwrap_or(indices);
            draws.push(DynamicModelSurface {
                geometry_source: crate::renderer::ModelGeometrySource::Md3,
                authored_lighting: matches!(stage.rgb_gen, RgbGen::LightingDiffuse | RgbGen::LightingDiffuseEntity),
                material_trace: Some(Arc::from(format!(
                    "MD3 AUTHORED STAGE {} (of {})\n{}", stage.index,
                    surface_asset.extra_stages.len() + 1, surface_asset.material_trace,
                ))),
                entity_num,
                wireframe_class: DynamicWireframeClass::Entity,
                raster_visible: true,
                vertices, indices,
                lighting_origin: Some(origin),
                // Reflection, detail, translucency and other overlay stages do
                // not create an additional rigid shadow caster.
                rt_rigid: None,
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture: stage.texture.clone(),
                alpha_mode: stage.alpha_mode,
                authored_glow: stage.authored_glow,
            });
        }
    }

    /// An MD3 refEntity with an explicit transform (frame 0), shaderRGBA and
    /// optional customShader.
    fn present_md3_ref_entity(
        &mut self,
        entity_num: u16,
        submission: &ItemDraw,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        if submission.custom_shader.is_none() && submission.model.starts_with("models/fnemote/") {
            return self.present_static_md3(entity_num,&submission.model,submission.origin,submission.axis,submission.rgba,0);
        }
        self.present_md3_ref_entity_lod(entity_num, submission, None)
    }

    fn present_md3_ref_entity_lod(
        &mut self,
        entity_num: u16,
        submission: &ItemDraw,
        forced_lod: Option<usize>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        self.present_md3_ref_entity_layers(entity_num, submission, forced_lod, None)
    }

    /// `layers` receives each draw's shader-stage index (0 = base stage), so a
    /// caller that reorders draws can keep coincident stages in authored order.
    fn present_md3_ref_entity_layers(
        &mut self,
        entity_num: u16,
        submission: &ItemDraw,
        forced_lod: Option<usize>,
        mut layers: Option<&mut Vec<u8>>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let asset = self.select_md3(&submission.model, submission.origin, 0, 1.0, forced_lod, false)?;
        if submission.custom_shader.is_some_and(|shader| self.is_noop_shader(shader)) {
            return Ok(Vec::new());
        }
        let custom = submission.custom_shader.map(|shader| {
            let material = self.fx_material(shader);
            (
                material.texture,
                material.blend.custom_shader_alpha_mode(),
                material.authored_glow,
            )
        });
        let mut draws = Vec::with_capacity(asset.surfaces.len());
        for surface_asset in &asset.surfaces {
            // The explicit customShader overrides the MD3's surface shader.
            if custom.is_none() && surface_asset.invisible_without_override {
                continue;
            }
            let Some(surface) = asset.model.surfaces.get(surface_asset.surface_index) else { continue };
            let Some(vertices) = surface.frame_vertices(0) else { continue };
            if vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let (vertices, indices) = self.md3_geometry(
                &asset, surface_asset, entity_num, 0, 0, submission.axis,
                submission.origin, 1.0, submission.rgba, 0.0);
            let (texture, alpha_mode, authored_glow) = custom.clone().unwrap_or_else(|| {
                (
                    surface_asset.texture.clone(),
                    surface_asset.alpha_mode,
                    surface_asset.authored_glow,
                )
            });
            draws.push(DynamicModelSurface {
                geometry_source: crate::renderer::ModelGeometrySource::Md3,
                authored_lighting: submission.custom_shader.is_none() && surface_asset.authored_lighting,
                material_trace: Some(if let Some(shader) = submission.custom_shader {
                    Arc::from(format!("CUSTOM SHADER OVERRIDE: {shader}\n{}", surface_asset.material_trace))
                } else {
                    Arc::clone(&surface_asset.material_trace)
                }),
                entity_num,
                wireframe_class: DynamicWireframeClass::Entity,
                raster_visible: true,
                vertices,
                indices: surface_asset.two_sided_indices.as_ref()
                    .map(Arc::clone).unwrap_or(indices),
                lighting_origin: Some(submission.origin),
                rt_rigid: self.rt_rigid_casters_enabled.then(|| RayTracedRigidInstance::md3(
                    Arc::clone(&asset.key),
                    Arc::clone(&asset.model),
                    surface_asset.surface_index,
                    0,
                    submission.axis,
                    submission.origin,
                    1.0,
                )),
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture,
                alpha_mode: blend_for_alpha(alpha_mode, submission.rgba[3]),

                authored_glow,
            });
            if submission.custom_shader.is_none() {
                self.present_md3_extra_stages(
                    &mut draws, &asset, surface_asset, entity_num, 0, submission.axis,
                    submission.origin, 1.0, submission.rgba, 0.0,
                );
            }
            if let Some(layers) = layers.as_deref_mut() {
                layers.extend((0..draws.len() - layers.len()).map(|stage| stage.min(255) as u8));
            }
        }
        Ok(draws)
    }

    fn present_model_index(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        model_index: i32,
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        let Some(qpath) = game.model_qpath(model_index) else {
            if model_index != 0 {
                self.log_entity_once(entity, &format!("unresolved CS_MODELS index {model_index}"));
            }
            return;
        };
        if !qpath.to_ascii_lowercase().ends_with(".md3") {
            self.log_entity_once(entity, &format!("non-MD3 registered model {qpath}"));
            return;
        }

        match self.present_md3(entity, &qpath) {
            Ok(mut model_draws) => draws.append(&mut model_draws),
            Err(error) => self.log_entity_once(entity, &format!("{qpath}: {error}")),
        }
    }

    fn present_md3(
        &mut self,
        entity: &PresentedEntity,
        qpath: &str,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let base = self.load_md3(qpath)?;
        let frame = entity.state.field_i32("frame").unwrap_or(0).max(0) as usize;
        let frame = frame.min(base.model.frames.len().saturating_sub(1));
        let axis = angles_to_axis(entity.angles);
        let scale_int = entity.state.field_i32("iModelScale").unwrap_or(0);
        let scale = if scale_int == 0 { 1.0 } else { scale_int as f32 / 100.0 };
        let color = entity_color(entity);
        let raster_visible = entity.state.field_i32("isPortalEnt").unwrap_or(0) != 0
            || self.entity_md3_visible(&base.model, frame, entity.origin, axis, scale);
        if !raster_visible && !self.rt_rigid_casters_enabled { return Ok(Vec::new()); }
        let asset = self.select_md3(qpath, entity.origin, frame, scale.abs(), None, false)?;
        let frame = frame.min(asset.model.frames.len().saturating_sub(1));

        let mut draws = Vec::with_capacity(asset.surfaces.len());
        for surface_asset in &asset.surfaces {
            if surface_asset.invisible_without_override {
                continue;
            }
            let Some(surface) = asset.model.surfaces.get(surface_asset.surface_index) else {
                continue;
            };
            let Some(vertices) = surface.frame_vertices(frame) else {
                continue;
            };
            if vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let (vertices, indices) = self.md3_geometry(
                &asset, surface_asset, entity.number, 0, frame, axis,
                entity.origin, scale, color, 0.0);
            draws.push(DynamicModelSurface {
                geometry_source: crate::renderer::ModelGeometrySource::Md3,
                authored_lighting: surface_asset.authored_lighting,
                material_trace: Some(Arc::clone(&surface_asset.material_trace)),
                entity_num: entity.number,
                wireframe_class: DynamicWireframeClass::Entity,
                raster_visible: true,
                vertices,
                indices: surface_asset.two_sided_indices.as_ref()
                    .map(Arc::clone).unwrap_or(indices),
                lighting_origin: Some(entity.origin),
                rt_rigid: self.rt_rigid_casters_enabled.then(|| RayTracedRigidInstance::md3(
                    Arc::clone(&asset.key),
                    Arc::clone(&asset.model),
                    surface_asset.surface_index,
                    frame,
                    axis,
                    entity.origin,
                    scale,
                )),
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture: surface_asset.texture.clone(),
                alpha_mode: if color[3] < 1.0 {
                    match surface_asset.alpha_mode {
                        DynamicModelAlphaMode::Opaque => DynamicModelAlphaMode::Blend,
                        DynamicModelAlphaMode::Mask => DynamicModelAlphaMode::MaskBlend,
                        mode => mode,
                    }
                } else {
                    surface_asset.alpha_mode
                },

                authored_glow: surface_asset.authored_glow,
            });
            self.present_md3_extra_stages(
                &mut draws, &asset, surface_asset, entity.number, frame, axis,
                entity.origin, scale, color, 0.0,
            );
        }
        if !raster_visible {
            for draw in &mut draws { draw.raster_visible = false; }
        }
        Ok(draws)
    }

    /// TaystJK/OpenJK `CG_AddViewWeapon` + first-person half of
    /// `CG_AddPlayerWeapon`. The hand MD3 is a tag parent only; JKA submits the
    /// weapon/barrel models, not the hand mesh itself. Coordinates remain in
    /// native JKA space until the ordinary dynamic-model transform so this uses
    /// the same model/material path as other MD3 entities.
    pub fn present_view_weapon(
        &mut self,
        entity_num: u16,
        weapon_num: i32,
        mut view_origin: [f32; 3],
        mut view_angles: [f32; 3],
        velocity: [f32; 3],
        bob_cycle: i32,
        current_time: i32,
        land_offset: f32,
        hand_frame: usize,
        hand_oldframe: usize,
        hand_backlerp: f32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        const WP_NONE: i32 = 0;
        const WP_STUN_BATON: i32 = 1;
        const WP_DISRUPTOR: i32 = 6;
        const WP_REPEATER: i32 = 8;
        const WP_FLECHETTE: i32 = 10;
        const WP_ROCKET_LAUNCHER: i32 = 11;
        const WP_CONCUSSION: i32 = 15;
        const WP_EMPLACED_GUN: i32 = 17;

        // EternalJK CG_AddViewWeapon forwards saber/melee through the same
        // first-person viewModel path as the other weapons.
        // CG_AddPlayerWeapon only special-cases the emplaced gun here.
        if matches!(weapon_num, WP_NONE | WP_EMPLACED_GUN) {
            return Ok(Vec::new());
        }

        // CG_RegisterWeapon walks bg_itemlist to find the IT_WEAPON entry with
        // this giTag. There is one authoritative view_model in the pinned table;
        // never synthesize a weapon filename from the world model.
        let item = (1..jka_movement::bg_item_count())
            .filter_map(jka_movement::bg_item)
            .find(|item| item.item_type == IT_WEAPON && item.tag == weapon_num)
            .ok_or_else(|| format!("weapon {weapon_num} has no IT_WEAPON bg_itemlist entry"))?;
        if item.view_model.is_empty() {
            return Ok(Vec::new());
        }

        let hand_qpath = replace_md3_suffix(&item.view_model, "_hand.md3");
        let (hand, resolved_hand_qpath) = match self.load_md3(&hand_qpath) {
            Ok(hand) => (hand, hand_qpath.as_str()),
            Err(_) => {
                // CG_RegisterWeapon falls back to the stock Bryar hand when
                // a weapon-specific *_hand.md3 is absent. Keep that exact
                // registration behavior instead of dropping the whole view gun.
                const FALLBACK_HAND: &str =
                    "models/weapons2/briar_pistol/briar_pistol_hand.md3";
                (self.load_md3(FALLBACK_HAND)?, FALLBACK_HAND)
            }
        };
        if hand.model.tags.is_empty() {
            return Err(format!("{resolved_hand_qpath}: view hand has no MD3 tags"));
        }

        // CG_CalculateWeaponPosition. cg.bobcycle is just the high bit of
        // ps->bobCycle after CG_OffsetFirstPersonView; cg.bobfracsin comes from
        // the low 7 bits. TaystJK's cg_weaponBob/cg_fallingBob defaults retain
        // the stock behavior, which is the path ported here.
        let xyspeed = (velocity[0] * velocity[0] + velocity[1] * velocity[1])
            .sqrt()
            .min(270.0);
        let bob_phase = (bob_cycle & 127) as f32 / 127.0 * std::f32::consts::PI;
        let bobfracsin = bob_phase.sin().abs();
        let scale = if ((bob_cycle & 128) >> 7) != 0 { -xyspeed } else { xyspeed };
        view_angles[2] += scale * bobfracsin * 0.005;
        view_angles[1] += scale * bobfracsin * 0.01;
        view_angles[0] += xyspeed * bobfracsin * 0.005;
        view_origin[2] += land_offset;
        let idle_scale = xyspeed + 40.0;
        let idle = (current_time as f32 * 0.001).sin();
        view_angles[2] += idle_scale * idle * 0.01;
        view_angles[1] += idle_scale * idle * 0.01;
        view_angles[0] += idle_scale * idle * 0.01;

        let hand_axis = angles_to_axis(view_angles);
        let tag_weapon = lerp_md3_tag(&hand.model, hand_oldframe, hand_frame, 1.0 - hand_backlerp, "tag_weapon")
            .ok_or_else(|| format!("{resolved_hand_qpath}: missing tag_weapon"))?;
        let (gun_origin, gun_axis) = position_on_md3_tag(view_origin, hand_axis, tag_weapon);

        let mut draws = self.present_view_md3(entity_num, &item.view_model, gun_origin, gun_axis)?;

        // CG_AddPlayerWeapon bolts these barrels directly to the hand model,
        // not to the gun model. Keep that slightly surprising original rule.
        if weapon_num == WP_STUN_BATON {
            const BATON_BARRELS: [(&str, &str); 3] = [
                ("models/weapons2/stun_baton/baton_barrel.md3", "tag_barrel"),
                ("models/weapons2/stun_baton/baton_barrel2.md3", "tag_barrel2"),
                ("models/weapons2/stun_baton/baton_barrel3.md3", "tag_barrel3"),
            ];
            for (qpath, tag_name) in BATON_BARRELS {
                if let Some(tag) = lerp_md3_tag(&hand.model, hand_oldframe, hand_frame, 1.0 - hand_backlerp, tag_name) {
                    let (origin, axis) = position_on_md3_tag(view_origin, hand_axis, tag);
                    draws.extend(self.present_view_md3(entity_num, qpath, origin, axis)?);
                }
            }
        } else if matches!(
            weapon_num,
            WP_DISRUPTOR | WP_REPEATER | WP_FLECHETTE | WP_ROCKET_LAUNCHER | WP_CONCUSSION
        ) {
            let barrel_qpath = replace_md3_suffix(&item.view_model, "_barrel.md3");
            if let Some(tag) = lerp_md3_tag(&hand.model, hand_oldframe, hand_frame, 1.0 - hand_backlerp, "tag_barrel") {
                let (origin, axis) = position_on_md3_tag(view_origin, hand_axis, tag);
                // CG_RegisterWeapon treats a missing optional barrel as handle 0;
                // do the same instead of making the whole view weapon disappear.
                if let Ok(mut barrel) = self.present_view_md3(entity_num, &barrel_qpath, origin, axis) {
                    draws.append(&mut barrel);
                }
            }
        }

        Ok(draws)
    }

    /// Submit an MD3 with OpenJK `RF_DEPTHHACK | RF_FIRST_PERSON` geometry
    /// semantics. RF_FIRST_PERSON mainly controls portal/mirror inclusion in
    /// the legacy renderer; Dinurdo's main scene has no separate mirror entity
    /// list, while the depth-range remap is preserved per vertex.
    fn present_view_md3(
        &mut self,
        entity_num: u16,
        qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let asset = self.load_md3(qpath)?;
        let mut draws = Vec::with_capacity(asset.surfaces.len());
        for surface_asset in &asset.surfaces {
            if surface_asset.invisible_without_override {
                continue;
            }
            let Some(surface) = asset.model.surfaces.get(surface_asset.surface_index) else { continue };
            let Some(vertices) = surface.frame_vertices(0) else { continue };
            if vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let (vertices, indices) = self.md3_geometry(
                &asset, surface_asset, entity_num, 0, 0, axis, origin,
                1.0, [1.0; 4], 1.0);
            draws.push(DynamicModelSurface {
                geometry_source: crate::renderer::ModelGeometrySource::Md3,
                authored_lighting: surface_asset.authored_lighting,
                material_trace: Some(Arc::clone(&surface_asset.material_trace)),
                entity_num,
                wireframe_class: DynamicWireframeClass::Entity,
                raster_visible: true,
                vertices,
                indices: surface_asset.two_sided_indices.as_ref()
                    .map(Arc::clone).unwrap_or(indices),
                lighting_origin: Some(origin),
                // View weapons are camera-space presentation and must never be
                // inserted into the world RT acceleration structure.
                rt_rigid: None,
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture: surface_asset.texture.clone(),
                alpha_mode: surface_asset.alpha_mode,

                authored_glow: surface_asset.authored_glow,
            });
            self.present_md3_extra_stages(
                &mut draws, &asset, surface_asset, entity_num, 0, axis,
                origin, 1.0, [1.0; 4], 1.0,
            );
        }
        Ok(draws)
    }

    /// Frame-zero model-space bounds shared by the MD3 browser and local model gizmo.
    pub fn md3_frame_bounds(&mut self, qpath: &str) -> Result<([f32; 3], [f32; 3]), String> {
        let asset = self.load_md3(qpath)?;
        let frame = asset.model.frames.first().ok_or("MD3 has no frames")?;
        Ok((frame.mins, frame.maxs))
    }

    /// Developer Asset Viewer path: submit an arbitrary MD3 through the same
    /// material/texture registration used by live CGame entities, without
    /// fabricating a protocol entity just to inspect a model.
    pub fn present_static_md3(
        &mut self,
        entity_num: u16,
        qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        frame: usize,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let asset = self.load_md3(qpath)?;
        if qpath.starts_with("models/fnemote/"){return self.present_emote_md3(&asset,entity_num,origin,axis,rgba,frame);}
        let frame = frame.min(asset.model.frames.len().saturating_sub(1));
        let mut draws = Vec::with_capacity(asset.surfaces.len());
        for surface_asset in &asset.surfaces {
            if surface_asset.invisible_without_override {
                continue;
            }
            let Some(surface) = asset.model.surfaces.get(surface_asset.surface_index) else {
                continue;
            };
            let Some(vertices) = surface.frame_vertices(frame) else {
                continue;
            };
            if vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let (vertices, indices) = self.md3_geometry(
                &asset, surface_asset, entity_num, 0, frame, axis, origin,
                1.0, rgba, 0.0);
            draws.push(DynamicModelSurface {
                geometry_source: crate::renderer::ModelGeometrySource::Md3,
                authored_lighting: surface_asset.authored_lighting,
                material_trace: Some(Arc::clone(&surface_asset.material_trace)),
                entity_num,
                wireframe_class: DynamicWireframeClass::Entity,
                raster_visible: true,
                vertices,
                indices: surface_asset.two_sided_indices.as_ref()
                    .map(Arc::clone).unwrap_or(indices),
                lighting_origin: Some(origin),
                rt_rigid: self.rt_rigid_casters_enabled.then(|| RayTracedRigidInstance::md3(
                    Arc::clone(&asset.key),
                    Arc::clone(&asset.model),
                    surface_asset.surface_index,
                    frame,
                    axis,
                    origin,
                    1.0,
                )),
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture: surface_asset.texture.clone(),
                alpha_mode: if rgba[3] < 1.0 {
                    match surface_asset.alpha_mode {
                        DynamicModelAlphaMode::Opaque => DynamicModelAlphaMode::Blend,
                        DynamicModelAlphaMode::Mask => DynamicModelAlphaMode::MaskBlend,
                        mode => mode,
                    }
                } else {
                    surface_asset.alpha_mode
                },

                authored_glow: surface_asset.authored_glow,
            });
            self.present_md3_extra_stages(
                &mut draws, &asset, surface_asset, entity_num, frame, axis,
                origin, 1.0, rgba, 0.0,
            );
        }
        Ok(draws)
    }

    /// Imported material ports retain all passes, their tint, UV operations,
    /// alpha test and emissive output. The regular MD3 path only has one pass.
    fn present_emote_md3(&mut self,asset:&Arc<Md3Asset>,entity_num:u16,origin:[f32;3],axis:[[f32;3];3],rgba:[f32;4],frame:usize)->Result<Vec<DynamicModelSurface>,String> {
        let mut draws=Vec::new();let seconds=self.material_clock.elapsed().as_secs_f32();
        let frame=frame.min(asset.model.frames.len().saturating_sub(1));
        for surface in &asset.surfaces {
            let shader_name=&asset.model.surfaces[surface.surface_index].shader;
            let definition=jka_assets::shader::find_shader(&self.shaders,shader_name).cloned();
            let materials=self.fx_material_stages(shader_name);
            for (stage,material) in materials.iter().enumerate(){
                let color=material.float_vertex_color(rgba);
                let (mut vertices,mut indices)=self.md3_geometry(asset,surface,entity_num,stage,frame,axis,origin,1.,color,0.);
                if !material.tc_mods.is_empty(){for vertex in Arc::make_mut(&mut vertices){for modifier in &material.tc_mods{vertex.uv=crate::fx::draw::modified_uv(vertex.uv,vertex.position,*modifier,seconds-material.shader_time_offset);}}}
                if definition.as_ref().is_some_and(|s|s.cull.eq_ignore_ascii_case("none")||s.cull.eq_ignore_ascii_case("twosided")){let original=indices.clone();for tri in original.chunks_exact(3){Arc::make_mut(&mut indices).extend([tri[2],tri[1],tri[0]]);}}
                let masked=definition.as_ref().and_then(|s|s.stages.get(stage)).is_some_and(|s|!s.alpha_test.is_empty());
                let gpu=if stage==0&&frame==0&&material.tc_mods.is_empty(){surface.gpu_vertices.as_ref().map(|gpu|crate::renderer::Ghoul2GpuSkinning {
                    mesh_key:Arc::from(format!("{}#surface{}",asset.key,surface.surface_index)),vertices:gpu.clone(),indices:indices.clone(),
                    bones:Arc::new(vec![crate::renderer::Ghoul2GpuBone{row0:[1.,0.,0.,0.],row1:[0.,1.,0.,0.],row2:[0.,0.,1.,0.]}]),axis,origin,color,uv_xform:[1.,1.,0.,0.],uv_cross:[0.;4],specular:None,bulge_height:0.,env_map:false,jiggle_offsets:Default::default(),source_material:surface.source_material.clone()
                })}else{None};
                draws.push(DynamicModelSurface {geometry_source:crate::renderer::ModelGeometrySource::Md3,authored_lighting:false,material_trace:None,entity_num,wireframe_class:DynamicWireframeClass::Entity,raster_visible:true,vertices,indices,lighting_origin:Some(origin),rt_rigid:None,rt_skinned_key:None,ghoul2_gpu:gpu,fx_gpu_sprites:None,texture:material.texture.clone(),alpha_mode:if masked{DynamicModelAlphaMode::Mask}else{blend_for_alpha(material.blend.custom_shader_alpha_mode(),color[3])},authored_glow:material.authored_glow});
            }
        }Ok(draws)
    }

    /// Retain the original MD3 source and shader decision, not just the final
    /// texture: a $whiteimage or a lost second stage must be traceable in game.
    fn md3_material_trace(
        &self,
        model_qpath: &str,
        surface: &md3::Surface,
        texture: Option<&Arc<TextureData>>,
        alpha_mode: DynamicModelAlphaMode,
    ) -> Arc<str> {
        let effective = self.shader_remaps.effective_name(&surface.shader);
        let shader = jka_assets::shader::find_shader(&self.shaders, &effective);
        let selected = shader.and_then(Shader::primary);
        let mut lines = vec![
            format!("MD3 MODEL: {model_qpath}"),
            format!("MD3 SURFACE: {}", surface.name),
            format!("EMBEDDED SHADER: {}", surface.shader),
            format!("EFFECTIVE SHADER: {effective}"),
            format!("RESOLVED ALPHA MODE: {alpha_mode:?}"),
            format!("RESOLVED TEXTURE: {}", texture.map_or("<none>", |texture| texture.label.as_str())),
        ];
        match shader {
            Some(shader) => {
                lines.push(format!("AUTHORED SHADER STAGES: {}", shader.stages.len()));
                for (index, stage) in shader.stages.iter().enumerate() {
                    let is_selected = selected.is_some_and(|primary| std::ptr::eq(primary, stage));
                    lines.push(format!(
                        "STAGE {index}{}: map={} blend={:?} alphaFunc={:?} rgbGen={:?} alphaGen={:?} tcGen={:?} tcMods={:?}",
                        if is_selected { " [MD3 RESOLVER SELECTED]" }
                        else if md3_extra_stage_supported(stage) { " [ADDITIONAL DRAW]" }
                        else { " [NOT SUPPORTED BY MD3 STAGE PATH]" },
                        stage.image, stage.blend, stage.alpha_test, stage.rgb_gen, stage.alpha_gen,
                        stage.tc_gen, stage.tc_mods,
                    ));
                }
                if shader.stages.len() > 1 {
                    lines.push("NOTICE: MD3 submits supported authored stages as separate material draws. Unsupported effects remain explicit in this trace.".into());
                }
                if selected.is_some_and(|stage| stage.image.eq_ignore_ascii_case("$whiteimage")) {
                    lines.push("NOTICE: Selected $whiteimage maps to texture=None. Check subsequent stages before changing this.".into());
                }
                if let Some(text) = &shader.definition_text {
                    lines.push("AUTHORED SHADER SCRIPT:".into());
                    lines.extend(text.lines().map(str::to_owned));
                }
            }
            None => lines.push("SHADER RESOLUTION: no authored shader; trying implicit image qpath".into()),
        }
        Arc::from(lines.join("\n"))
    }

    /// Classify a resolved *shader* based on TaystJK's blend/depth rules,
    /// never on a model name, shader name, or use of $whiteimage by itself.
    fn is_noop_shader(&self, source: &str) -> bool {
        let effective = self.shader_remaps.effective_name(source);
        jka_assets::shader::find_shader(&self.shaders, &effective)
            .is_some_and(md3_shader_is_color_depth_noop)
    }

    fn md3_surface_material(&mut self, surface: &md3::Surface, key: &str,
                            surface_index: usize, cached_indices: Arc<Vec<u32>>) -> Md3SurfaceAsset {
        let invisible_without_override = self.is_noop_shader(&surface.shader);
        let (texture, alpha_mode, authored_glow) = if invisible_without_override {
            (None, DynamicModelAlphaMode::Opaque, false)
        } else {
            self.resolve_surface_material(&surface.shader)
        };
        let effective = self.shader_remaps.effective_name(&surface.shader).into_owned();
        let definition = jka_assets::shader::find_shader(&self.shaders, &effective).cloned();
        let authored_lighting = definition.as_ref()
            .and_then(Shader::primary)
            .is_some_and(|stage| matches!(stage.rgb_gen, RgbGen::LightingDiffuse | RgbGen::LightingDiffuseEntity));
        let two_sided = definition.as_ref().is_some_and(|shader| {
            matches!(shader.cull.trim().to_ascii_lowercase().as_str(), "none" | "twosided" | "two-sided" | "disable")
        });
        let two_sided_indices = two_sided.then(|| md3_two_sided_indices(&cached_indices, true));
        let mut extra_stages = Vec::new();
        if !invisible_without_override && alpha_mode != DynamicModelAlphaMode::Glass {
            if let Some(shader) = definition.as_ref() {
                let first = shader.primary().map(|stage| stage as *const Stage);
                for (index, stage) in shader.stages.iter().enumerate() {
                    if Some(stage as *const Stage) == first || stage.image.is_empty()
                        || !md3_extra_stage_supported(stage)
                    {
                        continue;
                    }
                    // This is an authored raster stage, never an implicit model
                    // texture. Unknown special images must not become white.
                    if stage.image.starts_with('$') && !stage.image.eq_ignore_ascii_case("$whiteimage") {
                        continue;
                    }
                    let texture = if stage.image.eq_ignore_ascii_case("$whiteimage") {
                        None
                    } else {
                        Some(self.load_texture_arc_or_missing(&stage.image, stage.clamp, "MD3 STAGE"))
                    };
                    extra_stages.push(Md3ExtraStage {
                        index, texture,
                        alpha_mode: md3_stage_alpha_mode(stage),
                        authored_glow: stage.glow,
                        rgb_gen: stage.rgb_gen,
                        alpha_gen: stage.alpha_gen,
                        rgb_const: stage.color.unwrap_or([1.0; 3]),
                        alpha_const: stage.alpha.unwrap_or(1.0),
                        tc_gen: stage.tc_gen,
                        tc_mods: stage.tc_mods.clone(),
                    });
                }
            }
        }
        let material_trace = self.md3_material_trace(key, surface, texture.as_ref(), alpha_mode);
        // Imported emote props carry a normal map; light them on the GPU path.
        let normal = definition.as_ref().and_then(Shader::primary).and_then(|s| s.normal_map.clone());
        let source_material = if key.starts_with("models/fnemote/") {
            normal.and_then(|name| self.load_texture_arc(&name, false)).map(|texture| {
                let mut linear = (*texture).clone();
                linear.srgb = false;
                crate::renderer::SourceMaterialParams { normal_map: Some(Arc::new(linear)), phong_boost: 0.15, phong_exponent: 32., phong_fresnel: [0., 0.5, 1.], rimlight_boost: 0., rimlight_exponent: 1., half_lambert: false }
            })
        } else { None };
        let gpu_vertices = source_material.as_ref().map(|_| Arc::new(surface.vertices.iter().map(|v| crate::renderer::Ghoul2GpuVertex { position: v.position, normal: v.normal, uv: v.uv, bone_indices: [0; 4], weights: [1., 0., 0., 0.], weight_count: 1, jiggle_region: u32::MAX, jiggle_weight: 0., jiggle_coord: 4. }).collect()));
        Md3SurfaceAsset { surface_index, cached_indices, texture, alpha_mode,
            authored_glow, source_material, gpu_vertices, material_trace, invisible_without_override,
            extra_stages, two_sided_indices, authored_lighting }
    }

    fn refresh_md3_materials(&mut self, old: &Md3Asset) -> Md3Asset {
        let surfaces = old.surfaces.iter().map(|surface| {
            self.md3_surface_material(&old.model.surfaces[surface.surface_index],
                &old.key, surface.surface_index, Arc::clone(&surface.cached_indices))
        }).collect();
        Md3Asset { key: Arc::clone(&old.key), content_hash: old.content_hash,
            model: Arc::clone(&old.model), surfaces }
    }

    fn make_md3_asset(&mut self, key: &str, model: Md3Model, hash: u64) -> Md3Asset {
        let model = Arc::new(model);
        let surfaces = model.surfaces.iter().enumerate().map(|(surface_index, surface)| {
            self.md3_surface_material(surface, key, surface_index,
                Arc::new(flipped_md3_indices(&surface.indices)))
        }).collect();
        Md3Asset { key: Arc::from(key), content_hash: hash, model, surfaces }
    }

    /// Only the main thread reads VFS and resolves materials. This registers
    /// actual TaystJK-style _1/_2 variants before considering local fallbacks.
    fn ensure_md3_chain(&mut self, base: &Arc<Md3Asset>) {
        let key = base.key.to_string();
        if !self.md3_chains.contains_key(&key) {
            let mut chain = Md3Chain { lods: [Some(Arc::clone(base)), None, None],
                generated: [false; 3], queued: false, processed: false };
            for lod in 1..3 {
                let path = replace_md3_suffix(&key, &format!("_{lod}.md3"));
                if let Ok(Some(data)) = self.assets.read(&path, MAX_MD3_BYTES) {
                    let hash = md3_lod::content_hash(&data.bytes);
                    match md3::parse(&data.bytes) {
                        Ok(model) => {
                            if !model.frames.is_empty() && model.frames.len() == base.model.frames.len() {
                                chain.lods[lod] = Some(Arc::new(self.make_md3_asset(&path, model, hash)));
                            } else { println!("MD3 LOD: {path} incompatible frame count; skipping"); }
                        }
                        Err(error) => println!("MD3 LOD: {path}: {error}"),
                    }
                }
            }
            if self.md3_lod_debug {
                println!("MD3 LOD REGISTER: {key}: authored _1={} _2={}", chain.lods[1].is_some(), chain.lods[2].is_some());
            }
            self.md3_chains.insert(key.clone(), chain);
        }
        if !self.md3_auto_lod { return; }
        let Some(chain) = self.md3_chains.get_mut(&key) else { return; };
        if chain.queued || chain.processed { return; }
        let missing = [chain.lods[1].is_none(), chain.lods[2].is_none()];
        if !missing.iter().any(|missing| *missing) { return; }
        chain.queued = self.md3_worker.queue(md3_lod::Job {
            key, hash: base.content_hash, model: Arc::clone(&base.model), missing,
        });
    }

    fn md3_chain_count(&self, qpath: &str) -> usize {
        let key = qpath.replace('\\', "/").to_ascii_lowercase();
        let Some(chain) = self.md3_chains.get(&key) else { return 1; };
        (1..3).rev().find(|&lod| chain.lods[lod].is_some()
            && (self.md3_auto_lod || !chain.generated[lod]))
            .map_or(1, |lod| lod + 1)
    }

    fn selected_md3_asset(&self, key: &str, wanted: usize) -> Option<Arc<Md3Asset>> {
        let chain = self.md3_chains.get(key)?;
        // Fill missing slots from the nearest MORE detailed LOD (never invent
        // geometry by pointing to a less detailed source).
        (0..=wanted.min(2)).rev().find_map(|lod| {
            if chain.generated[lod] && !self.md3_auto_lod { None }
            else { chain.lods[lod].clone() }
        })
    }

    fn select_md3(
        &mut self, qpath: &str, origin: [f32; 3], frame: usize,
        scale: f32, forced_lod: Option<usize>, no_lod: bool,
    ) -> Result<Arc<Md3Asset>, String> {
        let base = self.load_md3(qpath)?;
        self.ensure_md3_chain(&base);
        let count = self.md3_chain_count(&base.key);
        let bounds = base.model.frames.get(frame).or_else(|| base.model.frames.first());
        // TaystJK RadiusFromBounds: 0.5 * length(maxs - mins).
        let radius = bounds.map(|f| {
            ((f.maxs[0] - f.mins[0]).powi(2)
              + (f.maxs[1] - f.mins[1]).powi(2)
              + (f.maxs[2] - f.mins[2]).powi(2)).sqrt() * 0.5 * scale
        }).unwrap_or(0.0);
        let wanted = forced_lod.unwrap_or_else(|| md3_lod_for_view(
            self.md3_lod_view, origin, radius, count,
            self.md3_lod_scale, self.md3_lod_bias, no_lod,
        )).min(count - 1);
        let key = base.key.to_string();
        let asset = self.selected_md3_asset(&key, wanted).unwrap_or(base);
        if self.md3_lod_debug && self.md3_lod_reported.insert((key.clone(), wanted)) {
            println!("MD3 LOD SELECT: {key} -> {wanted} ({}) tris={} generated={}",
                asset.key, asset.model.surfaces.iter().map(|s| s.indices.len()/3).sum::<usize>(),
                self.md3_chains.get(&key).is_some_and(|c| c.generated[wanted]));
        }
        Ok(asset)
    }

    /// CGame may correct camera pose after prediction (and suppress it during
    /// teleports). Use that same final view for MD3 and Ghoul2 LOD decisions.
    pub fn set_entity_pvs_enabled(&mut self, enabled: bool) {
        self.entity_pvs_enabled = enabled;
    }

    pub fn update_entity_area_mask(&mut self, mask: Option<[u8; 32]>) {
        self.entity_area_mask = mask;
    }

    fn entity_md3_visible(&self, model: &Md3Model, frame: usize,
                          origin: [f32; 3], axis: [[f32; 3]; 3], scale: f32) -> bool {
        if !self.entity_pvs_enabled { return true; }
        let Some(vis) = self.static_model_source.visibility.as_ref() else { return true };
        let Some(view) = self.md3_lod_view else { return true };
        let Some(frame) = model.frames.get(frame) else { return true };
        let (mins, maxs) = md3_world_bounds(frame.mins, frame.maxs, origin, axis, scale);
        vis.box_visible(vis.cluster_at(view.jka_position()), mins, maxs, self.entity_area_mask.as_ref())
    }

    pub fn update_md3_lod_view(&mut self, view: Option<Ghoul2PresentationView>) {
        self.md3_lod_view = view;
    }

    /// Called once per frame before any static/snapshot MD3 submissions.
    /// Polling never waits on the worker or performs disk I/O.
    pub fn set_md3_lod_settings(
        &mut self, view: Option<Ghoul2PresentationView>, scale: f32,
        bias: i32, auto: bool, debug: bool,
    ) {
        self.md3_lod_view = view;
        self.md3_lod_scale = scale.clamp(0.0, 20.0);
        self.md3_lod_bias = bias;
        self.md3_lod_debug = debug;
        if self.md3_auto_lod != auto { self.static_props = None; }
        self.md3_auto_lod = auto;
        let mut updated = false;
        while let Some(result) = self.md3_worker.pop() {
            // If the same qpath was overridden or refreshed, an obsolete result
            // cannot silently replace an authored asset.
            let Some(mut chain) = self.md3_chains.remove(&result.key) else { continue; };
            if chain.lods[0].as_ref().is_none_or(|a| a.content_hash != result.hash) {
                self.md3_chains.insert(result.key, chain);
                continue;
            }
            chain.queued = false;
            chain.processed = true;
            for (offset, generated) in result.lods.into_iter().enumerate() {
                let lod = offset + 1;
                if chain.lods[lod].is_some() { continue; }
                if let Some(model) = generated {
                    let key = format!("{}#generated_{lod}", result.key);
                    let base_hash = chain.lods[0].as_ref().map_or(0, |a| a.content_hash);
                    chain.lods[lod] = Some(Arc::new(self.make_md3_asset(&key, model, base_hash)));
                    chain.generated[lod] = true;
                    updated = true;
                }
            }
            if debug { println!("MD3 LOD WORKER: {} ({}) lod1={} lod2={}", result.key, result.source,
                chain.lods[1].is_some(), chain.lods[2].is_some()); }
            self.md3_chains.insert(result.key, chain);
        }
        if updated {
            self.static_props = None; // rebuild pretransformed static LODs once
            self.md3_geometry.clear();
            self.md3_geometry_bytes = 0;
        }
    }

    fn load_md3(&mut self, qpath: &str) -> Result<Arc<Md3Asset>, String> {
        let key = qpath.replace('\\', "/").to_ascii_lowercase();
        if let Some(model) = self.md3_models.get(&key) {
            return Ok(Arc::clone(model));
        }
        if self.failed_models.contains(&key) {
            return Err("model registration failed earlier".into());
        }

        let result = (|| {
            let (model, hash) = if let Some(model) = crate::emotes::prepared_prop_md3(&key) {
                ((*model).clone(), md3_lod::content_hash(key.as_bytes()))
            } else {
                let bytes = self
                    .assets
                    .read(&key, MAX_MD3_BYTES)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("missing {key}"))?
                    .bytes;
                (md3::parse(&bytes)?, md3_lod::content_hash(&bytes))
            };
            devprintln!(
                2,
                "ENTITY MD3: {} frames={} tags/frame={} surfaces={}",
                key,
                model.frames.len(),
                model.tags.first().map_or(0, Vec::len),
                model.surfaces.len(),
            );
            Ok::<_, String>(self.make_md3_asset(&key, model, hash))
        })();

        match result {
            Ok(model) => {
                let model = Arc::new(model);
                self.md3_models.insert(key, Arc::clone(&model));
                Ok(model)
            }
            Err(error) => {
                self.failed_models.insert(key);
                Err(error)
            }
        }
    }

    /// First renderable FX shader stage, retained for model customShader users.
    pub fn fx_material(&mut self, shader_name: &str) -> crate::fx::draw::FxMaterial {
        self.fx_material_stages(shader_name)
            .into_iter()
            .next()
            .expect("FX material resolver always returns at least one stage")
    }

    /// Resolve an FX material only when the authored shader or implicit image
    /// actually exists. This is useful for optional compatibility assets where
    /// the caller has a known stock fallback and should not draw the missing
    /// texture checker for the preferred asset.
    pub fn fx_material_stages_if_present(
        &mut self,
        shader_name: &str,
    ) -> Option<Vec<crate::fx::draw::FxMaterial>> {
        use crate::fx::draw::{FxBlend, FxMaterial};
        let key = scene::canonical_shader_name(shader_name);
        let effective = self.shader_remaps.effective_name(&key).into_owned();
        if let Some(materials) = self.fx_materials.get(&key) {
            return Some(materials.clone());
        }
        if jka_assets::shader::find_shader(&self.shaders, &effective).is_some() {
            return Some(self.fx_material_stages(shader_name));
        }
        let texture = self.load_texture_arc(&effective, false)?;
        let materials = vec![FxMaterial {
            animation: None,
            texture: Some(texture),
            blend: FxBlend::Alpha,
            rgb_vertex: true,
            alpha_vertex: true,
            rgb_const: [1.0; 3],
            alpha_const: 1.0,
            rgb_wave: None,
            alpha_wave: None,
            rgb_one_minus_vertex: false,
            alpha_one_minus_vertex: false,
            tc_mods: Vec::new(),
            shader_time_offset: self.shader_remaps.time_offset_for(&effective),
            shader_clamp_time: None,
            authored_glow: false,
        }];
        self.fx_materials.insert(key, materials.clone());
        Some(materials)
    }

    /// Every renderable stage of an FX shader. Jedi Academy effects are often
    /// multi-pass; notably `saberBlur` draws blurglow then blurcore, both with
    /// GL_ONE GL_ONE. Keeping this generic fixes other multi-stage FX too.
    pub fn fx_material_stages(&mut self, shader_name: &str) -> Vec<crate::fx::draw::FxMaterial> {
        use crate::fx::draw::{FxBlend, FxMaterial};
        use jka_assets::shader::{AlphaGen, RgbGen};
        let key = scene::canonical_shader_name(shader_name);
        // Engine-built material: untextured vertex-colour additive (enhanced
        // saber melt glow). Not an authored shader, so it never hits the loaders.
        if key == "$melt_glow" {
            return vec![FxMaterial {
                animation: None,
                texture: None,
                blend: FxBlend::Add,
                rgb_vertex: true,
                alpha_vertex: false,
                rgb_const: [1.0; 3],
                alpha_const: 1.0,
                rgb_wave: None,
                alpha_wave: None,
                rgb_one_minus_vertex: false,
                alpha_one_minus_vertex: false,
                tc_mods: Vec::new(),
                shader_time_offset: 0.0,
                shader_clamp_time: None,
                authored_glow: false,
            }];
        }
        if let Some(materials) = self.fx_materials.get(&key) {
            return materials.clone();
        }

        let effective = self.shader_remaps.effective_name(&key).into_owned();
        let shader_time_offset = self.shader_remaps.time_offset_for(&effective);
        let shader_clamp_time = jka_assets::shader::find_shader(&self.shaders, &effective).and_then(|shader| shader.clamp_time);

        let has_shader_definition = jka_assets::shader::find_shader(&self.shaders, &effective).is_some();
        let stages = jka_assets::shader::find_shader(&self.shaders, &effective).map(|shader| {
            shader.stages.iter().filter(|stage| {
                !stage.image.is_empty()
                    && (!stage.image.starts_with('$') || stage.image == "$whiteimage" || stage.image == "$lightmap")
            }).cloned().collect::<Vec<_>>()
        }).unwrap_or_default();
        let mut materials = Vec::with_capacity(stages.len().max(1));
        for stage in stages {
            // None is reserved for intentional built-ins such as $whiteimage.
            // Real image qpaths that fail lookup use the engine-wide missing
            // material checker instead of silently sampling white.
            let texture = if stage.image.eq_ignore_ascii_case("$whiteimage") || stage.image.eq_ignore_ascii_case("$lightmap") {
                None
            } else {
                Some(self.load_fx_texture_arc(&stage.image, stage.clamp, FxBlend::from_blend_func(&stage.blend)))
            };
            // Cache every authored animMap frame once. The actual frame is
            // chosen from FxView.seconds during draw preparation, so the
            // cached material remains stable across map/FX animation frames.
            let animation = stage.animation.as_ref().map(|animation| {
                let frames = animation.frames.iter().map(|path| {
                    self.load_fx_texture_arc(path, stage.clamp, FxBlend::from_blend_func(&stage.blend))
                }).collect();
                crate::fx::draw::FxTextureAnimation {
                    fps: animation.fps,
                    one_shot: animation.one_shot,
                    frames,
                }
            });
            materials.push(FxMaterial {
                animation,
                texture,
                blend: FxBlend::from_blend_func(&stage.blend),
                rgb_vertex: stage.rgb_gen.uses_vertex_color() || stage.entity_rgb,
                alpha_vertex: matches!(stage.alpha_gen, AlphaGen::Vertex | AlphaGen::OneMinusVertex)
                    || (stage.alpha_gen == AlphaGen::Identity && stage.rgb_gen == RgbGen::Vertex),
                rgb_const: if stage.rgb_gen == RgbGen::Const { stage.color.unwrap_or([1.0; 3]) } else { [1.0; 3] },
                alpha_const: stage.alpha.unwrap_or(1.0),
                rgb_wave: match stage.rgb_gen { RgbGen::Wave(wave) => Some(wave), _ => None },
                alpha_wave: match stage.alpha_gen { AlphaGen::Wave(wave) => Some(wave), _ => None },
                rgb_one_minus_vertex: stage.rgb_gen == RgbGen::OneMinusVertex,
                alpha_one_minus_vertex: stage.alpha_gen == AlphaGen::OneMinusVertex,
                tc_mods: stage.tc_mods.clone(),
                shader_time_offset,
                shader_clamp_time,
                authored_glow: stage.glow,
            });
        }
        if materials.is_empty() {
            if has_shader_definition {
                // An authored shader that produced no renderable stage in this
                // path is genuinely unresolved here: keep the engine-wide
                // magenta/checker convention instead of silently inventing a
                // material.
                rverbose!(
                    1,
                    "FX MATERIAL WARNING: {shader_name}: shader has no renderable image stage; using shared missing texture placeholder"
                );
                materials.push(FxMaterial {
                    animation: None,
                    texture: Some(Arc::clone(&self.missing_texture)),
                    blend: FxBlend::Opaque,
                    rgb_vertex: true,
                    alpha_vertex: true,
                    rgb_const: [1.0; 3],
                    alpha_const: 1.0,
                    rgb_wave: None,
                    alpha_wave: None,
                    rgb_one_minus_vertex: false,
                    alpha_one_minus_vertex: false,
                    tc_mods: Vec::new(),
                    shader_time_offset,
                    shader_clamp_time,
                    authored_glow: false,
                });
            } else {
                // Faithful RE_RegisterShader behavior: an explicit .shader
                // definition is optional. If an image with the requested qpath
                // exists, OpenJK builds an implicit LIGHTMAP_2D shader for it
                // (CGEN_VERTEX/AGEN_VERTEX, SRC_ALPHA/ONE_MINUS_SRC_ALPHA).
                // This is exactly how stock image-only FX such as rivet marks
                // remain transparent/dark instead of becoming opaque gray.
                match self.load_texture_arc(&effective, false) {
                    Some(texture) => materials.push(FxMaterial {
                        animation: None,
                        texture: Some(texture),
                        blend: FxBlend::Alpha,
                        rgb_vertex: true,
                        alpha_vertex: true,
                        rgb_const: [1.0; 3],
                        alpha_const: 1.0,
                        rgb_wave: None,
                        alpha_wave: None,
                        rgb_one_minus_vertex: false,
                        alpha_one_minus_vertex: false,
                        tc_mods: Vec::new(),
                        shader_time_offset,
                        shader_clamp_time,
                        authored_glow: false,
                    }),
                    None => {
                        if let Some(texture) = self.rgb_saber_fallback_texture(&effective) {
                            materials.push(FxMaterial {
                                animation: None,
                                texture: Some(texture),
                                blend: FxBlend::Add,
                                rgb_vertex: true,
                                alpha_vertex: false,
                                rgb_const: [1.0; 3],
                                alpha_const: 1.0,
                                rgb_wave: None,
                                alpha_wave: None,
                                rgb_one_minus_vertex: false,
                                alpha_one_minus_vertex: false,
                                tc_mods: Vec::new(),
                                shader_time_offset,
                                shader_clamp_time,
                                authored_glow: false,
                            });
                        } else {
                            if let Some(warning) = self.textures.warnings.last() {
                                rverbose!(1, "FX TEXTURE WARNING: {warning}");
                            }
                            rverbose!(
                                1,
                                "FX MATERIAL WARNING: {shader_name}: no shader definition or implicit image; using shared missing texture placeholder"
                            );
                            materials.push(FxMaterial {
                                animation: None,
                                texture: Some(Arc::clone(&self.missing_texture)),
                                blend: FxBlend::Opaque,
                                rgb_vertex: true,
                                alpha_vertex: true,
                                rgb_const: [1.0; 3],
                                alpha_const: 1.0,
                                rgb_wave: None,
                                alpha_wave: None,
                                rgb_one_minus_vertex: false,
                                alpha_one_minus_vertex: false,
                                tc_mods: Vec::new(),
                                shader_time_offset,
                                shader_clamp_time,
                                authored_glow: false,
                            });
                        }
                    }
                }
            }
        }
        self.fx_materials.insert(key, materials.clone());
        materials
    }

    fn load_texture_arc(&mut self, image: &str, clamp: bool) -> Option<Arc<TextureData>> {
        let index = self.textures.load(&mut self.assets, image, clamp)?;
        self.texture_arcs
            .entry(index)
            .or_insert_with(|| Arc::new(self.textures.images[index].clone()));
        Some(Arc::clone(&self.texture_arcs[&index]))
    }

    /// jaPRO's RGB saber glow/core are white images tinted per draw by vertex
    /// colour. Without its assets, derive equivalents from the stock saber images
    /// by keeping each texel's brightest channel as a grey level, so the tint
    /// alone decides the hue.
    fn rgb_saber_fallback_texture(&mut self, key: &str) -> Option<Arc<TextureData>> {
        let source = match key {
            "gfx/effects/sabers/rgbglow1" => "gfx/effects/sabers/yellow_glow2",
            "gfx/effects/sabers/rgbcore1" => "gfx/effects/sabers/blue_line",
            _ => return None,
        };
        let base = self.load_texture_arc(source, false)?;
        let mut texture = (*base).clone();
        texture.label = key.to_owned();
        texture.rgba16f = None;
        for texel in texture.rgba.chunks_exact_mut(4) {
            let grey = texel[0].max(texel[1]).max(texel[2]);
            texel[..3].fill(grey);
        }
        Some(Arc::new(texture))
    }

    fn load_fx_texture_arc(&mut self, image: &str, clamp: bool, blend: crate::fx::draw::FxBlend) -> Arc<TextureData> {
        let texture = self.load_texture_arc_or_missing(image, clamp, "FX");
        if blend == crate::fx::draw::FxBlend::Modulate2x && texture.srgb {
            // Doubled-modulation images encode multipliers. Their 128-grey
            // borders must remain neutral on every animMap frame. Keep the
            // ordinary color cache intact for other materials sharing the image.
            let mut multiplier = (*texture).clone();
            multiplier.srgb = false;
            Arc::new(multiplier)
        } else {
            texture
        }
    }

    fn load_texture_arc_or_missing(
        &mut self,
        image: &str,
        clamp: bool,
        log_prefix: &str,
    ) -> Arc<TextureData> {
        match self.load_texture_arc(image, clamp) {
            Some(texture) => texture,
            None => {
                if let Some(warning) = self.textures.warnings.last() {
                    rverbose!(1, "{log_prefix} TEXTURE WARNING: {warning}");
                }
                Arc::clone(&self.missing_texture)
            }
        }
    }

    fn resolve_surface_material(
        &mut self,
        shader_name: &str,
    ) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode, bool) {
        let effective = self.shader_remaps.effective_name(shader_name).into_owned();
        let shader = jka_assets::shader::find_shader(&self.shaders, &effective);
        let glass = shader.is_some_and(|shader| shader.is_glass() && crate::materials::glass_promotion_enabled());
        let (image_name, clamp, alpha_mode, authored_glow) = shader
            .and_then(Shader::primary)
            .map(|stage| {
                let alpha_mode = if glass { DynamicModelAlphaMode::Glass } else { md3_stage_alpha_mode(stage) };
                (stage.image.clone(), stage.clamp, alpha_mode, stage.glow)
            })
            .unwrap_or((
                effective,
                false,
                DynamicModelAlphaMode::Opaque,
                false,
            ));

        if image_name.eq_ignore_ascii_case("$whiteimage") {
            return (None, alpha_mode, authored_glow);
        }
        let texture = Some(self.load_texture_arc_or_missing(&image_name, clamp, "ENTITY"));
        (texture, alpha_mode, authored_glow)
    }

    fn log_entity_once(&mut self, entity: &PresentedEntity, reason: &str) {
        let key = format!("{}:{}:{reason}", entity.number, entity.entity_type);
        if self.logged_unsupported.insert(key) {
            devprintln!(
                2,
                "ENTITY PRESENTATION TODO: entity={} type={} modelindex={} {reason}",
                entity.number,
                entity.entity_type,
                entity.state.field_i32("modelindex").unwrap_or(0),
            );
        }
    }
}

/// Only submit stage forms this MD3 path actually implements. Complex wave,
/// animation and scroll stages still require the world shader evaluator; don't
/// silently render incorrect approximation passes on unrelated models.
fn md3_extra_stage_supported(stage: &Stage) -> bool {
    matches!(stage.tc_gen, TcGen::Base | TcGen::Environment | TcGen::Vector(..))
        && stage.tc_mods.iter().all(|op| matches!(op, TcMod::Scale(..) | TcMod::Transform(..)))
        && matches!(stage.rgb_gen,
            RgbGen::Identity | RgbGen::IdentityLighting | RgbGen::LightingDiffuse
            | RgbGen::LightingDiffuseEntity | RgbGen::Vertex | RgbGen::ExactVertex
            | RgbGen::OneMinusVertex | RgbGen::Const)
        && matches!(stage.alpha_gen,
            AlphaGen::Identity | AlphaGen::Const | AlphaGen::Vertex | AlphaGen::OneMinusVertex)
        && stage.animation.is_none()
        && !stage.depth_equal
        && !stage.video
        && stage.unsupported.is_empty()
}

/// Match authored stage lighting rather than inferring it from blend mode.
/// In particular `rgbGen lightingDiffuse` + alpha blend is **lit**, whereas
/// `rgbGen identity` + the same blend is unlit (TaystJK ComputeColors).
fn md3_stage_alpha_mode(stage: &Stage) -> DynamicModelAlphaMode {
    let mut mode = if !stage.alpha_test.trim().is_empty() {
        DynamicModelAlphaMode::Mask
    } else {
        crate::fx::draw::FxBlend::from_blend_func(&stage.blend)
            .custom_shader_alpha_mode()
    };
    if matches!(stage.rgb_gen, RgbGen::LightingDiffuse | RgbGen::LightingDiffuseEntity)
        && matches!(mode, DynamicModelAlphaMode::BlendUnlit)
    {
        mode = DynamicModelAlphaMode::Blend;
    }
    if stage.depth_write {
        mode = match mode {
            DynamicModelAlphaMode::Blend => DynamicModelAlphaMode::BlendDepthWrite,
            DynamicModelAlphaMode::BlendUnlit => DynamicModelAlphaMode::BlendUnlitDepthWrite,
            other => other,
        };
    }
    mode
}

fn md3_stage_color(stage: &Md3ExtraStage, entity: [f32; 4]) -> [f32; 4] {
    let rgb = match stage.rgb_gen {
        RgbGen::Vertex | RgbGen::ExactVertex | RgbGen::LightingDiffuseEntity => [entity[0], entity[1], entity[2]],
        RgbGen::OneMinusVertex => [1.0 - entity[0], 1.0 - entity[1], 1.0 - entity[2]],
        RgbGen::Const => stage.rgb_const,
        _ => [1.0; 3],
    };
    let alpha = match stage.alpha_gen {
        AlphaGen::Const => stage.alpha_const,
        AlphaGen::Vertex => entity[3],
        AlphaGen::OneMinusVertex => 1.0 - entity[3],
        _ => 1.0,
    };
    [rgb[0], rgb[1], rgb[2], alpha]
}

/// TaystJK tr_shade_calc.cpp RB_CalcEnvironmentTexCoords, in JKA axes;
/// world and view positions are stored in the renderer's Y-up axes.
fn md3_stage_uv(
    vertex: &DynamicModelVertex, stage: &Md3ExtraStage, camera: Option<[f32; 3]>,
) -> [f32; 2] {
    let mut uv = match stage.tc_gen {
        TcGen::Environment => {
            let Some(camera) = camera else { return vertex.uv; };
            let view = scene::jka_position(std::array::from_fn(|i| camera[i] - vertex.position[i]));
            let view_len = (view.iter().map(|x| x * x).sum::<f32>()).sqrt().max(1.0e-6);
            let view = view.map(|x| x / view_len);
            let normal = scene::jka_position(vertex.normal);
            let dot = (0..3).map(|i| normal[i] * view[i]).sum::<f32>();
            let reflected: [f32; 3] = std::array::from_fn(|i| normal[i] * 2.0 * dot - view[i]);
            [0.5 + reflected[1] * 0.5, 0.5 - reflected[2] * 0.5]
        }
        TcGen::Vector(s, t) => {
            let pos = scene::jka_position(vertex.position);
            [
                (0..3).map(|i| s[i] * pos[i]).sum(),
                (0..3).map(|i| t[i] * pos[i]).sum(),
            ]
        }
        _ => vertex.uv,
    };
    for op in &stage.tc_mods {
        match *op {
            TcMod::Scale(s, t) => { uv[0] *= s; uv[1] *= t; }
            TcMod::Transform([a,b,c,d,e,f]) => {
                uv = [uv[0] * a + uv[1] * b + e, uv[0] * c + uv[1] * d + f];
            }
            _ => {} // time-dependent operations require shader time; not faked
        }
    }
    uv
}

/// Registered once per MD3 surface and remap, never built per frame. A
/// dedicated no-cull pipeline would save the extra triangles; until that is
/// available, back-face culling of mirrored winding reproduces both faces.
fn md3_two_sided_indices(source: &Arc<Vec<u32>>, two_sided: bool) -> Arc<Vec<u32>> {
    if !two_sided { return Arc::clone(source); }
    let mut indices = Vec::with_capacity(source.len() * 2);
    indices.extend_from_slice(source);
    for triangle in source.chunks_exact(3) {
        indices.extend_from_slice(&[triangle[0], triangle[2], triangle[1]]);
    }
    Arc::new(indices)
}

/// A no-op blend means `source * GL_ZERO + destination * GL_ONE`.
/// TaystJK tr_shader.cpp clears the depth mask for blended stages unless
/// `depthWrite` is authored. Only suppress a shader if *every* stage
/// preserves destination and none writes depth or has a surface sprite.
/// The shader lookup must already have selected the effective remap target.
fn md3_shader_is_color_depth_noop(shader: &Shader) -> bool {
    !shader.stages.is_empty()
        && !shader.portal
        && !shader.sky
        && !shader.fog
        && shader.unsupported.is_empty()
        && shader.stages.iter().all(|stage| {
            let mut factors = stage.blend.split_ascii_whitespace();
            matches!(factors.next(), Some(src) if src.eq_ignore_ascii_case("gl_zero"))
                && matches!(factors.next(), Some(dst) if dst.eq_ignore_ascii_case("gl_one"))
                && factors.next().is_none()
                && !stage.depth_write
                && stage.surface_sprite.is_none()
                && stage.unsupported.is_empty()
        })
}

fn entity_color(entity: &PresentedEntity) -> [f32; 4] {
    let mut rgba = [0i32; 4];
    let mut any = false;
    for (index, channel) in rgba.iter_mut().enumerate() {
        *channel = entity
            .state
            .field_i32(&format!("customRGBA[{index}]"))
            .unwrap_or(0);
        any |= *channel != 0;
    }
    if !any {
        return [1.0; 4];
    }
    rgba.map(|channel| channel.clamp(0, 255) as f32 / 255.0)
}

fn replace_md3_suffix(qpath: &str, suffix: &str) -> String {
    let normalized = qpath.replace('\\', "/");
    let stem = normalized
        .strip_suffix(".md3")
        .or_else(|| normalized.strip_suffix(".MD3"))
        .unwrap_or(&normalized);
    format!("{stem}{suffix}")
}

/// Renderer `R_LerpTag`: linearly interpolate the same named tag between the
/// two MD3 frames, normalize the three basis rows, and return the lerped tag.
fn lerp_md3_tag(
    model: &Md3Model,
    oldframe: usize,
    frame: usize,
    front_lerp: f32,
    name: &str,
) -> Option<md3::Tag> {
    let oldframe = oldframe.min(model.tags.len().saturating_sub(1));
    let frame = frame.min(model.tags.len().saturating_sub(1));
    let old = model.tags.get(oldframe)?.iter().find(|tag| tag.name.eq_ignore_ascii_case(name))?;
    let new = model.tags.get(frame)?.iter().find(|tag| tag.name.eq_ignore_ascii_case(name))?;
    let front = front_lerp.clamp(0.0, 1.0);
    let back = 1.0 - front;
    let mut axis = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            axis[row][col] = old.axis[row][col] * back + new.axis[row][col] * front;
        }
        normalize3_in_place(&mut axis[row]);
    }
    Some(md3::Tag {
        name: name.to_owned(),
        origin: [
            old.origin[0] * back + new.origin[0] * front,
            old.origin[1] * back + new.origin[1] * front,
            old.origin[2] * back + new.origin[2] * front,
        ],
        axis,
    })
}

fn normalize3_in_place(v: &mut [f32; 3]) {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 {
        v[0] /= len;
        v[1] /= len;
        v[2] /= len;
    }
}

/// `CG_PositionEntityOnTag`. Axis arrays use the same basis-row convention as
/// the MD3 loader and `angles_to_axis`.
fn position_on_md3_tag(
    parent_origin: [f32; 3],
    parent_axis: [[f32; 3]; 3],
    tag: md3::Tag,
) -> ([f32; 3], [[f32; 3]; 3]) {
    let mut origin = parent_origin;
    for basis in 0..3 {
        for component in 0..3 {
            origin[component] += tag.origin[basis] * parent_axis[basis][component];
        }
    }
    let mut axis = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            axis[row][col] = tag.axis[row][0] * parent_axis[0][col]
                + tag.axis[row][1] * parent_axis[1][col]
                + tag.axis[row][2] * parent_axis[2][col];
        }
    }
    (origin, axis)
}

fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
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

/// Exact transform portion of TaystJK `CG_DrawPlayerSphere`: the model origin
/// is raised 9 units, then the half-shield is rotated to face the current
/// refdef view origin. `testboom.md3` is already authored at the correct size,
/// so the stock invulnerability path uses scale 1.0.
fn invulnerability_sphere_transform(
    player_origin: [f32; 3],
    view_origin: [f32; 3],
) -> Option<([f32; 3], [[f32; 3]; 3])> {
    let origin = [player_origin[0], player_origin[1], player_origin[2] + 9.0];
    let to_sphere = [
        origin[0] - view_origin[0],
        origin[1] - view_origin[1],
        origin[2] - view_origin[2],
    ];
    let length_sq = to_sphere[0] * to_sphere[0]
        + to_sphere[1] * to_sphere[1]
        + to_sphere[2] * to_sphere[2];
    if length_sq <= 0.01 {
        return None;
    }
    let mut angles = openjk_vectoangles(to_sphere);
    angles[2] += 180.0; // ROLL
    angles[0] += 180.0; // PITCH
    Some((origin, angles_to_axis(angles)))
}

/// TaystJK rd-vanilla/tr_mesh.cpp R_ComputeLOD: project bounding radius,
/// scale 0..20, truncate projected level, clamp, apply r_lodbias, clamp again.
fn md3_lod_for_view(
    view: Option<Ghoul2PresentationView>, origin: [f32; 3], radius: f32,
    num_lods: usize, scale: f32, bias: i32, no_lod: bool,
) -> usize {
    if num_lods < 2 || no_lod { return 0; }
    let projected = view.map_or(0.0, |view| view.projected_radius(origin, radius));
    let flod = if projected != 0.0 { 1.0 - projected * scale.clamp(0.0, 20.0) } else { 0.0 };
    let limit = num_lods.saturating_sub(1) as isize;
    let lod = ((flod * num_lods as f32) as isize).clamp(0, limit);
    (lod + bias as isize).clamp(0, limit) as usize
}

/// Draw order for all static prop surfaces. Opaque and alpha-tested stages
/// write depth and do not blend, so props may be interleaved freely as long as
/// a coincident later stage of the same surface stays behind its earlier one:
/// sort by (mode, stage layer, glow, texture). Every other mode keeps the
/// original per-prop submission order (the sort is stable).
fn static_draw_order(props: &[StaticProp]) -> Vec<StaticDrawRef> {
    let mut entries = Vec::new();
    for (prop_index, prop) in props.iter().enumerate() {
        let levels = std::iter::once(Some(&prop.surfaces))
            .chain(prop.lods.iter().map(Option::as_ref));
        for (lod, surfaces) in levels.enumerate() {
            let Some(surfaces) = surfaces else { continue };
            for (surface_index, surface) in surfaces.iter().enumerate() {
                let class = match surface.alpha_mode {
                    DynamicModelAlphaMode::Opaque => 0u8,
                    DynamicModelAlphaMode::Mask => 1,
                    _ => 2,
                };
                let key = if class == 2 { (2, 0, false, 0) } else {
                    (
                        class,
                        prop.layers[lod].get(surface_index).copied().unwrap_or(0),
                        surface.authored_glow,
                        surface.texture.as_ref().map_or(0, |texture| Arc::as_ptr(texture) as usize),
                    )
                };
                entries.push((key, StaticDrawRef {
                    prop: prop_index as u32,
                    lod: lod as u8,
                    surface: surface_index as u32,
                }));
            }
        }
    }
    entries.sort_by_key(|(key, _)| *key);
    entries.into_iter().map(|(_, entry)| entry).collect()
}

fn flipped_md3_indices(source: &[u32]) -> Vec<u32> {
    let mut indices = source.to_vec();
    for triangle in indices.chunks_exact_mut(3) { triangle.swap(1, 2); }
    indices
}

fn md3_geometry_input(
    axis: [[f32; 3]; 3], origin: [f32; 3], scale: f32,
    color: [f32; 4], depth_hack: f32,
) -> [u32; 18] {
    let mut input = [0; 18];
    for (i, value) in axis.into_iter().flatten().chain(origin).chain([scale])
        .chain(color).chain([depth_hack]).enumerate()
    {
        input[i] = value.to_bits();
    }
    input
}

fn transform_model_point(
    point: [f32; 3],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    scale: f32,
) -> [f32; 3] {
    let point = [point[0] * scale, point[1] * scale, point[2] * scale];
    let world = [
        origin[0] + axis[0][0] * point[0] + axis[1][0] * point[1] + axis[2][0] * point[2],
        origin[1] + axis[0][1] * point[0] + axis[1][1] * point[1] + axis[2][1] * point[2],
        origin[2] + axis[0][2] * point[0] + axis[1][2] * point[1] + axis[2][2] * point[2],
    ];
    scene::render_position(world)
}

fn transform_model_normal(normal: [f32; 3], axis: [[f32; 3]; 3]) -> [f32; 3] {
    let jka = [
        axis[0][0] * normal[0] + axis[1][0] * normal[1] + axis[2][0] * normal[2],
        axis[0][1] * normal[0] + axis[1][1] * normal[1] + axis[2][1] * normal[2],
        axis[0][2] * normal[0] + axis[1][2] * normal[1] + axis[2][2] * normal[2],
    ];
    let render = scene::render_position(jka);
    let length = (render[0] * render[0] + render[1] * render[1] + render[2] * render[2]).sqrt();
    if length > 0.0 {
        [render[0] / length, render[1] / length, render[2] / length]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Niagara camera-facing mesh basis: X towards the eye, Z up when locked
/// (else the world up made orthogonal), each axis keeping its length (scale).
fn facing_axis(axis: [[f32; 3]; 3], origin: [f32; 3], eye: [f32; 3], locked: bool) -> [[f32; 3]; 3] {
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let norm = |v: [f32; 3]| { let l = len(v).max(1e-6); [v[0] / l, v[1] / l, v[2] / l] };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let mut x = [eye[0] - origin[0], eye[1] - origin[1], if locked { 0.0 } else { eye[2] - origin[2] }];
    if len(x) < 1e-3 {
        return axis;
    }
    x = norm(x);
    let up = if x[2].abs() > 0.99 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] };
    let y = norm(cross(up, x));
    let z = cross(x, y);
    let s = [len(axis[0]), len(axis[1]), len(axis[2])];
    [x.map(|c| c * s[0]), y.map(|c| c * s[1]), z.map(|c| c * s[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires DJK_FN_ASSET_BASE with a converted native prop PK3"]
    fn imported_static_props_render_with_native_materials() {
        let root=std::path::PathBuf::from(std::env::var("DJK_FN_ASSET_BASE").unwrap());
        let mut assets=AssetSearchPath::open(&root).unwrap();
        let data=assets.read("models/fnemote/index.json",16*1024*1024).unwrap().unwrap();
        let index:serde_json::Value=serde_json::from_slice(&data.bytes).unwrap();
        let mut presenter=EntityPresenter::new(assets,false).unwrap();
        let mut count=0;
        for model in index["models"].as_array().unwrap() {
            let path=model["model"].as_str().unwrap();
            let draws=presenter.present_static_md3(1023,path,[0.;3],[[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]],[1.;4],0).unwrap();
            assert!(!draws.is_empty(),"no geometry for {path}");
            for surface in draws {
                assert!(!surface.indices.is_empty());
                assert!(surface.vertices.iter().flat_map(|v|v.position).all(f32::is_finite));
                let texture=surface.texture.as_ref().expect("missing material texture");
                assert!(texture.source.is_some(),"failed to decode authored or neutral texture for {path}");
            }
            count+=1;
        }
        assert!(count>0);
        println!("Rendered {count} imported prop models through the native MD3 presenter");
    }

    fn synthetic_md3_presenter() -> (crate::cvar_compatibility::Fixture, EntityPresenter) {
        let fixture = crate::cvar_compatibility::Fixture::new();
        let mut presenter = EntityPresenter::new(AssetSearchPath::open(&fixture.0).unwrap(), false).unwrap();
        let vertex = md3::Vertex { position: [1.0, 2.0, 3.0], normal: [0.0, 0.0, 1.0], uv: [0.2, 0.8] };
        let mut next = vertex;
        next.position[2] = 9.0;
        let frame = md3::Frame {
            mins: [0.0; 3], maxs: [10.0; 3], local_origin: [0.0; 3], radius: 10.0, name: String::new(),
        };
        let surface = md3::Surface {
            name: "test".into(), shader: "test".into(), vertices: vec![vertex; 3],
            frames: vec![vec![vertex; 3], vec![next; 3]], indices: vec![0, 1, 2],
        };
        presenter.md3_models.insert("synthetic.md3".into(), Arc::new(Md3Asset {
            key: "synthetic.md3".into(),
            content_hash: 0,
            model: Arc::new(Md3Model { frames: vec![frame.clone(), frame], tags: vec![], surfaces: vec![surface] }),
            surfaces: vec![Md3SurfaceAsset {
                surface_index: 0, cached_indices: Arc::new(vec![0, 2, 1]), texture: None,
                alpha_mode: DynamicModelAlphaMode::Opaque, authored_glow: true,
                source_material:None,gpu_vertices:None,
                material_trace: Arc::from("synthetic"), invisible_without_override: false,
                extra_stages: Vec::new(), two_sided_indices: None, authored_lighting: false,
            }],
        }));
        (fixture, presenter)
    }

    fn two_room_visibility() -> jka_assets::bsp::Visibility {
        let ints = |v: &[i32]| v.iter().flat_map(|n| n.to_le_bytes()).collect::<Vec<u8>>();
        let mut lumps = vec![Vec::new(); 18];
        lumps[0] = b"{\n\"classname\" \"worldspawn\"\n}\n".to_vec();
        lumps[2] = [1.0f32, 0.0, 0.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        lumps[3] = ints(&[0, -1, -2, 0, 0, 0, 0, 0, 0]);
        lumps[4] = ints(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                         1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        lumps[7] = vec![0; 40];
        lumps[16] = ints(&[2, 1]);
        lumps[16].extend([1, 2]);
        let mut bytes = vec![0; 152];
        bytes[..4].copy_from_slice(b"RBSP");
        bytes[4..8].copy_from_slice(&1i32.to_le_bytes());
        for (i, lump) in lumps.iter().enumerate() {
            let start = bytes.len() as i32;
            bytes[8+i*8..12+i*8].copy_from_slice(&start.to_le_bytes());
            bytes[12+i*8..16+i*8].copy_from_slice(&(lump.len() as i32).to_le_bytes());
            bytes.extend(lump);
        }
        let bsp = jka_assets::bsp::Bsp::parse(&bytes).unwrap();
        let source = bsp.static_models();
        assert!(source.placements.is_empty());
        source.visibility.expect("entity MD3s need PVS even without misc_model_static")
    }

    #[test]
    fn entity_md3_pvs_rejects_hidden_rooms_preserves_rt_and_obeys_off() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        presenter.set_static_models(Arc::new(StaticModels {
            placements: Vec::new(), visibility: Some(two_room_visibility()),
        }));
        presenter.update_md3_lod_view(Some(Ghoul2PresentationView::new(
            scene::render_position([100.0, 0.0, 0.0]), [-1.0, 0.0, 0.0], 1.2, 1.0, 0.0,
        )));
        let mut entity = PresentedEntity {
            number: 1, entity_type: super::super::ET_GENERAL,
            origin: [-100.0, 0.0, 0.0], angles: [0.0; 3],
            state: jka_protocol::gamestate::EntityState {
                number: 1, fields: [0; jka_protocol::gamestate::ENTITY_FIELDS.len()],
            },
        };
        assert!(presenter.present_md3(&entity, "synthetic.md3").unwrap().is_empty());
        presenter.rt_rigid_casters_enabled = true;
        let hidden = presenter.present_md3(&entity, "synthetic.md3").unwrap();
        assert!(!hidden.is_empty());
        assert!(hidden.iter().all(|draw| !draw.raster_visible && draw.rt_rigid.is_some()));
        presenter.set_entity_pvs_enabled(false);
        assert!(presenter.present_md3(&entity, "synthetic.md3").unwrap().iter().all(|d| d.raster_visible));
        presenter.set_entity_pvs_enabled(true);
        presenter.rt_rigid_casters_enabled = false;
        entity.origin[0] = -5.0; // origin hidden, but bounds cross into visible room
        assert!(!presenter.present_md3(&entity, "synthetic.md3").unwrap().is_empty());
        entity.origin[0] = 100.0;
        let mut mask = [0; 32]; mask[0] = 1;
        presenter.update_entity_area_mask(Some(mask));
        assert!(presenter.present_md3(&entity, "synthetic.md3").unwrap().is_empty());
        presenter.update_md3_lod_view(None);
        assert!(!presenter.present_md3(&entity, "synthetic.md3").unwrap().is_empty());
    }

    #[test]
    fn entity_md3_bounds_include_rotation_and_negative_scale() {
        let (mins, maxs) = md3_world_bounds([0.0; 3], [10.0, 20.0, 30.0],
            [100.0, 200.0, 300.0], [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], -2.0);
        assert_eq!(mins, [100.0, 180.0, 240.0]);
        assert_eq!(maxs, [140.0, 200.0, 300.0]);
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s"]
    fn t2_rancor_model_pvs_at_reported_viewpos() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let file = assets.read("maps/t2_rancor.bsp", 128 << 20).unwrap().unwrap();
        let bsp = jka_assets::bsp::Bsp::parse(&file.bytes).unwrap();
        let vis = bsp.visibility.as_ref().unwrap();
        let cluster = vis.cluster_at([-3476.0, 3503.0, 1170.0]);
        assert!(cluster.is_some());
        let (mut visible, mut rejected) = (0, 0);
        for model in bsp.models.iter().skip(1) {
            if vis.box_visible(cluster, model.mins, model.maxs, None) { visible += 1; }
            else { rejected += 1; }
        }
        println!("t2_rancor view cluster {cluster:?}: compiled inline bounds visible={visible}, PVS-rejected={rejected}; no area mask/frustum/Hi-Z");
        assert!(visible > 0 && rejected > 0);
    }

    fn shader_def(name: &str, stage: &str) -> String {
        format!("{name}\n{{\n{{\n{stage}\n}}\n}}\n")
    }

    fn test_static_props(models: usize) -> Arc<StaticModels> {
        Arc::new(StaticModels {
            placements: (0..models).map(|i| jka_assets::bsp::StaticModel {
                model: "synthetic.md3".into(),
                origin: [100.0 + 32.0 * i as f32, 20.0, 10.0],
                angles: [0.0, i as f32 * 45.0, 0.0],
                scale: [2.0, -3.0, 0.5],
                zoffset: 0.0, cull_cluster: None,
            }).collect(), visibility: None,
        })
    }

    #[test]
    fn static_bvh_preserves_every_original_frustum_and_distance_candidate() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        presenter.set_static_models(test_static_props(256));
        let props = presenter.build_static_props();
        let tree = StaticVisibilityTree::build(&props);
        let camera_origin = scene::render_position([0.0, 0.0, 0.0]);
        let forward = scene::render_position([1.0, 0.0, 0.0]);
        for max_dist in [0.0, 256.0, 2048.0] {
            let view = Ghoul2PresentationView::new(camera_origin, forward, 1.2, 16.0/9.0, max_dist);
            let mut candidates = Vec::new();
            let (_, pruned) = tree.candidates(view, None, None, &mut candidates);
            let set: HashSet<usize> = candidates.into_iter().collect();
            for (index, prop) in props.iter().enumerate() {
                let baseline_visible = !(prop.radius > 0.0
                    && view.sphere_outside(prop.cull_origin, prop.radius))
                    && !(max_dist > 0.0
                        && view.distance_to(prop.origin) - prop.radius > max_dist);
                if baseline_visible {
                    assert!(set.contains(&index), "BVH dropped originally visible static MD3 {index}");
                }
            }
            if max_dist == 256.0 { assert!(pruned > 0); }
        }
    }

    #[test]
    fn static_bvh_never_prunes_unknown_md3_bounds() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        presenter.set_static_models(test_static_props(48));
        let mut props = presenter.build_static_props();
        props[30].radius = 0.0; // the reference code disables frustum for this placement
        props[30].origin = [-9000.0, 0.0, 0.0];
        props[30].cull_origin = [-9000.0, 0.0, 100.0];
        let tree = StaticVisibilityTree::build(&props);
        let view = Ghoul2PresentationView::new(
            scene::render_position([0.0; 3]), scene::render_position([1.0, 0.0, 0.0]),
            1.2, 1.6, 0.0,
        );
        let mut candidates = Vec::new();
        tree.candidates(view, None, None, &mut candidates);
        assert!(candidates.contains(&30));
    }

    #[test]
    fn static_md3_bvh_runtime_toggle_matches_original_surface_order() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        presenter.set_static_models(test_static_props(128));
        let signature = |draws: &[DynamicModelSurface]| draws.iter().map(|draw| {
            (draw.entity_num, bytemuck::cast_slice::<_, u8>(draw.vertices.as_slice()).to_vec(),
                draw.indices.as_ref().clone(), draw.alpha_mode as u8, draw.lighting_origin,
                draw.raster_visible, draw.authored_glow, draw.rt_rigid.is_some(),
                draw.ghoul2_gpu.as_ref().map(|g| g.mesh_key.clone()))
        }).collect::<Vec<_>>();
        for rt in [false, true] {
            for instanced in [false, true] {
                presenter.rt_rigid_casters_enabled = rt;
                presenter.set_static_md3_instances(instanced);
                for forward in [[1.0,0.0,0.0],[-1.0,0.0,0.0],[0.0,0.0,1.0]] {
                    let view = Ghoul2PresentationView::new(
                        scene::render_position([0.0; 3]), scene::render_position(forward),
                        1.2, 16.0 / 9.0, 800.0,
                    );
                    presenter.set_static_md3_bvh(false);
                    let original = presenter.present_static_models(Some(view));
                    presenter.set_static_md3_bvh(true);
                    let accelerated = presenter.present_static_models(Some(view));
                    presenter.set_static_md3_bvh(false);
                    let restored = presenter.present_static_models(Some(view));
                    assert_eq!(signature(&original), signature(&accelerated), "BVH changed geometry/order/RT: rt={rt} instances={instanced}");
                    assert_eq!(signature(&original), signature(&restored), "disable must restore baseline");
                    if rt {
                        assert_eq!(original.iter().filter(|d| d.rt_rigid.is_some()).count(), 128);
                    }
                }
            }
        }
    }

    #[test]
    fn static_bvh_off_does_not_build_or_fill_hierarchy_storage() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        presenter.set_static_models(test_static_props(128));
        presenter.present_static_models(None);
        assert!(presenter.static_visibility_tree.nodes.is_empty());
        assert_eq!(presenter.static_candidate_scratch.capacity(), 0);
        presenter.set_static_md3_bvh(true);
        assert!(!presenter.static_visibility_tree.nodes.is_empty());
        presenter.set_static_md3_bvh(false);
        presenter.set_static_models(test_static_props(2));
        presenter.present_static_models(None);
        assert!(presenter.static_visibility_tree.nodes.is_empty(), "OFF rebuild must not build the tree");
    }

    #[test]
    fn repeated_static_md3_a_b_reuses_local_geometry_and_preserves_transforms() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        presenter.set_static_models(test_static_props(2));
        let baseline = presenter.present_static_models(None);
        assert_eq!(baseline.len(), 2);
        assert!(baseline.iter().all(|draw| draw.ghoul2_gpu.is_none()));
        presenter.set_static_md3_instances(true);
        let instanced = presenter.present_static_models(None);
        assert_eq!(instanced.len(), baseline.len());
        assert_eq!(presenter.static_md3_meshes.len(), 1);
        let left = instanced[0].ghoul2_gpu.as_ref().expect("promoted MD3");
        let right = instanced[1].ghoul2_gpu.as_ref().expect("promoted MD3");
        assert_eq!(left.mesh_key, right.mesh_key);
        assert!(Arc::ptr_eq(&left.vertices, &right.vertices));
        assert!(Arc::ptr_eq(&left.indices, &right.indices));
        assert!(Arc::ptr_eq(&left.bones, &right.bones));
        assert_eq!(left.vertices[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(left.indices.as_slice(), &[0, 2, 1]);
        for (baseline, gpu) in baseline.iter().zip(instanced.iter()) {
            let skin = gpu.ghoul2_gpu.as_ref().unwrap();
            assert_eq!(baseline.alpha_mode, gpu.alpha_mode);
            assert_eq!(baseline.authored_glow, gpu.authored_glow);
            assert_eq!(baseline.lighting_origin, gpu.lighting_origin);
            assert_eq!(baseline.vertices[0].position,
                transform_model_point(skin.vertices[0].position, skin.axis, skin.origin, 1.0));
            assert_eq!(baseline.vertices[0].normal,
                transform_model_normal(skin.vertices[0].normal, skin.axis));
        }
        presenter.set_static_md3_instances(false);
        assert!(presenter.present_static_models(None).iter().all(|draw| draw.ghoul2_gpu.is_none()));
    }

    #[test]
    fn static_md3_instancing_keeps_singletons_and_rt_casters_on_cpu() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        presenter.set_static_models(test_static_props(1));
        presenter.set_static_md3_instances(true);
        assert!(presenter.present_static_models(None).iter().all(|draw| draw.ghoul2_gpu.is_none()));
        presenter.set_static_models(test_static_props(2));
        presenter.rt_rigid_casters_enabled = true;
        let draws = presenter.present_static_models(None);
        assert_eq!(draws.len(), 2);
        assert!(draws.iter().all(|draw| draw.ghoul2_gpu.is_none() && draw.rt_rigid.is_some()));
    }

    #[test]
    fn static_md3_multistage_stays_on_original_draw_path() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        let asset = Arc::get_mut(presenter.md3_models.get_mut("synthetic.md3").unwrap()).unwrap();
        asset.surfaces[0].extra_stages.push(Md3ExtraStage {
            index: 1, texture: None, alpha_mode: DynamicModelAlphaMode::Opaque,
            authored_glow: false, rgb_gen: RgbGen::Identity, alpha_gen: AlphaGen::Identity,
            rgb_const: [1.0; 3], alpha_const: 1.0, tc_gen: TcGen::Base, tc_mods: vec![],
        });
        presenter.set_static_models(test_static_props(2));
        presenter.set_static_md3_instances(true);
        let draws = presenter.present_static_models(None);
        assert_eq!(draws.len(), 4);
        assert!(draws.iter().all(|draw| draw.ghoul2_gpu.is_none()));
    }

    #[test]
    fn md3_glass_two_stages_are_lit_then_subtle_environment_reflection() {
        let script = r#"
models/map_objects/factory/glass_b
{
  q3map_nolightmap
  cull twosided
  {
    map models/map_objects/factory/glass
    blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
    rgbGen lightingDiffuse
  }
  {
    map models/map_objects/factory/glass_reflect
    blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
    detail
    alphaGen const 0.05
    tcGen environment
    tcMod scale 1.5 1.5
  }
}
"#;
        let library = jka_assets::shader::parse(script).unwrap();
        let shader = &library["models/map_objects/factory/glass_b"];
        assert_eq!(shader.stages.len(), 2);
        assert_eq!(md3_stage_alpha_mode(&shader.stages[0]), DynamicModelAlphaMode::Blend);
        assert_eq!(md3_stage_alpha_mode(&shader.stages[1]), DynamicModelAlphaMode::BlendUnlit);
        assert!(md3_extra_stage_supported(&shader.stages[1]));
        assert!(matches!(shader.stages[0].rgb_gen, RgbGen::LightingDiffuse));
        let stage = Md3ExtraStage {
            index: 1, texture: None,
            alpha_mode: md3_stage_alpha_mode(&shader.stages[1]),
            authored_glow: false,
            rgb_gen: shader.stages[1].rgb_gen,
            alpha_gen: shader.stages[1].alpha_gen,
            rgb_const: shader.stages[1].color.unwrap_or([1.0; 3]),
            alpha_const: shader.stages[1].alpha.unwrap_or(1.0),
            tc_gen: shader.stages[1].tc_gen,
            tc_mods: shader.stages[1].tc_mods.clone(),
        };
        let color = md3_stage_color(&stage, [0.3, 0.4, 0.5, 0.8]);
        assert_eq!(&color[0..3], &[1.0, 1.0, 1.0]);
        assert!((color[3] - 0.05).abs() < 1.0e-5);
        let vertex = DynamicModelVertex {
            position: [0.0, 0.0, 0.0], normal: [0.0, 1.0, 0.0],
            uv: [0.7, 0.2], color: [1.0; 4], depth_hack: 0.0, water_wetness: 0.0,
        };
        let uv = md3_stage_uv(&vertex, &stage, Some([0.0, 10.0, 0.0]));
        assert!(uv.iter().all(|value| value.is_finite()));
        assert_ne!(uv, vertex.uv);
    }

    #[test]
    fn md3_twosided_stage_keeps_original_geometry_unchanged() {
        let source = Arc::new(vec![0, 1, 2]);
        assert!(Arc::ptr_eq(&source, &md3_two_sided_indices(&source, false)));
        assert_eq!(md3_two_sided_indices(&source, true).as_slice(), &[0, 1, 2, 0, 2, 1]);
        assert_eq!(source.as_slice(), &[0, 1, 2]);
    }

    #[test]
    fn clear_blend_is_noop_but_whiteimage_alone_is_not() {
        for texture in ["$whiteimage", "textures/test/not_white"] {
            let shaders = jka_assets::shader::parse(&shader_def("clear", &format!(
                "map {texture}\nblendFunc GL_ZERO GL_ONE"))).unwrap();
            assert!(md3_shader_is_color_depth_noop(&shaders["clear"]));
        }
        for shader in [
            shader_def("bright", "map $whiteimage\nblendFunc GL_ONE GL_ONE"),
            shader_def("bright", "map $whiteimage\nblendFunc GL_ZERO GL_ONE\ndepthWrite"),
            format!("bright\n{{\n{{\nmap $whiteimage\nblendFunc GL_ZERO GL_ONE\n}}\n{{\nmap textures/a\nblendFunc GL_ONE GL_ONE\n}}\n}}\n"),
        ] {
            let shaders = jka_assets::shader::parse(&shader).unwrap();
            assert!(!md3_shader_is_color_depth_noop(&shaders["bright"]));
        }
    }

    #[test]
    fn server_remap_clear_then_visible_restores_original_md3_without_reload() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        let clear = jka_assets::shader::parse(&shader_def("clear",
            "map $whiteimage\nblendFunc GL_ZERO GL_ONE")).unwrap();
        let visible = jka_assets::shader::parse(&shader_def("visible",
            "map $whiteimage\nblendFunc GL_ONE GL_ONE")).unwrap();
        presenter.shaders.extend(clear);
        presenter.shaders.extend(visible);
        let original = Arc::clone(&presenter.md3_models["synthetic.md3"].model);
        let origin = [0.0; 3];
        let axis = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let normal = ItemDraw { model: "synthetic.md3".into(), origin, axis,
            rgba: [1.0; 4], custom_shader: None };
        let mut remaps = scene::ShaderRemapState::default();
        assert!(remaps.apply("test", "clear", Some(0.0)));
        presenter.set_shader_remaps(remaps.clone());
        assert!(presenter.md3_models["synthetic.md3"].surfaces[0].invisible_without_override);
        assert!(presenter.present_md3_ref_entity(1023, &normal).unwrap().is_empty());
        assert!(presenter.present_static_md3(1023, "synthetic.md3", origin, axis,
            [1.0; 4], 0).unwrap().is_empty());
        let with_override = ItemDraw { model: "synthetic.md3".into(), origin, axis,
            rgba: [1.0; 4], custom_shader: Some("visible") };
        assert_eq!(presenter.present_md3_ref_entity(1023, &with_override).unwrap().len(), 1);
        // The custom override itself can also become a no-op.
        let hidden_override = ItemDraw { model: "synthetic.md3".into(), origin, axis,
            rgba: [1.0; 4], custom_shader: Some("clear") };
        assert!(presenter.present_md3_ref_entity(1023, &hidden_override).unwrap().is_empty());
        assert!(remaps.apply("test", "visible", Some(0.0)));
        presenter.set_shader_remaps(remaps.clone());
        assert!(!presenter.md3_models["synthetic.md3"].surfaces[0].invisible_without_override);
        assert!(Arc::ptr_eq(&original, &presenter.md3_models["synthetic.md3"].model));
        assert_eq!(presenter.present_md3_ref_entity(1023, &normal).unwrap().len(), 1);
        assert_eq!(presenter.present_static_md3(1023, "synthetic.md3", origin, axis,
            [1.0; 4], 0).unwrap().len(), 1);
        // Remap source to itself -> original surface material, not clear.
        assert!(remaps.apply("test", "test", None));
        presenter.set_shader_remaps(remaps);
        assert!(!presenter.md3_models["synthetic.md3"].surfaces[0].invisible_without_override);
        assert!(Arc::ptr_eq(&original, &presenter.md3_models["synthetic.md3"].model));
        assert_eq!(presenter.present_md3_ref_entity(1023, &normal).unwrap().len(), 1);
    }

    #[test]
    fn animated_fx_frames_keep_modulation_policy_and_cached_images() {
        use crate::fx::draw::FxBlend;
        let fixture = crate::cvar_compatibility::Fixture::new();
        std::fs::create_dir_all(fixture.0.join("textures/test")).unwrap();
        std::fs::create_dir_all(fixture.0.join("shaders")).unwrap();
        for name in ["a", "b"] {
            image::save_buffer(fixture.0.join(format!("textures/test/{name}.png")),
                &[128, 128, 128, 255], 1, 1, image::ColorType::Rgba8).unwrap();
        }
        std::fs::write(fixture.0.join("shaders/animation.shader"),
            "test {\nclampTime 0.15\n{\nclampAnimMap 10 textures/test/a textures/test/b\nblendFunc GL_DST_COLOR GL_SRC_COLOR\n}\n}\n").unwrap();
        let mut presenter = EntityPresenter::new(AssetSearchPath::open(&fixture.0).unwrap(), false).unwrap();
        let stages = presenter.fx_material_stages("test");
        assert_eq!(stages[0].blend, FxBlend::Modulate2x);
        let frames = &stages[0].animation.as_ref().unwrap().frames;
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|frame| !frame.srgb && frame.clamp));
        assert_eq!(stages[0].shader_clamp_time, Some(0.15));
        assert!(Arc::ptr_eq(&stages[0].texture_at_time(10.0).unwrap(), &frames[1]));
        let cached = presenter.fx_material_stages("test");
        assert!(Arc::ptr_eq(&cached[0].animation.as_ref().unwrap().frames[1], &frames[1]));
        assert!(presenter.load_texture_arc("textures/test/b", true).unwrap().srgb);
    }

    #[test]
    fn imported_materials_keep_tint_and_emissive_passes(){
        let fixture=crate::cvar_compatibility::Fixture::new();std::fs::create_dir_all(fixture.0.join("shaders")).unwrap();
        std::fs::write(fixture.0.join("shaders/ports.shader"),"models/fnemote/test\n{\n { map $whiteimage\n rgbGen const ( 0.2 0.3 0.4 ) }\n { map $whiteimage\n blendFunc add\n glow\n rgbGen const ( 0.1 0 0 ) }\n}\n").unwrap();
        let (_old,mut original)=synthetic_md3_presenter();let mut model=original.md3_models.remove("synthetic.md3").unwrap();
        let mut copied=(*model.model).clone();copied.surfaces[0].shader="models/fnemote/test".into();Arc::get_mut(&mut model).unwrap().model=Arc::new(copied);
        let mut presenter=EntityPresenter::new(AssetSearchPath::open(&fixture.0).unwrap(),false).unwrap();presenter.md3_models.insert("models/fnemote/test.md3".into(),model);
        let draws=presenter.present_static_md3(0,"models/fnemote/test.md3",[0.;3],[[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]],[1.;4],0).unwrap();
        assert_eq!(draws.len(),2);assert_eq!(draws[0].vertices[0].color,[0.2,0.3,0.4,1.]);assert!(draws[1].authored_glow);assert_eq!(draws[1].vertices[0].color,[0.1,0.,0.,1.]);
    }

    #[test]
    fn independent_prop_skinning_uses_local_bones_and_rejects_bad_bindings(){
        let fixture=crate::cvar_compatibility::Fixture::new();std::fs::create_dir_all(fixture.0.join("models/fnemote")).unwrap();
        let vertices=(0..3).map(|i|serde_json::json!({"position":[i,0,0],"normal":[0,0,1],"uv":[0,0],"bones":[0,0,0,0],"weights":[1,0,0,0]})).collect::<Vec<_>>();
        let mesh=serde_json::json!({"version":1,"bones":["Root"],"vertices":vertices,"indices":[0,1,2],"sections":[{"first":0,"end":3,"shader":"solid"}]});
        std::fs::write(fixture.0.join("models/fnemote/test.djk_prop"),serde_json::to_vec(&mesh).unwrap()).unwrap();
        let mut presenter=EntityPresenter::new(AssetSearchPath::open(&fixture.0).unwrap(),false).unwrap();let axis=[[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]];
        let pose=[[[1.,0.,0.,5.],[0.,1.,0.,6.],[0.,0.,1.,7.]]];
        let draws=presenter.present_imported_prop(0,"models/fnemote/test.djk_prop",[10.,20.,30.],axis,[1.;4],Some(&pose)).unwrap();
        assert_eq!(draws[0].vertices[0].position,scene::render_position([15.,26.,37.]));
        assert!(presenter.present_imported_prop(0,"models/fnemote/test.djk_prop",[0.;3],axis,[1.;4],Some(&[])).is_err());
    }
    #[test]
    fn imported_hologram_program_keeps_blue_glow_in_profile_lighting(){
        let fixture=crate::cvar_compatibility::Fixture::new();std::fs::create_dir_all(fixture.0.join("models/fnemote")).unwrap();
        let vertices=(0..3).map(|i|serde_json::json!({"position":[i,0,0],"normal":[0,0,1],"uv":[0,0],"bones":[0,0,0,0],"weights":[1,0,0,0]})).collect::<Vec<_>>();
        let mesh=serde_json::json!({"version":1,"bones":["Root"],"vertices":vertices,"indices":[0,1,2],"sections":[{"first":0,"end":3,"shader":"models/fnemote/hologram"}]});
        std::fs::write(fixture.0.join("models/fnemote/test.djk_prop"),serde_json::to_vec(&mesh).unwrap()).unwrap();
        std::fs::write(fixture.0.join("models/fnemote/hologram.djk_material"),br#"{"version":1,"unlit":true,"output":0,"nodes":[{"op":"constant","value":[0.009131,0.310453,0.913099,0.8]}]}"#).unwrap();
        let mut presenter=EntityPresenter::new(AssetSearchPath::open(&fixture.0).unwrap(),false).unwrap();
        let mut draws=presenter.present_imported_prop(0,"models/fnemote/test.djk_prop",[0.;3],[[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]],[1.;4],None).unwrap();
        let before=draws[0].vertices[0].color;assert!(before[2]>before[1]&&before[1]>before[0]);assert!(draws[0].authored_glow);assert!(draws[0].lighting_origin.is_none());
        crate::cgame::player_presenter::apply_profile_studio_light(&mut draws,true);assert_eq!(draws[0].vertices[0].color,before);
    }

    #[test]
    fn md3_geometry_cache_preserves_animation_transforms_colour_and_depth_hack() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        let (_reference_fixture, mut reference_presenter) = synthetic_md3_presenter();
        presenter.set_md3_geometry_cache(true);
        reference_presenter.set_md3_geometry_cache(false);
        let origin = [10.0, 20.0, 30.0];
        // Includes non-uniform and negative scale in the authored axis.
        let axis = [[2.0, 0.0, 0.0], [0.0, -3.0, 0.0], [0.0, 0.0, 4.0]];
        let asset = Arc::clone(&presenter.md3_models["synthetic.md3"]);
        let surface = &asset.surfaces[0];
        let mut previous = None;
        for variant in 0..7 {
            let frame = usize::from(variant != 0);
            let mut origin = origin;
            let mut axis = axis;
            let mut colour = [0.8, 0.6, 0.4, 0.3];
            let mut depth_hack = 0.0;
            let mut scale = 1.0;
            match variant {
                2 => origin[0] += 1.0,
                3 => axis[0][1] = 0.5,
                4 => colour[2] = 0.2,
                5 => depth_hack = 1.0,
                6 => scale = -0.5,
                _ => {},
            }
            let reference = reference_presenter.md3_geometry(&asset, surface, 7, 0, frame, axis, origin, scale, colour, depth_hack);
            let cached = presenter.md3_geometry(&asset, surface, 7, 0, frame, axis, origin, scale, colour, depth_hack);
            let hit = presenter.md3_geometry(&asset, surface, 7, 0, frame, axis, origin, scale, colour, depth_hack);
            assert!(Arc::ptr_eq(&cached.0, &hit.0));
            assert!(Arc::ptr_eq(&cached.1, &hit.1));
            assert_eq!(bytemuck::cast_slice::<_, u8>(&reference.0), bytemuck::cast_slice::<_, u8>(&hit.0));
            assert_eq!(reference.1, hit.1);
            if let Some(old) = previous { assert!(!Arc::ptr_eq(&cached.0, &old)); }
            previous = Some(cached.0);
        }
        // A changed frame must never freeze on a cached frame-0 vertex.
        let old = presenter.md3_geometry(&asset, surface, 7, 0, 0, axis, origin, 1.0, [1.0; 4], 0.0);
        let next = presenter.md3_geometry(&asset, surface, 7, 0, 1, axis, origin, 1.0, [1.0; 4], 0.0);
        assert_ne!(old.0[0].position, next.0[0].position);
        assert_eq!(next.0[0].position, [12.0, 66.0, -14.0]);
        assert!(Arc::ptr_eq(&old.1, &next.1));
        assert_eq!(presenter.md3_geometry.len(), 2); // ordinary and depth-hack slots
        presenter.set_md3_geometry_cache(false);
        assert!(presenter.md3_geometry.is_empty());
    }

    #[test]
    fn md3_geometry_cache_keeps_multistage_and_view_weapon_draw_semantics() {
        let (_fixture, mut presenter) = synthetic_md3_presenter();
        presenter.set_md3_geometry_cache(true);
        let axis = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let origin = [10.0, 20.0, 30.0];
        let asset = Arc::clone(&presenter.md3_models["synthetic.md3"]);
        let surface = &asset.surfaces[0];
        let a = presenter.md3_geometry(&asset, surface, 7, 0, 0, axis, origin, 1.0, [0.3; 4], 0.0);
        let b = presenter.md3_geometry(&asset, surface, 7, 1, 0, axis, origin, 1.0, [0.6; 4], 0.0);
        let a2 = presenter.md3_geometry(&asset, surface, 7, 0, 0, axis, origin, 1.0, [0.3; 4], 0.0);
        assert!(Arc::ptr_eq(&a.0, &a2.0));
        assert_ne!(a.0[0].color, b.0[0].color);
        let view = presenter.present_view_md3(7, "synthetic.md3", origin, axis).unwrap();
        let world = presenter.present_static_md3(7, "synthetic.md3", origin, axis, [1.0; 4], 0).unwrap();
        assert_eq!(view[0].vertices[0].depth_hack, 1.0);
        assert_eq!(world[0].vertices[0].depth_hack, 0.0);
        assert_eq!(view[0].indices.as_slice(), &[0, 2, 1]);
        assert!(view[0].rt_rigid.is_none());
        assert!(world[0].authored_glow);
        assert!(world[0].raster_visible);
        presenter.set_draw_map_models(false);
        assert!(presenter.present_static_models(None).is_empty());
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s"]
    fn repeater_impact_uses_neutral_modulation_and_authored_ripple_animation() {
        use crate::fx::draw::FxBlend;
        use jka_assets::shader::TcMod;
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut presenter = EntityPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        for (shader, passes) in [("gfx/effects/shock_ball", 1), ("gfx/effects/shock_ripple", 2)] {
            let color_texture = presenter.load_texture_arc(shader, false).unwrap();
            assert!(color_texture.srgb);
            let stages = presenter.fx_material_stages(shader);
            assert_eq!(stages.len(), passes);
            for stage in &stages {
                assert_eq!(stage.blend, FxBlend::Modulate2x);
                assert!(!stage.rgb_vertex, "stock impact stages use rgbGen identity");
                let texture = stage.texture.as_ref().unwrap();
                assert!(!texture.srgb, "multiplier bytes must bypass sRGB decoding");
                assert_eq!(texture.rgba, color_texture.rgba);
                // Ten overlapping shock-ball quads must preserve the scene at
                // their grey borders, rather than turning them into black squares.
                for byte in &texture.rgba[..3] {
                    let factor = 2.0 * f32::from(*byte) / 255.0;
                    assert!((factor.powi(10) - 1.0).abs() < 0.05);
                }
            }
            assert!(presenter.load_texture_arc(shader, false).unwrap().srgb,
                "the ordinary color cache must keep its sRGB sampling");
            let cached = presenter.fx_material_stages(shader);
            assert!(Arc::ptr_eq(stages[0].texture.as_ref().unwrap(), cached[0].texture.as_ref().unwrap()));
        }
        let ripple = presenter.fx_material_stages("gfx/effects/shock_ripple");
        assert_eq!(ripple[0].tc_mods, vec![TcMod::Scale(3.0, 1.0), TcMod::Scroll(0.9, 0.0)]);
        assert_eq!(ripple[1].tc_mods, vec![TcMod::Scale(2.0, 1.0), TcMod::Scroll(-0.4, 0.0)]);
        for shader in ["gfx/misc/steam2", "gfx/misc/steam3"] {
            let stages = presenter.fx_material_stages(shader);
            assert_eq!(stages[0].blend, FxBlend::Add);
            assert!(stages[0].texture.as_ref().unwrap().srgb);
        }
        let burn = presenter.fx_material_stages("gfx/damage/burnmark4");
        assert_eq!(burn[0].blend, FxBlend::Alpha);
        assert!(burn[0].texture.as_ref().unwrap().clamp);
    }

    #[test]
    fn invulnerability_sphere_matches_taystjk_origin_and_faces_view() {
        let player = [100.0, 200.0, 32.0];
        let view = [20.0, 200.0, 41.0];
        let (origin, axis) = invulnerability_sphere_transform(player, view).expect("sphere");
        assert_eq!(origin, [100.0, 200.0, 41.0]);
        // CG_DrawPlayerSphere starts from the vector sphere-view and applies
        // its 180-degree pitch/roll correction. At minimum keep a normalized
        // refEntity axis and the exact +9 origin contract locked down.
        for basis in axis {
            let len = (basis[0] * basis[0] + basis[1] * basis[1] + basis[2] * basis[2]).sqrt();
            assert!((len - 1.0).abs() < 1.0e-4, "{basis:?}");
        }
        assert!(invulnerability_sphere_transform(origin, origin).is_none());
    }

    /// Developer profile of what a demo asks CGame to present. Prints the
    /// entity-type histogram, per-type resource keys and event counts.
    #[test]
    #[ignore = "diagnostic: requires JKA_TEST_BASE; DEMO selects demos/<name>.dm_26 (default TEST)"]
    fn demo_entity_profile() {
        use jka_protocol::{demo::DemoReader, entity_event::{EntityEvent, EV_EVENT_BITS}, server::{Decoder, Event}};
        use std::collections::BTreeMap;
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let demo = std::env::var("DEMO").unwrap_or_else(|_| "TEST".into());
        let bytes = assets.read(&format!("demos/{demo}.dm_26"), 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut types: BTreeMap<i32, usize> = BTreeMap::new();
        let mut keys: BTreeMap<String, usize> = BTreeMap::new();
        let mut events: BTreeMap<i32, usize> = BTreeMap::new();
        let mut snapshots = 0;
        let mut last_message = -1;
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                        println!("PROFILE map={:?}", decoder.map_name());
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshot.message_num == last_message { continue; }
                        last_message = snapshot.message_num;
                        snapshots += 1;
                        for state in &snapshot.entities {
                            let et = state.field_i32("eType").unwrap_or(-1);
                            *types.entry(et).or_insert(0) += 1;
                            if et >= super::super::ET_EVENTS {
                                *events.entry(et - super::super::ET_EVENTS).or_insert(0) += 1;
                                continue;
                            }
                            let ev = state.field_i32("event").unwrap_or(0) & !EV_EVENT_BITS;
                            if ev != EntityEvent::EV_NONE.as_i32() { *events.entry(ev).or_insert(0) += 1; }
                            let mi = state.field_i32("modelindex").unwrap_or(0);
                            let key = match et {
                                2 => format!("ITEM modelindex={mi}"),
                                3 => format!("MISSILE weapon={} modelindex={mi} model={:?} eFlags={:#x}",
                                    state.field_i32("weapon").unwrap_or(0), game.model_qpath(mi),
                                    state.field_i32("eFlags").unwrap_or(0)),
                                13 => format!("NPC modelindex={mi} model={:?} weapon={} NPC_class={} g2radius={} modelScale={:?}",
                                    game.model_qpath(mi), state.field_i32("weapon").unwrap_or(0),
                                    state.field_i32("NPC_class").unwrap_or(-1), state.field_i32("g2radius").unwrap_or(0),
                                    state.field_i32("iModelScale")),
                                0 | 5 | 15 | 17 => format!("eType={et} modelindex={mi} model={:?} g2={} weapon={} eFlags={:#x} owner={} g2radius={} modelindex2={}",
                                    if et == 17 { game.effect_qpath(mi) } else { game.model_qpath(mi) },
                                    state.field_i32("modelGhoul2").unwrap_or(0), state.field_i32("weapon").unwrap_or(0),
                                    state.field_i32("eFlags").unwrap_or(0), state.field_i32("owner").unwrap_or(-1),
                                    state.field_i32("g2radius").unwrap_or(0), state.field_i32("modelindex2").unwrap_or(0)),
                                _ => continue,
                            };
                            *keys.entry(key).or_insert(0) += 1;
                        }
                        let pev = snapshot.player_state.field_i32("externalEvent").unwrap_or(0) & !EV_EVENT_BITS;
                        if pev != EntityEvent::EV_NONE.as_i32() { *events.entry(pev).or_insert(0) += 1; }
                    }
                    _ => {}
                }
            }
        }
        println!("PROFILE snapshots={snapshots} types={types:?}");
        for (key, count) in &keys { println!("PROFILE {count:>6} {key}"); }
        for (event, count) in &events {
            let name = EntityEvent::from_i32(*event).map(EntityEvent::name).unwrap_or("?");
            println!("PROFILE event {event:>3} {name:<28} x{count}");
        }
    }

    /// TEST.dm_26 (mp/ffa3) has ammo/health/shield MD3 items, Ghoul2 weapon
    /// pickups and force enlightenment powerups.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/TEST.dm_26"]
    fn demo_items_submit_md3_and_ghoul2_geometry() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/TEST.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut presenter = EntityPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        let mut ghoul2 = PlayerPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        ghoul2.set_async_loading(false);
        let mut weapon_fx = WeaponFx::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap());
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut drawn: std::collections::BTreeMap<String, usize> = Default::default();
        let mut respawn_fades = 0;
        let mut snapshots = 0;
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        let (draws, _, _) = presenter.present_snapshot_entities(
                            &entities, &game, snapshot.server_time, &mut ghoul2, &mut weapon_fx, false,
                        );
                        for entity in entities.iter().filter(|entity| entity.entity_type == super::super::ET_ITEM) {
                            let item = jka_movement::bg_item(entity.state.field_i32("modelindex").unwrap_or(0)).unwrap();
                            let own = draws.iter().filter(|surface| surface.entity_num == entity.number).collect::<Vec<_>>();
                            if !own.is_empty() {
                                *drawn.entry(item.classname.clone()).or_insert(0) += 1;
                            }
                            if own.iter().any(|surface| surface.vertices.iter().any(|vertex| vertex.color[3] < 1.0)) {
                                respawn_fades += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        println!("ITEM REGRESSION snapshots={snapshots} fades={respawn_fades} drawn={drawn:?}");
        for classname in ["ammo_powercell", "item_medpak_instant", "weapon_repeater", "item_force_enlighten_light"] {
            assert!(drawn.contains_key(classname), "{classname} never submitted geometry");
        }
    }

    /// TEST.dm_26 fires blasters, repeaters, rockets, thermals and concussion
    /// rifles: trails and impacts must run through the FX system and
    /// tessellate with resolved FX materials.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/TEST.dm_26"]
    fn demo_weapon_fx_trails_and_impacts_tessellate() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let open = || AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = open().read("demos/TEST.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut presenter = EntityPresenter::new(open(), false).unwrap();
        let mut ghoul2 = PlayerPresenter::new(open(), false).unwrap();
        ghoul2.set_async_loading(false);
        let mut weapon_fx = WeaponFx::new(open());
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let (mut snapshots, mut trail_frames, mut impact_events, mut peak_draws) = (0, 0, 0, 0);
        let mut textured = 0usize;
        let mut surfaces_total = 0usize;
        let mut kinds = std::collections::BTreeSet::new();
        let view = crate::fx::draw::FxView { origin: [0.0; 3], axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], fov_x: 90.0, seconds: 0.0 };
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        weapon_fx.begin_frame(snapshot.server_time);
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        for event in game.drain_presentation_events() {
                            if let Some(super::super::event_presenter::EventDispatchResult::Handled("FX_MISSILE_IMPACT")) =
                                weapon_fx.entity_event(&event, &game, &[])
                            {
                                impact_events += 1;
                            }
                        }
                        let missiles = entities.iter().filter(|entity| entity.entity_type == super::super::ET_MISSILE).count();
                        presenter.present_snapshot_entities(&entities, &game, snapshot.server_time, &mut ghoul2, &mut weapon_fx, false);
                        let frame = weapon_fx.end_frame();
                        if missiles > 0 && !frame.draws.is_empty() {
                            trail_frames += 1;
                        }
                        peak_draws = peak_draws.max(frame.draws.len());
                        for draw in &frame.draws {
                            kinds.insert(format!("{:?}", std::mem::discriminant(draw)));
                        }
                        let surfaces = crate::fx::draw::tessellate(&frame.draws, &view, &mut |shader| presenter.fx_material_stages(shader));
                        surfaces_total += surfaces.len();
                        textured += surfaces.iter().filter(|surface| surface.texture.is_some()).count();
                    }
                    _ => {}
                }
            }
        }
        println!(
            "FX REGRESSION snapshots={snapshots} trailFrames={trail_frames} impacts={impact_events} peakDraws={peak_draws} kinds={} surfaces={surfaces_total} textured={textured} stats={:?}",
            kinds.len(), weapon_fx.stats()
        );
        assert!(trail_frames > 0, "missiles never produced trail primitives");
        assert!(impact_events > 0, "no EV_MISSILE_* impact effect played");
        assert!(textured * 10 >= surfaces_total * 9, "FX materials failed to resolve textures");
    }

    /// Real demo -> CGame -> CG_Mover -> inline model instances, checked
    /// against the map's own BSP inline models. No window or GPU is needed.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/cheezyVsource.dm_26 (or MOVER_DEMO)"]
    fn demo_movers_submit_real_inline_models_and_move() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let demo = std::env::var("MOVER_DEMO").unwrap_or_else(|_| "cheezyVsource".into());
        let bytes = assets
            .read(&format!("demos/{demo}.dm_26"), 64 * 1024 * 1024)
            .unwrap()
            .expect("regression demo")
            .bytes;
        let mut presenter = EntityPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        let mut ghoul2 = PlayerPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        ghoul2.set_async_loading(false);
        let mut weapon_fx = WeaponFx::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap());
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut drawable: Option<std::collections::BTreeSet<u32>> = None;
        let mut first_pose: HashMap<u32, InlineModelInstance> = HashMap::new();
        let mut moved = std::collections::BTreeSet::new();
        let mut submitted = std::collections::BTreeSet::new();
        let mut snapshots = 0;
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                        let map = decoder.map_name().expect("gamestate map name");
                        let bsp_bytes = assets
                            .read(&format!("maps/{map}.bsp"), jka_assets::bsp::MAX_FILE_BYTES)
                            .unwrap()
                            .expect("demo map BSP")
                            .bytes;
                        let bsp = jka_assets::bsp::Bsp::parse(&bsp_bytes).unwrap();
                        let (_, batch_models) = bsp.inline_models_mesh(4).unwrap();
                        drawable = Some(batch_models.into_iter().collect());
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        game.set_initial_snapshot(snapshot).unwrap();
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        let (_, inline, _) = presenter.present_snapshot_entities(
                            &entities, &game, snapshot.server_time, &mut ghoul2, &mut weapon_fx, false,
                        );
                        snapshots += 1;
                        for instance in inline {
                            submitted.insert(instance.model);
                            let first = *first_pose.entry(instance.model).or_insert(instance);
                            if first != instance {
                                moved.insert(instance.model);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if snapshots >= 3000 && !moved.is_empty() { break; }
        }
        let drawable = drawable.expect("demo had a gamestate");
        println!(
            "MOVER REGRESSION snapshots={snapshots} drawableInline={} submitted={} moved={:?}",
            drawable.len(), submitted.len(), moved
        );
        assert!(!submitted.is_empty(), "no ET_MOVER inline models were submitted");
        // Every submitted mover with surfaces must resolve to BSP geometry; a
        // surfaceless one (e.g. an invisible blocker) legitimately draws nothing.
        assert!(submitted.iter().any(|model| drawable.contains(model)));
        assert!(submitted.iter().all(|&model| (model as usize) < 4096));
        assert!(!moved.is_empty(), "no mover changed pose during the demo");
    }

    #[test]
    fn entity_rgba_defaults_to_white() {
        let entity = PresentedEntity {
            number: 1,
            entity_type: super::super::ET_GENERAL,
            origin: [0.0; 3],
            angles: [0.0; 3],
            state: jka_protocol::gamestate::EntityState {
                number: 1,
                fields: [0; jka_protocol::gamestate::ENTITY_FIELDS.len()],
            },
        };
        assert_eq!(entity_color(&entity), [1.0; 4]);
    }
}

#[cfg(test)]
mod md3_lod_tests {
    use super::*;
    #[test]
    fn taystjk_bias_applies_after_projected_lod() {
        // With no view ProjectRadius=0, so the authored path starts at LOD 0.
        assert_eq!(md3_lod_for_view(None, [0.0;3], 32.0, 3, 5.0, 0, false), 0);
        assert_eq!(md3_lod_for_view(None, [0.0;3], 32.0, 3, 5.0, 1, false), 1);
        assert_eq!(md3_lod_for_view(None, [0.0;3], 32.0, 3, 5.0, 5, false), 2);
        assert_eq!(md3_lod_for_view(None, [0.0;3], 32.0, 3, 5.0, 5, true), 0);
        assert_eq!(md3_lod_for_view(None, [0.0;3], 32.0, 1, 5.0, 1, false), 0);
    }
    #[test]
    fn md3_lod_filenames_match_taystjk() {
        assert_eq!(replace_md3_suffix("models/map/rock.md3", "_2.md3"), "models/map/rock_2.md3");
    }
}
