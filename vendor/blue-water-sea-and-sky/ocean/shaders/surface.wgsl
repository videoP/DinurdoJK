// The ocean surface: ports the web game's `ocean/surface/OceanGeometry.ts` (the clipmap's snapping and geomorph in the vertex
// stage), the `displacement` and `surface` reads of the web game's `ocean/waves/field.ts` with its ripples, `gustGain` of
// the web game's `ocean/waves/gusts.ts`, `breakingGroup` of `ocean/waves/groups.ts`, the physical shading graph of
// the web game's `ocean/surface/OceanSurfaceMaterial.ts` (`realism.reflections` and `realism.waterColor` on, as the game
// defaults) and the scene fog of the web game's `ocean/screen/fog.ts`.
//
// The wakes (the web's `WakeSampler` with the realistic trail: wake field, bow waves, trail foam, bubbles and slick,
// torpedo tracks, hull contact foam, and the shelter hull sides give the water beside them, surface/shelter.ts) come from
// naval_ocean::wake (shaders/wake.wgsl), whose bindings join group 1. Ship shadows on the sea are the renderer core's
// (naval::shadow's `water_sun_shadow`, the web game's `WaterShadows.ts`), the clouds' shadow naval_clouds::shadow's; the two
// multiply into the web's `shadow` node. Hulls and land the mirrored ray meets on screen replace the sky
// (naval_ocean::screen_reflection, the web game's `ocean/screen/reflections.ts`) on the tiers that trace them, its table of the
// depth copy's tiles at binding 15. The wave field's
// reads and the sea around hulls (the web game's `ocean/waves/hullSea.ts`) are naval_ocean::waves (group 2).

// Bind groups follow the renderer core's scene pipelines (native/docs/render-core.md): 0 the camera (naval::camera)
// with the sun's shadow maps and the motion block, 1 the surface's own, 2 the wave field's, 3 the sky environment.

#import naval::camera::{camera, world_to_clip, curved}
#import naval::shadow::water_sun_shadow
#import naval::motion::motion_response
#import naval_clouds::shadow::{CloudShadow, cloud_shadow}
#import naval_ocean::noise::value_noise
#import naval_ocean::sky_environment::{SkyEnvironment, env_radiance, env_irradiance}
#import naval_ocean::reflection::{TRACE_BELOW, footprint_variance, wind_variances, reflection_lobe, mean_fresnel, sky_lobe, sun_glitter, glitter_density, glitter_glints, soft_knee, luminance_ceiling}
#import naval_ocean::screen_reflection::screen_space_reflection
#import naval_ocean::water_body::{BACKSCATTER, REFRACTION_SQUARED, subsurface_reflectance, upwelling, column_transmittance, water_body}
#import naval_ocean::water_properties::{WaterField, sample_water, water_beam, water_pigment}
#import naval_ocean::foam::{FOAM_GLOSS, STREAK_TEAR_STRETCH, foam_patterns, foam_footprint, foam_tear, torn_cover, foam_share, whitecap_foam, windrow_opacity, billows, thin_foam, foam_radiance, aeration_with_wake, aerated_reflectance, bubble_cloud, foam_over, churned_water, gale_streaks}
#import naval_ocean::wake::{wake_height, wake_surface, hull_shelter, shelter_light, shelter_reflect, wake_sampler}
#import naval_ocean::waves::{waves, field_displacement, field_derivatives, field_extras, field_sampler, field_uv, scatter, copies_grad, mix_zero, mix_mean, mix_variance, displacement, HullSurface, hull_surface, hull_sea_active, breaking_group, Shore, shore_at, shore_whitecaps, surf}
#import naval_ocean::underwater_ray::water_path_light
#import naval::aerial::aerial_apply
#import naval::lights::{PointLights, point_light_count}
#import naval_ocean::point_lights::{LightShares, SeaLights, light_frame, light_dither, sea_point_light}
#import naval_ocean::rain::rain_surface
#import naval_ocean::spray_haze::{spray_depth, spray_light}

struct Surface {
    // Per clipmap level: xy snapped centre, z cell (m), w 1 / cell (`geometry::MAX_CLIPMAP_LEVELS`).
    frames: array<vec4<f32>, 10>,
    // x: segments per level edge, y: levels, zw: the clipmap's centre (x, z): the eye, or what a magnified view is
    // aimed at (`geometry::focus`).
    clipmap: vec4<f32>,
    // x: the finest level drawn, y: how far it has morphed onto the next (`geometry::Clipmap::detail`); a magnified
    // view's fan (`geometry::Fan`): z its first row, counted from its knee, w its squeeze (what one over a row's range
    // falls by each row beyond the knee, 1 / m).
    detail: vec4<f32>,
    // The fan: x its first column's bearing (radians from +z toward +x), y its step (radians between columns, and the
    // log of a row's range over the one before within the knee), z its knee's range and w its last row's (m).
    fan: vec4<f32>,
    absorption: vec4<f32>,
    transmission: vec4<f32>,
    // rgb colour, a opacity.
    crest_foam: vec4<f32>,
    surface_foam: vec4<f32>,
    shoreline_foam: vec4<f32>,
    // x: crest foam's wind stretch, y: windrow coverage, z: the band mask's mean.
    foam: vec4<f32>,
    // rgb: fog colour, a: power.
    fog_color: vec4<f32>,
    // x: start, y: end, z: sky blend distance, w: 1 with fog.
    fog: vec4<f32>,
    // x: water shadows (the Graphics menu's 0 off, 1 low, 2 medium, 3 high), yzw unused.
    shadows: vec4<f32>,
    // Screen-space reflections: x 1 while traced (Graphics → Reflections `scene`), y the longest ray (m).
    reflections: vec4<f32>,
    // rgb the water's pigment (the underside's deep), w 1 while the underwater probe is this frame's.
    pigment: vec4<f32>,
    // x: rain (the weather's precipitation, 0 dry … 1 a downpour), yzw: the gale's spray haze (naval_ocean::spray_haze:
    // extinction at its base, scale height, base height).
    weather: vec4<f32>,
    // The scene's point lights on the sea (naval_ocean::point_lights): flashes, fires and lightning's stroke.
    lights: PointLights,
    water: WaterField,
}

@group(1) @binding(0) var<uniform> surface: Surface;
@group(1) @binding(1) var foam_map: texture_2d<f32>;
@group(1) @binding(2) var foam_sampler: sampler;
// The opaque scene behind the sea (colour and reversed depth), for straight-through transmission and shoreline foam.
@group(1) @binding(3) var scene_color: texture_2d<f32>;
@group(1) @binding(4) var scene_depth: texture_depth_2d;
// The clouds' shadow (naval_clouds::CloudShadow): the map, its linear repeat sampler and its uniforms (disabled: 1).
@group(1) @binding(11) var cloud_map: texture_2d<f32>;
@group(1) @binding(12) var cloud_sampler: sampler;
@group(1) @binding(13) var<uniform> cloud_params: CloudShadow;
// The underwater probe (naval_ocean::underwater): g at texel (0, 0) is the camera's depth below the waves.
@group(1) @binding(14) var underwater_probe: texture_2d<f32>;

@group(3) @binding(0) var sky_atlas: texture_2d<f32>;
@group(3) @binding(1) var sky_sampler: sampler;
@group(3) @binding(2) var<uniform> sky_env: SkyEnvironment;

const CASCADES: i32 = #CASCADES;
// Screen-space reflection steps along the clipped ray (the tier's `reflectionSteps`; 0 traces nothing).
const REFLECTION_STEPS: i32 = #REFLECTION_STEPS;
const ANISOTROPY: f32 = 16.0;
// A finer cascade's foam shows where the coarser cascades' compression, in its own deviations, passes these.
const GATE_NONE: f32 = -1.0;
const GATE_FULL: f32 = 0.75;
const FOAM_JACOBIAN_MIN: f32 = 0.35;
const FOAM_JACOBIAN_MAX: f32 = 2.5;
// The bubble cloud is read over an ellipse this many metres along the crests and across them.
const CREST_SPREAD: f32 = 4.0;
const CROSS_SPREAD: f32 = 1.5;
const MIN_JACOBIAN: f32 = 0.1;
const SETTLE_START: f32 = 0.4;
const SETTLE_FULL: f32 = 0.1;
// Gusts: three octaves (cell along, across, weight, drift share) and the sum's deviation.
const GUST_SUM_DEVIATION: f32 = 0.2759270;
const GUST_TOTAL_WEIGHT: f32 = 1.6625;
const GUST_FADE_START: f32 = 0.12;
const GUST_FADE_END: f32 = 0.35;
// Water shading.
const GLITTER_KNEE: f32 = 0.25;
// The brightest one glint's pixel may be, in the same units: four times the knee the glitter's mean takes.
const GLINT_PEAK: f32 = 1.0;
// The lowest light the scene's lamps' glints are kneed at: a full moon's at the sea (`MOONLIGHT` 0.8 × its tint's blue
// 0.871 × `moon_grade::TRANSMITTANCE` 0.8 × `sky_grade::GAIN` 1.4, as `SkyEnvironment::light_intensity` has it). With
// the sun and the moon down the sky's light is zero, and a knee at it would leave a fire's or a flash's glints none.
const LAMP_KNEE_FLOOR: f32 = 0.78;
// A lamp is marched toward (`light_seen`) only where it adds more than the larger of these to a pixel (luminance): the
// least radiance that shows on the darkest sea (under 3 of 255 levels with the eye at its most open at night), and a
// share of the light the pixel has without the lamps, under which its shadow would not show.
const LAMP_SEEN_LEAST: f32 = 1e-4;
const LAMP_SEEN_SHARE: f32 = 0.03;
// `CLOUD_SHADOW_FLOOR`: the beam a cloud's shadow keeps (none: the light under a cloud is the sky's).
const CLOUD_SHADOW_FLOOR: f32 = 0.0;
const SHORE_DEPTH: f32 = 0.8;
// The gale streaks show in patches this long along the wind and wide across it (m).
const STREAK_PATCH_ALONG: f32 = 170.0;
const STREAK_PATCH_ACROSS: f32 = 55.0;
const SHORE_EDGE: f32 = 0.3;
// The surf's lace edge, as the shoreline's.
const SURF_EDGE: f32 = 0.3;
// The share of the water the foam covers at the contact, and how much finer than the whitecaps' its tear is laid.
const SHORE_COVER: f32 = 0.4;
const SHORE_TEAR_SCALE: f32 = 4.0;
// How much of the shoreline foam a hull's side takes away (its contact foam is the hull's).
const SHORE_HULL: f32 = 0.85;

struct VertexIn {
    // Lattice position (x, z) in the level's frame, metres at unit cell.
    @location(0) position: vec2<f32>,
    // x: clipmap level (levels for the horizon ring), y: the horizon ring's vertex spacing (m). The fan's block
    // (`geometry::build_fan`) has level -1 and its (column, row) for position, a block of rows to an instance.
    @location(1) level: vec2<f32>,
    @builtin(instance_index) instance: u32,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    // Undisplaced world xz: where the waves are read.
    @location(0) grid: vec2<f32>,
    // Displaced world position.
    @location(1) world: vec3<f32>,
}

// GLSL's mod, as three's TSL `mod` compiles: the result takes the divisor's sign.
fn glsl_mod(x: vec2<f32>, y: f32) -> vec2<f32> {
    return x - y * floor(x / y);
}

// The skirt's rows (`naval_ocean::geometry::SKIRT_ROWS`), and the share of the far plane water beyond it is drawn at.
const SKIRT_ROWS: f32 = 6.0;
const BEYOND_DEPTH: f32 = 0.995;
// Metres of water under the surface where nothing stands behind it.
const OPEN_DEEP: f32 = 1.0e4;
// A crest this high (m) past the sea's horizon still shows over it: the water is drawn that far beyond the horizon.
const CREST_BEYOND: f32 = 20.0;
// Rows of cells in the fan's block (`naval_ocean::geometry::FAN_BLOCK_ROWS`).
const FAN_BLOCK_ROWS: f32 = 32.0;

@vertex
fn vertex(input: VertexIn) -> VertexOut {
    let levels = i32(surface.clipmap.y);
    let segments = surface.clipmap.x;
    let fan = input.level.x < 0.0;
    let level = i32(clamp(input.level.x, 0.0, f32(levels - 1)));
    let horizon = input.level.x > f32(levels) - 0.5;
    let frame = surface.frames[level];
    let finer = surface.frames[max(level - 1, 0)];
    let cell = frame.z;
    // Multiplying by the exact reciprocal keeps the lattice arithmetic exact.
    let lattice = input.position * frame.w;
    // A ring's inner edge sits on the finer level's outer edge, wherever that level snapped.
    let inner = max(abs(lattice.x), abs(lattice.y)) <= segments / 4.0 && level > 0 && !horizon;
    let index = lattice + select(vec2<f32>(0.0), (finer.xy - frame.xy) * frame.w, inner);
    let snapped = frame.xy + index * cell;
    // Geomorph by true distance from the centre; the outer row is always fully morphed. The finest level drawn also
    // morphs as a whole as the view draws back, until it is the middle of the next and that takes over.
    let reach = max(abs(snapped.x - surface.clipmap.z), abs(snapped.y - surface.clipmap.w)) / (cell * (segments / 2.0));
    let receding = select(0.0, surface.detail.y, level == i32(surface.detail.x));
    let morph = select(max(smoothstep(0.6, 0.85, reach), receding), 0.0, horizon);
    var grid = snapped - glsl_mod(index, 2.0) * morph * cell;
    // The skirt beyond the ring (`geometry.rs`): from an eye high enough to see past the ring, its rows run on from the
    // ring to the sea's horizon, where the Earth's drop equals the eye's height (`camera::curved`), each a share of
    // the way in equal ratios. Lower, the horizon lies within the ring and the skirt stays on the ring's circle.
    let skirt = input.level.x - f32(levels);
    let across = grid - camera.position.xz;
    let range = length(across);
    if skirt > 0.5 && camera.curvature > 0.0 {
        let sea_horizon = sqrt(max(camera.position.y, 0.0) / camera.curvature);
        if sea_horizon > range && range > 1.0 {
            // The first row stands where the water reaches the far plane, so the water within it keeps its own depth
            // (a hull out there stands in it as she does nearer); the rest run on to the horizon in equal ratios.
            let edge = BEYOND_DEPTH * camera.far;
            let first = clamp(sqrt(max(edge * edge - camera.position.y * camera.position.y, 0.0)), range, sea_horizon);
            let reach = first * pow(sea_horizon / first, (skirt - 1.0) / (SKIRT_ROWS - 1.0));
            grid = camera.position.xz + across * (reach / range);
        }
    }
    // Water past the sea's horizon is hidden by the water before it: the ring's rows beyond (and the skirt's left on the
    // ring's circle) close up where a crest past the horizon would still show, so a view whose far plane lies hundreds of
    // kilometres off (the land beyond a theatre window) shades no water only to lose it to the depth test. A closed-up
    // vertex takes the spacing the ring has there (`geometry.rs`: 4 r / segments), the same for every row closed onto
    // it, so their waves are read alike and the triangles between have no area.
    var spacing = select(cell * (morph + 1.0), input.level.y, horizon);
    if horizon && camera.curvature > 0.0 {
        let seen = sqrt(max(camera.position.y, 0.0) / camera.curvature) + sqrt(CREST_BEYOND / camera.curvature);
        let out = grid - camera.position.xz;
        let reach = length(out);
        if reach > seen {
            grid = camera.position.xz + out * (seen / reach);
            spacing = 4.0 * seen / segments;
        }
    }
    if fan {
        // A magnified view's mesh (`geometry::fan`): a polar lattice about the eye, columns a step of bearing apart.
        // Within its knee each row is e^step times the range of the one before, so a cell is square and the step
        // times its range across; beyond, one over a row's range falls by the squeeze each row, so the rows stand
        // evenly down the screen. The waves are read at the longer of a cell's sides, as a level's are at its cell.
        // The last row stands where the fan ends, as do the rows a draw's last block holds beyond it.
        let row = surface.detail.z + f32(input.instance) * FAN_BLOCK_ROWS + input.position.y;
        let bearing = surface.fan.x + input.position.x * surface.fan.y;
        var reach = surface.fan.z * exp(min(row, 0.0) * surface.fan.y);
        if row > 0.0 {
            reach = 1.0 / max(1.0 / surface.fan.z - surface.detail.w * row, 1e-9);
        }
        reach = min(reach, surface.fan.w);
        grid = camera.position.xz + vec2<f32>(sin(bearing), cos(bearing)) * reach;
        spacing = reach * max(surface.fan.y, surface.detail.w * reach);
    }
    let offset = displacement(grid, spacing);
    var world = vec3<f32>(grid.x + offset.x, offset.y + wake_height(grid), grid.y + offset.z);
    // The JKA local-tile mesh encodes a coarse-parent stride and transition
    // weight in the otherwise-unused (non-horizon) vertex spacing channel.
    // Reconstruct the *displaced* coarse triangle on the GPU, then move an
    // inserted fine vertex onto it before the fine triangles appear. Reverse
    // the interpolation before collapsing fine indices. Clipmap ring geomorph
    // above remains unchanged, and tile seams share identical parent samples.
    if !horizon && !fan && input.level.y >= 1028.0 {
        let packed = input.level.y - 1024.0;
        let parent_step = floor(packed * 0.5);
        let coarse = clamp(packed - parent_step * 2.0, 0.0, 1.0);
        let base = floor((lattice + segments * 0.5) / parent_step) * parent_step
            - segments * 0.5;
        let uv = clamp((lattice - base) / parent_step, vec2<f32>(0.0), vec2<f32>(1.0));
        var a = vec2<f32>(0.0);
        var b = vec2<f32>(0.0);
        var wc = 0.0;
        var wa = 0.0;
        var wb = 0.0;
        if uv.y <= uv.x && uv.y <= 1.0 - uv.x {
            a = base; b = base + vec2<f32>(parent_step, 0.0);
            wc = 2.0 * uv.y; wa = 1.0 - uv.x - uv.y; wb = uv.x - uv.y;
        } else if uv.y >= uv.x && uv.y >= 1.0 - uv.x {
            a = base + vec2<f32>(parent_step, parent_step);
            b = base + vec2<f32>(0.0, parent_step);
            wc = 2.0 * (1.0 - uv.y);
            wa = uv.x + uv.y - 1.0; wb = uv.y - uv.x;
        } else if uv.x <= uv.y {
            a = base + vec2<f32>(0.0, parent_step); b = base;
            wc = 2.0 * uv.x; wa = uv.y - uv.x; wb = 1.0 - uv.x - uv.y;
        } else {
            a = base + vec2<f32>(parent_step, 0.0);
            b = base + vec2<f32>(parent_step, parent_step);
            wc = 2.0 * (1.0 - uv.x);
            wa = uv.x - uv.y; wb = uv.x + uv.y - 1.0;
        }
        // Use the same base-grid snap and native ring geomorph for each parent
        // vertex. Sampling displacement at each *parent* point avoids the
        // pop from simply sliding an already-displaced fine vertex sideways.
        let shift = select(vec2<f32>(0.0), (finer.xy - frame.xy) * frame.w, inner);
        let ia = a + shift;
        let ib = b + shift;
        let ic = base + vec2<f32>(parent_step * 0.5) + shift;
        let pa = frame.xy + ia * cell - glsl_mod(ia, 2.0) * morph * cell;
        let pb = frame.xy + ib * cell - glsl_mod(ib, 2.0) * morph * cell;
        let pc = frame.xy + ic * cell - glsl_mod(ic, 2.0) * morph * cell;
        let da = displacement(pa, spacing);
        let db = displacement(pb, spacing);
        let dc = displacement(pc, spacing);
        let coarse_world = (vec3<f32>(pa.x + da.x, da.y + wake_height(pa), pa.y + da.z) * wa
            + vec3<f32>(pb.x + db.x, db.y + wake_height(pb), pb.y + db.z) * wb
            + vec3<f32>(pc.x + dc.x, dc.y + wake_height(pc), pc.y + dc.z) * wc);
        world = mix(world, coarse_world, coarse);
    }
    var output: VertexOut;
    output.clip = world_to_clip(world);
    if (horizon || fan) && camera.curvature > 0.0 {
        // Water beyond the far plane (the skirt's, and the ring's own far side when the clipmap is centred ahead of
        // the eye) is drawn along its own line of sight just inside it, each skirt row behind the one before: it
        // covers the pixels it would, behind what the far plane holds but for the last ring step before it.
        let line = curved(world) - camera.position;
        let limit = camera.far * (BEYOND_DEPTH + (0.9995 - BEYOND_DEPTH) * max(skirt, 0.0) / SKIRT_ROWS);
        if length(line) > limit {
            output.clip = camera.view_projection * vec4<f32>(camera.position + line * (limit / length(line)), 1.0);
        }
    }
    output.grid = grid;
    output.world = world;
    return output;
}

// The gain on the slope variance of waves shorter than the gust wavelength at grid point `xz`, for a pixel whose
// footprint reaches `length` metres along the view: lognormal, mean 1.
fn gust_gain(xz: vec2<f32>, length: f32) -> f32 {
    var octaves = array<vec4<f32>, 3>(
        vec4<f32>(4200.0, 2600.0, 0.6, 0.3),
        vec4<f32>(1200.0, 750.0, 1.0, 0.45),
        vec4<f32>(400.0, 250.0, 0.55, 0.6),
    );
    let wind = waves.wind.xy;
    let along = dot(xz, wind);
    let across = dot(xz, vec2<f32>(-wind.y, wind.x));
    let size = waves.gusts.z;
    let travel = waves.wind.z * waves.gusts.y * waves.gusts.w;
    var gust = 0.0;
    var variance = 0.0;
    for (var i = 0; i < 3; i++) {
        let octave = octaves[i];
        let width = size * octave.y;
        let shown = (1.0 - smoothstep(width * GUST_FADE_START, width * GUST_FADE_END, length)) * octave.z;
        let cell = vec2<f32>((along - travel * octave.w) / (size * octave.x), across / width);
        gust += (value_noise(cell, 301u + u32(i)) - 0.5) * shown;
        variance += shown * shown;
    }
    gust /= GUST_SUM_DEVIATION;
    variance /= GUST_TOTAL_WEIGHT;
    let strength = waves.gusts.x;
    return exp(gust * strength - variance * strength * strength * 0.5);
}

struct WaveSample {
    // World-space slope of the displaced surface (∂y/∂x, ∂y/∂z), mip-filtered.
    slope: vec2<f32>,
    jacobian: f32,
    foam: f32,
    fresh: f32,
    // The foam read over the bubble cloud's ellipse: the whitecaps' outline, softened.
    foam_soft: f32,
    whitecap_share: f32,
    bubbles: f32,
    slope_variance: f32,
    unresolved_variance: f32,
    // The surf's cover and its fresh share (`waves::surf`).
    breakers: vec2<f32>,
}

// The waves at grid point `xz` (`surface()` in field.ts), a wake's slick stilling `calm` of the short waves; near hulls the
// first cascade's long waves give way to the sea they ride, evaluated where the point is drawn (`at`); near a coast what
// its fetch and depth leave of each cascade (`waves::shore_at`), and its surf. Takes screen-space derivatives.
fn wave_surface(xz: vec2<f32>, calm: f32, at: vec2<f32>) -> WaveSample {
    // The pixel's footprint on the grid (m), as the anisotropic filter resolves it.
    let ddx = dpdx(xz);
    let ddy = dpdy(xz);
    let across = length(ddx);
    let down = length(ddy);
    let footprint = max(min(across, down), max(across, down) / ANISOTROPY);
    let wide = max(across, down);
    let wind = waves.wind.xy;
    let crestward = vec2<f32>(-wind.y, wind.x);
    let grazing = smoothstep(ANISOTROPY / 2.0, ANISOTROPY, wide / max(min(across, down), 1e-6));
    let gain = gust_gain(xz, wide);
    let top = waves.info.x;
    let shore = shore_at(xz);
    // The hull sea's share (`hullSurface`), read only while a hull is coupled (a uniform branch: explicit levels).
    var hulls = HullSurface(vec2<f32>(0.0), vec3<f32>(0.0), vec2<f32>(0.0), 0.0);
    if hull_sea_active() {
        hulls = hull_surface(xz, at, footprint, shore);
    }
    var slope = vec2<f32>(0.0);
    var strain = vec3<f32>(0.0);
    var foam = 0.0;
    var fresh = 0.0;
    var foam_soft = 0.0;
    var bubbles = 0.0;
    let still = 1.0 - calm;
    var variance = waves.tail.x * still * gain;
    var unresolved = waves.tail.y * still * gain;
    for (var i = 0; i < CASCADES; i++) {
        let tile = waves.tile[i];
        let copies = scatter(xz, i);
        // A cascade finer than its coarsest texel under this pixel fades out over the last level.
        let level = log2(footprint * waves.texels[i]);
        let detail = 1.0 - smoothstep(top - 1.0, top, level);
        let read_derivatives = copies_grad(field_derivatives, copies, i, ddx / tile, ddy / tile);
        let read_extras = copies_grad(field_extras, copies, i, ddx / tile, ddy / tile);
        // A coast's share of the cascade's amplitude: its slopes and strain scale with it, its variance and the foam its
        // breaking leaves with its square.
        let kept = shore.scales[i];
        let derivatives = mix_zero(read_derivatives, copies) * kept;
        let means = mix_mean(read_extras, copies) * vec4<f32>(1.0, kept * kept, 1.0, kept * kept);
        let extras = vec4<f32>(mix_zero(read_extras, copies).x * kept, means.y, means.z, means.w);
        // A finer cascade's foam shows on the crests of the coarser ones summed so far.
        let crests = select(1.0, smoothstep(GATE_NONE, GATE_FULL, -(strain.x + strain.y) * waves.gates[i]), i > 0);
        let shown = crests * breaking_group(xz, extras.y, extras.w, i) * detail;
        foam = max(foam, extras.y * shown);
        fresh = max(fresh, extras.w * shown);
        // The bubble cloud under the crests, read drawn out along them.
        let spread = mix_mean(copies_grad(field_extras, copies, i, crestward * max(wide, CREST_SPREAD) / tile,
            wind * max(wide, CROSS_SPREAD) / tile), copies) * (kept * kept);
        let spread_shown = crests * breaking_group(xz, spread.y, spread.w, i) * detail;
        bubbles = max(bubbles, spread.w * spread_shown);
        foam_soft = max(foam_soft, spread.y * spread_shown);
        let roughened = 1.0 + (gain - 1.0) * waves.gust_shares[i];
        let stilled = 1.0 - calm * waves.slick_shares[i];
        slope += derivatives.xy * detail * stilled * sqrt(roughened);
        strain += vec3<f32>(derivatives.zw, extras.x) * detail;
        let filtered = mix_variance(read_extras, read_derivatives, copies);
        // Once the pixel's length has averaged away most of the cascade, its variance is the expectation.
        let share = waves.slope_lengths[i] / max(wide, 1e-3);
        let settled = mix(filtered, waves.slopes[i] * max(1.0 - share, 0.0), (1.0 - smoothstep(SETTLE_FULL, SETTLE_START, share)) * grazing);
        let cascade_variance = mix(waves.slopes[i], settled, detail) * stilled * roughened * kept * kept;
        variance += cascade_variance;
        unresolved += cascade_variance;
        // Near hulls the first cascade's long waves give way to the sea they ride, before finer foam follows its crests.
        if i == 0 {
            slope += hulls.slope * detail;
            strain += hulls.strain * detail;
        }
    }
    // World slope of the displaced surface through the inverse transpose of the horizontal map's Jacobian.
    let xx = strain.x + 1.0;
    let zz = strain.y + 1.0;
    let cross_strain = strain.z;
    let jacobian = xx * zz - cross_strain * cross_strain;
    let world = vec2<f32>(zz * slope.x - cross_strain * slope.y, xx * slope.y - cross_strain * slope.x) / max(jacobian, MIN_JACOBIAN);
    // Waves shorter than the finest cascade: its slopes read as many times finer as its band spans.
    var ripple_slope = vec2<f32>(0.0);
    var ripple_variance = 0.0;
    var ripple_unresolved = 0.0;
    if CASCADES > 1 {
        let finest = CASCADES - 1;
        let ripple_level = max(log2(footprint * waves.tail.w), 0.0);
        let ripple_shown = 1.0 - smoothstep(top - 1.0, top, ripple_level);
        let read = textureSample(field_derivatives, field_sampler, xz / waves.tail.z + 0.5 / waves.info.z, finest);
        let drawn = -max(1.0 - ripple_level / top, 0.0) * ripple_shown;
        let kept = shore.scales[finest];
        ripple_slope = read.xy * ripple_shown * still * sqrt(gain) * kept;
        ripple_variance = min(waves.slopes[finest], waves.tail.x) * drawn * still * gain * kept * kept;
        ripple_unresolved = min(waves.slopes[finest], waves.tail.y) * drawn * still * gain * kept * kept;
    }
    // The surf: the breakers' white water where the waves break on a shoaling shore, and the bubbles they drive under.
    let surf_cover = surf(xz, shore, wide, footprint);
    bubbles = max(bubbles, surf_cover.x);
    // Foam rides the water: per area of sea it is as dense as the surface is compressed.
    let gathered = 1.0 / clamp(jacobian, FOAM_JACOBIAN_MIN, FOAM_JACOBIAN_MAX);
    return WaveSample(
        world + ripple_slope + hulls.world,
        jacobian,
        foam * gathered,
        fresh * gathered,
        foam_soft * gathered,
        waves.info.w * shore_whitecaps(shore),
        bubbles,
        variance + ripple_variance + hulls.variance,
        unresolved + ripple_unresolved + hulls.variance,
        surf_cover,
    );
}

// Light scattered through thin crests toward an observer looking into the sun.
fn crest_transmission(view: vec3<f32>, sun: vec3<f32>, radiance: vec3<f32>, tint: vec3<f32>, crest: f32) -> vec3<f32> {
    let flat_view = normalize(vec3<f32>(view.x, 0.0, view.z) + vec3<f32>(1e-6, 0.0, 0.0));
    let flat_sun = normalize(vec3<f32>(sun.x, 0.0, sun.z) + vec3<f32>(1e-6, 0.0, 0.0));
    let against = max(dot(-flat_view, flat_sun), 0.0);
    return tint * radiance * (pow(against, 4.0) * crest * 0.04);
}

// The air between the eye and the sea at `world` (`naval::aerial`: the atmosphere and the weather's haze, as every
// material takes it, so the far sea meets the dome's hazed horizon), while the scene's fog is on.
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    if surface.fog.w < 0.5 {
        return color;
    }
    return aerial_apply(color, world);
}

// Seen from below (`underside`): Snell's window shows the sky refracted through the surface, its radiance raised by
// water's n² on the way in; outside it, and in the window's own reflection, the surface mirrors the water below it by
// total internal reflection: the deep's light along the mirrored ray, as the underwater view lights the water about the
// camera (`water_path_light`), so beyond the window the underside is the deep seen in a mirror. The underwater view then
// takes the water between the surface and the camera.
const WATER_F0: f32 = 0.02;
const WATER_IOR: f32 = 1.333;
// Radiance crossing into water gains its refractive index squared (the basic radiance theorem).
const WINDOW_GAIN: f32 = WATER_IOR * WATER_IOR;
// Metres of water the mirrored ray crosses: the open deep.
const MIRRORED_DEEP: f32 = 1.0e4;
// Foam seen from below: the share of the light it shows from above that it passes down, how far that light takes the
// water's transmission tint, and how much of the underside dense foam (whitecaps, churned water), thin foam (windrows,
// a shore's) and the bubbles under them cover. The bubbles are the soft cloud a wake reads by from below; the foam's
// own lace is drawn through them at part strength, so the surface's hard edges do not print on the underside.
const FOAM_BELOW_LIGHT: f32 = 0.4;
const FOAM_BELOW_TINT: f32 = 0.4;
const FOAM_BELOW_DENSE: f32 = 0.5;
const FOAM_BELOW_THIN: f32 = 0.25;
const FOAM_BELOW_BUBBLES: f32 = 0.75;

fn water_fresnel(cosine: f32) -> f32 {
    return WATER_F0 + (1.0 - WATER_F0) * pow(1.0 - max(cosine, 1e-4), 5.0);
}

// `skyReflection`: the environment along a direction, the horizon's for anything below it.
fn sky_reflection(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    return env_radiance(sky_atlas, sky_sampler, sky_env, normalize(vec3<f32>(direction.x, max(direction.y, 0.0), direction.z)), roughness);
}

fn underside(
    view: vec3<f32>,
    normal: vec3<f32>,
    pigment: vec3<f32>,
    absorption: vec3<f32>,
    sun_direction: vec3<f32>,
    sun_color: vec3<f32>,
) -> vec3<f32> {
    let incident = -view;
    let down = -normal;
    let refracted = refract(incident, down, WATER_IOR);
    let mirrored = water_path_light(reflect(incident, down), 0.0, MIRRORED_DEEP, pigment, absorption, sun_direction, sun_color);
    let window = mix(sky_reflection(refracted, 0.0) * WINDOW_GAIN, mirrored, water_fresnel(max(dot(refracted, normal), 0.0)));
    return select(mirrored, window, dot(refracted, refracted) > 1e-6);
}

@fragment
fn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let xz = input.grid;
    // Metres per pixel, taken in uniform control flow: the trail's eddies give way to their mean below it.
    let footprint = max(fwidth(xz.x), fwidth(xz.y));
    // The pixel's area on the water (m²), for the sun's glints (`glitter_glints`).
    let grid_dx = dpdx(xz);
    let grid_dy = dpdy(xz);
    let pixel_area = abs(grid_dx.x * grid_dy.y - grid_dx.y * grid_dy.x);
    let wake = wake_surface(xz, input.world, footprint);
    let sample = wave_surface(xz, wake.slick, input.world.xz);
    let to_camera = camera.position.xyz - input.world;
    let distance = length(to_camera);
    let view = to_camera / distance;
    // Rain rings the water: drawn where the pixel resolves them, unresolved roughness beyond (naval_ocean::rain). The
    // pixel's size on the water from the view (the mean of its extents across and along), smooth across triangles.
    let pixel_size = distance * 2.0 * camera.tan_half_fov_y * camera.viewport.w / sqrt(max(abs(view.y), 0.05));
    let rain = rain_surface(xz, waves.wind.z, surface.weather.x, pixel_size);
    // The wave slope plus the wake's own slope (`surfaceNormal`).
    let total = sample.slope - wake.normal.xz / max(wake.normal.y, 0.05);
    // The rain's rings join after the pixel's own spread of normals is taken (`footprint_variance`): their fade keeps
    // them to pixels that resolve them.
    let dry = normalize(vec3<f32>(-total.x, 1.0, -total.y));
    let up = normalize(vec3<f32>(-total.x - rain.slope.x, 1.0, -total.y - rain.slope.y));
    // The sea shades with the raw celestial light: the sun by day, the moon once it outshines it.
    let sun_direction = sky_env.light_direction;
    let sun_intensity = sky_env.light_intensity;
    let sun_radiance = sky_env.light_color * sun_intensity;
    // Ship shadows times the clouds' (WaterShadows.ts); faint under the moon, as the light that casts them.
    let shadow = water_sun_shadow(input.world, u32(surface.shadows.x)) * cloud_shadow(cloud_map, cloud_sampler, cloud_params, input.world);
    let lit = mix(1.0, shadow, clamp(sun_intensity / 2.0, 0.0, 1.0));
    // The glint and the light through crests need the beam itself: a cloud's floor of scattered light is not the
    // sun's disc, and a ship's shadow stops the beam outright.
    let beam = clamp((shadow - CLOUD_SHADOW_FLOOR) / (1.0 - CLOUD_SHADOW_FLOOR), 0.0, 1.0);
    let heading = waves.sea.z;
    let downwind = vec2<f32>(cos(heading), sin(heading));
    let wind_speed = waves.sea.y;
    // Physical reflections mirror the facets the viewer sees through their spread.
    let variances = wind_variances(sample.unresolved_variance + footprint_variance(dry), wind_speed)
        + vec2<f32>(rain.variance + wake.variance) * 0.5;
    let lobe = reflection_lobe(view, up, variances, downwind);
    let daylight = env_irradiance(sky_atlas, sky_sampler, sky_env, vec3<f32>(0.0, 1.0, 0.0));
    // Hull sides beside the water hide part of the sky it mirrors and is lit by (surface/shelter.ts).
    let contact = shelter_light(hull_shelter(input.world, lobe.direction, lobe.in_plane), daylight, sun_radiance, sun_direction);

    // The opaque scene behind this pixel, and how much water lies between it and the surface.
    let pixel = vec2<i32>(input.clip.xy);
    let depth = textureLoad(scene_depth, pixel, 0);
    let near = camera.near;
    let far = camera.far;
    let scene_view_z = -near * far / (depth * (far - near) + near);
    let ray_view_z = (camera.view * vec4<f32>(-view, 0.0)).z;
    // Nothing behind (the clear depth) is the open deep, however far the water is drawn: the skirt's lies beyond the
    // far plane, where a column measured to the far plane would read as a shore.
    let column = select(max(scene_view_z / min(ray_view_z, -1e-4) - distance, 0.0), OPEN_DEEP, depth <= 0.0);
    let optics = sample_water(surface.water, xz, surface.absorption.rgb);
    let absorption = optics.absorption;
    let backscatter = optics.backscatter;
    let attenuation = select(absorption, water_beam(optics) - backscatter, surface.water.absorption.w > 0.5);
    let pigment = select(surface.pigment.rgb, water_pigment(optics, surface.absorption.rgb, surface.pigment.rgb), surface.water.absorption.w > 0.5);
    // Light scattered back out of the water, lit by the sun and sky that cross the surface.
    let deep = upwelling(subsurface_reflectance(absorption, backscatter), sun_direction, sun_radiance, contact.irradiance, lit);
    let body = water_body(textureLoad(scene_color, pixel, 0).rgb, deep, column_transmittance(attenuation, backscatter, column, view));

    // Fresnel between the reflected sky (or ships and land, where the screen trace finds them) and the water body.
    let reflectance = mean_fresnel(dot(up, view), lobe.sigma_view);
    let sky = shelter_reflect(contact, sky_lobe(sky_atlas, sky_sampler, sky_env, lobe));
    // Hulls and islands the mirrored ray meets on screen replace the sky: the lobe's traced ray, its image smeared
    // over the lobe's in-plane spread about its centre.
    let to_view = camera.view;
    let traced = screen_space_reflection(
        scene_color,
        wake_sampler,
        scene_depth,
        (to_view * vec4<f32>(curved(input.world), 1.0)).xyz,
        (to_view * vec4<f32>(lobe.traced, 0.0)).xyz,
        surface.reflections.x > 0.5,
        surface.reflections.y,
        REFLECTION_STEPS,
        (to_view * vec4<f32>(lobe.up, 0.0)).xyz,
        lobe.in_plane,
        TRACE_BELOW,
    );
    let reflected = mix(sky, traced.color, traced.confidence);
    // How far up a wave this point sits: crests reach about Hs / 2.
    let crest = clamp(input.world.y / max(waves.sea.x, 0.1), 0.0, 1.0);
    // The glitter's mean over the pixel, drawn as the few glints that make it (`glitter_glints`) on the waves' clock.
    let glint = luminance_ceiling(
        soft_knee(sun_glitter(up, view, sun_direction, sun_radiance, variances, downwind), sun_intensity * GLITTER_KNEE)
            * glitter_glints(xz, pixel_area, glitter_density(up, view, sun_direction, variances, downwind), waves.wind.z),
        sun_intensity * GLINT_PEAK,
    );
    let direct = glint + crest_transmission(view, sun_direction, sun_radiance, surface.transmission.rgb, crest);

    // Foam: the texture laid in the wind's frame and drawn out along it by the crest foam's wind stretch.
    let stretch = surface.foam.x * 2.0 + 1.0;
    let patterns = foam_patterns(foam_map, foam_sampler, surface.foam.z, xz, heading, stretch, sun_direction);
    let lace = patterns.pattern.r;
    let foam_size = foam_footprint(xz);
    let whitecaps = whitecap_foam(sample.foam, sample.fresh, sample.foam_soft, sample.whitecap_share, patterns, foam_size, xz);
    let sky_up = env_radiance(sky_atlas, sky_sampler, sky_env, normalize(vec3<f32>(up.x, max(up.y, 0.0), up.z)), 1.0);
    let foam_light = foam_radiance(sky_up * contact.sky_share, sun_radiance, up, sun_direction, lit);
    // The realistic wake's churned water is the whitecaps' dense white water.
    let churned = churned_water(wake.foam, wake.young, patterns.pattern, patterns.blur);
    let aerated = aeration_with_wake(sample.bubbles * (1.0 - whitecaps.averaged), wake.bubbles);
    let water = mix(bubble_cloud(body, aerated, foam_light, absorption), reflected, aerated_reflectance(reflectance, aerated));
    var above = water + direct * beam;

    // From the widest and faintest to the brightest: windrows, whitecaps, shorelines.
    var windrows = windrow_opacity(surface.foam.y, patterns.lines, surface.foam.z, patterns.gather, lace, patterns.streaks,
        max(patterns.blur, patterns.streaks_blur), sample.jacobian) * surface.surface_foam.a;
    // In a gale the whitecaps' old foam drifts downwind in long streaks (the second cascade's streak channel, written
    // by resolve.wgsl), shown in long patches laid on the world so the tile's repeat does not.
    if CASCADES > 1 {
        let old = textureSample(field_displacement, field_sampler, field_uv(xz, 1), 1).w;
        let wind = waves.wind.xy;
        let patches = value_noise(vec2<f32>(dot(xz, wind) / STREAK_PATCH_ALONG, dot(xz, vec2<f32>(-wind.y, wind.x)) / STREAK_PATCH_ACROSS), 907u);
        let gathered = old * smoothstep(0.25, 0.6, patches);
        // The streaks break along their length: the foam's tear laid long along the wind (no derivatives taken).
        var tear = 0.5;
        if gathered > 1e-3 {
            let wind_frame = vec2<f32>(dot(xz, wind) / STREAK_TEAR_STRETCH, dot(xz, vec2<f32>(-wind.y, wind.x)));
            tear = foam_tear(wind_frame, foam_size.size);
        }
        let streaks = gale_streaks(gathered, patterns.streaks, lace, max(patterns.streaks_blur, patterns.blur), tear);
        windrows = max(windrows, streaks);
    }
    let white = billows(patterns.pattern.a, patterns.churn_sunward, lace, max(whitecaps.dense, churned.dense), patterns.blur,
        absorption);
    // Where the water thins against a hull or a shore, broken foam: torn along the line at metres (the foam's tear, laid
    // finer), thinning away from the contact, lace rather than a painted stripe.
    var shore_tear = 0.5;
    if column < SHORE_DEPTH {
        shore_tear = foam_tear(xz * SHORE_TEAR_SCALE, foam_size.size * SHORE_TEAR_SCALE);
    }
    let shore_thin = 1.0 - smoothstep(0.0, SHORE_DEPTH, column);
    // Against a hull (whose sides hide the sky here) the hull's own contact foam draws the white water; the thin
    // column there is only where the water meets her plating, a line in any view.
    let against_hull = smoothstep(0.03, 0.2, 1.0 - contact.sky_share) * SHORE_HULL;
    let shore_cover = shore_thin * shore_thin * SHORE_COVER * torn_cover(shore_tear) * (1.0 - against_hull);
    let shore = foam_share(shore_cover, lace, patterns.blur, SHORE_EDGE) * surface.shoreline_foam.a;
    above = foam_over(above, water, foam_light * surface.surface_foam.rgb * thin_foam(absorption), reflected, reflectance, windrows);
    above = foam_over(above, water, foam_light * surface.crest_foam.rgb * white, reflected, reflectance,
        max(whitecaps.opacity * surface.crest_foam.a, churned.opacity));
    // The surf's white water: its bores dense, the broken water left behind them thinner.
    let breakers = foam_share(sample.breakers.x, lace, patterns.blur, SURF_EDGE) * surface.crest_foam.a;
    let breakers_white = billows(patterns.pattern.a, patterns.churn_sunward, lace, sample.breakers.y, patterns.blur,
        absorption);
    above = foam_over(above, water, foam_light * surface.crest_foam.rgb * breakers_white, reflected, reflectance,
        breakers);
    above = foam_over(above, water, foam_light * surface.shoreline_foam.rgb * thin_foam(absorption), reflected, reflectance, shore);
#ifdef POINT_LIGHTS
    // The scene's point lights (flashes, fires, lightning's stroke), in the variant drawn on frames that have any
    // (`OceanSurface::point_lights`), and taken last, where few of the shader's values are live: the block costs it
    // registers. What the opaque scene hides from a light (a hull, her upperworks, another ship, the land) it does not
    // light: each is marched toward over the scene's depth (`light_seen`). Each term reaches the surface by the paths
    // the sun's does: the glints where no foam covers the water; the light in the water through the body's share of the
    // surface (`water_body`, the reflectance); the foam's light through each layer's opacity (`foam_over`). The glints
    // take the knee the sun's or moon's take.
    let lamp_count = point_light_count(surface.lights);
    if lamp_count > 0u {
        let windrow_cover = clamp(windrows, 0.0, 1.0);
        let crest_cover = clamp(max(max(whitecaps.opacity * surface.crest_foam.a, churned.opacity), breakers), 0.0, 1.0);
        let shore_cover = clamp(shore, 0.0, 1.0);
        let uncovered = (1.0 - windrow_cover) * (1.0 - crest_cover) * (1.0 - shore_cover);
        let water_share = 1.0 - aerated_reflectance(reflectance, aerated);
        let in_water = subsurface_reflectance(absorption, backscatter)
            * (1.0 - column_transmittance(attenuation, backscatter, column, view)) * (water_share * uncovered / REFRACTION_SQUARED);
        let foam_cover = (surface.surface_foam.rgb * (windrow_cover * (1.0 - crest_cover) * (1.0 - shore_cover))
            + surface.crest_foam.rgb * (white * crest_cover * (1.0 - shore_cover))
            + surface.shoreline_foam.rgb * shore_cover) * (1.0 - reflectance * FOAM_GLOSS);
        // A light adding less than this to the pixel is not marched toward: its shadow could not show. With the water's
        // shadows off (the Graphics menu; the Low preset) none is.
        let least = select(3.4e38, max(LAMP_SEEN_LEAST, LAMP_SEEN_SHARE * dot(above, vec3<f32>(0.2126, 0.7152, 0.0722))),
            surface.shadows.x > 0.5);
        let shares = LightShares(vec3<f32>(uncovered), in_water, foam_cover, least);
        var lamps = SeaLights(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
        let frame = light_frame(up, variances, downwind);
        let dither = light_dither(input.clip.xy);
        for (var i = 0u; i < lamp_count; i++) {
            let lamp = sea_point_light(surface.lights.lights[i], frame, input.world, up, view, variances, downwind,
                scene_depth, dither, shares);
            lamps.glint += lamp.glint;
            lamps.into_water += lamp.into_water;
            lamps.foam += lamp.foam;
        }
        above += soft_knee(lamps.glint, max(sun_intensity, LAMP_KNEE_FLOOR) * GLITTER_KNEE) * uncovered
            + lamps.into_water * in_water + lamps.foam * foam_cover;
    }
#endif
    // Everything above was evaluated in uniform control flow (it takes screen-space derivatives). Only back faces seen
    // from under water pay for the underside; a camera above water sees back faces only where a storm crest folds over,
    // and those keep the crest's shading; a back face above the camera still counts, for a near plane that dips under.
    let submerged = surface.pigment.w > 0.5 && textureLoad(underwater_probe, vec2<i32>(0, 0), 0).g > 0.0;
    // A gale's spray hangs low between the eye and the water (naval_ocean::spray_haze), then the distance fog. The
    // underside is seen through water, which the underwater view takes: the air's spray and fog are for the sea seen
    // from above.
    let veil = exp(-spray_depth(camera.position.xyz, input.world, vec4<f32>(surface.weather.yzw, 0.0)));
    if veil < 1.0 {
        above = mix(spray_light(daylight * (1.0 / 3.141592653589793), sun_radiance * lit, view, sun_direction), above, veil);
    }
    var color = apply_fog(above, input.world);
    if !front && (submerged || input.world.y > camera.position.y) {
        color = underside(view, up, pigment, select(absorption, water_beam(optics), surface.water.absorption.w > 0.5), sun_direction, sky_env.light_color);
        // What floats on the sea shows from below (native only): whitecaps, a wake's churned water and a torpedo's
        // track let no image through and mirror nothing; they pass the daylight they gather down as a milky glow,
        // darker than the sky in Snell's window and brighter than the deep mirrored outside it. The bubbles under
        // them glow more softly. A ship is found from below by her wake.
        let crest_cover = clamp(max(max(whitecaps.opacity * surface.crest_foam.a, churned.opacity), breakers), 0.0, 1.0);
        let afloat = 1.0 - (1.0 - crest_cover * FOAM_BELOW_DENSE) * (1.0 - clamp(max(windrows, shore), 0.0, 1.0) * FOAM_BELOW_THIN)
            * (1.0 - clamp(aerated, 0.0, 1.0) * FOAM_BELOW_BUBBLES);
        let glow = foam_light * mix(vec3<f32>(1.0), surface.transmission.rgb, FOAM_BELOW_TINT) * FOAM_BELOW_LIGHT;
        color = mix(color, glow, afloat);
    }
    return vec4<f32>(color, 1.0);
}

// The motion pass for temporal anti-aliasing: the sea takes 70 % history (without it its glints dance), and keeps the
// motion below it. The rain's rings land somewhere new every frame, as its streaks do, so as the rain thickens the sea
// takes less, none from a third of a downpour (under history the rings faded).
@fragment
fn motion(input: VertexOut) -> @location(0) vec4<f32> {
    return motion_response(mix(0.3, 1.0, smoothstep(0.0, 0.3, surface.weather.x)));
}
