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
        });
    }

    pub(in crate::app) fn accumulate_perf_sample(&mut self, stats: &RenderStats) {
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
            "[JKA PERF SAMPLE] label={} fps_avg={:.1} fps_min={:.1} fps_max={:.1} cpu_frame_avg={:.3}ms gpu_frame_avg={} windows={} view=({:.0} {:.0} {:.0}) yaw={:.1} pitch={:.1}",
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
        );
        self.push_console_line(line);
    }
}
