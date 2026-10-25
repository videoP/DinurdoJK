//! World bindings.
use crate::renderer::{
    detail_texture_for_source, DrawBatch, GpuImage, HashMap, ReflectionProbeGpuSet, UniformSlot,
    WorldGpu, WorldAnimationFrame, WorldBatchAnimation,
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
    local_lightmaps: &[GpuImage],
    texture_clamp: &[bool],
    fallback_skybox: Option<[usize; 6]>,
    footprint_marks: &[&GpuImage; 2],
    source: &DrawBatch,
    material_buffer: &UniformSlot,
    additive_uniforms: &[UniformSlot; 2],
    surface_deformation_buffer: &wgpu::Buffer,
    deformation_field_views: [&wgpu::TextureView; 2],
    deformation_field_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let fallback_local = white._texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
    let local_view = source.lightmap.and_then(|i| local_lightmaps.get(i)).map_or(&fallback_local, |i| &i.view);
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
    // This authored overlay is not a sky texture. Bind its own sampler
    // so GL_CLAMP_TO_EDGE/GL_REPEAT remains correct even if the rock differs.
    let overlay_sampler = source.legacy_srgb_overlay
        .and_then(|index| texture_clamp.get(index).copied())
        .map(|clamp| if clamp { clamp_sampler } else { repeat_sampler })
        .unwrap_or(sky_sampler);
    let sky_faces: [&GpuImage; 6] = std::array::from_fn(|face| {
        if face < source.additive_companions.len() {
            return source.additive_companions[face].texture
                .and_then(|i| textures.get(i)).unwrap_or(missing);
        }
        if face == 0 {
            if let Some(overlay) = source.legacy_srgb_overlay.and_then(|index| textures.get(index)) {
                return overlay;
            }
        }
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
                binding: 34,
                resource: wgpu::BindingResource::TextureView(local_view),
            },
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
                resource: wgpu::BindingResource::Sampler(overlay_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 32,
                resource: additive_uniforms[0].binding(),
            },
            wgpu::BindGroupEntry {
                binding: 33,
                resource: additive_uniforms[1].binding(),
            },
            wgpu::BindGroupEntry {
                binding: 31,
                resource: wgpu::BindingResource::Sampler(repeat_sampler),
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
    local_lightmaps: &[GpuImage],
    deluxemaps: &[GpuImage],
    texture_clamp: &[bool],
    fallback_skybox: Option<[usize; 6]>,
    footprint_marks: &[&GpuImage; 2],
    reflection_probes: &ReflectionProbeGpuSet,
    source: &DrawBatch,
    material_buffer: &UniformSlot,
    additive_uniforms: &[UniformSlot; 2],
    fog_buffer: &UniformSlot,
    surface_deformation_buffer: &wgpu::Buffer,
    deformation_field_views: [&wgpu::TextureView; 2],
    deformation_field_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let fallback_local = white._texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
    let local_view = source.lightmap.and_then(|i| local_lightmaps.get(i)).map_or(&fallback_local, |i| &i.view);
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
    // This authored overlay is not a sky texture. Bind its own sampler
    // so GL_CLAMP_TO_EDGE/GL_REPEAT remains correct even if the rock differs.
    let overlay_sampler = source.legacy_srgb_overlay
        .and_then(|index| texture_clamp.get(index).copied())
        .map(|clamp| if clamp { clamp_sampler } else { repeat_sampler })
        .unwrap_or(sky_sampler);
    let sky_faces: [&GpuImage; 6] = std::array::from_fn(|face| {
        if face < source.additive_companions.len() {
            return source.additive_companions[face].texture
                .and_then(|i| textures.get(i)).unwrap_or(missing);
        }
        if face == 0 {
            if let Some(overlay) = source.legacy_srgb_overlay.and_then(|index| textures.get(index)) {
                return overlay;
            }
        }
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
                binding: 34,
                resource: wgpu::BindingResource::TextureView(local_view),
            },
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
                resource: wgpu::BindingResource::Sampler(overlay_sampler),
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
                binding: 32,
                resource: additive_uniforms[0].binding(),
            },
            wgpu::BindGroupEntry {
                binding: 33,
                resource: additive_uniforms[1].binding(),
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
/// The FULL batch whose bind groups an AUTO batch mirrors.
fn auto4_bind_source(index: usize, variant_base: usize, variant_members: &[Vec<usize>]) -> Option<usize> {
    match index.checked_sub(variant_base) {
        None => Some(index),
        Some(variant) => variant_members.get(variant).and_then(|members| members.first().copied()),
    }
}

pub(in crate::renderer) fn sync_auto4_bind_groups(world: &mut WorldGpu) {
    let WorldGpu {
        full_batches,
        auto4_batches,
        auto4_variant_members,
        auto4_variant_base,
        ..
    } = world;
    for (index, batch) in auto4_batches.iter_mut().enumerate() {
        let source = auto4_bind_source(index, *auto4_variant_base, auto4_variant_members);
        if let Some(full) = source.and_then(|source| full_batches.get(source)) {
            batch.bind_group = full.bind_group.clone();
            batch.fast_bind_group = full.fast_bind_group.clone();
            batch.authored_glow_bind_group = full.authored_glow_bind_group.clone();
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
        baked_sun_cache,
        static_sun,
        deluxemaps,
        texture_clamp,
        detail_texture_by_base,
        primary_skybox,
        reflection_probes,
        footprint_mark_textures,
        ..
    } = world;
    let original_lightmaps=baked_sun_cache.as_ref().filter(|c|c.active).map(|c|c.original.as_slice());
    let footprint_marks: [&GpuImage; 2] = std::array::from_fn(|slot| {
        footprint_mark_textures[slot]
            .and_then(|index| textures.get(index))
            .unwrap_or(white)
    });
    for batch in coarse_batches.iter_mut().chain(full_batches.iter_mut()) {
        let local_lightmaps = if !batch.is_inline_entity && baked_sun_cache.as_ref().is_some_and(|c| c.active) && static_sun.local_ready() { static_sun.local_flux.as_slice() } else { &[] };
        let lightmaps = if batch.is_inline_entity { original_lightmaps.unwrap_or(lightmaps.as_slice()) } else { lightmaps.as_slice() };
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
            lightmaps, local_lightmaps,
            deluxemaps,
            texture_clamp,
            *primary_skybox,
            &footprint_marks,
            reflection_probes,
            &batch.source,
            &batch._material_buffer,
            &batch.additive_uniforms,
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
            lightmaps, local_lightmaps,
            texture_clamp,
            *primary_skybox,
            &footprint_marks,
            &batch.source,
            &batch._material_buffer,
            &batch.additive_uniforms,
            surface_deformation_buffer,
            deformation_field_views,
            deformation_field_sampler,
        );
        batch.authored_glow_bind_group = batch._authored_glow_fog_buffer.as_ref().map(|glow_fog| {
            create_world_bind_group(
                device, surface_layout, white, missing, flat_normal,
                repeat_sampler, clamp_sampler, pbr_repeat_sampler, pbr_clamp_sampler,
                lightmap_sampler, sky_sampler, selected_detail_texture,
                textures, lightmaps, local_lightmaps, deluxemaps, texture_clamp, *primary_skybox,
                &footprint_marks, reflection_probes, &batch.source,
                &batch._material_buffer, &batch.additive_uniforms, glow_fog, surface_deformation_buffer,
                deformation_field_views, deformation_field_sampler,
            )
        });
        batch.animation = build_world_animation(
            &batch.source, &batch.bind_group, &batch.fast_bind_group,
            &batch.authored_glow_bind_group,
            |alternate| WorldAnimationFrame {
                full: create_world_bind_group(
                    device, surface_layout, white, missing, flat_normal,
                    repeat_sampler, clamp_sampler, pbr_repeat_sampler, pbr_clamp_sampler,
                    lightmap_sampler, sky_sampler, selected_detail_texture,
                    textures, lightmaps, local_lightmaps, deluxemaps, texture_clamp, *primary_skybox,
                    &footprint_marks, reflection_probes, alternate,
                    &batch._material_buffer, &batch.additive_uniforms, &batch._fog_buffer, surface_deformation_buffer,
                    deformation_field_views, deformation_field_sampler,
                ),
                fast: create_fast_world_bind_group(
                    device, fast_surface_layout, white, missing, repeat_sampler, clamp_sampler,
                    lightmap_sampler, sky_sampler, textures, lightmaps, local_lightmaps, texture_clamp,
                    *primary_skybox, &footprint_marks, alternate, &batch._material_buffer,
            &batch.additive_uniforms,
                    surface_deformation_buffer, deformation_field_views, deformation_field_sampler,
                ),
                glow: batch._authored_glow_fog_buffer.as_ref().map(|glow_fog| {
                    create_world_bind_group(
                        device, surface_layout, white, missing, flat_normal,
                        repeat_sampler, clamp_sampler, pbr_repeat_sampler, pbr_clamp_sampler,
                        lightmap_sampler, sky_sampler, selected_detail_texture,
                        textures, lightmaps, local_lightmaps, deluxemaps, texture_clamp, *primary_skybox,
                        &footprint_marks, reflection_probes, alternate,
                        &batch._material_buffer, &batch.additive_uniforms, glow_fog, surface_deformation_buffer,
                        deformation_field_views, deformation_field_sampler,
                    )
                }),
            },
        );
    }
    sync_auto4_bind_groups(world);
}

/// Cache frame-specific bind groups at upload/rebuild time, never during draw.
/// Factory is skipped entirely for static stages and the first animation image
/// reuses the ordinary stage's already allocated resource group.
pub(in crate::renderer) fn build_world_animation(
    source: &DrawBatch,
    first_full: &wgpu::BindGroup,
    first_fast: &wgpu::BindGroup,
    first_glow: &Option<wgpu::BindGroup>,
    mut make: impl FnMut(&DrawBatch) -> WorldAnimationFrame,
) -> Option<WorldBatchAnimation> {
    let animation = source.animation.as_ref()?;
    if animation.frames.len() <= 1 {
        return None;
    }
    let mut frames = Vec::with_capacity(animation.frames.len());
    frames.push(WorldAnimationFrame {
        full: first_full.clone(),
        fast: first_fast.clone(),
        glow: first_glow.clone(),
    });
    for &texture in animation.frames.iter().skip(1) {
        let mut alternate = source.clone();
        alternate.texture = Some(texture);
        alternate.animation = None;
        frames.push(make(&alternate));
    }
    Some(WorldBatchAnimation { frames, selected: 0 })
}

/// TaystJK uses shaderTime = refdef.floatTime - shader.timeOffset.
/// Scan only world batches; static batches have no animation and do no work.
pub(in crate::renderer) fn advance_world_animations(world: &mut WorldGpu, shader_time: f32) {
    let mut changed = false;
    let animated = world.animated_batches.get_or_insert_with(|| {
        let coarse = world.coarse_batches.iter().enumerate().map(|(i, b)| (false, i, b));
        let full = world.full_batches.iter().enumerate().map(|(i, b)| (true, i, b));
        let batches = coarse.chain(full)
            .filter(|(_, _, batch)| batch.source.animation.is_some())
            .map(|(set, i, _)| (set, i))
            .collect();
        let auto4 = (0..world.auto4_batches.len())
            .filter_map(|index| {
                let source = auto4_bind_source(index, world.auto4_variant_base, &world.auto4_variant_members)?;
                world.full_batches.get(source)?.source.animation.is_some().then_some((index, source))
            })
            .collect();
        super::types::AnimatedBatches { batches, auto4 }
    });
    for &(full, index) in &animated.batches {
        let batch = if full { &mut world.full_batches[index] } else { &mut world.coarse_batches[index] };
        let Some(animation) = batch.animation.as_mut() else { continue };
        let Some(definition) = batch.source.animation.as_ref() else { continue };
        let time = jka_assets::shader::shader_time(shader_time, batch.source.shader_time_offset, batch.source.shader_clamp_time);
        let selected = definition.frame_index(time);
        if animation.selected == selected { continue; }
        let frame = &animation.frames[selected];
        batch.bind_group = frame.full.clone();
        batch.fast_bind_group = frame.fast.clone();
        batch.authored_glow_bind_group = frame.glow.clone();
        animation.selected = selected;
        changed = true;
    }
    if changed {
        for &(index, source) in &animated.auto4 {
            let full = &world.full_batches[source];
            let batch = &mut world.auto4_batches[index];
            batch.bind_group = full.bind_group.clone();
            batch.fast_bind_group = full.fast_bind_group.clone();
            batch.authored_glow_bind_group = full.authored_glow_bind_group.clone();
        }
    }
}
