//! Spindrift host adapter. The wisp placement, lighting and shaping are the upstream `spindrift.wgsl`, unchanged but for
//! one added line that keeps wisps inside JKA's admitted water footprint (`jka_water_contains`, as the surface does).
//! The CPU constants and helpers below are copied from `vendor/.../ocean/src/spindrift.rs` (that file's `PrepareContext`
//! / `ScenePipeline` plumbing belongs to the renderer core, which the published reference omits); its two unit tests
//! are reproduced to keep them equal.
//! Copyright (c) 2026 Bill Yang. MIT; see vendor/blue-water-sea-and-sky/LICENSE.
use bytemuck::{Pod, Zeroable};

use crate::shaders;

/// Cells per level edge (`GRID` in `spindrift.wgsl`).
pub const GRID: u32 = 48;
/// Wisps per cell (`SLOTS`).
pub const SLOTS: u32 = 3;
/// Levels of cells, each twice the last.
pub const LEVELS: usize = 4;
/// The finest level's cell (m) (`FINEST_CELL`).
pub const FINEST_CELL: f64 = 2.5;
const BAND_CELLS: f64 = 22.0;
/// Spindrift begins to be seen at Beaufort 7 (13.9–17.1 m/s) and is everywhere by a gale.
pub const SPINDRIFT_ONSET: f64 = 13.0;
const SPINDRIFT_ESTABLISHED: f64 = 17.5;
const SPINDRIFT_REFERENCE: f64 = 20.0;
const HAZE_EXTINCTION: f64 = 1.8e-4;
const HAZE_REFERENCE: f64 = 20.0;
const HAZE_DOUBLING: f64 = 3.0;
const HAZE_ONSET: f64 = 17.0;
const HAZE_MOST: f64 = 0.01;
const HAZE_SCALE: f64 = 3.0;
const HAZE_SCALE_SEA: f64 = 0.4;

fn smoothstep(from: f64, to: f64, x: f64) -> f64 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The spindrift's strength at `wind` m/s: 0 below its onset, 1 at 20 m/s, growing with the square of the wind.
pub fn spindrift_strength(wind: f64) -> f64 {
    if !wind.is_finite() {
        return 0.0;
    }
    smoothstep(SPINDRIFT_ONSET, SPINDRIFT_ESTABLISHED, wind) * (wind / SPINDRIFT_REFERENCE).powi(2)
}

/// The spray haze at `wind` m/s over a sea of `significant_height` m (`spray_haze.wgsl`'s inputs). None below 17 m/s.
pub fn spray_haze(wind: f64, significant_height: f64) -> [f32; 4] {
    if !wind.is_finite() || wind <= HAZE_ONSET {
        return [0.0; 4];
    }
    let extinction = (HAZE_EXTINCTION
        * 2f64.powf((wind - HAZE_REFERENCE) / HAZE_DOUBLING)
        * smoothstep(HAZE_ONSET, HAZE_REFERENCE, wind))
    .min(HAZE_MOST);
    let scale = HAZE_SCALE + HAZE_SCALE_SEA * significant_height.max(0.0);
    [extinction as f32, scale as f32, 0.0, 0.0]
}

/// `Spray` in `spindrift.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct SprayUniforms {
    pub wind: [f32; 4],
    pub spray: [f32; 4],
    pub haze: [f32; 4],
    pub levels: [[f32; 4]; LEVELS],
    pub fog_color: [f32; 4],
    pub fog: [f32; 4],
}

/// Each level's first cell, cell and band about a camera at `eye` (world x, z).
pub fn level_frames(eye: [f64; 2]) -> [[f32; 4]; LEVELS] {
    std::array::from_fn(|level| {
        let cell = FINEST_CELL * f64::from(1u32 << level);
        let half = f64::from(GRID / 2);
        let corner = eye.map(|v| ((v / cell).floor() - half) * cell);
        [corner[0] as f32, corner[1] as f32, cell as f32, (BAND_CELLS * cell) as f32]
    })
}

/// What one frame of spindrift needs from the ocean.
pub struct SpindriftFrame {
    /// Wind speed (m/s) and the direction it blows toward (radians, as `WaveParameters::wind_direction`).
    pub wind_speed: f64,
    pub wind_direction: f64,
    /// The sea's clock (s).
    pub time: f64,
    pub significant_height: f64,
    pub flash: f32,
    /// Camera x, z in metres.
    pub eye: [f64; 2],
    /// Whether the aerial perspective (`naval::aerial`) is live on the camera group.
    pub aerial: bool,
}

impl SpindriftFrame {
    pub fn strength(&self) -> f32 {
        spindrift_strength(self.wind_speed) as f32
    }

    pub fn uniforms(&self) -> SprayUniforms {
        let (toward_z, toward_x) = self.wind_direction.sin_cos();
        SprayUniforms {
            wind: [toward_x as f32, toward_z as f32, self.wind_speed as f32, self.time as f32],
            spray: [self.strength(), self.flash, self.significant_height as f32, 0.0],
            haze: spray_haze(self.wind_speed, self.significant_height),
            levels: level_frames(self.eye),
            fog_color: [0.0; 4],
            fog: [0.0, 0.0, 0.0, if self.aerial { 1.0 } else { 0.0 }],
        }
    }
}

/// One promoted pool's inputs to the draw.
pub struct PoolDraw<'a> {
    /// Override the shared open-water field with this pool's coastal wave bindings.
    pub waves: Option<&'a wgpu::BindGroup>,
    /// Group 0: the pool's translated camera (and aerial volume).
    pub camera: &'a wgpu::BindGroup,
    /// The pool's `JkaWaterBoundary` uniform (`@group(1) @binding(16)`).
    pub boundary: &'a wgpu::Buffer,
    pub cloud: (&'a wgpu::TextureView, &'a wgpu::Sampler, &'a wgpu::Buffer),
    /// Group 3: the sky environment the surface lights from.
    pub environment: &'a wgpu::BindGroup,
}

pub struct Spindrift {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
}

impl Spindrift {
    /// `camera`, `waves` and `environment` are the surface pipeline's own group layouts (0, 2 and 3).
    pub fn new(
        device: &wgpu::Device,
        camera: &wgpu::BindGroupLayout,
        waves: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let visibility = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding, visibility, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
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
        let filterable = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean spindrift"),
            entries: &[
                uniform(0),
                texture(1, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Depth),
                texture(2, visibility, filterable),
                sampler(3, visibility),
                uniform(4),
                texture(5, wgpu::ShaderStages::FRAGMENT, filterable),
                sampler(6, wgpu::ShaderStages::FRAGMENT),
                uniform(16),
            ],
        });
        let mut composer = blue_water_gpu::shader_composer::composer()
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&naga_oil::compose::Composer::default())));
        let anchor = "    let source = smoothstep(SOURCE_FROM, SOURCE_FULL, breaking_at(birth, cell * 0.35));";
        assert_eq!(
            shaders::SPINDRIFT.source.replace("\r\n", "\n").matches(anchor).count(),
            1,
            "upstream spindrift contract changed"
        );
        let source = shaders::SPINDRIFT.source.replace("\r\n", "\n").replace(
            anchor,
            &format!("    if !jka_water_contains(birth) {{ return culled(); }}\n{anchor}"),
        );
        let source: &'static str = Box::leak(
            format!("{source}\n{}", blue_water_gpu::shader_composer::SURFACE_BOUNDARY_WGSL).into_boxed_str(),
        );
        let module = shaders::naga_module(
            &mut composer,
            shaders::Entry { path: shaders::SPINDRIFT.path, source },
            &[("CASCADES", 3)],
        )
        .unwrap_or_else(|e| panic!("composing spindrift: {}", e.emit_to_string(&composer)));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ocean spindrift"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ocean spindrift"),
            bind_group_layouts: &[Some(camera), Some(&layout), Some(waves), Some(environment)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ocean spindrift"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("spindrift_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("spindrift_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean spindrift"),
            size: std::mem::size_of::<SprayUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { layout, pipeline, uniforms }
    }

    /// Draw every pool's wisps into the open pass. `depth` is a single-sample copy of this frame's depth including the
    /// sea (the soft-particle backdrop); `waves` is the field's group 2. Returns the pools drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        frame: &SpindriftFrame,
        depth: &wgpu::TextureView,
        foam: (&wgpu::TextureView, &wgpu::Sampler),
        waves: &wgpu::BindGroup,
        pools: &[PoolDraw<'_>],
    ) -> usize {
        if frame.strength() <= 0.0 || pools.is_empty() {
            return 0;
        }
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&frame.uniforms()));
        pass.set_pipeline(&self.pipeline);
        for pool in pools {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ocean spindrift pool"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(pool.cloud.0) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(pool.cloud.1) },
                    wgpu::BindGroupEntry { binding: 4, resource: pool.cloud.2.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(foam.0) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(foam.1) },
                    wgpu::BindGroupEntry { binding: 16, resource: pool.boundary.as_entire_binding() },
                ],
            });
            pass.set_bind_group(0, pool.camera, &[]);
            pass.set_bind_group(1, &group, &[]);
            pass.set_bind_group(2, pool.waves.unwrap_or(waves), &[]);
            pass.set_bind_group(3, pool.environment, &[]);
            pass.draw(0..4, 0..LEVELS as u32 * GRID * GRID * SLOTS);
        }
        pools.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spray_starts_with_a_gale_and_grows_with_the_wind() {
        assert_eq!(spindrift_strength(9.0), 0.0);
        assert_eq!(spindrift_strength(13.0), 0.0);
        assert!(spindrift_strength(15.0) > 0.0 && spindrift_strength(15.0) < 0.5);
        assert!((spindrift_strength(20.0) - 1.0).abs() < 1e-12);
        assert!(spindrift_strength(28.0) > 1.9);
        assert_eq!(spray_haze(16.0, 5.0), [0.0; 4]);
        let visibility = |wind: f64| 3.912 / f64::from(spray_haze(wind, 7.0)[0]);
        assert!((10_000.0..16_000.0).contains(&visibility(22.0)));
        assert!((3_000.0..5_000.0).contains(&visibility(28.0)));
        assert!(visibility(32.0) < 1_500.0);
    }

    #[test]
    fn levels_cover_their_bands_about_the_camera() {
        let eye = [1234.5, -987.6];
        for (level, frame) in level_frames(eye).iter().enumerate() {
            let cell = f64::from(frame[2]);
            assert_eq!(cell, FINEST_CELL * f64::from(1u32 << level));
            let reach = f64::from(GRID) * cell;
            for (axis, &start) in [frame[0], frame[1]].iter().enumerate() {
                let start = f64::from(start);
                assert!(eye[axis] - start >= f64::from(frame[3]));
                assert!(start + reach - eye[axis] >= f64::from(frame[3]));
            }
        }
    }
}
