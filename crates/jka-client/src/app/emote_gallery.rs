//! `emote_gallery`: offline visual QA for the emote library.
//!
//! Runs on the isolated preview stage from the main menu (no map: only the
//! assets an emote needs), or inside a loaded local map. Copies of the local
//! player model are posed in front of the camera from four angles side by side, at scrubbed
//! animation times rather than in real time: one frame before the emote
//! starts, then evenly spaced samples across the clip. Each pose is captured
//! with the regular in-game screenshot path, and `report.json` records
//! per-emote warnings (missing props, track mismatches), audio status and
//! effect renderer counts for triage.
//!
//! Emote assets are prepared on background workers ahead of the renderer.
use super::*;
use crate::renderer::ScreenshotFormat;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

/// Model yaws relative to facing the camera: front, three-quarter, side, back.
const VIEWS: [f32; 4] = [0.0, 45.0, 90.0, 180.0];
/// Distance in front of the player and spacing between the side-by-side views.
const DEPTH: f32 = 330.0;
const SPACING: f32 = 95.0;
/// Wall time a new pose stays on screen before it is captured. The event loop
/// turns far faster than frames present, so this is time-based.
const SETTLE: Duration = Duration::from_millis(250);
/// Grace period after the local player exists before the gallery starts.
const STARTUP_GRACE: Duration = Duration::from_secs(4);
/// How long to wait for the player model / an emote's assets to load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(45);
/// Stage backdrop (dark grey, so both glows and dark smoke read).
const BACKDROP: [u8; 4] = [46, 48, 52, 255];
/// A capture with fewer pixels away from the backdrop/black is not yet drawn.
const MIN_DRAWN: f64 = 0.002;
const PREPARE_WORKERS: usize = 4;
/// Automatic sampling density: enough to catch short-lived particles.
const AUTO_PER_SECOND: f32 = 4.0;

type Prepared = Result<(crate::emotes::Emote, Vec<String>), String>;

/// Where the views stand: JKA world origin of the first view, the direction
/// from the views to the camera, and the sideways axis between views.
#[derive(Clone, Copy)]
struct Anchor {
    origin: [f32; 3],
    toward_camera: [f32; 2],
    side: [f32; 2],
}

pub(super) struct EmoteGallery {
    /// Catalogue positions (0-based; the Profile list shows position + 1).
    queue: Vec<usize>,
    /// Samples per emote; 0 = `AUTO_PER_SECOND` across each clip.
    samples: usize,
    /// Samples for the emote being rendered.
    emote_samples: usize,
    next: usize,
    current: Option<(usize, crate::emotes::Emote, Vec<String>)>,
    sample: usize,
    capture_at: Option<Instant>,
    /// Directory the pending screenshot is written into.
    capture: Option<PathBuf>,
    retries: u8,
    waiting_since: Option<Instant>,
    anchor: Anchor,
    /// Empty preview stage (no map) rather than the loaded world.
    stage: bool,
    out: PathBuf,
    report: Vec<serde_json::Value>,
    ready: HashMap<usize, Prepared>,
    results: mpsc::Receiver<(usize, Prepared)>,
    started: Instant,
}

impl App {
    /// `emote_gallery <first> [count] [samples]` with 1-based Profile numbers,
    /// or `emote_gallery ids <EID,...> [samples]`.
    pub(super) fn start_emote_gallery(&mut self, args: &[&str]) -> Result<String, String> {
        let catalog = crate::emotes::catalog(&self.base)?;
        let usage = "usage: emote_gallery <first> [count] [samples] | nums <n,...> [samples] | ids <EID,...> [samples]";
        let (queue, samples) = match args {
            ["nums", nums, rest @ ..] => {
                let queue = nums.split(',').filter_map(|n| n.trim().parse::<usize>().ok()).filter(|&n| n >= 1 && n <= catalog.len()).map(|n| n - 1).collect::<Vec<_>>();
                (queue, rest.first().and_then(|s| s.parse().ok()).unwrap_or(8))
            }
            ["ids", ids, rest @ ..] => {
                let wanted: Vec<&str> = ids.split(',').collect();
                let queue = catalog.iter().enumerate().filter(|(_, e)| wanted.iter().any(|w| w.eq_ignore_ascii_case(&e.id))).map(|(i, _)| i).collect::<Vec<_>>();
                (queue, rest.first().and_then(|s| s.parse().ok()).unwrap_or(8))
            }
            [first, rest @ ..] => {
                let first = first.parse::<usize>().map_err(|_| usage)?.max(1) - 1;
                let count = rest.first().and_then(|s| s.parse::<usize>().ok()).unwrap_or(16);
                ((first..(first + count).min(catalog.len())).collect(), rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(8))
            }
            [] => return Err(usage.into()),
        };
        if queue.is_empty() {
            return Err("emote_gallery: no matching emotes".into());
        }
        // world_position() and the camera are in render space (Y up).
        let half = (VIEWS.len() - 1) as f32 * 0.5;
        let stage = self.game_session.is_none();
        let anchor = if stage {
            // Stage camera at the origin looking down +X (JKA space).
            Anchor { origin: [DEPTH, half * SPACING, -22.0], toward_camera: [-1.0, 0.0], side: [0.0, -1.0] }
        } else {
            // world_position() and the camera are in render space (Y up).
            let player = crate::scene::jka_position(self.local_server.as_ref().ok_or("emote_gallery: load a local map first")?.world_position().to_array());
            // Views stand on the player's floor, in front of the camera.
            let forward = crate::scene::jka_position(self.camera.forward().to_array());
            let len = (forward[0] * forward[0] + forward[1] * forward[1]).sqrt().max(1e-4);
            let f = [forward[0] / len, forward[1] / len];
            let side = [f[1], -f[0]];
            Anchor {
                origin: [player[0] + f[0] * DEPTH - side[0] * half * SPACING, player[1] + f[1] * DEPTH - side[1] * half * SPACING, player[2]],
                toward_camera: [-f[0], -f[1]],
                side,
            }
        };
        let out = self.game_write_path("emote-gallery").join(run_stamp());
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let (tx, results) = mpsc::channel();
        let work = Arc::new(std::sync::Mutex::new(queue.iter().map(|&i| (i, catalog[i].clone())).collect::<std::collections::VecDeque<_>>()));
        for n in 0..PREPARE_WORKERS {
            let (tx, work) = (tx.clone(), Arc::clone(&work));
            let _ = std::thread::Builder::new().name(format!("emote-gallery-{n}")).spawn(move || loop {
                let Some((index, root)) = work.lock().ok().and_then(|mut q| q.pop_front()) else { break };
                let variant = crate::emotes::default_variant_index(&root);
                let emote = root.variants.get(variant).cloned().unwrap_or(root);
                if tx.send((index, crate::emotes::prepare_visuals(emote))).is_err() {
                    break;
                }
            });
        }
        let count = queue.len();
        self.emote_gallery = Some(EmoteGallery {
            queue,
            samples: samples.min(64),
            emote_samples: 0,
            next: 0,
            current: None,
            sample: 0,
            capture_at: None,
            capture: None,
            retries: 0,
            waiting_since: None,
            anchor,
            stage,
            out: out.clone(),
            report: Vec::new(),
            ready: HashMap::new(),
            results,
            started: Instant::now(),
        });
        if stage {
            self.profile_preview_active = true;
            self.render_command(RenderCommand::SetAssetPreviewBackground(Some(([2, 2], BACKDROP.repeat(4)))));
            self.render_command(RenderCommand::SetAssetPreviewMode(true));
            // The preview pass draws nothing without a viewport: use the window.
            let size = self.window.as_ref().map(|w| w.inner_size()).map_or([1920, 1080], |s| [s.width.max(1), s.height.max(1)]);
            self.render_command(RenderCommand::SetAssetPreviewViewport(Some([0, 0, size[0], size[1]])));
        } else {
            // No HUD/crosshair in the captures.
            let _ = self.set_console_cvar("cg_draw2D", "0");
        }
        Ok(format!("emote_gallery: {count} emotes -> {}", out.display()))
    }

    /// Models drawn into the world scene this frame (merged by render_snapshot).
    pub(super) fn emote_gallery_models(&self) -> Option<&Arc<Vec<DynamicModelSurface>>> {
        self.emote_gallery.as_ref().filter(|g| !g.stage).map(|_| &self.emote_gallery_scene)
    }

    /// The gallery owns the screen: no menus over the stage or the world.
    pub(super) fn emote_gallery_active(&self) -> bool {
        self.emote_gallery.is_some()
    }

    /// Stage camera: level with the models' chest, looking down +X.
    pub(super) fn emote_gallery_camera(&self) -> Option<Camera> {
        self.emote_gallery.as_ref().filter(|g| g.stage).map(|_| Camera::new_with_fov([0.0, -14.0, 0.0], 0.0, 58.0))
    }

    fn emote_gallery_view_camera(&self) -> Camera {
        self.emote_gallery_camera().unwrap_or(self.camera)
    }

    /// Advance the gallery by at most one step per event-loop turn.
    pub(super) fn tick_emote_gallery(&mut self) {
        if self.emote_gallery_request.is_some() {
            // Start once the menu (stage) or a loaded local map is really on
            // screen, after a grace period, with the emote catalogue loaded.
            let menu = self.game_session.is_none() && self.loading.is_none() && self.startup_first_menu_frame_seen;
            let in_game = self.game_session.is_some() && self.local_server.is_some() && self.loading.is_none();
            if !(menu || in_game) {
                self.emote_gallery_menu_seen = None;
            } else if *self.emote_gallery_menu_seen.get_or_insert_with(Instant::now) + STARTUP_GRACE <= Instant::now()
                && crate::emotes::catalog(&self.base).is_ok()
            {
                let args = self.emote_gallery_request.take().unwrap();
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let message = match self.start_emote_gallery(&args) { Ok(m) | Err(m) => m };
                println!("{message}");
                self.push_console_line(message.clone());
                self.console_status = message;
            }
        }
        let Some(mut gallery) = self.emote_gallery.take() else { return };
        while let Ok((index, prepared)) = gallery.results.try_recv() {
            gallery.ready.insert(index, prepared);
        }
        // A screenshot in flight: wait for the file, reject black frames.
        if let Some(dir) = gallery.capture.clone() {
            let Some(path) = std::fs::read_dir(&dir).ok().and_then(|d| d.filter_map(Result::ok).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "png"))) else {
                self.emote_gallery = Some(gallery);
                return;
            };
            // The writer may still be flushing; a decode failure means "not yet".
            let Ok(image) = image::open(&path) else {
                self.emote_gallery = Some(gallery);
                return;
            };
            let px = image.to_rgb8();
            // Fraction of pixels that are neither black nor the backdrop.
            let drawn = px.pixels().filter(|p| {
                let d = |c: [u8; 3]| (0..3).map(|i| (p[i] as i32 - c[i] as i32).abs()).max().unwrap_or(0);
                d([0, 0, 0]) > 12 && d([BACKDROP[0], BACKDROP[1], BACKDROP[2]]) > 12
            }).count() as f64 / (px.width() as f64 * px.height() as f64).max(1.0);
            let blank = drawn < MIN_DRAWN;
            gallery.capture = None;
            if blank && gallery.retries < 4 {
                gallery.retries += 1;
                let _ = std::fs::remove_dir_all(&dir);
                gallery.capture_at = Some(Instant::now() + SETTLE);
                self.emote_gallery = Some(gallery);
                return;
            }
            if blank {
                if let Some((_, _, warnings)) = gallery.current.as_mut() {
                    warnings.push(format!("sample {} stayed blank", gallery.sample));
                }
            }
            gallery.retries = 0;
            gallery.sample += 1;
        }
        // A posted pose waiting for its settle time.
        if let Some(at) = gallery.capture_at {
            if Instant::now() >= at {
                gallery.capture_at = None;
                let (index, emote, _) = gallery.current.as_ref().expect("gallery sample without emote");
                let dir = gallery.out.join(format!("{:04}_{}_{:02}", index + 1, emote.id, gallery.sample));
                let _ = std::fs::create_dir_all(&dir);
                self.render_command(RenderCommand::Screenshot { directory: dir.clone(), metadata: self.build_screenshot_metadata(), format: ScreenshotFormat::Png });
                gallery.capture = Some(dir);
            }
            self.emote_gallery = Some(gallery);
            return;
        }
        // Finished every sample of the current emote: record it, take the next.
        if gallery.current.is_some() && gallery.sample > gallery.emote_samples {
            let (index, emote, mut warnings) = gallery.current.take().unwrap();
            // Pose the whole timeline (no rendering) so ground spikes between
            // captured samples are measured too.
            let duration = ((emote.end_frame - emote.first_frame).max(1) as f32 / emote.fps.max(1.) * 1000.) as i32;
            for elapsed_ms in (0..duration).step_by(50) {
                let _ = self.emote_gallery_draws(gallery.anchor, Some(crate::emotes::Preview { emote: emote.clone(), elapsed_ms }));
            }
            // Source-rig models (e.g. 3_frieren) re-ground every emote frame;
            // a large correction shows as the body bobbing up and down.
            let shift = f32::from_bits(crate::cgame::player_presenter::EMOTE_GROUND_SHIFT_MAX.swap(0, std::sync::atomic::Ordering::Relaxed));
            if shift > GROUND_SHIFT_WARN {
                warnings.push(if shift.is_finite() { format!("ground correction up to {shift:.0} units") } else { "ground correction out of range".into() });
            }
            println!("EMOTE GALLERY {}/{} #{} {}", gallery.next, gallery.queue.len(), index + 1, emote.id);
            gallery.report.push(gallery_record(index, &emote, &warnings, None));
        }
        if gallery.current.is_none() {
            let Some(&index) = gallery.queue.get(gallery.next) else {
                self.finish_emote_gallery(gallery);
                return;
            };
            let Some(prepared) = gallery.ready.remove(&index) else {
                self.emote_gallery = Some(gallery);
                return;
            };
            gallery.next += 1;
            match prepared {
                Ok((emote, warnings)) => {
                    let seconds = (emote.end_frame - emote.first_frame).max(1) as f32 / emote.fps.max(1.);
                    gallery.emote_samples = if gallery.samples == 0 { ((seconds * AUTO_PER_SECOND).round() as usize).clamp(8, 48) } else { gallery.samples };
                    crate::cgame::player_presenter::EMOTE_GROUND_SHIFT_MAX.store(0, std::sync::atomic::Ordering::Relaxed);
                    gallery.current = Some((index, emote, warnings));
                    gallery.sample = 0;
                    gallery.waiting_since = Some(Instant::now());
                }
                Err(error) => {
                    let catalog = crate::emotes::catalog(&self.base).ok();
                    if let Some(emote) = catalog.as_ref().and_then(|c| c.get(index).cloned()) {
                        gallery.report.push(gallery_record(index, &emote, &[], Some(&error)));
                    }
                    self.emote_gallery = Some(gallery);
                    return;
                }
            }
        }
        // Pose this sample: 0 is the stand pose before the emote starts.
        let (_, emote, _) = gallery.current.as_ref().unwrap();
        let duration = ((emote.end_frame - emote.first_frame).max(1) as f32 / emote.fps.max(1.) * 1000.) as i32;
        let preview = (gallery.sample > 0).then(|| crate::emotes::Preview {
            emote: emote.clone(),
            elapsed_ms: ((duration - 1) as f32 * (gallery.sample - 1) as f32 / (gallery.emote_samples.max(2) - 1) as f32) as i32,
        });
        let anchor = gallery.anchor;
        match self.emote_gallery_draws(anchor, preview) {
            // The model and props load asynchronously: an error or an empty
            // pose means "not ready yet", not a result to capture.
            Ok(draws) if !draws.is_empty() => {
                if gallery.stage {
                    self.profile_dynamic_models = Arc::new(draws);
                } else {
                    self.emote_gallery_scene = Arc::new(draws);
                }
                gallery.waiting_since = None;
                gallery.capture_at = Some(Instant::now() + SETTLE);
            }
            result => {
                let waited = gallery.waiting_since.get_or_insert_with(Instant::now).elapsed();
                if waited > LOAD_TIMEOUT {
                    let (index, emote, mut warnings) = gallery.current.take().unwrap();
                    warnings.push(format!("timed out waiting for a pose: {}", result.err().unwrap_or_else(|| "no geometry".into())));
                    gallery.report.push(gallery_record(index, &emote, &warnings, None));
                    gallery.waiting_since = None;
                }
            }
        }
        self.emote_gallery = Some(gallery);
    }

    fn emote_gallery_draws(&mut self, anchor: Anchor, preview: Option<crate::emotes::Preview>) -> Result<Vec<DynamicModelSurface>, String> {
        self.ensure_asset_preview_player_presenter()?;
        self.ensure_asset_preview_entity_presenter()?;
        let info = self.solo_client_info.clone();
        let mut draws = Vec::new();
        {
            let presenter = self.asset_preview_player_presenter.as_mut().expect("gallery presenter");
            presenter.set_skinning_mode(Ghoul2SkinningMode::Cpu);
            presenter.set_early_frustum_cull(false);
            let _ = presenter.drain_fx_requests();
            let facing = anchor.toward_camera[1].atan2(anchor.toward_camera[0]).to_degrees();
            for (i, view) in VIEWS.iter().enumerate() {
                let (s, c) = (facing + view).to_radians().sin_cos();
                let axis = [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]];
                let origin = [anchor.origin[0] + anchor.side[0] * i as f32 * SPACING, anchor.origin[1] + anchor.side[1] * i as f32 * SPACING, anchor.origin[2]];
                let (mut surfaces, _) = presenter.present_profile_player_preview(&info, origin, axis, 0, "BOTH_STAND1", false, [1.0; 3], 0, [0.0; 2], false, preview.clone(), None)?;
                draws.append(&mut surfaces);
            }
            let _ = presenter.drain_fx_requests();
        }
        let worn = self.asset_preview_player_presenter.as_mut().expect("gallery presenter").drain_cosmetic_draws();
        let camera = self.emote_gallery_view_camera();
        let eye = crate::scene::jka_position(camera.position.to_array());
        let entity = self.asset_preview_entity_presenter.as_mut().expect("gallery entity presenter");
        entity.set_material_eye(eye);
        if !worn.is_empty() {
            draws.append(&mut entity.present_cosmetics(worn));
        }
        let (fx, _) = self.asset_preview_player_presenter.as_mut().expect("gallery presenter").drain_emote_fx();
        if !fx.is_empty() {
            let view = crate::fx::draw::FxView::from_camera(&camera);
            let entity = self.asset_preview_entity_presenter.as_mut().expect("gallery entity presenter");
            draws.extend(crate::fx::draw::tessellate(&fx, &view, &mut |shader| entity.fx_material_stages(shader)));
        }
        Ok(draws)
    }

    fn finish_emote_gallery(&mut self, gallery: EmoteGallery) {
        let report = serde_json::json!({
            "emotes": gallery.report,
            "samples": gallery.samples,
            "views": VIEWS,
            "seconds": gallery.started.elapsed().as_secs_f64(),
        });
        let path = gallery.out.join("report.json");
        if let Err(error) = std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap_or_default()) {
            self.push_console_line(format!("^1emote_gallery: {error}"));
        }
        self.push_console_line(format!("emote_gallery: done in {:.1}s -> {}", gallery.started.elapsed().as_secs_f64(), gallery.out.display()));
        println!("EMOTE GALLERY DONE {}", gallery.out.display());
        self.emote_gallery_scene = Arc::new(Vec::new());
        if gallery.stage {
            self.profile_dynamic_models = Arc::new(Vec::new());
            self.profile_preview_active = false;
            self.render_command(RenderCommand::SetAssetPreviewMode(false));
        } else {
            let _ = self.set_console_cvar("cg_draw2D", "1");
        }
    }
}

/// Ground corrections above this (JKA units) are reported as warnings.
const GROUND_SHIFT_WARN: f32 = 40.0;

fn gallery_record(index: usize, emote: &crate::emotes::Emote, warnings: &[String], error: Option<&str>) -> serde_json::Value {
    let effects = emote.effects.as_ref();
    serde_json::json!({
        "number": index + 1,
        "id": emote.id,
        "title": crate::emotes::display_title(emote),
        "variant": emote.variant_name,
        "seconds": (emote.end_frame - emote.first_frame) as f32 / emote.fps.max(1.),
        "audio_status": crate::emotes::audio_status_label(emote),
        "audio_detail": emote.audio_detail,
        "props": emote.props.iter().map(|p| serde_json::json!({"name": p.name, "model": p.model, "posed": p.pose.is_some()})).collect::<Vec<_>>(),
        "effect_renderers": effects.map_or(0, |e| e.renderers.len()),
        "effect_track": effects.is_some_and(|e| e.data.is_some()),
        "gpu_approximated": effects.map_or(0, |e| e.renderers.iter().filter(|r| r.name.contains("GPU approximation")).count()),
        "presentation_detail": emote.presentation_detail,
        "warnings": warnings,
        "error": error,
    })
}

fn run_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("run-{secs}-{}", std::process::id())
}
