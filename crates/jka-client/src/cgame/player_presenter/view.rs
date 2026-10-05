//! View.
use crate::cgame::player_presenter::{scene, PresentedEntity, Vec3};

/// The viewer's model animation, as fed to `CG_PlayerAnimation` and as played.
/// `*_input` are the raw `legsAnim`/`torsoAnim` (toggle bit included).
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewerAnimDebug {
    pub legs_input: i32,
    pub torso_input: i32,
    pub legs_anim: i32,
    pub legs_frame: i32,
    pub legs_old_frame: i32,
    pub legs_backlerp: f32,
    pub torso_anim: i32,
    pub torso_frame: i32,
    pub torso_old_frame: i32,
    pub torso_backlerp: f32,
    /// `current_time` the pose was last evaluated at.
    pub pose_time: i32,
    /// Where and how the model was placed: `entity.origin` / `entity.angles`.
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    /// `*l_leg_foot` / `*r_leg_foot` in model space after posing; a pose pop shows here.
    pub feet: [[f32; 3]; 2],
    /// Low 32 bits of a hash of the model/skin key, and whether the real model was ready.
    pub model_hash: u32,
    pub model_settled: bool,
    /// Posed bolt positions in model space (right hand, left hand, head, chest),
    /// from the same `pose` that is skinned and drawn. Zero when a bolt is missing.
    pub bones: [[f32; 3]; 4],
    /// The transform the model is actually drawn with (`ghoul2_render_transform`):
    /// origin and the facing yaw of its forward axis, in degrees.
    pub render_origin: [f32; 3],
    pub render_yaw: f32,
    /// Set by `present_player_entity` after the call: its `current_time`, whether
    /// it was asked to submit geometry, and how many surfaces it returned.
    pub call_time: i32,
    pub submit_geometry: bool,
    pub surfaces: u32,
    /// World-space anchors from the same pose and transform used for drawing.
    pub head_world: Option<[f32; 3]>,
    pub hilt_world: [Option<[f32; 3]>; 2],
    pub hilt_submitted: [bool; 2],
    pub blade_world: [[Option<[f32; 3]>; 8]; 2],
    pub blade_core_world: [[Option<[f32; 3]>; 8]; 2],
    pub blade_length: [[f32; 8]; 2],
    pub blade_submitted: [[bool; 8]; 2],
}

pub(in crate::cgame::player_presenter) const EF_TELEPORT_BIT: i32 = 1 << 3;

pub(in crate::cgame::player_presenter) const WP_SABER: i32 = 3;

pub(in crate::cgame::player_presenter) const WP_BRYAR_PISTOL: i32 = 4;

// OpenJK bg_public.h: server-authored detached Ghoul2 limb entities.
pub(in crate::cgame::player_presenter) const G2_MODEL_PART: i32 = 50;

pub(in crate::cgame::player_presenter) const G2_MODELPART_HEAD: i32 = 10;

pub(in crate::cgame::player_presenter) const G2_MODELPART_WAIST: i32 = 11;

pub(in crate::cgame::player_presenter) const G2_MODELPART_LARM: i32 = 12;

pub(in crate::cgame::player_presenter) const G2_MODELPART_RARM: i32 = 13;

pub(in crate::cgame::player_presenter) const G2_MODELPART_RHAND: i32 = 14;

pub(in crate::cgame::player_presenter) const G2_MODELPART_LLEG: i32 = 15;

pub(in crate::cgame::player_presenter) const G2_MODELPART_RLEG: i32 = 16;

pub(in crate::cgame::player_presenter) const EF_NODRAW: i32 = 1 << 8;

pub(in crate::cgame::player_presenter) const EF_DEAD: i32 = 1 << 1;

pub(in crate::cgame::player_presenter) const EF_RAG: i32 = 1 << 6;

pub(in crate::cgame::player_presenter) const EF2_SHIP_DEATH: i32 = 1 << 7;

// OpenJK MP CG_PlayerShadow / bg_public.h / teams.h values. Blob shadows are
// deliberately a CGame presentation feature, not part of the CSM/RT paths.
pub(in crate::cgame::player_presenter) const PW_CLOAKED: i32 = 11;

pub(in crate::cgame::player_presenter) const CLASS_REMOTE: i32 = 39;

pub(in crate::cgame::player_presenter) const CLASS_SEEKER: i32 = 41;

// Stock OpenJK MP CG_PlayerShadow uses SHADOW_DISTANCE 128; jaPRO, TaystJK and
// EternalJK raise it to 512, so the blob stays visible far below a jumping player.
pub(in crate::cgame::player_presenter) const BLOB_SHADOW_DISTANCE: f32 = 512.0;

pub(in crate::cgame::player_presenter) const BLOB_SHADOW_RADIUS: f32 = 24.0;

pub(in crate::cgame::player_presenter) const BLOB_SHADOW_DROID_RADIUS: f32 = 8.0;

pub(in crate::cgame::player_presenter) const BLOB_SHADOW_MINS: [f32; 3] = [-15.0, -15.0, 0.0];

pub(in crate::cgame::player_presenter) const BLOB_SHADOW_MAXS: [f32; 3] = [15.0, 15.0, 2.0];

pub(in crate::cgame::player_presenter) const MASK_PLAYERSOLID: i32 =
    0x0000_0001 | 0x0000_0010 | 0x0000_0100 | 0x0000_1000;

pub(in crate::cgame::player_presenter) const MAX_GLM_BYTES: usize = 64 * 1024 * 1024;

pub(in crate::cgame::player_presenter) const MAX_GLA_BYTES: usize = 128 * 1024 * 1024;

pub(in crate::cgame::player_presenter) const MAX_SKIN_BYTES: usize = 4 * 1024 * 1024;

pub(in crate::cgame::player_presenter) const MAX_JIGGLE_BYTES: usize = 64 * 1024;

/// Renderer-view data used by the Ghoul2 presenter for the same whole-model
/// sphere cull and projected-radius LOD decision that OpenJK performs in
/// `R_AddGhoulSurfaces`/`G2_ComputeLOD`.
#[derive(Clone, Copy, Debug)]
pub struct Ghoul2PresentationView {
    pub(in crate::cgame::player_presenter) position: Vec3,
    pub(in crate::cgame::player_presenter) forward: Vec3,
    pub(in crate::cgame::player_presenter) right: Vec3,
    pub(in crate::cgame::player_presenter) up: Vec3,
    pub(in crate::cgame::player_presenter) tan_half_fov_y: f32,
    pub(in crate::cgame::player_presenter) aspect: f32,
    /// Effective worldspawn distanceCull (JKA units); <= 0 means unlimited.
    pub(in crate::cgame::player_presenter) distance_cull: f32,
}

impl Ghoul2PresentationView {
    pub fn new(
        position: [f32; 3],
        forward: [f32; 3],
        fov_y: f32,
        aspect: f32,
        distance_cull: f32,
    ) -> Self {
        let forward = Vec3::from_array(forward).normalize_or_zero();
        let mut right = forward.cross(Vec3::Y).normalize_or_zero();
        if right.length_squared() < 1.0e-8 {
            right = Vec3::X;
        }
        let up = right.cross(forward).normalize_or_zero();
        Self {
            position: Vec3::from_array(position),
            forward,
            right,
            up,
            tan_half_fov_y: (fov_y * 0.5).tan().max(1.0e-6),
            aspect: aspect.max(1.0e-6),
            distance_cull,
        }
    }

    pub fn with_pose(self, position: [f32; 3], forward: [f32; 3]) -> Self {
        let forward = Vec3::from_array(forward).normalize_or_zero();
        let mut right = forward.cross(Vec3::Y).normalize_or_zero();
        if right.length_squared() < 1.0e-8 {
            right = Vec3::X;
        }
        let up = right.cross(forward).normalize_or_zero();
        Self {
            position: Vec3::from_array(position),
            forward,
            right,
            up,
            ..self
        }
    }

    pub(crate) fn sphere_outside(self, jka_origin: [f32; 3], radius: f32) -> bool {
        let center = Vec3::from_array(scene::render_position(jka_origin));
        let delta = center - self.position;
        let depth = delta.dot(self.forward);
        let radius = radius.max(0.0);

        // OpenJK's R_GCullModel only tests the four side frustum planes, not
        // zNear/zFar. Keep that behavior here: objects behind the camera are
        // rejected by the side planes just as they are by R_CullPointAndRadius.
        let x = delta.dot(self.right);
        let y = delta.dot(self.up);
        let tan_x = self.tan_half_fov_y * self.aspect;
        let side_x = depth * tan_x - x.abs();
        let side_y = depth * self.tan_half_fov_y - y.abs();
        let radius_x = radius * (1.0 + tan_x * tan_x).sqrt();
        let radius_y = radius * (1.0 + self.tan_half_fov_y * self.tan_half_fov_y).sqrt();
        side_x < -radius_x || side_y < -radius_y
    }

    pub(crate) fn distance_cull(self) -> f32 {
        self.distance_cull
    }

    pub(crate) fn aspect(self) -> f32 {
        self.aspect
    }

    pub(crate) fn fov_x_degrees(self) -> f32 {
        (2.0 * (self.tan_half_fov_y * self.aspect).atan()).to_degrees()
    }

    /// Camera origin converted back to JKA's native Z-up coordinates. Auxiliary
    /// CGame refEntities such as the invulnerability half-shield orient
    /// themselves from `cg.refdef.vieworg`, not from the player's viewangles.
    pub(crate) fn jka_position(self) -> [f32; 3] {
        scene::jka_position(self.position.to_array())
    }

    /// Straight-line distance from the view to a JKA-space point.
    pub(crate) fn distance_to(self, jka_origin: [f32; 3]) -> f32 {
        Vec3::from_array(scene::render_position(jka_origin)).distance(self.position)
    }

    pub(in crate::cgame::player_presenter) fn projected_radius(
        self,
        jka_origin: [f32; 3],
        radius: f32,
    ) -> f32 {
        let center = Vec3::from_array(scene::render_position(jka_origin));
        let depth = (center - self.position).dot(self.forward);
        // ProjectRadius returns zero for a model at/behind the view plane; the
        // caller treats that as LOD0 (view weapon / near-plane intersection).
        if depth <= 0.0 {
            0.0
        } else {
            radius.abs() / (depth * self.tan_half_fov_y)
        }
    }
}

pub(in crate::cgame::player_presenter) fn ghoul2_entity_radius(entity: &PresentedEntity) -> f32 {
    entity
        .state
        .field_i32("g2radius")
        .filter(|radius| *radius != 0)
        .map(|radius| radius as f32)
        .unwrap_or(64.0)
}

pub(in crate::cgame::player_presenter) fn ghoul2_model_scale(entity: &PresentedEntity) -> f32 {
    let scale = entity.state.field_i32("iModelScale").unwrap_or(0);
    if scale == 0 {
        1.0
    } else {
        (scale as f32 / 100.0).abs().max(1.0e-6)
    }
}

/// TaystJK/OpenJK CG_Player modelScale semantics. `iModelScale` is a uniform
/// percent scale for networked NPC/player Ghoul2 models. The renderer applies
/// it to all three entity axes. Non-vehicle characters also receive the legacy
/// vertical correction so small models (notably Jawas) keep their feet on the
/// floor instead of shrinking around their origin.
pub(in crate::cgame::player_presenter) fn ghoul2_render_transform(
    entity: &PresentedEntity,
    mut axis: [[f32; 3]; 3],
) -> ([[f32; 3]; 3], [f32; 3]) {
    let scale_int = entity.state.field_i32("iModelScale").unwrap_or(0);
    let mut origin = entity.origin;
    if scale_int == 0 {
        return (axis, origin);
    }

    let scale = scale_int as f32 / 100.0;
    if scale != 1.0 {
        for basis in &mut axis {
            for component in basis {
                *component *= scale;
            }
        }
        if entity.state.field_i32("NPC_class").unwrap_or(0) != crate::cgame::CLASS_VEHICLE {
            origin[2] += 24.0 * (scale - 1.0);
        }
    }
    (axis, origin)
}

pub(in crate::cgame::player_presenter) fn ghoul2_render_origin(
    entity: &PresentedEntity,
) -> [f32; 3] {
    let (_, origin) =
        ghoul2_render_transform(entity, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    origin
}

/// Faithful G2_ComputeLOD shape with OpenJK `r_lodscale` (default 5) and
/// r_autolodscalevalue=0. `global_lod_bias` is the archived JKA `r_lodbias`
/// cvar. Per-model mLodBias is not authored by our current GLM presentation
/// state, so it is zero; OpenJK therefore effectively uses max(r_lodbias, 0).
pub(in crate::cgame::player_presenter) fn ghoul2_lod_for_view(
    view: Ghoul2PresentationView,
    entity: &PresentedEntity,
    render_origin: [f32; 3],
    num_lods: usize,
    global_lod_bias: i32,
    lod_scale: f32,
) -> usize {
    if num_lods < 2 {
        return 0;
    }
    let scaled_radius = 0.75 * ghoul2_entity_radius(entity) * ghoul2_model_scale(entity);
    let projected_radius = view.projected_radius(render_origin, scaled_radius);
    let flod = if projected_radius != 0.0 {
        1.0 - projected_radius * lod_scale
    } else {
        0.0
    };
    // OpenJK x64 Q_ftol uses cvttss2si: truncate toward zero, clamps the
    // projected LOD, then adds max(r_lodbias, modelBias) and clamps again.
    let max_lod = num_lods.saturating_sub(1) as isize;
    let lod = ((flod * num_lods as f32) as isize).clamp(0, max_lod);
    let lod_bias = global_lod_bias.max(0) as isize;
    (lod + lod_bias).clamp(0, max_lod) as usize
}
