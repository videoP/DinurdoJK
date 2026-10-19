//! Shader entries for the effects, composed over the same upstream modules the JKA surface uses.
use naga_oil::compose::{NagaModuleDescriptor, ShaderDefValue, ShaderType};
use std::borrow::Cow;
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub path: &'static str,
    pub source: &'static str,
}
pub const WAKE_FIELD: Entry = Entry {
    path: "naval-ocean/shaders/wake_field.wgsl",
    source: include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/wake_field.wgsl"),
};
pub const STAMPS: Entry = Entry {
    path: "naval-ocean/shaders/stamps.wgsl",
    source: include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/stamps.wgsl"),
};
pub const SPINDRIFT: Entry = Entry {
    path: "naval-ocean/shaders/spindrift.wgsl",
    source: include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/spindrift.wgsl"),
};
pub fn naga_module(
    composer: &mut naga_oil::compose::Composer,
    entry: Entry,
    defs: &[(&str, u32)],
) -> Result<wgpu::naga::Module, Box<naga_oil::compose::ComposerError>> {
    composer
        .make_naga_module(NagaModuleDescriptor {
            source: entry.source,
            file_path: entry.path,
            shader_type: ShaderType::Wgsl,
            shader_defs: defs.iter().map(|(n, v)| (n.to_string(), ShaderDefValue::UInt(*v))).collect(),
            ..Default::default()
        })
        .map_err(Box::new)
}
pub fn shader_module(device: &wgpu::Device, entry: Entry, defs: &[(&str, u32)]) -> wgpu::ShaderModule {
    let mut composer = blue_water_gpu::shader_composer::composer()
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&naga_oil::compose::Composer::default())));
    let module = naga_module(&mut composer, entry, defs)
        .unwrap_or_else(|e| panic!("composing {}: {}", entry.path, e.emit_to_string(&composer)));
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(entry.path),
        source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
    })
}
