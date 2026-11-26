//! Console-spawned Fluid Ninja fields and scene-linear surface blending.
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use jka_ninja::{FLAME_SIM_IMAGES, FLAME_MOTION_MAX_STROKES, FlameMotionStroke, FlamePreset};
use wgpu::util::DeviceExt as _;

use crate::ninja_fire_debug::{
    CM_PER_MAP_UNIT, Emitter, EmitterSource, FireScene, PlayerEffect, PlayerPose,
    PROMOTED_SMOKE_PRESET,
};
use crate::renderer::{Camera, CloudForegroundBlade, Renderer, textures::GpuImage, MAX_CLOUD_FOREGROUND_BLADES};

mod brush_collision;
mod demo_bloom;
mod embers;
mod heat_haze;
mod local_post;
mod volume;
use brush_collision::{BrushCapture, CaptureEncode, CapturePipeline};
use embers::{EmberCompute, EmberFrame, ParticlePool, PoolKind};

type FlameTarget = (wgpu::TextureFormat, u32, Option<wgpu::TextureFormat>);

/// Smooth 1D value noise in 0..1: hashed lattice values, smoothstep-blended,
/// so the result is continuous in time and frame-rate independent.
fn value_noise(t: f32, seed: u32) -> f32 {
    fn hash(i: i32, seed: u32) -> f32 {
        let mut x = (i as u32).wrapping_mul(0x9E37_79B1) ^ seed;
        x ^= x >> 16;
        x = x.wrapping_mul(0x7FEB_352D);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846C_A68B);
        x ^= x >> 16;
        x as f32 / u32::MAX as f32
    }
    let i = t.floor();
    let f = t - i;
    let u = f * f * (3.0 - 2.0 * f);
    let i = i as i32;
    hash(i, seed) + (hash(i.wrapping_add(1), seed) - hash(i, seed)) * u
}

fn fire_flicker(time: f32, seed: u32) -> f32 {
    let swell = value_noise(time * 2.1, seed);
    let crackle = value_noise(time * 9.3, seed ^ 0xA511_E9B3);
    0.6 + 0.25 * swell + 0.15 * crackle
}

const PLAYER_FIRE_DRAIN_SECONDS: f32 = crate::ninja_fire_debug::PLAYER_FIRE_DRAIN_MS as f32 * 0.001;
/// What one preset's *unscaled* flame looks like, so an authored EFX footprint
/// can be matched to it. Only presets with a measurement are listed; the rest
/// keep their native plane.
struct NativeFlame {
    /// Authored-sprite footprint (width, height, cm) that the unscaled preset
    /// visually corresponds to. Calibrated against stock sprites: dividing an
    /// EFX's footprint (`fx::fire_geometry`, computed from its particle data)
    /// by this gives the horizontal and vertical magnification of the preset.
    footprint_cm: [f32; 2],
    /// Half-size, in cm of the preset's own field, of the region around the
    /// emitter that is displayed. The flame itself is only a small part of the
    /// 300 cm domain, so only this window is drawn and magnified.
    window_half_cm: [f32; 2],
}

/// Torch: measured at mp/ffa2 against the stock `env/small_fire` flame
/// (authored footprint 57.7 x 63.0 cm; the unscaled Ninja torch covered 1/3.96
/// of its width and 1/1.61 of its height on screen). The window holds the
/// flame, its wispy tail and a soft fade-out band, so nothing is cut off.
const TORCH_NATIVE: NativeFlame = NativeFlame {
    footprint_cm: [14.6, 39.2],
    window_half_cm: [24.0, 64.0],
};

fn native_flame(preset: FlamePreset) -> Option<&'static NativeFlame> {
    match preset {
        FlamePreset::Torch => Some(&TORCH_NATIVE),
        _ => None,
    }
}

const MIN_FOOTPRINT_SCALE: f32 = 0.25;
const MAX_FOOTPRINT_SCALE: f32 = 8.0;

/// World size of the drawn quad and the part of the field it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FlamePlane {
    width_cm: f32,
    height_cm: f32,
    /// Fraction of the field (u, v) shown across the quad; 1 = whole field.
    crop: [f32; 2],
}

/// Size the flame plane from the *authored EFX footprint*. One uniform
/// magnification (matching the footprint's area) is used for both axes: the
/// fluid keeps its own proportions instead of being smeared to the sprite
/// cloud's. Only a window around the emitter is drawn, with a soft edge in the
/// shader, so the quad stays smaller than the native plane despite the
/// magnification. The private field, its centre offset and its simulation are
/// untouched.
fn authored_fire_plane(
    authored: Option<crate::ninja_fire_debug::FlameFit>,
    preset: FlamePreset, native_width_cm: f32, native_height_cm: f32,
) -> FlamePlane {
    let native = FlamePlane {
        width_cm: native_width_cm, height_cm: native_height_cm, crop: [1.0, 1.0],
    };
    let (Some(bounds), Some(flame)) = (authored, native_flame(preset)) else { return native; };
    let size = (bounds.max - bounds.min) * CM_PER_MAP_UNIT;
    // Upright emitter: height is the vertical extent, width the wider of the
    // two horizontal extents (the plane faces the camera).
    let (width, height) = (size.x.max(size.z), size.y);
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return native;
    }
    let area_ratio = (width / flame.footprint_cm[0]) * (height / flame.footprint_cm[1]);
    let scale = area_ratio.sqrt().clamp(MIN_FOOTPRINT_SCALE, MAX_FOOTPRINT_SCALE);
    let crop = [
        (2.0 * flame.window_half_cm[0] / native_width_cm).min(1.0),
        (2.0 * flame.window_half_cm[1] / native_height_cm).min(1.0),
    ];
    FlamePlane {
        width_cm: crop[0] * native_width_cm * scale,
        height_cm: crop[1] * native_height_cm * scale,
        crop,
    }
}

const PLAYER_FIRE_IGNITE_SECONDS: f32 = 3.0;

#[derive(Default)]
pub(in crate::renderer) struct NinjaFireState {
    scene: Option<FireScene>,
    clock_ms: Option<i32>,
    previous_ms: Option<i32>,
    time: f32,
    fuel_off_seconds: f32,
    player_ignition: f32,
    player_light_level: f32,
    player_light_position: Vec3,
    /// Format passed by the last frame; photo warm-up must match it so the
    /// flame GPU targets are not recreated (which would drop the fields).
    last_format: Option<wgpu::TextureFormat>,
    /// A live HVOL smoke volume wants transient-only cluster lists even in
    /// Legacy/Vertex (`LightingSettings.feature_flags.z & 0x1000`).
    pub(in crate::renderer) hvol_cluster_lights: bool,
    slots: [FlameSlot; FlamePreset::ALL.len()],
    local_post: Option<local_post::LocalPost>,
    local_post_failed: bool,
    local_post_ready: bool,
    /// GPU particle pipelines (turret embers, Burning Man sparks), created on first use.
    ember_compute: Option<EmberCompute>,
    ember_compute_failed: bool,
    spark_curl: Option<wgpu::TextureView>,
    spark_curl_failed: bool,
    /// 1 / RMS of the TilingCurl64 xy lanes (heat haze normalisation).
    spark_curl_inv_rms: f32,
    /// r_ninjaFireHeatHaze resources, created on first use.
    heat_haze: Option<heat_haze::HeatHaze>,
    heat_haze_failed: bool,
    /// This frame the flame composite moves from the world pass to the HDR
    /// post point, after the heat haze has refracted the background.
    composite_after_haze: bool,
    /// GPU MeshField brush capture, created when brush collision is first used.
    capture_pipeline: Option<CapturePipeline>,
    capture_failed: bool,
}

#[derive(Default)]
struct FlameSlot {
    first_id: Option<u64>,
    failed: bool,
    gpu: Option<FlameGpu>,
}

struct FlameGpu {
    target: FlameTarget,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    simulation: [Option<GpuImage>; FLAME_SIM_IMAGES],
    detail: GpuImage,
    mesh: Option<(wgpu::Buffer, u32)>,
    clamp: wgpu::Sampler,
    wrap: wgpu::Sampler,
    fields: Vec<FlameDraw>,
    desc: jka_ninja::FlameDesc,
    /// Additive sprite pipeline for the GPU particle pools, built on first use.
    ember_pipeline: Option<wgpu::RenderPipeline>,
    shader: wgpu::ShaderModule,
    /// Heterogeneous-volume presentation (shared HVOL live shading).
    volume: Option<volume::SmokeVolume>,
    /// Heat haze gather/apply pipelines, built on first use.
    haze: Option<heat_haze::HazePipelines>,
}

struct FlameDraw {
    id: u64,
    field: jka_ninja::FlameField,
    uniform: wgpu::Buffer,
    bindings: Option<(wgpu::TextureView, wgpu::BindGroup)>,
    brush_capture: BrushCapture,
    body_placement_offset: Option<Vec3>,
    simulated_centre_cm: Vec3,
    airflow: FlameAirflow,
    /// Turret embers emitted by this source (r_ninjaFireEmbers).
    embers: Option<ParticlePool>,
    sparks: Option<ParticlePool>,
    smoke: Option<volume::SmokeDraw>,
    /// Per-field history is advanced only when the solver actually ticks.
    previous_blades: Vec<CloudForegroundBlade>,
    previous_blades_time: Option<f32>,
    motion_strokes: Vec<FlameMotionStroke>,
}

#[derive(Default)]
struct FlameAirflow {
    exposure: f32,
    recent_exposure: f32,
}

impl FlameAirflow {
    fn advance(&mut self, speed: f32, dt: f32) -> [f32; 4] {
        // Map units per second. Fade in over a range that includes normal JKA running speed.
        let t = ((speed - 90.0) / (330.0 - 90.0)).clamp(0.0, 1.0);
        let target = t * t * (3.0 - 2.0 * t);
        let fast = 1.0 - (-dt / 0.075).exp();
        let slow = 1.0 - (-dt / 0.45).exp();
        self.exposure += (target - self.exposure) * fast;
        // Remember the recent gust, then release that stored response more slowly.
        let weight = if target >= self.recent_exposure { fast } else { slow };
        self.recent_exposure += (target - self.recent_exposure) * weight;
        let recovery = (self.recent_exposure - self.exposure).max(0.0) * (1.0 - self.exposure);
        [self.exposure, recovery, 0.0, 0.0]
    }
}

/// Reconstruct a swept blade using its real presented endpoints. Match by the
/// hilt first (stable during a swing), then tip (distinguishes a saber staff).
/// Only the blade's motion, never its light, acts on Fluid Ninja's velocity.
fn saber_motion_strokes(
    out: &mut Vec<FlameMotionStroke>,
    current: &[CloudForegroundBlade], previous: &[CloudForegroundBlade],
    elapsed: Option<f32>,
) {
    out.clear();
    if !elapsed.is_some_and(|dt| (0.00001..=0.15).contains(&dt)) { return; }
    let mut used = [false; MAX_CLOUD_FOREGROUND_BLADES];
    for blade in current.iter().take(MAX_CLOUD_FOREGROUND_BLADES) {
        let start = Vec3::from_array(blade.start);
        let end = Vec3::from_array(blade.end);
        if !start.is_finite() || !end.is_finite() || !blade.radius.is_finite() { continue; }
        let match_index = previous.iter().take(MAX_CLOUD_FOREGROUND_BLADES)
            .enumerate()
            .filter(|(i, old)| !used[*i] && (start - Vec3::from_array(old.start)).length_squared() < 48.0_f32.powi(2))
            .min_by(|(_, a), (_, b)| {
                let score = |old: &CloudForegroundBlade| {
                    4.0 * start.distance_squared(Vec3::from_array(old.start))
                        + 0.1 * end.distance_squared(Vec3::from_array(old.end))
                };
                score(a).total_cmp(&score(b))
            }).map(|(i, _)| i);
        let Some(index) = match_index else { continue; };
        used[index] = true;
        let old = previous[index];
        let previous_start = Vec3::from_array(old.start);
        let previous_end = Vec3::from_array(old.end);
        // Use the authored Smoking Man skeletal tracker brush (.8) as the
        // minimum half-strength wake width, plus the blade's actual radius.
        // The 2000 Gaussian factor is from Ninja_Painter's profile.
        let tracker_radius_cm = 0.8 / (0.002 * 2000.0_f32.sqrt());
        let radius_cm = (tracker_radius_cm + blade.radius.max(0.0) * CM_PER_MAP_UNIT).min(24.0);
        for sample in 0..4 {
            let t = (sample as f32 + 0.5) * 0.25;
            let point = start.lerp(end, t);
            let old_point = previous_start.lerp(previous_end, t);
            // Reject jitter, missing-history jumps and teleports. Preserve
            // large but continuous fast saber sweeps.
            let distance = point.distance(old_point);
            if !(0.5..=160.0).contains(&distance) { continue; }
            out.push(FlameMotionStroke {
                current_cm: (point * CM_PER_MAP_UNIT).to_array(),
                previous_cm: (old_point * CM_PER_MAP_UNIT).to_array(),
                radius_cm,
            });
        }
        if out.len() >= FLAME_MOTION_MAX_STROKES { break; }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FlameUniform {
    view_proj: [[f32; 4]; 4],
    centre: [f32; 4],
    right_extent: [f32; 4],
    up_extent: [f32; 4],
    eye_time: [f32; 4],
    forward_and_units: [f32; 4],
    airflow: [f32; 4],
    /// xy: fraction of the field shown across the quad (1 = all of it).
    crop: [f32; 4],
}

/// The pawn's attached PointLight in Fire_Smoke_Explosive: offset from the
/// hull bottom in cm (forward, up, forward x up), linear colour, intensity,
/// radius in cm and legacy falloff exponent.
struct PlayerLight {
    offset_cm: [f32; 3],
    color: [f32; 3],
    intensity: f32,
    radius_cm: f32,
    exponent: f32,
}

/// None when the source pawn has no light (Pawn5, Smoking Man).
fn player_light(effect: PlayerEffect) -> Option<PlayerLight> {
    Some(match effect {
        // The mannequin's movable PointLight is attached to its component,
        // 18.712082 cm forward. FColor (255,57,0) is sRGB.
        PlayerEffect::BurningMan => PlayerLight {
            offset_cm: [18.712082, 0.0, 0.0], color: [1.0, 0.0409152, 0.0],
            intensity: 20.0, radius_cm: 2000.0, exponent: 128.0,
        },
        // Pawn2: Intensity 20, FColor (92,159,255) on the capsule at
        // (34.17, -4.14, 0) cm; capsule centre 88 cm above the hull bottom.
        // Radius and exponent are not overridden on this pawn; Burning Man's
        // light values are reused (unverified).
        PlayerEffect::SmokingFist => PlayerLight {
            offset_cm: [34.166891, 88.0, -4.141075], color: [0.107023, 0.346704, 1.0],
            intensity: 20.0, radius_cm: 2000.0, exponent: 128.0,
        },
        // Pawn6: Intensity 25, FColor (255,54,0), AttenuationRadius 2000,
        // LightFalloffExponent 200 (not inverse squared), at (57.17, 0.59,
        // 66.87) cm from the capsule centre.
        PlayerEffect::BurningMan3 | PlayerEffect::BurningMan3Constant => PlayerLight {
            offset_cm: [57.165394, 154.868304, 0.589683], color: [1.0, 0.0368005, 0.0],
            intensity: 25.0, radius_cm: 2000.0, exponent: 200.0,
        },
        PlayerEffect::SmokingMan | PlayerEffect::SmokingManConstant => return None,
    })
}

fn player_light_position(pose: &PlayerPose, light: &PlayerLight) -> Vec3 {
    let [forward, up, side] = light.offset_cm;
    pose.origin + (pose.forward * forward + Vec3::Y * up + pose.forward.cross(Vec3::Y) * side) / CM_PER_MAP_UNIT
}

fn sources(
    scene: &FireScene,
    preset: FlamePreset,
) -> impl Iterator<Item = (Emitter, Option<&PlayerPose>)> {
    let placed = scene
        .placed
        .iter()
        .flat_map(|emitters| emitters.iter())
        .filter(move |emitter| emitter.preset == preset)
        .map(|emitter| (*emitter, None));
    let player = scene
        .player
        .iter()
        .filter(move |_| scene.player_effect.presets().contains(&preset))
        .map(move |pose| {
            (
                Emitter {
                    id: pose.id,
                    origin: pose.origin,
                    preset,
                    source: EmitterSource::Manual,
                    fit: None,
                },
                Some(pose),
            )
        });
    placed.chain(player)
}

impl NinjaFireState {
    fn set_scene(
        &mut self,
        scene: Option<FireScene>,
        time_ms: Option<i32>,
    ) {
        let previous = self.scene.as_ref().and_then(|scene| scene.player.as_ref());
        if let Some(pose) = scene.as_ref().and_then(|scene| scene.player.as_ref()) {
            let elapsed = previous.filter(|old| old.id == pose.id)
                .map(|old| pose.time_ms.wrapping_sub(old.time_ms))
                .filter(|elapsed| (0..=1000).contains(elapsed));
            let dt = elapsed.unwrap_or_default() as f32 * 0.001;
            if elapsed.is_none() {
                self.fuel_off_seconds = PLAYER_FIRE_DRAIN_SECONDS;
                self.player_ignition = 0.0;
                self.player_light_level = 0.0;
            }
            let effect = scene.as_ref().map_or_else(PlayerEffect::default, |scene| scene.player_effect);
            if pose.emitting {
                self.fuel_off_seconds = 0.0;
                if effect != PlayerEffect::BurningMan {
                    // Only Burning Man ignites over a ramp; the other pawns emit at full strength.
                    self.player_ignition = 1.0;
                } else if previous.is_some_and(|old| old.emitting) {
                    self.player_ignition = (self.player_ignition + dt / PLAYER_FIRE_IGNITE_SECONDS).min(1.0);
                }
                if let Some(light) = player_light(effect) {
                    self.player_light_position = player_light_position(pose, &light);
                }
            } else {
                self.player_ignition = 0.0;
                self.fuel_off_seconds = (self.fuel_off_seconds + dt).min(PLAYER_FIRE_DRAIN_SECONDS);
            }
            // Old light can fade out through a quick re-entry while new fuel starts from zero.
            self.player_light_level = (self.player_light_level - dt / PLAYER_FIRE_DRAIN_SECONDS)
                .max(self.player_fuel());
        } else {
            self.fuel_off_seconds = PLAYER_FIRE_DRAIN_SECONDS;
            self.player_ignition = 0.0;
            self.player_light_level = 0.0;
        }
        for (preset, slot) in FlamePreset::ALL.into_iter().zip(&mut self.slots) {
            // Dynamic map FX can leave the current snapshot/PVS or turn off.
            // Their simulation fields must not remain drawing after their
            // source disappears while some other source keeps the preset live.
            if let Some(gpu) = slot.gpu.as_mut() {
                gpu.fields.retain(|field| scene.as_ref().is_some_and(|scene| {
                    sources(scene, preset).any(|(emitter, _)| emitter.id == field.id)
                }));
            }
            let first_id = scene
                .as_ref()
                .and_then(|scene| sources(scene, preset).next())
                .map(|(emitter, _)| emitter.id);
            if slot.first_id.is_some() != first_id.is_some() {
                // The preset just started or stopped being used.
                *slot = FlameSlot {
                    first_id,
                    ..Default::default()
                };
            } else {
                // Another source became the first one (nearest sources change as
                // the camera moves). Fields are keyed by source ID above, so the
                // others must keep simulating rather than all replaying.
                slot.first_id = first_id;
            }
        }
        if scene.is_none() {
            self.previous_ms = None;
            self.time = 0.0;
            self.local_post = None;
            self.local_post_ready = false;
            self.local_post_failed = false;
        }
        self.scene = scene;
        self.clock_ms = time_ms;
    }

    /// A volume-presented field will march this frame: the froxel lighting
    /// uniform (sun, shadows, clustered lights) must be current.
    pub(in crate::renderer) fn volume_wanted(&self) -> bool {
        self.scene.is_some()
            && FlamePreset::ALL.into_iter().zip(&self.slots).any(|(preset, slot)| {
                slot.first_id.is_some() && !slot.failed && self.draining_or_emitting(preset)
                    && jka_ninja::flame_desc(preset).is_some_and(|desc| desc.volume.is_some())
            })
    }

    /// Shared HVOL live shading for volume presets, composited onto the HDR
    /// scene before post, after the fog HVOL.
    pub(in crate::renderer) fn encode_volumes(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        lights: Option<&wgpu::BindGroup>,
    ) {
        let Some(lights) = lights else {
            crate::ninja_fire_debug::publish_ready_smoke(std::iter::empty());
            return;
        };
        let promoted = self.scene.as_ref().map_or(&[][..], |scene| scene.promoted_smoke.as_slice());
        let mut ready = Vec::new();
        for slot in &mut self.slots {
            let Some(gpu) = &mut slot.gpu else { continue; };
            let Some(volume) = &gpu.volume else { continue; };
            for field in &mut gpu.fields {
                let encoded = field.smoke.as_mut().is_some_and(|smoke| {
                    smoke.encode(volume, device, encoder, scene, depth, lights)
                });
                if encoded
                    && promoted.iter().any(|&(_, id)| id == field.id)
                {
                    ready.push(field.id);
                }
            }
        }
        crate::ninja_fire_debug::publish_ready_smoke(ready);
    }

    pub(in crate::renderer) fn wants_frame(&self) -> bool {
        self.scene.is_some()
            && FlamePreset::ALL.into_iter().zip(&self.slots).any(|(preset, slot)| {
                slot.first_id.is_some() && !slot.failed && self.draining_or_emitting(preset)
            })
    }

    fn draining_or_emitting(&self, preset: FlamePreset) -> bool {
        !preset.body_driven() || self.fuel_off_seconds < PLAYER_FIRE_DRAIN_SECONDS
            || (preset == PROMOTED_SMOKE_PRESET
                && self.scene.as_ref().is_some_and(|scene| !scene.promoted_smoke.is_empty()))
    }

    fn player_fuel(&self) -> f32 {
        // A small flame first, with most of the growth late in the ignition ramp.
        self.player_ignition * self.player_ignition * self.player_ignition
    }

    pub(in crate::renderer) fn append_lights(&self, lights: &mut Vec<super::TransientLight>) {
        let Some((pose, effect)) = self.scene.as_ref().and_then(|scene| Some((scene.player?, scene.player_effect))) else {
            return;
        };
        if !jka_ninja::INCLUDED || self.player_light_level <= 0.0
            || effect.presets().iter().any(|&preset| self.slots[preset as usize].failed)
        {
            return;
        }
        let Some(light) = player_light(effect) else { return };
        let intensity = if crate::ninja_fire_debug::flicker() {
            // Sample at 30 Hz on the presented game clock, independent of FPS.
            // The emitter ID keeps the phase stable when Kyle moves or turns.
            const LIGHT_HZ: i32 = 30;
            let tick = pose.time_ms.rem_euclid(3_600_000) * LIGHT_HZ / 1000;
            let seed = (pose.id as u32).wrapping_mul(0x9E37_79B1);
            light.intensity * fire_flicker(tick as f32 / LIGHT_HZ as f32, seed)
        } else {
            light.intensity
        };
        lights.push(super::TransientLight {
            kind: crate::fx::system::FxLightKind::AuthoredEffect,
            position: self.player_light_position.to_array(),
            color: light.color,
            radius: light.radius_cm / CM_PER_MAP_UNIT,
            source_radius: 0.0,
            intensity: intensity * self.player_light_level,
            radial_exponent: Some(light.exponent),
            spot: None,
            segment: None,
            blade_segments: None,
        });
    }

    /// Decide, before the world pass, whether the flame composite waits for
    /// the heat haze at the HDR post point this frame.
    pub(in crate::renderer) fn plan_heat_haze(&mut self, use_post: bool) {
        self.composite_after_haze = use_post && crate::ninja_fire_debug::heat_haze() && !self.heat_haze_failed
            && self.spark_curl.is_some() && self.local_post.is_some() && self.local_post_ready;
    }

    pub(in crate::renderer) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> bool {
        if let Some(local) = &self.local_post {
            if self.composite_after_haze {
                return false;
            }
            if self.local_post_ready {
                local.draw(pass);
            }
            return self.local_post_ready;
        }
        self.draw_fields(pass)
    }

    fn draw_fields(&self, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let mut submitted = false;
        for (preset, slot) in FlamePreset::ALL.into_iter().zip(&self.slots) {
            // Volume presets composite later, through `encode_volumes`.
            let Some(gpu) = slot.gpu.as_ref()
                .filter(|gpu| gpu.volume.is_none() && self.draining_or_emitting(preset)) else {
                continue;
            };
            let mut fields = gpu.fields.iter().filter_map(|draw| draw.bindings.as_ref());
            let Some((_, first)) = fields.next() else {
                continue;
            };
            pass.set_pipeline(&gpu.pipeline);
            let vertices = if let Some((mesh, vertices)) = gpu.mesh.as_ref() {
                pass.set_vertex_buffer(0, mesh.slice(..));
                *vertices
            } else {
                6
            };
            for bindings in std::iter::once(first).chain(fields.map(|(_, group)| group)) {
                pass.set_bind_group(0, bindings, &[]);
                pass.draw(0..vertices, 0..1);
            }
            if let Some(pipeline) = &gpu.ember_pipeline {
                pass.set_pipeline(pipeline);
                for draw in &gpu.fields {
                    let Some((_, group)) = &draw.bindings else { continue; };
                    if draw.sparks.is_none() && draw.embers.is_none() { continue; }
                    pass.set_bind_group(0, group, &[]);
                    if let Some(sparks) = &draw.sparks { sparks.draw(pass); }
                    if let Some(embers) = &draw.embers { embers.draw(pass); }
                }
            }
            submitted = true;
        }
        submitted
    }

    fn delta(&mut self, frame_delta: f32) -> f64 {
        let seconds = match (self.previous_ms, self.clock_ms) {
            (Some(previous), Some(now)) => {
                let elapsed = now.wrapping_sub(previous);
                if (0..=1000).contains(&elapsed) {
                    f64::from(elapsed) / 1000.0
                } else {
                    for slot in &mut self.slots {
                        slot.gpu = None;
                    }
                    self.time = 0.0;
                    0.0
                }
            }
            // Map-only preview has no presented CGame clock.
            (None, None) => f64::from(frame_delta),
            _ => 0.0,
        };
        self.previous_ms = self.clock_ms;
        self.time += seconds as f32;
        seconds
    }
}

/// View-independent CPU fire state copied from the live renderer into an
/// independent photo-capture renderer (its own GPU fields start empty).
#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct FireCaptureState {
    time: f32,
    fuel_off_seconds: f32,
    player_ignition: f32,
    player_light_level: f32,
    player_light_position: Vec3,
}

impl Renderer {
    pub(in crate::renderer) fn ninja_fire_capture_state(&self) -> FireCaptureState {
        let fire = &self.ninja_fire;
        FireCaptureState { time: fire.time, fuel_off_seconds: fire.fuel_off_seconds, player_ignition: fire.player_ignition,
            player_light_level: fire.player_light_level, player_light_position: fire.player_light_position }
    }

    /// Call after `set_ninja_fire_scene` on the capture renderer: a fresh
    /// scene would otherwise restart Burning Man ignition from zero.
    pub(in crate::renderer) fn install_ninja_fire_capture_state(&mut self, state: FireCaptureState) {
        let fire = &mut self.ninja_fire;
        fire.time = state.time;
        fire.fuel_off_seconds = state.fuel_off_seconds;
        fire.player_ignition = state.player_ignition;
        fire.player_light_level = state.player_light_level;
        fire.player_light_position = state.player_light_position;
    }

    /// Photo capture only. The independent renderer's flame fields are empty
    /// and its capture clock is frozen, so they would never simulate (and a
    /// promoted map flame stays suppressed by the live renderer's ACK). Step
    /// only the fire simulation (no world render) for `seconds` of game time
    /// from `camera`, then refreeze the clock so every face/tile presents the
    /// same field. Requires one prior capture render (GPU targets/format).
    /// This reproduces a settled fire, not the live field's exact history.
    pub(in crate::renderer) fn warm_ninja_fire_for_capture(&mut self, camera: &Camera, seconds: f32,
        cancel: &std::sync::atomic::AtomicBool) -> Result<u32, String> {
        let Some(format) = self.ninja_fire.last_format else { return Ok(0); };
        if !self.ninja_fire.wants_frame() || self.world.is_none() { return Ok(0); }
        const STEP_MS: i32 = 16;
        let size = [self.targets._scene_texture.width().max(1), self.targets._scene_texture.height().max(1)];
        let view_proj = camera.view_projection(size[0], size[1]);
        let clock = self.ninja_fire.clock_ms;
        let steps = (seconds * 1000.0 / STEP_MS as f32).ceil() as u32;
        let mut last = None;
        for step in 0..steps {
            if step % 32 == 0 { super::photo_capture::check_cancel(cancel)?; }
            // One simulation tick per call (the Ninja clock never bursts).
            self.ninja_fire.previous_ms = clock.map(|now| now.wrapping_sub(STEP_MS));
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Photo fire warm-up") });
            self.encode_ninja_fire(&mut encoder, camera, view_proj, view_proj, format, STEP_MS as f32 * 0.001);
            last = Some(self.queue.submit([encoder.finish()]));
        }
        self.ninja_fire.previous_ms = clock;
        if let Some(last) = last {
            self.device.poll(wgpu::PollType::Wait { submission_index: Some(last), timeout: Some(std::time::Duration::from_secs(60)) })
                .map_err(|e| format!("Screenshot fire warm-up wait failed: {e:?}"))?;
        }
        Ok(steps)
    }

    pub(in crate::renderer) fn set_ninja_fire_scene(&mut self, scene: Option<FireScene>, time_ms: Option<i32>) {
        self.ninja_fire.set_scene(scene, time_ms);
        // A new scene may omit a runner that was ready on the previous frame;
        // never suppress its original flame until a current live field ACKs.
        crate::ninja_fire_debug::retain_ready_promoted(self.ninja_fire.scene
            .as_ref().map(|scene| scene.promoted.as_slice()).unwrap_or(&[]));
        crate::ninja_fire_debug::retain_ready_smoke(self.ninja_fire.scene
            .as_ref().map(|scene| scene.promoted_smoke.as_slice()).unwrap_or(&[]));
    }

    pub(in crate::renderer) fn encode_ninja_fire(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        view_proj: Mat4,
        unjittered_view_proj: Mat4,
        format: wgpu::TextureFormat,
        frame_delta: f32,
    ) {
        let Some(scene) = self.ninja_fire.scene.clone() else {
            return;
        };
        self.ninja_fire.last_format = Some(format);
        let delta = self.ninja_fire.delta(frame_delta);
        let wind_time = self.scene_time.or_else(|| self.weather_server_time_ms.map(|ms| ms as f32 * 0.001))
            .unwrap_or_else(|| self.frame_time());
        let [wind_x, wind_y] = self.weather_wind.at(wind_time);
        let wind = Vec3::from_array(crate::scene::render_position([wind_x, wind_y, 0.0]));
        let wind_cm_s = (wind * CM_PER_MAP_UNIT).to_array();
        let airflow_response = crate::ninja_fire_debug::airflow_response();
        let embers_enabled = crate::ninja_fire_debug::embers()
            && !self.ninja_fire.ember_compute_failed;
        let sparks_wanted = !scene.promoted.is_empty() || (scene.player.is_some()
            && scene.player_effect.presets().contains(&FlamePreset::BurningMan));
        if (embers_enabled || sparks_wanted) && !self.ninja_fire.ember_compute_failed
            && self.ninja_fire.ember_compute.is_none()
        {
            match EmberCompute::new(&self.device, &self.queue) {
                Ok(compute) => self.ninja_fire.ember_compute = Some(compute),
                Err(error) => {
                    log::warn!("[NINJA FIRE] Embers and sparks unavailable ({error})");
                    self.ninja_fire.ember_compute_failed = true;
                }
            }
        }
        // The heat haze shares the sparks' TilingCurl64 as its noise.
        let haze_wanted = crate::ninja_fire_debug::heat_haze() && !self.ninja_fire.heat_haze_failed;
        if !crate::ninja_fire_debug::heat_haze() {
            // Release the render-size scratch while the haze is off.
            self.ninja_fire.heat_haze = None;
        }
        if ((sparks_wanted && self.ninja_fire.ember_compute.is_some()) || haze_wanted)
            && self.ninja_fire.spark_curl.is_none() && !self.ninja_fire.spark_curl_failed
        {
            match self.load_ninja_spark_curl() {
                Ok((curl, inv_rms)) => {
                    self.ninja_fire.spark_curl = Some(curl);
                    self.ninja_fire.spark_curl_inv_rms = inv_rms;
                }
                Err(error) => {
                    log::warn!("[NINJA FIRE] Burning Man sparks and heat haze unavailable ({error})");
                    self.ninja_fire.spark_curl_failed = true;
                }
            }
        }
        let embers_enabled = embers_enabled
            && self.ninja_fire.ember_compute.as_ref().is_some_and(|compute| compute.curl32.is_some());
        if scene.world_brushes.is_some() && !self.ninja_fire.capture_failed
            && self.ninja_fire.capture_pipeline.is_none()
        {
            match CapturePipeline::new(&self.device) {
                Ok(pipeline) => self.ninja_fire.capture_pipeline = Some(pipeline),
                Err(error) => {
                    log::warn!("[NINJA FIRE] Brush collision unavailable ({error})");
                    self.ninja_fire.capture_failed = true;
                }
            }
        }
        let main_target = (format, self.msaa_samples, Some(super::depth_format()));
        let surface_wanted = FlamePreset::ALL.into_iter().zip(&self.ninja_fire.slots)
            .any(|(preset, slot)| {
                slot.first_id.is_some() && !slot.failed && self.ninja_fire.draining_or_emitting(preset)
                    && jka_ninja::flame_desc(preset).is_some_and(|desc| desc.volume.is_none())
            });
        // The authored Fire Smoke film and bloom always apply to the flame
        // layer only (r_ninjaFireLocalPost is fixed on).
        if surface_wanted && !self.ninja_fire.local_post_failed && self.ninja_fire.local_post.as_ref()
            .is_none_or(|local| !local.matches(main_target, self.targets.render_size))
        {
            match local_post::LocalPost::new(&self.device, main_target, self.targets.render_size) {
                Ok(local) => self.ninja_fire.local_post = Some(local),
                Err(error) => {
                    log::warn!("[NINJA FIRE] Local post disabled ({error})");
                    self.ninja_fire.local_post = None;
                    self.ninja_fire.local_post_failed = true;
                }
            }
        }
        let target = if self.ninja_fire.local_post.is_some() {
            (wgpu::TextureFormat::Rgba16Float, 1, None)
        } else {
            main_target
        };
        let forward = camera.forward();
        let camera_right = forward.cross(Vec3::Y).normalize_or_zero();
        let roll = glam::Quat::from_axis_angle(forward, camera.roll);
        let right = roll * camera_right;
        let up = roll * camera_right.cross(forward).normalize_or_zero();
        let world_to_clip_cm = (unjittered_view_proj
            * Mat4::from_scale(Vec3::splat(CM_PER_MAP_UNIT.recip())))
        .to_cols_array_2d();
        let body_fuel = self.ninja_fire.player_fuel();
        // Same skybox ambient the ninja fog volume uses for its emission input.
        let sky = self
            .world
            .as_ref()
            .filter(|world| world.primary_skybox.is_some())
            .map_or([0.0; 3], |world| world.sky_average);
        // The scene target the volume composite writes (as the fog HVOL).
        let viewport = [
            self.targets._scene_texture.width().max(1),
            self.targets._scene_texture.height().max(1),
        ];
        for (index, preset) in FlamePreset::ALL.into_iter().enumerate() {
            let slot = &self.ninja_fire.slots[index];
            if slot.failed || slot.first_id.is_none() || !self.ninja_fire.draining_or_emitting(preset) {
                continue;
            }
            let needs_gpu = slot.gpu.as_ref().is_none_or(|gpu| gpu.target != target);
            let result = (|| {
                if needs_gpu {
                    self.ninja_fire.slots[index].gpu = Some(FlameGpu::new(self, target, preset)?);
                }
                let gpu = self.ninja_fire.slots[index]
                    .gpu
                    .as_mut()
                    .expect("flame GPU initialized");
                let ember_scale = embers_enabled.then(|| embers::ember_scale(preset, &gpu.desc)).flatten();
                let particles = self.ninja_fire.ember_compute.as_ref();
                let has_spark_sources = preset == FlamePreset::BurningMan
                    || sources(&scene, preset).any(|(emitter, _)| emitter.source == EmitterSource::Promoted);
                let spark_curl = self.ninja_fire.spark_curl.as_ref().filter(|_| has_spark_sources);
                let spark_compute = particles.filter(|_| spark_curl.is_some());
                let ember_compute = particles.filter(|_| ember_scale.is_some());
                if let Some(compute) = spark_compute.or(ember_compute) {
                    gpu.ensure_ember_pipeline(&self.device, &compute.draw_layout)?;
                }
                let capture_pipeline = self.ninja_fire.capture_pipeline.as_ref();
                let depth = &self.targets.linear_depth_view;
                let [width_cm, height_cm, _] = gpu.desc.extent_cm;
                for (emitter, pose) in sources(&scene, preset) {
                    // Promoted map fire is deliberately isolated from ambient
                    // wind (including material airflow response); manual and
                    // Burning Man flames retain their existing wind behavior.
                    let suppress_wind = emitter.source != EmitterSource::Manual
                        || pose.is_some_and(|pose| pose.suppress_wind);
                    let emitter_wind_cm_s = if suppress_wind { [0.0; 3] } else { wind_cm_s };
                    let draw = match gpu.fields.iter().position(|field| field.id == emitter.id) {
                        Some(i) => &mut gpu.fields[i],
                        None => {
                            gpu.fields.push(FlameDraw::new(
                                &self.device,
                                emitter.id,
                                preset,
                                &gpu.simulation,
                            )?);
                            gpu.fields.last_mut().expect("added field")
                        }
                    };
                    let spark_source = if preset == FlamePreset::BurningMan {
                        pose.map(|pose| {
                            // Decoration actor offset in Manny's placement axes, in centimetres.
                            let anchor = pose.origin + (pose.forward * -4.01242 + Vec3::Y * 181.68214
                                + pose.forward.cross(Vec3::Y) * 5.593614) / CM_PER_MAP_UNIT;
                            (anchor, 1.0, body_fuel)
                        })
                    } else if emitter.source == EmitterSource::Promoted {
                        emitter.fit.map(|fit| {
                            let height = fit.max.y - fit.min.y;
                            let scale = (height * CM_PER_MAP_UNIT / 180.0).clamp(0.25, 1.5);
                            // The shared spark recipe spawns 160 cm below its
                            // anchor. Place those births inside the fitted flame.
                            let spawn = (fit.min + fit.max) * 0.5;
                            let anchor = emitter.origin + spawn + Vec3::Y * (160.0 * scale / CM_PER_MAP_UNIT);
                            (anchor, scale, 1.0)
                        })
                    } else {
                        None
                    };
                    if let (Some(compute), Some(curl), Some((anchor, scale, fuel))) =
                        (spark_compute, spark_curl, spark_source)
                    {
                        if draw.sparks.as_ref().is_none_or(|sparks| (sparks.scale() / scale - 1.0).abs() > 0.01) {
                            draw.sparks = Some(ParticlePool::new(
                                &self.device, &self.queue, compute, curl, PoolKind::BurningManSparks,
                                anchor, scale, emitter.id as u32,
                            )?);
                        }
                        if let Some(sparks) = &mut draw.sparks {
                            sparks.encode(&self.device, &self.queue, encoder, compute, &EmberFrame {
                                anchor,
                                camera: camera.position,
                                fuel,
                                time: self.ninja_fire.time,
                                dt: delta as f32,
                                wind_cm_s: emitter_wind_cm_s,
                                brushes: None,
                            });
                        }
                    }
                    let offset = Vec3::from_array(gpu.desc.centre_offset_cm);
                    let offset = pose.map_or(offset, |pose| {
                        // The source simulation actor is separate from the
                        // skeletal actor. Align their placement once. Facing
                        // changes move the bones inside this window.
                        *draw.body_placement_offset.get_or_insert_with(|| {
                            pose.forward * offset.x + Vec3::Y * offset.y
                                + pose.forward.cross(Vec3::Y) * offset.z
                        })
                    });
                    let promoted_smoke = match (emitter.source, emitter.fit) {
                        (EmitterSource::PromotedSmoke { emitting }, Some(fit)) => {
                            fit.smoke.map(|motion| (emitting, volume::SmokePlume::new(
                                emitter,
                                fit,
                                motion,
                                gpu.desc.extent_cm,
                                camera_right,
                                world_to_clip_cm,
                                self.ninja_fire.time,
                                crate::ninja_fire_debug::smoke_lifetime(),
                            )))
                        }
                        _ => None,
                    };
                    let plume = promoted_smoke.as_ref().map(|(_, plume)| plume);
                    let centre_cm = plume.map_or(emitter.origin * CM_PER_MAP_UNIT + offset, |plume| plume.centre_cm);
                    let (field_right, field_up, field_forward) = if plume.is_some() {
                        (camera_right, Vec3::Y, Vec3::Y.cross(camera_right))
                    } else {
                        (right, up, forward)
                    };
                    let plane_up = if gpu.desc.billboard_world_up { Vec3::Y } else { up };
                    let plane = authored_fire_plane(emitter.fit, preset, width_cm, height_cm);
                    match ember_scale.zip(ember_compute).zip(ember_compute.and_then(|compute| compute.curl32.as_ref())) {
                        Some(((scale, compute), curl)) => {
                            // A promoted torch is drawn magnified; its embers follow.
                            let scale = scale * plane.width_cm / (plane.crop[0] * width_cm).max(1e-3);
                            // Body fires emit from the field's height on the moving player.
                            let anchor = pose.map_or(emitter.origin, |pose| {
                                pose.origin + Vec3::Y * (gpu.desc.centre_offset_cm[1] / CM_PER_MAP_UNIT)
                            });
                            if draw.embers.as_ref().is_none_or(|embers| (embers.scale() / scale - 1.0).abs() > 0.01) {
                                draw.embers = Some(ParticlePool::new(
                                    &self.device, &self.queue, compute, curl, PoolKind::TurretEmbers,
                                    anchor, scale, emitter.id as u32,
                                )?);
                            }
                            if let Some(embers) = draw.embers.as_mut() {
                                embers.encode(&self.device, &self.queue, encoder, compute, &EmberFrame {
                                    anchor,
                                    camera: camera.position,
                                    fuel: pose.map_or(1.0, |pose| if pose.emitting { body_fuel } else { 0.0 }),
                                    time: self.ninja_fire.time,
                                    dt: delta as f32,
                                    wind_cm_s: emitter_wind_cm_s,
                                    brushes: scene.ember_brushes.as_deref(),
                                });
                            }
                        }
                        None => draw.embers = None,
                    }
                    let joints = match &promoted_smoke {
                        Some((true, plume)) => {
                            let mut joints = [None; jka_ninja::FLAME_BODY_JOINTS];
                            joints[0] = Some(plume.source_cm.to_array());
                            Some(joints)
                        }
                        Some((false, _)) => None,
                        None => pose.filter(|pose| pose.emitting).map(|pose| {
                            pose.joints.map(|joint| joint.map(|p| (p * CM_PER_MAP_UNIT).to_array()))
                        }),
                    };
                    // The EFX volume is rescaled for presentation. A native
                    // field-size brush capture would sample the wrong geometry.
                    let world_brush_mask = match capture_pipeline.filter(|_| promoted_smoke.is_none()) {
                        Some(pipeline) => draw.brush_capture.sample(
                            CaptureEncode { device: &self.device, queue: &self.queue, encoder: &mut *encoder, pipeline },
                            scene.world_brushes.as_deref(), centre_cm, right, up, &gpu.desc,
                        ),
                        None => None,
                    };
                    // Physical disturbance is independent of light mode and
                    // the authored skeletal smoke emitters. Promoted EFX use
                    // a separate virtual-to-native map and are not swept here.
                    let native_smoke = gpu.desc.volume.is_some() && promoted_smoke.is_none();
                    if native_smoke {
                        saber_motion_strokes(
                            &mut draw.motion_strokes,
                            &self.cloud_foreground, &draw.previous_blades,
                            draw.previous_blades_time.map(|time| self.ninja_fire.time - time),
                        );
                    } else {
                        draw.motion_strokes.clear();
                    }
                    let stepped = draw.field.encode(
                        &self.queue,
                        encoder,
                        &jka_ninja::FlameFrame {
                            eye_cm: (camera.position * CM_PER_MAP_UNIT).to_array(),
                            centre_cm: centre_cm.to_array(),
                            right: field_right.to_array(),
                            up: field_up.to_array(),
                            forward: field_forward.to_array(),
                            wind_cm_s: plume.map_or(emitter_wind_cm_s, |plume| plume.wind_cm_s),
                            world_to_clip_cm: plume.map_or(world_to_clip_cm, |plume| plume.world_to_clip_cm),
                            game_delta: delta,
                            body_joints_cm: joints.as_ref(),
                            motion_strokes: &draw.motion_strokes,
                            body_scale: plume.map_or_else(|| pose.map_or(1.0, |pose| pose.scale), |plume| plume.body_scale),
                            body_fuel: promoted_smoke.as_ref().map_or(body_fuel, |(emitting, _)| {
                                f32::from(u8::from(*emitting))
                            }),
                            emit_at_rest: promoted_smoke.is_some() || matches!(scene.player_effect,
                                PlayerEffect::SmokingManConstant | PlayerEffect::BurningMan3Constant),
                            smoke_lifetime: plume.map_or(1.0, |plume| plume.lifetime_scale),
                            world_brush_mask,
                        },
                    )?;
                    if stepped {
                        draw.simulated_centre_cm = centre_cm;
                        if native_smoke {
                            draw.previous_blades.clear();
                            draw.previous_blades.extend_from_slice(
                                &self.cloud_foreground[..self.cloud_foreground.len().min(MAX_CLOUD_FOREGROUND_BLADES)],
                            );
                            draw.previous_blades_time = Some(self.ninja_fire.time);
                        } else {
                            draw.previous_blades.clear();
                            draw.previous_blades_time = None;
                        }
                    }
                    let airflow = if airflow_response && !suppress_wind {
                        let source_velocity = pose.filter(|pose| pose.velocity.is_finite())
                            .map_or(Vec3::ZERO, |pose| pose.velocity);
                        let mut response = draw.airflow.advance((wind - source_velocity).length(), delta as f32);
                        if pose.is_some_and(|pose| !pose.emitting) {
                            response[1] = 0.0; // Suppress the recovery flare while burning out.
                        }
                        response
                    } else {
                        draw.airflow = FlameAirflow::default();
                        [0.0; 4]
                    };
                    let Some(field) = draw.field.outputs().velocity_density else {
                        continue;
                    };
                    if let Some(volume) = &gpu.volume {
                        if promoted_smoke.is_some() && !stepped && draw.smoke.is_none() {
                            continue;
                        }
                        let volume_centre = draw.simulated_centre_cm / CM_PER_MAP_UNIT;
                        // BSP lightmaps can light the room without contributing
                        // to the live volume light cache. Sample that baked
                        // lighting at the map smoke source as diffuse incident light.
                        let baked_ambient = if promoted_smoke.is_some() {
                            self.world.as_ref().and_then(|world| world.entity_light_grid.as_ref())
                                .and_then(|grid| grid.sample_classic_entity_light(
                                    crate::scene::jka_position(volume_centre.to_array()),
                                ))
                                .map_or(Vec3::ZERO, |light| {
                                    // Mean clamped cosine over a sphere is 1/4.
                                    (Vec3::from_array(light.ambient)
                                        + 0.25 * Vec3::from_array(light.directed)) / 255.0
                                })
                        } else {
                            Vec3::ZERO
                        };
                        let smoke = draw.smoke.get_or_insert_with(|| volume::SmokeDraw::new(&self.device));
                        smoke.prepare(
                            volume,
                            &self.device,
                            &self.queue,
                            field,
                            &gpu.clamp,
                            &gpu.wrap,
                            &volume::SmokePlacement {
                                centre: volume_centre,
                                size: plume.map(|plume| plume.size),
                                right: field_right,
                                up: field_up,
                                forward: field_forward,
                                time: self.ninja_fire.time,
                                sky,
                                baked_ambient,
                                plume_flow: plume.map_or([0.0; 4], |plume| plume.flow),
                            },
                            &volume::SmokeView {
                                unjittered_view_proj,
                                camera: camera.position,
                                viewport,
                            },
                        );
                        continue;
                    }
                    if draw
                        .bindings
                        .as_ref()
                        .is_none_or(|(old_depth, _)| old_depth != depth)
                    {
                        let validation =
                            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
                        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Ninja flame draw"),
                            layout: &gpu.layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: draw.uniform.as_entire_binding(),
                                },
                                texture(1, field),
                                texture(2, &gpu.detail.view),
                                sampler(3, &gpu.clamp),
                                sampler(4, &gpu.wrap),
                                texture(5, depth),
                            ],
                        });
                        resolve_scopes(memory, validation)?;
                        draw.bindings = Some((depth.clone(), group));
                    }
                    let centre = draw.simulated_centre_cm / CM_PER_MAP_UNIT;
                    let uniform = FlameUniform {
                        view_proj: view_proj.to_cols_array_2d(),
                        centre: centre.extend(0.0).to_array(),
                        right_extent: right.extend(plane.width_cm / CM_PER_MAP_UNIT).to_array(),
                        up_extent: plane_up.extend(plane.height_cm / CM_PER_MAP_UNIT).to_array(),
                        eye_time: camera.position.extend(self.ninja_fire.time).to_array(),
                        forward_and_units: forward.extend(CM_PER_MAP_UNIT).to_array(),
                        airflow,
                        crop: [plane.crop[0], plane.crop[1], 0.0, 0.0],
                    };
                    self.queue
                        .write_buffer(&draw.uniform, 0, bytemuck::bytes_of(&uniform));
                }
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                log::warn!("[NINJA FIRE] {preset:?} disabled ({error})");
                self.ninja_fire.slots[index].gpu = None;
                self.ninja_fire.slots[index].failed = true;
            }
        }
        self.ninja_fire.local_post_ready = surface_wanted && self.ninja_fire.local_post.as_ref()
            .is_some_and(|local| local.encode(encoder, &self.ninja_fire));
        // A post-processing failure can also prevent NinjaFire from drawing.
        // Publish no ACK in that case so the authored EFX flame stays visible.
        let presentation_ready = self.ninja_fire.local_post.is_none()
            || self.ninja_fire.local_post_ready;
        crate::ninja_fire_debug::publish_ready_promoted(scene.promoted.iter().filter_map(|&(_, id)| {
            (presentation_ready && self.ninja_fire.slots.iter().any(|slot| !slot.failed
                && slot.gpu.as_ref().is_some_and(|gpu| gpu.fields.iter()
                    .any(|field| field.id == id && field.bindings.is_some()))))
                .then_some(id)
        }));
    }

    fn load_ninja_spark_curl(&self) -> Result<(wgpu::TextureView, f32), String> {
        let mut search = super::AssetSearchPath::open_game(&self.base, self.detail_texture_game.as_deref())
            .map_err(|error| error.to_string())?;
        let bytes = search.read(embers::SPARK_CURL_QPATH, embers::SPARK_CURL_BYTES)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("required local vector field missing: {}", embers::SPARK_CURL_QPATH))?;
        let rgba16f = embers::curl_rgba16f(&bytes.bytes)?;
        let view = embers::curl_texture(&self.device, &self.queue, "Ninja Burning Man TilingCurl64", 64, &rgba16f)?;
        Ok((view, heat_haze::curl_inv_rms(&bytes.bytes)))
    }

    /// r_ninjaFireHeatHaze on the HDR scene before post: refract the
    /// background over the drawn surface-flame quads only, then composite the
    /// flame layer on top so the flame itself stays sharp. Takes disjoint
    /// renderer fields: the frame's SMAA target keeps `self` mutably borrowed
    /// where this runs.
    pub(in crate::renderer) fn encode_ninja_fire_heat_haze(
        fire: &mut NinjaFireState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        scene_texture: &wgpu::Texture,
    ) {
        if !std::mem::take(&mut fire.composite_after_haze) {
            return;
        }
        Self::encode_heat_haze_passes(fire, device, queue, encoder, scene, scene_texture);
        // The world pass skipped the flame this frame; it must always land.
        if let Some(local) = fire.local_post.as_mut() {
            if let Err(error) = local.draw_onto_scene(device, encoder, scene, scene_texture.format()) {
                log::warn!("[NINJA FIRE] Heat haze unavailable ({error})");
                fire.heat_haze_failed = true;
            }
        }
    }

    fn encode_heat_haze_passes(
        fire: &mut NinjaFireState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        scene_texture: &wgpu::Texture,
    ) {
        let scene_format = scene_texture.format();
        let scene_size = [scene_texture.width(), scene_texture.height()];
        if fire.heat_haze_failed {
            return;
        }
        let (Some(local), Some(curl)) = (fire.local_post.as_ref(), fire.spark_curl.as_ref()) else { return };
        if fire.heat_haze.is_none() {
            match heat_haze::HeatHaze::new(device) {
                Ok(haze) => fire.heat_haze = Some(haze),
                Err(error) => {
                    log::warn!("[NINJA FIRE] Heat haze unavailable ({error})");
                    fire.heat_haze_failed = true;
                    return;
                }
            }
        }
        let haze = fire.heat_haze.as_mut().expect("heat haze initialized");
        // Same selection as `draw_fields`: drawn surface presets only.
        let fuel_left = fire.fuel_off_seconds < PLAYER_FIRE_DRAIN_SECONDS;
        let promoted_smoke = fire.scene.as_ref().is_some_and(|scene| !scene.promoted_smoke.is_empty());
        let drawn = |preset: FlamePreset| !preset.body_driven() || fuel_left
            || (preset == PROMOTED_SMOKE_PRESET && promoted_smoke);
        let mut any = false;
        for (preset, slot) in FlamePreset::ALL.into_iter().zip(&mut fire.slots) {
            let Some(gpu) = slot.gpu.as_mut()
                .filter(|gpu| gpu.volume.is_none() && drawn(preset)
                    && gpu.fields.iter().any(|draw| draw.bindings.is_some())) else {
                continue;
            };
            if let Err(error) = gpu.ensure_haze_pipelines(device, &haze.layout, scene_format) {
                log::warn!("[NINJA FIRE] Heat haze unavailable ({error})");
                fire.heat_haze_failed = true;
                return;
            }
            any = true;
        }
        if !any {
            return;
        }
        haze.prepare(
            device, queue, scene, scene_size, local.layer(), local.size(),
            curl, fire.spark_curl_inv_rms, fire.time,
        );
        let fields = FlamePreset::ALL.into_iter().zip(&fire.slots)
            .filter_map(move |(preset, slot)| slot.gpu.as_ref().filter(|gpu| gpu.volume.is_none() && drawn(preset)))
            .filter_map(|gpu| gpu.haze.as_ref().map(|pipelines| (pipelines, &gpu.fields)))
            .flat_map(|(pipelines, fields)| fields.iter()
                .filter_map(move |draw| draw.bindings.as_ref().map(|(_, group)| (pipelines, group))));
        haze.encode(encoder, scene, fields);
    }
}

impl FlameGpu {
    fn new(renderer: &Renderer, target: FlameTarget, preset: FlamePreset) -> Result<Self, String> {
        let device = &renderer.device;
        let desc =
            jka_ninja::flame_desc(preset).ok_or("Fluid Ninja is not included in this build")?;
        let mut search = super::AssetSearchPath::open_game(
            &renderer.base,
            renderer.detail_texture_game.as_deref(),
        )
        .map_err(|error| error.to_string())?;
        let mut decode = |spec: jka_ninja::FlameImage| {
            let asset = search
                .read(spec.qpath, 16 * 1024 * 1024)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("required local texture missing: {}", spec.qpath))?;
            let image = crate::materials::decode_texture_data_with_color_space(
                spec.qpath,
                &asset.bytes,
                false,
                spec.mipmaps,
                spec.srgb,
            )?;
            if [image.width, image.height] != spec.size {
                return Err(format!(
                    "{} is {}x{}, expected {}x{}",
                    spec.qpath, image.width, image.height, spec.size[0], spec.size[1]
                ));
            }
            Ok::<_, String>(image)
        };
        let images = jka_ninja::flame_input_images(preset);
        let mut decoded = [const { None }; FLAME_SIM_IMAGES];
        for (image, spec) in decoded.iter_mut().zip(images.simulation) {
            *image = spec.map(&mut decode).transpose()?;
        }
        let detail_data = images.detail.map(&mut decode).transpose()?;
        // Volume presentation (FLAME_VOLUME_CONTRACT) through the shared
        // HVOL live shading; the noise and blue noise load like the fog's.
        let volume = match desc.volume {
            Some(volume) => {
                let material = jka_ninja::flame_volume_wgsl(preset).ok_or("Ninja flame volume material missing")?;
                let noise = images.volume_noise.ok_or("Ninja flame volume noise missing")?;
                let noise = super::ninja_fog::image_view(&mut search, device, &renderer.queue, noise)?;
                let blue_noise = super::ninja_fog::image_view(
                    &mut search, device, &renderer.queue, jka_ninja::fog_mist_images().blue_noise,
                )?;
                let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                let smoke = volume::SmokeVolume::new(
                    device, &renderer.weather.fog.resources.layout, renderer.scene_format(),
                    &material, noise, blue_noise, &desc, volume,
                );
                resolve_scopes(memory, validation)?;
                Some(smoke)
            }
            None => None,
        };
        let mesh_data = if let Some(spec) = jka_ninja::flame_mesh(preset) {
            let size = u64::from(spec.vertices) * jka_ninja::FLAME_MESH_VERTEX_BYTES;
            let asset = search
                .read(spec.qpath, size as usize)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("required local mesh missing: {}", spec.qpath))?;
            if asset.bytes.len() as u64 != size {
                return Err(format!("{} requires {size} bytes", spec.qpath));
            }
            Some((asset.bytes, spec.vertices))
        } else {
            None
        };
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let upload = |image| super::textures::upload_texture(device, &renderer.queue, image);
        let simulation = decoded.each_ref().map(|image| image.as_ref().map(upload));
        // Material branches without FlowMapTexture leave binding 2 dormant.
        let detail = detail_data
            .as_ref()
            .map(upload)
            .or_else(|| simulation[0].clone());
        let mesh = mesh_data.map(|(bytes, vertices)| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Ninja flame reference plane"),
                contents: &bytes,
                usage: wgpu::BufferUsages::VERTEX,
            });
            (buffer, vertices)
        });
        let clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja flame field clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let wrap = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja flame flow detail wrap"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ninja flame material"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}\n{}",
                    include_str!("ninja_fire/ember_common.wgsl"),
                    include_str!("ninja_fire.wgsl"),
                    jka_ninja::flame_material_wgsl(preset),
                )
                .into(),
            ),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: if matches!(binding, 1 | 3) {
                wgpu::ShaderStages::VERTEX_FRAGMENT
            } else {
                wgpu::ShaderStages::FRAGMENT
            },
            ty,
            count: None,
        };
        let sample = |filterable| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let filtering = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja bonfire draw"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<FlameUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                entry(1, sample(true)),
                entry(2, sample(true)),
                entry(3, filtering),
                entry(4, filtering),
                entry(5, sample(false)),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ninja bonfire draw"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let mesh_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: jka_ninja::FLAME_MESH_VERTEX_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4],
        })];
        let (vertex_entry, vertex_buffers) = if mesh.is_some() {
            ("flame_mesh_vertex", mesh_layout.as_slice())
        } else {
            ("flame_vertex", &[][..])
        };
        // A volume preset's surface pipeline is built but never drawn.
        let fragment = if target.2.is_some() { "flame_fragment" } else { "flame_layer_fragment" };
        let pipeline = flame_pipeline(
            device, &pipeline_layout, &shader, target, vertex_entry, vertex_buffers, fragment,
            wgpu::BlendState::ALPHA_BLENDING,
        );
        resolve_scopes(memory, validation)?;
        Ok(Self {
            target,
            pipeline,
            layout,
            simulation,
            detail: detail.ok_or("Ninja flame has no texture for its draw bindings")?,
            mesh,
            clamp,
            wrap,
            fields: Vec::new(),
            desc,
            ember_pipeline: None,
            shader,
            volume,
            haze: None,
        })
    }

    /// GPU particle sprites (turret embers, Burning Man sparks) read from
    /// their simulation pool (group 1).
    fn ensure_ember_pipeline(&mut self, device: &wgpu::Device, draw_layout: &wgpu::BindGroupLayout) -> Result<(), String> {
        if self.ember_pipeline.is_some() {
            return Ok(());
        }
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ninja turret ember sprites"),
            bind_group_layouts: &[Some(&self.layout), Some(draw_layout)],
            immediate_size: 0,
        });
        let fragment = if self.target.2.is_some() { "spark_fragment" } else { "spark_layer_fragment" };
        let pipeline = flame_pipeline(
            device, &layout, &self.shader, self.target, "ember_vertex", &[], fragment, ADDITIVE_SPRITES,
        );
        resolve_scopes(memory, validation)?;
        self.ember_pipeline = Some(pipeline);
        Ok(())
    }
}

/// Additive sprites (SrcAlpha, One) that leave the layer's alpha unchanged.
const ADDITIVE_SPRITES: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

#[allow(clippy::too_many_arguments)]
fn flame_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target: FlameTarget,
    vertex_entry: &str,
    vertex_buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
    fragment_entry: &str,
    blend: wgpu::BlendState,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Ninja bonfire scene-linear surface"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex_entry),
            buffers: vertex_buffers,
            compilation_options: Default::default(),
        },
        primitive: Default::default(),
        depth_stencil: target.2.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(false),
            depth_compare: Some(super::camera_depth_compare()),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: target.1,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target.0,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

impl FlameDraw {
    fn new(
        device: &wgpu::Device,
        id: u64,
        preset: FlamePreset,
        simulation: &[Option<GpuImage>; FLAME_SIM_IMAGES],
    ) -> Result<Self, String> {
        let field = jka_ninja::FlameField::new(
            device,
            preset,
            simulation
                .each_ref()
                .map(|image| image.as_ref().map(|image| &image.view)),
        )?;
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ninja bonfire placement"),
            size: std::mem::size_of::<FlameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        resolve_scopes(memory, validation)?;
        Ok(Self {
            id,
            field,
            uniform,
            bindings: None,
            brush_capture: BrushCapture::default(),
            body_placement_offset: None,
            simulated_centre_cm: Vec3::ZERO,
            airflow: FlameAirflow::default(),
            embers: None,
            sparks: None,
            smoke: None,
            previous_blades: Vec::with_capacity(MAX_CLOUD_FOREGROUND_BLADES),
            previous_blades_time: None,
            motion_strokes: Vec::with_capacity(FLAME_MOTION_MAX_STROKES),
        })
    }
}

// Callers push validation first and memory second. Pop both in reverse order,
// even if the inner scope has an error.
fn resolve_scopes(
    memory: wgpu::ErrorScopeGuard,
    validation: wgpu::ErrorScopeGuard,
) -> Result<(), String> {
    let memory = jka_river::river_ninja::checked_scope(memory);
    let validation = jka_river::river_ninja::checked_scope(validation);
    validation.and(memory)
}

fn texture(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn sampler(binding: u32, sampler: &wgpu::Sampler) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Sampler(sampler),
    }
}

#[cfg(test)]
mod authored_fire_extent_tests {
    use super::*;
    use crate::ninja_fire_debug::FlameFit;

    fn fit(width_cm: f32, height_cm: f32) -> FlameFit {
        let (w, h) = (width_cm / CM_PER_MAP_UNIT, height_cm / CM_PER_MAP_UNIT);
        FlameFit {
            min: Vec3::new(-w / 2.0, 0.0, -w / 2.0),
            max: Vec3::new(w / 2.0, h, w / 2.0),
            smoke: None,
        }
    }

    #[test]
    fn plane_scales_uniformly_with_authored_footprint() {
        let native = &TORCH_NATIVE;
        // An EFX exactly as large as the unscaled torch is displayed 1:1.
        let same = authored_fire_plane(
            Some(fit(native.footprint_cm[0], native.footprint_cm[1])), FlamePreset::Torch, 300.0, 300.0);
        assert!((same.width_cm - 2.0 * native.window_half_cm[0]).abs() < 1e-3);
        assert!((same.height_cm - 2.0 * native.window_half_cm[1]).abs() < 1e-3);
        // Doubling both authored dimensions doubles both drawn dimensions.
        let large = authored_fire_plane(
            Some(fit(native.footprint_cm[0] * 2.0, native.footprint_cm[1] * 2.0)), FlamePreset::Torch, 300.0, 300.0);
        assert!((large.width_cm / same.width_cm - 2.0).abs() < 1e-4);
        assert!((large.height_cm / same.height_cm - 2.0).abs() < 1e-4);
        // Doubling only the width must not distort the fluid: the aspect holds.
        let wide = authored_fire_plane(
            Some(fit(native.footprint_cm[0] * 2.0, native.footprint_cm[1])), FlamePreset::Torch, 300.0, 300.0);
        assert!((wide.width_cm / wide.height_cm - same.width_cm / same.height_cm).abs() < 1e-4);
        // A crop never exceeds the field.
        assert!(same.crop.iter().all(|c| *c <= 1.0));
    }

    #[test]
    fn uncalibrated_or_unparsed_sources_keep_the_native_plane() {
        let native = FlamePlane { width_cm: 300.0, height_cm: 300.0, crop: [1.0, 1.0] };
        assert_eq!(authored_fire_plane(None, FlamePreset::Torch, 300.0, 300.0), native);
        assert_eq!(authored_fire_plane(Some(fit(60.0, 60.0)), FlamePreset::Bonfire, 300.0, 300.0), native);
    }
}
