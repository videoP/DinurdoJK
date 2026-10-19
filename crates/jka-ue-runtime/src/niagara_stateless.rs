//! Niagara stateless ("lightweight") emitters.
//!
//! A stateless emitter has no VectorVM scripts: its spawn schedule and its
//! modules are plain cooked data (`NiagaraStatelessEmitter.SpawnInfos`,
//! `NiagaraStatelessModule_*` objects), evaluated per particle from its age.
//! This runs the same data on the CPU (Engine/Plugins/FX/Niagara, Stateless).
//! Modules without a native implementation here are reported by name.
use crate::niagara_sim::{Buffer, Layout};
use crate::vector_vm::RandomStream;
use glam::{Quat, Vec3};
use serde_json::Value;
use std::collections::HashMap;

/// `FNiagaraStatelessGlobals` defaults.
const DEFAULT_LIFETIME: f32 = 1.;
const DEFAULT_SPRITE_SIZE: f32 = 10.;

/// A value range: uniform per component between `min` and `max`.
#[derive(Clone, Copy, Debug)]
struct Range<const N: usize> {
    min: [f32; N],
    max: [f32; N],
}
impl<const N: usize> Range<N> {
    fn constant(v: [f32; N]) -> Self {
        Self { min: v, max: v }
    }
    fn sample(&self, random: &mut RandomStream) -> [f32; N] {
        // One random number for the whole vector (UniformConstant); per
        // component variation (NonUniform) is rare in cooked character FX.
        let t = rand(random);
        std::array::from_fn(|i| self.min[i] + (self.max[i] - self.min[i]) * t)
    }
}

fn rand(random: &mut RandomStream) -> f32 {
    (random.next_word() >> 8) as f32 / (1u32 << 24) as f32
}

fn components<const N: usize>(v: &Value) -> Option<[f32; N]> {
    if let Some(x) = v.as_f64() {
        return Some([x as f32; N]);
    }
    const KEYS: [&str; 4] = ["X", "Y", "Z", "W"];
    const COLOR: [&str; 4] = ["R", "G", "B", "A"];
    let keys = if v.get("R").is_some() { COLOR } else { KEYS };
    let mut out = [0.; N];
    for (i, k) in keys.iter().take(N).enumerate() {
        out[i] = v.get(*k)?.as_f64()? as f32;
    }
    Some(out)
}

/// `FNiagaraDistribution*`: constants, ranges, `Values` tables and parameter
/// bindings (resolved against the system's user parameter defaults).
fn distribution<const N: usize>(
    d: &Value,
    user: &HashMap<String, Vec<f32>>,
    default: [f32; N],
) -> Range<N> {
    if d.is_null() {
        return Range::constant(default);
    }
    let mode = d["Mode"].as_str().unwrap_or("");
    if mode.ends_with("Binding") {
        let name = d["ParameterBinding"]["Name"].as_str().unwrap_or("");
        if let Some(v) = user.get(name).filter(|v| v.len() >= N) {
            return Range::constant(std::array::from_fn(|i| v[i]));
        }
    }
    if let Some(values) = d["Values"].as_array().filter(|v| !v.is_empty()) {
        let first = components::<N>(&values[0]).unwrap_or(default);
        let last = values.last().and_then(components::<N>).unwrap_or(first);
        return Range {
            min: first,
            max: last,
        };
    }
    match (components::<N>(&d["Min"]), components::<N>(&d["Max"])) {
        (Some(min), Some(max)) => Range { min, max },
        (Some(min), None) => Range::constant(min),
        _ => Range::constant(default),
    }
}

#[derive(Clone, Debug)]
enum Spawn {
    Burst { time: f32, amount: Range<1> },
    Rate { rate: Range<1> },
}

#[derive(Debug)]
pub struct Stateless {
    spawns: Vec<Spawn>,
    looping: bool,
    loop_duration: f32,
    lifetime: Range<1>,
    color: Range<4>,
    sprite_size: Range<2>,
    sprite_rotation: Range<1>,
    mesh_scale: Range<3>,
    position: Range<3>,
    orientation: Option<Range<3>>,
    velocity: Option<Range<3>>,
    acceleration: Vec3,
    drag: f32,
    solve: bool,
    /// Modules present in the asset but not simulated here.
    pub unsupported: Vec<String>,
}

/// `User.*` parameter defaults from `NiagaraSystem.ExposedParameters`.
pub fn user_parameters(system: &Value) -> HashMap<String, Vec<f32>> {
    let exposed = &system["Properties"]["ExposedParameters"];
    let data: Vec<u8> = exposed["ParameterData"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b.as_u64().map(|b| b as u8))
        .collect();
    let mut out = HashMap::new();
    for p in exposed["SortedParameterOffsets"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let (Some(name), Some(offset)) = (p["Name"].as_str(), p["Offset"].as_u64()) else {
            continue;
        };
        let class = p["TypeDef"]["ClassStructOrEnum"]["ObjectName"]
            .as_str()
            .unwrap_or("");
        let count =
            if class.contains("LinearColor") || class.contains("Vector4") || class.contains("Quat")
            {
                4
            } else if class.contains("Vector3")
                || class.contains("'Vector'")
                || class.contains("Position")
            {
                3
            } else if class.contains("Vector2") {
                2
            } else {
                1
            };
        let floats = (0..count)
            .map(|k| offset as usize + 4 * k)
            .filter_map(|at| data.get(at..at + 4))
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect::<Vec<_>>();
        if floats.len() == count {
            out.insert(name.to_owned(), floats);
        }
    }
    out
}

impl Stateless {
    /// `emitter` is the `NiagaraStatelessEmitter` object; `objects` the
    /// system package (its modules and renderers).
    pub fn parse(
        objects: &[Value],
        emitter: &Value,
        user: &HashMap<String, Vec<f32>>,
    ) -> Result<(Self, Layout), String> {
        let p = &emitter["Properties"];
        let layout = Layout::build(
            p["ParticleDataSetCompiledData"]["Variables"]
                .as_array()
                .into_iter()
                .flatten(),
        )?;
        if layout.get("Position").is_none() {
            return Err("stateless emitter dataset has no Position".into());
        }
        let state = &p["EmitterState"];
        let looping = !state["LoopBehavior"]
            .as_str()
            .is_some_and(|b| b.ends_with("Once"));
        let loop_duration = distribution::<1>(&state["LoopDuration"], user, [1.]).max[0].max(0.05);
        let mut spawns = Vec::new();
        for s in p["SpawnInfos"].as_array().into_iter().flatten() {
            if s["bEnabled"] == false {
                continue;
            }
            if s["Type"].as_str().is_some_and(|t| t.ends_with("Rate")) {
                spawns.push(Spawn::Rate {
                    rate: distribution(&s["Rate"], user, [0.]),
                });
            } else {
                spawns.push(Spawn::Burst {
                    time: s["SpawnTime"].as_f64().unwrap_or(0.) as f32,
                    amount: distribution(&s["Amount"], user, [1.]),
                });
            }
        }
        let mut out = Self {
            spawns,
            looping,
            loop_duration,
            lifetime: Range::constant([DEFAULT_LIFETIME]),
            color: Range::constant([1.; 4]),
            sprite_size: Range::constant([DEFAULT_SPRITE_SIZE; 2]),
            sprite_rotation: Range::constant([0.]),
            mesh_scale: Range::constant([1.; 3]),
            position: Range::constant([0.; 3]),
            orientation: None,
            velocity: None,
            acceleration: Vec3::ZERO,
            drag: 0.,
            solve: false,
            unsupported: Vec::new(),
        };
        let emitter_name = emitter["Name"].as_str().unwrap_or("");
        let owned = |o: &&Value| {
            o["Outer"]["ObjectName"]
                .as_str()
                .is_some_and(|s| s.ends_with(&format!(":{emitter_name}'")))
        };
        for module in objects.iter().filter(owned) {
            let kind = module["Type"].as_str().unwrap_or("");
            let m = &module["Properties"];
            match kind.strip_prefix("NiagaraStatelessModule_") {
                Some("InitializeParticle") => {
                    out.lifetime =
                        distribution(&m["LifetimeDistribution"], user, [DEFAULT_LIFETIME]);
                    out.color = distribution(&m["ColorDistribution"], user, [1.; 4]);
                    out.sprite_size =
                        distribution(&m["SpriteSizeDistribution"], user, [DEFAULT_SPRITE_SIZE; 2]);
                    out.sprite_rotation =
                        distribution(&m["SpriteRotationDistribution"], user, [0.]);
                    out.mesh_scale = distribution(&m["MeshScaleDistribution"], user, [1.; 3]);
                    out.position = distribution(&m["InitialPositionDistribution"], user, [0.; 3]);
                }
                Some("InitialMeshOrientation") => {
                    out.orientation = Some(distribution(&m["Rotation"], user, [0.; 3]))
                }
                Some("AddVelocity") => {
                    out.velocity = Some(distribution(
                        &m["LinearVelocityDistribution"],
                        user,
                        [0.; 3],
                    ))
                }
                Some("GravityForce") => {
                    out.acceleration += Vec3::from_array(
                        distribution(&m["GravityDistribution"], user, [0., 0., -980.]).max,
                    )
                }
                Some("AccelerationForce") => {
                    out.acceleration += Vec3::from_array(
                        distribution(&m["AccelerationDistribution"], user, [0.; 3]).max,
                    )
                }
                Some("Drag") => out.drag = distribution(&m["DragDistribution"], user, [0.]).max[0],
                Some("SolveVelocitiesAndForces") => out.solve = true,
                Some(other) => out.unsupported.push(other.to_owned()),
                None => {}
            }
        }
        Ok((out, layout))
    }

    /// Particles to spawn while the emitter clock moves from `from` to `to`.
    fn spawn_count(&self, from: f32, to: f32, random: &mut RandomStream) -> usize {
        let mut count = 0.;
        for spawn in &self.spawns {
            match spawn {
                Spawn::Rate { rate } => count += rate.sample(random)[0].max(0.) * (to - from),
                Spawn::Burst { time, amount } => {
                    // A burst at `time` in every loop iteration, counted when
                    // it falls in (from, to]; a negative `from` is the start.
                    let d = if self.looping {
                        self.loop_duration
                    } else {
                        f32::INFINITY
                    };
                    let mut k = if from < *time {
                        0
                    } else {
                        ((from - time) / d).floor() as i64 + 1
                    };
                    loop {
                        let at = time + k as f32 * d;
                        if at > to {
                            break;
                        }
                        if at > from {
                            count += amount.sample(random)[0].max(0.).round();
                        }
                        if !self.looping {
                            break;
                        }
                        k += 1;
                    }
                }
            }
        }
        count.round().max(0.) as usize
    }

    /// Advance live particles by `dt`, then spawn for the clock step
    /// `age - dt .. age` (the first tick also fires bursts at time 0).
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &self,
        layout: &Layout,
        data: &mut Buffer,
        random: &mut RandomStream,
        origin: Vec3,
        local_space: bool,
        dt: f32,
        age: f32,
        spawn: bool,
        max: usize,
        next_id: &mut i32,
    ) {
        let var = |name: &str| layout.get(name);
        let set = |data: &mut Buffer, name: &str, k: usize, values: &[f32]| {
            if let Some(v) = var(name) {
                for (c, x) in values.iter().enumerate().take(v.comps.f as usize) {
                    data.f[(v.f as usize) + c][k] = x.to_bits();
                }
            }
        };
        let get3 = |data: &Buffer, name: &str, k: usize| {
            var(name).map_or(Vec3::ZERO, |v| {
                Vec3::new(
                    data.float(v, 0, k),
                    data.float(v, 1, k),
                    data.float(v, 2, k),
                )
            })
        };
        let get = |data: &Buffer, name: &str, k: usize, default: f32| {
            var(name).map_or(default, |v| data.float(v, 0, k))
        };
        let mut keep = 0;
        for k in 0..data.len {
            let life = get(data, "Lifetime", k, DEFAULT_LIFETIME).max(1e-3);
            let particle_age = get(data, "Age", k, 0.) + dt;
            if particle_age >= life {
                continue;
            }
            if keep != k {
                data.copy_within(k, keep);
            }
            if self.solve {
                let mut velocity = get3(data, "Velocity", keep) + self.acceleration * dt;
                velocity *= (1. - self.drag * dt).max(0.);
                let position = get3(data, "Position", keep) + velocity * dt;
                set(data, "Velocity", keep, &velocity.to_array());
                set(data, "Position", keep, &position.to_array());
            }
            set(data, "Age", keep, &[particle_age]);
            set(data, "NormalizedAge", keep, &[particle_age / life]);
            keep += 1;
        }
        data.truncate(keep);
        if !spawn {
            return;
        }
        let from = if age - dt <= 1e-6 { -1. } else { age - dt };
        let count = self
            .spawn_count(from, age, random)
            .min(max.saturating_sub(data.len));
        let base = data.len;
        data.reserve_len(base + count);
        data.len = base + count;
        for k in base..base + count {
            let local = Vec3::from_array(self.position.sample(random));
            let position = if local_space { local } else { origin + local };
            set(data, "Position", k, &position.to_array());
            set(
                data,
                "Velocity",
                k,
                &self.velocity.map_or([0.; 3], |v| v.sample(random)),
            );
            set(data, "Age", k, &[0.]);
            set(data, "NormalizedAge", k, &[0.]);
            set(
                data,
                "Lifetime",
                k,
                &[self.lifetime.sample(random)[0].max(1e-3)],
            );
            set(data, "Color", k, &self.color.sample(random));
            set(data, "SpriteSize", k, &self.sprite_size.sample(random));
            set(
                data,
                "SpriteRotation",
                k,
                &self.sprite_rotation.sample(random),
            );
            set(data, "Scale", k, &self.mesh_scale.sample(random));
            set(data, "MaterialRandom", k, &[rand(random)]);
            let q = self
                .orientation
                .map_or(Quat::IDENTITY, |r| rotator_quat(r.sample(random)));
            set(data, "MeshOrientation", k, &q.to_array());
            set(data, "PreviousMeshOrientation", k, &q.to_array());
            if let Some(v) = layout.get("UniqueID").filter(|v| v.comps.i > 0) {
                data.i[v.i as usize][k] = *next_id as u32;
            }
            *next_id += 1;
        }
    }
}

/// `FNiagaraStatelessParticleSimContext::RotatorToQuat` (roll, pitch, yaw in degrees).
fn rotator_quat(r: [f32; 3]) -> Quat {
    let half = |deg: f32| (deg.to_radians() * 0.5).sin_cos();
    let ((sr, cr), (sp, cp), (sy, cy)) = (half(r[0]), half(r[1]), half(r[2]));
    Quat::from_xyzw(
        cr * sp * sy - sr * cp * cy,
        -cr * sp * cy - sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    )
    .normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bindings_resolve_user_defaults_and_ranges_fall_back() {
        let user = HashMap::from([("User.EyeColor".to_owned(), vec![0., 3.09, 5., 1.])]);
        let bound = distribution::<4>(
            &json!({"Mode": "ENiagaraDistributionMode::Binding", "ParameterBinding": {"Name": "User.EyeColor"}}),
            &user,
            [1.; 4],
        );
        assert_eq!(bound.min, [0., 3.09, 5., 1.]);
        let table = distribution::<3>(
            &json!({"Values": [{"X": 1, "Y": 2, "Z": 3}, {"X": 1, "Y": 2, "Z": 3}]}),
            &user,
            [0.; 3],
        );
        assert_eq!(table.max, [1., 2., 3.]);
        let unset = distribution::<1>(
            &json!({"ParameterBinding": {"Name": "None"}}),
            &user,
            [DEFAULT_LIFETIME],
        );
        assert_eq!(unset.min, [1.]);
    }

    #[test]
    fn looping_burst_fires_once_per_loop() {
        let s = Stateless {
            spawns: vec![Spawn::Burst {
                time: 0.,
                amount: Range::constant([1.]),
            }],
            looping: true,
            loop_duration: 1.,
            lifetime: Range::constant([1.]),
            color: Range::constant([1.; 4]),
            sprite_size: Range::constant([10.; 2]),
            sprite_rotation: Range::constant([0.]),
            mesh_scale: Range::constant([1.; 3]),
            position: Range::constant([0.; 3]),
            orientation: None,
            velocity: None,
            acceleration: Vec3::ZERO,
            drag: 0.,
            solve: false,
            unsupported: Vec::new(),
        };
        let mut random = RandomStream::new(1);
        // 89 ticks of 1/30 s end just before t = 3: bursts at 0, 1 and 2.
        let mut total = 0;
        let mut from = -1.;
        for step in 1..=89 {
            let t = step as f32 / 30.;
            total += s.spawn_count(from, t, &mut random);
            from = t;
        }
        assert_eq!(total, 3);
    }
}
