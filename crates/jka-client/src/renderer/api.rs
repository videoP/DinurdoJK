//! Api.
use crate::renderer::{
    scene, ui, Arc, Camera, CloudRenderResolution, CloudType, ColorLutPreset, CullDebugMode,
    DofMode, DofQuality, Duration, DynamicLightsMode, DynamicShadowsMode,
    EntityAmbientLightingMode, EntityShadowLight, FogMode, FootprintMode, Ghoul2BatchMode,
    Ghoul2SkinningMode, Hash, InputLatencySample, Instant, JumpShadeState, PathBuf, PhysicalSize,
    PlanarReflectionDebugMode, Pod, PreparedMap, PuddleQuality, PvsMode, RainIntensity,
    RayTracedRigidMeshKey, ReflectionQuality, SunVisibilityMode, SurfaceDeformationStamp,
    TextureData, TextureFilter, UiSnapshot, Vec3, ViewLatchMode, VsyncMode, Window, Zeroable,
};
use crate::renderer::{GrassDrawStats, ViewSampleMeasurement};
use crate::ui::GlowMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DynamicModelAlphaMode {
    Opaque,
    Mask,
    Blend,
    /// Authored blended stage with explicit `depthWrite` (TaystJK GLS_DEPTHMASK_TRUE).
    /// Still uses SRC_ALPHA / ONE_MINUS_SRC_ALPHA, but seeds depth in the
    /// see-through pass before ordinary additive overlays.
    BlendDepthWrite,
    /// Depth-writing SRC_ALPHA/ONE_MINUS_SRC_ALPHA stage with unlit rgbGen
    /// (identity, wave, const). Same authored blend/depth as BlendDepthWrite,
    /// but skip dynamic entity lighting.
    BlendUnlitDepthWrite,
    /// Alpha-tested material plus OpenJK RF_FORCE_ENT_ALPHA semantics.
    MaskBlend,
    /// Unlit GL_ONE GL_ONE additive stage. Alpha does not scale source RGB.
    AdditiveOne,
    /// Unlit GL_SRC_ALPHA GL_ONE additive presentation primitive.
    Additive,
    /// Unlit GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA (FX smoke, sprites).
    BlendUnlit,
    /// GL_DST_COLOR GL_ZERO: multiplies what is already drawn (FX filters).
    Modulate,
    /// GL_DST_COLOR GL_ONE: source*destination + destination.
    DstColorAdd,
    /// GL_DST_COLOR GL_SRC_COLOR: JKA 2x modulation (2 * source * destination).
    Modulate2x,
    /// GL_ZERO GL_ONE_MINUS_SRC_COLOR.
    Darken,
    /// Promoted `q3map_material Glass`, sampled from the opaque scene capture.
    Glass,
}

impl DynamicModelAlphaMode {
    pub(in crate::renderer) const COUNT: usize = 14;

    /// Dense index for per-alpha-mode pipeline tables and draw bitmasks.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Opaque => 0,
            Self::Mask => 1,
            Self::Blend => 2,
            Self::MaskBlend => 3,
            Self::AdditiveOne => 4,
            Self::Additive => 5,
            Self::BlendUnlit => 6,
            Self::Modulate => 7,
            Self::DstColorAdd => 8,
            Self::Modulate2x => 9,
            Self::Darken => 10,
            Self::BlendDepthWrite => 11,
            Self::BlendUnlitDepthWrite => 12,
            Self::Glass => 13,
        }
    }
}

pub(in crate::renderer) const DYNAMIC_MODEL_ALPHA_ORDER: [DynamicModelAlphaMode; 14] = [
    DynamicModelAlphaMode::Opaque,
    DynamicModelAlphaMode::Mask,
    DynamicModelAlphaMode::BlendDepthWrite,
    DynamicModelAlphaMode::BlendUnlitDepthWrite,
    DynamicModelAlphaMode::MaskBlend,
    DynamicModelAlphaMode::Modulate,
    DynamicModelAlphaMode::DstColorAdd,
    DynamicModelAlphaMode::Modulate2x,
    DynamicModelAlphaMode::Darken,
    DynamicModelAlphaMode::Blend,
    DynamicModelAlphaMode::Glass,
    DynamicModelAlphaMode::BlendUnlit,
    DynamicModelAlphaMode::AdditiveOne,
    DynamicModelAlphaMode::Additive,
];

/// Dynamic surfaces come from two geometry backends: transient CPU meshes
/// (including FX such as saber blades) and GPU-skinned Ghoul2 meshes. The
/// depth-writing phase must finish across *both* backends before any blended
/// surface is submitted. Otherwise a CPU additive effect drawn first can be
/// overwritten later by an opaque GPU-skinned player even when the effect is
/// physically in front of the player.
#[inline]
pub(in crate::renderer) fn dynamic_model_writes_depth(alpha_mode: DynamicModelAlphaMode) -> bool {
    matches!(
        alpha_mode,
        DynamicModelAlphaMode::Opaque
            | DynamicModelAlphaMode::Mask
            | DynamicModelAlphaMode::BlendDepthWrite
            | DynamicModelAlphaMode::BlendUnlitDepthWrite
    )
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DynamicModelVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// OpenJK refEntity shaderRGBA equivalent. Player presentation currently
    /// uses white RGB and the CG_CheckThirdPersonAlpha alpha channel.
    pub color: [f32; 4],
    /// OpenJK RF_DEPTHHACK: apply the legacy weapon depth-range semantics.
    /// `md3.wgsl` performs the reverse-Z equivalent clip-space remap.
    pub depth_hack: f32,
    /// CPU/RT skinned per-vertex immersion, interpolated per fragment.
    pub water_wetness: f32,
}

/// `ghoul2_skin_compute.wgsl` materializes this vertex through a raw `array<u32>`
/// and so hard-codes the stride. Keep the two in step: a desync silently writes
/// every skinned vertex at the wrong offset and leaves the tail of each range
/// zeroed, i.e. collapsed onto the world origin.
pub(in crate::renderer) const DYNAMIC_MODEL_VERTEX_WORDS: usize = 14;
const _: () = assert!(
    std::mem::size_of::<DynamicModelVertex>() == DYNAMIC_MODEL_VERTEX_WORDS * 4,
    "DynamicModelVertex changed size; update write_vertex in ghoul2_skin_compute.wgsl"
);

/// Bind-pose GLM vertex used by the optional GPU Ghoul2 skinning path. Bone
/// indices are GLA/global indices, resolved from the GLM surface-local table at
/// model registration time so the shader does no hierarchy indirection.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Ghoul2GpuVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub bone_indices: [u32; 4],
    pub weights: [f32; 4],
    pub weight_count: u32,
    /// Strongest post-skin soft-tissue region for this vertex; u32::MAX = none.
    pub jiggle_region: u32,
    pub jiggle_weight: f32,
    /// 4.0 for chest/non-glute vertices; glutes store normalized bind-pose Z.
    pub jiggle_coord: f32,
}

/// Storage-buffer-safe repack used only by the RT-on compute skinning path.
/// Keeping vec4-sized fields avoids WGSL storage-layout padding ambiguity while
/// the normal RT-off raster path keeps its original compact vertex unchanged.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(in crate::renderer) struct Ghoul2RtInputVertex {
    pub(in crate::renderer) position_uv_x: [f32; 4],
    pub(in crate::renderer) normal_uv_y: [f32; 4],
    pub(in crate::renderer) bone_indices: [u32; 4],
    pub(in crate::renderer) weights: [f32; 4],
    pub(in crate::renderer) params: [u32; 4],
}

impl From<&Ghoul2GpuVertex> for Ghoul2RtInputVertex {
    fn from(vertex: &Ghoul2GpuVertex) -> Self {
        Self {
            position_uv_x: [
                vertex.position[0],
                vertex.position[1],
                vertex.position[2],
                vertex.uv[0],
            ],
            normal_uv_y: [
                vertex.normal[0],
                vertex.normal[1],
                vertex.normal[2],
                vertex.uv[1],
            ],
            bone_indices: vertex.bone_indices,
            weights: vertex.weights,
            params: [
                vertex.weight_count,
                vertex.jiggle_region,
                vertex.jiggle_weight.to_bits(),
                vertex.jiggle_coord.to_bits(),
            ],
        }
    }
}

/// One final CPU-evaluated Ghoul2 bone matrix. GPU mode intentionally keeps
/// animation, BG_G2PlayerAngles and bolt semantics on the CPU; only the GLM
/// vertex deformation moves to the vertex shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Ghoul2GpuBone {
    pub row0: [f32; 4],
    pub row1: [f32; 4],
    pub row2: [f32; 4],
}

/// A Source 1 `VertexLitGeneric` material's lighting extras: `$bumpmap` plus the
/// `$phong`/`$rimlight`/`$halflambert` block. Present only on surfaces built from
/// a VMT, which is what keeps every JKA model on the cheaper pipeline variant
/// that has no normal-map sample compiled into it at all.
///
/// The scalars are Valve's, already in the form `skin_ps20b.fxc` consumes them;
/// see `jka_client::cgame::player_presenter::source_vtf::VmtPhong`.
#[derive(Clone, Debug)]
pub struct SourceMaterialParams {
    /// Tangent-space normal map, decoded linear. `None` when the VMT declares
    /// phong but no `$bumpmap`: the surface then takes the same pipeline with a
    /// flat normal bound, so the phong still applies to the geometric normal.
    pub normal_map: Option<Arc<TextureData>>,
    /// `$phongboost`; 0 when the material has no `$phong`.
    pub phong_boost: f32,
    /// `$phongexponent`.
    pub phong_exponent: f32,
    /// `$phongfresnelranges`, packed as `((mid-min)*2, mid, (max-mid)*2)`.
    pub phong_fresnel: [f32; 3],
    /// `$rimlightboost`; 0 when the material has no `$rimlight`.
    pub rimlight_boost: f32,
    /// `$rimlightexponent`.
    pub rimlight_exponent: f32,
    /// `$halflambert`: the diffuse term becomes `(N.L * 0.5 + 0.5)^2`.
    pub half_lambert: bool,
}

#[derive(Clone, Debug)]
pub struct Ghoul2GpuSkinning {
    /// Stable geometry cache key, e.g. `models/players/kyle/model.glm#lod0#surface3`.
    pub mesh_key: Arc<str>,
    pub vertices: Arc<Vec<Ghoul2GpuVertex>>,
    pub indices: Arc<Vec<u32>>,
    /// Shared across all surfaces belonging to one evaluated pose.
    pub bones: Arc<Vec<Ghoul2GpuBone>>,
    /// JKA entity/model axis and origin. The shader converts the result to the
    /// renderer's [x,z,-y] coordinate convention after skinning.
    pub axis: [[f32; 3]; 3],
    pub origin: [f32; 3],
    pub color: [f32; 4],
    /// Texture-coordinate transform `uv * xy + zw`; [1, 1, 0, 0] is identity.
    pub uv_xform: [f32; 4],
    /// Off-diagonal UV coefficients; zero on the ordinary fast path.
    pub uv_cross: [f32; 4],
    /// `alphaGen lightingSpecular`: JKA world-space (light, viewer) points. The
    /// shader multiplies the draw alpha by q3's `(R.V)^4` specular term.
    pub specular: Option<([f32; 3], [f32; 3])>,
    /// `deformVertexes bulge <0> <height> <0>` (JKA's static-offset special
    /// case; see `jka_assets::shader::Bulge::is_static`): a constant JKA-unit
    /// offset along the model-space vertex normal, applied before the entity
    /// transform. Zero for ordinary surfaces. Packed into the GPU draw's
    /// otherwise-unused `spec_viewer.w` (mutually exclusive with specular).
    pub bulge_height: f32,
    /// `tcGen environment` (e.g. `gfx/misc/personalshield`'s chrome stage):
    /// replaces `uv_xform`'s mapping of the base mesh UVs with q3's reflection-
    /// vector environment mapping (`RB_CalcEnvironmentTexCoords`).
    pub env_map: bool,
    /// Up to five model-space post-skin region offsets. Ordinary Ghoul2 draws
    /// keep these zero; jiggle vertices select one through `jiggle_region`.
    pub jiggle_offsets: [[f32; 4]; crate::cgame::jiggle::MAX_GPU_JIGGLE_REGIONS],
    /// Source 1 bump/phong material, if this surface came from a VMT. `None` for
    /// every JKA surface, and the renderer keys its pipeline choice off that.
    pub source_material: Option<SourceMaterialParams>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DynamicWireframeClass {
    Player,
    Entity,
    Effect,
}

impl DynamicWireframeClass {
    #[inline]
    pub(in crate::renderer) fn mask_bit(self) -> u32 {
        match self {
            Self::Player => ui::wireframe::PLAYERS,
            Self::Entity => ui::wireframe::ENTITIES,
            Self::Effect => ui::wireframe::EFFECTS,
        }
    }
}

#[derive(Clone)]
pub struct RayTracedRigidInstance {
    /// Stable asset identity shared by every instance of this MD3. The RT cache
    /// combines this with surface/frame so one object-space BLAS can be reused
    /// by any number of TLAS instances.
    pub(in crate::renderer) asset_key: Arc<str>,
    pub(in crate::renderer) model: Arc<jka_assets::md3::Model>,
    pub(in crate::renderer) surface_index: u32,
    pub(in crate::renderer) frame: u32,
    /// Object-space JKA MD3 -> renderer world-space affine transform, stored in
    /// wgpu/Vulkan TLAS row-major 3x4 form.
    pub(in crate::renderer) transform: [f32; 12],
}

impl RayTracedRigidInstance {
    pub fn md3(
        asset_key: Arc<str>,
        model: Arc<jka_assets::md3::Model>,
        surface_index: usize,
        frame: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        scale: f32,
    ) -> Self {
        // EntityPresenter's raster path computes:
        // render_position(origin + axis0*x + axis1*y + axis2*z).  Keep the
        // BLAS in native MD3/JKA object space and put that exact linear map in
        // the TLAS instance instead of rebuilding geometry for transform-only
        // motion.
        let c0 =
            scene::render_position([axis[0][0] * scale, axis[0][1] * scale, axis[0][2] * scale]);
        let c1 =
            scene::render_position([axis[1][0] * scale, axis[1][1] * scale, axis[1][2] * scale]);
        let c2 =
            scene::render_position([axis[2][0] * scale, axis[2][1] * scale, axis[2][2] * scale]);
        let translation = scene::render_position(origin);
        Self {
            asset_key,
            model,
            surface_index: u32::try_from(surface_index).unwrap_or(u32::MAX),
            frame: u32::try_from(frame).unwrap_or(u32::MAX),
            transform: [
                c0[0],
                c1[0],
                c2[0],
                translation[0],
                c0[1],
                c1[1],
                c2[1],
                translation[1],
                c0[2],
                c1[2],
                c2[2],
                translation[2],
            ],
        }
    }

    pub(in crate::renderer) fn mesh_key(&self, non_opaque: bool) -> RayTracedRigidMeshKey {
        RayTracedRigidMeshKey {
            asset_key: Arc::clone(&self.asset_key),
            surface_index: self.surface_index,
            frame: self.frame,
            non_opaque,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FxGpuSpriteInstance {
    /// Render-space center. W is padding for 16-byte instance attributes.
    pub origin: [f32; 4],
    /// Render-space half-extent vector after JKA billboard rotation.
    pub left: [f32; 4],
    /// Render-space half-extent vector after JKA billboard rotation.
    pub up: [f32; 4],
    /// Final per-stage shaderRGBA after rgbGen/alphaGen.
    pub color: [f32; 4],
}

#[derive(Clone)]
pub struct FxGpuSprites {
    pub instances: Arc<Vec<FxGpuSpriteInstance>>,
    /// Optional GPU-resident spark draw request. `instances` is the
    /// known-correct CPU-generated fallback used during lazy PSO compilation.
    pub resident: Option<FxGpuResidentSprites>,
    /// Instances are player blob-shadow requests, not billboards. The renderer
    /// clips the rendered floor into each request (`Renderer::build_blob_shadow_marks`,
    /// OpenJK R_MarkFragments) and draws the resulting polygons as one ordinary
    /// dynamic mesh. origin = center on the collision plane (w = shade), left/up =
    /// in-plane half axes of length `radius` (left.w = entity number).
    pub blob_shadow: bool,
}

/// One material-stage draw against stable resident slot IDs. Each surface
/// holds an Arc to the once-per-frame spawn delta; the backend applies it only
/// once, irrespective of the number of shader stages.
#[derive(Clone)]
pub struct FxGpuResidentSprites {
    pub frame: Arc<FxGpuResidentFrame>,
    pub slots: Arc<Vec<u32>>,
}

#[derive(Clone, Default)]
pub struct FxGpuResidentFrame {
    pub reset: bool,
    pub time_ms: i32,
    pub show_saber_impact: bool,
    /// New or reused slots only; every GPU record is uploaded once per birth.
    pub writes: Arc<Vec<(u32, crate::fx::gpu_sparks::GpuSparkGpuState)>>,
    /// The renderer publishes *actually compiled* resident alpha pipelines here.
    /// Never infer readiness from a requested compilation job. The shared mask
    /// is cleared on pipeline rebuild or on disabling GPU residency.
    pub ready_modes: Arc<std::sync::atomic::AtomicU16>,
}

/// Presentation provenance. This is diagnostic metadata, not material state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ModelGeometrySource {
    Md3,
    Ghoul2,
    Fx,
    BlobShadow,
    Other,
}

#[derive(Clone)]
pub struct DynamicModelSurface {
    pub geometry_source: ModelGeometrySource,
    /// Authored `rgbGen lightingDiffuse`: request TaystJK entity lightgrid
    /// for this stage even when the generic ambient-mode shortcut is Off.
    pub authored_lighting: bool,
    /// Shared, immutable source-to-shader diagnostic for MD3 surfaces.
    /// Kept separate from runtime material state; cloning is an Arc increment.
    pub material_trace: Option<Arc<str>>,
    /// Presentation identity retained for diagnostics/culling work and dynamic
    /// RT BLAS ownership.
    pub entity_num: u16,
    pub wireframe_class: DynamicWireframeClass,
    /// False for conservative RT-only Ghoul2 casters that were rejected by the
    /// main-camera frustum. They still feed skinning/BLAS but never color/depth.
    pub raster_visible: bool,
    pub vertices: Arc<Vec<DynamicModelVertex>>,
    pub indices: Arc<Vec<u32>>,
    /// JKA-space point used for classic BSP entity-lighting lookup. Effects and
    /// other deliberately-unlit transient geometry leave this unset.
    pub lighting_origin: Option<[f32; 3]>,
    /// Transform-only rigid source for hardware RT shadow casting. This is
    /// deliberately absent for FX and deforming/Ghoul2 geometry.
    pub rt_rigid: Option<RayTracedRigidInstance>,
    /// Stable deforming GLM surface identity. Present only while RT Shadows is
    /// active so CPU- and GPU-skinned Ghoul2 can share one dynamic BLAS path.
    pub rt_skinned_key: Option<Arc<str>>,
    /// Present only for r_ghoul2Skinning gpu. CPU geometry keeps this None.
    pub ghoul2_gpu: Option<Ghoul2GpuSkinning>,
    /// Instanced RT_SPRITE path used by r_fxGeometry gpu. Non-sprite EFX and
    /// the CPU reference modes leave this unset.
    pub fx_gpu_sprites: Option<FxGpuSprites>,
    /// Exact JKA shader-stage `glow` marker. Synthetic/modern FX leave this false.
    pub authored_glow: bool,
    pub texture: Option<Arc<TextureData>>,
    pub alpha_mode: DynamicModelAlphaMode,
}

impl DynamicModelSurface {
    pub fn vertex_count(&self) -> usize {
        if let Some(sprites) = self.fx_gpu_sprites.as_ref() {
            return sprites.instances.len().saturating_mul(4);
        }
        self.ghoul2_gpu
            .as_ref()
            .map_or(self.vertices.len(), |skin| skin.vertices.len())
    }

    pub fn index_count(&self) -> usize {
        if let Some(sprites) = self.fx_gpu_sprites.as_ref() {
            return sprites.instances.len().saturating_mul(6);
        }
        self.ghoul2_gpu
            .as_ref()
            .map_or(self.indices.len(), |skin| skin.indices.len())
    }

}

/// CGame's CG_Mover submission of an inline BSP model: R_AddRefEntityToScene
/// with `hModel = cgs.inlineDrawModel[modelindex]`, in JKA world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineModelInstance {
    pub model: u32,
    pub origin: [f32; 3],
    /// AnglesToAxis(lerpAngles): forward, left, up.
    pub axis: [[f32; 3]; 3],
    /// TaystJK entityState_t.isPortalEnt: this *instance*, not its shared BSP
    /// model handle, is eligible for CG_DrawSkyBoxPortal's entity pass.
    pub is_portal_entity: bool,
}

impl InlineModelInstance {
    /// The compiled pose, used when no CGame snapshot owns the movers.
    pub(in crate::renderer) fn compiled(model: u32) -> Self {
        Self {
            model,
            origin: [0.0; 3],
            axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            is_portal_entity: false,
        }
    }

    /// Render-space position of a compiled render-space point.
    pub(in crate::renderer) fn transform_point(&self, render: [f32; 3]) -> [f32; 3] {
        let local = scene::jka_position(render);
        let world = self.rotate(local);
        scene::render_position([
            world[0] + self.origin[0],
            world[1] + self.origin[1],
            world[2] + self.origin[2],
        ])
    }

    pub(in crate::renderer) fn transform_normal(&self, render: [f32; 3]) -> [f32; 3] {
        // Local editor instances may carry uniform scale in the axis vectors.
        // Normals must remain unit-length regardless of the placement scale.
        let rotated = scene::render_position(self.rotate(scene::jka_position(render)));
        let length_sq = rotated.iter().map(|value| value * value).sum::<f32>();
        if length_sq > 1.0e-20 {
            let inv = length_sq.sqrt().recip();
            rotated.map(|component| component * inv)
        } else {
            rotated
        }
    }

    pub(crate) fn rotate(&self, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| {
            self.axis[0][i] * v[0] + self.axis[1][i] * v[1] + self.axis[2][i] * v[2]
        })
    }

    /// Affine transform used by a TLAS instance whose BLAS vertices are the
    /// compiled inline-model `base` vertices in renderer space. Derive it from
    /// the same authoritative point transform used by raster movers so the two
    /// representations cannot drift on JKA<->render axis conversion.
    pub(in crate::renderer) fn rt_transform(&self) -> [f32; 12] {
        let p0 = self.transform_point([0.0, 0.0, 0.0]);
        let px = self.transform_point([1.0, 0.0, 0.0]);
        let py = self.transform_point([0.0, 1.0, 0.0]);
        let pz = self.transform_point([0.0, 0.0, 1.0]);
        let c0 = [px[0] - p0[0], px[1] - p0[1], px[2] - p0[2]];
        let c1 = [py[0] - p0[0], py[1] - p0[1], py[2] - p0[2]];
        let c2 = [pz[0] - p0[0], pz[1] - p0[1], pz[2] - p0[2]];
        [
            c0[0], c1[0], c2[0], p0[0], c0[1], c1[1], c2[1], p0[1], c0[2], c1[2], c2[2], p0[2],
        ]
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClientFramePerf {
    pub total_ms: f64,
    pub snapshot_ms: f64,
    pub audio_ms: f64,
    pub events_ms: f64,
    /// Side-effect-free event semantic preparation. Included in `events_ms`.
    pub event_prepare_ms: f64,
    pub event_worker_jobs: u32,
    pub event_worker_threads: u32,
    pub event_worker_parallel: bool,
    /// Uncached direct SFX codec work staged from event batches.
    pub event_sound_decode_ms: f64,
    pub event_sound_decode_jobs: u32,
    pub event_sound_decode_parallel: bool,
    pub entity_present_ms: f64,
    pub player_present_ms: f64,
    pub followed_player_ms: f64,
    pub fx_ms: f64,
    pub fx_tessellate_ms: f64,
    pub fx_draws: u32,
    pub fx_sprites: u32,
    pub fx_oriented_quads: u32,
    pub fx_lines: u32,
    pub fx_quads: u32,
    pub fx_meshes: u32,
    pub fx_cylinders: u32,
    pub fx_render_surfaces: u32,
    pub fx_cpu_geom_surfaces: u32,
    pub fx_cpu_vertices: u64,
    pub fx_cpu_indices: u64,
    pub fx_gpu_sprite_batches: u32,
    pub fx_gpu_sprite_instances: u32,
    pub ghoul2_pose_ms: f64,
    pub ghoul2_motion_pose_ms: f64,
    pub ghoul2_skin_ms: f64,
    pub ghoul2_skinning_mode: Ghoul2SkinningMode,
    pub ghoul2_bolt_ms: f64,
    pub ghoul2_pose_evals: u32,
    pub ghoul2_motion_pose_evals: u32,
    pub ghoul2_bolt_queries: u32,
    pub ghoul2_surfaces: u32,
    pub ghoul2_vertices: u64,
    pub ghoul2_frustum_tests: u32,
    pub ghoul2_frustum_culled: u32,
    pub ghoul2_lod_counts: [u32; 4],
    pub dynamic_surfaces: u32,
    pub dynamic_vertices: u64,
    pub dynamic_indices: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::renderer) struct LegacyDlightPerfStats {
    pub(in crate::renderer) transient_count: usize,
    pub(in crate::renderer) saber_sources: usize,
    pub(in crate::renderer) authored_fx_sources: usize,
    pub(in crate::renderer) promoted_fx_sources: usize,
    pub(in crate::renderer) saber_mark_sources: usize,
    pub(in crate::renderer) radius_sum: f64,
    pub(in crate::renderer) radius_max: f32,
    pub(in crate::renderer) surfaces_total: usize,
    pub(in crate::renderer) surfaces_eligible: usize,
    pub(in crate::renderer) surfaces_touched: usize,
    pub(in crate::renderer) candidate_pairs: u64,
    pub(in crate::renderer) saber_pairs: u64,
    pub(in crate::renderer) authored_fx_pairs: u64,
    pub(in crate::renderer) promoted_fx_pairs: u64,
    pub(in crate::renderer) saber_mark_pairs: u64,
    pub(in crate::renderer) max_candidates_per_surface: u32,
    pub(in crate::renderer) surface_buckets: [u32; 6],
    pub(in crate::renderer) max_surfaces_per_light: u32,
    pub(in crate::renderer) total_surfaces_per_light: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransientLight {
    /// Diagnostic-only provenance copied from the CGame/FX submission.
    pub kind: crate::fx::system::FxLightKind,
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub radius: f32,
    /// Physical emitter radius for stochastic hardware-RT soft shadows.
    /// This does not change raster attenuation/range.
    pub source_radius: f32,
    pub intensity: f32,
    /// Local projected-light cone. Direction is in renderer space.
    pub spot: Option<crate::fx::system::FxSpotLight>,
    pub segment: Option<[[f32; 3]; 2]>,
    pub blade_segments: Option<Arc<[crate::fx::system::FxLightSegment]>>,
}

impl TransientLight {
    /// Shared FX-to-renderer conversion for gameplay and menu scenes.
    pub(crate) fn from_fx(light: &crate::fx::system::FxLight) -> Option<Self> {
        if !light.radius.is_finite() || light.radius <= 0.0
            || !light.origin.iter().all(|value| value.is_finite())
            || !light.rgb.iter().all(|value| value.is_finite())
        {
            return None;
        }
        Some(Self {
            kind: light.kind,
            position: scene::render_position(light.origin),
            color: light.rgb.map(|value| value.max(0.0)),
            radius: light.radius,
            source_radius: light.source_radius.max(0.0),
            // OpenJK's FX primitive passes RGB plus radius to
            // RE_AddLightToScene; the renderer's point-light intensity
            // scale of 1.0 is the matching neutral source strength.
            intensity: 1.0,
            spot: light.spot.map(|spot| crate::fx::system::FxSpotLight {
                direction: scene::render_position(spot.direction),
                ..spot
            }),
            segment: light.segment.map(|ends| ends.map(scene::render_position)),
            blade_segments: light.blade_segments.as_ref().map(|blades| {
                blades
                    .iter()
                    .map(|blade| crate::fx::system::FxLightSegment {
                        endpoints: blade.endpoints.map(scene::render_position),
                        ..*blade
                    })
                    .collect::<Vec<_>>()
                    .into()
            }),
        })
    }
}

/// A lit saber blade in renderer space. The blade is additive FX light that no
/// depth buffer knows about, so the cloud composite is told where it is and
/// keeps its own transmittance and radiance off those pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudForegroundBlade {
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub radius: f32,
}

/// Most blades the cloud composite exempts. A saber has up to eight blades, and
/// the nearest ones win when a scene has more.
pub(in crate::renderer) const MAX_CLOUD_FOREGROUND_BLADES: usize = 8;

/// Team colour used by the spectator/demo through-wall outline pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectatorHighlightTeam {
    Red,
    Blue,
    Yellow,
}

/// One player entity to outline for spectator/demo viewing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectatorHighlight {
    pub entity_num: u16,
    pub team: SpectatorHighlightTeam,
}

#[derive(Clone)]
pub struct RenderSnapshot {
    pub model_frame_source: Option<crate::model_frame_log::FrameSource>,
    pub camera: Camera,
    /// Mapping from latest local-player input orientation to this render camera.
    pub view_latch: ViewLatchMode,
    /// Latest local input yaw/pitch in renderer radians. This is deliberately
    /// separate from `camera`: third-person camera rotation is derived output.
    pub input_view_rotation: Option<[f32; 2]>,
    /// Dynamic-crosshair world endpoint paired with the input orientation.
    /// High-rate subframe updates mirror this into `LatestViewState`.
    pub dynamic_crosshair_world: Option<[f32; 3]>,
    pub player_position: Option<Vec3>,
    pub ocean_actors: Arc<Vec<crate::ocean::interaction::ActorSample>>,
    /// Current server snapshot portal-area mask. Set bits are areas closed off
    /// by areaportals/doors. `None` for solo/offline scenes with no snapshot.
    pub area_mask: Option<[u8; 32]>,
    pub dynamic_models: Arc<Vec<DynamicModelSurface>>,
    /// Per-player immersion history sampled on CGame snapshot transitions.
    pub player_water: Arc<std::collections::HashMap<u16, crate::water_wetness::WaterProfile>>,
    /// Players outlined through walls while spectating or watching demos.
    pub spectator_highlights: Arc<Vec<SpectatorHighlight>>,
    /// Short-lived RE_AddLightToScene-style sources submitted by CGame/FX.
    /// These stay separate from map-static lights so moving flashes do not
    /// churn the cached local-shadow selection.
    pub transient_lights: Arc<Vec<TransientLight>>,
    /// Authored CGame screen-space stages, in 640x480 virtual coordinates.
    pub screen_fx: Arc<Vec<crate::fx::draw::ScreenFxDraw>>,
    /// Lit saber blades the volumetric clouds must not composite over.
    pub cloud_foreground: Arc<Vec<CloudForegroundBlade>>,
    /// `None`: no CGame owns the scene (solo/offline), so inline models keep
    /// their compiled pose. `Some`: exactly the movers in the current
    /// snapshot are drawn, as OpenJK only draws bmodels CGame submits.
    pub inline_models: Option<Arc<Vec<InlineModelInstance>>>,
    /// Optional second spectator camera for an auxiliary native surface. It
    /// references only data already present in this snapshot; the renderer does
    /// not perform a second visibility discovery from that camera.
    pub companion_scene: Option<CompanionSceneView>,
    pub dof_focus_target: f32,
    pub input_latency: Option<InputLatencySample>,
    pub client_perf: ClientFramePerf,
    /// Small Copy-only HUD state that changes with simulation/input. This rides
    /// the existing latest-snapshot mailbox instead of the retained UI command
    /// queue, so high-rate HUD updates never clone console/menu state.
    pub hud: Option<ui::HudState>,
    pub movement_hud: ui::MovementHudState,
    /// World-space TaystJK player labels. Visibility is app-thread sampled;
    /// projection is render-thread late-latched every frame.
    pub player_names: Option<ui::UiWorldPlayerNames>,
    /// jaPRO SP-physics jump-height helper. `Off` costs nothing in the world shader.
    pub jump_shade: JumpShadeState,
    /// Active `EV_SCREENSHAKE`/`CGCam_Shake`; the render thread rolls a fresh
    /// offset each frame so it composes with the late-latched view.
    pub camera_shake: Option<crate::camera::CameraShake>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PostEffects {
    pub hdr: bool,
    pub hdr_paper_white: f32,
    pub hdr_peak: f32,
    pub auto_exposure: f32,
    pub tone_mapping: f32,
    pub glow: GlowMode,
    pub halation: bool,
    pub ssao: bool,
    pub static_bsp_ao: bool,
    pub static_bsp_ao_lightmap: bool,
    pub static_bsp_ao_samples: u32,
    pub static_bsp_ao_resolution: u32,
    pub static_bsp_ao_strength: u32,
    pub static_bsp_ao_range: u32,
    pub static_bsp_ao_current_cell: bool,
    pub fxaa: bool,
    pub smaa: bool,
    pub taa: bool,
    pub rt_taa: u32,
    pub contact_shadows: bool,
    pub fog_mode: FogMode,
    pub fog_strength: f32,
    pub sun_override: bool,
    pub sun_yaw: f32,
    pub sun_pitch: f32,
    pub sun_intensity: f32,
    pub sun_shadow_strength: f32,
    pub sun_lighting: bool,
    pub sun_color: [f32; 3],
    pub sun_visibility: SunVisibilityMode,
    pub entity_sun_lighting: bool,
    /// 0..1: estimated subtraction of authored direct sunlight from BSP baked lighting.
    pub baked_sun_removal: f32,
    pub baked_sun_cache: bool,
    pub baked_sun_shadow_map: bool,
    pub baked_sun_edge_softness: f32,
    pub baked_sun_direct_limit: f32,
    pub baked_indirect_scale: f32,
    pub baked_indirect_saturation: f32,
    /// Nonlinear dimming of low irradiance: 1.0 = disabled.
    pub baked_indirect_power: f32,
    /// Keep high-luminance baked pools bright (heuristic; no light-source separation).
    pub baked_indirect_highlight_preserve: f32,
    pub sky_fractal_override: bool,
    pub sky_backend: u8,
    pub blue_water_environment: crate::blue_water_environment::Settings,
    pub cloud_backend: u8,
    pub cloud_portal_depth: u8,
    pub clouds: bool,
    pub cloud_type: CloudType,
    pub cloud_quality: f32,
    pub cloud_coverage: f32,
    pub cloud_height: f32,
    pub cloud_thickness: f32,
    pub weather_wind: crate::ocean::AtmosphericWind,
    pub wind_visuals: f32,
    pub cloud_shadows: bool,
    pub cloud_render_resolution: CloudRenderResolution,
    pub cloud_temporal: bool,
    pub cloud_temporal_depth_fix: bool,
    pub cloud_shear: f32,
    pub cloud_base_variation: f32,
    pub cloud_shape_evolution: bool,
    pub cloud_terrain_interaction: bool,
    pub cloud_empty_skip: bool,
    pub cloud_aerial: f32,
    pub cloud_sky_ambient: bool,
    pub cloud_history_blend: f32,
    pub cloud_motion_reject: f32,
    pub cloud_history_depth_reject: bool,
    pub cloud_thickness_variation: f32,
    pub cloud_size: f32,
    pub rain: bool,
    pub rain_intensity: RainIntensity,
    /// Compatibility multiplier for stacked JKA `r_we` rain clouds.
    pub rain_density_scale: f32,
    /// RGB multiplier. Acid rain uses the stock green color; normal rain is 1.
    pub rain_tint: [f32; 3],
    /// `r_we freeze` freezes only renderer world-effect rain, not the saved user rain cvar.
    pub rain_frozen: bool,
    /// Retail renderer-world snow. One density unit corresponds to one 1,000-flake cloud.
    pub snow: bool,
    pub snow_density_scale: f32,
    pub snow_frozen: bool,
    /// Legacy `r_we sand` cloud layers. Zero keeps its renderer path dormant.
    pub legacy_sand_layers: u32,
    /// Sum of positive `r_we spacedust <count>` cloud requests.
    pub legacy_space_dust_count: u32,
    /// `r_we freeze` also freezes procedural sand/spacedust advection.
    pub legacy_weather_frozen: bool,
    /// 0..1 toxic atmospheric treatment for the selected legacy acid-rain cloud.
    pub legacy_acid_rain: f32,
    pub rain_glass: bool,
    pub puddle_quality: PuddleQuality,
    pub puddle_scatter: f32,
    pub rain_grade: f32,
    pub reflection_quality: ReflectionQuality,
    pub reflection_debug: bool,
    pub chromatic_aberration: f32,
    pub vignette: bool,
    pub film_grain_strength: f32,
    pub motion_blur_strength: f32,
    pub depth_of_field_strength: f32,
    pub dof_mode: DofMode,
    pub crt_scope: ui::CrtScope,
    pub crt_style: ui::CrtStyle,
    pub crt_quality: ui::CrtQuality,
    pub crt_reference: ui::CrtReferenceResolution,
    pub crt_mask: ui::CrtMask,
    pub crt_scanline: f32,
    pub crt_sharpness: f32,
    pub crt_mask_intensity: f32,
    pub crt_curvature: f32,
    pub crt_corner: f32,
    pub dof_quality: DofQuality,
    pub color_lut: ColorLutPreset,
    pub color_lut_strength: f32,
    pub color_grade: crate::color_grading::ColorGradeSettings,
    pub split_toning: crate::color_grading::SplitToningSettings,
}

// Rend2 material profile locked after visual/performance testing. These values
// define the behavior used when the single runtime master switch (`r_pbr`) is ON.
// The individual profile knobs intentionally remain non-configurable.
pub const PBR_PROFILE_MATERIALS: bool = true;

pub const PBR_PROFILE_PARALLAX_OCCLUSION: bool = true;

pub const PBR_PROFILE_SHARED_MATERIAL_EVAL: bool = false;

pub const PBR_PROFILE_SHARED_TANGENT_FRAME: bool = false;

pub const PBR_PROFILE_POM_MIP_AWARE: bool = true;

pub const PBR_PROFILE_POM_ADAPTIVE_STEPS: bool = false;

pub const PBR_PROFILE_COMPANION_SAMPLER_TRILINEAR: bool = false;
// MATCH BASE
pub const PBR_PROFILE_COMPANION_COMPRESSION: bool = true;

pub const PBR_PROFILE_VERTEX_LIGHTGRID: bool = false;
// FRAGMENT

pub struct EguiRenderData {
    pub paint_jobs: Vec<egui::ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub pixels_per_point: f32,
    /// When set, the render thread reports back (once per distinct token) after
    /// presenting a frame that carried this paint data. Menu fades key their
    /// clock off that event instead of CPU-side frame construction time.
    pub present_token: Option<u64>,
}

impl Drop for EguiRenderData {
    fn drop(&mut self) {
        // A rejected/retired frame (shutdown or a failed send) deliberately
        // discards its updates. Superseded companion frames transfer deltas
        // out first so the presenter still replays them in order.
        self.textures_delta.clear();
    }
}


/// Stable identity for an auxiliary native presentation surface. Phase 1 only
/// exposes one companion window, but the renderer protocol is intentionally
/// multi-window so spectator scene views can be added without another rewrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompanionId(pub u32);

#[derive(Clone)]
pub struct CompanionSceneView {
    pub id: CompanionId,
    /// Player model hidden from this camera to approximate first-person
    /// spectator presentation. Other current-snapshot entities remain shared.
    pub target_entity: u16,
    pub camera: Camera,
}

#[derive(Clone, Copy, Debug)]
pub struct InspectorEntityHint {
    pub entity_num: u16,
    pub distance: f32,
    pub hit: [f32; 3],
}

/// Render-thread cost of each setting inside a `BatchBegin`..`BatchEnd` window.
pub(in crate::renderer) struct RenderBatchTiming {
    pub(in crate::renderer) title: String,
    pub(in crate::renderer) started: Instant,
    pub(in crate::renderer) current: String,
    pub(in crate::renderer) entries: Vec<(String, f64)>,
}

impl RenderBatchTiming {
    pub(in crate::renderer) fn new(title: String) -> Self {
        Self {
            title,
            started: Instant::now(),
            current: "(unlabelled)".to_owned(),
            entries: Vec::new(),
        }
    }

    pub(in crate::renderer) fn record(&mut self, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.0;
        match self
            .entries
            .iter_mut()
            .find(|(label, _)| *label == self.current)
        {
            Some((_, total)) => *total += ms,
            None => self.entries.push((self.current.clone(), ms)),
        }
    }

    pub(in crate::renderer) fn print(mut self) {
        let busy: f64 = self.entries.iter().map(|(_, ms)| ms).sum();
        self.entries.sort_by(|a, b| b.1.total_cmp(&a.1));
        rverbose!(
            2,
            "[BATCH] {}: render thread applied {} setting(s) in {:.2} ms busy ({:.2} ms wall)",
            self.title,
            self.entries.len(),
            busy,
            self.started.elapsed().as_secs_f64() * 1000.0,
        );
        for (label, ms) in self.entries.iter().filter(|(_, ms)| *ms >= 0.05).take(20) {
            rverbose!(2, "[BATCH]   render {label}: {ms:.2} ms");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotFormat {
    Jpeg,
    Png,
}

/// `gputilecost` console command: measured per-tile GPU cost.
pub enum TileCostCommand {
    /// Sweep a `cols` x `rows` grid with `samples` timings per tile; write `stem`.csv/.json.
    Start { stem: PathBuf, cols: u32, rows: u32, samples: usize },
    /// Show the finished heatmap of one GPU pass (report label, e.g. `ocean_surface`).
    Show(String),
    /// Hide the heatmap.
    Hide,
    /// Stop a running sweep without a report.
    Cancel,
    /// Log the passes the finished report has data for.
    ListPasses,
}

pub enum RenderCommand {
    /// On-demand accounting; no polling or resource walking in ordinary frames.
    MemoryInfo,
    /// Arm a bounded, off-by-default per-frame command-encoding capture.
    /// `seconds` makes the capture time-bounded (frames is then only a cap).
    StartPerfCapture { stem: PathBuf, frames: usize, seconds: Option<f64> },
    /// One-command six-phase GPU sky sweep on the existing Unified renderer.
    StartSkyPerfSweep { stem: PathBuf, frames: usize },
    TileCost(TileCostCommand),
    Resize(PhysicalSize<u32>),
    SetVsync(VsyncMode),
    SetMaxFrameLatency(u32),
    SetEarlySubmit(bool),
    SetPresentCopy(bool),
    SetInputLateLatch(bool),
    SetMsaa(u32),
    SetTextureFilter(TextureFilter),
    SetPicmip(u32),
    SetTextureCompression(u8),
    SetDetailTextures(bool),
    SetDetailTexture {
        path: String,
        game: Option<PathBuf>,
    },
    SetDetailTextureFade {
        enabled: bool,
        distance: f32,
    },
    SetWireframeMask(u32),
    SetShaderComplexity(bool),
    /// Trigger-volume / clip-brush debug overlays (session-only).
    SetDebugVolumes {
        triggers: bool,
        clips: bool,
    },
    /// `r_drawEntities`: boxes + link lines for every map entity, rebuilt on
    /// the app thread every tick (small vertex counts; see `DebugVolumeRenderer::entities`).
    /// `None` clears the overlay (cvar off or no map entity graph loaded).
    SetEntityMarkers(Option<jka_assets::bsp::DebugVolumeMesh>),
    /// `r_coastalDebug`: static Phase 2.5 classification/boundary/cell overlay.
    SetCoastalDebugMesh(Option<jka_assets::bsp::DebugVolumeMesh>),
    SetCoastalWater {
        enabled: bool,
        pool: u8,
        atlas: Option<Arc<jka_assets::bsp::bathymetry::Atlas>>,
        upstream_source: Option<Arc<crate::ocean::upstream_coast::Source>>,
    },
    /// Preview a sun ray; debug modes also colorize BSP baked-sun inputs.
    /// 0 = active sun ray only, 1 = baked removal amount, 2 = authored-sun visibility.
    SetSunRayPreview { enabled: bool, debug_mode: u8 },
    SetPvsMode(PvsMode),
    SetFacePlaneCull(bool),
    SetWorldFlares(bool),
    SetDrawInlineModels(bool),
    SetInlineTransformMode(ui::InlineTransformMode),
    /// r_fragmentShaderMode: 0 general world fragment program, 1 per-material specialized.
    SetFragmentShaderMode(u8),
    SetWorldSortMode(u8),
    SetPostSpecialize(bool),
    /// r_colorClearMode: 0 always clear, 1 skip when the sky box covers the background.
    SetColorClearMode(u8),
    /// Render-only controls for compiled model-path world stages and GPU A/B stage isolation.
    SetWorldStageDebug { draw_bsp_model_surfaces: bool, probe: u8 },
    SetFastSky(u8),
    SetFpsCap(u32),
    SetGamma(f32),
    SetGammaMethod(crate::gamma::GammaMethod),
    PrepareGammaMethod(u64),
    /// Effective extra multiplier for lit dynamic-model colour (1.0 = neutral).
    SetModelBrightness(f32),
    /// Effective multiplier for runtime (transient) dynamic-light intensity.
    SetDynamicLightBrightness(f32),
    SetPostEffects(PostEffects),
    SetFootprintMode(FootprintMode),
    SetGrassEnabled(bool),
    SetGrassPrecompute(bool),
    SetGrassMidLod(bool),
    SetGrassFrontToBack(bool),
    /// Runtime-only rain performance A/B controls.
    SetRainOptimizations {
        fast_post: bool,
        water_field: bool,
        fast_vertex: bool,
        lean_depth: bool,
        static_depth: bool,
        opaque_depth: bool,
        lean_models: bool,
        lean_post: bool,
    },
    SetContactShadowDebug(u8),
    SetSsrDebug(u8),
    SetOceanEnabled(bool),
    /// Phase-1 render-only clipmap continuation at qualifying BSP sky walls.
    SetOceanSkyExtend(u8),
    SetOceanSettings(crate::ocean::OceanSettings),
    /// Blue Water ship wake input (hulls, strikes, demo ships); ignored unless `r_oceanWake` is on.
    OceanWake(crate::ocean::WakeCommand),
    SetPromotedOceanSettings(Vec<(usize, crate::ocean::OceanSettings)>),
    SetAuthoredOceans(Vec<crate::ocean::authoring::AuthoredOcean>),
    SetOceanTime(f32),
    SetPerfTrace(bool),
    SetForceUnifiedWorld(bool),
    SetPom(bool),
    SetGpuTimings(bool),
    /// Native top-of-screen frame-interval graph, updated at 30Hz.
    SetFrameTimeGraph(bool),
    SetFxZeroAlphaDiscard(bool),
    /// A/B diagnostic for BSP `alphaGen vertex`, no WGSL runtime branch.
    SetDebugWorldVertexAlphaOne(bool),
    SetGhoul2BatchDraws(Ghoul2BatchMode),
    SetModelStateCache(bool),
    SetDynamicBatchDraws(bool),
    SetWorldCompactMinDraws(u32),
    SetPatchLod { curve_error: f32, no_curves: bool, speeds: i32 },
    /// Shared MD3 `r_lodScale` / `r_lodbias` settings for baked BSP models.
    SetBspModelLod { scale: f32, bias: i32, debug: bool, enabled: bool },
    SetModelPrepareThreads(u32),
    SetMd3GeometryCache(bool),
    SetModelRenderBundles(bool),
    SetSurfaceSpritesGpu(bool),
    SetSurfaceSprites(bool),
    SetEncoderFinishThreads(u32),
    SetModelIndexUploadCache(bool),
    SetGhoul2BatchKeyCache(bool),
    SetGpuVisibility {
        gpu_driven: bool,
        hiz_occlusion: bool,
    },
    SetDynamicLighting(DynamicLightsMode),
    SetDynamicLightDebug(bool),
    SetRtSamples(u32),
    SetDynamicLightFalloff(u32),
    SetRtTraceRate(u32),
    SetMapLightSimulation(bool),
    SetClassicWorldLighting {
        fullbright: bool,
        vertex_light: bool,
        lightmap_only: bool,
    },
    SetEmissiveAreaLights(bool),
    SetShaderLightPromotion { enabled: bool, mask: u32 },
    SetEntityAmbientLighting(EntityAmbientLightingMode),
    SetVoxelProbeGi(bool),
    SetLocalLightShadows(bool),
    SetTintedGlass(bool),
    SetTintedGlassDebug(u32),
    SetEntityShadowLight(EntityShadowLight),
    SetPbrSettings {
        enabled: bool,
        deluxe_mapping: bool,
        deluxe_specular: f32,
    },
    SetCascadedShadows(DynamicShadowsMode),
    SetCullDebug(CullDebugMode),
    SetPlanarReflectionDebug(PlanarReflectionDebugMode),
    SetPuddleDebug(u32),
    SetPuddleSkyBlend {
        window: f32,
        ceiling: f32,
    },
    DumpMaterials {
        filter: Option<String>,
        all: bool,
    },
    InspectSurface {
        x: f32,
        y: f32,
        width: u32,
        height: u32,
        entity_hint: Option<InspectorEntityHint>,
    },
    ClearSurfaceInspection,
    AddSurfaceDeformation(SurfaceDeformationStamp),
    Screenshot {
        directory: PathBuf,
        metadata: crate::screenshot::ScreenshotMetadata,
        format: ScreenshotFormat,
    },
    CopyFrameToClipboard,
    /// Write the Hybrid wave-classification log and a settings summary to `path`.
    HybridWaveDump { path: PathBuf },
    /// One-shot scene-only readback, before egui/HUD layers.
    CaptureFrontendScene { path: PathBuf, neutral_gamma: bool },
    CaptureVideoFrame { frame_index: u64, depth: bool },
    SetUi(UiSnapshot),
    /// Small independent overlay update; avoids rebuilding the full UiSnapshot.
    SetExtensionHud(Vec<crate::extensions::PlacedHudWidget>, crate::extensions::NativeHudMask, Vec<String>),
    SetTransientUi {
        chat_lines: Vec<ui::UiChatLine>,
        console_notify_lines: Vec<ui::UiConsoleNotifyLine>,
        center_print: Option<ui::UiCenterPrint>,
        demo_timeline: Option<ui::DemoTimelineUi>,
        prediction_debug: Option<ui::PredictionDebugUi>,
        crosshair_target: ui::UiCrosshairTarget,
        force_select: Option<ui::UiForceSelect>,
        follow_name: Option<String>,
        game_timer: Option<String>,
        mini_scores: Option<ui::UiMiniScores>,
        race_timer: Option<crate::japro_cg::RaceTimerUi>,
        vote_line: Option<String>,
        scoreboard: Option<ui::UiScoreboard>,
        scoreboard_focus_client: Option<i32>,
        speedometer: Option<crate::speedometer::Ui>,
        lagometer: Option<crate::lagometer::Ui>,
        team_overlay: Option<ui::TeamOverlayUi>,
    },
    SetUiTelemetry {
        perf: ui::PerfStats,
        threads: std::sync::Arc<[ui::ThreadPerfStats]>,
    },
    SetEgui(Option<EguiRenderData>),
    CreateCompanion {
        id: CompanionId,
        window: Arc<Window>,
        size: PhysicalSize<u32>,
        surface: wgpu::Surface<'static>,
    },
    DestroyCompanion {
        id: CompanionId,
    },
    ResizeCompanion {
        id: CompanionId,
        size: PhysicalSize<u32>,
    },
    /// Latest-wins companion UI mailbox. The auxiliary worker renders only when
    /// this changes; a static second monitor consumes no recurring CPU/GPU work.
    SetCompanionEgui {
        id: CompanionId,
        frame: EguiRenderData,
    },
    /// Developer Asset Viewer: ignore the loaded BSP and render preview
    /// submissions over a clean black background behind egui.
    SetAssetPreviewMode(bool),
    /// Physical-pixel viewport reserved by egui for the live preview. Keeping
    /// 3D rendering inside this rectangle prevents models/FX/materials from
    /// being drawn underneath translucent browser panes.
    SetAssetPreviewViewport(Option<[u32; 4]>),
    /// Decoded installed-map levelshot, uploaded once when selected.
    SetAssetPreviewBackground(Option<([u32;2],Vec<u8>)>),
    UnloadMap,
    /// Grow private inline render slots from the prepared CPU cache while the
    /// current CGame/collision session continues unchanged.
    GrowInlineInstances { map: Box<PreparedMap> },
    LoadMap {
        request_id: u64,
        started: Instant,
        name: String,
        map: Box<PreparedMap>,
    },
    /// Same prepared-world pipeline as LoadMap, but reuse safe immutable GPU
    /// images/PSOs from the current map when their cache keys match. Remap only.
    RemapMap {
        request_id: u64,
        started: Instant,
        name: String,
        plan: Box<crate::scene::PreparedRemapPlan>,
        base_texture_count: usize,
    },
    /// Open a settings batch (quality preset): world pipeline-variant
    /// activation is deferred and per-setting render-thread cost is recorded.
    BatchBegin(String),
    /// Attribute the render-thread cost of the following commands to this cvar.
    BatchLabel(String),
    /// Close the batch: activate the final pipeline variant once and print the
    /// per-setting timing report.
    BatchEnd,
    Shutdown,
}

/// CPU-side calls recorded *at the instrumented call sites* of the main
/// frame functions. These are not driver/API-internal or all-subrenderer totals.
/// In particular each multi-draw call is counted once, not once per GPU draw.
#[derive(Default, Clone, Copy)]
pub(in crate::renderer) struct PerfEncodeCallCounts {
    pub render_passes: u32,
    pub compute_passes: u32,
    pub set_pipeline: u32,
    pub set_bind_group: u32,
    pub set_vertex_buffer: u32,
    pub set_index_buffer: u32,
    pub draw: u32,
    pub draw_indexed: u32,
    pub world_helper_draw_indexed: u32,
    pub draw_indexed_indirect: u32,
    pub multi_draw_indexed_indirect: u32,
    pub multi_draw_indexed_indirect_count: u32,
}

/// Opt-in main-camera BSP draw-batch candidates indexed by material debug ID.
/// Indexed counts are prepared GPU batches, not unique map triangles or the
/// number of authored stages inside an additive fused draw. Disabled capture
/// leaves this allocation/counter work out of the normal frame hot path.
#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct WorldStagePerf {
    pub(in crate::renderer) stages: u64,
    pub(in crate::renderer) stage_indices: u64,
    /// Authored/resolved stage equivalents represented by these submitted
    /// world-batch candidates (fused batches still count as one draw).
    pub(in crate::renderer) resolved_stage_equivalents: u64,
    /// Real authored-glow draw_indexed calls recorded at the replay draw site.
    pub(in crate::renderer) glow_replay_draws: u64,
    pub(in crate::renderer) distinct_surfaces: u64,
    pub(in crate::renderer) distinct_indices: u64,
    pub(in crate::renderer) inline_stages: u64,
    pub(in crate::renderer) skipped_hidden_stages: u64,
    pub(in crate::renderer) skipped_hidden_indices: u64,
}

#[derive(Default)]
pub(in crate::renderer) struct FrameInfo {
    pub(in crate::renderer) native_thunder: Vec<blue_water_clouds::lightning::Thunder>,
    pub(in crate::renderer) patch_lod: super::world::patch_lod::PatchLodStats,
    pub(in crate::renderer) frame_ms: f64,
    pub(in crate::renderer) cpu_prepare_ms: f64,
    pub(in crate::renderer) cpu_acquire_ms: f64,
    pub(in crate::renderer) cpu_encode_ms: f64,
    pub(in crate::renderer) cpu_submit_ms: f64,
    pub(in crate::renderer) cpu_present_ms: f64,
    /// Opt-in diagnostic. Mutually exclusive encoder intervals; they do not
    /// measure GPU execution. `encode_pre_world` can include RT, shadows,
    /// reflections, and environment work on the unified path.
    pub(in crate::renderer) perf_encode_setup_ms: f64,
    pub(in crate::renderer) perf_encode_pre_world_ms: f64,
    /// First-use profiling subdivisions within pre-world command encoding.
    pub(in crate::renderer) perf_pre_world_ocean_ms: f64,
    pub(in crate::renderer) perf_pre_world_shadows_ms: f64,
    pub(in crate::renderer) perf_pre_world_remaining_ms: f64,
    pub(in crate::renderer) perf_encode_scene_ms: f64,
    pub(in crate::renderer) perf_encode_post_ui_ms: f64,
    pub(in crate::renderer) perf_encoder_finish_ms: f64,
    pub(in crate::renderer) perf_encoder_split_active: bool,
    pub(in crate::renderer) perf_encoder_prefix_finish_ms: f64,
    pub(in crate::renderer) perf_encoder_tail_finish_ms: f64,
    pub(in crate::renderer) perf_encoder_dispatch_ms: f64,
    pub(in crate::renderer) perf_encoder_join_wait_ms: f64,
    pub(in crate::renderer) perf_profiler_readback_encode_ms: f64,
    /// Isolated first `queue.submit()` API call, not the old `cpu_submit_ms`.
    pub(in crate::renderer) perf_primary_submit_api_ms: f64,
    /// Time AFTER the first queue submit until `cpu_submit_ms` stops. Includes
    /// optional SMAA, CRT, menu, egui, screenshot and related CPU work.
    pub(in crate::renderer) perf_followup_ms: f64,
    pub(in crate::renderer) perf_primary_command_buffers: u32,
    pub(in crate::renderer) perf_encode_calls: PerfEncodeCallCounts,
    pub(in crate::renderer) perf_deferred_bundle_hit: u32,
    pub(in crate::renderer) perf_deferred_bundle_build: u32,
    pub(in crate::renderer) perf_deferred_bundle_draws: u32,
    /// Main-view CPU compaction audit; zero if that path was inactive.
    pub(in crate::renderer) world_cpu_compaction_active: bool,
    pub(in crate::renderer) world_cpu_compact_visible_members: u32,
    pub(in crate::renderer) world_cpu_compact_singleton_groups: u32,
    pub(in crate::renderer) world_cpu_compact_multistage_excluded_batches: u32,
    /// Face-plane rejection in the primary CPU world pass (counts stage submissions).
    pub(in crate::renderer) world_face_plane_rejected_batches: u32,
    pub(in crate::renderer) world_face_plane_partial_batches: u32,
    pub(in crate::renderer) world_face_plane_rejected_triangles: u32,
    pub(in crate::renderer) world_face_plane_direct_draws: u32,
    /// Which world renderer produced this frame ("fast-baseline" / "unified").
    pub(in crate::renderer) world_path: &'static str,
    /// Advanced-path breakdown of `cpu_prepare_ms`. All zero on FastBaseline,
    /// whose prepare stage is not split (it stays free of extra timers).
    pub(in crate::renderer) prep_camera_ms: f64,
    pub(in crate::renderer) prep_shadow_ms: f64,
    pub(in crate::renderer) prep_world_ms: f64,
    pub(in crate::renderer) prep_reflection_ms: f64,
    pub(in crate::renderer) submit_call_at: Option<Instant>,
    pub(in crate::renderer) present_call_completed_at: Option<Instant>,
    pub(in crate::renderer) late_latch: Option<ViewSampleMeasurement>,
    /// True when the late-latch sample was intentionally taken after surface
    /// acquisition (fast baseline without camera-dependent early preparation).
    pub(in crate::renderer) late_latch_after_acquire: bool,
    /// r_earlySubmit applied: recorded offscreen, submitted before acquire.
    pub(in crate::renderer) early_submit: bool,
    pub(in crate::renderer) dynamic_model_prepare_ms: f64,
    pub(in crate::renderer) dynamic_model_surfaces: u32,
    pub(in crate::renderer) dynamic_model_vertices: u64,
    pub(in crate::renderer) dynamic_model_indices: u64,
    pub(in crate::renderer) ghoul2_gpu_instances: u32,
    pub(in crate::renderer) ghoul2_gpu_draw_calls: u32,
    pub(in crate::renderer) gpu_ms: Option<f64>,
    pub(in crate::renderer) cull_visible: u32,
    pub(in crate::renderer) cull_frustum_rejected: u32,
    pub(in crate::renderer) cull_hiz_rejected: u32,
    pub(in crate::renderer) cull_pvs_rejected: u32,
    pub(in crate::renderer) cull_area_rejected: u32,
    /// CPU-side world submission counts for the main camera. These remain
    /// useful when GPU-driven culling is disabled, which is exactly where
    /// OpenJK-style BSP/PVS + frustum rejection prevents command-encoder blowup.
    pub(in crate::renderer) world_pvs_batches: u32,
    pub(in crate::renderer) world_frustum_rejected: u32,
    /// Batches dropped because water absorption hides them (r_oceanUnderwaterCull).
    pub(in crate::renderer) world_ocean_culled: u32,
    pub(in crate::renderer) world_ocean_culled_triangles: u32,
    pub(in crate::renderer) world_encoded_batches: u32,
    /// Main world pass base-program binds: specialized (r_fragmentShaderMode 1) vs general.
    pub(in crate::renderer) world_specialized_binds: u32,
    pub(in crate::renderer) world_general_binds: u32,
    pub(in crate::renderer) world_shader_programs: crate::runtime::WorldShaderPrograms,
    /// Active world variant PSOs: general + per-material specialized (not total cache).
    pub(in crate::renderer) world_pipeline_counts: (u32, u32),
    /// Sort A/B: estimated pipeline transitions for eligible runs only.
    /// These are NOT actual bind counts (compaction/fog may change them).
    pub(in crate::renderer) world_sort_candidates: u32,
    pub(in crate::renderer) world_sort_runs: u32,
    pub(in crate::renderer) world_sort_moved: u32,
    pub(in crate::renderer) world_sort_binds_before: u32,
    pub(in crate::renderer) world_sort_binds_after: u32,
    pub(in crate::renderer) world_multidraw_groups: u32,
    pub(in crate::renderer) world_multidraw_batches: u32,
    /// Main-camera authored world stages surviving CPU area/frustum/optical
    /// rejection. These are geometry-submission *candidates* on the GPU-driven
    /// path; after CPU culling, they represent the submitted stage workload.
    pub(in crate::renderer) perf_world_stage_batches: u64,
    pub(in crate::renderer) perf_world_stage_indices: u64,
    pub(in crate::renderer) perf_world_distinct_surfaces: u64,
    pub(in crate::renderer) perf_world_distinct_indices: u64,
    /// Authoritative inline mover state at main-camera submission.
    pub(in crate::renderer) perf_inline_models_active: u64,
    pub(in crate::renderer) perf_inline_models_hidden: u64,
    pub(in crate::renderer) perf_inline_models_uninitialized: u64,
    /// PVS-selected authored stage work suppressed because a particular
    /// inline render instance is absent from the authoritative snapshot.
    pub(in crate::renderer) perf_inline_hidden_stage_batches: u64,
    pub(in crate::renderer) perf_inline_hidden_stage_indices: u64,
    pub(in crate::renderer) perf_world_inline_stages: u64,
    pub(in crate::renderer) perf_world_sky_stages: u64,
    pub(in crate::renderer) perf_world_largest_stage_indices: u64,
    pub(in crate::renderer) perf_world_opaque_indices: u64,
    pub(in crate::renderer) perf_world_mask_indices: u64,
    pub(in crate::renderer) perf_world_transparent_indices: u64,
    pub(in crate::renderer) perf_world_sky_indices: u64,
    /// Non-inline authored glow-stage replays in the main camera only.
    pub(in crate::renderer) perf_world_glow_replay_stages: u64,
    pub(in crate::renderer) perf_world_glow_replay_indices: u64,
    pub(in crate::renderer) perf_world_cpu_visibility: bool,
    pub(in crate::renderer) perf_world_material_stages: Vec<WorldStagePerf>,
    pub(in crate::renderer) grass: GrassDrawStats,
    pub(in crate::renderer) surface_sprites: crate::renderer::environment::surface_sprites::SurfaceSpriteStats,
    pub(in crate::renderer) ocean: crate::ocean::OceanDrawStats,
}

impl FrameInfo {
    #[inline]
    pub(in crate::renderer) fn perf_count_hidden_world_stage(&mut self, material_index: u32, indices: u32) {
        self.perf_inline_hidden_stage_batches += 1;
        self.perf_inline_hidden_stage_indices += u64::from(indices);
        if let Some(material) = self.perf_world_material_stages.get_mut(material_index as usize) {
            material.skipped_hidden_stages += 1;
            material.skipped_hidden_indices += u64::from(indices);
        }
    }

    pub(in crate::renderer) fn perf_count_world_stage(
        &mut self, material_index: u32, indices: u32, distinct: bool,
        inline: bool, class: crate::scene::DrawClass, resolved_stage_count: u8,
    ) {
        // An empty vector is the capture-disabled fast path.
        if self.perf_world_material_stages.is_empty() { return; }
        let indices = u64::from(indices);
        self.perf_world_stage_batches += 1;
        self.perf_world_stage_indices += indices;
        self.perf_world_largest_stage_indices = self.perf_world_largest_stage_indices.max(indices);
        self.perf_world_inline_stages += u64::from(inline);
        self.perf_world_sky_stages += u64::from(class == crate::scene::DrawClass::Sky);
        match class {
            crate::scene::DrawClass::Opaque => self.perf_world_opaque_indices += indices,
            crate::scene::DrawClass::Mask => self.perf_world_mask_indices += indices,
            crate::scene::DrawClass::Transparent => self.perf_world_transparent_indices += indices,
            crate::scene::DrawClass::Sky => self.perf_world_sky_indices += indices,
        }
        if distinct {
            self.perf_world_distinct_surfaces += 1;
            self.perf_world_distinct_indices += indices;
        }
        if let Some(mat) = self.perf_world_material_stages.get_mut(material_index as usize) {
            mat.stages += 1;
            mat.stage_indices += indices;
            mat.resolved_stage_equivalents += u64::from(resolved_stage_count.max(1));
            mat.inline_stages += u64::from(inline);
            if distinct {
                mat.distinct_surfaces += 1;
                mat.distinct_indices += indices;
            }
        }
    }
}

#[derive(Debug)]
pub(in crate::renderer) enum RenderError {
    Lost,
    Outdated,
    Timeout,
    Occluded,
    Validation,
    /// The WGPU device itself is gone (driver reset/TDR, removed adapter).
    /// Unlike `Lost`, which is a surface that only needs reconfiguring, nothing
    /// can be rendered on this device again.
    DeviceLost,
}
