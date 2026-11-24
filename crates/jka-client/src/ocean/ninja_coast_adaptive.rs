use blue_water_gpu::{original_surface::adaptive_classifier, shader_composer};
use jka_river::river_ninja::checked_scope;
use std::sync::mpsc;

use super::{ninja_coast::coast_surface_pipeline, ninja_ocean::{storage, texture, uniform},
    original_surface::ReducedOcean, OCEAN_UNITS_PER_METER};

pub(super) enum Work {
    Skip,
    Shade,
    Sample(mpsc::Sender<Result<(), wgpu::BufferAsyncError>>),
}

#[derive(Clone, Copy)]
struct View {
    position: glam::Vec3,
    direction: glam::Vec3,
    foam: f32,
    contrast: f32,
}

impl View {
    fn matches(self, other: Self) -> bool {
        self.position.distance_squared(other.position) <= (2.0 * OCEAN_UNITS_PER_METER).powi(2)
            && self.direction.dot(other.direction) >= 0.996
            && self.foam == other.foam
            && self.contrast == other.contrast
    }
}

struct Coverage {
    readback: wgpu::Buffer,
    view: Option<View>,
    pending: Option<(View, mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>)>,
    frames_until_sample: u32,
    admitted: bool,
}

impl Coverage {
    fn plan(&mut self, view: View, debug: bool) -> Work {
        if !self.view.is_some_and(|previous| previous.matches(view)) {
            self.view = Some(view);
            self.admitted = false;
            self.frames_until_sample = 0;
        }
        if let Some((sampled_view, receiver)) = &self.pending {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    if let Ok(bytes) = self.readback.slice(..).get_mapped_range() {
                        if sampled_view.matches(view) {
                            let tiles: &[u32] = bytemuck::cast_slice(&bytes);
                            let coarse = tiles.iter().filter(|&&decision| decision == 0).count();
                            // Same-launch Costa comparisons showed a loss with roughly
                            // 37% coarse pixels and a gain with 52%. Keep hysteresis
                            // as foam moves between tiles at the break-even point.
                            let minimum = if self.admitted { 40 } else { 45 };
                            self.admitted = coarse * 100 >= tiles.len() * minimum;
                            self.frames_until_sample = if self.admitted { 32 } else { 120 };
                        }
                    }
                    self.readback.unmap();
                    self.pending = None;
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if debug { return Work::Shade; }
        self.frames_until_sample = self.frames_until_sample.saturating_sub(1);
        if self.frames_until_sample == 0 && self.pending.is_none() {
            let (sender, receiver) = mpsc::channel();
            self.pending = Some((view, receiver));
            self.frames_until_sample = 120;
            return Work::Sample(sender);
        }
        if self.admitted { Work::Shade } else { Work::Skip }
    }
}

pub(super) struct Pipeline {
    pub coarse: wgpu::RenderPipeline,
    pub full: wgpu::RenderPipeline,
    pub classify: wgpu::ComputePipeline,
    pub target: ReducedOcean,
    coverage: Coverage,
}

impl Pipeline {
    pub fn new(device: &wgpu::Device, groups: &[wgpu::BindGroupLayout; 5],
        (width, height, samples, format): (u32, u32, u32, wgpu::TextureFormat),
        steps: u32, ssr: bool,
    ) -> Result<Self, String> {
        if device.limits().max_bind_groups < 6 {
            return Err("needs six render bind groups".into());
        }
        let material = jka_ninja::coast_material_wgsl().ok_or("Fluid Ninja private provider not included")?;
        let compose = |coarse| shader_composer::ninja_coast_adaptive_module(
            &material, jka_water::river::slw::SHADER, steps, ssr, coarse,
        );
        let coarse_module = compose(true)?;
        let full_module = compose(false)?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = |module, label| device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label), source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let coarse_shader = shader(coarse_module, "Coast 2x2 deep-water shading");
        let full_shader = shader(full_module, "Coast adaptive deep-water shading");
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let reduced = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Coast adaptive shading and tile decisions"),
            entries: &[texture(0, fragment), wgpu::BindGroupLayoutEntry {
                binding: 1, visibility: fragment, count: None,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
                },
            }, wgpu::BindGroupLayoutEntry { visibility: fragment, ..storage(2, true) }, uniform(3, fragment)],
        });
        let [camera, material, waves, environment, coast] = groups;
        let coarse_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Coast 2x2 shading"),
            bind_group_layouts: &[Some(camera), Some(material), Some(waves), Some(environment), Some(coast)],
            immediate_size: 0,
        });
        let full_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Coast adaptive shading and reconstruction"),
            bind_group_layouts: &[Some(camera), Some(material), Some(waves), Some(environment), Some(coast), Some(&reduced)],
            immediate_size: 0,
        });
        let coarse = coast_surface_pipeline(device, &coarse_layout, &coarse_shader, wgpu::TextureFormat::Rgba16Float,
            1, "deep_fragment", "Coast deep water at 2x2 rate", None);
        let full = coast_surface_pipeline(device, &full_layout, &full_shader, format, samples,
            "deep_fragment", "Coast adaptive deep-water shading", None);
        let (classify, classify_layout) = adaptive_classifier(device);
        let target = ReducedOcean::new(device, &reduced, Some(&classify_layout), width, height, 2, false);
        let valid = checked_scope(validation);
        let enough_memory = checked_scope(memory);
        valid?;
        enough_memory?;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Coast adaptive tile coverage readback"),
            size: target.tile_buffer.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let coverage = Coverage { readback, view: None, pending: None, frames_until_sample: 0, admitted: false };
        Ok(Self { coarse, full, classify, target, coverage })
    }

    pub fn plan(&mut self, camera: &crate::camera::Camera, foam: f32, contrast: f32, debug: bool) -> Work {
        self.coverage.plan(View { position: camera.position, direction: camera.forward(), foam, contrast }, debug)
    }

    pub fn sample(&self, encoder: &mut wgpu::CommandEncoder, complete: mpsc::Sender<Result<(), wgpu::BufferAsyncError>>) {
        let readback = &self.coverage.readback;
        encoder.copy_buffer_to_buffer(&self.target.tile_buffer, 0, readback, 0, readback.size());
        encoder.map_buffer_on_submit(readback, wgpu::MapMode::Read, .., move |result| {
            let _ = complete.send(result);
        });
    }
}
