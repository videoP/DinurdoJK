// Only appended to the opt-in model pipelines. The control shaders, layouts,
// vertex streams and glow replays contain none of these bindings or operations.
struct FixtureLight {
    position_radius: vec4<f32>,
    normal_power: vec4<f32>,
    color_side: vec4<f32>,
    shadow: vec4<f32>, // static cube slot, moving cube slot (-1 absent), reserved
};
struct FixtureLights { lights: array<FixtureLight, 4> };
@group(FIXTURE_GROUP) @binding(0) var<uniform> fixture_lights: FixtureLights;
@group(FIXTURE_GROUP) @binding(1) var fixture_depth: texture_depth_cube_array;
@group(FIXTURE_GROUP) @binding(2) var fixture_comparison: sampler_comparison;

fn fixture_visibility(light: FixtureLight, world: vec3<f32>, normal: vec3<f32>, slot: i32) -> f32 {
    let from_light = world - light.position_radius.xyz;
    let radial = length(from_light);
    let major = max(max(abs(from_light.x), abs(from_light.y)), abs(from_light.z));
    let near = 4.0;
    let far = max(light.position_radius.w, near + 1.0);
    if major <= near || radial >= far { return 1.0; }
    let direction = from_light / max(radial, 0.0001);
    let slope = 1.0 - max(dot(normal, -direction), 0.0);
    let biased = max(major - (0.5 + slope), near);
    let reference = far / (far-near) - far*near / ((far-near)*biased);
    let helper = select(vec3(1.0,0.0,0.0),vec3(0.0,1.0,0.0),abs(direction.y)<0.9);
    let tangent = normalize(cross(direction,helper));
    let bitangent = cross(direction,tangent);
    let spread = 1.6 / f32(textureDimensions(fixture_depth).x);
    var visible = 0.0;
    visible += textureSampleCompareLevel(fixture_depth,fixture_comparison,direction+(tangent+bitangent)*spread,slot,reference);
    visible += textureSampleCompareLevel(fixture_depth,fixture_comparison,direction+(-tangent+bitangent)*spread,slot,reference);
    visible += textureSampleCompareLevel(fixture_depth,fixture_comparison,direction+(tangent-bitangent)*spread,slot,reference);
    visible += textureSampleCompareLevel(fixture_depth,fixture_comparison,direction+(-tangent-bitangent)*spread,slot,reference);
    return visible*0.25;
}

fn fixture_irradiance(world: vec3<f32>, surface_normal: vec3<f32>) -> vec3<f32> {
    if camera.render_flags.x != 0u { return vec3(0.0); }
    let normal = normalize(surface_normal);
    var sum = vec3(0.0);
    for (var i=0u; i<4u; i+=1u) {
        let light = fixture_lights.lights[i];
        if light.normal_power.w <= 0.0 { continue; }
        let delta = light.position_radius.xyz-world;
        let d2 = dot(delta,delta);
        let r2 = light.position_radius.w*light.position_radius.w;
        if d2 < 0.0001 || d2 >= r2 { continue; }
        let toward = delta*inverseSqrt(d2);
        let receive = max(dot(normal,toward),0.0);
        let emit_cos = dot(light.normal_power.xyz,-toward);
        let emit = select(max(emit_cos,0.0),abs(emit_cos),light.color_side.w>0.5);
        if receive*emit <= 0.0 { continue; }
        let window = max(1.0-d2/r2,0.0);
        var visibility = fixture_visibility(light,world,normal,i32(light.shadow.x));
        if light.shadow.y >= 0.0 && visibility > 0.0 {
            visibility *= fixture_visibility(light,world,normal,i32(light.shadow.y));
        }
        // Restrained direct accent, separately evaluated for every surface normal.
        // The patch power is area-scaled; this is softened inverse-square
        // transport, not the world bake's broad radius-only falloff.
        sum += light.color_side.rgb * (light.normal_power.w * receive*emit
            * window*window * visibility * 0.12 / (d2+256.0));
    }
    // Hue-preserving shoulder bounds the accent without tinting the baked base.
    let peak = max(sum.r,max(sum.g,sum.b));
    return sum / (1.0+peak/0.30);
}
