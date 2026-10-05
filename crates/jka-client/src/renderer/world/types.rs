//! World types.
use crate::renderer::{
    legacy_dlight_receives, legacy_fog_pass_jobs, scene, Arc, Auto4CollapseCache, BTreeMap,
    BTreeSet, CullMode, DirectionalSun, DrawBatch, GpuImage, GpuVertex, GrassMapGpu, Hash,
    InlineModelInstance, InspectorTextureMeta, InspectorVertex, LocalShadowResources, Mat4, Mutex,
    PipelineKey, PreparedPortalPlanBatchRef, PvsMode, RtAlphaMaskSource, SnowShellGpu,
    StaticAoWorldSource, SurfaceSpriteEffectGpu, TextureData, Vec3, WorldShaderVariantKey,
};

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

#[derive(Clone)]
pub(in crate::renderer) struct WorldBatch {
    pub(in crate::renderer) source: DrawBatch,
    pub(in crate::renderer) indexed_range: std::ops::Range<u32>,
    /// Contiguous authored-BSP surface runs inside `indexed_range`. Legacy
    /// projected lights use these to redraw only triangles belonging to a
    /// surface whose OpenJK-style dlight mask is non-zero. Inline BSP models
    /// intentionally leave this empty and use their existing all-light fallback.
    pub(in crate::renderer) legacy_dlight_runs: Vec<LegacyDlightRun>,
    /// True for a CG_Mover inline BSP model rather than static world geometry.
    /// Debug-only classification; normal world submission never branches on it.
    pub(in crate::renderer) is_inline_entity: bool,
    /// Clipmap index ranges for this promoted water surface, low then high.
    pub(in crate::renderer) ocean_clipmap: Option<[std::ops::Range<u32>; 2]>,
    /// Which promoted surface `ocean_clipmap` belongs to. Every face merged into
    /// one surface shares that clipmap, so a pass draws it once, not per face.
    pub(in crate::renderer) ocean_clipmap_id: u8,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
    pub(in crate::renderer) fast_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) _material_buffer: UniformSlot,
    pub(in crate::renderer) _fog_buffer: UniformSlot,
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
            has_planar_reflectors: !world.planar_reflectors.is_empty(),
        }
    }
}

pub(in crate::renderer) struct WorldPipelineVariant {
    pub(in crate::renderer) pipelines: BTreeMap<PipelineKey, wgpu::RenderPipeline>,
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
    pub(in crate::renderer) coarse_batches: Vec<usize>,
    pub(in crate::renderer) full_batches: Vec<usize>,
    /// Last uploaded pose: `None` = never updated (buffer holds the compiled
    /// pose), `Some(None)` = hidden (not in the snapshot).
    pub(in crate::renderer) uploaded: Option<Option<InlineModelInstance>>,
}

pub(in crate::renderer) struct WorldGpu {
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
    /// Distinct promoted water planes/footprints; also drives Godot-style spray placement.
    pub(in crate::renderer) ocean_surfaces: Vec<crate::ocean::OceanSurface>,
    /// Real outlines of `ocean_surfaces`, by the slot baked into each clipmap.
    pub(in crate::renderer) ocean_masks: Arc<crate::ocean::OceanMasks>,
    pub(in crate::renderer) inline_models: Vec<InlineModelGpu>,
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
    /// Mirrors the GPU surface-mask buffer so the isolated Legacy dlight pass
    /// can submit only touched triangle runs instead of rasterizing every BSP
    /// surface just to discover a zero mask in the fragment shader.
    pub(in crate::renderer) legacy_dlight_surface_masks_cpu: Mutex<Vec<u32>>,
    /// CPU copy used only by the Trace/surface-inspector diagnostic.
    pub(in crate::renderer) legacy_dlight_triangle_surfaces: Vec<u32>,
    pub(in crate::renderer) legacy_dlight_surfaces: Vec<scene::LegacyDlightSurface>,
    pub(in crate::renderer) dynamic_lights: Vec<scene::DynamicLight>,
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
    pub(in crate::renderer) sun: Option<DirectionalSun>,
    pub(in crate::renderer) grass: Option<GrassMapGpu>,
    pub(in crate::renderer) surface_sprite_effects: Vec<SurfaceSpriteEffectGpu>,
    pub(in crate::renderer) active_pipeline_variant: WorldShaderVariantKey,
    pub(in crate::renderer) pipeline_variants:
        BTreeMap<WorldShaderVariantKey, WorldPipelineVariant>,
    pub(in crate::renderer) fast_pipelines: BTreeMap<PipelineKey, wgpu::RenderPipeline>,
    pub(in crate::renderer) planar_reflectors: Vec<PlanarReflector>,
    /// Original material/lightmap-first batches. OFF draws all of these; MINIMAL
    /// applies conservative PVS visibility to them.
    pub(in crate::renderer) coarse_batches: Vec<WorldBatch>,
    /// Exact PVS-signature sub-batches referencing subranges of the same vertex buffer.
    pub(in crate::renderer) full_batches: Vec<WorldBatch>,
    pub(in crate::renderer) visibility: Option<jka_assets::bsp::Visibility>,
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
    pub(in crate::renderer) last_cluster: Option<Option<usize>>,
    pub(in crate::renderer) textures: Vec<GpuImage>,
    pub(in crate::renderer) footprint_mark_textures: [Option<usize>; 2],
    pub(in crate::renderer) lightmaps: Vec<GpuImage>,
    pub(in crate::renderer) deluxemaps: Vec<GpuImage>,
    pub(in crate::renderer) lightmap_base_data: Vec<TextureData>,
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
