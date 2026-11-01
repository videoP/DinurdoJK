//! Native river foam field producer: cached Grid2D_SW_River stages 4 (Init
//! Foam, OnSimulationReset) and 16 (Foam, Always). CPU reference arithmetic.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
//! Evidence: foam-field-reference.json (bindings, sizing, defaults, placement)
//! and foam-reference.json (stage bodies, Random.ush/NiagaraSimplexNoise.cpp).
//! The selected FoamGrid output is also the FoamRT value: Grid2D_InitRT003
//! sizes FoamRT from FoamGrid and stage 16 writes the same half value to both.
use super::control::Grid;

const LIBRARY: &str = include_str!("foam_field.wgsl");
const STAGE: &str = include_str!("foam_field_stage.wgsl");

/// Pure helper functions, for production kernels and independent diagnostics.
pub fn library() -> &'static str {
    LIBRARY
}
pub fn shader() -> String {
    [LIBRARY, STAGE].join("\n")
}

/// Selected stage-16 inputs. Dissipation is the system's exposed
/// User.FoamDissipationRate default; the actor does not override it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub dissipation_rate: f32,
    pub advection_multiplier: f32,
    pub noise_strength: f32,
    pub noise_frequency: f32,
    pub noise_time_scale: f32,
    /// UNiagaraDataInterfaceCurlNoise constructor seed; neither DI overrides it.
    pub curl_seed: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            dissipation_rate: 2.5,
            advection_multiplier: 1.0,
            noise_strength: 60.0,
            noise_frequency: 0.004,
            noise_time_scale: 1.0,
            curl_seed: 0,
        }
    }
}

/// Grid2D_SW_EmissionCriteria ranges/multipliers wired in stage 16.
pub const VORTICITY: ([f32; 2], f32) = ([1.0, 3.0], 1.0);
pub const CURVATURE: ([f32; 2], f32) = ([0.002, 0.05], 5.0);
pub const VELOCITY: ([f32; 2], f32) = ([800.0, 1000.0], 0.0);
pub const DEPTH_DIFFERENCE: ([f32; 2], f32) = ([0.0, 0.0], 0.0);
pub const WATER_DEPTH: ([f32; 2], f32) = ([150.0, 500.0], 1.0);
pub const LOW_VELOCITY: ([f32; 2], f32) = ([50.0, 200.0], 1.0);
pub const LOCAL_PIVOT: [f32; 2] = [0.5, 0.5];
const LWC_TILE_SIZE: f64 = 2097152.0;

/// Engine.Owner placement used by Grid2D_CreateUnitToWorldTransform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Owner position in centimetres within the system's LWC tile.
    pub owner_position: [f32; 3],
    /// FQuat4f x, y, z, w.
    pub owner_rotation: [f32; 4],
    /// Emitter.Grid2D_SetResolution.WorldGridExtents.
    pub world_grid_extents: [f32; 2],
}

impl Placement {
    /// Unrotated actor at `origin` (cm). Niagara subtracts the LWC tile from
    /// the owner position; the tile is zero within +/-1048576 cm. A non-zero
    /// tile would change the noise input, so it is rejected, not ignored.
    pub fn unrotated(origin: [f64; 3], grid: &Grid) -> Result<Self, &'static str> {
        if origin.iter().any(|v| !v.is_finite()) {
            return Err("invalid river foam owner position");
        }
        if origin
            .iter()
            .any(|v| (v / LWC_TILE_SIZE + 0.5).floor() != 0.0)
        {
            return Err("river foam owner requires a non-zero Niagara LWC tile");
        }
        Ok(Self {
            owner_position: origin.map(|v| v as f32),
            owner_rotation: [0.0, 0.0, 0.0, 1.0],
            world_grid_extents: grid.world_size,
        })
    }

    /// Native row-vector matrix: mul(mul(mul(mul(TLocal,S),RLocal),R),T).
    /// TLocal is (-pivot.x,-pivot.y,-pivot.y): the source repeats Y in Z.
    pub fn unit_to_world(&self) -> [[f32; 4]; 4] {
        let p = LOCAL_PIVOT.map(|v| v * -1.0);
        let local = translation([p[0], p[1], p[1]]);
        let [ex, ey] = self.world_grid_extents;
        let scale = [
            [ex, 0.0, 0.0, 0.0],
            [0.0, ey, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let identity = translation([0.0; 3]);
        let rotation = quaternion_matrix(self.owner_rotation);
        mul(
            mul(mul(mul(local, scale), identity), rotation),
            translation(self.owner_position),
        )
    }
}

fn translation(t: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j] + a[i][3] * b[3][j])
    })
}

// Grid2D_CreateUnitToWorldTransform's quaternion-to-matrix custom HLSL.
fn quaternion_matrix(q: [f32; 4]) -> [[f32; 4]; 4] {
    let [x, y, z, w] = q;
    let (x2, y2, z2) = (x + x, y + y, z + z);
    let (xx, xy, xz) = (x * x2, x * y2, x * z2);
    let (yy, yz, zz) = (y * y2, y * z2, z * z2);
    let (wx, wy, wz) = (w * x2, w * y2, w * z2);
    let mut m = [[0.0; 4]; 4];
    m[0][0] = 1.0 - (yy + zz);
    m[1][0] = xy - wz;
    m[2][0] = xz + wy;
    m[0][1] = xy + wz;
    m[1][1] = 1.0 - (xx + zz);
    m[2][1] = yz - wx;
    m[0][2] = xz - wy;
    m[1][2] = yz + wx;
    m[2][2] = 1.0 - (xx + yy);
    m[3][3] = 1.0;
    m
}

/// ShallowWaterRiverActor's User.SimRes: double arithmetic, SMALL_NUMBER
/// (float 1e-8) tolerance, FloorToInt, then SetVariableVec2's float cast.
pub fn native_sim_res(world_grid_size: [f64; 2], resolution_max_axis: i32) -> [f32; 2] {
    let [x, y] = world_grid_size;
    let axis = f64::from(resolution_max_axis);
    let cell = x.max(y) / axis;
    let mut res = [axis, (axis * y / x).floor()];
    if y > x {
        res = [(axis * x / y).floor(), axis];
    }
    let small = f64::from(1.0e-8f32);
    if x > y && (cell * res[1] - y).abs() > small {
        res[1] += 1.0;
    }
    if x < y && (cell * res[0] - x).abs() > small {
        res[0] += 1.0;
    }
    res.map(|v| v as f32)
}

/// Fixed per-river inputs of the stage. `dx` is Emitter.dx (emission);
/// `foam_dx` is MakeFloatFromVector2d(SetResolution005.WorldCellSize).X.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setup {
    pub dimensions: [u32; 2],
    pub dx: f32,
    pub foam_dx: f32,
    pub sim_res: [f32; 2],
    pub unit_to_world: [[f32; 4]; 4],
    pub settings: Settings,
}

impl Setup {
    /// FoamGrid copies GridCollection's cells (resolution mult one) and the
    /// main grid extents, so its X cell size is the solver's X cell size.
    pub fn native(grid: &Grid, placement: Placement, sim_res: [f32; 2]) -> Self {
        Self {
            dimensions: grid.dimensions,
            dx: grid.cell_size[0],
            foam_dx: grid.cell_size[0],
            sim_res,
            unit_to_world: placement.unit_to_world(),
            settings: Settings::default(),
        }
    }
    /// Niagara Grid2D UnitToUV, also the ExecutionIndexToUnit reciprocal.
    pub fn unit_to_uv(&self) -> [f32; 2] {
        self.dimensions.map(|n| 1.0 / n as f32)
    }
    /// Static 128-byte uniform prefix; `Frame::uniform` supplies the tail.
    pub fn uniform(&self) -> [f32; 32] {
        let s = self.settings;
        let offset = offset_from_seed(s.curl_seed);
        let uv = self.unit_to_uv();
        let mut out = [0.0; 32];
        for (row, values) in self.unit_to_world.iter().enumerate() {
            out[row * 4..row * 4 + 4].copy_from_slice(values);
        }
        out[16..20].copy_from_slice(&[offset[0], offset[1], offset[2], s.noise_strength]);
        out[20..24].copy_from_slice(&[
            s.noise_frequency,
            s.noise_time_scale,
            s.advection_multiplier,
            s.dissipation_rate,
        ]);
        out[24..28].copy_from_slice(&[self.foam_dx, uv[0], uv[1], 0.0]);
        out[28..32].copy_from_slice(&[self.sim_res[0], self.sim_res[1], self.dx, 0.0]);
        out
    }
}

/// Engine.DeltaTime and Engine.Time (float(World->TimeSeconds)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub delta: f32,
    pub time: f32,
}
impl Frame {
    pub fn new(delta_seconds: f32, world_time_seconds: f64) -> Self {
        Self {
            delta: delta_seconds,
            time: world_time_seconds as f32,
        }
    }
    pub fn uniform(self) -> [f32; 4] {
        [self.delta, self.time, 0.0, 0.0]
    }
}

/// RandomPCG.ush / NiagaraSimplexNoise.cpp Rand3DPCG16.
pub fn rand3d_pcg16(p: [i32; 3]) -> [u32; 3] {
    let mut v = p.map(|c| (c as u32).wrapping_mul(1664525).wrapping_add(1013904223));
    for _ in 0..2 {
        v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
        v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
        v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    }
    v.map(|c| c >> 16)
}

/// NiagaraSimplexNoise.cpp SimplexNoiseOffsetFromSeed (CPU, uploaded as a DI parameter).
pub fn offset_from_seed(seed: u32) -> [f32; 3] {
    rand3d_pcg16([seed as i32; 3]).map(|v| v as i32 as f32 * (1.0f32 / 100.0f32))
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

// Random.ush SimplexCorners.
fn simplex_corners(v: [f32; 3]) -> [[f32; 3]; 4] {
    let tet = v.map(|c| (c + v[0] / 3.0 + v[1] / 3.0 + v[2] / 3.0).floor());
    let base = tet.map(|c| c - tet[0] / 6.0 - tet[1] / 6.0 - tet[2] / 6.0);
    let f: [f32; 3] = std::array::from_fn(|i| v[i] - base[i]);
    let step = |edge: f32, x: f32| if x >= edge { 1.0f32 } else { 0.0 };
    let g = [step(f[1], f[0]), step(f[2], f[1]), step(f[0], f[2])];
    let h = [1.0 - g[2], 1.0 - g[0], 1.0 - g[1]];
    let a1: [f32; 3] = std::array::from_fn(|i| g[i].min(h[i]) - 1.0 / 6.0);
    let a2: [f32; 3] = std::array::from_fn(|i| g[i].max(h[i]) - 1.0 / 3.0);
    [
        base,
        std::array::from_fn(|i| base[i] + a1[i]),
        std::array::from_fn(|i| base[i] + a2[i]),
        base.map(|c| c + 0.5),
    ]
}

/// Random.ush JacobianSimplex_ALU with bTiling=false: rows are the three
/// noise channels, columns d/dx, d/dy, d/dz and value.
pub fn jacobian_simplex(v: [f32; 3]) -> [[f32; 4]; 3] {
    const SCALE: f32 = 1024.0 / 375.0;
    let corners = simplex_corners(v);
    let fv: [[f32; 3]; 4] = std::array::from_fn(|c| std::array::from_fn(|i| v[i] - corners[c][i]));
    let mut gvec = [[[0.0f32; 3]; 4]; 3];
    let mut grad = [[0.0f32; 4]; 3];
    for c in 0..4 {
        let cell = corners[c].map(|t| (6.0 * t + 0.5).floor() as i32);
        let rand = rand3d_pcg16(cell);
        for a in 0..3 {
            gvec[a][c] = [
                (rand[a] & 0x8000) as f32 * (1.0 / 16384.0) - 1.0,
                (rand[a] & 0x4000) as f32 * (1.0 / 8192.0) - 1.0,
                (rand[a] & 0x2000) as f32 * (1.0 / 4096.0) - 1.0,
            ];
            grad[a][c] = dot3(gvec[a][c], fv[c]);
        }
    }
    let s = fv.map(|f| (2.0 * dot3(f, f)).clamp(0.0, 1.0));
    let sv = s.map(|s| 1.0 * SCALE + s * (-3.0 * SCALE + s * (3.0 * SCALE - s * SCALE)));
    let ds_scale = s.map(|s| -12.0 * SCALE + s * (24.0 * SCALE - s * 12.0 * SCALE));
    std::array::from_fn(|a| {
        let mut row = [0.0; 4];
        for axis in 0..3 {
            let smooth = (0..4).fold(0.0, |sum, c| sum + sv[c] * gvec[a][c][axis]);
            let derivative = (0..4).fold(0.0, |sum, c| sum + ds_scale[c] * fv[c][axis] * grad[a][c]);
            row[axis] = smooth + derivative;
        }
        row[3] = (0..4).fold(0.0, |sum, c| sum + sv[c] * grad[a][c]);
        row
    })
}

/// SampleNoiseField for UNiagaraDataInterfaceCurlNoise.
pub fn curl(xyz: [f32; 3], offset_from_seed: [f32; 3]) -> [f32; 3] {
    let j = jacobian_simplex(std::array::from_fn(|i| xyz[i] + offset_from_seed[i]));
    [j[1][2] - j[2][1], j[2][0] - j[0][2], j[0][1] - j[1][0]]
}

/// Grid2D_Foam_Advect NoiseOffset for one execution unit.
pub fn noise_offset(setup: &Setup, frame: Frame, unit: [f32; 2]) -> [f32; 2] {
    let s = setup.settings;
    let m = setup.unit_to_world;
    let world: [f32; 3] = std::array::from_fn(|j| unit[0] * m[0][j] + unit[1] * m[1][j] + 0.0 * m[2][j] + m[3][j]);
    let time = s.noise_time_scale * frame.time;
    let xyz = world.map(|w| w * s.noise_frequency + time);
    let value = curl(xyz, offset_from_seed(s.curl_seed)).map(|c| c * s.noise_strength);
    [value[0], value[1]]
}

/// Native ExecutionIndexToUnit for a 2D dispatch.
pub fn execution_unit(setup: &Setup, index: [u32; 2]) -> [f32; 2] {
    let uv = setup.unit_to_uv();
    [
        (index[0] as f32 + 0.5) * uv[0],
        (index[1] as f32 + 0.5) * uv[1],
    ]
}

/// FoamGrid advection: SamplePreviousGridFloatValue unit for this cell.
/// `velocity` is the VelocityReader sample at the execution unit.
pub fn advection_unit(setup: &Setup, frame: Frame, index: [u32; 2], velocity: [f32; 2]) -> [f32; 2] {
    let noise = noise_offset(setup, frame, execution_unit(setup, index));
    let velocity = [velocity[0] + noise[0], velocity[1] + noise[1]];
    let scale = setup.settings.advection_multiplier * frame.delta / setup.foam_dx;
    let uv = setup.unit_to_uv();
    std::array::from_fn(|i| (index[i] as f32 - scale * velocity[i] + 0.5) * uv[i])
}

/// HLSL smoothstep under D3D semantics. D3D saturate maps NaN to zero; this
/// writes the zero-width range's defined result explicitly because WGSL may
/// assume NaN/infinity absence. Vulkan leaves such smoothstep undefined.
pub fn criterion(value: f32, range: [f32; 2], multiplier: f32) -> f32 {
    let width = range[1] - range[0];
    let t = if width == 0.0 {
        if value - range[0] > 0.0 {
            1.0
        } else {
            0.0
        }
    } else {
        ((value - range[0]) / width).clamp(0.0, 1.0)
    };
    t * t * (3.0 - 2.0 * t) * multiplier
}

/// Grid2D_SW_SampleSecondaryEmission outputs consumed by the emission graph.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EmissionInputs {
    pub curl: f32,
    pub curvature: f32,
    pub velocity: [f32; 2],
    pub depth_difference: f32,
    pub water_depth: f32,
}

/// Previous-buffer Water/Velocity/Normal grid samplers (bilinear, clamp).
pub trait Fields {
    fn depth(&self, unit: [f32; 2]) -> f32;
    fn velocity(&self, unit: [f32; 2]) -> [f32; 2];
    fn normal(&self, unit: [f32; 2]) -> [f32; 3];
}

/// Sampled from grid (SampleFromGrid=true, SampleFromVT=false).
pub fn emission_inputs(setup: &Setup, frame: Frame, unit: [f32; 2], fields: &impl Fields) -> EmissionInputs {
    let mut out = EmissionInputs::default();
    let udx = setup.sim_res.map(|r| 1.0 / r);
    // Native `float ValidRange = 2. * UnitDX;` keeps only X, also for Y.
    let valid = 2.0 * udx[0];
    if !(unit[0] > valid && unit[0] < 1.0 - valid && unit[1] > valid && unit[1] < 1.0 - valid) {
        return out;
    }
    out.water_depth = fields.depth(unit);
    if out.water_depth > 1e-5 {
        let at = |x: f32, y: f32| [unit[0] + x * udx[0], unit[1] + y * udx[1]];
        let vx_1 = fields.velocity(at(0.0, 1.0))[0];
        let vx_m1 = fields.velocity(at(0.0, -1.0))[0];
        let vy_1 = fields.velocity(at(1.0, 0.0))[1];
        let vy_m1 = fields.velocity(at(-1.0, 0.0))[1];
        out.curl = (((vx_1 - vx_m1) - (vy_1 - vy_m1)) / (2.0 * setup.dx)).abs();
        out.velocity = [0.5 * (vx_1 + vx_m1), 0.5 * (vy_1 + vy_m1)];
        let n = fields.normal(at(0.0, 0.0));
        let right = fields.normal(at(1.0, 0.0));
        let up = fields.normal(at(0.0, 1.0));
        out.curvature = (((1.0 - dot3(n, right)) + (1.0 - dot3(n, up))) * 0.5).abs();
        let next: [f32; 2] =
            std::array::from_fn(|i| unit[i] + out.velocity[i] * frame.delta * udx[i] / setup.dx);
        out.depth_difference = fields.depth(next) - out.water_depth;
    }
    out
}

/// Grid2D_Secondary_EmissionPoints001 EmitAmount (all selected criteria).
pub fn emit_amount(e: EmissionInputs) -> f32 {
    let speed = (e.velocity[0] * e.velocity[0] + e.velocity[1] * e.velocity[1]).sqrt();
    let c = |v: f32, (range, mult): ([f32; 2], f32)| criterion(v, range, mult);
    let strongest = c(e.curl, VORTICITY)
        .max(c(e.curvature, CURVATURE))
        .max(c(speed, VELOCITY))
        .max(c(e.depth_difference, DEPTH_DIFFERENCE));
    (1.0 * strongest * c(e.water_depth, WATER_DEPTH) * c(speed, LOW_VELOCITY)).clamp(0.0, 1.0)
}

/// Add_Float then DissipateFloat (subtraction amount zero, clamp 0..1e9).
/// The result is stored as half into FoamGrid and FoamRT.
pub fn store_value(setup: &Setup, frame: Frame, advected: f32, emit: f32) -> f32 {
    let foam = advected + emit;
    let divided = foam / (frame.delta * setup.settings.dissipation_rate + 1.0);
    (divided - 0.0 * frame.delta).clamp(0.0, 1e9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcg_seed_offset_matches_native_integer_steps() {
        // Independent wrapping arithmetic for seed zero.
        let mut v = [1013904223u32; 3];
        for _ in 0..2 {
            v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
            v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
            v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
        }
        let expected = v.map(|c| (c >> 16) as f32 * 0.01f32);
        assert_eq!(offset_from_seed(0), expected);
        assert_eq!(rand3d_pcg16([-1, 0, 7]), rand3d_pcg16([-1, 0, 7]));
        assert_ne!(rand3d_pcg16([-1, 0, 7]), rand3d_pcg16([1, 0, 7]));
    }

    #[test]
    fn simplex_jacobian_matches_finite_differences_and_is_divergence_free_curl() {
        for v in [[0.13f32, 0.71, -0.42], [12.6, -3.3, 7.25], [640.25, 512.5, 3.75]] {
            let j = jacobian_simplex(v);
            let h = if v[0].abs() > 100.0 { 1e-2 } else { 1e-3 };
            for axis in 0..3 {
                let mut a = v;
                let mut b = v;
                a[axis] += h;
                b[axis] -= h;
                let (ja, jb) = (jacobian_simplex(a), jacobian_simplex(b));
                for channel in 0..3 {
                    let numeric = (ja[channel][3] - jb[channel][3]) / (2.0 * h);
                    assert!((numeric - j[channel][axis]).abs() < 0.05 * (1.0 + numeric.abs()), "{v:?} {axis} {channel}");
                }
            }
            for row in j {
                assert!(row[3].abs() <= 1.0 + 1e-3);
            }
        }
    }

    #[test]
    fn zero_width_criteria_follow_d3d_saturate_and_contribute_nothing() {
        for x in [-5.0, 0.0, 5.0] {
            assert_eq!(criterion(x, [0.0, 0.0], 0.0), 0.0);
        }
        assert_eq!(criterion(5.0, [0.0, 0.0], 1.0), 1.0);
        assert_eq!(criterion(0.0, [0.0, 0.0], 1.0), 0.0);
        assert_eq!(criterion(2.0, [1.0, 3.0], 1.0), 0.5);
        let e = EmissionInputs {
            curl: 2.0,
            curvature: 0.01,
            velocity: [900.0, 0.0],
            depth_difference: 0.0,
            water_depth: 300.0,
        };
        // Velocity range 800..1000 weighs zero; slow-flow gate is saturated.
        let expected = criterion(2.0, VORTICITY.0, 1.0).max(criterion(0.01, CURVATURE.0, 5.0)) * criterion(300.0, WATER_DEPTH.0, 1.0);
        assert_eq!(emit_amount(e), expected.clamp(0.0, 1.0));
    }

    #[test]
    fn unit_to_world_follows_native_composition_including_pivot_z() {
        let grid = Grid::max_axis([1000.0, 400.0], 50).unwrap();
        let placement = Placement::unrotated([1200.0, -800.0, 40.0], &grid).unwrap();
        let m = placement.unit_to_world();
        let w = |u: [f32; 2]| std::array::from_fn::<f32, 3, _>(|j| u[0] * m[0][j] + u[1] * m[1][j] + m[3][j]);
        assert_eq!(w([0.5, 0.5]), [1200.0, -800.0, 39.5]);
        assert_eq!(w([0.0, 0.0]), [700.0, -1000.0, 39.5]);
        let turned = Placement {
            owner_rotation: [0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2],
            ..placement
        };
        let m = turned.unit_to_world();
        // Unreal +90 degree yaw maps local +X to world +Y.
        assert!((m[0][1] - 1000.0).abs() < 1e-3 && m[0][0].abs() < 1e-3);
        assert!(Placement::unrotated([1.1e6, 0.0, 0.0], &grid).is_err());
    }

    #[test]
    fn sim_res_matches_native_grid_padding() {
        for (size, axis) in [([1000.0, 400.0], 50), ([400.0, 1000.0], 50), ([1280.0, 602.5], 512), ([100.0, 37.0], 16)] {
            let grid = Grid::max_axis([size[0] as f32, size[1] as f32], axis as u32).unwrap();
            assert_eq!(native_sim_res(size, axis), grid.dimensions.map(|n| n as f32), "{size:?}");
        }
    }

    #[test]
    fn dissipation_and_emission_use_native_order() {
        let grid = Grid::max_axis([100.0, 100.0], 10).unwrap();
        let setup = Setup::native(&grid, Placement::unrotated([0.0; 3], &grid).unwrap(), [10.0, 10.0]);
        let frame = Frame::new(0.1, 3.0);
        assert_eq!(store_value(&setup, frame, 0.5, 0.25), 0.75 / (0.1 * 2.5 + 1.0));
        assert_eq!(store_value(&setup, frame, -1.0, 0.0), 0.0);
        let unit = execution_unit(&setup, [0, 0]);
        assert_eq!(unit, [0.05, 0.05]);
        let a = advection_unit(&setup, frame, [4, 5], [0.0, 0.0]);
        let noise = noise_offset(&setup, frame, execution_unit(&setup, [4, 5]));
        assert!((a[0] - (4.5 - 0.1 / 10.0 * noise[0]) / 10.0).abs() < 1e-6);
    }
}
