//! Non-player CGame entity presentation.
//!
//! This is the Rust ownership boundary corresponding to OpenJK `CG_AddCEntity`.
//! Snapshot decoding and trajectory interpolation stay in `cgame.rs`; this module
//! dispatches already-presented entities into model/effect-specific render paths.
//! The first vertical slice intentionally renders ordinary MD3-backed
//! `ET_GENERAL` entities and non-inline MD3 `ET_MOVER` entities while keeping the
//! remaining entity kinds explicit instead of silently treating them as models.

use super::{
    item_presenter::{cg_item, item_cone_origin, ItemDraw, ItemInput, IT_WEAPON},
    weapon_fx::WeaponFx,
    player_presenter::{blend_for_alpha, Ghoul2PresentationView, PlayerPresenter},
    ClientGameState, EntityPresentationKind, PresentedEntity,
};
use crate::{
    materials::{self, TextureData, Textures},
    renderer::{
        DynamicModelAlphaMode, DynamicModelSurface, DynamicWireframeClass, DynamicModelVertex,
        InlineModelInstance, RayTracedRigidInstance,
    },
    scene,
};
use jka_assets::{
    bsp::StaticModel,
    md3::{self, Model as Md3Model},
    pk3::AssetSearchPath,
    shader::Shader,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

const MAX_MD3_BYTES: usize = 64 * 1024 * 1024;
const EF_NODRAW: i32 = 1 << 8;
const EF2_HYPERSPACE: i32 = 1 << 5;
const SOLID_BMODEL: i32 = 0x00ff_ffff;
const WP_SABER: i32 = 3;

#[derive(Clone)]
struct Md3SurfaceAsset {
    surface_index: usize,
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
}

struct Md3Asset {
    key: Arc<str>,
    model: Arc<Md3Model>,
    surfaces: Vec<Md3SurfaceAsset>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EntityDispatchSummary {
    pub general: usize,
    pub movers: usize,
    pub items: usize,
    pub missiles: usize,
    pub npcs: usize,
    pub fx: usize,
    pub other: usize,
    pub rendered_surfaces: usize,
}

pub struct EntityPresenter {
    assets: AssetSearchPath,
    pbr: bool,
    shaders: BTreeMap<String, Shader>,
    textures: Textures,
    texture_arcs: HashMap<usize, Arc<TextureData>>,
    missing_texture: Arc<TextureData>,
    md3_models: HashMap<String, Arc<Md3Asset>>,
    /// Cosmetic models that failed to load (logged once each).
    missing_cosmetics: std::collections::HashSet<&'static str>,
    failed_models: HashSet<String>,
    logged_unsupported: HashSet<String>,
    fx_materials: HashMap<String, Vec<crate::fx::draw::FxMaterial>>,
    /// True only while the renderer is actually using hardware RT shadows.
    /// Keep RT source metadata completely off the ordinary raster presentation path.
    rt_rigid_casters_enabled: bool,
    /// r_drawMapModels: server-placed MD3 props (ET_GENERAL / non-inline
    /// ET_MOVER). Inline BSP brush movers are never affected.
    draw_map_models: bool,
    /// jaPRO `cgs.miscStaticModels` source: the map's `misc_model_static`
    /// entities. They are client-only (the game module frees them), so they
    /// come from the BSP entity string rather than from snapshots.
    static_model_source: Arc<Vec<StaticModel>>,
    /// Surfaces for `static_model_source`, built on first use and rebuilt when
    /// the source changes or the entity assets are refreshed.
    static_props: Option<Vec<StaticProp>>,
    /// Whether `static_props` carry RT caster sources (they must be rebuilt when
    /// hardware RT shadows are toggled).
    static_props_rt: bool,
}

/// One placed `misc_model_static` with its world-space surfaces already built.
struct StaticProp {
    surfaces: Vec<DynamicModelSurface>,
    origin: [f32; 3],
    /// Origin the cull tests use (jaPRO adds 1 + zoffset to z).
    cull_origin: [f32; 3],
    /// `RadiusFromBounds` of the frame-0 bounds scaled by the model scale; 0
    /// when the model has no bounds, which disables the frustum test.
    radius: f32,
}

impl EntityPresenter {
    pub fn new(mut assets: AssetSearchPath, pbr: bool) -> Result<Self, String> {
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut assets, &mut shader_warnings, pbr)?;
        println!(
            "ENTITY ASSETS: shaderDefs={} mtrDefs={}",
            diagnostics.shader_definitions, diagnostics.mtr_definitions,
        );
        for warning in shader_warnings {
            println!("ENTITY MATERIAL WARNING: {warning}");
        }
        Ok(Self {
            assets,
            pbr,
            shaders,
            textures: Textures::new(),
            texture_arcs: HashMap::new(),
            missing_texture: Arc::new(materials::missing_texture_data()),
            md3_models: HashMap::new(),
            missing_cosmetics: std::collections::HashSet::new(),
            failed_models: HashSet::new(),
            logged_unsupported: HashSet::new(),
            fx_materials: HashMap::new(),
            rt_rigid_casters_enabled: false,
            draw_map_models: true,
            static_model_source: Arc::default(),
            static_props: None,
            static_props_rt: false,
        })
    }

    pub fn set_draw_map_models(&mut self, enabled: bool) {
        self.draw_map_models = enabled;
    }

    /// Installs the current map's `misc_model_static` placements.
    pub fn set_static_models(&mut self, models: Arc<Vec<StaticModel>>) {
        if Arc::ptr_eq(&self.static_model_source, &models) {
            return;
        }
        self.static_model_source = models;
        self.static_props = None;
    }

    /// jaPRO `CG_DrawMiscStaticModels`: every static prop that survives the
    /// frustum and distance cull, as already-transformed MD3 surfaces. Without a
    /// view (first frame, teleport) nothing is culled. With hardware RT shadows
    /// the props are architecture that must keep shadowing, so culled ones stay
    /// in the list as shadow-only (`raster_visible = false`); that also keeps the
    /// RT scene signature stable while the camera turns.
    pub fn present_static_models(
        &mut self,
        view: Option<Ghoul2PresentationView>,
    ) -> Vec<DynamicModelSurface> {
        if !self.draw_map_models || self.static_model_source.is_empty() {
            return Vec::new();
        }
        if self.static_props.is_none() || self.static_props_rt != self.rt_rigid_casters_enabled {
            self.static_props_rt = self.rt_rigid_casters_enabled;
            let props = self.build_static_props();
            self.static_props = Some(props);
        }
        let mut draws = Vec::new();
        for prop in self.static_props.iter().flatten() {
            let culled = view.is_some_and(|view| {
                let distance_cull = view.distance_cull();
                (prop.radius > 0.0 && view.sphere_outside(prop.cull_origin, prop.radius))
                    || (distance_cull > 0.0
                        && view.distance_to(prop.origin) - prop.radius > distance_cull)
            });
            if !culled {
                draws.extend(prop.surfaces.iter().cloned());
            } else if self.rt_rigid_casters_enabled {
                draws.extend(prop.surfaces.iter().cloned().map(|mut surface| {
                    surface.raster_visible = false;
                    surface
                }));
            }
        }
        draws
    }

    fn build_static_props(&mut self) -> Vec<StaticProp> {
        let source = Arc::clone(&self.static_model_source);
        let mut props = Vec::with_capacity(source.len());
        let mut surfaces_total = 0usize;
        for placement in source.iter() {
            let mut axis = angles_to_axis(placement.angles);
            for (row, scale) in axis.iter_mut().zip(placement.scale) {
                for component in row.iter_mut() {
                    *component *= scale;
                }
            }
            let submission = ItemDraw {
                model: placement.model.clone(),
                origin: placement.origin,
                axis,
                rgba: [1.0; 4],
                custom_shader: None,
            };
            // rt_rigid is filled by present_md3_ref_entity while RT casters are on.
            let surfaces = match self.present_md3_ref_entity(1023, &submission) {
                Ok(surfaces) => surfaces,
                Err(error) => {
                    if self.logged_unsupported.insert(format!("static:{}", placement.model)) {
                        println!("STATIC MODEL {}: {error}", placement.model);
                    }
                    continue;
                }
            };
            surfaces_total += surfaces.len();
            let radius = self
                .load_md3(&placement.model)
                .ok()
                .and_then(|asset| {
                    asset.model.frames.first().map(|frame| {
                        // RadiusFromBounds over the scaled frame-0 bounds.
                        let corner = |axis: usize| {
                            let scale = placement.scale[axis];
                            (frame.mins[axis] * scale).abs().max((frame.maxs[axis] * scale).abs())
                        };
                        (corner(0).powi(2) + corner(1).powi(2) + corner(2).powi(2)).sqrt()
                    })
                })
                .unwrap_or(0.0);
            let mut cull_origin = placement.origin;
            cull_origin[2] += 1.0 + placement.zoffset;
            props.push(StaticProp { surfaces, origin: placement.origin, cull_origin, radius });
        }
        println!(
            "STATIC MODELS: {} of {} misc_model_static placement(s) built, {} surface(s)",
            props.len(),
            source.len(),
            surfaces_total,
        );
        props
    }

    /// Targeted filesystem refresh for model registrations that previously
    /// failed because content was absent. Already-loaded MD3/model caches stay
    /// resident; only the VFS namespace, shader definition table and failure
    /// sentinels are refreshed.
    pub fn retry_failed_assets(&mut self) -> Result<usize, String> {
        self.assets
            .refresh()
            .map_err(|error| format!("ENTITY ASSET REFRESH ERROR: {error}"))?;
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut self.assets, &mut shader_warnings, self.pbr)?;
        self.shaders = shaders;
        for warning in shader_warnings {
            println!("ENTITY MATERIAL REFRESH WARNING: {warning}");
        }
        println!(
            "ENTITY ASSET REFRESH: shaderDefs={} mtrDefs={}",
            diagnostics.shader_definitions, diagnostics.mtr_definitions
        );
        let retried = self.failed_models.len();
        self.failed_models.clear();
        self.fx_materials.clear();
        self.static_props = None;
        Ok(retried)
    }

    /// `ghoul2` draws Ghoul2 world models (weapon items) through the shared
    /// player/saber Ghoul2 path, so both presenters use one model cache.
    pub fn present_snapshot_entities(
        &mut self,
        entities: &[PresentedEntity],
        game: &ClientGameState,
        time: i32,
        ghoul2: &mut PlayerPresenter,
        weapon_fx: &mut WeaponFx,
        rt_rigid_casters_enabled: bool,
    ) -> (Vec<DynamicModelSurface>, Vec<InlineModelInstance>, EntityDispatchSummary) {
        self.rt_rigid_casters_enabled = rt_rigid_casters_enabled;
        let mut draws = Vec::new();
        let mut inline_models = Vec::new();
        let mut summary = EntityDispatchSummary::default();

        for entity in entities {
            match entity.presentation_kind() {
                EntityPresentationKind::General
                | EntityPresentationKind::Holocron => {
                    summary.general += 1;
                    self.present_configstring_model(entity, game, &mut draws);
                }
                EntityPresentationKind::Body => {
                    // ET_BODY is a Ghoul2 corpse copied from a client entity.
                    // OpenJK sends it through CG_G2Animated -> CG_Player, with
                    // CG_RagDoll immediately before/inside player angles.
                    // PlayerPresenter owns that same Ghoul2 presentation seam.
                    summary.general += 1;
                }
                EntityPresentationKind::Mover => {
                    summary.movers += 1;
                    self.present_mover(entity, game, &mut draws, &mut inline_models);
                }
                EntityPresentationKind::Item => {
                    summary.items += 1;
                    self.present_item(entity, game, time, ghoul2, weapon_fx, &mut draws);
                }
                EntityPresentationKind::Missile => {
                    summary.missiles += 1;
                    // CG_Missile: the trail effect plays inside WeaponFx;
                    // model-carrying missiles also submit their refEntity.
                    if let Some(model) = weapon_fx.missile(entity, game, time) {
                        let submission = ItemDraw {
                            model: model.qpath.to_owned(),
                            origin: model.origin,
                            axis: model.axis,
                            rgba: [1.0; 4],
                            custom_shader: None,
                        };
                        match self.present_md3_ref_entity(entity.number, &submission) {
                            Ok(mut surfaces) => draws.append(&mut surfaces),
                            Err(error) => self.log_entity_once(entity, &format!("missile model {}: {error}", model.qpath)),
                        }
                    }
                }
                EntityPresentationKind::Npc => {
                    // CG_G2Animated -> CG_Player: presented by PlayerPresenter
                    // with the NPC's own clientinfo.
                    summary.npcs += 1;
                }
                EntityPresentationKind::Fx => {
                    summary.fx += 1;
                    // OpenJK CG_FX. Map-authored fx_runner entities are server
                    // entities of type ET_FX; modelindex/modelindex2 carry the
                    // effect resource and off/one-shot/continuous state.
                    weapon_fx.entity_fx(entity, game);
                }
                EntityPresentationKind::Player
                | EntityPresentationKind::Invisible
                | EntityPresentationKind::PushTrigger
                | EntityPresentationKind::TeleportTrigger
                | EntityPresentationKind::Event => {}
                kind => {
                    summary.other += 1;
                    let key = format!("kind:{kind:?}");
                    if self.logged_unsupported.insert(key) {
                        println!("ENTITY DISPATCH TODO: {kind:?}");
                    }
                }
            }
        }

        // CG_AddLocalEntities: LE_FRAGMENT debris from EV_DEBRIS.
        for chunk in weapon_fx.chunk_models() {
            let submission = ItemDraw {
                model: chunk.qpath.clone(),
                origin: chunk.origin,
                axis: chunk.axis,
                rgba: [1.0, 1.0, 1.0, chunk.alpha],
                custom_shader: None,
            };
            match self.present_md3_ref_entity(1023, &submission) {
                Ok(mut surfaces) => draws.append(&mut surfaces),
                Err(error) => {
                    if self.logged_unsupported.insert(format!("chunk:{}", chunk.qpath)) {
                        println!("CHUNK MODEL {}: {error}", chunk.qpath);
                    }
                }
            }
        }

        summary.rendered_surfaces = draws.len();
        (draws, inline_models, summary)
    }

    fn present_configstring_model(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        // Holocrons are gameplay pickups that share this path, not map props.
        if !self.draw_map_models
            && !matches!(entity.presentation_kind(), EntityPresentationKind::Holocron)
        {
            return;
        }
        if entity.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW != 0 {
            return;
        }
        if entity.state.field_i32("modelGhoul2").unwrap_or(0) != 0 {
            // OpenJK does not present an in-flight player saber through the
            // generic ET_GENERAL model path. CG_Player owns saberEntityNum and
            // manually submits the saber Ghoul2 instance/blades. The Rust
            // PlayerPresenter mirrors that path, so do not also report it as
            // an unsupported generic Ghoul2 model here.
            if entity.state.field_i32("weapon").unwrap_or(0) == WP_SABER {
                return;
            }
            self.log_entity_once(entity, "Ghoul2 generic model not implemented yet");
            return;
        }
        self.present_model_index(entity, game, entity.state.field_i32("modelindex").unwrap_or(0), draws);
    }

    /// OpenJK CG_Mover. Unlike CG_General it has no EF_NODRAW gate; only the
    /// vehicle hyperspace brush is suppressed (its in-hyperspace draw needs
    /// the predicted vehicle state, which demos of ordinary play never use).
    fn present_mover(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        draws: &mut Vec<DynamicModelSurface>,
        inline_models: &mut Vec<InlineModelInstance>,
    ) {
        if entity.state.field_i32("eFlags2").unwrap_or(0) & EF2_HYPERSPACE != 0 {
            return;
        }
        let model_index = entity.state.field_i32("modelindex").unwrap_or(0);
        if entity.state.field_i32("solid").unwrap_or(0) == SOLID_BMODEL {
            // ent.hModel = cgs.inlineDrawModel[modelindex], placed at
            // lerpOrigin with AnglesToAxis(lerpAngles).
            if let Ok(model) = u32::try_from(model_index) {
                if model > 0 {
                    inline_models.push(InlineModelInstance {
                        model,
                        origin: entity.origin,
                        axis: angles_to_axis(entity.angles),
                    });
                }
            }
        } else if self.draw_map_models {
            self.present_model_index(entity, game, model_index, draws);
        }

        // OpenJK CG_Mover submits modelindex2 as a second model on the same
        // transform.  Supporting it here also exercises CS_MODELS lookup without
        // pretending inline BSP models are ordinary registered model qpaths.
        let model_index2 = entity.state.field_i32("modelindex2").unwrap_or(0);
        if model_index2 > 0 && self.draw_map_models {
            self.present_model_index(entity, game, model_index2, draws);
        }
    }

    /// OpenJK CG_Item: `bg_itemlist[modelindex]` placed by `cg_item`, then
    /// each submission drawn as an MD3 or Ghoul2 refEntity.
    fn present_item(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        time: i32,
        ghoul2: &mut PlayerPresenter,
        weapon_fx: &mut WeaponFx,
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        let index = entity.state.field_i32("modelindex").unwrap_or(0);
        let Some(item) = jka_movement::bg_item(index) else {
            self.log_entity_once(entity, &format!("bad item index {index} (bg_numItems {})", jka_movement::bg_item_count()));
            return;
        };
        if index > 0 && !game.server_registers_item(index as usize) {
            // OpenJK only precaches CS_ITEMS items at level load; report the
            // mismatch once but still present the entity the server sent.
            self.log_entity_once(entity, &format!("{} not in the CS_ITEMS precache", item.classname));
        }
        let weapon_midpoint = if item.item_type == IT_WEAPON
            && !item.world_model.to_ascii_lowercase().ends_with(".glm")
        {
            // R_ModelBounds of an MD3 world model: frame 0 bounds midpoint.
            self.load_md3(&item.world_model)
                .ok()
                .and_then(|asset| asset.model.frames.first().map(|frame| {
                    std::array::from_fn(|i| frame.mins[i] + 0.5 * (frame.maxs[i] - frame.mins[i]))
                }))
                .unwrap_or([0.0; 3])
        } else {
            [0.0; 3]
        };
        let input = ItemInput {
            number: entity.number,
            origin: entity.origin,
            angles: super::entity_vec3(&entity.state, "angles").unwrap_or([0.0; 3]),
            e_flags: entity.state.field_i32("eFlags").unwrap_or(0),
            time,
            misc_time: game.entity_misc_time(entity.number),
            force_side: game
                .current_snapshot()
                .and_then(|snapshot| snapshot.player_state.field_i32("fd.forceSide"))
                .unwrap_or(0),
            weapon_midpoint,
        };
        if let Some(origin) = item_cone_origin(&item, &input) {
            weapon_fx.item_cone(entity.number, origin);
        }
        for submission in cg_item(&item, &input, angles_to_axis) {
            let result = if submission.model.to_ascii_lowercase().ends_with(".glm") {
                ghoul2.present_static_glm(
                    entity.number,
                    &submission.model,
                    submission.origin,
                    submission.axis,
                    submission.rgba,
                    submission.custom_shader,
                    time,
                )
            } else {
                self.present_md3_ref_entity(entity.number, &submission)
            };
            match result {
                Ok(mut surfaces) => draws.append(&mut surfaces),
                Err(error) => self.log_entity_once(entity, &format!("{} ({}): {error}", item.classname, submission.model)),
            }
        }
    }

    /// jaPRO cosmetics: the hat/cape MD3s `PlayerPresenter` bolted to players.
    /// They resolve through this presenter's own VFS like any other model, so
    /// they only appear when the asset search path provides them (`japro-assets.pk3` in base).
    pub fn present_cosmetics(
        &mut self,
        cosmetics: Vec<super::player_presenter::CosmeticDraw>,
    ) -> Vec<DynamicModelSurface> {
        let mut draws = Vec::new();
        for cosmetic in cosmetics {
            let submission = ItemDraw {
                model: cosmetic.model.to_owned(),
                origin: cosmetic.origin,
                axis: cosmetic.axis,
                rgba: cosmetic.rgba,
                custom_shader: cosmetic.custom_shader,
            };
            match self.present_md3_ref_entity(cosmetic.entity_num, &submission) {
                Ok(mut surfaces) => draws.append(&mut surfaces),
                Err(error) => {
                    if self.missing_cosmetics.insert(cosmetic.model) {
                        println!("COSMETIC UNAVAILABLE: {}: {error}", cosmetic.model);
                    }
                }
            }
        }
        draws
    }

    /// An MD3 refEntity with an explicit transform (frame 0), shaderRGBA and
    /// optional customShader.
    fn present_md3_ref_entity(
        &mut self,
        entity_num: u16,
        submission: &ItemDraw,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let asset = self.load_md3(&submission.model)?;
        let custom = submission.custom_shader.map(|shader| {
            let material = self.fx_material(shader);
            (material.texture, material.blend.custom_shader_alpha_mode())
        });
        let mut draws = Vec::with_capacity(asset.surfaces.len());
        for surface_asset in &asset.surfaces {
            let Some(surface) = asset.model.surfaces.get(surface_asset.surface_index) else { continue };
            let Some(vertices) = surface.frame_vertices(0) else { continue };
            if vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let vertices = vertices
                .iter()
                .map(|vertex| DynamicModelVertex {
                    position: transform_model_point(vertex.position, submission.axis, submission.origin, 1.0),
                    normal: transform_model_normal(vertex.normal, submission.axis),
                    uv: vertex.uv,
                    color: submission.rgba,
                })
                .collect::<Vec<_>>();
            let mut indices = surface.indices.clone();
            for triangle in indices.chunks_exact_mut(3) {
                triangle.swap(1, 2);
            }
            let (texture, alpha_mode) = custom
                .clone()
                .unwrap_or_else(|| (surface_asset.texture.clone(), surface_asset.alpha_mode));
            draws.push(DynamicModelSurface {
                entity_num,
                wireframe_class: DynamicWireframeClass::Entity,
                raster_visible: true,
                vertices: Arc::new(vertices),
                indices: Arc::new(indices),
                lighting_origin: Some(submission.origin),
                rt_rigid: self.rt_rigid_casters_enabled.then(|| RayTracedRigidInstance::md3(
                    Arc::clone(&asset.key),
                    Arc::clone(&asset.model),
                    surface_asset.surface_index,
                    0,
                    submission.axis,
                    submission.origin,
                    1.0,
                )),
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture,
                alpha_mode: blend_for_alpha(alpha_mode, submission.rgba[3]),
            });
        }
        Ok(draws)
    }

    fn present_model_index(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        model_index: i32,
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        let Some(qpath) = game.model_qpath(model_index) else {
            if model_index != 0 {
                self.log_entity_once(entity, &format!("unresolved CS_MODELS index {model_index}"));
            }
            return;
        };
        if !qpath.to_ascii_lowercase().ends_with(".md3") {
            self.log_entity_once(entity, &format!("non-MD3 registered model {qpath}"));
            return;
        }

        match self.present_md3(entity, &qpath) {
            Ok(mut model_draws) => draws.append(&mut model_draws),
            Err(error) => self.log_entity_once(entity, &format!("{qpath}: {error}")),
        }
    }

    fn present_md3(
        &mut self,
        entity: &PresentedEntity,
        qpath: &str,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let asset = self.load_md3(qpath)?;
        let frame = entity.state.field_i32("frame").unwrap_or(0).max(0) as usize;
        let frame = frame.min(asset.model.frames.len().saturating_sub(1));
        let axis = angles_to_axis(entity.angles);
        let scale_int = entity.state.field_i32("iModelScale").unwrap_or(0);
        let scale = if scale_int == 0 { 1.0 } else { scale_int as f32 / 100.0 };
        let color = entity_color(entity);

        let mut draws = Vec::with_capacity(asset.surfaces.len());
        for surface_asset in &asset.surfaces {
            let Some(surface) = asset.model.surfaces.get(surface_asset.surface_index) else {
                continue;
            };
            let Some(vertices) = surface.frame_vertices(frame) else {
                continue;
            };
            if vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let vertices = vertices
                .iter()
                .map(|vertex| DynamicModelVertex {
                    position: transform_model_point(vertex.position, axis, entity.origin, scale),
                    normal: transform_model_normal(vertex.normal, axis),
                    uv: vertex.uv,
                    color,
                })
                .collect::<Vec<_>>();
            let mut indices = surface.indices.clone();
            // MD3/GLM data is authored for the legacy OpenGL front-face convention.
            // The dynamic WGPU pipeline uses CCW, matching the existing player path.
            for triangle in indices.chunks_exact_mut(3) {
                triangle.swap(1, 2);
            }
            draws.push(DynamicModelSurface {
                entity_num: entity.number,
                wireframe_class: DynamicWireframeClass::Entity,
                raster_visible: true,
                vertices: Arc::new(vertices),
                indices: Arc::new(indices),
                lighting_origin: Some(entity.origin),
                rt_rigid: self.rt_rigid_casters_enabled.then(|| RayTracedRigidInstance::md3(
                    Arc::clone(&asset.key),
                    Arc::clone(&asset.model),
                    surface_asset.surface_index,
                    frame,
                    axis,
                    entity.origin,
                    scale,
                )),
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture: surface_asset.texture.clone(),
                alpha_mode: if color[3] < 1.0 {
                    match surface_asset.alpha_mode {
                        DynamicModelAlphaMode::Opaque => DynamicModelAlphaMode::Blend,
                        DynamicModelAlphaMode::Mask => DynamicModelAlphaMode::MaskBlend,
                        mode => mode,
                    }
                } else {
                    surface_asset.alpha_mode
                },
            });
        }
        Ok(draws)
    }

    /// Developer Asset Viewer path: submit an arbitrary MD3 through the same
    /// material/texture registration used by live CGame entities, without
    /// fabricating a protocol entity just to inspect a model.
    pub fn present_static_md3(
        &mut self,
        entity_num: u16,
        qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        frame: usize,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let asset = self.load_md3(qpath)?;
        let frame = frame.min(asset.model.frames.len().saturating_sub(1));
        let mut draws = Vec::with_capacity(asset.surfaces.len());
        for surface_asset in &asset.surfaces {
            let Some(surface) = asset.model.surfaces.get(surface_asset.surface_index) else {
                continue;
            };
            let Some(vertices) = surface.frame_vertices(frame) else {
                continue;
            };
            if vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let vertices = vertices
                .iter()
                .map(|vertex| DynamicModelVertex {
                    position: transform_model_point(vertex.position, axis, origin, 1.0),
                    normal: transform_model_normal(vertex.normal, axis),
                    uv: vertex.uv,
                    color: rgba,
                })
                .collect::<Vec<_>>();
            let mut indices = surface.indices.clone();
            for triangle in indices.chunks_exact_mut(3) {
                triangle.swap(1, 2);
            }
            draws.push(DynamicModelSurface {
                entity_num,
                wireframe_class: DynamicWireframeClass::Entity,
                raster_visible: true,
                vertices: Arc::new(vertices),
                indices: Arc::new(indices),
                lighting_origin: Some(origin),
                rt_rigid: self.rt_rigid_casters_enabled.then(|| RayTracedRigidInstance::md3(
                    Arc::clone(&asset.key),
                    Arc::clone(&asset.model),
                    surface_asset.surface_index,
                    frame,
                    axis,
                    origin,
                    1.0,
                )),
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture: surface_asset.texture.clone(),
                alpha_mode: if rgba[3] < 1.0 {
                    match surface_asset.alpha_mode {
                        DynamicModelAlphaMode::Opaque => DynamicModelAlphaMode::Blend,
                        DynamicModelAlphaMode::Mask => DynamicModelAlphaMode::MaskBlend,
                        mode => mode,
                    }
                } else {
                    surface_asset.alpha_mode
                },
            });
        }
        Ok(draws)
    }

    fn load_md3(&mut self, qpath: &str) -> Result<Arc<Md3Asset>, String> {
        let key = qpath.replace('\\', "/").to_ascii_lowercase();
        if let Some(model) = self.md3_models.get(&key) {
            return Ok(Arc::clone(model));
        }
        if self.failed_models.contains(&key) {
            return Err("model registration failed earlier".into());
        }

        let result = (|| {
            let bytes = self
                .assets
                .read(&key, MAX_MD3_BYTES)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("missing {key}"))?
                .bytes;
            let model = Arc::new(md3::parse(&bytes)?);
            let mut surfaces = Vec::with_capacity(model.surfaces.len());
            for (surface_index, surface) in model.surfaces.iter().enumerate() {
                let (texture, alpha_mode) = self.resolve_surface_material(&surface.shader);
                surfaces.push(Md3SurfaceAsset {
                    surface_index,
                    texture,
                    alpha_mode,
                });
            }
            println!(
                "ENTITY MD3: {} frames={} tags/frame={} surfaces={}",
                key,
                model.frames.len(),
                model.tags.first().map_or(0, Vec::len),
                model.surfaces.len(),
            );
            Ok::<_, String>(Md3Asset { key: Arc::from(key.as_str()), model, surfaces })
        })();

        match result {
            Ok(model) => {
                let model = Arc::new(model);
                self.md3_models.insert(key, Arc::clone(&model));
                Ok(model)
            }
            Err(error) => {
                self.failed_models.insert(key);
                Err(error)
            }
        }
    }

    /// First renderable FX shader stage, retained for model customShader users.
    pub fn fx_material(&mut self, shader_name: &str) -> crate::fx::draw::FxMaterial {
        self.fx_material_stages(shader_name)
            .into_iter()
            .next()
            .expect("FX material resolver always returns at least one stage")
    }

    /// Resolve an FX material only when the authored shader or implicit image
    /// actually exists. This is useful for optional compatibility assets where
    /// the caller has a known stock fallback and should not draw the missing
    /// texture checker for the preferred asset.
    pub fn fx_material_stages_if_present(
        &mut self,
        shader_name: &str,
    ) -> Option<Vec<crate::fx::draw::FxMaterial>> {
        use crate::fx::draw::{FxBlend, FxMaterial};
        let key = shader_name.replace('\\', "/").to_ascii_lowercase();
        if let Some(materials) = self.fx_materials.get(&key) {
            return Some(materials.clone());
        }
        if self.shaders.contains_key(&key) {
            return Some(self.fx_material_stages(shader_name));
        }
        let texture = self.load_texture_arc(&key, false)?;
        let materials = vec![FxMaterial {
            texture: Some(texture),
            blend: FxBlend::Alpha,
            rgb_vertex: true,
            alpha_vertex: true,
            rgb_const: [1.0; 3],
            alpha_const: 1.0,
        }];
        self.fx_materials.insert(key, materials.clone());
        Some(materials)
    }

    /// Every renderable stage of an FX shader. Jedi Academy effects are often
    /// multi-pass; notably `saberBlur` draws blurglow then blurcore, both with
    /// GL_ONE GL_ONE. Keeping this generic fixes other multi-stage FX too.
    pub fn fx_material_stages(&mut self, shader_name: &str) -> Vec<crate::fx::draw::FxMaterial> {
        use crate::fx::draw::{FxBlend, FxMaterial};
        use jka_assets::shader::{AlphaGen, RgbGen};
        let key = shader_name.replace('\\', "/").to_ascii_lowercase();
        // Engine-built material: untextured vertex-colour additive (enhanced
        // saber melt glow). Not an authored shader, so it never hits the loaders.
        if key == "$melt_glow" {
            return vec![FxMaterial {
                texture: None,
                blend: FxBlend::Add,
                rgb_vertex: true,
                alpha_vertex: false,
                rgb_const: [1.0; 3],
                alpha_const: 1.0,
            }];
        }
        if let Some(materials) = self.fx_materials.get(&key) {
            return materials.clone();
        }

        let has_shader_definition = self.shaders.contains_key(&key);
        let stages = self.shaders.get(&key).map(|shader| {
            shader.stages.iter().filter(|stage| {
                !stage.image.is_empty()
                    && (!stage.image.starts_with('$') || stage.image == "$whiteimage" || stage.image == "$lightmap")
            }).cloned().collect::<Vec<_>>()
        }).unwrap_or_default();
        let mut materials = Vec::with_capacity(stages.len().max(1));
        for stage in stages {
            // None is reserved for intentional built-ins such as $whiteimage.
            // Real image qpaths that fail lookup use the engine-wide missing
            // material checker instead of silently sampling white.
            let texture = if stage.image.eq_ignore_ascii_case("$whiteimage") || stage.image.eq_ignore_ascii_case("$lightmap") {
                None
            } else {
                Some(self.load_texture_arc_or_missing(&stage.image, stage.clamp, "FX"))
            };
            materials.push(FxMaterial {
                texture,
                blend: FxBlend::from_blend_func(&stage.blend),
                rgb_vertex: stage.rgb_gen.uses_vertex_color(),
                alpha_vertex: matches!(stage.alpha_gen, AlphaGen::Vertex | AlphaGen::OneMinusVertex),
                rgb_const: if stage.rgb_gen == RgbGen::Const { stage.color.unwrap_or([1.0; 3]) } else { [1.0; 3] },
                alpha_const: stage.alpha.unwrap_or(1.0),
            });
        }
        if materials.is_empty() {
            if has_shader_definition {
                // An authored shader that produced no renderable stage in this
                // path is genuinely unresolved here: keep the engine-wide
                // magenta/checker convention instead of silently inventing a
                // material.
                println!(
                    "FX MATERIAL WARNING: {shader_name}: shader has no renderable image stage; using shared missing texture placeholder"
                );
                materials.push(FxMaterial {
                    texture: Some(Arc::clone(&self.missing_texture)),
                    blend: FxBlend::Opaque,
                    rgb_vertex: true,
                    alpha_vertex: true,
                    rgb_const: [1.0; 3],
                    alpha_const: 1.0,
                });
            } else {
                // Faithful RE_RegisterShader behavior: an explicit .shader
                // definition is optional. If an image with the requested qpath
                // exists, OpenJK builds an implicit LIGHTMAP_2D shader for it
                // (CGEN_VERTEX/AGEN_VERTEX, SRC_ALPHA/ONE_MINUS_SRC_ALPHA).
                // This is exactly how stock image-only FX such as rivet marks
                // remain transparent/dark instead of becoming opaque gray.
                match self.load_texture_arc(&key, false) {
                    Some(texture) => materials.push(FxMaterial {
                        texture: Some(texture),
                        blend: FxBlend::Alpha,
                        rgb_vertex: true,
                        alpha_vertex: true,
                        rgb_const: [1.0; 3],
                        alpha_const: 1.0,
                    }),
                    None => {
                        if let Some(texture) = self.rgb_saber_fallback_texture(&key) {
                            materials.push(FxMaterial {
                                texture: Some(texture),
                                blend: FxBlend::Add,
                                rgb_vertex: true,
                                alpha_vertex: false,
                                rgb_const: [1.0; 3],
                                alpha_const: 1.0,
                            });
                        } else {
                            if let Some(warning) = self.textures.warnings.last() {
                                println!("FX TEXTURE WARNING: {warning}");
                            }
                            println!(
                                "FX MATERIAL WARNING: {shader_name}: no shader definition or implicit image; using shared missing texture placeholder"
                            );
                            materials.push(FxMaterial {
                                texture: Some(Arc::clone(&self.missing_texture)),
                                blend: FxBlend::Opaque,
                                rgb_vertex: true,
                                alpha_vertex: true,
                                rgb_const: [1.0; 3],
                                alpha_const: 1.0,
                            });
                        }
                    }
                }
            }
        }
        self.fx_materials.insert(key, materials.clone());
        materials
    }

    fn load_texture_arc(&mut self, image: &str, clamp: bool) -> Option<Arc<TextureData>> {
        let index = self.textures.load(&mut self.assets, image, clamp)?;
        self.texture_arcs
            .entry(index)
            .or_insert_with(|| Arc::new(self.textures.images[index].clone()));
        Some(Arc::clone(&self.texture_arcs[&index]))
    }

    /// jaPRO's RGB saber glow/core are white images tinted per draw by vertex
    /// colour. Without its assets, derive equivalents from the stock saber images
    /// by keeping each texel's brightest channel as a grey level, so the tint
    /// alone decides the hue.
    fn rgb_saber_fallback_texture(&mut self, key: &str) -> Option<Arc<TextureData>> {
        let source = match key {
            "gfx/effects/sabers/rgbglow1" => "gfx/effects/sabers/yellow_glow2",
            "gfx/effects/sabers/rgbcore1" => "gfx/effects/sabers/blue_line",
            _ => return None,
        };
        let base = self.load_texture_arc(source, false)?;
        let mut texture = (*base).clone();
        texture.label = key.to_owned();
        texture.rgba16f = None;
        for texel in texture.rgba.chunks_exact_mut(4) {
            let grey = texel[0].max(texel[1]).max(texel[2]);
            texel[..3].fill(grey);
        }
        Some(Arc::new(texture))
    }

    fn load_texture_arc_or_missing(
        &mut self,
        image: &str,
        clamp: bool,
        log_prefix: &str,
    ) -> Arc<TextureData> {
        match self.load_texture_arc(image, clamp) {
            Some(texture) => texture,
            None => {
                if let Some(warning) = self.textures.warnings.last() {
                    println!("{log_prefix} TEXTURE WARNING: {warning}");
                }
                Arc::clone(&self.missing_texture)
            }
        }
    }

    fn resolve_surface_material(
        &mut self,
        shader_name: &str,
    ) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        let (image_name, clamp, alpha_mode) = jka_assets::shader::find_shader(&self.shaders, shader_name)
            .and_then(Shader::primary)
            .map(|stage| {
                let alpha_mode = if !stage.alpha_test.trim().is_empty() {
                    DynamicModelAlphaMode::Mask
                } else {
                    crate::fx::draw::FxBlend::from_blend_func(&stage.blend)
                        .custom_shader_alpha_mode()
                };
                (stage.image.clone(), stage.clamp, alpha_mode)
            })
            .unwrap_or((shader_name.to_owned(), false, DynamicModelAlphaMode::Opaque));

        if image_name.eq_ignore_ascii_case("$whiteimage") {
            return (None, alpha_mode);
        }
        let texture = Some(self.load_texture_arc_or_missing(&image_name, clamp, "ENTITY"));
        (texture, alpha_mode)
    }

    fn log_entity_once(&mut self, entity: &PresentedEntity, reason: &str) {
        let key = format!("{}:{}:{reason}", entity.number, entity.entity_type);
        if self.logged_unsupported.insert(key) {
            println!(
                "ENTITY PRESENTATION TODO: entity={} type={} modelindex={} {reason}",
                entity.number,
                entity.entity_type,
                entity.state.field_i32("modelindex").unwrap_or(0),
            );
        }
    }
}

fn entity_color(entity: &PresentedEntity) -> [f32; 4] {
    let mut rgba = [0i32; 4];
    let mut any = false;
    for (index, channel) in rgba.iter_mut().enumerate() {
        *channel = entity
            .state
            .field_i32(&format!("customRGBA[{index}]"))
            .unwrap_or(0);
        any |= *channel != 0;
    }
    if !any {
        return [1.0; 4];
    }
    rgba.map(|channel| channel.clamp(0, 255) as f32 / 255.0)
}

fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [
        -sr * sp * cy + cr * sy,
        -sr * sp * sy - cr * cy,
        -sr * cp,
    ];
    let up = [
        cr * sp * cy + sr * sy,
        cr * sp * sy - sr * cy,
        cr * cp,
    ];
    [forward, [-right[0], -right[1], -right[2]], up]
}

fn transform_model_point(
    point: [f32; 3],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    scale: f32,
) -> [f32; 3] {
    let point = [point[0] * scale, point[1] * scale, point[2] * scale];
    let world = [
        origin[0] + axis[0][0] * point[0] + axis[1][0] * point[1] + axis[2][0] * point[2],
        origin[1] + axis[0][1] * point[0] + axis[1][1] * point[1] + axis[2][1] * point[2],
        origin[2] + axis[0][2] * point[0] + axis[1][2] * point[1] + axis[2][2] * point[2],
    ];
    scene::render_position(world)
}

fn transform_model_normal(normal: [f32; 3], axis: [[f32; 3]; 3]) -> [f32; 3] {
    let jka = [
        axis[0][0] * normal[0] + axis[1][0] * normal[1] + axis[2][0] * normal[2],
        axis[0][1] * normal[0] + axis[1][1] * normal[1] + axis[2][1] * normal[2],
        axis[0][2] * normal[0] + axis[1][2] * normal[1] + axis[2][2] * normal[2],
    ];
    let render = scene::render_position(jka);
    let length = (render[0] * render[0] + render[1] * render[1] + render[2] * render[2]).sqrt();
    if length > 0.0 {
        [render[0] / length, render[1] / length, render[2] / length]
    } else {
        [0.0, 1.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Developer profile of what a demo asks CGame to present. Prints the
    /// entity-type histogram, per-type resource keys and event counts.
    #[test]
    #[ignore = "diagnostic: requires JKA_TEST_BASE; DEMO selects demos/<name>.dm_26 (default TEST)"]
    fn demo_entity_profile() {
        use jka_protocol::{demo::DemoReader, entity_event::{EntityEvent, EV_EVENT_BITS}, server::{Decoder, Event}};
        use std::collections::BTreeMap;
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let demo = std::env::var("DEMO").unwrap_or_else(|_| "TEST".into());
        let bytes = assets.read(&format!("demos/{demo}.dm_26"), 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut types: BTreeMap<i32, usize> = BTreeMap::new();
        let mut keys: BTreeMap<String, usize> = BTreeMap::new();
        let mut events: BTreeMap<i32, usize> = BTreeMap::new();
        let mut snapshots = 0;
        let mut last_message = -1;
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                        println!("PROFILE map={:?}", decoder.map_name());
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshot.message_num == last_message { continue; }
                        last_message = snapshot.message_num;
                        snapshots += 1;
                        for state in &snapshot.entities {
                            let et = state.field_i32("eType").unwrap_or(-1);
                            *types.entry(et).or_insert(0) += 1;
                            if et >= super::super::ET_EVENTS {
                                *events.entry(et - super::super::ET_EVENTS).or_insert(0) += 1;
                                continue;
                            }
                            let ev = state.field_i32("event").unwrap_or(0) & !EV_EVENT_BITS;
                            if ev != EntityEvent::EV_NONE.as_i32() { *events.entry(ev).or_insert(0) += 1; }
                            let mi = state.field_i32("modelindex").unwrap_or(0);
                            let key = match et {
                                2 => format!("ITEM modelindex={mi}"),
                                3 => format!("MISSILE weapon={} modelindex={mi} model={:?} eFlags={:#x}",
                                    state.field_i32("weapon").unwrap_or(0), game.model_qpath(mi),
                                    state.field_i32("eFlags").unwrap_or(0)),
                                13 => format!("NPC modelindex={mi} model={:?} weapon={} NPC_class={} g2radius={} modelScale={:?}",
                                    game.model_qpath(mi), state.field_i32("weapon").unwrap_or(0),
                                    state.field_i32("NPC_class").unwrap_or(-1), state.field_i32("g2radius").unwrap_or(0),
                                    state.field_i32("iModelScale")),
                                0 | 5 | 15 | 17 => format!("eType={et} modelindex={mi} model={:?} g2={} weapon={} eFlags={:#x} owner={} g2radius={} modelindex2={}",
                                    if et == 17 { game.effect_qpath(mi) } else { game.model_qpath(mi) },
                                    state.field_i32("modelGhoul2").unwrap_or(0), state.field_i32("weapon").unwrap_or(0),
                                    state.field_i32("eFlags").unwrap_or(0), state.field_i32("owner").unwrap_or(-1),
                                    state.field_i32("g2radius").unwrap_or(0), state.field_i32("modelindex2").unwrap_or(0)),
                                _ => continue,
                            };
                            *keys.entry(key).or_insert(0) += 1;
                        }
                        let pev = snapshot.player_state.field_i32("externalEvent").unwrap_or(0) & !EV_EVENT_BITS;
                        if pev != EntityEvent::EV_NONE.as_i32() { *events.entry(pev).or_insert(0) += 1; }
                    }
                    _ => {}
                }
            }
        }
        println!("PROFILE snapshots={snapshots} types={types:?}");
        for (key, count) in &keys { println!("PROFILE {count:>6} {key}"); }
        for (event, count) in &events {
            let name = EntityEvent::from_i32(*event).map(EntityEvent::name).unwrap_or("?");
            println!("PROFILE event {event:>3} {name:<28} x{count}");
        }
    }

    /// TEST.dm_26 (mp/ffa3) has ammo/health/shield MD3 items, Ghoul2 weapon
    /// pickups and force enlightenment powerups.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/TEST.dm_26"]
    fn demo_items_submit_md3_and_ghoul2_geometry() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/TEST.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut presenter = EntityPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        let mut ghoul2 = PlayerPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        ghoul2.set_async_loading(false);
        let mut weapon_fx = WeaponFx::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap());
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut drawn: std::collections::BTreeMap<String, usize> = Default::default();
        let mut respawn_fades = 0;
        let mut snapshots = 0;
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        let (draws, _, _) = presenter.present_snapshot_entities(
                            &entities, &game, snapshot.server_time, &mut ghoul2, &mut weapon_fx, false,
                        );
                        for entity in entities.iter().filter(|entity| entity.entity_type == super::super::ET_ITEM) {
                            let item = jka_movement::bg_item(entity.state.field_i32("modelindex").unwrap_or(0)).unwrap();
                            let own = draws.iter().filter(|surface| surface.entity_num == entity.number).collect::<Vec<_>>();
                            if !own.is_empty() {
                                *drawn.entry(item.classname.clone()).or_insert(0) += 1;
                            }
                            if own.iter().any(|surface| surface.vertices.iter().any(|vertex| vertex.color[3] < 1.0)) {
                                respawn_fades += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        println!("ITEM REGRESSION snapshots={snapshots} fades={respawn_fades} drawn={drawn:?}");
        for classname in ["ammo_powercell", "item_medpak_instant", "weapon_repeater", "item_force_enlighten_light"] {
            assert!(drawn.contains_key(classname), "{classname} never submitted geometry");
        }
    }

    /// TEST.dm_26 fires blasters, repeaters, rockets, thermals and concussion
    /// rifles: trails and impacts must run through the FX system and
    /// tessellate with resolved FX materials.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/TEST.dm_26"]
    fn demo_weapon_fx_trails_and_impacts_tessellate() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let open = || AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = open().read("demos/TEST.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut presenter = EntityPresenter::new(open(), false).unwrap();
        let mut ghoul2 = PlayerPresenter::new(open(), false).unwrap();
        ghoul2.set_async_loading(false);
        let mut weapon_fx = WeaponFx::new(open());
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let (mut snapshots, mut trail_frames, mut impact_events, mut peak_draws) = (0, 0, 0, 0);
        let mut textured = 0usize;
        let mut surfaces_total = 0usize;
        let mut kinds = std::collections::BTreeSet::new();
        let view = crate::fx::draw::FxView { origin: [0.0; 3], axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], fov_x: 90.0 };
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        weapon_fx.begin_frame(snapshot.server_time);
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        for event in game.drain_presentation_events() {
                            if let Some(super::super::event_presenter::EventDispatchResult::Handled("FX_MISSILE_IMPACT")) =
                                weapon_fx.entity_event(&event, &game, &[])
                            {
                                impact_events += 1;
                            }
                        }
                        let missiles = entities.iter().filter(|entity| entity.entity_type == super::super::ET_MISSILE).count();
                        presenter.present_snapshot_entities(&entities, &game, snapshot.server_time, &mut ghoul2, &mut weapon_fx, false);
                        let frame = weapon_fx.end_frame();
                        if missiles > 0 && !frame.draws.is_empty() {
                            trail_frames += 1;
                        }
                        peak_draws = peak_draws.max(frame.draws.len());
                        for draw in &frame.draws {
                            kinds.insert(format!("{:?}", std::mem::discriminant(draw)));
                        }
                        let surfaces = crate::fx::draw::tessellate(&frame.draws, &view, &mut |shader| presenter.fx_material_stages(shader));
                        surfaces_total += surfaces.len();
                        textured += surfaces.iter().filter(|surface| surface.texture.is_some()).count();
                    }
                    _ => {}
                }
            }
        }
        println!(
            "FX REGRESSION snapshots={snapshots} trailFrames={trail_frames} impacts={impact_events} peakDraws={peak_draws} kinds={} surfaces={surfaces_total} textured={textured} stats={:?}",
            kinds.len(), weapon_fx.stats()
        );
        assert!(trail_frames > 0, "missiles never produced trail primitives");
        assert!(impact_events > 0, "no EV_MISSILE_* impact effect played");
        assert!(textured * 10 >= surfaces_total * 9, "FX materials failed to resolve textures");
    }

    /// Real demo -> CGame -> CG_Mover -> inline model instances, checked
    /// against the map's own BSP inline models. No window or GPU is needed.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/cheezyVsource.dm_26 (or MOVER_DEMO)"]
    fn demo_movers_submit_real_inline_models_and_move() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let demo = std::env::var("MOVER_DEMO").unwrap_or_else(|_| "cheezyVsource".into());
        let bytes = assets
            .read(&format!("demos/{demo}.dm_26"), 64 * 1024 * 1024)
            .unwrap()
            .expect("regression demo")
            .bytes;
        let mut presenter = EntityPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        let mut ghoul2 = PlayerPresenter::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap(), false).unwrap();
        ghoul2.set_async_loading(false);
        let mut weapon_fx = WeaponFx::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap());
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut drawable: Option<std::collections::BTreeSet<u32>> = None;
        let mut first_pose: HashMap<u32, InlineModelInstance> = HashMap::new();
        let mut moved = std::collections::BTreeSet::new();
        let mut submitted = std::collections::BTreeSet::new();
        let mut snapshots = 0;
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                        let map = decoder.map_name().expect("gamestate map name");
                        let bsp_bytes = assets
                            .read(&format!("maps/{map}.bsp"), jka_assets::bsp::MAX_FILE_BYTES)
                            .unwrap()
                            .expect("demo map BSP")
                            .bytes;
                        let bsp = jka_assets::bsp::Bsp::parse(&bsp_bytes).unwrap();
                        let (_, batch_models) = bsp.inline_models_mesh(4).unwrap();
                        drawable = Some(batch_models.into_iter().collect());
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        game.set_initial_snapshot(snapshot).unwrap();
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        let (_, inline, _) = presenter.present_snapshot_entities(
                            &entities, &game, snapshot.server_time, &mut ghoul2, &mut weapon_fx, false,
                        );
                        snapshots += 1;
                        for instance in inline {
                            submitted.insert(instance.model);
                            let first = *first_pose.entry(instance.model).or_insert(instance);
                            if first != instance {
                                moved.insert(instance.model);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if snapshots >= 3000 && !moved.is_empty() { break; }
        }
        let drawable = drawable.expect("demo had a gamestate");
        println!(
            "MOVER REGRESSION snapshots={snapshots} drawableInline={} submitted={} moved={:?}",
            drawable.len(), submitted.len(), moved
        );
        assert!(!submitted.is_empty(), "no ET_MOVER inline models were submitted");
        // Every submitted mover with surfaces must resolve to BSP geometry; a
        // surfaceless one (e.g. an invisible blocker) legitimately draws nothing.
        assert!(submitted.iter().any(|model| drawable.contains(model)));
        assert!(submitted.iter().all(|&model| (model as usize) < 4096));
        assert!(!moved.is_empty(), "no mover changed pose during the demo");
    }

    #[test]
    fn entity_rgba_defaults_to_white() {
        let entity = PresentedEntity {
            number: 1,
            entity_type: super::super::ET_GENERAL,
            origin: [0.0; 3],
            angles: [0.0; 3],
            state: jka_protocol::gamestate::EntityState {
                number: 1,
                fields: [0; jka_protocol::gamestate::ENTITY_FIELDS.len()],
            },
        };
        assert_eq!(entity_color(&entity), [1.0; 4]);
    }
}
