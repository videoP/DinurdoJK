//! Visibility settings.
use crate::renderer::{
    create_gpu_cull_bind_group, create_hiz_build_bind_group, create_hiz_reduce_bind_groups,
    CullDebugMode, GpuCullSettings, Renderer,
};

impl Renderer {
    pub(in crate::renderer) fn set_gpu_visibility(
        &mut self,
        gpu_driven: bool,
        hiz_occlusion: bool,
    ) {
        self.gpu_driven_enabled = gpu_driven && self.indirect_supported;
        self.hiz_occlusion_enabled = hiz_occlusion && self.gpu_driven_enabled;
        self.rebuild_frame_plan();
        self.update_gpu_cull_settings();
    }

    pub(in crate::renderer) fn update_gpu_cull_settings(&self) {
        let (active_cull_count, compact_group_count) = self
            .world
            .as_ref()
            .map(|world| {
                (
                    world.active_cull_count,
                    u32::try_from(world.compact_groups.len()).unwrap_or(u32::MAX),
                )
            })
            .unwrap_or((0, 0));
        let settings = GpuCullSettings {
            viewport_mips_flags: [
                self.targets.hiz_width,
                self.targets.hiz_height,
                self.targets.hiz_mip_count.max(1),
                u32::from(self.gpu_driven_enabled && self.hiz_occlusion_enabled),
            ],
            active_compaction: [
                active_cull_count,
                compact_group_count,
                u32::from(
                    self.gpu_driven_enabled
                        && self.gpu_compaction_supported
                        && compact_group_count != 0,
                ),
                u32::from(self.cull_debug_mode != CullDebugMode::Off),
            ],
        };
        self.queue.write_buffer(
            &self.gpu_cull_settings_buffer,
            0,
            bytemuck::bytes_of(&settings),
        );
    }

    pub(in crate::renderer) fn rebuild_gpu_visibility_resources(&mut self) {
        self.hiz_build_bind_group =
            create_hiz_build_bind_group(&self.device, &self.hiz_build_layout, &self.targets);
        self.hiz_reduce_bind_groups =
            create_hiz_reduce_bind_groups(&self.device, &self.hiz_reduce_layout, &self.targets);
        self.update_gpu_cull_settings();
        if let Some(world) = &mut self.world {
            world.cull_bind_group = Some(create_gpu_cull_bind_group(
                &self.device,
                &self.gpu_cull_layout,
                &world.cull_records_buffer,
                &world.indirect_buffer,
                &self.targets.hiz_view,
                &self.gpu_cull_settings_buffer,
                &world.active_cull_indices_buffer,
                &world.compact_indirect_buffer,
                &world.compact_count_buffer,
                &world.cull_debug_reason_buffer,
                &world.cull_debug_count_buffer,
                &world.early_indirect_buffer,
            ));
        }
    }
}
