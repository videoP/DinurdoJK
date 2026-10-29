use super::*;
use crate::renderer::*;
use glam::Vec3Swizzles;
use jka_movement::TraceWorld;

#[test]
#[ignore = "diagnostic: existing retail map, no window, no bake or disk cache"]
fn reported_ffa3_ground_materials_and_shadow_weights() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/base");
    let mut map = scene::prepare(&root, None, "mp/ffa3").unwrap();
    let mut opaque = Vec::new();
    for batch in &map.batches {
        if batch.pipeline.class != DrawClass::Opaque
            || batch.pipeline.depth_equal
            || batch.alpha_cutoff != 0.0
        {
            continue;
        }
        for v in
            map.vertices[batch.vertices.start as usize..batch.vertices.end as usize].chunks_exact(3)
        {
            opaque.push(StaticAoRenderTriangle {
                positions: [v[0].position, v[1].position, v[2].position],
            });
        }
    }
    let bvh = StaticAoBvh::build(Arc::new(opaque)).unwrap();
    for (eye, yaw, pitch) in [
        ([1707.0, 1067.0, 77.0], 178.0f32, 12.0f32),
        ([1632.0, 894.0, 35.0], 182.0, 49.0),
        ([1144.0, 1690.0, 337.0], 344.0, 40.0),
    ] {
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        let actor = Vec3::from_array(scene::render_position(eye))
            + Vec3::from_array(scene::render_position([cp * cy, cp * sy, -sp])) * 85.0
            - Vec3::Y * 53.0;
        let ground = map
            .collision
            .as_mut()
            .unwrap()
            .trace(jka_movement::TraceQuery {
                start: scene::jka_position((actor + Vec3::Y * 32.0).to_array()),
                end: scene::jka_position((actor - Vec3::Y * 512.0).to_array()),
                mins: [0.0; 3],
                maxs: [0.0; 3],
                pass_entity: 0,
                mask: 1,
            });
        assert!(ground.fraction < 1.0, "ground near {eye:?}");
        let p = Vec3::from_array(scene::render_position(ground.end));
        let n = Vec3::from_array(scene::render_position(ground.normal));
        println!(
            "VIEW {eye:?}: approximate actor {:?}, ground {:?}, normal {:?}",
            scene::jka_position(actor.to_array()),
            ground.end,
            ground.normal
        );
        let mut names = std::collections::BTreeSet::new();
        for batch in &map.batches {
            let touches = map.vertices[batch.vertices.start as usize..batch.vertices.end as usize]
                .chunks_exact(3)
                .any(|v| {
                    let a = Vec3::from_array(v[0].position);
                    let b = Vec3::from_array(v[1].position);
                    let c = Vec3::from_array(v[2].position);
                    let u = (b - a).xz();
                    let w = (c - a).xz();
                    let d = (p - a).xz();
                    let det = u.perp_dot(w);
                    if det.abs() < 0.001 {
                        return false;
                    }
                    let x = d.perp_dot(w) / det;
                    let y = u.perp_dot(d) / det;
                    x >= -0.001
                        && y >= -0.001
                        && x + y <= 1.001
                        && (a.y + x * (b.y - a.y) + y * (c.y - a.y) - p.y).abs() < 4.0
                });
            if !touches {
                continue;
            }
            let name = map
                .material_debug
                .entries
                .get(batch.material_debug_index as usize)
                .map_or("?", |e| e.name.as_str());
            let uniform = material_uniform(batch, false);
            names.insert(format!("{name}: class={:?}, blend={:?}, tcgen={}, flags={:#x}, explicitLM={}, implicitLM={}, texture={}",batch.pipeline.class,batch.pipeline.blend,uniform.header[0],uniform.header[2],batch.texture_is_lightmap,batch.modulate_lightmap,batch.texture.and_then(|i|map.textures.get(i)).map_or("$lightmap/white",|t|t.label.as_str())));
        }
        assert!(!names.is_empty(), "rendered ground identified");
        for name in names {
            println!("  RECEIVER {name}");
        }
        let mut lamps: Vec<_> = map
            .static_bake_lights
            .iter()
            .copied()
            .filter(|l| {
                super::super::lightmap_local::source_enabled(l, super::super::lightmap_local::ALL)
                    && inferred_lamp_in_player_pvs(map.visibility.as_ref(), *l, actor)
            })
            .map(|l| (local_shadow_importance(l, actor, false), l))
            .filter(|(s, _)| *s > 0.0)
            .collect();
        lamps.sort_by(|a, b| b.0.total_cmp(&a.0));
        lamps.truncate(4);
        let mut total = 0.0;
        let mut visible = 0.0;
        for (_, l) in lamps {
            let d = Vec3::from_array(l.position) - p;
            let d2 = d.length_squared();
            let to = d.normalize_or_zero();
            let e = Vec3::from_array(l.emitter_normal);
            let cosine = e.normalize_or_zero().dot(-to);
            let emission = if e.length_squared() < 1e-6 {
                1.0
            } else if l.emitter_two_sided {
                cosine.abs()
            } else {
                cosine.max(0.0)
            };
            let window = (1.0 - d2 / l.radius.powi(2)).max(0.0);
            let weight =
                l.shadow_power * n.dot(to).max(0.0) * emission * window * window / (d2 + 256.0);
            let clear = bvh
                .trace(p + n * 1.0, to, (d.length() - 8.0).max(0.0))
                .is_none();
            total += weight;
            if clear {
                visible += weight;
            }
            println!(
                "  LAMP {:?}: weight={weight:.6}, opaque_only_LOS={clear}",
                scene::jka_position(l.position)
            );
        }
        println!("  total={total:.6}, visible={visible:.6}, old_all-lamps-blocked darkening={:.3} (opaque LOS diagnostic excludes cutouts)",0.65*visible/(total+(total*0.25).max(0.02)));
    }
}

#[test]
#[ignore = "requires existing retail assets; no window or lightmap bake"]
fn original_lamp_shadow_sources_cover_reported_ffa3_positions() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/base");
    let map = scene::prepare(&root, None, "mp/ffa3").unwrap();
    for (eye, yaw, pitch) in [
        ([1707.0, 1067.0, 77.0], 178.0f32, 12.0f32),
        ([1632.0, 894.0, 35.0], 182.0, 49.0),
        ([1144.0, 1690.0, 337.0], 344.0, 40.0),
    ] {
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        // Reverse the ordinary 85-unit third-person camera offset to rank at
        // the actor, not at a camera that can be in another lamp's influence.
        let actor = Vec3::from_array(scene::render_position(eye))
            + Vec3::from_array(scene::render_position([cp * cy, cp * sy, -sp])) * 85.0
            - Vec3::Y * 53.0;
        let mut ranked: Vec<_> = map
            .static_bake_lights
            .iter()
            .copied()
            .filter(|light| {
                super::super::lightmap_local::source_enabled(
                    light,
                    super::super::lightmap_local::ALL,
                ) && inferred_lamp_in_player_pvs(map.visibility.as_ref(), *light, actor)
            })
            .map(|light| (local_shadow_importance(light, actor, false), light))
            .filter(|(score, _)| *score > 0.0)
            .collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        ranked.truncate(4);
        assert!(!ranked.is_empty(), "no inferred shadow source at {eye:?}");
        println!(
            "view {eye:?}, actor {:?}",
            scene::jka_position(actor.to_array())
        );
        for (score, light) in &ranked {
            println!(
                "  lamp {:?}, distance {:.1}, score {:.3}, power {:.1}, mask {}",
                scene::jka_position(light.position),
                actor.distance(Vec3::from_array(light.position)),
                score,
                light.shadow_power,
                light.shader_promotion_mask
            );
        }
        assert!(
            ranked[0].1.shadow_power > 0.0,
            "rank uses actual fixture power"
        );
        assert!(
            actor.distance(Vec3::from_array(ranked[0].1.position)) < ranked[0].1.radius,
            "strongest lamp reaches actor"
        );
    }
}

#[test]
fn cubemap_projection_agrees_with_cube_lookup_off_axis_on_every_face() {
    let matrices = local_shadow_face_matrices(Vec3::ZERO, 512.0);
    let probes = [
        Vec3::new(64.0, -24.0, 16.0),
        Vec3::new(-64.0, -24.0, -16.0),
        Vec3::new(16.0, 64.0, -24.0),
        Vec3::new(-16.0, -64.0, 24.0),
        Vec3::new(16.0, -24.0, 64.0),
        Vec3::new(-16.0, 24.0, -64.0),
    ];
    for (face, p) in probes.into_iter().enumerate() {
        let a = p.abs();
        // Cube-map face coordinates, including V, in texture coordinates.
        let (s, t, major) = match face {
            0 => (-p.z, -p.y, a.x),
            1 => (p.z, -p.y, a.x),
            2 => (p.x, p.z, a.y),
            3 => (p.x, -p.z, a.y),
            4 => (p.x, -p.y, a.z),
            _ => (-p.x, -p.y, a.z),
        };
        let ndc = matrices[face].project_point3(p);
        let actual = glam::Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let expected = glam::Vec2::new(s / major, t / major) * 0.5 + glam::Vec2::splat(0.5);
        assert!(
            actual.distance(expected) < 1e-5,
            "face {face}: captured {actual:?}, lookup {expected:?}"
        );
    }
}

#[test]
#[ignore = "requires a Vulkan GPU; offscreen, does not create a game window"]
fn local_cubemap_captured_entity_blocks_baked_light_on_all_faces_on_gpu() {
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries: &crate::foliage::camera_layout_entries(),
        });
        let (foliage_noise, foliage_sampler) = crate::foliage::test_noise_resources(&device, &queue);
        let white = upload_texture(
            &device,
            &queue,
            &TextureData {
                label: "local shadow test white".into(),
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
        let mut jobs = PipelineJobManager::new();
        let centers = [
            Vec3::new(64.0, -24.0, 16.0),
            Vec3::new(-64.0, -24.0, -16.0),
            Vec3::new(16.0, 64.0, -24.0),
            Vec3::new(-16.0, -64.0, 24.0),
            Vec3::new(16.0, -24.0, 64.0),
            Vec3::new(-16.0, 24.0, -64.0),
        ]
        .map(|p| p * 4.0);
        let mut vertices = Vec::new();
        for (face, center) in centers.into_iter().enumerate() {
            let (u, v) = match face / 2 {
                0 => (Vec3::Y, Vec3::Z),
                1 => (Vec3::X, Vec3::Z),
                _ => (Vec3::X, Vec3::Y),
            };
            for p in [
                center + u * 10.0 + v * 10.0,
                center - u * 10.0 + v * 10.0,
                center - v * 16.0,
            ] {
                vertices.push(DynamicModelVertex { foliage_wind: 0.0,
                    position: p.to_array(),
                    normal: Vec3::Y.to_array(),
                    uv: [0.0; 2],
                    color: [1.0; 4],
                    depth_hack: 0.0,
                    water_wetness: 0.0,
                });
            }
        }
        let surface = DynamicModelSurface {
            geometry_source: crate::renderer::ModelGeometrySource::Fx,
            authored_lighting: false,
            material_trace: None,
            entity_num: 1,
            wireframe_class: DynamicWireframeClass::Entity,
            raster_visible: true,
            indices: Arc::new((0..vertices.len() as u32).collect()),
            vertices: Arc::new(vertices),
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
            &mut jobs,
            &device,
            &queue,
            &[surface],
            None,
            false,
            false,
            false,
            false,
        );
        for _ in 0..2000 {
            models.ensure_entity_shadow_pipelines(&mut jobs, &device, false, false);
            if models.entity_shadow_pipelines[0].is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(models.entity_shadow_pipelines[0].is_some());
        const SIZE: u32 = 128;
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen moving local cubes"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 96,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let cube = depth.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::CubeArray),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let local = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::bytes_of(&0.4f32),
        );
        let local_view = local.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let points: Vec<[f32; 4]> = centers
            .into_iter()
            .flat_map(|p| {
                let mirrored =
                    if p.x.abs() > p.y.abs() && p.x.abs() > p.z.abs() || p.z.abs() > p.y.abs() {
                        Vec3::new(p.x, -p.y, p.z)
                    } else {
                        Vec3::new(p.x, p.y, -p.z)
                    };
                [
                    (p * 1.0625).extend(0.0).to_array(),
                    (mirrored * 1.0625).extend(0.0).to_array(),
                ]
            })
            .collect();
        let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&points),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 120 * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 120 * 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let world = include_str!("../../bsp.wgsl");
        let sample =
            crate::renderer::rt_models::declaration(world, "fn sample_local_shadow_visibility(")
                .unwrap();
        let moving =
            crate::renderer::rt_models::declaration(world, "fn baked_local_moving_visibility(")
                .unwrap();
        let maybe =
            crate::renderer::rt_models::declaration(world, "fn baked_local_moving_maybe(").unwrap();
        let source = format!(
            r#"
const ENABLE_LOCAL_SHADOWS:bool=true;const LOCAL_SHADOW_NEAR:f32=4.0;
struct PointLight {{position_radius:vec4<f32>,color_intensity:vec4<f32>,emitter:vec4<f32>,shape:vec4<f32>,shadow:vec4<f32>}}
struct Settings {{local_shadows:vec4<u32>,baked_sun_debug_mode:vec4<u32>,baked_indirect_curve:vec4<f32>,values:vec4<u32>}}
struct VertexOut {{shell_kind:u32,lightmap_uv:vec2<f32>,world_position:vec3<f32>,world_normal:vec3<f32>}}
var<private> lighting_settings:Settings;var<private> dynamic_lights:array<PointLight,2>;
@group(0) @binding(0) var local_shadow_texture:texture_depth_cube_array;
@group(0) @binding(1) var local_shadow_sampler:sampler_comparison;
@group(0) @binding(2) var baked_local_texture:texture_2d_array<f32>;
@group(0) @binding(3) var<storage,read> points:array<vec4<f32>>;
@group(0) @binding(4) var<storage,read_write> output:array<vec4<f32>>;
fn material_flags()->u32 {{return lighting_settings.values.x;}} fn material_tc_gen()->u32 {{return 0u;}}
fn dynamic_light_debug_enabled()->bool {{return false;}}
{sample}
{maybe}
{moving}
fn lamp_raster_receiver_culling_safe()->bool {{return true;}}
{helpers}
@compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id:vec3<u32>) {{
    lighting_settings.local_shadows=vec4(1u,1u,128u,0u);
    let mode=id.x/12u;
    lighting_settings.values.x=select(2u,131073u,mode==3u);
    lighting_settings.baked_sun_debug_mode=vec4(0u,1u,select(2u,4u,mode>0u),1u);
    lighting_settings.baked_indirect_curve=vec4(1.0,1.0,select(1.0,513.0,mode==6u || mode==7u),0.0);
    dynamic_lights[0].position_radius=vec4(0.0,0.0,0.0,512.0);
    dynamic_lights[0].color_intensity=vec4(1.0,0.0,0.0,1.0);
    var power=select(4096.0,409.6,mode==4u);
    if mode==8u {{power=409600.0;}} else if mode==9u {{power=0.0;}}
    dynamic_lights[0].shape=vec4(-64.0,power,0.0,select(1.0,-1.0,mode>0u));dynamic_lights[0].shadow.x=select(1.0,2.0,mode==2u);
    // A strong unrelated lamp is either behind a wall (6) or not blocked by
    // the actor (7). Neither may dilute the first lamp's shadow.
    dynamic_lights[1]=dynamic_lights[0];
    dynamic_lights[1].shape.y=409600.0;
    dynamic_lights[1].shadow.x=select(2.0,3.0,mode==7u);
    var receiver:VertexOut;receiver.lightmap_uv=vec2(0.5);receiver.world_position=points[id.x%12u].xyz;receiver.world_normal=normalize(-receiver.world_position);
    if mode==5u {{receiver.world_position*=1.5;}}
    let visibility=baked_local_moving_visibility(dynamic_lights[0],receiver.world_position,receiver.world_normal);
    output[id.x]=vec4(shade_baked_local(vec3(0.7,0.5,0.3),receiver),visibility);
}}
"#,
            helpers = include_str!("lightmap_local.wgsl")
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("actual cubemap lookup and baked correction"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&cube),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&local_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let cameras = crate::renderer::lighting::entity_shadows::EntityShadowState::with_count(
            &device,
            &camera_layout,
            6, &foliage_noise, &foliage_sampler,
        );
        let entities = std::collections::HashSet::from([1]);
        // Test the old and corrected projections against real cubemap sampling.
        for correct in [false, true] {
            let mut encoder = device.create_command_encoder(&Default::default());
            // Static cube 0 is open. Cubes 1 and 9 are blocked, allowing the
            // third group of probes to verify static-wall rejection explicitly.
            for (cube_index, clear) in [(0, 1.0), (1, 0.0), (2, 1.0), (9, 0.0), (10, 1.0)] {
                for face in 0..6 {
                    let view = depth.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_array_layer: cube_index * 6 + face,
                        array_layer_count: Some(1),
                        ..Default::default()
                    });
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("static visibility regression setup"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(clear),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
            }
            for (face, matrix) in local_shadow_face_matrices(Vec3::ZERO, 512.0)
                .into_iter()
                .enumerate()
            {
                let matrix = if correct {
                    matrix
                } else {
                    Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0)) * matrix
                };
                cameras.write_camera(&queue, face, matrix);
                let view = depth.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: 48 + face as u32,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("actual culled entity shadow capture"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                models.draw_local_entity_shadow(&mut pass, &cameras.cameras[face].1, &entities);
            }
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(120, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &read, 0, 120 * 16);
            queue.submit([encoder.finish()]);
            read.slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let bytes = read.slice(..).get_mapped_range().unwrap();
            for i in 0..120 {
                let at = |channel: usize| {
                    f32::from_le_bytes(
                        bytes[i * 16 + channel * 4..i * 16 + channel * 4 + 4]
                            .try_into()
                            .unwrap(),
                    )
                };
                let shadowed = (i % 2 == 0) == correct;
                if i >= 12 {
                    let factor = at(0) / 0.7;
                    let mode = i / 12;
                    if mode >= 4 {
                        let reference_at = (12 + i % 12) * 16;
                        let reference = f32::from_le_bytes(
                            bytes[reference_at..reference_at + 4].try_into().unwrap(),
                        ) / 0.7;
                        if shadowed {
                            match mode {
                                4 => assert!(
                                    ((1.0-factor) / (1.0-reference) - 0.1).abs() < 0.001,
                                    "tenfold dimmer lamp removes tenfold less light"
                                ),
                                5 => assert!(
                                    factor > reference + 0.001 && factor < 1.0,
                                    "same occluder casts a weaker shadow on a farther receiver"
                                ),
                                6 | 7 => assert!(
                                    (factor-reference).abs() < 1e-5,
                                    "hidden or unblocked competing lamps cannot dilute a shadow"
                                ),
                                8 => assert!((factor-0.48).abs() < 1e-5,
                                    "overestimated source energy retains the original residual floor"),
                                _ => {}
                            }
                        }
                    }
                    assert!(
                        factor >= 0.48 - 1e-5 && factor <= 1.0 + 1e-5,
                        "original ambient floor survives"
                    );
                    assert!(
                        (at(1) / 0.5 - factor).abs() < 1e-5 && (at(2) / 0.3 - factor).abs() < 1e-5,
                        "original hue survives"
                    );
                    if mode == 2 || mode == 9 || !shadowed {
                        assert!((factor-1.0).abs()<1e-5,"unshadowed or statically blocked lamps leave original lighting unchanged");
                    } else {
                        assert!(factor<0.99999,"moving entity shadows original lighting without an additive bake: mode={mode}, factor={factor}");
                    }
                    continue;
                }
                assert!(
                    if shadowed {
                        at(3) <= 0.5
                    } else {
                        (at(3) - 1.0).abs() < 0.01
                    },
                    "correct={correct}, face {}, mirrored={}, visibility={}",
                    i / 2,
                    i % 2 == 1,
                    at(3)
                );
                assert!(
                    (at(0) - (0.7 - 0.4 * (1.0 - at(3)))).abs() < 0.01,
                    "baked red source is removed only behind the caster"
                );
                assert!(
                    (at(1) - 0.5).abs() < 0.01 && (at(2) - 0.3).abs() < 0.01,
                    "other lighting survives"
                );
            }
            drop(bytes);
            read.unmap();
        }
        assert!(scope.pop().await.is_none(), "no GPU validation errors");
    });
}

#[test]
fn shadow_ab_octant_bounds_are_conservative() {
    use super::moving_caster_octants;
    let lamp = [0.0, 0.0, 0.0];
    assert_eq!(
        moving_caster_octants([-20.0, -20.0, -20.0], [20.0, 20.0, 20.0], lamp, 64.0),
        255
    );
    assert_eq!(
        moving_caster_octants([20.0, 20.0, 20.0], [40.0, 40.0, 40.0], lamp, 64.0),
        128
    );
    assert_eq!(
        moving_caster_octants([-40.0, -40.0, -40.0], [-20.0, -20.0, -20.0], lamp, 64.0),
        1
    );
    // Model crossing +X/-X must not be culled from either side.
    let mask = moving_caster_octants([-2.0, 20.0, 20.0], [2.0, 40.0, 40.0], lamp, 64.0);
    assert_eq!(mask & 0b1100_0000, 0b1100_0000);
}
