//! Developer "spawnriver" patches on the render thread: spawn, per-frame
//! native solver encoding and the water body's per-frame inputs. See
//! river_runtime.rs and river_surface.rs.
use super::{batch_area_visible, effective_area_mask, PvsMode, WorldBatch, WorldGpu};
use crate::river_runtime::LiveRiver;
use crate::river_surface::{RiverSurface, ViewUniform};
use jka_river::river_ninja::{RiverImage, RiverVolumeImage};

pub(super) struct PromotionClaim {
    pub(super) id: u64,
    coarse: Vec<usize>,
    full: Vec<usize>,
}

impl PromotionClaim {
    /// A dense river draws as one body. Keep the authored pieces whenever
    /// only part of the body is visible through the active PVS or area mask.
    fn fully_visible(&self, world: &WorldGpu, mode: PvsMode, area_mask: Option<&[u8; 32]>) -> bool {
        let cluster = world.last_cluster.flatten();
        let area_mask = effective_area_mask(world, mode, area_mask);
        let visible = |batches: &[WorldBatch], owned: &[usize], indices: Option<&[usize]>| {
            owned.iter().all(|&index| {
                indices.is_none_or(|list| list.binary_search(&index).is_ok())
                    && batch_area_visible(&batches[index], area_mask)
            })
        };
        if let Some(cluster) = cluster.filter(|_| mode != PvsMode::Off) {
            if matches!(mode, PvsMode::Full | PvsMode::Auto) {
                if let Some(indices) = world.full_visible_batches_by_cluster.get(cluster) {
                    return visible(&world.full_batches, &self.full, Some(indices));
                }
            }
            return visible(
                &world.coarse_batches,
                &self.coarse,
                world
                    .coarse_visible_batches_by_cluster
                    .get(cluster)
                    .map(Vec::as_slice),
            );
        }
        true
    }
}

/// A presented player as a JKA player hull (bg_public.h: mins -15,-15,-24;
/// maxs 15,15,40, or 16 crouched) in map units. Actor origins and velocities
/// are render space.
fn river_body(actor: &crate::ocean::interaction::ActorSample) -> crate::river_runtime::MapBody {
    let (mins, maxs, velocity) = actor.jka_hull();
    crate::river_runtime::MapBody { mins, maxs, velocity }
}

/// `riverdump`: the previous dump of each river, for the volume change and
/// the diff image. Depth per lattice cell in map units, keyed by tile.
struct PreviousDump {
    id: crate::river_authoring::Id,
    simulated_seconds: f64,
    volume: f64,
    depth: std::collections::BTreeMap<[i32; 2], Vec<f32>>,
}
static PREVIOUS_DUMPS: std::sync::Mutex<Vec<PreviousDump>> = std::sync::Mutex::new(Vec::new());
static DUMP_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum DumpClass {
    Dry,
    Wet,
    Wall,
    VoidWall,
    /// Empty column (bed at the contour floor) that did not become a wall.
    Pit,
}

impl DumpClass {
    fn name(self) -> &'static str {
        match self {
            Self::Dry => "dry bed",
            Self::Wet => "wet",
            Self::Wall => "border/collider wall",
            Self::VoidWall => "void wall (nothing captured)",
            Self::Pit => "PIT (empty column NOT walled: drains water)",
        }
    }
}

fn classify(dump: &jka_river::river_tiles::PoolDump, geometry: [f32; 4], state: [f32; 4]) -> DumpClass {
    if geometry[1] >= 0.5 {
        if geometry[0] >= dump.wall_cm - 1.0 { DumpClass::VoidWall } else { DumpClass::Wall }
    } else if geometry[0] <= dump.void_threshold_cm {
        DumpClass::Pit
    } else if state[1] > 0.1 {
        DumpClass::Wet
    } else {
        DumpClass::Dry
    }
}

/// The Fluid Ninja river input images (`river_ninja::river_input_images`),
/// loaded once per enable and dropped on disable or when rivers clear.
pub(in crate::renderer) struct NinjaInputTextures {
    pub images: Vec<super::textures::GpuImage>,
}

impl super::Renderer {
    /// Exact VFS qpaths and sizes from the private preset table; linear or sRGB
    /// per the UE asset setting, no mips (every read is an explicit mip 0), no
    /// picmip. A missing or mis-sized image disables the feature (logged once).
    fn load_ninja_input_textures(&self, images: &[RiverImage]) -> Result<NinjaInputTextures, String> {
        if images.len() != 7 {
            return Err("Fluid Ninja is not included in this build".into());
        }
        let mut assets = self.ninja_asset_path()?;
        let decoded = images.iter().map(|spec| decode_ninja_image(&mut assets, spec)).collect::<Result<Vec<_>, _>>()?;
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let images = decoded.iter().map(|data| super::textures::upload_texture(&self.device, &self.queue, data)).collect();
        jka_river::river_ninja::checked_scope(scope)?;
        Ok(NinjaInputTextures { images })
    }

    fn ninja_asset_path(&self) -> Result<super::AssetSearchPath, String> {
        super::AssetSearchPath::open_game(&self.base, self.detail_texture_game.as_deref())
            .map_err(|error| error.to_string())
    }

    fn load_ninja_caustics(&self, spec: RiverVolumeImage) -> Result<wgpu::TextureView, String> {
        let data = decode_ninja_image(&mut self.ninja_asset_path()?, &spec.image)?;
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let volume = upload_ninja_volume(&self.device, &self.queue, &data, &spec);
        jka_river::river_ninja::checked_scope(scope)?;
        volume
    }

    /// Ocean underwater mode 5 inputs, loaded once on first use (failures are kept,
    /// so the ocean falls back to Blue Water's composite without retrying).
    pub(in crate::renderer) fn ensure_ocean_ninja_underwater(&mut self) {
        if self.ocean_ninja_underwater.is_some() {
            return;
        }
        let loaded = jka_river::river_ninja::river_underwater_images().and_then(|specs| {
            let [noise, gradient] = self.load_ninja_underwater_images(specs);
            let caustics = jka_river::river_ninja::river_caustics_image().and_then(|spec| {
                self.load_ninja_caustics(spec).map_err(|e| eprintln!("[OCEAN NINJA] underwater caustics unavailable: {e}")).ok()
            });
            Some(crate::ocean::NinjaUnderwaterTextures { noise: noise?, gradient: gradient?, caustics })
        });
        if loaded.is_none() {
            eprintln!("[OCEAN NINJA] underwater inputs unavailable; Blue Water underwater composite retained");
        }
        self.ocean_ninja_underwater = Some(loaded);
    }

    fn load_ninja_underwater_images(&self, specs: [RiverImage; 2]) -> [Option<wgpu::TextureView>; 2] {
        let mut assets = self.ninja_asset_path();
        let surface_images = self.river_ninja_textures.as_ref().and_then(Option::as_ref);
        let surface_specs = jka_river::river_ninja::river_input_images();
        specs.map(|spec| {
            if let Some(image) = surface_images.and_then(|textures| {
                surface_specs.iter().position(|s| s == &spec).map(|i| &textures.images[i])
            }) {
                return Some(image.view.clone());
            }
            let result = assets.as_mut().map_err(|error| error.clone()).and_then(|assets| {
                let data = decode_ninja_image(assets, &spec)?;
                let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
                let image = super::textures::upload_texture(&self.device, &self.queue, &data);
                jka_river::river_ninja::checked_scope(scope)?;
                Ok(image.view.clone())
            });
            match result {
                Ok(view) => Some(view),
                Err(error) => {
                    eprintln!("[RIVER NINJA] underwater texture unavailable: {error}");
                    None
                }
            }
        })
    }
}

fn decode_ninja_image(
    assets: &mut super::AssetSearchPath,
    spec: &RiverImage,
) -> Result<crate::materials::TextureData, String> {
    let asset = assets.read(spec.qpath, 16 * 1024 * 1024).map_err(|error| error.to_string())?
        .ok_or_else(|| format!("required texture missing: {}", spec.qpath))?;
    let data = crate::materials::decode_texture_data_with_color_space(spec.qpath, &asset.bytes, false, false, spec.srgb)?;
    if [data.width, data.height] != spec.size {
        return Err(format!("texture {} is {}x{}, expected {}x{}", spec.qpath, data.width, data.height, spec.size[0], spec.size[1]));
    }
    Ok(data)
}

fn upload_ninja_volume(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &crate::materials::TextureData,
    spec: &RiverVolumeImage,
) -> Result<wgpu::TextureView, String> {
    let [width, height] = spec.slice_size;
    if !width.is_power_of_two() || !height.is_power_of_two()
        || data.width % width != 0 || data.height % height != 0
        || spec.image.srgb || data.rgba16f.is_some() || data.mip_level_count != 1
    {
        return Err(format!("invalid volume atlas: {}", data.label));
    }
    let [columns, rows] = [data.width / width, data.height / height];
    let depth = columns * rows;
    if !depth.is_power_of_two() {
        return Err(format!("invalid volume depth: {}", data.label));
    }
    let extent = wgpu::Extent3d { width, height, depth_or_array_layers: depth };
    let levels = width.max(height).max(depth).ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&data.label),
        size: extent,
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let row_bytes = data.width as usize * 4;
    let slice_row_bytes = width as usize * 4;
    let mut rgba = Vec::with_capacity(data.rgba.len());
    for tile_y in 0..rows {
        for tile_x in 0..columns as usize {
            for y in 0..height {
                let start = (tile_y * height + y) as usize * row_bytes + tile_x * slice_row_bytes;
                rgba.extend_from_slice(&data.rgba[start..start + slice_row_bytes]);
            }
        }
    }
    let write_level = |level, size: [usize; 3], pixels: &[u8]| {
        let [width, height, depth_or_array_layers] = size.map(|n| n as u32);
        let mut target = texture.as_image_copy();
        target.mip_level = level;
        queue.write_texture(
            target,
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers },
        );
    };
    let mut size = [width as usize, height as usize, depth as usize];
    write_level(0, size, &rgba);
    let mut source: Vec<[f32; 4]> = rgba.chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]].map(f32::from)).collect();
    for level in 1..levels {
        source = downsample_volume_mip(&source, size, &spec.mip_filter);
        size = size.map(|n| (n / 2).max(1));
        rgba.clear();
        rgba.extend(source.iter().flat_map(|p| p.map(|value| value.round() as u8)));
        write_level(level, size, &rgba);
    }
    Ok(texture.create_view(&Default::default()))
}

/// Separable XY downsample with wrap, then average each pair of slices.
/// Keep intermediate levels linear and unquantized while filtering.
fn downsample_volume_mip(source: &[[f32; 4]], size: [usize; 3], kernel: &[f32; 8]) -> Vec<[f32; 4]> {
    let [width, height, depth] = size;
    let [dst_width, dst_height, dst_depth] = size.map(|n| (n / 2).max(1));
    let mut mip = Vec::with_capacity(dst_width * dst_height * dst_depth);
    let mut vertical = vec![[0.0; 4]; width];
    for z in 0..dst_depth {
        let z0 = z * 2;
        let z1 = (z0 + 1).min(depth - 1);
        for y in 0..dst_height {
            for (x, color) in vertical.iter_mut().enumerate() {
                *color = [0.0; 4];
                for (tap, &weight) in kernel.iter().enumerate() {
                    let row = (y * 2 + tap).wrapping_sub(3) & (height - 1);
                    let a = source[(z0 * height + row) * width + x];
                    let b = source[(z1 * height + row) * width + x];
                    for channel in 0..4 {
                        color[channel] += (a[channel] + b[channel]) * (weight * 0.5);
                    }
                }
            }
            for x in 0..dst_width {
                let mut color = [0.0; 4];
                for (tap, &weight) in kernel.iter().enumerate() {
                    let column = (x * 2 + tap).wrapping_sub(3) & (width - 1);
                    for channel in 0..4 {
                        color[channel] += vertical[column][channel] * weight;
                    }
                }
                mip.push(color);
            }
        }
    }
    mip
}

impl super::Renderer {
    /// `riverdump`: synchronous readback of every live sparse river (a
    /// deliberate one-off stall), console report and top-down PNGs.
    pub(in crate::renderer) fn dump_live_rivers(&mut self, eye: [f32; 3], directory: &std::path::Path) {
        if self.live_rivers.is_empty() {
            eprintln!("[RIVER DUMP] no live rivers (place one with spawnriver)");
            return;
        }
        if let Err(error) = std::fs::create_dir_all(directory) {
            eprintln!("[RIVER DUMP] cannot create {}: {error}", directory.display());
            return;
        }
        let cm = f64::from(jka_water::river::CENTIMETRES_PER_MAP_UNIT);
        let number = DUMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut previous_dumps = PREVIOUS_DUMPS.lock().unwrap_or_else(|e| e.into_inner());
        previous_dumps.retain(|p| self.live_rivers.iter().any(|r| r.id == p.id));
        for river in &self.live_rivers {
            let (dump, corner_cm, source_box) = match river.dump(&self.device, &self.queue) {
                Ok(dump) => dump,
                Err(error) => {
                    eprintln!("[RIVER DUMP] river {}: {error}", river.id);
                    continue;
                }
            };
            let tile = dump.tile as i32;
            let dx = f64::from(dump.dx_cm);
            let map_z = |value_cm: f32| (dump.origin_z_cm + f64::from(value_cm)) / cm;
            let cell_map_xy = |cell: [i32; 2]| -> [f64; 2] {
                std::array::from_fn(|a| (corner_cm[a] + (f64::from(cell[a]) + 0.5) * dx) / cm)
            };
            // Totals and classes over resident cells.
            let mut counts = [0usize; 5];
            let mut volume_cm3 = 0.0f64;
            let mut bed_range = [f32::INFINITY, f32::NEG_INFINITY];
            let mut wet_pits: Vec<(f32, [i32; 2])> = Vec::new();
            for (t, cells) in &dump.tiles {
                for (lane, &(geometry, state)) in cells.iter().enumerate() {
                    let class = classify(&dump, geometry, state);
                    counts[class as usize] += 1;
                    if class == DumpClass::Wet || class == DumpClass::Pit {
                        volume_cm3 += f64::from(state[1].max(0.0)) * dx * dx;
                    }
                    if matches!(class, DumpClass::Wet | DumpClass::Dry) {
                        bed_range = [bed_range[0].min(geometry[0]), bed_range[1].max(geometry[0])];
                    }
                    if class == DumpClass::Pit && state[1] > 0.1 {
                        let cell = [t[0] * tile + (lane as i32 % tile), t[1] * tile + (lane as i32 / tile)];
                        wet_pits.push((state[1], cell));
                    }
                }
            }
            let volume = volume_cm3 / (cm * cm * cm);
            eprintln!(
                "[RIVER DUMP] river {} #{number}: sim={:.1}s resident_tiles={} lattice={}x{} cells of {:.1}u | water volume={:.0} cubic units ({:.0} litres)",
                river.id, river.simulated_seconds, dump.tiles.len(), dump.lattice_cells[0], dump.lattice_cells[1],
                dx / cm, volume, volume_cm3 / 1000.0,
            );
            eprintln!(
                "[RIVER DUMP]   cells: wet={} dry={} walls={} void_walls={} PITS={} (wet pits {}) | bed z {:.0}..{:.0} | contour floor z={:.0}, void wall top z={:.0} | drains={}",
                counts[DumpClass::Wet as usize], counts[DumpClass::Dry as usize], counts[DumpClass::Wall as usize],
                counts[DumpClass::VoidWall as usize], counts[DumpClass::Pit as usize], wet_pits.len(),
                map_z(bed_range[0]), map_z(bed_range[1]), map_z(dump.floor_cm), map_z(dump.wall_cm), dump.drains.len(),
            );
            // Water standing above the source level cannot come from the
            // source: list the highest such cells.
            let level_cm = (f64::from(river.source.water_level) * cm - dump.origin_z_cm) as f32;
            let mut over: Vec<(f32, [i32; 2], f32)> = Vec::new();
            for (t, cells) in &dump.tiles {
                for (lane, &(geometry, state)) in cells.iter().enumerate() {
                    if classify(&dump, geometry, state) == DumpClass::Wet && state[0] > level_cm + 8.0 * cm as f32 {
                        over.push((state[0], [t[0] * tile + (lane as i32 % tile), t[1] * tile + (lane as i32 / tile)], state[1]));
                    }
                }
            }
            over.sort_by(|a, b| b.0.total_cmp(&a.0));
            eprintln!("[RIVER DUMP]   cells with water more than 8u above the source level {:.0}: {}", river.source.water_level, over.len());
            for (height, cell, depth) in over.iter().take(5) {
                let [x, y] = cell_map_xy(*cell);
                eprintln!("[RIVER DUMP]     ({x:.0} {y:.0}) water z={:.0} depth={:.0}u", map_z(*height), f64::from(*depth) / cm);
            }
            // Which solver stage creates or destroys water: one frame replayed
            // on the CPU reference from this exact state.
            // Replayed with this river's transport, then with depth advection
            // flipped (r_riverAdvectDepth) for comparison.
            for (label, transport) in [
                ("this river", dump.transport),
                ("depth advection flipped", jka_water::river::Transport { advect_depth: !dump.transport.advect_depth, ..dump.transport }),
            ] {
                let started = std::time::Instant::now();
                match jka_river::river_tiles::replay_budget(&dump, Some(source_box), transport) {
                    Ok(budget) => {
                        let units = |cm3: f64| cm3 / (cm * cm * cm);
                        let net: f64 = budget.iter().map(|(_, v)| v).sum();
                        let frame_s = f64::from(dump.controls.frame_dt).max(1e-6);
                        eprintln!(
                            "[RIVER DUMP]   one-frame volume budget, {label} (depth advection {}; CPU replay {:.0} ms; cubic units per solver frame, x{:.0} per second): net {:+.0}",
                            if transport.advect_depth { "ON, Unreal" } else { "OFF, conservative" },
                            started.elapsed().as_secs_f64() * 1000.0, 1.0 / frame_s, units(net),
                        );
                        for (stage, delta) in &budget {
                            eprintln!("[RIVER DUMP]     {stage:<28} {:+.0}", units(*delta));
                        }
                    }
                    Err(error) => eprintln!("[RIVER DUMP]   CPU replay failed: {error}"),
                }
            }
            wet_pits.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (depth, cell) in wet_pits.iter().take(5) {
                let [x, y] = cell_map_xy(*cell);
                eprintln!("[RIVER DUMP]   wet pit at ({x:.0} {y:.0}) depth {:.0}u", f64::from(*depth) / cm);
            }
            // The column under the viewer.
            let eye_cell: [i32; 2] = std::array::from_fn(|a| ((f64::from(eye[a]) * cm - corner_cm[a]) / dx).floor() as i32);
            let eye_tile = [eye_cell[0].div_euclid(tile), eye_cell[1].div_euclid(tile)];
            match dump.tiles.get(&eye_tile) {
                Some(cells) => {
                    let lane = (eye_cell[1].rem_euclid(tile) * tile + eye_cell[0].rem_euclid(tile)) as usize;
                    let (geometry, state) = cells[lane];
                    eprintln!(
                        "[RIVER DUMP]   at you ({:.0} {:.0} {:.0}): {} | bed z={:.1} water z={:.1} depth={:.1}u speed={:.0}u/s collider={}",
                        eye[0], eye[1], eye[2], classify(&dump, geometry, state).name(),
                        map_z(geometry[0]), map_z(state[0]), f64::from(state[1]) / cm,
                        f64::from(state[2].hypot(state[3])) / cm, geometry[1],
                    );
                }
                None => eprintln!("[RIVER DUMP]   at you ({:.0} {:.0} {:.0}): tile {eye_tile:?} not resident (reads as a wall)", eye[0], eye[1], eye[2]),
            }
            // Image over the resident tiles' bounding box, north up.
            let (Some(lo), Some(hi)) = (
                dump.tiles.keys().copied().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1])]),
                dump.tiles.keys().copied().reduce(|a, b| [a[0].max(b[0]), a[1].max(b[1])]),
            ) else {
                continue;
            };
            let size = [((hi[0] - lo[0] + 1) * tile) as u32, ((hi[1] - lo[1] + 1) * tile) as u32];
            let scale = (1024 / size[0].max(size[1]).max(1)).clamp(1, 4);
            let source = river.source;
            let source_cells: [[i32; 2]; 2] = [-1.0f32, 1.0].map(|s| {
                std::array::from_fn(|a| ((f64::from(source.center[a] + s * 0.5 * source.source_width) * cm - corner_cm[a]) / dx).floor() as i32)
            });
            let previous = previous_dumps.iter().position(|p| p.id == river.id);
            let mut image = image::RgbImage::new(size[0] * scale, size[1] * scale);
            let mut diff = previous.map(|_| image::RgbImage::new(size[0] * scale, size[1] * scale));
            let bed_span = (bed_range[1] - bed_range[0]).max(1.0);
            let mut depths = std::collections::BTreeMap::new();
            let mut gained = 0.0f64;
            let mut lost = 0.0f64;
            for (t, cells) in &dump.tiles {
                let mut tile_depth = Vec::with_capacity(cells.len());
                let old = previous.and_then(|p| previous_dumps[p].depth.get(t));
                for (lane, &(geometry, state)) in cells.iter().enumerate() {
                    let local = [lane as i32 % tile, lane as i32 / tile];
                    let cell = [t[0] * tile + local[0], t[1] * tile + local[1]];
                    let class = classify(&dump, geometry, state);
                    let shade = (40.0 + 160.0 * ((geometry[0] - bed_range[0]) / bed_span).clamp(0.0, 1.0)) as u8;
                    let depth_units = (f64::from(state[1].max(0.0)) / cm) as f32;
                    tile_depth.push(depth_units);
                    let mut colour = match class {
                        DumpClass::Dry => [shade, shade, shade],
                        DumpClass::Wet => {
                            let d = (depth_units / 64.0).clamp(0.0, 1.0);
                            [0, (120.0 - 90.0 * d) as u8, (170.0 + 85.0 * d) as u8]
                        }
                        DumpClass::Wall => [150, 30, 30],
                        DumpClass::VoidWall => [200, 0, 200],
                        DumpClass::Pit => if depth_units > 0.0 { [255, 255, 0] } else { [140, 140, 0] },
                    };
                    let in_source = (0..2).all(|a| cell[a] >= source_cells[0][a] && cell[a] <= source_cells[1][a]);
                    if in_source {
                        colour = [0, 230, 0];
                    }
                    let centre = [(f64::from(cell[0]) + 0.5) * dx, (f64::from(cell[1]) + 0.5) * dx];
                    if dump.drains.iter().any(|d| {
                        let (ox, oy) = (centre[0] - f64::from(d.center[0]), centre[1] - f64::from(d.center[1]));
                        ox * ox + oy * oy <= f64::from(d.radius) * f64::from(d.radius)
                    }) {
                        colour = [255, 140, 0];
                    }
                    if (cell[0] - eye_cell[0]).abs() + (cell[1] - eye_cell[1]).abs() <= 1 {
                        colour = [255, 255, 255];
                    }
                    let pixel = [(cell[0] - lo[0] * tile) as u32, size[1] - 1 - (cell[1] - lo[1] * tile) as u32];
                    let put = |target: &mut image::RgbImage, rgb: [u8; 3]| {
                        for oy in 0..scale {
                            for ox in 0..scale {
                                target.put_pixel(pixel[0] * scale + ox, pixel[1] * scale + oy, image::Rgb(rgb));
                            }
                        }
                    };
                    put(&mut image, colour);
                    if let Some(diff) = diff.as_mut() {
                        let before = old.map_or(0.0, |old| old[lane]);
                        let change = depth_units - before;
                        let area = dx * dx / (cm * cm);
                        if change > 0.0 { gained += f64::from(change) * area } else { lost -= f64::from(change) * area }
                        let k = (change.abs() / 16.0).clamp(0.0, 1.0);
                        let grey = shade / 3;
                        let rgb = if change.abs() < 0.05 {
                            [grey, grey, grey]
                        } else if change < 0.0 {
                            [(80.0 + 175.0 * k) as u8, grey / 2, grey / 2]
                        } else {
                            [grey / 2, (80.0 + 175.0 * k) as u8, grey / 2]
                        };
                        put(diff, if in_source { [0, 0, 255] } else { rgb });
                    }
                }
                depths.insert(*t, tile_depth);
            }
            let path = directory.join(format!("river-dump-{}-{number:03}.png", river.id));
            match image.save(&path) {
                Ok(()) => crate::logging::write_line_with_path(
                    crate::logging::Level::Info,
                    format_args!("[RIVER DUMP]   wrote {} ({}x{} cells, {scale}px/cell, north up)", path.display(), size[0], size[1]),
                    path.clone(),
                ),
                Err(error) => eprintln!("[RIVER DUMP]   PNG write failed: {error}"),
            }
            if let (Some(p), Some(diff)) = (previous, diff) {
                let before = &previous_dumps[p];
                let seconds = (river.simulated_seconds - before.simulated_seconds).max(1e-6);
                eprintln!(
                    "[RIVER DUMP]   since last dump ({:.1}s simulated): volume {:+.0} cubic units ({:+.0}/s); cells gained {:.0}, lost {:.0} cubic units",
                    seconds, volume - before.volume, (volume - before.volume) / seconds, gained, lost,
                );
                let path = directory.join(format!("river-diff-{}-{number:03}.png", river.id));
                match diff.save(&path) {
                    Ok(()) => crate::logging::write_line_with_path(
                        crate::logging::Level::Info,
                        format_args!("[RIVER DUMP]   wrote {} (red = water left, green = water arrived, blue = source)", path.display()),
                        path.clone(),
                    ),
                    Err(error) => eprintln!("[RIVER DUMP]   diff PNG write failed: {error}"),
                }
            }
            let record = PreviousDump { id: river.id, simulated_seconds: river.simulated_seconds, volume, depth: depths };
            match previous {
                Some(p) => previous_dumps[p] = record,
                None => previous_dumps.push(record),
            }
        }
    }

    pub(in crate::renderer) fn spawn_live_river(
        &mut self,
        definition: crate::river_authoring::Definition,
        collision: &jka_assets::bsp::river::RiverCollision,
    ) -> Result<(), String> {
        let source = definition.source;
        let spawn_started = (crate::river_runtime::debug_mode() > 0).then(std::time::Instant::now);
        if spawn_started.is_some() { eprintln!("[RIVER DBG] preparing source center={:?} collision_triangles={} (render-thread spawn)", source.center, collision.surfaces.len()); }
        self.ensure_river_surface(spawn_started)?;
        let grid = self
            .world
            .as_ref()
            .and_then(|world| world.entity_light_grid.as_ref());
        let light = |point: [f32; 3]| grid.and_then(|grid| grid.sample_classic_entity_light(point));
        let map_toward_sun = grid.and_then(|grid| grid.map_toward_sun());
        match LiveRiver::spawn(
            &self.device,
            &self.queue,
            &collision.surfaces,
            collision.capture.as_ref(),
            source,
            light,
            map_toward_sun,
        ) {
            Ok(mut river) => {
                if let Some(started) = spawn_started { eprintln!("[RIVER DBG] source ready; spawn_cpu={:.2}ms; GPU initialization submitted (not waited)", started.elapsed().as_secs_f64() * 1000.0); }
                river.id = definition.id;
                river.set_settings(&self.queue, definition.appearance);
                river.set_drains(&self.queue, &self.river_drains);
                eprintln!(
                    "[RIVER] source at {:?}: {}x{} cells, {:.1} units/cell",
                    source.center,
                    river.grid.dimensions[0],
                    river.grid.dimensions[1],
                    river.grid.cell_size[0] / jka_water::river::CENTIMETRES_PER_MAP_UNIT
                );
                crate::river_runtime::set_audio_rivers_present(true);
                if let Some(existing) = self.live_rivers.iter_mut().find(|r| r.id == definition.id)
                {
                    *existing = river;
                } else {
                    self.live_rivers.push(river);
                }
                Ok(())
            }
            Err(error) => {
                if let Some(started) = spawn_started { eprintln!("[RIVER DBG] source creation failed after {:.2}ms", started.elapsed().as_secs_f64() * 1000.0); }
                Err(error)
            }
        }
    }

    fn ensure_river_surface(&mut self, spawn_started: Option<std::time::Instant>) -> Result<(), String> {
        if self.river_surface.is_none() {
            let mut surface = match RiverSurface::new(&self.device, &self.queue) {
                Ok(surface) => surface,
                Err(error) => {
                    if let Some(started) = spawn_started { eprintln!("[RIVER DBG] first-use resources failed after {:.2}ms", started.elapsed().as_secs_f64() * 1000.0); }
                    eprintln!("[RIVER] water resources failed: {error}");
                    return Err(error);
                }
            };
            // M_SimpleSpray's linear noise and normal map for the whitewater.
            let spray = [
                crate::river_surface::SPRAY_NOISE_TEXTURE,
                crate::river_surface::SPRAY_NORMALS_TEXTURE,
            ]
            .map(|name| {
                super::textures::try_load_texture_asset(&self.base, self.detail_texture_game.as_deref(), name, false, true, false, "River whitewater")
                    .map(|data| super::textures::upload_texture_picmip(&self.device, &self.queue, &data, self.picmip).view.clone())
            });
            match spray {
                [Some(noise), Some(normals)] => surface.set_spray_textures(noise, normals),
                _ => eprintln!(
                    "[RIVER] {} / {} not found; whitewater draws without its noise and normal map",
                    crate::river_surface::SPRAY_NOISE_TEXTURE,
                    crate::river_surface::SPRAY_NORMALS_TEXTURE
                ),
            }
            self.river_surface = Some(surface);
        }
        if let Some(surface) = &mut self.river_surface {
            let faces = self.world.as_ref().and_then(|world| {
                world.primary_skybox.and_then(|indices| {
                    let [Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)] =
                        indices.map(|i| world.textures.get(i).map(|t| &t.view))
                    else {
                        return None;
                    };
                    Some([a, b, c, d, e, f])
                })
            });
            surface.set_map_sky(&self.device, faces);
        }
        Ok(())
    }

    pub(in crate::renderer) fn install_promoted_rivers(
        &mut self,
        plans: &[jka_river::river_promotion::Plan],
        mode: u8,
    ) {
        if mode < 2 || plans.is_empty() {
            return;
        }
        if let Err(error) = self.ensure_river_surface(None) {
            eprintln!("[RIVER PROMOTION] resources unavailable: {error}");
            return;
        }
        let Some(world) = self.world.as_ref() else {
            return;
        };
        let grid = world.entity_light_grid.as_ref();
        let replace = mode == 3;
        for plan in plans {
            let claim = if replace {
                if !plan.replace || plan.world_ranges.is_empty() {
                    continue;
                }
                let owns = |batch: &WorldBatch| batch.river_owner == Some(plan.id);
                let mixed_recipe =
                    world
                        .auto4_variant_members
                        .iter()
                        .enumerate()
                        .any(|(i, members)| {
                            members
                                .iter()
                                .any(|&m| world.full_batches.get(m).is_some_and(owns))
                                && !world
                                    .auto4_batches
                                    .get(world.auto4_variant_base + i)
                                    .is_some_and(owns)
                        });
                let ocean_claim = world
                    .coarse_batches
                    .iter()
                    .chain(&world.full_batches)
                    .any(|batch| owns(batch) && batch.ocean_owners != 0);
                let owned_indices = |batches: &[WorldBatch]| {
                    batches
                        .iter()
                        .enumerate()
                        .filter_map(|(i, batch)| owns(batch).then_some(i))
                        .collect::<Vec<_>>()
                };
                let coarse = owned_indices(&world.coarse_batches);
                let full = owned_indices(&world.full_batches);
                if mixed_recipe
                    || ocean_claim
                    || coarse.is_empty()
                    || (!world.full_batches.is_empty() && full.is_empty())
                {
                    eprintln!(
                        "[RIVER PROMOTION] {}: unsupported or competing draw ownership",
                        plan.id
                    );
                    continue;
                }
                Some(PromotionClaim {
                    id: plan.id,
                    coarse,
                    full,
                })
            } else {
                None
            };
            match LiveRiver::spawn_promoted(
                &self.device,
                &self.queue,
                plan,
                replace,
                |point| grid.and_then(|g| g.sample_classic_entity_light(point)),
                grid.and_then(|g| g.map_toward_sun()),
            ) {
                Ok(river) => {
                    if let Some(claim) = claim {
                        self.river_promotion_claims.push(claim);
                    }
                    let role = if replace {
                        "replacement candidate"
                    } else {
                        "preview"
                    };
                    eprintln!("[RIVER PROMOTION] prefilled {role} {}", plan.id);
                    self.live_rivers.push(river);
                }
                Err(error) => eprintln!(
                    "[RIVER PROMOTION] plan {} retained original: {error}",
                    plan.id
                ),
            }
        }
    }

    pub(in crate::renderer) fn refresh_river_ownership(
        &mut self,
        ready: bool,
        area_mask: Option<&[u8; 32]>,
    ) {
        if self.river_promotion_claims.is_empty() && self.river_promotion_active == 0 {
            return;
        }
        let claims = &self.river_promotion_claims;
        let mut active = 0u8;
        if let Some(world) = self.world.as_ref().filter(|_| ready) {
            for (i, claim) in claims.iter().enumerate() {
                if self
                    .live_rivers
                    .iter()
                    .any(|r| r.id == claim.id && r.promoted_ready())
                    && claim.fully_visible(world, self.pvs_mode, area_mask)
                {
                    active |= 1 << i;
                }
            }
        }
        if active == self.river_promotion_active {
            return;
        }
        self.river_promotion_active = active;
        let is_active = |id| {
            claims
                .iter()
                .position(|claim| claim.id == id)
                .is_some_and(|i| active & (1 << i) != 0)
        };
        for river in &mut self.live_rivers {
            river.promotion_active = is_active(river.id);
        }
        if let Some(world) = self.world.as_mut() {
            for batch in world
                .coarse_batches
                .iter_mut()
                .chain(&mut world.full_batches)
                .chain(&mut world.auto4_batches)
            {
                batch.river_replaced = batch.river_owner.is_some_and(is_active);
            }
        }
        if let Some(surface) = &mut self.river_surface {
            surface.reset_audio_probe();
        }
        eprintln!("[RIVER PROMOTION] active replacement mask {active:#x}");
    }

    pub(in crate::renderer) fn clear_promoted_rivers(&mut self) {
        self.refresh_river_ownership(false, None);
        self.river_promotion_claims.clear();
        self.live_rivers
            .retain(|r| !jka_river::river_promotion::is_promoted(r.id));
        crate::river_runtime::set_audio_rivers_present(
            self.live_rivers.iter().any(LiveRiver::audio_enabled),
        );
    }

    /// Replace every developer drain; live rivers keep their water.
    pub(in crate::renderer) fn set_river_drains(&mut self, drains: Vec<crate::river_runtime::LocalDrain>) {
        for river in &mut self.live_rivers {
            river.set_drains(&self.queue, &drains);
        }
        self.river_drains = drains;
    }

    pub(in crate::renderer) fn clear_live_rivers(&mut self) {
        self.refresh_river_ownership(false, None);
        self.river_promotion_claims.clear();
        self.live_rivers.clear();
        self.river_ninja_history = None;
        self.river_ninja_textures = None;
        self.river_ninja_caustics = None;
        self.river_ninja_entities = None;
        self.river_drains.clear();
        if let Some(surface) = &mut self.river_surface {
            surface.clear_ninja_underwater_inputs();
            surface.reset_audio_probe();
        }
        crate::river_runtime::set_audio_rivers_present(false);
    }

    /// Native frame for each patch and the shared FFT detail, before any scene
    /// pass, plus the water body's pipeline/copy/view for this frame's target.
    /// `scene_format` is the scene pass's colour format; `copyable` is false
    /// when that target is the swapchain (no post, no MSAA).
    pub(in crate::renderer) fn encode_live_rivers(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame_delta: f32,
        camera: &super::Camera,
        scene_format: wgpu::TextureFormat,
        copyable: bool,
        allow_replacement: bool,
        area_mask: Option<&[u8; 32]>,
    ) {
        let has_rivers = self.river_surface.is_some() && !self.live_rivers.is_empty();
        let ninja_requested = has_rivers && jka_river::river_ninja::actor_capture_requested();
        let underwater_specs = if has_rivers && copyable && self.live_rivers.iter().any(LiveRiver::uses_ninja_underwater) {
            jka_river::river_ninja::river_underwater_images()
        } else {
            None
        };
        let underwater_requested = underwater_specs.is_some();
        if !ninja_requested {
            self.river_ninja_history = None;
            self.river_ninja_textures = None;
        } else if self.river_ninja_textures.is_none() {
            // Keep failures until the renderer is rebuilt, rather than retrying every frame.
            let loaded = self.load_ninja_input_textures(jka_river::river_ninja::river_input_images());
            if let Err(error) = &loaded {
                eprintln!("[RIVER NINJA] inputs unavailable: {error}");
            }
            self.river_ninja_textures = Some(loaded.ok());
        }
        if !underwater_requested && !self.river_ninja_textures.as_ref().is_some_and(Option::is_some) {
            self.river_ninja_caustics = None;
        } else if self.river_ninja_caustics.is_none() {
            let volume = jka_river::river_ninja::river_caustics_image().and_then(|spec| {
                match self.load_ninja_caustics(spec) {
                    Ok(view) => Some(view),
                    Err(error) => {
                        eprintln!("[RIVER NINJA] caustics unavailable: {error}");
                        None
                    }
                }
            });
            self.river_ninja_caustics = Some(volume);
        }
        if let Some(specs) = underwater_specs {
            if !self.river_surface.as_ref().is_some_and(RiverSurface::has_ninja_underwater_inputs) {
                let [noise, gradient] = self.load_ninja_underwater_images(specs);
                let caustics = self.river_ninja_caustics.as_ref().and_then(Option::as_ref);
                self.river_surface.as_mut().expect("checked above").set_ninja_underwater_inputs(
                    &self.device, noise.as_ref(), gradient.as_ref(), caustics,
                );
            }
        } else if let Some(water) = &mut self.river_surface {
            water.clear_ninja_underwater_inputs();
        }
        let Some(water) = self
            .river_surface
            .as_mut()
            .filter(|_| !self.live_rivers.is_empty())
        else {
            return;
        };
        let delta = if frame_delta.is_finite() {
            frame_delta.max(0.0)
        } else {
            0.0
        };
        water.clock += f64::from(delta);
        let clock = water.clock;
        let ninja_game_seconds = if ninja_requested || underwater_requested {
            self.scene_time.map(f64::from).or_else(|| self.weather_server_time_ms.map(|t| f64::from(t) / 1000.0))
        } else {
            None
        };
        if let Err(error) = water.encode_ocean(&self.device, encoder, clock as f32) {
            eprintln!("[RIVER] FFT detail failed, patches removed: {error}");
            self.refresh_river_ownership(false, None);
            self.river_promotion_claims.clear();
            self.live_rivers.clear();
            if let Some(surface) = &mut self.river_surface { surface.reset_audio_probe(); }
            crate::river_runtime::set_audio_rivers_present(false);
            return;
        }
        water.ensure_pipeline(
            &self.device,
            &self.camera_layout,
            &self.shadow_receiver_layout,
            scene_format,
            self.msaa_samples,
            super::depth_format(),
            super::camera_depth_compare(),
            self.live_rivers.len() == 1 && crate::river_runtime::fused_material(),
        );
        let size = [
            self.targets._scene_texture.width(),
            self.targets._scene_texture.height(),
        ];
        let blue_water_ambient = (self.sun_override || self.sky_backend == 1) && self.blue_water_sky.is_some();
        {
            let sun = self.river_scene_sun();
            let mut view = ViewUniform::new(
                camera,
                size,
                sun,
                blue_water_ambient,
                clock as f32,
                delta.min(0.1),
            );
            if ninja_requested || underwater_requested {
                view.right[3] = ninja_game_seconds.unwrap_or(clock) as f32;
            }
            if underwater_requested {
                view.up[3] = if self.river_ninja_caustics.as_ref().is_some_and(Option::is_some) { 2.0 } else { 1.0 };
            }
            // Bed clamp, vertex half (not Unreal; River menu A/B).
            view.time[3] = if crate::river_runtime::bed_clamp() { 1.0 } else { 0.0 };
            // Render-only material LOD. Shared by the base, composite, and far draws.
            view.material_lod[0] = f32::from(crate::river_runtime::material_lod());
            view.material_lod[1] = crate::river_runtime::material_lod_cutoff();
            view.material_lod[2] = if crate::river_runtime::adaptive_ssr() { 1.0 } else { 0.0 };
            // r_underwaterReflections, shared with the ocean.
            view.material_lod[3] = if crate::ocean::underwater_reflections() { 1.0 } else { 0.0 };
            if ninja_requested {
                view.ninja_debug[0] = jka_river::river_ninja::material_debug();
            }
            let water = self.river_surface.as_mut().expect("checked above");
            water.prepare(&self.device, &self.queue, size, self.msaa_samples, &view);
        }
        let water = self.river_surface.as_mut().expect("checked above");
        if !copyable && !water.warned_unsampleable {
            water.warned_unsampleable = true;
            eprintln!("[RIVER] the water look needs post-processing or MSAA (the scene must be copyable); not drawn");
        }
        // Sparse (long-distance) rivers keep their presentation window around
        // the camera; dense patches ignore this.
        {
            let eye = jka_render_core::coordinates::jka_position(camera.position.to_array());
            let grid = self.world.as_ref().and_then(|world| world.entity_light_grid.as_ref());
            let map_toward_sun = grid.and_then(|grid| grid.map_toward_sun());
            for river in &mut self.live_rivers {
                river.update_view(
                    &self.device,
                    &self.queue,
                    eye,
                    |point| grid.and_then(|grid| grid.sample_classic_entity_light(point)),
                    map_toward_sun,
                );
            }
            if let Some(water) = self.river_surface.as_ref() {
                for river in &mut self.live_rivers {
                    river.update_block_frustum(&self.queue, water);
                }
            }
        }
        // Player wake: the Water menu's "Player water deformation" toggle and
        // "Deformation strength" (1 = native body velocity). Players are the
        // presented records the ocean reads (filled while cg_waterwake or
        // ocean interaction is on); each is a JKA player hull. The Fluid Ninja river
        // also enables it: the Fluid Ninja field only shades, so without the
        // solver's body collisions its wake would never move the mesh.
        {
            let (interaction, strength) = self.river_interaction;
            let enabled = interaction || jka_river::river_ninja::actor_capture_requested();
            let bodies: Vec<crate::river_runtime::MapBody> = if enabled {
                self.river_actors.iter().map(river_body).collect()
            } else {
                Vec::new()
            };
            for river in &mut self.live_rivers {
                river.body_source = (enabled, self.river_actors.len());
                river.set_bodies(&self.device, &self.queue, &bodies, strength);
            }
        }
        // Whitewater ViewRecycler camera (r_riverWhitewater).
        {
            let view = jka_river::river_secondary::SprayView::from_camera(camera, size);
            for river in &mut self.live_rivers {
                river.set_spray_view(Some(view));
            }
        }
        let ninja_source = if ninja_requested {
            let history = self.river_ninja_history.get_or_insert_with(Default::default);
            let extras = self.river_ninja_entities.as_deref().map(Vec::as_slice).unwrap_or(&[]);
            let tracks = history.update(extras.iter().copied().filter(|actor| actor.kind.has_river_contact_shape()));
            let scene_revision = self.scene_time.map(|_| self.studio_revision);
            let textures = self.river_ninja_textures.as_ref().and_then(Option::as_ref);
            let images = textures.map(|t| std::array::from_fn(|i| &t.images[i].view));
            let caustics = self.river_ninja_caustics.as_ref().and_then(Option::as_ref);
            let debug = jka_river::river_ninja::wake_debug_enabled().then(|| {
                if scene_revision.is_some() { Default::default() }
                else { self.river_wake_debug_subject.unwrap_or_default() }
            });
            Some(jka_river::river_ninja::FrameSource { game_seconds: ninja_game_seconds, scene_revision, tracks, images, caustics, queue: &self.queue, debug })
        } else {
            self.river_ninja_history = None;
            None
        };
        let water = self.river_surface.as_ref();
        let device = &self.device;
        let profiler = &mut self.gpu_profiler;
        let mut solve_timestamp = |encoder: &mut wgpu::CommandEncoder, end: bool| {
            profiler.write_encoder_timestamp(encoder, super::GpuPass::RiverSolve, end);
        };
        self.live_rivers.retain_mut(|river| {
            match river.encode_frame_with_ninja(device, encoder, frame_delta, clock, water, &mut solve_timestamp, ninja_source) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("[RIVER] patch removed: {error}");
                    false
                }
            }
        });
        let ready = allow_replacement && copyable && self.river_surface.as_ref()
            .is_some_and(|s| s.ready_for(scene_format, self.msaa_samples));
        self.refresh_river_ownership(ready, area_mask);
        let audible = self.live_rivers.iter().any(LiveRiver::audio_enabled);
        crate::river_runtime::set_audio_rivers_present(audible);
        // Sample listener submersion from the solved and displaced river
        // height in one 1x1 GPU pass, no synchronous map or render stall.
        if let Some(water) = self.river_surface.as_mut().filter(|_| audible) {
            let environment = self.blue_water_sky.as_ref().filter(|_| self.sun_override || self.sky_backend == 1)
                .map(|atmosphere| &atmosphere.sky.environment().bind_group);
            water.encode_audio_probe(
                &self.device, &self.queue, encoder,
                &self.camera_layout, &self.shadow_receiver_layout,
                &self.camera_bind_group, &self.shadow_resources.receiver_bind_group,
                environment, &self.live_rivers,
            );
        }
        // Water mesh quadtree beyond the camera window: per-view traversal
        // (frustum side planes of this camera; depth planes are not tested).
        if let Some(water) = self.river_surface.as_ref() {
            let view_proj = camera.view_projection(size[0], size[1]);
            let planes = [
                view_proj.row(3) + view_proj.row(0),
                view_proj.row(3) - view_proj.row(0),
                view_proj.row(3) + view_proj.row(1),
                view_proj.row(3) - view_proj.row(1),
            ];
            let cm = f64::from(jka_water::river::CENTIMETRES_PER_MAP_UNIT);
            let frustum = |centre: [f64; 3], extent: [f64; 3]| {
                // River cm (JKA axes) -> render space map units: (x, z, -y).
                let c = glam::Vec3::new((centre[0] / cm) as f32, (centre[2] / cm) as f32, (-centre[1] / cm) as f32);
                let e = glam::Vec3::new((extent[0] / cm) as f32, (extent[2] / cm) as f32, (extent[1] / cm) as f32) * 1.05;
                planes.iter().all(|p| p.truncate().dot(c) + p.w + p.truncate().abs().dot(e) >= 0.0)
            };
            let eye = jka_render_core::coordinates::jka_position(camera.position.to_array()).map(|v| f64::from(v) * cm);
            let view = jka_river::river_far::FarView { eye_cm: eye, frustum: &frustum, lod_morphing: jka_river::river_far::lod_morph() };
            let grid = self.world.as_ref().and_then(|world| world.entity_light_grid.as_ref());
            let map_toward_sun = grid.and_then(|grid| grid.map_toward_sun());
            let mut light = |point: [f32; 3]| grid.and_then(|grid| grid.sample_classic_entity_light(point));
            for river in &mut self.live_rivers {
                river.prepare_far(&self.device, &self.queue, water, &view, &mut light, map_toward_sun);
            }
        }
        let debug = crate::river_runtime::debug_mode();
        if debug > 0 {
            let pipeline_ready = self.river_surface.as_ref().is_some_and(|surface| surface.ready_for(scene_format, self.msaa_samples));
            for (index, river) in self.live_rivers.iter_mut().enumerate() {
                river.debug_report(index, debug, copyable, pipeline_ready);
            }
        }
    }
}

/// Ocean2 has a separate authoritative texture inventory. No PNG implies a
/// color-space default: the metadata file is exported from the source UE project.
/// `coast` selects the Fluid Ninja Coast inventory, whose color spaces are all
/// established; its optional authority file only carries the host resolution.
pub(in crate::renderer) fn load_ninja_ocean_assets(
    device: &wgpu::Device, queue: &wgpu::Queue, base: &std::path::Path, game: Option<&std::path::Path>, coast: bool,
) -> Result<crate::ocean::NinjaOceanAssets, String> {
    let specs = if coast { jka_ninja::coast_images() } else { jka_ninja::ocean_images() }
        .ok_or("Fluid Ninja private provider not included")?;
    let mut assets = super::AssetSearchPath::open_game(base, game).map_err(|e|e.to_string())?;
    let qpath = "textures/japro/ocean2_authority.json";
    let meta = assets.read(qpath, 8192).map_err(|e|e.to_string())?;
    let authority: crate::ocean::NinjaOceanAuthority = match meta {
        Some(meta) => serde_json::from_slice(&meta.bytes).map_err(|e|format!("{qpath}: {e}"))?,
        None if coast => crate::ocean::NinjaOceanAuthority::coast_reference(),
        None => return Err(format!("missing {qpath}; run the UE Ocean2 authority exporter")),
    };
    authority.validate()?;
    let mut images = Vec::with_capacity(8);
    for spec in specs.simulation { images.push(decode_ninja_image(&mut assets, &spec)?); }
    for (i, spec) in specs.material.into_iter().enumerate() {
        let srgb = match spec.srgb { Some(v) => v, None if i == 2 => authority.tile1_srgb,
            None if i == 3 => authority.tile2_srgb, None => return Err(format!("unresolved color space: {}",spec.qpath)) };
        images.push(decode_ninja_image(&mut assets, &RiverImage { qpath:spec.qpath,size:spec.size,srgb })?);
    }
    let mem=device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let val=device.push_error_scope(wgpu::ErrorFilter::Validation);
    let views:Vec<_> = images.iter().map(|data| super::textures::upload_texture(device,queue,data).view.clone()).collect();
    let a=jka_river::river_ninja::checked_scope(val);let m=jka_river::river_ninja::checked_scope(mem);a?;m?;
    let [v0,v1,v2,v3,v4,v5,v6,v7]:[wgpu::TextureView;8]=views.try_into().map_err(|_|"Ocean2 texture inventory".to_string())?;
    // Coast MI CausticsTileComponentVolumeTexture = T_Caustics_Animated_02, the
    // same volume the river port builds from its 8-bit atlas.
    let caustics = if !coast { None } else {
        jka_river::river_ninja::river_caustics_image().and_then(|spec| {
            let volume = decode_ninja_image(&mut assets, &spec.image).and_then(|data| {
                let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let volume = upload_ninja_volume(device, queue, &data, &spec);
                jka_river::river_ninja::checked_scope(scope)?;
                volume
            });
            volume.map_err(|e| eprintln!("[OCEAN NINJA] Coast caustics unavailable: {e}")).ok()
        })
    };
    Ok(crate::ocean::NinjaOceanAssets { simulation:[v0,v1,v2,v3],material:[v4,v5,v6,v7],authority,caustics })
}
