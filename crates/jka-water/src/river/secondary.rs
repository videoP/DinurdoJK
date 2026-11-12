//! Native river whitewater: Grid2D_SW_River's `SW_River_Secondary_Emitter`
//! (WaterAdvanced, UE 5.8.1). CPU reference arithmetic and the WGSL update
//! kernel (`secondary.wgsl`). Original algorithms Copyright Epic Games, Inc.
//! All Rights Reserved.
//!
//! Evidence (2026-11-11, `Development Docs/Patch20/river-reference`):
//! - Stage structure: `SW_River_Secondary_Emitter.0.generated.hlsl`, a GPU
//!   ParticleSpawnUpdate stage. Its baked constants are module DEFAULTS (for
//!   example EmissionMult 0, depth range 50..999999, SimRes 0), not the
//!   asset's, so values come from the saved asset instead:
//! - `Grid2D_SW_River.uasset` UpdateScript_2_2/SpawnScript_2_2
//!   RapidIterationParameters, the emitter stack graph (linked inputs: SimRes
//!   <- User.SimRes, SampleFromGrid <- System.RunLiveSim) and the scratch
//!   module graphs (Sample_Water, Emission_Criterias, WhiteWater...), which
//!   match the HLSL bodies.
//! - One SpawnBurst_Instantaneous of 50000 (EmitterState Loop Behavior Once,
//!   no distance culling), infinitely lived looping 0.5 s particles recycled
//!   by the ViewRecycler module (frustum volume sampling, 110 degree maximum
//!   FOV, near 0..500, far 0..10000 cm per particle).
//! - Renderer: NiagaraSpriteRendererProperties_3 (MI_SimpleSpray, enabled by
//!   Emitter.RenderTranslucent = UseTranslucentSpray(true) && User.RenderSecondary;
//!   ShallowWaterRiverActor.cpp sets RenderSecondary for LiveSim). Defaults
//!   otherwise: FaceCamera, unaligned, renderer visibility tag 0.
//! - Curves: the data interfaces' baked ShaderLUTs (ALPHA_LUT, SIZE_LUT).
//!
//! Not ported (labelled): mesh-only modules (UpdateMeshOrientation001,
//! ScaleMeshSize001, mesh index/scale spawn); Emission_Criterias' sink-box
//! culling (no User.Sink* arrays are supplied); random terms whose asset
//! multipliers are zero; TAA jitter (zero). The depth gate uses the river's
//! authored foam minimum depth like the foam field (see
//! `foam_field::authored_depth_range`); at the default it is the native
//! 150..500 cm.
use super::foam_field::{self, EmissionInputs};

const COMMON: &str = include_str!("secondary_common.wgsl");
const KERNEL: &str = include_str!("secondary.wgsl");

/// Bytes per particle (`RiverSprayParticle`: five vec4).
pub const PARTICLE_BYTES: u64 = 80;

/// SpawnBurst_Instantaneous Spawn Count (fired once).
pub const PARTICLE_COUNT: u32 = 50_000;
/// InitializeParticle Lifetime; ViewRecycler resets to Initial.Lifetime.
pub const LIFETIME: f32 = 0.5;
/// InitializeParticle Sprite Size Mode Random Uniform, Min/Max (cm).
pub const SPRITE_SIZE_RANGE: [f32; 2] = [30.0, 75.0];
/// InitializeParticle Sprite Rotation Mode Random, degrees.
pub const SPRITE_ROTATION_RANGE: [f32; 2] = [0.0, 360.0];
/// RandomRangeFloat005 -> SpriteRotationRate001, degrees per second.
pub const ROTATION_RATE_RANGE: [f32; 2] = [-180.0, 180.0];
/// RandomRangeVector2D -> ViewRecycler Near / Far Distance (Minimum 0,0).
pub const NEAR_FAR_MAXIMUM: [f32; 2] = [500.0, 10000.0];
/// ViewRecycler Maximum FOV Angle (degrees).
pub const MAXIMUM_FOV_DEGREES: f32 = 110.0;
pub const REJECTION_SAMPLES: i32 = 1;
pub const GRAVITY: [f32; 3] = [0.0, 0.0, -980.0];
/// WhiteWaterVelocityAndForce Bubble Buoyancy, replaces the force under water.
pub const BUBBLE_BUOYANCY: [f32; 3] = [0.0, 0.0, 100.0];
pub const HIDDEN_VISIBILITY_TAG: i32 = -999;

/// Emission_Criterias rapid-iteration values: (range, multiplier).
pub const VORTICITY: ([f32; 2], f32) = ([1.0, 3.0], 0.0);
pub const VELOCITY: ([f32; 2], f32) = ([0.0, 0.0], 0.0);
pub const CURVATURE: ([f32; 2], f32) = ([0.001, 0.05], 5.0);
pub const DEPTH_DIFFERENCE: ([f32; 2], f32) = ([0.0, 0.0], 0.0);
pub const RESTRICT_DEPTH: [f32; 2] = [150.0, 500.0];
pub const RESTRICT_SPEED: [f32; 2] = [50.0, 200.0];
pub const EMISSION_MULT: f32 = 1.0;
pub const MAX_EMISSION: f32 = 1.0;
pub const MIN_EMIT_AMOUNT: f32 = 0.1;
pub const MIN_WATER_DEPTH: f32 = 0.0;
pub const NORMAL_VELOCITY_MAGNITUDE: f32 = 100.0;
pub const FLUID_VELOCITY_MAGNITUDE: f32 = 1.0;
pub const VELOCITY_MULT: f32 = 1.0;

/// Seeds baked into the CalculateRandomRange* functions (ScriptUsage5).
pub const SEED_FLOAT: i32 = 83030449;
pub const SEED_VECTOR2: i32 = 83030450;
pub const SEED_RECYCLER_CALL: i32 = 83030451;

/// FloatFromCurve004 (ScaleColor001 Scale Alpha over NormalizedAge) ShaderLUT.
pub const ALPHA_LUT: [f32; 108] = [
    0.0, 0.024570561945438385, 0.09175187349319458, 0.19174835085868835, 0.3147644102573395, 0.4510045349597931,
    0.59067302942276, 0.7239746451377869, 0.8411132097244263, 0.9322937726974487, 0.9877204298973083, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
    1.0, 1.0, 0.994965136051178, 0.9750226140022278, 0.9454430937767029, 0.9073152542114258,
    0.8617271780967712, 0.8097676634788513, 0.7525249719619751, 0.691087007522583, 0.6265429258346558, 0.5599803924560547,
    0.4924885630607605, 0.4251554608345032, 0.35906916856765747, 0.2953186631202698, 0.23499184846878052, 0.1791776418685913,
    0.12896400690078735, 0.08543926477432251, 0.04969233274459839, 0.02281099557876587, 0.005884230136871338, 5.960464477539063e-08,
];

/// ScaleSpriteSize002 Uniform Curve Sprite Scale over NormalizedAge ShaderLUT.
pub const SIZE_LUT: [f32; 98] = [
    0.0, 0.04432586953043938, 0.09070748835802078, 0.13848356902599335, 0.18699286878108978, 0.23557406663894653,
    0.2835659086704254, 0.33030709624290466, 0.3751363754272461, 0.4173924922943115, 0.4564140737056732, 0.4916512370109558,
    0.5247067809104919, 0.5563344359397888, 0.5865646600723267, 0.6154276728630066, 0.6429539918899536, 0.6691738367080688,
    0.6941177248954773, 0.7178159356117249, 0.740298867225647, 0.7615969181060791, 0.7817404866218567, 0.8007599115371704,
    0.8186854124069214, 0.8355475664138794, 0.8513767719268799, 0.8662033081054688, 0.8800574541091919, 0.8929696679115295,
    0.9049705266952515, 0.9160900115966797, 0.9263587594032288, 0.9358070492744446, 0.9444653391838074, 0.9523639678955078,
    0.9595332145690918, 0.9660035371780396, 0.9718053340911865, 0.9769689440727234, 0.9815247058868408, 0.9855029582977295,
    0.9889342784881592, 0.9918488264083862, 0.9942770004272461, 0.9962493181228638, 0.9977959394454956, 0.9989473819732666,
    0.9997339248657227, 1.0001236200332642, 0.9997587203979492, 0.9985748529434204, 0.996568500995636, 0.9937363862991333,
    0.9900749921798706, 0.9855810403823853, 0.9802510738372803, 0.9740816950798035, 0.9670695066452026, 0.9592111110687256,
    0.9505031108856201, 0.9409421682357788, 0.9305248260498047, 0.9192476868629456, 0.9071073532104492, 0.8941004276275635,
    0.8802235722541809, 0.8654733300209045, 0.849846363067627, 0.8333390951156616, 0.8159483671188354, 0.7976707220077515,
    0.7785026431083679, 0.7584407925605774, 0.7374818921089172, 0.7156223654747009, 0.6928589344024658, 0.6691881418228149,
    0.6446065902709961, 0.6191109418869019, 0.5926977396011353, 0.5653636455535889, 0.5371052026748657, 0.5079190731048584,
    0.47780174016952515, 0.4467499256134033, 0.4147602319717407, 0.3818292021751404, 0.34795355796813965, 0.3131297826766968,
    0.27735447883605957, 0.2406243085861206, 0.20293575525283813, 0.1642855405807495, 0.12467032670974731, 0.08408653736114502,
    0.04253119230270386, 3.5762786865234375e-07,
];

/// M_SimpleSpray via MI_SimpleSpray (values the instance overrides, parent
/// defaults otherwise).
pub mod material {
    pub const BASE_COLOR: [f32; 3] = [0.608430027961731, 0.6782609820365906, 0.6444460153579712];
    pub const SUBSURFACE_GAIN: [f32; 3] = [0.15450899302959442, 0.434783011674881, 0.3172830045223236];
    pub const OPACITY_MULT: f32 = 0.25;
    pub const DISTORTION_STRENGTH: f32 = 0.5;
    pub const DISTORTION_SCALE: f32 = 1.0;
    /// Subtract_3 ConstB on the noise red channel.
    pub const NOISE_CENTRE: f32 = 0.7;
    /// SphereMask centre 0.5, AttenuationRadius 0.3, HardnessPercent 50.
    pub const SPHERE_RADIUS: f32 = 0.3;
    pub const SPHERE_HARDNESS: f32 = 0.5;
    /// Custom_0 Radius (sprite sphere normal).
    pub const NORMAL_SPHERE_RADIUS: f32 = 1.25;
    /// FlattenNormal Flatness input (Constant_6).
    pub const NORMAL_FLATNESS: f32 = -5.0;
}

/// The update kernel (bindings at group 0) with the generated constants.
pub fn shader() -> String {
    format!("{}\n{KERNEL}", library())
}

/// Particle layout, random numbers, LUTs and constants without bindings, for
/// the sprite draw composed into another module.
pub fn library() -> String {
    let f = |v: f32| format!("{v:?}");
    let array = |name: &str, values: &[f32]| {
        format!(
            "const {name}: array<f32,{}> = array<f32,{}>({});",
            values.len(),
            values.len(),
            values.iter().map(|v| f(*v)).collect::<Vec<_>>().join(",")
        )
    };
    let constants = [
        array("RIVER_SPRAY_ALPHA_LUT", &ALPHA_LUT),
        array("RIVER_SPRAY_SIZE_LUT", &SIZE_LUT),
        format!("const RIVER_SPRAY_ALPHA_COUNT: u32 = {}u;", ALPHA_LUT.len()),
        format!("const RIVER_SPRAY_SIZE_COUNT: u32 = {}u;", SIZE_LUT.len()),
        format!("const RIVER_SPRAY_LIFETIME: f32 = {};", f(LIFETIME)),
        format!("const RIVER_SPRAY_SIZE_RANGE: vec2<f32> = vec2<f32>({},{});", f(SPRITE_SIZE_RANGE[0]), f(SPRITE_SIZE_RANGE[1])),
        format!("const RIVER_SPRAY_ROTATION_RANGE: vec2<f32> = vec2<f32>({},{});", f(SPRITE_ROTATION_RANGE[0]), f(SPRITE_ROTATION_RANGE[1])),
        format!("const RIVER_SPRAY_ROTATION_RATE: vec2<f32> = vec2<f32>({},{});", f(ROTATION_RATE_RANGE[0]), f(ROTATION_RATE_RANGE[1])),
        format!("const RIVER_SPRAY_NEAR_FAR: vec2<f32> = vec2<f32>({},{});", f(NEAR_FAR_MAXIMUM[0]), f(NEAR_FAR_MAXIMUM[1])),
        format!("const RIVER_SPRAY_REFERENCE_FOV: f32 = {};", f((MAXIMUM_FOV_DEGREES * 0.5).to_radians().tan())),
        format!("const RIVER_SPRAY_GRAVITY: vec3<f32> = vec3<f32>({},{},{});", f(GRAVITY[0]), f(GRAVITY[1]), f(GRAVITY[2])),
        format!("const RIVER_SPRAY_BUOYANCY: vec3<f32> = vec3<f32>({},{},{});", f(BUBBLE_BUOYANCY[0]), f(BUBBLE_BUOYANCY[1]), f(BUBBLE_BUOYANCY[2])),
        format!("const RIVER_SPRAY_HIDDEN: i32 = {HIDDEN_VISIBILITY_TAG};"),
        format!("const RIVER_SPRAY_REJECTION_SAMPLES: i32 = {REJECTION_SAMPLES};"),
        format!("const RIVER_SPRAY_SEED_FLOAT: i32 = {SEED_FLOAT};"),
        format!("const RIVER_SPRAY_SEED_VECTOR2: i32 = {SEED_VECTOR2};"),
        format!("const RIVER_SPRAY_SEED_RECYCLER: i32 = {SEED_RECYCLER_CALL};"),
        format!(
            "const RIVER_SPRAY_CRITERIA: array<vec4<f32>,4> = array<vec4<f32>,4>(vec4<f32>({},{},{},0.0),vec4<f32>({},{},{},0.0),vec4<f32>({},{},{},0.0),vec4<f32>({},{},{},0.0));",
            f(VORTICITY.0[0]), f(VORTICITY.0[1]), f(VORTICITY.1),
            f(VELOCITY.0[0]), f(VELOCITY.0[1]), f(VELOCITY.1),
            f(CURVATURE.0[0]), f(CURVATURE.0[1]), f(CURVATURE.1),
            f(DEPTH_DIFFERENCE.0[0]), f(DEPTH_DIFFERENCE.0[1]), f(DEPTH_DIFFERENCE.1),
        ),
        format!("const RIVER_SPRAY_SPEED_GATE: vec2<f32> = vec2<f32>({},{});", f(RESTRICT_SPEED[0]), f(RESTRICT_SPEED[1])),
        format!(
            "const RIVER_SPRAY_EMISSION: vec4<f32> = vec4<f32>({},{},{},{});",
            f(EMISSION_MULT), f(MAX_EMISSION), f(MIN_EMIT_AMOUNT), f(MIN_WATER_DEPTH)
        ),
        format!(
            "const RIVER_SPRAY_SPAWN_VELOCITY: vec3<f32> = vec3<f32>({},{},{});",
            f(NORMAL_VELOCITY_MAGNITUDE), f(FLUID_VELOCITY_MAGNITUDE), f(VELOCITY_MULT)
        ),
        {
            use material as m;
            format!(
                "const RIVER_SPRAY_BASE_COLOR: vec3<f32> = vec3<f32>({},{},{});\nconst RIVER_SPRAY_MATERIAL: vec4<f32> = vec4<f32>({},{},{},{});\nconst RIVER_SPRAY_MASK: vec4<f32> = vec4<f32>({},{},{},{});",
                f(m::BASE_COLOR[0]), f(m::BASE_COLOR[1]), f(m::BASE_COLOR[2]),
                f(m::OPACITY_MULT), f(m::DISTORTION_STRENGTH), f(m::DISTORTION_SCALE), f(m::NOISE_CENTRE),
                f(m::SPHERE_RADIUS), f(m::SPHERE_HARDNESS), f(m::NORMAL_SPHERE_RADIUS), f(m::NORMAL_FLATNESS),
            )
        },
    ];
    format!("{}\n{COMMON}", constants.join("\n"))
}

/// Niagara rand3 (NiagaraEmitterInstanceShader.usf): 3 normalized floats from
/// four integer seeds; all bits of the first two, lower 16 of the last two.
pub fn rand3(seed1: i32, seed2: i32, seed3: i32, seed4: i32) -> [f32; 3] {
    let m = |s: i32| s.wrapping_mul(1664525).wrapping_add(1013904223);
    let mut v = [m(seed1), m(seed2), m(seed4 | (seed3 << 16))];
    for _ in 0..2 {
        v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
        v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
        v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    }
    v.map(|x| ((x >> 8) & 0x00ff_ffff) as f32 / 16777216.0)
}

/// Niagara rand4: 4 normalized floats from four integer seeds.
pub fn rand4(seed1: i32, seed2: i32, seed3: i32, seed4: i32) -> [f32; 4] {
    let m = |s: i32| s.wrapping_mul(1664525).wrapping_add(1013904223);
    let mut v = [m(seed4), m(seed1), m(seed2), m(seed3)];
    for _ in 0..2 {
        v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[3]));
        v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
        v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
        v[3] = v[3].wrapping_add(v[1].wrapping_mul(v[2]));
    }
    v.map(|x| ((x >> 8) & 0x00ff_ffff) as f32 / 16777216.0)
}

/// Niagara float curve LUT sample (GetCurveLUTIndices, MinTime 0, InvTimeRange 1).
pub fn sample_lut(lut: &[f32], time: f32) -> f32 {
    let last = (lut.len() - 1) as f32;
    let t = time.clamp(0.0, 1.0) * last;
    let a = t.floor();
    let b = (a + 1.0).min(last);
    let fraction = t - a;
    let (va, vb) = (lut[a as usize], lut[b as usize]);
    va + (vb - va) * fraction
}

/// Sample_Water outputs beyond the foam field's (SampleSecondaryEmission).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WaterSample {
    pub emission: EmissionInputs,
    /// Grid water height plus LocalToWorldTranslation.z.
    pub water_height: f32,
    pub water_normal: [f32; 3],
}

/// Emission_Criterias for a particle in its respawn frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emission {
    pub emit_amount: f32,
    /// Respawned with EmitAmount below Min EmitAmount (or WaterDepth below
    /// Min WaterDepth): hidden this frame and recycled next frame.
    pub recycle: bool,
    pub spawn_position: [f32; 3],
    pub spawn_velocity: [f32; 3],
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The custom HLSL in Emission_Criterias with RandomVelocityMagnitude 0.
pub fn spawn_velocity(sim_velocity: [f32; 2], normal: [f32; 3]) -> [f32; 3] {
    let v = [sim_velocity[0], sim_velocity[1], 0.0];
    let d = dot3(v, normal);
    std::array::from_fn(|i| {
        ((v[i] - d * normal[i]) * FLUID_VELOCITY_MAGNITUDE + NORMAL_VELOCITY_MAGNITUDE * normal[i]) * VELOCITY_MULT
    })
}

/// Emission_Criterias (respawn frame). `depth_range` is RestrictEmissionDepthRange
/// (authored adapter: `foam_field::authored_depth_range`).
pub fn emission(position: [f32; 3], water: WaterSample, depth_range: [f32; 2]) -> Emission {
    let e = water.emission;
    let c = foam_field::criterion;
    let speed = (e.velocity[0] * e.velocity[0] + e.velocity[1] * e.velocity[1]).sqrt();
    let strongest = c(e.curl, VORTICITY.0, VORTICITY.1)
        .max(c(speed, VELOCITY.0, VELOCITY.1))
        .max(c(e.curvature, CURVATURE.0, CURVATURE.1))
        .max(c(e.depth_difference, DEPTH_DIFFERENCE.0, DEPTH_DIFFERENCE.1));
    let amount = (EMISSION_MULT * strongest * c(e.water_depth, depth_range, 1.0) * c(speed, RESTRICT_SPEED, 1.0))
        .clamp(0.0, MAX_EMISSION);
    Emission {
        emit_amount: amount,
        recycle: e.water_depth < MIN_WATER_DEPTH || amount < MIN_EMIT_AMOUNT,
        spawn_position: [position[0], position[1], water.water_height],
        spawn_velocity: spawn_velocity(e.velocity, water.water_normal),
    }
}

/// WhiteWaterVelocityAndForce: under the surface the particle rides the
/// projected surface velocity and buoyancy replaces the accumulated force.
pub fn whitewater(position: [f32; 3], velocity: [f32; 3], force: [f32; 3], water: WaterSample) -> ([f32; 3], [f32; 3]) {
    if !(position[2] < water.water_height) {
        return (velocity, force);
    }
    let v = [water.emission.velocity[0], water.emission.velocity[1], 0.0];
    let d = dot3(v, water.water_normal);
    (std::array::from_fn(|i| v[i] - d * water.water_normal[i]), BUBBLE_BUOYANCY)
}

/// SolveForcesAndVelocity (mass 1, no drag, no limits): semi-implicit Euler.
pub fn solve(position: [f32; 3], velocity: [f32; 3], force: [f32; 3], dt: f32) -> ([f32; 3], [f32; 3]) {
    let v: [f32; 3] = std::array::from_fn(|i| force[i] * dt + velocity[i]);
    (std::array::from_fn(|i| v[i] * dt + position[i]), v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::river::foam_field::{Fields, Setup};

    #[test]
    fn rand_matches_native_integer_pipeline() {
        // Hand-evaluated rand3(1,2,3,4): v = (1,2,4|(3<<16)) * 1664525 + 1013904223,
        // two mixing rounds with wrapping i32, upper 24 bits.
        let r = rand3(1, 2, 3, 4);
        let m = |s: i32| s.wrapping_mul(1664525).wrapping_add(1013904223);
        let (mut x, mut y, mut z) = (m(1), m(2), m(4 | (3 << 16)));
        for _ in 0..2 {
            x = x.wrapping_add(y.wrapping_mul(z));
            y = y.wrapping_add(z.wrapping_mul(x));
            z = z.wrapping_add(x.wrapping_mul(y));
        }
        assert_eq!(r[0], ((x >> 8) & 0xff_ffff) as f32 / 16777216.0);
        assert_eq!(r[2], ((z >> 8) & 0xff_ffff) as f32 / 16777216.0);
        // Range [0,1) and decorrelated over particle ids.
        let mut sum = 0.0f64;
        for id in 0..20000 {
            let v = rand3(id, SEED_FLOAT, 0, 1)[0];
            assert!((0.0..1.0).contains(&v));
            sum += f64::from(v);
        }
        assert!((sum / 20000.0 - 0.5).abs() < 0.01, "mean {}", sum / 20000.0);
        let v = rand4(7, 9, 7, -2);
        assert!(v.iter().all(|x| (0.0..1.0).contains(x)));
    }

    #[test]
    fn curve_luts_follow_saved_keys() {
        // Alpha: 0 at 0, 1 from 0.1 to 0.8, 0 at 1.
        assert_eq!(sample_lut(&ALPHA_LUT, 0.0), 0.0);
        assert_eq!(sample_lut(&ALPHA_LUT, 0.5), 1.0);
        assert!(sample_lut(&ALPHA_LUT, 1.0) < 1e-6);
        assert!(sample_lut(&ALPHA_LUT, 0.05) > 0.4 && sample_lut(&ALPHA_LUT, 0.05) < 0.8);
        // Size: 0 -> 0.48 at 0.11 -> 1 at 0.5 -> 0.
        assert_eq!(sample_lut(&SIZE_LUT, 0.0), 0.0);
        assert!((sample_lut(&SIZE_LUT, 0.5) - 1.0).abs() < 1e-3);
        assert!(sample_lut(&SIZE_LUT, 1.0) < 1e-6);
        assert!((sample_lut(&SIZE_LUT, 0.1106) - 0.48).abs() < 0.03);
    }

    /// A synthetic pourover: 200 cm deep upstream, a 60 cm drop at x = 0.5,
    /// 150 cm/s flow along +X. The surface normal tilts only across the drop.
    struct Pourover {
        cells: f32,
    }
    impl Pourover {
        fn height(&self, u: f32) -> f32 {
            // 3-cell linear ramp of the free surface across the lip.
            let t = ((u - 0.5) * self.cells / 3.0 + 0.5).clamp(0.0, 1.0);
            300.0 - 60.0 * t
        }
    }
    impl Fields for Pourover {
        fn depth(&self, unit: [f32; 2]) -> f32 {
            self.height(unit[0]) - 100.0
        }
        fn velocity(&self, _unit: [f32; 2]) -> [f32; 2] {
            [150.0, 0.0]
        }
        fn normal(&self, unit: [f32; 2]) -> [f32; 3] {
            let dx = 1.0 / self.cells;
            let slope = (self.height(unit[0] + dx) - self.height(unit[0] - dx)) / (2.0 * 25.0);
            let len = (slope * slope + 1.0).sqrt();
            [-slope / len, 0.0, 1.0 / len]
        }
    }

    #[test]
    fn emission_criteria_fire_at_the_pourover_lip_only() {
        let fields = Pourover { cells: 64.0 };
        let setup = Setup {
            dimensions: [64, 64],
            dx: 25.0,
            foam_dx: 25.0,
            sim_res: [64.0, 64.0],
            unit_to_world: [[0.0; 4]; 4],
            settings: Default::default(),
        };
        let frame = foam_field::Frame::new(1.0 / 60.0, 0.0);
        let sample = |u: f32| {
            let unit = [u, 0.5];
            WaterSample {
                emission: foam_field::emission_inputs(&setup, frame, unit, &fields),
                water_height: fields.height(u),
                water_normal: fields.normal(unit),
            }
        };
        let flat = emission([0.0; 3], sample(0.2), RESTRICT_DEPTH);
        assert!(flat.recycle && flat.emit_amount < MIN_EMIT_AMOUNT, "flat water emits {flat:?}");
        let lip = (0..64)
            .map(|i| (i as f32 + 0.5) / 64.0)
            .map(|u| (u, emission([0.0; 3], sample(u), RESTRICT_DEPTH)))
            .filter(|(_, e)| !e.recycle)
            .collect::<Vec<_>>();
        assert!(!lip.is_empty(), "no emission at the drop");
        for (u, e) in &lip {
            // Only around the lip; spawned moving up and along the flow.
            assert!((u - 0.5).abs() < 4.0 / 64.0, "emission away from the drop at {u}");
            assert!(e.spawn_velocity[2] > 0.0 && e.spawn_velocity[0] > 0.0, "{e:?}");
        }
        // Shallow water fails the native depth gate but passes the authored one.
        let u = lip[0].0;
        let shallow = WaterSample { emission: EmissionInputs { water_depth: 40.0, ..sample(u).emission }, ..sample(u) };
        assert!(emission([0.0; 3], shallow, RESTRICT_DEPTH).recycle);
        let authored = foam_field::authored_depth_range(0.0);
        assert!(!emission([0.0; 3], shallow, authored).recycle);
    }

    #[test]
    fn whitewater_and_solve_follow_module_order() {
        let water = WaterSample {
            emission: EmissionInputs { velocity: [100.0, 0.0], water_depth: 200.0, ..Default::default() },
            water_height: 50.0,
            water_normal: [0.0, 0.0, 1.0],
        };
        // Above water: gravity stands, ballistic step.
        let (v, f) = whitewater([0.0, 0.0, 60.0], [0.0, 0.0, 100.0], GRAVITY, water);
        assert_eq!((v, f), ([0.0, 0.0, 100.0], GRAVITY));
        let (p, v) = solve([0.0, 0.0, 60.0], v, f, 0.1);
        assert!((v[2] - 2.0).abs() < 1e-4 && (p[2] - 60.2).abs() < 1e-4);
        // Under water: surface velocity and buoyancy only.
        let (v, f) = whitewater([0.0, 0.0, 40.0], [5.0, 5.0, -300.0], GRAVITY, water);
        assert_eq!((v, f), ([100.0, 0.0, 0.0], BUBBLE_BUOYANCY));
    }
}
