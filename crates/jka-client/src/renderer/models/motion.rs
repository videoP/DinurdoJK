//! DLSS-only object motion for entity prepass motion vectors.
//!
//! GPU-skinned Ghoul2 keeps last frame's transform and bone palette per
//! instance, so its prepass writes true object + skeletal motion. MD3 and
//! CPU-skinned geometry arrive pre-transformed and get camera-only motion.
//! None of this exists unless `object_motion` is set (DLSS active): ordinary
//! frames keep the original pipelines, layouts and uploads.
use super::types::{DynamicModelRenderer, Ghoul2GpuDrawUniform};
use crate::renderer::{
    create_entity_prepass_pipeline, DynamicModelVertex, Ghoul2GpuBone, Ghoul2GpuVertex, HashMap, Pod,
    Zeroable, DYNAMIC_MODEL_VERTEX_ATTRIBUTES, GHOUL2_GPU_VERTEX_ATTRIBUTES,
};

/// Group-3 bindings of the Ghoul2 motion prepass. Far above the glass
/// bindings the same shader module declares at group 3.
pub(in crate::renderer) const PREVIOUS_BONES_BINDING: u32 = 16;
pub(in crate::renderer) const SKIN_MOTION_BINDING: u32 = 17;

/// Previous-frame transform for the Ghoul2 instance at the same draw index.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct SkinMotionUniform {
    axis0: [f32; 4],
    axis1: [f32; 4],
    axis2: [f32; 4],
    origin: [f32; 4],
    /// x: previous bone base, y: 1 when a previous pose exists.
    params: [u32; 4],
}

#[derive(Clone, Copy)]
struct PreviousPose {
    transform: [[f32; 4]; 4],
    bone_base: u32,
    bone_count: u32,
}

pub(in crate::renderer) struct MotionPrepassPipelines {
    pub(in crate::renderer) md3: wgpu::RenderPipeline,
    pub(in crate::renderer) md3_mask: wgpu::RenderPipeline,
    pub(in crate::renderer) skin: wgpu::RenderPipeline,
    pub(in crate::renderer) skin_mask: wgpu::RenderPipeline,
}

struct MotionBuffers {
    previous_bones: wgpu::Buffer,
    motion: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

#[derive(Default)]
pub(in crate::renderer) struct ObjectMotion {
    history: HashMap<(u16, u32), PreviousPose>,
    previous_bones: Vec<Ghoul2GpuBone>,
    motion: Vec<SkinMotionUniform>,
    pub(in crate::renderer) pipelines: Option<MotionPrepassPipelines>,
    buffers: Option<MotionBuffers>,
}

impl ObjectMotion {
    /// Forget poses so re-enabling DLSS never pairs with a stale frame.
    pub(in crate::renderer) fn reset(&mut self) {
        self.history.clear();
        self.previous_bones.clear();
    }

    pub(in crate::renderer) fn skin_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.buffers.as_ref().map(|buffers| &buffers.bind_group)
    }
}

impl DynamicModelRenderer {
    /// Uploads last frame's poses for this frame's Ghoul2 instances, then
    /// records this frame's for the next. Instances are matched by entity and
    /// by the rank of their bone palette within that entity; a palette whose
    /// size changed falls back to camera-only motion.
    pub(in crate::renderer) fn update_object_motion(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draws: &[Ghoul2GpuDrawUniform],
        bones: &[Ghoul2GpuBone],
    ) {
        if !self.object_motion {
            self.motion.reset();
            return;
        }
        self.ensure_motion_pipelines(device);
        let mut bases: Vec<u32> = draws.iter().map(|draw| draw.params[0]).collect();
        bases.sort_unstable();
        bases.dedup();
        let bone_count = |base: u32| {
            let next = bases.partition_point(|&other| other <= base);
            bases.get(next).copied().unwrap_or(bones.len() as u32).saturating_sub(base)
        };
        let entity = |index: usize| self.skin_instance_entities.get(index).copied().unwrap_or(u16::MAX);
        let mut instances: Vec<(u16, u32)> = draws
            .iter()
            .enumerate()
            .map(|(index, draw)| (entity(index), draw.params[0]))
            .collect();
        instances.sort_unstable();
        instances.dedup();
        let rank = |entity: u16, base: u32| {
            let first = instances.partition_point(|&(other, _)| other < entity);
            (instances.partition_point(|&key| key < (entity, base)) - first) as u32
        };
        let motion = &mut self.motion;
        motion.motion.clear();
        let mut history = HashMap::with_capacity(motion.history.len());
        for (index, draw) in draws.iter().enumerate() {
            let entity = entity(index);
            let base = draw.params[0];
            let key = (entity, rank(entity, base));
            let count = bone_count(base);
            let previous = (entity != u16::MAX)
                .then(|| motion.history.get(&key))
                .flatten()
                .filter(|pose| pose.bone_count == count);
            motion.motion.push(match previous {
                Some(pose) => SkinMotionUniform {
                    axis0: pose.transform[0],
                    axis1: pose.transform[1],
                    axis2: pose.transform[2],
                    origin: pose.transform[3],
                    params: [pose.bone_base, 1, 0, 0],
                },
                None => SkinMotionUniform {
                    axis0: draw.axis0,
                    axis1: draw.axis1,
                    axis2: draw.axis2,
                    origin: draw.origin,
                    params: [base, 0, 0, 0],
                },
            });
            history.insert(key, PreviousPose {
                transform: [draw.axis0, draw.axis1, draw.axis2, draw.origin],
                bone_base: base,
                bone_count: count,
            });
        }
        motion.history = history;

        let previous_bytes: &[u8] = bytemuck::cast_slice(&motion.previous_bones);
        let motion_bytes: &[u8] = bytemuck::cast_slice(&motion.motion);
        let fits = |buffer: &wgpu::Buffer, bytes: &[u8]| bytes.len() as u64 <= buffer.size();
        if motion.buffers.as_ref().is_none_or(|buffers| {
            !fits(&buffers.previous_bones, previous_bytes) || !fits(&buffers.motion, motion_bytes)
        }) {
            let buffer = |label, bytes: &[u8]| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: (bytes.len() as u64).next_power_of_two().max(4096),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            let previous_bones = buffer("JKA DLSS previous Ghoul2 bones", previous_bytes);
            let motion_buffer = buffer("JKA DLSS Ghoul2 previous transforms", motion_bytes);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA DLSS Ghoul2 motion bind group"),
                layout: &self.motion_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: PREVIOUS_BONES_BINDING,
                        resource: previous_bones.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: SKIN_MOTION_BINDING,
                        resource: motion_buffer.as_entire_binding(),
                    },
                ],
            });
            motion.buffers = Some(MotionBuffers { previous_bones, motion: motion_buffer, bind_group });
        }
        let buffers = motion.buffers.as_ref().expect("created above");
        if !previous_bytes.is_empty() {
            queue.write_buffer(&buffers.previous_bones, 0, previous_bytes);
        }
        if !motion_bytes.is_empty() {
            queue.write_buffer(&buffers.motion, 0, motion_bytes);
        }
        motion.previous_bones.clear();
        motion.previous_bones.extend_from_slice(bones);
    }

    /// Compiled on first DLSS frame only, never on ordinary frames.
    fn ensure_motion_pipelines(&mut self, device: &wgpu::Device) {
        if self.motion.pipelines.is_some() {
            return;
        }
        let md3_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DynamicModelVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
        };
        let skin_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Ghoul2GpuVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &GHOUL2_GPU_VERTEX_ATTRIBUTES,
        };
        let md3 = |alpha_test, label| {
            create_entity_prepass_pipeline(
                device, &self.pipeline_layout, &self.shader, md3_layout.clone(), alpha_test,
                Some("vs_prepass"), label,
            )
        };
        let skin = |alpha_test, label| {
            create_entity_prepass_pipeline(
                device, &self.skin_motion_pipeline_layout, &self.skin_shader, skin_layout.clone(),
                alpha_test, Some("vs_prepass_motion"), label,
            )
        };
        self.motion.pipelines = Some(MotionPrepassPipelines {
            md3: md3(false, "JKA dynamic model DLSS motion prepass"),
            md3_mask: md3(true, "JKA dynamic model alpha-test DLSS motion prepass"),
            skin: skin(false, "JKA Ghoul2 DLSS motion prepass"),
            skin_mask: skin(true, "JKA Ghoul2 alpha-test DLSS motion prepass"),
        });
    }
}
