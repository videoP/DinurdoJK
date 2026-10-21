//! Performance.
use crate::app::{App, Duration, Instant, PerfSample, RenderStats};

impl App {
    pub(in crate::app) fn begin_perf_sample(&mut self, arguments: &[&str]) {
        let seconds = arguments
            .first()
            .and_then(|text| text.parse::<f64>().ok())
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
        let Some(seconds) = seconds else {
            self.push_console_line("usage: perfsample <seconds> [label]".to_owned());
            return;
        };
        let label = if arguments.len() > 1 {
            arguments[1..].join(" ")
        } else {
            "sample".to_owned()
        };
        self.perf_sample = Some(PerfSample {
            label,
            started: Instant::now(),
            duration: Duration::from_secs_f64(seconds),
            skipped_first: false,
            fps: Vec::new(),
            frame_ms: Vec::new(),
            gpu_ms: Vec::new(),
            world_program_binds: (0, 0),
            world_shader_programs: Default::default(),
            ready_mode: None,
        });
    }

    /// `waitshadermode <mode> [timeout=120]`: hold the command buffer until the
    /// renderer reports `mode` both requested and active with no world variant
    /// still compiling, so scripted measurements never sample a compile.
    pub(in crate::app) fn begin_shader_mode_wait(&mut self, arguments: &[&str]) {
        let mode = arguments.first().and_then(|text| text.parse::<u8>().ok()).filter(|mode| *mode <= 4);
        let timeout = arguments.get(1).map_or(Some(120.0), |text| text.parse::<f64>().ok())
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
        let (Some(mode), Some(timeout)) = (mode, timeout) else {
            self.push_console_line("usage: waitshadermode <0-4> [timeout_seconds]".to_owned());
            return;
        };
        self.perf_sample = Some(PerfSample {
            label: format!("mode{mode}"),
            started: Instant::now(),
            duration: Duration::from_secs_f64(timeout),
            skipped_first: false,
            fps: Vec::new(),
            frame_ms: Vec::new(),
            gpu_ms: Vec::new(),
            world_program_binds: (0, 0),
            world_shader_programs: Default::default(),
            ready_mode: Some(mode),
        });
    }

    fn finish_shader_mode_wait(&mut self, stats: &RenderStats) {
        let Some(sample) = self.perf_sample.as_mut() else { return; };
        let Some(mode) = sample.ready_mode else { return; };
        // The first stats window can predate the cvar reaching the renderer.
        if !sample.skipped_first {
            sample.skipped_first = true;
            return;
        }
        let programs = stats.world_shader_programs;
        let ready = programs.requested == mode && programs.active == mode && !programs.pending;
        let elapsed = sample.started.elapsed();
        if !ready && elapsed < sample.duration {
            return;
        }
        self.perf_sample = None;
        self.push_console_line(format!(
            "[JKA SHADER MODE {}] mode={mode} requested={} active={} pending={} waited_ms={:.0} binds[general={} specialized={} minimal={} shared={} generated={}]",
            if ready { "READY" } else { "TIMEOUT" },
            programs.requested, programs.active, programs.pending,
            elapsed.as_secs_f64() * 1000.0,
            programs.binds[0], programs.binds[1], programs.binds[2], programs.binds[3], programs.binds[4],
        ));
    }

    pub(in crate::app) fn accumulate_perf_sample(&mut self, stats: &RenderStats) {
        if self.perf_sample.as_ref().is_some_and(|sample| sample.ready_mode.is_some()) {
            self.finish_shader_mode_wait(stats);
            return;
        }
        let Some(sample) = self.perf_sample.as_mut() else {
            return;
        };
        // The first stats window began before the sample did.
        if !sample.skipped_first {
            sample.skipped_first = true;
            return;
        }
        sample.fps.push(stats.fps);
        sample.frame_ms.push(stats.frame_ms);
        if let Some(gpu) = stats.gpu_ms {
            sample.gpu_ms.push(gpu);
        }
        sample.world_program_binds = stats.world_program_binds;
        sample.world_shader_programs = stats.world_shader_programs;
        if sample.started.elapsed() < sample.duration {
            return;
        }
        let sample = self.perf_sample.take().expect("checked above");
        let mean = |values: &[f64]| {
            (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
        };
        let min = sample.fps.iter().copied().fold(f64::INFINITY, f64::min);
        let max = sample.fps.iter().copied().fold(0.0, f64::max);
        let position = self.camera.position;
        let line = format!(
            "[JKA PERF SAMPLE] label={} fps_avg={:.1} fps_min={:.1} fps_max={:.1} cpu_frame_avg={:.3}ms gpu_frame_avg={} windows={} view=({:.0} {:.0} {:.0}) yaw={:.1} pitch={:.1} world_programs={}spec/{}gen shader_mode={}/{} binds[general={} specialized={} minimal={} shared={} generated={}]",
            sample.label,
            mean(&sample.fps).unwrap_or(0.0),
            min,
            max,
            mean(&sample.frame_ms).unwrap_or(0.0),
            mean(&sample.gpu_ms).map_or_else(|| "--".to_owned(), |ms| format!("{ms:.3}ms")),
            sample.fps.len(),
            position.x,
            -position.z,
            position.y,
            self.camera.yaw.to_degrees().rem_euclid(360.0),
            -self.camera.pitch.to_degrees(),
            sample.world_program_binds.0,
            sample.world_program_binds.1,
            sample.world_shader_programs.requested, sample.world_shader_programs.active,
            sample.world_shader_programs.binds[0], sample.world_shader_programs.binds[1],
            sample.world_shader_programs.binds[2], sample.world_shader_programs.binds[3], sample.world_shader_programs.binds[4],
        );
        self.push_console_line(line);
    }
}
