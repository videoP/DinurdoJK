//! Environment clouds.
use crate::renderer::{
    cloud_noise, CloudForegroundBlade, Mat4, Renderer, Vec3, MAX_CLOUD_FOREGROUND_BLADES,
};
use wgpu::util::DeviceExt;

pub(in crate::renderer) struct CloudNoiseResources {
    pub(in crate::renderer) _detail_texture: wgpu::Texture,
    /// Tileable Worley volume used to erode cloud silhouette edges.
    pub(in crate::renderer) detail_view: wgpu::TextureView,
    pub(in crate::renderer) _weather_texture: wgpu::Texture,
    /// Tileable weather map: base Perlin-Worley in `r`, remap contrast floor in
    /// `g`, per-region cloud height in `b`.
    pub(in crate::renderer) weather_view: wgpu::TextureView,
    pub(in crate::renderer) sampler: wgpu::Sampler,
}

impl Renderer {
    pub(in crate::renderer) fn ensure_cloud_noise_resources(&mut self) -> bool {
        if self.cloud_noise_resources.is_some() {
            return false;
        }
        rverbose!(
            1,
            "Volumetric clouds: baking {}^2 Perlin-Worley weather map + {}^3 Worley detail volume",
            cloud_noise::WEATHER_SIZE,
            cloud_noise::DETAIL_SIZE
        );
        self.cloud_noise_resources = Some(create_cloud_noise_resources(&self.device, &self.queue));
        true
    }

    pub(in crate::renderer) fn set_cloud_foreground(&mut self, blades: &[CloudForegroundBlade]) {
        if self.cloud_foreground != blades {
            self.cloud_foreground.clear();
            self.cloud_foreground.extend_from_slice(blades);
        }
    }

    /// Screen-space capsules for the blades in `cloud_foreground`, packed for
    /// the post uniform: two vec4 per blade, `[ax, ay, bx, by]` in pixels then
    /// `[glow radius px, nearest distance, 0, 0]`. Blades behind the camera or
    /// off the frame are dropped, and the nearest `MAX_CLOUD_FOREGROUND_BLADES`
    /// survive. Returns the packed array and its blade count.
    pub(in crate::renderer) fn cloud_foreground_uniform(
        &self,
        view_proj: Mat4,
        camera_position: Vec3,
    ) -> ([[f32; 4]; MAX_CLOUD_FOREGROUND_BLADES * 2], f32) {
        let mut packed = [[0.0f32; 4]; MAX_CLOUD_FOREGROUND_BLADES * 2];
        if !self.clouds_enabled || self.cloud_foreground.is_empty() {
            return (packed, 0.0);
        }
        let width = self.config.width.max(1) as f32;
        let height = self.config.height.max(1) as f32;
        // Rows of a perspective view-projection scale view space by the focal
        // length, so the y row's length is the vertical focal factor.
        let focal_y = view_proj.row(1).truncate().length();
        // Clip w is the view-space depth. Blade points nearer than this are
        // pulled back onto the plane instead of dropping the whole blade.
        const NEAR_W: f32 = 1.0;
        let mut blades: Vec<(f32, [f32; 4], [f32; 4])> =
            Vec::with_capacity(MAX_CLOUD_FOREGROUND_BLADES);
        for blade in self.cloud_foreground.iter() {
            let start = Vec3::from(blade.start);
            let end = Vec3::from(blade.end);
            let mut clip_start = view_proj * start.extend(1.0);
            let mut clip_end = view_proj * end.extend(1.0);
            if clip_start.w < NEAR_W && clip_end.w < NEAR_W {
                continue;
            }
            if clip_start.w < NEAR_W {
                clip_start = clip_start.lerp(
                    clip_end,
                    (NEAR_W - clip_start.w) / (clip_end.w - clip_start.w),
                );
            } else if clip_end.w < NEAR_W {
                clip_end = clip_end.lerp(
                    clip_start,
                    (NEAR_W - clip_end.w) / (clip_start.w - clip_end.w),
                );
            }
            let pixel = |clip: glam::Vec4| {
                let ndc = clip.truncate().truncate() / clip.w;
                [(ndc.x * 0.5 + 0.5) * width, (0.5 - ndc.y * 0.5) * height]
            };
            let a = pixel(clip_start);
            let b = pixel(clip_end);
            // Glow half-width: the shaders draw the blade line at 1.8x the
            // radius and the halo fades out beyond that.
            let mid_w = ((clip_start.w + clip_end.w) * 0.5).max(NEAR_W);
            let radius_px = blade.radius * 2.0 * focal_y * height * 0.5 / mid_w;
            let margin = radius_px + 4.0;
            let (min_x, max_x) = (a[0].min(b[0]) - margin, a[0].max(b[0]) + margin);
            let (min_y, max_y) = (a[1].min(b[1]) - margin, a[1].max(b[1]) + margin);
            if max_x < 0.0 || min_x > width || max_y < 0.0 || min_y > height {
                continue;
            }
            let nearest = (start - camera_position)
                .length()
                .min((end - camera_position).length());
            blades.push((
                nearest,
                [a[0], a[1], b[0], b[1]],
                [radius_px, nearest, 0.0, 0.0],
            ));
        }
        blades.sort_by(|left, right| left.0.total_cmp(&right.0));
        blades.truncate(MAX_CLOUD_FOREGROUND_BLADES);
        for (index, (_, segment, extent)) in blades.iter().enumerate() {
            packed[index * 2] = *segment;
            packed[index * 2 + 1] = *extent;
        }
        (packed, blades.len() as f32)
    }
}

pub(in crate::renderer) fn create_cloud_noise_sampler(
    device: &wgpu::Device,
    label: &str,
) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    })
}

pub(in crate::renderer) fn create_cloud_fallback_resources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> CloudNoiseResources {
    let detail_texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("JKA lazy cloud detail fallback 3D texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &[0, 0],
    );
    let detail_view = detail_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA lazy cloud detail fallback 3D view"),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    });

    let weather_texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("JKA lazy cloud weather fallback texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &[0, 0, 0, 0, 0, 0, 0, 0],
    );
    let weather_view = weather_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA lazy cloud weather fallback view"),
        ..Default::default()
    });

    CloudNoiseResources {
        _detail_texture: detail_texture,
        detail_view,
        _weather_texture: weather_texture,
        weather_view,
        sampler: create_cloud_noise_sampler(device, "JKA lazy cloud fallback sampler"),
    }
}

pub(in crate::renderer) fn create_cloud_noise_resources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> CloudNoiseResources {
    let weather_data = cloud_noise::bake_weather_map();
    let weather_size = cloud_noise::WEATHER_SIZE;
    let weather_texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("JKA cloud Perlin-Worley weather map"),
            size: wgpu::Extent3d {
                width: weather_size,
                height: weather_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // The remap contrast channel is negative across its whole range, so
            // this cannot be a unorm format.
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &weather_data,
    );
    let weather_view = weather_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA cloud weather map view"),
        ..Default::default()
    });

    let detail_data = cloud_noise::bake_detail_volume();
    let detail_size = cloud_noise::DETAIL_SIZE;
    let detail_texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("JKA cloud Worley detail volume"),
            size: wgpu::Extent3d {
                width: detail_size,
                height: detail_size,
                depth_or_array_layers: detail_size,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &detail_data,
    );
    let detail_view = detail_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA cloud detail volume view"),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    });

    CloudNoiseResources {
        _detail_texture: detail_texture,
        detail_view,
        _weather_texture: weather_texture,
        weather_view,
        sampler: create_cloud_noise_sampler(device, "JKA cloud repeating noise sampler"),
    }
}
