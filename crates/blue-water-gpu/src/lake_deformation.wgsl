// Static smooth simplex noise from the Gerstner material's noise baker. RGB holds
// height and its world-space derivatives: one filtered lookup, no wave simulation.
@group(1) @binding(10) var jka_lake_noise: texture_2d<f32>;
@group(1) @binding(11) var jka_lake_repeat: sampler;

// Height and smooth slope share one world-space field across ALL clipmap levels.
// In particular the height must never depend on a vertex's level or morph weight:
// two rings meeting at the same XZ must get exactly the same Y.
fn jka_lake_surface(xz: vec2<f32>) -> vec3<f32> {
    if lake.deformation.y < 0.5 || lake.deformation.x <= 0.0 { return vec3<f32>(0.0); }
    let delta = vec3<f32>(xz.x - camera.position.x, -camera.position.y, xz.y - camera.position.z);
    let range = length(delta);
    let t = clamp((range - 12.0) / 20.0, 0.0, 1.0);
    let kept = 1.0 - t * t * (3.0 - 2.0 * t);
    if kept <= 0.0 { return vec3<f32>(0.0); }
    let fade_slope = (-6.0 * t * (1.0 - t) / 20.0) * delta.xz / max(range, 1e-4);
    // Two moving layers change the shape as well as its position. The default
    // evolves on a seconds timescale, rather than creeping through a static field.
    let time = camera.time * lake.deformation.w;
    let first = textureSampleLevel(jka_lake_noise, jka_lake_repeat,
        xz / 32.0 + time * lake.wind.xy * 0.025, 0.0).rgb;
    let second = textureSampleLevel(jka_lake_noise, jka_lake_repeat,
        vec2<f32>(xz.y, -xz.x) / 40.0 + time * vec2<f32>(-0.019, 0.031), 0.0).rgb;
    let field = first * 0.6 + vec3<f32>(second.x, -second.z * 0.8, second.y * 0.8) * 0.4;
    let uv = (xz - lake.mask.xy) * lake.mask.zw;
    let shore = textureSampleLevel(lake_mask, mask_sampler, uv, 0.0).r;
    let gain = lake.deformation.x * shore;
    return vec3<f32>(field.x * kept, field.yz * kept + field.x * fade_slope) * gain;
}

@group(1) @binding(12) var jka_lake_planar: texture_2d_array<f32>;
@group(1) @binding(13) var jka_lake_planar_sampler: sampler;

fn jka_lake_reflection(world: vec3<f32>, up: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    if lake.tint.w <= 0.0 { return fallback; }
    if lake.reflection.y > 0.5 && lake.reflection.y < 1.5 {
        return jka_lake_ssr(world, up, fallback);
    }
    if lake.reflection.x < 0.0 { return fallback; }
    // The reflected view projects exactly onto the undeformed lake plane.
    // Small normal-driven offsets make the shared planar image follow the ripples.
    let point = world.xz + up.xz * 0.2;
    let projected = camera.view_projection * vec4<f32>(point.x, 0.0, point.y, 1.0);
    if projected.w <= 0.0 { return fallback; }
    let uv = projected.xy / projected.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) { return fallback; }
    return textureSampleLevel(jka_lake_planar, jka_lake_planar_sampler, uv, i32(lake.reflection.x), 0.0).rgb;
}

// Lake-local SSR reads the opaque copy taken before the water draw. It never
// enables the global SSR post-process or spends a planar reflection slot.
fn jka_lake_ssr(world: vec3<f32>, normal: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let reflected = reflect(normalize(world - camera.position), normal);
    let origin = world + normal * 0.08;
    let forward = -(camera.view * vec4<f32>(reflected, 0.0)).z >= 0.0;
    let count = clamp(u32(lake.reflection.z), 8u, 32u);
    let reach = max(lake.reflection.w, 1.0);
    var previous = 0.0;
    var previous_delta = -1.0;
    for (var i = 1u; i <= count; i += 1u) {
        let fraction = f32(i) / f32(count);
        // Spend most samples near the surface: nearby geometry otherwise falls
        // between widely spaced steps, especially on almost flat water.
        let near_reach = min(reach, 8.0);
        let near_fraction = min(fraction * 1.5, 1.0);
        let far_fraction = max(fraction * 3.0 - 2.0, 0.0);
        let distance = 0.04 + near_reach * near_fraction * near_fraction
            + (reach - near_reach) * far_fraction;
        let point = origin + reflected * distance;
        if point.y < -0.1 { break; }
        let clip = camera.view_projection * vec4<f32>(point, 1.0);
        if clip.w <= 0.0 { break; }
        let uv = clip.xy / clip.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
        if any(uv < vec2<f32>(0.001)) || any(uv > vec2<f32>(0.999)) { break; }
        let pixel = clamp(vec2<i32>(uv * camera.viewport.xy), vec2<i32>(0), vec2<i32>(camera.viewport.xy) - 1);
        let depth = textureLoad(scene_depth, pixel, 0);
        let ray_depth = -(camera.view * vec4<f32>(point, 1.0)).z;
        let delta = ray_depth - view_depth(depth);
        let crossing = select(delta <= 0.0 && previous_delta > 0.0,
            delta >= 0.0 && previous_delta < 0.0, forward);
        if depth > 0.0 && crossing {
            var lo = previous;
            var hi = distance;
            var hit_uv = uv;
            var hit_delta = abs(delta);
            for (var refine = 0u; refine < 5u; refine += 1u) {
                let mid = (lo + hi) * 0.5;
                let p = origin + reflected * mid;
                let c = camera.view_projection * vec4<f32>(p, 1.0);
                let u = c.xy / c.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
                let xy = clamp(vec2<i32>(u * camera.viewport.xy), vec2<i32>(0), vec2<i32>(camera.viewport.xy) - 1);
                let d = textureLoad(scene_depth, xy, 0);
                let difference = -(camera.view * vec4<f32>(p, 1.0)).z - view_depth(d);
                if d > 0.0 && select(difference <= 0.0, difference >= 0.0, forward) {
                    hi = mid; hit_uv = u; hit_delta = abs(difference);
                }
                else { lo = mid; }
            }
            // Reject disocclusion jumps instead of painting stretched foreground.
            if hit_delta < 0.15 + distance * 0.03 {
                let xy = clamp(vec2<i32>(hit_uv * camera.viewport.xy), vec2<i32>(0), vec2<i32>(camera.viewport.xy) - 1);
                let edge = min(min(hit_uv.x, 1.0-hit_uv.x), min(hit_uv.y, 1.0-hit_uv.y));
                let confidence = smoothstep(0.0, 0.06, edge) * (1.0 - smoothstep(reach * 0.8, reach, distance));
                return mix(fallback, textureLoad(scene_color, xy, 0).rgb, confidence);
            }
            break;
        }
        previous = distance;
        previous_delta = select(delta, -1.0, depth <= 0.0);
    }
    return fallback;
}

fn jka_lake_project(world: vec3<f32>, horizon: f32) -> vec4<f32> {
    var clip = camera.view_projection * vec4<f32>(world, 1.0);
    // Continue over sky beyond the finite scene's far clip. Nearby geometry
    // retains its ordinary depth and still occludes the extension.
    if (horizon > 0.5 || lake.offset.w > 0.5) && clip.w > 0.0 {
        clip.z = max(clip.z, clip.w * 0.000001);
    }
    return clip;
}

fn jka_lake_world(input: VertexIn) -> vec3<f32> {
    // The disabled path is retained for standalone material fixtures.
    if lake.deformation.y < 0.5 { return input.position; }
    let level = u32(input.lake.z);
    let frame = lake.frames[level];
    if input.lake.w > 0.5 {
        return vec3<f32>(frame.x + input.position.x, 0.0, frame.y + input.position.z);
    }
    let finer = lake.frames[max(level, 1u) - 1u];
    let lattice = input.position.xz * frame.w;
    let inner = max(abs(lattice.x), abs(lattice.y)) <= lake.clipmap.x * 0.25 && level > 0u;
    let index = lattice + select(vec2<f32>(0.0), (finer.xy - frame.xy) * frame.w, inner);
    let snapped = frame.xy + index * frame.z;
    let reach = max(abs(snapped.x - lake.clipmap.y), abs(snapped.y - lake.clipmap.z))
        / (frame.z * lake.clipmap.x * 0.5);
    let receding = select(0.0, lake.clipmap.w, level == u32(lake.deformation.z));
    let morph = max(smoothstep(0.6, 0.85, reach), receding);
    let odd = index - floor(index * 0.5) * 2.0;
    let grid = snapped - odd * morph * frame.z;
    return vec3<f32>(grid.x, jka_lake_surface(grid).x, grid.y);
}

@fragment
fn jka_lake_wireframe(input: VertexOut) -> @location(0) vec4<f32> {
    if !jka_water_contains(input.world.xz) { discard; }
    return vec4<f32>(0.1, 0.85, 1.0, 1.0);
}
