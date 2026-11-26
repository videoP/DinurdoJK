struct FlameUniform {
    view_proj: mat4x4<f32>,
    centre: vec4<f32>,
    right_extent: vec4<f32>,
    up_extent: vec4<f32>,
    eye_time: vec4<f32>,
    forward_and_units: vec4<f32>,
    airflow: vec4<f32>,
    crop: vec4<f32>,
}

@group(0) @binding(0) var<uniform> flame: FlameUniform;
@group(0) @binding(1) var flame_velocity_density: texture_2d<f32>;
@group(0) @binding(2) var flame_detail: texture_2d<f32>;
@group(0) @binding(3) var flame_clamp: sampler;
@group(0) @binding(4) var flame_wrap: sampler;
@group(0) @binding(5) var flame_radial_depth: texture_2d<f32>;

// |cos| between the plane normal and the direction to the eye, set before each
// private ninja_flame_shade call (opacity Fresnel; FLAME_MATERIAL_CONTRACT).
var<private> flame_view_cosine: f32 = 1.0;

struct FlameVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
}

fn flame_plane_vertex(plane_uv: vec2<f32>, uv: vec2<f32>) -> FlameVertex {
    let normal = cross(flame.right_extent.xyz, flame.up_extent.xyz);
    let world = flame.centre.xyz
        + flame.right_extent.xyz * ((plane_uv.x - 0.5) * flame.right_extent.w)
        + flame.up_extent.xyz * ((0.5 - plane_uv.y) * flame.up_extent.w)
        + ninja_flame_offset_cm(uv, normalize(normal)) / flame.forward_and_units.w;
    return FlameVertex(flame.view_proj * vec4<f32>(world, 1.0), uv, world);
}

@vertex
fn flame_vertex(@builtin(vertex_index) vertex: u32) -> FlameVertex {
    let uv = array(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    )[vertex];
    // The quad may show only a window of the field, centred on the emitter.
    let field_uv = vec2<f32>(0.5) + (uv - vec2<f32>(0.5)) * flame.crop.xy;
    return flame_plane_vertex(uv, field_uv);
}

@vertex
fn flame_mesh_vertex(@location(0) coords: vec4<f32>) -> FlameVertex {
    return flame_plane_vertex(coords.xy, coords.zw);
}

// A quad that shows only a window of the field fades out towards that
// window's border, so drifting smoke is never cut off in a visible
// rectangle. Quads that show the whole field are untouched (the material
// already fades at the field's own edge).
fn flame_window_fade(uv: vec2<f32>) -> f32 {
    let q = abs(uv - vec2<f32>(0.5)) / max(0.5 * flame.crop.xy, vec2<f32>(1e-4));
    let fade = vec2<f32>(1.0) - smoothstep(vec2<f32>(0.55), vec2<f32>(1.0), q);
    let windowed = flame.crop.xy < vec2<f32>(0.999);
    return select(1.0, fade.x, windowed.x) * select(1.0, fade.y, windowed.y);
}

fn shade_flame(in_vertex: FlameVertex, occlude: bool) -> vec4<f32> {
    let ray = in_vertex.world - flame.eye_time.xyz;
    let depth_pixel = vec2<i32>(in_vertex.position.xy);
    let behind = textureLoad(flame_radial_depth, depth_pixel, 0).r;
    let forward = dot(normalize(ray), flame.forward_and_units.xyz);
    let gap_cm = (behind - length(ray)) * forward * flame.forward_and_units.w;
    let normal = normalize(cross(flame.right_extent.xyz, flame.up_extent.xyz));
    flame_view_cosine = abs(dot(normal, normalize(ray)));
    // The private Ninja material is presented unchanged for every source.
    let raw = ninja_flame_shade(in_vertex.uv, flame.eye_time.w, gap_cm);
    let shaded = vec4<f32>(raw.rgb, raw.a * flame_window_fade(in_vertex.uv));
    if (occlude && behind < length(ray)) { discard; }
    let exposure = flame.airflow.x;
    let recovery = flame.airflow.y;
    if (exposure == 0.0 && recovery == 0.0) {
        return shaded;
    }
    // Use the material's existing opacity to distinguish the dense core from thin wisps.
    // Apply the response in scene-linear space so the hot core still drives bloom.
    let ember_mask = smoothstep(0.12, 0.55, shaded.a);
    let wind_tint = mix(vec3<f32>(0.72), vec3<f32>(1.65, 0.46, 0.22), ember_mask);
    let emissive = shaded.rgb * mix(vec3<f32>(1.0), wind_tint, exposure) * (1.0 + 0.55 * recovery);
    let opacity = shaded.a * (1.0 - 0.65 * exposure * (1.0 - ember_mask));
    return vec4<f32>(emissive, opacity + 0.35 * recovery * opacity * (1.0 - opacity));
}

@fragment
fn flame_fragment(in_vertex: FlameVertex) -> @location(0) vec4<f32> {
    return shade_flame(in_vertex, false);
}

@fragment
fn flame_layer_fragment(in_vertex: FlameVertex) -> @location(0) vec4<f32> {
    return shade_flame(in_vertex, true);
}

struct SparkVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) shape: vec4<f32>,
}

fn spark_quad(
    vertex: u32, position_size: vec4<f32>, direction_length: vec4<f32>, color: vec4<f32>, shape: vec4<f32>,
) -> SparkVertex {
    let uv = array(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    )[vertex];
    let to_eye = flame.eye_time.xyz - position_size.xyz;
    let face = to_eye * inverseSqrt(max(dot(to_eye, to_eye), 1e-12));
    let direction = direction_length.xyz;
    let perpendicular = cross(direction, face);
    var right = flame.right_extent.xyz;
    var up = flame.up_extent.xyz;
    if (dot(perpendicular, perpendicular) > 1e-8) {
        right = normalize(perpendicular);
        up = cross(face, right);
    }
    let world = position_size.xyz + right * ((uv.x - 0.5) * position_size.w)
        + up * ((0.5 - uv.y) * direction_length.w);
    return SparkVertex(flame.view_proj * vec4<f32>(world, 1.0), uv, world, color, shape);
}

// Fire Smoke GPU sprite particles (turret embers, Burning Man sparks) drawn
// straight from their simulation pool (ninja_fire/embers.wgsl).
struct EmberDraw {
    // Render-axis centimetres of the pool frame; length scale s.
    frame_scale: vec4<f32>,
    // Map units per authored sprite centimetre; CM_PER_MAP_UNIT.
    sprite: vec4<f32>,
}

@group(1) @binding(0) var<storage, read> ember_particles: array<EmberParticle>;
@group(1) @binding(1) var<uniform> ember_draw: EmberDraw;

fn ember_render_axes(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(v.x, v.z, -v.y);
}

@vertex
fn ember_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> SparkVertex {
    let particle = ember_particles[instance];
    let kind = u32(particle.size_mass_drag_kind.w);
    let recipe = ember_recipe(kind);
    let color = ember_color(particle, recipe, kind);
    let velocity = particle.velocity_lifetime.xyz;
    let aligned = recipe.aligned > 0.5;
    let speed = length(velocity);
    let width = particle.size_mass_drag_kind.x * ember_draw.sprite.x;
    let stretch = select(1.0, ember_stretch(speed, recipe), aligned);
    let direction = select(vec3<f32>(0.0), ember_render_axes(velocity) / max(speed, 1e-6), aligned && speed > 1e-6);
    let local = particle.anchor.xyz + particle.position_age.xyz;
    let position = (ember_draw.frame_scale.xyz + ember_render_axes(local) * ember_draw.frame_scale.w)
        / ember_draw.sprite.y;
    // Soft depth fade only for the turret's settling embers.
    let fade = select(0.0, 0.5 * width, kind != 2u);
    var quad = spark_quad(vertex, vec4<f32>(position, width), vec4<f32>(direction, width * stretch), color,
        vec4<f32>(recipe.inv_hardness, recipe.back_face, fade, 0.0));
    if (color.a <= 0.0) {
        // Invisible (a resting streak): outside the clip volume.
        quad.position = vec4<f32>(2.0, 2.0, 0.5, 1.0);
    }
    return quad;
}

fn shade_spark(in_vertex: SparkVertex, front: bool) -> vec4<f32> {
    // DefaultSpriteMaterial and its instances: SphereMask at UV 0.5, radius
    // 0.5: saturate((1 - distance / 0.5) / (1 - Hardness)). Emissive is the
    // particle colour, opacity its alpha times the mask.
    let mask = clamp((1.0 - 2.0 * distance(in_vertex.uv, vec2<f32>(0.5))) * in_vertex.shape.x, 0.0, 1.0);
    let emissive = in_vertex.color.rgb * select(in_vertex.shape.y, 1.0, front);
    return vec4<f32>(emissive, in_vertex.color.a * mask);
}

@fragment
fn spark_fragment(in_vertex: SparkVertex, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade_spark(in_vertex, front);
}

@fragment
fn spark_layer_fragment(in_vertex: SparkVertex, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let behind = textureLoad(flame_radial_depth, vec2<i32>(in_vertex.position.xy), 0).r;
    let gap = behind - distance(in_vertex.world, flame.eye_time.xyz);
    if (gap < 0.0) { discard; }
    let shaded = shade_spark(in_vertex, front);
    // Embers resting on a surface fade into it instead of being cut off.
    let fade = select(1.0, clamp(gap / max(in_vertex.shape.z, 1e-4), 0.0, 1.0), in_vertex.shape.z > 0.0);
    return vec4<f32>(shaded.rgb, shaded.a * fade);
}

// Heat haze (r_ninjaFireHeatHaze). While it is on, the flame layer is
// composited after this, so these passes refract the background only (also
// behind the translucent flame) and the flame lands sharp on top.
// `haze_gather` writes the displaced HDR scene and its mask into a scratch,
// `haze_apply` blends that back. Group 1 stays reserved for the ember pools.
struct HazeUniform {
    // x: time (s), y: strength, z: 1 / RMS of the curl field's xy, w: rise (noise tiles / s).
    params: vec4<f32>,
    // xy: 1 / gather target size (render size), zw: 1 / scene target size.
    inv_size: vec4<f32>,
}

@group(2) @binding(0) var<uniform> haze: HazeUniform;
@group(2) @binding(1) var haze_source: texture_2d<f32>;
@group(2) @binding(2) var haze_curl: texture_3d<f32>;
@group(2) @binding(3) var haze_repeat: sampler;
@group(2) @binding(4) var haze_clamp: sampler;

// The haze quad reaches this many flame-window heights above the window,
// where the hot column keeps rising past the flame tips.
const HAZE_RISE: f32 = 0.25;

struct HazeVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    // 0..1 across the (extended) haze quad.
    @location(2) quad: vec2<f32>,
    // Screen-UV displacement per map unit along the plane's right (xy) and up (zw).
    @location(3) axes: vec4<f32>,
}

fn haze_screen_uv(clip: vec4<f32>) -> vec2<f32> {
    let ndc = clip.xy / max(clip.w, 1e-3);
    return vec2<f32>(0.5 * ndc.x, -0.5 * ndc.y);
}

@vertex
fn haze_vertex(@builtin(vertex_index) vertex: u32) -> HazeVertex {
    let quad = array(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    )[vertex];
    // Plane v runs from HAZE_RISE above the flame window (negative) to its base.
    let plane = vec2<f32>(quad.x, mix(-HAZE_RISE, 1.0, quad.y));
    let field_uv = vec2<f32>(0.5) + (plane - vec2<f32>(0.5)) * flame.crop.xy;
    let base = flame_plane_vertex(plane, field_uv);
    let here = haze_screen_uv(base.position);
    let right = haze_screen_uv(flame.view_proj * vec4<f32>(base.world + flame.right_extent.xyz, 1.0)) - here;
    let up = haze_screen_uv(flame.view_proj * vec4<f32>(base.world + flame.up_extent.xyz, 1.0)) - here;
    return HazeVertex(base.position, field_uv, base.world, quad, vec4<f32>(right, up));
}

fn haze_density(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(flame_velocity_density, flame_clamp, uv, 0.0).b;
}

@fragment
fn haze_gather(in_vertex: HazeVertex) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in_vertex.position.xy);
    let eye_distance = length(in_vertex.world - flame.eye_time.xyz);
    // Geometry in front of the hot air is not refracted.
    if (textureLoad(flame_radial_depth, pixel, 0).r < eye_distance) { discard; }
    let vd = textureSampleLevel(flame_velocity_density, flame_clamp, in_vertex.uv, 0.0);
    // Hot air rises: fire below this point (towards the base, +v) heats the
    // column above it, fading with height. Taps are in flame-window heights.
    let tap = flame.crop.y;
    let heat = max(max(vd.b, 0.85 * haze_density(in_vertex.uv + vec2<f32>(0.0, 0.06 * tap))),
        max(0.65 * haze_density(in_vertex.uv + vec2<f32>(0.0, 0.14 * tap)),
            0.45 * haze_density(in_vertex.uv + vec2<f32>(0.0, 0.25 * tap))));
    let q = in_vertex.quad;
    let edge = smoothstep(0.0, 0.4, q.x) * smoothstep(1.0, 0.6, q.x)
        * smoothstep(0.0, 0.6, q.y) * smoothstep(1.0, 0.8, q.y);
    let mask = smoothstep(0.01, 0.15, heat) * edge;
    if (mask < 0.004) { discard; }
    let width = flame.right_extent.w;
    let height = flame.up_extent.w * (1.0 + HAZE_RISE);
    let time = haze.params.x;
    // One trilinear curl fetch: rising in v, evolving through the volume in w.
    let cell = 0.4 * min(width, flame.up_extent.w);
    let p = vec3<f32>(q.x * width / cell, q.y * height / cell + haze.params.w * time, 0.4 * time);
    let curl = textureSampleLevel(haze_curl, haze_repeat, p, 0.0).xy * haze.params.z;
    // Field-UV velocity (y down) leans the shimmer with the flow.
    let flow = clamp(vd.rg, vec2<f32>(-2.0), vec2<f32>(2.0)) * 0.25;
    let wobble = clamp(curl + flow, vec2<f32>(-2.0), vec2<f32>(2.0));
    let amplitude = haze.params.y * 0.05 * min(width, flame.up_extent.w) * mask;
    let offset = clamp(
        (in_vertex.axes.xy * wobble.x - in_vertex.axes.zw * wobble.y) * amplitude,
        vec2<f32>(-0.03), vec2<f32>(0.03),
    );
    let uv = in_vertex.position.xy * haze.inv_size.xy;
    // Never pull a foreground object into the haze.
    let size = vec2<i32>(textureDimensions(flame_radial_depth));
    let moved = clamp(pixel + vec2<i32>(offset / haze.inv_size.xy), vec2<i32>(0), size - vec2<i32>(1));
    let valid = textureLoad(flame_radial_depth, moved, 0).r >= eye_distance;
    let color = textureSampleLevel(haze_source, haze_clamp, uv + select(vec2<f32>(0.0), offset, valid), 0.0).rgb;
    return vec4<f32>(color, mask);
}

@fragment
fn haze_apply(in_vertex: HazeVertex) -> @location(0) vec4<f32> {
    let gathered = textureSampleLevel(haze_source, haze_clamp, in_vertex.position.xy * haze.inv_size.zw, 0.0);
    if (gathered.a < 0.004) { discard; }
    return gathered;
}
