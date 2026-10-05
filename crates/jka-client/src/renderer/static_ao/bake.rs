//! Static ao bake.
use crate::renderer::{
    bake_static_structural_vertex_ao, dilate_static_ao_mask, dilate_static_ao_owners, fs, mpsc,
    rasterize_static_ao_owners, rasterize_static_ao_texels, spread_static_ao_mask,
    static_ao_cache_path, static_ao_hq_visibility, static_ao_sample_pattern,
    static_ao_worker_count, thread, Arc, BTreeSet, DrawBatch, DrawClass, GpuVertex, Instant,
    OnceLock, StaticAoBakeData, StaticAoBakedLightmap, StaticAoBaseLightmap, StaticAoBvh,
    StaticAoJobKey, StaticAoLightmapTriangle, StaticAoRenderTriangle, StaticAoTexelSample,
    StaticAoWorkerResult, StaticAoWorkerUpdate, StaticAoWorldSource, Vec3,
    STATIC_AO_ADAPTIVE_MAX_CENTER_ERROR, STATIC_AO_ADAPTIVE_MAX_VISIBILITY_RANGE,
    STATIC_AO_ADAPTIVE_MIN_NORMAL_DOT,
};

#[derive(Clone, Copy)]
pub(in crate::renderer) struct StaticAoAdaptiveCell {
    pub(in crate::renderer) x0: u32,
    pub(in crate::renderer) y0: u32,
    pub(in crate::renderer) x1: u32,
    pub(in crate::renderer) y1: u32,
}

pub(in crate::renderer) fn static_ao_cell_has_coverage(
    texels: &[Option<StaticAoTexelSample>],
    width: u32,
    cell: StaticAoAdaptiveCell,
) -> bool {
    for y in cell.y0..cell.y1 {
        let row = y as usize * width as usize;
        for x in cell.x0..cell.x1 {
            if texels[row + x as usize].is_some() {
                return true;
            }
        }
    }
    false
}

pub(in crate::renderer) fn static_ao_cell_points(
    width: u32,
    cell: StaticAoAdaptiveCell,
) -> [usize; 9] {
    let x0 = cell.x0;
    let y0 = cell.y0;
    let x1 = cell.x1.saturating_sub(1);
    let y1 = cell.y1.saturating_sub(1);
    let xc = (x0 + x1) / 2;
    let yc = (y0 + y1) / 2;
    let index = |x: u32, y: u32| y as usize * width as usize + x as usize;
    [
        index(x0, y0),
        index(x1, y0),
        index(x0, y1),
        index(x1, y1),
        index(xc, y0),
        index(xc, y1),
        index(x0, yc),
        index(x1, yc),
        index(xc, yc),
    ]
}

pub(in crate::renderer) fn static_ao_trace_adaptive_indices(
    indices: &[usize],
    mask: &mut [u8],
    sampled: &mut [u8],
    texels: &[Option<StaticAoTexelSample>],
    bvh: Arc<StaticAoBvh>,
    sample_pattern: Arc<Vec<[f32; 2]>>,
    range_scale: f32,
    seed_base: usize,
    workers: usize,
) {
    if indices.is_empty() {
        return;
    }
    let mut results = vec![255u8; indices.len()];
    let chunk_size = indices.len().div_ceil(workers).max(1);
    thread::scope(|scope| {
        for (result_chunk, index_chunk) in results
            .chunks_mut(chunk_size)
            .zip(indices.chunks(chunk_size))
        {
            let bvh = Arc::clone(&bvh);
            let sample_pattern = Arc::clone(&sample_pattern);
            scope.spawn(move || {
                for (result, &index) in result_chunk.iter_mut().zip(index_chunk.iter()) {
                    if let Some(sample) = texels[index] {
                        *result = static_ao_hq_visibility(
                            &bvh,
                            sample.position,
                            sample.normal,
                            seed_base.wrapping_add(index),
                            &sample_pattern,
                            range_scale,
                        );
                    }
                }
            });
        }
    });
    for (&index, &visibility) in indices.iter().zip(results.iter()) {
        mask[index] = visibility;
        sampled[index] = 1;
    }
}

pub(in crate::renderer) fn static_ao_adaptive_cell_smooth(
    cell: StaticAoAdaptiveCell,
    width: u32,
    texels: &[Option<StaticAoTexelSample>],
    owners: &[u32],
    mask: &[u8],
    sampled: &[u8],
) -> bool {
    let points = static_ao_cell_points(width, cell);
    let mut owner = 0u32;
    let mut center_normal = Vec3::ZERO;
    let mut min_visibility = u8::MAX;
    let mut max_visibility = 0u8;
    for (point_index, &index) in points.iter().enumerate() {
        let Some(sample) = texels[index] else {
            return false;
        };
        if sampled[index] == 0 || owners[index] == 0 {
            return false;
        }
        if owner == 0 {
            owner = owners[index];
        } else if owners[index] != owner {
            return false;
        }
        let normal = Vec3::from_array(sample.normal).normalize_or_zero();
        if normal.length_squared() < 0.5 {
            return false;
        }
        if point_index == 8 {
            center_normal = normal;
        }
        min_visibility = min_visibility.min(mask[index]);
        max_visibility = max_visibility.max(mask[index]);
    }
    if max_visibility.saturating_sub(min_visibility) > STATIC_AO_ADAPTIVE_MAX_VISIBILITY_RANGE {
        return false;
    }
    for &index in &points[..8] {
        let normal = Vec3::from_array(texels[index].unwrap().normal).normalize_or_zero();
        if center_normal.dot(normal) < STATIC_AO_ADAPTIVE_MIN_NORMAL_DOT {
            return false;
        }
    }
    let corner_average = ((u32::from(mask[points[0]])
        + u32::from(mask[points[1]])
        + u32::from(mask[points[2]])
        + u32::from(mask[points[3]])
        + 2)
        / 4) as u8;
    mask[points[8]].abs_diff(corner_average) <= STATIC_AO_ADAPTIVE_MAX_CENTER_ERROR
}

pub(in crate::renderer) fn static_ao_fill_adaptive_cell(
    cell: StaticAoAdaptiveCell,
    width: u32,
    texels: &[Option<StaticAoTexelSample>],
    owners: &[u32],
    mask: &mut [u8],
    sampled: &mut [u8],
) {
    let points = static_ao_cell_points(width, cell);
    let owner = owners[points[8]];
    let v00 = f32::from(mask[points[0]]);
    let v10 = f32::from(mask[points[1]]);
    let v01 = f32::from(mask[points[2]]);
    let v11 = f32::from(mask[points[3]]);
    let vc = f32::from(mask[points[8]]);
    let center_bilinear = (v00 + v10 + v01 + v11) * 0.25;
    let center_correction = vc - center_bilinear;
    let span_x = cell.x1.saturating_sub(cell.x0 + 1).max(1) as f32;
    let span_y = cell.y1.saturating_sub(cell.y0 + 1).max(1) as f32;
    for y in cell.y0..cell.y1 {
        let ty = (y.saturating_sub(cell.y0) as f32 / span_y).clamp(0.0, 1.0);
        for x in cell.x0..cell.x1 {
            let index = y as usize * width as usize + x as usize;
            if texels[index].is_none() || owners[index] != owner {
                continue;
            }
            if sampled[index] != 0 {
                continue;
            }
            let tx = (x.saturating_sub(cell.x0) as f32 / span_x).clamp(0.0, 1.0);
            let top = v00 + (v10 - v00) * tx;
            let bottom = v01 + (v11 - v01) * tx;
            let mut value = top + (bottom - top) * ty;
            let center_weight = (4.0 * tx * (1.0 - tx)) * (4.0 * ty * (1.0 - ty));
            value += center_correction * center_weight;
            mask[index] = value.clamp(0.0, 255.0).round() as u8;
            sampled[index] = 2;
        }
    }
}

pub(in crate::renderer) fn static_ao_subdivide_cell(
    cell: StaticAoAdaptiveCell,
    output: &mut Vec<StaticAoAdaptiveCell>,
) {
    let width = cell.x1 - cell.x0;
    let height = cell.y1 - cell.y0;
    if width <= 1 && height <= 1 {
        return;
    }
    let xm = if width > 1 {
        cell.x0 + width / 2
    } else {
        cell.x1
    };
    let ym = if height > 1 {
        cell.y0 + height / 2
    } else {
        cell.y1
    };
    let xs = if width > 1 {
        [(cell.x0, xm), (xm, cell.x1)]
    } else {
        [(cell.x0, cell.x1), (cell.x1, cell.x1)]
    };
    let ys = if height > 1 {
        [(cell.y0, ym), (ym, cell.y1)]
    } else {
        [(cell.y0, cell.y1), (cell.y1, cell.y1)]
    };
    for &(y0, y1) in &ys {
        if y0 >= y1 {
            continue;
        }
        for &(x0, x1) in &xs {
            if x0 < x1 {
                output.push(StaticAoAdaptiveCell { x0, y0, x1, y1 });
            }
        }
    }
}

pub(in crate::renderer) fn bake_static_lightmap_texture_adaptive(
    texels: &[Option<StaticAoTexelSample>],
    owners: &[u32],
    width: u32,
    height: u32,
    scale: u32,
    bvh: Arc<StaticAoBvh>,
    sample_pattern: Arc<Vec<[f32; 2]>>,
    range_scale: f32,
    seed_base: usize,
    workers: usize,
) -> (Vec<u8>, Vec<u8>, usize) {
    let mut mask = vec![255u8; texels.len()];
    let mut sampled = vec![0u8; texels.len()];
    let root_span = (scale.max(1) * 2).max(2);
    let mut cells = Vec::new();
    let mut y = 0u32;
    while y < height {
        let mut x = 0u32;
        while x < width {
            let cell = StaticAoAdaptiveCell {
                x0: x,
                y0: y,
                x1: (x + root_span).min(width),
                y1: (y + root_span).min(height),
            };
            if static_ao_cell_has_coverage(texels, width, cell) {
                cells.push(cell);
            }
            x += root_span;
        }
        y += root_span;
    }

    while !cells.is_empty() {
        let mut needed = BTreeSet::<usize>::new();
        for &cell in &cells {
            for index in static_ao_cell_points(width, cell) {
                if texels[index].is_some() && sampled[index] == 0 {
                    needed.insert(index);
                }
            }
        }
        let needed = needed.into_iter().collect::<Vec<_>>();
        static_ao_trace_adaptive_indices(
            &needed,
            &mut mask,
            &mut sampled,
            texels,
            Arc::clone(&bvh),
            Arc::clone(&sample_pattern),
            range_scale,
            seed_base,
            workers,
        );

        let mut next_cells = Vec::new();
        for cell in cells.drain(..) {
            let cell_width = cell.x1 - cell.x0;
            let cell_height = cell.y1 - cell.y0;
            if static_ao_adaptive_cell_smooth(cell, width, texels, owners, &mask, &sampled) {
                static_ao_fill_adaptive_cell(cell, width, texels, owners, &mut mask, &mut sampled);
            } else if cell_width > 1 || cell_height > 1 {
                static_ao_subdivide_cell(cell, &mut next_cells);
            }
        }
        cells = next_cells;
    }

    // Any tiny coverage sliver that never landed on a cell's five probes gets an
    // exact sample here. This keeps adaptive sampling conservative at UV borders.
    let remaining = texels
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| (sample.is_some() && sampled[index] == 0).then_some(index))
        .collect::<Vec<_>>();
    static_ao_trace_adaptive_indices(
        &remaining,
        &mut mask,
        &mut sampled,
        texels,
        bvh,
        sample_pattern,
        range_scale,
        seed_base,
        workers,
    );
    let traced = sampled.iter().filter(|&&state| state == 1).count();
    let covered = texels
        .iter()
        .map(|sample| u8::from(sample.is_some()))
        .collect::<Vec<_>>();
    (mask, covered, traced)
}

#[derive(Clone, Copy)]
pub(in crate::renderer) struct StaticAoBakeTile {
    pub(in crate::renderer) texture: usize,
    pub(in crate::renderer) x0: u32,
    pub(in crate::renderer) y0: u32,
    pub(in crate::renderer) x1: u32,
    pub(in crate::renderer) y1: u32,
    pub(in crate::renderer) priority: f32,
}

pub(in crate::renderer) fn static_ao_extract_region<T: Copy>(
    source: &[T],
    source_width: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> Vec<T> {
    let mut result = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
    for y in y0..y1 {
        let start = y as usize * source_width as usize + x0 as usize;
        let end = start + (x1 - x0) as usize;
        result.extend_from_slice(&source[start..end]);
    }
    result
}

pub(in crate::renderer) fn static_ao_tile_priority(
    texels: &[Option<StaticAoTexelSample>],
    width: u32,
    tile: StaticAoBakeTile,
    camera_position: Vec3,
) -> Option<f32> {
    let mut best = f32::INFINITY;
    let mut found = false;
    for y in tile.y0..tile.y1 {
        let row = y as usize * width as usize;
        for x in tile.x0..tile.x1 {
            if let Some(sample) = texels[row + x as usize] {
                found = true;
                best =
                    best.min(Vec3::from_array(sample.position).distance_squared(camera_position));
            }
        }
    }
    found.then_some(best)
}

pub(in crate::renderer) fn bake_static_lightmap_ao(
    source: &StaticAoWorldSource,
    sample_pattern: Arc<Vec<[f32; 2]>>,
    scale: u32,
    range_scale: f32,
    strength: f32,
    bvh: Arc<StaticAoBvh>,
    workers: usize,
    key: StaticAoJobKey,
    camera_position: Vec3,
    updates: &mpsc::Sender<StaticAoWorkerUpdate>,
    progress: &mpsc::Sender<(u32, u32)>,
) -> Result<Vec<Vec<u8>>, String> {
    let scale = match scale {
        1 | 3 | 5 => scale,
        _ => 3,
    };
    let raster_started = Instant::now();
    let texels = rasterize_static_ao_texels(source, scale);
    let owners = rasterize_static_ao_owners(source, scale);
    let mut low_owners = rasterize_static_ao_owners(source, 1);
    let mut baked_high_owners = owners.clone();
    for ((low, high), &[base_width, base_height]) in low_owners
        .iter_mut()
        .zip(baked_high_owners.iter_mut())
        .zip(source.lightmap_sizes.iter())
    {
        dilate_static_ao_owners(low, base_width, base_height, 2);
        dilate_static_ao_owners(
            high,
            base_width.saturating_mul(scale),
            base_height.saturating_mul(scale),
            2 * scale,
        );
    }
    let raster_ms = raster_started.elapsed().as_secs_f64() * 1000.0;
    rverbose!(
        1,
        "Static BSP AO worker: tiled adaptive HQ lightmap raster {:.1} ms at {}x",
        raster_ms,
        scale
    );

    let mut masks = source
        .lightmap_sizes
        .iter()
        .map(|&[w, h]| {
            vec![255u8; w.saturating_mul(scale) as usize * h.saturating_mul(scale) as usize]
        })
        .collect::<Vec<_>>();
    let mut jobs = Vec::<StaticAoBakeTile>::new();
    let tile_span = (32 * scale).max(32);
    for (texture, texel_samples) in texels.iter().enumerate() {
        let [base_width, base_height] = source.lightmap_sizes[texture];
        let width = base_width.saturating_mul(scale);
        let height = base_height.saturating_mul(scale);
        let mut y = 0u32;
        while y < height {
            let mut x = 0u32;
            while x < width {
                let mut tile = StaticAoBakeTile {
                    texture,
                    x0: x,
                    y0: y,
                    x1: (x + tile_span).min(width),
                    y1: (y + tile_span).min(height),
                    priority: f32::INFINITY,
                };
                if let Some(priority) =
                    static_ao_tile_priority(texel_samples, width, tile, camera_position)
                {
                    tile.priority = priority;
                    jobs.push(tile);
                }
                x += tile_span;
            }
            y += tile_span;
        }
    }
    jobs.sort_by(|a, b| a.priority.total_cmp(&b.priority));
    if key.current_cell_only && jobs.len() > 1 {
        jobs.truncate(1);
    }
    rverbose!(
        1,
        "Static BSP AO worker: {} lightmap tile(s), nearest-first from [{:.1}, {:.1}, {:.1}]{}",
        jobs.len(),
        camera_position.x,
        camera_position.y,
        camera_position.z,
        if key.current_cell_only {
            " (current cell only)"
        } else {
            ""
        }
    );

    let mut traced_per_texture = vec![0usize; source.lightmap_sizes.len()];
    let mut covered_per_texture = vec![0usize; source.lightmap_sizes.len()];
    for (texture, texel_samples) in texels.iter().enumerate() {
        covered_per_texture[texture] = texel_samples
            .iter()
            .filter(|sample| sample.is_some())
            .count();
    }

    let total_jobs = jobs.len().max(1);
    for (job_index, tile) in jobs.into_iter().enumerate() {
        let [base_width, base_height] = source.lightmap_sizes[tile.texture];
        let full_width = base_width.saturating_mul(scale);
        let full_height = base_height.saturating_mul(scale);
        // Include a halo so the artificial AO shoulder and adaptive decisions are
        // stable across tile boundaries. Only the central tile is streamed/uploaded.
        let halo = 2 * scale.max(1);
        let ex0 = tile.x0.saturating_sub(halo);
        let ey0 = tile.y0.saturating_sub(halo);
        let ex1 = tile.x1.saturating_add(halo).min(full_width);
        let ey1 = tile.y1.saturating_add(halo).min(full_height);
        let local_width = ex1 - ex0;
        let local_height = ey1 - ey0;
        let local_texels =
            static_ao_extract_region(&texels[tile.texture], full_width, ex0, ey0, ex1, ey1);
        let local_owners =
            static_ao_extract_region(&owners[tile.texture], full_width, ex0, ey0, ex1, ey1);
        let seed_base = tile
            .texture
            .wrapping_mul(0x100000)
            .wrapping_add(ey0 as usize * full_width as usize + ex0 as usize);
        let (mut local_mask, mut local_covered, traced) = bake_static_lightmap_texture_adaptive(
            &local_texels,
            &local_owners,
            local_width,
            local_height,
            scale,
            Arc::clone(&bvh),
            Arc::clone(&sample_pattern),
            range_scale,
            seed_base,
            workers,
        );
        dilate_static_ao_mask(
            &mut local_mask,
            &mut local_covered,
            local_width,
            local_height,
            scale,
        );
        spread_static_ao_mask(
            &mut local_mask,
            &local_covered,
            &local_owners,
            local_width,
            local_height,
            scale,
        );
        traced_per_texture[tile.texture] += traced;

        for y in tile.y0..tile.y1 {
            let local_y = y - ey0;
            for x in tile.x0..tile.x1 {
                let local_x = x - ex0;
                let local_index = local_y as usize * local_width as usize + local_x as usize;
                let global_index = y as usize * full_width as usize + x as usize;
                masks[tile.texture][global_index] = local_mask[local_index];
            }
        }

        let rgba = static_ao_baked_lightmap_tile(
            &source.lightmap_bases[tile.texture],
            &masks[tile.texture],
            &low_owners[tile.texture],
            &baked_high_owners[tile.texture],
            scale,
            strength,
            tile.x0,
            tile.y0,
            tile.x1,
            tile.y1,
        );
        let _ = updates.send(StaticAoWorkerUpdate::LightmapTile {
            key,
            texture: tile.texture,
            x: tile.x0,
            y: tile.y0,
            width: tile.x1 - tile.x0,
            height: tile.y1 - tile.y0,
            rgba,
        });
        let completed = 45 + (((job_index + 1) * 45) / total_jobs) as u32;
        let _ = progress.send((completed.min(90), 100));
    }

    for texture in 0..source.lightmap_sizes.len() {
        let traced_percent = if covered_per_texture[texture] == 0 {
            0.0
        } else {
            traced_per_texture[texture] as f64 * 100.0 / covered_per_texture[texture] as f64
        };
        rverbose!(
            2,
            "Static BSP AO worker: tiled adaptive HQ lightmap {}/{} covered {} texels, traced {} ({:.1}%) at {}x x {} rays",
            texture + 1,
            source.lightmap_sizes.len(),
            covered_per_texture[texture],
            traced_per_texture[texture],
            traced_percent,
            scale,
            sample_pattern.len(),
        );
    }
    Ok(masks)
}

pub(in crate::renderer) fn static_ao_srgb_decode_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|index| {
            let encoded = index as f32 / 255.0;
            if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

pub(in crate::renderer) fn static_ao_srgb_encode_lut() -> &'static [u8; 65_536] {
    static LUT: OnceLock<[u8; 65_536]> = OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|index| {
            let linear = index as f32 / 65_535.0;
            let encoded = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            (encoded.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        })
    })
}

#[inline]
pub(in crate::renderer) fn static_ao_srgb_decode(value: u8) -> f32 {
    static_ao_srgb_decode_lut()[value as usize]
}

#[inline]
pub(in crate::renderer) fn static_ao_srgb_encode(linear: f32) -> u8 {
    let index = (linear.clamp(0.0, 1.0) * 65_535.0 + 0.5) as usize;
    static_ao_srgb_encode_lut()[index]
}

pub(in crate::renderer) fn static_ao_baked_pixel(
    base: &StaticAoBaseLightmap,
    low_owners: &[u32],
    high_owners: &[u32],
    scale: u32,
    x: u32,
    y: u32,
    visibility: u8,
    strength: f32,
) -> [u8; 4] {
    let scale = scale.max(1);
    let factor = 1.0 - strength * (1.0 - f32::from(visibility) / 255.0);
    if scale == 1 {
        let index = (y as usize * base.width as usize + x as usize) * 4;
        let mut out = [0u8; 4];
        for channel in 0..3 {
            let original = if base.srgb {
                static_ao_srgb_decode(base.rgba[index + channel])
            } else {
                f32::from(base.rgba[index + channel]) / 255.0
            };
            let baked = (original * factor).min(original);
            out[channel] = if base.srgb {
                static_ao_srgb_encode(baked)
            } else {
                (baked.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            };
        }
        out[3] = 255;
        return out;
    }

    let src_width = base.width as i32;
    let src_height = base.height as i32;
    let source_y = (y as f32 + 0.5) / scale as f32 - 0.5;
    let y_floor = source_y.floor() as i32;
    let fy = source_y - y_floor as f32;
    let y0 = y_floor.clamp(0, src_height - 1);
    let y1 = (y_floor + 1).clamp(0, src_height - 1);
    let source_x = (x as f32 + 0.5) / scale as f32 - 0.5;
    let x_floor = source_x.floor() as i32;
    let fx = source_x - x_floor as f32;
    let x0 = x_floor.clamp(0, src_width - 1);
    let x1 = (x_floor + 1).clamp(0, src_width - 1);
    let source_index = |sx: i32, sy: i32| (sy as usize * base.width as usize + sx as usize) * 4;
    let i00 = source_index(x0, y0);
    let i10 = source_index(x1, y0);
    let i01 = source_index(x0, y1);
    let i11 = source_index(x1, y1);
    let high_width = base.width.saturating_mul(scale) as usize;
    let owner = high_owners[y as usize * high_width + x as usize];
    let sample_owner =
        |sx: i32, sy: i32| -> u32 { low_owners[sy as usize * base.width as usize + sx as usize] };
    let weights = [
        ((1.0 - fx) * (1.0 - fy), i00, sample_owner(x0, y0)),
        (fx * (1.0 - fy), i10, sample_owner(x1, y0)),
        ((1.0 - fx) * fy, i01, sample_owner(x0, y1)),
        (fx * fy, i11, sample_owner(x1, y1)),
    ];
    let decode = |value| {
        if base.srgb {
            static_ao_srgb_decode(value)
        } else {
            f32::from(value) / 255.0
        }
    };
    let mut out = [0u8; 4];
    for channel in 0..3 {
        let mut original = 0.0f32;
        let mut weight_sum = 0.0f32;
        for (weight, index, sample_owner) in weights {
            if weight <= 0.0 {
                continue;
            }
            if owner == 0 || sample_owner == owner {
                original += decode(base.rgba[index + channel]) * weight;
                weight_sum += weight;
            }
        }
        if weight_sum <= 1.0e-6 {
            for (weight, index, _) in weights {
                if weight <= 0.0 {
                    continue;
                }
                original += decode(base.rgba[index + channel]) * weight;
                weight_sum += weight;
            }
        }
        original /= weight_sum.max(1.0e-6);
        let baked = (original * factor).min(original);
        out[channel] = if base.srgb {
            static_ao_srgb_encode(baked)
        } else {
            (baked.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        };
    }
    out[3] = 255;
    out
}

pub(in crate::renderer) fn static_ao_baked_lightmap(
    base: &StaticAoBaseLightmap,
    mask: &[u8],
    low_owners: &[u32],
    high_owners: &[u32],
    scale: u32,
    strength: f32,
) -> Result<StaticAoBakedLightmap, String> {
    let scale = scale.max(1);
    let width = base.width.saturating_mul(scale);
    let height = base.height.saturating_mul(scale);
    let pixels = width as usize * height as usize;
    if mask.len() != pixels
        || high_owners.len() != pixels
        || low_owners.len() != base.width as usize * base.height as usize
        || base.rgba.len() < base.width as usize * base.height as usize * 4
    {
        return Err("lightmap AO cache dimensions do not match current lightmap".into());
    }
    let mut rgba = vec![255u8; pixels * 4];
    for y in 0..height {
        for x in 0..width {
            let index = y as usize * width as usize + x as usize;
            let pixel = static_ao_baked_pixel(
                base,
                low_owners,
                high_owners,
                scale,
                x,
                y,
                mask[index],
                strength,
            );
            rgba[index * 4..index * 4 + 4].copy_from_slice(&pixel);
        }
    }
    Ok(StaticAoBakedLightmap {
        label: if scale == 1 {
            format!("{} + cached HQ AO", base.label)
        } else {
            format!("{} + cached HQ AO {}x", base.label, scale)
        },
        width,
        height,
        rgba,
        clamp: base.clamp,
        srgb: base.srgb,
    })
}

pub(in crate::renderer) fn static_ao_baked_lightmap_tile(
    base: &StaticAoBaseLightmap,
    mask: &[u8],
    low_owners: &[u32],
    high_owners: &[u32],
    scale: u32,
    strength: f32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> Vec<u8> {
    let scale = scale.max(1);
    let width = base.width.saturating_mul(scale);
    let mut rgba = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let index = y as usize * width as usize + x as usize;
            rgba.extend_from_slice(&static_ao_baked_pixel(
                base,
                low_owners,
                high_owners,
                scale,
                x,
                y,
                mask[index],
                strength,
            ));
        }
    }
    rgba
}

pub(in crate::renderer) fn build_static_ao_baked_lightmaps(
    source: &StaticAoWorldSource,
    masks: &[Vec<u8>],
    scale: u32,
    strength: f32,
) -> Result<Vec<StaticAoBakedLightmap>, String> {
    if masks.len() != source.lightmap_bases.len() {
        return Err("lightmap AO cache does not match current lightmap set".into());
    }
    let mut low_owners = rasterize_static_ao_owners(source, 1);
    let mut high_owners = rasterize_static_ao_owners(source, scale);
    for ((low, high), &[base_width, base_height]) in low_owners
        .iter_mut()
        .zip(high_owners.iter_mut())
        .zip(source.lightmap_sizes.iter())
    {
        dilate_static_ao_owners(low, base_width, base_height, 2);
        dilate_static_ao_owners(
            high,
            base_width.saturating_mul(scale.max(1)),
            base_height.saturating_mul(scale.max(1)),
            2 * scale.max(1),
        );
    }
    masks
        .iter()
        .zip(source.lightmap_bases.iter())
        .zip(low_owners.iter().zip(high_owners.iter()))
        .map(|((mask, base), (low, high))| {
            static_ao_baked_lightmap(base, mask, low, high, scale, strength)
        })
        .collect()
}

pub(in crate::renderer) fn read_static_ao_cache(
    source: &StaticAoWorldSource,
    key: StaticAoJobKey,
) -> Option<StaticAoBakeData> {
    let path = static_ao_cache_path(&source.cache, key);
    let bytes = fs::read(&path).ok()?;
    // JAL5 caches the final supersampled lightmap pixels, not just AO masks.
    // Cache hits therefore avoid repeating the expensive upscale/sRGB composite.
    if bytes.len() < 12 || !bytes.starts_with(b"JAL5") {
        return None;
    }
    let vertex_expected = source.vertices.len();
    let vertex_count = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
    let image_count = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    if vertex_count != vertex_expected || image_count != source.lightmap_bases.len() {
        return None;
    }
    let mut offset = 12usize;
    let mut images = Vec::with_capacity(image_count);
    for base in source.lightmap_bases.iter() {
        if offset + 12 > bytes.len() {
            return None;
        }
        let width = u32::from_le_bytes(bytes[offset..offset + 4].try_into().ok()?);
        let height = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?);
        let len = u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into().ok()?) as usize;
        offset += 12;
        let expected_len = width as usize * height as usize * 4;
        if len != expected_len || offset + len > bytes.len() {
            return None;
        }
        images.push(StaticAoBakedLightmap {
            label: format!("{} + cached baked AO", base.label),
            width,
            height,
            rgba: bytes[offset..offset + len].to_vec(),
            clamp: base.clamp,
            srgb: base.srgb,
        });
        offset += len;
    }
    if offset + vertex_expected != bytes.len() {
        return None;
    }
    let vertex_values = bytes[offset..].to_vec();
    Some(StaticAoBakeData::Lightmap {
        vertex_values,
        images,
    })
}

pub(in crate::renderer) fn write_static_ao_cache(
    source: &StaticAoWorldSource,
    key: StaticAoJobKey,
    data: &StaticAoBakeData,
) {
    let path = static_ao_cache_path(&source.cache, key);
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let StaticAoBakeData::Lightmap {
        vertex_values,
        images,
    } = data;
    let pixel_bytes = images.iter().map(|image| image.rgba.len()).sum::<usize>();
    let mut bytes = Vec::with_capacity(12 + images.len() * 12 + pixel_bytes + vertex_values.len());
    bytes.extend_from_slice(b"JAL5");
    bytes.extend_from_slice(&(vertex_values.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(images.len() as u32).to_le_bytes());
    for image in images {
        bytes.extend_from_slice(&image.width.to_le_bytes());
        bytes.extend_from_slice(&image.height.to_le_bytes());
        bytes.extend_from_slice(&(image.rgba.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&image.rgba);
    }
    bytes.extend_from_slice(vertex_values);
    if let Err(error) = fs::write(&path, bytes) {
        crate::logging::write_line_with_path(
            crate::logging::Level::Error,
            format_args!(
                "Static BSP AO: could not write cache {}: {error}",
                path.display()
            ),
            path.clone(),
        );
    }
}

pub(in crate::renderer) fn run_static_ao_worker(
    source: StaticAoWorldSource,
    key: StaticAoJobKey,
    force_rebuild: bool,
    progress: &mpsc::Sender<(u32, u32)>,
    updates: &mpsc::Sender<StaticAoWorkerUpdate>,
    camera_position: Vec3,
) -> Result<StaticAoWorkerResult, String> {
    let started = Instant::now();
    let _ = progress.send((1, 100));
    if !force_rebuild && !key.current_cell_only {
        if let Some(data) = read_static_ao_cache(&source, key) {
            let _ = progress.send((100, 100));
            return Ok(StaticAoWorkerResult {
                key,
                data,
                cache_hit: true,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            });
        }
    }

    if source.lightmap_sizes.is_empty() || source.lightmap_triangles.is_empty() {
        return Err("map has no usable lightmap-space BSP geometry".into());
    }
    let bvh_started = Instant::now();
    let bvh = Arc::new(StaticAoBvh::build(source.render_triangles.clone())?);
    let _ = progress.send((15, 100));
    rverbose!(
        1,
        "Static BSP AO worker: render BVH {} triangles / {} nodes in {:.1} ms",
        bvh.triangles.len(),
        bvh.nodes.len(),
        bvh_started.elapsed().as_secs_f64() * 1000.0,
    );
    let workers = static_ao_worker_count();
    let sample_pattern = Arc::new(static_ao_sample_pattern(key.samples));
    let range_scale = (key.range as f32 / 100.0).clamp(0.25, 4.0);
    let strength = (key.strength as f32 / 100.0).clamp(0.0, 1.0);
    // Switch to the supersampled authored lightmap immediately. AO tiles then
    // darken this texture in-place as the worker finishes them.
    let neutral_masks = source
        .lightmap_sizes
        .iter()
        .map(|&[w, h]| {
            vec![
                255u8;
                w.saturating_mul(key.scale.max(1)) as usize
                    * h.saturating_mul(key.scale.max(1)) as usize
            ]
        })
        .collect::<Vec<_>>();
    let neutral_images = build_static_ao_baked_lightmaps(&source, &neutral_masks, key.scale, 0.0)?;
    let _ = updates.send(StaticAoWorkerUpdate::InitLightmaps {
        key,
        images: neutral_images,
    });
    // One render-geometry BVH drives both receiver types, but they are baked
    // independently: vertex-lit/static models use vertex AO while lightmapped
    // BSP uses only lightmap-space AO.
    let mut vertex_values = if key.current_cell_only {
        // Fast test mode is intentionally lightmap-only: reset any previous
        // vertex AO and spend the bake budget on the nearest lightmap tile.
        vec![255u8; source.vertices.len()]
    } else {
        bake_static_structural_vertex_ao(
            &source,
            Arc::clone(&bvh),
            Arc::clone(&sample_pattern),
            range_scale,
            workers,
        )
    };
    let _ = progress.send((45, 100));
    if !key.current_cell_only {
        for visibility in &mut vertex_values {
            let raw = f32::from(*visibility) / 255.0;
            let effective = 1.0 - strength * (1.0 - raw);
            *visibility = (effective.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    let _ = updates.send(StaticAoWorkerUpdate::VertexAo {
        key,
        values: vertex_values.clone(),
    });
    let masks = bake_static_lightmap_ao(
        &source,
        Arc::clone(&sample_pattern),
        key.scale,
        range_scale,
        strength,
        bvh,
        workers,
        key,
        camera_position,
        updates,
        progress,
    )?;
    let _ = progress.send((90, 100));
    let images = build_static_ao_baked_lightmaps(&source, &masks, key.scale, strength)?;
    let _ = progress.send((95, 100));
    let data = StaticAoBakeData::Lightmap {
        vertex_values,
        images,
    };
    if !key.current_cell_only {
        write_static_ao_cache(&source, key, &data);
    }
    let _ = progress.send((100, 100));
    Ok(StaticAoWorkerResult {
        key,
        data,
        cache_hit: false,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

pub(in crate::renderer) fn collect_static_ao_lightmap_triangles(
    vertices: &[GpuVertex],
    batches: &[DrawBatch],
) -> Vec<StaticAoLightmapTriangle> {
    let mut seen = BTreeSet::<(u32, u32, usize)>::new();
    let mut triangles = Vec::new();
    let mut next_owner = 1u32;
    for batch in batches {
        let Some(texture) = batch.lightmap else {
            continue;
        };
        if batch.pipeline.class == DrawClass::Sky {
            continue;
        }
        let key = (batch.vertices.start, batch.vertices.end, texture);
        if !seen.insert(key) {
            continue;
        }
        let owner = next_owner;
        next_owner = next_owner.saturating_add(1);
        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        for triangle in vertices[start..end].chunks_exact(3) {
            if triangle
                .iter()
                .any(|vertex| vertex.lightmap_uv[0] < 0.0 || vertex.lightmap_uv[1] < 0.0)
            {
                continue;
            }
            triangles.push(StaticAoLightmapTriangle {
                texture,
                owner,
                positions: [
                    triangle[0].position,
                    triangle[1].position,
                    triangle[2].position,
                ],
                normals: [triangle[0].normal, triangle[1].normal, triangle[2].normal],
                uvs: [
                    triangle[0].lightmap_uv,
                    triangle[1].lightmap_uv,
                    triangle[2].lightmap_uv,
                ],
            });
        }
    }
    triangles
}

pub(in crate::renderer) fn collect_static_ao_vertex_receivers(
    vertex_count: usize,
    batches: &[DrawBatch],
) -> Vec<u8> {
    let mut receivers = vec![0u8; vertex_count];
    for batch in batches {
        // q3map LIGHTMAP_BY_VERTEX geometry stores baked lighting in vertex color.
        // Only the stage that actually consumes that color should receive cached AO.
        if !batch.vertex_lit
            || !batch.rgb_gen.uses_vertex_color()
            || !matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask)
        {
            continue;
        }
        let start = batch.vertices.start as usize;
        let end = (batch.vertices.end as usize).min(receivers.len());
        for receiver in &mut receivers[start.min(end)..end] {
            *receiver = 1;
        }
    }
    receivers
}

pub(in crate::renderer) fn collect_static_ao_render_triangles(
    vertices: &[GpuVertex],
    batches: &[DrawBatch],
) -> Vec<StaticAoRenderTriangle> {
    let mut seen = BTreeSet::<(u32, u32)>::new();
    let mut triangles = Vec::new();
    for batch in batches {
        // For the quality reference bake, use only truly opaque rendered BSP
        // surfaces. Sky/transparent never occlude; alpha-masked surfaces are
        // skipped rather than incorrectly treating their transparent holes as solid.
        if batch.pipeline.class != DrawClass::Opaque {
            continue;
        }
        let key = (batch.vertices.start, batch.vertices.end);
        if !seen.insert(key) {
            continue;
        }
        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        for triangle in vertices[start..end].chunks_exact(3) {
            triangles.push(StaticAoRenderTriangle {
                positions: [
                    triangle[0].position,
                    triangle[1].position,
                    triangle[2].position,
                ],
            });
        }
    }
    triangles
}
