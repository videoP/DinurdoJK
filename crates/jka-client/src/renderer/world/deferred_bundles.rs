//! Reuse the *exact ordered* main-view transparent BSP command stream when it
//! stays unchanged. Unlike shader fusion or sorting by material, this preserves
//! framebuffer blending, depth tests and authored stage ordering verbatim.
use crate::renderer::DEPTH_FORMAT;
use std::ops::Range;

const MIN_COMMANDS: usize = 48;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::renderer) struct DeferredResources {
    pub camera: wgpu::BindGroup,
    pub lighting: wgpu::BindGroup,
    pub shadow: wgpu::BindGroup,
    pub planar: wgpu::BindGroup,
    pub optical: Option<wgpu::BindGroup>,
    pub vertices: wgpu::Buffer,
    pub dlight_ids: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub color_format: wgpu::TextureFormat,
    pub sample_count: u32,
}

#[derive(Clone, PartialEq, Eq)]
pub(in crate::renderer) struct DeferredCommand {
    pub pipeline: wgpu::RenderPipeline,
    pub binding: crate::renderer::WorldPipelineBinding,
    pub surface: wgpu::BindGroup,
    pub ocean: Option<wgpu::BindGroup>,
    pub indices: Range<u32>,
}

#[derive(Default)]
pub(in crate::renderer) struct DeferredWorldBundles {
    resources: Option<DeferredResources>,
    commands: Vec<DeferredCommand>,
    pub scratch: Vec<DeferredCommand>,
    bundle: Option<wgpu::RenderBundle>,
    program_binds: [u32; 5],
}

#[derive(Default, Clone, Copy)]
pub(in crate::renderer) struct DeferredBundleResult {
    pub reused: bool,
    pub built: bool,
    pub represented_draws: u32,
    pub program_binds: [u32; 5],
}

impl DeferredWorldBundles {
    /// The first stable frame records a plan, the second builds the bundle,
    /// subsequent frames reuse it. Fast-changing PVS doesn't continuously
    /// rebuild cached commands. Returns no bundle for small/changed plans.
    pub fn update(&mut self, device: &wgpu::Device, resources: DeferredResources)
        -> DeferredBundleResult
    {
        let count = self.scratch.len();
        if count < MIN_COMMANDS {
            self.resources = None;
            self.commands.clear();
            self.scratch.clear();
            self.bundle = None;
            self.program_binds = [0; 5];
            return DeferredBundleResult::default();
        }
        if self.resources.as_ref() != Some(&resources) || self.commands != self.scratch {
            self.resources = Some(resources);
            std::mem::swap(&mut self.commands, &mut self.scratch);
            self.scratch.clear();
            self.bundle = None;
            self.program_binds = [0; 5];
            return DeferredBundleResult::default();
        }
        self.scratch.clear();
        if self.bundle.is_some() {
            return DeferredBundleResult { reused: true, represented_draws: count as u32, program_binds: self.program_binds, ..Default::default() };
        }
        let colors = [Some(resources.color_format)];
        let mut encoder = device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
            label: Some("JKA cached ordered transparent BSP"),
            color_formats: &colors,
            depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                format: DEPTH_FORMAT,
                depth_read_only: false,
                stencil_read_only: true,
            }),
            sample_count: resources.sample_count,
            multiview: None,
        });
        encoder.set_vertex_buffer(0, resources.vertices.slice(..));
        encoder.set_vertex_buffer(1, resources.dlight_ids.slice(..));
        encoder.set_index_buffer(resources.indices.slice(..), wgpu::IndexFormat::Uint32);
        encoder.set_bind_group(0, &resources.camera, &[]);
        encoder.set_bind_group(2, &resources.lighting, &[]);
        encoder.set_bind_group(3, &resources.shadow, &[]);
        encoder.set_bind_group(4, &resources.planar, &[]);
        if let Some(optical) = &resources.optical {
            encoder.set_bind_group(5, optical, &[]);
        }
        let mut pipeline: Option<&wgpu::RenderPipeline> = None;
        let mut surface: Option<&wgpu::BindGroup> = None;
        let mut ocean: Option<&wgpu::BindGroup> = None;
        for command in &self.commands {
            if pipeline != Some(&command.pipeline) {
                encoder.set_pipeline(&command.pipeline);
                use super::programs::StageProgram;
                let kind = match command.binding.1.map(|key| key.program) {
                    None => 0,
                    Some(StageProgram::Specialized | StageProgram::SpecializedLean) => 1,
                    Some(StageProgram::Minimal) => 2,
                    Some(StageProgram::SharedBasic | StageProgram::SharedEnhanced) => 3,
                    Some(StageProgram::GeneratedBasic | StageProgram::GeneratedEnhanced) => 4,
                };
                self.program_binds[kind] += 1;
                pipeline = Some(&command.pipeline);
                // Bind-group layouts can change with pipeline variants.
                surface = None;
                ocean = None;
            }
            if surface != Some(&command.surface) {
                encoder.set_bind_group(1, &command.surface, &[]);
                surface = Some(&command.surface);
            }
            if let Some(group) = &command.ocean {
                if ocean != Some(group) {
                    encoder.set_bind_group(6, group, &[]);
                    ocean = Some(group);
                }
            }
            encoder.draw_indexed(command.indices.clone(), 0, 0..1);
        }
        self.bundle = Some(encoder.finish(&wgpu::RenderBundleDescriptor {
            label: Some("JKA cached ordered transparent BSP"),
        }));
        DeferredBundleResult { built: true, represented_draws: count as u32, program_binds: self.program_binds, ..Default::default() }
    }

    pub fn bundle(&self) -> Option<&wgpu::RenderBundle> { self.bundle.as_ref() }
    pub fn clear(&mut self) { *self = Self::default(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_plans_do_not_cache() {
        assert_eq!(MIN_COMMANDS, 48);
        assert!(DeferredWorldBundles::default().bundle().is_none());
    }
}
