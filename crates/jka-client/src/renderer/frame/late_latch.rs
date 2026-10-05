//! Frame late latch.
use crate::renderer::{
    late_latch_third_person_view, scene, AtomicBool, AtomicU32, AtomicU64, Camera, Duration,
    Instant, Ordering, ThirdPersonLateLatchView, Vec3,
};

#[derive(Clone, Copy, Debug)]
pub struct InputLatencySample {
    pub sequence: u64,
    pub event_at: Instant,
    pub simulation_at: Instant,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ViewLatchMode {
    #[default]
    Disabled,
    /// First-person view: the newest input orientation is the final camera rotation.
    Direct,
    /// Ordinary local third person. Target damping/collision were resolved by
    /// CGame; the renderer may rebuild only the angle-dependent OpenJK orbit math.
    ThirdPerson(ThirdPersonLateLatchView),
}

/// Tiny single-writer/multi-reader mailbox for subframe *input* view rotation.
/// The main thread publishes the latest local-player yaw/pitch, mouse timestamp,
/// and dynamic-crosshair world endpoint as one seqlock tuple without taking the
/// RenderSnapshot mutex. Pairing the endpoint with its exact aim sample prevents
/// a newer late-latched camera from projecting a one-input-event-old crosshair.
/// The renderer maps the input onto the final camera via `ViewLatchMode`.
pub(in crate::renderer) struct LatestViewState {
    pub(in crate::renderer) epoch: Instant,
    pub(in crate::renderer) version: AtomicU64,
    pub(in crate::renderer) yaw_bits: AtomicU32,
    pub(in crate::renderer) pitch_bits: AtomicU32,
    pub(in crate::renderer) crosshair_valid: AtomicBool,
    pub(in crate::renderer) crosshair_x_bits: AtomicU32,
    pub(in crate::renderer) crosshair_y_bits: AtomicU32,
    pub(in crate::renderer) crosshair_z_bits: AtomicU32,
    pub(in crate::renderer) input_sequence: AtomicU64,
    pub(in crate::renderer) event_ns: AtomicU64,
    pub(in crate::renderer) simulation_ns: AtomicU64,
}

#[derive(Clone, Copy)]
pub(in crate::renderer) struct LatestViewSample {
    pub(in crate::renderer) yaw: f32,
    pub(in crate::renderer) pitch: f32,
    pub(in crate::renderer) crosshair_world: Option<[f32; 3]>,
    pub(in crate::renderer) input: Option<InputLatencySample>,
}

#[derive(Clone, Copy)]
pub(in crate::renderer) struct ViewSampleMeasurement {
    pub(in crate::renderer) input: InputLatencySample,
    pub(in crate::renderer) sampled_at: Instant,
    /// Dynamic-crosshair endpoint paired with exactly this input sample.
    pub(in crate::renderer) crosshair_world: Option<[f32; 3]>,
}

impl LatestViewState {
    pub(in crate::renderer) fn new(camera: Camera) -> Self {
        Self {
            epoch: Instant::now(),
            version: AtomicU64::new(0),
            yaw_bits: AtomicU32::new(camera.yaw.to_bits()),
            pitch_bits: AtomicU32::new(camera.pitch.to_bits()),
            crosshair_valid: AtomicBool::new(false),
            crosshair_x_bits: AtomicU32::new(0.0_f32.to_bits()),
            crosshair_y_bits: AtomicU32::new(0.0_f32.to_bits()),
            crosshair_z_bits: AtomicU32::new(0.0_f32.to_bits()),
            input_sequence: AtomicU64::new(0),
            event_ns: AtomicU64::new(0),
            simulation_ns: AtomicU64::new(0),
        }
    }

    #[inline]
    pub(in crate::renderer) fn instant_ns(&self, instant: Instant) -> Option<u64> {
        instant
            .checked_duration_since(self.epoch)
            .map(|elapsed| elapsed.as_nanos().min(u128::from(u64::MAX)) as u64)
    }

    pub(in crate::renderer) fn publish_rotation(
        &self,
        yaw: f32,
        pitch: f32,
        crosshair_world: Option<[f32; 3]>,
        input: Option<InputLatencySample>,
    ) {
        // Single writer (winit/main thread): odd marks an in-progress update;
        // even + Release publishes the complete tuple to the render thread.
        // AcqRel on the opening RMW keeps the payload stores after the odd
        // marker; Release on the closing RMW publishes the complete tuple.
        // That gives the Acquire/Acquire reader a real seqlock boundary rather
        // than relying on relaxed operations not being reordered.
        self.version.fetch_add(1, Ordering::AcqRel);
        self.yaw_bits.store(yaw.to_bits(), Ordering::Relaxed);
        self.pitch_bits.store(pitch.to_bits(), Ordering::Relaxed);
        if let Some([x, y, z]) = crosshair_world {
            self.crosshair_x_bits.store(x.to_bits(), Ordering::Relaxed);
            self.crosshair_y_bits.store(y.to_bits(), Ordering::Relaxed);
            self.crosshair_z_bits.store(z.to_bits(), Ordering::Relaxed);
            self.crosshair_valid.store(true, Ordering::Relaxed);
        } else {
            self.crosshair_valid.store(false, Ordering::Relaxed);
        }
        if let Some(sample) = input {
            if let (Some(event_ns), Some(simulation_ns)) = (
                self.instant_ns(sample.event_at),
                self.instant_ns(sample.simulation_at),
            ) {
                self.input_sequence
                    .store(sample.sequence, Ordering::Relaxed);
                self.event_ns.store(event_ns, Ordering::Relaxed);
                self.simulation_ns.store(simulation_ns, Ordering::Relaxed);
            }
        }
        self.version.fetch_add(1, Ordering::Release);
    }

    pub(in crate::renderer) fn load(&self) -> LatestViewSample {
        loop {
            let before = self.version.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let yaw = f32::from_bits(self.yaw_bits.load(Ordering::Relaxed));
            let pitch = f32::from_bits(self.pitch_bits.load(Ordering::Relaxed));
            let crosshair_valid = self.crosshair_valid.load(Ordering::Relaxed);
            let crosshair_world = crosshair_valid.then(|| {
                [
                    f32::from_bits(self.crosshair_x_bits.load(Ordering::Relaxed)),
                    f32::from_bits(self.crosshair_y_bits.load(Ordering::Relaxed)),
                    f32::from_bits(self.crosshair_z_bits.load(Ordering::Relaxed)),
                ]
            });
            let sequence = self.input_sequence.load(Ordering::Relaxed);
            let event_ns = self.event_ns.load(Ordering::Relaxed);
            let simulation_ns = self.simulation_ns.load(Ordering::Relaxed);
            let after = self.version.load(Ordering::Acquire);
            if before != after {
                std::hint::spin_loop();
                continue;
            }
            let input = (sequence != 0).then(|| InputLatencySample {
                sequence,
                event_at: self.epoch + Duration::from_nanos(event_ns),
                simulation_at: self.epoch + Duration::from_nanos(simulation_ns),
            });
            return LatestViewSample {
                yaw,
                pitch,
                crosshair_world,
                input,
            };
        }
    }
}

#[inline]
pub(in crate::renderer) fn sample_view_rotation(
    camera: &mut Camera,
    state: &LatestViewState,
    mode: ViewLatchMode,
) -> Option<ViewSampleMeasurement> {
    if mode == ViewLatchMode::Disabled {
        return None;
    }
    let sample = state.load();
    match mode {
        ViewLatchMode::Disabled => unreachable!(),
        ViewLatchMode::Direct => {
            camera.yaw = sample.yaw;
            camera.pitch = sample.pitch;
            // The base position (already shaken by the frame setup) is untouched.
            camera.apply_shake_angles();
        }
        ViewLatchMode::ThirdPerson(latch) => {
            // LatestViewState uses renderer conventions (yaw radians, positive
            // pitch up). The OpenJK camera port consumes native JKA degrees,
            // where positive pitch points down. This is the newest real input,
            // never an extrapolated/predicted angle.
            let view = late_latch_third_person_view(
                latch,
                [-sample.pitch.to_degrees(), sample.yaw.to_degrees(), 0.0],
            );
            camera.position = Vec3::from_array(scene::render_position(view.origin));
            camera.yaw = view.angles[1].to_radians();
            camera.pitch = -view.angles[0].to_radians();
            camera.apply_shake();
        }
    }
    let sampled_at = Instant::now();
    sample.input.map(|input| ViewSampleMeasurement {
        input,
        sampled_at,
        crosshair_world: sample.crosshair_world,
    })
}
