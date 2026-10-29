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
        if let Some(vram) = super::vram::query(&self.adapter.get_info()) {
            println!(
                "MEMORY GPU dedicated VRAM (DXGI, whole process incl. driver): {:.2} MiB used of {:.2} MiB OS budget ({:.0}%); GPU-visible system memory {:.2} MiB",
                vram.usage as f64 / mib, vram.budget as f64 / mib,
                vram.usage as f64 * 100.0 / vram.budget as f64,
                vram.shared_usage as f64 / mib,
            );
        }
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
            // Per-label totals: repeated per-scene resources show up as count x size.
            let mut by_label = std::collections::HashMap::<&str, (u64, usize)>::new();
            for allocation in &allocations {
                let entry = by_label.entry(allocation.name.as_str()).or_default();
                entry.0 += allocation.size;
                entry.1 += 1;
            }
            let mut by_label: Vec<_> = by_label.into_iter().collect();
            by_label.sort_unstable_by_key(|(_, (bytes, _))| std::cmp::Reverse(*bytes));
            for (name, (bytes, count)) in by_label.iter().take(20) {
                println!("MEMORY GPU label total: {:.2} MiB in {count} x {name}", *bytes as f64 / mib);
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
        if let Some(blend) = &self.dynamic_model_renderer.legacy_blend {
            println!("MEMORY GPU legacy model blend: {:.2} MiB logical payload", blend.bytes() as f64 / mib);
        }
        if !self.studio_scenes.is_empty() {
            let mut images = std::collections::HashSet::new();
            let mut unique = 0_u64;
            let mut aliases = 0_u64;
            let mut targets = 0_u64;
            let mut active = 0;
            for source in self.studio_scenes.values() {
                if source.size.width > 1 || source.size.height > 1 { active += 1; }
                targets += source.targets.memory_categories().iter().map(|(_, bytes)| bytes).sum::<u64>();
                targets += source.offscreen_output.as_ref().map_or(0, texture_bytes);
                if let Some(world) = source.world.as_ref() {
                    for image in world.textures.iter().chain(&world.lightmaps).chain(&world.deluxemaps) {
                        let bytes = texture_bytes(&image._texture);
                        if images.insert(image._texture.clone()) { unique += bytes; } else { aliases += bytes; }
                    }
                }
            }
            println!("MEMORY GPU Studio: {} retained scenes, {} active views; separate targets {:.2} MiB, unique world images {:.2} MiB, shared image aliases {:.2} MiB (counted once in unique images)",
                self.studio_scenes.len(), active, targets as f64 / mib, unique as f64 / mib, aliases as f64 / mib);
            // CPU data each retained scene's world keeps after upload.
            fn bytes<T>(items: &Vec<T>) -> usize { items.capacity() * std::mem::size_of::<T>() }
            let mut lightmaps = std::collections::HashSet::new();
            let mut indices = std::collections::HashSet::new();
            let mut totals = [0usize; 6];
            for world in self.studio_scenes.values().filter_map(|scene| scene.world.as_ref()) {
                totals[0] += bytes(&world.inspector_vertices) + bytes(&world.inspector_textures)
                    + bytes(&world.inspector_lightmaps);
                totals[1] += bytes(&world.static_ao_gpu_vertices) + bytes(&world.static_ao_source_to_gpu);
                totals[2] += bytes(&world.coarse_batches) + bytes(&world.full_batches) + bytes(&world.auto4_batches)
                    + world.coarse_visible_batches_by_cluster.iter().chain(&world.full_visible_batches_by_cluster)
                        .map(bytes).sum::<usize>()
                    + world.auto4_plan_refs.iter().map(bytes).sum::<usize>();
                if indices.insert(std::sync::Arc::as_ptr(&world.remap_index_source) as usize) {
                    totals[3] += bytes(&world.remap_index_source);
                }
                if lightmaps.insert(std::sync::Arc::as_ptr(&world.lightmap_base_data) as usize) {
                    totals[4] += world.lightmap_base_data.iter().map(|image| image.pixel_storage_bytes()).sum::<usize>();
                }
                totals[5] += bytes(&world.raster_glass_static) + bytes(&world.legacy_dlight_surfaces)
                    + bytes(&world.legacy_dlight_triangle_surfaces);
            }
            let (pinned_count, pinned_bytes) = crate::renderer::image_cache::pinned_pixel_bytes(&self.device);
            println!("MEMORY CPU Studio scene worlds: inspector {:.2} MiB, static-AO vertex copies {:.2}, batches/PVS lists {:.2}, AUTO4 source indices {:.2}, base lightmaps {:.2}, glass/dlight surfaces {:.2}; GPU image cache keeps {} source pixel buffers alive, {:.2} MiB (all renderers)",
                totals[0] as f64 / mib, totals[1] as f64 / mib, totals[2] as f64 / mib, totals[3] as f64 / mib,
                totals[4] as f64 / mib, totals[5] as f64 / mib, pinned_count, pinned_bytes as f64 / mib);
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
