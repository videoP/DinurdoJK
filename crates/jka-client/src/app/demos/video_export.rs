//! Deterministic demo -> editor/video export through an adjacent ffmpeg.exe.
use crate::app::{App, DemoAdvance, Duration, GameSession, Instant, RenderCommand, SessionPhase};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
};

pub(in crate::app) const DEMO_EXPORT_DEFAULT_FPS: u32 = 60;
const AUDIO_RATE: u64 = 48_000;
const EXPORT_FPS_CHOICES: [u32; 4] = [24, 30, 60, 120];

/// Export presets deliberately describe the *encoded* deliverable, not pretend
/// camera formats. The renderer currently hands the exporter an 8-bit sRGB-like
/// final presentation surface, so Log/RAW/HDR labels would be misleading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum DemoVideoPreset {
    H264High,
    H264Max,
    ProRes422,
    ProRes444,
}

impl DemoVideoPreset {
    pub(in crate::app) const DEFAULT: Self = Self::H264High;

    pub(in crate::app) fn next(self) -> Self {
        match self {
            Self::H264High => Self::H264Max,
            Self::H264Max => Self::ProRes422,
            Self::ProRes422 => Self::ProRes444,
            Self::ProRes444 => Self::H264High,
        }
    }

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::H264High => "H264 HIGH",
            Self::H264Max => "H264 MAX",
            Self::ProRes422 => "PRORES 422",
            Self::ProRes444 => "PRORES 444",
        }
    }

    pub(in crate::app) fn short_description(self) -> &'static str {
        match self {
            Self::H264High => "H.264 CRF 18, 4:2:0: compact high-quality delivery",
            Self::H264Max => "H.264 CRF 12, 4:2:0: larger near-transparent delivery",
            Self::ProRes422 => "ProRes 422 HQ, 10-bit container: edit-friendly master",
            Self::ProRes444 => "ProRes 4444, 4:4:4: best chroma retention; source is still 8-bit SDR",
        }
    }

    pub(in crate::app) fn extension(self) -> &'static str {
        match self {
            Self::H264High | Self::H264Max => "mp4",
            Self::ProRes422 | Self::ProRes444 => "mov",
        }
    }

    pub(in crate::app) fn ffmpeg_video_args(self) -> &'static [&'static str] {
        match self {
            Self::H264High => &[
                "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p",
                "-x264-params", "colorprim=bt709:transfer=iec61966-2-1:colormatrix=bt709:fullrange=off",
                "-movflags", "+faststart",
            ],
            Self::H264Max => &[
                "-c:v", "libx264", "-preset", "slow", "-crf", "12", "-pix_fmt", "yuv420p",
                "-x264-params", "colorprim=bt709:transfer=iec61966-2-1:colormatrix=bt709:fullrange=off",
                "-movflags", "+faststart",
            ],
            Self::ProRes422 => &[
                "-c:v", "prores_ks", "-profile:v", "3", "-pix_fmt", "yuv422p10le",
            ],
            Self::ProRes444 => &[
                "-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuv444p10le",
            ],
        }
    }

    fn ffmpeg_audio_args(self) -> &'static [&'static str] {
        match self {
            Self::H264High | Self::H264Max => {
                &["-c:a", "aac", "-b:a", "256k", "-shortest", "-movflags", "+faststart"]
            }
            Self::ProRes422 | Self::ProRes444 => {
                &["-c:a", "pcm_s24le", "-shortest"]
            }
        }
    }
}

pub(in crate::app) struct DemoVideoExport {
    ffmpeg: PathBuf,
    output: PathBuf,
    temp_video: PathBuf,
    temp_audio: PathBuf,
    preset: DemoVideoPreset,
    fps: u32,
    depth: bool,
    depth_dir: Option<PathBuf>,
    depth_child: Option<Child>,
    depth_stdin: Option<ChildStdin>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    audio: Option<rodio::mixer::MixerSource>,
    audio_file: Option<File>,
    audio_frames_written: u64,
    audio_frames_target: u64,
    pub(in crate::app) in_ms: i32,
    pub(in crate::app) frame_index: u64,
    pub(in crate::app) total_frames: u64,
    pub(in crate::app) awaiting_frame: bool,
    pub(in crate::app) capture_queued: bool,
}

impl Drop for DemoVideoExport {
    fn drop(&mut self) {
        self.stdin.take();
        self.depth_stdin.take();
        if let Some(mut child) = self.depth_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(in crate::app) fn adjacent_ffmpeg_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("DJK_STUDIO_TEST_FFMPEG").map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join("ffmpeg.exe");
    path.is_file().then_some(path)
}

fn safe_stem(source: &str) -> String {
    let stem = Path::new(source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("demo");
    let filtered: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' })
        .collect();
    if filtered.is_empty() { "demo".into() } else { filtered }
}

impl App {
    pub(in crate::app) fn demo_export_available(&self) -> bool {
        adjacent_ffmpeg_path().is_some()
    }

    pub(in crate::app) fn cycle_demo_export_preset(&mut self) {
        self.demo_export_preset = self.demo_export_preset.next();
        self.console_status = format!(
            "DEMO EXPORT QUALITY: {}: {}",
            self.demo_export_preset.label(),
            self.demo_export_preset.short_description()
        );
        self.publish_transient_ui();
    }

    pub(in crate::app) fn toggle_demo_export_depth(&mut self) {
        self.demo_export_depth = !self.demo_export_depth;
        self.console_status = if self.demo_export_depth {
            "DEMO EXPORT DEPTH: ON: synchronized 32-bit float EXR sequence".into()
        } else {
            "DEMO EXPORT DEPTH: OFF".into()
        };
        self.publish_transient_ui();
    }

    pub(in crate::app) fn cycle_demo_export_fps(&mut self) {
        let current = EXPORT_FPS_CHOICES
            .iter()
            .position(|fps| *fps == self.demo_export_fps)
            .unwrap_or(2);
        self.demo_export_fps = EXPORT_FPS_CHOICES[(current + 1) % EXPORT_FPS_CHOICES.len()];
        self.console_status = format!("DEMO EXPORT FPS: {}", self.demo_export_fps);
        self.publish_transient_ui();
    }

    pub(in crate::app) fn mark_demo_export_in(&mut self) {
        let Some(ms) = self.current_demo_elapsed_ms(Instant::now()) else { return };
        let ms = ms.round() as i32;
        self.demo_export_in_ms = Some(ms);
        if self.demo_export_out_ms.is_some_and(|out| out <= ms) {
            self.demo_export_out_ms = None;
        }
        self.console_status = format!("DEMO EXPORT IN: {:.3}s", ms as f64 / 1000.0);
        self.publish_transient_ui();
    }

    pub(in crate::app) fn mark_demo_export_out(&mut self) {
        let Some(ms) = self.current_demo_elapsed_ms(Instant::now()) else { return };
        let ms = ms.round() as i32;
        if self.demo_export_in_ms.is_some_and(|start| ms <= start) {
            self.console_status = "DEMO EXPORT OUT MUST BE AFTER IN".into();
            return;
        }
        self.demo_export_out_ms = Some(ms);
        self.console_status = format!("DEMO EXPORT OUT: {:.3}s", ms as f64 / 1000.0);
        self.publish_transient_ui();
    }

    pub(in crate::app) fn start_demo_video_export(&mut self) {
        if self.demo_video_export.is_some() {
            return;
        }
        let Some(ffmpeg) = adjacent_ffmpeg_path() else {
            self.console_status = "DEMO EXPORT DISABLED: ffmpeg.exe must be next to DinurdoJK.exe".into();
            self.push_console_line(format!("^3{}", self.console_status));
            return;
        };
        let (Some(in_ms), Some(out_ms)) = (self.demo_export_in_ms, self.demo_export_out_ms) else {
            self.console_status = "DEMO EXPORT: MARK IN AND OUT FIRST".into();
            return;
        };
        if out_ms <= in_ms {
            self.console_status = "DEMO EXPORT OUT MUST BE AFTER IN".into();
            return;
        }
        let Some(session) = self.game_session.as_ref().filter(|s| !s.live && s.phase == SessionPhase::Playing) else {
            self.console_status = "DEMO EXPORT REQUIRES ACTIVE DEMO PLAYBACK".into();
            return;
        };
        let source = safe_stem(&session.source);
        let duration_ms = out_ms - in_ms;
        let fps = self.demo_export_fps.max(1);
        let preset = self.demo_export_preset;
        let total_frames = ((duration_ms as u64 * fps as u64) + 999) / 1000;
        if total_frames == 0 {
            self.console_status = "DEMO EXPORT RANGE IS EMPTY".into();
            return;
        }

        let dir = self.game_write_path("videos");
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.console_status = format!("DEMO EXPORT: could not create {}: {error}", dir.display());
            return;
        }
        let stamp = crate::screenshot::unix_time_ms();
        let extension = preset.extension();
        let output = dir.join(format!("{source}_{stamp}.{extension}"));
        let temp_video = dir.join(format!(".{source}_{stamp}.video.{extension}"));
        let temp_audio = dir.join(format!(".{source}_{stamp}.audio.f32le"));
        let depth_dir = self
            .demo_export_depth
            .then(|| dir.join(format!("{source}_{stamp}_depth")));
        if let Some(depth_dir) = depth_dir.as_ref() {
            if let Err(error) = std::fs::create_dir_all(depth_dir) {
                self.console_status = format!(
                    "DEMO EXPORT: could not create depth directory {}: {error}",
                    depth_dir.display()
                );
                return;
            }
            let note = concat!(
                "DinurdoJK synchronized depth pass\n",
                "Files: depth_000001.exr ... one per exported color frame\n",
                "Channel: Y, 32-bit float\n",
                "Meaning: radial linear distance from the render camera in native JKA world units\n",
                "Background/clear value: 1000000\n",
                "Use the same frame rate as the sibling video; values are linear numeric depth, not display color.\n"
            );
            let _ = std::fs::write(depth_dir.join("README.txt"), note);
        }
        let audio_file = match File::create(&temp_audio) {
            Ok(file) => file,
            Err(error) => {
                self.console_status = format!("DEMO EXPORT: could not create audio temp file: {error}");
                if let Some(depth_dir) = depth_dir.as_ref() {
                    let _ = std::fs::remove_dir_all(depth_dir);
                }
                return;
            }
        };

        let now = Instant::now();
        let fraction = self.game_session.as_ref()
            .and_then(|s| s.demo_duration_ms())
            .filter(|d| *d > 0)
            .map_or(0.0, |d| in_ms as f32 / d as f32);
        if let Err(error) = self.seek_demo_scrub_fraction(fraction, now) {
            self.console_status = format!("DEMO EXPORT SEEK FAILED: {error}");
            let _ = std::fs::remove_file(&temp_audio);
            if let Some(depth_dir) = depth_dir.as_ref() {
                let _ = std::fs::remove_dir_all(depth_dir);
            }
            return;
        }

        let audio = self.game_session.as_mut().and_then(|session| {
            session.set_audio_suppressed(false);
            if let Some(timeline) = session.timeline.as_mut() {
                timeline.set_rate(now, 0.0);
            }
            let GameSession { sound_presenter, client_game, .. } = session;
            let sound = sound_presenter.as_mut()?;
            match sound.begin_offline_capture(client_game) {
                Ok(source) => Some(source),
                Err(error) => {
                    eprintln!("DEMO EXPORT AUDIO DISABLED: {error}");
                    None
                }
            }
        });

        self.demo_video_export = Some(DemoVideoExport {
            ffmpeg,
            output: output.clone(),
            temp_video,
            temp_audio,
            preset,
            fps,
            depth: self.demo_export_depth,
            depth_dir,
            depth_child: None,
            depth_stdin: None,
            child: None,
            stdin: None,
            audio,
            audio_file: Some(audio_file),
            audio_frames_written: 0,
            audio_frames_target: ((duration_ms as u64 * AUDIO_RATE) + 500) / 1000,
            in_ms,
            frame_index: 0,
            total_frames,
            awaiting_frame: false,
            capture_queued: false,
        });
        self.demo_scrub_fraction = None;
        self.demo_scrub_dragging = false;
        self.console_status = format!(
            "EXPORTING {total_frames} FRAMES @ {fps} FPS · {}{} · ESC TO CANCEL",
            preset.label(),
            if self.demo_export_depth { " · DEPTH EXR" } else { "" }
        );
        self.push_console_line(format!("^2DEMO EXPORT:^7 {}", output.display()));
        self.publish_transient_ui();
    }

    fn discard_demo_video_export(&mut self) -> bool {
        let Some(mut export) = self.demo_video_export.take() else { return false };
        export.stdin.take();
        export.depth_stdin.take();
        if let Some(mut child) = export.depth_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = export.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(session) = self.game_session.as_mut() {
            let GameSession { sound_presenter, client_game, .. } = session;
            if let Some(sound) = sound_presenter.as_mut() {
                sound.end_offline_capture(client_game);
            }
        }
        let output = export.output.clone();
        let temp_video = export.temp_video.clone();
        let temp_audio = export.temp_audio.clone();
        let depth_dir = export.depth_dir.clone();
        drop(export);
        let _ = std::fs::remove_file(output);
        let _ = std::fs::remove_file(temp_video);
        let _ = std::fs::remove_file(temp_audio);
        if let Some(depth_dir) = depth_dir {
            let _ = std::fs::remove_dir_all(depth_dir);
        }
        true
    }

    pub(in crate::app) fn cancel_demo_video_export(&mut self) {
        if !self.discard_demo_video_export() {
            return;
        }
        self.console_status = "DEMO EXPORT CANCELLED".into();
        self.push_console_line(format!("^3{}", self.console_status));
        self.publish_transient_ui();
    }

    pub(in crate::app) fn fail_demo_video_export(&mut self, error: impl std::fmt::Display) {
        let active = self.discard_demo_video_export();
        if !active {
            return;
        }
        self.console_status = format!("DEMO EXPORT FAILED: {error}");
        self.push_console_line(format!("^1{}", self.console_status));
        self.publish_transient_ui();
    }

    /// Advance exactly one movie frame. The renderer capture acknowledgement gates
    /// the next call, so encode speed can never drop or duplicate demo frames.
    pub(in crate::app) fn tick_demo_video_export(&mut self, now: Instant) -> Result<DemoAdvance, String> {
        let (frame_index, in_ms, fps, awaiting) = {
            let export = self.demo_video_export.as_ref().ok_or("DEMO EXPORT STATE DISAPPEARED")?;
            (export.frame_index, export.in_ms, export.fps, export.awaiting_frame)
        };
        if awaiting {
            return Ok(DemoAdvance::Running);
        }
        let target_elapsed = in_ms as f64 + frame_index as f64 * (1000.0 / fps as f64);
        let first = self.game_session.as_ref()
            .and_then(|s| s.demo_index.as_ref())
            .and_then(|i| i.first_active_server_time)
            .ok_or("DEMO EXPORT TIMELINE IS NOT READY")?;
        if let Some(session) = self.game_session.as_mut() {
            if let Some(timeline) = session.timeline.as_mut() {
                timeline.seek(now, first as f64 + target_elapsed);
                timeline.rate = 0.0;
            }
        }
        let advance = self.tick_demo_playback(now, Duration::from_secs_f64(1.0 / fps as f64))?;

        if let Some(export) = self.demo_video_export.as_mut() {
            if let (Some(source), Some(file)) = (export.audio.as_mut(), export.audio_file.as_mut()) {
                let cumulative = (((frame_index + 1) * AUDIO_RATE) + (fps as u64 / 2)) / fps as u64;
                let target = cumulative.min(export.audio_frames_target);
                let frames = target.saturating_sub(export.audio_frames_written);
                for _ in 0..frames * 2 {
                    let sample = source.next().unwrap_or(0.0_f32);
                    file.write_all(&sample.to_le_bytes()).map_err(|e| format!("DEMO EXPORT AUDIO WRITE FAILED: {e}"))?;
                }
                export.audio_frames_written = target;
            }
            export.awaiting_frame = true;
            export.capture_queued = false;
        }
        Ok(advance)
    }

    pub(in crate::app) fn queue_demo_video_capture(&mut self) {
        let frame = self.demo_video_export.as_ref().and_then(|export| {
            (export.awaiting_frame && !export.capture_queued)
                .then_some((export.frame_index, export.depth))
        });
        if let Some((frame_index, depth)) = frame {
            self.render_command(RenderCommand::CaptureVideoFrame { frame_index, depth, alpha: false });
            if let Some(export) = self.demo_video_export.as_mut() {
                export.capture_queued = true;
            }
        }
    }

    pub(in crate::app) fn on_demo_video_frame(
        &mut self,
        frame_index: u64,
        width: u32,
        height: u32,
        bgra: Vec<u8>,
        depth_f32le: Option<Vec<u8>>,
    ) {
        let mut depth_warning = None;
        let result = (|| -> Result<bool, String> {
            let export = self.demo_video_export.as_mut().ok_or("DEMO EXPORT IS NOT ACTIVE")?;
            if !export.awaiting_frame || frame_index != export.frame_index {
                return Err(format!("unexpected demo export frame {frame_index}, expected {}", export.frame_index));
            }
            if export.child.is_none() {
                let mut command = Command::new(&export.ffmpeg);
                command
                    .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgra"])
                    .arg("-s").arg(format!("{width}x{height}"))
                    .arg("-r").arg(export.fps.to_string())
                    .args(["-i", "pipe:0", "-an"])
                    // Final presentation readback is standard SDR RGB. Force the
                    // RGB->YUV conversion matrix/range instead of relying on
                    // resolution-dependent swscale guesses, and tag the result.
                    .args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2,scale=in_range=full:out_range=tv:out_color_matrix=bt709,setparams=range=limited:color_primaries=bt709:color_trc=iec61966-2-1:colorspace=bt709"])
                    .args(["-color_range", "tv", "-color_primaries", "bt709", "-color_trc", "iec61966-2-1", "-colorspace", "bt709"])
                    .args(export.preset.ffmpeg_video_args())
                    .arg(&export.temp_video)
                    .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
                let mut child = command
                    .spawn().map_err(|e| format!("could not start {}: {e}", export.ffmpeg.display()))?;
                export.stdin = child.stdin.take();
                export.child = Some(child);
            }
            let write_result = export
                .stdin
                .as_mut()
                .ok_or("ffmpeg stdin is unavailable")?
                .write_all(&bgra);
            if let Err(error) = write_result {
                // A Windows broken/closing pipe only tells us that FFmpeg stopped
                // reading. Close stdin and collect the real encoder diagnostic.
                export.stdin.take();
                let detail = export
                    .child
                    .take()
                    .and_then(|child| child.wait_with_output().ok())
                    .map(|output| String::from_utf8_lossy(&output.stderr).trim().to_owned())
                    .filter(|text| !text.is_empty());
                return Err(match detail {
                    Some(detail) => format!(
                        "ffmpeg video encoder exited while writing {} frame {} ({}x{} @ {} FPS): {detail}",
                        export.preset.label(),
                        frame_index + 1,
                        width,
                        height,
                        export.fps
                    ),
                    None => format!(
                        "ffmpeg video pipe failed while writing {} frame {} ({}x{} @ {} FPS): {error}",
                        export.preset.label(),
                        frame_index + 1,
                        width,
                        height,
                        export.fps
                    ),
                });
            }

            if export.depth {
                if let Some(depth) = depth_f32le.as_deref() {
                    if export.depth_child.is_none() {
                        let depth_dir = export.depth_dir.as_ref().ok_or(
                            "depth export directory disappeared",
                        )?;
                        let pattern = depth_dir.join("depth_%06d.exr");
                        // Keep the EXR command deliberately conservative. grayf32le
                        // already requests full 32-bit float samples; -format float
                        // was redundant, and old/prebuilt Windows FFmpeg packages do
                        // not all expose the same EXR private options. Explicit image2
                        // also avoids relying on output-format probing for a sequence.
                        let spawn = Command::new(&export.ffmpeg)
                            .args([
                                "-hide_banner", "-loglevel", "error", "-y",
                                "-f", "rawvideo", "-pix_fmt", "grayf32le",
                            ])
                            .arg("-s").arg(format!("{width}x{height}"))
                            .arg("-r").arg(export.fps.to_string())
                            .args(["-i", "pipe:0", "-an", "-c:v", "exr", "-f", "image2"])
                            .arg(pattern)
                            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped())
                            .spawn();
                        match spawn {
                            Ok(mut child) => {
                                export.depth_stdin = child.stdin.take();
                                export.depth_child = Some(child);
                            }
                            Err(error) => {
                                depth_warning = Some(format!(
                                    "DEPTH EXR DISABLED: could not start ffmpeg depth encoder: {error}"
                                ));
                                export.depth = false;
                            }
                        }
                    }

                    if export.depth {
                        let write_result = export
                            .depth_stdin
                            .as_mut()
                            .ok_or_else(|| std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "ffmpeg depth stdin is unavailable",
                            ))
                            .and_then(|stdin| stdin.write_all(depth));
                        if let Err(error) = write_result {
                            // Close stdin first, then collect the *actual* FFmpeg
                            // diagnostic. A broken pipe alone (Windows os error 109)
                            // tells us only that FFmpeg exited before Rust wrote.
                            export.depth_stdin.take();
                            let detail = export.depth_child.take()
                                .and_then(|child| child.wait_with_output().ok())
                                .map(|output| String::from_utf8_lossy(&output.stderr).trim().to_owned())
                                .filter(|text| !text.is_empty());
                            depth_warning = Some(match detail {
                                Some(detail) => format!("DEPTH EXR DISABLED: {detail}"),
                                None => format!("DEPTH EXR DISABLED: ffmpeg depth pipe failed: {error}"),
                            });
                            export.depth = false;
                        }
                    }
                } else {
                    depth_warning = Some(
                        "DEPTH EXR DISABLED: renderer did not return the requested linear-depth frame".into(),
                    );
                    export.depth = false;
                }

                if !export.depth {
                    export.depth_stdin.take();
                    if let Some(mut child) = export.depth_child.take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    if let Some(depth_dir) = export.depth_dir.take() {
                        let _ = std::fs::remove_dir_all(depth_dir);
                    }
                }
            }
            export.frame_index += 1;
            export.awaiting_frame = false;
            export.capture_queued = false;
            Ok(export.frame_index >= export.total_frames)
        })();
        if let Some(warning) = depth_warning {
            self.push_console_line(format!("^3DEMO EXPORT:^7 {warning}"));
        }
        match result {
            Ok(true) => self.finish_demo_video_export(),
            Ok(false) => {}
            Err(error) => self.fail_demo_video_export(error),
        }
    }

    fn finish_demo_video_export(&mut self) {
        let Some(mut export) = self.demo_video_export.take() else { return };
        export.stdin.take();
        export.depth_stdin.take();
        let (video_ok, video_error) = match export.child.take() {
            Some(child) => match child.wait_with_output() {
                Ok(output) if output.status.success() => (true, None),
                Ok(output) => {
                    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    (
                        false,
                        Some(if detail.is_empty() {
                            format!(
                                "ffmpeg video encoder returned {} for {}",
                                output.status,
                                export.preset.label()
                            )
                        } else {
                            format!("{}: {detail}", export.preset.label())
                        }),
                    )
                }
                Err(error) => (
                    false,
                    Some(format!(
                        "could not finish ffmpeg video encoder for {}: {error}",
                        export.preset.label()
                    )),
                ),
            },
            None => (false, Some("ffmpeg video encoder disappeared".into())),
        };

        // Depth is a sidecar deliverable. If its encoder fails at EOF/finalize,
        // report and discard only the depth sequence; never throw away a valid
        // color movie because an optional EXR pass was unavailable.
        let mut depth_warning = None;
        if export.depth {
            let depth_ok = export.depth_child.take().is_some_and(|child| {
                match child.wait_with_output() {
                    Ok(output) if output.status.success() => true,
                    Ok(output) => {
                        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                        depth_warning = Some(if detail.is_empty() {
                            "DEPTH EXR DISABLED: ffmpeg depth encoder returned an error".into()
                        } else {
                            format!("DEPTH EXR DISABLED: {detail}")
                        });
                        false
                    }
                    Err(error) => {
                        depth_warning = Some(format!(
                            "DEPTH EXR DISABLED: could not finish ffmpeg depth encoder: {error}"
                        ));
                        false
                    }
                }
            });
            if !depth_ok {
                export.depth = false;
                if let Some(depth_dir) = export.depth_dir.take() {
                    let _ = std::fs::remove_dir_all(depth_dir);
                }
            }
        } else {
            export.depth_child = None;
        }

        export.audio_file.take();
        if let Some(session) = self.game_session.as_mut() {
            let GameSession { sound_presenter, client_game, .. } = session;
            if let Some(sound) = sound_presenter.as_mut() {
                sound.end_offline_capture(client_game);
            }
        }
        let had_audio = export.audio.is_some();
        let mut mux_error = None;
        let mux_ok = if video_ok && had_audio {
            let mut command = Command::new(&export.ffmpeg);
            command
                .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                .arg(&export.temp_video)
                .args(["-f", "f32le", "-ar", "48000", "-ac", "2", "-i"])
                .arg(&export.temp_audio)
                .args(["-c:v", "copy"])
                .args(export.preset.ffmpeg_audio_args())
                .arg(&export.output)
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
            match command.output() {
                Ok(output) if output.status.success() => true,
                Ok(output) => {
                    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    mux_error = Some(if detail.is_empty() {
                        format!("ffmpeg audio mux returned {}", output.status)
                    } else {
                        format!("ffmpeg audio mux: {detail}")
                    });
                    false
                }
                Err(error) => {
                    mux_error = Some(format!("could not start ffmpeg audio mux: {error}"));
                    false
                }
            }
        } else if video_ok {
            match std::fs::rename(&export.temp_video, &export.output) {
                Ok(()) => true,
                Err(error) => {
                    mux_error = Some(format!("could not move completed video into place: {error}"));
                    false
                }
            }
        } else {
            false
        };
        let output = export.output.clone();
        let temp_video = export.temp_video.clone();
        let temp_audio = export.temp_audio.clone();
        let depth_dir = export.depth_dir.clone();
        drop(export);
        let _ = std::fs::remove_file(temp_video);
        let _ = std::fs::remove_file(temp_audio);
        if let Some(warning) = depth_warning {
            self.push_console_line(format!("^3DEMO EXPORT:^7 {warning}"));
        }
        if mux_ok {
            self.console_status = if let Some(depth_dir) = depth_dir.as_ref() {
                format!("WROTE {} + DEPTH {}", output.display(), depth_dir.display())
            } else {
                format!("WROTE {}", output.display())
            };
            self.push_console_path_line(format!("^2{}", self.console_status), output);
        } else {
            let _ = std::fs::remove_file(&output);
            if let Some(depth_dir) = depth_dir {
                let _ = std::fs::remove_dir_all(depth_dir);
            }
            let detail = video_error.or(mux_error).unwrap_or_else(|| "ffmpeg returned an error".into());
            self.console_status = format!("DEMO EXPORT FAILED: {detail}");
            self.push_console_line(format!("^1{}", self.console_status));
        }
        self.publish_transient_ui();
    }
}
