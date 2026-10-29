use super::*;

#[test]
fn foliage_world_camera_uniform_matches_shader_layouts() {
    use crate::weather::rain_layout::{offset_of_member, wgsl_struct_layout};
    use std::mem::{offset_of, size_of};
    for source in [
        compose_world_shader(include_str!("../../bsp.wgsl")),
        compose_world_shader(include_str!("../../bsp_lean.wgsl")),
        format!("{}\n{}\n{}", include_str!("../../bsp_fast.wgsl"),
            crate::foliage::surface_shader(), include_str!("../../sky_fractal.wgsl")),
        crate::glass::model_shader(include_str!("../../md3.wgsl"), 2),
        crate::glass::model_shader(include_str!("../../ghoul2_skin.wgsl"), 3),
    ] {
        let (size, members) = wgsl_struct_layout(&source, "Camera");
        assert_eq!(size as usize, size_of::<CameraUniform>());
        assert_eq!(offset_of_member(&members, "foliage_wind") as usize,
            offset_of!(CameraUniform, foliage_wind));
    }
}

#[test]
fn foliage_wind_vertex_gpu_moves_only_promoted_cards() {
    use wgpu::util::DeviceExt;
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let source = format!("{}\n{}", crate::foliage::SHADER, r#"
@group(0) @binding(0) var<storage, read_write> result: array<vec4<f32>, 8>;
@compute @workgroup_size(8) fn main(@builtin(local_invocation_index) i: u32) {
    let p = vec3<f32>(10.0, 50.0, 20.0);
    var tag = 1.234;
    var wind = vec4<f32>(0.3, 0.4, 0.0, 0.0);
    if (i == 0u) { tag = 0.0; }
    if (i == 1u) { wind = vec4<f32>(0.0); }
    if (i == 3u) { wind = vec4<f32>(wind.xy, 2.0, 0.0); }
    if (i == 6u) { tag = 1.876; }
    if (i == 7u) { wind = vec4<f32>(wind.xy * 2.0, wind.zw); }
    let position = p + select(vec3<f32>(0.0), vec3<f32>(1.0, 0.0, 0.0), i == 5u);
    result[i] = vec4<f32>(foliage_model_position(position, tag, wind) - position, 0.0);
}
"#);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("foliage displacement regression"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None, layout: None, module: &shader, entry_point: Some("main"),
            compilation_options: Default::default(), cache: None,
        });
        let output = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: &[0u8; 128],
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: 128, mapped_at_creation: false,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        });
        let (noise, sampler) = crate::foliage::test_noise_resources(&device, &queue);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &pipeline.get_bind_group_layout(0),
            entries: &crate::foliage::camera_entries(&output, &noise, &sampler),
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 128);
        queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |status| tx.send(status).unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(30)) }).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = slice.get_mapped_range().expect("foliage shader readback is mapped");
        let samples: &[[f32; 4]] = bytemuck::cast_slice(&bytes);
        assert_eq!(samples[0], [0.0; 4], "ordinary geometry must stay still");
        assert_eq!(samples[1], [0.0; 4], "zero wind must stop the sway");
        assert_ne!(samples[2], samples[3], "advecting grass noise must move the leaf");
        assert_ne!(samples[2], samples[6], "separate plants must get different local motion");
        assert_eq!(samples[2], samples[4], "shared positions must not develop seams");
        for sample in &samples[2..] {
            assert!(sample.iter().all(|v| v.is_finite() && v.abs() < crate::foliage::BOUNDS_PADDING));
        }
        for axis in 0..3 {
            assert!((samples[7][axis] - 2.0 * samples[2][axis]).abs() < 1e-4,
                "doubling inherited wind speed must double unsaturated bend");
            assert!((samples[2][axis] - samples[5][axis]).abs() < 0.4,
                "neighboring vertices must sway coherently");
        }
    });
}

#[test]
fn csm_coverage_ab_variants_specialize_and_emit_spirv() {
    use wgpu::naga;
    for base in [include_str!("../../bsp.wgsl"), include_str!("../../bsp_lean.wgsl")] {
        let source = compose_world_shader(base);
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all()).validate(&module).unwrap();
        for enabled in [false, true] {
            let key = WorldShaderVariantKey { cascaded_shadows: true,
                csm_coverage: enabled, baked_sun_relight: true, ..Default::default() };
            let constants = key.compilation_constants().into_iter()
                .map(|(name, value)| (name.to_owned(), value)).collect();
            let (specialized, info) = naga::back::pipeline_constants::process_overrides(
                &module, &info, None, &constants).unwrap();
            naga::back::spv::write_vec(&specialized, &info, &naga::back::spv::Options {
                lang_version: (1, 4), ..Default::default()
            }, None).unwrap();
        }
    }
}

#[test]
fn promoted_glass_shader_variants_validate() {
    for (name, source) in [
        ("glass", crate::glass::world_shader()),
        ("glass boundaries", include_str!("../../glass_edges.wgsl").to_owned()),
        ("glass Gaussian", include_str!("../../glass_blur.wgsl").to_owned()),
        ("ui", include_str!("../../ui.wgsl").to_owned()),
        ("world", compose_world_shader(include_str!("../../bsp.wgsl"))),
        ("lean", compose_world_shader(include_str!("../../bsp_lean.wgsl"))),
        ("fast", format!("{}\n{}\n{}",
            include_str!("../../bsp_fast.wgsl"),
            crate::foliage::surface_shader(),
            include_str!("../../sky_fractal.wgsl"))),
        ("md3", crate::glass::model_shader(include_str!("../../md3.wgsl"), 2)),
        ("ghoul2", crate::glass::model_shader(include_str!("../../ghoul2_skin.wgsl"), 3)),
    ] {
        let module = wgpu::naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(&source)));
    }
}

#[test]
fn promoted_ocean_edge_fast_path_skips_fft_and_stays_lod_independent() {
    let shader = include_str!("../../bsp.wgsl");
    let edge_begin = shader.find("fn promoted_ocean_edge_weight(").unwrap();
    let sample_begin = shader.find("fn sample_ocean_displacement(").unwrap();
    let next_function = shader[sample_begin..].find("struct VertexLightgridSample").unwrap()
        + sample_begin;
    let edge = &shader[edge_begin..sample_begin];
    let sample = &shader[sample_begin..next_function];
    assert!(!edge.contains("vertex_spacing") && !edge.contains("input.color"),
        "Shared fine/coarse clipmap vertices must receive the same edge weighting");
    let zero_return = sample.find("if (edge_weight <= 0.0) { return vec4<f32>(0.0); }").unwrap();
    let first_fft_read = sample.find("textureSampleLevel(ocean_displacement").unwrap();
    assert!(zero_return < first_fft_read, "Flat vertices must skip all FFT sampling");
    assert!(edge.contains("ocean.appearance_c.w"), "Outer seam must match analytic radius");
}

#[test]
fn legacy_rgb_transfer_is_shared_by_fused_and_additive_bsp_paths() {
    // Both zero-pass implementations must use the same exact-sRGB transfer.
    // Keep fast, lean and full variants in sync without another pipeline.
    for (name, shader) in [
        ("world", include_str!("../../bsp.wgsl")),
        ("lean", include_str!("../../bsp_lean.wgsl")),
        ("fast", include_str!("../../bsp_fast.wgsl")),
    ] {
        assert!(shader.contains("fn legacy_rgb_encode("), "{name}: missing shared encoder");
        assert!(shader.contains("fn legacy_rgb_decode("), "{name}: missing fused decoder");
        assert!(shader.contains("fn legacy_rgb_output("), "{name}: missing additive output");
        assert!(shader.contains("MATERIAL_LEGACY_ADDITIVE_SRGB: u32 = 536870912u"), "{name}: material bit changed");
        assert!(shader.contains("legacy_rgb_encode(overlay_sample.rgb)"), "{name}: fused path drifted");
        assert!(shader.contains("let vertex_wave_byte = round(clamp(waveform, 0.0, 1.0) * 255.0) / 255.0;"), "{name}: 8-bit TaystJK wave modulation missing");
        assert!(shader.contains("legacy_rgb_encode(texel_linear) * vertex_wave_byte"), "{name}: encoded-space wave modulation missing");
        assert!(shader.contains("fn legacy_additive_source("), "{name}: missing additive source transfer");
        assert!(shader.contains("sample = legacy_additive_source("), "{name}: fused additive path must use parity source transfer");
        assert!(!shader.contains("additive_blend_debug_mode"), "{name}: A/B selection leaked into production shader");
        assert!(!shader.contains("LEGACY_ADDITIVE_SOURCE_SRGB_TEST"), "{name}: temporary test leaked");
    }
}

#[test]
fn world_tcmods_match_taystjk_scroll_wrap_and_rotation_sign() {
    // rd-vanilla/tr_shade_calc.cpp: RB_CalcScrollTexCoords uses
    // scroll - floor(scroll), and RB_CalcRotateTexCoords negates the angle.
    // World, lean, fast, RT, shadow and depth paths must agree.
    for (name, wgsl) in [
        ("world", include_str!("../../bsp.wgsl")),
        ("world-fast", include_str!("../../bsp_fast.wgsl")),
        ("world-lean", include_str!("../../bsp_lean.wgsl")),
        ("rt-core", include_str!("../../rt_core.wgsl")),
        ("shadow", include_str!("../../shadow.wgsl")),
        ("depth", include_str!("../../depth_prepass.wgsl")),
    ] {
        assert!(wgsl.contains("uv = uv + scroll - floor(scroll);"), "{name}: missing TaystJK scroll wrap");
        assert!(wgsl.contains("let radians = -a.y * time * 0.017453292519943295;"),
            "{name}: tcMod rotate must use TaystJK's negative angle");
        assert!(wgsl.contains("let p = uv - vec2<f32>(0.5);"),
            "{name}: rotate must pivot around the post-scroll UV (0.5, 0.5)");
    }

    // seph_oceanglow2: scale 0.4, scroll -0.05 0.0008, rotate 4.5.
    // After 20s, scroll X is -1.0 and wraps to 0 before rotation.
    // Starting from (1.25,1.25), the scaled UV is (0.5,0.5).
    let seconds = 20.0_f32;
    let scroll_x = (-0.05 * seconds).rem_euclid(1.0);
    let scroll_y = (0.0008 * seconds).rem_euclid(1.0);
    let (sin, cos) = (-4.5 * seconds).to_radians().sin_cos();
    let x = 0.5 + scroll_x - 0.5;
    let y = 0.5 + scroll_y - 0.5;
    let uv = [0.5 + x*cos - y*sin, 0.5 + x*sin + y*cos];
    assert!((uv[0] - 0.516).abs() < 0.0001);
    assert!((uv[1] - 0.5).abs() < 0.0001);
}


#[test]
fn rt_isolation_variants_validate_and_match_receiver_layouts() {
    use crate::renderer::lighting::{ray_tracing::{ray_traced_world_shader_source_with_features,
        rt_receiver_layout_entries}, rt_diagnostics::ReceiverFeatures};
        for glass in [false, true] {
            let features = ReceiverFeatures { glass };
            for base in [include_str!("../../bsp.wgsl"), include_str!("../../bsp_lean.wgsl")] {
                let source = compose_world_shader(&ray_traced_world_shader_source_with_features(base, features).unwrap());
                let module = wgpu::naga::front::wgsl::parse_str(&source)
                    .unwrap_or_else(|e| panic!("{features:?}: {}", e.emit_to_string(&source)));
                wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
                    wgpu::naga::valid::Capabilities::all()).validate(&module)
                    .unwrap_or_else(|e| panic!("{features:?}: {}", e.emit_to_string(&source)));
                let actual: std::collections::BTreeSet<_> = module.global_variables.iter()
                    .filter_map(|(_, global)| global.binding.as_ref())
                    .filter(|binding| binding.group == 3).map(|binding| binding.binding).collect();
                let expected: std::collections::BTreeSet<_> = rt_receiver_layout_entries(features)
                    .iter().map(|entry| entry.binding).collect();
                assert_eq!(actual, expected, "{features:?}: WGSL/layout drift");
                assert!(!source.contains("fn rt_denoised_local_visibility("));
                assert!(!source.contains("fn rt_denoiser_debug("));
                assert_eq!(source.contains("fn rt_trace_shadow_throughput("), glass);
                assert_eq!(source.contains("var<storage, read> raster_glass_slots:"), glass);
            }
        }

}

#[test]
#[ignore = "requires a Vulkan GPU with hardware ray queries; run each isolation mode in a separate process"]
fn rt_isolation_world_gpu_pipelines() {
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.unwrap();
        println!("RT world pipeline validation: {:?}", adapter.get_info());
        let (device, _) = adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("RT world isolation"),
            required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY,
            required_limits: adapter.limits(),
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        }).await.unwrap();
        let minimal = std::env::var_os("JKA_RT_GPU_PROBE_MINIMAL").is_some();
        for (family, base) in [("bsp-rt-enhanced", include_str!("../../bsp.wgsl")),
            ("bsp-rt-lean", include_str!("../../bsp_lean.wgsl"))] {
            if minimal && family != "bsp-rt-enhanced" { continue; }
            let source = if let Some(path) = std::env::var_os("JKA_RT_GPU_PROBE_SOURCE") {
                std::fs::read_to_string(path).expect("GPU probe WGSL override")
            } else { compose_world_shader(&ray_traced_world_shader_source(base).unwrap()) };
            crate::renderer::lighting::rt_diagnostics::dump_shader(family, &source);
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(family), source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            // Exercise RT local lighting with optional CSM and legacy fragment entries.
            // Inferred layouts make this a headless compiler probe; it is not a
            // substitute for the exact map's recorded key and explicit layout.
            for pbr in [0.0, 1.0] {
                if minimal && pbr != 0.0 { continue; }
                for cascades in [0.0, 1.0] {
                    if minimal && cascades != 0.0 { continue; }
                    for entry in ["fs_main", "fs_main_legacy_fog", "fs_legacy_fog_pass"] {
                        if minimal && entry != "fs_main" { continue; }
                        let constants = [("ENABLE_RAY_TRACED_SHADOWS", 1.0),
                            ("ENABLE_CASCADED_SHADOWS", cascades), ("ENABLE_LOCAL_SHADOWS", 1.0),
                            ("ENABLE_POINT_LIGHTS", 1.0), ("ENABLE_PBR", pbr)];
                        println!("[RT GPU probe BEGIN] family={family} pbr={pbr} cascades={cascades} fs={entry}");
                        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                        let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                            label: Some("RT world isolation probe"), layout: None,
                            vertex: wgpu::VertexState {
                                module: &shader, entry_point: Some("vs_main"),
                                compilation_options: Default::default(),
                                buffers: &[Some(wgpu::VertexBufferLayout {
                                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                                    step_mode: wgpu::VertexStepMode::Vertex, attributes: &VERTEX_ATTRIBUTES,
                                })],
                            },
                            primitive: Default::default(),
                            depth_stencil: Some(wgpu::DepthStencilState {
                                format: DEPTH_FORMAT, depth_write_enabled: Some(true),
                                depth_compare: Some(camera_depth_compare()),
                                stencil: Default::default(), bias: Default::default(),
                            }),
                            multisample: Default::default(),
                            fragment: Some(wgpu::FragmentState {
                                module: &shader, entry_point: Some(entry),
                                compilation_options: wgpu::PipelineCompilationOptions {
                                    constants: &constants, ..Default::default()
                                },
                                targets: &[Some(wgpu::ColorTargetState {
                                    format: wgpu::TextureFormat::Rgba16Float,
                                    blend: None, write_mask: wgpu::ColorWrites::ALL,
                                })],
                            }),
                            multiview_mask: None, cache: None,
                        });
                        if let Some(error) = error_scope.pop().await {
                            panic!("RT GPU probe validation failed: {error}");
                        }
                        println!("[RT GPU probe END] family={family} pbr={pbr} cascades={cascades} fs={entry}");
                    }
                }
            }
        }
    });
}

/// Every shader that binds the post buffer reads it through one shared WGSL
/// struct. If a field is added on one side only, everything after it shifts;
/// rain_haze_mask.wgsl once kept a private, stale copy for exactly that reason.
#[test]
fn post_uniform_matches_the_shared_wgsl_struct() {
    use crate::weather::rain_layout::{offset_of_member, wgsl_struct_layout};
    use std::mem::{offset_of, size_of};
    let (size, members) =
        wgsl_struct_layout(include_str!("../../post_settings.wgsl"), "PostSettings");
    assert_eq!(size as usize, size_of::<PostUniform>());
    for (name, rust_offset) in [
        ("color", offset_of!(PostUniform, color)),
        ("aa", offset_of!(PostUniform, aa)),
        ("taa_params", offset_of!(PostUniform, taa_params)),
        ("scene", offset_of!(PostUniform, scene)),
        ("film", offset_of!(PostUniform, film)),
        ("grain", offset_of!(PostUniform, grain)),
        ("exposure_controls", offset_of!(PostUniform, exposure_controls)),
        ("camera_fx", offset_of!(PostUniform, camera_fx)),
        ("legacy_fog", offset_of!(PostUniform, legacy_fog)),
        ("clouds", offset_of!(PostUniform, clouds)),
        ("cloud_layer", offset_of!(PostUniform, cloud_layer)),
        ("cloud_sun_direction", offset_of!(PostUniform, cloud_sun)),
        ("cloud_shadow", offset_of!(PostUniform, cloud_shadow)),
        ("cloud_shaping", offset_of!(PostUniform, cloud_shaping)),
        (
            "cloud_sky_ambient",
            offset_of!(PostUniform, cloud_sky_ambient),
        ),
        (
            "cloud_temporal_tuning",
            offset_of!(PostUniform, cloud_temporal_tuning),
        ),
        ("cloud_variation", offset_of!(PostUniform, cloud_variation)),
        ("cloud_temporal", offset_of!(PostUniform, cloud_temporal)),
        (
            "cloud_wind_dir",
            offset_of!(PostUniform, cloud_wind) + offset_of!(PostCloudWindSettings, direction),
        ),
        (
            "cloud_wind_offset",
            offset_of!(PostUniform, cloud_wind) + offset_of!(PostCloudWindSettings, offset),
        ),
        (
            "cloud_wind_delta",
            offset_of!(PostUniform, cloud_wind) + offset_of!(PostCloudWindSettings, delta),
        ),
        (
            "cloud_detail_slip",
            offset_of!(PostUniform, cloud_wind) + offset_of!(PostCloudWindSettings, detail_slip),
        ),
        (
            "cloud_detail_billow",
            offset_of!(PostUniform, cloud_wind) + offset_of!(PostCloudWindSettings, detail_billow),
        ),
        ("rain", offset_of!(PostUniform, rain)),
        ("rain_occlusion", offset_of!(PostUniform, weather_occlusion)),
        ("weather_look", offset_of!(PostUniform, weather_look)),
        ("legacy_weather", offset_of!(PostUniform, legacy_weather)),
        ("snow_haze", offset_of!(PostUniform, snow_haze)),
        ("underwater", offset_of!(PostUniform, underwater)),
        (
            "cloud_foreground",
            offset_of!(PostUniform, cloud_foreground),
        ),
        ("cloud_blades", offset_of!(PostUniform, cloud_blades)),
        ("camera_pos_time", offset_of!(PostUniform, camera_pos_time)),
        (
            "prev_camera_pos_time",
            offset_of!(PostUniform, prev_camera_pos_time),
        ),
        ("view_proj", offset_of!(PostUniform, view_proj)),
        ("inv_view_proj", offset_of!(PostUniform, inv_view_proj)),
        (
            "cloud_inv_view_proj",
            offset_of!(PostUniform, cloud_inv_view_proj),
        ),
        (
            "cloud_prev_view_proj",
            offset_of!(PostUniform, cloud_prev_view_proj),
        ),
        ("prev_view_proj", offset_of!(PostUniform, prev_view_proj)),
        (
            "motion_prev_view_proj",
            offset_of!(PostUniform, motion_prev_view_proj),
        ),
    ] {
        assert_eq!(
            offset_of_member(&members, name) as usize,
            rust_offset,
            "{name}"
        );
    }
}

#[test]
fn rt_segment_light_preserves_power_and_point_mode() {
    let mut light = TransientLight {
        kind: crate::fx::system::FxLightKind::AuthoredEffect,
        position: [5.0, 2.0, 3.0],
        color: [1.0, 0.2, 0.1],
        radius: 14.0,
        source_radius: 3.5,
        intensity: 1.0,
        spot: None,
        segment: Some([[0.0, 2.0, 3.0], [10.0, 2.0, 3.0]]),
        blade_segments: None,
    };
    let point = transient_light_gpu(&light, 2.0, false);
    let segment = transient_light_gpu(&light, 2.0, true);
    assert_eq!(point.emitter, [0.0; 4]);
    assert_eq!(segment.emitter, [5.0, 0.0, 0.0, -1.0]);
    assert_eq!(segment.color_intensity, point.color_intensity);
    assert_eq!(segment.position_radius, point.position_radius);
    assert_eq!(point.shadow[2], 3.5, "source radius is separate from attenuation radius");
    assert_eq!(segment.shadow[2], 3.5, "segments retain a finite cross-section for RT sampling");
    // Cluster radius is influence radius plus half-length; it encloses all
    // endpoint influence spheres rather than incorrectly culling the tips.
    let center = Vec3::from_slice(&segment.position_radius[..3]);
    let half = Vec3::from_slice(&segment.emitter[..3]);
    let cluster_radius = segment.position_radius[3] + half.length();
    for endpoint in light.segment.unwrap() {
        assert!(Vec3::from_array(endpoint).distance(center) + light.radius <= cluster_radius);
    }
    light.segment = Some([[f32::NAN, 0.0, 0.0], [0.0; 3]]);
    assert_eq!(transient_light_gpu(&light, 2.0, true).emitter, [0.0; 4]);
    light.segment = Some([[1.0; 3]; 2]);
    assert_eq!(transient_light_gpu(&light, 2.0, true).emitter, [0.0; 4]);
}

#[test]
fn promoted_spot_light_packs_direction_and_cone_without_stealing_shadow_metadata() {
    let light = TransientLight {
        kind: crate::fx::system::FxLightKind::PromotedEffect,
        position: [1.0, 2.0, 3.0],
        color: [1.0, 0.5, 0.1],
        radius: 256.0,
        source_radius: 12.0,
        intensity: 1.0,
        spot: Some(crate::fx::system::FxSpotLight {
            direction: [2.0, 0.0, 0.0],
            inner_cone_cos: 0.94,
            outer_cone_cos: 0.82,
        }),
        segment: None,
        blade_segments: None,
    };
    let gpu = transient_light_gpu(&light, 1.0, false);
    assert_eq!(gpu.emitter, [1.0, 0.0, 0.0, 3.0]);
    assert_eq!(gpu.shape, [0.94, 0.82, 0.0, 0.0]);
    assert_eq!(gpu.shadow, [0.0, 0.0, 12.0, 1.0]);

    let mut malformed = light.clone();
    malformed.spot = Some(crate::fx::system::FxSpotLight {
        direction: [f32::NAN, 0.0, 0.0],
        inner_cone_cos: 0.94,
        outer_cone_cos: 0.82,
    });
    let fallback = transient_light_gpu(&malformed, 1.0, false);
    assert_eq!(fallback.emitter, [0.0; 4], "invalid inferred cones fall back to point lights");
    assert_eq!(fallback.shape, [0.0; 4]);
}

#[test]
fn rt_multiblade_expansion_conserves_aggregate_power() {
    use crate::fx::system::FxLightSegment;
    let light = TransientLight {
        kind: crate::fx::system::FxLightKind::AuthoredEffect,
        position: [0.0; 3],
        radius: 80.0,
        source_radius: 0.0,
        color: [0.25, 0.0, 0.75],
        intensity: 2.0,
        spot: None,
        segment: None,
        blade_segments: Some(
            vec![
                FxLightSegment {
                    endpoints: [[0.0; 3], [10.0, 0.0, 0.0]],
                    rgb: [1.0, 0.0, 0.0],
                    weight: 0.25,
                },
                FxLightSegment {
                    endpoints: [[0.0; 3], [0.0, 30.0, 0.0]],
                    rgb: [0.0, 0.0, 1.0],
                    weight: 0.75,
                },
            ]
            .into(),
        ),
    };
    let point = transient_light_gpu(&light, 2.0, false);
    let blades = transient_light_samples(&light, true)
        .map(|sample| transient_light_gpu(&sample, 2.0, true))
        .collect::<Vec<_>>();
    assert_eq!(blades.len(), 2);
    for channel in 0..3 {
        let sum: f32 = blades
            .iter()
            .map(|blade| blade.color_intensity[channel] * blade.color_intensity[3])
            .sum();
        assert!((sum - point.color_intensity[channel] * point.color_intensity[3]).abs() < 1e-6);
    }
    assert_eq!(blades[0].position_radius, [5.0, 0.0, 0.0, 80.0]);
    assert_eq!(blades[1].position_radius, [0.0, 15.0, 0.0, 80.0]);
    assert!(blades
        .iter()
        .all(|blade| blade.emitter[3] == -1.0 && blade.shadow[3] == 1.0));
    let fallback = transient_light_samples(&light, false).collect::<Vec<_>>();
    assert_eq!(fallback, [light]);
}

#[test]
fn rt_empty_alpha_tables_satisfy_shader_binding_sizes() {
    let descriptors = [
        rt_alpha_empty_buffer_descriptor::<RtAlphaVertexGpu>(
            "vertices",
            wgpu::BufferUsages::BLAS_INPUT,
        ),
        rt_alpha_empty_buffer_descriptor::<u32>("indices", wgpu::BufferUsages::BLAS_INPUT),
        rt_alpha_empty_buffer_descriptor::<RtAlphaGeometryGpu>(
            "geometries",
            wgpu::BufferUsages::empty(),
        ),
        rt_alpha_empty_buffer_descriptor::<RtAlphaMaterialGpu>(
            "materials",
            wgpu::BufferUsages::empty(),
        ),
        rt_alpha_empty_buffer_descriptor::<RtAlphaTextureGpu>(
            "textures",
            wgpu::BufferUsages::empty(),
        ),
        rt_alpha_empty_buffer_descriptor::<u32>("texels", wgpu::BufferUsages::empty()),
    ];
    let entries = rt_alpha_storage_entries();
    for base in [
        include_str!("../../bsp.wgsl"),
        include_str!("../../bsp_lean.wgsl"),
    ] {
        let source = compose_world_shader(&ray_traced_world_shader_source(base).unwrap());
        let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap();
        for (entry, descriptor) in entries.iter().zip(&descriptors) {
            let global = module
                .global_variables
                .iter()
                .map(|(_, global)| global)
                .find(|global| {
                    global.binding.as_ref().is_some_and(|binding| {
                        binding.group == 3 && binding.binding == entry.binding
                    })
                })
                .expect("RT alpha binding must exist in both generated shaders");
            let wgpu::naga::TypeInner::Array {
                stride,
                size: wgpu::naga::ArraySize::Dynamic,
                ..
            } = module.types[global.ty].inner
            else {
                panic!("RT alpha binding must be a runtime array");
            };
            let wgpu::BindingType::Buffer {
                min_binding_size: Some(minimum),
                ..
            } = entry.ty
            else {
                panic!("RT alpha binding must declare its minimum size");
            };
            assert_eq!(
                minimum.get(),
                u64::from(stride),
                "Rust/WGSL element mismatch at binding {}",
                entry.binding
            );
            assert!(
                descriptor.size >= u64::from(stride),
                "empty {} buffer is smaller than one WGSL element",
                descriptor.label.unwrap()
            );
            assert!(descriptor
                .usage
                .contains(wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST));
        }
    }
}

#[test]
fn rt_alpha_cache_reuses_textures_and_accepts_changed_skins() {
    let mut textures = Vec::new();
    let mut texels = Vec::new();
    let mut cache = HashMap::new();
    let mut skin = TextureData {
        label: "mask_a".into(),
        source: None,
        width: 2,
        height: 1,
        rgba: (vec![255, 255, 255, 0, 255, 255, 255, 192]).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: false,
        srgb: true,
    };
    let first =
        rt_alpha_ensure_dynamic_texture(Some(&skin), &mut textures, &mut texels, &mut cache);
    assert_eq!(texels, [192 << 8]);
    assert_eq!(textures[first as usize].values, [0, 2, 1, 0]);
    for _ in 0..4 {
        assert_eq!(
            rt_alpha_ensure_dynamic_texture(Some(&skin), &mut textures, &mut texels, &mut cache),
            first
        );
    }
    assert_eq!((textures.len(), texels.len()), (1, 1));
    skin.label = "mask_b".into();
    skin.rgba[3] = 255;
    let second =
        rt_alpha_ensure_dynamic_texture(Some(&skin), &mut textures, &mut texels, &mut cache);
    assert_ne!(first, second);
    assert_eq!(texels, [192 << 8, 255 | (192 << 8)]);
    let material = rt_alpha_dynamic_material(second);
    assert_eq!(material.texture[0], second);
    assert_eq!(material.params[0], 0.5);
    assert_eq!(material.color, [1.0; 4]);
    // Missing/truncated pixel data uses the same white fallback key during
    // both table insertion and the subsequent per-frame material lookup.
    skin.rgba.clear();
    let fallback =
        rt_alpha_ensure_dynamic_texture(Some(&skin), &mut textures, &mut texels, &mut cache);
    assert_eq!(cache[&rt_alpha_texture_key(Some(&skin))], fallback);
    assert_eq!(textures[fallback as usize].values[1..3], [1, 1]);
    assert_eq!(*texels.last().unwrap(), 255);
}

#[test]
fn sky_cloud_stages_become_ordered_sky_batches_with_view_direction_coordinates() {
    use crate::materials::{MaterialStage, StageTexture, SurfaceMaterial};
    use jka_assets::shader::{AlphaGen, RgbGen};

    let cloud = |texture: usize, blend: Option<crate::materials::BlendFunc>, mods: Vec<TcMod>| {
        MaterialStage {
            texture: StageTexture::Image(texture),
            animation: None,
            enhancements: Default::default(),
            blend,
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: mods,
            depth_write: false,
            depth_equal: false,
                    glow: false,
        }
    };
    let material = SurfaceMaterial {
        sky: true,
        sky_cloud_height: 2048.0,
        explicit: true,
        stages: vec![
            cloud(
                3,
                None,
                vec![TcMod::Transform([0.25, 0.5, 0.75, 1.0, 0.1, 0.2])],
            ),
            cloud(
                4,
                Some(crate::materials::BlendFunc {
                    src: crate::materials::BlendFactor::One,
                    dst: crate::materials::BlendFactor::One,
                }),
                Vec::new(),
            ),
        ],
        ..Default::default()
    };
    let mut batches = Vec::new();
    crate::scene::append_sky_batches(
        &mut batches,
        &material,
        Some(0),
        None,
        false,
        0..6,
        None,
        &[],
        [0; 4],
        [0.0; 4],
        false,
    );

    // Outer box first, then one batch per authored cloud stage, in order.
    assert_eq!(batches.len(), 3);
    assert!(batches
        .iter()
        .all(|batch| batch.pipeline.class == DrawClass::Sky));
    assert!(matches!(batches[0].tc_gen, TcGen::Base));
    assert!(matches!(batches[1].tc_gen, TcGen::SkyCloud(h) if h == 2048.0));
    assert_eq!((batches[1].texture, batches[2].texture), (Some(3), Some(4)));
    assert_eq!(batches[1].pipeline.blend, BlendMode::Opaque);
    assert_ne!(batches[2].pipeline.blend, BlendMode::Opaque);
    // Only the first opaque layer seeds depth; a later blended layer must not.
    assert!(batches[1].pipeline.depth_write);
    assert!(!batches[2].pipeline.depth_write);

    let uniform = material_uniform(&batches[1], false);
    assert_eq!(uniform.header[0], 4);
    assert_eq!(uniform.params[2], 2048.0);
    assert_eq!(uniform.header[1], 1);
    // Engine: s' = s*m00 + t*m10 + t0, t' = s*m01 + t*m11 + t1.
    assert_eq!(uniform.mods[0], [4.0, 0.25, 0.75, 0.1]);
    assert_eq!(uniform.mods[1], [0.5, 1.0, 0.2, 0.0]);
    assert_eq!(material_uniform(&batches[0], false).header[0], 0);

    // A sky without cloud stages is still just its outer box.
    let plain = SurfaceMaterial {
        sky: true,
        explicit: true,
        ..Default::default()
    };
    let mut plain_batches = Vec::new();
    crate::scene::append_sky_batches(
        &mut plain_batches,
        &plain,
        Some(0),
        None,
        false,
        0..6,
        None,
        &[],
        [0; 4],
        [0.0; 4],
        false,
    );
    assert_eq!(plain_batches.len(), 1);
}

#[test]
fn authored_world_lighting_diffuse_does_not_render_white() {
    use crate::materials::{MaterialStage, StageTexture, SurfaceMaterial};
    // Same semantics for a BSP world surface (including a vertex-lit one):
    // lightingDiffuse reads the refEntity lighting, NOT baked vertex RGB.
    // TaystJK's worldEntity has no ambient/directed terms, so RGB is black
    // but the texture/constant alpha must remain untouched.
    let stage = MaterialStage {
        texture: StageTexture::Image(1), enhancements: Default::default(),
        animation: None,
        blend: Some(crate::materials::BlendFunc {
            src: crate::materials::BlendFactor::SrcAlpha,
            dst: crate::materials::BlendFactor::OneMinusSrcAlpha,
        }),
        alpha_cutoff: 0.0, opacity: 0.35, color: [1.0, 1.0, 1.0],
        rgb_gen: RgbGen::LightingDiffuse, alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base, tc_mods: Vec::new(),
        depth_write: false, depth_equal: false, glow: false,
    };
    let material = SurfaceMaterial {
        sky: true, explicit: true, stages: vec![stage],
        ..Default::default()
    };
    let mut batches = Vec::new();
    crate::scene::append_sky_batches(
        &mut batches, &material, Some(0), None, false, 0..6,
        None, &[], [0;4], [0.0;4], false,
    );
    assert_eq!(batches[1].rgb_gen, RgbGen::LightingDiffuse);
    let uniform = material_uniform(&batches[1], false);
    assert_eq!(uniform.color, [0.0, 0.0, 0.0, 0.35]);
    assert_eq!(uniform.header[2] & 1, 0, "not rgbGen vertex");
    let mut unaffected = batches[1].clone();
    unaffected.rgb_gen = RgbGen::Identity;
    assert_eq!(material_uniform(&unaffected, false).color, [1.0, 1.0, 1.0, 0.35]);
}

#[test]
fn wave_color_and_alpha_generators_are_packed_for_the_shader() {
    use crate::materials::{MaterialStage, StageTexture, SurfaceMaterial};
    use jka_assets::shader::{Wave, WaveFunc};

    let stage = MaterialStage {
        texture: StageTexture::Image(1),
        animation: None,
        enhancements: Default::default(),
        blend: None,
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Wave(Wave {
            func: WaveFunc::Sin,
            base: 0.2,
            amplitude: 0.03,
            phase: 0.5,
            frequency: 0.04,
        }),
        alpha_gen: AlphaGen::Wave(Wave {
            func: WaveFunc::InverseSawtooth,
            base: 0.5,
            amplitude: 0.25,
            phase: 0.1,
            frequency: 2.0,
        }),
        tc_gen: TcGen::Base,
        tc_mods: vec![TcMod::Stretch(Wave {
            func: WaveFunc::Triangle, base: 1.0, amplitude: 0.5,
            phase: 0.25, frequency: 2.0,
        })],
        depth_write: false,
        depth_equal: false,
            glow: false,
    };
    let material = SurfaceMaterial {
        sky: true,
        sky_cloud_height: 512.0,
        explicit: true,
        stages: vec![stage],
        clamp_time: Some(2.0),
        ..Default::default()
    };
    let mut batches = Vec::new();
    crate::scene::append_sky_batches(
        &mut batches,
        &material,
        Some(0),
        None,
        false,
        0..6,
        None,
        &[],
        [0; 4],
        [0.0; 4],
        false,
    );
    let uniform = material_uniform(&batches[1], false);
    assert_ne!(uniform.header[2] & 134217728, 0);
    assert_ne!(uniform.header[2] & 268435456, 0);
    assert_eq!(uniform.wave_rgb, [0.2, 0.03, 0.5, 0.04]);
    assert_eq!(uniform.wave_alpha, [0.5, 0.25, 0.1, 2.0]);
    assert_eq!(uniform.wave_funcs[..2], [0, 4]);
    assert_eq!(uniform.vector_t[3], 2.0);
    assert_eq!(uniform.mods[0], [6.0, 1.0, 0.5, 0.25]);
    assert_eq!(uniform.mods[1], [2.0, 1.0, 0.0, 0.0]);
    // The clock limit shares a vector lane, so it must survive both tcGen
    // basis packing and the compact ray-tracing material conversion.
    let mut vector_batch = batches[1].clone();
    vector_batch.tc_gen = TcGen::Vector([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]);
    vector_batch.shader_time_offset = 1.5;
    let vector_uniform = material_uniform(&vector_batch, false);
    assert_eq!(vector_uniform.vector_s, [1.0, 3.0, -2.0, 0.0]);
    assert_eq!(vector_uniform.vector_t, [4.0, 6.0, -5.0, 2.0]);
    assert_eq!(vector_uniform.params[3], 1.5);
    use crate::renderer::lighting::ray_tracing::{RtAlphaMaterialSource,
        rt_alpha_material_gpu, rt_alpha_texture_source, rt_alpha_dynamic_material};
    let rt = rt_alpha_material_gpu(&RtAlphaMaterialSource {
        uniform: vector_uniform, texture: rt_alpha_texture_source(None),
        glass: false, was_opaque: false,
    }, 0);
    assert_eq!(rt.vector_t, vector_uniform.vector_t);
    assert_eq!(rt.params[3], 1.5);
    assert_eq!(rt_alpha_dynamic_material(0).vector_t[3], 0.0,
        "non-authored dynamic masks must keep an unbounded shader clock");
    let mut guarded = vector_batch.clone();
    guarded.authored_environment_composite = true;
    let guarded_uniform = material_uniform(&guarded, false);
    assert_ne!(guarded_uniform.header[3] & 4, 0);
    assert_eq!(guarded_uniform.header[2] & 8_388_608, 0,
        "the environment guard must not turn ordinary stages into explicit lightmap stages");
    guarded.authored_environment_composite = false;
    guarded.texture_is_lightmap = true;
    let lightmap_uniform = material_uniform(&guarded, false);
    assert_ne!(lightmap_uniform.header[2] & 8_388_608, 0);
    assert_eq!(lightmap_uniform.header[3] & 4, 0,
        "a real lightmap must not inherit the environment guard");
    // The outer box (no generators) leaves the wave flags clear.
    assert_eq!(
        material_uniform(&batches[0], false).header[2] & (134217728 | 268435456),
        0
    );
}

#[test]
fn jump_shade_world_variants_specialize_and_validate() {
    use wgpu::naga;
    for (name, base) in [
        ("bsp", include_str!("../../bsp.wgsl")),
        ("bsp_lean", include_str!("../../bsp_lean.wgsl")),
    ] {
        let source = compose_world_shader(base);
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        for enabled in [0.0, 1.0] {
            let constants = [("ENABLE_JUMP_SHADE".into(), enabled)]
                .into_iter()
                .collect();
            let (specialized, specialized_info) =
                naga::back::pipeline_constants::process_overrides(&module, &info, None, &constants)
                    .unwrap_or_else(|e| panic!("{name} jump shade {enabled}: {e:?}"));
            naga::back::spv::write_vec(
                &specialized,
                &specialized_info,
                &naga::back::spv::Options {
                    lang_version: (1, 4),
                    ..Default::default()
                },
                None,
            )
            .unwrap_or_else(|e| panic!("{name} jump shade {enabled}: {e:?}"));
        }
    }
}

#[test]
fn rt_shader_variants_specialize_and_emit_spirv() {
    use wgpu::naga;
    for base in [
        include_str!("../../bsp.wgsl"),
        include_str!("../../bsp_lean.wgsl"),
    ] {
        let source = compose_world_shader(&ray_traced_world_shader_source(base).unwrap());
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        // Exercise RT dynamic lights with optional raster sun and PBR.
        for (pbr, local, cascades) in [
            (0.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
            (1.0, 1.0, 0.0),
            (0.0, 1.0, 1.0),
            (1.0, 1.0, 1.0),
        ] {
            let constants = [
                ("ENABLE_RAY_TRACED_SHADOWS".into(), 1.0),
                ("ENABLE_CASCADED_SHADOWS".into(), cascades),
                ("ENABLE_LOCAL_SHADOWS".into(), local),
                ("ENABLE_POINT_LIGHTS".into(), 1.0),
                ("ENABLE_PBR".into(), pbr),
            ]
            .into_iter()
            .collect();
            let (specialized, specialized_info) =
                naga::back::pipeline_constants::process_overrides(&module, &info, None, &constants)
                    .unwrap();
            naga::back::spv::write_vec(
                &specialized,
                &specialized_info,
                &naga::back::spv::Options {
                    lang_version: (1, 4),
                    ..Default::default()
                },
                None,
            )
            .expect("RT shadow variants must compile through the Vulkan shader backend");
        }
    }
}

fn mover_vertex(jka: [f32; 3], normal: [f32; 3]) -> GpuVertex {
    GpuVertex {
        position: scene::render_position(jka),
        uv: [0.25, 0.5],
        lightmap_uv: [0.1, 0.2],
        normal: scene::render_position(normal),
        color: [1.0; 4],
        alpha_cutoff: 0.75,
    }
}

#[test]
fn inline_model_pose_matches_cg_mover_transform_in_jka_space() {
    let base = [
        mover_vertex([10.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
        mover_vertex([0.0, 0.0, 8.0], [0.0, 0.0, 1.0]),
    ];
    // AnglesToAxis(0 90 0): forward +Y, left -X, up +Z.
    let pose = InlineModelInstance {
        model: 3,
        origin: [5.0, 0.0, 2.0],
        axis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        is_portal_entity: false,
    };
    let (posed, minimum, maximum) = posed_inline_vertices(&base, pose);
    assert_eq!(scene::jka_position(posed[0].position), [5.0, 10.0, 2.0]);
    assert_eq!(scene::jka_position(posed[0].normal), [0.0, 1.0, 0.0]);
    assert_eq!(scene::jka_position(posed[1].position), [5.0, 0.0, 10.0]);
    // Only geometry moves; material/lighting attributes are the compiled ones.
    assert_eq!(
        (posed[0].uv, posed[0].lightmap_uv, posed[0].alpha_cutoff),
        ([0.25, 0.5], [0.1, 0.2], 0.75)
    );
    assert_eq!(minimum, [5.0, 2.0, -10.0]);
    assert_eq!(maximum, [5.0, 10.0, 0.0]);

    let (compiled, _, _) = posed_inline_vertices(&base, InlineModelInstance::compiled(3));
    assert_eq!(compiled[0].position, base[0].position);
    assert_eq!(compiled[1].normal, base[1].normal);
}

#[test]
fn inline_model_rt_transform_matches_raster_pose() {
    let pose = InlineModelInstance {
        model: 7,
        origin: [5.0, -3.0, 2.0],
        axis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        is_portal_entity: false,
    };
    let point = scene::render_position([10.0, 4.0, -6.0]);
    let expected = pose.transform_point(point);
    let transform = pose.rt_transform();
    let actual = [
        transform[0] * point[0] + transform[1] * point[1] + transform[2] * point[2] + transform[3],
        transform[4] * point[0] + transform[5] * point[1] + transform[6] * point[2] + transform[7],
        transform[8] * point[0]
            + transform[9] * point[1]
            + transform[10] * point[2]
            + transform[11],
    ];
    for axis in 0..3 {
        assert!((actual[axis] - expected[axis]).abs() < 1e-5);
    }
}

#[test]
fn inline_model_visibility_uses_cull_record_flags_without_vertex_mutation() {
    let base = [
        mover_vertex([1.0, 2.0, 3.0], [0.0, 0.0, 1.0]),
        mover_vertex([9.0, 2.0, 3.0], [0.0, 0.0, 1.0]),
    ];
    let (posed, minimum, maximum) = posed_inline_vertices(
        &base, InlineModelInstance::compiled(3),
    );
    assert_eq!(posed[0].position, base[0].position);
    assert_ne!(minimum, maximum);
    // Bit0 marks inline; bit1 marks snapshot-absent.
    assert_eq!(1u32 & 2u32, 0);
    assert_ne!(3u32 & 2u32, 0);
}

#[test]
fn planar_reflection_matrix_reflects_across_authored_plane() {
    let plane = [1.0, 0.0, 0.0, -5.0];
    let reflected = reflection_matrix(plane).transform_point3(Vec3::new(7.0, 2.0, -3.0));
    assert!((reflected - Vec3::new(3.0, 2.0, -3.0)).length() < 1e-5);
    let on_plane = reflection_matrix(plane).transform_point3(Vec3::new(5.0, 4.0, 9.0));
    assert!((on_plane - Vec3::new(5.0, 4.0, 9.0)).length() < 1e-5);
}

#[test]
fn planar_selection_prefers_screen_centre_over_edge() {
    let centre = projected_planar_importance(
        [-0.25, -0.25, 0.5],
        [0.25, 0.25, 0.5],
        [0.0, 0.0, 1.0, -0.5],
        Mat4::IDENTITY,
    )
    .unwrap();
    let edge = projected_planar_importance(
        [0.70, -0.25, 0.5],
        [0.95, 0.25, 0.5],
        [0.0, 0.0, 1.0, -0.5],
        Mat4::IDENTITY,
    )
    .unwrap();
    assert!(centre.0);
    assert!(!edge.0);
    assert!(centre.1 > edge.1);
}

#[test]
fn planar_selection_scores_larger_screen_coverage_higher() {
    let large = projected_planar_importance(
        [-0.60, -0.60, 0.5],
        [0.60, 0.60, 0.5],
        [0.0, 0.0, 1.0, -0.5],
        Mat4::IDENTITY,
    )
    .unwrap();
    let small = projected_planar_importance(
        [-0.10, -0.10, 0.5],
        [0.10, 0.10, 0.5],
        [0.0, 0.0, 1.0, -0.5],
        Mat4::IDENTITY,
    )
    .unwrap();
    assert!(large.0 && small.0);
    assert!(large.1 > small.1);
}

#[test]
fn planar_selection_keeps_large_plane_visible_when_camera_is_over_its_middle() {
    let view_proj = glam::camera::rh::proj::directx::perspective(60.0_f32.to_radians(), 1.0, 0.1, 1000.0);
    let importance = projected_planar_importance(
        [-100.0, -1.0, -100.0],
        [100.0, -1.0, 100.0],
        [0.0, 1.0, 0.0, 1.0],
        view_proj,
    )
    .expect("large plane crossing the camera footprint should remain visible");
    assert!(
        importance.1 > 0.1,
        "unexpectedly tiny screen importance: {importance:?}"
    );
}

#[test]
fn skybox_average_linearises_and_uses_the_smallest_mip() {
    use crate::materials::TextureData;

    // 2x2 base plus a 1x1 mip. Only the 1x1 should be read; if the base is
    // averaged instead the result picks up the black texels.
    let face = TextureData {
        label: "sky".to_string(),
        source: None,
        width: 2,
        height: 2,
        rgba: (vec![
            0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, // base
            188, 188, 188, 255, // 1x1 mip
        ]).into(),
        rgba16f: None,
        mip_level_count: 2,
        clamp: true,
        srgb: true,
    };
    let average = super::average_skybox_color(&[face], Some([0; 6]));
    // 188/255 in sRGB linearises to about 0.5.
    for channel in average {
        assert!(
            (channel - 0.5).abs() < 0.02,
            "expected ~0.5 linear, got {channel}"
        );
    }
}

#[test]
fn skybox_average_weights_cube_face_slots_not_batches() {
    let face = |name: &str, color: [u8; 4]| TextureData {
        label: name.to_owned(),
        source: None,
        width: 1,
        height: 1,
        rgba: (color.to_vec()).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    };
    let images = [
        face("red", [255, 0, 0, 255]),
        face("blue", [0, 0, 255, 255]),
    ];
    // Four skybox faces use red, two use blue. The mean must depend only
    // on those six face slots, not on world or PVS batch multiplicity.
    let average = super::average_skybox_color(&images, Some([0, 1, 0, 0, 1, 0]));
    assert!((average[0] - 2.0 / 3.0).abs() < 0.00001);
    assert!(average[1].abs() < 0.00001);
    assert!((average[2] - 1.0 / 3.0).abs() < 0.00001);
}

#[test]
fn skybox_average_preserves_srgb_transfer_for_mipless_image() {
    let face = TextureData {
        label: "mipless sky".to_owned(),
        source: None,
        width: 2,
        height: 1,
        rgba: (vec![64, 128, 188, 255, 255, 0, 0, 255]).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    };
    let average = super::average_skybox_color(&[face], Some([0; 6]));
    let srgb = |v: u8| {
        let raw = v as f32 / 255.0;
        if raw <= 0.04045 { raw / 12.92 } else { ((raw + 0.055) / 1.055).powf(2.4) }
    };
    for (actual, expected) in average.into_iter().zip([
        (srgb(64) + srgb(255)) / 2.0,
        (srgb(128) + srgb(0)) / 2.0,
        (srgb(188) + srgb(0)) / 2.0,
    ]) {
        assert!((actual - expected).abs() < 0.00001);
    }
}

#[test]
fn skybox_average_uses_only_selected_primary_skybox() {
    use crate::materials::TextureData;
    let image = |label: &str, rgb: [u8; 3]| TextureData {
        label: label.to_owned(),
        source: None,
        width: 1,
        height: 1,
        rgba: (vec![rgb[0], rgb[1], rgb[2], 255]).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    };
    let images = [image("primary", [255, 0, 0]), image("other", [0, 0, 255])];
    let primary = super::average_skybox_color(&images, Some([0; 6]));
    let secondary = super::average_skybox_color(&images, Some([1; 6]));
    assert_eq!(primary, [1.0, 0.0, 0.0]);
    assert_eq!(secondary, [0.0, 0.0, 1.0]);
}

#[test]
fn skybox_average_falls_back_without_faces() {
    let average = super::average_skybox_color(&[], None);
    assert_eq!(average, [0.45, 0.55, 0.75]);
    // An out-of-range face index must not panic or return black.
    let average = super::average_skybox_color(&[], Some([7; 6]));
    assert_eq!(average, [0.45, 0.55, 0.75]);
}

#[test]
fn dynamic_model_depth_phase_places_saber_fx_after_gpu_skinned_opaque_geometry() {
    // Saber glow/core are transient CPU additive surfaces while the default
    // player path is GPU Ghoul2. The player must establish depth first so
    // the blade is occluded only where it is actually behind the body.
    assert!(dynamic_model_writes_depth(DynamicModelAlphaMode::Opaque));
    assert!(dynamic_model_writes_depth(DynamicModelAlphaMode::Mask));
    assert!(dynamic_model_writes_depth(DynamicModelAlphaMode::BlendDepthWrite));
    assert!(dynamic_model_writes_depth(DynamicModelAlphaMode::BlendUnlitDepthWrite));
    assert!(!dynamic_model_writes_depth(DynamicModelAlphaMode::Blend));
    assert!(!dynamic_model_writes_depth(
        DynamicModelAlphaMode::AdditiveOne
    ));
    assert!(!dynamic_model_writes_depth(DynamicModelAlphaMode::Additive));
    assert!(!dynamic_model_writes_depth(
        DynamicModelAlphaMode::DstColorAdd
    ));
    assert!(!dynamic_model_writes_depth(
        DynamicModelAlphaMode::Modulate2x
    ));
    assert!(!dynamic_model_writes_depth(DynamicModelAlphaMode::Darken));

    let phases: Vec<_> = [true, false]
        .into_iter()
        .flat_map(|depth_phase| {
            DYNAMIC_MODEL_ALPHA_ORDER
                .into_iter()
                .filter(move |mode| dynamic_model_writes_depth(*mode) == depth_phase)
        })
        .collect();
    let opaque = phases
        .iter()
        .position(|mode| *mode == DynamicModelAlphaMode::Opaque)
        .unwrap();
    let ghost = phases
        .iter()
        .position(|mode| *mode == DynamicModelAlphaMode::BlendDepthWrite)
        .unwrap();
    let saber = phases
        .iter()
        .position(|mode| *mode == DynamicModelAlphaMode::AdditiveOne)
        .unwrap();
    assert!(opaque < ghost && ghost < saber);
}

/// The occlusion-culling shaders validate on their own, so a failure in an
/// unrelated shader cannot hide a Hi-Z regression.
#[test]
fn hiz_occlusion_shaders_validate() {
    for (name, source) in [
        ("depth_prepass", include_str!("../../depth_prepass.wgsl")),
        ("studio_lens", include_str!("../../studio_lens.wgsl")),
        ("shadow", include_str!("../../shadow.wgsl")),
        ("world_flares", include_str!("../../world_flares.wgsl")),
        ("gpu_cull", include_str!("../../gpu_cull.wgsl")),
        ("hiz_build", include_str!("../../hiz_build.wgsl")),
        ("hiz_reduce", include_str!("../../hiz_reduce.wgsl")),
        ("entity_hiz_probe", include_str!("../../entity_hiz_probe.wgsl")),
    ] {
        let module = wgpu::naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
    }
}

#[test]
fn lamp_receiver_culling_guard_resolves_for_raster_and_rt_shader_variants() {
    // Regression: an undeclared RT-only override in the shared lamp WGSL
    // used to crash Vulkan on startup while creating the raster BSP module.
    for base in [include_str!("../../bsp.wgsl"), include_str!("../../bsp_lean.wgsl")] {
        let raster = compose_world_shader(base);
        assert!(raster.contains(
            "fn lamp_raster_receiver_culling_safe() -> bool { return true; }"
        ));
        assert!(!raster.contains(
            "fn lamp_raster_receiver_culling_safe() -> bool { return !ENABLE_RAY_TRACED_SHADOWS; }"
        ));
        let rt = compose_world_shader(&ray_traced_world_shader_source(base).unwrap());
        assert!(rt.contains(
            "fn lamp_raster_receiver_culling_safe() -> bool { return true; }"
        ));
    }
}

#[test]
fn all_renderer_shaders_validate() {
    let bsp = compose_world_shader(include_str!("../../bsp.wgsl"));
    let bsp_lean = compose_world_shader(include_str!("../../bsp_lean.wgsl"));
    let bsp_fast = format!(
        "{}\n{}\n{}",
        include_str!("../../bsp_fast.wgsl"),
        crate::foliage::surface_shader(),
        include_str!("../../sky_fractal.wgsl")
    );
    let post = weather::with_weather_surface(&weather::with_post_settings(include_str!(
        "../../post.wgsl"
    )));
    let ssr_temporal = weather::with_bound_weather_surface(include_str!("../../ssr_temporal.wgsl"));
    let rain = weather::with_weather_surface(include_str!("../../rain.wgsl"));
    let rain_haze_mask = weather::with_post_settings(include_str!("../../rain_haze_mask.wgsl"));
    let md3 = crate::glass::model_shader(include_str!("../../md3.wgsl"), 2);
    let ghoul2_skin = crate::glass::model_shader(include_str!("../../ghoul2_skin.wgsl"), 3);
    for (name, source) in [
        ("bsp", bsp.as_str()),
        ("bsp_lean", bsp_lean.as_str()),
        ("bsp_fast", bsp_fast.as_str()),
        ("post", post.as_str()),
        ("rain", rain.as_str()),
        ("atmosphere", include_str!("../../atmosphere.wgsl")),
        ("rain_sim", include_str!("../../rain_sim.wgsl")),
        ("wind_visuals", include_str!("../../wind_visuals.wgsl")),
        (
            "rain_glass",
            include_str!("../../weather/rain_glass.wgsl"),
        ),
        ("rain_haze_mask", rain_haze_mask.as_str()),
        ("post_gamma", include_str!("../../post_gamma.wgsl")),
        ("crt_lottes_fast", include_str!("../../crt_lottes_fast.wgsl")),
        ("crt_lottes", include_str!("../../crt_lottes.wgsl")),
        ("depth_prepass", include_str!("../../depth_prepass.wgsl")),
        ("ssr_temporal", ssr_temporal.as_str()),
        ("ssr_visibility", include_str!("../../ssr_visibility.wgsl")),
        (
            "ssr_visibility_msaa",
            include_str!("../../ssr_visibility_msaa.wgsl"),
        ),
        ("gpu_cull", include_str!("../../gpu_cull.wgsl")),
        ("cull_debug", include_str!("../../cull_debug.wgsl")),
        ("hiz_build", include_str!("../../hiz_build.wgsl")),
        ("hiz_reduce", include_str!("../../hiz_reduce.wgsl")),
        ("entity_hiz_probe", include_str!("../../entity_hiz_probe.wgsl")),
        ("cluster_lights", include_str!("../../cluster_lights.wgsl")),
        ("froxel_fog", include_str!("../../froxel_fog.wgsl")),
        ("ocean_windrow", include_str!("../../ocean_windrow.wgsl")),
        ("ocean_interaction", include_str!("../../ocean_interaction.wgsl")),
        ("coastal_compute", include_str!("../../coastal_compute.wgsl")),
        ("ocean_spray", include_str!("../../ocean_spray.wgsl")),
        ("md3", md3.as_str()),
        ("ghoul2_skin", ghoul2_skin.as_str()),
        (
            "ghoul2_skin_compute",
            include_str!("../../ghoul2_skin_compute.wgsl"),
        ),
        ("grass", include_str!("../../grass.wgsl")),
        ("grass_prepared", include_str!("../../grass_prepared.wgsl")),
        (
            "snowflow_deformation_sim",
            include_str!("../../snowflow_deformation_sim.wgsl"),
        ),
    ] {
        let module = wgpu::naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            // grass_prepared unpacks f16 pairs with unpack2x16float.
            wgpu::naga::valid::Capabilities::CUBE_ARRAY_TEXTURES
                | wgpu::naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32,
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
    }

    // RT variants are source-transformed lazily at runtime. Validate the
    // transformed WGSL here too so candidate-intersection alpha handling
    // cannot silently drift from Naga/wgpu's current ray-query grammar.
    for (name, base) in [
        ("bsp_rt", include_str!("../../bsp.wgsl")),
        ("bsp_lean_rt", include_str!("../../bsp_lean.wgsl")),
    ] {
        let transformed = ray_traced_world_shader_source(base)
            .unwrap_or_else(|error| panic!("{name}: RT source transform failed: {error}"));
        let source = compose_world_shader(&transformed);
        let module = wgpu::naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
    }
}

#[test]
fn ocean_interaction_shaders_validate() {
    let bsp = compose_world_shader(include_str!("../../bsp.wgsl"));
    for (name, source) in [("interactive ocean BSP", bsp.as_str()),
        ("interactive ocean compute", include_str!("../../ocean_interaction.wgsl"))] {
        let module = wgpu::naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all()).validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
    }
}

/// Draws one opaque MD3-style triangle through the Entity map pipelines into a
/// real depth target and reads the depth back: proves the light-space pipelines
/// are valid against the entity layouts and that the pass writes caster depth.
#[test]
#[ignore = "requires a Vulkan GPU"]
fn entity_shadow_pass_writes_caster_depth() {
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        println!("entity shadow GPU test: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("entity shadow test"),
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries: &crate::foliage::camera_layout_entries(),
        });
        let (foliage_noise, foliage_sampler) = crate::foliage::test_noise_resources(&device, &queue);
        let white = upload_texture(
            &device,
            &queue,
            &TextureData {
                label: "white".into(),
                source: None,
                width: 1,
                height: 1,
                rgba: (vec![255; 4]).into(),
                rgba16f: None,
                mip_level_count: 1,
                clamp: true,
                srgb: true,
            },
        );
        let mut models = DynamicModelRenderer::new(
            &device,
            &queue,
            &camera_layout,
            &crate::ocean::optics::layout(&device),
            wgpu::TextureFormat::Bgra8UnormSrgb,
            1,
            &white,
        );
        let mut pipeline_jobs = PipelineJobManager::new();
        let vertex = |x: f32, z: f32| DynamicModelVertex { foliage_wind: 0.0,
            position: [x, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            color: [1.0; 4],
            depth_hack: 0.0,
            water_wetness: 0.0,
        };
        let surface = DynamicModelSurface {
            geometry_source: crate::renderer::ModelGeometrySource::Fx,
            authored_lighting: false,
            material_trace: None,
            entity_num: 1,
            wireframe_class: DynamicWireframeClass::Entity,
            raster_visible: true,
            vertices: Arc::new(vec![
                vertex(-100.0, -100.0),
                vertex(100.0, -100.0),
                vertex(0.0, 100.0),
            ]),
            indices: Arc::new(vec![0, 1, 2]),
            lighting_origin: None,
            rt_rigid: None,
            rt_skinned_key: None,
            ghoul2_gpu: None,
            fx_gpu_sprites: None,
            texture: None,
            alpha_mode: DynamicModelAlphaMode::Opaque,

            authored_glow: false,
        };
        models.prepare(
            &mut crate::renderer::brightness::BakedBrightness::default(),
            &mut pipeline_jobs,
            &device,
            &queue,
            &[surface],
            None,
            false,
            false,
            false,
            false,
        );
        assert!(
            models.has_depth_casters(),
            "opaque triangle is a shadow caster"
        );
        for _ in 0..2_000 {
            models.ensure_entity_shadow_pipelines(&mut pipeline_jobs, &device, false, false);
            models.ensure_entity_shadow_pipelines(&mut pipeline_jobs, &device, true, false);
            if models.entity_shadow_pipelines[0].is_some()
                && models.entity_shadow_pipelines[1].is_some()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            models.entity_shadow_pipelines[0].is_some(),
            "forward-Z shadow pipeline compiled"
        );
        assert!(
            models.entity_shadow_pipelines[1].is_some(),
            "reverse-Z shadow pipeline compiled"
        );

        // Light straight down at the origin; the triangle lies in y = 0.
        let view = glam::camera::rh::view::look_at_mat4(Vec3::new(0.0, 1000.0, 0.0), Vec3::ZERO, Vec3::Z);
        let projection = glam::camera::rh::proj::directx::orthographic(-256.0, 256.0, -256.0, 256.0, 0.1, 2000.0);
        let state = EntityShadowState::new(&device, &camera_layout, &foliage_noise, &foliage_sampler);
        state.write_camera(&queue, 0, projection * view);

        for reverse_z in [false, true] {
            const SIZE: u32 = 256;
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("entity shadow test depth"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let depth_view = depth.create_view(&Default::default());
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(SIZE * SIZE * 4),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("entity shadow test pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(if reverse_z { 0.0 } else { 1.0 }),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                models.draw_entity_shadow(&mut pass, &state.cameras[0].1, reverse_z);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(SIZE * 4),
                        rows_per_image: Some(SIZE),
                    },
                },
                wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
            );
            let submission = queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(30)),
                })
                .unwrap();
            rx.recv().unwrap().unwrap();
            let data = readback.slice(..).get_mapped_range().expect("GPU readback range");
            let texels = bytemuck::cast_slice::<u8, f32>(&data);
            let at = |x: u32, y: u32| texels[(y * SIZE + x) as usize];
            let centre = at(SIZE / 2, SIZE / 2);
            let cleared = if reverse_z { 0.0 } else { 1.0 };
            assert_ne!(
                centre, cleared,
                "triangle depth written at the map centre (reverse_z {reverse_z})"
            );
            assert_eq!(
                at(2, 2),
                cleared,
                "uncovered corner keeps the cleared depth (reverse_z {reverse_z})"
            );
            // 1000 units from the eye inside a 0.1..2000 ortho range.
            assert!(
                (centre - 1000.0 / 2000.0).abs() < 0.01,
                "ortho depth of a y = 0 caster, got {centre}"
            );
            drop(data);
        }
        assert!(
            block_on(error_scope.pop()).is_none(),
            "no wgpu validation errors"
        );
    });
}

#[test]
fn blob_shadow_request_projects_onto_a_render_space_floor() {
    // Render space is (x, z, -y). The floor is y = 0 (JKA z = 0), the request
    // sits on it with left = +x, up = -z_render so that left x up = +y (up).
    let big = 500.0;
    let surfaces = jka_assets::bsp::MarkSurfaces::from_world_triangles([
        (
            [[-big, -big, 0.0], [big, -big, 0.0], [big, big, 0.0]],
            [0.0, 0.0, 1.0],
        ),
        (
            [[-big, -big, 0.0], [big, big, 0.0], [-big, big, 0.0]],
            [0.0, 0.0, 1.0],
        ),
    ]);
    let radius = 24.0;
    let (left, up) = (Vec3::new(radius, 0.0, 0.0), Vec3::new(0.0, 0.0, -radius));
    assert_eq!(
        left.cross(up).normalize(),
        Vec3::Y,
        "render-space normal points up"
    );
    let (mut buffer, mut vertices, mut indices) = (
        jka_assets::bsp::MarkBuffer::default(),
        Vec::new(),
        Vec::new(),
    );
    project_blob_shadow_mark(
        &surfaces,
        &mut buffer,
        Vec3::ZERO,
        left,
        up,
        0.5,
        false,
        &mut vertices,
        &mut indices,
    );
    assert!(!indices.is_empty());
    for vertex in &vertices {
        assert!(vertex.position[0].abs() <= 24.01 && vertex.position[2].abs() <= 24.01);
        assert_eq!(vertex.color[3], 0.5, "markShadow fades through vertex alpha");
        assert!(
            (vertex.position[1] - BLOB_MARK_SURFACE_LIFT).abs() < 1.0e-4,
            "lifted along render up"
        );
        assert!((0.0..=1.0).contains(&vertex.uv[0]) && (0.0..=1.0).contains(&vertex.uv[1]));
    }
    // Dynamic-model pipelines cull back faces (CCW front): every triangle must
    // face along the surface normal, whichever winding the source triangles had.
    for tri in indices.chunks_exact(3) {
        let [a, b, c] =
            [tri[0], tri[1], tri[2]].map(|i| Vec3::from_array(vertices[i as usize].position));
        assert!(
            (b - a).cross(c - a).y > 0.0,
            "blob triangle is wound away from the camera"
        );
    }
    let mut area = 0.0;
    for tri in indices.chunks_exact(3) {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| vertices[i as usize].uv);
        area += 0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs();
    }
    assert!(
        (area - 1.0).abs() < 1.0e-3,
        "the square is fully covered, uv area {area}"
    );
}


/// `SkinDraw` is a storage-buffer struct matched to its Rust mirror by layout,
/// not by name, so a field added on one side only silently shifts everything
/// after it - the per-draw colour, the light grid sample, the jiggle offsets.
#[test]
fn ghoul2_draw_uniform_matches_the_skin_draw_wgsl_struct() {
    use crate::weather::rain_layout::{offset_of_member, wgsl_struct_layout};
    use std::mem::{offset_of, size_of};
    for source in [crate::glass::model_shader(include_str!("../../ghoul2_skin.wgsl"), 3),
        include_str!("../../ghoul2_skin_compute.wgsl").to_string()] {
    let (size, members) = wgsl_struct_layout(&source, "SkinDraw");
    assert_eq!(size as usize, size_of::<Ghoul2GpuDrawUniform>());
    for (name, rust_offset) in [
        ("axis0", offset_of!(Ghoul2GpuDrawUniform, axis0)),
        ("color", offset_of!(Ghoul2GpuDrawUniform, color)),
        ("params", offset_of!(Ghoul2GpuDrawUniform, params)),
        (
            "classic_ambient",
            offset_of!(Ghoul2GpuDrawUniform, classic_ambient),
        ),
        ("uv_xform", offset_of!(Ghoul2GpuDrawUniform, uv_xform)),
        ("uv_cross", offset_of!(Ghoul2GpuDrawUniform, uv_cross)),
        ("jiggle3", offset_of!(Ghoul2GpuDrawUniform, jiggle3)),
        ("jiggle4", offset_of!(Ghoul2GpuDrawUniform, jiggle4)),
        (
            "source_phong",
            offset_of!(Ghoul2GpuDrawUniform, source_phong),
        ),
        (
            "source_fresnel",
            offset_of!(Ghoul2GpuDrawUniform, source_fresnel),
        ),
        ("water_bands0", offset_of!(Ghoul2GpuDrawUniform, water_bands0)),
        ("water_bands1", offset_of!(Ghoul2GpuDrawUniform, water_bands1)),
        ("water_base", offset_of!(Ghoul2GpuDrawUniform, water_base)),
    ] {
        assert_eq!(
            offset_of_member(&members, name) as usize,
            rust_offset,
            "{name}"
        );
    }
    }
}

#[test]
fn ocean_material_uniform_matches_world_shader_layouts() {
    use crate::weather::rain_layout::{offset_of_member, wgsl_struct_layout};
    use std::mem::{offset_of, size_of};
    let shaders = [
        compose_world_shader(include_str!("../../bsp.wgsl")),
        compose_world_shader(include_str!("../../bsp_lean.wgsl")),
        format!("{}\n{}\n{}", include_str!("../../bsp_fast.wgsl"), crate::foliage::surface_shader(), include_str!("../../sky_fractal.wgsl")),
    ];
    for shader in shaders {
        let (size, members) = wgsl_struct_layout(&shader, "Material");
        assert_eq!(size as usize, size_of::<MaterialUniform>());
        assert_eq!(offset_of_member(&members, "ocean_color") as usize, offset_of!(MaterialUniform, ocean_color));
        assert_eq!(offset_of_member(&members, "ocean_material") as usize, offset_of!(MaterialUniform, ocean_material));
        assert_eq!(offset_of_member(&members, "ocean_waves") as usize, offset_of!(MaterialUniform, ocean_waves));
        assert_eq!(offset_of_member(&members, "ocean_blend_color") as usize, offset_of!(MaterialUniform, ocean_blend_color));
    }
}

#[test]
#[ignore = "requires JKA_TEST_BASE with stock assets and hothchallenges.pk3"]
fn hothchallenges_water_faces_remain_promotable() {
    let base = std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE"));
    let map = scene::prepare(&base, None, "hothchallenges").expect("prepare hothchallenges");
    for batches in [&map.batches, &map.pvs_batches] {
        let mut heights = std::collections::BTreeSet::new();
        for batch in batches.iter().filter(|b| b.water_primary && is_upward_water_face(b, &map.vertices)) {
            let (height, _, _) = water_surface_extent(batch, &map.vertices);
            heights.insert(height as i32);
            let appearance = crate::ocean::appearance::from_stage(batch, &map.vertices, &map.textures);
            assert!((appearance.color[3] - 0.2).abs() < 0.001, "water2 alphaGen const 0.2");
            assert!(!batch.water_deformed, "water2 only animates UVs");
            assert!(appearance.heuristic.amplitude >= 4.0 && appearance.heuristic.amplitude <= 16.0);
            let factors = crate::ocean::appearance::blend_factors(batch.pipeline.blend);
            assert_eq!(appearance.waves, [1.0, appearance.heuristic.amplitude, factors[0], factors[1]]);
        }
        let tops: std::collections::BTreeSet<_> = batches.iter()
            .filter(|b| b.water_primary && is_upward_water_face(b, &map.vertices))
            .map(|b| { let (h, min, max) = water_surface_extent(b, &map.vertices);
                ocean_surface_key(h, min, max, b.bsp_shader_index) }).collect();
        let undersides = batches.iter().filter(|b| b.water_primary &&
            map.vertices[b.vertices.start as usize..b.vertices.end as usize].iter().all(|v| v.normal[1] < -0.5))
            .collect::<Vec<_>>();
        assert_eq!(undersides.len(), 5);
        for underside in undersides {
            let (h, min, max) = water_surface_extent(underside, &map.vertices);
            assert!(tops.contains(&ocean_surface_key(h, min, max, underside.bsp_shader_index)),
                "each underside must belong to a promoted top and be suppressed");
        }
        assert_eq!(heights, [1216, 2536, 12776, 12784, 14064].into_iter().collect(),
            "all five water planes, including the main pool and its coplanar underside");
    }
}

#[test]
fn ocean_render_uniform_matches_water_and_spray_layouts() {
    use crate::ocean::OceanRenderUniform;
    use crate::weather::rain_layout::{offset_of_member,wgsl_struct_layout};
    use std::mem::{offset_of,size_of};
    for shader in [compose_world_shader(include_str!("../../bsp.wgsl")),
        include_str!("../../coastal_compute.wgsl").to_string(),
        include_str!("../../ocean_spray.wgsl").to_string()] {
        let (size,members) = wgsl_struct_layout(&shader,"OceanRenderSettings");
        assert_eq!(size as usize,size_of::<OceanRenderUniform>());
        for (name,offset) in [
            ("map_scales",offset_of!(OceanRenderUniform,map_scales)),
            ("spray_planes",offset_of!(OceanRenderUniform,spray_planes)),
            ("swell",offset_of!(OceanRenderUniform,swell)),
            ("authored_plane",offset_of!(OceanRenderUniform,authored_plane)),
            ("mask_ranges",offset_of!(OceanRenderUniform,mask_ranges)),
            ("mask_bounds",offset_of!(OceanRenderUniform,mask_bounds)),
            ("mask_tiles",offset_of!(OceanRenderUniform,mask_tiles)),
            ("mask_triangles",offset_of!(OceanRenderUniform,mask_triangles))] {
            assert_eq!(offset_of_member(&members,name) as usize,offset,"{name}");
        }
    }
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn promoted_water_gpu_views_and_spray_validate() {
    use crate::ocean::{appearance::{PromotedOcean,wave_heuristic},OceanGpu,OceanSettings,OceanSurface};
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends:wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device,queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits:adapter.limits(),..Default::default()
        }).await.unwrap();
        println!("promoted water GPU test: {:?}",adapter.get_info());
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:None,entries:&[wgpu::BindGroupLayoutEntry { binding:0,
                visibility:wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT,
                ty:wgpu::BindingType::Buffer { ty:wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset:false,min_binding_size:None },count:None }]
        });
        let albedo = upload_texture(&device,&queue,&TextureData { label:"spray test".into(),
            source:None,width:1,height:1,rgba: (vec![255;4]).into(),rgba16f: None,mip_level_count:1,
            clamp:true,srgb:true });
        let layout = OceanGpu::create_render_layout(&device);
        let _spray = crate::ocean::create_spray_pipeline(&device,&camera_layout,&layout,
            wgpu::TextureFormat::Bgra8UnormSrgb,1);
        let settings = OceanSettings { map_size:128,..Default::default() };
        let mut ocean = OceanGpu::new(&device,&queue,&layout,&albedo.view,settings);
        let pools: Vec<_> = (0..2).map(|i| PromotedOcean { index:i,name:format!("pool{i}"),
            surface:OceanSurface { plane_height:100.0+i as f32*200.0,
                minimum:[0.0;2],maximum:[4096.0;2] },heuristic:wave_heuristic(i as f32,None),
            color:[0.1,0.2,0.3],roughness:None,source_deformation: None, source_alpha: None,
            swell_direction:None }).collect();
        let surfaces: Vec<_> = pools.iter().map(|p| p.surface).collect();
        ocean.set_enabled(&queue,true);
        ocean.set_surfaces(&queue,&surfaces);
        ocean.set_promoted_surfaces(&device,&queue,&pools,&[]);
        let mut local = pools[0].settings(settings);
        local.authored.amplitude = 64.0;
        ocean.set_promoted_surfaces(&device,&queue,&pools,&[(0,local)]);
        let mut encoder = device.create_command_encoder(&Default::default());
        ocean.encode_frame(&queue,&mut encoder,1.0/60.0);
        let submission = queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::Wait { submission_index:Some(submission),
            timeout:Some(std::time::Duration::from_secs(30)) }).unwrap();
        // Removing a pool or changing quality must release/rebuild just its
        // render view while retaining valid references to the shared FFT field.
        ocean.set_promoted_surfaces(&device,&queue,&pools[1..],&[]);
        assert!(block_on(scope.pop()).is_none(),"ocean views and spray pipeline must validate");
    });
}

#[test]
#[ignore = "requires JKA_TEST_BASE with racepack8 assets"]
fn racepack8_water_uses_analyzed_color_and_screen_blend() {
    let base = std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE"));
    let map = scene::prepare(&base,None,"racepack8").expect("prepare racepack8");
    for batches in [&map.batches,&map.pvs_batches] {
        let mut count = 0;
        for batch in batches.iter().filter(|b| b.water_primary && is_upward_water_face(b,&map.vertices)) {
            let Some(texture) = batch.texture.and_then(|i| map.textures.get(i)) else {continue};
            if !texture.label.contains("rp8_water") {continue;}
            let appearance = crate::ocean::appearance::from_stage(batch,&map.vertices,&map.textures);
            assert_eq!(appearance.waves[2..],[8.0,1.0],"GL_ONE_MINUS_DST_COLOR GL_ONE");
            assert_eq!(batch.rgb_gen,RgbGen::Vertex);
            assert!(appearance.blend_color.iter().all(|v| v.is_finite()));
            let mut different_uvs = map.vertices[batch.vertices.start as usize..batch.vertices.end as usize].to_vec();
            for vertex in &mut different_uvs {vertex.uv = [0.0;2];}
            let mut local_batch = batch.clone();
            local_batch.vertices = 0..different_uvs.len() as u32;
            let no_uv = crate::ocean::appearance::from_stage(&local_batch,&different_uvs,&map.textures);
            assert_eq!(appearance.blend_color,no_uv.blend_color,"color analysis is independent of source mapping");
            assert_eq!(appearance.heuristic,no_uv.heuristic);
            count += 1;
        }
        assert!(count > 0,"actual rp8_water tops must remain promotable");
    }
}

/// Compare the actual ocean blend helpers against hardware blending on varied
/// backgrounds and painted detail, including JPEG-style alpha=1. This catches
/// opaque replacement regressions without depending on a screenshot comparison.
#[test]
#[ignore = "requires a Vulkan GPU"]
fn promoted_water_blends_match_authored_hardware_blending() {
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends:wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device,queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits:adapter.limits(),..Default::default()
        }).await.unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bsp = include_str!("../../bsp.wgsl");
        let begin = bsp.find("fn ocean_authored_blend_factor").unwrap();
        let end = bsp.find("fn ocean_surface_sample").unwrap();
        let helpers = &bsp[begin..end];
        use crate::materials::BlendFactor::*;
        for (src,dst) in [(OneMinusDstColor,One),(SrcAlpha,OneMinusSrcAlpha),
            (One,One),(DstColor,Zero),(DstColor,SrcColor),(One,OneMinusSrcAlpha)] {
            let packed = crate::ocean::appearance::blend_factors(BlendMode::Custom(src,dst));
            let body = r#"
@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    var points = array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    return vec4<f32>(points[i],0.0,1.0);
}
fn background(p:vec4<f32>)->vec4<f32> {
    return vec4<f32>(0.11+0.5*f32(u32(p.x)%2u),0.24,0.3,1.0);
}
fn painted_water(p:vec4<f32>)->vec4<f32> {
    let detail = 0.3+0.3*f32(u32(p.x)%3u);
    let alpha = f32(u32(p.x)%3u)*0.5;
    return vec4<f32>(vec3<f32>(detail,detail*0.8,0.7)*vec3<f32>(0.2,0.3,0.4),alpha);
}
@fragment fn bg(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {return background(p);}
@fragment fn original(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {return painted_water(p);}
@fragment fn promoted(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {
    return vec4<f32>(ocean_authored_blend(painted_water(p),background(p),BLEND_FACTORS),1.0);
}
"#;
            let source = format!("{helpers}\n{body}").replace("BLEND_FACTORS",
                &format!("vec2<u32>({}u,{}u)",packed[0] as u32-1,packed[1] as u32));
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label:Some("authored water blend comparison"),source:wgpu::ShaderSource::Wgsl(source.into()) });
            let pipeline = |entry:&str,blend| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label:None,layout:None,vertex:wgpu::VertexState {module:&shader,entry_point:Some("vs"),
                    compilation_options:Default::default(),buffers:&[]},primitive:Default::default(),
                depth_stencil:None,multisample:Default::default(),fragment:Some(wgpu::FragmentState {
                    module:&shader,entry_point:Some(entry),compilation_options:Default::default(),
                    targets:&[Some(wgpu::ColorTargetState {format:wgpu::TextureFormat::Rgba8Unorm,
                        blend,write_mask:wgpu::ColorWrites::ALL})] }),multiview_mask:None,cache:None });
            let bg = pipeline("bg",None);
            let component = wgpu::BlendComponent {src_factor:gpu_blend_factor(src),dst_factor:gpu_blend_factor(dst),
                operation:wgpu::BlendOperation::Add};
            let original = pipeline("original",Some(wgpu::BlendState {color:component,alpha:component}));
            let promoted = pipeline("promoted",None);
            let texture = || device.create_texture(&wgpu::TextureDescriptor {label:None,
                size:wgpu::Extent3d {width:8,height:1,depth_or_array_layers:1},mip_level_count:1,sample_count:1,
                dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba8Unorm,
                usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[] });
            let reference = texture(); let actual = texture();
            let readback = device.create_buffer(&wgpu::BufferDescriptor {label:None,size:512,
                usage:wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::MAP_READ,mapped_at_creation:false});
            let mut encoder = device.create_command_encoder(&Default::default());
            for (texture,passes,offset) in [(&reference,vec![&bg,&original],0),(&actual,vec![&promoted],256)] {
                let view = texture.create_view(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {label:None,
                        color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:&view,depth_slice:None,
                            resolve_target:None,ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store:wgpu::StoreOp::Store}})],..Default::default()});
                    for pipeline in passes {pass.set_pipeline(pipeline);pass.draw(0..3,0..1);}
                }
                encoder.copy_texture_to_buffer(wgpu::TexelCopyTextureInfo {texture,mip_level:0,
                    origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},wgpu::TexelCopyBufferInfo {
                    buffer:&readback,layout:wgpu::TexelCopyBufferLayout {offset,bytes_per_row:Some(256),rows_per_image:Some(1)}},
                    wgpu::Extent3d {width:8,height:1,depth_or_array_layers:1});
            }
            let submission = queue.submit([encoder.finish()]);
            let (tx,rx) = std::sync::mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read,move |r| tx.send(r).unwrap());
            device.poll(wgpu::PollType::Wait {submission_index:Some(submission),
                timeout:Some(std::time::Duration::from_secs(30))}).unwrap();
            rx.recv().unwrap().unwrap();
            let pixels = readback.slice(..).get_mapped_range().expect("GPU readback range");
            for x in 0..8 {for channel in 0..3 {
                let expected = pixels[x*4+channel]; let actual = pixels[256+x*4+channel];
                assert!(expected.abs_diff(actual)<=2,"{src:?}/{dst:?}: pixel {x}, channel {channel}: {actual} vs hardware {expected}");
            }}
            assert_ne!(pixels[256],pixels[260],"background/detail must survive promotion");
        }
        assert!(block_on(scope.pop()).is_none(),"water blend shader must validate");
    });
}

/// The exterior sky extension and the analytic tail must never replace the
/// captured transmission with raw fog RGB. The A/B regression produced
/// a hard bright-water/dark-water boundary at the original BSP footprint.
#[test]
fn ocean_sky_extension_does_not_force_raw_fog_transmission() {
    let shader = include_str!("../../bsp.wgsl");
    let begin = shader.find("fn ocean_surface_sample(").unwrap();
    let end = shader[begin..].find("fn shade_surface(").unwrap() + begin;
    let surface = &shader[begin..end];
    assert!(surface.contains("let transmission = textureSampleLevel(ocean_scene,"));
    assert!(!surface.contains("transmission = ocean_optics.fog.rgb"),
        "ocean exterior must not be painted with unattenuated fog color");
    assert!(shader.contains("let shaded = shade_surface(surface, eye.y >= sea_height);"),
        "analytic ocean must reuse the geometric ocean surface shader");
}

/// The analytic continuation keeps its radius separate from dynamic LOD
/// fields and draws after the original geometric clipmaps.
#[test]
fn ocean_infinite_tail_uses_reserved_radius_and_draws_after_mesh() {
    let ocean_source = include_str!("../../ocean.rs");
    let shader = include_str!("../../bsp.wgsl");
    let draws = include_str!("../frame/draw.rs");
    assert!(ocean_source.contains("local.appearance_c[3]=self.promoted_far_radii"));
    assert!(!ocean_source.contains("local.clipmap[2]=self.promoted_far_radii"));
    assert!(shader.contains("let radius = ocean.appearance_c.w;"));
    let start = draws.find("label: Some(\"JKA ocean surface and diagnostics\")")
        .expect("ocean pass");
    let pass = &draws[start..];
    let mesh = pass.find("let mut ocean_drawn = 0u8;").expect("geometric ocean draw");
    let tail = pass.find("if self.ocean_sky_infinite {").expect("analytic ocean draw");
    assert!(mesh < tail, "geometric water must draw before the analytic far tail");
}

#[test]
fn ocean_shading_does_not_sample_the_authored_base_texture() {
    let source = compose_world_shader(include_str!("../../bsp.wgsl"));
    let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap();
    let base = module.global_variables.iter().find(|(_,g)| g.name.as_deref()==Some("base_texture")).unwrap().0;
    let root = module.functions.iter().find(|(_,f)| f.name.as_deref()==Some("ocean_surface_sample")).unwrap().0;
    let mut pending = vec![root];
    let mut visited = std::collections::HashSet::new();
    while let Some(handle) = pending.pop() {
        if !visited.insert(handle) {continue;}
        let function = &module.functions[handle];
        for (_,expression) in function.expressions.iter() {
            match expression {
                wgpu::naga::Expression::GlobalVariable(global) => assert_ne!(*global,base,
                    "promoted water must use analyzed color and FFT detail, not source image sampling ({:?})",function.name),
                wgpu::naga::Expression::CallResult(callee) => pending.push(*callee),
                _ => {}
            }
        }
    }
}

#[test]
fn additive_encoded_stage_wave_is_evaluated_before_srgb_decode() {
    // TaystJK rd-vanilla reads byte vertex RGB from RB_CalcWaveColor and
    // multiplies it with the encoded GL texture. wgpu textures are already
    // decoded to linear; mixing the order produces a much stronger contribution.
    fn decode(v: f32) -> f32 {
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    }
    let encoded_texel: f32 = 0.5;
    let wave = (0.2_f32 * 255.0).round() / 255.0;
    let expected_linear = decode(encoded_texel * wave);
    let incorrect_linear = decode(encoded_texel) * wave;
    assert!((expected_linear - decode(0.1)).abs() < 1.0e-6);
    assert!(expected_linear < incorrect_linear * 0.6,
        "legacy encoded modulation must precede decode");
}

#[test]
fn additive_parity_has_no_runtime_debug_mode() {
    assert!(!include_str!("../../ui/settings/video.rs").contains("additive_blend_debug"));
    assert!(!include_str!("../api.rs").contains("SetAdditiveBlendDebug"));
    assert!(!include_str!("../../app/commands/registry/renderer_cvars.rs").contains("r_additiveBlendDebug"));
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn specialized_post_pipelines_validate() {
    // r_postSpecialize compiles fs_main with POST_FEATURES masks. Every mask,
    // including "nothing", must survive naga override processing and backend
    // compilation exactly like the general pipeline.
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, _queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }).await.unwrap();
        let source = crate::weather::with_weather_surface(&crate::weather::with_post_settings(
            include_str!("../../post.wgsl"),
        ));
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post specialization test"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        for features in [Some(0), Some(256 | 512), Some(u32::MAX), None] {
            let constants = features.map(|mask| [("POST_FEATURES", f64::from(mask))]);
            let _ = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("post specialization test"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: constants.as_ref().map_or(&[][..], |c| &c[..]),
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::TextureFormat::Bgra8UnormSrgb.into())],
                }),
                multiview_mask: None,
                cache: None,
            });
        }
        assert!(block_on(scope.pop()).is_none(), "specialized post pipelines must validate");
    });
}
