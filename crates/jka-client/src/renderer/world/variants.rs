//! World variants.
use crate::renderer::Hash;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    /// An r_planarDebug view is selected.
    pub(in crate::renderer) planar_debug: bool,
    /// Directional baked lighting (deluxemap / lightgrid) on PBR surfaces; implies `pbr`.
    pub(in crate::renderer) deluxe: bool,
    /// Deluxe GGX specular lobe; implies `deluxe` and a non-zero r_deluxeSpecular.
    pub(in crate::renderer) deluxe_specular: bool,
    pub(in crate::renderer) detail_texture_mode: u32,
}

impl WorldShaderVariantKey {
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
            ("DETAIL_TEXTURE_MODE", f64::from(self.detail_texture_mode)),
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
        if self.detail_texture_mode != 0 {
            enabled.push("detail");
        }
        if enabled.is_empty() {
            "base".to_string()
        } else {
            enabled.join("+")
        }
    }
}
