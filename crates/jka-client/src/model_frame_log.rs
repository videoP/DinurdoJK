//! Anchor positions from draw poses, projected with the final render camera.
//! One group of rows per successful present(), including repeated snapshots.
use crate::cgame::player_presenter::ViewerAnimDebug;
use glam::{Mat4, Vec3};
use std::{
    fs::{self, OpenOptions},
    io::{self, BufWriter, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static NEXT_SOURCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug)]
pub struct FrameSource {
    pub id: u64,
    pub published_at: Instant,
    pub entity: u16,
    pub anim: Option<ViewerAnimDebug>,
    pub raster_surfaces: usize,
}

impl FrameSource {
    pub fn new(entity: u16, anim: Option<ViewerAnimDebug>) -> Self {
        Self {
            id: NEXT_SOURCE.fetch_add(1, Ordering::Relaxed),
            published_at: Instant::now(),
            entity,
            anim,
            raster_surfaces: 0,
        }
    }
}

struct Record {
    frame: u64,
    at: Instant,
    source: FrameSource,
    view_proj: Mat4,
    unjittered: Mat4,
    camera: [f32; 3],
    size: [u32; 2],
}

struct Writer {
    tx: Option<Sender<Record>>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Closing the sender lets the worker drain every queued frame and flush.
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct ModelFrameLog {
    directory: PathBuf,
    source: Option<FrameSource>,
    writer: Option<Writer>,
    failed: bool,
    present_id: u64,
}

impl ModelFrameLog {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            source: None,
            writer: None,
            failed: false,
            present_id: 0,
        }
    }

    pub fn set_source(&mut self, source: Option<FrameSource>) {
        if source.is_none() {
            self.writer.take();
            self.failed = false;
        } else if self.writer.is_none() && !self.failed {
            let directory = self.directory.clone();
            let (tx, rx) = mpsc::channel::<Record>();
            match thread::Builder::new().name("jka-model-frame-log".into()).spawn(move || {
                let result = (|| -> io::Result<()> {
                    fs::create_dir_all(&directory)?;
                    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
                    let path = directory.join(format!("model-frames-{stamp}.csv"));
                    let file = OpenOptions::new().write(true).create_new(true).open(&path)?;
                    fs::write(path.with_extension("json"), r#"{
  "sampling": "Each successful render-thread present call; not app ticks or monitor scanout.",
  "world_coordinates": "JKA world axes: x/y horizontal, z up.",
  "screen_coordinates": "Output pixels, top-left origin, using the exact final latched camera matrix. Both jittered and unjittered projection are included.",
  "anchors": "head=*head_eyes bolt from the drawn pose; hilt=hand attachment origin in the drawn hilt transform; blade=authored blade bolt; blade_core=visual core start (one unit behind blade bolt). Thrown sabers use their own entity transform.",
  "availability": "Missing anchors have available=0 and blank coordinates. submitted describes geometry submission, not depth-test visibility. in_frustum does not establish occlusion visibility.",
  "limits": "Pose/attachment positions supplied to drawing, not pixel readback after TAA, motion blur, lens effects or GPU skinning. No inference of animation freezes from repeated integer animation frames alone."
}
"#)?;
                    crate::logging::write_line_with_path(crate::logging::Level::Info,
                        format_args!("MODEL-FRAMES: recording every presented frame to {}", path.display()), &path);
                    let mut out = BufWriter::with_capacity(256 * 1024, file);
                    writeln!(out, "frame_id,present_ms,dt_ms,source_id,source_repeated,source_age_ms,entity,pose_time,call_time,legs_anim,legs_frame,legs_old_frame,legs_blend,torso_anim,torso_frame,torso_old_frame,torso_blend,raster_surfaces,anchor,saber,blade,available,submitted,world_x,world_y,world_z,screen_x,screen_y,ndc_z,clip_w,in_frustum,stable_screen_x,stable_screen_y,camera_x,camera_y,camera_z,width,height")?;
                    let start = Instant::now();
                    let mut previous_at = None;
                    let mut previous_source = None;
                    let mut last_flush = Instant::now();
                    loop {
                        match rx.recv_timeout(Duration::from_millis(250)) {
                            Ok(record) => {
                                write_record(&mut out, &record, start, previous_at, previous_source)?;
                                previous_at = Some(record.at);
                                previous_source = Some(record.source.id);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        if last_flush.elapsed() >= Duration::from_millis(250) {
                            out.flush()?;
                            last_flush = Instant::now();
                        }
                    }
                    out.flush()?;
                    crate::logging::write_line_with_path(crate::logging::Level::Info,
                        format_args!("MODEL-FRAMES: saved {}", path.display()), &path);
                    Ok(())
                })();
                if let Err(error) = result { eprintln!("MODEL-FRAMES: writer failed: {error}"); }
            }) {
                Ok(worker) => self.writer = Some(Writer { tx: Some(tx), worker: Some(worker) }),
                Err(error) => { self.failed = true; eprintln!("MODEL-FRAMES: cannot start writer: {error}"); }
            }
        }
        self.source = source;
    }

    pub fn presented(
        &mut self,
        at: Instant,
        view_proj: Mat4,
        unjittered: Mat4,
        camera: Vec3,
        size: [u32; 2],
    ) {
        self.present_id += 1;
        let (Some(source), Some(writer)) = (self.source, self.writer.as_ref()) else {
            return;
        };
        let record = Record {
            frame: self.present_id,
            at,
            source,
            view_proj,
            unjittered,
            camera: crate::scene::jka_position(camera.to_array()),
            size,
        };
        if writer
            .tx
            .as_ref()
            .is_some_and(|tx| tx.send(record).is_err())
        {
            self.failed = true;
            self.writer.take();
            eprintln!("MODEL-FRAMES: logging stopped after writer failure; toggle cg_modelFrameDebug to retry");
        }
    }

    pub fn presented_without_scene(&mut self, at: Instant, camera: Vec3, size: [u32; 2]) {
        let original = self.source;
        if let Some(source) = self.source.as_mut() {
            source.anim = None;
            source.raster_surfaces = 0;
        }
        self.presented(at, Mat4::IDENTITY, Mat4::IDENTITY, camera, size);
        self.source = original;
    }
}

fn project(world: [f32; 3], matrix: Mat4, size: [u32; 2]) -> ([f32; 4], bool) {
    let render = Vec3::from_array(crate::scene::render_position(world));
    let clip = matrix * render.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let pixel = [
        (ndc.x + 1.0) * 0.5 * size[0] as f32,
        (1.0 - ndc.y) * 0.5 * size[1] as f32,
        ndc.z,
        clip.w,
    ];
    let inside = clip.is_finite()
        && clip.w > 0.0
        && ndc.x.abs() <= 1.0
        && ndc.y.abs() <= 1.0
        && (0.0..=1.0).contains(&ndc.z);
    (pixel, inside)
}

fn write_record(
    out: &mut impl Write,
    record: &Record,
    start: Instant,
    previous_at: Option<Instant>,
    previous_source: Option<u64>,
) -> io::Result<()> {
    let anim = record.source.anim.unwrap_or_default();
    let elapsed = record.at.saturating_duration_since(start).as_secs_f64() * 1000.0;
    let dt = previous_at.map_or(0.0, |at| {
        record.at.saturating_duration_since(at).as_secs_f64() * 1000.0
    });
    let age = record
        .at
        .saturating_duration_since(record.source.published_at)
        .as_secs_f64()
        * 1000.0;
    let mut anchor = |name: &str,
                      saber: usize,
                      blade: usize,
                      point: Option<[f32; 3]>,
                      submitted: bool|
     -> io::Result<()> {
        write!(
            out,
            "{},{:.3},{:.3},{},{},{:.3},{},{},{},{},{},{},{:.6},{},{},{},{:.6},{},{},{},{},{},{},",
            record.frame,
            elapsed,
            dt,
            record.source.id,
            u8::from(previous_source == Some(record.source.id)),
            age,
            record.source.entity,
            anim.pose_time,
            anim.call_time,
            anim.legs_anim,
            anim.legs_frame,
            anim.legs_old_frame,
            anim.legs_backlerp,
            anim.torso_anim,
            anim.torso_frame,
            anim.torso_old_frame,
            anim.torso_backlerp,
            record.source.raster_surfaces,
            name,
            saber,
            blade,
            u8::from(point.is_some()),
            u8::from(submitted)
        )?;
        if let Some(world) = point {
            let (pixel, inside) = project(world, record.view_proj, record.size);
            let (stable, _) = project(world, record.unjittered, record.size);
            write!(out, "{:.6},{:.6},{:.6},", world[0], world[1], world[2])?;
            if pixel.iter().all(|v| v.is_finite()) {
                write!(
                    out,
                    "{:.6},{:.6},{:.6},{:.6},{},{:.6},{:.6},",
                    pixel[0],
                    pixel[1],
                    pixel[2],
                    pixel[3],
                    u8::from(inside),
                    stable[0],
                    stable[1]
                )?;
            } else {
                write!(out, ",,,{:.6},0,,,", pixel[3])?;
            }
        } else {
            write!(out, ",,,,,,,,,,")?;
        }
        writeln!(
            out,
            "{:.6},{:.6},{:.6},{},{}",
            record.camera[0], record.camera[1], record.camera[2], record.size[0], record.size[1]
        )
    };
    anchor(
        "head",
        0,
        0,
        anim.head_world,
        anim.submit_geometry && record.source.raster_surfaces > 0,
    )?;
    for saber in 0..2 {
        if saber == 0 || anim.hilt_world[saber].is_some() {
            anchor(
                "hilt",
                saber,
                0,
                anim.hilt_world[saber],
                anim.hilt_submitted[saber],
            )?;
        }
        for blade in 0..8 {
            if (saber == 0 && blade == 0) || anim.blade_world[saber][blade].is_some() {
                anchor(
                    "blade",
                    saber,
                    blade,
                    anim.blade_world[saber][blade],
                    anim.blade_submitted[saber][blade],
                )?;
                anchor(
                    "blade_core",
                    saber,
                    blade,
                    anim.blade_core_world[saber][blade],
                    anim.blade_submitted[saber][blade],
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_jka_world_axes_to_output_pixels_and_detects_behind_camera() {
        let matrix = Mat4::perspective_rh(90.0_f32.to_radians(), 2.0, 0.1, 100.0);
        let (center, inside) = project([0.0, 5.0, 0.0], matrix, [200, 100]);
        assert!(inside);
        assert!((center[0] - 100.0).abs() < 0.001);
        assert!((center[1] - 50.0).abs() < 0.001);
        let (upper_right, inside) = project([1.0, 5.0, 1.0], matrix, [200, 100]);
        assert!(inside);
        assert!(upper_right[0] > center[0] && upper_right[1] < center[1]);
        let (behind, inside) = project([0.0, -5.0, 0.0], matrix, [200, 100]);
        assert!(!inside);
        assert!(behind[3] < 0.0);
    }

    #[test]
    fn repeated_sources_and_missing_anchors_still_produce_every_frame() {
        let start = Instant::now();
        let source = FrameSource {
            id: 7,
            published_at: start,
            entity: 0,
            anim: None,
            raster_surfaces: 0,
        };
        let mut record = Record {
            frame: 10,
            at: start,
            source,
            view_proj: Mat4::IDENTITY,
            unjittered: Mat4::IDENTITY,
            camera: [0.0; 3],
            size: [200, 100],
        };
        let mut csv = Vec::new();
        write_record(&mut csv, &record, start, None, None).unwrap();
        record.frame = 11;
        record.at = start + Duration::from_millis(8);
        write_record(&mut csv, &record, start, Some(start), Some(7)).unwrap();
        let csv = String::from_utf8(csv).unwrap();
        let rows: Vec<Vec<&str>> = csv.lines().map(|line| line.split(',').collect()).collect();
        assert_eq!(
            rows.len(),
            8,
            "head/hilt/blade/core rows even without anchors"
        );
        for row in &rows {
            assert_eq!(row.len(), 38);
            assert_eq!(row[21], "0");
            assert!(row[23..33].iter().all(|value| value.is_empty()));
        }
        assert_eq!(rows[0][0], "10");
        assert_eq!(rows[4][0], "11");
        assert_eq!(rows[4][4], "1", "repeated source is recorded, not filtered");
        assert_eq!(rows[4][2], "8.000");
    }
}
