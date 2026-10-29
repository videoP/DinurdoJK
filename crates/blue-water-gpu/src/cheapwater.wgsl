// Adapted from Cheap Interactive Water, Mirko Kunze (2024), MIT.
// See assets/cheapwater/LICENSE. Three rotated scrolling object-space normal
// layers with dielectric environment lighting; shared lake geometry supplies
// optional small height movement. A bounded local atlas supplies player ripple
// normals using expanding stamps, without a height solver.
@group(1) @binding(14) var cheapwater_normal: texture_2d<f32>;
@group(1) @binding(15) var cheapwater_ripples: texture_2d_array<f32>;
@group(1) @binding(17) var cheapwater_submerged_scene: texture_2d<f32>;
@group(1) @binding(18) var cheapwater_submerged_depth: texture_2d<f32>;
const CHEAPWATER_TURNS: array<vec2<f32>, 3> = array<vec2<f32>, 3>(
    vec2<f32>(-0.989992497, 0.141120008),
    vec2<f32>(0.372782736, -0.927918656),
    vec2<f32>(0.617209760, 0.786798648)
);

fn cheapwater_up(world: vec2<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    // The source Quake tcMod scroll direction is translated to render XZ on
    // upload. Apply it only for authored headings: the global/default lake
    // appearance must remain identical when source hints are disabled.
    let oriented = lake.wind.w > 0.5;
    let dir = lake.wind.xy;
    let sample_world = select(world,
        vec2<f32>(dot(world, dir), dot(world, vec2<f32>(-dir.y, dir.x))), oriented);
    let uv = sample_world * lake.appearance.w;
    let time = camera.time * lake.deformation.w;
    for (var i = 0u; i < 3u; i += 1u) {
        let turn = CHEAPWATER_TURNS[i];
        let rotated = vec2<f32>(turn.x*uv.x-turn.y*uv.y, turn.y*uv.x+turn.x*uv.y);
        let phase = 3.0 + f32(i) * 2.094395102;
        let noise = textureSample(cheapwater_normal, jka_lake_repeat,
            rotated + vec2<f32>(time*0.03, phase)).rgb * 2.0 - 1.0;
        let slope = vec2<f32>(turn.x*noise.x+turn.y*noise.y, -turn.y*noise.x+turn.x*noise.y);
        sum += vec3<f32>(slope.x, noise.z, slope.y);
    }
    if oriented {
        let local = sum.xz;
        sum.x = dir.x * local.x - dir.y * local.y;
        sum.z = dir.y * local.x + dir.x * local.y;
    }
    let relief = jka_lake_surface(world).yz;
    var surface = normalize(vec3<f32>(-relief.x, 1.0, -relief.y) + sum * lake.appearance.z);
    if lake.ripples.w >= 0.0 {
        let ripple_uv = (world-lake.ripples.xy)*lake.ripples.z;
        if all(ripple_uv > vec2<f32>(0.0)) && all(ripple_uv < vec2<f32>(1.0)) {
            let edge = min(min(ripple_uv.x,1.0-ripple_uv.x),min(ripple_uv.y,1.0-ripple_uv.y));
            let wake = textureSampleLevel(cheapwater_ripples,mask_sampler,ripple_uv,i32(lake.ripples.w),0.0).rg
                * smoothstep(0.0,0.08,edge);
            surface = normalize(surface+vec3<f32>(wake.x,0.0,wake.y));
        }
    }
    return surface;
}

// Hybrid's underside optical response, applied to the lake's SAME animated
// geometry, scrolling ripple normals and player-wake atlas. Its fog capture
// already attenuated the transmitted scene; only reflected interface light
// needs the extra interface-to-eye absorption below.
fn cheapwater_underwater(input: VertexOut, up: vec3<f32>, view: vec3<f32>) -> vec4<f32> {
    let normal = -up;
    let facing = max(dot(normal, view), 0.00002);
    var fresnel = 0.02 + 0.98 * pow(1.0 - facing, 5.0);
    // Water -> air: the animated normals deform Snell's window too.
    if 1.333 * 1.333 * (1.0 - facing * facing) > 1.0 { fresnel = 1.0; }
    let size = vec2<i32>(textureDimensions(cheapwater_submerged_scene));
    let uv = input.clip.xy / vec2<f32>(size);
    let pixel = clamp(vec2<i32>(input.clip.xy), vec2<i32>(0), size - 1);
    let receiver_depth = textureLoad(cheapwater_submerged_depth, pixel, 0).r;
    let units = max(lake.underwater.z, 0.001);
    let thickness = max(receiver_depth / units - input.depth.x, 0.0);
    let edge = smoothstep(0.0, 0.06, min(min(uv.x, uv.y), min(1.0-uv.x, 1.0-uv.y)));
    let tilt = normal - vec3<f32>(0.0, -1.0, 0.0);
    let screen_normal = (camera.view_projection * vec4<f32>(tilt, 0.0)).xy * vec2<f32>(1.0, -1.0);
    let offset = clamp(screen_normal, vec2<f32>(-1.0), vec2<f32>(1.0))
        * lake.transmission.y * edge * min(thickness / 1.6, 1.0);
    let shifted = clamp(vec2<i32>((uv + offset) * vec2<f32>(size)), vec2<i32>(0), size - 1);
    let shifted_depth = textureLoad(cheapwater_submerged_depth, shifted, 0).r;
    let ndc = (vec2<f32>(shifted) + 0.5) / vec2<f32>(size)
        * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    let near_h = camera.inverse_view_projection * vec4<f32>(ndc, 1.0, 1.0);
    let ray = normalize(near_h.xyz / near_h.w - camera.position);
    let axis = max(-(camera.view * vec4<f32>(ray, 0.0)).z, 0.001);
    let hit = camera.position + ray * (shifted_depth / units / axis);
    // The paired point read avoids smearing foreground color into refraction.
    let valid = shifted_depth >= 999999.0 || hit.y > input.world.y;
    let transmitted = textureLoad(cheapwater_submerged_scene, select(pixel, shifted, valid), 0).rgb;
    let direction = reflect(-view, normal);
    // Under a BSP roof the global atmosphere (including its solar halo) is
    // not a valid fallback for reflections seen from below the surface.
    let reflection = env_radiance(sky_atlas, sky_sampler, sky_env, direction,
        clamp(lake.appearance.x, 0.08, 1.0)) * jka_water_sky_admission(input.world.xz);
    // Same spectral ratios, exponential falloff and depth darkening as Hybrid.
    let ratios = vec3<f32>(38.0/7.5, 38.0/22.0, 1.0);
    let attenuation = exp2(-4.321928 * ratios * length(input.world-camera.position)
        / max(lake.underwater_fog.w, 0.025));
    let ambient = lake.underwater_fog.rgb * exp2(-4.321928 * ratios
        * max(input.world.y-camera.position.y, 0.0) / max(lake.underwater.y, 0.025));
    return vec4<f32>(transmitted * (1.0-fresnel)
        + (reflection * attenuation + ambient * (1.0-attenuation)) * fresnel, 1.0);
}

@fragment
fn jka_cheapwater_fragment(input: VertexOut) -> @location(0) vec4<f32> {
    if !jka_water_contains(input.world.xz) { discard; }
    let up = cheapwater_up(input.world.xz);
    let view = normalize(camera.position - input.world);
    if lake.underwater.x > 0.5 && camera.position.y < input.world.y {
        return cheapwater_underwater(input, up, view);
    }
    let facing = max(abs(dot(up, view)), 0.001);
    let roughness = clamp(lake.appearance.x, 0.08, 1.0);
    let size = vec2<i32>(textureDimensions(scene_depth));
    let pixel = clamp(vec2<i32>(input.clip.xy), vec2<i32>(0), size - 1);
    var behind = Behind(0.0, 0.0, true);
    if lake.transmission.x > 0.0 || lake.shallow_color.w > 0.0 {
        behind = behind_at(pixel, input.world, input.depth.x);
    }
    var color = lake.base_color.rgb * lake.tint.rgb;
    if lake.shallow_color.w > 0.0 {
        // Vertical bed depth, independent of view angle. The same depth fetch
        // serves transmission; sky/no bed uses the deep endpoint.
        let depth = select(max(input.world.y - behind.height, 0.0), lake.deep_color.w, behind.open);
        let blend = smoothstep(0.0, lake.deep_color.w, depth);
        let depth_color = mix(lake.shallow_color.rgb, lake.deep_color.rgb, blend);
        color = mix(color, depth_color * lake.tint.rgb, lake.shallow_color.w);
    }
    // Water is a dielectric: body pigments must never tint its reflections.
    let grazing = 1.0 - facing;
    let grazing2 = grazing * grazing;
    let fresnel = 0.02 + 0.98 * grazing2 * grazing2 * grazing;
    let light = sky_env.light_direction;
    let nl = max(dot(up, light), 0.0);
    let reflected_share = fresnel * lake.tint.w;
    var specular = vec3<f32>(0.0);
    if lake.tint.w > 0.0 {
        let direction = reflect(-view, up);
        let environment = env_radiance(sky_atlas, sky_sampler, sky_env,
            normalize(vec3<f32>(direction.x, max(direction.y, 0.0), direction.z)), roughness);
        // SSR/planar hits remain untouched. Only the global sky fallback is
        // removed when roofs block the reflected sky/sun halo.
        let admitted_sky = environment * jka_water_sky_admission(input.world.xz);
        specular = jka_lake_reflection(input.world, up, admitted_sky) * reflected_share;
        let half_vector = normalize(view + light);
        let nh = max(dot(up, half_vector), 0.0);
        let vh = max(dot(view, half_vector), 0.0);
        let a2 = roughness * roughness * roughness * roughness;
        let denominator = nh*nh*(a2-1.0)+1.0;
        let distribution = a2 / max(PI*denominator*denominator, 1e-5);
        let k = (roughness+1.0)*(roughness+1.0)*0.125;
        let visibility = (nl / max(nl*(1.0-k)+k, 1e-4)) / max(facing*(1.0-k)+k, 1e-4);
        let sun_grazing = 1.0 - vh;
        let sun_grazing2 = sun_grazing * sun_grazing;
        let sun_fresnel = 0.02 + 0.98 * sun_grazing2 * sun_grazing2 * sun_grazing;
        let sun = sky_env.light_color * sky_env.light_intensity;
        specular += sun * (sun_fresnel * distribution * visibility * 0.25)
            * lake.tint.w * jka_water_sun_admission(input.world.xz);
    }
    var body = color * (0.04 + nl * 0.12);
    // The absorption is exponential, but a constant body/transmission blend
    // still left a residual bed contribution at arbitrary depths. Fade that
    // blend itself by vertical depth, reaching the exact opaque body at the
    // authored cutoff. This stays almost flat/clear close to the shore.
    let opaque_depth = max(lake.transmission.w, 0.001);
    let bed_depth = max(input.world.y - behind.height, 0.0);
    var visibility = 1.0 - smoothstep(0.0, opaque_depth, bed_depth);
    if lake.transmission.x > 0.0 && !behind.open && behind.in_front > 0.0 && visibility > 0.0 {
        // Offset the opaque scene once, reusing the three normals above. Fade
        // distortion at shores and reject offsets onto foreground objects.
        let uv = input.clip.xy / vec2<f32>(size);
        var bed_pixel = pixel;
        var column = clamp(behind.in_front, 0.0, 80.0);
        if lake.transmission.y > 0.0 {
            let slope = (camera.view * vec4<f32>(up, 0.0)).xy * vec2<f32>(1.0, -1.0);
            let aspect = vec2<f32>(f32(size.y) / f32(size.x), 1.0);
            let bend = slope * aspect * lake.transmission.y * smoothstep(0.0, 0.4, behind.in_front);
            let shifted_uv = clamp(uv + bend, vec2<f32>(0.0), vec2<f32>(1.0));
            let shifted = clamp(vec2<i32>(shifted_uv * vec2<f32>(size)), vec2<i32>(0), size - 1);
            let depth = textureLoad(scene_depth, shifted, 0);
            // Reconstruct on the DISPLACED pixel's ray. Comparing its view depth
            // with the original ray's water depth accepts above-water players
            // at oblique angles and paints a displaced silhouette around them.
            let shifted_ndc = (vec2<f32>(shifted) + 0.5) / vec2<f32>(size)
                * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
            let hit_h = camera.inverse_view_projection * vec4<f32>(shifted_ndc, depth, 1.0);
            let hit = hit_h.xyz / hit_h.w;
            let across = hit.xz - camera.position.xz;
            let shifted_depth = input.world.y - (hit.y + camera.curvature * dot(across, across));
            if depth > 0.0 && shifted_depth > 0.0 {
                bed_pixel = shifted;
                let ray = hit - camera.position;
                column = clamp(shifted_depth * length(ray) / max(abs(ray.y), 1e-3), 0.0, 80.0);
                visibility = min(visibility, 1.0 - smoothstep(0.0, opaque_depth, shifted_depth));
            }
        }
        if visibility > 0.0 {
            let attenuation = exp(-lake.fresh_absorption.rgb * column);
            // Read precisely the texel whose depth passed the check. Bilinear
            // color filtering could otherwise blend in an unchecked foreground
            // neighbor at silhouettes. This adds no scene texture reads.
            let bed = textureLoad(scene_color, bed_pixel, 0).rgb;
            let transmitted = bed * attenuation * mix(vec3<f32>(1.0), color, 0.15)
                + color * (vec3<f32>(1.0) - attenuation) * 0.12;
            body = mix(body, transmitted, lake.transmission.x * visibility);
        }
    }
    var water = body * (1.0 - reflected_share) + specular;
    if lake.offset.z > 0.5 { water = aerial_apply(water, input.world); }
    return vec4<f32>(water, 1.0);
}

