//! GPU exposure against the original sky's cloudy and clear references.
use crate::renderer::{
    create_auto_exposure_bind_group, storage_buffer_entry, unfilterable_texture_entry_stages,
    uniform_entry,
};

pub(super) struct NativeExposure {
    sky_mean: blue_water_sky::SkyMeanSampler,
    layout: wgpu::BindGroupLayout,
    reference: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
    inputs: Option<(
        wgpu::TextureView,
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::BindGroup,
    )>,
}

impl NativeExposure {
    pub fn new(device: &wgpu::Device) -> Self {
        let sky_mean = blue_water_sky::SkyMeanSampler::new(device);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native exposure scene"),
            entries: &[
                unfilterable_texture_entry_stages(0, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(1, false, wgpu::ShaderStages::COMPUTE),
                uniform_entry(2, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let reference_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native exposure sky reference"),
            entries: &[storage_buffer_entry(0, true, wgpu::ShaderStages::COMPUTE)],
        });
        let reference = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native exposure sky reference"),
            layout: &reference_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sky_mean.buffer.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native sky-referenced exposure"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../../auto_exposure.wgsl"),
                    include_str!("blue_water_exposure.wgsl")
                )
                .into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native sky-referenced exposure"),
            bind_group_layouts: &[Some(&layout), Some(&reference_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("native sky-referenced exposure"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cs_native"),
            compilation_options: Default::default(),
            cache: crate::pipeline_cache::get().as_ref(),
        });
        Self {
            sky_mean,
            layout,
            reference,
            pipeline,
            inputs: None,
        }
    }

    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        environment: &blue_water_sky::EnvironmentMap,
        scene: &wgpu::TextureView,
        state: &wgpu::Buffer,
        settings: &wgpu::Buffer,
    ) {
        self.sky_mean.encode(encoder, environment);
        self.adapt(device, encoder, scene, state, settings);
    }

    fn adapt(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        state: &wgpu::Buffer,
        settings: &wgpu::Buffer,
    ) {
        if self
            .inputs
            .as_ref()
            .is_none_or(|(view, old_state, old_settings, _)| {
                view != scene || old_state != state || old_settings != settings
            })
        {
            let group =
                create_auto_exposure_bind_group(device, &self.layout, scene, state, settings);
            self.inputs = Some((scene.clone(), state.clone(), settings.clone(), group));
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Blue Water sky-referenced eye adaptation"),
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.inputs.as_ref().unwrap().3, &[]);
        pass.set_bind_group(1, &self.reference, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

#[cfg(test)]
mod tests;
