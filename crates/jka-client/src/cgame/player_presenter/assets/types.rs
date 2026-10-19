//! Assets types.
use crate::cgame::player_presenter::{
    Arc, DynamicModelAlphaMode, Ghoul2GpuVertex, GlaAnimation, GlmModel, JiggleProfile,
    SourceMaterialParams, TcMod, TextureData, Wave,
};

#[derive(Debug)]
pub(in crate::cgame::player_presenter) struct Ghoul2GpuMeshSource {
    /// Original GLM geometry key/topology used whenever jiggle is off.
    pub(in crate::cgame::player_presenter) base_key: Arc<str>,
    pub(in crate::cgame::player_presenter) base_vertices: Arc<Vec<Ghoul2GpuVertex>>,
    pub(in crate::cgame::player_presenter) cpu_indices: Arc<Vec<u32>>,
    /// GPU topology for this authored LOD: two, one, or no local promotion passes.
    pub(in crate::cgame::player_presenter) key: Arc<str>,
    pub(in crate::cgame::player_presenter) vertices: Arc<Vec<Ghoul2GpuVertex>>,
    pub(in crate::cgame::player_presenter) indices: Arc<Vec<u32>>,
}

#[derive(Clone)]
pub(in crate::cgame::player_presenter) struct SurfaceOverlay {
    pub(in crate::cgame::player_presenter) texture: Option<Arc<TextureData>>,
    pub(in crate::cgame::player_presenter) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::cgame::player_presenter) authored_glow: bool,
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
    /// Only lightingDiffuse generators use entity lighting; identity/const/wave
    /// stages preserve their authored color without another light-grid multiply.
    pub(in crate::cgame::player_presenter) unlit: bool,
    /// Stage generators run at shader time, not model-load time.
    pub(in crate::cgame::player_presenter) rgb_wave: Option<Wave>,
    pub(in crate::cgame::player_presenter) alpha_wave: Option<Wave>,
    /// `alphaGen lightingSpecular`: alpha is q3's specular term per vertex.
    pub(in crate::cgame::player_presenter) specular_alpha: bool,
    /// Authored affine tcMods evaluated at the current shader time.
    /// Position-dependent turbulence remains unsupported on this path.
    pub(in crate::cgame::player_presenter) tc_mods: Vec<TcMod>,
    pub(in crate::cgame::player_presenter) authored_glow: bool,
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
    /// Authored `cull twosided` / `cull none` / `cull disable`, independent
    /// of alpha test and blend mode. Ghoul2 pipelines cull back faces, so
    /// two-sided meshes receive reversed-winding triangles at load time.
    pub(in crate::cgame::player_presenter) two_sided: bool,
    /// Exact `glow` marker for the single-stage/base path.
    pub(in crate::cgame::player_presenter) authored_glow: bool,
    /// OpenJK renderer `shader->timeOffset`, in seconds.
    pub(in crate::cgame::player_presenter) shader_time_offset: f32,
    pub(in crate::cgame::player_presenter) shader_clamp_time: Option<f32>,
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
    pub(in crate::cgame::player_presenter) authored_glow: bool,
    /// OpenJK renderer `shader->timeOffset`, in seconds.
    pub(in crate::cgame::player_presenter) shader_time_offset: f32,
    pub(in crate::cgame::player_presenter) shader_clamp_time: Option<f32>,
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
    /// Source 1 `$bumpmap`/`$phong` material. `None` for every GLM surface.
    pub(in crate::cgame::player_presenter) source_material: Option<SourceMaterialParams>,
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
    pub(in crate::cgame::player_presenter) body_shape_available: bool,
    pub(in crate::cgame::player_presenter) body_shape_binding: Option<Arc<jka_assets::body_shape::BodyShapeBinding>>,
    pub(in crate::cgame::player_presenter) key: String,
    pub(in crate::cgame::player_presenter) glm: Arc<GlmModel>,
    pub(in crate::cgame::player_presenter) gla: Arc<GlaAnimation>,
    /// Gameplay evaluator and bone overrides operate on this animation rig.
    /// `gla` above binds the mesh, bolts, ragdolls and GPU skin matrices.
    pub(in crate::cgame::player_presenter) animation_gla: Arc<GlaAnimation>,
    pub(in crate::cgame::player_presenter) retargeter: Option<Arc<jka_assets::skeleton::PoseRetargeter>>,
    /// Anatomical pivots for Source models or shaped native GLM players.
    pub(in crate::cgame::player_presenter) source_native_pivots: Option<Arc<Vec<[f32; 3]>>>,
    /// Extra procedural bones appended after GLA matrices for Source players.
    pub(in crate::cgame::player_presenter) source_jiggles: Option<Arc<Vec<jka_assets::source1_rig::SourceJiggleBinding>>>,
    pub(in crate::cgame::player_presenter) source_jiggles_enabled: bool,
    /// `players/_humanoid/` skeletons use the normal multiplayer animation/angle
    /// path.  Creature skeletons use the model-local tables cached below.
    pub(in crate::cgame::player_presenter) humanoid: bool,
    /// TaystJK `CG_G2SkelForModel`: non-humanoid GLA-adjacent animation.cfg.
    pub(in crate::cgame::player_presenter) animations:
        Option<Arc<jka_assets::animation::AnimationSet>>,
    /// TaystJK NPC registration: model-directory animevents.cfg (including
    /// humanoid NPCs; real players use the shared humanoid event table).
    pub(in crate::cgame::player_presenter) anim_events:
        Option<Arc<jka_assets::animevents::AnimEvents>>,
    pub(in crate::cgame::player_presenter) surfaces: Vec<PlayerSurfaceAsset>,
    /// Optional post-Ghoul2 soft-tissue profile loaded beside model.glm.
    pub(in crate::cgame::player_presenter) jiggle: Option<Arc<JiggleProfile>>,
    /// TaystJK/OpenJK `CG_LoadCISounds` gender derived from this model/skin's
    /// authored `sounds*.cfg`, not from a server-provided userinfo hint.
    pub(in crate::cgame::player_presenter) female: bool,
}

impl PlayerModelAsset {
    pub(in crate::cgame::player_presenter) fn shares_mesh_pose_binding(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.gla, &other.gla)
            && self.source_jiggles_enabled == other.source_jiggles_enabled
            && match (&self.source_jiggles, &other.source_jiggles) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
            && match (&self.source_native_pivots, &other.source_native_pivots) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }

    /// Convert once before every consumer of the final mesh pose. The stock
    /// JKA path returns its original matrices without conversion or allocation.
    pub(in crate::cgame::player_presenter) fn bind_animation_pose(
        &self, mut pose: Vec<crate::cgame::player_presenter::Matrix3x4>,
    ) -> Result<Vec<crate::cgame::player_presenter::Matrix3x4>, String> {
        if let Some(plan) = &self.retargeter {
            pose = plan.retarget(jka_assets::skeleton::PoseInput::Ghoul2SkinMatrices(&pose))?;
        }
        self.bind_mesh_pose(pose)
    }

    pub(in crate::cgame::player_presenter) fn bind_mesh_pose(
        &self, pose: Vec<crate::cgame::player_presenter::Matrix3x4>,
    ) -> Result<Vec<crate::cgame::player_presenter::Matrix3x4>, String> {
        let mut pose = if let Some(binding) = &self.body_shape_binding {
            binding.retarget(&self.gla, &pose)?
        } else if let Some(pivots) = &self.source_native_pivots {
            jka_assets::source1_rig::rebase_source_ghoul2_pose(&self.gla, &pose, pivots)?
        } else {
            pose
        };
        // An ordinary Source/GLM animation never has its own jiggle palette.
        // Copy each extra bone's mapped-parent skin matrix as the neutral
        // fallback for previews, offscreen models, and physics-disabled mode.
        // The per-entity animator later replaces these entries in place.
        if let Some(bindings) = &self.source_jiggles {
            let gla_bones = self.gla.skeleton.len();
            if pose.len() != gla_bones {
                return Err(format!("Source pose has {} bones, expected {gla_bones}", pose.len()));
            }
            for binding in bindings.iter() {
                let matrix = *pose.get(binding.parent_gla)
                    .ok_or("Source jiggle parent outside GLA")?;
                pose.push(matrix);
            }
        }
        Ok(pose)
    }
}

pub(in crate::cgame::player_presenter) struct SaberModelAsset {
    pub(in crate::cgame::player_presenter) glm: Arc<GlmModel>,
    pub(in crate::cgame::player_presenter) gla: Arc<GlaAnimation>,
    pub(in crate::cgame::player_presenter) animation_gla: Arc<GlaAnimation>,
    pub(in crate::cgame::player_presenter) retargeter: Option<Arc<jka_assets::skeleton::PoseRetargeter>>,
    /// Vehicle Ghoul2 animations are authored in the animation.cfg beside the
    /// GLA (TaystJK CG_G2SkelForModel), not in the humanoid global table.
    /// Non-vehicle static GLMs remain free of this extra asset load.
    pub(in crate::cgame::player_presenter) animations: Option<Arc<jka_assets::animation::AnimationSet>>,
    pub(in crate::cgame::player_presenter) surfaces: Vec<PlayerSurfaceAsset>,
}

impl SaberModelAsset {
    pub(in crate::cgame::player_presenter) fn bind_animation_pose(
        &self, pose: Vec<crate::cgame::player_presenter::Matrix3x4>,
    ) -> Result<Vec<crate::cgame::player_presenter::Matrix3x4>, String> {
        match &self.retargeter {
            Some(plan) => plan.retarget(jka_assets::skeleton::PoseInput::Ghoul2SkinMatrices(&pose)),
            None => Ok(pose),
        }
    }
}
