//! Optional Fluid Ninja Coast render/simulation adapter (Water_Dense_Sea /
//! NinjaLive_Water_100m). The private provider owns the solver, the Landscape
//! DI capture and the Coast material; this module owns the world-anchored
//! patch, the BSP bed raster upload, FFT crest injection and render scheduling.
//! Same pool/fallback contract as `ninja_ocean::Renderer`.
use std::sync::Arc;
use jka_ninja::{CoastDesc, CoastField, CoastFrame, WAKE_MAX_TRACKS as MAX_TRACKS};
use jka_river::river_ninja::{ActorHistory, ActorSample, checked_scope, timeline_epoch, wake_points};
use blue_water_gpu::{original_surface::OriginalSurfacePipeline, shader_composer};
use super::ninja_ocean::{Assets, b, buf, draw_pipeline, s, sampler, storage, t, texture, uniform};
use super::upstream_coast::{CoastPatch, Source};
use super::{OCEAN_MAX_SURFACES, OCEAN_UNITS_PER_METER};

/// Reference actor ExtentsXYZ 100 x 100 m.
/// r_ninjaCoastFollowSwell: share of the wave drive a lee shore (facing away from the swell) loses.
const SWELL_EXPOSURE: f32 = 0.75;
/// Eye distance (m) to the nearest window water past which no Coast pixel has sim share (host
/// eye fade 45 m, `ninja_share`), with 10 m for surface displacement, swash and the wet-sand
/// margin beyond the run-up cells: the wet-map receiver is invisible past it.
const SIM_SHARE_HIDDEN_DISTANCE_M: f32 = 55.0;
/// Eye distance (m) to the nearest window water past which every whitewater sprite is fully
/// faded (85 m far fade + 10 m for spray above and beside the water cells).
const WHITEWATER_HIDDEN_DISTANCE_M: f32 = 95.0;
/// Bed height above the clamp (JKA units) the Coast surface can still cover: the swash
/// run-up (`NINJA_COAST_RUNUP_CM`, 150 cm).
const WATER_RUNUP_UNITS: f32 = 150.0 / CM_PER_UNIT;

/// Some window pixel can carry sim share: the window holds bed inside the shader's shore band
/// (`ninja_shore_weight` is 0 past it and on unknown bed). Always with shore depth 0.
fn sim_shaded(shore_depth_m: f32, blend: f32, highest_bed: f32) -> bool {
    shore_depth_m <= 0.0 || highest_bed >= -shore_depth_m * blend * OCEAN_UNITS_PER_METER
}

/// Highest known bed of a window patch (JKA units above the clamp), -inf when none is known.
/// `r_ninjaCoastSimQuality` -> (solver cells, RT_VelocityDensity export size; 0 = solver size).
/// 3: reference solver and full-size export (the original port); 2: reference solver and the
/// UE water actor's 512² export; 1 and 0: half and quarter solver resolution.
fn sim_resolution(reference: u32, quality: u8) -> (u32, u32) {
    let cells = (reference >> (2 - quality.min(2))).max(256);
    (cells, if quality >= 3 { 0 } else { 512 })
}

fn highest_bed(patch: &CoastPatch) -> f32 {
    patch.heights.iter().copied().filter(|h| h.is_finite()).fold(f32::NEG_INFINITY, f32::max)
}

/// Window cells the Coast surface may cover: unknown bed (open sea) or bed at most the run-up.
fn water_mask(patch: &CoastPatch) -> Vec<bool> {
    patch.heights.iter().map(|h| !h.is_finite() || *h <= WATER_RUNUP_UNITS).collect()
}
pub(super) const EXTENT_M: f32 = 100.0;
const CM_PER_UNIT: f32 = 100.0 / OCEAN_UNITS_PER_METER;
/// The window follows the clipmap centre once it is this many capture cells
/// (~1 m) away; smaller moves keep the bed upload off most frames.
const FOLLOW_CELLS: i32 = 5;
/// r_hybridMaterial 4 = mode 5 with the Blue Water FFT layered under the Coast
/// surface (displacement, slope normal, eye/contact heights). The FFT's
/// breaking-crest foam is not injected into the Coast simulation, so the sim
/// is identical to mode 5 (UE's Coast has no FFT). Set true to restore the
/// earlier crest-to-sim feed (MeshField source) for A/B.
const FFT_FEEDS_SIM: bool = false;
const CLAMP_TO_ACTOR_CM: f32 =jka_ninja::COAST_CLAMP_CM - jka_ninja::COAST_SIM_Z_CM;
/// The reference NinjaLive actor is attached to ThirdPersonCharacter's
/// CollisionCylinder at RelativeLocation.z -47 with ForceSimInitialVerticalPos
/// false, so its flat output mesh rides at the pawn's capsule centre - 47 cm.
/// UE 5.6.1 PIE readback (tools/coast/capture_coast_pressure.py): capsule
/// centre -298.44, scaled half height 90, water component -345.44: feet + 43 cm.
const MESH_ABOVE_FEET_CM: f32 = 90.0 - 47.0;
/// JKA standing feet below the eye: DEFAULT_VIEWHEIGHT 36 + -MINS_Z 24 (bg_public.h).
const FEET_BELOW_EYE_UNITS: f32 = 60.0;

/// UE Z of the flat output mesh (system position) for a plane-relative eye
/// height in native metres. The solver keeps its fixed SimZ datum: its
/// terms do not depend on it, and the material's height alignment
/// `HeightField - (WorldZ - TraceZ)` reconciles the two frames exactly; only
/// `DistanceToNearestSurface` of the flat mesh (the WPO dynamic clamp) moves.
fn mesh_z_cm(eye_y_m: f32) -> f32 {
    jka_ninja::COAST_CLAMP_CM + eye_y_m * 100.0 - FEET_BELOW_EYE_UNITS * CM_PER_UNIT + MESH_ABOVE_FEET_CM
}

struct Slot {
    /// First capture cell of the window (JKA units / `cell`), JKA plane height,
    /// and the derived window centre in native metres xz.
    origin: [i32; 2], cell: f32, size: u32, plane: f32, center: [f32; 2], field: CoastField,
    /// RT_VelocityDensity export size the field was built with (0 = solver size).
    vd: u32,
    /// Flat output mesh Z (UE cm), from the eye passed to the last `prepare`.
    mesh_z_cm: f32,
    terrain: wgpu::Texture, terrain_material: wgpu::Texture, terrain_material_view: wgpu::TextureView,
    params: wgpu::Buffer, capture_params: wgpu::Buffer, eye_params: wgpu::Buffer, probe_params: wgpu::Buffer,
    probe_camera: wgpu::Buffer, probe_eye: wgpu::Buffer, crest: wgpu::Texture, crest_view: wgpu::TextureView,
    points: wgpu::Buffer, flags: wgpu::Buffer,
    capture_group: Option<([wgpu::TextureView; 3], wgpu::BindGroup)>,
    draw_group: Option<([wgpu::TextureView; 4], wgpu::BindGroup)>,
    /// "PARTICLES 100m simdriven - whitewater" state, created on the first tick.
    whitewater: Option<super::ninja_coast_whitewater::Particles>,
    /// Auto wave-drive gain of the current bed window (`auto_wave_gain`).
    wave_gain: f32,
    /// Cells the Coast surface can cover (bed unknown = open sea, or at most the swash
    /// run-up above the clamp), row j = JKA +Y: the nearest-water distance gates.
    water: Vec<bool>,
    /// Highest captured bed in the window (JKA units above the clamp; -inf when none is known).
    highest_bed: f32,
    /// The bed is the all-unknown placeholder (open sea at the clamp, the
    /// capture's unknown-column rule) until the worker's tiles arrive, so the
    /// Coast surface draws from the first frame instead of authored water.
    provisional: bool,
}

/// IEEE binary16 bits, round to nearest (no `half` dependency in this crate).
fn f16_bits(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32 - 112;
    let mant = x & 0x7f_ffff;
    if exp <= 0 {
        if exp < -10 { return sign; }
        let m = (mant | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    if exp >= 31 { return sign | 0x7c00; }
    sign | (((exp as u32) << 10) + ((mant + 0x1000) >> 13)).min(0x7c00) as u16
}

fn bed_texture(device: &wgpu::Device, label: &str, size: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label), size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
    })
}

/// Bed relative to the clamp plane in UE cm; row j = JKA +Y = field unit V.
/// Exact (R height, G valid) for the capture, f16 (height, normal xy, valid) for the material.
/// Parity diagnostic: `DINURDO_COAST_BED=<file>` replaces the BSP bed with a
/// UE bed trace (RG f32 N x N, R = UE world Z cm, G = valid; row = +Y), e.g.
/// DinurdoJK-ninja/tools/coast/capture_coast_bed.py, bilinearly resampled to
/// the capture grid. Lets the solver run on the reference bathymetry.
fn override_bed(n: u32) -> Option<Vec<f32>> {
    let path = std::env::var_os("DINURDO_COAST_BED")?;
    let bytes = std::fs::read(&path).map_err(|e| eprintln!("[OCEAN NINJA] Coast bed override: {e}")).ok()?;
    let values: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let size = ((values.len() / 2) as f64).sqrt() as usize;
    if size < 2 || size * size * 2 != values.len() { eprintln!("[OCEAN NINJA] Coast bed override: not square RG f32"); return None; }
    let get = |i: usize, j: usize| { let k = (j * size + i) * 2; (values[k + 1] > 0.5).then_some(values[k] - jka_ninja::COAST_CLAMP_CM) };
    let mut out = vec![f32::NAN; (n * n) as usize];
    for j in 0..n as usize {
        for i in 0..n as usize {
            let f = |c: usize| ((c as f32 + 0.5) / n as f32 * size as f32 - 0.5).clamp(0.0, (size - 1) as f32);
            let (x, y) = (f(i), f(j));
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(size - 1), (y0 + 1).min(size - 1));
            let (tx, ty) = (x - x0 as f32, y - y0 as f32);
            if let (Some(a), Some(b), Some(c), Some(d)) = (get(x0, y0), get(x1, y0), get(x0, y1), get(x1, y1)) {
                out[j * n as usize + i] = (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty;
            }
        }
    }
    eprintln!("[OCEAN NINJA] Coast bed override: {} ({size}x{size} -> {n}x{n})", std::path::Path::new(&path).display());
    Some(out)
}

/// Parity diagnostic: `DINURDO_COAST_SEED=<file>` seeds a new window's
/// velocity/density from a square RGBA f32 RT_VelocityDensity readback (UE or
/// `r_ninjaCoastDump`), nearest-resampled to the solver grid.
fn seed_velocity_density(size: [u32; 2]) -> Option<Vec<u16>> {
    let path = std::env::var_os("DINURDO_COAST_SEED")?;
    let bytes = std::fs::read(&path).map_err(|e| eprintln!("[OCEAN NINJA] Coast seed: {e}")).ok()?;
    let values: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let n = ((values.len() / 4) as f64).sqrt() as usize;
    if n < 2 || n * n * 4 != values.len() { eprintln!("[OCEAN NINJA] Coast seed: not square RGBA f32"); return None; }
    let [w, h] = size.map(|v| v as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for j in 0..h {
        for i in 0..w {
            let k = ((j * n / h) * n + i * n / w) * 4;
            out.extend(values[k..k + 4].iter().map(|v| f16_bits(*v)));
        }
    }
    eprintln!("[OCEAN NINJA] Coast seed: {} ({n}x{n} -> {w}x{h})", std::path::Path::new(&path).display());
    Some(out)
}

/// Host substitution for UE's Landscape DI, which has a height everywhere:
/// columns the BSP raster cannot resolve (overhangs under roofs, palms,
/// umbrellas, stair walls) take the mean of their nearest resolved neighbours
/// (multi-source breadth-first fill). Left unknown, the capture treats them as
/// open sea at the clamp, and the density refill (`elevation <= clamp + 10 cm`)
/// turned each one into a permanent spring on dry sand that flooded the beach.
fn fill_unknown_bed(heights: &mut [f32], n: usize) {
    let mut frontier: Vec<usize> = (0..heights.len())
        .filter(|&k| !heights[k].is_finite() && neighbours(k, n).any(|m| heights[m].is_finite()))
        .collect();
    let mut next = Vec::new();
    while !frontier.is_empty() {
        // Assign the whole ring from the previous rings only, so the fill
        // does not depend on scan order.
        let values: Vec<f32> = frontier.iter().map(|&k| {
            let (sum, count) = neighbours(k, n).filter_map(|m| heights[m].is_finite().then_some(heights[m]))
                .fold((0.0, 0.0), |(s, c), h| (s + h, c + 1.0));
            sum / count
        }).collect();
        for (&k, v) in frontier.iter().zip(values) { heights[k] = v; }
        for &k in &frontier {
            next.extend(neighbours(k, n).filter(|&m| !heights[m].is_finite()));
        }
        next.sort_unstable();
        next.dedup();
        std::mem::swap(&mut frontier, &mut next);
        next.clear();
    }
}

fn neighbours(k: usize, n: usize) -> impl Iterator<Item = usize> {
    let (i, j) = (k % n, k / n);
    [(i > 0).then(|| k - 1), (i + 1 < n).then(|| k + 1), (j > 0).then(|| k - n), (j + 1 < n).then(|| k + n)]
        .into_iter().flatten()
}

/// Mean seabed slope at the Landscape DI footprint (the wave drive's normal):
/// 10 m vertex boxes (mean of the known raster, at least a quarter known),
/// sea vertices only (most known cells below L-1), central differences, the
/// outer 10 % of vertices skipped. Same metric as
/// DinurdoJK-ninja/tools/coast/analysis/footslope.py, which gives UE's
/// reference bed `COAST_SEA_FOOTPRINT_SLOPE`. `heights`: UE cm relative to L.
fn sea_footprint_slope(heights: &[f32], n: usize, cell_cm: f32) -> Option<f32> {
    let k = ((1000.0 / cell_cm).round() as usize).max(1);
    let m = n / k;
    if m < 5 { return None; }
    let mut vertex = vec![f32::NAN; m * m];
    let mut sea = vec![false; m * m];
    for vj in 0..m {
        for vi in 0..m {
            let (mut sum, mut known, mut wet) = (0.0f32, 0usize, 0usize);
            for j in vj * k..(vj + 1) * k {
                for &h in &heights[j * n + vi * k..j * n + (vi + 1) * k] {
                    if h.is_finite() { sum += h; known += 1; if h < -1.0 { wet += 1; } }
                }
            }
            if known * 4 >= k * k {
                vertex[vj * m + vi] = sum / known as f32;
                sea[vj * m + vi] = wet * 2 > known;
            }
        }
    }
    let spacing = k as f32 * cell_cm;
    let edge = (m / 10).max(1);
    let (mut total, mut count) = (0.0f32, 0usize);
    for vj in edge..m - edge {
        for vi in edge..m - edge {
            if !sea[vj * m + vi] { continue; }
            let gx = (vertex[vj * m + vi + 1] - vertex[vj * m + vi - 1]) / (2.0 * spacing);
            let gy = (vertex[(vj + 1) * m + vi] - vertex[(vj - 1) * m + vi]) / (2.0 * spacing);
            let s = gx.hypot(gy);
            if s.is_finite() { total += s; count += 1; }
        }
    }
    (count > 0).then(|| total / count as f32)
}

/// Host calibration of the wave drive (COAST_HANDOFF item 1, session 8): UE's
/// drive strength is its Landscape DI gain on its own seabed; scale the gain by
/// the reference / this window's footprint slope so a steeper (or gentler) bed
/// does not push the sea harder (or softer) than the reference level does.
fn auto_wave_gain(slope: Option<f32>) -> f32 {
    match slope {
        Some(s) if s > 1e-4 => (jka_ninja::COAST_LANDSCAPE_NORMAL_GAIN * jka_ninja::COAST_SEA_FOOTPRINT_SLOPE / s).clamp(0.5, 30.0),
        _ => jka_ninja::COAST_LANDSCAPE_NORMAL_GAIN,
    }
}

/// Uploads the bed and returns the auto wave gain for it (see `auto_wave_gain`).
fn write_bed(queue: &wgpu::Queue, exact_tex: &wgpu::Texture, material_tex: &wgpu::Texture, patch: &CoastPatch, n: u32, cell: f32) -> f32 {
    let cell_cm = cell * CM_PER_UNIT;
    let mut heights = override_bed(n)
        .unwrap_or_else(|| patch.heights.iter().map(|h| h * CM_PER_UNIT).collect());
    let gain = auto_wave_gain(sea_footprint_slope(&heights, n as usize, cell_cm));
    fill_unknown_bed(&mut heights, n as usize);
    let at = |i: isize, j: isize| -> Option<f32> {
        let (i, j) = (i.clamp(0, n as isize - 1) as usize, j.clamp(0, n as isize - 1) as usize);
        let h = heights[j * n as usize + i];
        h.is_finite().then_some(h)
    };
    let mut exact: Vec<f32> = Vec::with_capacity(patch.heights.len() * 2);
    let mut material: Vec<u16> = Vec::with_capacity(patch.heights.len() * 4);
    for j in 0..n as isize {
        for i in 0..n as isize {
            match at(i, j) {
                Some(h) => {
                    let slope = |a: Option<f32>, b: Option<f32>| match (a, b) {
                        (Some(a), Some(b)) => (a - b) / (2.0 * cell_cm),
                        (Some(a), None) => (a - h) / cell_cm,
                        (None, Some(b)) => (h - b) / cell_cm,
                        (None, None) => 0.0,
                    };
                    let dx = slope(at(i + 1, j), at(i - 1, j));
                    let dy = slope(at(i, j + 1), at(i, j - 1));
                    let len = (dx * dx + dy * dy + 1.0).sqrt();
                    exact.extend_from_slice(&[h, 1.0]);
                    material.extend_from_slice(&[h, -dx / len, -dy / len, 1.0].map(f16_bits));
                }
                None => {
                    exact.extend_from_slice(&[0.0, 0.0]);
                    material.extend_from_slice(&[0.0f32; 4].map(f16_bits));
                }
            }
        }
    }
    let extent = wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 1 };
    let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(n * 8), rows_per_image: Some(n) };
    queue.write_texture(exact_tex.as_image_copy(), bytemuck::cast_slice(&exact), layout, extent);
    queue.write_texture(material_tex.as_image_copy(), bytemuck::cast_slice(&material), layout, extent);
    gain
}

impl Slot {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, assets: &Assets, patch: &CoastPatch, origin: [i32; 2],
        cell: f32, n: u32, plane: f32, resolution: u32, vd: u32) -> Result<Self, String> {
        let mem = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let val = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let terrain = bed_texture(device, "Ninja coast bed raster", n, wgpu::TextureFormat::Rg32Float);
        let terrain_material = bed_texture(device, "Ninja coast bed material", n, wgpu::TextureFormat::Rgba16Float);
        let wave_gain = write_bed(queue, &terrain, &terrain_material, patch, n, cell);
        eprintln!("[OCEAN NINJA] Coast window seabed -> auto wave gain {wave_gain:.2} (UE reference {})", jka_ninja::COAST_LANDSCAPE_NORMAL_GAIN);
        let terrain_view = terrain.create_view(&Default::default());
        let terrain_material_view = terrain_material.create_view(&Default::default());
        let checked = checked_scope(val); let m = checked_scope(mem); checked?; m?;
        let centre_jka = origin.map(|o| (o as f32 + n as f32 * 0.5) * cell);
        let desc = CoastDesc { resolution: [resolution; 2], velocity_density_resolution: vd, extent_cm: [EXTENT_M * 100.0; 2],
            centre_cm: centre_jka.map(|c| c * CM_PER_UNIT), max_tracks: MAX_TRACKS as u32 };
        let mut field = CoastField::new(device, queue, &desc, assets.simulation.each_ref(), &terrain_view)?;
        if let Some(seed) = seed_velocity_density(field.solver_resolution()) {
            field.seed_velocity_density(queue, &seed)?;
        }
        let [width, height] = field.capture_resolution();
        if [width, height] != [n, n] { return Err("Ninja coast: bed raster does not match the capture grid".into()); }
        let mem = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let val = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let crest = device.create_texture(&wgpu::TextureDescriptor { label: Some("Ninja coast pre-limit crest density"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
        let crest_view = crest.create_view(&Default::default());
        let u = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        let out = Self { origin, cell, size: n, plane, center: Self::center_of(origin, cell, n), field, vd,
            mesh_z_cm: jka_ninja::COAST_SIM_Z_CM, terrain, terrain_material, terrain_material_view,
            params: buf(device, "Ninja coast draw params", 128, u), capture_params: buf(device, "Ninja coast capture params", 64, u),
            eye_params: buf(device, "Ninja coast eye params", 64, u), probe_params: buf(device, "Ninja coast probe params", 64, u),
            probe_camera: buf(device, "Ninja coast probe camera", 144, u),
            probe_eye: buf(device, "Ninja coast probe eye cache", 16, wgpu::BufferUsages::STORAGE), crest, crest_view,
            points: buf(device, "Ninja coast contact points", (MAX_TRACKS * 16) as u64, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST),
            flags: buf(device, "Ninja coast contact flags", (MAX_TRACKS * 4) as u64, wgpu::BufferUsages::STORAGE),
            capture_group: None, draw_group: None, whitewater: None, wave_gain, water: water_mask(patch), highest_bed: highest_bed(patch), provisional: false };
        let checked = checked_scope(val); let m = checked_scope(mem); checked?; m?;
        Ok(out)
    }
    fn center_of(origin: [i32; 2], cell: f32, n: u32) -> [f32; 2] {
        let c = origin.map(|o| (o as f32 + n as f32 * 0.5) * cell);
        [c[0] / OCEAN_UNITS_PER_METER, -c[1] / OCEAN_UNITS_PER_METER]
    }
    /// Move the simulated window to `origin` (whole capture cells): new bed,
    /// then Niagara's WorldOffsetDelta on the next tick.
    fn follow(&mut self, queue: &wgpu::Queue, patch: &CoastPatch, origin: [i32; 2]) {
        self.wave_gain = write_bed(queue, &self.terrain, &self.terrain_material, patch, self.size, self.cell);
        self.water = water_mask(patch);
        self.highest_bed = highest_bed(patch);
        self.field.shift([origin[0] - self.origin[0], origin[1] - self.origin[1]]);
        self.origin = origin;
        self.center = Self::center_of(origin, self.cell, self.size);
    }
    /// 3D distance (native m) from `eye` to the nearest water cell of this window on the
    /// plane; infinite when the window holds none (its sim share is then 0 everywhere).
    fn nearest_water_m(&self, eye: [f32; 3]) -> f32 {
        let n = self.size as usize;
        let ex = eye[0] * OCEAN_UNITS_PER_METER / self.cell - self.origin[0] as f32 - 0.5;
        let ey = -eye[2] * OCEAN_UNITS_PER_METER / self.cell - self.origin[1] as f32 - 0.5;
        let mut best = f32::INFINITY;
        for (k, _) in self.water.iter().enumerate().filter(|(_, w)| **w) {
            let (dx, dy) = ((k % n) as f32 - ex, (k / n) as f32 - ey);
            best = best.min(dx * dx + dy * dy);
        }
        let horizontal = best.sqrt() * self.cell / OCEAN_UNITS_PER_METER;
        let height = eye[1] - self.plane / OCEAN_UNITS_PER_METER;
        (horizontal * horizontal + height * height).sqrt()
    }
    fn capture_uniform(&self, eye: [f32; 3], time: f32, fft: bool, count: usize, spacing: f32, swell: [f32; 2]) -> [[f32; 4]; 4] {
        [[self.center[0], self.center[1], EXTENT_M, self.plane / OCEAN_UNITS_PER_METER], [eye[0], eye[1], eye[2], time],
            [if fft { 1.0 } else { 0.0 }, count as f32, spacing, swell[0]], [jka_ninja::COAST_CLAMP_CM, jka_ninja::COAST_SIM_Z_CM, self.mesh_z_cm, swell[1]]]
    }
}

/// Fullscreen landscape wet-map receiver over the opaque scene (no depth
/// attachment; it reads the captured scene depth). Blend: rgb = src + dst * src.a.
fn wet_pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat, samples: u32) -> wgpu::RenderPipeline {
    let blend = wgpu::BlendState {
        color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::SrcAlpha,
            operation: wgpu::BlendOperation::Add },
        alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add },
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("Ninja coast wet-map receiver"), layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("ninja_wet_vertex"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some("ninja_wet_fragment"), compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })] }),
        primitive: Default::default(), depth_stencil: None,
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None })
}

/// r_ninjaCoastDeepWater split: `deep_fragment` (opaque, drawn first) or the Coast `fragment`
/// alpha-blended over it across the shore band. Otherwise `draw_pipeline`'s state.
pub(super) fn split_pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat, samples: u32, deep: bool) -> wgpu::RenderPipeline {
    let blend = (!deep).then_some(wgpu::BlendState::ALPHA_BLENDING);
    let fragment = if deep { "deep_fragment" } else { "fragment" };
    let label = if deep { "Ninja coast deep water" } else { "Ninja coast shore band" };
    coast_surface_pipeline(device, layout, shader, format, samples, fragment, label, blend)
}

pub(super) fn coast_surface_pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat, samples: u32, fragment: &str, label: &str, blend: Option<wgpu::BlendState>) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label), layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("vertex"), compilation_options: Default::default(),
            buffers: &[Some(blue_water_gpu::geometry::OceanVertex::LAYOUT)] },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })] }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, front_face: wgpu::FrontFace::Ccw,
            cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None })
}

pub(super) fn late_fft_pipeline(device: &wgpu::Device, groups: &[wgpu::BindGroupLayout; 5],
    (format, samples, steps, ssr): (wgpu::TextureFormat, u32, u32, bool)) -> Result<wgpu::RenderPipeline, String> {
    let material = jka_ninja::coast_material_wgsl().ok_or("Fluid Ninja private provider not included")?;
    let module = shader_composer::ninja_coast_late_fft_module(&material, jka_water::river::slw::SHADER, steps, ssr)?;
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Coast late FFT layout"), bind_group_layouts: &groups.each_ref().map(Some), immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Coast late FFT"), source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
    });
    let pipeline = split_pipeline(device, &layout, &shader, format, samples, false);
    let valid = checked_scope(validation);
    let enough_memory = checked_scope(memory);
    valid?;
    enough_memory?;
    Ok(pipeline)
}

pub(super) struct Renderer {
    pub pipeline: wgpu::RenderPipeline,
    /// r_ninjaCoastDeepWater: the deep-water pass (`deep_fragment`), drawn before `pipeline`,
    /// which then blends only the shore band over it.
    pub deep: Option<wgpu::RenderPipeline>,
    pub wave_clipmap: Option<Result<super::ninja_coast_wave_clipmap::Pipeline, String>>,
    pub adaptive: Option<Result<super::ninja_coast_adaptive::Pipeline, String>>,
    pub late_fft: Option<Result<wgpu::RenderPipeline, String>>,
    /// Landscape wet-map receiver (`ninja_wet_fragment`): out = add + dst * mul.
    pub wet: wgpu::RenderPipeline,
    pub wire: Option<wgpu::RenderPipeline>,
    shader: wgpu::ShaderModule, pipeline_layout: wgpu::PipelineLayout,
    pub(super) draw_layout: wgpu::BindGroupLayout,
    capture_layout: wgpu::BindGroupLayout,
    crest_pipeline: wgpu::ComputePipeline, contacts_pipeline: wgpu::ComputePipeline, empty: wgpu::BindGroup,
    height_layout: wgpu::BindGroupLayout, eye_layout: wgpu::BindGroupLayout, probe_layout: wgpu::BindGroupLayout,
    boundary_layout: wgpu::BindGroupLayout, eye_pipeline: wgpu::ComputePipeline,
    probe_eye_pipeline: wgpu::ComputePipeline, probe_pipeline: wgpu::ComputePipeline,
    clamp: wgpu::Sampler, repeat: wgpu::Sampler, gf: wgpu::TextureView, zero: wgpu::TextureView, no_caustics: wgpu::TextureView,
    assets: Option<Assets>, source: Option<Arc<Source>>, debug: u8, swell: [f32; 2], look: [f32; 4], lod: [[f32; 4]; 2], detail: [f32; 4], options: f32,
    /// 3D eye distance (native m) to each pool's nearest window water, from the last prepare.
    water_distance: [f32; OCEAN_MAX_SURFACES],
    /// `r_ninjaCoastSimDistance` (m, 0 = always simulate).
    sim_distance: f32,
    /// `r_ninjaCoastFarMeshLevels`: clipmap levels dropped while the sim is hidden.
    far_mesh_levels: u8,
    /// `r_ninjaCoastShoreDepth` (m, 0 = everywhere): sim detail and whitewater only over shallower bed.
    shore_depth: f32,
    /// `r_ninjaCoastOpenFoam`: Coast foam strength on the FFT crests over open sea (0 off).
    open_foam: f32,
    /// `r_ninjaCoastFollowSwell`: [on, wave-drive speed scale, wave-drive height scale].
    follow: [f32; 3],
    dump: super::ninja_coast_dump::Dumper,
    slots: [Option<Slot>; OCEAN_MAX_SURFACES], failed: [bool; OCEAN_MAX_SURFACES],
    unavailable: bool, history: ActorHistory, epoch: u64, last_time: Option<f32>, delta: f64,
    fft: bool, format: wgpu::TextureFormat, samples: u32,
    whitewater: Option<super::ninja_coast_whitewater::Shared>,
    /// r_ninjaCoastWhitewater spawn-rate scale.
    whitewater_scale: f32,
    /// r_ninjaCoastWhitewaterOpacity sprite alpha scale.
    whitewater_opacity: f32,
    /// r_ninjaCoastWaveGain: multiplier on the auto wave-drive gain.
    wave_gain_scale: f32,
    /// r_ninjaCoastSimQuality (see `sim_resolution`).
    sim_quality: u8,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, waves: &wgpu::BindGroupLayout, base: &OriginalSurfacePipeline,
        format: wgpu::TextureFormat, samples: u32, steps: u32, ssr: bool, fft: bool, open_sea: bool) -> Result<Self, String> {
        let material = jka_ninja::coast_material_wgsl().ok_or("Fluid Ninja private provider not included")?;
        let module = shader_composer::ninja_coast_module(&material, jka_water::river::slw::SHADER, steps, ssr, open_sea)?;
        let capture_module = shader_composer::ninja_coast_capture_module(&material)?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let both = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let compute = wgpu::ShaderStages::COMPUTE;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Ninja coast material inputs"),
            entries: &[uniform(0, both), texture(1, both), texture(2, both), texture(3, both), texture(4, both), texture(5, both),
                texture(6, both), texture(7, both), texture(8, both), sampler(9, both), sampler(10, both), texture(11, both), texture(12, both),
                texture(13, both), wgpu::BindGroupLayoutEntry { binding: 14, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3, multisampled: false }, count: None }] });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Ninja coast surface layout"),
            bind_group_layouts: &[Some(&base.camera_layout), Some(&base.surface_layout), Some(waves), Some(&base.environment_layout), Some(&draw_layout)],
            immediate_size: 0 });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Ninja coast surface"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)) });
        let (pipeline, deep) = if open_sea {
            (split_pipeline(device, &pipeline_layout, &shader, format, samples, false),
                Some(split_pipeline(device, &pipeline_layout, &shader, format, samples, true)))
        } else { (draw_pipeline(device, &pipeline_layout, &shader, format, samples, false), None) };
        let wet = wet_pipeline(device, &pipeline_layout, &shader, format, samples);
        let heights = [texture(4, compute), texture(5, compute), texture(6, compute), texture(7, compute), texture(8, compute),
            texture(9, compute), sampler(10, compute), sampler(11, compute)];
        let mut capture_entries = vec![uniform(0, compute), wgpu::BindGroupLayoutEntry { binding: 1, visibility: compute,
            ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float, view_dimension: wgpu::TextureViewDimension::D2 }, count: None },
            storage(2, true), storage(3, false)];
        capture_entries.extend_from_slice(&heights);
        let capture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Ninja coast crest/contact capture"),
            entries: &capture_entries });
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Ninja coast empty group"), entries: &[] });
        let empty = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ninja coast empty group"), layout: &empty_layout, entries: &[] });
        let capture_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Ninja coast capture"),
            bind_group_layouts: &[Some(&capture_layout), Some(&empty_layout), Some(waves)], immediate_size: 0 });
        let capture_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Ninja coast capture"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(capture_module)) });
        let make = |entry| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(entry), layout: Some(&capture_pl),
            module: &capture_shader, entry_point: Some(entry), compilation_options: Default::default(), cache: None });
        let crest_pipeline = make("crest");
        let contacts_pipeline = make("contacts");
        let mut height_entries = vec![uniform(0, compute)];
        height_entries.extend_from_slice(&heights);
        let height_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Ninja coast height inputs"),
            entries: &height_entries });
        let eye_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Ninja coast eye output"), entries: &[storage(0, false)] });
        let boundary_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Ninja coast probe boundary"), entries: &[uniform(16, compute)] });
        let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Ninja coast probe output"),
            entries: &[uniform(1, compute), wgpu::BindGroupLayoutEntry { binding: 2, visibility: compute,
                ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba16Float, view_dimension: wgpu::TextureViewDimension::D2 }, count: None },
                storage(3, false)] });
        let height_pipeline = |entry, middle: &wgpu::BindGroupLayout, output: &wgpu::BindGroupLayout| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(entry),
                bind_group_layouts: &[Some(&height_layout), Some(middle), Some(waves), Some(output)], immediate_size: 0 });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(entry), layout: Some(&layout),
                module: &capture_shader, entry_point: Some(entry), compilation_options: Default::default(), cache: None })
        };
        let eye_pipeline = height_pipeline("eye_height", &empty_layout, &eye_layout);
        let probe_eye_pipeline = height_pipeline("underwater_eye_height", &boundary_layout, &probe_layout);
        let probe_pipeline = height_pipeline("underwater_probe", &boundary_layout, &probe_layout);
        let clamp = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("Ninja coast clamp"), mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear, mipmap_filter: wgpu::MipmapFilterMode::Linear, ..Default::default() });
        // W repeats too: the caustics volume scrolls along W with time (UE wraps volume textures).
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("Ninja coast wrap"), address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat, address_mode_w: wgpu::AddressMode::Repeat, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear, ..Default::default() });
        let gf = jka_river::river_slw::PreintegratedGf::new(device, queue).texture.create_view(&Default::default());
        let zero = device.create_texture(&wgpu::TextureDescriptor { label: Some("Ninja coast initial unadvanced sim"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
        // Zero-initialized stand-in when the caustics volume asset is missing
        // (ColorScale keeps 1.1 + the divergence term).
        let no_caustics = device.create_texture(&wgpu::TextureDescriptor { label: Some("Ninja coast missing caustics volume"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D3, format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
        let v = checked_scope(validation); let m = checked_scope(memory); v?; m?;
        let whitewater = match super::ninja_coast_whitewater::Shared::new(device, queue, base, waves, &shader, format, samples) {
            Ok(w) => w,
            Err(e) => { eprintln!("[OCEAN NINJA] Coast whitewater unavailable: {e}"); None }
        };
        Ok(Self { pipeline, deep, wave_clipmap: None, adaptive: None, late_fft: None, wet, wire: None, shader, pipeline_layout, draw_layout, capture_layout, crest_pipeline, contacts_pipeline,
            empty, height_layout, eye_layout, probe_layout, boundary_layout, eye_pipeline, probe_eye_pipeline, probe_pipeline,
            clamp, repeat, gf, zero, no_caustics, assets: None, source: None, debug: 0, swell: [1.0, 0.0], look: [1.0, 1.0, 1.0, 1.0], lod: [[0.0; 4]; 2], detail: [1.5, 1.0, 0.0, 0.0], options: 0.0,
            water_distance: [0.0; OCEAN_MAX_SURFACES], sim_distance: 0.0, far_mesh_levels: 0, shore_depth: 0.0, open_foam: 0.0, follow: [0.0, 1.0, 1.0], dump: Default::default(), slots: std::array::from_fn(|_| None),
            failed: [false; OCEAN_MAX_SURFACES], unavailable: false, history: ActorHistory::default(), epoch: timeline_epoch(),
            last_time: None, delta: 0.0, fft, format, samples, whitewater, whitewater_scale: 0.3, whitewater_opacity: 0.2, wave_gain_scale: 1.0,
            sim_quality: 3 })
    }

    /// The map's BSP bed source. A new map drops fields built from the old one.
    /// `dump`: `r_ninjaCoastDump`; a change to a nonzero value reads the targets back once.
    /// `swell_deg`: `r_blueWaterDirection` (travel, degrees from +X toward render +Z); the Coast
    /// passive tile pattern and its pan are rotated to travel with it.
    /// `tint`/`transparency`: Hybrid water color (relative to its default) and transparency for
    /// the Coast water body (scattering tint, extinction divisor).
    pub fn set_terrain_source(&mut self, source: Option<&Arc<Source>>, debug: u8, dump: u32, swell_deg: f32,
        tint: [f32; 3], transparency: f32, coast: [f32; 12], lod: [[f32; 4]; 2], detail: [f32; 4]) {
        self.debug = debug;
        self.lod = lod;
        self.detail = detail;
        self.whitewater_scale = coast[0];
        self.whitewater_opacity = coast[1];
        self.wave_gain_scale = coast[2];
        self.options = coast[3];
        self.sim_distance = if coast[4].is_finite() { coast[4].max(0.0) } else { 0.0 };
        self.far_mesh_levels = if coast[5].is_finite() { coast[5].clamp(0.0, 3.0) as u8 } else { 0 };
        self.shore_depth = if coast[6].is_finite() { coast[6].max(0.0) } else { 0.0 };
        self.open_foam = if coast[7].is_finite() { coast[7].clamp(0.0, 10.0) } else { 0.0 };
        self.follow = [coast[8], if coast[9].is_finite() { coast[9] } else { 1.0 }, if coast[10].is_finite() { coast[10].max(0.0) } else { 1.0 }];
        // A change rebuilds each pool's field on its next prepare (the sim restarts).
        self.sim_quality = if coast[11].is_finite() { coast[11].clamp(0.0, 3.0) as u8 } else { 3 };
        let ok = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.look = [ok(tint[0], 0.0, 8.0, 1.0), ok(tint[1], 0.0, 8.0, 1.0), ok(tint[2], 0.0, 8.0, 1.0), ok(transparency, 0.1, 10.0, 1.0)];
        let a = if swell_deg.is_finite() { swell_deg.to_radians() } else { 0.0 };
        self.swell = [a.cos(), a.sin()];
        self.dump.set_token(dump);
        if self.source.as_ref().map(Arc::as_ptr) != source.map(Arc::as_ptr) {
            self.source = source.cloned();
            self.slots = std::array::from_fn(|_| None);
            self.failed = [false; OCEAN_MAX_SURFACES];
        }
    }

    pub fn begin(&mut self, device: &wgpu::Device, time: f32, samples: &[ActorSample], wire: bool,
        load: impl FnOnce() -> Result<Assets, String>) {
        if self.unavailable { return; }
        self.dump.poll();
        if self.assets.is_none() {
            match load() {
                Ok(a) => self.assets = Some(a),
                Err(e) => { eprintln!("[OCEAN NINJA] Coast unavailable: {e}; switch material to retry"); self.unavailable = true; return; }
            }
        }
        let epoch = timeline_epoch();
        let delta = self.last_time.map_or(1.0 / 60.0, |t| f64::from(time - t));
        if self.epoch != epoch || !delta.is_finite() || !(0.0..=0.5).contains(&delta) {
            self.slots = std::array::from_fn(|_| None); self.failed = [false; OCEAN_MAX_SURFACES];
            self.history.reset(); self.epoch = epoch; self.delta = 0.0;
        } else { self.delta = delta; }
        self.last_time = Some(time);
        self.history.update(samples.iter().copied());
        if wire && self.wire.is_none() && device.features().contains(wgpu::Features::POLYGON_MODE_LINE) {
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let p = draw_pipeline(device, &self.pipeline_layout, &self.shader, self.format, self.samples, true);
            if checked_scope(scope).is_ok() { self.wire = Some(p); }
        }
    }

    fn height_group(&self, device: &wgpu::Device, slot: &Slot, params: &wgpu::Buffer) -> Option<wgpu::BindGroup> {
        let a = self.assets.as_ref()?;
        let o = slot.field.outputs();
        let (vd, pd, painter) = (o.velocity_density.unwrap_or(&self.zero), o.pressure_divergence.unwrap_or(&self.zero), o.painter.unwrap_or(&self.zero));
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ninja coast height inputs"), layout: &self.height_layout,
            entries: &[b(0, params), t(4, vd), t(5, pd), t(6, painter), t(7, &slot.terrain_material_view), t(8, &a.material[2]),
                t(9, &a.material[3]), s(10, &self.clamp), s(11, &self.repeat)] }))
    }

    pub fn encode_eye(&self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, index: usize,
        waves: &wgpu::BindGroup, eye: [f32; 3], time: f32, spacing: f32, result: &wgpu::Buffer) -> bool {
        if self.epoch != timeline_epoch() { return false; }
        let Some(slot) = self.slots[index].as_ref() else { return false };
        let Some(group) = self.height_group(device, slot, &slot.eye_params) else { return false };
        queue.write_buffer(&slot.eye_params, 0, bytemuck::cast_slice(&slot.capture_uniform(
            [eye[0], eye[1] - slot.plane / OCEAN_UNITS_PER_METER, eye[2]], time, self.fft, 0, spacing, self.swell)));
        let output = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ninja coast eye result"), layout: &self.eye_layout, entries: &[b(0, result)] });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Ninja coast combined eye height"), timestamp_writes: None });
        pass.set_pipeline(&self.eye_pipeline); pass.set_bind_group(0, &group, &[]); pass.set_bind_group(1, &self.empty, &[]);
        pass.set_bind_group(2, waves, &[]); pass.set_bind_group(3, &output, &[]); pass.dispatch_workgroups(1, 1, 1);
        true
    }

    pub fn encode_probe(&self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, index: usize,
        waves: &wgpu::BindGroup, camera: &blue_water_gpu::original_surface::SurfaceCamera, spacing: f32, near: bool,
        boundary: &wgpu::Buffer, target: &wgpu::TextureView) {
        let Some(slot) = self.slots[index].as_ref() else { return };
        let Some(group) = self.height_group(device, slot, &slot.probe_params) else { return };
        let p = camera.position_time;
        queue.write_buffer(&slot.probe_params, 0, bytemuck::cast_slice(&slot.capture_uniform([p[0], p[1], p[2]], p[3], self.fft, 0, spacing, self.swell)));
        let mut params = [[0.0f32; 4]; 9];
        params[..4].copy_from_slice(&camera.inverse_projection);
        params[4..8].copy_from_slice(&camera.inverse_view);
        params[8] = [p[0], p[1], p[2], if near { camera.near } else { 0.0 }];
        queue.write_buffer(&slot.probe_camera, 0, bytemuck::cast_slice(&params));
        let bounds = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ninja coast water column"), layout: &self.boundary_layout, entries: &[b(16, boundary)] });
        let output = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ninja coast underwater probe"), layout: &self.probe_layout,
            entries: &[b(1, &slot.probe_camera), t(2, target), b(3, &slot.probe_eye)] });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Ninja coast combined waterline"), timestamp_writes: None });
        pass.set_bind_group(0, &group, &[]); pass.set_bind_group(1, &bounds, &[]); pass.set_bind_group(2, waves, &[]); pass.set_bind_group(3, &output, &[]);
        if near { pass.set_pipeline(&self.probe_eye_pipeline); pass.dispatch_workgroups(1, 1, 1); }
        pass.set_pipeline(&self.probe_pipeline); pass.dispatch_workgroups(16, 9, 1);
    }

    /// Whitewater sprite draw for pool `index`: pipeline, group 4, indirect arguments (offset
    /// `ninja_coast_whitewater::DRAW_ARGS`). Groups 0-3 are the Coast surface's.
    /// None while the eye is too far from the window's water for any sprite to show
    /// (`ninja_ww_shade`'s far fade reaches 0 at 85 m).
    pub fn whitewater(&self, index: usize) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup, &wgpu::Buffer)> {
        if self.water_distance[index] > WHITEWATER_HIDDEN_DISTANCE_M { return None; }
        if !self.slots[index].as_ref().is_some_and(|slot| self.window_has_shore(slot)) { return None; }
        let shared = self.whitewater.as_ref()?;
        let particles = self.slots[index].as_ref()?.whitewater.as_ref()?;
        Some((&shared.pipeline, particles.draw_group()?, &particles.args))
    }

    pub fn group(&self, index: usize) -> Option<&wgpu::BindGroup> {
        self.slots[index].as_ref()?.draw_group.as_ref().map(|(_, g)| g)
    }

    /// r_ninjaCoastShoreDepth: the window holds bed shallower than the shore depth (always with 0).
    fn window_has_shore(&self, slot: &Slot) -> bool {
        self.shore_depth <= 0.0 || slot.highest_bed >= -self.shore_depth * OCEAN_UNITS_PER_METER
    }

    /// The wet-map receiver can touch a pixel: it discards where the host sim
    /// share is 0, which is everywhere once all window water is past the share's eye fade.
    pub fn wet_visible(&self, index: usize) -> bool {
        self.water_distance[index] <= SIM_SHARE_HIDDEN_DISTANCE_M
            && self.slots[index].as_ref().is_some_and(|slot| sim_shaded(self.shore_depth, self.detail[0], slot.highest_bed))
    }

    /// Clipmap levels to drop for pool `index` (`r_ninjaCoastFarMeshLevels`): the 128-segment
    /// Coast mesh resolves the sim's swash near the eye; once the sim shades no water the
    /// finest rings only carry FFT geometry, which the coarser levels also draw.
    pub fn far_mesh_levels(&self, index: usize) -> u8 {
        if self.sim_displaces(index) { 0 } else { self.far_mesh_levels }
    }

    /// The sim's WPO can move pool `index`'s vertices: some of its water lies
    /// within the share's eye fade. Past it the surface carries only the FFT.
    pub fn sim_displaces(&self, index: usize) -> bool {
        self.water_distance[index] <= SIM_SHARE_HIDDEN_DISTANCE_M
    }

    pub fn eye_source(&self, index: usize) -> Option<&wgpu::Buffer> {
        if self.epoch != timeline_epoch() { return None; }
        let slot = self.slots.get(index)?.as_ref()?;
        slot.draw_group.as_ref()?;
        Some(&slot.eye_params)
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, index: usize,
        waves: &wgpu::BindGroup, centre: [f32; 2], plane: f32, eye: [f32; 3], time: f32, spacing: f32, mask: &super::OceanMasks) {
        if self.unavailable || self.failed[index] { return; }
        if let Err(e) = self.prepare_inner(device, queue, encoder, index, waves, centre, plane, eye, time, spacing, mask) {
            eprintln!("[OCEAN NINJA] Coast pool {index} unavailable: {e}; switch material to retry");
            self.slots[index] = None; self.failed[index] = true;
        }
    }

    fn prepare_inner(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, index: usize,
        waves: &wgpu::BindGroup, centre: [f32; 2], plane: f32, eye: [f32; 3], time: f32, spacing: f32, mask: &super::OceanMasks) -> Result<(), String> {
        let (Some(a), Some(source)) = (self.assets.as_ref(), self.source.as_ref()) else { return Ok(()) };
        // Sim attached to the camera (reference: "SIM ATTACHED TO PAWN, MOVING
        // RANGE: INFINITE"): a world-cell-aligned window that follows the
        // clipmap centre by whole capture cells, keeping its history.
        let (resolution, vd) = sim_resolution(a.authority.resolution, self.sim_quality);
        let n = resolution / 4;
        let cell = EXTENT_M * OCEAN_UNITS_PER_METER / n as f32;
        let jka = [centre[0] * OCEAN_UNITS_PER_METER, -centre[1] * OCEAN_UNITS_PER_METER];
        let desired = jka.map(|c| (c / cell).round() as i32 - n as i32 / 2);
        let current = self.slots[index].as_ref().filter(|s| s.plane == plane && s.size == n && s.vd == vd)
            .map(|s| [desired[0] - s.origin[0], desired[1] - s.origin[1]]);
        let placeholder = || CoastPatch { level: plane, heights: vec![f32::NAN; (n * n) as usize] };
        match current {
            Some(d) if d[0].abs().max(d[1].abs()) < FOLLOW_CELLS => {
                // Provisional window: swap in the real bed once its tiles exist.
                let slot = self.slots[index].as_mut().expect("coast slot");
                if slot.provisional {
                    if let Some(patch) = source.ninja_bed_window(index, plane, cell, n as usize, slot.origin)
                        .filter(|p| (p.level - plane).abs() <= 0.5) {
                        let origin = slot.origin;
                        slot.follow(queue, &patch, origin);
                        slot.provisional = false;
                    }
                }
            }
            Some(d) if d[0].abs().max(d[1].abs()) < n as i32 / 2 => {
                // Shift only on a frame that ticks, so bed and offset stay paired.
                let slot = self.slots[index].as_mut().expect("coast slot");
                if slot.field.wants_step(self.delta, FFT_FEEDS_SIM && self.fft)? {
                    match source.ninja_bed_window(index, plane, cell, n as usize, desired) {
                        Some(patch) => { slot.follow(queue, &patch, desired); slot.provisional = false; }
                        None if slot.provisional => slot.follow(queue, &placeholder(), desired),
                        None => {}
                    }
                }
            }
            _ => match source.ninja_bed_window(index, plane, cell, n as usize, desired) {
                Some(patch) if (patch.level - plane).abs() <= 0.5 => {
                    self.slots[index] = Some(Slot::new(device, queue, a, &patch, desired, cell, n, plane, resolution, vd)?);
                }
                Some(patch) => return Err(format!("bed plane {} does not match pool plane {plane}", patch.level)),
                // Not ready: keep simulating the previous window, or start a
                // provisional open-sea window so the Coast surface draws now.
                None if self.slots[index].as_ref().is_some_and(|s| s.plane == plane && s.size == n && s.vd == vd) => {}
                None => {
                    let mut slot = Slot::new(device, queue, a, &placeholder(), desired, cell, n, plane, resolution, vd)?;
                    slot.provisional = true;
                    self.slots[index] = Some(slot);
                }
            },
        }
        let slot = self.slots[index].as_mut().expect("initialized coast slot");
        slot.mesh_z_cm = mesh_z_cm(eye[1]);
        self.water_distance[index] = slot.nearest_water_m(eye);
        // r_ninjaCoastSimDistance: hold the solver and whitewater while all window water is that
        // far from the eye (the drawn sim share is already 0 past 45 m; only its window WPO freezes).
        // Also hold it while the window has no bed inside the shader's shore band
        // (`ninja_shore_weight`: 0 past the configured shore band or on unknown bed): the
        // sim share is 0 on every pixel there, so its state is never drawn (open sea).
        let paused = slot.draw_group.is_some()
            && (!sim_shaded(self.shore_depth, self.detail[0], slot.highest_bed) ||(self.sim_distance > 0.0 && self.water_distance[index] > self.sim_distance));
        let domain = [slot.center[0], slot.center[1], EXTENT_M, plane / OCEAN_UNITS_PER_METER];
        // r_ninjaCoastFollowSwell: the wave drive takes the Blue Water swell's direction
        // (shore exposure), wavelength (speed) and height (strength).
        let follow = self.follow[0] > 0.5;
        let height_scale = if follow { self.follow[2] } else { 1.0 };
        slot.field.set_landscape_normal_gain(slot.wave_gain * self.wave_gain_scale * height_scale);
        slot.field.set_host_waves([self.swell[0], -self.swell[1]], if follow { SWELL_EXPOSURE } else { 0.0 },
            if follow { self.follow[1] } else { 1.0 });
        let config = [if self.fft { 1.0 } else { 0.0 }, jka_ninja::COAST_CLAMP_CM, jka_ninja::COAST_SIM_Z_CM, f32::from(self.debug)];
        let mesh = [slot.mesh_z_cm, self.swell[0], self.swell[1], self.options];
        queue.write_buffer(&slot.params, 0, bytemuck::cast_slice(&[domain, config, mesh, self.look, self.lod[0], self.lod[1],
            [self.shore_depth * 100.0, self.open_foam, 0.0, 0.0], self.detail]));
        let due = !paused && slot.field.wants_step(self.delta, FFT_FEEDS_SIM && self.fft)?;
        let mut tracks = Vec::new();
        let mut points = Vec::new();
        if due {
            // Shared actor wake (river_ninja::wake_points): joints or hull centre,
            // each contact-tested against its own water column.
            // Relative to the patch centre (UE cm); z relative to the actor plane.
            let center = slot.center;
            let rel = |q: [f32; 3]| [q[0] * CM_PER_UNIT - center[0] * 100.0, q[1] * CM_PER_UNIT + center[1] * 100.0,
                (q[2] - plane) * CM_PER_UNIT + CLAMP_TO_ACTOR_CM];
            let mut wake = Vec::new();
            for tr in self.history.tracks() {
                let p = tr.current.origin_map;
                if !mask.contains(index, [p[0], -p[1]]) { continue; }
                wake.clear();
                wake_points(tr, &mut wake);
                for w in &wake {
                    let position = rel(w.position_map);
                    if position[0].abs().max(position[1].abs()) >= EXTENT_M * 50.0 { continue; }
                    tracks.push(w.track(rel));
                    let q = w.position_map;
                    points.push([q[0] / OCEAN_UNITS_PER_METER, -q[1] / OCEAN_UNITS_PER_METER,
                        (w.mins_map[2] - plane) / OCEAN_UNITS_PER_METER, (w.maxs_map[2] - plane) / OCEAN_UNITS_PER_METER]);
                }
            }
            let o = slot.field.outputs();
            let inputs = [o.velocity_density.unwrap_or(&self.zero).clone(), o.pressure_divergence.unwrap_or(&self.zero).clone(),
                o.painter.unwrap_or(&self.zero).clone()];
            if slot.capture_group.as_ref().is_none_or(|(v, _)| *v != inputs) {
                let g = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ninja coast capture inputs"), layout: &self.capture_layout,
                    entries: &[b(0, &slot.capture_params), t(1, &slot.crest_view), b(2, &slot.points), b(3, &slot.flags),
                        t(4, &inputs[0]), t(5, &inputs[1]), t(6, &inputs[2]), t(7, &slot.terrain_material_view), t(8, &a.material[2]),
                        t(9, &a.material[3]), s(10, &self.clamp), s(11, &self.repeat)] });
                slot.capture_group = Some((inputs, g));
            }
            queue.write_buffer(&slot.capture_params, 0, bytemuck::cast_slice(&slot.capture_uniform(eye, time, self.fft, tracks.len(), spacing.max(0.01), self.swell)));
            if !points.is_empty() { queue.write_buffer(&slot.points, 0, bytemuck::cast_slice(&points)); }
            let feed = FFT_FEEDS_SIM && self.fft;
            if feed || !points.is_empty() {
                let group = &slot.capture_group.as_ref().expect("capture group").1;
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Ninja coast FFT crest + actor contact"), timestamp_writes: None });
                if feed {
                    pass.set_pipeline(&self.crest_pipeline); pass.set_bind_group(0, group, &[]); pass.set_bind_group(1, &self.empty, &[]);
                    pass.set_bind_group(2, waves, &[]);
                    let [w, h] = slot.field.capture_resolution();
                    pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
                }
                if !points.is_empty() {
                    pass.set_pipeline(&self.contacts_pipeline); pass.set_bind_group(0, group, &[]); pass.set_bind_group(1, &self.empty, &[]);
                    pass.set_bind_group(2, waves, &[]); pass.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1);
                }
            }
        }
        if !paused {
            slot.field.encode(device, queue, encoder, &CoastFrame { game_delta: self.delta, tracks: &tracks,
                // The Coast window follows the player: wakes are relative to its motion (UE pawn-attached sim).
                frame_velocity_cm_s: jka_river::river_ninja::wake_owner_velocity_cm_s(self.history.tracks()),
                contacts: Some(&slot.flags), crest_source: (FFT_FEEDS_SIM && self.fft).then_some(&slot.crest) })?;
        }
        let mut whitewater_failed = false;
        let shore = self.shore_depth <= 0.0 || slot.highest_bed >= -self.shore_depth * OCEAN_UNITS_PER_METER;
        // A pool that has never emitted whitewater needs no particle state
        // at zero spawn rate. Existing particles still age when it is turned off.
        let simulate_whitewater = !paused && shore && (self.whitewater_scale > 0.0 || slot.whitewater.is_some());
        if let (true, Some(shared), Some(vd), Some(pd), Some(painter)) = (simulate_whitewater, self.whitewater.as_ref(), slot.field.outputs().velocity_density,
            slot.field.outputs().pressure_divergence, slot.field.outputs().painter) {
            if slot.whitewater.is_none() {
                match super::ninja_coast_whitewater::Particles::new(device, shared, jka_ninja::COAST_CLAMP_CM, queue) {
                    Ok(p) => slot.whitewater = Some(p),
                    Err(e) => { eprintln!("[OCEAN NINJA] Coast whitewater unavailable: {e}"); whitewater_failed = true; }
                }
            }
            if let Some(particles) = slot.whitewater.as_mut() {
                // TraceMeshPosDouble = window centre at the painter datum; the
                // component (Engine.Owner) rides with the pawn-attached system.
                let trace = [slot.center[0] * 100.0, -slot.center[1] * 100.0, jka_ninja::COAST_SIM_Z_CM];
                particles.encode(device, queue, encoder, shared, &self.clamp, &super::ninja_coast_whitewater::Tick {
                    delta: self.delta as f32, time, trace, trace_size_m: EXTENT_M,
                    owner: [trace[0], trace[1], slot.mesh_z_cm], clamp_cm: jka_ninja::COAST_CLAMP_CM, debug: f32::from(self.debug), spawn_scale: self.whitewater_scale, opacity: self.whitewater_opacity,
                    inputs: [vd, pd, painter, &slot.terrain_material_view], shore_cm: self.shore_depth * 100.0 });
            }
        }
        if whitewater_failed { self.whitewater = None; }
        let o = slot.field.outputs();
        if self.dump.wanted() && o.velocity_density.is_some() {
            let views: Vec<(&'static str, &wgpu::TextureView)> = [("RT_VelocityDensity", o.velocity_density),
                ("RT_PressureDivergence", o.pressure_divergence), ("RT_Painter", o.painter), ("FieldCapture", o.capture),
                ("BedMaterial", Some(&slot.terrain_material_view))].into_iter().filter_map(|(n, v)| Some((n, v?))).collect();
            let meta = serde_json::json!({ "pool": index, "plane_jka": plane, "origin_cells": slot.origin, "cell_jka": slot.cell,
                "capture_cells": slot.size, "centre_native_m": slot.center, "extent_m": EXTENT_M, "time": time, "delta": self.delta,
                "fft": self.fft, "eye_native_m": eye, "mesh_z_cm": slot.mesh_z_cm, "layout": "RGBA f32 LE, row-major, row = field V = JKA +Y" });
            self.dump.encode(device, encoder, &views, meta);
        }
        let inputs = [o.velocity_density.unwrap_or(&self.zero).clone(), o.pressure_divergence.unwrap_or(&self.zero).clone(),
            o.painter.unwrap_or(&self.zero).clone(), o.capture.unwrap_or(&self.zero).clone()];
        if slot.draw_group.as_ref().is_none_or(|(v, _)| *v != inputs) {
            let g = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ninja coast surface graph inputs"), layout: &self.draw_layout,
                entries: &[b(0, &slot.params), t(1, &inputs[0]), t(2, &inputs[1]), t(3, &inputs[2]), t(4, &slot.terrain_material_view),
                    t(5, &a.material[0]), t(6, &a.material[1]), t(7, &a.material[2]), t(8, &a.material[3]),
                    s(9, &self.clamp), s(10, &self.repeat), t(11, &self.gf), t(12, &inputs[3]), t(13, &a.simulation[1]),
                    t(14, a.caustics.as_ref().unwrap_or(&self.no_caustics))] });
            slot.draw_group = Some((inputs, g));
        }
        Ok(())
    }
}
