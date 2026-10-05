//! Environment water.
use crate::renderer::{DrawBatch, GpuVertex, Renderer, WorldBatch};

impl Renderer {
    pub(in crate::renderer) fn set_grass_enabled(&mut self, enabled: bool) {
        self.grass_enabled = enabled;
    }

    pub(in crate::renderer) fn set_grass_precompute(&mut self, enabled: bool) {
        self.grass_precompute_enabled = enabled;
    }

    pub(in crate::renderer) fn set_grass_mid_lod(&mut self, enabled: bool) {
        self.grass_mid_lod_enabled = enabled;
    }

    pub(in crate::renderer) fn set_grass_front_to_back(&mut self, enabled: bool) {
        self.grass_front_to_back_enabled = enabled;
    }

    /// The index range promoted water draws instead of its authored face, or
    /// `None` when the ocean is off or this world carries no clipmap.
    pub(in crate::renderer) fn ocean_clipmap_quality(&self) -> Option<usize> {
        self.ocean_enabled
            .then(|| usize::from(self.ocean_settings.mesh_quality.min(1)))
    }

    pub(in crate::renderer) fn set_ocean_settings(
        &mut self,
        settings: crate::ocean::OceanSettings,
    ) {
        let settings = settings.sanitize();
        let recreate = self
            .ocean
            .as_ref()
            .is_some_and(|ocean| ocean.map_size() != settings.map_size);
        self.ocean_settings = settings;
        if self.ocean_enabled {
            if recreate {
                let mut ocean = crate::ocean::OceanGpu::new(
                    &self.device,
                    &self.queue,
                    &self.ocean_layout,
                    &self.ocean_spray_albedo.view,
                    self.ocean_settings,
                );
                ocean.set_enabled(&self.queue, true);
                self.ocean = Some(ocean);
            } else if let Some(ocean) = &mut self.ocean {
                ocean.set_settings(&self.queue, self.ocean_settings);
            }
        }
        let descriptors = self.authored_ocean_definitions.clone();
        self.set_authored_oceans(descriptors);
    }

    pub(in crate::renderer) fn set_authored_oceans(
        &mut self,
        descriptors: Vec<crate::ocean::authoring::AuthoredOcean>,
    ) {
        self.authored_ocean_definitions = descriptors.clone();
        if !self.ocean_enabled {
            self.authored_oceans.clear();
            return;
        }
        let mut previous = std::mem::take(&mut self.authored_oceans);
        for descriptor in descriptors {
            let mut settings = self.ocean_settings;
            settings.authored = descriptor.waves;
            settings.wind = descriptor.wind;
            settings.bounds = Some(crate::ocean::OceanSurface {
                plane_height: descriptor.height,
                minimum: [descriptor.mins[0], -descriptor.maxs[1]],
                maximum: [descriptor.maxs[0], -descriptor.mins[1]],
            });
            let mut gpu = if let Some(index) = previous
                .iter()
                .position(|(a, g)| a.index == descriptor.index && g.map_size() == settings.map_size)
            {
                previous.swap_remove(index).1
            } else {
                crate::ocean::OceanGpu::new(
                    &self.device,
                    &self.queue,
                    &self.ocean_layout,
                    &self.ocean_spray_albedo.view,
                    settings,
                )
            };
            gpu.set_settings(&self.queue, settings);
            gpu.set_enabled(&self.queue, self.ocean_enabled);
            self.authored_oceans.push((descriptor, gpu));
        }
    }

    pub(in crate::renderer) fn ocean_bind_group_for<'a>(
        batch: &WorldBatch,
        authored_oceans: &'a [(
            crate::ocean::authoring::AuthoredOcean,
            crate::ocean::OceanGpu,
        )],
        ocean: &'a Option<crate::ocean::OceanGpu>,
        inert: &'a wgpu::BindGroup,
    ) -> &'a wgpu::BindGroup {
        if batch.source.water_primary {
            let centre = std::array::from_fn(|i| (batch.bounds_min[i] + batch.bounds_max[i]) * 0.5);
            if let Some((_, gpu)) = authored_oceans.iter().find(|(a, _)| {
                batch
                    .source
                    .authored_ocean
                    .map_or_else(|| a.contains_render_point(centre), |id| a.index == id)
            }) {
                return &gpu.render_bind_group;
            }
        }
        ocean
            .as_ref()
            .map(|o| &o.render_bind_group)
            .unwrap_or(inert)
    }

    pub(in crate::renderer) fn set_ocean_enabled(&mut self, enabled: bool) {
        if self.ocean_enabled == enabled {
            return;
        }
        self.ocean_enabled = enabled;
        self.set_authored_oceans(self.authored_ocean_definitions.clone());
        if enabled {
            if self.ocean.is_none() {
                let mut ocean = crate::ocean::OceanGpu::new(
                    &self.device,
                    &self.queue,
                    &self.ocean_layout,
                    &self.ocean_spray_albedo.view,
                    self.ocean_settings,
                );
                ocean.set_enabled(&self.queue, true);
                self.ocean = Some(ocean);
            } else if let Some(ocean) = &mut self.ocean {
                ocean.set_enabled(&self.queue, true);
            }
        } else {
            // Release the large FFT textures/buffers when stock JKA water is selected.
            self.ocean = None;
            self.ocean_optics = crate::ocean::optics::Resources::new(
                &self.device,
                &self.ocean_optics_layout,
                &self.ocean_layout,
                (1, 1, self.msaa_samples, self.scene_format()),
            );
        }
        self.rebuild_planar_reflection_resources();
        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
    }
}

/// Upper bound on distinct water planes that get their own clipmap. Each one
/// costs a fixed mesh, so a map with many separate pools keeps the rest on
/// their authored faces rather than growing the vertex buffer without limit.
pub(in crate::renderer) const MAX_OCEAN_SURFACES: usize = crate::ocean::OCEAN_MAX_SURFACES;

/// Plane height and world-space xz footprint of a promoted water face.
pub(in crate::renderer) fn water_surface_extent(
    source: &DrawBatch,
    vertices: &[GpuVertex],
) -> (f32, [f32; 2], [f32; 2]) {
    let slice = &vertices[source.vertices.start as usize..source.vertices.end as usize];
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for vertex in slice {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(vertex.position[axis]);
            maximum[axis] = maximum[axis].max(vertex.position[axis]);
        }
    }
    (
        maximum[1],
        [minimum[0], minimum[2]],
        [maximum[0], maximum[2]],
    )
}

pub(in crate::renderer) fn ocean_surface_key(
    plane: f32,
    minimum: [f32; 2],
    maximum: [f32; 2],
) -> [u32; 5] {
    [
        plane.to_bits(),
        minimum[0].to_bits(),
        minimum[1].to_bits(),
        maximum[0].to_bits(),
        maximum[1].to_bits(),
    ]
}

/// A `surfaceparm water` brush contributes every face it has, so its underside
/// and its sides arrive looking exactly like its top. Only the upward-facing
/// horizontal face is the water surface the FFT ocean should replace.
pub(in crate::renderer) fn is_upward_water_face(
    source: &DrawBatch,
    vertices: &[GpuVertex],
) -> bool {
    let slice = &vertices[source.vertices.start as usize..source.vertices.end as usize];
    let mut lowest = f32::INFINITY;
    let mut highest = f32::NEG_INFINITY;
    let mut facing = 0.0f32;
    for vertex in slice {
        lowest = lowest.min(vertex.position[1]);
        highest = highest.max(vertex.position[1]);
        facing += vertex.normal[1];
    }
    lowest.is_finite() && facing > 0.0 && lowest >= highest - 1.0
}
