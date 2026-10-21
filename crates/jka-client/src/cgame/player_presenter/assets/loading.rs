//! Assets loading.
use jka_assets::{
    animation::parse_animation_cfg,
    animevents::parse_animation_events,
};

use crate::cgame::player_presenter::source_player::{
    self, build_source_player_model, source_descriptor_qpath,
};
use crate::cgame::player_presenter::{
    asset_jobs, build_joined_breast_bakes, build_lod_gpu_meshes, load_skin, materials, openjk_default_gla, parse_gla,
    player_model_key, static_model_key, AlphaGen, Arc, AssetPriority, AssetSearchPath,
    AssetSource, AssetState, BTreeMap, DynamicModelAlphaMode, GlaAnimation, HashMap, JiggleProfile, JiggleRemeshTuning,
    Mutex, OnceLock, PlayerModelAsset, PlayerPresenter, PlayerSurfaceAsset, Requested,
    ResolvedMaterial, ResolvedStage, RgbGen, SaberDefinition, SaberModelAsset, Shader,
    SurfaceOverlay, TcGen, TcMod, TextureData, Textures, Vec3, Wave, WaveFunc, ASSET_PENDING,
    MAX_GLA_BYTES, MAX_GLM_BYTES, MAX_SKIN_BYTES, OPENJK_DEFAULT_GLA_NAME,
};

pub(in crate::cgame::player_presenter) enum ModelLookup {
    Ready(Arc<PlayerModelAsset>),
    Pending,
    Failed(String),
    Missing,
}

pub(in crate::cgame::player_presenter) enum ModelResolution {
    /// The requested model (or its terminal OpenJK fallback) is available.
    Ready(Arc<PlayerModelAsset>),
    /// Requested model still loading; this resident fallback body stands in.
    Provisional(Arc<PlayerModelAsset>),
    /// Nothing usable yet; the request is in flight.
    Loading,
    Failed(String),
}

/// Everything a Ghoul2 model registration needs from the outside world. The
/// synchronous path and the asset workers implement this differently, but
/// share exactly the same OpenJK registration logic in `build_*`.
pub(in crate::cgame::player_presenter) trait ModelSource {
    fn read_model(&mut self, qpath: &str) -> Result<Vec<u8>, String>;
    fn read_optional(&mut self, qpath: &str, max_bytes: usize) -> Result<Option<Vec<u8>>, String>;
    fn load_gla(&mut self, qpath: &str) -> Result<Arc<GlaAnimation>, String>;
    fn load_skin(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String>;
    fn resolve_material(&mut self, shader_name: &str) -> ResolvedMaterial;
}

/// Original behavior: reads and decodes on the calling thread, sharing the
/// presenter's texture cache.
pub(in crate::cgame::player_presenter) struct SyncModelSource<'a> {
    pub(in crate::cgame::player_presenter) presenter: &'a mut PlayerPresenter,
}

impl ModelSource for SyncModelSource<'_> {
    fn read_model(&mut self, qpath: &str) -> Result<Vec<u8>, String> {
        Ok(self
            .presenter
            .assets
            .read(qpath, MAX_GLM_BYTES)
            .map_err(|error| format!("{qpath}: {error}"))?
            .ok_or_else(|| format!("missing {qpath}"))?
            .bytes)
    }

    fn read_optional(&mut self, qpath: &str, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
        self.presenter
            .assets
            .read(qpath, max_bytes)
            .map_err(|error| format!("{qpath}: {error}"))
            .map(|asset| asset.map(|asset| asset.bytes))
    }

    fn load_gla(&mut self, qpath: &str) -> Result<Arc<GlaAnimation>, String> {
        self.presenter.load_glm_animation(qpath)
    }

    fn load_skin(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String> {
        self.presenter.load_skin_qpath(qpath)
    }

    fn resolve_material(&mut self, shader_name: &str) -> ResolvedMaterial {
        self.presenter.resolve_surface_material(shader_name)
    }
}

/// Worker-side caches shared by every asset job of one presenter. Per-key
/// `OnceLock`s make concurrent jobs that need the same GLA or texture wait for
/// one decode instead of repeating it (all humanoid players share one GLA).
pub(in crate::cgame::player_presenter) struct ModelLoadShared {
    pub(in crate::cgame::player_presenter) shaders: Arc<BTreeMap<String, Shader>>,
    pub(in crate::cgame::player_presenter) shader_remaps: crate::scene::ShaderRemapState,
    pub(in crate::cgame::player_presenter) glas:
        Mutex<HashMap<String, Arc<OnceLock<Result<Arc<GlaAnimation>, String>>>>>,
    pub(in crate::cgame::player_presenter) textures:
        Mutex<HashMap<(String, bool), Arc<OnceLock<Option<Arc<TextureData>>>>>>,
}

impl ModelLoadShared {
    pub(in crate::cgame::player_presenter) fn new(
        shaders: Arc<BTreeMap<String, Shader>>,
        shader_remaps: crate::scene::ShaderRemapState,
    ) -> Self {
        Self {
            shaders,
            shader_remaps,
            glas: Mutex::new(HashMap::new()),
            textures: Mutex::new(HashMap::new()),
        }
    }
}

pub(in crate::cgame::player_presenter) struct WorkerModelSource<'a> {
    pub(in crate::cgame::player_presenter) vfs: &'a mut AssetSearchPath,
    pub(in crate::cgame::player_presenter) shared: &'a ModelLoadShared,
}

impl ModelSource for WorkerModelSource<'_> {
    fn read_model(&mut self, qpath: &str) -> Result<Vec<u8>, String> {
        Ok(self
            .vfs
            .read(qpath, MAX_GLM_BYTES)
            .map_err(|error| format!("{qpath}: {error}"))?
            .ok_or_else(|| format!("missing {qpath}"))?
            .bytes)
    }

    fn read_optional(&mut self, qpath: &str, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
        self.vfs
            .read(qpath, max_bytes)
            .map_err(|error| format!("{qpath}: {error}"))
            .map(|asset| asset.map(|asset| asset.bytes))
    }

    fn load_gla(&mut self, qpath: &str) -> Result<Arc<GlaAnimation>, String> {
        let cell = {
            let mut glas = self
                .shared
                .glas
                .lock()
                .map_err(|_| "GLA cache poisoned".to_owned())?;
            Arc::clone(glas.entry(qpath.to_ascii_lowercase()).or_default())
        };
        let vfs = &mut *self.vfs;
        cell.get_or_init(|| {
            if qpath.eq_ignore_ascii_case(OPENJK_DEFAULT_GLA_NAME) {
                return openjk_default_gla().map(Arc::new);
            }
            let bytes = vfs
                .read(qpath, MAX_GLA_BYTES)
                .map_err(|error| format!("{qpath}: {error}"))?
                .ok_or_else(|| format!("missing {qpath}"))?
                .bytes;
            parse_gla(&bytes).map(Arc::new)
        })
        .clone()
    }

    fn load_skin(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String> {
        let vfs = &mut *self.vfs;
        load_skin(qpath, |name| {
            vfs.read(name, MAX_SKIN_BYTES)
                .map_err(|error| error.to_string())
                .map(|asset| asset.map(|asset| asset.bytes))
        })
    }

    fn resolve_material(&mut self, shader_name: &str) -> ResolvedMaterial {
        let shared = self.shared;
        let effective = shared.shader_remaps.effective_name(shader_name);
        let layers = material_layers(&shared.shaders, effective.as_ref());
        let shader_time_offset = shared.shader_remaps.time_offset_for(effective.as_ref());
        let shader_clamp_time = jka_assets::shader::find_shader(&shared.shaders, effective.as_ref()).and_then(|shader| shader.clamp_time);
        let (mut texture, mut alpha_mode) = self.load_stage(&layers.base);
        let overlay = layers.overlay.as_ref().map(|stage| {
            let (texture, alpha_mode) = self.load_stage(stage);
            SurfaceOverlay {
                texture,
                alpha_mode,
                authored_glow: stage.authored_glow,
            }
        });
        let stages = layers
            .stages
            .iter()
            .map(|&(stage, mode)| {
                let (texture, _) = self.load_stage(&StageMaterial {
                    image: stage.image.as_str(),
                    clamp: stage.clamp,
                    alpha_mode: mode,
                    authored_glow: stage.glow,
                });
                resolved_stage(stage, mode, texture)
            })
            .collect::<Vec<_>>();
        if let Some(first) = stages.first() {
            texture = first.texture.clone();
            alpha_mode = first.alpha_mode;
        }
        ResolvedMaterial {
            texture,
            alpha_mode,
            entity_tint: layers.entity_tint,
            overlay,
            stages,
            two_sided: layers.two_sided,
            authored_glow: layers.base.authored_glow,
            shader_time_offset,
            shader_clamp_time,
        }
    }
}

impl WorkerModelSource<'_> {
    pub(in crate::cgame::player_presenter) fn load_stage(
        &mut self,
        stage: &StageMaterial<'_>,
    ) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        let (image_name, clamp, alpha_mode) = (stage.image, stage.clamp, stage.alpha_mode);
        if image_name.eq_ignore_ascii_case("$whiteimage") {
            return (None, alpha_mode);
        }
        let path = image_name.replace('\\', "/").to_ascii_lowercase();
        let cell = {
            let Ok(mut textures) = self.shared.textures.lock() else {
                return (None, alpha_mode);
            };
            Arc::clone(textures.entry((path.clone(), clamp)).or_default())
        };
        let vfs = &mut *self.vfs;
        let texture = cell
            .get_or_init(|| {
                // A scratch `Textures` reuses the exact decode/extension/
                // stock-override logic of the synchronous loader.
                let mut textures = Textures::new();
                let texture = textures
                    .load(vfs, &path, clamp)
                    .map(|index| Arc::new(textures.images.swap_remove(index)));
                if texture.is_none() {
                    if let Some(warning) = textures.warnings.last() {
                        rverbose!(1, "PLAYER TEXTURE WARNING: {warning}");
                    }
                }
                texture
            })
            .clone();
        (texture, alpha_mode)
    }
}

/// id Tech 3's periodic `base + func(phase + time*frequency) * amplitude`
/// (`RB_CalcWaveColorSingle`/`RB_CalcWaveAlphaSingle`), used here as a
/// per-draw approximation of a shell's `deformVertexes wave` bulge (no
/// per-vertex spatial phase, no real noise stream: `Noise` just holds at the
/// wave's base).
pub(in crate::cgame::player_presenter) fn wave_value(wave: Wave, time: f32) -> f32 {
    let phase = wave.phase + time * wave.frequency;
    let t = phase - phase.floor();
    let raw = match wave.func {
        WaveFunc::Sin => (phase * std::f32::consts::TAU).sin(),
        WaveFunc::Triangle => {
            if t < 0.5 {
                4.0 * t - 1.0
            } else {
                3.0 - 4.0 * t
            }
        }
        WaveFunc::Square => {
            if t < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        WaveFunc::Sawtooth => 2.0 * t - 1.0,
        WaveFunc::InverseSawtooth => 1.0 - 2.0 * t,
        WaveFunc::Noise => 0.0,
    };
    wave.base + raw * wave.amplitude
}

/// One shader stage as a player/Ghoul2 surface draws it.
pub(in crate::cgame::player_presenter) struct StageMaterial<'a> {
    pub(in crate::cgame::player_presenter) image: &'a str,
    pub(in crate::cgame::player_presenter) clamp: bool,
    pub(in crate::cgame::player_presenter) alpha_mode: DynamicModelAlphaMode,
    pub(in crate::cgame::player_presenter) authored_glow: bool,
}

pub(in crate::cgame::player_presenter) struct MaterialLayers<'a> {
    pub(in crate::cgame::player_presenter) base: StageMaterial<'a>,
    pub(in crate::cgame::player_presenter) entity_tint: bool,
    pub(in crate::cgame::player_presenter) overlay: Option<StageMaterial<'a>>,
    /// Every drawable stage of a blended multi-stage shader, in order.
    pub(in crate::cgame::player_presenter) stages:
        Vec<(&'a jka_assets::shader::Stage, DynamicModelAlphaMode)>,
    pub(in crate::cgame::player_presenter) two_sided: bool,
}

/// Preserve an authored depth mask on alpha-blended Ghoul2 shader stages.
/// TaystJK's `ParseStage` sets GLS_DEPTHMASK_TRUE independently of blendFunc;
/// FinishShader then schedules a blended depth writer in SS_SEE_THROUGH.
/// Stage lighting is independent of the blend equation: `lightingDiffuse`
/// must use the light-grid path, not the unlit FX blend path.
fn blended_alpha_mode(stage: &jka_assets::shader::Stage) -> DynamicModelAlphaMode {
    let lit = matches!(stage.rgb_gen, RgbGen::LightingDiffuse | RgbGen::LightingDiffuseEntity);
    match (stage.depth_write, lit) {
        (true, true) => DynamicModelAlphaMode::BlendDepthWrite,
        (true, false) => DynamicModelAlphaMode::BlendUnlitDepthWrite,
        (false, true) => DynamicModelAlphaMode::Blend,
        (false, false) => DynamicModelAlphaMode::BlendUnlit,
    }
}

/// The pass mode of one stage of a blended player shader, or `None` when this
/// path cannot draw it.  Preserve TaystJK's framebuffer-dependent blend stages
/// (filter/modulate/darken) in their authored order; silently dropping these
/// changes the appearance of the remaining passes.  `tcGen environment` is
/// drawn; other generated coordinates and alpha tests still are not.
pub(in crate::cgame::player_presenter) fn blended_stage_mode(
    stage: &jka_assets::shader::Stage,
    allow_opaque: bool,
) -> Option<DynamicModelAlphaMode> {
    use crate::fx::draw::FxBlend;
    if stage.image.is_empty()
        || (stage.image.starts_with('$') && !stage.image.eq_ignore_ascii_case("$whiteimage"))
        || stage.entity_rgb
        || !stage.alpha_test.trim().is_empty()
        || !matches!(stage.tc_gen, TcGen::Base | TcGen::Environment)
    {
        return None;
    }
    match FxBlend::from_blend_func(&stage.blend) {
        FxBlend::Opaque => allow_opaque.then_some(DynamicModelAlphaMode::Opaque),
        FxBlend::Add => Some(DynamicModelAlphaMode::AdditiveOne),
        FxBlend::AddAlpha => Some(DynamicModelAlphaMode::Additive),
        FxBlend::Modulate => Some(DynamicModelAlphaMode::Modulate),
        FxBlend::DstColorAdd => Some(DynamicModelAlphaMode::DstColorAdd),
        FxBlend::Modulate2x => Some(DynamicModelAlphaMode::Modulate2x),
        FxBlend::Darken => Some(DynamicModelAlphaMode::Darken),
        FxBlend::Alpha => {
            let blend = stage
                .blend
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            (blend == "gl_src_alpha gl_one_minus_src_alpha" || blend == "blend")
                .then_some(blended_alpha_mode(stage))
        }
    }
}

/// Stage list for the glow/sparkle shaders of custom player models: a shader
/// whose primary stage is blended and has subsequent compositing stages, or
/// whose primary stage is a plain opaque one (ShadowTink's
/// dark `rgbGen const` body, an eye with a `glow` map) with further drawable
/// stages on top. Alpha-tested primaries keep the single-stage path.
pub(in crate::cgame::player_presenter) fn blended_stages<'a>(
    shader: &'a Shader,
    primary: &jka_assets::shader::Stage,
) -> Vec<(&'a jka_assets::shader::Stage, DynamicModelAlphaMode)> {
    use crate::fx::draw::FxBlend;
    if primary.entity_rgb || !primary.alpha_test.trim().is_empty() {
        return Vec::new();
    }
    let opaque_primary = FxBlend::from_blend_func(&primary.blend) == FxBlend::Opaque;
    let stages = shader
        .stages
        .iter()
        .filter_map(|stage| {
            blended_stage_mode(stage, opaque_primary && std::ptr::eq(stage, primary))
                .map(|mode| (stage, mode))
        })
        .collect::<Vec<_>>();
    if opaque_primary {
        // The opaque base must itself be drawable, else the overlays would
        // replace it. A lone base without tcMods keeps the compact path.
        let base_drawn = stages
            .first()
            .is_some_and(|&(stage, _)| std::ptr::eq(stage, primary));
        return if base_drawn && (stages.len() > 1 || !primary.tc_mods.is_empty()) {
            stages
        } else {
            Vec::new()
        };
    }
    let additive = stages.iter().any(|(_, mode)| {
        matches!(
            mode,
            DynamicModelAlphaMode::AdditiveOne | DynamicModelAlphaMode::Additive
        )
    });
    // Do not silently discard single-stage alpha/wave semantics just because
    // there is no later additive pass. This is also how standalone
    // depth-writing translucent materials retain their explicit first pass.
    if (stages.len() > 1)
        || additive
        || primary.depth_write
        || matches!(primary.rgb_gen, RgbGen::Wave(_))
        || matches!(primary.alpha_gen, AlphaGen::Wave(_))
        || !primary.tc_mods.is_empty()
    {
        stages
    } else {
        Vec::new()
    }
}

pub(in crate::cgame::player_presenter) fn resolved_stage(
    stage: &jka_assets::shader::Stage,
    alpha_mode: DynamicModelAlphaMode,
    texture: Option<Arc<TextureData>>,
) -> ResolvedStage {
    let const_rgb = stage.rgb_gen == RgbGen::Const;
    ResolvedStage {
        texture,
        alpha_mode,
        rgb: if const_rgb {
            stage.color.unwrap_or([1.0; 3])
        } else {
            [1.0; 3]
        },
        alpha: if stage.alpha_gen == AlphaGen::Const {
            stage.alpha.unwrap_or(1.0)
        } else {
            1.0
        },
        unlit: !matches!(stage.rgb_gen, RgbGen::LightingDiffuse | RgbGen::LightingDiffuseEntity),
        rgb_wave: match stage.rgb_gen {
            RgbGen::Wave(wave) => Some(wave),
            _ => None,
        },
        alpha_wave: match stage.alpha_gen {
            AlphaGen::Wave(wave) => Some(wave),
            _ => None,
        },
        specular_alpha: stage.alpha_gen == AlphaGen::LightingSpecular,
        tc_mods: stage.tc_mods.clone(),
        env_map: stage.tc_gen == TcGen::Environment,
        authored_glow: stage.glow,
    }
}

/// Entity frame and viewer for the per-draw terms of a stage.
pub(in crate::cgame::player_presenter) struct StageFrame {
    pub(in crate::cgame::player_presenter) axis: [[f32; 3]; 3],
    pub(in crate::cgame::player_presenter) origin: [f32; 3],
    pub(in crate::cgame::player_presenter) viewer: Option<[f32; 3]>,
}

impl StageFrame {
    /// q3's `RB_CalcSpecularAlpha` light: a fixed point in the entity's own
    /// frame, here in JKA world space, with the viewer.
    pub(in crate::cgame::player_presenter) fn specular_points(
        &self,
    ) -> Option<([f32; 3], [f32; 3])> {
        const LIGHT_LOCAL: [f32; 3] = [-960.0, 1980.0, 96.0];
        let viewer = self.viewer?;
        let light = std::array::from_fn(|i| {
            self.origin[i]
                + self.axis[0][i] * LIGHT_LOCAL[0]
                + self.axis[1][i] * LIGHT_LOCAL[1]
                + self.axis[2][i] * LIGHT_LOCAL[2]
        });
        Some((light, viewer))
    }
}

/// `RB_CalcSpecularAlpha` for one CPU-skinned vertex given in render space.
pub(in crate::cgame::player_presenter) fn specular_alpha(
    position: [f32; 3],
    normal: [f32; 3],
    light: [f32; 3],
    viewer: [f32; 3],
) -> f32 {
    // Render space is [x, z, -y] of JKA space.
    let to_jka = |v: [f32; 3]| Vec3::new(v[0], -v[2], v[1]);
    let position = to_jka(position);
    let normal = to_jka(normal).normalize_or_zero();
    let light_dir = (Vec3::from_array(light) - position).normalize_or_zero();
    let reflected = normal * (2.0 * normal.dot(light_dir)) - light_dir;
    let to_viewer = (Vec3::from_array(viewer) - position).normalize_or_zero();
    let l = reflected.dot(to_viewer);
    if l < 0.0 {
        0.0
    } else {
        (l * l * l * l).min(1.0)
    }
}

/// q3 `RB_CalcEnvironmentTexCoords` for one CPU-skinned vertex in render
/// space, with the viewer in JKA space (the GPU path's ghoul2_skin.wgsl twin).
pub(in crate::cgame::player_presenter) fn environment_uv(
    position: [f32; 3],
    normal: [f32; 3],
    viewer: [f32; 3],
) -> [f32; 2] {
    // Render space is [x, z, -y] of JKA space.
    let to_jka = |v: [f32; 3]| Vec3::new(v[0], -v[2], v[1]);
    let normal = to_jka(normal).normalize_or_zero();
    let to_viewer = (Vec3::from_array(viewer) - to_jka(position)).normalize_or_zero();
    let reflected = normal * (2.0 * normal.dot(to_viewer)) - to_viewer;
    [0.5 + reflected.y * 0.5, 0.5 - reflected.z * 0.5]
}

/// Compose id Tech 3 affine tcMods in authored order.  The 2x3 matrix
/// [m00,m01,m10,m11,tx,ty] maps [s,t] -> [m00*s+m10*t+tx,
/// m01*s+m11*t+ty]. Turbulence is position-dependent, not affine, and is
/// intentionally left for the per-vertex deformation implementation.
pub(in crate::cgame::player_presenter) fn stage_uv_affine(mods: &[TcMod], seconds: f32) -> [f32; 6] {
    let mut result = [1.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0];
    for tc_mod in mods {
        let op = match *tc_mod {
            TcMod::Scale(x, y) => [x, 0.0, 0.0, y, 0.0, 0.0],
            TcMod::Scroll(x, y) => {
                let (sx, sy) = (x * seconds, y * seconds);
                [1.0, 0.0, 0.0, 1.0, sx - sx.floor(), sy - sy.floor()]
            }
            TcMod::Transform(transform) => transform,
            TcMod::Rotate(degrees) => {
                // Same negative rotation sign and pivot (0.5,0.5) as
                // TaystJK RB_CalcRotateTexCoords.
                let (s, c) = (-degrees * seconds).to_radians().sin_cos();
                [c, s, -s, c, 0.5 - 0.5*c + 0.5*s, 0.5 - 0.5*s - 0.5*c]
            }
            TcMod::Stretch(wave) => {
                let phase = if matches!(wave.func, WaveFunc::Noise) {
                    (seconds + wave.phase) * wave.frequency
                } else { wave.phase + seconds * wave.frequency };
                let x = phase.rem_euclid(1.0);
                let signal = match wave.func {
                    WaveFunc::Sin => (x * std::f32::consts::TAU).sin(),
                    WaveFunc::Triangle => if x < 0.25 { 4.0*x } else if x < 0.75 { 2.0 - 4.0*x } else { 4.0*x - 4.0 },
                    WaveFunc::Square => if x < 0.5 { 1.0 } else { -1.0 },
                    WaveFunc::Sawtooth => x,
                    WaveFunc::InverseSawtooth => 1.0 - x,
                    WaveFunc::Noise => {
                        let n = phase.floor();
                        let t = x*x*(3.0 - 2.0*x);
                        let hash = |i: f32| ((i * 127.1).sin() * 43758.547).fract().rem_euclid(1.0)*2.0 - 1.0;
                        hash(n)*(1.0-t) + hash(n+1.0)*t
                    }
                };
                let value = wave.base + wave.amplitude * signal;
                let factor = if value.abs() <= 1.0e-6 { 1.0 } else { 1.0/value };
                [factor, 0.0, 0.0, factor, 0.5-0.5*factor, 0.5-0.5*factor]
            }
            TcMod::Turb { .. } => continue,
        };
        let prev = result;
        result = [
            op[0]*prev[0] + op[2]*prev[1],
            op[1]*prev[0] + op[3]*prev[1],
            op[0]*prev[2] + op[2]*prev[3],
            op[1]*prev[2] + op[3]*prev[3],
            op[0]*prev[4] + op[2]*prev[5] + op[4],
            op[1]*prev[4] + op[3]*prev[5] + op[5],
        ];
    }
    result
}

/// Regression-test the original four-float scale/scroll API. Runtime shader
/// stages now carry the full affine matrix, including cross-axis coefficients.
#[cfg(test)]
pub(in crate::cgame::player_presenter) fn stage_uv_xform(mods: &[TcMod], seconds: f32) -> [f32; 4] {
    let matrix = stage_uv_affine(mods, seconds);
    [matrix[0], matrix[3], matrix[4], matrix[5]]
}

pub(in crate::cgame::player_presenter) fn stage_material(
    stage: &jka_assets::shader::Stage,
) -> StageMaterial<'_> {
    let alpha_mode = if !stage.alpha_test.trim().is_empty() {
        DynamicModelAlphaMode::Mask
    } else {
        let mode = crate::fx::draw::FxBlend::from_blend_func(&stage.blend)
            .custom_shader_alpha_mode();
        if matches!(mode, DynamicModelAlphaMode::Blend | DynamicModelAlphaMode::BlendUnlit) {
            blended_alpha_mode(stage)
        } else {
            mode
        }
    };
    StageMaterial {
        image: stage.image.as_str(),
        clamp: stage.clamp,
        alpha_mode,
        authored_glow: stage.glow,
    }
}

/// The primary stage of a JKA shader decides the image, clamping and alpha
/// mode of a player/Ghoul2 surface. A shader whose primary stage is
/// `rgbGen lightingDiffuseEntity` (the `char_color_*` tint) also carries the
/// alpha-blended stage that redraws the untinted texture over it.
pub(in crate::cgame::player_presenter) fn material_layers<'a>(
    shaders: &'a BTreeMap<String, Shader>,
    shader_name: &'a str,
) -> MaterialLayers<'a> {
    let unresolved = || MaterialLayers {
        base: StageMaterial {
            image: shader_name,
            clamp: false,
            alpha_mode: DynamicModelAlphaMode::Opaque,
            authored_glow: false,
        },
        entity_tint: false,
        overlay: None,
        stages: Vec::new(),
        two_sided: false,
    };
    let Some(shader) = jka_assets::shader::find_shader(shaders, shader_name) else {
        return unresolved();
    };
    let Some(primary) = shader.primary() else {
        return unresolved();
    };
    if shader.is_glass() && crate::materials::glass_promotion_enabled() {
        return MaterialLayers {
            base: StageMaterial {
                image: primary.image.as_str(),
                clamp: primary.clamp,
                alpha_mode: DynamicModelAlphaMode::Glass,
                authored_glow: false,
            },
            entity_tint: false,
            overlay: None,
            stages: Vec::new(),
            two_sided: matches!(
                shader.cull.trim().to_ascii_lowercase().as_str(),
                "none" | "twosided" | "two-sided" | "disable"
            ),
        };
    }
    let overlay = primary
        .entity_rgb
        .then(|| {
            shader
                .stages
                .iter()
                .skip_while(|stage| !std::ptr::eq(*stage, primary))
                .skip(1)
                .find(|stage| {
                    !stage.image.is_empty()
                        && !stage.image.starts_with('$')
                        && !stage.entity_rgb
                        && stage.alpha_test.trim().is_empty()
                        && crate::fx::draw::FxBlend::from_blend_func(&stage.blend)
                            == crate::fx::draw::FxBlend::Alpha
                })
                .map(|stage| StageMaterial {
                    image: stage.image.as_str(),
                    clamp: stage.clamp,
                    // rgbGen lightingDiffuse: a lit, alpha-blended layer.
                    alpha_mode: DynamicModelAlphaMode::Blend,
                    authored_glow: stage.glow,
                })
        })
        .flatten();
    let stages = blended_stages(shader, primary);
    let base = stage_material(primary);
    // TaystJK's `cull twosided` (also `none` / `disable`) disables face
    // culling regardless of stage alphaFunc or blend mode. The Ghoul2
    // pipelines in DinurdoJK always cull back faces, so represent the same
    // rendering semantics by appending reversed-winding triangles to *every*
    // authored two-sided surface, including opaque and alpha-tested capes.
    let two_sided = matches!(
        shader.cull.to_ascii_lowercase().as_str(),
        "none" | "twosided" | "disable"
    );
    MaterialLayers {
        base,
        entity_tint: primary.entity_rgb,
        overlay,
        stages,
        two_sided,
    }
}

const MAX_MODEL_SOUND_CONFIG_BYTES: usize = 4096;

/// TaystJK/OpenJK `CG_LoadCISounds` model-gender source. The player configstring
/// can carry a gender hint, but the loaded character's authored gender is taken
/// from the selected `sounds*.cfg`: default skins try `sounds.cfg` then
/// `sounds_default.cfg`; named skins try `sounds_<skin>.cfg` then `sounds.cfg`.
/// The final line carries the legacy `f` marker for female models.
fn authored_model_is_female(
    source: &mut dyn ModelSource,
    info: &crate::cgame::ClientInfo,
) -> bool {
    let model = info.model_name.as_str();
    let candidates = if info.skin_name.is_empty() || info.skin_name.eq_ignore_ascii_case("default") {
        [
            format!("models/players/{model}/sounds.cfg"),
            format!("models/players/{model}/sounds_default.cfg"),
        ]
    } else {
        [
            format!("models/players/{model}/sounds_{}.cfg", info.skin_name),
            format!("models/players/{model}/sounds.cfg"),
        ]
    };

    for qpath in candidates {
        let Ok(Some(bytes)) = source.read_optional(&qpath, MAX_MODEL_SOUND_CONFIG_BYTES) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        // Match TaystJK CG_LoadCISounds: only the final line is the gender
        // marker, and the stock convention is a lowercase `f`.
        return text.lines().last().is_some_and(|line| line.contains('f'));
    }
    false
}

/// OpenJK `CG_RegisterClientModelname`: GLM + GLA + skin -> drawable surfaces.
const MAX_CHARACTER_ANIMATION_CFG_BYTES: usize = 60_000 - 1;
const MAX_CHARACTER_ANIMEVENTS_CFG_BYTES: usize = 80_000 - 1;

fn gla_sibling_qpath(gla_qpath: &str, filename: &str) -> Option<String> {
    let normalized = gla_qpath.replace('\\', "/");
    let (directory, _) = normalized.rsplit_once('/')?;
    Some(format!("{directory}/{filename}"))
}

enum CowAnimationSet<'a> {
    Borrowed(&'a jka_assets::animation::AnimationSet),
    Owned(jka_assets::animation::AnimationSet),
}

impl CowAnimationSet<'_> {
    fn as_ref(&self) -> &jka_assets::animation::AnimationSet {
        match self {
            Self::Borrowed(value) => value,
            Self::Owned(value) => value,
        }
    }
}

pub(in crate::cgame::player_presenter) fn mesh_skeleton_binding(
    glm: &jka_assets::ghoul2::GlmModel, animation: &Arc<GlaAnimation>,
) -> Result<(Arc<GlaAnimation>, Option<Arc<jka_assets::skeleton::PoseRetargeter>>), String> {
    use jka_assets::skeleton::{cached_retargeter, humanoid_reference, HumanoidRig};
    let humanoid = glm.anim_name.replace('\\', "/").to_ascii_lowercase().contains("players/_humanoid/");
    let reference = match (humanoid, glm.num_bones, animation.num_bones()) {
        (true, 72, 53) => Some(HumanoidRig::Jk2_72),
        (true, 53, 72) => Some(HumanoidRig::Jka53),
        _ => None,
    };
    if let Some(rig) = reference {
        let mesh = humanoid_reference(rig);
        let plan = cached_retargeter(&animation.skeleton, &mesh.skeleton)?;
        Ok((mesh, Some(plan)))
    } else {
        Ok((Arc::clone(animation), None))
    }
}

fn build_character_model(
    source: &mut dyn ModelSource,
    model_qpath: String,
    requested_skin_qpath: Option<String>,
    fallback_skin_qpath: Option<String>,
    female: bool,
    npc_model: bool,
    key: String,
    jiggle_remesh: JiggleRemeshTuning,
) -> Result<PlayerModelAsset, String> {
    let model_bytes = source.read_model(&model_qpath)?;
    let glm = Arc::new(jka_assets::ghoul2::parse_glm_native(&model_bytes)?);

    let gla_qpath = if glm.anim_name.to_ascii_lowercase().ends_with(".gla") {
        glm.anim_name.clone()
    } else {
        format!("{}.gla", glm.anim_name)
    };
    let animation_gla = source.load_gla(&gla_qpath)?;
    let (gla, retargeter) = mesh_skeleton_binding(&glm, &animation_gla)?;
    // Restore the universal-body-shaping whole-body path. Anny adds details;
    // its donor skeleton must not drive this model's height and age.
    let mut glm = glm;
    let body_shape_available = jka_assets::body_shape::procedural::supports(&glm, &gla, None);
    let body_shape_pivots = if body_shape_available && (!jiggle_remesh.body_shape.is_neutral() || !jiggle_remesh.body_details.is_neutral()) {
        jka_assets::body_shape::anny::apply_profile(
            Arc::make_mut(&mut glm), &gla, jiggle_remesh.body_shape, jiggle_remesh.body_details, None,
        )?.map(Arc::new)
    } else { None };
    let body_shape_binding = body_shape_pivots.as_deref().map(|p| {
        jka_assets::body_shape::BodyShapeBinding::new(&gla, None, p).map(Arc::new)
    }).transpose()?;
    if glm.num_bones > gla.skeleton.len() {
        return Err(format!(
            "GLM references {} bones but GLA only provides {}",
            glm.num_bones,
            gla.skeleton.len()
        ));
    }

    let normalized_gla = gla_qpath.replace('\\', "/").to_ascii_lowercase();
    let humanoid = normalized_gla.contains("players/_humanoid/");
    if !humanoid && !npc_model {
        // TaystJK CG_RegisterClientModelname retries the stock player model when
        // a multiplayer client GLM does not use players/_humanoid/. Creature
        // skeletons are accepted only through CG_G2AnimEntModelLoad (ET_NPC).
        return Err(format!(
            "unsupported multiplayer model GLA {gla_qpath:?}: expected players/_humanoid/"
        ));
    }

    // TaystJK CG_G2AnimEntModelLoad / CG_CacheG2AnimInfo:
    // - a non-humanoid's animation.cfg is selected from the loaded GLA directory;
    // - every NPC's animevents.cfg is selected from the original GLM directory.
    // Real players keep the presenter's already-loaded shared humanoid tables.
    let animation_qpath = gla_sibling_qpath(&gla_qpath, "animation.cfg")
        .ok_or_else(|| format!("{gla_qpath}: cannot derive animation.cfg path"))?;
    let animations = if humanoid {
        None
    } else {
        let animation_bytes = source
            .read_optional(&animation_qpath, MAX_CHARACTER_ANIMATION_CFG_BYTES)?
            .ok_or_else(|| format!("missing {animation_qpath}"))?;
        Some(Arc::new(parse_animation_cfg(&animation_bytes)?))
    };
    let anim_events = if npc_model {
        let event_animations = if let Some(animations) = animations.as_deref() {
            CowAnimationSet::Borrowed(animations)
        } else {
            // Humanoid NPCs still get events from their model directory in
            // TaystJK, indexed against localAnimIndex 0 (the humanoid table).
            let animation_bytes = source
                .read_optional(&animation_qpath, MAX_CHARACTER_ANIMATION_CFG_BYTES)?
                .ok_or_else(|| format!("missing {animation_qpath}"))?;
            CowAnimationSet::Owned(parse_animation_cfg(&animation_bytes)?)
        };
        let events_qpath = gla_sibling_qpath(&model_qpath, "animevents.cfg");
        if let Some(events_qpath) = events_qpath {
            match source.read_optional(&events_qpath, MAX_CHARACTER_ANIMEVENTS_CFG_BYTES)? {
                Some(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    Some(Arc::new(parse_animation_events(&text, event_animations.as_ref())))
                }
                None => None,
            }
        } else {
            None
        }
    } else {
        None
    };

    let jiggle = if humanoid {
        match JiggleProfile::auto_humanoid(&glm, &gla, body_shape_pivots.as_deref().map(Vec::as_slice)) {
            Ok(Some(profile)) => {
                devprintln!(
                    2,
                    "PLAYER JIGGLE: {model_qpath} auto regions={}",
                    profile.region_count(),
                );
                Some(Arc::new(profile))
            }
            Ok(None) => None,
            Err(error) => {
                rverbose!(1, "PLAYER JIGGLE AUTO WARNING: {model_qpath}: {error}");
                None
            }
        }
    } else {
        None
    };

    let skin_map = if let Some(skin_qpath) = requested_skin_qpath.as_deref() {
        let skin = match source.load_skin(skin_qpath) {
            Ok(skin) => Some(skin),
            Err(primary_error) => {
                if let Some(fallback) = fallback_skin_qpath.as_deref() {
                    rverbose!(
                        1,
                        "PLAYER SKIN FALLBACK: {skin_qpath}: {primary_error}; trying {fallback}"
                    );
                    Some(source.load_skin(fallback)?)
                } else {
                    // Ghoul2 skin handle 0: an NPC model remains valid when an
                    // appended skin fails to register; embedded GLM shaders win.
                    rverbose!(
                        1,
                        "NPC APPENDED SKIN WARNING: {skin_qpath}: {primary_error}; using model shaders"
                    );
                    None
                }
            }
        };
        skin.map_or_else(HashMap::new, |skin| {
            skin.into_iter()
                .map(|surface| (surface.name.to_ascii_lowercase(), surface.shader))
                .collect::<HashMap<_, _>>()
        })
    } else {
        HashMap::new()
    };

    let lod = glm
        .lods
        .first()
        .ok_or_else(|| format!("{model_qpath} has no GLM LODs"))?;
    let joined_breast_bakes = build_joined_breast_bakes(&glm, jiggle.as_deref(), jiggle_remesh)?;
    let mut surfaces = Vec::with_capacity(lod.surfaces.len());
    for surface in &lod.surfaces {
        let hierarchy = glm.hierarchy.get(surface.surface_index).ok_or_else(|| {
            format!(
                "{model_qpath}: missing hierarchy for surface {}",
                surface.surface_index
            )
        })?;
        let skin_shader = skin_map
            .get(&hierarchy.name.to_ascii_lowercase())
            .map(String::as_str);
        let skin_off = skin_shader.is_some_and(|shader| shader.eq_ignore_ascii_case("*off"));
        // TaystJK RenderSurfaces: only zero-flag surfaces are rendered.
        // ISBOLT (0x1) is attachment geometry; OFF (0x2) is hidden geometry.
        let default_visible = !skin_off && hierarchy.flags == 0;
        let shader_name = if skin_off {
            hierarchy.shader.as_str()
        } else {
            skin_shader.unwrap_or(hierarchy.shader.as_str())
        };
        if shader_name.is_empty() || shader_name.eq_ignore_ascii_case("*off") {
            continue;
        }
        let material = source.resolve_material(shader_name);
        let two_sided = material.two_sided;
        surfaces.push(PlayerSurfaceAsset {
            surface_index: surface.surface_index,
            default_visible,
            surface_name: hierarchy.name.clone(),
            texture: material.texture,
            alpha_mode: material.alpha_mode,
            authored_glow: material.authored_glow,
            shader_time_offset: material.shader_time_offset,
            shader_clamp_time: material.shader_clamp_time,
            entity_tint: material.entity_tint,
            overlay: material.overlay,
            stages: material.stages,
            fallback_gray: false,
            // GLM surfaces are described by a JKA shader, never a VMT.
            source_material: None,
            gpu_meshes: build_lod_gpu_meshes(
                &model_qpath,
                &glm,
                surface.surface_index,
                two_sided,
                jiggle.as_deref(),
                jiggle_remesh,
                &joined_breast_bakes,
            )?,
        });
    }

    devprintln!(
        2,
        "GHOUL2 CHARACTER MODEL: {} skin={} surfaces={} gla={} bones={} humanoid={}",
        model_qpath,
        requested_skin_qpath.as_deref().unwrap_or("<none>"),
        surfaces.len(),
        gla_qpath,
        gla.skeleton.len(),
        humanoid,
    );

    Ok(PlayerModelAsset {
        body_shape_available,
        body_shape_binding,
        idle_effects: Vec::new(),
        key,
        glm,
        gla,
        animation_gla,
        retargeter,
        source_native_pivots: body_shape_pivots,
        source_jiggles: None,
        source_jiggles_enabled: false,
        emote_contacts: Default::default(),
        humanoid,
        animations,
        anim_events,
        surfaces,
        jiggle,
        female,
    })
}

/// OpenJK/TaystJK multiplayer player registration remains humanoid/fallback
/// based.  Only ET_NPC takes the exact model-index path below.
pub(in crate::cgame::player_presenter) fn build_player_model(
    source: &mut dyn ModelSource,
    info: &crate::cgame::ClientInfo,
    key: String,
    jiggle_remesh: JiggleRemeshTuning,
) -> Result<PlayerModelAsset, String> {
    // A Source 1 / Garry's Mod character is declared by source_model.json and has
    // no model.glm at all. It is rebound onto `_humanoid.gla` here, so from this
    // point on it is an ordinary Ghoul2 player to every other consumer.
    let descriptor_qpath = source_descriptor_qpath(&info.model_name);
    if let Some(descriptor) =
        source.read_optional(&descriptor_qpath, source_player::MAX_DESCRIPTOR_BYTES)?
    {
        return build_source_player_model(
            source,
            &info.model_name,
            key,
            &descriptor_qpath,
            &descriptor,
            jiggle_remesh,
        );
    }
    let female = authored_model_is_female(source, info);
    build_character_model(
        source,
        info.model_qpath(),
        Some(info.skin_qpath()),
        Some(info.default_skin_qpath()),
        female,
        false,
        key,
        jiggle_remesh,
    )
}

pub(in crate::cgame::player_presenter) fn build_npc_model(
    source: &mut dyn ModelSource,
    request: &crate::cgame::NpcModelRequest,
    key: String,
    jiggle_remesh: JiggleRemeshTuning,
) -> Result<PlayerModelAsset, String> {
    build_character_model(
        source,
        request.model_qpath.clone(),
        request.skin_qpath.clone(),
        None,
        false,
        true,
        key,
        jiggle_remesh,
    )
}

pub(in crate::cgame::player_presenter) fn npc_model_key(
    request: &crate::cgame::NpcModelRequest,
) -> String {
    format!(
        "npc:{}|{}",
        request.model_qpath,
        request.skin_qpath.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase()
}

pub(in crate::cgame::player_presenter) fn build_static_glm(
    source: &mut dyn ModelSource,
    model_qpath: &str,
    custom_skin: Option<&str>,
    label: &str,
    preview_fallback: bool,
) -> Result<SaberModelAsset, String> {
    let model_bytes = source.read_model(model_qpath)?;
    let glm = Arc::new(jka_assets::ghoul2::parse_glm_native(&model_bytes)?);
    let gla_qpath = if glm.anim_name.to_ascii_lowercase().ends_with(".gla") {
        glm.anim_name.clone()
    } else {
        format!("{}.gla", glm.anim_name)
    };
    let animation_gla = source.load_gla(&gla_qpath)?;
    let (gla, retargeter) = mesh_skeleton_binding(&glm, &animation_gla)?;
    // Vehicle registration in CG_G2AnimEntModelLoad uses the animation table
    // beside the GLA. The static weapon/hilt path must not load these tables.
    let animations = if label.starts_with('$') {
        // TaystJK CG_G2AnimEntModelLoad initializes Ghoul2 BEFORE trying
        // BG_ParseAnimationFile. An absent/bad vehicle animation.cfg gives
        // localAnimIndex=-1; it must not unregister the ship model itself.
        let cfg = gla_sibling_qpath(&gla_qpath, "animation.cfg");
        match cfg.as_deref() {
            Some(cfg) => match source.read_optional(cfg, MAX_CHARACTER_ANIMATION_CFG_BYTES) {
                Ok(Some(bytes)) => match parse_animation_cfg(&bytes) {
                    Ok(animations) => Some(Arc::new(animations)),
                    Err(error) => {
                        log::warn!("VEHICLE ANIMATION OPTIONAL: model={} cfg={} parse error={} (rendering static pose)", model_qpath, cfg, error);
                        None
                    }
                },
                Ok(None) => None,
                Err(error) => {
                    log::warn!("VEHICLE ANIMATION OPTIONAL: model={} cfg={} read error={} (rendering static pose)", model_qpath, cfg, error);
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };
    // Match OpenJK's MDXM registration rule: the mesh may use fewer
    // bones than the referenced GLA, it just may not reference beyond it.
    if glm.num_bones > gla.skeleton.len() {
        return Err(format!(
            "GLM references {} bones but GLA only provides {}",
            glm.num_bones,
            gla.skeleton.len()
        ));
    }

    let skin_map = if let Some(skin_qpath) = custom_skin {
        match source.load_skin(skin_qpath) {
            Ok(skin) => skin
                .into_iter()
                .map(|surface| (surface.name.to_ascii_lowercase(), surface.shader))
                .collect::<HashMap<_, _>>(),
            Err(error) => {
                // TaystJK R_RegisterSkin can return skinID=0 without failing
                // G2API_InitGhoul2Model. An optional skin registration failure
                // must not make the entire vehicle/NPC/weapon disappear.
                // With no skin, Ghoul2 uses the GLM's embedded shaders.
                log::warn!(
                    "GHOUL2 OPTIONAL SKIN: model={} skin={} error={}; using embedded shader names",
                    model_qpath, skin_qpath, error,
                );
                HashMap::new()
            },
        }
    } else {
        HashMap::new()
    };
    let lod = glm
        .lods
        .first()
        .ok_or_else(|| format!("{model_qpath} has no GLM LODs"))?;
    let mut surfaces = Vec::with_capacity(lod.surfaces.len());
    let mut missing_images = std::collections::HashSet::new();
    for surface in &lod.surfaces {
        let hierarchy = glm.hierarchy.get(surface.surface_index).ok_or_else(|| {
            format!(
                "{model_qpath}: missing hierarchy for surface {}",
                surface.surface_index
            )
        })?;
        // Match TaystJK's RenderSurfaces: nonzero hierarchy flags don't draw.
        // This excludes bolt triangles and authored OFF surfaces without relying
        // on naming conventions; the original GLM still retains attachment data.
        if hierarchy.flags != 0 {
            continue;
        }
        let shader_name = skin_map
            .get(&hierarchy.name.to_ascii_lowercase())
            .map(String::as_str)
            .unwrap_or(hierarchy.shader.as_str());
        if shader_name.eq_ignore_ascii_case("*off") {
            continue;
        }
        let missing_shader = shader_name.is_empty();
        let shader_name = if missing_shader {
            // In OpenJK, even when R_RegisterSkin returns zero, RenderSurfaces
            // submits the mesh with the renderer's default shader. Do not
            // erase a ship simply because both its embedded name and optional
            // skin mapping are unavailable. $whiteimage is our neutral default
            // material, while the asset log above retains the missing path.
            "$whiteimage"
        } else {
            shader_name
        };
        let material = source.resolve_material(shader_name);
        // Report the exact unresolved GLM shader on the asset-loading worker.
        // A missing image can otherwise look like a solid bright material;
        // never paper over it with a model-specific shader or color override.
        if material.texture.is_none()
            && !shader_name.eq_ignore_ascii_case("$whiteimage")
            && missing_images.insert(shader_name.to_ascii_lowercase())
        {
            log::warn!(
                "GLM MATERIAL MISSING: model={} surface={} shader={} (check its image in the active PK3/VFS)",
                model_qpath, hierarchy.name, shader_name);
        }
        let fallback_gray = preview_fallback
            && (missing_shader
                || (!shader_name.eq_ignore_ascii_case("$whiteimage")
                    && material.texture.is_none()));
        surfaces.push(PlayerSurfaceAsset {
            surface_index: surface.surface_index,
            default_visible: true,
            surface_name: hierarchy.name.clone(),
            texture: material.texture,
            alpha_mode: material.alpha_mode,
            authored_glow: material.authored_glow,
            shader_time_offset: material.shader_time_offset,
            shader_clamp_time: material.shader_clamp_time,
            entity_tint: material.entity_tint,
            overlay: material.overlay,
            stages: material.stages,
            fallback_gray,
            source_material: None,
            gpu_meshes: build_lod_gpu_meshes(
                &model_qpath,
                &glm,
                surface.surface_index,
                false,
                None,
                JiggleRemeshTuning::default(),
                &HashMap::new(),
            )?,
        });
    }
    devprintln!(
        2,
        "GHOUL2 STATIC MODEL REGISTERED: name={} model={} skin={} surfaces={} glmBones={} glaBones={} gla={}",
        label,
        model_qpath,
        custom_skin.unwrap_or("<default>"),
        surfaces.len(),
        glm.num_bones,
        gla.skeleton.len(),
        gla_qpath,
    );
    Ok(SaberModelAsset { glm, gla, animation_gla, retargeter, animations, surfaces })
}

impl PlayerPresenter {
    pub(in crate::cgame::player_presenter) fn load_model(
        &mut self,
        info: &crate::cgame::ClientInfo,
    ) -> Result<Arc<PlayerModelAsset>, String> {
        match self.try_load_model(info) {
            Ok(model) => Ok(model),
            Err(primary_error) => {
                let fallback = info.missing_model_fallback();
                self.try_load_model(&fallback).map_err(|fallback_error| {
                    format!(
                        "{primary_error}; fallback {}/{} failed: {fallback_error}",
                        fallback.model_name, fallback.skin_name,
                    )
                })
            }
        }
    }

    /// NPC counterpart to `load_model`: exact CS_MODELS model, no Kyle/Jan
    /// fallback.  This matches TaystJK's per-centity Ghoul2 ownership.
    pub(in crate::cgame::player_presenter) fn load_npc_model(
        &mut self,
        request: &crate::cgame::NpcModelRequest,
    ) -> Result<Arc<PlayerModelAsset>, String> {
        self.try_load_npc_model(request)
    }

    pub(in crate::cgame::player_presenter) fn try_load_npc_model(
        &mut self,
        request: &crate::cgame::NpcModelRequest,
    ) -> Result<Arc<PlayerModelAsset>, String> {
        let key = npc_model_key(request);
        match self.lookup_model(&key) {
            ModelLookup::Ready(model) => return Ok(model),
            ModelLookup::Failed(error) => return Err(error),
            ModelLookup::Pending | ModelLookup::Missing => {}
        }

        let jiggle_remesh = self.jiggle_remesh;
        let result = build_npc_model(
            &mut SyncModelSource { presenter: self },
            request,
            key.clone(),
            jiggle_remesh,
        );
        match result {
            Ok(model) => {
                let model = Arc::new(model);
                self.models.insert(key, Arc::clone(&model));
                Ok(model)
            }
            Err(error) => {
                println!("NPC MODEL REGISTRATION FAILED: {key}: {error}");
                self.failed_models.insert(key, error.clone());
                Err(error)
            }
        }
    }

    pub(in crate::cgame::player_presenter) fn report_player_status(
        &mut self,
        entity_num: u16,
        status: String,
    ) {
        if self.player_diagnostics.get(&entity_num) != Some(&status) {
            // In a protocol-26 demo, a missing CS_MODELS entry is not a
            // missing PK3: the server never supplied the vehicle identity.
            // Make that actionable at normal console verbosity instead of
            // silently reporting an invisible vehicle only at developer 2.
            if status.contains("has no CS_MODELS entry") {
                println!("VEHICLE MODEL UNRESOLVED: entity={entity_num} {status}; missing server/demo registration, not a guessed model name");
            } else if status.starts_with("vehicle submitted=0 reason=") {
                // A missing GLM, GLA or .veh definition can otherwise look
                // exactly like a valid EF_NODRAW vehicle; report the complete
                // registration chain once when the status changes.
                println!("VEHICLE PRESENTATION FAILED: entity={entity_num} {status}");
            } else {
                devprintln!(2, "PLAYER PRESENTATION: entity={entity_num} {status}");
            }
            self.player_diagnostics.insert(entity_num, status);
        }
    }

    pub(in crate::cgame::player_presenter) fn try_load_model(
        &mut self,
        info: &crate::cgame::ClientInfo,
    ) -> Result<Arc<PlayerModelAsset>, String> {
        let key = player_model_key(info);
        match self.lookup_model(&key) {
            ModelLookup::Ready(model) => return Ok(model),
            ModelLookup::Failed(error) => return Err(error),
            // Synchronous callers (previews, `cg_asyncAssets 0`) need the
            // answer now; an in-flight async copy just becomes redundant.
            ModelLookup::Pending | ModelLookup::Missing => {}
        }

        let result = self.load_model_uncached(info, key.clone());
        match result {
            Ok(model) => {
                let model = Arc::new(model);
                self.models.insert(key, Arc::clone(&model));
                Ok(model)
            }
            Err(error) => {
                println!("PLAYER MODEL REGISTRATION FAILED: {key}: {error}");
                self.failed_models.insert(key, error.clone());
                Err(error)
            }
        }
    }

    pub(in crate::cgame::player_presenter) fn load_model_uncached(
        &mut self,
        info: &crate::cgame::ClientInfo,
        key: String,
    ) -> Result<PlayerModelAsset, String> {
        let jiggle_remesh = self.jiggle_remesh;
        build_player_model(
            &mut SyncModelSource { presenter: self },
            info,
            key,
            jiggle_remesh,
        )
    }

    pub(in crate::cgame::player_presenter) fn lookup_model(&self, key: &str) -> ModelLookup {
        if let Some(model) = self.models.get(key) {
            return ModelLookup::Ready(Arc::clone(model));
        }
        if let Some(error) = self.failed_models.get(key) {
            return ModelLookup::Failed(error.clone());
        }
        match self.async_models.state(key) {
            Some(AssetState::Ready(model)) => ModelLookup::Ready(Arc::clone(model)),
            Some(AssetState::Failed(error)) => ModelLookup::Failed(error.clone()),
            Some(AssetState::Pending) => ModelLookup::Pending,
            None => ModelLookup::Missing,
        }
    }

    /// Queue `info`'s model on the asset workers unless it is already known.
    /// Returns immediately; cache hits and pending requests cost one lookup.
    pub(in crate::cgame::player_presenter) fn request_player_model(
        &mut self,
        info: &crate::cgame::ClientInfo,
        key: &str,
    ) -> ModelLookup {
        let lookup = self.lookup_model(key);
        if !matches!(lookup, ModelLookup::Missing) {
            if matches!(lookup, ModelLookup::Pending) {
                self.async_models.note_duplicate();
            }
            return lookup;
        }
        let source = Arc::clone(&self.asset_source);
        let shared = Arc::clone(&self.load_shared);
        let job_info = info.clone();
        let job_key = key.to_owned();
        let jiggle_remesh = self.jiggle_remesh;
        let requested = self
            .async_models
            .request(key, AssetPriority::High, move || {
                asset_jobs::with_worker_vfs(&source, |vfs| {
                    build_player_model(
                        &mut WorkerModelSource {
                            vfs,
                            shared: &shared,
                        },
                        &job_info,
                        job_key,
                        jiggle_remesh,
                    )
                })?
            });
        match requested {
            Requested::Ready(model) => ModelLookup::Ready(model),
            Requested::Pending => ModelLookup::Pending,
            Requested::Failed(error) => ModelLookup::Failed(error),
            Requested::Rejected => ModelLookup::Missing,
        }
    }

    pub(in crate::cgame::player_presenter) fn request_npc_model(
        &mut self,
        request: &crate::cgame::NpcModelRequest,
        key: &str,
    ) -> ModelLookup {
        let lookup = self.lookup_model(key);
        if !matches!(lookup, ModelLookup::Missing) {
            if matches!(lookup, ModelLookup::Pending) {
                self.async_models.note_duplicate();
            }
            return lookup;
        }
        let source = Arc::clone(&self.asset_source);
        let shared = Arc::clone(&self.load_shared);
        let job_request = request.clone();
        let job_key = key.to_owned();
        let jiggle_remesh = self.jiggle_remesh;
        let requested = self.async_models.request(key, AssetPriority::High, move || {
            asset_jobs::with_worker_vfs(&source, |vfs| {
                build_npc_model(
                    &mut WorkerModelSource { vfs, shared: &shared },
                    &job_request,
                    job_key,
                    jiggle_remesh,
                )
            })?
        });
        match requested {
            Requested::Ready(model) => ModelLookup::Ready(model),
            Requested::Pending => ModelLookup::Pending,
            Requested::Failed(error) => ModelLookup::Failed(error),
            Requested::Rejected => ModelLookup::Missing,
        }
    }

    /// Exact async NPC resolution.  Deliberately has no provisional/default
    /// player body: TaystJK never turns a failed Wampa registration into Kyle.
    pub(in crate::cgame::player_presenter) fn resolve_npc_model_async(
        &mut self,
        request: &crate::cgame::NpcModelRequest,
    ) -> ModelResolution {
        let key = npc_model_key(request);
        match self.request_npc_model(request, &key) {
            ModelLookup::Ready(model) => ModelResolution::Ready(model),
            ModelLookup::Failed(error) => ModelResolution::Failed(error),
            ModelLookup::Pending | ModelLookup::Missing => ModelResolution::Loading,
        }
    }

    pub(in crate::cgame::player_presenter) fn async_loading_enabled(&self) -> bool {
        self.async_loading && asset_jobs::async_available()
    }

    #[allow(dead_code)]
    pub fn set_async_loading(&mut self, enabled: bool) {
        self.async_loading = enabled;
    }

    /// Per-frame bounded drain of finished asset jobs. A no-op (one integer
    /// compare) whenever nothing is in flight.
    pub(in crate::cgame::player_presenter) fn poll_asset_completions(&mut self) {
        if self.async_models.pending_count() > 0 {
            self.async_models.drain(16);
        }
        if self.async_static_models.pending_count() > 0 {
            self.async_static_models.drain(16);
        }
    }

    /// Warm the fallback bodies OpenJK falls back to on registration failure so
    /// a brand-new remote player has something to show while their real model
    /// loads. Runs on the workers; nothing blocks.
    pub(in crate::cgame::player_presenter) fn prime_default_player_models(&mut self) {
        if !self.async_loading_enabled() {
            return;
        }
        for name in ["kyle", "jan"] {
            let info = crate::cgame::ClientInfo::solo_model(name);
            let key = player_model_key(&info);
            self.request_player_model(&info, &key);
        }
    }

    /// Model to present for `info` without ever blocking on IO.
    ///
    /// The desired key and the cache are distinct: a completion only fills its
    /// own key, so a stale finish can never overwrite a newer selection, and a
    /// model finished for one player is reused by every other player.
    pub(in crate::cgame::player_presenter) fn resolve_player_model_async(
        &mut self,
        info: &crate::cgame::ClientInfo,
    ) -> ModelResolution {
        let key = player_model_key(info);
        let fallback = info.missing_model_fallback();
        match self.request_player_model(info, &key) {
            ModelLookup::Ready(model) => return ModelResolution::Ready(model),
            ModelLookup::Failed(primary_error) => {
                // CG_LoadClientInfo failure policy: same as the synchronous path.
                let fallback_key = player_model_key(&fallback);
                return match self.request_player_model(&fallback, &fallback_key) {
                    ModelLookup::Ready(model) => ModelResolution::Ready(model),
                    ModelLookup::Failed(fallback_error) => ModelResolution::Failed(format!(
                        "{primary_error}; fallback {}/{} failed: {fallback_error}",
                        fallback.model_name, fallback.skin_name,
                    )),
                    ModelLookup::Pending | ModelLookup::Missing => ModelResolution::Loading,
                };
            }
            ModelLookup::Pending | ModelLookup::Missing => {}
        }
        // Not ready yet: show the OpenJK fallback body if it is already
        // resident (the plain default bodies are primed at creation). Only peek;
        // never queue extra loads on behalf of a model that is merely pending.
        let plain = crate::cgame::ClientInfo::solo_model(if info.female { "jan" } else { "kyle" });
        for candidate in [&fallback, &plain] {
            let candidate_key = player_model_key(candidate);
            if candidate_key == key {
                continue;
            }
            if let ModelLookup::Ready(model) = self.lookup_model(&candidate_key) {
                return ModelResolution::Provisional(model);
            }
        }
        ModelResolution::Loading
    }

    /// Re-scan the active VFS namespace and discard remembered registration
    /// failures. Successful model caches remain hot; this is the targeted
    /// `fs_refresh` boundary for content that appeared on disk after CGame was
    /// created, not a destructive CGame restart.
    pub fn retry_failed_assets(&mut self) -> Result<usize, String> {
        self.assets
            .refresh()
            .map_err(|error| format!("PLAYER ASSET REFRESH ERROR: {error}"))?;

        // A newly mounted package may contribute shader definitions needed by
        // the model that previously failed. Rebuild the lightweight definition
        // table while preserving already-decoded model/texture caches.
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut self.assets, &mut shader_warnings, self.pbr)?;
        self.shaders = Arc::new(shaders);
        for warning in shader_warnings {
            rverbose!(1, "PLAYER MATERIAL REFRESH WARNING: {warning}");
        }
        println!(
            "PLAYER ASSET REFRESH: shaderDefs={} mtrDefs={}",
            diagnostics.shader_definitions, diagnostics.mtr_definitions
        );

        let retried = self.async_models.retry_failed() + self.async_static_models.retry_failed();
        self.failed_models.clear();
        self.failed_saber_models.clear();
        self.reported_saber_warnings.clear();
        self.reported_saber_diagnostics.clear();
        self.reported_vehicle_fallbacks.clear();
        self.asset_source = AssetSource::from_search_path(&self.assets);
        // Worker-side GLA/texture caches remember failures too. A new load-shared
        // object makes future jobs resolve against the refreshed shader table.
        self.load_shared = Arc::new(ModelLoadShared::new(
            Arc::clone(&self.shaders),
            self.shader_remaps.clone(),
        ));
        self.blob_shadow_texture_resolved = false;
        self.blob_shadow_asset_warned = false;
        if retried > 0 {
            devprintln!(
                1,
                "[ASSET] retrying {retried} previously failed player asset(s)"
            );
        }
        Ok(retried)
    }

    pub(in crate::cgame::player_presenter) fn register_client_saber_assets(
        &mut self,
        info: &crate::cgame::ClientInfo,
        entity_num: u16,
    ) {
        // OpenJK WP_SetSaber removes saber slot 1 for "none"/"remove" (and for
        // two-handed combinations) and leaves an empty model. Those slots are
        // skipped rather than fed through the missing-definition fallback,
        // which would incorrectly create the stock Reborn hilt.
        let equipped = self.equipped_sabers(info);
        for (saber_name, definition) in [info.saber_name.as_str(), info.saber2_name.as_str()]
            .into_iter()
            .zip(equipped)
        {
            let Some(definition) = definition else {
                continue;
            };
            if saber_name.is_empty() {
                continue;
            }
            if let Err(error) = self.load_saber_model_in_game(&definition) {
                self.report_saber_warning_once(entity_num, &error);
            }
        }
    }

    pub(in crate::cgame::player_presenter) fn vehicle_fallback(
        &mut self,
        entity_num: u16,
        requested: &str,
        resolved_glm: Option<&str>,
        resolved_skin: Option<&str>,
        primary_error: &str,
    ) -> Result<Arc<SaberModelAsset>, String> {
        const SWOOP: &str = "models/players/swoop/model.glm";
        const SWOOP_SKIN: &str = "models/players/swoop/model_default.skin";
        let resolved = resolved_glm.unwrap_or("<unresolved vehicle definition>");
        let warning_key = format!("{}|{}|{}", requested, resolved, resolved_skin.unwrap_or(""))
            .to_ascii_lowercase();
        if self.reported_vehicle_fallbacks.insert(warning_key) {
            println!(
                "VEHICLE MODEL FALLBACK: entity={} requested={} resolved={} skin={} reason={}; using {}",
                entity_num,
                requested,
                resolved,
                resolved_skin.unwrap_or("<default>"),
                primary_error,
                SWOOP,
            );
        }
        self.load_static_glm_in_game(SWOOP, Some(SWOOP_SKIN), SWOOP)
            .map_err(|fallback_error| {
                format!(
                    "vehicle {requested} ({resolved}) failed ({primary_error}); fallback {SWOOP} failed: {fallback_error}"
                )
            })
    }

    pub(in crate::cgame::player_presenter) fn load_saber_model(
        &mut self,
        definition: &SaberDefinition,
    ) -> Result<Arc<SaberModelAsset>, String> {
        self.load_static_glm(
            &definition.model,
            definition.custom_skin.as_deref(),
            &definition.name,
        )
    }

    pub(in crate::cgame::player_presenter) fn load_saber_model_in_game(
        &mut self,
        definition: &SaberDefinition,
    ) -> Result<Arc<SaberModelAsset>, String> {
        self.load_static_glm_in_game(
            &definition.model,
            definition.custom_skin.as_deref(),
            &definition.name,
        )
    }

    /// Gameplay/presentation registration of a Ghoul2 hilt, weapon or vehicle
    /// model. Cache hits are one lookup. A miss queues the load on the asset
    /// workers and returns an `ASSET_PENDING` error, which callers treat as
    /// "nothing to draw yet"; explicit/preview paths keep `load_static_glm`.
    pub(in crate::cgame::player_presenter) fn load_static_glm_in_game(
        &mut self,
        model_qpath: &str,
        custom_skin: Option<&str>,
        label: &str,
    ) -> Result<Arc<SaberModelAsset>, String> {
        if !self.async_loading_enabled() {
            return self.load_static_glm(model_qpath, custom_skin, label);
        }
        let mut key = static_model_key(model_qpath, custom_skin, false);
        // The same GLM may be registered as a weapon/hilt and as a vehicle;
        // only the latter loads the model-local animation table. A shared key
        // would silently return a static model when the weapon loaded first.
        if label.starts_with('$') {
            key.push_str("|vehicle_animations=1");
        }
        if let Some(model) = self.saber_models.get(&key) {
            return Ok(Arc::clone(model));
        }
        if self.failed_saber_models.contains(&key) {
            return Err(format!(
                "Ghoul2 model {model_qpath} failed registration earlier"
            ));
        }
        match self.async_static_models.state(&key) {
            Some(AssetState::Ready(model)) => return Ok(Arc::clone(model)),
            Some(AssetState::Failed(error)) => return Err(error.clone()),
            Some(AssetState::Pending) => {
                self.async_static_models.note_duplicate();
                return Err(format!("Ghoul2 model {model_qpath}: {ASSET_PENDING}"));
            }
            None => {}
        }
        let source = Arc::clone(&self.asset_source);
        let shared = Arc::clone(&self.load_shared);
        let job_qpath = model_qpath.to_owned();
        let job_skin = custom_skin.map(str::to_owned);
        let job_label = label.to_owned();
        match self
            .async_static_models
            .request(&key, AssetPriority::High, move || {
                asset_jobs::with_worker_vfs(&source, |vfs| {
                    build_static_glm(
                        &mut WorkerModelSource {
                            vfs,
                            shared: &shared,
                        },
                        &job_qpath,
                        job_skin.as_deref(),
                        &job_label,
                        false,
                    )
                })?
            }) {
            Requested::Ready(model) => Ok(model),
            Requested::Failed(error) => Err(error),
            Requested::Pending | Requested::Rejected => {
                Err(format!("Ghoul2 model {model_qpath}: {ASSET_PENDING}"))
            }
        }
    }

    /// A Ghoul2 model drawn in its own default pose (saber hilts, weapon
    /// item world models): G2API_InitGhoul2Model with an optional skin.
    pub(in crate::cgame::player_presenter) fn load_static_glm(
        &mut self,
        model_qpath: &str,
        custom_skin: Option<&str>,
        label: &str,
    ) -> Result<Arc<SaberModelAsset>, String> {
        self.load_static_glm_with_options(model_qpath, custom_skin, label, false)
    }

    pub(in crate::cgame::player_presenter) fn load_static_glm_with_options(
        &mut self,
        model_qpath: &str,
        custom_skin: Option<&str>,
        label: &str,
        preview_fallback: bool,
    ) -> Result<Arc<SaberModelAsset>, String> {
        let mut key = static_model_key(model_qpath, custom_skin, preview_fallback);
        if label.starts_with('$') {
            key.push_str("|vehicle_animations=1");
        }
        if let Some(model) = self.saber_models.get(&key) {
            return Ok(Arc::clone(model));
        }
        if self.failed_saber_models.contains(&key) {
            return Err(format!(
                "Ghoul2 model {model_qpath} failed registration earlier"
            ));
        }
        let result =
            self.load_static_glm_uncached(model_qpath, custom_skin, label, preview_fallback);
        match result {
            Ok(model) => {
                let model = Arc::new(model);
                self.saber_models.insert(key, Arc::clone(&model));
                Ok(model)
            }
            Err(error) => {
                self.failed_saber_models.insert(key);
                Err(error)
            }
        }
    }

    pub(in crate::cgame::player_presenter) fn load_static_glm_uncached(
        &mut self,
        model_qpath: &str,
        custom_skin: Option<&str>,
        label: &str,
        preview_fallback: bool,
    ) -> Result<SaberModelAsset, String> {
        build_static_glm(
            &mut SyncModelSource { presenter: self },
            model_qpath,
            custom_skin,
            label,
            preview_fallback,
        )
    }

    /// OpenJK's `RE_RegisterModels_GetDiskFile` treats `*default.gla` as a
    /// renderer-internal synthetic GLA rather than a VFS path.  Saber/weapon
    /// GLMs commonly reference it, so do the same before touching the PK3 VFS.
    pub(in crate::cgame::player_presenter) fn load_glm_animation(
        &mut self,
        gla_qpath: &str,
    ) -> Result<Arc<GlaAnimation>, String> {
        if gla_qpath.eq_ignore_ascii_case(OPENJK_DEFAULT_GLA_NAME) {
            return Ok(Arc::new(openjk_default_gla()?));
        }
        let gla_bytes = self
            .assets
            .read(gla_qpath, MAX_GLA_BYTES)
            .map_err(|error| format!("{gla_qpath}: {error}"))?
            .ok_or_else(|| format!("missing {gla_qpath}"))?
            .bytes;
        Ok(Arc::new(parse_gla(&gla_bytes)?))
    }

    pub(in crate::cgame::player_presenter) fn load_skin_qpath(
        &mut self,
        qpath: &str,
    ) -> Result<Vec<jka_assets::skin::SkinSurface>, String> {
        load_skin(qpath, |name| {
            self.assets
                .read(name, MAX_SKIN_BYTES)
                .map_err(|error| error.to_string())
                .map(|asset| asset.map(|asset| asset.bytes))
        })
    }

    pub(in crate::cgame::player_presenter) fn resolve_surface_material(
        &mut self,
        shader_name: &str,
    ) -> ResolvedMaterial {
        let shaders = Arc::clone(&self.shaders);
        // Own the resolved shader name before taking mutable borrows of the
        // presenter to load textures. A borrowed Cow here would keep an
        // immutable borrow of self.shader_remaps alive through MaterialLayers.
        let effective = self.shader_remaps.effective_name(shader_name).into_owned();
        let shader_time_offset = self.shader_remaps.time_offset_for(&effective);
        let shader_clamp_time = jka_assets::shader::find_shader(&shaders, &effective).and_then(|shader| shader.clamp_time);
        let layers = material_layers(&shaders, &effective);
        let (mut texture, mut alpha_mode) = self.load_stage_texture(&layers.base);
        let overlay = layers.overlay.as_ref().map(|stage| {
            let (texture, alpha_mode) = self.load_stage_texture(stage);
            SurfaceOverlay {
                texture,
                alpha_mode,
                authored_glow: stage.authored_glow,
            }
        });
        let stages = layers
            .stages
            .iter()
            .map(|&(stage, mode)| {
                let (texture, _) = self.load_stage_texture(&StageMaterial {
                    image: stage.image.as_str(),
                    clamp: stage.clamp,
                    alpha_mode: mode,
                    authored_glow: stage.glow,
                });
                resolved_stage(stage, mode, texture)
            })
            .collect::<Vec<_>>();
        if let Some(first) = stages.first() {
            texture = first.texture.clone();
            alpha_mode = first.alpha_mode;
        }
        ResolvedMaterial {
            texture,
            alpha_mode,
            entity_tint: layers.entity_tint,
            overlay,
            stages,
            two_sided: layers.two_sided,
            authored_glow: layers.base.authored_glow,
            shader_time_offset,
            shader_clamp_time,
        }
    }

    pub(in crate::cgame::player_presenter) fn load_stage_texture(
        &mut self,
        stage: &StageMaterial<'_>,
    ) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        if stage.image.eq_ignore_ascii_case("$whiteimage") {
            return (None, stage.alpha_mode);
        }
        let texture = self
            .textures
            .load(&mut self.assets, stage.image, stage.clamp)
            .map(|index| {
                if !self.texture_arcs.contains_key(&index) {
                    let image = Arc::new(self.textures.images[index].clone());
                    self.texture_arcs.insert(index, image);
                }
                Arc::clone(&self.texture_arcs[&index])
            });
        if texture.is_none() {
            if let Some(warning) = self.textures.warnings.last() {
                rverbose!(1, "PLAYER TEXTURE WARNING: {warning}");
            }
        }
        (texture, stage.alpha_mode)
    }
}
