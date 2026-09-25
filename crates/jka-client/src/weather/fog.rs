use crate::{
    materials::{BlendFactor, BlendFunc, MaterialStage, StageTexture},
    ui::FogMode,
};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use jka_assets::{bsp::Bsp, shader::Shader};
use std::collections::BTreeMap;
use wgpu::util::DeviceExt;

pub(crate) const VOLUMETRIC_FOG_BASE_DENSITY: f32 = 0.00032;
pub(crate) const VOLUMETRIC_FOG_DEFAULT_COLOR: [f32; 3] = [0.16, 0.205, 0.265];
pub(crate) const FROXEL_X: u32 = 80;
pub(crate) const FROXEL_Y: u32 = 45;
pub(crate) const FROXEL_Z: u32 = 48;
const FROXEL_COUNT: u64 = (FROXEL_X as u64) * (FROXEL_Y as u64) * (FROXEL_Z as u64);
const SHADOW_CASCADES: usize = 4;
const SHADOW_MAP_SIZE: u32 = 2048;
const SHADOW_SPLITS: [f32; SHADOW_CASCADES] = [1024.0, 4096.0, 16384.0, 0.0];
const FALLBACK_SUN_DIRECTION: [f32; 3] = [-0.38, -0.84, -0.39];
const FALLBACK_SUN_COLOR: [f32; 3] = [0.57735026, 0.57735026, 0.57735026];
const FALLBACK_SUN_INTENSITY: f32 = 250.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FogColorOverride {
    None,
    Black,
    White,
}

pub(crate) fn stage_fog_color_override(
    stage: &MaterialStage,
    first: bool,
) -> FogColorOverride {
    // OpenJK changes the GL fog color for individual multipass stages so that
    // fogging does not destroy the blend equation. In particular, a lightmap
    // multiply stage fogs toward white rather than toward the map fog color.
    match stage.blend {
        Some(BlendFunc {
            src: BlendFactor::One,
            dst: BlendFactor::One,
        }) => FogColorOverride::Black,
        Some(BlendFunc {
            src: BlendFactor::Zero,
            dst: BlendFactor::Zero,
        })
        | Some(BlendFunc {
            src: BlendFactor::One,
            dst: BlendFactor::Zero,
        }) => FogColorOverride::White,
        None if !first => FogColorOverride::White,
        Some(BlendFunc {
            src: BlendFactor::DstColor,
            dst: BlendFactor::Zero,
        }) if matches!(stage.texture, StageTexture::Lightmap) => FogColorOverride::White,
        Some(BlendFunc {
            src: BlendFactor::DstColor,
            dst: BlendFactor::Zero,
        })
        | Some(BlendFunc {
            src: BlendFactor::One,
            dst: BlendFactor::OneMinusSrcColor,
        }) => FogColorOverride::Black,
        _ => FogColorOverride::None,
    }
}

pub(crate) fn bsp_fog_params(
    bsp: &Bsp,
    library: &BTreeMap<String, Shader>,
    fog_num: i32,
) -> [f32; 4] {
    let Some(fog) = fog_num
        .try_into()
        .ok()
        .and_then(|index: usize| bsp.fogs.get(index))
    else {
        return [0.0; 4];
    };
    let name = String::from_utf8_lossy(&fog.shader).to_ascii_lowercase();
    let Some(parameters) = library.get(&name).and_then(|shader| shader.fogparms) else {
        return [0.0; 4];
    };
    [
        parameters.color[0],
        parameters.color[1],
        parameters.color[2],
        parameters.depth_for_opaque,
    ]
}

pub(crate) fn bsp_global_fog_num(bsp: &Bsp) -> Option<i32> {
    bsp.fogs
        .iter()
        .enumerate()
        .rev()
        .find(|(_, fog)| fog.brush_num == -1)
        .and_then(|(index, _)| i32::try_from(index).ok())
}

pub(crate) fn bsp_global_fog_params(
    bsp: &Bsp,
    library: &BTreeMap<String, Shader>,
) -> Option<[f32; 4]> {
    // OpenJK marks a compiled-map global fog with fog.brushNum == -1. If a
    // malformed BSP contains more than one, its load loop leaves the last one
    // as world.globalFog, so mirror that behavior by searching from the end.
    let fog = bsp.fogs.iter().rev().find(|fog| fog.brush_num == -1)?;
    let name = String::from_utf8_lossy(&fog.shader).to_ascii_lowercase();
    let parameters = library.get(&name)?.fogparms?;
    let depth = parameters.depth_for_opaque;
    if !depth.is_finite() || depth <= 0.001 || !parameters.color.iter().all(|v| v.is_finite()) {
        return None;
    }
    Some([
        parameters.color[0],
        parameters.color[1],
        parameters.color[2],
        depth,
    ])
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_buffer_entry(
    binding: u32,
    read_only: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn unfilterable_texture_entry_compute(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn filterable_texture_entry_compute(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn filtering_sampler_entry_compute(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct FroxelUniform {
    pub inv_view_proj: [[f32; 4]; 4],
    pub camera_pos_time: [f32; 4],
    pub sun_direction_enabled: [f32; 4],
    pub sun_color_intensity: [f32; 4],
    pub fog_params: [f32; 4],
    pub fog_color_anisotropy: [f32; 4],
    // x: rain enabled, y: intensity, z: haze strength.
    pub rain_params: [f32; 4],
    // min render X/Z, inverse weather-heightfield extent X/Z.
    pub rain_occlusion: [f32; 4],
    // x/z prevailing atmospheric velocity in JKA units/second.
    pub rain_wind: [f32; 4],
    pub shadow_view_proj: [[[f32; 4]; 4]; SHADOW_CASCADES],
    pub split_depths: [f32; 4],
    pub shadow_params: [f32; 4],
    pub grid: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SurfaceFogUniform {
    color_depth: [f32; 4],
    // x: this surface uses the BSP global fog; y: OpenJK stage fog-color
    // override (0 none, 1 black, 2 white).
    flags: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LegacyFogControl {
    // x: enabled. y: 0 means authored MAP at 1.0x; on maps with fog, a
    // non-zero value is a multiplier of that authored curve. z says the map
    // has authored fog at all. Without map fog, y remains the manual strength.
    values: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct MapFogSummary {
    color: [f32; 3],
    depth_for_opaque: f32,
}

pub(crate) struct FroxelResources {
    pub buffer: wgpu::Buffer,
    pub uniform_buffer: wgpu::Buffer,
    pub layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::ComputePipeline,
}

pub(crate) struct FogSystem {
    pub mode: FogMode,
    pub strength: f32,
    pub resources: FroxelResources,
    legacy_control_buffer: wgpu::Buffer,
    map_fog: Option<MapFogSummary>,
    global_fog: Option<MapFogSummary>,
}

impl FogSystem {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let legacy_control_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA legacy fog control uniform"),
            contents: bytemuck::bytes_of(&LegacyFogControl { values: [0.0; 4] }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            mode: FogMode::Off,
            strength: 0.0,
            resources: create_froxel_resources(device),
            legacy_control_buffer,
            map_fog: None,
            global_fog: None,
        }
    }

    pub(crate) fn install_map<I>(&mut self, global_fog: Option<[f32; 4]>, batches: I)
    where
        I: IntoIterator<Item = ([f32; 4], u64)>,
    {
        self.global_fog = global_fog.and_then(map_fog_summary);
        // Prefer the compiled global-fog parameters for the MAP control. Surface
        // participation remains driven by each BSP dsurface fog assignment.
        // Fall back to the dominant local fog only when there is no global fog.
        self.map_fog = self.global_fog.or_else(|| summarize_map_fog(batches));

        if let Some(fog) = self.map_fog {
            println!(
                "Map fog: rgb=({:.3}, {:.3}, {:.3}) depth={:.1}; MAP baseline=1.00x{}",
                fog.color[0],
                fog.color[1],
                fog.color[2],
                fog.depth_for_opaque,
                if self.global_fog.is_some() {
                    " (BSP global fog)"
                } else {
                    ""
                },
            );
        } else {
            println!("Map fog: none; MAP/DEFAULT fog remains inactive");
        }
    }

    pub(crate) fn legacy_control_buffer(&self) -> &wgpu::Buffer {
        &self.legacy_control_buffer
    }

    pub(crate) fn legacy_effective(&self) -> bool {
        self.mode == FogMode::Legacy && (self.strength > 0.001 || self.map_fog.is_some())
    }

    pub(crate) fn volumetric_effective(&self) -> bool {
        self.mode == FogMode::Volumetric && (self.strength > 0.001 || self.map_fog.is_some())
    }

    pub(crate) fn volumetric_density_color(&self) -> (f32, [f32; 3], f32, f32) {
        // Rain can run the shared froxel pass even when the user's ordinary fog
        // mode is OFF/LEGACY. In that case the base fog contribution must stay
        // zero so rain does not accidentally promote authored map fog.
        if self.mode != FogMode::Volumetric {
            return (0.0, VOLUMETRIC_FOG_DEFAULT_COLOR, 0.00032, 0.0);
        }
        if let Some(map_fog) = self.map_fog {
            return (
                // For authored fog this field is a scale, not a Beer-Lambert
                // density. MAP (the zero sentinel) and an explicit 1.0x must
                // produce exactly the same curve and color.
                authored_fog_strength_scale(self.strength),
                map_fog.color,
                // Authored MAP fog follows the JKA EXP2 extinction curve in
                // the froxel integrator, without our stylized height fog.
                0.0,
                map_fog.depth_for_opaque,
            );
        }
        if self.strength > 0.001 {
            return (
                VOLUMETRIC_FOG_BASE_DENSITY * self.strength,
                VOLUMETRIC_FOG_DEFAULT_COLOR,
                0.00032,
                0.0,
            );
        }
        (0.0, VOLUMETRIC_FOG_DEFAULT_COLOR, 0.00032, 0.0)
    }

    pub(crate) fn write_legacy_control(&self, queue: &wgpu::Queue) {
        let fog_control = LegacyFogControl {
            values: [
                if self.legacy_effective() { 1.0 } else { 0.0 },
                self.strength,
                if self.map_fog.is_some() { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        queue.write_buffer(
            &self.legacy_control_buffer,
            0,
            bytemuck::bytes_of(&fog_control),
        );
    }

    pub(crate) fn clear_color(&self) -> wgpu::Color {
        if let Some(fog) = self.global_fog {
            return wgpu::Color {
                r: f64::from(fog.color[0]),
                g: f64::from(fog.color[1]),
                b: f64::from(fog.color[2]),
                a: 1.0,
            };
        }
        wgpu::Color {
            r: 0.12,
            g: 0.18,
            b: 0.25,
            a: 1.0,
        }
    }
}

pub(crate) fn create_surface_fog_buffer(
    device: &wgpu::Device,
    color_depth: [f32; 4],
    is_global: bool,
    color_override: FogColorOverride,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA surface fog uniform"),
        contents: bytemuck::bytes_of(&SurfaceFogUniform {
            // Keep the BSP surface's actual fog assignment. OpenJK's
            // main-world loader does not replace every fogNum=-1 surface with
            // globalFog; it uses the compiled dsurface fog index.
            color_depth,
            flags: [
                if is_global { 1.0 } else { 0.0 },
                match color_override {
                    FogColorOverride::None => 0.0,
                    FogColorOverride::Black => 1.0,
                    FogColorOverride::White => 2.0,
                },
                0.0,
                0.0,
            ],
        }),
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn map_fog_summary(values: [f32; 4]) -> Option<MapFogSummary> {
    if !values.iter().all(|value| value.is_finite()) || values[3] <= 0.001 {
        return None;
    }
    Some(MapFogSummary {
        color: [values[0], values[1], values[2]],
        depth_for_opaque: values[3],
    })
}

fn summarize_map_fog<I>(batches: I) -> Option<MapFogSummary>
where
    I: IntoIterator<Item = ([f32; 4], u64)>,
{
    // A BSP can reference more than one fog definition. Volumetric fog is a
    // single global froxel volume, so use the dominant authored fog signature
    // rather than blending unrelated colors/depths together. Weight by the
    // amount of submitted triangle-list geometry using each exact fog tuple.
    let mut weights = BTreeMap::<[u32; 4], u64>::new();
    for (fog, vertex_count) in batches {
        if !fog.iter().all(|value| value.is_finite()) || fog[3] <= 0.001 {
            continue;
        }
        let key = [
            fog[0].to_bits(),
            fog[1].to_bits(),
            fog[2].to_bits(),
            fog[3].to_bits(),
        ];
        *weights.entry(key).or_default() += vertex_count.max(1);
    }

    weights
        .into_iter()
        .max_by_key(|(_, weight)| *weight)
        .map(|(fog, _)| MapFogSummary {
            color: [
                f32::from_bits(fog[0]),
                f32::from_bits(fog[1]),
                f32::from_bits(fog[2]),
            ],
            depth_for_opaque: f32::from_bits(fog[3]),
        })
}

fn authored_fog_strength_scale(setting: f32) -> f32 {
    // Zero is the UI/config sentinel for unmodified MAP fog. Once the user
    // moves the slider, the value is a true multiplier of the authored curve,
    // so 1.0x is intentionally identical to MAP.
    if setting > 0.001 {
        setting
    } else {
        1.0
    }
}

fn create_froxel_resources(device: &wgpu::Device) -> FroxelResources {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA froxel volumetric fog layout"),
        entries: &[
            uniform_entry(0, wgpu::ShaderStages::COMPUTE),
            storage_buffer_entry(1, false, wgpu::ShaderStages::COMPUTE),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            storage_buffer_entry(3, true, wgpu::ShaderStages::COMPUTE),
            storage_buffer_entry(4, true, wgpu::ShaderStages::COMPUTE),
            uniform_entry(5, wgpu::ShaderStages::COMPUTE),
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::CubeArray,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            // Rain atmosphere reuses the same map-wide roof-height texture and
            // canonical GodotGrass wind field as precipitation.
            unfilterable_texture_entry_compute(8),
            filterable_texture_entry_compute(9),
            filtering_sampler_entry_compute(10),
            wgpu::BindGroupLayoutEntry {
                binding: 11,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("JKA froxel volumetric fog shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../froxel_fog.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("JKA froxel volumetric fog pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("JKA froxel volumetric fog pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA froxel volumetric fog uniform"),
        contents: bytemuck::bytes_of(&FroxelUniform {
            inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            camera_pos_time: [0.0; 4],
            sun_direction_enabled: [
                FALLBACK_SUN_DIRECTION[0],
                FALLBACK_SUN_DIRECTION[1],
                FALLBACK_SUN_DIRECTION[2],
                1.0,
            ],
            sun_color_intensity: [
                FALLBACK_SUN_COLOR[0],
                FALLBACK_SUN_COLOR[1],
                FALLBACK_SUN_COLOR[2],
                FALLBACK_SUN_INTENSITY,
            ],
            fog_params: [VOLUMETRIC_FOG_BASE_DENSITY, 0.00032, 768.0, 0.0],
            fog_color_anisotropy: [0.16, 0.205, 0.265, 0.35],
            rain_params: [0.0; 4],
            rain_occlusion: [0.0; 4],
            rain_wind: [0.0; 4],
            shadow_view_proj: [Mat4::IDENTITY.to_cols_array_2d(); SHADOW_CASCADES],
            split_depths: SHADOW_SPLITS,
            shadow_params: [SHADOW_MAP_SIZE as f32, 0.0006, 0.0, 0.0],
            grid: [FROXEL_X, FROXEL_Y, FROXEL_Z, 0],
        }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("JKA integrated froxel fog volume"),
        size: FROXEL_COUNT * std::mem::size_of::<[f32; 4]>() as u64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    FroxelResources {
        buffer,
        uniform_buffer,
        layout,
        pipeline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_map_fog_zero_sentinel_and_one_x_match() {
        assert!((authored_fog_strength_scale(0.0) - 1.0).abs() < 1e-6);
        assert!((authored_fog_strength_scale(1.0) - 1.0).abs() < 1e-6);
        assert!((authored_fog_strength_scale(0.5) - 0.5).abs() < 1e-6);
        assert!((authored_fog_strength_scale(2.0) - 2.0).abs() < 1e-6);
    }
}
