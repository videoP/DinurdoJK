//! Models types.
use crate::renderer::{
    scene, Arc, DynamicModelAlphaMode, DynamicModelVertex, DynamicWireframeClass,
    EntityAmbientLightingMode, FxGpuSpriteInstance, Ghoul2BatchMode, Ghoul2GpuBone, GpuImage, Hash,
    HashMap, Pod, TextureData, Vec3, Zeroable,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(in crate::renderer) struct Ghoul2GpuDrawUniform {
    pub(in crate::renderer) axis0: [f32; 4],
    pub(in crate::renderer) axis1: [f32; 4],
    pub(in crate::renderer) axis2: [f32; 4],
    pub(in crate::renderer) origin: [f32; 4],
    pub(in crate::renderer) color: [f32; 4],
    pub(in crate::renderer) params: [u32; 4],
    pub(in crate::renderer) classic_ambient: [f32; 4],
    pub(in crate::renderer) classic_directed: [f32; 4],
    pub(in crate::renderer) classic_direction: [f32; 4],
    // Runtime sun as a second directional light (entity sun lighting). Zero
    // radiance when the feature is off, which adds nothing in the shaders.
    pub(in crate::renderer) classic_sun_directed: [f32; 4],
    pub(in crate::renderer) classic_sun_direction: [f32; 4],
    /// Per-draw texture-coordinate transform `uv * xy + zw` (shader tcMod
    /// scale/scroll); identity is [1, 1, 0, 0].
    pub(in crate::renderer) uv_xform: [f32; 4],
    /// xyz = light point, w = 1 when `alphaGen lightingSpecular` applies.
    pub(in crate::renderer) spec_light: [f32; 4],
    pub(in crate::renderer) spec_viewer: [f32; 4],
    pub(in crate::renderer) jiggle0: [f32; 4],
    pub(in crate::renderer) jiggle1: [f32; 4],
    pub(in crate::renderer) jiggle2: [f32; 4],
    pub(in crate::renderer) jiggle3: [f32; 4],
}

/// Everything needed to swap the baked map sun in a lightgrid sample for the
/// runtime sun. Built per frame by `Renderer::entity_sun_relight`.
#[derive(Clone, Copy, Debug)]
pub(in crate::renderer) struct EntitySunRelight {
    /// Unit direction toward the sun baked into the lightgrid.
    pub(in crate::renderer) map_toward_sun: [f32; 3],
    /// Unit direction toward the runtime sun.
    pub(in crate::renderer) toward_sun: [f32; 3],
    /// Runtime sun radiance / baked map sun radiance, per channel.
    pub(in crate::renderer) gain: [f32; 3],
}

pub(in crate::renderer) fn sample_entity_classic_light(
    grid: &scene::ClassicEntityLightGrid,
    origin: [f32; 3],
    relight: Option<EntitySunRelight>,
) -> Option<scene::ClassicEntityLight> {
    let mut light = grid.sample_classic_entity_light(origin)?;
    if let Some(relight) = relight {
        light.relight_sun(relight.map_toward_sun, relight.toward_sun, relight.gain);
    }
    Some(light)
}

/// Classic vertex lighting for one normal: ambient + the (remaining) baked
/// directed light + the optional runtime sun, in 0..255 light units.
pub(in crate::renderer) fn classic_vertex_illumination(
    light: &scene::ClassicEntityLight,
    normal: Vec3,
) -> [f32; 3] {
    let incoming = normal.dot(Vec3::from_array(light.direction)).max(0.0);
    let sun_incoming = normal.dot(Vec3::from_array(light.sun_direction)).max(0.0);
    std::array::from_fn(|channel| {
        (light.ambient[channel]
            + incoming * light.directed[channel]
            + sun_incoming * light.sun_directed[channel])
            .clamp(0.0, 255.0)
            / 255.0
    })
}

pub(in crate::renderer) struct Ghoul2GpuMesh {
    pub(in crate::renderer) vertex_buffer: wgpu::Buffer,
    pub(in crate::renderer) index_buffer: wgpu::Buffer,
    pub(in crate::renderer) index_count: u32,
}

pub(in crate::renderer) struct Ghoul2RtComputeMesh {
    pub(in crate::renderer) input_buffer: wgpu::Buffer,
    pub(in crate::renderer) draw_indices_buffer: wgpu::Buffer,
    pub(in crate::renderer) draw_indices_capacity: u64,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
    pub(in crate::renderer) vertex_count: u32,
}

#[derive(Clone)]
pub(in crate::renderer) struct Ghoul2RtComputeBatch {
    pub(in crate::renderer) mesh_key: Arc<str>,
    pub(in crate::renderer) instance_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(in crate::renderer) struct RayTracedSkinnedMeshKey {
    pub(in crate::renderer) entity_num: u16,
    pub(in crate::renderer) mesh_key: Arc<str>,
    pub(in crate::renderer) non_opaque: bool,
}

#[derive(Clone)]
pub(in crate::renderer) struct RayTracedSkinnedPrepared {
    pub(in crate::renderer) key: RayTracedSkinnedMeshKey,
    pub(in crate::renderer) first_vertex: u32,
    pub(in crate::renderer) vertex_count: u32,
    pub(in crate::renderer) first_index: u32,
    pub(in crate::renderer) index_count: u32,
    pub(in crate::renderer) alpha_texture: Option<Arc<TextureData>>,
}

pub(in crate::renderer) struct Ghoul2RtSkinningResources {
    pub(in crate::renderer) mesh_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) global_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) pipeline: wgpu::ComputePipeline,
    pub(in crate::renderer) meshes: HashMap<Arc<str>, Ghoul2RtComputeMesh>,
    pub(in crate::renderer) vertex_buffer: wgpu::Buffer,
    pub(in crate::renderer) index_buffer: wgpu::Buffer,
    pub(in crate::renderer) vertex_capacity: u64,
    pub(in crate::renderer) index_capacity: u64,
    pub(in crate::renderer) global_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) batches: Vec<Ghoul2RtComputeBatch>,
    pub(in crate::renderer) casters: Vec<RayTracedSkinnedPrepared>,
}

pub(in crate::renderer) struct Ghoul2PreparedDraw {
    pub(in crate::renderer) entity_num: u16,
    pub(in crate::renderer) mesh_key: Arc<str>,
    pub(in crate::renderer) texture_key: Option<Arc<str>>,
    pub(in crate::renderer) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::renderer) wireframe_class: DynamicWireframeClass,
    pub(in crate::renderer) first_instance: u32,
    pub(in crate::renderer) instance_count: u32,
}

pub(in crate::renderer) struct Ghoul2PendingDraw {
    pub(in crate::renderer) entity_num: u16,
    pub(in crate::renderer) mesh_key: Arc<str>,
    pub(in crate::renderer) texture_key: Option<Arc<str>>,
    pub(in crate::renderer) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::renderer) wireframe_class: DynamicWireframeClass,
    pub(in crate::renderer) uniform: Ghoul2GpuDrawUniform,
    pub(in crate::renderer) sequence: u32,
    // Adaptive batching starts by hashing a bounded distributed sample.  If
    // the probe says batching is worthwhile, retain those hashes so the full
    // grouping pass continues from the work already done instead of starting
    // over.  Exact keys are still compared after the hash, so collisions are
    // harmless.
    pub(in crate::renderer) batch_hash: Option<u64>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct DynamicEntityLightingUniform {
    // x: EntityAmbientLightingMode (0=off, 1=BSP lightgrid, 2=irradiance volume).
    // BSP lightgrid is applied during dynamic-model preparation (CPU vertices)
    // or in the Ghoul2 vertex shader (GPU skinning); mode 2 retains the legacy
    // placeholder until dynamic models sample the actual irradiance volume.
    // y: f32 bits of the model brightness multiplier (Setup > Video > Models).
    pub(in crate::renderer) values: [u32; 4],
    // Self-applied Legacy fog, see md3.wgsl apply_self_legacy_fog.
    pub(in crate::renderer) legacy_fog: EntityLegacyFog,
    // Cloud ground shadow: xyz = unit direction toward the sun (render space),
    // w = 1 while projected cloud shadows are active. The entity depth prepass
    // uses it to publish a smooth sun-facing value (md3.wgsl).
    pub(in crate::renderer) cloud_shadow_sun: [f32; 4],
}

/// Legacy fog applied in the entity/grass shaders. Zeroed (mode 0) disables it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct EntityLegacyFog {
    /// Linear RGB, depthForOpaque.
    pub color_depth: [f32; 4],
    /// x: 0 off, 1 authored global EXP2, 2 manual; y: strength scale.
    pub params: [f32; 4],
}

impl DynamicEntityLightingUniform {
    pub(in crate::renderer) fn new(
        mode: EntityAmbientLightingMode,
        legacy_fog: EntityLegacyFog,
        brightness: f32,
        cloud_shadow_sun: [f32; 4],
    ) -> Self {
        let value = match mode {
            EntityAmbientLightingMode::Off => 0,
            EntityAmbientLightingMode::BspLightgridClassic => 1,
            EntityAmbientLightingMode::BevyIrradianceVolume => 2,
        };
        Self {
            values: [value, brightness.to_bits(), 0, 0],
            legacy_fog,
            cloud_shadow_sun,
        }
    }
}

// CG_ImpactMark's blob-shadow box: projected 20 units against the plane normal
// (the shared projector adds the stock 32-unit near plane).
pub(in crate::renderer) const BLOB_MARK_PROJECTION: f32 = 20.0;

// polygonOffset stand-in: dynamic meshes carry no depth bias, so lift the mark.
pub(in crate::renderer) const BLOB_MARK_SURFACE_LIFT: f32 = 0.25;

pub(in crate::renderer) struct DynamicGpuTexture {
    pub(in crate::renderer) _image: GpuImage,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
}

pub(in crate::renderer) struct FxGpuSpritePreparedDraw {
    pub(in crate::renderer) entity_num: u16,
    pub(in crate::renderer) first_instance: u32,
    pub(in crate::renderer) instance_count: u32,
    pub(in crate::renderer) texture_key: Option<Arc<str>>,
    pub(in crate::renderer) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::renderer) wireframe_class: DynamicWireframeClass,
}

pub(in crate::renderer) struct DynamicPreparedDraw {
    pub(in crate::renderer) entity_num: u16,
    pub(in crate::renderer) first_index: u32,
    pub(in crate::renderer) index_count: u32,
    pub(in crate::renderer) base_vertex: i32,
    pub(in crate::renderer) texture_key: Option<Arc<str>>,
    pub(in crate::renderer) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::renderer) wireframe_class: DynamicWireframeClass,
}

/// Dynamic indexed-mesh path used by cgame entities. CPU-skinned surfaces use
/// the legacy transient stream; GPU Ghoul2 surfaces keep static bind-pose meshes
/// cached here and upload only final CPU-evaluated bones plus per-draw transforms.
/// The renderer still knows nothing about demo/network provenance or JKA state.
pub(in crate::renderer) struct DynamicModelRenderer {
    pub(in crate::renderer) rt_raw_color_buffer: Option<wgpu::Buffer>,
    pub(in crate::renderer) scratch_raw_colors: Vec<[f32; 4]>,
    pub(in crate::renderer) texture_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) entity_lighting_buffer: wgpu::Buffer,
    pub(in crate::renderer) entity_ambient_lighting: EntityAmbientLightingMode,
    /// Set by the renderer each frame before `prepare`; None keeps the stock
    /// lightgrid sample untouched.
    pub(in crate::renderer) entity_sun_relight: Option<EntitySunRelight>,
    pub(in crate::renderer) model_brightness: f32,
    pub(in crate::renderer) entity_legacy_fog: EntityLegacyFog,
    /// Last value written to the uniform's `cloud_shadow_sun`; w = 0 when off.
    pub(in crate::renderer) cloud_shadow_sun: [f32; 4],
    /// Whether the colour pipelines were built with ENABLE_LEGACY_FOG.
    pub(in crate::renderer) legacy_fog_compiled: bool,
    pub(in crate::renderer) pipeline_layout: wgpu::PipelineLayout,
    pub(in crate::renderer) shader: wgpu::ShaderModule,
    pub(in crate::renderer) wireframe_pipeline: Option<wgpu::RenderPipeline>,
    /// Lazy per-alpha-mode pipelines; built only for modes with draws.
    pub(in crate::renderer) model_pipelines:
        [Option<wgpu::RenderPipeline>; DynamicModelAlphaMode::COUNT],
    /// Bit per `DynamicModelAlphaMode::index` with a prepared MD3 draw this frame.
    pub(in crate::renderer) model_alpha_mask: u16,
    pub(in crate::renderer) fx_sprite_shader: Option<wgpu::ShaderModule>,
    pub(in crate::renderer) fx_sprite_surface_format: wgpu::TextureFormat,
    pub(in crate::renderer) fx_sprite_samples: u32,
    pub(in crate::renderer) fx_sprite_wireframe_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_opaque_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_mask_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_blend_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_mask_blend_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_additive_one_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_additive_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_blend_unlit_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_modulate_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_dst_color_add_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_modulate2x_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_sprite_darken_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) fx_zero_alpha_discard: bool,
    pub(in crate::renderer) skin_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) skin_pipeline_layout: wgpu::PipelineLayout,
    pub(in crate::renderer) skin_shader: wgpu::ShaderModule,
    pub(in crate::renderer) skin_wireframe_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) skin_pipelines:
        [Option<wgpu::RenderPipeline>; DynamicModelAlphaMode::COUNT],
    /// Bit per `DynamicModelAlphaMode::index` with a prepared Ghoul2 draw this frame.
    pub(in crate::renderer) skin_alpha_mask: u16,
    // Depth-writing entities in the single-sample scene depth prepass. These
    // target the fixed prepass formats, so they never depend on MSAA/HDR.
    pub(in crate::renderer) prepass_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) prepass_mask_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) skin_prepass_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) skin_prepass_mask_pipeline: wgpu::RenderPipeline,
    /// Depth-only light-space variants for entity shadow casters, in the order
    /// [md3 opaque, md3 mask, skin opaque, skin mask]. Index 0 is forward-Z (Entity
    /// map and legacy CSM), 1 is reverse-Z (Bevy CSM). Each is built lazily the
    /// first time it draws, so modes that never need it never compile it.
    pub(in crate::renderer) entity_shadow_pipelines: [Option<[wgpu::RenderPipeline; 4]>; 2],
    pub(in crate::renderer) repeat_sampler: wgpu::Sampler,
    pub(in crate::renderer) clamp_sampler: wgpu::Sampler,
    pub(in crate::renderer) fallback_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) textures: HashMap<Arc<str>, DynamicGpuTexture>,
    pub(in crate::renderer) vertex_buffer: wgpu::Buffer,
    pub(in crate::renderer) index_buffer: wgpu::Buffer,
    pub(in crate::renderer) vertex_capacity: u64,
    pub(in crate::renderer) index_capacity: u64,
    pub(in crate::renderer) draws: Vec<DynamicPreparedDraw>,
    pub(in crate::renderer) fx_sprite_instance_buffer: wgpu::Buffer,
    pub(in crate::renderer) fx_sprite_instance_capacity: u64,
    pub(in crate::renderer) fx_sprite_draws: Vec<FxGpuSpritePreparedDraw>,
    pub(in crate::renderer) scratch_fx_sprite_instances: Vec<FxGpuSpriteInstance>,
    /// Floor-clipped blob-shadow mesh (local indices) for the frame's
    /// `FxGpuSprites::blob_shadow` surface, built by the renderer before `prepare`.
    pub(in crate::renderer) blob_mesh_vertices: Vec<DynamicModelVertex>,
    pub(in crate::renderer) blob_mesh_indices: Vec<u32>,
    pub(in crate::renderer) ghoul2_meshes: HashMap<Arc<str>, Ghoul2GpuMesh>,
    pub(in crate::renderer) skin_bone_buffer: wgpu::Buffer,
    pub(in crate::renderer) skin_draw_buffer: wgpu::Buffer,
    pub(in crate::renderer) skin_bone_capacity: u64,
    pub(in crate::renderer) skin_draw_capacity: u64,
    pub(in crate::renderer) skin_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) skin_draws: Vec<Ghoul2PreparedDraw>,
    pub(in crate::renderer) ghoul2_batch_draws: Ghoul2BatchMode,
    /// Entity hidden only when drawing the secondary first-person spectator view.
    /// `prepare` keeps this entity out of cross-entity Ghoul2 instance batches so
    /// the companion draw can reject it without changing the primary draw list.
    pub(in crate::renderer) companion_excluded_entity: Option<u16>,
    pub(in crate::renderer) ghoul2_input_instances: u32,
    pub(in crate::renderer) ghoul2_encoded_draws: u32,
    // Reused frame scratch storage. Dynamic model preparation runs every frame;
    // retaining these allocations avoids allocator churn in many-entity scenes.
    pub(in crate::renderer) scratch_vertices: Vec<DynamicModelVertex>,
    pub(in crate::renderer) scratch_indices: Vec<u32>,
    pub(in crate::renderer) scratch_bones: Vec<Ghoul2GpuBone>,
    pub(in crate::renderer) scratch_draw_data: Vec<Ghoul2GpuDrawUniform>,
    pub(in crate::renderer) scratch_bone_offsets: HashMap<usize, u32>,
    pub(in crate::renderer) scratch_skin_pending: Vec<Ghoul2PendingDraw>,
    /// Lazily created only while RT Shadows is active. RT-off keeps the original
    /// Ghoul2 vertex-shader skinning path and never allocates/dispatches this.
    pub(in crate::renderer) rt_skinning: Option<Ghoul2RtSkinningResources>,
    pub(in crate::renderer) rt_prepared: bool,
    // Per-frame pointer -> stable key cache. TextureData is Arc-backed and immutable
    // during prepare, so identical surfaces can avoid rebuilding the same formatted
    // texture cache key hundreds of times in crowd scenes. Cleared every frame so
    // allocator pointer reuse can never alias an older TextureData.
    pub(in crate::renderer) frame_texture_keys: HashMap<usize, Arc<str>>,
}
