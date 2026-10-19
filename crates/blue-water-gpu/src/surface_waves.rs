//! Bind Blue Water's field to the original surface shader's group 2 contract.
//! The preserved WGSL expects all nine entries, including calm hull and open
//! shore resources when JKA has no matching upstream simulation.
use blue_water_core::{groups, spectrum, whitecaps, CascadeInfo};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::WaveField;

const CASCADES: usize = 3;
const FETCH_TABLE: usize = 16;
const BUBBLE_LIFE: f64 = 0.5;
const TAIL_ROUGHNESS: f64 = 0.25;
const SLICK_WAVELENGTH: f64 = 25.0;

#[derive(Clone, Debug, Default)]
pub struct SpectrumStats {
    slopes: [f32; 4],
    slope_lengths: [f32; 4],
    heights: [f32; 4],
    tail: f32,
    shore_tables: [[f32; 4]; FETCH_TABLE],
}

impl SpectrumStats {
    /// CPU equivalent of the coastal shader, for an on-demand diagnostic (no GPU readback).
    pub fn sample_shore(&self, map: &crate::shore::ShoreMap, at: [f64; 2], fft: f32, swell_height: f32) -> crate::shore::ShoreSample {
        let tables: Vec<[f64; FETCH_TABLE]> = (0..CASCADES)
            .map(|i| std::array::from_fn(|row| f64::from(self.shore_tables[row][i])))
            .collect();
        let mut heights = self.heights.map(|h| f64::from(h * fft));
        heights[3] = f64::from(swell_height);
        map.sample(at, &tables, heights)
    }
    pub fn set_shore_tables(&mut self, spectrum: &spectrum::WaveSpectrum, wind: f64) {
        for (i, cascade) in spectrum.cascades.iter().enumerate().take(CASCADES) {
            let modes = crate::hull_sea::cascade_modes(&cascade.amplitudes, cascade.resolution as usize, cascade.size);
            for (row, value) in self.shore_tables.iter_mut().zip(crate::shore::band_fetch_table(&modes, wind)) {
                row[i] = value as f32;
            }
        }
    }
    pub fn from_spectrum(spectrum: &spectrum::WaveSpectrum) -> Self {
        let mut stats = Self {
            tail: spectrum.tail_slope_variance as f32,
            ..Default::default()
        };
        for (i, cascade) in spectrum.cascades.iter().enumerate().take(CASCADES) {
            stats.slopes[i] = cascade.slope_variance as f32;
            stats.slope_lengths[i] = cascade.slope_length as f32;
            stats.heights[i] = (4.0 * cascade.variance.max(0.0).sqrt()) as f32;
        }
        stats
    }
}

/// Byte-for-byte layout of `WaveUniforms` in the upstream `waves.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct WaveUniforms {
    pub tile: [f32; 4],
    pub texels: [f32; 4],
    pub longest: [f32; 4],
    pub slopes: [f32; 4],
    pub slope_lengths: [f32; 4],
    pub gates: [f32; 4],
    pub gust_shares: [f32; 4],
    pub groups: [[f32; 4]; CASCADES],
    pub tail: [f32; 4],
    pub info: [f32; 4],
    pub wind: [f32; 4],
    pub gusts: [f32; 4],
    pub sea: [f32; 4],
    pub slick_shares: [f32; 4],
    pub shore_heights: [f32; 4],
    pub shore_exposure: [f32; 4],
    pub shore_waves: [f32; 4],
    pub shore: [f32; 4],
    pub shore_fetch: [[f32; 4]; FETCH_TABLE],
}

fn short_shares(cascades: &[CascadeInfo], wavelength: f64) -> Vec<f64> {
    spectrum::cascade_bands(cascades)
        .iter()
        .enumerate()
        .map(|(i, band)| {
            let lo = if i == 0 {
                std::f64::consts::TAU / cascades[0].size
            } else {
                band.lo
            };
            let cut = lo.max(std::f64::consts::TAU / wavelength);
            ((band.hi / cut).ln() / (band.hi / lo).ln()).max(0.0)
        })
        .collect()
}

impl WaveUniforms {
    pub fn from_field(field: &WaveField, time: f64, foam_coverage: f64, foam_lifetime: f64) -> Self {
        let cascades = field.cascades();
        let sea = field.sea();
        let input = field.input_parameters();
        let stats = field.surface_stats();
        let bands = spectrum::cascade_bands(cascades);
        let (wz, wx) = sea.wind_direction.sin_cos();
        let share = whitecaps::whitecap_area(input.wind_speed, foam_coverage);
        let mut u = Self {
            slopes: stats.slopes,
            slope_lengths: stats.slope_lengths,
            shore_waves: stats.heights,
            tail: [stats.tail * TAIL_ROUGHNESS as f32, stats.tail, 0.0, 0.0],
            info: [
                (field.resolution() / 16).ilog2() as f32,
                cascades.len() as f32,
                field.resolution() as f32,
                share as f32,
            ],
            wind: [
                wx as f32,
                wz as f32,
                time as f32,
                groups::group_cut(groups::group_share(share)) as f32,
            ],
            gusts: [blue_water_core::gusts::gust_strength(sea.wind_speed) as f32,
                sea.wind_speed as f32, 1.0, 1.0],
            sea: [
                sea.significant_height as f32,
                input.wind_speed as f32,
                input.wind_direction as f32,
                0.0,
            ],
            ..Default::default()
        };
        let gust = short_shares(cascades, blue_water_core::gusts::GUST_WAVELENGTH);
        let slick = short_shares(cascades, SLICK_WAVELENGTH);
        let mut coarser: f64 = 0.0;
        for (i, cascade) in cascades.iter().enumerate() {
            u.tile[i] = cascade.size as f32;
            u.texels[i] = (f64::from(field.resolution()) / cascade.size) as f32;
            u.longest[i] = if i == 0 {
                cascade.size
            } else {
                std::f64::consts::TAU / bands[i].lo
            } as f32;
            u.gates[i] = if coarser > 0.0 {
                1.0 / coarser.sqrt()
            } else {
                0.0
            } as f32;
            coarser += sea.choppiness.max(0.0).powi(2) * f64::from(stats.slopes[i]);
            u.gust_shares[i] = gust[i] as f32;
            u.slick_shares[i] = slick[i] as f32;
            if let Some(breaking) = field.breaking().get(i) {
                let group = groups::group_layout(breaking.period, cascade.size);
                let decay = (foam_lifetime * breaking.period).max(1e-3);
                u.groups[i] = [
                    (1.0 / group.length) as f32,
                    (1.0 / group.width) as f32,
                    group.drift as f32,
                    (decay * BUBBLE_LIFE / (1.0 - BUBBLE_LIFE)) as f32,
                ];
            }
        }
        if cascades.len() > 1 {
            let fine = cascades.len() - 1;
            let ripple = cascades[fine].size * bands[fine].lo / bands[fine].hi;
            u.tail[2] = ripple as f32;
            u.tail[3] = (f64::from(field.resolution()) / ripple) as f32;
        }
        u
    }
}

fn texture_entry(binding: u32, array: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: if array {
                wgpu::TextureViewDimension::D2Array
            } else {
                wgpu::TextureViewDimension::D2
            },
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// The original surface shader's wave resources. Bind `group()` at group 2
/// after calling `write` for the field's current frame.
pub struct SurfaceWaves {
    layout: wgpu::BindGroupLayout,
    groups: [wgpu::BindGroup; 2],
    uniforms: wgpu::Buffer,
    _hull_sea: wgpu::Buffer,
    _shore: wgpu::Texture,
    _field_sampler: wgpu::Sampler,
    _shore_sampler: wgpu::Sampler,
    shore: Option<std::sync::Arc<crate::shore::ShoreMap>>,
}

impl SurfaceWaves {
    pub fn new(device: &wgpu::Device, field: &WaveField) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water original surface waves"),
            entries: &[
                texture_entry(0, true),
                texture_entry(1, true),
                texture_entry(2, true),
                sampler_entry(3),
                uniform_entry(4),
                uniform_entry(5),
                texture_entry(6, false),
                texture_entry(7, false),
                sampler_entry(8),
            ],
        });
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Blue Water original wave uniforms"),
            contents: bytemuck::bytes_of(&WaveUniforms::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // Upstream's calm hull uniform: 32 slots, 2 combat waves, 16 swell
        // waves, origin, count and clock. Zero count disables hull effects.
        let hull_sea = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blue Water calm hull sea"),
            size: (53 * 16) as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let shore = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Blue Water open shore"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shore_view = shore.create_view(&Default::default());
        let field_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Blue Water field sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let shore_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Blue Water open shore sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let groups = std::array::from_fn(|set| {
            let views = field.views_for(set);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blue Water original surface wave group"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[0]) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[1]) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[2]) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&field_sampler) },
                    wgpu::BindGroupEntry { binding: 4, resource: uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: hull_sea.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&shore_view) },
                    wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&shore_view) },
                    wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(&shore_sampler) },
                ],
            })
        });
        Self {
            layout,
            groups,
            uniforms,
            _hull_sea: hull_sea,
            _shore: shore,
            _field_sampler: field_sampler,
            _shore_sampler: shore_sampler,
            shore: None,
        }
    }

    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// A pool-specific coast; FFT textures stay shared, while depth/fetch and wave uniforms belong to this pool.
    pub fn with_shore(device: &wgpu::Device, queue: &wgpu::Queue, field: &WaveField,
        map: std::sync::Arc<crate::shore::ShoreMap>) -> Self {
        Self::with_shore_shelter(device, queue, field, map, true)
    }

    pub fn with_shore_shelter(device: &wgpu::Device, queue: &wgpu::Queue, field: &WaveField,
        map: std::sync::Arc<crate::shore::ShoreMap>, shelter: bool) -> Self {
        let map = if shelter { map } else { std::sync::Arc::new(coast_depth_only(&map)) };
        let mut waves = Self::new(device, field);
        let textures = crate::shore::ShoreTextures::new(device, queue, &map);
        waves.groups = std::array::from_fn(|set| {
            let views = field.views_for(set);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blue Water coastal wave group"), layout: &waves.layout,
                entries: &[
                    wgpu::BindGroupEntry {binding:0,resource:wgpu::BindingResource::TextureView(&views[0])},
                    wgpu::BindGroupEntry {binding:1,resource:wgpu::BindingResource::TextureView(&views[1])},
                    wgpu::BindGroupEntry {binding:2,resource:wgpu::BindingResource::TextureView(&views[2])},
                    wgpu::BindGroupEntry {binding:3,resource:wgpu::BindingResource::Sampler(&waves._field_sampler)},
                    wgpu::BindGroupEntry {binding:4,resource:waves.uniforms.as_entire_binding()},
                    wgpu::BindGroupEntry {binding:5,resource:waves._hull_sea.as_entire_binding()},
                    wgpu::BindGroupEntry {binding:6,resource:wgpu::BindingResource::TextureView(&textures.heights)},
                    wgpu::BindGroupEntry {binding:7,resource:wgpu::BindingResource::TextureView(&textures.exposure)},
                    wgpu::BindGroupEntry {binding:8,resource:wgpu::BindingResource::Sampler(&waves._shore_sampler)},
                ],
            })
        });
        waves.shore = Some(map);
        waves
    }

    pub fn group(&self, field: &WaveField) -> &wgpu::BindGroup {
        &self.groups[field.current_index()]
    }

    pub fn write(&self, queue: &wgpu::Queue, field: &WaveField, time: f64, foam_coverage: f64, foam_lifetime: f64) {
        self.write_with_swell(queue,field,time,foam_coverage,foam_lifetime,[[0.0;4],[0.0,0.0,1.0,0.0]]);
    }

    pub fn write_with_swell(&self, queue: &wgpu::Queue, field: &WaveField, time: f64, foam_coverage:f64, foam_lifetime:f64, swell:[[f32;4];2]) {
        let mut uniforms = WaveUniforms::from_field(field, time, foam_coverage, foam_lifetime);
        if let Some(map) = &self.shore {
            uniforms.shore_heights = map.heights.place().frame([0.0;2]);
            uniforms.shore_exposure = map.exposure.place().frame([0.0;2]);
            uniforms.shore_fetch = field.surface_stats().shore_tables;
            for h in &mut uniforms.shore_waves[..3] {*h*=swell[1][2];}
            uniforms.shore_waves[3]=4.0*swell[0][3].abs()/std::f32::consts::SQRT_2;
            let k=if swell[0][3]>0.0 {f64::from(swell[0][2])} else {std::f64::consts::TAU/field.sea().peak_wavelength.max(10.0)};
            let frequency = (spectrum::GRAVITY*k).sqrt();
            uniforms.shore = [1.0, frequency as f32, (1000.0 / crate::shore::SWELL_SHELTER_M) as f32,
                (frequency * time).rem_euclid(std::f64::consts::TAU * 64.0) as f32];
        }
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
    }
}

/// Keep the original depth/breaking/surf model while bypassing land's wind and swell shelter.
pub fn coast_depth_only(map: &crate::shore::ShoreMap) -> crate::shore::ShoreMap {
    let mut map = map.clone();
    map.exposure.values.fill([crate::shore::OPEN_FETCH_M as f32; 2]);
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_sizes_match_original_surface_shader() {
        let mut composer = crate::shader_composer::composer().expect("Blue Water modules");
        let module = crate::shader_composer::surface_module(&mut composer, false)
            .expect("original Blue Water surface");
        for (binding, rust_bytes) in [(4, std::mem::size_of::<WaveUniforms>()), (5, 53 * 16)] {
            let (_, variable) = module
                .global_variables
                .iter()
                .find(|(_, variable)| variable.binding.as_ref().is_some_and(|at| at.group == 2 && at.binding == binding))
                .expect("upstream group 2 uniform");
            let wgpu::naga::TypeInner::Struct { span, .. } = &module.types[variable.ty].inner else {
                panic!("upstream wave binding is not a struct");
            };
            assert_eq!(*span as usize, rust_bytes, "group 2 binding {binding}");
        }
    }
}
