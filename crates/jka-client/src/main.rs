macro_rules! println {
    () => { crate::logging::write_line(crate::logging::Level::Info, format_args!("")) };
    ($($arg:tt)*) => { crate::logging::write_line(crate::logging::Level::Info, format_args!($($arg)*)) };
}

macro_rules! eprintln {
    () => { crate::logging::write_line(crate::logging::Level::Error, format_args!("")) };
    ($($arg:tt)*) => { crate::logging::write_line(crate::logging::Level::Error, format_args!($($arg)*)) };
}

mod app;
mod audio;
mod camera;
mod cgame;
mod clipboard;
mod cloud_noise;
mod color_lut;
mod config;
mod console;
mod download;
mod fx;
mod grass;
mod keybinds;
mod lightmap_atlas;
mod logging;
mod local_server;
mod map_jobs;
mod materials;
mod net;
mod ocean;
mod player;
mod pure;
mod renderer;
mod runtime;
mod scene;
mod server_browser;
mod surface_deformation;
mod steam_audio;
mod thread_activity;
mod ui;
mod vgs;
mod weather;

use runtime::UserEvent;
use std::{
    io::{BufRead, IsTerminal},
    path::PathBuf,
};
use winit::event_loop::{EventLoop, EventLoopProxy};

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
    source: scene::MapSource,
    launch_map: bool,
    game: Option<String>,
    validate: bool,
    /// OpenJK-style `+command args` startup lines (e.g. `+connect host`).
    startup_commands: Vec<String>,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let directory = executable.parent().ok_or("Cannot locate executable")?;
        let adjacent = directory.join("base");
        let development = directory.parent().unwrap_or(directory).join("base");
        let mut options = Self {
            base: if adjacent.is_dir() {
                adjacent
            } else if development.is_dir() {
                development
            } else {
                adjacent
            },
            source: scene::MapSource::Bsp("mp/ffa3".into()),
            launch_map: false,
            game: None,
            validate: false,
            startup_commands: Vec::new(),
        };
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--base" => options.base = args.next().ok_or("--base needs a folder")?.into(),
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
                command if command.starts_with('+') && command.len() > 1 => {
                    options.startup_commands.push(command[1..].to_owned());
                }
                word if !word.starts_with("--") && !options.startup_commands.is_empty() => {
                    let line = options.startup_commands.last_mut().expect("checked non-empty");
                    line.push(' ');
                    line.push_str(word);
                }
                _ => {
                    return Err(
                        "Usage: DinurdoJK [--base path/to/base] [--game modname] [--map mp/ffa3[.bsp|.map] | --map-file path/to/map.map] [--validate-map] [+command args ...]"
                            .into(),
                    )
                }
            }
        }
        Ok(options)
    }
}

fn main() -> std::process::ExitCode {
    match logging::init() {
        Ok(path) => println!("Latest log: {}", path.display()),
        Err(error) => std::eprintln!("Logging initialization failed: {error}"),
    }

    let options = match Options::parse() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::FAILURE;
        }
    };

    println!("DinurdoJK renderer: winit + wgpu, no Bevy");
    println!("Base directory: {}", options.base.display());
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
        println!("Active game directory: {}", game.display());
    }
    println!("Threads: main + dedicated render + background map loader + map worker pool");

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
                println!(
                    "{}: {} triangles, {} draw batches, {} textures, {} lightmap pages; source {}",
                    label,
                    map.triangles,
                    map.batches.len(),
                    map.textures.len(),
                    map.lightmap_pages,
                    map.source.display()
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

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(loop_) => loop_,
        Err(error) => {
            eprintln!("Event loop creation failed: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let proxy = event_loop.create_proxy();
    let terminal_proxy = proxy.clone();
    let startup_commands = options.startup_commands;
    let mut application = match app::App::new(
        options.base,
        game_dir,
        options.source,
        options.launch_map,
        proxy,
    ) {
        Ok(mut application) => {
            application.queue_startup_commands(startup_commands);
            application
        }
        Err(error) => {
            eprintln!("Application initialization failed: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    spawn_terminal_command_reader(terminal_proxy);

    match event_loop.run_app(&mut application) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Event loop failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
