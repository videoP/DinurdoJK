//! World types.
use crate::renderer::{
    legacy_dlight_receives, legacy_fog_pass_jobs, material_specialization_jobs, scene, Arc, Auto4CollapseCache, BTreeMap,
    BTreeSet, CullMode, DirectionalSun, DrawBatch, GpuImage, GpuVertex, GrassMapGpu, Hash,
    InlineModelInstance, InspectorTextureMeta, InspectorVertex, LocalShadowResources, Mat4,
    MaterialFragmentKey, PipelineKey, PreparedPortalPlanBatchRef, PvsMode, RtAlphaMaskSource, SnowShellGpu,
    StaticAoWorldSource, SurfaceSpriteEffectGpu, TextureData, Vec3, WorldShaderVariantKey,
};

/// Map-load decision for a connected coplanar water footprint. The exact same
/// sorted footprints and keys drive both ocean ownership and Trace diagnostics.
/// Retaining only candidates avoids duplicating the BSP or water geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::renderer) struct OceanPromotionDecision {
    /// One-based rank after sorting all connected footprints by triangle area.
    pub(in crate::renderer) rank: usize,
    pub(in crate::renderer) total: usize,
    pub(in crate::renderer) area: f32,
    pub(in crate::renderer) faces: usize,
    /// A missing slot means this valid candidate lost the eight-slot selection.
    pub(in crate::renderer) slot: Option<u8>,
}

#[derive(Clone)]
pub(in crate::renderer) struct FacePlaneRun {
    /// Index offsets from the current batch's indexed_range.start, not absolute
    /// indices. AUTO-4 can relocate collapsed recipes while preserving this list.
    pub(in crate::renderer) offsets: std::ops::Range<u32>,
    /// Zero plane keeps this run, e.g. tri-soup, patch or unknown.
    pub(in crate::renderer) plane: [f32; 4],
    /// Render-space bounds of this run's geometry. Unknown bounds span all of
    /// space so no volume test can ever reject the run.
    pub(in crate::renderer) bounds_min: [f32; 3],
    pub(in crate::renderer) bounds_max: [f32; 3],
}

impl FacePlaneRun {
    pub(in crate::renderer) const UNKNOWN_BOUNDS: ([f32; 3], [f32; 3]) =
        ([f32::NEG_INFINITY; 3], [f32::INFINITY; 3]);
}

#[derive(Clone)]
pub(in crate::renderer) struct LegacyDlightRun {
    pub(in crate::renderer) surface_id: u32,
    pub(in crate::renderer) indexed_range: std::ops::Range<u32>,
}

/// A slice of a shared uniform buffer. Every world batch owns two small
/// uniform blocks; as separate buffers those are ~30,000 allocations on a big
/// map, and DX12 pads each to a 64 KiB resource (out of memory). Batches share
/// `UniformArena` chunks and bind their slice by offset instead.
#[derive(Clone)]
pub(in crate::renderer) struct UniformSlot {
    pub(in crate::renderer) buffer: wgpu::Buffer,
    pub(in crate::renderer) offset: u64,
    pub(in crate::renderer) size: u64,
}

impl UniformSlot {
    pub(in crate::renderer) fn binding(&self) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.buffer,
            offset: self.offset,
            size: wgpu::BufferSize::new(self.size),
        })
    }
}

/// Hands out fixed-size, offset-aligned `UniformSlot`s from chunked buffers.
pub(in crate::renderer) struct UniformArena {
    pub(in crate::renderer) label: &'static str,
    pub(in crate::renderer) stride: u64,
    pub(in crate::renderer) chunk: Option<(wgpu::Buffer, u64)>,
    pub(in crate::renderer) chunk_bytes: u64,
}

impl UniformArena {
    pub(in crate::renderer) const CHUNK_BYTES: u64 = 4 * 1024 * 1024;

    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        label: &'static str,
        uniform_size: u64,
    ) -> Self {
        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(1);
        Self {
            label,
            stride: uniform_size.div_ceil(alignment) * alignment,
            chunk: None,
            chunk_bytes: Self::CHUNK_BYTES.max(uniform_size.div_ceil(alignment) * alignment),
        }
    }

    pub(in crate::renderer) fn push(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        contents: &[u8],
    ) -> UniformSlot {
        let needs_chunk = self
            .chunk
            .as_ref()
            .is_none_or(|(_, used)| used + self.stride > self.chunk_bytes);
        if needs_chunk {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.chunk_bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.chunk = Some((buffer, 0));
        }
        let (buffer, used) = self.chunk.as_mut().expect("chunk was just created");
        let offset = *used;
        *used += self.stride;
        queue.write_buffer(buffer, offset, contents);
        UniformSlot {
            buffer: buffer.clone(),
            offset,
            size: contents.len() as u64,
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::renderer) struct RuntimeOceanClipmap {
    pub(in crate::renderer) slot: u8,
    pub(in crate::renderer) quality: usize,
    pub(in crate::renderer) layout: crate::ocean::PromotedClipmapLayout,
    pub(in crate::renderer) vertex_base: u32,
    pub(in crate::renderer) reserved_range: std::ops::Range<u32>,
    pub(in crate::renderer) active_range: std::ops::Range<u32>,
    pub(in crate::renderer) current_half_cells: i32,
    pub(in crate::renderer) current_minimum_level: u32,
    /// Levels drawn past which water absorption hides the sheet (u32::MAX = all).
    pub(in crate::renderer) current_level_limit: u32,
    /// Final per-surface centre after global and outer-ring snapping. NaN
    /// forces the first rendered frame to install footprint-pruned indices.
    pub(in crate::renderer) current_centre: [f32; 2],
    /// Exact BSP cluster whose visible water pieces shaped `active_range`.
    /// `None` is the conservative PVS-off/outside-the-tree footprint.
    pub(in crate::renderer) current_pvs_cluster: Option<usize>,
    pub(in crate::renderer) current_sky_extend: bool,
}

/// Prebuilt texture bindings for a TaystJK animMap stage. These are the same
/// pipeline/layout as an ordinary stage; only binding 0's image differs.
#[derive(Clone)]
pub(in crate::renderer) struct WorldAnimationFrame {
    pub(in crate::renderer) full: wgpu::BindGroup,
    pub(in crate::renderer) fast: wgpu::BindGroup,
    pub(in crate::renderer) glow: Option<wgpu::BindGroup>,
}

#[derive(Clone)]
pub(in crate::renderer) struct WorldBatchAnimation {
    pub(in crate::renderer) frames: Vec<WorldAnimationFrame>,
    pub(in crate::renderer) selected: usize,
}

#[derive(Clone)]
pub(in crate::renderer) struct WorldBatch {
    pub(in crate::renderer) full_indexed_range: std::ops::Range<u32>,
    pub(in crate::renderer) patch_geometry: Option<usize>,
    pub(in crate::renderer) patch_suppressed: bool,
    pub(in crate::renderer) source: DrawBatch,
    pub(in crate::renderer) indexed_range: std::ops::Range<u32>,
    /// Contiguous authored-BSP surface runs inside `indexed_range`. Legacy
    /// projected lights use these to redraw only triangles belonging to a
    /// surface whose OpenJK-style dlight mask is non-zero. Inline BSP models
    /// intentionally leave this empty and use their existing all-light fallback.
    pub(in crate::renderer) legacy_dlight_runs: Vec<LegacyDlightRun>,
    /// Shared: AUTO variants of one geometry reference the same merged runs.
    pub(in crate::renderer) face_cull_runs: Arc<[FacePlaneRun]>,
    /// Only nonzero for a packed run of coincident, single-sided BSP planar faces.
    /// ZERO means mixed/unknown => never reject the batch on its face plane.
    pub(in crate::renderer) face_cull_plane: [f32; 4],
    /// True for a CG_Mover inline BSP model rather than static world geometry.
    /// Used by grouped mover submission and diagnostics.
    pub(in crate::renderer) is_inline_entity: bool,
    /// Stable index in WorldGpu.inline_models, or u32::MAX for static geometry.
    /// Unlike model handles, this identifies each distinct render instance,
    /// including multiple movers sharing the same BSP model handle.
    pub(in crate::renderer) inline_model_slot: u32,
    /// Clipmap index ranges for this promoted water surface, low then high.
    pub(in crate::renderer) ocean_clipmap: Option<[std::ops::Range<u32>; 2]>,
    /// Which promoted surface `ocean_clipmap` belongs to. Every face merged into
    /// one surface shares that clipmap, so a pass draws it once, not per face.
    pub(in crate::renderer) ocean_clipmap_id: u8,
    /// An uploaded ocean replaces this stage, including its sibling passes.
    pub(in crate::renderer) ocean_replaced: bool,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
    /// Fixed at upload from the material uniform (r_fragmentShaderMode 1).
    pub(in crate::renderer) fragment_key: MaterialFragmentKey,
    /// Index of this batch's `(source.pipeline, fragment_key)` in
    /// `WorldGpu::pso_slot_keys`, so `base_pipeline` reads a Vec instead of
    /// searching maps per batch per frame. `u32::MAX` = unassigned; anything
    /// that changes either key must reset it (`WorldGpu::refresh_pso_slots`).
    pub(in crate::renderer) pso_slot: u32,
    pub(in crate::renderer) animation: Option<WorldBatchAnimation>,
    /// Same authored stage resources as `bind_group`, but with OpenJK's
    /// Dynamic Glow fog override: glow stages fog toward black during replay.
    /// Only allocated for exact `.shader` stages marked `glow`.
    pub(in crate::renderer) authored_glow_bind_group: Option<wgpu::BindGroup>,
    /// Diffuse-stage appearance used only when the ocean draws its replacement.
    pub(in crate::renderer) ocean_surface_bind_group: Option<wgpu::BindGroup>,
    pub(in crate::renderer) _ocean_material_buffer: Option<UniformSlot>,
    pub(in crate::renderer) fast_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) _material_buffer: UniformSlot,
    pub(in crate::renderer) additive_uniforms: [UniformSlot; 2],
    pub(in crate::renderer) _fog_buffer: UniformSlot,
    pub(in crate::renderer) _authored_glow_fog_buffer: Option<UniformSlot>,
    pub(in crate::renderer) cull_index: u32,
    pub(in crate::renderer) compact_group: Option<u32>,
    /// Draw-state class shared by mover (inline model) batches, or `u32::MAX`.
    /// Movers change every frame, so instead of load-time compaction groups the
    /// visible members of one class are packed into a single multi-draw.
    pub(in crate::renderer) inline_group: u32,
    pub(in crate::renderer) bounds_min: [f32; 3],
    pub(in crate::renderer) bounds_max: [f32; 3],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::renderer) enum WorldBatchSet {
    Coarse,
    Full,
}

pub(in crate::renderer) struct WorldDrawGroup {
    pub(in crate::renderer) representative_set: WorldBatchSet,
    pub(in crate::renderer) representative_index: usize,
    pub(in crate::renderer) output_base: u32,
    pub(in crate::renderer) max_count: u32,
}

#[derive(Clone, Debug, Hash)]
pub(in crate::renderer) struct WorldPipelineCompilePlan {
    pub(in crate::renderer) base_keys: Vec<PipelineKey>,
    pub(in crate::renderer) legacy_dlight_keys: Vec<PipelineKey>,
    pub(in crate::renderer) fog_jobs: Vec<(PipelineKey, bool)>,
    /// Every eligible (pipeline, material key) pair; see `material_specialization_eligible`.
    pub(in crate::renderer) material_jobs: Vec<(PipelineKey, MaterialFragmentKey)>,
    pub(in crate::renderer) has_planar_reflectors: bool,
}

impl WorldPipelineCompilePlan {
    pub(in crate::renderer) fn from_world(world: &WorldGpu) -> Self {
        let base_keys = world
            .coarse_batches
            .iter()
            .map(|batch| batch.source.pipeline)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let legacy_dlight_keys = world
            .coarse_batches
            .iter()
            .filter(|batch| legacy_dlight_receives(&batch.source))
            .map(|batch| batch.source.pipeline)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        Self {
            base_keys,
            legacy_dlight_keys,
            fog_jobs: legacy_fog_pass_jobs(&world.coarse_batches),
            material_jobs: material_specialization_jobs(world.all_world_batches()),
            has_planar_reflectors: !world.planar_reflectors.is_empty(),
        }
    }
}

#[derive(Clone)]
pub(in crate::renderer) struct WorldPipelineVariant {
    pub(in crate::renderer) pipelines: BTreeMap<PipelineKey, wgpu::RenderPipeline>,
    /// Compiled asynchronously only on first use of r_oceanSkyExtend 2.
    pub(in crate::renderer) ocean_far_pipeline: Option<wgpu::RenderPipeline>,
    /// Submerged Hybrid: Blue Water's clipmap through the Godot ocean material
    /// (`vs_ocean_native`), and its r_showtris overlay. Compiled on first use.
    pub(in crate::renderer) ocean_native_mesh_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) ocean_native_mesh_wireframe_pipeline: Option<wgpu::RenderPipeline>,
    /// OpenJK-style projected-light redraw. The normal world material pipelines
    /// compile with Legacy dlights disabled; only touched BSP triangles enter
    /// these additive depth-equal pipelines.
    pub(in crate::renderer) legacy_dlight_pipelines: BTreeMap<PipelineKey, wgpu::RenderPipeline>,
    // OpenJK r_drawfog 1, and r_drawfog 2 local brush fog, redraw the final
    // material result with a dedicated SRC_ALPHA/ONE_MINUS_SRC_ALPHA fog pass.
    // Keyed by the authored material pipeline so the draw loop can switch to
    // the matching cull/depth state without rebuilding a key at runtime.
    pub(in crate::renderer) fog_pass_pipelines: BTreeMap<(PipelineKey, bool), wgpu::RenderPipeline>,
    pub(in crate::renderer) reflection_pipelines: BTreeMap<PipelineKey, wgpu::RenderPipeline>,
    pub(in crate::renderer) reflection_fog_pass_pipelines:
        BTreeMap<(PipelineKey, bool), wgpu::RenderPipeline>,
    /// r_fragmentShaderMode 1: base-pass PSOs per material, keyed by the batch's
    /// full key; the value carries the narrowed key the PSO was built with, which
    /// identifies the program for redundant-bind checks. Empty in mode 0.
    pub(in crate::renderer) specialized_pipelines: super::pipelines::SpecializedWorldPipelines,
    /// `base_pipeline` resolved once per `WorldGpu::pso_slot_keys` entry.
    /// Empty until `WorldGpu::refresh_pso_slots` runs for this variant.
    pub(in crate::renderer) slot_pipelines: Vec<Option<(WorldPipelineBinding, wgpu::RenderPipeline)>>,
}

/// Which base-pass program is bound: the general one (`None`) or a specialized one.
pub(in crate::renderer) type WorldPipelineBinding = (PipelineKey, Option<MaterialFragmentKey>);

impl WorldPipelineVariant {
    /// Base-pass PSO for a world batch: its specialized program when this variant
    /// has one, otherwise the general program.
    pub(in crate::renderer) fn base_pipeline(
        &self,
        batch: &WorldBatch,
    ) -> (WorldPipelineBinding, &wgpu::RenderPipeline) {
        if let Some((binding, pipeline)) = self.slot_pipelines.get(batch.pso_slot as usize).and_then(Option::as_ref) {
            debug_assert_eq!(binding.0, batch.source.pipeline, "stale world PSO slot");
            return (*binding, pipeline);
        }
        self.lookup_base_pipeline(batch.source.pipeline, batch.fragment_key)
    }

    fn lookup_base_pipeline(
        &self,
        key: PipelineKey,
        fragment_key: MaterialFragmentKey,
    ) -> (WorldPipelineBinding, &wgpu::RenderPipeline) {
        if !self.specialized_pipelines.is_empty() {
            if let Some((narrowed, pipeline)) =
                self.specialized_pipelines.get(&(key, fragment_key))
            {
                return ((key, Some(*narrowed)), pipeline);
            }
        }
        ((key, None), &self.pipelines[&key])
    }

    /// Resolve every slot key once. A key this variant has no general PSO for
    /// stays `None`, and those batches take the map lookup exactly as before.
    pub(in crate::renderer) fn build_slot_pipelines(&mut self, keys: &[(PipelineKey, MaterialFragmentKey)]) {
        let resolved: Vec<_> = keys.iter()
            .map(|&(key, fragment_key)| self.pipelines.contains_key(&key).then(|| {
                let (binding, pipeline) = self.lookup_base_pipeline(key, fragment_key);
                (binding, pipeline.clone())
            }))
            .collect();
        self.slot_pipelines = resolved;
    }
}

pub(in crate::renderer) struct StaticLightGridGpu {
    pub(in crate::renderer) enabled: bool,
    pub(in crate::renderer) external_hdr: bool,
    pub(in crate::renderer) _direction_texture: wgpu::Texture,
    pub(in crate::renderer) direction_view: wgpu::TextureView,
    pub(in crate::renderer) _lighting_texture: wgpu::Texture,
    pub(in crate::renderer) lighting_view: wgpu::TextureView,
    pub(in crate::renderer) _irradiance_volume_texture: wgpu::Texture,
    pub(in crate::renderer) irradiance_volume_view: wgpu::TextureView,
    pub(in crate::renderer) sampler: wgpu::Sampler,
    pub(in crate::renderer) uniform_buffer: wgpu::Buffer,
    pub(in crate::renderer) irradiance_volume_uniform_buffer: wgpu::Buffer,
}

pub(in crate::renderer) struct VoxelProbeGiGpu {
    pub(in crate::renderer) enabled: bool,
    pub(in crate::renderer) _point_texture: wgpu::Texture,
    pub(in crate::renderer) point_view: wgpu::TextureView,
    pub(in crate::renderer) _area_texture: wgpu::Texture,
    pub(in crate::renderer) area_view: wgpu::TextureView,
    pub(in crate::renderer) uniform_buffer: wgpu::Buffer,
}

pub(in crate::renderer) struct GpuReflectionProbe {
    pub(in crate::renderer) _texture: wgpu::Texture,
    pub(in crate::renderer) view: wgpu::TextureView,
}

pub(in crate::renderer) struct ReflectionProbeGpuSet {
    pub(in crate::renderer) probes: Vec<GpuReflectionProbe>,
    pub(in crate::renderer) sampler: wgpu::Sampler,
    pub(in crate::renderer) source_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::renderer) enum PlanarReflectorKind {
    Authored,
    Environment,
    Ocean,
}

#[derive(Clone, Copy)]
pub(in crate::renderer) struct PlanarReflector {
    pub(in crate::renderer) kind: PlanarReflectorKind,
    pub(in crate::renderer) plane: [f32; 4],
    pub(in crate::renderer) pvs_origin: [f32; 3],
    pub(in crate::renderer) cull: CullMode,
    pub(in crate::renderer) bounds_min: [f32; 3],
    pub(in crate::renderer) bounds_max: [f32; 3],
    pub(in crate::renderer) area: f32,
    pub(in crate::renderer) coarse_batch_index: usize,
}

#[derive(Clone, Copy)]
pub(in crate::renderer) struct PlanarReflectionView {
    pub(in crate::renderer) kind: PlanarReflectorKind,
    pub(in crate::renderer) plane: [f32; 4],
    pub(in crate::renderer) view_proj: Mat4,
    pub(in crate::renderer) camera_position: Vec3,
    pub(in crate::renderer) camera_forward: Vec3,
    pub(in crate::renderer) cluster: Option<usize>,
    pub(in crate::renderer) coarse_batch_index: usize,
    /// `view_proj` narrowed to the screen rectangle covered by the visible
    /// coplanar reflectors. The reflected camera shares the main camera's clip
    /// space, so geometry outside this rectangle can never be sampled.
    pub(in crate::renderer) cull_view_proj: Mat4,
}

pub(in crate::renderer) struct PlanarReflectionResources {
    pub(in crate::renderer) _color: wgpu::Texture,
    pub(in crate::renderer) color_views: Vec<wgpu::TextureView>,
    pub(in crate::renderer) _color_array_view: wgpu::TextureView,
    pub(in crate::renderer) _fallback_color: wgpu::Texture,
    pub(in crate::renderer) _depth: wgpu::Texture,
    pub(in crate::renderer) depth_view: wgpu::TextureView,
    pub(in crate::renderer) _sampler: wgpu::Sampler,
    pub(in crate::renderer) uniform_buffer: wgpu::Buffer,
    pub(in crate::renderer) _reflection_pass_uniform_buffer: wgpu::Buffer,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
    pub(in crate::renderer) reflection_pass_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) _width: u32,
    pub(in crate::renderer) _height: u32,
    pub(in crate::renderer) active: bool,
}

/// Runtime state of one inline BSP model whose vertices live in a private
/// (never de-duplicated) region of the world vertex buffer.
pub(in crate::renderer) struct InlineModelGpu {
    /// Cached for the current view cluster/areamask; recompute for a changed pose.
    pub(in crate::renderer) pvs_visible: Option<bool>,
    pub(in crate::renderer) pvs_bounds_dirty: bool,
    pub(in crate::renderer) pvs_area_rejected: bool,
    pub(in crate::renderer) model: u32,
    pub(in crate::renderer) gpu_vertex_start: u32,
    /// Compiled render-space vertices, in GPU order.
    pub(in crate::renderer) base: Vec<GpuVertex>,
    /// Local indices for opaque RT casting. Kept CPU-side only until the user
    /// selects RT Shadows, at which point an immutable mover BLAS is built.
    pub(in crate::renderer) rt_indices: Vec<u32>,
    /// Authored alpha-tested stages that have no opaque depth-writing sibling.
    /// These are added to the same mover BLAS as non-opaque geometries.
    pub(in crate::renderer) rt_mask_sources: Vec<RtAlphaMaskSource>,
    pub(in crate::renderer) raster_glass_local: Vec<crate::renderer::lighting::glass_transmission::RasterGlassTriangle>,
    pub(in crate::renderer) coarse_batches: Vec<usize>,
    pub(in crate::renderer) full_batches: Vec<usize>,
    /// Last uploaded pose: `None` = never updated (buffer holds the compiled
    /// pose), `Some(None)` = hidden (not in the snapshot). Portal membership
    /// is carried in each submitted pose, never attached to a BSP model handle.
    pub(in crate::renderer) uploaded: Option<Option<InlineModelInstance>>,
}

#[derive(Default)]
pub(in crate::renderer) struct AnimatedBatches {
    /// (FULL set?, index) into coarse_batches / full_batches.
    pub(in crate::renderer) batches: Vec<(bool, usize)>,
    /// (auto4 index, FULL source index) for AUTO batches that mirror an animated FULL batch.
    pub(in crate::renderer) auto4: Vec<(usize, usize)>,
}

impl WorldGpu {
    /// Number the distinct `(pipeline, fragment)` keys of every world batch and
    /// rebuild each variant's slot table. Call after batches are created or
    /// their keys change, and after a variant is inserted.
    pub(in crate::renderer) fn refresh_pso_slots(&mut self) {
        let mut slots: std::collections::HashMap<(PipelineKey, MaterialFragmentKey), u32,
            std::hash::BuildHasherDefault<crate::scene::FoldHasher>> = Default::default();
        let mut keys = Vec::new();
        for batch in self.coarse_batches.iter_mut().chain(&mut self.full_batches).chain(&mut self.auto4_batches) {
            let key = (batch.source.pipeline, batch.fragment_key);
            batch.pso_slot = *slots.entry(key).or_insert_with(|| {
                keys.push(key);
                (keys.len() - 1) as u32
            });
        }
        for variant in self.pipeline_variants.values_mut() {
            variant.build_slot_pipelines(&keys);
        }
        self.pso_slot_keys = keys;
    }

    /// Every uploaded world batch representation (MINIMAL, FULL and AUTO).
    pub(in crate::renderer) fn all_world_batches(&self) -> impl Iterator<Item = &WorldBatch> {
        self.coarse_batches.iter().chain(&self.full_batches).chain(&self.auto4_batches)
    }
}

pub(in crate::renderer) struct WorldGpu {
    /// Immutable index source retained for material-only AUTO4 recipe rebinding.
    pub(in crate::renderer) remap_index_source: Arc<Vec<u32>>,
    /// Remap-only reuse key; never consulted by frame/draw code.
    pub(in crate::renderer) remap_asset_source: std::path::PathBuf,
    pub(in crate::renderer) remap_asset_hash: u64,
    pub(in crate::renderer) remap_texture_hashes: Vec<u64>,
    /// Count of pristine-map images before remap-only targets were appended.
    pub(in crate::renderer) remap_base_texture_count: usize,
    pub(in crate::renderer) remap_texture_usage: Vec<u8>,
    pub(in crate::renderer) remap_picmip: u32,
    pub(in crate::renderer) remap_compression: u8,
    pub(in crate::renderer) remap_pbr_compression: bool,
    pub(in crate::renderer) remap_scene_format: wgpu::TextureFormat,
    pub(in crate::renderer) remap_msaa_samples: u32,

    pub(in crate::renderer) bsp_model_lod: super::bsp_model_lod::BspModelLod,
    pub(in crate::renderer) patch_lod: super::patch_lod::WorldPatchLod,
    /// Any batch carries a promoted glass primary stage. Fixed at upload so the
    /// frame path does not scan every batch to decide whether post is needed.
    pub(in crate::renderer) has_glass_primary: bool,
    /// Static compiled CONTENTS_WATER/LAVA volumes, for thermal camera medium.
    pub(in crate::renderer) thermal_liquids: Vec<scene::ThermalLiquidVolume>,
    /// Batches whose material has an animMap. Sources and batch sets never
    /// change after upload; filled on first use so the per-frame animation tick
    /// does not walk every batch.
    pub(in crate::renderer) animated_batches: Option<AnimatedBatches>,
    /// Drawn, markable world surfaces shared with saber marks (R_MarkFragments).
    pub(in crate::renderer) mark_surfaces: Option<Arc<jka_assets::bsp::MarkSurfaces>>,
    /// Direct source-.map worlds have no compiled BSP/PVS. They therefore use
    /// conservative per-batch frustum rejection on their 1024-unit spatial batches.
    pub(in crate::renderer) source_map: bool,
    pub(in crate::renderer) source_map_lighting: scene::SourceMapLighting,
    /// Linear average of this map's skybox faces. Cloud ambient is tinted with
    /// it so a desert, sunset or night sky does not leave the clouds lit by the
    /// reference's hardcoded blue.
    pub(in crate::renderer) sky_average: [f32; 3],
    /// `misc_skyportal` camera; when present the sky is a second world view.
    pub(in crate::renderer) sky_portal: Option<scene::SkyPortal>,
    /// Representative authored map skybox used by non-sky surfaces that need environment sampling (ocean).
    pub(in crate::renderer) primary_skybox: Option<[usize; 6]>,
    /// Every sky shader draws an outer box (see `sky_outer_box_covers_background`).
    pub(in crate::renderer) sky_covers_background: bool,
    /// Distinct promoted water planes/footprints; also drives Godot-style spray placement.
    pub(in crate::renderer) ocean_surfaces: Vec<crate::ocean::OceanSurface>,
    /// Conservative authored floor for each promoted surface, in slot order.
    pub(in crate::renderer) ocean_bottoms: Vec<f32>,
    /// Whether FFT clipmaps were prepared when the map was loaded.
    pub(in crate::renderer) ocean_meshes_built: bool,
    /// All eligible faces (including candidates rejected by the surface cap).
    /// Keys are the same face keys used by the actual promotion pass.
    pub(in crate::renderer) ocean_promotion_decisions: BTreeMap<[u32; 6], OceanPromotionDecision>,
    pub(in crate::renderer) promoted_oceans: Vec<crate::ocean::appearance::PromotedOcean>,
    /// Real outlines of `ocean_surfaces`, by the slot baked into each clipmap.
    pub(in crate::renderer) ocean_masks: Arc<crate::ocean::OceanMasks>,
    /// OFF and ON masks are independent; ON adds only sky-connected geometry.
    pub(in crate::renderer) ocean_base_masks: Arc<crate::ocean::OceanMasks>,
    pub(in crate::renderer) ocean_sky_regions: Vec<Option<crate::ocean::sky_extend::SkyExtension>>,
    /// FULL-batch representatives for continuation draws that outlive PVS.
    pub(in crate::renderer) ocean_sky_representatives: Vec<Option<usize>>,
    /// Fine footprints for exact FULL water pieces, indexed in parallel with
    /// `full_batches`; unioned only when the camera changes BSP cluster.
    pub(in crate::renderer) ocean_pvs_footprints: crate::ocean::OceanPvsFootprints,
    /// Runtime-rebuildable promoted-water index ranges, one per surface and
    /// mesh quality. These are rewritten only when amplitude, 3D-distance,
    /// snapped centre or the camera's BSP cluster changes.
    pub(in crate::renderer) runtime_ocean_clipmaps: Vec<RuntimeOceanClipmap>,
    pub(in crate::renderer) inline_models: Vec<InlineModelGpu>,
    pub(in crate::renderer) inline_transform: Option<super::inline_transform::InlineTransformGpu>,
    pub(in crate::renderer) vertex_buffer: wgpu::Buffer,
    pub(in crate::renderer) index_buffer: wgpu::Buffer,
    /// Opaque, static-world index ranges eligible for the ray-query BLAS.
    pub(in crate::renderer) rt_shadow_ranges: Vec<std::ops::Range<u32>>,
    /// Alpha-tested static stages with no opaque depth-writing sibling. They are
    /// stored as compact CPU geometry only until RT Shadows is selected.
    pub(in crate::renderer) rt_shadow_masks: Vec<RtAlphaMaskSource>,
    pub(in crate::renderer) rt_vertex_count: u32,
    /// Coarsest clipmap cell size; the clipmap centre snaps to this lattice.
    pub(in crate::renderer) ocean_coarsest_spacing: f32,
    pub(in crate::renderer) cull_records_buffer: wgpu::Buffer,
    pub(in crate::renderer) indirect_buffer: wgpu::Buffer,
    /// Hi-Z early-pass draw list, one entry per cull record: last frame's
    /// visible batches, rewritten each frame by `cs_early`.
    pub(in crate::renderer) early_indirect_buffer: wgpu::Buffer,
    pub(in crate::renderer) active_cull_indices_buffer: wgpu::Buffer,
    pub(in crate::renderer) compact_indirect_buffer: wgpu::Buffer,
    pub(in crate::renderer) compact_count_buffer: wgpu::Buffer,
    /// Planar reflection passes pack their visible compact-group members here
    /// (every slot of one frame, back to back) and draw each group with one
    /// CPU-counted multi-draw.
    pub(in crate::renderer) reflection_compact_indirect_buffer: wgpu::Buffer,
    pub(in crate::renderer) reflection_compact_capacity: u32,
    /// Mover multi-draw arguments: one `inline_indirect_capacity` region for
    /// the main pass followed by one per planar reflection slot.
    pub(in crate::renderer) inline_indirect_buffer: wgpu::Buffer,
    pub(in crate::renderer) inline_indirect_capacity: u32,
    pub(in crate::renderer) inline_group_count: usize,
    pub(in crate::renderer) cull_debug_reason_buffer: wgpu::Buffer,
    pub(in crate::renderer) cull_debug_count_buffer: wgpu::Buffer,
    pub(in crate::renderer) cull_debug_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) cull_record_count: usize,
    pub(in crate::renderer) cull_bind_group: Option<wgpu::BindGroup>,
    pub(in crate::renderer) active_cull_count: u32,
    pub(in crate::renderer) active_selection_key:
        Option<(PvsMode, Option<usize>, Option<[u8; 32]>)>,
    pub(in crate::renderer) compact_groups: Vec<WorldDrawGroup>,
    pub(in crate::renderer) light_buffer: wgpu::Buffer,
    pub(in crate::renderer) _cluster_buffer: wgpu::Buffer,
    pub(in crate::renderer) legacy_dlight_surface_id_buffer: wgpu::Buffer,
    pub(in crate::renderer) legacy_dlight_surface_mask_buffer: wgpu::Buffer,
    /// Legacy dlight lit-surface lists: mask mirror and this frame's draws.
    pub(in crate::renderer) legacy_dlight:
        crate::renderer::lighting::legacy_dlight_lists::LegacyDlightLists,
    /// CPU copy used only by the Trace/surface-inspector diagnostic.
    pub(in crate::renderer) legacy_dlight_triangle_surfaces: Vec<u32>,
    pub(in crate::renderer) legacy_dlight_surfaces: Vec<scene::LegacyDlightSurface>,
    pub(in crate::renderer) dynamic_lights: Vec<scene::DynamicLight>,
    pub(in crate::renderer) runtime_light_count: usize,
    pub(in crate::renderer) static_bake_sources: Vec<(usize, scene::DynamicLight)>,
    pub(in crate::renderer) cluster_compute_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) lighting_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) lighting_bind_group_lean: wgpu::BindGroup,
    pub(in crate::renderer) _static_light_grid: StaticLightGridGpu,
    /// CPU copy retained only for classic dynamic-entity lightgrid sampling.
    pub(in crate::renderer) entity_light_grid: Option<scene::ClassicEntityLightGrid>,
    pub(in crate::renderer) voxel_probe_gi: VoxelProbeGiGpu,
    pub(in crate::renderer) reflection_probes: ReflectionProbeGpuSet,
    pub(in crate::renderer) froxel_bind_group: Option<wgpu::BindGroup>,
    pub(in crate::renderer) light_count: u32,
    pub(in crate::renderer) local_shadows: LocalShadowResources,
    pub(in crate::renderer) glass_texture_names: Vec<String>,
    pub(in crate::renderer) raster_glass_static: Vec<crate::renderer::lighting::glass_transmission::RasterGlassTriangle>,
    pub(in crate::renderer) raster_glass_dynamic: Vec<crate::renderer::lighting::glass_transmission::RasterGlassTriangle>,
    pub(in crate::renderer) sun: Option<DirectionalSun>,
    /// Direct irradiance measured from paired sunlit/shadowed BSP surfaces.
    pub(in crate::renderer) baked_sun_radiance: Option<[f32; 3]>,
    pub(in crate::renderer) sun_bake: crate::renderer::lighting::sun_bake::SunBake,
    pub(in crate::renderer) baked_sun_encoding: Option<scene::BakedSunEncoding>,
    pub(in crate::renderer) baked_sun_cache: Option<crate::renderer::lighting::baked_sun_cache::LightmapCache>,
    pub(in crate::renderer) static_sun: crate::renderer::lighting::lightmap_sun::StaticSun,
    pub(in crate::renderer) grass: Option<GrassMapGpu>,
    pub(in crate::renderer) surface_sprite_effects: Vec<SurfaceSpriteEffectGpu>,
    /// Map-load instances for vegetation surfaceSprites (r_surfaceSpritesGpu).
    pub(in crate::renderer) surface_sprite_resident: Option<crate::renderer::environment::surface_sprites::ResidentSurfaceSprites>,
    pub(in crate::renderer) active_pipeline_variant: WorldShaderVariantKey,
    pub(in crate::renderer) pipeline_variants:
        BTreeMap<WorldShaderVariantKey, WorldPipelineVariant>,
    /// Distinct `(pipeline, fragment)` keys; `WorldBatch::pso_slot` indexes it.
    pub(in crate::renderer) pso_slot_keys: Vec<(PipelineKey, MaterialFragmentKey)>,
    pub(in crate::renderer) fast_pipelines: BTreeMap<PipelineKey, wgpu::RenderPipeline>,
    pub(in crate::renderer) fast_complexity_pipelines: BTreeMap<PipelineKey, wgpu::RenderPipeline>,
    pub(in crate::renderer) complexity_pipelines: BTreeMap<WorldShaderVariantKey, BTreeMap<PipelineKey, wgpu::RenderPipeline>>,
    pub(in crate::renderer) planar_reflectors: Vec<PlanarReflector>,
    /// Original material/lightmap-first batches. OFF draws all of these; MINIMAL
    /// applies conservative PVS visibility to them.
    pub(in crate::renderer) coarse_batches: Vec<WorldBatch>,
    /// Exact PVS-signature sub-batches referencing subranges of the same vertex buffer.
    pub(in crate::renderer) full_batches: Vec<WorldBatch>,
    pub(in crate::renderer) visibility: Option<jka_assets::bsp::Visibility>,
    pub(in crate::renderer) inline_pvs: crate::renderer::visibility::pvs::InlinePvsSelection,
    pub(in crate::renderer) coarse_visible_batches_by_cluster: Vec<Vec<usize>>,
    pub(in crate::renderer) full_visible_batches_by_cluster: Vec<Vec<usize>>,
    /// AUTO representation. `[0, auto4_variant_base)` is a shallow alias of
    /// `full_batches` (same indices, cull records and compaction groups, used
    /// as the exact cold fallback); `[auto4_variant_base, ..)` holds one batch
    /// per collapsed recipe (visible FULL pieces of one MINIMAL batch).
    pub(in crate::renderer) auto4_batches: Vec<WorldBatch>,
    pub(in crate::renderer) auto4_plan_refs: Vec<Vec<PreparedPortalPlanBatchRef>>,
    pub(in crate::renderer) auto4_plan_by_cluster: Vec<usize>,
    pub(in crate::renderer) auto4_variant_members: Vec<Vec<usize>>,
    pub(in crate::renderer) auto4_variant_geometry: Vec<usize>,
    pub(in crate::renderer) auto4_variant_area_mixed: Vec<bool>,
    pub(in crate::renderer) auto4_geometry_members: Vec<Vec<usize>>,
    pub(in crate::renderer) auto4_geometry_variants: Vec<Vec<usize>>,
    pub(in crate::renderer) auto4_variant_base: usize,
    /// FULL pieces drawn in every cluster: pieces appended after the plan was
    /// built (no PVS) followed by the inline mover tail.
    pub(in crate::renderer) auto4_always_start: usize,
    pub(in crate::renderer) auto4_active_plan: Vec<usize>,
    pub(in crate::renderer) auto4_inline_start: usize,
    pub(in crate::renderer) auto4_inline_full_start: usize,
    pub(in crate::renderer) auto4_collapse_cache: Auto4CollapseCache,
    pub(in crate::renderer) coarse_all_batches: Vec<usize>,
    /// OFF/invalid-cluster fallback for the sky-portal camera. Its draw path
    /// must use FULL geometry rather than reintroducing whole-map sky unions.
    pub(in crate::renderer) full_all_batches: Vec<usize>,
    pub(in crate::renderer) last_cluster: Option<Option<usize>>,
    /// (camera cluster, bit per ocean slot whose original water faces that
    /// cluster can see). Gates the sky-extension ocean; see
    /// `Renderer::refresh_ocean_extension_gate`.
    pub(in crate::renderer) ocean_gate_cache: Option<(usize, u8)>,
    pub(in crate::renderer) textures: Vec<GpuImage>,
    pub(in crate::renderer) footprint_mark_textures: [Option<usize>; 2],
    pub(in crate::renderer) lightmaps: Vec<GpuImage>,
    pub(in crate::renderer) deluxemaps: Vec<GpuImage>,
    pub(in crate::renderer) lightmap_base_data: Arc<Vec<TextureData>>,
    pub(in crate::renderer) static_ao_source: Option<StaticAoWorldSource>,
    pub(in crate::renderer) static_ao_source_to_gpu: Vec<u32>,
    pub(in crate::renderer) static_ao_gpu_vertices: Vec<GpuVertex>,
    pub(in crate::renderer) static_ao_lightmap_active: bool,
    pub(in crate::renderer) texture_clamp: Vec<bool>,
    /// AUTO detail image index keyed by the already-bound base texture index.
    /// This keeps detail selection packing-neutral: batches that can share a draw
    /// already require the same base texture, so they necessarily resolve the
    /// same detail image. Conflicting MATERIAL_* uses collapse to generic.
    pub(in crate::renderer) detail_texture_by_base: Vec<u8>,
    pub(in crate::renderer) material_debug: scene::MaterialDebugInfo,
    pub(in crate::renderer) inspector_vertices: Vec<InspectorVertex>,
    pub(in crate::renderer) inspector_textures: Vec<InspectorTextureMeta>,
    pub(in crate::renderer) inspector_lightmaps: Vec<InspectorTextureMeta>,
    /// Dense replacement geometry is generated only near the latest verified
    /// snow contact. The source list retains the original (coarse) BSP snow
    /// triangles so the rest of the map never becomes a high-density mesh.
    pub(in crate::renderer) snow_shell: SnowShellGpu,
}

// Lightmap-space AO should emphasize true contact/corner occlusion. A small
// render-triangle bias keeps the bake close enough to catch wall/ceiling seams
// without the broad "dirty room" look of the earlier 512u-heavy blend.
pub(in crate::renderer) const STATIC_AO_LIGHTMAP_BIAS: f32 = 0.75;

pub(in crate::renderer) const STATIC_AO_VERTEX_BIAS: f32 = 2.0;

pub(in crate::renderer) const STATIC_AO_HQ_CONTACT_RADIUS: f32 = 32.0;

pub(in crate::renderer) const STATIC_AO_HQ_MEDIUM_RADIUS: f32 = 128.0;

pub(in crate::renderer) const STATIC_AO_HQ_BROAD_RADIUS: f32 = 384.0;

pub(in crate::renderer) const STATIC_AO_HQ_CONTACT_WEIGHT: f32 = 0.65;

pub(in crate::renderer) const STATIC_AO_HQ_MEDIUM_WEIGHT: f32 = 0.30;

pub(in crate::renderer) const STATIC_AO_HQ_BROAD_WEIGHT: f32 = 0.05;

pub(in crate::renderer) const STATIC_AO_BVH_LEAF_TRIANGLES: usize = 8;

pub(in crate::renderer) const STATIC_AO_ADAPTIVE_MAX_VISIBILITY_RANGE: u8 = 12;

pub(in crate::renderer) const STATIC_AO_ADAPTIVE_MAX_CENTER_ERROR: u8 = 8;

pub(in crate::renderer) const STATIC_AO_ADAPTIVE_MIN_NORMAL_DOT: f32 = 0.995;
