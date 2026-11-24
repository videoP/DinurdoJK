//! The coast in the waves (native only): what a shore leaves of the open sea's waves near it. The wave field draws one
//! sea everywhere; near land three things take it apart, each a share of a wave's amplitude that the wave reads
//! (`shaders/waves.wgsl`'s `shore_at`) apply to its displacement, slopes and foam:
//!
//! - **Fetch.** Wind waves grow with the open water the wind has blown over (the fetch). In the lee of land the sea
//!   is young: its peak wavelength is the fetch's, `λp = 0.0130·(U²/g)^0.34·F^0.66` (JONSWAP's
//!   `fp = 3.5·(g/U)·(gF/U²)^-0.33` with `λ = g/(2π·f²)`), and waves longer than that have not yet been raised. Each
//!   cascade keeps `exp(-5/8·λ²·(λp(F)⁻² − λp(F_open)⁻²))` of each wave's amplitude (the low-frequency side of
//!   JONSWAP's `exp(-5/4·(fp/f)⁴)`), and the root of the energy its waves keep ([`band_fetch_table`], tabled over
//!   fetch for the shader), so a band keeps its short waves where the fetch takes its long ones. The fetch is
//!   Saville's effective fetch: the open water upwind along seven bearings within 45° of the wind's, weighed by cos².
//!   A sea with no land within the open fetch upwind is the open sea, drawn as it is.
//! - **Swell.** The long swell (the sea hulls ride and its spread, `crate::hull_sea`) comes from far off, along the
//!   wind. Land upwind casts its shadow; the swell fills back in over [`SWELL_SHELTER_M`] of open water (diffraction
//!   round the land's ends). Near the hulls it is kept whole: they ride the simulation's sea, which knows no coast.
//! - **Depth.** No wave stands higher than [`BREAKING`] of the water's depth: in the surf zone the waves break and
//!   their height falls with the depth to nothing at the waterline. The shader draws the breakers as foam lines
//!   running in toward the beach once a swell period, closer together as the water shoals.
//!
//! [`ShoreMap`] is the CPU half: the land's heights on its own grid, and the upwind open water (wind sea and swell) on a
//! coarser one, built off the frame loop's thread from a land window and the wind's direction. The pure functions are
//! what the shader evaluates, for tests and CPU callers.

use crate::spectrum::GRAVITY;

/// Open water beyond which the sea is the open sea's (m): no land within it upwind, nothing taken away.
pub const OPEN_FETCH_M: f64 = 100_000.0;
/// The swell fills a land's shadow back in over about this much open water (m), its e-folding length.
pub const SWELL_SHELTER_M: f64 = 15_000.0;
/// The highest a wave stands against the water's depth (significant height over depth in the surf zone; a spilling
/// beach's breakers, Battjes & Stive 1985's γ for Hs).
pub const BREAKING: f64 = 0.42;
/// The bearings the wind sea's fetch is measured along, either side of the wind's (radians): Saville's within ±45°.
pub const FETCH_BEARINGS: [f64; 7] = [-0.785, -0.524, -0.262, 0.0, 0.262, 0.524, 0.785];
/// The swell's, a narrower cone: it comes from one quarter.
pub const SWELL_BEARINGS: [f64; 3] = [-0.175, 0.0, 0.175];
/// The exposure grid takes every this-many'th sample of the land's: the fetch changes over kilometres.
pub const EXPOSURE_STRIDE: usize = 4;
/// The fetch law's exponent on fetch, squared wavelength over squared peak: λp ∝ F^0.66.
pub const FETCH_POWER: f64 = 1.32;
/// Below this wind (m/s) the sea is a calm the fetch does not shape.
const STILL_WIND: f64 = 0.5;

/// A grid of values over a land window's chart: sample (i, j) at chart point origin + (i, j)·cell, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct ShoreGrid<T> {
    pub origin: [f64; 2],
    pub cell: f64,
    pub columns: usize,
    pub rows: usize,
    pub values: Vec<T>,
}

impl<T> ShoreGrid<T> {
    /// Where its samples lie.
    pub fn place(&self) -> GridPlace {
        GridPlace {
            origin: self.origin,
            cell: self.cell,
            columns: self.columns,
            rows: self.rows,
        }
    }
}

/// Where a grid's samples lie in its chart: sample (i, j) at origin + (i, j)·cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridPlace {
    pub origin: [f64; 2],
    pub cell: f64,
    pub columns: usize,
    pub rows: usize,
}

impl GridPlace {
    /// Where its texels lie in the world for the shader, the chart placed at `offset` (world = chart + offset): (x, z)
    /// of the first texel's corner, and the texture coordinate per metre on each axis.
    pub fn frame(&self, offset: [f64; 2]) -> [f32; 4] {
        [
            (self.origin[0] + offset[0] - self.cell / 2.0) as f32,
            (self.origin[1] + offset[1] - self.cell / 2.0) as f32,
            (1.0 / (self.columns as f64 * self.cell)) as f32,
            (1.0 / (self.rows as f64 * self.cell)) as f32,
        ]
    }
}

/// What the waves know of a coast, in the chart its land is drawn in (world = chart + the land's offset).
#[derive(Clone, Debug, PartialEq)]
pub struct ShoreMap {
    /// The land's heights (m; negative under water), sample for sample as the land layer draws them.
    pub heights: ShoreGrid<f32>,
    /// Per sample of a coarser grid ([`EXPOSURE_STRIDE`]): the wind sea's effective fetch and the swell's open water
    /// upwind (m), at most [`OPEN_FETCH_M`]; on land the sea's that runs onto it, 0 behind the beach
    /// ([`upwind_water`]).
    pub exposure: ShoreGrid<[f32; 2]>,
    /// The direction the wind blows toward (radians from +X toward +Z, `WaveParameters::wind_direction`) the fetch
    /// was measured for.
    pub toward: f64,
}

impl ShoreMap {
    /// The map of a land window: `heights` (m, row-major, `columns` × `rows` samples `cell` metres apart from chart
    /// point `origin`) with the wind blowing toward `toward`.
    pub fn build(
        heights: Vec<f32>,
        columns: usize,
        rows: usize,
        cell: f64,
        origin: [f64; 2],
        toward: f64,
    ) -> Self {
        assert_eq!(heights.len(), columns * rows, "a height per sample");
        let stride = EXPOSURE_STRIDE;
        let (coarse_columns, coarse_rows) = ((columns - 1) / stride + 1, (rows - 1) / stride + 1);
        let land: Vec<bool> = (0..coarse_rows)
            .flat_map(|j| (0..coarse_columns).map(move |i| (i, j)))
            .map(|(i, j)| heights[j * stride * columns + i * stride] > 0.0)
            .collect();
        let coarse = cell * stride as f64;
        let wind = effective_fetch(
            &land,
            coarse_columns,
            coarse_rows,
            coarse,
            toward,
            &FETCH_BEARINGS,
        );
        let swell = effective_fetch(
            &land,
            coarse_columns,
            coarse_rows,
            coarse,
            toward,
            &SWELL_BEARINGS,
        );
        Self {
            heights: ShoreGrid {
                origin,
                cell,
                columns,
                rows,
                values: heights,
            },
            exposure: ShoreGrid {
                origin,
                cell: coarse,
                columns: coarse_columns,
                rows: coarse_rows,
                values: wind.into_iter().zip(swell).map(|(w, s)| [w, s]).collect(),
            },
            toward,
        }
    }
}

/// The open water upwind of every sample of a land mask (`columns` × `rows`, `cell` metres apart) along the line
/// against `toward` (radians: the direction the wind blows toward): metres to the first land met, at most `open`, and
/// `open` where the line leaves the grid first. A land sample holds the water that runs onto it, as though the sea went
/// one step on (so the water between a beach and the last sea sample reads the sea's fetch, not a share of the land's),
/// and 0 behind the beach. One sweep from the upwind edge, each sample one step on from its upwind neighbour, read
/// between the two samples that line passes.
pub fn upwind_water(
    land: &[bool],
    columns: usize,
    rows: usize,
    cell: f64,
    toward: f64,
    open: f64,
) -> Vec<f32> {
    let (ux, uz) = (-toward.cos(), -toward.sin());
    // Swept along x or z, whichever the line runs more along: its upwind neighbour is one column (row) on, within a
    // sample across.
    let along_x = ux.abs() >= uz.abs();
    let (lines, length, step_along, across) = if along_x {
        (columns, rows, ux, uz)
    } else {
        (rows, columns, uz, ux)
    };
    let shift = across / step_along.abs();
    let step = (cell / step_along.abs()) as f32;
    let open = open as f32;
    let index = |line: usize, k: usize| {
        if along_x {
            k * columns + line
        } else {
            line * columns + k
        }
    };
    // The water's run (0 on land, which stops it) and what each sample holds.
    let mut run = vec![0f32; columns * rows];
    let mut out = vec![0f32; columns * rows];
    let forward = step_along > 0.0;
    for n in 0..lines {
        let line = if forward { lines - 1 - n } else { n };
        let upwind = if forward {
            line.checked_add(1).filter(|&l| l < lines)
        } else {
            line.checked_sub(1)
        };
        for k in 0..length {
            let at = index(line, k);
            let along = k as f64 + shift;
            let reach = match upwind {
                Some(previous) if along >= 0.0 && along <= (length - 1) as f64 => {
                    let k0 = (along.floor() as usize).min(length - 1);
                    let k1 = (k0 + 1).min(length - 1);
                    let t = (along - k0 as f64) as f32;
                    Some(run[index(previous, k0)] * (1.0 - t) + run[index(previous, k1)] * t)
                }
                _ => None,
            };
            if land[at] {
                out[at] = reach
                    .filter(|&r| r > 0.0)
                    .map_or(0.0, |r| (r + step).min(open));
            } else {
                run[at] = reach.map_or(open, |r| (r + step).min(open));
                out[at] = run[at];
            }
        }
    }
    out
}

/// The open water upwind along each of `bearings` (radians either side of the wind's), weighed by cos² of the bearing
/// (Saville's effective fetch: an open sea on every bearing is the open sea's).
pub fn effective_fetch(
    land: &[bool],
    columns: usize,
    rows: usize,
    cell: f64,
    toward: f64,
    bearings: &[f64],
) -> Vec<f32> {
    let mut sum = vec![0f32; columns * rows];
    let mut weights = 0.0;
    for &bearing in bearings {
        let weight = bearing.cos().powi(2);
        weights += weight;
        let line = upwind_water(land, columns, rows, cell, toward + bearing, OPEN_FETCH_M);
        for (total, reach) in sum.iter_mut().zip(line) {
            *total += reach * weight as f32;
        }
    }
    sum.iter().map(|total| total / weights as f32).collect()
}

/// The peak wavelength (m) of a sea raised by `wind` m/s over `fetch` m (JONSWAP; Hasselmann et al. 1973).
pub fn fetch_peak_wavelength(wind: f64, fetch: f64) -> f64 {
    let fp = 3.5 * GRAVITY / wind * (GRAVITY * fetch / (wind * wind)).powf(-0.33);
    GRAVITY / (2.0 * std::f64::consts::PI * fp * fp)
}

/// The fetch law's coefficient for waves of `wavelength` m under `wind` m/s: the share of their amplitude kept at a
/// fetch F (km) is exp(−q·(F^−1.32 − F_open^−1.32)) ([`fetch_scale`]). 0 (all kept) for no wind or no waves.
pub fn fetch_coefficient(wavelength: f64, wind: f64) -> f64 {
    if !(wind >= STILL_WIND && wavelength > 0.0) {
        return 0.0;
    }
    // λp = a·F^0.66 (F in km): a from the law at one kilometre.
    let a = fetch_peak_wavelength(wind, 1000.0);
    0.625 * wavelength * wavelength / (a * a)
}

/// The share of a wave's amplitude kept at `fetch` m of open water upwind, its law's coefficient `q`
/// ([`fetch_coefficient`]): all of it on the open sea.
pub fn fetch_scale(q: f64, fetch: f64) -> f64 {
    let km = |m: f64| (m / 1000.0).max(1e-3).powf(-FETCH_POWER);
    (-q * (km(fetch) - km(OPEN_FETCH_M)).max(0.0)).exp()
}

/// The share of the swell kept at `fetch` m of open water upwind: none against the land, filling back in over
/// [`SWELL_SHELTER_M`], all of it on the open sea.
pub fn swell_scale(fetch: f64) -> f64 {
    let fill = |m: f64| 1.0 - (-m.max(0.0) / SWELL_SHELTER_M).exp();
    (fill(fetch) / fill(OPEN_FETCH_M)).min(1.0)
}

/// The share of a wave of significant height `height` (m, as the open sea draws it) kept in `depth` m of water.
pub fn depth_scale(height: f64, depth: f64) -> f64 {
    if height <= 0.0 {
        return 1.0;
    }
    (BREAKING * depth.max(0.0) / height).min(1.0)
}

/// Fetches the band table holds ([`band_fetch_table`]): [`FETCH_TABLE`] of them, evenly spaced in log from
/// [`FETCH_LEAST_M`] to [`OPEN_FETCH_M`].
pub const FETCH_TABLE: usize = 16;
pub const FETCH_LEAST_M: f64 = 50.0;

/// The fetch (m) of the table's entry `i`.
pub fn table_fetch(i: usize) -> f64 {
    let t = i as f64 / (FETCH_TABLE - 1) as f64;
    FETCH_LEAST_M * (OPEN_FETCH_M / FETCH_LEAST_M).powf(t)
}

/// A band's share of its amplitude kept at each of the table's fetches under `wind` m/s: its (kx, kz, variance) modes
/// (`hull_sea::cascade_modes`) each keep [`fetch_scale`] of their amplitude, and the band keeps the root of the energy
/// left, so a band whose long waves the fetch takes keeps its short ones' share. 1 throughout with no modes or wind.
pub fn band_fetch_table(modes: &[f32], wind: f64) -> [f64; FETCH_TABLE] {
    let modes: Vec<(f64, f64)> = modes
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|m| {
            let k = f64::from(m[0]).hypot(f64::from(m[1]));
            (k > 0.0).then(|| {
                (
                    fetch_coefficient(2.0 * std::f64::consts::PI / k, wind),
                    f64::from(m[2]),
                )
            })
        })
        .collect();
    let total: f64 = modes.iter().map(|&(_, v)| v).sum();
    std::array::from_fn(|i| {
        if total <= 0.0 {
            return 1.0;
        }
        let fetch = table_fetch(i);
        let kept: f64 = modes
            .iter()
            .map(|&(q, v)| v * fetch_scale(q, fetch).powi(2))
            .sum();
        (kept / total).sqrt()
    })
}

/// A band's share at `fetch` m read from its table ([`band_fetch_table`]) as the shader reads it: linear between the
/// entries in log fetch, the first entry's below them and 1 beyond the open fetch.
pub fn table_scale(table: &[f64; FETCH_TABLE], fetch: f64) -> f64 {
    let t = (fetch.max(1e-3) / FETCH_LEAST_M).ln() / (OPEN_FETCH_M / FETCH_LEAST_M).ln()
        * (FETCH_TABLE - 1) as f64;
    let t = t.clamp(0.0, (FETCH_TABLE - 1) as f64);
    let i = (t.floor() as usize).min(FETCH_TABLE - 2);
    let f = t - i as f64;
    table[i] * (1.0 - f) + table[i + 1] * f
}

impl ShoreGrid<f32> {
    /// Bilinear height using known columns. A majority-unknown footprint or
    /// a point outside the grid falls back to open water.
    pub fn sample(&self, at: [f64; 2]) -> Option<f64> {
        let weight = self.sample_with(at, |k| f32::from(self.values[k].is_finite()))?;
        if weight < 0.5 {
            return None;
        }
        self.sample_with(at, |k| {
            let height = self.values[k];
            if height.is_finite() { height } else { 0.0 }
        }).map(|height| height / weight)
    }
}

/// What a coast leaves of the waves at a point (the shader's `shore_at`, on the CPU, for read-backs and tests).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShoreSample {
    /// The water's depth (m; none off the map) and the open water upwind of the wind sea and of the swell (m).
    pub depth: Option<f64>,
    pub wind_fetch: f64,
    pub swell_fetch: f64,
    /// Per cascade and (last) the swell: the share of the open sea's amplitude drawn there.
    pub scales: [f64; 4],
    /// The significant height of the waves arriving there before the depth breaks them (m).
    pub arriving: f64,
}

impl ShoreMap {
    /// What the coast leaves at chart point `at` of a sea whose cascades keep `tables` at each fetch
    /// ([`band_fetch_table`]) and whose cascades and swell stand `heights` high on the open sea (m).
    pub fn sample(
        &self,
        at: [f64; 2],
        tables: &[[f64; FETCH_TABLE]],
        heights: [f64; 4],
    ) -> ShoreSample {
        let depth = self.heights.sample(at).map(|h| (-h).max(0.0));
        let wind_fetch = self.exposure.sample_channel(at, 0).unwrap_or(OPEN_FETCH_M);
        let swell_fetch = self.exposure.sample_channel(at, 1).unwrap_or(OPEN_FETCH_M);
        let mut scales = [1.0; 4];
        for (scale, table) in scales.iter_mut().zip(tables) {
            *scale = table_scale(table, wind_fetch);
        }
        scales[3] = swell_scale(swell_fetch);
        let arriving = scales
            .iter()
            .zip(heights)
            .map(|(s, h)| (s * h).powi(2))
            .sum::<f64>()
            .sqrt();
        if let Some(depth) = depth {
            for (scale, height) in scales.iter_mut().zip(heights) {
                *scale = scale.min(depth_scale(height, depth));
            }
        }
        ShoreSample {
            depth,
            wind_fetch,
            swell_fetch,
            scales,
            arriving,
        }
    }
}

impl ShoreGrid<[f32; 2]> {
    /// One channel of the grid at chart point `at`, as [`ShoreGrid::sample`].
    pub fn sample_channel(&self, at: [f64; 2], channel: usize) -> Option<f64> {
        self.sample_with(at, |k| self.values[k][channel])
    }
}

impl<T> ShoreGrid<T> {
    /// The grid's bilinear value at chart point `at`, reading sample `k` (row-major) with `value`; none off it.
    fn sample_with(&self, at: [f64; 2], value: impl Fn(usize) -> f32) -> Option<f64> {
        let gx = (at[0] - self.origin[0]) / self.cell;
        let gz = (at[1] - self.origin[1]) / self.cell;
        if !(gx >= 0.0
            && gz >= 0.0
            && gx <= (self.columns - 1) as f64
            && gz <= (self.rows - 1) as f64)
        {
            return None;
        }
        let i = (gx.floor() as usize).min(self.columns.saturating_sub(2));
        let j = (gz.floor() as usize).min(self.rows.saturating_sub(2));
        let (u, v) = (gx - i as f64, gz - j as f64);
        let read = |i: usize, j: usize| {
            f64::from(value(
                j.min(self.rows - 1) * self.columns + i.min(self.columns - 1),
            ))
        };
        let top = read(i, j) * (1.0 - u) + read(i + 1, j) * u;
        let bottom = read(i, j + 1) * (1.0 - u) + read(i + 1, j + 1) * u;
        Some(top * (1.0 - v) + bottom * v)
    }
}

/// The GPU half: the map's two grids as textures (height in metres and validity in `Rg16Float`; the fetch in kilometres,
/// `Rg16Float`), or a calm placeholder of one texel each while there is no coast.
pub struct ShoreTextures {
    pub heights: wgpu::TextureView,
    pub exposure: wgpu::TextureView,
}

impl ShoreTextures {
    /// One texel of each, zeroed: bound while no coast is set (the shader reads neither).
    pub fn open(device: &wgpu::Device) -> Self {
        let texture = |label: &str, format: wgpu::TextureFormat| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        Self {
            heights: texture("ocean shore heights", wgpu::TextureFormat::Rg16Float),
            exposure: texture("ocean shore exposure", wgpu::TextureFormat::Rg16Float),
        }
    }

    /// `map`'s grids.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, map: &ShoreMap) -> Self {
        let km: Vec<[f32; 2]> = map
            .exposure
            .values
            .iter()
            .map(|&[w, s]| [w / 1000.0, s / 1000.0])
            .collect();
        Self::upload(
            device,
            queue,
            (map.heights.columns as u32, map.heights.rows as u32),
            &map.heights.values,
            (map.exposure.columns as u32, map.exposure.rows as u32),
            &km,
        )
    }

    fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heights_size: (u32, u32),
        heights: &[f32],
        exposure_size: (u32, u32),
        exposure: &[[f32; 2]],
    ) -> Self {
        let halves = |values: &mut dyn Iterator<Item = f32>| -> Vec<u16> {
            values.map(|v| half::f16::from_f32(v).to_bits()).collect()
        };
        let texture = |label: &str,
                       size: (u32, u32),
                       format: wgpu::TextureFormat,
                       texel: u32,
                       data: &[u16]| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(data),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.0 * texel),
                    rows_per_image: Some(size.1),
                },
                wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
            );
            texture.create_view(&wgpu::TextureViewDescriptor::default())
        };
        // Filter the height numerator and valid weight together. Missing bed
        // samples then contribute zero to both, without adding fake depth.
        let heights = halves(&mut heights.iter().flat_map(|&h| {
            if h.is_finite() { [h, 1.0] } else { [0.0, 0.0] }
        }));
        let exposure = halves(&mut exposure.iter().flatten().copied());
        Self {
            heights: texture(
                "ocean shore heights",
                heights_size,
                wgpu::TextureFormat::Rg16Float,
                4,
                &heights,
            ),
            exposure: texture(
                "ocean shore exposure",
                exposure_size,
                wgpu::TextureFormat::Rg16Float,
                4,
                &exposure,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shore_height_filter_excludes_missing_columns() {
        let heights = ShoreGrid {
            origin: [0.0; 2],
            cell: 1.0,
            columns: 2,
            rows: 2,
            values: vec![-2.0, f32::NAN, 4.0, f32::NAN],
        };
        assert_eq!(heights.sample([0.25, 0.5]), Some(1.0));
        assert_eq!(heights.sample([0.5, 0.0]), Some(-2.0));
        assert_eq!(heights.sample([0.75, 0.5]), None);
    }

    /// A straight coast along z at x ≤ `beach` (land), sea beyond, on a 101 × 41 grid of 100 m cells.
    fn coast(beach: usize) -> (Vec<bool>, usize, usize) {
        let (columns, rows) = (101, 41);
        let land = (0..rows)
            .flat_map(|_| (0..columns).map(move |i| i <= beach))
            .collect();
        (land, columns, rows)
    }

    #[test]
    fn the_open_water_upwind_runs_back_to_the_land_or_out_to_the_open_sea() {
        let (land, columns, rows) = coast(10);
        // An offshore wind (blowing toward +x): every sea sample's water runs back to the beach.
        let offshore = upwind_water(&land, columns, rows, 100.0, 0.0, OPEN_FETCH_M);
        for i in [11, 20, 60, 100] {
            assert!((offshore[20 * columns + i] - (i as f32 - 10.0) * 100.0).abs() < 1e-2);
        }
        assert_eq!(offshore[20 * columns + 5], 0.0, "land behind land has none");
        assert_eq!(offshore[20 * columns + 10], 0.0, "nor a lee beach");
        // An onshore wind: the line leaves the grid over the sea, the open sea's; the beach holds the sea that runs onto
        // it, the land behind none.
        let onshore = upwind_water(
            &land,
            columns,
            rows,
            100.0,
            std::f64::consts::PI,
            OPEN_FETCH_M,
        );
        assert_eq!(onshore[20 * columns + 50], OPEN_FETCH_M as f32);
        assert_eq!(onshore[20 * columns + 10], OPEN_FETCH_M as f32);
        assert_eq!(onshore[20 * columns + 9], 0.0);
        // A beach 2 km downwind of another: the water runs on onto it.
        let mut strait = land.clone();
        for j in 0..rows {
            strait[j * columns + 31] = true;
        }
        let across = upwind_water(&strait, columns, rows, 100.0, 0.0, OPEN_FETCH_M);
        assert!((across[20 * columns + 31] - 2100.0).abs() < 1e-2);
        assert!(
            (across[20 * columns + 32] - 100.0).abs() < 1e-2,
            "the water behind it starts again"
        );
        // At an angle, the slant distance to the beach: 1 / cos 30° of the square one.
        let slant = upwind_water(
            &land,
            columns,
            rows,
            100.0,
            std::f64::consts::FRAC_PI_6,
            OPEN_FETCH_M,
        );
        let expected = 30.0 * 100.0 / std::f64::consts::FRAC_PI_6.cos();
        let got = f64::from(slant[30 * columns + 40]);
        assert!((got - expected).abs() < 1.0, "{got} against {expected}");
        // Swept along z the same.
        let along_z = upwind_water(&land, columns, rows, 100.0, 1.2, OPEN_FETCH_M);
        let expected = 30.0 * 100.0 / 1.2f64.cos();
        let got = f64::from(along_z[2 * columns + 40]);
        // The line leaves the grid's edge (z < 0) before it reaches the beach: the open sea.
        assert_eq!(got, OPEN_FETCH_M, "{expected}");
    }

    #[test]
    fn the_effective_fetch_of_an_open_sea_is_the_open_seas_and_shorter_off_a_lee_shore() {
        let (columns, rows) = (64, 64);
        let open = vec![false; columns * rows];
        let fetch = effective_fetch(&open, columns, rows, 100.0, 0.4, &FETCH_BEARINGS);
        assert!(fetch.iter().all(|&f| (f - OPEN_FETCH_M as f32).abs() < 1.0));
        let (land, columns, rows) = coast(10);
        let offshore = effective_fetch(&land, columns, rows, 100.0, 0.0, &FETCH_BEARINGS);
        let straight = upwind_water(&land, columns, rows, 100.0, 0.0, OPEN_FETCH_M);
        let at = 20 * columns + 30;
        // The slanting bearings run further to the same beach.
        assert!(offshore[at] > straight[at] && offshore[at] < straight[at] * 1.3);
    }

    #[test]
    fn the_fetch_law_keeps_short_waves_in_the_lee_and_the_open_sea_whole() {
        let wind = 9.0;
        // The calibrated sea's peak, 32 m at 9 m/s, is a fetch of about 46 km.
        assert!((fetch_peak_wavelength(wind, 46_000.0) - 32.0).abs() < 1.5);
        for wavelength in [1.5, 10.0, 32.0, 128.0] {
            let q = fetch_coefficient(wavelength, wind);
            assert!((fetch_scale(q, OPEN_FETCH_M) - 1.0).abs() < 1e-9);
            assert!((fetch_scale(q, 2.0 * OPEN_FETCH_M) - 1.0).abs() < 1e-9);
        }
        // A kilometre off a lee shore the chop stands; the sea's peak is not yet raised.
        let short = fetch_scale(fetch_coefficient(1.5, wind), 1000.0);
        let peak = fetch_scale(fetch_coefficient(32.0, wind), 1000.0);
        assert!(short > 0.75 && peak < 1e-3, "{short} {peak}");
        // Longer fetch, more of every wave.
        let q = fetch_coefficient(10.0, wind);
        assert!(fetch_scale(q, 2000.0) < fetch_scale(q, 10_000.0));
        assert_eq!(fetch_coefficient(32.0, 0.0), 0.0);
    }

    #[test]
    fn the_swell_fills_a_lands_shadow_and_the_depth_breaks_the_waves() {
        assert_eq!(swell_scale(0.0), 0.0);
        assert!((swell_scale(OPEN_FETCH_M) - 1.0).abs() < 1e-12);
        assert!(swell_scale(SWELL_SHELTER_M) > 0.6 && swell_scale(SWELL_SHELTER_M) < 0.7);
        assert_eq!(depth_scale(2.0, 100.0), 1.0);
        assert!((depth_scale(2.0, 2.0) - BREAKING).abs() < 1e-12);
        assert_eq!(depth_scale(2.0, -1.0), 0.0);
    }

    #[test]
    fn a_map_takes_the_land_and_a_coarser_exposure_from_its_heights() {
        let (columns, rows) = (41, 81);
        let heights: Vec<f32> = (0..rows)
            .flat_map(|_| (0..columns).map(|i| if i < 8 { 12.0 } else { -20.0 }))
            .collect();
        let map = ShoreMap::build(heights, columns, rows, 40.0, [1000.0, -400.0], 0.0);
        assert_eq!((map.exposure.columns, map.exposure.rows), (11, 21));
        assert_eq!(map.exposure.cell, 160.0);
        // The coarse samples 0 and 1 are land; five cells off the beach the water runs back 800 m on the wind's own
        // bearing, further on the slanting ones.
        let [wind, swell] = map.exposure.values[10 * 11 + 6];
        assert!(wind > 850.0 && wind < 950.0, "{wind}");
        assert!(swell > 790.0 && swell < 830.0, "{swell}");
        assert_eq!(map.exposure.values[10 * 11 + 1], [0.0, 0.0]);
        let frame = map.heights.place().frame([10.0, 20.0]);
        assert_eq!(&frame[..2], &[990.0, -400.0]);
        assert!((f64::from(frame[2]) * 1640.0 - 1.0).abs() < 1e-6);
        assert!((f64::from(frame[3]) * 3240.0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_band_keeps_its_short_waves_share_where_the_fetch_takes_its_long_ones() {
        let k = |wavelength: f64| (2.0 * std::f64::consts::PI / wavelength) as f32;
        // Two modes of equal energy, 2 m and 20 m, at 9 m/s.
        let modes = [k(2.0), 0.0, 1.0, 0.0, k(20.0), 1.0];
        let table = band_fetch_table(&modes, 9.0);
        assert!(
            (table[FETCH_TABLE - 1] - 1.0).abs() < 1e-9,
            "the open sea's whole"
        );
        // Two kilometres off a lee shore the 20 m waves are gone and the 2 m ones stand: half the energy, 0.71 of the
        // amplitude.
        let at = table_scale(&table, 2000.0);
        let short = fetch_scale(fetch_coefficient(2.0, 9.0), 2000.0);
        let long = fetch_scale(fetch_coefficient(20.0, 9.0), 2000.0);
        let exact = ((short * short + long * long) / 2.0).sqrt();
        assert!((at - exact).abs() < 0.02, "{at} against {exact}");
        assert!(at > 0.6 && at < 0.75);
        // Monotone in fetch, and whole with no modes or no wind.
        assert!(table.windows(2).all(|w| w[0] <= w[1] + 1e-12));
        assert!(band_fetch_table(&[], 9.0).iter().all(|&v| v == 1.0));
        assert!(band_fetch_table(&modes, 0.0).iter().all(|&v| v == 1.0));
        assert_eq!(table_scale(&table, 1e9), 1.0);
        assert!((table_fetch(0) - FETCH_LEAST_M).abs() < 1e-9);
        assert!((table_fetch(FETCH_TABLE - 1) - OPEN_FETCH_M).abs() < 1e-6);
    }
}
