//! Optional upload-time gamma for scene colour textures. No shaders or render passes.
//! Originals are captured lazily, once, and retained only while baking/restoring.
use crate::pipeline_jobs::{PipelineJobKey, PipelineJobManager};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{mpsc, Arc};

const BATCH_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub(super) struct BakedBrightness {
    pub(super) enabled: bool,
    gamma: f32,
    pub(super) busy: bool,
    generation: u64,
    serial: u64,
    entries: HashMap<wgpu::Texture, Entry>,
    pending: VecDeque<wgpu::Texture>,
    inflight: Option<PipelineJobKey>,
}

struct Entry {
    original: Option<Arc<[u8]>>,
    applied: f32,
}
struct Input {
    texture: wgpu::Texture,
    original: Option<Arc<[u8]>>,
}
struct Output {
    texture: wgpu::Texture,
    original: Arc<[u8]>,
    pixels: Option<Vec<u8>>,
}
struct Batch {
    generation: u64,
    gamma: f32,
    result: Result<Vec<Output>, String>,
}

pub(super) fn gamma_table(gamma: f32) -> [u8; 256] {
    let gamma = if gamma.is_finite() {
        gamma.clamp(0.5, 3.0)
    } else {
        1.0
    };
    std::array::from_fn(|i| (255.0 * (i as f32 / 255.0).powf(1.0 / gamma) + 0.5) as u8)
}

fn bake(original: &[u8], table: &[u8; 256]) -> Vec<u8> {
    let mut pixels = original.to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        for component in &mut pixel[..3] {
            *component = table[*component as usize];
        }
    }
    pixels
}

impl BakedBrightness {
    pub(super) fn overrides_output(&self) -> bool {
        self.enabled || self.busy
    }

    fn target(&self) -> f32 {
        if self.enabled {
            self.gamma
        } else {
            1.0
        }
    }

    pub(super) fn set_target(&mut self, enabled: bool, gamma: f32) {
        self.enabled = enabled;
        self.gamma = if gamma.is_finite() {
            gamma.clamp(0.5, 3.0)
        } else {
            1.0
        };
        self.generation = self.generation.wrapping_add(1);
        self.pending.clear();
        let target = self.target();
        self.pending.extend(
            self.entries
                .iter()
                .filter(|(_, entry)| entry.applied != target)
                .map(|(texture, _)| texture.clone()),
        );
        self.settle();
    }

    /// Called on asset creation/settings changes only, never for cached draws.
    pub(super) fn register(&mut self, texture: &wgpu::Texture) {
        if !self.enabled
            || texture.format() != wgpu::TextureFormat::Rgba8UnormSrgb
            || texture.dimension() != wgpu::TextureDimension::D2
            || !texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
            || self.entries.contains_key(texture)
        {
            return;
        }
        self.entries.insert(
            texture.clone(),
            Entry {
                original: None,
                applied: 1.0,
            },
        );
        if self.target() != 1.0 {
            self.pending.push_back(texture.clone());
            self.busy = true;
        }
    }

    pub(super) fn retain_live(&mut self, textures: &[wgpu::Texture]) {
        let live: HashSet<_> = textures.iter().collect();
        self.entries.retain(|texture, _| live.contains(texture));
        self.pending
            .retain(|texture| self.entries.contains_key(texture));
        self.settle();
    }

    fn settle(&mut self) {
        self.busy = self.inflight.is_some() || !self.pending.is_empty();
        if !self.enabled && !self.busy {
            self.entries.clear();
        }
    }

    /// At most one small batch in flight. Slider changes replace pending work;
    /// stale completions may supply originals, but cannot upload stale pixels.
    /// All GPU waits and pixel transforms occur on the existing worker pool.
    pub(super) fn tick(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        jobs: &mut PipelineJobManager,
    ) -> bool {
        let old_override = self.overrides_output();
        if let Some(key) = self.inflight {
            if let Some(batch) = jobs.take_ready::<Batch>(key) {
                self.inflight = None;
                match batch.result {
                    Ok(outputs) => {
                        for output in outputs {
                            let Some(entry) = self.entries.get_mut(&output.texture) else {
                                continue;
                            };
                            if batch.generation == self.generation {
                                let pixels = output.pixels.as_deref().unwrap_or(&output.original);
                                write_mips(queue, &output.texture, pixels);
                                entry.applied = batch.gamma;
                            }
                            entry.original = Some(output.original);
                        }
                    }
                    Err(error) => {
                        eprintln!("Baked brightness failed; restoring original textures: {error}");
                        self.set_target(false, self.gamma);
                    }
                }
            }
        }
        if self.inflight.is_none() && !self.pending.is_empty() {
            let mut inputs = Vec::new();
            let mut bytes = 0;
            while let Some(texture) = self.pending.front() {
                let size = packed_len(texture);
                if !inputs.is_empty() && bytes + size > BATCH_BYTES {
                    break;
                }
                let texture = self.pending.pop_front().unwrap();
                if let Some(entry) = self.entries.get(&texture) {
                    inputs.push(Input {
                        texture,
                        original: entry.original.clone(),
                    });
                    bytes += size;
                }
            }
            if !inputs.is_empty() {
                self.serial = self.serial.wrapping_add(1);
                let key = PipelineJobKey::new("baked-brightness", 0, self.serial);
                let generation = self.generation;
                let gamma = self.target();
                let device = device.clone();
                let queue = queue.clone();
                let accepted = jobs.request(key, "Baked brightness texture update", move || {
                    // Return failures ourselves so a worker panic cannot leave a pending bake forever.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let table = gamma_table(gamma);
                        inputs
                            .into_iter()
                            .map(|input| {
                                let original = match input.original {
                                    Some(original) => original,
                                    None => {
                                        Arc::from(read_original(&device, &queue, &input.texture)?)
                                    }
                                };
                                let pixels = (gamma != 1.0).then(|| bake(&original, &table));
                                Ok(Output {
                                    texture: input.texture,
                                    original,
                                    pixels,
                                })
                            })
                            .collect::<Result<Vec<_>, String>>()
                    }))
                    .unwrap_or_else(|_| Err("texture update worker panicked".to_owned()));
                    Batch {
                        generation,
                        gamma,
                        result,
                    }
                });
                if accepted {
                    self.inflight = Some(key);
                } else {
                    self.set_target(false, self.gamma);
                }
            }
        }
        self.settle();
        old_override != self.overrides_output()
    }
}

fn mip_shapes(texture: &wgpu::Texture) -> impl Iterator<Item = (u32, u32, u32)> + '_ {
    (0..texture.mip_level_count()).map(|level| {
        (
            level,
            (texture.width() >> level).max(1),
            (texture.height() >> level).max(1),
        )
    })
}
fn packed_len(texture: &wgpu::Texture) -> usize {
    mip_shapes(texture)
        .map(|(_, w, h)| w as usize * h as usize * 4)
        .sum()
}

fn read_original(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Result<Vec<u8>, String> {
    let size: u64 = mip_shapes(texture)
        .map(|(_, w, h)| u64::from((w * 4).div_ceil(256) * 256) * u64::from(h))
        .sum();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Baked brightness original pixels"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Baked brightness original texture readback"),
    });
    let mut offset = 0;
    for (level, width, height) in mip_shapes(texture) {
        let stride = (width * 4).div_ceil(256) * 256;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        offset += u64::from(stride) * u64::from(height);
    }
    let submission = queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(30))
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let mapped = buffer.slice(..).get_mapped_range();
    let mut pixels = Vec::with_capacity(packed_len(texture));
    let mut offset = 0;
    for (_, width, height) in mip_shapes(texture) {
        let row = width as usize * 4;
        let stride = row.div_ceil(256) * 256;
        for y in 0..height as usize {
            let start = offset + y * stride;
            pixels.extend_from_slice(&mapped[start..start + row]);
        }
        offset += stride * height as usize;
    }
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}

fn write_mips(queue: &wgpu::Queue, texture: &wgpu::Texture, pixels: &[u8]) {
    let mut offset = 0;
    for (level, width, height) in mip_shapes(texture) {
        let len = width as usize * height as usize * 4;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels[offset..offset + len],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        offset += len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_gamma_is_byte_exact_and_invalid_gamma_is_neutral() {
        let identity = std::array::from_fn(|i| i as u8);
        assert_eq!(gamma_table(1.0), identity);
        assert_eq!(gamma_table(f32::NAN), identity);
        assert_eq!(gamma_table(f32::INFINITY), identity);
    }
    #[test]
    fn baked_curve_is_monotonic_and_preserves_endpoints() {
        for gamma in [0.5, 0.75, 1.0, 1.5, 2.0, 3.0] {
            let table = gamma_table(gamma);
            assert_eq!(table[0], 0);
            assert_eq!(table[255], 255);
            assert!(table.windows(2).all(|v| v[0] <= v[1]));
            if gamma > 1.0 {
                assert!(table[64] > 64);
            }
            if gamma < 1.0 {
                assert!(table[64] < 64);
            }
        }
    }
    #[test]
    fn rgb_changes_but_alpha_and_original_mips_do_not() {
        let original = [30, 60, 90, 0, 120, 150, 180, 127, 1, 2, 3, 255];
        let saved = original;
        let bright = bake(&original, &gamma_table(3.0));
        assert!(bright[0] > original[0]);
        for alpha in [3, 7, 11] {
            assert_eq!(bright[alpha], original[alpha]);
        }
        assert_eq!(original, saved);
        assert_eq!(bake(&original, &gamma_table(1.0)), original);
        // Each new curve starts from saved pixels, never the prior baked result.
        assert_ne!(
            bake(&original, &gamma_table(2.0)),
            bake(&bright, &gamma_table(2.0))
        );
    }
}

#[cfg(test)]
#[path = "brightness/gpu_tests.rs"]
mod gpu_tests;
