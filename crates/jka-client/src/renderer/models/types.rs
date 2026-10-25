//! Models types.
use crate::renderer::{
    scene, Arc, DynamicModelAlphaMode, DynamicModelVertex, DynamicWireframeClass,
    EntityAmbientLightingMode, FxGpuSpriteInstance, Ghoul2BatchMode, Ghoul2GpuBone, GpuImage, Hash,
    HashMap, Pod, SourceMaterialParams, TextureData, Vec3, Zeroable,
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
    pub(in crate::renderer) uv_cross: [f32; 4],
    /// xyz = light point, w = 1 when `alphaGen lightingSpecular` applies.
    pub(in crate::renderer) spec_light: [f32; 4],
    pub(in crate::renderer) spec_viewer: [f32; 4],
    pub(in crate::renderer) jiggle0: [f32; 4],
    pub(in crate::renderer) jiggle1: [f32; 4],
    pub(in crate::renderer) jiggle2: [f32; 4],
    pub(in crate::renderer) jiggle3: [f32; 4],
    pub(in crate::renderer) jiggle4: [f32; 4],
    /// Source 1 `$phong`: boost, exponent, `$rimlightboost`, `$rimlightexponent`.
    /// A zero boost switches that term off. Read only by the
    /// `ENABLE_SOURCE_MATERIAL` fragment variants.
    pub(in crate::renderer) source_phong: [f32; 4],
    /// xyz: `$phongfresnelranges`, packed `((mid-min)*2, mid, (max-mid)*2)`.
    /// w: bit 0 = `$halflambert`.
    pub(in crate::renderer) source_fresnel: [f32; 4],
    /// Per-player water immersion (8 body-relative bands; independent of rain).
    pub(in crate::renderer) water_bands0: [f32; 4],
    pub(in crate::renderer) water_bands1: [f32; 4],
    /// x: first sampled height in renderer Y-up; w: profile present.
    pub(in crate::renderer) water_base: [f32; 4],
}

/// `SkinDraw::source_phong` for one surface: `$phongboost`, `$phongexponent`,
/// `$rimlightboost`, `$rimlightexponent`. All zero without a Source material,
/// which is also what a zero boost means to the shader: term off.
pub(in crate::renderer) fn source_phong(material: Option<&SourceMaterialParams>) -> [f32; 4] {
    material.map_or([0.0; 4], |material| {
        [
            material.phong_boost,
            material.phong_exponent,
            material.rimlight_boost,
            material.rimlight_exponent,
        ]
    })
}

/// `SkinDraw::source_fresnel`: packed `$phongfresnelranges` plus the
/// `$halflambert` flag in w.
pub(in crate::renderer) fn source_fresnel(material: Option<&SourceMaterialParams>) -> [f32; 4] {
    material.map_or([0.0; 4], |material| {
        [
            material.phong_fresnel[0],
            material.phong_fresnel[1],
            material.phong_fresnel[2],
            f32::from(u8::from(material.half_lambert)),
        ]
    })
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

/// One runtime dynamic light as classic entity lighting consumes it: the
/// `dlight_t` that R_SetupEntityLighting walks.
#[derive(Clone, Copy, Debug)]
pub(in crate::renderer) struct EntityDlight {
    /// Renderer coordinates.
    pub(in crate::renderer) position: Vec3,
    /// `dl->color`, already scaled by the light's intensity and the Dynamic
    /// Light Brightness setting.
    pub(in crate::renderer) color: Vec3,
    pub(in crate::renderer) radius: f32,
}

/// MAX_DLIGHTS (tr_types.h); the Legacy BSP dlight masks share the cap.
pub(in crate::renderer) const MAX_ENTITY_DLIGHTS: usize = 32;
// tr_light.cpp: light added at the edge of a dlight's influence, and the
// shortest range ever used, so a light inside the model cannot blow up.
const DLIGHT_AT_RADIUS: f32 = 16.0;
const DLIGHT_MINIMUM_RADIUS: f32 = 16.0;

/// The dlight loop of TaystJK's R_SetupEntityLighting: each dlight adds
/// `16 r^2 / d^2` of its colour to the directed light and pulls the light
/// direction toward itself by the same weight. There is no radius cutoff.
/// `origin` is the lighting origin in renderer coordinates.
pub(in crate::renderer) fn add_entity_dlights(
    light: &mut scene::ClassicEntityLight,
    origin: Vec3,
    dlights: &[EntityDlight],
) {
    if dlights.is_empty() {
        return;
    }
    let mut directed = Vec3::from_array(light.directed);
    let mut direction = Vec3::from_array(light.direction) * directed.length();
    for dlight in dlights {
        let delta = dlight.position - origin;
        let distance = delta.length();
        let toward = if distance > 0.0 {
            delta / distance
        } else {
            Vec3::ZERO
        };
        let distance = distance.max(DLIGHT_MINIMUM_RADIUS);
        let weight = DLIGHT_AT_RADIUS * dlight.radius * dlight.radius / (distance * distance);
        directed += dlight.color * weight;
        direction += toward * weight;
    }
    light.directed = directed.to_array();
    light.direction = direction.normalize_or_zero().to_array();
}

/// Classic entity lighting for one prepare pass, memoized on the lighting
/// origin. Every surface of a multi-part model shares that origin
/// (RF_LIGHTING_ORIGIN), so a player's head, torso, legs and weapon cost one
/// lightgrid sample and one dlight loop instead of one per surface.
#[derive(Default)]
pub(in crate::renderer) struct EntityLightMemo {
    last: Option<([u32; 3], Option<scene::ClassicEntityLight>)>,
}

impl EntityLightMemo {
    /// `origin` is the JKA-space lighting origin, as `sample_entity_classic_light` takes it.
    pub(in crate::renderer) fn light(
        &mut self,
        grid: &scene::ClassicEntityLightGrid,
        origin: [f32; 3],
        relight: Option<EntitySunRelight>,
        dlights: &[EntityDlight],
    ) -> Option<scene::ClassicEntityLight> {
        let key = origin.map(f32::to_bits);
        if let Some((last_key, light)) = self.last {
            if last_key == key {
                return light;
            }
        }
        let mut light = sample_entity_classic_light(grid, origin, relight);
        if let Some(light) = light.as_mut() {
            add_entity_dlights(
                light,
                Vec3::from_array(scene::render_position(origin)),
                dlights,
            );
        }
        self.last = Some((key, light));
        light
    }
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
    pub(in crate::renderer) bind_radius: f32,
    pub(in crate::renderer) vertex_buffer: wgpu::Buffer,
    pub(in crate::renderer) index_buffer: wgpu::Buffer,
    pub(in crate::renderer) index_count: u32,
    pub(in crate::renderer) batch_keys: super::prepare::BatchKeyCache,
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
    pub(in crate::renderer) authored_glow: bool,
    /// This surface came from a Source 1 VMT, so it needs the pipeline variant
    /// that samples the normal map and runs Valve's phong.
    pub(in crate::renderer) source_material: bool,
    pub(in crate::renderer) first_instance: u32,
    pub(in crate::renderer) instance_count: u32,
}

pub(in crate::renderer) struct Ghoul2PendingDraw {
    pub(in crate::renderer) entity_num: u16,
    pub(in crate::renderer) mesh_key: Arc<str>,
    pub(in crate::renderer) texture_key: Option<Arc<str>>,
    pub(in crate::renderer) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::renderer) wireframe_class: DynamicWireframeClass,
    pub(in crate::renderer) authored_glow: bool,
    pub(in crate::renderer) source_material: bool,
    /// Stable material bind-group ID, for material-local Ghoul2 sorting.
    pub(in crate::renderer) texture_binding_id: u64,
    pub(in crate::renderer) uniform: Ghoul2GpuDrawUniform,
    pub(in crate::renderer) sequence: u32,
    // Cached sort/probe hash; exact keys are compared before instancing, so
    // collisions cannot combine incompatible GLM surfaces.
    pub(in crate::renderer) batch_hash: Option<u64>,
    /// Collision-free precomputed identity for mesh/material/pipeline/glow.
    pub(in crate::renderer) batch_group_id: Option<u64>,
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
    // Wet weather on entities, see model_weather.wgsl. Models carry no weather
    // bindings of their own, so the few values they need ride here, in the uniform
    // every model draw already binds.
    pub(in crate::renderer) weather: EntityWeather,
}

/// Wet weather applied to entities. Zeroed (wetness 0) disables it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct EntityWeather {
    /// xyz: the sun's travel direction in render space; w: relative sun intensity
    /// (250 = 1). Matches `WeatherSurfaceUniform`, so a model and the ground it
    /// stands on are lit by the same sun.
    pub sun_direction: [f32; 4],
    /// rgb: linear sun colour; w: wetness 0..1, already faded by rain intensity
    /// and by the camera's own exposure to the rain.
    pub sun_radiance: [f32; 4],
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
        weather: EntityWeather,
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
            weather,
        }
    }
}

// CG_ImpactMark's blob-shadow box: projected 20 units against the plane normal
// (the shared projector adds the stock 32-unit near plane).
pub(in crate::renderer) const BLOB_MARK_PROJECTION: f32 = 20.0;

// polygonOffset stand-in: dynamic meshes carry no depth bias, so lift the mark.
pub(in crate::renderer) const BLOB_MARK_SURFACE_LIFT: f32 = 0.25;

pub(in crate::renderer) struct DynamicGpuTexture {
    pub(in crate::renderer) binding_id: u64,
    pub(in crate::renderer) _image: GpuImage,
    /// The surface's `$bumpmap`, kept alive for as long as its bind group. Not
    /// registered with baked brightness: it is linear, which `register` skips.
    pub(in crate::renderer) _normal_image: Option<GpuImage>,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
}

pub(in crate::renderer) struct FxGpuSpritePreparedDraw {
    pub(in crate::renderer) entity_num: u16,
    pub(in crate::renderer) first_instance: u32,
    pub(in crate::renderer) instance_count: u32,
    pub(in crate::renderer) texture_key: Option<Arc<str>>,
    pub(in crate::renderer) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::renderer) wireframe_class: DynamicWireframeClass,
    pub(in crate::renderer) authored_glow: bool,

}

pub(in crate::renderer) struct DynamicPreparedDraw {
    pub(in crate::renderer) entity_num: u16,
    pub(in crate::renderer) first_index: u32,
    pub(in crate::renderer) index_count: u32,
    pub(in crate::renderer) base_vertex: i32,
    pub(in crate::renderer) texture_key: Option<Arc<str>>,
    pub(in crate::renderer) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::renderer) wireframe_class: DynamicWireframeClass,
    pub(in crate::renderer) authored_glow: bool,

}

/// Dynamic indexed-mesh path used by cgame entities. CPU-skinned surfaces use
/// the legacy transient stream; GPU Ghoul2 surfaces keep static bind-pose meshes
/// cached here and upload only final CPU-evaluated bones plus per-draw transforms.
/// The renderer still knows nothing about demo/network provenance or JKA state.
pub(in crate::renderer) struct DynamicModelRenderer {
    pub(in crate::renderer) rt_raw_color_buffer: Option<wgpu::Buffer>,
    pub(in crate::renderer) scratch_raw_colors: Vec<[f32; 4]>,
    pub(in crate::renderer) texture_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) camera_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) entity_lighting_buffer: wgpu::Buffer,
    pub(in crate::renderer) entity_ambient_lighting: EntityAmbientLightingMode,
    /// Set by the renderer each frame before `prepare`; None keeps the stock
    /// lightgrid sample untouched.
    pub(in crate::renderer) entity_sun_relight: Option<EntitySunRelight>,
    /// Runtime dynamic lights folded into classic entity lighting, set by the
    /// renderer each frame before `prepare`. Empty while dynamic lights are off.
    pub(in crate::renderer) entity_dlights: Vec<EntityDlight>,
    /// The RT model receivers light this frame's RT-prepared models per pixel,
    /// so `prepare_rt_skinned` leaves `entity_dlights` out rather than add them twice.
    pub(in crate::renderer) entity_dlights_rt_per_pixel: bool,
    pub(in crate::renderer) model_brightness: f32,
    pub(in crate::renderer) entity_legacy_fog: EntityLegacyFog,
    pub(in crate::renderer) entity_weather: EntityWeather,
    pub(in crate::renderer) entity_water: Arc<std::collections::HashMap<u16, crate::water_wetness::WaterProfile>>,
    /// Last value written to the uniform's `cloud_shadow_sun`; w = 0 when off.
    pub(in crate::renderer) cloud_shadow_sun: [f32; 4],
    /// Whether the colour pipelines were built with ENABLE_LEGACY_FOG.
    pub(in crate::renderer) legacy_fog_compiled: bool,
    /// ENABLE_MODEL_WET in the model pipelines (see `set_model_wet`).
    pub(in crate::renderer) model_wet_compiled: bool,
    pub(in crate::renderer) pipeline_layout: wgpu::PipelineLayout,
    /// Group 2 = shared opaque-scene snapshot; used only by Glass pipelines.
    pub(in crate::renderer) glass_pipeline_layout: wgpu::PipelineLayout,
    /// Group 3 = shared opaque-scene snapshot; used only by Glass pipelines.
    pub(in crate::renderer) glass_skin_pipeline_layout: wgpu::PipelineLayout,
    pub(in crate::renderer) shader: wgpu::ShaderModule,
    pub(in crate::renderer) wireframe_pipeline: Option<wgpu::RenderPipeline>,
    /// Lazy per-alpha-mode pipelines; built only for modes with draws.
    pub(in crate::renderer) model_pipelines:
        [Option<wgpu::RenderPipeline>; DynamicModelAlphaMode::COUNT],
    /// Capture variants keep material coverage separate from additive light.
    pub(in crate::renderer) studio_capture_pipelines:
        [Option<wgpu::RenderPipeline>; DynamicModelAlphaMode::COUNT],
    /// Lazy authored-glow replay variants. They preserve each stage's normal
    /// blend/depth/material behavior but force the OpenJK glow-pass black fog.
    pub(in crate::renderer) model_glow_pipelines:
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
    pub(in crate::renderer) fx_sprite_glow_pipelines:
        [Option<wgpu::RenderPipeline>; DynamicModelAlphaMode::COUNT],
    pub(in crate::renderer) fx_zero_alpha_discard: bool,
    pub(in crate::renderer) skin_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) skin_pipeline_layout: wgpu::PipelineLayout,
    pub(in crate::renderer) skin_shader: wgpu::ShaderModule,
    pub(in crate::renderer) skin_wireframe_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) skin_pipelines:
        [Option<wgpu::RenderPipeline>; DynamicModelAlphaMode::COUNT],
    pub(in crate::renderer) skin_glow_pipelines:
        [Option<wgpu::RenderPipeline>; DynamicModelAlphaMode::COUNT],
    /// `ENABLE_SOURCE_MATERIAL` variants of the two lit colour pipelines; see
    /// `ghoul2_skin.wgsl`. Only Opaque/Mask are ever compiled, and only once a
    /// Source 1 character is actually on screen, so a stock session never pays
    /// for them.
    pub(in crate::renderer) skin_source_pipelines:
        [Option<wgpu::RenderPipeline>; DynamicModelAlphaMode::COUNT],
    /// Bit per `DynamicModelAlphaMode::index` with a prepared Ghoul2 draw this frame.
    pub(in crate::renderer) skin_alpha_mask: u16,
    /// Subset of `skin_alpha_mask` whose draws carry a Source 1 VMT material.
    /// Zero in a stock session, which keeps the draw loop byte-identical.
    pub(in crate::renderer) skin_source_mask: u16,
    // Depth-writing entities in the single-sample scene depth prepass. These
    // target the fixed prepass formats, so they never depend on MSAA/HDR.
    pub(in crate::renderer) prepass_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) prepass_mask_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) skin_prepass_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) skin_prepass_mask_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) rain_prepass_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) rain_prepass_mask_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) skin_rain_prepass_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) skin_rain_prepass_mask_pipeline: wgpu::RenderPipeline,
    /// Depth-only light-space variants for entity shadow casters, in the order
    /// [md3 opaque, md3 mask, skin opaque, skin mask]. Index 0 is forward-Z (Entity
    /// map and legacy CSM), 1 is reverse-Z (Bevy CSM). Each is built lazily the
    /// first time it draws, so modes that never need it never compile it.
    pub(in crate::renderer) entity_shadow_pipelines: [Option<[wgpu::RenderPipeline; 4]>; 2],
    pub(in crate::renderer) repeat_sampler: wgpu::Sampler,
    pub(in crate::renderer) clamp_sampler: wgpu::Sampler,
    /// 1x1 linear (128, 128, 255): tangent space's own +Z. Bound as the normal
    /// map wherever a surface has none, so the bind group always satisfies the
    /// layout and the perturbed normal collapses to the geometric one.
    pub(in crate::renderer) flat_normal: GpuImage,
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
    /// Allocated only after the first eligible resident particle is submitted.
    pub(in crate::renderer) resident_fx: Option<super::resident_fx::ResidentFxBackend>,
    pub(in crate::renderer) resident_fx_frame: Option<Arc<crate::renderer::FxGpuResidentFrame>>,
    pub(in crate::renderer) resident_fx_requested_modes: u16,
    pub(in crate::renderer) resident_fx_draws: Vec<super::resident_fx::PreparedResidentFxDraw>,
    pub(in crate::renderer) resident_fx_slots: Vec<u32>,
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
    /// Shared opaque-scene snapshot for the Glass model variants.
    pub(in crate::renderer) glass_bind_group: Option<wgpu::BindGroup>,
    pub(in crate::renderer) skin_draws: Vec<Ghoul2PreparedDraw>,
    /// Entity number for each entry in `skin_draw_buffer`, preserving identity even
    /// when Ghoul2 surfaces are instanced across multiple players.
    pub(in crate::renderer) skin_instance_entities: Vec<u16>,
    pub(in crate::renderer) ghoul2_batch_draws: Ghoul2BatchMode,
    pub(in crate::renderer) model_state_cache: bool,
    pub(in crate::renderer) dynamic_batch_draws: bool,
    pub(in crate::renderer) model_prepare_threads: u32,
    pub(in crate::renderer) cpu_pack_pool: Option<rayon::ThreadPool>,
    pub(in crate::renderer) scratch_cpu_pack_jobs: Vec<super::prepare::CpuPackJob>,
    pub(in crate::renderer) model_render_bundles: bool,
    pub(in crate::renderer) model_bundles: super::bundles::ModelBundles,
    pub(in crate::renderer) perf_bundles: super::bundles::BundlePerf,
    pub(in crate::renderer) md3_geometry_cache: bool,
    pub(in crate::renderer) md3_stream_cache: super::md3_cache::Md3StreamCache,
    pub(in crate::renderer) model_index_upload_cache: bool,
    pub(in crate::renderer) index_upload_cache: super::prepare::IndexUploadCache,
    pub(in crate::renderer) ghoul2_batch_key_cache: bool,
    pub(in crate::renderer) next_texture_binding_id: u64,
    pub(in crate::renderer) next_ghoul2_group_id: u64,
    pub(in crate::renderer) capture_prepare: bool,
    pub(in crate::renderer) perf_prepare: super::prepare::PreparePerf,
    pub(in crate::renderer) perf_phase_calls: std::cell::Cell<super::submission::ModelPhaseCalls>,
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
    /// Reuse adaptive group counts across frames (no per-frame hashmap allocation).
    pub(in crate::renderer) scratch_batch_counts: HashMap<(u64, u32), u32>,
    /// Linear-time cached-group path; these retain allocations across frames.
    pub(in crate::renderer) scratch_group_lookup: HashMap<(u64, u32), usize>,
    pub(in crate::renderer) scratch_frame_groups: Vec<super::prepare::Ghoul2FrameGroup>,
    pub(in crate::renderer) scratch_group_sort: Vec<usize>,
    pub(in crate::renderer) scratch_group_order: Vec<usize>,
    /// Lazily created only while RT Shadows is active. RT-off keeps the original
    /// Ghoul2 vertex-shader skinning path and never allocates/dispatches this.
    pub(in crate::renderer) rt_skinning: Option<Ghoul2RtSkinningResources>,
    pub(in crate::renderer) rt_prepared: bool,
    // Per-frame (colour map, normal map) pointer pair -> stable key cache.
    // TextureData is Arc-backed and immutable during prepare, so identical
    // surfaces can avoid rebuilding the same formatted texture cache key
    // hundreds of times in crowd scenes. Cleared every frame so allocator
    // pointer reuse can never alias an older TextureData.
    pub(in crate::renderer) frame_texture_keys: HashMap<(usize, usize), Arc<str>>,
}
