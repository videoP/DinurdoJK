macro_rules! println {
    () => { crate::logging::write_line(crate::logging::Level::Info, format_args!("")) };
    ($($arg:tt)*) => { crate::logging::write_line(crate::logging::Level::Info, format_args!($($arg)*)) };
}

macro_rules! eprintln {
    () => { crate::logging::write_line(crate::logging::Level::Error, format_args!("")) };
    ($($arg:tt)*) => { crate::logging::write_line(crate::logging::Level::Error, format_args!($($arg)*)) };
}

/// Global diagnostic output. `developer 0` is quiet, 1 is basic lifecycle
/// information, 2 is verbose per-entity/per-job diagnostics, and 3 is trace-level.
macro_rules! devprintln {
    ($level:expr, $($arg:tt)*) => {{
        if crate::logging::developer_enabled($level) {
            println!($($arg)*);
        }
    }};
}

/// Renderer-specific verbose output. `r_verbose` can enable this without also
/// enabling cgame/client diagnostics; a matching `developer` level also enables it.
macro_rules! rverbose {
    ($level:expr, $($arg:tt)*) => {{
        if crate::logging::renderer_verbose_enabled($level) {
            println!($($arg)*);
        }
    }};
}

mod app;
mod asset_jobs;
use jka_audio::{audio, eal, steam_audio};
mod emotes;
use jka_render_core::camera;
use jka_client_services::{
    chat_translation, clipboard, credential_store, display_gamma, filesystem_paths, frontend_cache_color,
    jk2_diagnostics, neon_menu, protocol_link, pure, screenshot, server_browser, startup_timeline, vgs, vote,
};
mod cgame;
mod global_cosmetics;
mod profile_wire;
mod chat_log;
mod cloud_noise;
mod cloud_wind;
mod color_grading;
mod gamma;
mod color_lut;
mod config;
#[cfg(test)]
mod cvar_compatibility;
mod console;
mod crash;
mod dependency_dll;
mod download;
mod updater;
mod entity_graph;
mod extensions;
mod fx;
mod grass;
mod foliage;
mod items;
mod japro_cg;
mod jump_shade;
mod keybinds;
mod lagometer;
mod lightmap_atlas;
mod logging;
mod local_server;
mod map_jobs;
mod materials;
mod media_mp4;
mod model_frame_log;
mod net;
mod palette;
mod emote_net;
mod emote_net_codec;
mod ocean;
use jka_river::{river_authoring, river_runtime, river_settings, river_slw, river_surface};
// The remaining river modules are reached directly only by the GPU port tests.
#[cfg(test)]
use jka_river::{
    river_capture, river_composite, river_discharge, river_foam, river_gpu, river_grid_mesh, river_map,
    river_material, river_ocean, river_water_info, river_water_info_capture,
};
#[cfg(test)]
mod river_port_tests;
mod blue_water_environment;
mod player;
mod pipeline_cache;
mod pipeline_jobs;
mod renderer;
mod runtime;
mod scene;
mod streamer;
mod speedometer;
mod surface_deformation;
mod strafehelper;
mod strafe_trail;
mod thread_activity;
mod startup_console;
mod startup_error;
mod ui;
mod weather;
mod glass;
mod water_wetness;
mod windows_timer;
mod world_effects;

use runtime::UserEvent;
use std::{
    io::{BufRead, IsTerminal},
    path::PathBuf,
};
use winit::event_loop::{EventLoop, EventLoopProxy};

/// Work around the legacy AMD switchable-graphics implicit Vulkan layer before
/// wgpu loads the Vulkan loader. Old versions of that layer can return
/// `VK_INCOMPLETE` from `vkEnumeratePhysicalDevices` with an inconsistent count,
/// which wgpu/ash correctly surfaces as "A return array was too small for the
/// result" and then has no adapters to choose from. Native renderers that use a
/// different enumeration path can appear to work on the same machine.
///
/// `DISABLE_LAYER_AMD_SWITCHABLE_GRAPHICS_1` is the layer's own compatibility
/// escape hatch. Respect an explicit user value; otherwise set it once, before
/// DinurdoJK starts any worker/render threads. It only affects Vulkan loader
/// behavior, so selecting DX12 remains unchanged.
#[cfg(windows)]
fn apply_windows_vulkan_loader_compat() -> bool {
    const DISABLE_AMD_SWITCHABLE_LAYER: &str = "DISABLE_LAYER_AMD_SWITCHABLE_GRAPHICS_1";
    if std::env::var_os(DISABLE_AMD_SWITCHABLE_LAYER).is_some() {
        return false;
    }

    // SAFETY: this is called at process startup before DinurdoJK spawns any
    // threads. Environment mutation is unsafe in Rust 2024 only because racing
    // concurrent environment access is unsound.
    unsafe {
        std::env::set_var(DISABLE_AMD_SWITCHABLE_LAYER, "1");
    }
    true
}

#[cfg(not(windows))]
fn apply_windows_vulkan_loader_compat() -> bool {
    false
}

/// `App` is ~145 KB. Held by value, `main` kept three App-sized slots (the
/// `Result`, the match binding and the local), ~440 KB of the 1 MB main-thread
/// stack, live for the whole event loop. Synchronous CGame/session builds
/// inside window events then overflowed it (JK2 demo launch on -Faster).
/// Never inlined, so the by-value copies are released before `run_app`.
#[inline(never)]
fn new_boxed_app(
    base: PathBuf,
    game: Option<PathBuf>,
    source: scene::MapSource,
    launch_map: bool,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<Box<app::App>, String> {
    app::App::new(base, game, source, launch_map, proxy).map(Box::new)
}

fn spawn_terminal_command_reader(proxy: EventLoopProxy<UserEvent>) {
    if !std::io::stdin().is_terminal() {
        return;
    }

    let result = std::thread::Builder::new()
        .name("console-stdin".into())
        .spawn(move || {
            println!("Terminal command input enabled; type a console command and press Enter.");
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        eprintln!("Terminal input stopped: {error}");
                        break;
                    }
                };
                let command = line.trim().to_owned();
                if command.is_empty() {
                    continue;
                }
                if proxy
                    .send_event(UserEvent::ConsoleCommand(command))
                    .is_err()
                {
                    break;
                }
            }
        });
    if let Err(error) = result {
        eprintln!("Could not start terminal command input: {error}");
    }
}

struct Options {
    base: PathBuf,
    base_overridden: bool,
    source: scene::MapSource,
    launch_map: bool,
    game: Option<String>,
    validate: bool,
    /// OpenJK-style `+command args` startup lines (e.g. `+set fs_game japro`).
    startup_commands: Vec<String>,
    /// Image supplied as a positional argument, e.g. by dragging a screenshot
    /// onto DinurdoJK.exe in Explorer.
    screenshot_path: Option<PathBuf>,
    screenshot_metadata: Option<screenshot::ScreenshotMetadata>,
    /// `.dm_26` supplied as a positional argument, e.g. by dragging a demo
    /// onto DinurdoJK.exe in Explorer.
    demo_path: Option<PathBuf>,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut options = Self {
            base: filesystem_paths::default_base_directory()?,
            base_overridden: false,
            source: scene::MapSource::Bsp("mp/ffa3".into()),
            launch_map: false,
            game: None,
            validate: false,
            startup_commands: Vec::new(),
            screenshot_path: None,
            screenshot_metadata: None,
            demo_path: None,
        };
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--base" => {
                    options.base = args.next().ok_or("--base needs a folder")?.into();
                    options.base_overridden = true;
                },
                "--map" | "+devmap" => {
                    let map = args.next().ok_or("map name required")?;
                    options.source = scene::MapSource::from_map_argument(&map)?;
                    options.launch_map = true;
                }
                "--map-file" => {
                    options.source = scene::MapSource::MapFile(args.next().ok_or("--map-file needs a path")?.into());
                    options.launch_map = true;
                }
                "--game" => options.game = Some(args.next().ok_or("--game needs a directory name")?),
                "--validate-map" => options.validate = true,
                // Browsers deliver the entire URL as one positional argument.
                // Parse it before the generic OpenJK +command argument collector.
                url if url.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("jka:")) => {
                    let target = protocol_link::parse_jka_url(url)?;
                    options.startup_commands.push(format!("connect {target}"));
                }
                "--console" => {}, // Kept as a harmless compatibility flag; native console stays enabled.
                command if command.starts_with('+') && command.len() > 1 => {
                    options.startup_commands.push(command[1..].to_owned());
                }
                image if !image.starts_with('-')
                    && options.startup_commands.is_empty()
                    && is_screenshot_path(std::path::Path::new(image)) =>
                {
                    if options.screenshot_path.is_some() {
                        return Err("only one startup screenshot can be opened at a time".into());
                    }
                    let path = PathBuf::from(image);
                    options.screenshot_metadata = match screenshot::read_metadata_file(&path) {
                        Ok(metadata) => metadata,
                        Err(error) => {
                            eprintln!("Screenshot metadata: {error}");
                            None
                        }
                    };
                    options.screenshot_path = Some(path);
                }
                demo if !demo.starts_with('-')
                    && options.startup_commands.is_empty()
                    && is_demo_path(std::path::Path::new(demo)) =>
                {
                    if options.demo_path.is_some() {
                        return Err("only one startup demo can be opened at a time".into());
                    }
                    options.demo_path = Some(PathBuf::from(demo));
                }
                word if !word.starts_with("--") && !options.startup_commands.is_empty() => {
                    let line = options.startup_commands.last_mut().expect("checked non-empty");
                    line.push(' ');
                    line.push_str(word);
                }
                _ => {
                    return Err(
                        "Usage: DinurdoJK [--base path/to/base] [--game modname] [--map mp/ffa3[.bsp|.map] | --map-file path/to/map.map] [--validate-map] [--console] [jka://host[:port]] [screenshot.jpg] [demo.dm_26] [+command args ...] (Windows: --register-jka-protocol / --unregister-jka-protocol)"
                            .into(),
                    )
                }
            }
        }
        if let Some(metadata) = options
            .screenshot_metadata
            .clone()
            .filter(|metadata| metadata.can_go_to_spot())
        {
            if let Some(map_name) = metadata.map_name.as_deref() {
                options.source = scene::MapSource::Bsp(map_name.to_owned());
                options.launch_map = true;
                options.game = if metadata.game.trim().is_empty()
                    || metadata.game.eq_ignore_ascii_case("base")
                {
                    None
                } else {
                    Some(metadata.game.clone())
                };
            }
        }
        Ok(options)
    }
}

fn is_screenshot_path(path: &std::path::Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(extension.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png")
            })
}

fn is_demo_path(path: &std::path::Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "dm_26" | "dm_15" | "dm_16"))
}

fn main() -> std::process::ExitCode {
    // Crates split out of the client print through the client logger, exactly
    // as their println!/eprintln! did in-crate; install before anything runs.
    fn write_split_crate_line(error: bool, args: std::fmt::Arguments<'_>) {
        logging::write_line(if error { logging::Level::Error } else { logging::Level::Info }, args)
    }
    jka_render_core::log::install(write_split_crate_line);
    jka_audio::host::install(jka_audio::host::Host {
        write_line: write_split_crate_line,
        developer_enabled: logging::developer_enabled,
        read_emote_asset: emotes::read_asset,
        favorite_emote_asset: emotes::favorite_asset,
    });
    crate::startup_timeline::start();
    if let Some(code) = jk2_diagnostics::cli_entry() {
        return code;
    }
    if let Some(code) = protocol_link::registration_cli_entry() {
        return std::process::ExitCode::from(code as u8);
    }
    if let Some(code) = updater::helper_entry() {
        return std::process::ExitCode::from(code as u8);
    }
    // Other private helpers (notably the gamma guardian) must be routed
    // BEFORE checking pending updates, or those child processes would attempt
    // to launch the game updater too.
    if let Some(code) = display_gamma::helper_entry() {
        return std::process::ExitCode::from(code as u8);
    }
    match updater::apply_pending_on_launch() {
        Ok(true) => return std::process::ExitCode::SUCCESS,
        Ok(false) => {},
        Err(e) => std::eprintln!("Pending update could not start: {e}"),
    }

    // DinurdoJK/ (with its dependencies) must sit beside the executable. Check
    // before logging creates anything there, and say so in a dialog: this
    // process has no visible console yet, so a bare exit looks like a crash.
    match filesystem_paths::installation_directory() {
        Ok(install) => {
            if let Some(problem) = startup_error::verify_client_folder(&install) {
                return startup_error::fatal(problem);
            }
        }
        Err(error) => return startup_error::fatal(format!("Cannot locate the executable folder: {error}")),
    }

    // phonon.dll is delay-loaded on MSVC. Load our bundled copy by its
    // absolute path before any Steam Audio FFI; never rely on the process PATH.
    #[cfg(windows)]
    if let Err(error) = dependency_dll::preload("phonon.dll") {
        return startup_error::fatal(format!("Steam Audio dependency: {error}"));
    }

    crate::startup_console::prepare_own_console();
    let _console_reveal_on_exit = crate::startup_console::RevealOnExit;

    let disabled_legacy_amd_vulkan_layer = apply_windows_vulkan_loader_compat();
    match logging::init() {
        Ok(path) => {
            logging::write_line_with_path(
                logging::Level::Info,
                format_args!("Latest log: {}", path.display()),
                path.clone(),
            );
            crash::install(&path);
            crash::log_environment();
        }
        Err(error) => std::eprintln!("Logging initialization failed: {error}"),
    }

    crate::startup_timeline::mark(crate::startup_timeline::Milestone::LoggingReady);

    if disabled_legacy_amd_vulkan_layer {
        println!(
            "[GPU] Vulkan compatibility: disabled legacy AMD switchable-graphics implicit layer for this process"
        );
    }

    for error in display_gamma::recover_before_launch() {
        eprintln!("Display gamma recovery: {error}");
    }

    crate::startup_timeline::mark(crate::startup_timeline::Milestone::GammaReady);

    let mut options = match Options::parse() {
        Ok(options) => options,
        Err(error) => return startup_error::fatal(error),
    };

    // Install game roots before resolving the initial fs_game or prewarming assets.
    // --base remains an explicit JKA base-directory override; otherwise
    // fs_basepath selects the installation root containing base/.
    let mut fs_roots = filesystem_paths::load(&options.base);
    if let Err(error) = filesystem_paths::apply_startup_commands(
        &options.base, &mut fs_roots, &options.startup_commands
    ) {
        return startup_error::fatal(format!("Invalid filesystem startup setting: {error}"));
    }
    if !options.base_overridden {
        if let Some(root) = &fs_roots.basepath { options.base = root.join("base"); }
    }
    let install = match filesystem_paths::installation_directory() {
        Ok(root) => root,
        Err(error) => return startup_error::fatal(format!("Cannot locate the executable folder: {error}")),
    };
    fs_roots.client_install = Some(install.clone());
    // Keep JKA, JK2 and DinurdoJK/ from overlapping. A bad saved/typed root is
    // not fatal: fall back to the defaults (JKA = the executable directory) and
    // let Asset Mapping show what was rejected so it can be fixed in place.
    let validate = |options: &Options, roots: &jka_assets::pk3::FilesystemRoots| {
        filesystem_paths::validate_root_layout(
            &options.base, roots, &install)
    };
    if let Err(error) = validate(&options, &fs_roots) {
        eprintln!("[FILESYSTEM] {error}; using default roots");
        let configured = fs_roots.clone();
        fs_roots.basepath = None;
        fs_roots.jk2basepath = None;
        fs_roots.homepath = None;
        fs_roots.jk2homepath = None;
        if !options.base_overridden {
            options.base = match filesystem_paths::default_base_directory() {
                Ok(base) => base,
                Err(error) => return startup_error::fatal(error),
            };
        }
        if let Err(still) = validate(&options, &fs_roots) {
            // Only a --base override can get here: the defaults are valid.
            return startup_error::fatal(format!("{still}\n\n(from the --base launch argument)"));
        }
        app::asset_mapping::set_startup_notice(error, configured);
    }

    crate::startup_timeline::mark(crate::startup_timeline::Milestone::OptionsReady);

    println!("DinurdoJK renderer: winit + wgpu");
    logging::write_line_with_path(
        logging::Level::Info,
        format_args!("Base directory: {}", options.base.display()),
        options.base.clone(),
    );
    let game_dir = options.game.as_ref().map(|game| {
        let candidate = PathBuf::from(game);
        if candidate.is_absolute() {
            candidate
        } else {
            options
                .base
                .parent()
                .unwrap_or(&options.base)
                .join(candidate)
        }
    });
    if let Some(game) = &game_dir {
        logging::write_line_with_path(
            logging::Level::Info,
            format_args!("Active game directory: {}", game.display()),
            game.clone(),
        );
    }
    println!("Threads: main + dedicated render + background map loader + map worker pool");

    // The grass noise images are CPU-generated and deterministic; build them while
    // the window and GPU device are being created.
    grass::prewarm_noise();

    // Configure once for renderer/map workers. No legacy folder migration.
    jka_assets::pk3::configure_filesystem_roots(&options.base, fs_roots.clone());

    // A missing retail installation is not fatal (2D menu and tools still work).
    // Explain the fix once on startup instead of flooding the console with
    // secondary missing-map/texture errors. fs_cdpath points to GameData, NOT base/.
    if let Some(cdpath) = &fs_roots.cdpath {
        if !cdpath.join("base").is_dir() {
            eprintln!(
                "[FILESYSTEM] fs_cdpath={} has no base/ directory; expected <fs_cdpath>/base/assets0.pk3. Restart after correcting the cvar.",
                cdpath.display()
            );
        }
    }
    if !app::asset_mapping::jka_assets_present(&options.base, &fs_roots) {
        eprintln!(
            "[FILESYSTEM] Missing one or more JKA retail archives (assets0.pk3..assets3.pk3) in JKA base layers. Opening Asset Mapping; configure fs_basepath or fs_cdpath, then Save & Restart."
        );
    }

    // Index the asset packages now, in parallel with window, adapter and device
    // creation. The index is cached process-wide (jka_assets::pk3), so the
    // renderer's UI loads, the first map prepare and the presenters that open
    // the same directories reuse it, or wait for it, instead of each re-reading
    // every PK3 central directory.
    {
        let base = options.base.clone();
        let game = game_dir.clone();
        let _ = std::thread::Builder::new()
            .name("asset-index-prewarm".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                match jka_assets::pk3::AssetSearchPath::open_game(&base, game.as_deref()) {
                    Ok(assets) => println!(
                        "[ASSET INDEX] prewarm: {} qpaths in {:.1} ms",
                        assets.names().count(),
                        started.elapsed().as_secs_f64() * 1000.0
                    ),
                    Err(error) => eprintln!("[ASSET INDEX] prewarm failed: {error}"),
                }
            });
    }

    if options.validate {
        let label = options.source.label();
        println!("Validating {label}...");
        // JKA_AUDIT_LOW=1 mirrors r_reflectionQuality low + r_pbr 0 so the AUTO 4
        // audit measures the same batch topology the client builds in game.
        let prepared = match (&options.source, std::env::var_os("JKA_AUDIT_LOW")) {
            (scene::MapSource::Bsp(name), Some(_)) => scene::prepare_with_options(
                &options.base,
                game_dir.as_deref(),
                name,
                scene::MapPrepareOptions {
                    planar_reflections: false,
                    pbr_materials: false,
                    ..Default::default()
                },
            ),
            _ => scene::prepare_source(&options.base, game_dir.as_deref(), &options.source),
        };
        return match prepared {
            Ok(map) => {
                logging::write_line_with_path(
                    logging::Level::Info,
                    format_args!(
                        "{}: {} triangles, {} draw batches, {} textures, {} lightmap pages; source {}",
                        label,
                        map.triangles,
                        map.batches.len(),
                        map.textures.len(),
                        map.lightmap_pages,
                        map.source.display()
                    ),
                    map.source.clone(),
                );
                if let Some(stats) = map.map_file_stats {
                    println!(
                        "Source .map: {} entities, {} brushes parsed ({} worldspawn), {} rendered faces, {} non-worldspawn brushes skipped, {} patches skipped, {} degenerate faces, {} invalid brushes skipped",
                        stats.entities,
                        stats.brushes,
                        stats.world_brushes,
                        stats.rendered_faces,
                        stats.skipped_entity_brushes,
                        stats.patches_skipped,
                        stats.degenerate_faces,
                        stats.skipped_brushes,
                    );
                    println!(
                        "Source .map runtime prep: {} utility faces dropped, {} spatial chunks, {} geometry groups -> {} WGPU draw batches, spatial batching={}; {} gameplay collision brushes; brush reconstruction {:.2} ms on {} map worker(s)",
                        stats.utility_faces_skipped,
                        stats.spatial_chunks,
                        stats.geometry_groups,
                        stats.draw_batches,
                        stats.spatial_batching,
                        stats.collision_brushes,
                        stats.reconstruction_ms,
                        stats.worker_count,
                    );
                }
                for line in scene::auto4_audit_lines(&map) {
                    println!("{line}");
                }
                // Ocean promotion audit: which authored faces become the FFT
                // water plane. Only an upward-facing horizontal face qualifies;
                // a water brush also contributes its underside and its sides.
                let mut promoted = 0usize;
                for batch in map.batches.iter().chain(map.pvs_batches.iter()) {
                    if !batch.water_primary {
                        continue;
                    }
                    let slice = &map.vertices[batch.vertices.start as usize..batch.vertices.end as usize];
                    let mut lo = [f32::INFINITY; 3];
                    let mut hi = [f32::NEG_INFINITY; 3];
                    let mut facing = 0.0f32;
                    for vertex in slice {
                        for axis in 0..3 {
                            lo[axis] = lo[axis].min(vertex.position[axis]);
                            hi[axis] = hi[axis].max(vertex.position[axis]);
                        }
                        facing += vertex.normal[1];
                    }
                    let upward = facing > 0.0 && lo[1] >= hi[1] - 1.0;
                    if upward {
                        promoted += 1;
                    }
                    println!(
                        "  water face: {} tris | {} | plane y {:.0} | footprint x {:.0}..{:.0} z {:.0}..{:.0}",
                        slice.len() / 3,
                        if upward { "PROMOTED to FFT ocean" } else { "skipped (not an upward horizontal face)" },
                        hi[1], lo[0], hi[0], lo[2], hi[2],
                    );
                }
                println!("  FFT ocean surfaces: {promoted}");
                for warning in &map.warnings {
                    if map.map_file_stats.is_some() {
                        println!("Warning: {warning}");
                    } else {
                        println!("Material: {warning}");
                    }
                }
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("Map validation failed: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }

    let mut event_loop_builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        event_loop_builder.with_msg_hook(|message| {
            renderer::input_diag_win32_message(message);
            // False means continue Winit's normal message dispatch. The hook is
            // observation-only and must never consume WM_INPUT.
            false
        });
    }
    let event_loop = match event_loop_builder.build() {
        Ok(loop_) => loop_,
        Err(error) => {
            return startup_error::fatal(format!("Event loop creation failed: {error}"));
        }
    };
    crate::startup_timeline::mark(crate::startup_timeline::Milestone::EventLoopReady);
    let proxy = event_loop.create_proxy();
    let terminal_proxy = proxy.clone();
    let startup_commands = options.startup_commands;
    let startup_screenshot_path = options.screenshot_path.clone();
    let startup_screenshot_metadata = options.screenshot_metadata.clone();
    let startup_demo_path = options.demo_path.clone();
    crate::startup_timeline::mark(crate::startup_timeline::Milestone::AppBegin);
    let mut application = match new_boxed_app(
        options.base,
        game_dir,
        options.source,
        options.launch_map,
        proxy,
    ) {
        Ok(mut application) => {
            if let Some(path) = startup_screenshot_path {
                application.configure_startup_screenshot(path, startup_screenshot_metadata);
            }
            if let Some(path) = startup_demo_path {
                application.configure_startup_demo(path);
            }
            application.queue_startup_commands(startup_commands);
            application
        }
        Err(error) => {
            return startup_error::fatal(format!("Application initialization failed: {error}"));
        }
    };
    crate::startup_timeline::mark(crate::startup_timeline::Milestone::AppReady);
    spawn_terminal_command_reader(terminal_proxy);

    match event_loop.run_app(&mut *application) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Event loop failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
