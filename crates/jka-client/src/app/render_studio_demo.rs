//! Studio-owned DM26 playback and map lifetimes. Gameplay's session, VFS game
//! directory, map request generation and renderer world are never replaced.
use super::render_studio::{StudioDemoPreview, StudioVideoFormat};
use super::render_studio_timeline::{
    studio_follow_move_position, StudioCameraKey, StudioCameraSource, StudioDemoClip,
    StudioDemoPov, StudioDemoAnchor, StudioFollowFallback, StudioFollowMoveAim, StudioFollowPerspective,
};
use super::{race_ghost, scene, App, FrontendPage, GameSession, Instant, RenderCommand};
use crate::renderer::{StudioSceneFrame, StudioSourceFrame};
use std::{path::PathBuf, sync::Arc};

#[path = "render_studio_source_pool.rs"]
mod source_pool;

#[cfg(all(test, windows))]
#[path = "render_studio_demo_tests.rs"]
mod tests;

pub(in crate::app) const STUDIO_REQUEST_BIT: u64 = 1 << 63;
/// Comparison ghosts other than the followed runner draw at their overlay
/// opacity times this, so the default reads as a faint companion.
const COMPARISON_GHOST_ALPHA: f32 = 0.6;

/// Only one CPU load runs at a time, including cancelled loads. Do not detach
/// work on every scrub/project edit and accumulate unbounded asset builders.
pub(in crate::app) struct StudioSourceJob {
    pub id: u64,
    pub cancelled: bool,
    pub index: usize,
    pub name: String,
    pub game: String,
    pub progress: Arc<crate::runtime::LoadProgress>,
    /// Seeking uses the retained scene; it must never upload the map again.
    pub upload: bool,
    pub worker: std::thread::JoinHandle<Result<StudioDemoSource, String>>,
}

pub(in crate::app) struct StudioDemoSource {
    pub id: u64,
    pub progress: Arc<crate::runtime::LoadProgress>,
    /// Current borrower of this recording-owned context; not its cache key.
    pub clip_index: usize,
    pub name: String,
    pub game: String,
    pub game_dir: Option<PathBuf>,
    pub playback: GameSession,
    pub prepared: Option<super::PreparedMapCache>,
    pub race_map: Option<Arc<crate::ui::StudioRaceMap>>,
    pub race_map_job: Option<std::thread::JoinHandle<Arc<crate::ui::StudioRaceMap>>>,
    pub race_map_runners: Vec<(String, String)>,
    pub race_map_reference: usize,
    pub collision: Option<jka_movement::CollisionWorld>,
    pub gpu_ready: bool,
    pub failed: bool,
    pub revision: u64,
    pub last_sample_ms: Option<i32>,
    last_viewed_client: Option<i32>,
    pub shader_remaps: scene::ShaderRemapState,
    /// Last complete race segment, selected by the same parser as `/rGhost`.
    pub race_start_server_ms: Option<i32>,
    /// Full-precision server clock for exact race phase sampling (never f32).
    pub last_presentation_server_ms: Option<i32>,
    /// One native CGame mixer per demo source during offline Studio export.
    pub audio_capture: Option<rodio::mixer::MixerSource>,
    pub race_audio_state: Option<(usize,bool,f64)>,
    /// Per-recording jaPRO speedometer history, never shared across demos.
    pub studio_speedometer: crate::speedometer::Speedometer,
    wallrun_camera_yaw: std::collections::HashMap<i32, super::wallrun_camera::WallrunCameraYaw>,
    wallrun_camera_mode: u8,
}

/// Decoded on a worker. Only the player track is retained: never load a
/// second map, cgame, physics world or scene renderer for a composite actor.
pub(in crate::app) struct StudioRaceRuntime {
    pub key: (String, String),
    pub worker: Option<std::thread::JoinHandle<Result<race_ghost::RaceGhostTrack, String>>>,
    /// Keep a parsed track until the playhead reaches its matching map.
    pub pending_track: Option<race_ghost::RaceGhostTrack>,
    pub ghost: Option<race_ghost::RaceGhost>,
    pub error: Option<String>,
}

// One rendered demo actor, including the native inputs for OpenJK's chase
// camera and the player-facing basis for cinematic local offsets.
#[derive(Clone,Copy)]
struct StudioDemoActor {
    origin:[f32;3], eye:[f32;3], angles:[f32;3], velocity:[f32;3],
    view_height:i32, health:i32, dead_yaw:f32, legs_anim:i32,
}

impl StudioDemoSource {
    fn demo_actor(&self, sample:super::DemoCameraSample, client:i32)->Option<StudioDemoActor> {
        if client==sample.client_num {
            Some(StudioDemoActor {origin:sample.native_origin,eye:sample.eye_position,
                angles:sample.view_angles,velocity:sample.velocity,view_height:sample.view_height,
                health:sample.policy.health,dead_yaw:sample.dead_yaw,
                legs_anim:sample.policy.legs_anim})
        } else {
            let view=self.playback.demo_fake_view(client)?;
            Some(StudioDemoActor {origin:view.native_origin,eye:view.eye_position,
                angles:view.view_angles,velocity:view.velocity,view_height:26,
                health:view.health,dead_yaw:view.view_angles[1],legs_anim:view.legs_anim})
        }
    }

    fn recorded_wallrun_yaw(&mut self, sample: super::DemoCameraSample) -> f32 {
        self.playback.demo_index.as_ref().and_then(|index| index.wallrun_camera_yaw(
            sample.presentation_time_ms, sample.policy.legs_anim, sample.client_num,
            self.wallrun_camera_mode,
        )).unwrap_or_else(|| self.wallrun_camera_yaw.entry(sample.client_num).or_default().apply(
            sample.view_angles[1], sample.policy.legs_anim, sample.client_num,
            sample.presentation_time_ms, sample.teleported, self.wallrun_camera_mode,
        ))
    }

    /// Offsets use renderer coordinates (X forward, Y up, Z right for a player
    /// facing yaw=0). Optional actor-local mode rotates X/Z with the player;
    /// it is not claimed to be bone-local, since Ghoul2 bolt local axes are
    /// not required for the two independent anchor points.
    fn anchor_point(&self,sample:super::DemoCameraSample,client:i32,
        anchor:StudioDemoAnchor,offset:[f32;3],local:bool)->Option<glam::Vec3> {
        let actor=self.demo_actor(sample,client)?;
        let base=match anchor {
            StudioDemoAnchor::Eyes=>glam::Vec3::from_array(actor.eye),
            StudioDemoAnchor::Origin=>glam::Vec3::from_array(scene::render_position(actor.origin)),
            _=>glam::Vec3::from_array(self.playback.player_presenter
                .studio_demo_bone(client,anchor.bone_name()?)
                // Missing custom-model bolts or unposed demo entities should
                // not make the entire shot vanish. Eyes remain a safe fallback.
                .unwrap_or(actor.eye)),
        };
        let delta=glam::Vec3::from_array(offset);
        let delta=if local {
            let (sn,cs)=actor.angles[1].to_radians().sin_cos();
            glam::Vec3::new(cs*delta.x+sn*delta.z,delta.y,-sn*delta.x+cs*delta.z)
        }else{delta};
        Some(base+delta)
    }

    fn aim_camera(camera:&mut crate::camera::Camera,target:glam::Vec3) {
        let direction=target-camera.position;
        if direction.length_squared()>0.0001 {
            camera.yaw=(-direction.z).atan2(direction.x);
            camera.pitch=direction.y.atan2(direction.x.hypot(direction.z));
        }
    }

    fn clamp_camera(&mut self,anchor:glam::Vec3,camera:&mut crate::camera::Camera) {
        use jka_movement::TraceWorld;
        let Some(world)=self.collision.as_mut() else {return;};
        let trace=world.trace(jka_movement::TraceQuery {
            start:scene::jka_position(anchor.to_array()),
            end:scene::jka_position(camera.position.to_array()),
            mins:[-8.;3],maxs:[8.;3],pass_entity:-1,
            mask:0x1|0x10|0x1000,
        });
        if trace.start_solid==0 && trace.all_solid==0 && trace.fraction<1.0 {
            camera.position=anchor.lerp(camera.position,(trace.fraction-0.03).max(0.0));
        }
    }

    fn camera_for_key(&mut self,sample:super::DemoCameraSample,key:StudioCameraKey)
        ->Option<crate::camera::Camera> {
        let key=resolve_recorded_subject(key,sample.client_num);
        let recorded_yaw = self.recorded_wallrun_yaw(sample);
        let mut camera=crate::camera::Camera::new_with_fov(
            sample.eye_position,recorded_yaw.to_radians(),key.fov);
        camera.pitch=-sample.view_angles[0].to_radians();
        match key.source {
            StudioCameraSource::Demo{pov:StudioDemoPov::Free,client}=>{
                camera.position=glam::Vec3::from_array(key.position);
                camera.yaw=key.yaw.to_radians();camera.pitch=key.pitch.to_radians();
                // World-anchored Director shots (pan, fly-by, crane) stay put
                // and turn to keep the subject in frame.
                let rig=key.demo_follow;
                if rig.aim_enabled && key.focus_target.is_none() {
                    let subject=if rig.aim_client>=0 {rig.aim_client}
                        else if client>=0 {client} else {sample.client_num};
                    if let Some(aim)=self.anchor_point(sample,subject,rig.aim_anchor,rig.aim_offset,rig.aim_local) {
                        Self::aim_camera(&mut camera,aim);
                    }
                }
            }
            StudioCameraSource::Demo{pov:StudioDemoPov::Follow,client}=>{
                let mut actor=self.demo_actor(sample,client)?;
                actor.angles[1] = if client == sample.client_num {
                    recorded_yaw
                } else {
                    self.wallrun_camera_yaw.entry(client).or_default().apply(
                        actor.angles[1], actor.legs_anim, client,
                        sample.presentation_time_ms, sample.teleported, self.wallrun_camera_mode,
                    )
                };
                if !self.visible_from_recorded(sample,actor.eye){return None;}
                let rig=key.demo_follow;
                let anchor=self.anchor_point(sample,client,rig.position_anchor,
                    rig.position_offset,rig.position_local)?;
                camera.position=anchor;
                camera.yaw=(actor.angles[1]+rig.yaw_offset).to_radians();
                camera.pitch=-(actor.angles[0]+rig.pitch_offset).to_radians();
                match rig.perspective {
                    StudioFollowPerspective::FirstPerson=>{
                        // Victim blade view: on the swinger's blade line, out at
                        // this player's distance from the blade.
                        if rig.blade_view && rig.aim_client>=0 {
                            if let Some((base,tip))=self.playback.player_presenter.studio_demo_blade(rig.aim_client) {
                                let (base,tip)=(glam::Vec3::from_array(base),glam::Vec3::from_array(tip));
                                let dir=(tip-base).normalize_or_zero();
                                if dir!=glam::Vec3::ZERO {
                                    let reach=(anchor-base).length().clamp(16.0,2048.0);
                                    camera.position=base+dir*reach;
                                }
                            }
                        }
                    },
                    StudioFollowPerspective::ThirdPerson=>{
                        if let Some(world)=self.collision.as_mut(){
                            let mut settings=crate::camera::ThirdPersonSettings::default();
                            settings.enabled=true;settings.range=rig.range;
                            settings.angle=if rig.motion_direction {-1.0}else{rig.yaw_offset};
                            settings.pitch_offset=rig.pitch_offset;
                            settings.vert_offset=rig.vertical_offset;
                            settings.horz_offset=rig.horizontal_offset;
                            settings.camera_damp=1.0;settings.target_damp=1.0;
                            settings.special_cam=false;
                            let view=crate::camera::offset_third_person_view(world,settings,
                                &mut crate::camera::ThirdPersonCameraState::default(),
                                crate::camera::ThirdPersonViewInput {
                                    origin:actor.origin,view_angles:actor.angles,
                                    velocity:actor.velocity,view_height:actor.view_height,
                                    health:actor.health.max(1),dead_yaw:actor.dead_yaw,
                                    client_num:client,time:sample.presentation_time_ms,
                                    teleported:true,
                                });
                            // Match the legacy OpenJK result by default, but
                            // translate it to the authored anatomical anchor.
                            camera.position=glam::Vec3::from_array(scene::render_position(view.origin))
                                +anchor-glam::Vec3::from_array(actor.eye);
                            camera.yaw=view.angles[1].to_radians();
                            camera.pitch=-view.angles[0].to_radians();
                            if rig.position_anchor!=StudioDemoAnchor::Eyes ||
                                rig.position_offset!=[0.;3] {
                                self.clamp_camera(anchor,&mut camera);
                            }
                        }
                    }
                    StudioFollowPerspective::Orbit=>{
                        let yaw=(if rig.orbit_world_angles {rig.yaw_offset}
                            else{actor.angles[1]+rig.yaw_offset}).to_radians();
                        let pitch=rig.pitch_offset.to_radians();
                        let (sy,cy)=yaw.sin_cos();let (sp,cp)=pitch.sin_cos();
                        let forward=glam::Vec3::new(cy*cp,-sp,-sy*cp);
                        let right=glam::Vec3::new(sy,0.,cy);
                        camera.position=anchor-forward*rig.range
                            +glam::Vec3::Y*rig.vertical_offset+right*rig.horizontal_offset;
                        // Chase from target: behind the anchor as seen from the
                        // Look-at player, so the anchor stays in the foreground.
                        if rig.chase_aim && rig.aim_enabled {
                            let aim_client=if rig.aim_client<0 {client}else{rig.aim_client};
                            if let Some(aim)=self.anchor_point(sample,aim_client,rig.aim_anchor,rig.aim_offset,rig.aim_local) {
                                let away=(anchor-aim).normalize_or_zero();
                                if away!=glam::Vec3::ZERO {
                                    camera.position=anchor+away*rig.range+glam::Vec3::Y*rig.vertical_offset
                                        +right*rig.horizontal_offset;
                                }
                            }
                        }
                        if rig.orbit_collision {self.clamp_camera(anchor,&mut camera);}
                        Self::aim_camera(&mut camera,anchor);
                    }
                }
                if rig.aim_enabled {
                    let aim_client=if rig.aim_client<0 {client}else{rig.aim_client};
                    if let Some(aim)=self.anchor_point(sample,aim_client,rig.aim_anchor,
                        rig.aim_offset,rig.aim_local) {
                        if self.visible_from_recorded(sample,aim.to_array()) {
                            Self::aim_camera(&mut camera,aim);
                        }
                    }
                }
            }
            _=>{}
        }
        self.visible_from_recorded(sample,camera.position.to_array()).then_some(camera)
    }

    fn visible_from_recorded(&self, sample: super::DemoCameraSample, point: [f32; 3]) -> bool {
        let Some(v) = self
            .prepared
            .as_ref()
            .and_then(|p| p.map.visibility.as_ref())
        else {
            return true;
        };
        let mut eye = sample.native_origin;
        eye[2] += sample.view_height as f32;
        let target = scene::jka_position(point);
        let Some(cluster) = v.cluster_at(target) else {
            return false;
        };
        if !v.visible(v.cluster_at(eye), &[cluster]) {
            return false;
        }
        if let (Some(area), Some(mask)) = (v.area_at(target), self.playback.demo_area_mask()) {
            if area < 256 && mask[area / 8] & (1 << (area % 8)) != 0 {
                return false;
            }
        }
        true
    }

    fn follow_eye(
        &self,
        sample: super::DemoCameraSample,
        key: StudioCameraKey,
    ) -> Option<glam::Vec3> {
        let key = resolve_recorded_subject(key, sample.client_num);
        let StudioCameraSource::Demo {
            pov: StudioDemoPov::Follow,
            client,
        } = key.source
        else {
            return None;
        };
        let rig=key.demo_follow;
        let (target,anchor,offset,local)=if rig.aim_enabled {
            (if rig.aim_client<0 {client}else{rig.aim_client},rig.aim_anchor,
                rig.aim_offset,rig.aim_local)
        }else{(client,StudioDemoAnchor::Eyes,[0.;3],false)};
        let point=self.anchor_point(sample,target,anchor,offset,local)?;
        self.visible_from_recorded(sample,point.to_array()).then_some(point)
    }

    /// The point a shot is about: aim target or followed eyes (autofocus).
    fn subject_point(&self,sample:super::DemoCameraSample,key:StudioCameraKey)->Option<glam::Vec3> {
        match resolve_recorded_subject(key,sample.client_num).source {
            StudioCameraSource::Demo{pov:StudioDemoPov::Follow,..}=>self.follow_eye(sample,key),
            StudioCameraSource::Demo{pov:StudioDemoPov::Free,client} if key.demo_follow.aim_enabled=>{
                let rig=key.demo_follow;
                let subject=if rig.aim_client>=0 {rig.aim_client} else if client>=0 {client} else {sample.client_num};
                self.anchor_point(sample,subject,rig.aim_anchor,rig.aim_offset,rig.aim_local)
            }
            // Fixed framing (look-at point) but autofocus: the shot's player.
            StudioCameraSource::Demo{pov:StudioDemoPov::Free,client}=>{
                let subject=if client>=0 {client} else {sample.client_num};
                self.anchor_point(sample,subject,StudioDemoAnchor::Eyes,[0.;3],false)
            }
            _=>None,
        }
    }

    fn fallback_camera(
        &mut self,
        sample: super::DemoCameraSample,
        key: StudioCameraKey,
    ) -> crate::camera::Camera {
        if key.demo_follow.fallback == StudioFollowFallback::KeyedCamera
            && self.visible_from_recorded(sample, key.position)
        {
            return super::render_studio_spatial::camera_from_key(key);
        }
        if key.demo_follow.perspective != StudioFollowPerspective::FirstPerson {
            let selected=studio_viewed_client(key,sample.client_num).unwrap_or(sample.client_num);
            for client in [selected,sample.client_num] {
                let mut safe=key;
                safe.source=StudioCameraSource::Demo{pov:StudioDemoPov::Follow,client};
                safe.focus_target=None;
                safe.demo_follow=Default::default();
                safe.demo_follow.perspective=StudioFollowPerspective::ThirdPerson;
                safe.demo_follow.range=120.0;
                safe.demo_follow.pitch_offset=10.0;
                if let Some(camera)=self.camera_for_key(sample,safe) {
                    self.last_viewed_client=Some(client);
                    return camera;
                }
            }
        }
        self.last_viewed_client=Some(sample.client_num);
        let mut camera = crate::camera::Camera::new_with_fov(
            sample.eye_position,
            self.recorded_wallrun_yaw(sample).to_radians(),
            key.fov,
        );
        camera.pitch = -sample.view_angles[0].to_radians();
        camera
    }

    /// Test rendered actor positions, projection and BSP line of sight. PVS
    /// alone allows a camera staring at the wrong side of a nearby pillar.
    fn director_subject_visible(&mut self, sample: super::DemoCameraSample, client: i32, camera: &crate::camera::Camera) -> bool {
        use jka_movement::TraceWorld;
        let Some(actor) = self.demo_actor(sample, client) else { return false };
        for height in [12.0, 48.0] {
            let mut point = actor.origin; point[2] += height;
            let rendered = glam::Vec3::from_array(scene::render_position(point));
            let Some(p) = camera.project_to_screen(1280, 960, rendered) else { return false };
            if p.x < 64.0 || p.x > 1216.0 || p.y < 48.0 || p.y > 912.0 { return false; }
            if let Some(world) = self.collision.as_mut() {
                let trace = world.trace(jka_movement::TraceQuery {
                    start: scene::jka_position(camera.position.to_array()), end: point,
                    mins: [0.0; 3], maxs: [0.0; 3], pass_entity: -1, mask: 0x1 | 0x10 | 0x1000,
                });
                if trace.start_solid != 0 || trace.all_solid != 0 || trace.fraction < 0.995 { return false; }
            }
        }
        true
    }

    fn guard_director_camera(&mut self, sample: super::DemoCameraSample, key: StudioCameraKey, mut camera: crate::camera::Camera) -> crate::camera::Camera {
        let selected = studio_viewed_client(key, sample.client_num).unwrap_or(sample.client_num);
        if self.director_subject_visible(sample, selected, &camera) { self.last_viewed_client = Some(selected); return camera; }
        if let Some(actor) = self.demo_actor(sample, selected) {
            let mut point = actor.origin; point[2] += 28.0;
            Self::aim_camera(&mut camera, glam::Vec3::from_array(scene::render_position(point)));
            if self.director_subject_visible(sample, selected, &camera) { self.last_viewed_client = Some(selected); return camera; }
        }
        // A deterministic outside rescue, evaluated from this source frame.
        // It has no history and behaves identically while scrubbing/exporting.
        for client in [selected, sample.client_num] {
            let Some(actor) = self.demo_actor(sample, client) else { continue };
            let mut point = actor.origin; point[2] += 28.0;
            let target = glam::Vec3::from_array(scene::render_position(point));
            for angle in [0.0f32, -35.0, 35.0, -75.0, 75.0, 180.0] {
                for range in [200.0f32, 140.0, 90.0] {
                    let yaw = (actor.angles[1] + angle).to_radians();
                    let position = [point[0] - yaw.cos() * range, point[1] - yaw.sin() * range, point[2] + 40.0];
                    let mut safe = crate::camera::Camera::new_with_fov(scene::render_position(position), yaw, key.fov.max(90.0));
                    self.clamp_camera(glam::Vec3::from_array(actor.eye), &mut safe);
                    if safe.position.distance(target) < 48.0 { continue; }
                    Self::aim_camera(&mut safe, target);
                    if self.visible_from_recorded(sample, safe.position.to_array())
                        && self.director_subject_visible(sample, client, &safe) {
                        self.last_viewed_client = Some(client);
                        return safe;
                    }
                }
            }
        }
        self.fallback_camera(sample, key)
    }

    fn follow_camera(
        &mut self,
        sample: super::DemoCameraSample,
        key: StudioCameraKey,
        backup: StudioCameraKey,
        from: Option<(StudioCameraKey, f32)>,
    ) -> crate::camera::Camera {
        let Some(mut camera) = self.camera_for_key(sample, key) else {
            return self.fallback_camera(sample, if key.demo_follow.fallback==StudioFollowFallback::KeyedCamera {backup}else{key});
        };
        if let Some((previous, alpha)) = from {
            if let Some(a) = self.camera_for_key(sample, previous) {
                let incoming = camera.position;
                camera.position = studio_follow_move_position(
                    a.position,
                    camera.position,
                    alpha,
                    key.demo_follow.move_path,
                    key.demo_follow.move_arc,
                );
                let delta = (camera.yaw - a.yaw + std::f32::consts::PI)
                    .rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                camera.yaw = a.yaw + delta * alpha;
                camera.pitch = a.pitch + (camera.pitch - a.pitch) * alpha;
                camera.set_presentation_fov(a.cg_fov() + (camera.cg_fov() - a.cg_fov()) * alpha);
                if let (Some(a), Some(b)) = (
                    self.follow_eye(sample, previous),
                    self.follow_eye(sample, key),
                ) {
                    let target = a.lerp(b, alpha);
                    if key.demo_follow.avoid_walls && alpha > 0.0 && alpha < 1.0 {
                        use jka_movement::TraceWorld;
                        if let Some(world) = self.collision.as_mut() {
                            let trace = world.trace(jka_movement::TraceQuery {
                                start: scene::jka_position(target.to_array()),
                                end: scene::jka_position(camera.position.to_array()),
                                mins: [-4.0; 3],
                                maxs: [4.0; 3],
                                pass_entity: -1,
                                mask: 0x1 | 0x10 | 0x1000,
                            });
                            camera.position = if trace.start_solid != 0 || trace.all_solid != 0 {
                                incoming
                            } else {
                                target.lerp(
                                    camera.position,
                                    if trace.fraction < 1.0 {
                                        (trace.fraction - 0.015).max(0.0)
                                    } else {
                                        1.0
                                    },
                                )
                            };
                        }
                    }
                    if key.demo_follow.move_aim == StudioFollowMoveAim::LookAtSubjects {
                        let direction = target - camera.position;
                        if direction.length_squared() > 256.0 {
                            let yaw = (-direction.z).atan2(direction.x);
                            let pitch = direction.y.atan2(direction.x.hypot(direction.z));
                            let weight = 4.0
                                * alpha
                                * (1.0 - alpha)
                                * ((direction.length() - 16.0) / 48.0).clamp(0.0, 1.0);
                            camera.yaw += ((yaw - camera.yaw + std::f32::consts::PI)
                                .rem_euclid(std::f32::consts::TAU)
                                - std::f32::consts::PI)
                                * weight;
                            camera.pitch += (pitch - camera.pitch) * weight;
                        }
                    }
                }
            }
        }
        if self.visible_from_recorded(sample, camera.position.to_array()) {
            camera
        } else {
            self.fallback_camera(sample, if key.demo_follow.fallback==StudioFollowFallback::KeyedCamera {backup}else{key})
        }
    }

    fn sample(
        &mut self,
        clip: &StudioDemoClip,
        time: f64,
        key: StudioCameraKey,
        from_key: Option<(StudioCameraKey, f32)>,
        backup_key: StudioCameraKey,
        video: &crate::ui::VideoSettings,
        third_person: crate::camera::ThirdPersonSettings,
        spectator: crate::camera::SpectatorCameraSettings,
        composite_entity_numbers: &[u16],
        playing: bool,
        capturing: bool,
        scene_view:bool,
        guard_subject: bool,
        fpls: bool,
        now: Instant,
    ) -> Result<StudioSourceFrame, String> {
        let target = clip.source_time_ms(time);
        let session = &mut self.playback;
        session.player_presenter.studio_fpls = fpls;
        session.studio_external_player_entities.clear();
        session.studio_external_player_entities.extend_from_slice(composite_entity_numbers);
        session.set_audio_suppressed(!playing || clip.comparison.is_some());
        let rate = super::render_studio_timeline::studio_speed_at(&clip.speed_keys,f64::from(target)/1000.0) as f32;
        let current = session.demo_elapsed_ms(now).unwrap_or(-1.0);
        // Monotonic sampling uses the existing decoder. Only a discontinuity
        // replays history; two live sources never reset or swap one another.
        let reverse = self.last_sample_ms.is_some_and(|last| target < last);
        if reverse
            || !current.is_finite()
            || current < 0.0
            || (f64::from(target) - current).abs() > 1000.0
        {
            session.seek_demo(now, target)?;
            // History-based readouts must not bridge non-contiguous demo times.
            self.studio_speedometer = Default::default();
            self.revision = self.revision.wrapping_add(1);
        }
        // Seeking restores the old demo rate and clears transient voices.
        // Install the Studio speed AFTER seeking; attach the offline source
        // after the initial seek so map music uses the correct gamestate.
        if capturing && self.audio_capture.is_none() {
            let sound=session.sound_presenter.as_mut().ok_or(
                "Demo sound presenter is unavailable (check game assets)")?;
            self.audio_capture=Some(sound.begin_offline_capture(&session.client_game)?);
        }
        if let Some(sound) = session.sound_presenter.as_mut() {
            sound.set_rate(if playing { rate.max(0.01) } else { 0.0 });
        }
        let first = session
            .demo_index
            .as_ref()
            .and_then(|i| i.first_active_server_time)
            .ok_or("Studio demo has no active timeline")?;
        if let Some(clock) = session.timeline.as_mut() {
            clock.seek(now, f64::from(first) + f64::from(target));
            clock.rate = 0.0;
        }
        self.last_sample_ms = Some(target);
        let follow = match key.source {
            // -1 (recorded player) is the snapshot's own viewer.
            StudioCameraSource::Demo {
                pov: StudioDemoPov::Follow,
                client,
            } if client >= 0 => Some(client),
            _ => None,
        };
        // Hide other players: keep the recorder, the shot's subject and its
        // aim target only.
        session.studio_visible_players = clip.hide_other_players.then(|| {
            let resolved = resolve_recorded_subject(key, session.demo_followed_client().unwrap_or(-1));
            let subject = match resolved.source {
                StudioCameraSource::Demo { client, .. } => client,
                _ => -1,
            };
            [session.demo_followed_client().unwrap_or(-1), subject, resolved.demo_follow.aim_client]
                .into_iter().filter_map(|c| u16::try_from(c).ok()).collect()
        });
        if clip.comparison.is_some() {
            // Every runner, including the reference, comes from the indexed
            // ghost group. Never leave a second native recorder body behind.
            session.studio_visible_players=Some(Vec::new());
        }
        session.client_game.set_presentation_viewer(follow.map_or(
            crate::cgame::PresentationViewer::Snapshot,
            crate::cgame::PresentationViewer::Client,
        ));
        session.demo_hidden_view_client = follow.filter(|_| {
            key.demo_follow.perspective == StudioFollowPerspective::FirstPerson
                && from_key.is_none()
        });
        // Only a follow rig placed outside the actor (or a free camera / move)
        // shows the viewer's own model. The recorded camera is always first
        // person, whatever rig settings the key still carries.
        let follow_outside = matches!(key.source, StudioCameraSource::Demo { pov: StudioDemoPov::Follow, .. })
            && key.demo_follow.perspective != StudioFollowPerspective::FirstPerson;
        session.demo_spectator_camera_mode = Some(
            if scene_view || follow_outside
                || matches!(
                    key.source,
                    StudioCameraSource::Demo {
                        pov: StudioDemoPov::Free,
                        ..
                    }
                )
                || from_key.is_some()
            {
                crate::camera::SpectatorCameraMode::ThirdPerson
            } else {
                crate::camera::SpectatorCameraMode::FirstPerson
            },
        );
        // Only capture the Ghoul2 poses actually requested by this camera or
        // the outgoing shot in a camera move; ordinary gameplay pays nothing.
        let mut requested=Vec::with_capacity(4);
        let recorded=session.demo_followed_client().unwrap_or(-1);
        for shot in std::iter::once(key).chain(from_key.map(|(shot,_)|shot)) {
            let shot=resolve_recorded_subject(shot,recorded);
            if let StudioCameraSource::Demo{pov:StudioDemoPov::Free,client}=shot.source {
                if shot.demo_follow.aim_enabled {
                    if let Some(name)=shot.demo_follow.aim_anchor.bone_name() {
                        let aim_client=if shot.demo_follow.aim_client>=0 {shot.demo_follow.aim_client}
                            else if client>=0 {client} else {recorded};
                        if aim_client>=0 {requested.push((aim_client,name));}
                    }
                }
            }
            if let StudioCameraSource::Demo{pov:StudioDemoPov::Follow,client}=shot.source {
                if let Some(name)=shot.demo_follow.position_anchor.bone_name() {
                    requested.push((client,name));
                }
                if shot.demo_follow.aim_enabled {
                    if let Some(name)=shot.demo_follow.aim_anchor.bone_name() {
                        let aim_client=if shot.demo_follow.aim_client<0 {client}
                            else{shot.demo_follow.aim_client};
                        requested.push((aim_client,name));
                    }
                }
                if shot.demo_follow.blade_view && shot.demo_follow.aim_client>=0 {
                    requested.push((shot.demo_follow.aim_client,crate::cgame::player_presenter::SABER_TIP_ANCHOR));
                }
            }
        }
        session.player_presenter.studio_capture_demo_bones(&requested);
        session
            .player_presenter
            .set_skinning_mode(video.ghoul2_skinning);
        session.player_presenter.set_lod_bias(video.ghoul2_lod_bias);
        session
            .entity_presenter
            .set_draw_map_models(video.draw_map_models);
        session
            .entity_presenter
            .set_entity_pvs_enabled(video.pvs_mode != crate::ui::PvsMode::Off);
        session
            .entity_presenter
            .set_md3_geometry_cache(video.md3_geometry_cache);
        let (_, sample) = session.advance(
            now,
            third_person,
            spectator,
            // cg_fpls (EternalJK mode 3: body surfaces off, saber kept) for
            // first-person views of the recorded player, as in gameplay.
            fpls,
            0,
            false,
            None,
            video.dynamic_lights == crate::ui::DynamicLightsMode::RayTracedHardware,
            video.player_blob_shadow_settings(),
        )?;
        self.last_viewed_client=studio_viewed_client(key,sample.client_num);
        let mut camera = if clip.comparison.is_some() {super::render_studio_spatial::camera_from_key(key)}
            else {self.follow_camera(sample, key, backup_key, from_key)};
        let mut lens=key.lens_for_camera(&mut camera);
        // A focus handle also aims the camera. Resolve it BEFORE validating
        // the final view, or it turns a rescued shot back toward a stale point.
        if guard_subject && !scene_view && key.demo_follow.perspective != StudioFollowPerspective::FirstPerson {
            camera = self.guard_director_camera(sample, key, camera);
        }
        if key.demo_follow.autofocus {
            let focus = if guard_subject { self.last_viewed_client.and_then(|client| self.demo_actor(sample, client))
                .map(|actor| glam::Vec3::from_array(actor.eye)) } else { self.subject_point(sample,key) };
            match focus {
                Some(point)=>lens.focus_distance=camera.position.distance(point).clamp(1.0,65536.0),
                // Never blur the frame around nobody: without a player to
                // focus on, this frame is rendered with deep focus.
                None=>lens.aperture=None,
            }
        }
        self.last_presentation_server_ms = Some(sample.presentation_time_ms.round() as i32);
        let mut view = crate::fx::draw::FxView::from_camera(&camera);
        view.seconds = sample.presentation_time_ms as f32 / 1000.0;
        let session = &mut self.playback;
        let presenter = &mut session.entity_presenter;
        let fx = match video.fx_geometry {
            crate::ui::FxGeometryMode::Cpu => {
                crate::fx::draw::tessellate(&session.fx_draws, &view, &mut |s| {
                    presenter.fx_material_stages(s)
                })
            }
            crate::ui::FxGeometryMode::CpuWorkers => {
                crate::fx::draw::tessellate_workers(&session.fx_draws, &view, &mut |s| {
                    presenter.fx_material_stages(s)
                })
            }
            crate::ui::FxGeometryMode::Gpu => {
                crate::fx::draw::tessellate_gpu_particles(&session.fx_draws, &view, &mut |s| {
                    presenter.fx_material_stages(s)
                })
            }
        };
        let mut models = Vec::with_capacity(session.dynamic_models.len() + fx.len());
        models.extend(session.dynamic_models.iter().cloned());
        models.extend(fx);
        Ok(StudioSourceFrame {
            shot_camera:camera,lens,environment:None,
            id: self.id,
            revision: self.revision,
            camera,
            time: sample.presentation_time_ms as f32 / 1000.0,
            player_position: session.player_position,
            area_mask: session.demo_area_mask(),
            dynamic_models: Arc::new(models),
            inline_models: session.inline_models.clone(),
            lights: session.fx_lights.clone(),
            screen_fx: session.screen_fx.clone(),
            cloud_foreground: session.fx_blades.clone(),
            ocean_actors: session.ocean_actors.clone(),
            hud: None,
        })
    }
}

impl App {
    pub(in crate::app) fn invalidate_studio_sources(&mut self) {
        if let Some(job) = self.render_studio.source_job.as_mut() { job.cancelled = true; }
        for job in &mut self.render_studio.source_prepares { job.cancelled = true; }
        let ids:Vec<_>=self.render_studio.sources.iter().map(|source|source.id).collect();
        self.render_studio.sources.clear();
        self.render_studio.failed_sources.clear();
        self.render_studio.scene_frame = None;
        self.render_studio.source_wait = None;
        // Do not evict the independent BSP backdrop when editing demo sources.
        for id in ids {self.render_command(RenderCommand::RemoveStudioSource(id));}
    }

    pub(in crate::app) fn reload_studio_gpu_sources(&mut self) {
        let mut commands = Vec::new();
        if let Some(world) = self.render_studio.bsp_world.as_mut() {
            if let Some(prepared) = world.prepared.as_ref() {
                world.ready = false;
                commands.push(RenderCommand::LoadStudioSource {
                    id: world.id, game: self.game.clone(),
                    map: Box::new(prepared.as_ref().clone()), video: self.video,
                    progress: std::sync::Arc::clone(&world.progress),
                });
            }
        }
        for source in &mut self.render_studio.sources {
            if let Some(prepared) = source.prepared.as_ref() {
                source.gpu_ready = false;
                commands.push(RenderCommand::LoadStudioSource {
                    id: source.id,
                    game: source.game_dir.clone(),
                    map: Box::new(prepared.map.as_ref().clone()),
                    video: self.video,
                    progress: Arc::clone(&source.progress),
                });
            }
        }
        for command in commands {
            self.render_command(command);
        }
    }
    pub(in crate::app) fn leave_render_studio_for_launch(&mut self) {
        if self.frontend_page != FrontendPage::RenderStudio
            && self.render_studio.sources.is_empty()
            && self.render_studio.source_job.is_none()
            && self.render_studio.source_prepares.is_empty()
            && self.render_studio_world_compositor.is_none()
        {
            return;
        }
        let time = self.render_studio.timeline.current_time(None);
        self.render_studio.timeline.seek(time);
        self.render_studio.playing = false;
        self.render_studio.audio.preview = None;
        self.render_studio.cinematic_preview = None;
        self.cancel_render_studio_export();
        self.render_studio.still = None;
        self.set_studio_mouse_look(false);
        self.frontend_page = FrontendPage::Main;
        // The launch loads its own world; do not race it with the menu background.
        self.render_studio.frontend_world_released = false;
        self.release_studio_sources();
        self.profile_preview_active = false;
        self.profile_preview_key = None;
        self.asset_preview_viewport_key = None;
        self.render_command(RenderCommand::SetAssetPreviewViewport(None));
        self.render_command(RenderCommand::SetAssetPreviewMode(false));
    }

    pub(in crate::app) fn release_studio_sources(&mut self) {
        // Leaving the Studio returns to the shot camera next time.
        self.render_studio.spatial.scene_camera = None;
        self.render_studio.spatial.drag = None;
        if let Some(job) = self.render_studio.source_job.as_mut() { job.cancelled = true; }
        for job in &mut self.render_studio.source_prepares { job.cancelled = true; }
        self.render_studio.sources.clear();
        self.render_studio.failed_sources.clear();
        self.render_studio.scene_frame = None;
        self.render_studio.source_wait = None;
        self.render_studio.demo_preview = None;
        self.render_studio.preview_sample_time = None;
        self.render_studio.demo_viewport = None;
        self.render_studio.demo_drag = None;
        self.render_studio.bsp_world = None;
        self.render_studio_world_compositor = None;
        self.render_command(RenderCommand::ReleaseStudioSources);
        self.render_command(RenderCommand::SetStudioWorldViewport(None));
    }

    pub(in crate::app) fn studio_demo_scene_active(&self) -> bool {
        self.front_end
            && self.frontend_page == FrontendPage::RenderStudio
            && self.render_studio.scene_frame.is_some()
    }

    fn open_studio_source(&mut self, index: usize) -> Result<(), String> {
        let clip = self.render_studio.timeline.demos[index].clone();
        let base = self.base.clone();
        let project_dir = self.studio_project_directory();
        let video = self.video;
        let forced = self.forced_player_models.clone();
        let race_enabled = self.render_studio.timeline.demos.iter().any(|clip| clip.race_overlays.iter().any(|o| o.enabled));
        let shared_recording = self.render_studio.sources.iter()
            .find(|s| source_pool::matches_recording(&clip, &s.name, &s.game))
            .map(|s| (Arc::clone(&s.playback.demo_bytes), s.playback.demo_index.clone(),
                s.playback.source.clone(), s.game_dir.clone(), s.race_start_server_ms));
        let id = self.render_studio.next_source_id;
        self.render_studio.next_source_id = id.wrapping_add(1) | STUDIO_REQUEST_BIT;
        let name = clip.name.clone();
        let game = clip.game.clone();
        let progress = Arc::new(crate::runtime::LoadProgress::default());
        let worker_progress = Arc::clone(&progress);
        let worker = std::thread::Builder::new().name("studio-demo-load".into()).spawn(move || {
        let progress = worker_progress;
        let qpath = format!("demos/{}.dm_26", clip.name);
        let (bytes, demo_index, source, game_dir, race_start_server_ms) = if let Some(shared) = shared_recording {
            shared
        } else {
        progress.set("Reading recording", 0, 0);
        let reference=clip.source_path.as_deref().ok_or("Demo is not imported into the project")?;
        let path=super::render_studio_media::media_path(&project_dir,reference)?;
        let metadata=std::fs::metadata(&path).map_err(|e|format!("{}: {e}",path.display()))?;
        if metadata.len()==0 || metadata.len()>512*1024*1024 {return Err("Invalid recording size".into());}
        let data=std::fs::read(&path).map_err(|e|format!("{}: {e}",path.display()))?;
        // Match `/rGhost` race selection *once* per loaded authoritative
        // recording. It selects the final non-empty duelTime segment, not an
        // arbitrary current HUD timer or a continuously changing offset.
        let race_start_server_ms = if race_enabled {
            race_ghost::parse_track(clip.name.clone(), data.clone())
                .ok().map(|track| track.race_start_server_ms)
        } else { None };
        let probe = super::probe_demo_gamestate(&data)?;
        let game_dir = jka_assets::pk3::resolve_fs_game_directory(&base, &probe.fs_game)?;
        progress.set("Indexing recording", 0, 0);
        let bytes: Arc<[u8]> = Arc::from(data);
        let demo_index = super::build_demo_index(&bytes).map(Arc::new).map_err(|error| {
            format!("Could not index recording: {error}")
        })?;
        (bytes, Some(demo_index), path.display().to_string(), game_dir, race_start_server_ms)
        };
        progress.set("Loading demo assets", 0, 0);
        let (catalog, classes, player, entities, fx_assets) = App::build_cgame_cpu_assets(
            &base, game_dir.as_deref(), video.pbr, video.allow_asset_overrides)?;
        let mut playback = GameSession::new_with_recording(
            qpath,
            source,
            bytes,
            demo_index,
            catalog, classes, player, entities, crate::cgame::weapon_fx::WeaponFx::new(fx_assets), forced,
        );
        playback.read_until_map()?;
        playback.set_audio_suppressed(true);
        Ok(StudioDemoSource {
            id,
            progress,
            clip_index: index,
            name: clip.name,
            game: clip.game,
            game_dir,
            playback,
            prepared: None,
            race_map: None,
            race_map_job: None,
            race_map_runners: Vec::new(),
            race_map_reference: usize::MAX,
            collision: None,
            gpu_ready: false,
            failed: false,
            revision: 0,
            last_sample_ms: None,
            last_viewed_client: None,
            shader_remaps: Default::default(),
            race_start_server_ms,
            last_presentation_server_ms: None,
            audio_capture: None,
            race_audio_state: None,
            studio_speedometer: Default::default(),
            wallrun_camera_yaw: Default::default(),
            wallrun_camera_mode: 0,
        })
        }).map_err(|e| format!("Could not start Studio demo loader: {e}"))?;
        self.render_studio.source_job = Some(StudioSourceJob {
            id, cancelled: false, index, name, game, worker, progress, upload: true,
        });
        Ok(())
    }

    fn poll_studio_source_job(&mut self) {
        if !self.render_studio.source_job.as_ref().is_some_and(|j| j.worker.is_finished()) { return; }
        let job = self.render_studio.source_job.take().unwrap();
        let result = job.worker.join().unwrap_or_else(|_| Err("Studio demo loader panicked".into()));
        let Some(index) = self.studio_source_clip(job.index, &job.name, &job.game).filter(|_| !job.cancelled) else { return; };
        let result = result.and_then(|mut source| {
            source.clip_index = index;
            self.configure_game_session(&mut source.playback, source.game_dir.as_deref(), false);
            let map = source.playback.map_name.clone().ok_or("Studio demo has no map")?;
            let sub_bsp_names = scene::server_bsp_names(&source.playback.decoder.configstrings)?;
            let options = self.current_map_prepare_options();
            source.progress.set("Preparing map", 0, 0);
            // Reuse immutable preparation stages; the loader validates the BSP
            // hash and each stage's settings before accepting this seed.
            let seed = self.render_studio.sources.iter().find(|s|
                s.game_dir == source.game_dir && s.playback.map_name == source.playback.map_name)
                .and_then(|s| s.prepared.as_ref()).map(|p| Arc::clone(&p.map));
            self.loader_tx.send(super::map_loading::MapLoadRequest {
                request_id: source.id, started: Instant::now(), game: source.game_dir.clone(),
                source: scene::MapSource::Bsp(map), sub_bsp_names, prepare_options: options,
                analyze_fx_light_profiles: true, supersedable: false, seed,
            }).map_err(|_| "Studio map loader is unavailable")?;
            self.render_studio.sources.push(source);
            Ok(())
        });
        if let Err(error) = result {
            self.render_studio.status = format!("Demo preview failed: {error}");
            self.render_studio.failed_sources.insert((job.name, job.game), error);
        }
    }

    pub(in crate::app) fn on_studio_source_prepared(
        &mut self,
        id: u64,
        mut map: Box<scene::PreparedMap>,
        fx: Arc<crate::fx::light_promotion::FxLightPromotionCatalog>,
    ) {
        let options = self.current_map_prepare_options();
        let Some(index) = self.render_studio.sources.iter().position(|s| s.id == id) else {
            return;
        };
        let mut source = self.render_studio.sources.remove(index);
        let shared_images = self.render_studio.sources.iter()
            .filter(|s| s.game_dir == source.game_dir && s.playback.map_name == source.playback.map_name)
            .find_map(|s| s.prepared.as_ref().map(|p| Arc::clone(&p.map)));
        let clip = self.render_studio.timeline.demos[source.clip_index].clone();
        let target = clip.source_time_ms(self.render_studio.timeline_time().max(clip.start));
        let base = self.base.clone();
        let video = self.video;
        let remaps = self.japro_cg.remaps;
        let index = source.clip_index;
        let name = source.name.clone();
        let game = source.game.clone();
        let job_progress = Arc::clone(&source.progress);
        let worker = std::thread::Builder::new().name("studio-demo-prepare".into()).spawn(move || {
        source.progress.set("Preparing demo scene", 0, 0);
        let session = &mut source.playback;
        session.set_fx_light_promotion_catalog(fx);
        source.collision = map.collision.clone();
        session.weapon_fx.set_collision_world(map.collision.clone());
        session
            .weapon_fx
            .set_mark_surfaces(map.mark_surfaces.clone());
        session
            .entity_presenter
            .set_static_models(map.static_models.clone());
        session
            .player_presenter
            .set_collision_world(map.collision.clone());
        if let Some(sound) = session.sound_presenter.as_mut() {
            sound.set_steam_audio_map(
                map.steam_audio_acoustic_mesh.clone(),
                map.steam_audio_bake.clone(),
            );
            if let Some(name) = session.map_name.as_deref() {
                sound.set_eal_map(name);
            }
        }
        if let Some(stats) = map.bsp_stats.as_ref() {
            if let Some(sound) = session.sound_presenter.as_mut() {
                sound.set_inline_model_midpoints(stats.inline_model_midpoints.clone());
            }
            session
                .weapon_fx
                .set_inline_model_bounds(stats.inline_model_bounds.clone());
        }
        session.player_presenter.set_physics_map_mesh(&map.physics_collision)?;
        // Replay to the clip's in-point before marking it ready. This used to
        // decode the entire history on the UI thread at the first transition.
        let progress = Arc::clone(&source.progress);
        session.seek_demo_reporting(Instant::now(), target, move |done, total| progress.set("Seeking recording", done, total))?;
        if let Some(clock) = session.timeline.as_mut() { clock.rate = 0.0; }
        source.last_sample_ms = Some(target);
        source.revision = source.revision.wrapping_add(1);
        for (kind, remap) in session.client_game.drain_shader_remap_actions() {
            if remaps == 0
                || (remaps == 1
                    && kind == crate::cgame::ShaderRemapSource::ShaderState
                    && scene::canonical_shader_name(&remap.from).starts_with("models/players/"))
            {
                continue;
            }
            source
                .shader_remaps
                .apply(&remap.from, &remap.to, Some(remap.time_offset_seconds()));
        }
        session
            .entity_presenter
            .set_shader_remaps(source.shader_remaps.clone());
        session
            .player_presenter
            .set_shader_remaps(source.shader_remaps.clone());
        scene::apply_shader_remaps_in_place(
            &mut map,
            &base,
            source.game_dir.as_deref(),
            options,
            &source.shader_remaps,
        )?;
        if video.stage_collapse_extra {
            scene::apply_reverse_lightmap_collapse_in_place(&mut map);
        }
        if video.additive_stage_fusion {
            scene::fuse_additive_stage_fusion_in_place(&mut map);
        }
        if let Some(shared) = shared_images { map.share_immutable_images_from(&shared); }
        else { map.intern_images(); }
        source.prepared = Some(super::PreparedMapCache {
            label: source.playback.map_name.clone().unwrap_or_default(),
            prepare_options: options,
            map: Arc::from(map),
        });
        Ok(source)
        });
        match worker {
            Ok(worker) => self.render_studio.source_prepares.push(StudioSourceJob {
                id, cancelled: false, index, name, game, worker, upload: true,
                progress: job_progress,
            }),
            Err(error) => {
                self.render_studio.status = format!("Studio preparation worker failed: {error}");
                self.render_studio.failed_sources.insert((name, game), error.to_string());
            }
        }
    }

    fn poll_studio_source_prepares(&mut self) {
        let mut i = 0;
        while i < self.render_studio.source_prepares.len() {
            if !self.render_studio.source_prepares[i].worker.is_finished() { i += 1; continue; }
            let job = self.render_studio.source_prepares.remove(i);
            let result = job.worker.join().unwrap_or_else(|_| Err("Studio preparation worker panicked".into()));
            let index = self.studio_source_clip(job.index, &job.name, &job.game).filter(|_| !job.cancelled);
            if index.is_none() {
                if !job.upload { self.render_command(RenderCommand::RemoveStudioSource(job.id)); }
                continue;
            }
            match result {
                Ok(mut source) => {
                    source.clip_index = index.unwrap();
                    if job.upload {
                        let map = Box::new(source.prepared.as_ref().unwrap().map.as_ref().clone());
                        self.render_command(RenderCommand::LoadStudioSource {
                            id: source.id, game: source.game_dir.clone(), map, video: self.video,
                            progress: Arc::clone(&source.progress),
                        });
                    } else { source.progress.set("Ready", 1, 1); }
                    self.render_studio.sources.push(source);
                }
                Err(error) => {
                    if !job.upload { self.render_command(RenderCommand::RemoveStudioSource(job.id)); }
                    self.render_studio.status = format!("Studio source failed: {error}");
                    self.render_studio.failed_sources.insert((job.name, job.game), error);
                }
            }
        }
    }

    fn seek_studio_source(&mut self, position: usize, target: i32) {
        let mut source = self.render_studio.sources.remove(position);
        let id = source.id;
        let progress = Arc::clone(&source.progress);
        progress.set("Seeking recording", 0, target.max(1) as u64);
        let index = source.clip_index;
        let name = source.name.clone();
        let game = source.game.clone();
        let worker = std::thread::Builder::new().name("studio-demo-seek".into()).spawn(move || {
            let p = Arc::clone(&source.progress);
            source.playback.seek_demo_reporting(Instant::now(), target, move |done, total| p.set("Seeking recording", done, total))?;
            if let Some(clock) = source.playback.timeline.as_mut() { clock.rate = 0.0; }
            source.last_sample_ms = Some(target);
            source.revision = source.revision.wrapping_add(1);
            Ok(source)
        });
        match worker {
            Ok(worker) => self.render_studio.source_prepares.push(StudioSourceJob {
                id, cancelled: false, index, name, game, progress, upload: false, worker,
            }),
            Err(error) => {
                self.render_command(RenderCommand::RemoveStudioSource(id));
                self.render_studio.status = format!("Could not seek recording: {error}");
                self.render_studio.failed_sources.insert((name, game), error.to_string());
            }
        }
    }

    fn studio_source_clip(&self, preferred: usize, name: &str, game: &str) -> Option<usize> {
        let clips = &self.render_studio.timeline.demos;
        if clips.get(preferred).is_some_and(|c| source_pool::matches_recording(c, name, game)) {
            Some(preferred)
        } else { clips.iter().position(|c| source_pool::matches_recording(c, name, game)) }
    }

    /// Rebind clip borrowers without rebuilding their recording-owned state.
    fn reconcile_studio_sources(&mut self, active: &[usize], time: f64) -> Vec<usize> {
        let desired = source_pool::requests_within(&self.render_studio.timeline.demos, active, time,
            self.render_studio.keep_demos_loaded, self.render_studio.scene_capacity);
        let candidates: Vec<_> = self.render_studio.sources.iter().map(|s| source_pool::Candidate {
            id: s.id, index: s.clip_index, name: &s.name, game: &s.game, sampled_ms: s.last_sample_ms,
        }).chain(self.render_studio.source_job.iter().chain(&self.render_studio.source_prepares)
            .filter(|j| !j.cancelled).map(|j| source_pool::Candidate {
                id: j.id, index: j.index, name: &j.name, game: &j.game, sampled_ms: None,
            })).collect();
        let assigned = source_pool::assign(&self.render_studio.timeline.demos, &desired, &candidates, time);
        let binding = |id| assigned.iter().position(|a| *a == Some(id)).map(|i| desired[i]);
        for job in self.render_studio.source_job.iter_mut().chain(&mut self.render_studio.source_prepares) {
            if let Some(index) = binding(job.id) { job.index = index; } else { job.cancelled = true; }
        }
        let mut stale = Vec::new();
        self.render_studio.sources.retain_mut(|s| {
            if let Some(index) = binding(s.id) {
                if s.clip_index != index { s.revision = s.revision.wrapping_add(1); }
                s.clip_index = index;
                true
            } else { stale.push(s.id); false }
        });
        for id in stale { self.render_command(RenderCommand::RemoveStudioSource(id)); }
        if self.render_studio.source_job.is_none() && self.render_studio.source_prepares.is_empty()
            && self.render_studio.sources.iter().all(|s| s.prepared.is_some()) {
            if let Some(index) = desired.iter().zip(&assigned).find_map(|(&index, id)| {
                let clip = &self.render_studio.timeline.demos[index];
                (id.is_none() && !self.render_studio.failed_sources.contains_key(&(clip.name.clone(), clip.game.clone())))
                    .then_some(index)
            }) {
                if let Err(error) = self.open_studio_source(index) {
                    self.render_studio.status = format!("Demo preview failed: {error}");
                    let clip = &self.render_studio.timeline.demos[index];
                    self.render_studio.failed_sources.insert((clip.name.clone(), clip.game.clone()), error);
                }
            }
        }
        desired
    }

    pub(in crate::app) fn studio_map_progress(&mut self, id: u64, task: crate::thread_activity::Task, completed: u32, total: u32) {
        if let Some(world) = self.render_studio.bsp_world.as_ref().filter(|world|world.id == id) {
            world.progress.set(format!("Preparing BSP: {}",task.label()),completed as u64,total as u64);
            self.egui_repaint_requested=true;
            return;
        }
        if let Some(source) = self.render_studio.sources.iter().find(|s| s.id == id) {
            source.progress.set(format!("Preparing map: {}", task.label()), completed as u64, total as u64);
            self.egui_repaint_requested = true;
        }
    }

    pub(in crate::app) fn studio_loading_progress(&self) -> crate::runtime::LoadProgressState {
        let time = self.render_studio.timeline_time();
        let mut indices = Vec::new();
        if let Some(blend) = self.render_studio.timeline.demo_blend_at(time) { indices.extend([blend.outgoing, blend.incoming]); }
        else if let Some(index) = self.render_studio.timeline.demo_index_at(time) { indices.push(index); }
        for index in indices {
            if let Some(job) = self.render_studio.source_prepares.iter().find(|j| j.index == index && !j.cancelled) {
                return job.progress.snapshot();
            }
            if let Some(job) = self.render_studio.source_job.as_ref().filter(|j| j.index == index && !j.cancelled) {
                return job.progress.snapshot();
            }
            if let Some(source) = self.render_studio.sources.iter().find(|s| s.clip_index == index && !s.gpu_ready) {
                return source.progress.snapshot();
            }
        }
        crate::runtime::LoadProgressState { label: "Waiting for preview".into(), completed: 0, total: 0 }
    }

    pub(in crate::app) fn studio_memory_report_lines(&self) -> Vec<String> {
        let mut recordings = std::collections::HashSet::new();
        let mut buffers = std::collections::HashSet::new();
        let mut bytes = 0usize;
        for source in &self.render_studio.sources {
            recordings.insert((source.name.to_ascii_lowercase(), source.game.to_ascii_lowercase()));
            let data = &source.playback.demo_bytes;
            if buffers.insert(data.as_ptr() as usize) { bytes += data.len(); }
        }
        let pending = self.render_studio.source_job.iter().chain(&self.render_studio.source_prepares)
            .filter(|j| !j.cancelled).count();
        for job in self.render_studio.source_job.iter().chain(&self.render_studio.source_prepares).filter(|j| !j.cancelled) {
            recordings.insert((job.name.to_ascii_lowercase(), job.game.to_ascii_lowercase()));
        }
        let mut lines = vec![format!("MEMORY CPU Studio: {} timeline clips, {} unique recordings, {} available playback contexts, {} pending workers; available unique demo bytes {:.2} MiB (excludes presenters, physics, maps, and pending workers)",
            self.render_studio.timeline.demos.len(), recordings.len(), self.render_studio.sources.len(), pending,
            bytes as f64 / (1024.0 * 1024.0))];
        // App-side prepared maps, one per recording; shared maps and interned
        // pixel buffers count once. Each also sent a deep copy to its scene.
        let mut maps = std::collections::HashSet::new();
        let mut seen_pixels = std::collections::HashSet::new();
        let mut totals: Vec<(&'static str, usize)> = Vec::new();
        for prepared in self.render_studio.sources.iter().filter_map(|s| s.prepared.as_ref()) {
            if !maps.insert(Arc::as_ptr(&prepared.map) as usize) { continue; }
            for (index, (name, bytes)) in prepared.map.heap_estimate(&mut seen_pixels).into_iter().enumerate() {
                if let Some(total) = totals.get_mut(index) { total.1 += bytes; } else { totals.push((name, bytes)); }
            }
        }
        let sum: usize = totals.iter().map(|(_, bytes)| bytes).sum();
        lines.push(format!("MEMORY CPU Studio prepared maps: {} sources hold {} distinct maps, ~{:.2} MiB: {}",
            self.render_studio.sources.iter().filter(|s| s.prepared.is_some()).count(), maps.len(),
            sum as f64 / (1024.0 * 1024.0),
            totals.iter().map(|(name, bytes)| format!("{name} {:.1}", *bytes as f64 / (1024.0 * 1024.0)))
                .collect::<Vec<_>>().join(", ")));
        #[cfg(windows)] {
            use windows_sys::Win32::System::{
                ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
                Threading::GetCurrentProcess,
            };
            // SAFETY: the sized output struct and current-process handle are valid.
            let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
            counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(), counters.cb) } != 0 {
                lines.push(format!("MEMORY process: working set {:.2} MiB, private committed {:.2} MiB (whole process; separate from GPU allocator figures)",
                    counters.WorkingSetSize as f64 / (1024.0 * 1024.0), counters.PrivateUsage as f64 / (1024.0 * 1024.0)));
            }
        }
        lines.extend(super::process_memory::report_lines());
        lines
    }

    pub(in crate::app) fn on_studio_source_ready(&mut self, id: u64, error: Option<String>, vram: Option<(u64, u64)>, scenes: usize) {
        // Keep dedicated VRAM under this share of the OS budget: past it the
        // driver starts paging or failing allocations for every scene.
        const STUDIO_VRAM_FILL: f64 = 0.85;
        let out_of_memory = error.as_deref()
            .is_some_and(|e| e.starts_with(crate::renderer::STUDIO_SCENE_OUT_OF_GPU_MEMORY));
        let previous_capacity = self.render_studio.scene_capacity;
        if out_of_memory {
            // The failed scene did not fit beside the `scenes` already retained.
            self.render_studio.scene_capacity = previous_capacity.min(scenes);
        } else if vram.is_some_and(|(usage, budget)| usage as f64 > budget as f64 * STUDIO_VRAM_FILL) {
            self.render_studio.scene_capacity = previous_capacity.min(scenes.saturating_sub(1));
        }
        if self.render_studio.scene_capacity < previous_capacity {
            eprintln!("Studio: GPU memory near budget ({}); retaining at most {} demo scenes",
                vram.map_or_else(|| "allocation failed".to_owned(), |(usage, budget)|
                    format!("{:.0} / {:.0} MiB", usage as f64 / 1048576.0, budget as f64 / 1048576.0)),
                self.render_studio.scene_capacity);
        }
        if out_of_memory {
            // Retry once after the lower capacity evicts lower-priority extras;
            // reconcile reopens it. A second failure is reported as usual.
            if let Some(position) = self.render_studio.sources.iter().position(|s| s.id == id) {
                let key = (self.render_studio.sources[position].name.clone(), self.render_studio.sources[position].game.clone());
                if self.render_studio.oom_retried.insert(key) {
                    self.render_studio.sources.remove(position);
                    self.render_command(RenderCommand::RemoveStudioSource(id));
                    self.render_studio.status = "GPU memory is full: unloading other demos and retrying".into();
                    self.egui_repaint_requested = true;
                    return;
                }
            }
        }
        let race_enabled = self.render_studio.sources.iter().find(|s| s.id == id)
            .and_then(|s| self.render_studio.timeline.demos.get(s.clip_index))
            .is_some_and(|clip| clip.race_overlays.iter().any(|o| o.enabled));
        if let Some(source) = self.render_studio.sources.iter_mut().find(|s| s.id == id) {
            source.failed = error.is_some();
            source.gpu_ready = error.is_none();
            if error.is_none() { source.progress.set("Ready", 1, 1); }
            self.render_studio.status = error.map_or_else(
                || if race_enabled && source.race_start_server_ms.is_none() {
                    format!("Demo {} has no /rGhost-compatible duelTime race segment; composite actors cannot be aligned", source.name)
                } else {format!("Demo ready: {}", source.name)},
                |e| format!("Demo preview failed: {e}"),
            );
            self.egui_repaint_requested = true;
        }
    }

    /// A composite belongs to a *particular timeline demo*, not the entire
    /// project. The same source recording can therefore be attached to one
    /// clip without appearing in other clips, even after a Director cut.
    pub(in crate::app) fn toggle_studio_race_overlay_for_demo(
        &mut self, clip_index: usize, name: String, game: String,
    ) {
        let exists=self.render_studio.timeline.demos.get(clip_index)
            .is_some_and(|c|c.race_overlays.iter().any(|o|o.name==name && o.game==game));
        let source_path=if exists {None} else {
            match self.studio_copy_library_demo(&name,&game) {
                Ok(path)=>Some(path),
                Err(error)=>{self.render_studio.status=format!("Cannot copy composite demo: {error}");return;},
            }
        };
        let setting = super::render_studio_timeline::StudioRaceOverlay {
            source_path, name, game, alpha: 0.65, enabled: true,
            strafe_trail: false, trail_ticks: false, trail_tick_labels:Default::default(), trail_color: None,
        };
        if !setting.valid() {
            self.render_studio.status = "Invalid local demo path".into();
            return;
        }
        let previously_enabled = self.render_studio.timeline.demos.iter()
            .any(|clip| clip.race_overlays.iter().any(|o| o.enabled));
        let Some(clip) = self.render_studio.timeline.demos.get_mut(clip_index) else { return };
        if clip.comparison.is_some() {
            self.render_studio.status="Add recordings to the timeline and regenerate the ghost race in Director to change its runners".into();
            return;
        }
        let label = clip.name.clone();
        if let Some(index) = clip.race_overlays.iter().position(|o|
            o.name == setting.name && o.game == setting.game) {
            clip.race_overlays.remove(index);
            self.render_studio.status = format!("Removed composite race actor from {label}");
        } else {
            if clip.race_overlays.len() >= 32 {
                self.render_studio.status = "Maximum 32 composite race actors per demo clip".into();
                return;
            }
            clip.race_overlays.push(setting);
            self.render_studio.status = format!("Composite race actor attached to {label}");
        }
        // Entity numbers are allocated from the loaded union of sources.
        // Regenerate them after changes, but opacity drags never need a reload.
        // Keep existing decoded POV trails when composite assignments change.
        self.render_studio.race_runtime.clear();
        if !previously_enabled && self.render_studio.timeline.demos.iter()
            .any(|clip| clip.race_overlays.iter().any(|o| o.enabled)) {
            // Existing demo sessions were opened without race-start anchors.
            self.invalidate_studio_sources();
        }
    }

    /// Undo/redo can restore enabled composites onto demo sessions that were
    /// prepared before any race actors existed. Refresh those anchors once.
    pub(in crate::app) fn refresh_studio_composite_anchors_after_history(&mut self) {
        let missing=self.render_studio.sources.iter().any(|source| {
            self.render_studio.timeline.demos.get(source.clip_index).is_some_and(|clip|
                clip.race_overlays.iter().any(|overlay| overlay.enabled)) &&
            source.race_start_server_ms.is_none()
        });
        if missing { self.invalidate_studio_sources(); }
    }

    fn poll_studio_race_overlays(&mut self) {
        // Decode each distinct source once, even if multiple target clips
        // attach the same ghost. Keep cached sources across ordinary cuts.
        let overlays: Vec<_> = self.render_studio.timeline.demos.iter()
            .flat_map(|clip| clip.race_overlays.iter())
            .filter(|overlay| overlay.enabled).cloned().collect();
        // The recorded POV requests the exact same race decoder/route index
        // as composite actors. Cache once by recording identity, including
        // when one recording is both a source and a composite in another clip.
        let mut keys: Vec<(String,String)> = overlays.iter()
            .map(|o|(o.name.clone(),o.game.clone())).collect();
        for clip in &self.render_studio.timeline.demos {
            if clip.pov_trail.enabled {
                let key=(clip.name.clone(),clip.game.clone());
                if !keys.contains(&key) {keys.push(key);}
            }
        }
        self.render_studio.race_runtime.retain(|entry| keys.contains(&entry.key));
        for key in keys {

            if self.render_studio.race_runtime.iter().any(|entry| entry.key == key) { continue; }
            let project_dir = self.studio_project_directory();
            let managed = self.render_studio.timeline.demos.iter()
                .find(|c|c.name==key.0 && c.game==key.1).and_then(|c|c.source_path.clone())
                .or_else(||self.render_studio.timeline.demos.iter().flat_map(|c|&c.race_overlays)
                    .find(|o|o.name==key.0 && o.game==key.1).and_then(|o|o.source_path.clone()));
            let (name,game) = key.clone();
            let worker = std::thread::Builder::new()
                .name(format!("studio-race:{}", name.chars().take(24).collect::<String>()))
                .spawn(move || {
                    if let Some(reference)=managed {
                        let media=super::render_studio_media::media_path(&project_dir,&reference)?;
                        let bytes=std::fs::read(media).map_err(|e|e.to_string())?;
                        return race_ghost::parse_track(name,bytes);
                    }
                    Err(format!("Composite recording is not imported into this project: {name}"))
                });
            let (worker, error) = match worker {
                Ok(worker) => (Some(worker), None),
                Err(error) => (None, Some(error.to_string())),
            };
            self.render_studio.race_runtime.push(StudioRaceRuntime {
                key, worker, pending_track: None, ghost: None, error,
            });
        }
        // Only create player presenters when a real demo world is available.
        // Do not resolve against a warmed *future* scene: its map might
        // differ from the clip under the playhead and would cause a false
        // permanent mismatch. Defer registration until the actual world loads.
        let time = self.render_studio.timeline_time();
        let active_clip = self.render_studio.timeline.demo_index_at(time);
        let Some(source) = self.render_studio.sources.iter().find(|s| {
            Some(s.clip_index) == active_clip && s.gpu_ready && !s.failed
        }) else {return};
        let map = source.playback.map_name.as_deref();
        let siege_classes = &source.playback.siege_classes;
        for (index, runtime) in self.render_studio.race_runtime.iter_mut().enumerate() {
            if runtime.worker.as_ref().is_some_and(|worker| worker.is_finished()) {
                let result = runtime.worker.take().unwrap().join()
                    .unwrap_or_else(|_| Err("Race decode worker panicked".into()));
                match result {
                    Err(error) => runtime.error = Some(error),
                    Ok(track) => runtime.pending_track = Some(track),
                }
            }
            // The timeline may contain different maps. Keep the local decoded
            // player until its map actually becomes the authoritative world.
            if runtime.ghost.is_none() && runtime.pending_track.as_ref().is_some_and(|track| {
                map.is_some_and(|current| current.eq_ignore_ascii_case(&track.map_name))
            }) {
                let track = runtime.pending_track.take().unwrap();
                match race_ghost::RaceGhost::from_track(track, siege_classes, 60_000 + index as u16) {
                    Ok(mut ghost) => {ghost.enable_studio_presentation(siege_classes);runtime.ghost = Some(ghost);},
                    Err(error) => runtime.error = Some(error),
                }
            }
            if let Some(error) = &runtime.error {
                self.render_studio.status = format!("Studio race actor: {error}");
            }
        }
    }

    pub(in crate::app) fn tick_render_studio_demo_preview(&mut self, now: Instant) {
        self.poll_studio_source_job();
        self.poll_studio_source_prepares();
        if !self.front_end || self.frontend_page != FrontendPage::RenderStudio {
            if !self.render_studio.sources.is_empty()
                || self.render_studio.source_job.as_ref().is_some_and(|j| !j.cancelled)
                || self.render_studio.source_prepares.iter().any(|j| !j.cancelled)
                || self.render_studio_world_compositor.is_some()
            {
                self.release_studio_sources();
                self.render_command(RenderCommand::SetAssetPreviewMode(false));
            }
            return;
        }
        self.poll_studio_race_overlays();
        // The capture acknowledgement advances the export clock. Re-presenting
        // bones/FX/audio at the same clock while FFmpeg is busy is wasted work.
        if self.render_studio.export.as_ref().is_some_and(|e| !e.can_request())
            && self.render_studio.scene_frame.is_some() { return; }
        let time = self.render_studio.timeline_time();
        // A DM26 has no video transitions: its camera pass samples hard cuts.
        let dm26 = self.render_studio.dm26_capture.is_some();
        let blend = self.render_studio.timeline.demo_blend_at(time).filter(|_| {
            (self.render_studio.live_gpu_transitions
                || self.render_studio.export.is_some()
                || self.render_studio.still.is_some()) && !dm26
        });
        let mut desired = Vec::new();
        if let Some(b) = blend {
            desired.extend([b.outgoing, b.incoming]);
        } else if let Some(i) = self.render_studio.timeline.demo_index_at(time) {
            // Top visible demo layer.
            desired.push(i);
        }
        let active = desired.clone();
        let desired = self.reconcile_studio_sources(&active, time);
        for source in &mut self.render_studio.sources {
            if !active.contains(&source.clip_index) {
                source.playback.set_audio_suppressed(true);
            }
        }
        if active.is_empty() {
            if dm26 { self.record_render_studio_dm26_frame(super::render_studio_dm26::Dm26Frame::gap()); }
            self.render_studio.source_wait = None;
            self.render_studio.scene_frame = None;
            self.render_studio.demo_preview = None;
            // A BSP character backdrop uses the same full-quality world
            // compositor as demo footage, not the isolated model preview.
            self.render_command(RenderCommand::SetAssetPreviewMode(!self.studio_bsp_active()));
            return;
        }
        // A backward/large seek only replays the retained recording. Move that
        // work off the UI thread without discarding its prepared/GPU scene.
        if let Some((position, target)) = active.iter().find_map(|i| {
            let clip = &self.render_studio.timeline.demos[*i];
            let target = clip.source_time_ms(time);
            let position = self.render_studio.sources.iter().position(|s| s.clip_index == *i && s.gpu_ready && !s.failed)?;
            let source = &self.render_studio.sources[position];
            let current = source.playback.demo_elapsed_ms(now).unwrap_or(-1.0);
            (source.last_sample_ms.is_some_and(|last| target < last) || !current.is_finite()
                || current < 0.0 || (f64::from(target) - current).abs() > 1000.0).then_some((position, target))
        }) {
            self.seek_studio_source(position, target);
        }
        // Prepare the next borrower on an idle recording before the cut. A
        // future clip must never seek a context still serving an active clip.
        if self.render_studio.source_prepares.is_empty() {
            if let Some((position, target)) = desired.iter().filter(|i| !active.contains(i)).find_map(|i| {
                let clip = &self.render_studio.timeline.demos[*i];
                if clip.start < time { return None; }
                let position = self.render_studio.sources.iter().position(|s|
                    s.clip_index == *i && s.gpu_ready && !s.failed)?;
                let source = &self.render_studio.sources[position];
                let target = clip.source_time_ms(clip.start);
                let current = source.playback.demo_elapsed_ms(now).unwrap_or(-1.0);
                (source.last_sample_ms.is_some_and(|last| target < last) || !current.is_finite()
                    || current < 0.0 || (f64::from(target) - current).abs() > 1000.0).then_some((position, target))
            }) { self.seek_studio_source(position, target); }
        }
        // Warm the bounded HUD model off-thread, and include it in source
        // readiness so the first exported frame matches preview/scrubbing.
        for i in &active {
            let Some(race) = self.render_studio.timeline.demos[*i].comparison.as_ref() else { continue; };
            if let Some(source) = self.render_studio.sources.iter_mut().find(|s| s.clip_index == *i && s.gpu_ready && !s.failed) {
                if let Err(error) = super::render_studio_race_map::prepare(source, race, &self.render_studio.race_runtime) {
                    source.failed = true;
                    self.render_studio.status = error;
                }
            }
        }
        let ready = active.iter().all(|i| {
            self.render_studio
                .sources
                .iter()
                .any(|s| s.clip_index == *i && s.gpu_ready && !s.failed
                    && (self.render_studio.timeline.demos[*i].comparison.is_none() || s.race_map.is_some()))
                && self.render_studio.timeline.demos[*i].comparison.as_ref().is_none_or(|race|
                    race.runners.iter().all(|runner| self.render_studio.race_runtime.iter().any(|r|
                        r.key.0==runner.name&&r.key.1==runner.game&&r.ghost.is_some())))
        });
        if !ready {
            for source in &mut self.render_studio.sources {
                source.playback.set_audio_suppressed(true);
            }
            let failed = active.iter().any(|i| {
                let clip = &self.render_studio.timeline.demos[*i];
                clip.comparison.as_ref().is_some_and(|race| race.runners.iter().any(|runner|
                    self.render_studio.race_runtime.iter().any(|r|r.key.0==runner.name&&r.key.1==runner.game&&r.error.is_some())))
                || self.render_studio
                    .failed_sources
                    .contains_key(&(clip.name.clone(), clip.game.clone()))
                    || self
                        .render_studio
                        .sources
                        .iter()
                        .any(|s| s.clip_index == *i && s.failed)
            });
            if failed {
                self.render_studio.timeline.seek(time);
                self.render_studio.source_wait = None;
                self.render_studio.scene_frame = None;
                let clip = &self.render_studio.timeline.demos[active[0]];
                self.render_studio.demo_preview = Some(StudioDemoPreview {
                    name: clip.name.clone(),
                    game: clip.game.clone(),
                    active: false,
                    failed: true,
                });
                for source in &mut self.render_studio.sources {
                    source.playback.set_audio_suppressed(true);
                }
                if self.render_studio.export.is_some() || self.render_studio.still.is_some() {
                    self.fail_render_studio_export("A required demo source failed to load");
                }
                return;
            }
            // Loading is a preview concern. Hold transport without replacing
            // App.loading or blocking the editor's UI/gameplay event handlers.
            let transport = (
                self.render_studio.timeline.playhead,
                self.render_studio.timeline.play_started,
                self.render_studio.timeline.playing,
            );
            self.render_studio.source_wait = Some((transport, time));
            self.render_studio.scene_frame = None;
            return;
        }
        if self.render_studio.source_wait.take().is_some() && self.render_studio.export.is_none() {
            self.render_studio.timeline.playhead = time;
            self.render_studio.timeline.play_started = now;
        }
        // A live Director shot overrides the keyed camera inside its range.
        let key = self.render_studio.timeline.effective_demo_camera_at(time);
        let backup_key = self
            .render_studio
            .timeline
            .camera
            .iter()
            .rev()
            .find(|k| k.time <= time)
            .copied()
            .unwrap_or(key);
        // Studio demo audio is independent of the game's volume sliders
        // (s_volume / s_volumeVoice / music / emote): the base is unity and
        // levels come only from the clip Volume + envelope, the local preview
        // slider (preview only) and, for exports, the clip's own settings.
        // Only stereo separation (a spatial preference) follows the game.
        let (_, _, separation) = self.effective_audio_mix();
        let unfocused_mute = self.audio.mute_when_unfocused && !self.window_focused;
        let (effects, voice, music, emote) = if unfocused_mute { (0.0, 0.0, 0.0, 0.0) } else { (1.0, 1.0, 1.0, 1.0) };
        // Preview monitor level is separate from the authored clip envelopes
        // and must never enter the detached offline export capture.
        let monitor_volume = if self.render_studio.export.is_some() || self.render_studio.still.is_some() {
            1.0
        } else {
            self.render_studio.preview_volume
        };
        let from_key = self
            .render_studio
            .timeline
            .demo_follow_blend(time)
            // A keyed follow move must not bend a Director shot's pose.
            .filter(|_| self.render_studio.timeline.director_at(time).is_none())
            .map(|(key, _, a)| (key, a));
        let exporting=self.render_studio.export.is_some();
        let offline_capture=exporting || self.render_studio.still.is_some() || dm26;
        let mut dm26_frame=None;
        let capturing=exporting && self.render_studio.preset!=StudioVideoFormat::Gif;
        let playing = (self.render_studio.timeline.playing && self.render_studio.still.is_none()) || capturing;
        let any_demo_solo=self.render_studio.timeline.demos.iter().any(|c|c.audio.solo);
        let imported_solo=self.render_studio.timeline.audio_tracks.iter().any(|t|t.solo);
        let hud_defaults=self.render_studio.hud_settings.clone();
        // Studio-owned virtual reference positions; never read gameplay cg_hud*.
        let hud_layout=self.render_studio.hud_layout;
        let hud_crosshair=self.crosshair;
        let hud_strafe_helper=self.strafe_helper;
        let hud_keys=self.movement_keys_hud;
        let hud_speedometer=self.japro_cg.speedometer;
        let hud_video=self.video;
        let hud_spec_hud=self.japro_cg.spec_hud;
        let mut frames = Vec::new();
        let wallrun_camera_mode = self.wallrun_camera_mode;
        for index in active {
            if let Some(race)=self.render_studio.timeline.demos[index].comparison.as_mut() {
                super::render_studio_race::ensure_progress(race,&self.render_studio.race_runtime);
            }
            let source = self
                .render_studio
                .sources
                .iter_mut()
                .find(|s| s.clip_index == index)
                .unwrap();
            let viewport=super::render_studio::studio_world_frame_size(self.render_studio.capture_size,self.render_studio.demo_viewport);
            if let Some(race) = self.render_studio.timeline.demos[index].comparison.as_mut() {
                super::render_studio_race::ensure_camera(race, &self.render_studio.race_runtime,
                    source.collision.as_mut(), viewport);
            }
            let clip = self.render_studio.timeline.demos[index].clone();
            source.wallrun_camera_mode = wallrun_camera_mode;
            let race_sample=clip.comparison.as_ref().map(|race|race.sample(time-clip.start));
            let reference_state=clip.comparison.as_ref().zip(race_sample.as_ref()).and_then(|(race,sample)| {
                let focus=race.focus(sample.phase);
                let runner=&race.runners[focus];let ms=sample.clocks[focus].min(runner.race_ms);
                let ghost=self.render_studio.race_runtime.iter().find(|r|r.key.0==runner.name&&r.key.1==runner.game)?.ghost.as_ref()?;
                let index=ghost.track.frames.partition_point(|f|f.elapsed_ms<=ms).saturating_sub(1);
                Some((ghost.track.frames[index].player_state.clone(),ghost.track.race_start_server_ms.saturating_add(ms)))
            });
            let race_edit = self.render_studio.timeline.director_at(time)
                .filter(|shot| shot.comparison_label.is_some()).and_then(|shot| shot.comparison_camera.as_ref());
            let race_camera=clip.comparison.as_ref().zip(race_sample.as_ref()).map(|(race,sample)|
                super::render_studio_race::camera(race,sample,&self.render_studio.race_runtime,
                    source.collision.as_mut(),time-clip.start,viewport,race_edit));
            let has_race_camera=race_camera.is_some();
            let sample_key=if let Some((camera,lens))=&race_camera {
                let mut k=super::render_studio_spatial::key_from_camera(*camera);
                k.source=StudioCameraSource::Demo{pov:StudioDemoPov::Free,client:-1};k.lens=*lens;k
            }else{key};
            let gain = blend.map_or(1.0, |b| {
                if index == b.incoming {
                    b.progress
                } else {
                    1.0 - b.progress
                }
            });
            // A DM26 is an event source. During export, capture *that same*
            // SoundPresenter into a detached offline mixer rather than trying to
            // decode DM26 as PCM or recording OS speaker output.
            let demo_audio_gain=if (any_demo_solo && !clip.audio.solo) || (imported_solo && !any_demo_solo) {0.0}
                else {clip.audio_gain_at(time)};
            if let Some(sound) = source.playback.sound_presenter.as_mut() {
                // Native offline capture keeps original source gain. Render
                // Studio applies the linked track envelope at each OUTPUT
                // sample so preview/export never multiply volume twice.
                let level=if capturing {1.0}else{gain*demo_audio_gain*monitor_volume};
                // Unfocus-mute is a local speaker courtesy; exports stay audible.
                let (effects, voice, music, emote) = if capturing { (1.0, 1.0, 1.0, 1.0) } else { (effects, voice, music, emote) };
                sound.set_mix(effects * level, voice * level, separation);
                sound.set_music_volume(music * level);
                sound.set_emote_music_volume(emote * level);
            }
            // The GameSession presenter prunes entities not reported as live
            // on each frame. Keep Studio actors resident across interpolation,
            // scrubbing and ordinary camera playback; they still don't enter
            // gameplay's entity list, collision, sound or server simulation.
            let composite_entity_numbers = self.render_studio.race_runtime.iter()
                .filter_map(|runtime| runtime.ghost.as_ref().map(|ghost| (runtime, ghost)))
                .filter(|(runtime, _)| clip.race_overlays.iter().any(|o| {
                    o.enabled && o.name == runtime.key.0 && o.game == runtime.key.1
                }))
                .map(|(_, ghost)| ghost.entity_num).collect::<Vec<_>>();
            let hud_flags=hud_defaults.effective(&clip.hud_overrides);
            // Race HUD processing belongs to the Studio recording, not gameplay.
            // Preserve the selected jaPRO layout/precision while making the
            // Studio race toggle independent from cg_raceTimer.
            source.playback.japro_cg.race_timer=if hud_flags.enabled("race_timer") {
                self.japro_cg.race_timer.max(1)
            } else {0};
            source.playback.japro_cg.race_timer_x=self.japro_cg.race_timer_x;
            source.playback.japro_cg.race_timer_y=self.japro_cg.race_timer_y;
            source.playback.japro_cg.race_timer_size=self.japro_cg.race_timer_size;
            match source.sample(
                &clip,
                time,
                sample_key,
                if clip.comparison.is_some(){None}else{from_key},
                backup_key,
                &self.video,
                self.third_person,
                self.spectator_camera,
                &composite_entity_numbers,
                playing,
                capturing,
                !offline_capture && self.render_studio.spatial.scene_camera.is_some(),
                self.render_studio.timeline.director_at(time).is_some_and(|shot| shot.auto) && clip.comparison.is_none(),
                self.first_person_lightsaber,
                now,
            ) {
                Ok(mut frame) => {
                    let race_audible=clip.comparison.as_ref().zip(race_sample.as_ref())
                        .is_some_and(|(race,sample)|playing && race.audio_active(sample));
                    if let Some((race,sample))=clip.comparison.as_ref().zip(race_sample.as_ref()) {
                        let focus=race.focus(sample.phase);
                        if source.race_audio_state.is_some_and(|(old,was_audible,last)|old!=focus || was_audible!=race_audible || time<last || time-last>0.25) {
                            if let Some(sound)=source.playback.sound_presenter.as_mut() {sound.clear();}
                        }
                        source.race_audio_state=Some((focus,race_audible,time));
                    }
                    if let Some((camera,lens))=race_camera {
                        frame.camera=camera;frame.shot_camera=camera;frame.lens=lens;
                        // Server area masks describe the held native POV, not
                        // the indexed ghost's current replay location.
                        frame.area_mask=None;
                    }
                    // Preview observer is presentation-only. Resolve the final
                    // camera BEFORE view-dependent Studio trail culling/tessellation;
                    // offline captures still retain the authored camera.
                    apply_scene_observer(&mut frame,self.render_studio.spatial.scene_camera,offline_capture);
                    if dm26 {
                        let recorded=source.playback.demo_followed_client().unwrap_or(-1);
                        dm26_frame=Some(super::render_studio_dm26::Dm26Frame {
                            clip:Some(index),source_ms:f64::from(clip.source_time_ms(time)),
                            view:super::render_studio_dm26::studio_dm26_view(sample_key,recorded,
                                from_key.is_some()&&clip.comparison.is_none(),has_race_camera,&frame.shot_camera),
                            players:source.playback.studio_visible_players.clone(),
                        });
                    }
                    let hud_viewport=super::render_studio::studio_world_frame_size(self.render_studio.capture_size,self.render_studio.demo_viewport);
                    frame.hud=studio_hud_for_source(source,&hud_flags,hud_video,hud_layout,
                        hud_crosshair,hud_strafe_helper,hud_keys,hud_speedometer,hud_spec_hud,
                        reference_state.as_ref().map(|(ps,t)|(ps,*t)),sample_key,&frame.shot_camera,hud_viewport);
                    if let Some((race,sample))=clip.comparison.as_ref().zip(race_sample.as_ref()) {
                        let ui=frame.hud.get_or_insert_with(|| {
                            let mut hud=crate::ui::UiSnapshot::default();
                            hud.studio_hud_virtual_layout=true;
                            hud.crosshair.style=0;
                            hud.strafe_helper.flags=0;
                            Arc::new(hud)
                        });
                        super::render_studio_race::hud(race,sample,&self.render_studio.race_runtime,&frame.camera,hud_viewport,
                            source.race_map.clone(),time-clip.start,Arc::make_mut(ui));
                    }
                    if clip.pov_trail.enabled || clip.race_overlays.iter().any(|o|o.enabled) {
                        let anchor = source.race_start_server_ms;
                        let server_time = source.last_presentation_server_ms.unwrap_or(0);
                        let mut additional = Vec::new();
                        // Trail geometry is view dependent (same path as /rGhost).
                        // Keep it on the Studio scene's real camera and in this
                        // source frame so outgoing/incoming transition scenes do
                        // not accidentally inherit one another's routes.
                        let mut trail_draws = Vec::new();
                        let has_trails = clip.pov_trail.enabled ||
                            clip.race_overlays.iter().any(|o|o.enabled);
                        let mut trail_fx_view = has_trails.then(|| crate::fx::draw::FxView::from_camera(&frame.camera));
                        let trail_view = trail_fx_view.as_ref().map(|view| {
                            let [width,height] = super::render_studio::studio_world_frame_size(self.render_studio.capture_size,self.render_studio.demo_viewport);
                            let fov_y=frame.camera.fov_y_for_viewport(width,height);
                            let tan_y=(fov_y * 0.5).tan().max(1e-3);
                            crate::strafe_trail::TrailView {
                                origin:view.origin,forward:view.axis[0],left:view.axis[1],up:view.axis[2],
                                tan_half_fov_y:tan_y,
                                tan_half_fov_x:tan_y * width as f32 / height as f32,
                                focal_length_pixels:height as f32 / (2.0*tan_y),
                            }
                        });
                        // Own POV route is an indexed race-ghost trail but never
                        // an additional player model. Draw it in this source's
                        // coordinate space even for detached / Director shots.
                        if clip.pov_trail.enabled {
                            if let Some(view)=trail_view {
                                if let Some(runtime)=self.render_studio.race_runtime.iter_mut().find(|r|
                                    r.key==(clip.name.clone(),clip.game.clone())) {
                                    if let Some(ghost)=runtime.ghost.as_mut() {
                                        if source.playback.map_name.as_ref().is_some_and(|m|
                                            m.eq_ignore_ascii_case(&ghost.track.map_name)) {
                                            ghost.track.strafe_trail.append_studio_draws(
                                                &self.strafe_trails.settings,view,
                                                clip.pov_trail.resolved_trail_color(&clip),255,
                                                clip.pov_trail.trail_ticks,
                                                clip.pov_trail.trail_tick_labels,&mut trail_draws);
                                        }
                                    }
                                }
                            }
                        }
                        for overlay in &clip.race_overlays {
                            if !overlay.enabled { continue; }
                            let is_focused=clip.comparison.as_ref().zip(race_sample.as_ref()).is_some_and(|(race,sample)| {
                                let main=&race.runners[race.focus(sample.phase)];
                                main.name==overlay.name&&main.game==overlay.game
                            });
                            let Some(runtime) = self.render_studio.race_runtime.iter_mut().find(|r| {
                                r.key.0 == overlay.name && r.key.1 == overlay.game
                            }) else {continue};
                            let Some(ghost) = runtime.ghost.as_mut() else {continue};
                            if !source.playback.map_name.as_ref().is_some_and(|m| m.eq_ignore_ascii_case(&ghost.track.map_name)) {continue;}
                            // Source and overlay anchor are BOTH the selected
                            // duelTime reset. The original race ghost handles
                            // frame interpolation and teleport discontinuities.
                            let comparison_clock=clip.comparison.as_ref().zip(race_sample.as_ref()).and_then(|(race,sample)|
                                race.runners.iter().position(|r|r.name==overlay.name&&r.game==overlay.game).map(|i|
                                    sample.clocks[i].clamp(0,ghost.track.duration_ms())));
                            let sampled=if let Some(clock)=comparison_clock {ghost.sample(1,true,clock.saturating_add(1))}
                                else {anchor.and_then(|anchor|ghost.sample(anchor,true,server_time))};
                            if let Some(clock)=comparison_clock {
                                if is_focused {
                                    if let (Some(sound),Some(game),Some((entity,_,_)))=(source.playback.sound_presenter.as_mut(),ghost.studio_sound_game.as_ref(),sampled.as_ref()) {
                                        let ahead=glam::Vec3::from_array(super::scene::jka_position(frame.camera.forward().to_array()));
                                        let left=glam::Vec3::Z.cross(ahead).normalize_or_zero();
                                        let up=ahead.cross(left).normalize_or_zero();
                                        sound.recorded_player_frame(crate::audio::Listener {
                                            entity:ghost.track.client_num as u16,
                                            origin:super::scene::jka_position(frame.camera.position.to_array()),
                                            ahead:ahead.to_array(),left:left.to_array(),up:up.to_array(),
                                        },ghost.entity_num,entity,game,race_audible);
                                    }
                                }
                                let events=ghost.studio_audio_events(clock,is_focused && race_audible);
                                if let (Some(sound),Some(game))=(source.playback.sound_presenter.as_mut(),ghost.studio_sound_game.as_ref()) {
                                    for event in &events {sound.play_recorded_player_event(event,game,&[]);}
                                }
                            }
                            if let Some((entity, reset, elapsed)) = sampled {
                                let actor_time=if comparison_clock.is_some(){ghost.track.race_start_server_ms.saturating_add(elapsed)}else{server_time};
                                match source.playback.player_presenter.present_race_ghost_entity(
                                    &entity, &ghost.info, actor_time, if is_focused {1.0}else if clip.comparison.is_some() {overlay.alpha*COMPARISON_GHOST_ALPHA}else{overlay.alpha}, reset, None, true, is_focused && race_audible,
                                ) {
                                    Ok(mut geometry) => {
                                        if clip.comparison.is_some() && !is_focused {
                                            additional.extend(super::render_studio_race::silhouettes(&geometry,overlay.resolved_trail_color()));
                                        }
                                        additional.append(&mut geometry);
                                    },
                                    Err(error) => runtime.error = Some(error),
                                }
                                for impact in source.playback.player_presenter.drain_footsteps() {
                                    if let Some(sound)=source.playback.sound_presenter.as_mut() {
                                        let variant=crate::cgame::footsteps::sound_variant(&impact,actor_time);
                                        let step=crate::cgame::footsteps::material_step(impact.material);
                                        sound.play_footstep(&step.sound.path(impact.foot.heavy(),variant),impact.entity,impact.position);
                                    }
                                }
                                let mut rng=crate::fx::template::Rng::new(actor_time as u64);
                                for mut request in source.playback.player_presenter.drain_fx_requests() {
                                    if let Some(race)=&clip.comparison {
                                        if let Some(runner)=race.runners.iter().find(|r|r.name==overlay.name && r.game==overlay.game) {
                                            if let crate::cgame::player_presenter::PlayerFxRequest::SaberBlade {color,..}=&mut request {
                                                let [r,g,b]=runner.color.map(i32::from);
                                                *color=crate::cgame::SABER_CUSTOM_RGB | r | (g<<8) | (b<<16);
                                            }
                                        }
                                    }
                                    trail_draws.extend(crate::cgame::weapon_fx::profile_saber_blade_draws_for_request(&request,self.video.modern_sabers,&mut rng));
                                }
                                if overlay.strafe_trail {
                                    if let Some(view) = trail_view {
                                        // Comparison ghosts' trails stay readable but recede
                                        // behind the followed runner's.
                                        let trail_alpha=match (clip.comparison.is_some(),is_focused) {
                                            (false,_)=>255,(true,true)=>215,(true,false)=>150,
                                        };
                                        ghost.track.strafe_trail.append_studio_draws(
                                            &self.strafe_trails.settings, view,
                                            overlay.resolved_trail_color(), trail_alpha, overlay.trail_ticks,
                                            overlay.trail_tick_labels,&mut trail_draws,
                                        );
                                    }
                                }
                            }
                        }
                        if !trail_draws.is_empty() {
                            let view=trail_fx_view.as_mut().expect("Studio trail has a view");
                            view.seconds=frame.time;
                            let mut fx_geometry=crate::fx::draw::tessellate(
                                &trail_draws,view,&mut |shader| {
                                    let mut stages = source.playback.entity_presenter.fx_material_stages(shader);
                                    if shader.starts_with("gfx/2d/numbers/") {
                                        // Studio uses the normal number glyph textures, but the
                                        // clip/actor trail owns their RGB *and* alpha. Some
                                        // authored number shaders ignore shaderRGBA by using
                                        // constant generators; override only these Studio draws.
                                        for stage in &mut stages {
                                            stage.rgb_vertex = true;
                                            stage.rgb_one_minus_vertex = false;
                                            stage.rgb_wave = None;
                                            stage.alpha_vertex = true;
                                            stage.alpha_one_minus_vertex = false;
                                            stage.alpha_wave = None;
                                        }
                                    }
                                    stages
                                },
                            );
                            additional.append(&mut fx_geometry);
                        }
                        if !additional.is_empty() {
                            let mut geometry = Vec::with_capacity(frame.dynamic_models.len() + additional.len());
                            geometry.extend(frame.dynamic_models.iter().cloned());
                            geometry.extend(additional);
                            frame.dynamic_models = Arc::new(geometry);
                        }
                    }
                    let mut lights=self.render_studio.timeline.lights_at(time);
                    lights.extend(frame.lights.iter().cloned());
                    frame.lights=Arc::new(lights);
                    frames.push(frame)
                },
                Err(error) => {
                    source.failed = true;
                    self.render_studio.scene_frame = None;
                    self.render_studio.status = error;
                    return;
                }
            }
        }
        let outgoing = frames.remove(0);
        let incoming = frames.pop();
        self.render_studio.demo_preview = Some(StudioDemoPreview {
            name: self.render_studio.timeline.demos[desired[0]].name.clone(),
            game: self.render_studio.timeline.demos[desired[0]].game.clone(),
            active: true,
            failed: false,
        });
        self.render_studio.scene_frame = Some(Arc::new(StudioSceneFrame {
            outgoing,
            incoming,
            effect: blend.map_or(0, |b| b.kind as u32),
            progress: blend.map_or(0.0, |b| b.progress),
            video: self.video,
            offline_capture: self.render_studio.export.is_some() || self.render_studio.still.is_some(),
        }));
        self.render_command(RenderCommand::SetAssetPreviewMode(false));
        if let Some(frame)=dm26_frame { self.record_render_studio_dm26_frame(frame); }
    }
}

/// One native HUD snapshot per recorded source, independent of the editor
/// and gameplay state. Only replay-authoritative player data may be displayed.
/// Studio and export both use this exact snapshot inside StudioSourceFrame.
fn studio_hud_for_source(
    source:&mut StudioDemoSource,
    flags:&super::render_studio_cfg::StudioHudSettings,
    mut video:crate::ui::VideoSettings,
    layout:crate::ui::HudLayout,
    mut crosshair:crate::ui::CrosshairSettings,
    mut helper:crate::ui::StrafeHelperSettings,
    mut keys:crate::ui::MovementKeysSettings,
    mut speedo:crate::speedometer::Settings,
    spec_hud:bool,
    ghost_state:Option<(&jka_protocol::server::PlayerState,i32)>,
    shot:StudioCameraKey,
    camera:&crate::camera::Camera,
    viewport:[u32;2],
)->Option<Arc<crate::ui::UiSnapshot>> {
    use crate::ui::{HudState,UiSnapshot,MovementHudState};
    if !super::render_studio_cfg::HUD_ELEMENTS.iter().any(|&(key,_)| flags.enabled(key)) {
        return None;
    }
    // Before borrowing the session for the HUD: these trace the map collision.
    let recorded=source.playback.current_snapshot.as_ref()
        .and_then(|snap|snap.player_state.field_i32("clientNum")).unwrap_or(-1);
    let viewed=source.last_viewed_client.or_else(||studio_viewed_client(shot,recorded));
    let names=studio_names(source,flags,viewed,camera,viewport);
    let session=&source.playback;
    // DM26 only carries a complete playerState for its recorded POV. Never
    // display the recorder's health/ammo/speed as a different followed actor's.
    let ps=ghost_state.map(|(ps,_)|ps).or_else(||if session.client_game.presentation_player_state().is_some() {
        session.presented_player_state().or_else(||
            session.current_snapshot.as_ref().map(|s|&s.player_state))
    } else {None});
    let mut ui=UiSnapshot::default();
    video.skip_ui=false;video.draw_2d=true;
    // cg_drawScore belongs to the player HUD but can be switched on in Studio
    // independently of the health/armor/force core panels.
    video.draw_hud=flags.enabled("core")||flags.enabled("score");
    // Never pick up editor diagnostics or a gameplay FPS counter.
    video.draw_fps=0;
    ui.video=video;
    ui.hud_layout=layout;
    ui.studio_hud_virtual_layout=true;
    if !flags.enabled("crosshair") {crosshair.style=0;}
    else if crosshair.style==0 {crosshair.style=1;}
    ui.crosshair=crosshair;
    ui.crosshair_target.dynamic=false; // source cameras are resolved, no gameplay late latch
    if !flags.enabled("movement_keys") {keys.mode=0;}
    else if keys.mode==0 {keys.mode=1;}
    ui.movement_keys=keys;
    if !flags.enabled("strafe_helper") {helper.flags=0;}
    else if helper.flags==0 {helper.flags=crate::ui::SHELPER_ORIGINAL;}
    // A movement-helper crosshair must obey the same explicit Studio toggle.
    if !flags.enabled("crosshair") {helper.flags &= !crate::ui::SHELPER_CROSSHAIR;}
    ui.strafe_helper=helper;
    ui.studio_world_labels=names.0;
    if let Some(name)=names.1 {ui.crosshair_target.name=Some(name);ui.studio_crosshair_name=true;}
    // A Studio-only toggle for the real cg_drawScores mini-scoreboard. Keep
    // the Tayst formats (upper-right team line, centred duel boxes), rather
    // than rendering a decorative placeholder or borrowing gameplay CFG state.
    if flags.enabled("mini_scores") {
        let game=&session.client_game;
        let gametype=game.gametype();
        let limits=game.match_limits();
        let score=|value:i32| (value!=-9999).then_some(value);
        if gametype>=6 {
            ui.mini_scores=Some(crate::ui::UiMiniScores::Team {
                mode:2,red:score(limits.scores1),blue:score(limits.scores2),
            });
        } else if gametype==3 && ps.is_some_and(|ps|ps.stats[0]>0) {
            let duelists=game.duelists();
            let d1=usize::try_from(duelists[0]).ok();
            let d2=usize::try_from(duelists[1]).ok();
            if let (Some(d1),Some(d2))=(d1,d2) {
                if let (Some(blue),Some(red))=(
                    game.client_info(d1,&session.siege_classes),
                    game.client_info(d2,&session.siege_classes),
                ) {
                    let icon=|info:&crate::cgame::ClientInfo|format!(
                        "@team-model-icon\t{}\t{}\t{}\t{}\t{}\ngfx/2d/defer",
                        info.model_name,info.skin_name,info.team,info.gametype,
                        u8::from(info.jedi_v_merc),
                    );
                    let clean=|s:&str|crate::logging::strip_jka_colors(s)
                        .chars().take(18).collect::<String>();
                    ui.mini_scores=Some(crate::ui::UiMiniScores::Duel {
                        icon_paths:vec![icon(&blue),icon(&red)],
                        blue:crate::ui::UiDuelMiniScore {
                            name:clean(&blue.name),score:None,model_icon:Some(0),
                        },
                        red:crate::ui::UiDuelMiniScore {
                            name:clean(&red.name),score:None,model_icon:Some(1),
                        },
                    });
                }
            }
        }
    }
    if let Some(ps)=ps {
        let game=&session.client_game;
        let gametype=game.gametype();
        let limits=game.match_limits();
        let in_japro_race=source.game_dir.as_ref().and_then(|dir|dir.file_name())
            .is_some_and(|name|name.to_string_lossy().eq_ignore_ascii_case("japro"))
            && ps.stats[crate::japro_cg::STAT_RACEMODE]!=0;
        if flags.enabled("score") && ps.field_i32("pm_type")!=Some(2)
            && gametype!=4 && !in_japro_race {
            // Same protocol state and game-type gates as gameplay's
            // hud_score_ui; the demo POV is authoritative for its score.
            let score=session.current_snapshot.as_ref()
                .map(|snap|snap.player_state.persistant[0]).unwrap_or(ps.persistant[0]);
            let mut lines=vec![format!("Score: {score}")];
            if gametype==3 && limits.fraglimit>0 {
                if limits.fraglimit>1 {lines[0]=format!("Score: {score}/{}",limits.fraglimit);}
                else if let Some(client)=ps.field_i32("clientNum")
                    .and_then(|num|usize::try_from(num).ok()) {
                    if let Some((wins,losses))=game.client_duel_record(client){
                        lines[0]=format!("{wins}/{losses}");
                    }
                }
            } else if gametype>=6 && gametype!=7 {
                if let Some(client)=ps.field_i32("clientNum")
                    .and_then(|num|usize::try_from(num).ok()) {
                    if let Some(info)=game.client_info(client,&session.siege_classes) {
                        let own=if info.team==1 {limits.scores1}else{limits.scores2};
                        let other=if info.team==1 {limits.scores2}else{limits.scores1};
                        lines.push(format!("Team: {own} ({:+})",own-other));
                    }
                }
            }
            ui.hud_score=Some(crate::ui::UiHudScore{lines,offset:[0.,0.]});
        }
        if flags.enabled("enemy_info") && ps.stats[0]>0 && gametype!=4
            && !in_japro_race {
            const TEAM_SPECTATOR:i32=3;
            let recorded=session.current_snapshot.as_ref().map(|snap|&snap.player_state).unwrap_or(ps);
            let client=recorded.field_i32("clientNum").and_then(|v|usize::try_from(v).ok());
            let suppress=spec_hud && (recorded.persistant[3]==TEAM_SPECTATOR
                || recorded.field_i32("pm_flags").unwrap_or(0)&4096!=0);
            if let Some(viewer)=client.and_then(|client|game.client_info(client,&session.siege_classes))
                .filter(|_|!suppress) {
                let target=if gametype==2 {Some((game.jedi_master(),"Jedi Master".to_owned()))}
                    else if recorded.field_i32("duelInProgress").unwrap_or(0)!=0 {
                        recorded.field_i32("duelIndex").map(|id|(id,"Dueling".to_owned()))
                    } else if gametype==3 && viewer.team!=TEAM_SPECTATOR {
                        let duelists=game.duelists();
                        let me=recorded.field_i32("clientNum");
                        let opponent=if me==Some(duelists[0]){duelists[1]}
                            else if me==Some(duelists[1])||me==Some(duelists[2]){duelists[0]}
                            else {-1};
                        Some((opponent,"Dueling".to_owned()))
                    } else {Some((game.duel_winner(),format!("Leader: {}",limits.scores1)))};
                if let Some((target,title))=target {
                    if gametype==2 && target<0 {
                        ui.enemy_info=Some(crate::ui::UiEnemyInfo{
                            icon_paths:vec!["gfx/hud/w_icon_lightsaber".into()],
                            title:"Get Saber!".into(),name:None,detail:None,health:None});
                    } else if let Some(info)=usize::try_from(target).ok()
                        .filter(|&id|id<32).and_then(|id|game.client_info(id,&session.siege_classes)) {
                        let detail=if gametype==3 && viewer.team!=TEAM_SPECTATOR {
                            let score=session.demo_index.as_ref().and_then(|index| {
                                let elapsed=session.cgame_time.saturating_sub(
                                    index.first_active_server_time.unwrap_or(session.cgame_time));
                                index.scoreboard_at(elapsed).and_then(|sample|
                                    sample.entries.iter().find(|row|row.client==target).map(|row|row.score))
                            });
                            if limits.fraglimit>1 {
                                score.map(|score|format!("Score: {score}/{}",limits.fraglimit))
                            } else if limits.fraglimit==1 {
                                game.client_duel_record(target as usize)
                                    .map(|(wins,losses)|format!("Win/Loss: {wins}/{losses}"))
                            } else {score.map(|score|format!("Score: {score}"))}
                        }else{None};
                        let health=(if game.serverinfo_value(b"g_showDuelHealths")
                            .and_then(|v|v.parse::<i32>().ok()).unwrap_or(0)>=2 {
                            let duelists=game.duelists();
                            let healths=game.duelist_healths();
                            if target==duelists[0]{Some(healths[0])}
                            else if target==duelists[1]{Some(healths[1])}
                            else{None}
                        }else{None}).filter(|&v|v>=0);
                        ui.enemy_info=Some(crate::ui::UiEnemyInfo {
                            icon_paths:vec![format!("@team-model-icon\t{}\t{}\t{}\t{}\t{}",
                                info.model_name,info.skin_name,info.team,info.gametype,u8::from(info.jedi_v_merc))],
                            title,name:Some(info.name),detail,health,
                        });
                    }
                }
            }
        }
        let velocity=["velocity[0]","velocity[1]","velocity[2]"].map(|name|ps.field_f32(name).unwrap_or(0.0));
        let origin=["origin[0]","origin[1]","origin[2]"].map(|name|ps.field_f32(name).unwrap_or(0.0));
        let mut movement=MovementHudState::default();
        movement.velocity=velocity;
        movement.origin=origin;
        movement.view_yaw=ps.field_f32("viewangles[1]").unwrap_or(0.0);
        movement.view_pitch=ps.field_f32("viewangles[0]").unwrap_or(0.0);
        movement.player_speed=ps.field_f32("speed").unwrap_or(250.0);
        movement.grounded=ps.field_i32("groundEntityNum").unwrap_or(1023)!=1023;
        // Same TaystJK spectator command inference used for the gameplay HUD.
        let cmd=crate::strafehelper::spectated_cmd(
            ps.field_i32("movementDir").unwrap_or(-1),velocity,
            ps.field_i32("pm_flags").unwrap_or(0),
            ps.field_i32("eFlags").unwrap_or(0),false);
        movement.forward_move=cmd.forward_move;
        movement.right_move=cmd.right_move;
        movement.up_move=cmd.up_move;
        movement.buttons=cmd.buttons;
        ui.movement_hud=movement;
        if flags.enabled("core") {
            let weapon=ps.field_i32("weapon").unwrap_or(0);
            let ammo=jka_movement::weapon_info(weapon)
                .filter(|(index,_,_)|*index>0)
                .and_then(|(index,_,_)|ps.ammo.get(index as usize).copied());
            ui.hud=Some(HudState {
                health:ps.stats[0],max_health:ps.stats[8].max(1),armor:ps.stats[5],
                force_power:Some(ps.field_i32("fd.forcePower").unwrap_or(0)),
                force_power_max:100,ammo,weapon,
                saber_style:ps.field_i32("fd.saberDrawAnimLevel").unwrap_or(0),
                saber_active_style:ps.field_i32("fd.saberAnimLevel").unwrap_or(0),
                force_flash:false,
            });
        }
        if flags.enabled("speedometer") {
            speedo.flags|=crate::speedometer::flag::ENABLE;
            let v=velocity[0].hypot(velocity[1]);
            let t=ghost_state.map_or(session.cgame_time,|(_,t)|t);
            let speed=crate::speedometer::Input {
                v,vxyz:v.hypot(velocity[2]),vertical:velocity[2],
                wishspeed:movement.player_speed,player_speed:movement.player_speed,
                cgaz_frametime:0.008,on_ground:movement.grounded,
                pm_time:ps.field_i32("pm_time").unwrap_or(0),view_yaw:movement.view_yaw,
                origin,force_jump_z_start:ps.field_f32("fd.forceJumpZStart").unwrap_or(0.0),
                time_ms:t,frame_ms:8,real_ms:i64::from(t),
                lagometer:Default::default(),width_ratio_coef:1.0,
                ..Default::default()
            };
            ui.speedometer=source.studio_speedometer.update(&speedo,&speed);
        }
    }
    ui.speedometer_settings=if flags.enabled("speedometer") {speedo} else {Default::default()};
    if flags.enabled("race_timer") {ui.race_timer=session.race_timer_ui.clone();ui.race_components=[true,false];}
    if let Some((ps,time))=ghost_state {
        if let Some(timer)=ui.race_timer.as_mut() {
            let elapsed=time.saturating_sub(ps.field_i32("duelTime").unwrap_or(time)).max(0);
            timer.timer_text=format!("{}:{:02}.{:03}",elapsed/60000,elapsed/1000%60,elapsed%1000);
        }
    }
    if flags.enabled("game_timer") {
        // The recorded server clock, not the Studio editor's wall clock.
        let ms=session.cgame_time
            .saturating_sub(session.client_game.match_limits().level_start_time)
            .max(0)/1000;
        ui.game_timer=Some(format!("{}:{:02}",ms/60,ms%60));
    }
    if flags.enabled("follow") {
        let client=ghost_state.and_then(|(ps,_)|ps.field_i32("clientNum")).or(viewed);
        ui.follow_name=client.and_then(|id|usize::try_from(id).ok())
            .and_then(|id|session.client_game.client_name(id));
    }
    Some(Arc::new(ui))
}

fn studio_viewed_client(key:StudioCameraKey,recorded:i32)->Option<i32> {
    match key.source {
        StudioCameraSource::Demo {pov:StudioDemoPov::Recorded,..} => (recorded>=0).then_some(recorded),
        StudioCameraSource::Demo {client,..} => {
            let client=if client<0 {recorded}else{client};
            (client>=0).then_some(client)
        }
        _=>None,
    }
}

/// Follow subject -1 means "the recorded player": whoever the demo's POV is
/// on at this frame, so the camera follows the recorder's spectator switches.
pub(in crate::app) fn resolve_recorded_subject(mut key: StudioCameraKey, recorded: i32) -> StudioCameraKey {
    if let StudioCameraSource::Demo { pov: StudioDemoPov::Follow, client } = key.source {
        if client < 0 && recorded >= 0 {
            key.source = StudioCameraSource::Demo { pov: StudioDemoPov::Follow, client: recorded };
        }
    }
    key
}

/// Observer changes presentation only. Captures always use the authored shot.
fn apply_scene_observer(frame:&mut StudioSourceFrame,observer:Option<StudioCameraKey>,offline_capture:bool) {
    if !offline_capture {if let Some(observer)=observer {
        frame.camera=super::render_studio_spatial::camera_from_key(observer);frame.lens.aperture=None;
    }}
}
#[cfg(test)]
mod scene_observer_tests {
    use super::*;
    #[test] fn studio_hud_identity_follows_the_selected_actor_and_recorded_alias() {
        let mut key=StudioCameraKey::default();
        key.source=StudioCameraSource::Demo{pov:StudioDemoPov::Follow,client:7};
        assert_eq!(studio_viewed_client(key,2),Some(7));
        key.source=StudioCameraSource::Demo{pov:StudioDemoPov::Free,client:7};
        assert_eq!(studio_viewed_client(key,2),Some(7));
        key.source=StudioCameraSource::Demo{pov:StudioDemoPov::Follow,client:-1};
        assert_eq!(studio_viewed_client(key,2),Some(2));
        key.source=StudioCameraSource::Demo{pov:StudioDemoPov::Recorded,client:7};
        assert_eq!(studio_viewed_client(key,2),Some(2));
    }
    #[test] fn scene_navigation_does_not_change_exported_camera_or_lens() {
        let shot=crate::camera::Camera::new_with_fov([10.0,20.0,30.0],0.0,80.0);
        let original=StudioSourceFrame{shot_camera:shot,camera:shot,lens:crate::renderer::StudioLens{aperture:Some(2.8),focus_distance:64.0},environment:None,
            id:1,revision:0,time:0.0,player_position:None,area_mask:None,dynamic_models:Arc::new(vec![]),inline_models:Arc::new(vec![]),
            lights:Arc::new(vec![]),screen_fx:Arc::new(vec![]),cloud_foreground:Arc::new(vec![]),ocean_actors:Arc::new(vec![]),hud:None};
        let observer=StudioCameraKey{position:[200.0,80.0,100.0],..Default::default()};
        let mut preview=original.clone();apply_scene_observer(&mut preview,Some(observer),false);
        assert_eq!(preview.camera.position.to_array(),observer.position);assert!(preview.lens.aperture.is_none());
        assert_eq!(preview.shot_camera.position,shot.position);
        let mut export=original.clone();apply_scene_observer(&mut export,Some(observer),true);
        assert_eq!(export.camera.position,shot.position);assert_eq!(export.lens,original.lens);
    }
}

/// Studio crosshair name (the recorded player's real aim) and player names
/// (projected through the shot camera; cinematic: faded near the lens and
/// far away, smaller with distance). Unobstructed players only.
fn studio_names(source:&mut StudioDemoSource,flags:&super::render_studio_cfg::StudioHudSettings,viewed:Option<i32>,
    camera:&crate::camera::Camera,viewport:[u32;2])
    ->(Vec<crate::ui::UiStudioLabel>,Option<crate::ui::UiCrosshairName>) {
    use jka_movement::TraceWorld;
    let names=flags.enabled("player_names")||flags.enabled("player_names_cinematic");
    let cinematic=flags.enabled("player_names_cinematic");
    let aim=flags.enabled("crosshair_name");
    let mut labels=Vec::new();
    let mut crosshair_name=None;
    if !names && !aim {return (labels,crosshair_name);}
    let session=&source.playback;
    let Some(snapshot)=session.current_snapshot.as_ref() else {return (labels,crosshair_name)};
    let recorded=snapshot.player_state.field_i32("clientNum").unwrap_or(-1);
    let hidden=session.demo_hidden_view_client;
    const EF_DEAD:i32=1<<1;
    let players:Vec<(i32,[f32;3],String)>=session.presented_entities.iter()
        .filter(|e|e.entity_type==crate::cgame::ET_PLAYER && e.number<32
            && e.state.field_i32("eFlags").unwrap_or(0)&EF_DEAD==0)
        .filter_map(|e|{
            let client=i32::from(e.number);
            let name=session.client_game.client_name(usize::from(e.number)).filter(|n|!n.trim().is_empty())?;
            Some((client,e.origin,name))
        }).collect();
    let mut clear=|from:[f32;3],to:[f32;3]|->bool {
        let Some(world)=source.collision.as_mut() else {return true};
        let t=world.trace(jka_movement::TraceQuery{start:from,end:to,mins:[0.;3],maxs:[0.;3],pass_entity:-1,mask:0x1|0x10000});
        t.start_solid==0 && t.fraction>=0.99
    };
    if aim {
        // CG_DrawCrosshairNames: along the recorded view, the nearest player
        // whose body the line passes within ~20 units of.
        let ps=&snapshot.player_state;
        let origin=[ps.field_f32("origin[0]").unwrap_or(0.0),ps.field_f32("origin[1]").unwrap_or(0.0),ps.field_f32("origin[2]").unwrap_or(0.0)];
        let eye=glam::Vec3::from_array(origin)+glam::Vec3::Z*ps.field_i32("viewheight").unwrap_or(36) as f32;
        let (pitch,yaw)=(ps.field_f32("viewangles[0]").unwrap_or(0.0).to_radians(),ps.field_f32("viewangles[1]").unwrap_or(0.0).to_radians());
        let dir=glam::Vec3::new(pitch.cos()*yaw.cos(),pitch.cos()*yaw.sin(),-pitch.sin());
        let target=players.iter().filter(|p|p.0!=recorded).filter_map(|(_,o,name)|{
            let chest=glam::Vec3::from_array(*o)+glam::Vec3::Z*16.0;
            let along=(chest-eye).dot(dir);
            if along<=0.0||along>8192.0 {return None;}
            let miss=(chest-(eye+dir*along)).length();
            (miss<20.0).then_some((along,chest,name.clone()))
        }).min_by(|a,b|a.0.total_cmp(&b.0));
        if let Some((_,chest,name))=target {
            if clear(eye.to_array(),chest.to_array()) {
                crosshair_name=Some(crate::ui::UiCrosshairName{text:name,color:[1.0,1.0,1.0],alpha:1.0});
            }
        }
    }
    if names {
        let view_proj=camera.view_projection(viewport[0],viewport[1]);
        let eye_native=scene::jka_position(camera.position.to_array());
        for (client,origin,name) in &players {
            if Some(*client)==hidden || Some(*client)==viewed {continue;}
            let head=[origin[0],origin[1],origin[2]+64.0];
            let render=glam::Vec3::from_array(scene::render_position(head));
            let clip=view_proj*render.extend(1.0);
            if clip.w<=0.001 {continue;}
            let ndc=clip.truncate()/clip.w;
            if ndc.x.abs()>1.05||ndc.y.abs()>1.05||!(0.0..=1.0).contains(&ndc.z) {continue;}
            let distance=camera.position.distance(render);
            let (alpha,scale)=if cinematic {
                let smooth=|a:f32,b:f32,x:f32|{let t=((x-a)/(b-a)).clamp(0.0,1.0);t*t*(3.0-2.0*t)};
                // Fade out right against the lens (covering the view) and far away.
                (smooth(48.0,160.0,distance)*(1.0-smooth(1400.0,2600.0,distance)),(1.15-distance/4000.0).clamp(0.55,1.0))
            } else {(1.0,1.0)};
            if alpha<=0.02 {continue;}
            if !clear(eye_native,[origin[0],origin[1],origin[2]+40.0]) {continue;}
            labels.push(crate::ui::UiStudioLabel{color:[1.0;4],pos:[ndc.x*0.5+0.5,0.5-ndc.y*0.5],
                text:name.clone(),alpha,scale});
        }
    }
    (labels,crosshair_name)
}
