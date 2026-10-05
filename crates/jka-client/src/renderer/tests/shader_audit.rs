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
        ray_traced_shadows: false,
        ray_traced_sun: false,
        planar_reflections: false,
        legacy_fog: false,
        jump_shade: false,
        ocean: false,
        weather_surface: false,
        classic_render_flags: false,
        static_bsp_ao: false,
        planar_debug: false,
        deluxe: false,
        deluxe_specular: false,
        detail_texture_mode: 0,
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
    let constants: naga::back::PipelineConstants = key
        .compilation_constants()
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    let (specialized, _) =
        naga::back::pipeline_constants::process_overrides(&module, &info, None, &constants)
            .expect("minimal key specializes");
    specialized.into_owned()
}

fn fast_module() -> Module {
    let source = format!(
        "{}\n{}",
        include_str!("../../bsp_fast.wgsl"),
        include_str!("../../surface_deformation.wgsl")
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
    (
        "shade_surface_deformation: surface_deformation.header",
        "Snowflow/footprint data is per surface; identical in the bsp_fast reference",
    ),
    (
        "shade_snowflow_state: surface_deformation.snowflow_params, surface_deformation.snowflow_params2",
        "same as bsp_fast",
    ),
    ("shade_stock_2d_footprints: surface_deformation.header", "same as bsp_fast"),
    (
        "shade_stock_2d_footprints: surface_deformation.header, surface_deformation.stamps",
        "same as bsp_fast",
    ),
    ("shade_stock_2d_footprints: surface_deformation.stamps", "same as bsp_fast"),
    ("snowflow_falloff: surface_deformation.snowflow_header", "same as bsp_fast"),
    ("snowflow_sample_uv: surface_deformation.snowflow_header", "same as bsp_fast"),
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
    const EXEMPT: &[&str] = &["ENABLE_CLOUD_SHADOWS"];
    let supplied: BTreeSet<&str> = minimal_key()
        .compilation_constants()
        .into_iter()
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
