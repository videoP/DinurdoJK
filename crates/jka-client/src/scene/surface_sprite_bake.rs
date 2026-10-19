//! Map-prep bake of native surfaceSprites into static per-sprite instances.
//! Vertical/flattened/oriented sprites are seeded from their triangle alone
//! (Raven tr_surfacesprites.cpp), so the whole map is generated once on the
//! map job pool; the renderer only uploads the result and culls chunks.
use super::SurfaceSpriteEffectEmitter;
use crate::map_jobs::MapJobPool;
use crate::thread_activity::Task;
use glam::Vec3;
use std::ops::Range;

/// Baked instances plus their chunk index, in emitter order.
#[derive(Clone, Debug, Default)]
pub struct SurfaceSpriteBake {
    pub instances: Vec<SurfaceSpriteInstance>,
    pub chunks: Vec<SurfaceSpriteChunk>,
    /// Chunk range per emitter (same order as `PreparedMap::surface_sprite_effects`);
    /// empty for the effect kind and for emitters past the instance cap.
    pub emitter_chunks: Vec<Range<u32>>,
}

/// One emitter's sorted instances and chunks, with chunk ranges local to it.
type EmitterBake = (Vec<SurfaceSpriteInstance>, Vec<SurfaceSpriteChunk>);

fn bake_emitter(emitter: &SurfaceSpriteEffectEmitter, index: u32, floor_z: f32) -> EmitterBake {
    let mut scratch = Vec::new();
    let mut instances = Vec::new();
    let mut chunks = Vec::new();
    if !matches!(emitter.sprite.kind, jka_assets::shader::SurfaceSpriteType::Effect) {
        build_vegetation_instances(emitter, index, &mut scratch);
        // Filter after generation: Raven's RNG walks per triangle, so dropping
        // instances here cannot shift the surviving ones.
        scratch.retain(|instance| instance.position_spread[2] >= floor_z);
        bucket_resident_instances(emitter, &mut scratch, &mut instances, &mut chunks);
    }
    (instances, chunks)
}

/// Bakes every emitter, split into one contiguous emitter group per worker.
/// Runs as general map prep (no loading-bar row of its own).
/// Returns the bake and the summed worker CPU time in milliseconds.
/// Sprites planted below `floor_z` (JKA height) are dropped; pass
/// `f32::NEG_INFINITY` to keep them all.
pub fn bake_surface_sprites(
    emitters: &[SurfaceSpriteEffectEmitter],
    jobs: Option<&MapJobPool>,
    floor_z: f32,
) -> Result<(SurfaceSpriteBake, f64), String> {
    let mut per_emitter: Vec<EmitterBake> = Vec::with_capacity(emitters.len());
    let mut cpu_ms = 0.0;
    match jobs.filter(|_| emitters.len() > 1) {
        Some(jobs) => {
            let workers = jobs.worker_count().min(emitters.len()).max(1);
            let group = emitters.len().div_ceil(workers);
            let mut handles = Vec::new();
            for (group_index, chunk) in emitters.chunks(group).enumerate() {
                let chunk = chunk.to_vec();
                let first = (group_index * group) as u32;
                handles.push(jobs.submit(Task::MapPrepare, move || {
                    let started = std::time::Instant::now();
                    let baked: Vec<EmitterBake> = chunk.iter().enumerate()
                        .map(|(offset, emitter)| bake_emitter(emitter, first + offset as u32, floor_z))
                        .collect();
                    (baked, started.elapsed().as_secs_f64() * 1000.0)
                })?);
            }
            for handle in handles {
                let (mut baked, ms) = handle.join()?;
                cpu_ms += ms;
                per_emitter.append(&mut baked);
            }
        }
        None => {
            let started = std::time::Instant::now();
            per_emitter.extend(emitters.iter().enumerate()
                .map(|(index, emitter)| bake_emitter(emitter, index as u32, floor_z)));
            cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
    }
    let mut bake = SurfaceSpriteBake::default();
    for (mut instances, chunks) in per_emitter {
        let first_chunk = bake.chunks.len() as u32;
        if bake.instances.len() + instances.len() <= MAX_RESIDENT_INSTANCES {
            let base = bake.instances.len() as u32;
            bake.instances.append(&mut instances);
            bake.chunks.extend(chunks.into_iter().map(|mut chunk| {
                chunk.instances = chunk.instances.start + base..chunk.instances.end + base;
                chunk
            }));
        }
        bake.emitter_chunks.push(first_chunk..bake.chunks.len() as u32);
    }
    Ok((bake, cpu_ms))
}

/// Raven's fixed deterministic table from tr_surfacesprites.cpp. Keeping this
/// verbatim is important: surfaceSprites positions are seeded from triangle
/// coordinates and shader time, so replacing it with a modern RNG visibly moves
/// every splash/particle on legacy maps.
pub const SURFACE_SPRITE_RANDOM: [f32; 256] = [
    0.6554, 0.6909, 0.4806, 0.6218, 0.5717, 0.3896, 0.0677, 0.7356, 0.8333, 0.1105, 0.4445, 0.8161,
    0.4689, 0.0433, 0.7152, 0.0336, 0.0186, 0.9140, 0.1626, 0.6553, 0.8340, 0.7094, 0.2020, 0.8087,
    0.9119, 0.8009, 0.1339, 0.8492, 0.9173, 0.5003, 0.6012, 0.6117, 0.5525, 0.5787, 0.1586, 0.3293,
    0.9273, 0.7791, 0.8589, 0.4985, 0.0883, 0.8545, 0.2634, 0.4727, 0.3624, 0.1631, 0.7825, 0.0662,
    0.6704, 0.3510, 0.7525, 0.9486, 0.4685, 0.1535, 0.1545, 0.1121, 0.4724, 0.8483, 0.3833, 0.1917,
    0.8207, 0.3885, 0.9702, 0.9200, 0.8348, 0.7501, 0.6675, 0.4994, 0.0301, 0.5225, 0.8011, 0.1696,
    0.5351, 0.2752, 0.2962, 0.7550, 0.5762, 0.7303, 0.2835, 0.4717, 0.1818, 0.2739, 0.6914, 0.7748,
    0.7640, 0.8355, 0.7314, 0.5288, 0.7340, 0.6692, 0.6813, 0.2810, 0.8057, 0.0648, 0.8749, 0.9199,
    0.1462, 0.5237, 0.3014, 0.4994, 0.0278, 0.4268, 0.7238, 0.5107, 0.1378, 0.7303, 0.7200, 0.3819,
    0.2034, 0.7157, 0.5552, 0.4887, 0.0871, 0.3293, 0.2892, 0.4545, 0.0088, 0.1404, 0.0275, 0.0238,
    0.0515, 0.4494, 0.7206, 0.2893, 0.6060, 0.5785, 0.4182, 0.5528, 0.9118, 0.8742, 0.3859, 0.6030,
    0.3495, 0.4550, 0.9875, 0.6900, 0.6416, 0.2337, 0.7431, 0.9788, 0.6181, 0.2464, 0.4661, 0.7621,
    0.7020, 0.8203, 0.8869, 0.2145, 0.7724, 0.6093, 0.6692, 0.9686, 0.5609, 0.0310, 0.2248, 0.2950,
    0.2365, 0.1347, 0.2342, 0.1668, 0.3378, 0.4330, 0.2775, 0.9901, 0.7053, 0.7266, 0.4840, 0.2820,
    0.5733, 0.4555, 0.6049, 0.0770, 0.4760, 0.6060, 0.4159, 0.3427, 0.1234, 0.7062, 0.8569, 0.1878,
    0.9057, 0.9399, 0.8139, 0.1407, 0.1794, 0.9123, 0.9493, 0.2827, 0.9934, 0.0952, 0.4879, 0.5160,
    0.4118, 0.4873, 0.3642, 0.7470, 0.0866, 0.5172, 0.6365, 0.2676, 0.2407, 0.7223, 0.5761, 0.1143,
    0.7137, 0.2342, 0.3353, 0.6880, 0.2296, 0.6023, 0.6027, 0.4138, 0.5408, 0.9859, 0.1503, 0.7238,
    0.6054, 0.2477, 0.6804, 0.1432, 0.4540, 0.9776, 0.8762, 0.7607, 0.9025, 0.9807, 0.0652, 0.8661,
    0.7663, 0.2586, 0.3994, 0.0335, 0.7328, 0.0166, 0.9589, 0.4348, 0.5493, 0.7269, 0.6867, 0.6614,
    0.6800, 0.7804, 0.5591, 0.8381, 0.0910, 0.7573, 0.8985, 0.3083, 0.3188, 0.8481, 0.2356, 0.6736,
    0.4770, 0.4560, 0.6266, 0.4677,
];

/// One static vegetation sprite (vertical/flattened/oriented). Raven seeds
/// placement, size and fade threshold from the triangle alone, so these are
/// generated once at map load; only fade, width scaling, facing and wind
/// depend on the camera/time and are evaluated in `surface_sprite_resident.wgsl`
/// (renderer/environment/surface_sprites.rs uploads them).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Debug, PartialEq)]
pub struct SurfaceSpriteInstance {
    /// JKA position, then `sum(bary_k * |v_k - p|^2)` so the shader reproduces
    /// Raven's barycentric blend of per-vertex squared camera distances exactly.
    pub position_spread: [f32; 4],
    /// Signed variance-scaled width, height, fade start, baked light (0..255).
    pub size_fade_light: [f32; 4],
    pub skew: [f32; 2],
    pub right_index: u32,
    pub emitter: u32,
}

/// Spatial bucket of one emitter's resident instances.
#[derive(Clone, Debug)]
pub struct SurfaceSpriteChunk {
    pub instances: std::ops::Range<u32>,
    /// Conservative JKA bounds of every quad the chunk can produce.
    pub min_jka: [f32; 3],
    pub max_jka: [f32; 3],
}

/// Instances past this stay on the CPU path (48 bytes each).
pub const MAX_RESIDENT_INSTANCES: usize = 4 * 1024 * 1024;
pub const RESIDENT_CHUNK_SIZE: f32 = 512.0;

/// The static half of `append_jka_vegetation_surface_sprites`: the same
/// triangle walk and Raven RNG sequence, without the camera-dependent fade test
/// (a faded sprite never advances the RNG differently).
pub fn build_vegetation_instances(
    emitter: &SurfaceSpriteEffectEmitter,
    emitter_index: u32,
    out: &mut Vec<SurfaceSpriteInstance>,
) {
    use jka_assets::shader::SurfaceSpriteFacing;
    const FADE_RANGE: f32 = 250.0;
    const MAX_SPRITES_PER_TRIANGLE: usize = 16384;
    const MAX_CELLS_PER_TRIANGLE: usize = 65536;
    let sprite = emitter.sprite;
    if !sprite.width.is_finite() || !sprite.height.is_finite()
        || !sprite.density.is_finite() || sprite.density <= 0.0
        || sprite.width <= 0.0 || sprite.height <= 0.0
    {
        return;
    }
    let fadedist = sprite.fade_dist.max(0.0);
    let cutdist = sprite.fade_max.max(fadedist + 0.001);
    let fade_range = (FADE_RANGE / (cutdist - fadedist)).min(1.0);
    let hang_down = matches!(sprite.facing, SurfaceSpriteFacing::HangDown);
    let any_angle = matches!(sprite.facing, SurfaceSpriteFacing::AnyAngle);
    for triangle in &emitter.triangles {
        let valid_slope = triangle.normal_z.iter().all(|&nz| {
            if any_angle { true }
            else if hang_down { nz <= -0.5 }
            else { nz >= 0.5 }
        });
        if !valid_slope {
            continue;
        }
        let v = triangle.positions_jka.map(Vec3::from_array);
        let v12 = v[1] - v[0];
        let v13 = v[2] - v[0];
        let triarea = (v13.x * v12.y - v13.y * v12.x).abs();
        if !triarea.is_finite() || triarea <= 1.0 {
            continue;
        }
        let step = sprite.density / triarea.sqrt();
        if !step.is_finite() || step <= 0.0 {
            continue;
        }
        let mut random_index = surface_sprite_byte(
            v[0].x + v[0].y + v[1].x + v[1].y + v[2].x + v[2].y
        );
        let random_interval = surface_sprite_byte(v[0].x + v[1].y + v[2].z) | 0x03;
        let mut right_index = 0usize;
        let mut sprites = 0usize;
        let mut cells = 0usize;
        let mut pos_i = 0.0_f32;
        while pos_i < 1.0 && sprites < MAX_SPRITES_PER_TRIANGLE && cells < MAX_CELLS_PER_TRIANGLE {
            let mut pos_j = 0.0_f32;
            while pos_j < 1.0 - pos_i && sprites < MAX_SPRITES_PER_TRIANGLE && cells < MAX_CELLS_PER_TRIANGLE {
                cells += 1;
                let fa = pos_i + surface_sprite_random(random_index) * step;
                random_index = random_index.wrapping_add(random_interval);
                let fb = pos_j + surface_sprite_random(random_index) * step;
                random_index = random_index.wrapping_add(random_interval);
                right_index = (right_index + 1) & 3;
                if fa <= 1.0 && fb <= 1.0 - fa {
                    let fc = 1.0 - fa - fb;
                    let fade_start = fade_range
                        + (1.0 - fade_range) * surface_sprite_random(random_index);
                    random_index = random_index.wrapping_add(random_interval);
                    let position = v[0] * fa + v[1] * fb + v[2] * fc;
                    let spread = fa * (v[0] - position).length_squared()
                        + fb * (v[1] - position).length_squared()
                        + fc * (v[2] - position).length_squared();
                    let light = f32::from(triangle.light[0]) * fa
                        + f32::from(triangle.light[1]) * fb
                        + f32::from(triangle.light[2]) * fc;
                    let mut size_index = random_index;
                    let variance = surface_sprite_random(size_index);
                    let mut width = sprite.width * (1.0 + sprite.variance[0] * variance);
                    let height = sprite.height * (1.0 + sprite.variance[1] * variance);
                    size_index = size_index.wrapping_add(1);
                    if surface_sprite_random(size_index) > 0.5 {
                        width = -width;
                    }
                    size_index = size_index.wrapping_add(1);
                    let skew = if sprite.vert_skew != 0.0 {
                        let x = surface_sprite_random(size_index) * 2.0 - 1.0;
                        size_index = size_index.wrapping_add(1);
                        let y = surface_sprite_random(size_index) * 2.0 - 1.0;
                        [x * height * sprite.vert_skew, y * height * sprite.vert_skew]
                    } else {
                        [0.0, 0.0]
                    };
                    out.push(SurfaceSpriteInstance {
                        position_spread: [position.x, position.y, position.z, spread],
                        size_fade_light: [width, height, fade_start, light],
                        skew,
                        right_index: right_index as u32,
                        emitter: emitter_index,
                    });
                    sprites += 1;
                }
                pos_j += step;
            }
            pos_i += step;
        }
    }
}

/// Sorts one emitter's instances into XY cells and appends them with their
/// chunk records. Bounds cover the widest quad: ssFadescale widening, height,
/// skew, idle sway and wind bend.
pub(crate) fn bucket_resident_instances(
    emitter: &SurfaceSpriteEffectEmitter,
    scratch: &mut [SurfaceSpriteInstance],
    instances: &mut Vec<SurfaceSpriteInstance>,
    chunks: &mut Vec<SurfaceSpriteChunk>,
) {
    let cell = |instance: &SurfaceSpriteInstance| {
        let [x, y, ..] = instance.position_spread;
        ((x / RESIDENT_CHUNK_SIZE).floor() as i32, (y / RESIDENT_CHUNK_SIZE).floor() as i32)
    };
    scratch.sort_by_key(cell);
    let sprite = emitter.sprite;
    let widen = 1.0 + sprite.fade_scale.abs();
    let mut start = 0;
    while start < scratch.len() {
        let key = cell(&scratch[start]);
        let end = start + scratch[start..].iter().take_while(|instance| cell(instance) == key).count();
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for instance in &scratch[start..end] {
            let [width, height, ..] = instance.size_fade_light;
            let reach = width.abs() * widen + height * (1.0 + sprite.vert_skew.abs() * 2.0
                + sprite.wind.abs() * 0.4 + sprite.wind_idle.abs() * 0.1);
            let p = Vec3::from_slice(&instance.position_spread[..3]);
            min = min.min(p - Vec3::splat(reach));
            max = max.max(p + Vec3::splat(reach));
        }
        let first = instances.len() as u32;
        instances.extend_from_slice(&scratch[start..end]);
        chunks.push(SurfaceSpriteChunk {
            instances: first..instances.len() as u32,
            min_jka: min.to_array(),
            max_jka: max.to_array(),
        });
        start = end;
    }
}

pub fn surface_sprite_random(index: u8) -> f32 {
    SURFACE_SPRITE_RANDOM[index as usize]
}

pub fn surface_sprite_byte(value: f32) -> u8 {
    ((value.trunc() as i64).rem_euclid(256)) as u8
}


#[cfg(test)]
mod tests {
    use super::*;
    use jka_assets::shader::{SurfaceSprite, SurfaceSpriteFacing, SurfaceSpriteType};

    fn emitter(kind: SurfaceSpriteType, x: f32) -> SurfaceSpriteEffectEmitter {
        SurfaceSpriteEffectEmitter {
            texture: 0,
            clamp: false,
            blend: None,
            sprite: SurfaceSprite {
                kind, width: 48.0, height: 24.0, density: 45.0, fade_dist: 400.0,
                fade_max: 3000.0, fade_scale: 2.5, variance: [1.0, 2.0], wind: 0.8,
                wind_idle: 0.0, vert_skew: 0.0, fx_duration: 1000.0, fx_grow: [0.0; 2],
                fx_alpha_start: 1.0, fx_alpha_end: 0.0, facing: SurfaceSpriteFacing::Normal,
            },
            alpha_cutoff: 0.75,
            depth_write: true,
            godot_candidate: false,
            triangles: vec![super::super::SurfaceSpriteEffectTriangle {
                positions_jka: [[x, 0.0, 0.0], [x + 1200.0, 0.0, 0.0], [x, 1200.0, 0.0]],
                normal_z: [1.0; 3],
                light: [200; 3],
            }],
            pvs_signature: Vec::new(),
        }
    }

    #[test]
    fn bake_keeps_emitter_order_and_skips_effect_kind() {
        let emitters = vec![
            emitter(SurfaceSpriteType::Vertical, 0.0),
            emitter(SurfaceSpriteType::Effect, 0.0),
            emitter(SurfaceSpriteType::Flattened, 5000.0),
        ];
        let (bake, _) = bake_surface_sprites(&emitters, None, f32::NEG_INFINITY).unwrap();
        assert_eq!(bake.emitter_chunks.len(), 3);
        assert!(bake.emitter_chunks[1].is_empty());
        for (index, range) in bake.emitter_chunks.iter().enumerate() {
            for chunk in &bake.chunks[range.start as usize..range.end as usize] {
                let slice = &bake.instances[chunk.instances.start as usize..chunk.instances.end as usize];
                assert!(!slice.is_empty());
                assert!(slice.iter().all(|instance| instance.emitter == index as u32));
            }
        }
        let mut solo = Vec::new();
        build_vegetation_instances(&emitters[2], 2, &mut solo);
        let range = &bake.emitter_chunks[2];
        let count: u32 = bake.chunks[range.start as usize..range.end as usize].iter()
            .map(|chunk| chunk.instances.end - chunk.instances.start).sum();
        assert_eq!(count as usize, solo.len());
    }
}
