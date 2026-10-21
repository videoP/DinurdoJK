//! Blue Water's depth/fetch/surf model. Separate data, worker and controls from the JKA coastal solver.
use super::OCEAN_UNITS_PER_METER;
use blue_water_gpu::shore::ShoreMap;
use std::sync::{Arc, Mutex};
type Footprints = Vec<(
    u32,
    f32,
    Vec<jka_assets::bsp::bathymetry::FootprintTriangle>,
)>;
pub type Maps = Vec<(usize, Arc<ShoreMap>)>;
struct Terrain {
    slot: usize,
    heights: Vec<f32>,
    columns: usize,
    rows: usize,
    cell: f64,
    origin: [f64; 2],
}
#[derive(Default)]
struct State {
    requested: u32,
    working: bool,
    failed: bool,
    terrain: Option<Arc<Vec<Terrain>>>,
    ready: Option<(u32, Arc<Maps>)>,
}
pub struct Source {
    bsp: Arc<jka_assets::bsp::Bsp>,
    footprints: Footprints,
    state: Mutex<State>,
    /// `r_oceanType 5`'s contact fields (`super::gerstner_contact`), built on first request.
    contact: Mutex<ContactState>,
}
#[derive(Default)]
struct ContactState {
    working: bool,
    failed: bool,
    ready: Option<super::gerstner_contact::ContactFields>,
}
impl Source {
    pub fn report(&self, at: [f64; 2], settings: &super::OceanSettings) -> Vec<String> {
        let mut lines = vec![format!(
            "BLUE WATER COAST: enabled={} shelter={} backend={} | Hs {:.2}m, wavelength {:.2}m, wind {:.2}m/s, direction {:.1}deg | eye XZ {:.1},{:.1}m",
            settings.blue_water_coast, settings.blue_water_coast_shelter, settings.backend,
            settings.blue_water_waves.height, settings.blue_water_waves.wavelength,
            settings.blue_water_waves.wind_speed, settings.blue_water_waves.direction, at[0], at[1])];
        lines.push(self.status().into());
        let ready = self.state.try_lock().ok().and_then(|s| s.ready.clone());
        let Some((direction, maps)) = ready else {
            return lines;
        };
        let params = settings.blue_water_waves.parameters();
        let sea = blue_water_core::sea_state::drawn_sea(&params, false);
        let cascades = blue_water_core::sea_state::sea_state_cascades(
            &blue_water_core::QualityTier::High.cascades(),
            sea.peak_wavelength,
        );
        let spectrum = blue_water_core::spectrum::build_spectrum(&cascades, &sea);
        let mut stats = blue_water_gpu::surface_waves::SpectrumStats::from_spectrum(&spectrum);
        stats.set_shore_tables(&spectrum, sea.wind_speed);
        let swell = super::hybrid_swell_uniform(settings, 0.0);
        for (slot, map) in maps.iter() {
            let depth_only;
            let map = if settings.blue_water_coast_shelter {
                map.as_ref()
            } else {
                depth_only = blue_water_gpu::surface_waves::coast_depth_only(map);
                &depth_only
            };
            let sample = stats.sample_shore(
                map,
                at,
                swell[1][2],
                4.0 * swell[0][3].abs() / std::f32::consts::SQRT_2,
            );
            lines.push(format!("Pool {}: map direction {:.1}deg; depth {}; fetch wind {:.1}m / swell {:.1}m; arriving Hs {:.4}m; kept FFT {:.4}% / {:.4}% / {:.4}%, analytic {:.4}%",
                slot+1, f32::from_bits(direction).to_degrees(),
                sample.depth.map_or("outside height map (open)".into(), |d| if d >= 9999.0 { "unknown (deep/open fallback)".into() } else { format!("{d:.3}m") }),
                sample.wind_fetch, sample.swell_fetch, sample.arriving,
                sample.scales[0]*100.0, sample.scales[1]*100.0, sample.scales[2]*100.0, sample.scales[3]*100.0));
            if sample.scales[..3].iter().all(|s| *s < 0.005) {
                lines.push("Almost all FFT motion is attenuated here. Try r_blueWaterCoastShelter 0 to isolate shelter, or r_blueWaterCoast 0 to bypass the coast.".into());
            }
        }
        lines.push("CPU estimate at the eye's horizontal position; no GPU readback. Open water outside the coastal map retains the open-ocean field. Source-raster unknowns use deep/open fallback.".into());
        lines
    }
    pub fn status(&self) -> &'static str {
        let Ok(state) = self.state.try_lock() else {
            return "Preparing Blue Water shores...";
        };
        if state.failed {
            "Blue Water shores unavailable; using open-ocean waves."
        } else if state.working {
            "Preparing Blue Water shores..."
        } else if state.ready.is_some() {
            "Blue Water shores ready."
        } else {
            "Blue Water shores will prepare when the ocean is active."
        }
    }
    pub fn new(bsp: Arc<jka_assets::bsp::Bsp>, footprints: Footprints) -> Arc<Self> {
        Arc::new(Self {
            bsp,
            footprints,
            state: Mutex::new(State::default()),
            contact: Mutex::new(ContactState::default()),
        })
    }
    /// Nonblocking render-thread request for `r_oceanType 5`'s contact fields; `None` until the worker finishes (or
    /// for good, if it failed).
    pub fn request_contact(self: &Arc<Self>) -> Option<super::gerstner_contact::ContactFields> {
        let mut state = self.contact.try_lock().ok()?;
        if let Some(ready) = &state.ready {
            return Some(Arc::clone(ready));
        }
        if state.working || state.failed {
            return None;
        }
        state.working = true;
        let source = Arc::clone(self);
        if let Err(e) = std::thread::Builder::new()
            .name("gerstner-contact".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::gerstner_contact::build(&source.bsp, &source.footprints)
                }));
                let Ok(mut state) = source.contact.lock() else { return; };
                state.working = false;
                match built {
                    Ok(Ok(fields)) => {
                        eprintln!("GERSTNER CONTACT: {} pool fields in {:.0} ms", fields.len(),
                            started.elapsed().as_secs_f64() * 1000.0);
                        state.ready = Some(Arc::new(fields));
                    }
                    failure => {
                        eprintln!("GERSTNER CONTACT: unavailable, no intersection foam: {}", match failure {
                            Ok(Err(e)) => e,
                            _ => "worker panicked".into(),
                        });
                        state.failed = true;
                    }
                }
            })
        {
            state.working = false;
            state.failed = true;
            eprintln!("GERSTNER CONTACT: worker failed: {e}");
        }
        None
    }
    /// Nonblocking render-thread request. A changed wind replaces stale exposure; the last request wins.
    pub fn request(self: &Arc<Self>, direction: f32) -> Option<Arc<Maps>> {
        let key = direction.to_bits();
        let mut state = self.state.try_lock().ok()?;
        state.requested = key;
        if let Some((ready, maps)) = &state.ready {
            if *ready == key {
                return Some(Arc::clone(maps));
            }
        }
        if state.working || state.failed {
            return None;
        }
        state.working = true;
        let source = Arc::clone(self);
        if let Err(e) = std::thread::Builder::new()
            .name("blue-water-shore".into())
            .spawn(move || source.work())
        {
            state.working = false;
            state.failed = true;
            eprintln!("BLUE WATER COAST: worker failed: {e}");
        }
        None
    }
    fn terrain(&self) -> Result<Vec<Terrain>, String> {
        let geometry = jka_assets::bsp::bathymetry::Geometry::from_bsp(&self.bsp)?;
        let mut result = Vec::new();
        for (slot, level, triangles) in &self.footprints {
            if triangles.is_empty() {
                continue;
            }
            let mut low = [f32::INFINITY; 2];
            let mut high = [f32::NEG_INFINITY; 2];
            for p in triangles.iter().flatten() {
                for a in 0..2 {
                    low[a] = low[a].min(p[a]);
                    high[a] = high[a].max(p[a]);
                }
            }
            // Include real banks/islands outside the water outline for wind shelter. Texture edge means open sea.
            let halo = 128.0 * OCEAN_UNITS_PER_METER;
            for a in 0..2 {
                low[a] -= halo;
                high[a] += halo;
            }
            let cell = ((high[0] - low[0]).max(high[1] - low[1]) / 512.0).max(64.0);
            let size =
                [0, 1].map(|a| (((high[a] - low[a]) / cell).ceil() as usize + 1).clamp(2, 513));
            let mut heights =
                geometry.upstream_shore_heights(low, size, cell, *level, triangles)?;
            let known = heights.iter().filter(|h| h.is_finite()).count();
            // JKA +Y maps to renderer -Z. Unknown columns are open/deep, never guessed banks or shallow surf.
            for h in &mut heights {
                *h = if h.is_finite() {
                    (*h / OCEAN_UNITS_PER_METER).clamp(-10000.0, 10000.0)
                } else {
                    -10000.0
                };
            }
            let mut flipped = Vec::with_capacity(heights.len());
            for row in heights.chunks_exact(size[0]).rev() {
                flipped.extend_from_slice(row);
            }
            eprintln!(
                "BLUE WATER COAST: pool {} raster {}x{}, {:.2}m/cell, {known}/{} known columns",
                slot + 1,
                size[0],
                size[1],
                cell / OCEAN_UNITS_PER_METER,
                heights.len()
            );
            result.push(Terrain {
                slot: *slot as usize,
                heights: flipped,
                columns: size[0],
                rows: size[1],
                cell: f64::from(cell / OCEAN_UNITS_PER_METER),
                origin: [
                    f64::from(low[0] / OCEAN_UNITS_PER_METER),
                    f64::from(-(low[1] + (size[1] - 1) as f32 * cell) / OCEAN_UNITS_PER_METER),
                ],
            });
        }
        Ok(result)
    }
    fn work(self: Arc<Self>) {
        let work =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                let terrain = self
                    .state
                    .lock()
                    .map_err(|_| "shore state poisoned")?
                    .terrain
                    .clone();
                let terrain = match terrain {
                    Some(t) => t,
                    None => Arc::new(self.terrain()?),
                };
                self.state
                    .lock()
                    .map_err(|_| "shore state poisoned")?
                    .terrain = Some(Arc::clone(&terrain));
                loop {
                    let key = self
                        .state
                        .lock()
                        .map_err(|_| "shore state poisoned")?
                        .requested;
                    let maps: Maps = terrain
                        .iter()
                        .map(|t| {
                            (
                                t.slot,
                                Arc::new(ShoreMap::build(
                                    t.heights.clone(),
                                    t.columns,
                                    t.rows,
                                    t.cell,
                                    t.origin,
                                    f64::from(f32::from_bits(key)),
                                )),
                            )
                        })
                        .collect();
                    let mut state = self.state.lock().map_err(|_| "shore state poisoned")?;
                    if state.requested != key {
                        continue;
                    }
                    eprintln!(
                        "BLUE WATER COAST: {} pool maps ready; wave direction {:.1} degrees",
                        maps.len(),
                        f32::from_bits(key).to_degrees()
                    );
                    state.ready = Some((key, Arc::new(maps)));
                    state.working = false;
                    return Ok(());
                }
            }));
        if !matches!(work, Ok(Ok(()))) {
            eprintln!(
                "BLUE WATER COAST: terrain unavailable; retaining open-water waves: {}",
                match work {
                    Ok(Err(e)) => e,
                    _ => "worker panicked".into(),
                }
            );
            if let Ok(mut state) = self.state.lock() {
                state.working = false;
                state.failed = true;
            }
        }
    }
}
