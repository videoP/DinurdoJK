//! MaterialUniform's two independent flag namespaces: header.z and header.w.
//! Each entry owns its bits in that word, including packed fields. Unlisted bits
//! are unassigned. Add meanings here, never in a CPU writer or shader consumer.
//! The same macro emits Rust values and ordinary WGSL constants; no overrides,
//! runtime translation or additional pipeline identities are introduced.

macro_rules! material_word {
    ($entries:ident, $wgsl:ident; $(#[doc = $doc:literal] $name:ident = $value:literal;)*) => {
        $(#[doc = $doc] pub(crate) const $name: u32 = $value;)*
        pub(crate) const $entries: &[(&str, u32)] = &[$((stringify!($name), $name)),*];
        const $wgsl: &str = concat!($("const ", stringify!($name), ": u32 = ", stringify!($value), "u;\n"),*);
        const _: () = {
            let mut used = 0u32;
            let mut i = 0;
            while i < $entries.len() {
                let bits = $entries[i].1;
                assert!(bits != 0, "empty material registry entry");
                assert!(used & bits == 0, "overlapping material bits in the same header word");
                used |= bits;
                i += 1;
            }
        };
    };
}

material_word! { HEADER_Z, WGSL_Z;
    /// RGB uses vertex color (rgbGen vertex/exactVertex).
    MATERIAL_FLAG_RGB_VERTEX = 0x00000001;
    /// Multiply by the implicit BSP lightmap.
    MATERIAL_FLAG_IMPLICIT_LIGHTMAP = 0x00000002;
    /// Receives runtime lighting and environmental overlays.
    MATERIAL_FLAG_DYNAMIC_RECEIVER = 0x00000004;
    /// PBR normal texture.
    MATERIAL_FLAG_NORMAL_MAP = 0x00000008;
    /// PBR roughness texture.
    MATERIAL_FLAG_ROUGHNESS_MAP = 0x00000010;
    /// Parallax height texture.
    MATERIAL_FLAG_HEIGHT_MAP = 0x00000020;
    /// PBR metallic texture.
    MATERIAL_FLAG_METALLIC_MAP = 0x00000040;
    /// PBR specular texture.
    MATERIAL_FLAG_SPECULAR_MAP = 0x00000080;
    /// PBR emissive texture.
    MATERIAL_FLAG_EMISSIVE_MAP = 0x00000100;
    /// Packed roughness/metallic/occlusion channels.
    MATERIAL_FLAG_PACKED_RMO = 0x00000200;
    /// Height is in texture alpha.
    MATERIAL_FLAG_HEIGHT_FROM_ALPHA = 0x00000400;
    /// PBR reflection probe receiver.
    MATERIAL_FLAG_REFLECTION_PROBE = 0x00000800;
    /// Authored planar reflection stage.
    MATERIAL_FLAG_PLANAR_REFLECTION = 0x00001000;
    /// Candidate for live tcGen environment substitution (CPU/depth only).
    MATERIAL_FLAG_PLANAR_ENVIRONMENT = 0x00002000;
    /// Resolved plane for reflection diagnostics.
    MATERIAL_FLAG_PLANAR_DEBUG = 0x00004000;
    /// Blended environment reflection response.
    MATERIAL_FLAG_BLENDED_ENVIRONMENT = 0x00008000;
    /// Promoted primary water stage (depth/thermal policy).
    MATERIAL_FLAG_WATER_PRIMARY = 0x00010000;
    /// BSP vertex-lit cached-lighting receiver.
    MATERIAL_FLAG_VERTEX_LIT = 0x00020000;
    /// Packed RMO specular is in alpha (distinct from header.w foliage).
    MATERIAL_FLAG_RMO_SPECULAR_ALPHA = 0x00040000;
    /// RGB uses one minus vertex color.
    MATERIAL_FLAG_RGB_ONE_MINUS_VERTEX = 0x00080000;
    /// Alpha uses vertex alpha.
    MATERIAL_FLAG_ALPHA_VERTEX = 0x00100000;
    /// Alpha uses one minus vertex alpha.
    MATERIAL_FLAG_ALPHA_ONE_MINUS_VERTEX = 0x00200000;
    /// SSR receiver policy for the depth prepass.
    MATERIAL_FLAG_SSR = 0x00400000;
    /// Explicit authored $lightmap texture stage.
    MATERIAL_FLAG_EXPLICIT_LIGHTMAP = 0x00800000;
    /// Authored lightmap binding exists.
    MATERIAL_FLAG_HAS_LIGHTMAP = 0x01000000;
    /// Opaque stage blend policy.
    MATERIAL_FLAG_OPAQUE_STAGE = 0x02000000;
    /// Participates in global legacy fog (depth prepass).
    MATERIAL_FLAG_GLOBAL_FOG = 0x04000000;
    /// rgbGen wave.
    MATERIAL_FLAG_RGB_WAVE = 0x08000000;
    /// alphaGen wave.
    MATERIAL_FLAG_ALPHA_WAVE = 0x10000000;
    /// Legacy encoded-space additive source compositing.
    MATERIAL_FLAG_LEGACY_SRGB = 0x20000000;
    /// Alpha-test recovery uses only texture alpha.
    MATERIAL_FLAG_MASK_TEXTURE_ALPHA = 0x40000000;
    /// Additional static BSP lightstyle; do not repeat runtime lighting.
    MATERIAL_FLAG_EXTRA_STATIC_LIGHTSTYLE = 0x80000000;
}

material_word! { HEADER_W, WGSL_W;
    /// Authored skybox stage.
    MATERIAL_FLAG_SKYBOX = 0x00000001;
    /// Eligible for automatic detail texture.
    MATERIAL_FLAG_DETAIL = 0x00000002;
    /// Preserve authored environment compositing.
    MATERIAL_FLAG_AUTHORED_ENVIRONMENT = 0x00000004;
    /// Whole-surface lava heat in thermal capture.
    MATERIAL_FLAG_THERMAL_LAVA = 0x00000008;
    /// Explicit q3map_surfacelight heat in thermal capture.
    MATERIAL_FLAG_THERMAL_SURFACELIGHT = 0x00000020;
    /// Planar stage uses GL_ONE GL_SRC_ALPHA.
    MATERIAL_FLAG_PLANAR_SRC_ALPHA = 0x00000040;
    /// Planar stage uses GL_ONE GL_ONE_MINUS_SRC_ALPHA.
    MATERIAL_FLAG_PLANAR_ONE_MINUS_SRC_ALPHA = 0x00000080;
    /// Packed surface-material ID, bits 8..12; this is a field, not a boolean.
    MATERIAL_SURFACE_KIND_MASK = 0x00001f00;
    /// Dynamic lights are applied at the authored lightmap stage.
    MATERIAL_FLAG_DLIGHT_IN_LIGHTMAP = 0x00010000;
    /// Promoted primary glass stage.
    MATERIAL_FLAG_GLASS = 0x00020000;
    /// Conservatively promoted authored foliage cards may deform in wind.
    MATERIAL_FLAG_FOLIAGE_WIND = 0x00040000;
    /// Cached additive-stage vertex lighting gain.
    MATERIAL_FLAG_BAKED_ADDITIVE = 0x00080000;
    /// Prepared post-lightmap environmental overlay.
    MATERIAL_FLAG_SKY_OVERLAY = 0x00100000;
    /// Fused additive stage uses GL_ONE rather than GL_SRC_ALPHA.
    MATERIAL_FLAG_ADDITIVE_ONE = 0x00200000;
    /// Vertex-lit identityLighting overlay follows base relighting.
    MATERIAL_FLAG_VERTEX_OVERLAY = 0x00400000;
    /// Inline geometry lacks a receiver in the main-map baked-lighting atlas.
    MATERIAL_FLAG_INLINE_BAKED_RECEIVER_EXCLUSION = 0x00800000;
    /// Extra authored additive stage selects the clamp sampler.
    MATERIAL_FLAG_ADDITIVE_CLAMP = 0x01000000;
}

pub(crate) const MATERIAL_SURFACE_KIND_SHIFT: u32 = 8;

/// Called only while preparing shader modules. WGSL parses these as `const`,
/// so flags remain ordinary compile-time integers on every rendering path.
pub(crate) fn shader_constants() -> &'static str {
    static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SOURCE.get_or_init(|| format!("{WGSL_Z}{WGSL_W}const MATERIAL_SURFACE_KIND_SHIFT: u32 = {MATERIAL_SURFACE_KIND_SHIFT}u;\n"))
}

pub(crate) fn depth_shader() -> &'static str {
    static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SOURCE.get_or_init(|| format!("{}\n{}", shader_constants(), include_str!("depth_prepass.wgsl")))
}

pub(crate) fn shadow_shader() -> &'static str {
    static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SOURCE.get_or_init(|| format!("{}\n{}", shader_constants(), include_str!("shadow.wgsl")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_wgsl_matches_both_rust_namespaces() {
        let module = wgpu::naga::front::wgsl::parse_str(shader_constants()).unwrap();
        for &(name, value) in HEADER_Z.iter().chain(HEADER_W) {
            let constant = module.constants.iter().find(|(_, c)| c.name.as_deref() == Some(name)).unwrap().1;
            assert!(matches!(module.global_expressions[constant.init],
                wgpu::naga::Expression::Literal(wgpu::naga::Literal::U32(v)) if v == value), "{name}");
        }
    }
}
