//! Optional regional temporal resolve over the existing MSAA RT scene.
use crate::renderer::{pipeline_hash, weather, DynamicLightsMode, PipelineJobKey, Renderer};

pub(in crate::renderer) struct RtTaaResources {
    layout: wgpu::BindGroupLayout,
    pub pipeline: Option<wgpu::RenderPipeline>,
    pub input: Option<wgpu::BindGroup>,
    buffers: Option<[wgpu::Buffer; 2]>,
    format: (wgpu::TextureFormat, wgpu::TextureFormat),
}

impl Renderer {
    pub(in crate::renderer) fn rt_taa_active(&self) -> bool {
        self.rt_taa_mode != 0
            && !self.taa_enabled
            && !self.dlss_requested
            && !self.asset_preview_mode
            && self.dynamic_lights_mode == DynamicLightsMode::RayTracedHardware
            && self.ray_tracing_supported
    }

    pub(in crate::renderer) fn temporal_resolve_active(&self) -> bool {
        self.taa_enabled || self.rt_taa_active()
    }

    /// NVIDIA DLAA replaces the TAA resolve but consumes the same jittered
    /// camera and motion-vector prepass. Previews, offscreen exports, thermal
    /// vision and MSAA targets bypass it.
    pub(in crate::renderer) fn dlss_active(&self) -> bool {
        self.dlss_requested
            && !self.dlss_failed
            && !self.asset_preview_mode
            && self.offscreen_output.is_none()
            && self.thermal_vision == 0
            && self.msaa_samples == 1
    }

    /// Size of the world and screen-space targets: the window, or DLSS's
    /// lower internal resolution while it upscales (`targets.render_size`
    /// follows this via `apply_pending_scene_rebuild`).
    pub(in crate::renderer) fn render_extent(&self) -> (u32, u32) {
        let output = (self.config.width.max(1), self.config.height.max(1));
        let scale = self.dlss_mode.render_scale();
        if !self.dlss_active() || scale >= 1.0 {
            return output;
        }
        let scaled = |size: u32| ((size as f32 * scale).round() as u32).max(1);
        (scaled(output.0), scaled(output.1))
    }

    /// Whether the prepass writes motion vectors and the camera is jittered.
    pub(in crate::renderer) fn motion_vectors_wanted(&self) -> bool {
        self.temporal_resolve_active() || self.dlss_active()
    }

    pub(in crate::renderer) fn ensure_rt_taa_pipeline(&mut self) {
        if !self.rt_taa_active() {
            return;
        }
        let format = (self.config.format, self.scene_format());
        if self
            .rt_taa_resources
            .as_ref()
            .is_some_and(|r| r.format != format)
        {
            self.rt_taa_resources = None;
        }
        if self.rt_taa_resources.is_none() {
            let layout = input_layout(&self.device);
            self.rt_taa_resources = Some(RtTaaResources {
                layout,
                pipeline: None,
                input: None,
                buffers: None,
                format,
            });
        }
        let resources = self.rt_taa_resources.as_mut().unwrap();
        if let Some(world) = self.world.as_ref() {
            let buffers = [world.light_buffer.clone(), world._cluster_buffer.clone()];
            if resources.buffers.as_ref() != Some(&buffers) {
                resources.input = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("RT regional TAA live lights"),
                    layout: &resources.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffers[0].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: buffers[1].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.lighting_settings_buffer.as_entire_binding(),
                        },
                    ],
                }));
                resources.buffers = Some(buffers);
                self.history_valid = false;
            }
        } else {
            resources.input = None;
            resources.buffers = None;
        }
        if resources.pipeline.is_some() {
            return;
        }
        let key = PipelineJobKey::new("rt-regional-taa", 0, pipeline_hash(&format));
        if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
            resources.pipeline = Some(pipeline);
            self.history_valid = false;
            rverbose!(
                1,
                "RT regional TAA ready (MSAA retained, no extra RT trace pass)"
            );
        } else {
            let device = self.device.clone();
            let post_layout = self.post_layout.clone();
            let input_layout = resources.layout.clone();
            self.pipeline_jobs.request(key, "RT regional TAA", move || {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("RT regional TAA shader"),
                    source: wgpu::ShaderSource::Wgsl(
                        source().expect("RT TAA shader anchors").into(),
                    ),
                });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("RT regional TAA/post layout"),
                    bind_group_layouts: &[Some(&post_layout), Some(&input_layout)],
                    immediate_size: 0,
                });
                super::pipelines::create_taa_post_pipeline(
                    &device, &layout, &shader, format.0, format.1,
                )
            });
        }
    }
}

pub(super) fn input_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("RT regional TAA influence layout"),
        entries: &[
            crate::renderer::storage_buffer_entry(0, true, wgpu::ShaderStages::FRAGMENT),
            crate::renderer::storage_buffer_entry(1, true, wgpu::ShaderStages::FRAGMENT),
            crate::renderer::uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
        ],
    })
}

pub(super) fn source() -> Result<String, String> {
    let post = weather::with_weather_surface(&weather::with_post_settings(include_str!(
        "../../post.wgsl"
    )));
    let old = declaration(&post, "fn taa_resolve_bevy(")?;
    let mut source = post.replacen(
        old,
        "fn taa_resolve_bevy(uv: vec2<f32>) -> TaaResolve { return rt_taa_resolve(uv); }",
        1,
    );
    // The unused cloud entry point has its own texture binding; keep it out of
    // the regional light group while preserving module validation.
    source = source.replace(
        "@group(1) @binding(0) var cloud_march_texture",
        "@group(2) @binding(0) var cloud_march_texture",
    );
    let anchor = "    let taa = taa_resolve_bevy(input.uv);";
    if !source.contains(anchor) {
        return Err("missing TAA fragment hook".into());
    }
    source = source.replacen(anchor, "    if (settings.aa.w > 2.5) {\n        let affected = rt_taa_mask(input.uv, centre_depth);\n        let debug = select(vec3<f32>(0.0), vec3<f32>(0.0, 1.0, 0.0), affected);\n        return TaaFragmentOut(vec4<f32>(debug, 1.0), vec4<f32>(0.0));\n    }\n    let taa = taa_resolve_bevy(input.uv);", 1);
    for name in ["PointLight", "ClusterRecord", "LightingSettings"] {
        source.push_str(declaration(
            include_str!("../../bsp.wgsl"),
            &format!("struct {name} "),
        )?);
        source.push_str(";\n");
    }
    source.push_str(include_str!("../../rt_taa.wgsl"));
    Ok(source)
}

fn declaration<'a>(source: &'a str, anchor: &str) -> Result<&'a str, String> {
    let start = source
        .find(anchor)
        .ok_or_else(|| format!("missing {anchor}"))?;
    let body = start
        + source[start..]
            .find('{')
            .ok_or_else(|| format!("missing body {anchor}"))?;
    let mut depth = 0;
    for (offset, byte) in source.as_bytes()[body..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&source[start..body + offset + 1]);
                }
            }
            _ => {}
        }
    }
    Err(format!("unterminated {anchor}"))
}

#[cfg(test)]
mod tests;
