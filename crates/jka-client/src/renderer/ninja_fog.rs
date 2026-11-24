use super::{AssetSearchPath, Mat4, Renderer, Vec3};
use crate::ninja_fog_debug::{CM_PER_MAP_UNIT, FogSurface, Placement, SURFACE_SIZE};
use bytemuck::{Pod, Zeroable};
use jka_ninja::{FogField, FogFrame, FogImage, FogPreset, FogStroke};
use jka_river::river_ninja::{ActorHistory, ActorKind, ActorSample, FOG_BONE_SLOTS};
use std::collections::HashMap;
use std::sync::Arc;

use super::ninja_hvol as hvol;

/// Fog_Mist EffectsQuality@3 and its bound 512 max-axis volume resolution.
/// The pawn is inside the volume, so the lighting cache takes LOD 0.
const HVOL_CACHE_SIZE: [u32; 3] = [256, 120, 256];

// UE 5.6 volumetric fog at the demo's settings: Epic shadow scalability
// (BaseScalability.ini ShadowQuality@3) and the Fog_Mist ExponentialHeightFog
// component (queried from the level: VolumetricFogDistance 12000,
// VolumetricFogScatteringDistribution .1, NearFadeInDistance 0).
const VOLUME_GRID_PIXEL_SIZE: u32 = 5;
const VOLUME_GRID_SIZE_Z: u32 = 128;
const VOLUME_DEPTH_DISTRIBUTION_SCALE: f64 = 32.0;
const VOLUME_HISTORY_WEIGHT: f32 = 0.9;
const VOLUME_HISTORY_MISS_SUPERSAMPLE_COUNT: u32 = 4;
const VOLUME_FOG_DISTANCE_CM: f64 = 12_000.0;
const VOLUME_PHASE_G: f32 = 0.1;
// UE's default GNearClippingPlane (cm).
const VOLUME_NEAR_CLIP_CM: f64 = 10.0;
pub(in crate::renderer) const VOLUME_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(in crate::renderer) fn volume_composite_target(format: wgpu::TextureFormat) -> wgpu::ColorTargetState {
    wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::SrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        write_mask: wgpu::ColorWrites::ALL,
    }
}

#[derive(Default)]
pub(in crate::renderer) struct NinjaFogState {
    placement: Option<Placement>,
    ready: Option<Box<FogGpu>>,
    failed: bool,
    active_this_frame: bool,
}

struct FogGpu {
    field: FogField,
    volume: VolumePipelines,
    target: Option<VolumeTarget>,
    frame_number: u32,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    group: Option<wgpu::BindGroup>,
    textures: [wgpu::TextureView; 6],
    surface: Arc<FogSurface>,
    clamp: wgpu::Sampler,
    wrap: wgpu::Sampler,
    actors: ActorHistory,
    bone_velocities: HashMap<u16, FogBoneVelocities>,
    strokes: Vec<FogStroke>,
    game_ms: Option<i32>,
    time: f64,
}

struct FogBoneVelocities {
    time_ms: i32,
    current: [[f32; 3]; FOG_BONE_SLOTS.len()],
    previous: [[f32; 3]; FOG_BONE_SLOTS.len()],
}

/// UE's three volumetric fog stages for this volume.
struct VolumePipelines {
    scatter: wgpu::ComputePipeline,
    scatter_layout: wgpu::BindGroupLayout,
    integrate: wgpu::ComputePipeline,
    integrate_layout: wgpu::BindGroupLayout,
    apply: wgpu::RenderPipeline,
    apply_layout: wgpu::BindGroupLayout,
    depth_multisampled: bool,
    params: wgpu::Buffer,
    linear: wgpu::Sampler,
    hvol: Option<hvol::Pipeline>,
}

/// Resolution-dependent volume textures. Light scattering is ping-ponged:
/// one slot is written while the other is this frame's history.
struct VolumeTarget {
    grid: [u32; 3],
    viewport: [u32; 2],
    _scattering: [wgpu::TextureView; 2],
    integrated: wgpu::TextureView,
    scatter_groups: [wgpu::BindGroup; 2],
    integrate_groups: [wgpu::BindGroup; 2],
    write_slot: usize,
    history_valid: bool,
    encoded: bool,
    hvol: Option<hvol::Target>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct VolumeParams {
    pub(in crate::renderer) unjittered_inv_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) prev_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) jitter: [[f32; 4]; 4],
    pub(in crate::renderer) grid: [u32; 4],
    pub(in crate::renderer) grid_z: [f32; 4],
    pub(in crate::renderer) view: [f32; 4],
    pub(in crate::renderer) misc: [f32; 4],
    pub(in crate::renderer) frame: [u32; 4],
}

/// Math/Halton.h.
fn halton(mut index: u32, base: u32) -> f32 {
    let mut result = 0.0;
    let inv_base = 1.0 / base as f32;
    let mut fraction = inv_base;
    while index > 0 {
        result += (index % base) as f32 * fraction;
        index /= base;
        fraction *= inv_base;
    }
    result
}

/// VolumetricFog.cpp VolumetricFogTemporalRandom (jitter and reprojection on).
fn temporal_random(frame_number: u32) -> [f32; 4] {
    let index = frame_number & 1023;
    [halton(index, 2), halton(index, 3), halton(index, 5), 0.0]
}

/// RenderUtils.h CalculateGridZParams with VolumetricFog.cpp's near plane
/// (max(NearClippingDistance, VolumetricFogStartDistance)), in map units.
fn grid_z_params() -> [f32; 3] {
    let near_offset = 0.095 * 100.0;
    let s = VOLUME_DEPTH_DISTRIBUTION_SCALE;
    let n = (VOLUME_NEAR_CLIP_CM + near_offset) / f64::from(CM_PER_MAP_UNIT);
    let f = VOLUME_FOG_DISTANCE_CM / f64::from(CM_PER_MAP_UNIT);
    let o = (f - n * (f64::from(VOLUME_GRID_SIZE_Z) / s).exp2()) / (f - n);
    let b = (1.0 - o) / n;
    [b as f32, o as f32, s as f32]
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FogUniform {
    base_width: [f32; 4],
    height_cover_time: [f32; 4],
    sky_rgb: [f32; 4],
    preset: [u32; 4],
    ground_bounds: [f32; 4],
    sim_centre: [f32; 4],
}

impl NinjaFogState {
    pub(in crate::renderer) fn set_scene(&mut self, placement: Option<&Placement>) {
        if self.placement.as_ref().map(|p| p.revision)
            != placement.map(|p| p.revision)
        {
            *self = Self {
                placement: placement.cloned(),
                ..Self::default()
            };
        } else if let (Some(current), Some(incoming)) = (&mut self.placement, placement) {
            current.pawn = incoming.pawn;
            if !Arc::ptr_eq(&current.surface, &incoming.surface) {
                current.surface = Arc::clone(&incoming.surface);
            }
        }
    }

    pub(in crate::renderer) fn layout(&self) -> Option<&wgpu::BindGroupLayout> {
        self.ready.as_ref().map(|gpu| &gpu.layout)
    }

    pub(in crate::renderer) fn active(&self) -> bool {
        self.active_this_frame
    }

    fn fail(&mut self, error: String) {
        log::warn!("[NINJA FOG] volume disabled ({error})");
        self.ready = None;
        self.active_this_frame = false;
        self.failed = true;
    }
}

impl Renderer {
    pub(in crate::renderer) fn prepare_ninja_fog(&mut self, draw_world: bool) -> bool {
        let state = &mut self.ninja_fog;
        state.active_this_frame = false;
        let Some(placement) = state.placement.as_ref() else {
            return false;
        };
        if state.failed
            || (placement.preset == FogPreset::Mist && placement.pawn.is_none())
            || !draw_world
            || self
                .world
                .as_ref()
                .is_none_or(|world| world.froxel_bind_group.is_none())
        {
            return false;
        }
        if state
            .ready
            .as_ref()
            .is_some_and(|fog| {
                fog.volume.depth_multisampled != (self.msaa_samples > 1)
                    || fog.game_ms.zip(self.weather_server_time_ms)
                        .is_some_and(|(previous, now)| now < previous)
            })
        {
            state.ready = None;
        }
        if state.ready.is_none() {
            let placement = placement.clone();
            match FogGpu::new(self, &placement) {
                Ok(gpu) => self.ninja_fog.ready = Some(Box::new(gpu)),
                Err(error) => {
                    self.ninja_fog.fail(error);
                    return false;
                }
            }
        }
        self.ninja_fog.active_this_frame = true;
        true
    }

    pub(in crate::renderer) fn encode_ninja_fog(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let placement = self.ninja_fog.placement.as_ref().expect("prepared fog placement");
        let gpu = self.ninja_fog.ready.as_mut().expect("prepared fog GPU");
        // Use the authored linear skybox mean as the FVOL ambient emission
        // input when available. Ordinary in-scattering continues to be lit
        // by the froxel pass, including its direct and local shadow maps.
        let sky = self
            .world
            .as_ref()
            .filter(|world| world.primary_skybox.is_some())
            .map_or([0.0; 3], |world| world.sky_average);
        let result = gpu.encode(
            &self.device,
            &self.queue,
            encoder,
            placement,
            self.weather_server_time_ms,
            self.river_ninja_entities
                .as_deref()
                .map_or(&[], Vec::as_slice),
            sky,
        );
        if let Err(error) = result {
            self.ninja_fog.fail(error);
        }
    }
}

impl FogGpu {
    fn new(renderer: &Renderer, placement: &Placement) -> Result<Self, String> {
        let device = &renderer.device;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = Self::create(renderer, placement);
        let validation = jka_river::river_ninja::checked_scope(validation);
        let memory = jka_river::river_ninja::checked_scope(memory);
        validation.and(memory)?;
        result
    }

    fn create(renderer: &Renderer, placement: &Placement) -> Result<Self, String> {
        let device = &renderer.device;
        let queue = &renderer.queue;
        let images = jka_ninja::fog_input_images(placement.preset)
            .ok_or("Fluid Ninja is not included in this build")?;
        let mut assets =
            AssetSearchPath::open_game(&renderer.base, renderer.detail_texture_game.as_deref())
                .map_err(|error| error.to_string())?;
        let [painter, amplitude] = images.simulation;
        let painter = image_view(&mut assets, device, queue, painter)?;
        let amplitude = image_view(&mut assets, device, queue, amplitude)?;
        let height = image_view(&mut assets, device, queue, images.height_detail)?;
        let [noise1, noise2] = images.noise;
        let shared_noise = noise1.qpath == noise2.qpath;
        // Both volume textures are TEXTUREGROUP_World / TMGS_FromTextureGroup
        // in the source project, so UE samples a full SimpleAverage chain.
        let noise1 = world_group_image_view(&mut assets, device, queue, noise1)?;
        let noise2 = if shared_noise {
            noise1.clone()
        } else {
            world_group_image_view(&mut assets, device, queue, noise2)?
        };
        let (field, passive_density, passive_velocity, blue_noise) =
            if placement.preset == FogPreset::Mist {
                let mist = jka_ninja::fog_mist_images();
                let mut read = |image| image_view(&mut assets, device, queue, image);
                let density = read(mist.density_input)?;
                let velocity = read(mist.velocity_input)?;
                let density_noise = read(mist.density_noise)?;
                let blue_noise = read(mist.blue_noise)?;
                (
                    FogField::new(device, queue, placement.desc, [&painter, &amplitude],
                        Some([&density, &velocity, &density_noise]))?,
                    world_group_image_view(&mut assets, device, queue, mist.passive_density)?,
                    world_group_image_view(&mut assets, device, queue, mist.passive_velocity)?,
                    Some(blue_noise),
                )
            } else {
                (FogField::new(device, queue, placement.desc, [&painter, &amplitude], None)?,
                    height.clone(), height.clone(), None)
            };
        let ground = upload_ground(device, queue, &placement.surface);

        let texture_entry = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: binding != 7 },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Ninja fog material layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1, wgpu::TextureViewDimension::D2),
                texture_entry(2, wgpu::TextureViewDimension::D2),
                texture_entry(3, wgpu::TextureViewDimension::D3),
                texture_entry(4, wgpu::TextureViewDimension::D3),
                sampler_entry(5),
                sampler_entry(6),
                texture_entry(7, wgpu::TextureViewDimension::D2),
                texture_entry(8, wgpu::TextureViewDimension::D2),
                texture_entry(9, wgpu::TextureViewDimension::D2),
            ],
        });
        let volume = VolumePipelines::new(
            device,
            &renderer.weather.fog.resources.layout,
            &layout,
            renderer.scene_format(),
            blue_noise,
            renderer.msaa_samples > 1,
        );
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA Ninja fog volume uniform"),
            size: std::mem::size_of::<FogUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = |address| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("JKA Ninja fog material sampler"),
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        Ok(Self {
            field,
            volume,
            target: None,
            frame_number: 0,
            layout,
            uniform,
            group: None,
            textures: [height, noise1, noise2, ground, passive_density, passive_velocity],
            surface: Arc::clone(&placement.surface),
            clamp: sampler(wgpu::AddressMode::ClampToEdge),
            wrap: sampler(wgpu::AddressMode::Repeat),
            actors: ActorHistory::default(),
            bone_velocities: HashMap::new(),
            strokes: Vec::new(),
            game_ms: None,
            time: 0.0,
        })
    }

    fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        placement: &Placement,
        game_ms: Option<i32>,
        actors: &[ActorSample],
        sky: [f32; 3],
    ) -> Result<(), String> {
        let delta = self.game_ms.zip(game_ms).map_or(0.0, |(last, now)| {
            (i64::from(now) - i64::from(last)).max(0) as f64 / 1000.0
        });
        self.game_ms = game_ms;
        self.time += delta;
        if !Arc::ptr_eq(&placement.surface, &self.surface) {
            self.surface = Arc::clone(&placement.surface);
            self.textures[3] = upload_ground(device, queue, &self.surface);
            self.group = None;
            if let Some(target) = self.target.as_mut() {
                target.history_valid = false;
            }
        }
        self.strokes.clear();
        let width = placement.desc.width_cm / CM_PER_MAP_UNIT;
        let height = placement.desc.height_cm / CM_PER_MAP_UNIT;
        let base = crate::scene::jka_position(placement.base.to_array());
        let pawn = placement.pawn.unwrap_or(placement.base);
        let centre = crate::scene::jka_position(pawn.to_array());
        let mist = placement.preset == FogPreset::Mist;
        let half_width = width * 0.5;
        let local_cm = |position| {
            (Vec3::from_array(crate::scene::render_position(position)) - placement.base)
                .to_array()
                .map(|v| v * CM_PER_MAP_UNIT)
        };
        let velocity_cm =
            |velocity| crate::scene::render_position(velocity).map(|v| v * CM_PER_MAP_UNIT);
        for track in self.actors.update(actors.iter().copied()) {
            let current = track.current;
            let Some(previous) = track.previous else {
                self.bone_velocities.remove(&current.id);
                continue;
            };
            // Use continuous hull bounds to admit fast movers crossing the
            // volume between presentation samples.
            if (0..2).any(|i| {
                current.mins_map[i].min(previous.mins_map[i]) > centre[i] + half_width
                    || current.maxs_map[i].max(previous.maxs_map[i]) < centre[i] - half_width
            }) {
                continue;
            }
            let min_z = current.mins_map[2].min(previous.mins_map[2]);
            let max_z = current.maxs_map[2].max(previous.maxs_map[2]);
            if mist {
                if min_z > centre[2] + half_width || max_z < centre[2] - half_width {
                    continue;
                }
            } else if ![current.origin_map, previous.origin_map]
                .into_iter()
                .filter_map(|origin| placement.surface.ground_at(base, origin))
                .any(|ground| min_z <= ground + height && max_z >= ground)
            {
                continue;
            }
            let dt = (i64::from(current.time_ms) - i64::from(previous.time_ms)) as f32 * 0.001;
            if mist && current.kind == ActorKind::Player {
                if let Some((bones, old_bones)) = current.river_bones.zip(previous.river_bones) {
                    // PainterSourceData reads Previous.UnitVelocity for its glitch protector.
                    let previous_velocity = self.bone_velocities.get(&current.id).map_or_else(
                        || [[0.0; 3]; FOG_BONE_SLOTS.len()],
                        |saved| {
                            if saved.time_ms == previous.time_ms {
                                saved.current
                            } else if saved.time_ms == current.time_ms {
                                saved.previous
                            } else {
                                [[0.0; 3]; FOG_BONE_SLOTS.len()]
                            }
                        },
                    );
                    let mut velocities = [[0.0; 3]; FOG_BONE_SLOTS.len()];
                    for (slot, index) in FOG_BONE_SLOTS.into_iter().enumerate() {
                        let Some((bone, old)) = bones[index].zip(old_bones[index]) else { continue };
                        let velocity = std::array::from_fn(|i| (bone.position_map[i] - old.position_map[i]) / dt);
                        velocities[slot] = velocity_cm(velocity);
                        self.strokes.push(FogStroke {
                            position_cm: local_cm(bone.position_map),
                            previous_cm: local_cm(old.position_map),
                            velocity_cm_s: velocities[slot],
                            previous_velocity_cm_s: previous_velocity[slot],
                            // CalcPosAndScale2 feeds SkeletalMeshBrushSize
                            // straight into the tracked bone coefficient.
                            brush_scale: bone.parent_distance_map * CM_PER_MAP_UNIT * 0.05 * 2.0,
                        });
                    }
                    self.bone_velocities.insert(current.id, FogBoneVelocities {
                        time_ms: current.time_ms,
                        current: velocities,
                        previous: previous_velocity,
                    });
                }
                continue;
            }
            let derived =
                std::array::from_fn(|i| (current.origin_map[i] - previous.origin_map[i]) / dt);
            let velocity = current.velocity_map_s.unwrap_or(derived);
            if velocity == [0.0; 3] && current.origin_map == previous.origin_map {
                continue;
            }
            let brush_scale = if mist {
                let half_extent: [f32; 3] = std::array::from_fn(|axis| {
                    (current.maxs_map[axis] - current.mins_map[axis]) * (0.5 * CM_PER_MAP_UNIT)
                });
                // CalcPosAndScale1 selects the component's bounding sphere
                // for tracked primitive sizes. The BSP exposes its hull.
                Vec3::from_array(half_extent).length() * 0.7 * 0.1 * 2.0
            } else if current.kind == ActorKind::FogMissile {
                // Point projectile tracks use one solver cell of brush support.
                0.2 * placement.desc.width_cm / placement.desc.cells_per_side as f32
            } else {
                (current.maxs_map[0] - current.mins_map[0])
                    .max(current.maxs_map[1] - current.mins_map[1])
                    * (0.2 * CM_PER_MAP_UNIT)
            };
            if brush_scale <= 0.0 {
                continue;
            }
            self.strokes.push(FogStroke {
                position_cm: local_cm(current.origin_map),
                previous_cm: local_cm(previous.origin_map),
                velocity_cm_s: velocity_cm(velocity),
                previous_velocity_cm_s: velocity_cm(previous.velocity_map_s.unwrap_or(derived)),
                brush_scale,
            });
        }
        self.field.encode(
            queue,
            encoder,
            &FogFrame {
                game_delta: delta,
                origin_cm: ((pawn - placement.base) * CM_PER_MAP_UNIT).to_array(),
                anchor_cm: (placement.base * CM_PER_MAP_UNIT).to_array(),
                strokes: &self.strokes,
            },
        )?;
        let sim_centre = placement.base + Vec3::from_array(self.field.outputs().origin_cm) / CM_PER_MAP_UNIT;
        let uniform = FogUniform {
            base_width: placement.base.extend(width).to_array(),
            height_cover_time: [
                height,
                placement.surface.width,
                self.time as f32,
                0.0,
            ],
            sky_rgb: [sky[0], sky[1], sky[2], 0.0],
            preset: [placement.preset as u32, 0, 0, 0],
            ground_bounds: [
                placement.surface.vertical_bounds[0],
                placement.surface.vertical_bounds[1],
                placement.surface.centre_map.x,
                placement.surface.centre_map.y,
            ],
            sim_centre: sim_centre.extend(0.0).to_array(),
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        if self.group.is_none() {
            let Some(vd) = self.field.outputs().velocity_density else {
                return Ok(());
            };
            let texture = |binding, view| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            };
            let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA Ninja fog material bind group"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform.as_entire_binding(),
                    },
                    texture(1, vd),
                    texture(2, &self.textures[0]),
                    texture(3, &self.textures[1]),
                    texture(4, &self.textures[2]),
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.clamp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::Sampler(&self.wrap),
                    },
                    texture(7, &self.textures[3]),
                    texture(8, &self.textures[4]),
                    texture(9, &self.textures[5]),
                ],
            });
            jka_river::river_ninja::checked_scope(validation)?;
            self.group = Some(group);
        }
        Ok(())
    }
}

fn upload_ground(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &FogSurface,
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: SURFACE_SIZE,
        height: SURFACE_SIZE,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA Ninja fog ground planes"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(&surface.planes),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SURFACE_SIZE * 16),
            rows_per_image: Some(SURFACE_SIZE),
        },
        size,
    );
    texture.create_view(&Default::default())
}

impl VolumePipelines {
    fn new(
        device: &wgpu::Device,
        froxel_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        scene_format: wgpu::TextureFormat,
        blue_noise: Option<wgpu::TextureView>,
        depth_multisampled: bool,
    ) -> Self {
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<VolumeParams>() as u64),
            },
            count: None,
        };
        let texture = |binding, visibility, view_dimension, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: VOLUME_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        };
        let compute = wgpu::ShaderStages::COMPUTE;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let d3 = wgpu::TextureViewDimension::D3;
        let scatter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Ninja fog light scattering layout"),
            entries: &[uniform(0, compute), texture(1, compute, d3, true), sampler(2, compute), storage(3)],
        });
        let integrate_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Ninja fog final integration layout"),
            entries: &[uniform(0, compute), texture(1, compute, d3, true), storage(2)],
        });
        let apply_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Ninja fog apply layout"),
            entries: &[
                uniform(0, fragment),
                texture(1, fragment, d3, true),
                sampler(2, fragment),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: depth_multisampled,
                    },
                    count: None,
                },
            ],
        });
        let module = |label, source: String| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let scatter_module = module("JKA Ninja fog light scattering",
            volume_shader_source(include_str!("ninja_fog_volume.wgsl")));
        let integrate_module = module(
            "JKA Ninja fog final integration",
            format!("{VOLUME_COMMON_WGSL}\n{}", include_str!("ninja_fog_integrate.wgsl")),
        );
        let depth_source = if depth_multisampled {
            include_str!("ninja_fog_depth_msaa.wgsl")
        } else {
            include_str!("ninja_fog_depth.wgsl")
        };
        let apply_module = module(
            "JKA Ninja fog apply",
            format!("{VOLUME_COMMON_WGSL}\n{depth_source}\n{}", include_str!("ninja_fog_apply.wgsl")),
        );
        let pipeline_layout = |label, layouts: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                immediate_size: 0,
            })
        };
        let scatter = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("JKA Ninja fog light scattering"),
            layout: Some(&pipeline_layout(
                "JKA Ninja fog light scattering pipeline layout",
                &[Some(froxel_layout), Some(material_layout), Some(&scatter_layout)],
            )),
            module: &scatter_module,
            entry_point: Some("ninja_light_scattering"),
            compilation_options: Default::default(),
            cache: crate::pipeline_cache::get().as_ref(),
        });
        let integrate = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("JKA Ninja fog final integration"),
            layout: Some(&pipeline_layout(
                "JKA Ninja fog final integration pipeline layout",
                &[Some(&integrate_layout)],
            )),
            module: &integrate_module,
            entry_point: Some("ninja_final_integration"),
            compilation_options: Default::default(),
            cache: crate::pipeline_cache::get().as_ref(),
        });
        let apply = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("JKA Ninja fog apply"),
            layout: Some(&pipeline_layout("JKA Ninja fog apply pipeline layout", &[Some(&apply_layout)])),
            vertex: wgpu::VertexState {
                module: &apply_module,
                entry_point: Some("ninja_apply_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &apply_module,
                entry_point: Some("ninja_apply_fs"),
                compilation_options: Default::default(),
                targets: &[Some(volume_composite_target(scene_format))],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: crate::pipeline_cache::get().as_ref(),
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA Ninja fog volume parameters"),
            size: std::mem::size_of::<VolumeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // LightScatteringHistorySampler / SharedIntegratedLightScatteringSampler: bilinear clamp.
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA Ninja fog volume sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let hvol = blue_noise.map(|blue_noise| {
            let source = volume_shader_source(&[include_str!("ninja_fog_hvol.wgsl"), hvol::STAGE_WGSL].join("\n"));
            hvol::Pipeline::new(device, froxel_layout, material_layout, scene_format, blue_noise, source, HVOL_CACHE_SIZE)
        });
        Self { scatter, scatter_layout, integrate, integrate_layout, apply, apply_layout, depth_multisampled, params, linear, hvol }
    }

    fn create_target(&self, device: &wgpu::Device, viewport: [u32; 2]) -> VolumeTarget {
        let grid = [
            viewport[0].div_ceil(VOLUME_GRID_PIXEL_SIZE),
            viewport[1].div_ceil(VOLUME_GRID_PIXEL_SIZE),
            VOLUME_GRID_SIZE_Z,
        ];
        let volume = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: grid[0], height: grid[1], depth_or_array_layers: grid[2] },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: VOLUME_FORMAT,
                    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let scattering = [
            volume("JKA Ninja fog light scattering A"),
            volume("JKA Ninja fog light scattering B"),
        ];
        let integrated = volume("JKA Ninja fog integrated light scattering");
        let view = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let params = || wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() };
        let scatter_groups = std::array::from_fn(|write| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA Ninja fog light scattering bind group"),
                layout: &self.scatter_layout,
                entries: &[
                    params(),
                    view(1, &scattering[1 - write]),
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.linear) },
                    view(3, &scattering[write]),
                ],
            })
        });
        let integrate_groups = std::array::from_fn(|write| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA Ninja fog final integration bind group"),
                layout: &self.integrate_layout,
                entries: &[params(), view(1, &scattering[write]), view(2, &integrated)],
            })
        });
        VolumeTarget {
            grid,
            viewport,
            _scattering: scattering,
            integrated,
            scatter_groups,
            integrate_groups,
            write_slot: 0,
            history_valid: false,
            encoded: false,
            hvol: self.hvol.as_ref().map(|pipeline| pipeline.create_target(device, &self.params, viewport)),
        }
    }
}

impl Renderer {
    /// LightScatteringCS and FinalIntegrationCS. Runs after the froxel build,
    /// whose uniform, light clusters and shadow maps it shares.
    pub(in crate::renderer) fn encode_ninja_fog_volume(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        unjittered_view_proj: Mat4,
        previous_unjittered_view_proj: Mat4,
        camera_history_valid: bool,
        camera_position: Vec3,
    ) {
        let viewport = [
            self.targets._scene_texture.width().max(1),
            self.targets._scene_texture.height().max(1),
        ];
        let Some(froxel_group) = self.world.as_ref().and_then(|world| world.froxel_bind_group.as_ref()) else {
            return;
        };
        let Some(gpu) = self.ninja_fog.ready.as_mut() else { return };
        if gpu.group.is_none() {
            return;
        }
        if gpu.target.as_ref().is_none_or(|target| target.viewport != viewport) {
            gpu.target = Some(gpu.volume.create_target(&self.device, viewport));
        }
        let frame = gpu.frame_number;
        gpu.frame_number = gpu.frame_number.wrapping_add(1);
        let target = gpu.target.as_mut().expect("volume target");
        target.encoded = false;
        let history_valid = target.history_valid && camera_history_valid;
        let mut jitter = [[0.0; 4]; 4];
        for (offset, value) in jitter.iter_mut().enumerate() {
            *value = temporal_random(frame.wrapping_sub(offset as u32));
        }
        let [b, o, s] = grid_z_params();
        let params = VolumeParams {
            unjittered_inv_view_proj: unjittered_view_proj.inverse().to_cols_array_2d(),
            prev_view_proj: previous_unjittered_view_proj.to_cols_array_2d(),
            view_proj: unjittered_view_proj.to_cols_array_2d(),
            jitter,
            grid: [target.grid[0], target.grid[1], target.grid[2], VOLUME_HISTORY_MISS_SUPERSAMPLE_COUNT],
            grid_z: [b, o, s, if history_valid { VOLUME_HISTORY_WEIGHT } else { 0.0 }],
            view: [
                camera_position.x,
                camera_position.y,
                camera_position.z,
                (VOLUME_FOG_DISTANCE_CM / f64::from(CM_PER_MAP_UNIT)) as f32,
            ],
            // VolumetricFogNearFadeInDistance 0 => UE's 1e8 inverse.
            misc: [viewport[0] as f32, viewport[1] as f32, 100_000_000.0, VOLUME_PHASE_G],
            frame: [frame, hvol::DOWNSAMPLE, 0, 0],
        };
        self.queue.write_buffer(&gpu.volume.params, 0, bytemuck::bytes_of(&params));
        let write = target.write_slot;
        let material_group = gpu.group.as_ref().expect("material group");
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("JKA Ninja fog light scattering"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&gpu.volume.scatter);
            pass.set_bind_group(0, froxel_group, &[]);
            pass.set_bind_group(1, material_group, &[]);
            pass.set_bind_group(2, &target.scatter_groups[write], &[]);
            pass.dispatch_workgroups(target.grid[0].div_ceil(4), target.grid[1].div_ceil(4), target.grid[2].div_ceil(4));
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("JKA Ninja fog final integration"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&gpu.volume.integrate);
            pass.set_bind_group(0, &target.integrate_groups[write], &[]);
            pass.dispatch_workgroups(target.grid[0].div_ceil(8), target.grid[1].div_ceil(8), 1);
        }
        target.write_slot = 1 - write;
        target.history_valid = true;
        target.encoded = true;
    }

}

impl NinjaFogState {
    /// CombineVolumetricFog onto the final HDR scene, before post. Borrows
    /// only the fog state and targets, so it can run inside the frame's
    /// other outstanding field borrows.
    pub(in crate::renderer) fn encode_apply(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene_view: &wgpu::TextureView,
        scene_depth_view: &wgpu::TextureView,
        linear_depth_view: &wgpu::TextureView,
        froxel_group: Option<&wgpu::BindGroup>,
    ) {
        if !self.active_this_frame {
            return;
        }
        let Some(gpu) = self.ready.as_ref() else { return };
        let Some(target) = gpu.target.as_ref().filter(|target| target.encoded) else { return };
        // The scene depth target can be rebuilt independently of the
        // viewport, so this small group is created per frame.
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA Ninja fog apply bind group"),
            layout: &gpu.volume.apply_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: gpu.volume.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&target.integrated) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&gpu.volume.linear) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(scene_depth_view) },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("JKA Ninja fog apply"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: scene_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&gpu.volume.apply);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        if let (Some(pipeline), Some(target), Some(lights), Some(material)) = (
            &gpu.volume.hvol, &target.hvol, froxel_group, &gpu.group,
        ) {
            pipeline.encode(device, encoder, &gpu.volume.params, target, lights, material,
                linear_depth_view, scene_view);
        }
    }
}

const VOLUME_COMMON_WGSL: &str = include_str!("ninja_fog_volume_common.wgsl");

/// LightScatteringCS module: host froxel lighting library, the Ninja material
/// bridge and private material, then the UE-stage entry point.
fn volume_shader_source(stage: &str) -> String {
    [
        include_str!("../froxel_fog.wgsl"),
        include_str!("ninja_fog_froxel.wgsl"),
        jka_ninja::fog_volume_wgsl(),
        jka_ninja::fog_mist_wgsl(),
        VOLUME_COMMON_WGSL,
        stage,
    ].join("\n")
}

pub(in crate::renderer) fn image_view(
    assets: &mut AssetSearchPath,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    spec: FogImage,
) -> Result<wgpu::TextureView, String> {
    load_image_view(assets, device, queue, spec, false)
}

/// The TEXTUREGROUP_World mip chain for the authored fog textures.
fn world_group_image_view(
    assets: &mut AssetSearchPath,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    spec: FogImage,
) -> Result<wgpu::TextureView, String> {
    load_image_view(assets, device, queue, spec, true)
}

fn load_image_view(
    assets: &mut AssetSearchPath,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    spec: FogImage,
    with_mips: bool,
) -> Result<wgpu::TextureView, String> {
    let asset = assets
        .read(spec.qpath, 16 * 1024 * 1024)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("required local texture missing: {}", spec.qpath))?;
    let [width, height, depth] = spec.size;
    if depth == 0 {
        let data = crate::materials::decode_texture_data_with_color_space(
            spec.qpath,
            &asset.bytes,
            false,
            with_mips,
            spec.srgb,
        )?;
        if [data.width, data.height] != [width, height] {
            return Err(format!("{} requires {width}x{height}", spec.qpath));
        }
        return Ok(super::textures::upload_texture(device, queue, &data)
            .view
            .clone());
    }
    // Source export from UE's DDS exporter. This asset contract uses one
    // uncompressed BGRA8 volume mip with the DX10 header.
    let bytes = &asset.bytes;
    let word = |offset| {
        bytes
            .get(offset..offset + 4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
    };
    const DDS_HEADER_BYTES: usize = 148;
    let expected_format = if spec.srgb { 91 } else { 87 };
    let valid_header = bytes.starts_with(b"DDS ")
        && word(4) == Some(124)
        && word(12) == Some(height)
        && word(16) == Some(width)
        && word(24) == Some(depth)
        && word(28) == Some(1)
        && bytes.get(84..88) == Some(&b"DX10"[..])
        && word(128) == Some(expected_format)
        && word(132) == Some(4)
        && word(140) == Some(1);
    let expected_bytes =
        DDS_HEADER_BYTES as u64 + u64::from(width) * u64::from(height) * u64::from(depth) * 4;
    if !valid_header || bytes.len() as u64 != expected_bytes {
        return Err(format!(
            "{} requires a {width}x{height}x{depth} BGRA8 DDS volume",
            spec.qpath
        ));
    }
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: depth,
    };
    let mip_level_count = if with_mips {
        32 - width.max(height).max(depth).leading_zeros()
    } else {
        1
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(spec.qpath),
        size,
        mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: if spec.srgb {
            wgpu::TextureFormat::Bgra8UnormSrgb
        } else {
            wgpu::TextureFormat::Bgra8Unorm
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &bytes[DDS_HEADER_BYTES..],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    if mip_level_count > 1 {
        upload_simple_average_mips(
            queue,
            &texture,
            &bytes[DDS_HEADER_BYTES..],
            [width, height, depth],
            spec.srgb,
            mip_level_count,
        );
    }
    Ok(texture.create_view(&Default::default()))
}

/// UE TextureCompressor mip generation for TMGS_SimpleAverage: each level is
/// the 2x2x2 box average of the previous one (axes already at one texel are
/// not reduced). UE linearizes sRGB sources before filtering and re-encodes
/// each level for the sRGB format.
fn upload_simple_average_mips(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    base: &[u8],
    [mut w, mut h, mut d]: [u32; 3],
    srgb: bool,
    levels: u32,
) {
    let to_linear = |b: u8| {
        let v = f32::from(b) / 255.0;
        if !srgb {
            v
        } else if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let decode: [f32; 256] = std::array::from_fn(|b| to_linear(b as u8));
    let encode = |channel: usize, v: f32| -> u8 {
        let v = v.clamp(0.0, 1.0);
        // BGRA lanes 0..2 are colour; lane 3 is linear alpha.
        let v = if srgb && channel != 3 {
            if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        } else {
            v
        };
        (v * 255.0 + 0.5) as u8
    };
    let mut current: Vec<[f32; 4]> = base
        .chunks_exact(4)
        .map(|p| {
            std::array::from_fn(|c| if c == 3 { f32::from(p[3]) / 255.0 } else { decode[usize::from(p[c])] })
        })
        .collect();
    let mut bytes = Vec::new();
    for level in 1..levels {
        let [nw, nh, nd] = [w, h, d].map(|v| (v / 2).max(1));
        let [sx, sy, sz] = [w, h, d].map(|v| if v > 1 { 2 } else { 1 });
        let count = (sx * sy * sz) as f32;
        let mut next = vec![[0.0f32; 4]; (nw * nh * nd) as usize];
        for z in 0..nd {
            for y in 0..nh {
                for x in 0..nw {
                    let mut sum = [0.0f32; 4];
                    for dz in 0..sz {
                        for dy in 0..sy {
                            for dx in 0..sx {
                                let i = (((z * sz + dz) * h + (y * sy + dy)) * w + (x * sx + dx)) as usize;
                                for (total, value) in sum.iter_mut().zip(current[i]) {
                                    *total += value;
                                }
                            }
                        }
                    }
                    next[((z * nh + y) * nw + x) as usize] = sum.map(|total| total / count);
                }
            }
        }
        bytes.clear();
        bytes.extend(next.iter().flat_map(|p| std::array::from_fn::<u8, 4, _>(|c| encode(c, p[c]))));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(nw * 4),
                rows_per_image: Some(nh),
            },
            wgpu::Extent3d {
                width: nw,
                height: nh,
                depth_or_array_layers: nd,
            },
        );
        current = next;
        [w, h, d] = [nw, nh, nd];
    }
}

