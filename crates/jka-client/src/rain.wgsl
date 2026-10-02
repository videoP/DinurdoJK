struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

struct RainParticle {
    position_state: vec4<f32>,
    velocity_age: vec4<f32>,
    misc: vec4<f32>,
};

struct RainRenderUniform {
    appearance: vec4<f32>, // streak length world units, feather width world units, opacity, splash size world units
    splash: vec4<f32>,     // splash duration, viewport width, viewport height, puddle accumulation
    color: vec4<f32>,
    look: vec4<f32>,       // scattered puddle amount, camera underwater, remaining lanes reserved
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<storage, read> particles: array<RainParticle>;
@group(1) @binding(1) var<uniform> rain: RainRenderUniform;

struct DropOut {
    @builtin(position) position: vec4<f32>,
    @location(0) across: f32,
    @location(1) along: f32,
    @location(2) opacity: f32,
};

@vertex
fn vs_drop(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> DropOut {
    var out: DropOut;
    let particle = particles[instance_index];
    // Under water every drop is above the surface; none of it is visible from below.
    if (rain.look.y > 0.5 || particle.position_state.w < 0.5 || particle.position_state.w > 1.5) {
        out.position = vec4<f32>(2.5, 2.5, 0.0, 1.0);
        out.across = 0.0;
        out.along = 0.0;
        out.opacity = 0.0;
        return out;
    }

    let position = particle.position_state.xyz;
    let speed = max(length(particle.velocity_age.xyz), 1.0);
    let velocity = particle.velocity_age.xyz / speed;

    let to_camera_raw = camera.camera_pos_time.xyz - position;
    let to_camera_len = max(length(to_camera_raw), 0.001);
    let distance = to_camera_len;

    // The drop's dimensions are physical/world-space; perspective makes nearby
    // drops larger and distant ones smaller. Two limits keep that from looking
    // wrong. A drop right at the lens would smear across a quarter of the screen
    // as a long sideways streak, so a streak may cover at most ~10% of the screen
    // height. And a drop far away would shrink below a pixel and vanish, leaving
    // the middle distance empty, so it is never thinner than about a pixel (its
    // opacity gives back part of the width it gained, so it stays a fine line).
    let focal = max(camera.view_proj[1][1], 0.1);
    let pixel_world = 2.0 * distance / (focal * max(rain.splash.z, 1.0));
    let length_world = min(
        rain.appearance.x * clamp(speed / 1650.0, 0.96, 1.04),
        0.20 * distance / focal
    );
    let width_world = max(rain.appearance.y, pixel_world * 1.25);
    let half_streak = velocity * length_world * 0.5;
    let to_camera = to_camera_raw / to_camera_len;
    var side_world = cross(velocity, to_camera);
    if (dot(side_world, side_world) < 1e-6) {
        side_world = cross(velocity, vec3<f32>(1.0, 0.0, 0.0));
        if (dot(side_world, side_world) < 1e-6) {
            side_world = cross(velocity, vec3<f32>(0.0, 0.0, 1.0));
        }
    }
    side_world = normalize(side_world);
    let half_width = side_world * width_world * 0.5;

    var endpoint_world = position - half_streak;
    var side = -1.0;
    var along = 0.0;
    switch vertex_index {
        case 0u: { endpoint_world = position - half_streak; side = -1.0; along = 0.0; }
        case 1u: { endpoint_world = position + half_streak; side = -1.0; along = 1.0; }
        case 2u: { endpoint_world = position - half_streak; side =  1.0; along = 0.0; }
        default: { endpoint_world = position + half_streak; side =  1.0; along = 1.0; }
    }

    let clip = camera.view_proj * vec4<f32>(endpoint_world + half_width * side, 1.0);
    if (clip.w <= 0.01) {
        out.position = vec4<f32>(2.5, 2.5, 0.0, 1.0);
        out.across = 0.0;
        out.along = 0.0;
        out.opacity = 0.0;
        return out;
    }

    // Fade drops out as they near the lens, and keep the middle distance readable:
    // thin drops keep most of their opacity and gain a little with range to make
    // up for the haze that would otherwise swallow them.
    let near_fade = smoothstep(90.0, 240.0, distance);
    let distance_fade = 1.0 - smoothstep(1500.0, 2300.0, distance);
    let thinness = rain.appearance.y / width_world;
    let mid_range = 1.0 + 0.55 * smoothstep(250.0, 800.0, distance);
    out.position = clip;
    out.across = side;
    out.along = along;
    out.opacity = rain.color.a * rain.appearance.z * near_fade * distance_fade
        * mix(1.0, sqrt(thinness), 0.6) * mid_range;
    return out;
}

@fragment
fn fs_drop(input: DropOut) -> @location(0) vec4<f32> {
    let across_abs = abs(input.across);
    // World-space width defines the actual quad. fwidth only makes the coverage
    // transition stable/soft in pixel space; it does not change drop dimensions.
    let aa = clamp(fwidth(input.across) * 1.35, 0.035, 0.22);
    let bright_core = 1.0 - smoothstep(0.38 - aa, 0.66 + aa, across_abs);
    let soft_feather = (1.0 - smoothstep(0.48 - aa, 1.0, across_abs)) * 0.48;
    let profile = max(bright_core, soft_feather);

    let head = smoothstep(0.0, 0.09, input.along);
    let tail = 1.0 - smoothstep(0.70, 1.0, input.along);
    let alpha = input.opacity * profile * mix(0.62, 1.0, head * tail);
    if (alpha < 0.003) {
        discard;
    }
    return vec4<f32>(rain.color.rgb, alpha);
}

struct SplashOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) age: f32,
    @location(2) world_xz: vec2<f32>,
    // Flat-area score and basin score of the surface the drop landed on.
    @location(3) field: vec2<f32>,
};

@vertex
fn vs_splash(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> SplashOut {
    var out: SplashOut;
    let particle = particles[instance_index];
    if (rain.look.y > 0.5 || particle.position_state.w < 1.5) {
        out.position = vec4<f32>(2.5, 2.5, 0.0, 1.0);
        out.local = vec2<f32>(0.0);
        out.age = 1.0;
        out.world_xz = vec2<f32>(0.0);
        out.field = vec2<f32>(0.0);
        return out;
    }

    let corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0)
    );
    let local = corners[vertex_index];
    let age = clamp(particle.velocity_age.w / max(rain.splash.x, 0.001), 0.0, 1.0);
    let size = rain.appearance.w * mix(0.35, 1.0, age);
    let world = particle.position_state.xyz + vec3<f32>(local.x * size, 1.8, local.y * size);
    out.position = camera.view_proj * vec4<f32>(world, 1.0);
    out.local = local;
    out.age = age;
    out.world_xz = particle.position_state.xz;
    out.field = particle.misc.zy;
    return out;
}


@fragment
fn fs_splash(input: SplashOut) -> @location(0) vec4<f32> {
    let radius = length(input.local);
    let ring_radius = mix(0.16, 0.94, input.age);
    let ring = 1.0 - smoothstep(0.045, 0.12, abs(radius - ring_radius));

    // A faint six-lobed crown inside the expanding ring keeps impacts from
    // reading as perfectly clean water ripples on dry stone/metal surfaces.
    let angle = atan2(input.local.y, input.local.x);
    let lobe = pow(max(cos(angle * 6.0), 0.0), 10.0);
    let crown = lobe * smoothstep(0.12, 0.32, radius) * (1.0 - smoothstep(0.34, 0.72, radius));
    let fade = (1.0 - input.age) * (1.0 - input.age);

    // Reuse the existing impact particles as puddle ripples instead of adding a
    // second particle system. The impact carries the weather field's scores for
    // the surface it hit, and the shape is the very one the material pass draws,
    // so circular rings appear exactly where standing water is.
    let accumulation = clamp(rain.splash.w, 0.0, 1.0);
    var puddle = 0.0;
    if (accumulation > 0.001) {
        var field = weather_default_field();
        field.flat_area = input.field.x;
        field.basin = input.field.y;
        puddle = weather_puddle_shape(field, input.world_xz, accumulation, rain.look.x).coverage;
    }
    // The source wetness technique changes the surface normal/smoothness; it does
    // not paint bright white ripple decals over the scene. Keep only a restrained
    // impact flash here. The expanding water ring now lives in the puddle normal.
    let ring_gain = mix(0.10, 0.16, puddle);
    let crown_gain = mix(0.20, 0.08, puddle);
    let alpha = (ring * ring_gain + crown * crown_gain) * fade
        * rain.color.a * rain.appearance.z * mix(1.05, 1.35, puddle);
    if (alpha < 0.004) {
        discard;
    }
    return vec4<f32>(rain.color.rgb * mix(1.01, 1.06, puddle), alpha);
}
