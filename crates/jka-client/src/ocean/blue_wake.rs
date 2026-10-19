//! Host driver for the preserved Blue Water ship wake (`blue-water-fx`): dispersive wake field, trail foam and slick,
//! torpedo tracks, analytic bow waves, hull contact foam and impact rings, shaded by the upstream surface's wake
//! sampler. This exists beside JKA's own water interaction so the two can be A/B'd (`r_oceanWake`); it only reaches the
//! native / Hybrid Blue Water surface, which is where that sampler lives.
//!
//! JKA has no ships yet. Hulls come from [`WakeCommand::Ship`] (a future ship system, or a script) or from render-side
//! demo ships that sail themselves, so the wake can be reviewed today. Hulls are Blue Water's proxy `SimpleHull`
//! profiles; nothing here draws a hull.
use std::collections::HashMap;

use blue_water_fx::{
    hull_profile::{SimpleHull, WaterlineProfile},
    ship_wake::{ShipWake, WakeCamera, WakeInputs, WakeShip, WakeSplash},
    torpedo_tracks::TrackTorpedo,
    wake::{self, impulses::Strike},
};

use super::OCEAN_UNITS_PER_METER;

/// Proxy hull sizes. Waterline length, beam, draft (m) and the speed (m/s) the hull's wake saturates at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShipClass {
    Boat,
    Destroyer,
    Battleship,
}

impl ShipClass {
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "boat" | "small" => Some(Self::Boat),
            "destroyer" | "dd" | "medium" => Some(Self::Destroyer),
            "battleship" | "bb" | "large" => Some(Self::Battleship),
            _ => None,
        }
    }

    fn dimensions(self) -> (f64, f64, f64, f64) {
        match self {
            Self::Boat => (14.0, 4.0, 0.9, 12.0),
            Self::Destroyer => (115.0, 12.0, 4.2, 18.0),
            Self::Battleship => (251.0, 36.0, 9.3, 15.0),
        }
    }

    const ALL: [Self; 3] = [Self::Boat, Self::Destroyer, Self::Battleship];
}

/// What strikes the sea (`Strike` in the upstream impulses).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StrikeKind {
    /// A round of `caliber_m` entering at `speed` m/s.
    Round { caliber_m: f32, speed: f32 },
    /// `kg` of TNT-equivalent `depth` m under the surface.
    Charge { kg: f32, depth: f32 },
    /// An aircraft or wreck striking the water.
    Aircraft { speed: f32 },
}

#[derive(Clone, Debug)]
pub enum WakeCommand {
    /// Drive (or create) a ship. Position in render units (y ignored), heading in radians clockwise from above with 0
    /// toward -Z, speed in m/s.
    Ship {
        id: String,
        class: ShipClass,
        position: [f32; 3],
        heading: f32,
        speed: f32,
    },
    RemoveShip(String),
    ClearShips,
    /// A self-sailing ship placed ahead of the camera on its next frame: a circle of `radius` m (0: a straight run).
    Demo {
        class: ShipClass,
        speed: f32,
        radius: f32,
    },
    ClearDemos,
    /// A strike at render-unit `x`, `z`.
    Strike { x: f32, z: f32, kind: StrikeKind },
    /// A strike `distance_m` metres ahead of the camera on its next frame (console / scripting).
    StrikeAhead { distance_m: f32, kind: StrikeKind },
    /// Clear the field and every trail.
    Reset,
}

struct Driven {
    class: ShipClass,
    position: [f64; 2],
    heading: f64,
    speed: f64,
    /// Frames since a command last touched it; ships nobody updates are dropped after a few seconds.
    idle: f64,
}

struct Demo {
    id: String,
    class: ShipClass,
    speed: f64,
    radius: f64,
    /// Where it starts, set on the first frame it can see the camera.
    start: Option<Start>,
    travelled: f64,
}

struct Start {
    centre: [f64; 2],
    angle: f64,
    heading: f64,
}

pub struct BlueWake {
    pub wake: ShipWake,
    resolution: u32,
    profiles: HashMap<ShipClass, WaterlineProfile>,
    driven: HashMap<String, Driven>,
    demos: Vec<Demo>,
    demo_serial: u32,
    strikes: Vec<(WakeSplash, Strike)>,
    strikes_ahead: Vec<(f64, StrikeKind)>,
    sequence: u64,
    /// Seconds the wake keeps simulating after the last hull or strike: long enough for a trail and its rings to die.
    active_for: f64,
}

/// Seconds a driven ship may go without an update before its wake generators are released.
const DRIVEN_TIMEOUT: f64 = 3.0;
const MAX_DEMOS: usize = 4;
const QUIET_AFTER: f64 = 120.0;

impl BlueWake {
    /// A wake for the upstream High tier (512² field) unless the device is asked for another.
    pub fn new(device: &wgpu::Device, resolution: u32) -> Self {
        let profiles = ShipClass::ALL
            .into_iter()
            .map(|class| {
                let (length, beam, draft, _) = class.dimensions();
                (
                    class,
                    SimpleHull::warship(length as f32, beam as f32, draft as f32).profile(),
                )
            })
            .collect();
        Self {
            wake: ShipWake::new(device, resolution, 1.0),
            resolution,
            profiles,
            driven: HashMap::new(),
            demos: Vec::new(),
            demo_serial: 0,
            strikes: Vec::new(),
            strikes_ahead: Vec::new(),
            sequence: 0,
            active_for: 0.0,
        }
    }

    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    pub fn command(&mut self, device: &wgpu::Device, command: WakeCommand) {
        match command {
            WakeCommand::Ship { id, class, position, heading, speed } => {
                let state = Driven {
                    class,
                    position: [
                        f64::from(position[0] / OCEAN_UNITS_PER_METER),
                        f64::from(position[2] / OCEAN_UNITS_PER_METER),
                    ],
                    heading: f64::from(heading),
                    speed: f64::from(speed),
                    idle: 0.0,
                };
                if state.position.iter().all(|v| v.is_finite()) && state.heading.is_finite() {
                    self.driven.insert(id, state);
                }
            }
            WakeCommand::RemoveShip(id) => {
                self.driven.remove(&id);
            }
            WakeCommand::ClearShips => self.driven.clear(),
            WakeCommand::Demo { class, speed, radius } => {
                if self.demos.len() >= MAX_DEMOS {
                    self.demos.remove(0);
                }
                self.demo_serial += 1;
                self.demos.push(Demo {
                    id: format!("demo-{}", self.demo_serial),
                    class,
                    speed: f64::from(speed.clamp(0.0, 40.0)),
                    radius: f64::from(radius.max(0.0)),
                    start: None,
                    travelled: 0.0,
                });
            }
            WakeCommand::ClearDemos => self.demos.clear(),
            WakeCommand::Strike { x, z, kind } => {
                self.strike(
                    f64::from(x / OCEAN_UNITS_PER_METER),
                    f64::from(z / OCEAN_UNITS_PER_METER),
                    kind,
                );
            }
            WakeCommand::StrikeAhead { distance_m, kind } => {
                self.strikes_ahead.push((f64::from(distance_m.clamp(0.0, 2000.0)), kind));
            }
            WakeCommand::Reset => {
                if let Some(field) = &mut self.wake.field {
                    field.reset(device);
                }
                self.wake.version += 1;
            }
        }
    }

    fn strike(&mut self, x: f64, z: f64, kind: StrikeKind) {
        let (strike, caliber) = match kind {
            StrikeKind::Round { caliber_m, speed } => (
                Strike::Round { caliber_m: f64::from(caliber_m), speed: f64::from(speed) },
                f64::from(caliber_m),
            ),
            StrikeKind::Charge { kg, depth } => {
                (Strike::Charge { kg: f64::from(kg), depth: f64::from(depth) }, 0.4)
            }
            StrikeKind::Aircraft { speed } => (Strike::Aircraft { speed: f64::from(speed) }, 0.5),
        };
        self.sequence += 1;
        self.strikes.push((WakeSplash { sequence: self.sequence, x, z, caliber_m: caliber }, strike));
    }

    /// Advance every hull, run this frame's CPU wake update and record its GPU half. `eye` is the camera in metres
    /// (x, height above the water, z), `mesh` the clipmap detail the surface is about to draw with.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        dt: f64,
        eye: [f64; 3],
        forward: [f64; 3],
        fov_y: f64,
        drawing_height: f64,
        sea_height: f64,
        mesh: MeshDetail,
    ) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        let flat = forward[0].hypot(forward[2]).max(1e-6);
        for (distance, kind) in std::mem::take(&mut self.strikes_ahead) {
            self.strike(eye[0] + forward[0] / flat * distance, eye[2] + forward[2] / flat * distance, kind);
        }
        if self.driven.is_empty() && self.demos.is_empty() && self.strikes.is_empty() {
            self.active_for = (self.active_for - dt).max(0.0);
        } else {
            self.active_for = QUIET_AFTER;
        }
        for ship in self.driven.values_mut() {
            ship.idle += dt;
        }
        self.driven.retain(|_, ship| ship.idle < DRIVEN_TIMEOUT);
        // Demo ships sail themselves; the first frame places each ahead of the camera.
        for demo in &mut self.demos {
            let start = demo.start.get_or_insert_with(|| {
                let flat = forward[0].hypot(forward[2]).max(1e-6);
                let ahead = [forward[0] / flat, forward[2] / flat];
                if demo.radius > 0.0 {
                    // Circle with the camera outside it, the ship first crossing in front of the view.
                    let centre = [
                        eye[0] + ahead[0] * (demo.radius + 40.0),
                        eye[2] + ahead[1] * (demo.radius + 40.0),
                    ];
                    let angle = ahead[1].atan2(ahead[0]) + std::f64::consts::PI;
                    Start { centre, angle, heading: 0.0 }
                } else {
                    // A straight run across the view from the left, 120 m out.
                    let right = [-ahead[1], ahead[0]];
                    let origin = [
                        eye[0] + ahead[0] * 120.0 - right[0] * 150.0,
                        eye[2] + ahead[1] * 120.0 - right[1] * 150.0,
                    ];
                    Start {
                        centre: origin,
                        angle: 0.0,
                        heading: right[0].atan2(-right[1]),
                    }
                }
            });
            demo.travelled += demo.speed * dt;
            let (x, z, heading) = if demo.radius > 0.0 {
                let angle = start.angle + demo.travelled / demo.radius;
                let x = start.centre[0] + demo.radius * angle.cos();
                let z = start.centre[1] + demo.radius * angle.sin();
                let (vx, vz) = (-angle.sin(), angle.cos());
                (x, z, vx.atan2(-vz))
            } else {
                let (sin, cos) = start.heading.sin_cos();
                (
                    start.centre[0] + sin * demo.travelled,
                    start.centre[1] - cos * demo.travelled,
                    start.heading,
                )
            };
            self.driven.insert(
                demo.id.clone(),
                Driven {
                    class: demo.class,
                    position: [x, z],
                    heading,
                    speed: demo.speed,
                    idle: 0.0,
                },
            );
        }
        let Self { wake, profiles, driven, strikes, .. } = self;
        // The focus hull (the field centres on it) is the one nearest the camera.
        let mut order: Vec<(&String, &Driven)> = driven.iter().collect();
        order.sort_by(|a, b| {
            let d = |s: &Driven| (s.position[0] - eye[0]).hypot(s.position[1] - eye[2]);
            d(a.1).total_cmp(&d(b.1)).then_with(|| a.0.cmp(b.0))
        });
        let ships: Vec<WakeShip<'_>> = order
            .iter()
            .map(|(id, ship)| {
                let (length, beam, draft, top) = ship.class.dimensions();
                WakeShip {
                    id,
                    position: [ship.position[0], 0.0, ship.position[1]],
                    heading: ship.heading,
                    pitch: 0.0,
                    roll: 0.0,
                    speed: ship.speed,
                    length,
                    beam,
                    draft,
                    center_x: 0.0,
                    center_z: 0.0,
                    forward_speed: top,
                    reverse_speed: top * 0.4,
                    profile: profiles.get(&ship.class),
                }
            })
            .collect();
        // With no hull the field keeps its last centre; a strike elsewhere recentres it so impacts still ring.
        let splashes: Vec<WakeSplash> = strikes.iter().map(|(splash, _)| *splash).collect();
        if ships.is_empty() {
            if let (Some(field), Some((splash, _))) = (&mut wake.field, strikes.first()) {
                field.set_center(splash.x, splash.z);
            }
        }
        for (splash, strike) in strikes.iter() {
            if let Some(field) = &mut wake.field {
                field.strike(splash.x, splash.z, strike);
            }
        }
        strikes.clear();
        wake.bow_waves.mesh_centre = [eye[0], eye[2]];
        wake.bow_waves.mesh_fan = false;
        wake.bow_waves.vertex_spacing = mesh.spacing;
        wake.bow_waves.mesh_growth = mesh.growth;
        wake.update(&WakeInputs {
            ships: &ships,
            dt,
            splashes: &splashes,
            torpedoes: &[] as &[TrackTorpedo],
            camera: WakeCamera {
                eye,
                forward,
                fov_y,
                zoom: 1.0,
                drawing_height,
            },
            sea_height,
        });
        wake.encode(device, queue, encoder);
    }

    /// Whether anything still needs the wake (a hull, an ongoing strike or a ringing field), so an idle sea can skip
    /// the field's compute and drop to the calm bindings.
    pub fn busy(&self) -> bool {
        self.active_for > 0.0
            || !self.driven.is_empty()
            || !self.demos.is_empty()
            || !self.strikes.is_empty()
            || !self.strikes_ahead.is_empty()
    }

    pub fn resolution_for(tier: blue_water_fx::QualityTier) -> u32 {
        wake::resolution_for(tier)
    }
}

/// The clipmap's vertex spacing the bow waves squeeze themselves to (`BowWaves::vertex_spacing`, `mesh_growth`).
#[derive(Clone, Copy, Debug)]
pub struct MeshDetail {
    pub spacing: f64,
    pub growth: f64,
}
