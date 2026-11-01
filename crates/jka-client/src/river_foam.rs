//! Native foam field (stages 4/16). The foam detail texture itself ships as an
//! ordinary 8-bit PNG and loads through the JKA texture path (river_surface);
//! `Detail` here is the native RGBA16F archive the parity tests check against.
use jka_water::river::foam_field::{self, Frame, Setup};
#[cfg(test)]
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::io::{Cursor, Read};
use wgpu::util::DeviceExt;

#[cfg(test)]
pub struct Detail {
    pub texture: wgpu::Texture,
    pub sampler: wgpu::Sampler,
}

#[cfg(test)]
#[derive(serde::Deserialize)]
struct Reference {
    mips: Vec<Mip>,
}
#[cfg(test)]
#[derive(serde::Deserialize)]
struct Mip {
    name: String,
    size: u32,
    bytes: u64,
    sha256: String,
}

#[cfg(test)]
pub const REFERENCE: &str = include_str!("../../jka-water/src/river/foam-texture-reference.json");

#[cfg(test)]
pub fn explicit_mip_filter(backend: wgpu::Backend) -> Result<wgpu::MipmapFilterMode, String> {
    match backend {
        // Native D3D12State.cpp maps SF_AnisotropicPoint to linear mips, even
        // when MaxAnisotropy=1. VulkanState.cpp retains nearest mip selection.
        wgpu::Backend::Dx12 => Ok(wgpu::MipmapFilterMode::Linear),
        wgpu::Backend::Vulkan => Ok(wgpu::MipmapFilterMode::Nearest),
        _ => Err("native river sampler mapping is traced only for DX12/Vulkan".into()),
    }
}

#[cfg(test)]
impl Detail {
    /// Creates/uploads the persistent source texture once. Render-time sampling
    /// needs no new resources, decompression, uploads or CPU readback.
    pub fn from_archive(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        archive: &[u8],
    ) -> Result<Self, String> {
        let reference: Reference = serde_json::from_str(REFERENCE).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(|e| e.to_string())?;
        if zip.len() != reference.mips.len() {
            return Err("native foam archive mip count differs".into());
        }
        let mut mips = Vec::with_capacity(reference.mips.len());
        for (i, desc) in reference.mips.iter().enumerate() {
            let entry = zip.by_index(i).map_err(|e| e.to_string())?;
            if entry.name() != desc.name
                || entry.size() != desc.bytes
                || entry.is_dir()
                || desc.bytes != u64::from(desc.size) * u64::from(desc.size) * 8
            {
                return Err(format!("native foam mip {i} archive shape differs"));
            }
            let mut data = Vec::with_capacity(desc.bytes as usize);
            entry
                .take(desc.bytes + 1)
                .read_to_end(&mut data)
                .map_err(|e| e.to_string())?;
            if format!("{:x}", Sha256::digest(&data)) != desc.sha256 {
                return Err(format!("native foam mip {i} checksum differs"));
            }
            mips.push(data);
        }
        Self::upload(device, queue, &mips)
    }

    /// Raw RGBA16F mips 0..12 of the 2048x2048 source.
    fn upload(device: &wgpu::Device, queue: &wgpu::Queue, mips: &[Vec<u8>]) -> Result<Self, String> {
        let mipmap_filter = explicit_mip_filter(device.adapter_info().backend)?;
        if mips.len() != 12 || mips.iter().enumerate().any(|(i, m)| m.len() as u64 != u64::from(2048u32 >> i).pow(2) * 8) {
            return Err("native foam detail needs twelve RGBA16F mips from 2048x2048".into());
        }
        if device.limits().max_texture_dimension_2d < 2048 {
            return Err("native foam detail requires 2048x2048 texture support".into());
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native T_ShallowWater_Foam HDR detail"),
            size: wgpu::Extent3d {
                width: 2048,
                height: 2048,
                depth_or_array_layers: 1,
            },
            mip_level_count: 12,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, data) in mips.iter().enumerate() {
            let size = 2048u32 >> i;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: i as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 8),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
        }
        // Explicit SampleLevel is isotropic. Keep the native RHI's actual mip
        // mapping, including D3D12's promotion of requested point mips.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native foam World-group explicit-LOD sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            mipmap_filter,
            ..Default::default()
        });
        Ok(Self { texture, sampler })
    }
    pub fn logical_bytes(&self) -> u64 {
        (0..12).map(|m| u64::from(2048u32 >> m).pow(2) * 8).sum()
    }
}

/// Native FoamGrid producer (cached stages 4/16). Its current output is also
/// FoamRT for the material. Allocate per river; bind groups and pipelines are
/// built once. Frames record a 16-byte control copy and one dispatch.
pub struct Field {
    dimensions: [u32; 2],
    foam: [wgpu::Texture; 2],
    current: usize,
    init: wgpu::ComputePipeline,
    stage: wgpu::ComputePipeline,
    init_groups: [wgpu::BindGroup; 2],
    /// [hydraulic state parity][FoamGrid read parity]
    groups: [[Option<wgpu::BindGroup>; 2]; 2],
    uniform: wgpu::Buffer,
}

/// Previous-buffer readers of stage 16: Simulation's R32f hydraulic arrays and
/// the material's pre-blur half NormalGrid.
pub struct FieldInputs<'a> {
    pub state: &'a [wgpu::Texture; 2],
    pub normal_grid: &'a wgpu::Texture,
}

impl Field {
    pub fn new(device: &wgpu::Device, inputs: FieldInputs<'_>, setup: &Setup) -> Result<Self, String> {
        let dimensions = setup.dimensions;
        let limits = device.limits();
        let size = |t: &wgpu::Texture| [t.width(), t.height()];
        if dimensions.contains(&0)
            || dimensions.iter().any(|n| {
                *n > limits.max_texture_dimension_2d
                    || n.div_ceil(8) > limits.max_compute_workgroups_per_dimension
            })
            || inputs.state.iter().any(|t| {
                size(t) != dimensions
                    || t.depth_or_array_layers() != 4
                    || t.format() != crate::river_gpu::WATER_FORMAT
            })
            || size(inputs.normal_grid) != dimensions
            || inputs.normal_grid.format() != wgpu::TextureFormat::Rgba16Float
            || !(setup.dx > 0.0 && setup.foam_dx > 0.0)
            || setup.sim_res.iter().any(|v| !(*v >= 1.0))
            || setup.unit_to_world.iter().flatten().any(|v| !v.is_finite())
        {
            return Err("invalid native river foam grid inputs or GPU limits".into());
        }
        let foam = std::array::from_fn(|i| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(["native half FoamGrid/FoamRT 0", "native half FoamGrid/FoamRT 1"][i]),
                size: wgpu::Extent3d {
                    width: dimensions[0],
                    height: dimensions[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        });
        let mut bytes = Vec::with_capacity(144);
        bytes.extend_from_slice(bytemuck::cast_slice(&setup.uniform()));
        bytes.extend_from_slice(&[0; 16]);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native river foam stage controls"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native river foam stages 4/16"),
            source: wgpu::ShaderSource::Wgsl(foam_field::shader().into()),
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let (init, stage) = (pipeline("init_foam"), pipeline("foam_stage"));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Niagara FoamGrid/reader SF_Bilinear AM_Clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let views = foam.each_ref().map(|t| t.create_view(&Default::default()));
        let state = inputs.state.each_ref().map(|t| t.create_view(&Default::default()));
        let normal = inputs.normal_grid.create_view(&Default::default());
        let init_groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native river Init Foam"),
                layout: &init.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&views[i]),
                }],
            })
        });
        let groups = std::array::from_fn(|s| {
            std::array::from_fn(|f| {
                let resources = [
                    (0, uniform.as_entire_binding()),
                    (1, wgpu::BindingResource::TextureView(&state[s])),
                    (2, wgpu::BindingResource::TextureView(&normal)),
                    (3, wgpu::BindingResource::TextureView(&views[f])),
                    (4, wgpu::BindingResource::TextureView(&views[1 - f])),
                    (5, wgpu::BindingResource::Sampler(&sampler)),
                ];
                let entries: Vec<_> = resources
                    .into_iter()
                    .map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource })
                    .collect();
                Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("native river Foam stage"),
                    layout: &stage.get_bind_group_layout(0),
                    entries: &entries,
                }))
            })
        });
        Ok(Self {
            dimensions,
            foam,
            current: 0,
            init,
            stage,
            init_groups,
            groups,
            uniform,
        })
    }

    fn dispatch(&mut self, encoder: &mut wgpu::CommandEncoder, state: Option<usize>) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native river foam stage"),
            timestamp_writes: None,
        });
        match state {
            None => {
                pass.set_pipeline(&self.init);
                pass.set_bind_group(0, &self.init_groups[1 - self.current], &[]);
            }
            Some(s) => {
                pass.set_pipeline(&self.stage);
                pass.set_bind_group(0, self.groups[s][self.current].as_ref(), &[]);
            }
        }
        pass.dispatch_workgroups(self.dimensions[0].div_ceil(8), self.dimensions[1].div_ceil(8), 1);
        drop(pass);
        self.current = 1 - self.current;
    }

    /// Stage 4 (OnSimulationReset): FoamGrid.Foam = 0 for every cell.
    pub fn encode_reset(&mut self, encoder: &mut wgpu::CommandEncoder) {
        self.dispatch(encoder, None);
    }

    /// Stage 16 (Always), after the frame's hydraulic outputs and the material's
    /// NormalGrid write. The control copy keeps queued frames independent.
    pub fn encode_frame(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        state_index: usize,
        frame: Frame,
    ) -> Result<(), &'static str> {
        if state_index > 1 || self.groups[state_index][0].is_none() {
            return Err("river foam hydraulic state binding is not retained");
        }
        if !(frame.delta.is_finite() && frame.delta >= 0.0 && frame.time.is_finite()) {
            return Err("invalid river foam frame time");
        }
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native river foam frame controls"),
            contents: bytemuck::cast_slice(&frame.uniform()),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&upload, 0, &self.uniform, 128, 16);
        self.dispatch(encoder, Some(state_index));
        Ok(())
    }

    /// After hydraulics freeze, keep only the final state's bindings so the
    /// other ping-pong array can be released.
    pub fn retain_state(&mut self, state_index: usize) {
        for (i, groups) in self.groups.iter_mut().enumerate() {
            if i != state_index {
                *groups = [None, None];
            }
        }
    }

    /// Current FoamGrid output; also the native FoamRT value (R channel).
    pub fn foam(&self) -> &wgpu::Texture {
        &self.foam[self.current]
    }

    /// Both FoamGrid textures and which one `foam()` is now, so a material can
    /// keep one bind group per parity instead of rebuilding per frame.
    pub fn foam_pair(&self) -> (&[wgpu::Texture; 2], usize) {
        (&self.foam, self.current)
    }

    /// Owned FoamGrid ping-pong pair (8 bytes/cell each) plus controls.
    pub fn logical_bytes(&self) -> u64 {
        2 * u64::from(self.dimensions[0]) * u64::from(self.dimensions[1]) * 8 + 144
    }
}
