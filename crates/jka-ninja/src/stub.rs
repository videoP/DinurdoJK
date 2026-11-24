//! Public stub: built when the private DinurdoJK-ninja checkout is absent.
//! Mirrors every item consumers use from the private implementation, with
//! constructors returning `Err(NOT_INCLUDED)`. Nothing allocates.
use crate::flame_api::{FlameDesc, FlameFrame, FlameImages, FlameMesh, FlameOutputs, FlamePreset, FLAME_SIM_IMAGES};
use crate::fog_api::{FogDesc, FogFrame, FogImages, FogMistImages, FogOutputs, FogPreset, FOG_SIM_IMAGES};
use crate::river_api::{NinjaUnderwaterSettings, RiverDesc, RiverFrame, RiverImage, RiverOutputs, RiverVolumeImage, RiverWakeDebug};

/// Error returned by every stub constructor.
pub const NOT_INCLUDED: &str = "Fluid Ninja is not included in this build";

pub struct FogField(std::convert::Infallible);

impl FogField {
    pub fn new(
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _desc: FogDesc,
        _images: [&wgpu::TextureView; FOG_SIM_IMAGES],
        _mist_inputs: Option<[&wgpu::TextureView; 3]>,
    ) -> Result<Self, String> {
        Err(NOT_INCLUDED.into())
    }

    pub fn encode(
        &mut self,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        _frame: &FogFrame<'_>,
    ) -> Result<bool, String> {
        match self.0 {}
    }

    pub fn outputs(&self) -> FogOutputs<'_> {
        match self.0 {}
    }
}

pub fn fog_desc(_preset: FogPreset) -> Option<FogDesc> { None }
pub fn fog_input_images(_preset: FogPreset) -> Option<FogImages> { None }
pub fn fog_mist_images() -> FogMistImages {
    unreachable!("Fog_Mist requires the private Fluid Ninja implementation")
}

pub fn fog_mist_wgsl() -> &'static str {
    "fn ninja_mist_material(world: vec3<f32>, ground: f32, eye: vec3<f32>, sun: vec3<f32>, ddx: vec3<f32>, ddy: vec3<f32>, hvol: bool) -> NinjaFogSample {
        return NinjaFogSample(0.0, vec3<f32>(0.0), vec3<f32>(0.0));
    }"
}

pub fn fog_volume_wgsl() -> &'static str {
    "fn ninja_fog_density(world_cm: vec3<f32>, eye_cm: vec3<f32>, base_cm: vec3<f32>,
         width_cm: f32, height_cm: f32, time: f32, preset: u32, sky_rgb: vec3<f32>,
         light_direction: vec3<f32>, extend: bool,
         world_ddx_cm: vec3<f32>, world_ddy_cm: vec3<f32>) -> NinjaFogSample {
         return NinjaFogSample(0.0, vec3<f32>(0.0), vec3<f32>(0.0));
     }"
}

pub fn flame_desc(_preset: FlamePreset) -> Option<FlameDesc> { None }
pub fn flame_mesh(_preset: FlamePreset) -> Option<FlameMesh> { None }
pub fn flame_demo_film_wgsl() -> &'static str {
    "fn ninja_demo_film(color: vec3<f32>) -> vec3<f32> { return color; }"
}
pub fn flame_input_images(_preset: FlamePreset) -> FlameImages {
    FlameImages { simulation: [const { None }; FLAME_SIM_IMAGES], detail: None, volume_noise: None }
}

pub fn flame_volume_wgsl(_preset: FlamePreset) -> Option<String> { None }

pub fn flame_material_wgsl(_preset: FlamePreset) -> String {
    "fn ninja_flame_shade(uv: vec2<f32>, time: f32, depth_gap_cm: f32) -> vec4<f32> { return vec4<f32>(0.0); }
     fn ninja_flame_offset_cm(uv: vec2<f32>, normal: vec3<f32>) -> vec3<f32> { return vec3<f32>(0.0); }".into()
}

pub struct FlameField(std::convert::Infallible);

impl FlameField {
    pub fn new(
        _device: &wgpu::Device, _preset: FlamePreset, _images: [Option<&wgpu::TextureView>; FLAME_SIM_IMAGES],
    ) -> Result<Self, String> { Err(NOT_INCLUDED.into()) }

    pub fn encode(
        &mut self, _queue: &wgpu::Queue, _encoder: &mut wgpu::CommandEncoder, _frame: &FlameFrame<'_>,
    ) -> Result<bool, String> { match self.0 {} }

    pub fn outputs(&self) -> FlameOutputs<'_> { match self.0 {} }
}

/// Input images the river field needs; none without the implementation.
pub fn river_input_images() -> &'static [RiverImage] { &[] }
pub fn river_caustics_image() -> Option<RiverVolumeImage> { None }
pub fn river_underwater_images() -> Option<[RiverImage; 2]> { None }
pub fn river_underwater_defaults() -> NinjaUnderwaterSettings {
    NinjaUnderwaterSettings {
        distance_mult: 0.0,
        distortion: 0.0,
        brightness: 1.0,
        caustics_mult: 0.0,
        distance_color: [0.0; 3],
        caustics_color: [1.0; 3],
    }
}

/// Inert material: same signature as the private Large White 3 shade.
pub fn underwater_wgsl() -> &'static str {
    "fn ninja_underwater_distortion(uv: vec2<f32>, time: f32) -> vec2<f32> {
    return vec2<f32>(0.0);
}
fn ninja_underwater_color(scene: vec3<f32>, depth_cm: f32, uv: vec2<f32>, pixel: vec2<i32>) -> vec3<f32> {
    return scene;
}
"
}
pub fn river_material_wgsl() -> &'static str {
    "fn ninja_river_shade(uv: vec2<f32>, world: vec3<f32>, eye: vec3<f32>, pixel_depth: f32, time: f32, two_sided_sign: f32) -> RiverNinjaSurface {
    return RiverNinjaSurface(0.0, vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0), 0.0, 0.0, 0.0, 1.0);
}
fn ninja_river_color_scale(uv: vec2<f32>, world: vec3<f32>, eye: vec3<f32>, extent: vec2<f32>, pixel_depth: f32, scene_depth: f32, time: f32) -> vec3<f32> {
    return vec3<f32>(0.0);
}
fn ninja_river_volume_coefficients(color_scale: vec3<f32>) -> RiverSlwCoefficients {
    return RiverSlwCoefficients(vec3<f32>(0.0), vec3<f32>(0.0), 0.0, color_scale);
}
fn ninja_underwater_distortion(uv: vec2<f32>, time: f32) -> vec2<f32> {
    return vec2<f32>(0.0);
}
fn ninja_underwater_color(scene: vec3<f32>, depth_cm: f32, uv: vec2<f32>, pixel: vec2<i32>) -> vec3<f32> {
    return scene;
}
"
}

/// Shared wake joint contact band half-height (unused without the private field).
pub fn wake_contact_half_height_cm() -> f32 { 0.0 }
/// r_ninjaWakePush: no Fluid Ninja field to scale in public builds.
pub fn set_wake_push(_scale: f32) {}
pub fn wake_push() -> f32 { 1.0 }

/// River presentation field. Uninhabited in the stub: `new` always fails.
pub struct RiverField(std::convert::Infallible);

impl RiverField {
    pub fn new(_device: &wgpu::Device, _desc: &RiverDesc) -> Result<Self, String> { Err(NOT_INCLUDED.into()) }
    pub fn desc(&self) -> &RiverDesc { match self.0 {} }
    pub fn encode(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, _encoder: &mut wgpu::CommandEncoder, _frame: &RiverFrame<'_>) -> Result<bool, String> { match self.0 {} }
    pub fn outputs(&self) -> RiverOutputs<'_> { match self.0 {} }
    pub fn wake_debug(&self) -> RiverWakeDebug<'_> { match self.0 {} }
}
/// Public builds keep the host's existing blade shaders.
pub fn grass_blade_wgsl() -> Option<crate::GrassBladeShaders> {
    None
}

pub fn snow_material_wgsl(_profile: crate::SnowProfile) -> Option<crate::SnowMaterialShaders> {
    None
}

pub fn snow_drift_volume_wgsl() -> &'static str {
    "fn ninja_snow_dust_density(world: vec3<f32>, ground_y: f32, center: vec2<f32>, extent: f32, game_time: f32, sky_rgb: vec3<f32>) -> NinjaSnowDustSample { return NinjaSnowDustSample(0.0, vec3<f32>(0.0), vec3<f32>(0.0)); }"
}

pub struct SnowField(std::convert::Infallible);
impl SnowField {
    pub fn new(_device: &wgpu::Device, _profile: crate::SnowProfile, _images: [&wgpu::TextureView; 4]) -> Result<Self, String> { Err(NOT_INCLUDED.into()) }
    pub fn view(&self) -> &wgpu::TextureView { match self.0 {} }
    pub fn painter_view(&self) -> &wgpu::TextureView { match self.0 {} }
    pub fn step(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, _contacts: &[crate::SnowContact], _origin_cm: [f32; 2], _previous_origin_cm: [f32; 2], _time: f32, _delta: f64, _reset: bool) -> Result<bool, String> { match self.0 {} }
}

// Blood remains unavailable without both the private solver and its exact material.
use crate::blood_api::{BloodDesc, BloodOutputs, BloodSeed, BLOOD_SIM_IMAGES};
pub fn blood_desc() -> Option<BloodDesc> { None }
pub fn blood_input_images() -> Option<[crate::FlameImage; BLOOD_SIM_IMAGES]> { None }
pub fn blood_material_available() -> bool { false }
pub fn blood_material_wgsl() -> Option<String> { None }
pub struct BloodField(std::convert::Infallible);
impl BloodField {
    pub fn new(_device: &wgpu::Device, _images: [&wgpu::TextureView; BLOOD_SIM_IMAGES], _seed: BloodSeed) -> Result<Self, String> { Err(NOT_INCLUDED.into()) }
    pub fn encode(&mut self, _queue: &wgpu::Queue, _encoder: &mut wgpu::CommandEncoder, _game_delta: f64, _motions: &[crate::BloodMotion], _ink: Option<crate::BloodInk<'_>>, _skip_seed: bool) -> Result<bool, String> { match self.0 {} }
    pub fn restart(&mut self, _device: &wgpu::Device, _encoder: &mut wgpu::CommandEncoder, _seed: BloodSeed) -> Result<(), String> { match self.0 {} }
    pub fn outputs(&self) -> BloodOutputs<'_> { match self.0 {} }
}

// Public-only builds never create the private Ocean2 solver.
pub fn ocean_images() -> Option<crate::OceanImages> { None }
pub fn ocean_material_wgsl() -> Option<String> { None }
pub struct OceanField(std::convert::Infallible);
impl OceanField {
    pub fn new(_device: &wgpu::Device, _desc: &crate::OceanDesc, _images: [&wgpu::TextureView; 4]) -> Result<Self, String> { Err(NOT_INCLUDED.into()) }
    pub fn capture_resolution(&self) -> [u32; 2] { match self.0 {} }
    pub fn wants_step(&self, _delta: f64, _crest: bool) -> Result<bool, String> { match self.0 {} }
    pub fn encode(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, _encoder: &mut wgpu::CommandEncoder, _frame: &crate::OceanFrame<'_>) -> Result<bool, String> { match self.0 {} }
    pub fn outputs(&self) -> crate::OceanOutputs<'_> { match self.0 {} }
}

// Public-only builds never create the private Coast solver.
pub const COAST_CLAMP_CM: f32 = -480.0;
pub const COAST_SIM_Z_CM: f32 = -700.0;
pub fn coast_images() -> Option<crate::OceanImages> { None }
pub fn ocean_surface_volume(_coast: bool) -> Option<([f32; 3], [f32; 3])> { None }
pub fn coast_material_wgsl() -> Option<String> { None }
pub fn coast_whitewater_sim_wgsl() -> Option<String> { None }
pub fn coast_whitewater_curl_field() -> Option<&'static [u8]> { None }
pub const COAST_WHITEWATER_CAPACITY: u32 = 0;
pub const COAST_WHITEWATER_SPAWN_RATE: f32 = 0.0;
pub const COAST_LANDSCAPE_NORMAL_GAIN: f32 = 10.0;
pub const COAST_SEA_FOOTPRINT_SLOPE: f32 = 0.040;
pub struct CoastField(std::convert::Infallible);
impl CoastField {
    pub fn set_landscape_normal_gain(&mut self, _gain: f32) { match self.0 {} }
    pub fn new(_device: &wgpu::Device, _queue: &wgpu::Queue, _desc: &crate::CoastDesc, _images: [&wgpu::TextureView; 4], _terrain: &wgpu::TextureView) -> Result<Self, String> { Err(NOT_INCLUDED.into()) }
    pub fn capture_resolution(&self) -> [u32; 2] { match self.0 {} }
    pub fn shift(&mut self, _cells: [i32; 2]) { match self.0 {} }
    pub fn solver_resolution(&self) -> [u32; 2] { match self.0 {} }
    pub fn seed_velocity_density(&mut self, _queue: &wgpu::Queue, _rgba_f16: &[u16]) -> Result<(), String> { match self.0 {} }
    pub fn wants_step(&self, _delta: f64, _crest: bool) -> Result<bool, String> { match self.0 {} }
    pub fn encode(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, _encoder: &mut wgpu::CommandEncoder, _frame: &crate::CoastFrame<'_>) -> Result<bool, String> { match self.0 {} }
    pub fn outputs(&self) -> crate::CoastOutputs<'_> { match self.0 {} }
}

// Public-only builds never create the private lake wake field.
pub fn lake_images() -> Option<crate::OceanImages> { None }
pub fn lake_wake_wgsl() -> Option<String> { None }
pub struct LakeField(std::convert::Infallible);
impl LakeField {
    pub fn new(_device: &wgpu::Device, _desc: &crate::LakeDesc, _images: [&wgpu::TextureView; 4]) -> Result<Self, String> { Err(NOT_INCLUDED.into()) }
    pub fn cell_cm(&self) -> f32 { match self.0 {} }
    pub fn shift(&mut self, _cells: [i32; 2]) { match self.0 {} }
    pub fn encode(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, _encoder: &mut wgpu::CommandEncoder, _frame: &crate::LakeFrame<'_>) -> Result<bool, String> { match self.0 {} }
    pub fn outputs(&self) -> crate::LakeOutputs<'_> { match self.0 {} }
}
