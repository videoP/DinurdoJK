//! Assets loading.
use crate::cgame::player_presenter::{
    asset_jobs, build_lod_gpu_meshes, load_skin, materials, openjk_default_gla, parse_gla,
    parse_glm, player_model_key, static_model_key, AlphaGen, Arc, AssetPriority, AssetSearchPath,
    AssetSource, AssetState, BTreeMap, DynamicModelAlphaMode, GlaAnimation, HashMap, JiggleProfile,
    Mutex, OnceLock, PlayerModelAsset, PlayerPresenter, PlayerSurfaceAsset, Requested,
    ResolvedMaterial, ResolvedStage, RgbGen, SaberDefinition, SaberModelAsset, Shader,
    SurfaceOverlay, TcGen, TcMod, TextureData, Textures, Vec3, Wave, WaveFunc, ASSET_PENDING,
    MAX_GLA_BYTES, MAX_GLM_BYTES, MAX_JIGGLE_BYTES, MAX_SKIN_BYTES, OPENJK_DEFAULT_GLA_NAME,
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
    pub(in crate::cgame::player_presenter) glas:
        Mutex<HashMap<String, Arc<OnceLock<Result<Arc<GlaAnimation>, String>>>>>,
    pub(in crate::cgame::player_presenter) textures:
        Mutex<HashMap<(String, bool), Arc<OnceLock<Option<Arc<TextureData>>>>>>,
}

impl ModelLoadShared {
    pub(in crate::cgame::player_presenter) fn new(shaders: Arc<BTreeMap<String, Shader>>) -> Self {
        Self {
            shaders,
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
        let layers = material_layers(&shared.shaders, shader_name);
        let (mut texture, mut alpha_mode) = self.load_stage(&layers.base);
        let overlay = layers.overlay.as_ref().map(|stage| {
            let (texture, alpha_mode) = self.load_stage(stage);
            SurfaceOverlay {
                texture,
                alpha_mode,
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

/// The pass mode of one stage of a blended player shader, or `None` when this
/// path cannot draw it. Not drawn: environment/vector coordinates, alpha tests
/// and `GL_DST_COLOR` filters.
pub(in crate::cgame::player_presenter) fn blended_stage_mode(
    stage: &jka_assets::shader::Stage,
    allow_opaque: bool,
) -> Option<DynamicModelAlphaMode> {
    use crate::fx::draw::FxBlend;
    if stage.image.is_empty()
        || stage.image.starts_with('$')
        || stage.entity_rgb
        || !stage.alpha_test.trim().is_empty()
        || !matches!(stage.tc_gen, TcGen::Base)
    {
        return None;
    }
    match FxBlend::from_blend_func(&stage.blend) {
        FxBlend::Opaque => allow_opaque.then_some(DynamicModelAlphaMode::Opaque),
        FxBlend::Add => Some(DynamicModelAlphaMode::AdditiveOne),
        FxBlend::AddAlpha => Some(DynamicModelAlphaMode::Additive),
        _ => {
            let blend = stage
                .blend
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            (blend == "gl_src_alpha gl_one_minus_src_alpha" || blend == "blend")
                .then_some(DynamicModelAlphaMode::BlendUnlit)
        }
    }
}

/// Stage list for the glow/sparkle shaders of custom player models: a shader
/// whose primary stage is blended (not masked or a filter) and has at least one
/// additive stage, or whose primary stage is a plain opaque one (ShadowTink's
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
    let opaque_primary = match FxBlend::from_blend_func(&primary.blend) {
        FxBlend::Opaque => true,
        FxBlend::Modulate | FxBlend::Modulate2x | FxBlend::Darken => return Vec::new(),
        _ => false,
    };
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
        // replace it, and a lone base stays on the single-stage path.
        let base_drawn = stages
            .first()
            .is_some_and(|&(stage, _)| std::ptr::eq(stage, primary));
        return if base_drawn && stages.len() > 1 {
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
    if additive {
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
        unlit: const_rgb,
        specular_alpha: stage.alpha_gen == AlphaGen::LightingSpecular,
        tc_mods: stage.tc_mods.clone(),
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

/// `uv * xy + zw` for a stage's `tcMod scale`/`scroll` chain at `seconds`
/// (scroll wraps like the engine's, so the offset never loses precision).
pub(in crate::cgame::player_presenter) fn stage_uv_xform(mods: &[TcMod], seconds: f32) -> [f32; 4] {
    let (mut scale, mut offset) = ([1.0_f32; 2], [0.0_f32; 2]);
    for tc_mod in mods {
        match *tc_mod {
            TcMod::Scale(x, y) => {
                scale = [scale[0] * x, scale[1] * y];
                offset = [offset[0] * x, offset[1] * y];
            }
            TcMod::Scroll(x, y) => {
                let (sx, sy) = (x * seconds, y * seconds);
                offset = [offset[0] + sx - sx.floor(), offset[1] + sy - sy.floor()];
            }
            _ => {}
        }
    }
    [scale[0], scale[1], offset[0], offset[1]]
}

pub(in crate::cgame::player_presenter) fn stage_material(
    stage: &jka_assets::shader::Stage,
) -> StageMaterial<'_> {
    let alpha_mode = if !stage.alpha_test.trim().is_empty() {
        DynamicModelAlphaMode::Mask
    } else {
        crate::fx::draw::FxBlend::from_blend_func(&stage.blend).custom_shader_alpha_mode()
    };
    StageMaterial {
        image: stage.image.as_str(),
        clamp: stage.clamp,
        alpha_mode,
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
                })
        })
        .flatten();
    let stages = blended_stages(shader, primary);
    let base = stage_material(primary);
    // Pipelines always cull back faces; only a translucent `cull twosided`
    // surface (wings) shows its back side, so only it pays for the extra triangles.
    let two_sided = matches!(
        shader.cull.to_ascii_lowercase().as_str(),
        "none" | "twosided" | "disable"
    ) && !matches!(
        stages.first().map_or(base.alpha_mode, |&(_, mode)| mode),
        DynamicModelAlphaMode::Opaque
            | DynamicModelAlphaMode::Mask
            | DynamicModelAlphaMode::MaskBlend
    );
    MaterialLayers {
        base,
        entity_tint: primary.entity_rgb,
        overlay,
        stages,
        two_sided,
    }
}

pub(in crate::cgame::player_presenter) fn player_jiggle_qpath(model_qpath: &str) -> String {
    model_qpath.rsplit_once('.').map_or_else(
        || format!("{model_qpath}.jiggle"),
        |(base, _)| format!("{base}.jiggle"),
    )
}

/// OpenJK `CG_RegisterClientModelname`: GLM + GLA + skin -> drawable surfaces.
pub(in crate::cgame::player_presenter) fn build_player_model(
    source: &mut dyn ModelSource,
    info: &crate::cgame::ClientInfo,
    key: String,
) -> Result<PlayerModelAsset, String> {
    let model_qpath = info.model_qpath();
    let model_bytes = source.read_model(&model_qpath)?;
    let glm = Arc::new(parse_glm(&model_bytes)?);

    let gla_qpath = if glm.anim_name.to_ascii_lowercase().ends_with(".gla") {
        glm.anim_name.clone()
    } else {
        format!("{}.gla", glm.anim_name)
    };
    let gla = source.load_gla(&gla_qpath)?;
    // OpenJK mdx_format.h: mdxmHeader_t::numBones exists for an
    // in-game version/safety check to ensure the mesh does not reference
    // MORE bones than its GLA provides. Equality is not required; weapon
    // and saber meshes commonly use only a subset of a shared skeleton.
    if glm.num_bones > gla.skeleton.len() {
        return Err(format!(
            "GLM references {} bones but GLA only provides {}",
            glm.num_bones,
            gla.skeleton.len()
        ));
    }

    let jiggle_qpath = player_jiggle_qpath(&model_qpath);
    let explicit_jiggle = match source.read_optional(&jiggle_qpath, MAX_JIGGLE_BYTES) {
        Ok(Some(bytes)) => match String::from_utf8(bytes)
            .map_err(|error| format!("not UTF-8: {error}"))
            .and_then(|text| JiggleProfile::parse(&text, &glm, &gla))
        {
            Ok(profile) => {
                devprintln!(
                    2,
                    "PLAYER JIGGLE: {jiggle_qpath} override loaded regions={}",
                    profile.region_count(),
                );
                Some(profile)
            }
            Err(error) => {
                rverbose!(
                    1,
                    "PLAYER JIGGLE WARNING: {jiggle_qpath}: {error}; trying _humanoid auto profile"
                );
                None
            }
        },
        Ok(None) => None,
        Err(error) => {
            rverbose!(
                1,
                "PLAYER JIGGLE WARNING: {error}; trying _humanoid auto profile"
            );
            None
        }
    };
    let jiggle = match explicit_jiggle {
        Some(profile) => Some(Arc::new(profile)),
        None => match JiggleProfile::auto_humanoid(&glm, &gla) {
            Ok(Some(profile)) => {
                devprintln!(
                    2,
                    "PLAYER JIGGLE: {model_qpath} auto regions={} (model.jiggle override supported)",
                    profile.region_count(),
                );
                Some(Arc::new(profile))
            }
            Ok(None) => None,
            Err(error) => {
                rverbose!(1, "PLAYER JIGGLE AUTO WARNING: {model_qpath}: {error}");
                None
            }
        },
    };

    let skin_qpath = info.skin_qpath();
    let skin = match source.load_skin(&skin_qpath) {
        Ok(skin) => skin,
        Err(primary_error) => {
            let fallback = info.default_skin_qpath();
            rverbose!(
                1,
                "PLAYER SKIN FALLBACK: {skin_qpath}: {primary_error}; trying {fallback}"
            );
            source.load_skin(&fallback)?
        }
    };
    let skin_map = skin
        .into_iter()
        .map(|surface| (surface.name.to_ascii_lowercase(), surface.shader))
        .collect::<HashMap<_, _>>();

    let lod = glm
        .lods
        .first()
        .ok_or_else(|| format!("{model_qpath} has no GLM LODs"))?;
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
        const G2SURFACEFLAG_OFF: u32 = 0x0000_0002;
        let default_visible = !skin_off && hierarchy.flags & G2SURFACEFLAG_OFF == 0;
        // OpenJK keeps `*off` surfaces in the Ghoul2 instance and only toggles
        // their runtime flags. Keep a drawable material resident for those
        // dormant surfaces so stump/limb caps can be enabled without I/O.
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
            entity_tint: material.entity_tint,
            overlay: material.overlay,
            stages: material.stages,
            fallback_gray: false,
            gpu_meshes: build_lod_gpu_meshes(
                &model_qpath,
                &glm,
                surface.surface_index,
                two_sided,
                jiggle.as_deref(),
            )?,
        });
    }

    devprintln!(
        2,
        "PLAYER MODEL: {} skin={} surfaces={} gla={} bones={}",
        model_qpath,
        skin_qpath,
        surfaces.len(),
        gla_qpath,
        gla.skeleton.len(),
    );

    Ok(PlayerModelAsset {
        key,
        glm,
        gla,
        surfaces,
        jiggle,
    })
}

pub(in crate::cgame::player_presenter) fn build_static_glm(
    source: &mut dyn ModelSource,
    model_qpath: &str,
    custom_skin: Option<&str>,
    label: &str,
    preview_fallback: bool,
) -> Result<SaberModelAsset, String> {
    let model_bytes = source.read_model(model_qpath)?;
    let glm = Arc::new(parse_glm(&model_bytes)?);
    let gla_qpath = if glm.anim_name.to_ascii_lowercase().ends_with(".gla") {
        glm.anim_name.clone()
    } else {
        format!("{}.gla", glm.anim_name)
    };
    let gla = source.load_gla(&gla_qpath)?;
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
            Err(error) if preview_fallback => {
                rverbose!(
                    1,
                    "ASSET VIEWER GLM SKIN FALLBACK: model={} skin={} error={}; using embedded shaders/gray fallback",
                    model_qpath, skin_qpath, error,
                );
                HashMap::new()
            }
            Err(error) => return Err(error),
        }
    } else {
        HashMap::new()
    };
    let lod = glm
        .lods
        .first()
        .ok_or_else(|| format!("{model_qpath} has no GLM LODs"))?;
    let mut surfaces = Vec::with_capacity(lod.surfaces.len());
    for surface in &lod.surfaces {
        let hierarchy = glm.hierarchy.get(surface.surface_index).ok_or_else(|| {
            format!(
                "{model_qpath}: missing hierarchy for surface {}",
                surface.surface_index
            )
        })?;
        // Ghoul2 tag surfaces are attachment metadata, not visible hilt geometry.
        if hierarchy.name.starts_with('*') {
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
            if preview_fallback {
                // Player/NPC GLMs often have no embedded shader refs and
                // normally rely entirely on model_default.skin. If that
                // skin is absent/incomplete, keep the geometry inspectable
                // instead of silently producing zero draw surfaces.
                "$whiteimage"
            } else {
                continue;
            }
        } else {
            shader_name
        };
        let material = source.resolve_material(shader_name);
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
            entity_tint: material.entity_tint,
            overlay: material.overlay,
            stages: material.stages,
            fallback_gray,
            gpu_meshes: build_lod_gpu_meshes(
                &model_qpath,
                &glm,
                surface.surface_index,
                false,
                None,
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
    Ok(SaberModelAsset { glm, gla, surfaces })
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

    pub(in crate::cgame::player_presenter) fn report_player_status(
        &mut self,
        entity_num: u16,
        status: String,
    ) {
        if self.player_diagnostics.get(&entity_num) != Some(&status) {
            devprintln!(2, "PLAYER PRESENTATION: entity={entity_num} {status}");
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
        build_player_model(&mut SyncModelSource { presenter: self }, info, key)
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
        self.load_shared = Arc::new(ModelLoadShared::new(Arc::clone(&self.shaders)));
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
            rverbose!(
                1,
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
        let key = static_model_key(model_qpath, custom_skin, false);
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
        let key = static_model_key(model_qpath, custom_skin, preview_fallback);
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
        let layers = material_layers(&shaders, shader_name);
        let (mut texture, mut alpha_mode) = self.load_stage_texture(&layers.base);
        let overlay = layers.overlay.as_ref().map(|stage| {
            let (texture, alpha_mode) = self.load_stage_texture(stage);
            SurfaceOverlay {
                texture,
                alpha_mode,
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
