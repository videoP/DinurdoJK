use super::{GrassDrawPath, GrassPreparedDraw, GrassRenderer, create_grass_shadow_pipeline, texture_entry};
use crate::pipeline_jobs::{PipelineJobKey, PipelineJobManager};

pub(super) struct NinjaGrass {
    pub(super) draw_paths: [GrassDrawPath; 2],
    pub(super) bind_group: wgpu::BindGroup,
    shadow_layout: wgpu::PipelineLayout,
    shadow_shader: wgpu::ShaderModule,
    pub(super) shadows: [Option<wgpu::RenderPipeline>; 2],
}

impl NinjaGrass {
    pub(super) fn ensure_shadow_pipeline(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        reverse_z: bool,
    ) -> bool {
        let key = PipelineJobKey::new("grass-ninja-shadow", u32::from(reverse_z), 0);
        let pipeline = &mut self.shadows[usize::from(reverse_z)];
        if pipeline.is_some() {
            return true;
        }
        if let Some(ready) = jobs.take_ready(key) {
            *pipeline = Some(ready);
            return true;
        }
        let device = device.clone();
        let layout = self.shadow_layout.clone();
        let shader = self.shadow_shader.clone();
        jobs.request(key, "grass blade material shadow", move || {
            create_grass_shadow_pipeline(&device, &layout, &shader, reverse_z)
        });
        false
    }
}

impl GrassRenderer {
    pub(crate) fn set_ninja_style(&mut self, enabled: bool) {
        if self.ninja_requested != enabled {
            self.ninja_requested = enabled;
            self.ninja_load_attempted = false;
        }
    }

    pub(crate) fn clear_ninja_material(&mut self, jobs: &mut PipelineJobManager) {
        if self.ninja.take().is_some() {
            for slot in [0, 1] {
                jobs.cancel_slot("grass-ninja-shadow", slot);
                jobs.cancel_slot("grass-color", slot + 2);
                jobs.cancel_slot("grass-wireframe", slot + 2);
            }
        }
        self.ninja_load_attempted = false;
    }

    pub(crate) fn ensure_ninja_material(
        &mut self,
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        shadow_receiver_layout: &wgpu::BindGroupLayout,
        shadow_caster_layout: &wgpu::BindGroupLayout,
        load: impl FnOnce(&[jka_ninja::GrassImage; 5]) -> Result<[wgpu::TextureView; 5], String>,
    ) {
        if !self.ninja_requested || self.ninja.is_some() || self.ninja_load_attempted {
            return;
        }
        self.ninja_load_attempted = true;
        let Some(source) = jka_ninja::grass_blade_wgsl() else {
            eprintln!("r_grassStyle 1 unavailable in this build. Using style 0.");
            return;
        };
        let views = match load(&source.images) {
            Ok(views) => views,
            Err(error) => {
                eprintln!("r_grassStyle 1 unavailable. Using style 0. {error}");
                return;
            }
        };

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Grass material trilinear clamp sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Grass blade material layout"),
            entries: &[
                texture_entry(1),
                sampler_entry(2, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(5),
                texture_entry(6),
                texture_entry(7),
                texture_entry(8),
                sampler_entry(9, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                texture_entry(10),
            ],
        });
        let wind_view = self.wind_noise_texture().create_view(&Default::default());
        let [albedo, normal, roughness, mask, noise] = &views;
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grass blade material"),
            layout: &layout,
            entries: &[
                texture_binding(1, &wind_view),
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self._noise_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: self.globals_buffer.as_entire_binding() },
                texture_binding(5, albedo),
                texture_binding(6, normal),
                texture_binding(7, roughness),
                texture_binding(8, mask),
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::Sampler(&sampler) },
                texture_binding(10, noise),
            ],
        });

        let draw_path = |instances, base, vertex_offset, tangent| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Grass blade material draw layout"),
                bind_group_layouts: &[
                    Some(camera_layout), Some(&layout), Some(shadow_receiver_layout), Some(instances),
                ],
                immediate_size: 0,
            });
            let mut wgsl = blade_vertex_source(base, vertex_offset, tangent);
            wgsl.push('\n');
            wgsl.push_str(source.wind);
            wgsl.push('\n');
            wgsl.push_str(source.material);
            GrassDrawPath::new(pipeline_layout, blade_shader(device, wgsl))
        };
        let draw_paths = [
            draw_path(
                &self.map_layout,
                include_str!("../grass.wgsl"),
                "vertex_m += ninja_grass_offset(height_factor, wind_strength_turbulence, vec2<f32>(sin_turn, cos_turn)) * world_scale;",
                "out.blade_tangent = vec2<f32>(cos_turn, -sin_turn);",
            ),
            draw_path(
                &self.prepared_layout,
                include_str!("../grass_prepared.wgsl"),
                "vertex_m += ninja_grass_offset(height_factor, turbulence, vec2<f32>(instance.sin_turn, instance.cos_turn)) * (world_scale * instance.crushed_factor);",
                "out.blade_tangent = vec2<f32>(instance.cos_turn, -instance.sin_turn);",
            ),
        ];
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Grass blade material shadow layout"),
            bind_group_layouts: &[Some(shadow_caster_layout), Some(&self.prepared_layout), Some(&layout)],
            immediate_size: 0,
        });
        let mut shadow_wgsl = include_str!("../grass_shadow.wgsl").replace(
            "/* BLADE_MATERIAL_OFFSET */",
            "+ ninja_grass_offset(height_factor, turbulence, turn) * (range_fade * camera_crush.y)",
        );
        shadow_wgsl.push('\n');
        shadow_wgsl.push_str(&source.wind.replace("@group(1)", "@group(2)"));
        self.ninja = Some(Box::new(NinjaGrass {
            draw_paths,
            bind_group,
            shadow_layout,
            shadow_shader: blade_shader(device, shadow_wgsl),
            shadows: [None, None],
        }));
    }

    pub(super) fn draw_path(&self, prepared: &GrassPreparedDraw) -> &GrassDrawPath {
        let paths = if prepared.ninja_style {
            &self.ninja.as_ref().unwrap().draw_paths
        } else {
            &self.draw_paths
        };
        &paths[usize::from(prepared.gpu_precompute)]
    }

    pub(super) fn draw_path_mut(&mut self, precompute: bool, ninja_style: bool) -> &mut GrassDrawPath {
        let paths = if ninja_style {
            &mut self.ninja.as_mut().unwrap().draw_paths
        } else {
            &mut self.draw_paths
        };
        &mut paths[usize::from(precompute)]
    }

    pub(super) fn draw_bind_group(&self, prepared: &GrassPreparedDraw) -> &wgpu::BindGroup {
        if prepared.ninja_style {
            &self.ninja.as_ref().unwrap().bind_group
        } else {
            &self.bind_group
        }
    }
}

fn blade_vertex_source(base: &str, offset: &str, tangent: &str) -> String {
    base.replace(
        "// BLADE_MATERIAL_VARYINGS",
        "@location(8) @interpolate(flat) blade_tangent: vec2<f32>,\n    @location(9) blade_material_uv: vec2<f32>,",
    )
    .replace("// BLADE_MATERIAL_OFFSET", offset)
    .replace("// BLADE_MATERIAL_TANGENT", &format!(
        "{tangent}\n    out.blade_material_uv = ninja_grass_uv(input.uv, out.blade_random);"
    ))
}

fn blade_shader(device: &wgpu::Device, source: String) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Fluid Ninja opaque grass WGSL"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn sampler_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn texture_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
}
