//! Visibility bindings.
use crate::renderer::RenderTargets;

pub(in crate::renderer) fn create_hiz_build_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    targets: &RenderTargets,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA Hi-Z build bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&targets.ao_depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&targets.hiz_mip_views[0]),
            },
        ],
    })
}

pub(in crate::renderer) fn create_hiz_reduce_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    targets: &RenderTargets,
) -> Vec<wgpu::BindGroup> {
    (1..targets.hiz_mip_count)
        .map(|mip| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA Hi-Z reduce bind group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &targets.hiz_mip_views[(mip - 1) as usize],
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &targets.hiz_mip_views[mip as usize],
                        ),
                    },
                ],
            })
        })
        .collect()
}

pub(in crate::renderer) fn create_cull_debug_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    reasons: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA cull debug bind group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: reasons.as_entire_binding(),
        }],
    })
}

pub(in crate::renderer) fn create_gpu_cull_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    records: &wgpu::Buffer,
    indirect: &wgpu::Buffer,
    hi_z_view: &wgpu::TextureView,
    settings: &wgpu::Buffer,
    active_indices: &wgpu::Buffer,
    compact_indirect: &wgpu::Buffer,
    compact_counts: &wgpu::Buffer,
    debug_reasons: &wgpu::Buffer,
    debug_counts: &wgpu::Buffer,
    early_indirect: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA GPU cull bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: records.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: indirect.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(hi_z_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: settings.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: active_indices.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: compact_indirect.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: compact_counts.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: debug_reasons.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: debug_counts.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: early_indirect.as_entire_binding(),
            },
        ],
    })
}
