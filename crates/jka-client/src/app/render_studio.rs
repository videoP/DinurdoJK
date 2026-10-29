//! Offline character capture stage. Uses the same PlayerPresenter, emote poses,
//! GPU capture readback and encoding profiles as the rest of the client.
//! The stage is intentionally isolated: nothing here spawns network entities.
use crate::app::{App, FrontendPage, Instant, RenderCommand};
use crate::app::demos::video_export::{adjacent_ffmpeg_path, DemoVideoPreset};
use std::{io::Write, path::PathBuf, process::{Child, ChildStdin, Command, Stdio}};
use image::ImageEncoder;
use super::render_studio_camera::{StudioCameraRig, StudioCameraRigRuntime};
use super::render_studio_audio::StudioAudioRuntime;
use super::render_studio_timeline::{StudioAnimationClip, StudioTimeline, StudioProject, StudioProjectClip, StudioClipPlayback, StudioMotionKey, StudioCameraKey, StudioInterpolation, StudioItem, StudioDemoClip, StudioDemoDragKind};

/// Render Studio-only formats. The demo exporter deliberately retains its
/// existing preset enum/codec behavior; these additions apply only here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum StudioVideoFormat {
    H264High,
    H264Max,
    /// NVIDIA hardware encoders: fast and light on the CPU the export needs.
    H264Nvenc,
    HevcNvenc,
    ProRes422,
    ProRes444,
    WebmVp9,
    WebmVp9Max,
    Gif,
}

impl StudioVideoFormat {
    pub const OPTIONS: [Self; 9] = [
        Self::H264High, Self::H264Max, Self::H264Nvenc, Self::HevcNvenc, Self::ProRes422,
        Self::ProRes444, Self::WebmVp9, Self::WebmVp9Max, Self::Gif,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::H264High => "H264 HIGH",
            Self::H264Max => "H264 MAX",
            Self::H264Nvenc => "H264 NVENC (GPU)",
            Self::HevcNvenc => "HEVC NVENC (GPU)",
            Self::ProRes422 => "PRORES 422",
            Self::ProRes444 => "PRORES 4444",
            Self::WebmVp9 => "WEBM VP9 HIGH",
            Self::WebmVp9Max => "WEBM VP9 MAX",
            Self::Gif => "ANIMATED GIF",
        }
    }

    pub fn short_description(self) -> &'static str {
        match self {
            Self::H264High => DemoVideoPreset::H264High.short_description(),
            Self::H264Max => DemoVideoPreset::H264Max.short_description(),
            Self::H264Nvenc => "NVIDIA GPU H.264, constant quality 19; fast, light on CPU (needs an NVIDIA GPU)",
            Self::HevcNvenc => "NVIDIA GPU HEVC/H.265, constant quality 21; smaller files (needs an NVIDIA GPU)",
            Self::ProRes422 => DemoVideoPreset::ProRes422.short_description(),
            Self::ProRes444 => DemoVideoPreset::ProRes444.short_description(),
            Self::WebmVp9 => "VP9 CRF 28; compact transparent WebM (optional alpha)",
            Self::WebmVp9Max => "VP9 CRF 18; larger high-quality transparent WebM (optional alpha)",
            Self::Gif => "Looping 256-color GIF; optional 1-bit alpha, best at 10-30 fps",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::H264High | Self::H264Max | Self::H264Nvenc | Self::HevcNvenc => "mp4",
            Self::ProRes422 | Self::ProRes444 => "mov",
            Self::WebmVp9 | Self::WebmVp9Max => "webm",
            Self::Gif => "gif",
        }
    }

    pub fn supports_alpha(self) -> bool {
        matches!(self, Self::ProRes444 | Self::WebmVp9 | Self::WebmVp9Max | Self::Gif)
    }

    fn audio_ffmpeg_args(self)->&'static [&'static str] {
        match self {
            Self::H264High|Self::H264Max|Self::H264Nvenc|Self::HevcNvenc=>&["-c:a","aac","-b:a","192k"],
            Self::WebmVp9|Self::WebmVp9Max=>&["-c:a","libopus","-b:a","192k"],
            Self::ProRes422|Self::ProRes444=>&["-c:a","pcm_s24le"],
            Self::Gif=>&[],
        }
    }

    fn demo_preset(self) -> Option<DemoVideoPreset> {
        match self {
            Self::H264High => Some(DemoVideoPreset::H264High),
            Self::H264Max => Some(DemoVideoPreset::H264Max),
            Self::ProRes422 => Some(DemoVideoPreset::ProRes422),
            Self::ProRes444 => Some(DemoVideoPreset::ProRes444),
            Self::H264Nvenc | Self::HevcNvenc | Self::WebmVp9 | Self::WebmVp9Max | Self::Gif => None,
        }
    }

    /// FFmpeg encoder of the NVENC formats.
    pub fn nvenc_codec(self) -> Option<&'static str> {
        match self {
            Self::H264Nvenc => Some("h264_nvenc"),
            Self::HevcNvenc => Some("hevc_nvenc"),
            _ => None,
        }
    }
}

/// Default NVENC preset (p1 fastest .. p7 slowest).
pub(in crate::app) const STUDIO_NVENC_DEFAULT_PRESET: u8 = 5;

/// Encode one tiny frame with `codec` to check the FFmpeg build and GPU
/// support it before an export starts (instead of failing mid-pipe).
fn studio_probe_encoder(ffmpeg: &std::path::Path, codec: &str) -> Result<(), String> {
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=256x256:d=0.1",
        "-frames:v", "1", "-c:v", codec, "-f", "null", "-"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    let output = command.output().map_err(|error| format!("FFmpeg: {error}"))?;
    if output.status.success() { return Ok(()); }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // FFmpeg reports the cause first (e.g. a driver too old for its NVENC
    // API); the last lines are only the generic "no packets" fallout.
    let cause = stderr.lines().map(str::trim).filter(|line| !line.is_empty()).take(2)
        .map(|line| line.split_once("] ").map_or(line, |(_, message)| message))
        .collect::<Vec<_>>().join(" ");
    Err(if cause.is_empty() { "encoder test failed".into() } else { cause })
}

/// Pixel size Studio world frames are laid out for (camera aspect, HUD,
/// trails): the fixed capture size while one is active, else the viewer.
pub(in crate::app) fn studio_world_frame_size(capture: Option<[u32;2]>, viewport: Option<[u32;4]>) -> [u32;2] {
    capture.unwrap_or_else(|| {
        let v = viewport.unwrap_or([0,0,1280,720]);
        [v[2].max(1), v[3].max(1)]
    })
}

/// x264 speed preset for the H.264 formats. CRF (quality) is unchanged:
/// faster presets encode quicker and give larger files at that quality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum StudioEncoderSpeed {
    /// The format's own preset (High: medium, Max: slow).
    #[default]
    FormatDefault,
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
    Veryslow,
}
impl StudioEncoderSpeed {
    pub const OPTIONS: [Self; 10] = [Self::FormatDefault, Self::Ultrafast, Self::Superfast,
        Self::Veryfast, Self::Faster, Self::Fast, Self::Medium, Self::Slow, Self::Slower, Self::Veryslow];
    /// The x264 `-preset` value; `None` keeps the format's default.
    pub fn x264(self) -> Option<&'static str> {
        match self {
            Self::FormatDefault => None,
            Self::Ultrafast => Some("ultrafast"),
            Self::Superfast => Some("superfast"),
            Self::Veryfast => Some("veryfast"),
            Self::Faster => Some("faster"),
            Self::Fast => Some("fast"),
            Self::Medium => Some("medium"),
            Self::Slow => Some("slow"),
            Self::Slower => Some("slower"),
            Self::Veryslow => Some("veryslow"),
        }
    }
}

/// x264 CRF the Studio quality slider starts at (the H264 HIGH level).
pub(in crate::app) const STUDIO_DEFAULT_CRF: u8 = 18;

/// Named levels for the quality slider (lower CRF = better, larger).
pub(in crate::app) fn studio_crf_level(crf: u8) -> &'static str {
    match crf {
        0..=13 => "Max",
        14..=20 => "High",
        21..=25 => "Medium",
        _ => "Low",
    }
}

/// FFmpeg video arguments of an opaque H.264/ProRes export: the user's
/// quality (x264 CRF, or the matching NVENC CQ) and encoder speed applied.
fn studio_video_args(format: StudioVideoFormat, speed: StudioEncoderSpeed, nvenc_preset: u8, crf: u8) -> Vec<String> {
    let crf = crf.min(51);
    if let Some(codec) = format.nvenc_codec() {
        const PRESETS: [&str; 7] = ["p1", "p2", "p3", "p4", "p5", "p6", "p7"];
        let preset = PRESETS[usize::from(if nvenc_preset == 0 { STUDIO_NVENC_DEFAULT_PRESET } else { nvenc_preset }.clamp(1, 7)) - 1];
        // NVENC CQ runs a little lower in quality than x264 CRF at the same
        // number, and HEVC needs fewer bits: CQ 19 / 21 for CRF 18.
        let cq = (if format == StudioVideoFormat::HevcNvenc { crf + 3 } else { crf + 1 }).min(51);
        let mut args = vec!["-c:v", codec, "-preset", preset, "-tune", "hq", "-rc", "vbr", "-b:v", "0"]
            .into_iter().map(String::from).collect::<Vec<_>>();
        args.extend(["-cq".to_owned(), cq.to_string()]);
        // hvc1 tag for QuickTime.
        args.extend((if format == StudioVideoFormat::HevcNvenc {
            ["-profile:v", "main", "-tag:v", "hvc1"]
        } else {
            ["-profile:v", "high", "-tag:v", "avc1"]
        }).map(String::from));
        args.extend(["-pix_fmt", "yuv420p", "-movflags", "+faststart"].map(String::from));
        return args;
    }
    let mut args: Vec<String> = format.demo_preset().expect("Non-demo formats handled above")
        .ffmpeg_video_args().iter().map(|arg| (*arg).to_owned()).collect();
    if let (Some(speed), Some(index)) = (speed.x264(), args.iter().position(|arg| arg == "-preset")) {
        if let Some(value) = args.get_mut(index + 1) { *value = speed.to_owned(); }
    }
    if matches!(format, StudioVideoFormat::H264High | StudioVideoFormat::H264Max) {
        if let Some(value) = args.iter().position(|arg| arg == "-crf").and_then(|index| args.get_mut(index + 1)) {
            *value = crf.to_string();
        }
    }
    args
}

/// Output resolution of demo/BSP exports. Fixed sizes render offscreen,
/// independent of the window, its mode and the editor viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum StudioExportSize {
    /// The program viewer's own pixels (legacy window capture).
    Viewport,
    Hd720,
    #[default]
    Hd1080,
    Qhd1440,
    Uhd2160,
}
impl StudioExportSize {
    pub const OPTIONS: [Self; 5] = [Self::Viewport, Self::Hd720, Self::Hd1080, Self::Qhd1440, Self::Uhd2160];
    pub fn label(self) -> &'static str {
        match self {
            Self::Viewport => "Viewer size",
            Self::Hd720 => "1280×720",
            Self::Hd1080 => "1920×1080",
            Self::Qhd1440 => "2560×1440",
            Self::Uhd2160 => "3840×2160",
        }
    }
    pub fn dimensions(self) -> Option<[u32; 2]> {
        match self {
            Self::Viewport => None,
            Self::Hd720 => Some([1280, 720]),
            Self::Hd1080 => Some([1920, 1080]),
            Self::Qhd1440 => Some([2560, 1440]),
            Self::Uhd2160 => Some([3840, 2160]),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum StudioBackground {
    Black,
    Green,
    Blue,
    Levelshot,
    Bsp,
}

/// Real BSP background for model-only Studio frames. Its map is prepared by
/// the same async map loader and uploaded into a standalone Studio renderer.
/// Only the qpath is serialized; GPU/cache/request state never enters projects.
pub(in crate::app) struct StudioBspBackdrop {
    pub id: u64,
    pub map: String,
    pub ready: bool,
    pub progress: std::sync::Arc<crate::runtime::LoadProgress>,
    pub prepared: Option<std::sync::Arc<crate::scene::PreparedMap>>,
    pub inline_models: std::sync::Arc<Vec<crate::renderer::InlineModelInstance>>,
    pub error: Option<String>,
}

/// An export whose encoder drain and audio mux run off the main thread.
pub(in crate::app) struct StudioExportFinishing {
    worker: Option<std::thread::JoinHandle<Option<StudioExportFinished>>>,
    /// Only when the worker could not be started: finished on the next poll.
    inline: Option<RenderStudioExport>,
    profile: super::render_studio_profile::StudioExportProfile,
    settings: String,
    output: PathBuf,
}

struct StudioExportFinished {
    error: Option<String>,
    encoder_flush_ms: f64,
    mux_ms: f64,
    depth_dir: Option<PathBuf>,
}

/// Close the encoder pipes, wait for FFmpeg, then mux the captured native
/// audio. Dropping `export` at the end removes the intermediate video/WAV.
fn finish_studio_export_blocking(mut export: RenderStudioExport) -> StudioExportFinished {
    let flush_started = Instant::now();
    // Drain the writer threads (EOF to FFmpeg) before waiting on FFmpeg.
    let writers = export.stdin.take().map_or(Ok(()), |mut w| w.finish())
        .map_err(|error| format!("FFmpeg pipe: {error}"))
        .and(export.depth_stdin.take().map_or(Ok(()), |mut w| w.finish())
            .map_err(|error| format!("Depth pipe: {error}")));
    let audio_flush=writers.and(if let Some(mut file)=export.native_audio_file.take() {
        file.flush().map_err(|error|format!("Audio WAV flush failed: {error}"))
    }else{Ok(())});
    let result = match export.child.take() {
        Some(child) => child.wait_with_output(),
        None => Err(std::io::Error::other("studio FFmpeg was never started")),
    };
    let depth_result = export.depth_child.take().map(|child| child.wait_with_output());
    let encoder_flush_ms = flush_started.elapsed().as_secs_f64()*1000.0;
    let mux_started = Instant::now();
    let output = export.output.clone();
    // Two-pass mux is essential: native audio is only known after the
    // same demo frames were evaluated. FFmpeg must not read an unfinished WAV.
    let mux_result=if let Err(error)=audio_flush {Err(error)}
    else if result.as_ref().is_ok_and(|r|r.status.success()) && export.native_audio {
        (||->Result<(),String> {
            let video=export.encoded_video.as_ref().ok_or("Encoded video was lost")?;
            let wav=export.audio_wav.as_ref().ok_or("Captured audio WAV was lost")?;
            let ffmpeg=adjacent_ffmpeg_path().ok_or("ffmpeg.exe missing at mux stage")?;
            let mut command=Command::new(ffmpeg);
            command.args(["-hide_banner","-loglevel","error","-y"])
                .arg("-i").arg(video).arg("-i").arg(wav)
                .args(["-map","0:v:0","-map","1:a:0","-c:v","copy"])
                .args(export.preset.audio_ffmpeg_args())
                .arg(&output);
            #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
            let output_process=command.output().map_err(|e|e.to_string())?;
            if !output_process.status.success(){return Err(String::from_utf8_lossy(&output_process.stderr).to_string());}
            Ok(())
        })()
    }else{Ok(())};
    let mux_ms = mux_started.elapsed().as_secs_f64()*1000.0;
    let error = match result {
        Ok(result) if result.status.success() && mux_result.is_ok() && depth_result.as_ref().is_none_or(|r|
            r.as_ref().is_ok_and(|status| status.status.success())) => None,
        other => Some(if let Err(error)=&mux_result {error.clone()}
            else if let Some(Err(error)) = depth_result.as_ref() {
                error.to_string()
            } else if let Some(Ok(depth)) = depth_result.as_ref().filter(|r|
                r.as_ref().is_ok_and(|d| !d.status.success())) {
                String::from_utf8_lossy(&depth.stderr).trim().to_owned()
            } else {
                match other {
                    Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                    Err(e) => e.to_string(),
                }
            }),
    };
    if error.is_some() {
        let _ = std::fs::remove_file(&output);
        if let Some(dir) = export.depth_dir.as_ref() { let _ = std::fs::remove_dir_all(dir); }
    }
    StudioExportFinished { error, encoder_flush_ms, mux_ms, depth_dir: export.depth_dir.clone() }
}

pub(in crate::app) struct RenderStudioExport {
    /// Next frame to be written (the renderer delivers in request order).
    pub frame_index: u64,
    /// Frames requested so far; `requested - frame_index` are in flight.
    requested: u64,
    /// Offscreen captures carry their own scene, so the app builds the next
    /// frame while the renderer works; window captures stay strictly serial.
    max_in_flight: u64,
    pub total_frames: u64,
    pub fps: u32,
    output: PathBuf,
    alpha: bool,
    depth: bool,
    depth_dir: Option<PathBuf>,
    depth_child: Option<Child>,
    depth_stdin: Option<StudioPipeWriter>,
    child: Option<Child>,
    stdin: Option<StudioPipeWriter>,
    preset: StudioVideoFormat,
    encoder_speed: StudioEncoderSpeed,
    nvenc_preset: u8,
    crf: u8,
    audio_wav: Option<PathBuf>,
    /// Native event audio is mixed incrementally during the same deterministic
    /// frame evaluation as video; mux only AFTER audio/video are complete.
    native_audio: bool,
    native_audio_file: Option<std::fs::File>,
    audio_frames_written: u64,
    audio_frames_target: u64,
    export_in_time: f64,
    encoded_video: Option<PathBuf>,
    profile: super::render_studio_profile::StudioExportProfile,
    /// Size FFmpeg was opened with; every later frame must match it.
    frame_size: Option<(u32, u32)>,
}

impl RenderStudioExport {
    pub(in crate::app) fn can_request(&self) -> bool {
        self.requested < self.total_frames && self.requested - self.frame_index < self.max_in_flight
    }
    /// Frame whose timeline time the app evaluates. A serial (window) capture
    /// renders the live snapshot, so its time must hold until delivery.
    fn build_index(&self) -> u64 {
        if self.max_in_flight > 1 { self.requested.min(self.total_frames.saturating_sub(1)) } else { self.frame_index }
    }
    pub fn app_cost(&mut self, kind: super::render_studio_profile::StudioAppCost, ms: f64) {
        self.profile.app_cost(kind, ms);
    }
}

/// Feeds one FFmpeg stdin from its own thread, so the frame loop only blocks
/// when FFmpeg falls a few frames behind. Dropping it closes the pipe after
/// the queued frames; `finish` also reports the write result.
struct StudioPipeWriter {
    tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
}
impl StudioPipeWriter {
    const QUEUE: usize = 3;
    fn new(mut stdin: ChildStdin, name: &str) -> Result<Self, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(Self::QUEUE);
        let thread = std::thread::Builder::new().name(name.into()).spawn(move || {
            for frame in rx {
                stdin.write_all(&frame).map_err(|e| e.to_string())?;
                crate::runtime::recycle_video_frame_buffer(frame);
            }
            Ok(()) // stdin drops here: EOF for FFmpeg.
        }).map_err(|e| format!("{name} thread: {e}"))?;
        Ok(Self { tx: Some(tx), thread: Some(thread) })
    }
    fn send(&mut self, frame: Vec<u8>) -> Result<(), String> {
        let sent = self.tx.as_ref().is_some_and(|tx| tx.send(frame).is_ok());
        if sent { Ok(()) } else { Err(self.finish().err().unwrap_or_else(|| "writer closed".into())) }
    }
    fn finish(&mut self) -> Result<(), String> {
        self.tx.take();
        self.thread.take().map_or(Ok(()), |thread| thread.join().unwrap_or_else(|_| Err("writer panicked".into())))
    }
}

impl Drop for RenderStudioExport {
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
        self.native_audio_file.take();
        if let Some(path)=self.audio_wav.take(){let _=std::fs::remove_file(path);}
        if let Some(path)=self.encoded_video.take(){let _=std::fs::remove_file(path);}
    }
}

/// A one-shot still shares the video readback queue, but does not start an
/// encoder or advance the model timeline.
pub(in crate::app) struct RenderStudioStill {
    pub awaiting_frame: bool,
    output: PathBuf,
    alpha: bool,
    depth: bool,
}


/// Editor history uses immutable scene snapshots. A held pointer is one
/// transaction, so a 500-frame drag consumes one undo step, not 500.
#[derive(Clone)]
pub(in crate::app) struct StudioEditSnapshot {
    light_layers: Vec<super::render_studio_lights::StudioLightLayer>,
    scene_lighting: super::render_studio_lights::StudioEnvironment,
    clips: Vec<StudioAnimationClip>, demos: Vec<StudioDemoClip>, camera: Vec<StudioCameraKey>, lanes: usize,
    audio_tracks: Vec<super::render_studio_audio::StudioAudioTrack>,
    director_shots: Vec<super::render_studio_director::StudioDirectorShot>,
    camera_rig: StudioCameraRig,
    lane_models: Vec<String>, lane_sabers: Vec<Option<super::render_studio_timeline::StudioSabers>>,
    lane_char_colors: Vec<Option<[u8; 3]>>, lane_visible: Vec<bool>, in_point: f64, out_point: Option<f64>,
    interpolation: StudioInterpolation, model: String, with_sabers: bool,
    background: StudioBackground, background_map: String,
}
impl StudioEditSnapshot {
    pub(super) fn camera_pose_signature(&self) -> String {
        // Bone samples survive camera-key, offset and damping edits. Only
        // changes to the actual actor pose or attachment invalidate them.
        format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            self.clips.iter().map(|c|((&c.name,c.emote.as_ref().map(|e|&e.id),c.start,
                c.first_frame,c.end_frame,c.source_fps,c.lane,c.source_first,c.source_end,
                c.tail_frames,c.playback,c.blend),(&c.speed_keys,c.motion.iter().map(|m|(m.time,m.position,m.yaw)).collect::<Vec<_>>())))
                .collect::<Vec<_>>(),self.model,(&self.lane_models,&self.lane_sabers,&self.lane_visible),self.with_sabers,self.interpolation,
            (self.camera_rig.position.lane,self.camera_rig.position.part,&self.camera_rig.position.bone),
            (self.camera_rig.target.lane,self.camera_rig.target.part,&self.camera_rig.target.bone))
    }
    pub(super) fn signature(&self) -> String {
        format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            self.clips.iter().map(|c|((&c.name,c.emote.as_ref().map(|e| &e.id),c.start,
                c.first_frame,c.end_frame,c.source_fps,c.lane,c.source_first,c.source_end,
                c.tail_frames,c.playback,c.blend),(&c.speed_keys,c.motion.iter().map(|m|(m.time,m.position,m.yaw)).collect::<Vec<_>>())))
                .collect::<Vec<_>>(),
            self.demos.iter().map(|c|(&c.name,&c.game,c.start,c.duration_ms,c.in_ms,c.out_ms,&c.speed_keys,c.transition_in,&c.audio,&c.race_overlays,(&c.pov_trail,&c.hud_overrides,&c.comparison))).collect::<Vec<_>>(),
            self.camera.iter().map(|k|(k.time,k.position,k.yaw,k.pitch,k.fov,k.focus_target,k.lens,k.rig_orbit,k.source,k.demo_follow)).collect::<Vec<_>>(),
            self.lanes,(&self.lane_models,&self.lane_sabers,&self.lane_char_colors,&self.lane_visible),self.in_point,self.out_point,self.interpolation,
            self.model,self.with_sabers,self.background as u8,self.background_map,self.camera_rig)+
            format!("{:?}{:?}{:?}",self.audio_tracks,self.director_shots,
                (&self.scene_lighting,self.light_layers.iter().map(|l|(&l.name,l.enabled,&l.lights)).collect::<Vec<_>>())).as_str()
    }
}
#[derive(Default)]
pub(in crate::app) struct StudioEditHistory {
    undo: Vec<StudioEditSnapshot>, redo: Vec<StudioEditSnapshot>,
    /// Last observed state; also the "before" of a held-pointer transaction.
    baseline: Option<StudioEditSnapshot>,
    /// `baseline.signature()`, built once per baseline instead of every frame.
    baseline_signature: Option<String>,
    /// Edits applied without UI input (job results, scripts): the next gated
    /// observation must not be skipped.
    pub dirty: bool,
}
impl StudioEditHistory {
    fn set_baseline(&mut self, snapshot: StudioEditSnapshot) {
        self.baseline=Some(snapshot);self.baseline_signature=None;
    }
    fn observe(&mut self, current: StudioEditSnapshot) {
        let signature=current.signature();
        if let Some(before)=self.baseline.take() {
            let before_signature=self.baseline_signature.take().unwrap_or_else(||before.signature());
            if before_signature!=signature {
                self.undo.push(before);if self.undo.len()>256 {self.undo.remove(0);}
                self.redo.clear();
            }
        }
        self.baseline=Some(current);self.baseline_signature=Some(signature);
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::app) enum StudioMediaTab { #[default] Jka, Fortnite, Cutscenes, Demos }
#[derive(Clone)]
pub(in crate::app) struct StudioMediaDrag {pub name: String, pub emote_id: Option<String>,
    pub cinematic:Option<super::render_studio_cinematics::StudioCinematicSource>,
    pub demo_game:Option<String>}

#[derive(Clone)]
pub(in crate::app) enum StudioClipboard {
    Clip(StudioAnimationClip), Camera(StudioCameraKey), Motion(StudioMotionKey),
    Group(StudioClipboardGroup),
}
#[derive(Clone)]
pub(in crate::app) struct StudioClipboardGroup {
    origin: f64,
    clips: Vec<StudioAnimationClip>,
    camera: Vec<StudioCameraKey>,
    /// Source lane, absolute timeline time, and the copied movement value.
    motion: Vec<(usize,f64,StudioMotionKey)>,
}
/// Runtime-only binding to the single existing demo GameSession. This is not
/// serialized: projects only store source clips, never decoder state or GPU
/// resources. The ordinary Demo Player can later be a thin frontend to these
/// same session/seek operations, while export UI lives in Render Studio.
#[derive(Clone, Debug)]
pub(in crate::app) struct StudioDemoPreview {
    pub name: String,
    pub game: String,
    /// True when the timeline is currently over this source.
    pub active: bool,
    /// A generation-bound diagnostic so failure is not mistaken for a pending map.
    pub failed: bool,
}

#[derive(Clone,Copy,Debug)]
pub(in crate::app) struct StudioDemoDrag {
    pub index:usize,
    pub kind:StudioDemoDragKind,
    pub pointer_x:f32,
    pub initial_scroll:f32,
    pub start:f64,
    pub in_ms:i32,
    pub out_ms:i32,
}

/// Editor docking dimensions are runtime preferences, not scene or decoder state.
/// None means Taffy's responsive default; a collapsed sidebar has a small tab.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::app) struct StudioPanelState {
    pub library_width: Option<f32>,
    pub inspector_width: Option<f32>,
    pub timeline_height: Option<f32>,
    pub library_collapsed: bool,
    pub inspector_collapsed: bool,
    /// (separator identity, pointer coordinate at drag start, initial size).
    pub resize_drag: Option<(u8,f32,f32)>,
}

pub(in crate::app) struct RenderStudio {
    pub light_ui: super::render_studio_lights::StudioLightUi,
    /// Index of the global lighting key currently being edited in the viewer.
    pub scene_lighting_selected_key: usize,
    pub scene_lighting_popup_open: bool,
    pub keep_demos_loaded: bool,
    /// The main-menu 3D background was unloaded on entering the Studio and
    /// must be requested again when the Studio page is left for the menu.
    pub frontend_world_released: bool,
    /// Retained demo scenes allowed by GPU memory (current/next always stay).
    /// Lowered when VRAM nears its OS budget; reset when the Studio opens.
    pub scene_capacity: usize,
    /// Recordings already retried once after a scene ran out of VRAM.
    pub oom_retried: std::collections::HashSet<(String, String)>,
    pub sources: Vec<super::render_studio_demo::StudioDemoSource>,
    pub source_job: Option<super::render_studio_demo::StudioSourceJob>,
    pub source_prepares: Vec<super::render_studio_demo::StudioSourceJob>,
    pub composite_search: String,
    pub race_runtime: Vec<super::render_studio_demo::StudioRaceRuntime>,
    pub next_source_id: u64,
    pub scene_frame: Option<std::sync::Arc<crate::renderer::StudioSceneFrame>>,
    pub failed_sources: std::collections::HashMap<(String,String),String>,
    pub source_wait: Option<((f64,Instant,bool),f64)>,
    pub pending_capture_rect: Option<[u32;4]>,
    pub model: String,
    pub model_search: String,
    pub animation_search: String,
    pub emote_search: String,
    pub demo_search: String,
    /// (name, game, time, demo layer) waiting for duration metadata.
    pub pending_demo_drop: Option<(String,String,f64,usize)>,
    pub demo_preview: Option<StudioDemoPreview>,
    pub live_gpu_transitions: bool,
    /// Pixel rectangle supplied by egui for the embedded BSP scene.
    pub demo_viewport: Option<[u32;4]>,
    /// Requested output resolution for demo/BSP exports and stills.
    pub export_size: StudioExportSize,
    /// x264 preset override for the H.264 formats.
    pub encoder_speed: StudioEncoderSpeed,
    /// NVENC preset 1..=7 for the NVENC formats; 0 = default (p5).
    pub nvenc_preset: u8,
    /// Quality of the H.264 / NVENC formats as an x264 CRF (lower = better).
    pub crf: u8,
    /// Fixed offscreen size of the export/still in progress (renderer-side
    /// SetStudioExportSize); `None` = window/viewport capture.
    pub capture_size: Option<[u32;2]>,
    pub cinematics: super::render_studio_cinematics::StudioCinematicLibrary,
    pub cinematic_preview: Option<super::render_studio_cinematics::StudioCinematicPreview>,
    pub stock_animation: String,
    pub emote: Option<crate::emotes::Emote>,
    pub trim_start: i32,
    /// Exclusive end frame in the *source* clip.
    pub trim_stop: i32,
    pub clip_meta: Option<(i32, i32, f32)>,
    pub playhead: f64,
    pub playing: bool,
    pub play_started: Instant,
    pub with_sabers: bool,
    pub background: StudioBackground,
    pub background_map: String,
    pub bsp_world: Option<StudioBspBackdrop>,
    pub bsp_maps: Option<Vec<String>>,
    pub preset: StudioVideoFormat,
    /// Studio graphics profile, separate from gameplay renderer settings.
    pub graphics_profile: String,
    pub graphics_profiles: Vec<String>,
    pub graphics_new_name: String,
    pub graphics_confirm_delete: bool,
    pub graphics_settings_open: bool,
    /// Values last loaded from / written to the selected profile.
    pub graphics_saved: std::collections::BTreeMap<String,String>,
    pub hud_settings: super::render_studio_cfg::StudioHudSettings,
    pub hud_saved: super::render_studio_cfg::StudioHudSettings,
    /// Export-only HUD geometry in a 1280x720 reference frame. Never writes cg_hud*.
    pub hud_layout: crate::ui::HudLayout,
    pub hud_layout_saved: crate::ui::HudLayout,
    /// Transient Program Preview editing controls (never serialized into projects).
    pub hud_editing: bool,
    pub hud_selected: Option<crate::ui::HudElementId>,
    pub hud_drag_origin: Option<[f32;2]>,
    pub hud_scale_origin: Option<f32>,
    /// Gameplay renderer values while the Studio is open (restored on exit).
    pub graphics_gameplay: Option<super::render_studio_cfg::StudioGraphicsSnapshot>,
    pub fps: u32,
    pub transparent: bool,
    pub export_depth: bool,
    pub screenshot_alpha: bool,
    pub screenshot_depth: bool,
    pub still: Option<RenderStudioStill>,
    pub export: Option<RenderStudioExport>,
    /// A completed export still flushing its encoder / muxing audio.
    pub export_finishing: Option<StudioExportFinishing>,
    /// DM26 export, step 1: per-frame Studio camera capture (owns the clock).
    pub dm26_capture: Option<super::render_studio_dm26::StudioDm26Capture>,
    /// DM26 export, step 2: snapshot writer on a worker thread.
    pub dm26_export: Option<std::sync::mpsc::Receiver<Result<super::render_studio_dm26::BakeResult,String>>>,
    pub status: String,
    pub panels: StudioPanelState,
    pub timeline: StudioTimeline,
    pub audio: StudioAudioRuntime,
    /// In-UI media picker; avoids invisible Win32 dialogs in exclusive fullscreen.
    pub file_picker: super::render_studio_picker::StudioFilePicker,
    /// Auto-create a first preview clip once, never re-create it after delete.
    pub initial_clip_seeded: bool,
    pub project_name: String,
    /// Name of project that owns currently referenced managed media (Save As source).
    pub saved_project_name: String,
    /// Temporary Save As name; never affects live media resolution before saving.
    pub save_as_name: String,
    pub show_save_as: bool, pub show_open_project: bool,
    pub show_new_project: bool,
    pub new_project_name: String,
    pub show_render: bool, pub model_picker_lane: Option<usize>,
    /// "Leave Render Studio?" prompt (Esc with nothing left to cancel).
    pub confirm_leave: bool,
    /// Selected Director shot (by stable id) for the inspector.
    pub director_selected: Option<u64>,
    pub director_popup_open: bool,
    pub director_new_preset: super::render_studio_director::StudioDirectorPreset,
    pub director_new_client: i32,
    pub director_new_length: super::render_studio_director::StudioShotLength,
    /// Route Comparison: split threshold (map units) and running analysis.
    pub route_threshold: f32,
    /// Installed fonts for the overlay font picker: (name, path), scanned once.
    pub overlay_fonts: Option<Vec<(String, String)>>,
    pub overlay_fonts_job: Option<std::thread::JoinHandle<Vec<(String, String)>>>,
    pub overlay_font_filter: String,
    pub route_options: super::render_studio_director::StudioRouteOptions,
    pub route_job: Option<super::render_studio_director::StudioRouteJob>,
    /// Further selected Director shots (Ctrl/Shift+click, Ctrl+A); the
    /// primary stays `director_selected`.
    pub director_multi: Vec<u64>,
    pub director_anchor: Option<u64>,
    /// Auto Director: pacing, variation counter, trim a single run to its race.
    pub director_pacing: super::render_studio_auto_director::StudioDirectorPacing,
    pub director_seed: u32,
    pub director_trim_run: bool,
    /// Combat view: slow motion around kills (speed keys on the demo clip).
    pub director_slowmo: bool,
    /// Local monitor volume (0..1); not part of project settings or rendered audio.
    pub preview_volume: f32,
    /// Combat view: 2.5x through downtime between fights.
    pub director_fastforward: bool,
    /// Who the Auto Director cuts around.
    pub director_focus: super::render_studio_auto_director::StudioDirectorFocus,
    pub director_job: Option<super::render_studio_director::StudioDirectorJob>,
    pub director_paths: super::render_studio_auto_director::StudioPathCache,
    pub director_collision: super::render_studio_auto_director::StudioCollisionCache,
    /// Library multi-selection (asset keys) and the row order last drawn.
    pub library_selection: Vec<String>,
    pub library_anchor: Option<String>,
    pub library_rows: Vec<(String, StudioMediaDrag)>,
    pub library_rows_next: Vec<(String, StudioMediaDrag)>,
    pub library_hovered: bool,
    /// Further assets dragged with `media_drag` (placed back to back).
    pub media_drag_group: Vec<StudioMediaDrag>,
    /// Demos of a group drop still waiting for their duration: (name, game, layer).
    pub pending_demo_queue: std::collections::VecDeque<(String, String, usize)>,
    pub navigation: super::render_studio_navigation::StudioPreviewNavigation,
    pub camera_rig_runtime: StudioCameraRigRuntime,
    pub camera_orbit_edit_runtime: StudioCameraRigRuntime,
    pub camera_pending_match: Option<[f32;3]>,
    /// One shared sample time for all actors, attachment queries and FX in a frame.
    pub preview_sample_time: Option<f64>,
    pub camera_bone_search: [String;2],
    pub spatial: super::render_studio_spatial::StudioSpatialEditor,
    pub media_tab: StudioMediaTab, pub media_drag: Option<StudioMediaDrag>,
    /// Exact source length resolved once at drag-start, when possible.
    pub media_drag_duration: Option<f64>,
    pub demo_drag: Option<StudioDemoDrag>,
    pub selected_background: bool,
    pub background_popup_open: bool,
    /// Empty-track context menu keeps the right-click location across frames.
    pub context_time: Option<f64>, pub context_lane: Option<usize>,
    /// Track row under the last right-click (row-aware key menu).
    pub context_row: Option<usize>,
    pub history: StudioEditHistory, pub clipboard: Option<StudioClipboard>,
}

impl Default for RenderStudio {
    fn default() -> Self {
        Self {
            light_ui: Default::default(), scene_lighting_selected_key:0, scene_lighting_popup_open:false,
            keep_demos_loaded: true,
            frontend_world_released: false,
            scene_capacity: usize::MAX,
            oom_retried: Default::default(),
            sources: Vec::new(), source_job: None, source_prepares: Vec::new(), composite_search: String::new(), race_runtime: Vec::new(), next_source_id: 1u64 << 63, scene_frame: None, failed_sources: Default::default(), source_wait: None, pending_capture_rect: None,
            model: String::new(), model_search: String::new(),
            animation_search: String::new(), emote_search: String::new(),
            demo_search: String::new(), pending_demo_drop: None,
            demo_preview: None, live_gpu_transitions: true, demo_viewport: None,
            export_size: StudioExportSize::default(), capture_size: None,
            encoder_speed: StudioEncoderSpeed::default(), nvenc_preset: 0, crf: STUDIO_DEFAULT_CRF,
            cinematics: Default::default(),cinematic_preview:None,
            stock_animation: "BOTH_STAND1".into(), emote: None,
            trim_start: 0, trim_stop: 0, clip_meta: None,
            playhead: 0.0, playing: false, play_started: Instant::now(),
            with_sabers: false, background: StudioBackground::Black,
            background_map: String::new(), bsp_world: None, bsp_maps: None,
            preset: StudioVideoFormat::H264High, fps: 60,
            graphics_profile: super::render_studio_cfg::DEFAULT_PROFILE.into(), graphics_profiles: Vec::new(),
            graphics_new_name: String::new(), graphics_confirm_delete: false,
            graphics_settings_open: false, graphics_saved: Default::default(), graphics_gameplay: None,
            hud_settings: Default::default(), hud_saved: Default::default(),
            hud_layout: Default::default(), hud_layout_saved: Default::default(),
            hud_editing:false, hud_selected:None, hud_drag_origin:None, hud_scale_origin:None,
            transparent: false, export_depth: false,
            screenshot_alpha: true, screenshot_depth: false,
            still: None, export: None, export_finishing: None, dm26_capture: None, dm26_export: None, status: String::new(), panels: StudioPanelState::default(),
            timeline: StudioTimeline::default(), audio:StudioAudioRuntime::default(),
            file_picker:Default::default(), initial_clip_seeded: false,
            project_name: "timeline_project".into(), saved_project_name: "timeline_project".into(), save_as_name: String::new(),
            show_save_as: false, show_open_project: false,
            show_new_project: false, new_project_name: "Untitled".into(),
            show_render: false, model_picker_lane: None, confirm_leave: false,
            director_selected: None, director_popup_open: false, director_new_preset: Default::default(), director_new_client: -1,
            director_new_length: Default::default(), route_threshold: 160.0, overlay_fonts: None, overlay_fonts_job: None, overlay_font_filter: String::new(), route_options: Default::default(), route_job: None,
            director_multi: Vec::new(), director_anchor: None, director_pacing: Default::default(), director_seed: 1, director_trim_run: true, director_slowmo: true, preview_volume: 1.0, director_fastforward: true, director_focus: Default::default(),
            director_job: None, director_paths: Default::default(), director_collision: Default::default(),
            library_selection: Vec::new(), library_anchor: None, library_rows: Vec::new(), library_rows_next: Vec::new(),
            library_hovered: false, media_drag_group: Vec::new(), pending_demo_queue: Default::default(),
            navigation: Default::default(), camera_rig_runtime: Default::default(),camera_orbit_edit_runtime:Default::default(),
            camera_pending_match: None,
            preview_sample_time: None,
            camera_bone_search: Default::default(),
            spatial: Default::default(),
            media_tab: StudioMediaTab::Jka, media_drag: None, media_drag_duration:None,
            demo_drag:None, selected_background: false, background_popup_open: false,
            context_time: None, context_lane: None, context_row: None,
            history: StudioEditHistory::default(), clipboard: None,
        }
    }
}

impl RenderStudio {
    pub fn actor_with_sabers(&self, lane: usize) -> bool {
        self.timeline.lane_sabers.get(lane).and_then(Option::as_ref)
            .map_or(self.with_sabers, |sabers| sabers.enabled)
    }
    pub fn apply_actor_sabers(&self, lane: usize, info: &mut crate::cgame::ClientInfo) {
        if let Some(Some(sabers)) = self.timeline.lane_sabers.get(lane) { sabers.apply(info); }
    }
    pub fn actor_center(&self, zoom: f32) -> [f32;3] {
        let time=self.timeline_time();
        let lane=self.timeline.selected_clip.and_then(|i|self.timeline.clips.get(i))
            .map(|clip|clip.lane).or_else(||self.timeline.active_actors(time).first().map(|clip|clip.lane))
            .unwrap_or(0);
        let motion=self.timeline.clip_at_lane(time,lane)
            .map(|clip|clip.motion_at(time,self.timeline.interpolation != StudioInterpolation::Linear))
            .unwrap_or_default();
        [88.0*zoom.clamp(0.45,2.5)+motion.position[0],18.0+motion.position[1],
            -22.0+lane as f32*26.0+motion.position[2]]
    }
    /// Safe basename only: never let a project field select a parent directory.
    pub fn project_file_name(&self) -> String {
        Self::project_file_name_for(&self.project_name)
    }
    pub fn project_file_name_for(name: &str) -> String {
        let stem: String = name.chars().take(80)
            .filter(|c|c.is_ascii_alphanumeric() || *c=='_' || *c=='-').collect();
        format!("{}.json", if stem.is_empty(){"timeline_project"}else{&stem})
    }
    pub fn select_clip(&mut self, name: Option<&str>, emote: Option<crate::emotes::Emote>) {
        if let Some(name) = name { self.stock_animation = name.to_owned(); }
        self.emote = emote;
        self.clip_meta = None;
        self.playing = false;
        self.playhead = 0.0;
    }

    pub fn set_clip_meta(&mut self, meta: Option<(i32, i32, f32)>) {
        let Some((first, end, fps)) = meta else { return; };
        if first >= end || !fps.is_finite() || fps <= 0.0 { return; }
        if self.clip_meta != Some((first, end, fps)) {
            self.clip_meta = Some((first, end, fps));
            self.trim_start = first;
            self.trim_stop = end;
            self.playhead = 0.0;
            self.playing = false;
            // Seed the non-linear animation track once the first clip has
            // resolved its actual frame range. Subsequent source selections
            // are staged in the library until explicitly added to the track.
            if !self.initial_clip_seeded {
                self.initial_clip_seeded=true;
                self.timeline.add_clip(StudioAnimationClip {
                    name: self.stock_animation.clone(), emote: self.emote.clone(),
                    start: 0.0, first_frame: first, end_frame: end,
                    source_fps: f64::from(fps), lane: 0, source_first: first, source_end: end,
                    motion: vec![Default::default()],tail_frames:0,playback:StudioClipPlayback::Hold,
                speed_keys:vec![], blend:Default::default()});
            }
        }
    }

    pub fn add_current_clip_to_timeline(&mut self) -> Result<(), &'static str> {
        let (first, end) = self.clip_range().ok_or("Animation metadata not ready")?;
        let time = self.timeline.current_time(None);
        self.timeline.add_clip(StudioAnimationClip {
            name: self.stock_animation.clone(), emote: self.emote.clone(), start: time,
            first_frame: first, end_frame: end, source_fps: self.clip_fps(),
            lane: self.timeline.selected_clip.and_then(|i|self.timeline.clips.get(i).map(|c|c.lane)).unwrap_or(0),
            source_first: self.clip_meta.map_or(first,|m|m.0), source_end: self.clip_meta.map_or(end,|m|m.1),
            motion: vec![Default::default()],tail_frames:0,playback:StudioClipPlayback::Hold,
        speed_keys:vec![], blend:Default::default()});
        Ok(())
    }

    pub fn timeline_time(&self) -> f64 {
        if let Some(capture) = &self.dm26_capture {
            return self.timeline.current_time(Some((capture.index, capture.fps)));
        }
        if self.export.is_none() {
            if let Some((transport,time)) = self.source_wait {
                if transport == (self.timeline.playhead,self.timeline.play_started,self.timeline.playing) {
                    return time;
                }
            }
        }
        self.preview_sample_time.unwrap_or_else(||
            self.timeline.current_time(self.export.as_ref().map(|e| (e.build_index(), e.fps))))
    }

    /// Pose blend for the clip on `lane` at the playhead (smoothed transition).
    pub fn pose_blend_request(&self, lane: usize) -> Option<crate::cgame::player_presenter::StudioPoseBlend> {
        if self.cinematic_preview.is_some() || !self.timeline.lane_is_visible(lane) { return None; }
        let time = self.timeline_time();
        let index = self.timeline.clips.iter().position(|c| c.lane == lane && time >= c.start && time < c.end())?;
        let partner = self.timeline.pose_blend_at(index, time)?;
        let other = &self.timeline.clips[partner.clip];
        Some(crate::cgame::player_presenter::StudioPoseBlend {
            animation_name: other.name.clone(), time_ms: other.pose_time_ms(partner.sample_time),
            emote: other.preview(partner.sample_time), frame_range: Some((other.first_frame, other.end_frame)),
            weight: partner.weight,
        })
    }
    /// Motion of the clip on `lane` at `time`, blended across smoothed transitions.
    pub fn lane_motion(&self, lane: usize, time: f64) -> Option<super::render_studio_timeline::StudioMotionKey> {
        if !self.timeline.lane_is_visible(lane) { return None; }
        let index = self.timeline.clips.iter().position(|c| c.lane == lane && time >= c.start && time < c.end())?;
        Some(self.timeline.blended_motion_at(index, time, self.timeline.interpolation != StudioInterpolation::Linear))
    }

    pub fn active_animation(&self) -> (String, Option<crate::emotes::Preview>, Option<(i32,i32)>, i32) {
        if let Some(preview)=self.cinematic_preview.as_ref() {
            let ms=(preview.elapsed()*1000.).round() as i32;
            let mut emote=preview.emote.clone();emote.looping=false;emote.loop_start_frame=None;
            return (preview.clip.source.animation.clone(),Some(crate::emotes::Preview {emote,elapsed_ms:ms}),
                Some((preview.clip.first_frame,preview.clip.end_frame())),ms);
        }
        let time = self.timeline_time();
        if let Some(clip) = self.timeline.clip_at_lane(time,0) {
            return (clip.name.clone(), clip.preview(time), Some((clip.first_frame, clip.end_frame)), clip.pose_time_ms(time));
        }
        ("BOTH_STAND1".into(), None, None, 0)
    }

    pub fn clip_range(&self) -> Option<(i32, i32)> {
        self.clip_meta.map(|(lo, hi, _)| (
            self.trim_start.clamp(lo, hi - 1),
            self.trim_stop.clamp(self.trim_start.clamp(lo, hi - 1) + 1, hi),
        ))
    }

    pub fn clip_fps(&self) -> f64 {
        self.clip_meta.map_or(20.0, |(_, _, fps)| fps as f64)
    }

    pub fn duration_frames(&self) -> f64 {
        self.clip_range().map_or(1.0, |(first, end)| (end - first) as f64).max(1.0)
    }

    pub fn offset_frames(&self) -> f64 {
        if let Some(export) = &self.export {
            (export.build_index() as f64 * self.clip_fps() / export.fps as f64)
                .min((self.duration_frames() - f64::EPSILON).max(0.0))
        } else if self.playing {
            (self.playhead + self.play_started.elapsed().as_secs_f64() * self.clip_fps())
                % self.duration_frames()
        } else {
            self.playhead.clamp(0.0, self.duration_frames() - f64::EPSILON)
        }
    }

    pub fn presentation_time_ms(&self) -> i32 {
        let offset = self.offset_frames();
        let relative = if let Some(emote) = &self.emote {
            (self.trim_start - emote.first_frame).max(0) as f64 + offset
        } else {
            offset
        };
        (relative * 1000.0 / self.clip_fps()).round().clamp(0.0, i32::MAX as f64) as i32
    }

    pub fn emote_preview(&self) -> Option<crate::emotes::Preview> {
        self.emote.clone().map(|emote| crate::emotes::Preview {
            emote,
            elapsed_ms: self.presentation_time_ms(),
        })
    }
}

impl RenderStudio {
    /// Fixed offscreen output size, when it applies: demo worlds and BSP
    /// backdrops render offscreen; stage-only model frames use the window.
    pub fn offscreen_output_size(&self) -> Option<[u32;2]> {
        let worlds = !self.timeline.demos.is_empty() || self.background == StudioBackground::Bsp;
        if worlds { self.export_size.dimensions() } else { None }
    }
    pub fn actor_model(&self, lane: usize) -> &str {
        self.timeline.lane_models.get(lane).filter(|model|!model.is_empty())
            .map_or(self.model.as_str(),String::as_str)
    }
    /// Resolve a character-track color without mutating multiplayer userinfo cvars.
    pub fn actor_char_color(&self, lane: usize, profile: [u8; 3]) -> [u8; 3] {
        self.timeline.lane_char_colors.get(lane).copied().flatten().unwrap_or(profile)
    }
    pub fn edit_snapshot(&self) -> StudioEditSnapshot {
        StudioEditSnapshot {light_layers:self.timeline.light_layers.clone(),scene_lighting:self.timeline.scene_lighting.clone(),clips:self.timeline.clips.clone(),demos:self.timeline.demos.clone(),camera:self.timeline.camera.clone(),
            director_shots:self.timeline.director_shots.clone(),
            lanes:self.timeline.lanes,audio_tracks:self.timeline.audio_tracks.clone(),lane_models:self.timeline.lane_models.clone(),
            lane_sabers:self.timeline.lane_sabers.clone(),lane_char_colors:self.timeline.lane_char_colors.clone(),
            lane_visible:self.timeline.lane_visible.clone(),
            camera_rig:self.timeline.camera_rig.clone(),
            in_point:self.timeline.in_point,out_point:self.timeline.out_point,
            interpolation:self.timeline.interpolation,model:self.model.clone(),
            with_sabers:self.with_sabers,background:self.background,background_map:self.background_map.clone()}
    }
    fn apply_snapshot(&mut self,s:StudioEditSnapshot) {
        self.timeline.light_layers=s.light_layers;self.timeline.scene_lighting=s.scene_lighting;
        self.scene_lighting_selected_key=0;self.light_ui=Default::default();self.spatial.camera_handle_drag=None;
        self.cinematic_preview=None;
        self.timeline.clips=s.clips;self.timeline.demos=s.demos;self.race_runtime.clear();self.timeline.camera=s.camera;self.timeline.director_shots=s.director_shots;self.timeline.lanes=s.lanes;
        self.timeline.audio_tracks=s.audio_tracks;
        self.audio.selected=None; self.audio.selected_track=None; self.audio.selected_key=None; self.audio.selected_demo_audio=None; self.audio.preview=None;
        self.audio.drag=None;self.audio.fade_drag=None;self.audio.demo_fade_drag=None;self.audio.demo_gain_drag=None;self.audio.trim_drag=None;self.audio.key_drag=None;self.audio.pending_import=None;
        self.timeline.audio_expanded.resize(self.timeline.audio_tracks.len(),false);
        self.timeline.lane_models=s.lane_models;
        self.timeline.lane_sabers=s.lane_sabers;
        self.timeline.lane_char_colors=s.lane_char_colors;
        self.timeline.lane_visible=s.lane_visible;
        self.timeline.speed_drag=None;
        self.timeline.lane_speed_expanded.resize(s.lanes,false);
        self.timeline.camera_rig=s.camera_rig;self.camera_rig_runtime=Default::default();self.camera_pending_match=None;
        self.spatial.drag=None;
        self.timeline.in_point=s.in_point;self.timeline.out_point=s.out_point;
        self.timeline.interpolation=s.interpolation;
        self.model=s.model;self.with_sabers=s.with_sabers;
        self.background=s.background;self.background_map=s.background_map;
        self.timeline.clear_selection();
        self.timeline.drag=None;self.demo_drag=None;self.timeline.playing=false;
    }
    pub fn init_history(&mut self) {
        if self.history.baseline.is_none() {let s=self.edit_snapshot();self.history.set_baseline(s);}
    }
    pub fn track_edit(&mut self,pointer_down:bool) {
        // A held pointer is one transaction: its "before" stays the baseline,
        // so a 500-frame drag needs no per-frame snapshot.
        if pointer_down && self.history.baseline.is_some() {return;}
        let s=self.edit_snapshot();self.history.observe(s);
    }
    /// Per-frame UI tracking. Snapshotting and signing the whole timeline is
    /// costly (Director path keys), so frames without edit-type input (buttons,
    /// keys, text, wheel, drops) and no `history.dirty` mark are skipped.
    pub fn track_ui_edit(&mut self,pointer_down:bool,edit_input:bool) {
        let dirty=std::mem::take(&mut self.history.dirty);
        if !edit_input && !dirty && self.history.baseline.is_some() {return;}
        self.track_edit(pointer_down);
        // A transaction still open keeps the mark for its release frame.
        if pointer_down {self.history.dirty|=dirty;}
    }
    pub fn undo_edit(&mut self) -> bool {
        let Some(previous)=self.history.undo.pop() else{return false};
        let current=self.edit_snapshot();self.history.redo.push(current);
        self.apply_snapshot(previous);
        let s=self.edit_snapshot();self.history.set_baseline(s);
        true
    }
    pub fn redo_edit(&mut self) -> bool {
        let Some(next)=self.history.redo.pop() else{return false};
        let current=self.edit_snapshot();self.history.undo.push(current);
        self.apply_snapshot(next);
        let s=self.edit_snapshot();self.history.set_baseline(s);
        true
    }
    /// Consume semantic clipboard commands in order, including when Ctrl was
    /// released before this UI frame. Text fields retain their normal clipboard.
    pub fn handle_clipboard_input(&mut self, ctx: &egui::Context) -> bool {
        if ctx.text_edit_focused() { return false; }
        let commands=ctx.input_mut(|input| {
            let mut commands=Vec::new();
            input.events.retain(|event| {
                let copy=match event {
                    egui::Event::Copy=>true,
                    egui::Event::Paste(_)=>false,
                    _=>return true,
                };
                commands.push(copy);
                false
            });
            commands
        });
        let mut pasted=false;
        for copy in commands {
            if copy {
                self.status=if self.copy_selection() {"Selection copied"} else {"Select a timeline clip or key to copy"}.into();
            } else if self.paste_selection() {
                self.status="Selection pasted".into();
                pasted=true;
            } else {
                self.status="Clipboard empty or no destination clip selected for motion key".into();
            }
        }
        pasted
    }
    pub fn copy_selection(&mut self) -> bool {
        let items=self.timeline.selected_items();
        if items.len()>1 {
            let origin=items.iter().filter_map(|&item|self.timeline.item_time(item)).fold(f64::INFINITY,f64::min);
            let mut group=StudioClipboardGroup {origin,clips:Vec::new(),camera:Vec::new(),motion:Vec::new()};
            for item in &items {
                match *item {
                    StudioItem::Clip(i)=>group.clips.push(self.timeline.clips[i].clone()),
                    StudioItem::Camera(i)=>group.camera.push(self.timeline.camera[i]),
                    StudioItem::Motion(i,k)=>{
                        // A copied clip already contains its movement keys.
                        if !items.contains(&StudioItem::Clip(i)) {
                            group.motion.push((self.timeline.clips[i].lane,self.timeline.item_time(*item).unwrap(),self.timeline.clips[i].motion[k]));
                        }
                    }
                }
            }
            self.clipboard=Some(StudioClipboard::Group(group));return true;
        }
        let value=if let Some((i,k))=self.timeline.selected_motion {
            self.timeline.clips.get(i).and_then(|c|c.motion.get(k)).copied().map(StudioClipboard::Motion)
        } else if let Some(i)=self.timeline.selected_camera {
            self.timeline.camera.get(i).copied().map(StudioClipboard::Camera)
        } else {self.timeline.selected_clip.and_then(|i|self.timeline.clips.get(i)).cloned().map(StudioClipboard::Clip)};
        if value.is_some(){self.clipboard=value;true}else{false}
    }
    /// Track-header copy deliberately includes whole animation clips: their
    /// source trimming, time remap, and embedded motion keys travel together.
    /// Model/visibility settings belong to the destination character track.
    pub fn copy_animation_lane(&mut self, lane:usize) -> bool {
        if lane>=self.timeline.lanes {return false;}
        let clips:Vec<_>=self.timeline.clips.iter().filter(|c|c.lane==lane).cloned().collect();
        if clips.is_empty() {
            self.status=format!("Character {} has no animations to copy",lane+1);
            return false;
        }
        let count=clips.len();
        let origin=clips.iter().map(|c|c.start).fold(f64::INFINITY,f64::min);
        self.clipboard=Some(StudioClipboard::Group(StudioClipboardGroup {
            origin,clips,camera:Vec::new(),motion:Vec::new(),
        }));
        self.status=format!("Copied {count} animation clip(s) from character {}",lane+1);
        true
    }
    pub fn clipboard_contains_animations(&self)->bool {
        match self.clipboard.as_ref() {
            Some(StudioClipboard::Clip(_))=>true,
            Some(StudioClipboard::Group(group))=>!group.clips.is_empty(),
            _=>false,
        }
    }
    /// Dedicated track paste never copies unrelated camera/movement keys from
    /// a mixed timeline selection. An omitted time keeps the source timecode;
    /// a time places the first animation at that timestamp (first-fit on clash).
    pub fn paste_animations_on_lane(&mut self, lane:usize, time:Option<f64>)->bool {
        if lane>=self.timeline.lanes {return false;}
        let clips=match self.clipboard.as_ref() {
            Some(StudioClipboard::Clip(clip))=>vec![clip.clone()],
            Some(StudioClipboard::Group(group))=>group.clips.clone(),
            _=>return false,
        };
        if clips.is_empty(){return false;}
        let origin=clips.iter().map(|c|c.start).fold(f64::INFINITY,f64::min);
        let count=clips.len();
        let result=self.paste_group_at(StudioClipboardGroup {
            origin,clips,camera:Vec::new(),motion:Vec::new(),
        },Some(lane),time.unwrap_or(origin),time.is_some());
        if result { self.status=format!("Pasted {count} animation clip(s) onto character {}",lane+1); }
        result
    }
    pub fn paste_selection(&mut self) -> bool {
        // The most recently clicked/right-clicked character row is an explicit
        // keyboard destination. With none selected, preserve source lanes.
        self.paste_selection_on_lane(self.context_lane)
    }
    pub fn paste_selection_on_lane(&mut self, target_lane: Option<usize>) -> bool {
        self.paste_selection_on_lane_at(target_lane,self.timeline.current_time(None))
    }
    pub fn paste_selection_on_lane_at(&mut self, target_lane: Option<usize>, t:f64) -> bool {
        if target_lane.is_some_and(|lane|lane>=self.timeline.lanes){return false;}
        match self.clipboard.clone() {
            Some(StudioClipboard::Clip(mut c))=>{
                c.start=t;
                if let Some(lane)=target_lane {c.lane=lane;}
                self.timeline.add_clip(c);true
            }
            Some(StudioClipboard::Camera(mut k))=>{
                k.time=t;self.timeline.add_camera_key(k);true
            }
            Some(StudioClipboard::Motion(mut k))=>{
                let Some(i)=self.timeline.selected_clip else{return false};
                let Some(c)=self.timeline.clips.get_mut(i) else{return false};
                k.time=c.source_key_time(t);
                let j=c.add_motion_key(k);self.timeline.select_item(StudioItem::Motion(i,j),false,false);true
            }
            Some(StudioClipboard::Group(group))=>self.paste_group_at(group,target_lane,t,true),
            None=>false,
        }
    }
    fn paste_group_at(&mut self, group:StudioClipboardGroup, target_lane:Option<usize>, time:f64, snap:bool) -> bool {
        let min_lane=group.clips.iter().map(|clip|clip.lane).chain(group.motion.iter().map(|key|key.0)).min().unwrap_or(0);
        let max_lane=group.clips.iter().map(|clip|clip.lane).chain(group.motion.iter().map(|key|key.0)).max().unwrap_or(0);
        // A group may span several character rows. Reject an invalid target
        // instead of silently pasting on its original lanes.
        let lane_delta=if let Some(lane)=target_lane {
            if lane>=self.timeline.lanes || lane+max_lane-min_lane>=self.timeline.lanes {return false;}
            lane as isize-min_lane as isize
        } else {0};
        let mut offset=if snap {self.timeline.snap_time(time)} else {time.max(0.0)};
        offset-=group.origin;
        // Find one common offset, so collisions never collapse group spacing.
        loop {
            let mut advance=0.0f64;
            for clip in &group.clips {
                let lane=(clip.lane as isize+lane_delta) as usize;
                let start=clip.start+offset;
                for existing in self.timeline.clips.iter().filter(|existing|existing.lane==lane) {
                    if start<existing.end()-1e-8 && start+clip.duration()>existing.start+1e-8 {
                        advance=advance.max(existing.end()-start);
                    }
                }
            }
            if advance<=1e-8 {break;}
            offset+=advance;
        }
        let copies:Vec<_>=group.clips.iter().cloned().map(|mut clip| {
            clip.start+=offset;clip.lane=(clip.lane as isize+lane_delta) as usize;clip
        }).collect();
        let mut motions=Vec::new();
        // Validate every destination before changing any timeline contents.
        for (lane,time,mut key) in group.motion {
            let lane=(lane as isize+lane_delta) as usize;
            let time=time+offset;
            let Some((i,clip))=self.timeline.clips.iter().chain(copies.iter()).enumerate()
                .find(|(_,clip)|clip.lane==lane && time>=clip.start-1e-8 && time<=clip.end()+1e-8) else {return false};
            key.time=clip.source_key_time(time);
            motions.push((i,key));
        }
        let mut selected=Vec::new();
        for clip in copies {
            selected.push(StudioItem::Clip(self.timeline.clips.len()));self.timeline.clips.push(clip);
        }
        let mut camera_times=Vec::new();
        for mut key in group.camera {
            key.time+=offset;camera_times.push(key.time);self.timeline.insert_camera_key(key);
        }
        for time in camera_times {
            if let Some(i)=self.timeline.camera.iter().position(|key|(key.time-time).abs()<1e-5) {selected.push(StudioItem::Camera(i));}
        }
        for &(i,key) in &motions {self.timeline.clips[i].add_motion_key(key);}
        for (i,key) in motions {
            if let Some(k)=self.timeline.clips[i].motion.iter().position(|other|(other.time-key.time).abs()<1e-5) {selected.push(StudioItem::Motion(i,k));}
        }
        self.timeline.set_selection(selected);true
    }
}

impl App {
    /// Reframe the currently selected camera key (or author one at the
    /// playhead) toward the active actor. Explicit action: dropping media or
    /// scrubbing must never silently overwrite authored cinematic cameras.
    pub(in crate::app) fn render_studio_frame_actor(&mut self) {
        let time=self.render_studio.timeline_time();
        let lane=self.render_studio.timeline.selected_clip
            .and_then(|i|self.render_studio.timeline.clips.get(i))
            .map(|c|c.lane)
            .or_else(||self.render_studio.timeline.active_actors(time).first().map(|c|c.lane))
            .unwrap_or(0);
        // Camera coordinates are X-forward, Y-up, Z-sideways. Secondary
        // actors are spaced laterally on Z, never stacked vertically on Y.
        let target=self.render_studio.actor_center(self.profile_preview_zoom);
        let eye=[0.0,-11.0,0.0];
        let dx=target[0]-eye[0];
        let dy=target[1]-eye[1];
        let dz=target[2]-eye[2];
        let yaw=(-dz).atan2(dx).to_degrees().rem_euclid(360.0);
        let pitch=dy.atan2((dx*dx+dz*dz).sqrt()).to_degrees();
        if self.render_studio.timeline.camera_rig.active() {
            self.update_studio_camera_rig();
            let visible=self.studio_shot_camera();
            self.render_studio.timeline.begin_camera_edit();
            if let Some(key)=self.render_studio.timeline.selected_camera
                .and_then(|i|self.render_studio.timeline.camera.get_mut(i)) {
                for i in 0..3 {key.position[i]+=eye[i]-visible.position[i];}
                key.yaw=yaw;key.pitch=pitch;
            }
        } else if let Some(key)=self.render_studio.timeline.selected_camera
            .and_then(|i|self.render_studio.timeline.camera.get_mut(i)) {
            key.position=eye;key.yaw=yaw;key.pitch=pitch;
        } else {
            let mut key=self.render_studio.timeline.camera_at(time);
            key.time=time;key.position=eye;key.yaw=yaw;key.pitch=pitch;
            self.render_studio.timeline.add_camera_key(key);
        }
        self.render_studio.timeline.seek(time);
        self.profile_preview_key=None;
        self.render_studio.status=format!("Framed character {} in selected camera key",lane+1);
    }

    /// Resolve source metadata only on an actual drop, not for every tile in the bin.
    /// The library is a source catalogue; the timeline owns placed animation clips.
    pub(in crate::app) fn render_studio_drop_media(&mut self,
        asset: StudioMediaDrag, time: f64, lane: usize) {
        if lane>=self.render_studio.timeline.lanes {return;}
        self.render_studio.cinematic_preview=None;
        let emote = if let Some(source)=asset.cinematic.as_ref() {
            match self.resolve_studio_cinematic(source).and_then(|clip|clip.emote()) {
                Ok(emote)=>Some(emote),Err(error)=>{self.render_studio.status=error;return;}
            }
        } else if let Some(id)=asset.emote_id.as_deref() {
            match crate::emotes::catalog(&self.base) {
                Ok(catalog) => {
                    let found=catalog.iter().flat_map(|root|
                        std::iter::once(root).chain(root.variants.iter()))
                        .find(|candidate| candidate.id==id).cloned();
                    match found {
                        Some(variant) => match crate::emotes::prepare_visuals(variant) {
                            Ok((emote,_)) => Some(emote),
                            Err(err) => {self.render_studio.status=err;return;}
                        },
                        None=>{self.render_studio.status=format!("Emote not found: {id}");return;}
                    }
                }
                Err(err)=>{self.render_studio.status=err.to_string();return;}
            }
        } else {None};
        let (first,end,fps)=if let Some(ref e)=emote {
            (e.first_frame,e.end_frame,f64::from(e.fps))
        } else {
            match self.asset_preview_player_presenter.as_ref()
                .and_then(|presenter|presenter.profile_preview_animation_span(&asset.name)) {
                Some((first,end,fps))=>(first,end,f64::from(fps)),
                None=>{self.render_studio.status=format!("Animation unavailable for current model: {}",asset.name);return;}
            }
        };
        if end<=first || fps<=0.0 {self.render_studio.status="Invalid source animation range".into();return;}
        let playback=if asset.cinematic.is_some() && emote.as_ref().is_some_and(|e|e.looping) {StudioClipPlayback::Loop}else{StudioClipPlayback::Hold};
        let name=asset.cinematic.as_ref().map_or_else(||asset.name.clone(),|source|source.animation.clone());
        let clip=StudioAnimationClip {
            name,start:time,first_frame:first,end_frame:end,
            source_fps:fps,lane,source_first:first,source_end:end,
            emote,motion:vec![StudioMotionKey::default()],tail_frames:0,
            playback,speed_keys:vec![], blend:Default::default()
        };
        self.render_studio.timeline.add_clip(clip);
        self.render_studio.selected_background=false;
        self.render_studio.status=format!("Placed animation on character {}",lane+1);
        self.profile_preview_key=None;
    }

    pub(in crate::app) fn save_render_studio_project(&mut self) -> bool {
        if let Err(error) = self.studio_rehome_media_for_save() {
            self.render_studio.status = format!("Could not save timeline: {error}");
            return false;
        }
        let mut audio_tracks = self.render_studio.timeline.audio_tracks.clone();
        for row in &mut audio_tracks {
            for clip in &mut row.clips {clip.path = self.studio_saved_audio_path(&clip.path);}
        }
        if self.render_studio.timeline.demos.iter().any(|c|c.source_path.is_none()
            || c.race_overlays.iter().any(|o|o.source_path.is_none())) {
            self.render_studio.status = "Could not save timeline: demo has no imported recording".into();
            return false;
        }
        let project = StudioProject {
            version: 15,
            light_layers: self.render_studio.timeline.light_layers.clone(),
            scene_lighting: self.render_studio.timeline.scene_lighting.clone(),
            model: self.render_studio.model.clone(),
            demos: self.render_studio.timeline.demos.clone(),
            race_overlays: Vec::new(),
            director_shots: self.render_studio.timeline.director_shots.clone(),
            audio_tracks,
            with_sabers: self.render_studio.with_sabers,
            background: match self.render_studio.background {
                StudioBackground::Black => "black", StudioBackground::Green => "green",
                StudioBackground::Blue => "blue", StudioBackground::Levelshot => "levelshot",
                StudioBackground::Bsp => "bsp",
            }.into(),
            background_map: self.render_studio.background_map.clone(),
            clips: self.render_studio.timeline.clips.iter().map(|c| StudioProjectClip {
                name: c.name.clone(),
                cinematic: c.emote.as_ref().and_then(super::render_studio_cinematics::StudioCinematicSource::from_emote),
                emote_id: c.emote.as_ref().filter(|e|super::render_studio_cinematics::StudioCinematicSource::from_emote(e).is_none()).map(|e| e.id.clone()),
                start: c.start, first_frame: c.first_frame, end_frame: c.end_frame,
                source_fps: c.source_fps, lane: c.lane, source_first: Some(c.source_first),
                source_end: Some(c.source_end), motion: c.motion.clone(),
                tail_frames: c.tail_frames, playback: c.playback,speed_keys:c.speed_keys.clone(), blend: c.blend,
            }).collect(),
            camera: self.render_studio.timeline.camera.clone(),
            camera_rig: self.render_studio.timeline.camera_rig.clone(),
            lanes: self.render_studio.timeline.lanes,
            in_point: self.render_studio.timeline.in_point,
            out_point: self.render_studio.timeline.out_point,
            interpolation: self.render_studio.timeline.interpolation,
            lane_models: self.render_studio.timeline.lane_models.clone(),
            lane_sabers: self.render_studio.timeline.lane_sabers.clone(),
            lane_char_colors: self.render_studio.timeline.lane_char_colors.clone(),
            lane_visible: self.render_studio.timeline.lane_visible.clone(),
            render_cfg: self.render_studio.graphics_profile.clone(),
            demo_layer_hidden: self.render_studio.timeline.demo_layer_hidden.clone(),
        };
        let result = (|| -> Result<PathBuf, String> {
            let path = self.studio_project_path();
            std::fs::create_dir_all(path.parent().ok_or("Invalid project folder")?).map_err(|e|e.to_string())?;
            let bytes = serde_json::to_vec_pretty(&project).map_err(|e| e.to_string())?;
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            Ok(path)
        })();
        let saved=result.is_ok();
        if saved {self.render_studio.saved_project_name = self.render_studio.project_name.clone();}
        self.render_studio.status = match result {
            Ok(path) => format!("Saved editable timeline to {}", path.display()),
            Err(err) => format!("Could not save timeline: {err}"),
        };
        saved
    }

    pub(in crate::app) fn load_render_studio_project(&mut self) {
        let path = self.studio_project_path();
        self.load_render_studio_project_path(&path);
    }

    pub(in crate::app) fn load_render_studio_project_path(&mut self, path: &std::path::Path) {
        let result = (|| -> Result<(StudioProject, Vec<StudioAnimationClip>), String> {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            if text.len() > 1_000_000 { return Err("Project exceeds 1 MB".into()); }
            let project: StudioProject = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if project.version != 15 { return Err("Unsupported timeline project version".into()); }
            if !project.scene_lighting.valid() { return Err("Invalid Render Studio ambient / sun keys".into()); }
            if !super::render_studio_lights::valid_layers(&project.light_layers) { return Err("Invalid light layers or keyframes in project".into()); }
            if project.clips.len() > 500 || project.camera.len() > 500 || project.demos.len()>500 || project.director_shots.len()>500 {
                return Err("Project exceeds 500 clips / camera keys".into());
            }
            let mut clips = Vec::<StudioAnimationClip>::new();
            for c in &project.clips {
                if c.lane > 15 || c.motion.len()>1000 || c.tail_frames<0 || c.tail_frames>360000 ||
                    !c.start.is_finite() || c.start < 0.0 || c.start > 36000.0 ||
                    !super::render_studio_timeline::studio_speed_valid(&c.speed_keys,
                        f64::from(c.source_end.unwrap_or(c.end_frame)-c.source_first.unwrap_or(c.first_frame)+c.tail_frames)/c.source_fps.max(1.0)) ||
                    !c.source_fps.is_finite() || c.source_fps <= 0.0 || c.source_fps > 1000.0 ||
                    c.end_frame <= c.first_frame || c.end_frame - c.first_frame > 1_000_000 {
                    return Err("Invalid animation clip in project".into());
                }
                let emote = if let Some(source)=c.cinematic.as_ref() {
                    let clip=self.resolve_studio_cinematic(source)?;
                    if c.emote_id.is_some() || c.first_frame<clip.first_frame || c.end_frame>clip.end_frame()
                        || (c.source_fps-f64::from(clip.fps)).abs()>0.000001
                        || c.source_first.is_some_and(|f|f!=clip.first_frame) || c.source_end.is_some_and(|f|f!=clip.end_frame()) {
                        return Err("Cinematic clip trim is outside its source animation".into());
                    }
                    Some(clip.emote()?)
                } else if let Some(id) = c.emote_id.as_deref() {
                    let catalog = crate::emotes::catalog(&self.base).map_err(|e| e.to_string())?;
                    let variant = catalog.iter().flat_map(|root|
                        std::iter::once(root).chain(root.variants.iter()))
                        .find(|candidate| candidate.id == id)
                        .cloned().ok_or_else(|| format!("Missing Fortnite emote {id}"))?;
                    Some(crate::emotes::prepare_visuals(variant).map_err(|e| e.to_string())?.0)
                } else { None };
                clips.push(StudioAnimationClip {
                    name: c.name.clone(), emote, start: c.start,
                    first_frame: c.first_frame, end_frame: c.end_frame,
                    source_fps: c.source_fps, lane: c.lane,
                    source_first: c.source_first.unwrap_or(c.first_frame).min(c.first_frame),
                    source_end: c.source_end.unwrap_or(c.end_frame).max(c.end_frame),
                    motion: c.motion.clone(),tail_frames:c.tail_frames,playback:c.playback,speed_keys:c.speed_keys.clone(), blend: if c.blend.valid() {c.blend} else {Default::default()}
                });
            }
            if project.race_overlays.len()>32 || project.race_overlays.iter().any(|o| !o.valid()) {
                return Err("Invalid local composite race sources in project".into());
            }
            if project.audio_tracks.len()>32 || project.audio_tracks.iter().any(|t|!t.valid()) {
                return Err("Invalid audio tracks/clips/volume automation in project".into());
            }
            if project.director_shots.iter().any(|shot|!shot.valid()) {
                return Err("Invalid Director shot in project".into());
            }
            if project.demos.iter().any(|c| c.source_path.is_none() || !c.valid() || c.race_overlays.len()+project.race_overlays.iter().filter(|o| !c.race_overlays.iter().any(|existing|existing.name==o.name && existing.game==o.game)).count()>32) {
                return Err("Invalid demo source, time range or game directory in project".into());
            }
            if !StudioTimeline::demo_overlap_valid(&project.demos) {
                return Err("Demo overlap must be an explicitly authored transition between adjacent sources".into());
            }
            clips.sort_by(|a,b| a.lane.cmp(&b.lane).then_with(||a.start.total_cmp(&b.start)));
            if clips.windows(2).any(|p| p[0].lane==p[1].lane && p[0].end() > p[1].start + 0.00001) {
                return Err("Overlapping animation clips are unsupported".into());
            }
            if project.lanes==0 || project.lanes>16 || !project.camera_rig.valid(project.lanes) || !project.in_point.is_finite() || project.in_point<0.0 ||
                project.out_point.is_some_and(|t|!t.is_finite() || t <= project.in_point || t > 36000.0) ||
                clips.iter().any(|c|c.lane>=project.lanes || c.end()>36000.0 || c.motion.iter().any(|k|
                    !k.time.is_finite() || k.time<0.0 || k.time>(c.source_end-c.source_first+c.tail_frames) as f64/c.source_fps+0.0001 ||
                    !k.yaw.is_finite() || k.position.iter().any(|v|!v.is_finite()))) ||
                project.camera.iter().any(|k| !k.lens.valid() || !k.focus_target_valid() || !k.time.is_finite() || k.time < 0.0 ||
                k.time > 36000.0 || matches!(k.source,
                    super::render_studio_timeline::StudioCameraSource::Demo {client,..} if !(-1..=1023).contains(&client)) || !k.fov.is_finite() || k.fov < 1.0 || k.fov > 176.0 ||
                !k.yaw.is_finite() || !k.pitch.is_finite() || !k.demo_follow.valid() || k.rig_orbit.is_some_and(|o|!o.valid()) ||
                k.position.iter().any(|c| !c.is_finite())) {
                return Err("Invalid camera key in project".into());
            }
            // Validate all media paths before mutating the live project.
            let project_folder = path.parent().ok_or("Project has no directory")?;
            for clip in &project.demos {
                let reference = clip.source_path.as_deref().ok_or("Missing imported demo reference")?;
                let media = super::render_studio_media::media_path(project_folder,reference)?;
                if !media.is_file() {return Err(format!("Missing demo: {}",media.display()));}
                for overlay in &clip.race_overlays {
                    let reference=overlay.source_path.as_deref().ok_or("Composite demo has no imported source")?;
                    let media=super::render_studio_media::media_path(project_folder,reference)?;
                    if !media.is_file() {return Err(format!("Missing composite demo: {}",media.display()));}
                }
            }
            for clip in project.audio_tracks.iter().flat_map(|t|&t.clips) {
                if clip.path.starts_with("media/") {
                    let media = super::render_studio_media::media_path(project_folder,&clip.path)?;
                    if !media.is_file() {return Err(format!("Missing audio: {}",media.display()));}
                } else if !std::path::Path::new(&clip.path).is_absolute() {
                    return Err(format!("Invalid audio path: {}",clip.path));
                }
            }
            Ok((project, clips))
        })();
        match result {
            Ok((project, clips)) => {
                self.render_studio.timeline.light_layers=project.light_layers;
                self.render_studio.timeline.scene_lighting=project.scene_lighting;
                self.render_studio.scene_lighting_selected_key=0;
                self.render_studio.light_ui=Default::default();
                self.render_studio.cinematic_preview=None;
                self.render_studio.timeline.clips = clips;
                let mut demos = project.demos;
                // Previous Studio projects had a scene-wide ghost list. Import
                // each entry onto every original demo clip to preserve what
                // those projects displayed, but save all new edits per clip.
                for clip in &mut demos {
                    for overlay in &project.race_overlays {
                        if !clip.race_overlays.iter().any(|o| o.name==overlay.name && o.game==overlay.game) {
                            clip.race_overlays.push(overlay.clone());
                        }
                    }
                }
                self.render_studio.timeline.demos=demos;
                self.render_studio.race_runtime.clear();
                self.invalidate_studio_sources();
                let mut audio_tracks=project.audio_tracks;
                for row in &mut audio_tracks {
                    for clip in &mut row.clips {
                        if clip.path.starts_with("media/") {
                            clip.path = path.parent().unwrap().join(&clip.path).to_string_lossy().into_owned();
                        }
                    }
                }
                self.render_studio.timeline.audio_tracks=audio_tracks;
                self.render_studio.timeline.audio_expanded=vec![false;self.render_studio.timeline.audio_tracks.len()];
                self.render_studio.audio.selected=None;self.render_studio.audio.selected_track=None;
                self.render_studio.audio=StudioAudioRuntime::default();
                self.render_studio.audio.preview=None;self.render_studio.audio.pending_import=None;
                for path in self.render_studio.timeline.audio_tracks.iter().flat_map(|t|&t.clips).map(|c|c.path.clone()).collect::<Vec<_>>() {
                    self.render_studio.audio.request(&path);
                }
                self.render_studio.timeline.selected_demo=None;
                self.render_studio.initial_clip_seeded = true;
                self.render_studio.timeline.lanes=project.lanes;
                self.render_studio.timeline.lane_models=project.lane_models;
                self.render_studio.timeline.lane_models.resize(project.lanes,String::new());
                self.render_studio.timeline.lane_sabers=project.lane_sabers;
                self.render_studio.timeline.lane_sabers.resize(project.lanes,None);
                self.render_studio.timeline.lane_char_colors=project.lane_char_colors;
                self.render_studio.timeline.lane_char_colors.resize(project.lanes,None);
                self.render_studio.timeline.lane_visible=project.lane_visible;
                self.render_studio.timeline.lane_visible.resize(project.lanes,true);
                self.render_studio.timeline.lane_speed_expanded=vec![false;project.lanes];
                self.render_studio.timeline.lane_pinned=vec![false;project.lanes];
                self.render_studio.timeline.demo_speed_expanded=Vec::new();
                self.render_studio.timeline.demo_layer_collapsed=Vec::new();
                self.render_studio.timeline.demo_layer_pinned=Vec::new();
                self.render_studio.timeline.demo_layer_hidden=project.demo_layer_hidden.clone();
                self.render_studio.timeline.speed_drag=None;
                self.render_studio.timeline.in_point=project.in_point;
                self.render_studio.timeline.out_point=project.out_point;
                self.render_studio.timeline.interpolation=project.interpolation;
                self.render_studio.timeline.drag=None;
                self.render_studio.timeline.selected_motion=None;
                self.render_studio.timeline.camera = project.camera;
                self.render_studio.timeline.director_shots = project.director_shots;
                self.render_studio.timeline.normalize_director_ids();
                self.render_studio.director_selected = None;
                self.render_studio.timeline.camera_rig=project.camera_rig;
                self.render_studio.camera_rig_runtime=Default::default();
                self.render_studio.camera_pending_match=None;
                self.render_studio.spatial=Default::default();
                self.render_studio.timeline.camera.sort_by(|a,b|a.time.total_cmp(&b.time));
                self.render_studio.timeline.camera.dedup_by(|a,b|(a.time-b.time).abs()<0.00001);
                self.render_studio.timeline.selected_clip = None;
                self.render_studio.timeline.selected_camera = None;
                self.render_studio.timeline.clear_selection();
                self.render_studio.timeline.seek(0.0);
                self.render_studio.model = project.model;
                self.render_studio.with_sabers = project.with_sabers;
                self.render_studio.background = match project.background.as_str() {
                    "green" => StudioBackground::Green, "blue" => StudioBackground::Blue,
                    "levelshot" => StudioBackground::Levelshot,
                    "bsp" => StudioBackground::Bsp, _ => StudioBackground::Black,
                };
                self.render_studio.background_map = project.background_map;
                let cfg_error = (!project.render_cfg.is_empty()
                    && project.render_cfg != self.render_studio.graphics_profile)
                    .then(|| self.studio_cfg_select(&project.render_cfg).err()).flatten();
                self.render_studio_update_backdrop();
                self.profile_preview_key = None;
                self.render_studio.project_name=path.parent().and_then(|p|p.file_name())
                    .map(|s|s.to_string_lossy().into_owned()).unwrap_or_else(||"timeline_project".into());
                self.render_studio.saved_project_name=self.render_studio.project_name.clone();
                self.render_studio.save_as_name.clear();
                self.render_studio.director_paths=Default::default();
                self.render_studio.history=StudioEditHistory::default();
                self.render_studio.status = match cfg_error {
                    Some(error) => format!("Loaded {}, but its graphics profile failed: {error}", path.display()),
                    None => format!("Loaded {}", path.display()),
                };
            }
            Err(error) => self.render_studio.status = format!("Could not load timeline: {error}"),
        }
    }
}

impl App {
    pub(in crate::app) fn open_render_studio(&mut self) {
        self.render_studio.cinematic_preview=None;
        self.render_studio.spatial.drag=None;
        self.cancel_render_studio_export();
        self.render_studio.still = None;
        crate::emotes::stop();
        if self.render_studio.model.is_empty() {
            self.render_studio.model = self.solo_client_info.model_cvar();
        }
        self.render_studio.playing = false;
        self.render_studio.playhead = 0.0;
        self.render_studio.timeline.playing = false;
        self.render_studio.status.clear();
        self.render_studio.bsp_world = None;
        let fresh = self.render_studio.timeline.clips.is_empty() && self.render_studio.timeline.demos.is_empty()
            && self.render_studio.timeline.camera.len() == 1 && self.render_studio.timeline.camera[0].fov == 58.0;
        if fresh { self.render_studio.timeline.camera[0].fov = self.studio_default_fov(); }
        self.studio_cfg_enter();
        self.release_frontend_world_for_studio();
        self.render_studio.scene_capacity = usize::MAX;
        self.render_studio.oom_retried.clear();
        self.frontend_page = FrontendPage::RenderStudio;
        self.ensure_profile_catalog();
        self.sync_profile_preview_mode(true);
        self.profile_preview_key = None;
        self.render_command(RenderCommand::SetAssetPreviewMode(true));
        self.render_studio_update_backdrop();
        self.egui_repaint_requested = true;
    }

    /// The Studio never draws the main-menu 3D background; its demo views own
    /// separate scenes. Drop that world (GPU and prepared CPU copy) while the
    /// Studio is open so it does not sit behind every retained demo scene.
    fn release_frontend_world_for_studio(&mut self) {
        if self.render_studio.frontend_world_released || !self.front_end || self.game_session.is_some() {
            return;
        }
        // A frontend preparation still in flight must not upload behind the Studio.
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.frontend_background_request_id = None;
        self.deferred_frontend_map = None;
        self.frontend_cinematic = None;
        self.prepared_map_cache = None;
        self.render_command(RenderCommand::UnloadMap);
        self.render_studio.frontend_world_released = true;
    }

    /// Every route out of the Studio page: back on a menu page, the 3D
    /// background is requested again. A launch into a demo/map loads its own.
    pub(in crate::app) fn restore_frontend_world_after_studio(&mut self) {
        if !self.render_studio.frontend_world_released
            || (self.front_end && self.frontend_page == FrontendPage::RenderStudio) {
            return;
        }
        self.render_studio.frontend_world_released = false;
        if self.front_end && self.loading.is_none() && self.game_session.is_none() {
            self.request_frontend_background();
        }
    }

    /// Unattended presentation states for UI screenshots, e.g.
    /// `uipage renderstudio project=C:/x/test.json tab=demos select=demo:0 seek=2.5 render`.
    pub(in crate::app) fn render_studio_ui_script(&mut self, args: &[&str]) {
        // Scripted edits arrive without UI input; observe them next frame.
        self.render_studio.history.dirty = true;
        for arg in args {
            let (key, value) = arg.split_once('=').unwrap_or((arg, ""));
            let (kind, index) = value.split_once(':').unwrap_or((value, "0"));
            let index: usize = index.parse().unwrap_or(0);
            match key.to_ascii_lowercase().as_str() {
                "project" => self.load_render_studio_project_path(std::path::Path::new(value)),
                "tab" => self.render_studio.media_tab = match value {
                    "fortnite" => StudioMediaTab::Fortnite,
                    "cinematics" => StudioMediaTab::Cutscenes,
                    "demos" => StudioMediaTab::Demos,
                    _ => StudioMediaTab::Jka,
                },
                "seek" => self.render_studio.timeline.seek(value.parse().unwrap_or(0.0)),
                "select" => {
                    let timeline = &mut self.render_studio.timeline;
                    timeline.clear_selection();
                    self.render_studio.selected_background = false;
                    match kind {
                        "clip" => timeline.select_item(StudioItem::Clip(index), false, false),
                        "camera" => timeline.select_item(StudioItem::Camera(index), false, false),
                        "motion" => timeline.select_item(StudioItem::Motion(index, 0), false, false),
                        "demo" => timeline.selected_demo = Some(index),
                        "demoaudio" => {
                            timeline.selected_demo = Some(index);
                            self.render_studio.audio.selected_demo_audio = Some(index);
                        }
                        "audio" => self.render_studio.audio.selected_track = Some(index),
                        "background" => self.render_studio.selected_background = true,
                        "director" => self.render_studio.director_selected =
                            self.render_studio.timeline.director_shots.get(index).map(|s| s.id),
                        _ => {}
                    }
                }
                "speed" => self.render_studio.timeline.toggle_speed_track(index),
                "preview" => {
                    let source = self.render_studio.cinematics.catalog.as_ref().and_then(|c| c.as_ref().ok())
                        .and_then(|c| c.clips.get(index).map(|clip| clip.source.clone()));
                    if let Some(source) = source { self.preview_studio_cinematic(&source); }
                }
                "render" => self.render_studio.show_render = true,
                "graphics" => self.render_studio.graphics_settings_open = true,
                "leave" => self.render_studio.confirm_leave = true,
                // adddemo=<name>:<layer> drops a demo (base game folder) at the playhead.
                "adddemo" => {
                    let time = self.render_studio.timeline_time();
                    self.render_studio_queue_demo_drop(kind.to_owned(), String::new(), time, index);
                }
                // combatview = Director > Generate combat view on all demo clips.
                "combatview" => {
                    let targets = (0..self.render_studio.timeline.demos.len()).collect();
                    self.studio_start_combat_view(targets);
                }
                // routeview = Director > Generate route view on all demo clips.
                "routeview" => {
                    let targets = (0..self.render_studio.timeline.demos.len()).collect();
                    self.studio_start_route_view(targets);
                }
                // route = Director > Generate route comparison on the timeline's demos.
                "route" => {
                    let inputs = self.studio_route_inputs();
                    self.studio_start_route_job(inputs);
                }
                // shot=orbit:3 adds a live Director shot on client 3 at the playhead.
                "shot" => {
                    use super::render_studio_director::StudioDirectorPreset as P;
                    let preset = P::FOLLOW.iter().chain(P::WORLD.iter()).copied().find(|p| {
                        let label = p.label().to_ascii_lowercase().replace([' ', '-'], "");
                        label == kind || (kind == "eyes" && *p == P::PlayerEyes) || (kind == "pov" && *p == P::RecordedPov)
                    }).unwrap_or(P::Chase);
                    let time = self.render_studio.timeline_time();
                    if let Some(i) = self.studio_add_director_shot(preset, index as i32, time) {
                        self.render_studio.timeline.clear_selection();
                        self.render_studio.director_selected = Some(self.render_studio.timeline.director_shots[i].id);
                    }
                }
                "models" => self.render_studio.model_picker_lane = Some(index),
                "open" => self.render_studio.show_open_project = true,
                "saveas" => self.render_studio.show_save_as = true,
                "collapse" => match value {
                    "library" => self.render_studio.panels.library_collapsed = true,
                    "inspector" => self.render_studio.panels.inspector_collapsed = true,
                    _ => {}
                },
                _ => {}
            }
        }
        self.profile_preview_key = None;
        self.egui_repaint_requested = true;
    }

    /// The player's own cg_fov: the Studio's default lens instead of a fixed 58.
    pub(in crate::app) fn studio_default_fov(&self) -> f32 {
        self.console_cvar_value("cg_fov").and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite()).unwrap_or(80.0).clamp(10.0, 140.0)
    }

    /// Collision of the world behind the actors: the demo under the
    /// playhead (when loaded), else the 3D BSP background.
    pub(in crate::app) fn studio_floor_collision(&self) -> Option<jka_movement::CollisionWorld> {
        let time = self.render_studio.timeline_time();
        if let Some(clip) = self.render_studio.timeline.demo_at(time) {
            if let Some(world) = self.render_studio.sources.iter()
                .find(|s| s.name == clip.name && s.game == clip.game && s.collision.is_some())
                .and_then(|s| s.collision.clone()) { return Some(world); }
        }
        self.render_studio.bsp_world.as_ref().and_then(|w| w.prepared.as_ref()).and_then(|p| p.collision.clone())
    }

    /// Drop to floor: move each motion key up or down so the actor stands
    /// on the floor under it (player hull trace against the map collision).
    pub(in crate::app) fn studio_drop_to_floor(&mut self, keys: Vec<(usize, usize)>) {
        let Some(mut world) = self.studio_floor_collision() else {
            self.render_studio.status = "Drop to floor needs a loaded BSP background, or demo footage under the playhead".into();
            return;
        };
        let zoom = self.profile_preview_zoom.clamp(0.45, 2.5);
        let (mut moved, mut missed) = (0, 0);
        for (i, k) in keys {
            let Some(clip) = self.render_studio.timeline.clips.get_mut(i) else { continue };
            let lane = clip.lane;
            let Some(key) = clip.motion.get_mut(k) else { continue };
            // Same native mapping as the stage (render_studio_spatial::motion_world).
            let base_z = -22.0 + lane as f32 * 26.0;
            let origin = [88.0 * zoom + key.position[0], key.position[1], base_z + key.position[2]];
            match studio_floor_origin_z(&mut world, origin) {
                Some(z) => { key.position[2] = z - base_z; moved += 1; }
                None => missed += 1,
            }
        }
        self.profile_preview_key = None;
        self.render_studio.status = match (moved, missed) {
            (0, 0) => "Nothing to drop: select an animation clip or motion key".into(),
            (_, 0) => format!("Dropped {moved} motion key{} to the floor", if moved == 1 { "" } else { "s" }),
            (0, _) => "No floor found under the actor (outside the map?)".into(),
            _ => format!("Dropped {moved} motion keys; {missed} had no floor under them"),
        };
    }

    /// Every motion key of these clips.
    pub(in crate::app) fn studio_drop_clips_to_floor(&mut self, clips: Vec<usize>) {
        let keys = clips.into_iter().flat_map(|i| {
            let count = self.render_studio.timeline.clips.get(i).map_or(0, |c| c.motion.len());
            (0..count).map(move |k| (i, k))
        }).collect();
        self.studio_drop_to_floor(keys);
    }

    /// Smooth both transitions of clip `i` with its touching neighbors.
    pub(in crate::app) fn studio_smooth_clip_transitions(&mut self, i: usize, seconds: f64) {
        let (prev, next) = self.render_studio.timeline.touching_clips(i);
        let clips = &mut self.render_studio.timeline.clips;
        if let Some(p) = prev { clips[p].blend.blend_out = seconds; clips[i].blend.blend_in = seconds; }
        if let Some(n) = next { clips[n].blend.blend_in = seconds; clips[i].blend.blend_out = seconds; }
        self.render_studio.status = if prev.is_none() && next.is_none() {
            "No clip touches this one on its track: place clips back to back to smooth between them".into()
        } else { format!("Smoothed transitions ({seconds:.2} s each side)") };
        self.profile_preview_key = None;
    }

    /// New Director shot at `time`, using the player's cg_fov as its lens.
    pub(in crate::app) fn studio_add_director_shot(&mut self,
        preset: super::render_studio_director::StudioDirectorPreset, client: i32, time: f64) -> Option<usize> {
        use super::render_studio_director::StudioShotLength;
        let fov = self.studio_default_fov();
        // Over animation footage, follow the selected clip's character track.
        let lane = self.render_studio.timeline.selected_clip
            .and_then(|i| self.render_studio.timeline.clips.get(i)).map(|c| c.lane);
        let (clip_start, clip_end, _) = self.render_studio.timeline.footage_at(time, lane)?;
        let index = self.render_studio.timeline.add_director_shot_on(preset, client, time, lane)?;
        let shot = &mut self.render_studio.timeline.director_shots[index];
        let actor_lane = shot.actor_lane;
        preset.seed_shot(shot, fov);
        shot.actor_lane = actor_lane;
        match self.render_studio.director_new_length {
            StudioShotLength::ToClipEnd => shot.duration = (clip_end - time).max(0.1),
            StudioShotLength::WholeClip => { shot.start = clip_start; shot.duration = (clip_end - clip_start).max(0.1); }
            StudioShotLength::FourSeconds => {}
        }
        Some(index)
    }

    /// The authored camera at `time`: demo footage uses the native sampler's
    /// key (Director shot or keyed camera); the character stage evaluates
    /// stage Director shots against the actors' motion.
    pub(in crate::app) fn studio_camera_key_at(&self, time: f64) -> super::render_studio_timeline::StudioCameraKey {
        let timeline = &self.render_studio.timeline;
        if timeline.demo_at(time).is_some() { return timeline.effective_demo_camera_at(time); }
        timeline.stage_camera_at(time, self.profile_preview_zoom.clamp(0.45, 2.5), self.profile_preview_yaw)
            .unwrap_or_else(|| timeline.camera_at(time))
    }

    /// Auto Director over animation clips (character stage), synchronous.
    pub(in crate::app) fn studio_direct_stage_clips(&mut self, clips: Vec<usize>) {
        use super::render_studio_auto_director::{stage_summary, StageDirector};
        let (zoom, yaw, fov) = (self.profile_preview_zoom.clamp(0.45, 2.5), self.profile_preview_yaw, self.studio_default_fov());
        let (pacing, seed) = (self.render_studio.director_pacing, self.render_studio.director_seed);
        let timeline = &self.render_studio.timeline;
        let clips: Vec<usize> = clips.into_iter().filter(|&i| i < timeline.clips.len()).collect();
        if clips.is_empty() { self.render_studio.status = "Select an animation or emote clip first".into(); return; }
        let ranges: Vec<(f64, f64, usize)> = clips.iter().map(|&i| {
            let c = &timeline.clips[i];
            (c.start, c.end(), c.lane)
        }).collect();
        let shots = {
            let mut director = StageDirector::new(timeline, zoom, yaw, fov, pacing, seed);
            clips.iter().flat_map(|&i| director.direct_clip(i)).collect::<Vec<_>>()
        };
        let timeline = &mut self.render_studio.timeline;
        let demo_free: Vec<bool> = timeline.director_shots.iter()
            .map(|s| timeline.demo_at(s.start + s.duration * 0.5).is_none()).collect();
        let mut keep = demo_free.iter();
        timeline.director_shots.retain(|s| {
            let stage = *keep.next().unwrap_or(&false);
            !(stage && ranges.iter().any(|&(a, b, lane)| s.actor_lane == lane && s.start < b - 1e-6 && s.end() > a + 1e-6))
        });
        let summary = stage_summary(&shots, clips.len(), seed);
        for mut shot in shots {
            shot.id = timeline.next_director_id();
            timeline.director_shots.push(shot);
        }
        if let Some(&(start, _, _)) = ranges.first() { timeline.seek(start); }
        self.render_studio.director_seed = seed.wrapping_add(1);
        self.render_studio.director_selected = None;
        self.render_studio.director_multi.clear();
        self.profile_preview_key = None;
        self.render_studio.status = summary;
    }

    /// Regenerate / re-place stage shots in place (synchronous).
    pub(in crate::app) fn studio_regenerate_stage(&mut self, ids: &[u64],
        keep: Option<super::render_studio_director::StudioDirectorPreset>) -> usize {
        use super::render_studio_auto_director::StageDirector;
        let (zoom, yaw, fov) = (self.profile_preview_zoom.clamp(0.45, 2.5), self.profile_preview_yaw, self.studio_default_fov());
        let pacing = self.render_studio.director_pacing;
        let timeline = &self.render_studio.timeline;
        let mut director = StageDirector::new(timeline, zoom, yaw, fov, pacing, 0);
        let shots = &timeline.director_shots;
        let replaced: Vec<_> = ids.iter().filter_map(|id| {
            let shot = shots.iter().find(|s| s.id == *id)?;
            let prev = shots.iter().filter(|s| s.id != shot.id && s.end() <= shot.start + 0.05)
                .max_by(|a, b| a.end().total_cmp(&b.end())).map(|s| s.preset);
            let next = shots.iter().filter(|s| s.id != shot.id && s.start >= shot.end() - 0.05)
                .min_by(|a, b| a.start.total_cmp(&b.start)).map(|s| s.preset);
            director.regenerate(shot, [prev, next], keep)
        }).collect();
        let count = replaced.len();
        for new in replaced {
            if let Some(slot) = self.render_studio.timeline.director_shots.iter_mut().find(|s| s.id == new.id) { *slot = new; }
        }
        self.profile_preview_key = None;
        count
    }

    /// Default camera over demo footage: third person on the recorded player
    /// (follows the recorder's spectator switches).
    pub(in crate::app) fn studio_default_demo_camera(key: &mut StudioCameraKey) {
        use super::render_studio_timeline::{StudioCameraSource, StudioDemoPov, StudioFollowPerspective};
        key.source = StudioCameraSource::Demo { pov: StudioDemoPov::Follow, client: -1 };
        key.demo_follow.perspective = StudioFollowPerspective::ThirdPerson;
        if key.demo_follow.range < 40.0 { key.demo_follow.range = 100.0; }
    }

    pub(in crate::app) fn render_studio_update_backdrop(&mut self) {
        // The isolated character stage and native demo scenes are separate.
        // Only the 3D background needs a retained map; selecting another
        // backdrop releases its GPU scene without touching any demo source.
        let bsp = self.render_studio.background == StudioBackground::Bsp;
        let selection = self.render_studio.background_map.clone();
        if self.render_studio.bsp_world.as_ref().is_some_and(|world|
            !bsp || world.map != selection) {
            if let Some(world) = self.render_studio.bsp_world.take() {
                self.render_command(RenderCommand::RemoveStudioSource(world.id));
            }
        }
        if bsp {
            self.profile_background_requested = None;
            self.render_command(RenderCommand::SetAssetPreviewBackground(None));
            if selection.is_empty() || self.render_studio.bsp_world.is_some() { return; }
            // No synchronous map parsing, GPU upload, or gameplay map switch.
            let id = self.render_studio.next_source_id;
            self.render_studio.next_source_id = id.wrapping_add(1)
                | super::render_studio_demo::STUDIO_REQUEST_BIT;
            let progress = std::sync::Arc::new(crate::runtime::LoadProgress::default());
            progress.set("Preparing BSP background", 0, 0);
            self.render_studio.bsp_world = Some(StudioBspBackdrop {
                id, map: selection.clone(), ready: false,
                progress, prepared: None, inline_models: std::sync::Arc::new(Vec::new()),
                error: None,
            });
            if self.loader_tx.send(super::map_loading::MapLoadRequest {
                request_id: id, started: Instant::now(), game: self.game.clone(),
                source: crate::scene::MapSource::Bsp(selection), sub_bsp_names: Vec::new(),
                prepare_options: self.current_map_prepare_options(),
                analyze_fx_light_profiles: false, supersedable: false, seed: None,
            }).is_err() {
                self.render_studio.status = "Could not queue BSP background map loading".into();
                self.render_studio.bsp_world = None;
            }
            return;
        }
        if self.render_studio.background == StudioBackground::Black {
            self.profile_background_requested = None;
            self.render_command(RenderCommand::SetAssetPreviewCheckerboard);
            return;
        }
        let color: Option<[u8; 4]> = match self.render_studio.background {
            StudioBackground::Black => unreachable!(),
            StudioBackground::Green => Some([0, 177, 48, 255]),
            StudioBackground::Blue => Some([14, 54, 239, 255]),
            StudioBackground::Levelshot => None,
            StudioBackground::Bsp => unreachable!(),
        };
        if let Some(color) = color {
            self.profile_background_requested = None;
            self.render_command(RenderCommand::SetAssetPreviewBackground(Some(([2, 2], color.repeat(4)))));
        } else {
            self.render_command(RenderCommand::SetAssetPreviewBackground(None));
            if !self.render_studio.background_map.is_empty() {
                let map = self.render_studio.background_map.clone();
                self.profile_background_requested = Some(map.clone());
                self.ui_catalog.request(&self.base, self.game.as_deref(),
                    crate::app::ui_catalog::CatalogRequest::ProfileBackground { map_name: map });
            }
        }
    }

    pub(in crate::app) fn studio_bsp_prepared(&mut self, id: u64, map: Box<crate::scene::PreparedMap>) {
        let Some(world) = self.render_studio.bsp_world.as_mut().filter(|w| w.id == id) else {return};
        let prepared = std::sync::Arc::new(*map);
        world.inline_models = std::sync::Arc::new(prepared.inline_models.iter().map(|model|
            crate::renderer::InlineModelInstance {
                model: model.model, origin: [0.0;3],
                axis: [[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0]],
                is_portal_entity: false,
            }).collect());
        world.prepared = Some(std::sync::Arc::clone(&prepared));
        world.progress.set("Uploading BSP background", 0, 0);
        let progress=std::sync::Arc::clone(&world.progress);
        self.render_command(RenderCommand::LoadStudioSource {
            id, game: self.game.clone(), map: Box::new(prepared.as_ref().clone()),
            video: self.video, progress,
        });
    }

    pub(in crate::app) fn studio_bsp_ready(&mut self, id: u64, error: Option<String>) {
        if let Some(world) = self.render_studio.bsp_world.as_mut().filter(|w| w.id == id) {
            world.ready = error.is_none();
            world.error = error.clone();
            self.render_studio.status = error.map_or_else(
                || format!("BSP background ready: {}", world.map),
                |error| format!("BSP background failed: {error}"));
            self.egui_repaint_requested = true;
        }
    }

    pub(in crate::app) fn studio_bsp_active(&self) -> bool {
        self.front_end && self.frontend_page == FrontendPage::RenderStudio
            && self.profile_preview_active
            && self.render_studio.background == StudioBackground::Bsp
            && self.render_studio.timeline.demo_at(self.render_studio.timeline_time()).is_none()
            && self.render_studio.bsp_world.as_ref().is_some_and(|world|world.ready)
    }

    pub(in crate::app) fn start_render_studio_export(&mut self) {
        self.render_studio.cinematic_preview=None;
        if self.render_studio.export.is_some() || self.render_studio.still.is_some()
            || self.render_studio.dm26_capture.is_some() || self.render_studio.dm26_export.is_some() { return; }
        if self.render_studio.timeline.clips.is_empty() && self.render_studio.timeline.demos.is_empty() && self.render_studio.timeline.audio_tracks.iter().all(|t|t.clips.is_empty()) {
            self.render_studio.status = "Add an animation or demo clip before exporting".into();
            return;
        }
        let Some(ffmpeg) = adjacent_ffmpeg_path() else {
            self.render_studio.status = "Place ffmpeg.exe next to DinurdoJK.exe".into();
            return;
        };
        if let Some(codec) = self.render_studio.preset.nvenc_codec() {
            if let Err(error) = studio_probe_encoder(&ffmpeg, codec) {
                self.render_studio.status = format!("{} is unavailable ({error}). Pick an x264 format or update the NVIDIA driver / FFmpeg build.",
                    self.render_studio.preset.label());
                self.push_console_line(format!("^1RENDER STUDIO: {}", self.render_studio.status));
                return;
            }
        }
        // One frame for each output timestamp in [start, end); no doubled final pose.
        let fps = self.render_studio.fps;
        let (in_time, out_time) = self.render_studio.timeline.export_range();
        let total_frames = ((out_time-in_time) * fps as f64).ceil().max(1.0) as u64;
        if total_frames > 120_000 {
            self.render_studio.status = "Trim the clip to 120,000 output frames or fewer".into();
            return;
        }
        let folder = self.studio_project_directory().join("exports");
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.render_studio.status = format!("Output directory: {error}");
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let output = folder.join(format!("studio_{stamp}.{}", self.render_studio.preset.extension()));
        let alpha = self.render_studio.transparent;
        if alpha && (!self.render_studio.timeline.demos.is_empty() || self.render_studio.background == StudioBackground::Bsp) {
            self.render_studio.status = "Transparent BSP/demo output is not supported; select Empty background or disable Transparent video".into();
            return;
        }
        if alpha && !self.render_studio.preset.supports_alpha() {
            self.render_studio.status = "This video format does not support transparency".into();
            return;
        }
        let depth = self.render_studio.export_depth;
        if depth && self.render_studio.timeline.demos.iter().any(|c|
            c.transition_in != super::render_studio_timeline::StudioDemoTransition::Cut) {
            self.render_studio.status = "Depth EXR export cannot blend two different demo worlds; disable depth for demo transitions".into();
            return;
        }
        let depth_dir = depth.then(|| folder.join(format!("studio_{stamp}_depth")));
        if let Some(dir) = depth_dir.as_ref() {
            if let Err(error) = std::fs::create_dir_all(dir) {
                self.render_studio.status = format!("Could not create EXR depth directory: {error}");
                return;
            }
        }
        // Audio is evaluated directly at absolute output sample timestamps.
        // No speaker-loopback capture or wall-clock dependency.
        let native_audio=self.render_studio.preset!=StudioVideoFormat::Gif &&
            !self.render_studio.timeline.demos.is_empty();
        let audio_wav=if !native_audio && self.render_studio.timeline.audio_tracks.iter().any(|t|!t.clips.is_empty()) &&
            self.render_studio.preset!=StudioVideoFormat::Gif {
            if let Err(error)=self.render_studio.audio.all_ready(&self.render_studio.timeline.audio_tracks) {
                self.render_studio.status=format!("Cannot export audio: {error}");return;
            }
            let wav=folder.join(format!("studio_{stamp}_audio.wav"));
            let frames=((total_frames as u128 * super::render_studio_audio::RATE as u128 + fps as u128/2) / fps as u128) as u64;
            if let Err(error)=self.render_studio.audio.write_wav(&self.render_studio.timeline.audio_tracks,in_time,frames,&wav) {
                let _=std::fs::remove_file(&wav);
                self.render_studio.status=format!("Audio mix failed: {error}");return;
            }
            Some(wav)
        } else {None};
        // A native demo has no baked PCM. Validate external samples now, then
        // capture native voices as the exporter walks its timeline. This avoids
        // creating a second demo replay, phantom sound streams or speaker capture.
        let frames=((total_frames as u128 * super::render_studio_audio::RATE as u128 + fps as u128/2) / fps as u128) as u64;
        let (native_audio_file,audio_wav,encoded_video)=if native_audio {
            if let Err(error)=self.render_studio.audio.all_ready(&self.render_studio.timeline.audio_tracks) {
                self.render_studio.status=format!("Cannot export imported audio: {error}");return;
            }
            let wav=folder.join(format!("studio_{stamp}_linked_audio.wav"));
            let file=match super::render_studio_audio::new_capture_wav(&wav,frames) {
                Ok(file)=>file,Err(error)=>{
                    self.render_studio.status=format!("Cannot create linked audio WAV: {error}");return;
                }
            };
            let intermediate=folder.join(format!("studio_{stamp}_video.{}",self.render_studio.preset.extension()));
            (Some(file),Some(wav),Some(intermediate))
        }else{(None,audio_wav,None)};
        self.render_studio.audio.preview=None;
        self.render_studio.timeline.seek(in_time);
        self.render_studio.preview_sample_time = None;
        self.render_studio.playing = false;
        // Offscreen: one frame being built, one recording, one on the GPU.
        let max_in_flight = if self.render_studio.offscreen_output_size().is_some() { 3 } else { 1 };
        self.render_studio.export = Some(RenderStudioExport {
            frame_index: 0, requested: 0, max_in_flight, total_frames, fps,
            output: output.clone(), child: None, stdin: None, preset: self.render_studio.preset,
            encoder_speed: self.render_studio.encoder_speed,
            nvenc_preset: self.render_studio.nvenc_preset,
            crf: self.render_studio.crf,
            alpha, depth, depth_dir, depth_child: None, depth_stdin: None, audio_wav,
            native_audio, native_audio_file, audio_frames_written:0,
            audio_frames_target:frames, export_in_time:in_time, encoded_video,
            profile: super::render_studio_profile::StudioExportProfile::new(max_in_flight),
            frame_size: None,
        });
        self.set_studio_capture_size(self.render_studio.offscreen_output_size());
        self.profile_preview_key = None;
        self.render_studio.status = format!("Rendering {total_frames} clean frames to {}", output.display());
        self.push_console_line(format!("^2RENDER STUDIO:^7 {}", output.display()));
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn start_render_studio_screenshot(&mut self) {
        self.render_studio.cinematic_preview=None;
        if self.render_studio.export.is_some() || self.render_studio.still.is_some() || self.render_studio.dm26_capture.is_some() { return; }
        let folder = self.studio_project_directory().join("exports");
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.render_studio.status = format!("Screenshot output directory: {error}");
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let output = folder.join(format!("studio_{stamp}.png"));
        if self.render_studio.screenshot_depth && adjacent_ffmpeg_path().is_none() {
            self.render_studio.status = "ffmpeg.exe required next to game for depth EXR".into();
            return;
        }
        let current_time = self.render_studio.timeline_time();
        if self.render_studio.screenshot_depth && self.render_studio.timeline.demo_blend_at(current_time).is_some() {
            self.render_studio.status="Depth EXR cannot blend two demo worlds; seek outside the transition or disable depth".into();return;
        }
        self.render_studio.timeline.seek(current_time);
        self.render_studio.playhead = self.render_studio.offset_frames();
        self.render_studio.playing = false;
        let demo_still = self.render_studio.timeline.demo_at(current_time).is_some();
        let bsp_still = self.render_studio.background == StudioBackground::Bsp && !demo_still;
        self.set_studio_capture_size((demo_still || bsp_still).then(|| self.render_studio.export_size.dimensions()).flatten());
        self.render_studio.still = Some(RenderStudioStill {
            awaiting_frame: false, output,
            alpha: self.render_studio.screenshot_alpha && !demo_still && !bsp_still,
            depth: self.render_studio.screenshot_depth,
        });
        self.profile_preview_key = None;
        self.egui_repaint_requested = true;
    }

    fn set_render_studio_capture_viewport(&mut self) {
        if let Some(window) = self.window.as_ref() {
            let size = window.inner_size();
            let viewport = [0, 0, size.width.max(1), size.height.max(1)];
            if self.asset_preview_viewport_key != Some(viewport) {
                self.asset_preview_viewport_key = Some(viewport);
                self.render_command(RenderCommand::SetAssetPreviewViewport(Some(viewport)));
            }
        }
    }

    /// Fixed offscreen export size (or `None` for window/viewport capture).
    fn set_studio_capture_size(&mut self, size: Option<[u32;2]>) {
        self.render_studio.capture_size = size;
        self.profile_preview_key = None;
        self.render_command(RenderCommand::SetStudioExportSize(size));
    }

    fn reset_render_studio_capture_viewport(&mut self) {
        if self.render_studio.capture_size.is_some() { self.set_studio_capture_size(None); }
        self.asset_preview_viewport_key = None;
        self.render_command(RenderCommand::SetAssetPreviewViewport(None));
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn tick_render_studio_export(&mut self) {
        self.poll_studio_export_finish();
        self.tick_render_studio_dm26_export();
        if let Some(still) = self.render_studio.still.as_ref() {
            if still.awaiting_frame { return; }
            let demo = self.render_studio.timeline.demo_at(self.render_studio.timeline_time()).is_some();
            let bsp = self.render_studio.background == StudioBackground::Bsp && !demo;
            if bsp && self.render_studio.background_map.is_empty() {
                self.fail_render_studio_export("Select a BSP map in Background before capturing");
                return;
            }
            if self.frontend_page != FrontendPage::RenderStudio || (!demo && !self.profile_preview_active) {
                self.render_studio.still = None;
                self.reset_render_studio_capture_viewport();
                return;
            }
            let depth = still.depth;
            let alpha = still.alpha;
            if demo || bsp {
                if bsp {
                    if let Some(error)=self.render_studio.bsp_world.as_ref().and_then(|w|w.error.clone()) {
                        self.fail_render_studio_export(error);
                        return;
                    }
                }
                if !(if demo {self.studio_demo_scene_active()} else {self.studio_bsp_active()}) {
                    return; // Do not capture an uninitialized/stale world.
                }
                self.render_command(RenderCommand::SetAssetPreviewMode(false));
            } else {
                self.set_render_studio_capture_viewport();
                self.profile_preview_key = None;
                self.update_profile_preview();
            }
            // Offscreen captures are exactly the output; only window captures crop.
            self.render_studio.pending_capture_rect = (self.render_studio.capture_size.is_none() && (demo || bsp))
                .then_some(self.render_studio.demo_viewport).flatten();
            self.request_studio_capture(0, depth, alpha, demo || bsp);
            if let Some(still) = self.render_studio.still.as_mut() { still.awaiting_frame = true; }
            return;
        }
        let Some(export) = self.render_studio.export.as_ref() else { return; };
        if !export.can_request() { return; }
        if self.frontend_page != FrontendPage::RenderStudio {
            self.cancel_render_studio_export();
            return;
        }
        let at = self.render_studio.timeline_time();
        let demo = self.render_studio.timeline.demo_at(at).is_some();
        let bsp = self.render_studio.background == StudioBackground::Bsp && !demo;
        if bsp && self.render_studio.background_map.is_empty() {
            self.fail_render_studio_export("Select a BSP map in Background before exporting");
            return;
        }
        if demo || bsp {
            if bsp {
                if let Some(error)=self.render_studio.bsp_world.as_ref().and_then(|w|w.error.clone()) {
                    self.fail_render_studio_export(error);
                    return;
                }
            }
            if self.render_studio.demo_preview.as_ref().is_some_and(|p| p.failed) {
                self.fail_render_studio_export("demo clip could not load");
                return;
            }
            // During a clip switch the decoder and potentially the BSP upload
            // are pending. Do not emit stale frames from the preceding clip.
            if !(if demo {self.studio_demo_scene_active()} else {self.studio_bsp_active()}) {
                if let Some(export) = self.render_studio.export.as_mut() { export.profile.not_ready(); }
                return;
            }
            self.render_command(RenderCommand::SetAssetPreviewMode(false));
        } else {
            if !self.profile_preview_active {
                // The world->model switch is async. Do not abort or encode a
                // previous demo frame while the stage target comes online.
                if let Some(export) = self.render_studio.export.as_mut() { export.profile.not_ready(); }
                return;
            }
            self.set_render_studio_capture_viewport();
            self.profile_preview_key = None;
            self.update_profile_preview();
        }
        self.render_studio.pending_capture_rect = (self.render_studio.capture_size.is_none() && (demo || bsp))
            .then_some(self.render_studio.demo_viewport).flatten();
        let (frame_index, depth, alpha, native_audio) = self.render_studio.export.as_ref()
            .map_or((0, false, false, false), |e| (e.requested, e.depth, e.alpha, e.native_audio));
        // Pull this frame's demo audio now, while the demo sits at its time:
        // with frames in flight the demo moves on before this one is delivered.
        let audio_started = Instant::now();
        if native_audio {
            if let Err(error) = self.render_studio_write_native_audio(frame_index) {
                self.fail_render_studio_export(error);
                return;
            }
        }
        let audio_ms = audio_started.elapsed().as_secs_f64() * 1000.0;
        self.request_studio_capture(frame_index, depth, alpha, demo || bsp);
        if let Some(export) = self.render_studio.export.as_mut() {
            export.requested += 1;
            export.profile.requested(frame_index, audio_ms);
        }
    }

    /// Publish this tick's scene and request its capture. A fixed-size world
    /// capture carries exactly that scene (queued in the renderer), so the app
    /// may publish later frames before it is rendered; otherwise the renderer
    /// captures the window from the live snapshot.
    fn request_studio_capture(&mut self, frame_index: u64, depth: bool, alpha: bool, world: bool) {
        let started = Instant::now();
        let snapshot = self.render_snapshot();
        let scene = snapshot.studio_scene.clone();
        if let Some(render) = &self.render { render.publish(snapshot); }
        if let Some(export) = self.render_studio.export.as_mut() {
            export.app_cost(super::render_studio_profile::StudioAppCost::Snapshot,
                started.elapsed().as_secs_f64() * 1000.0);
        }
        match scene.filter(|_| world && self.render_studio.capture_size.is_some()) {
            Some(scene) => self.render_command(RenderCommand::CaptureStudioFrame { frame_index, depth, scene }),
            None => self.render_command(RenderCommand::CaptureVideoFrame { frame_index, depth, alpha }),
        }
    }

    pub(in crate::app) fn on_render_studio_still_frame(
        &mut self, index: u64, width: u32, height: u32, bgra: Vec<u8>, depth: Option<Vec<u8>>,
    ) {
        let (width,height,bgra,depth) = match studio_crop_capture(width,height,bgra,depth,
            self.render_studio.pending_capture_rect.take()) {
            Ok(frame)=>frame,
            Err(error)=>{self.fail_render_studio_export(error);return;}
        };
        let result = (|| -> Result<PathBuf, String> {
            let still = self.render_studio.still.as_ref().ok_or("No pending Render Studio screenshot")?;
            if !still.awaiting_frame || index != 0 { return Err("Unexpected screenshot frame".into()); }
            let output = still.output.clone();
            let alpha = still.alpha;
            let need_depth = still.depth;
            let mut rgba = Vec::with_capacity(bgra.len());
            for px in bgra.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], if alpha { px[3] } else { 255 }]);
            }
            let file = std::fs::File::create(&output).map_err(|e| format!("PNG create: {e}"))?;
            let writer = std::io::BufWriter::new(file);
            image::codecs::png::PngEncoder::new(writer)
                .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
                .map_err(|e| format!("PNG encode: {e}"))?;
            if need_depth {
                let pixels = depth.ok_or("Depth capture returned no depth pixels")?;
                let depth_path = output.with_file_name(format!("{}_depth.exr",
                    output.file_stem().and_then(|name| name.to_str()).unwrap_or("studio")));
                let ffmpeg = adjacent_ffmpeg_path().ok_or("ffmpeg.exe required to write depth EXR")?;
                let mut child = Command::new(ffmpeg)
                    .args(["-hide_banner", "-loglevel", "error", "-y",
                        "-f", "rawvideo", "-pix_fmt", "grayf32le"])
                    .arg("-s").arg(format!("{width}x{height}"))
                    .args(["-i", "pipe:0", "-frames:v", "1", "-c:v", "exr", "-compression", "zip16"])
                    .arg(&depth_path)
                    .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped())
                    .spawn().map_err(|e| format!("Depth FFmpeg spawn: {e}"))?;
                let mut stdin = child.stdin.take().ok_or("Depth FFmpeg stdin missing")?;
                stdin.write_all(&pixels).map_err(|e| format!("Depth EXR write: {e}"))?;
                drop(stdin);
                let done = child.wait_with_output().map_err(|e| format!("Depth EXR wait: {e}"))?;
                if !done.status.success() {
                    return Err(format!("Depth EXR encoder: {}", String::from_utf8_lossy(&done.stderr)));
                }
            }
            Ok(output)
        })();
        self.render_studio.still = None;
        self.reset_render_studio_capture_viewport();
        match result {
            Ok(path) => {
                self.render_studio.status = format!("Screenshot saved: {}", path.display());
                self.push_console_path_line(format!("^2{}", self.render_studio.status), path);
            }
            Err(error) => {
                self.render_studio.status = format!("Studio screenshot failed: {error}");
                self.push_console_line(format!("^1{}", self.render_studio.status));
            }
        }
    }

    /// Capture event audio at absolute output sample times. Each source owns
    /// an isolated software mixer, so even two overlapping demo worlds can
    /// contribute sounds without replacing the gameplay session/backend.
    fn render_studio_write_native_audio(&mut self,index:u64)->Result<(),String> {
        use super::render_studio_audio::RATE;
        let export=self.render_studio.export.as_ref().ok_or("No active export")?;
        if !export.native_audio {return Ok(());}
        if index!=export.requested {return Err("Audio/video frame index mismatch".into());}
        let cumulative=(((index+1)*RATE as u64)+export.fps as u64/2)/export.fps as u64;
        let end=cumulative.min(export.audio_frames_target);
        let written=export.audio_frames_written;
        let frames=end.saturating_sub(written) as usize;
        let begin=export.export_in_time+written as f64/RATE as f64;
        let any_demo_solo=self.render_studio.timeline.demos.iter().any(|c|c.audio.solo);
        let imported_solo=self.render_studio.timeline.audio_tracks.iter().any(|t|t.solo);
        // Preserve external track automation unless a linked demo is soloed.
        let mut output=if any_demo_solo {vec![0.0_f32;frames*2]}else{
            self.render_studio.audio.mix(&self.render_studio.timeline.audio_tracks,begin,frames)
        };
        for source in &mut self.render_studio.sources {
            let Some(clip)=self.render_studio.timeline.demos.get(source.clip_index) else {continue};
            let Some(mixer)=source.audio_capture.as_mut() else {
                // Warmed but unused sources need no sound, active frames do.
                if clip.start<=begin && begin<clip.end()
                    && self.render_studio.timeline.demo_top_layer_at(begin)==Some(clip.layer) {
                    return Err(format!("No native sound capture for {}",clip.name));
                }
                continue;
            };
            // Every active mixer must advance even when muted so the next
            // sample doesn't come from a stale pre-mute part of its timeline.
            let mut samples=Vec::with_capacity(frames*2);
            for _ in 0..frames*2 {samples.push(mixer.next().unwrap_or(0.0));}
            for i in 0..frames {
                let at=begin+i as f64/RATE as f64;
                // Only the top visible demo layer is heard (as it is seen).
                let active=clip.start<=at && at<clip.end()
                    && self.render_studio.timeline.demo_top_layer_at(at)==Some(clip.layer);
                if !active {continue;}
                let mut gain=if (any_demo_solo && !clip.audio.solo) || (imported_solo && !any_demo_solo) {
                    0.0
                }else{clip.audio_gain_at(at)};
                if let Some(blend)=self.render_studio.timeline.demo_blend_at(at) {
                    if source.clip_index==blend.outgoing {gain*=1.0-blend.progress;}
                    else if source.clip_index==blend.incoming {gain*=blend.progress;}
                    else {gain=0.0;}
                }else if self.render_studio.timeline.demos.iter().enumerate().rev()
                    .find(|(_,c)|c.start<=at && at<c.end()).map(|(i,_)|i)!=Some(source.clip_index) {
                    gain=0.0;
                }
                output[i*2]+=samples[i*2]*gain;
                output[i*2+1]+=samples[i*2+1]*gain;
            }
        }
        let file=self.render_studio.export.as_mut().and_then(|e|e.native_audio_file.as_mut())
            .ok_or("Native demo WAV is missing")?;
        let mut buf=Vec::with_capacity(frames*8);
        for sample in output {buf.extend_from_slice(&sample.clamp(-1.0,1.0).to_le_bytes());}
        file.write_all(&buf).map_err(|e|format!("Native demo audio write failed: {e}"))?;
        self.render_studio.export.as_mut().unwrap().audio_frames_written=end;
        Ok(())
    }

    pub(in crate::app) fn on_render_studio_frame(&mut self, index: u64, width: u32, height: u32, bgra: Vec<u8>, depth_f32le: Option<Vec<u8>>,
        timing: crate::runtime::VideoFrameTiming) {
        let received = Instant::now();
        let mut cost = super::render_studio_profile::StudioFrameAppCost::default();
        let crop_started = Instant::now();
        let (width,height,bgra,depth_f32le) = match studio_crop_capture(width,height,bgra,depth_f32le,
            self.render_studio.pending_capture_rect.take()) {
            Ok(frame)=>frame,
            Err(error)=>{self.fail_render_studio_export(error);return;}
        };
        cost.crop_ms = crop_started.elapsed().as_secs_f64()*1000.0;
        let (mut encode_pipe_ms, mut depth_pipe_ms) = (0.0, 0.0);
        let result = (|| -> Result<bool, String> {
            let export = self.render_studio.export.as_mut().ok_or("No studio export")?;
            if index != export.frame_index || export.frame_index >= export.requested {
                return Err(format!("Unexpected studio frame {index}, expected {}", export.frame_index));
            }
            // FFmpeg's raw stream has a fixed frame size: a different one would
            // silently shear every following frame, so stop instead.
            match export.frame_size {
                None => export.frame_size = Some((width, height)),
                Some((w, h)) if (w, h) != (width, height) => return Err(format!(
                    "frame {index} is {width}x{height} but the video is {w}x{h}. The capture source changed \
                    (window resized/restarted, or a stage-only frame in a fixed-resolution export: \
                    set Resolution to Viewer size for those)")),
                Some(_) => {}
            }
            if export.child.is_none() {
                let ffmpeg = adjacent_ffmpeg_path().ok_or("ffmpeg.exe disappeared")?;
                let mut command = Command::new(ffmpeg);
                command.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgra"])
                    .arg("-s").arg(format!("{width}x{height}"))
                    .arg("-r").arg(export.fps.to_string())
                    .args(["-i", "pipe:0"]);
                if !export.native_audio {
                    if let Some(wav)=export.audio_wav.as_ref() {command.arg("-i").arg(wav);}
                    else {command.arg("-an");}
                } else {command.arg("-an");}
                match export.preset {
                    StudioVideoFormat::WebmVp9 | StudioVideoFormat::WebmVp9Max => {
                        // VP9 alpha is an auxiliary stream in WebM: libvpx
                        // requires yuva420p and alternate references disabled.
                        // Preserve alpha even on hardware whose default FFmpeg
                        // conversion would otherwise flatten it to black.
                        let filter = if export.alpha {
                            "pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=yuva420p"
                        } else {
                            "pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p"
                        };
                        let crf = if export.preset == StudioVideoFormat::WebmVp9Max { "18" } else { "28" };
                        command.args(["-vf", filter, "-c:v", "libvpx-vp9",
                            "-pix_fmt", if export.alpha { "yuva420p" } else { "yuv420p" },
                            "-auto-alt-ref", "0", "-b:v", "0", "-crf", crf,
                            "-deadline", "good", "-cpu-used", "3", "-row-mt", "1"]);
                    }
                    StudioVideoFormat::Gif => {
                        // A fresh palette each frame permits *streaming* GIF
                        // encoding without retaining the entire animation in
                        // memory. GIF transparency is only binary; smooth alpha
                        // is quantized at a defined 128/255 threshold.
                        command.args(["-vf", "format=rgba,split[pal_in][gif_in];[pal_in]palettegen=stats_mode=single:reserve_transparent=1[pal];[gif_in][pal]paletteuse=new=1:dither=sierra2_4a:alpha_threshold=128",
                            "-gifflags", "+transdiff", "-loop", "0"]);
                    }
                    StudioVideoFormat::ProRes444 if export.alpha => {
                        command.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=yuva444p10le",
                            "-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le", "-alpha_bits", "16"]);
                    }
                    preset => {
                        // Preserve the original opaque H.264/ProRes encoding
                        // path and its BT.709/range tagging.
                        command.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2,scale=in_range=full:out_range=tv:out_color_matrix=bt709,setparams=range=limited:color_primaries=bt709:color_trc=iec61966-2-1:colorspace=bt709"])
                            .args(["-color_range", "tv", "-color_primaries", "bt709", "-color_trc", "iec61966-2-1", "-colorspace", "bt709"])
                            .args({
                                let mut args = studio_video_args(preset, export.encoder_speed, export.nvenc_preset, export.crf);
                                // An intermediate (remuxed with audio afterwards) gains
                                // nothing from faststart's second full-file rewrite.
                                if export.encoded_video.is_some() {
                                    if let Some(at) = args.iter().position(|arg| arg == "-movflags") {
                                        args.drain(at..(at + 2).min(args.len()));
                                    }
                                }
                                args
                            });
                    }
                }
                if export.audio_wav.is_some() && !export.native_audio {
                    command.args(export.preset.audio_ffmpeg_args());
                    command.args(["-map","0:v:0","-map","1:a:0"]);
                }
                command.arg(export.encoded_video.as_ref().unwrap_or(&export.output))
                    .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
                let mut child = command.spawn().map_err(|e| format!("FFmpeg spawn: {e}"))?;
                let stdin = child.stdin.take().ok_or("FFmpeg stdin missing")?;
                export.child = Some(child);
                export.stdin = Some(StudioPipeWriter::new(stdin, "studio-ffmpeg-writer")?);
            }
            if bgra.len() != width as usize * height as usize * 4 {
                return Err("Studio color capture size mismatch".into());
            }
            let color_bytes = bgra.len();
            let pipe_started = Instant::now();
            export.stdin.as_mut().ok_or("No FFmpeg stdin")?
                .send(bgra).map_err(|e| format!("FFmpeg write frame {index}: {e}"))?;
            encode_pipe_ms = pipe_started.elapsed().as_secs_f64()*1000.0;
            if export.depth {
                let pixels = depth_f32le.ok_or("Studio depth readback missing")?;
                if pixels.len() != color_bytes { return Err("Studio depth readback size mismatch".into()); }
                if export.depth_child.is_none() {
                    let ffmpeg = adjacent_ffmpeg_path().ok_or("ffmpeg.exe not found for depth")?;
                    let output = export.depth_dir.as_ref().ok_or("Missing depth output directory")?
                        .join("depth_%06d.exr");
                    let mut child = Command::new(ffmpeg)
                        .args(["-hide_banner", "-loglevel", "error", "-y",
                            "-f", "rawvideo", "-pix_fmt", "grayf32le"])
                        .arg("-s").arg(format!("{width}x{height}"))
                        .arg("-r").arg(export.fps.to_string())
                        .args(["-i", "pipe:0", "-c:v", "exr", "-compression", "zip16", "-start_number", "0"])
                        .arg(output)
                        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped())
                        .spawn().map_err(|e| format!("Depth encoder spawn: {e}"))?;
                    let stdin = child.stdin.take().ok_or("Depth FFmpeg stdin missing")?;
                    export.depth_child = Some(child);
                    export.depth_stdin = Some(StudioPipeWriter::new(stdin, "studio-depth-writer")?);
                }
                let depth_started = Instant::now();
                export.depth_stdin.as_mut().ok_or("Depth FFmpeg stdin missing")?
                    .send(pixels).map_err(|e| format!("Depth encoder frame {index}: {e}"))?;
                depth_pipe_ms = depth_started.elapsed().as_secs_f64()*1000.0;
            }
            export.frame_index += 1;
            Ok(export.frame_index == export.total_frames)
        })();
        if result.is_ok() {
            cost.encode_pipe_ms = encode_pipe_ms;
            cost.depth_pipe_ms = depth_pipe_ms;
            if let Some(export) = self.render_studio.export.as_mut() {
                export.profile.frame(index, (width, height), &timing, received, cost);
            }
        }
        match result {
            Ok(true) => self.finish_render_studio_export(),
            Ok(false) => { self.egui_repaint_requested = true; },
            Err(error) => self.fail_render_studio_export(error),
        }
    }

    fn end_render_studio_demo_capture(&mut self) {
        for source in &mut self.render_studio.sources {
            if source.audio_capture.take().is_some() {
                if let Some(sound)=source.playback.sound_presenter.as_mut() {
                    sound.end_offline_capture(&source.playback.client_game);
                }
            }
        }
    }

    fn finish_render_studio_export(&mut self) {
        let Some(mut export) = self.render_studio.export.take() else { return; };
        self.end_render_studio_demo_capture();
        let encoder = if export.preset.nvenc_codec().is_some() {
            format!(" (p{})", if export.nvenc_preset == 0 { STUDIO_NVENC_DEFAULT_PRESET } else { export.nvenc_preset })
        } else {
            export.encoder_speed.x264().map_or(String::new(), |speed| format!(" ({speed})"))
        };
        let quality = if export.preset.nvenc_codec().is_some()
            || matches!(export.preset, StudioVideoFormat::H264High | StudioVideoFormat::H264Max) {
            format!(" CRF {}", export.crf)
        } else { String::new() };
        let settings = format!("{}{encoder}{quality} · vsync {} · fps cap {}", export.preset.label(),
            self.video.vsync.config_value(), self.video.fps_cap);
        let profile = std::mem::replace(&mut export.profile, super::render_studio_profile::StudioExportProfile::new(1));
        let output = export.output.clone();
        // Draining the encoder and remuxing a multi-GB file takes seconds:
        // keep the editor live and finish on a worker.
        let worker = std::thread::Builder::new().name("studio-export-finish".into());
        let (export, worker) = {
            let shared = std::sync::Arc::new(std::sync::Mutex::new(Some(export)));
            let job = std::sync::Arc::clone(&shared);
            match worker.spawn(move || {
                let export = job.lock().unwrap_or_else(|e| e.into_inner()).take();
                export.map(finish_studio_export_blocking)
            }) {
                Ok(worker) => (None, Some(worker)),
                Err(_) => (shared.lock().unwrap_or_else(|e| e.into_inner()).take(), None),
            }
        };
        self.render_studio.export_finishing = Some(StudioExportFinishing {
            worker, inline: export, profile, settings, output,
        });
        self.render_studio.status = "Finishing video: encoder flush and audio mux…".into();
        self.asset_preview_viewport_key = None;
        self.render_command(RenderCommand::SetAssetPreviewViewport(None));
        self.set_studio_capture_size(None);
        self.egui_repaint_requested = true;
        self.poll_studio_export_finish();
    }

    /// Report a finished export once its worker is done (called every tick).
    pub(in crate::app) fn poll_studio_export_finish(&mut self) {
        let Some(finishing) = self.render_studio.export_finishing.as_mut() else { return; };
        let finished = match (finishing.worker.take(), finishing.inline.take()) {
            (Some(worker), _) if !worker.is_finished() => {
                finishing.worker = Some(worker);
                return;
            }
            (Some(worker), _) => worker.join().ok().flatten(),
            (None, Some(export)) => Some(finish_studio_export_blocking(export)),
            (None, None) => None,
        };
        let finishing = self.render_studio.export_finishing.take().expect("checked above");
        crate::runtime::clear_video_frame_pool();
        let finished = finished.unwrap_or(StudioExportFinished {
            error: Some("export finishing worker failed".into()), encoder_flush_ms: 0.0, mux_ms: 0.0, depth_dir: None,
        });
        let output = finishing.output;
        let (perf_lines, perf_status) = finishing.profile.finish(
            &output.with_file_name(format!("{}_perf.csv",
                output.file_stem().and_then(|s| s.to_str()).unwrap_or("studio"))),
            finished.encoder_flush_ms, finished.mux_ms, &finishing.settings);
        for line in perf_lines { self.push_console_line(line); }
        match finished.error {
            None => {
                self.render_studio.status = format!("Wrote {}{}{perf_status}", output.display(),
                    finished.depth_dir.as_ref().map_or(String::new(), |dir| format!(" + depth EXRs in {}", dir.display())));
                self.push_console_path_line(format!("^2{}", self.render_studio.status), output);
            }
            Some(detail) => {
                self.render_studio.status = format!("Render Studio FFmpeg failed: {detail}");
                self.push_console_line(format!("^1{}", self.render_studio.status));
            }
        }
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn cancel_render_studio_export(&mut self) {
        let Some(export) = self.render_studio.export.take() else { return; };
        self.end_render_studio_demo_capture();
        let output = export.output.clone();
        let depth_dir = export.depth_dir.clone();
        drop(export); // closes pipes and terminates the encoders
        crate::runtime::clear_video_frame_pool();
        let _ = std::fs::remove_file(output);
        if let Some(dir) = depth_dir { let _ = std::fs::remove_dir_all(dir); }
        self.render_studio.status = "Render Studio export cancelled".into();
        self.asset_preview_viewport_key = None;
        self.render_command(RenderCommand::SetAssetPreviewViewport(None));
        self.set_studio_capture_size(None);
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn fail_render_studio_export(&mut self, error: impl std::fmt::Display) {
        if self.render_studio.still.is_some() {
            self.render_studio.still = None;
            self.reset_render_studio_capture_viewport();
        } else {
            self.cancel_render_studio_export();
        }
        self.render_studio.status = format!("Render Studio export failed: {error}");
        self.push_console_line(format!("^1{}", self.render_studio.status));
    }
}

#[cfg(test)]
mod editor_history_tests {
    use super::*;
    fn group_clip(start:f64) -> StudioAnimationClip {
        StudioAnimationClip {name:"BOTH_STAND1".into(),emote:None,start,
            first_frame:0,end_frame:20,source_fps:20.0,lane:0,source_first:0,source_end:20,
            motion:vec![StudioMotionKey::default()],tail_frames:0,playback:StudioClipPlayback::Hold,speed_keys:vec![], blend:Default::default()}
    }
    #[test] fn mixed_group_paste_preserves_spacing_and_undoes_together() {
        let mut studio=RenderStudio::default();
        studio.timeline.clips=vec![group_clip(0.0),group_clip(2.0)];
        studio.timeline.camera.push(StudioCameraKey {time:1.5,fov:80.0,..Default::default()});
        studio.timeline.out_point=Some(10.0);
        studio.timeline.set_selection(vec![StudioItem::Clip(0),StudioItem::Clip(1),StudioItem::Camera(1),StudioItem::Motion(0,0)]);
        assert!(studio.copy_selection());
        studio.timeline.seek(5.0);studio.init_history();
        assert!(studio.paste_selection());
        assert_eq!(studio.timeline.clips[2].start,5.0);
        assert_eq!(studio.timeline.clips[3].start,7.0);
        assert_eq!(studio.timeline.clips[2].motion.len(),1);
        assert_eq!(studio.timeline.camera[2].time,6.5);
        assert_eq!(studio.timeline.camera[2].fov,80.0);
        assert_eq!(studio.timeline.selected_items().len(),3);
        studio.track_edit(false);
        assert!(studio.undo_edit());
        assert_eq!(studio.timeline.clips.len(),2);
        assert_eq!(studio.timeline.camera.len(),2);
        assert!(studio.redo_edit());
        assert_eq!(studio.timeline.clips.len(),4);
    }
    #[test] fn group_paste_resolves_collisions_without_collapsing_offsets() {
        let mut studio=RenderStudio::default();
        studio.timeline.snap=false;
        studio.timeline.clips=vec![group_clip(0.0),group_clip(2.0)];
        studio.timeline.set_selection(vec![StudioItem::Clip(0),StudioItem::Clip(1)]);
        assert!(studio.copy_selection());
        studio.timeline.seek(0.5);
        assert!(studio.paste_selection());
        assert_eq!(studio.timeline.clips[2].start,1.0);
        assert_eq!(studio.timeline.clips[3].start,3.0);
    }
    #[test] fn movement_group_paste_validates_all_destinations_before_editing() {
        let mut studio=RenderStudio::default();
        let mut clip=group_clip(0.0);
        clip.motion.push(StudioMotionKey {time:0.25,position:[12.0,0.0,0.0],..Default::default()});
        studio.timeline.clips=vec![clip];
        studio.timeline.snap=false;
        studio.timeline.out_point=Some(4.0);
        studio.timeline.set_selection(vec![StudioItem::Motion(0,0),StudioItem::Motion(0,1)]);
        assert!(studio.copy_selection());
        studio.timeline.seek(0.5);
        assert!(studio.paste_selection());
        assert_eq!(studio.timeline.clips[0].motion.len(),4);
        assert_eq!(studio.timeline.clips[0].motion[3].time,0.75);
        assert_eq!(studio.timeline.clips[0].motion[3].position,[12.0,0.0,0.0]);
        studio.timeline.seek(0.9);
        assert!(!studio.paste_selection());
        assert_eq!(studio.timeline.clips[0].motion.len(),4);
    }
    #[test]
    fn render_studio_models_resolve_each_layer_and_follow_edits() {
        let mut studio=RenderStudio::default();
        studio.model="kyle/default".into();
        studio.timeline.lane_models=vec!["alora2/blue".into(),"luke/default".into(),String::new()];
        assert_eq!(studio.actor_model(0),"alora2/blue");
        assert_eq!(studio.actor_model(1),"luke/default");
        assert_eq!(studio.actor_model(2),"kyle/default");
        studio.timeline.lane_models[0]="jan/default".into();
        assert_eq!(studio.actor_model(0),"jan/default");
        studio.timeline.lane_models[0].clear();
        assert_eq!(studio.actor_model(0),"kyle/default");
    }
    #[test]
    fn render_studio_char_colors_inherit_override_and_undo() {
        let mut studio=RenderStudio::default();
        let profile=[128,64,255];
        studio.timeline.lanes=2;
        studio.init_history();
        assert_eq!(studio.actor_char_color(0,profile),profile);
        assert_eq!(studio.actor_char_color(1,profile),profile);
        studio.timeline.lane_char_colors=vec![None,Some([12,200,45])];
        assert_eq!(studio.actor_char_color(0,profile),profile);
        assert_eq!(studio.actor_char_color(1,profile),[12,200,45]);
        assert_eq!(studio.actor_char_color(7,profile),profile);
        studio.track_edit(false);
        assert!(studio.undo_edit());
        assert_eq!(studio.actor_char_color(1,profile),profile);
        assert!(studio.redo_edit());
        assert_eq!(studio.actor_char_color(1,profile),[12,200,45]);
    }
    #[test]
    fn render_studio_sabers_resolve_per_actor_and_survive_undo() {
        use super::super::render_studio_timeline::StudioSabers;
        let mut studio=RenderStudio::default();
        studio.with_sabers=true;
        studio.timeline.lanes=2;
        studio.init_history();
        let baseline=studio.edit_snapshot().camera_pose_signature();
        let dual=StudioSabers {enabled:true,names:["single_1".into(),"single_2".into()],
            colors:[crate::cgame::SABER_CUSTOM_RGB|0x123456,5]};
        studio.timeline.lane_sabers=vec![None,Some(dual.clone())];
        let mut info=crate::cgame::ClientInfo::solo_model("kyle/default");
        info.saber_name="profile_saber".into();
        studio.apply_actor_sabers(0,&mut info);
        assert_eq!(info.saber_name,"profile_saber");
        studio.apply_actor_sabers(1,&mut info);
        assert_eq!([info.saber_name,info.saber2_name],dual.names);
        assert_eq!([info.saber_color,info.saber2_color],dual.colors);
        assert_ne!(studio.edit_snapshot().camera_pose_signature(),baseline);
        studio.track_edit(false);
        assert!(studio.undo_edit());
        assert!(studio.timeline.lane_sabers.iter().all(Option::is_none));
        assert!(studio.redo_edit());
        assert_eq!(studio.timeline.lane_sabers[1],Some(dual));
        studio.timeline.lane_sabers[1].as_mut().unwrap().enabled=false;
        assert!(studio.actor_with_sabers(0));
        assert!(!studio.actor_with_sabers(1));
        assert!(studio.actor_with_sabers(9)); // missing/legacy entry inherits
    }
    #[test]
    fn render_studio_sabers_project_roundtrip_and_legacy_defaults() {
        let legacy=serde_json::json!({"version":10,"model":"kyle/default","with_sabers":true,
            "background":"black","background_map":"","clips":[],"camera":[]});
        let mut project:StudioProject=serde_json::from_value(legacy).unwrap();
        assert!(project.lane_sabers.is_empty());
        assert!(project.lane_char_colors.is_empty());
        project.version=11;
        project.lanes=3;
        project.lane_sabers=vec![None,
            Some(super::super::render_studio_timeline::StudioSabers {enabled:true,
                names:["single_1".into(),"single_2".into()],colors:[0,crate::cgame::SABER_CUSTOM_RGB|0x123456]}),
            Some(super::super::render_studio_timeline::StudioSabers {enabled:false,
                names:["dual_1".into(),"none".into()],colors:[3,4]})];
        project.lane_char_colors=vec![None,Some([255,30,0]),Some([12,34,56])];
        let saved=serde_json::to_string(&project).unwrap();
        let loaded:StudioProject=serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded.lane_sabers,project.lane_sabers);
        assert_eq!(loaded.lane_char_colors,project.lane_char_colors);
    }
    fn clipboard_frame(studio: &mut RenderStudio, ctx: &egui::Context, events: Vec<egui::Event>) -> bool {
        let mut pasted=false;
        let _=ctx.run_ui(egui::RawInput {events,..Default::default()}, |ui| {
            pasted=studio.handle_clipboard_input(ui.ctx());
        });
        pasted
    }
    #[test]
    fn clipboard_events_work_without_ctrl_held_or_os_clipboard_text() {
        let mut studio=RenderStudio::default();
        let ctx=egui::Context::default();
        let _=ctx.run_ui(Default::default(),|ui| {
            ui.button("Timeline item").request_focus();
        });
        assert!(ctx.memory(|m|m.focused().is_some()));
        assert!(!ctx.text_edit_focused());
        studio.timeline.selected_camera=Some(0);
        studio.timeline.out_point=Some(4.0);
        studio.init_history();
        assert!(!clipboard_frame(&mut studio,&ctx,vec![egui::Event::Copy]));
        studio.timeline.camera[0].fov=90.0;
        studio.timeline.seek(2.0);
        assert!(clipboard_frame(&mut studio,&ctx,vec![egui::Event::Paste(String::new())]));
        assert_eq!(studio.timeline.camera.len(),2);
        assert_eq!(studio.timeline.camera[1].time,2.0);
        assert_eq!(studio.timeline.camera[1].fov,58.0);
        studio.track_edit(false);
        assert!(studio.undo_edit());
        assert_eq!(studio.timeline.camera.len(),1);
        assert!(studio.redo_edit());
        assert_eq!(studio.timeline.camera[1].time,2.0);
    }
    #[test]
    fn clipboard_events_copy_then_paste_clip_in_one_frame() {
        let mut studio=RenderStudio::default();
        let ctx=egui::Context::default();
        studio.timeline.add_clip(StudioAnimationClip {
            name:"BOTH_STAND1".into(),emote:None,start:0.0,
            first_frame:10,end_frame:30,source_fps:20.0,lane:0,
            source_first:0,source_end:40,motion:vec![StudioMotionKey::default()],
            tail_frames:0,playback:StudioClipPlayback::Loop,
        speed_keys:vec![], blend:Default::default()});
        studio.timeline.out_point=Some(4.0);
        studio.timeline.seek(2.0);
        assert!(clipboard_frame(&mut studio,&ctx,vec![egui::Event::Copy,egui::Event::Paste("unrelated OS text".into())]));
        assert_eq!(studio.timeline.clips.len(),2);
        assert_eq!(studio.timeline.clips[0].start,0.0);
        let pasted=&studio.timeline.clips[1];
        assert_eq!(pasted.start,2.0);
        assert_eq!(pasted.first_frame,10);
        assert_eq!(pasted.end_frame,30);
        assert_eq!(pasted.playback,StudioClipPlayback::Loop);

        studio.timeline.selected_motion=Some((1,0));
        studio.timeline.clips[1].motion[0].position=[12.0,3.0,4.0];
        assert!(!clipboard_frame(&mut studio,&ctx,vec![egui::Event::Copy]));
        studio.timeline.seek(2.5);
        assert!(clipboard_frame(&mut studio,&ctx,vec![egui::Event::Paste(String::new())]));
        let motion=&studio.timeline.clips[1].motion[1];
        assert_eq!(motion.time,1.0); // Half a second of source trim plus playhead offset.
        assert_eq!(motion.position,[12.0,3.0,4.0]);
    }
    #[test]
    fn clipboard_events_belong_to_focused_text_fields() {
        let mut studio=RenderStudio::default();
        let ctx=egui::Context::default();
        let id=egui::Id::new("studio_test_search");
        let mut search=String::new();
        let _=ctx.run_ui(Default::default(),|ui| {
            ui.add(egui::TextEdit::singleline(&mut search).id(id)).request_focus();
        });
        studio.timeline.selected_camera=Some(0);
        let _=ctx.run_ui(egui::RawInput {events:vec![egui::Event::Copy,egui::Event::Paste("search".into())],..Default::default()},|ui| {
            assert!(ui.ctx().text_edit_focused());
            assert!(!studio.handle_clipboard_input(ui.ctx()));
            assert!(studio.clipboard.is_none());
            ui.add(egui::TextEdit::singleline(&mut search).id(id));
        });
        assert_eq!(search,"search");
    }
    #[test]
    fn history_coalesces_pointer_drags_and_roundtrips_undo_redo() {
        let mut studio=RenderStudio::default();
        studio.init_history();
        studio.model="kyle/default".into();
        studio.track_edit(true);
        studio.model="jan/default".into();
        studio.track_edit(true);
        assert!(studio.history.undo.is_empty());
        studio.track_edit(false);
        assert_eq!(studio.history.undo.len(),1);
        assert!(studio.undo_edit());
        assert_eq!(studio.model,"");
        assert!(studio.redo_edit());
        assert_eq!(studio.model,"jan/default");
    }
    #[test]
    fn clipboard_clip_targets_other_lane_without_mutating_source_copy() {
        let mut studio=RenderStudio::default();
        studio.timeline.lanes=2;
        studio.timeline.lane_models.push("kyle/default".into());
        studio.timeline.add_clip(StudioAnimationClip {
            name:"BOTH_STAND1".into(),emote:None,start:0.0,
            first_frame:0,end_frame:20,source_fps:20.0,lane:0,
            source_first:0,source_end:20,motion:vec![StudioMotionKey::default()],
            tail_frames:0,playback:StudioClipPlayback::Hold,
        speed_keys:vec![], blend:Default::default()});
        assert!(studio.copy_selection());
        assert!(studio.paste_selection_on_lane(Some(1)));
        assert_eq!(studio.timeline.clips[1].lane,1);
        assert!(studio.paste_selection_on_lane(Some(0)));
        assert_eq!(studio.timeline.clips[2].lane,0);
        assert_eq!(studio.timeline.clips[0].lane,0);
    }
    #[test]
    fn copy_entire_animation_track_to_another_character_preserves_clip_automation() {
        let mut studio=RenderStudio::default();
        studio.timeline.lanes=3;
        studio.timeline.snap=false;
        let mut first=group_clip(1.0);
        first.motion.push(StudioMotionKey {time:0.5,position:[25.0,0.0,0.0],..Default::default()});
        first.playback=StudioClipPlayback::Loop;
        let second=group_clip(4.0);
        studio.timeline.clips=vec![first.clone(),second.clone()];
        studio.init_history();
        assert!(studio.copy_animation_lane(0));
        assert!(studio.clipboard_contains_animations());
        assert!(studio.paste_animations_on_lane(1,None));
        assert_eq!(studio.timeline.clips.len(),4);
        assert_eq!((studio.timeline.clips[2].lane,studio.timeline.clips[2].start),(1,1.0));
        assert_eq!((studio.timeline.clips[3].lane,studio.timeline.clips[3].start),(1,4.0));
        assert_eq!(studio.timeline.clips[2].motion.len(),first.motion.len());
        assert_eq!(studio.timeline.clips[2].motion[1].time,first.motion[1].time);
        assert_eq!(studio.timeline.clips[2].motion[1].position,first.motion[1].position);
        assert_eq!(studio.timeline.clips[2].playback,StudioClipPlayback::Loop);
        assert_eq!(studio.timeline.clips[0].lane,0);
        assert_eq!(studio.timeline.clips[1].start,4.0);
        studio.track_edit(false);
        assert!(studio.undo_edit());
        assert_eq!(studio.timeline.clips.len(),2);
        assert!(studio.redo_edit());
        assert_eq!(studio.timeline.clips.len(),4);
    }
    #[test]
    fn paste_animation_track_at_playhead_preserves_spacing_and_solves_collisions() {
        let mut studio=RenderStudio::default();
        studio.timeline.lanes=2;
        studio.timeline.snap=false;
        studio.timeline.clips=vec![group_clip(1.0),group_clip(4.0)];
        let mut blocker=group_clip(7.5);blocker.lane=1;
        studio.timeline.clips.push(blocker);
        assert!(studio.copy_animation_lane(0));
        assert!(studio.paste_animations_on_lane(1,Some(7.0)));
        // First desired clip overlaps the blocker, so the whole group shifts.
        assert_eq!(studio.timeline.clips[3].start,8.5);
        assert_eq!(studio.timeline.clips[4].start,11.5);
        assert_eq!(studio.timeline.clips[3].lane,1);
        assert_eq!(studio.timeline.clips[4].lane,1);
    }
    #[test]
    fn original_time_paste_does_not_quantize_unsnapped_source_timing() {
        let mut studio=RenderStudio::default();
        studio.timeline.lanes=2;
        studio.timeline.snap=true;
        studio.timeline.clips=vec![group_clip(1.12345)];
        assert!(studio.copy_animation_lane(0));
        assert!(studio.paste_animations_on_lane(1,None));
        assert_eq!(studio.timeline.clips[1].start,1.12345);
    }
    #[test]
    fn keyboard_paste_targets_last_clicked_character_track() {
        let mut studio=RenderStudio::default();
        studio.timeline.lanes=2;studio.timeline.snap=false;
        studio.timeline.clips=vec![group_clip(0.0)];
        studio.timeline.selected_clip=Some(0);
        assert!(studio.copy_selection());
        studio.context_lane=Some(1);
        studio.timeline.seek(2.0);
        assert!(studio.paste_selection());
        assert_eq!((studio.timeline.clips[1].lane,studio.timeline.clips[1].start),(1,2.0));
        assert_eq!(studio.timeline.clips[0].lane,0);
    }
    #[test]
    fn invalid_target_for_multiple_animation_tracks_fails_without_partial_paste() {
        let mut studio=RenderStudio::default();
        studio.timeline.lanes=3;studio.timeline.snap=false;
        let mut second=group_clip(2.0);second.lane=1;
        studio.timeline.clips=vec![group_clip(0.0),second];
        studio.timeline.set_selection(vec![StudioItem::Clip(0),StudioItem::Clip(1)]);
        assert!(studio.copy_selection());
        assert!(!studio.paste_selection_on_lane(Some(2)));
        assert_eq!(studio.timeline.clips.len(),2);
    }
    #[test]
    fn clipboard_camera_key_pastes_at_playhead() {
        let mut studio=RenderStudio::default();
        studio.timeline.selected_camera=Some(0);
        assert!(studio.copy_selection());
        studio.timeline.seek(0.0);
        studio.timeline.camera[0].fov=80.0;
        // Copy remembers the original camera value, not a reference.
        assert!(studio.paste_selection());
        assert_eq!(studio.timeline.camera[0].fov,58.0);
    }
}

/// Both color and radial depth use four bytes per pixel. Crop the source's
/// preview rectangle before encoding; editor backing pixels never enter video.
fn studio_crop_capture(width:u32,height:u32,bgra:Vec<u8>,depth:Option<Vec<u8>>,rect:Option<[u32;4]>)
    ->Result<(u32,u32,Vec<u8>,Option<Vec<u8>>),String> {
    let Some([x,y,w,h])=rect else{return Ok((width,height,bgra,depth));};
    if [x,y,w,h] == [0,0,width,height] && w > 0 && h > 0
        && bgra.len() == width as usize * height as usize * 4
        && depth.as_ref().is_none_or(|d| d.len() == bgra.len())
    { return Ok((width,height,bgra,depth)); }
    if w==0 || h==0 || x.saturating_add(w)>width || y.saturating_add(h)>height {
        return Err("Studio viewport changed during capture; retry the capture".into());
    }
    let crop=|pixels:Vec<u8>|->Result<Vec<u8>,String> {
        if pixels.len()!=width as usize*height as usize*4{return Err("Invalid Studio capture length".into());}
        let mut output=Vec::with_capacity(w as usize*h as usize*4);
        for row in y..y+h {
            let first=(row as usize*width as usize+x as usize)*4;
            output.extend_from_slice(&pixels[first..first+w as usize*4]);
        }
        Ok(output)
    };
    Ok((w,h,crop(bgra)?,depth.map(crop).transpose()?))
}

#[cfg(test)]
mod studio_capture_tests {
    use super::*;
    #[test]
    fn scene_capture_crops_color_and_depth_to_the_same_viewport() {
        let image:Vec<u8>=(0u8..12).flat_map(|p|[p,p,p,255]).collect();
        let (w,h,color,depth)=studio_crop_capture(4,3,image.clone(),Some(image),Some([1,1,2,2])).unwrap();
        assert_eq!((w,h),(2,2));
        assert_eq!(color.chunks_exact(4).map(|p|p[0]).collect::<Vec<_>>(),[5,6,9,10]);
        assert_eq!(depth.unwrap(),color);
        assert!(studio_crop_capture(4,3,vec![0;48],None,Some([3,1,2,2])).is_err());
    }
    #[test]
    fn preview_load_wait_does_not_survive_restart_and_play() {
        let mut studio=RenderStudio::default();
        studio.timeline.demos.push(StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"demo".into(),game:String::new(),start:0.0,
            duration_ms:10000,in_ms:0,out_ms:10000,speed_keys:vec![],transition_in:Default::default(),audio:Default::default()});
        studio.timeline.seek(6.5);studio.timeline.toggle_play();
        studio.source_wait=Some(((studio.timeline.playhead,studio.timeline.play_started,true),6.5));
        assert_eq!(studio.timeline_time(),6.5);
        studio.timeline.seek(0.0);studio.timeline.toggle_play();
        assert!(studio.timeline_time()<0.1);
    }
}

#[cfg(test)]
mod studio_audio_mux_tests {
    use super::*;
    #[test]
    #[ignore="requires DJK_STUDIO_TEST_FFMPEG pointing to installed ffmpeg.exe"]
    fn studio_audio_muxes_aac_opus_and_pcm() {
        use crate::app::render_studio_audio::{StudioAudioRuntime,StudioAudioClip,StudioAudioTrack,StudioAudioFade,RATE};
        let ffmpeg=std::env::var_os("DJK_STUDIO_TEST_FFMPEG").expect("DJK_STUDIO_TEST_FFMPEG");
        let fixture=crate::cvar_compatibility::Fixture::new();
        let wave=fixture.0.join("audio.wav");
        // Exercise the actual WAV mixer, not speaker capture or an ffmpeg tone.
        let mut audio=StudioAudioRuntime::default();
        let source=fixture.0.join("source.wav");
        let mut pcm=Vec::new();
        for i in 0..4800 {let v=(i as f32*0.1).sin()*0.25;pcm.extend_from_slice(&v.to_le_bytes());}
        // Make a mono float WAV to exercise import/channel conversion as well.
        let mut wav=Vec::new();wav.extend_from_slice(b"RIFF");wav.extend_from_slice(&(36u32+pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");wav.extend_from_slice(&16u32.to_le_bytes());wav.extend_from_slice(&3u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());wav.extend_from_slice(&RATE.to_le_bytes());wav.extend_from_slice(&(RATE*4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());wav.extend_from_slice(&32u16.to_le_bytes());wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());wav.extend_from_slice(&pcm);std::fs::write(&source,wav).unwrap();
        let path=source.to_str().unwrap().to_owned();audio.request(&path);
        let tracks=[StudioAudioTrack {clips:vec![StudioAudioClip {path,start:0.0,source_in:0.0,source_out:0.1,
            gain:1.0,keys:vec![],fade_in:0.0,fade_out:0.0,fade_curve:StudioAudioFade::Linear,envelope:None}],..StudioAudioTrack::new(1)}];
        let deadline=Instant::now()+std::time::Duration::from_secs(10);
        while audio.all_ready(&tracks).is_err() {assert!(Instant::now()<deadline);audio.poll();std::thread::sleep(std::time::Duration::from_millis(2));}
        audio.write_wav(&tracks,0.0,4800,&wave).unwrap();
        for preset in [StudioVideoFormat::H264High,StudioVideoFormat::WebmVp9,StudioVideoFormat::ProRes422] {
            let output=fixture.0.join(format!("test.{}",preset.extension()));
            let mut command=Command::new(&ffmpeg);
            #[cfg(windows)] {use std::os::windows::process::CommandExt;command.creation_flags(0x08000000);}
            command.args(["-hide_banner","-loglevel","error","-y","-f","rawvideo","-pix_fmt","bgra","-s","16x16","-r","10","-i","pipe:0"])
                .arg("-i").arg(&wave);
            if preset==StudioVideoFormat::WebmVp9 {command.args(["-c:v","libvpx-vp9","-pix_fmt","yuv420p"]);}
            else {command.args(preset.demo_preset().unwrap().ffmpeg_video_args());}
            command.args(preset.audio_ffmpeg_args()).args(["-map","0:v:0","-map","1:a:0"]).arg(&output)
                .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
            let mut child=command.spawn().unwrap();child.stdin.take().unwrap().write_all(&vec![80;16*16*4]).unwrap();
            let encoded=child.wait_with_output().unwrap();assert!(encoded.status.success(),"{preset:?}: {}",String::from_utf8_lossy(&encoded.stderr));
            let mut decode=Command::new(&ffmpeg);
            #[cfg(windows)] {use std::os::windows::process::CommandExt;decode.creation_flags(0x08000000);}
            let decoded=decode.args(["-hide_banner","-loglevel","error","-i"]).arg(&output)
                .args(["-map","0:a:0","-f","f32le","-ac","2","-ar","48000","pipe:1"]).output().unwrap();
            assert!(decoded.status.success(),"{preset:?} audio decode failed");
            assert!(decoded.stdout.len()>=4800*2*4,"{preset:?} truncated audio");
            assert!(decoded.stdout.chunks_exact(4).any(|s|f32::from_le_bytes(s.try_into().unwrap()).abs()>0.05));
            println!("[STUDIO AUDIO] {preset:?} encoded and decoded imported stereo audio");
        }
    }
}

/// Origin Z where a standing player hull rests on the floor below `origin`
/// (native JKA coordinates). Starts a little higher so a sunk actor pops up.
fn studio_floor_origin_z(world: &mut jka_movement::CollisionWorld, origin: [f32; 3]) -> Option<f32> {
    use jka_movement::{TraceQuery, TraceWorld};
    const MASK: i32 = 0x1 | 0x10 | 0x1000;
    for lift in [48.0f32, 0.0, 128.0, 256.0, 512.0] {
        let start = [origin[0], origin[1], origin[2] + lift];
        let trace = world.trace(TraceQuery { start, end: [start[0], start[1], start[2] - 8192.0],
            mins: [-15.0, -15.0, -24.0], maxs: [15.0, 15.0, 40.0], pass_entity: -1, mask: MASK });
        if trace.start_solid != 0 || trace.all_solid != 0 { continue; }
        return (trace.fraction < 1.0 && trace.end[2].is_finite()).then_some(trace.end[2]);
    }
    None
}
