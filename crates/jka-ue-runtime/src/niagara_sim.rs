//! CPU Niagara system host for cooked UE exports.
//!
//! Mirrors UE 5.8 `FNiagaraSystemSimulation` / `FNiagaraEmitterInstanceImpl`
//! scheduling: system spawn/update scripts run over the system dataset, their
//! results are copied by name into each emitter's spawn/update parameter
//! stores, spawn infos drive particle spawn batches and particle update runs
//! with acquire-index compaction. Constant tables use the engine order
//! `[Global, System, Owner, Emitter, External, (previous copies), Literals]`
//! for particle scripts and `[Global, External, Literals]` for system scripts.
//!
//! All positions stay in Unreal component/world units (cm, Z up). The caller
//! supplies the owner (attach socket) transform and source skeleton through
//! [`Environment`]. GPU emitters have no VM bytecode: they run through
//! [`crate::niagara_gpu_approx`], a labelled approximation, or are reported.
use crate::niagara::Curve;
use crate::vector_vm::{External, Host, Kernel, Numeric, RandomStream};
use glam::{Mat3, Quat, Vec3};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

const MAX_PARTICLES_PER_EMITTER: usize = 4096;
const GLOBAL_BYTES: usize = 32;
const SYSTEM_BYTES: usize = 64;
const OWNER_BYTES: usize = 512;
const EMITTER_BYTES: usize = 32;
/// ENiagaraExecutionState.
pub const ACTIVE: u32 = 0;
pub const INACTIVE: u32 = 1;
pub const INACTIVE_CLEAR: u32 = 2;
pub const COMPLETE: u32 = 3;
pub const DISABLED: u32 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Comps {
    pub f: u16,
    pub i: u16,
    pub h: u16,
}
/// Component counts of a Niagara type as laid out by `FNiagaraTypeLayoutInfo`.
pub fn components(type_def: &Value) -> Result<Comps, String> {
    let class = type_def["ClassStructOrEnum"]["ObjectName"]
        .as_str()
        .and_then(|s| s.split('\'').nth(1))
        .unwrap_or("");
    if type_def["UnderlyingType"].as_i64() == Some(3) {
        return Ok(Comps { f: 0, i: 1, h: 0 });
    }
    let (f, i, h) = match class {
        "NiagaraFloat" | "Float" => (1, 0, 0),
        "Vector2f" | "Vector2D" | "NiagaraVector2" => (2, 0, 0),
        "Vector3f" | "Vector" | "NiagaraPosition" | "NiagaraVector3" => (3, 0, 0),
        "Vector4f" | "Vector4" | "LinearColor" | "Quat4f" | "Quat" | "NiagaraVector4" => (4, 0, 0),
        "Matrix44f" | "Matrix" | "NiagaraMatrix" => (16, 0, 0),
        "NiagaraInt32" | "NiagaraBool" | "Int32" | "Bool" => (0, 1, 0),
        "NiagaraID" => (0, 2, 0),
        "NiagaraSpawnInfo" => (2, 2, 0),
        "NiagaraRandInfo" => (0, 3, 0),
        "NiagaraHalf" => (0, 0, 1),
        "NiagaraHalfVector2" => (0, 0, 2),
        "NiagaraHalfVector3" => (0, 0, 3),
        "NiagaraHalfVector4" => (0, 0, 4),
        _ if class.starts_with('E') => (0, 1, 0),
        _ => return Err(format!("unsupported Niagara dataset type {class}")),
    };
    Ok(Comps { f, i, h })
}

#[derive(Clone, Debug)]
pub struct Var {
    pub name: String,
    pub comps: Comps,
    pub f: u16,
    pub i: u16,
    pub h: u16,
}
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub vars: Vec<Var>,
    pub floats: usize,
    pub ints: usize,
    pub halfs: usize,
    index: HashMap<String, usize>,
}
impl Layout {
    /// `FNiagaraDataSetCompiledData::BuildLayout`: variables keep their order
    /// and receive consecutive float/int/half component ranges.
    pub fn build<'a>(vars: impl IntoIterator<Item = &'a Value>) -> Result<Self, String> {
        let mut layout = Layout::default();
        for v in vars {
            let name = v["Name"].as_str().ok_or("dataset variable without name")?;
            if layout.index.contains_key(name) {
                continue;
            }
            let comps = components(&v["TypeDef"])?;
            layout.index.insert(name.to_owned(), layout.vars.len());
            layout.vars.push(Var {
                name: name.to_owned(),
                comps,
                f: layout.floats as u16,
                i: layout.ints as u16,
                h: layout.halfs as u16,
            });
            layout.floats += comps.f as usize;
            layout.ints += comps.i as usize;
            layout.halfs += comps.h as usize;
            if layout.floats + layout.ints + layout.halfs > 4096 {
                return Err("Niagara dataset layout exceeds limit".into());
            }
        }
        Ok(layout)
    }
    pub fn get(&self, name: &str) -> Option<&Var> {
        self.index.get(name).map(|&i| &self.vars[i])
    }
}

/// Column-major instance data. Words are raw 32-bit values.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    pub f: Vec<Vec<u32>>,
    pub i: Vec<Vec<u32>>,
    pub h: Vec<Vec<u32>>,
    pub len: usize,
}
impl Buffer {
    pub fn new(layout: &Layout) -> Self {
        Self {
            f: vec![Vec::new(); layout.floats],
            i: vec![Vec::new(); layout.ints],
            h: vec![Vec::new(); layout.halfs],
            len: 0,
        }
    }
    pub(crate) fn reserve_len(&mut self, len: usize) {
        for c in self.f.iter_mut().chain(&mut self.i).chain(&mut self.h) {
            c.resize(len, 0);
        }
    }
    fn append(&mut self, other: &Buffer) {
        for (a, b) in self
            .f
            .iter_mut()
            .zip(&other.f)
            .chain(self.i.iter_mut().zip(&other.i))
            .chain(self.h.iter_mut().zip(&other.h))
        {
            a.truncate(self.len);
            a.extend_from_slice(&b[..other.len.min(b.len())]);
            a.resize(self.len + other.len, 0);
        }
        self.len += other.len;
    }
    fn copy_instance(&mut self, from: &Buffer, src: usize, dst: usize) {
        for (a, b) in self
            .f
            .iter_mut()
            .zip(&from.f)
            .chain(self.i.iter_mut().zip(&from.i))
            .chain(self.h.iter_mut().zip(&from.h))
        {
            a[dst] = b.get(src).copied().unwrap_or(0);
        }
    }
    /// Move instance `src` over `dst` within this buffer.
    pub(crate) fn copy_within(&mut self, src: usize, dst: usize) {
        for c in self.f.iter_mut().chain(&mut self.i).chain(&mut self.h) {
            if let Some(&w) = c.get(src) {
                if let Some(slot) = c.get_mut(dst) {
                    *slot = w;
                }
            }
        }
    }
    pub(crate) fn truncate(&mut self, len: usize) {
        for c in self.f.iter_mut().chain(&mut self.i).chain(&mut self.h) {
            c.truncate(len);
        }
        self.len = len;
    }
    fn column(&self, kind: Numeric, c: u16) -> Option<&Vec<u32>> {
        match kind {
            Numeric::Float => self.f.get(c as usize),
            Numeric::Int => self.i.get(c as usize),
            Numeric::Half => self.h.get(c as usize),
        }
    }
    pub fn float(&self, var: &Var, comp: usize, index: usize) -> f32 {
        if comp >= var.comps.f as usize {
            return 0.;
        }
        f32::from_bits(self.f[var.f as usize + comp][index])
    }
    pub fn int(&self, var: &Var, comp: usize, index: usize) -> i32 {
        if comp >= var.comps.i as usize {
            return 0;
        }
        self.i[var.i as usize + comp][index] as i32
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}
impl Transform {
    pub fn point(&self, p: Vec3) -> Vec3 {
        self.translation + self.rotation * (p * self.scale)
    }
    pub fn compose(&self, child: &Transform) -> Transform {
        Transform {
            translation: self.point(child.translation),
            rotation: self.rotation * child.rotation,
            scale: self.scale * child.scale,
        }
    }
}

/// Source skeleton and collision supplied by the caller. Bone indices are in
/// the caller's bone order; filtered DI bone names resolve against [`bones`].
pub trait Environment {
    fn bones(&self) -> &[String];
    /// Component-space transform of `bone` at `seconds` on the owner timeline.
    fn bone(&self, bone: usize, seconds: f32) -> Option<Transform>;
    /// (bone, relative transform) of a skeleton socket.
    fn socket(&self, name: &str) -> Option<(usize, Transform)>;
    /// Height of the floor plane, in component space.
    fn floor(&self) -> f32 {
        0.
    }
}
/// An environment without a skeleton; skeletal DIs report the owner origin.
pub struct NoSkeleton;
impl Environment for NoSkeleton {
    fn bones(&self) -> &[String] {
        &[]
    }
    fn bone(&self, _: usize, _: f32) -> Option<Transform> {
        None
    }
    fn socket(&self, _: &str) -> Option<(usize, Transform)> {
        None
    }
}

/// Static vector field (`UVectorFieldStatic`): FFloat16Color texels over Bounds.
#[derive(Debug)]
pub struct VectorField {
    size: [usize; 3],
    min: Vec3,
    extent: Vec3,
    texels: Vec<Vec3>,
}
impl VectorField {
    /// `object` is the exported VectorFieldStatic properties; `bytes` its SourceData.
    pub fn from_export(object: &Value, bytes: &[u8]) -> Result<Self, String> {
        let dim = |k: &str| {
            object[k]
                .as_u64()
                .filter(|n| (1..=256).contains(n))
                .map(|n| n as usize)
                .ok_or("invalid vector field size")
        };
        let size = [dim("SizeX")?, dim("SizeY")?, dim("SizeZ")?];
        let b = &object["Bounds"];
        let v = |o: &Value| {
            Vec3::new(
                o["X"].as_f64().unwrap_or(0.) as f32,
                o["Y"].as_f64().unwrap_or(0.) as f32,
                o["Z"].as_f64().unwrap_or(0.) as f32,
            )
        };
        let (min, max) = (v(&b["Min"]), v(&b["Max"]));
        let count = size[0] * size[1] * size[2];
        if bytes.len() != count * 8 || (max - min).min_element() <= 1e-4 {
            return Err("vector field data does not match its size/bounds".into());
        }
        let h = |i: usize| half::f16::from_le_bytes([bytes[i], bytes[i + 1]]).to_f32();
        let texels = (0..count)
            .map(|i| Vec3::new(h(i * 8), h(i * 8 + 2), h(i * 8 + 4)))
            .collect();
        Ok(Self {
            size,
            min,
            extent: max - min,
            texels,
        })
    }
    /// `UNiagaraDataInterfaceVectorField::SampleVectorField` + `FilteredSample`.
    pub fn sample(&self, position: Vec3, tile: [bool; 3]) -> Vec3 {
        let p = (position - self.min) / self.extent
            * Vec3::new(
                self.size[0] as f32,
                self.size[1] as f32,
                self.size[2] as f32,
            )
            - Vec3::splat(0.5);
        let mut idx = [[0usize; 2]; 3];
        let mut w = [0f32; 3];
        for a in 0..3 {
            let n = self.size[a] as i64;
            let f = p[a].floor();
            w[a] = p[a] - f;
            for (k, slot) in idx[a].iter_mut().enumerate() {
                let i = f as i64 + k as i64;
                *slot = if tile[a] {
                    i.rem_euclid(n) as usize
                } else {
                    i.clamp(0, n - 1) as usize
                };
            }
            if !tile[a] && (p[a] <= 0. || p[a] >= (n - 1) as f32) {
                w[a] = w[a].clamp(0., 1.);
            }
        }
        let at =
            |x: usize, y: usize, z: usize| self.texels[x + self.size[0] * (y + self.size[1] * z)];
        let mut out = Vec3::ZERO;
        for (dz, wz) in [(0, 1. - w[2]), (1, w[2])] {
            for (dy, wy) in [(0, 1. - w[1]), (1, w[1])] {
                for (dx, wx) in [(0, 1. - w[0]), (1, w[0])] {
                    out += at(idx[0][dx], idx[1][dy], idx[2][dz]) * (wx * wy * wz);
                }
            }
        }
        out
    }
}

/// Supplies vector fields referenced by data interfaces, by asset path.
pub type FieldResolver<'a> = &'a dyn Fn(&str) -> Option<Arc<VectorField>>;

#[derive(Debug)]
enum Di {
    Curve(Curve),
    FloatArray {
        comps: usize,
        data: Vec<f32>,
    },
    IntArray(Vec<i32>),
    Skeletal {
        bones: Vec<String>,
        sockets: Vec<String>,
    },
    SpriteInfo {
        sub_image: [f32; 2],
    },
    MeshInfo {
        meshes: usize,
    },
    ParticleRead {
        emitter: String,
    },
    Camera,
    Collision,
    VectorField {
        field: Option<Arc<VectorField>>,
        tile: [bool; 3],
    },
    Inert,
}
impl Di {
    fn parse(object: &Value, renderers: &HashMap<String, Value>, fields: FieldResolver) -> Di {
        let p = &object["Properties"];
        let floats = |key: &str| -> Vec<f32> {
            p[key]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|v| match v {
                    Value::Object(o) => ["X", "Y", "Z", "W", "R", "G", "B", "A"]
                        .iter()
                        .filter_map(|k| o.get(*k).and_then(Value::as_f64))
                        .map(|n| n as f32)
                        .collect::<Vec<_>>(),
                    v => v.as_f64().map(|n| vec![n as f32]).unwrap_or_default(),
                })
                .collect()
        };
        match object["Type"].as_str().unwrap_or("") {
            "NiagaraDataInterfaceCurve"
            | "NiagaraDataInterfaceVector2DCurve"
            | "NiagaraDataInterfaceVectorCurve"
            | "NiagaraDataInterfaceColorCurve" => Curve::from_export(object)
                .map(Di::Curve)
                .unwrap_or(Di::Inert),
            "NiagaraDataInterfaceArrayFloat" => Di::FloatArray {
                comps: 1,
                data: floats("FloatData"),
            },
            "NiagaraDataInterfaceArrayFloat2" => Di::FloatArray {
                comps: 2,
                data: floats("InternalFloatData"),
            },
            "NiagaraDataInterfaceArrayFloat3" => Di::FloatArray {
                comps: 3,
                data: floats("InternalFloatData"),
            },
            "NiagaraDataInterfaceArrayPosition" => Di::FloatArray {
                comps: 3,
                data: floats("PositionData"),
            },
            "NiagaraDataInterfaceArrayFloat4" => Di::FloatArray {
                comps: 4,
                data: floats("InternalFloatData"),
            },
            "NiagaraDataInterfaceArrayQuat" => Di::FloatArray {
                comps: 4,
                data: floats("InternalQuatData"),
            },
            "NiagaraDataInterfaceArrayColor" => Di::FloatArray {
                comps: 4,
                data: floats("ColorData"),
            },
            "NiagaraDataInterfaceArrayInt32" => Di::IntArray(
                p["IntData"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                    .map(|n| n as i32)
                    .collect(),
            ),
            "NiagaraDataInterfaceArrayBool" => Di::IntArray(
                p["BoolData"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|b| b.as_bool().unwrap_or(false) as i32)
                    .collect(),
            ),
            "NiagaraDataInterfaceSkeletalMesh" | "NiagaraDataInterfaceSocketReader" => {
                let names = |key: &str| {
                    p[key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                };
                Di::Skeletal {
                    bones: names("FilteredBones"),
                    sockets: names("FilteredSockets"),
                }
            }
            "NiagaraDataInterfaceSpriteRendererInfo" => {
                let key = reference_name(&p["SpriteRenderer"]).unwrap_or_default();
                let size = renderers.get(&key).map(|r| &r["SubImageSize"]);
                let x = size.and_then(|s| s["X"].as_f64()).unwrap_or(1.) as f32;
                let y = size.and_then(|s| s["Y"].as_f64()).unwrap_or(1.) as f32;
                Di::SpriteInfo {
                    sub_image: [x.max(1.), y.max(1.)],
                }
            }
            "NiagaraDataInterfaceMeshRendererInfo" => {
                let key = reference_name(&p["MeshRenderer"]).unwrap_or_default();
                let meshes = renderers
                    .get(&key)
                    .and_then(|r| r["Meshes"].as_array())
                    .map_or(1, Vec::len);
                Di::MeshInfo { meshes }
            }
            "NiagaraDataInterfaceParticleRead" => Di::ParticleRead {
                emitter: p["EmitterBinding"]["BindingName"]
                    .as_str()
                    .or_else(|| p["EmitterBinding"].as_str())
                    .unwrap_or("")
                    .to_owned(),
            },
            "NiagaraDataInterfaceCamera" => Di::Camera,
            "NiagaraDataInterfaceCollisionQuery" => Di::Collision,
            "NiagaraDataInterfaceVectorField" => Di::VectorField {
                field: p["Field"]["ObjectPath"]
                    .as_str()
                    .and_then(|_| fields(&reference_path(&p["Field"]))),
                tile: [
                    p["bTileX"].as_bool().unwrap_or(false),
                    p["bTileY"].as_bool().unwrap_or(false),
                    p["bTileZ"].as_bool().unwrap_or(false),
                ],
            },
            _ => Di::Inert,
        }
    }
}

fn reference_name(v: &Value) -> Option<String> {
    v["ObjectName"]
        .as_str()?
        .split('\'')
        .nth(1)?
        .rsplit(['.', ':'])
        .next()
        .map(str::to_owned)
}
/// Export index from `ObjectPath` "/Path/Package.N".
fn export_index(v: &Value) -> Option<usize> {
    v["ObjectPath"].as_str()?.rsplit_once('.')?.1.parse().ok()
}

/// Resolution of one VM external function call site.
#[derive(Debug, Clone)]
enum Call {
    /// Data interface index and the function's `Attribute` specifier, if any.
    Di(usize, Option<String>),
    FastPath,
    Unbound,
}

#[derive(Debug)]
struct ScriptAsset {
    kernel: Kernel,
    calls: Vec<Call>,
    literals: Vec<u8>,
    /// Current external parameters followed by the PREV_ copy when interpolated.
    params: Vec<u8>,
    external_size: usize,
    interpolated: bool,
    names: HashMap<String, usize>,
}
impl ScriptAsset {
    fn offset(&self, name: &str) -> Option<usize> {
        self.names.get(name).copied()
    }
}

#[derive(Debug, Clone)]
pub struct Binding {
    /// Particle dataset variable name.
    pub attribute: String,
}
#[derive(Debug, Clone)]
pub struct RendererDesc {
    pub kind: RendererKind,
    pub name: String,
    pub material: Option<String>,
    pub enabled: bool,
    pub properties: Value,
    pub bindings: HashMap<&'static str, String>,
    pub sub_image: [f32; 2],
    pub sort_hint: i32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Sprite,
    Mesh,
    Ribbon,
    Light,
    Other,
}

/// Event handler on a receiving emitter (`FNiagaraEventScriptProperties`).
#[derive(Debug)]
pub struct Handler {
    script: ScriptAsset,
    every_particle: bool,
    spawn_number: u32,
    min_spawn: u32,
    random: bool,
    max_events: usize,
    /// Source emitter handle GUID without dashes, lowercase; empty = self.
    source: String,
    event: String,
}

fn guid_key(s: &str) -> String {
    s.replace('-', "").to_ascii_lowercase()
}

#[derive(Debug)]
pub struct EmitterAsset {
    pub name: String,
    pub local_space: bool,
    pub gpu: bool,
    pub seed: i32,
    pub layout: Layout,
    pub spawn_infos: Vec<String>,
    pub renderers: Vec<RendererDesc>,
    pub handle_id: String,
    /// GPU compute emitter run as a labelled approximation (see
    /// `niagara_gpu_approx`); `None` for CPU emitters.
    pub approx: Option<crate::niagara_gpu_approx::GpuApprox>,
    /// Stateless (lightweight) emitter, simulated from its cooked module
    /// data (see `niagara_stateless`); always local space.
    pub stateless: Option<crate::niagara_stateless::Stateless>,
    /// Event generators written by the update / spawn scripts as datasets 1..n.
    update_events: Vec<(String, Layout)>,
    spawn_events: Vec<(String, Layout)>,
    handlers: Vec<Handler>,
    spawn: Option<ScriptAsset>,
    update: Option<ScriptAsset>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct SystemAsset {
    pub name: String,
    pub emitters: Vec<EmitterAsset>,
    system_layout: Layout,
    spawn_params: Layout,
    update_params: Layout,
    spawn: ScriptAsset,
    update: ScriptAsset,
    bindings: Value,
    dis: Vec<Di>,
    pub max_duration: Option<f32>,
}

fn script_object<'a>(objects: &'a [Value], reference: &Value) -> Option<&'a Value> {
    let index = export_index(reference);
    let name = reference["ObjectName"]
        .as_str()?
        .split('\'')
        .nth(1)?
        .rsplit(['.', ':'])
        .next()?
        .to_owned();
    index
        .and_then(|i| objects.get(i))
        .filter(|o| o["Name"] == name.as_str())
        .or_else(|| {
            let outer = reference["ObjectName"]
                .as_str()?
                .split('\'')
                .nth(1)?
                .to_owned();
            objects.iter().find(|o| {
                o["Name"] == name.as_str()
                    && o["Outer"]["ObjectName"].as_str().is_none_or(|s| {
                        outer.contains(
                            s.split('\'')
                                .nth(1)
                                .unwrap_or("")
                                .rsplit(['.', ':'])
                                .next()
                                .unwrap_or(""),
                        )
                    })
            })
        })
}

fn bytes(v: &Value) -> Vec<u8> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .map(|n| n as u8)
        .collect()
}

impl SystemAsset {
    /// Parse a cooked `NiagaraSystem` package exported as an object array.
    pub fn parse(package: &Value) -> Result<Self, String> {
        Self::parse_with(package, &|_| None)
    }
    /// Parse with vector-field data supplied by `fields`.
    pub fn parse_with(package: &Value, fields: FieldResolver) -> Result<Self, String> {
        let objects = package
            .as_array()
            .ok_or("Niagara package is not an object array")?;
        let system = objects
            .iter()
            .find(|o| o["Type"] == "NiagaraSystem")
            .ok_or("package has no NiagaraSystem")?;
        let p = &system["Properties"];
        let compiled = &p["SystemCompiledData"];
        let system_layout = Layout::build(
            compiled["DataSetCompiledData"]["Variables"]
                .as_array()
                .into_iter()
                .flatten(),
        )?;
        let spawn_params = Layout::build(
            compiled["SpawnInstanceParamsDataSetCompiledData"]["Variables"]
                .as_array()
                .into_iter()
                .flatten(),
        )?;
        let update_params = Layout::build(
            compiled["UpdateInstanceParamsDataSetCompiledData"]["Variables"]
                .as_array()
                .into_iter()
                .flatten(),
        )?;
        let cooked = p["ScriptRuntimeCookedDataMap"]
            .as_array()
            .ok_or("system has no cooked script runtime data")?;
        let mut renderer_objects = HashMap::new();
        for o in objects.iter().filter(|o| {
            o["Type"]
                .as_str()
                .is_some_and(|t| t.ends_with("RendererProperties"))
        }) {
            renderer_objects.insert(
                o["Name"].as_str().unwrap_or("").to_owned(),
                o["Properties"].clone(),
            );
        }
        let mut dis = Vec::<Di>::new();
        let mut di_index = HashMap::<usize, usize>::new();
        let mut user_params = HashMap::new();
        let exposed = &p["ExposedParameters"];
        if let Some(offsets) = exposed["SortedParameterOffsets"].as_array() {
            let data = bytes(&exposed["ParameterData"]);
            for o in offsets {
                let (Some(name), Some(offset)) = (o["Name"].as_str(), o["Offset"].as_u64()) else {
                    continue;
                };
                if o["TypeDef"]["UnderlyingType"].as_i64() == Some(1) {
                    continue;
                }
                let size = components(&o["TypeDef"])
                    .map(|c| 4 * (c.f + c.i) as usize + 2 * c.h as usize)
                    .unwrap_or(4);
                if let Some(slice) = data.get(offset as usize..offset as usize + size) {
                    user_params.insert(name.to_owned(), slice.to_vec());
                }
            }
        }
        let mut load = |usage: &str,
                        emitter_id: &str,
                        script: Option<&Value>|
         -> Result<Option<ScriptAsset>, String> {
            let Some(script) = script else {
                return Ok(None);
            };
            let usage = script["Properties"]["Usage"].as_str().unwrap_or(usage);
            // Several event scripts share an emitter; their cooked data is keyed by UsageId.
            let usage_id = script["Properties"]["UsageId"]
                .as_str()
                .filter(|_| usage.ends_with("EventScript"))
                .map(guid_key);
            let entry = cooked.iter().find(|e| {
                e["Key"]["ScriptUsage"].as_str() == Some(usage)
                    && usage_id.as_ref().is_none_or(|u| {
                        e["Key"]["ScriptUsageId"]
                            .as_str()
                            .is_some_and(|k| guid_key(k) == *u)
                    })
                    && e["Key"]["EmitterHandleId"].as_str().is_some_and(|id| {
                        id.replace('-', "")
                            .eq_ignore_ascii_case(&emitter_id.replace('-', ""))
                    })
            });
            let Some(entry) = entry else {
                return Err(format!("no cooked runtime data for {usage}"));
            };
            let store = &entry["Value"]["CookedExecutionParameterStore"];
            let vm = &script["Properties"]["CachedScriptVM"];
            let code = bytes(&vm["ByteCode"]["Data"]);
            if code.is_empty() {
                return Err(format!("{usage} has no CPU VectorVM bytecode"));
            }
            let externals = vm["CalledVMExternalFunctions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| External {
                    owner: f["OwnerName"].as_str().unwrap_or("").into(),
                    name: f["Name"].as_str().unwrap_or("").into(),
                    inputs: f["InputParamLocations"].as_array().map_or(0, Vec::len),
                    outputs: f["NumOutputs"].as_u64().unwrap_or(0) as usize,
                })
                .collect::<Vec<_>>();
            let kernel = Kernel::decode(
                &code,
                vm["NumTempRegisters"].as_u64().unwrap_or(1).max(1) as usize,
                externals,
            )?;
            let mut names = HashMap::new();
            for o in store["SortedParameterOffsets"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if o["TypeDef"]["UnderlyingType"].as_i64() == Some(1) {
                    continue;
                }
                if let (Some(n), Some(off)) = (o["Name"].as_str(), o["Offset"].as_u64()) {
                    names.insert(n.to_owned(), off as usize);
                }
            }
            let mut params = bytes(&store["ParameterData"]);
            let external_size = store["ParameterSize"]
                .as_u64()
                .unwrap_or(params.len() as u64) as usize;
            if external_size > params.len() {
                params.resize(external_size, 0);
            }
            let interpolated =
                names.keys().any(|n| n.starts_with("PREV_")) || usage.ends_with("Interpolated");
            if interpolated && params.len() < external_size * 2 {
                params.resize(external_size * 2, 0);
            }
            for (name, value) in &user_params {
                if let Some(&off) = names.get(name) {
                    if let Some(slot) = params.get_mut(off..off + value.len()) {
                        slot.copy_from_slice(value);
                    }
                }
            }
            let resolved = entry["Value"]["CookedScriptRuntimeCompiledData"]
                ["ResolvedDataInterfaces"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let specifiers = vm["CalledVMExternalFunctions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    f["FunctionSpecifiers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|s| s["Key"] == "Attribute")
                        .and_then(|s| s["Value"].as_str())
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>();
            let mut calls = Vec::new();
            for (n, f) in kernel.external.iter().enumerate() {
                let call = if f.owner == "FastPathLibrary" {
                    Call::FastPath
                } else if let Some(r) = resolved.iter().find(|r| {
                    r["CompileName"].as_str() == Some(f.owner.as_str())
                        || r["Name"].as_str() == Some(f.owner.as_str())
                }) {
                    let reference = &r["ResolvedDataInterface"];
                    match script_object(objects, reference) {
                        Some(object) => {
                            let key = object as *const Value as usize;
                            let index = *di_index.entry(key).or_insert_with(|| {
                                dis.push(Di::parse(object, &renderer_objects, fields));
                                dis.len() - 1
                            });
                            Call::Di(index, specifiers.get(n).cloned().flatten())
                        }
                        None => Call::Unbound,
                    }
                } else {
                    Call::Unbound
                };
                calls.push(call);
            }
            Ok(Some(ScriptAsset {
                kernel,
                calls,
                literals: bytes(&vm["ScriptLiterals"]),
                params,
                external_size,
                interpolated,
                names,
            }))
        };
        let spawn_script = script_object(objects, &p["SystemSpawnScript"]);
        let update_script = script_object(objects, &p["SystemUpdateScript"]);
        let zero = "00000000-00000000-00000000-00000000";
        let spawn =
            load("SystemSpawnScript", zero, spawn_script)?.ok_or("system spawn script missing")?;
        let update = load("SystemUpdateScript", zero, update_script)?
            .ok_or("system update script missing")?;
        let spawn_attributes = spawn_script
            .map(|s| s["Properties"]["CachedScriptVM"]["Attributes"].clone())
            .unwrap_or(Value::Null);
        let update_attributes = update_script
            .map(|s| s["Properties"]["CachedScriptVM"]["Attributes"].clone())
            .unwrap_or(Value::Null);
        let mut emitters = Vec::new();
        let user = crate::niagara_stateless::user_parameters(system);
        for handle in p["EmitterHandles"].as_array().into_iter().flatten() {
            if handle["bIsEnabled"] == false {
                continue;
            }
            let id = handle["Id"].as_str().unwrap_or("").to_owned();
            if handle["EmitterMode"]
                .as_str()
                .is_some_and(|m| m.ends_with("Stateless"))
            {
                let Some(emitter) = script_object(objects, &handle["StatelessEmitter"]) else {
                    continue;
                };
                let name = emitter["Properties"]["UniqueEmitterName"]
                    .as_str()
                    .or(handle["Name"].as_str())
                    .unwrap_or("Emitter")
                    .to_owned();
                let (stateless, layout, error) =
                    match crate::niagara_stateless::Stateless::parse(objects, emitter, &user) {
                        Ok((s, l)) => {
                            let error = (!s.unsupported.is_empty()).then(|| {
                                format!(
                                    "stateless modules not simulated: {}",
                                    s.unsupported.join(", ")
                                )
                            });
                            (Some(s), l, error)
                        }
                        Err(e) => (
                            None,
                            Layout::default(),
                            Some(format!("stateless emitter: {e}")),
                        ),
                    };
                let renderers = emitter["Properties"]["RendererProperties"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| script_object(objects, r))
                    .map(renderer)
                    .collect();
                emitters.push(EmitterAsset {
                    name,
                    local_space: true,
                    gpu: false,
                    seed: 0,
                    layout,
                    spawn_infos: Vec::new(),
                    renderers,
                    handle_id: guid_key(&id),
                    approx: None,
                    stateless,
                    update_events: Vec::new(),
                    spawn_events: Vec::new(),
                    handlers: Vec::new(),
                    spawn: None,
                    update: None,
                    error,
                });
                continue;
            }
            let Some(emitter) = script_object(objects, &handle["VersionedInstance"]["Emitter"])
            else {
                continue;
            };
            let unique = emitter["Properties"]["UniqueEmitterName"]
                .as_str()
                .or(handle["Name"].as_str())
                .unwrap_or("Emitter")
                .to_owned();
            let version = handle["VersionedInstance"]["Version"]
                .as_str()
                .unwrap_or("");
            let data = emitter["Properties"]["VersionData"]
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .find(|v| v["Version"]["VersionGuid"] == version)
                        .or(a.first())
                })
                .cloned()
                .unwrap_or(Value::Null);
            let gpu = data["SimTarget"]
                .as_str()
                .is_some_and(|s| s.contains("GPU"));
            let spawn_obj = script_object(objects, &data["SpawnScriptProps"]["Script"]);
            let update_obj = script_object(objects, &data["UpdateScriptProps"]["Script"]);
            let mut error = None;
            let mut spawn = None;
            let mut update = None;
            let mut layout = Layout::default();
            let mut handlers = Vec::new();
            let generators = |key: &str| -> Vec<(String, Layout)> {
                data[key]["EventGenerators"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|g| {
                        Some((
                            g["ID"].as_str()?.to_owned(),
                            Layout::build(g["DataSetCompiledData"]["Variables"].as_array()?.iter())
                                .ok()?,
                        ))
                    })
                    .collect()
            };
            let update_events = generators("UpdateScriptProps");
            let spawn_events = generators("SpawnScriptProps");
            let mut approx = None;
            if gpu {
                let gpu_script = script_object(objects, &data["GPUComputeScript"]);
                let emitter_name = emitter["Name"].as_str().unwrap_or("");
                match crate::niagara_gpu_approx::GpuApprox::parse(
                    objects,
                    &data,
                    gpu_script,
                    emitter_name,
                ) {
                    Ok((a, l)) => {
                        approx = Some(a);
                        layout = l;
                    }
                    Err(e) => error = Some(format!("GPU compute emitter: {e}")),
                }
            } else {
                match (
                    load("ParticleSpawnScript", &id, spawn_obj),
                    load("ParticleUpdateScript", &id, update_obj),
                ) {
                    (Ok(s), Ok(u)) => {
                        spawn = s;
                        update = u;
                    }
                    (Err(e), _) | (_, Err(e)) => error = Some(e),
                }
                // GatherCompiledParticleAttributes: update attributes, then
                // spawn attributes not already present.
                let attrs = update_obj
                    .into_iter()
                    .chain(spawn_obj)
                    .flat_map(|s| {
                        s["Properties"]["CachedScriptVM"]["Attributes"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>();
                match Layout::build(attrs.iter()) {
                    Ok(l) => layout = l,
                    Err(e) => error = Some(e),
                }
                for h in data["EventHandlerScriptProps"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    let Ok(Some(script)) = load(
                        "ParticleEventScript",
                        &id,
                        script_object(objects, &h["Script"]),
                    ) else {
                        continue;
                    };
                    let mode = h["ExecutionMode"].as_str().unwrap_or("");
                    if mode.ends_with("SingleParticle") {
                        continue;
                    }
                    let source = h["SourceEmitterID"]
                        .as_str()
                        .map(guid_key)
                        .filter(|g| g.chars().any(|c| c != '0'))
                        .unwrap_or_default();
                    handlers.push(Handler {
                        script,
                        every_particle: mode.ends_with("EveryParticle"),
                        spawn_number: h["SpawnNumber"].as_u64().unwrap_or(0).min(4096) as u32,
                        min_spawn: h["MinSpawnNumber"].as_u64().unwrap_or(0).min(4096) as u32,
                        random: h["bRandomSpawnNumber"].as_bool().unwrap_or(false),
                        max_events: h["MaxEventsPerFrame"].as_u64().unwrap_or(0) as usize,
                        source,
                        event: h["SourceEventName"].as_str().unwrap_or("").to_owned(),
                    });
                }
            }
            let prefix = format!("{unique}.");
            let mut spawn_infos = Vec::new();
            for a in spawn_attributes
                .as_array()
                .into_iter()
                .flatten()
                .chain(update_attributes.as_array().into_iter().flatten())
            {
                let name = a["Name"].as_str().unwrap_or("");
                if name.starts_with(&prefix)
                    && components(&a["TypeDef"]).ok() == Some(Comps { f: 2, i: 2, h: 0 })
                    && a["TypeDef"]["ClassStructOrEnum"]["ObjectName"]
                        .as_str()
                        .is_some_and(|s| s.contains("NiagaraSpawnInfo"))
                    && !spawn_infos.iter().any(|s: &String| s == name)
                {
                    spawn_infos.push(name.to_owned());
                }
            }
            let mut renderers = Vec::new();
            for r in data["RendererProperties"].as_array().into_iter().flatten() {
                let Some(object) = script_object(objects, r) else {
                    continue;
                };
                renderers.push(renderer(object));
            }
            emitters.push(EmitterAsset {
                name: unique,
                local_space: data["bLocalSpace"].as_bool().unwrap_or(false),
                gpu,
                seed: data["RandomSeed"].as_i64().unwrap_or(0) as i32,
                layout,
                spawn_infos,
                renderers,
                handle_id: guid_key(&id),
                approx,
                stateless: None,
                update_events,
                spawn_events,
                handlers,
                spawn,
                update,
                error,
            });
        }
        let max_duration = p["MaxSystemDuration"].as_f64().map(|n| n as f32);
        Ok(Self {
            name: system["Name"].as_str().unwrap_or("").into(),
            emitters,
            system_layout,
            spawn_params,
            update_params,
            spawn,
            update,
            bindings: compiled.clone(),
            dis,
            max_duration,
        })
    }
    /// Data interfaces or functions without a native binding, for reports.
    pub fn unbound_functions(&self) -> Vec<String> {
        let mut out = Vec::new();
        let scripts = [&self.spawn, &self.update].into_iter().chain(
            self.emitters
                .iter()
                .flat_map(|e| e.spawn.iter().chain(e.update.iter())),
        );
        for s in scripts {
            for (f, call) in s.kernel.external.iter().zip(&s.calls) {
                let bound = match call {
                    Call::FastPath => f.name == "FastMatrixToQuaternion",
                    Call::Unbound => false,
                    Call::Di(i, _) => match &self.dis[*i] {
                        Di::Inert => matches!(f.name.as_str(), "IsActive" | "IsValid"),
                        Di::VectorField { field, .. } => field.is_some(),
                        _ => true,
                    },
                };
                if !bound {
                    let n = format!("{}.{}", f.owner, f.name);
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
        out
    }
}

fn renderer(object: &Value) -> RendererDesc {
    let p = &object["Properties"];
    let kind = match object["Type"].as_str().unwrap_or("") {
        "NiagaraSpriteRendererProperties" => RendererKind::Sprite,
        "NiagaraMeshRendererProperties" => RendererKind::Mesh,
        "NiagaraRibbonRendererProperties" => RendererKind::Ribbon,
        "NiagaraLightRendererProperties" => RendererKind::Light,
        _ => RendererKind::Other,
    };
    let defaults: &[(&'static str, &'static str)] = &[
        ("PositionBinding", "Position"),
        ("ColorBinding", "Color"),
        ("VelocityBinding", "Velocity"),
        ("SpriteRotationBinding", "SpriteRotation"),
        ("SpriteSizeBinding", "SpriteSize"),
        ("SpriteFacingBinding", "SpriteFacing"),
        ("SpriteAlignmentBinding", "SpriteAlignment"),
        ("SubImageIndexBinding", "SubImageIndex"),
        ("CameraOffsetBinding", "CameraOffset"),
        ("PivotOffsetBinding", "PivotOffset"),
        ("UVScaleBinding", "UVScale"),
        ("NormalizedAgeBinding", "NormalizedAge"),
        ("MeshOrientationBinding", "MeshOrientation"),
        ("ScaleBinding", "Scale"),
        ("MeshIndexBinding", "MeshIndex"),
        ("RibbonWidthBinding", "RibbonWidth"),
        ("RibbonTwistBinding", "RibbonTwist"),
        ("RibbonFacingBinding", "RibbonFacing"),
        ("RibbonIdBinding", "RibbonID"),
        ("RibbonLinkOrderBinding", "RibbonLinkOrder"),
        ("RibbonUVDistance", "RibbonUVDistance"),
        ("LightRadiusBinding", "LightRadius"),
        ("LightExponentBinding", "LightExponent"),
        (
            "LightVolumetricScatteringBinding",
            "LightVolumetricScattering",
        ),
        ("RendererVisibilityTagBinding", "VisibilityTag"),
        ("DynamicMaterialBinding", "DynamicMaterialParameter"),
    ];
    let mut bindings = HashMap::new();
    for (key, default) in defaults {
        let b = &p[*key];
        let name = b["DataSetName"]
            .as_str()
            .or_else(|| {
                b["ParamMapVariable"]["Name"]
                    .as_str()
                    .map(|s| s.strip_prefix("Particles.").unwrap_or(s))
            })
            .unwrap_or(default);
        if name != "None" && !name.is_empty() {
            bindings.insert(*key, name.to_owned());
        }
    }
    let material = p["Material"]["ObjectPath"]
        .as_str()
        .map(|_| reference_path(&p["Material"]))
        .or_else(|| {
            p["Meshes"]
                .as_array()
                .and_then(|m| m.first())
                .map(|_| String::new())
        })
        .filter(|s| !s.is_empty());
    let sub = &p["SubImageSize"];
    RendererDesc {
        kind,
        name: object["Name"].as_str().unwrap_or("").into(),
        material,
        enabled: p["bIsEnabled"].as_bool().unwrap_or(true),
        properties: p.clone(),
        bindings,
        sub_image: [
            sub["X"].as_f64().unwrap_or(1.).max(1.) as f32,
            sub["Y"].as_f64().unwrap_or(1.).max(1.) as f32,
        ],
        sort_hint: p["SortOrderHint"].as_i64().unwrap_or(0) as i32,
    }
}
/// `/Path/Package.Object` from an export reference.
pub fn reference_path(v: &Value) -> String {
    let path = v["ObjectPath"].as_str().unwrap_or("");
    let name = v["ObjectName"]
        .as_str()
        .and_then(|s| s.split('\'').nth(1))
        .and_then(|s| s.rsplit(':').next())
        .unwrap_or("");
    format!("{}.{}", path.rsplit_once('.').map_or(path, |p| p.0), name)
}

#[derive(Default)]
struct IdTable {
    free: Vec<u32>,
    next: u32,
    map: HashMap<u32, u32>,
}

struct EmitterState {
    data: Buffer,
    spawn_params: Vec<u8>,
    update_params: Vec<u8>,
    state: u32,
    age: f32,
    tick: u32,
    total_spawned: i32,
    spawn_infos: Vec<(i32, f32, f32, i32)>,
    random: RandomStream,
    ids: IdTable,
    /// Events generated this tick, by generator ID.
    events: HashMap<String, Buffer>,
    event_params: Vec<Vec<u8>>,
}

/// Per-tick context: delta time, owner transform and environment.
pub struct TickContext<'a> {
    pub dt: f32,
    /// Owner (attached component) transform in component/world space.
    pub owner: Transform,
    pub owner_velocity: Vec3,
    /// Seconds on the environment timeline for skeletal queries.
    pub time: f32,
    pub env: &'a dyn Environment,
    pub camera: Transform,
}

pub struct SystemInstance {
    pub asset: Arc<SystemAsset>,
    system: Buffer,
    emitters: Vec<EmitterState>,
    random: RandomStream,
    age: f32,
    tick: u32,
    seed: i32,
    requested: u32,
    pub complete: bool,
    pub errors: Vec<String>,
    prev_globals: [u8; GLOBAL_BYTES],
    prev_system: [u8; SYSTEM_BYTES],
    prev_owner: [u8; OWNER_BYTES],
    prev_emitter: Vec<[u8; EMITTER_BYTES]>,
}

fn put_f(buf: &mut [u8], at: usize, v: f32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i(buf: &mut [u8], at: usize, v: i32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_v(buf: &mut [u8], at: usize, v: &[f32]) {
    for (i, n) in v.iter().enumerate() {
        put_f(buf, at + 4 * i, *n);
    }
}
/// UE row-vector FMatrix44f: rows are scaled axes, row 3 is translation.
fn matrix(t: &Transform) -> [f32; 16] {
    let m = Mat3::from_quat(t.rotation);
    let x = m.x_axis * t.scale.x;
    let y = m.y_axis * t.scale.y;
    let z = m.z_axis * t.scale.z;
    [
        x.x,
        x.y,
        x.z,
        0.,
        y.x,
        y.y,
        y.z,
        0.,
        z.x,
        z.y,
        z.z,
        0.,
        t.translation.x,
        t.translation.y,
        t.translation.z,
        1.,
    ]
}
fn inverse(m: [f32; 16]) -> [f32; 16] {
    let g = glam::Mat4::from_cols_array(&m).inverse();
    g.to_cols_array()
}
fn transpose(m: [f32; 16]) -> [f32; 16] {
    std::array::from_fn(|i| m[(i % 4) * 4 + i / 4])
}

impl SystemInstance {
    pub fn new(asset: Arc<SystemAsset>, seed: i32) -> Self {
        let emitters = asset
            .emitters
            .iter()
            .map(|e| EmitterState {
                data: Buffer::new(&e.layout),
                spawn_params: e
                    .spawn
                    .as_ref()
                    .map(|s| s.params.clone())
                    .unwrap_or_default(),
                update_params: e
                    .update
                    .as_ref()
                    .map(|s| s.params.clone())
                    .unwrap_or_default(),
                state: if e.error.is_some() { DISABLED } else { ACTIVE },
                age: 0.,
                tick: 0,
                total_spawned: 0,
                spawn_infos: Vec::new(),
                events: HashMap::new(),
                event_params: e.handlers.iter().map(|h| h.script.params.clone()).collect(),
                random: RandomStream::new((seed as u64) << 20 ^ e.seed as u64 ^ 0x51ed),
                ids: IdTable::default(),
            })
            .collect::<Vec<_>>();
        let count = emitters.len();
        Self {
            system: Buffer::new(&asset.system_layout),
            asset,
            emitters,
            random: RandomStream::new(seed as u64 ^ 0x5eed),
            age: 0.,
            tick: 0,
            seed,
            requested: ACTIVE,
            complete: false,
            errors: Vec::new(),
            prev_globals: [0; GLOBAL_BYTES],
            prev_system: [0; SYSTEM_BYTES],
            prev_owner: [0; OWNER_BYTES],
            prev_emitter: vec![[0; EMITTER_BYTES]; count],
        }
    }
    /// Owner requested deactivation: emitters stop spawning and complete
    /// once their particles die (`bDestroyAtEnd == false` notify behavior).
    pub fn deactivate(&mut self) {
        self.requested = INACTIVE;
    }
    pub fn particle_count(&self) -> usize {
        self.emitters.iter().map(|e| e.data.len).sum()
    }
    pub fn emitter_data(&self, index: usize) -> Option<&Buffer> {
        self.emitters.get(index).map(|e| &e.data)
    }
    pub fn emitter_state(&self, index: usize) -> u32 {
        self.emitters.get(index).map_or(DISABLED, |e| e.state)
    }

    fn globals(&self, dt: f32) -> [u8; GLOBAL_BYTES] {
        let mut g = [0u8; GLOBAL_BYTES];
        put_f(&mut g, 0, dt);
        put_f(&mut g, 4, dt);
        put_f(&mut g, 8, if dt > 0. { 1. / dt } else { 0. });
        put_f(&mut g, 12, self.age);
        put_f(&mut g, 16, self.age);
        put_i(&mut g, 20, 3);
        g
    }
    fn system_params(&self) -> [u8; SYSTEM_BYTES] {
        let mut s = [0u8; SYSTEM_BYTES];
        put_f(&mut s, 4, 300.);
        put_f(&mut s, 8, 0.05);
        put_f(&mut s, 12, self.age);
        put_i(&mut s, 16, self.requested as i32);
        put_i(&mut s, 20, self.tick as i32);
        put_i(&mut s, 24, self.emitters.len() as i32);
        put_i(
            &mut s,
            28,
            self.emitters.iter().filter(|e| e.state <= INACTIVE).count() as i32,
        );
        put_i(&mut s, 36, self.seed);
        put_i(&mut s, 44, 1);
        put_f(&mut s, 48, 1.);
        put_i(&mut s, 52, self.particle_count() as i32);
        s
    }
    fn owner_params(ctx: &TickContext) -> [u8; OWNER_BYTES] {
        let mut o = [0u8; OWNER_BYTES];
        let l2w = matrix(&ctx.owner);
        let w2l = inverse(l2w);
        let unscaled = Transform {
            scale: Vec3::ONE,
            ..ctx.owner
        };
        let n2w = matrix(&unscaled);
        put_v(&mut o, 0, &l2w);
        put_v(&mut o, 64, &w2l);
        put_v(&mut o, 128, &transpose(l2w));
        put_v(&mut o, 192, &transpose(w2l));
        put_v(&mut o, 256, &n2w);
        put_v(&mut o, 320, &inverse(n2w));
        let q = ctx.owner.rotation;
        put_v(&mut o, 384, &[q.x, q.y, q.z, q.w]);
        put_v(&mut o, 400, &ctx.owner.translation.to_array());
        put_v(&mut o, 416, &ctx.owner_velocity.to_array());
        put_v(&mut o, 432, &(q * Vec3::X).to_array());
        put_v(&mut o, 448, &(q * Vec3::Y).to_array());
        put_v(&mut o, 464, &(q * Vec3::Z).to_array());
        put_v(&mut o, 480, &ctx.owner.scale.to_array());
        o
    }
    fn emitter_params(&self, index: usize) -> [u8; EMITTER_BYTES] {
        let e = &self.emitters[index];
        let mut b = [0u8; EMITTER_BYTES];
        put_i(&mut b, 0, e.data.len as i32);
        put_i(&mut b, 4, e.total_spawned);
        put_f(&mut b, 8, 1.);
        put_f(&mut b, 12, e.age);
        put_i(&mut b, 16, self.asset.emitters[index].seed);
        put_i(&mut b, 20, self.seed ^ index as i32);
        b
    }
    /// `FNiagaraConstantBufferToDataSetBinding::CopyToDataSets`.
    fn instance_params(
        &self,
        layout: &Layout,
        prefix: &str,
        global: &[u8],
        system: &[u8],
        owner: &[u8],
    ) -> Buffer {
        let mut buf = Buffer::new(layout);
        buf.reserve_len(1);
        buf.len = 1;
        let b = &self.asset.bindings;
        let mut apply = |binding: &Value, source: &[u8]| {
            for (key, kind) in [("FloatOffsets", 0), ("Int32Offsets", 1), ("HalfOffsets", 2)] {
                for o in binding[key].as_array().into_iter().flatten() {
                    let (Some(src), Some(dst)) = (
                        o["ParameterOffset"].as_u64(),
                        o["DataSetComponentOffset"].as_u64(),
                    ) else {
                        continue;
                    };
                    let Some(word) = usize::try_from(src)
                        .ok()
                        .and_then(|at| source.get(at..at.checked_add(4)?))
                    else {
                        continue;
                    };
                    let word = u32::from_le_bytes(word.try_into().unwrap());
                    let column = match kind {
                        0 => buf.f.get_mut(dst as usize),
                        1 => buf.i.get_mut(dst as usize),
                        _ => buf.h.get_mut(dst as usize),
                    };
                    if let Some(c) = column {
                        c[0] = word;
                    }
                }
            }
        };
        apply(&b[format!("{prefix}InstanceGlobalBinding")], global);
        apply(&b[format!("{prefix}InstanceSystemBinding")], system);
        apply(&b[format!("{prefix}InstanceOwnerBinding")], owner);
        for (i, eb) in b[format!("{prefix}InstanceEmitterBindings")]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if i < self.emitters.len() {
                apply(eb, &self.emitter_params(i));
            }
        }
        buf
    }

    /// Advance the system by `ctx.dt` seconds.
    pub fn tick(&mut self, ctx: &TickContext) {
        if self.complete {
            return;
        }
        let asset = self.asset.clone();
        let dt = ctx.dt.max(1e-4);
        let owner = Self::owner_params(ctx);
        let system_bytes = self.system_params();
        let first = self.tick == 0;
        if first {
            self.prev_owner = owner;
            self.prev_system = system_bytes;
        }
        // System scripts.
        let result = (|| -> Result<(), String> {
            if first {
                let globals = self.globals(dt);
                let params = self.instance_params(
                    &asset.spawn_params,
                    "Spawn",
                    &globals,
                    &system_bytes,
                    &owner,
                );
                let mut out = Buffer::new(&asset.system_layout);
                run_system(
                    &asset,
                    &asset.spawn,
                    &globals,
                    &mut self.random,
                    &Buffer::default(),
                    &mut out,
                    &params,
                    1,
                    ctx,
                    &self.emitters,
                    &asset.dis,
                )?;
                self.system = out;
                // UpdateOnSpawn: tiny delta, same as GNiagaraSystemSimulationUpdateOnSpawn=1.
                let globals = self.globals(0.0001);
                let params = self.instance_params(
                    &asset.update_params,
                    "Update",
                    &globals,
                    &system_bytes,
                    &owner,
                );
                let mut out = Buffer::new(&asset.system_layout);
                run_system(
                    &asset,
                    &asset.update,
                    &globals,
                    &mut self.random,
                    &self.system,
                    &mut out,
                    &params,
                    1,
                    ctx,
                    &self.emitters,
                    &asset.dis,
                )?;
                self.system = out;
            } else {
                let globals = self.globals(dt);
                let params = self.instance_params(
                    &asset.update_params,
                    "Update",
                    &globals,
                    &system_bytes,
                    &owner,
                );
                let mut out = Buffer::new(&asset.system_layout);
                run_system(
                    &asset,
                    &asset.update,
                    &globals,
                    &mut self.random,
                    &self.system,
                    &mut out,
                    &params,
                    1,
                    ctx,
                    &self.emitters,
                    &asset.dis,
                )?;
                self.system = out;
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.errors.push(format!("system script: {e}"));
            self.complete = true;
            return;
        }
        let layout = &asset.system_layout;
        let system_state = layout
            .get("System.ExecutionState")
            .map_or(ACTIVE, |v| self.system.int(v, 0, 0) as u32);
        // TransferSystemSimResults.
        for (i, e) in asset.emitters.iter().enumerate() {
            let st = &mut self.emitters[i];
            if st.state == DISABLED {
                continue;
            }
            if let Some(v) = layout.get(&format!("{}.ExecutionState", e.name)) {
                st.state = self.system.int(v, 0, 0) as u32;
            }
            st.spawn_infos.clear();
            for name in &e.spawn_infos {
                if let Some(v) = layout.get(name) {
                    st.spawn_infos.push((
                        self.system.int(v, 0, 0),
                        self.system.float(v, 0, 0),
                        self.system.float(v, 1, 0),
                        self.system.int(v, 1, 0),
                    ));
                }
            }
            let handler_scripts = e
                .handlers
                .iter()
                .map(|h| Some(&h.script))
                .collect::<Vec<_>>();
            let mut stores: Vec<(Option<&ScriptAsset>, &mut Vec<u8>)> = vec![
                (e.spawn.as_ref(), &mut st.spawn_params),
                (e.update.as_ref(), &mut st.update_params),
            ];
            stores.extend(handler_scripts.into_iter().zip(st.event_params.iter_mut()));
            for (script, params) in stores {
                let Some(script) = script else { continue };
                for (name, &off) in &script.names {
                    if let Some(v) = layout.get(name) {
                        let words = (0..v.comps.f)
                            .map(|c| self.system.f[(v.f + c) as usize][0])
                            .chain((0..v.comps.i).map(|c| self.system.i[(v.i + c) as usize][0]));
                        for (k, w) in words.enumerate() {
                            if let Some(slot) = params.get_mut(off + 4 * k..off + 4 * k + 4) {
                                slot.copy_from_slice(&w.to_le_bytes());
                            }
                        }
                    }
                }
            }
        }
        let globals = self.globals(dt);
        let mut emitter_params = Vec::with_capacity(asset.emitters.len());
        for i in 0..asset.emitters.len() {
            self.emitters[i].age += dt;
            emitter_params.push(self.emitter_params(i));
        }
        if first {
            self.prev_globals = globals;
            self.prev_emitter = emitter_params.clone();
        }
        // Particle ParticleRead DIs read the previous tick's buffers.
        let snapshot = self
            .emitters
            .iter()
            .map(|e| e.data.clone())
            .collect::<Vec<_>>();
        for (i, e) in asset.emitters.iter().enumerate() {
            let tables = Tables {
                global: &globals,
                system: &system_bytes,
                owner: &owner,
                emitter: &emitter_params[i],
                prev_global: &self.prev_globals,
                prev_system: &self.prev_system,
                prev_owner: &self.prev_owner,
                prev_emitter: &self.prev_emitter[i],
            };
            let st = &mut self.emitters[i];
            if st.state == DISABLED {
                continue;
            }
            if let Err(error) = tick_emitter(&asset, e, st, &tables, ctx, dt, &snapshot) {
                self.errors.push(format!("{}: {error}", e.name));
                st.state = DISABLED;
                st.data.truncate(0);
            }
        }
        // Event handlers read the events their sources generated this tick.
        let events = self
            .emitters
            .iter_mut()
            .map(|e| std::mem::take(&mut e.events))
            .collect::<Vec<_>>();
        if events.iter().any(|e| !e.is_empty()) {
            let snapshot = self
                .emitters
                .iter()
                .map(|e| e.data.clone())
                .collect::<Vec<_>>();
            for (i, e) in asset.emitters.iter().enumerate() {
                if e.handlers.is_empty() || self.emitters[i].state == DISABLED {
                    continue;
                }
                let tables = Tables {
                    global: &globals,
                    system: &system_bytes,
                    owner: &owner,
                    emitter: &emitter_params[i],
                    prev_global: &self.prev_globals,
                    prev_system: &self.prev_system,
                    prev_owner: &self.prev_owner,
                    prev_emitter: &self.prev_emitter[i],
                };
                if let Err(error) = run_handlers(
                    &asset,
                    i,
                    &mut self.emitters[i],
                    &tables,
                    ctx,
                    dt,
                    &snapshot,
                    &events,
                ) {
                    self.errors.push(format!("{} events: {error}", e.name));
                }
            }
        }
        self.prev_globals = globals;
        self.prev_system = system_bytes;
        self.prev_owner = owner;
        self.prev_emitter = emitter_params;
        self.age += dt;
        self.tick += 1;
        let all_done = self.emitters.iter().all(|e| {
            e.state == DISABLED || e.state == COMPLETE || (e.state != ACTIVE && e.data.len == 0)
        });
        if system_state == COMPLETE || system_state == DISABLED || (all_done && self.tick > 1) {
            self.complete =
                self.emitters.iter().all(|e| e.data.len == 0) || system_state == DISABLED;
        }
    }
}

struct Tables<'a> {
    global: &'a [u8],
    system: &'a [u8],
    owner: &'a [u8],
    emitter: &'a [u8],
    prev_global: &'a [u8],
    prev_system: &'a [u8],
    prev_owner: &'a [u8],
    prev_emitter: &'a [u8],
}

fn tick_emitter(
    asset: &SystemAsset,
    e: &EmitterAsset,
    st: &mut EmitterState,
    t: &Tables,
    ctx: &TickContext,
    dt: f32,
    others: &[Buffer],
) -> Result<(), String> {
    if let Some(stateless) = &e.stateless {
        st.tick += 1;
        if st.state == INACTIVE_CLEAR {
            st.data.truncate(0);
            st.state = INACTIVE;
            return Ok(());
        }
        let mut next = st.total_spawned;
        stateless.tick(
            &e.layout,
            &mut st.data,
            &mut st.random,
            ctx.owner.translation,
            e.local_space,
            dt,
            st.age,
            st.state == ACTIVE,
            MAX_PARTICLES_PER_EMITTER,
            &mut next,
        );
        st.total_spawned = next;
        return Ok(());
    }
    if let Some(approx) = &e.approx {
        st.tick += 1;
        if st.state == INACTIVE_CLEAR {
            st.data.truncate(0);
            st.state = INACTIVE;
            return Ok(());
        }
        let count = if st.state == ACTIVE {
            st.spawn_infos.iter().map(|s| s.0.max(0) as usize).sum()
        } else {
            0
        };
        let mut next = st.total_spawned;
        approx.tick(
            &e.layout,
            &mut st.data,
            &mut st.random,
            ctx.owner.translation,
            e.local_space,
            dt,
            count,
            MAX_PARTICLES_PER_EMITTER,
            &mut next,
        );
        st.total_spawned = next;
        return Ok(());
    }
    let (Some(spawn), Some(update)) = (&e.spawn, &e.update) else {
        return Ok(());
    };
    if st.tick == 0 {
        copy_prev(&mut st.spawn_params, spawn);
        copy_prev(&mut st.update_params, update);
    }
    st.tick += 1;
    if st.state == INACTIVE_CLEAR {
        st.data.truncate(0);
        st.state = INACTIVE;
        return Ok(());
    }
    if st.data.len == 0 && st.state != ACTIVE {
        return Ok(());
    }
    // Update existing particles; generators are datasets 1..n.
    let orig = st.data.len;
    if orig > 0 {
        set_int(
            &mut st.update_params,
            update,
            "Engine.ExecutionCount",
            orig as i32,
        );
        let constants = constants(update, &st.update_params, t);
        let mut out = Buffer::new(&e.layout);
        out.reserve_len(orig);
        let empty = Buffer::default();
        let mut generated = e
            .update_events
            .iter()
            .map(|(_, l)| Buffer::new(l))
            .collect::<Vec<_>>();
        let mut host = Exec::new(
            &constants,
            &mut st.random,
            ctx,
            asset,
            update,
            others,
            &asset.emitters,
        );
        host.sets.push(Set {
            input: &st.data,
            output: &mut out,
            in_off: 0,
            out_off: 0,
            acquired: 0,
            acquire_used: false,
            ids: Some(&mut st.ids),
        });
        for g in generated.iter_mut() {
            host.sets.push(Set {
                input: &empty,
                output: g,
                in_off: 0,
                out_off: 0,
                acquired: 0,
                acquire_used: false,
                ids: None,
            });
        }
        host.run(orig)?;
        let n = host.sets[0].count(orig);
        let counts = host.sets[1..]
            .iter()
            .map(|s| if s.acquire_used { s.acquired } else { 0 })
            .collect::<Vec<_>>();
        drop(host);
        out.truncate(n);
        st.data = out;
        collect_events(&mut st.events, &e.update_events, generated, &counts);
    }
    // Spawn.
    if st.state == ACTIVE {
        let infos = st.spawn_infos.clone();
        for (count, start, interval, group) in infos {
            spawn_batch(
                asset,
                e,
                st,
                t,
                ctx,
                others,
                count.max(0) as usize,
                interval,
                start,
                group,
            )?;
        }
    }
    copy_prev(&mut st.spawn_params, spawn);
    copy_prev(&mut st.update_params, update);
    let _ = dt;
    Ok(())
}

/// Run the spawn script for `count` new particles; returns (first index, spawned).
#[allow(clippy::too_many_arguments)]
fn spawn_batch(
    asset: &SystemAsset,
    e: &EmitterAsset,
    st: &mut EmitterState,
    t: &Tables,
    ctx: &TickContext,
    others: &[Buffer],
    count: usize,
    interval: f32,
    start: f32,
    group: i32,
) -> Result<(usize, usize), String> {
    let Some(spawn) = &e.spawn else {
        return Ok((st.data.len, 0));
    };
    let count = count.min(MAX_PARTICLES_PER_EMITTER.saturating_sub(st.data.len));
    let base = st.data.len;
    if count == 0 {
        return Ok((base, 0));
    }
    set_float(
        &mut st.spawn_params,
        spawn,
        &format!("{}.SpawnInterval", e.name),
        interval,
    );
    set_float(
        &mut st.spawn_params,
        spawn,
        &format!("{}.InterpSpawnStartDt", e.name),
        start,
    );
    set_int(
        &mut st.spawn_params,
        spawn,
        &format!("{}.SpawnGroup", e.name),
        group,
    );
    set_int(
        &mut st.spawn_params,
        spawn,
        "Engine.ExecutionCount",
        count as i32,
    );
    let mut emitter = t.emitter.to_vec();
    put_i(&mut emitter, 4, st.total_spawned);
    put_i(&mut emitter, 0, st.data.len as i32);
    st.total_spawned += count as i32;
    let tables = Tables {
        emitter: &emitter,
        ..*t
    };
    let constants = constants(spawn, &st.spawn_params, &tables);
    let mut data = std::mem::take(&mut st.data);
    data.reserve_len(base + count);
    let input = data.clone();
    let empty = Buffer::default();
    let mut generated = e
        .spawn_events
        .iter()
        .map(|(_, l)| Buffer::new(l))
        .collect::<Vec<_>>();
    let mut host = Exec::new(
        &constants,
        &mut st.random,
        ctx,
        asset,
        spawn,
        others,
        &asset.emitters,
    );
    host.sets.push(Set {
        input: &input,
        output: &mut data,
        in_off: base,
        out_off: base,
        acquired: 0,
        acquire_used: false,
        ids: Some(&mut st.ids),
    });
    for g in generated.iter_mut() {
        host.sets.push(Set {
            input: &empty,
            output: g,
            in_off: 0,
            out_off: 0,
            acquired: 0,
            acquire_used: false,
            ids: None,
        });
    }
    let r = host.run(count);
    let n = host.sets[0].count(count);
    let counts = host.sets[1..]
        .iter()
        .map(|s| if s.acquire_used { s.acquired } else { 0 })
        .collect::<Vec<_>>();
    drop(host);
    r?;
    data.truncate(base + n);
    st.data = data;
    collect_events(&mut st.events, &e.spawn_events, generated, &counts);
    Ok((base, n))
}

fn collect_events(
    events: &mut HashMap<String, Buffer>,
    generators: &[(String, Layout)],
    generated: Vec<Buffer>,
    counts: &[usize],
) {
    for (((id, _), mut buffer), &n) in generators.iter().zip(generated).zip(counts) {
        if n == 0 {
            continue;
        }
        buffer.truncate(n);
        match events.get_mut(id) {
            Some(existing) => existing.append(&buffer),
            None => {
                events.insert(id.clone(), buffer);
            }
        }
    }
}

/// `FNiagaraEmitterInstanceImpl` event handling: spawned-particle handlers spawn
/// N particles per event and run the event script on them with the event row
/// bound as dataset 1; every-particle handlers run it over all particles.
#[allow(clippy::too_many_arguments)]
fn run_handlers(
    asset: &SystemAsset,
    index: usize,
    st: &mut EmitterState,
    t: &Tables,
    ctx: &TickContext,
    dt: f32,
    others: &[Buffer],
    events: &[HashMap<String, Buffer>],
) -> Result<(), String> {
    let e = &asset.emitters[index];
    for (h_index, h) in e.handlers.iter().enumerate() {
        let source = if h.source.is_empty() {
            Some(index)
        } else {
            asset.emitters.iter().position(|x| x.handle_id == h.source)
        };
        let Some(source) = source else { continue };
        let Some(event_data) = events[source].get(&h.event) else {
            continue;
        };
        let limit = if h.max_events > 0 {
            h.max_events
        } else {
            usize::MAX
        };
        for event in 0..event_data.len.min(limit) {
            let (base, count) = if h.every_particle {
                (0, st.data.len)
            } else {
                if st.state != ACTIVE {
                    break;
                }
                let n = if h.random && h.spawn_number > h.min_spawn {
                    h.min_spawn + st.random.next_word() % (h.spawn_number - h.min_spawn + 1)
                } else {
                    h.spawn_number
                };
                spawn_batch(asset, e, st, t, ctx, others, n as usize, 0., dt * 0.5, 0)?
            };
            if count == 0 {
                continue;
            }
            set_int(
                &mut st.event_params[h_index],
                &h.script,
                "Engine.ExecutionCount",
                count as i32,
            );
            let constants = constants(&h.script, &st.event_params[h_index], t);
            let input = st.data.clone();
            let mut scratch = Buffer::default();
            let mut host = Exec::new(
                &constants,
                &mut st.random,
                ctx,
                asset,
                &h.script,
                others,
                &asset.emitters,
            );
            host.sets.push(Set {
                input: &input,
                output: &mut st.data,
                in_off: base,
                out_off: base,
                acquired: 0,
                acquire_used: false,
                ids: Some(&mut st.ids),
            });
            host.sets.push(Set {
                input: event_data,
                output: &mut scratch,
                in_off: event,
                out_off: 0,
                acquired: 0,
                acquire_used: false,
                ids: None,
            });
            let r = host.run(count);
            let kept = host.sets[0].count(count);
            drop(host);
            r?;
            // Particles after the handled range keep their rows.
            let tail = input.len.saturating_sub(base + count);
            let mut merged = st.data.clone();
            merged.reserve_len(base + kept + tail);
            for k in 0..tail {
                merged.copy_instance(&input, base + count + k, base + kept + k);
            }
            merged.truncate(base + kept + tail);
            st.data = merged;
        }
    }
    Ok(())
}

fn copy_prev(params: &mut [u8], s: &ScriptAsset) {
    if s.interpolated && params.len() >= 2 * s.external_size {
        let (a, b) = params.split_at_mut(s.external_size);
        b[..s.external_size].copy_from_slice(a);
    }
}
fn set_int(params: &mut [u8], s: &ScriptAsset, name: &str, v: i32) {
    if let Some(off) = s.offset(name) {
        if off + 4 <= params.len() {
            put_i(params, off, v);
        }
    }
}
fn set_float(params: &mut [u8], s: &ScriptAsset, name: &str, v: f32) {
    if let Some(off) = s.offset(name) {
        if off + 4 <= params.len() {
            put_f(params, off, v);
        }
    }
}
fn constants(s: &ScriptAsset, params: &[u8], t: &Tables) -> Vec<u8> {
    let mut c = Vec::with_capacity(
        2 * (GLOBAL_BYTES + SYSTEM_BYTES + OWNER_BYTES + EMITTER_BYTES + s.external_size)
            + s.literals.len(),
    );
    c.extend_from_slice(t.global);
    c.extend_from_slice(t.system);
    c.extend_from_slice(t.owner);
    c.extend_from_slice(t.emitter);
    c.extend_from_slice(&params[..s.external_size.min(params.len())]);
    if s.interpolated {
        c.extend_from_slice(t.prev_global);
        c.extend_from_slice(t.prev_system);
        c.extend_from_slice(t.prev_owner);
        c.extend_from_slice(t.prev_emitter);
        c.extend_from_slice(
            params
                .get(s.external_size..2 * s.external_size)
                .unwrap_or(&[]),
        );
    }
    c.extend_from_slice(&s.literals);
    c
}

#[allow(clippy::too_many_arguments)]
fn run_system(
    asset: &SystemAsset,
    s: &ScriptAsset,
    globals: &[u8],
    random: &mut RandomStream,
    input: &Buffer,
    out: &mut Buffer,
    params: &Buffer,
    count: usize,
    ctx: &TickContext,
    emitters: &[EmitterState],
    _dis: &[Di],
) -> Result<(), String> {
    let mut c = Vec::with_capacity(GLOBAL_BYTES + s.external_size + s.literals.len());
    c.extend_from_slice(globals);
    c.extend_from_slice(&s.params[..s.external_size.min(s.params.len())]);
    c.extend_from_slice(&s.literals);
    out.reserve_len(count);
    let others = emitters.iter().map(|e| e.data.clone()).collect::<Vec<_>>();
    let mut host = Exec::new(&c, random, ctx, asset, s, &others, &asset.emitters);
    let mut scratch = params.clone();
    host.sets.push(Set {
        input,
        output: out,
        in_off: 0,
        out_off: 0,
        acquired: 0,
        acquire_used: false,
        ids: None,
    });
    host.sets.push(Set {
        input: params,
        output: &mut scratch,
        in_off: 0,
        out_off: 0,
        acquired: 0,
        acquire_used: false,
        ids: None,
    });
    host.run(count)?;
    let n = host.sets[0].count(count);
    drop(host);
    out.truncate(n.max(count));
    Ok(())
}

struct Set<'a> {
    input: &'a Buffer,
    output: &'a mut Buffer,
    in_off: usize,
    out_off: usize,
    acquired: usize,
    acquire_used: bool,
    ids: Option<&'a mut IdTable>,
}
impl Set<'_> {
    fn count(&self, executed: usize) -> usize {
        if self.acquire_used {
            self.acquired
        } else {
            executed
        }
    }
}

struct Exec<'a> {
    constants: &'a [u8],
    random: &'a mut RandomStream,
    ctx: &'a TickContext<'a>,
    asset: &'a SystemAsset,
    script: &'a ScriptAsset,
    others: &'a [Buffer],
    emitters: &'a [EmitterAsset],
    sets: Vec<Set<'a>>,
    lane: u32,
}
impl<'a> Exec<'a> {
    fn new(
        constants: &'a [u8],
        random: &'a mut RandomStream,
        ctx: &'a TickContext<'a>,
        asset: &'a SystemAsset,
        script: &'a ScriptAsset,
        others: &'a [Buffer],
        emitters: &'a [EmitterAsset],
    ) -> Self {
        Self {
            constants,
            random,
            ctx,
            asset,
            script,
            others,
            emitters,
            sets: Vec::new(),
            lane: 0,
        }
    }
    fn run(&mut self, count: usize) -> Result<(), String> {
        for i in 0..count {
            self.lane = i as u32;
            let script = self.script;
            script.kernel.execute(i as u32, self)?;
        }
        Ok(())
    }
}

fn f(w: Option<u32>) -> f32 {
    w.map(f32::from_bits).unwrap_or(0.)
}
fn i(w: Option<u32>) -> i32 {
    w.map(|n| n as i32).unwrap_or(0)
}
fn words_f(v: &[f32]) -> Vec<u32> {
    v.iter().map(|n| n.to_bits()).collect()
}
/// UE `FQuat(const FMatrix&)` for a row-major rotation matrix (rows = axes).
fn matrix_to_quat(m: &[f32; 16]) -> Quat {
    let r = |row: usize, col: usize| m[row * 4 + col];
    let tr = r(0, 0) + r(1, 1) + r(2, 2);
    let (x, y, z, w);
    if tr > 0. {
        let inv = (tr + 1.).sqrt().recip();
        w = 0.5 / inv;
        let s = 0.5 * inv;
        x = (r(1, 2) - r(2, 1)) * s;
        y = (r(2, 0) - r(0, 2)) * s;
        z = (r(0, 1) - r(1, 0)) * s;
    } else {
        let mut i = 0;
        if r(1, 1) > r(0, 0) {
            i = 1;
        }
        if r(2, 2) > r(i, i) {
            i = 2;
        }
        let next = [1, 2, 0];
        let j = next[i];
        let k = next[j];
        let s = r(i, i) - r(j, j) - r(k, k) + 1.;
        let inv = s.sqrt().recip();
        let mut q = [0f32; 4];
        q[i] = 0.5 / inv;
        let s = 0.5 * inv;
        q[3] = (r(j, k) - r(k, j)) * s;
        q[j] = (r(i, j) + r(j, i)) * s;
        q[k] = (r(i, k) + r(k, i)) * s;
        (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    }
    Quat::from_xyzw(x, y, z, w)
}

impl Exec<'_> {
    fn bone_transform(
        &self,
        sockets: &[String],
        index: i32,
        world: bool,
        time_offset: f32,
    ) -> (Transform, Vec3) {
        let env = self.ctx.env;
        let at = |t: f32| -> Option<Transform> {
            let n = env.bones().len() as i32;
            if index < 0 {
                return None;
            }
            if index < n {
                return env.bone(index as usize, t);
            }
            let socket = sockets.get((index - n) as usize)?;
            // Socket readers also list plain bone names (`hand_l`, `head`).
            let (bone, rel) = match env.socket(socket) {
                Some(found) => found,
                None => (
                    env.bones()
                        .iter()
                        .position(|b| b.eq_ignore_ascii_case(socket))?,
                    Transform::default(),
                ),
            };
            Some(env.bone(bone, t)?.compose(&rel))
        };
        let now = self.ctx.time + time_offset;
        let dt = self.ctx.dt.max(1e-3);
        let Some(t) = at(now) else {
            return (
                if world {
                    self.ctx.owner
                } else {
                    Transform::default()
                },
                Vec3::ZERO,
            );
        };
        let prev = at(now - dt).unwrap_or(t);
        // The owner is the actor root, so component and world space coincide
        // in this host.
        let velocity = (t.translation - prev.translation) / dt;
        (t, velocity)
    }
    fn resolve_bone(&self, list: &[String], index: i32) -> i32 {
        let Some(name) = usize::try_from(index).ok().and_then(|i| list.get(i)) else {
            return -1;
        };
        self.ctx
            .env
            .bones()
            .iter()
            .position(|b| b.eq_ignore_ascii_case(name))
            .map_or(-1, |n| n as i32)
    }
    fn rand_index(&mut self, len: usize) -> i32 {
        if len == 0 {
            return -1;
        }
        (self.random.next_word() as usize % len) as i32
    }
    fn call_di(
        &mut self,
        di: usize,
        attribute: Option<String>,
        func: &External,
        inputs: &[Option<u32>],
    ) -> Result<Vec<u32>, String> {
        let asset = self.asset;
        let di = &asset.dis[di];
        let last = |k: usize| inputs.get(inputs.len().wrapping_sub(k)).copied().flatten();
        let name = func.name.as_str();
        let out: Vec<u32> = match di {
            Di::Curve(curve) => {
                if !name.starts_with("Sample") {
                    return Err("unsupported curve function".into());
                }
                curve.sample(f(last(1)))?
            }
            Di::FloatArray { comps, data } => {
                let len = data.len() / comps.max(&1);
                match name {
                    "Length" | "Num" | "GetNum" => vec![len as u32],
                    "IsValidIndex" => vec![if (0..len as i32).contains(&i(last(1))) {
                        u32::MAX
                    } else {
                        0
                    }],
                    "Get" | "GetValue" => {
                        let index = if len == 0 {
                            0
                        } else {
                            i(last(1)).clamp(0, len as i32 - 1) as usize
                        };
                        words_f(
                            data.get(index * comps..index * comps + comps)
                                .unwrap_or(&[0., 0., 0., 0.][..*comps]),
                        )
                    }
                    "RemoveLastElem" => {
                        let index = len.saturating_sub(1);
                        let mut v = words_f(
                            data.get(index * comps..index * comps + comps)
                                .unwrap_or(&[0., 0., 0., 0.][..*comps]),
                        );
                        v.push(if len > 0 { u32::MAX } else { 0 });
                        v
                    }
                    _ => Vec::new(),
                }
            }
            Di::IntArray(data) => match name {
                "Length" | "Num" | "GetNum" => vec![data.len() as u32],
                "IsValidIndex" => vec![if (0..data.len() as i32).contains(&i(last(1))) {
                    u32::MAX
                } else {
                    0
                }],
                "Get" | "GetValue" => vec![if data.is_empty() {
                    0
                } else {
                    data[i(last(1)).clamp(0, data.len() as i32 - 1) as usize] as u32
                }],
                _ => Vec::new(),
            },
            Di::Skeletal { bones, sockets } => {
                let n = self.ctx.env.bones().len() as i32;
                match name {
                    "GetFilteredBoneCount" => vec![bones.len() as u32],
                    "GetFilteredSocketCount" => vec![sockets.len() as u32],
                    "GetFilteredSocketOrBoneCount" => vec![(bones.len() + sockets.len()) as u32],
                    "GetBoneCount" | "GetUnfilteredBoneCount" => vec![n as u32],
                    "GetFilteredBone" | "GetFilteredBoneAt" => {
                        vec![self.resolve_bone(bones, i(last(1))) as u32]
                    }
                    "GetFilteredSocket" | "GetFilteredSocketBoneAt" => {
                        let s = i(last(1));
                        vec![if s >= 0 && (s as usize) < sockets.len() {
                            (n + s) as u32
                        } else {
                            u32::MAX
                        }]
                    }
                    "GetFilteredSocketOrBone" | "GetFilteredSocketOrBoneAt" => {
                        let k = i(last(1));
                        vec![if (k as usize) < bones.len() {
                            self.resolve_bone(bones, k) as u32
                        } else {
                            (n + k - bones.len() as i32) as u32
                        }]
                    }
                    "RandomFilteredBone" => {
                        let k = self.rand_index(bones.len());
                        vec![self.resolve_bone(bones, k) as u32]
                    }
                    "RandomFilteredSocket" => {
                        let k = self.rand_index(sockets.len());
                        vec![if k < 0 { u32::MAX } else { (n + k) as u32 }]
                    }
                    "RandomBone" | "RandomUnfilteredBone" => {
                        vec![self.rand_index(n as usize) as u32]
                    }
                    "IsValidBone" => vec![if (0..n + sockets.len() as i32).contains(&i(last(1))) {
                        u32::MAX
                    } else {
                        0
                    }],
                    "GetParentBone" => vec![u32::MAX],
                    _ if name.starts_with("GetSkinnedBoneData")
                        || name.starts_with("GetFilteredSocketTransform")
                        || name == "GetSocketTransform"
                        || name.starts_with("GetSocketTransform") =>
                    {
                        let interpolated = name.ends_with("Interpolated");
                        let (bone, interp) = if interpolated {
                            (i(last(2)), f(last(1)))
                        } else {
                            (i(last(1)), 1.)
                        };
                        let index = if name.starts_with("GetFilteredSocketTransform")
                            || name.starts_with("GetSocketTransform")
                        {
                            if bone < 0 {
                                -1
                            } else {
                                n + bone
                            }
                        } else {
                            bone
                        };
                        let offset = (interp - 1.).clamp(-1., 0.) * self.ctx.dt;
                        let (t, v) =
                            self.bone_transform(sockets, index, name.contains("WS"), offset);
                        let q = t.rotation;
                        let mut out = vec![
                            t.translation.x,
                            t.translation.y,
                            t.translation.z,
                            q.x,
                            q.y,
                            q.z,
                            q.w,
                            t.scale.x,
                            t.scale.y,
                            t.scale.z,
                            v.x,
                            v.y,
                            v.z,
                        ];
                        out.truncate(func.outputs);
                        words_f(&out)
                    }
                    _ => Vec::new(),
                }
            }
            Di::SpriteInfo { sub_image } => match name {
                "GetSubUVDetails" => vec![u32::MAX, sub_image[0].to_bits(), sub_image[1].to_bits()],
                "IsValid" => vec![u32::MAX],
                _ => Vec::new(),
            },
            Di::MeshInfo { meshes } => match name {
                "GetNumMeshes" => vec![*meshes as u32],
                _ => Vec::new(),
            },
            Di::ParticleRead { emitter } => {
                let index = self.emitters.iter().position(|e| &e.name == emitter);
                match index {
                    Some(e) if name == "Get Num Particles" => vec![self.others[e].len as u32],
                    Some(e) => {
                        let layout = &self.emitters[e].layout;
                        let data = &self.others[e];
                        let mut out = vec![0u32; func.outputs];
                        // "By Index" takes [userptr, index]; "By ID" takes
                        // [userptr, id.index, id.tag]. Output 0 is validity.
                        let k = if name.contains("By ID") {
                            i(last(2))
                        } else {
                            i(last(1))
                        };
                        if let (Some(v), true) = (
                            attribute.as_deref().and_then(|a| layout.get(a)),
                            k >= 0 && (k as usize) < data.len,
                        ) {
                            out[0] = u32::MAX;
                            let words = (0..v.comps.f)
                                .map(|c| data.f[(v.f + c) as usize][k as usize])
                                .chain(
                                    (0..v.comps.i).map(|c| data.i[(v.i + c) as usize][k as usize]),
                                );
                            for (slot, w) in out[1..].iter_mut().zip(words) {
                                *slot = w;
                            }
                        }
                        out
                    }
                    None => vec![0; func.outputs],
                }
            }
            Di::Camera => {
                // The camera comes from the caller (`TickContext::camera`).
                let cam = self.ctx.camera;
                let fwd = cam.rotation * Vec3::X;
                let up = cam.rotation * Vec3::Z;
                let right = cam.rotation * Vec3::Y;
                let p = cam.translation;
                words_f(&[
                    p.x, p.y, p.z, fwd.x, fwd.y, fwd.z, up.x, up.y, up.z, right.x, right.y,
                    right.z, 1920., 1080., 10.,
                ])
            }
            Di::Collision => {
                // Floor plane collision at the environment floor height.
                let start = Vec3::new(
                    f(inputs.get(2).copied().flatten()),
                    f(inputs.get(3).copied().flatten()),
                    f(inputs.get(4).copied().flatten()),
                );
                let end = Vec3::new(
                    f(inputs.get(5).copied().flatten()),
                    f(inputs.get(6).copied().flatten()),
                    f(inputs.get(7).copied().flatten()),
                );
                let floor = self.ctx.env.floor();
                let hit = start.z >= floor && end.z < floor;
                let t = if hit {
                    (start.z - floor) / (start.z - end.z)
                } else {
                    0.
                };
                let p = start + (end - start) * t;
                let mut out = vec![
                    i(inputs.get(1).copied().flatten()) as u32,
                    if hit { u32::MAX } else { 0 },
                    0,
                ];
                out.extend(words_f(&[p.x, p.y, p.z, 0., 0., 1., 0.5, 0.3]));
                out.push(0);
                out
            }
            Di::VectorField { field, tile } => match (name, field) {
                ("SampleField", Some(field)) => {
                    let v = field.sample(
                        Vec3::new(
                            f(inputs.first().copied().flatten()),
                            f(inputs.get(1).copied().flatten()),
                            f(inputs.get(2).copied().flatten()),
                        ),
                        *tile,
                    );
                    words_f(&[v.x, v.y, v.z])
                }
                ("GetVectorFieldTilingAxes", _) => words_f(&tile.map(|t| if t { 1. } else { 0. })),
                (_, Some(field)) if name.contains("Dimensions") => {
                    words_f(&field.size.map(|n| n as f32))
                }
                (_, Some(field)) if name.contains("Bounds") => {
                    let max = field.min + field.extent;
                    words_f(&[field.min.x, field.min.y, field.min.z, max.x, max.y, max.z])
                }
                _ => vec![0; func.outputs],
            },
            Di::Inert => match name {
                "IsActive" | "IsValid" => vec![u32::MAX],
                _ => vec![0; func.outputs],
            },
        };
        let mut out = out;
        out.resize(func.outputs, 0);
        Ok(out)
    }
}

impl Host for Exec<'_> {
    fn random_word(&mut self) -> Result<u32, String> {
        Ok(self.random.next_word())
    }
    fn constant(&self, offset: usize) -> Result<u32, String> {
        let b = offset
            .checked_add(4)
            .and_then(|end| self.constants.get(offset..end))
            .ok_or_else(|| format!("unbound Niagara constant at byte {offset}"))?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn input(&self, d: u16, v: u16, i: u32) -> Result<u32, String> {
        self.input_typed(Numeric::Float, d, v, i)
    }
    fn output(&mut self, d: u16, v: u16, i: u32, w: u32) -> Result<(), String> {
        self.output_typed(Numeric::Float, d, v, i, w)
    }
    fn input_typed(&self, kind: Numeric, d: u16, v: u16, i: u32) -> Result<u32, String> {
        // Event datasets (generators/receivers) are not bound: reads are zero.
        let Some(set) = self.sets.get(d as usize) else {
            return Ok(0);
        };
        let index = set.in_off + i as usize;
        // Spawn reads of unwritten rows return zero, as a fresh allocation does.
        Ok(set
            .input
            .column(kind, v)
            .and_then(|c| c.get(index))
            .copied()
            .unwrap_or(0))
    }
    fn output_typed(
        &mut self,
        kind: Numeric,
        d: u16,
        v: u16,
        i: u32,
        w: u32,
    ) -> Result<(), String> {
        let Some(set) = self.sets.get_mut(d as usize) else {
            return Ok(());
        };
        let index = set.out_off + i as usize;
        let column = match kind {
            Numeric::Float => set.output.f.get_mut(v as usize),
            Numeric::Int => set.output.i.get_mut(v as usize),
            Numeric::Half => set.output.h.get_mut(v as usize),
        };
        if let Some(c) = column {
            if index >= c.len() {
                c.resize(index + 1, 0);
            }
            c[index] = w;
        }
        Ok(())
    }
    fn acquire_index(&mut self, d: u16, valid: bool) -> Result<u32, String> {
        let Some(set) = self.sets.get_mut(d as usize) else {
            return Ok(u32::MAX);
        };
        set.acquire_used = true;
        if !valid {
            return Ok(u32::MAX);
        }
        let n = set.acquired;
        set.acquired += 1;
        Ok(n as u32)
    }
    fn acquire_id(&mut self, d: u16) -> Result<(u32, u32), String> {
        let set = self
            .sets
            .get_mut(d as usize)
            .ok_or("unbound Niagara ID dataset")?;
        let ids = set.ids.as_mut().ok_or("dataset has no persistent IDs")?;
        let id = ids.free.pop().unwrap_or_else(|| {
            ids.next += 1;
            ids.next - 1
        });
        ids.map.insert(id, u32::MAX);
        Ok((id, self.lane))
    }
    fn update_id(&mut self, d: u16, id: u32, index: u32) -> Result<(), String> {
        let set = self
            .sets
            .get_mut(d as usize)
            .ok_or("unbound Niagara ID dataset")?;
        let ids = set.ids.as_mut().ok_or("dataset has no persistent IDs")?;
        if index == u32::MAX {
            if ids.map.remove(&id).is_some() {
                ids.free.push(id);
            }
        } else {
            ids.map.insert(id, index);
        }
        Ok(())
    }
    fn external(&mut self, func: &External, inputs: &[Option<u32>]) -> Result<Vec<u32>, String> {
        let index = self
            .script
            .kernel
            .external
            .iter()
            .position(|e| std::ptr::eq(e, func))
            .ok_or("unknown external")?;
        match self.script.calls[index].clone() {
            Call::Di(di, attribute) => self.call_di(di, attribute, func, inputs),
            Call::FastPath if func.name == "FastMatrixToQuaternion" => {
                let m: [f32; 16] = std::array::from_fn(|k| f(inputs.get(k).copied().flatten()));
                let q = matrix_to_quat(&m).normalize();
                Ok(words_f(&[q.x, q.y, q.z, q.w]))
            }
            _ => Ok(vec![0; func.outputs]),
        }
    }
}

/// One renderable particle, extracted through a renderer's bindings.
#[derive(Clone, Copy, Debug, Default)]
pub struct Particle {
    pub id: i32,
    pub position: Vec3,
    pub velocity: Vec3,
    pub color: [f32; 4],
    pub size: [f32; 2],
    pub rotation: f32,
    pub sub_image: f32,
    pub orientation: Quat,
    pub scale: Vec3,
    pub mesh_index: i32,
    pub ribbon_id: i32,
    pub link_order: f32,
    pub width: f32,
    pub facing: Vec3,
    pub alignment: Vec3,
    pub radius: f32,
    pub age: f32,
}

impl SystemInstance {
    /// Extract renderer-bound attributes for `emitter`'s `renderer`.
    pub fn particles(&self, emitter: usize, renderer: usize) -> Vec<Particle> {
        let asset = &self.asset.emitters[emitter];
        let Some(r) = asset.renderers.get(renderer) else {
            return Vec::new();
        };
        let data = &self.emitters[emitter].data;
        let layout = &asset.layout;
        let var = |key: &str| r.bindings.get(key).and_then(|n| layout.get(n));
        let id = layout.get("UniqueID");
        let get = |v: Option<&Var>, comps: usize, k: usize, default: &[f32]| -> Vec<f32> {
            (0..comps)
                .map(|c| {
                    v.map(|v| data.float(v, c, k))
                        .filter(|_| v.is_some_and(|v| (c as u16) < v.comps.f))
                        .unwrap_or(default[c])
                })
                .collect()
        };
        let pos = var("PositionBinding");
        let color = var("ColorBinding");
        let vel = var("VelocityBinding");
        let size = var("SpriteSizeBinding");
        let rot = var("SpriteRotationBinding");
        let sub = var("SubImageIndexBinding");
        let orient = var("MeshOrientationBinding");
        let scale = var("ScaleBinding");
        let mesh = var("MeshIndexBinding");
        let ribbon = var("RibbonIdBinding");
        let link = var("RibbonLinkOrderBinding");
        let width = var("RibbonWidthBinding");
        let radius = var("LightRadiusBinding");
        let age = var("NormalizedAgeBinding");
        let facing = var("SpriteFacingBinding");
        let alignment = var("SpriteAlignmentBinding");
        let mut out = Vec::with_capacity(data.len);
        for k in 0..data.len {
            let p = get(pos, 3, k, &[0., 0., 0.]);
            let c = get(color, 4, k, &[1., 1., 1., 1.]);
            let v = get(vel, 3, k, &[0., 0., 0.]);
            let s = get(size, 2, k, &[10., 10.]);
            let q = get(orient, 4, k, &[0., 0., 0., 1.]);
            let sc = get(scale, 3, k, &[1., 1., 1.]);
            out.push(Particle {
                id: id.map_or(k as i32, |v| data.int(v, 0, k)),
                position: Vec3::from_slice(&p),
                velocity: Vec3::from_slice(&v),
                color: [c[0], c[1], c[2], c[3]],
                size: [s[0], s[1]],
                rotation: get(rot, 1, k, &[0.])[0],
                sub_image: get(sub, 1, k, &[0.])[0],
                orientation: Quat::from_xyzw(q[0], q[1], q[2], q[3]),
                scale: Vec3::from_slice(&sc),
                mesh_index: mesh.map_or(0, |v| data.int(v, 0, k)),
                ribbon_id: ribbon.map_or(0, |v| {
                    if v.comps.i > 0 {
                        data.int(v, 0, k)
                    } else {
                        data.float(v, 0, k) as i32
                    }
                }),
                link_order: get(link, 1, k, &[k as f32])[0],
                width: get(width, 1, k, &[10.])[0],
                facing: Vec3::from_slice(&get(facing, 3, k, &[1., 0., 0.])),
                alignment: Vec3::from_slice(&get(alignment, 3, k, &[1., 0., 0.])),
                radius: get(radius, 1, k, &[100.])[0],
                age: get(age, 1, k, &[0.])[0],
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn ty(name: &str) -> Value {
        json!({"ClassStructOrEnum":{"ObjectName":format!("Class'{name}'")},"UnderlyingType":2})
    }
    #[test]
    fn layouts_follow_build_layout_component_order() {
        let vars = [
            json!({"Name":"Position","TypeDef":ty("NiagaraPosition")}),
            json!({"Name":"ID","TypeDef":ty("NiagaraID")}),
            json!({"Name":"Age","TypeDef":ty("NiagaraFloat")}),
            json!({"Name":"S","TypeDef":ty("NiagaraSpawnInfo")}),
        ];
        let l = Layout::build(vars.iter()).unwrap();
        assert_eq!(
            (
                l.get("Position").unwrap().f,
                l.get("Age").unwrap().f,
                l.get("S").unwrap().f
            ),
            (0, 3, 4)
        );
        assert_eq!((l.get("ID").unwrap().i, l.get("S").unwrap().i), (0, 2));
        assert_eq!((l.floats, l.ints), (6, 4));
    }
    #[test]
    fn owner_matrix_is_row_major_with_translation_row() {
        let t = Transform {
            translation: Vec3::new(1., 2., 3.),
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            scale: Vec3::ONE,
        };
        let m = matrix(&t);
        assert!((m[1] - 1.).abs() < 1e-6 && m[0].abs() < 1e-6);
        assert_eq!(&m[12..15], &[1., 2., 3.]);
        let q = matrix_to_quat(&m);
        assert!((q * Vec3::X - Vec3::Y).length() < 1e-5);
    }
}
