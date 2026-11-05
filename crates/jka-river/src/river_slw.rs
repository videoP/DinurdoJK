//! Single Layer Water GPU resources and scene-light adapter for the native
//! river port. No substitute water shading; see jka_water::river::slw.
use jka_water::river::slw;

/// The renderer's active sun expressed as the base pass's
/// `ResolvedView.DirectionalLightDirection` (toward the light, in the river's
/// Unreal/JKA Z-up axes) and `DirectionalLightColor` (illuminance / pi). The
/// river follows whichever sun is in use: the classic sky-shader/override
/// sun, or Blue Water's celestial light when that sky is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneSun {
    pub toward_light: [f32; 3],
    pub color: [f32; 3],
}

impl SceneSun {
    /// Classic world sun (`Renderer::active_sun`, renderer-space travel vector).
    /// bsp.wgsl adds `color * intensity / 255 * N.L` to the lighting term the
    /// albedo multiplies; a UE Lambert surface gets `albedo / pi * E * N.L`,
    /// so that radiance is exactly UE's DirectionalLightColor (E / pi).
    pub fn classic(travel: [f32; 3], color: [f32; 3], intensity: f32) -> Option<Self> {
        let toward = normalized(jka_render_core::coordinates::jka_position(travel.map(|v| -v)))?;
        Some(Self {
            toward_light: toward,
            color: color.map(|c| c * (intensity.max(0.0) / 255.0)),
        })
    }

    /// Blue Water `Sky::light()` (renderer-space vector toward the body). The
    /// sea shades with `color * intensity` as three.js-style irradiance E
    /// (Lambert `albedo / pi * E`), so DirectionalLightColor is E / pi.
    pub fn blue_water(toward: [f32; 3], color: [f32; 3], intensity: f32) -> Option<Self> {
        let toward = normalized(jka_render_core::coordinates::jka_position(toward))?;
        Some(Self {
            toward_light: toward,
            color: color.map(|c| c * intensity / std::f32::consts::PI),
        })
    }

    /// BasePassPixelShader: `SunIlluminance = DirectionalLightColor * PI`.
    pub fn illuminance(&self) -> [f32; 3] {
        self.color.map(|c| c * std::f32::consts::PI)
    }
}

/// Base-pass `DiffuseIndirectLighting` sources (illuminance = value * pi,
/// as BasePassPixelShader computes it).
///
/// Classic sky: the lightgrid as classic entity lighting reads it, with the
/// map's baked sun share removed and nothing added back (the active sun is the
/// separate SunIlluminance term). Entity shading lights albedo by
/// `(ambient + max(N.dir, 0) * directed) / 255`, the same scale as the world's
/// `runtime_sun_radiance`, so that sum is DiffuseIndirectLighting. Without a
/// runtime sun gain the result is static per map: it is sampled once per river
/// cell, never per frame. Blue Water sky: `env_irradiance(normal) / pi`, the
/// environment the Blue Water sea is lit with.
pub struct ClassicAmbientTexels {
    pub dimensions: [u32; 2],
    /// ambient / 255, octahedral direction x
    pub ambient: Vec<[f32; 4]>,
    /// directed / 255, octahedral direction y
    pub directed: Vec<[f32; 4]>,
}

/// `point(x, y)` is the JKA-space sample position of river cell (x, y);
/// `sample` is the classic lightgrid sampler (None: no lightgrid data there,
/// stored as zero indirect light).
pub fn classic_ambient_texels(
    dimensions: [u32; 2],
    point: impl Fn(u32, u32) -> [f32; 3],
    mut sample: impl FnMut([f32; 3]) -> Option<jka_render_core::light::ClassicEntityLight>,
    map_toward_sun: Option<[f32; 3]>,
) -> ClassicAmbientTexels {
    let count = (dimensions[0] * dimensions[1]) as usize;
    let mut out = ClassicAmbientTexels {
        dimensions,
        ambient: Vec::with_capacity(count),
        directed: Vec::with_capacity(count),
    };
    for y in 0..dimensions[1] {
        for x in 0..dimensions[0] {
            let Some(mut light) = sample(point(x, y)) else {
                out.ambient.push([0.0; 4]);
                out.directed.push([0.0; 4]);
                continue;
            };
            if let Some(toward) = map_toward_sun {
                // Gain zero: strip the baked sun without re-adding a sun here.
                light.relight_sun(toward, toward, [0.0; 3]);
            }
            let direction = normalized(jka_render_core::coordinates::jka_position(light.direction)).unwrap_or([0.0, 0.0, 1.0]);
            let oct = octahedral_encode(direction);
            let a = light.ambient.map(|v| v / 255.0);
            let d = light.directed.map(|v| v / 255.0);
            out.ambient.push([a[0], a[1], a[2], oct[0]]);
            out.directed.push([d[0], d[1], d[2], oct[1]]);
        }
    }
    out
}

pub fn octahedral_encode(n: [f32; 3]) -> [f32; 2] {
    let l1 = n[0].abs() + n[1].abs() + n[2].abs();
    let p = [n[0] / l1, n[1] / l1];
    let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
    if n[2] < 0.0 {
        [(1.0 - p[1].abs()) * sign(p[0]), (1.0 - p[0].abs()) * sign(p[1])]
    } else {
        p
    }
}

/// CPU form of `river_slw_classic_indirect` for tests and diagnostics.
pub fn classic_indirect(ambient: [f32; 4], directed: [f32; 4], normal: [f32; 3]) -> [f32; 3] {
    let dir = octahedral_decode([ambient[3], directed[3]]);
    let incoming = (normal[0] * dir[0] + normal[1] * dir[1] + normal[2] * dir[2]).max(0.0);
    std::array::from_fn(|k| ambient[k] + incoming * directed[k])
}

pub fn octahedral_decode(e: [f32; 2]) -> [f32; 3] {
    let mut n = [e[0], e[1], 1.0 - e[0].abs() - e[1].abs()];
    let t = (-n[2]).max(0.0);
    n[0] += if n[0] >= 0.0 { -t } else { t };
    n[1] += if n[1] >= 0.0 { -t } else { t };
    normalized(n).unwrap_or([0.0, 0.0, 1.0])
}

/// `fn river_slw_parameters()` for slw.wgsl, from the live-instance values.
pub fn parameters_shader(p: &slw::Parameters) -> String {
    let v = |x: [f32; 4]| format!("vec4<f32>({:?},{:?},{:?},{:?})", x[0], x[1], x[2], x[3]);
    format!(
        "fn river_slw_parameters()->RiverSlwParameters {{return RiverSlwParameters({},{},{},{},{},{},{},{});}}",
        v(p.scattering),
        v(p.absorption),
        v([p.water_albedo[0], p.water_albedo[1], p.water_albedo[2], p.caustics_max_intensity]),
        v([p.water_specular, p.water_fresnel_specular, p.water_roughness, p.water_fresnel_roughness]),
        v([p.water_fresnel_exponent, p.anisotropy, p.velocity_to_mask_waves, 0.0]),
        v([p.refraction, p.refraction_far, p.depth_for_min_refraction, p.refraction_bottom_amount]),
        v([p.critical_angle_dot, p.critical_angle_width, p.critical_angle_bend_width, p.lip_refraction]),
        v([p.refraction_depth_bias, 0.0, 0.0, 0.0]),
    )
}

/// WGSL adapter: indirect-light decode and the JKA -> renderer axis swap
/// (scene::render_position) for sampling Blue Water's renderer-space atlas.
pub const ADAPTER_SHADER: &str = r#"
fn river_slw_octahedral_decode(e:vec2<f32>)->vec3<f32> {
    var n=vec3<f32>(e,1.0-abs(e.x)-abs(e.y));
    let t=max(-n.z,0.0);
    n.x+=select(t,-t,n.x>=0.0);
    n.y+=select(t,-t,n.y>=0.0);
    return normalize(n);
}
// DiffuseIndirectLighting from the classic texels at a river-space normal.
fn river_slw_classic_indirect(ambient:vec4<f32>,directed:vec4<f32>,normal:vec3<f32>)->vec3<f32> {
    let dir=river_slw_octahedral_decode(vec2<f32>(ambient.w,directed.w));
    return ambient.rgb+max(dot(normal,dir),0.0)*directed.rgb;
}
// DiffuseIndirectLighting from Blue Water env_irradiance (irradiance / pi).
fn river_slw_sky_indirect(irradiance:vec3<f32>)->vec3<f32> {return irradiance/3.1415926535897932;}
fn river_slw_render_vector(v:vec3<f32>)->vec3<f32> {return vec3<f32>(v.x,v.z,-v.y);}
"#;

/// The classic indirect field on the GPU: two RGBA16F textures at the river
/// grid resolution (8 bytes/cell each), sampled with bilinear clamp. Built once.
pub struct ClassicAmbient {
    pub ambient: wgpu::Texture,
    pub directed: wgpu::Texture,
}
impl ClassicAmbient {
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, texels: &ClassicAmbientTexels) -> Result<Self, String> {
        use wgpu::util::DeviceExt;
        let [w, h] = texels.dimensions;
        let count = (w as usize) * (h as usize);
        if w == 0
            || h == 0
            || texels.ambient.len() != count
            || texels.directed.len() != count
            || w.max(h) > device.limits().max_texture_dimension_2d
            || texels.ambient.iter().chain(&texels.directed).flatten().any(|v| !v.is_finite())
        {
            return Err("invalid river ambient field".into());
        }
        let make = |label, values: &[[f32; 4]]| {
            let half: Vec<u16> = values.iter().flatten().map(|v| f32_to_f16(*v)).collect();
            device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
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
                bytemuck::cast_slice(&half),
            )
        };
        Ok(Self {
            ambient: make("river classic indirect ambient", &texels.ambient),
            directed: make("river classic indirect directed", &texels.directed),
        })
    }
    pub fn logical_bytes(&self) -> u64 {
        2 * u64::from(self.ambient.width()) * u64::from(self.ambient.height()) * 8
    }
}

/// IEEE binary16, round to nearest even, with subnormals, infinities and NaN.
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;
    if exponent == 0xff {
        return sign | 0x7c00 | if mantissa != 0 { 0x200 } else { 0 };
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7c00;
    }
    if unbiased >= -14 {
        // Normal half: 10 mantissa bits, round the dropped 13 to nearest even.
        let mut half = (((unbiased + 15) as u32) << 10) | (mantissa >> 13);
        let rest = mantissa & 0x1fff;
        if rest > 0x1000 || (rest == 0x1000 && half & 1 == 1) {
            half += 1; // may carry into the exponent, including to infinity
        }
        return sign | half as u16;
    }
    if unbiased < -25 {
        return sign;
    }
    // Subnormal half: shift the implicit-one significand into place.
    let significand = mantissa | 0x80_0000;
    let shift = (-14 - unbiased + 13) as u32;
    let mut half = significand >> shift;
    let rest = significand & ((1 << shift) - 1);
    let halfway = 1 << (shift - 1);
    if rest > halfway || (rest == halfway && half & 1 == 1) {
        half += 1;
    }
    sign | half as u16
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (l.is_finite() && l > 1e-12).then(|| v.map(|c| c / l))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_sun_matches_world_radiance_and_points_to_the_light() {
        // Sun overhead in JKA (+Z); light travels down.
        let travel = jka_render_core::coordinates::render_position([0.0, 0.0, -1.0]);
        let sun = SceneSun::classic(travel, [1.0, 0.5, 0.25], 255.0).unwrap();
        assert!((sun.toward_light[2] - 1.0).abs() < 1e-6);
        assert_eq!(sun.color, [1.0, 0.5, 0.25]);
        let e = sun.illuminance();
        assert!((e[0] - std::f32::consts::PI).abs() < 1e-6);
        assert!(SceneSun::classic([0.0; 3], [1.0; 3], 100.0).is_none());
    }

    #[test]
    fn f16_conversion_rounds_to_nearest_even_with_subnormals() {
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(-2.0), 0xc000);
        assert_eq!(f32_to_f16(65504.0), 0x7bff);
        assert_eq!(f32_to_f16(65520.0), 0x7c00); // rounds up to infinity
        assert_eq!(f32_to_f16(1.0 + 1.0 / 2048.0), 0x3c00); // tie to even
        assert_eq!(f32_to_f16(1.0 + 3.0 / 2048.0), 0x3c02); // tie to even (up)
        assert_eq!(f32_to_f16(5.960_464_5e-8), 0x0001); // smallest subnormal
        assert_eq!(f32_to_f16(2.0f32.powi(-26)), 0x0000); // tie to even (zero)
        assert_eq!(f32_to_f16(6.097_555e-5), 0x03ff); // largest subnormal
        assert_eq!(f32_to_f16(f32::NAN) & 0x7c00, 0x7c00);
    }

    #[test]
    fn octahedral_directions_round_trip() {
        for n in [[0.0, 0.0, 1.0], [0.6, -0.8, 0.0], [0.3, 0.4, -0.866], [-0.2, -0.1, 0.97]] {
            let n = normalized(n).unwrap();
            let back = octahedral_decode(octahedral_encode(n));
            for k in 0..3 {
                assert!((back[k] - n[k]).abs() < 1e-5, "{n:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn classic_indirect_strips_baked_sun_and_uses_river_axes() {
        // Light from JKA +X (renderer +X), all directed light is baked sun.
        let toward_render = jka_render_core::coordinates::render_position([1.0, 0.0, 0.0]);
        let light = jka_render_core::light::ClassicEntityLight {
            ambient: [51.0, 25.5, 0.0],
            directed: [102.0, 102.0, 102.0],
            direction: toward_render,
            baked_sun: [102.0, 102.0, 102.0],
            ..Default::default()
        };
        let texels = classic_ambient_texels([2, 1], |x, _| [x as f32, 0.0, 0.0], |_| Some(light), Some(toward_render));
        assert_eq!(&texels.ambient[0][..3], &[0.2, 0.1, 0.0]);
        assert_eq!(&texels.directed[0][..3], &[0.0, 0.0, 0.0]);
        // Without a known map sun the directed light stays and faces JKA +X.
        let kept = classic_ambient_texels([1, 1], |_, _| [0.0; 3], |_| Some(light), None);
        let lit = classic_indirect(kept.ambient[0], kept.directed[0], [1.0, 0.0, 0.0]);
        assert!((lit[0] - 0.6).abs() < 1e-6 && (lit[2] - 0.4).abs() < 1e-6);
        let unlit = classic_indirect(kept.ambient[0], kept.directed[0], [-1.0, 0.0, 0.0]);
        assert!((unlit[0] - 0.2).abs() < 1e-6);
        let missing = classic_ambient_texels([1, 1], |_, _| [0.0; 3], |_| None, None);
        assert_eq!(missing.ambient[0], [0.0; 4]);
    }

    #[test]
    fn blue_water_light_is_irradiance_in_its_own_units() {
        let toward = jka_render_core::coordinates::render_position([1.0, 0.0, 1.0]);
        let sun = SceneSun::blue_water(toward, [1.0, 0.9, 0.8], 6.0).unwrap();
        assert!((sun.illuminance()[0] - 6.0).abs() < 1e-5);
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!((sun.toward_light[0] - expected).abs() < 1e-6 && (sun.toward_light[2] - expected).abs() < 1e-6);
    }
}

/// GSystemTextures::PreintegratedGF, generated on the CPU exactly as the engine
/// does. Native storage is PF_G16R16 unorm; the identical k/65535 values are
/// stored in RG32F (FLOAT32_FILTERABLE is already required by the river), so
/// no 16-bit-norm device feature is needed. Shared once per renderer.
pub struct PreintegratedGf {
    pub texture: wgpu::Texture,
    /// BasePassRendering.cpp: SF_Bilinear, AM_Clamp on all axes.
    pub sampler: wgpu::Sampler,
}

impl PreintegratedGf {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let [w, h] = slw::GF_SIZE;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native PreintegratedGF"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let values: Vec<[f32; 2]> = slw::preintegrated_gf()
            .into_iter()
            .map(|t| t.map(|v| f32::from(v) / 65535.0))
            .collect();
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            texture.size(),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native PreIntegratedGFSampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self { texture, sampler }
    }
}
