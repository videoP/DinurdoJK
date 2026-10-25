//! Spatial keyframe coordinates and drag constraints, independent of UI layout.
use super::{App,render_studio_timeline::{StudioItem,StudioCameraKey,StudioTimeline,StudioInterpolation,StudioAnimationClip,StudioClipPlayback}};
use crate::camera::Camera;
use glam::Vec3;
use std::collections::BTreeMap;

#[derive(Clone,Debug)]
pub(in crate::app) struct StudioSpatialMarker {
    pub item: StudioItem,
    pub number: usize,
    pub time: f64,
    pub position: [f32;3],
    pub lane: Option<usize>,
}

pub(in crate::app) struct StudioMotionTick {
    pub second: i64,
    pub position: [f32;3],
    pub tangent: [f32;3],
}
pub(in crate::app) struct StudioMotionPath {
    pub lane: Option<usize>,
    pub points: Vec<[f32;3]>,
    pub ticks: Vec<StudioMotionTick>,
    /// Do not connect across a cinematic's loop reset/teleport.
    pub breaks: Vec<usize>,
}

#[derive(Default)]
struct StudioPathPoseCache {
    // The pelvis carries cinematic translation even when model_root stays put.
    bone: String,
    samples: BTreeMap<i32,[f32;3]>,
}
enum PathSampleError { Pending, Asset(String) }

/// Sample the rendered pelvis, including movement authored inside the animation,
/// then apply the same stage origin, lane, yaw and motion offsets as the actor.
/// The caller caches pose queries independently of these editable transforms.
fn motion_path<E>(clip:&StudioAnimationClip,smooth:bool,zoom:f32,yaw:f32,
    mut pose:impl FnMut(f64)->Result<[f32;3],E>)->Result<StudioMotionPath,E> {
    let offset=(clip.first_frame-clip.source_first) as f64/clip.source_fps.max(1.);
    let mut times=vec![clip.start,clip.end()];
    // Dense sampling follows curved skeletal movement and rotation. Bound the
    // density for exceptionally long clips; whole-second ticks remain exact.
    let steps=(clip.duration()*30.).ceil().clamp(1.,12000.) as usize;
    times.extend((1..steps).map(|i|clip.start+clip.duration()*i as f64/steps as f64));
    times.extend(clip.motion.iter().map(|k|clip.start+k.time-offset)
        .filter(|&t|t>clip.start && t<clip.end()));
    times.extend((clip.start.ceil() as i64..=clip.end().floor() as i64).map(|s|s as f64));
    if clip.playback==StudioClipPlayback::Loop {
        let duration=clip.source_duration();
        for i in 1..=(clip.duration()/duration).floor() as usize {
            let time=clip.start+i as f64*duration;
            times.extend([(time-0.001).max(clip.start),time.min(clip.end())]);
        }
    }
    times.sort_by(f64::total_cmp);times.dedup_by(|a,b|(*a-*b).abs()<1e-8);
    let mut points=Vec::with_capacity(times.len());let mut breaks=Vec::new();let mut previous_ms=None;
    for &time in &times {
        let motion=clip.motion_at(time,smooth);
        let rotation=glam::Quat::from_rotation_y((yaw+motion.yaw).to_radians());
        let animated=rotation*Vec3::from_array(pose(time)?);
        points.push((Vec3::from_array(motion_world(motion.position,clip.lane,zoom))+animated).to_array());
        let ms=clip.pose_time_ms(time);
        if previous_ms.is_some_and(|previous|ms<previous) {breaks.push(points.len()-1);}
        previous_ms=Some(ms);
    }
    Ok(path_with_ticks(&times,points,breaks,Some(clip.lane)))
}
fn path_with_ticks(times:&[f64],points:Vec<[f32;3]>,breaks:Vec<usize>,lane:Option<usize>)->StudioMotionPath {
    let mut ticks=Vec::new();
    for (i,&time) in times.iter().enumerate() {
        if (time-time.round()).abs()>1e-8 {continue;}
        let previous=if i>0 && breaks.binary_search(&i).is_err() {i-1}else{i};
        let next=if i+1<points.len() && breaks.binary_search(&(i+1)).is_err() {i+1}else{i};
        let tangent=(Vec3::from_array(points[next])-Vec3::from_array(points[previous])).normalize_or_zero();
        ticks.push(StudioMotionTick {second:time.round() as i64,position:points[i],tangent:tangent.to_array()});
    }
    StudioMotionPath {lane,points,ticks,breaks}
}
fn camera_path<E>(timeline:&StudioTimeline,mut position:impl FnMut(f64)->Result<[f32;3],E>)
    ->Result<Option<StudioMotionPath>,E> {
    let (Some(first),Some(last))=(timeline.camera.first(),timeline.camera.last()) else{return Ok(None);};
    if first.time>=last.time {return Ok(None);}
    let duration=last.time-first.time;
    let steps=(duration*30.).ceil().clamp(1.,12000.) as usize;
    let mut times:Vec<_>=(0..=steps).map(|i|first.time+duration*i as f64/steps as f64).collect();
    times.extend(timeline.camera.iter().map(|k|k.time));
    times.extend((first.time.ceil() as i64..=last.time.floor() as i64).map(|s|s as f64));
    times.sort_by(f64::total_cmp);times.dedup_by(|a,b|(*a-*b).abs()<1e-8);
    let points=times.iter().map(|&t|position(t)).collect::<Result<Vec<_>,_>>()?;
    Ok(Some(path_with_ticks(&times,points,Vec::new(),None)))
}

#[cfg(test)]
fn motion_paths(timeline:&StudioTimeline,zoom:f32)->Vec<StudioMotionPath> {
    timeline.clips.iter().map(|clip|motion_path(clip,timeline.interpolation!=StudioInterpolation::Linear,zoom,0.,
        |_|Ok::<_,std::convert::Infallible>([0.;3])).unwrap()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn camera()->Camera {
        let eye=Vec3::new(-100.,70.,130.);let aim=Vec3::new(88.,-10.,0.);let d=aim-eye;
        camera_from_key(StudioCameraKey {position:eye.to_array(),yaw:(-d.z).atan2(d.x).to_degrees(),
            pitch:d.y.atan2(d.x.hypot(d.z)).to_degrees(),..Default::default()})
    }
    fn project(camera:&Camera,rect:egui::Rect,p:Vec3)->egui::Pos2 {
        rect.min+camera.project_to_screen(rect.width() as u32,rect.height() as u32,p).unwrap().to_vec2()
    }
    fn timeline()->StudioTimeline {
        use super::super::render_studio_timeline::{StudioAnimationClip,StudioMotionKey};
        let mut timeline=StudioTimeline::default();
        timeline.clips.push(StudioAnimationClip {name:"BOTH_STAND1".into(),emote:None,start:2.,
            first_frame:10,end_frame:50,source_fps:20.,lane:1,source_first:0,source_end:50,
            motion:vec![StudioMotionKey {time:0.25,..Default::default()},
                StudioMotionKey {time:0.5,position:[2.,3.,4.],yaw:30.}],tail_frames:0,playback:Default::default()});
        timeline.lanes=2;timeline
    }
    #[test]
    fn actor_path_seconds_match_timeline_time_trim_and_eased_playback() {
        use super::super::render_studio_timeline::StudioMotionKey;
        let mut timeline=timeline();timeline.interpolation=StudioInterpolation::Linear;
        let clip=&mut timeline.clips[0];clip.start=2.25;clip.end_frame=90;clip.source_end=100;
        clip.motion=vec![StudioMotionKey::default(),StudioMotionKey {time:5.,position:[100.,0.,0.],yaw:0.}];
        let paths=motion_paths(&timeline,1.);let path=&paths[0];
        assert_eq!(path.points.first(),Some(&[98.,4.,0.]));
        assert_eq!(path.points.last(),Some(&[178.,4.,0.]));
        assert_eq!(path.ticks.iter().map(|t|t.second).collect::<Vec<_>>(),vec![3,4,5,6]);
        assert_eq!(path.ticks[0].position,[113.,4.,0.]);
        assert_eq!(path.ticks[3].position,[173.,4.,0.]);
        assert_eq!(path.ticks[0].tangent,[1.,0.,0.]);
        timeline.interpolation=StudioInterpolation::Smooth;
        let smooth=motion_paths(&timeline,1.);
        assert_eq!(smooth[0].ticks[0].position,[103.625,4.,0.]);
        timeline.interpolation=StudioInterpolation::Spline;
        assert_eq!(motion_paths(&timeline,1.)[0].ticks[0].position,smooth[0].ticks[0].position);
    }
    #[test]
    fn actor_paths_preserve_corners_and_motion_during_extended_animation_loops() {
        use super::super::render_studio_timeline::{StudioMotionKey,StudioClipPlayback};
        let mut timeline=timeline();timeline.interpolation=StudioInterpolation::Linear;
        let clip=&mut timeline.clips[0];clip.start=0.25;clip.tail_frames=40;
        clip.motion=vec![StudioMotionKey::default(),
            StudioMotionKey {time:1.,position:[20.,0.,0.],yaw:0.},
            StudioMotionKey {time:2.,position:[20.,40.,0.],yaw:0.},
            StudioMotionKey {time:6.,position:[60.,40.,0.],yaw:0.}];
        let hold=motion_paths(&timeline,1.);
        assert_eq!(hold[0].points.first(),Some(&[98.,4.,0.]));
        assert_eq!(hold[0].points.last(),Some(&[133.,4.,-40.]));
        assert!(hold[0].points.contains(&[108.,4.,0.]));assert!(hold[0].points.contains(&[108.,4.,-40.]));
        assert_eq!(hold[0].ticks.iter().map(|t|t.second).collect::<Vec<_>>(),vec![1,2,3,4]);
        timeline.clips[0].playback=StudioClipPlayback::Loop;
        let looping=motion_paths(&timeline,1.);
        assert_eq!(looping[0].ticks.iter().map(|t|t.position).collect::<Vec<_>>(),
            hold[0].ticks.iter().map(|t|t.position).collect::<Vec<_>>());
        assert_eq!(looping[0].ticks[3].position,[130.5,4.,-40.]);
    }
    #[test]
    fn actor_paths_keep_clips_and_lanes_separate_and_stationary_ticks_finite() {
        let mut timeline=timeline();timeline.clips[0].motion.clear();
        let mut next=timeline.clips[0].clone();next.start=8.;next.lane=0;timeline.clips.push(next);
        let paths=motion_paths(&timeline,1.5);
        assert_eq!(paths.len(),2);
        assert!(paths[0].points.iter().all(|&p|p==[132.,4.,0.]));
        assert!(paths[1].points.iter().all(|&p|p==[132.,-22.,0.]));
        assert_eq!(paths[0].ticks.iter().map(|t|t.second).collect::<Vec<_>>(),vec![2,3,4]);
        assert_eq!(paths[1].ticks.iter().map(|t|t.second).collect::<Vec<_>>(),vec![8,9,10]);
        assert!(paths.iter().flat_map(|p|&p.ticks).all(|t|t.tangent==[0.;3]));
    }
    #[test]
    fn animated_path_follows_curves_without_any_studio_motion_and_applies_actor_yaw() {
        let mut timeline=timeline();let clip=&mut timeline.clips[0];
        clip.start=0.;clip.first_frame=0;clip.end_frame=80;clip.motion.clear();
        let path=motion_path(clip,false,1.,90.,|t|Ok::<_,()>([t as f32*10.,(t*t) as f32,0.])).unwrap();
        let tick=&path.ticks[2];assert_eq!(tick.second,2);
        assert!((Vec3::from_array(tick.position)-Vec3::new(88.,8.,-20.)).length()<0.0001);
        assert!(path.points.len()>100,"trace must follow the curve within the clip");
        assert!(path.points.iter().any(|p|p[1]>4. && p[1]<20.));
    }
    #[test]
    fn camera_path_follows_spline_and_actual_rig_positions_with_second_ticks() {
        let mut timeline=timeline();timeline.interpolation=StudioInterpolation::Spline;
        timeline.camera=[(0.,[0.,0.,0.]),(2.,[20.,0.,0.]),(4.,[20.,20.,0.]),(6.,[40.,20.,0.])]
            .into_iter().map(|(time,position)|StudioCameraKey {time,position,..Default::default()}).collect();
        let path=camera_path(&timeline,|t|Ok::<_,()>(timeline.camera_at(t).position)).unwrap().unwrap();
        assert_eq!(path.lane,None);assert_eq!(path.ticks.len(),7);
        assert_eq!(path.ticks[1].position,[11.25,-1.25,0.]);
        assert!(timeline.camera.iter().all(|k|path.points.contains(&k.position)));
        let mut rig=super::super::render_studio_camera::StudioCameraRig::default();rig.position.enabled=true;
        let rig_path=camera_path(&timeline,|t|Ok::<_,()>(rig.resolve(timeline.camera_at(t),Some([0.,0.,t as f32*10.]),None).position))
            .unwrap().unwrap();
        assert_eq!(rig_path.ticks[1].position,[11.25,-1.25,10.]);
        timeline.camera.truncate(1);
        assert!(camera_path(&timeline,|_|Ok::<_,()>([0.;3])).unwrap().is_none());
    }
    #[test]
    fn animated_path_does_not_draw_a_return_line_across_a_loop_reset() {
        let mut timeline=timeline();let clip=&mut timeline.clips[0];
        clip.start=0.;clip.first_frame=0;clip.end_frame=40;clip.tail_frames=40;clip.motion.clear();clip.playback=StudioClipPlayback::Loop;
        let path=motion_path(clip,false,1.,0.,|t|Ok::<_,()>([clip.pose_time_ms(t) as f32*0.1,0.,0.])).unwrap();
        assert_eq!(path.breaks.len(),1);
        for &i in &path.breaks {assert!(path.points[i-1][0]-path.points[i][0]>190.);}
        assert_eq!(path.ticks[2].position,path.ticks[0].position);
    }
    #[test]
    #[ignore="requires JKA_TEST_BASE stock single-player assets"]
    fn cinematic_actor_path_tracks_t2_rogue_cin3_pelvis_and_matches_rendered_actor() {
        use crate::cgame::{ClientInfo,player_presenter::PlayerPresenter};
        let base=std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("JKA_TEST_BASE"));
        let mut assets=jka_assets::pk3::AssetSearchPath::open(&base).unwrap();
        let catalog=super::super::render_studio_cinematics::scan(&mut assets).unwrap();
        let source=catalog.clips.iter().find(|c|c.set=="t2_rogue" && c.source.animation=="BOTH_CIN_3").unwrap();
        let mut presenter=PlayerPresenter::new(assets,false).unwrap();presenter.set_async_loading(false);
        presenter.set_ghoul2_anim_smooth(0.);presenter.set_skinning_mode(crate::ui::Ghoul2SkinningMode::Cpu);
        let mut clip=StudioAnimationClip {name:source.source.animation.clone(),emote:Some(source.emote().unwrap()),start:0.25,
            first_frame:source.first_frame,end_frame:source.end_frame(),source_fps:f64::from(source.fps),lane:0,
            source_first:source.first_frame,source_end:source.end_frame(),motion:vec![Default::default()],tail_frames:0,playback:StudioClipPlayback::Hold};
        let identity=[[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]];
        for model in ["luke/default","kyle/default","jan/default"] {
            let info=ClientInfo::solo_model(model);
            for trimmed in [false,true] {
                let mut active=clip.clone();
                if trimmed {active.first_frame+=40;active.end_frame-=60;active.tail_frames=active.end_frame-active.first_frame+20;active.playback=StudioClipPlayback::Loop;}
                let path=motion_path(&active,true,1.,37.,|time| {
                    presenter.sample_profile_attachment(&info,[0.;3],identity,active.pose_time_ms(time),&active.name,[0.;2],
                        active.preview(time).as_ref(),Some((active.first_frame,active.end_frame)),None,"pelvis")
                        .map(|frame|frame.expect("pelvis").position)
                }).unwrap();
                assert!(presenter.drain_fx_requests().is_empty());
                let extent=path.points.iter().copied().map(Vec3::from_array).fold((Vec3::splat(f32::INFINITY),Vec3::splat(f32::NEG_INFINITY)),
                    |(min,max),p|(min.min(p),max.max(p)));
                assert!((extent.1-extent.0).length()>250.,"cinematic movement is missing: {model}");
                if trimmed {assert!(!path.breaks.is_empty());}
                for second in [6,12,18] {
                    let time=second as f64;let motion=active.motion_at(time,true);let yaw=(37.+motion.yaw).to_radians();let (s,c)=yaw.sin_cos();
                    let axis=[[c,s,0.],[-s,c,0.],[0.,0.,1.]];
                    let origin=[88.+motion.position[0],motion.position[1],-22.+motion.position[2]];
                    let preview=active.preview(time);let range=Some((active.first_frame,active.end_frame));let ms=active.pose_time_ms(time);
                    let expected=presenter.sample_profile_attachment(&info,origin,axis,ms,&active.name,[0.;2],preview.as_ref(),range,None,"pelvis").unwrap().unwrap();
                    let actual=path.ticks.iter().find(|t|t.second==second).unwrap();
                    assert!((Vec3::from_array(actual.position)-Vec3::from_array(expected.position)).length()<0.002);
                    let (draws,anchors)=presenter.present_profile_player_preview(&info,origin,axis,ms,&active.name,false,[1.;3],0,[0.;2],true,preview,range).unwrap();
                    assert!(!draws.is_empty());
                    let head=Vec3::from_array(crate::scene::render_position(anchors[0].unwrap()));
                    assert!(head.distance(Vec3::from_array(actual.position))<90.,"path must remain inside the rendered body");
                }
                println!("Animated path: {model}, trimmed/looped={trimmed}, {} samples, span {:.1} units",path.points.len(),(extent.1-extent.0).length());
            }
            // Repeat with authored translation/yaw as well as cinematic movement.
            clip.motion=vec![super::super::render_studio_timeline::StudioMotionKey {position:[15.,-10.,6.],yaw:25.,..Default::default()}];
        }
        // Exercise the editor's incremental cache too, not just the sampler:
        // a real cinematic with no authored translation must produce a trail.
        use winit::platform::windows::EventLoopBuilderExtWindows;
        let event_loop=winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
            .with_any_thread(true).build().unwrap();
        let fixture=crate::cvar_compatibility::Fixture::new();
        let mut app=App::new(fixture.0.clone(),None,crate::scene::MapSource::Bsp("mp/ffa3".into()),false,event_loop.create_proxy()).unwrap();
        app.frontend_page=super::super::FrontendPage::RenderStudio;
        app.asset_preview_player_presenter=Some(presenter);app.profile_preview_zoom=1.;app.profile_preview_yaw=37.;
        app.render_studio.model="luke/default".into();clip.motion.clear();
        app.render_studio.timeline.clips=vec![clip.clone()];app.render_studio.preview_sample_time=Some(6.3);
        let before=app.render_studio.edit_snapshot().signature();let started=std::time::Instant::now();let mut batches=0;
        loop {
            app.refresh_studio_spatial_markers();batches+=1;
            if !app.render_studio.spatial.paths_pending {break;}
            assert!(started.elapsed().as_secs()<20,"path generation did not finish");
        }
        assert!(app.render_studio.spatial.path_message.is_none(),"{:?}",app.render_studio.spatial.path_message);
        assert_eq!(app.render_studio.spatial.motion_paths.len(),1);
        assert!(app.render_studio.spatial.motion_paths[0].points.len()>700);
        assert_eq!(app.render_studio.preview_sample_time,Some(6.3));
        assert_eq!(app.render_studio.edit_snapshot().signature(),before);
        println!("Editor cached cinematic path in {batches} batches ({:.3}s)",started.elapsed().as_secs_f32());
        // Moving a motion key reuses the pose cache and finishes in one batch.
        clip.motion=vec![super::super::render_studio_timeline::StudioMotionKey {position:[10.,20.,5.],..Default::default()}];
        let original=app.render_studio.spatial.motion_paths[0].points[0];app.render_studio.timeline.clips[0]=clip;
        app.refresh_studio_spatial_markers();assert!(!app.render_studio.spatial.paths_pending);
        let moved=app.render_studio.spatial.motion_paths[0].points[0];
        assert!((Vec3::from_array(moved)-Vec3::from_array(original)-Vec3::new(10.,5.,-20.)).length()<0.001);
        // Follow the real animated head, with different copied orbit values at
        // two camera keys. Preview and sampled camera paths share this evaluator.
        use super::super::render_studio_camera::{StudioCameraAttachment,StudioCameraOrbit};
        let start=app.render_studio.timeline.clips[0].start;
        app.render_studio.timeline.camera=vec![StudioCameraKey {time:start,..Default::default()},
            StudioCameraKey {time:start+4.,fov:82.,..Default::default()}];
        app.render_studio.timeline.camera_rig.position=StudioCameraAttachment {enabled:true,half_life:0.12,..Default::default()};
        app.render_studio.timeline.camera_rig.target=app.render_studio.timeline.camera_rig.position.clone();
        assert!(app.set_studio_orbit_key(start,StudioCameraOrbit {distance:120.,yaw:0.,pitch:0.}));
        assert!(app.set_studio_orbit_key(start+4.,StudioCameraOrbit {distance:120.,yaw:90.,pitch:0.}));
        assert_eq!(app.render_studio.timeline.camera[0].rig_orbit.unwrap().yaw,0.);
        let mut shots=Vec::new();
        for (time,angle) in [(start,0.),(start+2.,45.),(start+4.,90.)] {
            app.render_studio.preview_sample_time=Some(time);app.update_studio_camera_rig();
            let camera=app.render_studio.camera_rig_runtime.camera.unwrap();
            let head=app.render_studio.camera_rig_runtime.frames[0].unwrap().position;
            let vector=Vec3::from_array(camera.position)-Vec3::from_array(head);
            assert!((vector.length()-120.).abs()<0.002);
            assert!(((-vector.z).atan2(vector.x).to_degrees()-angle).abs()<0.002);
            assert!(camera_from_key(camera).forward().dot(-vector.normalize())>0.9999);
            shots.push(camera.position);
        }
        let started=std::time::Instant::now();
        loop {
            app.refresh_studio_spatial_markers();
            if !app.render_studio.spatial.paths_pending {break;}
            assert!(started.elapsed().as_secs()<20);
        }
        let path=app.render_studio.spatial.camera_path.as_ref().unwrap();
        assert!(Vec3::from_array(path.points[0]).distance(Vec3::from_array(shots[0]))<0.002);
        assert!(Vec3::from_array(*path.points.last().unwrap()).distance(Vec3::from_array(shots[2]))<0.002);
        println!("Verified keyed 0/45/90 degree orbit at radius 120 around the animated head, including camera-path endpoints");
    }
    #[test]
    fn spatial_plane_drag_tracks_pointer_at_depth_without_label_offset_jump() {
        let camera=camera();let rect=egui::Rect::from_min_size(egui::pos2(260.,84.),egui::vec2(960.,600.));
        for distance in [20.,90.,350.] {
            let origin=camera.position+camera.forward()*distance;
            let start=project(&camera,rect,origin)+egui::vec2(42.,16.);
            let drag=StudioSpatialDrag {camera,viewport:rect,origin,start_pointer:start,axis:None,items:vec![]};
            assert!(drag.delta(start).unwrap().length()<1e-4);
            let offset=egui::vec2(87.,-49.);let delta=drag.delta(start+offset).unwrap();
            assert!(delta.dot(camera.forward()).abs()<0.002);
            let moved=project(&camera,rect,origin+delta)-project(&camera,rect,origin);
            assert!((moved-offset).length()<0.02,"{distance}: {moved:?}");
        }
    }
    #[test]
    fn spatial_axis_drags_recover_exact_world_translation() {
        let camera=camera();let rect=egui::Rect::from_min_size(egui::pos2(40.,120.),egui::vec2(1100.,650.));
        let origin=Vec3::new(88.,-22.,0.);let start=project(&camera,rect,origin);
        for (index,axis) in [Vec3::X,Vec3::Y,Vec3::Z].into_iter().enumerate() {
            let drag=StudioSpatialDrag {camera,viewport:rect,origin,start_pointer:start,axis:Some(index),items:vec![]};
            for distance in [-25.,12.,50.] {
                let pointer=project(&camera,rect,origin+axis*distance);
                let delta=drag.delta(pointer).unwrap();
                assert!((delta-axis*distance).length()<0.02,"axis {index}: {delta:?}");
            }
        }
    }
    #[test]
    fn spatial_drag_preserves_key_times_and_converts_motion_up_axis() {
        let mut timeline=timeline();let initial=timeline.clips[0].motion[1];
        let camera=timeline.camera[0];
        let delta=Vec3::new(10.,20.,30.);
        for _ in 0..2 {
            assert!(translate_item(&mut timeline,StudioItem::Camera(0),camera.position,delta));
            assert!(translate_item(&mut timeline,StudioItem::Motion(0,1),initial.position,delta));
        }
        assert_eq!(timeline.camera[0].position,[10.,9.,30.]);assert_eq!(timeline.camera[0].time,camera.time);
        assert_eq!(timeline.clips[0].motion[1].position,[12.,-27.,24.]);
        assert_eq!(timeline.clips[0].motion[1].time,initial.time);assert_eq!(timeline.clips[0].motion[1].yaw,initial.yaw);
        let before=Vec3::from_array(motion_world(initial.position,1,1.));
        let after=Vec3::from_array(motion_world(timeline.clips[0].motion[1].position,1,1.));
        assert_eq!(after-before,delta);
        assert_eq!(visible_motion_keys(&timeline),vec![(0,1,2.)]);
    }
    #[test]
    fn spatial_group_movement_is_one_undo_and_observer_navigation_is_not_a_scene_edit() {
        let mut studio=super::super::render_studio::RenderStudio::default();studio.timeline=timeline();
        studio.init_history();let before=studio.edit_snapshot().signature();
        studio.spatial.scene_camera=Some(key_from_camera(camera()));
        studio.spatial.scene_camera.as_mut().unwrap().orbit([88.,0.,0.],[23.,-12.]);
        assert_eq!(studio.edit_snapshot().signature(),before);
        let items=[StudioItem::Camera(0),StudioItem::Motion(0,1)];
        let originals:Vec<_>=items.iter().map(|&i|(i,stored_position(&studio.timeline,i).unwrap())).collect();
        for delta in [Vec3::X*10.,Vec3::Y*20.] {
            for (item,initial) in &originals {translate_item(&mut studio.timeline,*item,*initial,delta);}
            studio.track_edit(true);
        }
        studio.track_edit(false);assert!(studio.undo_edit());assert_eq!(studio.edit_snapshot().signature(),before);
        assert!(!studio.undo_edit());assert!(studio.redo_edit());
        assert_eq!(studio.timeline.clips[0].motion[1].position,[2.,3.,24.]);
    }
    #[test]
    fn spatial_camera_translation_is_exact_with_a_bone_follow_rig() {
        use super::super::render_studio_camera::{StudioCameraRig,StudioCameraAttachment};
        let rig=StudioCameraRig {position:StudioCameraAttachment {enabled:true,..Default::default()},
            target:StudioCameraAttachment {enabled:true,..Default::default()},reference_position:[1.,2.,3.]};
        let base=StudioCameraKey::default();let anchor=Some([90.,15.,20.]);let target=Some([70.,20.,15.]);
        let original=rig.resolve(base,anchor,target);
        let delta=Vec3::new(15.,-5.,8.);let mut moved=base;
        moved.position=(Vec3::from_array(moved.position)+delta).to_array();
        let resolved=rig.resolve(moved,anchor,target);
        assert_eq!(Vec3::from_array(resolved.position)-Vec3::from_array(original.position),delta);
    }
}
#[derive(Clone)]
pub(in crate::app) struct StudioSpatialDrag {
    pub camera: Camera,
    pub viewport: egui::Rect,
    pub origin: Vec3,
    pub start_pointer: egui::Pos2,
    pub axis: Option<usize>,
    pub items: Vec<(StudioItem,[f32;3])>,
}
pub(in crate::app) struct StudioSpatialEditor {
    pub show_keys: bool,
    pub show_paths: bool,
    /// Independent observer camera; never saved as a cinematic key or exported.
    pub scene_camera: Option<StudioCameraKey>,
    pub pivot: [f32;3],
    pub drag: Option<StudioSpatialDrag>,
    pub markers: Vec<StudioSpatialMarker>,
    pub motion_paths: Vec<StudioMotionPath>,
    pub camera_path: Option<StudioMotionPath>,
    pub paths_pending: bool,
    pub path_message: Option<String>,
    pub frame_when_ready: Option<egui::Rect>,
    path_poses: BTreeMap<String,StudioPathPoseCache>,
    camera_samples: BTreeMap<u64,[f32;3]>,
    path_camera_runtime: super::render_studio_camera::StudioCameraRigRuntime,
    signature: String,
    pose_signature: String,
    camera_offsets: BTreeMap<u64,[f32;3]>,
    retry: bool,
}
impl Default for StudioSpatialEditor {
    fn default()->Self {Self {show_keys:true,show_paths:true,scene_camera:None,pivot:[88.,0.,0.],drag:None,
        markers:Vec::new(),motion_paths:Vec::new(),camera_path:None,paths_pending:false,path_message:None,frame_when_ready:None,
        path_poses:Default::default(),camera_samples:Default::default(),path_camera_runtime:Default::default(),
        signature:String::new(),pose_signature:String::new(),camera_offsets:Default::default(),retry:false}}
}

pub(in crate::app) fn camera_from_key(key:StudioCameraKey)->Camera {
    let mut camera=Camera::new_with_fov(key.position,key.yaw.to_radians(),key.fov);
    camera.pitch=key.pitch.to_radians();camera
}
pub(in crate::app) fn key_from_camera(camera:Camera)->StudioCameraKey {
    StudioCameraKey {position:camera.position.to_array(),yaw:camera.yaw.to_degrees(),
        pitch:camera.pitch.to_degrees(),fov:camera.cg_fov(),..Default::default()}
}
pub(in crate::app) fn motion_world(position:[f32;3],lane:usize,zoom:f32)->[f32;3] {
    crate::scene::render_position([88.*zoom.clamp(0.45,2.5)+position[0],position[1],-22.+lane as f32*26.+position[2]])
}
pub(in crate::app) fn stored_position(timeline:&StudioTimeline,item:StudioItem)->Option<[f32;3]> {
    match item {StudioItem::Camera(i)=>timeline.camera.get(i).map(|k|k.position),
        StudioItem::Motion(i,k)=>timeline.clips.get(i)?.motion.get(k).map(|k|k.position),_=>None}
}
pub(in crate::app) fn translate_item(timeline:&mut StudioTimeline,item:StudioItem,initial:[f32;3],delta:Vec3)->bool {
    if !delta.is_finite() {return false;}
    let (position,delta)=match item {
        StudioItem::Camera(i)=>match timeline.camera.get_mut(i) {Some(k)=>(&mut k.position,delta.to_array()),None=>return false},
        StudioItem::Motion(i,k)=>match timeline.clips.get_mut(i).and_then(|c|c.motion.get_mut(k)) {
            Some(k)=>(&mut k.position,crate::scene::jka_position(delta.to_array())),None=>return false},
        _=>return false,
    };
    *position=std::array::from_fn(|i|(initial[i]+delta[i]).clamp(-50000.,50000.));true
}
pub(in crate::app) fn visible_motion_keys(timeline:&StudioTimeline)->Vec<(usize,usize,f64)> {
    timeline.clips.iter().enumerate().flat_map(|(i,c)|c.motion.iter().enumerate().filter_map(move |(k,key)| {
        let local=key.time-(c.first_frame-c.source_first) as f64/c.source_fps;
        (local>=-0.00001 && local<=c.duration()+0.00001).then_some((i,k,c.start+local))
    })).collect()
}

/// Invert the exact reversed-Z viewport projection used by the preview renderer.
pub(in crate::app) fn screen_ray(camera:&Camera,viewport:egui::Rect,pointer:egui::Pos2)->Option<(Vec3,Vec3)> {
    if viewport.width()<1. || viewport.height()<1. {return None;}
    let ndc=glam::Vec4::new((pointer.x-viewport.left())/viewport.width()*2.-1.,
        1.-(pointer.y-viewport.top())/viewport.height()*2.,1.,1.);
    let point=camera.view_projection(viewport.width().max(1.) as u32,viewport.height().max(1.) as u32).inverse()*ndc;
    if point.w.abs()<1e-8 {return None;}
    let direction=(point.truncate()/point.w-camera.position).normalize_or_zero();
    (direction.is_finite() && direction.length_squared()>0.5).then_some((camera.position,direction))
}
fn plane_hit(ray:(Vec3,Vec3),point:Vec3,normal:Vec3)->Option<Vec3> {
    let denom=ray.1.dot(normal);if denom.abs()<1e-5 {return None;}
    let t=(point-ray.0).dot(normal)/denom;
    (t>=0. && t.is_finite()).then_some(ray.0+ray.1*t)
}
fn axis_parameter(ray:(Vec3,Vec3),point:Vec3,axis:Vec3)->Option<f32> {
    let b=ray.1.dot(axis);let denom=1.-b*b;if denom<1e-4 {return None;}
    let w=ray.0-point;let value=(w.dot(axis)-b*w.dot(ray.1))/denom;
    value.is_finite().then_some(value)
}
impl StudioSpatialDrag {
    pub fn delta(&self,pointer:egui::Pos2)->Option<Vec3> {
        let before=screen_ray(&self.camera,self.viewport,self.start_pointer)?;
        let after=screen_ray(&self.camera,self.viewport,pointer)?;
        if let Some(index)=self.axis {
            let axis=[Vec3::X,Vec3::Y,Vec3::Z][index];
            Some(axis*(axis_parameter(after,self.origin,axis)?-axis_parameter(before,self.origin,axis)?))
        } else {
            let normal=self.camera.forward();
            Some(plane_hit(after,self.origin,normal)?-plane_hit(before,self.origin,normal)?)
        }
    }
}

impl App {
    pub(in crate::app) fn refresh_studio_spatial_markers(&mut self) {
        let snapshot=self.render_studio.edit_snapshot();
        let rig=self.render_studio.timeline.camera_rig.clone();
        let ids=[&rig.position,&rig.target].map(|b| {
            if !b.enabled {return None;}
            let info=self.studio_attachment_info(b.lane);
            self.ensure_asset_preview_player_presenter().ok()?;
            self.asset_preview_player_presenter.as_mut()?.profile_attachment_model_id(&info).ok()
        });
        // Readiness/model identity participates even without a camera rig, so
        // async loads and model changes invalidate the animated trajectory.
        let path_models:Vec<_>=(0..self.render_studio.timeline.lanes).map(|lane| {
            let info=self.studio_attachment_info(lane);
            let id=self.ensure_asset_preview_player_presenter().and_then(|()|
                self.asset_preview_player_presenter.as_mut().unwrap().profile_attachment_model_id(&info));
            (info,id)
        }).collect();
        let context=format!("{:?}",(self.profile_preview_zoom,self.profile_preview_yaw,
            self.profile_studio_head_look,&self.network.saber1,&self.network.saber2,ids,
            path_models.iter().map(|(_,id)|id).collect::<Vec<_>>()));
        let signature=snapshot.signature()+context.as_str();
        if self.render_studio.spatial.signature==signature && !self.render_studio.spatial.retry {return;}
        let pose_signature=format!("{}{:?}{}{:?}",snapshot.camera_pose_signature(),rig,context,
            self.render_studio.timeline.camera.iter().map(|k|(k.time,k.rig_orbit)).collect::<Vec<_>>());
        let mut spatial=std::mem::take(&mut self.render_studio.spatial);
        if spatial.signature!=signature {spatial.camera_samples.clear();spatial.camera_path=None;}
        if spatial.pose_signature!=pose_signature {spatial.camera_offsets.clear();spatial.pose_signature=pose_signature;}
        spatial.signature=signature;spatial.retry=false;spatial.markers.clear();
        let time=self.render_studio.preview_sample_time;
        let pending=self.render_studio.camera_pending_match.take();
        let runtime=std::mem::replace(&mut self.render_studio.camera_rig_runtime,std::mem::take(&mut spatial.path_camera_runtime));
        let keys=self.render_studio.timeline.camera.clone();
        for (i,key) in keys.iter().enumerate() {
            let offset=if !rig.active() {[0.;3]} else if let Some(offset)=spatial.camera_offsets.get(&key.time.to_bits()) {*offset} else {
                self.render_studio.preview_sample_time=Some(key.time);
                self.update_studio_camera_rig();
                let resolved=self.render_studio.camera_rig_runtime.camera.unwrap_or(*key);
                let offset=std::array::from_fn(|a|resolved.position[a]-key.position[a]);
                let loading=self.render_studio.camera_rig_runtime.messages.iter().flatten()
                    .any(|m|m.to_ascii_lowercase().contains("loading"));
                if loading {spatial.retry=true;}else{spatial.camera_offsets.insert(key.time.to_bits(),offset);}
                offset
            };
            spatial.markers.push(StudioSpatialMarker {item:StudioItem::Camera(i),number:i+1,time:key.time,
                position:std::array::from_fn(|a|key.position[a]+offset[a]),lane:None});
        }
        spatial.camera_offsets.retain(|t,_|keys.iter().any(|k|k.time.to_bits()==*t));
        let mut camera_pending=false;
        if spatial.camera_path.is_none() {
            let mut timeline=StudioTimeline::default();timeline.camera=keys.clone();
            timeline.interpolation=self.render_studio.timeline.interpolation;
            let started=std::time::Instant::now();let mut queries=0;
            let result=camera_path(&timeline,|time| {
                if !rig.active() {return Ok(timeline.camera_at(time).position);}
                if let Some(&position)=spatial.camera_samples.get(&time.to_bits()) {return Ok(position);}
                if queries>=128 || (queries>0 && started.elapsed().as_secs_f32()>0.004) {return Err(PathSampleError::Pending);}
                queries+=1;self.render_studio.preview_sample_time=Some(time);self.update_studio_camera_rig();
                if let Some(message)=self.render_studio.camera_rig_runtime.messages.iter().flatten()
                    .find(|message|message.to_ascii_lowercase().contains("loading")) {
                    return Err(PathSampleError::Asset(message.clone()));
                }
                let position=self.render_studio.camera_rig_runtime.camera.unwrap_or_else(||timeline.camera_at(time)).position;
                spatial.camera_samples.insert(time.to_bits(),position);Ok(position)
            });
            match result {
                Ok(path)=>spatial.camera_path=path,
                Err(PathSampleError::Pending)=>camera_pending=true,
                Err(PathSampleError::Asset(message))=>{
                    camera_pending=message.to_ascii_lowercase().contains("loading");
                }
            }
        }
        spatial.path_camera_runtime=std::mem::replace(&mut self.render_studio.camera_rig_runtime,runtime);
        self.render_studio.preview_sample_time=time;self.render_studio.camera_pending_match=pending;
        for (number,(i,k,time)) in visible_motion_keys(&self.render_studio.timeline).into_iter().enumerate() {
            let clip=&self.render_studio.timeline.clips[i];
            spatial.markers.push(StudioSpatialMarker {item:StudioItem::Motion(i,k),number:number+1,time,
                position:motion_world(clip.motion[k].position,clip.lane,self.profile_preview_zoom),lane:Some(clip.lane)});
        }
        self.refresh_studio_actor_paths(&mut spatial,&path_models);
        spatial.paths_pending|=camera_pending;spatial.retry|=camera_pending;
        self.render_studio.spatial=spatial;
    }
    fn refresh_studio_actor_paths(&mut self,spatial:&mut StudioSpatialEditor,
        models:&[(crate::cgame::ClientInfo,Result<usize,String>)]) {
        let started=std::time::Instant::now();let mut queries=0;
        let mut used=std::collections::BTreeSet::new();let mut paths=Vec::new();
        spatial.paths_pending=false;spatial.path_message=None;
        let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
        for clip in &self.render_studio.timeline.clips {
            let result=(|| {
                let (info,id)=models.get(clip.lane).ok_or_else(||PathSampleError::Asset("Missing actor layer".into()))?;
                let id=id.as_ref().map_err(|e|PathSampleError::Asset(e.clone()))?;
                let key=format!("{:?}",(id,&clip.name,clip.first_frame,clip.end_frame,clip.source_fps,
                    clip.emote.as_ref().map(|e|(&e.id,&e.gla,e.first_frame,e.end_frame,e.fps,e.looping)),self.profile_studio_head_look));
                used.insert(key.clone());
                let cache=spatial.path_poses.entry(key).or_default();
                let presenter=self.asset_preview_player_presenter.as_mut()
                    .ok_or_else(||PathSampleError::Asset("Player presenter unavailable".into()))?;
                if cache.bone.is_empty() {
                    let names=presenter.profile_attachment_names(info,None).map_err(PathSampleError::Asset)?;
                    cache.bone=["pelvis","hips","model_root","root"].into_iter()
                        .find_map(|bone|names.iter().find(|n|n.eq_ignore_ascii_case(bone)).cloned())
                        .ok_or_else(||PathSampleError::Asset("Actor has no trajectory bone".into()))?;
                }
                motion_path(clip,smooth,self.profile_preview_zoom,self.profile_preview_yaw,|time| {
                    let ms=clip.pose_time_ms(time);
                    if let Some(&position)=cache.samples.get(&ms) {return Ok(position);}
                    // Spread new skeletal queries over frames. Cached poses are
                    // reused when moving clips or editing motion keys.
                    if queries>=128 || (queries>0 && started.elapsed().as_secs_f32()>0.004) {return Err(PathSampleError::Pending);}
                    queries+=1;
                    let preview=clip.preview(time);
                    let frame=presenter.sample_profile_attachment(info,[0.;3],[[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]],
                        ms,&clip.name,self.profile_studio_head_look,preview.as_ref(),Some((clip.first_frame,clip.end_frame)),None,&cache.bone)
                        .map_err(PathSampleError::Asset)?.ok_or_else(||PathSampleError::Asset("Actor trajectory bone unavailable".into()))?;
                    if !Vec3::from_array(frame.position).is_finite() {return Err(PathSampleError::Asset("Invalid actor trajectory pose".into()));}
                    cache.samples.insert(ms,frame.position);Ok(frame.position)
                })
            })();
            match result {
                Ok(path)=>paths.push(path),
                Err(PathSampleError::Pending)=>spatial.paths_pending=true,
                Err(PathSampleError::Asset(message))=>{
                    spatial.paths_pending|=message.to_ascii_lowercase().contains("loading");
                    spatial.path_message=Some(message);
                }
            }
        }
        spatial.path_poses.retain(|key,_|used.contains(key));
        spatial.motion_paths=paths;spatial.retry|=spatial.paths_pending;
    }
    pub(in crate::app) fn frame_studio_spatial_keys(&mut self,viewport:egui::Rect) {
        self.refresh_studio_spatial_markers();
        if self.render_studio.spatial.paths_pending {
            self.render_studio.spatial.frame_when_ready=Some(viewport);return;
        }
        self.render_studio.spatial.frame_when_ready=None;
        let mut points:Vec<Vec3>=self.render_studio.spatial.markers.iter().map(|m|Vec3::from_array(m.position)).collect();
        points.extend(self.render_studio.spatial.markers.iter().filter(|m|m.lane.is_some())
            .map(|m|Vec3::from_array(m.position)+Vec3::Y*64.));
        for path in &self.render_studio.spatial.motion_paths {
            for &point in &path.points {
                let point=Vec3::from_array(point);points.extend([point,point+Vec3::Y*64.]);
            }
        }
        if let Some(path)=&self.render_studio.spatial.camera_path {points.extend(path.points.iter().copied().map(Vec3::from_array));}
        if points.is_empty() {points.push(Vec3::new(88.,0.,0.));}
        let min=points.iter().copied().reduce(Vec3::min).unwrap();
        let max=points.iter().copied().reduce(Vec3::max).unwrap();
        let pivot=(min+max)*0.5;let radius=((max-min).length()*0.5).max(32.);
        let mut key=StudioCameraKey::default();key.fov=65.;
        let camera=camera_from_key(key);
        let vertical=camera.fov_y_for_viewport(viewport.width().max(1.) as u32,viewport.height().max(1.) as u32)*0.5;
        let horizontal=(vertical.tan()*viewport.width()/viewport.height().max(1.)).atan();
        let distance=radius/vertical.min(horizontal).sin().max(0.1)*1.15;
        let eye=pivot+Vec3::new(-1.,0.65,0.9).normalize()*distance;
        let direction=pivot-eye;
        key.position=eye.to_array();key.yaw=(-direction.z).atan2(direction.x).to_degrees();
        key.pitch=direction.y.atan2(direction.x.hypot(direction.z)).to_degrees();
        self.render_studio.spatial.scene_camera=Some(key);self.render_studio.spatial.pivot=pivot.to_array();
        self.profile_preview_key=None;
    }
}
