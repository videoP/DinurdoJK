//! Optional GPU-compute lightning with static BSP BVH contacts. This is entirely
//! cosmetic: network prediction, gameplay traces and JKA/jaPRO EFX remain on CPU.
//! No per-frame GPU readback, no global renderer material/pipeline changes.
use crate::renderer::{FxGpuSpriteInstance, StaticAoBvh, StaticAoWorldSource, DynamicModelSurface, TransientLight, Vec3};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

// Exact CPU mirror of the main compute shader's discrete 24-link shape.
// At most two such paths are traced per FX sample, strictly for lighting;
// the GPU still clips all cosmetic forks and crawls against the BSP BVH.
fn arc_hash(v: f32) -> f32 {
    let n = (v*127.1+78.233).sin()*43758.5453;
    n-n.floor()
}
fn noise2(value: f32, key: f32) -> [f32;2] {
    [arc_hash(value*17.71+key)*2.0-1.0,
     arc_hash(value*23.19+key+3.7)*2.0-1.0]
}
fn large_anchor(index: u32,key:f32) -> [f32;2] {
    if index == 0 || index >= 3 { [0.0,0.0] }
    else { noise2(index as f32*3.17,key+2.3) }
}
fn arc_point(a: Vec3, b: Vec3, index: u32, seed: f32, clock: f32) -> Vec3 {
    const SEG: u32 = 24;
    if index == 0 { return a; }
    if index >= SEG { return b; }
    let delta = b-a;
    let length = delta.length();
    if length < 0.00001 { return b; }
    let direction = delta/length;
    let reference = if direction.y.abs()>0.9 { Vec3::Z } else { Vec3::Y };
    let right = direction.cross(reference).normalize_or_zero();
    let other = direction.cross(right);
    let t = index as f32/SEG as f32;
    let key = seed*31.79+(clock*34.0).floor()*113.17;
    let interval=t*3.0;
    let anchor_index=(interval.floor() as u32).min(2);
    let mix = interval.fract();
    let before=large_anchor(anchor_index,key);
    let after=large_anchor(anchor_index+1,key);
    let broad=[before[0]*(1.0-mix)+after[0]*mix,
               before[1]*(1.0-mix)+after[1]*mix];
    let medium_interval=index as f32/3.0;
    let medium_index=medium_interval.floor();
    let medium_mix=medium_interval.fract();
    let before=noise2(medium_index,key+17.9);
    let after=noise2(medium_index+1.0,key+17.9);
    let medium=[before[0]*(1.0-medium_mix)+after[0]*medium_mix,
                before[1]*(1.0-medium_mix)+after[1]*medium_mix];
    let sharp=noise2(index as f32*1.41,key+53.7);
    let envelope=(std::f32::consts::PI*t).sin().max(0.0).powf(0.50);
    let amplitude=(length*0.24+2.0).min(53.0)*envelope;
    let x=(broad[0]*0.68+medium[0]*0.33+sharp[0]*0.22)*amplitude;
    let y=(broad[1]*0.68+medium[1]*0.33+sharp[1]*0.22)*amplitude;
    a.lerp(b,t)+right*x+other*y
}

// One rendered bolt, two branch trees, four two-segment contact crawls.
// Keep this constant in lock-step with lightning_compute.wgsl STRIDE.
pub(super) const OUTPUTS_PER_BOLT: usize = 24*3+12*2+8+1;
const MAX_TRIANGLES: usize = 400_000;
const MAX_BOLTS: usize = 24;
const MAX_LIT_BOLTS: usize = 2;

fn projected_light(
    position: Vec3,
    direction: Vec3,
    radius: f32,
    intensity: f32,
    color: [f32;3],
    inner_degrees: f32,
    outer_degrees: f32,
) -> TransientLight {
    TransientLight {
        kind: crate::fx::system::FxLightKind::AuthoredEffect,
        position: position.to_array(), color, radius,
        source_radius: 0.0, intensity,
        spot: Some(crate::fx::system::FxSpotLight {
            direction: direction.to_array(),
            inner_cone_cos: inner_degrees.to_radians().cos(),
            outer_cone_cos: outer_degrees.to_radians().cos(),
        }),
        segment: None, blade_segments: None,
    }
}

fn plasma_light(position: Vec3, radius:f32, intensity:f32) -> TransientLight {
    TransientLight {
        kind: crate::fx::system::FxLightKind::AuthoredEffect,
        position: position.to_array(), color:[0.16,0.42,1.0], radius,
        source_radius: 0.0, intensity,
        spot: None, segment: None, blade_segments: None,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Node {
    minimum: [f32; 4],
    maximum: [f32; 4],
    meta: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Triangle { a: [f32; 4], b: [f32; 4], c: [f32; 4] }
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct Bolt {
    start: [f32; 4],
    end: [f32; 4],
    shape: [f32; 4],
    rgba: [f32; 4],
}

pub(in crate::renderer) struct LightningGpu {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    nodes: wgpu::Buffer,
    triangles: wgpu::Buffer,
    inputs: wgpu::Buffer,
    input_capacity: usize,
    params: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
    node_count: u32,
    triangle_count: u32,
    // CPU mirror is only used for a handful of FX-sampled impact LIGHTS.
    // The GPU remains authoritative for cosmetic secondary branch contacts.
    cpu_bvh: StaticAoBvh,
    // Start/end must be part of the key as the caster/target can move while
    // the same cg_fxFPS sample remains active.
    impact_key: Vec<[u32; 9]>,
    impact_cache: Vec<TransientLight>,
    pub(super) pending: Vec<Bolt>,
    first_sprite: u32,
}

impl LightningGpu {
    pub(super) fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        source: &StaticAoWorldSource,
    ) -> Option<Self> {
        let triangles = &source.render_triangles;
        if triangles.is_empty() || triangles.len() > MAX_TRIANGLES {
            return None;
        }
        // Reuse the exact opaque BSP triangle set and BVH builder used by
        // baked static AO. Build once, never in a frame's hot FX loop.
        let bvh = StaticAoBvh::build(std::sync::Arc::clone(&source.render_triangles)).ok()?;
        let nodes: Vec<Node> = bvh.nodes.iter().map(|n| Node {
            minimum: [n.minimum[0], n.minimum[1], n.minimum[2], 0.0],
            maximum: [n.maximum[0], n.maximum[1], n.maximum[2], 0.0],
            meta: [n.left, n.right, n.start, n.count],
        }).collect();
        // The BVH leaf ranges refer to the sorted index permutation. Physically
        // reorder triangles here so each shader leaf needs no random index fetch.
        let triangles: Vec<Triangle> = bvh.indices.iter().map(|&index| {
            let t = bvh.triangles[index as usize];
            Triangle {
                a: t.origin.extend(0.0).to_array(),
                b: (t.origin+t.edge1).extend(0.0).to_array(),
                c: (t.origin+t.edge2).extend(0.0).to_array(),
            }
        }).collect();
        let nodes_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA lightning static BSP BVH nodes"),
            contents: bytemuck::cast_slice(&nodes),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let triangle_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA lightning static BSP BVH triangles"),
            contents: bytemuck::cast_slice(&triangles),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA lightning compute BSP + emitter layout"),
            entries: &(0..4).map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: binding != 3 },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }).chain(std::iter::once(wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })).collect::<Vec<_>>(),
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA 3D lightning compute"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lightning_compute.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA 3D lightning compute pipeline layout"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("JKA lightning path and BSP contact compute"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let inputs = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA lightning emitter inputs"),
            size: std::mem::size_of::<Bolt>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA lightning compute bounds"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Some(Self {
            layout, pipeline, nodes: nodes_buffer, triangles: triangle_buffer,
            inputs, input_capacity: 1, params, bind_group: None,
            node_count: nodes.len() as u32,
            triangle_count: triangles.len() as u32,
            cpu_bvh: bvh,
            impact_key: Vec::new(), impact_cache: Vec::new(),
            pending: Vec::new(), first_sprite: 0,
        })
    }

    /// Project electricity into nearby world surfaces using real transient
    /// spotlights, not fake full-screen glare. At an opaque BSP contact, a
    /// second cone aims down the incoming arc at the surface and a short point
    /// flash fills the contact center. The first cone runs even in open space
    /// so the discharge lights nearby geometry along its forward direction.
    ///
    /// Both this cache and the CPU BVH probes advance only for new FX samples
    /// or changed endpoints; unchanged samples do not perform CPU BVH traces.
    pub(in crate::renderer) fn impact_lights(&mut self, surfaces: &[DynamicModelSurface]) -> Vec<TransientLight> {
        let mut candidates: Vec<FxGpuSpriteInstance> = Vec::new();
        let mut key: Vec<[u32; 9]> = Vec::new();
        for surface in surfaces {
            let Some(fx) = surface.fx_gpu_sprites.as_ref() else { continue; };
            if !fx.lightning { continue; }
            for chunk in fx.instances.chunks_exact(5) {
                if candidates.len() >= MAX_LIT_BOLTS { break; }
                let bolt = chunk[0];
                let origin = Vec3::from_slice(&bolt.origin[..3]);
                let end = Vec3::from_slice(&bolt.left[..3]);
                // Wide lightning can contain multiple visually similar authored
                // primitives. Do not pile several cones on the same trajectory.
                if !origin.is_finite() || !end.is_finite() ||
                    candidates.iter().any(|old| {
                        (Vec3::from_slice(&old.origin[..3])-origin).length_squared() < 14.0 * 14.0 &&
                        (Vec3::from_slice(&old.left[..3])-end).length_squared() < 38.0 * 38.0
                    }) { continue; }
                key.push([
                    bolt.origin[0].to_bits(), bolt.origin[1].to_bits(), bolt.origin[2].to_bits(),
                    bolt.left[0].to_bits(), bolt.left[1].to_bits(), bolt.left[2].to_bits(),
                    bolt.left[3].to_bits(), bolt.up[3].to_bits(), bolt.up[0].to_bits(),
                ]);
                candidates.push(bolt);
            }
            if candidates.len() >= MAX_LIT_BOLTS { break; }
        }
        if key == self.impact_key { return self.impact_cache.clone(); }
        self.impact_key = key;
        // Two beam spots, four bend lights, two contact spots and flashes maximum.
        let mut result: Vec<TransientLight> = Vec::with_capacity(MAX_LIT_BOLTS * 3);
        let mut contacts: Vec<Vec3> = Vec::with_capacity(MAX_LIT_BOLTS);
        for first in candidates {
            let origin = Vec3::from_slice(&first.origin[..3]);
            let end = Vec3::from_slice(&first.left[..3]);
            let displacement = end-origin;
            let length = displacement.length();
            if !length.is_finite() || length < 4.0 { continue; }
            let direction = displacement/length;
            let flicker = 0.82+0.18*(first.up[3]*31.0+first.left[3]*19.0).sin().abs();
            if !flicker.is_finite() { continue; }

            // Soft beam-directed illumination, subordinate to the actual
            // bend and impact lights so it doesn't wash out the whole room.
            result.push(projected_light(
                origin + direction*4.0, direction,
                (length*0.72+60.0).clamp(92.0, 300.0),
                0.45*flicker, [0.12,0.37,1.0], 12.0, 29.0,
            ));

            // Short local radiance pools follow two actual bends rather than
            // approximating a kilometer-long arc with one enormous spotlight.
            // These lights remain when the bolt misses BSP, so the environment
            // still responds to nearby electrical energy.
            result.push(plasma_light(arc_point(origin,end,7,first.left[3],first.up[3]),
                                     64.0,0.83*flicker));
            result.push(plasma_light(arc_point(origin,end,17,first.left[3],first.up[3]),
                                     64.0,0.70*flicker));
            // Trace the same 24 linked points as WGSL for the terminal light.
            // Only two lit conductors are sampled; cosmetic forks stay GPU-only.
            let mut previous = origin;
            let mut contact = None;
            let mut incoming = direction;
            for index in 1..=24u32 {
                let next = arc_point(origin,end,index,first.left[3],first.up[3]);
                let edge = next-previous;
                let span = edge.length();
                if span > 0.04 {
                    let edge_dir = edge/span;
                    if let Some(hit) = self.cpu_bvh.trace(previous,edge_dir,span) {
                        if hit > 0.035 && hit < span-0.01 {
                            contact = Some(previous+edge_dir*hit);
                            incoming = edge_dir;
                            break;
                        }
                    }
                }
                previous = next;
            }
            if contact.is_none() {
                contact = self.cpu_bvh.trace(end-direction*1.5,direction,4.0)
                    .map(|hit| end-direction*1.5+direction*hit);
            }
            let Some(hit) = contact else { continue; };
            if !hit.is_finite() || contacts.iter().any(|other| (*other-hit).length_squared() < 48.0*48.0) {
                continue;
            }
            contacts.push(hit);
            // A cone set *upstream* of the collision projects a real circular
            // pool onto the struck surface; its direction follows the final
            // jagged segment, rather than a fixed world-space axis.
            result.push(projected_light(
                hit-incoming*32.0, incoming, 152.0,
                1.65*flicker, [0.20,0.48,1.0], 21.0, 43.0,
            ));
            // Short, softer point flash fills the exact electrical contact.
            result.push(TransientLight {
                kind: crate::fx::system::FxLightKind::AuthoredEffect,
                position: (hit-incoming*1.5).to_array(),
                color: [0.17,0.45,1.0], radius: 110.0,
                source_radius: 0.0, intensity: 1.75*flicker,
                spot: None, segment: None, blade_segments: None,
            });
        }
        self.impact_cache = result.clone();
        result
    }

    /// The existing FX backend generated five layers per conductor. Extract
    /// one authoritative start/end and the material colour; WGPU computes all
    /// layers/contact sparks directly into the usual instance vertex buffer.
    pub(super) fn collect(
        &mut self,
        draws: &mut [super::super::FxGpuSpritePreparedDraw],
        packed: &[FxGpuSpriteInstance],
    ) -> usize {
        self.pending.clear();
        self.first_sprite = packed.len() as u32;
        for draw in draws.iter_mut().filter(|draw| draw.lightning) {
            let first = draw.first_instance as usize;
            let end = first.saturating_add(draw.instance_count as usize);
            if end > packed.len() || (end-first)%5 != 0 ||
                self.pending.len() + (end-first)/5 > MAX_BOLTS {
                // Preserve original non-compute draw when the budget is full.
                continue;
            }
            let output_first = packed.len() + self.pending.len()*OUTPUTS_PER_BOLT;
            let mut inserted = 0usize;
            for chunk in packed[first..end].chunks_exact(5) {
                let p = chunk[0];
                let origin=Vec3::from_slice(&p.origin[..3]);
                let end_pos=Vec3::from_slice(&p.left[..3]);
                if !origin.is_finite() || !end_pos.is_finite() { continue; }
                // Authored EFX sometimes repeats almost identical paths. They
                // shouldn't become overlapping white beams after GPU expansion.
                let duplicate = self.pending.iter().any(|previous| {
                    (Vec3::from_slice(&previous.start[..3])-origin).length_squared()<12.0*12.0 &&
                    (Vec3::from_slice(&previous.end[..3])-end_pos).length_squared()<24.0*24.0
                });
                if duplicate { continue; }
                let color = chunk[1].color;
                self.pending.push(Bolt {
                    start: p.origin, end: p.left,
                    shape: [p.up[0]/3.2, p.up[2], p.left[3], p.up[3]],
                    rgba: color.map(|v| v/0.68),
                });
                inserted += 1;
            }
            draw.first_instance = output_first as u32;
            draw.instance_count = (inserted*OUTPUTS_PER_BOLT) as u32;
            draw.lightning = false; // compute outputs six-vertex sprites
        }
        packed.len() + self.pending.len()*OUTPUTS_PER_BOLT
    }

    pub(super) fn upload(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        output: &wgpu::Buffer,
    ) {
        if self.pending.is_empty() { self.bind_group = None; return; }
        if self.pending.len() > self.input_capacity {
            self.input_capacity = self.pending.len().next_power_of_two();
            self.inputs = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA lightning source bolt array"),
                size: (self.input_capacity * std::mem::size_of::<Bolt>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.inputs, 0, bytemuck::cast_slice(&self.pending));
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&[
            self.pending.len() as u32, self.first_sprite, self.node_count, self.triangle_count,
        ]));
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA lightning collision compute bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.nodes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.triangles.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.inputs.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: output.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: self.params.as_entire_binding() },
            ],
        }));
    }

    pub(super) fn encode(&self, encoder: &mut wgpu::CommandEncoder, camera: &wgpu::BindGroup) {
        let Some(bind_group) = &self.bind_group else { return; };
        if self.pending.is_empty() { return; }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("JKA static-geometry Force Lightning contacts"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, bind_group, &[]);
        pass.dispatch_workgroups(self.pending.len() as u32, 1, 1);
    }
}
