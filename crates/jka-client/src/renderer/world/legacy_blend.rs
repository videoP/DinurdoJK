//! Ordered authored BSP stages blend in display space, as in TaystJK.
//! Keep material stages and their fog redraws in one attachment scope.
use crate::renderer::{BlendFactor, BlendMode, PipelineKey, WorldGpu};
use std::collections::BTreeMap;

pub(in crate::renderer) fn additive_pipeline(key: PipelineKey) -> bool {
    matches!(
        key.blend,
        BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::One)
            | BlendMode::Custom(BlendFactor::One, BlendFactor::One)
    ) && key.class == crate::renderer::DrawClass::Transparent
}

/// Color attachment format of an encoded world pass: the UNORM alias of an
/// sRGB scene, otherwise the float encoded-blend target.
pub(in crate::renderer) fn encoded_format(scene: wgpu::TextureFormat) -> wgpu::TextureFormat {
    if scene.is_srgb() { scene.remove_srgb_suffix() } else { wgpu::TextureFormat::Rgba16Float }
}

pub(in crate::renderer) fn pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    keys: impl Iterator<Item = PipelineKey>,
    variant: crate::renderer::WorldShaderVariantKey,
) -> BTreeMap<PipelineKey, wgpu::RenderPipeline> {
    let encoded_format = encoded_format(format);
    let keys = keys.collect::<std::collections::BTreeSet<_>>();
    if !keys.iter().any(|key| additive_pipeline(*key)) {
        return BTreeMap::new();
    }
    keys.into_iter().filter(|key| key.class != crate::renderer::DrawClass::Sky)
        .into_iter()
        .map(|key| {
            let pipeline = crate::renderer::create_world_pipeline_specialized(
                device,
                layout,
                shader,
                encoded_format,
                key,
                samples,
                variant.legacy_fog,
                false,
                Some(variant),
                false,
                None,
                None,
                false,
                false,
                true,
            );
            (key, pipeline)
        })
        .collect()
}

pub(in crate::renderer) fn fog_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    keys: impl Iterator<Item = PipelineKey>,
    variant: crate::renderer::WorldShaderVariantKey,
) -> BTreeMap<(PipelineKey, bool), wgpu::RenderPipeline> {
    let keys = keys.collect::<std::collections::BTreeSet<_>>();
    if !variant.legacy_fog || !keys.iter().any(|key| additive_pipeline(*key)) {
        return BTreeMap::new();
    }
    let encoded_format = encoded_format(format);
    let mut compiled = BTreeMap::new();
    keys.into_iter().filter(|key| key.class != crate::renderer::DrawClass::Sky)
        .flat_map(|key| [false, true].map(move |depth| (key, depth)))
        .map(|(original, depth)| {
            let key = super::pipelines::legacy_fog_pass_pipeline_key(original, depth);
            let pipeline = compiled.entry(key).or_insert_with(||
                crate::renderer::create_world_pipeline_specialized(
                    device, layout, shader, encoded_format, key, samples,
                    true, true, Some(variant), false, None, None, false, false, true,
                ));
            ((original, depth), pipeline.clone())
        }).collect()
}

/// Resume the existing ordered command stream after changing attachment views.
/// Restore all world bindings because ending a render pass resets GPU state.
pub(in crate::renderer) fn resume<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    resolve: Option<&wgpu::TextureView>,
    depth: &wgpu::TextureView,
    world: &WorldGpu,
    camera: &wgpu::BindGroup,
    lighting: &wgpu::BindGroup,
    shadow: &wgpu::BindGroup,
    planar: &wgpu::BindGroup,
    optics: Option<&wgpu::BindGroup>,
    ocean: Option<&wgpu::BindGroup>,
) -> wgpu::RenderPass<'a> {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("JKA ordered world blend continuation"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: resolve,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        ..Default::default()
    });
    pass.set_bind_group(0, camera, &[]);
    pass.set_bind_group(2, lighting, &[]);
    pass.set_bind_group(3, shadow, &[]);
    pass.set_bind_group(4, planar, &[]);
    if let Some(optics) = optics {
        pass.set_bind_group(5, optics, &[]);
    }
    if let Some(ocean) = ocean {
        pass.set_bind_group(6, ocean, &[]);
    }
    pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
    pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
    pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    pass
}
