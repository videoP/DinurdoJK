//! Where the opaque scene's depth copy holds any surface, for the screen-space reflections: a port of upstream
//! `naval_ocean::reflection_tiles` (vendor/blue-water-sea-and-sky/ocean/src/reflection_tiles.rs) running its unchanged
//! `shaders/reflection_tiles.wgsl`. After the scene copy, `occupancy` marks each [`TILE`]² pixel tile holding a surface
//! and `sum` adds the marks into a summed-area table the surface reads in `surface_between`: a ray whose screen box holds
//! no surface would read sky at every step and find nothing, so it returns at once with what the trace would return.
//!
//! Without it the surface binds a zeroed table whose header matches no depth copy, so `surface_between` answers "maybe"
//! and every water pixel marches its full ray. The reach stays 0 (every surface nearer than the clear is marked): the
//! upstream reach bound assumes its own camera and far plane, so it is not ported.
//!
//! JKA maps put a seabed under the water, and the depth copy holds it behind every water pixel, so upstream's test
//! marks the whole sea and every ray is traced again. The JKA occupancy also leaves unmarked any depth pixel lying below
//! the floor ([`ReflectionTiles::set_floor`]: the lowest water plane less its trough bound), while the camera is above
//! that floor. That is lossless: a ray starts on the waves and is mirrored upward, so every point it reads lies above the
//! floor; a camera ray reaching a surface below the floor (from an eye above it) is heading down, so on that camera ray
//! the ray's point comes before the surface and is never behind it, and the trace cannot accept a hit there. A box
//! holding only sky and such surfaces therefore finds nothing, exactly as the full trace would.
//!
//! Native only, as upstream.

/// Pixels per tile edge (`REFLECTION_TILE` in both shaders).
pub const TILE: u32 = 16;

const LABEL: &str = "Blue Water reflection tiles";
const SHADER: &str = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/reflection_tiles.wgsl");

/// The JKA occupancy: upstream's shader with the floor test added to its one mark (see the module docs).
fn jka_shader() -> String {
    const REACH: &str = "@group(0) @binding(3) var<uniform> reach: vec4<f32>;";
    const MARK: &str = "textureLoad(depth, vec2<i32>(pixel.xy), 0) > reach.x";
    assert_eq!(SHADER.matches(REACH).count(), 1, "upstream reflection tiles reach binding changed");
    assert_eq!(SHADER.matches(MARK).count(), 1, "upstream reflection tiles mark changed");
    SHADER.replace(REACH, &format!("{REACH}\n{FLOOR_WGSL}"))
        .replace(MARK, "jka_marked(vec2<i32>(pixel.xy), textureLoad(depth, vec2<i32>(pixel.xy), 0))")
}

const FLOOR_WGSL: &str = "
// JKA: the surface camera's inverse view-projection (its ocean space, metres, the water plane at y = 0) and x the floor
// (ocean y), y 1 while the floor test applies.
struct JkaTileFloor {
    inverse_view_projection: mat4x4<f32>,
    floor: vec4<f32>,
}
@group(0) @binding(4) var<uniform> jka_floor: JkaTileFloor;

fn jka_marked(pixel: vec2<i32>, value: f32) -> bool {
    if value <= reach.x {
        return false;
    }
    if jka_floor.floor.y < 0.5 {
        return true;
    }
    let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(textureDimensions(depth));
    let point = jka_floor.inverse_view_projection * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, value, 1.0);
    return point.y / point.w >= jka_floor.floor.x;
}
";

/// Tiles (across, down) over a depth copy `width` × `height` pixels.
pub fn tile_counts(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(TILE), height.div_ceil(TILE))
}

/// Bytes of the table for `tiles` (across, down): the header and (tiles + 1)² entries.
pub fn table_bytes((across, down): (u32, u32)) -> u64 {
    (2 + u64::from(across + 1) * u64::from(down + 1)) * 4
}

/// The tiles pass and its table for one depth copy (its size is fixed for the copy's life).
pub struct ReflectionTiles {
    occupancy: wgpu::ComputePipeline,
    sum: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    table: wgpu::Buffer,
    tiles: (u32, u32),
    floor: wgpu::Buffer,
    dump: Option<Dump>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TileFloor {
    inverse_view_projection: [[f32; 4]; 4],
    floor: [f32; 4],
}

/// Diagnostic (`JKA_SSR_TILES_DUMP=<file.pgm>`): one frame's tile marks written as a PGM image (255 = a tile holding a
/// surface, so rays crossing it are traced), read back a few frames after it is copied, outside any map callback.
struct Dump {
    path: std::path::PathBuf,
    readback: wgpu::Buffer,
    frame: std::sync::atomic::AtomicU32,
    /// The last floor set (f32 bits, NaN for none), for the dump's report.
    floor: std::sync::atomic::AtomicU32,
    mapped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Frames after the table is built until the dump copies it (the camera has settled), then until it is mapped and read.
const DUMP_COPY_FRAME: u32 = 600;
const DUMP_MAP_FRAME: u32 = DUMP_COPY_FRAME + 10;
const DUMP_READ_FRAME: u32 = DUMP_MAP_FRAME + 10;

impl ReflectionTiles {
    pub fn new(device: &wgpu::Device, depth: &wgpu::TextureView, (width, height): (u32, u32)) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(LABEL),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(1),
                storage(2),
                uniform(3),
                uniform(4),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(LABEL),
            source: wgpu::ShaderSource::Wgsl(jka_shader().into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(LABEL),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(LABEL),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        });
        let tiles = tile_counts(width, height);
        let marks = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: u64::from(tiles.0 * tiles.1).max(1) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let table = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: table_bytes(tiles),
            // COPY_SRC for the GPU test's readback.
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // Zeroed: reach 0 marks every surface nearer than the clear.
        let reach = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        // Zeroed: no floor test until `set_floor` (upstream's marks).
        let floor = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: std::mem::size_of::<TileFloor>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 1, resource: marks.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: table.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: reach.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: floor.as_entire_binding() },
            ],
        });
        let dump = std::env::var_os("JKA_SSR_TILES_DUMP").map(|path| Dump {
            path: path.into(),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Blue Water reflection tiles dump"),
                size: table_bytes(tiles),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            frame: std::sync::atomic::AtomicU32::new(0),
            floor: std::sync::atomic::AtomicU32::new(f32::NAN.to_bits()),
            mapped: Default::default(),
        });
        Self { occupancy: pipeline("occupancy"), sum: pipeline("sum"), group, table, tiles, floor, dump }
    }

    /// This frame's floor (see the module docs): the surface camera's inverse view-projection the depth copy was drawn
    /// with and the floor in its space, or `None` (the camera below it) to mark every surface as upstream does.
    pub fn set_floor(&self, queue: &wgpu::Queue, floor: Option<([[f32; 4]; 4], f32)>) {
        let value = floor.map_or_else(bytemuck::Zeroable::zeroed, |(inverse_view_projection, floor)| TileFloor {
            inverse_view_projection,
            floor: [floor, 1.0, 0.0, 0.0],
        });
        queue.write_buffer(&self.floor, 0, bytemuck::bytes_of(&value));
        if let Some(dump) = &self.dump {
            dump.floor.store(floor.map_or(f32::NAN, |(_, floor)| floor).to_bits(), std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// The table the surface binds at `@group(1) @binding(15)`.
    pub fn table(&self) -> &wgpu::Buffer {
        &self.table
    }

    /// Marks the tiles of this frame's depth copy and sums them (after the copy, before the water draws).
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(LABEL),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_pipeline(&self.occupancy);
        pass.dispatch_workgroups(self.tiles.0, self.tiles.1, 1);
        pass.set_pipeline(&self.sum);
        pass.dispatch_workgroups(1, 1, 1);
        drop(pass);
        if let Some(dump) = &self.dump {
            self.step_dump(dump, encoder);
        }
    }

    fn step_dump(&self, dump: &Dump, encoder: &mut wgpu::CommandEncoder) {
        use std::sync::atomic::Ordering;
        match dump.frame.fetch_add(1, Ordering::Relaxed) {
            DUMP_COPY_FRAME => encoder.copy_buffer_to_buffer(&self.table, 0, &dump.readback, 0, dump.readback.size()),
            DUMP_MAP_FRAME => {
                let mapped = dump.mapped.clone();
                dump.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                    mapped.store(result.is_ok(), Ordering::Release);
                });
            }
            DUMP_READ_FRAME if dump.mapped.load(Ordering::Acquire) => {
                let marks = {
                    let view = dump.readback.slice(..).get_mapped_range().expect("mapped dump");
                    tile_marks(bytemuck::cast_slice(&view), self.tiles)
                };
                dump.readback.unmap();
                let (across, down) = self.tiles;
                let mut image = format!("P5 {across} {down} 255\n").into_bytes();
                image.extend(marks.iter().map(|&marked| if marked { 255u8 } else { 0 }));
                let count = marks.iter().filter(|&&marked| marked).count();
                match std::fs::write(&dump.path, image) {
                    Ok(()) => println!("[SSR TILES] {count}/{} tiles hold a surface (floor {:.2} m) -> {}", marks.len(),
                        f32::from_bits(dump.floor.load(Ordering::Relaxed)), dump.path.display()),
                    Err(error) => println!("[SSR TILES] dump failed: {error}"),
                }
            }
            _ => {}
        }
    }
}

/// Each tile's mark (row by row) from the summed-area `table` (header, then (tiles + 1)² entries).
pub fn tile_marks(table: &[u32], (across, down): (u32, u32)) -> Vec<bool> {
    let width = across + 1;
    let at = |x: u32, y: u32| table[(2 + y * width + x) as usize];
    (0..down).flat_map(|y| (0..across).map(move |x| (x, y)))
        .map(|(x, y)| at(x + 1, y + 1).wrapping_sub(at(x, y + 1)).wrapping_sub(at(x + 1, y)).wrapping_add(at(x, y)) != 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_copy() {
        assert_eq!(tile_counts(2560, 1440), (160, 90));
        assert_eq!(tile_counts(1921, 1081), (121, 68));
        assert_eq!(tile_counts(1, 1), (1, 1));
    }

    /// The shaders and this module agree on the tile.
    #[test]
    fn the_shaders_use_this_tile() {
        let line = format!("const REFLECTION_TILE: u32 = {TILE}u;");
        for (name, source) in [
            ("reflection_tiles.wgsl", SHADER),
            ("screen_reflection.wgsl", include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/screen_reflection.wgsl")),
        ] {
            assert!(source.contains(&line), "{name} lacks `{line}`");
        }
    }

    /// A depth copy with one surface tile: the table counts exactly that tile and its header matches the copy, so
    /// `surface_between` trusts it.
    #[test]
    fn table_marks_only_surface_tiles_on_gpu() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.expect("GPU adapter");
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.expect("GPU device");
            let (width, height) = (40u32, 20u32);
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = depth.create_view(&Default::default());
            // Clear to sky (0), then one pixel of surface (depth 0.5) at (35, 17): tile (2, 1).
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(
                    "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                        let p = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
                        return vec4(p[i], 0.5, 1.0);
                    }".into(),
                ),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: None,
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: None,
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
            let tiles = ReflectionTiles::new(&device, &view, (width, height));
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: table_bytes(tile_counts(width, height)),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let run = || {
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &view,
                            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });
                    pass.set_pipeline(&pipeline);
                    pass.set_scissor_rect(35, 17, 1, 1);
                    pass.draw(0..3, 0..1);
                }
                tiles.encode(&mut encoder);
                encoder.copy_buffer_to_buffer(tiles.table(), 0, &readback, 0, readback.size());
                queue.submit([encoder.finish()]);
                readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
                let data: Vec<u32> = bytemuck::cast_slice(&readback.slice(..).get_mapped_range().expect("mapped")).to_vec();
                readback.unmap();
                data
            };
            let data = run();
            let (across, down) = tile_counts(width, height);
            assert_eq!((data[0], data[1]), (across + 1, down + 1), "header");
            let at = |x: u32, y: u32| data[2 + (y * (across + 1) + x) as usize];
            // Summed-area table: the single marked tile (2, 1) is counted by every entry at or past (3, 2).
            assert_eq!(at(across, down), 1, "one tile marked in total");
            assert_eq!(at(2, down), 0, "tiles left of the surface hold none");
            assert_eq!(at(across, 1), 0, "tiles above the surface hold none");
            assert_eq!(at(3, 2), 1);
            let marks = tile_marks(&data, (across, down));
            assert_eq!(marks.iter().filter(|&&m| m).count(), 1);
            assert!(marks[(1 * across + 2) as usize], "tile (2, 1) marked");
            // The floor: with an identity unprojection the pixel's y is its NDC y, 1 - 2 * 17.5 / 20 = -0.75.
            let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
            tiles.set_floor(&queue, Some((identity, -0.5)));
            let marked = |data: &[u32]| tile_marks(data, (across, down)).iter().filter(|&&m| m).count();
            assert_eq!(marked(&run()), 0, "a surface below the floor is not marked");
            tiles.set_floor(&queue, Some((identity, -1.0)));
            assert_eq!(marked(&run()), 1, "a surface above the floor is marked");
            tiles.set_floor(&queue, None);
            assert_eq!(marked(&run()), 1, "no floor marks every surface");
        });
    }
}
