//! On-demand accounting of the large buffers retained for renderer restarts.
use super::{GpuVertex, PreparedMap};

impl PreparedMap {
    /// Approximate retained CPU bytes by category (Vec capacities; nested
    /// owners that are not listed are excluded). Pixel buffers already in
    /// `seen_pixels` are skipped, so interned images count once overall.
    pub(crate) fn heap_estimate(&self, seen_pixels: &mut std::collections::HashSet<usize>) -> [(&'static str, usize); 9] {
        fn bytes<T>(items: &Vec<T>) -> usize { items.capacity() * std::mem::size_of::<T>() }
        let pixels = |images: &[crate::materials::TextureData], seen: &mut std::collections::HashSet<usize>| {
            images.iter().map(|image| {
                let mut total = 0;
                if seen.insert(image.rgba.identity()) { total += image.rgba.capacity(); }
                if let Some(half) = &image.rgba16f {
                    if seen.insert(half.identity()) { total += half.capacity() * 2; }
                }
                total
            }).sum::<usize>()
        };
        let plan = &self.portal_draw_plan;
        let weather = self.weather_occlusion.as_ref().map_or(0, |w|
            bytes(&w.brushes) + bytes(&w.triangles) + bytes(&w.topography_triangles));
        let grid = self.static_light_grid.as_ref().map_or(0, |g|
            bytes(&g.direction_rgba) + bytes(&g.lighting_rgba) + bytes(&g.irradiance_volume_rgba))
            + self.voxel_probe_gi.as_ref().map_or(0, |g| bytes(&g.point_rgba) + bytes(&g.area_rgba));
        [
            ("texture pixels", pixels(&self.textures, seen_pixels)),
            ("lightmap pixels", pixels(&self.lightmaps, seen_pixels) + pixels(&self.deluxemaps, seen_pixels)),
            ("vertices", bytes(&self.vertices) + bytes(&self.inline_vertices)),
            ("draw batches", bytes(&self.batches) + bytes(&self.pvs_batches) + bytes(&self.inline_batches)
                + bytes(&self.legacy_dlight_surfaces) + bytes(&self.legacy_dlight_triangle_surfaces)),
            ("portal plans", bytes(&plan.variants) + bytes(&plan.geometries) + bytes(&plan.plans)
                + plan.plans.iter().map(bytes).sum::<usize>() + bytes(&plan.plan_by_cluster)),
            ("surface sprites", bytes(&self.surface_sprite_bake.instances) + bytes(&self.surface_sprite_bake.chunks)),
            ("weather occlusion", weather),
            ("light grid/GI", grid),
            ("grass", bytes(&self.grass_patches)),
        ]
    }

    pub(crate) fn memory_report_lines(&self) -> Vec<String> {
        let mib = 1024.0 * 1024.0;
        let mut lines = Vec::new();
        for (name, images) in [
            ("textures", &self.textures),
            ("lightmaps", &self.lightmaps),
            ("deluxemaps", &self.deluxemaps),
        ] {
            let rgba: usize = images.iter().map(|image| image.rgba.capacity()).sum();
            let bytes: usize = images.iter().map(|image| image.pixel_storage_bytes()).sum();
            lines.push(format!(
                "MEMORY CPU prepared {name}: {:.2} MiB (RGBA8 {:.2}, FP16 {:.2}); {} images, {} collection owners",
                bytes as f64 / mib, rgba as f64 / mib, (bytes - rgba) as f64 / mib,
                images.len(), std::sync::Arc::strong_count(images),
            ));
        }
        lines.push(format!(
            "MEMORY CPU prepared vertices: {:.2} MiB world + {:.2} MiB inline",
            (self.vertices.capacity() * std::mem::size_of::<GpuVertex>()) as f64 / mib,
            (self.inline_vertices.capacity() * std::mem::size_of::<GpuVertex>()) as f64 / mib,
        ));
        lines.push("MEMORY CPU figures cover listed buffers, including mips; shared collections count once per line. Other map/engine allocations are excluded. Do not sum shared prepared/renderer images.".to_owned());
        lines
    }
}
