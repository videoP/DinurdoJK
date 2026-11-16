//! Cold-path Phong normals for live sky irradiance. No lightmap bake is used.
use crate::renderer::{GpuVertex, Renderer, Vec3};
use std::collections::{HashMap, HashSet};
use wgpu::util::DeviceExt;

/// Coincident-position, corner-angle weighted smoothing, with a non-transitive
/// 45 degree crease limit. Duplicate material stages must not add extra votes.
fn smooth_normals<'a>(
    vertices: &[GpuVertex], indices: &[u32], ranges: impl Iterator<Item = &'a std::ops::Range<u32>>,
) -> Vec<[f32; 3]> {
    let mut corners: HashMap<[i32; 3], Vec<(Vec3, f32)>> = HashMap::new();
    let mut seen = HashSet::new();
    let point = |p: [f32; 3]| p.map(|v| (v * 32.0).round() as i32);
    for range in ranges {
        let Some(slice) = indices.get(range.start as usize..range.end as usize) else { continue; };
        for triangle in slice.chunks_exact(3) {
            let mut key = [triangle[0], triangle[1], triangle[2]];
            key.sort_unstable();
            if !seen.insert(key) { continue; }
            let [Some(a), Some(b), Some(c)] = [vertices.get(triangle[0] as usize),
                vertices.get(triangle[1] as usize), vertices.get(triangle[2] as usize)] else { continue; };
            let v = [a, b, c];
            for i in 0..3 {
                let p = Vec3::from_array(v[i].position);
                let a = (Vec3::from_array(v[(i + 1) % 3].position) - p).normalize_or_zero();
                let b = (Vec3::from_array(v[(i + 2) % 3].position) - p).normalize_or_zero();
                let normal = Vec3::from_array(v[i].normal).normalize_or_zero();
                if a.length_squared() < 0.5 || b.length_squared() < 0.5
                    || !normal.is_finite() || normal.length_squared() < 0.5 { continue; }
                let weight = a.dot(b).clamp(-1.0, 1.0).acos();
                corners.entry(point(v[i].position)).or_default().push((normal, weight));
            }
        }
    }
    vertices.iter().map(|v| {
        let normal = Vec3::from_array(v.normal).normalize_or_zero();
        let Some(votes) = corners.get(&point(v.position)) else { return normal.to_array(); };
        let sum = votes.iter().filter(|(other, _)| normal.dot(*other) >= 0.707_106_77)
            .fold(Vec3::ZERO, |sum, &(other, weight)| sum + other * weight).normalize_or_zero();
        if sum.is_finite() && sum.length_squared() > 0.5 { sum.to_array() } else { normal.to_array() }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtime_sky_normals_keep_creases_and_ignore_duplicate_material_stages() {
        use bytemuck::Zeroable;
        let angle = 30f32.to_radians();
        let mut vertices = Vec::new();
        let mut triangle = |positions: [[f32; 3]; 3], normal: Vec3| {
            vertices.extend(positions.map(|position| GpuVertex { position, normal: normal.to_array(), ..GpuVertex::zeroed() }));
        };
        triangle([[0., 0., 0.], [0., 0., 2.], [2., 0., 0.]], Vec3::Y);
        triangle([[0., 0., 2.], [0., 0., 0.], [-2. * angle.cos(), -2. * angle.sin(), 1.]],
            Vec3::new(-angle.sin(), angle.cos(), 0.));
        triangle([[0., 0., 0.], [0., -2., 0.], [0., 0., 2.]], Vec3::X);
        let indices: Vec<_> = (0..9).collect();
        let normal = smooth_normals(&vertices, &indices, [0..9].iter());
        let repeated = smooth_normals(&vertices, &indices, [0..9, 0..3, 0..3].iter());
        assert_eq!(normal, repeated, "multipass materials must not bias normal weighting");
        assert!(normal[0][0] < -0.1 && normal[0][1] > 0.9);
        assert!((Vec3::from_array(normal[0]) - Vec3::from_array(normal[4])).length() < 1e-6);
        assert_eq!(normal[6], Vec3::X.to_array(), "90 degree corners must remain hard");
        assert_eq!(normal[2], Vec3::Y.to_array(), "unconnected corners remain authored");
        assert_eq!(vertices[0].normal, Vec3::Y.to_array(), "authored material normals are immutable");
    }

    #[test]
    fn realtime_sky_normal_shaders_validate() {
        let source = crate::material_flags::depth_shader();
        let depth = wgpu::naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::default()).validate(&depth)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        let sky = blue_water_clouds::shaders::compose(
            include_str!("../environment/blue_water_scene.wgsl"), "realtime sky smooth normals", &[])
            .expect("native sky shader composition");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::default()).validate(&sky).expect("native sky shader validation");
        for name in ["vs_sky", "vs_sky_mask", "fs_sky", "fs_sky_mask"] {
            assert!(depth.entry_points.iter().any(|e| e.name == name));
        }
    }
}

pub(in crate::renderer) struct RealtimeSkyNormals {
    pub opaque: wgpu::RenderPipeline,
    pub masked: wgpu::RenderPipeline,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

fn target(device: &wgpu::Device, size: (u32, u32)) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("realtime sky Phong normals (octahedral)"),
        size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

impl Renderer {
    pub(in crate::renderer) fn realtime_sky_normals_active(&self) -> bool {
        self.blue_water_environment.world_lighting > 0.0
            && self.blue_sky_diag & 8 == 0 && self.thermal_vision == 0 && self.world.is_some()
    }

    /// Lazy once per map for vertices, once per renderer for pipelines, and
    /// once per render-size change for the target. Time/sun never enter a key.
    pub(in crate::renderer) fn ensure_realtime_sky_normals(&mut self) {
        if !self.realtime_sky_normals_active() { return; }
        let world = self.world.as_mut().unwrap();
        if world.sky_normal_buffer.is_none() {
            let ranges = world.full_batches.iter().filter(|b| !b.is_inline_entity
                && b.source.pipeline.class != crate::scene::DrawClass::Sky
                && !b.source.pipeline.deform_move && b.source.pipeline.depth_write
                && !b.source.pipeline.depth_equal).map(|b| &b.full_indexed_range);
            let mut normals = smooth_normals(&world.static_ao_gpu_vertices, &world.remap_index_source, ranges);
            // Inline models move independently: leave their entries invalid,
            // so the fullscreen pass retains geometric normals there.
            normals.resize((world.vertex_buffer.size() / std::mem::size_of::<GpuVertex>() as u64) as usize, [0.0; 3]);
            world.sky_normal_buffer = Some(self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("static world sky Phong normal stream"),
                contents: bytemuck::cast_slice(&normals), usage: wgpu::BufferUsages::VERTEX,
            }));
        }
        let size = self.targets.render_size;
        if self.realtime_sky_normals.is_none() {
            let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("realtime sky normal prepass"),
                source: wgpu::ShaderSource::Wgsl(crate::material_flags::depth_shader().into()),
            });
            let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("realtime sky normal prepass"),
                bind_group_layouts: &[Some(&self.camera_layout), Some(&self.surface_layout), Some(&self.planar_reflection_layout)],
                immediate_size: 0,
            });
            let opaque = super::pipelines::create_depth_prepass_pipeline_with_entry(&self.device, &layout, &shader, "fs_sky");
            let masked = super::pipelines::create_depth_prepass_mask_pipeline_with_entry(&self.device, &layout, &shader, "fs_sky_mask");
            let (texture, view) = target(&self.device, size);
            self.realtime_sky_normals = Some(RealtimeSkyNormals { opaque, masked, texture, view });
        } else if let Some(state) = self.realtime_sky_normals.as_mut() {
            if (state.texture.width(), state.texture.height()) != size {
                (state.texture, state.view) = target(&self.device, size);
            }
        }
    }
}
