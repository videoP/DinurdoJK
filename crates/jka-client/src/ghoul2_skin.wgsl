struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

// Pipeline-specialized: false compiles the self-applied Legacy fog out entirely.
// Rebuilt only when FogSystem::legacy_self_fog toggles, never checked per frame.
override ENABLE_LEGACY_FOG: bool = false;
// OpenJK Dynamic Glow temporarily overrides the fog colour to black while
// replaying explicitly-authored `glow` stages. Pipeline-specialize that replay
// rather than changing material classification or blend behavior.
override FORCE_FOG_BLACK: bool = false;
// Only the spectator visible-player erase pipeline uses this. Move the depth
// test toward the eye by this many JKA world units without changing the
// rasterized silhouette (screen XY/W remain exactly as before).
override SPEC_ERASE_DEPTH_TOLERANCE: f32 = 0.0;
// A Source 1 ($bumpmap / $phong) material. False compiles the normal-map
// sample, the per-pixel tangent frame and all of Valve's phong out of the
// module, so every JKA model keeps exactly the cost it had. It must be false
// unless the surface's bind group carries a real normal map, and it is also
// read by vs_main, which hands the light grid to the fragment stage instead of
// folding it into the draw colour.
override ENABLE_SOURCE_MATERIAL: bool = false;

struct BoneMatrix {
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
};

struct SkinDraw {
    axis0: vec4<f32>,
    axis1: vec4<f32>,
    axis2: vec4<f32>,
    origin: vec4<f32>,
    color: vec4<f32>,
    params: vec4<u32>,
    classic_ambient: vec4<f32>,
    classic_directed: vec4<f32>,
    classic_direction: vec4<f32>,
    classic_sun_directed: vec4<f32>,
    classic_sun_direction: vec4<f32>,
    uv_xform: vec4<f32>,
    uv_cross: vec4<f32>,
    spec_light: vec4<f32>,
    spec_viewer: vec4<f32>,
    jiggle0: vec4<f32>,
    jiggle1: vec4<f32>,
    jiggle2: vec4<f32>,
    jiggle3: vec4<f32>,
    // Source 1 $phong: boost, exponent, $rimlightboost, $rimlightexponent.
    // A zero boost means the term is off.
    source_phong: vec4<f32>,
    // xyz: $phongfresnelranges, packed ((mid-min)*2, mid, (max-mid)*2) by the
    // material loader exactly as Valve's helper packs it. w bit 0: $halflambert.
    source_fresnel: vec4<f32>,
    water_bands0: vec4<f32>,
    water_bands1: vec4<f32>,
    water_base: vec4<f32>,
};
// q3 RB_CalcSpecularAlpha: (reflected light . viewer)^4, all in JKA world space.
fn specular_alpha(draw: SkinDraw, world: vec3<f32>, world_normal: vec3<f32>) -> f32 {
    if (draw.spec_light.w == 0.0) {
        return 1.0;
    }
    let normal = normalize(world_normal);
    let light_dir = normalize(draw.spec_light.xyz - world);
    let reflected = normal * (2.0 * dot(normal, light_dir)) - light_dir;
    let viewer = normalize(draw.spec_viewer.xyz - world);
    let l = dot(reflected, viewer);
    if (l < 0.0) {
        return 0.0;
    }
    let l2 = l * l;
    return min(l2 * l2, 1.0);
}


@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
// Source 1 `$bumpmap`, linear. A flat (128, 128, 255) 1x1 texture is bound
// wherever a surface has none, so this is always a legal read.
@group(1) @binding(3) var normal_texture: texture_2d<f32>;

struct EntityLightingSettings {
    values: vec4<u32>,
    // Self-applied Legacy fog: linear RGB, depthForOpaque.
    legacy_fog_color_depth: vec4<f32>,
    // x: 0 off, 1 authored global EXP2, 2 manual fog; y: strength scale.
    legacy_fog_params: vec4<f32>,
    // Cloud ground shadow: xyz = unit direction toward the sun (render space),
    // w = 1 while projected cloud shadows are active, else 0.
    cloud_shadow_sun: vec4<f32>,
    // Wet weather on entities (model_weather.wgsl). xyz = the sun's travel
    // direction in render space, w = relative sun intensity (250 = 1), matching
    // WeatherSurfaceUniform so a model and the ground agree on the light.
    weather_sun_direction: vec4<f32>,
    // rgb = linear sun colour, w = model wetness 0..1, already faded by rain
    // intensity and by the camera's own exposure to the rain.
    weather_sun_radiance: vec4<f32>,
};
@group(1) @binding(2) var<uniform> entity_lighting: EntityLightingSettings;
@group(2) @binding(0) var<storage, read> bones: array<BoneMatrix>;
@group(2) @binding(1) var<storage, read> skin_draws: array<SkinDraw>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bone_indices: vec4<u32>,
    @location(4) weights: vec4<f32>,
    @location(5) weight_count: u32,
    @location(6) jiggle_region: u32,
    @location(7) jiggle_weight: f32,
    @location(8) jiggle_coord: f32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world_position: vec3<f32>,
    // `instance_index` is a vertex-stage builtin, so the Source material path
    // gets its draw's row of `skin_draws` through this flat interpolant. One
    // 32-bit flat varying, which is why the per-pixel light does not instead
    // ride along as several interpolated vectors.
    @location(4) @interpolate(flat) draw_index: u32,
};

fn transform_point(bone: BoneMatrix, point: vec3<f32>) -> vec3<f32> {
    let p = vec4<f32>(point, 1.0);
    return vec3<f32>(dot(bone.row0, p), dot(bone.row1, p), dot(bone.row2, p));
}

fn transform_vector(bone: BoneMatrix, vector: vec3<f32>) -> vec3<f32> {
    let v = vec4<f32>(vector, 0.0);
    return vec3<f32>(dot(bone.row0, v), dot(bone.row1, v), dot(bone.row2, v));
}

fn skin_position(
    position: vec3<f32>,
    bone_indices: vec4<u32>,
    weights: vec4<f32>,
    weight_count: u32,
    bone_base: u32,
) -> vec3<f32> {
    let p0 = transform_point(bones[bone_base + bone_indices.x], position);
    if weight_count <= 1u {
        return p0;
    }

    let p1 = transform_point(bones[bone_base + bone_indices.y], position);
    if weight_count == 2u {
        // Match OpenJK/RB_SurfaceGhoul's optimized two-weight form exactly.
        return weights.x * (p0 - p1) + p1;
    }

    let p2 = transform_point(bones[bone_base + bone_indices.z], position);
    if weight_count == 3u {
        let residual = 1.0 - weights.x - weights.y;
        return weights.x * p0 + weights.y * p1 + residual * p2;
    }

    let p3 = transform_point(bones[bone_base + bone_indices.w], position);
    let residual = 1.0 - weights.x - weights.y - weights.z;
    return weights.x * p0
        + weights.y * p1
        + weights.z * p2
        + residual * p3;
}

fn jiggle_offset(draw: SkinDraw, region: u32) -> vec3<f32> {
    switch region {
        case 0u: { return draw.jiggle0.xyz; }
        case 1u: { return draw.jiggle1.xyz; }
        case 2u: { return draw.jiggle2.xyz; }
        case 3u: { return draw.jiggle3.xyz; }
        default: { return vec3<f32>(0.0); }
    }
}

fn jiggle_effective_weight(draw: SkinDraw, weight: f32, coord: f32) -> f32 {
    if (abs(coord) < 2.0) {
        let bottom = draw.jiggle0.w;
        let top = draw.jiggle1.w;
        let lower = smoothstep(-0.70 + bottom, -0.15 + bottom, coord);
        var upper = 1.0;
        if (top < 0.0) {
            upper = 1.0 - smoothstep(0.65 + top, 1.0 + top, coord);
        }
        return weight * lower * upper;
    }
    return weight;
}

fn model_to_world(draw: SkinDraw, model_position: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        draw.origin.x + draw.axis0.x * model_position.x + draw.axis1.x * model_position.y + draw.axis2.x * model_position.z,
        draw.origin.y + draw.axis0.y * model_position.x + draw.axis1.y * model_position.y + draw.axis2.y * model_position.z,
        draw.origin.z + draw.axis0.z * model_position.x + draw.axis1.z * model_position.y + draw.axis2.z * model_position.z,
    );
}

// Same JKA -> renderer conversion as scene::render_position([x,y,z]).
fn jka_to_render(world: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(world.x, world.z, -world.y);
}

@vertex
fn vs_main(input: VertexIn, @builtin(instance_index) draw_index: u32) -> VertexOut {
    let draw = skin_draws[draw_index];
    var model_position = skin_position(
        input.position,
        input.bone_indices,
        input.weights,
        input.weight_count,
        draw.params.x,
    );
    if (input.jiggle_weight > 0.0 && input.jiggle_region < 4u) {
        let jiggle_weight = jiggle_effective_weight(draw, input.jiggle_weight, input.jiggle_coord);
        model_position += jiggle_offset(draw, input.jiggle_region) * jiggle_weight;
    }

    // OpenJK's fast Ghoul2 path transforms the normal only by weight-0's bone.
    let model_normal = transform_vector(
        bones[draw.params.x + input.bone_indices.x],
        input.normal,
    );

    // `deformVertexes bulge <0> <height> <0>`: JKA's static-offset special
    // case (RB_CalcBulgeVertexes), packed into spec_viewer.w (see
    // ghoul2_specular_viewer). A constant push outward along the model-space
    // normal, applied before the entity transform, same as the engine.
    let bulge_height = draw.spec_viewer.w;
    if (bulge_height != 0.0) {
        model_position += normalize(model_normal) * bulge_height;
    }

    let world = model_to_world(draw, model_position);
    let world_normal = vec3<f32>(
        draw.axis0.x * model_normal.x + draw.axis1.x * model_normal.y + draw.axis2.x * model_normal.z,
        draw.axis0.y * model_normal.x + draw.axis1.y * model_normal.y + draw.axis2.y * model_normal.z,
        draw.axis0.z * model_normal.x + draw.axis1.z * model_normal.y + draw.axis2.z * model_normal.z,
    );

    let render_position = jka_to_render(world);
    let render_normal = normalize(vec3<f32>(world_normal.x, world_normal.z, -world_normal.y));

    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(render_position, 1.0);
    if (SPEC_ERASE_DEPTH_TOLERANCE > 0.0) {
        let toward_eye = camera.camera_pos_time.xyz - render_position;
        let eye_distance = length(toward_eye);
        let shift = min(SPEC_ERASE_DEPTH_TOLERANCE, eye_distance * 0.5);
        let nearer_position = render_position + toward_eye * (shift / max(eye_distance, 0.0001));
        let nearer_clip = camera.view_proj * vec4<f32>(nearer_position, 1.0);
        // Keep the original pixel footprint; only bias the depth test.
        output.clip_position.z = nearer_clip.z * (output.clip_position.w / nearer_clip.w);
    }
    // `tcGen environment` (packed into params.w; see ghoul2_specular_viewer /
    // Ghoul2GpuSkinning::env_map): q3's RB_CalcEnvironmentTexCoords, done
    // directly in render space (an orthogonal axis relabeling of JKA space,
    // so the reflection vector's components just swap/flip accordingly:
    // JKA .y = render -.z, JKA .z = render .y).
    // As in q3, the stage's tcMods transform the generated coordinates too.
    var base_uv = input.uv;
    if (draw.params.w != 0u) {
        let viewer = normalize(camera.camera_pos_time.xyz - render_position);
        let d = 2.0 * dot(render_normal, viewer);
        let reflected = render_normal * d - viewer;
        base_uv = vec2<f32>(0.5 - reflected.z * 0.5, 0.5 - reflected.y * 0.5);
    }
    output.uv = base_uv * draw.uv_xform.xy + draw.uv_xform.zw
        + vec2<f32>(base_uv.y * draw.uv_cross.x, base_uv.x * draw.uv_cross.y);
    output.normal = render_normal;
    output.draw_index = draw_index;
    var draw_color = draw.color;
    // A Source 1 material reacts to the light grid per pixel, with its own
    // normal and Valve's diffuse term, so the fragment stage reads the same
    // numbers straight out of `skin_draws` instead.
    if (!ENABLE_SOURCE_MATERIAL && draw.params.y != 0u) {
        var incoming = 0.0;
        let light_dir = draw.classic_direction.xyz;
        let light_len_sq = dot(light_dir, light_dir);
        if (light_len_sq > 1.0e-8) {
            incoming = max(dot(render_normal, light_dir * inverseSqrt(light_len_sq)), 0.0);
        }
        // Runtime sun (entity sun lighting): zero radiance when the feature is off.
        var sun_incoming = 0.0;
        let sun_dir = draw.classic_sun_direction.xyz;
        let sun_len_sq = dot(sun_dir, sun_dir);
        if (sun_len_sq > 1.0e-8) {
            sun_incoming = max(dot(render_normal, sun_dir * inverseSqrt(sun_len_sq)), 0.0);
        }
        let lighting = clamp(
            (draw.classic_ambient.xyz
                + incoming * draw.classic_directed.xyz
                + sun_incoming * draw.classic_sun_directed.xyz) / 255.0,
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        draw_color = vec4<f32>(draw.color.rgb * lighting, draw.color.a);
    }
    draw_color.a = draw_color.a * specular_alpha(draw, world, world_normal);
    output.color = draw_color;
    output.world_position = render_position;
    return output;
}

fn model_brightness() -> f32 {
    // Setup > Video > Model brightness (extra multiplier, 1.0 = neutral).
    return bitcast<f32>(entity_lighting.values.y);
}

fn lit_color(normal: vec3<f32>, tex_in: vec4<f32>) -> vec4<f32> {
    // BSP LIGHTGRID is already applied above from the per-draw OpenJK probe
    // sample. The previous directional placeholder remains only under
    // irradiance-volume mode until Ghoul2 is bound to the ambient-cube data.
    // Setup > Video > Model brightness (extra multiplier, 1.0 = neutral).
    let tex = vec4<f32>(tex_in.rgb * model_brightness(), tex_in.a);
    if (entity_lighting.values.x != 2u) {
        return tex;
    }
    let light_dir = normalize(vec3<f32>(0.35, 0.75, 0.55));
    let diffuse = max(dot(normalize(normal), light_dir), 0.0);
    let lighting = 0.32 + 0.68 * diffuse;
    return vec4<f32>(tex.rgb * lighting, tex.a);
}

// OpenJK r_drawfog 2 applies GL_EXP2 hardware fog to entities too: their fog
// num is the world's global fog, whose bounds cover the whole world. This is
// the same curve and colour decode as bsp.wgsl legacy_separate_fog, so a
// player and the floor at the same distance receive identical fog.
// Manual fog (no authored map fog) uses the same path in both Legacy modes;
// Legacy 1 authored global fog is instead applied in post from the prepass.
fn apply_self_legacy_fog(rgb: vec3<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec3<f32> {
    if (!ENABLE_LEGACY_FOG) {
        return rgb;
    }
    let mode = entity_lighting.legacy_fog_params.x;
    if (mode < 0.5) {
        return rgb;
    }
    let authored_depth = max(entity_lighting.legacy_fog_color_depth.a, 0.001);
    let scale = entity_lighting.legacy_fog_params.y;
    var amount = 0.0;
    if (mode < 1.5) {
        // GL_EXP2 density ln(255)^0.5 / depthForOpaque over eye-forward depth.
        let forward_distance = max(
            dot(world_position - camera.camera_pos_time.xyz, normalize(camera.camera_forward.xyz)),
            0.0
        );
        let scaled = forward_distance / authored_depth * scale;
        amount = 1.0 - exp(-5.5412635 * scaled * scaled);
    } else {
        let radial = distance(world_position, camera.camera_pos_time.xyz) / authored_depth;
        amount = 1.0 - exp(-radial * scale * 0.26);
    }
    // OpenJK mGLFogColorOverride: GL_ONE GL_ONE and GL_DST_COLOR GL_ZERO stages
    // fog toward black so the blend equation does not add/multiply fog colour.
    let fog_color = select(entity_lighting.legacy_fog_color_depth.rgb, vec3<f32>(0.0), fog_to_black);
    return mix(rgb, fog_color, clamp(amount, 0.0, 1.0));
}

fn fogged(color: vec4<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec4<f32> {
    return vec4<f32>(apply_self_legacy_fog(color.rgb, world_position, fog_to_black || FORCE_FOG_BLACK), color.a);
}

@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}

// -----------------------------------------------------------------------------
// Source 1 `VertexLitGeneric` phong, ported from the Source SDK:
// `common_vertexlitgeneric_dx9.h` (DiffuseTerm, SpecularAndRimTerms, Fresnel,
// Fresnel4) and the body of `skin_ps20b.fxc`, onto this engine's light set.
//
// JKA gives an entity one light grid sample - a direction-independent ambient
// plus one directed light - and optionally the runtime sun as a second light.
// Those stand in for Valve's ambient cube and its local light array; nothing
// else about the model changes. Everything is in render space, which is what
// the interpolated normal and position are in, and whose .y is JKA's up axis
// (so it is what Valve's `worldSpaceNormal.z` means in the rim term).
// -----------------------------------------------------------------------------

/// Per-pixel tangent frame from screen-space derivatives, so no vertex format,
/// compute-skinning path or geometry cache has to grow a tangent.
///
/// T is `+dP/du` and B is `+dP/dv`, which is the frame Source itself uses:
/// `CalcTriangleTangentSpace` returns exactly those two partials as its
/// TANGENT/BINORMAL, and `skin_vs20.fxc` puts them in that order. A Source
/// normal map's green channel therefore means here what it meant there.
fn source_tangent_normal(
    n: vec3<f32>,
    position: vec3<f32>,
    uv: vec2<f32>,
    texel: vec3<f32>,
) -> vec3<f32> {
    let dp1 = dpdx(position);
    let dp2 = dpdy(position);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    // Solved against the plane of the normal, so the frame is orthogonal to it
    // without a separate Gram-Schmidt step.
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    // One scale for both axes keeps the frame's shape; a degenerate frame (no
    // UV gradient) collapses to the geometric normal rather than exploding.
    let scale = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1.0e-12));
    let tangent = texel * 2.0 - 1.0;
    return normalize((t * tangent.x + b * tangent.y) * scale + n * tangent.z);
}

/// `DiffuseTerm`: a saturated Lambert, or Valve's Half Lambert squared.
fn source_diffuse_term(n: vec3<f32>, l: vec3<f32>, half_lambert: bool) -> f32 {
    let n_dot_l = dot(n, l);
    if (half_lambert) {
        let scaled = clamp(n_dot_l * 0.5 + 0.5, 0.0, 1.0);
        return scaled * scaled;
    }
    return clamp(n_dot_l, 0.0, 1.0);
}

/// `Fresnel(vNormal, vEyeDir, vRanges)`, in the engine's own packed form:
/// `vRanges` arrives as ((mid-min)*2, mid, (max-mid)*2).
fn source_fresnel_ranges(n: vec3<f32>, v: vec3<f32>, ranges: vec3<f32>) -> f32 {
    var f = clamp(1.0 - dot(n, v), 0.0, 1.0);
    f = f * f - 0.5;
    return ranges.y + select(ranges.x, ranges.z, f >= 0.0) * f;
}

/// `Fresnel4`: the traditional term squared twice. Valve uses this one, not the
/// ranges, for the rim.
fn source_fresnel4(n: vec3<f32>, v: vec3<f32>) -> f32 {
    let f = clamp(1.0 - dot(n, v), 0.0, 1.0);
    let f2 = f * f;
    return f2 * f2;
}

struct SourceLighting {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
    rim: vec3<f32>,
};

/// One light's contribution: `DiffuseTerm` plus `SpecularAndRimTerms`.
/// `direction` is the unnormalized render-space direction toward the light and
/// `radiance` its 0..255 light grid colour.
fn source_add_light(
    sum: SourceLighting,
    n: vec3<f32>,
    v: vec3<f32>,
    direction: vec3<f32>,
    radiance: vec3<f32>,
    phong: vec4<f32>,
    half_lambert: bool,
) -> SourceLighting {
    var out = sum;
    let length_sq = dot(direction, direction);
    if (length_sq <= 1.0e-8) {
        return out;
    }
    let l = direction * inverseSqrt(length_sq);
    let color = radiance / 255.0;
    out.diffuse += color * source_diffuse_term(n, l, half_lambert);
    if (phong.x > 0.0) {
        // SpecularAndRimTerms: (L.R)^k, masked by N.L and the light colour. The
        // rim shares the reflection dot and only changes the exponent.
        let reflected = n * (2.0 * dot(n, v)) - v;
        let l_dot_r = clamp(dot(reflected, l), 0.0, 1.0);
        let n_dot_l = clamp(dot(n, l), 0.0, 1.0);
        out.specular += color * (pow(l_dot_r, phong.y) * n_dot_l);
        if (phong.z > 0.0) {
            out.rim += color * (pow(l_dot_r, phong.w) * n_dot_l);
        }
    }
    return out;
}

fn source_material_color(input: VertexOut, tex: vec4<f32>) -> vec4<f32> {
    let draw = skin_draws[input.draw_index];
    // Sampled before any branch: a texture read may not sit in non-uniform
    // control flow, and nothing read out of `skin_draws` is provably uniform.
    let normal_texel = textureSample(normal_texture, base_sampler, input.uv);
    if (draw.params.y == 0u) {
        // No light grid sample for this draw (entity lighting off): there is
        // nothing for Valve's model to react to, so stay on the JKA path.
        return lit_color(input.normal, tex) * input.color;
    }
    let geometric = normalize(input.normal);
    let n = source_tangent_normal(geometric, input.world_position, input.uv, normal_texel.rgb);
    let v = normalize(camera.camera_pos_time.xyz - input.world_position);
    let ambient = draw.classic_ambient.xyz / 255.0;
    let half_lambert = (u32(draw.source_fresnel.w) & 1u) != 0u;

    var sum: SourceLighting;
    sum.diffuse = ambient;
    sum.specular = vec3<f32>(0.0);
    sum.rim = vec3<f32>(0.0);
    sum = source_add_light(
        sum,
        n,
        v,
        draw.classic_direction.xyz,
        draw.classic_directed.xyz,
        draw.source_phong,
        half_lambert,
    );
    // The runtime sun, which carries zero radiance while that feature is off.
    sum = source_add_light(
        sum,
        n,
        v,
        draw.classic_sun_direction.xyz,
        draw.classic_sun_directed.xyz,
        draw.source_phong,
        half_lambert,
    );

    // skin_ps20b.fxc: the phong mask is the normal map's alpha, modulated by the
    // Fresnel ranges and then by $phongboost.
    var specular = sum.specular
        * (normal_texel.a
            * source_fresnel_ranges(n, v, draw.source_fresnel.xyz)
            * draw.source_phong.x);
    let rim_boost = draw.source_phong.z;
    if (rim_boost > 0.0) {
        // $rimmask needs the specular exponent texture, which is not supported
        // here, so fRimMask is 1 and fRimMultiply is the plain Fresnel.
        let rim_multiply = source_fresnel4(n, v);
        // Folded in with max() so the same light is not added twice, then the
        // ambient view-ray lookup on top - the flat light grid ambient here.
        specular = max(specular, sum.rim * rim_multiply);
        specular += (ambient * rim_boost) * clamp(rim_multiply * n.y, 0.0, 1.0);
    }
    // JKA clamps its own light grid sum to 1 before it reaches the texture, and
    // the grid routinely saturates, so the diffuse half keeps that range.
    let lighting = clamp(sum.diffuse, vec3<f32>(0.0), vec3<f32>(1.0));
    let albedo = tex.rgb * model_brightness() * input.color.rgb;
    return vec4<f32>(albedo * lighting + specular, tex.a * input.color.a);
}

/// The lit colour for one fragment. `ENABLE_SOURCE_MATERIAL` is a pipeline
/// constant, so exactly one of these two bodies survives compilation and stock
/// models never sample a normal map.
fn shaded_color(input: VertexOut, tex: vec4<f32>) -> vec4<f32> {
    if (ENABLE_SOURCE_MATERIAL) {
        return source_material_color(input, tex);
    }
    return lit_color(input.normal, tex) * input.color;
}

// Profile is addressed by each skin instance, not the global camera rain.
// World (render) Y is JKA Z; 8 samples every 8 JKA units, blended in height.
fn immersion_wetness(input: VertexOut) -> f32 {
    if (!ENABLE_MODEL_WET) { return 0.0; }
    let draw = skin_draws[input.draw_index];
    if (draw.water_base.w < 0.5) { return 0.0; }
    let band = clamp((input.world_position.y - draw.water_base.x) * 0.125, 0.0, 7.0);
    let lo = u32(band);
    let hi = min(lo + 1u, 7u);
    let levels = array<f32, 8>(
        draw.water_bands0.x, draw.water_bands0.y, draw.water_bands0.z, draw.water_bands0.w,
        draw.water_bands1.x, draw.water_bands1.y, draw.water_bands1.z, draw.water_bands1.w
    );
    return mix(levels[lo], levels[hi], fract(band));
}

@fragment
fn fs_glass(input: VertexOut) -> @location(0) vec4<f32> {
    let last_mip = f32(max(textureNumLevels(base_texture), 1u) - 1u);
    let tint = textureSampleLevel(base_texture, base_sampler, vec2<f32>(0.5), last_mip).rgb;
    let brightness = dot(tint, vec3<f32>(0.2126, 0.7152, 0.0722));
    let f0 = 0.04 + 0.08 * sqrt(clamp(brightness, 0.0, 1.0));
    return glass_shade(input.world_position, input.normal, input.clip_position,
        camera.view_proj, camera.camera_pos_time.xyz, camera.camera_forward.xyz,
        camera.camera_pos_time.w, tint, f0);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(
        model_wet_color_with_water(shaded_color(input, tex), input.normal, input.world_position, immersion_wetness(input)),
        input.world_position,
        false
    );
}

@fragment
fn fs_mask(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    if tex.a < 0.5 {
        discard;
    }
    return fogged(
        model_wet_color_with_water(shaded_color(input, tex), input.normal, input.world_position, immersion_wetness(input)),
        input.world_position,
        false
    );
}

@fragment
fn fs_unlit(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(tex * input.color, input.world_position, false);
}

@fragment
fn fs_unlit_fog_black(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(tex * input.color, input.world_position, true);
}

// Scene depth prepass; see md3.wgsl. Skinning is identical to vs_main so the
// prepass and colour pass rasterize the same surface.
struct PrepassVertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) normal: vec3<f32>,
};

struct PrepassOut {
    @location(0) linear_depth: f32,
    @location(1) motion_vector: vec2<f32>,
    // G carries the smooth cloud-shadow sun-facing code (see md3.wgsl).
    @location(2) reflection_policy: vec4<f32>,
};

// See md3.wgsl cloud_shadow_facing_code.
fn cloud_shadow_facing_code(normal: vec3<f32>) -> f32 {
    let sun = entity_lighting.cloud_shadow_sun;
    if (sun.w < 0.5) {
        return 0.0;
    }
    let length_sq = dot(normal, normal);
    if (length_sq < 1.0e-6) {
        return 0.0;
    }
    let facing = smoothstep(0.0, 0.3, dot(normal * inverseSqrt(length_sq), sun.xyz));
    return (26.0 + 100.0 * facing) / 255.0;
}

fn prepass_output(world_position: vec3<f32>, normal: vec3<f32>) -> PrepassOut {
    var out: PrepassOut;
    out.linear_depth = distance(world_position, camera.camera_pos_time.xyz);
    out.motion_vector = vec2<f32>(0.0);
    out.reflection_policy = vec4<f32>(0.0, cloud_shadow_facing_code(normal), 1.0, 2.0 / 3.0);
    return out;
}

@vertex
fn vs_prepass(input: VertexIn, @builtin(instance_index) draw_index: u32) -> PrepassVertexOut {
    let draw = skin_draws[draw_index];
    let model_position = skin_position(
        input.position,
        input.bone_indices,
        input.weights,
        input.weight_count,
        draw.params.x,
    );
    let render_position = jka_to_render(model_to_world(draw, model_position));
    // Same weight-0 bone normal as vs_main, for the cloud-shadow facing code.
    let model_normal = transform_vector(
        bones[draw.params.x + input.bone_indices.x],
        input.normal,
    );
    let world_normal = vec3<f32>(
        draw.axis0.x * model_normal.x + draw.axis1.x * model_normal.y + draw.axis2.x * model_normal.z,
        draw.axis0.y * model_normal.x + draw.axis1.y * model_normal.y + draw.axis2.y * model_normal.z,
        draw.axis0.z * model_normal.x + draw.axis1.z * model_normal.y + draw.axis2.z * model_normal.z,
    );
    var output: PrepassVertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(render_position, 1.0);
    output.uv = input.uv;
    output.world_position = render_position;
    output.normal = vec3<f32>(world_normal.x, world_normal.z, -world_normal.y);
    return output;
}

@fragment
fn fs_prepass(input: PrepassVertexOut) -> PrepassOut {
    return prepass_output(input.world_position, input.normal);
}

@fragment
fn fs_prepass_mask(input: PrepassVertexOut) -> PrepassOut {
    if textureSample(base_texture, base_sampler, input.uv).a < 0.5 {
        discard;
    }
    return prepass_output(input.world_position, input.normal);
}

@fragment
fn fs_rain_prepass(input: PrepassVertexOut) -> @location(0) f32 {
    return distance(input.world_position, camera.camera_pos_time.xyz);
}

@fragment
fn fs_rain_prepass_mask(input: PrepassVertexOut) -> @location(0) f32 {
    if textureSample(base_texture, base_sampler, input.uv).a < 0.5 {
        discard;
    }
    return distance(input.world_position, camera.camera_pos_time.xyz);
}

// Entity shadow map (Dynamic shadows = Entity map): depth-only light-space pass.
// Opaque casters use vs_prepass with no fragment stage; only alpha-tested
// surfaces need this discard. Same 0.5 cutoff as fs_prepass_mask.
@fragment
fn fs_shadow_mask(input: PrepassVertexOut) {
    if textureSample(base_texture, base_sampler, input.uv).a < 0.5 {
        discard;
    }
}

// Spectator/demo outline mask. These deliberately ignore material/lighting; the
// fullscreen compositor extracts only the antialiased silhouette edge.
@fragment
fn fs_spec_mask_red() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.045, 0.025, 1.0); }
@fragment
fn fs_spec_mask_blue() -> @location(0) vec4<f32> { return vec4<f32>(0.035, 0.22, 1.0, 1.0); }
@fragment
fn fs_spec_mask_yellow() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.62, 0.025, 1.0); }
@fragment
fn fs_spec_mask_clear() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }

@fragment fn fs_glass_mask(input: VertexOut) -> @location(0) vec4<f32> { return glass_mask_value(input.world_position,input.normal,camera.camera_pos_time.xyz,camera.camera_forward.xyz); }
