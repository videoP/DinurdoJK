use super::*;

fn source() -> Source {
    // Source preparation supplies a normal material-batch template without PK3s.
    let text = r#"{
"classname" "worldspawn"
{
( 0 0 0 ) ( 0 64 0 ) ( 0 0 64 ) test/wall 0 0 0 1 1
( 64 0 0 ) ( 64 0 64 ) ( 64 64 0 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 0 0 64 ) ( 64 0 0 ) test/wall 0 0 0 1 1
( 0 64 0 ) ( 64 64 0 ) ( 0 64 64 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 64 0 0 ) ( 0 64 0 ) test/floor 0 0 0 1 1
( 0 0 64 ) ( 0 64 64 ) ( 64 0 64 ) test/ceiling 0 0 0 1 1
}
}"#;
    let map = scene::prepare_source(
        &std::env::temp_dir().join("jka-sun-fixture-assets"),
        None,
        &scene::MapSource::MapEditPreview {
            path: "sun-fixture.map".into(),
            text: text.into(),
        },
    )
    .unwrap();
    let mut floor = map.batches[0].clone();
    floor.vertices = 0..3;
    floor.pipeline.class = crate::renderer::DrawClass::Opaque;
    floor.lightmap = Some(0);
    floor.alpha_shadow = false;
    floor.light_filter = false;
    let mut sky = floor.clone();
    sky.vertices = 3..6;
    sky.pipeline.class = crate::renderer::DrawClass::Sky;
    sky.lightmap = None;
    sky.tc_gen = crate::renderer::TcGen::Base;
    let positions = [
        [0., 0., 0.],
        [100., 0., 0.],
        [0., 0., 100.],
        [-100., 100., -100.],
        [300., 100., -100.],
        [-100., 100., 300.],
    ];
    let uvs = [[0., 0.], [1., 0.], [0., 1.]];
    let vertices = positions
        .into_iter()
        .enumerate()
        .map(|(index, position)| GpuVertex {
            position,
            uv: uvs[index % 3],
            lightmap_uv: uvs[index % 3],
            normal: [0., 1., 0.],
            color: [1.; 4],
            alpha_cutoff: 1.,
        })
        .collect();
    let image = TextureData {
        label: "sun fixture".into(),
        source: None,
        width: 32,
        height: 32,
        rgba: [128, 128, 128, 255].repeat(32 * 32),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    };
    Source {
        cache: scene::StaticBspAoCacheInfo {
            directory: "unused/static-bsp-ao".into(),
            map_name: "fixture".into(),
            map_hash: 42,
            version: 1,
        },
        vertices: Arc::new(vertices),
        batches: vec![floor, sky],
        textures: Arc::new(vec![crate::materials::missing_texture_data()]),
        lightmaps: Arc::new(vec![image]),
        surface_ids: vec![0, 1],
        surfaces: vec![scene::LegacyDlightSurface::default(); 2],
        sun: scene::DirectionalSun {
            direction: [0., -1., 0.],
            color: [1.; 3],
            intensity: 340.,
        },
        encoding: Some(scene::BakedSunEncoding {
            exposure: 0.,
            gamma: 1.,
            compensate: 1.,
            brightness: 1.,
            deviance: 2.,
            samples: 32,
        }),
    }
}

#[test]
fn zero_settings_do_not_request_sunlight_work() {
    assert!(!requested(0.0, 0));
    assert!(requested(0.01, 0));
    assert!(requested(0.0, 1));
    assert!(requested(0.0, 2));
    let (tx, rx) = mpsc::channel();
    let signal = Arc::new(AtomicBool::new(false));
    let pending = Pending {
        rx,
        cancelled: Arc::clone(&signal),
    };
    drop(pending);
    assert!(signal.load(Ordering::Relaxed));
    assert!(tx.send(Err("stale map".to_owned())).is_err());
}

#[test]
fn sunlight_cache_fingerprint_tracks_geometry_sun_lightmaps_and_alpha_casters() {
    let mut source = source();
    let key = fingerprint(&source);
    source.sun.intensity += 1.;
    assert_ne!(key, fingerprint(&source));
    source.sun.intensity -= 1.;
    assert_eq!(key, fingerprint(&source));
    Arc::make_mut(&mut source.vertices)[0].position[0] += 1.;
    assert_ne!(key, fingerprint(&source));
    Arc::make_mut(&mut source.vertices)[0].position[0] -= 1.;
    Arc::make_mut(&mut source.lightmaps)[0].rgba[0] ^= 1;
    assert_ne!(key, fingerprint(&source));
    Arc::make_mut(&mut source.lightmaps)[0].rgba[0] ^= 1;
    source.batches[0].alpha_shadow = true;
    source.batches[0].baked_shadow_texture = Some(0);
    let caster_key = fingerprint(&source);
    Arc::make_mut(&mut source.textures)[0].rgba[3] ^= 255;
    assert_ne!(caster_key, fingerprint(&source));
}

#[test]
fn sun_cache_lossless_roundtrip_rejects_stale_truncated_and_corrupt_results() {
    let data = BakeData {
        calibration: BakedSunCalibration {
            radiance: [0.25, 0.5, 1.],
            surfaces: vec![[0., 1., 4., -1.], [260., 2.2, 4., 1.]],
        },
        masks: Some(vec![(0..1024).map(|v| (v * 73) as u8).collect()]),
    };
    let file = cache::encode(42, &data).unwrap();
    let decoded = cache::decode(&file, 42, 2, &[[32, 32]]).unwrap();
    assert_eq!(decoded.masks, data.masks);
    assert_eq!(decoded.calibration.radiance, data.calibration.radiance);
    assert_eq!(decoded.calibration.surfaces, data.calibration.surfaces);
    assert!(cache::decode(&file, 43, 2, &[[32, 32]]).is_none());
    assert!(cache::decode(&file, 42, 3, &[[32, 32]]).is_none());
    assert!(cache::decode(&file[..file.len() / 2], 42, 2, &[[32, 32]]).is_none());
    let mut corrupt = file.clone();
    corrupt[24] ^= 1; // checksum of uncompressed output
    assert!(cache::decode(&corrupt, 42, 2, &[[32, 32]]).is_none());
    corrupt[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(cache::decode(&corrupt, 42, 2, &[[32, 32]]).is_none());
}

#[test]
fn one_atlas_sunlight_is_bit_identical_with_one_or_four_workers() {
    let source = source();
    let bake = |workers| {
        let mut images = source.lightmaps.as_ref().clone();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        let fit = pool
            .install(|| {
                original_sun::bake_authored_sun_masks(
                    &source.vertices,
                    &source.batches,
                    &source.textures,
                    &mut images,
                    &source.surface_ids,
                    source.surfaces.len(),
                    source.sun,
                    source.encoding.unwrap(),
                    &AtomicBool::new(false),
                )
            })
            .unwrap();
        (images.remove(0).rgba, fit)
    };
    let (serial, fit1) = bake(1);
    let (parallel, fit4) = bake(4);
    assert_eq!(serial, parallel);
    assert_eq!(fit1.surfaces, fit4.surfaces);
    assert!(parallel
        .chunks_exact(4)
        .zip(source.lightmaps[0].rgba.chunks_exact(4))
        .all(|(a, b)| a[..3] == b[..3]));
    let mut cancelled_images = source.lightmaps.as_ref().clone();
    assert!(original_sun::bake_authored_sun_masks(
        &source.vertices,
        &source.batches,
        &source.textures,
        &mut cancelled_images,
        &source.surface_ids,
        2,
        source.sun,
        source.encoding.unwrap(),
        &AtomicBool::new(true)
    )
    .is_none());
    assert_eq!(cancelled_images[0].rgba, source.lightmaps[0].rgba);
}

#[test]
fn ao_alpha_patching_preserves_rgb_and_matches_full_image_for_tiles() {
    let mut full = [12, 34, 56, 255].repeat(4);
    patch_mask(&[0, 255], [2, 1], &mut full, [4, 1], [0, 0, 4, 1]);
    assert_eq!(
        full.chunks_exact(4).map(|p| p[3]).collect::<Vec<_>>(),
        [0, 64, 191, 255]
    );
    assert!(full.chunks_exact(4).all(|p| p[..3] == [12, 34, 56]));
    let mut tile = [12, 34, 56, 255].repeat(2);
    patch_mask(&[0, 255], [2, 1], &mut tile, [4, 1], [1, 0, 2, 1]);
    assert_eq!(tile, full[4..12]);
}

#[test]
fn completed_sun_mask_preserves_ao_rgb_on_gpu_in_linear_and_srgb() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let upload = MaskUpload::new(&device);
        for srgb in [false, true] {
            let rgb = [
                1, 24, 170, 255, 28, 56, 123, 255, 75, 125, 200, 255, 128, 240, 255, 255,
            ];
            let original = TextureData {
                label: "already AO shaded".into(),
                source: None,
                width: 4,
                height: 1,
                rgba: rgb.to_vec(),
                rgba16f: None,
                mip_level_count: 1,
                clamp: true,
                srgb,
            };
            let mut base = original.clone();
            base.width = 2;
            let images = [crate::renderer::upload_texture(&device, &queue, &original)];
            let installed = upload.apply(&device, &queue, &images, &[base], &[vec![0, 255]]);
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                installed[0]._texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d {
                    width: 4,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, |result| result.unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let mapped = readback.slice(..).get_mapped_range().unwrap();
            for (index, pixel) in mapped[..16].chunks_exact(4).enumerate() {
                assert_eq!(
                    &pixel[..3],
                    &rgb[index * 4..index * 4 + 3],
                    "sRGB={srgb}: AO RGB changed"
                );
                assert_eq!(pixel[3], [0, 64, 191, 255][index]);
            }
            drop(mapped);
            readback.unmap();
        }
        assert!(scope.pop().await.is_none());
    });
}
