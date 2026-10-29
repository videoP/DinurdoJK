// WaterAdvanced cached stages 5/6: depth projection, contour dilation,
// bridge selection and two-cell border. See capture.rs / provenance.json.
// Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
struct Settings {
    dimensions: vec4<u32>, // grid XY, dilation radius, capture resolution
    size: vec4<f32>,       // world XY, square capture width, bottom floor
    cameras: array<vec4<f32>, 5>, // local camera Z, +/- depth direction
};
struct Contours {
    first: vec4<f32>, // landscape, actors top/under, dilated top
    second: vec4<f32>, // dilated under, padding
};
@group(0) @binding(0) var<uniform> params: Settings;
@group(0) @binding(1) var depths: texture_2d_array<f32>;
// Five tightly packed scalars per cell keep Tsangpo's ~4.9M cells below the
// desktop 128-MiB storage binding limit; vec4+scalar struct arrays pad to 32B.
@group(0) @binding(2) var<storage, read_write> contours: array<f32>;
@group(0) @binding(3) var<storage, read> water_info: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> geometry: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> previous_bottom: array<f32>;

fn depth_at(cell: vec2<i32>, layer: i32) -> f32 {
    return textureLoad(depths, clamp(cell, vec2<i32>(0), vec2<i32>(i32(params.dimensions.w) - 1)), layer, 0).x;
}
fn bilinear(uv: vec2<f32>, layer: i32) -> f32 {
    let texel = uv * f32(params.dimensions.w) - 0.5;
    let lower = vec2<i32>(floor(texel));
    let f = fract(texel);
    let a = mix(depth_at(lower, layer), depth_at(lower + vec2<i32>(1,0), layer), f.x);
    let b = mix(depth_at(lower + vec2<i32>(0,1), layer), depth_at(lower + vec2<i32>(1,1), layer), f.x);
    return mix(a, b, f.y);
}
@compute @workgroup_size(8,8)
fn project_contours(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= params.dimensions.xy) { return; }
    let uv = 0.5 + ((vec2<f32>(id.xy) + 0.5) / vec2<f32>(params.dimensions.xy) - 0.5) * params.size.xy / params.size.z;
    var values: array<f32, 5>;
    for (var layer = 0; layer < 5; layer++) {
        values[layer] = params.size.w;
        if all(uv >= vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0)) {
            values[layer] = max(params.cameras[layer].x + params.cameras[layer].y * bilinear(uv, layer), params.size.w);
        }
    }
    let i = id.y * params.dimensions.x + id.x;
    for (var layer = 0u; layer < 5u; layer++) {
        contours[i * 5u + layer] = values[layer];
    }
}
fn load_contours(i: u32) -> Contours {
    return Contours(vec4<f32>(contours[i*5u], contours[i*5u+1u], contours[i*5u+2u], contours[i*5u+3u]), vec4<f32>(contours[i*5u+4u],0.0,0.0,0.0));
}
@compute @workgroup_size(8,8)
fn prepare_geometry(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= params.dimensions.xy) { return; }
    let i = id.y * params.dimensions.x + id.x;
    let cell = load_contours(i);
    let upper = vec2<i32>(params.dimensions.xy) - 1;
    let radius = i32(params.dimensions.z);
    var top = -100000.0;
    var under = -100000.0;
    for (var x = -radius; x <= radius; x++) {
        for (var y = -radius; y <= radius; y++) {
            let p = vec2<u32>(clamp(vec2<i32>(id.xy) + vec2<i32>(x,y), vec2<i32>(0), upper));
            let sample = load_contours(p.y * params.dimensions.x + p.x);
            top = max(top, sample.first.w);
            under = max(under, sample.second.x);
        }
    }
    top = max(top, cell.first.y);
    under = min(under, cell.first.z);
    if water_info[i].x - under <= 0.0 && under > cell.first.x && top > under && top > cell.first.x {
        top = cell.first.x;
    }
    let border = any(vec2<i32>(id.xy) < vec2<i32>(2)) || any(vec2<i32>(id.xy) > upper - 2);
    geometry[i] = vec4<f32>(max(top, cell.first.x), select(0.0, 1.0, border), 0.0, 0.0);
    // First-iteration history is the grid's reset zero. Further PreSim updates
    // must copy the preceding selected bottom before replacing geometry.
    previous_bottom[i] = 0.0;
}
