//! Assets types.
use crate::cgame::player_presenter::{
    Arc, DynamicModelAlphaMode, Ghoul2GpuVertex, GlaAnimation, GlmModel, JiggleProfile, TcMod,
    TextureData,
};

#[derive(Debug)]
pub(in crate::cgame::player_presenter) struct Ghoul2GpuMeshSource {
    /// Original GLM geometry key/topology used whenever jiggle is off.
    pub(in crate::cgame::player_presenter) base_key: Arc<str>,
    pub(in crate::cgame::player_presenter) base_vertices: Arc<Vec<Ghoul2GpuVertex>>,
    pub(in crate::cgame::player_presenter) cpu_indices: Arc<Vec<u32>>,
    /// GPU/promoted topology. Jiggle surfaces use one Phong-style subdivision.
    pub(in crate::cgame::player_presenter) key: Arc<str>,
    pub(in crate::cgame::player_presenter) vertices: Arc<Vec<Ghoul2GpuVertex>>,
    pub(in crate::cgame::player_presenter) indices: Arc<Vec<u32>>,
}

#[derive(Clone)]
pub(in crate::cgame::player_presenter) struct SurfaceOverlay {
    pub(in crate::cgame::player_presenter) texture: Option<Arc<TextureData>>,
    pub(in crate::cgame::player_presenter) alpha_mode: DynamicModelAlphaMode,
}

/// One stage of a fully blended (additive/glow) shader, drawn as its own
/// pass over the surface: OpenJK runs every stage of a `trans` player shader, so
/// the first stage alone is usually not the look the author built.
#[derive(Clone)]
pub(in crate::cgame::player_presenter) struct ResolvedStage {
    pub(in crate::cgame::player_presenter) texture: Option<Arc<TextureData>>,
    pub(in crate::cgame::player_presenter) alpha_mode: DynamicModelAlphaMode,
    /// `rgbGen const` colour (white otherwise) and `alphaGen const` alpha.
    pub(in crate::cgame::player_presenter) rgb: [f32; 3],
    pub(in crate::cgame::player_presenter) alpha: f32,
    /// `rgbGen const` stages are not lit by the light grid.
    pub(in crate::cgame::player_presenter) unlit: bool,
    /// `alphaGen lightingSpecular`: alpha is q3's specular term per vertex.
    pub(in crate::cgame::player_presenter) specular_alpha: bool,
    /// Only `tcMod scale` / `tcMod scroll` are applied.
    pub(in crate::cgame::player_presenter) tc_mods: Vec<TcMod>,
}

/// A surface's resolved material: the primary stage plus the entity-tint layers.
pub(in crate::cgame::player_presenter) struct ResolvedMaterial {
    pub(in crate::cgame::player_presenter) texture: Option<Arc<TextureData>>,
    pub(in crate::cgame::player_presenter) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::cgame::player_presenter) entity_tint: bool,
    pub(in crate::cgame::player_presenter) overlay: Option<SurfaceOverlay>,
    /// Non-empty for a blended multi-stage shader; then `texture`/`alpha_mode`
    /// are the first entry's and the surface is drawn once per stage.
    pub(in crate::cgame::player_presenter) stages: Vec<ResolvedStage>,
    /// `cull twosided` on a translucent surface (wings): the mesh also needs its
    /// reversed-winding triangles because the pipelines always cull back faces.
    pub(in crate::cgame::player_presenter) two_sided: bool,
}

#[derive(Clone)]
pub(in crate::cgame::player_presenter) struct PlayerSurfaceAsset {
    pub(in crate::cgame::player_presenter) surface_index: usize,
    /// Ghoul2/skin default state. Dismemberment keeps authored `*off` cap
    /// surfaces resident and force-enables them only after a server sever event.
    pub(in crate::cgame::player_presenter) default_visible: bool,
    /// Ghoul2 hierarchy name. Kept so runtime surface on/off rules such as
    /// TaystJK cg_fpls can be applied without rebuilding the model.
    pub(in crate::cgame::player_presenter) surface_name: String,
    pub(in crate::cgame::player_presenter) texture: Option<Arc<TextureData>>,
    pub(in crate::cgame::player_presenter) alpha_mode: DynamicModelAlphaMode,
    /// Asset Viewer only: this surface had no usable authored material, so
    /// render it neutral gray instead of disappearing into the black preview.
    pub(in crate::cgame::player_presenter) fallback_gray: bool,
    /// The primary stage is `rgbGen lightingDiffuseEntity`: the entity's
    /// shaderRGBA (`char_color_*`) tints it.
    pub(in crate::cgame::player_presenter) entity_tint: bool,
    /// The later alpha-blended stage of a tinted shader, redrawn untinted over the
    /// base: the base texture's alpha marks the regions that follow `char_color_*`.
    pub(in crate::cgame::player_presenter) overlay: Option<SurfaceOverlay>,
    /// Every drawable stage of a blended multi-stage shader (see `ResolvedStage`).
    pub(in crate::cgame::player_presenter) stages: Vec<ResolvedStage>,
    /// Static bind-pose mesh for each authored GLM LOD. A surface can be
    /// absent from a lower LOD, matching Ghoul2's per-LOD surface tables.
    pub(in crate::cgame::player_presenter) gpu_meshes: Vec<Option<Arc<Ghoul2GpuMeshSource>>>,
}

pub(in crate::cgame::player_presenter) const FPLS_MODE3_OFF_SURFACES: &[&str] = &[
    // EternalJK CG_ForceFPLSPlayerModel: every FPLS mode removes the head
    // variants and the TIE-pilot hoses so first person never sits inside them.
    "head_eyes_mouth",
    "heada_eyes_mouth",
    "head",
    "heada",
    "heada_face",
    "headb",
    "headb_face",
    "headb_eyes_mouth",
    "torso_l_hose",
    "torso_r_hose",
    // EternalJK modes 2/3 additionally turn these exact surfaces off; its
    // source describes that state as "removes everything but the saber hilt".
    // Dinurdo's cg_fpls is boolean, so enabled maps to mode 3: no player body,
    // normal first-person camera, held saber hilt/blades retained.
    "hips",
    "hipsa",
    "torso",
    "torsoa",
];

pub(in crate::cgame::player_presenter) fn fpls_mode3_surface_hidden(
    glm: &GlmModel,
    surface_index: usize,
) -> bool {
    // EternalJK passes TURN_OFF (G2SURFACEFLAG_NODESCENDANTS), so disabling
    // hips/torso also suppresses every child surface below those roots. Walk
    // the GLM parent chain to reproduce Ghoul2's hierarchy semantics rather
    // than merely hiding the four surfaces whose names were passed to G2API.
    let mut index = surface_index;
    for _ in 0..=glm.hierarchy.len() {
        let Some(surface) = glm.hierarchy.get(index) else {
            return false;
        };
        if FPLS_MODE3_OFF_SURFACES
            .iter()
            .any(|hidden| surface.name.eq_ignore_ascii_case(hidden))
        {
            return true;
        }
        let Ok(parent) = usize::try_from(surface.parent_index) else {
            return false;
        };
        if parent == index {
            return false;
        }
        index = parent;
    }
    false
}

pub(in crate::cgame::player_presenter) struct PlayerModelAsset {
    pub(in crate::cgame::player_presenter) key: String,
    pub(in crate::cgame::player_presenter) glm: Arc<GlmModel>,
    pub(in crate::cgame::player_presenter) gla: Arc<GlaAnimation>,
    pub(in crate::cgame::player_presenter) surfaces: Vec<PlayerSurfaceAsset>,
    /// Optional post-Ghoul2 soft-tissue profile loaded beside model.glm.
    pub(in crate::cgame::player_presenter) jiggle: Option<Arc<JiggleProfile>>,
}

pub(in crate::cgame::player_presenter) struct SaberModelAsset {
    pub(in crate::cgame::player_presenter) glm: Arc<GlmModel>,
    pub(in crate::cgame::player_presenter) gla: Arc<GlaAnimation>,
    pub(in crate::cgame::player_presenter) surfaces: Vec<PlayerSurfaceAsset>,
}
