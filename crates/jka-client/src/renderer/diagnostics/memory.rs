//! Logical texture sizes and allocator reservations, queried only on request.
use crate::renderer::Renderer;

/// Payload estimate from format, mip extents and samples. Driver alignment and
/// allocator slack are deliberately excluded; the allocator report covers those.
pub(in crate::renderer) fn texture_bytes(texture: &wgpu::Texture) -> u64 {
    let format = texture.format();
    let (block_width, block_height) = format.block_dimensions();
    let block_bytes = format.block_copy_size(None).unwrap_or(4) as u64;
    (0..texture.mip_level_count())
        .map(|mip| {
            let width = (texture.width() >> mip).max(1).div_ceil(block_width) as u64;
            let height = (texture.height() >> mip).max(1).div_ceil(block_height) as u64;
            width
                * height
                * texture.depth_or_array_layers() as u64
                * block_bytes
                * texture.sample_count() as u64
        })
        .sum()
}

impl Renderer {
    pub(in crate::renderer) fn print_memory_report(&self) {
        let mib = 1024.0 * 1024.0;
        if let Some(report) = self.device.generate_allocator_report() {
            println!(
                "MEMORY GPU allocator: live {:.2} MiB, reserved {:.2} MiB, unused reservation {:.2} MiB; {} allocations, {} blocks",
                report.total_allocated_bytes as f64 / mib,
                report.total_reserved_bytes as f64 / mib,
                report.total_reserved_bytes.saturating_sub(report.total_allocated_bytes) as f64 / mib,
                report.allocations.len(), report.blocks.len(),
            );
            let mut allocations = report.allocations;
            allocations.sort_unstable_by_key(|allocation| std::cmp::Reverse(allocation.size));
            for allocation in allocations.iter().take(8) {
                println!(
                    "MEMORY GPU allocation: {:.2} MiB {}",
                    allocation.size as f64 / mib,
                    allocation.name
                );
            }
        } else {
            println!("MEMORY GPU allocator: report unavailable on this backend");
        }
        for (name, bytes) in self.targets.memory_categories() {
            println!(
                "MEMORY GPU targets {name}: {:.2} MiB logical payload",
                bytes as f64 / mib
            );
        }
        if let Some(world) = self.world.as_ref() {
            let pixels: usize = world
                .lightmap_base_data
                .iter()
                .map(|image| image.pixel_storage_bytes())
                .sum();
            println!(
                "MEMORY CPU renderer: base lightmaps {:.2} MiB ({} collection owners), retained indexed vertices {:.2} MiB",
                pixels as f64 / mib, std::sync::Arc::strong_count(&world.lightmap_base_data),
                (world.static_ao_gpu_vertices.capacity() * std::mem::size_of::<crate::scene::GpuVertex>()) as f64 / mib,
            );
            let images: u64 = world
                .textures
                .iter()
                .chain(world.lightmaps.iter())
                .chain(world.deluxemaps.iter())
                .map(|image| texture_bytes(&image._texture))
                .sum();
            println!("MEMORY GPU world: textures/lightmaps/deluxemaps {:.2} MiB logical payload, vertex/index buffers {:.2} MiB",
                images as f64 / mib, (world.vertex_buffer.size() + world.index_buffer.size()) as f64 / mib);
        }
        println!("MEMORY GPU allocator figures include backend-managed heaps, not total process/VRAM usage. Logical payload estimates exclude padding and may include aliases; do not add them to allocator totals.");
    }
}
