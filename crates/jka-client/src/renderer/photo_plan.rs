//! Panorama capture plan. World rendering, cube retention and output are
//! independent decisions, made once from the request, device limits and a
//! memory budget. Estimates are conservative byte counts, not measurements.
use super::{PhotoOptions, PhotoProjection};

/// Screen-space filter margin around an off-center world tile.
pub const GUARD: u32 = 128;
/// Disk page side (RGB8). Tile cores in the disk plan are page multiples.
pub const PAGE: u32 = 256;
/// Largest whole-face / tile world render (the long-shipped face bound).
const RENDER_MAX: u32 = 4096;
/// Smallest tile render; also the proven OOM-fallback size.
pub const FALLBACK_RENDER: u32 = 1024 + 2 * GUARD;
/// Full GPU output texture + readback is kept for outputs up to 8192x4096.
const DIRECT_OUTPUT_MAX_BYTES: u64 = 8192 * 4096 * 4;
/// Target bytes per GPU strip readback slot.
const STRIP_TARGET_BYTES: u64 = 32 << 20;
/// Readback slots in flight (Render Studio export uses the same depth).
pub const READBACK_SLOTS: u32 = 3;
/// Used when the OS reports no VRAM budget (non-Windows, integrated GPU).
pub const DEFAULT_GPU_BUDGET: u64 = 2 << 30;
const CACHE_MIN_BYTES: u64 = 256 << 20;
const CACHE_MAX_BYTES: u64 = 1536 << 20;

/// Estimated bytes per rendered pixel: per MSAA sample FP16 color + depth32,
/// plus full-resolution resolved/HDR/depth, post, ocean-optics, glass and
/// atmosphere targets. Calibrated against an RTX 3060 OOM at 3840 px
/// (2026-11-25); `[PHOTO PERF] vram_delta_mb` reports the measured value.
fn render_bytes(side: u32, msaa: u32) -> u64 {
    u64::from(side) * u64::from(side) * (u64::from(msaa.max(1)) * 12 + 256)
}
/// The capture renderer loads its own copy of the map (textures, lightmaps,
/// geometry, probes, bakes); counted once for every plan.
const SCENE_BYTES: u64 = 1 << 30;
fn cube_bytes(face: u32) -> u64 { 6 * u64::from(face) * u64::from(face) * 4 }
pub fn padded_row(width: u32) -> Option<u32> {
    width.checked_mul(4)?.checked_next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldPlan {
    /// Six whole faces into a GPU cube texture.
    Faces,
    /// Off-center world tiles copied into a GPU cube texture.
    GpuTiles,
    /// Off-center world tiles read back into a bounded disk page store.
    DiskTiles,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputPlan {
    /// One output texture per image, existing screenshot readback/encoder.
    Direct,
    /// GPU projection into bounded row strips, async readback, streaming encode.
    Strips,
    /// Parallel CPU projection from the disk page store, streaming encode.
    Cpu,
}

/// Developer A/B override (`JKA_PHOTO_PLAN`); never a user requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForcePlan { Direct, Strips, GpuTiles, DiskTiles }
impl ForcePlan {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "direct" => Some(Self::Direct), "strips" => Some(Self::Strips),
            "gputiles" | "gpu" => Some(Self::GpuTiles), "disktiles" | "disk" => Some(Self::DiskTiles),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanInputs {
    pub max_texture: u32,
    pub max_buffer: u64,
    /// Capture MSAA sample count used for the memory estimate.
    pub msaa: u32,
    /// Extra GPU memory the capture may use.
    pub gpu_budget: u64,
    /// True when `gpu_budget` came from the OS (DXGI) rather than the default.
    pub measured: bool,
    pub force: Option<ForcePlan>,
    /// Developer override of the tile render side (tests tile seams cheaply).
    pub tile_render: Option<u32>,
}
impl PlanInputs {
    /// Menu estimate before the device is consulted (typical discrete GPU
    /// headroom). The capture itself uses device limits and the DXGI budget.
    pub fn nominal(options: PhotoOptions) -> Self {
        Self { max_texture: 16384, max_buffer: 1 << 30, msaa: if options.max_msaa { 8 } else { 4 },
            gpu_budget: 2 * DEFAULT_GPU_BUDGET, measured: false, force: None, tile_render: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StripPlan {
    pub rows: u32,
    /// Width of one horizontal GPU segment (texture limit).
    pub segment: u32,
    pub segments: u32,
    pub padded_row: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturePlan {
    pub world: WorldPlan,
    pub output: OutputPlan,
    pub image: [u32; 2],
    /// Virtual cube face side, including supersampling.
    pub face: u32,
    /// Offscreen world render target side.
    pub render: u32,
    pub guard: u32,
    /// Face pixels covered by one tile (whole face: `face`).
    pub core: u32,
    pub tiles_per_side: u32,
    pub strip: Option<StripPlan>,
    pub gpu_bytes: u64,
    pub ram_bytes: u64,
    pub disk_bytes: u64,
    /// Page cache size for the CPU projector (disk plan only).
    pub cache_bytes: u64,
    pub measured: bool,
}

impl CapturePlan {
    pub fn world_renders(&self) -> u32 { 6 * self.tiles_per_side * self.tiles_per_side }
    pub fn label(&self) -> &'static str {
        match (self.world, self.output) {
            (WorldPlan::Faces, OutputPlan::Direct) => "direct GPU",
            (WorldPlan::Faces, _) => "whole faces + GPU strips",
            (WorldPlan::GpuTiles, OutputPlan::Direct) => "tiled world + GPU cube",
            (WorldPlan::GpuTiles, _) => "tiled world + GPU strips",
            (WorldPlan::DiskTiles, _) => "tiled world + disk pages",
        }
    }
    pub fn describe(&self) -> String {
        let gb = |v: u64| v as f64 / f64::from(1u32 << 30);
        let mut text = format!("{}: {} world render{} of {} px, ~{:.1} GB GPU{}",
            self.label(), self.world_renders(), if self.world_renders() == 1 { "" } else { "s" },
            self.render, gb(self.gpu_bytes), if self.measured { "" } else { " (estimate)" });
        if self.ram_bytes >= 64 << 20 { text += &format!(", ~{:.1} GB RAM", gb(self.ram_bytes)); }
        if self.disk_bytes > 0 { text += &format!(", ~{:.1} GB temporary disk", gb(self.disk_bytes)); }
        text
    }
    pub fn page_tiles(&self) -> u32 { self.core / PAGE }

    /// Smallest proven plan, retried with a fresh renderer after OOM: disk
    /// pages for equirectangular (no cube in VRAM), else GPU-cube tiles.
    /// None when this plan already is that fallback.
    pub fn oom_fallback(&self, options: PhotoOptions, inputs: PlanInputs) -> Option<Self> {
        let force = if options.projection == PhotoProjection::Equirectangular { ForcePlan::DiskTiles } else { ForcePlan::GpuTiles };
        let inputs = PlanInputs { force: Some(force), tile_render: Some(FALLBACK_RENDER), ..inputs };
        plan(options, inputs).ok().filter(|p| p.world != self.world || p.render < self.render)
    }
}

fn strip_plan(image: [u32; 2], inputs: PlanInputs) -> Option<StripPlan> {
    let padded = padded_row(image[0])?;
    let segment = image[0].min(inputs.max_texture);
    let mut rows = (STRIP_TARGET_BYTES / u64::from(padded)).clamp(8, u64::from(inputs.max_texture)) as u32;
    rows = rows.min(image[1]).min((inputs.max_buffer / u64::from(padded)).min(u64::from(u32::MAX)) as u32);
    (rows > 0 && segment > 0).then_some(StripPlan { rows, segment, segments: image[0].div_ceil(segment), padded_row: padded })
}

/// Hard limits always apply. `soft` also keeps equirectangular output above
/// 8192x4096 off the full-image texture/readback/RAM path; other projections
/// keep their previous direct behavior wherever the device limits allow it.
fn direct_fits(image: [u32; 2], inputs: PlanInputs, soft: bool) -> bool {
    let Some(padded) = padded_row(image[0]) else { return false; };
    let bytes = u64::from(padded) * u64::from(image[1]);
    image.into_iter().all(|v| v > 0 && v <= inputs.max_texture) && bytes <= inputs.max_buffer
        && (!soft || bytes <= DIRECT_OUTPUT_MAX_BYTES)
}

/// Render sides from `cap` down to the fallback tile, in 64 px steps.
fn render_sides(cap: u32) -> impl Iterator<Item = u32> {
    let steps = cap.saturating_sub(FALLBACK_RENDER) / 64;
    (0..=steps).map(move |i| cap - i * 64)
}

/// Choose a capture plan. Returns a user-facing error when no plan satisfies
/// the dimensional limits (memory estimates only steer the choice; the disk
/// plan is always allowed as the last resort for equirectangular output).
pub fn plan(options: PhotoOptions, inputs: PlanInputs) -> Result<CapturePlan, String> {
    if !options.panorama { return Err("Panorama plan requested for a standard screenshot".into()); }
    let image = options.output_size([1, 1]);
    let face = options.render_size([1, 1]).ok_or("Screenshot render dimensions overflow; reduce supersampling")?[0];
    if image.into_iter().any(|v| v == 0) || face == 0 { return Err("Invalid screenshot dimensions".into()); }
    let equirect = options.projection == PhotoProjection::Equirectangular;
    let faces_export = options.projection == PhotoProjection::Faces;
    let cap = RENDER_MAX.min(inputs.max_texture);
    let make = |world: WorldPlan, render: u32, output: OutputPlan| -> Option<CapturePlan> {
        let (guard, core, n) = match world {
            WorldPlan::Faces => (0, face, 1),
            WorldPlan::GpuTiles => {
                let span = render.checked_sub(2 * GUARD).filter(|v| *v >= 64)?;
                let n = face.div_ceil(span);
                (GUARD, face.div_ceil(n), n)
            }
            WorldPlan::DiskTiles => {
                let pages = render.checked_sub(2 * GUARD)? / PAGE;
                if pages == 0 { return None; }
                let n = face.div_ceil(pages * PAGE);
                let core = face.div_ceil(PAGE).div_ceil(n) * PAGE;
                (GUARD, core, n)
            }
        };
        let render = if world == WorldPlan::Faces { face } else { core + 2 * guard };
        if render == 0 || render > inputs.max_texture { return None; }
        let strip = if output == OutputPlan::Direct { None } else { Some(strip_plan(image, inputs)?) };
        if output == OutputPlan::Direct && !direct_fits(image, inputs, false) { return None; }
        if world != WorldPlan::DiskTiles && face > inputs.max_texture { return None; }
        let rgba = |w: u32, h: u32| u64::from(padded_row(w).unwrap_or(u32::MAX)) * u64::from(h);
        let mut gpu = SCENE_BYTES + render_bytes(render, inputs.msaa);
        let mut ram = 0u64;
        let (mut disk, mut cache) = (0u64, 0u64);
        if world != WorldPlan::DiskTiles { gpu += cube_bytes(face); }
        match output {
            OutputPlan::Direct => { gpu += 2 * rgba(image[0], image[1]); ram += 3 * rgba(image[0], image[1]); }
            OutputPlan::Strips | OutputPlan::Cpu => {
                if let Some(s) = strip {
                    gpu += rgba(s.segment, s.rows) + u64::from(READBACK_SLOTS) * u64::from(s.padded_row) * u64::from(s.rows);
                    ram += (u64::from(READBACK_SLOTS) + 1) * u64::from(image[0]) * 3 * u64::from(s.rows);
                }
            }
        }
        if world == WorldPlan::DiskTiles {
            let k = u64::from(core / PAGE);
            disk = 6 * u64::from(n) * u64::from(n) * k * k * u64::from(PAGE) * u64::from(PAGE) * 3;
            gpu += u64::from(READBACK_SLOTS) * rgba(core, core);
            // Working set of one output band: the latitude ring of pages.
            cache = (16 * u64::from(face) * u64::from(PAGE) * 3).clamp(CACHE_MIN_BYTES, CACHE_MAX_BYTES);
            ram += cache + u64::from(core) * u64::from(core) * 3;
        }
        Some(CapturePlan { world, output, image, face, render, guard, core, tiles_per_side: n, strip,
            gpu_bytes: gpu, ram_bytes: ram, disk_bytes: disk, cache_bytes: cache, measured: inputs.measured })
    };
    let output = if direct_fits(image, inputs, equirect) { OutputPlan::Direct } else { OutputPlan::Strips };
    if output != OutputPlan::Direct && faces_export { return Err("Cubemap face images exceed GPU texture limits; reduce resolution".into()); }
    let tile_render = inputs.tile_render.map(|v| v.clamp(2 * GUARD + 64, cap.max(2 * GUARD + 64)));
    if let Some(force) = inputs.force {
        let forced = match force {
            ForcePlan::Direct => make(WorldPlan::Faces, face, OutputPlan::Direct),
            ForcePlan::Strips if !faces_export => make(WorldPlan::Faces, face, OutputPlan::Strips),
            ForcePlan::GpuTiles => make(WorldPlan::GpuTiles, tile_render.unwrap_or(cap), output),
            ForcePlan::DiskTiles if equirect => make(WorldPlan::DiskTiles, tile_render.unwrap_or(cap), OutputPlan::Cpu),
            _ => None,
        };
        return forced.ok_or_else(|| format!("Forced screenshot plan {force:?} is not possible for this capture"));
    }
    let fits = |p: &CapturePlan| p.gpu_bytes <= inputs.gpu_budget;
    // 1. Six whole faces: fewest world submissions, hardware reprojection.
    if face <= cap && tile_render.is_none() {
        if let Some(p) = make(WorldPlan::Faces, face, output).filter(fits) { return Ok(p); }
    }
    // 2. Tiled world into a GPU-resident cube: largest tile the budget allows.
    let gpu_tiles = match tile_render {
        Some(side) => make(WorldPlan::GpuTiles, side, output),
        None => render_sides(cap).find_map(|side| make(WorldPlan::GpuTiles, side, output).filter(fits)),
    };
    // 3. Bounded disk pages + parallel CPU projection (equirectangular only).
    // Always available as the last resort; OOM is still reported if it fails.
    let disk = equirect.then(|| {
        let side = tile_render.unwrap_or_else(|| render_sides(cap)
            .find(|side| make(WorldPlan::DiskTiles, *side, OutputPlan::Cpu).is_some_and(|p| fits(&p)))
            .unwrap_or(FALLBACK_RENDER));
        make(WorldPlan::DiskTiles, side, OutputPlan::Cpu)
    }).flatten();
    match (gpu_tiles, disk) {
        // A large cube can leave only small tiles: many more world renders
        // cost more than CPU projection from bigger disk tiles.
        (Some(gpu), Some(disk)) if gpu.world_renders() > 2 * disk.world_renders() => Ok(disk),
        (Some(gpu), _) => Ok(gpu),
        (None, Some(disk)) => Ok(disk),
        (None, None) if equirect => Err("Screenshot exceeds GPU limits; reduce resolution or supersampling".into()),
        (None, None) => Err("This projection needs the whole cube in GPU memory; reduce resolution or supersampling".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::ScreenshotFormat;
    fn equi(width: u32, ss: u32) -> PhotoOptions {
        PhotoOptions { panorama: true, panorama_width: width, supersample: ss, ..Default::default() }
    }
    fn inputs(budget: u64) -> PlanInputs {
        PlanInputs { max_texture: 16384, max_buffer: 1 << 30, msaa: 8, gpu_budget: budget, measured: true, force: None, tile_render: None }
    }
    #[test]
    fn moderate_sizes_render_six_whole_faces() {
        let p = plan(equi(8192, 1), inputs(4 << 30)).unwrap();
        assert_eq!((p.world, p.output, p.world_renders()), (WorldPlan::Faces, OutputPlan::Direct, 6));
        let p = plan(equi(12288, 1), inputs(8 << 30)).unwrap();
        assert_eq!((p.world, p.output, p.world_renders(), p.face), (WorldPlan::Faces, OutputPlan::Strips, 6, 3912));
        // Less headroom: tiles into the GPU cube, never the disk store.
        let p = plan(equi(12288, 1), inputs(4 << 30)).unwrap();
        assert_eq!((p.world, p.output), (WorldPlan::GpuTiles, OutputPlan::Strips));
        assert_eq!(p.disk_bytes, 0);
    }
    #[test]
    fn large_faces_tile_into_gpu_cube_then_disk() {
        let p = plan(equi(12288, 2), inputs(8 << 30)).unwrap();
        assert_eq!(p.world, WorldPlan::GpuTiles);
        assert!(p.world_renders() < 96 && p.core * p.tiles_per_side >= p.face && p.render <= 4096);
        let p = plan(equi(131072, 1), inputs(4 << 30)).unwrap();
        assert_eq!((p.world, p.output), (WorldPlan::DiskTiles, OutputPlan::Cpu));
        assert_eq!(p.core % PAGE, 0);
        assert!(p.core * p.tiles_per_side >= p.face);
        assert!(p.disk_bytes > u64::from(u32::MAX));
        let s = p.strip.unwrap();
        assert!(s.segment <= 16384 && s.segments * s.segment >= 131072);
    }
    #[test]
    fn small_budget_or_limits_fall_back() {
        let p = plan(equi(12288, 1), inputs(256 << 20)).unwrap();
        assert_eq!(p.world, WorldPlan::DiskTiles);
        let mut limited = inputs(4 << 30);
        limited.max_texture = 8192;
        let p = plan(equi(16384, 1), limited).unwrap();
        assert_eq!(p.output, OutputPlan::Strips);
        assert_eq!(p.strip.unwrap().segments, 2);
        let mut cross = equi(16384, 4);
        cross.projection = PhotoProjection::Cross;
        assert!(plan(cross, inputs(1 << 30)).is_err());
    }
    #[test]
    fn forced_plans_and_tiny_tiles() {
        let mut i = inputs(4 << 30);
        i.force = Some(ForcePlan::Strips);
        assert_eq!(plan(equi(8192, 1), i).unwrap().output, OutputPlan::Strips);
        i.force = Some(ForcePlan::DiskTiles);
        i.tile_render = Some(2 * GUARD + PAGE);
        let p = plan(equi(8192, 1), i).unwrap();
        assert_eq!((p.core, p.render), (PAGE, PAGE + 2 * GUARD));
        assert_eq!(p.tiles_per_side, 2608u32.div_ceil(PAGE));
        let mut faces = equi(8192, 1);
        faces.projection = PhotoProjection::Faces;
        i.force = Some(ForcePlan::DiskTiles);
        assert!(plan(faces, i).is_err());
        let p = plan(equi(8192, 1), inputs(4 << 30)).unwrap();
        let fallback = p.oom_fallback(equi(8192, 1), inputs(4 << 30)).unwrap();
        assert_eq!((fallback.world, fallback.render), (WorldPlan::DiskTiles, FALLBACK_RENDER));
        assert!(fallback.oom_fallback(equi(8192, 1), inputs(4 << 30)).is_none());
    }
    #[test]
    fn jpeg_limit_is_independent_of_plan() {
        let mut options = equi(PhotoOptions::MAX_JPEG_PANORAMA_WIDTH, 1);
        options.format = ScreenshotFormat::Jpeg;
        assert!(plan(options, inputs(4 << 30)).is_ok());
        assert!(options.output_size([1,1]).into_iter().all(|v| v <= PhotoOptions::JPEG_MAX_DIMENSION));
        options.panorama_width = 65536;
        assert!(options.output_size([1,1]).into_iter().any(|v| v > PhotoOptions::JPEG_MAX_DIMENSION));
        assert!(padded_row(u32::MAX).is_none());
    }
}
