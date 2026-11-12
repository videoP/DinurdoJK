//! Declarative command handlers.
use crate::app::{demo_recording_timestamp, App, GameSession, RenderCommand, SessionPhase};

use super::{CommandDef, CommandResult, CommandScope};
commands! {
    cmd_jk2_demo_info ["jk2_demoInfo"] ("jk2_demoInfo <demoname or absolute path>", "Decode an entire JK2 demo and report its version, map, duration, and snapshot integrity. Names resolve under fs_jk2basepath/base/demos, then fs_jk2cdpath/base/demos.", CommandScope::Always) |self, command| {
        let args = jka_protocol::commands::tokenize(command.as_bytes());
        let result = (|| {
            if args.len() != 2 { return Err("usage: jk2_demoInfo <demoname or absolute path>".to_owned()); }
            let name = std::str::from_utf8(&args[1]).map_err(|_| "Invalid demo path".to_owned())?;
            let path = crate::jk2_diagnostics::resolve_demo(&self.base, &self.fs_roots, name)?;
            let tx = self.background_result_tx.clone();
            self.background_jobs.io(move || {
                let result = crate::jk2_diagnostics::inspect(&path);
                let _ = tx.send(crate::app::background_jobs::JobResult::Jk2DemoInfo { result });
            })?;
            Ok(())
        })();
        match result {
            Ok(()) => self.push_console_line("JK2 demo inspection queued"),
            Err(error) => self.push_console_line(error),
        }
        CommandResult::Handled
    }

    cmd_force_jump_slope_fix ["force_jump_slope_fix"] ("force_jump_slope_fix [0|1]", "Solo authority: time-consistent Force Jump boost on overhead slopes (default 1). Remote correction applies only to jaPRO race mode and requires matching server physics.", CommandScope::Connected) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let Some(server) = self.local_server.as_mut() else {
            self.push_console_line("force_jump_slope_fix requires a local game".to_owned());
            return CommandResult::Handled;
        };
        if let [_verb, value @ ("0" | "1")] = words.as_slice() {
            server.set_force_jump_slope_fix(*value == "1");
        } else if words.len() != 1 {
            self.push_console_line("usage: force_jump_slope_fix [0|1]".to_owned());
            return CommandResult::Handled;
        }
        let enabled = server.force_jump_slope_fix();
        self.push_console_line(format!("force_jump_slope_fix {} (local authority)", i32::from(enabled)));
        CommandResult::Handled
    }

    cmd_jumptrace ["jumptrace"] ("jumptrace [milliseconds]", "Solo: log bounded native Force Jump/collision traces (default off; captures 2000 ms, maximum 5000).", CommandScope::Connected) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let msec = match words.as_slice() {
            [_verb] => Some(2000),
            [_verb, value] => value.parse::<i32>().ok().filter(|v| (0..=5000).contains(v)),
            _ => None,
        };
        match (self.local_server.as_mut(), msec) {
            (Some(server), Some(msec)) => {
                server.start_jump_trace(msec);
                self.push_console_line(format!("jumptrace: capturing {msec} ms of local physics to latest.log"));
            }
            _ => self.push_console_line("usage: jumptrace [0..5000] (local game)".to_owned()),
        }
        CommandResult::Handled
    }

    cmd_movespeed ["movespeed"] ("movespeed", "Print the local authoritative horizontal speed, velocity and position.", CommandScope::Connected) |self, _command| {
        if let Some(server) = self.local_server.as_ref() {
            let v = server.view();
            self.push_console_line(format!("MOVESPEED time={} horizontal={:.3} velocity={:?} origin={:?}", v.command_time, v.velocity[0].hypot(v.velocity[1]), v.velocity, v.origin));
        } else {
            self.push_console_line("movespeed requires a local game".to_owned());
        }
        CommandResult::Handled
    }
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
        for line in self.studio_memory_report_lines() { self.push_console_line(line); }
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

    cmd_shitter_sweep ["shitter_sweep", "shittersweep"] ("shitter_sweep", "Automatically compare full-resolution Shitter world and swapchain paths; save one TXT report in renderperf and restore all original modes.", CommandScope::Connected) |self, command| {
        if command.split_whitespace().count() != 1 {
            self.push_console_line("^3usage:^7 shitter_sweep".to_owned());
            return CommandResult::Handled;
        }
        let path = self.game_write_path(std::path::PathBuf::from("renderperf")
            .join(format!("shitter-sweep-{}.txt", demo_recording_timestamp())));
        let result = self.render.as_ref().ok_or("renderer is unavailable")
            .and_then(|render| render.start_shitter_sweep(path.clone()));
        match result {
            Ok(()) => self.push_console_line(format!(
                "^2SHITTER SWEEP ARMED^7 Close console, keep camera still. Automatic test and settings restoration. Send this one log: {}",
                path.display())),
            Err(error) => self.push_console_line(format!("^1shitter_sweep:^7 {error}")),
        }
        CommandResult::Handled
    }

    cmd_igpu_audit ["igpu_audit", "render_gpu_audit"] ("igpu_audit [label]", "One-command iGPU study: frame CPU/GPU timing, stage exclusions, color clear, timestamp overhead, and automatic 4x4 GPU pixel-cost map. Writes CSV/JSON reports and restores settings.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if words.len() > 2 {
            self.push_console_line("^3usage:^7 igpu_audit [label]".to_owned());
            return CommandResult::Handled;
        }
        let label: String = words.get(1).copied().unwrap_or("")
            .chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(32).collect();
        let name = if label.is_empty() { format!("igpu-audit-{}", demo_recording_timestamp()) }
            else { format!("igpu-audit-{}-{label}", demo_recording_timestamp()) };
        let stem = self.game_write_path(std::path::PathBuf::from("renderperf").join(name));
        let result = self.render.as_ref().ok_or("renderer is unavailable")
            .and_then(|render| render.start_render_perf_audit(stem.clone()));
        match result {
            Ok(()) => self.push_console_line(format!(
                "^2IGPU AUDIT ARMED^7 Keep camera still; run will automatically finish and restore settings. Output: {}.csv/.json",
                stem.display())),
            Err(error) => self.push_console_line(format!("^1igpu_audit:^7 {error}")),
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

    cmd_sailingdebug ["sailingdebug", ] ("sailingdebug [0|1]", "jaPRO sailing: draw a line from your head to the predicted (server-physics) water surface below you. Yellow cross = physics surface at your command time; magenta cross = Hybrid's drawn analytic swell at the ocean render time; cyan = the same analytic swell at your command time. Replaces the r_drawEntities overlay while on.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let enabled = match words.as_slice() {
            [_verb] => !self.sailing_debug,
            [_verb, arg] => *arg != "0",
            _ => return CommandResult::Unhandled,
        };
        self.sailing_debug = enabled;
        crate::ocean::authoring::SAILING_DEBUG.store(enabled, std::sync::atomic::Ordering::Relaxed);
        if !enabled {
            self.render_command(RenderCommand::SetEntityMarkers(None));
            self.entity_marker_cache = None;
        }
        self.console_status = format!("SAILINGDEBUG {}", if enabled { "ON" } else { "OFF" });
        CommandResult::Handled
    }

    cmd_menuent ["menuent", ]("menuent [overwrite]", "In a menudevmap session: load the menu props from mapmods/mp/duel9.ent as editable Map Mods placements (writing it from the built-in layout first if it does not exist; `menuent overwrite` rewrites it from the built-in layout). Edit them with the placement tool, then Map Mods > EXPORT to mapmods/mp/duel9.ent: the main menu reads that file.", CommandScope::Always) |self, command| {
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

    cmd_spawnriver ["spawnriver", "riversource"] ("spawnriver [height above ground] [patch radius] [source width] [cells] [simulation speed] [CFS|level|discharge:<CFS>]", "Developer test: pour water in at the crosshair using the native Unreal river solver (simulation-first, no spline). Defaults come from Video > River > New river defaults. Optional CFS caps supply while filling toward the target level; level selects unlimited held-water-level mode. Simulation speed is the fast-solve rate and automatically returns to 1x after steady-flow detection. Draws the ported Single Layer Water body with sky and screen-space reflections; foam uses textures/japro/river_foam_detail. 'spawndrain' adds drains; 'riverclear' removes all rivers and drains.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let defaults=self.river_editor.defaults;
        let parsed=(||->Result<crate::river_authoring::SourceParameters,String> {
            let number=|i:usize,default:f32|->Result<f32,String> {
                words.get(i).map_or(Ok(default),|w|w.parse::<f32>().map_err(|_|format!("invalid source parameter: {w}")))
            };
            let cells=number(4,defaults.cells as f32)?;
            if !cells.is_finite()||cells.fract()!=0.0||!(16.0..=1024.0).contains(&cells) {return Err("cells must be an integer from 16 to 1024".into());}
            let (mode, discharge_cfs)=match words.get(6) {
                Some(w) if w.eq_ignore_ascii_case("level") =>
                    (crate::river_runtime::SourceMode::WaterLevel, None),
                Some(w) if w.to_ascii_lowercase().starts_with("discharge:") =>
                    (crate::river_runtime::SourceMode::Discharge,
                        Some(w["discharge:".len()..].parse::<f32>().map_err(|_|"discharge CFS must be numeric".to_owned())?)),
                Some(w) => (crate::river_runtime::SourceMode::WaterLevel,
                    Some(w.parse::<f32>().map_err(|_|"CFS must be numeric, 'level', or 'discharge:<CFS>'".to_owned())?)),
                None => (defaults.mode, defaults.discharge_cfs),
            };
            Ok(crate::river_authoring::SourceParameters {height:number(1,defaults.height)?,radius:number(2,defaults.radius)?,width:number(3,defaults.width)?,cells:cells as u32,speed:number(5,defaults.speed)?,mode,discharge_cfs})
        })();
        let params=match parsed {Ok(p)=>p,Err(error)=>{self.push_console_line(format!("^1spawnriver: {error}"));return CommandResult::Handled;}};
        if let Err(error)=params.at([0.0;3]).validate() {self.push_console_line(format!("^1spawnriver: {error}"));return CommandResult::Handled;}
        let Some(trace) = self.river_crosshair_trace("spawnriver") else { return CommandResult::Handled; };
        if crate::river_runtime::debug_mode() >= 2 {
            self.push_console_line(format!(
                "[RIVER DBG HIT] normal=({:.3},{:.3},{:.3}) flags=0x{:x} contents=0x{:x} trace_fraction={:.5} {}",
                trace.normal[0], trace.normal[1], trace.normal[2], trace.surface_flags,
                trace.contents, trace.fraction,
                if trace.normal[2] < 0.6 { "WARNING: steep/vertical face, not a floor" } else { "floor-like face" }
            ));
        }
        let collision = match self.river_collision() {
            Ok(collision) => collision,
            Err(error) => {
                self.push_console_line(format!("^1spawnriver: {error}"));
                return CommandResult::Handled;
            }
        };
        let source = params.at(trace.end);
        if let Err(error) = source.validate() {
            self.push_console_line(format!("^1spawnriver: {error}"));
            return CommandResult::Handled;
        }
        let definition=self.river_editor.add(source,self.video.river_settings);
        let supply=match (source.mode, source.discharge_cfs) {
            (crate::river_runtime::SourceMode::WaterLevel, Some(q)) => format!("water level {:.0}, max {q} CFS", source.water_level),
            (crate::river_runtime::SourceMode::WaterLevel, None) => format!("unlimited water level {:.0}", source.water_level),
            (crate::river_runtime::SourceMode::Discharge, Some(q)) => format!("discharge {q} CFS (unclamped)",),
            (crate::river_runtime::SourceMode::Discharge, None) => unreachable!("validated discharge source must have CFS"),
        };
        self.push_console_line(format!("River {} at ({:.0} {:.0} {:.0}), {supply}, patch +/-{:.0}, {} cells, {}x simulation speed",definition.id,trace.end[0],trace.end[1],trace.end[2],source.radius,source.resolution,source.rate));
        self.render_command(RenderCommand::RiverSpawn(definition, collision));
        CommandResult::Handled
    }

    cmd_spawndrain ["spawndrain", ] ("spawndrain [radius] [overflow height|floor] [max CFS|unlimited]", "Developer river drain at the crosshair (not Unreal): water inside the radius (map units) leaves every river. 'floor' (default) empties the cells; a number is an overflow drain that holds water at that height above the crosshair point. Max CFS caps the removal rate per simulated second. Defaults come from Video > River > Drains, which also edits placed drains live. 'riverclear' removes rivers and drains.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let defaults = self.river_editor.drain_defaults;
        let parsed = (|| -> Result<crate::river_authoring::DrainParameters, String> {
            let radius = match words.get(1) {
                Some(w) => w.parse::<f32>().map_err(|_| format!("invalid radius: {w}"))?,
                None => defaults.radius,
            };
            let overflow_height = match words.get(2) {
                Some(w) if w.eq_ignore_ascii_case("floor") => None,
                Some(w) => Some(w.parse::<f32>().map_err(|_| format!("overflow height must be a number or 'floor': {w}"))?),
                None => defaults.overflow_height,
            };
            let max_cfs = match words.get(3) {
                Some(w) if w.eq_ignore_ascii_case("unlimited") => None,
                Some(w) => Some(w.parse::<f32>().map_err(|_| format!("max CFS must be a number or 'unlimited': {w}"))?),
                None => defaults.max_cfs,
            };
            Ok(crate::river_authoring::DrainParameters { radius, overflow_height, max_cfs })
        })();
        let params = match parsed { Ok(p) => p, Err(error) => { self.push_console_line(format!("^1spawndrain: {error}")); return CommandResult::Handled; } };
        let Some(trace) = self.river_crosshair_trace("spawndrain") else { return CommandResult::Handled; };
        let drain = params.at(trace.end);
        if let Err(error) = drain.validate() {
            self.push_console_line(format!("^1spawndrain: {error}"));
            return CommandResult::Handled;
        }
        self.river_editor.add_drain(drain);
        self.push_console_line(format!(
            "Drain at ({:.0} {:.0} {:.0}), radius {:.0}, {}, {}",
            trace.end[0], trace.end[1], trace.end[2], drain.radius,
            drain.level.map_or("floor drain".to_owned(), |z| format!("overflow at z {z:.0}")),
            drain.max_cfs.map_or("unlimited flow".to_owned(), |q| format!("max {q} CFS")),
        ));
        self.send_river_drains();
        CommandResult::Handled
    }

    cmd_riverdump ["riverdump", ] ("riverdump", "River diagnostics (sparse rivers): read the solver back once (brief GPU stall), print the bed/water of the column at your view position, water volume and void-wall/pit counts, and write top-down PNGs to screenshots/ (river-dump-*: bed grey, water blue, walls red, void walls magenta, unwalled void pits yellow, source green, drains orange, you white). A second dump also writes river-diff-* (red = water left, green = water arrived) and the volume change.", CommandScope::Always) |self, _command| {
        let eye = crate::scene::jka_position(self.camera.position.to_array());
        let directory = self.game_write_path("screenshots");
        self.render_command(RenderCommand::RiverDump { eye, directory });
        CommandResult::Handled
    }

    cmd_riverclear ["riverclear", ] ("riverclear", "Remove all developer rivers (spawnriver) and drains (spawndrain).", CommandScope::Always) |self, _command| {
        self.river_editor.clear();
        self.render_command(RenderCommand::RiverClear);
        self.push_console_line("riverclear: removed rivers and drains".to_owned());
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

    cmd_trace ["trace", ] ("trace [entityNum]", "Trace the view or annotate a server trace with client-visible entity state, effect configstring and BSP key/value pairs.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.trace_surface_center();
                CommandResult::Status
            }
            [_, number] => {
                // Preserve the existing server-side `trace <entityNum>` output
                // (including server-only fields) by forwarding the command.
                // Add the information the client can resolve, especially the
                // CS_EFFECTS qpath which the server's trace does not print.
                if let Ok(number) = number.parse::<u16>() {
                    if number < 1024 {
                        self.trace_entity_number_details(number);
                    }
                }
                if self.live_connected() { CommandResult::Unhandled } else { CommandResult::Handled }
            }
            _ => CommandResult::Unhandled,
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
                    .and_then(|sound| sound.info().map(|info| (info, sound.source_rate_summary())))
                    .or_else(|| self.menu_ambience.info().map(|info| (info, "menu rain (SFX volume)".to_owned())));
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

    cmd_chat_translate_status ["chatTranslateStatus", ] ("chatTranslateStatus", "Show chat translation settings, request counts and the last service error.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if words.len() != 1 {
            return CommandResult::Unhandled;
        }
        let settings = &self.chat_translation.settings;
        let target = crate::chat_translation::lang::resolve_target(&settings.target);
        let line = format!(
            "^3TRANSLATION:^7 mode {:?}, {} into {} ({}), display {:?}",
            settings.mode,
            settings.provider.label(),
            crate::chat_translation::lang::name(target),
            target,
            settings.display,
        );
        let status = self.chat_translation.status_line();
        self.push_console_line(line);
        self.push_console_line(format!("^3TRANSLATION:^7 {status}"));
        CommandResult::Handled
    }

}
