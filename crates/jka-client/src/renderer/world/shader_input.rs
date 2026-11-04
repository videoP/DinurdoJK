//! World-stage input-format A/B, deliberately independent of fragment program mode.
//!
//! Mode 0 uses the existing WGSL modules and pipeline overrides unchanged.
//! Mode 1 resolves overrides per VS/FS entry point, compiles stage-specific
//! SPIR-V, and caches it by source + entry + resolved settings under
//! DinurdoJK/cache/world_spirv. Every emitted OR cached binary is round-trip
//! imported and validated via Naga BEFORE creating a wgpu module.
//! Mode 2 (native passthrough) is intentionally disabled after an observed
//! NVIDIA driver access violation triggered by unvalidated SPIR-V.
//!
//! This is an on-demand *persistent* SPIR-V cache, not a claim that every modded
//! shader has been precompiled into the executable. First use must parse WGSL.
//! RT shaders and non-BSP passes are not part of this experiment.
use crate::renderer::{MaterialFragmentKey, WorldShaderFamily, WorldShaderVariantKey};
use crate::renderer::world::programs::StageProgram;
use std::{borrow::Cow, collections::HashSet, path::{Path, PathBuf}, sync::{Mutex, OnceLock, atomic::{AtomicU64, Ordering}}};
use sha2::{Digest, Sha256};

static CACHE_ROOT: OnceLock<PathBuf> = OnceLock::new();
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
static WGSL_FALLBACKS: AtomicU64 = AtomicU64::new(0);
// A rejected variant never gets submitted to wgpu again during this session.
// Useful when many map surfaces share a shader with a Naga SPIR-V round-trip bug.
static REJECTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub(in crate::renderer) fn install_cache_root(base: &Path) {
    let _ = CACHE_ROOT.set(jka_assets::pk3::dinurdojk_cache_directory(base).join("world_spirv"));
}

fn source_for(variant: WorldShaderVariantKey, material: Option<MaterialFragmentKey>) -> String {
    if let Some(material) = material {
        if matches!(material.program, StageProgram::GeneratedBasic | StageProgram::GeneratedEnhanced) {
            return super::programs::generated_source(variant, material);
        }
    }
    // Generated/shared/minimal/specialized-lean programs are based on lean WGSL;
    // only the enhanced Specialized path uses bsp.wgsl.
    let enhanced = material.map_or(true, |m| m.program == StageProgram::Specialized)
        && variant.family() == WorldShaderFamily::Enhanced;
    if enhanced {
        ENHANCED_SOURCE.get_or_init(|| crate::renderer::compose_world_shader(include_str!("../../bsp.wgsl"))).clone()
    } else {
        LEAN_SOURCE.get_or_init(|| crate::renderer::compose_world_shader(include_str!("../../bsp_lean.wgsl"))).clone()
    }
}
static LEAN_SOURCE: OnceLock<String> = OnceLock::new();
static ENHANCED_SOURCE: OnceLock<String> = OnceLock::new();

fn cache_id(source: &str, stage: naga::ShaderStage, entry: &str, constants: &[(&str, f64)]) -> String {
    let mut sha = Sha256::new();
    // v2 invalidates previously cached, unverified SPIR-V from the crash build.
    // v3 also invalidates the Y-flipped binaries emitted by the original
    // conversion patch. They passed validation yet reversed world geometry.
    sha.update(b"dinurdojk/world-spv/naga30/ndc-y-up-v3\0");
    sha.update(source.as_bytes());
    sha.update([match stage { naga::ShaderStage::Vertex => 1, naga::ShaderStage::Fragment => 2, _ => 3 }]);
    sha.update(entry.as_bytes());
    let mut constants = constants.to_vec();
    constants.sort_unstable_by(|(name_a, _), (name_b, _)| name_a.cmp(name_b));
    for (name, value) in constants {
        sha.update(name.as_bytes());
        sha.update([0]);
        sha.update(value.to_bits().to_le_bytes());
    }
    sha.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn read_words(path: &Path) -> Option<Vec<u32>> {
    let bytes = std::fs::read(path).ok()?;
    // Trailing SHA-256 detects truncated/corrupted files before passing cached
    // bytes to the unsafe native passthrough API. The path already keys the
    // source, specialization values, stage and entry point.
    if bytes.len() < 52 || (bytes.len() - 32) % 4 != 0 { return None; }
    let (binary, checksum) = bytes.split_at(bytes.len() - 32);
    if Sha256::digest(binary).as_slice() != checksum { return None; }
    let words = binary.chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect::<Vec<_>>();
    (words.first() == Some(&0x0723_0203)).then_some(words)
}
fn persist_words(path: &Path, words: &[u32]) {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() { return; }
    }
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for &word in words { bytes.extend_from_slice(&word.to_le_bytes()); }
    let checksum = Sha256::digest(&bytes);
    bytes.extend_from_slice(&checksum);
    // Different compiler workers can race on identical output; a successful
    // atomic rename is sufficient. A crash never exposes a partial .spv.
    let thread_id = format!("{:?}", std::thread::current().id())
        .replace('(', "_").replace(')', "");
    let temporary = path.with_extension(format!("{thread_id}.tmp"));
    if std::fs::write(&temporary, &bytes).is_ok() {
        let _ = std::fs::rename(&temporary, path);
    }
    let _ = std::fs::remove_file(temporary);
}

fn compile_words(
    source: &str, stage: naga::ShaderStage, entry: &str,
    constants: &[(&str, f64)],
) -> Result<Vec<u32>, String> {
    let shader = naga::front::wgsl::parse_str(source)
        .map_err(|e| format!("WGSL parse: {}", e.emit_to_string(source)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()
    ).validate(&shader).map_err(|e| format!("WGSL validate: {e}"))?;
    let values: naga::back::PipelineConstants = constants.iter()
        .map(|(key, value)| ((*key).to_owned(), *value)).collect();
    // Crucial for passthrough: wgpu 30 cannot apply these WGSL overrides to a
    // native SPIR-V module. Bake the exact per-entry values into SPIR-V instead.
    let (shader, info) = naga::back::pipeline_constants::process_overrides(
        &shader, &info, Some((stage, entry)), &values,
    ).map_err(|e| format!("resolve override constants: {e}"))?;
    naga::back::spv::write_vec(
        &shader, &info, &spv_writer_options(),
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: stage, entry_point: entry.to_owned(),
        }),
    ).map_err(|e| format!("SPIR-V emission: {e}"))
}

/// WGPU 30's `ShaderSource::SpirV` imports with
/// `adjust_coordinate_space: false` (the NDC_Y_UP convention). Naga's
/// SPIR-V writer defaults to ADJUST_COORDINATE_SPACE, which flips position.y.
/// A converted vertex shader with that default flips triangle winding and
/// can make the BSP appear mirrored, upside down, or inside out.
fn spv_writer_options() -> naga::back::spv::Options<'static> {
    let mut options = naga::back::spv::Options::default();
    options.flags.remove(naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE);
    // Never invent descriptor bindings for a program that is not ready to
    // emit valid SPIR-V: compilation failure must use the WGSL fallback.
    options.fake_missing_bindings = false;
    options
}

/// Match WGPU 30's validated-SPIR-V importer exactly, including its NDC
/// convention. Using Naga's importer defaults here silently hides Y flips.
fn spv_import_options() -> naga::front::spv::Options {
    naga::front::spv::Options {
        adjust_coordinate_space: false,
        strict_capabilities: true,
        block_ctx_dump_prefix: None,
    }
}

/// Validate the actual SPIR-V output, not only the WGSL that produced it.
/// Naga's SPIR-V writer and importer are independent transformations. A valid
/// WGSL module can generate SPIR-V that the importer rejects (as seen in the
/// user's `Expression [831]` crash). In particular, a SHA-256 file checksum
/// cannot prove that cached bytecode is a valid shader.
fn validate_words(words: &[u32], stage: naga::ShaderStage, entry: &str) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words { bytes.extend_from_slice(&word.to_le_bytes()); }
    let imported = naga::front::spv::parse_u8_slice(
        &bytes, &spv_import_options(),
    ).map_err(|error| format!("SPIR-V re-import: {error}"))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all(),
    ).validate(&imported)
        .map_err(|error| format!("SPIR-V round-trip validation: {error}"))?;
    if !imported.entry_points.iter().any(|point| point.stage == stage && point.name == entry) {
        return Err(format!("SPIR-V round-trip missing entry {stage:?}::{entry}"));
    }
    Ok(())
}

fn rejected_ids() -> &'static Mutex<HashSet<String>> {
    REJECTED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn reject_stage(source: &str, stage: naga::ShaderStage, entry: &str, constants: &[(&str, f64)]) {
    let id = cache_id(source, stage, entry, constants);
    rejected_ids().lock().unwrap_or_else(|p| p.into_inner()).insert(id.clone());
    if let Some(root) = CACHE_ROOT.get() {
        let _ = std::fs::remove_file(root.join(format!("{id}.spv")));
    }
}

/// A converted shader can pass Naga validation but fail the pipeline's
/// layout/interface check in wgpu. Quarantine both stage binaries so other
/// surfaces don't repeatedly hit the same failing PSO combination.
#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn quarantine_pipeline(
    variant: WorldShaderVariantKey, material: Option<MaterialFragmentKey>,
    vertex_entry: &str, fragment_entry: &str,
    vertex_constants: &[(&str, f64)], fragment_constants: &[(&str, f64)],
    reason: &str,
) {
    let source = source_for(variant, material);
    reject_stage(&source, naga::ShaderStage::Vertex, vertex_entry, vertex_constants);
    reject_stage(&source, naga::ShaderStage::Fragment, fragment_entry, fragment_constants);
    let _ = fallback(reason);
}

fn stage_words(
    source: &str, stage: naga::ShaderStage, entry: &str, constants: &[(&str, f64)],
) -> Result<Vec<u32>, String> {
    let id = cache_id(source, stage, entry, constants);
    if rejected_ids().lock().unwrap_or_else(|p| p.into_inner()).contains(&id) {
        return Err(format!("previously rejected SPIR-V variant {}", &id[..12]));
    }
    let path = CACHE_ROOT.get().map(|root| root.join(format!("{id}.spv")));
    let result = (|| -> Result<Vec<u32>, String> {
        if let Some(words) = path.as_ref().and_then(|p| read_words(p)) {
            validate_words(&words, stage, entry)?;
            CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            return Ok(words);
        }
        let words = compile_words(source, stage, entry, constants)?;
        validate_words(&words, stage, entry)?;
        CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
        if let Some(ref path) = path { persist_words(path, &words); }
        Ok(words)
    })();
    if result.is_err() {
        // Never retry this invalid stage in the current session or keep its
        // disk copy. The next session may retry after a compiler upgrade.
        reject_stage(source, stage, entry, constants);
    }
    result.map_err(|error| format!("{stage:?}::{entry} SPIR-V [{}]: {error}", &id[..12]))
}

fn module(device: &wgpu::Device, words: Vec<u32>) -> Result<wgpu::ShaderModule, String> {
    // Catch any extra wgpu validation errors that the independent Naga
    // round-trip might miss, instead of triggering wgpu's fatal error handler.
    // The scope is per-thread and the module creation is already on the
    // background pipeline compiler, never in the frame draw loop.
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("DinurdoJK BSP SPIR-V round-trip validated"),
        source: wgpu::ShaderSource::SpirV(Cow::Owned(words)),
    });
    if let Some(error) = futures_lite::future::block_on(scope.pop()) {
        return Err(format!("wgpu SPIR-V module validation: {error}"));
    }
    Ok(module)
}

pub(in crate::renderer) struct StageModules {
    pub vertex: wgpu::ShaderModule,
    pub fragment: wgpu::ShaderModule,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn prepare(
    device: &wgpu::Device, variant: WorldShaderVariantKey,
    material: Option<MaterialFragmentKey>,
    vertex_entry: &str, fragment_entry: &str,
    vertex_constants: &[(&str, f64)], fragment_constants: &[(&str, f64)],
) -> Option<StageModules> {
    let mode = variant.shader_input_mode;
    if mode == 0 || variant.ray_traced_shadows { return None; }
    if mode != 1 {
        // Belt-and-suspenders guard against older UI/configs requesting 2.
        return fallback("unsafe native SPIR-V passthrough is disabled after a driver crash");
    }
    let source = source_for(variant, material);
    let compiled = (|| -> Result<StageModules, String> {
        let vertex = stage_words(&source, naga::ShaderStage::Vertex, vertex_entry, vertex_constants)?;
        let fragment = stage_words(&source, naga::ShaderStage::Fragment, fragment_entry, fragment_constants)?;
        let vertex_module = module(device, vertex).map_err(|error| {
            reject_stage(&source, naga::ShaderStage::Vertex, vertex_entry, vertex_constants);
            error
        })?;
        let fragment_module = module(device, fragment).map_err(|error| {
            reject_stage(&source, naga::ShaderStage::Fragment, fragment_entry, fragment_constants);
            error
        })?;
        Ok(StageModules { vertex: vertex_module, fragment: fragment_module })
    })();
    match compiled {
        Ok(modules) => Some(modules),
        Err(error) => fallback(&error),
    }
}

fn fallback(reason: &str) -> Option<StageModules> {
    let count = WGSL_FALLBACKS.fetch_add(1, Ordering::Relaxed) + 1;
    if count <= 4 || count.is_power_of_two() {
        eprintln!("[SHADER INPUT FALLBACK] using WGSL, count={count}, reason={reason}");
    }
    None
}

pub(in crate::renderer) fn counters() -> (u64, u64, u64) {
    (CACHE_HITS.load(Ordering::Relaxed), CACHE_MISSES.load(Ordering::Relaxed), WGSL_FALLBACKS.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wgsl_overrides_are_baked_into_spirv_per_entry() {
        let src = r#"
            override ENABLE_TEST: bool = false;
            @vertex fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
                return vec4<f32>(f32(vertex), 0.0, 0.0, 1.0);
            }
            @fragment fn fs_main() -> @location(0) vec4<f32> {
                if (ENABLE_TEST) { return vec4<f32>(1.0, 1.0, 1.0, 1.0); }
                return vec4<f32>(0.0, 0.0, 0.0, 1.0);
            }
        "#;
        let off = compile_words(src, naga::ShaderStage::Fragment, "fs_main", &[("ENABLE_TEST", 0.0)]).unwrap();
        let on = compile_words(src, naga::ShaderStage::Fragment, "fs_main", &[("ENABLE_TEST", 1.0)]).unwrap();
        assert_eq!(off.first(), Some(&0x0723_0203));
        assert_eq!(on.first(), Some(&0x0723_0203));
        assert_ne!(off, on, "specialization must change the emitted SPIR-V");
        let vertex = compile_words(src, naga::ShaderStage::Vertex, "vs_main", &[]).unwrap();
        assert_eq!(vertex.first(), Some(&0x0723_0203));
        // In contrast to the original patch, test both sides of the round-trip.
        validate_words(&off, naga::ShaderStage::Fragment, "fs_main").unwrap();
        validate_words(&on, naga::ShaderStage::Fragment, "fs_main").unwrap();
        validate_words(&vertex, naga::ShaderStage::Vertex, "vs_main").unwrap();
    }

    #[test]
    fn emitted_spirv_preserves_wgpu_ndc_y_up_coordinates() {
        // The writer's default flips Position.y, while WGPU 30 deliberately
        // does NOT flip Y when importing ShaderSource::SpirV. The options
        // must always agree; otherwise culling and geometry invert.
        assert!(!spv_writer_options().flags.contains(
            naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE
        ));
        assert!(!spv_import_options().adjust_coordinate_space);
        let source = r#"
            @vertex fn vs_main() -> @builtin(position) vec4<f32> {
                return vec4<f32>(0.0, 0.25, 0.0, 1.0);
            }
        "#;
        let words = compile_words(source, naga::ShaderStage::Vertex, "vs_main", &[]).unwrap();
        validate_words(&words, naga::ShaderStage::Vertex, "vs_main").unwrap();
    }

    #[test]
    fn corrupted_spirv_never_reaches_wgpu() {
        let invalid = [0x0723_0203u32, 0, 0, 0, 0];
        assert!(validate_words(&invalid, naga::ShaderStage::Fragment, "fs_main").is_err());
    }

    #[test]
    fn hybrid_generated_source_matches_existing_generator() {
        use crate::renderer::world::programs::{WorldProgramMode, StageProgram};
        let variant = WorldShaderVariantKey {
            program_mode: WorldProgramMode::Hybrid,
            shader_input_mode: 2,
            material_specialization: true,
            ..Default::default()
        };
        let generated = MaterialFragmentKey {
            tc_gen: 2,
            program: StageProgram::GeneratedBasic,
            ..Default::default()
        };
        assert_eq!(source_for(variant, Some(generated)),
            super::super::programs::generated_source(variant, generated));
        assert!(source_for(variant, Some(generated)).contains("fn vs_generated("));
        assert!(source_for(variant, Some(generated)).contains("fn fs_generated("));
    }

    #[test]
    fn cache_id_is_order_independent_but_source_and_overrides_are_not() {
        let a = cache_id("source", naga::ShaderStage::Fragment, "fs_main", &[("one", 1.0), ("two", 0.0)]);
        assert_eq!(a, cache_id("source", naga::ShaderStage::Fragment, "fs_main", &[("two", 0.0), ("one", 1.0)]));
        assert_ne!(a, cache_id("source", naga::ShaderStage::Fragment, "fs_main", &[("one", 0.0), ("two", 0.0)]));
        assert_ne!(a, cache_id("source", naga::ShaderStage::Vertex, "vs_main", &[("one", 1.0), ("two", 0.0)]));
    }
}
