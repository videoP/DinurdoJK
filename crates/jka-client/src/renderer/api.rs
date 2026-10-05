//! Api.
use crate::renderer::{
    scene, ui, Arc, Camera, CloudRenderResolution, CloudType, ColorLutPreset, CullDebugMode,
    DetailTextureMode, DofQuality, Duration, DynamicLightsMode, DynamicShadowsMode,
    EntityAmbientLightingMode, EntityShadowLight, FogMode, FootprintMode, Ghoul2BatchMode,
    Ghoul2SkinningMode, Hash, InputLatencySample, Instant, JumpShadeState, PathBuf, PhysicalSize,
    PlanarReflectionDebugMode, Pod, PreparedMap, PuddleQuality, PvsMode, RainIntensity,
    RayTracedRigidMeshKey, ReflectionQuality, SunVisibilityMode, SurfaceDeformationStamp,
    TextureData, TextureFilter, UiSnapshot, Vec3, ViewLatchMode, VsyncMode, Window, Zeroable,
};
use crate::renderer::{GrassDrawStats, ViewSampleMeasurement};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DynamicModelAlphaMode {
    Opaque,
    Mask,
    Blend,
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
}

impl DynamicModelAlphaMode {
    pub(in crate::renderer) const COUNT: usize = 11;

    /// Dense index for per-alpha-mode pipeline tables and draw bitmasks.
    pub(in crate::renderer) const fn index(self) -> usize {
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
        }
    }
}

pub(in crate::renderer) const DYNAMIC_MODEL_ALPHA_ORDER: [DynamicModelAlphaMode; 11] = [
    DynamicModelAlphaMode::Opaque,
    DynamicModelAlphaMode::Mask,
    DynamicModelAlphaMode::MaskBlend,
    DynamicModelAlphaMode::Modulate,
    DynamicModelAlphaMode::DstColorAdd,
    DynamicModelAlphaMode::Modulate2x,
    DynamicModelAlphaMode::Darken,
    DynamicModelAlphaMode::Blend,
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
        DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
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
}

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
    /// Up to four model-space post-skin region offsets. Ordinary Ghoul2 draws
    /// keep these zero; jiggle vertices select one through `jiggle_region`.
    pub jiggle_offsets: [[f32; 4]; 4],
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
    /// Instances are player blob-shadow requests, not billboards. The renderer
    /// clips the rendered floor into each request (`Renderer::build_blob_shadow_marks`,
    /// OpenJK R_MarkFragments) and draws the resulting polygons as one ordinary
    /// dynamic mesh. origin = center on the collision plane (w = shade), left/up =
    /// in-plane half axes of length `radius` (left.w = entity number).
    pub blob_shadow: bool,
}

#[derive(Clone)]
pub struct DynamicModelSurface {
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
}

impl InlineModelInstance {
    /// The compiled pose, used when no CGame snapshot owns the movers.
    pub(in crate::renderer) fn compiled(model: u32) -> Self {
        Self {
            model,
            origin: [0.0; 3],
            axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
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
        scene::render_position(self.rotate(scene::jka_position(render)))
    }

    pub(in crate::renderer) fn rotate(&self, v: [f32; 3]) -> [f32; 3] {
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
    pub(in crate::renderer) saber_mark_sources: usize,
    pub(in crate::renderer) radius_sum: f64,
    pub(in crate::renderer) radius_max: f32,
    pub(in crate::renderer) surfaces_total: usize,
    pub(in crate::renderer) surfaces_eligible: usize,
    pub(in crate::renderer) surfaces_touched: usize,
    pub(in crate::renderer) candidate_pairs: u64,
    pub(in crate::renderer) saber_pairs: u64,
    pub(in crate::renderer) authored_fx_pairs: u64,
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
    pub intensity: f32,
    pub segment: Option<[[f32; 3]; 2]>,
    pub blade_segments: Option<Arc<[crate::fx::system::FxLightSegment]>>,
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
    /// Current server snapshot portal-area mask. Set bits are areas closed off
    /// by areaportals/doors. `None` for solo/offline scenes with no snapshot.
    pub area_mask: Option<[u8; 32]>,
    pub dynamic_models: Arc<Vec<DynamicModelSurface>>,
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
    pub auto_exposure: bool,
    pub tone_mapping: bool,
    pub bloom: bool,
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
    pub contact_shadows: bool,
    pub fog_mode: FogMode,
    pub fog_strength: f32,
    pub sun_override: bool,
    pub sun_yaw: f32,
    pub sun_pitch: f32,
    pub sun_intensity: f32,
    pub sun_color: [f32; 3],
    pub sun_visibility: SunVisibilityMode,
    pub entity_sun_lighting: bool,
    pub clouds: bool,
    pub cloud_type: CloudType,
    pub cloud_quality: f32,
    pub cloud_coverage: f32,
    pub cloud_height: f32,
    pub cloud_thickness: f32,
    pub weather_wind: crate::ocean::OceanWind,
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
    pub dof_quality: DofQuality,
    pub color_lut: ColorLutPreset,
    pub color_lut_strength: f32,
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

pub enum RenderCommand {
    Resize(PhysicalSize<u32>),
    SetVsync(VsyncMode),
    SetMaxFrameLatency(u32),
    SetInputLateLatch(bool),
    SetMsaa(u32),
    SetTextureFilter(TextureFilter),
    SetPicmip(u32),
    SetDetailTextures(DetailTextureMode),
    SetDetailTexture {
        path: String,
        game: Option<PathBuf>,
    },
    SetDetailTextureFade {
        enabled: bool,
        distance: f32,
    },
    SetWireframeMask(u32),
    /// Trigger-volume / clip-brush debug overlays (session-only).
    SetDebugVolumes {
        triggers: bool,
        clips: bool,
    },
    /// `r_drawEntities`: boxes + link lines for every map entity, rebuilt on
    /// the app thread every tick (small vertex counts; see `DebugVolumeRenderer::entities`).
    /// `None` clears the overlay (cvar off or no map entity graph loaded).
    SetEntityMarkers(Option<jka_assets::bsp::DebugVolumeMesh>),
    /// Draw a beam from the sun to the camera (player head) while sun angles are edited.
    SetSunRayPreview(bool),
    SetPvsMode(PvsMode),
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
    SetContactShadowDebug(u8),
    SetOceanEnabled(bool),
    SetOceanSettings(crate::ocean::OceanSettings),
    SetAuthoredOceans(Vec<crate::ocean::authoring::AuthoredOcean>),
    SetOceanTime(f32),
    SetPerfTrace(bool),
    SetForceUnifiedWorld(bool),
    SetPom(bool),
    SetGpuTimings(bool),
    SetFxZeroAlphaDiscard(bool),
    SetGhoul2BatchDraws(Ghoul2BatchMode),
    SetGpuVisibility {
        gpu_driven: bool,
        hiz_occlusion: bool,
    },
    SetDynamicLighting(DynamicLightsMode),
    SetRtSamples(u32),
    SetDynamicLightFalloff(u32),
    SetRtHalfResolution(bool),
    SetMapLightSimulation(bool),
    SetClassicWorldLighting {
        fullbright: bool,
        vertex_light: bool,
        lightmap_only: bool,
    },
    SetEmissiveAreaLights(bool),
    SetEntityAmbientLighting(EntityAmbientLightingMode),
    SetVoxelProbeGi(bool),
    SetLocalLightShadows(bool),
    SetEntityShadowLight(EntityShadowLight),
    SetPbrSettings {
        enabled: bool,
        deluxe_mapping: bool,
        deluxe_specular: f32,
    },
    SetCascadedShadows(DynamicShadowsMode),
    SetCullDebug(CullDebugMode),
    SetPlanarReflectionDebug(PlanarReflectionDebugMode),
    SetPuddleDebug(bool),
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
    },
    CopyFrameToClipboard,
    SetUi(UiSnapshot),
    SetTransientUi {
        chat_lines: Vec<ui::UiChatLine>,
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
        threads: [ui::ThreadPerfStats; crate::thread_activity::SLOT_COUNT],
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
    UnloadMap,
    LoadMap {
        request_id: u64,
        started: Instant,
        name: String,
        map: Box<PreparedMap>,
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

#[derive(Default)]
pub(in crate::renderer) struct FrameInfo {
    pub(in crate::renderer) frame_ms: f64,
    pub(in crate::renderer) cpu_prepare_ms: f64,
    pub(in crate::renderer) cpu_acquire_ms: f64,
    pub(in crate::renderer) cpu_encode_ms: f64,
    pub(in crate::renderer) cpu_submit_ms: f64,
    pub(in crate::renderer) cpu_present_ms: f64,
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
    pub(in crate::renderer) world_encoded_batches: u32,
    pub(in crate::renderer) world_multidraw_groups: u32,
    pub(in crate::renderer) world_multidraw_batches: u32,
    pub(in crate::renderer) grass: GrassDrawStats,
}

#[derive(Debug)]
pub(in crate::renderer) enum RenderError {
    Lost,
    Outdated,
    Timeout,
    Occluded,
    Validation,
}
