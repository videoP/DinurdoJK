// River whitewater sprites: Niagara sprite renderer (FaceCamera, unaligned,
// renderer visibility tag 0) with M_SimpleSpray through MI_SimpleSpray.
// Copyright Epic Games, Inc. All Rights Reserved. See river_secondary.rs.
// Lighting deviation (not Unreal): the translucency lighting volume's SH2
// (TLM_VolumetricDirectional) is approximated by the water body's indirect
// term plus the shadowed sun through the SH2 cosine lobe 0.25 + 0.5 N.L.
@group(6) @binding(0) var<storage, read> river_spray_particles: array<RiverSprayParticle>;
@group(6) @binding(1) var river_spray_noise: texture_2d<f32>;
@group(6) @binding(2) var river_spray_normals: texture_2d<f32>;
@group(6) @binding(3) var river_spray_sampler: sampler;

struct RiverSprayOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) @interpolate(flat) alpha_random: vec2<f32>,
    @location(3) @interpolate(flat) tangent_right: vec3<f32>,
    @location(4) @interpolate(flat) tangent_up: vec3<f32>,
};

@vertex fn vs_spray(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> RiverSprayOut {
    var out: RiverSprayOut;
    let p = river_spray_particles[instance];
    var corners = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
                                      vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let uv = corners[vertex % 6u];
    let alpha = river_spray_alpha(p.misc.y); // Color = Initial.Color * (1,1,1,ScaleAlpha)
    if p.state.x != 0 || !river_spray_finite3(p.position_age.xyz) || alpha <= 0.0 {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0); // outside the clip volume
        return out;
    }
    // GetTangents, FACING_MODE_FACECAMERA: Right = ViewRight, Up = -ViewUp,
    // then the sprite rotation (degrees).
    let camera_right = river_view.right.xyz;
    let camera_up = -river_view.up.xyz;
    let rotation = p.misc.x / 180.0 * 3.14159265358979;
    let s = sin(rotation);
    let c = cos(rotation);
    let right = camera_up * s + camera_right * c;
    let up = camera_up * c - camera_right * s;
    let size = abs(p.size.xy);
    let world = p.position_age.xyz + size.x * (uv.x - 0.5) * right + size.y * (uv.y - 0.5) * up;
    let map = world / RIVER_CM_PER_UNIT;
    out.clip = camera.view_proj * vec4<f32>(map.x, map.z, -map.y, 1.0);
    out.uv = uv;
    out.world = world;
    out.alpha_random = vec2<f32>(alpha, p.misc.z);
    out.tangent_right = right;
    out.tangent_up = up;
    return out;
}

// MaterialExpressionSphereMask: saturate((1 - |A-B|/R) / (1 - Hardness)).
fn river_spray_sphere_mask(a: vec2<f32>, b: vec2<f32>, radius: f32, hardness: f32) -> f32 {
    let normalized = length(a - b) / max(0.00001, radius);
    return saturate((1.0 - normalized) / max(1.0 - hardness, 0.00001));
}

@fragment fn fs_spray(input: RiverSprayOut) -> @location(0) vec4<f32> {
    let m = RIVER_SPRAY_MATERIAL; // opacity mult, distortion strength, distortion scale, noise centre
    let k = RIVER_SPRAY_MASK;     // sphere radius, hardness, normal sphere radius, normal flatness
    // Noise coordinates: TexCoord * DistortionScale + ParticleRandom.
    let noise_uv = input.uv * m.z + vec2<f32>(input.alpha_random.y);
    let noise = textureSample(river_spray_noise, river_spray_sampler, noise_uv).r;
    let normal_map = textureSample(river_spray_normals, river_spray_sampler, noise_uv).xy;
    let mask_uv = input.uv + vec2<f32>((noise - m.w) * m.y);
    let opacity = river_spray_sphere_mask(mask_uv, vec2<f32>(0.5), k.x, k.y) * m.x * input.alpha_random.x;
    if opacity <= 0.0 {
        discard;
    }
    // Custom_0 sphere normal, FlattenNormal(normal map, -5) and
    // BlendAngleCorrectedNormals, all in sprite tangent space (BC5-style
    // normal map: Z rebuilt from XY).
    let x = (input.uv.x - 0.5) * 2.0;
    let y = (input.uv.y - 0.5) * 2.0;
    let z2 = k.z * k.z - x * x - y * y;
    var base = vec3<f32>(0.0, 0.0, 1.0);
    if z2 >= 0.0 {
        base = vec3<f32>(x, y, sqrt(z2));
    }
    let sample = normal_map * 2.0 - 1.0;
    let detail = vec3<f32>(sample, sqrt(saturate(1.0 - dot(sample, sample))));
    let flattened = mix(detail, vec3<f32>(0.0, 0.0, 1.0), k.w);
    let t = vec3<f32>(base.xy, base.z + 1.0);
    let u = vec3<f32>(-flattened.xy, flattened.z);
    let tangent_normal = normalize(normalize(t * dot(t, u) - (base.z + 1.0) * u));
    // CalcTangentBasis (ENM_CameraFacing): the third axis faces the camera.
    let facing = -river_view.forward.xyz;
    let n = normalize(input.tangent_right * tangent_normal.x + input.tangent_up * tangent_normal.y + facing * tangent_normal.z);

    let diffuse_color = RIVER_SPRAY_BASE_COLOR; // Base Color * Particle Color (1), metallic 0
    var indirect: vec3<f32>;
    if river_view.sun_illuminance.w > 0.5 {
        indirect = river_slw_sky_indirect(env_irradiance(env_map, env_sampler, env, river_slw_render_vector(n)));
    } else {
        // The window's classic indirect field at this XY (window UV).
        let uv = (input.world.xy - body.origin.xy) / body.grid.xy + 0.5;
        let ambient = textureSampleLevel(ambient_texture, ambient_sampler, uv, 0.0);
        let directed = textureSampleLevel(directed_texture, ambient_sampler, uv, 0.0);
        indirect = river_slw_classic_indirect(ambient, directed, n);
    }
    let l = river_view.sun_direction.xyz;
    let map_position = input.world / RIVER_CM_PER_UNIT;
    let shadow = cascaded_shadow_visibility(vec3<f32>(map_position.x, map_position.z, -map_position.y), river_slw_render_vector(n));
    let sun = river_view.sun_illuminance.rgb * shadow * max(0.25 + 0.5 * dot(n, l), 0.0);
    let color = diffuse_color * indirect + diffuse_color * (1.0 / 3.14159265358979) * sun;
    return vec4<f32>(color, saturate(opacity));
}
