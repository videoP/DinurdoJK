//! Project camera attachments and deterministic, timeline-based damping.
use super::{App, FrontendPage};
use super::render_studio_timeline::{StudioCameraKey, StudioInterpolation};
use crate::cgame::{ClientInfo, player_presenter::ProfileAttachmentFrame};
use std::collections::BTreeMap;

pub(in crate::app) use jka_studio::render_studio_camera::*;

#[derive(Default)]
pub(in crate::app) struct StudioCameraRigRuntime {
    signature: String,
    sample_signature: String,
    evaluated_at: Option<f64>,
    samples: [BTreeMap<i64,ProfileAttachmentFrame>;2],
    pub camera: Option<StudioCameraKey>,
    pub frames: [Option<ProfileAttachmentFrame>;2],
    pub messages: [Option<String>;2],
}

/// Finite exponential filter on an absolute 120 Hz timeline grid. The tail is
/// initialized at eight half-lives (or the clip's start). Fractional intervals
/// use their exact exponential weight. No UI clock, previous frame or export
/// FPS participates, so seeking and out-of-order still captures are identical.
fn damped_point(time:f64,start:f64,half_life:f32,mut point:impl FnMut(f64)->Result<[f32;3],String>)
    ->Result<[f32;3],String> {
    if half_life<=0. {return point(time);}
    let begin=(time-f64::from(half_life)*8.).max(start);
    let mut value=glam::DVec3::from_array(point(begin)?.map(f64::from));
    let mut previous=begin;
    let first=(begin*120.).floor() as i64+1;
    let last=(time*120.).floor() as i64;
    for tick in first..=last {
        let t=tick as f64/120.;
        let p=glam::DVec3::from_array(point(t)?.map(f64::from));
        let alpha=1.-(-std::f64::consts::LN_2*(t-previous)/f64::from(half_life)).exp();
        value=value.lerp(p,alpha);previous=t;
    }
    if time>previous {
        let p=glam::DVec3::from_array(point(time)?.map(f64::from));
        let alpha=1.-(-std::f64::consts::LN_2*(time-previous)/f64::from(half_life)).exp();
        value=value.lerp(p,alpha);
    }
    Ok(value.as_vec3().to_array())
}

impl App {
    /// Editor queries use their own pose cache and never consume a pending view
    /// match, seek playback or overwrite the shot's current runtime camera.
    pub(in crate::app) fn studio_orbit_edit_frame(&mut self,time:f64)->Option<ProfileAttachmentFrame> {
        let lane=self.render_studio.timeline.camera_rig.position.lane;
        // Camera keys may sit just before/after a clip. Use its nearest valid
        // pose for editing the attachment parameters without seeking playback.
        let time=if self.render_studio.timeline.clip_at_lane(time,lane).is_some() {time} else {
            self.render_studio.timeline.clips.iter().filter(|c|c.lane==lane)
                .map(|c|time.clamp(c.start,(c.end()-0.0001).max(c.start)))
                .min_by(|a,b|(a-time).abs().total_cmp(&(b-time).abs())).unwrap_or(time)
        };
        let sample=self.render_studio.preview_sample_time;
        let pending=self.render_studio.camera_pending_match.take();
        let editor=std::mem::take(&mut self.render_studio.camera_orbit_edit_runtime);
        let runtime=std::mem::replace(&mut self.render_studio.camera_rig_runtime,editor);
        self.render_studio.preview_sample_time=Some(time);self.update_studio_camera_rig();
        let frame=self.render_studio.camera_rig_runtime.frames[0];
        self.render_studio.camera_orbit_edit_runtime=std::mem::replace(&mut self.render_studio.camera_rig_runtime,runtime);
        self.render_studio.preview_sample_time=sample;self.render_studio.camera_pending_match=pending;frame
    }
    pub(in crate::app) fn studio_orbit_key_value(&mut self,key:StudioCameraKey)->Option<StudioCameraOrbit> {
        let frame=self.studio_orbit_edit_frame(key.time)?;
        let rig=&self.render_studio.timeline.camera_rig;
        let mut binding=rig.position.clone();
        if let Some(orbit)=key.rig_orbit {orbit.apply(&mut binding,frame,[0.;3]);}
        StudioCameraOrbit::from_binding(&binding,frame,std::array::from_fn(|a|key.position[a]-rig.reference_position[a]))
    }
    pub(in crate::app) fn set_studio_orbit_key(&mut self,time:f64,orbit:StudioCameraOrbit)->bool {
        if !orbit.valid() {return false;}
        let Some(fallback)=self.studio_orbit_edit_frame(time) else{return false;};
        // Seed every existing key before the first keyed edit. Otherwise the
        // first authored orbit would be held across earlier legacy keys too.
        let mut keys=self.render_studio.timeline.camera.clone();
        let rig=self.render_studio.timeline.camera_rig.clone();
        for key in &mut keys {
            if key.rig_orbit.is_some() {continue;}
            let frame=self.studio_orbit_edit_frame(key.time);
            let active=self.render_studio.timeline.clip_at_lane(key.time,rig.position.lane).is_some();
            let delta=if active {std::array::from_fn(|a|key.position[a]-rig.reference_position[a])}else{[0.;3]};
            let Some(value)=StudioCameraOrbit::from_binding(&rig.position,frame.unwrap_or(fallback),delta) else{return false;};
            key.rig_orbit=Some(value);
            if active {key.position=rig.reference_position;}
        }
        self.render_studio.timeline.camera=keys;
        let mut key=self.render_studio.timeline.camera_at(time);key.time=time;
        key.position=rig.reference_position;key.rig_orbit=Some(orbit);
        self.render_studio.timeline.insert_camera_key(key);self.render_studio.timeline.seek(time);
        self.update_studio_camera_rig();self.profile_preview_key=None;true
    }
    pub(in crate::app) fn convert_studio_orbit_space(&mut self,local:bool) {
        let old=self.render_studio.timeline.camera_rig.position.clone();
        if old.local_offset==local {return;}
        let keys=self.render_studio.timeline.camera.clone();
        for (i,key) in keys.into_iter().enumerate() {
            let Some(orbit)=key.rig_orbit else{continue;};
            let Some(frame)=self.studio_orbit_edit_frame(key.time) else{continue;};
            let mut binding=old.clone();if !orbit.apply(&mut binding,frame,[0.;3]) {continue;}
            let point=frame_point(frame,binding.offset,old.local_offset);
            binding.local_offset=local;binding.offset=frame_offset(frame,point,local);
            if let Some(value)=StudioCameraOrbit::from_binding(&binding,frame,[0.;3]) {
                self.render_studio.timeline.camera[i].rig_orbit=Some(value);
            }
        }
    }
    pub(in crate::app) fn studio_attachment_info(&self,lane:usize)->ClientInfo {
        let mut info=self.solo_client_info.clone();
        let staged=ClientInfo::solo_model(self.render_studio.actor_model(lane));
        info.model_name=staged.model_name;info.skin_name=staged.skin_name;
        info.female=self.network.sex.as_bytes().first().is_some_and(|b|b.eq_ignore_ascii_case(&b'f'));
        info.saber_name=self.network.saber1.clone();info.saber2_name=self.network.saber2.clone();
        self.render_studio.apply_actor_sabers(lane,&mut info);
        info
    }

    pub(in crate::app) fn studio_profile_sabers(&self) -> super::render_studio_timeline::StudioSabers {
        super::render_studio_timeline::StudioSabers {
            enabled: self.render_studio.with_sabers,
            names: [self.network.saber1.clone(), self.network.saber2.clone()],
            colors: [crate::cgame::resolve_saber_color(i32::from(self.network.color1),self.network.sb_rgb1 as i32,true),
                crate::cgame::resolve_saber_color(i32::from(self.network.color2),self.network.sb_rgb2 as i32,true)],
        }
    }

    pub(in crate::app) fn set_studio_actor_sabers_enabled(&mut self,lane:usize,enabled:bool) {
        let mut sabers=self.render_studio.timeline.lane_sabers.get(lane).and_then(Option::as_ref)
            .cloned().unwrap_or_else(||self.studio_profile_sabers());
        sabers.enabled=enabled;
        self.render_studio.timeline.lane_sabers.resize(self.render_studio.timeline.lanes,None);
        if let Some(slot)=self.render_studio.timeline.lane_sabers.get_mut(lane) {*slot=Some(sabers);}
        self.profile_preview_key=None;
    }

    pub(in crate::app) fn studio_attachment_frame(&mut self,binding:&StudioCameraAttachment,time:f64)
        ->Result<ProfileAttachmentFrame,String> {
        let clip=self.render_studio.timeline.clip_at_lane(time,binding.lane)
            .ok_or_else(||"No active clip on this character layer".to_owned())?;
        if binding.part.saber().is_some() && !self.render_studio.actor_with_sabers(binding.lane) {
            return Err("Enable Sabers on this character track".into());
        }
        let motion=clip.motion_at(time,self.render_studio.timeline.interpolation!=StudioInterpolation::Linear);
        let name=clip.name.clone();let emote=clip.preview(time);
        let range=Some((clip.first_frame,clip.end_frame));let ms=clip.pose_time_ms(time);
        let yaw=(self.profile_preview_yaw+motion.yaw).to_radians();let (s,c)=yaw.sin_cos();
        let axis=[[c,s,0.],[-s,c,0.],[0.,0.,1.]];
        let origin=[88.*self.profile_preview_zoom.clamp(0.45,2.5)+motion.position[0],
            binding.lane as f32*26.+motion.position[1],-22.+motion.position[2]];
        let info=self.studio_attachment_info(binding.lane);
        self.ensure_asset_preview_player_presenter()?;
        self.asset_preview_player_presenter.as_mut().ok_or("Player presenter unavailable")?
            .sample_profile_attachment(&info,origin,axis,ms,&name,self.profile_studio_head_look,
                emote.as_ref(),range,binding.part.saber(),&binding.bone)?
            .ok_or_else(||format!("Attachment {} is unavailable on {}",binding.bone,binding.part.label()))
    }

    pub(in crate::app) fn update_studio_camera_rig(&mut self) {
        if self.frontend_page!=FrontendPage::RenderStudio || self.render_studio.cinematic_preview.is_some() {return;}
        let rig=self.render_studio.timeline.camera_rig.clone();
        if !rig.active() {self.render_studio.camera_rig_runtime=Default::default();return;}
        let time=self.render_studio.timeline_time();
        let mut base=self.render_studio.timeline.camera_at(time);
        let model_ids=[&rig.position,&rig.target].map(|binding| {
            if !binding.enabled {return None;}
            let info=self.studio_attachment_info(binding.lane);
            self.ensure_asset_preview_player_presenter().ok()?;
            self.asset_preview_player_presenter.as_mut()?.profile_attachment_model_id(&info).ok()
        });
        let snapshot=self.render_studio.edit_snapshot();
        let sample_context=format!("{:?}",
            (self.profile_preview_zoom,self.profile_preview_yaw,self.profile_studio_head_look,
             &self.network.saber1,&self.network.saber2,model_ids));
        let signature=snapshot.signature()+sample_context.as_str();
        let sample_signature=snapshot.camera_pose_signature()+sample_context.as_str();
        let mut runtime=std::mem::take(&mut self.render_studio.camera_rig_runtime);
        if runtime.signature!=signature {runtime.signature=signature;runtime.evaluated_at=None;}
        if runtime.sample_signature!=sample_signature {
            runtime.samples=Default::default();runtime.sample_signature=sample_signature;
        }
        if runtime.evaluated_at==Some(time) && runtime.messages.iter().all(Option::is_none)
            && self.render_studio.camera_pending_match.is_none() {
            self.render_studio.camera_rig_runtime=runtime;return;
        }
        let mut points=[None;2];
        for (i,binding) in [&rig.position,&rig.target].into_iter().enumerate() {
            runtime.messages[i]=None;runtime.frames[i]=None;
            if !binding.enabled {continue;}
            let frame=match self.studio_attachment_frame(binding,time) {
                Ok(frame)=>frame,
                Err(error)=>{runtime.messages[i]=Some(error);continue;}
            };
            let start=self.render_studio.timeline.clip_at_lane(time,binding.lane).map_or(time,|clip|clip.start);
            let cache=&mut runtime.samples[i];
            let begin=(time-f64::from(binding.half_life)*8.).max(start);
            let initial=if (begin-time).abs()<1e-8 {frame} else {
                match self.studio_attachment_frame(binding,begin) {
                    Ok(frame)=>frame,Err(error)=>{runtime.messages[i]=Some(error);continue;}
                }
            };
            let mut sample=|t:f64| -> Result<ProfileAttachmentFrame,String> {
                if (t-time).abs()<1e-8 {return Ok(frame);}
                if (t-begin).abs()<1e-8 {return Ok(initial);}
                let tick=(t*120.).round() as i64;
                let on_grid=(t*120.-tick as f64).abs()<1e-7;
                let sample=if on_grid {
                    if let Some(frame)=cache.get(&tick) {*frame} else {
                        let frame=self.studio_attachment_frame(binding,t)?;cache.insert(tick,frame);frame
                    }
                } else {self.studio_attachment_frame(binding,t)?};
                Ok(sample)
            };
            let filtered=(||->Result<ProfileAttachmentFrame,String> {
                let position=damped_point(time,start,binding.half_life,|t|Ok(sample(t)?.position))?;
                let mut axis=[[0.;3];3];
                for (j,axis) in axis.iter_mut().enumerate() {
                    *axis=damped_point(time,start,binding.half_life,|t|Ok(sample(t)?.axis[j]))?;
                }
                Ok(ProfileAttachmentFrame {position,axis})
            })();
            match filtered {
                Ok(frame)=>{points[i]=Some(frame_point(frame,binding.offset,binding.local_offset));runtime.frames[i]=Some(frame);}
                Err(error)=>runtime.messages[i]=Some(error),
            }
            // Bound memory, including after long exports and arbitrary seeks.
            let lo=((time-f64::from(binding.half_life)*8.-1.)*120.).floor() as i64;
            let hi=(time*120.).ceil() as i64+1;
            cache.retain(|tick,_|*tick>=lo && *tick<=hi);
        }
        if let Some(frame)=runtime.frames[0] {points[0]=Some(rig.position_point(base,frame));}
        let mut rig=rig;
        if let Some(eye)=self.render_studio.camera_pending_match {
            if let Some(frame)=runtime.frames[0] {
                if !rig.position.local_offset || glam::Mat3::from_cols_array_2d(&frame.axis).determinant().abs()>1e-8 {
                    if base.rig_orbit.is_some() {
                        let mut binding=rig.position.clone();binding.offset=frame_offset(frame,eye,binding.local_offset);
                        base.rig_orbit=StudioCameraOrbit::from_binding(&binding,frame,[0.;3]);
                        base.position=rig.reference_position;base.time=time;
                        self.render_studio.timeline.insert_camera_key(base);
                    } else {
                        rig.reference_position=base.position;
                        rig.position.offset=frame_offset(frame,eye,rig.position.local_offset);
                    }
                    self.render_studio.timeline.camera_rig=rig.clone();
                    self.render_studio.camera_pending_match=None;
                    points[0]=Some(eye);
                } else {runtime.messages[0]=Some("Bone rotates too far during damping; use World offsets or less damping".into());}
            }
        }
        if let Some(eye)=self.render_studio.camera_pending_match {base.position=eye;points[0]=None;}
        let camera=rig.resolve(base,points[0],points[1]);
        runtime.camera=Some(camera);
        runtime.evaluated_at=Some(time);
        self.render_studio.camera_rig_runtime=runtime;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn studio_keyed_orbit_interpolates_around_a_moving_head_at_constant_distance() {
        use super::super::render_studio_timeline::StudioTimeline;
        let mut timeline=StudioTimeline::default();
        let first=StudioCameraKey {position:[0.;3],rig_orbit:Some(StudioCameraOrbit {distance:120.,yaw:0.,pitch:0.}),..Default::default()};
        let mut second=first;second.time=4.;second.fov=82.;second.rig_orbit.as_mut().unwrap().yaw=90.;
        timeline.camera=vec![first,second];
        let rig=StudioCameraRig {position:StudioCameraAttachment {enabled:true,..Default::default()},
            target:StudioCameraAttachment {enabled:true,..Default::default()},..Default::default()};
        for mode in [StudioInterpolation::Linear,StudioInterpolation::Smooth,StudioInterpolation::Spline] {
            timeline.interpolation=mode;
            for time in [0.,0.5,1.,2.,3.,4.] {
                let key=timeline.camera_at(time);
                let head=[time as f32*20.,40.,-15.];
                let frame=ProfileAttachmentFrame {position:head,axis:[[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]]};
                let native_head=crate::scene::jka_position(head);
                let camera=rig.resolve(key,Some(rig.position_point(key,frame)),Some(native_head));
                let vector=glam::Vec3::from_array(camera.position)-glam::Vec3::from_array(native_head);
                assert!((vector.length()-120.).abs()<0.001,"camera must orbit instead of cutting a chord");
                if time==2. {assert!((vector-glam::Vec3::new(84.85281,84.85281,0.)).length()<0.001);assert_eq!(key.fov,70.);}
                assert!(super::super::render_studio_spatial::camera_from_key(camera).forward()
                    .dot(-glam::Vec3::from_array(crate::scene::render_position(vector.to_array())).normalize())>0.9999,"camera must keep aiming at the moving head");
            }
            let quarter=timeline.camera_at(1.).rig_orbit.unwrap().yaw;
            assert!((quarter-if mode==StudioInterpolation::Smooth {14.0625}else{22.5}).abs()<0.001);
        }
        assert_eq!(timeline.camera[0].rig_orbit.unwrap().yaw,0.);
        timeline.camera[0].rig_orbit.as_mut().unwrap().yaw=170.;timeline.camera[1].rig_orbit.as_mut().unwrap().yaw=-170.;
        assert!((timeline.camera_at(2.).rig_orbit.unwrap().yaw.abs()-180.).abs()<0.001);
    }
    #[test]
    fn studio_keyed_orbit_copy_edit_save_and_undo_preserve_the_first_camera_key() {
        let mut studio=super::super::render_studio::RenderStudio::default();
        studio.timeline.camera[0].rig_orbit=Some(StudioCameraOrbit {distance:90.,yaw:15.,pitch:12.});
        studio.timeline.selected_camera=Some(0);assert!(studio.copy_selection());studio.timeline.seek(4.);assert!(studio.paste_selection());
        assert_eq!(studio.timeline.camera[1].rig_orbit,studio.timeline.camera[0].rig_orbit);
        studio.init_history();studio.timeline.camera[1].rig_orbit.as_mut().unwrap().yaw=95.;studio.track_edit(false);
        assert_eq!(studio.timeline.camera[0].rig_orbit.unwrap().yaw,15.);
        assert!((studio.timeline.camera_at(2.).rig_orbit.unwrap().yaw-55.).abs()<0.001);
        let saved=serde_json::to_string(&studio.timeline.camera).unwrap();
        let restored:Vec<StudioCameraKey>=serde_json::from_str(&saved).unwrap();assert_eq!(restored[1].rig_orbit.unwrap().yaw,95.);
        assert!(studio.undo_edit());assert_eq!(studio.timeline.camera[1].rig_orbit.unwrap().yaw,15.);
        assert!(studio.redo_edit());assert_eq!(studio.timeline.camera[1].rig_orbit.unwrap().yaw,95.);
        let legacy:StudioCameraKey=serde_json::from_str(r#"{"time":0,"position":[0,-11,0],"yaw":0,"pitch":5,"fov":58}"#).unwrap();
        assert!(legacy.rig_orbit.is_none());
    }
    #[test]
    fn studio_orbit_distance_and_angles_control_the_resolved_eye_with_keyed_movement() {
        let frame=ProfileAttachmentFrame {position:[30.,40.,50.],axis:[[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]]};
        let mut binding=StudioCameraAttachment {enabled:true,offset:[100.,20.,-15.],..Default::default()};
        let delta=[10.,-5.,25.];let before=frame_point(frame,binding.offset,false);
        let original=StudioCameraOrbit::from_binding(&binding,frame,delta).unwrap();
        assert!(original.apply(&mut binding,frame,delta));
        assert!((glam::Vec3::from_array(frame_point(frame,binding.offset,false))-glam::Vec3::from_array(before)).length()<0.001);
        let orbit=StudioCameraOrbit {distance:120.,yaw:90.,pitch:30.};
        assert!(orbit.apply(&mut binding,frame,delta));
        let rig=StudioCameraRig {position:binding,target:Default::default(),reference_position:[0.;3]};
        let key=StudioCameraKey {position:delta,fov:77.,..Default::default()};
        let eye=rig.resolve(key,Some(frame_point(frame,rig.position.offset,false)),None);
        let vector=glam::Vec3::from_array(eye.position)-glam::Vec3::from_array(crate::scene::jka_position(frame.position));
        assert!((vector-glam::Vec3::new(0.,103.92305,-60.)).length()<0.001);assert_eq!(eye.fov,77.);
        let saved:StudioCameraRig=serde_json::from_str(&serde_json::to_string(&rig).unwrap()).unwrap();
        assert_eq!(saved,rig,"orbit controls must persist through the existing offset format");
    }
    #[test]
    fn studio_orbit_preserves_physical_radius_with_rotated_damped_bone_axes() {
        let frame=ProfileAttachmentFrame {position:[5.,-4.,10.],axis:[[0.,0.,-0.7],[0.8,0.1,0.],[0.1,0.9,0.2]]};
        let mut binding=StudioCameraAttachment {local_offset:true,offset:[30.,-70.,20.],..Default::default()};
        let initial=binding.offset;let delta=[17.,5.,-11.];
        let original=StudioCameraOrbit::from_binding(&binding,frame,delta).unwrap();
        assert!(original.apply(&mut binding,frame,delta));
        assert!((glam::Vec3::from_array(binding.offset)-glam::Vec3::from_array(initial)).length()<0.001);
        for (yaw,pitch) in [(-150.,-30.),(0.,0.),(75.,55.),(180.,89.9)] {
            let orbit=StudioCameraOrbit {distance:150.,yaw,pitch};assert!(orbit.apply(&mut binding,frame,delta));
            let result=StudioCameraOrbit::from_binding(&binding,frame,delta).unwrap();
            assert!((result.distance-150.).abs()<0.001);
            assert!(((result.yaw-yaw+180.).rem_euclid(360.)-180.).abs()<0.01);
            assert!((result.pitch-pitch).abs()<0.01);
        }
        let singular=ProfileAttachmentFrame {axis:[[0.;3];3],..frame};
        assert!(StudioCameraOrbit::from_binding(&binding,singular,delta).is_none());
        let zero=StudioCameraOrbit {distance:0.,yaw:30.,pitch:10.};assert!(zero.apply(&mut binding,frame,delta));
        assert!(StudioCameraOrbit::from_binding(&binding,frame,delta).unwrap().distance<0.001);
    }
    #[test]
    fn studio_rig_tracks_actual_eye_preserves_keys_and_handles_coincident_target() {
        let rig=StudioCameraRig {position:StudioCameraAttachment {enabled:true,..Default::default()},
            target:StudioCameraAttachment {enabled:true,..Default::default()},reference_position:[1.,2.,3.]};
        let base=StudioCameraKey {position:[11.,2.,3.],fov:73.,..Default::default()};
        let camera=rig.resolve(base,Some([40.,5.,6.]),Some([50.,15.,6.]));
        assert_eq!(camera.position,[50.,5.,6.]);assert_eq!(camera.fov,73.);
        assert_eq!(camera.yaw,90.);assert_eq!(camera.pitch,0.);
        let camera=rig.resolve(base,Some([40.,5.,6.]),Some([50.,5.,6.]));
        assert_eq!(camera.yaw,base.yaw);assert_eq!(camera.pitch,base.pitch);
        assert_eq!(rig.resolve(base,None,None).position,base.position);
    }
    #[test]
    fn studio_rig_damping_is_causal_and_independent_of_frame_order() {
        let sample=|t:f64|Ok([if t>=1. {10.}else{0.},(t*3.).sin() as f32,0.]);
        let expected=damped_point(1.5,0.,0.25,sample).unwrap();
        assert!((expected[0]-7.5).abs()<0.1,"half-life response {expected:?}");
        for t in [4.,1.,0.,2.,1.5] {let _=damped_point(t,0.,0.25,sample).unwrap();}
        assert_eq!(damped_point(1.5,0.,0.25,sample).unwrap(),expected);
        assert_eq!(damped_point(1.5,0.,0.,sample).unwrap(),sample(1.5).unwrap());
        assert_eq!(damped_point(5.,5.,0.5,|_|Ok([7.;3])).unwrap(),[7.;3]);
    }
    #[test]
    fn studio_rig_serialization_and_offset_axes_round_trip() {
        let rig:StudioCameraRig=serde_json::from_str("{}").unwrap();assert!(!rig.active());
        let rig:StudioCameraRig=serde_json::from_str(&serde_json::to_string(&rig).unwrap()).unwrap();
        assert!(rig.valid(1));
        let frame=ProfileAttachmentFrame {position:[4.,5.,6.],axis:[[0.,1.,0.],[-1.,0.,0.],[0.,0.,1.]]};
        let offset=[10.,2.,3.];let p=frame_point(frame,offset,true);
        assert_eq!(p,[2.,-9.,15.]);assert_eq!(frame_offset(frame,p,true),offset);
        assert_eq!(frame_offset(frame,frame_point(frame,offset,false),false),offset);
        let project:super::super::render_studio_timeline::StudioProject=serde_json::from_value(serde_json::json!({
            "version":3,"model":"kyle/default","with_sabers":true,"background":"black",
            "background_map":"","clips":[],"camera":[]
        })).unwrap();
        assert!(!project.camera_rig.active(),"older projects must keep their free camera");
    }
    #[test]
    fn studio_rig_changes_undo_and_redo_as_a_scene_edit() {
        let mut studio=super::super::render_studio::RenderStudio::default();
        studio.init_history();
        studio.timeline.camera_rig.position.enabled=true;
        studio.timeline.camera_rig.position.bone="*r_hand".into();
        studio.timeline.camera_rig.position.half_life=0.25;
        studio.timeline.camera_rig.reference_position=[20.,30.,40.];
        let expected=studio.timeline.camera_rig.clone();studio.track_edit(false);
        assert!(studio.undo_edit());assert!(!studio.timeline.camera_rig.active());
        assert!(studio.redo_edit());assert_eq!(studio.timeline.camera_rig,expected);
        let restored:StudioCameraRig=serde_json::from_str(&serde_json::to_string(&expected).unwrap()).unwrap();
        assert_eq!(restored,expected);
    }
}
