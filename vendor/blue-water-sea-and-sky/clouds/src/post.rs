//! The sky's full-screen addition over the scene in linear radiance, before the display grade (`SkyApi.postProcess`):
//! heavy rain's veil, from the web game's `sky/weather/haze.ts`. It reads the scene's colour, so it runs after the scene pass:
//! [`PostProcess::encode`] takes a copy of the scene's colour and writes the result into the scene target. Nothing runs
//! while it is invisible. (The screen-space sun shafts that also ran here went with the old clouds: takram's light
//! shafts, in the sky's dome, take their place.)

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use naval_render::{
    MakePipelines, Pipeline, PipelineId, PipelineRegistry, PlaceholderPipelines, RenderContext,
    ShaderError,
};

use crate::gpu;
use crate::layer::CloudView;
use crate::shaders;
use crate::weather::{HazeState, WeatherLight};

/// `PostUniforms` in `shaders/post.wgsl`, 192 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct PostUniforms {
    inverse_projection: [[f32; 4]; 4],
    camera_world: [[f32; 4]; 4],
    haze_strength: f32,
    haze_height: f32,
    haze_ceiling: f32,
    lightning: f32,
    grey: f32,
    padding: [f32; 3],
    bolt_direction: [f32; 3],
    spare: f32,
    forward: [f32; 3],
    spare_too: f32,
}

/// The rain veil. See the module docs.
pub struct PostProcess {
    uniforms: PostUniforms,
    buffer: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    /// The uniforms' and the sky's group layouts and the composite's format, for the registry's recipe.
    uniform_layout: wgpu::BindGroupLayout,
    sky_layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

/// The veil's pipeline on the renderer's registry ([`PostProcess::register_pipelines`]), taken in by
/// [`PostProcess::adopt_pipelines`].
#[derive(Clone, Copy, Debug)]
pub struct PostPipelines {
    composite: PipelineId,
}

fn veil_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    [uniform_layout, sky_layout, texture_layout]: &[wgpu::BindGroupLayout; 3],
    format: wgpu::TextureFormat,
    cache: Option<&wgpu::PipelineCache>,
) -> wgpu::RenderPipeline {
    gpu::screen_pipeline(
        device,
        "sky post",
        module,
        "haze_vertex",
        "haze_fragment",
        &[Some(uniform_layout), Some(sky_layout), Some(texture_layout)],
        format,
        None,
        cache,
    )
}

impl PostProcess {
    /// The veil, writing `format` (the scene's `HDR_FORMAT`), over the sky's group-1 layout.
    pub fn new(
        device: &wgpu::Device,
        sky_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self::build(device, sky_layout, format, MakePipelines::Now)
    }

    /// The same with `placeholders` for its pipeline until [`PostProcess::adopt_pipelines`] takes the registry's.
    pub fn new_deferred(
        device: &wgpu::Device,
        sky_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        placeholders: &PlaceholderPipelines,
    ) -> Self {
        Self::build(
            device,
            sky_layout,
            format,
            MakePipelines::Deferred(placeholders),
        )
    }

    /// Builds the veil's pipeline through the renderer's registry, as [`PostProcess::new`] builds it. The clouds'
    /// files must be registered first (`crate::shaders::register`).
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<PostPipelines, ShaderError> {
        let layouts = [
            self.uniform_layout.clone(),
            self.sky_layout.clone(),
            self.texture_layout.clone(),
        ];
        let format = self.format;
        let composite = ctx.add_pipeline(
            "sky post",
            shaders::HAZE_PATH,
            Vec::new(),
            Box::new(move |build| {
                Pipeline::Render(veil_pipeline(build.device, build.module, &layouts, format, build.cache))
            }),
        )?;
        Ok(PostPipelines { composite })
    }

    /// Takes the registry's current pipeline in place of the pass's own; whether it changed.
    pub fn adopt_pipelines(&mut self, registry: &PipelineRegistry, ids: &PostPipelines) -> bool {
        let new = registry.render(ids.composite);
        if &self.pipeline != new {
            self.pipeline = new.clone();
            return true;
        }
        false
    }

    fn build(
        device: &wgpu::Device,
        sky_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        make: MakePipelines<'_>,
    ) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky post"),
            size: std::mem::size_of::<PostUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_layout = gpu::layout(device, "sky post uniforms", &[gpu::uniform_entry(0)]);
        let uniform_group = gpu::bind_group(
            device,
            "sky post uniforms",
            &uniform_layout,
            &[(0, buffer.as_entire_binding())],
        );
        let texture_layout = gpu::layout(
            device,
            "sky post textures",
            &[
                gpu::texture_entry(0, wgpu::TextureViewDimension::D2, false),
                gpu::depth_entry(1),
            ],
        );
        let layouts = [
            uniform_layout.clone(),
            sky_layout.clone(),
            texture_layout.clone(),
        ];
        let pipeline = make.render(|| {
            let module =
                shaders::shader_module(device, shaders::HAZE_WGSL, shaders::HAZE_PATH, &[]);
            veil_pipeline(device, &module, &layouts, format, None)
        });
        Self {
            uniforms: PostUniforms::default(),
            buffer,
            uniform_group,
            texture_layout,
            pipeline,
            uniform_layout,
            sky_layout: sky_layout.clone(),
            format,
        }
    }

    /// Take the rain veil's state for this view.
    pub fn update(&mut self, view: &CloudView, haze: HazeState, lightning: WeatherLight) {
        let u = &mut self.uniforms;
        u.inverse_projection = view.projection.inverse().to_cols_array_2d();
        u.camera_world = view.view.inverse().to_cols_array_2d();
        u.haze_strength = haze.strength;
        u.haze_height = haze.height;
        u.haze_ceiling = haze.ceiling;
        u.grey = haze.grey;
        u.lightning = lightning.irradiance;
        u.bolt_direction = lightning.direction.to_array();
        u.forward = view
            .view
            .inverse()
            .transform_vector3(Vec3::NEG_Z)
            .normalize()
            .to_array();
    }

    /// True when the veil would draw anything.
    pub fn active(&self) -> bool {
        self.uniforms.haze_strength > 0.0
    }

    /// Draws the veil into `target` (the scene's colour) from `color` (a copy of it). Returns false, touching
    /// nothing, when it is invisible.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        sky_group: &wgpu::BindGroup,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) -> bool {
        if !self.active() {
            return false;
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&self.uniforms));
        let group = gpu::bind_group(
            device,
            "sky post textures",
            &self.texture_layout,
            &[
                (0, wgpu::BindingResource::TextureView(color)),
                (1, wgpu::BindingResource::TextureView(depth)),
            ],
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sky post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_group, &[]);
        pass.set_bind_group(1, sky_group, &[]);
        pass.set_bind_group(2, &group, &[]);
        pass.draw(0..3, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rust_and_wgsl_layouts_agree() {
        let module = crate::shaders::compose(crate::shaders::HAZE_WGSL, "haze.wgsl", &[]).unwrap();
        let span = module
            .types
            .iter()
            .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
                (Some(name), wgpu::naga::TypeInner::Struct { span, .. })
                    if name.starts_with("PostUniforms") =>
                {
                    Some(*span)
                }
                _ => None,
            })
            .expect("PostUniforms");
        assert_eq!(span as usize, std::mem::size_of::<super::PostUniforms>());
    }
}
