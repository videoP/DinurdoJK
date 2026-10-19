//! Labelled approximation of GPU compute emitters.
//!
//! A cooked GPU emitter has no VectorVM bytecode: its particle spawn/update
//! logic and module constants (lifetime, velocity, size) are compiled into the
//! shader. What does survive cooking, and is used here as-is:
//!
//! * the spawn count, which the system scripts compute on the CPU and hand
//!   over as `NiagaraSpawnInfo` (so bursts and rates are real);
//! * the particle dataset layout (`CachedScriptVM.Attributes`);
//! * the module stack (`StatScopes`), telling which behaviours exist;
//! * curve data interfaces (alpha, colour and size over life, as LUTs);
//! * the emitter's fixed bounds and the renderers with their materials.
//!
//! The missing constants are derived from the bounds: particles are born
//! near the emitter, drift through a fraction of the bounds over a nominal
//! lifetime, and take colour, alpha and size from the real curves. Bakes mark
//! such renderers as approximations.
use crate::niagara::Curve;
use crate::niagara_sim::{Buffer, Layout};
use crate::vector_vm::RandomStream;
use glam::Vec3;
use serde_json::Value;

/// Nominal particle lifetime when the module constant is compiled away.
const LIFETIME: f32 = 1.4;

#[derive(Debug)]
pub struct GpuApprox {
    center: Vec3,
    extent: Vec3,
    speed: f32,
    gravity: bool,
    drag: bool,
    sprite_size: f32,
    alpha: Option<Curve>,
    color: Option<Curve>,
    size: Option<Curve>,
}

fn v3(v: &Value) -> Option<Vec3> {
    Some(Vec3::new(
        v["X"].as_f64()? as f32,
        v["Y"].as_f64()? as f32,
        v["Z"].as_f64()? as f32,
    ))
}

impl GpuApprox {
    /// `emitter_name` is the emitter object's name (e.g. `Sparks_0`), used to
    /// find curve data interfaces owned by its scripts.
    pub fn parse(
        objects: &[Value],
        version_data: &Value,
        gpu_script: Option<&Value>,
        emitter_name: &str,
    ) -> Result<(Self, Layout), String> {
        let script = gpu_script.ok_or("GPU emitter has no GPUComputeScript")?;
        let vm = &script["Properties"]["CachedScriptVM"];
        let layout = Layout::build(vm["Attributes"].as_array().into_iter().flatten())?;
        if layout.get("Position").is_none() {
            return Err("GPU emitter dataset has no Position".into());
        }
        let modules: Vec<String> = vm["StatScopes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["FriendlyName"].as_str())
            .map(str::to_ascii_lowercase)
            .collect();
        let has = |k: &[&str]| modules.iter().any(|m| k.iter().any(|k| m.contains(k)));
        let bounds = &version_data["FixedBounds"];
        let (min, max) = (
            v3(&bounds["Min"]).unwrap_or(Vec3::splat(-100.)),
            v3(&bounds["Max"]).unwrap_or(Vec3::splat(100.)),
        );
        let extent = ((max - min) * 0.5).clamp(Vec3::splat(5.), Vec3::splat(2000.));
        // Curve DIs, in the order the GPU script lists them, by HLSL symbol.
        let symbols: Vec<String> = vm["ShaderScriptParametersMetadata"]["DataInterfaceParamInfo"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| {
                d["DIClassName"]
                    .as_str()
                    .is_some_and(|c| c.contains("Curve"))
            })
            .filter_map(|d| d["DataInterfaceHLSLSymbol"].as_str())
            .map(str::to_ascii_lowercase)
            .collect();
        let owned = |o: &&Value| {
            o["Type"]
                .as_str()
                .is_some_and(|t| t.starts_with("NiagaraDataInterface") && t.contains("Curve"))
                && o["Outer"]["ObjectName"]
                    .as_str()
                    .is_some_and(|s| s.contains(&format!(":{emitter_name}.")))
        };
        let curves: Vec<&Value> = objects.iter().filter(owned).collect();
        let mut alpha = None;
        let mut color = None;
        let mut size = None;
        if curves.len() == symbols.len() {
            for (symbol, object) in symbols.iter().zip(curves) {
                let Ok(curve) = Curve::from_export(object) else {
                    continue;
                };
                let channels = object["Type"].as_str().unwrap_or("");
                if symbol.contains("alpha") && alpha.is_none() {
                    alpha = Some(curve);
                } else if (symbol.contains("color") || symbol.contains("colour"))
                    && channels.contains("Color")
                    && color.is_none()
                {
                    color = Some(curve);
                } else if (symbol.contains("size") || symbol.contains("scale"))
                    && !symbol.contains("color")
                    && size.is_none()
                {
                    size = Some(curve);
                }
            }
        }
        let sprite_size = (extent.min_element() * 0.18).clamp(3., 80.);
        Ok((
            Self {
                center: (min + max) * 0.5,
                extent,
                speed: if has(&[
                    "velocity",
                    "force",
                    "vortex",
                    "curlnoise",
                    "noise",
                    "point attraction",
                ]) {
                    extent.length() * 0.35 / LIFETIME
                } else {
                    0.
                },
                gravity: has(&["gravity"]),
                drag: has(&["drag"]),
                sprite_size,
                alpha,
                color,
                size,
            },
            layout,
        ))
    }

    fn rand(random: &mut RandomStream) -> f32 {
        (random.next_word() >> 8) as f32 / (1u32 << 24) as f32
    }

    fn sample(curve: &Option<Curve>, t: f32) -> Option<Vec<f32>> {
        curve
            .as_ref()
            .and_then(|c| c.sample(t).ok())
            .map(|w| w.into_iter().map(f32::from_bits).collect())
    }

    /// Spawn `count` particles and advance all of them by `dt`.
    pub fn tick(
        &self,
        layout: &Layout,
        data: &mut Buffer,
        random: &mut RandomStream,
        origin: Vec3,
        local_space: bool,
        dt: f32,
        count: usize,
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
        let get = |data: &Buffer, name: &str, c: usize, k: usize, default: f32| {
            var(name)
                .filter(|v| (c as u16) < v.comps.f)
                .map_or(default, |v| data.float(v, c, k))
        };
        // Update and compact.
        let mut keep = 0;
        for k in 0..data.len {
            let life = get(data, "Lifetime", 0, k, LIFETIME).max(0.05);
            let age = get(data, "Age", 0, k, 0.) + dt;
            if age >= life {
                continue;
            }
            if keep != k {
                data.copy_within(k, keep);
            }
            let t = age / life;
            let mut vel = Vec3::new(
                get(data, "Velocity", 0, keep, 0.),
                get(data, "Velocity", 1, keep, 0.),
                get(data, "Velocity", 2, keep, 0.),
            );
            if self.gravity {
                vel.z -= 980. * dt;
            }
            if self.drag {
                vel *= (1. - 2.0 * dt).max(0.);
            }
            let pos = Vec3::new(
                get(data, "Position", 0, keep, 0.),
                get(data, "Position", 1, keep, 0.),
                get(data, "Position", 2, keep, 0.),
            ) + vel * dt;
            set(data, "Age", keep, &[age]);
            set(data, "NormalizedAge", keep, &[t]);
            set(data, "Position", keep, &pos.to_array());
            set(data, "Velocity", keep, &vel.to_array());
            let base: Vec<f32> = (0..4)
                .map(|c| get(data, "Initial.Color", c, keep, 1.))
                .collect();
            let mut color = Self::sample(&self.color, t).map_or(base.clone(), |c| {
                (0..4)
                    .map(|i| base[i] * c.get(i).copied().unwrap_or(1.))
                    .collect()
            });
            // Without an alpha curve, fade in and out over life.
            color[3] *= Self::sample(&self.alpha, t)
                .map_or((t * 8.).min(1.) * (1. - t).powf(0.7), |a| a[0]);
            set(data, "Color", keep, &color);
            let s = Self::sample(&self.size, t).map_or(1., |s| s[0]);
            let initial = get(data, "Initial.Scale", 0, keep, 1.);
            set(
                data,
                "SpriteSize",
                keep,
                &[self.sprite_size * s * initial; 2],
            );
            set(data, "Scale", keep, &[s * initial; 3]);
            keep += 1;
        }
        data.truncate(keep);
        // Spawn.
        let count = count.min(max.saturating_sub(data.len));
        let base = data.len;
        data.reserve_len(base + count);
        data.len = base + count;
        let center = if local_space {
            self.center
        } else {
            origin + self.center
        };
        for k in base..base + count {
            let mut r3 = || {
                Vec3::new(
                    Self::rand(random) * 2. - 1.,
                    Self::rand(random) * 2. - 1.,
                    Self::rand(random) * 2. - 1.,
                )
            };
            let pos = center + r3() * self.extent * 0.15;
            let dir = r3().normalize_or_zero();
            let vel = dir * self.speed * (0.5 + Self::rand(random));
            let life = LIFETIME * (0.7 + 0.6 * Self::rand(random));
            let initial = 0.75 + 0.5 * Self::rand(random);
            set(data, "Position", k, &pos.to_array());
            set(data, "Velocity", k, &vel.to_array());
            set(data, "Age", k, &[0.]);
            set(data, "NormalizedAge", k, &[0.]);
            set(data, "Lifetime", k, &[life]);
            set(data, "Initial.Color", k, &[1., 1., 1., 1.]);
            set(data, "Color", k, &[1., 1., 1., 0.]);
            set(data, "Initial.Scale", k, &[initial]);
            set(data, "SpriteSize", k, &[self.sprite_size * initial; 2]);
            set(data, "Scale", k, &[initial; 3]);
            set(data, "SpriteRotation", k, &[Self::rand(random) * 360.]);
            set(data, "MaterialRandom", k, &[Self::rand(random)]);
            let q = glam::Quat::from_euler(
                glam::EulerRot::XYZ,
                Self::rand(random) * 6.283,
                Self::rand(random) * 6.283,
                Self::rand(random) * 6.283,
            );
            set(data, "MeshOrientation", k, &q.to_array());
            if let Some(v) = layout.get("UniqueID").filter(|v| v.comps.i > 0) {
                data.i[v.i as usize][k] = *next_id as u32;
            }
            *next_id += 1;
        }
    }
}
