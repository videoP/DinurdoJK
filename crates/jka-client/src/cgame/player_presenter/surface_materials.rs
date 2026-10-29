//! Materials.
use crate::cgame::player_presenter::{
    stage_uv_affine, wave_value, Arc, Bulge, DynamicModelAlphaMode, PlayerPresenter, Shader, Stage,
    TcGen, TextureData,
};

impl PlayerPresenter {
    /// CPU-skinned custom shaders need the same generated coordinates and
    /// static shell bulge as the GPU vertex stage, independently of skin UVs.
    pub(super) fn custom_shell_cpu_geometry(&self, shell: &mut super::DynamicModelSurface,
        base: &super::DynamicModelSurface, uv: [f32; 6], environment: bool, bulge: f32) {
        if shell.ghoul2_gpu.is_some() { return; }
        let eye = self.stage_view_position.unwrap_or([0.0; 3]);
        let color = shell.vertices.first().map_or([1.0; 4], |v| v.color);
        shell.vertices = Arc::new(base.vertices.iter().map(|original| {
            let mut vertex = *original;
            let normal = super::Vec3::from_array(vertex.normal).normalize_or_zero();
            for i in 0..3 { vertex.position[i] += normal[i] * bulge; }
            let st = if environment { super::environment_uv(vertex.position, vertex.normal, eye) } else { original.uv };
            vertex.uv = [st[0]*uv[0] + st[1]*uv[2] + uv[4], st[0]*uv[1] + st[1]*uv[3] + uv[5]];
            // Keep the refEntity tint already assigned by the shell clone.
            vertex.color = color;
            vertex
        }).collect());
    }

    /// A refEntity customShader: its first stage's texture with FX blending
    /// semantics (these shaders are unlit, vertex/entity colored).
    pub(in crate::cgame::player_presenter) fn custom_shader_material(
        &mut self,
        shader_name: &str,
    ) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode, bool) {
        let effective = self.shader_remaps.effective_name(shader_name);
        let key = effective.replace('\\', "/").to_ascii_lowercase();
        let stage = jka_assets::shader::find_shader(&self.shaders, effective.as_ref())
            .and_then(Shader::primary)
            .cloned();
        let (image, clamp, blend) = match &stage {
            Some(stage) => (stage.image.clone(), stage.clamp, stage.blend.clone()),
            None => (key.clone(), false, String::new()),
        };
        let texture = if image.eq_ignore_ascii_case("$whiteimage") {
            None
        } else {
            self.textures
                .load(&mut self.assets, &image, clamp)
                .map(|index| {
                    if !self.texture_arcs.contains_key(&index) {
                        self.texture_arcs
                            .insert(index, Arc::new(self.textures.images[index].clone()));
                    }
                    Arc::clone(&self.texture_arcs[&index])
                })
        };
        (
            texture,
            crate::fx::draw::FxBlend::from_blend_func(&blend).custom_shader_alpha_mode(),
            stage.is_some_and(|stage| stage.glow),
        )
    }

    /// A refEntity customShader's authored affine `tcMod` chain (e.g. the Rage
    /// shells' `gfx/misc/electric`/`fullbodyelectric2`, which scroll+tile the
    /// lightning texture instead of mapping it 1:1 onto the body's own UVs).
    pub(in crate::cgame::player_presenter) fn custom_shader_uv_xform(
        &self,
        shader_name: &str,
        seconds: f32,
    ) -> [f32; 6] {
        let effective = self.shader_remaps.effective_name(shader_name);
        let shader = jka_assets::shader::find_shader(&self.shaders, effective.as_ref());
        let tc_mods = shader.and_then(Shader::primary)
            .map(|stage| stage.tc_mods.as_slice()).unwrap_or_default();
        let seconds = jka_assets::shader::shader_time(seconds,
            self.shader_remaps.time_offset_for(effective.as_ref()),
            shader.and_then(|shader| shader.clamp_time));
        stage_uv_affine(tc_mods, seconds)
    }

    /// A refEntity customShader's `deformVertexes bulge` offset (e.g. the Rage
    /// shells, which JKA's `RB_CalcBulgeVertexes` special-cases into a
    /// constant push outward along the vertex normal rather than the stock id
    /// Tech 3 time/UV-varying sine wave; see `Bulge::is_static`). A
    /// time-varying `deformVertexes wave` (e.g. the team-power shell
    /// `powerups/ysalimarishell`'s `wave 100 sin 0 1 0 1` pulse) is
    /// approximated the same way: one oscillating value pushed uniformly
    /// along every vertex's normal, dropping the real formula's per-vertex
    /// `(x+y+z)/div` phase spread (there is no per-draw slot to carry it
    /// through to the GPU skin, only this single scalar).
    pub(in crate::cgame::player_presenter) fn custom_shader_bulge_height(
        &self,
        shader_name: &str,
        seconds: f32,
    ) -> f32 {
        let effective = self.shader_remaps.effective_name(shader_name);
        let Some(shader) = jka_assets::shader::find_shader(&self.shaders, effective.as_ref()) else {
            return 0.0;
        };
        let seconds = jka_assets::shader::shader_time(seconds,
            self.shader_remaps.time_offset_for(effective.as_ref()), shader.clamp_time);
        if let Some(bulge) = shader.bulge.filter(Bulge::is_static) {
            return bulge.height;
        }
        shader
            .vertex_wave
            .map_or(0.0, |vertex_wave| wave_value(vertex_wave.wave, seconds))
    }

    /// A refEntity customShader's `tcGen environment` (e.g. Absorb's
    /// `gfx/misc/personalshield` chrome stage): reflection-vector UVs instead
    /// of the base mesh's own, same as `RB_CalcEnvironmentTexCoords`.
    pub(in crate::cgame::player_presenter) fn custom_shader_env_map(
        &self,
        shader_name: &str,
    ) -> bool {
        let effective = self.shader_remaps.effective_name(shader_name);
        jka_assets::shader::find_shader(&self.shaders, effective.as_ref())
            .and_then(Shader::primary)
            .is_some_and(|stage| stage.tc_gen == TcGen::Environment)
    }

    /// Additive stages of a refEntity customShader beyond its primary (e.g.
    /// the team-power shell's second `glow` + `tcMod turb` pass, which gives
    /// it its swirling sheen on top of the base tint). `custom_shader_material`
    /// only reads stage 0, mirroring OpenJK's common single-xstage shells;
    /// this covers the few that layer a second additive pass on top.
    /// Environment coordinates are generated independently for each stage;
    /// vector coordinates and alpha-tested overlays are still skipped. `force_alpha_blend`
    /// carries the base layer's `RF_FORCE_ENT_ALPHA` override so a follow-on
    /// stage blends the same way instead of fighting it with its own additive
    /// `blendFunc`.
    pub(in crate::cgame::player_presenter) fn custom_shader_overlay_stages(
        &mut self,
        shader_name: &str,
        seconds: f32,
        force_alpha_blend: bool,
    ) -> Vec<(Option<Arc<TextureData>>, DynamicModelAlphaMode, [f32; 6], bool, bool)> {
        use crate::fx::draw::FxBlend;
        let effective = self.shader_remaps.effective_name(shader_name);
        let Some(shader) = jka_assets::shader::find_shader(&self.shaders, effective.as_ref()) else {
            return Vec::new();
        };
        let seconds = jka_assets::shader::shader_time(seconds,
            self.shader_remaps.time_offset_for(effective.as_ref()), shader.clamp_time);
        let Some(primary) = shader.primary() else {
            return Vec::new();
        };
        let overlays: Vec<Stage> = shader
            .stages
            .iter()
            .filter(|stage| !std::ptr::eq(*stage, primary))
            .filter(|stage| {
                !stage.image.is_empty()
                    && !stage.image.starts_with('$')
                    && stage.alpha_test.trim().is_empty()
                    && matches!(stage.tc_gen, TcGen::Base | TcGen::Environment)
                    && matches!(
                        FxBlend::from_blend_func(&stage.blend),
                        FxBlend::Add | FxBlend::AddAlpha
                    )
            })
            .cloned()
            .collect();
        overlays
            .into_iter()
            .map(|stage| {
                let texture = if stage.image.eq_ignore_ascii_case("$whiteimage") {
                    None
                } else {
                    self.textures
                        .load(&mut self.assets, &stage.image, stage.clamp)
                        .map(|index| {
                            if !self.texture_arcs.contains_key(&index) {
                                self.texture_arcs
                                    .insert(index, Arc::new(self.textures.images[index].clone()));
                            }
                            Arc::clone(&self.texture_arcs[&index])
                        })
                };
                let alpha_mode = if force_alpha_blend {
                    DynamicModelAlphaMode::BlendUnlit
                } else {
                    FxBlend::from_blend_func(&stage.blend).custom_shader_alpha_mode()
                };
                let uv_xform = stage_uv_affine(&stage.tc_mods, seconds);
                (texture, alpha_mode, uv_xform, stage.glow, stage.tc_gen == TcGen::Environment)
            })
            .collect()
    }
}
