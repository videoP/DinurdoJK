//! Windowless validation, opt in on a machine with a GPU using --ignored.
use super::*;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

fn finish(
    state: &mut BakedBrightness,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    jobs: &mut PipelineJobManager,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while state.busy {
        state.tick(device, queue, jobs);
        assert!(Instant::now() < deadline, "brightness jobs did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    original: &[u8],
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Baked brightness odd-size mip test"),
            size: wgpu::Extent3d {
                width: 5,
                height: 3,
                depth_or_array_layers: 1,
            },
            mip_level_count: 3,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        original,
    )
}

fn exercise(backend: wgpu::Backends) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: backend,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter =
        futures_lite::future::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("headless GPU adapter");
    println!("Baked brightness GPU test: {:?}", adapter.get_info());
    let (device, queue) =
        futures_lite::future::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .unwrap();
    let mut jobs = PipelineJobManager::new();
    // Three levels: 5x3, 2x1, 1x1, deliberately unaligned row widths.
    let original: Vec<u8> = (0..72).map(|i| ((i * 31 + 13) % 256) as u8).collect();
    let texture = image(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &original,
    );
    let retained_handle = texture.clone();
    let linear = image(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, &original);
    let mut state = BakedBrightness::default();

    state.register(&texture);
    assert!(state.entries.is_empty());
    assert!(!state.busy);
    assert_eq!(jobs.stats().queued, 0);
    state.set_target(true, 1.0);
    state.register(&texture);
    state.register(&linear);
    assert_eq!(state.entries.len(), 1, "data textures must be excluded");
    assert!(
        state.entries[&texture].original.is_none(),
        "neutral setting must not capture pixels"
    );
    assert!(!state.busy);
    assert_eq!(jobs.stats().queued, 0);

    state.set_target(true, 2.0);
    state.tick(&device, &queue, &mut jobs); // first job queued but not installed
    state.set_target(true, 3.0); // supersede while first job is in flight
    assert_eq!(read_original(&device, &queue, &texture).unwrap(), original);
    finish(&mut state, &device, &queue, &mut jobs);
    assert_eq!(
        read_original(&device, &queue, &texture).unwrap(),
        bake(&original, &gamma_table(3.0))
    );
    assert_eq!(read_original(&device, &queue, &linear).unwrap(), original);
    assert_eq!(
        texture, retained_handle,
        "textures and their views must be reused"
    );
    assert_eq!(
        state.entries[&texture].original.as_deref().unwrap(),
        original
    );
    assert_eq!(state.entries[&texture].applied, 3.0);
    assert_eq!(
        jobs.stats().queued,
        2,
        "slider updates must not backlog obsolete jobs"
    );

    state.set_target(true, 0.5);
    finish(&mut state, &device, &queue, &mut jobs);
    assert_eq!(
        read_original(&device, &queue, &texture).unwrap(),
        bake(&original, &gamma_table(0.5))
    );
    state.set_target(true, 1.0);
    finish(&mut state, &device, &queue, &mut jobs);
    assert_eq!(read_original(&device, &queue, &texture).unwrap(), original);

    // New assets inherit the active target without a renderer restart.
    state.set_target(true, 2.0);
    finish(&mut state, &device, &queue, &mut jobs);
    let new_texture = image(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &original,
    );
    state.register(&new_texture);
    finish(&mut state, &device, &queue, &mut jobs);
    assert_eq!(
        read_original(&device, &queue, &new_texture).unwrap(),
        bake(&original, &gamma_table(2.0))
    );

    // Switching off during a pending adjustment restores both original images.
    state.set_target(true, 3.0);
    state.tick(&device, &queue, &mut jobs);
    state.set_target(false, 3.0);
    assert!(
        state.overrides_output(),
        "hold output gamma neutral during restoration"
    );
    finish(&mut state, &device, &queue, &mut jobs);
    assert_eq!(read_original(&device, &queue, &texture).unwrap(), original);
    assert_eq!(
        read_original(&device, &queue, &new_texture).unwrap(),
        original
    );
    assert!(!state.overrides_output());
    assert!(
        state.entries.is_empty(),
        "original RAM must be released after restoring"
    );
    assert_eq!(jobs.stats().failed, 0);

    // Map replacement retires obsolete cached originals and stale work safely.
    state.set_target(true, 2.0);
    state.register(&texture);
    state.tick(&device, &queue, &mut jobs);
    state.retain_live(&[new_texture.clone()]);
    state.register(&new_texture);
    finish(&mut state, &device, &queue, &mut jobs);
    assert!(!state.entries.contains_key(&texture));
    assert_eq!(read_original(&device, &queue, &texture).unwrap(), original);
    state.set_target(false, 2.0);
    finish(&mut state, &device, &queue, &mut jobs);
    assert_eq!(
        read_original(&device, &queue, &new_texture).unwrap(),
        original
    );
    assert_eq!(jobs.stats().failed, 0);
}

#[test]
#[ignore = "requires a Vulkan GPU; no window is created"]
fn vulkan_bake_mips_coalesce_restore_and_retire() {
    exercise(wgpu::Backends::VULKAN);
}

#[test]
#[ignore = "requires a DirectX 12 GPU; no window is created"]
fn dx12_bake_mips_coalesce_restore_and_retire() {
    exercise(wgpu::Backends::DX12);
}
