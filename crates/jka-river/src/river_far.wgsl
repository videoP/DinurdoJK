// Water mesh quadtree tiles for a sparse river, outside the camera window.
// Vertex side: UE 5.8.1 Water plugin WaterMeshVertexFactory.ush (CPU-quadtree
// instance path: UnpackWaterVertexFactoryInstanceInput, MorphTranslatedWorld-
// Position applied up to twice). Copyright Epic Games, Inc. All Rights Reserved.
// The water mesh is flat at the tile's base height; the river material's WPO
// gives it shape. Here that WPO is the same live-grid graph the camera window
// runs (river_live_grid_wpo / river_live_grid_normal), fed from the sparse
// pool's PrepareSurface / ComputeNormals outputs instead of the window's
// textures: the same bilinear zero-load field and bilinear normals, addressed
// through the tile table. Foam is absent outside the window (FoamRT only
// exists there); the classic indirect field is sampled per tile on the CPU.

struct RiverFar {
    origin: vec4<f32>,  // solver origin xyz (river cm), dx cm
    lattice: vec4<i32>, // lattice cells xy, tile table width, log2 tile size
    lod: vec4<f32>,     // LODScale (cm), LOD morphing enabled, NumQuadsLOD0, tile cells
    window: vec4<f32>,  // camera-window interior min xy, max xy (river cm); empty when min > max
    format: vec4<u32>,  // x: pool normals packed (r_riverStorageMode 2), else vec4<f32> words; yzw unused
}
struct RiverFarInstance {
    translation: vec4<f32>, // Data[0]: tile centre xy, base height, unused (river cm)
    packed: vec4<u32>,      // Data[1].x bits: 0-7 LOD level, 8 bShouldMorph, 9 bCanMorphTwice, 10-14 density index
    scale: vec4<f32>,       // Data[1].zw scale xy, Data[1].y HeightMorph, unused
    ambient: vec4<f32>,     // classic indirect field at the tile centre
    directed: vec4<f32>,
}
@group(1) @binding(10) var<uniform> river_far: RiverFar;
@group(1) @binding(11) var<storage, read> far_slots: array<i32>;
@group(1) @binding(12) var<storage, read> far_surface: array<vec4<f32>>;
// Pool normals as words: four FP32 words per cell, or one octahedral word.
@group(1) @binding(13) var<storage, read> far_normals: array<u32>;
@group(1) @binding(14) var<storage, read> far_instances: array<RiverFarInstance>;

// Pool index of a lattice cell, or -1 outside the lattice / not resident.
fn river_far_index(cell: vec2<i32>) -> i32 {
    if any(cell < vec2<i32>(0)) || any(cell >= river_far.lattice.xy) { return -1; }
    let tile = vec2<u32>(cell) >> vec2<u32>(u32(river_far.lattice.w));
    let slot = far_slots[tile.y * u32(river_far.lattice.z) + tile.x];
    if slot < 0 { return -1; }
    let size = u32(river_far.lod.w);
    let lane = vec2<u32>(cell) & vec2<u32>(size - 1u);
    return slot * i32(size * size) + i32(lane.y * size + lane.x);
}
fn river_far_load(cell: vec2<i32>) -> vec4<f32> {
    let i = river_far_index(cell);
    if i < 0 { return vec4<f32>(0.0); }
    return far_surface[u32(i)];
}
fn river_far_normal_load(cell: vec2<i32>) -> vec3<f32> {
    let i = river_far_index(cell);
    if i < 0 { return vec3<f32>(0.0, 0.0, 1.0); }
    if river_far.format.x != 0u { return river_unpack_normal(far_normals[u32(i)]); }
    let w = u32(i) * 4u;
    return bitcast<vec3<f32>>(vec3<u32>(far_normals[w], far_normals[w + 1u], far_normals[w + 2u]));
}
// river_live_field on the pool: cell-centred bilinear, zero outside.
fn river_far_field(world_xy: vec2<f32>) -> vec4<f32> {
    let position = (world_xy - river_far.origin.xy) / river_far.origin.w - vec2<f32>(0.5);
    let cell = vec2<i32>(floor(position));
    let f = position - vec2<f32>(cell);
    let a = river_far_load(cell);
    let b = river_far_load(cell + vec2<i32>(1, 0));
    let c = river_far_load(cell + vec2<i32>(0, 1));
    let d = river_far_load(cell + vec2<i32>(1, 1));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}
// NormalsRT's bilinear sample on the pool.
fn river_far_base_normal(world_xy: vec2<f32>) -> vec3<f32> {
    let position = (world_xy - river_far.origin.xy) / river_far.origin.w - vec2<f32>(0.5);
    let cell = vec2<i32>(floor(position));
    let f = position - vec2<f32>(cell);
    let a = river_far_normal_load(cell);
    let b = river_far_normal_load(cell + vec2<i32>(1, 0));
    let c = river_far_normal_load(cell + vec2<i32>(0, 1));
    let d = river_far_normal_load(cell + vec2<i32>(1, 1));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

struct RiverFarMorph { position: vec3<f32>, lod_factor: f32 }

// MorphTranslatedWorldPosition (camera origin XY in river cm).
fn river_far_morph(original: vec3<f32>, morph_origin: vec2<f32>, lod_level: f32, lod_scale: f32, quad_size: vec2<f32>, height_lod_factor: f32) -> RiverFarMorph {
    var position = original;
    let distance_to_vert = distance(position.xy, river_view.eye.xy);
    var lod_factor = saturate(distance_to_vert / (lod_scale * pow(2.0, lod_level)) - 1.0);
    lod_factor = saturate(height_lod_factor + lod_factor);
    let grid4 = 4.0 * quad_size;
    let local = position.xy - morph_origin;
    let offset = fract(local / grid4) - vec2<f32>(0.5);
    let min_radius = 0.26;
    if abs(offset.x) < min_radius { position.x += offset.x * lod_factor * grid4.x; }
    if abs(offset.y) < min_radius { position.y += offset.y * lod_factor * grid4.y; }
    return RiverFarMorph(position, lod_factor);
}

struct RiverFarOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>, // river axes, cm
    @location(1) @interpolate(flat) ambient: vec4<f32>,
    @location(2) @interpolate(flat) directed: vec4<f32>,
    @location(3) @interpolate(flat) eye_under: f32,
    @location(4) @interpolate(flat) lod: vec2<f32>, // LOD level, density index
}

@vertex fn vs_far(@location(0) position: vec2<f32>, @builtin(instance_index) instance_index: u32) -> RiverFarOut {
    let instance = far_instances[instance_index];
    let packed = instance.packed.x;
    let lod_level = f32(packed & 0xFFu);
    let should_morph = ((packed >> 8u) & 1u) != 0u && river_far.lod.y > 0.5;
    let can_morph_twice = ((packed >> 9u) & 1u) != 0u;
    let density = (packed >> 10u) & 0x1Fu;
    // The CPU path's vertex factory for this density: NumQuadsLOD0 >> density.
    let quads = max(1u, u32(river_far.lod.z) >> density);
    let scale = instance.scale.xy;
    var p = vec3<f32>(position * scale, 0.0) + instance.translation.xyz;
    if should_morph {
        let quad_size = scale / f32(quads);
        let corner = instance.translation.xy - scale * 0.5;
        var m = river_far_morph(p, corner, lod_level, river_far.lod.x, quad_size, instance.scale.z);
        if m.lod_factor >= 1.0 && can_morph_twice {
            m = river_far_morph(m.position, corner, lod_level + 1.0, river_far.lod.x, quad_size * 2.0, 0.0);
        }
        p = m.position;
    }
    // River WPO on the pool (river_live_grid_wpo): the flat tile sits at the
    // solver origin; the field's height and the advected wave lift it.
    let world_xy = p.xy;
    let up = vec3<f32>(0.0, 0.0, 1.0);
    let rest = vec3<f32>(world_xy, river_far.origin.z);
    let camera_distance = distance(rest, river_view.eye.xyz);
    let fluid = river_far_field(world_xy);
    let normal = river_far_base_normal(world_xy);
    let detail = river_wave_vertex(ocean_detail, ocean_sampler, world_xy, fluid.zw, normal, river_view.time.x, river_wpo_advection());
    let wave_height = river_advected_height(detail.a, length(fluid.zw), camera_distance, river_wave_height());
    var world = river_grid_displacement(fluid, normal, wave_height, rest, up, river_motion(river_view.time.z), false);
    // Bed clamp, vertex half (river_surface.wgsl river_lip_lift).
    {
        let position = (world_xy - river_far.origin.xy) / river_far.origin.w - vec2<f32>(0.5);
        let cell = vec2<i32>(floor(position));
        let a = river_far_load(cell);
        let b = river_far_load(cell + vec2<i32>(1, 0));
        let c = river_far_load(cell + vec2<i32>(0, 1));
        let d = river_far_load(cell + vec2<i32>(1, 1));
        world += up * river_lip_lift(a, b, c, d, fluid.x, river_far.origin.w);
    }
    let map = world / RIVER_CM_PER_UNIT;
    var out: RiverFarOut;
    out.clip = camera.view_proj * vec4<f32>(map.x, map.z, -map.y, 1.0);
    out.world = world;
    out.lod = vec2<f32>(lod_level, f32(density));
    out.ambient = instance.ambient;
    out.directed = instance.directed;
    // Seen from inside the river the submerged medium pass takes over.
    let at_eye = river_far_field(river_view.eye.xy);
    let surface = river_far.origin.z + at_eye.x;
    out.eye_under = select(0.0, 1.0, at_eye.y > 1e-8 && river_view.eye.z < surface && river_view.eye.z > surface - at_eye.y);
    return out;
}

// The material graph's inputs on the pool, before any discard (the advected
// detail samples take fragment derivatives).
struct RiverFarPixel { graph_normal: vec3<f32>, material_mask: f32, outside_window: bool }

fn river_far_pixel(world: vec3<f32>, pixel_depth: f32, foam: RiverFoam) -> RiverFarPixel {
    let world_xy = world.xy;
    let time = river_view.time.x;
    let fluid = river_far_field(world_xy);
    // Inside the simulation everywhere (the lattice border is a collider).
    let masks = river_surface_masks(fluid.y, 1.0, vec2<f32>(0.5), vec2<f32>(1.0e6), 1.0, pixel_depth, 3.0e38);
    let base = river_far_base_normal(world_xy);
    let wpo = river_wave_fragment_lod(ocean_detail, ocean_sampler, world_xy, fluid.zw, base, time, river_wpo_advection(), river_view.material_lod.xy).xyz;
    let detail = river_wave_fragment_lod(ocean_detail, ocean_sampler, world_xy, fluid.zw, base, time, river_detail_advection(), river_view.material_lod.xy).xyz;
    let graph_normal = river_surface_normal(base, wpo, detail, length(fluid.zw), river_motion(river_view.time.z), river_wave_height(),
        foam.normal, foam.total, river_foam_settings().shading.x, masks.simulation_mask);
    // The camera window draws its interior (it drops its own outermost cell).
    let inside = all(world_xy > river_far.window.xy) && all(world_xy < river_far.window.zw);
    return RiverFarPixel(graph_normal, masks.material_mask, !inside);
}

@fragment fn fs_far(input: RiverFarOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let eye = river_view.eye.xyz;
    let pixel_depth = dot(input.world - eye, river_view.forward.xyz);
    let slope = max(abs(dpdxFine(pixel_depth)), abs(dpdyFine(pixel_depth)));
    let foam = RiverFoam(vec3<f32>(0.0, 0.0, 1.0), 0.0, 0.0, 0.0, 0.0, vec3<f32>(0.0));
    let pixel = river_far_pixel(input.world, pixel_depth, foam);
    if !pixel.outside_window || pixel.material_mask < 0.3333 || input.eye_under > 0.5 {
        discard;
    }
    return river_base_pass(input.clip, input.world, front, pixel.graph_normal, foam, pixel_depth, slope, input.ambient, input.directed);
}

// r_showtris river bit: the water mesh tiles, coloured by LOD level (as
// r.Water.WaterMesh.ShowLODLevels colours by GColorList index).
// r_riverWire 0: only where fs_far draws (outside the window, wet).
@fragment fn fs_far_wire(input: RiverFarOut) -> @location(0) vec4<f32> {
    let fluid = river_far_field(input.world.xy);
    let masks = river_surface_masks(fluid.y, 1.0, vec2<f32>(0.5), vec2<f32>(1.0e6), 1.0, 0.0, 3.0e38);
    let inside = all(input.world.xy > river_far.window.xy) && all(input.world.xy < river_far.window.zw);
    if inside || masks.material_mask < 0.3333 {
        discard;
    }
    return river_far_lod_colour(input);
}
// r_riverWire 1: every tile, dry parts included.
@fragment fn fs_far_wire_full(input: RiverFarOut) -> @location(0) vec4<f32> {
    return river_far_lod_colour(input);
}
fn river_far_lod_colour(input: RiverFarOut) -> vec4<f32> {
    let colours = array<vec3<f32>, 8>(
        vec3<f32>(1.0, 0.2, 0.2), vec3<f32>(1.0, 0.6, 0.1), vec3<f32>(1.0, 1.0, 0.2), vec3<f32>(0.3, 1.0, 0.3),
        vec3<f32>(0.2, 0.9, 1.0), vec3<f32>(0.3, 0.4, 1.0), vec3<f32>(0.8, 0.3, 1.0), vec3<f32>(1.0, 1.0, 1.0));
    return vec4<f32>(colours[min(u32(input.lod.x), 7u)], 1.0);
}

@fragment fn fs_composite_far(input: RiverFarOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let eye = river_view.eye.xyz;
    let pixel_depth = dot(input.world - eye, river_view.forward.xyz);
    let slope = max(abs(dpdxFine(pixel_depth)), abs(dpdyFine(pixel_depth)));
    let foam = RiverFoam(vec3<f32>(0.0, 0.0, 1.0), 0.0, 0.0, 0.0, 0.0, vec3<f32>(0.0));
    let pixel = river_far_pixel(input.world, pixel_depth, foam);
    if !pixel.outside_window || pixel.material_mask < 0.3333 || input.eye_under > 0.5 {
        discard;
    }
    return river_composite_pass(input.clip, input.world, front, pixel.graph_normal, foam, pixel_depth, slope);
}
