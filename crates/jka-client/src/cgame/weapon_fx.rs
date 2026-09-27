//! OpenJK CG_Missile (cg_ents.c), the missile/impact visuals registered by
//! CG_RegisterWeapon (cg_weaponinit.c) and fx_*.c, and the effect-playing
//! entity events: EV_MISSILE_HIT/MISS/MISS_METAL and EV_PLAY_EFFECT[_ID].
//! Vanilla branches only (JA+/JAPRO "tribes" substitutions are not ported).

use super::{event_presenter::EventDispatchResult, ClientGameState, PresentationEvent, PresentedEntity};
use super::player_presenter::PlayerFxRequest;
use crate::{
    fx::{
        system::{EffectId, FxDraw, FxFrame, FxLight, FxSound, FxStats, FxSystem},
        template::Rng,
    },
    ui::SaberMarkMode,
};
use jka_assets::pk3::AssetSearchPath;
use jka_movement::{CollisionWorld, TraceQuery, TraceWorld, ENTITY_WORLD};
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
    thread::{self, JoinHandle},
};

const WP_SABER: i32 = 3;
const WP_BRYAR_PISTOL: i32 = 4;
const WP_BLASTER: i32 = 5;
const WP_DISRUPTOR: i32 = 6;
const WP_BOWCASTER: i32 = 7;
const WP_REPEATER: i32 = 8;
const WP_DEMP2: i32 = 9;
const WP_FLECHETTE: i32 = 10;
const WP_ROCKET_LAUNCHER: i32 = 11;
const WP_THERMAL: i32 = 12;
const WP_DET_PACK: i32 = 14;
const WP_CONCUSSION: i32 = 15;
const WP_BRYAR_OLD: i32 = 16;
const WP_EMPLACED_GUN: i32 = 17;
const WP_TURRET: i32 = 18;
const G2_MODEL_PART: i32 = 50;

const EF_ALT_FIRING: i32 = 1 << 10;
const EF_JETPACK_ACTIVE: i32 = 1 << 11;
const EF_MISSILE_STICK: i32 = 1 << 22;

const TR_STATIONARY: i32 = 0;
const TR_INTERPOLATE: i32 = 1;

const EV_PLAY_EFFECT: i32 = 68;
const EV_PLAY_EFFECT_ID: i32 = 69;
const EV_PLAY_PORTAL_EFFECT_ID: i32 = 70;
const EV_MISSILE_HIT: i32 = 85;
const EV_MISSILE_MISS: i32 = 86;
const EV_MISSILE_MISS_METAL: i32 = 87;

const MAX_EFX_BYTES: usize = 65536;
const CONTINUOUS_FX_BACKFILL_MAX_MS: i32 = 250;
// bg_public.h ET_FX states, carried in entityState.modelindex2.
const FX_STATE_OFF: i32 = 0;
const FX_STATE_ONE_SHOT_LIMIT: i32 = 10;

// OpenJK MASK_SOLID is CONTENTS_SOLID|CONTENTS_TERRAIN. Saber contact is kept
// on the presentation collision clone so it never mutates gameplay/prediction.
const CONTENTS_SOLID: i32 = 0x0000_0001;
const CONTENTS_TERRAIN: i32 = 0x0000_1000;
const SABER_CONTACT_MASK: i32 = CONTENTS_SOLID | CONTENTS_TERRAIN;
const SURF_NOIMPACT: i32 = 0x0008_0000;
const SURF_NOMARKS: i32 = 0x0010_0000;
// OpenJK cg_marks.cpp: MARK_TOTAL_TIME=10000, MARK_FADE_TIME=1000.
// CG_CreateSaberMarks backdates the glow by 8500 ms, giving it 1500 ms total
// (500 ms full intensity + the final 1000 ms fade). Keep Legacy exact.
const SABER_MARK_LIFETIME_MS: i32 = 10_000;
const SABER_MARK_FADE_MS: i32 = 1_000;
const SABER_GLOW_LIFETIME_MS: i32 = 1_500;
const SABER_MARK_MAX: usize = 768;
const MAX_SABER_BLADES: usize = 8;

// Enhanced is deliberately a separate cosmetic material simulation rather than
// a variation of the stock mark quad. Contact is sampled spatially, heat is
// accumulated by dwell time, and a smooth relief mesh is rebuilt while hot.
const MELT_POINT_SPACING: f32 = 1.25;
const MELT_HOLD_RADIUS: f32 = 1.8;
const MELT_STROKE_BREAK_MS: i32 = 140;
const MELT_COOL_TIME_MS: i32 = 8_000;
const MELT_SCAR_LIFETIME_MS: i32 = 30_000;
const MELT_MAX_POINTS_PER_STROKE: usize = 320;
const MELT_MAX_FINISHED_STROKES: usize = 64;

// Enhanced marks deliberately remain presentation-only. Rapier is a rigid-body
// solver, not a viscous/soft-body surface solver; using it for attached melt
// would produce detached beads/chunks rather than a coherent molten wall. If
// cooled slag later breaks free, those detached pieces can become Rapier bodies.

#[derive(Clone, Copy, Debug, Default)]
struct EntityFxState { next_time: i32, one_shot_sequence: i32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MissileFxGeneration {
    trajectory_time: i32,
    weapon: i32,
    custom_effect: i32,
    alt_fire: bool,
}

#[derive(Clone, Debug)]
struct MissileFxState {
    generation: MissileFxGeneration,
    last_time: i32,
    last_origin: [f32; 3],
    next_sample_time: f64,
}

enum FxWorkerCommand {
    AdjustTime(i32),
    ResetTime(i32),
    PlayDir { name: String, origin: [f32; 3], dir: [f32; 3] },
    PlayAxis { name: String, origin: [f32; 3], axis: [[f32; 3]; 3] },
    Frame { reply: mpsc::SyncSender<(FxFrame, FxStats)> },
    Shutdown,
}

struct FxWorker { fx: FxSystem, assets: AssetSearchPath, ids: HashMap<String, EffectId>, missing: HashSet<String> }

impl FxWorker {
    fn effect(&mut self, name: &str) -> EffectId {
        if let Some(id) = self.ids.get(name) { return *id; }
        let assets = &mut self.assets;
        let id = self.fx.register(name, &mut |path| assets.read(path, MAX_EFX_BYTES).ok().flatten().map(|a| a.bytes));
        if id == 0 && self.missing.insert(name.to_ascii_lowercase()) { println!("FX: effect {name} unavailable"); }
        self.ids.insert(name.to_owned(), id);
        id
    }
}

fn fx_worker_loop(rx: mpsc::Receiver<FxWorkerCommand>, assets: AssetSearchPath) {
    let mut worker = FxWorker { fx: FxSystem::new(), assets, ids: HashMap::new(), missing: HashSet::new() };
    while let Ok(command) = rx.recv() {
        match command {
            FxWorkerCommand::AdjustTime(time) => worker.fx.adjust_time(time),
            FxWorkerCommand::ResetTime(time) => worker.fx.reset_time(time),
            FxWorkerCommand::PlayDir { name, origin, dir } => { let id = worker.effect(&name); if id != 0 { worker.fx.play_effect_dir(id, origin, dir); } }
            FxWorkerCommand::PlayAxis { name, origin, axis } => { let id = worker.effect(&name); if id != 0 { worker.fx.play_effect(id, origin, axis); } }
            FxWorkerCommand::Frame { reply } => { let frame = worker.fx.frame(); let _ = reply.send((frame, worker.fx.stats())); }
            FxWorkerCommand::Shutdown => break,
        }
    }
}

fn saber_shaders(color: i32) -> (&'static str, &'static str) {
    match color {
        0 => ("gfx/effects/sabers/red_glow", "gfx/effects/sabers/red_line"),
        1 => ("gfx/effects/sabers/orange_glow", "gfx/effects/sabers/orange_line"),
        2 => ("gfx/effects/sabers/yellow_glow", "gfx/effects/sabers/yellow_line"),
        3 => ("gfx/effects/sabers/green_glow", "gfx/effects/sabers/green_line"),
        4 => ("gfx/effects/sabers/blue_glow", "gfx/effects/sabers/blue_line"),
        5 => ("gfx/effects/sabers/purple_glow", "gfx/effects/sabers/purple_line"),
        _ => ("gfx/effects/sabers/blue_glow", "gfx/effects/sabers/blue_line"),
    }
}

/// OpenJK CG_RGBForSaberColor. Keep this separate from the authored glow
/// shader lookup because the dynamic-light tint intentionally uses softened
/// channel values (for example blue = 0.2, 0.4, 1.0).
fn saber_light_rgb(color: i32) -> Option<[f32; 3]> {
    Some(match color {
        0 => [1.0, 0.2, 0.2],
        1 => [1.0, 0.5, 0.1],
        2 => [1.0, 1.0, 0.2],
        3 => [0.2, 1.0, 0.2],
        4 => [0.2, 0.4, 1.0],
        5 => [0.9, 0.2, 1.0],
        _ => return None,
    })
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length > 1.0e-6 {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        [1.0, 0.0, 0.0]
    }
}

fn madd3(origin: [f32; 3], direction: [f32; 3], distance: f32) -> [f32; 3] {
    [
        origin[0] + direction[0] * distance,
        origin[1] + direction[1] * distance,
        origin[2] + direction[2] * distance,
    ]
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(v: [f32; 3], scale: f32) -> [f32; 3] {
    [v[0] * scale, v[1] * scale, v[2] * scale]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length3(v: [f32; 3]) -> f32 {
    dot3(v, v).sqrt()
}

/// OpenJK CG_CreateSaberMarks texture projection. The stock rivet/glow images
/// are authored around UV 0.5,0.5; mapping a slash to 0..1 made short sampled
/// segments look like isolated dots. Keep the same centered projection scales
/// while lifting the polygon slightly because the dynamic FX path does not yet
/// carry q3 shader polygonOffset/depth bias.
fn saber_mark_ribbon(
    start: [f32; 3],
    end: [f32; 3],
    normal: [f32; 3],
    half_width: f32,
    normal_offset: f32,
) -> Option<([[f32; 3]; 4], [[f32; 2]; 4])> {
    let delta = sub3(end, start);
    let distance = length3(delta);
    if distance < 1.0e-4 {
        return None;
    }
    let tangent = scale3(delta, 1.0 / distance);
    let side_raw = cross3(tangent, normal);
    let side_length = length3(side_raw);
    if side_length < 1.0e-4 {
        return None;
    }
    let side = scale3(side_raw, 1.0 / side_length);
    let tangent_cap = scale3(tangent, half_width);
    let side_cap = scale3(side, half_width);
    let push = scale3(normal, normal_offset);
    let start_cap = sub3(start, tangent_cap);
    let end_cap = add3(end, tangent_cap);
    let positions_unpushed = [
        sub3(start_cap, side_cap),
        sub3(end_cap, side_cap),
        add3(end_cap, side_cap),
        add3(start_cap, side_cap),
    ];
    let positions = positions_unpushed.map(|point| add3(point, push));
    let mid = scale3(add3(start, end), 0.5);
    // OpenJK randomizes these within 0.05..0.08 and 0.15..0.20. Midpoints
    // preserve the same visual frequency without introducing frame-rate RNG.
    let u_scale = 0.065;
    let v_scale = 0.175;
    let uvs = positions_unpushed.map(|point| {
        let d = sub3(point, mid);
        [
            0.5 + dot3(d, tangent) * u_scale,
            0.5 + dot3(d, side) * v_scale,
        ]
    });
    Some((positions, uvs))
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    add3(a, scale3(sub3(b, a), t))
}

fn catmull_rom3(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3], t: f32) -> [f32; 3] {
    let t2 = t * t;
    let t3 = t2 * t;
    std::array::from_fn(|i| {
        0.5 * ((2.0 * p1[i])
            + (-p0[i] + p2[i]) * t
            + (2.0 * p0[i] - 5.0 * p1[i] + 4.0 * p2[i] - p3[i]) * t2
            + (-p0[i] + 3.0 * p1[i] - 3.0 * p2[i] + p3[i]) * t3)
    })
}

fn catmull_rom_scalar(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() < 1.0e-6 {
        return if value >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn cooled_heat(point: &MeltPoint, time: i32) -> f32 {
    let age = time.saturating_sub(point.last_heated_at).max(0) as f32;
    let remaining = (1.0 - age / MELT_COOL_TIME_MS as f32).clamp(0.0, 1.0);
    point.heat * remaining.powf(1.55)
}

fn gravity_on_surface(normal: [f32; 3]) -> [f32; 3] {
    let gravity = [0.0, 0.0, -1.0];
    let tangent = sub3(gravity, scale3(normal, dot3(gravity, normal)));
    if length3(tangent) > 0.15 {
        normalize3(tangent)
    } else {
        [0.0; 3]
    }
}

fn smooth_melt_samples(stroke: &MeltStroke, time: i32) -> Vec<MeltRenderSample> {
    let points = &stroke.points;
    if points.is_empty() {
        return Vec::new();
    }
    if points.len() == 1 {
        let point = &points[0];
        return vec![MeltRenderSample {
            position: point.position,
            normal: point.normal,
            heat: cooled_heat(point, time),
            dwell_ms: point.dwell_ms,
            phase: point.phase,
        }];
    }

    let mut out = Vec::new();
    for index in 0..points.len() - 1 {
        let i0 = index.saturating_sub(1);
        let i1 = index;
        let i2 = index + 1;
        let i3 = (index + 2).min(points.len() - 1);
        let p0 = &points[i0];
        let p1 = &points[i1];
        let p2 = &points[i2];
        let p3 = &points[i3];
        let segment_length = length3(sub3(p2.position, p1.position));
        let steps = ((segment_length / 0.75).ceil() as usize).clamp(1, 8);
        for step in 0..steps {
            let t = step as f32 / steps as f32;
            let normal = normalize3(catmull_rom3(p0.normal, p1.normal, p2.normal, p3.normal, t));
            let h0 = cooled_heat(p0, time);
            let h1 = cooled_heat(p1, time);
            let h2 = cooled_heat(p2, time);
            let h3 = cooled_heat(p3, time);
            out.push(MeltRenderSample {
                position: catmull_rom3(p0.position, p1.position, p2.position, p3.position, t),
                normal,
                heat: catmull_rom_scalar(h0, h1, h2, h3, t).max(0.0),
                dwell_ms: catmull_rom_scalar(p0.dwell_ms, p1.dwell_ms, p2.dwell_ms, p3.dwell_ms, t).max(0.0),
                phase: catmull_rom_scalar(p0.phase, p1.phase, p2.phase, p3.phase, t),
            });
        }
    }
    let point = points.last().unwrap();
    out.push(MeltRenderSample {
        position: point.position,
        normal: point.normal,
        heat: cooled_heat(point, time),
        dwell_ms: point.dwell_ms,
        phase: point.phase,
    });
    out
}

#[derive(Clone, Copy)]
enum MeltLayer {
    Scar,
    Relief,
    Halo,
    Core,
    WhiteCore,
}

fn heat_color(scale: f32, rgb: [f32; 3]) -> [u8; 4] {
    [
        (rgb[0] * scale).round().clamp(0.0, 255.0) as u8,
        (rgb[1] * scale).round().clamp(0.0, 255.0) as u8,
        (rgb[2] * scale).round().clamp(0.0, 255.0) as u8,
        255,
    ]
}

fn melt_strip_mesh(samples: &[MeltRenderSample], time: i32, layer: MeltLayer) -> Option<FxDraw> {
    if samples.len() < 2 {
        return None;
    }
    let cross: &[f32] = match layer {
        MeltLayer::Relief => &[-1.0, -0.58, -0.20, 0.0, 0.20, 0.58, 1.0],
        _ => &[-1.0, 1.0],
    };
    let mut positions = Vec::with_capacity(samples.len() * cross.len());
    let mut uvs = Vec::with_capacity(positions.capacity());
    let mut rgba = Vec::with_capacity(positions.capacity());
    let mut indices = Vec::with_capacity((samples.len() - 1) * (cross.len() - 1) * 6);
    let mut distance_along = 0.0_f32;

    for (index, sample) in samples.iter().enumerate() {
        if index > 0 {
            distance_along += length3(sub3(sample.position, samples[index - 1].position));
        }
        let tangent = if index == 0 {
            sub3(samples[1].position, sample.position)
        } else if index + 1 == samples.len() {
            sub3(sample.position, samples[index - 1].position)
        } else {
            sub3(samples[index + 1].position, samples[index - 1].position)
        };
        let side_raw = cross3(normalize3(tangent), sample.normal);
        if length3(side_raw) < 1.0e-4 {
            continue;
        }
        let side = normalize3(side_raw);
        let hot = sample.heat.clamp(0.0, 1.5);
        let hot01 = (hot / 1.25).clamp(0.0, 1.0);
        let dwell = (sample.dwell_ms / 1000.0).max(0.0);
        let mass = dwell.sqrt().min(2.2);
        let wobble = 1.0 + (sample.phase + time as f32 * 0.0045).sin() * 0.045 * hot01;
        let width = match layer {
            MeltLayer::Scar => (0.95 + 0.55 * mass + 0.40 * hot01) * wobble,
            MeltLayer::Relief => (1.05 + 1.15 * mass + 0.95 * hot01) * wobble,
            MeltLayer::Halo => (1.55 + 1.45 * mass + 1.90 * hot01) * wobble,
            MeltLayer::Core => (0.34 + 0.30 * mass + 0.48 * hot01) * wobble,
            MeltLayer::WhiteCore => (0.12 + 0.15 * mass + 0.24 * hot01) * wobble,
        };
        let down = gravity_on_surface(sample.normal);
        let slump = if down == [0.0; 3] {
            0.0
        } else {
            ((sample.dwell_ms - 260.0).max(0.0) / 1000.0 * 1.45 * hot01).min(6.5)
        };
        let center = add3(sample.position, scale3(down, slump));

        for &v in cross {
            let abs_v = v.abs();
            let relief = match layer {
                MeltLayer::Scar => 0.16,
                MeltLayer::Halo => 0.22,
                MeltLayer::Core => 0.44 + 0.18 * hot01,
                MeltLayer::WhiteCore => 0.54 + 0.20 * hot01,
                MeltLayer::Relief => {
                    if abs_v < 0.08 {
                        0.15 + 0.08 * hot01
                    } else if abs_v < 0.30 {
                        0.48 + 0.95 * hot01 + 0.16 * mass
                    } else if abs_v < 0.72 {
                        0.30 + 0.58 * hot01 + 0.12 * mass
                    } else {
                        0.17 + 0.16 * hot01
                    }
                }
            };
            let position = add3(add3(center, scale3(side, width * v)), scale3(sample.normal, relief));
            positions.push(position);
            uvs.push([0.5 + distance_along * 0.065, 0.5 + v * 0.48]);
            let color = match layer {
                MeltLayer::Scar => [255; 4],
                MeltLayer::Halo => heat_color(hot01.powf(0.65), [128.0, 32.0, 2.0]),
                MeltLayer::Core => heat_color(smoothstep(0.10, 0.95, hot), [255.0, 154.0, 24.0]),
                MeltLayer::WhiteCore => heat_color(smoothstep(0.62, 1.30, hot), [255.0, 246.0, 205.0]),
                MeltLayer::Relief => {
                    let lip = smoothstep(0.10, 0.92, hot);
                    if abs_v < 0.10 {
                        heat_color(lip, [210.0, 74.0, 5.0])
                    } else if abs_v < 0.32 {
                        heat_color(lip, [255.0, 190.0, 62.0])
                    } else if abs_v < 0.72 {
                        heat_color(lip, [238.0, 91.0, 8.0])
                    } else {
                        heat_color(lip * 0.65, [150.0, 32.0, 2.0])
                    }
                }
            };
            rgba.push(color);
        }
    }

    if positions.len() != samples.len() * cross.len() {
        return None;
    }
    let columns = cross.len() as u32;
    for row in 0..(samples.len() as u32 - 1) {
        for column in 0..columns - 1 {
            let a = row * columns + column;
            let b = (row + 1) * columns + column;
            let c = (row + 1) * columns + column + 1;
            let d = row * columns + column + 1;
            indices.extend_from_slice(&[a, b, d, d, b, c]);
        }
    }
    let shader = match layer {
        MeltLayer::Scar => "gfx/damage/rivetmark",
        MeltLayer::Relief | MeltLayer::Halo | MeltLayer::Core | MeltLayer::WhiteCore => "gfx/effects/saberDamageGlow",
    };
    Some(FxDraw::Mesh { positions, uvs, rgba, indices, shader: shader.to_owned() })
}

fn melt_blob_mesh(sample: MeltRenderSample, time: i32, hot_layer: bool) -> Option<FxDraw> {
    let heat = sample.heat.clamp(0.0, 1.5);
    let hot01 = (heat / 1.25).clamp(0.0, 1.0);
    let dwell_s = (sample.dwell_ms / 1000.0).max(0.0);
    if hot_layer && hot01 < 0.02 {
        return None;
    }
    if dwell_s < 0.18 && !hot_layer {
        return None;
    }
    let normal = sample.normal;
    let down = gravity_on_surface(normal);
    let right = if down == [0.0; 3] {
        let trial = cross3(normal, [1.0, 0.0, 0.0]);
        if length3(trial) > 0.1 { normalize3(trial) } else { normalize3(cross3(normal, [0.0, 1.0, 0.0])) }
    } else {
        normalize3(cross3(down, normal))
    };
    let down_axis = if down == [0.0; 3] { normalize3(cross3(normal, right)) } else { down };
    let radius = (0.85 + dwell_s.sqrt() * 1.65 + hot01 * 0.75).min(5.8);
    let sag = ((sample.dwell_ms - 240.0).max(0.0) / 1000.0 * 1.3 * hot01).min(6.0);
    let pulse = 1.0 + (sample.phase + time as f32 * 0.004).sin() * 0.04 * hot01;
    let center = add3(add3(sample.position, scale3(down_axis, sag * 0.38)), scale3(normal, if hot_layer { 0.50 } else { 0.18 }));
    let segments = 20usize;
    let mut positions = Vec::with_capacity(segments + 1);
    let mut uvs = Vec::with_capacity(segments + 1);
    let mut rgba = Vec::with_capacity(segments + 1);
    positions.push(add3(center, scale3(normal, if hot_layer { 0.38 + 0.45 * hot01 } else { 0.10 })));
    uvs.push([0.5, 0.5]);
    rgba.push(if hot_layer {
        heat_color(smoothstep(0.18, 1.18, heat), [255.0, 188.0, 66.0])
    } else {
        [255; 4]
    });
    for index in 0..segments {
        let angle = std::f32::consts::TAU * index as f32 / segments as f32;
        let (sin, cos) = angle.sin_cos();
        let downward = sin.max(0.0);
        let vertical_scale = 0.72 + downward * (0.65 + 0.55 * hot01 + 0.18 * dwell_s.min(3.0));
        let radial = add3(scale3(right, cos * radius * pulse), scale3(down_axis, sin * radius * vertical_scale));
        positions.push(add3(center, add3(radial, scale3(normal, 0.04 + 0.12 * hot01))));
        uvs.push([0.5 + cos * 0.5, 0.5 + sin * 0.5]);
        rgba.push(if hot_layer {
            heat_color(hot01.powf(0.7) * 0.72, [230.0, 78.0, 8.0])
        } else {
            [255; 4]
        });
    }
    let mut indices = Vec::with_capacity(segments * 3);
    for index in 0..segments {
        let next = (index + 1) % segments;
        indices.extend_from_slice(&[0, 1 + index as u32, 1 + next as u32]);
    }
    Some(FxDraw::Mesh {
        positions,
        uvs,
        rgba,
        indices,
        shader: if hot_layer { "gfx/effects/whiteGlow" } else { "gfx/damage/rivetmark" }.to_owned(),
    })
}

fn melt_drip_mesh(sample: MeltRenderSample, time: i32) -> Option<FxDraw> {
    let down = gravity_on_surface(sample.normal);
    if down == [0.0; 3] || sample.dwell_ms < 650.0 {
        return None;
    }
    let heat = sample.heat.clamp(0.0, 1.5);
    let hot01 = (heat / 1.25).clamp(0.0, 1.0);
    if hot01 < 0.04 {
        return None;
    }
    let dwell_s = sample.dwell_ms / 1000.0;
    let length = ((dwell_s - 0.55) * 1.75 * hot01).clamp(0.0, 8.5);
    if length < 0.25 {
        return None;
    }
    let right = normalize3(cross3(down, sample.normal));
    let steps = 9usize;
    let mut positions = Vec::with_capacity(steps * 2);
    let mut uvs = Vec::with_capacity(steps * 2);
    let mut rgba = Vec::with_capacity(steps * 2);
    let mut indices = Vec::with_capacity((steps - 1) * 6);
    for step in 0..steps {
        let t = step as f32 / (steps - 1) as f32;
        let bend = (sample.phase * 1.7 + t * 3.2 + time as f32 * 0.0015).sin() * 0.16 * hot01 * t;
        let center = add3(
            add3(sample.position, scale3(down, length * t)),
            add3(scale3(right, bend), scale3(sample.normal, 0.38 + 0.16 * hot01)),
        );
        let half_width = (0.44 + 0.20 * hot01) * (1.0 - 0.72 * t);
        let color = heat_color(hot01.powf(0.72) * (1.0 - 0.34 * t), [255.0, 132.0, 18.0]);
        positions.push(sub3(center, scale3(right, half_width)));
        positions.push(add3(center, scale3(right, half_width)));
        uvs.push([t, 0.0]);
        uvs.push([t, 1.0]);
        rgba.push(color);
        rgba.push(color);
        if step + 1 < steps {
            let a = (step * 2) as u32;
            indices.extend_from_slice(&[a, a + 2, a + 1, a + 1, a + 2, a + 3]);
        }
    }
    Some(FxDraw::Mesh {
        positions,
        uvs,
        rgba,
        indices,
        shader: "gfx/effects/saberDamageGlow".to_owned(),
    })
}

/// The refEntity CG_Missile submits for a model-carrying missile.
#[derive(Clone, Debug, PartialEq)]
pub struct MissileModel {
    pub qpath: &'static str,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
}

/// weaponInfo_t missileTrailFunc effect (and its alt-fire twin).
fn trail_effect(weapon: i32, alt: bool) -> Option<&'static str> {
    Some(match (weapon, alt) {
        (WP_BRYAR_PISTOL | WP_BRYAR_OLD, _) => "bryar/shot",
        (WP_BLASTER | WP_EMPLACED_GUN, _) => "blaster/shot",
        (WP_BOWCASTER, _) => "bowcaster/shot",
        (WP_REPEATER, false) => "repeater/projectile",
        (WP_REPEATER, true) => "repeater/alt_projectile",
        (WP_DEMP2, false) => "demp2/projectile",
        (WP_FLECHETTE, false) => "flechette/shot",
        (WP_FLECHETTE, true) => "flechette/alt_shot",
        (WP_ROCKET_LAUNCHER, _) => "rocket/shot",
        (WP_CONCUSSION, _) => "concussion/shot",
        (WP_TURRET, _) => "turret/shot",
        _ => return None,
    })
}

/// weaponInfo_t missileModel / altMissileModel.
fn missile_model(weapon: i32, alt: bool) -> Option<&'static str> {
    Some(match (weapon, alt) {
        (WP_FLECHETTE, false) => "models/weapons2/golan_arms/projectileMain.md3",
        (WP_FLECHETTE, true) => "models/weapons2/golan_arms/projectile.md3",
        (WP_ROCKET_LAUNCHER, _) => "models/weapons2/merr_sonn/projectile.md3",
        (WP_THERMAL, _) => "models/weapons2/thermal/thermal_proj.md3",
        (WP_DET_PACK, _) => "models/weapons2/detpack/det_pack.md3",
        _ => return None,
    })
}

/// CG_MissileHitWall -> FX_*HitWall. The second field marks effects that
/// play along world up instead of the impact normal.
fn wall_impacts(weapon: i32, alt: bool, charge: i32) -> &'static [(&'static str, bool)] {
    match (weapon, alt) {
        (WP_BRYAR_PISTOL | WP_BRYAR_OLD, true) => match charge {
            4 | 5 => &[("bryar/wall_impact3", false)],
            2 | 3 => &[("bryar/wall_impact2", false)],
            _ => &[("bryar/wall_impact", false)],
        },
        (WP_BRYAR_PISTOL | WP_BRYAR_OLD | WP_TURRET, _) => &[("bryar/wall_impact", false)],
        (WP_CONCUSSION, _) => &[("concussion/explosion", false)],
        (WP_BLASTER | WP_EMPLACED_GUN, _) => &[("blaster/wall_impact", false)],
        (WP_DISRUPTOR, _) => &[("disruptor/alt_miss", false)],
        (WP_BOWCASTER, _) => &[("bowcaster/explosion", false)],
        (WP_REPEATER, true) => &[("repeater/concussion", false)],
        (WP_REPEATER, false) => &[("repeater/wall_impact", false)],
        (WP_DEMP2, true) => &[("demp2/altDetonate", false)],
        (WP_DEMP2, false) => &[("demp2/wall_impact", false)],
        (WP_FLECHETTE, false) => &[("flechette/wall_impact", false)],
        (WP_ROCKET_LAUNCHER, _) => &[("rocket/explosion", false)],
        (WP_THERMAL, _) => &[("thermal/explosion", false), ("thermal/shockwave", true)],
        _ => &[],
    }
}

/// CG_MissileHitPlayer -> FX_*HitPlayer (humanoid targets).
fn player_impacts(weapon: i32, alt: bool) -> &'static [(&'static str, bool)] {
    match (weapon, alt) {
        (WP_BRYAR_PISTOL | WP_BRYAR_OLD | WP_TURRET, _) => &[("bryar/flesh_impact", false)],
        (WP_CONCUSSION, _) => &[("concussion/explosion", false)],
        (WP_BLASTER | WP_EMPLACED_GUN, _) => &[("blaster/flesh_impact", false)],
        (WP_DISRUPTOR, _) => &[("disruptor/alt_hit", false)],
        (WP_BOWCASTER, _) => &[("bowcaster/explosion", false)],
        (WP_REPEATER, true) => &[("repeater/concussion", false)],
        (WP_REPEATER, false) => &[("repeater/flesh_impact", false)],
        (WP_DEMP2, true) => &[("demp2/altDetonate", false)],
        (WP_DEMP2, false) => &[("demp2/flesh_impact", false)],
        (WP_FLECHETTE, _) => &[("flechette/flesh_impact", false)],
        (WP_ROCKET_LAUNCHER, _) => &[("rocket/explosion", false)],
        (WP_THERMAL, _) => &[("thermal/explosion", false), ("thermal/shockwave", true)],
        _ => &[],
    }
}

/// EV_PLAY_EFFECT's effectTypes_t table (cgs.effects registrations).
fn play_effect_type(parm: i32) -> Option<&'static str> {
    Some(match parm {
        1 => "emplaced/dead_smoke",
        2 => "emplaced/explode",
        3 | 10 => "turret/explode",
        4 => "sparks/spark_explosion",
        5 => "tripMine/explosion",
        6 => "detpack/explosion",
        7 => "flechette/alt_blow",
        8 => "stunBaton/flesh_impact",
        9 => "demp2/altDetonate",
        11 => "sparks/spark_exp_nosnd",
        12 => "env/water_impact",
        13 => "env/acid_splash",
        14 => "env/lava_splash",
        15 => "materials/mud_large",
        16 => "materials/sand_large",
        17 => "materials/dirt_large",
        18 => "materials/snow_large",
        19 => "materials/gravel_large",
        _ => return None,
    })
}

/// cg_localents.c LE_PUFF: a sprite drifting on TR_LINEAR that fades and
/// grows over its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SaberTrailKey {
    entity_num: u16,
    saber_num: u8,
    blade_num: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SaberLightKey {
    entity_num: u16,
    saber_num: u8,
}

/// Per-frame OpenJK CG_DoSaberLight accumulator for sabers with 3+ blades.
/// Fixed storage avoids a per-frame heap allocation for staff lights.
#[derive(Clone, Copy, Debug)]
struct SaberLightAggregate {
    tips: [[f32; 3]; MAX_SABER_BLADES],
    count: usize,
    first_midpoint: [f32; 3],
    first_rgb: [f32; 3],
    weighted_rgb: [f32; 3],
    total_length: f32,
    diameter: f32,
}

impl Default for SaberLightAggregate {
    fn default() -> Self {
        Self {
            tips: [[0.0; 3]; MAX_SABER_BLADES],
            count: 0,
            first_midpoint: [0.0; 3],
            first_rgb: [0.0; 3],
            weighted_rgb: [0.0; 3],
            total_length: 0.0,
            diameter: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SaberTrailHistory {
    base: [f32; 3],
    tip: [f32; 3],
    last_time: i32,
}

#[derive(Clone, Debug)]
struct SaberTrailSegment {
    start_time: i32,
    end_time: i32,
    positions: [[f32; 3]; 4],
    start_uvs: [[f32; 2]; 4],
    rgba: [u8; 4],
    shader: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct SaberContactHistory {
    have_old_pos: bool,
    old_pos: [f32; 3],
    last_time: i32,
    last_sound_time: i32,
}

impl Default for SaberContactHistory {
    fn default() -> Self {
        Self {
            have_old_pos: false,
            old_pos: [0.0; 3],
            last_time: i32::MIN / 2,
            last_sound_time: i32::MIN / 2,
        }
    }
}

#[derive(Clone, Debug)]
struct SaberWallMark {
    start_time: i32,
    end_time: i32,
    positions: [[f32; 3]; 4],
    uvs: [[f32; 2]; 4],
    normal: [f32; 3],
}

#[derive(Clone, Debug)]
struct MeltPoint {
    position: [f32; 3],
    normal: [f32; 3],
    last_heated_at: i32,
    heat: f32,
    dwell_ms: f32,
    phase: f32,
}

#[derive(Clone, Debug)]
struct MeltStroke {
    points: Vec<MeltPoint>,
    last_contact_time: i32,
}

#[derive(Clone, Copy, Debug)]
struct MeltRenderSample {
    position: [f32; 3],
    normal: [f32; 3],
    heat: f32,
    dwell_ms: f32,
    phase: f32,
}

#[derive(Clone, Debug)]
struct Puff {
    start_time: i32,
    end_time: i32,
    base: [f32; 3],
    delta: [f32; 3],
    radius: f32,
    rotation: f32,
    color: [f32; 3],
    shader: &'static str,
}

pub struct WeaponFx {
    worker_tx: mpsc::Sender<FxWorkerCommand>,
    worker: Option<JoinHandle<()>>,
    cached_stats: FxStats,
    entity_fx: HashMap<u16, EntityFxState>,
    missile_fx: HashMap<u16, MissileFxState>,
    /// `0` is stock JKA: invoke continuous projectile FX every presentation
    /// frame. Non-zero values sample them at a fixed rate independent of FPS.
    continuous_fx_fps: u32,
    runner_rng: Rng,
    puffs: Vec<Puff>,
    /// Per-frame presentation primitives that do not live in the FX scheduler
    /// (notably CG_DoSaber's RT_SABER_GLOW / RT_LINE submissions).
    immediate_draws: Vec<FxDraw>,
    immediate_sounds: Vec<FxSound>,
    immediate_lights: Vec<FxLight>,
    /// OpenJK combines 3+ blade sabers into one dynamic light per saber.
    saber_multi_lights: HashMap<SaberLightKey, SaberLightAggregate>,
    modern_sabers: bool,
    saber_marks: SaberMarkMode,
    collision_world: Option<CollisionWorld>,
    saber_contact_history: HashMap<SaberTrailKey, SaberContactHistory>,
    saber_wall_marks: Vec<SaberWallMark>,
    active_melt_strokes: HashMap<SaberTrailKey, MeltStroke>,
    finished_melt_strokes: Vec<MeltStroke>,
    /// OpenJK cg_saberTrail: 0=off, 1=normal, 2=high-frequency/special mode.
    /// WGPU currently renders value 2 with the normal authored blur material
    /// rather than the legacy stencil-refraction path.
    saber_trail: i32,
    saber_trail_history: HashMap<SaberTrailKey, SaberTrailHistory>,
    saber_trail_segments: Vec<SaberTrailSegment>,
    time: i32,
    /// cg.refdef.vieworg / viewaxis[1] of the last rendered view.
    view_origin: [f32; 3],
    view_left: [f32; 3],
}

impl WeaponFx {
    pub fn new(assets: AssetSearchPath) -> Self {
        let (worker_tx, rx) = mpsc::channel();
        let worker = thread::Builder::new().name("jka-fx".to_owned()).spawn(move || fx_worker_loop(rx, assets)).expect("failed to start JKA FX worker");
        Self {
            worker_tx,
            worker: Some(worker),
            cached_stats: FxStats::default(),
            entity_fx: HashMap::new(),
            missile_fx: HashMap::new(),
            continuous_fx_fps: crate::fx::FX_FPS_DEFAULT,
            runner_rng: Rng::new(0x4658_5255_4E4E_4552),
            puffs: Vec::new(),
            immediate_draws: Vec::new(),
            immediate_sounds: Vec::new(),
            immediate_lights: Vec::new(),
            saber_multi_lights: HashMap::new(),
            modern_sabers: false,
            saber_marks: SaberMarkMode::Legacy,
            collision_world: None,
            saber_contact_history: HashMap::new(),
            saber_wall_marks: Vec::new(),
            active_melt_strokes: HashMap::new(),
            finished_melt_strokes: Vec::new(),
            saber_trail: 1,
            saber_trail_history: HashMap::new(),
            saber_trail_segments: Vec::new(),
            time: 0,
            view_origin: [0.0; 3],
            view_left: [0.0, 1.0, 0.0],
        }
    }

    pub fn stats(&self) -> crate::fx::system::FxStats {
        self.cached_stats
    }

    /// Select the optional continuous-ribbon saber presentation. The legacy
    /// path remains OpenJK-compatible and is the default.
    pub fn set_modern_sabers(&mut self, enabled: bool) {
        self.modern_sabers = enabled;
    }

    pub fn set_saber_marks(&mut self, mode: SaberMarkMode) {
        if self.saber_marks != mode {
            self.saber_marks = mode;
            self.saber_contact_history.clear();
            if !matches!(mode, SaberMarkMode::Legacy) {
                self.saber_wall_marks.clear();
            }
            if !matches!(mode, SaberMarkMode::Enhanced) {
                self.active_melt_strokes.clear();
                self.finished_melt_strokes.clear();
            }
        }
    }

    /// Presentation-only collision clone used by the saber/world contact pass.
    pub fn set_collision_world(&mut self, world: Option<CollisionWorld>) {
        self.collision_world = world;
        self.saber_contact_history.clear();
        self.saber_wall_marks.clear();
        self.active_melt_strokes.clear();
        self.finished_melt_strokes.clear();
    }

    pub fn set_saber_trail(&mut self, value: i32) {
        self.saber_trail = value.clamp(0, 2);
    }

    pub fn set_continuous_fx_fps(&mut self, value: u32) {
        let value = if value == crate::fx::FX_FPS_LEGACY_JKA {
            crate::fx::FX_FPS_LEGACY_JKA
        } else {
            value.clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX)
        };
        if self.continuous_fx_fps != value {
            self.continuous_fx_fps = value;
            // Start the new cadence from the next presented sample instead of
            // inheriting phase from the previous rate/mode.
            self.missile_fx.clear();
        }
    }

    /// Clear time-dependent presentation while retaining registered effect assets.
    /// Used by demo seeking so replayed events do not leak across the seek boundary.
    pub fn reset_for_seek(&mut self, time: i32) {
        let _ = self.worker_tx.send(FxWorkerCommand::ResetTime(time));
        self.entity_fx.clear();
        self.missile_fx.clear();
        self.puffs.clear();
        self.immediate_draws.clear();
        self.immediate_sounds.clear();
        self.immediate_lights.clear();
        self.saber_multi_lights.clear();
        self.saber_trail_history.clear();
        self.saber_trail_segments.clear();
        self.saber_contact_history.clear();
        self.saber_wall_marks.clear();
        self.active_melt_strokes.clear();
        self.finished_melt_strokes.clear();
        self.time = time;
    }

    /// FX_AdjustTime for this presentation frame (`cg.time`).
    pub fn begin_frame(&mut self, time: i32) {
        self.immediate_draws.clear();
        self.immediate_sounds.clear();
        self.immediate_lights.clear();
        self.saber_multi_lights.clear();
        if time < self.time {
            self.puffs.clear();
            self.entity_fx.clear();
            self.missile_fx.clear();
            self.saber_trail_history.clear();
            self.saber_trail_segments.clear();
            self.saber_contact_history.clear();
            self.saber_wall_marks.clear();
            self.active_melt_strokes.clear();
            self.finished_melt_strokes.clear();
        }
        self.missile_fx.retain(|_, state| {
            time <= state.last_time.saturating_add(CONTINUOUS_FX_BACKFILL_MAX_MS * 4)
        });
        self.time = time;
        let _ = self.worker_tx.send(FxWorkerCommand::AdjustTime(time));
    }

    /// The view used for puff drift and near-kill (previous render frame).
    pub fn set_view(&mut self, origin: [f32; 3], left: [f32; 3]) {
        self.view_origin = origin;
        self.view_left = left;
    }

    /// FX_AddScheduledEffects + FX_Add, then CG_AddLocalEntities' puffs.
    pub fn end_frame(&mut self) -> FxFrame {
        let (reply, recv) = mpsc::sync_channel(1);
        let mut frame = if self.worker_tx.send(FxWorkerCommand::Frame { reply }).is_ok() {
            match recv.recv() { Ok((frame, stats)) => { self.cached_stats = stats; frame }, Err(_) => FxFrame::default() }
        } else { FxFrame::default() };
        let time = self.time;
        self.saber_trail_segments.retain(|segment| {
            if time >= segment.end_time {
                return false;
            }
            // OpenJK CTrail::Update linearly advances each vertex ST toward
            // destST (which CG_AddSaberBlade sets to start U + 1), clamping
            // U at 1.0. Preserve that texture sweep over the segment life.
            let life = (segment.end_time - segment.start_time).max(1) as f32;
            let progress = ((time - segment.start_time).max(0) as f32 / life).clamp(0.0, 1.0);
            let mut uvs = segment.start_uvs;
            for uv in &mut uvs {
                uv[0] = (uv[0] + progress).min(1.0);
            }
            frame.draws.push(FxDraw::Quad {
                positions: segment.positions,
                uvs,
                rgba: segment.rgba,
                shader: segment.shader.to_owned(),
            });
            true
        });
        // Histories only need to bridge nearby samples; discard entities that
        // have not submitted a blade for several seconds.
        self.saber_trail_history
            .retain(|_, history| time <= history.last_time.saturating_add(2500));
        self.saber_contact_history
            .retain(|_, history| time <= history.last_time.saturating_add(2500));
        self.append_saber_wall_marks(&mut frame);
        if matches!(self.saber_marks, SaberMarkMode::Enhanced) {
            self.append_enhanced_melt(&mut frame);
        }
        self.flush_saber_multi_lights();
        frame.draws.append(&mut self.immediate_draws);
        frame.sounds.append(&mut self.immediate_sounds);
        frame.lights.append(&mut self.immediate_lights);
        let view = self.view_origin;
        self.puffs.retain(|puff| {
            if time >= puff.end_time {
                return false;
            }
            // CG_AddPuff.
            let c = (puff.end_time - time) as f32 / (puff.end_time - puff.start_time) as f32;
            let seconds = (time - puff.start_time) as f32 * 0.001;
            let origin = std::array::from_fn(|i| puff.base[i] + puff.delta[i] * seconds);
            let offset: [f32; 3] = std::array::from_fn(|i| origin[i] - view[i]);
            if (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt() < puff.radius {
                return false; // the view is inside the sprite
            }
            let rgb = puff.color.map(|value| (value * c).clamp(0.0, 255.0) as u8);
            frame.draws.push(FxDraw::Sprite {
                origin,
                radius: puff.radius * (1.0 - c) + 8.0,
                rotation: puff.rotation,
                rgba: [rgb[0], rgb[1], rgb[2], 0],
                shader: puff.shader.to_owned(),
            });
            true
        });
        frame
    }

    /// OpenJK CG_FX: consume the fx_runner state carried by an ET_FX snapshot.
    pub fn entity_fx(&mut self, entity: &PresentedEntity, game: &ClientGameState) {
        let state = &entity.state;
        let fx_state = state.field_i32("modelindex2").unwrap_or(FX_STATE_OFF);
        if fx_state == FX_STATE_OFF { return; }
        {
            let runtime = self.entity_fx.entry(entity.number).or_default();
            if runtime.next_time > self.time { return; }
            if fx_state < FX_STATE_ONE_SHOT_LIMIT {
                if runtime.one_shot_sequence == fx_state { return; }
                runtime.one_shot_sequence = fx_state;
            }
        }
        let delay = state.field_f32("speed").unwrap_or(0.0).max(0.0) as i32;
        let random = state.field_i32("time").unwrap_or(0).max(0);
        let jitter = (self.runner_rng.flrand(0.0, 1.0) * random as f32) as i32;
        self.entity_fx.entry(entity.number).or_default().next_time = self.time.saturating_add(delay).saturating_add(jitter);
        let effect_index = state.field_i32("modelindex").unwrap_or(0);
        let Some(name) = game.effect_qpath(effect_index) else { return; };
        let mut dir = angles_to_axis(super::entity_vec3(state, "angles").unwrap_or([0.0; 3]))[0];
        if dir == [0.0; 3] { dir[1] = 1.0; }
        let _ = self.play(&name, entity.origin, dir);
    }

    /// OpenJK CG_DoSaber / CG_DoSaberLight dynamic-light submission. Sabers
    /// with fewer than three blades emit one light per visible blade; staffs
    /// and other 3+ blade sabers accumulate into one light for the whole saber.
    fn saber_dynamic_light(
        &mut self,
        key: SaberTrailKey,
        origin: [f32; 3],
        direction: [f32; 3],
        visible_length: f32,
        authored_length: f32,
        color: i32,
        num_blades: u8,
        no_dlight: bool,
    ) {
        if no_dlight {
            return;
        }

        let direction = normalize3(direction);
        if num_blades < 3 {
            if visible_length < 0.5 {
                return;
            }
            // CG_DoSaber initializes rgb to white before CG_RGBForSaberColor.
            let rgb = saber_light_rgb(color).unwrap_or([1.0; 3]);
            // OpenJK CG_DoSaber: midpoint of the actually rendered/clipped
            // blade, radius = length * 1.4 + Q_flrand(0, 1) * 3.
            let midpoint = madd3(origin, direction, visible_length * 0.5);
            let radius = visible_length * 1.4 + self.runner_rng.flrand(0.0, 1.0) * 3.0;
            self.immediate_lights.push(FxLight { origin: midpoint, radius, rgb });
            return;
        }

        // OpenJK CG_DoSaberLight reads saberInfo_t blade lengths/bolts rather
        // than CG_AddSaberBlade's wall-clipped local saberLen. Mirror that for
        // 3+ blade sabers and combine them once at end_frame.
        let length = authored_length.max(0.0);
        if length < 0.5 {
            return;
        }
        // CG_DoSaberLight's rgbs array is zero-initialized before the helper,
        // so an unknown color remains black there (unlike CG_DoSaber above).
        let rgb = saber_light_rgb(color).unwrap_or([0.0; 3]);
        let tip = madd3(origin, direction, length);
        let light_key = SaberLightKey { entity_num: key.entity_num, saber_num: key.saber_num };
        let aggregate = self.saber_multi_lights.entry(light_key).or_default();
        if aggregate.count >= MAX_SABER_BLADES {
            return;
        }
        if aggregate.count == 0 {
            aggregate.first_midpoint = madd3(origin, direction, length * 0.5);
            aggregate.first_rgb = rgb;
        }
        aggregate.tips[aggregate.count] = tip;
        aggregate.count += 1;
        aggregate.total_length += length;
        for channel in 0..3 {
            aggregate.weighted_rgb[channel] += rgb[channel] * length;
        }
        aggregate.diameter = aggregate.diameter.max(length * 2.0);
    }

    fn flush_saber_multi_lights(&mut self) {
        // Drain rather than replace the map so staff-heavy scenes retain the
        // small allocation across frames instead of churning the heap.
        let lights = &mut self.immediate_lights;
        let rng = &mut self.runner_rng;
        for (_, aggregate) in self.saber_multi_lights.drain() {
            if aggregate.count == 0 || aggregate.total_length <= 0.0 {
                continue;
            }

            let (origin, rgb) = if aggregate.count == 1 {
                (aggregate.first_midpoint, aggregate.first_rgb)
            } else {
                let mut midpoint = [0.0; 3];
                for tip in &aggregate.tips[..aggregate.count] {
                    midpoint = add3(midpoint, *tip);
                }
                let inv_count = 1.0 / aggregate.count as f32;
                midpoint = midpoint.map(|component| component * inv_count);
                let inv_length = 1.0 / aggregate.total_length;
                let rgb = aggregate.weighted_rgb.map(|component| component * inv_length);
                (midpoint, rgb)
            };

            // OpenJK starts with the longest blade diameter, then expands to
            // the farthest pair of blade tips.
            let mut diameter = aggregate.diameter;
            for i in 0..aggregate.count {
                for j in 0..aggregate.count {
                    diameter = diameter.max(length3(sub3(aggregate.tips[i], aggregate.tips[j])));
                }
            }
            let radius = diameter + rng.flrand(0.0, 1.0) * 8.0;
            lights.push(FxLight { origin, radius, rgb });
        }
    }

    /// Force-power visuals requested by CG_Player.
    pub fn player_fx(&mut self, request: &PlayerFxRequest) {
        match request {
            PlayerFxRequest::Effect { name, origin, axis } => {
                let _ = self.worker_tx.send(FxWorkerCommand::PlayAxis { name: (*name).to_owned(), origin: *origin, axis: *axis });
            }
            PlayerFxRequest::SaberBlade {
                origin,
                direction,
                length,
                radius,
                color,
                entity_alpha,
                entity_num,
                saber_num,
                blade_num,
                saber_move,
                torso_anim,
                saber_in_flight,
                trail_style,
                num_blades,
                no_dlight,
                no_wall_marks,
            } => {
                let key = SaberTrailKey {
                    entity_num: *entity_num,
                    saber_num: *saber_num,
                    blade_num: *blade_num,
                };
                let clipped_length = self.saber_world_contact(
                    key,
                    *origin,
                    *direction,
                    *length,
                    *no_wall_marks,
                );
                self.saber_dynamic_light(
                    key,
                    *origin,
                    *direction,
                    clipped_length,
                    *length,
                    *color,
                    *num_blades,
                    *no_dlight,
                );
                self.saber_trail(
                    key,
                    *origin,
                    *direction,
                    clipped_length,
                    *color,
                    *entity_alpha,
                    *saber_move,
                    *torso_anim,
                    *saber_in_flight,
                    *trail_style,
                );
                self.saber_blade(
                    *origin,
                    *direction,
                    clipped_length,
                    *radius,
                    *color,
                    *entity_alpha,
                );
            }
            PlayerFxRequest::PushPuffs { origin } => {
                // CG_ForcePushBlur (LE_PUFF path).
                self.puff(*origin, 55.0, 0.0, [24.0, 32.0, 40.0], "gfx/effects/forcePush");
                self.puff(*origin, -55.0, 180.0, [24.0, 32.0, 40.0], "gfx/effects/forcePush");
            }
            PlayerFxRequest::GripPuffs { origin } => {
                // CG_ForceGripEffect.
                let wv = (self.time as f32 * 0.004).sin() * 0.08 + 0.1;
                self.puff(*origin, 55.0, 0.0, [(200.0 + wv * 255.0).min(255.0), 0.0, 0.0], "gfx/effects/forcePush");
                self.puff(*origin, -55.0, 180.0, [255.0; 3], "gfx/effects/sabers/red_glow");
            }
        }
    }

    fn finish_melt_stroke(&mut self, key: SaberTrailKey) {
        let Some(stroke) = self.active_melt_strokes.remove(&key) else {
            return;
        };
        if !stroke.points.is_empty() {
            self.finished_melt_strokes.push(stroke);
            if self.finished_melt_strokes.len() > MELT_MAX_FINISHED_STROKES {
                let excess = self.finished_melt_strokes.len() - MELT_MAX_FINISHED_STROKES;
                self.finished_melt_strokes.drain(..excess);
            }
        }
    }

    fn deposit_enhanced_melt(
        &mut self,
        key: SaberTrailKey,
        position: [f32; 3],
        normal: [f32; 3],
    ) -> f32 {
        let now = self.time;
        let should_break = self.active_melt_strokes.get(&key).is_some_and(|stroke| {
            let stale = now.saturating_sub(stroke.last_contact_time) > MELT_STROKE_BREAK_MS;
            let plane_changed = stroke
                .points
                .last()
                .is_some_and(|point| dot3(point.normal, normal) <= 0.45);
            stale || plane_changed
        });
        if should_break {
            self.finish_melt_stroke(key);
        }

        let stroke = self.active_melt_strokes.entry(key).or_insert_with(|| MeltStroke {
            points: Vec::new(),
            last_contact_time: now,
        });
        let dt_ms = now.saturating_sub(stroke.last_contact_time).clamp(0, 50) as f32;
        stroke.last_contact_time = now;

        if stroke.points.is_empty() {
            stroke.points.push(MeltPoint {
                position,
                normal,
                last_heated_at: now,
                heat: 0.48,
                dwell_ms: dt_ms.max(8.0),
                phase: (position[0] * 0.071 + position[1] * 0.113 + position[2] * 0.047).sin() * 3.1,
            });
            return 0.48;
        }

        let last = stroke.points.last_mut().unwrap();
        let distance = length3(sub3(position, last.position));

        if distance <= MELT_HOLD_RADIUS {
            let cooled = cooled_heat(last, now);
            last.heat = (cooled + dt_ms * 0.00075).min(1.5);
            last.dwell_ms = (last.dwell_ms + dt_ms).min(12_000.0);
            let blend = (0.10 + dt_ms * 0.002).clamp(0.10, 0.24);
            last.position = lerp3(last.position, position, blend);
            last.normal = normalize3(lerp3(last.normal, normal, blend));
            last.last_heated_at = now;
            return last.heat;
        }

        let start = last.position;
        let start_normal = last.normal;
        let steps = ((distance / MELT_POINT_SPACING).ceil() as usize).clamp(1, 16);
        let mut newest_heat = 0.52;
        for step in 1..=steps {
            if stroke.points.len() >= MELT_MAX_POINTS_PER_STROKE {
                break;
            }
            let t = step as f32 / steps as f32;
            let point = lerp3(start, position, t);
            let point_normal = normalize3(lerp3(start_normal, normal, t));
            let phase = (point[0] * 0.071 + point[1] * 0.113 + point[2] * 0.047).sin() * 3.1;
            newest_heat = 0.50 + 0.10 * (phase * 1.7).sin().abs();
            stroke.points.push(MeltPoint {
                position: point,
                normal: point_normal,
                last_heated_at: now,
                heat: newest_heat,
                dwell_ms: dt_ms.min(35.0),
                phase,
            });
        }
        newest_heat
    }

    fn saber_world_contact(
        &mut self,
        key: SaberTrailKey,
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        no_wall_marks: bool,
    ) -> f32 {
        if matches!(self.saber_marks, SaberMarkMode::Off) || length < 0.5 {
            self.saber_contact_history.remove(&key);
            self.finish_melt_stroke(key);
            return length;
        }
        let direction = normalize3(direction);
        let end = madd3(origin, direction, length + 1.0);
        let Some(world) = self.collision_world.as_mut() else {
            self.finish_melt_stroke(key);
            return length;
        };
        let hit = world.trace(TraceQuery {
            start: origin,
            mins: [0.0; 3],
            maxs: [0.0; 3],
            end,
            pass_entity: i32::from(key.entity_num),
            mask: SABER_CONTACT_MASK,
        });
        if hit.fraction >= 1.0 {
            if let Some(history) = self.saber_contact_history.get_mut(&key) {
                history.have_old_pos = false;
                history.last_time = self.time;
            }
            self.finish_melt_stroke(key);
            return length;
        }

        let clipped_length = (length * hit.fraction).max(0.1);
        let hit_normal = normalize3(hit.normal);
        let can_impact = hit.surface_flags & SURF_NOIMPACT == 0;
        let can_mark = can_impact
            && hit.surface_flags & SURF_NOMARKS == 0
            && hit.entity == ENTITY_WORLD
            && !no_wall_marks;
        let previous = self.saber_contact_history.get(&key).copied().unwrap_or_default();
        let mut next = previous;
        next.last_time = self.time;

        // OpenJK performs saber/world contact once per presentation frame. The
        // cg_fxFPS knob is for scheduled/continuous .efx emitters, not marks.
        if !no_wall_marks && can_impact {
            self.play("sparks/spark_nosnd", hit.end, hit_normal);
        }

        match self.saber_marks {
            SaberMarkMode::Legacy => {
                self.finish_melt_stroke(key);
                if can_mark && previous.have_old_pos {
                    if length3(sub3(hit.end, previous.old_pos)) > 1.0e-4 {
                        self.push_legacy_saber_wall_mark(previous.old_pos, hit.end, hit_normal);
                    }
                    // OpenJK uses blade.hitWallDebounceTime and allows another
                    // wall-hit sound every 100 ms while continuous contact lasts.
                    if self.time.saturating_sub(previous.last_sound_time) >= 100 {
                        let variant = self.runner_rng.irand(1, 3);
                        self.immediate_sounds.push(FxSound {
                            origin: hit.end,
                            qpath: format!("sound/weapons/saber/saberhitwall{variant}.wav"),
                        });
                        next.last_sound_time = self.time;
                    }
                }
            }
            SaberMarkMode::Enhanced => {
                if can_mark {
                    let heat = self.deposit_enhanced_melt(key, hit.end, hit_normal);
                    if previous.have_old_pos && self.time.saturating_sub(previous.last_sound_time) >= 100 {
                        let variant = self.runner_rng.irand(1, 3);
                        self.immediate_sounds.push(FxSound {
                            origin: hit.end,
                            qpath: format!("sound/weapons/saber/saberhitwall{variant}.wav"),
                        });
                        next.last_sound_time = self.time;
                    }
                    let flicker = 0.94 + (self.time as f32 * 0.029).sin() * 0.06;
                    let heat01 = (heat / 1.25).clamp(0.0, 1.0);
                    self.immediate_lights.push(FxLight {
                        origin: madd3(hit.end, hit_normal, 1.1),
                        radius: 48.0 + 72.0 * heat01,
                        rgb: [1.0 * flicker, (0.24 + 0.24 * heat01) * flicker, (0.025 + 0.055 * heat01) * flicker],
                    });
                } else {
                    self.finish_melt_stroke(key);
                }
            }
            SaberMarkMode::Off => {}
        }

        next.have_old_pos = can_mark;
        next.old_pos = hit.end;
        self.saber_contact_history.insert(key, next);
        clipped_length
    }

    fn push_legacy_saber_wall_mark(&mut self, start: [f32; 3], end: [f32; 3], normal: [f32; 3]) -> bool {
        // Legacy keeps OpenJK's 0.65 radius and centered rivet/glow UVs. The
        // small normal lift substitutes for q3 polygonOffset in the transient FX path.
        let Some((positions, uvs)) = saber_mark_ribbon(start, end, normal, 0.65, 0.24) else {
            return false;
        };
        self.saber_wall_marks.push(SaberWallMark {
            start_time: self.time,
            end_time: self.time + SABER_MARK_LIFETIME_MS,
            positions,
            uvs,
            normal,
        });
        if self.saber_wall_marks.len() > SABER_MARK_MAX {
            let excess = self.saber_wall_marks.len() - SABER_MARK_MAX;
            self.saber_wall_marks.drain(..excess);
        }
        true
    }

    fn append_saber_wall_marks(&mut self, frame: &mut FxFrame) {
        let time = self.time;
        let mut retained = Vec::with_capacity(self.saber_wall_marks.len());
        for mark in self.saber_wall_marks.drain(..) {
            if time >= mark.end_time {
                continue;
            }
            let remaining = mark.end_time - time;
            let burn_fade = if remaining < SABER_MARK_FADE_MS {
                remaining as f32 / SABER_MARK_FADE_MS as f32
            } else {
                1.0
            };
            let burn = (255.0 * burn_fade.clamp(0.0, 1.0)).round() as u8;
            frame.draws.push(FxDraw::Quad {
                positions: mark.positions,
                uvs: mark.uvs,
                rgba: [burn, burn, burn, burn],
                shader: "gfx/damage/rivetmark".to_owned(),
            });

            let age = (time - mark.start_time).max(0);
            if age < SABER_GLOW_LIFETIME_MS {
                let fade = if age <= 500 {
                    1.0
                } else {
                    1.0 - (age - 500) as f32 / (SABER_GLOW_LIFETIME_MS - 500) as f32
                };
                frame.draws.push(FxDraw::Quad {
                    positions: mark.positions.map(|point| madd3(point, mark.normal, 0.035)),
                    uvs: mark.uvs,
                    rgba: [
                        (235.0 * fade.max(0.0)) as u8,
                        (118.0 * fade.max(0.0)) as u8,
                        (12.0 * fade.max(0.0)) as u8,
                        255,
                    ],
                    shader: "gfx/effects/saberDamageGlow".to_owned(),
                });
            }
            retained.push(mark);
        }
        self.saber_wall_marks = retained;
    }

    fn append_enhanced_melt(&mut self, frame: &mut FxFrame) {
        let time = self.time;
        let stale_keys = self
            .active_melt_strokes
            .iter()
            .filter_map(|(key, stroke)| {
                (time.saturating_sub(stroke.last_contact_time) > MELT_STROKE_BREAK_MS).then_some(*key)
            })
            .collect::<Vec<_>>();
        for key in stale_keys {
            self.finish_melt_stroke(key);
        }
        self.finished_melt_strokes.retain(|stroke| {
            time <= stroke.last_contact_time.saturating_add(MELT_SCAR_LIFETIME_MS)
        });

        let append = |stroke: &MeltStroke, frame: &mut FxFrame| {
            let samples = smooth_melt_samples(stroke, time);
            if samples.is_empty() {
                return;
            }
            // Cooled, darkened substrate first; all hot layers are separate
            // additive passes on top. This gives a large highlight/shadow range
            // instead of merely tinting the legacy quad orange.
            if let Some(draw) = melt_strip_mesh(&samples, time, MeltLayer::Scar) {
                frame.draws.push(draw);
            }
            let hottest = samples.iter().fold(0.0_f32, |max_heat, sample| max_heat.max(sample.heat));
            // Once a stroke is cold, submit only the persistent scar. Avoid
            // rebuilding zero-colour additive meshes for old finished strokes.
            if hottest > 0.015 {
                if let Some(draw) = melt_strip_mesh(&samples, time, MeltLayer::Halo) {
                    frame.draws.push(draw);
                }
                if let Some(draw) = melt_strip_mesh(&samples, time, MeltLayer::Relief) {
                    frame.draws.push(draw);
                }
                if let Some(draw) = melt_strip_mesh(&samples, time, MeltLayer::Core) {
                    frame.draws.push(draw);
                }
                if hottest > 0.62 {
                    if let Some(draw) = melt_strip_mesh(&samples, time, MeltLayer::WhiteCore) {
                        frame.draws.push(draw);
                    }
                }
            }

            // Dwell points become smooth molten pools. The pool grows and sags
            // under gravity while the blade is held in one place, then the hot
            // layer shrinks/cools while the dark relief remains much longer.
            for point in &stroke.points {
                if point.dwell_ms < 180.0 {
                    continue;
                }
                let sample = MeltRenderSample {
                    position: point.position,
                    normal: point.normal,
                    heat: cooled_heat(point, time),
                    dwell_ms: point.dwell_ms,
                    phase: point.phase,
                };
                if let Some(draw) = melt_blob_mesh(sample, time, false) {
                    frame.draws.push(draw);
                }
                if let Some(draw) = melt_blob_mesh(sample, time, true) {
                    frame.draws.push(draw);
                }
                if let Some(draw) = melt_drip_mesh(sample, time) {
                    frame.draws.push(draw);
                }
            }
        };

        for stroke in &self.finished_melt_strokes {
            append(stroke, frame);
        }
        for stroke in self.active_melt_strokes.values() {
            append(stroke, frame);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn saber_trail(
        &mut self,
        key: SaberTrailKey,
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        color: i32,
        entity_alpha: f32,
        saber_move: i32,
        torso_anim: i32,
        saber_in_flight: bool,
        trail_style: i32,
    ) {
        if self.saber_trail == 0 || trail_style > 1 || length < 0.5 {
            return;
        }

        let direction = normalize3(direction);
        let tip = madd3(origin, direction, length + 3.0);
        let now = self.time;
        let previous = self.saber_trail_history.get(&key).copied();
        let sample_due = previous
            .map(|history| self.saber_trail == 2 || now > history.last_time.saturating_add(2))
            .unwrap_or(true);
        if !sample_due {
            return;
        }

        if let Some(history) = previous {
            let move_trail_length = jka_movement::saber_move_trail_length(saber_move);
            let super_break = jka_movement::super_break_win_anim(torso_anim);
            let active = super_break || move_trail_length > 0 || saber_in_flight;
            let fresh = now < history.last_time.saturating_add(2000);
            let diff = now.saturating_sub(history.last_time);
            if active && fresh && diff <= 10_000 {
                // OpenJK: duration = saberMoveData[].trailLength / 5, with a
                // 150 ms super-break fallback and SABER_TRAIL_TIME (40 ms)
                // otherwise. trailStyle 1 uses swordTrail for twice as long.
                let mut duration = if move_trail_length > 0 {
                    move_trail_length as f32 / 5.0
                } else if super_break {
                    150.0
                } else {
                    40.0
                };
                let (shader, rgb) = if trail_style == 1 {
                    duration *= 2.0;
                    ("gfx/effects/sabers/swordTrail", [32u8, 32, 32])
                } else {
                    ("gfx/effects/sabers/saberBlur", saber_trail_rgb(color))
                };
                let duration = duration.max(1.0);
                // Exact CG_AddSaberBlade winding/UV progression: new base,
                // new tip, old tip, old base. clampmap handles U beyond 1.
                let old_u = diff as f32 / duration;
                let intensity = entity_alpha.clamp(0.0, 1.0);
                let rgb = rgb.map(|c| (f32::from(c) * intensity).round().clamp(0.0, 255.0) as u8);
                self.saber_trail_segments.push(SaberTrailSegment {
                    start_time: now,
                    end_time: now.saturating_add(duration.round() as i32),
                    positions: [origin, tip, history.tip, history.base],
                    start_uvs: [[0.0, 1.0], [0.0, 0.0], [old_u, 0.0], [old_u, 1.0]],
                    rgba: [rgb[0], rgb[1], rgb[2], 255],
                    shader,
                });
            }
        }

        // OpenJK stores the newest base/tip even when the current move does
        // not draw a trail, so the next attack connects from a fresh sample.
        self.saber_trail_history.insert(
            key,
            SaberTrailHistory {
                base: origin,
                tip,
                last_time: now,
            },
        );
    }

    fn saber_blade(
        &mut self,
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        radius: f32,
        color: i32,
        entity_alpha: f32,
    ) {
        if length < 0.5 || radius <= 0.0 {
            return;
        }
        let direction = normalize3(direction);
        let tip = madd3(origin, direction, length);
        let line_base = madd3(origin, direction, -1.0);
        let (glow_shader, line_shader) = saber_shaders(color);
        // GL_ONE GL_ONE has no source-alpha weighting. RF_FORCE_ENT_ALPHA-like
        // fades therefore scale vertex RGB, which is what rgbGen vertex uses.
        let intensity = (entity_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        let white = [intensity, intensity, intensity, 255];

        if self.modern_sabers {
            // Continuous camera-facing capsule: smoother than the legacy bead
            // chain while still using the stock authored saber materials.
            let soft = (f32::from(intensity) * 0.55).round() as u8;
            self.immediate_draws.push(FxDraw::Line {
                start: line_base,
                end: tip,
                width: radius * 1.8,
                rgba: [soft, soft, soft, 255],
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Line {
                start: line_base,
                end: tip,
                width: radius * 0.95,
                rgba: white,
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Sprite {
                origin,
                radius: (radius * 1.5).max(5.5),
                rotation: 0.0,
                rgba: [soft, soft, soft, 255],
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Sprite {
                origin: tip,
                radius: radius * 0.9,
                rotation: 0.0,
                rgba: [soft, soft, soft, 255],
                shader: glow_shader.to_owned(),
            });
        } else {
            // OpenJK RB_SurfaceSaberGlow: march view-facing glow sprites from
            // blade tip to hilt, growing the radius slightly each step.
            let mut distance = length;
            let mut glow_radius = radius;
            while distance > 0.0 {
                self.immediate_draws.push(FxDraw::Sprite {
                    origin: madd3(origin, direction, distance),
                    radius: glow_radius,
                    rotation: 0.0,
                    rgba: white,
                    shader: glow_shader.to_owned(),
                });
                distance -= (glow_radius * 0.65).max(0.05);
                glow_radius += 0.017;
            }
            self.immediate_draws.push(FxDraw::Sprite {
                origin,
                radius: 5.5,
                rotation: 0.0,
                rgba: white,
                shader: glow_shader.to_owned(),
            });
        }

        // OpenJK CG_DoSaber uses an RT_LINE from blade tip to one unit behind
        // the hilt origin, at radius / 3, with the color-specific line shader.
        self.immediate_draws.push(FxDraw::Line {
            start: tip,
            end: line_base,
            width: (radius / 3.0).max(0.01),
            rgba: white,
            shader: line_shader.to_owned(),
        });
    }

    fn puff(&mut self, origin: [f32; 3], speed: f32, rotation: f32, color: [f32; 3], shader: &'static str) {
        self.puffs.push(Puff {
            start_time: self.time,
            end_time: self.time + 120,
            base: origin,
            delta: self.view_left.map(|v| v * speed),
            radius: 2.0,
            rotation,
            color,
            shader,
        });
    }

    fn play(&mut self, name: &str, origin: [f32; 3], dir: [f32; 3]) -> bool {
        self.worker_tx
            .send(FxWorkerCommand::PlayDir { name: name.to_owned(), origin, dir })
            .is_ok()
    }

    fn missile_fx_samples(
        &mut self,
        entity_number: u16,
        generation: MissileFxGeneration,
        time: i32,
        origin: [f32; 3],
    ) -> Vec<[f32; 3]> {
        if self.continuous_fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
            return vec![origin];
        }

        let period = 1000.0 / f64::from(self.continuous_fx_fps.max(1));
        use std::collections::hash_map::Entry;
        let state = match self.missile_fx.entry(entity_number) {
            Entry::Vacant(entry) => {
                entry.insert(MissileFxState {
                    generation,
                    last_time: time,
                    last_origin: origin,
                    next_sample_time: f64::from(time) + period,
                });
                return vec![origin];
            }
            Entry::Occupied(entry) => entry.into_mut(),
        };

        let elapsed = time.saturating_sub(state.last_time);
        if state.generation != generation
            || time < state.last_time
            || elapsed > CONTINUOUS_FX_BACKFILL_MAX_MS
        {
            *state = MissileFxState {
                generation,
                last_time: time,
                last_origin: origin,
                next_sample_time: f64::from(time) + period,
            };
            return vec![origin];
        }

        if time == state.last_time {
            state.last_origin = origin;
            return Vec::new();
        }

        let start_time = f64::from(state.last_time);
        let end_time = f64::from(time);
        let duration = end_time - start_time;
        let previous_origin = state.last_origin;
        let mut samples = Vec::new();
        while state.next_sample_time <= end_time + 1.0e-6 {
            let alpha = ((state.next_sample_time - start_time) / duration).clamp(0.0, 1.0) as f32;
            let sample: [f32; 3] = std::array::from_fn(|axis| {
                previous_origin[axis] + (origin[axis] - previous_origin[axis]) * alpha
            });
            samples.push(sample);
            state.next_sample_time += period;
        }
        state.last_time = time;
        state.last_origin = origin;
        samples
    }

    /// CG_Missile: play the weapon's trail effect for this frame and return
    /// the missile model refEntity, if the weapon has one.
    pub fn missile(&mut self, entity: &PresentedEntity, game: &ClientGameState, time: i32) -> Option<MissileModel> {
        let state = &entity.state;
        let weapon = state.field_i32("weapon").unwrap_or(0);
        if weapon == WP_SABER || weapon == G2_MODEL_PART {
            return None; // thrown sabers / G2 parts: owned by the player presenter
        }
        let e_flags = state.field_i32("eFlags").unwrap_or(0);
        let alt = e_flags & EF_ALT_FIRING != 0;
        let delta = super::entity_vec3(state, "pos.trDelta").unwrap_or([0.0; 3]);
        let forward = normalize_or_up(delta);

        let other2 = state.field_i32("otherEntityNum2").unwrap_or(0);
        let generation = MissileFxGeneration {
            trajectory_time: state.field_i32("pos.trTime").unwrap_or(0),
            weapon,
            custom_effect: other2,
            alt_fire: alt,
        };
        if other2 != 0 {
            // An over-ridden trail effect (cgs.gameEffects = CS_EFFECTS).
            if e_flags & EF_JETPACK_ACTIVE == 0 {
                if let Some(name) = game.effect_qpath(other2) {
                    let samples =
                        self.missile_fx_samples(entity.number, generation, time, entity.origin);
                    for origin in samples {
                        self.play(&name, origin, forward);
                    }
                }
            }
            return None;
        }

        if let Some(name) = trail_effect(weapon, alt) {
            let samples = self.missile_fx_samples(entity.number, generation, time, entity.origin);
            for origin in samples {
                if alt && (weapon == WP_BRYAR_PISTOL || weapon == WP_BRYAR_OLD) {
                    // FX_BryarAltProjectileThink: one crackle per charge level.
                    for _ in 1..state.field_i32("generic1").unwrap_or(0) {
                        self.play("bryar/crackleShot", origin, forward);
                    }
                }
                self.play(name, origin, forward);
            }
        }

        let qpath = missile_model(weapon, alt)?;
        let axis = if state.field_i32("apos.trType").unwrap_or(0) != TR_INTERPOLATE {
            let stick = e_flags & EF_MISSILE_STICK != 0;
            let yaw = if state.field_i32("pos.trType").unwrap_or(0) != TR_STATIONARY {
                time as f32 * if stick { 0.5 } else { 0.25 }
            } else if stick {
                state.field_i32("pos.trTime").unwrap_or(0) as f32 * 0.5
            } else {
                state.field_i32("time").unwrap_or(0) as f32
            };
            jka_movement::rotate_around_direction(forward, yaw)
        } else {
            // lerpAngles is a copy of s1->angles for missiles.
            angles_to_axis(super::entity_vec3(state, "angles").unwrap_or([0.0; 3]))
        };
        Some(MissileModel { qpath, origin: entity.origin, axis })
    }

    /// The FX part of CG_EntityEvent.
    pub fn entity_event(&mut self, event: &PresentationEvent, game: &ClientGameState) -> Option<EventDispatchResult> {
        let state = &event.state;
        match event.event {
            EV_MISSILE_HIT | EV_MISSILE_MISS | EV_MISSILE_MISS_METAL => {
                let dir = jka_movement::byte_to_dir(event.parm);
                let custom = state.field_i32("emplacedOwner").unwrap_or(0);
                if custom != 0 {
                    // Hack in OpenJK too: an index to a custom impact effect.
                    let played = game.effect_qpath(custom).is_some_and(|name| self.play(&name, event.position, dir));
                    return Some(if played { EventDispatchResult::Handled("FX_CUSTOM_IMPACT") } else { EventDispatchResult::Partial("FX_CUSTOM_IMPACT_MISSING") });
                }
                if state.field_i32("eFlags").unwrap_or(0) & EF_JETPACK_ACTIVE != 0 && state.field_i32("otherEntityNum2").unwrap_or(0) != 0 {
                    return Some(EventDispatchResult::Partial("FX_VEHICLE_WEAPON_IMPACT_PENDING"));
                }
                let weapon = state.field_i32("weapon").unwrap_or(0);
                let alt = state.field_i32("eFlags").unwrap_or(0) & EF_ALT_FIRING != 0;
                let effects = if event.event == EV_MISSILE_HIT {
                    player_impacts(weapon, alt)
                } else {
                    wall_impacts(weapon, alt, if alt { state.field_i32("generic1").unwrap_or(0) } else { 0 })
                };
                let mut played = false;
                for &(name, up) in effects {
                    played |= self.play(name, event.position, if up { [0.0, 0.0, 1.0] } else { dir });
                }
                Some(if played { EventDispatchResult::Handled("FX_MISSILE_IMPACT") } else { EventDispatchResult::Partial("FX_MISSILE_IMPACT_NONE") })
            }
            EV_PLAY_EFFECT => {
                let Some(name) = play_effect_type(event.parm) else {
                    return Some(EventDispatchResult::Partial("FX_PLAY_EFFECT_TYPE_UNKNOWN"));
                };
                let mut dir = super::entity_vec3(state, "angles").unwrap_or([0.0; 3]);
                if dir == [0.0; 3] {
                    dir[1] = 1.0;
                }
                let origin = super::entity_vec3(state, "origin").unwrap_or(event.position);
                Some(if self.play(name, origin, dir) { EventDispatchResult::Handled("FX_PLAY_EFFECT") } else { EventDispatchResult::Partial("FX_EFFECT_MISSING") })
            }
            EV_PLAY_EFFECT_ID | EV_PLAY_PORTAL_EFFECT_ID => {
                let mut dir = angles_to_axis(super::entity_vec3(state, "angles").unwrap_or([0.0; 3]))[0];
                if dir == [0.0; 3] {
                    dir[1] = 1.0;
                }
                let Some(name) = game.effect_qpath(event.parm) else {
                    return Some(EventDispatchResult::Partial("EFFECT_RESOURCE_MISSING"));
                };
                // Portal effects belong to the sky-portal scene, which is not
                // rendered separately; they play in the main scene.
                Some(if self.play(&name, event.position, dir) { EventDispatchResult::Handled("FX_PLAY_EFFECT_ID") } else { EventDispatchResult::Partial("FX_EFFECT_MISSING") })
            }
            _ => None,
        }
    }
}

fn saber_trail_rgb(color: i32) -> [u8; 3] {
    match color {
        0 => [255, 0, 0],
        1 => [255, 64, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 64, 255],
        5 => [220, 0, 255],
        _ => [0, 64, 255],
    }
}

impl Drop for WeaponFx {
    fn drop(&mut self) {
        let _ = self.worker_tx.send(FxWorkerCommand::Shutdown);
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

fn normalize_or_up(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length == 0.0 { [0.0, 0.0, 1.0] } else { v.map(|c| c / length) }
}

fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    [
        [cp * cy, cp * sy, -sp],
        [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp],
        [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missile_generation() -> MissileFxGeneration {
        MissileFxGeneration {
            trajectory_time: 900,
            weapon: WP_BLASTER,
            custom_effect: 0,
            alt_fire: false,
        }
    }

    #[test]
    fn fixed_continuous_fx_rate_is_independent_of_presentation_frames() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.set_continuous_fx_fps(90);
        let generation = missile_generation();

        assert_eq!(
            fx.missile_fx_samples(7, generation, 1000, [0.0, 0.0, 0.0]),
            vec![[0.0, 0.0, 0.0]],
            "first presentation seeds one visible sample"
        );
        assert!(
            fx.missile_fx_samples(7, generation, 1005, [5.0, 0.0, 0.0]).is_empty(),
            "rendering again before the 90 Hz tick must not add density"
        );

        let sample = fx.missile_fx_samples(7, generation, 1012, [12.0, 0.0, 0.0]);
        assert_eq!(sample.len(), 1);
        assert!((sample[0][0] - 11.111_111).abs() < 0.001);

        let samples = fx.missile_fx_samples(7, generation, 1035, [35.0, 0.0, 0.0]);
        assert_eq!(samples.len(), 2);
        assert!((samples[0][0] - 22.222_221).abs() < 0.001);
        assert!((samples[1][0] - 33.333_332).abs() < 0.001);
    }

    #[test]
    fn legacy_jka_continuous_fx_samples_every_presentation() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.set_continuous_fx_fps(crate::fx::FX_FPS_LEGACY_JKA);
        let generation = missile_generation();

        for (time, x) in [(1000, 0.0), (1001, 1.0), (1001, 1.25), (1002, 2.0)] {
            assert_eq!(
                fx.missile_fx_samples(9, generation, time, [x, 0.0, 0.0]),
                vec![[x, 0.0, 0.0]],
            );
        }
    }

    #[test]
    fn push_puffs_follow_cg_add_puff() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.set_view([1000.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        fx.begin_frame(1000);
        fx.player_fx(&PlayerFxRequest::PushPuffs { origin: [0.0; 3] });
        let frame = fx.end_frame();
        assert_eq!(frame.draws.len(), 2);
        let FxDraw::Sprite { radius, rgba, .. } = &frame.draws[0] else { panic!() };
        assert_eq!((*radius, *rgba), (8.0, [24, 32, 40, 0]), "c = 1 at spawn");
        fx.begin_frame(1060);
        let frame = fx.end_frame();
        let FxDraw::Sprite { origin, radius, rgba, .. } = &frame.draws[0] else { panic!() };
        assert!((origin[1] - 3.3).abs() < 1e-4, "drifts along the view's left at 55 u/s");
        assert!((radius - 9.0).abs() < 1e-5 && rgba[0] == 12, "half faded, half grown");
        fx.begin_frame(1120);
        assert!(fx.end_frame().draws.is_empty(), "120 ms life");
    }

    #[test]
    fn saber_blade_uses_stock_openjk_materials_and_optional_ribbon() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        let blade = PlayerFxRequest::SaberBlade {
            origin: [1.0, 2.0, 3.0],
            direction: [1.0, 0.0, 0.0],
            length: 10.0,
            radius: 2.0,
            color: 0,
            entity_alpha: 1.0,
            entity_num: 7,
            saber_num: 0,
            blade_num: 0,
            saber_move: 0,
            torso_anim: 0,
            saber_in_flight: false,
            trail_style: 0,
            num_blades: 1,
            no_dlight: false,
            no_wall_marks: false,
        };

        fx.begin_frame(1000);
        fx.player_fx(&blade);
        let classic = fx.end_frame();
        assert_eq!(classic.lights.len(), 1, "CG_DoSaber emits one dlight for a single blade");
        assert_eq!(classic.lights[0].origin, [6.0, 2.0, 3.0]);
        assert_eq!(classic.lights[0].rgb, [1.0, 0.2, 0.2]);
        assert!(classic.lights[0].radius >= 14.0 && classic.lights[0].radius <= 17.0);
        assert!(classic.draws.iter().any(|draw| matches!(
            draw,
            FxDraw::Sprite { shader, .. } if shader == "gfx/effects/sabers/red_glow"
        )));
        assert!(classic.draws.iter().any(|draw| matches!(
            draw,
            FxDraw::Line { shader, .. } if shader == "gfx/effects/sabers/red_line"
        )));
        assert!(classic.draws.iter().all(|draw| match draw {
            FxDraw::Sprite { shader, .. } => shader == "gfx/effects/sabers/red_glow",
            FxDraw::Line { shader, .. } => shader == "gfx/effects/sabers/red_line",
            _ => false,
        }));

        fx.set_modern_sabers(true);
        fx.begin_frame(1001);
        fx.player_fx(&blade);
        let modern = fx.end_frame();
        assert_eq!(modern.draws.len(), 5, "two glow ribbons, two caps, one hot core");
        assert_eq!(
            modern.draws.iter().filter(|draw| matches!(draw, FxDraw::Line { .. })).count(),
            3
        );
    }

    #[test]
    fn saber_no_dlight_suppresses_stock_blade_light() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        let blade = PlayerFxRequest::SaberBlade {
            origin: [0.0; 3],
            direction: [1.0, 0.0, 0.0],
            length: 40.0,
            radius: 3.0,
            color: 4,
            entity_alpha: 1.0,
            entity_num: 3,
            saber_num: 0,
            blade_num: 0,
            saber_move: 0,
            torso_anim: 0,
            saber_in_flight: false,
            trail_style: 0,
            num_blades: 1,
            no_dlight: true,
            no_wall_marks: false,
        };
        fx.begin_frame(1000);
        fx.player_fx(&blade);
        let frame = fx.end_frame();
        assert!(frame.lights.is_empty());
        assert!(!frame.draws.is_empty(), "noDlight must not suppress the blade itself");
    }

    #[test]
    fn three_blade_saber_combines_to_one_openjk_light() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.begin_frame(1000);
        for (blade_num, direction, color) in [
            (0, [1.0, 0.0, 0.0], 0),
            (1, [-1.0, 0.0, 0.0], 3),
            (2, [0.0, 1.0, 0.0], 4),
        ] {
            fx.player_fx(&PlayerFxRequest::SaberBlade {
                origin: [0.0; 3],
                direction,
                length: 10.0,
                radius: 2.0,
                color,
                entity_alpha: 1.0,
                entity_num: 9,
                saber_num: 0,
                blade_num,
                saber_move: 0,
                torso_anim: 0,
                saber_in_flight: false,
                trail_style: 0,
                num_blades: 3,
                no_dlight: false,
                no_wall_marks: false,
            });
        }
        let frame = fx.end_frame();
        assert_eq!(frame.lights.len(), 1, "OpenJK CG_DoSaberLight emits one staff light");
        let light = &frame.lights[0];
        assert!((light.origin[0] - 0.0).abs() < 1e-5);
        assert!((light.origin[1] - (10.0 / 3.0)).abs() < 1e-5);
        assert!((light.origin[2] - 0.0).abs() < 1e-5);
        let expected = [
            (1.0 + 0.2 + 0.2) / 3.0,
            (0.2 + 1.0 + 0.4) / 3.0,
            (0.2 + 0.2 + 1.0) / 3.0,
        ];
        for (actual, expected) in light.rgb.iter().zip(expected) {
            assert!((*actual - expected).abs() < 1e-5);
        }
        assert!(light.radius >= 20.0 && light.radius <= 28.0);
    }

    #[test]
    fn saber_trail_connects_consecutive_blade_samples_with_stock_shader() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        // A thrown saber is always trail-active in OpenJK and avoids depending
        // on a specific saberMove enum value in this focused renderer test.
        let mut blade = PlayerFxRequest::SaberBlade {
            origin: [0.0, 0.0, 0.0], direction: [1.0, 0.0, 0.0], length: 10.0, radius: 2.0,
            color: 4, entity_alpha: 1.0, entity_num: 2, saber_num: 0, blade_num: 0,
            saber_move: 0, torso_anim: 0, saber_in_flight: true, trail_style: 0,
            num_blades: 1, no_dlight: false, no_wall_marks: false,
        };
        fx.begin_frame(1000);
        fx.player_fx(&blade);
        let _ = fx.end_frame();
        if let PlayerFxRequest::SaberBlade { origin, direction, .. } = &mut blade {
            *origin = [0.0, 1.0, 0.0];
            *direction = [0.0, 1.0, 0.0];
        }
        fx.begin_frame(1010);
        fx.player_fx(&blade);
        let frame = fx.end_frame();
        let quad = frame.draws.iter().find_map(|draw| match draw {
            FxDraw::Quad { positions, uvs, rgba, shader } => Some((positions, uvs, rgba, shader)),
            _ => None,
        }).expect("second sample should emit a saber trail quad");
        assert_eq!(quad.3, "gfx/effects/sabers/saberBlur");
        assert_eq!(quad.2[..3], [0, 64, 255]);
        assert_eq!(quad.0[0], [0.0, 1.0, 0.0]);
        assert_eq!(quad.1[2][0], 0.25, "10 ms / 40 ms stock fallback duration");

        fx.set_saber_trail(0);
        fx.begin_frame(1060);
        fx.player_fx(&blade);
        assert!(!fx.end_frame().draws.iter().any(|draw| matches!(draw, FxDraw::Quad { .. })));
    }

    #[test]
    fn enhanced_melt_smooths_and_builds_relief_mesh() {
        let stroke = MeltStroke {
            last_contact_time: 1200,
            points: vec![
                MeltPoint { position: [0.0, 0.0, 0.0], normal: [0.0, 1.0, 0.0], last_heated_at: 1200, heat: 1.2, dwell_ms: 900.0, phase: 0.2 },
                MeltPoint { position: [4.0, 0.0, 1.0], normal: [0.0, 1.0, 0.0], last_heated_at: 1200, heat: 1.0, dwell_ms: 250.0, phase: 0.8 },
                MeltPoint { position: [8.0, 0.0, 1.5], normal: [0.0, 1.0, 0.0], last_heated_at: 1200, heat: 0.8, dwell_ms: 80.0, phase: 1.4 },
            ],
        };
        let samples = smooth_melt_samples(&stroke, 1200);
        assert!(samples.len() > stroke.points.len(), "Catmull-Rom path should add smooth intermediate samples");
        let draw = melt_strip_mesh(&samples, 1200, MeltLayer::Relief).expect("hot stroke should make a relief mesh");
        let FxDraw::Mesh { positions, indices, shader, .. } = draw else { panic!("relief must be an indexed mesh") };
        assert_eq!(shader, "gfx/effects/saberDamageGlow");
        assert!(positions.len() >= samples.len() * 7);
        assert!(!indices.is_empty());
    }

    #[test]
    fn enhanced_heat_cools_by_time_not_frame_count() {
        let point = MeltPoint {
            position: [0.0; 3], normal: [0.0, 1.0, 0.0], last_heated_at: 1000, heat: 1.0, dwell_ms: 1000.0, phase: 0.0,
        };
        assert!((cooled_heat(&point, 1000) - 1.0).abs() < 1.0e-6);
        assert!(cooled_heat(&point, 5000) < cooled_heat(&point, 2000));
        assert_eq!(cooled_heat(&point, 9000), 0.0);
    }

    #[test]
    fn vanilla_weapon_tables_match_cg_register_weapon() {
        assert_eq!(trail_effect(WP_BLASTER, false), Some("blaster/shot"));
        assert_eq!(trail_effect(WP_DEMP2, true), None, "DEMP2 alt has no trail func");
        assert_eq!(trail_effect(WP_THERMAL, false), None);
        assert_eq!(missile_model(WP_ROCKET_LAUNCHER, true), Some("models/weapons2/merr_sonn/projectile.md3"));
        assert_eq!(missile_model(WP_BLASTER, false), None);
        assert_eq!(wall_impacts(WP_THERMAL, false, 0).len(), 2);
        assert_eq!(wall_impacts(WP_FLECHETTE, true, 0).len(), 0, "flechette alt impact is its own blow effect");
        assert_eq!(wall_impacts(WP_BRYAR_PISTOL, true, 4), [("bryar/wall_impact3", false)]);
        assert_eq!(play_effect_type(12), Some("env/water_impact"));
    }
}
