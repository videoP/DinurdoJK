//! Conservative, load-time promotion of authored foliage cards to wind sway.
use jka_assets::shader::Shader;

/// Covers the maximum shader displacement on every axis (in renderer units).
pub(crate) const BOUNDS_PADDING: f32 = 8.0;
pub(crate) const MATERIAL_FLAG: u32 = 1 << 18;

pub(crate) const SHADER: &str = include_str!("foliage_wind.wgsl");

/// World shader composition also includes the existing snow/footprint module.
/// Models consume SHADER directly without binding any world-only resources.
pub(crate) fn surface_shader() -> String {
    format!("{SHADER}\n{}", include_str!("surface_deformation.wgsl"))
}

/// All scene cameras share the existing grass wind texture. Vertex-stage
/// consumers can sample it without extending each material's bind group.
pub(crate) fn camera_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

pub(crate) fn camera_entries<'a>(
    buffer: &'a wgpu::Buffer,
    noise: &'a wgpu::TextureView,
    sampler: &'a wgpu::Sampler,
) -> [wgpu::BindGroupEntry<'a>; 3] {
    [
        wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(noise),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: wgpu::BindingResource::Sampler(sampler),
        },
    ]
}

/// Zero means ineligible; [1, 2) carries a stable per-model variation seed.
/// The same origin produces the same tag for CPU meshes, GPU instances and
/// every authored stage. Cached geometry does not change with time or wind.
pub(crate) fn model_tag(origin: [f32; 3]) -> f32 {
    let mut hash = origin.into_iter().fold(0x811c9dc5u32, |hash, value| {
        (hash ^ value.to_bits()).wrapping_mul(0x01000193)
    });
    hash = (hash ^ (hash >> 16)).wrapping_mul(0x7feb352d);
    hash = (hash ^ (hash >> 15)).wrapping_mul(0x846ca68b);
    hash ^= hash >> 16;
    1.0 + ((hash >> 9) as f32) / 8_388_608.0
}

#[cfg(test)]
pub(crate) fn test_noise_resources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::TextureView, wgpu::Sampler) {
    use wgpu::util::DeviceExt;
    let pixels = crate::grass::godot_wind_noise();
    let size = pixels.len().isqrt() as u32;
    assert_eq!(pixels.len(), (size * size) as usize);
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("canonical grass wind noise for foliage regression"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        pixels.as_slice(),
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (texture.create_view(&Default::default()), sampler)
}

fn foliage_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    // Reject woody basenames even inside a foliage directory. Positive hints
    // may occur anywhere in the shader path. Prefixes accept tree09a,
    // plants01 and grass_patch, without accidentally matching "street".
    let words: Vec<_> = stem
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .collect();
    if words.iter().any(|word| {
        ["bark", "trunk", "wood", "branch", "log"]
            .iter()
            .any(|prefix| word.starts_with(prefix))
    }) {
        return false;
    }
    lower.split(|c: char| !c.is_ascii_alphabetic()).any(|word| {
        [
            "tree", "leaf", "leaves", "foliage", "fern", "palm", "bush", "shrub", "plant", "grass",
            "frond", "ivy", "vine", "canopy",
        ]
        .iter()
        .any(|prefix| word.starts_with(prefix))
    })
}

pub(crate) fn promotes_shader(name: &str, shader: &Shader) -> bool {
    // Alpha-tested two-sided geometry is a useful structural hint, but flags
    // alone also describe fences, grilles and cloth. Require a foliage name.
    // Keep authored wave/bulge animation rather than stacking guessed motion.
    foliage_name(name)
        && (shader.cull.eq_ignore_ascii_case("twosided")
            || shader.cull.eq_ignore_ascii_case("none")
            || shader.cull.eq_ignore_ascii_case("disable")
            || shader.auto_sprite2)
        && !shader.sky
        && !shader.water
        && !shader.portal
        && !shader.polygon_offset
        && shader.bulge.is_none()
        && shader.vertex_wave.is_none()
        && shader.stages.iter().any(|stage| {
            stage.surface_sprite.is_none()
                && matches!(
                    stage.alpha_test.as_str(),
                    "ge128" | "ge192" | "gt0" | "lt128"
                )
                && (stage.blend.is_empty() || stage.blend.eq_ignore_ascii_case("GL_ONE GL_ZERO"))
        })
}

/// The same gust and direction waves as GodotGrass, evaluated once per view.
/// xy = current velocity in Godot metres/s with grass's 20% inheritance;
/// zw = base-wind noise transport in Godot metres (exactly the grass path).
pub(crate) fn wind_uniform(
    wind: crate::ocean::AtmosphericWind,
    time: f32,
    enabled: bool,
) -> [f32; 4] {
    if !enabled {
        return [0.0; 4];
    }
    let wind = wind.sanitize();
    let t = f64::from(time);
    let shift = (t * 0.13).sin() * 0.7 + (t * 0.047 + 2.4).sin() * 0.3;
    let gust = (t * 0.83).sin() * 0.65 + (t * 1.71 + 1.9).sin() * 0.35;
    let angle = f64::from(wind.direction.to_radians()) + f64::from(wind.shift.to_radians()) * shift;
    let speed = f64::from(wind.speed) * (1.0 + f64::from(wind.gust) * gust).max(0.0);
    let inherited_speed = speed / 64.0 * 0.20;
    let base_angle = f64::from(wind.direction).to_radians();
    let travel = f64::from(wind.speed) / 64.0 * t;
    // All noise scales (.005/.025/.075) repeat after 200 metres. Wrap in f64
    // before uploading to preserve noise detail in long sessions.
    [
        (angle.cos() * inherited_speed) as f32,
        (angle.sin() * inherited_speed) as f32,
        ((base_angle.cos() * travel).rem_euclid(200.0) as f32).rem_euclid(200.0),
        ((base_angle.sin() * travel).rem_euclid(200.0) as f32).rem_euclid(200.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shader(name: &str, directives: &str) -> Shader {
        jka_assets::shader::parse(&format!(
            "{name}\n{{\n{directives}\n{{\nmap {name}\nalphaFunc GE128\nrgbGen vertex\n}}\n}}\n"
        ))
        .unwrap()
        .remove(name)
        .unwrap()
    }

    #[test]
    fn foliage_promotion_requires_both_name_and_card_semantics() {
        for name in [
            "models/map_objects/yavin/tree09a",
            "textures/Leaves",
            "textures/plant01",
            "textures/grass_patch",
            "textures/fern",
            "textures/shrub02",
            "textures/leaves/green01",
        ] {
            assert!(
                promotes_shader(name, &shader(name, "cull twosided")),
                "{name}"
            );
            assert!(!promotes_shader(name, &shader(name, "cull back")), "{name}");
        }
        for name in [
            "textures/tree_bark",
            "textures/tree_trunk",
            "textures/tree_wood",
            "textures/tree_branch",
            "textures/trees/bark",
            "textures/street",
            "textures/fence",
        ] {
            assert!(
                !promotes_shader(name, &shader(name, "cull twosided")),
                "{name}"
            );
        }
        let name = "textures/tree";
        for directives in [
            "cull twosided\nsurfaceparm water",
            "cull twosided\ndeformVertexes wave 100 sin 0 2 0 1",
        ] {
            assert!(!promotes_shader(name, &shader(name, directives)));
        }
        let mut opaque = shader(name, "cull twosided");
        opaque.stages[0].alpha_test.clear();
        assert!(!promotes_shader(name, &opaque));
        assert!(promotes_shader(
            name,
            &shader(name, "deformVertexes autoSprite2")
        ));
    }

    #[test]
    fn foliage_wind_matches_grass_speed_and_noise_transport() {
        let wind = crate::ocean::AtmosphericWind {
            speed: 180.0,
            direction: 0.0,
            gust: 0.0,
            shift: 0.0,
        };
        assert_eq!(wind_uniform(wind, 10.0, true), [0.5625, 0.0, 28.125, 0.0]);
        assert_eq!(
            wind_uniform(
                crate::ocean::AtmosphericWind {
                    speed: 360.0,
                    ..wind
                },
                10.0,
                true
            ),
            [1.125, 0.0, 56.25, 0.0]
        );
        assert_eq!(wind_uniform(wind, 10.0, false), [0.0; 4]);
        for time in [0.0, 1.25, 600.0, 86400.0] {
            assert_eq!(
                wind_uniform(
                    crate::ocean::AtmosphericWind { speed: 0.0, ..wind },
                    time,
                    true
                ),
                [0.0; 4]
            );
            let storm = wind_uniform(
                crate::ocean::AtmosphericWind {
                    speed: 10000.0,
                    gust: 1.0,
                    shift: 90.0,
                    ..wind
                },
                time,
                true,
            );
            assert!(storm.iter().all(|value| value.is_finite()));
            assert!(storm[0].hypot(storm[1]) <= 8192.0 / 64.0 * 0.20 * 2.0);
            assert!(storm[2..]
                .iter()
                .all(|offset| (0.0..200.0).contains(offset)));
        }
    }

    #[test]
    fn foliage_model_variation_is_stable_and_differs_between_neighbors() {
        let origin = [-1267.0, -5964.0, 406.0];
        let tag = model_tag(origin);
        assert_eq!(model_tag(origin), tag);
        assert!((1.0..2.0).contains(&tag));
        for neighbor in [[-1235.0, -5964.0, 406.0], [-1267.0, -5932.0, 406.0]] {
            assert_ne!(model_tag(neighbor), tag);
            assert!((1.0..2.0).contains(&model_tag(neighbor)));
        }
    }
}
