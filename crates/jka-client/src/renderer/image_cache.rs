//! Device-local immutable uploads, shared by asset content rather than map.
//! Both the device registry and image entries are weak: no unused GPU asset
//! survives solely because it has appeared in this cache.
use super::textures::{GpuImage, GpuImageInner};
use crate::materials::SharedPixels;
use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock, Weak}};

#[derive(Clone)]
pub(in crate::renderer) enum Pixels {
    U8(SharedPixels<u8>, usize),
    U16(SharedPixels<u16>),
}
impl Pixels {
    fn identity(&self) -> (u8, usize, usize) {
        match self { Self::U8(p, offset) => (0, p.identity(), *offset), Self::U16(p) => (1, p.identity(), 0) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::renderer) struct ImageSpec {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub format: wgpu::TextureFormat,
    pub usage: wgpu::TextureUsages,
}
#[derive(PartialEq, Eq, Hash)]
struct Key { spec: ImageSpec, pixels: (u8, usize, usize) }
#[derive(Default)]
struct Entries { images: HashMap<Key, Weak<GpuImageInner>>, misses: usize }
pub(in crate::renderer) struct DeviceImages { device: wgpu::Device, entries: Mutex<Entries> }

fn device_images(device: &wgpu::Device) -> Arc<DeviceImages> {
    static DEVICES: OnceLock<Mutex<Vec<Weak<DeviceImages>>>> = OnceLock::new();
    let mut devices = DEVICES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    devices.retain(|d| d.strong_count() != 0);
    if let Some(cache) = devices.iter().filter_map(Weak::upgrade).find(|c| c.device == *device) { return cache; }
    let cache = Arc::new(DeviceImages { device: device.clone(), entries: Mutex::new(Entries::default()) });
    devices.push(Arc::downgrade(&cache));
    cache
}

pub(in crate::renderer) fn get_or_upload(
    device: &wgpu::Device, spec: ImageSpec, pixels: Pixels, upload: impl FnOnce() -> GpuImage,
) -> GpuImage {
    let cache = device_images(device);
    let key = Key { spec, pixels: pixels.identity() };
    let mut entries = cache.entries.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(image) = entries.images.get(&key).and_then(Weak::upgrade) { return GpuImage(image); }
    entries.misses += 1;
    if entries.misses >= 64 {
        entries.images.retain(|_, image| image.strong_count() != 0);
        entries.misses = 0;
    }
    let uploaded = upload();
    let image = Arc::new(GpuImageInner {
        _texture: uploaded._texture.clone(), view: uploaded.view.clone(),
        // Keep the canonical source allocation/identity valid while its GPU
        // image is live. This is shared storage, not another decoded copy.
        _pixels: Some(pixels), _cache: Some(Arc::clone(&cache)),
    });
    entries.images.insert(key, Arc::downgrade(&image));
    GpuImage(image)
}

/// Unique CPU pixel buffers kept alive by live cached GPU images on `device`
/// (count, bytes). These are the canonical upload sources described above.
pub(in crate::renderer) fn pinned_pixel_bytes(device: &wgpu::Device) -> (usize, usize) {
    let cache = device_images(device);
    let entries = cache.entries.lock().unwrap_or_else(|e| e.into_inner());
    let mut seen = std::collections::HashSet::new();
    let (mut count, mut bytes) = (0usize, 0usize);
    for image in entries.images.values().filter_map(Weak::upgrade) {
        let Some(pixels) = image._pixels.as_ref() else { continue };
        let (identity, size) = match pixels {
            Pixels::U8(p, _) => (p.identity(), p.capacity()),
            Pixels::U16(p) => (p.identity(), p.capacity() * 2),
        };
        if seen.insert(identity) { count += 1; bytes += size; }
    }
    (count, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{materials::TextureData, renderer::textures::{upload_cached_texture, upload_texture}};

    fn image(label: &str) -> TextureData {
        TextureData { label: label.into(), source: Some(std::path::PathBuf::from(label)),
            width: 4, height: 4, rgba: vec![17; 64].into(), rgba16f: None,
            mip_level_count: 1, clamp: false, srgb: true }
    }

    fn first_pixel(device: &wgpu::Device, queue: &wgpu::Queue, image: &GpuImage) -> Vec<u8> {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(image._texture.as_image_copy(), wgpu::TexelCopyBufferInfo {
            buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: None },
        }, wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 });
        queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| { tx.send(r).unwrap(); });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = buffer.slice(..).get_mapped_range().unwrap();
        let pixel = bytes[..4].to_vec();
        drop(bytes);
        buffer.unmap();
        pixel
    }

    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn immutable_asset_cache_crosses_maps_and_preserves_mutable_isolation() {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN, ..wgpu::InstanceDescriptor::new_without_display_handle() });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let bc = adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC;
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: bc, ..Default::default() }).await.unwrap();
            let a = image("maps/a/pak0.pk3/stone");
            let b = image("maps/b/custom.pk3/another-name");
            assert!(a.rgba.ptr_eq(&b.rgba));
            let gpu_a = upload_cached_texture(&device, &queue, &a);
            let gpu_b = upload_cached_texture(&device, &queue, &b);
            assert_eq!(gpu_a._texture, gpu_b._texture, "map/name/provider must not block identical uploads");
            let mut changed = b.clone(); changed.rgba[0] = 92;
            let different = upload_cached_texture(&device, &queue, &changed);
            assert_ne!(gpu_a._texture, different._texture);
            let mut linear = a.clone(); linear.srgb = false;
            assert_ne!(gpu_a._texture, upload_cached_texture(&device, &queue, &linear)._texture);
            let mut dimensions = a.clone(); dimensions.width = 2; dimensions.height = 8;
            assert_ne!(gpu_a._texture, upload_cached_texture(&device, &queue, &dimensions)._texture);

            let mutable = upload_texture(&device, &queue, &a);
            assert_ne!(gpu_a._texture, mutable._texture, "streaming/writable uploads must stay private");
            let mut detached = gpu_b.clone();
            assert!(detached.detach_for_write(&device, &queue));
            queue.write_texture(detached._texture.as_image_copy(), &[99; 64],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16), rows_per_image: Some(4) },
                wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 });
            assert_eq!(first_pixel(&device, &queue, &gpu_a), [17; 4]);
            assert_eq!(first_pixel(&device, &queue, &detached), [99; 4]);
            assert_eq!(upload_cached_texture(&device, &queue, &a)._texture, gpu_a._texture,
                "a private edit must not poison the immutable asset cache");

            if !bc.is_empty() {
                let encoded_a = crate::renderer::texture_compression::encode_texture(&a, 0).unwrap();
                let encoded_b = crate::renderer::texture_compression::encode_texture(&b, 0).unwrap();
                let x = crate::renderer::texture_compression::upload_cached_encoded_texture(&device, &queue, &a.label, &encoded_a);
                let y = crate::renderer::texture_compression::upload_cached_encoded_texture(&device, &queue, &b.label, &encoded_b);
                assert_eq!(x._texture, y._texture, "identical BC payloads must share too");
            }
            let (other_device, other_queue) = adapter.request_device(&Default::default()).await.unwrap();
            assert_ne!(gpu_a._texture, upload_cached_texture(&other_device, &other_queue, &a)._texture);
            let weak = Arc::downgrade(&gpu_a.0);
            drop((gpu_a, gpu_b));
            assert!(weak.upgrade().is_none(), "the GPU cache must not retain unused assets");
        });
    }
}
