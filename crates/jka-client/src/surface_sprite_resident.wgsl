// Resident native surfaceSprites (vertical / flattened / oriented).
// Instances are Raven's static per-sprite values baked at map load; this
// vertex shader evaluates the camera/time half of
// append_jka_vegetation_surface_sprites (surface_sprites.rs) per corner.
// Keep the two in step: the CPU path is the r_surfaceSpritesGpu 0 reference.

struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

struct Frame {
    // JKA camera position, seconds.
    camera_seconds: vec4<f32>,
    right: vec4<f32>,
    forward: vec4<f32>,
    // Normalized wind direction xy, strength z.
    wind: vec4<f32>,
};

struct Instance {
    position_spread: vec4<f32>,
    // Signed width, height, fade start, baked light 0..255.
    size_fade_light: vec4<f32>,
    skew: vec2<f32>,
    right_index: u32,
    emitter: u32,
};

struct Emitter {
    // fadedist^2, 1 / (cutdist^2 - fadedist^2), fade range, ssFadescale.
    fade: vec4<f32>,
    // ssWind, ssWindIdle, alpha cutoff, unused.
    wind: vec4<f32>,
    // hang_down, flattened, oriented, additive.
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;
@group(2) @binding(0) var<uniform> frame: Frame;
@group(2) @binding(1) var<storage, read> instances: array<Instance>;
@group(2) @binding(2) var<storage, read> emitters: array<Emitter>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) alpha_cutoff: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var output: VertexOutput;
    let sprite = instances[instance_index];
    let emitter = emitters[sprite.emitter];
    let p = sprite.position_spread.xyz;
    let to_camera = frame.camera_seconds.xyz - p;
    // Raven blends 1 - (|cam - v_k|^2 - fadedist^2) * inv over barycentrics;
    // sum(b_k |cam - v_k|^2) = |cam - p|^2 + sum(b_k |v_k - p|^2).
    let alpha_pos = 1.0 - (dot(to_camera, to_camera) + sprite.position_spread.w - emitter.fade.x) * emitter.fade.y;
    var alpha = clamp(1.0 - (sprite.size_fade_light.z - alpha_pos) / max(emitter.fade.z, 1.0e-6), 0.0, 1.0);
    if alpha <= 0.0 {
        // Collapse every corner to one point outside the clip volume.
        output.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        output.uv = vec2<f32>(0.0);
        output.color = vec4<f32>(0.0);
        output.alpha_cutoff = 0.0;
        return output;
    }
    var light = sprite.size_fade_light.w;
    if emitter.flags.w != 0u {
        light = (128.0 + light * 0.5) * alpha;
        alpha = 1.0;
    }
    var width = sprite.size_fade_light.x;
    let height = sprite.size_fade_light.y;
    if emitter.fade.w != 0.0 && alpha_pos < 1.0 {
        width = width * (1.0 + emitter.fade.w * (1.0 - alpha_pos));
    }
    let seconds = frame.camera_seconds.w;
    let angle = (p.x + p.y) * 0.02 + seconds * 1.5;
    let idle = height * emitter.wind.y * 0.075;
    var tip = p + vec3<f32>(sprite.skew, 0.0) + vec3<f32>(cos(angle), sin(angle), 0.0) * idle;
    tip.z = tip.z + select(height, -height, emitter.flags.x != 0u);
    let wind_strength = frame.wind.z;
    if emitter.wind.x > 0.0 && wind_strength > 0.001 {
        tip = tip + vec3<f32>(frame.wind.xy, 0.0) * (height * emitter.wind.x * wind_strength * 0.3);
        tip.z = tip.z + sin(angle * 2.5) * height * emitter.wind.x * wind_strength * 0.03;
    }
    let view_right = frame.right.xyz;
    let forward = frame.forward.xyz;
    let flattened = emitter.flags.y != 0u;
    let oriented = emitter.flags.z != 0u;
    var right: vec3<f32>;
    if flattened {
        // TaystJK's distinctive flattened sprite right-vector.
        right = vec3<f32>(sin(radians(p.x)) * width, cos(radians(p.x)) * height, 0.0);
    } else if oriented {
        right = view_right * (width * 0.5);
    } else {
        var right_vectors = array<vec3<f32>, 4>(
            view_right * 0.985 + forward * 0.174,
            view_right * 0.866 - forward * 0.5,
            view_right * 0.866 + forward * 0.5,
            view_right * 0.985 - forward * 0.174,
        );
        right = right_vectors[sprite.right_index & 3u] * (width * 0.5);
    }
    let top_left_skew = select(forward * (width * 0.2), vec3<f32>(0.0), oriented);
    var points = array<vec3<f32>, 4>(p + right, tip + right, tip - right + top_left_skew, p - right);
    var uvs = array<vec2<f32>, 4>(vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0));
    var corners = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let corner = corners[vertex_index];
    let jka = points[corner];
    // scene::render_position: [x, z, -y].
    output.position = camera.view_proj * vec4<f32>(jka.x, jka.z, -jka.y, 1.0);
    output.uv = uvs[corner];
    let intensity = clamp(light / 255.0, 0.0, 1.0);
    output.color = vec4<f32>(intensity, intensity, intensity, alpha);
    output.alpha_cutoff = emitter.wind.z;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(sprite_texture, sprite_sampler, input.uv);
    // AlphaFunc GE192/GE128/GT0 tests sampled alpha multiplied by vertex fade.
    // Discard *before* depth write: opaque One/Zero stages must not paint a quad.
    if input.alpha_cutoff > 0.0 && texel.a * input.color.a < input.alpha_cutoff {
        discard;
    }
    if input.alpha_cutoff < 0.0 && texel.a * input.color.a >= -input.alpha_cutoff {
        discard;
    }
    return texel * input.color;
}

@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
