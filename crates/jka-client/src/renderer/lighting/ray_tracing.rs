//! Lighting ray tracing.
use crate::renderer::{
    comparison_sampler_entry, depth_array_texture_entry, material_uniform,
    nonfiltering_sampler_entry, rt_models, rt_resolution, unfilterable_texture_entry_stages,
    uniform_entry, weather, Arc, DrawBatch, DynamicModelAlphaMode, DynamicModelSurface,
    DynamicModelVertex, DynamicShadowsMode, Ghoul2GpuVertex, GpuVertex, Hash, HashMap, HashSet,
    Hasher, InlineModelGpu, InlineModelInstance, MaterialUniform, Pod, RayTracedRigidInstance,
    RayTracedSkinnedMeshKey, RayTracedSkinnedPrepared, Renderer, ShadowResources, TextureData,
    WorldGpu, Zeroable, RT_DYNAMIC_TLAS_INSTANCE_BUDGET, RT_RIGID_BLAS_CACHE_LIMIT,
    RT_SKINNED_BLAS_CACHE_LIMIT,
};
use wgpu::util::DeviceExt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(in crate::renderer) struct RayTracedRigidMeshKey {
    pub(in crate::renderer) asset_key: Arc<str>,
    pub(in crate::renderer) surface_index: u32,
    pub(in crate::renderer) frame: u32,
    pub(in crate::renderer) non_opaque: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(in crate::renderer) struct RtAlphaVertexGpu {
    // xyz object-space position, w base-u
    pub(in crate::renderer) position_u: [f32; 4],
    // xyz object-space normal, w base-v
    pub(in crate::renderer) normal_v: [f32; 4],
    // xy lightmap uv, z vertex alpha, w unused
    pub(in crate::renderer) lightmap_alpha: [f32; 4],
}

impl From<&GpuVertex> for RtAlphaVertexGpu {
    fn from(vertex: &GpuVertex) -> Self {
        Self {
            position_u: [
                vertex.position[0],
                vertex.position[1],
                vertex.position[2],
                vertex.uv[0],
            ],
            normal_v: [
                vertex.normal[0],
                vertex.normal[1],
                vertex.normal[2],
                vertex.uv[1],
            ],
            lightmap_alpha: [
                vertex.lightmap_uv[0],
                vertex.lightmap_uv[1],
                vertex.color[3],
                0.0,
            ],
        }
    }
}

impl From<&DynamicModelVertex> for RtAlphaVertexGpu {
    fn from(vertex: &DynamicModelVertex) -> Self {
        Self {
            position_u: [
                vertex.position[0],
                vertex.position[1],
                vertex.position[2],
                vertex.uv[0],
            ],
            normal_v: [
                vertex.normal[0],
                vertex.normal[1],
                vertex.normal[2],
                vertex.uv[1],
            ],
            lightmap_alpha: [0.0, 0.0, vertex.color[3], 0.0],
        }
    }
}

impl From<&Ghoul2GpuVertex> for RtAlphaVertexGpu {
    fn from(vertex: &Ghoul2GpuVertex) -> Self {
        Self {
            position_u: [
                vertex.position[0],
                vertex.position[1],
                vertex.position[2],
                vertex.uv[0],
            ],
            normal_v: [
                vertex.normal[0],
                vertex.normal[1],
                vertex.normal[2],
                vertex.uv[1],
            ],
            lightmap_alpha: [0.0, 0.0, 1.0, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(in crate::renderer) struct RtAlphaGeometryGpu {
    // x first vertex, y first index, z material index, w reserved
    pub(in crate::renderer) values: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(in crate::renderer) struct RtAlphaTextureGpu {
    // x texel offset, y width, z height, w clamp (1) / repeat (0)
    pub(in crate::renderer) values: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct RtAlphaMaterialGpu {
    pub(in crate::renderer) header: [u32; 4],
    pub(in crate::renderer) vector_s: [f32; 4],
    pub(in crate::renderer) vector_t: [f32; 4],
    pub(in crate::renderer) mods: [[f32; 4]; 8],
    pub(in crate::renderer) color: [f32; 4],
    pub(in crate::renderer) params: [f32; 4],
    // x alpha-texture descriptor index; remaining fields reserved.
    pub(in crate::renderer) texture: [u32; 4],
}

#[derive(Clone)]
pub(in crate::renderer) struct RtAlphaTextureSource {
    pub(in crate::renderer) key: String,
    pub(in crate::renderer) width: u32,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) clamp: bool,
    pub(in crate::renderer) alpha: Vec<u8>,
}

#[derive(Clone)]
pub(in crate::renderer) struct RtAlphaMaterialSource {
    pub(in crate::renderer) uniform: MaterialUniform,
    pub(in crate::renderer) texture: RtAlphaTextureSource,
}

#[derive(Clone)]
pub(in crate::renderer) struct RtAlphaMaskSource {
    pub(in crate::renderer) vertices: Vec<RtAlphaVertexGpu>,
    pub(in crate::renderer) indices: Vec<u32>,
    pub(in crate::renderer) material: RtAlphaMaterialSource,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::renderer) struct RtAlphaAttributeRange {
    pub(in crate::renderer) first_vertex: u32,
    pub(in crate::renderer) first_index: u32,
}

pub(in crate::renderer) fn rt_alpha_texture_key(texture: Option<&TextureData>) -> String {
    let Some(texture) = texture else {
        return "<rt-alpha-white>".to_owned();
    };
    let count = (texture.width as usize).saturating_mul(texture.height as usize);
    let (width, height) = if count == 0 || texture.rgba.len() < count.saturating_mul(4) {
        (1, 1)
    } else {
        (texture.width, texture.height)
    };
    format!("{}:{}x{}:{}", texture.label, width, height, texture.clamp)
}

pub(in crate::renderer) fn rt_alpha_texture_source(
    texture: Option<&TextureData>,
) -> RtAlphaTextureSource {
    let Some(texture) = texture else {
        return RtAlphaTextureSource {
            key: "<rt-alpha-white>".to_owned(),
            width: 1,
            height: 1,
            clamp: true,
            alpha: vec![255],
        };
    };
    let texel_count = (texture.width as usize).saturating_mul(texture.height as usize);
    let mut alpha = Vec::with_capacity(texel_count);
    for rgba in texture
        .rgba
        .get(..texel_count.saturating_mul(4))
        .unwrap_or(&[])
        .chunks_exact(4)
    {
        alpha.push(rgba[3]);
    }
    let (width, height) = if alpha.len() != texel_count || texel_count == 0 {
        alpha.clear();
        alpha.push(255);
        (1, 1)
    } else {
        (texture.width.max(1), texture.height.max(1))
    };
    RtAlphaTextureSource {
        key: rt_alpha_texture_key(Some(texture)),
        width,
        height,
        clamp: texture.clamp,
        alpha,
    }
}

pub(in crate::renderer) fn rt_alpha_material_from_draw(
    batch: &DrawBatch,
    texture: Option<&TextureData>,
) -> RtAlphaMaterialSource {
    RtAlphaMaterialSource {
        // Detail textures affect RGB only; candidate rejection uses base alpha.
        uniform: material_uniform(batch, false),
        texture: rt_alpha_texture_source(texture),
    }
}

pub(in crate::renderer) fn rt_alpha_dynamic_material(texture_index: u32) -> RtAlphaMaterialGpu {
    let mut material = RtAlphaMaterialGpu::zeroed();
    material.color = [1.0; 4];
    material.params[0] = 0.5;
    material.texture[0] = texture_index;
    material
}

pub(in crate::renderer) fn rt_alpha_mask_source_from_gpu_vertices(
    vertices: &[GpuVertex],
    batch: &DrawBatch,
    texture: Option<&TextureData>,
) -> Option<RtAlphaMaskSource> {
    let range = batch.vertices.start as usize..batch.vertices.end as usize;
    let source = vertices.get(range)?;
    if source.len() < 3 || source.len() % 3 != 0 {
        return None;
    }
    Some(RtAlphaMaskSource {
        vertices: source.iter().map(RtAlphaVertexGpu::from).collect(),
        indices: (0..u32::try_from(source.len()).ok()?).collect(),
        material: rt_alpha_material_from_draw(batch, texture),
    })
}

pub(in crate::renderer) fn rt_alpha_material_gpu(
    source: &RtAlphaMaterialSource,
    texture_index: u32,
) -> RtAlphaMaterialGpu {
    let material = source.uniform;
    RtAlphaMaterialGpu {
        header: material.header,
        vector_s: material.vector_s,
        vector_t: material.vector_t,
        mods: material.mods,
        color: material.color,
        params: material.params,
        texture: [texture_index, 0, 0, 0],
    }
}

/// One immutable object-space triangle BLAS. Static inline BSP meshes and MD3
/// frame/surfaces share this representation; only their TLAS transform changes.
pub(in crate::renderer) struct RayTracedTriangleBlas {
    pub(in crate::renderer) _vertex_buffer: wgpu::Buffer,
    pub(in crate::renderer) _index_buffer: wgpu::Buffer,
    pub(in crate::renderer) size: wgpu::BlasTriangleGeometrySizeDescriptor,
    pub(in crate::renderer) blas: wgpu::Blas,
    pub(in crate::renderer) triangle_count: u32,
}

pub(in crate::renderer) struct RayTracedMaskBlas {
    pub(in crate::renderer) sizes: Vec<wgpu::BlasTriangleGeometrySizeDescriptor>,
    pub(in crate::renderer) ranges: Vec<RtAlphaAttributeRange>,
    pub(in crate::renderer) blas: wgpu::Blas,
    pub(in crate::renderer) triangle_count: u32,
    pub(in crate::renderer) geometry_base: u32,
}

/// Persistent BLAS object for one deforming Ghoul2 entity/surface. The final
/// skinned vertices live in DynamicModelRenderer's shared RT buffer and are
/// rebuilt into this BLAS each RT frame; world placement is already baked into
/// those vertices, so the TLAS instance remains identity.
pub(in crate::renderer) struct RayTracedSkinnedBlas {
    pub(in crate::renderer) size: wgpu::BlasTriangleGeometrySizeDescriptor,
    pub(in crate::renderer) blas: wgpu::Blas,
}

/// Hardware ray-query resources for the BSP sun-shadow visibility path.
///
/// This is deliberately separate from the normal cascaded-shadow bind group and
/// pipeline layouts.  The ordinary raster shaders/layouts remain byte-for-byte
/// unchanged until Ray Traced Shadows is selected, preserving the renderer's
/// lazy-pipeline/zero-overhead-off principle.
pub(in crate::renderer) struct RayTracedShadowResources {
    pub(in crate::renderer) sun_shadow_history: rt_resolution::RtSunShadowHistory,
    pub(in crate::renderer) model_receivers: Option<rt_models::RtModelReceivers>,
    pub(in crate::renderer) receiver_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) world_pipeline_layout: wgpu::PipelineLayout,
    pub(in crate::renderer) world_pipeline_layout_lean: wgpu::PipelineLayout,
    pub(in crate::renderer) world_shader: wgpu::ShaderModule,
    pub(in crate::renderer) world_shader_lean: wgpu::ShaderModule,
    pub(in crate::renderer) receiver_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) _blas: wgpu::Blas,
    pub(in crate::renderer) static_mask_blas: Option<RayTracedMaskBlas>,
    pub(in crate::renderer) _tlas: wgpu::Tlas,
    /// Fixed slot-per-inline-model opaque BLASes plus an optional non-opaque
    /// alpha-tested BLAS in the paired TLAS slot.
    pub(in crate::renderer) inline_blas: Vec<Option<RayTracedTriangleBlas>>,
    pub(in crate::renderer) inline_mask_blas: Vec<Option<RayTracedMaskBlas>>,
    pub(in crate::renderer) rigid_blas_cache: HashMap<RayTracedRigidMeshKey, RayTracedTriangleBlas>,
    pub(in crate::renderer) skinned_blas_cache:
        HashMap<RayTracedSkinnedMeshKey, RayTracedSkinnedBlas>,
    pub(in crate::renderer) rigid_alpha_attributes:
        HashMap<RayTracedRigidMeshKey, RtAlphaAttributeRange>,
    pub(in crate::renderer) skinned_alpha_attributes: HashMap<Arc<str>, RtAlphaAttributeRange>,
    pub(in crate::renderer) alpha_vertices: Vec<RtAlphaVertexGpu>,
    pub(in crate::renderer) alpha_indices: Vec<u32>,
    pub(in crate::renderer) alpha_geometries: Vec<RtAlphaGeometryGpu>,
    pub(in crate::renderer) alpha_materials: Vec<RtAlphaMaterialGpu>,
    pub(in crate::renderer) alpha_textures: Vec<RtAlphaTextureGpu>,
    pub(in crate::renderer) alpha_texels: Vec<u32>,
    pub(in crate::renderer) alpha_texture_cache: HashMap<String, u32>,
    pub(in crate::renderer) alpha_vertex_buffer: wgpu::Buffer,
    pub(in crate::renderer) alpha_index_buffer: wgpu::Buffer,
    pub(in crate::renderer) alpha_geometry_buffer: wgpu::Buffer,
    pub(in crate::renderer) alpha_material_buffer: wgpu::Buffer,
    pub(in crate::renderer) alpha_texture_buffer: wgpu::Buffer,
    pub(in crate::renderer) alpha_texel_buffer: wgpu::Buffer,
    pub(in crate::renderer) alpha_dynamic_geometry_base: u32,
    pub(in crate::renderer) alpha_dynamic_material_base: u32,
    pub(in crate::renderer) alpha_buffers_generation: u64,
    pub(in crate::renderer) inline_slot_count: usize,
    pub(in crate::renderer) dynamic_instance_capacity: usize,
    pub(in crate::renderer) last_scene_signature: u64,
    pub(in crate::renderer) last_dynamic_instance_count: usize,
    pub(in crate::renderer) rigid_cache_limit_warned: bool,
    pub(in crate::renderer) skinned_cache_limit_warned: bool,
    pub(in crate::renderer) geometry_count: usize,
    pub(in crate::renderer) triangle_count: u64,
    pub(in crate::renderer) masked_geometry_count: usize,
    pub(in crate::renderer) masked_triangle_count: u64,
    pub(in crate::renderer) active_masked_rigid: usize,
    pub(in crate::renderer) active_masked_skinned: usize,
    pub(in crate::renderer) active_rigid_triangles: u64,
    pub(in crate::renderer) active_skinned_triangles: u64,
}

pub(in crate::renderer) fn rt_alpha_append_texture(
    source: &RtAlphaTextureSource,
    textures: &mut Vec<RtAlphaTextureGpu>,
    texels: &mut Vec<u32>,
    cache: &mut HashMap<String, u32>,
) -> u32 {
    if let Some(&index) = cache.get(&source.key) {
        return index;
    }
    let index = u32::try_from(textures.len()).unwrap_or(u32::MAX);
    let offset = u32::try_from(texels.len()).unwrap_or(u32::MAX);
    for chunk in source.alpha.chunks(4) {
        let mut packed = 0u32;
        for (lane, alpha) in chunk.iter().copied().enumerate() {
            packed |= u32::from(alpha) << (lane * 8);
        }
        texels.push(packed);
    }
    textures.push(RtAlphaTextureGpu {
        // x is the packed-u32 base. Four alpha texels occupy each word.
        values: [
            offset,
            source.width.max(1),
            source.height.max(1),
            u32::from(source.clamp),
        ],
    });
    cache.insert(source.key.clone(), index);
    index
}

pub(in crate::renderer) fn rt_alpha_ensure_dynamic_texture(
    texture: Option<&TextureData>,
    textures: &mut Vec<RtAlphaTextureGpu>,
    texels: &mut Vec<u32>,
    cache: &mut HashMap<String, u32>,
) -> u32 {
    // Check metadata before touching pixels: animated casters reuse this table
    // every frame and often share a texture across many entity instances.
    if let Some(&index) = cache.get(&rt_alpha_texture_key(texture)) {
        return index;
    }
    rt_alpha_append_texture(&rt_alpha_texture_source(texture), textures, texels, cache)
}

pub(in crate::renderer) fn rt_alpha_append_material(
    source: &RtAlphaMaterialSource,
    materials: &mut Vec<RtAlphaMaterialGpu>,
    textures: &mut Vec<RtAlphaTextureGpu>,
    texels: &mut Vec<u32>,
    cache: &mut HashMap<String, u32>,
) -> u32 {
    let texture = rt_alpha_append_texture(&source.texture, textures, texels, cache);
    let index = u32::try_from(materials.len()).unwrap_or(u32::MAX);
    materials.push(rt_alpha_material_gpu(source, texture));
    index
}

pub(in crate::renderer) fn rt_alpha_append_attributes(
    source: &RtAlphaMaskSource,
    vertices: &mut Vec<RtAlphaVertexGpu>,
    indices: &mut Vec<u32>,
) -> RtAlphaAttributeRange {
    let first_vertex = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    let first_index = u32::try_from(indices.len()).unwrap_or(u32::MAX);
    vertices.extend_from_slice(&source.vertices);
    indices.extend_from_slice(&source.indices);
    RtAlphaAttributeRange {
        first_vertex,
        first_index,
    }
}

pub(in crate::renderer) fn rt_alpha_append_dynamic_attributes(
    source_vertices: impl IntoIterator<Item = RtAlphaVertexGpu>,
    source_indices: &[u32],
    vertices: &mut Vec<RtAlphaVertexGpu>,
    indices: &mut Vec<u32>,
) -> RtAlphaAttributeRange {
    let first_vertex = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    let first_index = u32::try_from(indices.len()).unwrap_or(u32::MAX);
    vertices.extend(source_vertices);
    indices.extend_from_slice(source_indices);
    RtAlphaAttributeRange {
        first_vertex,
        first_index,
    }
}

pub(in crate::renderer) fn rt_alpha_empty_buffer_descriptor<T: Pod>(
    label: &str,
    extra_usage: wgpu::BufferUsages,
) -> wgpu::BufferDescriptor<'_> {
    wgpu::BufferDescriptor {
        label: Some(label),
        // A bound WGSL runtime array requires space for at least one element,
        // even when this scene has no masked geometry and never indexes it.
        size: (std::mem::size_of::<T>() as u64).max(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | extra_usage,
        mapped_at_creation: false,
    }
}

pub(in crate::renderer) fn create_rt_alpha_buffer<T: Pod>(
    device: &wgpu::Device,
    label: &str,
    values: &[T],
    extra_usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    if values.is_empty() {
        return device.create_buffer(&rt_alpha_empty_buffer_descriptor::<T>(label, extra_usage));
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(values),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | extra_usage,
    })
}

pub(in crate::renderer) fn create_rt_mask_blas(
    device: &wgpu::Device,
    label: &str,
    sources: &[RtAlphaMaskSource],
    alpha_vertices: &mut Vec<RtAlphaVertexGpu>,
    alpha_indices: &mut Vec<u32>,
    alpha_geometries: &mut Vec<RtAlphaGeometryGpu>,
    alpha_materials: &mut Vec<RtAlphaMaterialGpu>,
    alpha_textures: &mut Vec<RtAlphaTextureGpu>,
    alpha_texels: &mut Vec<u32>,
    texture_cache: &mut HashMap<String, u32>,
) -> Option<RayTracedMaskBlas> {
    if sources.is_empty() {
        return None;
    }
    let geometry_base = u32::try_from(alpha_geometries.len()).ok()?;
    let mut sizes = Vec::with_capacity(sources.len());
    let mut ranges = Vec::with_capacity(sources.len());
    let mut triangle_count = 0u32;
    for source in sources {
        if source.vertices.is_empty() || source.indices.len() < 3 || source.indices.len() % 3 != 0 {
            continue;
        }
        let range = rt_alpha_append_attributes(source, alpha_vertices, alpha_indices);
        let material_index = rt_alpha_append_material(
            &source.material,
            alpha_materials,
            alpha_textures,
            alpha_texels,
            texture_cache,
        );
        let vertex_count = u32::try_from(source.vertices.len()).ok()?;
        let index_count = u32::try_from(source.indices.len()).ok()?;
        sizes.push(wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count,
            index_format: Some(wgpu::IndexFormat::Uint32),
            index_count: Some(index_count),
            flags: wgpu::AccelerationStructureGeometryFlags::empty(),
        });
        ranges.push(range);
        alpha_geometries.push(RtAlphaGeometryGpu {
            values: [range.first_vertex, range.first_index, material_index, 0],
        });
        triangle_count = triangle_count.saturating_add(index_count / 3);
    }
    if sizes.is_empty() {
        return None;
    }
    let blas = device.create_blas(
        &wgpu::CreateBlasDescriptor {
            label: Some(label),
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        },
        wgpu::BlasGeometrySizeDescriptors::Triangles {
            descriptors: sizes.clone(),
        },
    );
    Some(RayTracedMaskBlas {
        sizes,
        ranges,
        blas,
        triangle_count,
        geometry_base,
    })
}

pub(in crate::renderer) fn rt_mask_blas_build_entry<'a>(
    resource: &'a RayTracedMaskBlas,
    vertex_buffer: &'a wgpu::Buffer,
    index_buffer: &'a wgpu::Buffer,
) -> wgpu::BlasBuildEntry<'a> {
    let geometries = resource
        .sizes
        .iter()
        .zip(resource.ranges.iter())
        .map(|(size, range)| wgpu::BlasTriangleGeometry {
            size,
            vertex_buffer,
            first_vertex: range.first_vertex,
            vertex_stride: std::mem::size_of::<RtAlphaVertexGpu>() as wgpu::BufferAddress,
            index_buffer: Some(index_buffer),
            first_index: Some(range.first_index),
            transform_buffer: None,
            transform_buffer_offset: None,
        })
        .collect::<Vec<_>>();
    wgpu::BlasBuildEntry {
        blas: &resource.blas,
        geometry: wgpu::BlasGeometries::TriangleGeometries(geometries),
    }
}

pub(in crate::renderer) fn create_rt_triangle_blas(
    device: &wgpu::Device,
    label: &str,
    positions: &[[f32; 3]],
    indices: &[u32],
    non_opaque: bool,
) -> Option<RayTracedTriangleBlas> {
    if positions.is_empty()
        || indices.len() < 3
        || indices.len() % 3 != 0
        || indices
            .iter()
            .any(|&index| index as usize >= positions.len())
    {
        return None;
    }
    let vertex_count = u32::try_from(positions.len()).ok()?;
    let index_count = u32::try_from(indices.len()).ok()?;
    let size = wgpu::BlasTriangleGeometrySizeDescriptor {
        vertex_format: wgpu::VertexFormat::Float32x3,
        vertex_count,
        index_format: Some(wgpu::IndexFormat::Uint32),
        index_count: Some(index_count),
        flags: if non_opaque {
            wgpu::AccelerationStructureGeometryFlags::empty()
        } else {
            wgpu::AccelerationStructureGeometryFlags::OPAQUE
        },
    };
    let blas = device.create_blas(
        &wgpu::CreateBlasDescriptor {
            label: Some(label),
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        },
        wgpu::BlasGeometrySizeDescriptors::Triangles {
            descriptors: vec![size.clone()],
        },
    );
    let vertex_label = format!("{label} vertices");
    let index_label = format!("{label} indices");
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&vertex_label),
        contents: bytemuck::cast_slice(positions),
        usage: wgpu::BufferUsages::BLAS_INPUT,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&index_label),
        contents: bytemuck::cast_slice(indices),
        usage: wgpu::BufferUsages::BLAS_INPUT,
    });
    Some(RayTracedTriangleBlas {
        _vertex_buffer: vertex_buffer,
        _index_buffer: index_buffer,
        size,
        blas,
        triangle_count: index_count / 3,
    })
}

pub(in crate::renderer) fn rt_triangle_blas_build_entry(
    resource: &RayTracedTriangleBlas,
) -> wgpu::BlasBuildEntry<'_> {
    wgpu::BlasBuildEntry {
        blas: &resource.blas,
        geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
            size: &resource.size,
            vertex_buffer: &resource._vertex_buffer,
            first_vertex: 0,
            vertex_stride: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
            index_buffer: Some(&resource._index_buffer),
            first_index: Some(0),
            transform_buffer: None,
            transform_buffer_offset: None,
        }]),
    }
}

pub(in crate::renderer) fn rt_skinned_blas_build_entry<'a>(
    resource: &'a RayTracedSkinnedBlas,
    caster: &RayTracedSkinnedPrepared,
    vertex_buffer: &'a wgpu::Buffer,
    index_buffer: &'a wgpu::Buffer,
) -> wgpu::BlasBuildEntry<'a> {
    wgpu::BlasBuildEntry {
        blas: &resource.blas,
        geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
            size: &resource.size,
            vertex_buffer,
            first_vertex: caster.first_vertex,
            vertex_stride: std::mem::size_of::<DynamicModelVertex>() as wgpu::BufferAddress,
            index_buffer: Some(index_buffer),
            first_index: Some(caster.first_index),
            transform_buffer: None,
            transform_buffer_offset: None,
        }]),
    }
}

pub(in crate::renderer) fn inline_rt_pose(model: &InlineModelGpu) -> Option<InlineModelInstance> {
    model
        .uploaded
        .unwrap_or_else(|| Some(InlineModelInstance::compiled(model.model)))
}

pub(in crate::renderer) fn hash_rt_transform(hasher: &mut impl Hasher, transform: [f32; 12]) {
    for value in transform {
        value.to_bits().hash(hasher);
    }
}

pub(in crate::renderer) fn rt_dynamic_scene_signature(
    world: &WorldGpu,
    inline_slot_count: usize,
    static_mask_present: bool,
    dynamic_models: &[DynamicModelSurface],
    dynamic_instance_capacity: usize,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    inline_slot_count.hash(&mut hasher);
    static_mask_present.hash(&mut hasher);
    for (slot, model) in world
        .inline_models
        .iter()
        .take(inline_slot_count)
        .enumerate()
    {
        slot.hash(&mut hasher);
        model.model.hash(&mut hasher);
        model.rt_indices.is_empty().hash(&mut hasher);
        model.rt_mask_sources.is_empty().hash(&mut hasher);
        match inline_rt_pose(model) {
            Some(pose) => {
                true.hash(&mut hasher);
                hash_rt_transform(&mut hasher, pose.rt_transform());
            }
            None => false.hash(&mut hasher),
        }
    }
    let mut dynamic_count = 0usize;
    for surface in dynamic_models {
        if dynamic_count >= dynamic_instance_capacity {
            break;
        }
        if !matches!(
            surface.alpha_mode,
            DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
        ) {
            continue;
        }
        let Some(source) = surface.rt_rigid.as_ref() else {
            continue;
        };
        let non_opaque = surface.alpha_mode == DynamicModelAlphaMode::Mask;
        source.asset_key.hash(&mut hasher);
        source.surface_index.hash(&mut hasher);
        source.frame.hash(&mut hasher);
        non_opaque.hash(&mut hasher);
        if non_opaque {
            rt_alpha_texture_key(surface.texture.as_deref()).hash(&mut hasher);
        }
        hash_rt_transform(&mut hasher, source.transform);
        dynamic_count += 1;
    }
    dynamic_count.hash(&mut hasher);
    hasher.finish()
}

impl Renderer {
    /// Build the static BSP BLAS/TLAS and the matching ray-query BSP shader.
    /// Opaque geometry stays on the fast auto-commit path. Authored mask stages
    /// are separate non-opaque geometries so WGSL can evaluate candidate alpha
    /// exactly before confirming a shadow hit. Everything remains cold/lazy.
    pub(in crate::renderer) fn ensure_ray_traced_shadow_resources(&mut self) -> bool {
        if self.ray_traced_shadows.is_some() {
            return true;
        }
        if !self.ray_tracing_supported {
            println!("RT Shadows: unavailable - EXPERIMENTAL_RAY_QUERY is not supported by this adapter/backend");
            return false;
        }

        let Some(world) = self.world.as_ref() else {
            return false;
        };
        if world.rt_shadow_ranges.is_empty() || world.rt_vertex_count == 0 {
            println!("RT Shadows: current map has no eligible opaque static BSP triangles");
            return false;
        }

        let geometry_count = world.rt_shadow_ranges.len();
        let triangle_count = world
            .rt_shadow_ranges
            .iter()
            .map(|range| u64::from(range.end.saturating_sub(range.start) / 3))
            .sum::<u64>();
        let limits = self.device.limits();
        if geometry_count > limits.max_blas_geometry_count as usize {
            println!(
                "RT Shadows: map requires {geometry_count} opaque BLAS geometries, adapter limit is {}; leaving RT shadows inactive",
                limits.max_blas_geometry_count
            );
            return false;
        }
        if triangle_count > u64::from(limits.max_blas_primitive_count) {
            println!(
                "RT Shadows: map requires {triangle_count} opaque BLAS triangles, adapter limit is {}; leaving RT shadows inactive",
                limits.max_blas_primitive_count
            );
            return false;
        }
        let max_tlas_instances = limits.max_tlas_instance_count as usize;
        if max_tlas_instances == 0 {
            println!("RT Shadows: adapter reports max_tlas_instance_count=0; leaving RT shadows inactive");
            return false;
        }

        let geometry_sizes = world
            .rt_shadow_ranges
            .iter()
            .map(|range| wgpu::BlasTriangleGeometrySizeDescriptor {
                vertex_format: wgpu::VertexFormat::Float32x3,
                vertex_count: world.rt_vertex_count,
                index_format: Some(wgpu::IndexFormat::Uint32),
                index_count: Some(range.end.saturating_sub(range.start)),
                flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
            })
            .collect::<Vec<_>>();

        let blas = self.device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("JKA RT Shadows static opaque BSP BLAS"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: geometry_sizes.clone(),
            },
        );

        // Build candidate-attribute storage only for alpha-tested geometry. This
        // data does not exist at all until RT Shadows is selected.
        let mut alpha_vertices = Vec::<RtAlphaVertexGpu>::new();
        let mut alpha_indices = Vec::<u32>::new();
        let mut alpha_geometries = Vec::<RtAlphaGeometryGpu>::new();
        let mut alpha_materials = Vec::<RtAlphaMaterialGpu>::new();
        let mut alpha_textures = Vec::<RtAlphaTextureGpu>::new();
        let mut alpha_texels = Vec::<u32>::new();
        let mut alpha_texture_cache = HashMap::<String, u32>::new();

        let static_mask_blas = if world.rt_shadow_masks.len()
            <= limits.max_blas_geometry_count as usize
        {
            create_rt_mask_blas(
                &self.device,
                "JKA RT Shadows static alpha-tested BSP BLAS",
                &world.rt_shadow_masks,
                &mut alpha_vertices,
                &mut alpha_indices,
                &mut alpha_geometries,
                &mut alpha_materials,
                &mut alpha_textures,
                &mut alpha_texels,
                &mut alpha_texture_cache,
            )
        } else {
            println!(
                "RT Shadows: {} static masked geometries exceed adapter BLAS geometry limit {}; masked BSP casting disabled",
                world.rt_shadow_masks.len(), limits.max_blas_geometry_count
            );
            None
        };

        // Each mover can consume two fixed TLAS slots (opaque + alpha-tested).
        // Reserve static world slots first, then the bounded dynamic tail.
        let static_slot_count = 1usize + usize::from(static_mask_blas.is_some());
        let inline_slot_count = world
            .inline_models
            .len()
            .min(max_tlas_instances.saturating_sub(static_slot_count) / 2);
        if inline_slot_count < world.inline_models.len() {
            println!(
                "RT Shadows: TLAS limit {} truncates inline BSP casters from {} to {}",
                limits.max_tlas_instance_count,
                world.inline_models.len(),
                inline_slot_count
            );
        }

        let mut inline_blas = (0..inline_slot_count)
            .map(|_| None)
            .collect::<Vec<Option<RayTracedTriangleBlas>>>();
        let mut inline_mask_blas = (0..inline_slot_count)
            .map(|_| None)
            .collect::<Vec<Option<RayTracedMaskBlas>>>();
        let mut inline_triangle_count = 0u64;
        let mut inline_geometry_count = 0usize;
        let mut inline_mask_triangle_count = 0u64;
        let mut inline_mask_geometry_count = 0usize;
        for (slot, model) in world
            .inline_models
            .iter()
            .take(inline_slot_count)
            .enumerate()
        {
            if !model.rt_indices.is_empty() {
                let triangles = model.rt_indices.len() / 3;
                if triangles <= limits.max_blas_primitive_count as usize {
                    let positions = model
                        .base
                        .iter()
                        .map(|vertex| vertex.position)
                        .collect::<Vec<_>>();
                    let label = format!("JKA RT Shadows inline BSP *{} opaque BLAS", model.model);
                    if let Some(resource) = create_rt_triangle_blas(
                        &self.device,
                        &label,
                        &positions,
                        &model.rt_indices,
                        false,
                    ) {
                        inline_triangle_count += u64::from(resource.triangle_count);
                        inline_geometry_count += 1;
                        inline_blas[slot] = Some(resource);
                    }
                }
            }
            if model.rt_mask_sources.len() <= limits.max_blas_geometry_count as usize {
                let label = format!(
                    "JKA RT Shadows inline BSP *{} alpha-tested BLAS",
                    model.model
                );
                if let Some(resource) = create_rt_mask_blas(
                    &self.device,
                    &label,
                    &model.rt_mask_sources,
                    &mut alpha_vertices,
                    &mut alpha_indices,
                    &mut alpha_geometries,
                    &mut alpha_materials,
                    &mut alpha_textures,
                    &mut alpha_texels,
                    &mut alpha_texture_cache,
                ) {
                    inline_mask_triangle_count += u64::from(resource.triangle_count);
                    inline_mask_geometry_count += resource.sizes.len();
                    inline_mask_blas[slot] = Some(resource);
                }
            }
        }

        let fixed_slot_count = static_slot_count + inline_slot_count.saturating_mul(2);
        let dynamic_instance_capacity = max_tlas_instances
            .saturating_sub(fixed_slot_count)
            .min(RT_DYNAMIC_TLAS_INSTANCE_BUDGET);
        let tlas_instance_capacity = fixed_slot_count + dynamic_instance_capacity;

        // Reserve one geometry/material record per dynamic TLAS slot. Masked
        // instances overwrite their slot every frame; opaque instances never
        // enter candidate handling, so their record is irrelevant.
        let alpha_dynamic_geometry_base = u32::try_from(alpha_geometries.len()).unwrap_or(u32::MAX);
        let alpha_dynamic_material_base = u32::try_from(alpha_materials.len()).unwrap_or(u32::MAX);
        alpha_geometries.resize(
            alpha_geometries
                .len()
                .saturating_add(dynamic_instance_capacity),
            RtAlphaGeometryGpu::zeroed(),
        );
        alpha_materials.resize(
            alpha_materials
                .len()
                .saturating_add(dynamic_instance_capacity),
            RtAlphaMaterialGpu::zeroed(),
        );

        let alpha_vertex_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha candidate vertices",
            &alpha_vertices,
            wgpu::BufferUsages::BLAS_INPUT,
        );
        let alpha_index_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha candidate indices",
            &alpha_indices,
            wgpu::BufferUsages::BLAS_INPUT,
        );
        let alpha_geometry_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha geometry table",
            &alpha_geometries,
            wgpu::BufferUsages::empty(),
        );
        let alpha_material_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha material table",
            &alpha_materials,
            wgpu::BufferUsages::empty(),
        );
        let alpha_texture_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha texture table",
            &alpha_textures,
            wgpu::BufferUsages::empty(),
        );
        let alpha_texel_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha texels",
            &alpha_texels,
            wgpu::BufferUsages::empty(),
        );

        let mut tlas = self.device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("JKA RT Shadows world + dynamic TLAS"),
            max_instances: u32::try_from(tlas_instance_capacity)
                .unwrap_or(limits.max_tlas_instance_count),
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        tlas[0] = Some(wgpu::TlasInstance::new(&blas, identity, 0, 0xff));
        let mut inline_base = 1usize;
        if let Some(mask) = static_mask_blas.as_ref() {
            tlas[1] = Some(wgpu::TlasInstance::new(
                &mask.blas,
                identity,
                mask.geometry_base,
                0xff,
            ));
            inline_base += 1;
        }
        for slot in 0..inline_slot_count {
            let Some(model) = world.inline_models.get(slot) else {
                continue;
            };
            let Some(pose) = inline_rt_pose(model) else {
                continue;
            };
            let base = inline_base + slot * 2;
            if let Some(caster) = inline_blas[slot].as_ref() {
                tlas[base] = Some(wgpu::TlasInstance::new(
                    &caster.blas,
                    pose.rt_transform(),
                    0,
                    0xff,
                ));
            }
            if let Some(caster) = inline_mask_blas[slot].as_ref() {
                tlas[base + 1] = Some(wgpu::TlasInstance::new(
                    &caster.blas,
                    pose.rt_transform(),
                    caster.geometry_base,
                    0xff,
                ));
            }
        }

        let geometries = world
            .rt_shadow_ranges
            .iter()
            .zip(geometry_sizes.iter())
            .map(|(range, size)| wgpu::BlasTriangleGeometry {
                size,
                vertex_buffer: &world.vertex_buffer,
                first_vertex: 0,
                vertex_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                index_buffer: Some(&world.index_buffer),
                first_index: Some(range.start),
                transform_buffer: None,
                transform_buffer_offset: None,
            })
            .collect::<Vec<_>>();
        let static_blas_build = wgpu::BlasBuildEntry {
            blas: &blas,
            geometry: wgpu::BlasGeometries::TriangleGeometries(geometries),
        };
        let inline_blas_builds = inline_blas
            .iter()
            .filter_map(Option::as_ref)
            .map(rt_triangle_blas_build_entry)
            .collect::<Vec<_>>();
        let static_mask_build = static_mask_blas
            .as_ref()
            .map(|mask| rt_mask_blas_build_entry(mask, &alpha_vertex_buffer, &alpha_index_buffer));
        let inline_mask_builds = inline_mask_blas
            .iter()
            .filter_map(Option::as_ref)
            .map(|mask| rt_mask_blas_build_entry(mask, &alpha_vertex_buffer, &alpha_index_buffer))
            .collect::<Vec<_>>();
        let mut blas_builds = Vec::with_capacity(
            1 + inline_blas_builds.len()
                + usize::from(static_mask_build.is_some())
                + inline_mask_builds.len(),
        );
        blas_builds.push(static_blas_build);
        blas_builds.extend(inline_blas_builds);
        if let Some(build) = static_mask_build {
            blas_builds.push(build);
        }
        blas_builds.extend(inline_mask_builds);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA RT Shadows acceleration-structure build"),
            });
        encoder.build_acceleration_structures(blas_builds.iter(), std::iter::once(&tlas));
        self.queue.submit(std::iter::once(encoder.finish()));

        let receiver_layout = create_ray_traced_shadow_receiver_layout(&self.device);
        let world_pipeline_layout =
            self.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("JKA BSP ray-traced-shadow pipeline layout"),
                    bind_group_layouts: &[
                        Some(&self.camera_layout),
                        Some(&self.surface_layout),
                        Some(&self.lighting_layout),
                        Some(&receiver_layout),
                        Some(&self.planar_reflection_layout),
                        Some(&self.ocean_optics_layout),
                        Some(&self.ocean_layout),
                    ],
                    immediate_size: 0,
                });
        let world_pipeline_layout_lean =
            self.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("JKA BSP ray-traced-shadow pipeline layout (lean baseline)"),
                    bind_group_layouts: &[
                        Some(&self.camera_layout),
                        Some(&self.surface_layout),
                        Some(&self.lighting_layout_lean),
                        Some(&receiver_layout),
                        Some(&self.planar_reflection_layout),
                    ],
                    immediate_size: 0,
                });

        let world_shader_source = ray_traced_world_shader_source(include_str!("../../bsp.wgsl"))
            .expect("RT Shadows: bsp.wgsl transform must match the renderer shader");
        let world_shader_lean_source =
            ray_traced_world_shader_source(include_str!("../../bsp_lean.wgsl"))
                .expect("RT Shadows: bsp_lean.wgsl transform must match the renderer shader");
        let world_shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("JKA BSP shader (hardware ray-traced shadows)"),
                source: wgpu::ShaderSource::Wgsl(compose_world_shader(&world_shader_source).into()),
            });
        let world_shader_lean = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("JKA BSP shader lean (hardware ray-traced shadows)"),
                source: wgpu::ShaderSource::Wgsl(
                    compose_world_shader(&world_shader_lean_source).into(),
                ),
            });
        let sun_shadow_history = rt_resolution::RtSunShadowHistory::new(&self.device);
        let receiver_bind_group = create_rt_receiver_bind_group(
            &self.device,
            &self.shadow_resources,
            self.weather.fog.legacy_control_buffer(),
            self.cascaded_shadow_mode,
            &receiver_layout,
            RtReceiverScene {
                tlas: &tlas,
                alpha_vertices: &alpha_vertex_buffer,
                alpha_indices: &alpha_index_buffer,
                alpha_geometries: &alpha_geometry_buffer,
                alpha_materials: &alpha_material_buffer,
                alpha_textures: &alpha_texture_buffer,
                alpha_texels: &alpha_texel_buffer,
            },
            &sun_shadow_history,
        );

        let static_mask_geometry_count =
            static_mask_blas.as_ref().map_or(0, |mask| mask.sizes.len());
        let static_mask_triangle_count = static_mask_blas
            .as_ref()
            .map_or(0, |mask| u64::from(mask.triangle_count));
        let masked_geometry_count = static_mask_geometry_count + inline_mask_geometry_count;
        let masked_triangle_count = static_mask_triangle_count + inline_mask_triangle_count;

        self.ray_traced_shadows = Some(RayTracedShadowResources {
            sun_shadow_history,
            model_receivers: None,
            receiver_layout,
            world_pipeline_layout,
            world_pipeline_layout_lean,
            world_shader,
            world_shader_lean,
            receiver_bind_group,
            _blas: blas,
            static_mask_blas,
            _tlas: tlas,
            inline_blas,
            inline_mask_blas,
            rigid_blas_cache: HashMap::new(),
            skinned_blas_cache: HashMap::new(),
            rigid_alpha_attributes: HashMap::new(),
            skinned_alpha_attributes: HashMap::new(),
            alpha_vertices,
            alpha_indices,
            alpha_geometries,
            alpha_materials,
            alpha_textures,
            alpha_texels,
            alpha_texture_cache,
            alpha_vertex_buffer,
            alpha_index_buffer,
            alpha_geometry_buffer,
            alpha_material_buffer,
            alpha_texture_buffer,
            alpha_texel_buffer,
            alpha_dynamic_geometry_base,
            alpha_dynamic_material_base,
            alpha_buffers_generation: 0,
            inline_slot_count,
            dynamic_instance_capacity,
            last_scene_signature: u64::MAX,
            last_dynamic_instance_count: 0,
            rigid_cache_limit_warned: false,
            skinned_cache_limit_warned: false,
            geometry_count,
            triangle_count,
            masked_geometry_count,
            masked_triangle_count,
            active_masked_rigid: 0,
            active_masked_skinned: 0,
            active_rigid_triangles: 0,
            active_skinned_triangles: 0,
        });
        println!(
            "RT Shadows: built HW BLAS/TLAS: opaque BSP {geometry_count} geometries / {triangle_count} tris; alpha-tested BSP+movers {masked_geometry_count} geometries / {masked_triangle_count} tris; opaque inline {inline_geometry_count} BLASes / {inline_triangle_count} tris; dynamic capacity {dynamic_instance_capacity}"
        );
        true
    }

    pub(in crate::renderer) fn rebuild_rt_alpha_candidate_buffers(&mut self) {
        let Some(mut rt) = self.ray_traced_shadows.take() else {
            return;
        };
        rt.alpha_vertex_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha candidate vertices",
            &rt.alpha_vertices,
            wgpu::BufferUsages::BLAS_INPUT,
        );
        rt.alpha_index_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha candidate indices",
            &rt.alpha_indices,
            wgpu::BufferUsages::BLAS_INPUT,
        );
        rt.alpha_geometry_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha geometry table",
            &rt.alpha_geometries,
            wgpu::BufferUsages::empty(),
        );
        rt.alpha_material_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha material table",
            &rt.alpha_materials,
            wgpu::BufferUsages::empty(),
        );
        rt.alpha_texture_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha texture table",
            &rt.alpha_textures,
            wgpu::BufferUsages::empty(),
        );
        rt.alpha_texel_buffer = create_rt_alpha_buffer(
            &self.device,
            "JKA RT alpha texels",
            &rt.alpha_texels,
            wgpu::BufferUsages::empty(),
        );
        rt.alpha_buffers_generation = rt.alpha_buffers_generation.wrapping_add(1);
        rt.receiver_bind_group = create_rt_receiver_bind_group(
            &self.device,
            &self.shadow_resources,
            self.weather.fog.legacy_control_buffer(),
            self.cascaded_shadow_mode,
            &rt.receiver_layout,
            rt.scene(),
            &rt.sun_shadow_history,
        );
        self.ray_traced_shadows = Some(rt);
    }

    /// Rebuild dynamic ray-tracing geometry before the first world ray query.
    ///
    /// Rigid BSP/MD3 casters retain immutable BLAS geometry and only update TLAS
    /// transforms. Ghoul2 casters are different: animation deforms their actual
    /// triangles, so RT-on compute skinning writes the final world-space vertices
    /// once and each active skinned BLAS is rebuilt from that shared buffer.
    pub(in crate::renderer) fn encode_ray_traced_dynamic_casters(
        &mut self,
        dynamic_models: &[DynamicModelSurface],
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if !self.hardware_rt_requested() {
            return;
        }
        if self.ray_traced_shadows.is_none() && !self.ensure_ray_traced_shadow_resources() {
            return;
        }

        let skinned_prepared = self.dynamic_model_renderer.rt_skinned_casters().to_vec();
        let Some(world) = self.world.as_ref() else {
            return;
        };
        let Some(rt_read) = self.ray_traced_shadows.as_ref() else {
            return;
        };

        // Skinned player/NPC geometry takes priority in the bounded dynamic TLAS
        // tail. Rigid MD3s use whatever slots remain.
        let skinned_requested = skinned_prepared
            .len()
            .min(rt_read.dynamic_instance_capacity);
        let rigid_capacity = rt_read
            .dynamic_instance_capacity
            .saturating_sub(skinned_requested);
        let signature = rt_dynamic_scene_signature(
            world,
            rt_read.inline_slot_count,
            rt_read.static_mask_blas.is_some(),
            dynamic_models,
            rigid_capacity,
        );
        let inline_transforms = world
            .inline_models
            .iter()
            .take(rt_read.inline_slot_count)
            .map(|model| inline_rt_pose(model).map(|pose| pose.rt_transform()))
            .collect::<Vec<_>>();
        // A deforming BLAS must be rebuilt even when entity transforms did not
        // change, because the current animation pose may have changed.
        if skinned_requested == 0 && signature == rt_read.last_scene_signature {
            return;
        }

        let limits = self.device.limits();
        let eligible_rigid = dynamic_models
            .iter()
            .filter(|surface| {
                matches!(
                    surface.alpha_mode,
                    DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
                ) && surface.rt_rigid.is_some()
            })
            .take(rigid_capacity)
            .collect::<Vec<_>>();

        // Find missing immutable MD3 frame/surface BLASes before taking the
        // mutable cache borrow. The cache key includes whether the triangles are
        // opaque or alpha-tested because candidate handling is a BLAS geometry flag.
        let mut missing_rigid = Vec::<(RayTracedRigidInstance, bool)>::new();
        let mut queued_rigid = HashSet::<RayTracedRigidMeshKey>::new();
        for surface in &eligible_rigid {
            let Some(source) = surface.rt_rigid.as_ref() else {
                continue;
            };
            let non_opaque = surface.alpha_mode == DynamicModelAlphaMode::Mask;
            let key = source.mesh_key(non_opaque);
            if rt_read.rigid_blas_cache.contains_key(&key) || !queued_rigid.insert(key) {
                continue;
            }
            missing_rigid.push((source.clone(), non_opaque));
        }

        let device = &self.device;
        let alpha_storage_changed;
        let mut new_rigid_keys = Vec::<RayTracedRigidMeshKey>::new();
        {
            let Some(rt) = self.ray_traced_shadows.as_mut() else {
                return;
            };
            let alpha_vertex_len_before = rt.alpha_vertices.len();
            let alpha_index_len_before = rt.alpha_indices.len();
            let alpha_texture_len_before = rt.alpha_textures.len();
            let alpha_texel_len_before = rt.alpha_texels.len();

            // A cached MD3 BLAS can be used with a newly selected skin. Texture
            // residency must follow active materials, independently of BLAS misses.
            for surface in &eligible_rigid {
                if surface.alpha_mode == DynamicModelAlphaMode::Mask {
                    rt_alpha_ensure_dynamic_texture(
                        surface.texture.as_deref(),
                        &mut rt.alpha_textures,
                        &mut rt.alpha_texels,
                        &mut rt.alpha_texture_cache,
                    );
                }
            }

            for (source, non_opaque) in missing_rigid {
                if rt.rigid_blas_cache.len() >= RT_RIGID_BLAS_CACHE_LIMIT {
                    if !rt.rigid_cache_limit_warned {
                        println!(
                            "RT Shadows: rigid BLAS cache reached {} entries; additional MD3 frame/surfaces will not cast until the map changes",
                            RT_RIGID_BLAS_CACHE_LIMIT
                        );
                        rt.rigid_cache_limit_warned = true;
                    }
                    break;
                }
                let key = source.mesh_key(non_opaque);
                if rt.rigid_blas_cache.contains_key(&key) {
                    continue;
                }
                let Some(surface) = source.model.surfaces.get(source.surface_index as usize) else {
                    continue;
                };
                let Some(vertices) = surface.frame_vertices(source.frame as usize) else {
                    continue;
                };
                if vertices.is_empty()
                    || surface.indices.is_empty()
                    || surface.indices.len() % 3 != 0
                {
                    continue;
                }
                let triangles = surface.indices.len() / 3;
                if triangles > limits.max_blas_primitive_count as usize {
                    println!(
                        "RT Shadows: {} surface {} frame {} has {triangles} triangles, over adapter BLAS limit {}; skipping caster",
                        source.asset_key,
                        source.surface_index,
                        source.frame,
                        limits.max_blas_primitive_count
                    );
                    continue;
                }
                let positions = vertices
                    .iter()
                    .map(|vertex| vertex.position)
                    .collect::<Vec<_>>();
                let label = format!(
                    "JKA RT Shadows MD3 {} surface {} frame {} {} BLAS",
                    source.asset_key,
                    source.surface_index,
                    source.frame,
                    if non_opaque { "alpha-tested" } else { "opaque" }
                );
                let Some(resource) = create_rt_triangle_blas(
                    device,
                    &label,
                    &positions,
                    &surface.indices,
                    non_opaque,
                ) else {
                    continue;
                };
                if non_opaque && !rt.rigid_alpha_attributes.contains_key(&key) {
                    let attrs = rt_alpha_append_dynamic_attributes(
                        vertices.iter().map(|vertex| RtAlphaVertexGpu {
                            position_u: [
                                vertex.position[0],
                                vertex.position[1],
                                vertex.position[2],
                                vertex.uv[0],
                            ],
                            normal_v: [
                                vertex.normal[0],
                                vertex.normal[1],
                                vertex.normal[2],
                                vertex.uv[1],
                            ],
                            lightmap_alpha: [0.0, 0.0, 1.0, 0.0],
                        }),
                        &surface.indices,
                        &mut rt.alpha_vertices,
                        &mut rt.alpha_indices,
                    );
                    rt.rigid_alpha_attributes.insert(key.clone(), attrs);
                }
                rt.rigid_blas_cache.insert(key.clone(), resource);
                new_rigid_keys.push(key);
            }

            // Create persistent BLAS objects for newly encountered deforming
            // entity/surfaces. Their backing geometry stays in the shared RT skinned
            // vertex/index buffers and is rebuilt below after the compute pass.
            for caster in skinned_prepared.iter().take(skinned_requested) {
                if !rt.skinned_blas_cache.contains_key(&caster.key) {
                    if rt.skinned_blas_cache.len() >= RT_SKINNED_BLAS_CACHE_LIMIT {
                        if !rt.skinned_cache_limit_warned {
                            println!(
                                "RT Shadows: skinned Ghoul2 BLAS cache reached {} entries; additional entity/surfaces will not cast until the map changes",
                                RT_SKINNED_BLAS_CACHE_LIMIT
                            );
                            rt.skinned_cache_limit_warned = true;
                        }
                        break;
                    }
                    if caster.vertex_count == 0
                        || caster.index_count < 3
                        || caster.index_count % 3 != 0
                    {
                        continue;
                    }
                    let triangles = caster.index_count / 3;
                    if triangles > limits.max_blas_primitive_count {
                        println!(
                            "RT Shadows: skinned entity {} mesh {} has {triangles} triangles, over adapter BLAS limit {}; skipping caster",
                            caster.key.entity_num,
                            caster.key.mesh_key,
                            limits.max_blas_primitive_count
                        );
                        continue;
                    }
                    let size = wgpu::BlasTriangleGeometrySizeDescriptor {
                        vertex_format: wgpu::VertexFormat::Float32x3,
                        vertex_count: caster.vertex_count,
                        index_format: Some(wgpu::IndexFormat::Uint32),
                        index_count: Some(caster.index_count),
                        flags: if caster.key.non_opaque {
                            wgpu::AccelerationStructureGeometryFlags::empty()
                        } else {
                            wgpu::AccelerationStructureGeometryFlags::OPAQUE
                        },
                    };
                    let label = format!(
                        "JKA RT Shadows Ghoul2 entity {} {} {} BLAS",
                        caster.key.entity_num,
                        caster.key.mesh_key,
                        if caster.key.non_opaque {
                            "alpha-tested"
                        } else {
                            "opaque"
                        }
                    );
                    let blas = device.create_blas(
                        &wgpu::CreateBlasDescriptor {
                            label: Some(&label),
                            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
                            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                        },
                        wgpu::BlasGeometrySizeDescriptors::Triangles {
                            descriptors: vec![size.clone()],
                        },
                    );
                    rt.skinned_blas_cache
                        .insert(caster.key.clone(), RayTracedSkinnedBlas { size, blas });
                }

                if caster.key.non_opaque
                    && !rt
                        .skinned_alpha_attributes
                        .contains_key(&caster.key.mesh_key)
                {
                    let source = dynamic_models.iter().find(|surface| {
                        surface.entity_num == caster.key.entity_num
                            && surface.alpha_mode == DynamicModelAlphaMode::Mask
                            && surface.rt_skinned_key.as_ref() == Some(&caster.key.mesh_key)
                    });
                    if let Some(source) = source {
                        let attrs = if let Some(skin) = source.ghoul2_gpu.as_ref() {
                            rt_alpha_append_dynamic_attributes(
                                skin.vertices.iter().map(RtAlphaVertexGpu::from),
                                skin.indices.as_slice(),
                                &mut rt.alpha_vertices,
                                &mut rt.alpha_indices,
                            )
                        } else {
                            rt_alpha_append_dynamic_attributes(
                                source.vertices.iter().map(RtAlphaVertexGpu::from),
                                source.indices.as_slice(),
                                &mut rt.alpha_vertices,
                                &mut rt.alpha_indices,
                            )
                        };
                        rt.skinned_alpha_attributes
                            .insert(Arc::clone(&caster.key.mesh_key), attrs);
                    }
                }
                if caster.key.non_opaque {
                    rt_alpha_ensure_dynamic_texture(
                        caster.alpha_texture.as_deref(),
                        &mut rt.alpha_textures,
                        &mut rt.alpha_texels,
                        &mut rt.alpha_texture_cache,
                    );
                }
            }

            alpha_storage_changed = rt.alpha_vertices.len() != alpha_vertex_len_before
                || rt.alpha_indices.len() != alpha_index_len_before
                || rt.alpha_textures.len() != alpha_texture_len_before
                || rt.alpha_texels.len() != alpha_texel_len_before;
        }

        // Candidate attribute/texture tables are append-only. Reallocate them
        // only when a new masked asset/frame is first encountered, never every frame.
        if alpha_storage_changed {
            self.rebuild_rt_alpha_candidate_buffers();
        }

        let Some(rt) = self.ray_traced_shadows.as_mut() else {
            return;
        };
        let active_skinned = skinned_prepared
            .iter()
            .take(skinned_requested)
            .filter(|caster| rt.skinned_blas_cache.contains_key(&caster.key))
            .collect::<Vec<_>>();

        // Static masked world may occupy TLAS slot 1. Each inline model then owns
        // a fixed opaque/masked slot pair so hiding or moving a BSP mover updates
        // both representations together without touching its immutable BLASes.
        let inline_base = 1 + usize::from(rt.static_mask_blas.is_some());
        for slot in 0..rt.inline_slot_count {
            let base = inline_base + slot * 2;
            let pose = inline_transforms.get(slot).copied().flatten();
            rt._tlas[base] = pose.and_then(|transform| {
                rt.inline_blas
                    .get(slot)
                    .and_then(Option::as_ref)
                    .map(|caster| wgpu::TlasInstance::new(&caster.blas, transform, 0, 0xff))
            });
            rt._tlas[base + 1] = pose.and_then(|transform| {
                rt.inline_mask_blas
                    .get(slot)
                    .and_then(Option::as_ref)
                    .map(|caster| {
                        wgpu::TlasInstance::new(&caster.blas, transform, caster.geometry_base, 0xff)
                    })
            });
        }

        let dynamic_base = inline_base + rt.inline_slot_count * 2;
        let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut active_masked_skinned = 0usize;
        let mut active_skinned_triangles = 0u64;
        let mut populated_dynamic_slots = 0usize;

        for caster in &active_skinned {
            if populated_dynamic_slots >= rt.dynamic_instance_capacity {
                break;
            }
            if !rt.skinned_blas_cache.contains_key(&caster.key) {
                continue;
            }
            let slot = populated_dynamic_slots;
            let index = dynamic_base + slot;
            let custom_data = if caster.key.non_opaque {
                let Some(attrs) = rt
                    .skinned_alpha_attributes
                    .get(&caster.key.mesh_key)
                    .copied()
                else {
                    continue;
                };
                let geometry_index = rt.alpha_dynamic_geometry_base.saturating_add(slot as u32);
                let material_index = rt.alpha_dynamic_material_base.saturating_add(slot as u32);
                let Some(&texture_index) = rt
                    .alpha_texture_cache
                    .get(&rt_alpha_texture_key(caster.alpha_texture.as_deref()))
                else {
                    continue;
                };
                if let Some(record) = rt.alpha_geometries.get_mut(geometry_index as usize) {
                    *record = RtAlphaGeometryGpu {
                        values: [attrs.first_vertex, attrs.first_index, material_index, 0],
                    };
                }
                if let Some(record) = rt.alpha_materials.get_mut(material_index as usize) {
                    *record = rt_alpha_dynamic_material(texture_index);
                }
                active_masked_skinned += 1;
                geometry_index
            } else {
                0
            };
            let instance = rt.skinned_blas_cache.get(&caster.key).map(|resource| {
                wgpu::TlasInstance::new(&resource.blas, identity, custom_data, 0xff)
            });
            rt._tlas[index] = instance;
            active_skinned_triangles =
                active_skinned_triangles.saturating_add(u64::from(caster.index_count / 3));
            populated_dynamic_slots += 1;
        }

        let mut active_rigid_instances = 0usize;
        let mut active_masked_rigid = 0usize;
        let mut active_rigid_triangles = 0u64;
        for surface in &eligible_rigid {
            if populated_dynamic_slots >= rt.dynamic_instance_capacity {
                break;
            }
            let Some(source) = surface.rt_rigid.as_ref() else {
                continue;
            };
            let non_opaque = surface.alpha_mode == DynamicModelAlphaMode::Mask;
            let key = source.mesh_key(non_opaque);
            if !rt.rigid_blas_cache.contains_key(&key) {
                continue;
            }
            let slot = populated_dynamic_slots;
            let index = dynamic_base + slot;
            let custom_data = if non_opaque {
                let Some(attrs) = rt.rigid_alpha_attributes.get(&key).copied() else {
                    continue;
                };
                let geometry_index = rt.alpha_dynamic_geometry_base.saturating_add(slot as u32);
                let material_index = rt.alpha_dynamic_material_base.saturating_add(slot as u32);
                let Some(&texture_index) = rt
                    .alpha_texture_cache
                    .get(&rt_alpha_texture_key(surface.texture.as_deref()))
                else {
                    continue;
                };
                if let Some(record) = rt.alpha_geometries.get_mut(geometry_index as usize) {
                    *record = RtAlphaGeometryGpu {
                        values: [attrs.first_vertex, attrs.first_index, material_index, 0],
                    };
                }
                if let Some(record) = rt.alpha_materials.get_mut(material_index as usize) {
                    *record = rt_alpha_dynamic_material(texture_index);
                }
                active_masked_rigid += 1;
                geometry_index
            } else {
                0
            };
            let (instance, triangle_count) =
                rt.rigid_blas_cache
                    .get(&key)
                    .map_or((None, 0u32), |caster| {
                        (
                            Some(wgpu::TlasInstance::new(
                                &caster.blas,
                                source.transform,
                                custom_data,
                                0xff,
                            )),
                            caster.triangle_count,
                        )
                    });
            rt._tlas[index] = instance;
            active_rigid_instances += 1;
            active_rigid_triangles =
                active_rigid_triangles.saturating_add(u64::from(triangle_count));
            populated_dynamic_slots += 1;
        }
        for slot in populated_dynamic_slots..rt.last_dynamic_instance_count {
            rt._tlas[dynamic_base + slot] = None;
        }

        // Dynamic geometry/material records have fixed slots, so updating them
        // does not allocate or rebuild bind groups on ordinary animated frames.
        // Only the populated head of the dynamic tail can have changed; the
        // static records and the unused slots stay untouched on the GPU.
        if active_masked_skinned + active_masked_rigid > 0 && populated_dynamic_slots > 0 {
            let geometry_range = rt.alpha_dynamic_geometry_base as usize
                ..rt.alpha_dynamic_geometry_base as usize + populated_dynamic_slots;
            let material_range = rt.alpha_dynamic_material_base as usize
                ..rt.alpha_dynamic_material_base as usize + populated_dynamic_slots;
            self.queue.write_buffer(
                &rt.alpha_geometry_buffer,
                (geometry_range.start * std::mem::size_of::<RtAlphaGeometryGpu>()) as u64,
                bytemuck::cast_slice(&rt.alpha_geometries[geometry_range]),
            );
            self.queue.write_buffer(
                &rt.alpha_material_buffer,
                (material_range.start * std::mem::size_of::<RtAlphaMaterialGpu>()) as u64,
                bytemuck::cast_slice(&rt.alpha_materials[material_range]),
            );
        }

        let new_rigid_builds = new_rigid_keys
            .iter()
            .filter_map(|key| rt.rigid_blas_cache.get(key))
            .map(rt_triangle_blas_build_entry)
            .collect::<Vec<_>>();

        // Borrow the shared RT-skinned buffers only after any alpha-table
        // reallocation above. Keeping this borrow late avoids tying up the
        // DynamicModelRenderer while the RT receiver bind group is rebuilt.
        let skinned_buffers = self.dynamic_model_renderer.rt_skinning_buffers();
        let skinned_builds = match skinned_buffers {
            Some((vertex_buffer, index_buffer)) => active_skinned
                .iter()
                .filter_map(|caster| {
                    let resource = rt.skinned_blas_cache.get(&caster.key)?;
                    Some(rt_skinned_blas_build_entry(
                        resource,
                        caster,
                        vertex_buffer,
                        index_buffer,
                    ))
                })
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };

        // wgpu tracks compute-storage-write -> BLAS_INPUT dependencies inside
        // this encoder. Rigid BLASes build only when first cached; deforming
        // Ghoul2 BLASes rebuild each active RT frame, followed by the TLAS.
        encoder.build_acceleration_structures(
            new_rigid_builds.iter().chain(skinned_builds.iter()),
            std::iter::once(&rt._tlas),
        );
        drop(new_rigid_builds);
        drop(skinned_builds);

        let previous_dynamic_count = rt.last_dynamic_instance_count;
        rt.last_scene_signature = signature;
        rt.last_dynamic_instance_count = populated_dynamic_slots;
        rt.active_masked_rigid = active_masked_rigid;
        rt.active_masked_skinned = active_masked_skinned;
        rt.active_rigid_triangles = active_rigid_triangles;
        rt.active_skinned_triangles = active_skinned_triangles;

        if previous_dynamic_count != populated_dynamic_slots || !new_rigid_keys.is_empty() {
            let inline_active = inline_transforms
                .iter()
                .enumerate()
                .filter(|(slot, transform)| {
                    transform.is_some()
                        && (rt.inline_blas[*slot].is_some() || rt.inline_mask_blas[*slot].is_some())
                })
                .count();
            rverbose!(
                2,
                "RT Shadows dynamic casters: inline BSP {inline_active}/{}, skinned GLM {}/{}, rigid MD3 {active_rigid_instances}/{}, alpha-tested dynamic {} (GLM {} + MD3 {}), cached skinned BLAS {}, cached rigid BLAS {} (+{})",
                rt.inline_slot_count,
                active_skinned.len(),
                rt.dynamic_instance_capacity,
                rt.dynamic_instance_capacity,
                active_masked_skinned + active_masked_rigid,
                active_masked_skinned,
                active_masked_rigid,
                rt.skinned_blas_cache.len(),
                rt.rigid_blas_cache.len(),
                new_rigid_keys.len()
            );
        }
    }
}

/// Produce the hardware-ray-query BSP shader from the authoritative raster BSP
/// shader. Keeping this as a narrow source transform means the RT variant shares
/// every JKA lighting/material rule with the normal shader; sun and local-light
/// visibility are replaced. The stock WGSL is still what all raster modes
/// compile.
/// Wrap the full light evaluation so attenuation, BRDF and visibility use the
/// same sample. In particular, `continue` must reject only the current sample.
pub(in crate::renderer) fn rt_sampled_light_loops(source: &str) -> Result<String, String> {
    const LOOP: &str = "for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {";
    const LIGHT: &str = "let light = dynamic_lights[cluster.indices[i]];";
    let mut result = source.to_owned();
    let starts = source
        .match_indices(LOOP)
        .map(|(start, _)| start)
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return Err("RT light loops not found".into());
    }
    for start in starts.into_iter().rev() {
        let body = start + LOOP.len();
        let mut depth = 1;
        let end = source[body..]
            .char_indices()
            .find_map(|(offset, c)| {
                if c == '{' {
                    depth += 1;
                }
                if c == '}' {
                    depth -= 1;
                }
                (depth == 0).then_some(body + offset)
            })
            .ok_or("unterminated RT light loop")?;
        let original = &source[body..end];
        if original.matches(LIGHT).count() != 1 {
            return Err("RT light lookup anchor changed".into());
        }
        // Local lights are always traced directly (never cached): they are
        // already gated by real cluster membership, so most receivers pay
        // nothing for lights that aren't nearby. Only the per-sample area-light
        // loop (independent of caching) is introduced here.
        let sampled = original.replace(LIGHT,
            "let light = rt_sample_local_emitter(rt_source, input, cluster.indices[i], rt_sample_index, rt_count);");
        result.replace_range(start..end, &format!("{LOOP}\n        let rt_source = dynamic_lights[cluster.indices[i]];\n        let rt_count = rt_local_sample_count(rt_source);\n        for (var rt_sample_index = 0u; rt_sample_index < rt_count; rt_sample_index += 1u) {{{sampled}\n        }}\n    "));
    }
    Ok(result)
}

/// A complete world shader: a variant's base source plus the modules every world
/// variant shares (the weather maths and forward weather shading, and surface
/// deformation).
pub(in crate::renderer) fn compose_world_shader(base: &str) -> String {
    format!(
        "{}\n{}",
        weather::with_world_weather(base),
        include_str!("../../surface_deformation.wgsl")
    )
}

pub(in crate::renderer) fn ray_traced_world_shader_source(base: &str) -> Result<String, String> {
    const BINDING: &str =
        "@group(3) @binding(7) var sky_admission_texture: texture_depth_2d_array;";
    const CASCADE_FN: &str =
        "fn cascaded_shadow_visibility(input: VertexOut, normal: vec3<f32>) -> f32 {";
    const LOCAL_FN: &str = "fn local_light_shadow_visibility(light: PointLight, world_position: vec3<f32>, receiver_normal: vec3<f32>) -> f32 {";
    const SURFACE_SUN: &str = "        let shadow_visibility = cascaded_shadow_visibility(input, input.world_normal);\n        let light_direction = normalize(shadow_settings.light_direction_enabled.xyz);\n        let sun_facing = max(dot(normalize(input.world_normal), -light_direction), 0.0);";

    if !base.contains(BINDING) {
        return Err("group-3 sky-admission binding anchor not found".to_string());
    }
    if !base.contains(CASCADE_FN) {
        return Err("cascaded_shadow_visibility anchor not found".to_string());
    }
    if !base.contains(LOCAL_FN) {
        return Err("local_light_shadow_visibility anchor not found".to_string());
    }
    // Both checked-in shaders can have CRLF on Windows. Normalize only the
    // generated RT variant so multiline source anchors are platform-independent.
    let base = base.replace("\r\n", "\n");
    if !base.contains(SURFACE_SUN) {
        return Err("surface sun contribution anchor not found".to_string());
    }

    let mut source = format!("enable wgpu_ray_query;\n{base}");
    source = source.replacen(
        "override ENABLE_CASCADED_SHADOWS: bool = false;",
        "override ENABLE_CASCADED_SHADOWS: bool = false;\noverride ENABLE_RAY_TRACED_SHADOWS: bool = false;\noverride ENABLE_RAY_TRACED_SUN: bool = false;",
        1,
    );
    source = source.replacen(
        BINDING,
        concat!(
            "@group(3) @binding(7) var sky_admission_texture: texture_depth_2d_array;\n",
            "@group(3) @binding(8) var rt_shadow_scene: acceleration_structure;\n",
            "\nstruct RtAlphaVertex {\n    position_u: vec4<f32>,\n    normal_v: vec4<f32>,\n    lightmap_alpha: vec4<f32>,\n};\n\nstruct RtAlphaGeometry {\n    values: vec4<u32>,\n};\n\nstruct RtAlphaMaterial {\n    header: vec4<u32>,\n    vector_s: vec4<f32>,\n    vector_t: vec4<f32>,\n    mods: array<vec4<f32>, 8>,\n    color: vec4<f32>,\n    params: vec4<f32>,\n    texture: vec4<u32>,\n};\n\nstruct RtAlphaTexture {\n    values: vec4<u32>,\n};\n\n@group(3) @binding(9) var<storage, read> rt_alpha_vertices: array<RtAlphaVertex>;\n@group(3) @binding(10) var<storage, read> rt_alpha_indices: array<u32>;\n@group(3) @binding(11) var<storage, read> rt_alpha_geometries: array<RtAlphaGeometry>;\n@group(3) @binding(12) var<storage, read> rt_alpha_materials: array<RtAlphaMaterial>;\n@group(3) @binding(13) var<storage, read> rt_alpha_textures: array<RtAlphaTexture>;\n@group(3) @binding(14) var<storage, read> rt_alpha_texels: array<u32>;"
        ),
        1,
    );

    // The shared ray-query core (sampling, alpha-tested candidate handling,
    // shadow-ray helpers). Lives in its own file so the world shader, the sun
    // compute pass and the model receivers all read one reviewable source.
    let rt_visibility = include_str!("../../rt_core.wgsl").replace("\r\n", "\n");
    source = source.replacen(
        CASCADE_FN,
        &format!(
            "{rt_visibility}{CASCADE_FN}\n    if (ENABLE_RAY_TRACED_SUN) {{\n        return ray_traced_shadow_visibility(input, normal);\n    }}"
        ),
        1,
    );

    source = source.replacen(
        LOCAL_FN,
        &format!(
            "{LOCAL_FN}\n    if (ENABLE_RAY_TRACED_SHADOWS) {{\n        return ray_traced_local_shadow_visibility(light, world_position, receiver_normal);\n    }}"
        ),
        1,
    );

    // The lean shader compiles the sun-shadow darkening out unless a cascade is
    // enabled. The RT sun is not a cascade, so open that gate for it. Absent in
    // bsp.wgsl, which keeps the block unconditional; the no-op replace is fine.
    source = source.replacen(
        "    if (ENABLE_CASCADED_SHADOWS && (material.header.z & 4u) != 0u) {\n        let shadow_visibility = cascaded_shadow_visibility(",
        "    if ((ENABLE_CASCADED_SHADOWS || ENABLE_RAY_TRACED_SUN) && (material.header.z & 4u) != 0u) {\n        let shadow_visibility = cascaded_shadow_visibility(",
        1,
    );

    // Ordinary BSP sun visibility is multiplied by sun_facing and strength.
    // Avoid a full traversal when that multiplier is zero. Keep this at the
    // contribution site: ocean specular uses the shared visibility helper too,
    // and must retain its own receiver rules.
    source = source.replacen(
        SURFACE_SUN,
        "        let light_direction = normalize(shadow_settings.light_direction_enabled.xyz);\n        let sun_facing = max(dot(normalize(input.world_normal), -light_direction), 0.0);\n        var shadow_visibility = 1.0;\n        if (sun_facing > 0.0 && shadow_settings.params.z != 0.0) {\n            shadow_visibility = cascaded_shadow_visibility(input, input.world_normal);\n        }",
        1,
    );

    // Sample before radius rejection and BRDF evaluation in every clustered
    // receiver path (legacy, PBR and ocean). Stock shaders keep point lighting.
    const CLUSTER_LIGHT: &str = "let light = dynamic_lights[cluster.indices[i]];";
    if source.matches(CLUSTER_LIGHT).count() < 2 {
        return Err("clustered emitter sampling anchors not found".to_string());
    }
    source = rt_sampled_light_loops(&source)?;
    source.push_str(include_str!("../../rt_resolution_read.wgsl"));

    // The enhanced ocean path has one outer guard that bypasses the common
    // visibility helper. Let the RT variant enter that same authoritative path.
    source = source.replace(
        "ENABLE_CASCADED_SHADOWS && shadow_settings.light_direction_enabled.w > 0.5",
        "(ENABLE_CASCADED_SHADOWS || ENABLE_RAY_TRACED_SUN) && shadow_settings.light_direction_enabled.w > 0.5",
    );
    Ok(source)
}

pub(in crate::renderer) fn rt_alpha_storage_entry<T: Pod>(
    binding: u32,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            // Validate table sizes at bind-group creation instead of first draw.
            min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<T>() as u64),
        },
        count: None,
    }
}

pub(in crate::renderer) fn rt_alpha_storage_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
    [
        rt_alpha_storage_entry::<RtAlphaVertexGpu>(9),
        rt_alpha_storage_entry::<u32>(10),
        rt_alpha_storage_entry::<RtAlphaGeometryGpu>(11),
        rt_alpha_storage_entry::<RtAlphaMaterialGpu>(12),
        rt_alpha_storage_entry::<RtAlphaTextureGpu>(13),
        rt_alpha_storage_entry::<u32>(14),
    ]
}

pub(in crate::renderer) fn create_ray_traced_shadow_receiver_layout(
    device: &wgpu::Device,
) -> wgpu::BindGroupLayout {
    let alpha_entries = rt_alpha_storage_entries();
    let cache_entries = rt_resolution::receiver_entries();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA hardware ray-traced sun/weather receiver layout"),
        entries: &[
            depth_array_texture_entry(0),
            comparison_sampler_entry(1),
            uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
            unfilterable_texture_entry_stages(
                4,
                wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
            ),
            uniform_entry(
                5,
                wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
            ),
            nonfiltering_sampler_entry(6, wgpu::ShaderStages::FRAGMENT),
            depth_array_texture_entry(7),
            wgpu::BindGroupLayoutEntry {
                binding: 8,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::AccelerationStructure {
                    vertex_return: false,
                },
                count: None,
            },
            alpha_entries[0],
            alpha_entries[1],
            alpha_entries[2],
            alpha_entries[3],
            alpha_entries[4],
            alpha_entries[5],
            cache_entries[0],
            cache_entries[1],
        ],
    })
}

impl RayTracedShadowResources {
    pub(in crate::renderer) fn scene(&self) -> RtReceiverScene<'_> {
        RtReceiverScene {
            tlas: &self._tlas,
            alpha_vertices: &self.alpha_vertex_buffer,
            alpha_indices: &self.alpha_index_buffer,
            alpha_geometries: &self.alpha_geometry_buffer,
            alpha_materials: &self.alpha_material_buffer,
            alpha_textures: &self.alpha_texture_buffer,
            alpha_texels: &self.alpha_texel_buffer,
        }
    }
}

/// The ray-query scene the receivers (and the sun compute pass) trace against.
pub(in crate::renderer) struct RtReceiverScene<'a> {
    pub(in crate::renderer) tlas: &'a wgpu::Tlas,
    pub(in crate::renderer) alpha_vertices: &'a wgpu::Buffer,
    pub(in crate::renderer) alpha_indices: &'a wgpu::Buffer,
    pub(in crate::renderer) alpha_geometries: &'a wgpu::Buffer,
    pub(in crate::renderer) alpha_materials: &'a wgpu::Buffer,
    pub(in crate::renderer) alpha_textures: &'a wgpu::Buffer,
    pub(in crate::renderer) alpha_texels: &'a wgpu::Buffer,
}

/// Group 3 of every ray-traced world/model pipeline: the ordinary sun-shadow
/// receiver resources plus the scene and the published sun-visibility cache.
/// One builder, because all three places that recreate it (initial build, alpha
/// table growth, receiver rebuild) must produce identical bindings.
pub(in crate::renderer) fn create_rt_receiver_bind_group(
    device: &wgpu::Device,
    shadow: &ShadowResources,
    fog_control_buffer: &wgpu::Buffer,
    cascaded_shadow_mode: DynamicShadowsMode,
    layout: &wgpu::BindGroupLayout,
    scene: RtReceiverScene<'_>,
    sun_shadow_history: &rt_resolution::RtSunShadowHistory,
) -> wgpu::BindGroup {
    let shadow_sampler = if cascaded_shadow_mode == DynamicShadowsMode::CascadedShadowMaps {
        &shadow.bevy_sampler
    } else {
        &shadow.legacy_sampler
    };
    let shadow_array_view = &shadow._array_view;
    let sky_array_view = &shadow.sky_array_view;
    let receiver_buffer = &shadow.receiver_buffer;
    let weather_height_view = &shadow.weather_height_view;
    let weather_sampler = &shadow.weather_sampler;
    let weather_surface_buffer = &shadow.weather_surface_buffer;
    let RtReceiverScene {
        tlas,
        alpha_vertices: alpha_vertex_buffer,
        alpha_indices: alpha_index_buffer,
        alpha_geometries: alpha_geometry_buffer,
        alpha_materials: alpha_material_buffer,
        alpha_textures: alpha_texture_buffer,
        alpha_texels: alpha_texel_buffer,
    } = scene;
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA hardware ray-traced sun/weather receiver bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(shadow_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: receiver_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: fog_control_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(weather_height_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: weather_surface_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(weather_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(sky_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: tlas.as_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: alpha_vertex_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: alpha_index_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: alpha_geometry_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: alpha_material_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: alpha_texture_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: alpha_texel_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 15,
                resource: sun_shadow_history.flags_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: wgpu::BindingResource::TextureView(&sun_shadow_history.published),
            },
        ],
    })
}

pub(in crate::renderer) fn create_shadow_receiver_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shadow_array_view: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
    sky_array_view: &wgpu::TextureView,
    receiver_buffer: &wgpu::Buffer,
    fog_control_buffer: &wgpu::Buffer,
    weather_height_view: &wgpu::TextureView,
    weather_sampler: &wgpu::Sampler,
    weather_surface_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA cascaded sun/weather receiver bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(shadow_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: receiver_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: fog_control_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(weather_height_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: weather_surface_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(weather_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(sky_array_view),
            },
        ],
    })
}
