//! Declarative command handlers.
use crate::app::{demo_recording_timestamp, App, GameSession, RenderCommand, SessionPhase};

use super::{CommandDef, CommandResult, CommandScope};
commands! {
    cmd_memory_info ["memory_info"] ("memory_info", "Report retained map CPU pixels and GPU live/reserved allocations.", CommandScope::Always) |self, command| {
        if command.split_whitespace().count() != 1 {
            self.push_console_line("usage: memory_info".to_owned());
            return CommandResult::Handled;
        }
        if let Some(cache) = self.prepared_map_cache.as_ref() {
            let label = cache.label.clone();
            let lines = cache.map.memory_report_lines();
            self.push_console_line(format!("MEMORY prepared map: {label}"));
            for line in lines { self.push_console_line(line); }
        } else {
            self.push_console_line("MEMORY CPU: no prepared map cache".to_owned());
        }
        if self.render.is_some() {
            self.render_command(RenderCommand::MemoryInfo);
        } else {
            self.push_console_line("MEMORY GPU: renderer unavailable".to_owned());
        }
        CommandResult::Handled
    }
    cmd_baked_sun_debug ["baked_sun_debug"] ("baked_sun_debug <0|1|2>", "Baked sunlight diagnostic: 0 normal, 1 removed amount, 2 sun visibility.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let mode = match words.as_slice() {
            [_, "0"] => 0,
            [_, "1"] => 1,
            [_, "2"] => 2,
            _ => {
                self.push_console_line("usage: baked_sun_debug <0|1|2>".to_owned());
                return CommandResult::Handled;
            }
        };
        self.baked_sun_debug_view = mode;
        self.sun_ray_until = None;
        self.sync_sun_ray();
        CommandResult::Handled
    }
    cmd_bluewater_coast_debug ["bluewater_coast_debug"] ("bluewater_coast_debug", "Inspect upstream coastal depth, fetch and wave attenuation at the view position.", CommandScope::Always) |self, command| {
        if command.split_whitespace().count() != 1 {
            self.push_console_line("usage: bluewater_coast_debug".to_owned());
            return CommandResult::Handled;
        }
        let at = [self.camera.position.x as f64 / crate::ocean::OCEAN_UNITS_PER_METER as f64,
                  self.camera.position.z as f64 / crate::ocean::OCEAN_UNITS_PER_METER as f64];
        let lines = self.prepared_map_cache.as_ref()
            .and_then(|cache| cache.map.coastal_bathymetry.as_ref())
            .map(|job| job.upstream_shore.report(at, &self.video.ocean_settings))
            .unwrap_or_else(|| vec!["No upstream shore source: load a BSP with water promotion enabled.".into()]);
        for line in lines { self.push_console_line(line); }
        CommandResult::Handled
    }
    cmd_gpu_info ["gpu_info"] ("gpu_info", "Show the active graphics adapter and staged GPU preference.", CommandScope::Always) |self, line| {
        if line.split_whitespace().count() != 1 { self.push_console_line("usage: gpu_info".to_owned()); return CommandResult::Handled; }
        self.push_console_line(format!("GPU ACTIVE: {} ({}) | backend {}", self.active_gpu_name,
            self.active_gpu_type, self.applied_renderer_backend.label()));
        self.push_console_line(format!("GPU PREFERENCE: applied {} / selected {}", crate::renderer::gpu_adapter::label(self.applied_gpu_adapter),
            crate::renderer::gpu_adapter::label(self.video.gpu_adapter)));
        CommandResult::Handled
    }
    cmd_gpu_list ["gpu_list"] ("gpu_list", "Enumerate graphics adapters available on the current DX12/Vulkan backend.", CommandScope::Always) |self, line| {
        if line.split_whitespace().count() != 1 { self.push_console_line("usage: gpu_list".to_owned()); return CommandResult::Handled; }
        let backend = if self.video.renderer_backend == crate::ui::RendererBackend::Dx12 { wgpu::Backends::DX12 } else { wgpu::Backends::VULKAN };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: backend, ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapters = futures_lite::future::block_on(crate::renderer::gpu_adapter::enumerate(&instance));
        self.push_console_line(format!("GPU ADAPTERS: {} found ({})", adapters.len(), self.video.renderer_backend.label()));
        for (index, adapter) in adapters.iter().enumerate() {
            let info = adapter.get_info();
            self.push_console_line(format!("  {}. {} | {:?} | {:?} | driver {} {}", index, info.name,
                info.device_type, info.backend, info.driver, info.driver_info));
        }
        self.push_console_line("Use r_gpuAdapter 0/1/2/3, then vid_restart. 3 is available only if listed as Cpu.".to_owned());
        CommandResult::Handled
    }

    cmd_coastal_debug ["coastal_debug", ] ("coastal_debug", "Print background BSP bathymetry results and the interactive debug report path.", CommandScope::Always) |self, command| {
        if command.split_whitespace().count() != 1 {
            self.push_console_line("usage: coastal_debug".to_owned());
            return CommandResult::Handled;
        }
        let lines = self.prepared_map_cache.as_ref()
            .and_then(|cache| cache.map.coastal_bathymetry.as_ref())
            .map(|job| job.report_lines())
            .unwrap_or_else(|| vec!["No coastal geometry job for this map (enable ocean and load a BSP with promoted water).".to_owned()]);
        for line in lines { self.push_console_line(line); }
        CommandResult::Handled
    }

    cmd_configstrings ["configstrings", ] ("configstrings", "Print the active client configstrings list.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                // TaystJK CL_Configstrings_f: only an active CGame owns the
                // printable gameState table, and empty configstrings are skipped.
                let active = self
                    .game_session
                    .as_ref()
                    .filter(|session| session.phase == SessionPhase::Playing)
                    .map(|session| {
                        session
                            .client_game
                            .configstrings()
                            .iter()
                            .filter(|(_, value)| !value.is_empty())
                            .map(|(&index, value)| {
                                (index, String::from_utf8_lossy(value).into_owned())
                            })
                            .collect::<Vec<_>>()
                    });
                let Some(strings) = active else {
                    self.push_console_line("Not connected to a server.".to_owned());
                    return CommandResult::Handled;
                };
                for (index, value) in strings {
                    self.push_console_line(format!("{index:4}: {value}"));
                }
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 configstrings".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_version ["version", ] ("version", "Print the DinurdoJK version and UTC compile time embedded in this executable.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.push_console_line(format!(
                    "^2DinurdoJK^7 {}  ^8compiled^7 {}",
                    env!("CARGO_PKG_VERSION"),
                    env!("DINURDOJK_BUILD_UTC")
                ));
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 version".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_predsettings ["predsettings", ] ("predsettings", "Print the pmove settings the predictor is running with: race/style, pmove_float/fixed/msec, velocity snapping, command chopping.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                let command_rate = format!(
                    "{} ({:.2} ms usercmd gaps)",
                    self.network.command_rate,
                    1000.0 / f64::from(self.network.command_rate.max(15))
                );
                for line in self.predictor.regime_report(&command_rate) {
                    self.push_console_line(line);
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_hybridwave ["hybridwave", ] ("hybridwave", "Write the Hybrid ocean wave-classification log (last ~600 frames + settings) to <game>/hybridwave/*.txt and queue a PNG screenshot at the same moment. Send both files.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                let path = self.game_write_path(std::path::PathBuf::from("hybridwave")
                    .join(format!("hybridwave-{}.txt", demo_recording_timestamp())));
                self.push_console_line(format!("hybridwave: {}", path.display()));
                self.render_command(RenderCommand::HybridWaveDump { path });
                self.request_screenshot_png();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_hitchmark ["hitchmark", ] ("hitchmark [label]", "Write the last ~5 s of per-frame prediction/view state to <game>/hitch/*.csv. Bind it and tap it right after a hitch (needs cg_hitchrecord 1).", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.hitch_mark(None);
                CommandResult::Handled
            }
            [_verb, label] => {
                self.hitch_mark(Some(label));
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 hitchmark [label]".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_sky_perf_sweep ["sky_perf_sweep", "skyperfsweep"] ("sky_perf_sweep [frames_per_case=300]", "Automated six-phase Unified sky GPU benchmark: normal, solid fill, constant sample, one-face varying UV, fastsky, normal repeat. Restores sky, stage, timestamp settings; writes one CSV and JSON. Stand still and close the console.", CommandScope::Always) |self, command| {
        let args: Vec<_> = command.split_whitespace().collect();
        if args.len() > 2 {
            self.push_console_line("^3usage:^7 sky_perf_sweep [frames_per_case=300]".to_owned());
            return CommandResult::Handled;
        }
        let frames = match args.get(1).map(|s| s.parse::<usize>()).transpose() {
            Ok(Some(n)) if (60..=1000).contains(&n) => n,
            Ok(None) => 300,
            _ => {
                self.push_console_line("^1sky_perf_sweep:^7 frames per case must be 60..1000".to_owned());
                return CommandResult::Handled;
            }
        };
        let stem = self.game_write_path(std::path::PathBuf::from("renderperf")
            .join(format!("sky-sweep-{}", demo_recording_timestamp())));
        let result = self.render.as_ref().ok_or("renderer is unavailable")
            .and_then(|render| render.start_sky_perf_sweep(stem.clone(), frames));
        match result {
            Ok(()) => self.push_console_line(format!(
                "^2SKY SWEEP ARMED^7 Six cases × {frames} frames. Hold camera still, close console. Automatically restores settings. Output: {}.csv/.json", stem.display()
            )),
            Err(error) => self.push_console_line(format!("^1sky_perf_sweep:^7 {error}")),
        }
        CommandResult::Handled
    }

    cmd_render_perf_capture ["render_perf_capture", "renderperfcapture"] ("render_perf_capture [frames=600|<seconds>s] [label]", "Record per-frame renderer CPU stage timings to CSV/JSON after a 2s delay and 120 warmup frames. A count ending in s (e.g. 10s) records for that long instead. Opt-in; no GPU stalls.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if words.len() > 3 {
            self.push_console_line("^3usage:^7 render_perf_capture [frames=600|<seconds>s] [label]".to_owned());
            return CommandResult::Handled;
        }
        let mut seconds = None;
        let frames = if let Some(arg) = words.get(1) {
            if let Some(value) = arg.strip_suffix('s') {
                match value.parse::<f64>() {
                    Ok(value) if (1.0..=120.0).contains(&value) => seconds = Some(value),
                    _ => {
                        self.push_console_line("^1render_perf_capture:^7 seconds must be 1..120".to_owned());
                        return CommandResult::Handled;
                    }
                }
                0
            } else {
                match arg.parse::<usize>() {
                    Ok(value) if (30..=6000).contains(&value) => value,
                    _ => {
                        self.push_console_line("^1render_perf_capture:^7 frames must be 30..6000".to_owned());
                        return CommandResult::Handled;
                    }
                }
            }
        } else { 600 };
        let label: String = words.get(2).copied().unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(32)
            .collect();
        let stem_name = if label.is_empty() {
            format!("renderperf-{}", demo_recording_timestamp())
        } else {
            format!("renderperf-{}-{label}", demo_recording_timestamp())
        };
        let stem = self.game_write_path(std::path::PathBuf::from("renderperf").join(stem_name));
        let result = self.render.as_ref().ok_or("renderer is unavailable")
            .and_then(|render| render.start_render_perf_capture(stem.clone(), frames, seconds));
        let span = seconds.map_or_else(|| format!("{frames} samples"), |seconds| format!("{seconds}s"));
        match result {
            Ok(()) => self.push_console_line(format!(
                "^2RENDER PERF ARMED^7 {span} after 2s + 120-frame warmup. Close console; output: {}.csv/.json",
                stem.display()
            )),
            Err(error) => self.push_console_line(format!("^1render_perf_capture:^7 {error}")),
        }
        CommandResult::Handled
    }

    cmd_gputilecost ["gputilecost", "gpu_tile_cost"] ("gputilecost [<cols>x<rows>] [samples] [label] | show <pass> | passes | off | cancel", "Measure real GPU cost per screen tile: scissors every screen-space pass to one tile per frame, subtracts a 1x1 baseline, then overlays a heatmap and writes CSV/JSON. Hold the camera still.", CommandScope::Always) |self, command| {
        use crate::renderer::TileCostCommand;
        let words: Vec<_> = command.split_whitespace().collect();
        let usage = "^3usage:^7 gputilecost [<cols>x<rows>=32x18] [samples=8] [label] | show <pass> | passes | off | cancel";
        let request = match words.get(1).map(|word| word.to_ascii_lowercase()).as_deref() {
            Some("show") => match words.get(2) {
                Some(pass) => TileCostCommand::Show((*pass).to_owned()),
                None => {
                    self.push_console_line(usage.to_owned());
                    return CommandResult::Handled;
                }
            },
            Some("passes") => TileCostCommand::ListPasses,
            Some("off") | Some("hide") => TileCostCommand::Hide,
            Some("cancel") | Some("stop") => TileCostCommand::Cancel,
            _ => {
                let mut rest = &words[1..];
                let (mut cols, mut rows) = (32u32, 18u32);
                if let Some((c, r)) = rest.first().and_then(|word| word.split_once(['x', 'X'])) {
                    match (c.parse::<u32>(), r.parse::<u32>()) {
                        (Ok(c), Ok(r)) if (1..=64).contains(&c) && (1..=64).contains(&r) => (cols, rows) = (c, r),
                        _ => {
                            self.push_console_line(usage.to_owned());
                            return CommandResult::Handled;
                        }
                    }
                    rest = &rest[1..];
                }
                let mut samples = 8usize;
                if let Some(value) = rest.first().and_then(|word| word.parse::<usize>().ok()) {
                    if !(3..=64).contains(&value) {
                        self.push_console_line("^1gputilecost:^7 samples must be 3..64".to_owned());
                        return CommandResult::Handled;
                    }
                    samples = value;
                    rest = &rest[1..];
                }
                let label: String = rest.first().copied().unwrap_or("")
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .take(32)
                    .collect();
                let stem_name = if label.is_empty() {
                    format!("tilecost-{}", demo_recording_timestamp())
                } else {
                    format!("tilecost-{}-{label}", demo_recording_timestamp())
                };
                let stem = self.game_write_path(std::path::PathBuf::from("renderperf").join(stem_name));
                self.push_console_line(format!(
                    "^2GPU TILE COST ARMED^7 {cols}x{rows}, {samples} samples/tile after 2s. Close console, hold still; heatmap appears when done. Output: {}.csv/.json",
                    stem.display()
                ));
                TileCostCommand::Start { stem, cols, rows, samples }
            }
        };
        if let Err(error) = self.render.as_ref().ok_or("renderer is unavailable")
            .and_then(|render| render.tile_cost(request)) {
            self.push_console_line(format!("^1gputilecost:^7 {error}"));
        }
        CommandResult::Handled
    }

    cmd_inputdiag ["inputdiag", "framediag"] ("inputdiag [label]", "Arm a 5-second raw-mouse/Win32/event-loop/render capture. Recording starts on the first gameplay mouse movement and stops/writes automatically.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] | [_verb, _] => {
                let label = words.get(1).copied().unwrap_or("");
                let label: String = label
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .take(32)
                    .collect();
                let stem_name = if label.is_empty() {
                    format!("inputdiag-{}", demo_recording_timestamp())
                } else {
                    format!("inputdiag-{}-{label}", demo_recording_timestamp())
                };
                let stem = self.game_write_path(std::path::PathBuf::from("inputdiag").join(stem_name));
                if let Some(parent) = stem.parent() {
                    if let Err(error) = std::fs::create_dir_all(parent) {
                        self.push_console_line(format!(
                            "^1inputdiag:^7 couldn't create {}: {error}",
                            parent.display()
                        ));
                        return CommandResult::Handled;
                    }
                }
                let result = self
                    .render
                    .as_ref()
                    .ok_or("renderer is unavailable")
                    .and_then(|render| render.arm_input_diagnostic(stem));
                match result {
                    Ok(()) => self.push_console_line(
                        "^2INPUT DIAG ARMED^7 - close the console, then move the mouse in steady circles continuously. Capture starts on the first gameplay mouse movement, records 5 seconds, and stops automatically."
                            .to_owned(),
                    ),
                    Err(error) => self.push_console_line(format!("^1inputdiag:^7 {error}")),
                }
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 inputdiag [label]".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_cg_eventstats ["cg_eventStats", ] ("cg_eventStats [clear]", "Summarize accepted demo events; use 'cg_eventStats clear' to reset counters.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                let lines = self
                    .game_session
                    .as_ref()
                    .map(GameSession::event_stats_lines)
                    .unwrap_or_else(|| vec!["^3CG EVENT STATS:^7 no demo is playing".into()]);
                for line in lines {
                    self.push_console_line(line);
                }
                CommandResult::Handled
            }
            [_verb, argument] if argument.eq_ignore_ascii_case("clear") => {
                if let Some(playback) = self.game_session.as_mut() {
                    playback.clear_event_stats();
                    self.console_status = "CG EVENT STATS CLEARED".into();
                } else {
                    self.console_status = "NO DEMO IS PLAYING".into();
                }
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_r_dumpmaterials ["r_dumpmaterials", "dumpmaterials"] ("r_dumpmaterials [argument]", "Dump material diagnostics; optional filter or 'all'.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.render_command(RenderCommand::DumpMaterials {
                    filter: None,
                    all: false,
                });
                self.console_status =
                    "MATERIAL DEBUG DUMPED TO TERMINAL (ENHANCED MATERIALS ONLY)".into();
                CommandResult::Status
            }
            [_verb, argument] => {
                self.render_command(RenderCommand::DumpMaterials {
                    filter: (!argument.eq_ignore_ascii_case("all")).then(|| (*argument).to_owned()),
                    all: argument.eq_ignore_ascii_case("all"),
                });
                self.console_status = if argument.eq_ignore_ascii_case("all") {
                    "ALL MATERIAL DEBUG DUMPED TO TERMINAL".into()
                } else {
                    format!("MATERIAL DEBUG FILTER '{argument}' DUMPED TO TERMINAL")
                };
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_benchturn ["benchturn", ] ("benchturn [counts] [period]", "Benchmarks: inject a synthetic raw mouse-motion event of <counts> along X every event-loop turn (through the real mouse path); with <period>, reverse direction every <period> turns (a wiggle). 0 or no argument stops.", CommandScope::Always) |self, command| {
        let mut words = command.split_whitespace().skip(1).map(|v| v.parse::<i32>().ok());
        let counts = words.next().flatten().unwrap_or(0);
        let period = words.next().flatten().unwrap_or(0).max(0);
        crate::app::lifecycle::BENCH_TURN_COUNTS.store(counts, std::sync::atomic::Ordering::Relaxed);
        crate::app::lifecycle::BENCH_TURN_PERIOD.store(period, std::sync::atomic::Ordering::Relaxed);
        self.push_console_line(format!("benchturn {counts} {period}"));
        CommandResult::Handled
    }

    cmd_viewpos ["viewpos", ] ("viewpos","Print the view origin and angles: (x y z) : yaw (pitch p).", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.print_viewpos();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_menudevmap ["menudevmap", ] ("menudevmap", "devmap mp/duel9 with the main-menu look (Yavin retextures, water, campfire and waterfall fx) but without the props. Then run `menuent` to stage the props as editable Map Mods placements.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.menu_look_oneshot = true;
                self.execute_command_line("devmap mp/duel9");
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_placeedit ["placeedit", ] ("placeedit", "Edit the staged model placements: aim at one and click to pick it up (move, wheel to scale, right-drag to rotate, click to drop), Ctrl+click copies, Delete/X removes, Ctrl+Z undoes, E toggles back to placing, Esc exits.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.enter_placement_edit();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_menuent ["menuent", ] ("menuent [overwrite]", "In a menudevmap session: load the menu props from mapmods/mp/duel9.ent as editable Map Mods placements (writing it from the built-in layout first if it does not exist; `menuent overwrite` rewrites it from the built-in layout). Edit them with the placement tool, then Map Mods > EXPORT to mapmods/mp/duel9.ent: the main menu reads that file.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let overwrite = match words.as_slice() {
            [_verb] => false,
            [_verb, arg] if arg.eq_ignore_ascii_case("overwrite") => true,
            _ => return CommandResult::Unhandled,
        };
        match self.write_default_menu_ent(overwrite) {
            Ok(message) => self.push_console_line(format!("^5MENUENT:^7 {message}")),
            Err(error) => self.push_console_line(format!("^1menuent:^7 {error}")),
        }
        CommandResult::Handled
    }

    cmd_oceanpool ["oceanpool", ] ("oceanpool [pool] [reset | amp wavelength direction speed steepness slosh windchop foam spray]", "Script the Water settings page's local pool override (1-based pool number as in the pool label). No arguments lists pools; 'reset' is RESET OCEAN DEFAULTS.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                let lines: Vec<String> = self.promoted_oceans.iter().map(|pool| {
                    let local = self.promoted_ocean_overrides.iter().any(|(slot, _)| *slot == pool.index);
                    format!("{} | heuristic amplitude {:.1}u variation {:.0}% | {}", pool.label(),
                        pool.heuristic.amplitude, pool.heuristic.variation * 100.0, if local { "local override" } else { "auto" })
                }).collect();
                for line in lines { self.push_console_line(line); }
                CommandResult::Handled
            }
            [_verb, pool, rest @ ..] => {
                let Some(descriptor) = pool.parse::<usize>().ok().and_then(|n| n.checked_sub(1))
                    .and_then(|slot| self.promoted_oceans.iter().find(|p| p.index == slot)).cloned() else {
                    self.push_console_line("^1oceanpool: no such pool (run oceanpool with no arguments)".to_owned());
                    return CommandResult::Handled;
                };
                if rest == ["reset"] {
                    self.promoted_ocean_overrides.retain(|(slot, _)| *slot != descriptor.index);
                } else {
                    let values: Vec<f32> = rest.iter().filter_map(|word| word.parse().ok()).collect();
                    if values.len() != 9 || values.len() != rest.len() {
                        self.push_console_line("usage: oceanpool <pool> reset | oceanpool <pool> amp wavelength direction speed steepness slosh windchop foam spray".to_owned());
                        return CommandResult::Handled;
                    }
                    let local = self.promoted_ocean_overrides.iter().find(|(slot, _)| *slot == descriptor.index).map(|(_, s)| *s);
                    let mut settings = crate::ocean::appearance::surface_settings(self.video.ocean_settings, &descriptor, local);
                    let a = &mut settings.authored;
                    (a.amplitude, a.wavelength, a.direction, a.speed, a.steepness, a.slosh, a.wind_chop, a.foam, a.spray) =
                        (values[0], values[1], values[2], values[3], values[4], values[5], values[6], values[7], values[8]);
                    self.promoted_ocean_overrides.retain(|(slot, _)| *slot != descriptor.index);
                    self.promoted_ocean_overrides.push((descriptor.index, settings.sanitize()));
                }
                self.render_command(RenderCommand::SetPromotedOceanSettings(self.promoted_ocean_overrides.clone()));
                self.publish_ui();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_oceanwake ["oceanwake", ] ("oceanwake ship [boat|destroyer|battleship] [m/s] [circle radius m] | strike [round|charge|aircraft] [size] | clear | reset", "Drive Blue Water's ship wake (r_oceanWake 1, native / Hybrid ocean). 'ship' sails a proxy hull ahead of the camera (radius 0: a straight run across the view); 'strike' drops an impact 60 m ahead (round: calibre m; charge: kg TNT; aircraft: m/s).", CommandScope::Always) |self, command| {
        use crate::ocean::{ShipClass, StrikeKind, WakeCommand};
        let words: Vec<_> = command.split_whitespace().collect();
        let number = |word: Option<&&str>, default: f32| word.and_then(|w| w.parse::<f32>().ok()).unwrap_or(default);
        let wake = match words.as_slice() {
            [_verb, "ship", rest @ ..] => {
                let class = rest.first().and_then(|w| ShipClass::parse(w));
                let offset = usize::from(class.is_some());
                let class = class.unwrap_or(ShipClass::Destroyer);
                let default_speed = match class { ShipClass::Boat => 9.0, ShipClass::Destroyer => 14.0, ShipClass::Battleship => 12.0 };
                let default_radius = match class { ShipClass::Boat => 60.0, ShipClass::Destroyer => 250.0, ShipClass::Battleship => 450.0 };
                WakeCommand::Demo { class, speed: number(rest.get(offset), default_speed), radius: number(rest.get(offset + 1), default_radius) }
            }
            [_verb, "strike", rest @ ..] => {
                let size = rest.get(1);
                let kind = match rest.first().copied().unwrap_or("round") {
                    "charge" => StrikeKind::Charge { kg: number(size, 500.0), depth: 0.0 },
                    "aircraft" => StrikeKind::Aircraft { speed: number(size, 60.0) },
                    _ => StrikeKind::Round { caliber_m: number(size, 0.38), speed: 700.0 },
                };
                WakeCommand::StrikeAhead { distance_m: 60.0, kind }
            }
            [_verb, "clear"] => { self.render_command(RenderCommand::OceanWake(WakeCommand::ClearShips)); WakeCommand::ClearDemos }
            [_verb, "reset"] => WakeCommand::Reset,
            _ => {
                self.push_console_line(format!("r_oceanWake {} (set 1 to enable). usage: oceanwake ship [boat|destroyer|battleship] [m/s] [circle radius m] | strike [round|charge|aircraft] [size] | clear | reset", self.video.ocean_settings.blue_wake));
                return CommandResult::Handled;
            }
        };
        if self.video.ocean_settings.blue_wake == 0 {
            self.push_console_line("^3oceanwake: r_oceanWake is 0, so Blue Water's wake is off; set it to 1 (native or Hybrid ocean).".to_owned());
        }
        self.render_command(RenderCommand::OceanWake(wake));
        CommandResult::Handled
    }

    cmd_perfsample ["perfsample", ] ("perfsample [arguments]", "perfsample <seconds> [label]: hold the command buffer, then print the average render FPS.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, arguments @ ..] => {
                self.begin_perf_sample(arguments);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_waitshadermode ["waitshadermode", ] ("waitshadermode <mode> [timeout]", "Hold the command buffer until r_fragmentShaderMode <mode> is active and no world variant is compiling; logs READY/TIMEOUT with the wait time.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, arguments @ ..] => {
                self.begin_shader_mode_wait(arguments);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_puddle_debug ["puddle_debug", ] ("puddle_debug [mode]", "puddle_debug [0-8]: wetness, puddle-coat, runoff, material and flow diagnostic views; no argument toggles view 1.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let mode = match words.as_slice() {
            // Bare verb keeps the old toggle so the PUDDLE DEBUG keybind still works.
            [_verb] => u32::from(self.puddle_debug_mode == 0),
            [_verb, argument] => match argument.parse::<u32>() {
                Ok(mode) if mode <= 8 => mode,
                _ => {
                    self.console_status = "puddle_debug: mode must be 0-8".into();
                    return CommandResult::Status;
                }
            },
            _ => return CommandResult::Unhandled,
        };
        self.puddle_debug_mode = mode;
        self.render_command(RenderCommand::SetPuddleDebug(mode));
        self.console_status = match mode {
            0 => "PUDDLE DEBUG OFF".into(),
            1 => "PUDDLE DEBUG 1: coverage (blue=standing water, green=wet film)".into(),
            2 => "PUDDLE DEBUG 2: coat source (red=skybox, green=reflection probe)".into(),
            3 => "PUDDLE DEBUG 3: reflection-probe radiance the coat reflects".into(),
            4 => "PUDDLE DEBUG 4: skybox radiance the coat reflects".into(),
            5 => "PUDDLE DEBUG 5: runoff (red=rivulets, blue=across-slope tilt)".into(),
            6 => "PUDDLE DEBUG 6: material (red=porosity, green=gloss, blue=receiver)".into(),
            7 => "PUDDLE DEBUG 7: runoff albedo darkening at 3x (white = a third darker)".into(),
            _ => "PUDDLE DEBUG 8: flow (red = impossible uphill, green = normal drift, blue = direction)".into(),
        };
        CommandResult::Status
    }

    cmd_puddle_sky ["puddle_sky", ] ("puddle_sky [window] [ceiling]", "puddle_sky <window> <ceiling>: where the puddle coat hands over from the reflection probe to the skybox, and the skybox's largest share.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {}
            [_verb, window, ceiling] => {
                match (window.parse::<f32>(), ceiling.parse::<f32>()) {
                    (Ok(window), Ok(ceiling)) => {
                        self.puddle_sky_blend =
                            (window.clamp(0.02, 1.0), ceiling.clamp(0.0, 1.0));
                        self.render_command(RenderCommand::SetPuddleSkyBlend {
                            window: self.puddle_sky_blend.0,
                            ceiling: self.puddle_sky_blend.1,
                        });
                    }
                    _ => {
                        self.console_status = "puddle_sky: expected two numbers".into();
                        return CommandResult::Status;
                    }
                }
            }
            _ => return CommandResult::Unhandled,
        }
        let (window, ceiling) = self.puddle_sky_blend;
        self.console_status = format!(
            "puddle_sky: window {window:.3} (hands over to skybox by {:.1} degrees), ceiling {ceiling:.3}",
            window.min(1.0).asin().to_degrees()
        );
        CommandResult::Status
    }

    cmd_dev ["dev", ] ("dev", "Hold bound key/button for the relative-mouse developer wheel: Trace, 3D Entities, Entity Viewer, Remap Shader (planned), and Shader Complexity. Left click selects; releasing the dev bind cancels.", CommandScope::Always) |self, command| {
        match command.split_whitespace().collect::<Vec<_>>().as_slice() {
            [_verb] => {
                self.toggle_dev_radial();
                CommandResult::Handled
            }
            _ => CommandResult::Unhandled,
        }
    }

    cmd_trace ["trace", ] ("trace", "jaPRO: Trace what you are looking at.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.trace_surface_center();
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_trace_clear ["trace_clear", ] ("trace_clear", "Clear the pinned surface inspection and triangle highlight.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.clear_surface_inspection();
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_entities ["entities", "entgraph"] ("entities [targetname|classname]", "Open the 2D entity blueprint: entities [targetname|classname] selects the first match.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, query @ ..] => {
                let query = query.join(" ");
                self.open_entity_graph((!query.is_empty()).then_some(query.as_str()));
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_uipage ["uipage", ] ("uipage [page]", "Inspect a UI page.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, page @ ..] => {
                self.open_ui_page(page);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_fxinfo ["fxinfo", ] ("fxinfo", "Print current effects diagnostics.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                match self.game_session.as_ref() {
                    Some(playback) => {
                        let stats = playback.weapon_fx.stats();
                        self.push_console_line(format!(
                            "^3FX:^7 effects {} | live primitives {} | GPU-offloaded {} | GPU slots {} | scheduled {} | dropped {} | unsupported spawns {} | lod culled {} | lod saved {} | surfaces {}",
                            stats.registered,
                            stats.active,
                            stats.gpu_offloaded,
                            playback.gpu_pool.live_count(),
                            stats.scheduled,
                            stats.dropped,
                            stats.unsupported_spawns,
                            stats.lod_culled,
                            stats.lod_saved,
                            playback.fx_surfaces.len(),
                        ));
                    }
                    None => self.push_console_line("^3FX:^7 no active CGame (start a demo first)."),
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_soundinfo ["soundinfo", ] ("soundinfo", "Show active output format, device buffer, voice count and pre-limiter overload diagnostics.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                let sound_state = self
                    .game_session
                    .as_ref()
                    .and_then(|playback| playback.sound_presenter.as_ref())
                    .and_then(|sound| sound.info().map(|info| (info, sound.source_rate_summary())));
                if let Some((info, source_rates)) = sound_state {
                    self.push_console_line(format!(
                        "^3AUDIO OUTPUT:^7 {} ch | {} Hz | {} | buffer {}",
                        info.channels, info.sample_rate, info.sample_format, info.buffer_size
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO MIX:^7 voices {} / 32 | loops {} ({} requests) | pre-limit peak {:.3} | overload samples {}",
                        info.active_voices, info.active_loops, info.loop_requests, info.peak_before_limiter, info.overload_samples
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO SOURCES:^7 {} (each retains its decoded rate; mixer converts to {} Hz output)",
                        source_rates, info.sample_rate
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO LEVELS:^7 s_volume {:.3} | s_volumeVoice {:.3} | s_musicvolume {:.3} | s_separation {:.3}",
                        self.audio.effects_volume,
                        self.audio.voice_volume,
                        self.audio.music_volume,
                        self.audio.separation
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO PAN:^7 largest per-frame gain step {:.3} (glided per sample, not stepped)",
                        info.largest_gain_step
                    ));
                    if let Some(path) = &info.eal_file {
                        self.push_console_line(format!(
                            "^3EAL ZONES:^7 {} | {} presets | listener: {} | authored acoustics: {}",
                            path, info.eal_preset_count,
                            info.eal_environment.as_deref().unwrap_or("outside EAL geometry"),
                            if info.eal_effects_active { "active" } else { "inactive" }
                        ));
                    } else {
                        self.push_console_line(format!(
                            "^3EAL ZONES:^7 no .eal dataset loaded | s_eal {} | authored acoustics inactive",
                            u8::from(info.eal_enabled)
                        ));
                    }
                    if let Some(error) = &info.eal_effects_error {
                        self.push_console_line(format!("^3EAL DSP:^7 {error}"));
                    }
                    if info.eal_effects_active {
                        self.push_console_line(format!("^3EAL SCENE:^7 {} sources | {} rooms | {} filtered | {} diffracted | {} directional rooms | {} merged fallback loops | HRTF {}",
                            info.eal_sources, info.eal_rooms, info.eal_filtered_sources, info.eal_diffracted_sources,
                            info.eal_directional_rooms, info.eal_fallback_loops, if info.eal_hrtf { "on" } else { "off" }));
                        self.push_console_line(format!(
                            "^3EAL WORKER:^7 {:.1} ms scene latency | max render {} us | {} underruns | {} queue/capacity drops",
                            info.eal_latency_ms, info.eal_max_render_us, info.eal_underruns, info.eal_overflows
                        ));
                        self.push_console_line(format!("^3EAL DEBUG:^7 mode {} (0 EAX callback (default), 1 PCM, 2 OpenAL dry, 3 cached EAX, 4 OpenAL 2D, 5 AL tone, 6 AL double-preroll, 7 EAX callback, 8 callback tone, 9 legacy EAX queue)", info.eal_debug_mode));
                        self.push_console_line(format!(
                            "^3EAL OPENAL:^7 {} empty queues | {} unexpected restarts | {} multi-buffer processed | {} queued-buffer reuses",
                            info.eal_queue_starves, info.eal_restarts, info.eal_multi_processed, info.eal_buffer_reuse
                        ));
                        self.push_console_line(format!(
                            "^3EAL TRANSPORT:^7 {} | callback underfeed {} samples | dropped {} samples | device HRTF status {}",
                            match info.eal_transport { 1 => "callback", 2 => "legacy queue", _ => "raw PCM / initializing" },
                            info.eal_callback_underfeed_samples, info.eal_callback_dropped_samples, info.eal_hrtf_status
                        ));
                        self.push_console_line(format!(
                            "^3EAL EDGES:^7 maximum block-boundary jump (input {:.5}, OpenAL output {:.5}) | cumulative since mode switch",
                            info.eal_input_edge_max, info.eal_output_edge_max
                        ));
                    }
                    if info.overload_samples > 0 {
                        self.push_console_line(
                            "^3AUDIO NOTE:^7 the game mix exceeded full scale before the output limiter; this is a likely crackle/clipping source.",
                        );
                    }
                } else {
                    self.push_console_line(
                        "^3AUDIO:^7 no active CGame audio backend (start a demo/map presentation first).",
                    );
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

}
