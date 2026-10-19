//! Background worker for frontend catalog scans and image decodes.
//!
//! The egui menu lives on the main thread, so anything that opens the VFS,
//! reads package data or decodes an image must not run inside a page build. The
//! worker owns one long-lived `AssetSearchPath` (reopened when the game
//! directories or the app-side generation change) and answers requests in FIFO
//! order. The main thread only installs results.

use super::frontend::{
    self, AssetEntry, ProfileModelEntry, ProfileSaberEntry, SoloMapEntry,
};
use crate::{
    chat_log::{BrowserDetail as ChatLogDetail, BrowserIndexEntry as ChatLogIndexEntry},
    thread_activity::{self, Task, ThreadSlot},
};
use jka_assets::pk3::AssetSearchPath;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Instant,
};

/// An image decoded off-thread; the main thread only uploads it.
pub(super) struct DecodedImage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

pub(super) enum CatalogRequest {
    AssetViewer,
    ModelThumbnail { qpath: String },
    ConsoleFiles,
    DemoGames { token: u64 },
    Demos { token: u64 },
    ChatLogs,
    ChatLogDetail { path: PathBuf },
    Profile,
    SoloMaps,
    SourceMaps,
    Levelshot { source: bool, map_name: String },
    ProfileBackground { map_name: String },
    ProfileBackgroundMaps,
    ProfileIcon { key: String, model: String, skin: String },
    /// `gfx/2d/crosshair*` thumbnail for the settings picker, by `cg_crosshairImage`.
    CrosshairImage { index: u8 },
    ShaderNames,
    ShaderThumbnail { map_label: String, source: String, image_path: String },
    ScreenshotCatalog,
    ScreenshotImage { path: PathBuf },
}

type ProfileCatalog = (Vec<ProfileModelEntry>, Vec<ProfileSaberEntry>);

pub(super) enum CatalogPayload {
    AssetViewer(Result<Vec<AssetEntry>, String>),
    ModelThumbnail { qpath: String, image: Option<DecodedImage> },
    ConsoleFiles(Result<frontend::ConsoleFileCatalog, String>),
    DemoGames { token: u64, games: Result<Vec<String>, String> },
    Demos { token: u64, game: Option<PathBuf>, demos: Result<Vec<frontend::DemoEntry>, String> },
    ChatLogs(Result<Vec<ChatLogIndexEntry>, String>),
    ChatLogDetail { path: PathBuf, detail: Result<ChatLogDetail, String> },
    Profile(Result<ProfileCatalog, String>),
    SoloMaps(Result<Vec<SoloMapEntry>, String>),
    SourceMaps(Result<Vec<SoloMapEntry>, String>),
    Levelshot { source: bool, map_name: String, image: Option<DecodedImage> },
    ProfileBackground { map_name: String, image: Option<DecodedImage> },
    ProfileBackgroundMaps(Vec<String>),
    ProfileIcon { key: String, image: Option<DecodedImage> },
    CrosshairImage { index: u8, image: Option<DecodedImage> },
    ShaderNames(Vec<String>),
    ShaderThumbnail { map_label: String, source: String, image: Option<DecodedImage> },
    ScreenshotCatalog(Result<Vec<crate::screenshot::ScreenshotEntry>, String>),
    ScreenshotImage { path: PathBuf, image: Result<DecodedImage, String> },
}

pub(super) struct CatalogResult {
    pub generation: u64,
    pub payload: CatalogPayload,
}

struct Job {
    generation: u64,
    base: PathBuf,
    game: Option<PathBuf>,
    request: CatalogRequest,
}

/// Which catalog scans are currently in flight.
#[derive(Default, Clone, Copy)]
pub(super) struct PendingCatalogs {
    pub asset_viewer: bool,
    pub console_files: bool,
    pub chat_logs: bool,
    pub chat_log_detail: bool,
    pub profile: bool,
    pub solo_maps: bool,
    pub source_maps: bool,
    pub screenshots: bool,
}

pub(super) struct UiCatalog {
    tx: Sender<Job>,
    rx: Receiver<CatalogResult>,
    generation: u64,
    pub pending: PendingCatalogs,
    /// Profile icon keys requested but not yet answered.
    pub icons_inflight: HashSet<String>,
    /// Profile icon keys that resolved to no usable image.
    pub icons_missing: HashSet<String>,
}

impl UiCatalog {
    pub fn spawn() -> Result<Self, String> {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<CatalogResult>();
        thread::Builder::new()
            .name("ui-catalog".to_owned())
            .spawn(move || worker_main(job_rx, result_tx))
            .map_err(|error| format!("could not start UI catalog worker: {error}"))?;
        Ok(Self {
            tx: job_tx,
            rx: result_rx,
            generation: 0,
            pending: PendingCatalogs::default(),
            icons_inflight: HashSet::new(),
            icons_missing: HashSet::new(),
        })
    }

    /// Invalidate everything in flight, e.g. after the game directory changed.
    /// Results from older generations are dropped on arrival and the worker
    /// reopens its VFS for the next request.
    pub fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = PendingCatalogs::default();
        self.icons_inflight.clear();
        self.icons_missing.clear();
    }

    pub fn request(&mut self, base: &std::path::Path, game: Option<&std::path::Path>, request: CatalogRequest) {
        let job = Job {
            generation: self.generation,
            base: base.to_path_buf(),
            game: game.map(std::path::Path::to_path_buf),
            request,
        };
        if self.tx.send(job).is_err() {
            eprintln!("UI catalog worker is not running; request dropped");
        }
    }

    /// Drain finished results belonging to the current generation.
    pub fn drain(&mut self) -> Vec<CatalogResult> {
        let mut done = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            if result.generation == self.generation {
                done.push(result);
            }
        }
        done
    }
}

fn decode(asset: frontend::LevelshotAsset, what: &str) -> Option<DecodedImage> {
    match image::load_from_memory_with_format(&asset.bytes, asset.format) {
        Ok(image) => {
            let rgba = image.into_rgba8();
            Some(DecodedImage {
                size: [rgba.width() as usize, rgba.height() as usize],
                rgba: rgba.into_raw(),
            })
        }
        Err(error) => {
            eprintln!("{what} decode failed: {error}");
            None
        }
    }
}

struct OpenVfs {
    generation: u64,
    base: PathBuf,
    game: Option<PathBuf>,
    assets: AssetSearchPath,
}

fn worker_main(jobs: Receiver<Job>, results: Sender<CatalogResult>) {
    let mut vfs: Option<OpenVfs> = None;
    while let Ok(job) = jobs.recv() {
        let _activity = thread_activity::activity(ThreadSlot::UiCatalog, Task::UiCatalog);
        let started = Instant::now();

        // Chat logs are ordinary writable files, not VFS assets. Keep their
        // potentially large recursive scan/parsing on this worker, but do not
        // pay for opening/reopening the game PK3 search path.
        match &job.request {
            CatalogRequest::DemoGames { token } => {
                let payload = CatalogPayload::DemoGames {
                    token: *token,
                    games: frontend::scan_demo_game_directories(&job.base),
                };
                let _ = results.send(CatalogResult { generation: job.generation, payload });
                continue;
            }
            CatalogRequest::Demos { token } => {
                let payload = CatalogPayload::Demos {
                    token: *token,
                    game: job.game.clone(),
                    demos: frontend::scan_demos(&job.base, job.game.as_deref()),
                };
                let _ = results.send(CatalogResult { generation: job.generation, payload });
                continue;
            }
            CatalogRequest::ChatLogs => {
                let payload = CatalogPayload::ChatLogs(crate::chat_log::scan_browser_index(&job.base));
                devprintln!(
                    2,
                    "UI catalog: chat log scan took {:.1} ms",
                    started.elapsed().as_secs_f64() * 1000.0
                );
                let _ = results.send(CatalogResult { generation: job.generation, payload });
                continue;
            }
            CatalogRequest::ChatLogDetail { path } => {
                let path = path.clone();
                let detail = crate::chat_log::read_browser_detail(&path);
                let payload = CatalogPayload::ChatLogDetail { path, detail };
                let _ = results.send(CatalogResult { generation: job.generation, payload });
                continue;
            }
            _ => {}
        }

        // Screenshot browsing is a loose-filesystem job, not a JKA VFS job.
        // Handle it before opening/reopening AssetSearchPath so visiting the
        // browser cannot trigger package work that it does not need.
        match &job.request {
            CatalogRequest::ScreenshotCatalog => {
                let payload = CatalogPayload::ScreenshotCatalog(
                    crate::screenshot::scan_screenshots(&job.base)
                );
                let _ = results.send(CatalogResult { generation: job.generation, payload });
                continue;
            }
            CatalogRequest::ScreenshotImage { path } => {
                let image = crate::screenshot::read_image_rgba(path).map(|(size, rgba)| DecodedImage { size, rgba });
                let payload = CatalogPayload::ScreenshotImage { path: path.clone(), image };
                let _ = results.send(CatalogResult { generation: job.generation, payload });
                continue;
            }
            _ => {}
        }

        let reusable = vfs.as_ref().is_some_and(|open| {
            open.generation == job.generation && open.base == job.base && open.game == job.game
        });
        if !reusable {
            vfs = match AssetSearchPath::open_game(&job.base, job.game.as_deref()) {
                Ok(assets) => Some(OpenVfs {
                    generation: job.generation,
                    base: job.base.clone(),
                    game: job.game.clone(),
                    assets,
                }),
                Err(error) => {
                    let error = error.to_string();
                    let payload = match job.request {
                        CatalogRequest::AssetViewer => Some(CatalogPayload::AssetViewer(Err(error.clone()))),
                        CatalogRequest::ModelThumbnail { qpath } => Some(CatalogPayload::ModelThumbnail { qpath, image: None }),
                        CatalogRequest::ConsoleFiles => Some(CatalogPayload::ConsoleFiles(Err(error))),
                        CatalogRequest::DemoGames { .. } | CatalogRequest::Demos { .. } => unreachable!("demo requests handled before VFS"),
                        // Handled before VFS open above.
                        CatalogRequest::ChatLogs | CatalogRequest::ChatLogDetail { .. } => None,
                        CatalogRequest::Profile => Some(CatalogPayload::Profile(Err(error))),
                        CatalogRequest::SoloMaps => Some(CatalogPayload::SoloMaps(Err(error))),
                        CatalogRequest::SourceMaps => Some(CatalogPayload::SourceMaps(Err(error))),
                        CatalogRequest::Levelshot { source, map_name } => {
                            Some(CatalogPayload::Levelshot { source, map_name, image: None })
                        }
                        CatalogRequest::ProfileBackground {map_name}=>Some(CatalogPayload::ProfileBackground{map_name,image:None}),
                        CatalogRequest::ProfileBackgroundMaps=>Some(CatalogPayload::ProfileBackgroundMaps(Vec::new())),
                        CatalogRequest::ProfileIcon { key, .. } => {
                            Some(CatalogPayload::ProfileIcon { key, image: None })
                        }
                        CatalogRequest::CrosshairImage { index } => {
                            Some(CatalogPayload::CrosshairImage { index, image: None })
                        }
                        CatalogRequest::ShaderNames => Some(CatalogPayload::ShaderNames(Vec::new())),
                        CatalogRequest::ShaderThumbnail { map_label, source, .. } => {
                            Some(CatalogPayload::ShaderThumbnail { map_label, source, image: None })
                        }
                        CatalogRequest::ScreenshotCatalog | CatalogRequest::ScreenshotImage { .. } => unreachable!("filesystem-only screenshot request reached VFS open"),
                    };
                    if let Some(payload) = payload {
                        let _ = results.send(CatalogResult { generation: job.generation, payload });
                    }
                    continue;
                }
            };
            devprintln!(2, "UI catalog: VFS opened in {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
        }
        let assets = &mut vfs.as_mut().expect("VFS opened above").assets;

        let scan_started = Instant::now();
        let (label, payload) = match job.request {
            CatalogRequest::DemoGames { .. } | CatalogRequest::Demos { .. } => unreachable!("demo requests handled before VFS"),
            CatalogRequest::ConsoleFiles => (
                "console files",
                CatalogPayload::ConsoleFiles(Ok(frontend::scan_console_files(assets))),
            ),
            CatalogRequest::ModelThumbnail { qpath } => {
                let image = assets.read(&qpath, 32 * 1024 * 1024).ok().flatten()
                    .and_then(|file| jka_assets::md3::parse(&file.bytes).ok())
                    .and_then(|model| raster_md3_thumbnail(&model));
                ("model thumbnail", CatalogPayload::ModelThumbnail { qpath, image })
            }
            CatalogRequest::AssetViewer => (
                "asset viewer",
                CatalogPayload::AssetViewer(frontend::scan_asset_viewer_assets(assets)),
            ),
            CatalogRequest::ChatLogs | CatalogRequest::ChatLogDetail { .. } => {
                unreachable!("chat log requests are handled before VFS open")
            }
            CatalogRequest::Profile => (
                "profile",
                CatalogPayload::Profile(frontend::scan_profile_catalog(assets)),
            ),
            CatalogRequest::SoloMaps => (
                "solo maps",
                CatalogPayload::SoloMaps(Ok(frontend::scan_solo_maps(assets))),
            ),
            CatalogRequest::SourceMaps => (
                "source maps",
                CatalogPayload::SourceMaps(Ok(frontend::scan_source_maps(assets))),
            ),
            CatalogRequest::Levelshot { source, map_name } => {
                let image = frontend::read_levelshot(assets, &map_name)
                    .and_then(|asset| decode(asset, &format!("Levelshot {map_name}")));
                ("levelshot", CatalogPayload::Levelshot { source, map_name, image })
            }
            CatalogRequest::ProfileIcon { key, model, skin } => {
                let image = frontend::read_profile_icon(assets, &model, &skin)
                    .and_then(|asset| decode(asset, &format!("Profile icon {key}")));
                ("profile icon", CatalogPayload::ProfileIcon { key, image })
            }
            CatalogRequest::ProfileBackground{map_name}=>{
                let image=frontend::read_levelshot(assets,&map_name).and_then(|asset|decode(asset,&format!("Profile background {map_name}")))
                    .map(|image|{
                        if image.size[0]<=2048&&image.size[1]<=2048{return image;}
                        let raw=image::RgbaImage::from_raw(image.size[0] as u32,image.size[1] as u32,image.rgba).expect("decoded RGBA size");
                        let resized=image::DynamicImage::ImageRgba8(raw).thumbnail(2048,2048).into_rgba8();
                        DecodedImage{size:[resized.width() as usize,resized.height() as usize],rgba:resized.into_raw()}
                    });
                ("profile background",CatalogPayload::ProfileBackground{map_name,image})
            }
            CatalogRequest::ProfileBackgroundMaps=>{
                let names:HashSet<_>=assets.names().map(|n|n.to_ascii_lowercase()).collect();
                let maps=frontend::scan_solo_maps(assets).into_iter().map(|m|m.map_name).filter(|name|{
                    [name.as_str(),name.rsplit('/').next().unwrap_or(name)].iter().any(|stem|
                        ["jpg","jpeg","png","tga"].iter().any(|ext|names.contains(&format!("levelshots/{stem}.{ext}"))))
                }).collect();
                ("profile map images",CatalogPayload::ProfileBackgroundMaps(maps))
            }
            CatalogRequest::CrosshairImage { index } => {
                let image = frontend::read_crosshair_image(assets, index)
                    .and_then(|asset| decode(asset, &format!("Crosshair image {index}")));
                ("crosshair image", CatalogPayload::CrosshairImage { index, image })
            }
            CatalogRequest::ShaderNames => {
                let mut names: Vec<String> = assets.names().filter_map(|name| {
                    let lower = name.to_ascii_lowercase();
                    [".tga", ".jpg", ".jpeg", ".png"]
                        .iter().find_map(|ext| lower.strip_suffix(ext))
                        .map(str::to_owned)
                }).collect();
                names.sort_unstable();
                names.dedup();
                ("shader names", CatalogPayload::ShaderNames(names))
            }
            CatalogRequest::ShaderThumbnail { map_label, source, image_path } => {
                // The UI worker owns VFS reads and decoding. Bound previews
                // to 96px so a map with thousands of materials stays cheap.
                let mut decoded = None;
                let stem = crate::scene::canonical_shader_name(&image_path);
                for ext in ["tga", "png", "jpg", "jpeg"] {
                    if let Ok(Some(asset)) = assets.read(&format!("{stem}.{ext}"), 8 * 1024 * 1024) {
                        if let Ok(raw) = image::load_from_memory(&asset.bytes) {
                            let preview = raw.thumbnail(96, 96).to_rgba8();
                            decoded = Some(DecodedImage {
                                size: [preview.width() as usize, preview.height() as usize],
                                rgba: preview.into_raw(),
                            });
                            break;
                        }
                    }
                }
                ("shader thumbnail", CatalogPayload::ShaderThumbnail { map_label, source, image: decoded })
            }
            CatalogRequest::ScreenshotCatalog | CatalogRequest::ScreenshotImage { .. } => unreachable!("filesystem-only screenshot request reached VFS scan"),
        };
        // Bulk scans are the interesting ones; per-image jobs are too chatty.
        if !matches!(
            payload,
            CatalogPayload::Levelshot { .. }
                | CatalogPayload::ProfileBackground { .. }
                | CatalogPayload::ProfileIcon { .. }
                | CatalogPayload::CrosshairImage { .. }
                | CatalogPayload::ShaderThumbnail { .. }
        ) {
            devprintln!(
                2,
                "UI catalog: {label} scan took {:.1} ms",
                scan_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        let _ = results.send(CatalogResult { generation: job.generation, payload });
    }
}


/// Software thumbnail of real MD3 frame-zero mesh geometry. The catalog worker
/// handles all triangles and the UI receives just a small RGBA icon; no GPU
/// allocations or asset disk reads happen while scrolling the picker. Thumbs
/// deliberately use neutral material so a missing image cannot hide geometry.
fn raster_md3_thumbnail(model: &jka_assets::md3::Model) -> Option<DecodedImage> {
    const W: usize = 96;
    const H: usize = 96;
    let (yaw, elevation) = (0.65_f32, 0.43_f32);
    let (sy,cy) = yaw.sin_cos();
    let (sp,cp) = elevation.sin_cos();
    let project = |p: [f32; 3]| -> [f32; 3] {
        let sideways = sy*p[0] + cy*p[1];
        [cy*p[0] - sy*p[1], -sp*sideways + cp*p[2], cp*sideways + sp*p[2]]
    };
    let mut range_min = [f32::MAX; 2];
    let mut range_max = [f32::MIN; 2];
    let mut any = false;
    for surface in &model.surfaces {
        for vertex in &surface.vertices {
            let p = project(vertex.position);
            if !p.iter().all(|v| v.is_finite()) { continue; }
            for a in 0..2 {
                range_min[a] = range_min[a].min(p[a]);
                range_max[a] = range_max[a].max(p[a]);
            }
            any = true;
        }
    }
    if !any { return None; }
    let center = [(range_min[0]+range_max[0])*0.5, (range_min[1]+range_max[1])*0.5];
    let width = (range_max[0]-range_min[0]).max(0.01);
    let height = (range_max[1]-range_min[1]).max(0.01);
    let scale = 83.0 / width.max(height);
    let screen = |p: [f32; 3]| -> [f32; 3] {
        let v = project(p);
        [48.0+(v[0]-center[0])*scale, 48.0-(v[1]-center[1])*scale, v[2]]
    };
    let mut rgba = vec![0u8; W*H*4];
    for pixel in rgba.chunks_exact_mut(4) { pixel.copy_from_slice(&[25,34,46,255]); }
    let mut depth = vec![f32::NEG_INFINITY; W*H];
    for surface in &model.surfaces {
        let vertices: Vec<_> = surface.vertices.iter().map(|v| screen(v.position)).collect();
        for face in surface.indices.chunks_exact(3) {
            let (Some(a),Some(b),Some(c)) = (
                vertices.get(face[0] as usize),
                vertices.get(face[1] as usize),
                vertices.get(face[2] as usize)
            ) else { continue; };
            let area = (b[0]-a[0])*(c[1]-a[1])-(b[1]-a[1])*(c[0]-a[0]);
            if area.abs() < 0.001 { continue; }
            let lo_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
            let hi_x = a[0].max(b[0]).max(c[0]).ceil().min(95.0) as usize;
            let lo_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
            let hi_y = a[1].max(b[1]).max(c[1]).ceil().min(95.0) as usize;
            let brightness = (105.0 + area.abs().sqrt().min(90.0)*0.33).round() as u8;
            for y in lo_y..=hi_y {
                for x in lo_x..=hi_x {
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    let u = ((b[0]-px)*(c[1]-py)-(b[1]-py)*(c[0]-px))/area;
                    let v = ((c[0]-px)*(a[1]-py)-(c[1]-py)*(a[0]-px))/area;
                    let w = 1.0-u-v;
                    if u < -0.001 || v < -0.001 || w < -0.001 { continue; }
                    let z = u*a[2]+v*b[2]+w*c[2];
                    let i = y*W+x;
                    if z <= depth[i] { continue; }
                    depth[i] = z;
                    rgba[i*4..i*4+4].copy_from_slice(&[
                        brightness.saturating_sub(18), brightness,
                        brightness.saturating_add(24), 255
                    ]);
                }
            }
        }
    }
    Some(DecodedImage { size:[W,H], rgba })
}
