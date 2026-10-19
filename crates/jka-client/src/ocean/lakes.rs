//! Promoted JKA footprints drawn with Blue Water's lake material, without an ocean simulation.
use super::{OceanGpu, OceanPvsTileMask, OCEAN_MAX_SURFACES, OCEAN_UNITS_PER_METER};
use blue_water_gpu::{
    lakes::{LakeBindings, LakePipeline, LakeUniforms, LakeVertex},
    map_environment::MapEnvironment,
    original_surface::SurfaceBoundary,
    scene_copy::SceneCopy,
    sky_environment::{CelestialLight, EnvironmentUniforms},
};
use bytemuck::Zeroable;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LakeSettings {
    pub wind_speed: f32,
    pub direction: f32,
    pub transparency: f32,
    pub salt: bool,
}
impl Default for LakeSettings {
    fn default() -> Self {
        Self {
            wind_speed: 4.0,
            direction: 0.0,
            transparency: 1.0,
            salt: false,
        }
    }
}
impl LakeSettings {
    pub fn sanitize(mut self) -> Self {
        fn finite(v: f32, d: f32, lo: f32, hi: f32) -> f32 {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                d
            }
        }
        self.wind_speed = finite(self.wind_speed, 4.0, 0.0, 40.0);
        self.direction = finite(self.direction, 0.0, -180.0, 180.0);
        self.transparency = finite(self.transparency, 1.0, 0.1, 10.0);
        self
    }
}
struct Slot {
    bindings: LakeBindings,
    vertices: wgpu::Buffer,
    count: u32,
    geometry_key: (usize, [u32; 4], bool),
    ready: bool,
}
pub(super) struct LakeRenderer {
    key: (u32, u32, u32, wgpu::TextureFormat),
    pub scene: SceneCopy,
    pipeline: LakePipeline,
    slots: [Option<Slot>; OCEAN_MAX_SURFACES],
    map: MapEnvironment,
    faces: Option<[wgpu::TextureView; 6]>,
    environment: Option<wgpu::BindGroup>,
    environment_view: Option<wgpu::TextureView>,
    resources: Option<(Option<wgpu::TextureView>, Option<wgpu::TextureView>)>,
}

// Exact footprint triangles are stationary; only their shading animates. Fetch follows upstream's sqrt(area).
fn vertices(bounds: [f32; 4], triangles: &[[f32; 4]], salt: bool) -> Vec<LakeVertex> {
    let mut points = Vec::new();
    for t in triangles.chunks_exact(2) {
        points.extend_from_slice(&[[t[0][0], t[0][1]], [t[0][2], t[0][3]], [t[1][0], t[1][1]]]);
    }
    if points.is_empty() {
        let [x0, z0, x1, z1] = bounds;
        points.extend_from_slice(&[[x0, z0], [x0, z1], [x1, z0], [x1, z0], [x0, z1], [x1, z1]]);
    }
    let area: f32 = points
        .chunks_exact(3)
        .map(|t| {
            ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0]))
                .abs()
                * 0.5
        })
        .sum();
    let fetch = area.sqrt() / OCEAN_UNITS_PER_METER;
    points
        .into_iter()
        .map(|p| LakeVertex {
            position: [
                p[0] / OCEAN_UNITS_PER_METER,
                0.0,
                p[1] / OCEAN_UNITS_PER_METER,
            ],
            lake: [fetch, f32::from(salt), 0.5, 0.0],
        })
        .collect()
}
impl OceanGpu {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_lakes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &crate::camera::Camera,
        key: (u32, u32, u32, wgpu::TextureFormat),
        jitter: [f32; 2],
        dt: f32,
        frame: u32,
        pvs: &[Option<OceanPvsTileMask>; OCEAN_MAX_SURFACES],
        sky_faces: Option<[&wgpu::TextureView; 6]>,
        sky: Option<&blue_water_sky::EnvironmentMap>,
        aerial: Option<&blue_water_render::AerialVolume>,
        cloud: Option<&blue_water_clouds::CloudShadow>,
    ) {
        if self.lakes.as_ref().is_none_or(|r| r.key != key) {
            let map = MapEnvironment::new(device);
            // Stand-in ambient when the map has no skybox. Native sky or map faces replace it below.
            let pixel: [u16; 4] = [0x3400, 0x3600, 0x3800, 0x3c00];
            let pixels = vec![pixel; (map.map.layout.width * map.map.layout.height) as usize];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &map.map.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&pixels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(map.map.layout.width * 8),
                    rows_per_image: Some(map.map.layout.height),
                },
                wgpu::Extent3d {
                    width: map.map.layout.width,
                    height: map.map.layout.height,
                    depth_or_array_layers: 1,
                },
            );
            self.lakes = Some(LakeRenderer {
                key,
                scene: SceneCopy::new(device, key.0, key.1, key.2),
                pipeline: LakePipeline::new(device, key.3, key.2),
                slots: std::array::from_fn(|_| None),
                map,
                faces: None,
                environment: None,
                environment_view: None,
                resources: None,
            });
        }
        let r = self.lakes.as_mut().unwrap();
        let faces = sky_faces.map(|f| f.map(Clone::clone));
        if r.faces != faces {
            if let Some(faces) = sky_faces {
                r.map.bake(device, encoder, faces);
            }
            r.faces = faces;
        }
        let fallback = &r.map.map;
        let light = CelestialLight {
            direction: glam::Vec3::from_array(
                self.environment.sun_direction_intensity[..3]
                    .try_into()
                    .unwrap(),
            ) * -1.0,
            color: glam::Vec3::from_array(self.environment.sun_color[..3].try_into().unwrap()),
            intensity: self.environment.sun_direction_intensity[3],
            night: false,
        };
        queue.write_buffer(
            &fallback.uniforms,
            0,
            bytemuck::bytes_of(&EnvironmentUniforms::new(
                &fallback.layout,
                0,
                &light,
                light.direction,
                -light.direction,
                0.5,
            )),
        );
        let (view, sampler, uniforms) = sky.map_or(
            (&fallback.view, &fallback.sampler, &fallback.uniforms),
            |s| (&s.view, &s.sampler, &s.uniforms),
        );
        if r.environment_view.as_ref() != Some(view) {
            r.environment = Some(r.pipeline.environment(device, view, sampler, uniforms));
            r.environment_view = Some(view.clone());
        }
        let resources = (
            aerial.map(|a| a.volume_view.clone()),
            cloud.map(|c| c.view.clone()),
        );
        if r.resources.as_ref() != Some(&resources) {
            // Bindings retain the viewport copies and live atmosphere resources between frames.
            for slot in r.slots.iter_mut().flatten() {
                slot.bindings = r.pipeline.bindings(
                    device,
                    &r.scene.color,
                    &r.scene.depth,
                    aerial.map(|a| (&a.volume_view, &a.sampler)),
                    cloud.map(|c| (&c.view, &c.sampler, &c.uniforms)),
                );
            }
            r.resources = Some(resources);
        }
        for slot in r.slots.iter_mut().flatten() {
            slot.ready = false;
        }
        for promoted in &self.promoted_surfaces {
            let index = promoted.descriptor.index;
            if index >= OCEAN_MAX_SURFACES || pvs[index].is_some_and(|p| p.iter().all(|w| *w == 0))
            {
                continue;
            }
            let surface = promoted.descriptor.surface;
            let bounds = [
                surface.minimum[0],
                surface.minimum[1],
                surface.maximum[0],
                surface.maximum[1],
            ];
            let start = self.masks.ranges[index][0] as usize * 2;
            let count = self.masks.ranges[index][1] as usize * 2;
            let settings = self.settings.lake;
            let geometry_key = (
                std::sync::Arc::as_ptr(&self.masks) as usize,
                bounds.map(f32::to_bits),
                settings.salt,
            );
            if r.slots[index]
                .as_ref()
                .is_none_or(|s| s.geometry_key != geometry_key)
            {
                let geometry = vertices(
                    bounds,
                    &self.masks.triangles[start..start + count],
                    settings.salt,
                );
                let bindings = r.slots[index]
                    .take()
                    .map(|s| s.bindings)
                    .unwrap_or_else(|| {
                        r.pipeline.bindings(
                            device,
                            &r.scene.color,
                            &r.scene.depth,
                            aerial.map(|a| (&a.volume_view, &a.sampler)),
                            cloud.map(|c| (&c.view, &c.sampler, &c.uniforms)),
                        )
                    });
                r.slots[index] = Some(Slot {
                    bindings,
                    vertices: LakePipeline::vertices(device, &geometry),
                    count: geometry.len() as u32,
                    geometry_key,
                    ready: false,
                });
            }
            let slot = r.slots[index].as_mut().unwrap();
            let camera = super::original_surface::surface_camera(
                camera,
                surface.plane_height,
                key.0,
                key.1,
                jitter,
                self.elapsed,
                dt,
                frame,
            );
            queue.write_buffer(&slot.bindings.camera, 0, bytemuck::bytes_of(&camera));
            queue.write_buffer(
                &slot.bindings.uniforms,
                0,
                bytemuck::bytes_of(&LakeUniforms::new(
                    settings.wind_speed,
                    settings.direction.to_radians(),
                    settings.transparency,
                    aerial.is_some(),
                )),
            );
            let mut boundary = SurfaceBoundary::zeroed();
            boundary.bounds = bounds;
            boundary.scale = [OCEAN_UNITS_PER_METER, 0.0, 0.0, 0.0];
            // The mesh itself is the exact outline; only PVS admission is needed per pixel.
            if let Some(pvs) = pvs[index] {
                boundary.counts[2] = 1;
                for (to, from) in boundary.pvs.iter_mut().zip(pvs.chunks_exact(4)) {
                    to.copy_from_slice(from);
                }
            }
            queue.write_buffer(&slot.bindings.boundary, 0, bytemuck::bytes_of(&boundary));
            slot.ready = true;
        }
    }
}
impl LakeRenderer {
    pub fn triangles(&self, index: usize) -> Option<u64> {
        self.slots
            .get(index)?
            .as_ref()
            .filter(|s| s.ready)
            .map(|s| u64::from(s.count) / 3)
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, admitted: u8) -> (u32, u32, u64) {
        let Some(env) = &self.environment else {
            return (0, 0, 0);
        };
        pass.set_pipeline(&self.pipeline.pipeline);
        pass.set_bind_group(2, env, &[]);
        let mut draws = 0;
        for (i, slot) in self.slots.iter().enumerate() {
            let Some(s) = slot.as_ref().filter(|s| s.ready && admitted >> i & 1 != 0) else {
                continue;
            };
            pass.set_bind_group(0, &s.bindings.camera_group, &[]);
            pass.set_bind_group(1, &s.bindings.material, &[]);
            pass.set_vertex_buffer(0, s.vertices.slice(..));
            pass.draw(0..s.count, 0..1);
            draws += 1;
        }
        (draws, 0, 0)
    }
}
