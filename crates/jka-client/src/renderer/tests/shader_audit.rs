//! Specialization audit for the world shaders.
//!
//! `WorldShaderVariantKey` turns settings into WGSL `override` constants, and the
//! pipeline cache compiles one specialized pipeline per key. That only removes a
//! feature if the shader guards it with an override. A feature guarded by a
//! runtime uniform test (`if (weather_surface.puddle.x > 0.001)`) is compiled into
//! every variant and costs registers and code even while it is off. Nothing fails
//! when that happens; the only symptom is a frame-time gap against the lean path.
//!
//! This audit specializes the all-features-off ("minimal") key exactly as the
//! pipeline cache would, folds every branch decided by the constants, and reports
//! what is left in the fragment and vertex entry points: live code size, bindings
//! still used, and branches that still depend on a settings uniform. The
//! settings-driven branches are checked against a reviewed allow-list, so a new
//! feature that forgets its override fails here instead of showing up as a
//! benchmark regression. `bsp_fast.wgsl` is analysed the same way as the
//! reference the unified path is being compared with.
//!
//! Run `cargo test -p DinurdoJK --release shader_audit -- --nocapture` for the
//! full report.

use super::{compose_world_shader, WorldShaderVariantKey};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use wgpu::naga::{
    self, AddressSpace, Arena, BinaryOperator, Block, Expression, Function, Handle, Literal,
    Module, ShaderStage, Statement, UnaryOperator,
};

/// Uniform/storage globals that legitimately vary per draw rather than per
/// setting. Branches that depend only on these are not audited.
const PER_DRAW_GLOBALS: &[&str] = &["material"];

/// Locals currently holding a known constant. naga lowers `a || b` / `a && b` into
/// a temporary local and an `if`, so conditions are often `Load(local)`.
type Env = HashMap<Handle<naga::LocalVariable>, Value>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Value {
    Bool(bool),
    U32(u32),
    I32(i32),
    F32(f32),
}

fn literal(value: &Literal) -> Option<Value> {
    match *value {
        Literal::Bool(v) => Some(Value::Bool(v)),
        Literal::U32(v) => Some(Value::U32(v)),
        Literal::I32(v) => Some(Value::I32(v)),
        Literal::F32(v) => Some(Value::F32(v)),
        _ => None,
    }
}

/// Constant-folds the small expression subset override-driven conditions use.
fn eval(
    module: &Module,
    arena: &Arena<Expression>,
    env: &Env,
    handle: Handle<Expression>,
) -> Option<Value> {
    match &arena[handle] {
        Expression::Literal(value) => literal(value),
        Expression::Constant(constant) => eval(
            module,
            &module.global_expressions,
            &Env::new(),
            module.constants[*constant].init,
        ),
        Expression::Select {
            condition,
            accept,
            reject,
        } => match eval(module, arena, env, *condition)? {
            Value::Bool(true) => eval(module, arena, env, *accept),
            Value::Bool(false) => eval(module, arena, env, *reject),
            _ => None,
        },
        Expression::Load { pointer } => match &arena[*pointer] {
            Expression::LocalVariable(local) => env.get(local).copied(),
            _ => None,
        },
        Expression::Unary {
            op: UnaryOperator::LogicalNot,
            expr,
        } => match eval(module, arena, env, *expr)? {
            Value::Bool(value) => Some(Value::Bool(!value)),
            _ => None,
        },
        Expression::Binary { op, left, right } => {
            let left = eval(module, arena, env, *left);
            let right = eval(module, arena, env, *right);
            match op {
                BinaryOperator::LogicalAnd => {
                    if left == Some(Value::Bool(false)) || right == Some(Value::Bool(false)) {
                        Some(Value::Bool(false))
                    } else if left == Some(Value::Bool(true)) && right == Some(Value::Bool(true)) {
                        Some(Value::Bool(true))
                    } else {
                        None
                    }
                }
                BinaryOperator::LogicalOr => {
                    if left == Some(Value::Bool(true)) || right == Some(Value::Bool(true)) {
                        Some(Value::Bool(true))
                    } else if left == Some(Value::Bool(false)) && right == Some(Value::Bool(false))
                    {
                        Some(Value::Bool(false))
                    } else {
                        None
                    }
                }
                BinaryOperator::And | BinaryOperator::InclusiveOr | BinaryOperator::ExclusiveOr => {
                    match (left?, right?) {
                        (Value::U32(a), Value::U32(b)) => Some(Value::U32(match op {
                            BinaryOperator::And => a & b,
                            BinaryOperator::InclusiveOr => a | b,
                            _ => a ^ b,
                        })),
                        _ => None,
                    }
                }
                _ => compare(*op, left?, right?),
            }
        }
        _ => None,
    }
}

fn compare(op: BinaryOperator, left: Value, right: Value) -> Option<Value> {
    use std::cmp::Ordering;
    let ordering = match (left, right) {
        (Value::U32(a), Value::U32(b)) => a.cmp(&b),
        (Value::I32(a), Value::I32(b)) => a.cmp(&b),
        (Value::F32(a), Value::F32(b)) => a.partial_cmp(&b)?,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(&b),
        _ => return None,
    };
    Some(Value::Bool(match op {
        BinaryOperator::Equal => ordering == Ordering::Equal,
        BinaryOperator::NotEqual => ordering != Ordering::Equal,
        BinaryOperator::Less => ordering == Ordering::Less,
        BinaryOperator::LessEqual => ordering != Ordering::Greater,
        BinaryOperator::Greater => ordering == Ordering::Greater,
        BinaryOperator::GreaterEqual => ordering != Ordering::Less,
        _ => return None,
    }))
}

fn children(expression: &Expression, out: &mut Vec<Handle<Expression>>) {
    match expression {
        Expression::Access { base, index } => out.extend([*base, *index]),
        Expression::AccessIndex { base, .. } => out.push(*base),
        Expression::Splat { value, .. } => out.push(*value),
        Expression::Swizzle { vector, .. } => out.push(*vector),
        Expression::Load { pointer } => out.push(*pointer),
        Expression::Unary { expr, .. } => out.push(*expr),
        Expression::Binary { left, right, .. } => out.extend([*left, *right]),
        Expression::Select {
            condition,
            accept,
            reject,
        } => out.extend([*condition, *accept, *reject]),
        Expression::Math {
            arg,
            arg1,
            arg2,
            arg3,
            ..
        } => {
            out.push(*arg);
            out.extend(arg1.iter().chain(arg2.iter()).chain(arg3.iter()).copied());
        }
        Expression::As { expr, .. } => out.push(*expr),
        Expression::Derivative { expr, .. } => out.push(*expr),
        Expression::Relational { argument, .. } => out.push(*argument),
        Expression::Compose { components, .. } => out.extend(components.iter().copied()),
        Expression::ImageSample {
            image,
            sampler,
            coordinate,
            array_index,
            ..
        } => {
            out.extend([*image, *sampler, *coordinate]);
            out.extend(array_index.iter().copied());
        }
        Expression::ImageLoad {
            image,
            coordinate,
            array_index,
            ..
        } => {
            out.extend([*image, *coordinate]);
            out.extend(array_index.iter().copied());
        }
        Expression::ImageQuery { image, .. } => out.push(*image),
        Expression::ArrayLength(pointer) => out.push(*pointer),
        _ => {}
    }
}

fn stored_locals(
    block: &Block,
    function: &Function,
    out: &mut BTreeSet<Handle<naga::LocalVariable>>,
) {
    for statement in block.iter() {
        match statement {
            Statement::Store { pointer, .. } => {
                if let Expression::LocalVariable(local) = &function.expressions[*pointer] {
                    out.insert(*local);
                }
            }
            Statement::Block(inner) => stored_locals(inner, function, out),
            Statement::If { accept, reject, .. } => {
                stored_locals(accept, function, out);
                stored_locals(reject, function, out);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    stored_locals(&case.body, function, out);
                }
            }
            Statement::Loop {
                body, continuing, ..
            } => {
                stored_locals(body, function, out);
                stored_locals(continuing, function, out);
            }
            _ => {}
        }
    }
}

/// What remains of one entry point after specialization.
#[derive(Default)]
struct Report {
    live_functions: BTreeSet<String>,
    emitted_expressions: usize,
    /// Live expressions by function, to show where a lean-vs-fast gap sits.
    per_function: BTreeMap<String, usize>,
    bindings: BTreeSet<String>,
    /// Runtime branches whose condition reads a settings uniform.
    settings_branches: BTreeMap<String, u32>,
    /// Runtime branches that depend only on per-draw / per-fragment data.
    other_branches: u32,
}

struct Audit<'a> {
    module: &'a Module,
    visited: BTreeSet<Handle<Function>>,
    globals: BTreeSet<Handle<naga::GlobalVariable>>,
    report: Report,
}

impl<'a> Audit<'a> {
    fn global_name(&self, handle: Handle<naga::GlobalVariable>) -> String {
        self.module.global_variables[handle]
            .name
            .clone()
            .unwrap_or_else(|| format!("global{}", handle.index()))
    }

    fn is_settings_global(&self, name: &str) -> bool {
        !PER_DRAW_GLOBALS.contains(&name)
            && self.module.global_variables.iter().any(|(_, global)| {
                global.name.as_deref() == Some(name)
                    && matches!(
                        global.space,
                        AddressSpace::Uniform | AddressSpace::Storage { .. }
                    )
            })
    }

    fn collect_globals(&mut self, function: &Function, root: Handle<Expression>) {
        let mut stack = vec![root];
        let mut seen = BTreeSet::new();
        while let Some(handle) = stack.pop() {
            if !seen.insert(handle) {
                continue;
            }
            let expression = &function.expressions[handle];
            if let Expression::GlobalVariable(global) = expression {
                self.globals.insert(*global);
            }
            children(expression, &mut stack);
        }
    }

    /// `global.member` for loads through a global, e.g. `camera.render_flags`.
    fn path(&self, function: &Function, handle: Handle<Expression>) -> Option<String> {
        match &function.expressions[handle] {
            Expression::GlobalVariable(global) => Some(self.global_name(*global)),
            Expression::AccessIndex { base, index } => {
                let base_path = self.path(function, *base)?;
                if let Expression::GlobalVariable(global) = &function.expressions[*base] {
                    let ty = self.module.global_variables[*global].ty;
                    if let naga::TypeInner::Struct { members, .. } = &self.module.types[ty].inner {
                        if let Some(name) =
                            members.get(*index as usize).and_then(|m| m.name.as_ref())
                        {
                            return Some(format!("{base_path}.{name}"));
                        }
                    }
                }
                Some(base_path)
            }
            Expression::Access { base, .. } => self.path(function, *base),
            _ => None,
        }
    }

    fn dependencies(
        &self,
        function: &Function,
        handle: Handle<Expression>,
        calls: &HashMap<Handle<Expression>, String>,
        out: &mut BTreeSet<String>,
    ) {
        match &function.expressions[handle] {
            Expression::Literal(_) | Expression::Constant(_) | Expression::ZeroValue(_) => {}
            Expression::FunctionArgument(_) => {
                out.insert("arg".to_owned());
            }
            Expression::LocalVariable(_) => {
                out.insert("local".to_owned());
            }
            Expression::CallResult(_) => {
                out.insert(format!(
                    "callee:{}",
                    calls.get(&handle).map_or("?", String::as_str)
                ));
            }
            expression => {
                if let Some(path) = self.path(function, handle) {
                    out.insert(path);
                    return;
                }
                let mut next = Vec::new();
                children(expression, &mut next);
                for child in next {
                    self.dependencies(function, child, calls, out);
                }
            }
        }
    }

    fn record_branch(
        &mut self,
        function: &Function,
        name: &str,
        condition: Handle<Expression>,
        calls: &HashMap<Handle<Expression>, String>,
    ) {
        let mut deps = BTreeSet::new();
        self.dependencies(function, condition, calls, &mut deps);
        let settings: Vec<&String> = deps
            .iter()
            .filter(|dep| {
                dep.split('.')
                    .next()
                    .is_some_and(|global| self.is_settings_global(global))
            })
            .collect();
        if settings.is_empty() {
            self.report.other_branches += 1;
        } else {
            let joined = settings
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            *self
                .report
                .settings_branches
                .entry(format!("{name}: {joined}"))
                .or_default() += 1;
        }
    }

    fn audit_function(
        &mut self,
        name: &str,
        function: &Function,
        handle: Option<Handle<Function>>,
    ) {
        if let Some(handle) = handle {
            if !self.visited.insert(handle) {
                return;
            }
        }
        self.report.live_functions.insert(name.to_owned());
        let mut calls = HashMap::new();
        let mut env = Env::new();
        for (local, variable) in function.local_variables.iter() {
            if let Some(init) = variable.init {
                if let Some(value) = eval(self.module, &function.expressions, &env, init) {
                    env.insert(local, value);
                }
            }
        }
        self.walk(function, name, &function.body, &mut calls, &mut env);
    }

    /// Returns true when the block always ends the path (return/discard/break).
    fn walk(
        &mut self,
        function: &Function,
        name: &str,
        block: &Block,
        calls: &mut HashMap<Handle<Expression>, String>,
        env: &mut Env,
    ) -> bool {
        for statement in block.iter() {
            match statement {
                Statement::Emit(range) => {
                    for handle in range.clone() {
                        self.report.emitted_expressions += 1;
                        *self.report.per_function.entry(name.to_owned()).or_default() += 1;
                        self.collect_globals(function, handle);
                    }
                }
                Statement::Block(inner) => {
                    if self.walk(function, name, inner, calls, env) {
                        return true;
                    }
                }
                Statement::If {
                    condition,
                    accept,
                    reject,
                } => {
                    match eval(self.module, &function.expressions, env, *condition) {
                        Some(Value::Bool(true)) => {
                            if self.walk(function, name, accept, calls, env) {
                                return true;
                            }
                        }
                        Some(Value::Bool(false)) => {
                            if self.walk(function, name, reject, calls, env) {
                                return true;
                            }
                        }
                        _ => {
                            self.record_branch(function, name, *condition, calls);
                            self.collect_globals(function, *condition);
                            let mut assigned = BTreeSet::new();
                            stored_locals(accept, function, &mut assigned);
                            stored_locals(reject, function, &mut assigned);
                            let mut accept_env = env.clone();
                            let mut reject_env = env.clone();
                            let accepted =
                                self.walk(function, name, accept, calls, &mut accept_env);
                            let rejected =
                                self.walk(function, name, reject, calls, &mut reject_env);
                            // Join: a local keeps its constant when every path that
                            // reaches the join agrees (`x && false` stores false on both).
                            for local in &assigned {
                                let accept_value = accept_env.get(local).copied();
                                let reject_value = reject_env.get(local).copied();
                                let joined = match (accepted, rejected) {
                                    (true, false) => reject_value,
                                    (false, true) => accept_value,
                                    _ if accept_value == reject_value => accept_value,
                                    _ => None,
                                };
                                match joined {
                                    Some(value) => {
                                        env.insert(*local, value);
                                    }
                                    None => {
                                        env.remove(local);
                                    }
                                }
                            }
                            if accepted && rejected {
                                return true;
                            }
                        }
                    }
                }
                Statement::Switch { selector, cases } => {
                    self.collect_globals(function, *selector);
                    let mut assigned = BTreeSet::new();
                    for case in cases {
                        stored_locals(&case.body, function, &mut assigned);
                        let mut case_env = env.clone();
                        self.walk(function, name, &case.body, calls, &mut case_env);
                    }
                    for local in &assigned {
                        env.remove(local);
                    }
                }
                Statement::Loop {
                    body,
                    continuing,
                    break_if,
                } => {
                    let mut assigned = BTreeSet::new();
                    stored_locals(body, function, &mut assigned);
                    stored_locals(continuing, function, &mut assigned);
                    for local in &assigned {
                        env.remove(local);
                    }
                    let mut body_env = env.clone();
                    self.walk(function, name, body, calls, &mut body_env);
                    let mut continuing_env = env.clone();
                    self.walk(function, name, continuing, calls, &mut continuing_env);
                    if let Some(condition) = break_if {
                        self.collect_globals(function, *condition);
                    }
                }
                Statement::Return { value } => {
                    if let Some(value) = value {
                        self.collect_globals(function, *value);
                    }
                    return true;
                }
                Statement::Kill | Statement::Break | Statement::Continue => return true,
                Statement::Store { pointer, value } => {
                    self.collect_globals(function, *pointer);
                    self.collect_globals(function, *value);
                    if let Expression::LocalVariable(local) = &function.expressions[*pointer] {
                        match eval(self.module, &function.expressions, env, *value) {
                            Some(known) => {
                                env.insert(*local, known);
                            }
                            None => {
                                env.remove(local);
                            }
                        }
                    }
                }
                Statement::Call {
                    function: callee,
                    arguments,
                    result,
                } => {
                    for argument in arguments {
                        self.collect_globals(function, *argument);
                    }
                    let callee_name = self.module.functions[*callee]
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("fn{}", callee.index()));
                    if let Some(result) = result {
                        calls.insert(*result, callee_name.clone());
                    }
                    let module = self.module;
                    self.audit_function(&callee_name, &module.functions[*callee], Some(*callee));
                }
                _ => {}
            }
        }
        false
    }
}

fn audit_entry(module: &Module, stage: ShaderStage, entry: &str) -> Report {
    let entry_point = module
        .entry_points
        .iter()
        .find(|ep| ep.stage == stage && ep.name == entry)
        .unwrap_or_else(|| panic!("entry point {entry} not found"));
    let mut audit = Audit {
        module,
        visited: BTreeSet::new(),
        globals: BTreeSet::new(),
        report: Report::default(),
    };
    audit.audit_function(entry, &entry_point.function, None);
    let mut bindings = BTreeSet::new();
    for global in &audit.globals {
        let variable = &module.global_variables[*global];
        let name = audit.global_name(*global);
        bindings.insert(match variable.binding {
            Some(binding) => format!("@{}/{} {name}", binding.group, binding.binding),
            None => name,
        });
    }
    audit.report.bindings = bindings;
    audit.report
}

fn print_report(label: &str, report: &Report) {
    println!(
        "== {label}: {} live functions, {} live expressions, {} bindings, {} settings-driven + {} per-draw runtime branches",
        report.live_functions.len(),
        report.emitted_expressions,
        report.bindings.len(),
        report.settings_branches.values().sum::<u32>(),
        report.other_branches,
    );
    for binding in &report.bindings {
        println!("     binding {binding}");
    }
    for (branch, count) in &report.settings_branches {
        println!("     settings branch x{count}  {branch}");
    }
}

/// The all-features-off key: what a Minimal preset on a dry, unfogged map asks for.
/// Every field is spelled out so adding a key field forces this test to be updated.
fn minimal_key() -> WorldShaderVariantKey {
    WorldShaderVariantKey {
        pbr: false,
        pom: false,
        pbr_shared_material_eval: false,
        pbr_shared_tangent_frame: false,
        pom_mip_aware: false,
        pom_adaptive_steps: false,
        pbr_companion_sampler: false,
        pbr_vertex_lightgrid: false,
        point_lights: false,
        map_light_simulation: false,
        source_map_world: false,
        legacy_dlights: false,
        vertex_dlights: false,
        clustered_lite_dlights: false,
        area_lights: false,
        irradiance_volume: false,
        voxel_gi: false,
        local_shadows: false,
        cascaded_shadows: false,
        csm_coverage: false,
        ray_traced_shadows: false,

        rt_receiver_glass: false,
        planar_reflections: false,
        legacy_fog: false,
        jump_shade: false,
        ocean: false,
        weather_surface: false,
        classic_render_flags: false,
        static_bsp_ao: false,
        baked_sun_relight: false,
        planar_debug: false,
        deluxe: false,
        deluxe_specular: false,
        detail_textures_enabled: false,
        surface_deformation: false,
        dynamic_light_debug: false,
        material_specialization: false,
        minimal_entries: false,
        program_mode: super::world::programs::WorldProgramMode::Specialized,
        sky_debug: false,
    }
}

#[test]
fn baked_sun_relight_variant_validates() {
    for cascaded_shadows in [false, true] {
        let variant = WorldShaderVariantKey {
            baked_sun_relight: true,
            cascaded_shadows,
            ..minimal_key()
        };
        assert!(!WorldShaderVariantKey {
            minimal_entries: true,
            material_specialization: true,
            ..variant
        }.minimal_entry_compatible());
        let module = specialized_lean_module(variant);
        let report = audit_entry(&module, ShaderStage::Fragment, "fs_main");
        assert!(report.live_functions.contains("remove_baked_sun"));
        assert!(report.live_functions.contains("original_sun_visibility"),
            "Authored-sun visibility must survive even with runtime CSM disabled");
    }
}

fn specialized_lean_module(key: WorldShaderVariantKey) -> Module {
    let source = compose_world_shader(include_str!("../../bsp_lean.wgsl"));
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("bsp_lean: {}", e.emit_to_string(&source)));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("bsp_lean validates");
    // Audit the DrawClass::Opaque program (create_world_pipeline adds this per class).
    let constants: naga::back::PipelineConstants = key
        .compilation_constants()
        .into_iter()
        .chain([("ENABLE_ALPHA_TEST", 0.0)])
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    let (specialized, _) =
        naga::back::pipeline_constants::process_overrides(&module, &info, None, &constants)
            .expect("minimal key specializes");
    specialized.into_owned()
}

fn fast_module() -> Module {
    let source = format!(
        "{}\n{}\n{}",
        include_str!("../../bsp_fast.wgsl"),
        crate::foliage::surface_shader(),
        include_str!("../../sky_fractal.wgsl")
    );
    naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("bsp_fast: {}", e.emit_to_string(&source)))
}

/// Settings-driven runtime branches the minimal lean fragment shader is allowed to
/// keep, each with the reason it is runtime on purpose. Anything not listed here
/// is a feature that skipped the variant key: give it an `override` and a key
/// field (see `weather_surface`) rather than extending this list. When writing a
/// condition, put the `ENABLE_*` test first (`ENABLE_X && runtime`, `!ENABLE_X ||
/// runtime`): naga lowers `&&`/`||` as short-circuit control flow, so a constant
/// on the right cannot fold the result.
const ALLOWED_FRAGMENT_SETTINGS_BRANCHES: &[(&str, &str)] = &[
];

/// Bindings the minimal lean fragment shader may still read beyond what
/// `bsp_fast.wgsl` reads.
const ALLOWED_EXTRA_FRAGMENT_BINDINGS: &[(&str, &str)] = &[];

#[test]
fn minimal_world_variant_shader_audit() {
    let lean = specialized_lean_module(minimal_key());
    let lean_fragment = audit_entry(&lean, ShaderStage::Fragment, "fs_main");
    let lean_vertex = audit_entry(&lean, ShaderStage::Vertex, "vs_main");
    let fast = fast_module();
    let fast_fragment = audit_entry(&fast, ShaderStage::Fragment, "fs_main");
    let fast_vertex = audit_entry(&fast, ShaderStage::Vertex, "vs_main");

    print_report("bsp_fast  fs_main (reference)", &fast_fragment);
    print_report("bsp_lean  fs_main (minimal key)", &lean_fragment);
    print_report("bsp_fast  vs_main (reference)", &fast_vertex);
    print_report("bsp_lean  vs_main (minimal key)", &lean_vertex);

    let mut by_function: Vec<_> = lean_fragment.per_function.iter().collect();
    by_function.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (function, lean_count) in by_function {
        let fast_count = fast_fragment
            .per_function
            .get(function)
            .copied()
            .unwrap_or(0);
        println!("   fs expr  {function:<44} lean {lean_count:>4}  fast {fast_count:>4}");
    }
    println!(
        "   live in bsp_lean fs_main but not in bsp_fast: {}",
        lean_fragment
            .live_functions
            .difference(&fast_fragment.live_functions)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );

    let unexpected: Vec<&String> = lean_fragment
        .settings_branches
        .keys()
        .filter(|branch| {
            !ALLOWED_FRAGMENT_SETTINGS_BRANCHES
                .iter()
                .any(|(allowed, _)| allowed == &branch.as_str())
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "minimal bsp_lean fs_main still branches at runtime on settings uniforms \
         (give each feature a WorldShaderVariantKey override, or allow it with a reason):\n  {}",
        unexpected
            .iter()
            .map(|b| b.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    let stale: Vec<&str> = ALLOWED_FRAGMENT_SETTINGS_BRANCHES
        .iter()
        .map(|(allowed, _)| *allowed)
        .filter(|allowed| !lean_fragment.settings_branches.contains_key(*allowed))
        .chain(
            ALLOWED_EXTRA_FRAGMENT_BINDINGS
                .iter()
                .map(|(allowed, _)| *allowed)
                .filter(|allowed| !lean_fragment.bindings.contains(*allowed)),
        )
        .collect();
    assert!(
        stale.is_empty(),
        "allow-list entries the minimal shader no longer needs (delete them so the list stays honest):
  {}",
        stale.join("
  ")
    );

    let extra_bindings: Vec<&String> = lean_fragment
        .bindings
        .difference(&fast_fragment.bindings)
        .filter(|binding| {
            !ALLOWED_EXTRA_FRAGMENT_BINDINGS
                .iter()
                .any(|(allowed, _)| allowed == &binding.as_str())
        })
        .collect();
    assert!(
        extra_bindings.is_empty(),
        "minimal bsp_lean fs_main still reads bindings bsp_fast does not (a feature compiled in \
         without an override, or an allow-list entry is missing):\n  {}",
        extra_bindings
            .iter()
            .map(|b| b.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// Every `override` a world shader declares must be fed by the key. One that is
/// not silently keeps its WGSL default in every variant.
#[test]
fn every_declared_world_override_is_supplied_by_the_variant_key() {
    // Declared in bsp_lean.wgsl but referenced nowhere; safe to delete, kept out of
    // the way of that file's owner.
    // ENABLE_ALPHA_TEST is supplied per pipeline class by create_world_pipeline.
    const EXEMPT: &[&str] = &["ENABLE_CLOUD_SHADOWS", "ENABLE_ALPHA_TEST"];
    // MATERIAL_KEY_* are supplied per specialized PSO by MaterialFragmentKey.
    let supplied: BTreeSet<&str> = minimal_key()
        .compilation_constants()
        .into_iter()
        .chain(super::MaterialFragmentKey::default().compilation_constants())
        .map(|(name, _)| name)
        .collect();
    let mut missing = Vec::new();
    for (file, base) in [
        ("bsp.wgsl", include_str!("../../bsp.wgsl")),
        ("bsp_lean.wgsl", include_str!("../../bsp_lean.wgsl")),
    ] {
        let source = compose_world_shader(base);
        for line in source.lines() {
            let Some(rest) = line.trim_start().strip_prefix("override ") else {
                continue;
            };
            let name = rest.split(':').next().unwrap_or("").trim();
            if !supplied.contains(name) && !EXEMPT.contains(&name) {
                missing.push(format!("{file}: {name}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "world shader overrides not supplied by WorldShaderVariantKey::compilation_constants:\n  {}",
        missing.join("\n  ")
    );
}

/// Regression: toggling the isolated OpenJK Legacy projected-dlight pass must
/// not create a distinct *base-world* shader program. In contrast, the Legacy
/// projected-light entry point still has the authored feature enabled.
#[test]
fn legacy_light_per_pass_specialization_key() {
    let mut legacy = minimal_key();
    legacy.legacy_dlights = true;
    let off = legacy.base_pass_key();
    assert_eq!(off, minimal_key());
    assert_eq!(off.compilation_constants(), minimal_key().compilation_constants());
    assert_ne!(legacy.compilation_constants(), off.compilation_constants());
    assert_eq!(
        legacy.compilation_constants().into_iter()
            .find(|(name, _)| *name == "ENABLE_LEGACY_DLIGHTS").map(|(_, value)| value),
        Some(1.0),
    );
    assert_eq!(
        off.compilation_constants().into_iter()
            .find(|(name, _)| *name == "ENABLE_LEGACY_DLIGHTS").map(|(_, value)| value),
        Some(0.0),
    );
    // Other visual features, and in particular ray-tracing and planar
    // reflection specialization, MUST still require distinct shaders.
    let mut with_gi = legacy;
    with_gi.voxel_gi = true;
    assert_ne!(with_gi.base_pass_key(), off);
    let mut with_rt = legacy;
    with_rt.ray_traced_shadows = true;
    assert_ne!(with_rt.base_pass_key(), off);
    let mut with_reflections = legacy;
    with_reflections.planar_reflections = true;
    assert_ne!(with_reflections.base_pass_key(), off);
}

/// Effective per-pass PSO identities are stricter than the user-settings key:
/// only the *separate* Legacy additive pass may differ while the base shader
/// remains identical. The new cache must never alias changed shaders, formats,
/// sample counts, or blend/depth/cull state.
#[test]
fn world_pass_pipeline_dependencies_are_exact() {
    use super::{WorldPassPipelineIdentity, WorldPipelinePass};
    let minimal = minimal_key();
    let mut legacy = minimal;
    legacy.legacy_dlights = true;
    let native = super::ocean_pipeline_key();
    let format = wgpu::TextureFormat::Rgba16Float;
    let make = |variant, pass, key, f, msaa| {
        WorldPassPipelineIdentity::new(variant, pass, key, f, msaa)
    };
    let base = WorldPipelinePass::Base { authored_fog: false };
    assert_eq!(make(minimal, base, native, format, 1),
               make(legacy, base, native, format, 1));
    assert_ne!(make(minimal, WorldPipelinePass::LegacyDlight, native, format, 1),
               make(legacy, WorldPipelinePass::LegacyDlight, native, format, 1));
    assert_ne!(make(minimal, base, native, format, 1),
               make(minimal, WorldPipelinePass::Reflection { authored_fog: false }, native, format, 1));
    assert_ne!(make(minimal, base, native, format, 1),
               make(minimal, base, native, wgpu::TextureFormat::Bgra8UnormSrgb, 1));
    assert_ne!(make(minimal, base, native, format, 1),
               make(minimal, base, native, format, 4));
    let mut other_key = native;
    other_key.depth_equal = !other_key.depth_equal;
    assert_ne!(make(minimal, base, native, format, 1),
               make(minimal, base, other_key, format, 1));
    let mut gi = minimal;
    gi.voxel_gi = true;
    assert_ne!(make(minimal, base, native, format, 1),
               make(gi, base, native, format, 1));
    let mut rt = minimal;
    rt.ray_traced_shadows = true;
    assert_ne!(make(minimal, base, native, format, 1),
               make(rt, base, native, format, 1));
    let mut deluxe = minimal;
    deluxe.deluxe = true;
    assert_ne!(make(minimal, base, native, format, 1),
               make(deluxe, base, native, format, 1));
    assert_ne!(make(minimal, base, native, format, 1),
               make(minimal, WorldPipelinePass::Base { authored_fog: true }, native, format, 1));
}

/// r_fragmentShaderMode 1: every world shader (lean, enhanced) must accept the
/// material-key overrides for the entry points a specialized PSO uses, and the key
/// must narrow only bits whose code the global variant compiles out.
#[test]
fn material_specialized_world_shaders_validate() {
    use super::MaterialFragmentKey;
    // Plain lightmapped wall, alpha-tested mask with vertex alpha, and an
    // rgbGen wave + tcGen environment transparent stage.
    let keys = [
        (MaterialFragmentKey::from_material([0, 0, 2 | 4 | 16777216 | 33554432, 0], 0.0), false),
        (MaterialFragmentKey::from_material([0, 1, 1 | 1048576 | 1073741824, 0], 0.5), true),
        (MaterialFragmentKey::from_material([3, 2, 134217728 | 268435456 | 536870912, 4], -0.5), true),
    ];
    for (name, base) in [
        ("bsp_lean", include_str!("../../bsp_lean.wgsl")),
        ("bsp", include_str!("../../bsp.wgsl")),
    ] {
        let source = compose_world_shader(base);
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name} validates: {e:?}"));
        for &(key, alpha_test) in &keys {
            let key = key.for_variant(minimal_key(), alpha_test);
            let constants: naga::back::PipelineConstants = minimal_key()
                .compilation_constants()
                .into_iter()
                .chain([("ENABLE_ALPHA_TEST", f64::from(u8::from(alpha_test)))])
                .chain(key.compilation_constants())
                .map(|(name, value)| (name.to_owned(), value))
                .collect();
            let minimal: &[(ShaderStage, &str)] = if name == "bsp_lean" {
                &[(ShaderStage::Fragment, "fs_world_minimal"), (ShaderStage::Vertex, "vs_world_minimal")]
            } else {
                &[]
            };
            for &(stage, entry) in [
                (ShaderStage::Fragment, "fs_main"),
                (ShaderStage::Fragment, "fs_main_legacy_fog"),
                (ShaderStage::Fragment, "fs_legacy_srgb_three_stage"),
                (ShaderStage::Vertex, "vs_main"),
            ].iter().chain(minimal) {
                naga::back::pipeline_constants::process_overrides(
                    &module, &info, Some((stage, entry)), &constants,
                )
                .unwrap_or_else(|e| panic!("{name} {entry} {key:?}: {e:?}"));
            }
        }
    }
}

#[test]
fn material_fragment_key_narrows_only_compiled_out_bits() {
    use super::MaterialFragmentKey;
    let pbr_normal_map = 8;
    let planar = 4096;
    let full = MaterialFragmentKey::from_material([0, 0, 2 | pbr_normal_map | planar, 2 | 4], 0.25);
    assert_eq!(full.alpha_func, 2);
    let minimal = full.for_variant(minimal_key(), true);
    // Bits that only feed compiled-out code leave both the value and the mask, so
    // they are read from the uniform rather than fixed to a wrong constant.
    assert_eq!(minimal.flags_mask & (pbr_normal_map | planar), 0);
    assert_eq!(minimal.flags & (pbr_normal_map | planar), 0);
    assert_eq!(minimal.flags_w_mask & (2 | 4), 0);
    assert_eq!(minimal.flags & 2, 2);
    assert_eq!(minimal.alpha_func, 2);
    let mut pbr = minimal_key();
    pbr.pbr = true;
    assert_eq!(full.for_variant(pbr, true).flags & pbr_normal_map, pbr_normal_map);
    // Opaque pipelines are compiled without the alpha test.
    assert_eq!(full.for_variant(minimal_key(), false).alpha_func, 0);
    // TaystJK-style classification: on Minimal only the live stage bits split
    // PSOs; tcGen stays dynamic unless static AO or planar reads it.
    let wall = MaterialFragmentKey::from_material([0, 0, 2 | 4 | 16777216 | 33554432 | 131072, 0], 0.0)
        .for_variant(minimal_key(), false);
    assert_eq!(wall.flags, 2);
    assert_eq!(wall.tc_gen, u32::MAX);
    let mut ao = minimal_key();
    ao.static_bsp_ao = true;
    let wall_ao = MaterialFragmentKey::from_material([1, 0, 2 | 131072, 0], 0.0).for_variant(ao, false);
    assert_eq!((wall_ao.flags, wall_ao.tc_gen), (2 | 131072, 1));
    assert_eq!(MaterialFragmentKey::from_material([0; 4], -0.5).alpha_func, 3);
    assert_eq!(MaterialFragmentKey::from_material([0; 4], 0.0).alpha_func, 1);
}

/// Exercise the actual WGSL inverse/subtraction on a GPU, with known compiler
/// photon values. This catches encoded-space subtraction and color-normalization
/// mistakes that shader parsing alone cannot detect.
#[test]
#[ignore = "requires a Vulkan GPU"]
fn authored_baked_sun_removes_known_photons_on_gpu() {
    let full = include_str!("../../bsp.wgsl");
    let start = full.find("fn baked_file_to_linear(").unwrap();
    let end = full.find("fn remove_baked_sun(").unwrap();
    let helpers = &full[start..end];
    let encode = |raw: [f32; 3], gamma: f32, compensate: f32, srgb: bool| -> [f32; 3] {
        let g = raw.map(|v| (v / 255.0).powf(1.0 / gamma) * 255.0);
        let peak = g.into_iter().fold(0.0_f32, f32::max);
        g.map(|v| {
            let v = v / peak * (1.0 - (-peak / 260.0).exp()) / compensate;
            if !srgb { v } else if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        })
    };
    let flux = 48.0_f32.to_radians().sin();
    let direct = 900.0 * flux;
    let mut expected = Vec::new();
    let mut cases = String::new();
    for (index, (baseline, strength, visibility, gamma, compensate, srgb)) in [
        ([100.0; 3], 0.0, flux, 1.0, 1.0, true),
        ([100.0; 3], 1.0, flux, 1.0, 1.0, true),
        ([100.0; 3], 1.0, 0.0, 1.0, 1.0, true),
        ([100.0; 3], 0.5, flux, 1.0, 1.0, true),
        ([120.0, 180.0, 90.0], 1.0, flux, 1.0, 1.0, true),
        ([120.0, 180.0, 90.0], 1.0, flux, 2.2, 1.0, true),
        ([100.0; 3], 1.0, flux, 1.0, 2.0, true),
        ([100.0; 3], 1.0, flux, 1.0, 1.0, false),
    ].into_iter().enumerate() {
        let sunlight = if visibility > 0.0 { direct } else { 0.0 };
        let input = encode(baseline.map(|v| v + sunlight), gamma, compensate, srgb);
        expected.push(encode(baseline.map(|v| v + sunlight * (1.0 - strength)), gamma, compensate, srgb));
        let signed_compensate = if srgb { compensate } else { -compensate };
        cases.push_str(&format!("if (id.x == {index}u) {{ lighting_settings.baked_sun_direction_strength.w = {strength:?}; input.baked_sun_fit = vec4<f32>(260.0, {gamma:?}, 4.0, {signed_compensate:?}); output[{index}] = vec4<f32>(subtract_authored_baked_sun(vec3<f32>({:?}, {:?}, {:?}), {visibility:?}, input), 1.0); }}\n", input[0], input[1], input[2]));
    }
    let boundary = encode([100.0; 3], 1.0, 1.0, true);
    expected.push(boundary);
    cases.push_str("if (id.x == 8u) { lighting_settings.baked_sun_direction_strength.w = 1.0; input.baked_sun_fit = vec4<f32>(260.0, 1.0, 4.0, 1.0); input.lightmap_uv = vec2<f32>(0.5); output[8] = vec4<f32>(sample_authored_baked_sun(vec3<f32>(0.0), input), 1.0); }\n");
    // Retail maps such as ffa3 omit compiler metadata. Exercise the standard
    // zero-exposure transform using its authored colored sun, without clipping.
    let default_sun = [0.874291 * 175.0, 175.0, 0.819403 * 175.0];
    for (strength, visibility, gamma, compensate, srgb) in [
        (0.0, flux, 1.0, 1.0, true),
        (1.0, flux, 1.0, 1.0, true),
        (0.5, flux, 1.0, 1.0, true),
        (1.0, 0.0, 1.0, 1.0, true),
        (1.0, flux, 2.2, 2.0, true),
        (1.0, flux, 1.0, 1.0, false),
    ] {
        let encode_default = |raw: [f32; 3]| {
            let corrected = raw.map(|v| (v / 255.0).powf(1.0 / gamma));
            let peak = corrected.into_iter().fold(1.0_f32, f32::max);
            corrected.map(|v| {
                let v = v / peak / compensate;
                if !srgb { v } else if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            })
        };
        let baseline = [40.0, 60.0, 30.0];
        let input = encode_default(std::array::from_fn(|c| baseline[c] + default_sun[c] * visibility));
        let index = expected.len();
        expected.push(encode_default(std::array::from_fn(|c| baseline[c] + default_sun[c] * visibility * (1.0 - strength))));
        let signed_compensate = if srgb { compensate } else { -compensate };
        cases.push_str(&format!("if (id.x == {index}u) {{ lighting_settings.baked_sun_radiance = vec4<f32>({:?}, {:?}, {:?}, 0.0); lighting_settings.baked_sun_direction_strength.w = {strength:?}; input.baked_sun_fit = vec4<f32>(0.0, {gamma:?}, 4.0, {signed_compensate:?}); output[{index}] = vec4<f32>(subtract_authored_baked_sun(vec3<f32>({:?}, {:?}, {:?}), {visibility:?}, input), 1.0); }}\n", default_sun[0] / 255.0, default_sun[1] / 255.0, default_sun[2] / 255.0, input[0], input[1], input[2]));
    }
    let live_start = full.find("fn light_bsp_surface(").unwrap();
    let live_end = full[live_start..].find("fn shade_surface(").unwrap() + live_start;
    let live_helper = &full[live_start..live_end];
    for (strength, visibility, facing, enabled, gain) in [
        (0.0, 1.0, true, true, 1.0),
        (1.0, 1.0, true, true, 1.0),
        (1.0, 0.0, true, true, 1.0),
        (1.0, 1.0, false, true, 1.0),
        (1.0, 1.0, true, true, 2.0),
        (1.0, 1.0, true, false, 1.0),
    ] {
        let index = expected.len();
        let baseline = [0.1, 0.15, 0.05];
        let radiance = [0.4, 0.2, 0.1];
        expected.push(std::array::from_fn(|c| baseline[c] + if enabled && facing { radiance[c] * gain * visibility } else { 0.0 }));
        let direction = if facing { "0.0, 1.0, 0.0" } else { "1.0, 0.0, 0.0" };
        let enabled = if enabled { 1.0 } else { 0.0 };
        cases.push_str(&format!("if (id.x == {index}u) {{ input.world_normal = vec3<f32>(0.0, 1.0, 0.0); lighting_settings.baked_sun_direction_strength.w = {strength:?}; lighting_settings.runtime_sun_direction = vec4<f32>({direction}, 0.0); lighting_settings.runtime_sun_radiance = vec4<f32>({:?}, {:?}, {:?}, {enabled:?}); test_visibility = {visibility:?}; output[{index}] = vec4<f32>(light_bsp_surface(vec3<f32>(0.1, 0.15, 0.05), 0.0, input), 1.0); }}\n", radiance[0] * gain, radiance[1] * gain, radiance[2] * gain));
    }
    // Explicit runtime sun removes its direct contribution. Legacy alpha
    // already contains user-controlled moving-shadow contrast.
    for (enabled,visibility) in [(true,0.0f32),(true,0.5),(true,1.0),(false,0.0),(false,1.0)] {
        let index=expected.len();let flux=if enabled {1.0}else{0.3};
        let rgb=if enabled {[1.6,1.4,1.2]}else{[1.0;3]};
        expected.push(std::array::from_fn(|c|if enabled {rgb[c]-[0.6,0.4,0.2][c]*flux*(1.0-visibility)}else{rgb[c]*(1.0-flux*(1.0-visibility))}));
        cases.push_str(&format!("if (id.x == {index}u) {{ lighting_settings.baked_sun_debug_mode.z=1u; lighting_settings.runtime_sun_radiance=vec4<f32>(0.6,0.4,0.2,{:?}); test_visibility={visibility:?}; output[{index}]=vec4<f32>(light_bsp_surface(vec3<f32>({:?},{:?},{:?}),{flux:?},input),1.0); }}\n",if enabled {1.0}else{0.0},rgb[0],rgb[1],rgb[2]));
    }
    for strength in [0.0_f32, 0.25, 1.0] {
        let index = expected.len();
        expected.push([1.6 - 0.6 * strength, 1.4 - 0.4 * strength, 1.2 - 0.2 * strength]);
        cases.push_str(&format!("if (id.x == {index}u) {{ lighting_settings.baked_sun_debug_mode.z=1u; lighting_settings.runtime_sun_radiance=vec4<f32>(0.6,0.4,0.2,1.0); shadow_settings.params.z={strength:?}; test_visibility=0.0; output[{index}]=vec4<f32>(light_bsp_surface(vec3<f32>(1.6,1.4,1.2),1.0,input),1.0); }}\n"));
    }
    let composed=crate::renderer::rt_models::declaration(include_str!("../lighting/lightmap_sun.wgsl"),"fn shade_composed_sun(").unwrap();
    // Run the actual explicit-stage alpha handling and relight call from both
    // world families. A filter lightmap must match a folded base*lightmap,
    // including zero/partial sunlight, while still supplying opaque blend alpha.
    let mut explicit_helpers = String::new();
    for (family, shader) in [("full", full), ("lean", include_str!("../../bsp_lean.wgsl"))] {
        let alpha_start = shader.find("// Lightmap alpha carries internal sun visibility.").unwrap();
        let alpha_end = alpha_start + shader[alpha_start..].find("// Classic id Tech 3").unwrap();
        let relight_start = shader.find("let relight_explicit =").unwrap();
        let relight_end = relight_start + shader[relight_start..].find("var base =").unwrap();
        explicit_helpers.push_str(&format!(r#"
fn explicit_{family}(sample: vec4<f32>, input: VertexOut) -> vec4<f32> {{
    var source_sample = sample;
    let explicit_lightmap_stage = true;
    let classic_flags = 0u;
    var stage_color = vec4<f32>(1.0);
    {}
    {}
    return source_sample;
}}
"#, &shader[alpha_start..alpha_end], &shader[relight_start..relight_end]));
        for flux in [0.0_f32, 0.3, 0.75, 1.0] {
            for visibility in [0.0_f32, 0.5, 1.0] {
                for enabled in [false, true] {
                    let rgb = [1.6_f32, 1.4, 1.2];
                    let wanted = std::array::from_fn(|c| if enabled {
                        rgb[c] - [0.6, 0.4, 0.2][c] * flux * (1.0 - visibility)
                    } else { rgb[c] * (1.0 - flux * (1.0 - visibility)) });
                    for explicit in [false, true] {
                        let index = expected.len();
                        expected.push(wanted);
                        let result = if explicit {
                            format!("explicit_{family}(vec4<f32>(1.6,1.4,1.2,{flux:?}), input)")
                        } else {
                            format!("vec4<f32>(light_bsp_surface(vec3<f32>(1.6,1.4,1.2),{flux:?},input),1.0)")
                        };
                        cases.push_str(&format!("if (id.x == {index}u) {{ lighting_settings.baked_sun_debug_mode.z=1u; lighting_settings.runtime_sun_radiance=vec4<f32>(0.6,0.4,0.2,{:?}); test_visibility={visibility:?}; output[{index}]={result}; }}\n", if enabled {1.0} else {0.0}));
                    }
                }
            }
        }
    }
    let live_helper=format!("{live_helper}\n{composed}\n{explicit_helpers}");
    let source = format!("{}\n{}\n{}\n{}", r#"
struct Settings { baked_sun_direction_strength: vec4<f32>, baked_sun_radiance: vec4<f32>, baked_sun_debug_mode: vec4<u32>, runtime_sun_direction: vec4<f32>, runtime_sun_radiance: vec4<f32> }
var<private> lighting_settings: Settings;
var<private> test_visibility: f32;
struct ShadowSettings { params: vec4<f32> }
var<private> shadow_settings: ShadowSettings;
struct VertexOut { baked_sun_fit: vec4<f32>, world_normal: vec3<f32>, lightmap_uv: vec2<f32>, color: vec4<f32> }
const ENABLE_BAKED_SUN_RELIGHT: bool = true;
struct Material { color: vec4<f32> }
var<private> material: Material;
fn material_flags() -> u32 { return 0u; }
@group(0) @binding(0) var<storage, read_write> output: array<vec4<f32>>;
@group(0) @binding(1) var lightmap_texture: texture_2d<f32>;
fn baked_luminance(rgb: vec3<f32>) -> f32 { return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722)); }
fn baked_indirect_tone(rgb: vec3<f32>) -> vec3<f32> { return rgb; }
fn remove_baked_sun(rgb: vec3<f32>, flux: f32, input: VertexOut) -> vec3<f32> { return rgb; }
fn sun_shadow_transmission(input: VertexOut, normal: vec3<f32>) -> vec3<f32> { return vec3<f32>(test_visibility); }
fn static_sun_lightmap_receiver(input: VertexOut) -> bool { return lighting_settings.baked_sun_debug_mode.z != 0u; }
fn lightmap_moving_sun_visibility(input: VertexOut) -> f32 { return test_visibility; }
fn composed_lightmap_receiver(input: VertexOut) -> bool { return static_sun_lightmap_receiver(input); }
fn baked_local_total(input: VertexOut) -> vec3<f32> { return vec3(0.0); }
fn shade_baked_local(lightmap: vec3<f32>, input: VertexOut) -> vec3<f32> { return lightmap; }

"#, helpers, live_helper, format!("@compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{ var input: VertexOut; shadow_settings.params.z = 1.0; lighting_settings.baked_sun_radiance = vec4<f32>(900.0 / 255.0, 900.0 / 255.0, 900.0 / 255.0, 0.0); {cases} }}"));
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::VULKAN, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.expect("sun subtraction GPU adapter");
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("actual authored-sun subtraction"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &shader, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
        let bytes = expected.len() as u64 * 16;
        let output = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: bytes, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("baked shadow boundary fixture"), size: wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
        });
        let to_byte = |raw: f32| ((1.0 - (-raw / 260.0).exp()) * 255.0).round() as u8;
        let lit = to_byte(100.0 + direct); let shadow = to_byte(100.0);
        let flux_byte = (flux * 255.0).round() as u8;
        let texels = [lit, lit, lit, flux_byte, shadow, shadow, shadow, 0, lit, lit, lit, flux_byte, shadow, shadow, shadow, 0];
        queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &texels, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8), rows_per_image: Some(2) }, wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 1 });
        let view = texture.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: output.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
        ] });
        let mut encoder = device.create_command_encoder(&Default::default());
        { let mut pass = encoder.begin_compute_pass(&Default::default()); pass.set_pipeline(&pipeline); pass.set_bind_group(0, &group, &[]); pass.dispatch_workgroups(expected.len() as u32, 1, 1); }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
        queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |result| result.unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        let data = readback.slice(..).get_mapped_range().unwrap();
        let values: &[[f32; 4]] = bytemuck::cast_slice(&data);
        for (index, (actual, expected)) in values.iter().zip(expected).enumerate() {
            assert_eq!(actual[3], 1.0, "sun case {index}: internal flux must not leak into blend alpha");
            let tolerance = if index == 8 { 0.01 } else { 0.0001 };
            for channel in 0..3 { assert!((actual[channel] - expected[channel]).abs() < tolerance, "sun case {index} channel {channel}: {} != {}", actual[channel], expected[channel]); }
        }
    });
}
