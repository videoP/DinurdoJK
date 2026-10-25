//! Pipeline state for the main-menu compositor. Built once per output format
//! on a pipeline-job worker; nothing here depends on the window size.
use super::{CRT_FORMAT, HDR};
use crate::renderer::bindings::{sampler_entry, texture_entry};

pub(super) struct Pipelines {
    pub signs_layout: wgpu::BindGroupLayout,
    pub signs: wgpu::RenderPipeline,
    pub text_layout: wgpu::BindGroupLayout,
    pub text_hdr: wgpu::RenderPipeline,
    pub text_crt: wgpu::RenderPipeline,
    pub blur_layout: wgpu::BindGroupLayout,
    pub blur_down: wgpu::RenderPipeline,
    pub blur_up: wgpu::RenderPipeline,
    pub outside_layout: wgpu::BindGroupLayout,
    pub outside: wgpu::RenderPipeline,
    pub window_layout: wgpu::BindGroupLayout,
    pub window: wgpu::RenderPipeline,
    pub passthrough: wgpu::RenderPipeline,
    pub crt_layout: wgpu::BindGroupLayout,
    pub crt: wgpu::RenderPipeline,
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Uint32x4];
const TEXT_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4];

struct Builder<'a> {
    device: &'a wgpu::Device,
}

impl Builder<'_> {
    fn layout(&self, label: &str, entries: &[wgpu::BindGroupLayoutEntry]) -> wgpu::BindGroupLayout {
        self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
    }

    fn shader(&self, label: &str, source: &'static str) -> wgpu::ShaderModule {
        self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn pipeline(
        &self,
        label: &str,
        layout: &wgpu::BindGroupLayout,
        shader: &wgpu::ShaderModule,
        fragment: &str,
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
        buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
    ) -> wgpu::RenderPipeline {
        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers,
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: crate::pipeline_cache::get().as_ref(),
        })
    }
}

pub(super) fn create(device: &wgpu::Device, output: wgpu::TextureFormat) -> Pipelines {
    let b = Builder { device };
    let fragment = wgpu::ShaderStages::FRAGMENT;
    let both = wgpu::ShaderStages::VERTEX_FRAGMENT;

    let signs_layout = b.layout("JKA neon menu signs", &[uniform_entry(0, both), storage_entry(1), storage_entry(2)]);
    let signs_shader = b.shader("JKA neon menu signs WGSL", include_str!("../../neon_menu/neon_signs.wgsl"));
    let signs = b.pipeline("JKA neon menu tubes", &signs_layout, &signs_shader, "fs_main", HDR, Some(ADDITIVE),
        &[Some(wgpu::VertexBufferLayout {
            array_stride: 32,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE_ATTRIBUTES,
        })]);

    let text_layout = b.layout("JKA neon menu text", &[uniform_entry(0, both), texture_entry(1), texture_entry(2), sampler_entry(3)]);
    let text_shader = b.shader("JKA neon menu text WGSL", include_str!("../../neon_menu/neon_text.wgsl"));
    let text_buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: 48,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &TEXT_ATTRIBUTES,
    })];
    let over = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
    let text_hdr = b.pipeline("JKA neon menu lightbox", &text_layout, &text_shader, "fs_main", HDR, over, &text_buffers);
    let text_crt = b.pipeline("JKA neon menu CRT text", &text_layout, &text_shader, "fs_main", CRT_FORMAT, over, &text_buffers);

    let blur_layout = b.layout("JKA neon menu blur", &[texture_entry(0), texture_entry(1), sampler_entry(2)]);
    let blur_shader = b.shader("JKA neon menu blur WGSL", include_str!("../../neon_menu/neon_blur.wgsl"));
    let blur_down = b.pipeline("JKA neon menu blur down", &blur_layout, &blur_shader, "fs_down", HDR, None, &[]);
    let blur_up = b.pipeline("JKA neon menu blur up", &blur_layout, &blur_shader, "fs_up", HDR, None, &[]);

    let outside_layout = b.layout("JKA neon menu street", &[
        texture_entry(0), texture_entry(1), texture_entry(2), sampler_entry(3), uniform_entry(4, fragment), texture_entry(5),
        texture_entry(6),
    ]);
    let outside_shader = b.shader("JKA neon menu street WGSL", include_str!("../../neon_menu/neon_outside.wgsl"));
    let outside = b.pipeline("JKA neon menu street", &outside_layout, &outside_shader, "fs_main", HDR, None, &[]);

    let window_layout = b.layout("JKA neon menu window", &[
        texture_entry(0), texture_entry(1), texture_entry(2), sampler_entry(4),
        uniform_entry(5, fragment), uniform_entry(6, fragment), texture_entry(7), texture_entry(8),
    ]);
    let window_shader = b.shader("JKA neon menu window WGSL", include_str!("../../neon_menu/neon_window.wgsl"));
    let window = b.pipeline("JKA neon menu window", &window_layout, &window_shader, "fs_main", output, None, &[]);
    let passthrough = b.pipeline("JKA neon menu scene copy", &window_layout, &window_shader, "fs_passthrough", output, None, &[]);

    // The stock crt-lottes port, unchanged, rendering into the menu monitor.
    let crt_layout = b.layout("JKA neon menu CRT", &[texture_entry(0), sampler_entry(1), uniform_entry(2, fragment)]);
    let crt_shader = b.shader("JKA neon menu crt-lottes WGSL", include_str!("../../crt_lottes.wgsl"));
    let crt = b.pipeline("JKA neon menu crt-lottes", &crt_layout, &crt_shader, "fs_lottes", CRT_FORMAT, None, &[]);

    Pipelines {
        signs_layout, signs, text_layout, text_hdr, text_crt, blur_layout, blur_down, blur_up,
        outside_layout, outside, window_layout, window, passthrough, crt_layout, crt,
    }
}
