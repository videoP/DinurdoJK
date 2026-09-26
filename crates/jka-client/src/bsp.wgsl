struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
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
    emitter: vec4<f32>, // xyz normal; w: 0 point, 1 one-sided area, 2 two-sided area
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
    map_ambient: vec4<f32>, // xyz q3map2 ambient RGB; w reserved
    map_minlight: vec4<f32>, // source-.map q3map2 minlight RGB; unused by compute/fog consumers
};
struct PbrSettings {
    // x: PBR companions enabled, y: parallax enabled, z: height scale * 1000.
    values: vec4<u32>,
    // x: directional baked lighting enabled, y: deluxe specular scale.
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
};
struct SurfaceFogSettings {
    color_depth: vec4<f32>,
    flags: vec4<f32>, // global, color override, Legacy2 in-stage safe, Legacy1 post eligible
};
struct LegacyFogControl {
    values: vec4<f32>, // enabled, strength/scale, map has authored fog, r_drawfog mode
};
struct WeatherSurfaceSettings {
    amount_distance: vec4<f32>,
    puddle: vec4<f32>, // accumulation, time, active-rain ripple strength, reserved
    occlusion_uv: vec4<f32>,
    occlusion_size: vec4<u32>,
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
};
struct OceanOptics {
    inverse_view_projection: mat4x4<f32>, eye: vec4<f32>, forward: vec4<f32>,
    fog: vec4<f32>, params: vec4<f32>, minimum: array<vec4<f32>,8>, maximum: array<vec4<f32>,8>,
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
@group(6) @binding(1) var ocean_normal: texture_2d_array<f32>;
@group(6) @binding(2) var ocean_sampler: sampler;
@group(6) @binding(3) var<uniform> ocean: OceanRenderSettings;
@group(6) @binding(5) var<storage, read> ocean_windrow: array<f32>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lightmap_uv: vec2<f32>,
    @location(5) normal: vec3<f32>,
    @location(3) color: vec4<f32>,
    @location(4) static_ao: f32,
};
struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
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
    @location(10) ocean_vertex: f32,
    // Optional vertex-sampled static lightgrid data. Disabled by default because
    // interpolation is an accuracy/performance tradeoff; the fragment path remains authoritative.
    @location(11) pbr_lightgrid_direction: vec4<f32>,
    @location(12) pbr_lightgrid_lighting: vec4<f32>,
    @location(13) vertex_dlight: vec3<f32>,
};

fn generated_uv(input: VertexIn) -> vec2<f32> {
    var uv = input.uv;
    if (material.header.x == 1u) {
        uv = input.lightmap_uv;
    }
    if (material.header.x == 2u) {
        uv = vec2<f32>(
            dot(input.position, material.vector_s.xyz),
            dot(input.position, material.vector_t.xyz)
        );
    }
    if (material.header.x == 3u) {
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

    let time = camera.camera_pos_time.w;
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (i >= material.header.y) {
            break;
        }
        let a = material.mods[i * 2u];
        let b = material.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            uv = uv + a.yz * time;
        }
        if (kind == 2u) {
            uv = uv * a.yz;
        }
        if (kind == 3u) {
            let radians = a.y * time * 0.017453292519943295;
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
        if (kind == 5u) {
            let wave = a.y
                + sin((a.w + time * b.x + uv.x + uv.y) * 6.28318530718) * a.z;
            uv = uv + vec2<f32>(wave);
        }
    }
    return uv;
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
fn ocean_clipmap_position(input: VertexIn) -> vec3<f32> {
    let minimum = select(input.uv, ocean.authored_bounds.xy, ocean.authored_plane.x > 0.0);
    let maximum = select(input.lightmap_uv, ocean.authored_bounds.zw, ocean.authored_plane.x > 0.0);

    // The clipmap follows the viewer, but the viewer is often nowhere near the
    // water - a lake is a small part of a map. Pulling the centre back into the
    // footprint keeps the fine rings on the nearest water instead of folding
    // the whole mesh onto one edge, and re-snapping to the same lattice
    // afterwards keeps every level aligned.
    let step = max(ocean.clipmap.z, 1.0);
    let centre = round(clamp(ocean.clipmap.xy, minimum, maximum) / step) * step;

    var offset = input.position.xz;
    let snap = input.color.g;
    if (snap > 0.0) {
        offset = round(offset / snap) * snap;
    }
    let folded = clamp(centre + offset, minimum, maximum);
    return vec3<f32>(folded.x, select(input.position.y, ocean.authored_plane.y, ocean.authored_plane.x > 0.0), folded.y);
}

fn sample_ocean_displacement(position: vec3<f32>, is_ocean: bool) -> vec4<f32> {
    if (!is_ocean) {
        return vec4<f32>(0.0);
    }
    let units_per_meter = max(ocean.ocean_info.z, 0.001);
    let camera_distance_m = distance(position.xz, camera.camera_pos_time.xz) / units_per_meter;
    // Direct port of water.gdshader vertex(): sample in the undisplaced world
    // XZ plane, preserve the raw wave height, then attenuate displacement only.
    let distance_factor = min(exp(-(camera_distance_m - 150.0) * 0.007), 1.0);
    let world_m = position.xz / units_per_meter;
    var displacement_m = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let scale = ocean.map_scales[i];
        displacement_m += textureSampleLevel(
            ocean_displacement, ocean_sampler, world_m * vec2<f32>(1.0,-1.0) * scale.xy, i32(i), 0.0
        ).xyz * vec3<f32>(1.0,1.0,-1.0) * scale.z;
    }
    displacement_m.x *= ocean.swell_motion.z;
    displacement_m.z *= ocean.swell_motion.z;
    let phase = dot(world_m, ocean.swell.xy) * ocean.swell.z - ocean.swell_motion.x * ocean.swell_motion.w;
    let horizontal = ocean.swell.w * ocean.swell_motion.y * cos(phase);
    displacement_m += vec3<f32>(ocean.swell.x * horizontal, ocean.swell.w * sin(phase), ocean.swell.y * horizontal);
    return vec4<f32>(displacement_m * distance_factor, displacement_m.y);
}

struct VertexLightgridSample {
    direction: vec4<f32>,
    lighting: vec4<f32>,
};

fn sample_vertex_static_lightgrid(world_position: vec3<f32>) -> VertexLightgridSample {
    var result: VertexLightgridSample;
    result.direction = vec4<f32>(0.0);
    result.lighting = vec4<f32>(0.0);
    let has_pbr_brdf_map = (material.header.z & (8u | 16u | 64u | 128u)) != 0u;
    if (!PBR_VERTEX_LIGHTGRID
        || !ENABLE_PBR
        || static_lightgrid.origin_enabled.w < 0.5
        || (material.header.z & 2u) == 0u
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
        return 1.70;
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
        let falloff = max(1.0 - distance_to_light / radius, 0.0);
        result += light.color_intensity.rgb * light.color_intensity.a
            * (falloff * falloff) * ndotl * 0.70;
    }
    return result;
}

@vertex fn vs_main(input: VertexIn, @builtin(instance_index) instance_index: u32) -> VertexOut {
    var output: VertexOut;
    let allow_shell = camera.render_flags.x == 0u;
    let ocean_vertex = is_ocean_vertex(input);
    var source_position = input.position;
    var source_normal = input.normal;
    if (ocean_vertex) {
        source_position = ocean_clipmap_position(input);
        source_normal = vec3<f32>(0.0, 1.0, 0.0);
    }
    let deformation = deform_surface_deformation_vertex(
        source_position,
        source_normal,
        material.header.w,
        instance_index,
        allow_shell
    );
    let ocean_sample = sample_ocean_displacement(deformation.xyz, ocean_vertex);
    let ocean_displacement_m = ocean_sample.xyz;
    let world_position = deformation.xyz + ocean_displacement_m * max(ocean.ocean_info.z, 0.001);
    output.clip_position = camera.view_proj * vec4<f32>(world_position, 1.0);
    output.uv = generated_uv(input);
    output.lightmap_uv = input.lightmap_uv;
    output.color = input.color;
    output.sky_dir_ao = vec4<f32>(world_position - camera.camera_pos_time.xyz, input.static_ao);
    output.world_position = world_position;
    output.world_normal = surface_deformation_vertex_normal(
        source_position, source_normal, material.header.w, instance_index, allow_shell
    );
    output.vertex_dlight = transient_vertex_dlight(world_position, output.world_normal);
    output.shell_kind = select(0u, 1u, instance_index != 0u);
    output.shell_coverage = deformation.w;
    output.ocean_wave_height = ocean_sample.w;
    output.ocean_uv_m = deformation.xz / max(ocean.ocean_info.z, 0.001);
    output.ocean_vertex = select(0.0, 1.0, ocean_vertex);
    let vertex_lightgrid = sample_vertex_static_lightgrid(world_position);
    output.pbr_lightgrid_direction = vertex_lightgrid.direction;
    output.pbr_lightgrid_lighting = vertex_lightgrid.lighting;
    return output;
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
    if (!ENABLE_POM || (material.header.z & 32u) == 0u) {
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
    var sampled_depth = 1.0 - select(first_height_sample.r, first_height_sample.a, (material.header.z & 1024u) != 0u);
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
        sampled_depth = 1.0 - select(height_sample.r, height_sample.a, (material.header.z & 1024u) != 0u);
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
    if (!ENABLE_PBR || (material.header.z & 8u) == 0u) {
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
    return sample_shadow_map_bevy_castano(uv, ndc.z, cascade);
}

fn shader_sun_sky_admission(input: VertexOut, cascade: i32) -> f32 {
    let mode = shadow_settings.camera_forward.w;
    if (mode < 0.5) {
        return 1.0;
    }
    let clip = shadow_settings.view_proj[u32(cascade)] * vec4<f32>(input.world_position, 1.0);
    if (clip.w <= 0.0) {
        return 0.0;
    }
    let ndc = clip.xyz / clip.w;
    if (any(ndc.xy < vec2<f32>(-1.0)) || ndc.z < 0.0 || any(ndc > vec3<f32>(1.0))) {
        return 0.0;
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
    if (cascade < 2i) {
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

fn local_light_shadow_visibility(light: PointLight, world_position: vec3<f32>, receiver_normal: vec3<f32>) -> f32 {
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
    let depth_ref = far_plane / (far_plane - LOCAL_SHADOW_NEAR)
        - (far_plane * LOCAL_SHADOW_NEAR) / ((far_plane - LOCAL_SHADOW_NEAR) * major_distance);
    let to_light = normalize(-from_light);
    let slope = 1.0 - max(dot(normalize(receiver_normal), to_light), 0.0);
    let bias = 0.0012 + 0.0025 * slope;
    let direction = normalize(from_light);
    let helper = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(direction.y) < 0.9);
    let tangent = normalize(cross(direction, helper));
    let bitangent = normalize(cross(direction, tangent));
    let spread = 1.6 / f32(max(lighting_settings.local_shadows.z, 1u));
    let shadow_index = u32(light.shadow.x - 1.0);
    var visibility = 0.0;
    visibility += textureSampleCompareLevel(local_shadow_texture, local_shadow_sampler, normalize(direction + tangent * spread + bitangent * spread), shadow_index, depth_ref - bias);
    visibility += textureSampleCompareLevel(local_shadow_texture, local_shadow_sampler, normalize(direction - tangent * spread + bitangent * spread), shadow_index, depth_ref - bias);
    visibility += textureSampleCompareLevel(local_shadow_texture, local_shadow_sampler, normalize(direction + tangent * spread - bitangent * spread), shadow_index, depth_ref - bias);
    visibility += textureSampleCompareLevel(local_shadow_texture, local_shadow_sampler, normalize(direction - tangent * spread - bitangent * spread), shadow_index, depth_ref - bias);
    return visibility * 0.25;
}

fn emitter_visibility(light: PointLight, direction_to_light: vec3<f32>) -> f32 {
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
    let is_area_light = light.emitter.w >= 0.5;
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

fn weather_material_gloss_response() -> f32 {
    let material_kind = (material.header.w >> 8u) & 31u;
    if (material_kind == 5u || material_kind == 6u || material_kind == 14u
        || material_kind == 19u || material_kind == 20u || material_kind == 21u
        || material_kind == 22u || material_kind == 27u) { return 0.28; }
    if (material_kind == 7u || material_kind == 8u || material_kind == 9u || material_kind == 17u) { return 0.46; }
    if (material_kind == 3u || material_kind == 4u || material_kind == 10u
        || material_kind == 12u || material_kind == 15u || material_kind == 18u
        || material_kind == 25u || material_kind == 26u || material_kind == 29u
        || material_kind == 30u || material_kind == 31u) { return 1.0; }
    return 0.78;
}

struct LegacyDynamicLighting {
    diffuse_factor: vec3<f32>,
    wet_specular: vec3<f32>,
};

fn legacy_dynamic_light(input: VertexOut) -> LegacyDynamicLighting {
    var result: LegacyDynamicLighting;
    result.diffuse_factor = vec3<f32>(0.0);
    result.wet_specular = vec3<f32>(0.0);
    if (!ENABLE_LEGACY_DLIGHTS || camera.render_flags.x != 0u || lighting_settings.values.y == 0u) {
        return result;
    }

    // JKA-style triangle-plane response with a smooth radial blob. This keeps
    // authored runtime lights cheap and stable without the old 16x16 lookup or
    // an extra redraw pass.
    let dp_x = dpdx(input.world_position);
    let dp_y = dpdy(input.world_position);
    let plane_cross = cross(dp_x, dp_y);
    let plane_len_sq = dot(plane_cross, plane_cross);
    if (plane_len_sq < 1.0e-10) { return result; }
    var plane_normal = plane_cross * inverseSqrt(plane_len_sq);
    if (dot(plane_normal, input.world_normal) < 0.0) {
        plane_normal = -plane_normal;
    }

    let start = min(lighting_settings.local_shadows.w, lighting_settings.values.y);
    let end = min(lighting_settings.values.y, start + 32u);
    for (var i = start; i < end; i += 1u) {
        let light = dynamic_lights[i];
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
        result.diffuse_factor += light.color_intensity.rgb
            * light.color_intensity.a
            * plane_modulate
            * blob
            * 0.225;
    }
    return result;
}

struct WeatherSurfaceResponse {
    wetness: f32,
    puddle: f32,
};

fn weather_hash12(p: vec2<f32>) -> f32 {
    let p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    let q = p3 + vec3<f32>(dot(p3, p3.yzx + vec3<f32>(33.33)));
    return fract((q.x + q.y) * q.z);
}

fn weather_puddle_noise(world_xz: vec2<f32>) -> f32 {
    let p = world_xz / 170.0;
    let cell = floor(p);
    let f = fract(p);
    let u = f * f * (vec2<f32>(3.0) - 2.0 * f);
    let n00 = weather_hash12(cell);
    let n10 = weather_hash12(cell + vec2<f32>(1.0, 0.0));
    let n01 = weather_hash12(cell + vec2<f32>(0.0, 1.0));
    let n11 = weather_hash12(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(n00, n10, u.x), mix(n01, n11, u.x), u.y);
}

fn weather_puddle_mask(world_xz: vec2<f32>, accumulation: f32, depression: f32) -> f32 {
    // Puddles are now selected by the cached local-depression field, not random
    // world-space noise. Noise only breaks up the edge of a real basin.
    let low = mix(0.46, 0.18, accumulation);
    let high = mix(0.72, 0.34, accumulation);
    let basin = smoothstep(low, high, depression);
    let edge_breakup = mix(0.82, 1.0, weather_puddle_noise(world_xz));
    return clamp(basin * edge_breakup, 0.0, 1.0);
}

fn weather_hash22(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        weather_hash12(p + vec2<f32>(17.17, 3.11)),
        weather_hash12(p + vec2<f32>(5.73, 41.91))
    );
}

fn weather_ripple_layer(
    world_xz: vec2<f32>,
    time_seconds: f32,
    uv_offset: vec2<f32>,
    phase_offset: f32,
    cell_size: f32,
) -> vec2<f32> {
    // Procedural equivalent of the classic four shifted rain-ripple texture
    // layers: each world-space cell owns a stable impact center and phase, then
    // repeatedly emits an expanding capillary ring. No simulation texture is
    // required and the pattern is stable as the camera moves.
    let uv = world_xz / cell_size + uv_offset;
    let cell = floor(uv);
    let local = fract(uv);
    let rnd = weather_hash22(cell + uv_offset * 19.0);
    let centre = vec2<f32>(0.14) + rnd * 0.72;
    let delta = local - centre;
    let dist = max(length(delta), 0.001);
    let age = fract(time_seconds * 0.52 + phase_offset
        + weather_hash12(cell + uv_offset * 31.0));
    let radius = mix(0.025, 0.78, age);
    let signed_distance = dist - radius;
    let band = 1.0 - smoothstep(0.018, 0.095, abs(signed_distance));
    let birth = smoothstep(0.0, 0.06, age);
    let death = 1.0 - smoothstep(0.72, 1.0, age);
    let slope_sign = select(-1.0, 1.0, signed_distance >= 0.0);
    return (delta / dist) * band * birth * death * slope_sign;
}

fn weather_ripple_gradient(
    world_xz: vec2<f32>,
    time_seconds: f32,
    rain_strength: f32,
) -> vec2<f32> {
    // Faithful to the common four-layer rain-ripple structure: rain intensity
    // progressively enables 1..4 offset layers at quarter-strength steps. Heavy
    // rain therefore has a persistent overlapping field rather than one lonely
    // disappearing ring.
    let weights = clamp(
        (vec4<f32>(rain_strength) - vec4<f32>(0.0, 0.25, 0.50, 0.75)) * 4.0,
        vec4<f32>(0.0),
        vec4<f32>(1.0)
    );
    let r1 = weather_ripple_layer(world_xz, time_seconds, vec2<f32>( 0.25,  0.00), 0.00, 64.0);
    let r2 = weather_ripple_layer(world_xz, time_seconds, vec2<f32>(-0.55,  0.30), 0.31, 67.0);
    let r3 = weather_ripple_layer(world_xz, time_seconds, vec2<f32>( 0.60,  0.85), 0.57, 61.0);
    let r4 = weather_ripple_layer(world_xz, time_seconds, vec2<f32>( 0.50, -0.75), 0.79, 70.0);

    // Very small continuous capillary motion keeps a mature puddle alive between
    // individual rings. This is deliberately much lower amplitude than the drops.
    let capillary = vec2<f32>(
        sin(world_xz.x * 0.092 + world_xz.y * 0.037 + time_seconds * 2.7)
            + 0.55 * sin(world_xz.y * 0.121 - time_seconds * 3.2),
        cos(world_xz.y * 0.086 - world_xz.x * 0.031 - time_seconds * 2.9)
            + 0.50 * cos(world_xz.x * 0.115 + time_seconds * 3.5)
    ) * (0.11 * rain_strength);

    return r1 * weights.x + r2 * weights.y + r3 * weights.z + r4 * weights.w + capillary;
}

fn weather_surface_response(input: VertexOut) -> WeatherSurfaceResponse {
    var result: WeatherSurfaceResponse;
    result.wetness = 0.0;
    result.puddle = 0.0;
    if ((material.header.z & 4u) == 0u
        || (weather_surface.amount_distance.x <= 0.001 && weather_surface.puddle.x <= 0.001)) {
        return result;
    }

    let delta = input.world_position - camera.camera_pos_time.xyz;
    let distance_to_surface = length(delta);
    let fade_end = max(weather_surface.amount_distance.z, 1.0);
    let distance_weight = 1.0 - smoothstep(weather_surface.amount_distance.y, fade_end, distance_to_surface);

    var exposure = 1.0;
    // Fallback only matters before the cached weather field exists. Once active,
    // physical_upness comes from the actual rain-facing BSP plane stored at map
    // preparation/first weather enable, not the authored/interpolated shader normal.
    var physical_upness = 0.0;
    var local_depression = 0.0;
    if (weather_surface.occlusion_size.z != 0u) {
        let uv = (input.world_position.xz - weather_surface.occlusion_uv.xy) * weather_surface.occlusion_uv.zw;
        if (all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0))) {
            let dims = weather_surface.occlusion_size.xy;
            let texel_position = uv * vec2<f32>(f32(dims.x), f32(dims.y)) - vec2<f32>(0.5);
            let blend = fract(texel_position);
            let covers = textureGather(0, weather_occlusion_height, weather_occlusion_sampler, uv);
            let surface_heights = textureGather(1, weather_occlusion_height, weather_occlusion_sampler, uv);
            let upness = textureGather(2, weather_occlusion_height, weather_occlusion_sampler, uv);
            let basins = textureGather(3, weather_occlusion_height, weather_occlusion_sampler, uv);
            let e00 = weather_exposure_from_cover(covers.w, input.world_position.y);
            let e10 = weather_exposure_from_cover(covers.z, input.world_position.y);
            let e01 = weather_exposure_from_cover(covers.x, input.world_position.y);
            let e11 = weather_exposure_from_cover(covers.y, input.world_position.y);
            exposure = mix(mix(e00, e10, blend.x), mix(e01, e11, blend.x), blend.y);

            // Match the rendered fragment to the cached physical top surface. This
            // prevents a roof's topography from being reused by a lower floor at
            // the same X/Z, while still giving soft texel transitions.
            let s00 = 1.0 - smoothstep(6.0, 20.0, abs(surface_heights.w - input.world_position.y));
            let s10 = 1.0 - smoothstep(6.0, 20.0, abs(surface_heights.z - input.world_position.y));
            let s01 = 1.0 - smoothstep(6.0, 20.0, abs(surface_heights.x - input.world_position.y));
            let s11 = 1.0 - smoothstep(6.0, 20.0, abs(surface_heights.y - input.world_position.y));
            physical_upness = mix(
                mix(upness.w * s00, upness.z * s10, blend.x),
                mix(upness.x * s01, upness.y * s11, blend.x),
                blend.y
            );
            local_depression = mix(
                mix(basins.w * s00, basins.z * s10, blend.x),
                mix(basins.x * s01, basins.y * s11, blend.x),
                blend.y
            );
        }
    }

    let geometric_normal = normalize(input.world_normal);
    let orientation = smoothstep(-0.45, 0.12, geometric_normal.y);
    let film = clamp(weather_surface.amount_distance.x * weather_surface.amount_distance.w
        * distance_weight * exposure * orientation, 0.0, 1.0);

    var puddle = 0.0;
    if (weather_surface.puddle.x > 0.001 && exposure > 0.001
        && physical_upness > 0.70 && local_depression > 0.001) {
        let flatness = smoothstep(0.70, 0.985, physical_upness);
        let accumulation = clamp(weather_surface.puddle.x, 0.0, 1.0);
        let basin = weather_puddle_mask(input.world_position.xz, accumulation, local_depression);
        // Puddle placement is persistent world state: no camera-distance weight.
        // Walking toward or away from a basin must never create/remove it.
        puddle = clamp((0.28 + accumulation * 1.04) * exposure * flatness * basin, 0.0, 1.0);
    }

    result.puddle = puddle;
    // Standing water remains a wet surface even after the thin-film scalar has
    // mostly dried, which lets puddles linger naturally after rain stops.
    result.wetness = max(film, puddle * 0.96);
    return result;
}

fn weather_wet_roughness(dry_roughness: f32, wetness: f32, puddle: f32) -> f32 {
    let gloss_amount = pow(clamp(wetness, 0.0, 1.0), 1.35);
    let film_roughness = mix(dry_roughness, max(0.12, dry_roughness * 0.28),
        gloss_amount * weather_material_gloss_response());
    // Source-repo behavior: inside the wet mask the smoothness buffer is
    // overridden toward a true water-film response instead of merely nudging
    // the original material roughness.
    return clamp(mix(film_roughness, 0.012, smoothstep(0.05, 0.80, puddle)), 0.01, 1.0);
}

fn weather_reflection_normal(base_normal: vec3<f32>, world_position: vec3<f32>, puddle: f32) -> vec3<f32> {
    var n = normalize(base_normal);
    if (puddle <= 0.001) { return n; }

    // A puddle is a new water surface laid over the material. The source effect
    // achieves this by overriding the normal buffer; do the forward-renderer
    // equivalent here by flattening the reflection normal toward the water plane.
    // Puddle eligibility was already decided from the cached physical BSP plane.
    // Once water exists, it is a new horizontal film and must not inherit a
    // misleading authored/smoothed material normal.
    let water_mask = smoothstep(0.05, 0.85, puddle);
    n = normalize(mix(n, vec3<f32>(0.0, 1.0, 0.0), water_mask * 0.985));

    let ripple_strength = clamp(weather_surface.puddle.z, 0.0, 1.0) * water_mask;
    if (ripple_strength > 0.001) {
        let ripple = weather_ripple_gradient(
            world_position.xz,
            weather_surface.puddle.y,
            clamp(weather_surface.puddle.z, 0.0, 1.0)
        );
        let water_normal = normalize(vec3<f32>(-ripple.x * 0.116, 1.0, -ripple.y * 0.116));
        n = normalize(mix(n, water_normal, ripple_strength * 0.82));
    }
    return n;
}

fn weather_surface_coat(input: VertexOut, wetness: f32, puddle: f32) -> vec3<f32> {
    if (lighting_settings.feature_flags.w < 1u || (wetness <= 0.001 && puddle <= 0.001)) { return vec3<f32>(0.0); }
    let n = weather_reflection_normal(input.world_normal, input.world_position, puddle);
    let v = normalize(camera.camera_pos_time.xyz - input.world_position);
    let reflection = normalize(reflect(-v, n));
    let jka_reflection = normalize(vec3<f32>(reflection.x, -reflection.z, reflection.y));
    let wet_lod = mix(0.25, 0.12, pow(clamp(wetness, 0.0, 1.0), 1.25));
    let lod_fraction = mix(wet_lod, 0.0, smoothstep(0.05, 0.80, puddle));
    let environment = textureSampleLevel(reflection_probe_texture, reflection_probe_sampler,
        jka_reflection, lod_fraction * f32(max(textureNumLevels(reflection_probe_texture), 1u) - 1u)).rgb;
    let fresnel = 0.025 + 0.975 * pow(1.0 - max(dot(n, v), 0.0), 5.0);
    let coat_amount = max(pow(clamp(wetness, 0.0, 1.0), 1.45), smoothstep(0.02, 0.75, puddle));
    let film_response = 0.34 * weather_material_gloss_response();
    let water_response = 0.92;
    return environment * fresnel * coat_amount * mix(film_response, water_response, puddle);
}

fn clustered_dynamic_light_legacy(input: VertexOut, wetness: f32, puddle: f32) -> LegacyDynamicLighting {
    var result: LegacyDynamicLighting;
    result.diffuse_factor = vec3<f32>(0.0);
    result.wet_specular = vec3<f32>(0.0);
    if (camera.render_flags.x != 0u || !(ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS || ENABLE_AREA_LIGHTS) || lighting_settings.values.y == 0u) {
        return result;
    }
    let cluster = cluster_for_fragment(input);
    let normal = normalize(input.world_normal);
    let specular_normal = weather_reflection_normal(normal, input.world_position, puddle);
    let view_direction = normalize(camera.camera_pos_time.xyz - input.world_position);
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
        let light_direction = delta / distance_to_light;
        let ndotl = max(dot(normal, light_direction), 0.0);
        let emission_visibility = emitter_visibility(light, light_direction);
        let shadow_visibility = local_light_shadow_visibility(light, input.world_position, normal);
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
            result.diffuse_factor += energy * ndotl * local_light_surface_scale(light);
        }
        if ((wetness > 0.001 || puddle > 0.001) && ndotl > 0.0) {
            let half_vector = normalize(light_direction + view_direction);
            let spec_amount = max(pow(clamp(wetness, 0.0, 1.0), 1.35), puddle * 0.92);
            result.wet_specular += energy
                * pow(max(dot(specular_normal, half_vector), 0.0), mix(30.0, 140.0, max(wetness, puddle)))
                * spec_amount * mix(weather_material_gloss_response(), 1.0, puddle)
                * mix(0.18, 0.48, puddle);
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
    return f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
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

    let packed_rmo = (material.header.z & 512u) != 0u;
    if (packed_rmo && (material.header.z & 16u) != 0u) {
        // rmoMap is R=roughness, G=metalness, B=ambient occlusion. Both
        // roughness_texture and metallic_texture point at the same packed image,
        // so sample it once instead of doing two identical fetches.
        let rmo = companion_sample_roughness(uv);
        result.roughness = rmo.r;
        result.metallic = rmo.g;
        result.ao = rmo.b;
        if ((material.header.z & 262144u) != 0u) {
            result.specular_scale = rmo.a;
        }
    } else {
        if ((material.header.z & 16u) != 0u) {
            result.roughness = companion_sample_roughness(uv).r;
        }
        if ((material.header.z & 64u) != 0u) {
            result.metallic = companion_sample_metallic(uv).r;
        }
    }

    if ((material.header.z & 128u) != 0u) {
        let specular = companion_sample_specular(uv);
        result.dielectric_f0 = clamp(specular.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        // Rend2's gloss workflow stores glossiness in specular alpha. Only use
        // it when no roughness/RMO map is present; authored roughness wins.
        if ((material.header.z & 16u) == 0u) {
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
    result.specular_n = weather_reflection_normal(geometric, input.world_position, weather.puddle);
    result.v = normalize(camera.camera_pos_time.xyz - input.world_position);
    result.roughness = weather_wet_roughness(0.65, weather.wetness, weather.puddle);
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
    result.specular_n = weather_reflection_normal(result.n, input.world_position, weather.puddle);
    let pbr = sample_pbr_material(uv);
    result.roughness = weather_wet_roughness(pbr.roughness, weather.wetness, weather.puddle);
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

fn weather_exposure_from_cover(cover_y: f32, surface_y: f32) -> f32 {
    if (cover_y <= -1.0e19) { return 1.0; }
    return 1.0 - smoothstep(6.0, 24.0, cover_y - surface_y);
}

fn reflection_probe_specular(
    input: VertexOut,
    albedo: vec3<f32>,
    uv: vec2<f32>,
    weather: WeatherSurfaceResponse,
    shared_frame: mat3x3<f32>,
    shared_surface: PbrSurface,
) -> vec3<f32> {
    if (!ENABLE_PBR || lighting_settings.feature_flags.w < 1u || (material.header.z & 2048u) == 0u) {
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
    if (!ENABLE_PBR || pbr_settings.deluxe.x < 0.5) {
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
        let recovered_direct = lightmap_color / max(geometric_ndotl, 0.25);
        let recovered_ambient = max(
            lightmap_color - recovered_direct * geometric_ndotl,
            vec3<f32>(0.0)
        );
        let remapped = recovered_direct * mapped_ndotl + recovered_ambient;
        if ((material.header.z & 8u) != 0u) {
            let factor = clamp(
                remapped / max(lightmap_color, vec3<f32>(1e-4)),
                vec3<f32>(0.25),
                vec3<f32>(1.85)
            );
            result.diffuse_factor *= factor;
        }
        let light_luma = max(dot(lightmap_color, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-4);
        ambient_fraction = clamp(
            dot(recovered_ambient, vec3<f32>(0.2126, 0.7152, 0.0722)) / light_luma,
            0.0,
            1.0
        );
        specular_radiance = recovered_direct;
    } else {
        if ((material.header.z & 8u) != 0u) {
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
    if (deluxe_specular_scale > 0.0) {
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
            let g = geometry_smith(n, v, l, deluxe_roughness);
            let f = fresnel_schlick(max(dot(h, v), 0.0), surface.f0);
            let denominator = max(4.0 * ndotv * ndotl, 1e-4);
            let specular = (ndf * g / denominator) * f;
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
        if ((material.header.z & (16u | 64u)) != 0u) {
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
    if (camera.render_flags.x != 0u || !(ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS || ENABLE_AREA_LIGHTS) || lighting_settings.values.y == 0u) {
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
        let shadow_visibility = local_light_shadow_visibility(light, input.world_position, n);
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
        illumination += diffuse * radiance * ndotl + specular * radiance * spec_ndotl;
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
    // Legacy 2 is emitted as one post-material EXP2 geometry pass for all
    // authored BSP fog in DinurdoJK. Mixing an in-stage path with a fallback
    // path produced visibly different fog strengths between materials even at
    // the same depth. The separate pass preserves one equation/order for every
    // fogged BSP surface while still leaving dynamic entities untouched.
    if (drawfog_mode >= 1.5 && map_has_authored_fog && has_map_fog) {
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
    // routes every authored fogged BSP surface through this geometry pass so
    // all materials share the same EXP2 equation and ordering.
    let legacy1_geometry = drawfog_mode < 1.5
        && (surface_fog.flags.x < 0.5 || surface_fog.flags.w < 0.5);
    let legacy2_geometry = drawfog_mode >= 1.5
        && map_has_authored_fog
        && has_map_fog;
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
        textureSample(ocean_normal, ocean_sampler, vec2<f32>(h.y, h.w), layer),
        textureSample(ocean_normal, ocean_sampler, vec2<f32>(h.x, h.w), layer),
        w.x
    );
    let top = mix(
        textureSample(ocean_normal, ocean_sampler, vec2<f32>(h.y, h.z), layer),
        textureSample(ocean_normal, ocean_sampler, vec2<f32>(h.x, h.z), layer),
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

// Map-sky sampling for water environment lighting. The CPU binds a representative
// authored skybox to every surface that may render promoted ocean, so water does
// not fall back to a hardcoded blue environment.
fn ocean_sky_uv(s: f32, t: f32) -> vec2<f32> {
    return clamp(vec2<f32>((s + 1.0) * 0.5, (1.0 - t) * 0.5), vec2<f32>(0.001), vec2<f32>(0.999));
}

fn sample_map_sky_renderer(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    // Convert renderer [x,z,-y] back to JKA [x,y,z].
    let d = normalize(vec3<f32>(direction.x, -direction.z, direction.y));
    let a = abs(d);
    // Sky textures carry their generated mip chain, so roughness can use it as
    // the cheap prefiltered-environment approximation this renderer already uses.
    let levels = max(textureNumLevels(sky_rt), 1u);
    let lod = clamp(roughness, 0.0, 1.0) * f32(levels - 1u);
    if (a.x >= a.y && a.x >= a.z) {
        if (d.x >= 0.0) {
            let m = a.x;
            return textureSampleLevel(sky_rt, sky_sampler, ocean_sky_uv(-d.y / m, d.z / m), lod).rgb;
        }
        let m = a.x;
        return textureSampleLevel(sky_lf, sky_sampler, ocean_sky_uv(d.y / m, d.z / m), lod).rgb;
    }
    if (a.y >= a.x && a.y >= a.z) {
        if (d.y >= 0.0) {
            let m = a.y;
            return textureSampleLevel(sky_bk, sky_sampler, ocean_sky_uv(d.x / m, d.z / m), lod).rgb;
        }
        let m = a.y;
        return textureSampleLevel(sky_ft, sky_sampler, ocean_sky_uv(-d.x / m, d.z / m), lod).rgb;
    }
    if (d.z >= 0.0) {
        let m = a.z;
        return textureSampleLevel(sky_up, sky_sampler, ocean_sky_uv(-d.y / m, -d.x / m), lod).rgb;
    }
    let m = a.z;
    return textureSampleLevel(sky_dn, sky_sampler, ocean_sky_uv(-d.y / m, d.x / m), lod).rgb;
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
    if (ENABLE_PBR && lighting_settings.feature_flags.w >= 1u && (material.header.z & 2048u) != 0u) {
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
    if (camera.render_flags.x != 0u || !(ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS || ENABLE_AREA_LIGHTS) || lighting_settings.values.y == 0u) {
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
                * local_light_shadow_visibility(light, input.world_position, n);
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
            * local_light_shadow_visibility(light, input.world_position, n)
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

fn ocean_surface_sample(input: VertexOut, below: bool) -> vec4<f32> {
    let units_per_meter = max(ocean.ocean_info.z, 0.001);
    let world_m = input.ocean_uv_m;
    let dist = distance(input.world_position.xz, camera.camera_pos_time.xz) / units_per_meter;
    let map_size = ocean.ocean_info.y;

    // Upstream sums all cascade gradients AND foam. Do not max the foam: the
    // original shader deliberately blends accumulated Jacobian foam.
    var gradient_foam = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let scale = ocean.map_scales[i];
        let coords = vec3<f32>(world_m * vec2<f32>(1.0,-1.0) * scale.xy, f32(i));
        let ppm = map_size * min(scale.x, scale.y);
        let bicubic = ocean_texture_bicubic(coords);
        let bilinear = textureSample(ocean_normal, ocean_sampler, coords.xy, i32(i));
        let sample = mix(bicubic, bilinear, min(1.0, ppm*0.1));
        gradient_foam += vec3<f32>(sample.x * scale.w, -sample.y * scale.w, sample.w);
    }

    let phase = dot(world_m, ocean.swell.xy) * ocean.swell.z - ocean.swell_motion.x * ocean.swell_motion.w;
    gradient_foam = vec3<f32>(gradient_foam.xy + ocean.swell.xy * ocean.swell.w * ocean.swell.z * cos(phase), gradient_foam.z);
    let breaker_foam = smoothstep(0.0, 1.0, gradient_foam.z*0.75) * exp(-dist*0.0075);
    var windrow_foam = 0.0;
    if (ocean.surface.w > 0.5) {
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
        let a = ocean_windrow[offset + y0*windrow_size + x0];
        let b = ocean_windrow[offset + y0*windrow_size + x1];
        let c = ocean_windrow[offset + y1*windrow_size + x0];
        let d = ocean_windrow[offset + y1*windrow_size + x1];
        let transported = mix(mix(a,b,f.x), mix(c,d,f.x), f.y);
        // Keep thin windrows visible farther than crest foam while still fading
        // sub-texel structure before it reaches the horizon.
        windrow_foam = smoothstep(0.08, 0.72, transported) * exp(-dist*0.0022);
    }
    let foam_factor = 1.0 - (1.0-breaker_foam) * (1.0-windrow_foam);
    let normal_fade = mix(0.015, ocean.surface.y, exp(-dist*0.0175));
    var n = normalize(vec3<f32>(-gradient_foam.x*normal_fade, 1.0, -gradient_foam.y*normal_fade));
    let view_dir = normalize(camera.camera_pos_time.xyz - input.world_position);
    n = select(n, -n, below);
    let dot_nv = max(dot(n, view_dir), 2e-5);
    let roughness = clamp(ocean.surface.x, 0.001, 1.0);
    let reflectance = 0.02;
    let fresnel_curve = pow(1.0 - dot_nv, 5.0*exp(-2.69*roughness)) /
        (1.0 + 22.7*pow(roughness, 1.5));
    var fresnel = mix(fresnel_curve, 1.0, reflectance);
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
    let sun_radiance = ocean.sun_color.rgb * ocean.sun_direction_intensity.w;
    let halfway = normalize(light_dir + view_dir);
    let dot_nl = max(dot(n, light_dir), 2e-5);
    let light_mask = ocean_smith_masking_shadowing(roughness, dot_nv);
    let view_mask = ocean_smith_masking_shadowing(roughness, dot_nl);
    let microfacet_distribution = ocean_ggx_distribution(max(dot(n, halfway), 0.0), roughness);
    let geometric_attenuation = 1.0 / (1.0 + light_mask + view_mask);
    var attenuation = 1.0;
    if (ENABLE_CASCADED_SHADOWS && shadow_settings.light_direction_enabled.w > 0.5) {
        attenuation = cascaded_shadow_visibility(input, n);
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
    let direct_diffuse = mix(water_diffuse, ocean.foam_color.rgb, foam_factor)
        * (1.0 - fresnel) * attenuation * sun_radiance;

    let albedo = mix(ocean.water_color.rgb, ocean.foam_color.rgb, foam_factor);
    let environment = ocean_environment(input, n, view_dir, ibl_roughness);
    let indirect = ocean_indirect_lighting(input, n, albedo);
    let local_lights = ocean_dynamic_lighting(input, n, view_dir, albedo, roughness);

    let dims = vec2<f32>(textureDimensions(ocean_scene));
    let uv = input.clip_position.xy / dims;
    let water_depth = dot(input.world_position - camera.camera_pos_time.xyz, camera.camera_forward.xyz);
    let undistorted_depth = textureLoad(ocean_scene_depth, vec2<i32>(input.clip_position.xy), 0).r;
    let thickness = max(undistorted_depth - water_depth, 0.0);
    let edge = smoothstep(0.0, 0.06, min(min(uv.x, uv.y), min(1.0-uv.x, 1.0-uv.y)));
    let tilt = n - vec3<f32>(0.0, select(1.0, -1.0, below), 0.0);
    let screen_normal = (camera.view_proj * vec4<f32>(tilt, 0.0)).xy * vec2<f32>(1.0,-1.0);
    let offset = clamp(screen_normal, vec2<f32>(-1.0), vec2<f32>(1.0)) * ocean_optics.params.y * edge * min(thickness / 64.0, 1.0);
    var refracted_uv = clamp(uv + offset, 0.5/dims, 1.0-0.5/dims);
    let sample_depth = textureLoad(ocean_scene_depth, vec2<i32>(refracted_uv*dims), 0).r;
    if (sample_depth <= water_depth + 1.0) { refracted_uv = uv; }
    // The snapshot already contains water-path absorption. Do not charge it twice.
    let transmission = textureSampleLevel(ocean_scene, ocean_scene_sampler, refracted_uv, 0.0).rgb;
    let surface_light = indirect + albedo * direct_diffuse + local_lights;
    var color = transmission * (1.0-fresnel) * (1.0-foam_factor)
        + surface_light * foam_factor + specular + environment * fresnel;
    if (below) {
        // Reflected/surface light travels from the interface to the eye through water.
        let ratios = vec3<f32>(38.0/7.5, 38.0/22.0, 1.0);
        let t = exp2(-4.321928 * ratios * length(input.world_position-camera.camera_pos_time.xyz) / max(ocean_optics.fog.w, 1.0));
        let depth = max(input.world_position.y-camera.camera_pos_time.y, 0.0);
        let ambient = ocean_optics.fog.rgb * exp2(-4.321928 * ratios * depth / max(ocean_optics.params.x, 1.0));
        color = transmission * (1.0-fresnel) * (1.0-foam_factor)
            + (surface_light * foam_factor + specular + environment * fresnel) * t
            + ambient * (1.0-t) * (fresnel + (1.0-fresnel)*foam_factor);
    }
    return vec4<f32>(color, 1.0);
}

fn shade_surface(input: VertexOut, front: bool) -> vec4<f32> {
    let classic_flags = camera.render_flags.z;
    let classic_fullbright = (classic_flags & CLASSIC_FULLBRIGHT) != 0u;
    let classic_vertex_light = (classic_flags & CLASSIC_VERTEX_LIGHT) != 0u;
    let classic_lightmap_only = (classic_flags & CLASSIC_LIGHTMAP_ONLY) != 0u;
    let explicit_lightmap_stage = (material.header.z & MATERIAL_EXPLICIT_LIGHTMAP) != 0u;
    let has_lightmap = (material.header.z & MATERIAL_HAS_LIGHTMAP) != 0u;
    let opaque_stage = (material.header.z & MATERIAL_OPAQUE_STAGE) != 0u;
    if (dot(camera.clip_plane.xyz, camera.clip_plane.xyz) > 0.5
        && dot(camera.clip_plane.xyz, input.world_position) + camera.clip_plane.w < 0.0) {
        discard;
    }
    if (input.ocean_vertex > 0.5) {
        // The ocean pipeline never uses the fs_main_legacy_fog entry, so this
        // is the only place Legacy fog reaches the GodotOcean surface. It is
        // compiled only into Legacy fog world variants.
        let ocean_color = ocean_surface_sample(input, !front);
        if (ENABLE_LEGACY_FOG) {
            return apply_ocean_legacy_fog(ocean_color, input.world_position);
        }
        return ocean_color;
    }
    var stage_color = material.color;
    if ((material.header.z & 1u) != 0u) {
        stage_color = vec4<f32>(stage_color.rgb * input.color.rgb, stage_color.a);
    } else if ((material.header.z & 524288u) != 0u) {
        stage_color = vec4<f32>(
            stage_color.rgb * (vec3<f32>(1.0) - input.color.rgb),
            stage_color.a,
        );
    }
    if ((material.header.z & 1048576u) != 0u) {
        stage_color.a = stage_color.a * input.color.a;
    } else if ((material.header.z & 2097152u) != 0u) {
        stage_color.a = stage_color.a * (1.0 - input.color.a);
    }

    let authored_planar = ENABLE_PLANAR_REFLECTIONS && (material.header.z & 4096u) != 0u;
    let tcgen_environment = material.header.x == 3u;
    let environment_mode = ENABLE_PLANAR_REFLECTIONS && planar_reflection.viewport.w > 0.5;
    // For promoted environment stages, the tcGen mode itself is authoritative.
    // The CPU-only candidate bit is still useful for discovery/budgeting, but
    // the active render representation (coarse vs FULL PVS sub-batches) must
    // not be able to make the replacement disappear.
    let promoted_environment = tcgen_environment && environment_mode;
    let planar_candidate = authored_planar || promoted_environment;
    let planar_debug_surface = (material.header.z & 16384u) != 0u || promoted_environment;
    let planar_debug_mode = u32(planar_reflection.debug.x + 0.5);
    let planar_slot = planar_slot_for_surface(input, promoted_environment);
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

    let has_pbr_brdf_map = (material.header.z & (8u | 16u | 64u | 128u)) != 0u;
    let static_pbr_path = has_pbr_brdf_map
        && classic_flags == 0u
        && (material.header.z & 2u) != 0u
        && input.lightmap_uv.x >= 0.0;
    let reflection_pbr_path = (material.header.z & 2048u) != 0u;
    let gi_pbr_path = ENABLE_VOXEL_GI && (material.header.z & 4u) != 0u;
    // Bevy's diffuse-indirect priority is lightmap first, then irradiance volume.
    // JKA LIGHTMAP_BY_VERTEX is also baked diffuse lighting, so treat it as the
    // same higher-priority class instead of double-lighting compiled static MD3s.
    let irradiance_pbr_path = ENABLE_IRRADIANCE_VOLUME
        && (material.header.z & 4u) != 0u
        && input.lightmap_uv.x < 0.0
        && (material.header.z & 131072u) == 0u;
    let dynamic_pbr_path = has_pbr_brdf_map
        && (material.header.z & 4u) != 0u
        && (ENABLE_POINT_LIGHTS || ENABLE_CLUSTERED_LITE_DLIGHTS || ENABLE_AREA_LIGHTS);
    let needs_pbr_surface = ENABLE_PBR
        && (static_pbr_path || reflection_pbr_path || gi_pbr_path || irradiance_pbr_path || dynamic_pbr_path);

    var shared_frame = mat3x3<f32>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0)
    );
    let needs_shared_frame = (ENABLE_POM && (material.header.z & 32u) != 0u)
        || (needs_pbr_surface && (material.header.z & 8u) != 0u);
    if (PBR_SHARED_TANGENT_FRAME && needs_shared_frame) {
        shared_frame = cotangent_frame(input);
    }
    let surface_uv = parallax_uv(input, shared_frame);
    var source_sample = textureSample(base_texture, base_sampler, surface_uv);

    // Planar promotion is a *source substitution for this exact shader stage*.
    // For tcGen environment this removes the legacy sphere-map image/UVs and
    // substitutes the rendered planar scene. Later shader stages (base decal,
    // lightmap, etc.) remain ordered and blended exactly as authored.
    if (selected_plane && planar_candidate) {
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
            let blended_environment = (material.header.z & 32768u) != 0u;
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
        && (material.header.z & 2u) == 0u) {
        if (opaque_stage) {
            source_sample = vec4<f32>(1.0);
            stage_color = vec4<f32>(1.0);
        } else {
            discard;
        }
    }

    var base = source_sample * stage_color;
    if (material.params.x > 0.0 && base.a < material.params.x) {
        discard;
    }
    let weather = weather_surface_response(input);
    let wetness = weather.wetness;
    let puddle = weather.puddle;
    if (weather_surface.puddle.w > 0.5) {
        let film_only = clamp(wetness - puddle * 0.65, 0.0, 1.0);
        let dry = vec3<f32>(0.05, 0.05, 0.05);
        let wet_film = vec3<f32>(0.12, 0.48, 0.14);
        let puddle_debug = vec3<f32>(0.05, 0.72, 1.0);
        return vec4<f32>(mix(mix(dry, wet_film, film_only), puddle_debug, puddle), 1.0);
    }
    if (wetness > 0.001) {
        let dry_color = base.rgb;
        let luma = dot(dry_color, vec3<f32>(0.2126, 0.7152, 0.0722));
        var wet_color = max(mix(vec3<f32>(luma), dry_color, 1.08), vec3<f32>(0.0)) * 0.82;
        wet_color *= mix(1.0, 0.82, puddle);
        base = vec4<f32>(mix(dry_color, wet_color, wetness), base.a);
    }
    let albedo = base.rgb;
    let source_map_unbaked_receiver = ENABLE_SOURCE_MAP_WORLD
        && (material.header.z & 4u) != 0u
        && input.lightmap_uv.x < 0.0
        && (material.header.z & 131072u) == 0u;
    if (ENABLE_MAP_LIGHT_SIMULATION
        && (material.header.z & 4u) != 0u
        && input.lightmap_uv.x < 0.0
        && (material.header.z & 131072u) == 0u) {
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
    if ((material.header.z & 2u) != 0u && input.lightmap_uv.x >= 0.0) {
        var lightmap_color = textureSample(lightmap_texture, lightmap_sampler, input.lightmap_uv).rgb;
        if (classic_fullbright) {
            // Vanilla r_fullbright keeps the diffuse texture but replaces the
            // baked lightmap contribution with white.
            lightmap_color = vec3<f32>(1.0);
        } else if (classic_vertex_light) {
            // r_vertexLight substitutes the BSP vertex color for baked lightmap
            // sampling. r_lightmap + r_vertexLight is the vanilla white debug case.
            lightmap_color = select(input.color.rgb, vec3<f32>(1.0), classic_lightmap_only);
        }
        if (classic_lightmap_only) {
            // r_lightmap removes the diffuse texture so the lighting data itself
            // is visible (the familiar white-walls/lightmap debug presentation).
            base = vec4<f32>(lightmap_color, base.a);
        } else {
            base = vec4<f32>(base.rgb * lightmap_color, base.a);
        }
        if (ENABLE_PBR && has_pbr_brdf_map && classic_flags == 0u) {
            let baked = static_baked_pbr(input, albedo, surface_uv, lightmap_color, weather, shared_frame, shared_pbr);
            base = vec4<f32>(base.rgb * baked.diffuse_factor + baked.specular, base.a);
        }
    }

    // LIGHTMAP_BY_VERTEX surfaces have no lightmap texture to expose. Preserve
    // their compiled vertex lighting for r_lightmap; vanilla r_vertexLight +
    // r_lightmap deliberately resolves to white instead.
    if (classic_lightmap_only && (material.header.z & 131072u) != 0u && !has_lightmap) {
        base = vec4<f32>(select(input.color.rgb, vec3<f32>(1.0), classic_vertex_light), base.a);
    }

    // Cached static BSP AO only modulates the normal baked-light contribution.
    // Classic debug/compatibility modes should show their source lighting rather
    // than a second modern AO modulation.
    if (camera.render_flags.y != 0u && classic_flags == 0u
        && (material.header.x == 1u || (material.header.z & 2u) != 0u || (material.header.z & 131072u) != 0u)) {
        let static_ao = clamp(input.sky_dir_ao.w, 0.0, 1.0);
        base = vec4<f32>(base.rgb * static_ao, base.a);
    }
    if (ENABLE_PBR && (material.header.z & 2048u) != 0u) {
        base = vec4<f32>(base.rgb + reflection_probe_specular(input, albedo, surface_uv, weather, shared_frame, shared_pbr), base.a);
    }
    if ((material.header.z & 4u) != 0u) {
        base = vec4<f32>(base.rgb + voxel_probe_indirect(input, albedo, surface_uv, weather, shared_frame, shared_pbr), base.a);
        let shadow_visibility = cascaded_shadow_visibility(input, input.world_normal);
        let light_direction = normalize(shadow_settings.light_direction_enabled.xyz);
        let sun_facing = max(dot(normalize(input.world_normal), -light_direction), 0.0);
        let shadow_strength = shadow_settings.params.z * sun_facing * (1.0 - shadow_visibility);
        base = vec4<f32>(base.rgb * (1.0 - shadow_strength), base.a);
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
        if (ENABLE_CLUSTERED_LITE_DLIGHTS) {
            let clustered_lite = clustered_dynamic_light_legacy(input, wetness, puddle);
            base = vec4<f32>(base.rgb + albedo * clustered_lite.diffuse_factor + clustered_lite.wet_specular, base.a);
        } else if (ENABLE_PBR && has_pbr_brdf_map) {
            base = vec4<f32>(base.rgb + clustered_dynamic_light_pbr(input, albedo, surface_uv, weather, shared_frame, shared_pbr), base.a);
        } else {
            let legacy_lighting = clustered_dynamic_light_legacy(input, wetness, puddle);
            base = vec4<f32>(base.rgb + albedo * legacy_lighting.diffuse_factor + legacy_lighting.wet_specular, base.a);
        }
    }
    if (ENABLE_MAP_LIGHT_SIMULATION
        && (material.header.z & 4u) != 0u
        && input.lightmap_uv.x < 0.0
        && (material.header.z & 131072u) == 0u) {
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
    if (wetness > 0.001 || puddle > 0.001) {
        base = vec4<f32>(base.rgb + weather_surface_coat(input, wetness, puddle), base.a);
    }
    if (ENABLE_PBR && (material.header.z & 256u) != 0u) {
        // Emissive companions are authored as color data (sRGB texture format,
        // sampled here in linear space). They intentionally bypass scene light
        // attenuation, while bloom/HDR can consume the resulting bright color.
        base = vec4<f32>(base.rgb + textureSample(emissive_texture, base_sampler, surface_uv).rgb, base.a);
    }
    return shade_surface_deformation(
        input.world_position,
        input.world_normal,
        material.header.w,
        material.header.x,
        input.shell_kind,
        input.shell_coverage,
        base
    );
}

@fragment fn fs_main(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (surface_deformation_should_discard(
        input.world_position, material.header.w, input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy,
        camera.render_flags.x == 0u
    )) {
        discard;
    }
    return shade_surface(input, front);
}

@fragment fn fs_main_legacy_fog(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (surface_deformation_should_discard(
        input.world_position, material.header.w, input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy,
        camera.render_flags.x == 0u
    )) {
        discard;
    }
    return apply_legacy_fog(shade_surface(input, front), input.world_position);
}

@fragment fn fs_legacy_fog_pass(input: VertexOut, @builtin(front_facing) _front: bool) -> @location(0) vec4<f32> {
    if (surface_deformation_should_discard(
        input.world_position, material.header.w, input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy,
        camera.render_flags.x == 0u
    )) {
        discard;
    }
    return legacy_separate_fog(input.world_position);
}

fn sky_uv(s: f32, t: f32) -> vec2<f32> {
    // Quake 3 MakeSkyVec maps [-1,1] to [0,1] and flips T. Keep a tiny
    // inset to avoid bilinear/trilinear sampling across a face edge.
    return clamp(vec2<f32>((s + 1.0) * 0.5, (1.0 - t) * 0.5), vec2<f32>(0.001), vec2<f32>(0.999));
}

@fragment fn fs_sky(input: VertexOut) -> @location(0) vec4<f32> {
    if ((material.header.w & 1u) == 0u) {
        // skyParms "-" has no outerbox. OpenJK draws no skybox here, so leave
        // the scene clear color visible (global-fog color when the BSP has one).
        discard;
    }

    // Convert the renderer's [x,z,-y] direction back to JKA coordinates.
    let d = normalize(vec3<f32>(input.sky_dir_ao.x, -input.sky_dir_ao.z, input.sky_dir_ao.y));
    let a = abs(d);
    if (a.x >= a.y && a.x >= a.z) {
        if (d.x >= 0.0) {
            let m = a.x;
            return textureSample(sky_rt, sky_sampler, sky_uv(-d.y / m, d.z / m));
        }
        let m = a.x;
        return textureSample(sky_lf, sky_sampler, sky_uv(d.y / m, d.z / m));
    }
    if (a.y >= a.x && a.y >= a.z) {
        if (d.y >= 0.0) {
            let m = a.y;
            return textureSample(sky_bk, sky_sampler, sky_uv(d.x / m, d.z / m));
        }
        let m = a.y;
        return textureSample(sky_ft, sky_sampler, sky_uv(-d.x / m, d.z / m));
    }
    if (d.z >= 0.0) {
        let m = a.z;
        return textureSample(sky_up, sky_sampler, sky_uv(-d.y / m, -d.x / m));
    }
    let m = a.z;
    return textureSample(sky_dn, sky_sampler, sky_uv(-d.y / m, d.x / m));
}
