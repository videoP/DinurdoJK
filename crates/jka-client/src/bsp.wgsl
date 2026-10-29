// TaystJK RB_CalcStretchTexCoords: reciprocal waveform scaled about (0.5, 0.5).
// Only evaluated for kind 6 (tcMod stretch).
fn tc_stretch_wave(kind: u32, phase: f32) -> f32 {
    let x = fract(phase);
    if (kind == 0u) { return sin(x * 6.28318530718); }
    if (kind == 1u) { return select(select(2.0 - 4.0*x, 4.0*x - 4.0, x >= 0.75), 4.0*x, x < 0.25); }
    if (kind == 2u) { return select(-1.0, 1.0, x < 0.5); }
    if (kind == 3u) { return x; }
    if (kind == 4u) { return 1.0 - x; }
    // Stable noise approximation for GF_NOISE, where the reference uses a LUT.
    let n0 = floor(phase);
    let t = fract(phase);
    let a = fract(sin(n0 * 127.1) * 43758.5453) * 2.0 - 1.0;
    let b = fract(sin((n0 + 1.0) * 127.1) * 43758.5453) * 2.0 - 1.0;
    return mix(a, b, t*t*(3.0 - 2.0*t));
}

struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
    unjittered_view_proj: mat4x4<f32>,
    previous_unjittered_view_proj: mat4x4<f32>,
    jump_shade: vec4<f32>,
    foliage_wind: vec4<f32>,
};
struct Material {
    header: vec4<u32>,
    vector_s: vec4<f32>,
    vector_t: vec4<f32>,
    mods: array<vec4<f32>, 8>,
    color: vec4<f32>,
    params: vec4<f32>,
    pbr_params0: vec4<f32>, // xy normalScale, z fixed roughness (-1 = none), w cached roughness hint
    pbr_params1: vec4<f32>, // xyz fixed dielectric reflectance, w authored flag
    reflection_probe: vec4<f32>,
    planar_plane: vec4<f32>,
    wave_rgb: vec4<f32>,
    wave_alpha: vec4<f32>,
    wave_funcs: vec4<u32>,
    ocean_color: vec4<f32>, // linear authored texture tint, opacity; negative alpha = default
    ocean_material: vec4<f32>, // xyz reflectance, w roughness; negative = default
    ocean_waves: vec4<f32>, // xy reserved/amplitude hint, z/w authored blend factors
    ocean_blend_color: vec4<f32>, // analyzed mean color/brightness; never a texture lookup
};
struct PlanarReflectionSettings {
    planes: array<vec4<f32>, 4>,
    // xy: main viewport, z: active planar slot count, w: tcGen environment promotion enabled.
    viewport: vec4<f32>,
    // x: 0 normal, 1 candidates, 2 selected plane, 3 texture preview, 4 raw applied sample, 5 binding test.
    debug: vec4<f32>,
};
struct PointLight {
    position_radius: vec4<f32>,
    color_intensity: vec4<f32>,
    emitter: vec4<f32>, // xyz normal/direction; w: -1 segment, 0 point, 1/2 area, 3 spot
    shape: vec4<f32>,   // spot: x inner cone cos, y outer cone cos
    shadow: vec4<f32>,
};
struct ClusterRecord {
    count: u32,
    indices: array<u32, 32>,
};
struct LightingSettings {
    values: vec4<u32>, // enabled, light count, viewport width, viewport height
    local_shadows: vec4<u32>, // enabled, shadowed count, cubemap size, reserved
    feature_flags: vec4<u32>, // emissive area lights, voxel/probe GI, point/entity lights, reflection quality
    map_ambient: vec4<f32>, // xyz q3map2 ambient RGB; w RT sample count
    map_minlight: vec4<f32>, // source-.map q3map2 minlight RGB; w runtime-dlight falloff mode (0 stock, 1 physical)
    baked_sun_direction_strength: vec4<f32>, // xyz toward old sun, w removal strength
    baked_sun_radiance: vec4<f32>, // RGB old sun intensity / 255
    original_sun_shadow_view_proj: mat4x4<f32>,
    original_sun_shadow_params: vec4<f32>, // x requested, y shadow texel size, z cache ready
    baked_indirect_adjust: vec4<f32>, // x residual exposure, y residual saturation, z shadow PCF softness, w removal budget
    baked_indirect_curve: vec4<f32>, // x gamma-like power in linear light, y highlight retention, zw reserved
    baked_sun_debug_mode: vec4<u32>, // x: 0 off, 1 removal amount, 2 old-sun visibility
    runtime_sun_direction: vec4<f32>,
    runtime_sun_radiance: vec4<f32>, // xyz radiance, w live BSP sunlight enabled
};
struct PbrSettings {
    // x: PBR companions enabled, y: parallax enabled, z: height scale * 1000.
    values: vec4<u32>,
    // x: directional baked lighting enabled, y: deluxe specular scale,
    // z: detail distance fade enabled, w: detail fade distance in JKA units.
    deluxe: vec4<f32>,
};
struct StaticLightGridSettings {
    origin_enabled: vec4<f32>,
    inv_size: vec4<f32>,
    bounds: vec4<f32>,
};
struct IrradianceVolumeSettings {
    // Bevy LightProbe model: world -> unit 1x1x1 probe cube centered at origin.
    light_from_world: mat4x4<f32>,
    // xyz: per-axis falloff ratio, w: irradiance-volume intensity.
    falloff_intensity: vec4<f32>,
    // x: volume present/enabled. Remaining components are padding.
    volume_enabled_pad: vec4<f32>,
};
struct VoxelProbeGiSettings {
    origin_enabled: vec4<f32>,
    inv_cell: vec4<f32>,
    bounds_strength: vec4<f32>,
};
struct StaticPbrLighting {
    diffuse_factor: vec3<f32>,
    specular: vec3<f32>,
};
struct ShadowSettings {
    view_proj: array<mat4x4<f32>, 4>,
    split_depths: vec4<f32>,
    light_direction_enabled: vec4<f32>,
    params: vec4<f32>, // map size, legacy base bias, shadow strength, 1 = Bevy CSM
    camera_forward: vec4<f32>,
    cascade_texel_sizes: vec4<f32>,
    bevy_params: vec4<f32>, // depth bias, normal bias, overlap proportion, cascade count
    dynamic_coverage: array<vec4<u32>, 4>,
};
struct SurfaceFogSettings {
    color_depth: vec4<f32>,
    flags: vec4<f32>, // global, color override, Legacy2 in-stage safe, Legacy1 post eligible
};
struct LegacyFogControl {
    values: vec4<f32>, // enabled, strength/scale, map has authored fog, r_drawfog mode
};
struct OceanRenderSettings {
    map_scales: array<vec4<f32>, 3>, // xy inverse tile length, z displacement, w normal scale
    water_color: vec4<f32>,
    foam_color: vec4<f32>,
    ocean_info: vec4<f32>, // enabled, map size, JKA units per meter, windrow domain metres
    surface: vec4<f32>, // roughness, normal strength, sea spray enabled, wind foam streaks enabled
    clipmap: vec4<f32>, // centre x, centre z, coarsest cell size, active windrow layer
    sun_direction_intensity: vec4<f32>, // xyz light travel direction, w relative q3map intensity
    sun_color: vec4<f32>,
    sky_ambient: vec4<f32>, // rgb linear map-sky average, w real skybox present
    spray_info: vec4<f32>, // surface count, total particles, visible lifetime, lifetime randomness
    spray_bounds: array<vec4<f32>, 8>,
    spray_planes: array<vec4<f32>, 8>,
    swell: vec4<f32>,
    swell_motion: vec4<f32>,
    wind_motion: vec4<f32>,
    authored_bounds: vec4<f32>,
    authored_plane: vec4<f32>,
    // Outline of each promoted surface (see OceanMasks): per slot, first
    // triangle and triangle count; per triangle, two vec4s a.xy b.xy / c.xy.
    mask_ranges: array<vec4<f32>, 8>,
    mask_bounds: array<vec4<f32>, 8>,
    mask_tiles: array<vec4<u32>, 8>,
    mask_triangles: array<vec4<f32>, 512>,
    // Extra per-pool rendering data, NOT part of the coastal or spray inputs.
    sky_segments: array<vec4<f32>, 16>,
    appearance_a: vec4<f32>, // detail mode, foam gates, foam gain, promoted seam flatten (0/1)
    appearance_b: vec4<f32>, // body color mode, foam brightness, sun gain, foam debug
    appearance_c: vec4<f32>, // x visual fixes; y dynamic LOD ready; z mesh spacing; w analytic far radius
    hybrid_eye: vec4<f32>, // x Godot-mesh lift near the camera (metres); y active
    // Submerged Hybrid: Blue Water's own clipmap through `vs_ocean_native`.
    // Its upstream `Surface` prefix: per level xy snapped centre (m), z cell (m), w 1 / cell.
    native_frames: array<vec4<f32>, 10>,
    native_clipmap: vec4<f32>, // segments, levels, clipmap centre x, z (m)
    native_detail: vec4<f32>, // finest level drawn, its morph, plane height (units), mask slot
    // The outline's 2-bit 32x32 tile classification (0 boundary, 1 outside, 2 inside), 16 vec4 per slot.
    mask_fine_tiles: array<vec4<u32>, 128>,
};
struct SkyFogRegion {
    bounds: vec4<f32>,
    plane: vec4<f32>,
    segments: array<vec4<f32>, 16>,
};
struct OceanOptics {
    inverse_view_projection: mat4x4<f32>, eye: vec4<f32>, forward: vec4<f32>,
    fog: vec4<f32>, params: vec4<f32>, minimum: array<vec4<f32>,8>, maximum: array<vec4<f32>,8>,
    sky_regions: array<SkyFogRegion, 8>,
    sky_info: vec4<f32>,
};
@group(5) @binding(0) var ocean_scene: texture_2d<f32>;
@group(5) @binding(1) var ocean_scene_sampler: sampler;
@group(5) @binding(2) var ocean_scene_depth: texture_2d<f32>;
@group(5) @binding(3) var<uniform> ocean_optics: OceanOptics;
const CLUSTER_X: u32 = 16u;
const CLUSTER_Y: u32 = 9u;
const CLUSTER_Z: u32 = 24u;
const CLUSTER_NEAR: f32 = 1.0;
const CLUSTER_FAR: f32 = 65536.0;
const LOCAL_SHADOW_NEAR: f32 = 4.0;

const CLASSIC_FULLBRIGHT: u32 = 1u;
const CLASSIC_VERTEX_LIGHT: u32 = 2u;
const CLASSIC_LIGHTMAP_ONLY: u32 = 4u;
const MATERIAL_EXPLICIT_LIGHTMAP: u32 = 8388608u;
const MATERIAL_DLIGHT_IN_LIGHTMAP_STAGE: u32 = 65536u; // header.w
const MATERIAL_HAS_LIGHTMAP: u32 = 16777216u;
const MATERIAL_OPAQUE_STAGE: u32 = 33554432u;

// Pipeline-specialized feature switches. These are WebGPU/WGSL override
// constants, not runtime uniforms: the renderer supplies them when a pipeline
// variant is created, allowing the driver to dead-strip disabled systems.
override ENABLE_PBR: bool = false;
override ENABLE_POM: bool = false;
override ENABLE_POINT_LIGHTS: bool = false;
override ENABLE_MAP_LIGHT_SIMULATION: bool = false;
override ENABLE_SOURCE_MAP_WORLD: bool = false;
override ENABLE_LEGACY_DLIGHTS: bool = false;
override ENABLE_VERTEX_DLIGHTS: bool = false;
override ENABLE_CLUSTERED_LITE_DLIGHTS: bool = false;
override ENABLE_AREA_LIGHTS: bool = false;
override ENABLE_VOXEL_GI: bool = false;
override ENABLE_IRRADIANCE_VOLUME: bool = false;
override ENABLE_LOCAL_SHADOWS: bool = false;
override ENABLE_CASCADED_SHADOWS: bool = false;
override ENABLE_PLANAR_REFLECTIONS: bool = false;
override ENABLE_OCEAN: bool = false;
// Legacy fog pipeline variant (WorldShaderVariantKey::legacy_fog).
override ENABLE_LEGACY_FOG: bool = false;
// jaPRO SP-physics jump-height helper variant (WorldShaderVariantKey::jump_shade).
override ENABLE_JUMP_SHADE: bool = false;
// Dev/debug render state that is off in almost every frame. Each one is decided on
// the cold path from the settings (see WorldShaderVariantKey) instead of being read
// from a uniform per fragment, which costs registers and ALU even while it is off.
// They default to on so a pipeline built without the constant keeps full behavior.
override ENABLE_CLASSIC_RENDER_FLAGS: bool = true; // r_fullbright / r_vertexLight / r_lightmap
override ENABLE_BAKED_SUN_RELIGHT: bool = false;
override ENABLE_STATIC_BSP_AO: bool = true; // cached BSP ambient occlusion
override ENABLE_DELUXE: bool = true; // directional baked lighting (deluxemap / lightgrid) on PBR surfaces
override ENABLE_DELUXE_SPECULAR: bool = true; // GGX lobe from the baked light direction
override ENABLE_PLANAR_DEBUG: bool = true; // r_planarDebug views
override ENABLE_DETAIL_TEXTURES: bool = false;
override ENABLE_DYNAMIC_LIGHT_DEBUG: bool = true; // r_dynamicLightDebug receiver view
// Sky cost probe and the experimental fractal sky (sky_debug key): together they
// were most of the sky program (469 -> 125 DXIL instructions without them).
override ENABLE_SKY_DEBUG: bool = true;
// Snow shells, Snowflow deformation and stock footprint stamps: only compiled in
// while footprints are on and the map has snow surfaces (surface_deformation key).
override ENABLE_SURFACE_DEFORMATION: bool = true;
// Per pipeline, not per variant key: off for DrawClass::Opaque so plain walls carry
// no discard and keep early depth rejection; on for Mask/Transparent/Sky.
override ENABLE_ALPHA_TEST: bool = true;
// Per-material specialization (r_fragmentShaderMode 1, MaterialFragmentKey). Header
// bits inside a *_MASK are compile-time constants for that pipeline, so the material
// checks on them fold away; bits outside it are still read from the uniform. Every
// material drawn with such a pipeline has exactly those bits, so the program stays
// exact. The defaults leave every check dynamic (the general program).
override MATERIAL_KEY_FLAGS: u32 = 0u;
override MATERIAL_KEY_FLAGS_MASK: u32 = 0u;
override MATERIAL_KEY_FLAGS_W: u32 = 0u;
override MATERIAL_KEY_FLAGS_W_MASK: u32 = 0u;
// header.x (tcGen mode); 0xffffffff reads it from the uniform.
override MATERIAL_KEY_TC_GEN: u32 = 4294967295u;
// 0: runtime alpha function, 1: none, 2: discard below params.x, 3: discard at/above -params.x.
override MATERIAL_KEY_ALPHA_FUNC: u32 = 0u;
fn material_flags() -> u32 {
    return (material.header.z & ~MATERIAL_KEY_FLAGS_MASK) | (MATERIAL_KEY_FLAGS & MATERIAL_KEY_FLAGS_MASK);
}
fn material_flags_w() -> u32 {
    return (material.header.w & ~MATERIAL_KEY_FLAGS_W_MASK) | (MATERIAL_KEY_FLAGS_W & MATERIAL_KEY_FLAGS_W_MASK);
}
fn material_tc_gen() -> u32 {
    if (MATERIAL_KEY_TC_GEN != 4294967295u) {
        return MATERIAL_KEY_TC_GEN;
    }
    return material.header.x;
}
fn material_alpha_rejects(alpha: f32) -> bool {
    let cutoff = material.params.x;
    if (MATERIAL_KEY_ALPHA_FUNC == 1u) {
        return false;
    }
    if (MATERIAL_KEY_ALPHA_FUNC == 2u) {
        return alpha < cutoff;
    }
    if (MATERIAL_KEY_ALPHA_FUNC == 3u) {
        return alpha >= -cutoff;
    }
    return (cutoff > 0.0 && alpha < cutoff) || (cutoff < 0.0 && alpha >= -cutoff);
}
// PBR optimization switches. These remain pipeline-specialized so each option
// can be benchmarked independently without paying a runtime branch in the hot path.
override PBR_SHARED_MATERIAL_EVAL: bool = true;
override PBR_SHARED_TANGENT_FRAME: bool = true;
override POM_MIP_AWARE: bool = true;
override POM_ADAPTIVE_STEPS: bool = false;
override PBR_COMPANION_SAMPLER: bool = false;
override PBR_VERTEX_LIGHTGRID: bool = false;
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(34) var baked_local_texture: texture_2d_array<f32>;
@group(1) @binding(2) var lightmap_texture: texture_2d<f32>;
@group(1) @binding(3) var lightmap_sampler: sampler;
@group(1) @binding(4) var<uniform> material: Material;
@group(1) @binding(5) var sky_rt: texture_2d<f32>;
@group(1) @binding(6) var sky_bk: texture_2d<f32>;
@group(1) @binding(7) var sky_lf: texture_2d<f32>;
@group(1) @binding(8) var sky_ft: texture_2d<f32>;
@group(1) @binding(9) var sky_up: texture_2d<f32>;
@group(1) @binding(10) var sky_dn: texture_2d<f32>;
@group(1) @binding(11) var sky_sampler: sampler;
// Fused additive siblings share the base draw geometry, but NOT material state.
// These are the same MaterialUniform blocks as their original separate passes.
@group(1) @binding(32) var<uniform> additive_stage1: Material;
@group(1) @binding(33) var<uniform> additive_stage2: Material;
@group(1) @binding(24) var additive_clamp_sampler: sampler;


@group(1) @binding(12) var normal_texture: texture_2d<f32>;
@group(1) @binding(13) var roughness_texture: texture_2d<f32>;
@group(1) @binding(14) var height_texture: texture_2d<f32>;
@group(1) @binding(15) var<uniform> surface_fog: SurfaceFogSettings;
@group(1) @binding(16) var metallic_texture: texture_2d<f32>;
@group(1) @binding(17) var specular_texture: texture_2d<f32>;
@group(1) @binding(18) var emissive_texture: texture_2d<f32>;
@group(1) @binding(19) var reflection_probe_texture: texture_cube<f32>;
@group(1) @binding(20) var reflection_probe_sampler: sampler;
@group(1) @binding(28) var pbr_sampler: sampler;
@group(1) @binding(29) var deluxemap_texture: texture_2d<f32>;
@group(1) @binding(30) var detail_texture: texture_2d<f32>;
@group(1) @binding(31) var detail_sampler: sampler;
@group(2) @binding(0) var<storage, read> dynamic_lights: array<PointLight>;
@group(2) @binding(1) var<storage, read> light_clusters: array<ClusterRecord>;
@group(2) @binding(2) var<uniform> lighting_settings: LightingSettings;
@group(2) @binding(3) var<uniform> pbr_settings: PbrSettings;
@group(2) @binding(4) var local_shadow_texture: texture_depth_cube_array;
@group(2) @binding(5) var local_shadow_sampler: sampler_comparison;
@group(2) @binding(6) var static_lightgrid_direction: texture_3d<f32>;
@group(2) @binding(7) var static_lightgrid_lighting: texture_3d<f32>;
@group(2) @binding(8) var static_lightgrid_sampler: sampler;
@group(2) @binding(9) var<uniform> static_lightgrid: StaticLightGridSettings;
@group(2) @binding(10) var voxel_gi_point: texture_3d<f32>;
@group(2) @binding(11) var voxel_gi_area: texture_3d<f32>;
@group(2) @binding(12) var<uniform> voxel_gi: VoxelProbeGiSettings;
@group(2) @binding(13) var irradiance_volume: texture_3d<f32>;
@group(2) @binding(14) var<uniform> irradiance_volume_info: IrradianceVolumeSettings;
@group(2) @binding(16) var<storage, read> legacy_dlight_surface_masks: array<u32>;
@group(3) @binding(0) var shadow_texture: texture_depth_2d_array;
@group(3) @binding(1) var shadow_sampler: sampler_comparison;
@group(3) @binding(2) var<uniform> shadow_settings: ShadowSettings;
@group(3) @binding(3) var<uniform> legacy_fog: LegacyFogControl;
@group(3) @binding(4) var weather_occlusion_height: texture_2d<f32>;
@group(3) @binding(5) var<uniform> weather_surface: WeatherSurfaceSettings;
@group(3) @binding(6) var weather_occlusion_sampler: sampler;
@group(3) @binding(7) var sky_admission_texture: texture_depth_2d_array;
@group(4) @binding(0) var planar_reflection_texture: texture_2d_array<f32>;
@group(4) @binding(1) var planar_reflection_sampler: sampler;
@group(4) @binding(2) var<uniform> planar_reflection: PlanarReflectionSettings;
@group(6) @binding(0) var ocean_displacement: texture_2d_array<f32>;
// Spatial mesh-LOD caches (not the 3 FFT frequency cascades).
@group(6) @binding(12) var ocean_displacement_medium: texture_2d_array<f32>;
@group(6) @binding(13) var ocean_displacement_far: texture_2d_array<f32>;
@group(6) @binding(1) var ocean_normal: texture_2d_array<f32>;
@group(6) @binding(2) var ocean_sampler: sampler;
@group(6) @binding(3) var<uniform> ocean: OceanRenderSettings;
@group(6) @binding(5) var<storage, read> ocean_windrow: array<f32>;
@group(6) @binding(14) var<storage, read> ocean_windrow_medium: array<f32>;
@group(6) @binding(15) var<storage, read> ocean_windrow_far: array<f32>;
struct OceanInteractionDomain {
    region: vec4<f32>, bounds: vec4<f32>, controls: vec4<f32>,
    identity: vec4<f32>,
};
struct OceanInteractionDomains { domains: array<OceanInteractionDomain, 4> };
@group(6) @binding(6) var ocean_interaction_field: texture_2d_array<f32>;
@group(6) @binding(7) var<uniform> ocean_interaction: OceanInteractionDomains;
struct CoastalParams { grid:vec4<f32>, control:vec4<u32>, limits:vec4<f32> };
struct CoastalPresentation { previous:vec4<f32>, current:vec4<f32>, slopes:vec4<f32>, accounting:vec4<f32> };
struct CoastalSurface { failed:u32, pad0:u32, pad1:u32, pad2:u32, cells:array<CoastalPresentation> };
struct CoastalSample { field:vec4<f32>, slope:vec2<f32> };
@group(6) @binding(9) var<uniform> coastal_params:CoastalParams;
@group(6) @binding(10) var<storage,read> coastal_lookup:array<vec4<i32>>;
@group(6) @binding(11) var<storage,read> coastal_surface:CoastalSurface;
fn coastal_hash(coordinate:vec2<i32>)->u32 {
    return (u32(coordinate.x)*73856093u)^(u32(coordinate.y)*19349663u);
}
fn coastal_cell(coordinate:vec2<i32>)->u32 {
    let tile=vec2<i32>(floor(vec2<f32>(coordinate)/16.0));
    let local=coordinate-tile*16;
    var index=coastal_hash(tile)&coastal_params.control.y;
    for (var probe=0u;probe<=coastal_params.control.y;probe++) {
        let record=coastal_lookup[index];
        if (record.w==0) {return 0xffffffffu;}
        if (all(record.xy==tile)) {
            let address=u32(record.z+local.y*16+local.x);
            if (coastal_params.control.w!=0u) {return bitcast<u32>(coastal_lookup[address/4u][address%4u]);}
            return address;
        }
        index=(index+1u)&coastal_params.control.y;
    }
    return 0xffffffffu;
}
fn coastal_open_at(coordinate:vec2<i32>,axis:u32)->bool {
    let k=coastal_cell(coordinate);if (k==0xffffffffu) {return false;}
    return (u32(coastal_surface.cells[k].accounting.w)&(1u<<axis))!=0u;
}
fn coastal_connected(a:vec2<i32>,b:vec2<i32>)->bool {
    let delta=b-a;if (all(delta==vec2<i32>(0))) {return true;}
    let x=select(0u,1u,delta.x>0);let y=select(2u,3u,delta.y>0);
    if (delta.x==0) {return coastal_open_at(a,y);}
    if (delta.y==0) {return coastal_open_at(a,x);}
    return (coastal_open_at(a,x)&&coastal_open_at(vec2<i32>(b.x,a.y),y))
        ||(coastal_open_at(a,y)&&coastal_open_at(vec2<i32>(a.x,b.y),x));
}
// Sky-promoted ocean uses the same FFT geometry and shader, but exterior
// pixels have no authored seabed or scene behind them. This flag exists only
// in the promoted per-pool render uniform (1 = original, 2 = extended).
// The original shoreline/outline and the separate coastal solver are never
// expanded. Checking the original water AABB is enough here because the
// precise sky-contact admission runs separately in ocean_mask_contains.
fn ocean_sky_exterior(position: vec2<f32>) -> bool {
    return ocean.authored_plane.w > 1.5
        && (any(position < ocean.authored_bounds.xy)
            || any(position > ocean.authored_bounds.zw));
}

fn sample_coastal(position:vec3<f32>)->CoastalSample {
    var result:CoastalSample;
    result.field=vec4<f32>(0.0);result.slope=vec2<f32>(0.0);
    // Scope by stable slot and still-water plane so stacked pools cannot alias.
    if (ocean_sky_exterior(position.xz) || coastal_params.control.x==0u
        || coastal_surface.failed!=0u || ocean.authored_plane.w<0.5
        || abs(ocean.spray_planes[0].z-coastal_params.grid.w)>0.1
        || abs(position.y-coastal_params.grid.z)>0.5) {return result;}
    let cell=position.xz*vec2<f32>(1.0,-1.0)/coastal_params.grid.x-vec2<f32>(0.5);
    let base=vec2<i32>(floor(cell));let blend=fract(cell);
    let anchor=vec2<i32>(floor(cell+vec2<f32>(0.5)));
    let alpha=coastal_params.limits.x;
    var total=0.0;var retained=0.0;
    for (var corner=0u;corner<4u;corner++) {
        let offset=vec2<i32>(i32(corner&1u),i32(corner>>1u));
        if (!coastal_connected(anchor,base+offset)) {continue;}
        let k=coastal_cell(base+offset);if (k==0xffffffffu) {continue;}
        let record=coastal_surface.cells[k];
        let sample=mix(record.previous,record.current,alpha);
        let geometric_weight=select(1.0-blend.x,blend.x,offset.x==1)*select(1.0-blend.y,blend.y,offset.y==1);
        retained+=geometric_weight;
        let w=geometric_weight*sample.w;
        result.field=vec4<f32>(result.field.xyz+sample.xyz*w,result.field.w);
        result.slope+=mix(record.slopes.xy,record.slopes.zw,alpha)*w;
        total+=w;
    }
    if (total>0.00001) {result.field=vec4<f32>(result.field.xyz/total,result.field.w);result.slope/=total;}
    result.field.w=total/max(retained,0.00001);
    return result;
}

// Bind the persistent field to the actual draw's body, not to a compacted
// surface-list index. Identical XZ on different planes cannot share ripples.
fn sample_ocean_interaction(position: vec3<f32>) -> vec4<f32> {
    if (ocean_interaction.domains[0].controls.w < 0.5) { return vec4<f32>(0.0); }
    let body_bounds = select(ocean.authored_bounds, ocean.spray_bounds[0], ocean.authored_plane.w > 0.5);
    let body_id = select(0.0, ocean.spray_planes[0].z+1.0, ocean.authored_plane.w > 0.5);
    for (var i = 0u; i < 4u; i++) {
        let domain = ocean_interaction.domains[i];
        if (domain.controls.x < 0.5 || abs(domain.identity.x-body_id) > 0.1 || abs(domain.region.w-position.y) > 0.5
            || any(abs(domain.bounds-body_bounds) > vec4<f32>(0.1))) { continue; }
        let uv = (position.xz-domain.region.xy)*domain.region.z;
        if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) { return vec4<f32>(0.0); }
        let field = textureSampleLevel(ocean_interaction_field, ocean_sampler, uv, i32(i), 0.0);
        return vec4<f32>(field.xyz*domain.controls.z, domain.controls.y);
    }
    return vec4<f32>(0.0);
}

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lightmap_uv: vec2<f32>,
    @location(5) normal: vec3<f32>,
    @location(3) color: vec4<f32>,
    @location(4) static_ao: f32,
};
struct VertexOut {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lightmap_uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // xyz: sky direction, w: cached static BSP AO. Packing AO here avoids
    // allocating another interpolator slot.
    @location(3) sky_dir_ao: vec4<f32>,
    @location(4) world_position: vec3<f32>,
    @location(5) world_normal: vec3<f32>,
    @location(6) @interpolate(flat) shell_kind: u32,
    @location(7) shell_coverage: f32,
    @location(8) ocean_wave_height: f32,
    @location(9) ocean_uv_m: vec2<f32>,
    // 0 for ordinary geometry; for a clipmap vertex 2 + its mask slot, where
    // slot -1 (no mask) therefore encodes as 1. Flat so it stays exact.
    @location(10) @interpolate(flat) ocean_vertex: f32,
    // Optional vertex-sampled static lightgrid data. Disabled by default because
    // interpolation is an accuracy/performance tradeoff; the fragment path remains authoritative.
    @location(11) pbr_lightgrid_direction: vec4<f32>,
    @location(12) pbr_lightgrid_lighting: vec4<f32>,
    @location(13) vertex_dlight: vec3<f32>,
    @location(14) @interpolate(flat) legacy_dlight_bits: u32,
    @location(15) @interpolate(flat) baked_sun_fit: vec4<f32>,
};

// Fused additive stages only. Reuse locations unused by this shader family;
// ordinary world vertex/fragment entry points keep their original varyings.
// Affine tcMods commute with perspective-correct interpolation, so calculate
// each stage's coordinates per vertex as the original JKA stage path did.
struct AdditiveVertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) additive_uv1: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) additive_uv2: vec2<f32>,
    @location(4) world_position: vec3<f32>,
    @location(5) world_normal: vec3<f32>,
    @location(6) @interpolate(flat) shell_kind: u32,
    @location(7) shell_coverage: f32,
};


// Waveform generators (rgbGen wave / alphaGen wave). Same functions as id Tech 3's
// tables: value = base + func(phase + time * frequency) * amplitude.
fn wave_noise(t: f32) -> f32 {
    let i = floor(t);
    let u = fract(t);
    let s = u * u * (3.0 - 2.0 * u);
    let a = fract(sin(i * 127.1) * 43758.5453) * 2.0 - 1.0;
    let b = fract(sin((i + 1.0) * 127.1) * 43758.5453) * 2.0 - 1.0;
    return mix(a, b, s);
}

fn wave_value(func: u32, p: vec4<f32>, time: f32) -> f32 {
    let x = fract(p.z + time * p.w);
    var v = 0.0;
    if (func == 0u) {
        v = sin(x * 6.28318530718);
    } else if (func == 1u) {
        v = select(select(2.0 - 4.0 * x, 4.0 * x - 4.0, x >= 0.75), 4.0 * x, x < 0.25);
    } else if (func == 2u) {
        v = select(-1.0, 1.0, x < 0.5);
    } else if (func == 3u) {
        v = x;
    } else if (func == 4u) {
        v = 1.0 - x;
    } else {
        v = wave_noise((time + p.z) * p.w);
    }
    return p.x + v * p.y;
}

// JaPro: subtract the remap offset, then apply a positive clampTime.
// vector_t.w is reserved for the limit; zero leaves the clock unbounded.
fn shader_stage_time() -> f32 {
    let elapsed = camera.camera_pos_time.w - material.params.w;
    return select(elapsed, min(elapsed, material.vector_t.w), material.vector_t.w > 0.0);
}

fn apply_wave_gens(color: vec4<f32>) -> vec4<f32> {
    var out = color;
    let time = shader_stage_time();
    if ((material_flags() & 134217728u) != 0u) {
        let g = clamp(wave_value(material.wave_funcs.x, material.wave_rgb, time), 0.0, 1.0);
        out = vec4<f32>(vec3<f32>(g), out.a);
    }
    if ((material_flags() & 268435456u) != 0u) {
        out.a = clamp(wave_value(material.wave_funcs.y, material.wave_alpha, time), 0.0, 1.0);
    }
    return out;
}

// Stage tcMods (scroll/scale/rotate/transform/turb) applied to base coordinates.
// Shared by the per-vertex generated_uv and the per-pixel sky cloud layer.
// `world_pos` is the vertex position in this renderer's [x, z, -y] render
// space (JKA [x, y, z]); it only matters for tcMod turb (kind 5), which
// OpenJK's RB_CalcTurbulentTexCoords drives from (JKA x + JKA z) for S and
// JKA y for T, scaled by 1/128 * 0.125, NOT from the surface's own UV -
// using UV there aliases badly on any surface that tiles many times.
fn apply_tc_mods(base_uv: vec2<f32>, world_pos: vec3<f32>) -> vec2<f32> {
    var uv = base_uv;
    let time = shader_stage_time();
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (i >= material.header.y) {
            break;
        }
        let a = material.mods[i * 2u];
        let b = material.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            // TaystJK RB_CalcScrollTexCoords wraps the offset before later tcMods.
            // This matters when rotate follows scroll: a whole-texture offset
            // cannot be discarded after the rotation.
            let scroll = a.yz * time;
            uv = uv + scroll - floor(scroll);
        }
        if (kind == 2u) {
            uv = uv * a.yz;
        }
        if (kind == 3u) {
            // TaystJK RB_CalcRotateTexCoords negates degrees-per-second.
            let radians = -a.y * time * 0.017453292519943295;
            let c = cos(radians);
            let sn = sin(radians);
            let p = uv - vec2<f32>(0.5);
            uv = vec2<f32>(p.x * c - p.y * sn, p.x * sn + p.y * c) + vec2<f32>(0.5);
        }
        if (kind == 4u) {
            uv = vec2<f32>(
                uv.x * a.y + uv.y * a.z + a.w,
                uv.x * b.x + uv.y * b.y + b.z
            );
        }
        if (kind == 6u) {
            let value = a.y + a.z * tc_stretch_wave(u32(b.y + 0.5), select(a.w + time*b.x, (time+a.w)*b.x, u32(b.y + 0.5) == 5u));
            if (abs(value) > 1.0e-6) {
                uv = (uv - vec2<f32>(0.5)) / value + vec2<f32>(0.5);
            }
        }
        if (kind == 5u) {
            let now = a.w + time * b.x;
            let wave = vec2<f32>(
                sin((now + (world_pos.x + world_pos.y) * 0.0009765625) * 6.28318530718) * a.z,
                sin((now - world_pos.z * 0.0009765625) * 6.28318530718) * a.z
            );
            uv = uv + wave;
        }
    }
    return uv;
}

fn generated_uv(input: VertexIn) -> vec2<f32> {
    var uv = input.uv;
    if (material_tc_gen() == 1u) {
        uv = input.lightmap_uv;
    }
    if (material_tc_gen() == 2u) {
        uv = vec2<f32>(
            dot(input.position, material.vector_s.xyz),
            dot(input.position, material.vector_t.xyz)
        );
    }
    if (material_tc_gen() == 3u) {
        // Q3/JKA sphere-style environment mapping: view direction reflected by
        // the surface normal. Coordinates are in this renderer's transformed
        // [x,z,-y] space, so use render-space Y/Z for the two lookup axes.
        let n = normalize(input.normal);
        let view = normalize(camera.camera_pos_time.xyz - input.position);
        let reflected = reflect(-view, n);
        // OpenJK computes s from JKA Y and t from JKA Z. The renderer uses
        // [x,z,-y], therefore JKA Y = -render Z and JKA Z = render Y.
        uv = vec2<f32>(0.5 - reflected.z * 0.5, 0.5 - reflected.y * 0.5);
    }

    return apply_tc_mods(uv, input.position);
}

// Promoted water does not draw its authored brush face. It draws a clipmap
// built for that surface, and every vertex says so: real BSP vertices carry
// cached static AO here, which is never negative.
//
// Recognising the ocean from the geometry rather than from the material flag,
// the ocean uniform and the ENABLE_OCEAN override means nothing can disagree
// with the draw call that selected the clipmap in the first place.
fn is_ocean_vertex(input: VertexIn) -> bool {
    return input.static_ao < -0.5;
}

// Clipmap vertices carry a grid offset in position.xz, their surface's plane
// height in position.y, the footprint in uv/lightmap_uv, their own cell size in
// color.r and, on a level's outer boundary, the parent cell size in color.g.
//
// Snapping those boundary vertices onto the parent lattice makes the mismatched
// half of them land exactly on a coarse vertex, so the 2:1 LOD seam closes with
// degenerate triangles instead of needing transition fans.
fn ocean_clipmap_center(input: VertexIn) -> vec2<f32> {
    let promoted_dynamic = ocean.authored_plane.w > 0.5;
    let use_uniform_bounds = ocean.authored_plane.x > 0.0 || promoted_dynamic;
    let minimum = select(input.uv, ocean.authored_bounds.xy, use_uniform_bounds);
    let maximum = select(input.lightmap_uv, ocean.authored_bounds.zw, use_uniform_bounds);
    let static_step = select(max(ocean.clipmap.z,1.0),input.color.b,input.color.b > 0.0);
    let step = select(static_step,max(ocean.wind_motion.w,1.0),promoted_dynamic);
    let sky_enabled = promoted_dynamic && ocean.authored_plane.w > 1.5;
    let origin = select(clamp(ocean.clipmap.xy,minimum,maximum),ocean.clipmap.xy,sky_enabled);
    return round(origin / step) * step;
}

fn ocean_clipmap_position(input: VertexIn) -> vec3<f32> {
    let promoted_dynamic = ocean.authored_plane.w > 0.5;
    let use_uniform_bounds = ocean.authored_plane.x > 0.0 || promoted_dynamic;
    let minimum = select(input.uv, ocean.authored_bounds.xy, use_uniform_bounds);
    let maximum = select(input.lightmap_uv, ocean.authored_bounds.zw, use_uniform_bounds);
    let static_step = select(max(ocean.clipmap.z,1.0),input.color.b,input.color.b > 0.0);
    let step = select(static_step,max(ocean.wind_motion.w,1.0),promoted_dynamic);
    let sky_enabled = promoted_dynamic && ocean.authored_plane.w > 1.5;
    let centre = ocean_clipmap_center(input);

    var offset = input.position.xz;
    if (promoted_dynamic) {
        // Runtime promoted grids store their maximum per-level half-width in G.
        // The live half-width in authored_plane.y picks the actual outer seam.
        // Snap only that active seam onto the parent 2:1 lattice; interior
        // vertices remain untouched and the outermost active level has no parent.
        let spacing = max(input.color.r, 1.0e-4);
        let vertex_half = max(abs(input.color.g), 1.0);
        let requested_half = max(ocean.authored_plane.y, 1.0);
        let active_half = select(
            min(requested_half, vertex_half),
            min(max(32.0, requested_half * 0.5), vertex_half),
            input.color.g < 0.0
        );
        let cell = round(input.position.xz / spacing);
        let on_outer_edge = max(abs(cell.x), abs(cell.y)) >= active_half - 0.25;
        if (on_outer_edge && spacing < step - 0.25) {
            let parent = spacing * 2.0;
            offset = round(offset / parent) * parent;
        }
    } else {
        let snap = input.color.g;
        if (snap > 0.0) {
            offset = round(offset / snap) * snap;
        }
    }
    let unfolded = centre + offset;
    let folded = select(clamp(unfolded, minimum, maximum), unfolded, sky_enabled);
    // Promoted grids are shared by every pool of the map; the pool's own
    // promoted view uniform carries its plane height.
    let vertex_height = select(input.position.y, ocean.spray_planes[0].x, promoted_dynamic);
    return vec3<f32>(folded.x, select(vertex_height, ocean.authored_plane.y, ocean.authored_plane.x > 0.0), folded.y);
}

// Whether the undisplaced world xz point `p` lies on the surface's real
// outline. Triangles are grown by a fraction of a unit so shared edges between
// neighbouring pieces cannot open pinholes; only the silhouette matters.
fn ocean_mask_contains(slot: i32, p: vec2<f32>) -> bool {
    if (slot < 0 || slot >= 8) {
        return true;
    }
    let range = ocean.mask_ranges[slot];
    let count = u32(range.y);
    let bounds = ocean.mask_bounds[slot];
    if (range.z > 0.5) {
        // Sky continuation is admitted ONLY beyond the source water AABB.
        // Source pools retain their exact land/shoreline mask inside it.
        if (any(p < bounds.xy) || any(p > bounds.zw)) {
            for (var i = 0u; i < u32(range.z); i = i + 1u) {
                let segment = ocean.sky_segments[i];
                let side = u32(segment.w);
                if ((side == 1u && p.x <= segment.x + 2.0 && p.y >= segment.y && p.y <= segment.z)
                    || (side == 2u && p.x >= segment.x - 2.0 && p.y >= segment.y && p.y <= segment.z)
                    || (side == 3u && p.y <= segment.x + 2.0 && p.x >= segment.y && p.x <= segment.z)
                    || (side == 4u && p.y >= segment.x - 2.0 && p.x >= segment.y && p.x <= segment.z)) {
                    return true;
                }
            }
            // Fill an exterior quadrant only when BOTH sky-connected edges
            // meet at its original water corner; independent strips leave gaps.
            let xside = select(select(0u, 2u, p.x > bounds.z), 1u, p.x < bounds.x);
            let zside = select(select(0u, 4u, p.y > bounds.w), 3u, p.y < bounds.y);
            if (xside != 0u && zside != 0u) {
                let xc = select(bounds.z, bounds.x, xside == 1u);
                let zc = select(bounds.w, bounds.y, zside == 3u);
                var found_x = false;
                var found_z = false;
                for (var i = 0u; i < u32(range.z); i = i + 1u) {
                    let s = ocean.sky_segments[i];
                    let side = u32(s.w);
                    if (side == xside && zc >= s.y - 2.0 && zc <= s.z + 2.0) { found_x = true; }
                    if (side == zside && xc >= s.y - 2.0 && xc <= s.z + 2.0) { found_z = true; }
                }
                if (found_x && found_z) { return true; }
            }
            return false;
        }
    }
    if (count == 0u) { return true; }
    if (any(p < bounds.xy) || any(p > bounds.zw)) { return false; }
    let tile_xy = vec2<u32>(clamp(
        floor((p - bounds.xy) / max(bounds.zw - bounds.xy, vec2<f32>(1.0e-4)) * 8.0),
        vec2<f32>(0.0), vec2<f32>(7.0)));
    let tile = tile_xy.y * 8u + tile_xy.x;
    let code = (ocean.mask_tiles[slot][tile >> 4u] >> ((tile & 15u) * 2u)) & 3u;
    if (code == 1u) { return false; }
    if (code == 2u) { return true; }
    let first = u32(range.x);
    let tolerance = 0.75;
    for (var i = 0u; i < count; i = i + 1u) {
        let t0 = ocean.mask_triangles[(first + i) * 2u];
        let t1 = ocean.mask_triangles[(first + i) * 2u + 1u];
        let a = t0.xy;
        let b = t0.zw;
        let c = t1.xy;

        // Most clipmap fragments are nowhere near most source triangles. Do a
        // cheap triangle-AABB reject before the normalized edge-distance test.
        // Irregular/L-shaped water remains exact, but the common miss path no
        // longer executes three square roots per source triangle.
        let lo = min(a, min(b, c)) - vec2<f32>(tolerance);
        let hi = max(a, max(b, c)) + vec2<f32>(tolerance);
        if (any(p < lo) || any(p > hi)) {
            continue;
        }

        let ab = b - a;
        let bc = c - b;
        let ca = a - c;
        let e0 = (ab.x * (p.y - a.y) - ab.y * (p.x - a.x)) / max(length(ab), 1.0e-4);
        let e1 = (bc.x * (p.y - b.y) - bc.y * (p.x - b.x)) / max(length(bc), 1.0e-4);
        let e2 = (ca.x * (p.y - c.y) - ca.y * (p.x - c.x)) / max(length(ca), 1.0e-4);
        if ((e0 >= -tolerance && e1 >= -tolerance && e2 >= -tolerance)
            || (e0 <= tolerance && e1 <= tolerance && e2 <= tolerance)) {
            return true;
        }
    }
    return false;
}

// This is a VERTEX-only predicate. Evaluate it before any texture fetch,
// Gerstner swell, coastal lookup, or interaction sampling, so an exactly-flat
// vertex costs just arithmetic and an early return instead of 3 FFT reads.
// Flattens the outermost clipmap ring where it joins the mesh-free infinite
// ocean. appearance_a.w is 1 only in a promoted per-pool
// render uniform that has that analytic tail.
fn promoted_ocean_edge_weight(position: vec3<f32>, mesh_center: vec2<f32>) -> f32 {
    if (floor(ocean.appearance_a.w) < 0.5 || ocean.authored_plane.w < 0.5) { return 1.0; }
    // The CPU's analytic continuation uses this exact square radius, including
    // its underwater LOD limit. Do not flatten LOD ring joins.
    let radius = ocean.appearance_c.w;
    if (radius <= 0.0) { return 1.0; }
    // The seam attenuation MUST use position + per-pool uniforms only. A fine
    // and coarse LOD vertex at the same world position must get the exact
    // same height or the existing watertight 2:1 clipmap transition cracks.
    let seam_cell = min(max(ocean.wind_motion.w, 1.0), radius / max(ocean.authored_plane.y, 1.0));
    let seam_fade = max(seam_cell * 2.0, 1.0);
    let outer_distance = radius - max(abs(position.x-mesh_center.x),
        abs(position.z-mesh_center.y));
    return smoothstep(0.0, seam_fade, outer_distance);
}

fn sample_ocean_displacement(position: vec3<f32>, is_ocean: bool, mesh_center: vec2<f32>) -> vec4<f32> {
    if (!is_ocean) {
        return vec4<f32>(0.0);
    }
    let edge_weight = promoted_ocean_edge_weight(position, mesh_center);
    if (edge_weight <= 0.0) { return vec4<f32>(0.0); }
    let units_per_meter = max(ocean.ocean_info.z, 0.001);
    let camera_distance_m = distance(position, camera.camera_pos_time.xyz) / units_per_meter;
    // Direct port of water.gdshader vertex(): sample in the undisplaced world
    // XZ plane, preserve the raw wave height, then attenuate displacement only.
    let distance_factor = min(exp(-(camera_distance_m - 150.0) * 0.007), 1.0);
    let world_m = position.xz / units_per_meter;
    var displacement_m = vec3<f32>(0.0);
    var crest_scale = 1.0;
    // The clipping rings are concentric squares. Position-only selection
    // means the exact shared edge vertices of adjacent LODs use identical
    // displacement, rather than opening a crack from mismatched wave ages.
    // Both nearest mesh levels (0, 1) use live waves all the way to their
    // outer seam; blending begins *inside* the next coarser mesh ring.
    var medium_weight = 0.0;
    var far_weight = 0.0;
    if (ocean.appearance_c.y > 0.5) {
        let half_cells = select(128.0, max(ocean.authored_plane.y,1.0), ocean.authored_plane.w > 0.5);
        let base_spacing = ocean.appearance_c.z;
        let square_radius = max(abs(position.x-mesh_center.x),abs(position.z-mesh_center.y));
        let near_seam = half_cells * base_spacing * 2.0;
        let middle_seam = half_cells * base_spacing * 4.0;
        // Fade across a few coarse cells; no per-ring choice at the seam.
        medium_weight = smoothstep(near_seam,near_seam+base_spacing*16.0,square_radius);
        far_weight = smoothstep(middle_seam,middle_seam+base_spacing*32.0,square_radius);
    }
    if ocean.map_scales[0].z > 0.0 {
    for (var i = 0u; i < 3u; i = i + 1u) {
        let scale = ocean.map_scales[i];
        let uv=world_m * vec2<f32>(1.0,-1.0) * scale.xy;
        var sample: vec4<f32>;
        // One displacement-field read per FFT cascade in the steady state,
        // not simultaneous reads from live, medium AND far. The seam path
        // pays for blending to keep the seams watertight.
        if (far_weight >= 1.0) {
            sample=textureSampleLevel(ocean_displacement_far,ocean_sampler,uv,i32(i),0.0);
        } else if (medium_weight >= 1.0 && far_weight <= 0.0) {
            sample=textureSampleLevel(ocean_displacement_medium,ocean_sampler,uv,i32(i),0.0);
        } else if (medium_weight <= 0.0) {
            sample=textureSampleLevel(ocean_displacement,ocean_sampler,uv,i32(i),0.0);
        } else {
            let medium=textureSampleLevel(ocean_displacement_medium,ocean_sampler,uv,i32(i),0.0);
            if (far_weight > 0.0) {
                let far=textureSampleLevel(ocean_displacement_far,ocean_sampler,uv,i32(i),0.0);
                sample=mix(medium,far,far_weight);
            } else {
                let live=textureSampleLevel(ocean_displacement,ocean_sampler,uv,i32(i),0.0);
                sample=mix(live,medium,medium_weight);
            }
        }
        displacement_m += sample.xyz * vec3<f32>(1.0,1.0,-1.0) * scale.z;
        if sample.w < 0.0 { crest_scale = -sample.w; }
    }
    }
    displacement_m.x *= ocean.swell_motion.z;
    displacement_m.z *= ocean.swell_motion.z;
    let phase = dot(world_m, ocean.swell.xy) * ocean.swell.z - ocean.swell_motion.x * ocean.swell_motion.w;
    let horizontal = ocean.swell.w * ocean.swell_motion.y * cos(phase) * crest_scale;
    displacement_m += vec3<f32>(ocean.swell.x * horizontal, ocean.swell.w * sin(phase), ocean.swell.y * horizontal);
    let coastal=sample_coastal(position);
    // Incoming FFT has already forced the coastal solution. Replace that wave
    // field here; adding both elevations would count the incoming wave twice.
    displacement_m=mix(displacement_m,vec3<f32>(0.0,coastal.field.x/units_per_meter,0.0),coastal.field.w);
    // The selected pool's render uniform carries its effective amplitude;
    // no material multiplier attenuates the wave controls a second time.
    let interaction = sample_ocean_interaction(position);
    displacement_m.y += interaction.x / units_per_meter;
    // Hybrid: the Godot mesh and the native mesh read different wave fields, so
    // near the eye lift this one onto the surface the classification uses.
    if (ocean.hybrid_eye.y > 0.5) {
        let eye_distance_m = distance(position.xz, camera.camera_pos_time.xz) / units_per_meter;
        displacement_m.y += ocean.hybrid_eye.x * (1.0 - smoothstep(0.0, 60.0, eye_distance_m));
    }
    // A zero-weight vertex returns BEFORE any of these expensive samples.
    // Only the narrow transition strip pays for this multiply.
    return vec4<f32>(displacement_m * distance_factor * edge_weight,
        displacement_m.y * edge_weight);
}

struct VertexLightgridSample {
    direction: vec4<f32>,
    lighting: vec4<f32>,
};

fn sample_vertex_static_lightgrid(world_position: vec3<f32>) -> VertexLightgridSample {
    var result: VertexLightgridSample;
    result.direction = vec4<f32>(0.0);
    result.lighting = vec4<f32>(0.0);
    let has_pbr_brdf_map = (material_flags() & (8u | 16u | 64u | 128u)) != 0u;
    if (!PBR_VERTEX_LIGHTGRID
        || !ENABLE_PBR
        || !ENABLE_DELUXE
        || static_lightgrid.origin_enabled.w < 0.5
        || (material_flags() & 2u) == 0u
        || !has_pbr_brdf_map) {
        return result;
    }
    let jka_position = vec3<f32>(world_position.x, -world_position.z, world_position.y);
    let grid = (jka_position - static_lightgrid.origin_enabled.xyz) * static_lightgrid.inv_size.xyz;
    let bounds = static_lightgrid.bounds.xyz;
    if (any(grid < vec3<f32>(-0.5)) || any(grid > bounds - vec3<f32>(0.5))) {
        return result;
    }
    let uvw = (grid + vec3<f32>(0.5)) / max(bounds, vec3<f32>(1.0));
    result.direction = textureSampleLevel(static_lightgrid_direction, static_lightgrid_sampler, uvw, 0.0);
    result.lighting = textureSampleLevel(static_lightgrid_lighting, static_lightgrid_sampler, uvw, 0.0);
    return result;
}

fn q3map_effective_distance(light: PointLight, distance_to_light: f32) -> f32 {
    let extra_distance = max(light.shadow.z, 0.0);
    return max(16.0, sqrt(distance_to_light * distance_to_light + extra_distance * extra_distance));
}

fn q3map_surface_angle(light: PointLight, ndotl: f32) -> f32 {
    if (light.shadow.y <= 0.5) {
        return ndotl;
    }
    // Source-map q3map point metadata is packed into negative emitter.w:
    // -1 = no angle attenuation, -2 = ordinary Lambert, <-2 = _anglescale.
    if (light.emitter.w > -1.5) {
        return 1.0;
    }
    var angle = ndotl;
    if (light.emitter.w < -2.0001) {
        let angle_scale = max(-light.emitter.w - 2.0, 1.0e-5);
        angle = min(angle / angle_scale, 1.0);
    }
    return max(angle, 0.0);
}

fn local_light_attenuation(light: PointLight, distance_to_light: f32) -> f32 {
    // Runtime/FX lights keep the engine's existing smooth finite-radius response.
    // Source-map q3map lights use q3map_light_scalar() instead.
    let radius = max(light.position_radius.w, 1.0);
    if (light.shadow.w >= 0.5 && !ENABLE_CLUSTERED_LITE_DLIGHTS) {
        // Runtime dlights (sabers, blasters, explosions) use the stock JKA
        // 1 - d^2/r^2 response. The steeper (1 - d/r)^2 below left them a
        // fraction of Legacy's brightness at typical wall/floor distances.
        let x = distance_to_light / radius;
        if (lighting_settings.map_minlight.w > 0.5) {
            // Windowed inverse-square: peaky near the source, long soft tail, and
            // exactly zero with zero slope at the radius so no visible edge. The
            // 0.25 core keeps the peak finite; the gain matches stock mid-range.
            let x2 = x * x;
            let window = clamp(1.0 - x2 * x2, 0.0, 1.0);
            return window * window * (0.25 / (0.25 + x2)) * 2.04;
        }
        return max(1.0 - x * x, 0.0);
    }
    let edge = max(1.0 - distance_to_light / radius, 0.0);
    return edge * edge;
}

fn q3map_light_scalar(light: PointLight, distance_to_light: f32, surface_angle: f32) -> f32 {
    // color_intensity.a stores q3map photons / 255, so this returns exactly the
    // normalized lightmap-space scalar before authored light color is applied.
    let intensity = light.color_intensity.a;
    let distance = q3map_effective_distance(light, distance_to_light);
    if (light.shadow.y > 1.5) {
        // Quake3/JKA default: photons / distance^2 * angle. The radius is only
        // the q3map envelope used by clustering/culling and never softens falloff.
        return max(intensity * surface_angle / (distance * distance), 0.0);
    }
    if (light.shadow.y > 0.5) {
        // q3map2 linear: max(0, angle*photons*linearScale - distance*fade).
        // scene.rs stores radius = photons*linearScale/fade, so fade can be
        // eliminated algebraically. Unlike a normal edge fade, the distance term
        // is NOT multiplied by angle; this also keeps _anglescale behavior exact.
        let radius = max(light.position_radius.w, 1.0);
        return intensity * (1.0 / 8000.0) * max(surface_angle - distance / radius, 0.0);
    }
    return 0.0;
}

fn q3map_fast_contribution_visible(light_scalar: f32) -> bool {
    // The reference compile uses -fast. q3map2 rejects scalar contributions <=
    // falloffTolerance (default 1) before light color is applied. The scalar here
    // is normalized from q3map's 0..255 lightmap accumulation domain.
    return light_scalar > (1.0 / 255.0);
}

fn local_light_surface_scale(light: PointLight) -> f32 {
    if (ENABLE_MAP_LIGHT_SIMULATION && light.shadow.y > 0.5) {
        return 1.0;
    }
    if (ENABLE_CLUSTERED_LITE_DLIGHTS && light.shadow.w >= 0.5) {
        return 2.20;
    }
    return 0.35;
}

fn transient_vertex_dlight(world_position: vec3<f32>, world_normal: vec3<f32>) -> vec3<f32> {
    if (!ENABLE_VERTEX_DLIGHTS || camera.render_flags.x != 0u || lighting_settings.values.y == 0u) {
        return vec3<f32>(0.0);
    }
    let start = min(lighting_settings.local_shadows.w, lighting_settings.values.y);
    let end = min(lighting_settings.values.y, start + 32u);
    let normal = normalize(world_normal);
    var result = vec3<f32>(0.0);
    for (var i = start; i < end; i += 1u) {
        let light = dynamic_lights[i];
        if (light.shadow.w < 0.5) { continue; }
        let delta = light.position_radius.xyz - world_position;
        let distance_to_light = length(delta);
        let radius = max(light.position_radius.w, 1.0);
        if (distance_to_light <= 0.0001 || distance_to_light >= radius) { continue; }
        let light_direction = delta / distance_to_light;
        let ndotl = max(dot(normal, light_direction), 0.0);
        if (ndotl <= 0.0) { continue; }
        let emission_visibility = emitter_visibility(light, light_direction);
        if (emission_visibility <= 0.0) { continue; }
        let falloff = max(1.0 - distance_to_light / radius, 0.0);
        result += light.color_intensity.rgb * light.color_intensity.a
            * (falloff * falloff) * ndotl * emission_visibility * 2.20;
    }
    return result;
}

fn world_vertex(input: VertexIn, instance_index: u32, legacy_dlight_surface_id: u32) -> VertexOut {
    var output: VertexOut;
    // Only the planar-reflection pass sets render_flags.x, and it only exists while
    // ENABLE_PLANAR_REFLECTIONS is on.
    let allow_shell = !ENABLE_PLANAR_REFLECTIONS || camera.render_flags.x == 0u;
    let ocean_vertex = is_ocean_vertex(input);
    var source_position = input.position;
    var source_normal = input.normal;
    var ocean_grid_center = vec2<f32>(0.0);
    if (ocean_vertex) {
        ocean_grid_center = ocean_clipmap_center(input);
        source_position = ocean_clipmap_position(input);
        source_normal = vec3<f32>(0.0, 1.0, 0.0);
    }
    var deformation = vec4<f32>(source_position, 0.0);
    if (ENABLE_SURFACE_DEFORMATION) {
        deformation = deform_surface_deformation_vertex(
            source_position,
            source_normal,
            material_flags_w(),
            instance_index,
            allow_shell
        );
    }
    let ocean_sample = sample_ocean_displacement(deformation.xyz, ocean_vertex, ocean_grid_center);
    let ocean_displacement_m = ocean_sample.xyz;
    let world_position = foliage_wind_position(
        deformation.xyz + ocean_displacement_m * max(ocean.ocean_info.z, 0.001),
        material_flags_w(), camera.foliage_wind);
    output.clip_position = camera.view_proj * vec4<f32>(world_position, 1.0);
    if (ocean_vertex && output.clip_position.w > 0.0) {
        // The map's distanceCull sets the camera far plane, but water is meant to
        // reach the horizon. Reverse-Z puts the far plane at depth 0, so flatten
        // beyond-far vertices to the same epsilon the analytic far ocean writes
        // instead of letting the clipper cut the clipmap off. Only depth changes;
        // x/y/w, and so the screen position and interpolated world position, are exact.
        output.clip_position.z = max(output.clip_position.z, output.clip_position.w * 0.000001);
    }
    output.uv = vec2<f32>(0.0);
    if (!ocean_vertex) {
        output.uv = generated_uv(input);
    }
    output.lightmap_uv = input.lightmap_uv;
    output.color = input.color;
    output.sky_dir_ao = vec4<f32>(world_position - camera.camera_pos_time.xyz, input.static_ao);
    output.world_position = world_position;
    output.world_normal = normalize(source_normal);
    if (ENABLE_SURFACE_DEFORMATION) {
        output.world_normal = surface_deformation_vertex_normal(
            source_position, source_normal, material_flags_w(), instance_index, allow_shell
        );
    }
    output.baked_sun_fit = vec4<f32>(0.0);
    if (ENABLE_BAKED_SUN_RELIGHT && instance_index == 0u) {
        let surface_count = arrayLength(&legacy_dlight_surface_masks) / 5u;
        if (legacy_dlight_surface_id < surface_count) {
            let base = surface_count + legacy_dlight_surface_id * 4u;
            output.baked_sun_fit = bitcast<vec4<f32>>(vec4<u32>(
                legacy_dlight_surface_masks[base], legacy_dlight_surface_masks[base + 1u],
                legacy_dlight_surface_masks[base + 2u], legacy_dlight_surface_masks[base + 3u]));
        }
    }
    output.vertex_dlight = transient_vertex_dlight(world_position, output.world_normal);
    output.shell_kind = select(0u, 1u, instance_index != 0u);
    output.shell_coverage = deformation.w;
    output.ocean_wave_height = ocean_sample.w;
    output.ocean_uv_m = deformation.xz / max(ocean.ocean_info.z, 0.001);
    // Shared promoted grids carry no mask slot; the pool's promoted view has
    // it as its surface index (an unmasked slot has an empty mask range).
    let ocean_mask_slot = select(input.color.a, ocean.spray_planes[0].z, ocean.authored_plane.w > 0.5);
    output.ocean_vertex = select(0.0, 2.0 + ocean_mask_slot, ocean_vertex);
    let vertex_lightgrid = sample_vertex_static_lightgrid(world_position);
    output.pbr_lightgrid_direction = vertex_lightgrid.direction;
    output.pbr_lightgrid_lighting = vertex_lightgrid.lighting;
    // Legacy dlight membership is constant for an authored BSP surface. Resolve
    // the storage-buffer lookup once per vertex and pass the resulting bitmask
    // flat to the fragment shader instead of reading the mask buffer for every
    // world fragment. Unknown/procedural geometry preserves the conservative
    // all-lights fallback used by the previous path.
    var legacy_dlight_bits = 0u;
    if (ENABLE_LEGACY_DLIGHTS) {
        legacy_dlight_bits = 0xffffffffu;
        if (instance_index == 0u && legacy_dlight_surface_id != 0xffffffffu) {
            if (legacy_dlight_surface_id < arrayLength(&legacy_dlight_surface_masks)) {
                legacy_dlight_bits = legacy_dlight_surface_masks[legacy_dlight_surface_id];
            } else {
                legacy_dlight_bits = 0u;
            }
        }
    }
    output.legacy_dlight_bits = legacy_dlight_bits;
    return output;
}

// TaystJK rd-vanilla Autosprite2Deform, prepared per authored quad at map load.
// position = midpoint of its short edge, normal = shared long axis,
// lightmap_uv.x = signed half-width; y = -1337 is the encoding marker.
// Unmodified geometry (including source .map) keeps its original positions.
// This entry exists only for the autoSprite2 pipeline key; ordinary BSP vertex
// shaders have no billboard branch, uniforms or per-vertex overhead.
fn autosprite2_input(original: VertexIn) -> VertexIn {
    var input = original;
    let half_width = original.lightmap_uv.x;
    if (original.lightmap_uv.y == -1337.0) {
        // BackEnd view axis[0] is the camera forward direction (not the
        // position-to-eye vector). A single long-axis rotation, not full XYZ.
        let minor = cross(original.normal, camera.camera_forward.xyz);
        let length_sq = dot(minor, minor);
        if (length_sq > 0.000001) {
            input.position = original.position + minor * (half_width * inverseSqrt(length_sq));
            // Deformed surface normal for lighting passes that use it.
            input.normal = normalize(cross(original.normal, minor));
        }
    }
    return input;
}

@vertex fn vs_autosprite2(input: VertexIn, @builtin(instance_index) instance_index: u32) -> VertexOut {
    return world_vertex(autosprite2_input(input), instance_index, 0xffffffffu);
}

@vertex fn vs_autosprite2_legacy(
    input: VertexIn,
    @location(6) legacy_dlight_surface_id: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    return world_vertex(autosprite2_input(input), instance_index, legacy_dlight_surface_id);
}

@vertex fn vs_main(input: VertexIn, @builtin(instance_index) instance_index: u32) -> VertexOut {
    return world_vertex(input, instance_index, 0xffffffffu);
}

@vertex fn vs_main_legacy(
    input: VertexIn,
    @location(6) legacy_dlight_surface_id: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    return world_vertex(input, instance_index, legacy_dlight_surface_id);
}

// Submerged Hybrid draws Blue Water's own clipmap (`OceanVertex`: offset from
// its level's frame in metres, level) with this shader, so only the mesh comes
// from Blue Water while the displacement and material stay the Godot ones.
struct NativeOceanVertexIn {
    @location(0) position: vec2<f32>,
    @location(1) level: vec2<f32>,
};

fn native_ocean_mod2(x: vec2<f32>) -> vec2<f32> {
    return x - 2.0 * floor(x * 0.5);
}

// Blue Water's `surface.wgsl` vertex placement, verbatim: each level snapped to
// its own lattice, a ring's inner edge on the finer level's frame, and the
// geomorph that closes the 2:1 seam at a level's outer row. Returns metres.
fn native_ocean_grid(input: NativeOceanVertexIn) -> vec2<f32> {
    let levels = i32(ocean.native_clipmap.y);
    let segments = ocean.native_clipmap.x;
    let level = i32(clamp(input.level.x, 0.0, f32(levels - 1)));
    let frame = ocean.native_frames[level];
    let finer = ocean.native_frames[max(level - 1, 0)];
    let cell = frame.z;
    let lattice = input.position * frame.w;
    let inner = max(abs(lattice.x), abs(lattice.y)) <= segments / 4.0 && level > 0;
    let index = lattice + select(vec2<f32>(0.0), (finer.xy - frame.xy) * frame.w, inner);
    let snapped = frame.xy + index * cell;
    let reach = max(abs(snapped.x - ocean.native_clipmap.z), abs(snapped.y - ocean.native_clipmap.w))
        / (cell * (segments / 2.0));
    let receding = select(0.0, ocean.native_detail.y, level == i32(ocean.native_detail.x));
    let morph = max(smoothstep(0.6, 0.85, reach), receding);
    return snapped - native_ocean_mod2(index) * morph * cell;
}

@vertex fn vs_ocean_native(input: NativeOceanVertexIn) -> VertexOut {
    var output: VertexOut;
    let units_per_meter = max(ocean.ocean_info.z, 0.001);
    let unfolded = native_ocean_grid(input) * units_per_meter;
    // The Godot clipmap's fold onto the pool rectangle (`ocean_clipmap_position`):
    // an unmasked pool has no fragment outline test, only this clamp.
    let promoted_dynamic = ocean.authored_plane.w > 0.5;
    let sky_enabled = promoted_dynamic && ocean.authored_plane.w > 1.5;
    let folded = select(unfolded, clamp(unfolded, ocean.authored_bounds.xy, ocean.authored_bounds.zw),
        promoted_dynamic && !sky_enabled);
    let source_position = vec3<f32>(folded.x, ocean.native_detail.z, folded.y);
    let ocean_sample = sample_ocean_displacement(source_position, true,
        ocean.native_clipmap.zw * units_per_meter);
    let world_position = source_position + ocean_sample.xyz * units_per_meter;
    output.clip_position = camera.view_proj * vec4<f32>(world_position, 1.0);
    if (output.clip_position.w > 0.0) {
        // As `world_vertex`: water past the far plane keeps its screen position.
        output.clip_position.z = max(output.clip_position.z, output.clip_position.w * 0.000001);
    }
    output.uv = vec2<f32>(0.0);
    output.lightmap_uv = vec2<f32>(0.0);
    output.color = vec4<f32>(1.0);
    output.sky_dir_ao = vec4<f32>(world_position - camera.camera_pos_time.xyz, -1.0);
    output.world_position = world_position;
    output.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    output.shell_kind = 0u;
    output.shell_coverage = 0.0;
    output.ocean_wave_height = ocean_sample.w;
    output.ocean_uv_m = source_position.xz / units_per_meter;
    output.ocean_vertex = 2.0 + ocean.native_detail.w;
    output.pbr_lightgrid_direction = vec4<f32>(0.0);
    output.pbr_lightgrid_lighting = vec4<f32>(0.0);
    output.vertex_dlight = vec3<f32>(0.0);
    output.legacy_dlight_bits = 0u;
    output.baked_sun_fit = vec4<f32>(0.0);
    return output;
}

// OpenJK does not rasterize the authored sky brush at its literal world-space
// depth. The brush polygons only determine which angular part of the sky is
// visible; the generated outer sky is placed at zFar. Keep the original X/Y/W
// projection here so the BSP polygon remains the visibility mask, but force the
// reversed-Z depth to 0 (the camera far plane). This prevents distanceCull from
// clipping distant sky brushes while leaving ordinary world geometry unchanged.
fn sky_vertex(input: VertexIn, instance_index: u32, legacy_dlight_surface_id: u32) -> VertexOut {
    var output = world_vertex(input, instance_index, legacy_dlight_surface_id);
    output.clip_position.z = 0.0;
    return output;
}

@vertex fn vs_sky(input: VertexIn, @builtin(instance_index) instance_index: u32) -> VertexOut {
    return sky_vertex(input, instance_index, 0xffffffffu);
}

@vertex fn vs_sky_legacy(
    input: VertexIn,
    @location(6) legacy_dlight_surface_id: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    return sky_vertex(input, instance_index, legacy_dlight_surface_id);
}

fn geometric_frame(input: VertexOut) -> mat3x3<f32> {
    let geometric = normalize(input.world_normal);
    var helper = vec3<f32>(0.0, 0.0, 1.0);
    if (abs(geometric.z) > 0.9) {
        helper = vec3<f32>(0.0, 1.0, 0.0);
    }
    let tangent = normalize(cross(helper, geometric));
    let bitangent = normalize(cross(geometric, tangent));
    return mat3x3<f32>(tangent, bitangent, geometric);
}

fn cotangent_frame(input: VertexOut) -> mat3x3<f32> {
    let geometric = normalize(input.world_normal);
    let dp1 = dpdx(input.world_position);
    let dp2 = dpdy(input.world_position);
    let duv1 = dpdx(input.uv);
    let duv2 = dpdy(input.uv);
    let determinant = duv1.x * duv2.y - duv1.y * duv2.x;
    if (abs(determinant) < 1e-8) {
        return geometric_frame(input);
    }
    let tangent = normalize((dp1 * duv2.y - dp2 * duv1.y) / determinant);
    let bitangent = normalize((-dp1 * duv2.x + dp2 * duv1.x) / determinant);
    return mat3x3<f32>(tangent, bitangent, geometric);
}

fn companion_sample_height(uv: vec2<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> vec4<f32> {
    if (PBR_COMPANION_SAMPLER) {
        if (POM_MIP_AWARE) {
            return textureSampleGrad(height_texture, pbr_sampler, uv, uv_dx, uv_dy);
        }
        return textureSampleLevel(height_texture, pbr_sampler, uv, 0.0);
    }
    if (POM_MIP_AWARE) {
        return textureSampleGrad(height_texture, base_sampler, uv, uv_dx, uv_dy);
    }
    return textureSampleLevel(height_texture, base_sampler, uv, 0.0);
}

fn companion_sample_normal(uv: vec2<f32>) -> vec4<f32> {
    if (PBR_COMPANION_SAMPLER) {
        return textureSample(normal_texture, pbr_sampler, uv);
    }
    return textureSample(normal_texture, base_sampler, uv);
}

fn parallax_uv(input: VertexOut, shared_frame: mat3x3<f32>) -> vec2<f32> {
    if (!ENABLE_POM || (material_flags() & 32u) == 0u) {
        return input.uv;
    }
    var frame = shared_frame;
    if (!PBR_SHARED_TANGENT_FRAME) {
        frame = cotangent_frame(input);
    }
    let view_world = normalize(camera.camera_pos_time.xyz - input.world_position);
    let view_ts = vec3<f32>(
        dot(view_world, frame[0]),
        dot(view_world, frame[1]),
        dot(view_world, frame[2])
    );
    if (view_ts.z <= 0.05) {
        return input.uv;
    }

    let renderer_height_scale = max(f32(pbr_settings.values.z) * 0.001, 0.001);
    let height_scale = select(renderer_height_scale, material.params.y, material.params.y > 0.0);
    let grazing = 1.0 - clamp(view_ts.z, 0.0, 1.0);
    var layer_count = u32(round(mix(8.0, 24.0, grazing)));
    let uv_dx = dpdx(input.uv);
    let uv_dy = dpdy(input.uv);
    if (POM_ADAPTIVE_STEPS) {
        // Estimate how much the full parallax ray can move in screen pixels.
        // Sub-pixel displacement is invisible, while larger displacement earns
        // progressively more layers up to the original angle-driven budget.
        let uv_per_pixel = max(max(length(uv_dx), length(uv_dy)), 1e-6);
        let max_uv_shift = length((view_ts.xy / max(view_ts.z, 0.1)) * height_scale);
        let displacement_pixels = max_uv_shift / uv_per_pixel;
        if (displacement_pixels < 0.35) {
            return input.uv;
        }
        let screen_layers = clamp(ceil(displacement_pixels * 2.0), 4.0, 24.0);
        layer_count = max(4u, min(layer_count, u32(screen_layers)));
    }
    let safe_layer_count = max(layer_count, 1u);
    let layer_depth = 1.0 / f32(safe_layer_count);
    let uv_step = (view_ts.xy / max(view_ts.z, 0.1)) * height_scale / f32(safe_layer_count);

    var uv = input.uv;
    var previous_uv = uv;
    var current_layer_depth = 0.0;
    let first_height_sample = companion_sample_height(uv, uv_dx, uv_dy);
    var sampled_depth = 1.0 - select(first_height_sample.r, first_height_sample.a, (material_flags() & 1024u) != 0u);
    var previous_sampled_depth = sampled_depth;
    var i = 0u;
    loop {
        if (i >= layer_count || current_layer_depth >= sampled_depth) {
            break;
        }
        previous_uv = uv;
        previous_sampled_depth = sampled_depth;
        uv -= uv_step;
        current_layer_depth += layer_depth;
        let height_sample = companion_sample_height(uv, uv_dx, uv_dy);
        sampled_depth = 1.0 - select(height_sample.r, height_sample.a, (material_flags() & 1024u) != 0u);
        i += 1u;
    }

    // Linear refinement between the last two layer intersections removes most
    // of the visible stepping without a second expensive search loop.
    let after = sampled_depth - current_layer_depth;
    let before = previous_sampled_depth - (current_layer_depth - layer_depth);
    let denominator = after - before;
    var weight = 0.5;
    if (abs(denominator) >= 1e-5) {
        weight = clamp(after / denominator, 0.0, 1.0);
    }
    return mix(uv, previous_uv, weight);
}

fn cotangent_normal(input: VertexOut, uv: vec2<f32>, shared_frame: mat3x3<f32>) -> vec3<f32> {
    let geometric = normalize(input.world_normal);
    if (!ENABLE_PBR || (material_flags() & 8u) == 0u) {
        return geometric;
    }
    var map_normal = companion_sample_normal(uv).xyz * 2.0 - vec3<f32>(1.0);
    map_normal = normalize(vec3<f32>(
        map_normal.x * material.pbr_params0.x,
        map_normal.y * material.pbr_params0.y,
        map_normal.z
    ));
    var frame = shared_frame;
    if (!PBR_SHARED_TANGENT_FRAME) {
        frame = cotangent_frame(input);
    }
    return normalize(frame[0] * map_normal.x + frame[1] * map_normal.y + frame[2] * map_normal.z);
}

fn sample_shadow_map_bevy_hardware(light_local: vec2<f32>, depth: f32, cascade: i32) -> f32 {
    return textureSampleCompareLevel(
        shadow_texture,
        shadow_sampler,
        light_local,
        cascade,
        depth
    );
}

// Faithful Bevy 0.19.1 Castano '13 directional-shadow filter from
// bevy_pbr/src/render/shadow_sampling.wgsl.
fn sample_shadow_map_bevy_castano(light_local: vec2<f32>, depth: f32, cascade: i32) -> f32 {
    let shadow_map_size = vec2<f32>(textureDimensions(shadow_texture));
    let inv_shadow_map_size = 1.0 / shadow_map_size;
    let uv = light_local * shadow_map_size;
    var base_uv = floor(uv + 0.5);
    let ss = uv.x + 0.5 - base_uv.x;
    let tt = uv.y + 0.5 - base_uv.y;
    base_uv = (base_uv - 0.5) * inv_shadow_map_size;

    let uw0 = 4.0 - 3.0 * ss;
    let uw1 = 7.0;
    let uw2 = 1.0 + 3.0 * ss;
    let u0 = (3.0 - 2.0 * ss) / uw0 - 2.0;
    let u1 = (3.0 + ss) / uw1;
    let u2 = ss / uw2 + 2.0;

    let vw0 = 4.0 - 3.0 * tt;
    let vw1 = 7.0;
    let vw2 = 1.0 + 3.0 * tt;
    let v0 = (3.0 - 2.0 * tt) / vw0 - 2.0;
    let v1 = (3.0 + tt) / vw1;
    let v2 = tt / vw2 + 2.0;

    var sum = 0.0;
    sum += uw0 * vw0 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u0, v0) * inv_shadow_map_size, depth, cascade);
    sum += uw1 * vw0 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u1, v0) * inv_shadow_map_size, depth, cascade);
    sum += uw2 * vw0 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u2, v0) * inv_shadow_map_size, depth, cascade);
    sum += uw0 * vw1 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u0, v1) * inv_shadow_map_size, depth, cascade);
    sum += uw1 * vw1 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u1, v1) * inv_shadow_map_size, depth, cascade);
    sum += uw2 * vw1 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u2, v1) * inv_shadow_map_size, depth, cascade);
    sum += uw0 * vw2 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u0, v2) * inv_shadow_map_size, depth, cascade);
    sum += uw1 * vw2 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u1, v2) * inv_shadow_map_size, depth, cascade);
    sum += uw2 * vw2 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u2, v2) * inv_shadow_map_size, depth, cascade);
    return sum * (1.0 / 144.0);
}

fn sample_cascade_shadow_legacy(input: VertexOut, normal: vec3<f32>, cascade: i32) -> f32 {
    let clip = shadow_settings.view_proj[u32(cascade)] * vec4<f32>(input.world_position, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);
    if (
        uv.x <= 0.0 || uv.x >= 1.0 || uv.y <= 0.0 || uv.y >= 1.0
        || ndc.z <= 0.0 || ndc.z >= 1.0
    ) {
        return 1.0;
    }
    let light_direction = normalize(shadow_settings.light_direction_enabled.xyz);
    let receiver_normal = normalize(normal);
    let slope = 1.0 - max(dot(receiver_normal, -light_direction), 0.0);
    let bias = shadow_settings.params.y * (1.0 + slope * 4.0);
    let texel = 1.0 / max(shadow_settings.params.x, 1.0);
    var visibility = 0.0;
    for (var y: i32 = -1; y <= 1; y += 1) {
        for (var x: i32 = -1; x <= 1; x += 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel;
            visibility += textureSampleCompare(
                shadow_texture,
                shadow_sampler,
                uv + offset,
                cascade,
                ndc.z - bias
            );
        }
    }
    return visibility / 9.0;
}

fn sample_cascade_shadow_bevy(input: VertexOut, normal: vec3<f32>, cascade: i32) -> f32 {
    if shadow_settings.params.w > 1.5 {
        return sample_dynamic_cascade(input.world_position, normal, cascade);
    }
    let light_direction = normalize(shadow_settings.light_direction_enabled.xyz);
    let texel_size = shadow_settings.cascade_texel_sizes[u32(cascade)];
    // Bevy stores direction_to_light; JKA stores light travel direction, so negate it.
    let normal_offset = shadow_settings.bevy_params.y * texel_size * normal;
    let depth_offset = shadow_settings.bevy_params.x * -light_direction;
    let offset_position = vec4<f32>(input.world_position + normal_offset + depth_offset, 1.0);
    let clip = shadow_settings.view_proj[u32(cascade)] * offset_position;
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if (any(ndc.xy < vec2<f32>(-1.0)) || ndc.z < 0.0 || any(ndc > vec3<f32>(1.0))) {
        return 1.0;
    }
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return min(sample_shadow_map_bevy_castano(uv, ndc.z, cascade), sample_dynamic_cascade(input.world_position, normal, cascade));
}

fn shader_sun_sky_admission(input: VertexOut, cascade: i32) -> f32 {
    if shadow_settings.params.w > 1.5 { return 1.0; }
    let mode = shadow_settings.camera_forward.w;
    if (mode < 0.5) {
        return 1.0;
    }
    let clip = shadow_settings.view_proj[u32(cascade)] * vec4<f32>(input.world_position, 1.0);
    if (clip.w <= 0.0) {
        return 0.0;
    }
    let ndc = clip.xyz / clip.w;
    // Only the cascade's X/Y footprint bounds the sky map. The encoded sky depth is
    // valid for any receiver Z (it is atan-mapped, not clipped), and a receiver
    // outside the footprint is left to the shadow-map path, which also reports lit.
    // Rejecting on Z here made a receiver at the edge of a view-dependent Z range
    // read as "no sky", so lighting changed with camera orientation.
    if (any(abs(ndc.xy) > vec2<f32>(1.0))) {
        return 1.0;
    }
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    let dims = vec2<i32>(textureDimensions(sky_admission_texture));
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - vec2<i32>(1));
    let sky_depth = textureLoad(sky_admission_texture, pixel, cascade, 0);
    let bevy_mode = shadow_settings.params.w > 0.5;
    let clear_depth = select(1.0, 0.0, bevy_mode);
    if (abs(sky_depth - clear_depth) < 0.000001) {
        return 0.0;
    }
    let receiver_depth = 0.5 + atan(ndc.z) / 3.14159265358979323846;
    let epsilon = 0.0005;
    return select(
        select(0.0, 1.0, sky_depth <= receiver_depth + epsilon),
        select(0.0, 1.0, sky_depth >= receiver_depth - epsilon),
        bevy_mode
    );
}

fn cascaded_shadow_visibility(input: VertexOut, normal: vec3<f32>) -> f32 {
    if (!ENABLE_CASCADED_SHADOWS || camera.render_flags.x != 0u) {
        return 1.0;
    }
    if (shadow_settings.light_direction_enabled.w < 0.5) {
        return 1.0;
    }
    let camera_depth = max(
        dot(
            input.world_position - camera.camera_pos_time.xyz,
            normalize(shadow_settings.camera_forward.xyz)
        ),
        0.0
    );

    let bevy_mode = shadow_settings.params.w > 0.5;
    let cascade_count = i32(shadow_settings.bevy_params.w + 0.5);
    var cascade = 0i;
    if (camera_depth > shadow_settings.split_depths.x) { cascade = 1i; }
    if (camera_depth > shadow_settings.split_depths.y) { cascade = 2i; }
    if (camera_depth > shadow_settings.split_depths.z) { cascade = 3i; }
    if (cascade >= cascade_count || camera_depth > shadow_settings.split_depths[u32(cascade)]) {
        return 1.0;
    }

    if (bevy_mode) {
        var visibility = sample_cascade_shadow_bevy(input, normal, cascade);
        let next_cascade = cascade + 1i;
        if (next_cascade < cascade_count) {
            let this_far_bound = shadow_settings.split_depths[u32(cascade)];
            let next_near_bound = (1.0 - shadow_settings.bevy_params.z) * this_far_bound;
            if (camera_depth >= next_near_bound) {
                let next_visibility = sample_cascade_shadow_bevy(input, normal, next_cascade);
                visibility = mix(
                    visibility,
                    next_visibility,
                    (camera_depth - next_near_bound) / (this_far_bound - next_near_bound)
                );
            }
        }
        return visibility * shader_sun_sky_admission(input, cascade);
    }

    var visibility = sample_cascade_shadow_legacy(input, normal, cascade);
    if (cascade < 2i && cascade + 1i < cascade_count) {
        let split = shadow_settings.split_depths[u32(cascade)];
        let blend_width = max(split * 0.08, 64.0);
        let blend = smoothstep(split - blend_width, split, camera_depth);
        if (blend > 0.0) {
            let next_visibility = sample_cascade_shadow_legacy(input, normal, cascade + 1i);
            visibility = mix(visibility, next_visibility, blend);
        }
    }
    return visibility * shader_sun_sky_admission(input, cascade);
}

fn cluster_for_fragment(input: VertexOut) -> ClusterRecord {
    let width = max(lighting_settings.values.z, 1u);
    let height = max(lighting_settings.values.w, 1u);
    let tile_x = min(u32(input.clip_position.x / f32(width) * f32(CLUSTER_X)), CLUSTER_X - 1u);
    let tile_y = min(u32(input.clip_position.y / f32(height) * f32(CLUSTER_Y)), CLUSTER_Y - 1u);
    let depth = max(distance(camera.camera_pos_time.xyz, input.world_position), CLUSTER_NEAR);
    let z_fraction = clamp(
        log(depth / CLUSTER_NEAR) / log(CLUSTER_FAR / CLUSTER_NEAR),
        0.0,
        0.999999
    );
    let slice = min(u32(z_fraction * f32(CLUSTER_Z)), CLUSTER_Z - 1u);
    let cluster_index = slice * CLUSTER_X * CLUSTER_Y + tile_y * CLUSTER_X + tile_x;
    return light_clusters[cluster_index];
}

fn sample_local_shadow_visibility(light: PointLight, world_position: vec3<f32>, receiver_normal: vec3<f32>) -> f32 {
    if (!ENABLE_LOCAL_SHADOWS || lighting_settings.local_shadows.x == 0u || light.shadow.x < 0.5) {
        return 1.0;
    }
    let from_light = world_position - light.position_radius.xyz;
    let radial_distance = length(from_light);
    let far_plane = max(light.position_radius.w, LOCAL_SHADOW_NEAR + 1.0);
    if (radial_distance <= LOCAL_SHADOW_NEAR || radial_distance >= far_plane) {
        return 1.0;
    }
    let abs_delta = abs(from_light);
    let major_distance = max(max(abs_delta.x, abs_delta.y), abs_delta.z);
    if (major_distance <= LOCAL_SHADOW_NEAR) {
        return 1.0;
    }
    let direction = from_light / radial_distance;
    let to_light = -direction;
    let slope = 1.0 - max(dot(normalize(receiver_normal), to_light), 0.0);
    // Bias in world units. A fixed normalized-depth bias erases human-sized
    // shadows at distance because perspective depth is reciprocal distance.
    let biased_distance=max(major_distance-(0.5+1.0*slope),LOCAL_SHADOW_NEAR);
    let depth_ref = far_plane / (far_plane - LOCAL_SHADOW_NEAR)
        - (far_plane * LOCAL_SHADOW_NEAR) / ((far_plane - LOCAL_SHADOW_NEAR) * biased_distance);
    let helper = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(direction.y) < 0.9);
    let tangent = normalize(cross(direction, helper));
    let bitangent = normalize(cross(direction, tangent));
    let spread = 1.6 / f32(max(lighting_settings.local_shadows.z, 1u));
    let shadow_index = u32(light.shadow.x - 1.0);
    var visibility = 0.0;
    visibility += textureSampleCompareLevel(local_shadow_texture, local_shadow_sampler, direction + tangent * spread + bitangent * spread, shadow_index, depth_ref);
    visibility += textureSampleCompareLevel(local_shadow_texture, local_shadow_sampler, direction - tangent * spread + bitangent * spread, shadow_index, depth_ref);
    visibility += textureSampleCompareLevel(local_shadow_texture, local_shadow_sampler, direction + tangent * spread - bitangent * spread, shadow_index, depth_ref);
    visibility += textureSampleCompareLevel(local_shadow_texture, local_shadow_sampler, direction - tangent * spread - bitangent * spread, shadow_index, depth_ref);
    return visibility * 0.25;
}
fn local_light_shadow_visibility(light: PointLight, world_position: vec3<f32>, receiver_normal: vec3<f32>) -> f32 {
    let stationary=sample_local_shadow_visibility(light,world_position,receiver_normal);
    if (lighting_settings.baked_sun_debug_mode.w & 1u) == 0u { return stationary; }
    return min(stationary,baked_local_moving_visibility(light,world_position,receiver_normal));
}
// Cheap conservative test reused by the receiver early-out. It cannot
// reject a sample unless the existing face/octant metadata already says
// that no moving caster can affect that lamp-to-receiver direction.
fn baked_local_moving_maybe(light: PointLight, position: vec3<f32>) -> bool {
    if !ENABLE_LOCAL_SHADOWS || (lighting_settings.baked_sun_debug_mode.w & 1u) == 0u || light.shadow.x < 0.5 { return false; }
    if light.shape.x < 0.0 {
        let d=position-light.position_radius.xyz; let a=abs(d);
        var face=select(1u,0u,d.x>=0.0);
        if a.y>a.x && a.y>=a.z { face=select(3u,2u,d.y>=0.0); }
        if a.z>a.x && a.z>a.y { face=select(5u,4u,d.z>=0.0); }
        let packed = u32(-light.shape.x - 1.0);
        if (packed & (1u << face)) == 0u { return false; }
        // AABB-conservative moving-caster octants, generated only by A/B
        // variant B. A ray cannot be occluded by geometry in another octant.
        // Mask zero is the legacy unfiltered path, so old saves/variants match.
        let octants = (packed >> 6u) & 255u;
        if octants != 0u {
            let octant = select(0u, 1u, d.x >= 0.0)
                | select(0u, 2u, d.y >= 0.0)
                | select(0u, 4u, d.z >= 0.0);
            if (octants & (1u << octant)) == 0u { return false; }
        }
    }
    return true;
}
fn baked_local_moving_visibility(light: PointLight, position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if !baked_local_moving_maybe(light,position) { return 1.0; }
    var moving=light;
    let cubes=textureNumLayers(local_shadow_texture);
    if cubes < 16u { return 1.0; }
    moving.shadow.x += 8.0;
    return sample_local_shadow_visibility(moving,position,normal);
}


struct RasterGlassTriangle {
    a: vec4<f32>, b: vec4<f32>, c: vec4<f32>,
    color_a: vec4<f32>, color_b: vec4<f32>, color_c: vec4<f32>,
};
struct RasterGlassSlot {
    header: vec4<u32>,
    triangles: array<RasterGlassTriangle, 64>,
};
@group(2) @binding(17) var<storage, read> raster_glass_slots: array<RasterGlassSlot>;

fn raster_glass_throughput(light: PointLight, receiver: vec3<f32>) -> vec3<f32> {
    if ((lighting_settings.feature_flags.z & 0x200u) == 0u
        || lighting_settings.local_shadows.x == 0u || light.shadow.x < 0.5) {
        return vec3<f32>(1.0);
    }
    let slot_index = u32(light.shadow.x - 1.0);
    if (slot_index >= arrayLength(&raster_glass_slots)) { return vec3<f32>(1.0); }
    let count = min(raster_glass_slots[slot_index].header.x, 64u);
    if (count == 0u) { return vec3<f32>(1.0); }
    let ray = receiver - light.position_radius.xyz;
    let distance = length(ray);
    if (distance < 0.5 || distance > light.position_radius.w) { return vec3<f32>(1.0); }
    let direction = ray / distance;
    var throughput = vec3<f32>(1.0);
    for (var i = 0u; i < count; i += 1u) {
        let triangle = raster_glass_slots[slot_index].triangles[i];
        let e1 = triangle.b.xyz - triangle.a.xyz;
        let e2 = triangle.c.xyz - triangle.a.xyz;
        let p = cross(direction, e2);
        let determinant = dot(e1, p);
        if (abs(determinant) <= 0.0000001) { continue; }
        let inv = 1.0 / determinant;
        let t = light.position_radius.xyz - triangle.a.xyz;
        let u = dot(t, p) * inv;
        if (u < 0.00001 || u > 0.99999) { continue; }
        let q = cross(t, e1);
        let v = dot(direction, q) * inv;
        if (v < 0.00001 || u + v > 0.99999) { continue; }
        let hit = dot(e2, q) * inv;
        // This is the essential depth test: a pane behind the receiver
        // never casts colored light onto a surface in front of it.
        if (hit < 0.25 || hit > distance - 0.25) { continue; }
        let color = triangle.color_a.rgb * (1.0 - u - v)
                  + triangle.color_b.rgb * u + triangle.color_c.rgb * v;
        throughput *= clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
        if (all(throughput < vec3<f32>(0.001))) { return vec3<f32>(0.0); }
    }
    return throughput;
}

// Directional thin-glass transmission. Uses the same per-triangle linear RGB
// throughput as local lights, but the segment extends toward shaderSun rather
// than a finite point emitter. Opaque cascade visibility is always applied first.
fn raster_sun_glass_throughput(receiver: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    if ((lighting_settings.feature_flags.z & 0x200u) == 0u) { return vec3<f32>(1.0); }
    // Empty and overflow-disabled tables take one read instead of probing eight slots.
    if (arrayLength(&raster_glass_slots) <= 8u || raster_glass_slots[8u].header.x == 0u) {
        return vec3<f32>(1.0);
    }
    let toward_sun = -shadow_settings.light_direction_enabled.xyz;
    let len_sq = dot(toward_sun, toward_sun);
    if (len_sq < 1.0e-8) { return vec3<f32>(1.0); }
    let direction = toward_sun * inverseSqrt(len_sq);
    let safe_normal = normal * inverseSqrt(max(dot(normal, normal), 1.0e-12));
    let oriented = select(-safe_normal, safe_normal, dot(safe_normal, direction) >= 0.0);
    let origin = receiver + oriented * 0.5;
    var transmission = vec3<f32>(1.0);
    // Sun casters live in reserved slots after the local-light cubemaps.
    for (var slot = 8u; slot < 16u; slot += 1u) {
        if (slot >= arrayLength(&raster_glass_slots)) { break; }
        let count = min(raster_glass_slots[slot].header.x, 64u);
        if (count == 0u) { continue; }
        // Conservative ray-vs-slot sphere check, avoiding up to 64 triangle
        // tests for almost every sunlit fragment outside this glass patch.
        let bounds = raster_glass_slots[slot].triangles[0];
        let center = vec3<f32>(bounds.a.w, bounds.b.w, bounds.c.w);
        let offset = center - origin;
        let projected = dot(offset, direction);
        let radius = bounds.color_a.w;
        if (projected < -radius || dot(offset, offset) - projected * projected > radius * radius) { continue; }
        for (var i = 0u; i < count; i += 1u) {
            let tri = raster_glass_slots[slot].triangles[i];
            let e1 = tri.b.xyz - tri.a.xyz;
            let e2 = tri.c.xyz - tri.a.xyz;
            let p = cross(direction, e2);
            let determinant = dot(e1, p);
            if (abs(determinant) < 1e-7) { continue; }
            let inv = 1.0 / determinant;
            let t = origin - tri.a.xyz;
            let u = dot(t, p) * inv;
            if (u < -1e-5 || u > 1.00001) { continue; }
            let q = cross(t, e1);
            let v = dot(direction, q) * inv;
            if (v < -1e-5 || u + v > 1.00001) { continue; }
            // Only panes sunward of this receiver filter it. A pane behind the
            // receiving surface must not stain the ground in front of it.
            let hit = dot(e2, q) * inv;
            if (hit < 0.25) { continue; }
            let rgb = tri.color_a.rgb * (1.0 - u - v)
                    + tri.color_b.rgb * u + tri.color_c.rgb * v;
            transmission *= clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
            if (all(transmission < vec3<f32>(0.001))) { return vec3<f32>(0.0); }
        }
    }
    return transmission;
}

fn sun_shadow_transmission(input: VertexOut, normal: vec3<f32>) -> vec3<f32> {
    if (!ENABLE_CASCADED_SHADOWS) { return vec3<f32>(1.0); }
    let opaque_visibility = cascaded_shadow_visibility(input, normal);
    if (opaque_visibility <= 0.0 || shadow_settings.params.w > 1.5 || (lighting_settings.feature_flags.z & 0x200u) == 0u) {
        return vec3<f32>(opaque_visibility);
    }
    let transmission = raster_sun_glass_throughput(input.world_position, normal);
    if ((lighting_settings.feature_flags.z & 0x400u) != 0u) {
        return vec3<f32>(opaque_visibility) * select(vec3<f32>(1.0), vec3<f32>(1.0, 0.05, 0.8), any(transmission < vec3<f32>(0.995)));
    }
    if ((lighting_settings.feature_flags.z & 0x800u) != 0u) {
        return vec3<f32>(opaque_visibility) * (vec3<f32>(1.0) - transmission);
    }
    return vec3<f32>(opaque_visibility) * transmission;
}

fn local_light_shadow_rgb(light: PointLight, world_position: vec3<f32>, receiver_normal: vec3<f32>) -> vec3<f32> {
    let opaque_visibility = local_light_shadow_visibility(light, world_position, receiver_normal);
    if (opaque_visibility <= 0.0) { return vec3<f32>(0.0); }
    let transmission = raster_glass_throughput(light, world_position);
    if ((lighting_settings.feature_flags.z & 0x400u) != 0u) {
        return select(vec3<f32>(1.0), vec3<f32>(1.0, 0.05, 0.8),
            any(transmission < vec3<f32>(0.995))) * opaque_visibility;
    }
    if ((lighting_settings.feature_flags.z & 0x800u) != 0u) {
        return (vec3<f32>(1.0) - transmission) * opaque_visibility;
    }
    return vec3<f32>(opaque_visibility) * transmission;
}

fn emitter_visibility(light: PointLight, direction_to_light: vec3<f32>) -> f32 {
    // Runtime spot/projected light. direction_to_light points receiver->source,
    // so -direction_to_light is the emitted ray direction source->receiver.
    if (light.shadow.w >= 0.5 && light.emitter.w > 2.5) {
        let cone_cos = dot(normalize(light.emitter.xyz), -direction_to_light);
        let inner = clamp(light.shape.x, -1.0, 1.0);
        let outer = clamp(light.shape.y, -1.0, inner);
        if (inner - outer <= 1.0e-5) {
            return select(0.0, 1.0, cone_cos >= inner);
        }
        return smoothstep(outer, inner, cone_cos);
    }
    if (light.emitter.w < 0.5) {
        return 1.0;
    }
    let facing = dot(normalize(light.emitter.xyz), -direction_to_light);
    if (light.emitter.w > 1.5) {
        return abs(facing);
    }
    return max(facing, 0.0);
}

fn direct_light_source_enabled(light: PointLight) -> bool {
    if light.shape.w < -0.5 {return false;}
    if light.shape.w > 0.5 { return true; }
    let is_area_light = light.emitter.w >= 0.5 && light.emitter.w < 2.5;
    if (is_area_light) {
        return ENABLE_AREA_LIGHTS;
    }
    if (ENABLE_POINT_LIGHTS) {
        // Source-map lights use Forward+ even when runtime dlights use Legacy/Vertex.
        // Keep transient lights exclusively on those cheaper paths to avoid doubling.
        if (ENABLE_MAP_LIGHT_SIMULATION
            && (ENABLE_LEGACY_DLIGHTS || ENABLE_VERTEX_DLIGHTS)
            && light.shadow.w >= 0.5) {
            return false;
        }
        return true;
    }
    if (ENABLE_CLUSTERED_LITE_DLIGHTS) {
        return light.shadow.w >= 0.5;
    }
    return false;
}

struct LegacyDynamicLighting {
    diffuse_factor: vec3<f32>,
    wet_specular: vec3<f32>,
};


fn dynamic_light_debug_enabled() -> bool {
    // feature_flags.z low byte is the normal point-light/cluster mode. Bit 8 is
    // a session-only receiver diagnostic flag so the uniform ABI stays stable.
    // Callers test ENABLE_DYNAMIC_LIGHT_DEBUG first so the branch folds away.
    return (lighting_settings.feature_flags.z & 0x100u) != 0u;
}

fn runtime_dynamic_light_debug(input: VertexOut) -> vec3<f32> {
    if (camera.render_flags.x != 0u || lighting_settings.values.y == 0u) {
        return vec3<f32>(0.0);
    }

    // Runtime/transient Vertex lighting is already accumulated in the vertex
    // output. Legacy runtime lights are drawn by their late additive pass. In
    // both modes we can still inspect shader-promoted area emitters through the
    // clustered list, because those are static geometry sources rather than
    // RE_AddLightToScene-style transient lights.
    var result = vec3<f32>(0.0);
    if (ENABLE_VERTEX_DLIGHTS) {
        result += input.vertex_dlight;
    }

    let cluster_sources = ENABLE_AREA_LIGHTS
        || (!ENABLE_VERTEX_DLIGHTS && !ENABLE_LEGACY_DLIGHTS
            && (ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS));
    if (!cluster_sources) {
        return result;
    }

    let cluster = cluster_for_fragment(input);
    let normal = normalize(input.world_normal);
    for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {
        let light = dynamic_lights[cluster.indices[i]];
        let is_area = light.emitter.w >= 0.5 && light.emitter.w < 2.5;
        // Debug means "show the active direct-local light result", not only
        // transient FX. In Vertex/Legacy modes runtime dlights are already
        // supplied by their mode-specific path, so the clustered side contributes
        // only active area emitters (authored q3map or shader-promoted). In
        // Clustered/Forward+/RT the GPU list already contains exactly the source
        // classes enabled by the current settings.
        let debug_source = select(true, is_area, ENABLE_VERTEX_DLIGHTS || ENABLE_LEGACY_DLIGHTS);
        if (!debug_source || !direct_light_source_enabled(light)) {
            continue;
        }
        let delta = light.position_radius.xyz - input.world_position;
        let distance_to_light = length(delta);
        let radius = max(light.position_radius.w, 1.0);
        if (distance_to_light <= 0.0001 || distance_to_light >= radius) {
            continue;
        }
        let light_direction = delta / distance_to_light;
        let ndotl = max(dot(normal, light_direction), 0.0);
        if (ndotl <= 0.0) {
            continue;
        }
        let emission = emitter_visibility(light, light_direction);
        if (emission <= 0.0) {
            continue;
        }
        let shadow = local_light_shadow_rgb(light, input.world_position, normal);
        let attenuation = local_light_attenuation(light, distance_to_light);
        result += light.color_intensity.rgb
            * light.color_intensity.a
            * attenuation
            * emission
            * shadow
            * ndotl
            * local_light_surface_scale(light);
    }
    return result;
}
fn legacy_dynamic_light(input: VertexOut) -> LegacyDynamicLighting {
    var result: LegacyDynamicLighting;
    result.diffuse_factor = vec3<f32>(0.0);
    result.wet_specular = vec3<f32>(0.0);
    if (!ENABLE_LEGACY_DLIGHTS || camera.render_flags.x != 0u || lighting_settings.values.y == 0u) {
        return result;
    }

    let transient_start = min(lighting_settings.local_shadows.w, lighting_settings.values.y);
    let transient_count = min(lighting_settings.values.y - transient_start, 32u);
    if (transient_count == 0u) { return result; }

    // OpenJK prunes runtime dlights at BSP-surface granularity before the
    // expensive projected-light work. Surface identity arrives as a normal flat
    // vertex attribute so this remains portable on backends without WGSL
    // primitive_index support. Unknown/procedural geometry keeps the conservative
    // all-lights fallback.
    // The authored-surface mask was fetched in the vertex stage and arrives
    // flat, so unlit surfaces avoid a storage-buffer read in every fragment.
    let dlight_bits = input.legacy_dlight_bits;
    if (dlight_bits == 0u) { return result; }

    // Preserve the existing Legacy/JKA projected-light response exactly. The
    // surface mask only removes lights that OpenJK-style coarse culling proved
    // cannot touch this surface; it does not change the per-triangle light math.
    let dp_x = dpdx(input.world_position);
    let dp_y = dpdy(input.world_position);
    let plane_cross = cross(dp_x, dp_y);
    let plane_len_sq = dot(plane_cross, plane_cross);
    if (plane_len_sq < 1.0e-10) { return result; }
    var plane_normal = plane_cross * inverseSqrt(plane_len_sq);
    if (dot(plane_normal, input.world_normal) < 0.0) {
        plane_normal = -plane_normal;
    }

    for (var bit = 0u; bit < transient_count; bit += 1u) {
        if ((dlight_bits & (1u << bit)) == 0u) { continue; }
        let light = dynamic_lights[transient_start + bit];
        if (light.shadow.w < 0.5) { continue; }

        let radius = max(light.position_radius.w, 1.0);
        let radius_sq = radius * radius;
        let delta = light.position_radius.xyz - input.world_position;
        let plane_distance = dot(plane_normal, delta);
        if (plane_distance <= 0.0 || plane_distance >= radius) { continue; }

        let projected_radius_sq = max(radius_sq - plane_distance * plane_distance, 1.0e-4);
        let planar_distance_sq = max(dot(delta, delta) - plane_distance * plane_distance, 0.0);
        let radial_sq = planar_distance_sq / projected_radius_sq;
        if (radial_sq >= 1.0) { continue; }
        let x = 1.0 - radial_sq;
        let blob = x * x * (3.0 - 2.0 * x);
        let plane_modulate = max(1.0 - (plane_distance * plane_distance) / radius_sq, 0.0);
        let distance_to_light = length(delta);
        if (distance_to_light <= 1.0e-5) { continue; }
        let emission_visibility = emitter_visibility(light, delta / distance_to_light);
        if (emission_visibility <= 0.0) { continue; }
        result.diffuse_factor += light.color_intensity.rgb
            * light.color_intensity.a
            * plane_modulate
            * blob
            * emission_visibility
            * 0.225;
    }
    return result;
}

// OpenJK ProjectDlightTexture2 projects a small radial dlight image onto the
// receiving triangle after the normal material stages. DinurdoJK keeps the
// same operation in a dedicated depth-equal pass, but evaluates the projection
// directly instead of re-running the entire material shader twice.
//
// This is the exact procedural fallback generated by OpenJK's
// R_CreateDlightImage when gfx/2d/dlight is unavailable: a 16x16 centered
// inverse-square blob, sampled with the same clamp + linear-filter semantics.
fn openjk_dlight_fallback_texel(coord: vec2<i32>) -> f32 {
    let x = f32(clamp(coord.x, 0, 15));
    let y = f32(clamp(coord.y, 0, 15));
    let dx = 7.5 - x;
    let dy = 7.5 - y;
    let distance_sq = max(dx * dx + dy * dy, 1.0e-6);
    var value = 4000.0 / distance_sq;
    if (value > 255.0) {
        value = 255.0;
    } else if (value < 75.0) {
        value = 0.0;
    }
    return value * (1.0 / 255.0);
}

fn sample_openjk_dlight_fallback(uv: vec2<f32>) -> f32 {
    let clamped_uv = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
    // OpenGL/WebGPU normalized linear sampling addresses texel centers at
    // (n + 0.5) / size. Reconstruct that filtering explicitly so this helper
    // does not require another sampled-texture binding in the hot world layout.
    let texel = clamped_uv * 16.0 - vec2<f32>(0.5);
    let base = vec2<i32>(i32(floor(texel.x)), i32(floor(texel.y)));
    let frac_part = fract(texel);
    let a = openjk_dlight_fallback_texel(base);
    let b = openjk_dlight_fallback_texel(base + vec2<i32>(1, 0));
    let c = openjk_dlight_fallback_texel(base + vec2<i32>(0, 1));
    let d = openjk_dlight_fallback_texel(base + vec2<i32>(1, 1));
    return mix(mix(a, b, frac_part.x), mix(c, d, frac_part.x), frac_part.y);
}

fn openjk_projected_dlight_contribution(input: VertexOut) -> vec3<f32> {
    if (!ENABLE_LEGACY_DLIGHTS || camera.render_flags.x != 0u || lighting_settings.values.y == 0u) {
        return vec3<f32>(0.0);
    }

    let transient_start = min(lighting_settings.local_shadows.w, lighting_settings.values.y);
    let transient_count = min(lighting_settings.values.y - transient_start, 32u);
    let dlight_bits = input.legacy_dlight_bits;
    if (transient_count == 0u || dlight_bits == 0u) {
        return vec3<f32>(0.0);
    }

    // ProjectDlightTexture2 derives a plane per triangle, rejects back-facing
    // triangles, and projects the radial dlight image into that plane. Fragment
    // derivatives recover the same constant triangle plane without rebuilding
    // the authored material shader.
    let dp_x = dpdx(input.world_position);
    let dp_y = dpdy(input.world_position);
    let plane_cross = cross(dp_x, dp_y);
    let plane_len_sq = dot(plane_cross, plane_cross);
    if (plane_len_sq < 1.0e-10) {
        return vec3<f32>(0.0);
    }
    var plane_normal = plane_cross * inverseSqrt(plane_len_sq);
    if (dot(plane_normal, input.world_normal) < 0.0) {
        plane_normal = -plane_normal;
    }

    // ProjectDlightTexture2 modulates its projected blob by the usable diffuse
    // stage when one is present. DinurdoJK's isolated pass is already issued
    // from the opaque receiver batch, so binding 0 is that stage's image and
    // input.uv carries the stage's generated/authored coordinates.
    let diffuse = textureSample(base_texture, base_sampler, input.uv).rgb;
    var contribution = vec3<f32>(0.0);

    for (var bit = 0u; bit < transient_count; bit += 1u) {
        if ((dlight_bits & (1u << bit)) == 0u) {
            continue;
        }
        let light = dynamic_lights[transient_start + bit];
        if (light.shadow.w < 0.5) {
            continue;
        }

        let radius = max(light.position_radius.w, 1.0);
        let radius_sq = radius * radius;
        let to_light = light.position_radius.xyz - input.world_position;
        let plane_distance = dot(plane_normal, to_light);
        // OpenJK rejects backfaces and triangles whose plane is outside radius.
        if (plane_distance <= 0.0 || plane_distance >= radius) {
            continue;
        }

        let projected_radius_sq = radius_sq - plane_distance * plane_distance;
        if (projected_radius_sq <= 1.0e-6) {
            continue;
        }
        let planar = to_light - plane_normal * plane_distance;
        let planar_distance = length(planar);
        let projected_radius = sqrt(projected_radius_sq);
        if (planar_distance >= projected_radius) {
            continue;
        }

        // ProjectDlightTexture2 uses fac = 0.5 / projected_radius and centers
        // the dlight texture at (0.5, 0.5). The fallback image is radial, so its
        // orientation in the triangle plane is immaterial; only radius matters.
        let projected_texel_radius = 0.5 * planar_distance / projected_radius;
        let dlight_image = sample_openjk_dlight_fallback(
            vec2<f32>(0.5 + projected_texel_radius, 0.5)
        );
        if (dlight_image <= 0.0) {
            continue;
        }

        let plane_modulate = 1.0 - (plane_distance * plane_distance) / radius_sq;
        let distance_to_light = length(to_light);
        if (distance_to_light <= 1.0e-5) { continue; }
        let emission_visibility = emitter_visibility(light, to_light / distance_to_light);
        if (emission_visibility <= 0.0) { continue; }
        // Preserve DinurdoJK's existing Legacy source-strength calibration while
        // replacing only the expensive full-material subtraction. The spatial
        // projection and triangle-plane attenuation above are OpenJK's.
        contribution += diffuse
            * light.color_intensity.rgb
            * light.color_intensity.a
            * plane_modulate
            * dlight_image
            * emission_visibility
            * 0.225;
    }
    return contribution;
}

fn clustered_dynamic_light_legacy(input: VertexOut, weather: WeatherSurfaceResponse) -> LegacyDynamicLighting {
    var result: LegacyDynamicLighting;
    result.diffuse_factor = vec3<f32>(0.0);
    result.wet_specular = vec3<f32>(0.0);
    if (!(ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS || ENABLE_AREA_LIGHTS) || camera.render_flags.x != 0u || lighting_settings.values.y == 0u) {
        return result;
    }
    if baked_local_receiver(input) && (lighting_settings.baked_sun_debug_mode.w & 2u) != 0u && !(ENABLE_WEATHER_SURFACE && (weather.wetness > 0.001 || weather.puddle > 0.001)) { return result; }
    let cluster = cluster_for_fragment(input);
    let normal = normalize(input.world_normal);
    let specular_normal = weather_reflection_normal(normal, weather);
    let view_direction = normalize(camera.camera_pos_time.xyz - input.world_position);
    for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {
        let light = dynamic_lights[cluster.indices[i]];
        let already_baked=local_light_already_baked(light,input);
        if already_baked && !(ENABLE_WEATHER_SURFACE && (weather.wetness > 0.001 || weather.puddle > 0.001)) { continue; }
        if (!direct_light_source_enabled(light)) {
            continue;
        }
        let delta = light.position_radius.xyz - input.world_position;
        let distance_to_light = length(delta);
        let radius = max(light.position_radius.w, 1.0);
        if (distance_to_light >= radius || distance_to_light <= 0.0001) {
            continue;
        }
        let light_direction = delta / distance_to_light;
        let ndotl = max(dot(normal, light_direction), 0.0);
        let emission_visibility = emitter_visibility(light, light_direction);
        let shadow_visibility = local_light_shadow_rgb(light, input.world_position, normal);
        var energy = vec3<f32>(0.0);
        if (ENABLE_MAP_LIGHT_SIMULATION && light.shadow.y > 0.5) {
            let surface_angle = q3map_surface_angle(light, ndotl);
            let light_scalar = q3map_light_scalar(light, distance_to_light, surface_angle);
            if (!q3map_fast_contribution_visible(light_scalar)) {
                continue;
            }
            energy = light.color_intensity.rgb
                * light_scalar
                * emission_visibility
                * shadow_visibility;
            result.diffuse_factor += energy;
        } else {
            let attenuation = local_light_attenuation(light, distance_to_light);
            energy = light.color_intensity.rgb
                * light.color_intensity.a
                * attenuation
                * emission_visibility
                * shadow_visibility;
            result.diffuse_factor += select(energy * ndotl * local_light_surface_scale(light),vec3(0.0),already_baked);
        }
        if (ENABLE_WEATHER_SURFACE && (weather.wetness > 0.001 || weather.puddle > 0.001) && ndotl > 0.0) {
            result.wet_specular += energy
                * weather_light_specular(specular_normal, view_direction, light_direction, weather);
        }
    }
    return result;
}

fn distribution_ggx(n: vec3<f32>, h: vec3<f32>, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let ndoth = max(dot(n, h), 0.0);
    let ndoth2 = ndoth * ndoth;
    let denominator = ndoth2 * (a2 - 1.0) + 1.0;
    return a2 / max(3.14159265359 * denominator * denominator, 1e-5);
}

fn geometry_schlick_ggx(ndotv: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return ndotv / max(ndotv * (1.0 - k) + k, 1e-5);
}

fn geometry_smith(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, roughness: f32) -> f32 {
    return geometry_schlick_ggx(max(dot(n, v), 0.0), roughness)
        * geometry_schlick_ggx(max(dot(n, l), 0.0), roughness);
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    // x^5 as a multiply chain: x is clamped to [0, 1], so this is exact and
    // avoids the log2/exp2 pair a general pow() lowers to.
    let x = clamp(1.0 - cos_theta, 0.0, 1.0);
    let x2 = x * x;
    return f0 + (vec3<f32>(1.0) - f0) * (x2 * x2 * x);
}

// Smith-GGX visibility term, G / (4 N.L N.V). The N.L and N.V numerators of the
// two Schlick-GGX factors cancel against the denominator, leaving no divide by
// N.L * N.V and no grazing-angle blow-up to clamp.
fn smith_ggx_visibility(ndotv: f32, ndotl: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return 0.25 / ((ndotv * (1.0 - k) + k) * (ndotl * (1.0 - k) + k));
}

fn companion_sample_roughness(uv: vec2<f32>) -> vec4<f32> {
    if (PBR_COMPANION_SAMPLER) { return textureSample(roughness_texture, pbr_sampler, uv); }
    return textureSample(roughness_texture, base_sampler, uv);
}

fn companion_sample_metallic(uv: vec2<f32>) -> vec4<f32> {
    if (PBR_COMPANION_SAMPLER) { return textureSample(metallic_texture, pbr_sampler, uv); }
    return textureSample(metallic_texture, base_sampler, uv);
}

fn companion_sample_specular(uv: vec2<f32>) -> vec4<f32> {
    if (PBR_COMPANION_SAMPLER) { return textureSample(specular_texture, pbr_sampler, uv); }
    return textureSample(specular_texture, base_sampler, uv);
}

struct PbrMaterialSample {
    roughness: f32,
    metallic: f32,
    ao: f32,
    dielectric_f0: vec3<f32>,
    specular_scale: f32,
};

fn sample_pbr_material(uv: vec2<f32>) -> PbrMaterialSample {
    var result: PbrMaterialSample;
    result.roughness = 0.65;
    result.metallic = 0.0;
    result.ao = 1.0;
    result.dielectric_f0 = vec3<f32>(0.04);
    result.specular_scale = 1.0;

    let packed_rmo = (material_flags() & 512u) != 0u;
    if (packed_rmo && (material_flags() & 16u) != 0u) {
        // rmoMap is R=roughness, G=metalness, B=ambient occlusion. Both
        // roughness_texture and metallic_texture point at the same packed image,
        // so sample it once instead of doing two identical fetches.
        let rmo = companion_sample_roughness(uv);
        result.roughness = rmo.r;
        result.metallic = rmo.g;
        result.ao = rmo.b;
        if ((material_flags() & 262144u) != 0u) {
            result.specular_scale = rmo.a;
        }
    } else {
        if ((material_flags() & 16u) != 0u) {
            result.roughness = companion_sample_roughness(uv).r;
        }
        if ((material_flags() & 64u) != 0u) {
            result.metallic = companion_sample_metallic(uv).r;
        }
    }

    if ((material_flags() & 128u) != 0u) {
        let specular = companion_sample_specular(uv);
        result.dielectric_f0 = clamp(specular.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        // Rend2's gloss workflow stores glossiness in specular alpha. Only use
        // it when no roughness/RMO map is present; authored roughness wins.
        if ((material_flags() & 16u) == 0u) {
            result.roughness = 1.0 - specular.a;
        }
    }

    if (material.pbr_params0.z >= 0.0) {
        result.roughness = material.pbr_params0.z;
    }
    if (material.pbr_params1.w > 0.5) {
        result.dielectric_f0 = clamp(material.pbr_params1.xyz, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    // Rend2 defines roughness=0 as mirror-like. Avoid the GGX singularity but
    // keep enough range for polished metals to produce tight highlights.
    result.roughness = clamp(result.roughness, 0.02, 1.0);
    result.metallic = clamp(result.metallic, 0.0, 1.0);
    result.ao = clamp(result.ao, 0.0, 1.0);
    return result;
}

struct PbrSurface {
    geometric_n: vec3<f32>,
    n: vec3<f32>,
    specular_n: vec3<f32>,
    v: vec3<f32>,
    roughness: f32,
    metallic: f32,
    ao: f32,
    f0: vec3<f32>,
};

fn default_pbr_surface(input: VertexOut, albedo: vec3<f32>, weather: WeatherSurfaceResponse) -> PbrSurface {
    var result: PbrSurface;
    let geometric = normalize(input.world_normal);
    result.geometric_n = geometric;
    result.n = geometric;
    result.specular_n = weather_reflection_normal(geometric, weather);
    result.v = normalize(camera.camera_pos_time.xyz - input.world_position);
    result.roughness = weather_wet_roughness(0.65, weather);
    result.metallic = 0.0;
    result.ao = 1.0;
    result.f0 = vec3<f32>(0.04);
    return result;
}

fn evaluate_pbr_surface(
    input: VertexOut,
    albedo: vec3<f32>,
    uv: vec2<f32>,
    weather: WeatherSurfaceResponse,
    shared_frame: mat3x3<f32>,
) -> PbrSurface {
    var result = default_pbr_surface(input, albedo, weather);
    result.n = cotangent_normal(input, uv, shared_frame);
    result.specular_n = weather_reflection_normal(result.n, weather);
    let pbr = sample_pbr_material(uv);
    result.roughness = weather_wet_roughness(pbr.roughness, weather);
    result.metallic = pbr.metallic;
    result.ao = pbr.ao;
    result.f0 = mix(pbr.dielectric_f0, albedo, pbr.metallic) * pbr.specular_scale;
    return result;
}

fn pbr_surface_for_path(
    input: VertexOut,
    albedo: vec3<f32>,
    uv: vec2<f32>,
    weather: WeatherSurfaceResponse,
    shared_frame: mat3x3<f32>,
    shared_surface: PbrSurface,
) -> PbrSurface {
    if (PBR_SHARED_MATERIAL_EVAL) {
        return shared_surface;
    }
    return evaluate_pbr_surface(input, albedo, uv, weather, shared_frame);
}

fn reflection_probe_specular(
    input: VertexOut,
    albedo: vec3<f32>,
    uv: vec2<f32>,
    weather: WeatherSurfaceResponse,
    shared_frame: mat3x3<f32>,
    shared_surface: PbrSurface,
) -> vec3<f32> {
    if (!ENABLE_PBR || lighting_settings.feature_flags.w < 1u || (material_flags() & 2048u) == 0u) {
        return vec3<f32>(0.0);
    }
    let surface = pbr_surface_for_path(input, albedo, uv, weather, shared_frame, shared_surface);
    let n = surface.specular_n;
    let v = surface.v;
    var reflection = reflect(-v, n);

    // Rend2's Radius is a parallax-proxy size. Approximate a local spherical
    // proxy so reflections translate as the viewer moves instead of behaving
    // like an infinitely distant skybox.
    let probe_center = material.reflection_probe.xyz;
    let probe_radius = max(material.reflection_probe.w, 1.0);
    let offset = input.world_position - probe_center;
    let b = dot(offset, reflection);
    let c = dot(offset, offset) - probe_radius * probe_radius;
    let discriminant = b * b - c;
    if (discriminant > 0.0) {
        let distance_to_proxy = -b + sqrt(discriminant);
        if (distance_to_proxy > 0.0) {
            let hit = input.world_position + reflection * distance_to_proxy;
            reflection = normalize(hit - probe_center);
        }
    }

    let fresnel = fresnel_schlick(max(dot(n, v), 0.0), surface.f0);
    // The DDS faces are authored in JKA/OpenGL world axes; convert this
    // renderer's [x,z,-y] direction back to JKA [x,y,z] for cube lookup.
    let jka_reflection = normalize(vec3<f32>(reflection.x, -reflection.z, reflection.y));
    let mip_count = max(textureNumLevels(reflection_probe_texture), 1u);
    let lod = surface.roughness * f32(mip_count - 1u);
    let environment = textureSampleLevel(
        reflection_probe_texture,
        reflection_probe_sampler,
        jka_reflection,
        lod
    ).rgb;
    // Roughness changes the reflection bandwidth through the prefiltered mip
    // selection; it should not also arbitrarily erase most reflection energy.
    return environment * fresnel * mix(0.35, 1.0, surface.ao);
}

fn static_baked_pbr(
    input: VertexOut,
    albedo: vec3<f32>,
    uv: vec2<f32>,
    lightmap_color: vec3<f32>,
    weather: WeatherSurfaceResponse,
    shared_frame: mat3x3<f32>,
    shared_surface: PbrSurface,
) -> StaticPbrLighting {
    var result: StaticPbrLighting;
    result.diffuse_factor = vec3<f32>(1.0);
    result.specular = vec3<f32>(0.0);
    if (!ENABLE_PBR || !ENABLE_DELUXE) {
        return result;
    }

    // Prefer a compiled q3map2 deluxemap. The CPU supplies a neutral 1x1
    // texture with alpha=0 when no companion exists, reserving alpha purely as
    // a validity marker. q3map2 stores the direction in JKA world axes; convert
    // it to this renderer's [x,z,-y] world basis before evaluating the BRDF.
    let deluxe_sample = textureSample(deluxemap_texture, lightmap_sampler, input.lightmap_uv);
    var compiled_deluxe = deluxe_sample.a > 0.5;
    var encoded_direction = vec4<f32>(0.0);
    var lighting = vec4<f32>(0.0);
    var l = vec3<f32>(0.0, 1.0, 0.0);
    if (compiled_deluxe) {
        let jka_direction = deluxe_sample.rgb * 2.0 - vec3<f32>(1.0);
        let renderer_direction = vec3<f32>(jka_direction.x, jka_direction.z, -jka_direction.y);
        if (dot(renderer_direction, renderer_direction) < 1e-5) {
            compiled_deluxe = false;
        } else {
            l = normalize(renderer_direction);
        }
    }

    // Rend2 falls back to an approximation derived from the BSP lightgrid on
    // maps that were not compiled with deluxemaps. Preserve that behavior so
    // old JKA maps still gain directional normal/specular response.
    if (!compiled_deluxe) {
        if (static_lightgrid.origin_enabled.w < 0.5) {
            return result;
        }
        if (PBR_VERTEX_LIGHTGRID) {
            encoded_direction = input.pbr_lightgrid_direction;
            lighting = input.pbr_lightgrid_lighting;
        } else {
            let jka_position = vec3<f32>(
                input.world_position.x,
                -input.world_position.z,
                input.world_position.y
            );
            let grid = (jka_position - static_lightgrid.origin_enabled.xyz) * static_lightgrid.inv_size.xyz;
            let bounds = static_lightgrid.bounds.xyz;
            if (any(grid < vec3<f32>(-0.5)) || any(grid > bounds - vec3<f32>(0.5))) {
                return result;
            }
            let uvw = (grid + vec3<f32>(0.5)) / max(bounds, vec3<f32>(1.0));
            encoded_direction = textureSample(static_lightgrid_direction, static_lightgrid_sampler, uvw);
            lighting = textureSample(static_lightgrid_lighting, static_lightgrid_sampler, uvw);
        }
        if (lighting.a < 0.01 || encoded_direction.a < 0.005) {
            return result;
        }
        l = normalize(encoded_direction.rgb * 2.0 - vec3<f32>(1.0));
    }

    let surface = pbr_surface_for_path(input, albedo, uv, weather, shared_frame, shared_surface);
    let geometric_ndotl = max(dot(surface.geometric_n, l), 0.0);
    let mapped_ndotl = max(dot(surface.n, l), 0.0);
    var ambient_fraction = 1.0;
    var specular_radiance = vec3<f32>(0.0);

    if (compiled_deluxe) {
        // Match Rend2's lightmap+deluxemap reconstruction: the baked lightmap
        // already contains the geometric N.L attenuation, so recover a directed
        // term and leave any unrecoverable energy as ambient before applying the
        // normal-mapped N.L. This avoids double-darkening grazing surfaces.
        //
        // recovered_direct = lightmap * direct_scale, and the leftover ambient is
        // the same fraction of the lightmap in every channel, so the whole
        // reconstruction reduces to scalars (no per-channel divides or luma
        // ratios): remapped / lightmap == remap_factor for all channels.
        let direct_scale = 1.0 / max(geometric_ndotl, 0.25);
        let ambient_share = max(1.0 - geometric_ndotl * direct_scale, 0.0);
        if ((material_flags() & 8u) != 0u) {
            let remap_factor = mapped_ndotl * direct_scale + ambient_share;
            result.diffuse_factor *= vec3<f32>(clamp(remap_factor, 0.25, 1.85));
        }
        ambient_fraction = clamp(ambient_share, 0.0, 1.0);
        specular_radiance = lightmap_color * direct_scale;
    } else {
        if ((material_flags() & 8u) != 0u) {
            let safe_geometric_ndotl = max(geometric_ndotl, 0.12);
            let ratio = clamp(mapped_ndotl / safe_geometric_ndotl, 0.25, 1.85);
            result.diffuse_factor *= vec3<f32>(mix(1.0, ratio, encoded_direction.a * 0.85));
        }
        ambient_fraction = 1.0 - encoded_direction.a;
        specular_radiance = lightmap_color * lighting.rgb * encoded_direction.a;
    }

    let ambient_ao = mix(1.0, surface.ao, ambient_fraction);
    result.diffuse_factor *= vec3<f32>((1.0 - surface.metallic) * ambient_ao);

    let deluxe_specular_scale = clamp(pbr_settings.deluxe.y, 0.0, 1.0);
    if (ENABLE_DELUXE_SPECULAR && deluxe_specular_scale > 0.0) {
        let n = surface.specular_n;
        let v = surface.v;
        let h = normalize(v + l);
        let ndotl = max(dot(n, l), 0.0);
        let ndotv = max(dot(n, v), 0.0);
        if (ndotl > 0.0 && ndotv > 0.0) {
            // Rend2 multiplies gloss by r_deluxeSpecular before rebuilding the
            // specular lobe, then scales the resulting specular energy by the
            // cvar as well. In roughness space, gloss scaling is equivalent to
            // moving roughness toward 1.0 as the control approaches zero.
            let deluxe_roughness = clamp(
                mix(1.0, surface.roughness, deluxe_specular_scale),
                0.04,
                1.0
            );
            let ndf = distribution_ggx(n, h, deluxe_roughness);
            let visibility = smith_ggx_visibility(ndotv, ndotl, deluxe_roughness);
            let f = fresnel_schlick(max(dot(h, v), 0.0), surface.f0);
            let specular = (ndf * visibility) * f;
            result.specular = specular * specular_radiance * ndotl * deluxe_specular_scale;
        }
    }
    return result;
}

// Bevy's non-bindless/single-texture LightProbe query and irradiance-volume
// sampler. The map adapter supplies one volume, but the runtime behavior matches
// Bevy's bounded unit-cube probe model, per-axis falloff weighting, intensity,
// clamp-to-texel-center atlas sampling and Valve ambient-cube N² basis.
struct BevyIrradianceVolumeSample {
    irradiance: vec3<f32>,
    weight: f32,
};

fn bevy_irradiance_volume_light(world_position: vec3<f32>, N: vec3<f32>) -> BevyIrradianceVolumeSample {
    var result: BevyIrradianceVolumeSample;
    result.irradiance = vec3<f32>(0.0);
    result.weight = 0.0;
    if (!ENABLE_IRRADIANCE_VOLUME || irradiance_volume_info.volume_enabled_pad.x < 0.5) {
        return result;
    }

    // Direct port of Bevy light_probe_iterator_next()'s probe-space containment
    // and falloff weight calculation. Probe space is a 1x1x1 cube at ±0.5.
    let probe_space_pos = (irradiance_volume_info.light_from_world * vec4<f32>(world_position, 1.0)).xyz;
    let falloff = max(irradiance_volume_info.falloff_intensity.xyz, vec3<f32>(0.0001));
    let axis_weights = clamp(
        (vec3<f32>(1.0) - 2.0 * abs(probe_space_pos)) / (2.0 * falloff),
        vec3<f32>(0.0),
        vec3<f32>(1.0)
    );
    let weight = min(min(axis_weights.x, axis_weights.y), axis_weights.z);
    if (weight == 0.0) {
        return result;
    }

    // Match Bevy: derive the logical probe resolution from the packed 3D
    // texture itself instead of duplicating dimensions in probe metadata.
    let atlas_resolution = vec3<f32>(textureDimensions(irradiance_volume));
    let resolution = atlas_resolution / vec3<f32>(1.0, 2.0, 3.0);
    // Direct port of Bevy irradiance_volume_light(): clamp to voxel centers to
    // keep hardware trilinear filtering from bleeding across packed cube faces.
    let stp = clamp(
        (probe_space_pos + vec3<f32>(0.5)) * resolution,
        vec3<f32>(0.5),
        resolution - vec3<f32>(0.5)
    );
    let uvw = stp / atlas_resolution;
    let normal = normalize(N);
    let neg_offset = select(vec3<f32>(0.0), vec3<f32>(0.5), normal < vec3<f32>(0.0));
    let uvw_x = uvw + vec3<f32>(0.0, neg_offset.x, 0.0);
    let uvw_y = uvw + vec3<f32>(0.0, neg_offset.y, 1.0 / 3.0);
    let uvw_z = uvw + vec3<f32>(0.0, neg_offset.z, 2.0 / 3.0);
    let rgb_x = textureSampleLevel(irradiance_volume, static_lightgrid_sampler, uvw_x, 0.0).rgb;
    let rgb_y = textureSampleLevel(irradiance_volume, static_lightgrid_sampler, uvw_y, 0.0).rgb;
    let rgb_z = textureSampleLevel(irradiance_volume, static_lightgrid_sampler, uvw_z, 0.0).rgb;
    let NN = normal * normal;

    // Bevy accumulates weighted probes then divides by total weight. With this
    // map's one source volume that normalization cancels the non-zero weight,
    // but retaining the exact weighted form keeps the probe-management semantics
    // faithful and makes the boundary/containment behavior identical.
    let weighted = (rgb_x * NN.x + rgb_y * NN.y + rgb_z * NN.z)
        * irradiance_volume_info.falloff_intensity.w * weight;
    result.irradiance = weighted / weight;
    result.weight = weight;
    return result;
}

fn sample_voxel_gi_volume(world_position: vec3<f32>, area: bool) -> vec3<f32> {
    let grid = (world_position - voxel_gi.origin_enabled.xyz) * voxel_gi.inv_cell.xyz;
    let bounds = voxel_gi.bounds_strength.xyz;
    if (any(grid < vec3<f32>(-0.5)) || any(grid > bounds - vec3<f32>(0.5))) {
        return vec3<f32>(0.0);
    }

    // Probe zero is stored at the first texel center. RGBA8 is intentional:
    // this is a low-frequency field, and hardware trilinear filtering turns
    // what used to be eight textureLoad operations into one texture sample.
    let uvw = (grid + vec3<f32>(0.5)) / bounds;
    if (area) {
        return textureSampleLevel(voxel_gi_area, static_lightgrid_sampler, uvw, 0.0).rgb * 8.0;
    }
    return textureSampleLevel(voxel_gi_point, static_lightgrid_sampler, uvw, 0.0).rgb * 8.0;
}

fn voxel_probe_indirect(
    input: VertexOut,
    albedo: vec3<f32>,
    uv: vec2<f32>,
    weather: WeatherSurfaceResponse,
    shared_frame: mat3x3<f32>,
    shared_surface: PbrSurface,
) -> vec3<f32> {
    if (!ENABLE_VOXEL_GI
        || camera.render_flags.x != 0u
        || voxel_gi.origin_enabled.w < 0.5) {
        return vec3<f32>(0.0);
    }

    var n = normalize(input.world_normal);
    var diffuse_weight = 1.0;
    var ambient_occlusion = 1.0;
    if (ENABLE_PBR) {
        let surface = pbr_surface_for_path(input, albedo, uv, weather, shared_frame, shared_surface);
        n = surface.n;
        if ((material_flags() & (16u | 64u)) != 0u) {
            diffuse_weight = 1.0 - surface.metallic;
            ambient_occlusion = surface.ao;
        }
    }
    let sample_position = input.world_position + n * voxel_gi.inv_cell.w * 0.72;
    // GI is its own feature. It consumes every baked radiance source in the
    // probe field, including emissive surfaces, regardless of whether their
    // *direct* area-light contribution is enabled.
    let irradiance = sample_voxel_gi_volume(sample_position, false)
        + sample_voxel_gi_volume(sample_position, true);
    return irradiance * albedo * diffuse_weight * ambient_occlusion * voxel_gi.bounds_strength.w;
}

fn clustered_dynamic_light_pbr(
    input: VertexOut,
    albedo: vec3<f32>,
    uv: vec2<f32>,
    weather: WeatherSurfaceResponse,
    shared_frame: mat3x3<f32>,
    shared_surface: PbrSurface,
) -> vec3<f32> {
    if (!(ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS || ENABLE_AREA_LIGHTS) || camera.render_flags.x != 0u || lighting_settings.values.y == 0u) {
        return vec3<f32>(0.0);
    }
    if baked_local_receiver(input) && (lighting_settings.baked_sun_debug_mode.w & 2u) != 0u && !dynamic_light_debug_enabled() {
        return vec3<f32>(0.0);
    }
    let cluster = cluster_for_fragment(input);
    let surface = pbr_surface_for_path(input, albedo, uv, weather, shared_frame, shared_surface);
    let n = surface.n;
    let specular_n = surface.specular_n;
    let v = surface.v;
    var illumination = vec3<f32>(0.0);
    for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {
        let light = dynamic_lights[cluster.indices[i]];
        if local_light_already_baked(light,input) { continue; }
        if (!direct_light_source_enabled(light)) {
            continue;
        }
        let delta = light.position_radius.xyz - input.world_position;
        let distance_to_light = length(delta);
        let radius = max(light.position_radius.w, 1.0);
        if (distance_to_light >= radius || distance_to_light <= 0.0001) {
            continue;
        }
        let l = delta / distance_to_light;
        let h = normalize(v + l);
        let ndotl = max(dot(n, l), 0.0);
        let emission_visibility = emitter_visibility(light, l);
        let shadow_visibility = local_light_shadow_rgb(light, input.world_position, n);
        if (ENABLE_MAP_LIGHT_SIMULATION && light.shadow.y > 0.5) {
            let surface_angle = q3map_surface_angle(light, ndotl);
            let light_scalar = q3map_light_scalar(light, distance_to_light, surface_angle);
            if (!q3map_fast_contribution_visible(light_scalar)) {
                continue;
            }
            // A compiled lightmap is diffuse irradiance modulation, not a runtime
            // microfacet light. Keep source-map q3map lights out of the PBR BRDF.
            let radiance = light.color_intensity.rgb
                * light_scalar
                * emission_visibility
                * shadow_visibility;
            illumination += albedo * radiance;
            continue;
        }
        if (ndotl <= 0.0) {
            continue;
        }
        let attenuation = local_light_attenuation(light, distance_to_light);
        let radiance = light.color_intensity.rgb
            * light.color_intensity.a
            * attenuation
            * emission_visibility
            * shadow_visibility
            * local_light_surface_scale(light);
        let ndf = distribution_ggx(specular_n, h, surface.roughness);
        let g = geometry_smith(specular_n, v, l, surface.roughness);
        let f = fresnel_schlick(max(dot(h, v), 0.0), surface.f0);
        let spec_ndotl = max(dot(specular_n, l), 0.0);
        let denominator = max(4.0 * max(dot(specular_n, v), 0.0) * max(spec_ndotl, 1e-4), 1e-4);
        let specular = (ndf * g / denominator) * f;
        let kd = (vec3<f32>(1.0) - f) * (1.0 - surface.metallic);
        let diffuse = kd * albedo / 3.14159265359;
        illumination += select(diffuse * radiance * ndotl,vec3(0.0),local_light_already_baked(light,input)) + specular * radiance * spec_ndotl;
    }
    return illumination;
}

fn legacy_srgb_channel_to_linear(value: f32) -> f32 {
    let c = clamp(value, 0.0, 1.0);
    return select(c / 12.92, pow((c + 0.055) / 1.055, 2.4), c > 0.04045);
}

fn legacy_authored_fog_color(color: vec3<f32>) -> vec3<f32> {
    // BSP fog parms are authored as the same display-space RGB values OpenJK
    // sent to its legacy framebuffer. DinurdoJK shades in linear space, so
    // convert the authored tint before blending; using it as linear makes the
    // fog visibly brighter/stronger than the original renderer.
    return vec3<f32>(
        legacy_srgb_channel_to_linear(color.r),
        legacy_srgb_channel_to_linear(color.g),
        legacy_srgb_channel_to_linear(color.b)
    );
}

fn openjk_fog_texel_alpha(texel_x: f32) -> f32 {
    // Exact global-row contents of OpenJK's 256x32 *fog texture:
    // R_FogFactor((x + 0.5)/256, 31.5/32) subtracts 1/512, multiplies S by
    // 8, indexes the 256-entry sqrt fog table, then stores 255*d in an 8-bit
    // alpha channel (C byte conversion truncates).
    let x = clamp(texel_x, 0.0, 255.0);
    let table_index = floor(clamp(x / 32.0, 0.0, 1.0) * 255.0);
    let table_value = sqrt(table_index / 255.0);
    return floor(table_value * 255.0) / 255.0;
}

fn openjk_global_fog_alpha(forward_normalized: f32) -> f32 {
    // OpenJK generates S = forward/(depth*8) + 1/512 and linearly samples a
    // 256-wide fog texture. Mapping normalized UV to texel space subtracts
    // 0.5, exactly cancelling that +1/512 half-texel bias.
    let texel = max(forward_normalized, 0.0) * 32.0;
    let x0 = floor(texel);
    let frac_x = fract(texel);
    let a0 = openjk_fog_texel_alpha(x0);
    let a1 = openjk_fog_texel_alpha(x0 + 1.0);
    return mix(a0, a1, frac_x);
}

// jaPRO SP-physics jump-height helper (jump_shade.rs). World Y is JKA Z, so heights
// compare directly. camera.jump_shade: x jump-line floor height, y highest reachable
// floor height, z gradient range below the line, w strength (0 while grounded).
// Compiled out unless the pipeline variant carries ENABLE_JUMP_SHADE.
fn apply_jump_shade(color: vec4<f32>, world_position: vec3<f32>) -> vec4<f32> {
    if (!ENABLE_JUMP_SHADE || camera.jump_shade.w <= 0.0
        || (material_flags() & MATERIAL_OPAQUE_STAGE) == 0u) {
        return color;
    }
    // Only tint surfaces flat enough to land on. The facet slope comes from
    // world-position derivatives so bad smoothed vertex normals cannot mislabel
    // walls as floors; 0.71 is JKA's minimum walkable normal Z.
    let facet = cross(dpdx(world_position), dpdy(world_position));
    if (abs(facet.y) <= 0.71 * length(facet)) {
        return color;
    }
    let line = camera.jump_shade.x;
    var tint: vec3<f32>;
    var strength = camera.jump_shade.w;
    if (world_position.y < line) {
        // Landing below the jump line keeps speed: green at the line (ideal),
        // through yellow to red over the gradient range, then alternating
        // red/pink 16-unit bands so deeper drops still read.
        let depth = line - world_position.y;
        let range = camera.jump_shade.z;
        if (depth <= range) {
            let t = depth / range;
            tint = vec3<f32>(min(t * 2.0, 1.0), min((1.0 - t) * 2.0, 1.0), 0.0);
        } else if ((i32(floor((depth - range) / 16.0)) & 1) == 0) {
            tint = vec3<f32>(1.0, 0.0, 0.0);
        } else {
            tint = vec3<f32>(1.0, 0.45, 0.75);
        }
    } else {
        // At or above the line but still reachable: the speed-penalty zone. Dim
        // blue brightening to cyan at the apex, the highest floor you can reach.
        let above_start = line + 1.0;
        let apex = camera.jump_shade.y;
        if (world_position.y <= above_start || world_position.y > apex) {
            return color;
        }
        let u = clamp((world_position.y - above_start) / max(apex - above_start, 1.0), 0.0, 1.0);
        tint = mix(vec3<f32>(0.0, 0.0, 0.35), vec3<f32>(0.0, 0.7, 0.9), u);
        strength = strength * 0.85;
    }
    return vec4<f32>(mix(color.rgb, tint, strength), color.a);
}

fn apply_legacy_fog(color: vec4<f32>, world_position: vec3<f32>) -> vec4<f32> {
    let has_map_fog = surface_fog.color_depth.a > 0.0;
    let has_override = legacy_fog.values.y > 0.001;
    let map_has_authored_fog = legacy_fog.values.z > 0.5;
    let drawfog_mode = legacy_fog.values.w;
    if (legacy_fog.values.x < 0.5) {
        return color;
    }
    // OpenJK r_drawfog 1 never folds fog into material stages: every fogged
    // surface is redrawn by the explicit fog pass after its material stages.
    if (drawfog_mode < 1.5) {
        return color;
    }
    // Legacy 2 uses fixed-function-style per-stage fog when the material can
    // reproduce it exactly; that path is required for OpenJK's black/white fog
    // overrides on additive/filter stages. Complex destination-dependent
    // materials still fall back to the post-material geometry pass.
    if (drawfog_mode >= 1.5
        && map_has_authored_fog
        && has_map_fog
        && surface_fog.flags.z < 0.5) {
        return color;
    }
    // If this map has authored fog, the strength control scales that authored
    // assignment only. It must not turn unfogged surfaces into manual fog.
    if (map_has_authored_fog && !has_map_fog) {
        return color;
    }
    if (!map_has_authored_fog && !has_override) {
        return color;
    }
    // Zero is the explicit "use the map" sentinel. Non-zero values override
    // the authored distance-to-opaque. A map without fog gets a neutral global
    // fallback so the Legacy mode remains useful as a manual fog control.
    var fog_color = select(
        vec3<f32>(0.55, 0.62, 0.70),
        legacy_authored_fog_color(surface_fog.color_depth.rgb),
        has_map_fog
    );
    // r_drawfog 2 mirrors OpenJK's hardware-fog stage handling: blend stages
    // use a neutral fog color so applying fog per material stage preserves the
    // fixed-function blend equation.
    if (surface_fog.flags.y > 1.5) {
        fog_color = vec3<f32>(1.0);
    } else if (surface_fog.flags.y > 0.5) {
        fog_color = vec3<f32>(0.0);
    }
    let authored_depth = select(1024.0, surface_fog.color_depth.a, has_map_fog);
    let radial_distance = distance(camera.camera_pos_time.xyz, world_position);
    let radial_normalized = radial_distance / max(authored_depth, 0.001);
    let camera_forward = normalize(camera.camera_forward.xyz);
    let forward_distance = max(
        dot(world_position - camera.camera_pos_time.xyz, camera_forward),
        0.0
    );
    let forward_normalized = forward_distance / max(authored_depth, 0.001);
    let authored_scale = select(1.0, legacy_fog.values.y, has_override);
    // OpenJK's r_drawfog 2 global fog uses GL_EXP2 and chooses density so the
    // transmittance is 1/255 at depthForOpaque.
    let global_map_amount = 1.0 - exp(
        -5.5412635 * authored_scale * forward_normalized * forward_normalized
    );
    let local_map_amount = clamp(radial_normalized * authored_scale, 0.0, 1.0);
    let map_amount = select(
        local_map_amount,
        clamp(global_map_amount, 0.0, 1.0),
        surface_fog.flags.x > 0.5
    );
    // On a map with no authored fog, a non-zero slider remains our explicit
    // manual fog control and preserves its existing calibration.
    let manual_amount = 1.0 - exp(-radial_normalized * legacy_fog.values.y * 0.26);
    let amount = select(
        clamp(manual_amount, 0.0, 1.0),
        map_amount,
        has_map_fog
    );
    return vec4<f32>(mix(color.rgb, fog_color, amount), color.a);
}

// Legacy OpenGL dithers fixed-point framebuffer writes (GL_DITHER). The fog
// pass is blended in fixed function (SrcAlpha, OneMinusSrcAlpha), so the shader
// cannot dither the blended pixel itself; instead jitter the blend alpha so the
// 8-bit rounding of the result becomes ordered noise instead of contour bands.
// A change of d in alpha moves the result by d * |fog - scene|. The scene pixel
// is unknown here, so assume a typical dark-ish scene luminance and size d so
// the result moves by about one 8-bit sRGB code step (the same linear LSB
// model post_fast.wgsl uses). One scalar offset keeps neutral fog hues.
fn legacy_fog_dither_alpha(fog: vec4<f32>, pixel_position: vec2<f32>) -> vec4<f32> {
    if (fog.a <= 0.0 || fog.a >= 1.0) {
        return fog;
    }
    let bayer4 = array<f32, 16>(
         0.0,  8.0,  2.0, 10.0,
        12.0,  4.0, 14.0,  6.0,
         3.0, 11.0,  1.0,  9.0,
        15.0,  7.0, 13.0,  5.0
    );
    let x = u32(pixel_position.x) & 3u;
    let y = u32(pixel_position.y) & 3u;
    let ordered = (bayer4[y * 4u + x] + 0.5) / 16.0 - 0.5;
    let fog_luma = clamp(dot(fog.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    let assumed_scene_luma = 0.15;
    let result_luma = mix(assumed_scene_luma, fog_luma, fog.a);
    let dark_lsb = 1.0 / (255.0 * 12.92);
    let bright_lsb = 2.275 / 255.0;
    let linear_lsb = mix(dark_lsb, bright_lsb, sqrt(result_luma));
    let color_span = max(abs(fog_luma - assumed_scene_luma), 0.12);
    let alpha_jitter = ordered * linear_lsb / color_span;
    return vec4<f32>(fog.rgb, clamp(fog.a + alpha_jitter, 0.0, 1.0));
}

fn legacy_separate_fog(world_position: vec3<f32>) -> vec4<f32> {
    let has_map_fog = surface_fog.color_depth.a > 0.0;
    let has_override = legacy_fog.values.y > 0.001;
    let map_has_authored_fog = legacy_fog.values.z > 0.5;
    let drawfog_mode = legacy_fog.values.w;
    if (legacy_fog.values.x < 0.5 || drawfog_mode < 0.5) {
        return vec4<f32>(0.0);
    }
    if (map_has_authored_fog && !has_map_fog) {
        return vec4<f32>(0.0);
    }
    if (!map_has_authored_fog && !has_override) {
        return vec4<f32>(0.0);
    }
    // Legacy 1 global opaque/depth-writing geometry is fogged later in
    // display-space to match OpenJK's old gamma/LDR framebuffer blend. Local
    // fog and transparent-only global geometry still use this pass. Legacy 2
    // uses it only when the material cannot safely reproduce fixed-function
    // fog per stage.
    let legacy1_geometry = drawfog_mode < 1.5
        && (surface_fog.flags.x < 0.5 || surface_fog.flags.w < 0.5);
    let legacy2_geometry = drawfog_mode >= 1.5
        && map_has_authored_fog
        && has_map_fog
        && surface_fog.flags.z < 0.5;
    let separate_pass = legacy1_geometry || legacy2_geometry;
    if (!separate_pass) {
        return vec4<f32>(0.0);
    }
    return legacy_fog_color_amount(world_position);
}

// Fog colour (rgb) and amount (a) for this surface's fog assignment. Shared by
// the separate BSP fog pass and the promoted ocean so both use one equation.
fn legacy_fog_color_amount(world_position: vec3<f32>) -> vec4<f32> {
    let has_map_fog = surface_fog.color_depth.a > 0.0;
    let has_override = legacy_fog.values.y > 0.001;
    let drawfog_mode = legacy_fog.values.w;
    let fog_color = select(
        vec3<f32>(0.55, 0.62, 0.70),
        legacy_authored_fog_color(surface_fog.color_depth.rgb),
        has_map_fog
    );
    let authored_depth = select(1024.0, surface_fog.color_depth.a, has_map_fog);
    let radial_distance = distance(camera.camera_pos_time.xyz, world_position);
    let radial_normalized = radial_distance / max(authored_depth, 0.001);
    let camera_forward = normalize(camera.camera_forward.xyz);
    let forward_distance = max(
        dot(world_position - camera.camera_pos_time.xyz, camera_forward),
        0.0
    );
    let forward_normalized = forward_distance / max(authored_depth, 0.001);
    let authored_scale = select(1.0, legacy_fog.values.y, has_override);

    var amount = 0.0;
    if (has_map_fog) {
        if (surface_fog.flags.x > 0.5) {
            if (drawfog_mode >= 1.5) {
                // Legacy 2 always uses the OpenJK/JKA GL_EXP2 curve. Because
                // every authored BSP surface reaches this same pass, material
                // complexity can no longer change the apparent fog strength.
                let scaled = forward_normalized * authored_scale;
                amount = clamp(1.0 - exp(-5.5412635 * scaled * scaled), 0.0, 1.0);
            } else {
                // Match RB_FogPass' filtered *fog texture, including its sqrt
                // table and 8-bit alpha quantization.
                amount = openjk_global_fog_alpha(forward_normalized * authored_scale);
            }
        } else {
            // The current BSP fog uniform does not yet carry the local brush
            // clipping plane, so retain this renderer's established local-fog
            // distance curve while moving it to OpenJK's correct separate pass.
            amount = clamp(radial_normalized * authored_scale, 0.0, 1.0);
        }
    } else {
        amount = clamp(
            1.0 - exp(-radial_normalized * legacy_fog.values.y * 0.26),
            0.0,
            1.0
        );
    }
    return vec4<f32>(fog_color, amount);
}

// The promoted GodotOcean surface is drawn by its own pipeline after (and
// displaced away from) the flat BSP water, so neither the in-stage path nor the
// depth-EQUAL separate fog pass can reach it. Like entities and grass, it fogs
// itself here with the same curves as legacy_separate_fog.
fn apply_ocean_legacy_fog(color: vec4<f32>, world_position: vec3<f32>) -> vec4<f32> {
    let has_map_fog = surface_fog.color_depth.a > 0.0;
    let has_override = legacy_fog.values.y > 0.001;
    let map_has_authored_fog = legacy_fog.values.z > 0.5;
    let drawfog_mode = legacy_fog.values.w;
    if (legacy_fog.values.x < 0.5 || drawfog_mode < 0.5) {
        return color;
    }
    if (map_has_authored_fog && !has_map_fog) {
        return color;
    }
    if (!map_has_authored_fog && !has_override) {
        return color;
    }
    // Legacy 1 authored global fog is composited in post from the depth prepass.
    if (drawfog_mode < 1.5 && has_map_fog && surface_fog.flags.x > 0.5) {
        return color;
    }
    let fog = legacy_fog_color_amount(world_position);
    return vec4<f32>(mix(color.rgb, fog.rgb, fog.a), color.a);
}

fn material_matches_planar_plane(selected_plane: vec4<f32>) -> bool {
    let material_len2 = dot(material.planar_plane.xyz, material.planar_plane.xyz);
    if (material_len2 <= 0.5) {
        return false;
    }
    let material_n = normalize(material.planar_plane.xyz);
    let selected_n = normalize(selected_plane.xyz);
    let alignment = dot(material_n, selected_n);
    let plane_delta = select(
        abs(material.planar_plane.w + selected_plane.w),
        abs(material.planar_plane.w - selected_plane.w),
        alignment >= 0.0
    );
    return abs(alignment) > 0.999 && plane_delta < 2.0;
}

fn planar_slot_for_surface(input: VertexOut, promoted_environment: bool) -> i32 {
    if (!ENABLE_PLANAR_REFLECTIONS || planar_reflection.viewport.z <= 0.5) {
        return -1;
    }
    let active_count = min(u32(planar_reflection.viewport.z + 0.5), 4u);
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        if (slot >= active_count) {
            break;
        }
        let selected_plane = planar_reflection.planes[slot];
        let selected_n_len2 = dot(selected_plane.xyz, selected_plane.xyz);
        if (selected_n_len2 <= 0.5) {
            continue;
        }
        if (promoted_environment) {
            // The CPU selected strictly planar tcGen environment faces. Match
            // the actual fragment plane so coarse/FULL PVS sub-batches can use
            // the same slot without depending on batch identity.
            let selected_n = normalize(selected_plane.xyz);
            let distance_to_plane = abs(dot(selected_n, input.world_position) + selected_plane.w);
            if (distance_to_plane < 0.5) {
                return i32(slot);
            }
        } else if (material_matches_planar_plane(selected_plane)) {
            return i32(slot);
        }
    }
    return -1;
}

fn load_planar_texel(pixel: vec2<i32>, maximum: vec2<i32>, slot: i32) -> vec4<f32> {
    return textureLoad(
        planar_reflection_texture,
        clamp(pixel, vec2<i32>(0), maximum),
        slot,
        0
    );
}

fn sample_planar_reflection(
    fragment_position: vec2<f32>,
    slot: i32,
    soften_environment: bool
) -> vec4<f32> {
    let dimensions_u = textureDimensions(planar_reflection_texture);
    let dimensions = vec2<f32>(f32(dimensions_u.x), f32(dimensions_u.y));
    let main_viewport = max(planar_reflection.viewport.xy, vec2<f32>(1.0));
    let pixel = vec2<i32>(floor(fragment_position * dimensions / main_viewport));
    let maximum = vec2<i32>(
        max(i32(dimensions_u.x) - 1, 0),
        max(i32(dimensions_u.y) - 1, 0)
    );
    let center = load_planar_texel(pixel, maximum, slot);
    if (!soften_environment) {
        return center;
    }

    // Promoted environment stages should read as rough polished material, not
    // a literal mirror. Use the existing linear sampler as a wide five-tap
    // filter. At the quarter-resolution environment target a 3-texel radius is
    // roughly a 12-pixel full-resolution footprint, while retaining only five
    // reflection samples per fragment. Authored mirrors keep the exact texel
    // path above.
    let uv = clamp(
        fragment_position / main_viewport,
        vec2<f32>(0.0),
        vec2<f32>(1.0)
    );
    let texel = 1.0 / max(dimensions, vec2<f32>(1.0));
    let radius = texel * 3.0;
    let softened =
        textureSampleLevel(planar_reflection_texture, planar_reflection_sampler, uv, slot, 0.0) * 0.36
        + textureSampleLevel(planar_reflection_texture, planar_reflection_sampler, uv + vec2<f32>(radius.x, 0.0), slot, 0.0) * 0.16
        + textureSampleLevel(planar_reflection_texture, planar_reflection_sampler, uv - vec2<f32>(radius.x, 0.0), slot, 0.0) * 0.16
        + textureSampleLevel(planar_reflection_texture, planar_reflection_sampler, uv + vec2<f32>(0.0, radius.y), slot, 0.0) * 0.16
        + textureSampleLevel(planar_reflection_texture, planar_reflection_sampler, uv - vec2<f32>(0.0, radius.y), slot, 0.0) * 0.16;
    return vec4<f32>(softened.rgb * 0.90, softened.a);
}

fn ocean_cubic_weights(a: f32) -> vec4<f32> {
    let a2 = a*a;
    let a3 = a2*a;
    return vec4<f32>(
        -a3 + a2*3.0 - a*3.0 + 1.0,
        a3*3.0 - a2*6.0 + 4.0,
        -a3*3.0 + a2*3.0 + a*3.0 + 1.0,
        a3
    ) / 6.0;
}

// Fast third-order B-spline filtering, translated directly from the upstream
// GodotOceanWaves shader / GPU Gems 2 implementation.
fn ocean_texture_bicubic(coords: vec3<f32>) -> vec4<f32> {
    let dims_u = textureDimensions(ocean_normal, 0);
    let dims = vec2<f32>(f32(dims_u.x), f32(dims_u.y));
    let dims_inv = vec2<f32>(1.0) / dims;
    let uv_texel = coords.xy * dims + vec2<f32>(0.5);
    let fuv = fract(uv_texel);
    let wx = ocean_cubic_weights(fuv.x);
    let wy = ocean_cubic_weights(fuv.y);
    let g = vec4<f32>(
        wx.x + wx.y,
        wx.z + wx.w,
        wy.x + wy.y,
        wy.z + wy.w
    );
    let base = floor(uv_texel);
    let h = vec4<f32>(
        wx.y / max(g.x, 1e-8) - 1.5 + base.x,
        wx.w / max(g.y, 1e-8) + 0.5 + base.x,
        wy.y / max(g.z, 1e-8) - 1.5 + base.y,
        wy.w / max(g.w, 1e-8) + 0.5 + base.y
    ) * vec4<f32>(dims_inv.x, dims_inv.x, dims_inv.y, dims_inv.y);
    let w = vec2<f32>(
        g.x / max(g.x + g.y, 1e-8),
        g.z / max(g.z + g.w, 1e-8)
    );
    let layer = i32(coords.z);
    let bottom = mix(
        textureSampleLevel(ocean_normal, ocean_sampler, vec2<f32>(h.y, h.w), layer, 0.0),
        textureSampleLevel(ocean_normal, ocean_sampler, vec2<f32>(h.x, h.w), layer, 0.0),
        w.x
    );
    let top = mix(
        textureSampleLevel(ocean_normal, ocean_sampler, vec2<f32>(h.y, h.z), layer, 0.0),
        textureSampleLevel(ocean_normal, ocean_sampler, vec2<f32>(h.x, h.z), layer, 0.0),
        w.x
    );
    return mix(bottom, top, w.y);
}

fn ocean_smith_masking_shadowing(cos_theta: f32, alpha: f32) -> f32 {
    let safe_sin = sqrt(max(1.0 - cos_theta*cos_theta, 1e-8));
    let a = cos_theta / max(alpha * safe_sin, 1e-8);
    let a_sq = a*a;
    return select(0.0, (1.0 - 1.259*a + 0.396*a_sq) / max(3.535*a + 2.181*a_sq, 1e-8), a < 1.6);
}

fn ocean_ggx_distribution(cos_theta: f32, alpha: f32) -> f32 {
    let a_sq = alpha*alpha;
    let d = 1.0 + (a_sq - 1.0) * cos_theta * cos_theta;
    return a_sq / max(3.14159265359 * d*d, 1e-8);
}

// Godot's environment/specular response: authored planar reflections first,
// then a configured reflection probe, then the actual map skybox. Directional
// environment sampling obeys the renderer's reflection-quality setting; map-sky
// diffuse ambient remains available even with reflections disabled.
fn ocean_environment(input: VertexOut, n: vec3<f32>, view_dir: vec3<f32>, ibl_roughness: f32) -> vec3<f32> {
    if (ENABLE_PLANAR_REFLECTIONS && planar_reflection.viewport.z > 0.5) {
        let slot = planar_slot_for_surface(input, false);
        if (slot >= 0) {
            let tilt = n - vec3<f32>(0.0, sign(n.y), 0.0);
            let perturb = (camera.view_proj * vec4<f32>(tilt, 0.0)).xy * vec2<f32>(1.0,-1.0);
            let offset = clamp(perturb, vec2<f32>(-1.0), vec2<f32>(1.0)) * planar_reflection.viewport.xy * ocean_optics.params.y;
            return sample_planar_reflection(input.clip_position.xy + offset, slot, ibl_roughness > 0.25).rgb;
        }
    }
    if (ENABLE_PBR && lighting_settings.feature_flags.w >= 1u && (material_flags() & 2048u) != 0u) {
        let reflection = reflect(-view_dir, n);
        let jka_reflection = normalize(vec3<f32>(reflection.x, -reflection.z, reflection.y));
        let mip_count = max(textureNumLevels(reflection_probe_texture), 1u);
        let lod = clamp(ibl_roughness, 0.0, 1.0) * f32(mip_count - 1u);
        return textureSampleLevel(
            reflection_probe_texture,
            reflection_probe_sampler,
            jka_reflection,
            lod
        ).rgb;
    }
    if (lighting_settings.feature_flags.w >= 1u && ocean.sky_ambient.w > 0.5) {
        return sample_map_sky_renderer(reflect(-view_dir, n), ibl_roughness);
    }
    return vec3<f32>(0.0);
}

fn ocean_dynamic_lighting(
    input: VertexOut,
    n: vec3<f32>,
    view_dir: vec3<f32>,
    albedo: vec3<f32>,
    roughness: f32,
) -> vec3<f32> {
    if (!(ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS || ENABLE_AREA_LIGHTS) || camera.render_flags.x != 0u || lighting_settings.values.y == 0u) {
        return vec3<f32>(0.0);
    }
    let cluster = cluster_for_fragment(input);
    var illumination = vec3<f32>(0.0);
    let f0 = vec3<f32>(0.02);
    for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {
        let light = dynamic_lights[cluster.indices[i]];
        if (!direct_light_source_enabled(light)) {
            continue;
        }
        let delta = light.position_radius.xyz - input.world_position;
        let distance_to_light = length(delta);
        let radius = max(light.position_radius.w, 1.0);
        if (distance_to_light >= radius || distance_to_light <= 0.0001) {
            continue;
        }
        let l = delta / distance_to_light;
        let ndotl = max(dot(n, l), 0.0);
        let h = normalize(view_dir + l);
        if (ENABLE_MAP_LIGHT_SIMULATION && light.shadow.y > 0.5) {
            let surface_angle = q3map_surface_angle(light, ndotl);
            let light_scalar = q3map_light_scalar(light, distance_to_light, surface_angle);
            if (!q3map_fast_contribution_visible(light_scalar)) {
                continue;
            }
            illumination += albedo
                * light.color_intensity.rgb
                * light_scalar
                * emitter_visibility(light, l)
                * local_light_shadow_rgb(light, input.world_position, n);
            continue;
        }
        if (ndotl <= 0.0) {
            continue;
        }
        let attenuation = local_light_attenuation(light, distance_to_light);
        let radiance = light.color_intensity.rgb
            * light.color_intensity.a
            * attenuation
            * emitter_visibility(light, l)
            * local_light_shadow_rgb(light, input.world_position, n)
            * local_light_surface_scale(light);
        let ndf = distribution_ggx(n, h, roughness);
        let g = geometry_smith(n, view_dir, l, roughness);
        let f = fresnel_schlick(max(dot(h, view_dir), 0.0), f0);
        let ndotv = max(dot(n, view_dir), 1e-4);
        let specular = (ndf * g / max(4.0 * ndotv * ndotl, 1e-4)) * f;
        let diffuse = (vec3<f32>(1.0) - f) * albedo / 3.14159265359;
        illumination += (diffuse + specular) * radiance * ndotl;
    }
    return illumination;
}

fn ocean_indirect_lighting(input: VertexOut, n: vec3<f32>, albedo: vec3<f32>) -> vec3<f32> {
    // Baseline comes from this map's sky, not a fixed reference-demo color.
    var indirect = albedo * ocean.sky_ambient.rgb * 0.68;

    // When the user enables the Bevy irradiance-volume path, let the BSP
    // lightgrid own diffuse indirect illumination. This makes indoor/covered
    // water respond to the actual map instead of remaining open-sky bright.
    if (ENABLE_IRRADIANCE_VOLUME && irradiance_volume_info.volume_enabled_pad.x > 0.5) {
        let probe = bevy_irradiance_volume_light(input.world_position, n);
        if (probe.weight > 0.0) {
            indirect = probe.irradiance * albedo;
        }
    }

    // The optional voxel GI setting contributes the same low-frequency bounced
    // point/area radiance to ocean that it contributes to ordinary surfaces.
    if (ENABLE_VOXEL_GI && voxel_gi.origin_enabled.w > 0.5 && camera.render_flags.x == 0u) {
        let sample_position = input.world_position + n * voxel_gi.inv_cell.w * 0.72;
        let gi = sample_voxel_gi_volume(sample_position, false)
            + sample_voxel_gi_volume(sample_position, true);
        indirect += gi * albedo * voxel_gi.bounds_strength.w;
    }
    return indirect;
}

// Reproduce the authored blend against the captured scene. Transparency may
// live in RGB (screen/additive/modulate), independent of texture/alphaGen alpha.
fn ocean_authored_blend_factor(id: u32, src: vec4<f32>, dst: vec4<f32>) -> vec3<f32> {
    switch id {
        case 0u: { return vec3<f32>(0.0); }
        case 1u: { return vec3<f32>(1.0); }
        case 2u: { return src.rgb; }
        case 3u: { return vec3<f32>(1.0)-src.rgb; }
        case 4u: { return vec3<f32>(src.a); }
        case 5u: { return vec3<f32>(1.0-src.a); }
        case 6u: { return dst.rgb; }
        case 7u: { return vec3<f32>(1.0)-dst.rgb; }
        case 8u: { return vec3<f32>(dst.a); }
        case 9u: { return vec3<f32>(1.0-dst.a); }
        default: { return vec3<f32>(min(src.a,1.0-dst.a)); }
    }
}

fn ocean_authored_blend(src: vec4<f32>, dst: vec4<f32>, factors: vec2<u32>) -> vec3<f32> {
    return src.rgb * ocean_authored_blend_factor(factors.x,src,dst)
        + dst.rgb * ocean_authored_blend_factor(factors.y,src,dst);
}

// Wind-foam transport is one shared field. At slower mesh LODs, its *visible*
// transport state is cached on the same 32/24 Hz snapshots as displacement.
// The expensive transport compute still runs once globally at the live rate.
fn ocean_windrow_lod(index: u32, live_offset: u32, middle: f32, far: f32) -> f32 {
    if (far >= 1.0) {return ocean_windrow_far[index];}
    if (middle >= 1.0 && far <= 0.0) {return ocean_windrow_medium[index];}
    if (middle <= 0.0) {return ocean_windrow[live_offset+index];}
    if (far > 0.0) {return mix(ocean_windrow_medium[index],ocean_windrow_far[index],far);}
    return mix(ocean_windrow[live_offset+index],ocean_windrow_medium[index],middle);
}

fn ocean_surface_sample(input: VertexOut, below: bool) -> vec4<f32> {
    let units_per_meter = max(ocean.ocean_info.z, 0.001);
    let world_m = input.ocean_uv_m;
    // True 3D view distance: directly above the water is far, not distance zero.
    let dist = distance(input.world_position, camera.camera_pos_time.xyz) / units_per_meter;
    let map_size = ocean.ocean_info.y;

    // Screen-space FFT LOD. The generated normal arrays intentionally have one
    // mip. Once a cascade spans multiple source texels per screen pixel, keeping
    // it is wasted bandwidth and aliases; fade it out and leave the analytic
    // swell as the distant normal reference. This follows altitude, FOV and
    // resolution automatically rather than inventing another horizontal ring.
    let metres_per_pixel = max(length(dpdx(world_m)), length(dpdy(world_m)));

    // An opt-in one-switch bundle for comparing the restrained BSP-derived
    // appearance to the more complete FFT/foam presentation. Individual A/B
    // controls below still work with this switch off, and can be increased
    // further when a bundle is active. No FFT, map topology or material cache
    // needs rebuilding to toggle this runtime setting.
    let visual_mode = ocean.appearance_c.x;
    let tuned_foam_detail = max(ocean.appearance_a.x, select(0.0, 1.0, visual_mode > 0.5));
    // The bundle keeps both promoted-pool foam gates: they are what keeps a
    // near-flat pond from being blanketed by the shared FFT foam field.
    let tuned_foam_activity = ocean.appearance_a.y;
    let tuned_foam_gain = max(ocean.appearance_a.z,
        select(0.0, select(1.35, 1.15, visual_mode < 1.5), visual_mode > 0.5));

    // Preserve the upstream three-cascade sum while a cascade is resolvable.
    // Near water keeps the faithful bicubic/bilinear response. Distant or tiny
    // projected detail falls to one bilinear tap, then zero taps.
    var gradient_foam = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let scale = ocean.map_scales[i];
        let coords = vec3<f32>(world_m * vec2<f32>(1.0,-1.0) * scale.xy, f32(i));
        let texels_per_metre = map_size * max(scale.x, scale.y);
        let texels_per_pixel = metres_per_pixel * texels_per_metre;
        let cascade_weight = 1.0 - smoothstep(0.75, 2.25, texels_per_pixel);
        let detail_mode = tuned_foam_detail;
        if ((cascade_weight > 0.001 || detail_mode > 0.5)
            && (abs(scale.w) > 0.0 || ocean.foam_color.a > 0.5)) {
            let bilinear_weight = min(1.0, map_size * min(scale.x, scale.y) * 0.1);
            var sample = vec4<f32>(0.0);
            if (bilinear_weight >= 0.94 || (texels_per_pixel >= 0.70 && detail_mode < 1.5)) {
                sample = textureSampleLevel(ocean_normal, ocean_sampler, coords.xy, i32(i), 0.0);
            } else {
                let bicubic = ocean_texture_bicubic(coords);
                if (bilinear_weight <= 0.01) {
                    sample = bicubic;
                } else {
                    let bilinear = textureSampleLevel(ocean_normal, ocean_sampler, coords.xy, i32(i), 0.0);
                    sample = mix(bicubic, bilinear, bilinear_weight);
                }
            }
            // Mode 1 keeps foam even when normals become subpixel; mode 2
            // keeps the complete Godot cascade response, including gradients.
            let normal_weight = select(cascade_weight, 1.0, detail_mode > 1.5);
            let foam_weight = select(cascade_weight, 1.0, detail_mode > 0.5);
            gradient_foam += vec3<f32>(sample.x * scale.w * normal_weight,
                -sample.y * scale.w * normal_weight, sample.w * foam_weight);
        }
    }

    let phase = dot(world_m, ocean.swell.xy) * ocean.swell.z - ocean.swell_motion.x * ocean.swell_motion.w;
    gradient_foam = vec3<f32>(gradient_foam.xy + ocean.swell.xy * ocean.swell.w * ocean.swell.z * cos(phase), gradient_foam.z);
    let wave_activity = select(1.0, ocean.spray_planes[0].y, ocean.authored_plane.w > 0.5);
    // Match the wave normals to the displaced geometry, and avoid ocean-sized
    // whitecaps on an almost-flat promoted pool.
    let foam_gain = select(1.0, ocean.spray_planes[0].w, ocean.authored_plane.w > 0.5);
    let raw_foam = gradient_foam.z;
    let activity_mode = tuned_foam_activity;
    let activity_gate = select(wave_activity, 1.0, activity_mode >= 1.5);
    let heuristic_gate = select(foam_gain, 1.0, activity_mode == 1.0 || activity_mode >= 2.5);
    gradient_foam = vec3<f32>(gradient_foam.xy, raw_foam * activity_gate * heuristic_gate);
    // Whitecaps carry no distance fade.
    let breaker_foam = smoothstep(0.0, 1.0, gradient_foam.z * 0.75 * tuned_foam_gain);
    var windrow_foam = 0.0;
    if (ocean.surface.w > 0.5 && dist < 180.0 && metres_per_pixel < 0.75) {
        let windrow_uv = fract(world_m / max(ocean.ocean_info.w, 1.0));
        let windrow_layer = u32(clamp(ocean.clipmap.w, 0.0, 1.0) + 0.5);
        let windrow_size = 256u;
        let windrow_cells = windrow_size * windrow_size;
        let p = windrow_uv * f32(windrow_size) - vec2<f32>(0.5);
        let base = vec2<i32>(floor(p));
        let f = fract(p);
        let n = i32(windrow_size);
        let x0 = u32((base.x % n + n) % n);
        let y0 = u32((base.y % n + n) % n);
        let x1 = (x0 + 1u) % windrow_size;
        let y1 = (y0 + 1u) % windrow_size;
        let offset = windrow_layer * windrow_cells;
        var mid_lod = 0.0;
        var far_lod = 0.0;
        if (ocean.appearance_c.y > 0.5) {
            let half_cells=select(128.0,max(ocean.authored_plane.y,1.0),ocean.authored_plane.w > 0.5);
            let spacing=ocean.appearance_c.z;
            let world_pos=world_m*units_per_meter;
            let radius=max(abs(world_pos.x-ocean.clipmap.x),abs(world_pos.y-ocean.clipmap.y));
            let near_seam=half_cells*spacing*2.0;
            let middle_seam=half_cells*spacing*4.0;
            mid_lod=smoothstep(near_seam,near_seam+spacing*16.0,radius);
            far_lod=smoothstep(middle_seam,middle_seam+spacing*32.0,radius);
        }
        let a = ocean_windrow_lod(y0*windrow_size + x0,offset,mid_lod,far_lod);
        let b = ocean_windrow_lod(y0*windrow_size + x1,offset,mid_lod,far_lod);
        let c = ocean_windrow_lod(y1*windrow_size + x0,offset,mid_lod,far_lod);
        let d = ocean_windrow_lod(y1*windrow_size + x1,offset,mid_lod,far_lod);
        let transported = mix(mix(a,b,f.x), mix(c,d,f.x), f.y);
        // Keep thin windrows visible farther than crest foam while still fading
        // sub-texel structure before it reaches the horizon.
        windrow_foam = smoothstep(0.08, 0.72, transported) * exp(-dist*0.0022);
    }
    windrow_foam *= activity_gate * heuristic_gate;
    // FFT foam accumulates over time. Explicit zero amplitude must hide any
    // old foam immediately, including transported windrows and foam A/B modes.
    let foam_factor = (1.0 - (1.0-breaker_foam) * (1.0-windrow_foam)) * ocean.foam_color.a;
    // Foam debug is deliberately before optics, allowing an unambiguous
    // check of simulation -> promoted gates -> final shading coverage.
    if (ocean.appearance_b.w > 0.5) {
        let debug_foam = select(raw_foam,
            select(gradient_foam.z, foam_factor, ocean.appearance_b.w >= 2.5),
            ocean.appearance_b.w >= 1.5);
        return vec4<f32>(vec3<f32>(clamp(debug_foam, 0.0, 1.0)), 1.0);
    }
    // Godot fades normals to 1.5% with distance; that flattens the far sea into a
    // mirror. With full-distance detail (mode 2) keep 40% so far waves still
    // roughen the reflection and the sun's glints.
    let normal_floor = select(0.015, 0.4 * ocean.surface.y, tuned_foam_detail > 1.5);
    let normal_fade = mix(normal_floor, ocean.surface.y, exp(-dist*0.0175));
    let interaction_plane = select(ocean.authored_plane.y, ocean.spray_planes[0].x, ocean.authored_plane.w > 0.5);
    let interaction = sample_ocean_interaction(vec3<f32>(world_m.x*units_per_meter, interaction_plane, world_m.y*units_per_meter));
    if (interaction.w > 0.5) {
        return vec4<f32>(0.08+max(-interaction.x,0.0)*0.4,
            0.08+length(interaction.yz)*2.0, 0.08+max(interaction.x,0.0)*0.4, 1.0);
    }
    // Interaction slopes describe geometry, independent of FFT normal gain.
    let interaction_lod = 1.0-smoothstep(0.75,2.0,metres_per_pixel/(4.0/units_per_meter));
    var n = normalize(vec3<f32>(-gradient_foam.x*normal_fade-interaction.y*interaction_lod,
        1.0, -gradient_foam.y*normal_fade-interaction.z*interaction_lod));
    let coastal=sample_coastal(vec3<f32>(world_m.x*units_per_meter,interaction_plane,world_m.y*units_per_meter));
    n=normalize(mix(n,normalize(vec3<f32>(-coastal.slope.x-interaction.y*interaction_lod,
        1.0,-coastal.slope.y-interaction.z*interaction_lod)),coastal.field.w));
    let view_dir = normalize(camera.camera_pos_time.xyz - input.world_position);
    n = select(n, -n, below);
    let dot_nv = max(dot(n, view_dir), 2e-5);
    let authored_water = material.ocean_color.a >= 0.0;
    let roughness = clamp(select(ocean.surface.x, material.ocean_material.w,
        authored_water && material.ocean_material.w >= 0.0 && ocean.authored_plane.z < 0.5), 0.001, 1.0);
    // Shared with wet ground and puddles (weather_surface.wgsl): one water Fresnel.
    var fresnel = water_fresnel(dot_nv, roughness);
    if (authored_water && material.ocean_material.x >= 0.0) {
        let f0 = clamp(dot(material.ocean_material.rgb, vec3<f32>(1.0/3.0)), 0.0, 1.0);
        fresnel = f0 + (1.0-f0)*pow(1.0-dot_nv, 5.0);
    }
    // Water -> air: total internal reflection outside Snell's window.
    if (below && 1.333 * 1.333 * (1.0 - dot_nv * dot_nv) > 1.0) { fresnel = 1.0; }

    // Upstream keeps direct-BRDF roughness separate from environment roughness.
    // Foam makes the reflected environment rougher while clean grazing water
    // remains sharper; using the one global roughness for both was too milky.
    let ibl_roughness = clamp((1.0 - fresnel) * foam_factor + 0.4, 0.0, 1.0);

    // Map-authored q3map sun. The renderer stores light travel direction, so
    // negate it to obtain surface -> light. Intensity is normalized such that
    // the common q3map value 250 maps to the upstream reference energy 1.0.
    let light_dir = normalize(-ocean.sun_direction_intensity.xyz);
    let sun_radiance = ocean.sun_color.rgb * ocean.sun_direction_intensity.w * ocean.appearance_b.z;
    let halfway = normalize(light_dir + view_dir);
    let dot_nl = max(dot(n, light_dir), 2e-5);
    let light_mask = ocean_smith_masking_shadowing(roughness, dot_nv);
    let view_mask = ocean_smith_masking_shadowing(roughness, dot_nl);
    let microfacet_distribution = ocean_ggx_distribution(max(dot(n, halfway), 0.0), roughness);
    let geometric_attenuation = 1.0 / (1.0 + light_mask + view_mask);
    var attenuation = vec3<f32>(1.0);
    if (ENABLE_CASCADED_SHADOWS && shadow_settings.light_direction_enabled.w > 0.5) {
        attenuation = sun_shadow_transmission(input, n);
    }
    let specular = sun_radiance * (fresnel * microfacet_distribution * geometric_attenuation /
        (4.0 * dot_nv + 0.1) * attenuation);

    let sss_modifier = vec3<f32>(0.9, 1.15, 0.85);
    let sss_height = max(0.0, input.ocean_wave_height + 2.5)
        * pow(max(dot(light_dir, -view_dir), 0.0), 4.0)
        * pow(0.5 - 0.5 * dot(light_dir, n), 3.0);
    let sss_near = 0.5*pow(dot_nv, 2.0);
    let lambertian = 0.5*dot_nl;
    let water_diffuse = (sss_height + sss_near) * sss_modifier / (1.0 + light_mask) + vec3<f32>(lambertian);
    // Existing path multiplies the foam's albedo by its diffuse tint again.
    // The bundled comparison keeps the foam tint only in albedo, making its
    // direct and ambient light respond as a whitecap instead of muddy foam.
    // Albedo stays at or below the configured foam color (single tint, no
    // brightness boost): lit foam must not exceed a white surface in the same
    // light, and there is no tonemap to roll an overshoot off.
    let foam_color = ocean.foam_color.rgb * ocean.appearance_b.y;
    let foam_diffuse = select(foam_color, vec3<f32>(1.0), visual_mode > 0.5);
    let direct_diffuse = mix(water_diffuse, foam_diffuse, foam_factor)
        * (1.0 - fresnel) * attenuation * sun_radiance;

    // Keep the global water color control as a tint relative to its
    // default, while the authored texture supplies each pool's base color.
    let default_water_color = vec3<f32>(0.010022826, 0.01960665, 0.02721178);
    var body_color = select(ocean.water_color.rgb,
        material.ocean_color.rgb * ocean.water_color.rgb / default_water_color,
        authored_water && ocean.authored_plane.w < 0.5);
    if (ocean.appearance_b.x == 1.0 || ocean.appearance_b.x >= 2.5) {
        body_color = ocean.water_color.rgb;
    }
    // Both geometric and analytic ocean share the same captured scene and
    // Beer-Lambert optics. The exterior eligibility mask is handled by
    // ocean_mask_contains; do not use it to override water transmission.
    var body_opacity = select(0.0, material.ocean_color.a, authored_water);
    // The ocean optics capture now recognizes the same extended underwater
    // medium outside the BSP. Do NOT force exterior opacity to 1: that would
    // discard the fogged transmission and recreate the hard-colored rectangle.
    // The authored alpha blends against Beer-Lambert-attenuated background.
    // Generators remain live; clipmap vertex colors contain mesh metadata,
    // so their authored averages were resolved at upload instead.
    if (authored_water && (material_flags() & 134217728u) != 0u) {
        body_color *= clamp(wave_value(material.wave_funcs.x, material.wave_rgb, shader_stage_time()), 0.0, 1.0);
    }
    if (authored_water && (material_flags() & 268435456u) != 0u) {
        body_opacity *= clamp(wave_value(material.wave_funcs.y, material.wave_alpha, shader_stage_time()), 0.0, 1.0);
    }
    let albedo = mix(body_color, foam_color, foam_factor);
    let environment = ocean_environment(input, n, view_dir, ibl_roughness);
    let indirect = ocean_indirect_lighting(input, n, albedo);
    let local_lights = ocean_dynamic_lighting(input, n, view_dir, albedo, roughness);

    let dims = vec2<f32>(textureDimensions(ocean_scene));
    let uv = input.clip_position.xy / dims;
    let water_depth = dot(input.world_position - camera.camera_pos_time.xyz, camera.camera_forward.xyz);
    let undistorted_depth = textureLoad(ocean_scene_depth, vec2<i32>(input.clip_position.xy), 0).r;
    // Ocean optics encodes no finite receiver (map sky, Blue Water sky/clouds)
    // with the 1e6 linear-depth sentinel. In a submerged view this captured
    // background must transmit through the water surface, not be replaced by
    // an authored JKA water stage whose source texture happens to be opaque.
    // Keep Fresnel/total-internal-reflection and optical absorption intact.
    let sky_transmission_below = below && undistorted_depth >= 999999.0;
    let thickness = max(undistorted_depth - water_depth, 0.0);
    let edge = smoothstep(0.0, 0.06, min(min(uv.x, uv.y), min(1.0-uv.x, 1.0-uv.y)));
    let tilt = n - vec3<f32>(0.0, select(1.0, -1.0, below), 0.0);
    let screen_normal = (camera.view_proj * vec4<f32>(tilt, 0.0)).xy * vec2<f32>(1.0,-1.0);
    let offset = clamp(screen_normal, vec2<f32>(-1.0), vec2<f32>(1.0)) * ocean_optics.params.y * edge * min(thickness / 64.0, 1.0);
    var refracted_uv = clamp(uv + offset, 0.5/dims, 1.0-0.5/dims);
    if (dist < 180.0 && metres_per_pixel < 0.75) {
        let sample_depth = textureLoad(ocean_scene_depth, vec2<i32>(refracted_uv*dims), 0).r;
        if (sample_depth <= water_depth + 1.0) { refracted_uv = uv; }
    } else {
        // At this footprint the refraction displacement is sub-pixel. Preserve
        // transparency with the captured scene but skip the second depth lookup.
        refracted_uv = uv;
    }
    // The capture already contains absorption for real scene receivers.
    // Missing receiver/sky pixels retain the captured background; forcing
    // raw fog RGB here only on sky-exterior pixels creates the visible
    // interior/extension color boundary (also in the analytic far tail).
    let transmission = textureSampleLevel(ocean_scene, ocean_scene_sampler, refracted_uv, 0.0).rgb;
    let surface_light = indirect + albedo * direct_diffuse + local_lights;
    // alphaGen/texture alpha controls how much lit water replaces the
    // transmitted scene. Fresnel and foam still use the FFT ocean response.
    // A/B against Godot's fully shaded body, rather than JKA's alpha-blended
    // authored map water. Ignore authored blending in this comparison mode.
    let opaque_body_test = ocean.appearance_b.x >= 1.5;
    if (opaque_body_test) { body_opacity = max(body_opacity, 0.95); }
    // The baseline respects JKA stage transparency verbatim. A/B mode offers
    // a lit water-body floor in promoted oceans; otherwise near-opaque foam
    // swims on top of largely unchanged gray scene capture.
    if (visual_mode > 0.5 && ocean.authored_plane.w > 0.5 && !below) {
        body_opacity = max(body_opacity, select(0.42, 0.72, visual_mode > 1.5));
    }
    // Authored alphas describe the above-water material. They must not turn
    // the sky into a solid water-colored ceiling when viewed from below.
    if (sky_transmission_below) { body_opacity = 0.0; }
    var body_transmission = mix(transmission, surface_light, clamp(body_opacity, 0.0, 1.0));
    // Authored framebuffer blends are meaningful only over the authored
    // geometry. Extending them onto sky would simply reintroduce sky pixels.
    let authored_blend = authored_water && material.ocean_waves.z > 0.0
        && !opaque_body_test && !sky_transmission_below;
    let factors = vec2<u32>(u32(max(material.ocean_waves.z-1.0,0.0)),u32(material.ocean_waves.w));
    var stage = vec4<f32>(0.0);
    if (authored_blend) {
        // Use only the CPU-analyzed mean. The original image and its tcMods
        // never appear on the promoted surface; FFT supplies all wave detail.
        let tint = body_color / max(material.ocean_color.rgb,vec3<f32>(0.0001));
        stage = vec4<f32>(material.ocean_blend_color.rgb*tint,body_opacity);
        body_transmission = ocean_authored_blend(stage,vec4<f32>(transmission,1.0),factors);
    }
    // Foam is matte: no mirror reflection or sun glint on whitecaps.
    let glossy = 1.0 - foam_factor;
    var color = body_transmission * (1.0-fresnel) * (1.0-foam_factor)
        + surface_light * foam_factor + (specular + environment * fresnel) * glossy;
    if (below) {
        // Reflected/surface light travels from the interface to the eye through water.
        let ratios = vec3<f32>(38.0/7.5, 38.0/22.0, 1.0);
        let t = exp2(-4.321928 * ratios * length(input.world_position-camera.camera_pos_time.xyz) / max(ocean_optics.fog.w, 1.0));
        let depth = max(input.world_position.y-camera.camera_pos_time.y, 0.0);
        let ambient = ocean_optics.fog.rgb * exp2(-4.321928 * ratios * depth / max(ocean_optics.params.x, 1.0));
        var below_body = mix(transmission, surface_light * t + ambient * (1.0-t), clamp(body_opacity, 0.0, 1.0));
        if (authored_blend) {
            below_body = ocean_authored_blend(vec4<f32>(stage.rgb*t,stage.a),vec4<f32>(transmission,1.0),factors);
        }
        color = below_body * (1.0-fresnel) * (1.0-foam_factor)
            + (surface_light * foam_factor + (specular + environment * fresnel) * glossy) * t
            + ambient * (1.0-t) * (fresnel + (1.0-fresnel)*foam_factor);
    }
    return vec4<f32>(color, 1.0);
}

// Original sun is a known RGB *direction* from q3map_sun/q3map_sunExt.
// The sun's authored intensity is not a calibrated linear-lightmap energy value.
// The decomposition is constrained by local baked-shadow samples rather than
// independent RGB clamps against an arbitrary (0.10,0.10,0.10) black floor.
fn baked_luminance(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn old_sun_texel_visible(pixel: vec2<i32>, receiver_depth: f32, sky_receiver_depth: f32, depth_bias: f32) -> f32 {
    let sky = textureLoad(sky_admission_texture, pixel, 4i, 0);
    let caster = textureLoad(shadow_texture, pixel, 4i, 0);
    let sky_ok = sky > 0.000001 && sky >= sky_receiver_depth - 0.0005;
    let caster_ok = receiver_depth + depth_bias >= caster;
    return select(0.0, 1.0, sky_ok && caster_ok);
}

fn original_sun_visibility(input: VertexOut, normal: vec3<f32>) -> f32 {
    let toward_old = normalize(lighting_settings.baked_sun_direction_strength.xyz);
    // Offset receiver *toward* the light in WORLD units to prevent large-map
    // reverse-Z precision and parallel-floor self-shadow stippling.
    let receiver = input.world_position + toward_old * 1.5;
    let clip = lighting_settings.original_sun_shadow_view_proj * vec4<f32>(receiver, 1.0);
    if (clip.w <= 0.0) { return 0.0; }
    let ndc = clip.xyz / clip.w;
    if (any(abs(ndc.xy) >= vec2<f32>(0.995)) || ndc.z <= 0.0 || ndc.z >= 1.0) { return 0.0; }
    let dims = vec2<i32>(textureDimensions(shadow_texture));
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let texel_center = uv * vec2<f32>(dims);
    let old_layer = 4i;
    let encoded_receiver_depth = 0.5 + atan(ndc.z) / 3.14159265358979323846;
    let slope = 1.0 - max(dot(normalize(normal), toward_old), 0.0);
    let depth_bias = 0.00035 + 0.001 * slope;
    // Fast path: five visibility taps detect uniform sun/shadow regions.
    // The 4x4 filter runs only in the penumbra, not over whole floors.
    let radius = lighting_settings.baked_indirect_adjust.z;
    let center_pixel = clamp(vec2<i32>(floor(texel_center)), vec2<i32>(0), dims - vec2<i32>(1));
    var sum = old_sun_texel_visible(center_pixel, ndc.z, encoded_receiver_depth, depth_bias);
    for (var k = 0i; k < 4i; k += 1i) {
        let corner = vec2<f32>(select(-1.0, 1.0, (k & 1) != 0),
            select(-1.0, 1.0, (k & 2) != 0));
        let loc = clamp(vec2<i32>(floor(texel_center + corner * radius)),
            vec2<i32>(0), dims - vec2<i32>(1));
        sum += old_sun_texel_visible(loc, ndc.z, encoded_receiver_depth, depth_bias);
    }
    if (sum <= 0.0 || sum >= 5.0 || radius <= 0.001) {
        return sum * 0.2;
    }
    // Deterministic screen-resolution-independent 4x4 PCF, including SKY.
    var visible = 0.0;
    for (var y = 0i; y < 4i; y += 1i) {
        for (var x = 0i; x < 4i; x += 1i) {
            let step_xy = (vec2<f32>(f32(x), f32(y)) - vec2<f32>(1.5)) / 1.5;
            let loc = clamp(vec2<i32>(floor(texel_center + step_xy * radius)),
                vec2<i32>(0), dims - vec2<i32>(1));
            visible += old_sun_texel_visible(loc, ndc.z, encoded_receiver_depth, depth_bias);
        }
    }
    return visible * 0.0625;
}

// A low-end estimate of indirect irradiance near the receiver. Lightmap texels
// rather than adjacent SCREEN pixels make this stable with camera movement.
// Samples remain within the same bound texture, but may cross chart borders:
// therefore this is a soft limiter, not a claimed lightmap reconstruction.
fn baked_indirect_tone(rgb: vec3<f32>) -> vec3<f32> {
    let intensity = lighting_settings.baked_indirect_adjust.x;
    let saturation = lighting_settings.baked_indirect_adjust.y;
    let luminance = max(baked_luminance(rgb), 0.0);
    var curve_gain = 1.0;
    let power = lighting_settings.baked_indirect_curve.x;
    if (power > 1.0001) {
        // Lightmap textures are sampled in linear light. Power curves must be
        // applied to luminance as a common RGB gain, not to channels separately.
        // This is highlight-preserving tone shaping, not true light separation.
        let low_light_gain = pow(clamp(luminance, 0.0, 1.0), power - 1.0);
        let highlight_weight = smoothstep(0.35, 0.85, luminance) *
            lighting_settings.baked_indirect_curve.y;
        curve_gain = mix(low_light_gain, 1.0, highlight_weight);
    }
    let gray = vec3<f32>(luminance);
    return max(mix(gray, rgb, saturation) * (intensity * curve_gain), vec3<f32>(0.0));
}

// Q3Map2 applies gamma, then a hue-preserving exponential exposure, then
// overbright compensation. Undo that transform BEFORE subtracting photon units.
fn baked_file_to_linear(rgb: vec3<f32>) -> vec3<f32> {
    return select(rgb / 12.92, pow((rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), rgb > vec3<f32>(0.04045));
}
fn baked_linear_to_file(rgb: vec3<f32>) -> vec3<f32> {
    return select(rgb * 12.92, 1.055 * pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055, rgb > vec3<f32>(0.0031308));
}
fn subtract_authored_baked_sun(lightmap: vec3<f32>, flux: f32, input: VertexOut) -> vec3<f32> {
    let fit = input.baked_sun_fit;
    let strength = lighting_settings.baked_sun_direction_strength.w;
    var remaining = lightmap;
    if (strength > 0.0 && flux > 0.0) {
        var encoded = lightmap;
        if (fit.w > 0.0) { encoded = baked_linear_to_file(encoded); }
        let compensated = encoded * abs(fit.w);
        let maximum = max(max(compensated.r, compensated.g), compensated.b);
        // Default compiler output has zero exposure and stores photon values
        // directly until saturation. Positive exposure uses the exponential curve.
        var raw_gamma = compensated * 255.0;
        if (fit.x > 0.0) {
            let raw_maximum = -fit.x * log(max(1.0 - min(maximum, 254.5 / 255.0), 0.5 / 255.0));
            raw_gamma = compensated / max(maximum, 0.00001) * raw_maximum;
        }
        let raw = pow(max(raw_gamma / 255.0, vec3<f32>(0.0)), vec3<f32>(fit.y)) * 255.0;
        let direct = lighting_settings.baked_sun_radiance.rgb * (255.0 * flux * strength);
        let residual = max(raw - direct, vec3<f32>(0.0));
        let gamma_residual = pow(residual / 255.0, vec3<f32>(1.0 / fit.y)) * 255.0;
        let peak = max(max(gamma_residual.r, gamma_residual.g), gamma_residual.b);
        encoded = gamma_residual / max(peak, 255.0) / abs(fit.w);
        if (fit.x > 0.0) {
            encoded = gamma_residual / max(peak, 0.00001) * (1.0 - exp(-peak / fit.x)) / abs(fit.w);
        }
        if (fit.w > 0.0) { encoded = baked_file_to_linear(encoded); }
        remaining = min(lightmap, max(encoded, vec3<f32>(0.0)));
    }
    let debug_mode = lighting_settings.baked_sun_debug_mode.x;
    if (debug_mode == 1u) {
        let fraction = clamp((baked_luminance(lightmap) - baked_luminance(remaining)) / max(baked_luminance(lightmap), 0.001), 0.0, 1.0);
        return vec3<f32>(fraction, fraction * 0.16, 0.0);
    }
    if (debug_mode == 2u) {
        let facing = max(dot(normalize(input.world_normal), normalize(lighting_settings.baked_sun_direction_strength.xyz)), 0.001);
        let visibility = clamp(flux / facing, 0.0, 1.0);
        return mix(vec3<f32>(1.0, 0.03, 0.02), vec3<f32>(0.02, 1.0, 0.03), visibility);
    }
    return remaining;
}

// Filtering exposed samples before inverse lighting loses energy at edges:
// inverse(mean(exposure(L))) != mean(L). Subtract at each original luxel first.
fn sample_authored_baked_sun(lightmap: vec3<f32>, input: VertexOut) -> vec3<f32> {
    if (lighting_settings.baked_sun_direction_strength.w <= 0.0 && lighting_settings.baked_sun_debug_mode.x == 0u) {
        return baked_indirect_tone(lightmap);
    }
    let dimensions = vec2<i32>(textureDimensions(lightmap_texture));
    let position = input.lightmap_uv * vec2<f32>(dimensions) - vec2<f32>(0.5);
    let origin = vec2<i32>(floor(position));
    let fraction = fract(position);
    let offsets = array<vec2<i32>, 4>(vec2<i32>(0, 0), vec2<i32>(1, 0), vec2<i32>(0, 1), vec2<i32>(1, 1));
    let weights = vec4<f32>((1.0 - fraction.x) * (1.0 - fraction.y), fraction.x * (1.0 - fraction.y),
        (1.0 - fraction.x) * fraction.y, fraction.x * fraction.y);
    var result = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i += 1u) {
        let coordinate = clamp(origin + offsets[i], vec2<i32>(0), dimensions - vec2<i32>(1));
        let texel = textureLoad(lightmap_texture, coordinate, 0);
        result += subtract_authored_baked_sun(texel.rgb, texel.a, input) * weights[i];
    }
    if (lighting_settings.baked_sun_debug_mode.x == 0u) { return baked_indirect_tone(result); }
    return result;
}

fn remove_baked_sun(lightmap: vec3<f32>, flux: f32, input: VertexOut) -> vec3<f32> {
    if (!ENABLE_BAKED_SUN_RELIGHT) { return lightmap; }
    // A change-triggered compute pass already subtracted each original luxel.
    if (lighting_settings.baked_sun_debug_mode.y != 0u && input.baked_sun_fit.z == 4.0 && input.shell_kind == 0u) { return baked_indirect_tone(lightmap); }
    if (input.baked_sun_fit.z == 4.0) { return sample_authored_baked_sun(lightmap, input); }
    let strength = lighting_settings.baked_sun_direction_strength.w;
    var remaining = lightmap;
    let toward_old = normalize(lighting_settings.baked_sun_direction_strength.xyz);
    let old_facing = max(dot(normalize(input.world_normal), toward_old), 0.0);
    let fit_kind = u32(input.baked_sun_fit.z + 0.5);
    var visibility = select(1.0, 0.0, fit_kind == 3u);
    let shadow_map_requested = lighting_settings.original_sun_shadow_params.x > 0.5;
    let shadow_map_ready = lighting_settings.original_sun_shadow_params.z > 0.5;
    if (shadow_map_requested && fit_kind == 0u) {
        // Fail closed while the map-wide visibility cache is being built or
        // when its projection is invalid. Never treat a missing cache as lit.
        visibility = 0.0;
        if (shadow_map_ready) {
            visibility = original_sun_visibility(input, input.world_normal);
        }
    }
    let sun_rgb = max(lighting_settings.baked_sun_radiance.rgb, vec3<f32>(0.0)) * old_facing;
    let sun_y = baked_luminance(sun_rgb);
    let y = baked_luminance(lightmap);
    var removed_y = 0.0;
    if (old_facing > 0.0 && sun_y > 0.00001 && visibility > 0.0 && strength > 0.0) {
        // q3map_sun intensity is not calibrated to the compiled lightmap's
        // irradiance. Keep the estimate local and monotonic: cap the removal to
        // a fraction of this texel and to the texel's available RGB channels.
        // Do not sample neighboring packed lightmap charts to guess a baseline.
        // Unpaired surfaces retain the conservative cap. Paired surfaces
        // below use their measured residual and direct upper bound instead.
        let budget = y * lighting_settings.baked_indirect_adjust.w;
        let sun_unit = sun_rgb / sun_y;
        let channel_budget = min(
            min(lightmap.r / max(sun_unit.r, 0.001), lightmap.g / max(sun_unit.g, 0.001)),
            lightmap.b / max(sun_unit.b, 0.001));
        var energy = min(min(sun_y, budget), channel_budget);
        if (fit_kind == 1u) {
            // Use the lightmap's own shadow edge at its authored resolution.
            // Subtraction stops at the measured indirect baseline; dark texels
            // stay dark without projecting a coarse shadow-map edge onto them.
            energy = min(channel_budget, min(input.baked_sun_fit.y,
                max(y - input.baked_sun_fit.x, 0.0)));
        }
        removed_y = energy * strength * visibility;
        remaining = max(lightmap - sun_unit * removed_y, vec3<f32>(0.0));
    }

    let debug_mode = lighting_settings.baked_sun_debug_mode.x;
    if (debug_mode == 1u) {
        // Black means no estimated subtraction; red gets brighter as a larger
        // fraction of this lightmap sample is removed.
        let removed_fraction = clamp(removed_y / max(y, 0.001), 0.0, 1.0);
        return vec3<f32>(removed_fraction, removed_fraction * 0.16, 0.0);
    }
    if (debug_mode == 2u) {
        if (old_facing <= 0.0) { return vec3<f32>(0.02, 0.03, 0.12); }
        if (!shadow_map_requested) { return vec3<f32>(1.0, 0.45, 0.02); }
        if (shadow_map_requested && !shadow_map_ready) { return vec3<f32>(0.1, 0.2, 1.0); }
        var shown_visibility = visibility;
        if (fit_kind == 1u) {
            shown_visibility = clamp((y - input.baked_sun_fit.x) / max(input.baked_sun_fit.y, 0.001), 0.0, 1.0);
        }
        if (shown_visibility <= 0.001) { return vec3<f32>(1.0, 0.03, 0.02); }
        return vec3<f32>(0.02, shown_visibility, 0.03);
    }
    // Night-mode controls affect ONLY surviving baked irradiance, not runtime
    // direct sun, local dynamic lights, textures or display gamma.
    remaining = baked_indirect_tone(remaining);
    return remaining;
}

// Runtime sunlight is evaluated AFTER cached subtraction/filtering and indirect
// tone controls. Its direction/intensity never modify the original-sun cache.
fn light_bsp_surface(lightmap: vec3<f32>, flux: f32, input: VertexOut) -> vec3<f32> {
    // ONE shade_baked_local call site: under hardware RT its lamp loop inlines
    // ray queries, and the driver compiles every call site into each world PSO.
    let static_receiver = static_sun_lightmap_receiver(input);
    var unshadowed: vec3<f32>;
    if static_receiver {
        unshadowed = shade_composed_sun(lightmap, flux, input);
    } else {
        unshadowed = select(remove_baked_sun(lightmap, flux, input), lightmap, composed_lightmap_receiver(input));
    }
    let remaining = shade_baked_local(unshadowed, input);
    if (static_receiver || lighting_settings.baked_sun_debug_mode.x != 0u
        || lighting_settings.runtime_sun_radiance.w < 0.5) {
        return remaining;
    }
    let facing = max(dot(normalize(input.world_normal), normalize(lighting_settings.runtime_sun_direction.xyz)), 0.0);
    if (facing <= 0.0 || all(lighting_settings.runtime_sun_radiance.rgb <= vec3<f32>(0.0))) { return remaining; }
    let visibility = sun_shadow_transmission(input, input.world_normal);
    return remaining + lighting_settings.runtime_sun_radiance.rgb * facing * visibility;
}

fn shade_surface(input: VertexOut, front: bool) -> vec4<f32> {
    let classic_flags = select(0u, camera.render_flags.z, ENABLE_CLASSIC_RENDER_FLAGS);
    let classic_fullbright = (classic_flags & CLASSIC_FULLBRIGHT) != 0u;
    let classic_vertex_light = (classic_flags & CLASSIC_VERTEX_LIGHT) != 0u;
    let classic_lightmap_only = (classic_flags & CLASSIC_LIGHTMAP_ONLY) != 0u;
    let explicit_lightmap_stage = (material_flags() & MATERIAL_EXPLICIT_LIGHTMAP) != 0u;
    let has_lightmap = (material_flags() & MATERIAL_HAS_LIGHTMAP) != 0u;
    let opaque_stage = (material_flags() & MATERIAL_OPAQUE_STAGE) != 0u;
    if (ENABLE_PLANAR_REFLECTIONS && dot(camera.clip_plane.xyz, camera.clip_plane.xyz) > 0.5
        && dot(camera.clip_plane.xyz, input.world_position) + camera.clip_plane.w < 0.0) {
        discard;
    }
    if (input.ocean_vertex > 0.5) {
        if ((ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) {
            return vec4<f32>(vec3<f32>(0.035) + runtime_dynamic_light_debug(input), 1.0);
        }
        let units_per_meter = max(ocean.ocean_info.z, 0.001);
        let coastal=sample_coastal(vec3<f32>(input.ocean_uv_m.x*units_per_meter,
            ocean.spray_planes[0].x,input.ocean_uv_m.y*units_per_meter));
        if (coastal.field.w>0.999 && coastal.field.y<0.02) {discard;}
        if (!ocean_mask_contains(i32(round(input.ocean_vertex)) - 2, input.ocean_uv_m * units_per_meter)) {
            discard;
        }
        // The ocean pipeline never uses the fs_main_legacy_fog entry, so this
        // is the only place Legacy fog reaches the GodotOcean surface. It is
        // compiled only into Legacy fog world variants.
        let ocean_color = ocean_surface_sample(input, !front);
        // Numeric foam diagnostics must not be washed out by map fog.
        if (ENABLE_LEGACY_FOG && ocean.appearance_b.w < 0.5) {
            return apply_ocean_legacy_fog(ocean_color, input.world_position);
        }
        return ocean_color;
    }
    var stage_color = material.color;
    if ((material_flags() & 1u) != 0u) {
        stage_color = vec4<f32>(stage_color.rgb * input.color.rgb, stage_color.a);
    } else if ((material_flags() & 524288u) != 0u) {
        stage_color = vec4<f32>(
            stage_color.rgb * (vec3<f32>(1.0) - input.color.rgb),
            stage_color.a,
        );
    }
    if ((material_flags() & 1048576u) != 0u) {
        stage_color.a = stage_color.a * input.color.a;
    } else if ((material_flags() & 2097152u) != 0u) {
        stage_color.a = stage_color.a * (1.0 - input.color.a);
    }
    stage_color = apply_wave_gens(stage_color);

    let authored_planar = ENABLE_PLANAR_REFLECTIONS && (material_flags() & 4096u) != 0u;
    let tcgen_environment = material_tc_gen() == 3u;
    let environment_mode = ENABLE_PLANAR_REFLECTIONS && planar_reflection.viewport.w > 0.5;
    // For promoted environment stages, the tcGen mode itself is authoritative.
    // The CPU-only candidate bit is still useful for discovery/budgeting, but
    // the active render representation (coarse vs FULL PVS sub-batches) must
    // not be able to make the replacement disappear.
    // Preserve multi-pass authored environment effects (e.g. glass whose
    // first stage filters the framebuffer and whose env stage adds the tint).
    // A selected Ultra planar slot must not silently replace their texture.
    let authored_environment_effect = (material_flags_w() & 4u) != 0u;
    let promoted_environment = tcgen_environment && environment_mode && !authored_environment_effect;
    let planar_candidate = authored_planar || promoted_environment;
    let planar_debug_surface = (material_flags() & 16384u) != 0u || promoted_environment;
    var planar_debug_mode = 0u;
    if (ENABLE_PLANAR_DEBUG) {
        planar_debug_mode = u32(planar_reflection.debug.x + 0.5);
    }
    var planar_slot = -1;
    if (ENABLE_PLANAR_REFLECTIONS) {
        planar_slot = planar_slot_for_surface(input, promoted_environment);
    }
    let selected_plane = planar_slot >= 0;

    // Stage 1 diagnostic: prove that the material stage reaching this shader is
    // actually a planar-capable stage. Authored mirrors are cyan; tcGen
    // environment stages are orange.
    if (planar_debug_mode == 1u && planar_debug_surface) {
        let candidate_color = select(
            vec3<f32>(1.0, 0.45, 0.05),
            vec3<f32>(0.05, 0.85, 1.0),
            authored_planar
        );
        return vec4<f32>(candidate_color, 1.0);
    }

    // Stage 2 diagnostic: for tcGen environment use the fragment's actual
    // world-space plane membership. This deliberately bypasses material-plane
    // metadata so coarse/FULL PVS representation mismatches are visible.
    if (planar_debug_mode == 2u && planar_debug_surface) {
        let selection_color = select(
            vec3<f32>(0.95, 0.05, 0.08),
            vec3<f32>(0.05, 1.0, 0.18),
            selected_plane
        );
        return vec4<f32>(selection_color, 1.0);
    }

    let has_pbr_brdf_map = (material_flags() & (8u | 16u | 64u | 128u)) != 0u;
    let static_pbr_path = ENABLE_DELUXE
        && has_pbr_brdf_map
        && classic_flags == 0u
        && (material_flags() & 2u) != 0u
        && input.lightmap_uv.x >= 0.0;
    let reflection_pbr_path = (material_flags() & 2048u) != 0u;
    let gi_pbr_path = ENABLE_VOXEL_GI && (material_flags() & 4u) != 0u;
    // Bevy's diffuse-indirect priority is lightmap first, then irradiance volume.
    // JKA LIGHTMAP_BY_VERTEX is also baked diffuse lighting, so treat it as the
    // same higher-priority class instead of double-lighting compiled static MD3s.
    let irradiance_pbr_path = ENABLE_IRRADIANCE_VOLUME
        && (material_flags() & 4u) != 0u
        && input.lightmap_uv.x < 0.0
        && (material_flags() & 131072u) == 0u;
    let dynamic_pbr_path = has_pbr_brdf_map
        && (material_flags() & 4u) != 0u
        && (ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS || ENABLE_AREA_LIGHTS);
    let needs_pbr_surface = ENABLE_PBR
        && (static_pbr_path || reflection_pbr_path || gi_pbr_path || irradiance_pbr_path || dynamic_pbr_path);

    var shared_frame = mat3x3<f32>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0)
    );
    let needs_shared_frame = (ENABLE_POM && (material_flags() & 32u) != 0u)
        || (needs_pbr_surface && (material_flags() & 8u) != 0u);
    if (PBR_SHARED_TANGENT_FRAME && needs_shared_frame) {
        shared_frame = cotangent_frame(input);
    }
    let surface_uv = parallax_uv(input, shared_frame);
    var source_sample = textureSample(base_texture, base_sampler, surface_uv);
    if (ENABLE_DETAIL_TEXTURES && (material_flags_w() & 2u) != 0u) {
        // Synthetic neutral 2x fallback only, at authored-style tcMod scale 8 8.
        // Authored stages are never routed through this block.
        //
        // When distance fade is enabled, resolve its weight *before* sampling
        // the detail texture. At/after the fade distance the branch is coherent
        // for distant surfaces and skips the detail fetch/blend entirely.
        var detail_weight = 1.0;
        if (pbr_settings.deluxe.z > 0.5) {
            let fade_distance = max(pbr_settings.deluxe.w, 1.0);
            let to_surface = input.world_position - camera.camera_pos_time.xyz;
            // Algebraically identical to pow(distance / fade_distance, 4),
            // but avoids both the sqrt in distance() and a general pow().
            let distance_ratio_sq = dot(to_surface, to_surface) / (fade_distance * fade_distance);
            let far_weight = clamp(distance_ratio_sq * distance_ratio_sq, 0.0, 1.0);
            detail_weight = 1.0 - far_weight;
        }

        if (detail_weight > 0.0) {
            let detail_linear = textureSample(
                detail_texture,
                detail_sampler,
                surface_uv * 8.0
            ).rgb;
            // Neutral 2x: recover the encoded sRGB detail before modulation.
            // 128 grey stays approximately neutral instead of darkening the base.
            let lo = detail_linear * 12.92;
            let hi = 1.055 * pow(max(detail_linear, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
            let detail_encoded = select(lo, hi, detail_linear > vec3<f32>(0.0031308));
            let detail_factor = detail_encoded * 2.0;

            let faded_detail_factor = mix(vec3<f32>(1.0), detail_factor, detail_weight);
            source_sample = vec4<f32>(source_sample.rgb * faded_detail_factor, source_sample.a);
        }
    }

    // Planar promotion is a *source substitution for this exact shader stage*.
    // For tcGen environment this removes the legacy sphere-map image/UVs and
    // substitutes the rendered planar scene. Later shader stages (base decal,
    // lightmap, etc.) remain ordered and blended exactly as authored.
    if (planar_candidate && selected_plane) {
        let debug_raw = planar_debug_mode == 4u || planar_debug_mode == 5u;
        let reflected = sample_planar_reflection(
            input.clip_position.xy,
            planar_slot,
            promoted_environment && !debug_raw
        );
        if (debug_raw) {
            return vec4<f32>(reflected.rgb, 1.0);
        }
        if (promoted_environment) {
            // Keep the legacy environment texture as the material's authored
            // reflection response. Opaque environment stages (for example the
            // imperial/square base stage) are genuine source replacement. But
            // blended environment stages such as byss/floor_byss used a dark
            // environment image and/or alpha to control how much reflection was
            // added. Replacing that with an unmasked HDR-ish scene sample makes
            // the floor look transparent. Preserve both its RGB response and
            // alpha while replacing only the reflected *content*.
            let blended_environment = (material_flags() & 32768u) != 0u;
            let response = select(
                vec3<f32>(1.0),
                source_sample.rgb,
                blended_environment
            );
            source_sample = vec4<f32>(reflected.rgb * response, source_sample.a);
        } else {
            source_sample = reflected;
        }
    } else if ((planar_debug_mode == 4u || planar_debug_mode == 5u) && planar_debug_surface) {
        return vec4<f32>(0.95, 0.05, 0.75, 1.0);
    }

    // Lightmap alpha carries internal sun visibility. Legacy material blending
    // must continue to see an opaque lightmap even when removal is disabled.
    // Preserve the lighting payload before replacing alpha for stage blending.
    let explicit_lightmap_flux = source_sample.a;
    if (explicit_lightmap_stage) { source_sample.a = 1.0; }

    // Classic id Tech 3 world-lighting controls. Keep them runtime flags so
    // development toggles do not create shader/pipeline permutations.
    if (explicit_lightmap_stage) {
        if (classic_fullbright || (classic_vertex_light && classic_lightmap_only)) {
            source_sample = vec4<f32>(1.0);
        } else if (classic_vertex_light) {
            source_sample = vec4<f32>(input.color.rgb, source_sample.a);
        }
    }

    // r_lightmap on an explicit multi-pass material effectively leaves the
    // opaque base as white, lets the lightmap/filter pass supply the visible
    // image, and suppresses later decorative passes. The compact implicit
    // base*lightmap path is handled below without needing a second pass.
    if (classic_lightmap_only && has_lightmap
        && !explicit_lightmap_stage
        && (material_flags() & 2u) == 0u) {
        if (opaque_stage) {
            source_sample = vec4<f32>(1.0);
            stage_color = vec4<f32>(1.0);
        } else {
            discard;
        }
    }

    // Explicit $lightmap stages use the stage's source sample, not the
    // synthesized implicit lightmap multiply below. Preserve blending order.
    // q3map vertex-lit faces have no lightmap. Only touch the authored vertex
    // color path, never unrelated animated/non-lightmapped shader stages.
    // The two cases exclude each other (explicit_lightmap_stage), so they share
    // ONE light_bsp_surface call site: every call site inlines the whole
    // runtime relight, including hardware-RT traversals, into each world PSO.
    let relight_explicit = ENABLE_BAKED_SUN_RELIGHT && classic_flags == 0u
        && explicit_lightmap_stage && input.lightmap_uv.x >= 0.0;
    let relight_vertex_lit = ENABLE_BAKED_SUN_RELIGHT && classic_flags == 0u
        && (material_flags() & 131072u) != 0u && (material_flags() & 1u) != 0u
        && !explicit_lightmap_stage;
    if (relight_explicit || relight_vertex_lit) {
        let baked = light_bsp_surface(select(input.color.rgb, source_sample.rgb, relight_explicit),
            select(0.0, explicit_lightmap_flux, relight_explicit), input);
        if (lighting_settings.baked_sun_debug_mode.x != 0u) {
            return vec4<f32>(baked, select(stage_color.a, source_sample.a, relight_explicit));
        }
        if (relight_explicit) {
            source_sample = vec4<f32>(baked, select(source_sample.a, 1.0, input.baked_sun_fit.z == 4.0 || composed_lightmap_receiver(input)));
        } else {
            stage_color = vec4<f32>(material.color.rgb * baked, stage_color.a);
        }
    }
    var base = source_sample * stage_color;
    // An authored masked base with BSP vertex alpha 127/255 and GE128 cannot
    // seed depth. For the narrowly identified multi-stage depth-equal case,
    // test the source texture's alpha while retaining the vertex alpha for
    // the actual stage output and for all sibling shaders.
    let mask_alpha = select(base.a, source_sample.a * material.color.a,
        (material_flags() & 1073741824u) != 0u);
    if (ENABLE_ALPHA_TEST && material_alpha_rejects(mask_alpha)) {
        discard;
    }
    if ((ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) {
        // Collapse multi-stage materials to one neutral opaque receiver. Alpha
        // test above remains respected, but textures/static lighting do not.
        if (!opaque_stage) {
            discard;
        }
        var debug_light = vec3<f32>(0.0);
        if ((material_flags() & 4u) != 0u) {
            debug_light = runtime_dynamic_light_debug(input);
        }
        return vec4<f32>(vec3<f32>(0.035) + debug_light, base.a);
    }
    let weather = weather_surface_response(input);
    let wetness = weather.wetness;
    let puddle = weather.puddle;
    // `puddle_debug <mode>`; see weather_surface_debug for what each mode paints.
    // Branch form: the uniform load itself must vanish from dry variants.
    var weather_debug_mode = 0u;
    if (ENABLE_WEATHER_SURFACE) {
        weather_debug_mode = u32(max(weather_surface.puddle.w, 0.0) + 0.5);
    }
    if (ENABLE_WEATHER_SURFACE && weather_debug_mode != 0u) {
        return vec4<f32>(weather_surface_debug(input, weather, weather_debug_mode), 1.0);
    }
    if (ENABLE_WEATHER_SURFACE && wetness > 0.001) {
        base = vec4<f32>(weather_wet_albedo(base.rgb, weather) * weather_ripple_relief(weather) * weather_water_depth_tint(weather), base.a);
    }
    let albedo = base.rgb;
    let source_map_unbaked_receiver = ENABLE_SOURCE_MAP_WORLD
        && (material_flags() & 4u) != 0u
        && input.lightmap_uv.x < 0.0
        && (material_flags() & 131072u) == 0u;
    if (ENABLE_MAP_LIGHT_SIMULATION
        && (material_flags() & 4u) != 0u
        && input.lightmap_uv.x < 0.0
        && (material_flags() & 131072u) == 0u) {
        // q3map2 LightingAtSample starts style 0 at worldspawn `_ambient`
        // (tinted by worldspawn `_color`) before direct lights are accumulated.
        base = vec4<f32>(albedo * lighting_settings.map_ambient.rgb, base.a);
    }
    var shared_pbr = default_pbr_surface(input, albedo, weather);
    if (PBR_SHARED_MATERIAL_EVAL && needs_pbr_surface) {
        shared_pbr = evaluate_pbr_surface(input, albedo, surface_uv, weather, shared_frame);
    }

    // Bevy integrates irradiance volumes as diffuse indirect illumination:
    // irradiance * diffuse_color * diffuse_occlusion. Do that here rather than
    // multiplying base color earlier in the material stage. Lightmapped and
    // LIGHTMAP_BY_VERTEX surfaces were excluded above because baked diffuse has
    // higher priority in Bevy's light-probe source selection.
    if (irradiance_pbr_path) {
        var indirect_normal = normalize(input.world_normal);
        var diffuse_color = albedo;
        var diffuse_occlusion = 1.0;
        if (ENABLE_PBR) {
            let surface = pbr_surface_for_path(
                input,
                albedo,
                surface_uv,
                weather,
                shared_frame,
                shared_pbr
            );
            indirect_normal = surface.n;
            diffuse_color = diffuse_color * (1.0 - surface.metallic);
            diffuse_occlusion = surface.ao;
        }
        let probe = bevy_irradiance_volume_light(input.world_position, indirect_normal);
        // An enabled Bevy irradiance-volume path owns diffuse indirect lighting
        // for eligible unlightmapped surfaces. Outside the bounded probe there is
        // no probe irradiance, matching Bevy's light-probe query semantics.
        base = vec4<f32>(probe.irradiance * diffuse_color * diffuse_occlusion, base.a);
    }

    // Only the synthesized implicit material uses a single-pass
    // base-texture × lightmap path. Explicit JKA shader scripts render their
    // `$lightmap` as its own ordered blend stage instead.
    if ((material_flags() & 2u) != 0u && input.lightmap_uv.x >= 0.0) {
        let lightmap_sample = textureSample(lightmap_texture, lightmap_sampler, input.lightmap_uv);
        var lightmap_color = lightmap_sample.rgb;
        if (classic_fullbright) {
            // Vanilla r_fullbright keeps the diffuse texture but replaces the
            // baked lightmap contribution with white.
            lightmap_color = vec3<f32>(1.0);
        } else if (classic_vertex_light) {
            // r_vertexLight substitutes the BSP vertex color for baked lightmap
            // sampling. r_lightmap + r_vertexLight is the vanilla white debug case.
            lightmap_color = select(input.color.rgb, vec3<f32>(1.0), classic_lightmap_only);
        }
        if (ENABLE_BAKED_SUN_RELIGHT && classic_flags == 0u) {
            lightmap_color = light_bsp_surface(lightmap_color, lightmap_sample.a, input);
            if (lighting_settings.baked_sun_debug_mode.x != 0u) {
                return vec4<f32>(lightmap_color, base.a);
            }
        }
        if (classic_lightmap_only) {
            // r_lightmap removes the diffuse texture so the lighting data itself
            // is visible (the familiar white-walls/lightmap debug presentation).
            base = vec4<f32>(lightmap_color, base.a);
        } else {
            base = vec4<f32>(base.rgb * lightmap_color, base.a);
        }
        if (ENABLE_PBR && ENABLE_DELUXE && has_pbr_brdf_map && classic_flags == 0u) {
            let baked = static_baked_pbr(input, albedo, surface_uv, lightmap_color, weather, shared_frame, shared_pbr);
            base = vec4<f32>(base.rgb * baked.diffuse_factor + baked.specular, base.a);
        }
    }

    // LIGHTMAP_BY_VERTEX surfaces have no lightmap texture to expose. Preserve
    // their compiled vertex lighting for r_lightmap; vanilla r_vertexLight +
    // r_lightmap deliberately resolves to white instead.
    if (classic_lightmap_only && (material_flags() & 131072u) != 0u && !has_lightmap) {
        base = vec4<f32>(select(input.color.rgb, vec3<f32>(1.0), classic_vertex_light), base.a);
    }

    // Cached static BSP AO only modulates the normal baked-light contribution.
    // Classic debug/compatibility modes should show their source lighting rather
    // than a second modern AO modulation.
    if (ENABLE_STATIC_BSP_AO && (camera.render_flags.y & 1u) != 0u && classic_flags == 0u
        && (material_tc_gen() == 1u || (material_flags() & 2u) != 0u || (material_flags() & 131072u) != 0u)) {
        let static_ao = clamp(input.sky_dir_ao.w, 0.0, 1.0);
        base = vec4<f32>(base.rgb * static_ao, base.a);
    }
    if (ENABLE_PBR && (material_flags() & 2048u) != 0u) {
        base = vec4<f32>(base.rgb + reflection_probe_specular(input, albedo, surface_uv, weather, shared_frame, shared_pbr), base.a);
    }
    if ((material_flags() & 4u) != 0u) {
        base = vec4<f32>(base.rgb + voxel_probe_indirect(input, albedo, surface_uv, weather, shared_frame, shared_pbr), base.a);
        // When direct sun is added explicitly, its visibility already casts
        // shadows. Legacy mode instead darkens the existing baked illumination.
        if (lighting_settings.runtime_sun_radiance.w < 0.5 && !static_sun_lightmap_receiver(input)) {
        let shadow_rgb = sun_shadow_transmission(input, input.world_normal);
        let light_direction = normalize(shadow_settings.light_direction_enabled.xyz);
        let sun_facing = max(dot(normalize(input.world_normal), -light_direction), 0.0);
        let shadow_strength = shadow_settings.params.z * sun_facing;
        base = vec4<f32>(base.rgb * (vec3<f32>(1.0) - shadow_strength * (vec3<f32>(1.0) - shadow_rgb)), base.a);
        }
        // Projected cloud shadows are applied once to the fully composited scene
        // in post.wgsl. Keeping them out of individual legacy material stages
        // avoids shader-dependent holes and prevents multipass surfaces from
        // receiving a different result than otherwise-identical neighbours.
        if (ENABLE_VERTEX_DLIGHTS) {
            base = vec4<f32>(base.rgb + albedo * input.vertex_dlight, base.a);
        }
        if (ENABLE_LEGACY_DLIGHTS) {
            let projected_lighting = legacy_dynamic_light(input);
            base = vec4<f32>(base.rgb + albedo * projected_lighting.diffuse_factor, base.a);
        }
        if ((material_flags_w() & MATERIAL_DLIGHT_IN_LIGHTMAP_STAGE) != 0u) {
            // Added once in this material's `$lightmap` stage instead.
        } else if (ENABLE_CLUSTERED_LITE_DLIGHTS) {
            let clustered_lite = clustered_dynamic_light_legacy(input, weather);
            base = vec4<f32>(base.rgb + albedo * clustered_lite.diffuse_factor + clustered_lite.wet_specular, base.a);
        } else if (ENABLE_PBR && has_pbr_brdf_map) {
            base = vec4<f32>(base.rgb + clustered_dynamic_light_pbr(input, albedo, surface_uv, weather, shared_frame, shared_pbr), base.a);
        } else {
            let legacy_lighting = clustered_dynamic_light_legacy(input, weather);
            base = vec4<f32>(base.rgb + albedo * legacy_lighting.diffuse_factor + legacy_lighting.wet_specular, base.a);
        }
    } else if (explicit_lightmap_stage && classic_flags == 0u
        && (material_flags_w() & MATERIAL_DLIGHT_IN_LIGHTMAP_STAGE) != 0u) {
        // Multi-stage JKA shaders (opaque base, alpha-blended texture, then a
        // `$lightmap` filter stage) overwrite or scale whatever an earlier stage
        // added, so point/RT lights added in the base stage never survive. The
        // lightmap stage is the one every later or earlier texture stage is
        // multiplied by: albedo * (lightmap + light) == albedo * lightmap +
        // albedo * light, which is exactly an additive local light.
        let stage_light = clustered_dynamic_light_legacy(input, weather);
        base = vec4<f32>(base.rgb + stage_light.diffuse_factor, base.a);
    }
    if (ENABLE_MAP_LIGHT_SIMULATION
        && (material_flags() & 4u) != 0u
        && input.lightmap_uv.x < 0.0
        && (material_flags() & 131072u) == 0u) {
        // q3map2 `_minlight` is a floor on static surface lighting, using the
        // same worldspawn `_color` as `_ambient`. Applying it here after the
        // q3map point-light accumulation reproduces that minimum in normalized
        // lightmap space.
        base = vec4<f32>(max(base.rgb, albedo * lighting_settings.map_minlight.rgb), base.a);
    }
    if (source_map_unbaked_receiver && !ENABLE_MAP_LIGHT_SIMULATION) {
        // OFF is an editor-style global-light/fullbright source-map preview.
        // Enforce the diffuse texture as a neutral 1.0 light floor *after* the
        // normal lighting/shadow paths, so missing baked lightmaps, AO, CSM or
        // other optional lighting features cannot turn raw .map surfaces black.
        // Dynamic/emissive effects may still brighten the surface above this.
        base = vec4<f32>(max(base.rgb, albedo), base.a);
    }
    if (ENABLE_WEATHER_SURFACE && (wetness > 0.001 || puddle > 0.001)) {
        base = vec4<f32>(base.rgb + weather_surface_coat(input, weather) + weather_sun_glint(input, weather), base.a);
    }
    if (ENABLE_PBR && (material_flags() & 256u) != 0u) {
        // Emissive companions are authored as color data (sRGB texture format,
        // sampled here in linear space). They intentionally bypass scene light
        // attenuation, while bloom/HDR can consume the resulting bright color.
        base = vec4<f32>(base.rgb + textureSample(emissive_texture, base_sampler, surface_uv).rgb, base.a);
    }
    if (!ENABLE_SURFACE_DEFORMATION) {
        return base;
    }
    return shade_surface_deformation(
        input.world_position,
        input.world_normal,
        material_flags_w(),
        material_tc_gen(),
        input.shell_kind,
        input.shell_coverage,
        base
    );
}

@fragment fn fs_legacy_dlight_pass(
    input: VertexOut,
    @builtin(front_facing) _front: bool,
) -> @location(0) vec4<f32> {
    if (input.legacy_dlight_bits == 0u || (material_flags() & 4u) == 0u || camera.render_flags.x != 0u) {
        discard;
    }

    // OpenJK performs this as a purpose-built projected-light redraw after the
    // authored material stages. Do not call shade_surface here: the previous
    // lit-minus-unlit implementation evaluated the complete world material
    // twice for every dlight fragment and dominated GPU time.
    var contribution = openjk_projected_dlight_contribution(input);
    if (dot(contribution, contribution) <= 1.0e-12) {
        discard;
    }

    // Separate Legacy fog has already been composited into the base world pass.
    // The old inline dlight lived underneath it, so attenuate this late additive
    // redraw by the same transmittance without re-running any material stages.
    if (ENABLE_LEGACY_FOG && !(ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) {
        contribution *= 1.0 - legacy_separate_fog(input.world_position).a;
    }
    return vec4<f32>(contribution, 0.0);
}


// Unified legacy RGB policy: the opaque lightmap/overlay fusion below can
// compose its known inputs exactly in encoded sRGB. Additive BSP passes have
// an unknown framebuffer destination, so their encoded source is a no-pass
// approximation while blending remains linear in the scene target. Both use
// the SAME transfer function. Keep alpha and authored blend factors unchanged.
const MATERIAL_LEGACY_ADDITIVE_SRGB: u32 = 536870912u;
// Approximate TaystJK's additive SOURCE modulation: its 8-bit rgbGen wave
// multiplies the encoded texture sample before the blend. The scene target
// remains linear, so this does not emulate encoded-framebuffer destination
// blending (which would require access to the destination color).
fn legacy_additive_source(source: vec4<f32>, waveform: f32, has_wave: bool) -> vec4<f32> {
    if (!has_wave) {
        return source;
    }
    if (waveform <= 0.0) {
        return vec4<f32>(vec3<f32>(0.0), source.a);
    }
    // Textures were sRGB-decoded by sampling and modulated by rgbGen wave.
    // Recover their linear sample, perform the legacy modulation in encoded
    // space with the reference 8-bit wave, then decode for WGPU blending.
    let texel_linear = max(source.rgb / waveform, vec3<f32>(0.0));
    let vertex_wave_byte = round(clamp(waveform, 0.0, 1.0) * 255.0) / 255.0;
    return vec4<f32>(legacy_rgb_decode(legacy_rgb_encode(texel_linear) * vertex_wave_byte), source.a);
}
fn legacy_rgb_output(color: vec4<f32>) -> vec4<f32> {
    if ((material_flags() & MATERIAL_LEGACY_ADDITIVE_SRGB) != 0u) {
        let has_wave = (material_flags() & 134217728u) != 0u;
        var wave = 1.0;
        if (has_wave) {
            wave = clamp(wave_value(material.wave_funcs.x, material.wave_rgb, shader_stage_time()), 0.0, 1.0);
        }
        return legacy_additive_source(color, wave, has_wave);
    }
    return color;
}

fn legacy_rgb_encode(value: vec3<f32>) -> vec3<f32> {
    let v = max(value, vec3<f32>(0.0));
    return select(1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055),
                  12.92 * v, v <= vec3<f32>(0.0031308));
}
fn legacy_rgb_decode(value: vec3<f32>) -> vec3<f32> {
    let v = max(value, vec3<f32>(0.0));
    return select(pow((v + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)),
                  v / 12.92, v <= vec3<f32>(0.04045));
}
fn classic_three_stage_rgb(input: VertexOut, shaded_base: vec4<f32>) -> vec4<f32> {
    // 0: base diffuse; 2: baked lightmap; 5: stage-three overlay (non-sky).
    let diffuse = textureSample(base_texture, base_sampler, input.uv).rgb;
    let lightmap = textureSample(lightmap_texture, lightmap_sampler, input.lightmap_uv).rgb;
    let overlay_sample = textureSample(sky_rt, sky_sampler, input.uv);
    // alphaGen vertex multiplies the authored image alpha, not replaces it.
    let vertex_alpha = clamp(input.color.a * overlay_sample.a, 0.0, 1.0);
    let encoded = mix(
        legacy_rgb_encode(diffuse) * legacy_rgb_encode(lightmap),
        legacy_rgb_encode(overlay_sample.rgb), vertex_alpha);
    // Runtime sun/cloud shadow, static AO and dynamic-light adjustments from
    // the normal world pipeline should not disappear when stages are fused.
    // Apply their delta to the legacy-composited result, attenuated by the
    // authored overlay coverage. With no extra lights, this is exactly mode 12.
    let original_base = diffuse * lightmap;
    let extra_lighting = (shaded_base.rgb - original_base) * (1.0 - vertex_alpha);
    return vec4<f32>(max(legacy_rgb_decode(encoded) + extra_lighting,
        vec3<f32>(0.0)), 1.0);
}

@fragment fn fs_legacy_srgb_three_stage(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (ENABLE_SURFACE_DEFORMATION && surface_deformation_should_discard(input.world_position, material_flags_w(),
        input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy,
        (!ENABLE_PLANAR_REFLECTIONS || camera.render_flags.x == 0u))) {
        discard;
    }
    let shaded_base = shade_surface(input, front);
    if ((ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) { return shaded_base; }
    let result = apply_jump_shade(classic_three_stage_rgb(input, shaded_base), input.world_position);
    if (ENABLE_LEGACY_FOG) { return apply_legacy_fog(result, input.world_position); }
    return result;
}


// Source additive draws previously computed each of these transformations in
// its own vertex shader. Reversing the primary affine tcMods gives the exact
// common pre-modified coordinates; applying each sibling's own tcMods then
// evaluates the same independent animated UV in one fragment shader.
fn additive_time(m: Material) -> f32 {
    let elapsed = camera.camera_pos_time.w - m.params.w;
    return select(elapsed, min(elapsed, m.vector_t.w), m.vector_t.w > 0.0);
}
fn additive_transform_uv(raw: vec2<f32>, m: Material) -> vec2<f32> {
    var uv = raw;
    let time = additive_time(m);
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (i >= m.header.y) { break; }
        let a = m.mods[i * 2u];
        let b = m.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            let scroll = a.yz * time;
            uv += scroll - floor(scroll);
        } else if (kind == 2u) {
            uv *= a.yz;
        } else if (kind == 3u) {
            let radians = -a.y * time * 0.017453292519943295;
            let c = cos(radians);
            let sn = sin(radians);
            let p = uv - vec2<f32>(0.5);
            uv = vec2<f32>(p.x * c - p.y * sn, p.x * sn + p.y * c) + vec2<f32>(0.5);
        } else if (kind == 4u) {
            uv = vec2<f32>(uv.x * a.y + uv.y * a.z + a.w,
                           uv.x * b.x + uv.y * b.y + b.z);
        }
    }
    return uv;
}
fn additive_untransform_uv(modified: vec2<f32>, m: Material) -> vec2<f32> {
    var uv = modified;
    let time = additive_time(m);
    for (var j = 0u; j < 4u; j = j + 1u) {
        if (j >= m.header.y) { break; }
        let i = m.header.y - 1u - j;
        let a = m.mods[i * 2u];
        let b = m.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            let scroll = a.yz * time;
            uv -= scroll - floor(scroll);
        } else if (kind == 2u) {
            uv /= a.yz;
        } else if (kind == 3u) {
            let radians = a.y * time * 0.017453292519943295;
            let c = cos(radians);
            let sn = sin(radians);
            let p = uv - vec2<f32>(0.5);
            uv = vec2<f32>(p.x * c - p.y * sn, p.x * sn + p.y * c) + vec2<f32>(0.5);
        } else if (kind == 4u) {
            let determinant = a.y * b.y - a.z * b.x;
            let p = uv - vec2<f32>(a.w, b.z);
            uv = vec2<f32>((p.x * b.y - p.y * a.z) / determinant,
                           (p.y * a.y - p.x * b.x) / determinant);
        }
    }
    return uv;
}
// The source stage tcMods run on vertices in JKA. A fused fragment can use
// interpolated UVs for its companions instead of rebuilding/inverting tcMods
// for each covered pixel (especially expensive on screen-filling water).
fn additive_vertex_output(base: VertexOut, count: u32) -> AdditiveVertexOut {
    var out: AdditiveVertexOut;
    out.clip_position = base.clip_position;
    out.uv = base.uv;
    out.color = base.color;
    out.world_position = base.world_position;
    out.world_normal = base.world_normal;
    out.shell_kind = base.shell_kind;
    out.shell_coverage = base.shell_coverage;
    let raw_uv = additive_untransform_uv(base.uv, material);
    out.additive_uv1 = additive_transform_uv(raw_uv, additive_stage1);
    out.additive_uv2 = vec2<f32>(0.0);
    if (count == 3u) {
        out.additive_uv2 = additive_transform_uv(raw_uv, additive_stage2);
    }
    return out;
}

@vertex fn vs_additive_fused2(input: VertexIn, @builtin(instance_index) instance_index: u32) -> AdditiveVertexOut {
    return additive_vertex_output(world_vertex(input, instance_index, 0xffffffffu), 2u);
}
@vertex fn vs_additive_fused3(input: VertexIn, @builtin(instance_index) instance_index: u32) -> AdditiveVertexOut {
    return additive_vertex_output(world_vertex(input, instance_index, 0xffffffffu), 3u);
}

@vertex fn vs_additive_fused2_legacy(input: VertexIn, @location(6) legacy_dlight_surface_id: u32,
    @builtin(instance_index) instance_index: u32) -> AdditiveVertexOut {
    return additive_vertex_output(world_vertex(input, instance_index, legacy_dlight_surface_id), 2u);
}
@vertex fn vs_additive_fused3_legacy(input: VertexIn, @location(6) legacy_dlight_surface_id: u32,
    @builtin(instance_index) instance_index: u32) -> AdditiveVertexOut {
    return additive_vertex_output(world_vertex(input, instance_index, legacy_dlight_surface_id), 3u);
}

fn additive_stage_tint(m: Material, vertex_color: vec4<f32>) -> vec4<f32> {
    var color = m.color;
    if ((m.header.z & 1u) != 0u) {
        color = vec4<f32>(color.rgb * vertex_color.rgb, color.a);
    } else if ((m.header.z & 524288u) != 0u) {
        color = vec4<f32>(color.rgb * (vec3<f32>(1.0) - vertex_color.rgb), color.a);
    }
    if ((m.header.z & 1048576u) != 0u) {
        color.a *= vertex_color.a;
    } else if ((m.header.z & 2097152u) != 0u) {
        color.a *= (1.0 - vertex_color.a);
    }
    if ((m.header.z & 134217728u) != 0u) {
        let wave = clamp(wave_value(m.wave_funcs.x, m.wave_rgb, additive_time(m)), 0.0, 1.0);
        color = vec4<f32>(vec3<f32>(wave), color.a);
    }
    if ((m.header.z & 268435456u) != 0u) {
        color.a = clamp(wave_value(m.wave_funcs.y, m.wave_alpha, additive_time(m)), 0.0, 1.0);
    }
    return color;
}
fn additive_weighted_sample(texel: vec4<f32>, m: Material, vertex_color: vec4<f32>) -> vec4<f32> {
    var sample = texel * additive_stage_tint(m, vertex_color);
    if ((m.header.z & MATERIAL_LEGACY_ADDITIVE_SRGB) != 0u) {
        let has_wave = (m.header.z & 134217728u) != 0u;
        var wave = 1.0;
        if (has_wave) {
            wave = clamp(wave_value(m.wave_funcs.x, m.wave_rgb, additive_time(m)), 0.0, 1.0);
        }
        sample = legacy_additive_source(sample, wave, has_wave);
    }
    // Authored source blend is GL_SRC_ALPHA GL_ONE. Fused fixed blend is ONE/ONE,
    // so preweight RGB and alpha by the source alpha as before.
    return vec4<f32>(sample.rgb * sample.a, sample.a * sample.a);
}
fn additive_fused_contributions(input: AdditiveVertexOut, count: u32) -> vec4<f32> {
    let base = textureSample(base_texture, base_sampler, input.uv);
    var total = additive_weighted_sample(base, material, input.color);
    // Every companion shares the same primary inverse tcMods. Evaluate that
    // inverse once per fragment, then apply independent authored sibling tcMods.
    // Three-stage fusion previously recomputed the inverse twice; no sampling,
    // stage ordering, blending or shader permutation changes.
    let uv1 = input.additive_uv1;
    if ((additive_stage1.header.w & 65536u) != 0u) {
        total += additive_weighted_sample(textureSample(sky_rt, additive_clamp_sampler, uv1), additive_stage1, input.color);
    } else {
        total += additive_weighted_sample(textureSample(sky_rt, detail_sampler, uv1), additive_stage1, input.color);
    }
    if (count >= 3u) {
        let uv2 = input.additive_uv2;
        if ((additive_stage2.header.w & 65536u) != 0u) {
            total += additive_weighted_sample(textureSample(sky_bk, additive_clamp_sampler, uv2), additive_stage2, input.color);
        } else {
            total += additive_weighted_sample(textureSample(sky_bk, detail_sampler, uv2), additive_stage2, input.color);
        }
    }
    return total;
}


@fragment fn fs_additive_fused2(input: AdditiveVertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (ENABLE_SURFACE_DEFORMATION && surface_deformation_should_discard(input.world_position, material_flags_w(),
        input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy,
        (!ENABLE_PLANAR_REFLECTIONS || camera.render_flags.x == 0u))) { discard; }
    return additive_fused_contributions(input, 2u);
}
@fragment fn fs_additive_fused3(input: AdditiveVertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (ENABLE_SURFACE_DEFORMATION && surface_deformation_should_discard(input.world_position, material_flags_w(), input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy, (!ENABLE_PLANAR_REFLECTIONS || camera.render_flags.x == 0u))) { discard; }
    return additive_fused_contributions(input, 3u);
}
// Secondary views have no matching transmission capture. Main-view glass
// uses the dedicated shared shader, independent of world shader family.
fn promoted_glass(input: VertexOut, front: bool) -> vec4<f32> {
    let to_eye = normalize(camera.camera_pos_time.xyz - input.world_position);
    let facing = abs(dot(normalize(input.world_normal), to_eye));
    let f0 = clamp(material.ocean_color.a, 0.02, 0.18);
    let fresnel = f0 + (1.0 - f0) * pow(1.0 - clamp(facing, 0.0, 1.0), 5.0);
    return vec4<f32>(mix(vec3<f32>(0.65), material.ocean_color.rgb, 0.35), 0.08 + 0.55 * fresnel);
}

// Debug only. The vertex path has already applied the FFT, swell, coastal,
// interaction, horizontal, distance and edge displacement; this is its actual
// rasterized world-space position, interpolated across the visible triangle.
// It does NOT claim to be a camera-XZ wave-height probe.
fn hybrid_rendered_mesh_debug(input: VertexOut) -> vec4<f32> {
    let delta = input.world_position.y - camera.camera_pos_time.y;
    if (abs(delta) <= 8.0) { return vec4<f32>(1.0, 0.82, 0.02, 1.0); }
    if (delta > 0.0) { return vec4<f32>(0.02, 0.92, 0.19, 1.0); }
    return vec4<f32>(0.98, 0.08, 0.08, 1.0);
}

@fragment fn fs_main(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (ENABLE_SURFACE_DEFORMATION && surface_deformation_should_discard(
        input.world_position, material_flags_w(), input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy,
        (!ENABLE_PLANAR_REFLECTIONS || camera.render_flags.x == 0u)
    )) {
        discard;
    }
    if (input.ocean_vertex >= 1.0 && ocean.appearance_b.w >= 8.0) {
        return hybrid_rendered_mesh_debug(input);
    }
    if ((material_flags_w() & (1u << 17u)) != 0u) {
        return promoted_glass(input, front);
    }
    let shaded = shade_surface(input, front);
    if ((ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) {
        return shaded;
    }
    return legacy_rgb_output(apply_jump_shade(shaded, input.world_position));
}

@fragment fn fs_main_legacy_fog(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (ENABLE_SURFACE_DEFORMATION && surface_deformation_should_discard(
        input.world_position, material_flags_w(), input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy,
        (!ENABLE_PLANAR_REFLECTIONS || camera.render_flags.x == 0u)
    )) {
        discard;
    }
    if (input.ocean_vertex >= 1.0 && ocean.appearance_b.w >= 8.0) {
        return hybrid_rendered_mesh_debug(input);
    }
    if ((material_flags_w() & (1u << 17u)) != 0u) {
        return promoted_glass(input, front);
    }
    let shaded = shade_surface(input, front);
    if ((ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) {
        return shaded;
    }
    return legacy_rgb_output(apply_legacy_fog(apply_jump_shade(shaded, input.world_position), input.world_position));
}

@fragment fn fs_legacy_fog_pass(input: VertexOut, @builtin(front_facing) _front: bool) -> @location(0) vec4<f32> {
    if ((ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) {
        discard;
    }
    if (ENABLE_SURFACE_DEFORMATION && surface_deformation_should_discard(
        input.world_position, material_flags_w(), input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy,
        (!ENABLE_PLANAR_REFLECTIONS || camera.render_flags.x == 0u)
    )) {
        discard;
    }
    let fog = legacy_separate_fog(input.world_position);
    // Only Legacy 2 (EXP2) bands visibly; Legacy 1 keeps its authored 8-bit table.
    if (legacy_fog.values.w >= 1.5) {
        return legacy_fog_dither_alpha(fog, input.clip_position.xy);
    }
    return fog;
}

fn sky_uv(s: f32, t: f32) -> vec2<f32> {
    // Quake 3 MakeSkyVec maps [-1,1] to [0,1] and flips T. Keep a tiny
    // inset to avoid bilinear/trilinear sampling across a face edge.
    return clamp(vec2<f32>((s + 1.0) * 0.5, (1.0 - t) * 0.5), vec2<f32>(0.001), vec2<f32>(0.999));
}

// Cloud layer of a sky shader (R_InitSkyTexCoords). The view direction `d`
// (JKA axes, z up) is projected onto a spherical shell `height` above a
// 4096-unit-radius ground sphere, and the direction from the sphere's centre to
// that point gives the base coordinates as two angles in radians. The stage's
// tcMods are then applied on top, exactly like any other stage.
fn sky_cloud_uv(d: vec3<f32>, height: f32) -> vec2<f32> {
    let radius = 4096.0;
    let dd = dot(d, d);
    let discriminant = d.z * d.z * radius * radius + dd * (2.0 * radius * height + height * height);
    let p = (-2.0 * d.z * radius + 2.0 * sqrt(discriminant)) / (2.0 * dd);
    var v = d * p;
    v.z = v.z + radius;
    v = normalize(v);
    return vec2<f32>(acos(clamp(v.x, -1.0, 1.0)), acos(clamp(v.y, -1.0, 1.0)));
}

fn sky_cloud_stage_color(vertex_color: vec4<f32>) -> vec4<f32> {
    var stage_color = material.color;
    if ((material_flags() & 1u) != 0u) {
        stage_color = vec4<f32>(stage_color.rgb * vertex_color.rgb, stage_color.a);
    } else if ((material_flags() & 524288u) != 0u) {
        stage_color = vec4<f32>(stage_color.rgb * (vec3<f32>(1.0) - vertex_color.rgb), stage_color.a);
    }
    if ((material_flags() & 1048576u) != 0u) {
        stage_color.a = stage_color.a * vertex_color.a;
    } else if ((material_flags() & 2097152u) != 0u) {
        stage_color.a = stage_color.a * (1.0 - vertex_color.a);
    }
    return apply_wave_gens(stage_color);
}

fn sky_cloud_color(input: VertexOut, d: vec3<f32>) -> vec4<f32> {
    // The engine never draws clouds on the bottom face of the sky box.
    let a = abs(d);
    if (d.z < 0.0 && a.z >= a.x && a.z >= a.y) {
        discard;
    }
    let uv = apply_tc_mods(sky_cloud_uv(d, material.params.z), vec3<f32>(0.0));
    let base = textureSample(base_texture, base_sampler, uv) * sky_cloud_stage_color(input.color);
    if ((material.params.x > 0.0 && base.a < material.params.x || material.params.x < 0.0 && base.a >= -material.params.x)) {
        discard;
    }
    return base;
}

// The six Quake/JKA sky faces are projective: scaling the direction does
// not change S/T. The old shader normalized d on every pixel BEFORE dividing
// by its dominant component; the normalization cancels exactly. Work with
// the unnormalized view ray, and use one reciprocal for both UV components.
// This retains the BSP sky brush as the visibility mask; unlike moving UVs to
// the raw BSP vertices it cannot introduce cube-face seams when the camera
// moves across a large sky polygon.
fn sample_outer_sky(d: vec3<f32>) -> vec4<f32> {
    let a = abs(d);
    if (a.x >= a.y && a.x >= a.z) {
        let inv = 1.0 / max(a.x, 0.0000001);
        if (d.x >= 0.0) {
            return textureSample(sky_rt, sky_sampler, sky_uv(-d.y * inv, d.z * inv));
        }
        return textureSample(sky_lf, sky_sampler, sky_uv(d.y * inv, d.z * inv));
    }
    if (a.y >= a.x && a.y >= a.z) {
        let inv = 1.0 / max(a.y, 0.0000001);
        if (d.y >= 0.0) {
            return textureSample(sky_bk, sky_sampler, sky_uv(d.x * inv, d.z * inv));
        }
        return textureSample(sky_ft, sky_sampler, sky_uv(-d.x * inv, d.z * inv));
    }
    let inv = 1.0 / max(a.z, 0.0000001);
    if (d.z >= 0.0) {
        return textureSample(sky_up, sky_sampler, sky_uv(-d.y * inv, -d.x * inv));
    }
    return textureSample(sky_dn, sky_sampler, sky_uv(-d.y * inv, d.x * inv));
}

// Fast native PSO for the normal TaystJK outerbox stage only. Its material
// identity guarantees six sky faces, so it never tests cloud/no-outerbox modes
// and never uses discard; this permits early depth rejection of covered sky.
// The optional fractal override still behaves exactly as before.
// Sky cost sweep. Diagnostic modes affect only outer skybox texture shading.
// They deliberately retain sky BSP geometry, its pipeline, and framebuffer writes.
// Mode 1: raster/fill/color write, no texture. Mode 2: one texel repeatedly.
// Mode 3: one face with varying projective UV. Mode 0: original six-face selection.
fn sample_sky_cost_probe(d: vec3<f32>, probe: u32) -> vec4<f32> {
    if (probe == 1u) {
        return vec4<f32>(0.2, 0.4, 0.6, 1.0);
    }
    if (probe == 2u) {
        return textureSample(sky_rt, sky_sampler, vec2<f32>(0.5, 0.5));
    }
    if (probe == 3u) {
        let inv = 1.0 / max(abs(d.z), 0.0000001);
        return textureSample(sky_up, sky_sampler, sky_uv(-d.y * inv, -d.x * inv));
    }
    return sample_outer_sky(d);
}

// While the sky-extension ocean is active (r_oceanSkyExtend on and the camera's
// PVS sees the water) a map with an authored water/vertical-sky contact uses a
// black lower sky hemisphere.
// This is an angular horizon test in JKA Z-up, NOT a water height/depth test.
// Camera render_flags.y bit 16 is set per frame from the extension gate.
fn ocean_connected_sky_below_horizon(direction_z: f32) -> bool {
    return (camera.render_flags.y & 65536u) != 0u && direction_z < 0.0;
}

@fragment fn fs_sky_outer(input: VertexOut) -> @location(0) vec4<f32> {
    if ((ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) {
        return vec4<f32>(0.012, 0.012, 0.012, 1.0);
    }
    let direction = vec3<f32>(input.sky_dir_ao.x, -input.sky_dir_ao.z, input.sky_dir_ao.y);
    if (ocean_connected_sky_below_horizon(direction.z)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    if (ENABLE_SKY_DEBUG) {
        let probe = (camera.render_flags.y >> 8u) & 3u;
        if (probe != 0u) { return sample_sky_cost_probe(direction, probe); }
        if ((camera.render_flags.y & 2u) != 0u) {
            return experimental_star_nest_sky(normalize(direction));
        }
    }
    return sample_outer_sky(direction);
}

@fragment fn fs_sky(input: VertexOut) -> @location(0) vec4<f32> {
    if ((ENABLE_DYNAMIC_LIGHT_DEBUG && dynamic_light_debug_enabled())) {
        return vec4<f32>(0.012, 0.012, 0.012, 1.0);
    }
    // Convert the renderer's [x,z,-y] direction back to JKA coordinates.
    let raw_direction = vec3<f32>(input.sky_dir_ao.x, -input.sky_dir_ao.z, input.sky_dir_ao.y);
    if (ocean_connected_sky_below_horizon(raw_direction.z)) {
        // Clouds may be blended/additive: suppress rather than write an
        // opaque black cloud stage over the already black outer sky.
        if (material_tc_gen() == 4u || (material_flags_w() & 1u) == 0u) { discard; }
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    if (ENABLE_SKY_DEBUG) {
        let probe = (camera.render_flags.y >> 8u) & 3u;
        if (probe != 0u && material_tc_gen() != 4u && (material_flags_w() & 1u) != 0u) {
            return sample_sky_cost_probe(raw_direction, probe);
        }
    }
    let d = normalize(raw_direction);
    // The runtime override replaces sky cubemaps AND authored sky-cloud stages.
    // World geometry is never touched. Restore the OpenJK sky on the next frame
    // by switching this bit off.
    if (ENABLE_SKY_DEBUG && (camera.render_flags.y & 2u) != 0u) {
        if (material_tc_gen() == 4u) { discard; }
        return experimental_star_nest_sky(d);
    }
    if (material_tc_gen() == 4u) {
        // A cloud-layer stage of the sky shader (tcGen sky cloud).
        return sky_cloud_color(input, d);
    }
    if ((material_flags_w() & 1u) == 0u) {
        // skyParms "-" has no outerbox. OpenJK draws no skybox here, so leave
        // the scene clear color visible (global-fog color when the BSP has one).
        discard;
    }

    return sample_outer_sky(d);
}


// Diagnostic-only line overlay. The pipeline reuses vs_main so procedural
// vertex motion (Snowflow/ocean) stays identical to the filled geometry.
@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}

// TaystJK DrawSkyBox: the outer box is drawn as six face quads with a one-texture
// program (vk.std_pipeline.skybox_pipeline, TYPE_SINGLE_TEXTURE_FIXED_COLOR)
// instead of choosing the face per pixel on the BSP sky polygons. The renderer
// draws it after the opaque world at far depth, so the depth test leaves only
// the pixels no world surface covered (TaystJK bounds it by clipping the sky
// polygons on the CPU instead). Each face samples only its own image with the
// same coordinates as sample_outer_sky: (p, q) are the sky_uv inputs on that
// face, so the interpolated UV equals the per-pixel projection exactly.
struct SkyBoxOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) direction_z: f32, // JKA Z-up, interpolated on each sky face
};

// JKA-space view direction of face point (p, q) in [-1, 1]^2: the inverse of
// sample_outer_sky, so (p, q) are that face's sky_uv inputs.
fn sky_box_direction(face: u32, p: f32, q: f32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3<f32>(1.0, -p, q); }   // rt: +X, sky_uv(-d.y, d.z)
        case 1u: { return vec3<f32>(p, 1.0, q); }    // bk: +Y, sky_uv(d.x, d.z)
        case 2u: { return vec3<f32>(-1.0, p, q); }   // lf: -X, sky_uv(d.y, d.z)
        case 3u: { return vec3<f32>(-p, -1.0, q); }  // ft: -Y, sky_uv(-d.x, d.z)
        case 4u: { return vec3<f32>(-q, -p, 1.0); }  // up: +Z, sky_uv(-d.y, -d.x)
        default: { return vec3<f32>(q, -p, -1.0); }  // dn: -Z, sky_uv(-d.y, d.x)
    }
}

// Renderer space is JKA [x, z, -y]. Any box size works: depth is forced to the
// far value exactly as sky_vertex does for the BSP sky polygons.
fn sky_box_clip(d: vec3<f32>) -> vec4<f32> {
    let position = camera.camera_pos_time.xyz + vec3<f32>(d.x, d.z, -d.y) * 1024.0;
    var clip = camera.view_proj * vec4<f32>(position, 1.0);
    clip.z = 0.0;
    return clip;
}

fn sky_box_vertex(face: u32, vertex_index: u32) -> SkyBoxOut {
    // Two triangles over the face, corners in (p, q) = [-1, 1]^2.
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let pq = corners[vertex_index];
    var out: SkyBoxOut;
    let direction = sky_box_direction(face, pq.x, pq.y);
    out.clip_position = sky_box_clip(direction);
    out.uv = vec2<f32>((pq.x + 1.0) * 0.5, (1.0 - pq.y) * 0.5);
    out.direction_z = direction.z;
    return out;
}

// TaystJK FillCloudBox + R_InitSkyTexCoords: a cloud-layer stage is drawn on the
// box faces subdivided SKY_SUBDIVISIONS (8) times, never the bottom face, with
// the cloud coordinate computed per grid vertex and the stage's tcMods applied
// per vertex, then shaded like any single-texture stage. Instance = face 0..4.
struct SkyCloudOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) direction_z: f32,
};

@vertex fn vs_sky_cloud(@builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) face: u32) -> SkyCloudOut {
    let corners = array<vec2<u32>, 6>(
        vec2<u32>(0u, 0u), vec2<u32>(1u, 0u), vec2<u32>(1u, 1u),
        vec2<u32>(0u, 0u), vec2<u32>(1u, 1u), vec2<u32>(0u, 1u),
    );
    let cell = vertex_index / 6u;
    let grid = vec2<u32>(cell % 8u, cell / 8u) + corners[vertex_index % 6u];
    let p = f32(grid.x) * 0.25 - 1.0;
    let q = f32(grid.y) * 0.25 - 1.0;
    let d = sky_box_direction(face, p, q);
    var out: SkyCloudOut;
    out.clip_position = sky_box_clip(d);
    out.uv = apply_tc_mods(sky_cloud_uv(d, material.params.z), vec3<f32>(0.0));
    // Per vertex like TaystJK ComputeColors; cloud-box vertices carry no colour,
    // so rgbGen/alphaGen vertex see white and the result is constant per draw.
    out.color = sky_cloud_stage_color(vec4<f32>(1.0));
    out.direction_z = d.z;
    return out;
}

@fragment fn fs_sky_cloud(input: SkyCloudOut) -> @location(0) vec4<f32> {
    if (ocean_connected_sky_below_horizon(input.direction_z)) { discard; }
    let base = textureSample(base_texture, base_sampler, input.uv) * input.color;
    if (material_alpha_rejects(base.a)) {
        discard;
    }
    return base;
}

@vertex fn vs_sky_box_rt(@builtin(vertex_index) i: u32) -> SkyBoxOut { return sky_box_vertex(0u, i); }
@vertex fn vs_sky_box_bk(@builtin(vertex_index) i: u32) -> SkyBoxOut { return sky_box_vertex(1u, i); }
@vertex fn vs_sky_box_lf(@builtin(vertex_index) i: u32) -> SkyBoxOut { return sky_box_vertex(2u, i); }
@vertex fn vs_sky_box_ft(@builtin(vertex_index) i: u32) -> SkyBoxOut { return sky_box_vertex(3u, i); }
@vertex fn vs_sky_box_up(@builtin(vertex_index) i: u32) -> SkyBoxOut { return sky_box_vertex(4u, i); }
@vertex fn vs_sky_box_dn(@builtin(vertex_index) i: u32) -> SkyBoxOut { return sky_box_vertex(5u, i); }

// sky_uv's edge inset, kept per pixel so the result matches sample_outer_sky.
fn sky_box_uv(uv: vec2<f32>) -> vec2<f32> {
    return clamp(uv, vec2<f32>(0.001), vec2<f32>(0.999));
}
fn sky_box_color(input: SkyBoxOut, color: vec4<f32>) -> vec4<f32> {
    if (ocean_connected_sky_below_horizon(input.direction_z)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return color;
}
@fragment fn fs_sky_box_rt(input: SkyBoxOut) -> @location(0) vec4<f32> { return sky_box_color(input, textureSample(sky_rt, sky_sampler, sky_box_uv(input.uv))); }
@fragment fn fs_sky_box_bk(input: SkyBoxOut) -> @location(0) vec4<f32> { return sky_box_color(input, textureSample(sky_bk, sky_sampler, sky_box_uv(input.uv))); }
@fragment fn fs_sky_box_lf(input: SkyBoxOut) -> @location(0) vec4<f32> { return sky_box_color(input, textureSample(sky_lf, sky_sampler, sky_box_uv(input.uv))); }
@fragment fn fs_sky_box_ft(input: SkyBoxOut) -> @location(0) vec4<f32> { return sky_box_color(input, textureSample(sky_ft, sky_sampler, sky_box_uv(input.uv))); }
@fragment fn fs_sky_box_up(input: SkyBoxOut) -> @location(0) vec4<f32> { return sky_box_color(input, textureSample(sky_up, sky_sampler, sky_box_uv(input.uv))); }
@fragment fn fs_sky_box_dn(input: SkyBoxOut) -> @location(0) vec4<f32> { return sky_box_color(input, textureSample(sky_dn, sky_sampler, sky_box_uv(input.uv))); }

// Phase 2: mesh-free continuation outside the camera-relative displaced grid.
// A single fullscreen triangle produces world-space intersections with the
// promoted water plane. No enormous BSP mesh, CPU tessellation, or far LOD
// update is needed. This entrypoint has its own lazy PSO, compiled only when
// r_oceanSkyExtend=2, and reuses the SAME per-pool material/FFT/optics bindings.
struct OceanFarVertex {
    @builtin(position) position: vec4<f32>,
};

@vertex fn vs_ocean_far(@builtin(vertex_index) i: u32) -> OceanFarVertex {
    var output: OceanFarVertex;
    // Standard oversize fullscreen triangle, no vertex buffer.
    let corner = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    output.position = vec4<f32>(corner * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    return output;
}

struct OceanFarFragment {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment fn fs_ocean_far(pixel: OceanFarVertex) -> OceanFarFragment {
    // The inverse view-projection is already in the optical capture uniform,
    // updated using this exact frame's (possibly TAA-jittered) view matrix.
    let dimensions = vec2<f32>(textureDimensions(ocean_scene));
    let ndc = vec2<f32>(pixel.position.x / dimensions.x * 2.0 - 1.0,
        1.0 - pixel.position.y / dimensions.y * 2.0);
    // Reverse-Z perspective: avoid exactly 0 at the infinite far plane where
    // inverse projection produces homogeneous w=0.
    let a = ocean_optics.inverse_view_projection * vec4<f32>(ndc, 0.75, 1.0);
    let b = ocean_optics.inverse_view_projection * vec4<f32>(ndc, 0.25, 1.0);
    let ray = normalize(b.xyz / b.w - a.xyz / a.w);
    let eye = camera.camera_pos_time.xyz;
    let sea_height = ocean.spray_planes[0].x; // promoted BSP pool plane; Y up
    if (abs(ray.y) < 1.0e-7) { discard; }
    let distance_to_plane = (sea_height - eye.y) / ray.y;
    if (distance_to_plane <= 0.0) { discard; }
    let world = eye + ray * distance_to_plane;

    // The clipmap's current outermost *drawn* ring, not a made-up extension
    // distance or the original BSP footprint. For a square projected grid,
    // max(abs(dx),abs(dz)) is the matching exterior coverage predicate.
    let radius = ocean.appearance_c.w;
    // The mesh edge and this per-pixel test are two independent evaluations of the
    // same square, so a one-pixel line can fall between them and show the clear
    // colour. With the seam flattened (the edge flatten) the mesh is nearly flat
    // there, so start half an outer cell inside it and let the depth test pick.
    let edge_mode = floor(ocean.appearance_a.w);
    var seam_overlap = 0.0;
    if (radius > 0.0 && edge_mode == 1.0) {
        seam_overlap = 0.5 * min(max(ocean.wind_motion.w, 1.0), radius / max(ocean.authored_plane.y, 1.0));
    }
    if (radius <= 0.0 || max(abs(world.x-ocean.clipmap.x),
        abs(world.z-ocean.clipmap.y)) < radius - seam_overlap) { discard; }
    let pool_slot = i32(round(ocean.spray_planes[0].z));
    if (!ocean_mask_contains(pool_slot, world.xz)) { discard; }

    let projected = camera.view_proj * vec4<f32>(world, 1.0);
    // Reverse-Z maps the far horizon toward zero. Clamp the infinite part
    // just above sky clear depth (0), retaining foreground depth occlusion.
    let depth = clamp(projected.z / projected.w, 0.000001, 1.0);

    // Feed the existing physical/optical water shading with reconstructed
    // fragment inputs. Exterior shading already bypasses coastal bathymetry.
    var surface: VertexOut;
    surface.clip_position = pixel.position;
    surface.uv = vec2<f32>(0.0);
    surface.lightmap_uv = vec2<f32>(0.0);
    surface.color = vec4<f32>(1.0);
    surface.sky_dir_ao = vec4<f32>(world-eye, 0.0);
    surface.world_position = world;
    surface.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    surface.shell_kind = 0u;
    surface.shell_coverage = 0.0;
    surface.ocean_wave_height = 0.0;
    surface.ocean_uv_m = world.xz / max(ocean.ocean_info.z, 0.001);
    surface.ocean_vertex = 2.0 + f32(pool_slot);
    surface.pbr_lightgrid_direction = vec4<f32>(0.0);
    surface.pbr_lightgrid_lighting = vec4<f32>(0.0);
    surface.vertex_dlight = vec3<f32>(0.0);
    surface.legacy_dlight_bits = 0u;

    var result: OceanFarFragment;
    // Keep analytic pixels on the same material, optics and fog path as the
    // displaced mesh.
    let shaded = shade_surface(surface, eye.y >= sea_height);
    result.color = legacy_rgb_output(apply_jump_shade(shaded, world));
    result.depth = depth;
    return result;
}
