# jka-ue-runtime

Native, bounded execution of Unreal Engine program data that
[`jka-ue-assets`](../jka-ue-assets/README.md) (or any exporter that follows
CUE4Parse's JSON) leaves behind: Blueprint bytecode, Niagara VectorVM kernels
and material expressions. It is a set of **embeddable building blocks**, not an
Unreal host: it has no UObject model, no RHI and no renderer. Your game
supplies those through small traits and decides what is allowed to happen.

| Module | What it runs | What the embedding game provides |
| --- | --- | --- |
| `material` | validated expression DAG (arithmetic, time, UV, position, normal, Fresnel, lerp, ...) | a `Sample` per vertex or pixel |
| `blueprint` | Kismet instruction AST: branches, locals, containers, casts, delegates, in/out args | a `blueprint::Host` for properties, class checks and native calls |
| `vector_vm` | original (non-optimised) Niagara VectorVM bytecode | a `vector_vm::Host` for datasets, constants and external functions |
| `niagara` | CPU context for one kernel: typed datasets, persistent IDs, curve LUTs | constant buffers in the script's engine order |
| `niagara_sim` | whole Niagara systems: system/emitter scheduling, spawn, update, renderers | an `Environment` (skeleton, sockets, floor) and a `TickContext` |
| `niagara_stateless`, `niagara_gpu_approx` | stateless emitters; GPU emitters as a *labelled* approximation | nothing extra |

Everything is deterministic (seeded XORWOW streams), allocation-bounded and
returns `Err` for anything it cannot honour. A missing binding is an error, never
a guessed value.

## Example: evaluate a material expression

```rust
use jka_ue_assets::material::Program;
use jka_ue_runtime::material::{evaluate, Sample};

// core colour -> edge colour by Fresnel (what the importer emits for glow materials)
let program = Program::parse(br#"{"version":1,"unlit":true,"output":3,"nodes":[
    {"op":"constant","value":[0,0,1,1]},
    {"op":"constant","value":[1,1,1,1]},
    {"op":"fresnel","exponent":2.0},
    {"op":"lerp","inputs":[0,1,2]}]}"#)?;
let facing = Sample {
    seconds: 0.0,
    position: glam::Vec3::ZERO,
    normal: glam::Vec3::Z,
    uv: [0.0, 0.0],
    eye: glam::Vec3::new(0.0, 0.0, 10.0),
};
assert_eq!(evaluate(&program, &facing)?, [0.0, 0.0, 1.0, 1.0]); // facing the eye: core colour
# Ok::<(), String>(())
```

## Example: run an exported Blueprint function

```rust
use jka_ue_runtime::blueprint::{Frame, Function, Host};
use serde_json::{json, Value};

/// The game decides what a native call does; anything unbound is an error.
struct Game;
impl Host for Game {
    fn read_property(&self, _: &Value, name: &str) -> Result<Value, String> { Err(format!("no property {name}")) }
    fn write_property(&mut self, _: &Value, _: &str, _: Value) -> Result<(), String> { Ok(()) }
    fn call(&mut self, _: &Value, function: &Value, args: &mut [Value]) -> Result<Value, String> {
        match function.as_str() {
            Some("Increment") => { args[0] = json!(args[0].as_i64().ok_or("not an int")? + 1); Ok(Value::Null) }
            other => Err(format!("native function {other:?} has no binding")),
        }
    }
}

let local = |name: &str| json!({"Token":"EX_LocalVariable","Variable":{"Property":{"Name":name}}});
let function = Function::parse(&json!({"Type":"Function","Name":"Bump","ScriptBytecode":[
    {"StatementIndex":0,  "Token":"EX_FinalFunction","Function":"Increment","Parameters":[local("n")]},
    {"StatementIndex":10, "Token":"EX_Return","Expression":local("n")},
    {"StatementIndex":20, "Token":"EX_EndOfScript"}]}))?;

let mut frame = Frame::default();
frame.locals.insert("n".into(), json!(4));
assert_eq!(function.execute(&mut frame, &mut Game, 1_000)?, json!(5)); // 1_000 = instruction budget
# Ok::<(), String>(())
```

## Example: tick a Niagara system

`SystemAsset::parse` takes the exported package (an array of exported objects);
the rest is a plain loop. This sketch only compiles, since it needs real data:

```rust,no_run
use jka_ue_runtime::niagara_sim::{NoSkeleton, SystemAsset, SystemInstance, TickContext, Transform};
use std::sync::Arc;

let package: serde_json::Value = serde_json::from_slice(&std::fs::read("Objects/<id>.json")?)?;
let asset = Arc::new(SystemAsset::parse(&package)?);
println!("unbound external functions: {:?}", asset.unbound_functions());
let mut system = SystemInstance::new(asset, /* seed */ 1);
for frame in 0..60 {
    let t = frame as f32 / 30.0;
    system.tick(&TickContext {
        dt: 1.0 / 30.0, owner: Transform::default(), owner_velocity: glam::Vec3::ZERO,
        time: t, env: &NoSkeleton, camera: Transform::default(),
    });
}
for particle in system.particles(/* emitter */ 0, /* renderer */ 0) {
    // position / size / colour in UE units (cm, Z up); convert when drawing
    let _ = particle;
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

Implement `Environment` to expose a skeleton, sockets and a floor height to
skeletal-mesh and collision data interfaces. `SystemInstance::errors` lists
anything that could not run (unbound data interfaces, unsupported modules).

## Fidelity and limits

* VectorVM opcode/operand ABI follows UE 5.8's `VectorVMOptimizer.cpp`;
  scheduling follows `FNiagaraSystemSimulation` / `FNiagaraEmitterInstanceImpl`.
  Optimised-VM and GPU shader bytecode use different ABIs and are rejected.
* Noise opcodes that return zero in the reference VM return zero here; vector
  field data interfaces need the field asset (`VectorField::from_export`).
* GPU emitters keep their real spawn counts, curves and bounds but derive
  lifetime and velocity heuristically; renderers built on them are flagged as
  approximations.
* Decoding a program is not the same as it behaving like the engine: native
  engine/game classes are not part of any public SDK, so calls into them fail
  loudly until you bind them.
* Material expressions are a small DAG for vertex colour / alpha. This is not a
  per-pixel graph VM and not PBR.

## Testing

```sh
cargo test -p jka-ue-runtime      # unit tests and the examples above
```
