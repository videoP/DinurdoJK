//! On-demand accounting of the large buffers retained for renderer restarts.
use super::{GpuVertex, PreparedMap};

impl PreparedMap {
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
