//! Post bindings.
use crate::renderer::{BloomPyramid, DofTarget, TemporalSsaoHistory, TemporalSsrHistory};

pub(in crate::renderer) fn create_ssao_temporal_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    linear_depth_view: &wgpu::TextureView,
    history: Option<&TemporalSsaoHistory>,
    sampler: &wgpu::Sampler,
    buffer: &wgpu::Buffer,
) -> Option<[wgpu::BindGroup; 2]> {
    let history = history?;
    Some(std::array::from_fn(|history_index| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA temporal SSAO bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(linear_depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&history.views[history_index]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buffer.as_entire_binding(),
                },
            ],
        })
    }))
}

pub(in crate::renderer) fn create_ssr_temporal_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_view: &wgpu::TextureView,
    linear_depth_view: &wgpu::TextureView,
    history: Option<&TemporalSsrHistory>,
    sampler: &wgpu::Sampler,
    buffer: &wgpu::Buffer,
    weather_height_view: &wgpu::TextureView,
    weather_sampler: &wgpu::Sampler,
    weather_surface_buffer: &wgpu::Buffer,
    reflection_mask_view: &wgpu::TextureView,
    ssr_visibility_view: Option<&wgpu::TextureView>,
) -> Option<[wgpu::BindGroup; 2]> {
    let history = history?;
    let ssr_visibility_view = ssr_visibility_view?;
    Some(std::array::from_fn(|history_index| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA temporal SSR bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(linear_depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(
                        &history.radiance_views[history_index],
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(
                        &history.depth_views[history_index],
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(weather_height_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(weather_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: weather_surface_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(reflection_mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(ssr_visibility_view),
                },
            ],
        })
    }))
}

pub(in crate::renderer) fn create_fast_post_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA known-fast gamma post bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buffer.as_entire_binding(),
            },
        ],
    })
}

pub(in crate::renderer) fn create_gamma_post_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    buffer: &wgpu::Buffer,
    color_lut_view: &wgpu::TextureView,
    color_lut_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA gamma-only post bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(color_lut_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(color_lut_sampler),
            },
        ],
    })
}

pub(in crate::renderer) fn create_auto_exposure_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_view: &wgpu::TextureView,
    state_buffer: &wgpu::Buffer,
    settings_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA auto-exposure bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: state_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: settings_buffer.as_entire_binding(),
            },
        ],
    })
}

pub(in crate::renderer) fn create_post_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_view: &wgpu::TextureView,
    linear_depth_view: &wgpu::TextureView,
    history_view: &wgpu::TextureView,
    ssao_view: &wgpu::TextureView,
    ssr_radiance_view: &wgpu::TextureView,
    ssr_depth_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    buffer: &wgpu::Buffer,
    froxel_buffer: &wgpu::Buffer,
    bloom: Option<&BloomPyramid>,
    dof: Option<&DofTarget>,
    color_lut_view: &wgpu::TextureView,
    color_lut_sampler: &wgpu::Sampler,
    cloud_detail_view: &wgpu::TextureView,
    cloud_noise_sampler: &wgpu::Sampler,
    cloud_transfer_view: &wgpu::TextureView,
    weather_occlusion_view: &wgpu::TextureView,
    rain_haze_mask_view: &wgpu::TextureView,
    cloud_weather_view: &wgpu::TextureView,
    motion_vector_view: &wgpu::TextureView,
    scene_depth_view: &wgpu::TextureView,
    reflection_mask_view: &wgpu::TextureView,
    auto_exposure_state_buffer: &wgpu::Buffer,
    ssr_visibility_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    let bloom_half = bloom.map_or(scene_view, |pyramid| &pyramid.views[0]);
    let bloom_quarter = bloom.map_or(scene_view, |pyramid| &pyramid.views[1]);
    let bloom_eighth = bloom.map_or(scene_view, |pyramid| &pyramid.views[2]);
    let dof_view = dof.map_or(scene_view, |target| &target.view);
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA post-process bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(linear_depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(history_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: froxel_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(bloom_half),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(bloom_quarter),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(bloom_eighth),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(color_lut_view),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::Sampler(color_lut_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(ssao_view),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: wgpu::BindingResource::TextureView(ssr_radiance_view),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: wgpu::BindingResource::TextureView(ssr_depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: wgpu::BindingResource::TextureView(dof_view),
            },
            wgpu::BindGroupEntry {
                binding: 15,
                resource: wgpu::BindingResource::TextureView(cloud_detail_view),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: wgpu::BindingResource::Sampler(cloud_noise_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 17,
                resource: wgpu::BindingResource::TextureView(cloud_transfer_view),
            },
            wgpu::BindGroupEntry {
                binding: 18,
                resource: wgpu::BindingResource::TextureView(weather_occlusion_view),
            },
            wgpu::BindGroupEntry {
                binding: 19,
                resource: wgpu::BindingResource::TextureView(rain_haze_mask_view),
            },
            wgpu::BindGroupEntry {
                binding: 20,
                resource: wgpu::BindingResource::TextureView(cloud_weather_view),
            },
            wgpu::BindGroupEntry {
                binding: 21,
                resource: wgpu::BindingResource::TextureView(motion_vector_view),
            },
            wgpu::BindGroupEntry {
                binding: 22,
                resource: wgpu::BindingResource::TextureView(scene_depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 23,
                resource: wgpu::BindingResource::TextureView(reflection_mask_view),
            },
            wgpu::BindGroupEntry {
                binding: 24,
                resource: auto_exposure_state_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 25,
                resource: wgpu::BindingResource::TextureView(ssr_visibility_view),
            },
        ],
    })
}

pub(in crate::renderer) fn create_dof_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_view: &wgpu::TextureView,
    linear_depth_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    buffer: &wgpu::Buffer,
    dof: Option<&DofTarget>,
) -> Option<wgpu::BindGroup> {
    dof?;
    Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA Gaussian DOF bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(linear_depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: buffer.as_entire_binding(),
            },
        ],
    }))
}

pub(in crate::renderer) fn create_bloom_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    scene_view: &wgpu::TextureView,
    bloom: Option<&BloomPyramid>,
) -> Option<[wgpu::BindGroup; 3]> {
    let bloom = bloom?;
    let sources = [scene_view, &bloom.views[0], &bloom.views[1]];
    Some(std::array::from_fn(|level| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA bloom downsample bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(sources[level]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }))
}
