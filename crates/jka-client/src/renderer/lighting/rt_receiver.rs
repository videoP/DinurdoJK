//! Shared controls and visibility history for dynamic-light RT receivers.
use super::*;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RtShadowReceiverFlags {
    flags: [u32; 4], // reserved, local cache enabled, map-light mode, ray budget
}

pub(super) struct RtReceiverResources {
    pub local: rt_local_rate::RtLocalRateHistory,
    pub flags_uniform: wgpu::Buffer,
}

impl RtReceiverResources {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            local: rt_local_rate::RtLocalRateHistory::new(device),
            flags_uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RT dynamic-light receiver flags"),
                contents: bytemuck::bytes_of(&RtShadowReceiverFlags::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.local.invalidate();
    }
}

pub(super) fn receiver_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        uniform_entry(15, wgpu::ShaderStages::FRAGMENT),
        rt_local_rate::receiver_entry(),
    ]
}

impl Renderer {
    pub(super) fn prepare_rt_receiver_flags(&self) {
        let Some(rt) = self.ray_traced_shadows.as_ref() else {
            return;
        };
        let map_lights = self
            .world
            .as_ref()
            .is_some_and(|w| w.active_pipeline_variant.map_light_simulation);
        self.queue.write_buffer(
            &rt.receiver.flags_uniform,
            0,
            bytemuck::bytes_of(&RtShadowReceiverFlags {
                flags: [
                    0,
                    u32::from(self.rt_local_cache_wanted()),
                    u32::from(map_lights),
                    self.rt_trace_rate,
                ],
            }),
        );
    }
}
