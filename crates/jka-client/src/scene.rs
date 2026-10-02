//! RBSP -> immutable render data. This runs once when a map is loaded.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DrawClass {
    Sky,
    Opaque,
    Mask,
    Transparent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlendMode {
    Opaque,
    Custom(BlendFactor, BlendFactor),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
        if let Some(index) = spawns.iter().position(|spawn| usable(spawn) && spawn.initial) {
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
            (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2], index)
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
            let assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
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
            let assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
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
    pub fn from_q3_angles(
        color: [f32; 3],
        intensity: f32,
        azimuth: f32,
        elevation: f32,
    ) -> Self {
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
        let azimuth = toward_sun.y.atan2(toward_sun.x).to_degrees().rem_euclid(360.0);
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
        let sun_chroma_len = sun_color.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
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
            let alignment: f32 = (0..3).map(|axis| cell.direction[axis] * toward_sun[axis]).sum();
            let chroma: f32 = (0..3)
                .map(|axis| cell.directed[axis] / directed_len * sun_chroma[axis])
                .sum();
            cell.sun_weight = smoothstep_range(0.6, 0.95, alignment)
                * smoothstep_range(0.85, 0.98, chroma);
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
        let base_index = base[0] as usize
            + bx * (base[1] as usize + by * base[2] as usize);

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

        let length_sq = direction.into_iter().map(|value| value * value).sum::<f32>();
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
    pub enhanced: bool,
    /// Resolved stage/base/PBR image providers used by this material.
    pub image_sources: Vec<String>,
    pub lines: Vec<String>,
}

#[derive(Default)]
struct Geometry {
    /// Exact PVS pieces inside one coarse material/lightmap batch, keyed by
    /// camera-cluster visibility and area membership. Pieces are laid out
    /// contiguously so FULL and AUTO 4 reference subranges of the coarse range
    /// without duplicating world vertices.
    by_pvs_signature: BTreeMap<(Vec<u64>, [u64; 4]), WorldGeometryChunk>,
}

#[derive(Default)]
struct WorldGeometryChunk {
    vertices: Vec<GpuVertex>,
    surface_ids: Vec<u32>,
}


#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GrassPatchKey {
    cell_x: i32,
    cell_z: i32,
    pvs_signature: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GrassWorkerPatchKey {
    cell_x: i32,
    cell_z: i32,
    signature_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    class: DrawClass,
    shader: usize,
    lightmap: Option<usize>,
    vertex_lit: bool,
    color_slot: u8,
    fog_num: i32,
    transparent_order: usize,
    planar_group: Option<PlanarGroupKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PlanarGroupKey {
    normal_x: i32,
    normal_y: i32,
    normal_z: i32,
    distance: i32,
}

/// Markable-surface index for a source-.map world, which has no BSP surface
/// table to read shader flags from: every opaque/mask render triangle that faces
/// up enough is treated like an `SF_FACE`, as the old renderer-side gather did.
fn mark_surfaces_from_batches(vertices: &[GpuVertex], batches: &[DrawBatch]) -> Arc<jka_assets::bsp::MarkSurfaces> {
    let mut triangles = Vec::new();
    for batch in batches {
        if !matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask) {
            continue;
        }
        let start = batch.vertices.start as usize;
        let end = (batch.vertices.end as usize).min(vertices.len());
        if start >= end {
            continue;
        }
        for triangle in vertices[start..end].chunks_exact(3) {
            let normal = Vec3::from_array(triangle[0].normal)
                + Vec3::from_array(triangle[1].normal)
                + Vec3::from_array(triangle[2].normal);
            let normal = normal.normalize_or_zero();
            if normal == Vec3::ZERO {
                continue;
            }
            triangles.push((
                [triangle[0].position, triangle[1].position, triangle[2].position].map(jka_position),
                jka_position(normal.to_array()),
            ));
        }
    }
    Arc::new(jka_assets::bsp::MarkSurfaces::from_world_triangles(triangles))
}

pub fn render_position([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, z, -y]
}

pub fn jka_position([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, -z, y]
}

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

fn bsp_physics_collision_mesh(
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
            if valid && compact[0] != compact[1] && compact[1] != compact[2] && compact[2] != compact[0] {
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

/// Whether rain can pool on a surface with these shader contents/flags.
/// `MATERIAL_*` ids come from JKA `surfaceflags.h` (see `jka_assets::bsp`).
fn surface_holds_puddles(contents: u32, surface_flags: u32) -> bool {
    const CONTENTS_LAVA: u32 = 0x0000_0002;
    const CONTENTS_WATER: u32 = 0x0000_0004;
    const ABSORBENT_MATERIALS: [u32; 13] = [
        5,  // short grass
        6,  // long grass
        8,  // sand
        9,  // gravel
        13, // water
        14, // snow
        15, // ice
        16, // flesh
        19, // dry leaves
        20, // green leaves
        21, // fabric
        22, // canvas
        27, // carpet
    ];
    if contents & (CONTENTS_LAVA | CONTENTS_WATER) != 0 {
        return false;
    }
    let material = surface_flags & jka_assets::bsp::MATERIAL_MASK;
    !ABSORBENT_MATERIALS.contains(&material)
}

fn bsp_weather_occlusion_source(
    bsp: &Bsp,
    mesh: &jka_assets::bsp::Mesh,
) -> Option<WeatherOcclusionSource> {
    const CONTENTS_SOLID: u32 = 0x0000_0001;
    const CONTENTS_TERRAIN: u32 = 0x0000_1000;
    const SURF_SKY: u32 = 0x0000_2000;
    const SURF_NODRAW: u32 = 0x0020_0000;
    let world = bsp.models.first()?;
    let mut brushes = Vec::new();
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for brush_index in world.brushes.clone() {
        let Some(brush) = bsp.brushes.get(brush_index) else {
            continue;
        };
        let Some(brush_shader) = bsp.shaders.get(brush.shader) else {
            continue;
        };
        if brush_shader.contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) == 0 {
            continue;
        }
        let mut sides = Vec::with_capacity(brush.sides.len());
        let mut has_sky_side = false;
        for side_index in brush.sides.clone() {
            let Some(side) = bsp.brush_sides.get(side_index) else {
                continue;
            };
            let Some(plane) = bsp.planes.get(side.plane) else {
                continue;
            };
            let surface_flags = bsp
                .shaders
                .get(side.shader)
                .map_or(0, |shader| shader.surface_flags);
            has_sky_side |= surface_flags & SURF_SKY != 0;
            sides.push(WeatherOcclusionSide {
                normal: plane.normal,
                distance: plane.distance,
            });
        }
        if has_sky_side || sides.len() < 6 {
            continue;
        }
        let brush_min_x = -sides[0].distance;
        let brush_max_x = sides[1].distance;
        let brush_min_y = -sides[2].distance;
        let brush_max_y = sides[3].distance;
        if ![brush_min_x, brush_max_x, brush_min_y, brush_max_y]
            .into_iter()
            .all(f32::is_finite)
            || brush_min_x >= brush_max_x
            || brush_min_y >= brush_max_y
        {
            continue;
        }
        min_x = min_x.min(brush_min_x);
        min_y = min_y.min(brush_min_y);
        max_x = max_x.max(brush_max_x);
        max_y = max_y.max(brush_max_y);
        brushes.push(WeatherOcclusionBrush {
            mins_xy: [brush_min_x, brush_min_y],
            maxs_xy: [brush_max_x, brush_max_y],
            sides,
        });
    }

    let mut triangles = Vec::new();
    let mut topography_triangles = Vec::new();
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
        if shader.surface_flags & (SURF_SKY | SURF_NODRAW) != 0 {
            continue;
        }
        let blocks_rain = shader.contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) != 0;
        let holds_puddles = surface_holds_puddles(shader.contents, shader.surface_flags);
        for indices in mesh.indices[batch.indices.clone()].chunks_exact(3) {
            let (Some(a), Some(b), Some(c)) = (
                mesh.vertices.get(indices[0] as usize).map(|v| v.position),
                mesh.vertices.get(indices[1] as usize).map(|v| v.position),
                mesh.vertices.get(indices[2] as usize).map(|v| v.position),
            ) else {
                continue;
            };
            let ab = Vec3::from_array(b) - Vec3::from_array(a);
            let ac = Vec3::from_array(c) - Vec3::from_array(a);
            // Vertical/degenerate triangles have no XZ footprint for standing water
            // and cannot block vertically falling precipitation in the heightfield.
            if ab.cross(ac).z.abs() <= 1.0e-4 {
                continue;
            }
            for point in [a, b, c] {
                min_x = min_x.min(point[0]);
                min_y = min_y.min(point[1]);
                max_x = max_x.max(point[0]);
                max_y = max_y.max(point[1]);
            }
            let triangle = WeatherOcclusionTriangle {
                positions: [a, b, c],
                holds_puddles,
            };
            topography_triangles.push(triangle);
            if blocks_rain && matches!(surface.kind, SurfaceKind::Patch | SurfaceKind::Triangles) {
                // Brush solids already provide their ordinary planar blocker faces.
                // Keep tessellated patches/triangle soups as blocker supplements.
                triangles.push(triangle);
            }
        }
    }
    if (brushes.is_empty() && triangles.is_empty() && topography_triangles.is_empty())
        || ![min_x, min_y, max_x, max_y].into_iter().all(f32::is_finite)
    {
        return None;
    }
    Some(WeatherOcclusionSource {
        min_xz: [min_x, -max_y],
        max_xz: [max_x, -min_y],
        brushes,
        triangles,
        topography_triangles,
    })
}

// 2Retr0/GodotGrass uses 5 m tiles and 10 blades per meter at density 1.0.
// JKA map units are treated as 64 units / Godot meter in grass.rs. The common
// stock Yavin grass sprite density is 42; preserve authored relative density
// while calibrating that value to the GodotGrass demo's full-density spacing.
const GRASS_PATCH_SIZE: f32 = 5.0 * 64.0;
const GRASS_REFERENCE_SPRITE_DENSITY: f32 = 42.0;
const GRASS_FULL_DENSITY_SPACING: f32 = 64.0 / 10.0;
const GRASS_MIN_SPACING: f32 = 3.2;
const GRASS_MAX_SPACING: f32 = 96.0;
const GRASS_MAX_BLADES_PER_TRIANGLE: usize = 32768;

fn grass_hash(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn grass_random(seed: u32, salt: u32) -> f32 {
    let value = grass_hash(seed ^ salt);
    (value as f32) / (u32::MAX as f32)
}

fn srgb_u8_to_linear(value: u8) -> f32 {
    let value = f32::from(value) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn sample_rgba_srgb_bilinear(rgba: &[u8], width: u32, height: u32, uv: [f32; 2]) -> [f32; 3] {
    if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
        return [1.0; 3];
    }
    let x = uv[0].clamp(0.0, 1.0) * (width.saturating_sub(1)) as f32;
    let y = uv[1].clamp(0.0, 1.0) * (height.saturating_sub(1)) as f32;
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width as usize - 1);
    let y1 = (y0 + 1).min(height as usize - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let sample = |sx: usize, sy: usize| {
        let offset = (sy * width as usize + sx) * 4;
        [
            srgb_u8_to_linear(rgba[offset]),
            srgb_u8_to_linear(rgba[offset + 1]),
            srgb_u8_to_linear(rgba[offset + 2]),
        ]
    };
    let a = sample(x0, y0);
    let b = sample(x1, y0);
    let c = sample(x0, y1);
    let d = sample(x1, y1);
    let mut out = [0.0; 3];
    for channel in 0..3 {
        let top = a[channel] + (b[channel] - a[channel]) * tx;
        let bottom = c[channel] + (d[channel] - c[channel]) * tx;
        out[channel] = top + (bottom - top) * ty;
    }
    out
}

fn sample_embedded_lightmap_bilinear(
    bytes: &[u8],
    page: usize,
    uv: [f32; 2],
) -> Option<[f32; 3]> {
    const PAGE: usize = 128;
    const BYTES_PER_PAGE: usize = PAGE * PAGE * 3;
    let page_start = page.checked_mul(BYTES_PER_PAGE)?;
    let data = bytes.get(page_start..page_start + BYTES_PER_PAGE)?;
    let x = uv[0].clamp(0.0, 1.0) * (PAGE - 1) as f32;
    let y = uv[1].clamp(0.0, 1.0) * (PAGE - 1) as f32;
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(PAGE - 1);
    let y1 = (y0 + 1).min(PAGE - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let sample = |sx: usize, sy: usize| {
        let offset = (sy * PAGE + sx) * 3;
        [
            srgb_u8_to_linear(data[offset]),
            srgb_u8_to_linear(data[offset + 1]),
            srgb_u8_to_linear(data[offset + 2]),
        ]
    };
    let a = sample(x0, y0);
    let b = sample(x1, y0);
    let c = sample(x0, y1);
    let d = sample(x1, y1);
    let mut out = [0.0; 3];
    for channel in 0..3 {
        let top = a[channel] + (b[channel] - a[channel]) * tx;
        let bottom = c[channel] + (d[channel] - c[channel]) * tx;
        out[channel] = top + (bottom - top) * ty;
    }
    Some(out)
}

fn grass_ground_albedo_tint(material: &SurfaceMaterial, textures: &Textures) -> [u8; 4] {
    // Analyze the opaque/base image once while preparing the map. Hardware sRGB
    // sampling yields linear values in the world shader, so average in linear space
    // here too. Limit the work to ~4k texels regardless of source resolution.
    let stage = material
        .stages
        .iter()
        .find(|stage| stage.blend.is_none() && matches!(stage.texture, StageTexture::Image(_)))
        .or_else(|| {
            material
                .stages
                .iter()
                .find(|stage| matches!(stage.texture, StageTexture::Image(_)))
        });
    let Some(stage) = stage else {
        return [0, 0, 0, 0];
    };
    let StageTexture::Image(index) = stage.texture else {
        return [0, 0, 0, 0];
    };
    let Some(image) = textures.images.get(index) else {
        return [0, 0, 0, 0];
    };
    let pixel_count = (image.width as usize).saturating_mul(image.height as usize);
    let base_bytes = pixel_count.saturating_mul(4).min(image.rgba.len());
    if pixel_count == 0 || base_bytes < 4 {
        return [0, 0, 0, 0];
    }
    let stride = ((pixel_count as f64 / 4096.0).sqrt().ceil() as usize).max(1);
    let mut sum = [0.0_f64; 3];
    let mut samples = 0_u64;
    for y in (0..image.height as usize).step_by(stride) {
        for x in (0..image.width as usize).step_by(stride) {
            let offset = (y * image.width as usize + x) * 4;
            if offset + 3 >= base_bytes || image.rgba[offset + 3] < 16 {
                continue;
            }
            for channel in 0..3 {
                let raw = image.rgba[offset + channel];
                let value = if image.srgb {
                    srgb_u8_to_linear(raw)
                } else {
                    f32::from(raw) / 255.0
                };
                sum[channel] += f64::from(value * stage.color[channel]);
            }
            samples += 1;
        }
    }
    if samples == 0 {
        return [0, 0, 0, 0];
    }
    let inv = 1.0 / samples as f64;
    [
        ((sum[0] * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        ((sum[1] * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        ((sum[2] * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        255,
    ]
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceSpriteEffectTriangle {
    /// Original JKA coordinates are retained because Raven seeds/distributes
    /// effect sprites from the triangle's authored X/Y/Z values.
    pub positions_jka: [[f32; 3]; 3],
    pub normal_z: [f32; 3],
    /// OpenJK's effect path uses the blue byte of the active vertex-color style
    /// as its scalar baked-light value.
    pub light: [u8; 3],
}

#[derive(Debug, Clone)]
pub struct SurfaceSpriteEffectEmitter {
    pub texture: usize,
    pub clamp: bool,
    pub blend: Option<materials::BlendFunc>,
    pub sprite: jka_assets::shader::SurfaceSprite,
    pub triangles: Vec<SurfaceSpriteEffectTriangle>,
    /// Same camera-cluster visibility signature used by ordinary BSP batches.
    pub pvs_signature: Vec<u64>,
}

fn collect_surface_sprite_effect_emitters(
    bsp: &Bsp,
    mesh: &jka_assets::bsp::Mesh,
    surface_materials: &[SurfaceMaterial],
) -> Vec<SurfaceSpriteEffectEmitter> {
    let mut emitters = Vec::new();
    for batch in &mesh.batches {
        let Some(material) = surface_materials.get(batch.shader) else {
            continue;
        };
        if material.hidden || material.surface_sprite_effects.is_empty() {
            continue;
        }
        let Some(surface) = bsp.surfaces.get(batch.surface) else {
            continue;
        };
        let lightmap_slot =
            (0..4).find(|&i| batch.lightmaps[i] >= 0 && batch.lightmap_styles[i] < 254);
        let vertex_lit_slot = (0..4)
            .find(|&i| batch.lightmaps[i] == LIGHTMAP_BY_VERTEX && surface.vertex_styles[i] < 254);
        let color_slot = vertex_lit_slot
            .or(lightmap_slot)
            .or_else(|| (0..4).find(|&i| surface.vertex_styles[i] < 254))
            .unwrap_or(0);
        let pvs_signature = bsp
            .visibility
            .as_ref()
            .map(|vis| pvs_signature(vis, &vis.surface_clusters[batch.surface]))
            .unwrap_or_default();
        let mut triangles = Vec::new();
        for triangle in mesh.indices[batch.indices.clone()].as_chunks::<3>().0 {
            // Keep original JKA index order here. Render geometry reverses the
            // winding when converting coordinate handedness, but Raven's random
            // interval uses v1.x + v2.y + v3.z and therefore depends on order.
            let source = triangle.map(|index| mesh.vertices[index as usize]);
            triangles.push(SurfaceSpriteEffectTriangle {
                positions_jka: source.map(|vertex| vertex.position),
                normal_z: source.map(|vertex| vertex.normal[2]),
                light: source.map(|vertex| vertex.color[color_slot][2]),
            });
        }
        if triangles.is_empty() {
            continue;
        }
        for effect in &material.surface_sprite_effects {
            emitters.push(SurfaceSpriteEffectEmitter {
                texture: effect.texture,
                clamp: effect.clamp,
                blend: effect.blend,
                sprite: effect.sprite,
                triangles: triangles.clone(),
                pvs_signature: pvs_signature.clone(),
            });
        }
    }
    emitters
}

#[derive(Clone)]
struct GrassExternalLightmap {
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
}

#[derive(Clone)]
struct GrassLightmapSources {
    embedded: Arc<[u8]>,
    external: Arc<BTreeMap<usize, GrassExternalLightmap>>,
}

#[derive(Clone)]
struct GrassEmitterTriangle {
    positions: [[f32; 3]; 3],
    colors: [[u8; 4]; 3],
    lightmap_uv: Option<[[f32; 2]; 3]>,
    lightmap_page: Option<usize>,
    signature_id: u32,
    triangle_seed: u32,
    height: f32,
    density: f32,
    ground_tint_rgba: [u8; 4],
}

fn collect_grass_emitters(
    bsp: &Bsp,
    mesh: &jka_assets::bsp::Mesh,
    surface_materials: &[SurfaceMaterial],
    grass_ground_tints: &[[u8; 4]],
) -> (Vec<GrassEmitterTriangle>, Vec<Vec<u64>>) {
    let mut emitters = Vec::new();
    let mut signature_ids = BTreeMap::<Vec<u64>, u32>::new();
    let mut signatures = Vec::<Vec<u64>>::new();
    for batch in &mesh.batches {
        let Some(material) = surface_materials.get(batch.shader) else {
            continue;
        };
        let Some(grass) = material.grass else {
            continue;
        };
        if material.hidden {
            continue;
        }
        let Some(surface) = bsp.surfaces.get(batch.surface) else {
            continue;
        };
        let lightmap_slot =
            (0..4).find(|&i| batch.lightmaps[i] >= 0 && batch.lightmap_styles[i] < 254);
        let vertex_lit_slot = (0..4)
            .find(|&i| batch.lightmaps[i] == LIGHTMAP_BY_VERTEX && surface.vertex_styles[i] < 254);
        let color_slot = vertex_lit_slot
            .or(lightmap_slot)
            .or_else(|| (0..4).find(|&i| surface.vertex_styles[i] < 254))
            .unwrap_or(0);
        let lightmap_page = lightmap_slot
            .filter(|_| !material.sky)
            .map(|slot| batch.lightmaps[slot] as usize);
        let signature = bsp
            .visibility
            .as_ref()
            .map(|vis| pvs_signature(vis, &vis.surface_clusters[batch.surface]))
            .unwrap_or_default();
        let signature_id = if let Some(&id) = signature_ids.get(&signature) {
            id
        } else {
            let id = signatures.len() as u32;
            signature_ids.insert(signature.clone(), id);
            signatures.push(signature);
            id
        };
        let ground_tint_rgba = grass_ground_tints
            .get(batch.shader)
            .copied()
            .unwrap_or([0, 0, 0, 0]);

        for (triangle_index, triangle) in mesh.indices[batch.indices.clone()]
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
        {
            let source = [0usize, 2, 1].map(|corner| mesh.vertices[triangle[corner] as usize]);
            let positions = source.map(|vertex| render_position(vertex.position));
            let p = positions.map(Vec3::from_array);
            let cross = (p[1] - p[0]).cross(p[2] - p[0]);
            let double_area = cross.length();
            if double_area <= 1.0e-4 {
                continue;
            }
            let authored_normal = source
                .iter()
                .map(|vertex| Vec3::from_array(render_position(vertex.normal)))
                .fold(Vec3::ZERO, |sum, normal| sum + normal)
                .normalize_or_zero();
            let mut normal = cross / double_area;
            if authored_normal.length_squared() > 0.25 {
                normal = authored_normal;
            }
            if !grass.any_angle && normal.y < 0.5 {
                continue;
            }
            let colors = source.map(|vertex| vertex.color[color_slot]);
            let lightmap_uv = lightmap_slot.map(|slot| source.map(|vertex| vertex.lightmap_uv[slot]));
            let triangle_seed = grass_hash(
                (batch.surface as u32).wrapping_mul(0x9e37_79b9)
                    ^ (triangle_index as u32).wrapping_mul(0x85eb_ca6b)
                    ^ (batch.shader as u32).wrapping_mul(0xc2b2_ae35),
            );
            emitters.push(GrassEmitterTriangle {
                positions,
                colors,
                lightmap_uv,
                lightmap_page,
                signature_id,
                triangle_seed,
                height: grass.height.max(1.0),
                density: grass.density,
                ground_tint_rgba,
            });
        }
    }
    (emitters, signatures)
}

/// Fingerprint of everything blade generation reads: each emitter triangle, the
/// PVS signatures its `signature_id`s resolve to, and the sampled lightmap pixels.
fn grass_fingerprint(
    emitters: &[GrassEmitterTriangle],
    signatures: &[Vec<u64>],
    lightmaps: &GrassLightmapSources,
) -> u64 {
    let mut fingerprint = StageFingerprint::new("grass");
    fingerprint.pod(&[emitters.len() as u64]);
    for emitter in emitters {
        fingerprint
            .pod(&emitter.positions)
            .pod(&emitter.colors)
            .pod(&[
                emitter.signature_id,
                emitter.triangle_seed,
                emitter.height.to_bits(),
                emitter.density.to_bits(),
                u32::from_le_bytes(emitter.ground_tint_rgba),
            ])
            .debug(&emitter.lightmap_uv)
            .debug(&emitter.lightmap_page);
    }
    for signature in signatures {
        fingerprint.pod(signature);
    }
    fingerprint.bytes(&lightmaps.embedded);
    for (page, image) in lightmaps.external.iter() {
        fingerprint
            .pod(&[*page as u64, u64::from(image.width), u64::from(image.height)])
            .bytes(&image.rgba);
    }
    fingerprint.finish()
}

fn grass_lightmap_sources(
    bsp: &Bsp,
    external_lightmaps: &[TextureData],
    external_lightmap_lookup: &BTreeMap<usize, usize>,
) -> GrassLightmapSources {
    let external = external_lightmap_lookup
        .iter()
        .filter_map(|(&page, &index)| {
            let image = external_lightmaps.get(index)?;
            let base_len = (image.width as usize)
                .saturating_mul(image.height as usize)
                .saturating_mul(4)
                .min(image.rgba.len());
            Some((
                page,
                GrassExternalLightmap {
                    width: image.width,
                    height: image.height,
                    rgba: Arc::from(image.rgba[..base_len].to_vec()),
                },
            ))
        })
        .collect();
    GrassLightmapSources {
        embedded: Arc::from(bsp.lightmaps.clone()),
        external: Arc::new(external),
    }
}

fn grass_worker_baked_lighting(
    sources: &GrassLightmapSources,
    page: usize,
    uv: [f32; 2],
) -> Option<[f32; 3]> {
    if let Some(image) = sources.external.get(&page) {
        return Some(sample_rgba_srgb_bilinear(
            &image.rgba,
            image.width,
            image.height,
            uv,
        ));
    }
    sample_embedded_lightmap_bilinear(&sources.embedded, page, uv)
}

fn generate_grass_chunk(
    emitters: Vec<GrassEmitterTriangle>,
    lightmaps: Arc<GrassLightmapSources>,
    clump_data: Arc<Vec<u8>>,
) -> (HashMap<GrassWorkerPatchKey, Vec<GrassInstance>>, usize) {
    let mut groups = HashMap::<GrassWorkerPatchKey, Vec<GrassInstance>>::new();
    let mut blade_count = 0usize;
    for emitter in emitters {
        let positions = emitter.positions.map(Vec3::from_array);
        let area = (positions[1] - positions[0])
            .cross(positions[2] - positions[0])
            .length()
            * 0.5;
        if area <= 1.0e-4 {
            continue;
        }
        let spacing = (emitter.density.abs() / GRASS_REFERENCE_SPRITE_DENSITY
            * GRASS_FULL_DENSITY_SPACING)
            .clamp(GRASS_MIN_SPACING, GRASS_MAX_SPACING);
        let desired = area / (spacing * spacing);
        let whole = desired.floor() as usize;
        let extra = if whole < GRASS_MAX_BLADES_PER_TRIANGLE
            && grass_random(emitter.triangle_seed, 0xa511_e9b3) < desired.fract()
        {
            1
        } else {
            0
        };
        let count = (whole + extra).min(GRASS_MAX_BLADES_PER_TRIANGLE);
        for blade in 0..count {
            let seed = grass_hash(
                emitter.triangle_seed ^ (blade as u32).wrapping_mul(0x27d4_eb2d),
            );
            let r1 = grass_random(seed, 0x68bc_21eb).sqrt();
            let r2 = grass_random(seed, 0x02e5_be93);
            let bary = [1.0 - r1, r1 * (1.0 - r2), r1 * r2];
            let position = positions[0] * bary[0]
                + positions[1] * bary[1]
                + positions[2] * bary[2];
            let mut color = [0.0_f32; 3];
            for corner in 0..3 {
                for channel in 0..3 {
                    color[channel] +=
                        f32::from(emitter.colors[corner][channel]) / 255.0 * bary[corner];
                }
            }
            if let (Some(page), Some(uvs)) = (emitter.lightmap_page, emitter.lightmap_uv) {
                let raw_uv = [0usize, 1usize].map(|axis| {
                    uvs[0][axis] * bary[0] + uvs[1][axis] * bary[1] + uvs[2][axis] * bary[2]
                });
                if let Some(baked) = grass_worker_baked_lighting(&lightmaps, page, raw_uv) {
                    color = baked;
                }
            }
            let cell_x = (position.x / GRASS_PATCH_SIZE).floor() as i32;
            let cell_z = (position.z / GRASS_PATCH_SIZE).floor() as i32;
            groups
                .entry(GrassWorkerPatchKey {
                    cell_x,
                    cell_z,
                    signature_id: emitter.signature_id,
                })
                .or_default()
                .push({
                    let packed_clump = crate::grass::pack_static_clump_alpha(
                        position.to_array(),
                        &clump_data,
                        emitter.ground_tint_rgba[3] >= 128,
                    );
                    let baked_light_rgba = [
                        (color[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        (color[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        (color[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        packed_clump as u8,
                    ];
                    let mut ground_tint_rgba = emitter.ground_tint_rgba;
                    ground_tint_rgba[3] = (packed_clump >> 8) as u8;
                    GrassInstance {
                        position: position.to_array(),
                        height: emitter.height,
                        baked_light_rgba,
                        ground_tint_rgba,
                    }
                });
            blade_count += 1;
        }
    }
    (groups, blade_count)
}

fn grass_instance_lod_key(instance: &GrassInstance) -> u32 {
    let mut value = instance.position[0].to_bits().wrapping_mul(0x9e37_79b9)
        ^ instance.position[1].to_bits().rotate_left(11)
        ^ instance.position[2].to_bits().wrapping_mul(0x85eb_ca6b);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn finish_grass_patch(key: GrassPatchKey, mut instances: Vec<GrassInstance>) -> Option<GrassPatch> {
    if instances.is_empty() {
        return None;
    }
    // Sorting once on the map workers means renderer upload can copy the compact
    // 24-byte records directly instead of re-sorting ~29M blades on the render thread.
    instances.sort_unstable_by_key(grass_instance_lod_key);
    let mut minimum = Vec3::splat(f32::INFINITY);
    let mut maximum = Vec3::splat(f32::NEG_INFINITY);
    let mut max_height = 0.0_f32;
    for instance in &instances {
        let position = Vec3::from_array(instance.position);
        minimum = minimum.min(position);
        maximum = maximum.max(position);
        max_height = max_height.max(instance.height);
    }
    maximum.y += max_height * 4.0;
    minimum.y -= max_height * 0.5;
    let center = (minimum + maximum) * 0.5;
    let radius = (maximum - center).length();
    Some(GrassPatch {
        instances,
        pvs_signature: key.pvs_signature,
        center: center.to_array(),
        radius,
    })
}

fn finish_grass_patches(groups: BTreeMap<GrassPatchKey, Vec<GrassInstance>>) -> Vec<GrassPatch> {
    groups
        .into_iter()
        .filter_map(|(key, instances)| finish_grass_patch(key, instances))
        .collect()
}

fn finish_grass_patches_with_jobs(
    groups: BTreeMap<GrassPatchKey, Vec<GrassInstance>>,
    jobs: Option<&MapJobPool>,
) -> Result<(Vec<GrassPatch>, f64), String> {
    let Some(jobs) = jobs.filter(|_| groups.len() > 64) else {
        let started = Instant::now();
        let patches = finish_grass_patches(groups);
        return Ok((patches, started.elapsed().as_secs_f64() * 1000.0));
    };
    let entries = groups.into_iter().collect::<Vec<_>>();
    let worker_count = jobs.worker_count().min(entries.len()).max(1);
    let chunk_size = entries.len().div_ceil(worker_count);
    let mut handles = Vec::new();
    let mut base = 0usize;
    let mut iter = entries.into_iter();
    loop {
        let chunk = iter.by_ref().take(chunk_size).collect::<Vec<_>>();
        if chunk.is_empty() {
            break;
        }
        let chunk_base = base;
        base += chunk.len();
        handles.push(jobs.submit(Task::MapGrass, move || {
            let started = Instant::now();
            let patches = chunk
                .into_iter()
                .enumerate()
                .filter_map(|(offset, (key, instances))| {
                    finish_grass_patch(key, instances).map(|patch| (chunk_base + offset, patch))
                })
                .collect::<Vec<_>>();
            (patches, started.elapsed().as_secs_f64() * 1000.0)
        })?);
    }
    let mut ordered = Vec::<(usize, GrassPatch)>::new();
    let mut cpu_ms = 0.0_f64;
    for handle in handles {
        let (mut patches, worker_ms) = handle.join()?;
        cpu_ms += worker_ms;
        ordered.append(&mut patches);
    }
    ordered.sort_unstable_by_key(|(index, _)| *index);
    Ok((
        ordered.into_iter().map(|(_, patch)| patch).collect(),
        cpu_ms,
    ))
}

/// Renderer-space sky portal camera (cgame `CG_DrawSkyBoxPortal`): the sky view
/// sits at `origin` plus the player's offset from `orient_origin`, scaled by
/// `scale`. Without a `misc_skyportal_orient` the scale is zero, so the sky
/// camera stays fixed at `origin`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyPortal {
    pub origin: [f32; 3],
    pub orient_origin: [f32; 3],
    pub scale: f32,
}

impl SkyPortal {
    pub fn camera_position(&self, view: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|axis| {
            self.origin[axis] + (view[axis] - self.orient_origin[axis]) * self.scale
        })
    }
}

fn bsp_sky_portal(bsp: &Bsp) -> Option<SkyPortal> {
    let mut portal: Option<[f32; 3]> = None;
    let mut orient: Option<([f32; 3], f32)> = None;
    for entity in &bsp.entities {
        let Some(classname) = entity
            .get(b"classname")
            .and_then(|value| std::str::from_utf8(value).ok())
        else {
            continue;
        };
        let origin = entity
            .get(b"origin")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(parse_light_triplet)
            .unwrap_or([0.0; 3]);
        if classname.eq_ignore_ascii_case("misc_skyportal") {
            portal.get_or_insert(origin);
        } else if classname.eq_ignore_ascii_case("misc_skyportal_orient") {
            let scale = entity
                .get(b"modelscale")
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(parse_leading_f32)
                .unwrap_or(0.0);
            orient.get_or_insert((origin, scale));
        }
    }
    let origin = portal?;
    let (orient_origin, scale) = orient.unwrap_or(([0.0; 3], 0.0));
    Some(SkyPortal {
        origin: render_position(origin),
        orient_origin: render_position(orient_origin),
        scale,
    })
}

const DISTANCE_CULL_KEYS: [&str; 4] = ["distancecull", "_distancecull", "_farplanedist", "fogclip"];

fn parse_distance_cull(raw: &str) -> Option<f32> {
    // OpenJK uses sscanf(value, "%f", ...), so it accepts a valid leading
    // float even when mapping tools append a distance-measure suffix (for
    // example `24000r`). Rust's `str::parse::<f32>()` requires the entire
    // string to be numeric, so mirror sscanf's prefix behavior here.
    let value = parse_leading_f32(raw)?;
    (value.is_finite() && value > 1.0).then_some(value)
}

fn parse_leading_f32(raw: &str) -> Option<f32> {
    let text = raw.trim_start();
    if text.is_empty() {
        return None;
    }
    for end in (1..=text.len()).rev() {
        if !text.is_char_boundary(end) {
            continue;
        }
        if let Ok(value) = text[..end].trim_end().parse::<f32>() {
            return Some(value);
        }
    }
    None
}

fn bsp_worldspawn_distance_cull(bsp: &Bsp, warnings: &mut Vec<String>) -> Option<f32> {
    let Some(world) = bsp.entities.iter().find(|entity| {
        entity.properties.iter().rev().any(|(key, value)| {
            key.eq_ignore_ascii_case(b"classname") && value.eq_ignore_ascii_case(b"worldspawn")
        })
    }) else {
        return None;
    };

    for wanted in DISTANCE_CULL_KEYS {
        if let Some((key, raw)) = world
            .properties
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case(wanted.as_bytes()))
        {
            let text = String::from_utf8_lossy(raw);
            if let Some(value) = parse_distance_cull(&text) {
                return Some(value);
            }
            warnings.push(format!(
                "worldspawn {} value {:?} is not a usable positive distance; using the client default far plane",
                String::from_utf8_lossy(key),
                text
            ));
            return None;
        }
    }
    None
}

fn map_worldspawn_distance_cull(
    world: &jka_assets::map::MapEntity,
    warnings: &mut Vec<String>,
) -> Option<f32> {
    for wanted in DISTANCE_CULL_KEYS {
        if let Some((key, raw)) = world
            .properties
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case(wanted))
        {
            if let Some(value) = parse_distance_cull(raw) {
                return Some(value);
            }
            warnings.push(format!(
                "worldspawn {key} value {raw:?} is not a usable positive distance; using the client default far plane"
            ));
            return None;
        }
    }
    None
}

fn render_sun(sun: ShaderSun) -> DirectionalSun {
    DirectionalSun::from_q3_angles(sun.color, sun.intensity, sun.azimuth, sun.elevation)
}

fn authored_sun<'a>(
    shader_names: impl Iterator<Item = &'a str>,
    library: &BTreeMap<String, Shader>,
    warnings: &mut Vec<String>,
) -> Option<DirectionalSun> {
    let mut strongest: Option<(&str, ShaderSun)> = None;
    let mut count = 0usize;
    for name in shader_names {
        let Some(shader) = library.get(name) else {
            continue;
        };
        if !shader.sky {
            continue;
        }
        for &sun in &shader.suns {
            count += 1;
            if strongest.is_none_or(|(_, current)| sun.intensity > current.intensity) {
                strongest = Some((name, sun));
            }
        }
    }
    let (shader_name, sun) = strongest?;
    if count > 1 {
        warnings.push(format!(
            "{count} authored sky suns are referenced; using strongest sun from {shader_name} ({:.0} intensity)",
            sun.intensity
        ));
    }
    Some(render_sun(sun))
}

fn validate_map_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.split('/').any(|segment| {
            segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
    {
        return Err("Use a map name such as mp/ffa3, optionally followed by .bsp or .map.".into());
    }
    Ok(())
}

fn map_asset_name(name: &str, extension: &str) -> Result<String, String> {
    validate_map_name(name)?;
    Ok(format!("maps/{name}.{extension}"))
}

fn stage_class(stage: &MaterialStage) -> DrawClass {
    if stage.blend.is_some() {
        DrawClass::Transparent
    } else if stage.alpha_cutoff > 0.0 {
        DrawClass::Mask
    } else {
        DrawClass::Opaque
    }
}

fn class_for(material: &SurfaceMaterial) -> DrawClass {
    if material.sky {
        DrawClass::Sky
    } else {
        material
            .stages
            .first()
            .map(stage_class)
            .unwrap_or(DrawClass::Opaque)
    }
}

/// True when the authored alpha-test stage can never survive its own alpha
/// cutoff. Texture alpha is normalized to [0, 1], so a constant alpha below
/// the cutoff remains below it after multiplication by any sampled texel.
///
/// Keep this deliberately narrow: it recognizes a mathematically guaranteed
/// discard, not merely a stage that "looks invisible" for common content.
fn stage_is_guaranteed_alpha_discard(stage: &MaterialStage) -> bool {
    matches!(stage.alpha_gen, AlphaGen::Const)
        && stage.alpha_cutoff > 0.0
        && stage.opacity < stage.alpha_cutoff
}

/// Map-load-only render cull for explicit shader geometry whose ordinary
/// stages are all guaranteed to alpha-discard. The BSP surface itself remains
/// intact so compile/runtime semantics that consume its triangles first (for
/// example q3map_surfacelight extraction) are preserved.
fn material_render_is_guaranteed_discarded(
    material: &SurfaceMaterial,
    vertex_lit: bool,
) -> bool {
    if !material.explicit
        || material.sky
        || material.water
        || material.planar_reflection
        || material.grass.is_some()
        || !material.surface_sprite_effects.is_empty()
        || material.stages.is_empty()
    {
        return false;
    }

    prepared_stages(material, vertex_lit)
        .iter()
        .all(stage_is_guaranteed_alpha_discard)
}

fn blend_for(stage: &MaterialStage) -> BlendMode {
    stage
        .blend
        .map(|blend| BlendMode::Custom(blend.src, blend.dst))
        .unwrap_or(BlendMode::Opaque)
}

fn effective_world_cull(material: &SurfaceMaterial) -> CullMode {
    // Raven's quick-sprite renderer ends a surfaceSprites group by enabling
    // face culling. On a parent shader authored `cull twosided`, that leaves
    // retail JKA effectively one-sided after the sprite path. wgpu pipelines
    // do not have that kind of global mutable raster state, so reproduce the
    // retail-visible result directly and deterministically on the world draw.
    //
    // Do not touch already-one-sided authored modes, and do not touch the
    // procedural grass pipelines themselves (they remain explicitly two-sided).
    if material.surface_sprite_cull_quirk && material.cull == CullMode::None {
        CullMode::Back
    } else {
        material.cull
    }
}

fn stage_pipeline(material: &SurfaceMaterial, stage: &MaterialStage, first: bool) -> PipelineKey {
    PipelineKey {
        class: if material.sky {
            DrawClass::Sky
        } else {
            stage_class(stage)
        },
        blend: blend_for(stage),
        cull: if material.sky {
            CullMode::None
        } else {
            effective_world_cull(material)
        },
        offset: material.offset,
        // OpenJK's default opaque first stage writes depth. Blended later stages
        // do not unless the shader explicitly requests depthWrite.
        depth_write: stage.depth_write || (first && stage.blend.is_none()),
        depth_equal: stage.depth_equal,
    }
}

fn resolved_bsp_surface_fog(
    bsp: &Bsp,
    library: &BTreeMap<String, Shader>,
    material: &SurfaceMaterial,
    fog_num: i32,
    global_fog_num: Option<i32>,
    global_fog: Option<[f32; 4]>,
) -> ([f32; 4], bool) {
    let authored = bsp_fog_params(bsp, library, fog_num);
    if authored[3] > 0.001 {
        return (authored, global_fog_num == Some(fog_num));
    }

    // q3map2 normally writes the default/global fog index into every eligible
    // draw surface. Some legacy/custom BSPs leave fogNum == -1 instead. OpenJK
    // can inherit the world's global fog for BSP-model surfaces; recover the
    // same visual intent here for any missing assignment, but never override
    // q3map_nofog authored by the material. Local brush fog remains strictly
    // driven by the compiled per-surface fog index.
    if fog_num < 0 && !material.no_fog {
        if let Some(global) = global_fog {
            return (global, true);
        }
    }

    (authored, false)
}

/// See `DrawBatch::dlight_in_lightmap_stage`. Materials with any PBR companion
/// keep the base-stage light so their microfacet response is unchanged.
fn material_dlight_in_lightmap_stage(material: &SurfaceMaterial) -> bool {
    material.stages.iter().any(|stage| matches!(stage.texture, StageTexture::Lightmap))
        && !material.stages.iter().any(|stage| {
            let e = &stage.enhancements;
            e.normal_texture.is_some()
                || e.roughness_texture.is_some()
                || e.height_texture.is_some()
                || e.metallic_texture.is_some()
                || e.specular_texture.is_some()
                || e.emissive_texture.is_some()
                || e.roughness_override.is_some()
                || e.specular_reflectance.is_some()
        })
}

fn stage_batch(
    material: &SurfaceMaterial,
    stage: &MaterialStage,
    first: bool,
    vertex_lit: bool,
    vertices: Range<u32>,
    bsp_shader_index: Option<usize>,
    material_debug_index: Option<usize>,
    lightmap: Option<usize>,
    pvs_signature: &[u64],
    area_signature: [u64; 4],
    fog: [f32; 4],
    fog_is_global: bool,
) -> DrawBatch {
    let (texture, texture_is_lightmap, texture_is_white) = match stage.texture {
        StageTexture::Image(index) => (Some(index), false, false),
        StageTexture::Lightmap => (None, true, false),
        StageTexture::White => (None, false, true),
    };
    let pipeline = stage_pipeline(material, stage, first);
    let detail_texture_eligible = !material.authored_detail
        && !material.water
        && !material.planar_reflection
        && matches!(stage.texture, StageTexture::Image(_))
        && matches!(stage.tc_gen, TcGen::Base)
        && stage.tc_mods.is_empty()
        && pipeline.class == DrawClass::Opaque
        && pipeline.blend == BlendMode::Opaque;
    DrawBatch {
        vertices,
        bsp_shader_index: bsp_shader_index.map_or(u32::MAX, |index| index as u32),
        material_debug_index: material_debug_index.map_or(u32::MAX, |index| index as u32),
        surface_material: material.surface_material,
        texture,
        texture_is_lightmap,
        texture_is_white,
        detail_texture_eligible,
        normal_texture: stage.enhancements.normal_texture,
        roughness_texture: stage.enhancements.roughness_texture,
        height_texture: stage.enhancements.height_texture,
        metallic_texture: stage.enhancements.metallic_texture,
        specular_texture: stage.enhancements.specular_texture,
        emissive_texture: stage.enhancements.emissive_texture,
        height_from_alpha: stage.enhancements.height_from_alpha,
        rmo_packed: stage.enhancements.rmo_packed,
        rmo_specular_alpha: stage.enhancements.rmo_specular_alpha,
        normal_scale: stage.enhancements.normal_scale,
        roughness_override: stage.enhancements.roughness_override,
        specular_reflectance: stage.enhancements.specular_reflectance,
        parallax_depth: stage.enhancements.parallax_depth,
        lightmap,
        modulate_lightmap: false,
        dlight_in_lightmap_stage: material_dlight_in_lightmap_stage(material),
        vertex_lit,
        pipeline,
        tc_gen: stage.tc_gen,
        tc_mods: stage.tc_mods.clone(),
        rgb_gen: stage.rgb_gen,
        alpha_gen: stage.alpha_gen,
        color: [
            stage.color[0],
            stage.color[1],
            stage.color[2],
            stage.opacity,
        ],
        alpha_cutoff: stage.alpha_cutoff,
        fog,
        fog_is_global,
        fog_color_override: stage_fog_color_override(stage, first),
        // A sky's stages are cloud layers drawn as extra batches; they must not
        // change the fog eligibility the sky had when it was a single batch.
        legacy2_fog_in_stage_safe: material_legacy2_in_stage_safe(if material.sky {
            &[]
        } else {
            &material.stages
        }),
        global_fog_post_eligible: !material.sky
            && material
                .stages
                .iter()
                .enumerate()
                .any(|(index, candidate)| {
                    stage_pipeline(material, candidate, index == 0).depth_write
                }),
        planar_reflection: material.planar_reflection && first,
        water: material.water,
        alpha_shadow: material.alpha_shadow,
        light_filter: material.light_filter,
        water_primary: material.water && first,
        authored_ocean: None,
        planar_environment_candidate: matches!(stage.tc_gen, TcGen::Environment),
        planar_plane: [0.0; 4],
        planar_pvs_origin: [0.0; 3],
        skybox: material.skybox,
        reflection_probe: None,
        reflection_probe_position_radius: [0.0; 4],
        reflection_cache_flags: 0,
        reflection_roughness_hint: 1.0,
        pvs_signature: pvs_signature.to_vec(),
        area_signature,
    }
}

fn pvs_signature(vis: &Visibility, target_clusters: &[usize]) -> Vec<u64> {
    vis.pvs_signature(target_clusters)
}

fn portal_batch_visible_from_cluster(source: &DrawBatch, cluster: usize) -> bool {
    if source.pvs_signature.is_empty() {
        return true;
    }
    source
        .pvs_signature
        .get(cluster / 64)
        .is_some_and(|word| word & (1_u64 << (cluster % 64)) != 0)
}

/// Pieces AUTO 4 keeps as individual draws: promoted/authored water and planar
/// mirror stages are replaced or special-cased per piece by the renderer.
/// Everything else (including blended and sky stages) collapses within its
/// MINIMAL batch, which preserves MINIMAL's primitive order exactly.
fn auto4_collapsible(source: &DrawBatch) -> bool {
    !source.water
        && !source.water_primary
        && source.authored_ocean.is_none()
        && !source.planar_reflection
        && !source.planar_environment_candidate
}

/// Opaque/alpha-tested pieces are order-free under the depth test, so the
/// audit's ideal may merge them across MINIMAL batches by draw state.
fn auto4_merge_safe(source: &DrawBatch) -> bool {
    matches!(source.pipeline.class, DrawClass::Opaque | DrawClass::Mask) && auto4_collapsible(source)
}

/// For each FULL piece, the MINIMAL batch (same material stage, geometry
/// range containing the piece) that owns it. `None` keeps the piece as an
/// individual draw.
fn auto4_piece_owners(coarse: &[DrawBatch], full: &[DrawBatch]) -> Vec<Option<usize>> {
    // All stages of one coarse geometry share one vertex range and are pushed
    // consecutively, in stage order.
    let mut order = (0..coarse.len()).collect::<Vec<_>>();
    order.sort_by_key(|&index| (coarse[index].vertices.start, index));
    let mut groups = Vec::<(Range<u32>, Vec<usize>)>::new();
    for index in order {
        let range = coarse[index].vertices.clone();
        if range.is_empty() {
            continue;
        }
        match groups.last_mut() {
            Some((last, stages)) if *last == range => stages.push(index),
            _ => groups.push((range, vec![index])),
        }
    }

    let mut owners = vec![None; full.len()];
    let mut stage = 0usize;
    for (index, piece) in full.iter().enumerate() {
        stage = if index > 0 && full[index - 1].vertices == piece.vertices {
            stage + 1
        } else {
            0
        };
        if piece.vertices.is_empty() {
            continue;
        }
        let Some(group) = groups
            .partition_point(|(range, _)| range.start <= piece.vertices.start)
            .checked_sub(1)
        else {
            continue;
        };
        let (range, stages) = &groups[group];
        if piece.vertices.end > range.end {
            continue;
        }
        owners[index] = stages
            .get(stage)
            .copied()
            .filter(|&owner| draw_batches_share_state(&coarse[owner], piece))
            .or_else(|| {
                stages
                    .iter()
                    .copied()
                    .find(|&owner| draw_batches_share_state(&coarse[owner], piece))
            });
    }
    owners
}

fn auto4_source_bounds(source: &DrawBatch, vertices: &[GpuVertex]) -> ([f32; 3], [f32; 3]) {
    let start = usize::try_from(source.vertices.start).unwrap_or(usize::MAX);
    let end = usize::try_from(source.vertices.end)
        .unwrap_or(usize::MAX)
        .min(vertices.len());
    if start >= end {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for vertex in &vertices[start..end] {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(vertex.position[axis]);
            maximum[axis] = maximum[axis].max(vertex.position[axis]);
        }
    }
    (minimum, maximum)
}

fn auto4_union_bounds(
    a_min: [f32; 3],
    a_max: [f32; 3],
    b_min: [f32; 3],
    b_max: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let mut minimum = a_min;
    let mut maximum = a_max;
    for axis in 0..3 {
        minimum[axis] = minimum[axis].min(b_min[axis]);
        maximum[axis] = maximum[axis].max(b_max[axis]);
    }
    (minimum, maximum)
}

pub(crate) fn draw_batches_share_state(a: &DrawBatch, b: &DrawBatch) -> bool {
    // AUTO 4 and the renderer's multi-draw compactor must use one authoritative
    // compatibility test: a representative bind group/pipeline is only valid
    // when every draw-relevant field below matches.
    if a.planar_reflection
        || b.planar_reflection
        || a.planar_environment_candidate
        || b.planar_environment_candidate
    {
        return false;
    }
    a.pipeline == b.pipeline
        && a.texture == b.texture
        && a.texture_is_lightmap == b.texture_is_lightmap
        && a.texture_is_white == b.texture_is_white
        && a.normal_texture == b.normal_texture
        && a.roughness_texture == b.roughness_texture
        && a.height_texture == b.height_texture
        && a.metallic_texture == b.metallic_texture
        && a.specular_texture == b.specular_texture
        && a.emissive_texture == b.emissive_texture
        && a.height_from_alpha == b.height_from_alpha
        && a.rmo_packed == b.rmo_packed
        && a.rmo_specular_alpha == b.rmo_specular_alpha
        && a.normal_scale == b.normal_scale
        && a.roughness_override == b.roughness_override
        && a.specular_reflectance == b.specular_reflectance
        && a.parallax_depth == b.parallax_depth
        && a.lightmap == b.lightmap
        && a.modulate_lightmap == b.modulate_lightmap
        && a.dlight_in_lightmap_stage == b.dlight_in_lightmap_stage
        && a.vertex_lit == b.vertex_lit
        && a.tc_gen == b.tc_gen
        && a.tc_mods == b.tc_mods
        && a.rgb_gen == b.rgb_gen
        && a.alpha_gen == b.alpha_gen
        && a.color == b.color
        && a.alpha_cutoff == b.alpha_cutoff
        && a.fog == b.fog
        && a.fog_is_global == b.fog_is_global
        && a.fog_color_override == b.fog_color_override
        && a.legacy2_fog_in_stage_safe == b.legacy2_fog_in_stage_safe
        && a.global_fog_post_eligible == b.global_fog_post_eligible
        && a.skybox == b.skybox
}

/// Build AUTO 4's immutable cluster -> draw-recipe table while the map is still
/// on the CPU preparation path. For each camera cluster the exact PVS-visible
/// FULL pieces are regrouped under the MINIMAL batch that owns them, so a warm
/// plan issues exactly MINIMAL's draws, in MINIMAL's order, with only visible
/// geometry. Recipes are deduplicated globally; each unique member list maps to
/// one physical index recipe shared by every stage of the same surfaces.
fn build_prepared_portal_draw_plan<F>(
    coarse: &[DrawBatch],
    full: &[DrawBatch],
    visibility: &Visibility,
    vertices: &[GpuVertex],
    mut progress: F,
) -> PreparedPortalDrawPlan
where
    F: FnMut(u32),
{
    let cluster_count = visibility.clusters;
    if cluster_count == 0 || full.is_empty() {
        return PreparedPortalDrawPlan::default();
    }

    #[derive(Clone, Copy)]
    enum Slot {
        Base(usize),
        Owner(usize),
    }

    let owners = auto4_piece_owners(coarse, full);
    let bounds = full
        .iter()
        .map(|piece| auto4_source_bounds(piece, vertices))
        .collect::<Vec<_>>();
    let piece_len = |index: usize| full[index].vertices.end.saturating_sub(full[index].vertices.start) as usize;

    // Pieces visible from each cluster, ascending by piece index (the order the
    // per-cluster loop must see them in). Built by walking each piece's set bits
    // once instead of testing every piece against every cluster.
    let mut visible_pieces = vec![Vec::<u32>::new(); cluster_count];
    for (index, piece) in full.iter().enumerate() {
        let index = index as u32;
        if piece.pvs_signature.is_empty() {
            for list in &mut visible_pieces {
                list.push(index);
            }
            continue;
        }
        for (word_index, &word) in piece.pvs_signature.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let cluster = word_index * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if let Some(list) = visible_pieces.get_mut(cluster) {
                    list.push(index);
                }
            }
        }
    }

    type FastMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<FoldHasher>>;
    let mut members_by_owner = vec![Vec::<usize>::new(); coarse.len()];
    let mut slots = Vec::<Slot>::new();
    let mut variants = Vec::<PreparedPortalMergedVariant>::new();
    let mut variant_lookup = FastMap::<Vec<usize>, usize>::default();
    let mut geometries = Vec::<PreparedPortalGeometry>::new();
    let mut geometry_lookup = FastMap::<Vec<(u32, u32)>, usize>::default();
    let mut plans = Vec::<Vec<PreparedPortalPlanBatchRef>>::new();
    let mut plan_lookup = FastMap::<Vec<PreparedPortalPlanBatchRef>, usize>::default();
    let mut plan_by_cluster = Vec::with_capacity(cluster_count);
    let mut packed_index_count = 0usize;
    let mut reused_variant_hits = 0usize;
    let mut reused_plan_hits = 0usize;
    // Keep loading-screen traffic bounded on maps with hundreds/thousands of
    // clusters while still providing smooth enough progress feedback.
    let progress_stride = cluster_count.div_ceil(128).max(1);

    for cluster in 0..cluster_count {
        // Pieces are in MINIMAL submission order, so first-touch order of each
        // owner reproduces MINIMAL's draw order exactly.
        slots.clear();
        for &visible_index in &visible_pieces[cluster] {
            let index = visible_index as usize;
            let piece = &full[index];
            match owners[index] {
                Some(owner) if auto4_collapsible(piece) => {
                    let list = &mut members_by_owner[owner];
                    if list.is_empty() {
                        slots.push(Slot::Owner(owner));
                    }
                    list.push(index);
                }
                _ => slots.push(Slot::Base(index)),
            }
        }

        let mut refs = Vec::with_capacity(slots.len());
        for &slot in &slots {
            let owner = match slot {
                Slot::Base(index) => {
                    refs.push(PreparedPortalPlanBatchRef::Base(index));
                    continue;
                }
                Slot::Owner(owner) => owner,
            };
            let members = std::mem::take(&mut members_by_owner[owner]);
            if members.len() == 1 {
                refs.push(PreparedPortalPlanBatchRef::Base(members[0]));
                continue;
            }
            if let Some(&variant) = variant_lookup.get(&members) {
                reused_variant_hits += 1;
                refs.push(PreparedPortalPlanBatchRef::Merged(variant));
                continue;
            }
            let ranges = members
                .iter()
                .map(|&member| (full[member].vertices.start, full[member].vertices.end))
                .collect::<Vec<_>>();
            let geometry = *geometry_lookup.entry(ranges).or_insert_with(|| {
                let contiguous = members
                    .windows(2)
                    .all(|pair| full[pair[0]].vertices.end == full[pair[1]].vertices.start);
                if !contiguous {
                    packed_index_count += members.iter().map(|&member| piece_len(member)).sum::<usize>();
                }
                geometries.push(PreparedPortalGeometry {
                    members: members.clone(),
                    contiguous,
                });
                geometries.len() - 1
            });
            let first_area = full[members[0]].area_signature;
            let mut area_signature = [0_u64; 4];
            let mut area_mixed = false;
            let (mut bounds_min, mut bounds_max) = bounds[members[0]];
            for &member in &members {
                let signature = full[member].area_signature;
                area_mixed |= signature != first_area;
                for (word, value) in area_signature.iter_mut().zip(signature) {
                    *word |= value;
                }
                (bounds_min, bounds_max) =
                    auto4_union_bounds(bounds_min, bounds_max, bounds[member].0, bounds[member].1);
            }
            let variant = variants.len();
            variants.push(PreparedPortalMergedVariant {
                representative: members[0],
                members: members.clone(),
                geometry,
                area_signature,
                area_mixed,
                bounds_min,
                bounds_max,
            });
            variant_lookup.insert(members, variant);
            refs.push(PreparedPortalPlanBatchRef::Merged(variant));
        }

        let plan_id = if let Some(&plan_id) = plan_lookup.get(&refs) {
            reused_plan_hits += 1;
            plan_id
        } else {
            plans.push(refs.clone());
            plan_lookup.insert(refs, plans.len() - 1);
            plans.len() - 1
        };
        plan_by_cluster.push(plan_id);

        let completed = cluster + 1;
        if completed == cluster_count || completed % progress_stride == 0 {
            progress(u32::try_from(completed).unwrap_or(u32::MAX));
        }
    }

    PreparedPortalDrawPlan {
        variants,
        geometries,
        plans,
        plan_by_cluster,
        piece_count: full.len(),
        packed_index_count,
        reused_variant_hits,
        reused_plan_hits,
    }
}

/// Offline AUTO 4 audit for `--validate-map`. For every camera cluster it
/// compares MINIMAL, FULL, the current AUTO 4 recipe and the ideal recipe
/// (exact PVS-visible FULL pieces rebatched purely by draw-state class), and
/// attributes every AUTO 4 draw above the ideal to the rule that caused it.
pub fn auto4_audit_lines(map: &PreparedMap) -> Vec<String> {
    let plan = &map.portal_draw_plan;
    let cluster_count = plan.plan_by_cluster.len();
    if cluster_count == 0 {
        return vec!["AUTO 4 audit: no compiled PVS / no AUTO 4 plan".into()];
    }
    let tris = |batch: &DrawBatch| u64::from(batch.vertices.end.saturating_sub(batch.vertices.start) / 3);

    // Global draw-state class ids, shared by FULL and portal pieces so "ideal"
    // and "current" are measured against the renderer's own compatibility key.
    fn classify(batch: &DrawBatch, reps: &mut Vec<DrawBatch>) -> usize {
        if let Some(id) = reps.iter().position(|rep| draw_batches_share_state(rep, batch)) {
            id
        } else {
            reps.push(batch.clone());
            reps.len() - 1
        }
    }
    let mut reps = Vec::<DrawBatch>::new();
    let full_class = map.pvs_batches.iter().map(|b| classify(b, &mut reps)).collect::<Vec<_>>();
    let class_count = reps.len();
    // Relaxed keys for "what if the binding model changed": lightmap page moved
    // into a texture array (lightmap ignored), and fully bindless materials
    // (only the fixed-function PipelineKey remains).
    let mut reps_nolm = Vec::<DrawBatch>::new();
    let full_class_nolm = map
        .pvs_batches
        .iter()
        .map(|b| {
            let mut probe = b.clone();
            probe.lightmap = None;
            classify(&probe, &mut reps_nolm)
        })
        .collect::<Vec<_>>();
    let mut pipelines = Vec::<PipelineKey>::new();
    let full_pipeline = map
        .pvs_batches
        .iter()
        .map(|b| {
            pipelines.iter().position(|p| *p == b.pipeline).unwrap_or_else(|| {
                pipelines.push(b.pipeline);
                pipelines.len() - 1
            })
        })
        .collect::<Vec<_>>();

    #[derive(Default, Clone, Copy)]
    struct Sum {
        draws: u64,
        tris: u64,
        max_draws: u64,
        max_tris: u64,
    }
    impl Sum {
        fn add(&mut self, draws: u64, tris: u64) {
            self.draws += draws;
            self.tris += tris;
            self.max_draws = self.max_draws.max(draws);
            self.max_tris = self.max_tris.max(tris);
        }
    }
    let mut minimal = Sum::default();
    let mut minimal_mergeable = 0u64;
    let mut full = Sum::default();
    let mut auto4_warm = Sum::default();
    let mut auto4_cold = Sum::default();
    let mut ideal = Sum::default();
    let mut ideal_nolm = 0u64;
    let mut ideal_bindless = 0u64;
    let (mut over_minimal, mut over_ideal) = (0u64, 0u64);
    // Ideal recipe memory: unique (class, visible FULL piece set) pairs that
    // are not "everything in this class", and how many are one contiguous run.
    let mut ideal_recipes = BTreeSet::<(usize, Vec<usize>)>::new();
    let class_full_members = {
        let mut members = vec![Vec::<usize>::new(); class_count];
        for (index, &class) in full_class.iter().enumerate() {
            if auto4_merge_safe(&map.pvs_batches[index]) {
                members[class].push(index);
            }
        }
        members
    };
    let mut worst = (0u64, 0usize);
    let mut rows = Vec::<(usize, [u64; 11])>::new();

    for cluster in 0..cluster_count {
        let visible = |b: &DrawBatch| portal_batch_visible_from_cluster(b, cluster);
        // MINIMAL: every coarse batch with any visible piece, full index range.
        let (mut min_draws, mut min_tris, mut min_unmergeable) = (0u64, 0u64, 0u64);
        for batch in map.batches.iter().filter(|b| visible(b)) {
            min_draws += 1;
            min_tris += tris(batch);
            if !auto4_merge_safe(batch) {
                min_unmergeable += 1;
            }
        }
        minimal.add(min_draws, min_tris);
        minimal_mergeable += min_draws - min_unmergeable;

        // FULL + IDEAL. IDEAL = exact visible FULL pieces; opaque/mask merged by
        // draw-state class (order-free under depth test), everything else kept
        // at MINIMAL's one-draw-per-coarse-batch order with only visible pieces.
        let mut full_by_class = vec![Vec::<usize>::new(); class_count];
        let mut nolm = BTreeSet::<usize>::new();
        let mut bindless = BTreeSet::<usize>::new();
        let (mut full_draws, mut full_tris) = (0u64, 0u64);
        for (index, batch) in map.pvs_batches.iter().enumerate().filter(|(_, b)| visible(b)) {
            full_draws += 1;
            full_tris += tris(batch);
            if auto4_merge_safe(batch) {
                full_by_class[full_class[index]].push(index);
                nolm.insert(full_class_nolm[index]);
                bindless.insert(full_pipeline[index]);
            }
        }
        full.add(full_draws, full_tris);
        let merge_classes = full_by_class.iter().filter(|m| !m.is_empty()).count() as u64;
        let ideal_draws = merge_classes + min_unmergeable;
        ideal.add(ideal_draws, full_tris);
        ideal_nolm += nolm.len() as u64 + min_unmergeable;
        ideal_bindless += bindless.len() as u64 + min_unmergeable;
        for (class, members) in full_by_class.into_iter().enumerate() {
            if !members.is_empty() && members != class_full_members[class] {
                ideal_recipes.insert((class, members));
            }
        }

        // Current AUTO 4 recipe for this cluster. Warm = one draw per ref;
        // cold = members drawn individually until a non-contiguous recipe's
        // physical index range has been built.
        let refs = &plan.plans[plan.plan_by_cluster[cluster]];
        let (mut warm, mut cold, mut a4_tris) = (0u64, 0u64, 0u64);
        for reference in refs {
            warm += 1;
            let members: &[usize] = match reference {
                PreparedPortalPlanBatchRef::Base(index) => std::slice::from_ref(index),
                PreparedPortalPlanBatchRef::Merged(variant) => &plan.variants[*variant].members,
            };
            cold += match reference {
                PreparedPortalPlanBatchRef::Merged(variant)
                    if !plan.geometries[plan.variants[*variant].geometry].contiguous =>
                {
                    members.len() as u64
                }
                _ => 1,
            };
            for &member in members {
                a4_tris += tris(&map.pvs_batches[member]);
            }
        }
        auto4_warm.add(warm, a4_tris);
        auto4_cold.add(cold, a4_tris);
        let row_over_minimal = warm.saturating_sub(min_draws);
        let row_over_ideal = warm.saturating_sub(ideal_draws);
        over_minimal += row_over_minimal;
        over_ideal += row_over_ideal;
        let excess = row_over_ideal;
        if excess > worst.0 {
            worst = (excess, cluster);
        }
        rows.push((
            cluster,
            [min_draws, min_tris, full_draws, full_tris, warm, cold, a4_tris, ideal_draws,
             row_over_minimal, row_over_ideal, 0],
        ));
    }

    let n = cluster_count as f64;
    let mut lines = Vec::new();
    lines.push(format!(
        "AUTO 4 audit: {cluster_count} cluster(s), {class_count} draw-state class(es) ({} ignoring lightmap page, {} fixed-function pipelines); {} coarse / {} full batch(es); {} recipe(s) over {} geometr(ies), {} contiguous",
        reps_nolm.len(),
        pipelines.len(),
        map.batches.len(),
        map.pvs_batches.len(),
        plan.variants.len(),
        plan.geometries.len(),
        plan.geometries.iter().filter(|geometry| geometry.contiguous).count(),
    ));
    for (label, sum) in [
        ("MINIMAL", &minimal),
        ("FULL", &full),
        ("AUTO 4 warm", &auto4_warm),
        ("AUTO 4 cold", &auto4_cold),
        ("IDEAL (current key)", &ideal),
    ] {
        lines.push(format!(
            "  {label:<20} avg {:7.1} draws / {:9.0} tris   max {:5} draws / {:8} tris",
            sum.draws as f64 / n,
            sum.tris as f64 / n,
            sum.max_draws,
            sum.max_tris
        ));
    }
    lines.push(format!(
        "  MINIMAL draw mix avg: {:.1} opaque/mask, {:.1} transparent/sky/water/env",
        minimal_mergeable as f64 / n,
        (minimal.draws - minimal_mergeable) as f64 / n,
    ));
    lines.push(format!(
        "  IDEAL with lightmap texture-array avg {:.1} draws; with bindless materials avg {:.1} draws",
        ideal_nolm as f64 / n,
        ideal_bindless as f64 / n,
    ));
    lines.push(format!(
        "  AUTO 4 warm draws above MINIMAL avg {:.2}, above IDEAL avg {:.2} per cluster",
        over_minimal as f64 / n,
        over_ideal as f64 / n,
    ));
    let recipe_indices: u64 = ideal_recipes
        .iter()
        .map(|(_, members)| members.iter().map(|&m| tris(&map.pvs_batches[m]) * 3).sum::<u64>())
        .sum();
    let single_run = ideal_recipes
        .iter()
        .filter(|(_, members)| {
            members
                .windows(2)
                .all(|w| map.pvs_batches[w[0]].vertices.end == map.pvs_batches[w[1]].vertices.start)
        })
        .count();
    lines.push(format!(
        "  IDEAL recipes needing new index data: {} unique ({} already one contiguous vertex run), eager cost {:.2} MiB",
        ideal_recipes.len(),
        single_run,
        recipe_indices as f64 * 4.0 / (1024.0 * 1024.0)
    ));
    rows.sort_by_key(|(_, r)| r[3]);
    let median = rows[rows.len() / 2];
    let heaviest = *rows.last().unwrap();
    let worst_row = *rows.iter().find(|(c, _)| *c == worst.1).unwrap_or(&median);
    for (label, (cluster, r)) in [("median", median), ("heaviest", heaviest), ("worst-excess", worst_row)] {
        lines.push(format!(
            "  {label:<12} cluster {cluster:5}: MINIMAL {}d/{}t  FULL {}d/{}t  AUTO4 warm {}d (cold {}d)/{}t  IDEAL {}d/{}t  above MINIMAL {}, above IDEAL {}",
            r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[3], r[8], r[9]
        ));
    }
    // Strict plan verification: every cluster must cover exactly its visible
    // FULL pieces once, and every recipe's bounds must contain its pieces.
    let piece_bounds = map
        .pvs_batches
        .iter()
        .map(|piece| auto4_source_bounds(piece, &map.vertices))
        .collect::<Vec<_>>();
    let mut coverage_errors = 0usize;
    let mut first_coverage_error = None;
    for cluster in 0..cluster_count {
        let mut expected = (0..plan.piece_count.min(map.pvs_batches.len()))
            .filter(|&index| portal_batch_visible_from_cluster(&map.pvs_batches[index], cluster))
            .collect::<Vec<_>>();
        let mut covered = Vec::new();
        for reference in &plan.plans[plan.plan_by_cluster[cluster]] {
            match reference {
                PreparedPortalPlanBatchRef::Base(index) => covered.push(*index),
                PreparedPortalPlanBatchRef::Merged(variant) => {
                    covered.extend_from_slice(&plan.variants[*variant].members)
                }
            }
        }
        expected.sort_unstable();
        covered.sort_unstable();
        if expected != covered {
            coverage_errors += 1;
            first_coverage_error.get_or_insert((cluster, expected.len(), covered.len()));
        }
    }
    let mut bounds_errors = 0usize;
    for variant in &plan.variants {
        let contains = |member: usize| {
            let (minimum, maximum) = piece_bounds[member];
            let empty = map.pvs_batches[member].vertices.is_empty();
            empty || (0..3).all(|axis| {
                minimum[axis] >= variant.bounds_min[axis] && maximum[axis] <= variant.bounds_max[axis]
            })
        };
        if !variant.members.iter().all(|&member| contains(member)) {
            bounds_errors += 1;
        }
    }
    lines.push(format!(
        "  VERIFY: {coverage_errors} cluster(s) with wrong piece coverage{}, {bounds_errors} recipe(s) with bounds not containing their pieces",
        first_coverage_error
            .map(|(cluster, expected, covered)| format!(" (first: cluster {cluster}, expected {expected} pieces, plan covers {covered})"))
            .unwrap_or_default(),
    ));
    // JKA_AUDIT_POS="x y z" in renderer coordinates (as printed by
    // [JKA PERF STATE] camera=[..]) reports that camera's cluster and plan.
    if let (Some(position), Some(vis)) = (std::env::var("JKA_AUDIT_POS").ok(), map.visibility.as_ref()) {
        let parts = position
            .split(|c: char| c == ' ' || c == ',')
            .filter_map(|part| part.trim().parse::<f32>().ok())
            .collect::<Vec<_>>();
        if let [x, y, z] = parts[..] {
            match vis.cluster_at(jka_position([x, y, z])) {
                Some(cluster) if cluster < cluster_count => {
                    let plan_id = plan.plan_by_cluster[cluster];
                    let refs = &plan.plans[plan_id];
                    let visible = map
                        .pvs_batches
                        .iter()
                        .filter(|piece| portal_batch_visible_from_cluster(piece, cluster))
                        .count();
                    lines.push(format!(
                        "  POS [{x}, {y}, {z}] -> cluster {cluster}, plan {plan_id}: {} ref(s), {visible} visible FULL piece(s)",
                        refs.len()
                    ));
                    for reference in refs {
                        if let PreparedPortalPlanBatchRef::Merged(variant) = reference {
                            let recipe = &plan.variants[*variant];
                            let geometry = &plan.geometries[recipe.geometry];
                            lines.push(format!(
                                "    recipe {variant}: {} member(s) {:?}, geometry {} contiguous={} area_mixed={} bounds {:?}..{:?}",
                                recipe.members.len(),
                                &recipe.members[..recipe.members.len().min(8)],
                                recipe.geometry,
                                geometry.contiguous,
                                recipe.area_mixed,
                                recipe.bounds_min,
                                recipe.bounds_max,
                            ));
                        }
                    }
                }
                other => lines.push(format!("  POS [{x}, {y}, {z}] -> cluster {other:?} (outside PVS)")),
            }
        }
    }
    lines
}

/// Order one MINIMAL batch's PVS pieces so pieces seen from the same camera
/// clusters are adjacent. AUTO 4 draws each cluster's visible pieces of a batch
/// as one indexed draw; when those pieces are adjacent, FULL's index data
/// already holds that draw as one run and nothing has to be built at runtime.
/// The sum of Hamming distances between neighbouring signatures counts the
/// visibility-run boundaries over all clusters, so a greedy nearest-neighbour
/// path over that distance, refined with 2-opt, keeps each cluster's visible
/// pieces together.
fn order_pvs_pieces<T>(pieces: BTreeMap<(Vec<u64>, [u64; 4]), T>) -> Vec<((Vec<u64>, [u64; 4]), T)> {
    // Quadratic; very large batches keep signature order to bound load time.
    const MAX_ORDERED_PIECES: usize = 4096;
    let mut pieces = pieces.into_iter().collect::<Vec<_>>();
    let count = pieces.len();
    if count <= 2 || count > MAX_ORDERED_PIECES {
        return pieces;
    }
    // Signatures in one contiguous matrix, zero-padded to a common width. A
    // missing word counts as zero, so xor with the padding equals the old
    // "popcount of the longer tail" and every distance is unchanged.
    let stride = pieces.iter().map(|piece| piece.0 .0.len()).max().unwrap_or(0);
    let mut matrix = vec![0_u64; count * stride];
    for (index, piece) in pieces.iter().enumerate() {
        matrix[index * stride..index * stride + piece.0 .0.len()].copy_from_slice(&piece.0 .0);
    }
    let signature = |piece: usize| &matrix[piece * stride..(piece + 1) * stride];
    let distance = |a: &[u64], b: &[u64]| -> u32 {
        a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum::<u32>()
    };
    // Start from the most narrowly visible piece: it is a natural path end.
    let mut current = (0..count)
        .min_by_key(|&index| signature(index).iter().map(|word| word.count_ones()).sum::<u32>())
        .unwrap_or(0);
    let mut visited = vec![false; count];
    let mut order = Vec::with_capacity(count);
    visited[current] = true;
    order.push(current);
    for _ in 1..count {
        let next = (0..count)
            .filter(|&index| !visited[index])
            .min_by_key(|&index| distance(signature(current), signature(index)))
            .expect("unvisited piece remains");
        visited[next] = true;
        order.push(next);
        current = next;
    }
    // 2-opt: reverse any segment whose endpoints then join more cheaply.
    for _ in 0..8 {
        let mut improved = false;
        for i in 0..count.saturating_sub(2) {
            for j in i + 2..count {
                let (a, b, c) = (order[i], order[i + 1], order[j]);
                let d = order.get(j + 1).copied();
                let before = distance(signature(a), signature(b))
                    + d.map_or(0, |d| distance(signature(c), signature(d)));
                let after = distance(signature(a), signature(c))
                    + d.map_or(0, |d| distance(signature(b), signature(d)));
                if after < before {
                    order[i + 1..=j].reverse();
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    let mut slots = pieces.drain(..).map(Some).collect::<Vec<_>>();
    order
        .into_iter()
        .map(|index| slots[index].take().expect("each piece is ordered once"))
        .collect()
}

fn external_lightmap_pages(mesh: &jka_assets::bsp::Mesh) -> BTreeSet<usize> {
    mesh.batches
        .iter()
        .flat_map(|batch| {
            (0..4).filter_map(move |slot| {
                (batch.lightmaps[slot] >= 0 && batch.lightmap_styles[slot] < 254)
                    .then_some(batch.lightmaps[slot] as usize)
            })
        })
        .collect()
}

fn embedded_deluxe_mapping(mesh: &jka_assets::bsp::Mesh, page_count: usize) -> bool {
    if page_count < 2 {
        return false;
    }
    let pages = external_lightmap_pages(mesh);
    !pages.is_empty()
        && pages
            .iter()
            .all(|&page| page % 2 == 0 && page + 1 < page_count)
}

fn hdr_lightmap_indexed(assets: &AssetSearchPath, map_name: &str) -> bool {
    let prefix = format!("maps/{map_name}/lm_").to_ascii_lowercase();
    assets.names().any(|name| {
        let lower = name.to_ascii_lowercase();
        lower.starts_with(&prefix) && lower.ends_with(".hdr")
    })
}

fn try_load_hdr_lightmap(
    assets: &mut AssetSearchPath,
    map_name: &str,
    page: usize,
) -> Result<Option<TextureData>, String> {
    let path = format!("maps/{map_name}/lm_{page:04}.hdr");
    let Some(asset) = assets
        .read(&path, 64 * 1024 * 1024)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let mut image = materials::decode_hdr_texture_data(
        &path,
        &asset.bytes,
        true,
        std::f32::consts::FRAC_1_PI,
    )?;
    image.source = Some(asset.source);
    Ok(Some(image))
}

fn load_external_lightmap(
    assets: &mut AssetSearchPath,
    map_name: &str,
    page: usize,
    prefer_hdr: bool,
) -> Result<TextureData, String> {
    if prefer_hdr {
        if let Some(image) = try_load_hdr_lightmap(assets, map_name, page)? {
            return Ok(image);
        }
    }
    let stem = format!("maps/{map_name}/lm_{page:04}");
    let candidates = [
        format!("{stem}.tga"),
        format!("{stem}.jpg"),
        format!("{stem}.png"),
    ];

    for path in &candidates {
        if let Some(asset) = assets
            .read(path, 64 * 1024 * 1024)
            .map_err(|error| error.to_string())?
        {
            let mut image = materials::decode_texture_data(path, &asset.bytes, true, false)?;
            image.source = Some(asset.source);
            if prefer_hdr {
                materials::promote_lightmap_to_float(&mut image);
            }
            return Ok(image);
        }
    }

    Err(format!(
        "External lightmap page {page} is referenced by {map_name}.bsp but {} was not found on the game/base asset search path",
        candidates[0]
    ))
}

fn neutral_deluxemap(label: impl Into<String>) -> TextureData {
    TextureData {
        label: label.into(),
        source: None,
        width: 1,
        height: 1,
        rgba: vec![127, 127, 127, 0],
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: false,
    }
}

fn normalize_deluxemap(mut image: TextureData) -> TextureData {
    image.srgb = false;
    image.rgba16f = None;
    for pixel in image.rgba.chunks_exact_mut(4) {
        if pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 {
            pixel[0] = 127;
            pixel[1] = 127;
            pixel[2] = 127;
        }
        // Alpha is reserved as a renderer-side validity marker. Authored
        // q3map2 deluxemaps do not need to carry meaningful alpha.
        pixel[3] = 255;
    }
    image
}

fn try_load_external_deluxemap(
    assets: &mut AssetSearchPath,
    map_name: &str,
    page: usize,
    paired_lightmaps: bool,
) -> Result<Option<TextureData>, String> {
    let mut stems = Vec::new();
    if paired_lightmaps {
        stems.push(format!("maps/{map_name}/lm_{:04}", page + 1));
        stems.push(format!("maps/{map_name}/dm_{:04}", page / 2));
    }
    stems.push(format!("maps/{map_name}/dm_{page:04}"));
    stems.dedup();
    for stem in stems {
        for ext in ["tga", "jpg", "png"] {
            let path = format!("{stem}.{ext}");
            if let Some(asset) = assets
                .read(&path, 64 * 1024 * 1024)
                .map_err(|error| error.to_string())?
            {
                let mut image = materials::decode_texture_data_with_color_space(
                    &path,
                    &asset.bytes,
                    true,
                    false,
                    false,
                )?;
                image.source = Some(asset.source);
                return Ok(Some(normalize_deluxemap(image)));
            }
        }
    }
    Ok(None)
}

fn embedded_lightmap_texture(index: usize, page: &[u8]) -> TextureData {
    TextureData {
        label: format!("JKA lightmap {index}"),
        source: None,
        width: 128,
        height: 128,
        rgba: page
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    }
}

fn embedded_deluxemap_texture(index: usize, page: &[u8]) -> TextureData {
    let mut rgba: Vec<u8> = page
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    for pixel in rgba.chunks_exact_mut(4) {
        if pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 {
            pixel[0] = 127;
            pixel[1] = 127;
            pixel[2] = 127;
        }
    }
    TextureData {
        label: format!("JKA deluxemap {index}"),
        source: None,
        width: 128,
        height: 128,
        rgba,
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: false,
    }
}

fn decode_lightgrid_direction(lat_long: [u8; 2]) -> [f32; 3] {
    // OpenJK/Q3 stores longitude first and latitude second. The original
    // renderer indexes its 1024-entry trig table with byte * 4, which is
    // exactly byte * 2*pi/256 in radians.
    let longitude = f32::from(lat_long[0]) * std::f32::consts::TAU / 256.0;
    let latitude = f32::from(lat_long[1]) * std::f32::consts::TAU / 256.0;
    let sin_longitude = longitude.sin();
    let jka = [
        latitude.cos() * sin_longitude,
        latitude.sin() * sin_longitude,
        longitude.cos(),
    ];
    // JKA -> renderer coordinates: [x, z, -y].
    let render = [jka[0], jka[2], -jka[1]];
    let length = (render[0] * render[0] + render[1] * render[1] + render[2] * render[2])
        .sqrt()
        .max(1e-6);
    [render[0] / length, render[1] / length, render[2] / length]
}

fn light_luminance(rgb: [f32; 3]) -> f32 {
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

fn smoothstep_range(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn encode_unit_channel(value: f32) -> u8 {
    ((value.clamp(0.0, 1.0) * 255.0) + 0.5) as u8
}

fn prepare_static_light_grid(
    bsp: &Bsp,
    assets: &mut AssetSearchPath,
    map_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Option<StaticLightGrid>, String> {
    let Some(grid) = bsp.light_grid.as_ref() else {
        return Ok(None);
    };
    let cell_count = grid.cell_count();
    if cell_count == 0 {
        return Ok(None);
    }

    // Rend2/q3map2 may ship an expanded HDR lightgrid alongside the BSP.
    // It contains six little-endian f32 values per grid point: ambient RGB,
    // directed RGB. We only need scale-independent directionality/chromaticity
    // here because the BSP lightmap still provides the baked irradiance scale.
    let raw_path = format!("maps/{map_name}/lightgrid.raw");
    let external = assets
        .read(&raw_path, cell_count.saturating_mul(24).saturating_add(1))
        .map_err(|error| error.to_string())?;
    let external_values = if let Some(asset) = external {
        if asset.bytes.len() == cell_count * 24 {
            let values = asset
                .bytes
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                .collect::<Vec<_>>();
            warnings.push(format!(
                "{map_name}: using Rend2 HDR lightgrid {} ({} grid points)",
                raw_path, cell_count
            ));
            Some(values)
        } else {
            warnings.push(format!(
                "{map_name}: ignored {} because it is {} bytes; expected {}",
                raw_path,
                asset.bytes.len(),
                cell_count * 24
            ));
            None
        }
    } else {
        None
    };

    let mut direction_rgba = Vec::with_capacity(cell_count * 4);
    let mut lighting_rgba = Vec::with_capacity(cell_count * 4);
    let mut classic_cells = Vec::with_capacity(cell_count);
    // Keep the six ambient-cube irradiances in float until every probe has been
    // seen, then choose one map-wide scale so HDR lightgrid.raw values are not
    // clipped by the RGBA8 volume texture. Face order: +X,-X,+Y,-Y,+Z,-Z.
    let mut ambient_cube_faces = vec![[[0.0_f32; 3]; 6]; cell_count];
    let mut ambient_cube_peak = 255.0_f32;
    for cell in 0..cell_count {
        let sample = grid.sample_for_cell(cell);
        let style = sample.and_then(|sample| sample.styles.iter().position(|style| *style < 254));
        let valid = sample.is_some() && style.is_some();
        let direction = sample
            .map(|sample| decode_lightgrid_direction(sample.lat_long))
            .unwrap_or([0.0, 1.0, 0.0]);

        let (ambient, directed) = if let Some(values) = external_values.as_ref() {
            let base = cell * 6;
            let clean = |value: f32| {
                if value.is_finite() {
                    value.max(0.0)
                } else {
                    0.0
                }
            };
            (
                [
                    clean(values[base]),
                    clean(values[base + 1]),
                    clean(values[base + 2]),
                ],
                [
                    clean(values[base + 3]),
                    clean(values[base + 4]),
                    clean(values[base + 5]),
                ],
            )
        } else if let (Some(sample), Some(style)) = (sample, style) {
            (
                sample.ambient_light[style].map(f32::from),
                sample.direct_light[style].map(f32::from),
            )
        } else {
            ([0.0; 3], [0.0; 3])
        };

        // Keep a second absolute-light representation for dynamic entities.
        // OpenJK's entity path consumes every active style slot (styleColors
        // initialize to white), whereas the scale-free PBR representation above
        // only needs one representative chroma/direction sample.
        let classic_valid = sample.is_some_and(|sample| sample.styles[0] != 255);
        let (classic_ambient, classic_directed) = if classic_valid {
            if let Some(values) = external_values.as_ref() {
                // Rend2 loads lightgrid.raw as radiance / PI, then converts back
                // to the renderer's 0..255 light units during entity sampling.
                let base = cell * 6;
                let convert = |value: f32| {
                    if value.is_finite() {
                        value.max(0.0) * (255.0 / std::f32::consts::PI)
                    } else {
                        0.0
                    }
                };
                (
                    [convert(values[base]), convert(values[base + 1]), convert(values[base + 2])],
                    [
                        convert(values[base + 3]),
                        convert(values[base + 4]),
                        convert(values[base + 5]),
                    ],
                )
            } else if let Some(sample) = sample {
                let mut classic_ambient = [0.0_f32; 3];
                let mut classic_directed = [0.0_f32; 3];
                for style_index in 0..sample.styles.len() {
                    if sample.styles[style_index] == 255 {
                        break;
                    }
                    for channel in 0..3 {
                        classic_ambient[channel] +=
                            f32::from(sample.ambient_light[style_index][channel]);
                        classic_directed[channel] +=
                            f32::from(sample.direct_light[style_index][channel]);
                    }
                }
                (classic_ambient, classic_directed)
            } else {
                ([0.0; 3], [0.0; 3])
            }
        } else {
            ([0.0; 3], [0.0; 3])
        };
        classic_cells.push(ClassicLightGridCell {
            ambient: classic_ambient,
            directed: classic_directed,
            direction,
            sun_weight: 0.0,
            valid: classic_valid,
        });

        let axes = [
            [1.0_f32, 0.0, 0.0], [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0], [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0], [0.0, 0.0, -1.0],
        ];
        for (face_index, axis) in axes.into_iter().enumerate() {
            let ndotl = (axis[0] * direction[0] + axis[1] * direction[1] + axis[2] * direction[2]).max(0.0);
            let irradiance = [
                ambient[0] + directed[0] * ndotl,
                ambient[1] + directed[1] * ndotl,
                ambient[2] + directed[2] * ndotl,
            ];
            ambient_cube_peak = ambient_cube_peak.max(irradiance.into_iter().fold(0.0_f32, f32::max));
            ambient_cube_faces[cell][face_index] = irradiance;
        }

        let ambient_luma = light_luminance(ambient);
        let directed_luma = light_luminance(directed);
        let directional_fraction = if valid && ambient_luma + directed_luma > 1e-6 {
            directed_luma / (ambient_luma + directed_luma)
        } else {
            0.0
        };
        let direct_max = directed.into_iter().fold(0.0_f32, f32::max);
        let chroma = if direct_max > 1e-6 {
            directed.map(|value| value / direct_max)
        } else {
            [1.0; 3]
        };

        direction_rgba.extend_from_slice(&[
            encode_unit_channel(direction[0] * 0.5 + 0.5),
            encode_unit_channel(direction[1] * 0.5 + 0.5),
            encode_unit_channel(direction[2] * 0.5 + 0.5),
            encode_unit_channel(directional_fraction),
        ]);
        lighting_rgba.extend_from_slice(&[
            encode_unit_channel(chroma[0]),
            encode_unit_channel(chroma[1]),
            encode_unit_channel(chroma[2]),
            if valid { 255 } else { 0 },
        ]);
    }

    let bx = grid.bounds[0] as usize;
    let by = grid.bounds[1] as usize;
    let bz = grid.bounds[2] as usize;
    let atlas_height = by * 2;
    let atlas_depth = bz * 3;
    let mut irradiance_volume_rgba = vec![0_u8; bx * atlas_height * atlas_depth * 4];
    let encode_irradiance = |value: f32| -> u8 {
        ((value.max(0.0) / ambient_cube_peak).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    };
    for z in 0..bz {
        for y in 0..by {
            for x in 0..bx {
                let cell = x + bx * (y + by * z);
                for axis in 0..3 {
                    for negative in 0..2 {
                        let face = axis * 2 + negative;
                        let atlas_y = y + negative * by;
                        let atlas_z = z + axis * bz;
                        let dst = 4 * (x + bx * (atlas_y + atlas_height * atlas_z));
                        let rgb = ambient_cube_faces[cell][face];
                        irradiance_volume_rgba[dst] = encode_irradiance(rgb[0]);
                        irradiance_volume_rgba[dst + 1] = encode_irradiance(rgb[1]);
                        irradiance_volume_rgba[dst + 2] = encode_irradiance(rgb[2]);
                        irradiance_volume_rgba[dst + 3] = 255;
                    }
                }
            }
        }
    }
    let irradiance_intensity = ambient_cube_peak / 255.0;

    warnings.push(format!(
        "{map_name}: Bevy irradiance volume {}x{}x{} ambient-cube atlas generated from the BSP lightgrid",
        bx, atlas_height, atlas_depth
    ));
    warnings.push(format!(
        "{map_name}: static lightgrid {}x{}x{} available for PBR directional baked lighting and classic entity lighting{}",
        grid.bounds[0],
        grid.bounds[1],
        grid.bounds[2],
        if external_values.is_some() {
            " (HDR companion)"
        } else {
            ""
        }
    ));

    Ok(Some(StaticLightGrid {
        // Preserve JKA X/Y/Z indexing in the 3D texture; the shader converts
        // render-space positions back to JKA coordinates before sampling.
        origin: grid.origin,
        size: grid.size,
        bounds: [
            grid.bounds[0] as u32,
            grid.bounds[1] as u32,
            grid.bounds[2] as u32,
        ],
        direction_rgba,
        lighting_rgba,
        irradiance_volume_rgba,
        irradiance_intensity,
        external_hdr: external_values.is_some(),
        classic_entity_grid: ClassicEntityLightGrid {
            origin: grid.origin,
            size: grid.size,
            bounds: [
                grid.bounds[0] as u32,
                grid.bounds[1] as u32,
                grid.bounds[2] as u32,
            ],
            external_hdr: external_values.is_some(),
            cells: classic_cells,
            map_toward_sun: None,
        },
    }))
}

#[derive(Debug, Clone)]
struct ReflectionProbeDescriptor {
    name: Option<String>,
    position: [f32; 3],
    radius: f32,
}

fn json_f32(value: &serde_json::Value) -> Option<f32> {
    let parsed = if let Some(number) = value.as_f64() {
        number as f32
    } else {
        value.as_str()?.trim().parse::<f32>().ok()?
    };
    parsed.is_finite().then_some(parsed)
}

fn json_vec3(value: &serde_json::Value) -> Option<[f32; 3]> {
    let values = value.as_array()?;
    if values.len() != 3 {
        return None;
    }
    Some([
        json_f32(&values[0])?,
        json_f32(&values[1])?,
        json_f32(&values[2])?,
    ])
}

fn reflection_probe_descriptors(
    bsp: &Bsp,
    assets: &mut AssetSearchPath,
    map_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<ReflectionProbeDescriptor>, String> {
    // Match Rend2/OpenGL2 exactly: cubemaps/<world baseName>/env.json takes
    // precedence over misc_cubemap entities.
    let env_path = format!("cubemaps/{map_name}/env.json");
    if let Some(asset) = assets
        .read(&env_path, 4 * 1024 * 1024)
        .map_err(|error| error.to_string())?
    {
        match serde_json::from_slice::<serde_json::Value>(&asset.bytes) {
            Ok(root) => {
                let Some(entries) = root.get("Cubemaps").and_then(serde_json::Value::as_array)
                else {
                    warnings.push(format!("{map_name}: {env_path} has no Cubemaps array"));
                    return Ok(Vec::new());
                };
                let mut probes = Vec::with_capacity(entries.len());
                for (index, entry) in entries.iter().enumerate() {
                    let Some(position) = entry.get("Position").and_then(json_vec3) else {
                        warnings.push(format!(
                            "{map_name}: ignored cubemap {index} in {env_path}: invalid Position"
                        ));
                        continue;
                    };
                    let radius = entry
                        .get("Radius")
                        .and_then(json_f32)
                        .filter(|value| *value > 0.0)
                        .unwrap_or(1000.0);
                    let name = entry
                        .get("Name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned);
                    probes.push(ReflectionProbeDescriptor {
                        name,
                        position,
                        radius,
                    });
                }
                warnings.push(format!(
                    "{map_name}: parsed {} reflection probe(s) from Rend2 {env_path}",
                    probes.len()
                ));
                return Ok(probes);
            }
            Err(error) => {
                warnings.push(format!("{map_name}: invalid Rend2 {env_path}: {error}"));
                return Ok(Vec::new());
            }
        }
    }

    let mut probes = Vec::new();
    for entity in &bsp.entities {
        let classname = entity
            .get(b"classname")
            .and_then(|value| std::str::from_utf8(value).ok());
        if !classname.is_some_and(|value| value.eq_ignore_ascii_case("misc_cubemap")) {
            continue;
        }
        let Some(position) = entity
            .get(b"origin")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(parse_light_triplet)
        else {
            continue;
        };
        let radius = entity
            .get(b"radius")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(1000.0);
        let name = entity
            .get(b"name")
            .and_then(|value| std::str::from_utf8(value).ok())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        probes.push(ReflectionProbeDescriptor {
            name,
            position,
            radius,
        });
    }
    if !probes.is_empty() {
        warnings.push(format!(
            "{map_name}: using {} misc_cubemap reflection probe entity/entities",
            probes.len()
        ));
    }
    Ok(probes)
}

fn cubemap_asset_name(map_name: &str, index: usize, name: Option<&str>) -> String {
    if let Some(name) = name {
        let normalized = name.trim().replace('\\', "/");
        let slash = normalized.rfind('/').map_or(0, |index| index + 1);
        let stem = normalized[slash..]
            .rfind('.')
            .map(|extension| &normalized[..slash + extension])
            .unwrap_or(normalized.as_str());
        return format!("{stem}.dds");
    }
    format!("cubemaps/{map_name}/{index:03}.dds")
}

fn load_reflection_probes(
    bsp: &Bsp,
    assets: &mut AssetSearchPath,
    map_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<ReflectionProbe>, String> {
    let descriptors = reflection_probe_descriptors(bsp, assets, map_name, warnings)?;
    let mut probes = Vec::new();
    for (index, descriptor) in descriptors.into_iter().enumerate() {
        let path = cubemap_asset_name(map_name, index, descriptor.name.as_deref());
        let Some(asset) = assets
            .read(&path, 128 * 1024 * 1024)
            .map_err(|error| error.to_string())?
        else {
            warnings.push(format!(
                "{map_name}: reflection probe image not found: {path}"
            ));
            continue;
        };
        let dds = match image_dds::ddsfile::Dds::read(std::io::Cursor::new(&asset.bytes)) {
            Ok(dds) => dds,
            Err(error) => {
                warnings.push(format!("{map_name}: failed to parse {path}: {error}"));
                continue;
            }
        };
        let decoded = match image_dds::SurfaceRgba8::decode_dds(&dds) {
            Ok(decoded) => decoded,
            Err(error) => {
                warnings.push(format!("{map_name}: failed to decode {path}: {error}"));
                continue;
            }
        };
        if decoded.layers != 6 || decoded.depth != 1 || decoded.width != decoded.height {
            warnings.push(format!(
                "{map_name}: ignored {path}: expected 6-face square cubemap, got {}x{} depth={} layers={}",
                decoded.width, decoded.height, decoded.depth, decoded.layers
            ));
            continue;
        }
        probes.push(ReflectionProbe {
            label: path,
            position: render_position(descriptor.position),
            radius: descriptor.radius,
            width: decoded.width,
            height: decoded.height,
            mip_level_count: decoded.mipmaps.max(1),
            rgba: decoded.data,
        });
    }
    if !probes.is_empty() {
        warnings.push(format!(
            "{map_name}: loaded {} Rend2 reflection cubemap(s)",
            probes.len()
        ));
    }
    Ok(probes)
}

fn assign_reflection_probes(
    batches: &mut [DrawBatch],
    vertices: &[GpuVertex],
    probes: &[ReflectionProbe],
) {
    if probes.is_empty() {
        return;
    }
    for batch in batches {
        if batch.pipeline.class == DrawClass::Sky
            || batch.texture_is_lightmap
            || batch.texture_is_white
        {
            continue;
        }
        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        let Some(slice) = vertices.get(start..end) else {
            continue;
        };
        if slice.is_empty() {
            continue;
        }
        let mut minimum = [f32::INFINITY; 3];
        let mut maximum = [f32::NEG_INFINITY; 3];
        for vertex in slice {
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex.position[axis]);
                maximum[axis] = maximum[axis].max(vertex.position[axis]);
            }
        }
        let center: [f32; 3] = std::array::from_fn(|axis| (minimum[axis] + maximum[axis]) * 0.5);
        let Some((probe_index, probe)) = probes.iter().enumerate().min_by(|(_, a), (_, b)| {
            let distance = |probe: &ReflectionProbe| {
                (0..3)
                    .map(|axis| {
                        let delta = center[axis] - probe.position[axis];
                        delta * delta
                    })
                    .sum::<f32>()
            };
            distance(a).total_cmp(&distance(b))
        }) else {
            continue;
        };
        batch.reflection_probe = Some(probe_index);
        batch.reflection_probe_position_radius = [
            probe.position[0],
            probe.position[1],
            probe.position[2],
            probe.radius,
        ];
    }
}

fn cache_reflection_decisions(batches: &mut [DrawBatch]) {
    for batch in batches {
        batch.reflection_cache_flags = 0;
        batch.reflection_roughness_hint = 1.0;
        if batch.pipeline.class == DrawClass::Sky
            || batch.texture_is_lightmap
            || batch.texture_is_white
        {
            continue;
        }

        let has_pbr_companion = batch.normal_texture.is_some()
            || batch.roughness_texture.is_some()
            || batch.metallic_texture.is_some()
            || batch.specular_texture.is_some()
            || batch.roughness_override.is_some()
            || batch.specular_reflectance.is_some();
        let probe_eligible = batch.reflection_probe.is_some() && has_pbr_companion;
        let planar_eligible = batch.planar_reflection || batch.planar_environment_candidate;

        let roughness = batch.roughness_override.unwrap_or_else(|| {
            if batch.water {
                0.08
            } else if batch.planar_reflection {
                0.02
            } else if batch.planar_environment_candidate {
                0.22
            } else if batch.metallic_texture.is_some() || batch.specular_texture.is_some() {
                0.35
            } else if batch.roughness_texture.is_some() {
                0.50
            } else if batch.specular_reflectance.is_some() {
                0.45
            } else {
                0.85
            }
        });
        batch.reflection_roughness_hint = roughness.clamp(0.0, 1.0);

        // The map-load cache answers whether this geometry can participate in
        // the screen-space pass at all. Keep opaque/masked depth-writing world
        // surfaces eligible because rain can make an otherwise rough material
        // reflective at runtime; the SSR shader applies the quality/roughness
        // budget after evaluating current wetness. This preserves the existing
        // dynamic wet-surface behavior while still avoiding work on sky/blended
        // stages that cannot produce a stable depth-buffer reflection.
        let ssr_eligible = batch.pipeline.blend == BlendMode::Opaque
            && matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask);

        if probe_eligible {
            batch.reflection_cache_flags |= REFLECTION_CACHE_PROBE;
        }
        if ssr_eligible {
            batch.reflection_cache_flags |= REFLECTION_CACHE_SSR;
        }
        if planar_eligible {
            batch.reflection_cache_flags |= REFLECTION_CACHE_PLANAR;
        }
    }
}

fn log_reflection_cache(name: &str, batches: &[DrawBatch]) {
    let probe = batches
        .iter()
        .filter(|batch| (batch.reflection_cache_flags & REFLECTION_CACHE_PROBE) != 0)
        .count();
    let ssr = batches
        .iter()
        .filter(|batch| (batch.reflection_cache_flags & REFLECTION_CACHE_SSR) != 0)
        .count();
    let planar = batches
        .iter()
        .filter(|batch| (batch.reflection_cache_flags & REFLECTION_CACHE_PLANAR) != 0)
        .count();
    println!(
        "{name}: reflection cache: {probe} probe batch(es), {ssr} SSR-eligible batch(es), {planar} planar candidate batch(es)"
    );
}

const PORTAL_SURFACE_MAX_DISTANCE: f32 = 64.0;

#[derive(Clone, Copy)]
struct PortalSurfaceAnchor {
    origin: [f32; 3],
    mirror: bool,
}

const ENVIRONMENT_PLANAR_DISTANCE_EPSILON: f32 = 0.5;
const ENVIRONMENT_PLANAR_NORMAL_DOT: f32 = 0.999;

fn planar_group_key(vertices: &[GpuVertex]) -> Option<PlanarGroupKey> {
    let mut plane = planar_batch_plane(vertices, true)?;

    // The same geometric plane can be represented as (n, d) or (-n, -d).
    // Canonicalise the sign so opposite triangle winding does not fragment an
    // otherwise identical reflector. Reflection math is invariant to this flip.
    let flip = plane[0] < -1e-6
        || (plane[0].abs() <= 1e-6 && plane[1] < -1e-6)
        || (plane[0].abs() <= 1e-6 && plane[1].abs() <= 1e-6 && plane[2] < 0.0);
    if flip {
        for value in &mut plane {
            *value = -*value;
        }
    }

    // BSP planes are normally exact enough that this mostly removes float
    // noise. Keep distance quantisation finer than the strict planar validation
    // epsilon so unrelated parallel surfaces are not casually merged.
    const NORMAL_QUANT: f32 = 4096.0;
    const DISTANCE_QUANT: f32 = 4.0;
    Some(PlanarGroupKey {
        normal_x: (plane[0] * NORMAL_QUANT).round() as i32,
        normal_y: (plane[1] * NORMAL_QUANT).round() as i32,
        normal_z: (plane[2] * NORMAL_QUANT).round() as i32,
        distance: (plane[3] * DISTANCE_QUANT).round() as i32,
    })
}

fn planar_batch_plane(vertices: &[GpuVertex], strict: bool) -> Option<[f32; 4]> {
    let mut plane = None;
    for triangle in vertices.chunks_exact(3) {
        let a = Vec3::from_array(triangle[0].position);
        let b = Vec3::from_array(triangle[1].position);
        let c = Vec3::from_array(triangle[2].position);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if normal.length_squared() <= 1e-6 {
            continue;
        }
        plane = Some([normal.x, normal.y, normal.z, -normal.dot(a)]);
        break;
    }
    let plane = plane?;
    if !strict {
        return Some(plane);
    }

    let reference_normal = Vec3::new(plane[0], plane[1], plane[2]);
    if vertices.iter().any(|vertex| {
        (reference_normal.dot(Vec3::from_array(vertex.position)) + plane[3]).abs()
            > ENVIRONMENT_PLANAR_DISTANCE_EPSILON
    }) {
        return None;
    }
    for triangle in vertices.chunks_exact(3) {
        let a = Vec3::from_array(triangle[0].position);
        let b = Vec3::from_array(triangle[1].position);
        let c = Vec3::from_array(triangle[2].position);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if normal.length_squared() > 1e-6
            && normal.dot(reference_normal).abs() < ENVIRONMENT_PLANAR_NORMAL_DOT
        {
            return None;
        }
    }
    Some(plane)
}

fn assign_planar_reflection_planes(batches: &mut [DrawBatch], vertices: &[GpuVertex], environment: bool) {
    if !environment {
        for batch in batches.iter_mut() {
            batch.planar_environment_candidate = false;
        }
    }
    for batch in batches
        .iter_mut()
        .filter(|batch| batch.planar_reflection || batch.planar_environment_candidate)
    {
        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        let Some(slice) = vertices.get(start..end) else {
            batch.planar_environment_candidate = false;
            continue;
        };
        // Authored mirrors remain authoritative even if their tessellation is
        // slightly imperfect. Environment-map promotion is deliberately strict:
        // every triangle must describe the same geometric plane.
        let strict = batch.planar_environment_candidate && !batch.planar_reflection;
        if let Some(plane) = planar_batch_plane(slice, strict) {
            batch.planar_plane = plane;
        } else if batch.planar_reflection {
            batch.planar_reflection = false;
            batch.planar_environment_candidate = false;
        } else {
            batch.planar_environment_candidate = false;
        }
    }

    // Debug views need to survive multi-stage shaders. A tcGen environment
    // stage can be followed by opaque/alpha/lightmap stages that redraw the
    // exact same vertex range, which would otherwise hide a diagnostic color
    // emitted only by the environment stage. Propagate just the resolved plane
    // (not reflection eligibility) to sibling stages of the same surface. Normal
    // rendering still keys exclusively off planar_reflection / candidate flags.
    let resolved: std::collections::BTreeMap<(u32, u32), [f32; 4]> = batches
        .iter()
        .filter(|batch| {
            (batch.planar_reflection || batch.planar_environment_candidate)
                && Vec3::from_array([
                    batch.planar_plane[0],
                    batch.planar_plane[1],
                    batch.planar_plane[2],
                ])
                .length_squared()
                    > 0.5
        })
        .map(|batch| {
            (
                (batch.vertices.start, batch.vertices.end),
                batch.planar_plane,
            )
        })
        .collect();
    for batch in batches {
        if let Some(plane) = resolved.get(&(batch.vertices.start, batch.vertices.end)) {
            batch.planar_plane = *plane;
        }
    }
}

fn retain_authored_planar_mirrors(batches: &mut [DrawBatch], anchors: &[PortalSurfaceAnchor]) {
    for batch in batches.iter_mut().filter(|batch| batch.planar_reflection) {
        let normal = Vec3::from_array([
            batch.planar_plane[0],
            batch.planar_plane[1],
            batch.planar_plane[2],
        ]);
        let distance = batch.planar_plane[3];
        let matching_anchor = anchors
            .iter()
            .filter_map(|anchor| {
                let plane_distance = (normal.dot(Vec3::from_array(anchor.origin)) + distance).abs();
                (plane_distance <= PORTAL_SURFACE_MAX_DISTANCE).then_some((plane_distance, *anchor))
            })
            .min_by(|(a, _), (b, _)| a.total_cmp(b));
        if let Some((_, anchor)) = matching_anchor.filter(|(_, anchor)| anchor.mirror) {
            batch.planar_pvs_origin = anchor.origin;
        } else {
            // A targeted misc_portal_surface denotes a camera portal, not a
            // mirror. Leave those authored portal surfaces on their material
            // path until the client has a real portal-camera entity renderer.
            batch.planar_reflection = false;
        }
    }
}

fn bsp_portal_surface_anchors(bsp: &Bsp) -> (Vec<PortalSurfaceAnchor>, usize) {
    let mut anchors = Vec::new();
    let mut camera_portals = 0usize;
    for entity in &bsp.entities {
        let Some(classname) = entity
            .get(b"classname")
            .and_then(|value| std::str::from_utf8(value).ok())
        else {
            continue;
        };
        if !classname.eq_ignore_ascii_case("misc_portal_surface") {
            continue;
        }
        let targeted = entity
            .get(b"target")
            .and_then(|value| std::str::from_utf8(value).ok())
            .is_some_and(|value| !value.trim().is_empty());
        if targeted {
            camera_portals += 1;
        }
        let Some(origin) = entity
            .get(b"origin")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(parse_light_triplet)
        else {
            continue;
        };
        anchors.push(PortalSurfaceAnchor {
            origin: render_position(origin),
            mirror: !targeted,
        });
    }
    (anchors, camera_portals)
}

fn map_portal_surface_anchors(document: &MapDocument) -> (Vec<PortalSurfaceAnchor>, usize) {
    let mut anchors = Vec::new();
    let mut camera_portals = 0usize;
    for entity in &document.entities {
        if !entity
            .classname()
            .is_some_and(|classname| classname.eq_ignore_ascii_case("misc_portal_surface"))
        {
            continue;
        }
        let targeted = entity
            .properties
            .get("target")
            .is_some_and(|target| !target.trim().is_empty());
        if targeted {
            camera_portals += 1;
        }
        let Some(origin) = entity
            .properties
            .get("origin")
            .and_then(|value| parse_light_triplet(value))
        else {
            continue;
        };
        anchors.push(PortalSurfaceAnchor {
            origin: render_position(origin),
            mirror: !targeted,
        });
    }
    (anchors, camera_portals)
}

fn prepared_stages(material: &SurfaceMaterial, vertex_lit: bool) -> Vec<MaterialStage> {
    let mut stages = material.stages.clone();
    if !vertex_lit {
        return stages;
    }

    // q3map static models and other LIGHTMAP_BY_VERTEX surfaces store baked
    // lighting in BSP vertex colors. OpenJK substitutes that vertex color for a
    // `$lightmap` stage instead of trying to sample a nonexistent lightmap page.
    if let Some(index) = stages
        .iter()
        .position(|stage| matches!(stage.texture, StageTexture::Lightmap))
    {
        if index == 0 {
            // An initial lightmap stage is folded into the next pass. This mirrors
            // OpenJK's "move lightmap stage down" treatment for vertex-lit data.
            stages.remove(0);
            if let Some(first) = stages.first_mut() {
                first.rgb_gen = RgbGen::ExactVertex;
                first.alpha_gen = AlphaGen::Identity;
                first.blend = None;
            }
        } else if let Some(stage) = stages.get_mut(index) {
            stage.texture = StageTexture::White;
            stage.rgb_gen = RgbGen::ExactVertex;
            stage.alpha_gen = AlphaGen::Identity;
        }
    } else if !material.explicit {
        // Implicit materials normally render base texture × lightmap. For a
        // vertex-lit BSP surface the corresponding operation is base × vertex.
        if let Some(first) = stages.first_mut() {
            first.rgb_gen = RgbGen::ExactVertex;
            first.alpha_gen = AlphaGen::Identity;
        }
    } else if !stages.iter().any(|stage| stage.rgb_gen.uses_vertex_color()) {
        // Rend2 .mtr overrides often replace the classic JKA diffuse stage with
        // a PBR-enhanced diffuse stage (normal/RMO/specular companions) but may
        // omit the legacy rgbGen that consumed q3map's LIGHTMAP_BY_VERTEX data.
        // Do not make every explicit shader vertex-lit: classic explicit shaders
        // can intentionally author identity/fullbright passes. Restrict this
        // compatibility fallback to the primary opaque PBR-enhanced base stage,
        // and leave additive/glow sibling stages untouched.
        if let Some(base) = stages.iter_mut().find(|stage| enhanced_identity_base_stage(stage)) {
            base.rgb_gen = RgbGen::ExactVertex;
            base.alpha_gen = AlphaGen::Identity;
        }
    }
    stages
}

fn has_stage_enhancements(stage: &MaterialStage) -> bool {
    let e = &stage.enhancements;
    e.normal_texture.is_some()
        || e.roughness_texture.is_some()
        || e.height_texture.is_some()
        || e.metallic_texture.is_some()
        || e.specular_texture.is_some()
        || e.emissive_texture.is_some()
        || e.roughness_override.is_some()
        || e.specular_reflectance.is_some()
}

fn enhanced_identity_base_stage(stage: &MaterialStage) -> bool {
    matches!(stage.texture, StageTexture::Image(_))
        && stage.blend.is_none()
        && matches!(stage.tc_gen, TcGen::Base)
        && matches!(stage.rgb_gen, RgbGen::Identity)
        && has_stage_enhancements(stage)
}

fn debug_texture_label(index: Option<usize>, textures: &Textures) -> String {
    index
        .and_then(|index| textures.images.get(index))
        .map(|image| image.label.clone())
        .unwrap_or_else(|| "-".into())
}

fn debug_texture_source(index: Option<usize>, textures: &Textures) -> String {
    index
        .and_then(|index| textures.images.get(index))
        .map(|image| {
            let provider = image
                .source
                .as_deref()
                .map_or_else(|| "<generated / unknown>".to_owned(), |path| path.display().to_string());
            format!("{} @ {provider}", image.label)
        })
        .unwrap_or_else(|| "-".into())
}

fn build_material_debug_entry(
    name: &str,
    definition: Option<&Shader>,
    origin: Option<&materials::ShaderDefinitionOrigin>,
    material: &SurfaceMaterial,
    textures: &Textures,
) -> MaterialDebugEntry {
    let mut lines = Vec::new();
    if let Some(definition) = definition {
        let stage_unsupported = definition
            .stages
            .iter()
            .map(|stage| stage.unsupported.len())
            .sum::<usize>();
        let unsupported = definition.unsupported.len() + stage_unsupported;
        lines.push(format!(
            "parsed shader: stages={} portal={} sky={} nodraw={} translucent={} nofog={} cull={} unsupported={} (shader={} stage={})",
            definition.stages.len(),
            definition.portal,
            definition.sky,
            definition.nodraw,
            definition.translucent,
            definition.no_fog,
            if definition.cull.is_empty() {
                "default"
            } else {
                &definition.cull
            },
            unsupported,
            definition.unsupported.len(),
            stage_unsupported
        ));
        for (index, stage) in definition.stages.iter().enumerate() {
            lines.push(format!(
                "authored stage {index}: map={} blend={} rgbGen={:?} alphaGen={:?} normalMap={} normalHeightMap={} normalScale={} rmoMap={} rmos={} specMap={} roughness={} specularReflectance={} parallaxDepth={}",
                if stage.image.is_empty() { "-" } else { &stage.image },
                if stage.blend.is_empty() { "opaque" } else { &stage.blend },
                stage.rgb_gen,
                stage.alpha_gen,
                stage.normal_map.as_deref().unwrap_or("-"),
                stage.normal_height_map.as_deref().unwrap_or("-"),
                stage.normal_scale
                    .map(|value| format!("{:.3},{:.3}", value[0], value[1]))
                    .unwrap_or_else(|| "-".into()),
                stage.rmo_map.as_deref().unwrap_or("-"),
                stage.rmo_specular_alpha,
                stage.specular_map.as_deref().unwrap_or("-"),
                stage.roughness
                    .map(|value| format!("{value:.4}"))
                    .unwrap_or_else(|| "-".into()),
                stage.specular_reflectance
                    .map(|value| format!("{:.3},{:.3},{:.3}", value[0], value[1], value[2]))
                    .unwrap_or_else(|| "-".into()),
                stage.parallax_depth
                    .map(|value| format!("{value:.4}"))
                    .unwrap_or_else(|| "-".into())
            ));
            if let Some(sprite) = stage.surface_sprite {
                lines.push(format!(
                    "authored stage {index}: surfaceSprites={:?} width={:.1} height={:.1} density={:.1} fade={:.1}..{:.1} fadeScale={:.2} variance={:.2},{:.2} wind={:.2} idle={:.2} vertSkew={:.2} facing={:?}",
                    sprite.kind,
                    sprite.width,
                    sprite.height,
                    sprite.density,
                    sprite.fade_dist,
                    sprite.fade_max,
                    sprite.fade_scale,
                    sprite.variance[0],
                    sprite.variance[1],
                    sprite.wind,
                    sprite.wind_idle,
                    sprite.vert_skew,
                    sprite.facing,
                ));
            }
            for unsupported in &stage.unsupported {
                lines.push(format!("authored stage {index}: UNSUPPORTED {unsupported}"));
            }
        }
    } else {
        lines.push("parsed shader: none (implicit material)".into());
    }

    if let Some(grass) = material.grass {
        lines.push(format!(
            "resolved grass emitter: GodotGrass per-blade path height={:.1} JKA-width-hint={:.1} density={:.1} fade={:.1}..{:.1} wind={:.2}; legacy sprite card suppressed",
            grass.height,
            grass.authored_width,
            grass.density,
            grass.fade_dist,
            grass.fade_max,
            grass.wind,
        ));
    }

    if material.surface_sprite_cull_quirk && material.cull == CullMode::None {
        lines.push(
            "retail surfaceSprites cull quirk: authored twosided -> effective world cull=Back; procedural grass remains two-sided"
                .into(),
        );
    }

    if let Some(surface_light) = material.surface_light {
        lines.push(format!(
            "{} surface emitter: value={:.1} color={:.3},{:.3},{:.3} subdivide={:.1}",
            if surface_light.inferred_from_emissive {
                "emissive-texture inferred"
            } else {
                "q3map authored"
            },
            surface_light.value,
            surface_light.color[0],
            surface_light.color[1],
            surface_light.color[2],
            surface_light.subdivide,
        ));
    }

    if material.stages.len() >= 2
        && can_fold_jka_lightmap_pair(&material.stages[0], &material.stages[1])
    {
        let stage = &material.stages[1];
        let pbr = has_stage_enhancements(stage);
        let pom = stage.enhancements.height_texture.is_some();
        lines.push(format!(
            "effective render: OpenJK-style $lightmap + GL_DST_COLOR/GL_ZERO pair folds to ONE opaque base*lightmap batch; pbr_eligible={pbr} pom_eligible={pom}"
        ));
    }

    let mut enhanced = false;
    let mut image_sources = Vec::new();
    for (index, stage) in material.stages.iter().enumerate() {
        let base = match stage.texture {
            StageTexture::Image(texture) => textures
                .images
                .get(texture)
                .map(|image| image.label.clone())
                .unwrap_or_else(|| format!("<missing texture #{texture}>")),
            StageTexture::Lightmap => "$lightmap".into(),
            StageTexture::White => "$whiteimage".into(),
        };
        let e = &stage.enhancements;
        if let StageTexture::Image(texture) = stage.texture {
            image_sources.push(format!("STAGE {index} BASE IMAGE: {}", debug_texture_source(Some(texture), textures)));
        }
        for (kind, texture) in [
            ("NORMAL", e.normal_texture),
            ("ROUGHNESS", e.roughness_texture),
            ("METALLIC", e.metallic_texture),
            ("SPECULAR", e.specular_texture),
            ("EMISSIVE", e.emissive_texture),
            ("HEIGHT", e.height_texture),
        ] {
            if texture.is_some() {
                image_sources.push(format!("STAGE {index} {kind} IMAGE: {}", debug_texture_source(texture, textures)));
            }
        }
        let stage_enhanced = has_stage_enhancements(stage);
        enhanced |= stage_enhanced;
        let pbr_eligible = matches!(stage.texture, StageTexture::Image(_))
            && stage.blend.is_none()
            && stage.alpha_cutoff == 0.0
            && matches!(stage.tc_gen, TcGen::Base);
        lines.push(format!(
            "resolved stage {index}: base={base} blend={:?} alpha_cutoff={:.3} tcGen={:?} rgbGen={:?} alphaGen={:?} pbr_eligible={} normal={} roughness={} metallic={} specular={} emissive={} height={} height_from_alpha={} rmo_packed={} rmos_alpha={} normalScale={:.3},{:.3} roughnessOverride={} specularReflectance={} parallaxDepth={:.4}",
            stage.blend,
            stage.alpha_cutoff,
            stage.tc_gen,
            stage.rgb_gen,
            stage.alpha_gen,
            pbr_eligible,
            debug_texture_label(e.normal_texture, textures),
            debug_texture_label(e.roughness_texture, textures),
            debug_texture_label(e.metallic_texture, textures),
            debug_texture_label(e.specular_texture, textures),
            debug_texture_label(e.emissive_texture, textures),
            debug_texture_label(e.height_texture, textures),
            e.height_from_alpha,
            e.rmo_packed,
            e.rmo_specular_alpha,
            e.normal_scale[0],
            e.normal_scale[1],
            e.roughness_override
                .map(|value| format!("{value:.4}"))
                .unwrap_or_else(|| "-".into()),
            e.specular_reflectance
                .map(|value| format!("{:.3},{:.3},{:.3}", value[0], value[1], value[2]))
                .unwrap_or_else(|| "-".into()),
            e.parallax_depth,
        ));
        if stage_enhanced && !pbr_eligible {
            lines.push(format!(
                "resolved stage {index}: NOTE companion maps loaded but current stage is not eligible for the PBR/POM shader path"
            ));
        }
    }

    MaterialDebugEntry {
        name: name.to_owned(),
        definition_file: origin.map(|origin| origin.file.clone()),
        definition_source: origin.map(|origin| origin.source.clone()),
        source: origin
            .map(|origin| format!("{} @ {}", origin.file, origin.source.display()))
            .unwrap_or_else(|| "<implicit/no shader definition>".into()),
        mtr_override: origin.is_some_and(|origin| origin.mtr_override),
        enhanced,
        image_sources,
        lines,
    }
}

fn material_debug_info(
    library_debug: &materials::ShaderLibraryDiagnostics,
    names_and_materials: impl IntoIterator<Item = (String, SurfaceMaterial)>,
    library: &BTreeMap<String, Shader>,
    textures: &Textures,
) -> MaterialDebugInfo {
    let mut entries = names_and_materials
        .into_iter()
        .map(|(name, material)| {
            build_material_debug_entry(
                &name,
                library.get(&name),
                library_debug.origins.get(&name),
                &material,
                textures,
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    MaterialDebugInfo {
        shader_files: library_debug.shader_files,
        mtr_files: library_debug.mtr_files,
        shader_definitions: library_debug.shader_definitions,
        mtr_definitions: library_debug.mtr_definitions,
        entries,
    }
}

fn can_fold_jka_lightmap_pair(
    lightmap_stage: &MaterialStage,
    material_stage: &MaterialStage,
) -> bool {
    // OpenJK/JKA treats an opaque $lightmap pass followed by a diffuse pass
    // using GL_DST_COLOR/GL_ZERO as ordinary modulation. Its multitexture
    // collapse swaps the lightmap to bundle 1 and renders diffuse * lightmap.
    // Use our existing one-pass base*lightmap path for the same semantics.
    // This is a legacy shader rule, not a PBR-only optimization.
    matches!(lightmap_stage.texture, StageTexture::Lightmap)
        && lightmap_stage.blend.is_none()
        && lightmap_stage.alpha_cutoff == 0.0
        && lightmap_stage.opacity == 1.0
        && lightmap_stage.color == [1.0; 3]
        && matches!(lightmap_stage.rgb_gen, RgbGen::Identity)
        && matches!(lightmap_stage.alpha_gen, AlphaGen::Identity)
        && matches!(lightmap_stage.tc_gen, TcGen::Lightmap)
        && lightmap_stage.tc_mods.is_empty()
        && !lightmap_stage.depth_equal
        && matches!(material_stage.texture, StageTexture::Image(_))
        && material_stage.blend
            == Some(materials::BlendFunc {
                src: BlendFactor::DstColor,
                dst: BlendFactor::Zero,
            })
        && material_stage.alpha_cutoff == 0.0
        && material_stage.opacity == 1.0
        && material_stage.color == [1.0; 3]
        && matches!(material_stage.rgb_gen, RgbGen::Identity)
        && matches!(material_stage.alpha_gen, AlphaGen::Identity)
        && matches!(material_stage.tc_gen, TcGen::Base)
        && !material_stage.depth_equal
}

/// A sky surface is the outer box (discarded by the shader when the sky has
/// none) followed by one cloud batch per stage of the sky shader. The engine
/// draws them in that order, with each stage's coordinates generated from the
/// view direction. Ordinary same-surface batch ordering keeps the box first
/// and the stages in authored order.
pub(crate) fn append_sky_batches(
    out: &mut Vec<DrawBatch>,
    material: &SurfaceMaterial,
    bsp_shader_index: Option<usize>,
    material_debug_index: Option<usize>,
    vertex_lit: bool,
    range: Range<u32>,
    lightmap: Option<usize>,
    pvs_signature: &[u64],
    area_signature: [u64; 4],
    fog: [f32; 4],
    fog_is_global: bool,
) {
    let box_stage = MaterialStage {
        texture: StageTexture::White,
        enhancements: Default::default(),
        blend: None,
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: true,
        depth_equal: false,
    };
    out.push(stage_batch(
        material,
        &box_stage,
        true,
        vertex_lit,
        range.clone(),
        bsp_shader_index,
        material_debug_index,
        lightmap,
        pvs_signature,
        area_signature,
        fog,
        fog_is_global,
    ));
    for (stage_index, stage) in material.stages.iter().enumerate() {
        let mut cloud = stage.clone();
        cloud.tc_gen = TcGen::SkyCloud(material.sky_cloud_height);
        out.push(stage_batch(
            material,
            &cloud,
            stage_index == 0,
            vertex_lit,
            range.clone(),
            bsp_shader_index,
            material_debug_index,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        ));
    }
}

fn append_material_batches(
    out: &mut Vec<DrawBatch>,
    material: &SurfaceMaterial,
    bsp_shader_index: usize,
    material_debug_index: Option<usize>,
    vertex_lit: bool,
    range: Range<u32>,
    lightmap: Option<usize>,
    pvs_signature: &[u64],
    area_signature: [u64; 4],
    fog: [f32; 4],
    fog_is_global: bool,
) {
    let start = out.len();
    append_material_batches_unflagged(
        out,
        material,
        bsp_shader_index,
        material_debug_index,
        vertex_lit,
        range,
        lightmap,
        pvs_signature,
        area_signature,
        fog,
        fog_is_global,
    );
    // `dlight_in_lightmap_stage` is derived from the authored stages, but the
    // `$lightmap` stage can be folded into the diffuse pass (GL_DST_COLOR pair)
    // or replaced by vertex color. With no lightmap-stage batch left, the
    // shader would skip the base-stage light and nothing would add it: the
    // surface would ignore every runtime/per-pixel light.
    let emitted = &mut out[start..];
    if !emitted.iter().any(|batch| batch.texture_is_lightmap) {
        for batch in emitted {
            batch.dlight_in_lightmap_stage = false;
        }
    }
}

fn append_material_batches_unflagged(
    out: &mut Vec<DrawBatch>,
    material: &SurfaceMaterial,
    bsp_shader_index: usize,
    material_debug_index: Option<usize>,
    vertex_lit: bool,
    range: Range<u32>,
    lightmap: Option<usize>,
    pvs_signature: &[u64],
    area_signature: [u64; 4],
    fog: [f32; 4],
    fog_is_global: bool,
) {
    if material.sky {
        append_sky_batches(
            out,
            material,
            Some(bsp_shader_index),
            material_debug_index,
            vertex_lit,
            range,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        );
        return;
    }

    let stages = prepared_stages(material, vertex_lit);
    if stages.is_empty() {
        return;
    }

    // Keep the overwhelmingly common implicit BSP material in one draw call.
    if !material.explicit && !vertex_lit {
        let mut batch = stage_batch(
            material,
            &stages[0],
            true,
            vertex_lit,
            range,
            Some(bsp_shader_index),
            material_debug_index,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        );
        batch.modulate_lightmap = lightmap.is_some();
        out.push(batch);
        return;
    }

    // A Rend2/PBR override can replace the original lightmapped diffuse stage
    // without carrying an explicit `$lightmap` pass. Preserve classic explicit
    // shader semantics in general, but for a PBR-enhanced identity base stage
    // the BSP lightmap is still the map's baked diffuse lighting source.
    if !vertex_lit
        && lightmap.is_some()
        && material.explicit
        && !stages
            .iter()
            .any(|stage| matches!(stage.texture, StageTexture::Lightmap))
        && stages.iter().any(enhanced_identity_base_stage)
    {
        for (stage_index, stage) in stages.iter().enumerate() {
            let mut batch = stage_batch(
                material,
                stage,
                stage_index == 0,
                vertex_lit,
                range.clone(),
                Some(bsp_shader_index),
                material_debug_index,
                lightmap,
                pvs_signature,
                area_signature,
                fog,
                fog_is_global,
            );
            if enhanced_identity_base_stage(stage) {
                batch.modulate_lightmap = true;
            }
            out.push(batch);
        }
        return;
    }

    if !vertex_lit
        && lightmap.is_some()
        && stages.len() >= 2
        && can_fold_jka_lightmap_pair(&stages[0], &stages[1])
    {
        let mut material_stage = stages[1].clone();
        material_stage.blend = None;
        let mut batch = stage_batch(
            material,
            &material_stage,
            true,
            vertex_lit,
            range.clone(),
            Some(bsp_shader_index),
            material_debug_index,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        );
        batch.modulate_lightmap = true;
        out.push(batch);

        for stage in stages.iter().skip(2) {
            out.push(stage_batch(
                material,
                stage,
                false,
                vertex_lit,
                range.clone(),
                Some(bsp_shader_index),
                material_debug_index,
                lightmap,
                pvs_signature,
                area_signature,
                fog,
                fog_is_global,
            ));
        }
        return;
    }

    // Other explicit shader scripts remain true ordered passes.
    for (stage_index, stage) in stages.iter().enumerate() {
        out.push(stage_batch(
            material,
            stage,
            stage_index == 0,
            vertex_lit,
            range.clone(),
            Some(bsp_shader_index),
            material_debug_index,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        ));
    }
}

#[derive(Debug, Clone, Copy)]
struct SurfaceLightCandidate {
    light: DynamicLight,
    weight: f32,
}

fn surface_light_luminance(color: [f32; 3]) -> f32 {
    color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722
}

fn append_surface_light_triangle(
    candidates: &mut Vec<SurfaceLightCandidate>,
    positions: [Vec3; 3],
    normal: Vec3,
    authored: materials::SurfaceLight,
    two_sided: bool,
) {
    if !authored.value.is_finite() || authored.value <= 0.0 {
        return;
    }

    // q3map2 subdivides emitting draw surfaces before creating its area lights.
    // Do the same geometrically so a large emissive face is represented by
    // several local sources rather than one giant point light. Longest-edge
    // bisection is deterministic and also works for already-tessellated patches.
    let max_edge = authored.subdivide.max(16.0);
    let max_edge_sq = max_edge * max_edge;
    let mut pending = vec![(positions, 0_u8)];
    while let Some((triangle, depth)) = pending.pop() {
        let edge_sq = [
            triangle[0].distance_squared(triangle[1]),
            triangle[1].distance_squared(triangle[2]),
            triangle[2].distance_squared(triangle[0]),
        ];
        let (longest, longest_sq) = edge_sq
            .into_iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .unwrap_or((0, 0.0));
        if longest_sq > max_edge_sq && depth < MAX_SURFACE_LIGHT_SUBDIVISION_DEPTH {
            let (a, b, c) = match longest {
                0 => (triangle[0], triangle[1], triangle[2]),
                1 => (triangle[1], triangle[2], triangle[0]),
                _ => (triangle[2], triangle[0], triangle[1]),
            };
            let midpoint = (a + b) * 0.5;
            pending.push(([a, midpoint, c], depth + 1));
            pending.push(([midpoint, b, c], depth + 1));
            continue;
        }

        let area = 0.5
            * (triangle[1] - triangle[0])
                .cross(triangle[2] - triangle[0])
                .length();
        if !area.is_finite() || area <= 0.01 {
            continue;
        }
        let emitter_normal = normal.normalize_or_zero();
        if emitter_normal.length_squared() <= 1e-6 {
            continue;
        }

        let centroid = (triangle[0] + triangle[1] + triangle[2]) / 3.0;
        // Normalize each generated sample by the authored subdivision cell area.
        // This keeps total emitted energy approximately proportional to surface
        // area and avoids depending on the BSP's incidental triangle density.
        let area_scale = (area / (max_edge * max_edge)).clamp(0.0, 1.0);
        let intensity = ((authored.value / 300.0) * area_scale).clamp(0.0, 8.0);
        if intensity <= 1e-5 {
            continue;
        }
        let radius = authored.value.max(max_edge * 2.0).clamp(64.0, 4096.0);
        let luminance = surface_light_luminance(authored.color).max(0.0);
        candidates.push(SurfaceLightCandidate {
            light: DynamicLight {
                // Keep the sample just in front of its source plane so receiving
                // surfaces that meet the emitter do not start numerically behind it.
                position: (centroid + emitter_normal * 8.0).to_array(),
                color: authored.color,
                radius,
                intensity,
                falloff: DynamicLightFalloff::Smooth,
                surface_lighting: true,
                emitter_normal: emitter_normal.to_array(),
                emitter_two_sided: two_sided,
                angle_attenuation: true,
                angle_scale: 0.0,
                extra_distance: 0.0,
            },
            // Modern mesh-light samplers prioritize emitting triangles by area
            // times luminance. Include q3map's authored strength in that weight.
            weight: area * luminance * authored.value,
        });
    }
}

fn bsp_surface_lights(
    mesh: &jka_assets::bsp::Mesh,
    materials: &[SurfaceMaterial],
) -> (Vec<DynamicLight>, usize) {
    let mut candidates = Vec::new();
    for batch in &mesh.batches {
        let Some(material) = materials.get(batch.shader) else {
            continue;
        };
        let Some(authored) = material.surface_light else {
            continue;
        };
        let two_sided = material.cull == CullMode::None;
        for triangle in mesh.indices[batch.indices.clone()].as_chunks::<3>().0 {
            let vertices = (*triangle).map(|index| &mesh.vertices[index as usize]);
            let positions =
                vertices.map(|vertex| Vec3::from_array(render_position(vertex.position)));
            let normal = vertices
                .map(|vertex| Vec3::from_array(render_position(vertex.normal)))
                .into_iter()
                .sum::<Vec3>()
                .normalize_or_zero();
            append_surface_light_triangle(&mut candidates, positions, normal, authored, two_sided);
        }
    }

    let candidate_count = candidates.len();
    candidates.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    candidates.truncate(MAX_EXTRACTED_SURFACE_LIGHTS);
    (
        candidates
            .into_iter()
            .map(|candidate| candidate.light)
            .collect(),
        candidate_count,
    )
}

fn voxel_gi_index(x: u32, y: u32, z: u32, bounds: [u32; 3]) -> usize {
    (z as usize * bounds[1] as usize + y as usize) * bounds[0] as usize + x as usize
}

fn voxel_gi_coords_for_position(
    position: Vec3,
    minimum: Vec3,
    cell_size: f32,
    bounds: [u32; 3],
) -> Option<[u32; 3]> {
    let relative = (position - minimum) / cell_size;
    if relative.min_element() < 0.0 {
        return None;
    }
    let coords = [
        relative.x.floor() as u32,
        relative.y.floor() as u32,
        relative.z.floor() as u32,
    ];
    (coords[0] < bounds[0] && coords[1] < bounds[1] && coords[2] < bounds[2]).then_some(coords)
}

fn voxel_gi_nearest_free(
    position: Vec3,
    minimum: Vec3,
    cell_size: f32,
    bounds: [u32; 3],
    occupied: &[bool],
) -> Option<usize> {
    let base = voxel_gi_coords_for_position(position, minimum, cell_size, bounds)?;
    let mut best = None::<(usize, f32)>;
    for radius in 0_i32..=3 {
        for dz in -radius..=radius {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()).max(dz.abs()) != radius {
                        continue;
                    }
                    let x = base[0] as i32 + dx;
                    let y = base[1] as i32 + dy;
                    let z = base[2] as i32 + dz;
                    if x < 0
                        || y < 0
                        || z < 0
                        || x >= bounds[0] as i32
                        || y >= bounds[1] as i32
                        || z >= bounds[2] as i32
                    {
                        continue;
                    }
                    let index = voxel_gi_index(x as u32, y as u32, z as u32, bounds);
                    if occupied[index] {
                        continue;
                    }
                    let center = minimum
                        + Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) * cell_size;
                    let distance = center.distance_squared(position);
                    if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                        best = Some((index, distance));
                    }
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    best.map(|(index, _)| index)
}

fn voxel_gi_add_source(target: &mut [Vec3], index: usize, energy: Vec3) {
    if let Some(value) = target.get_mut(index) {
        *value = (*value + energy).min(Vec3::splat(VOXEL_GI_MAX_RADIANCE));
    }
}

fn voxel_gi_add_source_max(target: &mut [Vec3], index: usize, energy: Vec3) {
    if let Some(value) = target.get_mut(index) {
        *value = (*value).max(energy).min(Vec3::splat(VOXEL_GI_MAX_RADIANCE));
    }
}

fn voxel_gi_ray_reaches_outside(
    start: Vec3,
    direction: Vec3,
    minimum: Vec3,
    cell_size: f32,
    bounds: [u32; 3],
    occupied: &[bool],
) -> bool {
    let direction = direction.normalize_or_zero();
    if direction.length_squared() <= 1e-6 {
        return false;
    }
    let mut position = start;
    let step = cell_size * 0.55;
    let max_steps = ((bounds[0] + bounds[1] + bounds[2]) * 4).max(16);
    let mut previous = None;
    for _ in 0..max_steps {
        position += direction * step;
        let Some([x, y, z]) = voxel_gi_coords_for_position(position, minimum, cell_size, bounds)
        else {
            return true;
        };
        let index = voxel_gi_index(x, y, z, bounds);
        if previous == Some(index) {
            continue;
        }
        previous = Some(index);
        if occupied[index] {
            return false;
        }
    }
    false
}

fn voxel_gi_propagate(source: &[Vec3], occupied: &[bool], bounds: [u32; 3]) -> Vec<Vec3> {
    let mut current = source.to_vec();
    let mut next = vec![Vec3::ZERO; source.len()];
    for _ in 0..VOXEL_GI_PROPAGATION_STEPS {
        next.copy_from_slice(source);
        for z in 0..bounds[2] {
            for y in 0..bounds[1] {
                for x in 0..bounds[0] {
                    let index = voxel_gi_index(x, y, z, bounds);
                    if occupied[index] {
                        next[index] = Vec3::ZERO;
                        continue;
                    }
                    let mut neighbours = Vec3::ZERO;
                    if x > 0 {
                        neighbours += current[voxel_gi_index(x - 1, y, z, bounds)];
                    }
                    if x + 1 < bounds[0] {
                        neighbours += current[voxel_gi_index(x + 1, y, z, bounds)];
                    }
                    if y > 0 {
                        neighbours += current[voxel_gi_index(x, y - 1, z, bounds)];
                    }
                    if y + 1 < bounds[1] {
                        neighbours += current[voxel_gi_index(x, y + 1, z, bounds)];
                    }
                    if z > 0 {
                        neighbours += current[voxel_gi_index(x, y, z - 1, bounds)];
                    }
                    if z + 1 < bounds[2] {
                        neighbours += current[voxel_gi_index(x, y, z + 1, bounds)];
                    }
                    next[index] = (source[index] + neighbours * (VOXEL_GI_PROPAGATION / 6.0))
                        .min(Vec3::splat(VOXEL_GI_MAX_RADIANCE));
                }
            }
        }
        std::mem::swap(&mut current, &mut next);
    }
    current
        .into_iter()
        .zip(source)
        .zip(occupied)
        .map(|((field, source), occupied)| {
            if *occupied {
                Vec3::ZERO
            } else {
                (field - *source * 0.75).max(Vec3::ZERO)
            }
        })
        .collect()
}

fn build_voxel_probe_gi(
    vertices: &[GpuVertex],
    batches: &[DrawBatch],
    lights: &[DynamicLight],
    sun: Option<DirectionalSun>,
) -> Option<VoxelProbeGi> {
    if vertices.is_empty() || (lights.is_empty() && sun.is_none()) {
        return None;
    }

    let mut ranges = BTreeSet::<(u32, u32)>::new();
    for batch in batches {
        if !matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask) {
            continue;
        }
        ranges.insert((batch.vertices.start, batch.vertices.end));
    }
    if ranges.is_empty() {
        return None;
    }

    let mut world_min = Vec3::splat(f32::INFINITY);
    let mut world_max = Vec3::splat(f32::NEG_INFINITY);
    for &(start, end) in &ranges {
        for vertex in &vertices[start as usize..(end as usize).min(vertices.len())] {
            let position = Vec3::from_array(vertex.position);
            if position.is_finite() {
                world_min = world_min.min(position);
                world_max = world_max.max(position);
            }
        }
    }
    if !world_min.is_finite() || !world_max.is_finite() {
        return None;
    }

    let extent = (world_max - world_min).max(Vec3::splat(1.0));
    let longest = extent.max_element();
    let cell_size = (longest / (VOXEL_GI_MAX_AXIS as f32 - 4.0)).max(VOXEL_GI_MIN_CELL_SIZE);
    let minimum = world_min - Vec3::splat(cell_size * 1.5);
    let maximum = world_max + Vec3::splat(cell_size * 1.5);
    let padded_extent = maximum - minimum;
    let bounds = [
        ((padded_extent.x / cell_size).ceil() as u32).clamp(4, VOXEL_GI_MAX_AXIS),
        ((padded_extent.y / cell_size).ceil() as u32).clamp(4, VOXEL_GI_MAX_AXIS),
        ((padded_extent.z / cell_size).ceil() as u32).clamp(4, VOXEL_GI_MAX_AXIS),
    ];
    let cell_count = bounds[0] as usize * bounds[1] as usize * bounds[2] as usize;
    let mut occupied = vec![false; cell_count];
    let mut surface_normals = vec![Vec3::ZERO; cell_count];

    // Conservative surface voxelization. Sampling at <= half-cell spacing keeps
    // thin BSP walls continuous without the large overfill caused by rasterizing
    // each triangle's full voxel AABB.
    for &(start, end) in &ranges {
        let slice = &vertices[start as usize..(end as usize).min(vertices.len())];
        for triangle in slice.chunks_exact(3) {
            let p0 = Vec3::from_array(triangle[0].position);
            let p1 = Vec3::from_array(triangle[1].position);
            let p2 = Vec3::from_array(triangle[2].position);
            if !(p0.is_finite() && p1.is_finite() && p2.is_finite()) {
                continue;
            }
            let triangle_normal = triangle
                .iter()
                .map(|vertex| Vec3::from_array(vertex.normal))
                .sum::<Vec3>()
                .normalize_or_zero();
            let longest_edge = p0.distance(p1).max(p1.distance(p2)).max(p2.distance(p0));
            let steps = ((longest_edge / (cell_size * 0.48)).ceil() as usize).clamp(1, 128);
            for i in 0..=steps {
                for j in 0..=(steps - i) {
                    let a = i as f32 / steps as f32;
                    let b = j as f32 / steps as f32;
                    let c = 1.0 - a - b;
                    let point = p0 * c + p1 * a + p2 * b;
                    if let Some([x, y, z]) =
                        voxel_gi_coords_for_position(point, minimum, cell_size, bounds)
                    {
                        let index = voxel_gi_index(x, y, z, bounds);
                        occupied[index] = true;
                        surface_normals[index] += triangle_normal;
                    }
                }
            }
            let centroid = (p0 + p1 + p2) / 3.0;
            if let Some([x, y, z]) =
                voxel_gi_coords_for_position(centroid, minimum, cell_size, bounds)
            {
                let index = voxel_gi_index(x, y, z, bounds);
                occupied[index] = true;
                surface_normals[index] += triangle_normal;
            }
        }
    }

    let occupied_voxels = occupied.iter().filter(|occupied| **occupied).count();
    let mut point_source = vec![Vec3::ZERO; cell_count];
    let mut area_source = vec![Vec3::ZERO; cell_count];
    let mut deposited = 0_usize;

    // Treat direct sun on exposed world surfaces as the first bounce source.
    // The coarse voxel ray is intentionally map-load work; runtime shading only
    // pays for the resulting filtered probe sample.
    if let Some(sun) = sun.filter(|sun| sun.intensity.is_finite() && sun.intensity > 0.0) {
        let to_sun = -Vec3::from_array(sun.direction).normalize_or_zero();
        let sun_color = Vec3::from_array(sun.color).max(Vec3::ZERO);
        let sun_energy = sun_color * (sun.intensity / 300.0).clamp(0.05, 4.0) * 0.55;
        for z in 0..bounds[2] {
            for y in 0..bounds[1] {
                for x in 0..bounds[0] {
                    let surface_index = voxel_gi_index(x, y, z, bounds);
                    if !occupied[surface_index] {
                        continue;
                    }
                    let normal = surface_normals[surface_index].normalize_or_zero();
                    let facing = normal.dot(to_sun).max(0.0);
                    if facing <= 0.05 {
                        continue;
                    }
                    let surface_center = minimum
                        + Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) * cell_size;
                    let source_position = surface_center + normal * cell_size * 0.72;
                    let Some(source_index) = voxel_gi_nearest_free(
                        source_position,
                        minimum,
                        cell_size,
                        bounds,
                        &occupied,
                    ) else {
                        continue;
                    };
                    let source_center = {
                        let sx = source_index % bounds[0] as usize;
                        let sy = (source_index / bounds[0] as usize) % bounds[1] as usize;
                        let sz = source_index / (bounds[0] as usize * bounds[1] as usize);
                        minimum
                            + Vec3::new(sx as f32 + 0.5, sy as f32 + 0.5, sz as f32 + 0.5)
                                * cell_size
                    };
                    if !voxel_gi_ray_reaches_outside(
                        source_center,
                        to_sun,
                        minimum,
                        cell_size,
                        bounds,
                        &occupied,
                    ) {
                        continue;
                    }
                    voxel_gi_add_source_max(&mut point_source, source_index, sun_energy * facing);
                    deposited += 1;
                }
            }
        }
    }

    for light in lights {
        if !light.intensity.is_finite() || light.intensity <= 0.0 {
            continue;
        }
        let normal = Vec3::from_array(light.emitter_normal).normalize_or_zero();
        let is_area = normal.length_squared() > 1e-6;
        let radius_scale = (light.radius.max(64.0) / 512.0).sqrt().clamp(0.45, 1.75);
        // Source-map q3map lights store compiler photons/255 in `intensity` so
        // direct rendering can match baked luxels. Probe-GI authoring historically
        // expects the engine's ~light/300 scale; convert only those compiler lights
        // back so enabling source-map preview cannot saturate the GI volume.
        let gi_intensity = if light.falloff == DynamicLightFalloff::Smooth {
            light.intensity
        } else {
            light.intensity * Q3MAP_LIGHTMAP_BYTE_SCALE / (Q3MAP_POINT_SCALE * 300.0)
        };
        let energy = Vec3::from_array(light.color).max(Vec3::ZERO)
            * gi_intensity
            * radius_scale
            * if is_area { 0.65 } else { 0.5 };
        if energy.max_element() <= 1e-5 {
            continue;
        }

        let position = Vec3::from_array(light.position);
        let mut positions = Vec::with_capacity(2);
        if is_area {
            positions.push(position + normal * cell_size * 0.65);
            if light.emitter_two_sided {
                positions.push(position - normal * cell_size * 0.65);
            }
        } else {
            positions.push(position);
        }
        for source_position in positions {
            let Some(index) =
                voxel_gi_nearest_free(source_position, minimum, cell_size, bounds, &occupied)
            else {
                continue;
            };
            if is_area {
                voxel_gi_add_source(&mut area_source, index, energy);
            } else {
                voxel_gi_add_source(&mut point_source, index, energy);
            }
            deposited += 1;
        }
    }
    if deposited == 0 {
        return None;
    }

    let point_field = voxel_gi_propagate(&point_source, &occupied, bounds);
    let area_field = voxel_gi_propagate(&area_source, &occupied, bounds);
    let to_rgba = |field: Vec<Vec3>| {
        field
            .into_iter()
            .zip(&occupied)
            .map(|(value, occupied)| {
                let encode = |channel: f32| {
                    ((channel.clamp(0.0, VOXEL_GI_MAX_RADIANCE) / VOXEL_GI_MAX_RADIANCE * 255.0)
                        + 0.5) as u8
                };
                [
                    encode(value.x),
                    encode(value.y),
                    encode(value.z),
                    if *occupied { 0 } else { 255 },
                ]
            })
            .collect::<Vec<_>>()
    };

    Some(VoxelProbeGi {
        origin: (minimum + Vec3::splat(cell_size * 0.5)).to_array(),
        cell_size,
        bounds,
        point_rgba: to_rgba(point_field),
        area_rgba: to_rgba(area_field),
        occupied_voxels,
    })
}

const STATIC_BSP_AO_CACHE_VERSION: u32 = 11;

#[derive(Debug, Clone)]
pub struct StaticBspAoCacheInfo {
    pub directory: PathBuf,
    pub map_name: String,
    pub map_hash: u64,
    pub version: u32,
}

fn map_content_hash(bytes: &[u8]) -> u64 {
    // Deterministic FNV-1a: unlike DefaultHasher this remains stable across runs.
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Fingerprints of the option-dependent stage inputs a prepared map was built
/// from. A later preparation of the same map compares its own inputs against
/// these and, on a match, copies the finished result out of the previous map
/// (the app's restart cache) instead of recomputing it. A hit is therefore
/// bit-identical to a rebuild; a changed input recomputes only that stage.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrepStageKeys {
    pub grass: Option<u64>,
    pub gi: Option<u64>,
    pub portal: Option<u64>,
}

/// Input fingerprint for a reusable stage. `DefaultHasher::new()` is
/// deterministic within a process, which is all an in-memory comparison needs.
struct StageFingerprint(FoldHasher);

/// 64-bit multiply-fold hasher (wyhash style): one 128-bit multiply per eight
/// bytes. Fingerprints only ever compare against another fingerprint made by
/// this process, so speed matters and stability across builds does not; SipHash
/// was the bulk of the time spent keying stages over millions of vertices.
#[derive(Default)]
pub(crate) struct FoldHasher {
    state: u64,
    length: u64,
}

impl FoldHasher {
    const K1: u64 = 0x9e37_79b9_7f4a_7c15;
    const K2: u64 = 0xd6e8_feb8_6659_fd93;

    #[inline]
    fn round(&mut self, word: u64) {
        let product = u128::from(self.state ^ word ^ Self::K1) * u128::from(Self::K2);
        self.state = (product as u64) ^ ((product >> 64) as u64);
    }
}

impl std::hash::Hasher for FoldHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.length = self.length.wrapping_add(bytes.len() as u64);
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.round(u64::from_le_bytes(chunk.try_into().expect("chunk of eight")));
        }
        let tail = chunks.remainder();
        if !tail.is_empty() {
            let mut padded = [0_u8; 8];
            padded[..tail.len()].copy_from_slice(tail);
            // The tail length keeps "ab" and "ab\0" apart.
            self.round(u64::from_le_bytes(padded) ^ ((tail.len() as u64) << 59));
        }
    }

    fn finish(&self) -> u64 {
        let product = u128::from(self.state ^ self.length) * u128::from(Self::K2);
        (product as u64) ^ ((product >> 64) as u64)
    }
}

impl StageFingerprint {
    fn new(stage: &str) -> Self {
        use std::hash::Hasher;
        let mut hasher = FoldHasher::default();
        hasher.write(stage.as_bytes());
        Self(hasher)
    }

    /// Hash draw batches. Their `Debug` text is cheap except for the PVS
    /// signature (dozens of words per batch), so each signature is set aside,
    /// hashed as raw bytes, and put back.
    fn draw_batches(&mut self, batches: &mut [DrawBatch]) -> &mut Self {
        use std::hash::Hasher;
        self.0.write_usize(batches.len());
        for batch in batches {
            let signature = std::mem::take(&mut batch.pvs_signature);
            self.debug(&*batch);
            self.pod(&signature);
            batch.pvs_signature = signature;
        }
        self
    }

    /// Hash vertex positions only, in blocks so the hasher sees large writes.
    fn positions(&mut self, vertices: &[GpuVertex]) -> &mut Self {
        let mut block = [[0.0_f32; 3]; 512];
        for chunk in vertices.chunks(block.len()) {
            for (slot, vertex) in block.iter_mut().zip(chunk) {
                *slot = vertex.position;
            }
            self.pod(&block[..chunk.len()]);
        }
        self
    }

    fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        use std::hash::Hasher;
        self.0.write_usize(bytes.len());
        self.0.write(bytes);
        self
    }

    fn pod<T: Pod>(&mut self, values: &[T]) -> &mut Self {
        self.bytes(bytemuck::cast_slice(values))
    }

    /// Hash a value through its `Debug` text without allocating it.
    fn debug<T: std::fmt::Debug + ?Sized>(&mut self, value: &T) -> &mut Self {
        use std::fmt::Write;
        let _ = write!(self, "{value:?}");
        self
    }

    fn finish(&self) -> u64 {
        use std::hash::Hasher;
        self.0.finish()
    }
}

impl std::fmt::Write for StageFingerprint {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        use std::hash::Hasher;
        self.0.write(text.as_bytes());
        Ok(())
    }
}

fn static_bsp_ao_cache_info(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    map_hash: u64,
) -> StaticBspAoCacheInfo {
    StaticBspAoCacheInfo {
        directory: active_game_directory(root, game)
            .join("jka-rust-cache")
            .join("static-bsp-ao"),
        map_name: name.to_string(),
        map_hash,
        version: STATIC_BSP_AO_CACHE_VERSION,
    }
}

pub fn prepare(root: &Path, game: Option<&Path>, name: &str) -> Result<PreparedMap, String> {
    prepare_internal(root, game, name, None, MapPrepareOptions::default(), None)
}

pub fn prepare_with_options(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    prepare_internal(root, game, name, None, options, None)
}

/// `prepare_with_options` that reuses unchanged stages of an earlier preparation.
#[cfg(test)]
pub fn prepare_with_seed(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    options: MapPrepareOptions,
    seed: &Arc<PreparedMap>,
) -> Result<PreparedMap, String> {
    prepare_internal(root, game, name, None, options, Some(seed))
}

pub fn prepare_with_jobs_options(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
    seed: Option<&Arc<PreparedMap>>,
) -> Result<PreparedMap, String> {
    prepare_internal(root, game, name, Some(jobs), options, seed)
}

/// Records a stage lent by the seed and shows it as finished on the loading panel.
fn note_reused_stage(
    jobs: Option<&MapJobPool>,
    reused: &mut Vec<&'static str>,
    label: &'static str,
    task: Task,
) {
    reused.push(label);
    if let Some(jobs) = jobs {
        jobs.mark_reused(task);
    }
}

fn prepare_internal(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
    seed: Option<&Arc<PreparedMap>>,
) -> Result<PreparedMap, String> {
    let prepare_started = Instant::now();
    let mut load_timings = MapLoadTimings {
        worker_count: jobs.map_or(1, MapJobPool::worker_count),
        ..Default::default()
    };
    let asset_name = map_asset_name(name, "bsp")?;
    let archive_opens_before = jka_assets::pk3::archive_open_stats();

    let stage = Instant::now();
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    assets.set_allow_asset_overrides(options.allow_asset_overrides);
    load_timings.asset_index_ms = stage.elapsed().as_secs_f64() * 1000.0;

    let stage = Instant::now();
    let asset = assets
        .read(&asset_name, MAX_FILE_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Map {asset_name} not found on the game/base asset search path"))?;
    load_timings.bsp_read_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let asset_source = asset.source.clone();
    let bytes = Arc::new(asset.bytes);
    // FNV-1a over the whole BSP is a serial byte chain (tens of ms on a big map),
    // and its value names on-disk caches so it cannot change. With a worker pool
    // and no seed to validate, it runs beside the BSP parse on an idle worker and
    // is joined once the parse is done; otherwise it is computed right here.
    let hash_job = match jobs {
        Some(jobs) if seed.is_none() => {
            let hash_bytes = Arc::clone(&bytes);
            Some(jobs.submit(Task::MapPrepare, move || map_content_hash(hash_bytes.as_slice()))?)
        }
        _ => None,
    };
    let inline_hash = hash_job.is_none().then(|| map_content_hash(bytes.as_slice()));

    let mut warnings = Vec::new();
    // A previous preparation of this same map (the app's restart cache) lends
    // every stage whose inputs are unchanged, so a forced re-prepare does not
    // redo them. Nothing extra is stored: the seed is data the app already holds.
    let seed = seed.filter(|seed| inline_hash == Some(seed.map_hash));
    let mut reused = Vec::<&'static str>::new();
    // Collision and the acoustic mesh read only the BSP bytes.
    let seed_collision = seed.and_then(|seed| seed.collision.clone());
    let seed_acoustic = seed.and_then(|seed| seed.steam_audio_acoustic_mesh.clone());
    if seed_collision.is_some() {
        note_reused_stage(jobs, &mut reused, "collision", Task::MapCollision);
    }

    let mut steam_audio_job = None;
    let mut steam_audio_sync = None;
    // The collision world is not needed until after texture preload has been
    // queued, so with a worker pool it is joined later (see below) instead of
    // stalling the loader right after the BSP parse.
    let mut collision_pending = None;
    let (bsp, mesh, collision) = if let Some(jobs) = jobs {
        let parse_bytes = Arc::clone(&bytes);
        let parse = jobs.submit(Task::MapBspParse, move || {
            let started = Instant::now();
            let result = Bsp::parse(parse_bytes.as_slice())
                .map_err(|error| error.to_string())
                .and_then(|bsp| {
                    let mesh = bsp.world_mesh(4).map_err(|error| error.to_string())?;
                    Ok((Arc::new(bsp), Arc::new(mesh)))
                });
            (result, started.elapsed().as_secs_f64() * 1000.0)
        })?;

        if seed_collision.is_none() {
            let collision_bytes = Arc::clone(&bytes);
            collision_pending = Some(jobs.submit(Task::MapCollision, move || {
                let started = Instant::now();
                let result = jka_movement::CollisionWorld::from_bsp(collision_bytes.as_slice());
                (result, started.elapsed().as_secs_f64() * 1000.0)
            })?);
        }

        // While BSP/collision CPU work is running, keep the loader thread useful:
        // animation.cfg IO and shader-file reads overlap those worker jobs.
        let movement_stage = Instant::now();
        let movement = load_movement(&mut assets, &mut warnings);
        load_timings.movement_ms = movement_stage.elapsed().as_secs_f64() * 1000.0;
        let (library, library_debug) =
            materials::shader_library_with_jobs(&mut assets, &mut warnings, jobs, options.pbr_materials)?;
        load_timings.shader_read_ms = library_debug.read_ms;
        load_timings.shader_parse_wall_ms = library_debug.parse_wall_ms;
        load_timings.shader_parse_cpu_ms = library_debug.parse_cpu_ms;

        // Workers idle while the parse finishes. Have a few open the retail
        // archives so the parallel texture reads that follow find ready handles
        // (opening one parses its whole central directory). Queued after the
        // shader library so it cannot delay shader parsing.
        for _ in 0..jobs.worker_count().saturating_sub(3).min(5) {
            let mut warm = assets.fork();
            jobs.submit(Task::MapPrepare, move || warm.warm_stock_archives())?;
        }
        let (parsed, parse_ms) = parse.join()?;
        load_timings.bsp_parse_ms = parse_ms;
        let (bsp, mesh) = parsed?;
        if options.steam_audio {
            if let Some(hit) = seed_acoustic.clone() {
                note_reused_stage(Some(jobs), &mut reused, "acoustic mesh", Task::MapAcoustics);
                steam_audio_sync = Some((Ok(hit), 0.0));
            } else {
                let acoustic_bsp = Arc::clone(&bsp);
                steam_audio_job = Some(jobs.submit(Task::MapAcoustics, move || {
                    let started = Instant::now();
                    let result = acoustic_bsp
                        .world_acoustic_mesh(4)
                        .map(Arc::new)
                        .map_err(|error| error.to_string());
                    (result, started.elapsed().as_secs_f64() * 1000.0)
                })?);
            }
        }
        (bsp, mesh, (seed_collision, movement, library, library_debug))
    } else {
        let stage = Instant::now();
        let bsp = Arc::new(Bsp::parse(bytes.as_slice()).map_err(|e| e.to_string())?);
        let mesh = Arc::new(bsp.world_mesh(4).map_err(|e| e.to_string())?);
        load_timings.bsp_parse_ms = stage.elapsed().as_secs_f64() * 1000.0;

        let stage = Instant::now();
        let collision = if let Some(hit) = seed_collision {
            Some(hit)
        } else {
            match jka_movement::CollisionWorld::from_bsp(bytes.as_slice()) {
                Ok(world) => Some(world),
                Err(error) => {
                    warnings.push(format!("Player collision unavailable: {error}"));
                    None
                }
            }
        };
        load_timings.collision_ms = stage.elapsed().as_secs_f64() * 1000.0;

        if options.steam_audio {
            if let Some(hit) = seed_acoustic.clone() {
                note_reused_stage(jobs, &mut reused, "acoustic mesh", Task::MapAcoustics);
                steam_audio_sync = Some((Ok(hit), 0.0));
            } else {
                let acoustic_stage = Instant::now();
                let result = bsp
                    .world_acoustic_mesh(4)
                    .map(Arc::new)
                    .map_err(|error| error.to_string());
                steam_audio_sync = Some((
                    result,
                    acoustic_stage.elapsed().as_secs_f64() * 1000.0,
                ));
            }
        }

        let stage = Instant::now();
        let movement = load_movement(&mut assets, &mut warnings);
        load_timings.movement_ms = stage.elapsed().as_secs_f64() * 1000.0;
        let (library, library_debug) = materials::shader_library(&mut assets, &mut warnings, options.pbr_materials)?;
        load_timings.shader_read_ms = library_debug.read_ms;
        load_timings.shader_parse_wall_ms = library_debug.parse_wall_ms;
        load_timings.shader_parse_cpu_ms = library_debug.parse_cpu_ms;
        (bsp, mesh, (collision, movement, library, library_debug))
    };
    let (collision, movement, library, library_debug) = collision;
    let map_hash = match hash_job {
        Some(job) => job.join()?,
        None => inline_hash.expect("map hash computed inline without a hash job"),
    };
    let static_bsp_ao_cache = Some(static_bsp_ao_cache_info(root, game, name, map_hash));
    let steam_audio_bake_cache = options
        .steam_audio
        .then(|| crate::steam_audio::cache_info(root, game, name, map_hash));
    let mut phase_clock = prepare_started;
    phase_lap(&mut load_timings, 0, &mut phase_clock);
    let (inline_mesh, inline_batch_models) =
        bsp.inline_models_mesh(4).map_err(|error| error.to_string())?;

    // Which shaders (and so which textures) the map uses needs only the meshes and
    // the shader library. Queue texture reads and decodes on the workers right now
    // so they overlap everything the loader does until `preload_finish`.
    let used: BTreeSet<_> = mesh
        .batches
        .iter()
        .chain(&inline_mesh.batches)
        .map(|batch| batch.shader)
        .collect();
    let used_shader_names: Vec<String> = used
        .iter()
        .filter_map(|&index| bsp.shaders.get(index))
        .map(|shader| String::from_utf8_lossy(&shader.name).to_ascii_lowercase())
        .collect();
    let mut textures = Textures::new();
    if let Some(seed) = seed {
        textures.set_seed(materials::TextureSeed::new(seed));
    }
    let texture_preload = if let Some(jobs) = jobs {
        let requests = materials::primary_texture_requests(
            &used_shader_names,
            &library,
            options.omit_environment_stages,
        );
        Some(textures.preload_start(&assets, &requests, jobs)?)
    } else {
        None
    };
    // By now the collision job has had the whole mesh build to finish.
    let collision = if let Some(job) = collision_pending {
        let (collision_result, collision_ms) = job.join()?;
        load_timings.collision_ms = collision_ms;
        match collision_result {
            Ok(world) => Some(world),
            Err(error) => {
                warnings.push(format!("Player collision unavailable: {error}"));
                None
            }
        }
    } else {
        collision
    };

    let physics_collision = if options.client_physics {
        bsp_physics_collision_mesh(&bsp, &mesh)
    } else {
        crate::cgame::ragdoll::PhysicsMapMesh::default()
    };
    let weather_occlusion = collision
        .as_ref()
        .and_then(|_| bsp_weather_occlusion_source(&bsp, &mesh));
    // R_MarkFragments reads the runtime shader's flags, so the scripts win over
    // the flags compiled into the BSP shader lump.
    let mark_shader_flags: Vec<(u32, u32)> = bsp
        .shaders
        .iter()
        .map(|shader| {
            let name = String::from_utf8_lossy(&shader.name).to_ascii_lowercase();
            library.get(&name).map_or((shader.surface_flags, shader.contents), |script| {
                (script.collision_surface_flags_add, script.collision_contents_add)
            })
        })
        .collect();
    let mark_surfaces = Arc::new(jka_assets::bsp::MarkSurfaces::from_bsp(&bsp, &mesh, &mark_shader_flags));

    let bsp_stats = BspMapStats {
        inline_model_midpoints: bsp
            .models
            .iter()
            .map(|model| std::array::from_fn(|i| (model.mins[i] + model.maxs[i]) * 0.5))
            .collect(),
        inline_model_bounds: bsp.models.iter().map(|model| (model.mins, model.maxs)).collect(),
        brushes: bsp.brushes.len(),
        brush_sides: bsp.brush_sides.len(),
        planes: bsp.planes.len(),
        surfaces: bsp.surfaces.len(),
        vertices: bsp.vertices.len(),
        indices: bsp.indices.len(),
        collision_nodes: bsp.collision.as_ref().map_or(0, |tree| tree.nodes.len()),
        collision_leaves: bsp.collision.as_ref().map_or(0, |tree| tree.leaves.len()),
        pvs_clusters: bsp
            .visibility
            .as_ref()
            .map_or(0, |visibility| visibility.clusters),
    };
    let distance_cull = bsp_worldspawn_distance_cull(&bsp, &mut warnings);
    let sky_portal = bsp_sky_portal(&bsp);
    let global_fog_num = bsp_global_fog_num(&bsp);
    let global_fog = bsp_global_fog_params(&bsp, &library);
    if let Some(fog) = global_fog {
        println!(
            "{name}: BSP global fog rgb=({:.3}, {:.3}, {:.3}) depth={:.1}",
            fog[0], fog[1], fog[2], fog[3]
        );
    }
    phase_lap(&mut load_timings, 1, &mut phase_clock);
    let lightmap_stage = Instant::now();
    let embedded_lightmap_pages = bsp.lightmaps.len() / (128 * 128 * 3);
    let mut referenced_lightmaps = external_lightmap_pages(&mesh);
    referenced_lightmaps.extend(external_lightmap_pages(&inline_mesh));
    let world_deluxe_mapping = embedded_deluxe_mapping(&mesh, embedded_lightmap_pages);
    let hdr_lightmaps_available =
        options.float_lightmap && hdr_lightmap_indexed(&assets, name);

    let mesh_uvs_bounded = |mesh: &jka_assets::bsp::Mesh| {
        mesh.batches.iter().all(|batch| {
            let slot = (0..4).find(|&i| batch.lightmaps[i] >= 0 && batch.lightmap_styles[i] < 254);
            slot.is_none_or(|slot| {
                mesh.indices[batch.indices.clone()].iter().all(|&index| {
                    mesh.vertices[index as usize].lightmap_uv[slot]
                        .iter()
                        .all(|v| (0.0..=1.0).contains(v))
                })
            })
        })
    };
    // Inline models share the world's lightmap pages, so they must also fit
    // the atlas before it can replace per-page lightmaps.
    let bounded_uvs = mesh_uvs_bounded(&mesh) && mesh_uvs_bounded(&inline_mesh);
    // An atlas cannot preserve the q3map2 lightmap/deluxemap pairing or FP16
    // companion values without building a second typed atlas. Keep the classic
    // atlas for ordinary 8-bit maps and use per-page textures for richer data.
    let atlas = (embedded_lightmap_pages > 0
        && bounded_uvs
        && !world_deluxe_mapping
        && !options.float_lightmap)
        .then(|| Atlas::new(&bsp.lightmaps));

    let mut external_lightmaps = Vec::new();
    let mut external_deluxemaps = Vec::new();
    let mut external_lightmap_lookup = BTreeMap::new();
    if embedded_lightmap_pages == 0 && !referenced_lightmaps.is_empty() {
        let paired_external = referenced_lightmaps.iter().all(|page| page % 2 == 0);
        let mut deluxe_count = 0usize;
        for &page in &referenced_lightmaps {
            let gpu_index = external_lightmaps.len();
            let image = load_external_lightmap(
                &mut assets,
                name,
                page,
                options.float_lightmap,
            )?;
            let deluxe = try_load_external_deluxemap(
                &mut assets,
                name,
                page,
                paired_external,
            )?;
            if deluxe.is_some() {
                deluxe_count += 1;
            }
            external_lightmaps.push(image);
            external_deluxemaps.push(
                deluxe.unwrap_or_else(|| neutral_deluxemap(format!("{name} neutral deluxe {page}"))),
            );
            external_lightmap_lookup.insert(page, gpu_index);
        }
        warnings.push(format!(
            "{name}: using {} external lightmap page(s) from maps/{name}/lm_XXXX{}",
            external_lightmaps.len(),
            if deluxe_count != 0 {
                format!("; {deluxe_count} directional deluxemap companion(s)")
            } else {
                String::new()
            }
        ));
    } else if world_deluxe_mapping {
        warnings.push(format!(
            "{name}: q3map2/Rend2 embedded deluxemaps detected ({} lightmap + direction pairs)",
            embedded_lightmap_pages / 2
        ));
    }
    if options.float_lightmap {
        warnings.push(if hdr_lightmaps_available {
            format!(
                "{name}: r_floatLightmap: HDR lm_XXXX.hdr companions detected; preserving baked lighting in FP16"
            )
        } else {
            format!(
                "{name}: r_floatLightmap: using FP16 lightmap storage (no HDR companion set detected)"
            )
        });
    }
    let lightmap_pages = if embedded_lightmap_pages > 0 {
        if world_deluxe_mapping {
            embedded_lightmap_pages / 2
        } else if atlas.is_some() {
            1
        } else {
            embedded_lightmap_pages
        }
    } else {
        external_lightmaps.len()
    };
    let static_light_grid = prepare_static_light_grid(&bsp, &mut assets, name, &mut warnings)?;
    load_timings.lightmap_ms = lightmap_stage.elapsed().as_secs_f64() * 1000.0;
    phase_lap(&mut load_timings, 2, &mut phase_clock);

    // Spawns, FX runners, brush entities and the entity graph read only the BSP,
    // so they run here while the workers are still decoding textures.
    let static_models = Arc::new(bsp.static_models());
    let mut spawns: Vec<_> = bsp
        .deathmatch_spawns()
        .into_iter()
        .map(|spawn| SpawnPoint {
            position: {
                let mut p = render_position(spawn.origin);
                p[1] += 9.0;
                p
            },
            yaw: spawn.yaw.to_radians(),
            initial: spawn.initial,
            no_humans: spawn.no_humans,
        })
        .collect();
    if spawns.is_empty() {
        let model = &bsp.models[0];
        let center = std::array::from_fn(|i| (model.mins[i] + model.maxs[i]) * 0.5);
        let mut position = render_position(center);
        position[1] += 9.0;
        spawns.push(SpawnPoint { position, yaw: 0.0, ..SpawnPoint::default() });
    }
    let fx_runners = bsp_fx_runners(&bsp, &mut warnings);
    let brush_entities = bsp_brush_entities(&bsp);
    let entity_graph = Some(Arc::new(crate::entity_graph::EntityGraph::build(&bsp)));
    phase_lap(&mut load_timings, 3, &mut phase_clock);
    if let Some(preload) = texture_preload {
        // Everything above overlapped the workers; this is only the remainder.
        textures.preload_finish(preload)?;
    }
    phase_lap(&mut load_timings, 4, &mut phase_clock);
    let material_stage = Instant::now();
    if let Some(jobs) = jobs {
        jobs.mark_started(Task::MapMaterials);
    }
    let sun = authored_sun(
        used_shader_names.iter().map(String::as_str),
        &library,
        &mut warnings,
    );
    let describe_started = Instant::now();
    let stats_before = (
        textures.load_stats.read_ms,
        textures.load_stats.decode_ms,
        textures.load_stats.mip_ms,
        textures.load_stats.decoded_images,
        textures.images.len(),
    );
    let surface_materials: Vec<_> = bsp
        .shaders
        .iter()
        .enumerate()
        .map(|(index, shader)| {
            if !used.contains(&index) {
                return SurfaceMaterial {
                    hidden: true,
                    ..Default::default()
                };
            }
            let shader_name = String::from_utf8_lossy(&shader.name).to_ascii_lowercase();
            materials::describe(
                &shader_name,
                shader.surface_flags,
                library.get(&shader_name),
                library_debug.origins.get(&shader_name),
                &mut assets,
                &mut textures,
                options.gen_normal_maps,
                options.omit_environment_stages,
            )
        })
        .collect();
    let describe_ms = describe_started.elapsed().as_secs_f64() * 1000.0;
    let tints_started = Instant::now();

    let grass_ground_tints: Vec<[u8; 4]> = surface_materials
        .iter()
        .map(|material| {
            material
                .grass
                .map(|_| grass_ground_albedo_tint(material, &textures))
                .unwrap_or([0, 0, 0, 0])
        })
        .collect();
    let tints_ms = tints_started.elapsed().as_secs_f64() * 1000.0;

    let lights_started = Instant::now();
    let (surface_lights, surface_light_candidates) = bsp_surface_lights(&mesh, &surface_materials);
    let surface_lights_ms = lights_started.elapsed().as_secs_f64() * 1000.0;

    let debug_started = Instant::now();
    let material_debug = material_debug_info(
        &library_debug,
        used.iter().filter_map(|&index| {
            let shader = bsp.shaders.get(index)?;
            let name = String::from_utf8_lossy(&shader.name).to_ascii_lowercase();
            Some((name, surface_materials.get(index)?.clone()))
        }),
        &library,
        &textures,
    );
    load_timings.material_ms = material_stage.elapsed().as_secs_f64() * 1000.0;
    println!(
        "[MAP MATERIALS] total {:.1} ms | describe {:.1} (of which texture read/probe {:.1}, decode {:.1}, mip {:.1}; {} decoded, {} new image slot(s); light-image averaging {:.1}) | grass tints {:.1} | surface lights {:.1} | material debug {:.1}",
        load_timings.material_ms,
        describe_ms,
        textures.load_stats.read_ms - stats_before.0,
        textures.load_stats.decode_ms - stats_before.1,
        textures.load_stats.mip_ms - stats_before.2,
        textures.load_stats.decoded_images - stats_before.3,
        textures.images.len() - stats_before.4,
        materials::LIGHT_IMAGE_AVERAGE_NS.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 1.0e6,
        tints_ms,
        surface_lights_ms,
        debug_started.elapsed().as_secs_f64() * 1000.0,
    );
    if let Some(jobs) = jobs {
        jobs.mark_done(Task::MapMaterials);
    }
    phase_lap(&mut load_timings, 5, &mut phase_clock);

    let surface_sprite_effects =
        collect_surface_sprite_effect_emitters(&bsp, &mesh, &surface_materials);
    if !surface_sprite_effects.is_empty() {
        let triangle_count: usize = surface_sprite_effects
            .iter()
            .map(|emitter| emitter.triangles.len())
            .sum();
        println!(
            "{name}: surfaceSprites effect: {} emitter stage(s), {triangle_count} source triangle(s)",
            surface_sprite_effects.len()
        );
    }

    // Grass blade generation is independent of ordinary BSP vertex packing once
    // emitter triangles/material metadata have been captured. Start it on the map
    // worker pool before entering the main geometry walk so both jobs overlap.
    let grass_stage = Instant::now();
    let (grass_emitters, grass_signatures) = if options.grass {
        collect_grass_emitters(&bsp, &mesh, &surface_materials, &grass_ground_tints)
    } else {
        (Vec::new(), Vec::new())
    };
    let mut grass_jobs = Vec::new();
    let mut grass_sync = None;
    // Blade generation is a pure function of the emitter triangles and the
    // lightmap pixels they sample, so an unchanged fingerprint reuses the patches.
    let mut grass_key = None;
    let mut grass_memo_hit = None;
    let grass_lightmaps = (!grass_emitters.is_empty()).then(|| {
        Arc::new(grass_lightmap_sources(&bsp, &external_lightmaps, &external_lightmap_lookup))
    });
    if let Some(lightmaps) = &grass_lightmaps {
        let key = grass_fingerprint(&grass_emitters, &grass_signatures, lightmaps);
        grass_key = Some(key);
        grass_memo_hit = seed
            .filter(|seed| seed.stage_keys.grass == Some(key))
            .map(|seed| {
                let blades = seed.grass_patches.iter().map(|patch| patch.instances.len()).sum();
                (seed.grass_patches.clone(), blades)
            });
        if grass_memo_hit.is_some() {
            note_reused_stage(jobs, &mut reused, "grass", Task::MapGrass);
        }
    }
    if let Some(lightmaps) = grass_lightmaps.filter(|_| grass_memo_hit.is_none()) {
        let clump_data = crate::grass::godot_clump_noise();
        if let Some(jobs) = jobs {
            let worker_count = jobs.worker_count().min(grass_emitters.len()).max(1);
            let chunk_size = grass_emitters.len().div_ceil(worker_count);
            let mut iter = grass_emitters.into_iter();
            loop {
                let chunk = iter.by_ref().take(chunk_size).collect::<Vec<_>>();
                if chunk.is_empty() {
                    break;
                }
                let lightmaps = Arc::clone(&lightmaps);
                let clump_data = Arc::clone(&clump_data);
                grass_jobs.push(jobs.submit(Task::MapGrass, move || {
                    let started = Instant::now();
                    let result = generate_grass_chunk(chunk, lightmaps, clump_data);
                    (result, started.elapsed().as_secs_f64() * 1000.0)
                })?);
            }
        } else {
            let started = Instant::now();
            let result = generate_grass_chunk(grass_emitters, lightmaps, clump_data);
            load_timings.grass_cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
            grass_sync = Some(result);
        }
    }

    phase_lap(&mut load_timings, 6, &mut phase_clock);
    let geometry_stage = Instant::now();
    if let Some(jobs) = jobs {
        jobs.mark_started(Task::MapGeometry);
    }
    // Atomic so `batch_geometry` is `Fn` and the world walk can run it on rayon.
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    let alpha_discard_culled_batches = AtomicUsize::new(0);
    let alpha_discard_culled_triangles = AtomicUsize::new(0);
    let alpha_discard_culled_vertices = AtomicUsize::new(0);
    // One BSP mesh batch -> its material/lightmap group and render vertices.
    // Shared by the world and inline models so a mover's surfaces resolve
    // lightmaps, vertex lighting and fog exactly like the world's.
    let batch_geometry = |mesh: &jka_assets::bsp::Mesh,
                              batch_index: usize,
                              batch: &jka_assets::bsp::DrawBatch|
     -> Option<(GroupKey, WorldGeometryChunk)> {
        let material = &surface_materials[batch.shader];
        if material.hidden {
            return None;
        }
        let surface = &bsp.surfaces[batch.surface];
        let lightmap_slot =
            (0..4).find(|&i| batch.lightmaps[i] >= 0 && batch.lightmap_styles[i] < 254);
        let vertex_lit_slot = (0..4)
            .find(|&i| batch.lightmaps[i] == LIGHTMAP_BY_VERTEX && surface.vertex_styles[i] < 254);
        let vertex_lit = vertex_lit_slot.is_some();
        if material_render_is_guaranteed_discarded(material, vertex_lit) {
            alpha_discard_culled_batches.fetch_add(1, AtomicOrdering::Relaxed);
            alpha_discard_culled_triangles
                .fetch_add(batch.indices.len() / 3, AtomicOrdering::Relaxed);
            // World geometry is expanded to one GPU vertex per source index,
            // so this is the exact number of render vertices we avoid packing.
            alpha_discard_culled_vertices.fetch_add(batch.indices.len(), AtomicOrdering::Relaxed);
            return None;
        }
        let class = class_for(material);
        let source_lightmap = lightmap_slot
            .filter(|_| !material.sky)
            .map(|i| batch.lightmaps[i] as usize);
        let lightmap = source_lightmap.map(|page| {
            if atlas.is_some() {
                0
            } else if !external_lightmap_lookup.is_empty() {
                external_lightmap_lookup[&page]
            } else if world_deluxe_mapping {
                page >> 1
            } else {
                page
            }
        });
        let color_slot = vertex_lit_slot
            .or(lightmap_slot)
            .or_else(|| (0..4).find(|&i| surface.vertex_styles[i] < 254))
            .unwrap_or(0) as u8;
        let mut geometry = Vec::with_capacity(batch.indices.len());
        for triangle in mesh.indices[batch.indices.clone()]
            .as_chunks::<3>()
            .0
            .iter()
        {
            let source_indices = [triangle[0] as usize, triangle[2] as usize, triangle[1] as usize];

            for vertex_index in source_indices {
                let vertex = mesh.vertices[vertex_index];
                let raw_lightmap_uv = lightmap_slot
                    .map(|i| vertex.lightmap_uv[i])
                    .unwrap_or([-1.0, -1.0]);
                let lightmap_uv = match (&atlas, source_lightmap) {
                    (Some(atlas), Some(page)) => atlas.uv(page, raw_lightmap_uv),
                    (None, Some(_)) => raw_lightmap_uv,
                    _ => [-1.0, -1.0],
                };
                let color = vertex.color[color_slot as usize].map(|v| f32::from(v) / 255.0);
                geometry.push(GpuVertex {
                    position: render_position(vertex.position),
                    uv: vertex.texcoord,
                    lightmap_uv,
                    normal: render_position(vertex.normal),
                    color,
                    alpha_cutoff: 1.0,
                });
            }
        }

        let has_environment_stage = material
            .stages
            .iter()
            .any(|stage| matches!(stage.tc_gen, TcGen::Environment));
        let planar_group = if options.planar_environment
            && has_environment_stage
            && class != DrawClass::Transparent
            && !material.planar_reflection
        {
            planar_group_key(&geometry)
        } else {
            None
        };
        let preserve_unique_plane = options.planar_reflections
            && (material.planar_reflection
                || (options.planar_environment && has_environment_stage && planar_group.is_none()));
        let key = GroupKey {
            class,
            shader: batch.shader,
            lightmap,
            vertex_lit,
            color_slot,
            fog_num: surface.fog_num,
            // Surfaces of one shader/lightmap/fog state share a group, like
            // OpenJK's per-shader batches, so transparent surfaces collapse into
            // one draw per stage instead of one per BSP surface (groups are
            // still ordered by shader). Planar reflection surfaces only need
            // isolation while the reflection topology is enabled; tcGen
            // environment geometry that is itself planar is grouped by plane
            // instead of by individual BSP surface.
            transparent_order: if preserve_unique_plane {
                batch_index + 1
            } else {
                0
            },
            planar_group,
        };
        let surface_ids = vec![u32::try_from(batch.surface).unwrap_or(u32::MAX); geometry.len() / 3];
        Some((key, WorldGeometryChunk { vertices: geometry, surface_ids }))
    };

    // OpenJK performs coarse dlight rejection on BSP surfaces before projected
    // lighting. Retain equivalent immutable surface data so the WGPU path can
    // build a small per-surface bitmask each frame instead of testing every
    // runtime light in every fragment.
    let dlight_stage = Instant::now();
    let legacy_dlight_surfaces = bsp
        .surfaces
        .iter()
        .map(|surface| {
            let material = &surface_materials[surface.shader];
            let shader_flags = bsp.shaders.get(surface.shader).map_or(0, |shader| shader.surface_flags);
            let eligible = !material.hidden
                && !material.sky
                && shader_flags & jka_assets::bsp::SURF_NODLIGHT == 0
                && !matches!(surface.kind, SurfaceKind::Flare);
            if !eligible {
                return LegacyDlightSurface::default();
            }

            let mut bounds_min = [f32::INFINITY; 3];
            let mut bounds_max = [f32::NEG_INFINITY; 3];
            for vertex in &bsp.vertices[surface.vertices.clone()] {
                let p = render_position(vertex.position);
                for axis in 0..3 {
                    bounds_min[axis] = bounds_min[axis].min(p[axis]);
                    bounds_max[axis] = bounds_max[axis].max(p[axis]);
                }
            }
            if !bounds_min[0].is_finite() {
                bounds_min = [0.0; 3];
                bounds_max = [0.0; 3];
            }

            let plane = if matches!(surface.kind, SurfaceKind::Planar) {
                surface.vertices.clone().find_map(|index| {
                    let vertex = bsp.vertices.get(index)?;
                    let normal = Vec3::from_array(render_position(vertex.normal)).normalize_or_zero();
                    if normal.length_squared() <= 1.0e-10 { return None; }
                    let point = Vec3::from_array(render_position(vertex.position));
                    Some([normal.x, normal.y, normal.z, normal.dot(point)])
                }).unwrap_or([0.0; 4])
            } else {
                [0.0; 4]
            };

            LegacyDlightSurface {
                cull_kind: match surface.kind {
                    SurfaceKind::Planar if plane[0] != 0.0 || plane[1] != 0.0 || plane[2] != 0.0 => 1,
                    SurfaceKind::Patch | SurfaceKind::Triangles | SurfaceKind::Planar => 2,
                    SurfaceKind::Flare => 0,
                },
                plane, bounds_min, bounds_max,
            }
        })
        .collect::<Vec<_>>();
    let dlight_surfaces_ms = dlight_stage.elapsed().as_secs_f64() * 1000.0;

    let mut groups: BTreeMap<GroupKey, Geometry> = BTreeMap::new();
    let mut triangles = 0usize;
    let walk_stage = Instant::now();
    let mut signature_time = std::time::Duration::ZERO;
    // Each BSP batch expands to its render vertices and PVS signature
    // independently, so that runs on rayon; the results come back in batch order
    // and are merged sequentially, which keeps every group's contents identical.
    let walked = {
        use rayon::prelude::*;
        mesh.batches
            .par_iter()
            .enumerate()
            .map(|(batch_index, batch)| {
                let (key, chunk) = batch_geometry(&mesh, batch_index, batch)?;
                let signature_started = Instant::now();
                let signature = bsp
                    .visibility
                    .as_ref()
                    .map(|vis| pvs_signature(vis, &vis.surface_clusters[batch.surface]))
                    .unwrap_or_default();
                let signature_elapsed = signature_started.elapsed();
                let area_signature = bsp
                    .visibility
                    .as_ref()
                    .and_then(|vis| vis.surface_area_masks.get(batch.surface).copied())
                    .unwrap_or([0_u64; 4]);
                Some((key, chunk, signature, area_signature, signature_elapsed))
            })
            .collect::<Vec<_>>()
    };
    for (key, batch_geometry, signature, area_signature, signature_elapsed) in
        walked.into_iter().flatten()
    {
        signature_time += signature_elapsed;
        triangles += batch_geometry.vertices.len() / 3;
        let chunk = groups
            .entry(key)
            .or_default()
            .by_pvs_signature
            .entry((signature, area_signature))
            .or_default();
        chunk.vertices.extend_from_slice(&batch_geometry.vertices);
        chunk.surface_ids.extend_from_slice(&batch_geometry.surface_ids);
    }
    let walk_ms = walk_stage.elapsed().as_secs_f64() * 1000.0;

    let pack_stage = Instant::now();
    // Ordering is a pure function of one group's pieces, so every group is
    // ordered at once; the groups stay in key order for the sequential pack below.
    let order_started = Instant::now();
    let ordered_groups = {
        use rayon::prelude::*;
        groups
            .into_iter()
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(key, geometry)| (key, order_pvs_pieces(geometry.by_pvs_signature)))
            .collect::<Vec<_>>()
    };
    let order_time = order_started.elapsed();
    let mut vertices = Vec::new();
    let mut legacy_dlight_triangle_surfaces = Vec::new();
    let mut batches = Vec::new();
    let mut pvs_batches = Vec::new();
    for (key, ordered_pieces) in ordered_groups {
        let material = &surface_materials[key.shader];
        let material_debug_index = bsp.shaders.get(key.shader).and_then(|shader| {
            let shader_name = String::from_utf8_lossy(&shader.name);
            material_debug
                .entries
                .iter()
                .position(|entry| entry.name.eq_ignore_ascii_case(shader_name.as_ref()))
        });
        let coarse_start =
            u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;
        let mut coarse_signature = Vec::<u64>::new();
        let mut coarse_area_signature = [0_u64; 4];

        for ((signature, area_signature), piece) in ordered_pieces {
            let piece_start =
                u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;

            let (fog, fog_is_global) = resolved_bsp_surface_fog(
                &bsp,
                &library,
                material,
                key.fog_num,
                global_fog_num,
                global_fog,
            );
            vertices.extend_from_slice(&piece.vertices);
            legacy_dlight_triangle_surfaces.extend_from_slice(&piece.surface_ids);
            let piece_end =
                u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;

            if coarse_signature.len() < signature.len() {
                coarse_signature.resize(signature.len(), 0);
            }
            for (word, value) in signature.iter().enumerate() {
                coarse_signature[word] |= value;
            }
            for word in 0..coarse_area_signature.len() {
                coarse_area_signature[word] |= area_signature[word];
            }

            append_material_batches(
                &mut pvs_batches,
                material,
                key.shader,
                material_debug_index,
                key.vertex_lit,
                piece_start..piece_end,
                key.lightmap,
                &signature,
                area_signature,
                fog,
                fog_is_global,
            );
        }

        let coarse_end =
            u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;
        let (fog, fog_is_global) = resolved_bsp_surface_fog(
            &bsp,
            &library,
            material,
            key.fog_num,
            global_fog_num,
            global_fog,
        );
        append_material_batches(
            &mut batches,
            material,
            key.shader,
            material_debug_index,
            key.vertex_lit,
            coarse_start..coarse_end,
            key.lightmap,
            &coarse_signature,
            coarse_area_signature,
            fog,
            fog_is_global,
        );
    }
    load_timings.geometry_detail_ms = [
        dlight_surfaces_ms,
        walk_ms,
        signature_time.as_secs_f64() * 1000.0,
        pack_stage.elapsed().as_secs_f64() * 1000.0,
        order_time.as_secs_f64() * 1000.0,
    ];

    let mut inline_vertices = Vec::new();
    let mut inline_batches = Vec::new();
    let mut inline_models = Vec::new();
    {
        let mut start = 0usize;
        while start < inline_mesh.batches.len() {
            let model = inline_batch_models[start];
            let end = inline_batch_models[start..]
                .iter()
                .position(|&other| other != model)
                .map_or(inline_mesh.batches.len(), |offset| start + offset);
            let mut model_groups: BTreeMap<GroupKey, Vec<GpuVertex>> = BTreeMap::new();
            for batch_index in start..end {
                let batch = &inline_mesh.batches[batch_index];
                if let Some((key, batch_geometry)) = batch_geometry(&inline_mesh, batch_index, batch) {
                    model_groups.entry(key).or_default().extend_from_slice(&batch_geometry.vertices);
                }
            }
            let vertex_start =
                u32::try_from(inline_vertices.len()).map_err(|_| "inline vertex count exceeds u32")?;
            let batch_start = inline_batches.len();
            for (key, group_vertices) in model_groups {
                let material = &surface_materials[key.shader];
                let material_debug_index = bsp.shaders.get(key.shader).and_then(|shader| {
                    let shader_name = String::from_utf8_lossy(&shader.name);
                    material_debug
                        .entries
                        .iter()
                        .position(|entry| entry.name.eq_ignore_ascii_case(shader_name.as_ref()))
                });
                let range_start =
                    u32::try_from(inline_vertices.len()).map_err(|_| "inline vertex count exceeds u32")?;
                inline_vertices.extend_from_slice(&group_vertices);
                let range_end =
                    u32::try_from(inline_vertices.len()).map_err(|_| "inline vertex count exceeds u32")?;
                let (fog, fog_is_global) = resolved_bsp_surface_fog(
                    &bsp,
                    &library,
                    material,
                    key.fog_num,
                    global_fog_num,
                    global_fog,
                );
                append_material_batches(
                    &mut inline_batches,
                    material,
                    key.shader,
                    material_debug_index,
                    key.vertex_lit,
                    range_start..range_end,
                    key.lightmap,
                    &[],
                    [0_u64; 4],
                    fog,
                    fog_is_global,
                );
            }
            // A moving brush cannot be promoted to the ocean clipmap, and
            // mirrors are bound to static authored planes.
            for batch in &mut inline_batches[batch_start..] {
                batch.water_primary = false;
                batch.planar_reflection = false;
                batch.planar_environment_candidate = false;
            }
            let vertex_end =
                u32::try_from(inline_vertices.len()).map_err(|_| "inline vertex count exceeds u32")?;
            if batch_start != inline_batches.len() {
                inline_models.push(InlineModelGeometry {
                    model,
                    vertices: vertex_start..vertex_end,
                    batches: batch_start..inline_batches.len(),
                });
            }
            start = end;
        }
    }
    if !inline_models.is_empty() {
        println!(
            "{name}: {} drawable inline BSP model(s), {} vertices, {} material batch(es)",
            inline_models.len(),
            inline_vertices.len(),
            inline_batches.len()
        );
    }
    let alpha_discard_culled_batches = alpha_discard_culled_batches.into_inner();
    if alpha_discard_culled_batches > 0 {
        println!(
            "{name}: guaranteed alpha-discard render cull: {alpha_discard_culled_batches} BSP batch(es), {} triangle(s), {} GPU render vertices omitted; source BSP geometry retained for semantic extraction",
            alpha_discard_culled_triangles.into_inner(),
            alpha_discard_culled_vertices.into_inner(),
        );
    }

    phase_lap(&mut load_timings, 7, &mut phase_clock);
    let mut lights = bsp_dynamic_lights(&bsp);
    let entity_light_count = lights.len();
    let surface_light_count = surface_lights.len();
    lights.extend(surface_lights);
    if surface_light_candidates > 0 {
        println!(
            "{name}: emissive/area lights: {surface_light_count} real-time sample(s) selected from {surface_light_candidates} candidate(s); {entity_light_count} entity light(s) retained first"
        );
    }
    // GI reads immutable world geometry/lights only. Clone the modest render mesh
    // snapshot and let a map worker voxelize/propagate while the loader continues
    // reflection/planar/lightmap/spawn finalization on the map-loader thread.
    let gi_key = options.voxel_probe_gi.then(|| {
        let mut fingerprint = StageFingerprint::new("voxel-probe-gi");
        fingerprint
            .pod(&vertices)
            .draw_batches(&mut batches)
            .debug(&lights)
            .debug(&sun);
        fingerprint.finish()
    });
    let gi_memo_hit = gi_key.and_then(|key| {
        seed.filter(|seed| seed.stage_keys.gi == Some(key))
            .map(|seed| seed.voxel_probe_gi.clone())
    });
    if gi_memo_hit.is_some() {
        note_reused_stage(jobs, &mut reused, "voxel GI", Task::MapGi);
    }
    // Without a worker pool GI runs right here: the geometry vectors are handed to
    // the PVS plan job further down, so nothing after this point may read them.
    let mut gi_sync = None;
    let gi_job = if options.voxel_probe_gi && gi_memo_hit.is_none() {
        if let Some(jobs) = jobs {
            let gi_vertices = vertices.clone();
            let gi_batches = batches.clone();
            let gi_lights = lights.clone();
            Some(jobs.submit(Task::MapGi, move || {
                let started = Instant::now();
                let grid = build_voxel_probe_gi(&gi_vertices, &gi_batches, &gi_lights, sun);
                (grid, started.elapsed().as_secs_f64() * 1000.0)
            })?)
        } else {
            let gi_stage = Instant::now();
            let grid = build_voxel_probe_gi(&vertices, &batches, &lights, sun);
            gi_sync = Some((grid, gi_stage.elapsed().as_secs_f64() * 1000.0));
            None
        }
    } else {
        None
    };

    let reflection_probes = load_reflection_probes(&bsp, &mut assets, name, &mut warnings)?;
    assign_reflection_probes(&mut batches, &vertices, &reflection_probes);
    assign_reflection_probes(&mut pvs_batches, &vertices, &reflection_probes);
    assign_reflection_probes(&mut inline_batches, &inline_vertices, &reflection_probes);
    let (portal_anchors, camera_portal_count) = bsp_portal_surface_anchors(&bsp);
    let authored_portal_batch_count;
    if options.planar_reflections {
        assign_planar_reflection_planes(&mut batches, &vertices, options.planar_environment);
        assign_planar_reflection_planes(&mut pvs_batches, &vertices, options.planar_environment);
        authored_portal_batch_count = batches
            .iter()
            .filter(|batch| batch.planar_reflection)
            .count();
        retain_authored_planar_mirrors(&mut batches, &portal_anchors);
        retain_authored_planar_mirrors(&mut pvs_batches, &portal_anchors);
    } else {
        authored_portal_batch_count = 0;
        // Reflection quality is restart-latched specifically so the map can be
        // prepared without per-plane metadata when planar reflections are off.
        // Clear the material-authored eligibility bits as well; otherwise later
        // draw-compaction sees them and still refuses to merge compatible draws.
        for batch in batches
            .iter_mut()
            .chain(pvs_batches.iter_mut())
        {
            batch.planar_reflection = false;
            batch.planar_environment_candidate = false;
            batch.planar_plane = [0.0; 4];
        }
    }
    cache_reflection_decisions(&mut batches);
    cache_reflection_decisions(&mut pvs_batches);
    log_reflection_cache(name, &batches);
    let planar_mirror_batch_count = batches
        .iter()
        .filter(|batch| batch.planar_reflection)
        .count();
    if authored_portal_batch_count != 0 && planar_mirror_batch_count == 0 {
        warnings.push(
            "portal shader surfaces found, but none has an untargeted misc_portal_surface within 64 units; no planar mirrors enabled"
                .into(),
        );
    }
    if options.planar_reflections && camera_portal_count != 0 {
        warnings.push(format!(
            "{camera_portal_count} targeted misc_portal_surface camera portal(s) detected; planar mirror pass leaves camera portals on their authored material"
        ));
    }
    load_timings.geometry_ms = geometry_stage.elapsed().as_secs_f64() * 1000.0;
    if let Some(jobs) = jobs {
        jobs.mark_done(Task::MapGeometry);
    }
    phase_lap(&mut load_timings, 8, &mut phase_clock);

    // The authored ocean planes were the last thing that edited the batch lists
    // before the plan, and they read only the BSP, so they run here and the plan
    // can start now instead of after lightmaps, grass, GI and audio.
    let authored_oceans = crate::ocean::authoring::AuthoredOcean::from_bsp(&bsp);
    append_authored_ocean_planes(
        &authored_oceans,
        &mut vertices,
        &mut batches,
        &mut pvs_batches,
    );

    // AUTO 4 is pure CPU map preprocessing. Keep its potentially expensive
    // per-cluster PVS/group search off the renderer thread and expose it as its
    // own loading-screen stage. The plan reads only batch draw state, PVS/area
    // signatures and vertex positions (never decoded textures, GI, grass or
    // lightmap pixels), so the worker is started as soon as those are final and
    // joined after the unrelated stages below. It owns the vectors in the
    // meantime and returns them unchanged alongside the immutable draw plan.
    let visibility = bsp.visibility.clone();
    let plan_wanted = visibility.is_some() && !pvs_batches.is_empty();
    // The plan reads the draw-state of both batch lists and the vertex positions
    // (for bounds); visibility is fixed by the BSP bytes the memo is keyed on.
    let portal_key = plan_wanted.then(|| {
        let mut fingerprint = StageFingerprint::new("auto4-portal-plan");
        fingerprint
            .draw_batches(&mut batches)
            .draw_batches(&mut pvs_batches)
            .positions(&vertices);
        fingerprint.finish()
    });
    let portal_memo_hit = portal_key.and_then(|key| {
        seed.filter(|seed| seed.stage_keys.portal == Some(key))
            .map(|seed| seed.portal_draw_plan.clone())
    });
    if portal_memo_hit.is_some() {
        note_reused_stage(jobs, &mut reused, "PVS plans", Task::MapPortalPlans);
    }
    let mut portal_inputs = Some((vertices, batches, pvs_batches, visibility));
    let mut portal_job = None;
    if portal_memo_hit.is_none() && plan_wanted {
        if let Some(jobs) = jobs {
            let (vertices, batches, pvs_batches, visibility) =
                portal_inputs.take().expect("plan inputs are taken once");
            let total = u32::try_from(visibility.as_ref().map_or(0, |vis| vis.clusters))
                .unwrap_or(u32::MAX)
                .max(1);
            portal_job = Some(jobs.submit_progress(Task::MapPortalPlans, total, move |progress| {
                let started = Instant::now();
                let plan = build_prepared_portal_draw_plan(
                    &batches,
                    &pvs_batches,
                    visibility.as_ref().expect("visibility checked before AUTO 4 job"),
                    &vertices,
                    |completed| progress.set_completed(completed),
                );
                let plan_ms = started.elapsed().as_secs_f64() * 1000.0;
                (vertices, batches, pvs_batches, visibility, plan, plan_ms)
            })?);
        }
    }
    phase_lap(&mut load_timings, 9, &mut phase_clock);

    let (lightmaps, deluxemaps) = if !external_lightmaps.is_empty() {
        (external_lightmaps, external_deluxemaps)
    } else if let Some(atlas) = atlas {
        (
            vec![TextureData {
                label: "JKA lightmap atlas".into(),
                source: None,
                width: atlas.width,
                height: atlas.height,
                rgba: atlas.rgba,
                rgba16f: None,
                mip_level_count: 1,
                clamp: true,
                srgb: true,
            }],
            vec![neutral_deluxemap("JKA atlas neutral deluxe")],
        )
    } else {
        let pages = bsp.lightmaps.as_chunks::<{ 128 * 128 * 3 }>().0;
        let mut lightmaps = Vec::with_capacity(lightmap_pages);
        let mut deluxemaps = Vec::with_capacity(lightmap_pages);
        if world_deluxe_mapping {
            for effective in 0..lightmap_pages {
                let source_page = effective * 2;
                let mut lightmap = if hdr_lightmaps_available {
                    try_load_hdr_lightmap(&mut assets, name, source_page)?
                        .unwrap_or_else(|| embedded_lightmap_texture(source_page, &pages[source_page]))
                } else {
                    embedded_lightmap_texture(source_page, &pages[source_page])
                };
                if options.float_lightmap {
                    materials::promote_lightmap_to_float(&mut lightmap);
                }
                lightmaps.push(lightmap);
                deluxemaps.push(embedded_deluxemap_texture(
                    source_page + 1,
                    &pages[source_page + 1],
                ));
            }
        } else {
            for (source_page, page) in pages.iter().enumerate() {
                let mut lightmap = if hdr_lightmaps_available {
                    try_load_hdr_lightmap(&mut assets, name, source_page)?
                        .unwrap_or_else(|| embedded_lightmap_texture(source_page, page))
                } else {
                    embedded_lightmap_texture(source_page, page)
                };
                if options.float_lightmap {
                    materials::promote_lightmap_to_float(&mut lightmap);
                }
                lightmaps.push(lightmap);
                deluxemaps.push(neutral_deluxemap(format!(
                    "JKA neutral deluxe {source_page}"
                )));
            }
        }
        (lightmaps, deluxemaps)
    };

    phase_lap(&mut load_timings, 10, &mut phase_clock);
    // Grass generation jobs were launched before the main geometry walk. Resolve and
    // merge them before joining GI so patch-finalization jobs can use the remaining
    // map workers while the queued/running GI job occupies at most one worker.
    let mut grass_groups = BTreeMap::<GrassPatchKey, Vec<GrassInstance>>::new();
    let mut grass_blades = 0usize;
    if let Some((groups, blades)) = grass_sync {
        grass_blades += blades;
        for (worker_key, mut instances) in groups {
            let key = GrassPatchKey {
                cell_x: worker_key.cell_x,
                cell_z: worker_key.cell_z,
                pvs_signature: grass_signatures
                    .get(worker_key.signature_id as usize)
                    .cloned()
                    .unwrap_or_default(),
            };
            match grass_groups.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(instances);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().append(&mut instances);
                }
            }
        }
    }
    for handle in grass_jobs {
        let ((groups, blades), cpu_ms) = handle.join()?;
        load_timings.grass_cpu_ms += cpu_ms;
        grass_blades += blades;
        for (worker_key, mut instances) in groups {
            let key = GrassPatchKey {
                cell_x: worker_key.cell_x,
                cell_z: worker_key.cell_z,
                pvs_signature: grass_signatures
                    .get(worker_key.signature_id as usize)
                    .cloned()
                    .unwrap_or_default(),
            };
            match grass_groups.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(instances);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().append(&mut instances);
                }
            }
        }
    }
    let grass_patches = if let Some((patches, blades)) = grass_memo_hit {
        grass_blades = blades;
        patches
    } else {
        let (patches, grass_finalize_cpu_ms) = finish_grass_patches_with_jobs(grass_groups, jobs)?;
        load_timings.grass_cpu_ms += grass_finalize_cpu_ms;
        patches
    };
    load_timings.grass_ms = grass_stage.elapsed().as_secs_f64() * 1000.0;
    if grass_blades != 0 {
        let grass_cpu_bytes = grass_blades * std::mem::size_of::<GrassInstance>();
        println!(
            "{name}: procedural grass: {grass_blades} blade(s) in {} spatial/PVS patch(es), {:.1} MiB compact CPU instances ({} bytes/blade); worker CPU {:.1} ms, wall-to-ready {:.1} ms",
            grass_patches.len(),
            grass_cpu_bytes as f64 / (1024.0 * 1024.0),
            std::mem::size_of::<GrassInstance>(),
            load_timings.grass_cpu_ms,
            load_timings.grass_ms,
        );
    }

    phase_lap(&mut load_timings, 11, &mut phase_clock);
    // GI was queued after geometry became immutable. Joining it only after grass
    // finalization allows the worker pool to overlap both CPU-heavy finishing stages.
    let voxel_probe_gi = if let Some(hit) = gi_memo_hit {
        hit
    } else if let Some(handle) = gi_job {
        let (grid, cpu_ms) = handle.join()?;
        load_timings.gi_ms = cpu_ms;
        grid
    } else if let Some((grid, gi_ms)) = gi_sync {
        load_timings.gi_ms = gi_ms;
        grid
    } else {
        None
    };
    if let Some(grid) = &voxel_probe_gi {
        println!(
            "{name}: voxel/probe GI: {}x{}x{} probes, {:.1}-unit cells, {} occupied surface voxel(s)",
            grid.bounds[0], grid.bounds[1], grid.bounds[2], grid.cell_size, grid.occupied_voxels
        );
    }

    let steam_audio_result = if let Some(handle) = steam_audio_job {
        Some(handle.join()?)
    } else {
        steam_audio_sync
    };
    let steam_audio_acoustic_mesh = match steam_audio_result {
        Some((Ok(mesh), cpu_ms)) => {
            load_timings.steam_audio_ms = cpu_ms;
            println!(
                "{name}: Steam Audio acoustic scene: {} vertices, {} triangles, {:.1} ms CPU",
                mesh.vertices.len(),
                mesh.triangles.len(),
                cpu_ms,
            );
            Some(mesh)
        }
        Some((Err(error), cpu_ms)) => {
            load_timings.steam_audio_ms = cpu_ms;
            warnings.push(format!(
                "{name}: Steam Audio acoustic scene unavailable: {error}"
            ));
            None
        }
        None => None,
    };

    let mut steam_audio_bake = None;
    let mut steam_audio_bake_request = None;
    if let (Some(acoustic_mesh), Some(cache)) =
        (&steam_audio_acoustic_mesh, steam_audio_bake_cache)
    {
        match crate::steam_audio::load_cached_bake(&cache) {
            Ok(Some(data)) => {
                steam_audio_bake = Some(Arc::new(data));
            }
            Ok(None) => {
                let num_threads = jobs
                    .map(|jobs| jobs.worker_count().saturating_sub(1).max(1))
                    .unwrap_or_else(|| {
                        std::thread::available_parallelism()
                            .map_or(1, |count| count.get().saturating_sub(2).clamp(1, 8))
                    });
                println!(
                    "{name}: Steam Audio bake cache MISS; background bake queued after map preparation ({num_threads} Steam Audio thread(s))"
                );
                steam_audio_bake_request = Some(crate::steam_audio::SteamAudioBakeRequest {
                    mesh: Arc::clone(acoustic_mesh),
                    cache,
                    num_threads,
                });
            }
            Err(error) => {
                // A corrupt/stale cache is rebuildable and must not block the map.
                let num_threads = jobs
                    .map(|jobs| jobs.worker_count().saturating_sub(1).max(1))
                    .unwrap_or(1);
                warnings.push(format!(
                    "{name}: Steam Audio cache invalid; rebuilding in background: {error}"
                ));
                steam_audio_bake_request = Some(crate::steam_audio::SteamAudioBakeRequest {
                    mesh: Arc::clone(acoustic_mesh),
                    cache,
                    num_threads,
                });
            }
        }
    }

    phase_lap(&mut load_timings, 12, &mut phase_clock);
    let (footprint_mark_textures, footprint_mark_blend_modes) =
        load_legacy_footprint_marks(&library, &mut assets, &mut textures);

    let texture_stats = textures.load_stats;
    let texture_hashes = textures.content_hashes();
    load_timings.texture_format_images = texture_stats.format_images;
    load_timings.texture_format_decode_ms = texture_stats.format_decode_ms;
    load_timings.texture_preload_wall_ms = texture_stats.preload_wall_ms;
    load_timings.texture_read_ms = texture_stats.read_ms;
    load_timings.texture_decode_ms = texture_stats.decode_ms;
    load_timings.texture_mip_ms = texture_stats.mip_ms;
    load_timings.texture_images = texture_stats.decoded_images;
    load_timings.generated_normal_ms = texture_stats.generated_normal_ms;
    load_timings.generated_normals = texture_stats.generated_normals;
    warnings.extend(textures.warnings);
    warnings.sort();
    warnings.dedup();

    phase_lap(&mut load_timings, 13, &mut phase_clock);
    // Join the plan started after the geometry stage. `portal_plans_ms` is the
    // plan's own compute time; `portal_wait_ms` is how long the loader stalled.
    let portal_wait_started = Instant::now();
    let (vertices, batches, pvs_batches, visibility, portal_draw_plan) =
        if let Some(plan) = portal_memo_hit {
            let (vertices, batches, pvs_batches, visibility) =
                portal_inputs.take().expect("plan inputs kept on a memo hit");
            (vertices, batches, pvs_batches, visibility, plan)
        } else if let Some(handle) = portal_job {
            let (vertices, batches, pvs_batches, visibility, plan, plan_ms) = handle.join()?;
            load_timings.portal_plans_ms = plan_ms;
            (vertices, batches, pvs_batches, visibility, plan)
        } else if plan_wanted {
            let (vertices, batches, pvs_batches, visibility) =
                portal_inputs.take().expect("plan inputs kept without a worker pool");
            let plan_stage = Instant::now();
            let plan = build_prepared_portal_draw_plan(
                &batches,
                &pvs_batches,
                visibility.as_ref().expect("visibility checked before AUTO 4 build"),
                &vertices,
                |_| {},
            );
            load_timings.portal_plans_ms = plan_stage.elapsed().as_secs_f64() * 1000.0;
            (vertices, batches, pvs_batches, visibility, plan)
        } else {
            let (vertices, batches, pvs_batches, visibility) =
                portal_inputs.take().expect("plan inputs kept when no plan is wanted");
            (vertices, batches, pvs_batches, visibility, PreparedPortalDrawPlan::default())
        };
    let portal_wait_ms = portal_wait_started.elapsed().as_secs_f64() * 1000.0;
    phase_lap(&mut load_timings, 14, &mut phase_clock);
    if !reused.is_empty() {
        println!("{name}: re-prepare reused unchanged stage(s): {}", reused.join(", "));
    }
    if !portal_draw_plan.plan_by_cluster.is_empty() {
        println!(
            "{name}: AUTO 4 map-worker plans: {} FULL piece(s), {} collapsed recipe(s) over {} physical geometr(ies) ({} already contiguous, {:.2} MiB to build lazily), {} unique plan(s) for {} cluster(s); {} recipe reuse hit(s), {} whole-plan reuse hit(s), {:.1} ms (loader waited {:.1} ms)",
            pvs_batches.len(),
            portal_draw_plan.variants.len(),
            portal_draw_plan.geometries.len(),
            portal_draw_plan.geometries.iter().filter(|geometry| geometry.contiguous).count(),
            portal_draw_plan.packed_index_count as f64 * 4.0 / (1024.0 * 1024.0),
            portal_draw_plan.plans.len(),
            portal_draw_plan.plan_by_cluster.len(),
            portal_draw_plan.reused_variant_hits,
            portal_draw_plan.reused_plan_hits,
            load_timings.portal_plans_ms,
            portal_wait_ms,
        );
    }
    let debug_volumes = match bsp.debug_volumes() {
        Ok(volumes) => Arc::new(volumes),
        Err(error) => {
            warnings.push(format!("{name}: trigger/clip debug volumes unavailable: {error}"));
            Arc::new(jka_assets::bsp::DebugVolumes::default())
        }
    };
    phase_lap(&mut load_timings, 15, &mut phase_clock);
    let archive_opens_after = jka_assets::pk3::archive_open_stats();
    load_timings.archive_opens = (archive_opens_after.0 - archive_opens_before.0) as u32;
    load_timings.archive_open_ms = archive_opens_after.1 - archive_opens_before.1;
    load_timings.prepare_wall_ms = prepare_started.elapsed().as_secs_f64() * 1000.0;

    Ok(PreparedMap {
        debug_volumes,
        authored_oceans,
        movement,
        collision,
        mark_surfaces: Some(mark_surfaces),
        static_models,
        physics_collision,
        weather_occlusion,
        vertices,
        legacy_dlight_triangle_surfaces,
        legacy_dlight_surfaces,
        batches,
        pvs_batches,
        portal_draw_plan,
        inline_vertices,
        inline_batches,
        inline_models,
        videos: textures.videos,
        textures: textures.images,
        footprint_mark_textures,
        footprint_mark_blend_modes,
        lightmaps,
        deluxemaps,
        static_bsp_ao_cache,
        steam_audio_acoustic_mesh,
        steam_audio_bake,
        steam_audio_bake_request,
        visibility,
        lights,
        source_map_lighting: SourceMapLighting::default(),
        sun,
        static_light_grid,
        voxel_probe_gi,
        reflection_probes,
        grass_patches,
        surface_sprite_effects,
        global_fog,
        warnings,
        spawns,
        fx_runners,
        brush_entities,
        entity_graph,
        distance_cull,
        sky_portal,
        triangles,
        lightmap_pages,
        source: asset_source,
        map_file_stats: None,
        bsp_stats: Some(bsp_stats),
        load_timings,
        material_debug,
        map_hash,
        stage_keys: PrepStageKeys { grass: grass_key, gi: gi_key, portal: portal_key },
        texture_hashes,
    })
}

fn parse_light_triplet(value: &str) -> Option<[f32; 3]> {
    let values: Vec<f32> = value
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if values.len() != 3 || !values.iter().all(|value| value.is_finite()) {
        return None;
    }
    values.try_into().ok()
}

fn dynamic_light_from_values(
    classname: &str,
    origin: Option<&str>,
    color: Option<&str>,
    brightness: Option<&str>,
) -> Option<DynamicLight> {
    if classname != "light" && classname != "lightJunior" {
        return None;
    }
    let origin = parse_light_triplet(origin?)?;
    let color = color
        .and_then(parse_light_triplet)
        .unwrap_or([1.0, 1.0, 1.0])
        .map(|value| value.max(0.0));
    let brightness = brightness
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(300.0);
    // Radiant's `light` key is an authored brightness rather than a strict
    // physical radius. Using it as the enhancement cutoff keeps the modern
    // dynamic contribution local and predictable without changing the baked
    // lightmap that remains the primary lighting source.
    let radius = brightness.clamp(32.0, 4096.0);
    let intensity = (brightness / 300.0).clamp(0.05, 8.0);
    Some(DynamicLight {
        position: render_position(origin),
        color,
        radius,
        intensity,
        falloff: DynamicLightFalloff::Smooth,
        // Preserve the existing compiled-BSP enhancement path exactly. Source-map
        // lightJunior filtering is handled by map_dynamic_light below.
        surface_lighting: true,
        emitter_normal: [0.0; 3],
        emitter_two_sided: false,
        angle_attenuation: true,
        angle_scale: 0.0,
        extra_distance: 0.0,
    })
}

pub(crate) fn append_authored_ocean_planes(
    oceans: &[crate::ocean::authoring::AuthoredOcean],
    vertices: &mut Vec<GpuVertex>,
    batches: &mut Vec<DrawBatch>,
    pvs_batches: &mut Vec<DrawBatch>,
) {
    let Some(template) = batches.iter().find(|b| b.water_primary).or_else(|| batches.first()).cloned() else { return; };
    // The marker brush is a volume; its top becomes an independently rendered ocean.
    // Remove fully covered stock water stages to avoid drawing two surfaces there.
    let covered = |b: &DrawBatch| b.water && oceans.iter().any(|o| {
        vertices[b.vertices.start as usize..b.vertices.end as usize].iter().all(|v| o.contains_render_point(v.position))
    });
    batches.retain(|b| b.authored_ocean.is_none() && !covered(b));
    // AUTO 4 plans address FULL pieces by index, and this also runs after the
    // plan was built (networked oceans). Empty replaced pieces in place instead
    // of removing them so every plan index stays valid.
    for b in pvs_batches.iter_mut().filter(|b| b.authored_ocean.is_some() || covered(&**b)) {
        b.vertices = b.vertices.start..b.vertices.start;
        b.water = false;
        b.water_primary = false;
        b.authored_ocean = None;
    }
    for o in oceans {
        let first = vertices.len() as u32;
        let corners = [[o.mins[0],o.height,-o.maxs[1]],[o.maxs[0],o.height,-o.maxs[1]],
            [o.maxs[0],o.height,-o.mins[1]],[o.mins[0],o.height,-o.mins[1]]];
        for i in [0,2,1,0,3,2] {
            vertices.push(GpuVertex { position:corners[i],uv:[0.0;2],lightmap_uv:[0.0;2],normal:[0.0,1.0,0.0],color:[1.0;4],alpha_cutoff:1.0 });
        }
        let mut b = template.clone();
        b.vertices = first..vertices.len() as u32;
        b.water = true; b.water_primary = true;
        b.authored_ocean = Some(o.index);
        b.bsp_shader_index = u32::MAX; b.material_debug_index = u32::MAX;
        b.texture = None; b.texture_is_lightmap = false; b.texture_is_white = false;
        b.lightmap = None; b.modulate_lightmap = false;
        b.tc_mods.clear(); b.color = [1.0;4]; b.alpha_cutoff = 0.0;
        b.pipeline = PipelineKey {class:DrawClass::Transparent,blend:BlendMode::Opaque,cull:CullMode::None,offset:false,depth_write:true,depth_equal:false};
        b.pvs_signature.clear(); b.area_signature = [0;4];
        b.planar_reflection = false; b.planar_environment_candidate = false;
        batches.push(b.clone());
        pvs_batches.push(b);
    }
}

fn bsp_entity_value<'a>(entity: &'a jka_assets::bsp::Entity, key: &[u8]) -> Option<&'a [u8]> {
    entity
        .properties
        .iter()
        .rev()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_slice())
}

fn parse_bsp_entity_triplet(value: &[u8]) -> Option<[f32; 3]> {
    let text = std::str::from_utf8(value).ok()?;
    let mut values = text.split_whitespace().map(str::parse::<f32>);
    let result = [values.next()?.ok()?, values.next()?.ok()?, values.next()?.ok()?];
    if values.next().is_some() || !result.iter().all(|value| value.is_finite()) {
        return None;
    }
    Some(result)
}

fn parse_bsp_entity_i32(value: Option<&[u8]>, default: i32) -> i32 {
    value
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(default)
}

fn parse_bsp_entity_f32(value: Option<&[u8]>, default: f32) -> f32 {
    value
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn vector_to_jka_angles(direction: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = direction;
    if x == 0.0 && y == 0.0 {
        return [if z > 0.0 { -90.0 } else { 90.0 }, 0.0, 0.0];
    }
    let yaw = y.atan2(x).to_degrees().rem_euclid(360.0);
    let forward = (x * x + y * y).sqrt();
    let pitch = z.atan2(forward).to_degrees().rem_euclid(360.0);
    [-pitch, yaw, 0.0]
}

fn bsp_entity_angles(entity: &jka_assets::bsp::Entity) -> [f32; 3] {
    if let Some(angles) = bsp_entity_value(entity, b"angles").and_then(parse_bsp_entity_triplet) {
        return angles;
    }
    if let Some(angle) = bsp_entity_value(entity, b"angle")
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
    {
        // id Tech's ANGLE_UP / ANGLE_DOWN shortcuts.
        return match angle as i32 {
            -1 => [-90.0, 0.0, 0.0],
            -2 => [90.0, 0.0, 0.0],
            _ => [0.0, angle, 0.0],
        };
    }
    [0.0; 3]
}

/// Think_SetupTrainTargets: walk from the entity named `target`, linking each
/// corner to the first `path_corner` its own `target` names. The walk ends at a
/// corner without a target/successor, or where it re-enters the route.
fn bsp_train_corners(bsp: &Bsp, target: &[u8]) -> Vec<TrainCorner> {
    let find = |name: &[u8], path_corner_only: bool| {
        bsp.entities.iter().position(|entity| {
            bsp_entity_value(entity, b"targetname") == Some(name)
                && (!path_corner_only || bsp_entity_value(entity, b"classname") == Some(b"path_corner".as_slice()))
        })
    };
    let mut entity_of_corner = Vec::<usize>::new();
    let mut corners = Vec::<TrainCorner>::new();
    let mut current = find(target, false);
    while let Some(entity_index) = current {
        let entity = &bsp.entities[entity_index];
        let index = corners.len();
        entity_of_corner.push(entity_index);
        corners.push(TrainCorner {
            origin: bsp_entity_value(entity, b"origin").and_then(parse_bsp_entity_triplet).unwrap_or([0.0; 3]),
            speed: parse_bsp_entity_f32(bsp_entity_value(entity, b"speed"), 0.0),
            wait: parse_bsp_entity_f32(bsp_entity_value(entity, b"wait"), 0.0),
            next: None,
        });
        let next_entity = bsp_entity_value(entity, b"target").and_then(|name| find(name, true));
        match next_entity {
            Some(next_entity) => {
                if let Some(seen) = entity_of_corner.iter().position(|&seen| seen == next_entity) {
                    corners[index].next = Some(seen);
                    break;
                }
                corners[index].next = Some(index + 1);
                current = Some(next_entity);
            }
            None => break,
        }
    }
    corners
}

pub(crate) fn bsp_brush_entities(bsp: &Bsp) -> Vec<MapBrushEntity> {
    bsp.entities
        .iter()
        .filter_map(|entity| {
            let spawn_vars = entity
                .properties
                .iter()
                .map(|(key, value)| {
                    (
                        String::from_utf8_lossy(key).into_owned(),
                        String::from_utf8_lossy(value).into_owned(),
                    )
                })
                .collect();
            // SV_SetBrushModel: `*N` is atoi(name + 1); model 0 is the world.
            let brush_model = bsp_entity_value(entity, b"model")
                .and_then(|name| std::str::from_utf8(name).ok())
                .and_then(|name| name.trim().strip_prefix('*')?.parse::<u32>().ok())
                .filter(|&model| model > 0);
            let Some(model) = brush_model else {
                // The point entities that carry a use from triggers to movers.
                let logic = bsp_entity_value(entity, b"classname").is_some_and(|class| {
                    [
                        &b"target_relay"[..],
                        b"target_delay",
                        b"target_activate",
                        b"target_deactivate",
                        b"target_counter",
                        b"trigger_always",
                    ]
                    .iter()
                    .any(|logic| class.eq_ignore_ascii_case(logic))
                });
                return logic.then_some(MapBrushEntity {
                    model: 0,
                    mins: [0.0; 3],
                    maxs: [0.0; 3],
                    train_corners: Vec::new(),
                    spawn_vars,
                });
            };
            let bounds = bsp.models.get(model as usize)?;
            let is_train = bsp_entity_value(entity, b"classname")
                .is_some_and(|class| class.eq_ignore_ascii_case(b"func_train"));
            let train_corners = if is_train {
                bsp_entity_value(entity, b"target").map(|target| bsp_train_corners(bsp, target)).unwrap_or_default()
            } else {
                Vec::new()
            };
            Some(MapBrushEntity { model, mins: bounds.mins, maxs: bounds.maxs, train_corners, spawn_vars })
        })
        .collect()
}

fn bsp_fx_runners(bsp: &Bsp, warnings: &mut Vec<String>) -> Vec<MapFxRunner> {
    let targets: BTreeMap<String, [f32; 3]> = bsp
        .entities
        .iter()
        .filter_map(|entity| {
            let name = bsp_entity_value(entity, b"targetname")?;
            let name = std::str::from_utf8(name).ok()?.trim();
            if name.is_empty() {
                return None;
            }
            let origin = bsp_entity_value(entity, b"origin").and_then(parse_bsp_entity_triplet)?;
            Some((name.to_owned(), origin))
        })
        .collect();

    bsp.entities
        .iter()
        .filter_map(|entity| {
            let classname = bsp_entity_value(entity, b"classname")?;
            if !classname.eq_ignore_ascii_case(b"fx_runner") {
                return None;
            }

            let origin = bsp_entity_value(entity, b"origin")
                .and_then(parse_bsp_entity_triplet)
                .unwrap_or([0.0; 3]);
            let Some(effect) = bsp_entity_value(entity, b"fxFile")
                .and_then(|value| std::str::from_utf8(value).ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                warnings.push(format!(
                    "fx_runner at {:.1} {:.1} {:.1} has no fxFile; skipped",
                    origin[0], origin[1], origin[2]
                ));
                return None;
            };

            let mut angles = bsp_entity_angles(entity);
            if angles == [0.0; 3] {
                // OpenJK SP_fx_runner defaults an unaimed runner to straight up.
                angles = [-90.0, 0.0, 0.0];
            }
            if let Some(target_name) = bsp_entity_value(entity, b"target")
                .and_then(|value| std::str::from_utf8(value).ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if let Some(target) = targets.get(target_name) {
                    let direction = std::array::from_fn(|axis| target[axis] - origin[axis]);
                    let length_sq = direction.iter().map(|value| value * value).sum::<f32>();
                    if length_sq > f32::EPSILON {
                        angles = vector_to_jka_angles(direction);
                    }
                } else {
                    warnings.push(format!(
                        "fx_runner target '{target_name}' not found at {:.1} {:.1} {:.1}; using authored/default angles",
                        origin[0], origin[1], origin[2]
                    ));
                }
            }

            Some(MapFxRunner {
                effect: effect.replace('\\', "/"),
                origin,
                angles,
                delay_ms: parse_bsp_entity_i32(bsp_entity_value(entity, b"delay"), 200),
                random_ms: parse_bsp_entity_f32(bsp_entity_value(entity, b"random"), 0.0) as i32,
                spawnflags: parse_bsp_entity_i32(bsp_entity_value(entity, b"spawnflags"), 0),
            })
        })
        .collect()
}

fn bsp_dynamic_lights(bsp: &Bsp) -> Vec<DynamicLight> {
    bsp.entities
        .iter()
        .filter_map(|entity| {
            let classname = std::str::from_utf8(entity.get(b"classname")?).ok()?;
            let origin = entity
                .get(b"origin")
                .and_then(|value| std::str::from_utf8(value).ok());
            let color = entity
                .get(b"_color")
                .and_then(|value| std::str::from_utf8(value).ok());
            let brightness = entity
                .get(b"light")
                .and_then(|value| std::str::from_utf8(value).ok());
            dynamic_light_from_values(classname, origin, color, brightness)
        })
        .collect()
}

fn parse_positive_map_f32(entity: &jka_assets::map::MapEntity, key: &str) -> Option<f32> {
    entity
        .properties
        .get(key)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn parse_map_f32(entity: &jka_assets::map::MapEntity, key: &str) -> Option<f32> {
    entity
        .properties
        .get(key)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
}

// NetRadiant Custom q3map2 defaults for the Quake3/JKA lighting model. Keep
// these next to source-map parsing: runtime/FX dlights intentionally retain the
// engine's existing response and do not use compiler-space photon units.
const Q3MAP_POINT_SCALE: f32 = 7500.0;
const Q3MAP_LINEAR_SCALE: f32 = 1.0 / 8000.0;
const Q3MAP_FALLOFF_TOLERANCE: f32 = 1.0;
const Q3MAP_LIGHTMAP_BYTE_SCALE: f32 = 255.0;

fn q3map_color_normalize(color: Vec3) -> Vec3 {
    let max_component = color.max_element();
    if max_component > 0.0 {
        color / max_component
    } else {
        Vec3::ONE
    }
}

fn map_world_lighting(world: &jka_assets::map::MapEntity) -> SourceMapLighting {
    // NetRadiant Custom LightWorld(): worldspawn `_color` tints both `_ambient`
    // and `_minlight`. Missing/zero color becomes white. The active JA profile
    // reports `_color colorspace: linear`, so source-map preview keeps authored
    // components in that space instead of inventing an sRGB transform here.
    let color = world
        .properties
        .get("_color")
        .and_then(|value| parse_light_triplet(value))
        .map(Vec3::from_array)
        .filter(|value| value.length_squared() > 0.0)
        .unwrap_or(Vec3::ONE);
    let ambient = parse_map_f32(world, "_ambient")
        .or_else(|| parse_map_f32(world, "ambient"))
        .unwrap_or(0.0);
    let minlight = parse_map_f32(world, "_minlight").unwrap_or(0.0);
    SourceMapLighting {
        ambient: (color * (ambient / Q3MAP_LIGHTMAP_BYTE_SCALE)).to_array(),
        minlight: (color * (minlight / Q3MAP_LIGHTMAP_BYTE_SCALE)).to_array(),
    }
}

fn map_dynamic_light(entity: &jka_assets::map::MapEntity) -> Option<DynamicLight> {
    let classname = entity.classname()?;
    if classname != "light" && classname != "lightJunior" {
        return None;
    }
    let origin = parse_light_triplet(entity.properties.get("origin")?)?;
    let spawnflags = entity
        .properties
        .get("spawnflags")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);

    // q3map2's ColorNormalize divides by the largest component, not vector
    // length. Spawnflag 32 (NRC's "unnormalized") keeps the authored values.
    // The active JA compile profile reports `_color colorspace: linear`, so no
    // sRGB conversion belongs in this source-map preview path.
    let color = entity
        .properties
        .get("_color")
        .and_then(|value| parse_light_triplet(value))
        .map(Vec3::from_array)
        .map(|color| color.max(Vec3::ZERO))
        .map(|color| {
            if spawnflags & 32 != 0 {
                color
            } else {
                q3map_color_normalize(color)
            }
        })
        .unwrap_or(Vec3::ONE)
        .to_array();

    // q3map2 priority/defaults: `_light`, then `light`, default 300. `scale`
    // multiplies authored intensity, then ordinary point lights multiply by the
    // game pointScale (7500 for the JA/Q3 profile) to become compiler photons.
    let authored = parse_positive_map_f32(entity, "_light")
        .or_else(|| parse_positive_map_f32(entity, "light"))
        .unwrap_or(300.0);
    let scale = parse_map_f32(entity, "scale")
        .filter(|value| *value != 0.0)
        .unwrap_or(1.0);
    let brightness = (authored * scale).max(0.0);
    let photons = brightness * Q3MAP_POINT_SCALE;

    let linear = spawnflags & 1 != 0;
    let fade = if linear {
        parse_map_f32(entity, "fade")
            .filter(|value| *value != 0.0)
            .unwrap_or(1.0)
            .max(1.0e-5)
    } else {
        1.0
    };
    let angle_scale = parse_map_f32(entity, "_anglescale").unwrap_or(0.0);
    // In Q3/JKA, linear (spawnflag 1) disables angle attenuation, spawnflag 2
    // also disables it, while an explicit _anglescale re-enables it.
    let angle_attenuation = angle_scale != 0.0 || (!linear && spawnflags & 2 == 0);
    let extra_distance = parse_map_f32(entity, "_extradist").unwrap_or(0.0).abs();

    // q3map2's envelope is a culling optimization. It must not be multiplied
    // into the light curve. `-fast` drops contributions <= falloffTolerance (1),
    // so inverse-square point lights become irrelevant at sqrt(photons / 1).
    // Linear lights naturally reach zero at photons*linearScale/fade. The 16u
    // minimum mirrors q3map2's hot-spot distance clamp.
    let radius = if linear {
        (photons * Q3MAP_LINEAR_SCALE / fade).max(16.0)
    } else {
        (photons / Q3MAP_FALLOFF_TOLERANCE).sqrt().max(16.0)
    };

    // q3map2 accumulates direct-light values in lightmap-byte space. Divide the
    // photons once here so the shader's resulting diffuse factor is equivalent
    // to sampling the compiled lightmap as normalized 0..1 RGB.
    let intensity = photons / Q3MAP_LIGHTMAP_BYTE_SCALE;

    Some(DynamicLight {
        position: render_position(origin),
        color,
        radius,
        intensity,
        falloff: if linear {
            DynamicLightFalloff::Linear
        } else {
            DynamicLightFalloff::InverseSquare
        },
        surface_lighting: classname != "lightJunior",
        emitter_normal: [0.0; 3],
        emitter_two_sided: false,
        angle_attenuation,
        angle_scale,
        extra_distance,
    })
}

fn map_dynamic_lights(document: &jka_assets::map::MapDocument) -> Vec<DynamicLight> {
    document.entities.iter().filter_map(map_dynamic_light).collect()
}

const BRUSH_INSIDE_EPSILON: f64 = 0.05;
fn load_movement(
    assets: &mut AssetSearchPath,
    warnings: &mut Vec<String>,
) -> Option<jka_movement::PmoveContext> {
    let result = assets
        .read("models/players/_humanoid/animation.cfg", 60_000)
        .map_err(|e| e.to_string())
        .and_then(|asset| {
            asset.ok_or_else(|| "Missing models/players/_humanoid/animation.cfg".to_string())
        })
        .and_then(|asset| jka_movement::PmoveContext::new(&asset.bytes));
    match result {
        Ok(movement) => Some(movement),
        Err(error) => {
            warnings.push(format!("Player animation timings unavailable: {error}"));
            None
        }
    }
}
const BRUSH_FACE_EPSILON: f64 = 0.1;
const BRUSH_VERTEX_EPSILON: f64 = 0.05;
const FALLBACK_TEXTURE_SIZE: f64 = 128.0;

#[derive(Debug)]
pub(crate) struct ReconstructedFace {
    pub(crate) face_index: usize,
    pub(crate) vertices: Vec<DVec3>,
}

#[derive(Debug)]
pub(crate) struct ReconstructedBrush {
    vertices: Vec<DVec3>,
    pub(crate) faces: Vec<ReconstructedFace>,
}

#[derive(Default)]
struct MapGeometry {
    vertices: Vec<GpuVertex>,
}

const SOURCE_MAP_CHUNK_SIZE: f64 = 1024.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MapChunkKey {
    x: i32,
    y: i32,
    z: i32,
}

fn map_chunk_key(vertices: &[DVec3]) -> MapChunkKey {
    let center = if vertices.is_empty() {
        DVec3::ZERO
    } else {
        vertices.iter().copied().sum::<DVec3>() / vertices.len() as f64
    };
    let coord = |value: f64| (value / SOURCE_MAP_CHUNK_SIZE).floor() as i32;
    MapChunkKey {
        x: coord(center.x),
        y: coord(center.y),
        z: coord(center.z),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MapGroupKey {
    material: usize,
    // Transparent/authored-portal geometry keeps its old per-face submission
    // isolation. Ordinary opaque geometry stays global unless GPU-driven source
    // batching was requested; environment-planar groups are spatial regardless
    // because the old source path isolated every one of those faces anyway.
    transparent_order: usize,
    chunk: Option<MapChunkKey>,
    planar_group: Option<PlanarGroupKey>,
}

struct MapMaterial {
    material: SurfaceMaterial,
    uv_size: [f64; 2],
}

#[derive(Debug)]
struct ReconstructedMapBrush {
    entity_index: usize,
    brush_index: usize,
    result: Result<ReconstructedBrush, String>,
}

fn map_vec(value: [f64; 3]) -> DVec3 {
    DVec3::new(value[0], value[1], value[2])
}

fn plane_intersection(a: &MapPlane, b: &MapPlane, c: &MapPlane) -> Option<DVec3> {
    let an = map_vec(a.normal);
    let bn = map_vec(b.normal);
    let cn = map_vec(c.normal);
    let b_cross_c = bn.cross(cn);
    let determinant = an.dot(b_cross_c);
    if determinant.abs() < 1e-6 {
        return None;
    }
    let point = (a.distance * b_cross_c + b.distance * cn.cross(an) + c.distance * an.cross(bn))
        / determinant;
    (point.x.is_finite() && point.y.is_finite() && point.z.is_finite()).then_some(point)
}

fn point_inside_brush(point: DVec3, brush: &MapBrush) -> bool {
    brush.faces.iter().all(|face| {
        map_vec(face.plane.normal).dot(point) - face.plane.distance <= BRUSH_INSIDE_EPSILON
    })
}

fn deduplicate_vertex(vertices: &mut Vec<DVec3>, point: DVec3) {
    let epsilon_sq = BRUSH_VERTEX_EPSILON * BRUSH_VERTEX_EPSILON;
    if !vertices
        .iter()
        .any(|existing| existing.distance_squared(point) <= epsilon_sq)
    {
        vertices.push(point);
    }
}

fn sort_face_vertices(vertices: &mut [DVec3], normal: DVec3) {
    let center = vertices.iter().copied().sum::<DVec3>() / vertices.len() as f64;
    let helper = if normal.z.abs() < 0.9 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let tangent = helper.cross(normal).normalize_or_zero();
    let bitangent = normal.cross(tangent);
    vertices.sort_by(|a, b| {
        let da = *a - center;
        let db = *b - center;
        let aa = da.dot(bitangent).atan2(da.dot(tangent));
        let ab = db.dot(bitangent).atan2(db.dot(tangent));
        aa.total_cmp(&ab)
    });

    if vertices.len() >= 3 {
        let triangle_normal = (vertices[1] - vertices[0]).cross(vertices[2] - vertices[0]);
        if triangle_normal.dot(normal) < 0.0 {
            vertices.reverse();
        }
    }
}

fn brush_has_unbounded_direction(brush: &MapBrush) -> bool {
    const DIRECTION_EPSILON: f64 = 1e-8;
    for i in 0..brush.faces.len().saturating_sub(1) {
        let a = map_vec(brush.faces[i].plane.normal);
        for j in i + 1..brush.faces.len() {
            let direction = a.cross(map_vec(brush.faces[j].plane.normal));
            let length_squared = direction.length_squared();
            if length_squared <= DIRECTION_EPSILON * DIRECTION_EPSILON {
                continue;
            }
            let direction = direction / length_squared.sqrt();
            for candidate in [direction, -direction] {
                if brush
                    .faces
                    .iter()
                    .all(|face| map_vec(face.plane.normal).dot(candidate) <= DIRECTION_EPSILON)
                {
                    return true;
                }
            }
        }
    }
    false
}

pub(crate) fn reconstruct_brush(brush: &MapBrush) -> Result<ReconstructedBrush, String> {
    if brush.faces.len() < 4 {
        return Err("fewer than four valid planes".into());
    }
    if brush_has_unbounded_direction(brush) {
        return Err("plane half-spaces do not form a bounded volume".into());
    }

    let mut vertices = Vec::new();
    for i in 0..brush.faces.len() - 2 {
        for j in i + 1..brush.faces.len() - 1 {
            for k in j + 1..brush.faces.len() {
                let Some(point) = plane_intersection(
                    &brush.faces[i].plane,
                    &brush.faces[j].plane,
                    &brush.faces[k].plane,
                ) else {
                    continue;
                };
                if point_inside_brush(point, brush) {
                    deduplicate_vertex(&mut vertices, point);
                }
            }
        }
    }
    if vertices.len() < 4 {
        return Err(format!(
            "only {} bounded corner(s) reconstructed",
            vertices.len()
        ));
    }

    let mut faces = Vec::new();
    for (face_index, face) in brush.faces.iter().enumerate() {
        let normal = map_vec(face.plane.normal);
        let mut face_vertices: Vec<_> = vertices
            .iter()
            .copied()
            .filter(|point| (normal.dot(*point) - face.plane.distance).abs() <= BRUSH_FACE_EPSILON)
            .collect();
        if face_vertices.len() < 3 {
            return Err(format!(
                "face {face_index} does not bound a polygon ({} on-plane corners)",
                face_vertices.len()
            ));
        }
        sort_face_vertices(&mut face_vertices, normal);
        faces.push(ReconstructedFace {
            face_index,
            vertices: face_vertices,
        });
    }
    if faces.len() < 4 {
        return Err(format!(
            "only {} face polygon(s) reconstructed",
            faces.len()
        ));
    }

    Ok(ReconstructedBrush { vertices, faces })
}

fn reconstruct_source_brushes(
    document: Arc<MapDocument>,
    static_entity_indices: &[usize],
    jobs: Option<&MapJobPool>,
) -> Result<Vec<ReconstructedMapBrush>, String> {
    let work = static_entity_indices
        .iter()
        .flat_map(|&entity_index| {
            (0..document.entities[entity_index].brushes.len())
                .map(move |brush_index| (entity_index, brush_index))
        })
        .collect::<Vec<_>>();

    if work.len() < 256 || jobs.is_none() {
        return Ok(work
            .into_iter()
            .map(|(entity_index, brush_index)| ReconstructedMapBrush {
                entity_index,
                brush_index,
                result: reconstruct_brush(&document.entities[entity_index].brushes[brush_index]),
            })
            .collect());
    }

    let jobs = jobs.expect("source reconstruction jobs");
    let job_count = (jobs.worker_count().saturating_mul(2)).clamp(1, work.len());
    let chunk_size = work.len().div_ceil(job_count);
    let mut handles = Vec::new();
    for chunk in work.chunks(chunk_size) {
        let document = Arc::clone(&document);
        let chunk = chunk.to_vec();
        handles.push(jobs.submit(Task::MapPrepare, move || {
            chunk
                .into_iter()
                .map(|(entity_index, brush_index)| ReconstructedMapBrush {
                    entity_index,
                    brush_index,
                    result: reconstruct_brush(&document.entities[entity_index].brushes[brush_index]),
                })
                .collect::<Vec<_>>()
        })?);
    }

    let mut reconstructed = Vec::with_capacity(work.len());
    for handle in handles {
        reconstructed.extend(handle.join()?);
    }
    // Worker completion order must not affect transparent ordering, warning order,
    // or generated vertex ranges. Preserve the original entity/brush order.
    reconstructed.sort_by_key(|item| (item.entity_index, item.brush_index));
    Ok(reconstructed)
}

fn legacy_texture_axes(normal: DVec3) -> (DVec3, DVec3) {
    const AXES: [([f64; 3], [f64; 3], [f64; 3]); 6] = [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
    ];
    // Match q3map's TextureAxisFromPlane exactly: ties keep the earlier axis.
    let mut best = 0.0;
    let mut best_axis = 0usize;
    for (index, (axis, _, _)) in AXES.iter().enumerate() {
        let candidate = map_vec(*axis).dot(normal);
        if candidate > best {
            best = candidate;
            best_axis = index;
        }
    }
    (map_vec(AXES[best_axis].1), map_vec(AXES[best_axis].2))
}

fn quake_sin_cos(degrees: f64) -> (f64, f64) {
    match degrees {
        0.0 => (0.0, 1.0),
        90.0 => (1.0, 0.0),
        180.0 => (0.0, -1.0),
        270.0 => (-1.0, 0.0),
        _ => degrees.to_radians().sin_cos(),
    }
}

fn brush_primitive_axes(normal: DVec3) -> (DVec3, DVec3) {
    // q3map2/GtkRadiant brush-primitives basis: rotate world space around Z and
    // then Y so the local Z axis aligns with the face normal. The stored 2x3
    // matrix maps the resulting local X/Y coordinates directly to normalized UV.
    let theta_z = if normal.x.abs() > 1e-9 || normal.y.abs() > 1e-9 {
        normal.y.atan2(normal.x)
    } else {
        0.0
    };
    let theta_y = normal
        .z
        .atan2((normal.x * normal.x + normal.y * normal.y).sqrt());
    let x_axis = DVec3::new(-theta_z.sin(), theta_z.cos(), 0.0);
    let y_axis = DVec3::new(
        theta_y.sin() * theta_z.cos(),
        theta_y.sin() * theta_z.sin(),
        -theta_y.cos(),
    );
    (x_axis, y_axis)
}

fn safe_scale(value: f64) -> f64 {
    if value.abs() < 1e-9 {
        1.0
    } else {
        value
    }
}

fn face_uv(face: &MapFace, point: DVec3, texture_size: [f64; 2]) -> [f32; 2] {
    let width = texture_size[0].max(1.0);
    let height = texture_size[1].max(1.0);
    let normal = map_vec(face.plane.normal);
    let uv = match &face.projection {
        TextureProjection::Legacy {
            shift,
            rotate,
            scale,
        } => {
            let (base_u, base_v) = legacy_texture_axes(normal);
            let (sin, cos) = quake_sin_cos(*rotate);
            let u_axis = base_u * cos - base_v * sin;
            let v_axis = base_u * sin + base_v * cos;
            [
                (point.dot(u_axis) / safe_scale(scale[0]) + shift[0]) / width,
                (point.dot(v_axis) / safe_scale(scale[1]) + shift[1]) / height,
            ]
        }
        TextureProjection::Valve220 {
            u_axis,
            u_shift,
            v_axis,
            v_shift,
            scale,
            ..
        } => [
            (point.dot(map_vec(*u_axis)) / safe_scale(scale[0]) + *u_shift) / width,
            (point.dot(map_vec(*v_axis)) / safe_scale(scale[1]) + *v_shift) / height,
        ],
        TextureProjection::BrushPrimitive { matrix } => {
            let (x_axis, y_axis) = brush_primitive_axes(normal);
            let x = point.dot(x_axis);
            let y = point.dot(y_axis);
            [
                matrix[0][0] * x + matrix[0][1] * y + matrix[0][2],
                matrix[1][0] * x + matrix[1][1] * y + matrix[1][2],
            ]
        }
    };
    [uv[0] as f32, uv[1] as f32]
}

fn is_utility_shader(name: &str) -> bool {
    let normalized = name.replace('\\', "/").to_ascii_lowercase();
    let normalized = normalized.strip_prefix("textures/").unwrap_or(&normalized);
    let leaf = normalized.rsplit('/').next().unwrap_or(normalized);
    matches!(
        leaf,
        "caulk"
            | "nodraw"
            | "clip"
            | "playerclip"
            | "monsterclip"
            | "botclip"
            | "weaponclip"
            | "trigger"
            | "hint"
            | "skip"
            | "areaportal"
            | "origin"
            | "lightgrid"
            | "cushion"
            | "fog"
    ) || leaf.starts_with("caulk")
        || leaf.starts_with("nodraw")
        || leaf.starts_with("trigger_")
        || leaf.starts_with("clip_")
        || leaf.ends_with("_clip")
}

/// Shader-script metadata is authoritative for utility-only surfaces. A custom
/// shader does not have to be named `caulk`/`playerclip`/etc, so filename
/// heuristics alone can accidentally send compiler-only faces to WGPU. Keep
/// visible nonsolid materials (glass/water/etc.) out of this test.
fn shader_is_render_utility(shader: &Shader) -> bool {
    shader.nodraw
        || shader.player_clip
        || shader.monster_clip
        || shader.bot_clip
        || shader.shot_clip
        || shader.trigger
        || shader.fog
}

const SOURCE_CONTENTS_SOLID: u32 = 0x0000_0001;
const SOURCE_CONTENTS_LAVA: u32 = 0x0000_0002;
const SOURCE_CONTENTS_WATER: u32 = 0x0000_0004;
const SOURCE_CONTENTS_FOG: u32 = 0x0000_0008;
const SOURCE_CONTENTS_PLAYERCLIP: u32 = 0x0000_0010;
const SOURCE_CONTENTS_MONSTERCLIP: u32 = 0x0000_0020;
const SOURCE_CONTENTS_BOTCLIP: u32 = 0x0000_0040;
const SOURCE_CONTENTS_SHOTCLIP: u32 = 0x0000_0080;
const SOURCE_CONTENTS_TRIGGER: u32 = 0x0000_0400;
const SOURCE_CONTENTS_OPAQUE: u32 = 0x0000_8000;
const SOURCE_SURF_SKY: u32 = 0x0000_2000;
const SOURCE_SURF_SLICK: u32 = 0x0000_4000;
const SOURCE_SURF_METALSTEPS: u32 = 0x0000_8000;
const SOURCE_SURF_NODAMAGE: u32 = 0x0004_0000;
const SOURCE_SURF_NODRAW: u32 = 0x0020_0000;
const SOURCE_SURF_NOSTEPS: u32 = 0x0040_0000;
const SOURCE_SURF_NOMISCENTS: u32 = 0x0100_0000;
const SOURCE_SURF_BEVELS_MASK: u32 = SOURCE_SURF_SLICK
    | SOURCE_SURF_METALSTEPS
    | SOURCE_SURF_NODAMAGE
    | SOURCE_SURF_NOSTEPS
    | SOURCE_SURF_NOMISCENTS;

fn source_shader_leaf(name: &str) -> String {
    let normalized = name.replace('\\', "/").to_ascii_lowercase();
    normalized.rsplit('/').next().unwrap_or(&normalized).to_string()
}

fn source_face_collision_flags(face: &MapFace, shader: Option<&Shader>) -> (i32, i32) {
    let leaf = source_shader_leaf(&face.shader);
    // q3map2's JA/SOF2 table applies this default before shader surfaceParms.
    let mut contents = SOURCE_CONTENTS_SOLID | SOURCE_CONTENTS_OPAQUE;
    let mut surface_flags = 0u32;

    if let Some(shader) = shader {
        contents &= !shader.collision_contents_clear;
        contents |= shader.collision_contents_add;
        surface_flags &= !shader.collision_surface_flags_clear;
        surface_flags |= shader.collision_surface_flags_add;

        // skyparms can identify a sky shader even when an unusual script omits
        // `surfaceparm sky`; preserve the gameplay surface bit in that case.
        if shader.sky { surface_flags |= SOURCE_SURF_SKY; }
        if shader.slick { surface_flags |= SOURCE_SURF_SLICK; }
        if shader.nodraw { surface_flags |= SOURCE_SURF_NODRAW; }
    } else {
        // Fallback only when no shader script exists. These mirror the normal
        // JKA tool-texture intent closely enough that loose source maps remain
        // playable; a loaded shader always wins over filename heuristics.
        match leaf.as_str() {
            "clip" | "playerclip" => {
                contents = SOURCE_CONTENTS_PLAYERCLIP;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "monsterclip" => {
                contents = SOURCE_CONTENTS_MONSTERCLIP;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "botclip" => {
                contents = SOURCE_CONTENTS_BOTCLIP;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "weaponclip" | "shotclip" => {
                contents = SOURCE_CONTENTS_SHOTCLIP;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "trigger" => {
                contents = SOURCE_CONTENTS_TRIGGER;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "water" => contents = SOURCE_CONTENTS_WATER | SOURCE_CONTENTS_OPAQUE,
            "lava" => contents = SOURCE_CONTENTS_LAVA | SOURCE_CONTENTS_OPAQUE,
            // Raven's SOF2/JKA SLIME bit is also used as projectileclip.
            "slime" => contents = 0x0002_0000 | SOURCE_CONTENTS_OPAQUE,
            "fog" => contents = SOURCE_CONTENTS_FOG,
            "nodraw" => surface_flags |= SOURCE_SURF_NODRAW,
            _ if leaf.starts_with("caulk") || leaf.starts_with("nodraw") => {
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            _ => {}
        }
    }

    // Legacy .map writers may append q3map contents/surfaceFlags/value after
    // the texture projection. Radiant/q3map writers commonly store compile
    // modifiers such as CONTENTS_DETAIL there without repeating the material
    // contents already deduced from the shader. Treating a modifier as the
    // complete gameplay contents makes ordinary detail floors non-solid. Merge
    // modifier-only values; replace only when the explicit bits actually name a
    // gameplay volume/collision class.
    if let Some(value) = face.trailing.first().copied().filter(|value| value.is_finite()) {
        let explicit = value.round() as i64 as u32;
        if explicit != 0 {
            const EXPLICIT_GAMEPLAY_CLASS: u32 = SOURCE_CONTENTS_SOLID
                | SOURCE_CONTENTS_LAVA
                | SOURCE_CONTENTS_WATER
                | SOURCE_CONTENTS_FOG
                | SOURCE_CONTENTS_PLAYERCLIP
                | SOURCE_CONTENTS_MONSTERCLIP
                | SOURCE_CONTENTS_BOTCLIP
                | SOURCE_CONTENTS_SHOTCLIP
                | SOURCE_CONTENTS_TRIGGER
                | 0x0000_1000 // CONTENTS_TERRAIN
                | 0x0002_0000; // CONTENTS_SLIME
            if explicit & EXPLICIT_GAMEPLAY_CLASS == 0 {
                // Compile modifiers (DETAIL, TRANSLUCENT, etc.) augment the
                // contents already deduced from the material. This also preserves
                // an authored surfaceParm nonsolid clear from the shader parser.
                contents |= explicit;
            } else {
                // Actual gameplay content classes are authoritative: playerclip,
                // liquids, trigger volumes, explicit solid, and so on must not
                // inherit ordinary SOLID merely because the face has a texture.
                contents = explicit;
            }
        }
    }
    if let Some(value) = face.trailing.get(1).copied().filter(|value| value.is_finite()) {
        let explicit = value.round() as i64 as u32;
        if explicit != 0 { surface_flags = explicit; }
    }
    (contents as i32, surface_flags as i32)
}

fn push_source_collision_plane(
    planes: &mut Vec<jka_movement::SourceCollisionPlane>,
    normal: DVec3,
    distance: f64,
    surface_flags: i32,
) {
    let length = normal.length();
    if !length.is_finite() || length < 1e-9 || !distance.is_finite() {
        return;
    }
    let normal = normal / length;
    let distance = distance / length;
    let n = [normal.x as f32, normal.y as f32, normal.z as f32];
    let d = distance as f32;
    if let Some(existing) = planes.iter_mut().find(|plane| {
        (plane.normal[0] - n[0]).abs() < 1e-4
            && (plane.normal[1] - n[1]).abs() < 1e-4
            && (plane.normal[2] - n[2]).abs() < 1e-4
            && (plane.distance - d).abs() < 0.05
    }) {
        // q3map2 ORs the bevel-relevant surface flags when a generated bevel
        // resolves to an already-present plane.
        existing.surface_flags |= surface_flags;
    } else {
        planes.push(jka_movement::SourceCollisionPlane { normal: n, distance: d, surface_flags });
    }
}

fn source_collision_brush(
    brush: &MapBrush,
    reconstructed: &ReconstructedBrush,
    library: &BTreeMap<String, Shader>,
) -> Option<jka_movement::SourceCollisionBrush> {
    if reconstructed.vertices.len() < 4 {
        return None;
    }
    let mut minimum = DVec3::splat(f64::INFINITY);
    let mut maximum = DVec3::splat(f64::NEG_INFINITY);
    for &point in &reconstructed.vertices {
        minimum = minimum.min(point);
        maximum = maximum.max(point);
    }

    let mut planes = Vec::new();
    let mut contents = 0i32;
    let mut face_surface_flags = Vec::with_capacity(brush.faces.len());
    for face in &brush.faces {
        let shader_name = canonical_map_shader(&face.shader, library);
        let (face_contents, surface_flags) =
            source_face_collision_flags(face, library.get(&shader_name));
        contents |= face_contents;
        face_surface_flags.push(surface_flags);
        push_source_collision_plane(
            &mut planes,
            map_vec(face.plane.normal),
            face.plane.distance,
            surface_flags,
        );
    }
    if contents == 0 || planes.len() < 4 {
        return None;
    }

    // q3map2 adds axial and edge bevels to collision brushes. Face planes alone
    // are sufficient for point traces, but a swept player AABB needs these
    // additional Minkowski planes around slanted brush edges to match CM.
    const BEVEL_EPSILON: f64 = 0.1;
    for axis in 0..3 {
        let axial_flags = |coordinate: f64| -> i32 {
            reconstructed
                .faces
                .iter()
                .filter(|polygon| {
                    polygon
                        .vertices
                        .iter()
                        .any(|vertex| (vertex[axis] - coordinate).abs() < BEVEL_EPSILON)
                })
                .fold(0u32, |flags, polygon| {
                    flags
                        | (face_surface_flags
                            .get(polygon.face_index)
                            .copied()
                            .unwrap_or_default() as u32
                            & SOURCE_SURF_BEVELS_MASK)
                }) as i32
        };
        let mut positive = DVec3::ZERO;
        positive[axis] = 1.0;
        push_source_collision_plane(
            &mut planes,
            positive,
            maximum[axis],
            axial_flags(maximum[axis]),
        );
        push_source_collision_plane(
            &mut planes,
            -positive,
            -minimum[axis],
            axial_flags(minimum[axis]),
        );
    }
    const MAX_SOURCE_COLLISION_PLANES: usize = 256;
    'faces: for polygon in &reconstructed.faces {
        if polygon.vertices.len() < 2 { continue; }
        for edge_index in 0..polygon.vertices.len() {
            let a = polygon.vertices[edge_index];
            let b = polygon.vertices[(edge_index + 1) % polygon.vertices.len()];
            let edge = b - a;
            let edge_length = edge.length();
            if edge_length < 1e-6 { continue; }
            let edge = edge / edge_length;
            if edge.x.abs() > 0.9999 || edge.y.abs() > 0.9999 || edge.z.abs() > 0.9999 {
                continue;
            }
            for axis in 0..3 {
                for sign in [-1.0, 1.0] {
                    let mut axis_vector = DVec3::ZERO;
                    axis_vector[axis] = sign;
                    let candidate = edge.cross(axis_vector);
                    let length = candidate.length();
                    if length < 1e-6 { continue; }
                    let normal = candidate / length;
                    let distance = normal.dot(a);
                    let mut has_inside = false;
                    let valid = reconstructed.vertices.iter().all(|point| {
                        let delta = normal.dot(*point) - distance;
                        if delta < -BEVEL_EPSILON { has_inside = true; }
                        delta <= BEVEL_EPSILON
                    });
                    if valid && has_inside {
                        let bevel_surface_flags = face_surface_flags
                            .get(polygon.face_index)
                            .copied()
                            .unwrap_or_default() as u32
                            & SOURCE_SURF_BEVELS_MASK;
                        push_source_collision_plane(
                            &mut planes,
                            normal,
                            distance,
                            bevel_surface_flags as i32,
                        );
                        if planes.len() >= MAX_SOURCE_COLLISION_PLANES { break 'faces; }
                    }
                }
            }
        }
    }

    Some(jka_movement::SourceCollisionBrush {
        planes,
        mins: [minimum.x as f32, minimum.y as f32, minimum.z as f32],
        maxs: [maximum.x as f32, maximum.y as f32, maximum.z as f32],
        contents,
    })
}

fn parse_map_triplet(value: Option<&String>) -> Option<[f32; 3]> {
    let mut values = value?.split_whitespace().map(str::parse::<f32>);
    let result = [values.next()?.ok()?, values.next()?.ok()?, values.next()?.ok()?];
    (values.next().is_none() && result.iter().all(|value| value.is_finite())).then_some(result)
}

/// FFA spawns only (`info_player_start` is an `info_player_deathmatch` alias);
/// duel/siege spots are used only when a map has nothing else.
fn map_spawn_points(document: &MapDocument) -> Vec<SpawnPoint> {
    let ffa = map_spawn_points_of(document, |classname| {
        matches!(classname, "info_player_deathmatch" | "info_player_start")
    });
    if !ffa.is_empty() {
        return ffa;
    }
    map_spawn_points_of(document, |classname| {
        matches!(classname, "info_player_duel" | "info_player_siegeteam1" | "info_player_siegeteam2")
    })
}

fn map_spawn_points_of(document: &MapDocument, accepts: impl Fn(&str) -> bool) -> Vec<SpawnPoint> {
    document.entities.iter().filter_map(|entity| {
        if !entity.classname().is_some_and(&accepts) {
            return None;
        }
        let mut origin = parse_map_triplet(entity.properties.get("origin"))?;
        // BSP spawn preparation applies the same small floor nudge used by JKA.
        origin[2] += 9.0;
        let yaw = entity.properties.get("angle")
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .or_else(|| parse_map_triplet(entity.properties.get("angles")).map(|angles| angles[1]))
            .unwrap_or(0.0);
        let integer = |key: &str| {
            entity.properties.get(key).and_then(|value| value.trim().parse::<i32>().ok()).unwrap_or(0)
        };
        Some(SpawnPoint {
            position: render_position(origin),
            yaw: yaw.to_radians(),
            initial: integer("spawnflags") & 1 != 0,
            no_humans: integer("nohumans") != 0,
        })
    }).collect()
}

fn canonical_map_shader(name: &str, library: &BTreeMap<String, Shader>) -> String {
    let raw = name.replace('\\', "/").to_ascii_lowercase();
    if library.contains_key(&raw) || raw.starts_with("textures/") {
        return raw;
    }
    // Loose map writers commonly omit `textures/`; direct image lookup still
    // needs the package-relative texture path even without a shader script.
    format!("textures/{raw}")
}

fn material_uv_size(material: &SurfaceMaterial, textures: &Textures) -> [f64; 2] {
    material
        .stages
        .iter()
        .find_map(|stage| match stage.texture {
            StageTexture::Image(index) => textures
                .images
                .get(index)
                .map(|image| [f64::from(image.width), f64::from(image.height)]),
            StageTexture::Lightmap | StageTexture::White => None,
        })
        .unwrap_or([FALLBACK_TEXTURE_SIZE; 2])
}

fn add_bounds(bounds: &mut Option<(DVec3, DVec3)>, point: DVec3) {
    match bounds {
        Some((minimum, maximum)) => {
            *minimum = minimum.min(point);
            *maximum = maximum.max(point);
        }
        None => *bounds = Some((point, point)),
    }
}

fn push_face_triangles(
    output: &mut Vec<GpuVertex>,
    face: &MapFace,
    polygon: &[DVec3],
    texture_size: [f64; 2],
) -> usize {
    if polygon.len() < 3 {
        return 0;
    }
    let normal = map_vec(face.plane.normal);
    let render_normal = render_position(face.plane.normal.map(|value| value as f32));
    let mut triangles = 0;
    for index in 1..polygon.len() - 1 {
        let points = [polygon[0], polygon[index], polygon[index + 1]];
        let area = (points[1] - points[0]).cross(points[2] - points[0]);
        if area.length_squared() < 1e-12 {
            continue;
        }
        let points = if area.dot(normal) >= 0.0 {
            points
        } else {
            [points[0], points[2], points[1]]
        };
        for point in points {
            output.push(GpuVertex {
                position: render_position(point.to_array().map(|value| value as f32)),
                uv: face_uv(face, point, texture_size),
                lightmap_uv: [-1.0, -1.0],
                normal: render_normal,
                color: [1.0; 4],
                alpha_cutoff: 1.0,
            });
        }
        triangles += 1;
    }
    triangles
}

pub fn prepare_source(
    root: &Path,
    game: Option<&Path>,
    source: &MapSource,
) -> Result<PreparedMap, String> {
    match source {
        MapSource::Bsp(name) => prepare(root, game, name),
        MapSource::Map(name) => prepare_map_asset(root, game, name),
        MapSource::MapFile(path) => prepare_map_file(root, game, path),
        MapSource::MapEditPreview { path, text } => {
            prepare_map_edit_preview(root, game, path, text, None, MapPrepareOptions::default())
        }
    }
}

pub fn prepare_source_with_jobs(
    root: &Path,
    game: Option<&Path>,
    source: &MapSource,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
    seed: Option<&Arc<PreparedMap>>,
) -> Result<PreparedMap, String> {
    match source {
        MapSource::Bsp(name) => prepare_with_jobs_options(root, game, name, jobs, options, seed),
        MapSource::Map(name) => prepare_map_asset_with_jobs(root, game, name, jobs, options),
        MapSource::MapFile(path) => prepare_map_file_with_jobs(root, game, path, jobs, options),
        MapSource::MapEditPreview { path, text } => {
            prepare_map_edit_preview(root, game, path, text, Some(jobs), options)
        }
    }
}

pub fn prepare_map_asset(
    root: &Path,
    game: Option<&Path>,
    name: &str,
) -> Result<PreparedMap, String> {
    prepare_map_asset_inner(root, game, name, None, MapPrepareOptions::default())
}

fn prepare_map_asset_with_jobs(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    prepare_map_asset_inner(root, game, name, Some(jobs), options)
}

fn prepare_map_asset_inner(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    let asset_name = map_asset_name(name, "map")?;
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    let asset = assets
        .read(&asset_name, MAX_MAP_FILE_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!("Source map {asset_name} not found on the game/base asset search path")
        })?;
    let text = std::str::from_utf8(&asset.bytes)
        .map_err(|error| format!("{asset_name} is not UTF-8 text: {error}"))?;
    let document =
        jka_assets::map::parse(text).map_err(|error| format!("{asset_name}: {error}"))?;
    prepare_map_document_with_assets_options(&asset.source, document, &mut assets, jobs, options)
}

pub fn prepare_map_file(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
) -> Result<PreparedMap, String> {
    prepare_map_file_inner(root, game, path, None, MapPrepareOptions::default())
}

fn prepare_map_file_with_jobs(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    prepare_map_file_inner(root, game, path, Some(jobs), options)
}

fn prepare_map_file_inner(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("map"))
    {
        return Err(format!(
            "Loose map source must use a .map extension: {}",
            path.display()
        ));
    }
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not stat loose map {}: {error}", path.display()))?;
    if metadata.len() > MAX_MAP_FILE_BYTES as u64 {
        return Err(format!(
            "Loose map {} exceeds {} MiB limit",
            path.display(),
            MAX_MAP_FILE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read loose map {}: {error}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("Loose map {} is not UTF-8 text: {error}", path.display()))?;
    let document =
        jka_assets::map::parse(text).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    prepare_map_document_with_assets_options(path, document, &mut assets, jobs, options)
}

fn prepare_map_edit_preview(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
    text: &str,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("map"))
    {
        return Err(format!(
            "Edited source-map preview must refer to a .map file: {}",
            path.display()
        ));
    }
    if text.len() > MAX_MAP_FILE_BYTES {
        return Err(format!(
            "Edited map preview {} exceeds {} MiB limit",
            path.display(),
            MAX_MAP_FILE_BYTES / (1024 * 1024)
        ));
    }
    let document = jka_assets::map::parse(text)
        .map_err(|error| format!("Edited preview {}: {error}", path.display()))?;
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    prepare_map_document_with_assets_options(path, document, &mut assets, jobs, options)
}

#[cfg(test)]
fn prepare_map_document(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
    document: MapDocument,
) -> Result<PreparedMap, String> {
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    prepare_map_document_with_assets_options(
        path, document, &mut assets, None, MapPrepareOptions::default(),
    )
}

fn prepare_map_document_with_assets_options(
    path: &Path,
    document: MapDocument,
    assets: &mut AssetSearchPath,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    assets.set_allow_asset_overrides(options.allow_asset_overrides);
    let document = Arc::new(document);
    let world_indices: Vec<_> = document
        .entities
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| (entity.classname() == Some("worldspawn")).then_some(index))
        .collect();
    let Some(&world_index) = world_indices.first() else {
        return Err("Source .map has no worldspawn entity".into());
    };

    let mut warnings = Vec::new();
    if world_indices.len() > 1 {
        warnings.push(format!(
            "{} worldspawn entities found; rendering only the first",
            world_indices.len()
        ));
    }
    if document.stats.patches_skipped != 0 {
        warnings.push(format!(
            "{} patchDef2/patchDef3 object(s) skipped (Milestone 1 supports brushes only)",
            document.stats.patches_skipped
        ));
    }
    if document.stats.unsupported_objects_skipped != 0 {
        warnings.push(format!(
            "{} unsupported map object block(s) skipped",
            document.stats.unsupported_objects_skipped
        ));
    }
    if document.stats.degenerate_faces != 0 {
        warnings.push(format!(
            "{} degenerate brush face plane(s) ignored while parsing",
            document.stats.degenerate_faces
        ));
    }

    // q3map2 treats func_group as an editor/compiler grouping construct rather
    // than a runtime brush entity. Its brushes belong to the static world.
    // Keep true runtime/submodel brush entities separate until entity/model
    // transforms and behavior are implemented.
    let grouped_world_indices: Vec<_> = document
        .entities
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| (entity.classname() == Some("func_group")).then_some(index))
        .collect();
    let grouped_world_brushes: usize = grouped_world_indices
        .iter()
        .map(|index| document.entities[*index].brushes.len())
        .sum();

    let skipped_entity_brushes = document
        .entities
        .iter()
        .enumerate()
        .filter(|(index, entity)| {
            *index != world_index
                && entity.classname() != Some("worldspawn")
                && entity.classname() != Some("func_group")
        })
        .map(|(_, entity)| entity.brushes.len())
        .sum();
    if skipped_entity_brushes != 0 {
        warnings.push(format!(
            "{skipped_entity_brushes} runtime/non-static entity brush(es) parsed but skipped"
        ));
    }
    let additional_world_brushes: usize = world_indices
        .iter()
        .skip(1)
        .map(|index| document.entities[*index].brushes.len())
        .sum();
    if additional_world_brushes != 0 {
        warnings.push(format!(
            "{additional_world_brushes} brush(es) in additional worldspawn entities skipped"
        ));
    }

    let world = &document.entities[world_index];
    let distance_cull = map_worldspawn_distance_cull(world, &mut warnings);
    let mut static_entity_indices = Vec::with_capacity(1 + grouped_world_indices.len());
    static_entity_indices.push(world_index);
    static_entity_indices.extend(grouped_world_indices.iter().copied());
    let (library, library_debug) = if let Some(jobs) = jobs {
        materials::shader_library_with_jobs(assets, &mut warnings, jobs, options.pbr_materials)?
    } else {
        materials::shader_library(assets, &mut warnings, options.pbr_materials)?
    };
    let mut textures = Textures::new();
    let mut material_lookup = BTreeMap::<String, usize>::new();
    let mut map_materials = Vec::<MapMaterial>::new();
    let mut groups = BTreeMap::<MapGroupKey, MapGeometry>::new();
    let mut triangles = 0usize;
    let mut rendered_faces = 0usize;
    let mut utility_faces_skipped = 0usize;
    let mut skipped_brushes = 0usize;
    let mut bounds = None;
    let mut transparent_order = 0usize;

    let reconstruction_started = Instant::now();
    let reconstructed_brushes = reconstruct_source_brushes(
        Arc::clone(&document),
        &static_entity_indices,
        jobs,
    )?;
    let reconstruction_ms = reconstruction_started.elapsed().as_secs_f64() * 1000.0;
    let mut source_collision_brushes = Vec::new();

    for item in reconstructed_brushes {
        let entity_index = item.entity_index;
        let brush_index = item.brush_index;
        let entity = &document.entities[entity_index];
        let entity_kind = if entity_index == world_index {
            "worldspawn"
        } else {
            "func_group"
        };
        let brush = &entity.brushes[brush_index];
        let reconstructed = match item.result {
            Ok(brush) => brush,
            Err(error) => {
                skipped_brushes += 1;
                warnings.push(format!(
                    "{entity_kind} entity {entity_index} brush {brush_index} (line {}): {error}; brush skipped",
                    brush.line
                ));
                continue;
            }
        };
        for &point in &reconstructed.vertices {
            add_bounds(&mut bounds, point);
        }

        // Gameplay collision is deliberately generated before the render-only
        // utility-surface filter below. Caulk/nodraw/playerclip must stay in CM
        // even though they never consume a WGPU draw batch.
        if let Some(collision_brush) = source_collision_brush(brush, &reconstructed, &library) {
            source_collision_brushes.push(collision_brush);
        }

        // Match the conservative Radiant large-map index: one static brush
        // belongs to the 1024-unit cell containing its centre. A brush may
        // extend beyond that cell; renderer batch bounds are computed from
        // the actual vertices, so frustum rejection remains conservative.
        let brush_chunk = map_chunk_key(&reconstructed.vertices);

        for polygon in reconstructed.faces {
            let face = &brush.faces[polygon.face_index];
            if is_utility_shader(&face.shader) {
                utility_faces_skipped += 1;
                continue;
            }

            let shader_name = canonical_map_shader(&face.shader, &library);
            if library.get(&shader_name).is_some_and(shader_is_render_utility) {
                utility_faces_skipped += 1;
                continue;
            }
            let material_index = if let Some(&index) = material_lookup.get(&shader_name) {
                index
            } else {
                let material = materials::describe(
                    &shader_name,
                    0,
                    library.get(&shader_name),
                    library_debug.origins.get(&shader_name),
                    assets,
                    &mut textures,
                    options.gen_normal_maps,
                    options.omit_environment_stages,
                );
                let uv_size = material_uv_size(&material, &textures);
                let index = map_materials.len();
                map_materials.push(MapMaterial { material, uv_size });
                material_lookup.insert(shader_name, index);
                index
            };
            let map_material = &map_materials[material_index];
            if map_material.material.hidden {
                continue;
            }

            // Build this face first so environment-mapped opaque geometry can
            // use the same coplanar grouping rule as the BSP path. The old
            // source-map path isolated every tcGen environment face, which can
            // turn a large map into thousands of WGPU commands.
            let mut face_vertices = Vec::with_capacity(
                polygon.vertices.len().saturating_sub(2).saturating_mul(3),
            );
            let face_triangles = push_face_triangles(
                &mut face_vertices,
                face,
                &polygon.vertices,
                map_material.uv_size,
            );
            if face_triangles == 0 {
                continue;
            }

            let class = class_for(&map_material.material);
            let has_environment_stage = map_material
                .material
                .stages
                .iter()
                .any(|stage| matches!(stage.tc_gen, TcGen::Environment));
            let planar_candidate = options.planar_reflections
                && class != DrawClass::Transparent
                && (map_material.material.planar_reflection
                    || (options.planar_environment && has_environment_stage));
            let planar_group = planar_candidate
                .then(|| planar_group_key(&face_vertices))
                .flatten();
            let preserve_unique_plane = planar_candidate && planar_group.is_none();
            if class == DrawClass::Transparent || preserve_unique_plane {
                transparent_order += 1;
            }
            let unique_submission = class == DrawClass::Transparent || preserve_unique_plane;
            let spatial_submission = planar_group.is_some()
                || (options.source_spatial_batches && !unique_submission);
            let key = MapGroupKey {
                material: material_index,
                transparent_order: if unique_submission { transparent_order } else { 0 },
                chunk: spatial_submission.then_some(brush_chunk),
                planar_group,
            };
            groups.entry(key).or_default().vertices.extend(face_vertices);
            triangles += face_triangles;
            rendered_faces += 1;
        }
    }

    let sun = authored_sun(
        material_lookup.keys().map(String::as_str),
        &library,
        &mut warnings,
    );

    let material_debug = material_debug_info(
        &library_debug,
        material_lookup.iter().filter_map(|(name, &index)| {
            Some((name.clone(), map_materials.get(index)?.material.clone()))
        }),
        &library,
        &textures,
    );

    let Some((minimum, maximum)) = bounds else {
        return Err("No bounded static world brushes could be reconstructed".into());
    };

    let spatial_chunks = groups
        .keys()
        .filter_map(|key| key.chunk)
        .collect::<BTreeSet<_>>()
        .len();
    let geometry_groups = groups.len();
    let collision_brush_count = source_collision_brushes.len();

    let mut vertices = Vec::new();
    let mut batches = Vec::new();
    for (key, geometry) in groups {
        if geometry.vertices.is_empty() {
            continue;
        }
        let start = u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;
        vertices.extend_from_slice(&geometry.vertices);
        let end = u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;
        let range = start..end;
        let material = &map_materials[key.material].material;

        if material.sky {
            append_sky_batches(
                &mut batches,
                material,
                None,
                None,
                false,
                range,
                None,
                &[],
                [0_u64; 4],
                [0.0; 4],
                false,
            );
            continue;
        }

        let stages = prepared_stages(material, false);
        if stages.is_empty() {
            continue;
        }
        if !material.explicit {
            batches.push(stage_batch(
                material,
                &stages[0],
                true,
                false,
                range,
                None,
                None,
                None,
                &[],
                [0_u64; 4],
                [0.0; 4],
                false,
            ));
        } else if stages.len() >= 2 && can_fold_jka_lightmap_pair(&stages[0], &stages[1]) {
            // A direct .map has no compiled `$lightmap` texture. Rendering the
            // authored Q3/JKA `$lightmap` + GL_DST_COLOR/GL_ZERO pair literally
            // therefore leaves no sensible receiver for the realtime q3map2
            // preview (and can collapse to white/black depending on fallback
            // bindings). Treat the diffuse half as the opaque source surface.
            // With simulation off this is the expected fullbright editor look;
            // with simulation on the normal opaque receiver is multiplied by the
            // q3map2-equivalent direct/ambient lighting. Decorative stages after
            // the canonical pair retain their authored order.
            let mut diffuse = stages[1].clone();
            diffuse.blend = None;
            diffuse.depth_write = true;
            diffuse.depth_equal = false;
            let mut folded = stage_batch(
                material,
                &diffuse,
                true,
                false,
                range.clone(),
                None,
                None,
                None,
                &[],
                [0_u64; 4],
                [0.0; 4],
                false,
            );
            // The `$lightmap` stage is folded away, so this batch is the receiver.
            folded.dlight_in_lightmap_stage = false;
            batches.push(folded);
            for stage in stages.iter().skip(2) {
                batches.push(stage_batch(
                    material,
                    stage,
                    false,
                    false,
                    range.clone(),
                    None,
                    None,
                    None,
                    &[],
                    [0_u64; 4],
                    [0.0; 4],
                    false,
                ));
            }
        } else {
            for (stage_index, stage) in stages.iter().enumerate() {
                batches.push(stage_batch(
                    material,
                    stage,
                    stage_index == 0,
                    false,
                    range.clone(),
                    None,
                    None,
                    None,
                    &[],
                    [0_u64; 4],
                    [0.0; 4],
                    false,
                ));
            }
        }
    }

    let (portal_anchors, camera_portal_count) = map_portal_surface_anchors(&document);
    let authored_portal_batch_count;
    if options.planar_reflections {
        assign_planar_reflection_planes(&mut batches, &vertices, options.planar_environment);
        authored_portal_batch_count = batches
            .iter()
            .filter(|batch| batch.planar_reflection)
            .count();
        retain_authored_planar_mirrors(&mut batches, &portal_anchors);
    } else {
        authored_portal_batch_count = 0;
        for batch in &mut batches {
            batch.planar_reflection = false;
            batch.planar_environment_candidate = false;
            batch.planar_plane = [0.0; 4];
        }
    }
    cache_reflection_decisions(&mut batches);
    let draw_batches = batches.len();
    log_reflection_cache(&path.display().to_string(), &batches);
    let planar_mirror_batch_count = batches
        .iter()
        .filter(|batch| batch.planar_reflection)
        .count();
    if authored_portal_batch_count != 0 && planar_mirror_batch_count == 0 {
        warnings.push(
            "portal shader surfaces found, but none has an untargeted misc_portal_surface within 64 units; no planar mirrors enabled"
                .into(),
        );
    }
    if options.planar_reflections && camera_portal_count != 0 {
        warnings.push(format!(
            "{camera_portal_count} targeted misc_portal_surface camera portal(s) detected; planar mirror pass leaves camera portals on their authored material"
        ));
    }

    warnings.extend(textures.warnings);
    warnings.sort();
    warnings.dedup();

    let center = (minimum + maximum) * 0.5;
    let movement = load_movement(assets, &mut warnings);
    let collision = match jka_movement::CollisionWorld::from_source_brushes(source_collision_brushes) {
        Ok(world) => Some(world),
        Err(error) => {
            warnings.push(format!("Source-map player collision unavailable: {error}"));
            None
        }
    };
    let mut spawns = map_spawn_points(&document);
    if spawns.is_empty() {
        let spawn_jka = DVec3::new(center.x, center.y, maximum.z + 64.0);
        spawns.push(SpawnPoint {
            position: render_position(spawn_jka.to_array().map(|value| value as f32)),
            yaw: 0.0,
            ..SpawnPoint::default()
        });
    }

    let lights = map_dynamic_lights(&document);
    let source_map_lighting = map_world_lighting(world);
    if source_map_lighting.ambient != [0.0; 3] || source_map_lighting.minlight != [0.0; 3] {
        println!(
            "Source .map q3map2 baseline: ambient={:.3},{:.3},{:.3} minlight={:.3},{:.3},{:.3}",
            source_map_lighting.ambient[0], source_map_lighting.ambient[1], source_map_lighting.ambient[2],
            source_map_lighting.minlight[0], source_map_lighting.minlight[1], source_map_lighting.minlight[2],
        );
    }
    let voxel_probe_gi = options
        .voxel_probe_gi
        .then(|| build_voxel_probe_gi(&vertices, &batches, &lights, sun))
        .flatten();

    let mark_surfaces = mark_surfaces_from_batches(&vertices, &batches);
    Ok(PreparedMap {
        authored_oceans: Vec::new(),
        vertices,
        legacy_dlight_triangle_surfaces: Vec::new(),
        legacy_dlight_surfaces: Vec::new(),
        batches,
        pvs_batches: Vec::new(),
        portal_draw_plan: PreparedPortalDrawPlan::default(),
        debug_volumes: Arc::default(),
        inline_vertices: Vec::new(),
        inline_batches: Vec::new(),
        inline_models: Vec::new(),
        movement,
        collision,
        mark_surfaces: Some(mark_surfaces),
        static_models: Arc::default(),
        physics_collision: crate::cgame::ragdoll::PhysicsMapMesh::default(),
        weather_occlusion: None,
        videos: textures.videos,
        textures: textures.images,
        footprint_mark_textures: [None; 2],
        footprint_mark_blend_modes: [0; 2],
        lightmaps: Vec::new(),
        deluxemaps: Vec::new(),
        static_bsp_ao_cache: None,
        steam_audio_acoustic_mesh: None,
        steam_audio_bake: None,
        steam_audio_bake_request: None,
        visibility: None,
        lights,
        source_map_lighting,
        sun,
        static_light_grid: None,
        voxel_probe_gi,
        reflection_probes: Vec::new(),
        grass_patches: Vec::new(),
        surface_sprite_effects: Vec::new(),
        global_fog: None,
        warnings,
        spawns,
        fx_runners: Vec::new(),
        brush_entities: Vec::new(),
        entity_graph: None,
        distance_cull,
        sky_portal: None,
        triangles,
        lightmap_pages: 0,
        source: path.to_path_buf(),
        material_debug,
        map_hash: 0,
        stage_keys: PrepStageKeys::default(),
        texture_hashes: Vec::new(),
        bsp_stats: None,
        load_timings: MapLoadTimings {
            geometry_ms: reconstruction_ms,
            worker_count: jobs.map_or(1, MapJobPool::worker_count),
            ..MapLoadTimings::default()
        },
        map_file_stats: Some(MapFileStats {
            entities: document.stats.entities,
            brushes: document.stats.brushes,
            world_brushes: world.brushes.len(),
            grouped_world_brushes,
            rendered_faces,
            utility_faces_skipped,
            skipped_entity_brushes,
            patches_skipped: document.stats.patches_skipped,
            degenerate_faces: document.stats.degenerate_faces,
            skipped_brushes,
            spatial_chunks,
            geometry_groups,
            draw_batches,
            collision_brushes: collision_brush_count,
            spatial_batching: options.source_spatial_batches,
            worker_count: jobs.map_or(1, MapJobPool::worker_count),
            reconstruction_ms,
        }),
    })
}

#[cfg(test)]
mod tests {
    /// The original ordering implementation, kept to prove the matrix version
    /// returns exactly the same sequence.
    fn order_pvs_pieces_reference<T>(
        pieces: std::collections::BTreeMap<(Vec<u64>, [u64; 4]), T>,
    ) -> Vec<((Vec<u64>, [u64; 4]), T)> {
        const MAX_ORDERED_PIECES: usize = 4096;
        let mut pieces = pieces.into_iter().collect::<Vec<_>>();
        let count = pieces.len();
        if count <= 2 || count > MAX_ORDERED_PIECES {
            return pieces;
        }
        let distance = |a: &[u64], b: &[u64]| -> u32 {
            let shared = a.len().min(b.len());
            let tail = a[shared..].iter().chain(&b[shared..]).map(|word| word.count_ones()).sum::<u32>();
            a[..shared].iter().zip(&b[..shared]).map(|(x, y)| (x ^ y).count_ones()).sum::<u32>() + tail
        };
        let mut current = (0..count)
            .min_by_key(|&index| pieces[index].0 .0.iter().map(|word| word.count_ones()).sum::<u32>())
            .unwrap_or(0);
        let mut visited = vec![false; count];
        let mut order = Vec::with_capacity(count);
        visited[current] = true;
        order.push(current);
        for _ in 1..count {
            let next = (0..count)
                .filter(|&index| !visited[index])
                .min_by_key(|&index| distance(&pieces[current].0 .0, &pieces[index].0 .0))
                .unwrap();
            visited[next] = true;
            order.push(next);
            current = next;
        }
        let signature = |piece: usize| pieces[piece].0 .0.as_slice();
        for _ in 0..8 {
            let mut improved = false;
            for i in 0..count.saturating_sub(2) {
                for j in i + 2..count {
                    let (a, b, c) = (order[i], order[i + 1], order[j]);
                    let d = order.get(j + 1).copied();
                    let before = distance(signature(a), signature(b))
                        + d.map_or(0, |d| distance(signature(c), signature(d)));
                    let after = distance(signature(a), signature(c))
                        + d.map_or(0, |d| distance(signature(b), signature(d)));
                    if after < before {
                        order[i + 1..=j].reverse();
                        improved = true;
                    }
                }
            }
            if !improved {
                break;
            }
        }
        let mut slots = pieces.drain(..).map(Some).collect::<Vec<_>>();
        order.into_iter().map(|index| slots[index].take().unwrap()).collect()
    }

    #[test]
    fn piece_ordering_matches_the_reference() {
        let mut state = 0x1234_5678_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for (count, words) in [(3, 1), (17, 2), (60, 3), (150, 5), (40, 0)] {
            let mut pieces = std::collections::BTreeMap::new();
            for index in 0..count {
                let signature = (0..words)
                    .map(|_| u64::from(next()) << 32 | u64::from(next() & next()))
                    .collect::<Vec<_>>();
                pieces.insert((signature, [index as u64, 0, 0, 0]), index);
            }
            let fast = super::order_pvs_pieces(pieces.clone());
            let reference = order_pvs_pieces_reference(pieces);
            assert_eq!(fast, reference, "{count} pieces x {words} words");
        }
    }

    /// A second preparation that only changes options the heavy stages do not
    /// read must reuse them and still return an identical world.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s"]
    fn reprepare_reuses_unchanged_stages_and_matches_a_fresh_build() {
        let base = std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE"));
        let name = std::env::var("JKA_TEST_MAP").unwrap_or_else(|_| "mp/duel3".into());

        let first_options = MapPrepareOptions { ocean: true, steam_audio: false, ..Default::default() };
        let started = Instant::now();
        let first = Arc::new(prepare_with_options(&base, None, &name, first_options).unwrap());
        let first_ms = started.elapsed().as_secs_f64() * 1000.0;

        // Ocean/physics do not feed the reusable stages, so everything is lent by `first`.
        let second_options = MapPrepareOptions { ocean: false, client_physics: true, ..first_options };
        let started = Instant::now();
        let second = prepare_with_seed(&base, None, &name, second_options, &first).unwrap();
        let second_ms = started.elapsed().as_secs_f64() * 1000.0;

        // Reflection topology changes the batches, so the PVS plan must be rebuilt.
        let third_options = MapPrepareOptions { planar_reflections: false, planar_environment: false, ..first_options };
        let third = prepare_with_seed(&base, None, &name, third_options, &first).unwrap();

        // Ground truth: the same options as `second`, built with no seed at all.
        let fresh = prepare_with_options(&base, None, &name, second_options).unwrap();

        println!(
            "first {first_ms:.0} ms, second {second_ms:.0} ms (plans {:.0}, gi {:.0}, bsp {:.0}), third plans {:.0} ms",
            second.load_timings.portal_plans_ms,
            second.load_timings.gi_ms,
            second.load_timings.bsp_parse_ms,
            third.load_timings.portal_plans_ms,
        );
        println!("first timings {:?}
second timings {:?}", first.load_timings, second.load_timings);
        assert!(second_ms < first_ms, "re-prepare should be faster than the first build");
        assert_eq!(second.vertices.len(), fresh.vertices.len(), "vertices");
        assert_eq!(second.batches.len(), fresh.batches.len(), "batches");
        assert_eq!(second.pvs_batches.len(), fresh.pvs_batches.len(), "pvs batches");
        assert_eq!(format!("{:?}", second.portal_draw_plan), format!("{:?}", fresh.portal_draw_plan), "plan");
        assert_eq!(format!("{:?}", second.voxel_probe_gi), format!("{:?}", fresh.voxel_probe_gi), "gi");
        assert_eq!(second.grass_patches.len(), fresh.grass_patches.len(), "grass");
        assert_eq!(second.textures.len(), fresh.textures.len(), "textures");
        assert!(
            second.textures.iter().zip(&fresh.textures).all(|(a, b)| a.rgba == b.rgba && a.label == b.label),
            "texture pixels"
        );
        assert_ne!(
            format!("{:?}", third.portal_draw_plan),
            format!("{:?}", first.portal_draw_plan),
            "a batch-topology change must not reuse the old plan"
        );
    }

    /// Full headless map preparation of a demo's map: inline BSP models must
    /// come out as self-consistent mover geometry sharing the world's
    /// material and lightmap resources.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/cheezyVsource.dm_26"]
    fn demo_map_prepares_inline_models_with_world_materials() {
        use jka_protocol::{demo::DemoReader, server::Decoder};
        let base = std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE"));
        let mut assets = AssetSearchPath::open(&base).unwrap();
        let bytes = assets.read("demos/cheezyVsource.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let map = loop {
            let record = reader.next_record().unwrap().expect("demo gamestate");
            decoder.parse_packet(record.sequence, &record.payload).unwrap();
            if let Some(map) = decoder.map_name() { break map; }
        };
        let prepared = prepare(&base, None, &map).unwrap();
        println!(
            "INLINE PREPARE {map}: models={:?} vertices={} batches={} worldVertices={}",
            prepared.inline_models.iter().map(|model| model.model).collect::<Vec<_>>(),
            prepared.inline_vertices.len(),
            prepared.inline_batches.len(),
            prepared.vertices.len(),
        );
        assert!(!prepared.inline_models.is_empty());
        let mut next_batch = 0;
        for model in &prepared.inline_models {
            assert!(model.model > 0, "model 0 is the static world");
            assert_eq!(model.batches.start, next_batch, "models own contiguous batches");
            next_batch = model.batches.end;
            assert!(model.vertices.end as usize <= prepared.inline_vertices.len());
            assert_eq!(model.vertices.len() % 3, 0);
            for batch in &prepared.inline_batches[model.batches.clone()] {
                assert!(batch.vertices.start >= model.vertices.start && batch.vertices.end <= model.vertices.end);
                assert!(batch.pvs_signature.is_empty(), "movers are not PVS-bound to compiled clusters");
                assert!(!batch.water_primary && !batch.planar_reflection);
                assert!(batch.lightmap.is_none_or(|page| page < prepared.lightmaps.len()));
                assert!(batch.texture.is_none_or(|texture| texture < prepared.textures.len()));
            }
        }
        assert_eq!(next_batch, prepared.inline_batches.len());
        assert!(prepared.inline_batches.iter().any(|batch| batch.lightmap.is_some()), "inline models lost their lightmaps");
    }

    use super::*;

    fn sun_test_grid(cells: Vec<ClassicLightGridCell>) -> ClassicEntityLightGrid {
        ClassicEntityLightGrid {
            origin: [0.0; 3],
            size: [64.0, 64.0, 128.0],
            bounds: [cells.len() as u32, 1, 1],
            external_hdr: false,
            cells,
            map_toward_sun: None,
        }
    }

    /// Coherence is the length of the blended probe direction: 1 when neighbouring
    /// probes agree, 0 when they point opposite ways and cancel.
    #[test]
    fn direction_coherence_reports_disagreeing_probes() {
        let cell = |direction: [f32; 3]| ClassicLightGridCell {
            ambient: [40.0; 3],
            directed: [100.0; 3],
            direction,
            sun_weight: 0.0,
            valid: true,
        };
        let agree = sun_test_grid(vec![cell([0.0, 1.0, 0.0]), cell([0.0, 1.0, 0.0])]);
        let oppose = sun_test_grid(vec![cell([0.0, 1.0, 0.0]), cell([0.0, -1.0, 0.0])]);
        // Halfway between the two probes (64-unit spacing in x).
        let midpoint = [32.0, 0.0, 0.0];
        let agreeing = agree.sample_classic_entity_light(midpoint).unwrap();
        let opposing = oppose.sample_classic_entity_light(midpoint).unwrap();
        assert!(agreeing.direction_coherence > 0.99, "{}", agreeing.direction_coherence);
        assert!(opposing.direction_coherence < 0.05, "{}", opposing.direction_coherence);
    }

    /// The estimate must separate a sunlit probe (blended direction on the sun,
    /// directed color equal to the sun color) from torch-lit and skylit probes,
    /// and the relight must move exactly the sun share to the new sun.
    #[test]
    fn entity_sun_weights_split_sunlit_from_torch_and_sky_probes() {
        let toward_sun = [0.0, 1.0, 0.0];
        let warm = [255.0, 200.0, 120.0];
        let cell = |directed: [f32; 3], direction: [f32; 3]| ClassicLightGridCell {
            ambient: [40.0; 3],
            directed,
            direction,
            sun_weight: 0.0,
            valid: true,
        };
        let mut grid = sun_test_grid(vec![
            cell(warm, toward_sun),                 // sunlit
            cell(warm, [1.0, 0.0, 0.0]),            // warm torch from the side
            cell([60.0, 90.0, 255.0], toward_sun),  // blue sky fill from above
            cell([0.0; 3], toward_sun),             // unlit
        ]);
        // Travel direction is the negated toward-sun direction.
        grid.estimate_sun_weights([0.0, -1.0, 0.0], [warm[0] / 255.0, warm[1] / 255.0, warm[2] / 255.0]);
        let weights = grid.cells.iter().map(|c| c.sun_weight).collect::<Vec<_>>();
        assert!(weights[0] > 0.99, "sunlit probe: {weights:?}");
        assert!(weights[1] < 0.01, "side torch: {weights:?}");
        assert!(weights[2] < 0.01, "blue skylight: {weights:?}");
        assert!(weights[3] == 0.0, "unlit probe: {weights:?}");
        assert_eq!(grid.map_toward_sun(), Some(toward_sun));

        let mut light = ClassicEntityLight {
            ambient: [40.0; 3],
            directed: [100.0, 80.0, 48.0],
            direction: toward_sun,
            baked_sun: [100.0, 80.0, 48.0],
            ..Default::default()
        };
        let new_toward = [1.0, 0.0, 0.0];
        light.relight_sun(toward_sun, new_toward, [0.5, 1.0, 2.0]);
        assert_eq!(light.directed, [0.0; 3], "all directed light was sun");
        assert_eq!(light.sun_directed, [50.0, 80.0, 96.0]);
        assert_eq!(light.sun_direction, new_toward);

        // A probe with no sun share is left exactly as sampled.
        let mut torch = ClassicEntityLight {
            directed: [30.0; 3],
            direction: [1.0, 0.0, 0.0],
            ..Default::default()
        };
        torch.relight_sun(toward_sun, new_toward, [9.0; 3]);
        assert_eq!(torch.directed, [30.0; 3]);
        assert_eq!(torch.sun_directed, [0.0; 3]);
    }

    fn legacy_lightmap_modulate_pair(enhanced: bool) -> [MaterialStage; 2] {
        let lightmap = MaterialStage {
            texture: StageTexture::Lightmap,
            enhancements: Default::default(),
            blend: None,
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Lightmap,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        };
        let mut diffuse = MaterialStage {
            texture: StageTexture::Image(7),
            enhancements: Default::default(),
            blend: Some(materials::BlendFunc {
                src: BlendFactor::DstColor,
                dst: BlendFactor::Zero,
            }),
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        };
        if enhanced {
            diffuse.enhancements.normal_texture = Some(8);
        }
        [lightmap, diffuse]
    }

    #[test]
    fn plain_jka_lightmap_then_dst_color_diffuse_collapses_to_modulation() {
        let [lightmap, diffuse] = legacy_lightmap_modulate_pair(false);
        assert!(can_fold_jka_lightmap_pair(&lightmap, &diffuse));
    }

    #[test]
    fn enhanced_lightmap_then_dst_color_diffuse_still_collapses_to_modulation() {
        let [lightmap, diffuse] = legacy_lightmap_modulate_pair(true);
        assert!(can_fold_jka_lightmap_pair(&lightmap, &diffuse));
    }

    #[test]
    fn nonidentity_diffuse_stage_does_not_use_plain_lightmap_collapse() {
        let [lightmap, mut diffuse] = legacy_lightmap_modulate_pair(false);
        diffuse.color = [0.5, 1.0, 1.0];
        assert!(!can_fold_jka_lightmap_pair(&lightmap, &diffuse));
    }

    #[test]
    fn vertex_lit_lightmap_stage_keeps_explicit_white_texture_source() {
        let diffuse = MaterialStage {
            texture: StageTexture::Image(7),
            enhancements: Default::default(),
            blend: None,
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: true,
            depth_equal: false,
        };
        let lightmap = MaterialStage {
            texture: StageTexture::Lightmap,
            enhancements: Default::default(),
            blend: Some(materials::BlendFunc {
                src: BlendFactor::DstColor,
                dst: BlendFactor::Zero,
            }),
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        };
        let material = SurfaceMaterial {
            stages: vec![diffuse, lightmap],
            explicit: true,
            ..Default::default()
        };

        let stages = prepared_stages(&material, true);
        assert!(matches!(stages[1].texture, StageTexture::White));
        assert_eq!(stages[1].rgb_gen, RgbGen::ExactVertex);

        let batch = stage_batch(
            &material,
            &stages[1],
            false,
            true,
            0..3,
            Some(226),
            None,
            None,
            &[],
            [0; 4],
            [0.0; 4],
            false,
        );
        assert!(batch.texture.is_none());
        assert!(!batch.texture_is_lightmap);
        assert!(batch.texture_is_white);
    }

    #[test]
    fn vertex_lit_pbr_override_consumes_bsp_vertex_lighting() {
        let mut base = MaterialStage {
            texture: StageTexture::Image(7),
            enhancements: Default::default(),
            blend: None,
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        };
        base.enhancements.normal_texture = Some(8);
        base.enhancements.roughness_texture = Some(9);

        let glow = MaterialStage {
            texture: StageTexture::Image(10),
            enhancements: Default::default(),
            blend: Some(materials::BlendFunc {
                src: BlendFactor::One,
                dst: BlendFactor::One,
            }),
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        };
        let material = SurfaceMaterial {
            stages: vec![base, glow],
            explicit: true,
            ..Default::default()
        };

        let stages = prepared_stages(&material, true);
        assert_eq!(stages[0].rgb_gen, RgbGen::ExactVertex);
        assert_eq!(stages[1].rgb_gen, RgbGen::Identity);
    }

    #[test]
    fn classic_explicit_vertex_lit_identity_stage_is_not_forced() {
        let base = MaterialStage {
            texture: StageTexture::Image(7),
            enhancements: Default::default(),
            blend: None,
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        };
        let material = SurfaceMaterial {
            stages: vec![base],
            explicit: true,
            ..Default::default()
        };

        let stages = prepared_stages(&material, true);
        assert_eq!(stages[0].rgb_gen, RgbGen::Identity);
    }

    #[test]
    fn guaranteed_alpha_discard_render_cull_keeps_surface_light_semantics() {
        let material = SurfaceMaterial {
            stages: vec![MaterialStage {
                texture: StageTexture::White,
                enhancements: Default::default(),
                blend: None,
                alpha_cutoff: 0.5,
                opacity: 0.0,
                color: [1.0; 3],
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Const,
                tc_gen: TcGen::Base,
                tc_mods: Vec::new(),
                depth_write: false,
                depth_equal: false,
            }],
            surface_light: Some(materials::SurfaceLight {
                value: 750.0,
                color: [1.0; 3],
                subdivide: 120.0,
                inferred_from_emissive: false,
            }),
            explicit: true,
            ..Default::default()
        };

        assert!(material.surface_light.is_some());
        assert!(material_render_is_guaranteed_discarded(&material, true));
    }

    #[test]
    fn guaranteed_alpha_discard_render_cull_requires_every_stage_to_discard() {
        let invisible = MaterialStage {
            texture: StageTexture::White,
            enhancements: Default::default(),
            blend: None,
            alpha_cutoff: 0.5,
            opacity: 0.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Const,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        };
        let visible = MaterialStage {
            opacity: 1.0,
            alpha_gen: AlphaGen::Identity,
            ..invisible.clone()
        };
        let material = SurfaceMaterial {
            stages: vec![invisible, visible],
            explicit: true,
            ..Default::default()
        };

        assert!(!material_render_is_guaranteed_discarded(&material, false));
    }

    #[test]
    fn coordinate_conversion_round_trips() {
        let jka = [1.0, 2.0, 3.0];
        assert_eq!(render_position(jka), [1.0, 3.0, -2.0]);
        assert_eq!(jka_position(render_position(jka)), jka);
    }

    #[test]
    fn distance_cull_matches_openjk_leading_float_parse() {
        assert_eq!(parse_distance_cull("24000"), Some(24000.0));
        assert_eq!(parse_distance_cull("24000r"), Some(24000.0));
        assert_eq!(parse_distance_cull(" 8500.5 trailing"), Some(8500.5));
        assert_eq!(parse_distance_cull("bogus"), None);
    }

    #[test]
    fn surface_sprite_twosided_material_uses_retail_one_sided_world_cull() {
        let material = SurfaceMaterial {
            cull: CullMode::None,
            surface_sprite_cull_quirk: true,
            ..Default::default()
        };
        assert_eq!(effective_world_cull(&material), CullMode::Back);

        let ordinary_twosided = SurfaceMaterial {
            cull: CullMode::None,
            ..Default::default()
        };
        assert_eq!(effective_world_cull(&ordinary_twosided), CullMode::None);
    }

    fn planar_test_vertex(position: [f32; 3]) -> GpuVertex {
        GpuVertex {
            position,
            uv: [0.0; 2],
            lightmap_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [1.0; 4],
            alpha_cutoff: 1.0,
        }
    }

    #[test]
    fn environment_planar_promotion_rejects_nonplanar_geometry() {
        let planar = [
            planar_test_vertex([0.0, 0.0, 0.0]),
            planar_test_vertex([64.0, 0.0, 0.0]),
            planar_test_vertex([0.0, 64.0, 0.0]),
            planar_test_vertex([64.0, 0.0, 0.0]),
            planar_test_vertex([64.0, 64.0, 0.0]),
            planar_test_vertex([0.0, 64.0, 0.0]),
        ];
        assert!(planar_batch_plane(&planar, true).is_some());

        let mut bent = planar;
        bent[4].position[2] = 8.0;
        assert!(planar_batch_plane(&bent, true).is_none());
        // Authored mirrors remain authoritative and can still derive a plane
        // from their first valid triangle, matching the original mirror path.
        assert!(planar_batch_plane(&bent, false).is_some());
    }

    #[test]
    fn q3map_surface_light_subdivision_preserves_area_scaled_energy() {
        let authored = materials::SurfaceLight {
            value: 300.0,
            color: [1.0, 1.0, 1.0],
            subdivide: 100.0,
            inferred_from_emissive: false,
        };
        let mut candidates = Vec::new();
        append_surface_light_triangle(
            &mut candidates,
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(200.0, 0.0, 0.0),
                Vec3::new(0.0, 200.0, 0.0),
            ],
            Vec3::Z,
            authored,
            false,
        );
        assert!(candidates.len() > 1);
        let total_intensity: f32 = candidates
            .iter()
            .map(|candidate| candidate.light.intensity)
            .sum();
        assert!((total_intensity - 2.0).abs() < 1e-4);
        assert!(candidates.iter().all(|candidate| {
            candidate.light.emitter_normal == Vec3::Z.to_array()
                && !candidate.light.emitter_two_sided
        }));
    }

    #[test]
    fn voxel_gi_propagation_does_not_cross_a_solid_wall() {
        let bounds = [7, 3, 3];
        let cell_count = bounds[0] as usize * bounds[1] as usize * bounds[2] as usize;
        let mut occupied = vec![false; cell_count];
        for z in 0..bounds[2] {
            for y in 0..bounds[1] {
                occupied[voxel_gi_index(3, y, z, bounds)] = true;
            }
        }
        let mut source = vec![Vec3::ZERO; cell_count];
        source[voxel_gi_index(1, 1, 1, bounds)] = Vec3::ONE;
        let field = voxel_gi_propagate(&source, &occupied, bounds);

        assert!(field[voxel_gi_index(2, 1, 1, bounds)].max_element() > 0.0);
        assert_eq!(field[voxel_gi_index(4, 1, 1, bounds)], Vec3::ZERO);
        assert_eq!(field[voxel_gi_index(5, 1, 1, bounds)], Vec3::ZERO);
    }

    #[test]
    fn q3map_sun_angles_round_trip_through_renderer_space() {
        for (yaw, pitch) in [(0.0, 0.0), (90.0, 45.0), (220.0, 30.0), (314.25595, 57.04725)] {
            let sun = DirectionalSun::from_q3_angles([1.0, 0.5, 0.25], 250.0, yaw, pitch);
            let [actual_yaw, actual_pitch] = sun.q3_angles();
            let yaw_error = (actual_yaw - yaw).abs().min(360.0 - (actual_yaw - yaw).abs());
            assert!(yaw_error < 1e-3, "yaw {yaw} became {actual_yaw}");
            assert!((actual_pitch - pitch).abs() < 1e-3, "pitch {pitch} became {actual_pitch}");
        }
    }

    #[test]
    fn q3map_sun_direction_converts_to_renderer_space() {
        let noon = render_sun(ShaderSun {
            color: [1.0, 0.0, 0.0],
            intensity: 100.0,
            azimuth: 0.0,
            elevation: 90.0,
            deviance: 0.0,
            samples: 0,
        });
        assert!(noon.direction[0].abs() < 1e-5);
        assert!((noon.direction[1] + 1.0).abs() < 1e-5);
        assert!(noon.direction[2].abs() < 1e-5);

        let east_horizon = render_sun(ShaderSun {
            color: [1.0, 0.0, 0.0],
            intensity: 100.0,
            azimuth: 0.0,
            elevation: 0.0,
            deviance: 0.0,
            samples: 0,
        });
        assert!((east_horizon.direction[0] + 1.0).abs() < 1e-5);
        assert!(east_horizon.direction[1].abs() < 1e-5);
        assert!(east_horizon.direction[2].abs() < 1e-5);
    }

    fn legacy_box_source(top_shader: &str) -> String {
        format!(
            r#"{{
"classname" "worldspawn"
{{
( 0 0 0 ) ( 0 64 0 ) ( 0 0 64 ) test/wall 0 0 0 1 1
( 64 0 0 ) ( 64 0 64 ) ( 64 64 0 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 0 0 64 ) ( 64 0 0 ) test/wall 0 0 0 1 1
( 0 64 0 ) ( 64 64 0 ) ( 0 64 64 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 64 0 0 ) ( 0 64 0 ) test/floor 0 0 0 1 1
( 0 0 64 ) ( 0 64 64 ) ( 64 0 64 ) {top_shader} 0 0 0 1 1
}}
}}
"#
        )
    }

    fn parsed_box() -> MapBrush {
        let document = jka_assets::map::parse(&legacy_box_source("test/ceiling")).unwrap();
        document.entities[0].brushes[0].clone()
    }

    #[test]
    fn six_sided_box_reconstructs_all_faces_and_corners() {
        let reconstructed = reconstruct_brush(&parsed_box()).unwrap();
        assert_eq!(reconstructed.vertices.len(), 8);
        assert_eq!(reconstructed.faces.len(), 6);
        assert!(reconstructed
            .faces
            .iter()
            .all(|face| face.vertices.len() == 4));
    }

    #[test]
    fn sloped_brush_face_reconstructs() {
        let mut brush = parsed_box();
        let length = 1.25_f64.sqrt();
        brush.faces[5].plane = MapPlane {
            normal: [-0.5 / length, 0.0, 1.0 / length],
            distance: 64.0 / length,
        };
        let reconstructed = reconstruct_brush(&brush).unwrap();
        assert_eq!(reconstructed.vertices.len(), 8);
        let top = reconstructed
            .faces
            .iter()
            .find(|face| face.face_index == 5)
            .unwrap();
        assert_eq!(top.vertices.len(), 4);
        assert!(top
            .vertices
            .iter()
            .any(|point| (point.z - 96.0).abs() < 1e-5));
    }

    #[test]
    fn reversed_plane_does_not_produce_a_valid_closed_brush() {
        let mut brush = parsed_box();
        brush.faces[0].plane.normal = [1.0, 0.0, 0.0];
        brush.faces[0].plane.distance = 0.0;
        assert!(reconstruct_brush(&brush).is_err());
    }

    #[test]
    fn reconstructed_face_winding_points_outward() {
        let brush = parsed_box();
        let reconstructed = reconstruct_brush(&brush).unwrap();
        for polygon in reconstructed.faces {
            let normal = map_vec(brush.faces[polygon.face_index].plane.normal);
            let a = polygon.vertices[0];
            let b = polygon.vertices[1];
            let c = polygon.vertices[2];
            assert!((b - a).cross(c - a).dot(normal) > 0.0);
        }
    }

    #[test]
    fn utility_face_is_hidden_but_still_clips_the_brush() {
        let document = jka_assets::map::parse(&legacy_box_source("system/caulk")).unwrap();
        let reconstructed = reconstruct_brush(&document.entities[0].brushes[0]).unwrap();
        assert_eq!(reconstructed.vertices.len(), 8);
        assert_eq!(reconstructed.faces.len(), 6);

        let root = std::env::temp_dir().join(format!("jka-map-scene-test-{}", std::process::id()));
        let prepared =
            prepare_map_document(&root, None, Path::new("fixture.map"), document).unwrap();
        let stats = prepared.map_file_stats.unwrap();
        assert_eq!(stats.world_brushes, 1);
        assert_eq!(stats.grouped_world_brushes, 0);
        assert_eq!(stats.rendered_faces, 5);
        assert_eq!(stats.collision_brushes, 1);
        assert_eq!(prepared.triangles, 10);
        assert!(prepared.collision.is_some(), "caulk must stay in gameplay collision");
    }

    #[test]
    fn shader_metadata_hides_custom_utility_surfaces() {
        let mut shader = Shader::default();
        shader.player_clip = true;
        assert!(shader_is_render_utility(&shader));
        shader.player_clip = false;
        shader.nonsolid = true;
        shader.translucent = true;
        assert!(!shader_is_render_utility(&shader), "visible glass must not be dropped just because it is nonsolid");
    }

    #[test]
    fn implicit_detail_face_keeps_default_solid_contents() {
        let face = MapFace {
            plane: MapPlane { normal: [0.0, 0.0, 1.0], distance: 64.0 },
            shader: "textures/rift/floor1b".into(),
            projection: TextureProjection::Legacy {
                shift: [0.0, 0.0],
                rotate: 0.0,
                scale: [0.5, 0.5],
            },
            // Radiant commonly stores CONTENTS_DETAIL here without repeating
            // the implicit SOLID default.
            trailing: vec![0x0800_0000_u32 as f64, 0.0, 0.0],
            plane_span: [0, 0],
            line: 1,
        };
        let (contents, _) = source_face_collision_flags(&face, None);
        let contents = contents as u32;
        assert_ne!(contents & SOURCE_CONTENTS_SOLID, 0);
        assert_ne!(contents & SOURCE_CONTENTS_OPAQUE, 0);
        assert_ne!(contents & 0x0800_0000, 0);

        // The same modifier must not erase solidity merely because the texture
        // has an explicit shader definition.
        let scripted = Shader::default();
        let (contents, _) = source_face_collision_flags(&face, Some(&scripted));
        let contents = contents as u32;
        assert_ne!(contents & SOURCE_CONTENTS_SOLID, 0);
        assert_ne!(contents & SOURCE_CONTENTS_OPAQUE, 0);
        assert_ne!(contents & 0x0800_0000, 0);

        // A real gameplay content class is different: playerclip must remain a
        // clip volume, not inherit the ordinary SOLID bit from the implicit base.
        let clip_face = MapFace {
            shader: "textures/common/playerclip".into(),
            trailing: vec![SOURCE_CONTENTS_PLAYERCLIP as f64, 0.0, 0.0],
            ..face
        };
        let (contents, _) = source_face_collision_flags(&clip_face, None);
        let contents = contents as u32;
        assert_eq!(contents & SOURCE_CONTENTS_SOLID, 0);
        assert_ne!(contents & SOURCE_CONTENTS_PLAYERCLIP, 0);
    }

    #[test]
    fn source_map_world_ambient_and_minlight_match_q3map2_keys() {
        let mut world = jka_assets::map::MapEntity::default();
        world.properties.insert("classname".into(), "worldspawn".into());
        world.properties.insert("_color".into(), "0.5 1 0.25".into());
        world.properties.insert("_ambient".into(), "51".into());
        world.properties.insert("_minlight".into(), "25.5".into());
        let lighting = map_world_lighting(&world);
        assert_eq!(lighting.ambient, [0.1, 0.2, 0.05]);
        assert_eq!(lighting.minlight, [0.05, 0.1, 0.025]);

        let mut alias = jka_assets::map::MapEntity::default();
        alias.properties.insert("ambient".into(), "25.5".into());
        let lighting = map_world_lighting(&alias);
        assert_eq!(lighting.ambient, [0.1, 0.1, 0.1]);
    }

    #[test]
    fn source_map_light_uses_authored_scale_color_and_linear_flag() {
        let mut entity = jka_assets::map::MapEntity::default();
        entity.properties.insert("classname".into(), "light".into());
        entity.properties.insert("origin".into(), "64 32 16".into());
        entity.properties.insert("_color".into(), "1 0 0".into());
        entity.properties.insert("_light".into(), "450".into());
        entity.properties.insert("scale".into(), "0.5".into());
        entity.properties.insert("spawnflags".into(), "1".into());
        entity.properties.insert("fade".into(), "2".into());

        let light = map_dynamic_light(&entity).expect("light entity");
        assert_eq!(light.color, [1.0, 0.0, 0.0]);
        assert_eq!(light.falloff, DynamicLightFalloff::Linear);
        assert!(light.surface_lighting);
        let photons = 450.0 * 0.5 * Q3MAP_POINT_SCALE;
        assert!((light.intensity - photons / Q3MAP_LIGHTMAP_BYTE_SCALE).abs() < 1e-3);
        assert!((light.radius - photons * Q3MAP_LINEAR_SCALE / 2.0).abs() < 1e-3);
        assert!(!light.angle_attenuation);
        assert_eq!(light.color, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn source_map_point_light_uses_q3map2_photon_scale_and_fast_envelope() {
        let mut entity = jka_assets::map::MapEntity::default();
        entity.properties.insert("classname".into(), "light".into());
        entity.properties.insert("origin".into(), "-24 128 400".into());
        entity.properties.insert("light".into(), "300".into());

        let light = map_dynamic_light(&entity).expect("light entity");
        let photons = 300.0 * Q3MAP_POINT_SCALE;
        assert_eq!(light.color, [1.0, 1.0, 1.0]);
        assert_eq!(light.falloff, DynamicLightFalloff::InverseSquare);
        assert!(light.angle_attenuation);
        assert_eq!(light.angle_scale, 0.0);
        assert_eq!(light.extra_distance, 0.0);
        assert!((light.intensity - photons / Q3MAP_LIGHTMAP_BYTE_SCALE).abs() < 1e-3);
        assert!((light.radius - photons.sqrt()).abs() < 1e-3);
    }

    #[test]
    fn spawn_selection_follows_jka_rules() {
        let at = |x: f32, initial: bool, no_humans: bool| SpawnPoint {
            position: [x, 0.0, 0.0],
            initial,
            no_humans,
            ..SpawnPoint::default()
        };
        let spawns = [at(0.0, false, false), at(100.0, true, false), at(200.0, false, false), at(300.0, false, false)];
        // The initial spot wins for the first spawn, whatever the roll.
        assert_eq!(select_spawn_index(&spawns, [0.0; 3], true, 0.9), Some(1));
        // Bot-only initial spots are skipped, falling back to the furthest half.
        let bot_only = [at(0.0, false, false), at(100.0, true, true), at(200.0, false, false), at(300.0, false, false)];
        assert_eq!(select_spawn_index(&bot_only, [0.0; 3], true, 0.0), Some(3));
        // Later spawns pick from the furthest half (300 and 200 from the origin).
        assert_eq!(select_spawn_index(&spawns, [0.0; 3], false, 0.0), Some(3));
        assert_eq!(select_spawn_index(&spawns, [0.0; 3], false, 0.99), Some(2));
        // Avoiding the far end flips the ranking.
        assert_eq!(select_spawn_index(&spawns, [300.0, 0.0, 0.0], false, 0.0), Some(0));
        assert_eq!(select_spawn_index(&[], [0.0; 3], true, 0.0), None);
        assert_eq!(select_spawn_index(&spawns[..1], [0.0; 3], false, 0.5), Some(0));
    }

    #[test]
    fn source_map_spawn_yaw_uses_radians() {
        let source = format!(
            "{}\n{{\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"16 32 48\"\n\"angle\" \"90\"\n}}\n",
            legacy_box_source("test/ceiling")
        );
        let document = jka_assets::map::parse(&source).unwrap();
        let spawns = map_spawn_points(&document);
        assert_eq!(spawns.len(), 1);
        assert!((spawns[0].yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn source_map_can_load_from_loose_base_maps_directory() {
        let root = std::env::temp_dir().join(format!("jka-map-vfs-test-{}", std::process::id()));
        let base = root.join("base");
        std::fs::create_dir_all(base.join("maps")).unwrap();
        std::fs::write(
            base.join("maps/loose_source.map"),
            legacy_box_source("test/ceiling"),
        )
        .unwrap();

        let prepared = prepare_source(&base, None, &MapSource::Map("loose_source".into())).unwrap();
        assert_eq!(prepared.triangles, 12);
        assert!(prepared.source.ends_with("base/maps/loose_source.map"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nonworld_entity_brushes_are_skipped_and_reported() {
        let mut source = legacy_box_source("test/ceiling");
        source.push_str(
            r#"
{
"classname" "func_static"
{
( 128 0 0 ) ( 128 64 0 ) ( 128 0 64 ) test/wall 0 0 0 1 1
( 192 0 0 ) ( 192 0 64 ) ( 192 64 0 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 128 0 64 ) ( 192 0 0 ) test/wall 0 0 0 1 1
( 128 64 0 ) ( 192 64 0 ) ( 128 64 64 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 192 0 0 ) ( 128 64 0 ) test/floor 0 0 0 1 1
( 128 0 64 ) ( 128 64 64 ) ( 192 0 64 ) test/ceiling 0 0 0 1 1
}
}
"#,
        );
        let document = jka_assets::map::parse(&source).unwrap();
        let root = std::env::temp_dir().join(format!("jka-map-scene-test-{}", std::process::id()));
        let prepared =
            prepare_map_document(&root, None, Path::new("fixture.map"), document).unwrap();
        let stats = prepared.map_file_stats.unwrap();
        assert_eq!(stats.world_brushes, 1);
        assert_eq!(stats.grouped_world_brushes, 0);
        assert_eq!(stats.skipped_entity_brushes, 1);
        assert_eq!(prepared.triangles, 12);
    }

    #[test]
    fn func_group_brushes_are_folded_into_static_world() {
        let mut source = legacy_box_source("test/ceiling");
        source.push_str(
            r#"
{
"classname" "func_group"
{
( 128 0 0 ) ( 128 64 0 ) ( 128 0 64 ) test/wall 0 0 0 1 1
( 192 0 0 ) ( 192 0 64 ) ( 192 64 0 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 128 0 64 ) ( 192 0 0 ) test/wall 0 0 0 1 1
( 128 64 0 ) ( 192 64 0 ) ( 128 64 64 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 192 0 0 ) ( 128 64 0 ) test/floor 0 0 0 1 1
( 128 0 64 ) ( 128 64 64 ) ( 192 0 64 ) test/ceiling 0 0 0 1 1
}
}
"#,
        );
        let document = jka_assets::map::parse(&source).unwrap();
        let root = std::env::temp_dir().join(format!("jka-map-scene-test-{}", std::process::id()));
        let prepared =
            prepare_map_document(&root, None, Path::new("fixture.map"), document).unwrap();
        let stats = prepared.map_file_stats.unwrap();
        assert_eq!(stats.world_brushes, 1);
        assert_eq!(stats.grouped_world_brushes, 1);
        assert_eq!(stats.skipped_entity_brushes, 0);
        assert_eq!(prepared.triangles, 24);
    }

    #[test]
    fn multiple_world_brushes_are_counted() {
        let first = legacy_box_source("test/ceiling");
        let second_brush = r#"{
( 128 0 0 ) ( 128 64 0 ) ( 128 0 64 ) test/wall 0 0 0 1 1
( 192 0 0 ) ( 192 0 64 ) ( 192 64 0 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 128 0 64 ) ( 192 0 0 ) test/wall 0 0 0 1 1
( 128 64 0 ) ( 192 64 0 ) ( 128 64 64 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 192 0 0 ) ( 128 64 0 ) test/floor 0 0 0 1 1
( 128 0 64 ) ( 128 64 64 ) ( 192 0 64 ) test/ceiling 0 0 0 1 1
}"#;
        let source = first.replacen("\n}\n}", &format!("\n}}\n{second_brush}\n}}"), 1);
        let document = jka_assets::map::parse(&source).unwrap();
        assert_eq!(document.entities[0].brushes.len(), 2);
    }

    #[test]
    fn map_arguments_choose_source_by_extension() {
        assert!(matches!(
            MapSource::from_map_argument("mp/ffa3").unwrap(),
            MapSource::Bsp(name) if name == "mp/ffa3"
        ));
        assert!(matches!(
            MapSource::from_map_argument("mp/ffa3.bsp").unwrap(),
            MapSource::Bsp(name) if name == "mp/ffa3"
        ));
        assert!(matches!(
            MapSource::from_map_argument("maps/mp/ffa3.map").unwrap(),
            MapSource::Map(name) if name == "mp/ffa3"
        ));
        assert!(matches!(
            MapSource::from_map_argument("mp\\ffa3.map").unwrap(),
            MapSource::Map(name) if name == "mp/ffa3"
        ));
        assert!(MapSource::from_map_argument("mp/ffa3.pk3").is_err());
        assert!(MapSource::from_map_argument("../ffa3.map").is_err());
    }

    #[test]
    fn map_source_preflight_checks_asset_existence() {
        let root = std::env::temp_dir().join(format!(
            "jka-map-preflight-test-{}",
            std::process::id()
        ));
        let base = root.join("base");
        std::fs::create_dir_all(base.join("maps")).unwrap();
        std::fs::write(base.join("maps/present.bsp"), b"not parsed during preflight").unwrap();

        assert!(verify_map_source_exists(
            &base,
            None,
            &MapSource::Bsp("present".into())
        )
        .is_ok());
        assert!(verify_map_source_exists(
            &base,
            None,
            &MapSource::Bsp("missing".into())
        )
        .is_err());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dynamic_light_entities_are_bounded_and_finite() {
        let light =
            dynamic_light_from_values("light", Some("1 2 3"), Some("1.2 -0.5 0.4"), Some("600"))
                .unwrap();
        assert_eq!(light.position, render_position([1.0, 2.0, 3.0]));
        assert_eq!(light.color, [1.2, 0.0, 0.4]);
        assert_eq!(light.radius, 600.0);
        assert_eq!(light.intensity, 2.0);
        assert!(dynamic_light_from_values("light", Some("NaN 0 0"), None, None).is_none());
        assert!(
            dynamic_light_from_values("info_player_deathmatch", Some("0 0 0"), None, None)
                .is_none()
        );
    }
}
