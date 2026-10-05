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
    ChatLogs,
    ChatLogDetail { path: PathBuf },
    Profile,
    SoloMaps,
    SourceMaps,
    Levelshot { source: bool, map_name: String },
    ProfileIcon { key: String, model: String, skin: String },
    /// `gfx/2d/crosshair*` thumbnail for the settings picker, by `cg_crosshairImage`.
    CrosshairImage { index: u8 },
    ScreenshotCatalog,
    ScreenshotImage { path: PathBuf },
}

type ProfileCatalog = (Vec<ProfileModelEntry>, Vec<ProfileSaberEntry>);

pub(super) enum CatalogPayload {
    AssetViewer(Result<Vec<AssetEntry>, String>),
    ChatLogs(Result<Vec<ChatLogIndexEntry>, String>),
    ChatLogDetail { path: PathBuf, detail: Result<ChatLogDetail, String> },
    Profile(Result<ProfileCatalog, String>),
    SoloMaps(Result<Vec<SoloMapEntry>, String>),
    SourceMaps(Result<Vec<SoloMapEntry>, String>),
    Levelshot { source: bool, map_name: String, image: Option<DecodedImage> },
    ProfileIcon { key: String, image: Option<DecodedImage> },
    CrosshairImage { index: u8, image: Option<DecodedImage> },
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
                        CatalogRequest::AssetViewer => Some(CatalogPayload::AssetViewer(Err(error))),
                        // Handled before VFS open above.
                        CatalogRequest::ChatLogs | CatalogRequest::ChatLogDetail { .. } => None,
                        CatalogRequest::Profile => Some(CatalogPayload::Profile(Err(error))),
                        CatalogRequest::SoloMaps => Some(CatalogPayload::SoloMaps(Err(error))),
                        CatalogRequest::SourceMaps => Some(CatalogPayload::SourceMaps(Err(error))),
                        CatalogRequest::Levelshot { source, map_name } => {
                            Some(CatalogPayload::Levelshot { source, map_name, image: None })
                        }
                        CatalogRequest::ProfileIcon { key, .. } => {
                            Some(CatalogPayload::ProfileIcon { key, image: None })
                        }
                        CatalogRequest::CrosshairImage { index } => {
                            Some(CatalogPayload::CrosshairImage { index, image: None })
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
            CatalogRequest::CrosshairImage { index } => {
                let image = frontend::read_crosshair_image(assets, index)
                    .and_then(|asset| decode(asset, &format!("Crosshair image {index}")));
                ("crosshair image", CatalogPayload::CrosshairImage { index, image })
            }
            CatalogRequest::ScreenshotCatalog | CatalogRequest::ScreenshotImage { .. } => unreachable!("filesystem-only screenshot request reached VFS scan"),
        };
        // Bulk scans are the interesting ones; per-image jobs are too chatty.
        if !matches!(
            payload,
            CatalogPayload::Levelshot { .. }
                | CatalogPayload::ProfileIcon { .. }
                | CatalogPayload::CrosshairImage { .. }
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
