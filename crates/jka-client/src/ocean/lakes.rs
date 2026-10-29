//! Promoted JKA footprints on cheap, gently displaced clipmaps, without an ocean simulation.
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
    /// Maximum displacement above/below the map plane, in inches.
    pub amplitude: f32,
    pub speed: f32,
    pub water_color: [f32; 3],
    pub tint_color: [f32; 3],
    pub fog_color: [f32; 3],
    pub reflections: f32,
    /// 0 original optical material, 1 Cheapwater material.
    pub shader: u8,
    /// Lake-local: 0 sky, 1 SSR, 2 planar. Independent of global reflection settings.
    pub reflection_mode: u8,
    pub wind_speed: f32,
    pub direction: f32,
    pub transparency: f32,
    pub cheap_transmission: f32,
    pub cheap_refraction: f32,
    /// Vertical depth in feet where Cheapwater bed visibility reaches zero.
    pub opaque_depth: f32,
    pub wake_strength: f32,
    /// Use the source JKA shader's per-pool appearance and motion cues.
    /// Disabled is bit-for-bit equivalent in uniforms to the old global lake path.
    pub source_hints: bool,
    pub shallow_color: [f32; 3],
    pub deep_color: [f32; 3],
    pub depth_color_strength: f32,
    pub color_depth: f32,
}
impl Default for LakeSettings {
    fn default() -> Self {
        Self {
            amplitude: 6.65,
            speed: 1.45,
            water_color: [0.41818184, 0.9619174, 1.0],
            tint_color: [0.09994107; 3],
            fog_color: [0.0, 0.00003021201, 0.01662098],
            reflections: 0.525,
            shader: 1,
            reflection_mode: 2,
            wind_speed: 0.0,
            direction: 52.0,
            transparency: 0.5,
            cheap_transmission: 1.0,
            cheap_refraction: 0.026,
            opaque_depth: 8.0,
            wake_strength: 10.0,
            source_hints: true,
            shallow_color: [0.0, 0.043898553, 0.025940072],
            deep_color: [0.0, 0.08160421, 0.061574105],
            depth_color_strength: 0.52,
            color_depth: 52.0,
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
        let defaults = Self::default();
        self.amplitude = finite(self.amplitude, defaults.amplitude, 0.0, 12.0);
        self.speed = finite(self.speed, defaults.speed, 0.0, 3.0);
        self.reflections = finite(self.reflections, defaults.reflections, 0.0, 1.0);
        self.shader = self.shader.min(1);
        self.reflection_mode = self.reflection_mode.min(2);
        self.water_color =
            std::array::from_fn(|i| finite(self.water_color[i], defaults.water_color[i], 0.0, 1.0));
        self.tint_color =
            std::array::from_fn(|i| finite(self.tint_color[i], defaults.tint_color[i], 0.0, 1.0));
        self.fog_color =
            std::array::from_fn(|i| finite(self.fog_color[i], defaults.fog_color[i], 0.0, 1.0));
        self.wind_speed = finite(self.wind_speed, defaults.wind_speed, 0.0, 40.0);
        self.direction = finite(self.direction, defaults.direction, -180.0, 180.0);
        self.transparency = finite(self.transparency, defaults.transparency, 0.1, 10.0);
        self.cheap_transmission = finite(
            self.cheap_transmission,
            defaults.cheap_transmission,
            0.0,
            1.0,
        );
        self.cheap_refraction = finite(self.cheap_refraction, defaults.cheap_refraction, 0.0, 0.05);
        self.opaque_depth = finite(self.opaque_depth, defaults.opaque_depth, 0.25, 100.0);
        self.wake_strength = finite(self.wake_strength, defaults.wake_strength, 0.0, 10.0);
        self.shallow_color = std::array::from_fn(|i| {
            finite(self.shallow_color[i], defaults.shallow_color[i], 0.0, 1.0)
        });
        self.deep_color =
            std::array::from_fn(|i| finite(self.deep_color[i], defaults.deep_color[i], 0.0, 1.0));
        self.depth_color_strength = finite(
            self.depth_color_strength,
            defaults.depth_color_strength,
            0.0,
            1.0,
        );
        self.color_depth = finite(self.color_depth, defaults.color_depth, 0.1, 100.0);
        self
    }
    pub fn needs_scene_color(self) -> bool {
        self.shader == 0
            || self.cheap_transmission > 0.0
            || (self.reflections > 0.0 && self.reflection_mode == 1)
    }
    pub fn needs_scene_copy(self) -> bool {
        self.needs_scene_color() || self.depth_color_strength > 0.0
    }
}

/// Lake-specific interpretation of the existing Godot promotion descriptor.
/// The ocean's 10-unit amplitude floor and FFT-derived chop/foam are not lake
/// concepts: use the saved lake controls as reference scales, not replacements.
fn lake_source_settings(mut lake: LakeSettings, source: &super::appearance::PromotedOcean) -> LakeSettings {
    if !lake.source_hints {
        return lake;
    }
    let variation = source.heuristic.variation.clamp(0.0, 1.0);
    // Explicit deformVertexes/bulge controls physical displacement; otherwise
    // texture variation is merely weak evidence of surface activity. A slider
    // set to zero must remain a true zero even when source geometry was wavy.
    let amplitude_ratio = source.source_deformation
        .filter(|a| a.is_finite() && *a > 0.0)
        .map_or(0.65 + 0.55 * variation, |a| (a / 6.65).clamp(0.15, 2.0));
    lake.amplitude = (lake.amplitude * amplitude_ratio).clamp(0.0, 12.0);
    lake.speed = (lake.speed * (0.75 + 0.50 * variation)).clamp(0.0, 3.0);

    // color is already the source texture/stage/vertex tint in sRGB. Baked
    // brightness and foam highlights aren't physically meaningful absorption;
    // keep only its hue, while the lake sliders still set absolute brightness.
    let color = source.color;
    let peak = color.iter().copied().fold(0.0f32, f32::max);
    let low = color.iter().copied().fold(1.0f32, f32::min);
    if color.iter().all(|c| c.is_finite()) && peak > 0.02 && peak - low > peak * 0.08 {
        let hue = color.map(|c| (c / peak).clamp(0.0, 1.0));
        let recolor = |original: [f32; 3]| {
            let intensity = original.iter().copied().fold(0.0f32, f32::max);
            hue.map(|c| c * intensity)
        };
        lake.water_color = recolor(lake.water_color);
        // Cheapwater blends in its separate shallow/deep palette. That needs
        // the same source hue or a red water shader turns green with depth.
        lake.shallow_color = recolor(lake.shallow_color);
        lake.deep_color = recolor(lake.deep_color);
    }

    // Only conventional SRC_ALPHA/ONE_MINUS_SRC_ALPHA carries a trustworthy
    // optical alpha cue. Preserve the user's zero-transmission override.
    if let Some(alpha) = source.source_alpha.filter(|a| a.is_finite()) {
        lake.transparency = (lake.transparency * (1.0 + (1.0 - alpha.clamp(0.0, 1.0)) * 1.5))
            .clamp(0.1, 10.0);
    }
    lake
}

struct Slot {
    bindings: LakeBindings,
    boundary: SurfaceBoundary,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    mesh: LakeMesh,
    first: usize,
    shore: wgpu::TextureView,
    count: u32,
    geometry_key: (usize, [u32; 4], u32),
    horizon: bool,
    ready: bool,
}
pub(super) struct LakeRenderer {
    key: (u32, u32, u32, wgpu::TextureFormat),
    pub scene: SceneCopy,
    pipeline: LakePipeline,
    noise: wgpu::TextureView,
    normal: wgpu::TextureView,
    cheap: bool,
    slots: [Option<Slot>; OCEAN_MAX_SURFACES],
    map: MapEnvironment,
    faces: Option<[wgpu::TextureView; 6]>,
    environment: Option<wgpu::BindGroup>,
    environment_view: Option<wgpu::TextureView>,
    pub(super) resources: Option<(Option<wgpu::TextureView>, Option<wgpu::TextureView>)>,
    planar_view: Option<wgpu::TextureView>,
    ripple_view: Option<wgpu::TextureView>,
    submerged_views: Option<(wgpu::TextureView, wgpu::TextureView)>,
    ripples: Option<blue_water_gpu::lake_ripples::RippleAtlas>,
    wake_tracker: super::lake_wakes::LakeWakes,
}

// 32 cells across, 0.5m near spacing, doubling toward a finite outer ring.
// Keep the upstream snapping/morph topology. Sky-qualified water adds a flat,
// eight-triangle continuation beyond the quiet outer ring.
struct LakeMesh {
    map: blue_water_gpu::geometry::Clipmap,
    scale: f32,
    vertices: Vec<LakeVertex>,
    indices: Vec<u32>,
    fills: Vec<std::ops::Range<u32>>,
    rings: Vec<u32>,
}
impl LakeMesh {
    fn new(bounds: [f32; 4], triangles: &[[f32; 4]]) -> Self {
        Self::extended(bounds, triangles, None)
    }
    fn extended(bounds: [f32; 4], triangles: &[[f32; 4]], horizon: Option<f32>) -> Self {
        let extent = (bounds[2] - bounds[0]).max(bounds[3] - bounds[1]) / OCEAN_UNITS_PER_METER;
        let mut levels = if horizon.is_some() { 4 } else { 1 };
        while levels < 10 && 8.0 * ((1u32 << (levels - 1)) as f32) * 0.9 < extent {
            levels += 1;
        }
        let map = blue_water_gpu::geometry::Clipmap::new(32, levels);
        let scale = (0.5 / map.cell(0) as f32).max(extent / (4096.0 * 0.9));
        let built = blue_water_gpu::geometry::build(map, 512.0);
        let end = built.rings[levels - 1] + 32 * 32 * 3 / 4 * 6;
        let area: f32 = triangles
            .chunks_exact(2)
            .map(|t| {
                let [ax, az, bx, bz] = t[0];
                ((bx - ax) * (t[1][1] - az) - (bz - az) * (t[1][0] - ax)).abs() * 0.5
            })
            .sum();
        let fetch = if area > 0.0 {
            area.sqrt() / OCEAN_UNITS_PER_METER
        } else {
            extent
        };
        let vertices = built
            .vertices
            .iter()
            .take((33 * 33 * levels) as usize)
            .map(|v| LakeVertex {
                position: [v.position[0] * scale, 0.0, v.position[1] * scale],
                lake: [fetch, 0.0, v.level[0], 0.0],
            })
            .collect();
        let mut mesh = Self {
            map,
            scale,
            vertices,
            indices: built.indices[..end as usize].to_vec(),
            fills: built.fills,
            rings: built.rings,
        };
        if let Some(far) = horizon {
            // Eight triangles connect the quiet outer ring to the far plane.
            // Exact sky-contact clipping still runs per pixel. At least four
            // levels put this join beyond the 32m displacement fade.
            let inner = map.half(levels - 1) as f32 * scale;
            let outer = far.max(inner * 2.0);
            let corners = [[-1., -1.], [-1., 1.], [1., 1.], [1., -1.]];
            for side in 0..4 {
                let a = corners[side];
                let b = corners[(side + 1) % 4];
                let base = mesh.vertices.len() as u32;
                for (p, radius) in [(a, inner), (b, inner), (a, outer), (b, outer)] {
                    mesh.vertices.push(LakeVertex {
                        position: [p[0] * radius, 0., p[1] * radius],
                        lake: [fetch, 0., (levels - 1) as f32, 1.],
                    });
                }
                mesh.indices
                    .extend([base, base + 1, base + 2, base + 2, base + 1, base + 3]);
            }
        }
        mesh
    }
    fn count(&self, first: usize) -> u32 {
        self.fills[first].len() as u32 + self.indices.len() as u32 - self.rings[first]
    }
}

// Cached shoreline attenuation, sampled once per vertex. Conservative texel padding
// pins the real outline (including hole rims); no per-frame edge searches are needed.
fn shore_pixels(bounds: [f32; 4], edges: &[[f32; 4]], size: u32) -> Vec<u8> {
    let step = [
        (bounds[2] - bounds[0]) / size as f32,
        (bounds[3] - bounds[1]) / size as f32,
    ];
    let padding = step[0].hypot(step[1]);
    (0..size * size)
        .map(|i| {
            let p = [
                bounds[0] + (i % size) as f32 * step[0] + step[0] * 0.5,
                bounds[1] + (i / size) as f32 * step[1] + step[1] * 0.5,
            ];
            let distance = edges
                .iter()
                .map(|e| {
                    let d = [e[2] - e[0], e[3] - e[1]];
                    let t = (((p[0] - e[0]) * d[0] + (p[1] - e[1]) * d[1])
                        / (d[0] * d[0] + d[1] * d[1]).max(1e-8))
                    .clamp(0.0, 1.0);
                    (p[0] - e[0] - t * d[0]).hypot(p[1] - e[1] - t * d[1])
                })
                .fold(f32::INFINITY, f32::min);
            let t = ((distance - padding) / (0.75 * OCEAN_UNITS_PER_METER)).clamp(0.0, 1.0);
            (t * t * (3.0 - 2.0 * t) * 255.0) as u8
        })
        .collect()
}
fn shoreline(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bounds: [f32; 4],
    edges: &[[f32; 4]],
) -> wgpu::TextureView {
    const SIZE: u32 = 128;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cached lake shoreline fade"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &shore_pixels(bounds, edges, SIZE),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SIZE),
            rows_per_image: Some(SIZE),
        },
        texture.size(),
    );
    texture.create_view(&Default::default())
}
impl OceanGpu {
    pub(crate) fn set_lake_actors(
        &mut self,
        sources: &std::sync::Arc<Vec<super::interaction::ActorSample>>,
    ) {
        if self.settings.backend == 3 {
            self.lake_actors = std::sync::Arc::clone(sources);
        }
    }
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
        sky_pvs_mask: u8,
        sky_faces: Option<[&wgpu::TextureView; 6]>,
        sky: Option<&blue_water_sky::EnvironmentMap>,
        aerial: Option<&blue_water_render::AerialVolume>,
        cloud: Option<&blue_water_clouds::CloudShadow>,
        seconds: f32,
        planar: Option<(&wgpu::TextureView, &wgpu::Sampler, &[Option<[f32; 4]>; 4])>,
        submerged: Option<(&wgpu::TextureView, &wgpu::TextureView)>,
        volumes: &super::optics::Uniform,
        camera_water_surface: f32,
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
                noise: LakePipeline::noise(
                    device,
                    queue,
                    &super::gerstner_patterns::lake_noise(),
                    128,
                ),
                normal: self.lake_normal.clone(),
                cheap: self.settings.lake.shader == 1,
                slots: std::array::from_fn(|_| None),
                map,
                faces: None,
                environment: None,
                environment_view: None,
                resources: None,
                planar_view: None,
                ripple_view: None,
                submerged_views: None,
                ripples: None,
                wake_tracker: Default::default(),
            });
        }
        let r = self.lakes.as_mut().unwrap();
        r.cheap = self.settings.lake.shader == 1;
        let ripple_origin = super::lake_wakes::LakeWakes::origin(camera.position.to_array());
        let wake_enabled = r.cheap && self.settings.lake.wake_strength > 0.0;
        let wake_visible: [_; OCEAN_MAX_SURFACES] = std::array::from_fn(|i| {
            let sky = self.sky_extend_enabled
                && sky_pvs_mask & (1u8 << i) != 0
                && self.sky_extend_regions[i].is_some();
            Self::native_pool_visible(pvs[i], sky)
        });
        if wake_enabled {
            let bodies: Vec<_> = self
                .promoted_surfaces
                .iter()
                .filter(|p| {
                    p.descriptor.index < OCEAN_MAX_SURFACES && wake_visible[p.descriptor.index]
                })
                .map(|p| {
                    let i = p.descriptor.index;
                    let sky = (self.sky_extend_enabled && sky_pvs_mask & (1u8 << i) != 0)
                        .then_some(self.sky_extend_regions[i])
                        .flatten();
                    (i, p.descriptor.surface, sky)
                })
                .collect();
            r.wake_tracker.update(
                &self.lake_actors,
                seconds,
                camera.position.to_array(),
                &bodies,
                &self.masks,
            );
        } else {
            r.wake_tracker.clear();
        }
        let ripple_stamps: Vec<_> = (0..OCEAN_MAX_SURFACES)
            .map(|i| {
                if wake_visible[i] {
                    r.wake_tracker.stamps(
                        i,
                        seconds,
                        ripple_origin,
                        self.settings.lake.wake_strength,
                    )
                } else {
                    Vec::new()
                }
            })
            .collect();
        if r.ripples.is_none() && ripple_stamps.iter().any(|s| !s.is_empty()) {
            r.ripples = Some(blue_water_gpu::lake_ripples::RippleAtlas::new(device));
        }
        if let Some(ripples) = &r.ripples {
            for (i, stamps) in ripple_stamps
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.is_empty())
            {
                ripples.encode(queue, encoder, i, stamps);
            }
        }
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
        let planar_view = planar.map(|p| p.0.clone());
        let ripple_view = r.ripples.as_ref().map(|p| p.view.clone());
        let submerged_views = submerged.map(|(color, depth)| (color.clone(), depth.clone()));
        if r.resources.as_ref() != Some(&resources)
            || r.planar_view != planar_view
            || r.ripple_view != ripple_view
            || r.submerged_views != submerged_views
        {
            // Bindings retain the viewport copies and live atmosphere resources between frames.
            for slot in r.slots.iter_mut().flatten() {
                slot.bindings = r.pipeline.bindings(
                    device,
                    &r.scene.color,
                    &r.scene.depth,
                    aerial.map(|a| (&a.volume_view, &a.sampler)),
                    cloud.map(|c| (&c.view, &c.sampler, &c.uniforms)),
                    Some(&slot.shore),
                    Some(&r.noise),
                    planar.map(|p| (p.0, p.1)),
                    Some(&r.normal),
                    r.ripples.as_ref().map(|p| &p.view),
                    submerged,
                    self.sky_admission.as_ref().map(|a| &a.0),
                );
            }
            r.resources = Some(resources);
            r.planar_view = planar_view;
            r.ripple_view = ripple_view;
            r.submerged_views = submerged_views;
        }
        for slot in r.slots.iter_mut().flatten() {
            slot.ready = false;
        }
        for promoted in &self.promoted_surfaces {
            let index = promoted.descriptor.index;
            if index >= OCEAN_MAX_SURFACES {
                continue;
            }
            let extension = (self.sky_extend_enabled && sky_pvs_mask & (1u8 << index) != 0)
                .then_some(self.sky_extend_regions[index])
                .flatten();
            if !Self::native_pool_visible(pvs[index], extension.is_some()) {
                continue;
            }
            let horizon = extension.is_some() && self.sky_infinite_enabled;
            let horizon_radius = if horizon {
                (camera.far_distance() / OCEAN_UNITS_PER_METER * 1.05).max(1_000_000.0)
            } else {
                0.0
            };
            let surface = promoted.descriptor.surface;
            let bounds = [
                surface.minimum[0],
                surface.minimum[1],
                surface.maximum[0],
                surface.maximum[1],
            ];
            let start = self.masks.ranges[index][0] as usize * 2;
            let count = self.masks.ranges[index][1] as usize * 2;
            let settings = lake_source_settings(self.settings.lake, &promoted.descriptor);
            let geometry_key = (
                std::sync::Arc::as_ptr(&self.masks) as usize,
                bounds.map(f32::to_bits),
                horizon_radius.to_bits(),
            );
            if r.slots[index]
                .as_ref()
                .is_none_or(|s| s.geometry_key != geometry_key)
            {
                let mesh = LakeMesh::extended(
                    bounds,
                    &self.masks.triangles[start..start + count],
                    horizon.then_some(horizon_radius),
                );
                let rectangle = [
                    [bounds[0], bounds[1], bounds[2], bounds[1]],
                    [bounds[2], bounds[1], bounds[2], bounds[3]],
                    [bounds[2], bounds[3], bounds[0], bounds[3]],
                    [bounds[0], bounds[3], bounds[0], bounds[1]],
                ];
                let edges = self.masks.boundaries[index]
                    .as_deref()
                    .map(Vec::as_slice)
                    .filter(|edges| !edges.is_empty())
                    .unwrap_or(&rectangle);
                let shore = shoreline(device, queue, bounds, edges);
                let bindings = r.pipeline.bindings(
                    device,
                    &r.scene.color,
                    &r.scene.depth,
                    aerial.map(|a| (&a.volume_view, &a.sampler)),
                    cloud.map(|c| (&c.view, &c.sampler, &c.uniforms)),
                    Some(&shore),
                    Some(&r.noise),
                    planar.map(|p| (p.0, p.1)),
                    Some(&r.normal),
                    r.ripples.as_ref().map(|p| &p.view),
                    submerged,
                    self.sky_admission.as_ref().map(|a| &a.0),
                );
                use wgpu::util::DeviceExt;
                let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("finite lake clipmap indices"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                let mut boundary = SurfaceBoundary::zeroed();
                boundary.bounds = bounds;
                boundary.scale = [OCEAN_UNITS_PER_METER, 0.0, 0.0, 0.0];
                boundary.counts[0] = (count / 2) as u32;
                boundary.tiles = self.masks.tiles[index];
                boundary.triangles[..count]
                    .copy_from_slice(&self.masks.triangles[start..start + count]);
                boundary.prepare_candidates();
                boundary.counts[3] = 3;
                r.slots[index] = Some(Slot {
                    bindings,
                    boundary,
                    vertices: LakePipeline::vertices(device, &mesh.vertices),
                    count: mesh.count(0),
                    indices,
                    mesh,
                    first: 0,
                    shore,
                    geometry_key,
                    horizon,
                    ready: false,
                });
            }
            let slot = r.slots[index].as_mut().unwrap();
            slot.horizon = horizon;
            let camera = super::original_surface::surface_camera(
                camera,
                surface.plane_height,
                key.0,
                key.1,
                jitter,
                seconds,
                dt,
                frame,
            );
            queue.write_buffer(&slot.bindings.camera, 0, bytemuck::bytes_of(&camera));
            let eye = [camera.position_time[0], camera.position_time[2]];
            let centre = if extension.is_some() {
                eye
            } else {
                [
                    eye[0].clamp(
                        bounds[0] / OCEAN_UNITS_PER_METER,
                        bounds[2] / OCEAN_UNITS_PER_METER,
                    ),
                    eye[1].clamp(
                        bounds[1] / OCEAN_UNITS_PER_METER,
                        bounds[3] / OCEAN_UNITS_PER_METER,
                    ),
                ]
            };
            let distance = (eye[0] - centre[0])
                .hypot(eye[1] - centre[1])
                .hypot(camera.position_time[1]);
            let detail = slot.mesh.map.detail((distance / slot.mesh.scale) as f64);
            slot.first = detail.first;
            slot.count = slot.mesh.count(slot.first);
            let mut uniforms = LakeUniforms::new(
                settings.wind_speed,
                settings.direction.to_radians(),
                settings.transparency,
                aerial.is_some(),
            );
            uniforms.underwater_fog = [
                volumes.fog[0],
                volumes.fog[1],
                volumes.fog[2],
                volumes.fog[3] / OCEAN_UNITS_PER_METER,
            ];
            uniforms.underwater = [
                f32::from(
                    r.cheap
                        && submerged.is_some()
                        && camera_water_surface > crate::renderer::NO_WATER_SURFACE,
                ),
                volumes.params[0] / OCEAN_UNITS_PER_METER,
                OCEAN_UNITS_PER_METER,
                0.0,
            ];
            uniforms.frames = slot
                .mesh
                .map
                .frames(
                    (centre[0] / slot.mesh.scale) as f64,
                    (centre[1] / slot.mesh.scale) as f64,
                )
                .map(|f| {
                    [
                        f[0] * slot.mesh.scale,
                        f[1] * slot.mesh.scale,
                        f[2] * slot.mesh.scale,
                        f[3] / slot.mesh.scale,
                    ]
                });
            uniforms.clipmap = [32.0, centre[0], centre[1], detail.morph];
            uniforms.deformation = [
                settings.amplitude * 0.0254,
                1.0,
                detail.first as f32,
                settings.speed,
            ];
            uniforms.offset[3] = f32::from(horizon);
            if !ripple_stamps[index].is_empty() {
                uniforms.ripples = [
                    ripple_origin[0],
                    ripple_origin[1],
                    1.0 / blue_water_gpu::lake_ripples::SPAN,
                    index as f32,
                ];
            }
            uniforms.shallow_color = [
                settings.shallow_color[0],
                settings.shallow_color[1],
                settings.shallow_color[2],
                settings.depth_color_strength,
            ];
            uniforms.deep_color = [
                settings.deep_color[0],
                settings.deep_color[1],
                settings.deep_color[2],
                settings.color_depth,
            ];
            uniforms.set_body_color(settings.water_color, settings.transparency);
            uniforms.transmission = [
                settings.cheap_transmission,
                settings.cheap_refraction,
                settings.transparency,
                settings.opaque_depth * 0.3048,
            ];
            uniforms.reflection[1] = settings.reflection_mode as f32;
            if settings.source_hints {
                if let Some(roughness) = promoted.descriptor.roughness.filter(|v| v.is_finite()) {
                    uniforms.appearance[0] = roughness.clamp(0.08, 1.0);
                }
                if let Some(degrees) = promoted.descriptor.swell_direction.filter(|v| v.is_finite()) {
                    // Godot reports angle about -Z, Lake's wind uses +Z.
                    // Steer both the vertex relief field and Cheapwater normals.
                    let radians = -degrees.to_radians();
                    uniforms.wind[0] = radians.cos();
                    uniforms.wind[1] = radians.sin();
                    uniforms.wind[3] = 1.0; // Cheapwater orientation is opt-in.
                }
            }
            uniforms.tint = [
                settings.tint_color[0],
                settings.tint_color[1],
                settings.tint_color[2],
                settings.reflections,
            ];
            if settings.reflection_mode == 2 {
                if let Some((_, _, planes)) = planar {
                    if let Some(slot) = planes.iter().position(|plane| {
                        plane.is_some_and(|p| {
                            p[1].abs() > 0.99 && (p[1] * surface.plane_height + p[3]).abs() < 0.5
                        })
                    }) {
                        uniforms.reflection[0] = slot as f32;
                    }
                }
            }
            uniforms.mask = [
                bounds[0] / OCEAN_UNITS_PER_METER,
                bounds[1] / OCEAN_UNITS_PER_METER,
                OCEAN_UNITS_PER_METER / (bounds[2] - bounds[0]).max(0.001),
                OCEAN_UNITS_PER_METER / (bounds[3] - bounds[1]).max(0.001),
            ];
            queue.write_buffer(&slot.bindings.uniforms, 0, bytemuck::bytes_of(&uniforms));
            // The coarse mesh is clipped to the exact footprint, including holes.
            let mut boundary = slot.boundary;
            boundary.sky_plane[0] = promoted.descriptor.surface.plane_height;
            boundary.sky_uv = self.sky_admission.as_ref().map_or([0.0; 4], |a| a.1);
            if let Some(sky) = extension {
                boundary.counts[1] = sky.count as u32;
                boundary.sky = sky.segments;
            } else if let Some(pvs) = pvs[index] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_hints_remap_each_pool_without_changing_saved_global_lake_settings() {
        let global = LakeSettings::default();
        let mut red = super::super::appearance::tests::pool(0, 0.8);
        red.color = [0.9, 0.1, 0.15];
        red.source_deformation = Some(12.0);
        red.source_alpha = Some(0.25);
        red.roughness = Some(0.7);
        red.swell_direction = Some(90.0);
        let adapted = lake_source_settings(global, &red);
        assert!(adapted.water_color[0] > adapted.water_color[1]);
        assert!(adapted.deep_color[0] > adapted.deep_color[1]);
        assert!(adapted.amplitude > global.amplitude);
        assert!(adapted.transparency > global.transparency);
        assert_eq!(adapted.reflection_mode, global.reflection_mode);
        assert_eq!(adapted.reflections, global.reflections);
        assert_eq!(global, LakeSettings::default(), "do not rewrite global sliders");

        let mut disabled = global;
        disabled.source_hints = false;
        assert_eq!(lake_source_settings(disabled, &red), disabled, "A/B off means zero source changes");

        let mut white = super::super::appearance::tests::pool(1, 0.0);
        white.color = [1.0, 1.0, 1.0];
        let neutral = lake_source_settings(global, &white);
        assert_eq!(neutral.water_color, global.water_color, "uncolored fallback textures are not pigment");
        assert_eq!(neutral.deep_color, global.deep_color);

        let mut frozen = global;
        frozen.amplitude = 0.0;
        frozen.speed = 0.0;
        assert_eq!(lake_source_settings(frozen, &red).amplitude, 0.0);
        assert_eq!(lake_source_settings(frozen, &red).speed, 0.0);
    }

    #[test]
    fn cheapwater_avoids_scene_capture_except_for_active_ssr() {
        let mut settings = LakeSettings::default();
        settings.shader = 0;
        assert!(
            settings.needs_scene_copy(),
            "original needs depth transmission"
        );
        settings.shader = 1;
        settings.cheap_transmission = 0.0;
        settings.depth_color_strength = 0.0;
        for mode in 0..=2 {
            settings.reflection_mode = mode;
            assert_eq!(settings.needs_scene_copy(), mode == 1);
        }
        settings.reflection_mode = 1;
        settings.reflections = 0.0;
        assert!(!settings.needs_scene_copy());
        settings.depth_color_strength = 1.0;
        assert!(
            settings.needs_scene_copy(),
            "depth color must capture the bed without transparency or reflections"
        );
        assert!(
            !settings.needs_scene_color(),
            "depth coloring alone must skip the color copy"
        );
        settings.depth_color_strength = 0.0;
        settings.cheap_transmission = 0.5;
        assert!(settings.needs_scene_color());
        assert!(
            settings.needs_scene_copy(),
            "transparent water must capture the bed even without reflections"
        );
    }

    #[test]
    fn lake_horizon_adds_a_bounded_strip_outside_the_displacement_field() {
        let bounds = [-400., -400., 400., 400.];
        let mesh = LakeMesh::extended(bounds, &[], Some(10_000.));
        let finite = LakeMesh::new(bounds, &[]);
        let outer = mesh.map.half(mesh.map.levels - 1) as f32 * mesh.scale;
        assert!(
            outer >= 64.0,
            "horizon join must lie beyond displacement fade"
        );
        let horizon: Vec<_> = mesh.vertices.iter().filter(|v| v.lake[3] == 1.0).collect();
        assert_eq!(horizon.len(), 16);
        assert!(horizon
            .iter()
            .all(|v| v.position[0].abs().max(v.position[2].abs()) >= outer));
        assert!(horizon.iter().any(|v| v.position[0].abs() == 10_000.));
        assert!(mesh.count(0) / 3 <= 16_000);
        assert!(finite.vertices.iter().all(|v| v.lake[3] == 0.0));
        assert!(mesh
            .indices
            .iter()
            .all(|&i| (i as usize) < mesh.vertices.len()));
    }

    #[test]
    fn lake_clipmap_covers_finite_pools_with_a_small_bounded_budget() {
        for metres in [3.0, 50.0, 500.0, 10_000.0] {
            let extent = metres * OCEAN_UNITS_PER_METER;
            let mesh = LakeMesh::new([-extent, -extent, 0., 0.], &[]);
            assert!(mesh
                .indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len()));
            assert!(mesh.count(0) / 3 <= 16_000);
            // Leave enough margin for the outer ring's independently snapped centre.
            let outer = mesh.map.levels - 1;
            let coverage = (mesh.map.half(outer) - mesh.map.cell(outer)) as f32 * mesh.scale;
            assert!(
                coverage >= metres,
                "pool size {metres} exceeds coverage {coverage}"
            );
            if outer > 0 {
                assert!(mesh.count(outer) < mesh.count(0));
            }
        }
    }

    #[test]
    fn lake_shore_fade_pins_outer_shores_and_hole_rims() {
        let edges = [
            [0., 0., 400., 0.],
            [400., 0., 400., 400.],
            [400., 400., 0., 400.],
            [0., 400., 0., 0.],
            [160., 160., 240., 160.],
            [240., 160., 240., 240.],
            [240., 240., 160., 240.],
            [160., 240., 160., 160.],
        ];
        let pixels = shore_pixels([0., 0., 400., 400.], &edges, 128);
        assert_eq!(pixels[32 * 128], 0, "outer shoreline must stay flat");
        assert_eq!(pixels[64 * 128 + 51], 0, "hole rim must stay flat");
        assert_eq!(
            pixels[32 * 128 + 32],
            255,
            "interior should keep the full relief"
        );
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
        self.draw_mesh(pass, admitted, false)
    }
    pub fn draw_wireframe<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, admitted: u8) {
        self.draw_mesh(pass, admitted, true);
    }
    fn draw_mesh<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        admitted: u8,
        wireframe: bool,
    ) -> (u32, u32, u64) {
        let Some(env) = &self.environment else {
            return (0, 0, 0);
        };
        let pipeline = if wireframe {
            self.pipeline.wireframe.as_ref()
        } else if self.cheap {
            Some(&self.pipeline.cheap)
        } else {
            Some(&self.pipeline.pipeline)
        };
        let Some(pipeline) = pipeline else {
            return (0, 0, 0);
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(2, env, &[]);
        let mut draws = 0;
        for (i, slot) in self.slots.iter().enumerate() {
            let Some(s) = slot
                .as_ref()
                .filter(|s| s.ready && (admitted >> i & 1 != 0 || s.horizon))
            else {
                continue;
            };
            pass.set_bind_group(0, &s.bindings.camera_group, &[]);
            pass.set_bind_group(1, &s.bindings.material, &[]);
            pass.set_vertex_buffer(0, s.vertices.slice(..));
            pass.set_index_buffer(s.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(s.mesh.fills[s.first].clone(), 0, 0..1);
            pass.draw_indexed(s.mesh.rings[s.first]..s.mesh.indices.len() as u32, 0, 0..1);
            draws += 2;
        }
        (draws, 0, 0)
    }
}
