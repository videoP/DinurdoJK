//! Bodies dismemberment.
use crate::cgame::player_presenter::{
    animation_index, ghoul2_lod_for_view, is_death_animation, model_bolt_matrix_timed,
    multiply_3x4, normalize_vec3, transform_jka_model_point, transform_jka_model_vector, Arc,
    DetachedLimbGeneration, DetachedLimbSpawn, DynamicModelSurface, Ghoul2PresentationView,
    GlmModel, HashSet, Matrix3x4, PlayerFxRequest, PlayerModelAsset, PlayerPresenter,
    PresentedEntity, ANIM_TOGGLEBIT, EF_DEAD, ET_GENERAL, G2_MODELPART_HEAD, G2_MODELPART_LARM,
    G2_MODELPART_LLEG, G2_MODELPART_RARM, G2_MODELPART_RHAND, G2_MODELPART_RLEG,
    G2_MODELPART_WAIST, G2_MODEL_PART,
};

#[derive(Clone, Copy, Debug)]
pub(in crate::cgame::player_presenter) struct BodyQueueCopyState {
    pub(in crate::cgame::player_presenter) source_client: u16,
    pub(in crate::cgame::player_presenter) _known_weapon: i32,
    pub(in crate::cgame::player_presenter) _light_side: bool,
    /// Ghoul2 model index 1 after CG_BodyQueueCopy has duplicated the source
    /// instance and applied its knownWeapon correction. This is deliberately a
    /// g2WeaponInstances-style weapon identity: for ET_BODY, WP_SABER resolves
    /// to OpenJK's default saber instance rather than the live client's custom
    /// primary saber instance.
    pub(in crate::cgame::player_presenter) model1_weapon: Option<i32>,
    /// Ghoul2 model index 2 survives the body copy independently. OpenJK uses
    /// it for the second saber and does not strip it with the model-1 rule.
    pub(in crate::cgame::player_presenter) model2_saber: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::cgame::player_presenter) enum DismemberPart {
    Head,
    Waist,
    LeftArm,
    RightArm,
    RightHand,
    LeftLeg,
    RightLeg,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::cgame::player_presenter) struct DismemberSpec {
    /// Ghoul2 bone OpenJK uses to position/orient the detached copy.
    pub(in crate::cgame::player_presenter) rotate_bone: &'static str,
    pub(in crate::cgame::player_presenter) limb_root: &'static str,
    pub(in crate::cgame::player_presenter) stub_root: &'static str,
    pub(in crate::cgame::player_presenter) limb_tag: &'static str,
    pub(in crate::cgame::player_presenter) stub_tag: &'static str,
    /// Bone endpoints used only for the single cheap Rapier collider.
    pub(in crate::cgame::player_presenter) collider_a: &'static str,
    pub(in crate::cgame::player_presenter) collider_b: Option<&'static str>,
    pub(in crate::cgame::player_presenter) radius: f32,
    pub(in crate::cgame::player_presenter) mass_kg: f32,
}

impl DismemberPart {
    pub(in crate::cgame::player_presenter) fn from_model_part(value: i32) -> Option<Self> {
        Some(match value {
            G2_MODELPART_HEAD => Self::Head,
            G2_MODELPART_WAIST => Self::Waist,
            G2_MODELPART_LARM => Self::LeftArm,
            G2_MODELPART_RARM => Self::RightArm,
            G2_MODELPART_RHAND => Self::RightHand,
            G2_MODELPART_LLEG => Self::LeftLeg,
            G2_MODELPART_RLEG => Self::RightLeg,
            _ => return None,
        })
    }

    pub(in crate::cgame::player_presenter) fn model_part(self) -> i32 {
        match self {
            Self::Head => G2_MODELPART_HEAD,
            Self::Waist => G2_MODELPART_WAIST,
            Self::LeftArm => G2_MODELPART_LARM,
            Self::RightArm => G2_MODELPART_RARM,
            Self::RightHand => G2_MODELPART_RHAND,
            Self::LeftLeg => G2_MODELPART_LLEG,
            Self::RightLeg => G2_MODELPART_RLEG,
        }
    }

    pub(in crate::cgame::player_presenter) fn allowed_at_level(self, level: u8) -> bool {
        level >= 2 || (level >= 1 && !matches!(self, Self::Head | Self::Waist))
    }

    pub(in crate::cgame::player_presenter) fn spec(self) -> DismemberSpec {
        match self {
            // Exact OpenJK CG_General names/tags. Collider bones are a modern
            // Rapier-only approximation and never feed gameplay or sever choice.
            Self::Head => DismemberSpec {
                rotate_bone: "cranium",
                limb_root: "head",
                stub_root: "torso",
                limb_tag: "*head_cap_torso",
                stub_tag: "*torso_cap_head",
                collider_a: "cranium",
                collider_b: None,
                radius: 5.5,
                mass_kg: 5.0,
            },
            Self::Waist => DismemberSpec {
                rotate_bone: "thoracic",
                limb_root: "torso",
                stub_root: "hips",
                limb_tag: "*torso_cap_hips",
                stub_tag: "*hips_cap_torso",
                collider_a: "lower_lumbar",
                collider_b: Some("cranium"),
                radius: 7.0,
                mass_kg: 30.0,
            },
            Self::LeftArm => DismemberSpec {
                rotate_bone: "lradius",
                limb_root: "l_arm",
                stub_root: "torso",
                limb_tag: "*l_arm_cap_torso",
                stub_tag: "*torso_cap_l_arm",
                collider_a: "lhumerus",
                collider_b: Some("lhand"),
                radius: 3.2,
                mass_kg: 4.3,
            },
            Self::RightArm => DismemberSpec {
                rotate_bone: "rradius",
                limb_root: "r_arm",
                stub_root: "torso",
                limb_tag: "*r_arm_cap_torso",
                stub_tag: "*torso_cap_r_arm",
                collider_a: "rhumerus",
                collider_b: Some("rhand"),
                radius: 3.2,
                mass_kg: 4.3,
            },
            Self::RightHand => DismemberSpec {
                rotate_bone: "rhand",
                limb_root: "r_hand",
                stub_root: "r_arm",
                limb_tag: "*r_hand_cap_r_arm",
                stub_tag: "*r_arm_cap_r_hand",
                collider_a: "rhand",
                collider_b: None,
                radius: 2.8,
                mass_kg: 0.7,
            },
            Self::LeftLeg => DismemberSpec {
                rotate_bone: "ltibia",
                limb_root: "l_leg",
                stub_root: "hips",
                limb_tag: "*l_leg_cap_hips",
                stub_tag: "*hips_cap_l_leg",
                collider_a: "lfemurYZ",
                collider_b: Some("ltalus"),
                radius: 4.0,
                mass_kg: 12.0,
            },
            Self::RightLeg => DismemberSpec {
                rotate_bone: "rtibia",
                limb_root: "r_leg",
                stub_root: "hips",
                limb_tag: "*r_leg_cap_hips",
                stub_tag: "*hips_cap_r_leg",
                collider_a: "rfemurYZ",
                collider_b: Some("rtalus"),
                radius: 4.0,
                mass_kg: 12.0,
            },
        }
    }

    pub(in crate::cgame::player_presenter) fn removes_weapon(self) -> bool {
        matches!(self, Self::Waist | Self::RightArm | Self::RightHand)
    }
}

#[derive(Clone)]
pub(in crate::cgame::player_presenter) struct DismemberSourceSnap {
    pub(in crate::cgame::player_presenter) model: Arc<PlayerModelAsset>,
    pub(in crate::cgame::player_presenter) pose: Vec<Matrix3x4>,
    pub(in crate::cgame::player_presenter) axis: [[f32; 3]; 3],
    pub(in crate::cgame::player_presenter) origin: [f32; 3],
    pub(in crate::cgame::player_presenter) model_scale: f32,
    pub(in crate::cgame::player_presenter) body_rgba: [f32; 4],
    pub(in crate::cgame::player_presenter) body_rgb: [f32; 3],
    /// Ghoul2 model slot 1 as it existed before CG_General mutates the source.
    /// OpenJK duplicates this slot onto the detached copy after first removing
    /// model slot 2 (the second saber) and model slot 3 (jetpack).
    pub(in crate::cgame::player_presenter) model1_weapon: Option<i32>,
    pub(in crate::cgame::player_presenter) model1_primary_saber: bool,
    pub(in crate::cgame::player_presenter) client_info: crate::cgame::ClientInfo,
}

#[derive(Clone)]
pub(in crate::cgame::player_presenter) struct DetachedLimbVisual {
    pub(in crate::cgame::player_presenter) generation: DetachedLimbGeneration,
    pub(in crate::cgame::player_presenter) source_entity: u16,
    pub(in crate::cgame::player_presenter) part: DismemberPart,
    pub(in crate::cgame::player_presenter) model: Arc<PlayerModelAsset>,
    pub(in crate::cgame::player_presenter) pose: Vec<Matrix3x4>,
    pub(in crate::cgame::player_presenter) surfaces: HashSet<usize>,
    pub(in crate::cgame::player_presenter) spawn_entity_matrix: Matrix3x4,
    pub(in crate::cgame::player_presenter) spawn_body_matrix: Matrix3x4,
    pub(in crate::cgame::player_presenter) body_rgba: [f32; 4],
    pub(in crate::cgame::player_presenter) body_rgb: [f32; 3],
    pub(in crate::cgame::player_presenter) model1_weapon: Option<i32>,
    pub(in crate::cgame::player_presenter) model1_primary_saber: bool,
    pub(in crate::cgame::player_presenter) client_info: crate::cgame::ClientInfo,
    pub(in crate::cgame::player_presenter) next_smoke_time: i32,
}

pub(in crate::cgame::player_presenter) fn dismember_source_entity(
    entity: &PresentedEntity,
) -> Option<u16> {
    if entity.entity_type != ET_GENERAL
        || entity.state.field_i32("weapon").unwrap_or(0) != G2_MODEL_PART
        || DismemberPart::from_model_part(entity.state.field_i32("modelGhoul2").unwrap_or(0))
            .is_none()
    {
        return None;
    }
    let model_index = entity.state.field_i32("modelindex").unwrap_or(-1);
    let source = if model_index >= 0 {
        model_index
    } else {
        entity.state.field_i32("otherEntityNum2").unwrap_or(-1)
    };
    u16::try_from(source).ok()
}

pub(in crate::cgame::player_presenter) fn dismember_generation(
    entity: &PresentedEntity,
    source_entity: u16,
    part: DismemberPart,
) -> DetachedLimbGeneration {
    DetachedLimbGeneration {
        source_entity,
        kind: part.model_part() as u8,
        trajectory_time: entity.state.field_i32("pos.trTime").unwrap_or(0),
    }
}

pub(in crate::cgame::player_presenter) fn dismember_source_ready(
    source_entity: &PresentedEntity,
    presented_torso_anim: Option<i32>,
) -> bool {
    // Stock CG_General waits until the owner's EF_DEAD and death animation have
    // reached the client before cloning the Ghoul2 instance. Without this gate,
    // a newly received model-part entity can visually sever a still-living pose.
    if source_entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0 {
        return false;
    }

    let state_anim = source_entity.state.field_i32("torsoAnim").unwrap_or(0) & !ANIM_TOGGLEBIT;
    let chopped_hand =
        animation_index("BOTH_RIGHTHANDCHOPPEDOFF").and_then(|index| i32::try_from(index).ok());
    if chopped_hand == Some(state_anim) {
        return true;
    }

    let Some(presented_anim) = presented_torso_anim else {
        // CG_General also requires clEnt->pe.torso.animationNumber to be a
        // death animation. If this source has not been presented yet, wait one
        // frame rather than assuming its lerp-frame state already caught up.
        return false;
    };
    let presented_anim = presented_anim & !ANIM_TOGGLEBIT;
    is_death_animation(state_anim) && is_death_animation(presented_anim)
}

pub(in crate::cgame::player_presenter) fn hierarchy_surface_index(
    glm: &GlmModel,
    name: &str,
) -> Option<usize> {
    glm.hierarchy
        .iter()
        .position(|surface| surface.name.eq_ignore_ascii_case(name))
}

/// OpenJK asks `BG_GetRootSurfNameWithVariant` for limb/stub roots. Player
/// skins use one-letter root variants (`r_arma`, `torsoa`, `hipsa`, ...), while
/// `_1`/`_2` suffixes are LOD surface names and must not be mistaken for a skin
/// variant. Prefer the unsuffixed stock root, then accept exactly one ASCII
/// alphabetic variant character.
pub(in crate::cgame::player_presenter) fn resolve_dismember_root(
    glm: &GlmModel,
    base: &str,
) -> Option<usize> {
    hierarchy_surface_index(glm, base).or_else(|| {
        let base = base.to_ascii_lowercase();
        glm.hierarchy.iter().position(|surface| {
            let name = surface.name.to_ascii_lowercase();
            let Some(suffix) = name.strip_prefix(&base) else {
                return false;
            };
            suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_alphabetic()
        })
    })
}

pub(in crate::cgame::player_presenter) fn collect_surface_subtree(
    glm: &GlmModel,
    root: usize,
    out: &mut HashSet<usize>,
) {
    if !out.insert(root) {
        return;
    }
    if let Some(surface) = glm.hierarchy.get(root) {
        for &child in &surface.children {
            collect_surface_subtree(glm, child, out);
        }
    }
}

pub(in crate::cgame::player_presenter) fn variant_cap_name(
    glm: &GlmModel,
    root: usize,
    base_root: &str,
    default_tag: &str,
) -> String {
    let default_cap = default_tag.trim_start_matches('*');
    let Some(root_name) = glm.hierarchy.get(root).map(|surface| surface.name.as_str()) else {
        return default_cap.to_owned();
    };
    if root_name.eq_ignore_ascii_case(base_root) {
        return default_cap.to_owned();
    }
    let suffix = default_cap.strip_prefix(base_root).unwrap_or(default_cap);
    format!("{root_name}{suffix}")
}

pub(in crate::cgame::player_presenter) fn default_surface_set(
    model: &PlayerModelAsset,
) -> HashSet<usize> {
    model
        .surfaces
        .iter()
        .filter(|surface| surface.default_visible)
        .map(|surface| surface.surface_index)
        .collect()
}

pub(in crate::cgame::player_presenter) fn source_dismember_surface_set(
    model: &PlayerModelAsset,
    parts: Option<&HashSet<DismemberPart>>,
) -> Option<HashSet<usize>> {
    let parts = parts.filter(|parts| !parts.is_empty())?;
    let mut visible = default_surface_set(model);
    let drawable = model
        .surfaces
        .iter()
        .map(|surface| surface.surface_index)
        .collect::<HashSet<_>>();
    for part in parts {
        let spec = part.spec();
        let Some(limb_root) = resolve_dismember_root(&model.glm, spec.limb_root) else {
            continue;
        };
        let mut hidden = HashSet::new();
        collect_surface_subtree(&model.glm, limb_root, &mut hidden);
        visible.retain(|index| !hidden.contains(index));

        let Some(stub_root) = resolve_dismember_root(&model.glm, spec.stub_root) else {
            continue;
        };
        let cap = variant_cap_name(&model.glm, stub_root, spec.stub_root, spec.stub_tag);
        if let Some(cap_index) =
            hierarchy_surface_index(&model.glm, &cap).filter(|index| drawable.contains(index))
        {
            visible.insert(cap_index);
        }
    }
    Some(visible)
}

pub(in crate::cgame::player_presenter) fn detached_limb_surface_set(
    model: &PlayerModelAsset,
    current: DismemberPart,
    all_parts: Option<&HashSet<DismemberPart>>,
) -> HashSet<usize> {
    let spec = current.spec();
    let drawable = model
        .surfaces
        .iter()
        .map(|surface| surface.surface_index)
        .collect::<HashSet<_>>();
    let Some(root) = resolve_dismember_root(&model.glm, spec.limb_root) else {
        return HashSet::new();
    };
    let mut subtree = HashSet::new();
    collect_surface_subtree(&model.glm, root, &mut subtree);
    let mut visible = model
        .surfaces
        .iter()
        .filter(|surface| surface.default_visible && subtree.contains(&surface.surface_index))
        .map(|surface| surface.surface_index)
        .collect::<HashSet<_>>();

    let cap = variant_cap_name(&model.glm, root, spec.limb_root, spec.limb_tag);
    if let Some(cap_index) =
        hierarchy_surface_index(&model.glm, &cap).filter(|index| drawable.contains(index))
    {
        visible.insert(cap_index);
    }

    // The detached Ghoul2 copy inherits older sever flags from the source.
    if let Some(parts) = all_parts {
        for part in parts.iter().copied().filter(|part| *part != current) {
            let prior = part.spec();
            if let Some(prior_root) = resolve_dismember_root(&model.glm, prior.limb_root) {
                let mut hidden = HashSet::new();
                collect_surface_subtree(&model.glm, prior_root, &mut hidden);
                visible.retain(|index| !hidden.contains(index));
            }
        }
    }
    visible
}

pub(in crate::cgame::player_presenter) fn presentation_matrix(
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
) -> Matrix3x4 {
    [
        [axis[0][0], axis[1][0], axis[2][0], origin[0]],
        [axis[0][1], axis[1][1], axis[2][1], origin[1]],
        [axis[0][2], axis[1][2], axis[2][2], origin[2]],
    ]
}

pub(in crate::cgame::player_presenter) fn scaled_bone_matrix(
    mut matrix: Matrix3x4,
    scale: f32,
) -> Matrix3x4 {
    matrix[0][3] *= scale;
    matrix[1][3] *= scale;
    matrix[2][3] *= scale;
    matrix
}

pub(in crate::cgame::player_presenter) fn affine_inverse_3x4(
    matrix: &Matrix3x4,
) -> Option<Matrix3x4> {
    let m = glam::Mat4::from_cols(
        glam::Vec4::new(matrix[0][0], matrix[1][0], matrix[2][0], 0.0),
        glam::Vec4::new(matrix[0][1], matrix[1][1], matrix[2][1], 0.0),
        glam::Vec4::new(matrix[0][2], matrix[1][2], matrix[2][2], 0.0),
        glam::Vec4::new(matrix[0][3], matrix[1][3], matrix[2][3], 1.0),
    );
    if m.determinant().abs() < 1.0e-8 {
        return None;
    }
    let inv = m.inverse();
    Some([
        [inv.x_axis.x, inv.y_axis.x, inv.z_axis.x, inv.w_axis.x],
        [inv.x_axis.y, inv.y_axis.y, inv.z_axis.y, inv.w_axis.y],
        [inv.x_axis.z, inv.y_axis.z, inv.z_axis.z, inv.w_axis.z],
    ])
}

pub(in crate::cgame::player_presenter) fn presentation_axis_origin(
    matrix: &Matrix3x4,
) -> ([[f32; 3]; 3], [f32; 3]) {
    (
        [
            [matrix[0][0], matrix[1][0], matrix[2][0]],
            [matrix[0][1], matrix[1][1], matrix[2][1]],
            [matrix[0][2], matrix[1][2], matrix[2][2]],
        ],
        [matrix[0][3], matrix[1][3], matrix[2][3]],
    )
}

pub(in crate::cgame::player_presenter) fn affine_point(
    matrix: &Matrix3x4,
    point: [f32; 3],
) -> [f32; 3] {
    [
        matrix[0][0] * point[0] + matrix[0][1] * point[1] + matrix[0][2] * point[2] + matrix[0][3],
        matrix[1][0] * point[0] + matrix[1][1] * point[1] + matrix[1][2] * point[2] + matrix[1][3],
        matrix[2][0] * point[0] + matrix[2][1] * point[1] + matrix[2][2] * point[2] + matrix[2][3],
    ]
}

impl PlayerPresenter {
    pub(in crate::cgame::player_presenter) fn queue_dismember_smoke(
        &mut self,
        model: &PlayerModelAsset,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        tag: &str,
    ) -> Result<(), String> {
        let Some(matrix) =
            model_bolt_matrix_timed(&mut self.perf, &model.glm, &model.gla, pose, tag)?
        else {
            return Ok(());
        };
        let smoke_origin =
            transform_jka_model_point([matrix[0][3], matrix[1][3], matrix[2][3]], axis, origin);
        // Public G2API_GetBoltMatrix's NEGATIVE_Y convention corresponds to
        // -column 1 of the internal bolt matrix used by this presenter.
        let smoke_dir = normalize_vec3(transform_jka_model_vector(
            [-matrix[0][1], -matrix[1][1], -matrix[2][1]],
            axis,
        ));
        self.fx_requests.push(PlayerFxRequest::EffectDir {
            name: "blaster/smoke_bolton".to_owned(),
            origin: smoke_origin,
            dir: smoke_dir,
        });
        Ok(())
    }

    pub(in crate::cgame::player_presenter) fn present_detached_limb(
        &mut self,
        entity: &PresentedEntity,
        source_entity: Option<&PresentedEntity>,
        current_time: i32,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let level = self.ragdolls.dismemberment_level();
        let Some(part) =
            DismemberPart::from_model_part(entity.state.field_i32("modelGhoul2").unwrap_or(0))
        else {
            return Ok(Vec::new());
        };
        if !part.allowed_at_level(level) {
            return Ok(Vec::new());
        }
        let Some(source_num) = dismember_source_entity(entity) else {
            return Ok(Vec::new());
        };
        let generation = dismember_generation(entity, source_num, part);
        let needs_visual = self
            .detached_limb_visuals
            .get(&entity.number)
            .is_none_or(|visual| visual.generation != generation);

        if needs_visual {
            let Some(source_entity) = source_entity else {
                return Ok(Vec::new());
            };
            let presented_torso_anim = self
                .entities
                .get(&source_num)
                .map(|runtime| runtime.animation.torso.animation_number);
            if !dismember_source_ready(source_entity, presented_torso_anim) {
                return Ok(Vec::new());
            }
            let Some(source) = self.dismember_source_snaps.get(&source_num).cloned() else {
                // OpenJK waits for a valid source Ghoul2/death pose too. The
                // next presentation frame will retry after the source is posed.
                return Ok(Vec::new());
            };
            let spec = part.spec();
            let surfaces =
                detached_limb_surface_set(&source.model, part, self.dismembered.get(&source_num));
            if surfaces.is_empty() {
                return Err(format!(
                    "{} has no Ghoul2 subtree for dismember part {:?}",
                    source.model.key, part
                ));
            }

            let rotate_bone = if part == DismemberPart::Waist
                && model_bolt_matrix_timed(
                    &mut self.perf,
                    &source.model.glm,
                    &source.model.gla,
                    &source.pose,
                    spec.rotate_bone,
                )?
                .is_none()
            {
                // Exact OpenJK non-humanoid fallback.
                "pelvis"
            } else {
                spec.rotate_bone
            };
            let Some(bone_model) = model_bolt_matrix_timed(
                &mut self.perf,
                &source.model.glm,
                &source.model.gla,
                &source.pose,
                rotate_bone,
            )?
            else {
                return Err(format!(
                    "{} has no dismember bone {rotate_bone}",
                    source.model.key
                ));
            };
            let source_world = presentation_matrix(source.axis, source.origin);
            let spawn_body_matrix = multiply_3x4(
                &source_world,
                &scaled_bone_matrix(bone_model, source.model_scale),
            );
            let spawn_entity_matrix = source_world;

            let capsule_local = if let Some(end_name) = spec.collider_b {
                let endpoint_world = |presenter: &mut PlayerPresenter,
                                      bone: &str|
                 -> Result<Option<[f32; 3]>, String> {
                    Ok(model_bolt_matrix_timed(
                        &mut presenter.perf,
                        &source.model.glm,
                        &source.model.gla,
                        &source.pose,
                        bone,
                    )?
                    .map(|matrix| {
                        let matrix = scaled_bone_matrix(matrix, source.model_scale);
                        affine_point(&source_world, [matrix[0][3], matrix[1][3], matrix[2][3]])
                    }))
                };
                let a = endpoint_world(self, spec.collider_a)?;
                let b = endpoint_world(self, end_name)?;
                match (a, b, affine_inverse_3x4(&spawn_body_matrix)) {
                    (Some(a), Some(b), Some(inverse)) => {
                        Some((affine_point(&inverse, a), affine_point(&inverse, b)))
                    }
                    _ => None,
                }
            } else {
                None
            };

            let source_origin = source_entity.origin;
            let outward = normalize_vec3([
                entity.origin[0] - source_origin[0],
                entity.origin[1] - source_origin[1],
                entity.origin[2] - source_origin[2],
            ]);
            let network_velocity =
                crate::cgame::entity_vec3(&source_entity.state, "pos.trDelta").unwrap_or([0.0; 3]);
            let inherited = self
                .ragdolls
                .ragdoll_bone_velocity(source_num, &source.model.gla, rotate_bone)
                .unwrap_or(network_velocity);
            // Server G_Dismember adds an 80 u/s outward kick before optional
            // saber-swing contribution. We can observe the source trajectory but
            // not the server's private saber swing vector, so preserve the exact
            // visible component and let Rapier take over from there.
            let mut linear_velocity = [
                inherited[0] + outward[0] * 80.0,
                inherited[1] + outward[1] * 80.0,
                inherited[2] + outward[2] * 80.0,
            ];
            if matches!(part, DismemberPart::Head | DismemberPart::Waist) {
                // Exact server G_Dismember baseline before optional saber-swing
                // contribution.
                linear_velocity[2] += 10.0;
            }
            let mut spin_axis = [outward[1], -outward[0], 0.35];
            spin_axis = normalize_vec3(spin_axis);
            let spin = 2.6 + f32::from((entity.number % 5) as u8) * 0.35;
            let angular_velocity = [
                spin_axis[0] * spin,
                spin_axis[1] * spin,
                spin_axis[2] * spin,
            ];

            // Stock CG_General emits one blaster/smoke_bolton puff from both
            // exposed cap tags when the sever is created. The detached cap then
            // repeats every 400 ms while the server part is moving.
            self.queue_dismember_smoke(
                &source.model,
                &source.pose,
                source.axis,
                source.origin,
                spec.limb_tag,
            )?;
            self.queue_dismember_smoke(
                &source.model,
                &source.pose,
                source.axis,
                source.origin,
                spec.stub_tag,
            )?;

            self.ragdolls.ensure_detached_limb(
                DetachedLimbSpawn {
                    entity: entity.number,
                    generation,
                    body_world: spawn_body_matrix,
                    capsule_local,
                    radius: spec.radius * source.model_scale,
                    mass_kg: spec.mass_kg * source.model_scale.powi(3),
                    linear_velocity,
                    angular_velocity,
                },
                current_time,
            );

            self.detached_limb_visuals.insert(
                entity.number,
                DetachedLimbVisual {
                    generation,
                    source_entity: source_num,
                    part,
                    model: Arc::clone(&source.model),
                    pose: source.pose,
                    surfaces,
                    spawn_entity_matrix,
                    spawn_body_matrix,
                    body_rgba: source.body_rgba,
                    body_rgb: source.body_rgb,
                    model1_weapon: source.model1_weapon,
                    model1_primary_saber: source.model1_primary_saber,
                    client_info: source.client_info,
                    next_smoke_time: current_time.saturating_add(400),
                },
            );
        }

        let Some(visual) = self.detached_limb_visuals.get(&entity.number).cloned() else {
            return Ok(Vec::new());
        };
        if visual.generation != generation {
            return Ok(Vec::new());
        }

        // Rapier renders at display rate from the fixed-step interpolation. If
        // client physics is off/unavailable, preserve OpenJK's server ExPhys
        // translation while keeping the sever pose rigid.
        let current_body = self
            .ragdolls
            .detached_limb_matrix(entity.number, generation)
            .unwrap_or_else(|| {
                let mut matrix = visual.spawn_body_matrix;
                matrix[0][3] = entity.origin[0];
                matrix[1][3] = entity.origin[1];
                matrix[2][3] = entity.origin[2];
                matrix
            });
        let render_matrix = affine_inverse_3x4(&visual.spawn_body_matrix)
            .map(|inverse| {
                multiply_3x4(
                    &multiply_3x4(&current_body, &inverse),
                    &visual.spawn_entity_matrix,
                )
            })
            .unwrap_or(visual.spawn_entity_matrix);
        let (axis, origin) = presentation_axis_origin(&render_matrix);
        let lod = view.map_or(0, |view| {
            ghoul2_lod_for_view(
                view,
                entity,
                origin,
                visual.model.glm.lods.len(),
                self.lod_bias,
                self.lod_scale,
            )
        });
        let mut draws = self.render_glm_surfaces_tinted_selected(
            entity.number,
            &format!("{}#dismember-{:?}", visual.model.key, visual.part),
            &visual.model.glm,
            &visual.model.gla,
            &visual.model.surfaces,
            None,
            &visual.pose,
            lod,
            axis,
            origin,
            visual.body_rgba,
            visual.body_rgb,
            None,
            true,
            Some(&visual.surfaces),
        )?;
        // Detached pieces are one rigid caster, not another articulated skin.
        // CPU and GPU skinning both retain the frozen sever pose; the entity
        // transform above supplies all subsequent movement.
        for draw in &mut draws {
            draw.lighting_origin = Some(origin);
        }

        // CG_General duplicates Ghoul2 after removing model slot 2/3 from the
        // owner, so a held model-slot-1 weapon/hilt remains attached to the
        // detached copy. Generic limb entities never run CG_AddSaberBlade, so
        // saber hilts are rendered here without live blades/trails.
        if visual.model1_primary_saber {
            if let Err(error) = self.append_player_sabers(
                &mut draws,
                entity,
                &visual.client_info,
                &visual.model,
                &visual.pose,
                axis,
                origin,
                current_time,
                visual.body_rgba[3],
                [true, false],
                false,
            ) {
                self.report_saber_warning_once(
                    entity.number,
                    &format!("dismember saber hilt: {error}"),
                );
            }
        } else if let Some(weapon) = visual.model1_weapon {
            if let Err(error) = self.append_held_weapon(
                &mut draws,
                entity,
                weapon,
                &visual.model,
                &visual.pose,
                axis,
                origin,
                current_time,
                visual.body_rgba[3],
            ) {
                self.report_saber_warning_once(
                    entity.number,
                    &format!("dismember held weapon {weapon}: {error}"),
                );
            }
        }

        let tr_delta = crate::cgame::entity_vec3(&entity.state, "pos.trDelta").unwrap_or([0.0; 3]);
        let moving = tr_delta.iter().any(|value| value.abs() > f32::EPSILON);
        if moving && current_time > visual.next_smoke_time {
            let spec = visual.part.spec();
            self.queue_dismember_smoke(&visual.model, &visual.pose, axis, origin, spec.limb_tag)?;
            if let Some(live) = self.detached_limb_visuals.get_mut(&entity.number) {
                if live.generation == generation {
                    live.next_smoke_time = current_time.saturating_add(400);
                }
            }
        }
        Ok(draws)
    }
}
