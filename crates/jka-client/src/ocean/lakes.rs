//! Promoted JKA footprints shaded as Source water on gently displaced clipmaps
//! (a single flat quad at zero amplitude), without an ocean simulation.
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
    pub reflections: f32,
    /// Lake-local: 0 sky, 1 SSR, 2 planar. Independent of global reflection settings.
    pub reflection_mode: u8,
    /// Drift direction of the surface relief, degrees.
    pub direction: f32,
    /// Underwater visibility (Godot fog and Fluid Ninja submerged views).
    pub transparency: f32,
    /// Source distortion: scales HL2's $refractamount/$reflectamount (0.026 = HL2).
    pub cheap_refraction: f32,
    /// Source fog distance in feet ($fogend), above and below the surface.
    pub opaque_depth: f32,
    pub wake_strength: f32,
    /// Use the source JKA shader's per-pool appearance and motion cues.
    /// Disabled is bit-for-bit equivalent in uniforms to the old global lake path.
    pub source_hints: bool,
    /// Source fog color ($fogcolor).
    pub deep_color: [f32; 3],
    /// r_lakeUnderwaterMode: 4 Godot submerged fog (shared with Hybrid), 5 Fluid Ninja,
    /// 6 Source (HL2 $bottommaterial, water fog and $underwateroverlay).
    pub underwater_mode: u8,
    /// Source water material: 0 canal (`water_canals_water2`), 1 open coast (`water_coast01`).
    pub source_style: u8,
    /// Source water cheap-water fade start in metres (end = 1.5x, as the canals'
    /// 2000/3000 units); beyond it reflections fade to the sky cubemap. 0 = off.
    pub source_cubemap_fade: f32,
    /// `r_lakeCaustics`: Fluid Ninja caustics on the lakebed seen through the water.
    pub caustics: bool,
    /// `r_lakeUnderwaterCaustics`: the same caustics on submerged geometry in the
    /// Source underwater view (r_lakeUnderwaterMode 6).
    pub underwater_caustics: bool,
}
impl Default for LakeSettings {
    fn default() -> Self {
        Self {
            amplitude: 6.65,
            speed: 1.45,
            water_color: [0.41818184, 0.9619174, 1.0],
            tint_color: [0.09994107; 3],
            reflections: 0.525,
            reflection_mode: 2,
            direction: 52.0,
            transparency: 0.5,
            cheap_refraction: 0.026,
            opaque_depth: 8.0,
            wake_strength: 10.0,
            source_hints: true,
            deep_color: [0.0, 0.08160421, 0.061574105],
            underwater_mode: 4,
            source_style: 0,
            source_cubemap_fade: 50.0,
            caustics: true,
            underwater_caustics: true,
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
        self.reflection_mode = self.reflection_mode.min(2);
        self.water_color =
            std::array::from_fn(|i| finite(self.water_color[i], defaults.water_color[i], 0.0, 1.0));
        self.tint_color =
            std::array::from_fn(|i| finite(self.tint_color[i], defaults.tint_color[i], 0.0, 1.0));
        self.direction = finite(self.direction, defaults.direction, -180.0, 180.0);
        self.transparency = finite(self.transparency, defaults.transparency, 0.1, 10.0);
        self.cheap_refraction = finite(self.cheap_refraction, defaults.cheap_refraction, 0.0, 0.05);
        self.opaque_depth = finite(self.opaque_depth, defaults.opaque_depth, 0.25, 100.0);
        self.wake_strength = finite(self.wake_strength, defaults.wake_strength, 0.0, 10.0);
        self.deep_color =
            std::array::from_fn(|i| finite(self.deep_color[i], defaults.deep_color[i], 0.0, 1.0));
        self.source_style = self.source_style.min(1);
        self.source_cubemap_fade =
            finite(self.source_cubemap_fade, defaults.source_cubemap_fade, 0.0, 500.0);

        self.underwater_mode = match self.underwater_mode {
            super::NINJA_UNDERWATER_MODE | super::SOURCE_UNDERWATER_MODE => self.underwater_mode,
            _ => 4,
        };
        self
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
        // The Water color (Source $fogcolor, also the underwater fog) takes the
        // map's hue only while it is still the default, so a red water shader
        // does not turn green with depth. A color the user picked always wins.
        if lake.deep_color == LakeSettings::default().deep_color {
            lake.deep_color = recolor(lake.deep_color);
        }
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
    geometry_key: (usize, [u32; 4], u32, bool),
    horizon: bool,
    ready: bool,
}
pub(super) struct LakeRenderer {
    key: (u32, u32, u32, wgpu::TextureFormat),
    pub scene: SceneCopy,
    pipeline: LakePipeline,
    noise: wgpu::TextureView,
    normal: wgpu::TextureView,
    /// Source water normal frames the slot bindings were built with.
    source_view: Option<wgpu::TextureView>,
    /// r_lakeUnderwaterMode 6 post pass, created on first submerged frame.
    source_underwater: Option<super::source_underwater::SourceUnderwater>,
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
    /// r_lakeUnderwaterMode 5 pass, built on first submerged use.
    ninja_underwater: Option<super::ninja_underwater::NinjaUnderwater>,
    /// r_lakeNinjaWake: the Fluid Ninja lake wake window and its bound outputs.
    ninja_lake: super::ninja_lake::NinjaLake,
    ninja_views: Option<(wgpu::TextureView, wgpu::TextureView)>,
    /// Fluid Ninja inputs for the lake wake and caustics, loaded on first use.
    ninja_assets: Option<NinjaLakeAssets>,
    ninja_assets_failed: bool,
    caustics_view: Option<wgpu::TextureView>,
}

/// The Fluid Ninja lake inputs: Coast simulation images, flow map, caustics volume.
pub(super) struct NinjaLakeAssets {
    pub simulation: [wgpu::TextureView; 4],
    pub flow: wgpu::TextureView,
    pub caustics: Option<wgpu::TextureView>,
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
    /// Zero amplitude: one flat quad over the clipmap's reach, no rings.
    flat: bool,
}
impl LakeMesh {
    #[cfg(test)]
    fn new(bounds: [f32; 4]) -> Self {
        Self::extended(bounds, None, false)
    }
    fn extended(bounds: [f32; 4], horizon: Option<f32>, flat: bool) -> Self {
        let extent = (bounds[2] - bounds[0]).max(bounds[3] - bounds[1]) / OCEAN_UNITS_PER_METER;
        let mut levels = if horizon.is_some() { 4 } else { 1 };
        while levels < 10 && 8.0 * ((1u32 << (levels - 1)) as f32) * 0.9 < extent {
            levels += 1;
        }
        let map = blue_water_gpu::geometry::Clipmap::new(32, levels);
        let scale = (0.5 / map.cell(0) as f32).max(extent / (4096.0 * 0.9));
        let inner = map.half(levels - 1) as f32 * scale;
        let outer_level = (levels - 1) as f32;
        let mut mesh = if flat {
            // Nothing displaces a zero-amplitude surface, so the rings would
            // only rasterize the same plane. One quad spans the clipmap's
            // whole reach about the same snapped centre (vertex w = 1: placed
            // relative to the outer level's frame, like the horizon strip),
            // so footprint clipping, the far clip and the horizon join match.
            let vertices = [[-1.0f32, -1.0], [-1.0, 1.0], [1.0, -1.0], [1.0, 1.0]]
                .map(|p| LakeVertex {
                    position: [p[0] * inner, 0.0, p[1] * inner],
                    lake: [0.0, 0.0, outer_level, 1.0],
                })
                .to_vec();
            Self {
                map,
                scale,
                vertices,
                indices: vec![0, 1, 2, 2, 1, 3],
                fills: Vec::new(),
                rings: Vec::new(),
                flat: true,
            }
        } else {
            let built = blue_water_gpu::geometry::build(map, 512.0);
            let end = built.rings[levels - 1] + 32 * 32 * 3 / 4 * 6;
            let vertices = built
                .vertices
                .iter()
                .take((33 * 33 * levels) as usize)
                .map(|v| LakeVertex {
                    position: [v.position[0] * scale, 0.0, v.position[1] * scale],
                    lake: [0.0, 0.0, v.level[0], 0.0],
                })
                .collect();
            Self {
                map,
                scale,
                vertices,
                indices: built.indices[..end as usize].to_vec(),
                fills: built.fills,
                rings: built.rings,
                flat: false,
            }
        };
        if let Some(far) = horizon {
            // Eight triangles connect the quiet outer ring to the far plane.
            // Exact sky-contact clipping still runs per pixel. At least four
            // levels put this join beyond the 32m displacement fade.
            let outer = far.max(inner * 2.0);
            let corners = [[-1., -1.], [-1., 1.], [1., 1.], [1., -1.]];
            for side in 0..4 {
                let a = corners[side];
                let b = corners[(side + 1) % 4];
                let base = mesh.vertices.len() as u32;
                for (p, radius) in [(a, inner), (b, inner), (a, outer), (b, outer)] {
                    mesh.vertices.push(LakeVertex {
                        position: [p[0] * radius, 0., p[1] * radius],
                        lake: [0., 0., outer_level, 1.],
                    });
                }
                mesh.indices
                    .extend([base, base + 1, base + 2, base + 2, base + 1, base + 3]);
            }
        }
        mesh
    }
    fn count(&self, first: usize) -> u32 {
        if self.flat {
            return self.indices.len() as u32;
        }
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
        ninja_actors: &[jka_river::river_ninja::ActorSample],
        ninja_load: impl FnOnce(bool) -> Result<super::NinjaOceanAssets, String>,
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
                pipeline: LakePipeline::new(device, key.3, key.2, jka_ninja::lake_wake_wgsl().as_deref()),
                noise: LakePipeline::noise(
                    device,
                    queue,
                    &super::lake_noise::lake_noise(),
                    128,
                ),
                normal: self.lake_normal.clone(),
                source_view: None,
                source_underwater: None,
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
                ninja_underwater: None,
                ninja_lake: super::ninja_lake::NinjaLake::new(device),
                ninja_views: None,
                ninja_assets: None,
                ninja_assets_failed: false,
                caustics_view: None,
            });
        }
        let r = self.lakes.as_mut().unwrap();
        let ripple_origin = super::lake_wakes::LakeWakes::origin(camera.position.to_array());
        let wake_enabled = self.settings.lake.wake_strength > 0.0;
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
        // Fluid Ninja lake wake (r_lakeNinjaWake, gated by cg_waterwake).
        let ninja_on = jka_ninja::INCLUDED && self.settings.lake_ninja_wake
            && jka_river::river_ninja::wake_enabled();
        let caustics_on = jka_ninja::INCLUDED && self.settings.lake.caustics;
        let underwater_caustics_on = jka_ninja::INCLUDED && self.settings.lake.underwater_caustics
            && self.settings.blue_water_underwater_mode == super::SOURCE_UNDERWATER_MODE;
        if (ninja_on || caustics_on || underwater_caustics_on) && r.ninja_assets.is_none() && !r.ninja_assets_failed {
            match ninja_load(true) {
                Ok(a) => {
                    let [flow, ..] = a.material;
                    r.ninja_assets = Some(NinjaLakeAssets { simulation: a.simulation, flow, caustics: a.caustics });
                }
                Err(e) => { eprintln!("[LAKE NINJA] inputs unavailable: {e}"); r.ninja_assets_failed = true; }
            }
        }
        let caustics_view = r.ninja_assets.as_ref().filter(|_| caustics_on).and_then(|a| a.caustics.clone());
        let ninja = if let (true, Some(assets)) = (ninja_on, r.ninja_assets.as_ref()) {
            let pools: Vec<_> = self.promoted_surfaces.iter()
                .filter(|p| p.descriptor.index < OCEAN_MAX_SURFACES && wake_visible[p.descriptor.index])
                .map(|p| {
                    let s = p.descriptor.surface;
                    super::ninja_lake::LakePool { index: p.descriptor.index, plane: s.plane_height,
                        bounds: [s.minimum[0], s.minimum[1], s.maximum[0], s.maximum[1]] }
                })
                .collect();
            r.ninja_lake.prepare(device, queue, encoder, seconds, camera.position.to_array(), ninja_actors,
                &pools, &self.masks, assets)
        } else {
            r.ninja_lake.idle(queue);
            None
        };
        let ninja_views = ninja.as_ref().map(|n| (n.0.clone(), n.1.clone()));
        let ninja_bind = ninja.as_ref().map(|n| (&n.0, &n.1, &n.2, n.3));
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
            intensity: self.environment.map_sun_intensity(),
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
        let source_view = self.source_water_normal.clone();
        if r.resources.as_ref() != Some(&resources)
            || r.source_view != source_view
            || r.planar_view != planar_view
            || r.ripple_view != ripple_view
            || r.submerged_views != submerged_views
            || r.ninja_views != ninja_views
            || r.caustics_view != caustics_view
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
                    self.source_water_normal.as_ref(),
                    r.ripples.as_ref().map(|p| &p.view),
                    submerged,
                    self.sky_admission.as_ref().map(|a| &a.0),
                    ninja_bind,
                    caustics_view.as_ref(),
                );
            }
            r.resources = Some(resources);
            r.planar_view = planar_view;
            r.source_view = source_view;
            r.ripple_view = ripple_view;
            r.submerged_views = submerged_views;
            r.ninja_views = ninja_views;
            r.caustics_view = caustics_view.clone();
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
            // Amplitude is per pool after source hints; zero needs no relief mesh.
            let flat = settings.amplitude <= 0.0;
            let geometry_key = (
                std::sync::Arc::as_ptr(&self.masks) as usize,
                bounds.map(f32::to_bits),
                horizon_radius.to_bits(),
                flat,
            );
            if r.slots[index]
                .as_ref()
                .is_none_or(|s| s.geometry_key != geometry_key)
            {
                let mesh = LakeMesh::extended(bounds, horizon.then_some(horizon_radius), flat);
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
                    self.source_water_normal.as_ref(),
                    r.ripples.as_ref().map(|p| &p.view),
                    submerged,
                    self.sky_admission.as_ref().map(|a| &a.0),
                    ninja_bind,
                    caustics_view.as_ref(),
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
            let mut uniforms = LakeUniforms::new(settings.direction.to_radians(), aerial.is_some());
            uniforms.underwater_fog = [
                volumes.fog[0],
                volumes.fog[1],
                volumes.fog[2],
                volumes.fog[3] / OCEAN_UNITS_PER_METER,
            ];
            // x: 1 shared submerged view (Godot / Ninja), 2 Source $bottommaterial.
            let eye_submerged = camera_water_surface > crate::renderer::NO_WATER_SURFACE;
            let source_beneath = self.settings.blue_water_underwater_mode
                == super::SOURCE_UNDERWATER_MODE
                && eye_submerged;
            uniforms.underwater = [
                if source_beneath {
                    2.0
                } else {
                    f32::from(submerged.is_some() && eye_submerged)
                },
                volumes.params[0] / OCEAN_UNITS_PER_METER,
                OCEAN_UNITS_PER_METER,
                // Source water: cheap-water (cubemap) fade start, metres.
                settings.source_cubemap_fade,
            ];
            uniforms.appearance[1] = f32::from(settings.source_style);
            // r_lakeCaustics: only with the caustics volume bound.
            uniforms.appearance[2] = f32::from(caustics_view.is_some());
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
            uniforms.deep_color = [
                settings.deep_color[0],
                settings.deep_color[1],
                settings.deep_color[2],
                0.0,
            ];
            uniforms.transmission = [0.0, settings.cheap_refraction, 0.0, settings.opaque_depth * 0.3048];
            uniforms.reflection[1] = settings.reflection_mode as f32;
            if settings.source_hints {
                if let Some(roughness) = promoted.descriptor.roughness.filter(|v| v.is_finite()) {
                    uniforms.appearance[0] = roughness.clamp(0.08, 1.0);
                }
                if let Some(degrees) = promoted.descriptor.swell_direction.filter(|v| v.is_finite()) {
                    // Godot reports angle about -Z, Lake's wind uses +Z.
                    // Steer both the vertex relief field and the Source normals.
                    let radians = -degrees.to_radians();
                    uniforms.wind[0] = radians.cos();
                    uniforms.wind[1] = radians.sin();
                    uniforms.wind[3] = 1.0; // Normal-map orientation is opt-in.
                }
            }
            // w gates the world reflection (SSR/planar); 0 leaves the sky only.
            uniforms.tint = [1.0, 1.0, 1.0, settings.reflections];
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
        let mut picked = global;
        picked.deep_color = [0.0, 0.45, 0.45];
        assert_eq!(lake_source_settings(picked, &red).deep_color, picked.deep_color,
            "a Water color the user picked is not recolored by the source hue");

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
    fn lake_horizon_adds_a_bounded_strip_outside_the_displacement_field() {
        let bounds = [-400., -400., 400., 400.];
        let mesh = LakeMesh::extended(bounds, Some(10_000.), false);
        let finite = LakeMesh::new(bounds);
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

        // Zero amplitude: one quad over the same reach, joined to the same strip.
        let flat = LakeMesh::extended(bounds, Some(10_000.), true);
        assert_eq!(flat.count(0), 6 + 8 * 3);
        assert!(flat.vertices.iter().all(|v| v.lake[3] == 1.0));
        assert!(flat.vertices[..4]
            .iter()
            .all(|v| v.position[0].abs() == outer && v.position[2].abs() == outer));
        assert!(flat.indices.iter().all(|&i| (i as usize) < flat.vertices.len()));
    }

    #[test]
    fn lake_clipmap_covers_finite_pools_with_a_small_bounded_budget() {
        for metres in [3.0, 50.0, 500.0, 10_000.0] {
            let extent = metres * OCEAN_UNITS_PER_METER;
            let mesh = LakeMesh::new([-extent, -extent, 0., 0.]);
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
impl OceanGpu {
    /// r_lakeUnderwaterMode 5 while submerged: the Fluid Ninja underwater post
    /// material over the finished scene, masked by the flat-volume `water_path`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_lake_ninja_underwater(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView, water_path: &wgpu::TextureView,
        ninja: (super::NinjaUnderwaterView, &super::NinjaUnderwaterTextures)) {
        if self.settings.backend != 3 || self.settings.blue_water_underwater_mode != super::NINJA_UNDERWATER_MODE {
            return;
        }
        let Some(r) = self.lakes.as_mut() else { return };
        // Refresh after water and transparency; this copy is re-captured before the next frame's water.
        r.scene.capture(device, encoder, color, depth);
        let (view, textures) = ninja;
        let group = r.ninja_underwater.get_or_insert_with(|| super::ninja_underwater::NinjaUnderwater::new(device))
            .prepare(device, queue, r.key.3, r.key.2, textures, view, &r.scene, super::ninja_underwater::Mask::Path(water_path));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Lake Fluid Ninja underwater"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: color, depth_slice: None, resolve_target: resolve,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }), stencil_ops: None }),
            ..Default::default()
        });
        crate::renderer::tile_probe_scissor_surface(&mut pass);
        if let Some(ninja) = r.ninja_underwater.as_ref() { ninja.draw(&mut pass, &group); }
    }
}

impl OceanGpu {
    /// r_lakeUnderwaterMode 6 while submerged: HL2's water fog over the
    /// finished scene and, for the coast material, its underwater overlay.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_lake_source_underwater(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView, view: super::source_underwater::View) {
        if self.settings.backend != 3 || self.settings.blue_water_underwater_mode != super::SOURCE_UNDERWATER_MODE {
            return;
        }
        let Some(r) = self.lakes.as_mut() else { return };
        // After water and transparency, as the Ninja pass.
        r.scene.capture(device, encoder, color, depth);
        let (format, samples) = (r.key.3, r.key.2);
        r.source_underwater
            .get_or_insert_with(|| super::source_underwater::SourceUnderwater::new(device, format, samples))
            .draw(device, queue, encoder, color, resolve, &r.scene, self.source_water_normal.as_ref(),
                r.ninja_assets.as_ref().filter(|_| self.settings.lake.underwater_caustics).and_then(|a| a.caustics.as_ref()), view);
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
            if s.mesh.flat {
                pass.draw_indexed(0..s.mesh.indices.len() as u32, 0, 0..1);
                draws += 1;
            } else {
                pass.draw_indexed(s.mesh.fills[s.first].clone(), 0, 0..1);
                pass.draw_indexed(s.mesh.rings[s.first]..s.mesh.indices.len() as u32, 0, 0..1);
                draws += 2;
            }
        }
        (draws, 0, 0)
    }
}
