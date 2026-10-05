//! World bindings.
use crate::renderer::{
    detail_texture_for_source, DrawBatch, GpuImage, HashMap, ReflectionProbeGpuSet, UniformSlot,
    WorldGpu,
};

#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn create_fast_world_bind_group(
    device: &wgpu::Device,
    surface_layout: &wgpu::BindGroupLayout,
    white: &GpuImage,
    missing: &GpuImage,
    repeat_sampler: &wgpu::Sampler,
    clamp_sampler: &wgpu::Sampler,
    lightmap_sampler: &wgpu::Sampler,
    sky_sampler: &wgpu::Sampler,
    textures: &[GpuImage],
    lightmaps: &[GpuImage],
    texture_clamp: &[bool],
    fallback_skybox: Option<[usize; 6]>,
    footprint_marks: &[&GpuImage; 2],
    source: &DrawBatch,
    material_buffer: &UniformSlot,
    surface_deformation_buffer: &wgpu::Buffer,
    deformation_field_views: [&wgpu::TextureView; 2],
    deformation_field_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let explicit_lightmap = source
        .lightmap
        .and_then(|index| lightmaps.get(index))
        .unwrap_or(white);
    let (base, base_sampler) = if source.texture_is_lightmap {
        (explicit_lightmap, lightmap_sampler)
    } else if source.texture_is_white {
        (white, clamp_sampler)
    } else {
        let base = source
            .texture
            .and_then(|index| textures.get(index))
            .unwrap_or(missing);
        let sampler = source
            .texture
            .and_then(|index| texture_clamp.get(index))
            .map(|clamp| {
                if *clamp {
                    clamp_sampler
                } else {
                    repeat_sampler
                }
            })
            .unwrap_or(clamp_sampler);
        (base, sampler)
    };
    let sky_faces: [&GpuImage; 6] = std::array::from_fn(|face| {
        source
            .skybox
            .or(fallback_skybox)
            .and_then(|indices| textures.get(indices[face]))
            .unwrap_or(missing)
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA fast BSP surface bind group"),
        layout: surface_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&base.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(base_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&explicit_lightmap.view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(lightmap_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: material_buffer.binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&sky_faces[0].view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&sky_faces[1].view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&sky_faces[2].view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&sky_faces[3].view),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(&sky_faces[4].view),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&sky_faces[5].view),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::Sampler(sky_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 21,
                resource: surface_deformation_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 22,
                resource: wgpu::BindingResource::TextureView(&footprint_marks[0].view),
            },
            wgpu::BindGroupEntry {
                binding: 23,
                resource: wgpu::BindingResource::TextureView(&footprint_marks[1].view),
            },
            wgpu::BindGroupEntry {
                binding: 24,
                resource: wgpu::BindingResource::Sampler(clamp_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 25,
                resource: wgpu::BindingResource::TextureView(deformation_field_views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 26,
                resource: wgpu::BindingResource::TextureView(deformation_field_views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 27,
                resource: wgpu::BindingResource::Sampler(deformation_field_sampler),
            },
        ],
    })
}

#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn create_world_bind_group(
    device: &wgpu::Device,
    surface_layout: &wgpu::BindGroupLayout,
    white: &GpuImage,
    missing: &GpuImage,
    flat_normal: &GpuImage,
    repeat_sampler: &wgpu::Sampler,
    clamp_sampler: &wgpu::Sampler,
    pbr_repeat_sampler: &wgpu::Sampler,
    pbr_clamp_sampler: &wgpu::Sampler,
    lightmap_sampler: &wgpu::Sampler,
    sky_sampler: &wgpu::Sampler,
    detail_texture: &GpuImage,
    textures: &[GpuImage],
    lightmaps: &[GpuImage],
    deluxemaps: &[GpuImage],
    texture_clamp: &[bool],
    fallback_skybox: Option<[usize; 6]>,
    footprint_marks: &[&GpuImage; 2],
    reflection_probes: &ReflectionProbeGpuSet,
    source: &DrawBatch,
    material_buffer: &UniformSlot,
    fog_buffer: &UniformSlot,
    surface_deformation_buffer: &wgpu::Buffer,
    deformation_field_views: [&wgpu::TextureView; 2],
    deformation_field_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let explicit_lightmap = source
        .lightmap
        .and_then(|index| lightmaps.get(index))
        .unwrap_or(white);
    let deluxemap = source
        .lightmap
        .and_then(|index| deluxemaps.get(index))
        .unwrap_or_else(|| &deluxemaps[0]);
    let (base, base_sampler) = if source.texture_is_lightmap {
        (explicit_lightmap, lightmap_sampler)
    } else if source.texture_is_white {
        (white, clamp_sampler)
    } else {
        let base = source
            .texture
            .and_then(|index| textures.get(index))
            .unwrap_or(missing);
        let sampler = source
            .texture
            .and_then(|index| texture_clamp.get(index))
            .map(|clamp| {
                if *clamp {
                    clamp_sampler
                } else {
                    repeat_sampler
                }
            })
            .unwrap_or(clamp_sampler);
        (base, sampler)
    };
    let companion_clamp = [
        source.normal_texture,
        source.roughness_texture,
        source.height_texture,
        source.metallic_texture,
        source.specular_texture,
    ]
    .into_iter()
    .flatten()
    .find_map(|index| texture_clamp.get(index).copied())
    .unwrap_or_else(|| {
        source
            .texture
            .and_then(|index| texture_clamp.get(index))
            .copied()
            .unwrap_or(true)
    });
    let pbr_sampler = if companion_clamp {
        pbr_clamp_sampler
    } else {
        pbr_repeat_sampler
    };
    let normal = source
        .normal_texture
        .and_then(|index| textures.get(index))
        .unwrap_or(flat_normal);
    let roughness = source
        .roughness_texture
        .and_then(|index| textures.get(index))
        .unwrap_or(white);
    let height = source
        .height_texture
        .and_then(|index| textures.get(index))
        .unwrap_or(white);
    let metallic = source
        .metallic_texture
        .and_then(|index| textures.get(index))
        .unwrap_or(white);
    let specular = source
        .specular_texture
        .and_then(|index| textures.get(index))
        .unwrap_or(white);
    let emissive = source
        .emissive_texture
        .and_then(|index| textures.get(index))
        .unwrap_or(white);
    let reflection_probe = source
        .reflection_probe
        .and_then(|index| reflection_probes.probes.get(index))
        .unwrap_or(&reflection_probes.probes[0]);
    let sky_faces: [&GpuImage; 6] = std::array::from_fn(|face| {
        source
            .skybox
            .or(fallback_skybox)
            .and_then(|indices| textures.get(indices[face]))
            .unwrap_or(missing)
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("BSP surface bind group"),
        layout: surface_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&base.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(base_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&explicit_lightmap.view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(lightmap_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: material_buffer.binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&sky_faces[0].view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&sky_faces[1].view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&sky_faces[2].view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&sky_faces[3].view),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(&sky_faces[4].view),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&sky_faces[5].view),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::Sampler(sky_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: wgpu::BindingResource::TextureView(&normal.view),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: wgpu::BindingResource::TextureView(&roughness.view),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: wgpu::BindingResource::TextureView(&height.view),
            },
            wgpu::BindGroupEntry {
                binding: 15,
                resource: fog_buffer.binding(),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: wgpu::BindingResource::TextureView(&metallic.view),
            },
            wgpu::BindGroupEntry {
                binding: 17,
                resource: wgpu::BindingResource::TextureView(&specular.view),
            },
            wgpu::BindGroupEntry {
                binding: 18,
                resource: wgpu::BindingResource::TextureView(&emissive.view),
            },
            wgpu::BindGroupEntry {
                binding: 19,
                resource: wgpu::BindingResource::TextureView(&reflection_probe.view),
            },
            wgpu::BindGroupEntry {
                binding: 20,
                resource: wgpu::BindingResource::Sampler(&reflection_probes.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 21,
                resource: surface_deformation_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 22,
                resource: wgpu::BindingResource::TextureView(&footprint_marks[0].view),
            },
            wgpu::BindGroupEntry {
                binding: 23,
                resource: wgpu::BindingResource::TextureView(&footprint_marks[1].view),
            },
            wgpu::BindGroupEntry {
                binding: 24,
                resource: wgpu::BindingResource::Sampler(clamp_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 25,
                resource: wgpu::BindingResource::TextureView(deformation_field_views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 26,
                resource: wgpu::BindingResource::TextureView(deformation_field_views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 27,
                resource: wgpu::BindingResource::Sampler(deformation_field_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 28,
                resource: wgpu::BindingResource::Sampler(pbr_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 29,
                resource: wgpu::BindingResource::TextureView(&deluxemap.view),
            },
            wgpu::BindGroupEntry {
                binding: 30,
                resource: wgpu::BindingResource::TextureView(&detail_texture.view),
            },
            wgpu::BindGroupEntry {
                binding: 31,
                resource: wgpu::BindingResource::Sampler(repeat_sampler),
            },
        ],
    })
}

/// AUTO 4 batches share FULL's GPU resources: the first `auto4_variant_base`
/// entries alias FULL 1:1 and each recipe uses its representative piece's
/// bind state. Copy handles instead of creating a bind group per recipe.
pub(in crate::renderer) fn sync_auto4_bind_groups(world: &mut WorldGpu) {
    let WorldGpu {
        full_batches,
        auto4_batches,
        auto4_variant_members,
        auto4_variant_base,
        ..
    } = world;
    for (index, batch) in auto4_batches.iter_mut().enumerate() {
        let source = match index.checked_sub(*auto4_variant_base) {
            None => Some(index),
            Some(variant) => auto4_variant_members
                .get(variant)
                .and_then(|members| members.first().copied()),
        };
        if let Some(full) = source.and_then(|source| full_batches.get(source)) {
            batch.bind_group = full.bind_group.clone();
            batch.fast_bind_group = full.fast_bind_group.clone();
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn rebuild_world_bind_groups(
    device: &wgpu::Device,
    surface_layout: &wgpu::BindGroupLayout,
    fast_surface_layout: &wgpu::BindGroupLayout,
    white: &GpuImage,
    missing: &GpuImage,
    flat_normal: &GpuImage,
    repeat_sampler: &wgpu::Sampler,
    clamp_sampler: &wgpu::Sampler,
    pbr_repeat_sampler: &wgpu::Sampler,
    pbr_clamp_sampler: &wgpu::Sampler,
    lightmap_sampler: &wgpu::Sampler,
    sky_sampler: &wgpu::Sampler,
    detail_texture: &GpuImage,
    detail_texture_auto: bool,
    detail_auto_textures: &HashMap<String, GpuImage>,
    surface_deformation_buffer: &wgpu::Buffer,
    deformation_field_views: [&wgpu::TextureView; 2],
    deformation_field_sampler: &wgpu::Sampler,
    world: &mut WorldGpu,
) {
    let WorldGpu {
        coarse_batches,
        full_batches,
        textures,
        lightmaps,
        deluxemaps,
        texture_clamp,
        detail_texture_by_base,
        primary_skybox,
        reflection_probes,
        footprint_mark_textures,
        ..
    } = world;
    let footprint_marks: [&GpuImage; 2] = std::array::from_fn(|slot| {
        footprint_mark_textures[slot]
            .and_then(|index| textures.get(index))
            .unwrap_or(white)
    });
    for batch in coarse_batches.iter_mut().chain(full_batches.iter_mut()) {
        let selected_detail_texture = detail_texture_for_source(
            &batch.source,
            detail_texture,
            detail_texture_auto,
            detail_auto_textures,
            detail_texture_by_base,
        );
        batch.bind_group = create_world_bind_group(
            device,
            surface_layout,
            white,
            missing,
            flat_normal,
            repeat_sampler,
            clamp_sampler,
            pbr_repeat_sampler,
            pbr_clamp_sampler,
            lightmap_sampler,
            sky_sampler,
            selected_detail_texture,
            textures,
            lightmaps,
            deluxemaps,
            texture_clamp,
            *primary_skybox,
            &footprint_marks,
            reflection_probes,
            &batch.source,
            &batch._material_buffer,
            &batch._fog_buffer,
            surface_deformation_buffer,
            deformation_field_views,
            deformation_field_sampler,
        );
        batch.fast_bind_group = create_fast_world_bind_group(
            device,
            fast_surface_layout,
            white,
            missing,
            repeat_sampler,
            clamp_sampler,
            lightmap_sampler,
            sky_sampler,
            textures,
            lightmaps,
            texture_clamp,
            *primary_skybox,
            &footprint_marks,
            &batch.source,
            &batch._material_buffer,
            surface_deformation_buffer,
            deformation_field_views,
            deformation_field_sampler,
        );
    }
    sync_auto4_bind_groups(world);
}
