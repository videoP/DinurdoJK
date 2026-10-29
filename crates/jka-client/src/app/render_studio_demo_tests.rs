//! Run each GPU integration test in its own process (one winit event loop).
use super::*;
use crate::app::{scene, UserEvent};
use std::{path::PathBuf, time::Duration};
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::{
    application::ApplicationHandler,
    event_loop::{ActiveEventLoop, EventLoop},
};

#[test]
#[ignore = "requires DJK_STUDIO_TEST_BASE, timeline_project1 assets, GPU; run alone with DJK_HEADLESS=1"]
fn studio_project_isolated_transition_and_gameplay_handoff() {
    let base =
        PathBuf::from(std::env::var_os("DJK_STUDIO_TEST_BASE").expect("DJK_STUDIO_TEST_BASE"));
    let path = std::env::var_os("DJK_STUDIO_TEST_PROJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            base.parent()
                .unwrap()
                .join("japlus/render-studio/timeline_project1.json")
        });
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut demos: Vec<StudioDemoClip> = serde_json::from_value(json["demos"].clone()).unwrap();
    if std::env::var_os("DJK_STUDIO_TEST_FOUR_DEMOS").is_some() {
        let original = demos[0].clone();
        demos = (0..4).map(|i| {
            let mut clip = original.clone();
            clip.start = i as f64 * 3.0;
            clip.in_ms = 1000;
            clip.out_ms = 5500;
            clip.speed_keys.clear();
            clip.transition_in = if i == 0 { super::super::render_studio_timeline::StudioDemoTransition::Cut }
                else { super::super::render_studio_timeline::StudioDemoTransition::Dissolve };
            clip
        }).collect();
    }
    if std::env::var_os("DJK_STUDIO_TEST_ROUTE_CUTS").is_some() {
        let recordings = demos.clone();
        demos = (0..12).map(|i| {
            let mut clip = recordings[i % 2].clone();
            clip.start = match i { 0 => 0.0, 1 => 1.5, _ => 4.0 + (i - 2) as f64 };
            clip.in_ms = 1000;
            clip.out_ms = if i < 2 { 3500 } else { 2000 };
            clip.speed_keys.clear();
            clip.transition_in = if i == 1 { super::super::render_studio_timeline::StudioDemoTransition::Dissolve }
                else { super::super::render_studio_timeline::StudioDemoTransition::Cut };
            clip
        }).collect();
    }
    let camera = serde_json::from_value(json["camera"].clone()).unwrap();
    let fixture = crate::cvar_compatibility::Fixture::new();
    let mut roots = crate::filesystem_paths::load(&base);
    roots.homepath = Some(fixture.0.clone());
    jka_assets::pk3::configure_filesystem_roots(&base, roots);
    std::fs::create_dir_all(fixture.0.join("base")).unwrap();
    std::fs::write(fixture.0.join("base/DinurdoJK.cfg"),
        "seta r_fullscreen 0\nseta r_customwidth 800\nseta r_customheight 600\nseta s_volume 0\nseta s_steamAudio 0\nseta r_maxfps 60\n").unwrap();
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .with_any_thread(true)
        .build()
        .unwrap();
    let mut harness = StudioRun {
        app: App::new(
            base,
            None,
            scene::MapSource::Bsp("mp/ffa3".into()),
            false,
            event_loop.create_proxy(),
        )
        .unwrap(),
        demos,
        camera,
        step: 0,
        started: Instant::now(),
        step_at: Instant::now(),
        overlap_frames: 0,
        ids: Vec::new(),
        main_request: 0,
        first_map: String::new(),
        shutting_down: false,
        capture_requested: false,
        capture_received: false,
        seen: std::collections::BTreeSet::new(),
        export_test_done: false,
        ready_events: 0,
        source_generation: 0,
    };
    event_loop.run_app(&mut harness).unwrap();
    assert_eq!(harness.step, 4);
    assert_eq!(harness.seen.len(), harness.demos.len(), "all demo sources must play");
    assert!(
        harness.capture_received,
        "must capture a real blended GPU frame"
    );
    assert!(
        harness.overlap_frames >= 20,
        "must render both animated sources continuously, got {} frames",
        harness.overlap_frames
    );
}

struct StudioRun {
    app: App,
    demos: Vec<StudioDemoClip>,
    camera: Vec<StudioCameraKey>,
    step: u8,
    started: Instant,
    step_at: Instant,
    overlap_frames: usize,
    ids: Vec<u64>,
    main_request: u64,
    first_map: String,
    shutting_down: bool,
    capture_requested: bool,
    capture_received: bool,
    seen: std::collections::BTreeSet<usize>,
    export_test_done: bool,
    ready_events: usize,
    source_generation: u64,
}
impl ApplicationHandler<UserEvent> for StudioRun {
    fn resumed(&mut self, e: &ActiveEventLoop) {
        self.app.resumed(e);
    }
    fn user_event(&mut self, e: &ActiveEventLoop, event: UserEvent) {
        if matches!(&event, UserEvent::StudioSourceReady { error: None, .. }) { self.ready_events += 1; }
        if let UserEvent::StudioTestScenePresented { overlap: true } = &event {
            self.overlap_frames += 1;
        }
        if let UserEvent::ScreenshotFinished(Ok(crate::runtime::ScreenshotOutput::VideoFrame {
            width,
            height,
            bgra,
            ..
        })) = &event
        {
            let colors: std::collections::HashSet<_> =
                bgra.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect();
            assert!(
                colors.len() > 32,
                "captured preview must contain the rendered scene"
            );
            let rgba: Vec<u8> = bgra
                .chunks_exact(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect();
            std::fs::create_dir_all("artifacts").unwrap();
            image::save_buffer(
                "artifacts/studio-isolation-preview.png",
                &rgba,
                *width,
                *height,
                image::ColorType::Rgba8,
            )
            .unwrap();
            self.capture_received = true;
        }
        if let UserEvent::RendererError(error) = &event {
            panic!("Studio renderer error: {error}");
        }
        let ready = matches!(event, UserEvent::RendererReady { .. });
        self.app.user_event(e, event);
        if ready {
            self.app.front_end = true;
            self.app.frontend_page = FrontendPage::RenderStudio;
            self.app.render_studio.timeline.demos = self.demos.clone();
            if self.demos.len() >= 4 {
                use super::super::render_studio_director::{StudioDirectorPreset, StudioDirectorShot};
                self.app.render_studio.timeline.director_shots = self.demos.iter().enumerate().map(|(i, clip)|
                    StudioDirectorShot::new(i as u64 + 1, StudioDirectorPreset::Chase, -1, clip.start, clip.duration()))
                    .collect();
            }
            self.app.render_studio.timeline.camera = self.camera.clone();
            if std::env::var_os("DJK_STUDIO_TEST_LIGHTS_LENS").is_some() {
                use super::super::render_studio_lights::*;
                let lens=crate::renderer::StudioLens{aperture:Some(2.0),focus_distance:128.0};
                for key in &mut self.app.render_studio.timeline.camera {key.lens=lens;}
                for shot in &mut self.app.render_studio.timeline.director_shots {shot.lens=lens;}
                let mut layer=StudioLightLayer::new(1);
                layer.lights.push(StudioLight{name:"Integration point".into(),kind:StudioLightKind::Point,
                    emitter:StudioLightEmitter::default(),enabled:true,start:0.0,duration:100.0,keys:vec![StudioLightKey::default()]});
                layer.lights.push(StudioLight{name:"Integration spot".into(),kind:StudioLightKind::Spot,
                    emitter:StudioLightEmitter::default(),enabled:true,start:0.0,duration:100.0,keys:vec![StudioLightKey{color:[0.0,0.0,1.0],..Default::default()}]});
                self.app.render_studio.timeline.light_layers=vec![layer];
            }
            self.app.render_studio.timeline.seek(0.0);
            self.app.render_studio.demo_viewport = Some([0, 0, 400, 300]);
        }
    }
    fn window_event(
        &mut self,
        e: &ActiveEventLoop,
        id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        self.app.window_event(e, id, event);
    }
    fn about_to_wait(&mut self, e: &ActiveEventLoop) {
        assert!(
            self.started.elapsed() < Duration::from_secs(150),
            "Studio timeout step {}: {} / {}",
            self.step,
            self.app.console_status,
            self.app.render_studio.status
        );
        if self.shutting_down {
            if self.app.render.as_ref().is_none_or(|r| r.is_finished()) {
                self.app.render = None;
                e.exit();
            }
            e.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(5),
            ));
            return;
        }
        self.app.about_to_wait(e);
        let expected_contexts = source_pool::requests(&self.demos, &[0], 0.0, true).len();
        assert!(self.app.render_studio.sources.len() + self.app.render_studio.source_prepares.len()
            + usize::from(self.app.render_studio.source_job.is_some()) <= expected_contexts,
            "clip count must not multiply recording-owned playback contexts");
        if self.step > 0 {
            assert_eq!(self.app.render_studio.next_source_id, self.source_generation, "switching clips must not open recordings again");
            assert_eq!(self.ready_events, expected_contexts, "switching/seeking must not rebuild already-loaded scenes");
        }
        if let Some(frame) = self.app.render_studio.scene_frame.as_ref() {
            for sample in std::iter::once(&frame.outgoing).chain(frame.incoming.as_ref()) {
                if std::env::var_os("DJK_STUDIO_TEST_TARGET").is_some() && self.step>0 && self.step_at.elapsed()>Duration::from_millis(100) {
                    let target=self.app.render_studio.timeline.effective_demo_camera_at(self.app.render_studio.timeline_time()).focus_target.unwrap();
                    let delta=glam::Vec3::from_array(target)-sample.shot_camera.position;
                    assert!((sample.lens.focus_distance-delta.length().clamp(1.0,65536.0)).abs()<0.01,"focus must use the actual resolved shot position");
                    if delta.length_squared()>1.0 {assert!(sample.shot_camera.forward().dot(delta.normalize())>0.999,"camera must aim at its focus handle");}
                }
                if let Some(source) = self.app.render_studio.sources.iter().find(|s| s.id == sample.id) {
                    self.seen.insert(source.clip_index);
                }
            }
        }
        assert!(
            !self.app.render_studio.sources.iter().any(|s| s.failed),
            "Studio source failed: {}",
            self.app.render_studio.status
        );
        match self.step {
            0 => {
                if self.app.render_studio.sources.len() != expected_contexts
                    || self.app.render_studio.sources.iter().any(|s| !s.gpu_ready)
                    || self.app.render_studio.scene_frame.is_none()
                    || self.app.loading.is_some()
                {
                    return;
                }
                assert!(
                    self.app.game_session.is_none(),
                    "Studio must never occupy the gameplay session"
                );
                self.ids = self
                    .app
                    .render_studio
                    .sources
                    .iter()
                    .map(|s| s.id)
                    .collect();
                if std::env::var_os("DJK_STUDIO_TEST_TARGET").is_some() {
                    let camera=self.app.render_studio.scene_frame.as_ref().unwrap().outgoing.shot_camera;
                    let point=(camera.position+camera.forward()*96.0).to_array();
                    for key in &mut self.app.render_studio.timeline.camera {key.focus_target=Some(point);}
                    for shot in &mut self.app.render_studio.timeline.director_shots {shot.focus_target=Some(point);}
                }
                self.main_request = self.app.latest_request_id;
                self.source_generation = self.app.render_studio.next_source_id;
                if self.demos.len() == 4 {
                    let a = self.app.render_studio.sources[0].prepared.as_ref().unwrap();
                    let b = self.app.render_studio.sources[1].prepared.as_ref().unwrap();
                    assert!(Arc::ptr_eq(&a.map.textures, &b.map.textures), "same-map decoded textures must be shared");
                    assert!(Arc::ptr_eq(&a.map.lightmaps, &b.map.lightmaps), "same-map decoded lightmaps must be shared");
                    let a = &self.app.render_studio.sources[0].playback;
                    let b = &self.app.render_studio.sources[1].playback;
                    assert!(Arc::ptr_eq(&a.demo_bytes, &b.demo_bytes), "overlapping views must share the recording bytes");
                    assert!(Arc::ptr_eq(a.demo_index.as_ref().unwrap(), b.demo_index.as_ref().unwrap()), "overlapping views must share the recording index");
                }
                for line in self.app.studio_memory_report_lines() { println!("{line}"); }
                self.app.render_command(RenderCommand::MemoryInfo);
                self.first_map = self.app.map_name.clone();
                let overlap = self.demos[1].start;
                self.app.render_studio.timeline.seek(overlap - 0.3);
                self.app.render_studio.timeline.toggle_play();
                self.step = 1;
                self.step_at = Instant::now();
                println!(
                    "[STUDIO TEST] sources resident {:?}; main map {}",
                    self.ids, self.first_map
                );
            }
            1 => {
                assert!(self.app.game_session.is_none());
                assert_eq!(
                    self.app.latest_request_id, self.main_request,
                    "preview must not request/replace gameplay maps"
                );
                assert_eq!(self.app.map_name, self.first_map);
                assert!(self.app.render_studio.sources.len() <= self.demos.len());
                // Recording identities stay stable across cuts and fades.
                if self.app.render_studio.timeline_time() < self.demos[0].end() {
                    assert!(self.app.render_studio.sources.iter().all(|s| self.ids.contains(&s.id)));
                }
                if !self.capture_requested
                    && self
                        .app
                        .render_studio
                        .scene_frame
                        .as_ref()
                        .is_some_and(|s| s.incoming.is_some() && s.progress > 0.45)
                {
                    self.app.render_command(RenderCommand::CaptureVideoFrame {
                        frame_index: 0,
                        depth: false,
                        alpha: false,
                    });
                    self.capture_requested = true;
                }
                if self.app.render_studio.timeline_time() > self.demos[self.demos.len() - 2].end() + 0.2 {
                    assert!(self.overlap_frames >= 20);
                    self.app.render_studio.timeline.seek(0.0);
                    self.app.render_studio.timeline.toggle_play();
                    self.step = 2;
                    self.step_at = Instant::now();
                }
            }
            2 => {
                if self.step_at.elapsed() < Duration::from_millis(300) {
                    return;
                }
                let Some(frame) = self
                    .app
                    .render_studio
                    .scene_frame
                    .as_ref() else { return; };
                let restored = self.app.render_studio.sources.iter().find(|s| s.id == frame.outgoing.id).unwrap();
                assert_eq!(restored.clip_index, 0);
                assert!(self.ids.contains(&frame.outgoing.id), "restart must reuse a loaded recording context and GPU scene");
                assert!(frame.incoming.is_none());
                assert!(restored.last_sample_ms.unwrap() < 2000);
                assert!(self.app.game_session.is_none());
                if !self.export_test_done && std::env::var_os("DJK_STUDIO_TEST_EXPORT").is_some() {
                    // Export four real blended frames plus native demo audio.
                    // This exercises capture acknowledgements and reuse of an
                    // unchanged offline render while the pipe is backpressured.
                    let at = self.demos[1].start + 0.5;
                    self.app.render_studio.timeline.in_point = at;
                    self.app.render_studio.timeline.out_point = Some(at + 0.4);
                    self.app.render_studio.fps = 10;
                    self.app.render_studio.preset = StudioVideoFormat::H264High;
                    self.app.start_render_studio_export();
                    assert!(self.app.render_studio.export.is_some(), "{}", self.app.render_studio.status);
                    self.step = 5;
                    return;
                }
                // Exercise the real command during an overlap, including a
                // late preview completion delivered after gameplay took over.
                self.app
                    .render_studio
                    .timeline
                    .seek(self.demos[1].start + 0.5);
                self.app.tick_render_studio_demo_preview(Instant::now());
                self.app.execute_command_line("devmap mp/ffa3");
                assert!(self.app.frontend_page != FrontendPage::RenderStudio);
                assert!(self.app.render_studio.sources.is_empty());
                assert!(self.app.render_studio.scene_frame.is_none());
                assert!(self.app.render_studio_world_compositor.is_none());
                let request = self.app.latest_request_id;
                self.app.on_studio_source_ready(self.ids[0], None, None, 1);
                assert_eq!(self.app.latest_request_id, request);
                self.step = 3;
            }
            3 => {
                if self.app.loading.is_some() || self.app.front_end {
                    return;
                }
                let session = self
                    .app
                    .game_session
                    .as_ref()
                    .expect("devmap owns local session");
                assert!(session.local);
                assert_eq!(session.phase, super::super::SessionPhase::Playing);
                assert!(self.app.map_name.to_ascii_lowercase().contains("ffa3"));
                self.step = 4;
                self.shutting_down = true;
                println!(
                    "[STUDIO TEST] {} overlap frames, restart and devmap passed",
                    self.overlap_frames
                );
                if let Some(render) = self.app.render.as_ref() {
                    render.request_shutdown();
                }
            }
            5 => {
                if self.app.render_studio.export.is_some() { return; }
                assert!(self.app.render_studio.status.starts_with("Wrote "), "{}", self.app.render_studio.status);
                let folder = self.app.game_write_path("render-studio");
                let output = std::fs::read_dir(&folder).unwrap().filter_map(Result::ok)
                    .map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "mp4")).expect("encoded movie");
                let ffmpeg = std::env::var_os("DJK_STUDIO_TEST_FFMPEG").unwrap();
                let decoded = std::process::Command::new(&ffmpeg).args(["-hide_banner", "-loglevel", "error", "-i"])
                    .arg(&output).args(["-map", "0:v:0", "-f", "framemd5", "pipe:1"]).output().unwrap();
                assert!(decoded.status.success(), "{}", String::from_utf8_lossy(&decoded.stderr));
                let count = String::from_utf8_lossy(&decoded.stdout).lines().filter(|l| !l.starts_with('#') && !l.is_empty()).count();
                assert_eq!(count, 4, "export must contain exactly four video timestamps");
                let audio = std::process::Command::new(&ffmpeg).args(["-hide_banner", "-loglevel", "error", "-i"])
                    .arg(output).args(["-map", "0:a:0", "-f", "f32le", "-ac", "2", "-ar", "48000", "pipe:1"]).output().unwrap();
                assert!(audio.status.success() && !audio.stdout.is_empty(), "native demo audio must mux and decode");
                assert!(!std::fs::read_dir(folder).unwrap().filter_map(Result::ok).any(|e| e.path().extension().is_some_and(|e| e == "wav")),
                    "temporary audio must be finalized and removed");
                println!("[STUDIO TEST] four real exported video frames and native audio decoded");
                self.export_test_done = true;
                self.app.render_studio.timeline.seek(0.0);
                self.step = 2;
            }
            _ => unreachable!(),
        }
    }
    fn exiting(&mut self, _: &ActiveEventLoop) {}
}


#[test]
#[ignore = "Requires local Ventara recordings, installed maps and GPU; run alone with DJK_HEADLESS=1"]
fn ventara_ghost_comparison_gpu_all_runners_and_clock_phases() {
    use super::super::render_studio_director as director;
    let base=PathBuf::from(std::env::var_os("DJK_STUDIO_TEST_BASE").unwrap());
    let (inputs,prior)=director::route_test_inputs(&base);
    let runs=director::load_route_runs(base.clone(),base.parent().unwrap_or(&base).join("render-studio/Test"),Default::default(),inputs).unwrap();
    let mut plan=director::build_route_comparison_plan(&runs,160.0,None).unwrap();
    director::inherit_route_presentation(&mut plan,&prior,&Default::default());
    director::direct_route_plan(&mut plan,90.0,1);
    let mut clip=plan.clips.remove(0);
    let reference=clip.comparison.as_ref().unwrap().reference;
    // Existing projects may have a low stored reference opacity. Rendering
    // must still present the followed runner as a fully opaque actor.
    clip.race_overlays[reference].alpha=0.2;
    let race=clip.comparison.as_ref().unwrap();
    let mut phases=std::collections::HashSet::new();
    let mut times=vec![0.5];
    for phase in &race.phases {
        if phases.insert(format!("{}-{:?}-{:?}",phase.section,phase.kind,phase.replay.as_ref().map(|r|r.focus))) {times.push(phase.start+phase.duration*0.5);}
    }
    let fixture=crate::cvar_compatibility::Fixture::new();
    let mut roots=crate::filesystem_paths::load(&base);roots.homepath=Some(fixture.0.clone());
    jka_assets::pk3::configure_filesystem_roots(&base,roots);
    std::fs::create_dir_all(fixture.0.join("base")).unwrap();
    std::fs::write(fixture.0.join("base/DinurdoJK.cfg"),
        "seta r_fullscreen 0\nseta r_customwidth 1280\nseta r_customheight 720\nseta s_volume 0\nseta s_steamAudio 0\nseta r_maxfps 30\n").unwrap();
    let event_loop=EventLoop::<UserEvent>::with_user_event().with_any_thread(true).build().unwrap();
    let mut harness=GhostRaceRun {app:App::new(base,None,scene::MapSource::Bsp("mp/ffa3".into()),false,event_loop.create_proxy()).unwrap(),
        clip,shots:plan.shots,times,stage:0,stage_at:Instant::now(),started:Instant::now(),initialized:false,stage_set:false,pending:false,captures:0,shutdown:false,ghost_pixel_rois:Vec::new(),combat:false,combat_errors:Vec::new()};
    event_loop.run_app(&mut harness).unwrap();
    assert_eq!(harness.captures,harness.times.len());
}

struct GhostRaceRun {
    app:App,clip:StudioDemoClip,shots:Vec<super::super::render_studio_director::StudioDirectorShot>,times:Vec<f64>,
    stage:usize,stage_at:Instant,started:Instant,initialized:bool,stage_set:bool,pending:bool,captures:usize,shutdown:bool,ghost_pixel_rois:Vec<[u32;4]>,combat:bool,combat_errors:Vec<String>,
}

#[test]
#[ignore = "Requires col_vs40s4, generated review project and GPU; run alone with DJK_HEADLESS=1"]
fn combat_director_gpu_reported_frames_keep_real_actors_visible() {
    let base=PathBuf::from(std::env::var_os("DJK_STUDIO_TEST_BASE").unwrap());
    let project=std::env::var_os("DJK_STUDIO_TEST_PROJECT").map(PathBuf::from)
        .unwrap_or_else(||base.join("render-studio/SaberExample2 - Revised.json"));
    let json:serde_json::Value=serde_json::from_slice(&std::fs::read(project).unwrap()).unwrap();
    let fixture=crate::cvar_compatibility::Fixture::new();
    let mut roots=crate::filesystem_paths::load(&base); roots.homepath=Some(fixture.0.clone());
    jka_assets::pk3::configure_filesystem_roots(&base,roots);
    std::fs::create_dir_all(fixture.0.join("base")).unwrap();
    std::fs::write(fixture.0.join("base/DinurdoJK.cfg"),
        "seta r_fullscreen 0\nseta r_customwidth 1280\nseta r_customheight 720\nseta s_volume 0\nseta s_steamAudio 0\nseta r_maxfps 30\n").unwrap();
    let event_loop=EventLoop::<UserEvent>::with_user_event().with_any_thread(true).build().unwrap();
    let combat_clip:StudioDemoClip=serde_json::from_value(json["demos"][0].clone()).unwrap();
    let combat_times=std::fs::read("artifacts/studio-combat-source-times.json").ok()
        .map(|bytes|serde_json::from_slice::<Vec<i32>>(&bytes).unwrap().into_iter().map(|ms|combat_clip.timeline_at_source_ms(ms)).collect())
        .unwrap_or_else(||vec![11.15,18.183,39.133,77.783,87.75,111.517,123.233,136.5]);
    let mut harness=GhostRaceRun {
        app:App::new(base,None,scene::MapSource::Bsp("mp/ffa3".into()),false,event_loop.create_proxy()).unwrap(),
        clip:combat_clip,
        shots:serde_json::from_value(json["director_shots"].clone()).unwrap(),
        times:combat_times,
        stage:0,stage_at:Instant::now(),started:Instant::now(),initialized:false,stage_set:false,pending:false,
        captures:0,shutdown:false,ghost_pixel_rois:Vec::new(),combat:true,combat_errors:Vec::new(),
    };
    event_loop.run_app(&mut harness).unwrap();
    assert_eq!(harness.captures,harness.times.len());
    assert!(harness.combat_errors.is_empty(),"{}",harness.combat_errors.join("\n"));
}
impl ApplicationHandler<UserEvent> for GhostRaceRun {
    fn resumed(&mut self,e:&ActiveEventLoop) {self.app.resumed(e);}
    fn window_event(&mut self,e:&ActiveEventLoop,id:winit::window::WindowId,event:winit::event::WindowEvent) {self.app.window_event(e,id,event);}
    fn user_event(&mut self,e:&ActiveEventLoop,event:UserEvent) {
        if let UserEvent::RendererError(error)=&event {panic!("Ghost race GPU: {error}");}
        if let UserEvent::ScreenshotFinished(Ok(crate::runtime::ScreenshotOutput::VideoFrame{width,height,bgra,..}))=&event {
            assert!(self.pending);
            let rgba:Vec<u8>=bgra.chunks_exact(4).flat_map(|p|[p[2],p[1],p[0],255]).collect();
            assert!(rgba.chunks_exact(4).map(|p|[p[0],p[1],p[2]]).collect::<std::collections::HashSet<_>>().len()>64);
            std::fs::create_dir_all("artifacts").unwrap();
            image::save_buffer(format!("artifacts/studio-{}-{}.png",if self.combat {"combat"} else {"ghost-race"},self.stage),&rgba,*width,*height,image::ColorType::Rgba8).unwrap();
            if self.stage==0 && !self.combat {
                // Project actual ghost body bounds into the current Program
                // viewport. A fixed screenshot rectangle assumed one saved
                // project/layout and missed actors after camera changes.
                assert!(!self.ghost_pixel_rois.is_empty());
                let mut pixels=std::collections::HashSet::new();
                for &[left,top,right,bottom] in &self.ghost_pixel_rois {
                    for y in top.min(*height)..bottom.min(*height) {for x in left.min(*width)..right.min(*width) {
                        let p=&rgba[((y*width+x)*4) as usize..][..3];
                        let max=*p.iter().max().unwrap();let min=*p.iter().min().unwrap();
                        if max>100 && max.saturating_sub(min)>45 {pixels.insert((x,y));}
                    }}
                }
                assert!(pixels.len()>75,"colored ghost bodies must appear inside projected actor bounds, not only labels ({} pixels)",pixels.len());
            }
            self.pending=false;self.captures+=1;self.stage+=1;self.stage_set=false;
        }
        let ready=matches!(event,UserEvent::RendererReady{..});
        self.app.user_event(e,event);
        if ready {
            self.app.front_end=true;self.app.frontend_page=FrontendPage::RenderStudio;
            if std::env::var_os("DJK_STUDIO_TEST_NO_MODEL_CACHE").is_some() {
                for command in ["r_ghoul2BatchKeyCache 0","r_ghoul2BatchDraws 0","r_modelIndexUploadCache 0","r_md3GeometryCache 0"] {self.app.execute_command_line(command);}
            }
            self.app.render_studio.timeline.demos=vec![self.clip.clone()];
            self.app.render_studio.timeline.director_shots=self.shots.clone();
            self.app.render_studio.timeline.seek(self.times[0]);
            self.app.render_studio.demo_viewport=Some([0,0,1280,720]);
            self.initialized=true;
        }
    }
    fn about_to_wait(&mut self,e:&ActiveEventLoop) {
        assert!(self.started.elapsed()<Duration::from_secs(210),"ghost test timeout: {}",self.app.render_studio.status);
        if self.shutdown {
            if self.app.render.as_ref().is_none_or(|r|r.is_finished()) {self.app.render=None;e.exit();}
            e.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(Instant::now()+Duration::from_millis(10)));return;
        }
        self.app.about_to_wait(e);
        if !self.initialized {return;}
        assert!(self.app.game_session.is_none());
        assert!(!self.app.render_studio.sources.iter().any(|s|s.failed),"{}",self.app.render_studio.status);
        assert!(!self.app.render_studio.race_runtime.iter().any(|r|r.error.is_some()),"{}",self.app.render_studio.status);
        if self.stage==self.times.len() {
            println!("[GHOST RACE GPU] {} phases captured with an opaque reference and eight colored ghost shells",self.captures);
            self.shutdown=true;if let Some(render)=&self.app.render {render.request_shutdown();}return;
        }
        if !self.stage_set {
            self.app.render_studio.timeline.seek(self.times[self.stage]);
            self.stage_at=Instant::now();self.stage_set=true;return;
        }
        if self.pending||self.stage_at.elapsed()<if self.stage==0 {Duration::from_secs(4)}else{Duration::from_millis(600)} {return;}
        let Some(scene)=&self.app.render_studio.scene_frame else{return};
        if self.combat {
            use jka_movement::TraceWorld;
            let source = &mut self.app.render_studio.sources[0];
            let ms = self.clip.source_time_ms(self.times[self.stage]);
            if source.last_sample_ms != Some(ms) { return; }
            let client = source.last_viewed_client.expect("camera must identify its actual actor");
            let ps = source.playback.presented_player_state().unwrap();
            let origin = if ps.field_i32("clientNum") == Some(client) {
                super::super::playerstate_vec3(ps, "origin").unwrap()
            } else { source.playback.demo_fake_view(client).expect("visible subject").native_origin };
            eprintln!("[COMBAT CAMERA] time {} camera {:?} shot {:?} client {client} origin {origin:?} recorded {:?} observer {:?}",self.times[self.stage],
                scene.outgoing.camera.position,scene.outgoing.shot_camera.position,ps.field_i32("clientNum"),self.app.render_studio.spatial.scene_camera);
            for height in [12.0,48.0] {
                let mut point=origin; point[2]+=height;
                let p=scene.outgoing.camera.project_to_screen(1280,960,glam::Vec3::from_array(scene::render_position(point)));
                if !p.is_some_and(|p|p.x>0.0 && p.x<1280.0 && p.y>0.0 && p.y<960.0) {
                    self.combat_errors.push(format!("subject offscreen at {}: {p:?}",self.times[self.stage]));
                }
                let trace=source.collision.as_mut().unwrap().trace(jka_movement::TraceQuery {
                    start:scene::jka_position(scene.outgoing.camera.position.to_array()),end:point,
                    mins:[0.0;3],maxs:[0.0;3],pass_entity:-1,mask:0x1|0x10|0x1000,
                });
                if trace.start_solid!=0 || trace.fraction<=0.99 {self.combat_errors.push(format!("wall hides subject at {}",self.times[self.stage]));}
            }
            if !scene.outgoing.dynamic_models.iter().any(|m|m.entity_num==client as u16 && m.raster_visible && m.vertex_count()>0) {self.combat_errors.push(format!("subject body missing at {}",self.times[self.stage]));}
            eprintln!("[COMBAT GPU] {}s source {ms}, visible client {client}",self.times[self.stage]);
            self.app.render_command(RenderCommand::CaptureVideoFrame{frame_index:self.stage as u64,depth:false,alpha:false});
            self.pending=true;return;
        }
        let sample=self.clip.comparison.as_ref().unwrap().sample(self.times[self.stage]);
        let Some(hud)=scene.outgoing.hud.as_ref().and_then(|h|h.studio_race.as_ref()) else{return};
        if hud.phase!=self.clip.comparison.as_ref().unwrap().phase_label(sample.phase) {return;}
        if self.stage == 6 {
            if let Some(map) = &hud.map {
                let (vertices, triangles) = map.diagnostic_mesh();
                let runners: Vec<_> = hud.runners.iter().map(|r| serde_json::json!({"name":r.name,"color":r.color,"position":r.position,"focused":r.focused,"rank":r.rank})).collect();
                std::fs::write("artifacts/studio-map-preview-data.json", serde_json::to_vec(&serde_json::json!({"vertices":vertices,"triangles":triangles,"runners":runners,"route":hud.route})).unwrap()).unwrap();
            }
        }
        assert_eq!(hud.runners.len(),9);
        assert_eq!(self.app.render_studio.sources.len(),1,"one world context for all runners");
        let models=&scene.outgoing.dynamic_models;
        let reference=60000+self.clip.comparison.as_ref().unwrap().focus(sample.phase) as u16;
        for id in 60000..60000+hud.runners.len() as u16 {
            assert!(models.iter().any(|m|m.entity_num==id&&m.raster_visible&&m.vertex_count()>0&&m.alpha_mode!=crate::renderer::DynamicModelAlphaMode::SightShellNoDepth),"missing runner {id} at stage {}",self.stage);
            if id==reference {
                assert!(!models.iter().any(|m|m.entity_num==id&&m.alpha_mode==crate::renderer::DynamicModelAlphaMode::SightShellNoDepth),"reference must not have a ghost shell");
                assert!(models.iter().any(|m|m.entity_num==id&&matches!(m.alpha_mode,crate::renderer::DynamicModelAlphaMode::Opaque|crate::renderer::DynamicModelAlphaMode::Mask)),"reference must include an opaque body");
            }else{
                assert!(models.iter().any(|m|m.entity_num==id&&m.alpha_mode==crate::renderer::DynamicModelAlphaMode::SightShellNoDepth),"missing silhouette {id}");
            }
        }
        assert_eq!(self.app.render_studio.sources[0].last_sample_ms,Some(sample.world_ms));
        self.ghost_pixel_rois.clear();
        if self.stage==0 {
            let [vx,vy,vw,vh]=self.app.render_studio.demo_viewport.unwrap_or([0,0,1280,720]);
            for runtime in &self.app.render_studio.race_runtime {
                let Some(ghost)=&runtime.ghost else {continue};
                if ghost.entity_num==reference {continue;}
                let Some(sample)=ghost.visual_sample else {continue};
                let mut low=glam::Vec2::splat(f32::INFINITY);let mut high=glam::Vec2::splat(f32::NEG_INFINITY);
                for x in [-18.0,18.0] {for y in [-18.0,18.0] {for z in [6.0,56.0] {
                    let jka=glam::Vec3::from_array(sample.origin)+glam::Vec3::new(x,y,z);
                    let world=glam::Vec3::from_array(scene::render_position(jka.to_array()));
                    if let Some(p)=scene.outgoing.camera.project_to_screen(vw,vh,world) {
                        let p=glam::Vec2::new(p.x+vx as f32,p.y+vy as f32);low=low.min(p);high=high.max(p);
                    }
                }}}
                if low.is_finite() && high.is_finite() {self.ghost_pixel_rois.push([low.x.max(0.0) as u32,low.y.max(0.0) as u32,high.x.ceil().max(0.0) as u32,high.y.ceil().max(0.0) as u32]);}
            }
        }
        self.app.render_command(RenderCommand::CaptureVideoFrame{frame_index:self.stage as u64,depth:false,alpha:false});
        self.pending=true;
    }
}
