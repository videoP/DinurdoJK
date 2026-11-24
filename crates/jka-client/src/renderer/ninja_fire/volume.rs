//! Heterogeneous-volume presentation of a Fluid Ninja flame field
//! (FLAME_VOLUME_CONTRACT) through the shared `ninja_hvol` live shading used
//! by the fog HVOL: lighting cache from the map's sun shadows and clustered
//! lights, half-resolution ray march and the UE composite.
use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};

use crate::fx::fire_geometry::SmokeMotion;
use crate::ninja_fire_debug::{CM_PER_MAP_UNIT, FlameFit};
use crate::renderer::ninja_fog::VolumeParams;
use crate::renderer::ninja_hvol as hvol;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SmokeUniform {
    centre_time: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
    axis_z: [f32; 4],
    resolution: [f32; 4],
    sky_rgb: [f32; 4],
    baked_ambient: [f32; 4],
    plume_flow: [f32; 4],
}

/// Per-frame placement of the camera-facing volume box.
pub(super) struct SmokePlacement {
    pub centre: Vec3,
    pub size: Option<Vec3>,
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
    pub time: f32,
    pub sky: [f32; 3],
    pub baked_ambient: Vec3,
    pub plume_flow: [f32; 4],
}

/// Maps an authored EFX plume into the player smoke solver's native field.
pub(super) struct SmokePlume {
    pub centre_cm: Vec3,
    pub size: Vec3,
    pub source_cm: Vec3,
    pub wind_cm_s: [f32; 3],
    pub body_scale: f32,
    pub lifetime_scale: f32,
    pub world_to_clip_cm: [[f32; 4]; 4],
    pub flow: [f32; 4],
}

impl SmokePlume {
    pub(super) fn new(
        emitter: super::Emitter,
        fit: FlameFit,
        motion: SmokeMotion,
        native: [f32; 3],
        right: Vec3,
        world_to_clip_cm: [[f32; 4]; 4],
        time: f32,
        lifetime_scale: f32,
    ) -> Self {
        let width = (fit.max.x - fit.min.x).max(fit.max.z - fit.min.z);
        // Leave room above the authored trajectory for the expanding head
        // of the plume to roll over and fade.
        let top = fit.max + Vec3::Y * (motion.radius * 0.5);
        let size = Vec3::new(width, top.y - fit.min.y, width);
        let centre = (fit.min + top) * 0.5;
        let centre_cm = (emitter.origin + centre) * CM_PER_MAP_UNIT;

        let offset = Vec3::from_array(motion.origin) - centre;
        let puff_period = motion.lifetime.clamp(0.45, 2.0);
        let start_phase = (emitter.id % 997) as f32 / 997.0;
        let phase = (time / puff_period + start_phase) * std::f32::consts::TAU;
        let radius_uv = motion.radius / width;
        // Keep the root over the authored launch point. A small, uneven
        // flicker feeds separate puffs into the rising field.
        let source_x = (offset.dot(right) / width).clamp(-0.28, 0.28)
            + 0.12 * radius_uv * (phase * 1.7).sin();
        let source_y = (offset.y / size.y).clamp(-0.34, 0.15);
        let source_cm = centre_cm + right * (source_x * native[0])
            + Vec3::Y * (source_y * native[1]);
        // The player preset takes a tracker size. Match its Gaussian painter
        // to a narrow core, leaving the wake to fill out the visible cloud.
        let native_half_radius_cm = 0.8 / (0.002 * 2000.0_f32.sqrt());
        let body_scale = (0.44 + 0.10 * phase.sin()) * radius_uv
            * (native[0] / native_half_radius_cm);

        let rise = motion.rise_speed.max(motion.radius / motion.lifetime);
        let rise_uv = (rise / size.y).min(2.0);
        // Feed buoyancy into the solver's advection. A cross-flow varies as
        // the puffs rise, while the preset's velocity field rolls their edges.
        let cross_uv = 0.30 * rise_uv * ((phase * 0.83).sin() + 0.35 * (phase * 1.53).sin());
        let wind_cm_s = (Vec3::Y * (rise_uv * native[1])
            + right * (cross_uv * native[0])).to_array();
        // A pair of alternating rolls travels through the upper plume with
        // the same lift as the solver. The material uses the source UV, the
        // distance between rolls and their phase in cycles.
        let roll_period = puff_period * 0.58;
        let flow = [0.5 + source_x, 0.5 - source_y,
            rise_uv * roll_period, time / roll_period + start_phase];

        // Clip the remapped tracker in the real volume, rather than at its
        // larger, virtual coordinates in the native simulation plane.
        let basis = Mat3::from_cols(right, Vec3::Y, Vec3::Y.cross(right));
        let fit_scale = Vec3::new(width * CM_PER_MAP_UNIT / native[0],
            size.y * CM_PER_MAP_UNIT / native[1], 1.0);
        let native_to_volume = Mat4::from_translation(centre_cm)
            * Mat4::from_mat3(basis * Mat3::from_diagonal(fit_scale) * basis.transpose())
            * Mat4::from_translation(-centre_cm);
        let world_to_clip_cm =
            (Mat4::from_cols_array_2d(&world_to_clip_cm) * native_to_volume).to_cols_array_2d();
        // Smoking Man density reaches about 5% after 0.6 s at 60 Hz.
        // Make the authored lifetime the baseline for the in-game slider.
        let lifetime_scale = (motion.lifetime / 0.6 * lifetime_scale).clamp(0.25, 8.0);
        Self { centre_cm, size, source_cm, wind_cm_s, body_scale, lifetime_scale, world_to_clip_cm, flow }
    }
}

pub(super) struct SmokeView {
    pub unjittered_view_proj: Mat4,
    pub camera: Vec3,
    pub viewport: [u32; 2],
}

pub(super) struct SmokeVolume {
    hv: hvol::Pipeline,
    layout: wgpu::BindGroupLayout,
    noise: wgpu::TextureView,
    /// Box size in map units (X, Y, Z) and its voxel resolution.
    size: Vec3,
    resolution: [u32; 3],
}

pub(super) struct SmokeDraw {
    params: wgpu::Buffer,
    uniform: wgpu::Buffer,
    /// Material group, keyed by the field's velocity/density view.
    group: Option<(wgpu::TextureView, wgpu::BindGroup)>,
    target: Option<([u32; 2], hvol::Target)>,
    frame: u32,
    pending: bool,
}

impl SmokeVolume {
    pub(super) fn new(
        device: &wgpu::Device,
        lights: &wgpu::BindGroupLayout,
        scene_format: wgpu::TextureFormat,
        material_wgsl: &str,
        noise: wgpu::TextureView,
        blue_noise: wgpu::TextureView,
        desc: &jka_ninja::FlameDesc,
        volume: jka_ninja::FlameVolumeDesc,
    ) -> Self {
        let [width, height, _] = desc.extent_cm;
        let size_cm = [width, height, volume.depth_cm];
        // NiagaraRendererVolumes: per-axis resolution = ceil(size / max * MaxAxis).
        let max_cm = size_cm.iter().copied().fold(0.0f32, f32::max).max(1e-3);
        let resolution = size_cm.map(|axis| ((axis / max_cm) * volume.resolution_max_axis as f32).ceil().max(1.0) as u32);
        // Lighting cache at LightingDownsampleFactor (LOD 0), as the fog instance.
        let cache_size = resolution.map(|axis| axis.div_ceil(hvol::DOWNSAMPLE).max(1));
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let texture = |view_dimension| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension,
            multisampled: false,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja flame volume material"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<SmokeUniform>() as u64),
                }),
                entry(1, texture(wgpu::TextureViewDimension::D2)),
                entry(2, texture(wgpu::TextureViewDimension::D3)),
                entry(3, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(4, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let source = [
            include_str!("../../froxel_fog.wgsl"),
            hvol::COMMON_WGSL,
            include_str!("../ninja_fire_hvol.wgsl"),
            material_wgsl,
            hvol::STAGE_WGSL,
        ].join("\n");
        let hv = hvol::Pipeline::new(device, lights, &layout, scene_format, blue_noise, source, cache_size);
        Self {
            hv,
            layout,
            noise,
            size: Vec3::from_array(size_cm) / CM_PER_MAP_UNIT,
            resolution,
        }
    }
}

impl SmokeDraw {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let buffer = |label, size| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            params: buffer("Ninja flame volume view", std::mem::size_of::<VolumeParams>() as u64),
            uniform: buffer("Ninja flame volume placement", std::mem::size_of::<SmokeUniform>() as u64),
            group: None,
            target: None,
            frame: 0,
            pending: false,
        }
    }

    /// Writes this frame's placement and view. The march runs in `encode`.
    pub(super) fn prepare(
        &mut self,
        volume: &SmokeVolume,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        field: &wgpu::TextureView,
        clamp: &wgpu::Sampler,
        wrap: &wgpu::Sampler,
        placement: &SmokePlacement,
        view: &SmokeView,
    ) {
        if self.group.as_ref().is_none_or(|(old, _)| old != field) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Ninja flame volume material"),
                layout: &volume.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(field) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&volume.noise) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(clamp) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(wrap) },
                ],
            });
            self.group = Some((field.clone(), group));
        }
        let half = placement.size.unwrap_or(volume.size) * 0.5;
        let axis = |direction: Vec3, half: f32| direction.extend(half).to_array();
        let [rx, ry, rz] = volume.resolution.map(|axis| axis as f32);
        // Smoking Man's peak density contributes 0.25 extinction per cm.
        // The squared ellipsoid integrates to 8/15 along its centre ray.
        // An optical depth of 2.6 before the taper allows about 75% opacity
        // at peak density, independent of the authored box depth.
        // Zero selects native smoke.
        let smoke_extinction_scale = placement.size.map_or(0.0, |size| {
            2.6 / (0.25 * size.z * CM_PER_MAP_UNIT)
        });
        let uniform = SmokeUniform {
            centre_time: placement.centre.extend(placement.time).to_array(),
            axis_x: axis(placement.right, half.x),
            axis_y: axis(-placement.up, half.y),
            axis_z: axis(-placement.forward, half.z),
            resolution: [rx, ry, rz, smoke_extinction_scale],
            sky_rgb: [placement.sky[0], placement.sky[1], placement.sky[2], 0.0],
            baked_ambient: placement.baked_ambient.extend(0.0).to_array(),
            plume_flow: placement.plume_flow,
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        let params = VolumeParams {
            unjittered_inv_view_proj: view.unjittered_view_proj.inverse().to_cols_array_2d(),
            prev_view_proj: view.unjittered_view_proj.to_cols_array_2d(),
            view_proj: view.unjittered_view_proj.to_cols_array_2d(),
            jitter: [[0.0; 4]; 4],
            grid: [0; 4],
            grid_z: [0.0; 4],
            view: view.camera.extend(0.0).to_array(),
            misc: [view.viewport[0] as f32, view.viewport[1] as f32, 0.0, 0.0],
            frame: [self.frame, hvol::DOWNSAMPLE, 0, 0],
        };
        self.frame = self.frame.wrapping_add(1);
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        if self.target.as_ref().is_none_or(|(size, _)| *size != view.viewport) {
            self.target = Some((view.viewport, volume.hv.create_target(device, &self.params, view.viewport)));
        }
        self.pending = true;
    }

    /// Cache, march and composite onto the HDR scene (before post).
    pub(super) fn encode(
        &mut self,
        volume: &SmokeVolume,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        lights: &wgpu::BindGroup,
    ) -> bool {
        if !std::mem::take(&mut self.pending) {
            return false;
        }
        let (Some((_, group)), Some((_, target))) = (&self.group, &self.target) else { return false; };
        volume.hv.encode(device, encoder, &self.params, target, lights, group, depth, scene);
        true
    }
}
