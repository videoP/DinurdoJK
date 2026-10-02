use egui::Pos2;
use glam::{Mat4, Vec3};
use jka_movement::{TraceQuery, TraceWorld};

/// Raven/OpenJK renderer default when worldspawn does not author distanceCull.
pub const DEFAULT_DISTANCE_CULL: f32 = 6000.0;
/// TaystJK's archived field-of-view default. `cg_fov` is a horizontal FOV
/// authored against the original 4:3 viewport, not a raw vertical WGPU FOV.
pub const DEFAULT_CG_FOV: f32 = 90.0;
pub const MIN_CG_FOV: f32 = 1.0;
pub const MAX_CG_FOV: f32 = 140.0;
/// OpenJK keeps the camera far plane at least this far out.
pub const MIN_FAR_DISTANCE: f32 = 2048.0;
/// Raven expands distanceCull by the diagonal of a unit cube so the far plane
/// does not clip geometry at the corners of the visibility volume.
const DISTANCE_CULL_DIAGONAL: f32 = 1.732_050_8;

// OpenJK MP third-person camera constants.  The cgame camera performs two
// box traces per frame with MASK_CAMERACLIP.  MASK_SOLID includes terrain in
// JKA; MASK_CAMERACLIP adds CONTENTS_PLAYERCLIP.
const CAMERA_DAMP_INTERVAL: f32 = 50.0;
const CAMERA_SIZE: f32 = 4.0;
const CONTENTS_SOLID: i32 = 0x0000_0001;
const CONTENTS_PLAYERCLIP: i32 = 0x0000_0010;
const CONTENTS_TERRAIN: i32 = 0x0000_1000;
const MASK_CAMERACLIP: i32 = CONTENTS_SOLID | CONTENTS_PLAYERCLIP | CONTENTS_TERRAIN;

/// Direct Rust representation of the stock OpenJK MP cg_thirdPerson* cvars.
/// Most defaults come from codemp/cgame/cg_xcvar.h. DinurdoJK intentionally
/// defaults both damping controls to 1.0 so the Setup -> Game defaults match
/// the requested camera behavior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPersonSettings {
    pub enabled: bool,
    pub alpha: f32,
    pub angle: f32,
    pub camera_damp: f32,
    pub horz_offset: f32,
    pub pitch_offset: f32,
    pub range: f32,
    pub special_cam: bool,
    pub target_damp: f32,
    pub vert_offset: f32,
}

impl Default for ThirdPersonSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            alpha: 1.0,
            angle: 0.0,
            camera_damp: 1.0,
            horz_offset: 0.0,
            pitch_offset: 0.0,
            range: 100.0,
            special_cam: false,
            target_damp: 1.0,
            vert_offset: 16.0,
        }
    }
}

/// Persistent globals used by OpenJK's third-person damp code.  OpenJK stores
/// these as cgame file statics; keeping them together makes the same lifetime
/// explicit in the Rust client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPersonCameraState {
    current_target: [f32; 3],
    current_location: [f32; 3],
    last_frame: f64,
    last_yaw: f32,
    stiff_factor: f32,
    last_camera_trace_fraction: f32,
    initialized: bool,
}

impl Default for ThirdPersonCameraState {
    fn default() -> Self {
        Self {
            current_target: [0.0; 3],
            current_location: [0.0; 3],
            last_frame: 0.0,
            last_yaw: 0.0,
            stiff_factor: 0.0,
            last_camera_trace_fraction: 0.0,
            initialized: false,
        }
    }
}

impl ThirdPersonCameraState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPersonViewInput {
    /// Player actor origin in native JKA coordinates (Z up).
    pub origin: [f32; 3],
    /// Client view angles in native JKA degrees: pitch, yaw, roll.
    pub view_angles: [f32; 3],
    pub view_height: i32,
    pub health: i32,
    pub dead_yaw: f32,
    pub client_num: i32,
    /// Presentation clock in milliseconds. Live/OpenJK callers may pass whole
    /// milliseconds; demo/spectator rendering may pass fractional values.
    pub time: f64,
    pub teleported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPersonViewOutput {
    /// Final camera origin in native JKA coordinates.
    pub origin: [f32; 3],
    /// Final camera angles in native JKA degrees.
    pub angles: [f32; 3],
    /// Render-only state that can rebuild the ordinary on-foot camera from a
    /// newer real mouse orientation without repeating BSP collision traces.
    /// This is only provided when camera damping has already resolved directly
    /// to the ideal location and the authoritative camera trace was unobstructed.
    pub late_latch: Option<ThirdPersonLateLatchView>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPersonLateLatchView {
    settings: ThirdPersonSettings,
    current_target: [f32; 3],
    ideal_target: [f32; 3],
}

#[inline]
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
fn length(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

#[inline]
fn normalize(a: [f32; 3]) -> ([f32; 3], f32) {
    let len = length(a);
    if len == 0.0 {
        ([0.0; 3], 0.0)
    } else {
        (scale(a, 1.0 / len), len)
    }
}

/// OpenJK/Q3 AngleVectors, retaining the native PITCH/YAW/ROLL convention.
fn angle_vectors(angles: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let pitch = angles[0].to_radians();
    let yaw = angles[1].to_radians();
    let roll = angles[2].to_radians();
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let (sr, cr) = roll.sin_cos();

    let forward = [cp * cy, cp * sy, -sp];
    let right = [
        -sr * sp * cy + -cr * -sy,
        -sr * sp * sy + -cr * cy,
        -sr * cp,
    ];
    let up = [
        cr * sp * cy + -sr * -sy,
        cr * sp * sy + -sr * cy,
        cr * cp,
    ];
    (forward, right, up)
}

/// OpenJK AnglesToAxis uses -right as axis[1].
fn angles_axis1(angles: [f32; 3]) -> [f32; 3] {
    let (_, right, _) = angle_vectors(angles);
    scale(right, -1.0)
}

/// OpenJK vectoangles, including its negative pitch convention.
fn vector_to_angles(value: [f32; 3]) -> [f32; 3] {
    let (yaw, pitch) = if value[0] == 0.0 && value[1] == 0.0 {
        let pitch = if value[2] > 0.0 { 90.0 } else { 270.0 };
        (0.0, pitch)
    } else {
        let mut yaw = value[1].atan2(value[0]).to_degrees();
        if yaw < 0.0 {
            yaw += 360.0;
        }
        let forward = (value[0] * value[0] + value[1] * value[1]).sqrt();
        let mut pitch = value[2].atan2(forward).to_degrees();
        if pitch < 0.0 {
            pitch += 360.0;
        }
        (yaw, pitch)
    };
    [-pitch, yaw, 0.0]
}

fn camera_trace_with_fraction<W: TraceWorld>(
    world: &mut W,
    start: [f32; 3],
    end: [f32; 3],
    pass_entity: i32,
) -> ([f32; 3], f32) {
    let mins = [-CAMERA_SIZE; 3];
    let maxs = [CAMERA_SIZE; 3];
    let trace = world.trace(TraceQuery {
        start,
        mins,
        maxs,
        end,
        pass_entity,
        mask: MASK_CAMERACLIP,
    });
    (trace.end, trace.fraction)
}

fn camera_trace<W: TraceWorld>(
    world: &mut W,
    start: [f32; 3],
    end: [f32; 3],
    pass_entity: i32,
) -> [f32; 3] {
    camera_trace_with_fraction(world, start, end, pass_entity).0
}

fn ideal_target(settings: ThirdPersonSettings, input: ThirdPersonViewInput) -> ([f32; 3], [f32; 3]) {
    // CG_CalcIdealThirdPersonViewTarget, ordinary on-foot path.
    let mut focus = input.origin;
    focus[2] += input.view_height as f32;
    let mut target = focus;
    target[2] += settings.vert_offset;
    (focus, target)
}

fn ideal_location(target: [f32; 3], forward: [f32; 3], range: f32) -> [f32; 3] {
    // CG_CalcIdealThirdPersonViewLocation, ordinary on-foot path.
    add(target, scale(forward, -range))
}

fn reset_third_person_damp<W: TraceWorld>(
    world: &mut W,
    settings: ThirdPersonSettings,
    state: &mut ThirdPersonCameraState,
    input: ThirdPersonViewInput,
    focus_angles: &mut [f32; 3],
) {
    // CG_ResetThirdPersonViewDamp.
    focus_angles[0] = focus_angles[0].clamp(-89.0, 89.0);
    let (forward, _, _) = angle_vectors(*focus_angles);
    let (focus, target) = ideal_target(settings, input);
    let location = ideal_location(target, forward, settings.range);

    state.current_target = camera_trace(world, focus, target, input.client_num);
    let (location, camera_fraction) =
        camera_trace_with_fraction(world, state.current_target, location, input.client_num);
    state.current_location = location;
    state.last_camera_trace_fraction = camera_fraction;
    state.last_frame = input.time;
    state.last_yaw = focus_angles[1];
    state.stiff_factor = 0.0;
    state.initialized = true;
}

fn update_target_damp<W: TraceWorld>(
    world: &mut W,
    settings: ThirdPersonSettings,
    state: &mut ThirdPersonCameraState,
    input: ThirdPersonViewInput,
) {
    // CG_UpdateThirdPersonTargetDamp, ordinary on-foot path.
    let (focus, ideal) = ideal_target(settings, input);
    if settings.target_damp >= 1.0 || input.teleported {
        state.current_target = ideal;
    } else if settings.target_damp >= 0.0 {
        let diff = sub(ideal, state.current_target);
        let damp = 1.0 - settings.target_damp;
        let dtime = (input.time - state.last_frame) as f32 / CAMERA_DAMP_INTERVAL;
        let ratio = damp.powf(dtime);
        state.current_target = add(ideal, scale(diff, -ratio));
    }
    state.current_target = camera_trace(world, focus, state.current_target, input.client_num);
}

fn update_camera_damp<W: TraceWorld>(
    world: &mut W,
    settings: ThirdPersonSettings,
    state: &mut ThirdPersonCameraState,
    input: ThirdPersonViewInput,
    focus_angles: [f32; 3],
    forward: [f32; 3],
) {
    // CG_UpdateThirdPersonCameraDamp, ordinary on-foot path.  OpenJK has a
    // special mover re-lerp/retrace branch after this trace; that branch must
    // wait for the client entity/mover collision layer instead of being faked.
    let (_, ideal_target) = ideal_target(settings, input);
    let ideal = ideal_location(ideal_target, forward, settings.range);
    let mut damp_factor = 0.0;
    if settings.camera_damp != 0.0 {
        let pitch = focus_angles[0].abs() / 115.0;
        damp_factor = (1.0 - settings.camera_damp) * (pitch * pitch) + settings.camera_damp;
        if state.stiff_factor > 0.0 {
            damp_factor += (1.0 - damp_factor) * state.stiff_factor;
        }
    }

    if damp_factor >= 1.0 || input.teleported {
        state.current_location = ideal;
    } else if damp_factor >= 0.0 {
        let diff = sub(ideal, state.current_location);
        let left = 1.0 - damp_factor;
        let dtime = (input.time - state.last_frame) as f32 / CAMERA_DAMP_INTERVAL;
        let ratio = left.powf(dtime);
        state.current_location = add(ideal, scale(diff, -ratio));
    }
    let (location, camera_fraction) = camera_trace_with_fraction(
        world,
        state.current_target,
        state.current_location,
        input.client_num,
    );
    state.current_location = location;
    state.last_camera_trace_fraction = camera_fraction;
}

/// Port of the ordinary, on-foot OpenJK MP CG_OffsetThirdPersonView camera
/// path. Vehicle overrides, the Rancor-held camera, and OpenJK's mover-specific
/// re-lerp/retrace are intentionally not approximated; they belong to their
/// corresponding entity systems when those are ported.
pub fn offset_third_person_view<W: TraceWorld>(
    world: &mut W,
    settings: ThirdPersonSettings,
    state: &mut ThirdPersonCameraState,
    input: ThirdPersonViewInput,
) -> ThirdPersonViewOutput {
    let mut focus_angles = input.view_angles;
    state.stiff_factor = 0.0;

    if input.health <= 0 {
        focus_angles[1] = input.dead_yaw;
    } else {
        focus_angles[1] += settings.angle;
        focus_angles[0] += settings.pitch_offset;
    }

    let reset_damp = !state.initialized || state.last_frame == 0.0 || state.last_frame > input.time;
    if reset_damp {
        reset_third_person_damp(world, settings, state, input, &mut focus_angles);
    } else {
        focus_angles[0] = focus_angles[0].clamp(-80.0, 80.0);
        let (forward, _, _) = angle_vectors(focus_angles);
        let mut delta_yaw = (focus_angles[1] - state.last_yaw).abs();
        if delta_yaw > 180.0 {
            delta_yaw = (delta_yaw - 360.0).abs();
        }
        let delta_time = input.time - state.last_frame;
        if delta_time > 0.0 {
            state.stiff_factor = delta_yaw / delta_time as f32;
            if state.stiff_factor < 1.0 {
                state.stiff_factor = 0.0;
            } else if state.stiff_factor > 2.5 {
                state.stiff_factor = 0.75;
            } else {
                state.stiff_factor = (state.stiff_factor - 1.0) * 0.5;
            }
        }
        state.last_yaw = focus_angles[1];
        update_target_damp(world, settings, state, input);
        update_camera_damp(world, settings, state, input, focus_angles, forward);
    }

    let (forward, _, _) = angle_vectors(focus_angles);
    let (mut aim, dist) = normalize(sub(state.current_target, state.current_location));
    if dist == 0.0 || aim[0] == 0.0 || aim[1] == 0.0 {
        aim = forward;
    }
    let angles = vector_to_angles(aim);

    // OpenJK applies cg_thirdPersonHorzOffset after calculating the final aim
    // angles, using viewaxis[1] from AnglesToAxis.
    if settings.horz_offset != 0.0 {
        state.current_location = add(
            state.current_location,
            scale(angles_axis1(angles), settings.horz_offset),
        );
    }

    state.last_frame = input.time;
    let (_, ideal_target) = ideal_target(settings, input);
    let late_latch = (!reset_damp
        && input.health > 0
        && !input.teleported
        && settings.camera_damp >= 1.0
        && state.last_camera_trace_fraction >= 0.999_999)
        .then_some(ThirdPersonLateLatchView {
            settings,
            current_target: state.current_target,
            ideal_target,
        });
    ThirdPersonViewOutput {
        origin: state.current_location,
        angles,
        late_latch,
    }
}

/// Rebuild only the angle-dependent, no-collision portion of the ordinary
/// OpenJK third-person camera. The authoritative camera call above has already
/// resolved target damping/target clipping for this presentation frame. This
/// helper is intentionally available only through `ThirdPersonLateLatchView`,
/// which is withheld while the camera is damped or collision-clipped.
pub fn late_latch_third_person_view(
    latch: ThirdPersonLateLatchView,
    view_angles: [f32; 3],
) -> ThirdPersonViewOutput {
    let mut focus_angles = view_angles;
    focus_angles[1] += latch.settings.angle;
    focus_angles[0] += latch.settings.pitch_offset;
    focus_angles[0] = focus_angles[0].clamp(-80.0, 80.0);

    let (forward, _, _) = angle_vectors(focus_angles);
    let mut location = ideal_location(latch.ideal_target, forward, latch.settings.range);
    let (mut aim, dist) = normalize(sub(latch.current_target, location));
    if dist == 0.0 || aim[0] == 0.0 || aim[1] == 0.0 {
        aim = forward;
    }
    let angles = vector_to_angles(aim);

    if latch.settings.horz_offset != 0.0 {
        location = add(
            location,
            scale(angles_axis1(angles), latch.settings.horz_offset),
        );
    }

    ThirdPersonViewOutput {
        origin: location,
        angles,
        late_latch: Some(latch),
    }
}

pub fn far_distance_for_cull(distance_cull: f32) -> f32 {
    if !distance_cull.is_finite() || distance_cull <= 0.0 {
        return DEFAULT_DISTANCE_CULL * DISTANCE_CULL_DIAGONAL;
    }
    (distance_cull * DISTANCE_CULL_DIAGONAL).max(MIN_FAR_DISTANCE)
}

/// OpenJK `MAX_SHAKE_INTENSITY`.
const MAX_SHAKE_INTENSITY: f32 = 16.0;

/// One active `CGCam_Shake`: linear falloff over `duration` from `start`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraShake {
    pub intensity: f32,
    pub start: std::time::Instant,
    pub duration: std::time::Duration,
}

impl CameraShake {
    pub fn new(intensity: f32, start: std::time::Instant, duration_ms: i32) -> Option<Self> {
        (intensity > 0.0 && duration_ms > 0).then(|| Self {
            intensity: intensity.min(MAX_SHAKE_INTENSITY),
            start,
            duration: std::time::Duration::from_millis(duration_ms as u64),
        })
    }

    pub fn expired(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.start) >= self.duration
    }

    /// Port of `CG_SE_UpdateShake`: a fresh random offset every frame, scaled by
    /// the remaining fraction. Roll is left alone, as in OpenJK.
    pub fn sample(&self, now: std::time::Instant, rng: &mut ShakeRng) -> Option<CameraShakeOffset> {
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed >= self.duration {
            return None;
        }
        let remaining = 1.0 - elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let intensity = self.intensity * remaining;
        let position = Vec3::new(rng.next_signed(), rng.next_signed(), rng.next_signed()) * intensity;
        Some(CameraShakeOffset {
            position,
            pitch: (rng.next_signed() * intensity).to_radians(),
            yaw: (rng.next_signed() * intensity).to_radians(),
        })
    }
}

/// Per-frame shake displacement in renderer space (position units, radians).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CameraShakeOffset {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// xorshift32 stand-in for `Q_flrand(-1, 1)`; shake needs no quality.
#[derive(Debug, Clone, Copy)]
pub struct ShakeRng(u32);

impl Default for ShakeRng {
    fn default() -> Self {
        Self(0x9E37_79B9)
    }
}

impl ShakeRng {
    fn next_signed(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Pending CGame screen shake. Not part of the base view: it is re-applied
    /// after every late-latch overwrite of position/yaw/pitch.
    pub shake: CameraShakeOffset,
    /// OpenJK/TaystJK `cg_fov`: horizontal degrees on the legacy 4:3 baseline.
    cg_fov: f32,
    near: f32,
    far: f32,
}
impl Camera {
    #[cfg(test)]
    pub fn new(position: [f32; 3], yaw: f32) -> Self {
        Self::new_with_fov(position, yaw, DEFAULT_CG_FOV)
    }

    pub fn new_with_fov(position: [f32; 3], yaw: f32, cg_fov: f32) -> Self {
        Self {
            position: Vec3::from_array(position),
            yaw,
            pitch: 0.0,
            shake: CameraShakeOffset::default(),
            cg_fov: cg_fov.clamp(MIN_CG_FOV, MAX_CG_FOV),
            near: 1.0,
            far: far_distance_for_cull(DEFAULT_DISTANCE_CULL),
        }
    }

    /// Apply JKA worldspawn distanceCull semantics. The authored distance is a
    /// visibility/culling distance, not the literal projection far plane.
    pub fn set_distance_cull(&mut self, distance_cull: f32) {
        self.far = far_distance_for_cull(distance_cull);
    }

    pub fn set_cg_fov(&mut self, cg_fov: f32) {
        self.cg_fov = cg_fov.clamp(MIN_CG_FOV, MAX_CG_FOV);
    }

    /// CGame-only presentation override used by TaystJK's held +zoom. Unlike
    /// archived cg_fov, TaystJK permits the temporary zoom FOV up to 176.
    pub fn set_presentation_fov(&mut self, cg_fov: f32) {
        self.cg_fov = cg_fov.clamp(1.0, 176.0);
    }

    pub fn cg_fov(&self) -> f32 {
        self.cg_fov
    }

    /// Add the pending shake to a freshly assigned base view (position + angles).
    pub fn apply_shake(&mut self) {
        self.position += self.shake.position;
        self.apply_shake_angles();
    }

    /// Angles only: for late-latch paths that leave the base position untouched.
    pub fn apply_shake_angles(&mut self) {
        self.yaw += self.shake.yaw;
        self.pitch += self.shake.pitch;
    }

    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(cy * cp, sp, -sy * cp).normalize_or_zero()
    }

    /// Port of OpenJK/TaystJK `CG_CalcFov` with aspect adjustment permanently
    /// enabled. `cg_fov` is authored as horizontal FOV on the legacy 4:3
    /// baseline; widescreen expands horizontally (Hor+) while preserving the
    /// corresponding vertical framing.
    pub fn fov_y_for_viewport(&self, width: u32, height: u32) -> f32 {
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        let aspect = width / height;
        let desired_fov_x = self.cg_fov.to_radians();
        let base_aspect = 0.75_f32; // TaystJK/OpenJK: 3 / 4.
        let fov_x = 2.0 * ((desired_fov_x * 0.5).tan() * base_aspect * aspect).atan();

        2.0 * ((height / width) * (fov_x * 0.5).tan()).atan()
    }

    pub fn frustum_corners(
        &self,
        width: u32,
        height: u32,
        near_distance: f32,
        far_distance: f32,
    ) -> [Vec3; 8] {
        let fov_y = self.fov_y_for_viewport(width, height);
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        let aspect = width / height;
        let tan_half_fov = (fov_y * 0.5).tan();
        let forward = self.forward();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        let mut corners = [Vec3::ZERO; 8];
        for (plane, distance) in [near_distance.max(self.near), far_distance.min(self.far)]
            .into_iter()
            .enumerate()
        {
            let centre = self.position + forward * distance;
            let half_height = tan_half_fov * distance;
            let half_width = half_height * aspect;
            let base = plane * 4;
            corners[base] = centre - right * half_width - up * half_height;
            corners[base + 1] = centre + right * half_width - up * half_height;
            corners[base + 2] = centre + right * half_width + up * half_height;
            corners[base + 3] = centre - right * half_width + up * half_height;
        }
        corners
    }

    pub fn view_projection(&self, width: u32, height: u32) -> Mat4 {
        self.view_projection_jittered(width, height, [0.0, 0.0])
    }

    /// Projects a renderer-space (post `scene::render_position`) world point to
    /// screen-space pixel coordinates, or `None` when it's behind the camera
    /// or outside the near/far range. Shared by the map editor's egui overlay
    /// and the `r_drawEntities` label painter.
    pub fn project_to_screen(&self, width: u32, height: u32, render_point: Vec3) -> Option<Pos2> {
        let view_proj = self.view_projection(width, height);
        let clip = view_proj * render_point.extend(1.0);
        if clip.w <= 0.001 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.z < -0.1 || ndc.z > 1.1 {
            return None;
        }
        Some(Pos2::new(
            (ndc.x * 0.5 + 0.5) * width as f32,
            (1.0 - (ndc.y * 0.5 + 0.5)) * height as f32,
        ))
    }

    pub fn view_projection_jittered(
        &self,
        width: u32,
        height: u32,
        jitter_pixels: [f32; 2],
    ) -> Mat4 {
        let fov_y = self.fov_y_for_viewport(width, height);
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        let aspect = width / height;
        let view = Mat4::look_to_rh(self.position, self.forward(), Vec3::Y);
        // Camera depth is permanently reversed-Z: near -> 1, far -> 0.
        // Shadow maps keep their own conventional depth convention.
        let projection = Mat4::perspective_rh(fov_y, aspect, self.far, self.near);
        let jitter = Mat4::from_translation(Vec3::new(
            2.0 * jitter_pixels[0] / width,
            -2.0 * jitter_pixels[1] / height,
            0.0,
        ));
        jitter * projection * view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_shake_falls_off_and_expires() {
        let start = std::time::Instant::now();
        let shake = CameraShake::new(100.0, start, 1000).unwrap();
        assert_eq!(shake.intensity, MAX_SHAKE_INTENSITY);
        let mut rng = ShakeRng::default();
        let early = shake.sample(start, &mut rng).unwrap();
        assert!(early.position.abs().max_element() <= MAX_SHAKE_INTENSITY);
        assert!(early.position != Vec3::ZERO);
        assert!(shake.sample(start + std::time::Duration::from_millis(1000), &mut rng).is_none());
        assert!(shake.expired(start + std::time::Duration::from_millis(1000)));
        assert!(CameraShake::new(0.0, start, 1000).is_none());
        assert!(CameraShake::new(4.0, start, 0).is_none());
    }

    #[test]
    fn jka_distance_cull_expands_to_far_plane() {
        assert!((far_distance_for_cull(6000.0) - 10392.305).abs() < 0.01);
        assert!((far_distance_for_cull(24000.0) - 41569.22).abs() < 0.05);
    }

    #[test]
    fn jka_distance_cull_keeps_minimum_far_plane() {
        assert_eq!(far_distance_for_cull(100.0), MIN_FAR_DISTANCE);
    }

    #[test]
    fn taystjk_aspect_adjust_is_hor_plus() {
        let camera = Camera::new_with_fov([0.0, 0.0, 0.0], 0.0, 90.0);
        let fov_y_4_3 = camera.fov_y_for_viewport(640, 480);
        let fov_y_16_9 = camera.fov_y_for_viewport(1920, 1080);

        // Aspect-adjusted cg_fov preserves the 4:3 vertical framing while
        // widening the horizontal FOV on widescreen displays.
        assert!((fov_y_4_3 - fov_y_16_9).abs() < 1e-6);
        let widescreen_fov_x =
            2.0 * ((fov_y_16_9 * 0.5).tan() * (1920.0_f32 / 1080.0)).atan();
        assert!(widescreen_fov_x.to_degrees() > 90.0);
    }

    #[test]
    fn reversed_z_maps_near_to_one_and_far_to_zero() {
        let camera = Camera::new([0.0, 0.0, 0.0], 0.0);
        let projection = camera.view_projection(1280, 720);
        let view = Mat4::look_to_rh(camera.position, camera.forward(), Vec3::Y);
        let projection_only = projection * view.inverse();

        let near_clip = projection_only * Vec3::new(0.0, 0.0, -camera.near).extend(1.0);
        let far_clip = projection_only * Vec3::new(0.0, 0.0, -camera.far).extend(1.0);
        let near_depth = near_clip.z / near_clip.w;
        let far_depth = far_clip.z / far_clip.w;

        assert!((near_depth - 1.0).abs() < 1e-5);
        assert!(far_depth.abs() < 1e-5);
    }
}
