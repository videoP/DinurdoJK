// Heavy rain's veil over the composed scene in linear radiance (`rainHaze` in the web game's `sky/weather/haze.ts`): by the
// length of rain along each view ray, colour drains toward grey and low-contrast distance lifts toward the rain-lit
// air, glowing toward a strike. One full-screen pass from the scene's colour copy into the scene.

#import naval_sky::atmosphere::{sky_radiance}
#import naval_clouds::post::{post, scene_color, scene_depth}

const RAIN_VISIBILITY: f32 = 1800.0;
const DESATURATE: f32 = 0.35;
const LIFT: f32 = 0.18;
const LONGEST: f32 = 30000.0;
const RAIN_AIR: f32 = 0.85;
const AIR_LIGHTNING: f32 = 0.12;
const AIR_LIGHTNING_G: f32 = 0.6;
const SKY_ELEVATION: f32 = 0.15;

struct HazeVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) ray: vec3<f32>,
    // The rain-lit air toward the horizon ahead (the same at every corner: evaluated here, not per pixel).
    @location(1) tint: vec3<f32>,
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn phase(cosine: f32, g: f32) -> f32 {
    return (1.0 - g * g) / (4.0 * 3.141592653589793) / pow(max(1.0 + g * g - cosine * 2.0 * g, 1e-4), 1.5);
}

@vertex
fn haze_vertex(@builtin(vertex_index) index: u32) -> HazeVertex {
    let clip = vec2<f32>(select(-1.0, 3.0, index == 1u), select(-1.0, 3.0, index == 2u));
    var out: HazeVertex;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    let point = post.inverse_projection * vec4<f32>(clip, 0.5, 1.0);
    out.ray = (post.camera_world * vec4<f32>(point.xyz / point.w, 0.0)).xyz;
    var tint = vec3<f32>(0.0);
    if post.haze_strength > 0.0 {
        let ahead = normalize(vec3<f32>(post.forward.x, 0.08, post.forward.z));
        let around = sky_radiance(normalize(vec3<f32>(ahead.x, max(ahead.y, SKY_ELEVATION), ahead.z)), false);
        tint = mix(around, vec3<f32>(luminance(around)), post.grey) * RAIN_AIR;
    }
    out.tint = tint;
    return out;
}

@fragment
fn haze_fragment(in: HazeVertex) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    var color = textureLoad(scene_color, pixel, 0);
    if post.haze_strength > 0.0 {
        let ray = normalize(in.ray);
        var rain = min(select(post.haze_ceiling / max(ray.y, 0.02), post.haze_height / max(-ray.y, 1e-4), ray.y < 0.0), LONGEST);
        // Upstream's scene is sea and sky, so the veil was the rain's length along the ray whatever stood in it. Here a
        // wall, a hill or a player in front of the horizon would be veiled as if it were the horizon itself: stop the
        // rain at the first opaque surface (reversed depth 0 is sky, which keeps the full length).
        let depth_size = textureDimensions(scene_depth);
        let depth = textureLoad(scene_depth, min(vec2<u32>(in.position.xy), depth_size - 1u), 0);
        if depth > 0.0 {
            let uv = in.position.xy / vec2<f32>(depth_size);
            let h = post.inverse_projection * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
            rain = min(rain, length(h.xyz / h.w));
        }
        let veil = (1.0 - exp(rain / -RAIN_VISIBILITY)) * post.haze_strength;
        let grey = mix(color.rgb, vec3<f32>(luminance(color.rgb)), veil * DESATURATE);
        var lit = in.tint;
        if post.lightning > 0.0 {
            lit += vec3<f32>(0.8, 0.87, 1.0) * (post.lightning * phase(dot(ray, post.bolt_direction), AIR_LIGHTNING_G) * AIR_LIGHTNING);
        }
        color = vec4<f32>(mix(grey, lit, veil * LIFT), color.a);
    }
    return color;
}
