//! Diagnostic: where an RT world PSO's compile time goes (naga vs native driver),
//! how many ray-query traversals the fragment inlines, and which RT features
//! drive the native cost. Windowless Vulkan; run with --ignored --nocapture.
use super::*;
use crate::renderer::{GpuVertex, MaterialFragmentKey, WorldShaderVariantKey, DEPTH_FORMAT, VERTEX_ATTRIBUTES};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Instant;
use wgpu::naga;

fn hybrid_variant() -> WorldShaderVariantKey {
    WorldShaderVariantKey {
        point_lights: true,
        local_shadows: true,
        cascaded_shadows: true,
        ray_traced_shadows: true,
        ray_traced_sun: true,
        rt_receiver_glass: true,
        material_specialization: true,
        ..WorldShaderVariantKey::default()
    }
}

/// The composed RT world source. `nonce` perturbs one ray tmin literal so the
/// SPIR-V differs per run and the NVIDIA driver's own shader cache misses.
fn rt_source(nonce: u32) -> String {
    let source = ray_traced_world_shader_source(include_str!("../../bsp.wgsl")).unwrap();
    let source = compose_world_shader(&source);
    let tmin = format!("{:.7}", 0.25 + f64::from(nonce % 997 + 1) * 1e-6);
    source.replace("RayDesc(ray_flags, instance_mask, 0.25,", &format!("RayDesc(ray_flags, instance_mask, {tmin},"))
}

fn raster_source() -> String {
    compose_world_shader(include_str!("../../bsp.wgsl"))
}

fn constants(variant: WorldShaderVariantKey, material: Option<MaterialFragmentKey>) -> Vec<(&'static str, f64)> {
    let mut constants = variant.compilation_constants();
    constants.push(("ENABLE_ALPHA_TEST", 0.0));
    if let Some(material) = material {
        constants.extend(material.compilation_constants());
    }
    constants
}

fn const_bool(module: &naga::Module, exprs: &naga::Arena<naga::Expression>, h: naga::Handle<naga::Expression>) -> Option<bool> {
    match exprs[h] {
        naga::Expression::Literal(naga::Literal::Bool(b)) => Some(b),
        naga::Expression::Constant(c) => match module.global_expressions[module.constants[c].init] {
            naga::Expression::Literal(naga::Literal::Bool(b)) => Some(b),
            _ => None,
        },
        _ => None,
    }
}

/// Ray-query traversals the driver must inline into the entry point: every live
/// call site counts its callee's traversals again (loops count once).
fn inlined_traversals(module: &naga::Module, entry: &naga::Function) -> u32 {
    fn block(module: &naga::Module, f: &naga::Function, b: &naga::Block, memo: &mut HashMap<naga::Handle<naga::Function>, u32>) -> u32 {
        let mut n = 0;
        for s in b.iter() {
            n += match s {
                naga::Statement::Block(inner) => block(module, f, inner, memo),
                naga::Statement::If { condition, accept, reject } => match const_bool(module, &f.expressions, *condition) {
                    Some(true) => block(module, f, accept, memo),
                    Some(false) => block(module, f, reject, memo),
                    None => block(module, f, accept, memo) + block(module, f, reject, memo),
                },
                naga::Statement::Switch { cases, .. } => cases.iter().map(|c| block(module, f, &c.body, memo)).sum(),
                naga::Statement::Loop { body, continuing, .. } => block(module, f, body, memo) + block(module, f, continuing, memo),
                naga::Statement::RayQuery { fun: naga::RayQueryFunction::Initialize { .. }, .. } => 1,
                naga::Statement::Call { function, .. } => function_count(module, *function, memo),
                _ => 0,
            };
        }
        n
    }
    fn function_count(module: &naga::Module, h: naga::Handle<naga::Function>, memo: &mut HashMap<naga::Handle<naga::Function>, u32>) -> u32 {
        if let Some(&n) = memo.get(&h) {
            return n;
        }
        let f = &module.functions[h];
        let n = block(module, f, &f.body, memo);
        memo.insert(h, n);
        n
    }
    block(module, entry, &entry.body, &mut HashMap::new())
}

struct NagaProbe {
    millis: f64,
    spirv_words: usize,
    traversals: u32,
}

fn naga_probe(module: &naga::Module, info: &naga::valid::ModuleInfo, constants: &[(&str, f64)]) -> NagaProbe {
    let started = Instant::now();
    let map = constants.iter().map(|&(k, v)| (k.to_string(), v)).collect::<naga::back::PipelineConstants>();
    let (processed, processed_info) = naga::back::pipeline_constants::process_overrides(
        module, info, Some((naga::ShaderStage::Fragment, "fs_main")), &map,
    ).unwrap();
    let spirv = naga::back::spv::write_vec(
        &processed, &processed_info, &naga::back::spv::Options::default(),
        Some(&naga::back::spv::PipelineOptions { shader_stage: naga::ShaderStage::Fragment, entry_point: "fs_main".into() }),
    ).unwrap();
    let millis = started.elapsed().as_secs_f64() * 1000.0;
    let entry = processed.entry_points.iter().find(|e| e.name == "fs_main").unwrap();
    NagaProbe { millis, spirv_words: spirv.len(), traversals: inlined_traversals(&processed, &entry.function) }
}

fn parse(source: &str) -> (naga::Module, naga::valid::ModuleInfo) {
    let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module).unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    (module, info)
}

fn opaque_lightmapped_material(variant: WorldShaderVariantKey) -> MaterialFragmentKey {
    // Implicit lightmap + dynamic receiver + has-lightmap + opaque stage.
    MaterialFragmentKey::from_material([0, 0, 2 | 4 | 16777216 | 33554432, 0], 0.0).for_variant(variant, false)
}

fn vertex_lit_material(variant: WorldShaderVariantKey) -> MaterialFragmentKey {
    MaterialFragmentKey::from_material([0, 0, 1 | 4 | 131072 | 33554432, 0], 0.0).for_variant(variant, false)
}

#[test]
fn rt_world_fragment_inlines_traversals_offline() {
    let source = rt_source(0);
    let started = Instant::now();
    let (module, info) = parse(&source);
    println!("RT enhanced: {} bytes WGSL, parse+validate {:.0} ms", source.len(), started.elapsed().as_secs_f64() * 1000.0);
    let variant = hybrid_variant();
    let cases: Vec<(&str, WorldShaderVariantKey, Option<MaterialFragmentKey>)> = vec![
        ("hybrid general", variant, None),
        ("hybrid lightmapped", variant, Some(opaque_lightmapped_material(variant))),
        ("hybrid vertex-lit", variant, Some(vertex_lit_material(variant))),
        ("rt local only", WorldShaderVariantKey { ray_traced_sun: false, ..variant }, None),
        ("rt sun only (no point lights)", WorldShaderVariantKey { point_lights: false, local_shadows: false, ..variant }, None),
    ];
    for (name, v, material) in cases {
        let probe = naga_probe(&module, &info, &constants(v, material));
        println!("{name:32} naga overrides+spv {:7.1} ms  spirv {:7} words  inlined traversals {}",
            probe.millis, probe.spirv_words, probe.traversals);
    }
    // Per-function breakdown: traversals reachable from one call of each function,
    // and how many live call sites to it the entry point inlines.
    let map = constants(variant, Some(opaque_lightmapped_material(variant))).iter()
        .map(|&(k, v)| (k.to_string(), v)).collect::<naga::back::PipelineConstants>();
    let (processed, _) = naga::back::pipeline_constants::process_overrides(
        &module, &info, Some((naga::ShaderStage::Fragment, "fs_main")), &map).unwrap();
    let mut rows = Vec::new();
    for (h, f) in processed.functions.iter() {
        let one = inlined_traversals(&processed, f);
        if one > 0 {
            let mut callers = 0;
            for (_, g) in processed.functions.iter() {
                callers += count_calls(&processed, g, &g.body, h);
            }
            let entry = &processed.entry_points.iter().find(|e| e.name == "fs_main").unwrap().function;
            callers += count_calls(&processed, entry, &entry.body, h);
            rows.push((one, callers, f.name.clone().unwrap_or_default()));
        }
    }
    rows.sort();
    for (one, callers, name) in rows {
        println!("  {name:44} traversals/call {one:3}  live call sites {callers}");
    }
}

fn count_calls(module: &naga::Module, f: &naga::Function, b: &naga::Block, target: naga::Handle<naga::Function>) -> u32 {
    b.iter().map(|s| match s {
        naga::Statement::Block(inner) => count_calls(module, f, inner, target),
        naga::Statement::If { condition, accept, reject } => match const_bool(module, &f.expressions, *condition) {
            Some(true) => count_calls(module, f, accept, target),
            Some(false) => count_calls(module, f, reject, target),
            None => count_calls(module, f, accept, target) + count_calls(module, f, reject, target),
        },
        naga::Statement::Switch { cases, .. } => cases.iter().map(|c| count_calls(module, f, &c.body, target)).sum(),
        naga::Statement::Loop { body, continuing, .. } => count_calls(module, f, body, target) + count_calls(module, f, continuing, target),
        naga::Statement::Call { function, .. } => u32::from(*function == target),
        _ => 0,
    }).sum()
}

#[test]
#[ignore = "requires a Vulkan GPU with hardware ray queries; offscreen, no game window"]
fn rt_world_pipeline_compile_probe() {
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.unwrap();
        println!("GPU: {:?}", adapter.get_info());
        let (device, _queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY
                | (adapter.features() & wgpu::Features::all_native_mask()),
            required_limits: adapter.limits(),
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        }).await.unwrap();
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as u32;
        let variant = hybrid_variant();
        let raster = WorldShaderVariantKey { ray_traced_shadows: false, ray_traced_sun: false, rt_receiver_glass: false, ..variant };
        let lit = Some(opaque_lightmapped_material(variant));
        let vlit = Some(vertex_lit_material(variant));
        // (label, source, trusted, variant, material)
        let mut cases: Vec<(String, String, bool, WorldShaderVariantKey, Option<MaterialFragmentKey>)> = Vec::new();
        let mut n = nonce;
        let mut next = || { n = n.wrapping_add(1); n };
        cases.push(("raster CSM general".into(), raster_source(), false, raster, None));
        cases.push(("raster CSM lightmapped".into(), raster_source(), false, raster, lit));
        cases.push(("RT hybrid general".into(), rt_source(next()), false, variant, None));
        cases.push(("RT hybrid lightmapped".into(), rt_source(next()), false, variant, lit));
        cases.push(("RT hybrid vertex-lit".into(), rt_source(next()), false, variant, vlit));
        let warm = rt_source(next());
        cases.push(("RT hybrid lightmapped (cold)".into(), warm.clone(), false, variant, lit));
        cases.push(("RT hybrid lightmapped (warm)".into(), warm, false, variant, lit));
        cases.push(("RT hybrid lightmapped UNCHECKED".into(), rt_source(next()), true, variant, lit));
        cases.push(("RT local only".into(), rt_source(next()), false, WorldShaderVariantKey { ray_traced_sun: false, ..variant }, lit));
        cases.push(("RT sun only".into(), rt_source(next()), false, WorldShaderVariantKey { point_lights: false, local_shadows: false, ..variant }, lit));
        let parsed = std::cell::RefCell::new(HashMap::<String, (naga::Module, naga::valid::ModuleInfo)>::new());
        for (label, source, trusted, v, material) in cases {
            let constants = constants(v, material);
            let naga_ms = {
                let mut cache = parsed.borrow_mut();
                let (module, info) = cache.entry(source.clone()).or_insert_with(|| parse(&source));
                naga_probe(module, info, &constants)
            };
            let module_started = Instant::now();
            let descriptor = wgpu::ShaderModuleDescriptor { label: Some("rt probe"), source: wgpu::ShaderSource::Wgsl(source.clone().into()) };
            let module = if trusted {
                unsafe { device.create_shader_module_trusted(descriptor, wgpu::ShaderRuntimeChecks::unchecked()) }
            } else {
                device.create_shader_module(descriptor)
            };
            let module_ms = module_started.elapsed().as_secs_f64() * 1000.0;
            let vertex_constants = constants.iter().copied()
                .filter(|(name, _)| !matches!(*name, "ENABLE_RAY_TRACED_SHADOWS" | "ENABLE_RAY_TRACED_SUN"))
                .collect::<Vec<_>>();
            let buffers = [Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            })];
            let started = Instant::now();
            let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rt probe"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module, entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &vertex_constants, ..Default::default() },
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT, depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(), bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module, entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            });
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            println!("{label:34} pipeline {total_ms:8.1} ms (naga fs {:6.1} ms, module {module_ms:5.1} ms) spirv {:6} words, inlined traversals {}",
                naga_ms.millis, naga_ms.spirv_words, naga_ms.traversals);
        }
    });
}

#[test]
#[ignore = "requires a Vulkan GPU with hardware ray queries; offscreen, no game window"]
fn rt_world_pipeline_levers_probe() {
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.unwrap();
        let (device, _queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY | wgpu::Features::PIPELINE_CACHE
                | (adapter.features() & wgpu::Features::all_native_mask()),
            required_limits: adapter.limits(),
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        }).await.unwrap();
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as u32;
        let variant = hybrid_variant();
        let lit = Some(opaque_lightmapped_material(variant));
        let compile = |source: &str, v: WorldShaderVariantKey, material: Option<MaterialFragmentKey>,
                       cull: Option<wgpu::Face>, cache: Option<&wgpu::PipelineCache>| -> f64 {
            let constants = constants(v, material);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("rt probe"), source: wgpu::ShaderSource::Wgsl(source.to_string().into()) });
            let vertex_constants = constants.iter().copied()
                .filter(|(name, _)| !matches!(*name, "ENABLE_RAY_TRACED_SHADOWS" | "ENABLE_RAY_TRACED_SUN"))
                .collect::<Vec<_>>();
            let buffers = [Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            })];
            let started = Instant::now();
            let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rt probe"), layout: None,
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &vertex_constants, ..Default::default() },
                    buffers: &buffers },
                primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None, cache,
            });
            started.elapsed().as_secs_f64() * 1000.0
        };
        let nonce_glass_off = |nonce: u32| {
            let source = ray_traced_world_shader_source_with_features(include_str!("../../bsp.wgsl"), ReceiverFeatures { glass: false }).unwrap();
            let tmin = format!("{:.7}", 0.25 + f64::from(nonce % 997 + 1) * 1e-6);
            compose_world_shader(&source).replace("RayDesc(ray_flags, instance_mask, 0.25,", &format!("RayDesc(ray_flags, instance_mask, {tmin},"))
        };
        // Warm the device/driver once so the first measurement is not an outlier.
        compile(&rt_source(seed), variant, lit, Some(wgpu::Face::Back), None);
        println!("glass on  (shipping)         {:8.1} ms", compile(&rt_source(seed + 1), variant, lit, Some(wgpu::Face::Back), None));
        println!("glass off (JKA_RT_RECEIVER_GLASS=0) {:8.1} ms", compile(&nonce_glass_off(seed + 2), variant, lit, Some(wgpu::Face::Back), None));
        // Same fragment program, different native state (cull): does the driver reuse the compiled shader?
        let same = rt_source(seed + 3);
        println!("same program, cull back      {:8.1} ms", compile(&same, variant, lit, Some(wgpu::Face::Back), None));
        println!("same program, cull none      {:8.1} ms", compile(&same, variant, lit, None, None));
        println!("same program, cull front     {:8.1} ms", compile(&same, variant, lit, Some(wgpu::Face::Front), None));
        // VkPipelineCache: entry size of one RT PSO and the warm hit.
        let cache = unsafe { device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor { label: None, data: None, fallback: true }) };
        let before = cache.get_data().map_or(0, |d| d.len());
        let cached = rt_source(seed + 4);
        let cold = compile(&cached, variant, lit, Some(wgpu::Face::Back), Some(&cache));
        let after = cache.get_data().map_or(0, |d| d.len());
        let warm = compile(&cached, variant, lit, Some(wgpu::Face::Back), Some(&cache));
        let raster_before = after;
        compile(&raster_source(), WorldShaderVariantKey { ray_traced_shadows: false, ray_traced_sun: false, rt_receiver_glass: false, ..variant }, lit, Some(wgpu::Face::Back), Some(&cache));
        let raster_after = cache.get_data().map_or(0, |d| d.len());
        println!("pipeline cache: RT cold {cold:.1} ms, warm {warm:.1} ms, RT entry {} KiB, raster entry {} KiB",
            (after - before) / 1024, (raster_after - raster_before) / 1024);
        // Parallel scaling: N distinct RT programs on N threads.
        for threads in [1usize, 2, 4] {
            let sources = (0..4).map(|i| rt_source(seed + 10 + threads as u32 * 10 + i)).collect::<Vec<_>>();
            let started = Instant::now();
            let next = std::sync::atomic::AtomicUsize::new(0);
            std::thread::scope(|scope| {
                for _ in 0..threads {
                    scope.spawn(|| loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(source) = sources.get(i) else { break };
                        compile(source, variant, lit, Some(wgpu::Face::Back), None);
                    });
                }
            });
            println!("4 RT PSOs on {threads} thread(s): {:8.1} ms wall", started.elapsed().as_secs_f64() * 1000.0);
        }
    });
}

/// Per-pipeline disk cache in the patched wgpu-hal: a cold RT PSO writes one
/// file, recreating it (fresh module, no wgpu cache object) is a warm hit.
/// Run alone: the HAL reads `DINURDOJK_VK_PIPELINE_CACHE` once per process.
#[test]
#[ignore = "requires a Vulkan GPU with hardware ray queries; set JKA_TEST_VK_CACHE_DIR to an empty directory"]
fn rt_world_pipeline_disk_cache_probe() {
    let dir = std::path::PathBuf::from(std::env::var_os("JKA_TEST_VK_CACHE_DIR").expect("JKA_TEST_VK_CACHE_DIR"));
    std::env::set_var("DINURDOJK_VK_PIPELINE_CACHE", &dir);
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.unwrap();
        let (device, _queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY
                | (adapter.features() & wgpu::Features::all_native_mask()),
            required_limits: adapter.limits(),
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        }).await.unwrap();
        let variant = hybrid_variant();
        let lit = Some(opaque_lightmapped_material(variant));
        let source = rt_source(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as u32);
        let files = || std::fs::read_dir(&dir).map_or(0, |entries| entries.filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "bin")).count());
        let before = files();
        let compile = |cull: Option<wgpu::Face>| {
            let constants = constants(variant, lit);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("rt cache probe"), source: wgpu::ShaderSource::Wgsl(source.clone().into()) });
            let vertex_constants = constants.iter().copied()
                .filter(|(name, _)| !matches!(*name, "ENABLE_RAY_TRACED_SHADOWS" | "ENABLE_RAY_TRACED_SUN"))
                .collect::<Vec<_>>();
            let buffers = [Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            })];
            let started = Instant::now();
            let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rt cache probe"), layout: None,
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &vertex_constants, ..Default::default() },
                    buffers: &buffers },
                primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None, cache: None,
            });
            started.elapsed().as_secs_f64() * 1000.0
        };
        let cold = compile(Some(wgpu::Face::Back));
        let after_cold = files();
        let warm = compile(Some(wgpu::Face::Back));
        let other_state = compile(None);
        println!("disk cache: cold {cold:.1} ms, warm {warm:.1} ms, other cull {other_state:.1} ms; files {before} -> {after_cold} -> {}", files());
        assert!(after_cold > before, "a cold pipeline must write its cache file");
        assert!(warm < cold / 10.0, "recreating the pipeline must hit its file");
        assert!(files() > after_cold, "different native state gets its own file");
    });
}

/// The user's 2026-10-24 config (mode 0 general PSO): PBR + deluxe specular,
/// Hybrid Ocean, rain-wet surfaces, detail textures, RT lights + hybrid sun.
fn user_variant() -> WorldShaderVariantKey {
    // From the in-game label: pbr+pom+pom-mip+local-shadow+sun-shadow+rt-shadow
    // +ocean+baked-sun+deluxe+spec+detail (no point lights, no wet surfaces).
    WorldShaderVariantKey {
        pbr: true,
        pom: true,
        pom_mip_aware: true,
        pbr_shared_material_eval: true,
        pbr_shared_tangent_frame: true,
        pbr_companion_sampler: true,
        deluxe: true,
        deluxe_specular: true,
        ocean: true,
        baked_sun_relight: true,
        detail_textures_enabled: true,
        point_lights: false,
        material_specialization: false,
        ..hybrid_variant()
    }
}

#[test]
#[ignore = "requires a Vulkan GPU with hardware ray queries; offscreen, no game window"]
fn rt_user_variant_feature_cost_probe() {
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.unwrap();
        let (device, _queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY | wgpu::Features::PIPELINE_CACHE
                | (adapter.features() & wgpu::Features::all_native_mask()),
            required_limits: adapter.limits(),
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        }).await.unwrap();
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as u32;
        let user = user_variant();
        let no_rt = WorldShaderVariantKey { ray_traced_shadows: false, ray_traced_sun: false, rt_receiver_glass: false, ..user };
        let cases: Vec<(&str, WorldShaderVariantKey, bool, Option<MaterialFragmentKey>)> = vec![
            ("user general (mode 0)", user, true, None),
            ("user, RT dyn lights only (CSM sun)", WorldShaderVariantKey { ray_traced_sun: false, ..user }, true, None),
            ("user, RT lights only, mode 1 LM PSO", WorldShaderVariantKey { ray_traced_sun: false, material_specialization: true, ..user }, true,
                Some(opaque_lightmapped_material(WorldShaderVariantKey { ray_traced_sun: false, material_specialization: true, ..user }))),
            ("user without POM", WorldShaderVariantKey { pom: false, pom_mip_aware: false, ..user }, true, None),
            ("user without baked-sun relight", WorldShaderVariantKey { baked_sun_relight: false, ..user }, true, None),
            ("user without POM and baked-sun", WorldShaderVariantKey { pom: false, pom_mip_aware: false, baked_sun_relight: false, ..user }, true, None),
            ("user without ocean", WorldShaderVariantKey { ocean: false, ..user }, true, None),
            ("user without PBR/POM/deluxe", WorldShaderVariantKey { pbr: false, pom: false, pom_mip_aware: false, pbr_shared_material_eval: false,
                pbr_shared_tangent_frame: false, pbr_companion_sampler: false, deluxe: false, deluxe_specular: false, ..user }, true, None),
            ("user, raster (no RT)", no_rt, false, None),
        ];
        let mut parsed: HashMap<bool, (naga::Module, naga::valid::ModuleInfo)> = HashMap::new();
        for (i, (label, v, rt, material)) in cases.into_iter().enumerate() {
            let source = if rt { rt_source(seed + i as u32) } else { raster_source() };
            let constants = constants(v, material);
            let probe = {
                let (module, info) = parsed.entry(rt).or_insert_with(|| parse(&source));
                naga_probe(module, info, &constants)
            };
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("user probe"), source: wgpu::ShaderSource::Wgsl(source.clone().into()) });
            let vertex_constants = constants.iter().copied()
                .filter(|(name, _)| !matches!(*name, "ENABLE_RAY_TRACED_SHADOWS" | "ENABLE_RAY_TRACED_SUN"))
                .collect::<Vec<_>>();
            let buffers = [Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            })];
            let cache = unsafe { device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor { label: None, data: None, fallback: true }) };
            let started = Instant::now();
            let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("user probe"), layout: None,
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &vertex_constants, ..Default::default() },
                    buffers: &buffers },
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None, cache: Some(&cache),
            });
            println!("{label:34} pipeline {:8.1} ms  inlined traversals <= {}  driver binary {} KiB",
                started.elapsed().as_secs_f64() * 1000.0, probe.traversals, cache.get_data().map_or(0, |d| d.len()) / 1024);
        }
    });
}
