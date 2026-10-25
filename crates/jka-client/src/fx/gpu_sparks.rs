//! Narrow, renderer-independent staging contract for eventually GPU-owned EFX.
//!
//! These are spawn-time parameters, not 64-byte pre-tessellated sprite vertices.
//! The FX scheduler remains the authority for random picks and child effects.
//! No GPU resources, shaders or draw behavior are enabled by this module.
//!
//! The clock is explicitly the FX/demo clock rather than the renderer's
//! `camera_pos_time.w`: the latter continues advancing during paused demos.

use super::template::{
    PrimType, FX_ALPHA_SHIFT, FX_APPLY_PHYSICS, FX_DEATH_RUNS_FX, FX_EMIT_FX,
    FX_EXPENSIVE_PHYSICS, FX_GHOUL2_TRACE, FX_IMPACT_RUNS_FX,
    FX_KILL_ON_IMPACT, FX_PARM_MASK, FX_RAND, FX_RGB_SHIFT, FX_SIZE_SHIFT,
    FX_WAVE,
};
use super::system::FxClass;

/// Why a primitive stays with the known-correct CPU FX evaluator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuSparkFallback {
    NotBillboard,
    Collision,
    ChildEffect,
    DynamicRandom,
    AutoLight,
    InvalidSpawn,
    MaterialUnknown,
}

/// Primitive-only eligibility, *not* final material-stage eligibility.
/// Final promotion must also check shader animation/tcMods/blending before
/// removing the CPU drawing path. A collision-enabled particle can safely
/// run without traces when the chosen fx_physics mode would not trace it.
pub(crate) fn primitive_eligibility(
    kind: PrimType,
    flags: u32,
    physics_mode: u32,
    impact_fx: u32,
    death_fx: u32,
    emitter_fx: u32,
    auto_light: bool,
    spawn_finite: bool,
) -> Result<(), GpuSparkFallback> {
    if kind != PrimType::Particle {
        return Err(GpuSparkFallback::NotBillboard);
    }
    if !spawn_finite {
        return Err(GpuSparkFallback::InvalidSpawn);
    }
    // Keeping *all* links on CPU is conservative: effects may play from a
    // kill, death, tick or impact without themselves drawing a sprite.
    if flags & (FX_DEATH_RUNS_FX | FX_EMIT_FX | FX_IMPACT_RUNS_FX | FX_KILL_ON_IMPACT) != 0
        || impact_fx != 0 || death_fx != 0 || emitter_fx != 0
    {
        return Err(GpuSparkFallback::ChildEffect);
    }
    if auto_light {
        return Err(GpuSparkFallback::AutoLight);
    }
    // FX_RAND is re-rolled *each update* by TaystJK; a random value chosen
    // once at spawn is not equivalent. WAVE is deterministic but needs exact
    // shader parity first, so both stay CPU for this initial contract.
    for shift in [FX_ALPHA_SHIFT, FX_RGB_SHIFT, FX_SIZE_SHIFT] {
        let interp = flags >> shift;
        if interp & FX_RAND != 0 || interp & FX_PARM_MASK == FX_WAVE {
            return Err(GpuSparkFallback::DynamicRandom);
        }
    }
    // Check the actual trace gate in system::physics_step; FX_APPLY_PHYSICS
    // alone does NOT imply a trace at mode=2. All mode=3 particles with the
    // flag remain on the CPU (including ordinary flesh sparks).
    let trace_enabled = flags & FX_APPLY_PHYSICS != 0
        && flags & super::template::FX_PLAYER_VIEW == 0
        && (physics_mode >= 3
            || (physics_mode == 2 && flags & FX_EXPENSIVE_PHYSICS != 0));
    if trace_enabled || flags & FX_GHOUL2_TRACE != 0 {
        return Err(GpuSparkFallback::Collision);
    }
    Ok(())
}

/// Deliberately stricter than the existing GPU geometry tessellator: a
/// resident particle must not need the CPU to evaluate a material stage's
/// texture frame, tcMods, wave generator, or nonlinear UV animation each tick.
/// A rejected stage must remain on the reference path rather than silently
/// losing its shader semantics. Multi-stage shaders are inspected one stage
/// at a time; final promotion requires *every* drawn stage to be supported.
pub fn material_stage_eligible(material: &super::draw::FxMaterial) -> Result<(), GpuSparkFallback> {
    if !material.tc_mods.is_empty()
        || material.animation.is_some()
        || material.rgb_wave.is_some()
        || material.alpha_wave.is_some()
        || !material.rgb_vertex
        || !material.alpha_vertex
        || material.rgb_one_minus_vertex
        || material.alpha_one_minus_vertex
    {
        return Err(GpuSparkFallback::MaterialUnknown);
    }
    Ok(())
}

/// One stable, already-randomized spawn. No camera-dependent half-axis or
/// per-frame RGBA is included: these are evaluated by the eventual GPU path.
#[derive(Clone, Debug)]
pub struct GpuSparkSpawn {
    pub id: u64,
    pub generation: u64,
    pub class: FxClass,
    pub shader: String,
    pub start_ms: i32,
    pub end_ms: i32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub acceleration: [f32; 3],
    pub start_rotation: f32,
    pub rotation_delta: f32,
    pub rgb_start: [f32; 3],
    pub rgb_end: [f32; 3],
    pub alpha: (f32, f32, f32),
    pub size: (f32, f32, f32),
    pub rgb_parm: f32,
    pub flags: u32,
}

impl GpuSparkSpawn {
    /// Matches resident WGSL's analytic presentation. Used only if a pipeline
    /// is invalidated *after* its CPU owner has been put to sleep. This ensures
    /// an offloaded sprite is never lost while the new pipeline compiles.
    pub fn reference_draw_at(&self, time_ms: i32) -> super::system::FxDraw {
        use super::template::{FX_ALPHA_SHIFT, FX_RGB_SHIFT, FX_SIZE_SHIFT, FX_USE_ALPHA};
        let elapsed = (time_ms - self.start_ms).max(0) as f32 * 0.001;
        let origin = std::array::from_fn(|i| {
            self.origin[i] + self.velocity[i] * elapsed
                + 0.5 * self.acceleration[i] * elapsed * elapsed
        });
        let pct = |shift, parm| spawn_percent(self.flags >> shift, parm,
            self.start_ms, self.end_ms, time_ms.clamp(self.start_ms, self.end_ms));
        let size_pct = pct(FX_SIZE_SHIFT, self.size.2);
        let radius = self.size.0 * size_pct + self.size.1 * (1.0 - size_pct);
        let rgb_pct = pct(FX_RGB_SHIFT, self.rgb_parm);
        let alpha_pct = pct(FX_ALPHA_SHIFT, self.alpha.2);
        let rgb = std::array::from_fn::<u8, 3, _>(|i| {
            ((self.rgb_start[i] * rgb_pct + self.rgb_end[i] * (1.0 - rgb_pct)) * 255.0)
                .clamp(0.0, 255.0) as u8
        });
        let alpha = ((self.alpha.0 * alpha_pct + self.alpha.1 * (1.0 - alpha_pct))
            .clamp(0.0, 1.0) * 255.0) as u8;
        let rgba = if self.flags & FX_USE_ALPHA != 0 {
            [rgb[0], rgb[1], rgb[2], alpha]
        } else {
            [((u16::from(rgb[0]) * u16::from(alpha)) >> 8) as u8,
             ((u16::from(rgb[1]) * u16::from(alpha)) >> 8) as u8,
             ((u16::from(rgb[2]) * u16::from(alpha)) >> 8) as u8, 0]
        };
        let rotation = self.start_rotation + self.rotation_delta * (10.0 / 0.7)
            * (1.0 - (-0.7 * elapsed).exp());
        super::system::FxDraw::Sprite { shader_time: 0.0,
            origin, radius, rotation, rgba, shader: self.shader.clone(),
        }
    }

    pub fn values_finite(&self) -> bool {
        let coefficients = [
            self.alpha.0, self.alpha.1, self.alpha.2,
            self.size.0, self.size.1, self.size.2,
            self.rgb_parm, self.start_rotation, self.rotation_delta,
        ];
        self.origin.iter().chain(&self.velocity).chain(&self.acceleration)
            .chain(&self.rgb_start).chain(&self.rgb_end)
            .chain(coefficients.iter())
            .all(|n| n.is_finite())
            && self.end_ms > self.start_ms
            && !self.shader.is_empty()
    }
}

/// The scheduler clock is sent every FX frame, including while paused.
/// Incrementing `generation` on demo seek/restart makes stale GPU commands
/// unambiguously invalid without GPU readback or a blocking synchronization.
#[derive(Clone, Debug, Default)]
pub struct GpuSparkCommands {
    pub generation: u64,
    pub time_ms: i32,
    pub reset: bool,
    pub spawns: Vec<GpuSparkSpawn>,
}

/// The float version can be compared against the OpenJK CPU evaluator before
/// introducing a shader. Only analytic *presentation*, not its physics mode,
/// belongs here. The semi-implicit Euler step itself must be handled separately
/// if acceleration is nonzero and variable frame deltas are relevant.
pub fn spawn_percent(flags: u32, parm: f32, start_ms: i32, end_ms: i32, time_ms: i32) -> f32 {
    use super::template::{FX_LINEAR, FX_NONLINEAR, FX_CLAMP};
    let span = (end_ms - start_ms).max(1) as f32;
    let mut linear = if flags & FX_LINEAR != 0 { 1.0 - (time_ms - start_ms) as f32 / span } else { 1.0 };
    let other = match flags & FX_PARM_MASK {
        FX_NONLINEAR if time_ms as f32 > parm => 1.0 - (time_ms as f32 - parm) / ((end_ms as f32 - parm).max(0.001)),
        FX_NONLINEAR => 1.0,
        FX_CLAMP if (time_ms as f32) < parm => (parm - time_ms as f32) / (parm - start_ms as f32).max(0.001),
        FX_CLAMP => 0.0,
        _ => return linear,
    };
    linear = if flags & FX_LINEAR != 0 { linear * 0.5 + other * 0.5 } else { other };
    linear
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::template::{FX_LINEAR, FX_NONLINEAR, FX_CLAMP};
    #[test]
    fn authored_physics_gates_flesh_sparks_correctly() {
        assert!(primitive_eligibility(PrimType::Particle, FX_APPLY_PHYSICS, 2, 0, 0, 0, false, true).is_ok());
        assert_eq!(primitive_eligibility(PrimType::Particle, FX_APPLY_PHYSICS, 3, 0, 0, 0, false, true), Err(GpuSparkFallback::Collision));
        assert_eq!(primitive_eligibility(PrimType::Particle, FX_APPLY_PHYSICS | FX_EXPENSIVE_PHYSICS, 2, 0, 0, 0, false, true), Err(GpuSparkFallback::Collision));
        assert!(primitive_eligibility(PrimType::Particle, FX_APPLY_PHYSICS, 0, 0, 0, 0, false, true).is_ok());
    }
    #[test]
    fn never_steals_authored_children_or_dynamic_random() {
        assert_eq!(primitive_eligibility(PrimType::Particle, 0, 2, 0, 1, 0, false, true), Err(GpuSparkFallback::ChildEffect));
        assert_eq!(primitive_eligibility(PrimType::Particle, FX_RAND << FX_RGB_SHIFT, 2, 0, 0, 0, false, true), Err(GpuSparkFallback::DynamicRandom));
        assert_eq!(primitive_eligibility(PrimType::Tail, 0, 2, 0, 0, 0, false, true), Err(GpuSparkFallback::NotBillboard));
    }
    #[test]
    fn authored_curves_match_reference_samples() {
        for t in [0, 25, 50, 75, 100] {
            let expect = |flags, parm| {
                let mut rng = super::super::template::Rng::new(42);
                super::super::system::interpolation(flags, parm, 0, 100, t, &mut rng)
            };
            for (flags, parm) in [(0, 0.0), (FX_LINEAR, 0.0), (FX_NONLINEAR, 50.0), (FX_CLAMP, 50.0), (FX_LINEAR | FX_NONLINEAR, 50.0)] {
                assert!((spawn_percent(flags, parm, 0, 100, t) - expect(flags, parm)).abs() < 1e-6);
            }
        }
    }
}

/// WGPU storage layout: exactly seven 16-byte columns per spark. One 112-byte
/// spawn upload replaces hundreds of 64-byte *per-frame* sprite uploads.
/// Render coordinates are used throughout; the shader has no JKA axis conversion.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuSparkGpuState {
    pub origin_birth: [f32; 4],
    pub velocity_death: [f32; 4],
    pub acceleration_rotation: [f32; 4],
    pub rotation_alpha: [f32; 4],
    pub size_rgb_parm: [f32; 4],
    pub rgb_start_flags: [f32; 4],
    pub rgb_end_class: [f32; 4],
}

impl From<&GpuSparkSpawn> for GpuSparkGpuState {
    fn from(s: &GpuSparkSpawn) -> Self {
        use crate::scene::render_position;
        let origin = render_position(s.origin);
        let velocity = render_position(s.velocity);
        let acceleration = render_position(s.acceleration);
        let class = if s.class == FxClass::SaberImpact { 1.0 } else { 0.0 };
        Self {
            origin_birth: [origin[0], origin[1], origin[2], s.start_ms as f32],
            velocity_death: [velocity[0], velocity[1], velocity[2], s.end_ms as f32],
            acceleration_rotation: [acceleration[0], acceleration[1], acceleration[2], s.start_rotation],
            rotation_alpha: [s.rotation_delta, s.alpha.0, s.alpha.1, s.alpha.2],
            size_rgb_parm: [s.size.0, s.size.1, s.size.2, s.rgb_parm],
            rgb_start_flags: [s.rgb_start[0], s.rgb_start[1], s.rgb_start[2], f32::from_bits(s.flags)],
            rgb_end_class: [s.rgb_end[0], s.rgb_end[1], s.rgb_end[2], class],
        }
    }
}

/// FX clock, not wall-clock or `camera_pos_time.w` (demos can be paused).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuSparkClock {
    pub now_ms: f32,
    pub show_saber_impact: u32,
    pub reserved: [u32; 2],
}

/// Pack the GPU buffer writes without re-uploading any existing particle.
pub fn encode_gpu_writes(writes: &GpuSparkWrites) -> Vec<(u32, GpuSparkGpuState)> {
    writes.writes.iter().map(|(slot, spawn)| (*slot, GpuSparkGpuState::from(spawn))).collect()
}

/// Stable GPU-memory slots. The renderer will use each `write` as a single
/// queue.write_buffer at `slot * stride`, not upload the entire active pool on
/// every presentation frame. This CPU bookkeeping never needs GPU readback.
/// We retain saber-impact slots while hidden so paused-demo A/B can restore
/// precisely the same population, as the existing FX worker does.
#[derive(Default, Debug)]
pub struct GpuSparkPool {
    generation: Option<u64>,
    slots: Vec<Option<GpuSparkSpawn>>,
    free: Vec<usize>,
    by_id: std::collections::HashMap<u64, usize>,
}

#[derive(Default, Debug)]
pub struct GpuSparkWrites {
    /// Invalidate the entire GPU storage pool before applying this batch.
    pub clear: bool,
    /// New states only; no per-frame transform/colour uploads.
    pub writes: Vec<(u32, GpuSparkSpawn)>,
}

impl GpuSparkPool {
    pub fn slot_for_id(&self, id: u64) -> Option<u32> {
        self.by_id.get(&id).and_then(|&slot| u32::try_from(slot).ok())
    }

    pub fn live_count(&self) -> usize {
        self.by_id.len()
    }

    pub fn update(&mut self, commands: &GpuSparkCommands, capacity: usize) -> GpuSparkWrites {
        let mut writes = GpuSparkWrites::default();
        if commands.reset || self.generation != Some(commands.generation) {
            self.slots.clear();
            self.free.clear();
            self.by_id.clear();
            self.generation = Some(commands.generation);
            writes.clear = true;
        }
        // Stock CParticle survives at time==end but dies at time>end.
        for slot in 0..self.slots.len() {
            if self.slots[slot].as_ref().is_some_and(|p| commands.time_ms > p.end_ms) {
                let previous = self.slots[slot].take().expect("checked occupied slot");
                self.by_id.remove(&previous.id);
                self.free.push(slot);
            }
        }
        for spawn in &commands.spawns {
            if spawn.generation != commands.generation
                || commands.time_ms > spawn.end_ms
                || !spawn.values_finite()
                || self.by_id.contains_key(&spawn.id)
            {
                continue;
            }
            let slot = if let Some(free) = self.free.pop() {
                free
            } else if self.slots.len() < capacity.min(1800) {
                let index = self.slots.len();
                self.slots.push(None);
                index
            } else {
                // Never evict live authored effects merely to make room.
                continue;
            };
            self.by_id.insert(spawn.id, slot);
            self.slots[slot] = Some(spawn.clone());
            writes.writes.push((slot as u32, spawn.clone()));
        }
        writes
    }

    /// Stable indices for draw selection. The material resolver can group
    /// these by the shader's actual compatible stage. Never calls GPU readback.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &GpuSparkSpawn)> {
        self.slots.iter().enumerate()
            .filter_map(|(slot, particle)| particle.as_ref().map(|p| (slot as u32, p)))
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;
    fn spawn(id: u64, generation: u64) -> GpuSparkSpawn {
        GpuSparkSpawn {
            id, generation, class: FxClass::SaberImpact,
            shader: "gfx/effects/sabers/orange_glow".to_owned(),
            start_ms: 100, end_ms: 200, origin: [0.0; 3],
            velocity: [1.0, 0.0, 0.0], acceleration: [0.0; 3],
            start_rotation: 0.0, rotation_delta: 0.0,
            rgb_start: [1.0; 3], rgb_end: [0.0; 3],
            alpha: (1.0, 0.0, 0.0), size: (1.0, 0.0, 0.0),
            rgb_parm: 0.0, flags: 0,
        }
    }
    #[test]
    fn pool_uploads_only_births_reuses_expired_slots_and_honors_generation() {
        let mut pool = GpuSparkPool::default();
        let mut packet = GpuSparkCommands { generation: 1, time_ms: 100, reset: true,
            spawns: vec![spawn(10, 1)] };
        let first = pool.update(&packet, 1);
        assert!(first.clear);
        assert_eq!(first.writes.len(), 1);
        assert_eq!(first.writes[0].0, 0);
        assert_eq!(pool.live_count(), 1);
        assert_eq!(pool.slot_for_id(10), Some(0));
        assert_eq!(pool.slot_for_id(11), None);
        packet.reset = false;
        packet.spawns.clear();
        packet.time_ms = 150;
        assert!(pool.update(&packet, 1).writes.is_empty());
        packet.spawns.push(spawn(11, 1));
        assert!(pool.update(&packet, 1).writes.is_empty(), "cannot evict active particle");
        packet.time_ms = 200;
        assert_eq!(pool.live_count(), 1);
        assert!(pool.update(&packet, 1).writes.is_empty(), "expiry is strictly after killTime");
        packet.time_ms = 201;
        assert_eq!(pool.update(&packet, 1).writes.len(), 0, "new candidate already expired");
        assert_eq!(pool.live_count(), 0);
        packet.generation = 2;
        packet.reset = true;
        packet.time_ms = 100;
        packet.spawns = vec![spawn(12, 2)];
        let seek = pool.update(&packet, 1);
        assert!(seek.clear);
        assert_eq!(seek.writes.len(), 1);
        assert_eq!(pool.iter().next().unwrap().1.id, 12);
    }
}

#[cfg(test)]
mod gpu_layout_tests {
    use super::*;
    #[test]
    fn gpu_layout_matches_wgsl_columns() {
        assert_eq!(std::mem::size_of::<GpuSparkGpuState>(), 7 * 16);
        assert_eq!(std::mem::align_of::<GpuSparkGpuState>(), 4);
        assert_eq!(std::mem::size_of::<GpuSparkClock>(), 16);
    }
    #[test]
    fn gpu_spawn_converts_jka_coordinates_without_losing_flags() {
        let spark = GpuSparkSpawn {
            id: 1, generation: 0, class: FxClass::SaberImpact,
            shader: "gfx/test".to_owned(), start_ms: 100, end_ms: 350,
            origin: [1.0, 2.0, 3.0], velocity: [4.0, 5.0, 6.0],
            acceleration: [7.0, 8.0, 9.0], start_rotation: 30.0,
            rotation_delta: 0.0, rgb_start: [1.0; 3], rgb_end: [0.0; 3],
            alpha: (1.0, 0.0, 0.0), size: (3.0, 1.0, 100.0),
            rgb_parm: 0.0, flags: 0x0800_0105,
        };
        let gpu = GpuSparkGpuState::from(&spark);
        assert_eq!(gpu.origin_birth, [1.0, 3.0, -2.0, 100.0]);
        assert_eq!(gpu.velocity_death, [4.0, 6.0, -5.0, 350.0]);
        assert_eq!(gpu.acceleration_rotation, [7.0, 9.0, -8.0, 30.0]);
        assert_eq!(gpu.rgb_start_flags[3].to_bits(), spark.flags);
        assert_eq!(gpu.rgb_end_class[3], 1.0);
    }
}
