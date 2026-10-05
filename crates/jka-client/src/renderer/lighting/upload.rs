//! Lighting upload.
use crate::renderer::{
    scene, GpuReflectionProbe, IrradianceVolumeUniform, ReflectionProbeGpuSet, StaticLightGridGpu,
    StaticLightGridUniform, VoxelProbeGiGpu, VoxelProbeGiUniform,
};
use wgpu::util::DeviceExt;

pub(in crate::renderer) fn upload_reflection_probes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    probes: &[scene::ReflectionProbe],
) -> ReflectionProbeGpuSet {
    let mut uploaded = Vec::with_capacity(probes.len().max(1));
    if probes.is_empty() {
        let data = [128u8, 128, 128, 255].repeat(6);
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("JKA fallback reflection cubemap"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("JKA fallback reflection cubemap view"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            array_layer_count: Some(6),
            ..Default::default()
        });
        uploaded.push(GpuReflectionProbe {
            _texture: texture,
            view,
        });
    } else {
        for probe in probes {
            let texture = device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some(&probe.label),
                    size: wgpu::Extent3d {
                        width: probe.width,
                        height: probe.height,
                        depth_or_array_layers: 6,
                    },
                    mip_level_count: probe.mip_level_count,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &probe.rgba,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("JKA Rend2 reflection cubemap view"),
                dimension: Some(wgpu::TextureViewDimension::Cube),
                array_layer_count: Some(6),
                ..Default::default()
            });
            uploaded.push(GpuReflectionProbe {
                _texture: texture,
                view,
            });
        }
    }
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA reflection cubemap sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    ReflectionProbeGpuSet {
        probes: uploaded,
        sampler,
        source_count: probes.len(),
    }
}

pub(in crate::renderer) fn upload_voxel_probe_gi(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grid: Option<&scene::VoxelProbeGi>,
) -> VoxelProbeGiGpu {
    let fallback = [[0_u8; 4]];
    let (size, point_data, area_data, uniform) = if let Some(grid) = grid {
        let size = wgpu::Extent3d {
            width: grid.bounds[0].max(1),
            height: grid.bounds[1].max(1),
            depth_or_array_layers: grid.bounds[2].max(1),
        };
        (
            size,
            grid.point_rgba.as_slice(),
            grid.area_rgba.as_slice(),
            VoxelProbeGiUniform {
                origin_enabled: [grid.origin[0], grid.origin[1], grid.origin[2], 1.0],
                inv_cell: [
                    1.0 / grid.cell_size.max(1e-6),
                    1.0 / grid.cell_size.max(1e-6),
                    1.0 / grid.cell_size.max(1e-6),
                    grid.cell_size,
                ],
                bounds_strength: [
                    grid.bounds[0] as f32,
                    grid.bounds[1] as f32,
                    grid.bounds[2] as f32,
                    0.38,
                ],
            },
        )
    } else {
        (
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            fallback.as_slice(),
            fallback.as_slice(),
            VoxelProbeGiUniform {
                origin_enabled: [0.0; 4],
                inv_cell: [1.0, 1.0, 1.0, 1.0],
                bounds_strength: [1.0, 1.0, 1.0, 0.0],
            },
        )
    };

    let descriptor = |label: &'static str| wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    let point_texture = device.create_texture_with_data(
        queue,
        &descriptor("JKA voxel GI point probes"),
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(point_data),
    );
    let area_texture = device.create_texture_with_data(
        queue,
        &descriptor("JKA voxel GI emissive probes"),
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(area_data),
    );
    let view_descriptor = wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    };
    let point_view = point_texture.create_view(&view_descriptor);
    let area_view = area_texture.create_view(&view_descriptor);
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA voxel/probe GI uniform"),
        contents: bytemuck::bytes_of(&uniform),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    VoxelProbeGiGpu {
        enabled: grid.is_some(),
        _point_texture: point_texture,
        point_view,
        _area_texture: area_texture,
        area_view,
        uniform_buffer,
    }
}

pub(in crate::renderer) fn upload_static_light_grid(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grid: Option<&scene::StaticLightGrid>,
) -> StaticLightGridGpu {
    let enabled = grid.is_some();
    let external_hdr = grid.is_some_and(|grid| grid.external_hdr);
    let (
        size,
        direction_data,
        lighting_data,
        uniform,
        irradiance_size,
        irradiance_data,
        irradiance_uniform,
    ) = if let Some(grid) = grid {
        let size = wgpu::Extent3d {
            width: grid.bounds[0].max(1),
            height: grid.bounds[1].max(1),
            depth_or_array_layers: grid.bounds[2].max(1),
        };
        let uniform = StaticLightGridUniform {
            origin_enabled: [grid.origin[0], grid.origin[1], grid.origin[2], 1.0],
            inv_size: [
                1.0 / grid.size[0].max(1e-6),
                1.0 / grid.size[1].max(1e-6),
                1.0 / grid.size[2].max(1e-6),
                0.0,
            ],
            bounds: [
                grid.bounds[0] as f32,
                grid.bounds[1] as f32,
                grid.bounds[2] as f32,
                0.0,
            ],
        };
        let irradiance_size = wgpu::Extent3d {
            width: grid.bounds[0].max(1),
            height: grid.bounds[1].max(1).saturating_mul(2),
            depth_or_array_layers: grid.bounds[2].max(1).saturating_mul(3),
        };
        // Bevy light probes are unit cubes centered on the origin. Expand the
        // JKA lightgrid by half a cell around its first/last probes so every BSP
        // sample lands at the same texel center after Bevy's clamp-to-center
        // sampling. Renderer coordinates are [JKA X, JKA Z, -JKA Y].
        let resolution = [
            grid.bounds[0] as f32,
            grid.bounds[1] as f32,
            grid.bounds[2] as f32,
        ];
        let full_extent = [
            grid.size[0].max(1e-6) * resolution[0].max(1.0),
            grid.size[1].max(1e-6) * resolution[1].max(1.0),
            grid.size[2].max(1e-6) * resolution[2].max(1.0),
        ];
        let center_jka = [
            grid.origin[0] + grid.size[0] * (resolution[0] - 1.0) * 0.5,
            grid.origin[1] + grid.size[1] * (resolution[1] - 1.0) * 0.5,
            grid.origin[2] + grid.size[2] * (resolution[2] - 1.0) * 0.5,
        ];
        let inv_extent = [
            1.0 / full_extent[0],
            1.0 / full_extent[1],
            1.0 / full_extent[2],
        ];
        let irradiance_uniform = IrradianceVolumeUniform {
            // Column-major affine matrix, matching WGSL mat4x4 layout.
            // probe.x = (renderer.x  - JKA_center.x) / JKA_extent.x
            // probe.y = (-renderer.z - JKA_center.y) / JKA_extent.y
            // probe.z = (renderer.y  - JKA_center.z) / JKA_extent.z
            light_from_world: [
                [inv_extent[0], 0.0, 0.0, 0.0],
                [0.0, 0.0, inv_extent[2], 0.0],
                [0.0, -inv_extent[1], 0.0, 0.0],
                [
                    -center_jka[0] * inv_extent[0],
                    -center_jka[1] * inv_extent[1],
                    -center_jka[2] * inv_extent[2],
                    1.0,
                ],
            ],
            // Bevy's LightProbe default falloff is zero (sharp probe boundary).
            // Keeping this as explicit probe metadata makes the runtime path use
            // the same bounded-volume query/weight machinery as Bevy.
            falloff_intensity: [0.0, 0.0, 0.0, grid.irradiance_intensity],
            volume_enabled_pad: [1.0, 0.0, 0.0, 0.0],
        };
        (
            size,
            grid.direction_rgba.as_slice(),
            grid.lighting_rgba.as_slice(),
            uniform,
            irradiance_size,
            grid.irradiance_volume_rgba.as_slice(),
            irradiance_uniform,
        )
    } else {
        (
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            &[128, 255, 128, 0][..],
            &[255, 255, 255, 0][..],
            StaticLightGridUniform {
                origin_enabled: [0.0; 4],
                inv_size: [1.0, 1.0, 1.0, 0.0],
                bounds: [1.0, 1.0, 1.0, 0.0],
            },
            wgpu::Extent3d {
                width: 1,
                height: 2,
                depth_or_array_layers: 3,
            },
            &[0u8; 24][..],
            IrradianceVolumeUniform {
                light_from_world: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
                falloff_intensity: [0.0, 0.0, 0.0, 1.0],
                volume_enabled_pad: [0.0; 4],
            },
        )
    };

    let descriptor = |label: &'static str| wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    let direction_texture = device.create_texture_with_data(
        queue,
        &descriptor("JKA static lightgrid direction"),
        wgpu::util::TextureDataOrder::LayerMajor,
        direction_data,
    );
    let lighting_texture = device.create_texture_with_data(
        queue,
        &descriptor("JKA static lightgrid lighting"),
        wgpu::util::TextureDataOrder::LayerMajor,
        lighting_data,
    );
    let direction_view = direction_texture.create_view(&Default::default());
    let lighting_view = lighting_texture.create_view(&Default::default());
    let irradiance_descriptor = wgpu::TextureDescriptor {
        label: Some("Bevy irradiance volume ambient-cube atlas"),
        size: irradiance_size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    let irradiance_volume_texture = device.create_texture_with_data(
        queue,
        &irradiance_descriptor,
        wgpu::util::TextureDataOrder::LayerMajor,
        irradiance_data,
    );
    let irradiance_volume_view = irradiance_volume_texture.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA static lightgrid sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA static lightgrid uniform"),
        contents: bytemuck::bytes_of(&uniform),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let irradiance_volume_uniform_buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Bevy irradiance volume uniform"),
            contents: bytemuck::bytes_of(&irradiance_uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    StaticLightGridGpu {
        enabled,
        external_hdr,
        _direction_texture: direction_texture,
        direction_view,
        _lighting_texture: lighting_texture,
        lighting_view,
        _irradiance_volume_texture: irradiance_volume_texture,
        irradiance_volume_view,
        sampler,
        uniform_buffer,
        irradiance_volume_uniform_buffer,
    }
}
