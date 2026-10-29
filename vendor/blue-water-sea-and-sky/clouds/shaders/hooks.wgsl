// What the clouds hand the sky's dome and environment bake (`naval_sky::CloudHooks`, at group 2 of both): the light
// shafts' length at a pixel of the frame being drawn, which the dome takes out of its sky radiance (takram's
// `shadowLength` in the sky and aerial perspective), and for the bake a short march of the clouds (premultiplied
// radiance in rgb, transmittance in a, the aerial perspective from the sea included) and the share of the scene's light
// they let through to the sea, which the bake's sea reflects (`clouds_bake_light`).

#define_import_path naval_clouds::hooks

#import naval_clouds::common::{clouds, shadow_length_a, shadow_length_b, world_to_planet, ray_sphere_second, RECIPROCAL_PI4}
#import naval_clouds::field::{ColumnAbove, sample_weather, sample_media, weather_uv, column_above, any_deck}
#import naval_clouds::march::{frame_light, clouds_sun_sky_irradiance, ray_light, sun_scattering, sky_light, apply_aerial_perspective, ray_spheres, ray_intersects_ground}

// Kilometres of the view ray through `pixel` (the dome's fragment position) that the clouds' shadow covers, 0 without
// light shafts.
fn clouds_shadow_length(pixel: vec2<f32>) -> f32 {
    if clouds.state.z < 0.5 || clouds.shafts.w < 0.5 {
        return 0.0;
    }
    let size = textureDimensions(shadow_length_a);
    let at = min(vec2<u32>(max(pixel / clouds.viewport.xy * vec2<f32>(size), vec2<f32>(0.0))), size - 1u);
    if clouds.state.w > 0.5 {
        return textureLoad(shadow_length_b, at, 0).r;
    }
    return textureLoad(shadow_length_a, at, 0).r;
}

// The sun's optical depth from a bake sample up to the top of the shadow-casting layers, in even steps. The march
// on screen takes the bulk of it from the Beer shadow maps, which cover only the camera's frustum; with takram's short
// march toward the sun alone (100 m) the bake's clouds came out nearly unshaded, and the sea, which reflects the bake
// and takes its water's light from it, turned grey under a partly cloudy sky.
const BAKE_SUN_STEPS: i32 = 6;

fn bake_sun_optical_depth(position: vec3<f32>, mip_level: f32) -> f32 {
    let sun = clouds.sun.xyz;
    let top = clouds.sun.w + clouds.heights.z;
    let b = dot(position, sun);
    let reach = -b + sqrt(max(b * b - dot(position, position) + top * top, 0.0));
    if reach <= 0.0 {
        return 0.0;
    }
    let step_size = reach / f32(BAKE_SUN_STEPS);
    var depth = 0.0;
    for (var i = 0; i < BAKE_SUN_STEPS; i++) {
        let point = position + sun * ((f32(i) + 0.5) * step_size);
        let uv = weather_uv(point);
        let weather = sample_weather(uv, length(point) - clouds.sun.w, mip_level, true);
        if !any(weather.density > vec4<f32>(clouds.march_limits.y)) {
            continue;
        }
        depth += sample_media(weather, point, uv, mip_level, 0.5).extinction * step_size;
    }
    return depth;
}

// The share of the scene's light (toward `toward`: the sun, or the moon once it outshines it) the clouds let through to
// the sea at `origin` (world m), for the sea's reflection of it in the bake: the shadow-casting layers marched in
// `BAKE_SUN_STEPS` even steps, as the ground shadow map (`ground.wgsl`) marches them, at its strength (none with the
// light low). 1 without clouds drawn.
fn clouds_bake_light(origin: vec3<f32>, toward: vec3<f32>) -> f32 {
    if clouds.state.z < 0.5 || toward.y <= 0.0 {
        return 1.0;
    }
    let ground = world_to_planet(origin);
    let enter = max(ray_sphere_second(ground, toward, clouds.sun.w + clouds.heights.w), 0.0);
    let leave = ray_sphere_second(ground, toward, clouds.sun.w + clouds.heights.z);
    var optical_depth = 0.0;
    if leave > enter {
        let step_size = (leave - enter) / f32(BAKE_SUN_STEPS);
        for (var i = 0; i < BAKE_SUN_STEPS; i++) {
            let position = ground + toward * (enter + (f32(i) + 0.5) * step_size);
            let uv = weather_uv(position);
            let weather = sample_weather(uv, length(position) - clouds.sun.w, 0.0, true);
            if any(weather.density > vec4<f32>(clouds.shadow_march.w)) {
                optical_depth += sample_media(weather, position, uv, 0.0, 0.5).extinction * step_size;
            }
        }
    }
    return mix(1.0, exp(-optical_depth), clouds.bake.y);
}

// How far into `stretch` metres of the medium the light it sends back along the ray comes from: the mean
// depth of its scattering, each part weighed by what reaches the start. Half the stretch where the medium is thin,
// one mean free path (1 / extinction) where it is opaque.
fn light_depth(extinction: f32, stretch: f32) -> f32 {
    let depth = extinction * stretch;
    if depth < 0.05 {
        return stretch * (0.5 - depth / 12.0);
    }
    let through = exp(-depth);
    return (1.0 - (1.0 + depth) * through) / ((1.0 - through) * extinction);
}

// The environment's march from `origin` (world m, the sea under the camera) along `direction`: the tier's bake steps
// evenly through the layers, `BAKE_SUN_STEPS` light samples toward the sun, a mid jitter. A step is hundreds of metres
// long, more than a deck is deep: lit at its middle, a deck seen from the sea took the light of a point most of the way
// up it, where little cloud is left overhead, and came out up to twice as bright as the base the march on screen
// draws (and the sea, the hulls and the haze with it, which take the bake's light). A step of a slab (a deck, fog) is
// lit where its light comes from (`light_depth`): a few tens of metres past the base of a deck. Broken cloud keeps its
// middle.
fn clouds_bake(origin: vec3<f32>, direction: vec3<f32>) -> vec4<f32> {
    if clouds.state.z < 0.5 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let start = world_to_planet(origin);
    if ray_intersects_ground(start, direction) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let spheres = ray_spheres(start, direction, clouds.sun.w + vec4<f32>(0.0, clouds.heights.x, clouds.heights.y, 0.0));
    let near = max(spheres.second.y, 0.0);
    let far = min(spheres.second.z, clouds.march.w);
    if far <= near {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let steps = max(i32(clouds.bake.x), 1);
    let step_size = (far - near) / f32(steps);
    let light = frame_light(start);
    let ray = ray_light(dot(clouds.sun.xyz, direction));
    var radiance = vec3<f32>(0.0);
    var transmittance = 1.0;
    var weighted_distance_sum = 0.0;
    var transmittance_sum = 0.0;
    for (var i = 0; i < steps; i++) {
        let distance = near + (f32(i) + 0.5) * step_size;
        let position = start + direction * distance;
        let height = length(position) - clouds.sun.w;
        let uv = weather_uv(position);
        let mip_level = log2(1.0 + distance * 1e-5);
        let weather = sample_weather(uv, height, mip_level, false);
        if !any(weather.density > vec4<f32>(clouds.march_limits.y)) {
            continue;
        }
        let media = sample_media(weather, position, uv, mip_level, 0.5);
        if media.extinction <= clouds.march_limits.z {
            continue;
        }
        let step_transmittance = exp(-media.extinction * step_size);
        let opacity = 1.0 - transmittance * vec2<f32>(1.0, step_transmittance);
        // Where the step is lit: its middle, or for a slab the depth its light comes from.
        var lit = position;
        var lit_height = height;
        var lit_uv = uv;
        let slab = dot(clouds.layer_slabs, media.weight);
        if slab > 0.0 {
            let from_start = mix(0.5 * step_size, light_depth(media.extinction, step_size), slab);
            lit = start + direction * (near + f32(i) * step_size + from_start);
            lit_height = length(lit) - clouds.sun.w;
            lit_uv = weather_uv(lit);
        }
        // The march's light (`naval_clouds::march::sample_radiance`) with the bake's own optical depth toward the sun.
        let irradiance = clouds_sun_sky_irradiance(lit, lit_height, light);
        let toward_sun = bake_sun_optical_depth(lit, mip_level);
        var source = irradiance.sun * sun_scattering(toward_sun, opacity, media, ray);
        var column = ColumnAbove(0.0, 0.0);
        if any_deck() {
            column = column_above(lit_uv, lit_height, mip_level);
        }
        source += sky_light(irradiance, weather, media, column, normalize(lit)) * RECIPROCAL_PI4 * clouds.scattering.x;
        source *= media.scattering;
        radiance += transmittance * (source - source * step_transmittance) / max(media.extinction, 1e-7);
        transmittance *= step_transmittance;
        weighted_distance_sum += distance * transmittance;
        transmittance_sum += transmittance;
        if transmittance <= clouds.march_limits.w {
            break;
        }
    }
    if transmittance_sum <= 0.0 {
        return vec4<f32>(radiance, transmittance);
    }
    let front = start + direction * (weighted_distance_sum / transmittance_sum);
    let color = apply_aerial_perspective(start, front, 0.0, vec4<f32>(radiance, 1.0 - transmittance));
    return vec4<f32>(color.rgb, transmittance);
}
