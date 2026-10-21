use super::*;

enum TestBinding {
    Buffer(wgpu::Buffer),
    Texture(wgpu::TextureView),
    Sampler(wgpu::Sampler),
}

/// Bind all resources reachable from the actual entry points. General world
/// shaders retain their complete layout even when overrides disable effects.
fn test_groups(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    source: &str,
    entries: [&str; 2],
    camera: &wgpu::Buffer,
    material: &wgpu::Buffer,
    base: &wgpu::TextureView,
    lightmap: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> Vec<(u32, wgpu::BindGroup)> {
    use wgpu::naga::{self, AddressSpace, ImageClass, ImageDimension, TypeInner};
    use wgpu::util::DeviceExt;
    let module = naga::front::wgsl::parse_str(source).unwrap();
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    let indices = entries.map(|name| {
        module
            .entry_points
            .iter()
            .position(|e| e.name == name)
            .unwrap()
    });
    let mut groups = std::collections::BTreeMap::<u32, Vec<(u32, TestBinding)>>::new();
    for (handle, global) in module.global_variables.iter() {
        let Some(binding) = global.binding else {
            continue;
        };
        if indices
            .iter()
            .all(|&i| info.get_entry_point(i)[handle].is_empty())
        {
            continue;
        }
        let resource = match global.name.as_deref() {
            Some("camera") => TestBinding::Buffer(camera.clone()),
            Some("material") => TestBinding::Buffer(material.clone()),
            Some("pbr_settings") => TestBinding::Buffer(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::bytes_of(&crate::renderer::PbrSettings {
                        values: [1, 1, 30, 0],
                        deluxe: [1.0, 0.0, 0.0, 1000.0],
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            )),
            Some("legacy_fog") => TestBinding::Buffer(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&[1.0_f32, 0.4, 0.0, 2.0]),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            )),
            Some("base_texture") => TestBinding::Texture(base.clone()),
            Some("lightmap_texture") => TestBinding::Texture(lightmap.clone()),
            _ => match global.space {
                AddressSpace::Uniform | AddressSpace::Storage { .. } => {
                    TestBinding::Buffer(device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("disabled world test resource"),
                        size: 65536,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    }))
                }
                _ => match module.types[global.ty].inner {
                    TypeInner::Sampler { comparison } => TestBinding::Sampler(if comparison {
                        device.create_sampler(&wgpu::SamplerDescriptor {
                            compare: Some(wgpu::CompareFunction::LessEqual),
                            ..Default::default()
                        })
                    } else {
                        sampler.clone()
                    }),
                    TypeInner::Image {
                        dim,
                        arrayed,
                        class,
                    } => {
                        let layers = if dim == ImageDimension::Cube { 6 } else { 1 };
                        let (dimension, view_dimension) = match (dim, arrayed) {
                            (ImageDimension::D3, _) => {
                                (wgpu::TextureDimension::D3, wgpu::TextureViewDimension::D3)
                            }
                            (ImageDimension::Cube, false) => {
                                (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::Cube)
                            }
                            (ImageDimension::Cube, true) => (
                                wgpu::TextureDimension::D2,
                                wgpu::TextureViewDimension::CubeArray,
                            ),
                            (ImageDimension::D2, true) => (
                                wgpu::TextureDimension::D2,
                                wgpu::TextureViewDimension::D2Array,
                            ),
                            (ImageDimension::D2, false) => {
                                (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::D2)
                            }
                            _ => panic!("unexpected world test image dimension"),
                        };
                        let format = match class {
                            ImageClass::Depth { .. } => wgpu::TextureFormat::Depth32Float,
                            ImageClass::Sampled {
                                kind: naga::ScalarKind::Uint,
                                ..
                            } => wgpu::TextureFormat::R32Uint,
                            ImageClass::Sampled { .. } => wgpu::TextureFormat::Rgba8Unorm,
                            _ => panic!("unexpected world test image class"),
                        };
                        let texture = device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("disabled world test image"),
                            size: wgpu::Extent3d {
                                width: 1,
                                height: 1,
                                depth_or_array_layers: layers,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension,
                            format,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        });
                        TestBinding::Texture(texture.create_view(&wgpu::TextureViewDescriptor {
                            dimension: Some(view_dimension),
                            ..Default::default()
                        }))
                    }
                    _ => panic!("unexpected world test binding type"),
                },
            },
        };
        groups
            .entry(binding.group)
            .or_default()
            .push((binding.binding, resource));
    }
    groups
        .into_iter()
        .map(|(index, resources)| {
            let entries = resources
                .iter()
                .map(|(binding, resource)| wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: match resource {
                        TestBinding::Buffer(buffer) => buffer.as_entire_binding(),
                        TestBinding::Texture(view) => wgpu::BindingResource::TextureView(view),
                        TestBinding::Sampler(sampler) => wgpu::BindingResource::Sampler(sampler),
                    },
                })
                .collect::<Vec<_>>();
            (
                index,
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("world test complete group"),
                    layout: &pipeline.get_bind_group_layout(index),
                    entries: &entries,
                }),
            )
        })
        .collect()
}

#[test]
fn vertex_recipe_tracks_operations_not_values() {
    let mut a = MaterialUniform::default();
    a.header = [0, 2, 134217728, 14 << 8];
    a.mods[0] = [1.0, 1.0, 2.0, 0.0];
    a.mods[2] = [6.0, 1.0, 0.3, 0.0];
    a.mods[3] = [2.0, 3.0, 0.0, 0.0];
    a.wave_funcs[0] = 2;
    let mut b = a;
    b.mods[0][1] = 90.0;
    b.mods[2][2] = 0.9;
    b.wave_rgb[3] = 8.0;
    assert_eq!(
        VertexRecipe::from_uniform(&a),
        VertexRecipe::from_uniform(&b)
    );
    b.mods[0][0] = 3.0;
    assert_ne!(
        VertexRecipe::from_uniform(&a),
        VertexRecipe::from_uniform(&b)
    );
    b = a;
    b.mods[3][1] = 4.0;
    assert_ne!(
        VertexRecipe::from_uniform(&a),
        VertexRecipe::from_uniform(&b)
    );
}

#[test]
fn medium_plain_stages_drop_only_proven_unused_features() {
    let medium = WorldShaderVariantKey {
        pbr: true,
        pom: true,
        deluxe: true,
        detail_textures_enabled: true,
        planar_reflections: true,
        surface_deformation: true,
        legacy_fog: true,
        static_bsp_ao: true,
        ..Default::default()
    };
    let wall = MaterialFragmentKey::from_material([0, 0, 2, 0], 0.0);
    let v = effective_stage_variant(medium, wall);
    assert!(basic(v));
    assert!(v.legacy_fog && v.static_bsp_ao);
    let mut lit = wall;
    lit.flags |= 8;
    lit.flags_w |= 2;
    let v = effective_stage_variant(medium, lit);
    assert!(v.pbr && v.pom && v.deluxe && v.detail_textures_enabled);
    let mut glass = wall;
    glass.tc_gen = 3;
    assert!(effective_stage_variant(medium, glass).planar_reflections);
    let mut snow = wall;
    snow.vertex_recipe.surface_material = 14;
    assert!(effective_stage_variant(medium, snow).surface_deformation);
}

#[test]
fn shared_families_ignore_material_options_and_generated_keys_retain_operations() {
    let pipeline = super::super::pipelines::ocean_pipeline_key();
    let v = WorldShaderVariantKey {
        material_specialization: true,
        program_mode: WorldProgramMode::Shared,
        ..Default::default()
    };
    let a = MaterialFragmentKey::from_material([0, 0, 2, 0], 0.0);
    let b = MaterialFragmentKey::from_material([3, 0, 1 | 134217728, 0], 0.0);
    assert_eq!(program_key(v, pipeline, a), program_key(v, pipeline, b));
    let v = WorldShaderVariantKey {
        program_mode: WorldProgramMode::Generated,
        ..v
    };
    assert_ne!(program_key(v, pipeline, a), program_key(v, pipeline, b));
    let unsupported = WorldShaderVariantKey {
        ray_traced_shadows: true,
        ..v
    };
    assert_eq!(
        program_key(unsupported, pipeline, a).program,
        StageProgram::Specialized
    );
}

#[test]
fn glass_primary_stages_keep_the_specialized_fs_main_program() {
    // Only fs_main shades promoted glass; minimal/shared/generated entries
    // would draw it as an opaque stage.
    let pipeline = super::super::pipelines::ocean_pipeline_key();
    let glass = MaterialFragmentKey::from_material([0, 0, 2, 1 << 17], 0.0);
    for mode in [WorldProgramMode::Shared, WorldProgramMode::Generated] {
        let v = WorldShaderVariantKey {
            material_specialization: true,
            program_mode: mode,
            ..Default::default()
        };
        let key = program_key(v, pipeline, glass);
        assert_eq!(key.program, StageProgram::Specialized);
        assert!(!uses_minimal_entry(key));
        // The glass bit only selects the program; it never splits PSOs.
        assert_eq!(key.flags_w & (1 << 17), 0);
    }
}

#[test]
fn generated_stage_matrix_validates_and_specializes() {
    use wgpu::naga::{self, ShaderStage};
    let minimal = WorldShaderVariantKey {
        material_specialization: true,
        program_mode: WorldProgramMode::Generated,
        ..Default::default()
    };
    let low = WorldShaderVariantKey {
        legacy_dlights: true,
        legacy_fog: true,
        ..minimal
    };
    let medium = WorldShaderVariantKey {
        pbr: true,
        pom: true,
        deluxe: true,
        pbr_vertex_lightgrid: true,
        detail_textures_enabled: true,
        planar_reflections: true,
        surface_deformation: true,
        ..low
    };
    let baked = WorldShaderVariantKey {
        baked_sun_relight: true,
        ..low
    };
    let mut pipeline = super::super::pipelines::ocean_pipeline_key();
    for variant in [minimal, low, medium, baked] {
        for flags in [
            0,
            2,
            1 | 1048576,
            134217728 | 268435456,
            2 | 8 | 16 | 32 | 128,
            4096,
            8388608,
            131072 | 1,
        ] {
            for tc_gen in 0..4 {
                let mut uniform = MaterialUniform::default();
                uniform.header = [tc_gen, 4, flags, 2 | (14 << 8)];
                uniform.params[0] = -0.5;
                uniform.mods[0][0] = 1.0;
                uniform.mods[2][0] = 3.0;
                uniform.mods[4][0] = 5.0;
                uniform.mods[6][0] = 6.0;
                uniform.mods[7][1] = 5.0;
                uniform.wave_funcs = [5, 2, 0, 0];
                for class in [
                    crate::renderer::DrawClass::Opaque,
                    crate::renderer::DrawClass::Mask,
                ] {
                    pipeline.class = class;
                    let key = program_key(
                        variant,
                        pipeline,
                        MaterialFragmentKey::from_uniform(&uniform),
                    );
                    let effective = program_variant(variant, key);
                    let source = generated_source(effective, key);
                    let module = naga::front::wgsl::parse_str(&source)
                        .unwrap_or_else(|e| panic!("{key:?}: {}", e.emit_to_string(&source)));
                    let info = naga::valid::Validator::new(
                        naga::valid::ValidationFlags::all(),
                        naga::valid::Capabilities::all(),
                    )
                    .validate(&module)
                    .unwrap_or_else(|e| panic!("{key:?}: {e:?}"));
                    let constants = effective
                        .compilation_constants()
                        .into_iter()
                        .chain(key.compilation_constants())
                        .chain([(
                            "ENABLE_ALPHA_TEST",
                            f64::from(u8::from(class != crate::renderer::DrawClass::Opaque)),
                        )])
                        .map(|(name, value)| (name.to_owned(), value))
                        .collect();
                    for (stage, entry) in [
                        (ShaderStage::Vertex, "vs_generated"),
                        (ShaderStage::Fragment, "fs_generated"),
                    ] {
                        naga::back::pipeline_constants::process_overrides(
                            &module,
                            &info,
                            Some((stage, entry)),
                            &constants,
                        )
                        .unwrap_or_else(|e| panic!("{key:?} {entry}: {e:?}"));
                    }
                }
            }
        }
    }
}

#[test]
fn generated_plain_interface_omits_unused_varyings_and_tcmod_interpreter() {
    let variant = WorldShaderVariantKey {
        material_specialization: true,
        program_mode: WorldProgramMode::Generated,
        ..Default::default()
    };
    let key = program_key(
        variant,
        super::super::pipelines::ocean_pipeline_key(),
        MaterialFragmentKey::from_material([0, 0, 2, 0], 0.0),
    );
    let source = generated_source(program_variant(variant, key), key);
    let interface = source
        .split("struct GeneratedVertexOut {")
        .nth(1)
        .unwrap()
        .split("};")
        .next()
        .unwrap();
    assert!(interface.contains("lightmap_uv"));
    for field in [
        "color:",
        "world_position:",
        "world_normal:",
        "shell_kind:",
        "lightgrid",
    ] {
        assert!(!interface.contains(field), "unused {field}");
    }
    assert!(!function_body(&source, "apply_tc_mods").contains("for ("));
    assert!(!function_body(&source, "generated_uv").contains("material_tc_gen"));
}

#[test]
fn generated_wave_evaluation_is_vertex_only_and_flat() {
    let variant = WorldShaderVariantKey {
        material_specialization: true,
        program_mode: WorldProgramMode::Generated,
        ..Default::default()
    };
    let key = program_key(
        variant,
        super::super::pipelines::ocean_pipeline_key(),
        MaterialFragmentKey::from_material([0, 0, 134217728 | 268435456, 0], 0.0),
    );
    let source = generated_source(program_variant(variant, key), key);
    assert!(function_body(&source, "vs_generated").contains("vertex_wave_gens"));
    assert!(source.contains("@interpolate(flat) wave_color"));
    assert!(!function_body(&source, "apply_wave_gens").contains("wave_value"));
    assert!(!function_body(&source, "legacy_rgb_output").contains("wave_value"),
        "additive A/B must reuse vertex-generated wave in generated stages");
}

#[test]
fn program_cache_distinguishes_mode_interface_and_tcmod_sequence() {
    use super::super::pipelines::WorldPassPipelineIdentity;
    let mut pipeline = super::super::pipelines::ocean_pipeline_key();
    pipeline.class = crate::renderer::DrawClass::Opaque;
    let m = MaterialFragmentKey::from_material([0, 0, 2, 0], 0.0);
    let v = WorldShaderVariantKey {
        material_specialization: true,
        ..Default::default()
    };
    let make = |variant, raw| {
        let key = program_key(variant, pipeline, raw);
        WorldPassPipelineIdentity::specialized(
            variant,
            false,
            pipeline,
            key,
            super::super::pipelines::material_minimal_entry(variant, &pipeline),
            wgpu::TextureFormat::Bgra8UnormSrgb,
            1,
        )
    };
    let original = make(v, m);
    assert_ne!(
        original,
        make(
            WorldShaderVariantKey {
                minimal_entries: true,
                ..v
            },
            m
        )
    );
    let shared = WorldShaderVariantKey {
        program_mode: WorldProgramMode::Shared,
        ..v
    };
    let generated = WorldShaderVariantKey {
        program_mode: WorldProgramMode::Generated,
        ..v
    };
    assert_ne!(make(shared, m), make(generated, m));
    let mut modified = m;
    modified.vertex_recipe.tc_mods[0] = 1;
    assert_ne!(make(generated, m), make(generated, modified));
    assert_eq!(make(shared, m), make(shared, modified));
}

/// Real driver compilation plus image comparison. Run explicitly so CPU-only
/// environments retain deterministic shader validation coverage.
#[test]
#[ignore = "requires a Vulkan GPU"]
fn world_program_modes_gpu_images_match() {
    use crate::renderer::{block_on, CameraUniform, GpuVertex, VERTEX_ATTRIBUTES};
    use bytemuck::Zeroable;
    use wgpu::util::DeviceExt;
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        println!("world program GPU image test: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world program reference"),
            source: wgpu::ShaderSource::Wgsl(
                crate::renderer::compose_world_shader(include_str!("../../bsp_lean.wgsl")).into(),
            ),
        });
        let camera = CameraUniform {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            camera_pos_time: [0.0, 2.0, 2.0, 1.37],
            ..CameraUniform::zeroed()
        };
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&camera),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let vertices = [
            ([-1.0, -1.0, 0.5], [0.0, 0.0], [0.1, 0.3, 0.8, 0.1]),
            ([3.0, -1.0, 0.5], [2.0, 0.0], [0.8, 0.2, 0.4, 0.9]),
            ([-1.0, 3.0, 0.5], [0.0, 2.0], [0.3, 0.9, 0.1, 0.4]),
        ]
        .map(|(position, uv, color)| GpuVertex {
            position,
            uv,
            lightmap_uv: uv,
            normal: [0.0, 1.0, 0.0],
            color,
            alpha_cutoff: 0.8,
        });
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let texture = |lightmap| {
            let mut bytes = Vec::new();
            for y in 0..4 {
                for x in 0..4 {
                    bytes.extend(if lightmap {
                        [120, 160 + y * 20, 140 + x * 20, 255]
                    } else {
                        [
                            40 + x * 40,
                            30 + y * 45,
                            100,
                            if (x + y) % 2 == 0 { 80 } else { 230 },
                        ]
                    });
                }
            }
            device
                .create_texture_with_data(
                    &queue,
                    &wgpu::TextureDescriptor {
                        label: None,
                        size: wgpu::Extent3d {
                            width: 4,
                            height: 4,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &bytes,
                )
                .create_view(&Default::default())
        };
        let base_view = texture(false);
        let lm_view = texture(true);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 32,
                height: 32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32 * 256,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let modules = ProgramModules::default();
        for (flags, cutoff, tc_gen, func, profile, material_word) in [
            (2, 0.0, 0, 0, 0, 0),
            (1 | 1048576 | 2, 0.5, 0, 0, 0, 0),
            (524288 | 2097152 | 2, -0.5, 1, 0, 0, 0),
            (2 | 1073741824, 0.5, 2, 0, 0, 0),
            (2 | 134217728 | 268435456, 0.0, 3, 0, 0, 0),
            (2 | 134217728 | 268435456, 0.5, 0, 1, 0, 0),
            (2 | 134217728 | 268435456, 0.0, 0, 2, 0, 0),
            (2 | 134217728 | 268435456, 0.0, 0, 3, 0, 0),
            (2 | 134217728 | 268435456, 0.0, 0, 4, 0, 0),
            (2 | 134217728 | 268435456, 0.0, 0, 5, 0, 0),
            (2, 0.0, 0, 0, 1, 0),
            (2, 0.0, 0, 0, 2, 0),
            (2, 0.0, 0, 0, 2, 2),
            (2 | 8 | 16 | 32, 0.0, 0, 0, 2, 2),
            (2 | 8 | 134217728, 0.5, 0, 1, 2, 2),
            (2, 0.0, 0, 0, 2, 14 << 8),
            (4096, 0.0, 3, 0, 2, 0),
            // Explicit $lightmap stage: source alpha is forced opaque.
            (8388608, 0.0, 1, 0, 0, 0),
            (8388608 | 1048576, 0.0, 1, 0, 1, 0),
        ] {
            let mut material = MaterialUniform::default();
            material.header = [tc_gen, 3, flags, material_word];
            material.params[0] = cutoff;
            material.color = [0.8, 0.7, 0.9, 0.75];
            material.vector_s = [0.3, 0.2, 0.0, 0.0];
            material.vector_t = [0.0, 0.4, 0.2, 0.0];
            material.mods[0] = [2.0, 0.8, 0.7, 0.0];
            material.mods[2] = [1.0, -0.15, 0.12, 0.0];
            material.mods[4] = [3.0, 17.0, 0.0, 0.0];
            material.wave_funcs = [func, func, 0, 0];
            material.wave_rgb = [0.5, 0.35, 0.2, 0.4];
            material.wave_alpha = [0.6, 0.2, 0.1, 0.3];
            let material_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&material),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let mut reference: Option<Vec<u8>> = None;
            for mode in 0..5 {
                let variant = WorldShaderVariantKey {
                    material_specialization: mode > 0,
                    minimal_entries: mode == 2,
                    program_mode: WorldProgramMode::from_console(mode),
                    legacy_fog: profile > 0,
                    legacy_dlights: profile > 0,
                    pbr: profile == 2,
                    pom: profile == 2,
                    deluxe: profile == 2,
                    detail_textures_enabled: profile == 2,
                    planar_reflections: profile == 2,
                    surface_deformation: profile == 2,
                    ..Default::default()
                };
                let mut native = super::super::pipelines::ocean_pipeline_key();
                native.class = if cutoff == 0.0 {
                    crate::renderer::DrawClass::Opaque
                } else {
                    crate::renderer::DrawClass::Mask
                };
                let key = program_key(
                    variant,
                    native,
                    MaterialFragmentKey::from_uniform(&material),
                );
                let effective = program_variant(variant, key);
                let program = modules.shader(&device, &shader, &shader, effective, key);
                let generated = matches!(
                    key.program,
                    StageProgram::GeneratedBasic | StageProgram::GeneratedEnhanced
                );
                let compact = matches!(
                    key.program,
                    StageProgram::Minimal | StageProgram::SharedBasic
                );
                let constants: Vec<_> = effective
                    .compilation_constants()
                    .into_iter()
                    .chain([("ENABLE_ALPHA_TEST", f64::from(u8::from(cutoff != 0.0)))])
                    .chain(
                        (mode > 0)
                            .then(|| key.compilation_constants())
                            .into_iter()
                            .flatten(),
                    )
                    .collect();
                let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("world modes image comparison"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &program,
                        entry_point: Some(if generated {
                            "vs_generated"
                        } else if compact {
                            "vs_world_minimal"
                        } else {
                            "vs_main"
                        }),
                        compilation_options: wgpu::PipelineCompilationOptions {
                            constants: &constants,
                            ..Default::default()
                        },
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<GpuVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &VERTEX_ATTRIBUTES,
                        })],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &program,
                        entry_point: Some(if generated {
                            "fs_generated"
                        } else if compact {
                            "fs_world_minimal"
                        } else if effective.legacy_fog {
                            "fs_main_legacy_fog"
                        } else {
                            "fs_main"
                        }),
                        compilation_options: wgpu::PipelineCompilationOptions {
                            constants: &constants,
                            ..Default::default()
                        },
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                assert!(
                    scope.pop().await.is_none(),
                    "mode {mode} pipeline validation"
                );
                let reference_source = if generated {
                    generated_source(effective, key)
                } else {
                    crate::renderer::compose_world_shader(include_str!("../../bsp_lean.wgsl"))
                };
                let groups = test_groups(
                    &device,
                    &pipeline,
                    &reference_source,
                    [
                        if generated {
                            "vs_generated"
                        } else if compact {
                            "vs_world_minimal"
                        } else {
                            "vs_main"
                        },
                        if generated {
                            "fs_generated"
                        } else if compact {
                            "fs_world_minimal"
                        } else if effective.legacy_fog {
                            "fs_main_legacy_fog"
                        } else {
                            "fs_main"
                        },
                    ],
                    &camera_buffer,
                    &material_buffer,
                    &base_view,
                    &lm_view,
                    &sampler,
                );
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.125,
                                    g: 0.25,
                                    b: 0.375,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&pipeline);
                    for (index, group) in &groups {
                        pass.set_bind_group(*index, group, &[]);
                    }
                    pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    pass.draw(0..3, 0..1);
                }
                encoder.copy_texture_to_buffer(
                    target.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(32),
                        },
                    },
                    wgpu::Extent3d {
                        width: 32,
                        height: 32,
                        depth_or_array_layers: 1,
                    },
                );
                queue.submit([encoder.finish()]);
                let (tx, rx) = std::sync::mpsc::channel();
                readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    tx.send(r).unwrap();
                });
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                rx.recv().unwrap().unwrap();
                let mapped = readback.slice(..).get_mapped_range().unwrap();
                let pixels: Vec<_> = mapped
                    .chunks(256)
                    .flat_map(|row| row[..128].iter().copied())
                    .collect();
                drop(mapped);
                readback.unmap();
                if let Some(expected) = &reference {
                    let max = expected
                        .iter()
                        .zip(&pixels)
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(max <= 1, "mode {mode} flags={flags} cutoff={cutoff} tcGen={tc_gen} wave={func}: maximum pixel difference {max}");
                } else {
                    reference = Some(pixels);
                }
            }
        }
    });
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn deferred_world_bundle_fp16_and_program_counts_gpu() {
    use crate::renderer::{block_on, DEPTH_FORMAT};
    use super::super::deferred_bundles::{DeferredCommand, DeferredResources, DeferredWorldBundles};
    use wgpu::util::DeviceExt;
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, _) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(), ..Default::default()
        }).await.unwrap();
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[] });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &group_layout, entries: &[] });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None,
            bind_group_layouts: &[Some(&group_layout), Some(&group_layout), Some(&group_layout), Some(&group_layout), Some(&group_layout)], immediate_size: 0 });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None,
            source: wgpu::ShaderSource::Wgsl("@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> { return vec4<f32>(f32(i),0.0,0.5,1.0); } @fragment fn fs()->@location(0) vec4<f32> { return vec4<f32>(1.0); }".into()) });
        let make_pipeline = || device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None, layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"),
                compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float,
                    blend: None, write_mask: wgpu::ColorWrites::ALL })] }), multiview_mask: None, cache: None,
        });
        let pipelines = [make_pipeline(), make_pipeline()];
        let vertices = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::VERTEX, mapped_at_creation: false });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[0_u32,1,2]), usage: wgpu::BufferUsages::INDEX });
        let resources = DeferredResources { camera: group.clone(), lighting: group.clone(), shadow: group.clone(), planar: group.clone(),
            optical: None, vertices: vertices.clone(), dlight_ids: vertices, indices, color_format: wgpu::TextureFormat::Rgba16Float, sample_count: 1 };
        let key = super::super::pipelines::ocean_pipeline_key();
        let commands = (0..48).map(|i| DeferredCommand { pipeline: pipelines[usize::from(i >= 24)].clone(), surface: group.clone(), ocean: None,
            binding: (key, (i >= 24).then_some(MaterialFragmentKey { program: StageProgram::GeneratedBasic, ..Default::default() })), indices: 0..3 }).collect::<Vec<_>>();
        let mut cache = DeferredWorldBundles::default();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        for frame in 0..3 {
            cache.scratch = commands.clone();
            let result = cache.update(&device, resources.clone());
            if frame == 0 { assert!(!result.built && !result.reused); }
            else { assert_eq!(result.program_binds, [1,0,0,0,1]); assert_eq!(result.represented_draws, 48); }
            if frame == 2 { assert!(result.reused); }
        }
        let error = scope.pop().await;
        assert!(error.is_none(), "FP16 bundle validation: {error:?}");
    });
}

#[test]
fn trace_stage_eligibility_matches_program_selection() {
    // The trace's reasons must never say ELIGIBLE while the program selector
    // actually chooses a specialized fallback (or vice versa) for common cases.
    let mut pipeline = super::super::pipelines::ocean_pipeline_key();
    pipeline.class = crate::renderer::DrawClass::Opaque;
    let plain = MaterialFragmentKey::from_material([0, 0, 2, 0], 0.0);
    for mode in 2..=4 {
        let variant = WorldShaderVariantKey {
            material_specialization: true,
            minimal_entries: mode == 2,
            program_mode: WorldProgramMode::from_console(mode),
            ..Default::default()
        };
        assert!(stage_rejection_reasons(variant, pipeline, plain, mode).is_empty());
        assert_ne!(program_key(variant, pipeline, plain).program, StageProgram::Specialized);
        // Hybrid Ocean alone no longer blocks dry stages (see dry_variant).
        let ocean = WorldShaderVariantKey { ocean: true, ..variant };
        assert!(stage_rejection_reasons(ocean, pipeline, plain, mode).is_empty());
        assert_ne!(program_key(ocean, pipeline, plain).program, StageProgram::Specialized);
        let lit = WorldShaderVariantKey { point_lights: true, ..ocean };
        assert!(stage_rejection_reasons(lit, pipeline, plain, mode)
            .iter().any(|why| why.contains("world shader family")));
        assert_eq!(program_key(lit, pipeline, plain).program, StageProgram::Specialized);
        let mut autosprite = pipeline;
        autosprite.auto_sprite2 = true;
        assert!(stage_rejection_reasons(variant, autosprite, plain, mode)
            .iter().any(|why| why.contains("autosprite2")));
        assert_eq!(program_key(variant, autosprite, plain).program, StageProgram::Specialized);
        let glass = MaterialFragmentKey::from_material([0, 0, 2, 1 << 17], 0.0);
        assert!(stage_rejection_reasons(variant, pipeline, glass, mode)
            .iter().any(|why| why.contains("glass")));
        assert_eq!(program_key(variant, pipeline, glass).program, StageProgram::Specialized);
    }
    let generated = WorldShaderVariantKey {
        material_specialization: true,
        program_mode: WorldProgramMode::Generated,
        ..Default::default()
    };
    let special_tcgen = MaterialFragmentKey::from_material([7, 0, 2, 0], 0.0);
    assert!(stage_rejection_reasons(generated, pipeline, special_tcgen, 4)
        .iter().any(|why| why.contains("tcGen")));
    assert_eq!(program_key(generated, pipeline, special_tcgen).program, StageProgram::Specialized);
}

#[test]
fn hybrid_ocean_dry_stages_use_lean_programs() {
    let mut pipeline = super::super::pipelines::ocean_pipeline_key();
    pipeline.class = crate::renderer::DrawClass::Opaque;
    let wall = MaterialFragmentKey::from_material([0, 0, 2, 0], 0.0);
    let mode1 = WorldShaderVariantKey { material_specialization: true, ocean: true, ..Default::default() };
    assert_eq!(mode1.family(), WorldShaderFamily::Enhanced);
    let key = program_key(mode1, pipeline, wall);
    assert_eq!(key.program, StageProgram::SpecializedLean);
    let effective = program_variant(mode1, key);
    assert!(!effective.ocean && effective.family() == WorldShaderFamily::Lean);
    // The Enhanced-module specialization keeps the real variant.
    assert!(program_variant(mode1, MaterialFragmentKey { program: StageProgram::Specialized, ..key }).ocean);
    // Mode 1 on a dry variant is unchanged.
    assert_eq!(program_key(WorldShaderVariantKey { ocean: false, ..mode1 }, pipeline, wall).program, StageProgram::Specialized);
    // Another Enhanced feature or RT keeps the Enhanced program.
    for blocked in [
        WorldShaderVariantKey { voxel_gi: true, ..mode1 },
        WorldShaderVariantKey { ray_traced_shadows: true, ..mode1 },
    ] {
        assert!(dry_variant(blocked).is_none());
        assert_eq!(program_key(blocked, pipeline, wall).program, StageProgram::Specialized);
    }
    let generated = WorldShaderVariantKey { program_mode: WorldProgramMode::Generated, ..mode1 };
    let key = program_key(generated, pipeline, wall);
    assert_eq!(key.program, StageProgram::GeneratedBasic);
    assert!(!program_variant(generated, key).ocean);
}

#[test]
fn baked_relight_specializes_only_lightmap_stages() {
    let mut pipeline = super::super::pipelines::ocean_pipeline_key();
    pipeline.class = crate::renderer::DrawClass::Opaque;
    let variant = WorldShaderVariantKey {
        material_specialization: true,
        program_mode: WorldProgramMode::Generated,
        baked_sun_relight: true,
        ..Default::default()
    };
    // A plain texture stage never reaches light_bsp_surface.
    let texture = MaterialFragmentKey::from_material([0, 0, 0, 0], 0.0);
    let key = program_key(variant, pipeline, texture);
    assert_eq!(key.program, StageProgram::GeneratedBasic);
    assert!(!program_variant(variant, key).baked_sun_relight);
    for flags in [2, 8388608, 131072 | 1] {
        let lit = MaterialFragmentKey::from_material([0, 0, flags, 0], 0.0);
        let key = program_key(variant, pipeline, lit);
        assert_eq!(key.program, StageProgram::GeneratedEnhanced, "flags {flags}");
        let effective = program_variant(variant, key);
        assert!(effective.baked_sun_relight);
        let source = generated_source(effective, key);
        let vs = function_body(&source, "vs_generated");
        assert!(vs.contains("world_vertex(input, instance_index, legacy_dlight_surface_id)"));
        assert!(source.contains("@interpolate(flat) baked_sun_fit"));
    }
    // Vertex colour alone (no vertex-lit flag) is not baked lighting.
    let vertex_color = MaterialFragmentKey::from_material([0, 0, 1, 0], 0.0);
    assert!(!program_variant(variant, program_key(variant, pipeline, vertex_color)).baked_sun_relight);
    // Mode 3 keeps lightmapped relight specialized; plain stages stay shared.
    let shared = WorldShaderVariantKey { program_mode: WorldProgramMode::Shared, ..variant };
    let lightmapped = MaterialFragmentKey::from_material([0, 0, 2, 0], 0.0);
    assert_eq!(program_key(shared, pipeline, lightmapped).program, StageProgram::Specialized);
    assert!(stage_rejection_reasons(shared, pipeline, lightmapped, 3).iter().any(|why| why.contains("baked")));
    assert_eq!(program_key(shared, pipeline, texture).program, StageProgram::SharedBasic);
    assert!(stage_rejection_reasons(shared, pipeline, texture, 3).is_empty());
}
