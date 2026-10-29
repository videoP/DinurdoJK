//! Frontend navigation shares the *main menu's* HDR neon shader and Jimenez
//! bloom chain, without its rain, flicker, or fogged-window treatment. Taffy
//! owns all positions and pointer targets; egui remains input-only here.
use super::{font, saber, upload, Chain, GlyphInstance, ItemLight, ViewUniform, FULLY_LIT, HDR, TITLE_GAS};
use crate::neon_menu::{ChromeFrame, ChromeLine};
use crate::pipeline_jobs::{hash_config as pipeline_hash, PipelineJobKey};
use crate::renderer::Renderer;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FooterUniform {
    screen: [f32; 4],
    back: [f32; 4],
    quit: [f32; 4],
    apply: [f32; 4],
    mode: [f32; 4],
}

struct ChromePipelines {
    footer: wgpu::RenderPipeline,
    signs: wgpu::RenderPipeline,
    blur_down: wgpu::RenderPipeline,
    blur_up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    footer_layout: wgpu::BindGroupLayout,
    signs_layout: wgpu::BindGroupLayout,
    blur_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
}

struct ChromeTargets {
    width: u32,
    height: u32,
    _emission: wgpu::Texture,
    emission: wgpu::TextureView,
    halo: Chain,
    composite_bind: wgpu::BindGroup,
}

const OVER: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
    alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
};
const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
    alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
};

fn make_pipelines(device: &wgpu::Device, format: wgpu::TextureFormat) -> ChromePipelines {
    let uniform = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
            min_binding_size: None }, count: None };
    let storage = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false, min_binding_size: None }, count: None };
    let texture = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
    let sampler = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None };
    let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| device.create_bind_group_layout(
        &wgpu::BindGroupLayoutDescriptor { label: Some(label), entries });
    let footer_layout = layout("Frontend Taffy footer", &[uniform(0)]);
    let signs_layout = layout("Frontend main-menu neon tubes", &[uniform(0), storage(1), storage(2)]);
    let blur_layout = layout("Frontend shared Jimenez bloom", &[texture(0), texture(1), sampler(2)]);
    let composite_layout = layout("Frontend neon emission composite", &[texture(0), texture(1), sampler(2)]);
    let create = |name: &str, group: &wgpu::BindGroupLayout, source: &'static str, fragment: &str,
                  target_format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>,
                  vertices: &[Option<wgpu::VertexBufferLayout<'_>>]| {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name),
            source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(name), bind_group_layouts: &[Some(group)], immediate_size: 0 });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(name), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"),
                compilation_options: Default::default(), buffers: vertices },
            primitive: wgpu::PrimitiveState::default(), depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fragment),
                compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState {
                    format: target_format, blend, write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None, cache: crate::pipeline_cache::get().as_ref(),
        })
    };
    let footer = create("Frontend lightless footer", &footer_layout,
        include_str!("../../neon_menu/neon_footer.wgsl"), "fs_main", format, Some(OVER), &[]);
    let attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Uint32x4];
    // Exactly the same fragment and HDR additive blending used by Main.
    let signs = create("Frontend shared neon signs", &signs_layout,
        include_str!("../../neon_menu/neon_signs.wgsl"), "fs_main", HDR, Some(ADDITIVE),
        &[Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Instance,
            attributes: &attrs })]);
    let blur_source = include_str!("../../neon_menu/neon_blur.wgsl");
    let blur_down = create("Frontend neon bloom down", &blur_layout, blur_source, "fs_down", HDR, None, &[]);
    let blur_up = create("Frontend neon bloom up", &blur_layout, blur_source, "fs_up", HDR, None, &[]);
    let composite = create("Frontend neon bloom composite", &composite_layout,
        include_str!("../../neon_menu/neon_chrome.wgsl"), "fs_main", format, Some(OVER), &[]);
    ChromePipelines { footer, signs, blur_down, blur_up, composite,
        footer_layout, signs_layout, blur_layout, composite_layout }
}

fn create_targets(device: &wgpu::Device, p: &ChromePipelines, width: u32, height: u32) -> ChromeTargets {
    let emission = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Frontend neon HDR emission"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: HDR, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let emission_view = emission.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Frontend neon bloom linear sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let halo = Chain::new(device, &p.blur_layout, &sampler, &emission_view, width, height, "Frontend neon bloom");
    let composite_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Frontend neon emission + halo"), layout: &p.composite_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&emission_view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(halo.result()) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    ChromeTargets { width, height, _emission: emission, emission: emission_view, halo, composite_bind }
}

#[derive(Default)]
pub(super) struct Chrome {
    frame: Option<ChromeFrame>,
    pipeline: Option<ChromePipelines>,
    targets: Option<ChromeTargets>,
    format: Option<wgpu::TextureFormat>,
    view: Option<wgpu::Buffer>,
    bar: Option<wgpu::Buffer>,
    prims: Option<wgpu::Buffer>,
    lights: Option<wgpu::Buffer>,
    instances: Option<wgpu::Buffer>,
    signs_bind: Option<wgpu::BindGroup>,
    bar_bind: Option<wgpu::BindGroup>,
    count: u32,
    previous: Option<(ChromeFrame, u32, u32, u32)>,
}

impl Chrome {
    pub(super) fn set(&mut self, frame: Option<ChromeFrame>) { self.frame = frame; }
    pub(super) fn wanted(&self) -> bool { self.frame.is_some() }

    fn push_letters(line: &ChromeLine, index: u32, w: f32, h: f32,
                    prims: &mut Vec<[f32;4]>, instances: &mut Vec<GlyphInstance>) {
        let (ox,y,cap) = (line.position.x*w,line.position.baseline*h,line.position.cap*h);
        let r = (cap*font::tube_ratio()*line.tube_scale).max(0.9);
        let margin = r*7.0+2.0;
        for (letter, placed) in font::place(&line.text).iter().enumerate() {
            let start=(prims.len()/2) as u32;
            let to_screen = |x: f32, z: f32| [ox+(placed.x+x)*cap,y-z*cap];
            let mut rect=[f32::MAX,f32::MAX,f32::MIN,f32::MIN];
            for prim in placed.glyph.prims.iter() {
                match *prim {
                    font::Prim::Line { a,b } => {
                        let (a,b)=(to_screen(a[0],a[1]),to_screen(b[0],b[1]));
                        prims.push([0.0,a[0],a[1],b[0]]); prims.push([b[1],0.,0.,0.]);
                    }
                    font::Prim::Arc { c,r,start,sweep } => {
                        let center=to_screen(c[0],c[1]);
                        let mut from=-start.to_radians(); let mut arc=-sweep.to_radians();
                        if arc<0. { from+=arc; arc=-arc; }
                        prims.push([1.0,center[0],center[1],r*cap]);
                        prims.push([from,arc,0.,0.]);
                    }
                }
                let b=font::bounds(prim);
                let a=to_screen(b[0],b[3]); let z=to_screen(b[2],b[1]);
                rect=[rect[0].min(a[0]),rect[1].min(a[1]),rect[2].max(z[0]),rect[3].max(z[1])];
            }
            instances.push(GlyphInstance { rect: [rect[0]-margin,rect[1]-margin,rect[2]+margin,rect[3]+margin],
                range: [start,placed.glyph.prims.len() as u32,index,letter as u32] });
        }
    }

    /// The footer's saber is the original main-menu hilt and blade, built from
    /// its *shared* geometry routine and drawn by the same HDR tube shader.
    fn push_saber(lines: &[[[f32; 2]; 2]], tube: f32, index: u32,
                  prims: &mut Vec<[f32; 4]>, instances: &mut Vec<GlyphInstance>) {
        let start = (prims.len()/2) as u32;
        let margin = tube*7.0+2.0;
        let mut bounds=[f32::MAX,f32::MAX,f32::MIN,f32::MIN];
        for [a,b] in lines {
            prims.push([0.,a[0],a[1],b[0]]);
            prims.push([b[1],0.,0.,0.]);
            bounds=[bounds[0].min(a[0].min(b[0])),bounds[1].min(a[1].min(b[1])),
                    bounds[2].max(a[0].max(b[0])),bounds[3].max(a[1].max(b[1]))];
        }
        instances.push(GlyphInstance {
            rect:[bounds[0]-margin,bounds[1]-margin,bounds[2]+margin,bounds[3]+margin],
            range:[start,lines.len() as u32,index,0],
        });
    }

    pub(super) fn prepare(&mut self, r: &mut Renderer, format: wgpu::TextureFormat) {
        let Some(frame)=self.frame.clone() else { return };
        let key=PipelineJobKey::new("neon-chrome",1,pipeline_hash(&format));
        if self.format != Some(format) {
            self.pipeline=None; self.format=Some(format); self.targets=None;
            self.bar_bind=None; self.signs_bind=None;
        }
        if self.pipeline.is_none() {
            if let Some(p)=r.pipeline_jobs.take_ready::<ChromePipelines>(key) { self.pipeline=Some(p); }
            else { let device=r.device.clone();
                r.pipeline_jobs.request(key,"frontend neon chrome",move || make_pipelines(&device,format));
                return;
            }
        }
        let p=self.pipeline.as_ref().expect("obtained above");
        let device=&r.device; let queue=&r.queue;
        let w=r.config.width.max(1); let h=r.config.height.max(1);
        if !self.targets.as_ref().is_some_and(|t|t.width==w && t.height==h) {
            self.targets=Some(create_targets(device,p,w,h));
        }
        let view=ViewUniform { size: [w as f32,h as f32,0.,0.], time:[0.;4] };
        let changed_view=upload(device,queue,&mut self.view,"Frontend neon view",
            wgpu::BufferUsages::UNIFORM,bytemuck::bytes_of(&view));
        let footer=&frame.footer;
        let uni=FooterUniform { screen:[w as f32,h as f32,footer.top,
            (frame.back_hovered as u32 + 2*(frame.quit_hovered as u32) + 4*(frame.apply_hovered as u32)) as f32],
            back:footer.back.map_or([0.;4],|(_,rect)|rect), quit:footer.quit.1,
            apply:footer.apply.map_or([0.;4],|(_,rect)|rect),
            mode: [if frame.dimmed {1.0} else {0.0},0.0,0.0,0.0] };
        let changed_bar=upload(device,queue,&mut self.bar,"Frontend neon footer",
            wgpu::BufferUsages::UNIFORM,bytemuck::bytes_of(&uni));
        if changed_bar || self.bar_bind.is_none() {
            self.bar_bind=Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label:Some("Frontend neon footer"),layout:&p.footer_layout,
                entries:&[wgpu::BindGroupEntry {binding:0, resource:self.bar.as_ref().unwrap().as_entire_binding()}] }));
        }
        let cache=(frame,w,h,font::generation());
        if self.previous.as_ref()!=Some(&cache) || self.signs_bind.is_none() {
            let mut all_prims=Vec::new(); let mut all_instances=Vec::new();
            for (index,line) in cache.0.lines.iter().enumerate() {
                Self::push_letters(line,index as u32,w as f32,h as f32,&mut all_prims,&mut all_instances);
            }
            let logo = saber(cache.0.footer.brand, w as f32, h as f32);
            let hilt_index = cache.0.lines.len() as u32;
            Self::push_saber(&logo.hilt,logo.hilt_tube,hilt_index,&mut all_prims,&mut all_instances);
            Self::push_saber(&[logo.blade],logo.blade_tube,hilt_index+1,&mut all_prims,&mut all_instances);
            let changed_prims=upload(device,queue,&mut self.prims,"Frontend neon tubes",
                wgpu::BufferUsages::STORAGE,bytemuck::cast_slice(&all_prims));
            upload(device,queue,&mut self.instances,"Frontend neon glyphs",
                wgpu::BufferUsages::VERTEX,bytemuck::cast_slice(&all_instances));
            self.count=all_instances.len() as u32;
            let mut lights:Vec<_>=cache.0.lines.iter().map(|line| {
                let brand = line.text == crate::neon_menu::TITLE;
                let gas = if brand { TITLE_GAS } else { line.gas };
                ItemLight {
                    color:[gas[0],gas[1],gas[2], if brand {0.8} else {0.62}],
                    // Steady-state power: no startup sweep, faulty electrode or flicker.
                    state:[line.power,FULLY_LIT,(line.position.cap*h as f32*font::tube_ratio()*line.tube_scale).max(0.9),0.],
                    fault:[-1.,1.,0.,0.],
                }
            }).collect();
            // Original main-menu saber colors and radii, steady rather than
            // replaying the title's ignition each time a submenu is opened.
            lights.push(ItemLight {
                color:[0.72,0.76,0.84,0.8],
                state:[0.85*0.6,FULLY_LIT,logo.hilt_tube,0.],
                fault:[-1.,1.,0.,0.],
            });
            lights.push(ItemLight {
                color:[0.22,0.52,1.0,0.85],
                state:[1.15,FULLY_LIT,logo.blade_tube,0.],
                fault:[-1.,1.,0.,0.],
            });
            let changed_lights=upload(device,queue,&mut self.lights,"Frontend neon sign power",
                wgpu::BufferUsages::STORAGE,bytemuck::cast_slice(&lights));
            if changed_view || changed_prims || changed_lights || self.signs_bind.is_none() {
                self.signs_bind=Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label:Some("Frontend neon HDR signs"),layout:&p.signs_layout,
                    entries:&[
                        wgpu::BindGroupEntry {binding:0,resource:self.view.as_ref().unwrap().as_entire_binding()},
                        wgpu::BindGroupEntry {binding:1,resource:self.prims.as_ref().unwrap().as_entire_binding()},
                        wgpu::BindGroupEntry {binding:2,resource:self.lights.as_ref().unwrap().as_entire_binding()},
                    ] }));
            }
            self.previous=Some(cache);
        } else if changed_view { self.signs_bind=None; self.previous=None; }
    }

    pub(super) fn encode(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: bool) -> bool {
        let (Some(p),Some(bar),Some(signs),Some(targets))=
            (&self.pipeline,&self.bar_bind,&self.signs_bind,&self.targets) else { return false };
        // Preserve main-menu HDR emission and the very same Jimenez multi-mip
        // optical halo; *only* the final composite differs (no rainy glass).
        {
            let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Frontend shared neon HDR emission"),
                color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:&targets.emission,
                    depth_slice:None,resolve_target:None,
                    ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color::BLACK),store:wgpu::StoreOp::Store} })],
                depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None,
            });
            if self.count>0 { if let Some(instances)=&self.instances {
                pass.set_pipeline(&p.signs); pass.set_bind_group(0,signs,&[]);
                pass.set_vertex_buffer(0,instances.slice(..)); pass.draw(0..6,0..self.count);
            }}
        }
        targets.halo.encode(encoder,&p.blur_down,&p.blur_up);
        let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label:Some("Frontend Taffy navigation/footer and HDR neon composite"),
            color_attachments:&[Some(wgpu::RenderPassColorAttachment {view, depth_slice:None,resolve_target:None,
                ops:wgpu::Operations {load:if clear { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) }
                    else { wgpu::LoadOp::Load },store:wgpu::StoreOp::Store} })],
            depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None,
        });
        pass.set_pipeline(&p.footer); pass.set_bind_group(0,bar,&[]); pass.draw(0..6,0..1);
        pass.set_pipeline(&p.composite); pass.set_bind_group(0,&targets.composite_bind,&[]); pass.draw(0..3,0..1);
        true
    }
}
