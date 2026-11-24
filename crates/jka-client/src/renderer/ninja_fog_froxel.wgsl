struct NinjaFogSample {
    extinction: f32,
    albedo: vec3<f32>,
    emission: vec3<f32>,
};

struct NinjaFogVolume {
    base_width: vec4<f32>,  // map-unit base centre and simulation width
    height_cover_time: vec4<f32>, // height and cover width in map units, game seconds
    sky_rgb: vec4<f32>,
    preset: vec4<u32>,
    ground_bounds: vec4<f32>, // ground offset bounds and capture centre in JKA XY
    sim_centre: vec4<f32>,
};

struct NinjaFogSegment {
    optical_depth: f32,
    scattering: vec3<f32>,
    emission: vec3<f32>,
};

@group(1) @binding(0) var<uniform> ninja_fog: NinjaFogVolume;
@group(1) @binding(1) var ninja_fog_vd: texture_2d<f32>;
@group(1) @binding(2) var ninja_fog_height: texture_2d<f32>;
@group(1) @binding(3) var ninja_fog_noise1: texture_3d<f32>;
@group(1) @binding(4) var ninja_fog_noise2: texture_3d<f32>;
@group(1) @binding(5) var ninja_fog_clamp: sampler;
@group(1) @binding(6) var ninja_fog_wrap: sampler;
@group(1) @binding(7) var ninja_fog_ground: texture_2d<f32>;
@group(1) @binding(8) var ninja_mist_passive_density: texture_2d<f32>;
@group(1) @binding(9) var ninja_mist_passive_velocity: texture_2d<f32>;

const NINJA_CM_PER_MAP_UNIT: f32 = 2.54;

fn ninja_fog_emission_integral(optical_depth: f32, transmittance: f32) -> f32 {
    if (optical_depth < 1.0e-4) {
        return 1.0 - 0.5 * optical_depth;
    }
    return (1.0 - transmittance) / optical_depth;
}

fn ninja_fog_ray_bounds(origin: vec3<f32>, ray: vec3<f32>) -> vec2<f32> {
    let centre = ninja_fog.base_width.xyz;
    let half_width = 0.5 * ninja_fog.height_cover_time.y;
    let lower = centre + vec3<f32>(-half_width, ninja_fog.ground_bounds.x, -half_width);
    let upper = centre + vec3<f32>(half_width, ninja_fog.ground_bounds.y + ninja_fog.height_cover_time.x, half_width);
    return fog_ray_box(origin, ray, lower, upper);
}

fn ninja_fog_ground_at(world: vec3<f32>) -> vec2<f32> {
    let base = ninja_fog.base_width.xyz;
    let cover = ninja_fog.height_cover_time.y;
    let uv = (vec2<f32>(world.x, -world.z) - ninja_fog.ground_bounds.zw) / cover + 0.5;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec2<f32>(0.0);
    }
    // Recover the height on the BSP plane captured for this column.
    // Filtering across separate floors creates heights in empty space.
    let size = textureDimensions(ninja_fog_ground);
    let grid = uv * vec2<f32>(size);
    let cell = min(vec2<u32>(grid), size - 1u);
    let plane = textureLoad(ninja_fog_ground, cell, 0);
    if (plane.z <= 0.0) {
        return vec2<f32>(0.0);
    }
    let offset = (grid - vec2<f32>(cell) - 0.5) * (cover / vec2<f32>(size));
    let height = base.y + plane.w - dot(plane.xy, offset) / plane.z;
    return vec2<f32>(height, 1.0);
}

fn ninja_fog_sample(
    world: vec3<f32>, eye: vec3<f32>, sun_direction: vec3<f32>,
    world_ddx: vec3<f32>, world_ddy: vec3<f32>,
) -> NinjaFogSample {
    let ground = ninja_fog_ground_at(world);
    let empty = NinjaFogSample(0.0, vec3<f32>(0.0), vec3<f32>(0.0));
    if (ground.y == 0.0) {
        return empty;
    }
    if (ninja_fog.preset.x == 3u) {
        return ninja_mist_material(world, ground.x, eye, -sun_direction, world_ddx, world_ddy, false);
    }
    let base = vec3<f32>(ninja_fog.base_width.x, ground.x, ninja_fog.base_width.z);
    let height = ninja_fog.height_cover_time.x;
    if (world.y < base.y || world.y > base.y + height) {
        return empty;
    }
    return ninja_fog_density(
        world * NINJA_CM_PER_MAP_UNIT, eye * NINJA_CM_PER_MAP_UNIT,
        base * NINJA_CM_PER_MAP_UNIT, ninja_fog.base_width.w * NINJA_CM_PER_MAP_UNIT,
        height * NINJA_CM_PER_MAP_UNIT, ninja_fog.height_cover_time.z, ninja_fog.preset.x,
        ninja_fog.sky_rgb.xyz, -sun_direction, true,
        world_ddx * NINJA_CM_PER_MAP_UNIT, world_ddy * NINJA_CM_PER_MAP_UNIT,
    );
}

fn ninja_fog_segment(
    ray: vec3<f32>, bounds: vec2<f32>, start: f32, end: f32,
    sun_direction: vec3<f32>,
    forward: vec3<f32>, ray_dx: vec3<f32>, ray_dy: vec3<f32>,
) -> NinjaFogSegment {
    let lo = max(bounds.x, start);
    let hi = min(bounds.y, end);
    if (lo >= hi) {
        return NinjaFogSegment(0.0, vec3<f32>(0.0), vec3<f32>(0.0));
    }
    let eye = settings.camera_pos_time.xyz;
    let ray_t = 0.5 * (hi + lo);
    let point = eye + ray * ray_t;
    // VoxelizePS rasterizes each slice at constant view depth, so material
    // gradients are the world offsets to the neighbouring voxels on that
    // plane. They drive the hardware mip selection UE gets from ddx/ddy.
    let view_depth = ray_t * dot(ray, forward);
    let world_ddx = ray_dx * (view_depth / dot(ray_dx, forward)) - ray * ray_t;
    let world_ddy = ray_dy * (view_depth / dot(ray_dy, forward)) - ray * ray_t;
    let sample = ninja_fog_sample(point, eye, sun_direction, world_ddx, world_ddy);
    // UE's FVOL shader applies 1/100 and integrates lengths in cm.
    let metres = (hi - lo) * (NINJA_CM_PER_MAP_UNIT / 100.0);
    let optical_depth = sample.extinction * metres;
    return NinjaFogSegment(optical_depth, sample.albedo * optical_depth, sample.emission * metres);
}
