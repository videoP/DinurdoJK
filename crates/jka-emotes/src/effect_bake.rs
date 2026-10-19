//! Bake montage Niagara notifies into native particle tracks.
use crate::effects::{Align, Facing, Kind, Particle as Baked, Renderer, Writer};
use glam::{Quat, Vec3};
use jka_ue_runtime::niagara_sim::{
    reference_path, Environment, RendererDesc, RendererKind, SystemAsset, SystemInstance,
    TickContext, Transform,
};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::{Arc, Mutex},
};

fn load(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| e.to_string())
}

/// PSA space is Unreal space reflected across Y.
fn reflect_point(p: Vec3) -> Vec3 {
    Vec3::new(p.x, -p.y, p.z)
}
fn reflect_quat(q: Quat) -> Quat {
    Quat::from_xyzw(-q.x, q.y, -q.z, q.w)
}
/// Unreal FRotator in Unreal space (no reflection).
pub fn ue_rotator(v: &Value) -> Quat {
    let pitch = v["Pitch"].as_f64().unwrap_or(0.) as f32;
    let yaw = v["Yaw"].as_f64().unwrap_or(0.) as f32;
    let roll = v["Roll"].as_f64().unwrap_or(0.) as f32;
    Quat::from_rotation_z(yaw.to_radians())
        * Quat::from_rotation_y(-pitch.to_radians())
        * Quat::from_rotation_x(-roll.to_radians())
}
fn ue_vec(v: &Value, default: Vec3) -> Vec3 {
    if v.is_null() {
        default
    } else if let Some(n) = v.as_f64() {
        Vec3::splat(n as f32)
    } else {
        Vec3::new(
            v["X"].as_f64().unwrap_or(default.x as f64) as f32,
            v["Y"].as_f64().unwrap_or(default.y as f64) as f32,
            v["Z"].as_f64().unwrap_or(default.z as f64) as f32,
        )
    }
}

/// Composed source animation as a Niagara skeletal environment (Unreal space).
pub struct PsaEnvironment {
    names: Vec<String>,
    /// frames x bones world transforms.
    world: Vec<(Vec3, Quat)>,
    bones: usize,
    frames: usize,
    fps: f32,
    sockets: HashMap<String, (usize, Transform)>,
}
impl PsaEnvironment {
    pub fn new(psa: &crate::psa::Psa, sockets: &HashMap<String, Value>) -> Self {
        let bones = psa.bones.len();
        let mut world: Vec<(Vec3, Quat)> = Vec::with_capacity(psa.frames * bones);
        for frame in 0..psa.frames {
            let base = world.len();
            for (i, b) in psa.bones.iter().enumerate() {
                let k = &psa.keys[frame * bones + i];
                let next = match b.parent {
                    Some(parent) => {
                        let (pp, pq) = world[base + parent];
                        (pp + pq * k.position, (pq * k.rotation).normalize())
                    }
                    None => (k.position, k.rotation.normalize()),
                };
                world.push(next);
            }
        }
        let world = world
            .into_iter()
            .map(|(p, q)| (reflect_point(p), reflect_quat(q)))
            .collect();
        let names = psa.bones.iter().map(|b| b.name.clone()).collect::<Vec<_>>();
        let mut map = HashMap::new();
        for (name, def) in sockets {
            let Some(bone_name) = def["BoneName"].as_str() else {
                continue;
            };
            let Some(bone) = names.iter().position(|n| n.eq_ignore_ascii_case(bone_name)) else {
                continue;
            };
            let rel = Transform {
                translation: ue_vec(&def["RelativeLocation"], Vec3::ZERO),
                rotation: ue_rotator(&def["RelativeRotation"]),
                scale: ue_vec(&def["RelativeScale"], Vec3::ONE),
            };
            map.insert(name.to_lowercase(), (bone, rel));
        }
        Self {
            names,
            world,
            bones,
            frames: psa.frames,
            fps: psa.fps.max(1.),
            sockets: map,
        }
    }
    /// A single, unmoving pose: component-space (translation, rotation) per
    /// bone in Unreal space, e.g. a character's rest pose for idle effects.
    pub fn static_pose(
        names: Vec<String>,
        world: Vec<(Vec3, Quat)>,
        sockets: &HashMap<String, Value>,
    ) -> Self {
        let mut map = HashMap::new();
        for (name, def) in sockets {
            let Some(bone_name) = def["BoneName"].as_str() else {
                continue;
            };
            let Some(bone) = names.iter().position(|n| n.eq_ignore_ascii_case(bone_name)) else {
                continue;
            };
            let rel = Transform {
                translation: ue_vec(&def["RelativeLocation"], Vec3::ZERO),
                rotation: ue_rotator(&def["RelativeRotation"]),
                scale: ue_vec(&def["RelativeScale"], Vec3::ONE),
            };
            map.insert(name.to_lowercase(), (bone, rel));
        }
        Self {
            bones: names.len(),
            names,
            world,
            frames: 1,
            fps: 30.,
            sockets: map,
        }
    }
    /// Transform of a socket or bone by name.
    pub fn named(&self, name: &str, seconds: f32) -> Option<Transform> {
        if let Some((bone, rel)) = self.sockets.get(&name.to_lowercase()) {
            return Some(self.bone(*bone, seconds)?.compose(rel));
        }
        let bone = self
            .names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))?;
        self.bone(bone, seconds)
    }
}
impl Environment for PsaEnvironment {
    fn bones(&self) -> &[String] {
        &self.names
    }
    fn bone(&self, bone: usize, seconds: f32) -> Option<Transform> {
        if bone >= self.bones || self.frames == 0 {
            return None;
        }
        let f = (seconds * self.fps).clamp(0., (self.frames - 1) as f32);
        let a = f.floor() as usize;
        let b = (a + 1).min(self.frames - 1);
        let w = f - a as f32;
        let (pa, qa) = self.world[a * self.bones + bone];
        let (pb, qb) = self.world[b * self.bones + bone];
        Some(Transform {
            translation: pa.lerp(pb, w),
            rotation: qa.slerp(qb, w),
            scale: Vec3::ONE,
        })
    }
    fn socket(&self, name: &str) -> Option<(usize, Transform)> {
        self.sockets.get(&name.to_lowercase()).copied()
    }
}

/// A Niagara spawn notify on the variant timeline.
#[derive(Clone, Debug)]
pub struct EffectSpec {
    pub template: String,
    pub socket: String,
    pub location: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub attached: bool,
    pub timed: bool,
    pub destroy_at_end: bool,
    /// Variant-clock lifetimes (seconds); one-shots use the start only.
    pub intervals: Vec<[f32; 2]>,
}
impl EffectSpec {
    pub fn from_notify(class: &str, props: &Value, intervals: Vec<[f32; 2]>) -> Option<Self> {
        if !props["Template"]["ObjectName"]
            .as_str()
            .is_some_and(|s| s.starts_with("NiagaraSystem"))
        {
            return None;
        }
        let template = reference_path(&props["Template"]);
        let timed = class.contains("NotifyState") || class.contains("Timed");
        let attached = props["Attached"]
            .as_bool()
            .or(props["bAttached"].as_bool())
            .unwrap_or(true)
            || timed;
        let scale = ue_vec(props.get("Scale").unwrap_or(&Value::Null), Vec3::ONE);
        Some(Self {
            template,
            socket: props["SocketName"].as_str().unwrap_or("None").to_owned(),
            location: ue_vec(&props["LocationOffset"], Vec3::ZERO),
            rotation: ue_rotator(&props["RotationOffset"]),
            scale: if scale.cmple(Vec3::ZERO).any() {
                Vec3::ONE
            } else {
                scale
            },
            attached,
            timed,
            destroy_at_end: props["bDestroyAtEnd"].as_bool().unwrap_or(false),
            intervals,
        })
    }
}

/// Load exported vector fields (`Objects/<id>.json` + `.vectorfield.bin`) by asset path.
pub struct FieldStore {
    directory: std::path::PathBuf,
    lookup: HashMap<String, String>,
    cache: Mutex<HashMap<String, Option<Arc<jka_ue_runtime::niagara_sim::VectorField>>>>,
}
impl FieldStore {
    pub fn new(directory: &Path, lookup: HashMap<String, String>) -> Self {
        Self {
            directory: directory.to_owned(),
            lookup,
            cache: Mutex::new(HashMap::new()),
        }
    }
    pub fn get(&self, path: &str) -> Option<Arc<jka_ue_runtime::niagara_sim::VectorField>> {
        let key = path.to_lowercase();
        if let Some(found) = self.cache.lock().unwrap().get(&key) {
            return found.clone();
        }
        let loaded = self.lookup.get(&key).and_then(|id| {
            let objects = load(&self.directory.join("Objects").join(format!("{id}.json"))).ok()?;
            let object = objects
                .as_array()?
                .iter()
                .find(|o| o["Type"] == "VectorFieldStatic")?;
            let bytes = std::fs::read(
                self.directory
                    .join("Objects")
                    .join(format!("{id}.vectorfield.bin")),
            )
            .ok()?;
            jka_ue_runtime::niagara_sim::VectorField::from_export(&object["Properties"], &bytes)
                .ok()
                .map(Arc::new)
        });
        self.cache.lock().unwrap().insert(key, loaded.clone());
        loaded
    }
}

/// Parsed systems shared between variants and worker threads.
#[derive(Default)]
pub struct SystemCache {
    systems: Mutex<HashMap<String, Option<Arc<SystemAsset>>>>,
    fields: std::sync::OnceLock<FieldStore>,
}
impl SystemCache {
    pub fn get(
        &self,
        directory: &Path,
        lookup: &HashMap<String, String>,
        template: &str,
    ) -> Option<Arc<SystemAsset>> {
        let key = template.to_lowercase();
        if let Some(found) = self.systems.lock().unwrap().get(&key) {
            return found.clone();
        }
        let fields = self
            .fields
            .get_or_init(|| FieldStore::new(directory, lookup.clone()));
        let parsed = lookup
            .get(&key)
            .and_then(|id| load(&directory.join("Objects").join(format!("{id}.json"))).ok())
            .and_then(|p| SystemAsset::parse_with(&p, &|path| fields.get(path)).ok())
            .map(Arc::new);
        self.systems.lock().unwrap().insert(key, parsed.clone());
        parsed
    }
}

/// A native asset import (`<pk3>.report.json`), the export it came from and
/// the material/model tables effect bakes convert renderers with. Shared by
/// emote presentation and character idle-effect bakes.
pub struct NativeContext {
    pub index: Value,
    pub export_dir: std::path::PathBuf,
    /// lowercase asset path -> export id.
    pub export_lookup: HashMap<String, String>,
    pub material_lookup: HashMap<String, String>,
    pub static_models: HashMap<String, String>,
    pub grid_models: std::collections::HashSet<String>,
    pub tints: HashMap<String, [f32; 3]>,
    pub modulate: std::collections::HashSet<String>,
}
impl NativeContext {
    pub fn load(report: &Path) -> Result<Self, String> {
        let index = load(report)?;
        let export_dir = std::path::PathBuf::from(
            index["source_directory"]
                .as_str()
                .ok_or("native assets have no source directory")?,
        );
        let export_report = load(&export_dir.join("report.json"))?;
        let export_lookup = export_report["assets"]
            .as_array()
            .ok_or("export report has no assets")?
            .iter()
            .filter_map(|r| {
                Some((
                    r["asset"].as_str()?.to_lowercase(),
                    r["id"].as_str()?.to_owned(),
                ))
            })
            .collect();
        let materials_index = load(&export_dir.join("materials.json"))?;
        let materials = materials_index.as_object().cloned().unwrap_or_default();
        let material_lookup = materials
            .iter()
            .filter_map(|(k, v)| Some((v["asset"].as_str()?.to_lowercase(), k.clone())))
            .collect();
        let static_models = index["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                Some((
                    r["asset"].as_str()?.to_lowercase(),
                    r["model"].as_str()?.to_owned(),
                ))
            })
            .collect();
        let modulate = materials
            .iter()
            .filter(|(_, v)| v["blend"] == "BLEND_Modulate")
            .map(|(k, _)| k.clone())
            .collect();
        let grid_ids = materials
            .iter()
            .filter(|(_, v)| {
                v["asset"]
                    .as_str()
                    .is_some_and(|a| a.ends_with("/WorldGridMaterial.WorldGridMaterial"))
            })
            .map(|(k, _)| k.clone())
            .collect::<std::collections::HashSet<_>>();
        let grid_models = index["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| {
                let ids = m["material_ids"].as_array().cloned().unwrap_or_default();
                !ids.is_empty()
                    && ids
                        .iter()
                        .all(|id| id.as_str().is_none_or(|id| grid_ids.contains(id)))
            })
            .filter_map(|m| m["asset"].as_str().map(str::to_lowercase))
            .collect();
        let tints = materials
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), crate::effect_materials::tint(v)?)))
            .collect();
        Ok(Self {
            index,
            export_dir,
            export_lookup,
            material_lookup,
            static_models,
            grid_models,
            tints,
            modulate,
        })
    }
    pub fn render_assets<'a>(
        &'a self,
        used_materials: &'a Mutex<BTreeSet<String>>,
        mesh_materials: &'a Mutex<BTreeSet<String>>,
        sprite_materials: &'a Mutex<BTreeSet<String>>,
    ) -> RenderAssets<'a> {
        RenderAssets {
            materials: &self.material_lookup,
            models: &self.static_models,
            used_materials,
            mesh_materials,
            sprite_materials,
            grid_models: &self.grid_models,
            tints: &self.tints,
            modulate: &self.modulate,
        }
    }
}

/// Native asset context for renderer conversion.
pub struct RenderAssets<'a> {
    /// lowercase material asset path -> materials.json key.
    pub materials: &'a HashMap<String, String>,
    /// lowercase mesh asset path -> native model qpath.
    pub models: &'a HashMap<String, String>,
    pub used_materials: &'a Mutex<BTreeSet<String>>,
    /// Materials applied to mesh particles: ported as UV-mapped surfaces, not
    /// as sprite shapes (see `effect_materials::write`).
    pub mesh_materials: &'a Mutex<BTreeSet<String>>,
    /// Materials applied to sprites and ribbons.
    pub sprite_materials: &'a Mutex<BTreeSet<String>>,
    /// Mesh assets whose only material is the engine default (WorldGrid).
    /// A mesh renderer that does not override it draws UE's grey grid, so
    /// the renderer carries no authored look and is skipped.
    pub grid_models: &'a std::collections::HashSet<String>,
    /// materials.json key -> authored tint folded into particle color.
    pub tints: &'a HashMap<String, [f32; 3]>,
    /// BLEND_Modulate materials. They are drawn with
    /// `GL_ZERO GL_ONE_MINUS_SRC_COLOR`, so the baked particle color is
    /// `(1 - rgb) * alpha`: the framebuffer becomes `dst * lerp(1, rgb, alpha)`,
    /// UE's modulate with opacity, instead of tinting the whole quad.
    pub modulate: &'a std::collections::HashSet<String>,
}

pub struct BakeResult {
    pub track: Vec<u8>,
    pub renderers: Vec<Renderer>,
    pub diagnostics: Vec<String>,
}

pub const BAKE_FPS: f32 = crate::effects::TRACK_FPS;
const SIM_FPS: f32 = 30.;
const MAX_ACTIVE: usize = 24;
const MAX_DRAWN: usize = 3000;

struct Active {
    spec: usize,
    serial: u16,
    instance: SystemInstance,
    fixed: Option<Transform>,
    previous: Option<Vec3>,
    stop_at: Option<f32>,
}

/// Camera used for camera-relative modules, in front of the character.
fn camera_for(basis: Quat) -> Transform {
    let front = reflect_point(basis.inverse() * Vec3::X).normalize_or(Vec3::Y);
    Transform {
        translation: front * 300. + Vec3::Z * 120.,
        rotation: Quat::from_rotation_arc(Vec3::X, -front),
        scale: Vec3::ONE,
    }
}

/// Simulate every effect of one variant and record its particle track.
#[allow(clippy::too_many_arguments)]
pub fn bake_variant(
    directory: &Path,
    lookup: &HashMap<String, String>,
    cache: &SystemCache,
    assets: &RenderAssets,
    specs: &[EffectSpec],
    env: &PsaEnvironment,
    basis: Quat,
    scale: f32,
    frames: usize,
    loop_start: Option<usize>,
) -> Option<BakeResult> {
    if specs.is_empty() || frames == 0 {
        return None;
    }
    let mut diagnostics = Vec::new();
    let short = |t: &str| t.rsplit('.').next().unwrap_or("").to_owned();
    let systems = specs
        .iter()
        .map(|s| cache.get(directory, lookup, &s.template))
        .collect::<Vec<_>>();
    for (s, sys) in specs.iter().zip(&systems) {
        match sys {
            None => diagnostics.push(format!(
                "{}: Niagara system not exported",
                short(&s.template)
            )),
            Some(sys) => {
                for e in &sys.emitters {
                    if let Some(error) = &e.error {
                        diagnostics.push(format!("{}/{}: {error}", short(&s.template), e.name));
                    }
                }
                let unbound = sys.unbound_functions();
                if !unbound.is_empty() {
                    diagnostics.push(format!(
                        "{}: approximated {}",
                        short(&s.template),
                        unbound.join(", ")
                    ));
                }
            }
        }
    }
    diagnostics.sort();
    diagnostics.dedup();
    let total = frames as f32 / SIM_FPS;
    let loop_from = loop_start
        .filter(|&l| l < frames)
        .map(|l| l as f32 / SIM_FPS);
    let steps = match loop_from {
        Some(l) => frames + 2 * ((total - l) * SIM_FPS).round() as usize,
        None => frames,
    };
    let records = ((total * BAKE_FPS).ceil() as usize).max(1);
    let mut recorded: Vec<Option<Vec<Baked>>> = vec![None; records];
    let camera = camera_for(basis);
    let root0 = env.bone(0, 0.).map_or(Vec3::ZERO, |t| t.translation);
    let mut active: Vec<Active> = Vec::new();
    let mut serial: u16 = 0;
    let mut slots: HashMap<(usize, usize, usize), u8> = HashMap::new();
    let mut renderers: Vec<Renderer> = Vec::new();
    let mut pass = 0;
    let mut prev_clock = -1.0f32;
    let dt = 1. / SIM_FPS;
    for step in 0..steps {
        let raw = step as f32 * dt;
        let clock = match loop_from {
            Some(l) if raw >= total => l + (raw - total) % (total - l),
            _ => raw.min(total - 1e-4),
        };
        let wrapped = clock < prev_clock;
        if wrapped {
            pass += 1;
        }
        let crossed = |x: f32| -> bool {
            if !wrapped {
                x > prev_clock && x <= clock
            } else {
                x > prev_clock || (loop_from.is_some_and(|l| x >= l) && x <= clock)
            }
        };
        for (i, spec) in specs.iter().enumerate() {
            let Some(asset) = &systems[i] else { continue };
            for span in &spec.intervals {
                if crossed(span[0]) {
                    if active.len() >= MAX_ACTIVE {
                        active.remove(0);
                    }
                    serial = serial.wrapping_add(1);
                    let owner = owner_transform(env, spec, clock);
                    active.push(Active {
                        spec: i,
                        serial,
                        instance: SystemInstance::new(
                            asset.clone(),
                            (serial as i32) * 7919 + i as i32,
                        ),
                        fixed: (!spec.attached).then_some(owner),
                        previous: None,
                        stop_at: spec.timed.then_some(span[1]),
                    });
                }
            }
        }
        for a in &mut active {
            let spec = &specs[a.spec];
            if let Some(stop) = a.stop_at {
                if crossed(stop) {
                    if spec.destroy_at_end {
                        a.instance.complete = true;
                    } else {
                        a.instance.deactivate();
                    }
                    a.stop_at = None;
                }
            }
            if a.instance.complete {
                continue;
            }
            let owner = a.fixed.unwrap_or_else(|| owner_transform(env, spec, clock));
            let velocity = a
                .previous
                .map_or(Vec3::ZERO, |p| (owner.translation - p) / dt);
            a.previous = Some(owner.translation);
            let ctx = TickContext {
                dt,
                owner,
                owner_velocity: velocity,
                time: clock,
                env,
                camera,
            };
            a.instance.tick(&ctx);
            for e in a.instance.errors.drain(..) {
                let e = format!("{}: {e}", short(&spec.template));
                if diagnostics.len() < 64 && !diagnostics.contains(&e) {
                    diagnostics.push(e);
                }
            }
        }
        active.retain(|a| !a.instance.complete);
        prev_clock = clock;
        let slot = (clock * BAKE_FPS).round() as usize;
        let on_sample = ((clock * BAKE_FPS) - slot as f32).abs() < 0.26 && slot < records;
        let take = match loop_from {
            Some(l) => (pass == 0 && clock < l) || pass == 2,
            None => true,
        };
        if !on_sample || !take || recorded[slot].is_some() {
            continue;
        }
        let mut frame = Vec::new();
        for a in &active {
            let owner = a
                .fixed
                .unwrap_or_else(|| owner_transform(env, &specs[a.spec], clock));
            let sys = a.instance.asset.clone();
            for (e, em) in sys.emitters.iter().enumerate() {
                for (r, rd) in em.renderers.iter().enumerate() {
                    if !rd.enabled || matches!(rd.kind, RendererKind::Other) {
                        continue;
                    }
                    let key = (a.spec, e, r);
                    let slot = match slots.get(&key) {
                        Some(s) => *s,
                        None => {
                            let native = (renderers.len() < crate::effects::MAX_RENDERERS)
                                .then(|| convert_renderer(rd, assets))
                                .flatten();
                            let s = match native {
                                Some(native) => {
                                    // GPU compute emitters run as a labelled approximation.
                                    let label = if em.approx.is_some() {
                                        " [GPU approximation]"
                                    } else {
                                        ""
                                    };
                                    renderers.push(Renderer {
                                        name: format!("{}/{}{label}", sys.name, em.name),
                                        ..native
                                    });
                                    (renderers.len() - 1) as u8
                                }
                                None => u8::MAX,
                            };
                            slots.insert(key, s);
                            s
                        }
                    };
                    if slot == u8::MAX {
                        continue;
                    }
                    let renderer = renderers[slot as usize].clone();
                    let tint = renderer
                        .shader
                        .strip_prefix("fnemote/fx/m_")
                        .and_then(|k| assets.tints.get(k))
                        .copied()
                        .unwrap_or([1.; 3]);
                    let modulate = renderer
                        .shader
                        .strip_prefix("fnemote/fx/m_")
                        .is_some_and(|k| assets.modulate.contains(k));
                    for p in a.instance.particles(e, r) {
                        if frame.len() >= MAX_DRAWN {
                            break;
                        }
                        let world = if em.local_space {
                            owner.point(p.position)
                        } else {
                            p.position
                        };
                        let local = basis * (reflect_point(world) - reflect_point(root0)) * scale;
                        let mut c = p.color;
                        for (x, t) in c.iter_mut().zip(tint) {
                            *x *= t;
                        }
                        let peak = c[0].max(c[1]).max(c[2]);
                        if peak > 1. {
                            for x in &mut c[..3] {
                                *x /= peak;
                            }
                        }
                        if c[3] <= 0.004 || !local.is_finite() {
                            continue;
                        }
                        if modulate {
                            let a = c[3].clamp(0., 1.);
                            c = [(1. - c[0]) * a, (1. - c[1]) * a, (1. - c[2]) * a, 1.];
                        }
                        let rgba = c.map(|x| (x.clamp(0., 1.) * 255.).round() as u8);
                        let to_model = |v: Vec3| {
                            (basis
                                * reflect_point(if em.local_space {
                                    owner.rotation * v
                                } else {
                                    v
                                }))
                            .normalize_or_zero()
                            .to_array()
                        };
                        let direction = to_model(if renderer.align == Align::Custom {
                            p.alignment
                        } else {
                            p.velocity
                        });
                        let normal = if renderer.facing == Facing::Custom {
                            to_model(p.facing)
                        } else {
                            [0.; 3]
                        };
                        let id = (p.id as u16).wrapping_add(a.serial.wrapping_mul(4099));
                        let mut out = Baked {
                            renderer: slot,
                            id,
                            position: local.to_array(),
                            rgba,
                            direction,
                            normal,
                            ..Default::default()
                        };
                        match renderer.kind {
                            Kind::Sprite => {
                                out.size = [p.size[0].abs() * scale, p.size[1].abs() * scale];
                                out.rotation = p.rotation;
                                let count = (renderer.sub_image[0] as u32
                                    * renderer.sub_image[1] as u32)
                                    .max(1);
                                out.sub_image = (p.sub_image.max(0.).floor() as u32 % count) as u8;
                            }
                            Kind::Mesh => {
                                let meshes = &rd.properties["Meshes"];
                                let mesh = meshes
                                    .get(p.mesh_index.max(0) as usize)
                                    .or_else(|| meshes.get(0));
                                let local_rot =
                                    mesh.map_or(Quat::IDENTITY, |m| ue_rotator(&m["Rotation"]));
                                let mesh_scale =
                                    mesh.map_or(Vec3::ONE, |m| ue_vec(&m["Scale"], Vec3::ONE));
                                let q = if em.local_space {
                                    owner.rotation * p.orientation
                                } else {
                                    p.orientation
                                };
                                out.orientation = (basis * reflect_quat(q.normalize() * local_rot))
                                    .normalize()
                                    .to_array();
                                out.scale = (p.scale * mesh_scale * (scale / 0.4)).to_array();
                                out.mesh = p.mesh_index.clamp(0, 255) as u8;
                            }
                            Kind::Ribbon => {
                                out.size = [p.width.abs() * scale, 0.];
                                out.ribbon =
                                    (p.ribbon_id as u16).wrapping_add(a.serial.wrapping_mul(97));
                                out.link = p.link_order;
                            }
                            Kind::Light => out.radius = p.radius * scale,
                        }
                        frame.push(out);
                    }
                }
            }
        }
        frame.sort_by_key(|p| (p.renderer, p.id));
        frame.dedup_by_key(|p| (p.renderer, p.id));
        recorded[slot] = Some(frame);
    }
    if renderers.is_empty() {
        return (!diagnostics.is_empty()).then_some(BakeResult {
            track: Vec::new(),
            renderers,
            diagnostics,
        });
    }
    let mut writer = Writer::new(BAKE_FPS, renderers.clone());
    let mut last = Vec::new();
    for frame in recorded {
        let frame = frame.unwrap_or_else(|| last.clone());
        writer.push_frame(&frame);
        last = frame;
    }
    let empty = writer.is_empty();
    let track = writer.finish();
    Some(BakeResult {
        track: if empty { Vec::new() } else { track },
        renderers,
        diagnostics,
    })
}

fn owner_transform(env: &PsaEnvironment, spec: &EffectSpec, clock: f32) -> Transform {
    let base = if spec.socket.is_empty() || spec.socket.eq_ignore_ascii_case("None") {
        env.bone(0, clock).unwrap_or_default()
    } else {
        env.named(&spec.socket, clock)
            .or_else(|| env.bone(0, clock))
            .unwrap_or_default()
    };
    base.compose(&Transform {
        translation: spec.location,
        rotation: spec.rotation,
        scale: spec.scale,
    })
}

fn convert_renderer(rd: &RendererDesc, assets: &RenderAssets) -> Option<Renderer> {
    let p = &rd.properties;
    let shader_for = |path: Option<&String>| -> Option<String> {
        let key = assets.materials.get(&path?.to_lowercase())?.clone();
        assets.used_materials.lock().unwrap().insert(key.clone());
        Some(format!("fnemote/fx/m_{key}"))
    };
    let facing = if p["FacingMode"]
        .as_str()
        .is_some_and(|f| f.ends_with("CustomFacingVector"))
    {
        Facing::Custom
    } else {
        Facing::Camera
    };
    let align = match p["Alignment"].as_str().unwrap_or("") {
        a if a.ends_with("VelocityAligned") => Align::Velocity,
        a if a.ends_with("CustomAlignment") => Align::Custom,
        _ => Align::None,
    };
    let base = Renderer {
        kind: Kind::Sprite,
        shader: String::new(),
        models: Vec::new(),
        sub_image: [
            rd.sub_image[0].clamp(1., 64.) as u8,
            rd.sub_image[1].clamp(1., 64.) as u8,
        ],
        facing,
        align,
        sort: rd.sort_hint,
        name: String::new(),
        pivot: [
            p["PivotInUVSpace"]["X"].as_f64().unwrap_or(0.5) as f32,
            p["PivotInUVSpace"]["Y"].as_f64().unwrap_or(0.5) as f32,
        ],
    };
    match rd.kind {
        RendererKind::Sprite | RendererKind::Ribbon => {
            let shader = shader_for(rd.material.as_ref())?;
            if let Some(key) = shader.strip_prefix("fnemote/fx/m_") {
                assets
                    .sprite_materials
                    .lock()
                    .unwrap()
                    .insert(key.to_owned());
            }
            let kind = if matches!(rd.kind, RendererKind::Ribbon) {
                Kind::Ribbon
            } else {
                Kind::Sprite
            };
            Some(Renderer {
                kind,
                shader,
                ..base
            })
        }
        RendererKind::Light => Some(Renderer {
            kind: Kind::Light,
            ..base
        }),
        RendererKind::Mesh => {
            let models = p["Meshes"]
                .as_array()?
                .iter()
                .map(|m| {
                    assets
                        .models
                        .get(&reference_path(&m["Mesh"]).to_lowercase())
                        .cloned()
                })
                .collect::<Vec<_>>();
            if models.iter().all(Option::is_none) {
                return None;
            }
            let fallback = models.iter().flatten().next().cloned().unwrap_or_default();
            // Mesh renderers normally replace the mesh's (often WorldGridMaterial)
            // materials with the effect material in OverrideMaterials.
            let override_material = (p["bOverrideMaterials"].as_bool() == Some(true))
                .then(|| {
                    p["OverrideMaterials"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find_map(|m| {
                            m["ExplicitMat"]["ObjectPath"]
                                .as_str()
                                .map(|_| reference_path(&m["ExplicitMat"]))
                        })
                })
                .flatten();
            let shader = shader_for(override_material.as_ref()).unwrap_or_default();
            if shader.is_empty() {
                let assets_used = p["Meshes"]
                    .as_array()?
                    .iter()
                    .map(|m| reference_path(&m["Mesh"]).to_lowercase())
                    .collect::<Vec<_>>();
                if assets_used.iter().all(|m| assets.grid_models.contains(m)) {
                    return None;
                }
            } else if let Some(key) = shader.strip_prefix("fnemote/fx/m_") {
                assets.mesh_materials.lock().unwrap().insert(key.to_owned());
            }
            // Mesh FacingMode: Default follows the particle orientation
            // (Custom); CameraPosition/CameraPlane turn the mesh X axis to the
            // viewer, with Z held vertical when the axis is locked.
            let camera = p["FacingMode"]
                .as_str()
                .is_some_and(|f| f.ends_with("CameraPosition") || f.ends_with("CameraPlane"));
            let facing = if camera {
                Facing::Camera
            } else {
                Facing::Custom
            };
            let align = if camera && p["bLockedAxisEnable"].as_bool() == Some(true) {
                Align::Custom
            } else {
                Align::None
            };
            Some(Renderer {
                kind: Kind::Mesh,
                shader,
                models: models
                    .into_iter()
                    .map(|m| m.unwrap_or_else(|| fallback.clone()))
                    .collect(),
                facing,
                align,
                ..base
            })
        }
        RendererKind::Other => None,
    }
}
