//! World variants.
use crate::renderer::Hash;
use super::programs::{StageProgram, VertexRecipe, WorldProgramMode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::renderer) enum WorldRenderPath {
    /// Exact low-overhead renderer used by the known-fast pre-enhancement build.
    /// It has only camera + surface bind groups and records the simple direct
    /// draw loop without consulting advanced renderer state per draw.
    FastBaseline,
    #[default]
    Advanced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::renderer) enum WorldShaderFamily {
    Lean,
    Enhanced,
}

/// Cold-path key for a compiled BSP shader/pipeline family.  Unlike the runtime
/// settings uniforms, these flags are supplied as WGSL `override` constants at
/// pipeline creation time.  A setting change selects an existing cached variant
/// or creates it once; steady-state frames never compile pipelines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::renderer) struct WorldShaderVariantKey {
    pub(in crate::renderer) pbr: bool,
    pub(in crate::renderer) pom: bool,
    pub(in crate::renderer) pbr_shared_material_eval: bool,
    pub(in crate::renderer) pbr_shared_tangent_frame: bool,
    pub(in crate::renderer) pom_mip_aware: bool,
    pub(in crate::renderer) pom_adaptive_steps: bool,
    pub(in crate::renderer) pbr_companion_sampler: bool,
    pub(in crate::renderer) pbr_vertex_lightgrid: bool,
    pub(in crate::renderer) point_lights: bool,
    pub(in crate::renderer) map_light_simulation: bool,
    pub(in crate::renderer) source_map_world: bool,
    pub(in crate::renderer) legacy_dlights: bool,
    pub(in crate::renderer) vertex_dlights: bool,
    pub(in crate::renderer) clustered_lite_dlights: bool,
    pub(in crate::renderer) area_lights: bool,
    pub(in crate::renderer) irradiance_volume: bool,
    pub(in crate::renderer) voxel_gi: bool,
    pub(in crate::renderer) local_shadows: bool,
    pub(in crate::renderer) cascaded_shadows: bool,
    pub(in crate::renderer) ray_traced_shadows: bool,
    pub(in crate::renderer) ray_traced_sun: bool,
    /// Shader source/interface identity, frozen for the lifetime of the process.
    pub(in crate::renderer) rt_receiver_glass: bool,
    pub(in crate::renderer) planar_reflections: bool,
    pub(in crate::renderer) legacy_fog: bool,
    pub(in crate::renderer) jump_shade: bool,
    pub(in crate::renderer) ocean: bool,
    /// Wetness/puddle shading compiled in. Off for dry scenes (see `weather_surface_needed`).
    pub(in crate::renderer) weather_surface: bool,
    /// r_fullbright / r_vertexLight / r_lightmap active.
    pub(in crate::renderer) classic_render_flags: bool,
    /// Cached BSP ambient occlusion applied to baked light.
    pub(in crate::renderer) static_bsp_ao: bool,
    /// Compile BSP baked-sun separation only when actually selected.
    pub(in crate::renderer) baked_sun_relight: bool,
    /// An r_planarDebug view is selected.
    pub(in crate::renderer) planar_debug: bool,
    /// Directional baked lighting (deluxemap / lightgrid) on PBR surfaces; implies `pbr`.
    pub(in crate::renderer) deluxe: bool,
    /// Deluxe GGX specular lobe; implies `deluxe` and a non-zero r_deluxeSpecular.
    pub(in crate::renderer) deluxe_specular: bool,
    pub(in crate::renderer) detail_textures_enabled: bool,
    /// Snow shells / Snowflow / stock footprint stamps compiled in: footprints are
    /// not Off and the loaded map has snow surfaces.
    pub(in crate::renderer) surface_deformation: bool,
    /// r_dynamicLightDebug receiver view.
    pub(in crate::renderer) dynamic_light_debug: bool,
    /// Sky cost probe or the experimental fractal sky override is active.
    pub(in crate::renderer) sky_debug: bool,
    /// r_fragmentShaderMode 1: base-pass PSOs are also compiled per
    /// MaterialFragmentKey. Only selects which programs a variant holds; no WGSL
    /// override of its own.
    pub(in crate::renderer) material_specialization: bool,
    /// r_fragmentShaderMode 2: eligible specialized PSOs use the dedicated
    /// minimal entry points (`vs_world_minimal` / `fs_world_minimal`).
    pub(in crate::renderer) minimal_entries: bool,
    pub(in crate::renderer) program_mode: WorldProgramMode,
}

// MaterialUniform header.z bits that are never read by the world WGSL (CPU,
// depth-prepass or RT only). Fixing them would only split pipelines.
const MATERIAL_FLAGS_UNREAD: u32 = 8192 | 4194304 | 67108864 | 65536;
const MATERIAL_FLAGS_PBR: u32 =
    8 | 16 | 32 | 64 | 128 | 256 | 512 | 1024 | 2048 | 262144;
const MATERIAL_FLAGS_PLANAR: u32 = 4096 | 16384 | 32768;
const MATERIAL_FLAG_IMPLICIT_LIGHTMAP: u32 = 2;
const MATERIAL_FLAG_DYNAMIC_RECEIVER: u32 = 4;
const MATERIAL_FLAG_VERTEX_LIT: u32 = 131072;
const MATERIAL_FLAGS_VERTEX_COLOR: u32 = 1 | 524288 | 1048576 | 2097152;
const MATERIAL_FLAG_EXPLICIT_LIGHTMAP: u32 = 8388608;
const MATERIAL_FLAG_HAS_LIGHTMAP: u32 = 16777216;
const MATERIAL_FLAG_OPAQUE_STAGE: u32 = 33554432;
const MATERIAL_FLAGS_WAVES: u32 = 134217728 | 268435456;
const MATERIAL_FLAG_LEGACY_SRGB: u32 = 536870912;
const MATERIAL_FLAG_MASK_TEXTURE_ALPHA: u32 = 1073741824;
/// MATERIAL_KEY_TC_GEN value that leaves tcGen read from the uniform.
const MATERIAL_TC_GEN_DYNAMIC: u32 = u32::MAX;
const MATERIAL_FLAGS_W_DETAIL: u32 = 2;
const MATERIAL_FLAGS_W_PLANAR: u32 = 4;
const MATERIAL_FLAGS_W_DLIGHT_IN_LIGHTMAP: u32 = 65536;
/// Promoted glass primary stage. Only `fs_main` / `fs_main_legacy_fog` shade it,
/// so it is carried in the raw key for program selection; `for_variant` drops it.
pub(in crate::renderer) const MATERIAL_FLAGS_W_GLASS: u32 = 1 << 17;

/// TaystJK-style per-material fragment specialization (r_fragmentShaderMode 1).
///
/// The material checks in the world WGSL read `MaterialUniform.header` through
/// `material_flags()` / `material_flags_w()` / `material_tc_gen()`. A pipeline built
/// with this key fixes the bits in `flags_mask` / `flags_w_mask` (and tcGen and the
/// alpha function) as override constants, so the driver folds those branches away.
/// The key is taken from the uploaded uniform itself and every batch drawn with the
/// PSO has the same key, so the specialized program is exact by construction.
/// Masks only drop bits whose code is compiled out by the global variant anyway,
/// to keep the PSO count down; dropped bits are still read from the uniform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::renderer) struct MaterialFragmentKey {
    pub(in crate::renderer) flags: u32,
    pub(in crate::renderer) flags_mask: u32,
    pub(in crate::renderer) flags_w: u32,
    pub(in crate::renderer) flags_w_mask: u32,
    pub(in crate::renderer) tc_gen: u32,
    /// 1 none, 2 discard below params.x, 3 discard at/above -params.x.
    pub(in crate::renderer) alpha_func: u32,
    /// Structural vertex operations; numeric parameters remain uniforms.
    pub(in crate::renderer) vertex_recipe: VertexRecipe,
    pub(in crate::renderer) program: StageProgram,
}

impl MaterialFragmentKey {
    /// The full key of an uploaded material; `for_variant` narrows it.
    pub(in crate::renderer) fn from_material(header: [u32; 4], alpha_cutoff: f32) -> Self {
        Self {
            flags: header[2] & !MATERIAL_FLAGS_UNREAD,
            flags_mask: !MATERIAL_FLAGS_UNREAD,
            flags_w: header[3] & (MATERIAL_FLAGS_W_DETAIL | MATERIAL_FLAGS_W_PLANAR | MATERIAL_FLAGS_W_DLIGHT_IN_LIGHTMAP | MATERIAL_FLAGS_W_GLASS),
            flags_w_mask: MATERIAL_FLAGS_W_DETAIL | MATERIAL_FLAGS_W_PLANAR | MATERIAL_FLAGS_W_DLIGHT_IN_LIGHTMAP | MATERIAL_FLAGS_W_GLASS,
            tc_gen: header[0],
            alpha_func: if alpha_cutoff > 0.0 {
                2
            } else if alpha_cutoff < 0.0 {
                3
            } else {
                1
            },
            vertex_recipe: VertexRecipe::default(),
            program: StageProgram::Specialized,
        }
    }

    pub(in crate::renderer) fn from_uniform(material: &crate::renderer::MaterialUniform) -> Self {
        let mut key = Self::from_material(material.header, material.params[0]);
        key.vertex_recipe = VertexRecipe::from_uniform(material);
        key
    }

    /// TaystJK-style classification (r_fragmentShaderMode 1): keep only the
    /// header bits whose code the global variant actually compiles in, so
    /// materials that differ elsewhere share one PSO (TaystJK collapses stages
    /// into a small set of shader types the same way). On Minimal that leaves
    /// lightmap multiply, colour/alpha source, waves, legacy sRGB output and the
    /// alpha function. Dropped bits stay readable from the uniform, so a dropped
    /// bit can never change the image; it only keeps a check whose code is dead.
    pub(in crate::renderer) fn for_variant(
        mut self,
        variant: WorldShaderVariantKey,
        alpha_test: bool,
    ) -> Self {
        let (keep, keep_w, fix_tc_gen) = Self::relevant_bits(variant, alpha_test);
        self.vertex_recipe = VertexRecipe::default();
        self.program = StageProgram::Specialized;
        self.flags &= keep;
        self.flags_mask &= keep;
        self.flags_w &= keep_w;
        self.flags_w_mask &= keep_w;
        if !fix_tc_gen {
            self.tc_gen = MATERIAL_TC_GEN_DYNAMIC;
        }
        if !alpha_test {
            self.alpha_func = 0;
        }
        self
    }

    /// Header bits (z, w) and tcGen whose WGSL reads are live under `variant`.
    /// Mirrors the `material_flags()` / `material_flags_w()` / `material_tc_gen()`
    /// reads in shade_surface and its helpers (bsp_lean.wgsl / bsp.wgsl).
    fn relevant_bits(variant: WorldShaderVariantKey, alpha_test: bool) -> (u32, u32, bool) {
        // Always live: implicit lightmap multiply, rgbGen/alphaGen vertex and
        // one-minus-vertex, rgbGen/alphaGen wave, legacy sRGB additive output,
        // and the explicit $lightmap stage's opaque-alpha override.
        let mut keep = MATERIAL_FLAG_IMPLICIT_LIGHTMAP
            | MATERIAL_FLAG_EXPLICIT_LIGHTMAP
            | MATERIAL_FLAGS_VERTEX_COLOR
            | MATERIAL_FLAGS_WAVES
            | MATERIAL_FLAG_LEGACY_SRGB;
        let mut keep_w = 0;
        let mut fix_tc_gen = false;
        if alpha_test {
            keep |= MATERIAL_FLAG_MASK_TEXTURE_ALPHA;
        }
        if variant.pbr || variant.pom {
            keep |= MATERIAL_FLAGS_PBR | MATERIAL_FLAG_DYNAMIC_RECEIVER;
        }
        if variant.planar_reflections || variant.planar_debug {
            keep |= MATERIAL_FLAGS_PLANAR;
            keep_w |= MATERIAL_FLAGS_W_PLANAR;
            fix_tc_gen = true;
        }
        if variant.classic_render_flags || variant.baked_sun_relight {
            keep |= MATERIAL_FLAG_EXPLICIT_LIGHTMAP
                | MATERIAL_FLAG_HAS_LIGHTMAP
                | MATERIAL_FLAG_OPAQUE_STAGE
                | MATERIAL_FLAG_VERTEX_LIT;
        }
        if variant.dynamic_light_debug || variant.jump_shade {
            keep |= MATERIAL_FLAG_OPAQUE_STAGE | MATERIAL_FLAG_DYNAMIC_RECEIVER;
        }
        if variant.static_bsp_ao {
            keep |= MATERIAL_FLAG_VERTEX_LIT;
            fix_tc_gen = true;
        }
        if variant.map_light_simulation || variant.source_map_world {
            keep |= MATERIAL_FLAG_DYNAMIC_RECEIVER | MATERIAL_FLAG_VERTEX_LIT;
        }
        if variant.voxel_gi
            || variant.cascaded_shadows
            || variant.vertex_dlights
            || variant.legacy_dlights
            || variant.clustered_lite_dlights
            || variant.point_lights
            || variant.area_lights
        {
            keep |= MATERIAL_FLAG_DYNAMIC_RECEIVER;
        }
        if variant.detail_textures_enabled {
            keep_w |= MATERIAL_FLAGS_W_DETAIL;
        }
        if variant.legacy_dlights {
            keep_w |= MATERIAL_FLAGS_W_DLIGHT_IN_LIGHTMAP;
        }
        (keep, keep_w, fix_tc_gen)
    }

    pub(in crate::renderer) fn compilation_constants(self) -> [(&'static str, f64); 6] {
        [
            ("MATERIAL_KEY_FLAGS", f64::from(self.flags)),
            ("MATERIAL_KEY_FLAGS_MASK", f64::from(self.flags_mask)),
            ("MATERIAL_KEY_FLAGS_W", f64::from(self.flags_w)),
            ("MATERIAL_KEY_FLAGS_W_MASK", f64::from(self.flags_w_mask)),
            ("MATERIAL_KEY_TC_GEN", f64::from(self.tc_gen)),
            ("MATERIAL_KEY_ALPHA_FUNC", f64::from(self.alpha_func)),
        ]
    }
}

impl WorldShaderVariantKey {
    /// Whether the minimal world entries equal fs_main under these settings: every
    /// global feature they leave out is compiled out. Static AO, legacy fog and
    /// the jump-shade helper are kept by the minimal entries themselves. The base
    /// pass never compiles Legacy dlights in (see `base_pass_key`).
    pub(in crate::renderer) fn minimal_entry_compatible(self) -> bool {
        self.minimal_entries
            && self.material_specialization
            && self.family() == WorldShaderFamily::Lean
            && !self.pbr
            && !self.pom
            && !self.point_lights
            && !self.map_light_simulation
            && !self.source_map_world
            && !self.vertex_dlights
            && !self.clustered_lite_dlights
            && !self.area_lights
            && !self.irradiance_volume
            && !self.voxel_gi
            && !self.local_shadows
            && !self.cascaded_shadows
            && !self.ray_traced_shadows
            && !self.planar_reflections
            && !self.ocean
            && !self.weather_surface
            && !self.baked_sun_relight
            && !self.classic_render_flags
            && !self.planar_debug
            && !self.deluxe
            && !self.detail_textures_enabled
            && !self.surface_deformation
            && !self.dynamic_light_debug
    }

    /// The authored Legacy dlight contribution is a separate depth-equal pass.
    /// The base material BSP pass has ENABLE_LEGACY_DLIGHTS=0 even while the
    /// selected dynamic-light mode is Legacy. Treat its compiled PSOs as the
    /// *same program* across otherwise-identical variants (no runtime flags).
    pub(in crate::renderer) fn base_pass_key(mut self) -> Self {
        self.legacy_dlights = false;
        self
    }

    pub(in crate::renderer) fn family(self) -> WorldShaderFamily {
        if self.point_lights
            || self.area_lights
            || self.irradiance_volume
            || self.voxel_gi
            || self.ocean
        {
            WorldShaderFamily::Enhanced
        } else {
            WorldShaderFamily::Lean
        }
    }

    pub(in crate::renderer) fn compilation_constants(self) -> Vec<(&'static str, f64)> {
        let mut constants = vec![
            ("ENABLE_PBR", f64::from(u8::from(self.pbr))),
            ("ENABLE_POM", f64::from(u8::from(self.pom))),
            (
                "PBR_SHARED_MATERIAL_EVAL",
                f64::from(u8::from(self.pbr_shared_material_eval)),
            ),
            (
                "PBR_SHARED_TANGENT_FRAME",
                f64::from(u8::from(self.pbr_shared_tangent_frame)),
            ),
            ("POM_MIP_AWARE", f64::from(u8::from(self.pom_mip_aware))),
            (
                "POM_ADAPTIVE_STEPS",
                f64::from(u8::from(self.pom_adaptive_steps)),
            ),
            (
                "PBR_COMPANION_SAMPLER",
                f64::from(u8::from(self.pbr_companion_sampler)),
            ),
            (
                "PBR_VERTEX_LIGHTGRID",
                f64::from(u8::from(self.pbr_vertex_lightgrid)),
            ),
            (
                "ENABLE_POINT_LIGHTS",
                f64::from(u8::from(self.point_lights)),
            ),
            (
                "ENABLE_MAP_LIGHT_SIMULATION",
                f64::from(u8::from(self.map_light_simulation)),
            ),
            (
                "ENABLE_SOURCE_MAP_WORLD",
                f64::from(u8::from(self.source_map_world)),
            ),
            (
                "ENABLE_LEGACY_DLIGHTS",
                f64::from(u8::from(self.legacy_dlights)),
            ),
            (
                "ENABLE_VERTEX_DLIGHTS",
                f64::from(u8::from(self.vertex_dlights)),
            ),
            (
                "ENABLE_CLUSTERED_LITE_DLIGHTS",
                f64::from(u8::from(self.clustered_lite_dlights)),
            ),
            ("ENABLE_AREA_LIGHTS", f64::from(u8::from(self.area_lights))),
            (
                "ENABLE_IRRADIANCE_VOLUME",
                f64::from(u8::from(self.irradiance_volume)),
            ),
            ("ENABLE_VOXEL_GI", f64::from(u8::from(self.voxel_gi))),
            (
                "ENABLE_LOCAL_SHADOWS",
                f64::from(u8::from(self.local_shadows)),
            ),
            (
                "ENABLE_CASCADED_SHADOWS",
                f64::from(u8::from(self.cascaded_shadows)),
            ),
            (
                "ENABLE_PLANAR_REFLECTIONS",
                f64::from(u8::from(self.planar_reflections)),
            ),
            ("ENABLE_OCEAN", f64::from(u8::from(self.ocean))),
            ("ENABLE_DETAIL_TEXTURES", f64::from(u8::from(self.detail_textures_enabled))),
            ("ENABLE_LEGACY_FOG", f64::from(u8::from(self.legacy_fog))),
            ("ENABLE_JUMP_SHADE", f64::from(u8::from(self.jump_shade))),
            (
                "ENABLE_WEATHER_SURFACE",
                f64::from(u8::from(self.weather_surface)),
            ),
            (
                "ENABLE_CLASSIC_RENDER_FLAGS",
                f64::from(u8::from(self.classic_render_flags)),
            ),
            ("ENABLE_BAKED_SUN_RELIGHT", f64::from(u8::from(self.baked_sun_relight))),
            (
                "ENABLE_STATIC_BSP_AO",
                f64::from(u8::from(self.static_bsp_ao)),
            ),
            (
                "ENABLE_PLANAR_DEBUG",
                f64::from(u8::from(self.planar_debug)),
            ),
            ("ENABLE_DELUXE", f64::from(u8::from(self.deluxe))),
            (
                "ENABLE_DELUXE_SPECULAR",
                f64::from(u8::from(self.deluxe_specular)),
            ),
            (
                "ENABLE_SURFACE_DEFORMATION",
                f64::from(u8::from(self.surface_deformation)),
            ),
            (
                "ENABLE_DYNAMIC_LIGHT_DEBUG",
                f64::from(u8::from(self.dynamic_light_debug)),
            ),
            ("ENABLE_SKY_DEBUG", f64::from(u8::from(self.sky_debug))),
        ];
        // The stock BSP shaders intentionally do not declare the experimental
        // ray-query override. Only the lazily-created RT shader receives it, so
        // selecting any raster shadow mode keeps the normal WGSL/layout exactly
        // as it was before hardware RT support existed.
        if self.ray_traced_shadows {
            constants.push(("ENABLE_RAY_TRACED_SHADOWS", 1.0));
            constants.push((
                "ENABLE_RAY_TRACED_SUN",
                f64::from(u8::from(self.ray_traced_sun)),
            ));
        }
        constants
    }

    pub(in crate::renderer) fn short_label(self) -> String {
        let mut enabled = Vec::new();
        if self.pbr {
            enabled.push("pbr");
        }
        if self.pom {
            enabled.push("pom");
        }
        if self.pbr && self.pbr_shared_material_eval {
            enabled.push("pbr-cache");
        }
        if (self.pbr || self.pom) && self.pbr_shared_tangent_frame {
            enabled.push("tbn-cache");
        }
        if self.pom && self.pom_mip_aware {
            enabled.push("pom-mip");
        }
        if self.pom && self.pom_adaptive_steps {
            enabled.push("pom-adapt");
        }
        if self.pbr && self.pbr_companion_sampler {
            enabled.push("pbr-sampler");
        }
        if self.pbr && self.pbr_vertex_lightgrid {
            enabled.push("vertex-lightgrid");
        }
        if self.point_lights {
            enabled.push("point");
        }
        if self.source_map_world {
            enabled.push("source-map");
        }
        if self.map_light_simulation {
            enabled.push("map-light-sim");
        }
        if self.legacy_dlights {
            enabled.push("legacy-dlight");
        }
        if self.vertex_dlights {
            enabled.push("vertex-dlight");
        }
        if self.clustered_lite_dlights {
            enabled.push("cluster-lite");
        }
        if self.area_lights {
            enabled.push("area");
        }
        if self.voxel_gi {
            enabled.push("gi");
        }
        if self.local_shadows {
            enabled.push("local-shadow");
        }
        if self.cascaded_shadows {
            enabled.push("sun-shadow");
        }
        if self.ray_traced_shadows {
            enabled.push(if self.ray_traced_sun {
                "rt-shadow"
            } else {
                "rt-local"
            });
        }
        if self.planar_reflections {
            enabled.push("planar");
        }
        if self.legacy_fog {
            enabled.push("legacy-fog");
        }
        if self.jump_shade {
            enabled.push("jump-shade");
        }
        if self.ocean {
            enabled.push("ocean");
        }
        if self.weather_surface {
            enabled.push("wet");
        }
        if self.classic_render_flags {
            enabled.push("classic");
        }
        if self.baked_sun_relight { enabled.push("baked-sun"); }
        if self.static_bsp_ao {
            enabled.push("static-ao");
        }
        if self.planar_debug {
            enabled.push("planar-debug");
        }
        if self.deluxe {
            enabled.push(if self.deluxe_specular {
                "deluxe+spec"
            } else {
                "deluxe"
            });
        }
        if self.detail_textures_enabled {
            enabled.push("detail");
        }
        if self.surface_deformation {
            enabled.push("snow-deform");
        }
        if self.dynamic_light_debug {
            enabled.push("dlight-debug");
        }
        if self.material_specialization {
            enabled.push(match self.program_mode {
                WorldProgramMode::Shared => "shared-families",
                WorldProgramMode::Generated => "generated-stages",
                WorldProgramMode::Specialized if self.minimal_entries => "mat-spec+minimal",
                WorldProgramMode::Specialized => "mat-spec",
            });
        }
        if self.sky_debug {
            enabled.push("sky-debug");
        }
        if enabled.is_empty() {
            "base".to_string()
        } else {
            enabled.join("+")
        }
    }
}
