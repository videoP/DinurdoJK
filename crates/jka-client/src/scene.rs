//! scene module facade. Implementations are grouped by responsibility.
// Retain the existing facade API, including entry points used by external callers or tests.
use entities::parse_map_f32;
use lighting::{
    map_world_lighting, Q3MAP_LIGHTMAP_BYTE_SCALE, Q3MAP_LINEAR_SCALE, Q3MAP_POINT_SCALE,
};
use lighting::{q3map_color_normalize, Q3MAP_FALLOFF_TOLERANCE};
use source_map::brushes::FALLBACK_TEXTURE_SIZE;
#[allow(unused_imports)]
pub(crate) use source_map::brushes::{ReconstructedBrush, ReconstructedFace};
#[allow(unused_imports)]
pub use source_map::prepare::{prepare_map_asset, prepare_map_file};
#[allow(unused_imports)]
pub use vegetation::SurfaceSpriteEffectTriangle;
mod cache;
mod collision;
mod coordinates;
mod entities;
mod environment;
mod geometry;
mod lighting;
mod lightmaps;
mod material_batches;
mod prepare;
mod reflections;
mod source;
mod source_map;
mod vegetation;
mod visibility;
pub(crate) use cache::FoldHasher;
use cache::{map_content_hash, static_bsp_ao_cache_info, StageFingerprint};
pub use cache::{PrepStageKeys, StaticBspAoCacheInfo};
use collision::bsp_physics_collision_mesh;
pub use collision::prepare_physics_collision;
pub use coordinates::{jka_position, render_position};
pub use entities::SkyPortal;
pub(crate) use entities::{append_authored_ocean_planes, bsp_brush_entities};
use entities::{
    bsp_dynamic_lights, bsp_fx_runners, bsp_sky_portal, bsp_worldspawn_distance_cull,
    load_movement, map_dynamic_lights, map_worldspawn_distance_cull, parse_light_triplet,
};
#[cfg(test)]
use entities::{dynamic_light_from_values, map_dynamic_light, parse_distance_cull};
use environment::{
    bsp_weather_occlusion_source, GRASS_FULL_DENSITY_SPACING, GRASS_MAX_BLADES_PER_TRIANGLE,
    GRASS_MAX_SPACING, GRASS_MIN_SPACING, GRASS_PATCH_SIZE, GRASS_REFERENCE_SPRITE_DENSITY,
};
use geometry::{
    mark_surfaces_from_batches, Geometry, GrassPatchKey, GrassWorkerPatchKey, GroupKey,
    PlanarGroupKey, WorldGeometryChunk,
};
#[cfg(test)]
use lighting::{append_surface_light_triangle, voxel_gi_index, voxel_gi_propagate};
use lighting::{
    bsp_surface_lights, build_voxel_probe_gi, light_luminance, prepare_static_light_grid,
    smoothstep_range, STATIC_BSP_AO_CACHE_VERSION,
};
use lightmaps::{
    embedded_deluxe_mapping, embedded_deluxemap_texture, embedded_lightmap_texture,
    external_lightmap_pages, hdr_lightmap_indexed, load_external_lightmap, neutral_deluxemap,
    try_load_external_deluxemap, try_load_hdr_lightmap,
};
pub(crate) use material_batches::append_sky_batches;
use material_batches::{
    append_material_batches, can_fold_jka_lightmap_pair, class_for, material_debug_info,
    material_render_is_guaranteed_discarded, prepared_stages, resolved_bsp_surface_fog,
    stage_batch,
};
#[cfg(test)]
use material_batches::{blend_for, effective_world_cull, stage_class, stage_pipeline};
#[cfg(test)]
pub use prepare::prepare_with_seed;
pub use prepare::{prepare, prepare_with_jobs_options, prepare_with_options};
#[cfg(test)]
use reflections::planar_batch_plane;
use reflections::{
    assign_planar_reflection_planes, assign_reflection_probes, bsp_portal_surface_anchors,
    cache_reflection_decisions, load_reflection_probes, log_reflection_cache,
    map_portal_surface_anchors, planar_group_key, retain_authored_planar_mirrors,
};
#[cfg(test)]
use source::render_sun;
use source::{authored_sun, map_asset_name, validate_map_name};
pub(crate) use source_map::brushes::reconstruct_brush;
use source_map::brushes::{
    face_uv, is_utility_shader, map_chunk_key, map_vec, reconstruct_source_brushes,
    shader_is_render_utility, source_collision_brush, MapGeometry, MapGroupKey, MapMaterial,
};
#[cfg(test)]
use source_map::brushes::{
    source_face_collision_flags, SOURCE_CONTENTS_OPAQUE, SOURCE_CONTENTS_PLAYERCLIP,
    SOURCE_CONTENTS_SOLID,
};
use source_map::geometry::{
    add_bounds, canonical_map_shader, map_spawn_points, material_uv_size, push_face_triangles,
};
#[cfg(test)]
use source_map::prepare::prepare_map_document;
pub use source_map::prepare::{prepare_source, prepare_source_with_jobs};
pub use vegetation::SurfaceSpriteEffectEmitter;
use vegetation::{
    collect_grass_emitters, collect_surface_sprite_effect_emitters, finish_grass_patches_with_jobs,
    generate_grass_chunk, grass_fingerprint, grass_ground_albedo_tint, grass_lightmap_sources,
    grass_local_fog_slots,
};
pub use visibility::auto4_audit_lines;
pub(crate) use visibility::draw_batches_share_state;
use visibility::{build_prepared_portal_draw_plan, order_pvs_pieces, pvs_signature};

// RBSP -> immutable render data. This runs once when a map is loaded.
use crate::{
    lightmap_atlas::Atlas,
    map_jobs::MapJobPool,
    materials::{
        self, BlendFactor, CullMode, MaterialStage, StageTexture, SurfaceMaterial, TextureData,
        Textures,
    },
    thread_activity::Task,
    weather::fog::{
        bsp_fog_params, bsp_global_fog_num, bsp_global_fog_params, material_legacy2_in_stage_safe,
        stage_fog_color_override, FogColorOverride,
    },
};
use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Vec3};
use jka_assets::{
    bsp::{Bsp, SurfaceKind, Visibility, MAX_FILE_BYTES},
    map::{
        MapBrush, MapDocument, MapFace, Plane as MapPlane, TextureProjection, MAX_MAP_FILE_BYTES,
    },
    pk3::{active_game_directory, AssetSearchPath},
    shader::{AlphaGen, RgbGen, Shader, Sun as ShaderSun, TcGen, TcMod},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

const LIGHTMAP_BY_VERTEX: i32 = -3;
const MAX_EXTRACTED_SURFACE_LIGHTS: usize = 128;
const MAX_SURFACE_LIGHT_SUBDIVISION_DEPTH: u8 = 8;
const VOXEL_GI_MAX_AXIS: u32 = 42;
const VOXEL_GI_MIN_CELL_SIZE: f32 = 96.0;
const VOXEL_GI_PROPAGATION_STEPS: usize = 12;
const VOXEL_GI_PROPAGATION: f32 = 0.78;
const VOXEL_GI_MAX_RADIANCE: f32 = 8.0;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub lightmap_uv: [f32; 2],
    pub normal: [f32; 3],
    pub color: [f32; 4],
    /// Legacy per-vertex scalar slot. The BSP renderer uses this as cached static AO;
    /// material alpha cutoff is supplied separately in MaterialUniform.
    pub alpha_cutoff: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DrawClass {
    Sky,
    Opaque,
    Mask,
    Transparent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlendMode {
    Opaque,
    Custom(BlendFactor, BlendFactor),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PipelineKey {
    pub class: DrawClass,
    pub blend: BlendMode,
    pub cull: CullMode,
    pub offset: bool,
    pub depth_write: bool,
    pub depth_equal: bool,
}

pub const REFLECTION_CACHE_PROBE: u8 = 1 << 0;
pub const REFLECTION_CACHE_SSR: u8 = 1 << 1;
pub const REFLECTION_CACHE_PLANAR: u8 = 1 << 2;

#[derive(Debug, Clone)]
pub struct DrawBatch {
    pub vertices: Range<u32>,
    /// Exact BSP shader-table index that produced this draw. Kept only as
    /// developer/debug metadata so the surface inspector never has to infer a
    /// material from the stage texture name.
    /// `u32::MAX` means this came from the source-.map path rather than a BSP.
    pub bsp_shader_index: u32,
    /// Exact material-debug entry for `bsp_shader_index`; `u32::MAX` when absent.
    /// Two packed u32s keep this developer metadata to 8 bytes per draw batch.
    pub material_debug_index: u32,
    pub surface_material: u8,
    /// Image index when this stage samples a regular texture.
    pub texture: Option<usize>,
    /// True when binding 0 should be the BSP lightmap instead of `texture`.
    pub texture_is_lightmap: bool,
    /// True when binding 0 should be the engine's neutral white texture.
    /// Keep this distinct from `texture == None`: the latter means a missing/
    /// unresolved regular image and must continue to use the missing-texture fallback.
    pub texture_is_white: bool,
    /// This stage is a safe receiver for DinurdoJK's optional fallback
    /// close-range detail texture. It is precomputed from authored material
    /// semantics so the fragment shader needs only one packed bit when the
    /// detail pipeline variant is active.
    pub detail_texture_eligible: bool,
    /// Optional linear-data enhancement maps associated with the material base image.
    pub normal_texture: Option<usize>,
    pub roughness_texture: Option<usize>,
    pub height_texture: Option<usize>,
    pub metallic_texture: Option<usize>,
    pub specular_texture: Option<usize>,
    pub emissive_texture: Option<usize>,
    pub height_from_alpha: bool,
    pub rmo_packed: bool,
    pub rmo_specular_alpha: bool,
    pub normal_scale: [f32; 2],
    pub roughness_override: Option<f32>,
    pub specular_reflectance: Option<[f32; 3]>,
    pub parallax_depth: f32,
    /// BSP lightmap page/atlas index for either an explicit `$lightmap` pass or
    /// the compact implicit base-texture × lightmap path.
    pub lightmap: Option<usize>,
    /// True only for the compact implicit material path.
    pub modulate_lightmap: bool,
    /// Multi-stage material with an explicit `$lightmap` stage and no PBR
    /// companion maps: point/RT local lights are added once in the lightmap
    /// stage (which every other stage is multiplied by) instead of in the base
    /// stage, where later stages would overwrite them.
    pub dlight_in_lightmap_stage: bool,
    /// True when q3map stored this surface's baked lighting in BSP vertex colors
    /// (`LIGHTMAP_BY_VERTEX`), including compiled static-model/MD3 geometry.
    pub vertex_lit: bool,
    pub pipeline: PipelineKey,
    pub tc_gen: TcGen,
    pub tc_mods: Vec<TcMod>,
    pub rgb_gen: RgbGen,
    pub alpha_gen: AlphaGen,
    pub color: [f32; 4],
    pub alpha_cutoff: f32,
    /// Map-authored fog color and distance-to-opaque for the surface.
    pub fog: [f32; 4],
    /// This surface explicitly references the BSP global-fog record.
    pub fog_is_global: bool,
    /// OpenJK fixed-function fog-color override for this individual shader stage.
    pub fog_color_override: FogColorOverride,
    /// True when this complete authored material can reproduce Legacy 2 global
    /// GL_EXP2 fog directly in each WGPU stage.
    pub legacy2_fog_in_stage_safe: bool,
    /// True when this material has a depth-writing stage, allowing its frontmost
    /// pixels to receive Legacy 1 global fog in the display-space post pass.
    pub global_fog_post_eligible: bool,
    /// Authored id Tech 3 portal/mirror surface. `planar_plane` is in renderer
    /// coordinates as xyz normal + plane d (`dot(n, p) + d = 0`).
    pub planar_reflection: bool,
    /// Authored `surfaceparm water`; eligible for FFT-ocean promotion.
    pub water: bool,
    /// q3map2 lighting-only transparency metadata retained for future fidelity
    /// work; runtime sun visibility does not consume these authored compile flags yet.
    #[allow(dead_code)]
    pub alpha_shadow: bool,
    #[allow(dead_code)]
    pub light_filter: bool,
    /// First authored stage used as the promoted-water replacement draw.
    pub water_primary: bool,
    pub authored_ocean: Option<usize>,
    /// A `tcGen environment` stage whose source geometry is sufficiently planar
    /// to be promoted to the exact planar-reflection path when requested.
    pub planar_environment_candidate: bool,
    pub planar_plane: [f32; 4],
    /// Origin of the untargeted misc_portal_surface that authorizes this mirror.
    /// id Tech 3 uses this portal entity position as the reflected view's PVS origin.
    pub planar_pvs_origin: [f32; 3],
    pub skybox: Option<[usize; 6]>,
    /// Nearest Rend2 reflection probe selected once at map preparation time.
    pub reflection_probe: Option<usize>,
    /// Probe origin in renderer coordinates and authored parallax radius.
    pub reflection_probe_position_radius: [f32; 4],
    /// Map-load reflection classification. This is deliberately view independent:
    /// realtime selection only spends work among techniques pre-approved here.
    pub reflection_cache_flags: u8,
    /// Cheap map-load roughness estimate used for SSR budgeting/debug. Exact PBR
    /// roughness still comes from the material textures in the shader.
    pub reflection_roughness_hint: f32,
    /// Camera-cluster visibility signature. Bit N is set when this batch may be
    /// visible from BSP cluster N. Empty for sources without compiled PVS.
    pub pvs_signature: Vec<u64>,
    /// Portal-area membership. Bit N means some geometry in this batch belongs
    /// to BSP area N. All-zero means unknown/not-applicable and is therefore
    /// treated conservatively as visible.
    pub area_signature: [u64; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreparedPortalPlanBatchRef {
    /// One FULL PVS piece drawn from its own index range.
    Base(usize),
    /// One collapsed recipe: the visible FULL pieces of one MINIMAL batch.
    Merged(usize),
}

/// AUTO 4 recipe: the FULL pieces of one MINIMAL (coarse) batch that are
/// PVS-visible from some camera cluster, drawn as one physical indexed draw.
#[derive(Debug, Clone)]
pub struct PreparedPortalMergedVariant {
    /// FULL piece whose material/bind state the recipe draws with.
    pub representative: usize,
    /// Visible FULL pieces in vertex (= MINIMAL primitive) order.
    pub members: Vec<usize>,
    /// Physical index recipe. Stages of one surface share one geometry.
    pub geometry: usize,
    /// Union of member area signatures.
    pub area_signature: [u64; 4],
    /// Members carry different area signatures, so a closed areaportal can hide
    /// part of the recipe; the renderer then draws the members individually.
    pub area_mixed: bool,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

/// One unique physical index recipe (a list of FULL piece ranges).
#[derive(Debug, Clone)]
pub struct PreparedPortalGeometry {
    pub members: Vec<usize>,
    /// Members are adjacent in vertex order, so FULL's existing index data
    /// already holds this recipe as one run and nothing has to be built.
    pub contiguous: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PreparedPortalDrawPlan {
    pub variants: Vec<PreparedPortalMergedVariant>,
    pub geometries: Vec<PreparedPortalGeometry>,
    /// Unique static-world recipes. `plan_by_cluster` maps every BSP cluster to
    /// one entry here, so identical PVS rows never duplicate recipe storage.
    pub plans: Vec<Vec<PreparedPortalPlanBatchRef>>,
    pub plan_by_cluster: Vec<usize>,
    /// FULL pieces the plan was built over. Pieces appended afterwards (e.g.
    /// networked authored-ocean planes) carry no PVS and draw in every cluster.
    pub piece_count: usize,
    /// Index count of every non-contiguous geometry, i.e. the eager cost of
    /// collapsing all recipes at once. The renderer builds them lazily.
    pub packed_index_count: usize,
    pub reused_variant_hits: usize,
    pub reused_plan_hits: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SpawnPoint {
    pub position: [f32; 3],
    pub yaw: f32,
    /// `info_player_deathmatch` spawnflag 1 ("initial").
    pub initial: bool,
    /// `nohumans 1`: bot-only spot.
    pub no_humans: bool,
}

/// OpenJK's FFA spawn choice for a human player (`SelectInitialSpawnPoint` /
/// `SelectRandomFurthestSpawnPoint`). `avoid` is the position to stay away
/// from (`ps.origin`; JKA passes the world origin for the initial spawn).
/// Returns an index into `spawns`. `random` yields a value in `[0, 1)`.
///
/// Initial: the first non-`nohumans` spot flagged initial, otherwise the
/// furthest-spot rule. Furthest: rank every usable spot by distance from
/// `avoid` and pick randomly from the furthest half. Spots do not telefrag in
/// solo, so `SpotWouldTelefrag` has nothing to reject.
pub fn select_spawn_index(
    spawns: &[SpawnPoint],
    avoid: [f32; 3],
    initial: bool,
    random: f32,
) -> Option<usize> {
    const MAX_SPAWN_POINTS: usize = 128;
    let usable = |spawn: &SpawnPoint| !spawn.no_humans;
    if initial {
        if let Some(index) = spawns
            .iter()
            .position(|spawn| usable(spawn) && spawn.initial)
        {
            return Some(index);
        }
    }
    let mut ranked: Vec<(f32, usize)> = spawns
        .iter()
        .enumerate()
        .filter(|(_, spawn)| usable(spawn))
        .take(MAX_SPAWN_POINTS)
        .map(|(index, spawn)| {
            let delta = std::array::from_fn::<f32, 3, _>(|axis| spawn.position[axis] - avoid[axis]);
            (
                delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2],
                index,
            )
        })
        .collect();
    if ranked.is_empty() {
        // JKA falls back to the first spot rather than refusing to spawn.
        return (!spawns.is_empty()).then_some(0);
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let half = (ranked.len() / 2).max(1);
    let pick = ((random.clamp(0.0, 0.999_999) * half as f32) as usize).min(half - 1);
    Some(ranked[pick].1)
}

/// Server-facing representation of a map-authored `fx_runner`. Values remain
/// in JKA coordinates because the local-server shim publishes them through the
/// same protocol/CGame boundary as a remote game server.
#[derive(Debug, Clone)]
pub struct MapFxRunner {
    pub effect: String,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub delay_ms: i32,
    pub random_ms: i32,
    pub spawnflags: i32,
}

/// A BSP entity whose `model` key names an inline model (`*N`), kept as the
/// spawn variables the game module would see. The local server shim runs its
/// port of the OpenJK spawn functions over these; a remote server sends the
/// resulting ET_MOVER entities in its snapshots instead. The point entities
/// that pass a use along to movers (target_relay, trigger_always, ...) are
/// kept too, with `model` 0 and no bounds.
#[derive(Debug, Clone)]
pub struct MapBrushEntity {
    pub model: u32,
    /// CM_ModelBounds of the inline model in JKA units (`r.mins`/`r.maxs`).
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// A func_train's path: `corners[0]` is the entity named by `target`,
    /// linked like Think_SetupTrainTargets. Empty for every other class.
    pub train_corners: Vec<TrainCorner>,
    pub spawn_vars: Vec<(String, String)>,
}

/// One path_corner of a func_train route (JKA units).
#[derive(Debug, Clone, PartialEq)]
pub struct TrainCorner {
    pub origin: [f32; 3],
    /// `speed` key: 0 means use the train's own speed.
    pub speed: f32,
    /// `wait` key in seconds before moving on.
    pub wait: f32,
    /// Index of the next corner (`nextTrain`); a cycle points back into the list.
    pub next: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum MapSource {
    /// A compiled BSP resolved through the JKA virtual asset search path.
    Bsp(String),
    /// A source .map resolved through the same virtual asset search path.
    Map(String),
    /// Explicit OS path escape hatch for source maps outside GameData search paths.
    MapFile(PathBuf),
    /// Unsaved source-map editor preview. The path remains the real loose .map
    /// identity for materials/debugging, while geometry comes from the editor's
    /// in-memory working text. Never written here.
    MapEditPreview { path: PathBuf, text: Arc<str> },
}

impl MapSource {
    pub fn from_map_argument(argument: &str) -> Result<Self, String> {
        let mut value = argument.trim().replace('\\', "/");
        if value.to_ascii_lowercase().starts_with("maps/") {
            value = value[5..].to_owned();
        }
        let lower = value.to_ascii_lowercase();
        let source = if lower.ends_with(".map") {
            Self::Map(value[..value.len() - 4].to_owned())
        } else if lower.ends_with(".bsp") {
            Self::Bsp(value[..value.len() - 4].to_owned())
        } else if value
            .rsplit('/')
            .next()
            .is_some_and(|leaf| leaf.contains('.'))
        {
            return Err("Map extension must be .bsp or .map".into());
        } else {
            // Preserve classic JKA behavior: an extensionless map command means BSP.
            Self::Bsp(value)
        };
        match &source {
            Self::Bsp(name) | Self::Map(name) => validate_map_name(name)?,
            Self::MapFile(_) | Self::MapEditPreview { .. } => unreachable!(),
        }
        Ok(source)
    }

    pub fn label(&self) -> String {
        match self {
            Self::Bsp(name) => format!("{name}.bsp"),
            Self::Map(name) => format!("{name}.map"),
            Self::MapFile(path) | Self::MapEditPreview { path, .. } => path.display().to_string(),
        }
    }
}

pub fn verify_map_source_exists(
    root: &Path,
    game: Option<&Path>,
    source: &MapSource,
) -> Result<(), String> {
    match source {
        MapSource::Bsp(name) => {
            let asset_name = map_asset_name(name, "bsp")?;
            let assets =
                AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
            if assets
                .names()
                .any(|candidate| candidate.eq_ignore_ascii_case(&asset_name))
            {
                Ok(())
            } else {
                Err(format!(
                    "Map {asset_name} not found on the game/base asset search path"
                ))
            }
        }
        MapSource::Map(name) => {
            let asset_name = map_asset_name(name, "map")?;
            let assets =
                AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
            if assets
                .names()
                .any(|candidate| candidate.eq_ignore_ascii_case(&asset_name))
            {
                Ok(())
            } else {
                Err(format!(
                    "Source map {asset_name} not found on the game/base asset search path"
                ))
            }
        }
        MapSource::MapFile(path) => {
            if path.is_file() {
                Ok(())
            } else {
                Err(format!("Loose map {} not found", path.display()))
            }
        }
        MapSource::MapEditPreview { path, text } => {
            if !path.is_file() {
                return Err(format!("Loose map {} not found", path.display()));
            }
            if text.len() > MAX_MAP_FILE_BYTES {
                return Err(format!(
                    "Edited map preview {} exceeds {} MiB limit",
                    path.display(),
                    MAX_MAP_FILE_BYTES / (1024 * 1024)
                ));
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MapFileStats {
    pub entities: usize,
    pub brushes: usize,
    pub world_brushes: usize,
    pub grouped_world_brushes: usize,
    pub rendered_faces: usize,
    pub utility_faces_skipped: usize,
    pub skipped_entity_brushes: usize,
    pub patches_skipped: usize,
    pub degenerate_faces: usize,
    pub skipped_brushes: usize,
    pub spatial_chunks: usize,
    pub geometry_groups: usize,
    pub draw_batches: usize,
    pub collision_brushes: usize,
    pub spatial_batching: bool,
    pub worker_count: usize,
    pub reconstruction_ms: f64,
}

/// One inline BSP model's slice of `PreparedMap::inline_vertices` and
/// `PreparedMap::inline_batches`.
#[derive(Debug, Clone)]
pub struct InlineModelGeometry {
    /// BSP model number, i.e. an ET_MOVER's `modelindex` when solid == SOLID_BMODEL.
    pub model: u32,
    pub vertices: Range<u32>,
    pub batches: Range<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct BspMapStats {
    /// OpenJK `cgs.inlineModelMidpoints`, indexed by inline model number
    /// (`*N`, entity `modelindex`). CGame places brush-model loop sounds at
    /// `lerpOrigin + midpoint`, since mover origins are relative offsets.
    pub inline_model_midpoints: std::sync::Arc<[[f32; 3]]>,
    /// `(mins, maxs)` of each inline model, indexed like the midpoints. CGame's
    /// glass shatter tessellates the brush's face from these.
    pub inline_model_bounds: std::sync::Arc<[([f32; 3], [f32; 3])]>,
    pub brushes: usize,
    pub brush_sides: usize,
    pub planes: usize,
    pub surfaces: usize,
    pub vertices: usize,
    pub indices: usize,
    pub collision_nodes: usize,
    pub collision_leaves: usize,
    pub pvs_clusters: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MapLoadTimings {
    pub prepare_wall_ms: f64,
    pub asset_index_ms: f64,
    pub bsp_read_ms: f64,
    pub bsp_parse_ms: f64,
    pub collision_ms: f64,
    pub movement_ms: f64,
    pub shader_read_ms: f64,
    pub shader_parse_wall_ms: f64,
    pub shader_parse_cpu_ms: f64,
    pub texture_preload_wall_ms: f64,
    pub texture_read_ms: f64,
    pub texture_decode_ms: f64,
    pub texture_mip_ms: f64,
    pub texture_images: usize,
    pub generated_normal_ms: f64,
    pub generated_normals: usize,
    pub lightmap_ms: f64,
    pub material_ms: f64,
    pub geometry_ms: f64,
    pub grass_ms: f64,
    pub grass_cpu_ms: f64,
    pub gi_ms: f64,
    pub ocean_ms: f64,
    pub steam_audio_ms: f64,
    pub portal_plans_ms: f64,
    pub worker_count: usize,
    /// Wall time of each consecutive `prepare_internal` phase on the loader
    /// thread (including any worker joins it blocks on); see `PREP_PHASES`.
    pub phase_ms: [f64; PREP_PHASES.len()],
    /// Inside the geometry phase: dlight surfaces, surface walk (including the
    /// vertex expansion), PVS signatures (part of the walk), piece pack, and
    /// piece ordering (part of the pack).
    pub geometry_detail_ms: [f64; 5],
    /// Preloaded texture count and decode CPU by file type (tga, jpg, png).
    pub texture_format_images: [u32; 3],
    pub texture_format_decode_ms: [f64; 3],
    /// Archive handles opened (not reused from the idle pool) during this
    /// preparation, and the time that took summed over all threads.
    pub archive_opens: u32,
    pub archive_open_ms: f64,
}

/// Labels for `MapLoadTimings::phase_ms`, in execution order. Together they
/// cover the whole of `prepare_internal`, so they sum to `prepare_wall_ms`.
pub const PREP_PHASES: [&str; 16] = [
    "startup (index/parse/shaders)",
    "tex submit+collision+meshes+fog",
    "lightmaps",
    "spawns/entities",
    "texture wait",
    "materials",
    "grass setup",
    "geometry+inline",
    "lights/probes/planar",
    "ocean+plan submit",
    "lightmap pages",
    "grass join",
    "gi/audio join",
    "footprints+tex hashes",
    "plan join",
    "debug volumes/final",
];

/// Charge the time since the previous lap to `phase` and restart the clock.
fn phase_lap(timings: &mut MapLoadTimings, phase: usize, clock: &mut Instant) {
    timings.phase_ms[phase] += clock.elapsed().as_secs_f64() * 1000.0;
    *clock = Instant::now();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapPrepareOptions {
    /// Renderer upload policy captured with the prepared world so changing
    /// classic `r_picmip` can use the normal Apply Video Settings/map-reload
    /// path without pretending the old GPU textures already changed.
    pub picmip: u32,
    pub grass: bool,
    pub voxel_probe_gi: bool,
    pub ocean: bool,
    /// Build Rapier's static visual-physics world on the map-prep worker.
    pub client_physics: bool,
    pub gen_normal_maps: bool,
    pub float_lightmap: bool,
    /// Preserve reflection-plane topology for authored mirrors and tcGen
    /// environment surfaces. When false, those surfaces are free to batch like
    /// ordinary opaque geometry; enabling the planar reflection path therefore
    /// requires a map/renderer restart.
    pub planar_reflections: bool,
    /// Also preserve plane topology for tcGen environment surfaces so they can
    /// be promoted to planar reflections (High/Ultra). Below that only authored
    /// portal mirrors need isolation; splitting every environment-mapped
    /// surface by plane would multiply draw calls for nothing.
    pub planar_environment: bool,
    /// Cold-path material specialization for the absolute lowest reflection
    /// quality. When true, authored tcGen environment stages are omitted while
    /// preparing materials instead of surviving as legacy sphere-map passes.
    pub omit_environment_stages: bool,
    /// Source `.map` only: split ordinary opaque material batches into the
    /// Radiant-style 1024-unit spatial grid. This is only profitable when the
    /// renderer can consume those chunks through its GPU-driven indirect path;
    /// otherwise one global batch per material is cheaper to encode on the CPU.
    pub source_spatial_batches: bool,
    /// Material-library policy captured at map load. When false, Rend2 `.mtr`
    /// files are not parsed as overrides; ordinary JKA `.shader` / implicit
    /// materials remain authoritative until the next map load / vid_restart.
    pub pbr_materials: bool,
    /// Ordinary VFS compatibility policy. When false, qpaths supplied by retail
    /// assets0.pk3 through assets3.pk3 cannot be globally shadowed by addon/loose
    /// providers. Explicit dependencies of an active .mtr remain source-affine.
    pub allow_asset_overrides: bool,
    /// Build the static acoustic triangle scene that feeds Steam Audio's
    /// offline probe/reflection/pathing bake. When false there is no acoustic
    /// geometry work during map preparation.
    pub steam_audio: bool,
}

impl Default for MapPrepareOptions {
    fn default() -> Self {
        Self {
            picmip: 0,
            grass: true,
            voxel_probe_gi: true,
            ocean: true,
            client_physics: false,
            gen_normal_maps: false,
            float_lightmap: false,
            planar_reflections: true,
            planar_environment: true,
            omit_environment_stages: false,
            source_spatial_batches: false,
            pbr_materials: true,
            allow_asset_overrides: true,
            steam_audio: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicLightFalloff {
    /// Existing runtime/FX light response: smooth quadratic fade to the cutoff radius.
    Smooth,
    /// q3map2 spawnflag 1 point-light behavior. The stored radius is the exact
    /// zero-contribution distance and is used only for clustered culling.
    Linear,
    /// q3map2 default point-light behavior. The stored radius is only the
    /// -fast/falloff-tolerance envelope; it never changes inverse-square falloff.
    InverseSquare,
}

impl DynamicLightFalloff {
    pub fn shader_value(self) -> f32 {
        match self {
            Self::Smooth => 0.0,
            Self::Linear => 1.0,
            Self::InverseSquare => 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DynamicLight {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub radius: f32,
    pub intensity: f32,
    pub falloff: DynamicLightFalloff,
    /// q3map2 lightJunior contributes to the lightgrid, but not directly to surfaces.
    /// Keeping it in the source list still lets voxel/probe GI consume it.
    pub surface_lighting: bool,
    /// Zero for ordinary point lights. Non-zero identifies an authored q3map
    /// surface emitter and points out of the emitting face.
    pub emitter_normal: [f32; 3],
    /// q3map surface lights can be authored on two-sided materials. Point
    /// lights ignore this flag.
    pub emitter_two_sided: bool,
    /// q3map2 point-light surface-angle attenuation. Runtime/FX and area lights
    /// leave this true; it is only consumed for q3map source-map falloff modes.
    pub angle_attenuation: bool,
    /// q3map2 `_anglescale`; zero means the ordinary Lambert curve.
    pub angle_scale: f32,
    /// q3map2 `_extradist`, folded into the distance before attenuation.
    pub extra_distance: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct DirectionalSun {
    /// Direction the light travels in renderer coordinates.
    pub direction: [f32; 3],
    /// q3map-normalized RGB chromaticity.
    pub color: [f32; 3],
    /// Authored q3map sun intensity.
    pub intensity: f32,
}

impl DirectionalSun {
    /// Build the renderer-space sun from the same azimuth/elevation convention
    /// used by `sun`, `q3map_sun` and `q3map_sunExt` shader directives.
    /// Azimuth is 0=east, 90=north; elevation is degrees above the horizon.
    pub fn from_q3_angles(color: [f32; 3], intensity: f32, azimuth: f32, elevation: f32) -> Self {
        let azimuth = azimuth.to_radians();
        let elevation = elevation.to_radians();
        let cos_elevation = elevation.cos();
        let toward_sun = [
            azimuth.cos() * cos_elevation,
            azimuth.sin() * cos_elevation,
            elevation.sin(),
        ];
        let direction = render_position(toward_sun.map(|value| -value));
        let raw = Vec3::from_array(color).max(Vec3::ZERO);
        let normalized = if raw.length_squared() > 1e-12 {
            raw.normalize()
        } else {
            Vec3::ONE.normalize()
        };
        Self {
            direction,
            color: normalized.to_array(),
            intensity: intensity.max(0.0),
        }
    }

    /// Recover q3map azimuth/elevation from the renderer-space light-travel
    /// vector. This is used by the editor so a custom sun starts exactly at the
    /// current map-authored direction instead of jumping to a hard-coded angle.
    pub fn q3_angles(self) -> [f32; 2] {
        let travel_map = Vec3::from_array(jka_position(self.direction));
        let toward_sun = -travel_map.normalize_or_zero();
        if toward_sun.length_squared() <= 1e-12 {
            return [0.0, 45.0];
        }
        let azimuth = toward_sun
            .y
            .atan2(toward_sun.x)
            .to_degrees()
            .rem_euclid(360.0);
        let elevation = toward_sun.z.clamp(-1.0, 1.0).asin().to_degrees();
        [azimuth, elevation]
    }
}

/// Expanded, renderer-ready RBSP lightgrid. The direction texture stores a
/// render-space incoming-light direction in RGB and the directional-light
/// fraction in A. The lighting texture stores directional-light chromaticity
/// in RGB and validity in A. Keeping this scale-free lets embedded byte grids
/// and Rend2 HDR `lightgrid.raw` data share the same shader path.
#[derive(Debug, Clone)]
pub struct StaticLightGrid {
    pub origin: [f32; 3],
    pub size: [f32; 3],
    pub bounds: [u32; 3],
    pub direction_rgba: Vec<u8>,
    pub lighting_rgba: Vec<u8>,
    /// Bevy irradiance-volume atlas: (Rx, 2*Ry, 3*Rz), + then - face in Y,
    /// X/Y/Z face families in successive Z thirds. Values are linear irradiance.
    pub irradiance_volume_rgba: Vec<u8>,
    /// Converts normalized RGBA8 atlas values back to JKA light units / 255.
    pub irradiance_intensity: f32,
    pub external_hdr: bool,
    classic_entity_grid: ClassicEntityLightGrid,
}

#[derive(Debug, Clone, Copy, Default)]
struct ClassicLightGridCell {
    ambient: [f32; 3],
    directed: [f32; 3],
    direction: [f32; 3],
    /// Fraction (0..1) of `directed` that is the map's baked sun. Filled by
    /// `ClassicEntityLightGrid::estimate_sun_weights`; zero until then, so an
    /// unestimated grid behaves exactly like the stock lightgrid.
    sun_weight: f32,
    valid: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ClassicEntityLight {
    /// OpenJK light units, nominally 0..255 after interpolation/scaling.
    pub ambient: [f32; 3],
    /// OpenJK light units, nominally 0..255 before the final vertex clamp.
    pub directed: [f32; 3],
    /// Unit incoming-light direction in renderer coordinates [x,z,-y].
    pub direction: [f32; 3],
    /// Part of `directed` that is the baked map sun, same units. Already
    /// included in `directed` until `relight_sun` moves it out.
    pub baked_sun: [f32; 3],
    /// Second directional light: the runtime sun after `relight_sun`. Zero
    /// (and ignored by the shaders) otherwise.
    pub sun_directed: [f32; 3],
    /// Unit incoming-light direction of `sun_directed`, renderer coordinates.
    pub sun_direction: [f32; 3],
    /// 0..1 agreement between the probes blended into `direction` (length of the
    /// blended direction over the total weight). Low means neighbouring probes
    /// point different ways, so the blended direction is unreliable. It cannot
    /// see lights cancelling inside one probe: q3map2 bakes only the normalized
    /// sum, so that disagreement is gone from the data.
    pub direction_coherence: f32,
}

impl ClassicEntityLight {
    /// Moves the baked sun share out of the lightgrid's directed term and
    /// replaces it with a separate directional light: `baked_sun` scaled by the
    /// per-channel `gain` (runtime sun radiance / map sun radiance), arriving
    /// from `toward_sun` (unit, renderer coordinates).
    ///
    /// The remaining directed light keeps its own direction, recovered by
    /// subtracting the map sun's luminance-weighted direction from the probe's
    /// blended one, so torch and skylight shading does not swing toward the
    /// removed sun.
    pub fn relight_sun(&mut self, map_toward_sun: [f32; 3], toward_sun: [f32; 3], gain: [f32; 3]) {
        let sun_luma = light_luminance(self.baked_sun);
        if sun_luma <= 1e-4 {
            return;
        }
        let total_luma = light_luminance(self.directed).max(sun_luma);
        let mut rest_direction = [0.0_f32; 3];
        for axis in 0..3 {
            rest_direction[axis] =
                self.direction[axis] * total_luma - map_toward_sun[axis] * sun_luma;
        }
        let rest_len_sq: f32 = rest_direction.iter().map(|v| v * v).sum();
        // With nothing left over the remainder direction is irrelevant; keep the
        // probe direction so the shader never sees a zero vector for a live term.
        if rest_len_sq > 1e-8 {
            let inv = rest_len_sq.sqrt().recip();
            self.direction = rest_direction.map(|v| v * inv);
        }
        for channel in 0..3 {
            self.directed[channel] = (self.directed[channel] - self.baked_sun[channel]).max(0.0);
            self.sun_directed[channel] = self.baked_sun[channel] * gain[channel];
        }
        self.sun_direction = toward_sun;
    }
}

/// Compact CPU-only subset retained after the renderer uploads the static
/// lightgrid textures. This avoids keeping the much larger irradiance atlas in
/// system memory solely for classic entity lighting.
#[derive(Debug, Clone)]
pub struct ClassicEntityLightGrid {
    origin: [f32; 3],
    size: [f32; 3],
    bounds: [u32; 3],
    external_hdr: bool,
    cells: Vec<ClassicLightGridCell>,
    /// Unit direction toward the map's baked sun once `estimate_sun_weights`
    /// has run; `None` means no sun share is known for this grid.
    map_toward_sun: Option<[f32; 3]>,
}

impl StaticLightGrid {
    pub fn into_classic_entity_grid(self) -> ClassicEntityLightGrid {
        self.classic_entity_grid
    }
}

impl ClassicEntityLightGrid {
    /// Direction toward the baked map sun, if `estimate_sun_weights` found one.
    pub fn map_toward_sun(&self) -> Option<[f32; 3]> {
        self.map_toward_sun
    }

    /// Fills each probe's `sun_weight`: how much of its directed light is the
    /// map's baked sun. The lightgrid stores one blended direction and color per
    /// probe, so the sun share is inferred rather than read:
    ///  - a sunlit probe's blended direction lies close to the sun, while torch
    ///    or skylight-dominated probes point elsewhere, and
    ///  - its directed color matches the sun color, which separates it from
    ///    differently tinted sky fill.
    /// This is the single seam for the estimate. A map-load bake that traces the
    /// sun from each probe can replace the body and write exact weights; the
    /// sampler and the shaders only ever consume `sun_weight`.
    ///
    /// `sun_direction` is the direction the light travels (renderer coordinates,
    /// as in `DirectionalSun::direction`); `sun_color` is its chromaticity.
    pub fn estimate_sun_weights(&mut self, sun_direction: [f32; 3], sun_color: [f32; 3]) {
        let length = sun_direction.iter().map(|v| v * v).sum::<f32>().sqrt();
        if length <= 1e-6 {
            self.map_toward_sun = None;
            return;
        }
        let toward_sun = sun_direction.map(|v| -v / length);
        let sun_chroma_len = sun_color
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt()
            .max(1e-6);
        let sun_chroma = sun_color.map(|v| v.max(0.0) / sun_chroma_len);
        for cell in &mut self.cells {
            cell.sun_weight = 0.0;
            if !cell.valid {
                continue;
            }
            let directed_len = cell.directed.iter().map(|v| v * v).sum::<f32>().sqrt();
            if directed_len < 1.0 {
                continue;
            }
            let alignment: f32 = (0..3)
                .map(|axis| cell.direction[axis] * toward_sun[axis])
                .sum();
            let chroma: f32 = (0..3)
                .map(|axis| cell.directed[axis] / directed_len * sun_chroma[axis])
                .sum();
            cell.sun_weight =
                smoothstep_range(0.6, 0.95, alignment) * smoothstep_range(0.85, 0.98, chroma);
        }
        self.map_toward_sun = Some(toward_sun);
    }

    /// Match OpenJK's R_SetupEntityLightingGrid: sample the eight neighboring
    /// BSP probes with trilinear weights, ignore invalid/in-wall probes, and
    /// renormalize the surviving ambient/directed contribution near walls.
    /// `lighting_origin` is in JKA coordinates, exactly like refEntity origin.
    pub fn sample_classic_entity_light(
        &self,
        lighting_origin: [f32; 3],
    ) -> Option<ClassicEntityLight> {
        if self.cells.is_empty()
            || self.bounds.into_iter().any(|bound| bound == 0)
            || self.size.into_iter().any(|size| size.abs() <= f32::EPSILON)
        {
            return None;
        }

        let bounds = self.bounds.map(|bound| bound as i32);
        let mut base = [0_i32; 3];
        let mut frac = [0.0_f32; 3];
        for axis in 0..3 {
            let grid = (lighting_origin[axis] - self.origin[axis]) / self.size[axis];
            let floored = grid.floor();
            base[axis] = (floored as i32).clamp(0, bounds[axis] - 1);
            frac[axis] = grid - floored;
        }

        let bx = self.bounds[0] as usize;
        let by = self.bounds[1] as usize;
        let steps = [1_usize, bx, bx.saturating_mul(by)];
        let base_index = base[0] as usize + bx * (base[1] as usize + by * base[2] as usize);

        let mut ambient = [0.0_f32; 3];
        let mut directed = [0.0_f32; 3];
        let mut direction = [0.0_f32; 3];
        let mut baked_sun = [0.0_f32; 3];
        let mut total_factor = 0.0_f32;

        for corner in 0..8_usize {
            let mut factor = 1.0_f32;
            let mut index = base_index;
            for axis in 0..3 {
                if corner & (1 << axis) != 0 {
                    factor *= frac[axis];
                    index = index.saturating_add(steps[axis]);
                } else {
                    factor *= 1.0 - frac[axis];
                }
            }

            let Some(cell) = self.cells.get(index).copied() else {
                continue;
            };
            if !cell.valid {
                continue;
            }

            total_factor += factor;
            for channel in 0..3 {
                ambient[channel] += factor * cell.ambient[channel];
                directed[channel] += factor * cell.directed[channel];
                baked_sun[channel] += factor * cell.directed[channel] * cell.sun_weight;
                direction[channel] += factor * cell.direction[channel];
            }
        }

        // OpenJK compensates when one or more of the eight probes are invalid
        // (commonly because the point lies in solid) so entities do not dim as
        // they approach walls. Direction is normalized separately below.
        if total_factor > 0.0 && total_factor < 0.99 {
            let inv = total_factor.recip();
            ambient = ambient.map(|value| value * inv);
            directed = directed.map(|value| value * inv);
            baked_sun = baked_sun.map(|value| value * inv);
        }

        // OpenJK defaults: r_ambientScale=0.6 and r_directedScale=1.0.
        ambient = ambient.map(|value| value * 0.6);

        // The classic LDR path adds a 32-unit minimum ambient term. Rend2's HDR
        // lightgrid path skips this while HDR lighting is active.
        if !self.external_hdr {
            ambient = ambient.map(|value| value + 32.0);
            ambient = ambient.map(|value| value.clamp(0.0, 255.0));
        } else {
            ambient = ambient.map(|value| value.max(0.0));
        }

        let length_sq = direction
            .into_iter()
            .map(|value| value * value)
            .sum::<f32>();
        let direction_coherence = if total_factor > 0.0 && length_sq > 1.0e-12 {
            (length_sq.sqrt() / total_factor).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if length_sq > 1.0e-12 {
            let inv_length = length_sq.sqrt().recip();
            direction = direction.map(|value| value * inv_length);
        } else {
            direction = [0.0; 3];
        }

        Some(ClassicEntityLight {
            ambient,
            directed,
            direction,
            baked_sun,
            sun_directed: [0.0; 3],
            sun_direction: [0.0; 3],
            direction_coherence,
        })
    }
}

/// Low-frequency indirect lighting field generated once per map. World geometry
/// is voxelized as an occlusion barrier, light/entity radiance is injected into
/// free cells, and a few Jacobi diffusion steps produce a probe volume that can
/// be sampled cheaply by the BSP shader. Point/entity and emissive-area energy
/// are kept separate so the runtime area-light toggle remains authoritative.
#[derive(Debug, Clone)]
pub struct VoxelProbeGi {
    /// Center of voxel/probe [0,0,0] in renderer coordinates.
    pub origin: [f32; 3],
    pub cell_size: f32,
    pub bounds: [u32; 3],
    pub point_rgba: Vec<[u8; 4]>,
    pub area_rgba: Vec<[u8; 4]>,
    pub occupied_voxels: usize,
}

#[derive(Debug, Clone)]
pub struct ReflectionProbe {
    pub label: String,
    pub position: [f32; 3],
    pub radius: f32,
    pub width: u32,
    pub height: u32,
    pub mip_level_count: u32,
    /// RGBA8, ordered by cube face then mip level to match wgpu LayerMajor.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct WeatherOcclusionSide {
    pub normal: [f32; 3],
    pub distance: f32,
}

#[derive(Debug, Clone)]
pub struct WeatherOcclusionBrush {
    pub mins_xy: [f32; 2],
    pub maxs_xy: [f32; 2],
    pub sides: Vec<WeatherOcclusionSide>,
}

#[derive(Debug, Clone, Copy)]
pub struct WeatherOcclusionTriangle {
    pub positions: [[f32; 3]; 3],
    /// False for surfaces that cannot hold a scattered puddle: open water and
    /// lava, or materials that soak rain up (grass, sand, snow, foliage, cloth).
    /// Only the scattered-puddle score honours this; basins and rain cover do not.
    pub holds_puddles: bool,
}

#[derive(Debug, Clone)]
pub struct WeatherOcclusionSource {
    pub min_xz: [f32; 2],
    pub max_xz: [f32; 2],
    pub brushes: Vec<WeatherOcclusionBrush>,
    /// Geometry that blocks vertically falling precipitation.
    pub triangles: Vec<WeatherOcclusionTriangle>,
    /// Rendered world surfaces used only for physical puddle topography. This is
    /// intentionally broader than collision/weather blockers so planar shader faces
    /// such as taspir/landing_pad still contribute their true height/plane.
    pub topography_triangles: Vec<WeatherOcclusionTriangle>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GrassInstance {
    // Keep only values consumed by the GodotGrass renderer. Stable LOD ordering is
    // derived from position and sorted once during map preparation, so there is no
    // per-blade seed field and renderer upload can copy these records directly.
    // Ground tint reuses those four bytes and keeps the compact layout at 24 bytes.
    // The low 8 mantissa bits of this positive finite height carry a compact local
    // BSP fog slot. Clearing them changes height by < 0.004%, while avoiding any
    // stride/bandwidth increase for maps with local brush fog.
    pub position: [f32; 3],
    pub height: f32,
    pub baked_light_rgba: [u8; 4],
    pub ground_tint_rgba: [u8; 4],
}

#[derive(Debug, Clone)]
pub struct GrassPatch {
    pub instances: Vec<GrassInstance>,
    pub pvs_signature: Vec<u64>,
    pub center: [f32; 3],
    pub radius: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SourceMapLighting {
    /// q3map2 worldspawn `_ambient` / `ambient`, already converted from the
    /// compiler's 0..255 lightmap domain into normalized RGB.
    pub ambient: [f32; 3],
    /// q3map2 worldspawn `_minlight`, in the same normalized RGB domain.
    pub minlight: [f32; 3],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LegacyDlightSurface {
    /// 0 = reject, 1 = bounds + planar face test, 2 = bounds-only test.
    pub cull_kind: u32,
    /// Render-space plane `dot(n, p) = w` for planar BSP surfaces.
    pub plane: [f32; 4],
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

#[derive(Clone)]
pub struct PreparedMap {
    pub authored_oceans: Vec<crate::ocean::authoring::AuthoredOcean>,
    pub movement: Option<jka_movement::PmoveContext>,
    pub collision: Option<jka_movement::CollisionWorld>,
    /// Drawn-and-markable world surfaces (OpenJK R_MarkFragments), shared by
    /// saber marks and blob shadows. Source-.map previews build it from their
    /// render triangles because they have no BSP surface table.
    pub mark_surfaces: Option<Arc<jka_assets::bsp::MarkSurfaces>>,
    /// `misc_model_static` props the client places from the BSP entity string
    /// (the game module frees them, so they never arrive in a snapshot).
    pub static_models: Arc<Vec<jka_assets::bsp::StaticModel>>,
    /// Compact static world surface used only by client-side Rapier visuals.
    /// It stays in JKA coordinates; the Rapier layer owns unit conversion.
    pub physics_collision: crate::cgame::ragdoll::PhysicsMapMesh,
    pub weather_occlusion: Option<WeatherOcclusionSource>,
    pub vertices: Vec<GpuVertex>,
    /// Original BSP surface for every static-world source triangle.
    pub legacy_dlight_triangle_surfaces: Vec<u32>,
    /// OpenJK-style coarse dlight rejection metadata, indexed by BSP surface.
    pub legacy_dlight_surfaces: Vec<LegacyDlightSurface>,
    /// Coarse material/lightmap batches used by OFF and MINIMAL PVS modes.
    pub batches: Vec<DrawBatch>,
    /// Exact PVS-signature sub-batches. They reference subranges of the same
    /// vertex buffer as `batches`, so FULL/AUTO do not duplicate world vertices.
    pub pvs_batches: Vec<DrawBatch>,
    /// AUTO 4's immutable per-cluster draw recipes over `pvs_batches`,
    /// generated on the map-load worker. The renderer only materializes their
    /// physical index ranges (lazily) and uploads GPU resources.
    pub portal_draw_plan: PreparedPortalDrawPlan,
    /// Trigger / clip brush overlay geometry (JKA coordinates), built once at
    /// load and only uploaded when a debug overlay is enabled.
    pub debug_volumes: Arc<jka_assets::bsp::DebugVolumes>,
    /// Inline BSP model (`*N`) render vertices at their compiled positions.
    /// Kept apart from `vertices` so static AO/GI/snow never see geometry that
    /// CGame moves at runtime.
    pub inline_vertices: Vec<GpuVertex>,
    /// Material batches for inline models; `vertices` index `inline_vertices`.
    /// They carry no PVS signature: like OpenJK bmodels, their visibility is
    /// decided by the entity being in the snapshot, not by the world clusters.
    pub inline_batches: Vec<DrawBatch>,
    pub inline_models: Vec<InlineModelGeometry>,
    pub textures: Vec<TextureData>,
    /// `videoMap` cinematics streamed into entries of `textures`.
    pub videos: Vec<materials::VideoSource>,
    pub footprint_mark_textures: [Option<usize>; 2],
    pub footprint_mark_blend_modes: [u8; 2],
    pub lightmaps: Vec<TextureData>,
    /// Directional q3map2/Rend2 companion for each effective lightmap page.
    /// Missing pages are represented by a neutral direction texture so runtime
    /// shaders can fall back to the BSP lightgrid without changing bindings.
    pub deluxemaps: Vec<TextureData>,
    /// Metadata only; cached AO generation itself is deferred to a dedicated
    /// runtime worker and therefore never blocks map preparation when disabled.
    pub static_bsp_ao_cache: Option<StaticBspAoCacheInfo>,
    /// Static solid/terrain acoustic scene, generated only when s_steamAudio is
    /// enabled. This is deliberately brush-derived instead of renderer-derived
    /// so nodraw/caulk walls remain acoustically solid.
    pub steam_audio_acoustic_mesh: Option<Arc<jka_assets::bsp::AcousticMesh>>,
    /// Validated serialized Steam Audio scene + probe batch, when this map has
    /// already been baked and cached.
    pub steam_audio_bake: Option<Arc<crate::steam_audio::SteamAudioBakeData>>,
    /// First-load cache misses are baked after MapPrepared is delivered. This
    /// keeps an offline-quality bake from delaying entry into a live server.
    pub steam_audio_bake_request: Option<crate::steam_audio::SteamAudioBakeRequest>,
    pub visibility: Option<Visibility>,
    pub lights: Vec<DynamicLight>,
    /// Global q3map2 source-.map lighting controls. Compiled BSPs keep zeroes.
    pub source_map_lighting: SourceMapLighting,
    pub sun: Option<DirectionalSun>,
    /// Static directional lighting derived from the JKA RBSP lightgrid.
    pub static_light_grid: Option<StaticLightGrid>,
    /// Runtime voxel/probe indirect-light field generated from world geometry
    /// and point/emissive sources during background map preparation.
    pub voxel_probe_gi: Option<VoxelProbeGi>,
    pub reflection_probes: Vec<ReflectionProbe>,
    /// Per-blade grass generated once from BSP surfaces authored with
    /// `surfaceSprites vertical`. Runtime rendering only culls and instances it.
    pub grass_patches: Vec<GrassPatch>,
    /// Compact local BSP fog parameters indexed by the per-blade fog slot.
    /// Slot 0 is always none; slots 1..=255 are map-local brush fogs only.
    pub grass_local_fogs: Vec<[f32; 4]>,
    /// Retail JKA `surfaceSprites effect` emitters. The authored stage texture
    /// is expanded into transient QuickSprite quads every frame instead of being
    /// mapped across the BSP surface.
    pub surface_sprite_effects: Vec<SurfaceSpriteEffectEmitter>,
    /// OpenJK-style global fog encoded by a BSP fog record whose brush number is -1.
    /// The color is also the scene clear color even when the optional fog effect is disabled.
    pub global_fog: Option<[f32; 4]>,
    pub warnings: Vec<String>,
    pub spawns: Vec<SpawnPoint>,
    /// Map-authored `fx_runner`s retained for the local server shim. Remote
    /// servers provide the equivalent state as CS_EFFECTS + ET_FX snapshots.
    pub fx_runners: Vec<MapFxRunner>,
    /// Entities that own inline BSP models, for the local server shim.
    pub brush_entities: Vec<MapBrushEntity>,
    /// Entity blueprint (entities, target links, floor plan) for the
    /// `entities` overlay. Shared so the restart cache clone stays cheap.
    pub entity_graph: Option<Arc<crate::entity_graph::EntityGraph>>,
    /// Authored worldspawn far-plane / visibility distance in JKA map units.
    /// `None` means the map did not provide a usable distancecull-style key.
    pub distance_cull: Option<f32>,
    /// `misc_skyportal` camera plus its optional `misc_skyportal_orient`.
    pub sky_portal: Option<SkyPortal>,
    pub triangles: usize,
    pub lightmap_pages: usize,
    pub source: PathBuf,
    pub map_file_stats: Option<MapFileStats>,
    pub bsp_stats: Option<BspMapStats>,
    pub load_timings: MapLoadTimings,
    pub material_debug: MaterialDebugInfo,
    /// Content hash of the BSP this was prepared from (0 for source `.map` worlds).
    pub map_hash: u64,
    /// Inputs of the reusable stages, for the next preparation of this map.
    pub stage_keys: PrepStageKeys,
    /// Per-texture decode hashes aligned with `textures` (0 = not a plain decode).
    pub texture_hashes: Vec<u64>,
}

const LEGACY_FOOTPRINT_SHADERS: [&str; 2] = ["footstep_l", "footstep_r"];

fn legacy_footprint_blend_mode(blend: &str) -> u8 {
    match blend.trim().to_ascii_lowercase().as_str() {
        "filter" | "gl_dst_color gl_zero" => 1,
        "gl_zero gl_one_minus_src_color" => 2,
        "add" | "gl_one gl_one" => 3,
        _ => 0,
    }
}

fn load_legacy_footprint_marks(
    library: &BTreeMap<String, Shader>,
    assets: &mut AssetSearchPath,
    textures: &mut Textures,
) -> ([Option<usize>; 2], [u8; 2]) {
    let mut indices = [None; 2];
    let mut modes = [0; 2];
    for (slot, name) in LEGACY_FOOTPRINT_SHADERS.iter().enumerate() {
        let Some(shader) = library.get(*name) else {
            indices[slot] = textures.load(assets, name, true);
            continue;
        };
        let Some(stage) = shader
            .primary()
            .filter(|stage| !stage.image.starts_with('$'))
        else {
            continue;
        };
        indices[slot] = textures.load(assets, &stage.image, stage.clamp);
        modes[slot] = legacy_footprint_blend_mode(&stage.blend);
    }
    (indices, modes)
}

#[derive(Debug, Clone, Default)]
pub struct MaterialDebugInfo {
    pub shader_files: usize,
    pub mtr_files: usize,
    pub shader_definitions: usize,
    pub mtr_definitions: usize,
    pub entries: Vec<MaterialDebugEntry>,
}

#[derive(Debug, Clone)]
pub struct MaterialDebugEntry {
    pub name: String,
    /// Logical `.shader` / `.mtr` qpath containing this material definition.
    pub definition_file: Option<String>,
    /// Physical PK3/ZIP/loose provider that supplied `definition_file`.
    pub definition_source: Option<PathBuf>,
    pub source: String,
    pub mtr_override: bool,
    /// Canonical shader-script block retained from the parsed .shader/.mtr.
    pub definition_text: Option<String>,
    pub enhanced: bool,
    /// Resolved stage/base/PBR image providers used by this material.
    pub image_sources: Vec<String>,
    pub lines: Vec<String>,
}

#[cfg(test)]
#[path = "scene/tests/tests.rs"]
mod tests;
