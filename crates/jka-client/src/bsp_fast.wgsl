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
};
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

@vertex fn vs_main(input: VertexIn, @builtin(instance_index) instance_index: u32) -> VertexOut {
    var output: VertexOut;
    let deformation = deform_surface_deformation_vertex(
        input.position,
        input.normal,
        material.header.w,
        instance_index,
        true
    );
    let world_position = deformation.xyz;
    output.clip_position = camera.view_proj * vec4<f32>(world_position, 1.0);
    output.uv = generated_uv(input);
    output.lightmap_uv = input.lightmap_uv;
    output.color = input.color;
    // Keep the known-fast fragment path free of the camera bind group.
    // When cached BSP AO is disabled, feed neutral AO (1.0) through the
    // existing interpolator so fs_main never needs camera.render_flags.
    let cached_static_ao = select(1.0, input.static_ao, camera.render_flags.y != 0u);
    output.sky_dir_ao = vec4<f32>(world_position - camera.camera_pos_time.xyz, cached_static_ao);
    output.world_position = world_position;
    output.world_normal = surface_deformation_vertex_normal(
        input.position, input.normal, material.header.w, instance_index, true
    );
    output.shell_kind = select(0u, 1u, instance_index != 0u);
    output.shell_coverage = deformation.w;
    return output;
}

@fragment fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    if (surface_deformation_should_discard(
        input.world_position, material.header.w, input.world_normal, input.shell_kind, input.shell_coverage, input.clip_position.xy, true
    )) {
        discard;
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

    var base = textureSample(base_texture, base_sampler, input.uv) * stage_color;
    if (material.params.x > 0.0 && base.a < material.params.x) {
        discard;
    }

    // Only the synthesized implicit material uses a single-pass
    // base-texture Ã— lightmap path. Explicit JKA shader scripts render their
    // `$lightmap` as its own ordered blend stage instead.
    if ((material.header.z & 2u) != 0u && input.lightmap_uv.x >= 0.0) {
        base = vec4<f32>(
            base.rgb * textureSample(lightmap_texture, lightmap_sampler, input.lightmap_uv).rgb,
            base.a
        );
    }
    // Cached static BSP AO only modulates the baked-light contribution.
    // Explicit $lightmap stages use tcGen lightmap (header.x == 1); the synthesized
    // one-pass material is identified by the modulate-lightmap bit. Dynamic lights
    // added by the advanced shaders therefore remain unoccluded by this static cache.
    if (material.header.x == 1u || (material.header.z & 2u) != 0u || (material.header.z & 131072u) != 0u) {
        let static_ao = clamp(input.sky_dir_ao.w, 0.0, 1.0);
        base = vec4<f32>(base.rgb * static_ao, base.a);
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
