//! Asynchronous static geometry plus the opt-in Phase 2.5 debug overlay.
//! Also supplies immutable geometry to the opt-in coastal simulation.
use crate::scene::{DrawBatch, GpuVertex};
use jka_assets::bsp::{
    bathymetry::{self, Atlas, Boundary, Class, Geometry, Grid, Options, Region},
    Bsp, DebugVolumeMesh,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

// Promotion's authored-image heuristic spans wavelengths 256..768u. Keep
// bathymetry for its entire shallow-depth range, independent of pool selection.
pub const SHORELINE_SCREENING_DEPTH: f32 = 768.0 * 0.05;

pub enum Status {
    Pending,
    Ready {
        atlas: Arc<Atlas>,
        cache_hit: bool,
        total_ms: f64,
        report: PathBuf,
        report_error: Option<String>,
    },
    Failed(String),
}

pub struct Job {
    pub upstream_shore: Arc<super::upstream_coast::Source>,
    pub status: Mutex<Status>,
    /// CPU selection/descriptor work on the loader; geometry and IO are excluded.
    pub selection_ms: f64,
    /// Exact promoted footprints survive even when a grid is rejected, so the
    /// debug view never substitutes the enclosing rectangle.
    footprints: Vec<(u32, f32, Vec<bathymetry::FootprintTriangle>)>,
}

/// This runs on the map loader, uses the exact uploader's faces/clustering/cap,
/// then hands all geometry work and cache IO to a dedicated background thread.
pub fn start(
    bsp: Arc<Bsp>,
    vertices: &[GpuVertex],
    coarse: &[DrawBatch],
    full: &[DrawBatch],
    map_hash: u64,
    base: &Path,
    game: Option<&Path>,
) -> Option<Arc<Job>> {
    let selection_started = Instant::now();
    let entries = super::surfaces::collect(vertices, coarse, full);
    let faces: Vec<_> = entries.iter().map(|e| e.face).collect();
    let triangle_sets: Vec<_> = entries.iter().map(|e| e.triangles.clone()).collect();
    let footprints =
        super::build_water_footprints(&faces, &triangle_sets, super::WATER_FOOTPRINT_TILE_SIZE);
    let options = Options::default();
    let mut regions = Vec::new();
    let mut debug_footprints = Vec::new();
    let mut warnings = Vec::new();
    for (slot, footprint) in footprints
        .iter()
        .take(super::OCEAN_MAX_SURFACES)
        .enumerate()
    {
        if faces[footprint.members[0]].authored_ocean.is_some() {
            continue;
        }
        let exact: Vec<bathymetry::FootprintTriangle> = footprint
            .triangles
            .iter()
            .map(|t| t.map(|[x, z]| [x, -z]))
            .collect();
        debug_footprints.push((slot as u32, footprint.plane, exact.clone()));
        let planar = footprint.members.iter().all(|&i| {
            let s = entries[i].source;
            vertices[s.vertices.start as usize..s.vertices.end as usize]
                .iter()
                .all(|v| (v.position[1] - footprint.plane).abs() < 0.01)
        });
        if !planar {
            warnings.push(format!(
                "slot {slot}: nonplanar promoted surface; retain legacy water"
            ));
            continue;
        }
        for tile in &footprint.tiles {
            let triangles = tile
                .triangles
                .iter()
                .filter_map(|&index| exact.get(index as usize).copied())
                .collect();
            regions.push(Region {
                slot: slot as u32,
                level: footprint.plane,
                triangles,
                bounds: Some([
                    [tile.minimum[0], -tile.maximum[1]],
                    [tile.maximum[0], -tile.minimum[1]],
                ]),
            });
        }
    }
    if regions.is_empty() && warnings.is_empty() {
        return None;
    }
    regions.sort_by(|a, b| {
        a.slot.cmp(&b.slot).then_with(|| {
            let aa = a.bounds.map_or([0.0; 2], |v| v[0]);
            let bb = b.bounds.map_or([0.0; 2], |v| v[0]);
            aa[0].total_cmp(&bb[0]).then(aa[1].total_cmp(&bb[1]))
        })
    });
    let mut descriptor = Vec::new();
    for v in [
        options.cell_size.to_bits(),
        options.padding_cells,
        options.max_cells_per_region as u32,
        SHORELINE_SCREENING_DEPTH.to_bits(),
    ] {
        descriptor.extend(v.to_le_bytes());
    }
    // Rejections also form part of identity. A cache from a previously eligible
    // shader/footprint must not survive a changed runtime material definition.
    for warning in &warnings {
        descriptor.extend(warning.as_bytes());
        descriptor.push(0);
    }
    for r in &regions {
        descriptor.extend(r.slot.to_le_bytes());
        descriptor.extend(r.level.to_le_bytes());
        descriptor.extend((r.triangles.len() as u32).to_le_bytes());
        for v in r.triangles.iter().flatten().flatten() {
            descriptor.extend(v.to_le_bytes());
        }
        if let Some(bounds) = r.bounds {
            for value in bounds.into_iter().flatten() {
                descriptor.extend(value.to_le_bytes());
            }
        }
    }
    let key = [map_hash, bathymetry::fingerprint(&descriptor)];
    let directory = game.unwrap_or(base).join("cache/coastal").join(format!(
        "v{}-{:016x}-{:016x}",
        bathymetry::CACHE_VERSION,
        key[0],
        key[1]
    ));
    let job = Arc::new(Job {
        upstream_shore: super::upstream_coast::Source::new(Arc::clone(&bsp),debug_footprints.clone()),
        status: Mutex::new(Status::Pending),
        selection_ms: selection_started.elapsed().as_secs_f64() * 1000.0,
        footprints: debug_footprints,
    });
    let output = Arc::clone(&job);
    let spawned = std::thread::Builder::new()
        .name("jka-coastal-bake".into())
        .spawn(move || {
            let started = Instant::now();
            let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<Status, String> {
                    let file = directory.join("bathymetry.bin");
                    let cached = std::fs::metadata(&file)
                        .ok()
                        .filter(|m| m.len() <= 128 * 1024 * 1024)
                        .and_then(|_| std::fs::read(&file).ok())
                        .and_then(|b| Atlas::decode(&b, key).ok());
                    let cache_hit = cached.is_some();
                    let mut atlas = if let Some(atlas) = cached {
                        atlas
                    } else {
                        let geometry = Geometry::from_bsp(&bsp)?;
                        let mut atlas = geometry.bake_shoreline_bands(
                            &regions,
                            options,
                            SHORELINE_SCREENING_DEPTH,
                        )?;
                        atlas.warnings.extend(warnings);
                        atlas.bake_ms = started.elapsed().as_secs_f64() * 1000.0;
                        atlas
                    };
                    let mut report_error = None;
                    if !cache_hit {
                        let save = (|| -> std::io::Result<()> {
                            std::fs::create_dir_all(&directory)?;
                            let temporary =
                                directory.join(format!("bathymetry-{}.tmp", std::process::id()));
                            std::fs::write(&temporary, atlas.encode(key))?;
                            std::fs::rename(&temporary, &file)
                        })();
                        if let Err(e) = save {
                            atlas.warnings.push(format!("cache write failed: {e}"));
                        }
                    }
                    if let Err(e) = atlas.write_debug(&directory) {
                        report_error = Some(e);
                    }
                    let total_ms = started.elapsed().as_secs_f64() * 1000.0;
                    eprintln!(
                        "COASTAL GEOMETRY: {} grids; {} bytes; {:.2} ms {}; report {}",
                        atlas.grids.len(),
                        atlas.memory_bytes(),
                        total_ms,
                        if cache_hit {
                            "cache hit"
                        } else {
                            "background bake"
                        },
                        directory.join("bathymetry.html").display()
                    );
                    Ok(Status::Ready {
                        atlas: Arc::new(atlas),
                        cache_hit,
                        total_ms,
                        report: directory.join("bathymetry.html"),
                        report_error,
                    })
                },
            ));
            let status = match work {
                Ok(Ok(status)) => status,
                Ok(Err(e)) => Status::Failed(e),
                Err(_) => Status::Failed("bathymetry bake panicked; legacy water retained".into()),
            };
            if let Ok(mut output) = output.status.lock() {
                *output = status;
            }
        });
    if let Err(e) = spawned {
        if let Ok(mut status) = job.status.lock() {
            *status = Status::Failed(format!("could not start coastal worker: {e}"));
        }
    }
    Some(job)
}

impl Job {
    /// Changes once when the worker publishes its terminal result. The app uses
    /// this to upload a debug mesh once, rather than rebuilding it every tick.
    pub fn debug_status_tag(&self) -> u8 {
        match self.status.lock().as_deref() {
            Ok(Status::Pending) => 0,
            Ok(Status::Ready { .. }) => 1,
            Ok(Status::Failed(_)) | Err(_) => 2,
        }
    }

    /// Build the requested Phase 2.5 overlay and one assessment line for every
    /// promoted ocean, including rejected/missing grids.
    pub fn debug_view(
        &self,
        mode: u8,
        promoted: &[super::appearance::PromotedOcean],
    ) -> (Option<DebugVolumeMesh>, Vec<String>) {
        let Ok(status) = self.status.lock() else {
            return (None, vec!["Coastal geometry status is unavailable.".into()]);
        };
        match &*status {
            Status::Pending => (
                None,
                vec![
                    "Coastal geometry bake is still running; all promoted oceans are pending."
                        .into(),
                ],
            ),
            Status::Failed(error) => (
                None,
                promoted
                    .iter()
                    .map(|o| format!("{}: UNSUPPORTED - {error}", o.label()))
                    .collect(),
            ),
            Status::Ready { atlas, .. } => {
                build_debug_view(atlas, mode, promoted, &self.footprints)
            }
        }
    }

    pub fn report_lines(&self) -> Vec<String> {
        let Ok(status) = self.status.try_lock() else {
            return vec!["Coastal geometry worker is publishing its result.".into()];
        };
        match &*status {
            Status::Pending => vec!["Coastal geometry bake is running in the background.".into()],
            Status::Failed(e) => vec![format!("Coastal geometry unavailable: {e}")],
            Status::Ready {
                atlas,
                cache_hit,
                total_ms,
                report,
                report_error,
            } => {
                let mut lines = vec![format!(
                    "Coastal geometry: {} grids; {:.3} MiB; {:.2} ms {}; selection {:.3} ms; report {}",
                    atlas.grids.len(),
                    atlas.memory_bytes() as f64 / 1048576.0,
                    total_ms,
                    if *cache_hit {
                        "cache hit"
                    } else {
                        "background bake"
                    },
                    self.selection_ms,
                    report.display()
                )];
                lines.extend(atlas.summary().lines().map(str::to_owned));
                if let Some(e) = report_error {
                    lines.push(format!("Report write failed: {e}"));
                }
                lines
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct BoundaryCounts {
    bank: usize,
    wall: usize,
    outline: usize,
    unsupported: usize,
}

#[derive(Clone, Copy, Debug)]
enum Treatment {
    WetDryBank,
    RigidWall,
    Mixed,
    FftOnly,
    Unsupported,
}

impl Treatment {
    fn label(self) -> &'static str {
        match self {
            Self::WetDryBank => "WET/DRY BANK - shoreline candidate",
            Self::RigidWall => "RIGID WALL - wall attenuation only",
            Self::Mixed => "MIXED - shoreline + wall attenuation",
            Self::FftOnly => "FFT-ONLY - no movable bank",
            Self::Unsupported => "UNSUPPORTED - not solver-safe",
        }
    }

    fn color(self) -> [u8; 4] {
        match self {
            Self::WetDryBank => [54, 225, 92, 104],
            Self::RigidWall => [55, 132, 255, 104],
            Self::Mixed => [255, 205, 42, 112],
            Self::FftOnly => [145, 154, 164, 88],
            Self::Unsupported => [255, 52, 220, 116],
        }
    }
}

fn face_counts(grid: &Grid) -> BoundaryCounts {
    let (nx, ny) = (grid.size[0] as usize, grid.size[1] as usize);
    let wet = |x: usize, y: usize| grid.cells[y * nx + x].class == Class::Wet;
    let mut result = BoundaryCounts::default();
    let mut add = |boundary| match boundary {
        Boundary::Bank => result.bank += 1,
        Boundary::Wall => result.wall += 1,
        Boundary::Outline => result.outline += 1,
        Boundary::Unsupported => result.unsupported += 1,
        Boundary::Open => {}
    };
    for y in 0..ny {
        for x in 0..=nx {
            if (x > 0 && wet(x - 1, y)) || (x < nx && wet(x, y)) {
                add(grid.faces_x[y * (nx + 1) + x].boundary);
            }
        }
    }
    for y in 0..=ny {
        for x in 0..nx {
            if (y > 0 && wet(x, y - 1)) || (y < ny && wet(x, y)) {
                add(grid.faces_y[y * nx + x].boundary);
            }
        }
    }
    result
}

fn treatment(has_wet: bool, counts: BoundaryCounts) -> Treatment {
    if !has_wet {
        Treatment::Unsupported
    } else if counts.bank > 0 && counts.wall > 0 {
        Treatment::Mixed
    } else if counts.bank > 0 {
        Treatment::WetDryBank
    } else if counts.wall > 0 {
        Treatment::RigidWall
    } else if counts.unsupported > 0 {
        Treatment::Unsupported
    } else {
        Treatment::FftOnly
    }
}

fn push_line(mesh: &mut DebugVolumeMesh, a: [f32; 3], b: [f32; 3], color: [u8; 4]) {
    let base = mesh.positions.len() as u32;
    mesh.positions.extend_from_slice(&[a, b]);
    mesh.colors.extend_from_slice(&[color, color]);
    mesh.line_indices.extend_from_slice(&[base, base + 1]);
}

fn push_triangle_fill(mesh: &mut DebugVolumeMesh, points: [[f32; 3]; 3], color: [u8; 4]) {
    let base = mesh.positions.len() as u32;
    mesh.positions.extend_from_slice(&points);
    mesh.colors.extend_from_slice(&[color; 3]);
    mesh.triangle_indices
        .extend_from_slice(&[base, base + 1, base + 2]);
}

fn promoted_rectangle(ocean: &super::appearance::PromotedOcean, z: f32) -> [[f32; 3]; 4] {
    let surface = ocean.surface;
    [
        [surface.minimum[0], -surface.maximum[1], z],
        [surface.maximum[0], -surface.maximum[1], z],
        [surface.maximum[0], -surface.minimum[1], z],
        [surface.minimum[0], -surface.minimum[1], z],
    ]
}

fn build_debug_view(
    atlas: &Atlas,
    mode: u8,
    promoted: &[super::appearance::PromotedOcean],
    footprints: &[(u32, f32, Vec<bathymetry::FootprintTriangle>)],
) -> (Option<DebugVolumeMesh>, Vec<String>) {
    let mut mesh = DebugVolumeMesh::default();
    let mut lines = vec![
        "r_coastalDebug: green wet/dry bank; blue rigid wall; yellow mixed; gray FFT-only; magenta unsupported; cyan footprint.".into(),
    ];
    for ocean in promoted {
        let grids: Vec<_> = atlas
            .grids
            .iter()
            .filter(|grid| grid.slot as usize == ocean.index)
            .collect();
        let exact = footprints
            .iter()
            .find(|(slot, _, _)| *slot as usize == ocean.index);
        if grids.is_empty() {
            let reason = atlas
                .warnings
                .iter()
                .find(|warning| warning.starts_with(&format!("slot {}:", ocean.index)))
                .map(String::as_str)
                .unwrap_or("no bathymetry grid was produced");
            let selected = if reason.contains("FFT-only") {
                Treatment::FftOnly
            } else {
                Treatment::Unsupported
            };
            lines.push(format!(
                "{}: {} - {reason}",
                ocean.label(),
                selected.label()
            ));
            let debug_z = ocean.surface.plane_height + ocean.heuristic.amplitude.max(2.0) + 4.0;
            if mode == 1 || mode == 3 {
                if let Some((_, _, triangles)) = exact {
                    for triangle in triangles {
                        push_triangle_fill(
                            &mut mesh,
                            triangle.map(|point| [point[0], point[1], debug_z]),
                            selected.color(),
                        );
                    }
                } else {
                    let polygon = promoted_rectangle(ocean, debug_z);
                    for triangle in [
                        [polygon[0], polygon[1], polygon[2]],
                        [polygon[0], polygon[2], polygon[3]],
                    ] {
                        push_triangle_fill(&mut mesh, triangle, selected.color());
                    }
                }
            } else if mode == 2 {
                if let Some((_, _, triangles)) = exact {
                    for triangle in triangles {
                        let points = triangle.map(|point| [point[0], point[1], debug_z]);
                        for edge in 0..3 {
                            push_line(
                                &mut mesh,
                                points[edge],
                                points[(edge + 1) % 3],
                                selected.color(),
                            );
                        }
                    }
                } else {
                    let polygon = promoted_rectangle(ocean, debug_z);
                    for edge in polygon.windows(2) {
                        push_line(&mut mesh, edge[0], edge[1], selected.color());
                    }
                    push_line(&mut mesh, polygon[3], polygon[0], selected.color());
                }
            }
            continue;
        }
        let mut counts = BoundaryCounts::default();
        for grid in &grids {
            let part = face_counts(grid);
            counts.bank += part.bank;
            counts.wall += part.wall;
            counts.outline += part.outline;
            counts.unsupported += part.unsupported;
        }
        let wet: Vec<_> = grids
            .iter()
            .flat_map(|grid| grid.cells.iter())
            .filter(|cell| cell.class == Class::Wet)
            .collect();
        let selected = treatment(!wet.is_empty(), counts);
        let wavelength = ocean.heuristic.wavelength.max(1.0);
        let shallow = wet
            .iter()
            .filter(|cell| cell.depth / wavelength <= 0.05)
            .count();
        let transition = wet
            .iter()
            .filter(|cell| cell.depth / wavelength > 0.05 && cell.depth / wavelength <= 0.5)
            .count();
        let deep = wet.len().saturating_sub(shallow + transition);
        let allocated_cells: usize = grids.iter().map(|grid| grid.cells.len()).sum();
        let spacing = grids[0].cell_size;
        let envelope_cells = (((ocean.surface.maximum[0] - ocean.surface.minimum[0]) / spacing)
            .ceil()
            .max(0.0) as usize)
            .saturating_mul(
                ((ocean.surface.maximum[1] - ocean.surface.minimum[1]) / spacing)
                    .ceil()
                    .max(0.0) as usize,
            );
        lines.push(format!(
            "{}: {}; {} footprint tiles, {allocated_cells} allocated cells vs {envelope_cells} envelope; bank {:.0}u, wall {:.0}u, outline {:.0}u, unresolved {:.0}u; wet cells shallow/transition/deep {shallow}/{transition}/{deep}",
            ocean.label(),
            selected.label(),
            grids.len(),
            counts.bank as f32 * grids[0].cell_size,
            counts.wall as f32 * grids[0].cell_size,
            counts.outline as f32 * grids[0].cell_size,
            counts.unsupported as f32 * grids[0].cell_size,
        ));
        if let Some(band) = atlas
            .shoreline_bands
            .iter()
            .find(|b| b.slot as usize == ocean.index)
        {
            lines.push(format!("Pool {} allocation: shoreline bands + halo, {} / {} footprint tiles retained; screening depth {:.1}u; bank patch radius {} cells (0 = full shallow band); offshore stays FFT.",
                ocean.index + 1, band.retained_tiles, band.full_tiles, band.max_depth, band.patch_radius_cells));
        }
        // Float diagnostics above the promoted wave envelope so depth testing
        // does not make the overlay disappear under FFT crests.
        let z = grids[0].level + ocean.heuristic.amplitude.max(2.0) + 4.0;
        match mode {
            1 => {
                if let Some((_, _, triangles)) = exact {
                    for triangle in triangles {
                        push_triangle_fill(
                            &mut mesh,
                            triangle.map(|point| [point[0], point[1], z]),
                            selected.color(),
                        );
                    }
                }
            }
            2 => {
                let boundary_color = |boundary| match boundary {
                    Boundary::Bank => [54, 225, 92, 255],
                    Boundary::Wall => [55, 132, 255, 255],
                    Boundary::Outline => [72, 224, 238, 255],
                    Boundary::Unsupported => [255, 52, 220, 255],
                    Boundary::Open => [0, 0, 0, 0],
                };
                for grid in &grids {
                    let (nx, ny) = (grid.size[0] as usize, grid.size[1] as usize);
                    let wet_at = |x: usize, y: usize| grid.cells[y * nx + x].class == Class::Wet;
                    for y in 0..ny {
                        for x in 0..=nx {
                            let face = grid.faces_x[y * (nx + 1) + x];
                            if face.boundary != Boundary::Open
                                && ((x > 0 && wet_at(x - 1, y)) || (x < nx && wet_at(x, y)))
                            {
                                let px = grid.origin[0] + x as f32 * grid.cell_size;
                                let py = grid.origin[1] + y as f32 * grid.cell_size;
                                push_line(
                                    &mut mesh,
                                    [px, py, z],
                                    [px, py + grid.cell_size, z],
                                    boundary_color(face.boundary),
                                );
                            }
                        }
                    }
                    for y in 0..=ny {
                        for x in 0..nx {
                            let face = grid.faces_y[y * nx + x];
                            if face.boundary != Boundary::Open
                                && ((y > 0 && wet_at(x, y - 1)) || (y < ny && wet_at(x, y)))
                            {
                                let px = grid.origin[0] + x as f32 * grid.cell_size;
                                let py = grid.origin[1] + y as f32 * grid.cell_size;
                                push_line(
                                    &mut mesh,
                                    [px, py, z],
                                    [px + grid.cell_size, py, z],
                                    boundary_color(face.boundary),
                                );
                            }
                        }
                    }
                }
            }
            3 => {
                for grid in &grids {
                    let nx = grid.size[0] as usize;
                    for (index, cell) in grid.cells.iter().enumerate() {
                        if cell.class == Class::Outside {
                            continue;
                        }
                        let ratio = cell.depth / wavelength;
                        let color = match cell.class {
                            Class::Wet if ratio <= 0.05 => [67, 230, 151, 100],
                            Class::Wet if ratio <= 0.5 => [42, 174, 235, 92],
                            Class::Wet => [54, 89, 215, 84],
                            Class::Dry => [145, 105, 57, 100],
                            Class::Outline => [244, 215, 74, 100],
                            Class::NoFloor | Class::Overhang | Class::Mixed => [255, 52, 220, 112],
                            Class::Outside => unreachable!(),
                        };
                        let (x, y) = (index % nx, index / nx);
                        let min = [
                            grid.origin[0] + x as f32 * grid.cell_size,
                            grid.origin[1] + y as f32 * grid.cell_size,
                            z,
                        ];
                        mesh.push_polygon(
                            &[
                                min,
                                [min[0] + grid.cell_size, min[1], z],
                                [min[0] + grid.cell_size, min[1] + grid.cell_size, z],
                                [min[0], min[1] + grid.cell_size, z],
                            ],
                            color,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    ((!mesh.is_empty()).then_some(mesh), lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises real runtime shader resolution and PVS batches, rather than
    /// the probe's raw CONTENTS_WATER discovery. Requires the user's local assets.
    #[test]
    #[ignore = "requires target/release/base retail and RaceArena assets"]
    fn coastal_checkpoint_real_runtime_maps() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let base = workspace.join("target/release/base");
        for map_name in ["racearena_pro", "yavin1"] {
            let options = crate::scene::MapPrepareOptions {
                ocean: true,
                grass: false,
                voxel_probe_gi: false,
                client_physics: false,
                gen_normal_maps: false,
                steam_audio: false,
                planar_reflections: false,
                planar_environment: false,
                ..Default::default()
            };
            let started = Instant::now();
            let map = crate::scene::prepare_with_options(&base, None, map_name, options)
                .expect("prepare real map");
            let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
            let job = map
                .coastal_bathymetry
                .as_ref()
                .expect("promoted-water background job");
            let deadline = Instant::now() + std::time::Duration::from_secs(60);
            loop {
                let status = job.status.lock().unwrap();
                match &*status {
                    Status::Pending => {}
                    Status::Failed(e) => panic!("{map_name}: {e}"),
                    Status::Ready {
                        atlas,
                        cache_hit,
                        total_ms,
                        ..
                    } => {
                        assert!(!atlas.grids.is_empty());
                        assert!(atlas
                            .grids
                            .iter()
                            .flat_map(|g| &g.cells)
                            .any(|c| c.class == bathymetry::Class::Wet));
                        let report = workspace
                            .join("target/coastal-checkpoint1/runtime")
                            .join(map_name);
                        atlas.write_debug(&report).expect("write runtime results");
                        std::fs::write(report.join("runtime.txt"),format!("Runtime shader/PVS discovery\nMap: {map_name}\nPrepare returned in {prepare_ms:.3} ms without waiting for the bake\nCoastal selection on loader: {:.3} ms\nWorker including cache/report IO: {total_ms:.3} ms\nCache hit: {cache_hit}\n",job.selection_ms)).unwrap();
                        eprintln!("{map_name}: prepare={prepare_ms:.3} ms, worker={total_ms:.3} ms, cache_hit={cache_hit}\n{}",atlas.summary());
                        break;
                    }
                }
                drop(status);
                assert!(
                    Instant::now() < deadline,
                    "coastal geometry worker timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    }
}
