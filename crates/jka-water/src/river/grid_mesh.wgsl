// Grid2D_SW_River cached GPU shader: reset-only stages 1 and 2.
// Native attribute values/types with an interleaved 32-byte vertex layout.
// Copyright Epic Games, Inc. All Rights Reserved. See grid-mesh-reference.json.
struct Settings {
    cells: vec2<u32>,
    vertices: vec2<u32>,
    size: vec2<f32>,
    padding: vec2<u32>,
}
@group(0) @binding(0) var<uniform> settings: Settings;
@group(0) @binding(1) var<storage, read_write> vertices: array<u32>;
@group(0) @binding(2) var<storage, read_write> indices: array<u32>;

@compute @workgroup_size(8,8)
fn create_vertices(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= settings.vertices) { return; }
    let uv = vec2<f32>(id.xy) / vec2<f32>(settings.cells);
    let position = (uv - vec2<f32>(0.5)) * settings.size;
    let offset = (id.x + id.y * settings.vertices.x) * 8u;
    vertices[offset] = bitcast<u32>(position.x);
    vertices[offset+1u] = bitcast<u32>(position.y);
    vertices[offset+2u] = bitcast<u32>(0.0);
    // GPU DI PackTangent: axis*127.499 truncated, sign from cross(X,Z).dot(Y).
    vertices[offset+3u] = 0x8000007fu;
    vertices[offset+4u] = 0x807f0000u;
    vertices[offset+5u] = bitcast<u32>(uv.x);
    vertices[offset+6u] = bitcast<u32>(uv.y);
    vertices[offset+7u] = 0xffffffffu;
}

@compute @workgroup_size(8,8)
fn create_triangles(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= settings.cells) { return; }
    let width = settings.vertices.x;
    let a = (id.x+1u) + (id.y+1u) * width;
    let b = (id.x+1u) + id.y * width;
    let c = id.x + id.y * width;
    let d = id.x + (id.y+1u) * width;
    let offset = (id.x + id.y * settings.cells.x) * 6u;
    indices[offset] = a;
    indices[offset+1u] = b;
    indices[offset+2u] = c;
    indices[offset+3u] = c;
    indices[offset+4u] = d;
    indices[offset+5u] = a;
}
