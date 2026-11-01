//! Exact command reuse for the ordinary opaque/masked indexed model stream.
//! Buffers/uniforms remain live; changed commands or resource handles invalidate.
use super::submission::model_runs;
use crate::renderer::{DynamicModelAlphaMode, DynamicModelRenderer, depth_format};
use std::{ops::Range, time::Instant};

const MIN_DRAWS: usize = 32;

#[derive(Clone, PartialEq, Eq)]
struct Resources {
    camera: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    samples: u32,
}

#[derive(Clone, PartialEq, Eq)]
struct Command {
    texture: wgpu::BindGroup,
    indices: Range<u32>,
    base_vertex: i32,
}

#[derive(Default)]
struct Slot {
    resources: Option<Resources>,
    commands: Vec<Command>,
    scratch: Vec<Command>,
    bundle: Option<wgpu::RenderBundle>,
}

impl Slot {
    /// Wait for two consecutive identical command plans before recording.
    /// Rapidly changing visibility never pays recurring bundle creation costs.
    fn update(&mut self, device: &wgpu::Device, resources: Resources) -> Update {
        if self.scratch.len() < MIN_DRAWS {
            self.resources = None;
            self.commands.clear();
            self.bundle = None;
            self.scratch.clear();
            return Update::Small;
        }
        if self.resources.as_ref() != Some(&resources) || self.commands != self.scratch {
            self.resources = Some(resources);
            std::mem::swap(&mut self.commands, &mut self.scratch);
            self.scratch.clear();
            self.bundle = None;
            return Update::Changed;
        }
        self.scratch.clear();
        if self.bundle.is_some() {
            return Update::Hit;
        }
        let colors = [Some(resources.format)];
        let mut encoder =
            device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                label: Some("JKA opaque/masked indexed model commands"),
                color_formats: &colors,
                depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                    format: depth_format(),
                    depth_read_only: false,
                    stencil_read_only: true,
                }),
                sample_count: resources.samples,
                multiview: None,
            });
        encoder.set_pipeline(&resources.pipeline);
        encoder.set_vertex_buffer(0, resources.vertices.slice(..));
        encoder.set_index_buffer(resources.indices.slice(..), wgpu::IndexFormat::Uint32);
        encoder.set_bind_group(0, &resources.camera, &[]);
        let mut bound_texture = None;
        for command in &self.commands {
            if bound_texture != Some(&command.texture) {
                encoder.set_bind_group(1, &command.texture, &[]);
                bound_texture = Some(&command.texture);
            }
            encoder.draw_indexed(command.indices.clone(), command.base_vertex, 0..1);
        }
        self.bundle = Some(encoder.finish(&wgpu::RenderBundleDescriptor {
            label: Some("JKA cached opaque/masked indexed model draws"),
        }));
        Update::Built
    }

    fn executable(
        &self,
        camera: &wgpu::BindGroup,
        pipeline: &wgpu::RenderPipeline,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
    ) -> Option<(&wgpu::RenderBundle, u32, &wgpu::BindGroup)> {
        let resources = self.resources.as_ref()?;
        if &resources.camera != camera
            || &resources.pipeline != pipeline
            || &resources.vertices != vertices
            || &resources.indices != indices
        {
            return None;
        }
        Some((
            self.bundle.as_ref()?,
            self.commands.len() as u32,
            &self.commands.last()?.texture,
        ))
    }
}

#[derive(PartialEq, Eq, Debug)]
enum Update {
    Small,
    Changed,
    Hit,
    Built,
}

#[derive(Default)]
pub(in crate::renderer) struct ModelBundles {
    slots: [Slot; 2],
}

#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct BundlePerf {
    pub key_ms: f64,
    pub build_ms: f64,
    pub hits: u32,
    pub misses: u32,
    pub builds: u32,
    pub eligible_draws: u32,
}

impl ModelBundles {
    pub fn clear(&mut self) {
        self.slots = Default::default();
    }

    pub fn executable(
        &self,
        alpha: DynamicModelAlphaMode,
        camera: &wgpu::BindGroup,
        pipeline: &wgpu::RenderPipeline,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
    ) -> Option<(&wgpu::RenderBundle, u32, &wgpu::BindGroup)> {
        self.slots
            .get(slot_index(alpha)?)?
            .executable(camera, pipeline, vertices, indices)
    }
}

fn slot_index(alpha: DynamicModelAlphaMode) -> Option<usize> {
    match alpha {
        DynamicModelAlphaMode::Opaque => Some(0),
        DynamicModelAlphaMode::Mask => Some(1),
        _ => None,
    }
}

pub(super) fn eligible_pass(
    rt_prepared: bool,
    excluded_entity: Option<u16>,
    authored_glow_only: bool,
) -> bool {
    !rt_prepared && excluded_entity.is_none() && !authored_glow_only
}

/// Keep the state contract of ordinary indexed model draws after execution.
#[allow(clippy::too_many_arguments)]
pub(super) fn execute_and_restore_model_state<'a>(
    pass: &mut wgpu::RenderPass<'a>,
    bundle: &'a wgpu::RenderBundle,
    camera: &'a wgpu::BindGroup,
    pipeline: &'a wgpu::RenderPipeline,
    vertices: &'a wgpu::Buffer,
    indices: &'a wgpu::Buffer,
    last_texture: &'a wgpu::BindGroup,
) {
    pass.execute_bundles(std::iter::once(bundle));
    pass.set_pipeline(pipeline);
    pass.set_vertex_buffer(0, vertices.slice(..));
    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
    pass.set_bind_group(0, camera, &[]);
    pass.set_bind_group(1, last_texture, &[]);
}

impl DynamicModelRenderer {
    pub(in crate::renderer) fn set_model_render_bundles(&mut self, enabled: bool) {
        if self.model_render_bundles != enabled {
            self.model_bundles.clear();
            self.model_render_bundles = enabled;
        }
    }

    /// Called after ordinary preparation and pipeline readiness, before a pass.
    /// All object identities and the exact ordered/merged draw list are checked.
    pub(in crate::renderer) fn prepare_model_bundles(
        &mut self,
        device: &wgpu::Device,
        camera: &wgpu::BindGroup,
    ) {
        self.perf_bundles = Default::default();
        if !self.model_render_bundles && self.igpu_ab_mode & 4 == 0 {
            return;
        }
        if self.rt_prepared || self.draws.is_empty() {
            self.model_bundles.clear();
            return;
        }
        for (slot_index, alpha) in [DynamicModelAlphaMode::Opaque, DynamicModelAlphaMode::Mask]
            .into_iter()
            .enumerate()
        {
            let Some(pipeline) = self.model_pipelines[alpha.index()].as_ref() else {
                self.model_bundles.slots[slot_index] = Default::default();
                continue;
            };
            let started = self.capture_prepare.then(Instant::now);
            let slot = &mut self.model_bundles.slots[slot_index];
            slot.scratch.clear();
            for (draw, indices) in model_runs(
                self.draws.iter().filter(|draw| draw.alpha_mode == alpha),
                self.dynamic_batch_draws,
            ) {
                let texture = draw
                    .texture_key
                    .as_ref()
                    .and_then(|key| self.textures.get(key))
                    .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                slot.scratch.push(Command {
                    texture: texture.clone(),
                    indices,
                    base_vertex: draw.base_vertex,
                });
            }
            self.perf_bundles.eligible_draws += slot.scratch.len() as u32;
            let resources = Resources {
                camera: camera.clone(),
                vertices: self.vertex_buffer.clone(),
                indices: self.index_buffer.clone(),
                pipeline: pipeline.clone(),
                format: self.fx_sprite_surface_format,
                samples: self.fx_sprite_samples,
            };
            if let Some(started) = started {
                self.perf_bundles.key_ms += started.elapsed().as_secs_f64() * 1000.0;
            }
            let started = self.capture_prepare.then(Instant::now);
            match slot.update(device, resources) {
                Update::Hit => self.perf_bundles.hits += 1,
                Update::Changed => self.perf_bundles.misses += 1,
                Update::Built => self.perf_bundles.builds += 1,
                Update::Small => {}
            }
            if let Some(started) = started {
                self.perf_bundles.build_ms += started.elapsed().as_secs_f64() * 1000.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_only_accept_ordinary_opaque_masked_draws() {
        assert!(eligible_pass(false, None, false));
        assert!(!eligible_pass(true, None, false));
        assert!(!eligible_pass(false, Some(7), false));
        assert!(!eligible_pass(false, None, true));
        assert_eq!(slot_index(DynamicModelAlphaMode::Opaque), Some(0));
        assert_eq!(slot_index(DynamicModelAlphaMode::Mask), Some(1));
        assert_eq!(slot_index(DynamicModelAlphaMode::Blend), None);
        assert_eq!(slot_index(DynamicModelAlphaMode::Additive), None);
        assert_eq!(
            slot_index(DynamicModelAlphaMode::BlendUnlitDepthWrite),
            None
        );
    }
}

#[cfg(test)]
#[path = "bundles_tests.rs"]
mod gpu_tests;
