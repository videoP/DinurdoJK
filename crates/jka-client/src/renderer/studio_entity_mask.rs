//! Render Studio identity masks (not inferred from RGB or radial depth).
//! R: visible player geometry; G: visible dynamic model geometry. The mask
//! uses the same camera, prepared Ghoul2 skin instances and scene depth as
//! the source's final frame. Allocated only for an identity-aware overlap.
use crate::renderer::{
    DynamicModelRenderer, DynamicModelVertex, DynamicWireframeClass, Ghoul2GpuVertex,
    DYNAMIC_MODEL_VERTEX_ATTRIBUTES, GHOUL2_GPU_VERTEX_ATTRIBUTES, DEPTH_FORMAT,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
// Protocol 26 reserves client entity numbers 0..31. NPCs are NOT players.
const MAX_CLIENTS: u16 = 32;

pub(in crate::renderer) struct StudioEntityMask {
    size: (u32, u32, u32),
    _texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    _msaa: Option<wgpu::Texture>,
    msaa_view: Option<wgpu::TextureView>,
    // Opaque and alpha-tested paths for MD3 and GPU-skinned Ghoul2.
    model_all: [wgpu::RenderPipeline;2],
    skin_all: [wgpu::RenderPipeline;2],
    model_players: [wgpu::RenderPipeline;2],
    skin_players: [wgpu::RenderPipeline;2],
}

impl StudioEntityMask {
    pub fn matches(&self, width:u32, height:u32, samples:u32)->bool {
        self.size == (width,height,samples)
    }
    pub fn new(device:&wgpu::Device, models:&DynamicModelRenderer, width:u32, height:u32, samples:u32)->Self {
        let size=(width.max(1),height.max(1),samples.max(1));
        let desc=wgpu::TextureDescriptor {
            label:Some("Render Studio visible entity identity mask"),
            size:wgpu::Extent3d {width:size.0,height:size.1,depth_or_array_layers:1},
            mip_level_count:1, sample_count:1, dimension:wgpu::TextureDimension::D2,
            format:FORMAT, usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats:&[],
        };
        let texture=device.create_texture(&desc);
        let view=texture.create_view(&Default::default());
        let msaa=(size.2>1).then(||device.create_texture(&wgpu::TextureDescriptor {
            label:Some("Render Studio entity identity MSAA"),sample_count:size.2,
            usage:wgpu::TextureUsages::RENDER_ATTACHMENT,..desc
        }));
        let msaa_view=msaa.as_ref().map(|t|t.create_view(&Default::default()));
        Self {
            size, _texture:texture, view, _msaa:msaa, msaa_view,
            model_all:std::array::from_fn(|mask|pipeline(device,&models.pipeline_layout,&models.shader,false,size.2,wgpu::ColorWrites::GREEN,mask!=0)),
            skin_all:std::array::from_fn(|mask|pipeline(device,&models.skin_pipeline_layout,&models.skin_shader,true,size.2,wgpu::ColorWrites::GREEN,mask!=0)),
            model_players:std::array::from_fn(|mask|pipeline(device,&models.pipeline_layout,&models.shader,false,size.2,wgpu::ColorWrites::RED,mask!=0)),
            skin_players:std::array::from_fn(|mask|pipeline(device,&models.skin_pipeline_layout,&models.skin_shader,true,size.2,wgpu::ColorWrites::RED,mask!=0)),
        }
    }
    pub fn encode(&self,device:&wgpu::Device,queue:&wgpu::Queue,models:&DynamicModelRenderer,
        camera:&wgpu::BindGroup, depth:&wgpu::TextureView) {
        let mut encoder=device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label:Some("Render Studio visible entity identity encoder")
        });
        {
            let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label:Some("Render Studio visible actor/entity mask"),
                color_attachments:&[Some(wgpu::RenderPassColorAttachment {
                    view:self.msaa_view.as_ref().unwrap_or(&self.view), depth_slice:None,
                    resolve_target:self.msaa_view.as_ref().map(|_|&self.view),
                    ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),store:wgpu::StoreOp::Store},
                })],
                depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment {
                    view:depth,depth_ops:Some(wgpu::Operations {load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}),
                    stencil_ops:None,
                }), ..Default::default()
            });
            // Green channel: all dynamically drawn model surfaces.
            // Red channel: actor/player model surfaces only. Separate color
            // write masks preserve either category in overlapping geometry.
            self.draw_models(&mut pass,models,camera,false);
            self.draw_models(&mut pass,models,camera,true);
        }
        queue.submit([encoder.finish()]);
    }
    fn draw_models<'a>(&'a self,pass:&mut wgpu::RenderPass<'a>,models:&'a DynamicModelRenderer,
        camera:&'a wgpu::BindGroup,players_only:bool) {
        let model_psos=if players_only {&self.model_players}else{&self.model_all};
        let skin_psos=if players_only {&self.skin_players}else{&self.skin_all};
        let modes=[crate::renderer::DynamicModelAlphaMode::Opaque,crate::renderer::DynamicModelAlphaMode::Mask];
        if !models.draws.is_empty() {
            let (vertices,indices)=models.prepared_dynamic_buffers();
            pass.set_vertex_buffer(0,vertices.slice(..));
            pass.set_index_buffer(indices.slice(..),wgpu::IndexFormat::Uint32);
            pass.set_bind_group(0,camera,&[]);
            for (index,mode) in modes.into_iter().enumerate() {
                pass.set_pipeline(&model_psos[index]);
                pass.set_bind_group(1,&models.fallback_bind_group,&[]);
                for draw in models.draws.iter().filter(|draw|
                    draw.alpha_mode==mode && (!players_only||(draw.wireframe_class==DynamicWireframeClass::Player
                        && draw.entity_num<MAX_CLIENTS))
                ) {
                    if index==1 {
                        let bind=draw.texture_key.as_ref().and_then(|key|models.textures.get(key))
                            .map_or(&models.fallback_bind_group,|texture|&texture.bind_group);
                        pass.set_bind_group(1,bind,&[]);
                    }
                    pass.draw_indexed(draw.first_index..draw.first_index.saturating_add(draw.index_count),draw.base_vertex,0..1);
                }
            }
        }
        if !models.skin_draws.is_empty() {
            pass.set_bind_group(0,camera,&[]);
            pass.set_bind_group(2,&models.skin_bind_group,&[]);
            for (index,mode) in modes.into_iter().enumerate() {
                pass.set_pipeline(&skin_psos[index]);
                pass.set_bind_group(1,&models.fallback_bind_group,&[]);
                for draw in models.skin_draws.iter().filter(|draw|
                    draw.alpha_mode==mode && (!players_only||draw.wireframe_class==DynamicWireframeClass::Player)
                ) {
                    let Some(mesh)=models.ghoul2_meshes.get(&draw.mesh_key) else {continue};
                    if index==1 {
                        let bind=draw.texture_key.as_ref().and_then(|key|models.textures.get(key))
                            .map_or(&models.fallback_bind_group,|texture|&texture.bind_group);
                        pass.set_bind_group(1,bind,&[]);
                    }
                    pass.set_vertex_buffer(0,mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(mesh.index_buffer.slice(..),wgpu::IndexFormat::Uint32);
                    let end=draw.first_instance.saturating_add(draw.instance_count);
                    if !players_only {
                        pass.draw_indexed(0..mesh.index_count,0,draw.first_instance..end);
                    } else {
                        // A skinned draw can batch several entity numbers. The
                        // instance table is authoritative; never include NPCs.
                        let mut cursor=draw.first_instance;
                        while cursor<end {
                            let is_player=|id:u32|models.skin_instance_entities.get(id as usize)
                                .is_some_and(|&entity|entity<MAX_CLIENTS);
                            if !is_player(cursor) {cursor+=1;continue;}
                            let start=cursor;
                            while cursor<end && is_player(cursor) {cursor+=1;}
                            pass.draw_indexed(0..mesh.index_count,0,start..cursor);
                        }
                    }
                }
            }
        }
    }
}

fn pipeline(device:&wgpu::Device,layout:&wgpu::PipelineLayout,shader:&wgpu::ShaderModule,
    skin:bool,samples:u32,writes:wgpu::ColorWrites,masked:bool)->wgpu::RenderPipeline {
    let vertex=if skin {wgpu::VertexBufferLayout {
        array_stride:std::mem::size_of::<Ghoul2GpuVertex>() as u64,
        step_mode:wgpu::VertexStepMode::Vertex,attributes:&GHOUL2_GPU_VERTEX_ATTRIBUTES,
    }} else {wgpu::VertexBufferLayout {
        array_stride:std::mem::size_of::<DynamicModelVertex>() as u64,
        step_mode:wgpu::VertexStepMode::Vertex,attributes:&DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
    }};
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label:Some("Render Studio actor/entity classification pipeline"),layout:Some(layout),
        vertex:wgpu::VertexState {module:shader,entry_point:Some("vs_main"),
            compilation_options:Default::default(),buffers:&[Some(vertex)]},
        fragment:Some(wgpu::FragmentState {module:shader,entry_point:Some(if masked {"fs_studio_identity_alpha"} else {"fs_studio_identity"}),
            compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState {
                format:FORMAT,blend:None,write_mask:writes
            })]}),
        depth_stencil:Some(wgpu::DepthStencilState {
            format:DEPTH_FORMAT,depth_write_enabled:Some(false),
            depth_compare:Some(wgpu::CompareFunction::GreaterEqual),
            stencil:Default::default(),bias:Default::default(),
        }),
        multisample:wgpu::MultisampleState {count:samples,..Default::default()},
        primitive:wgpu::PrimitiveState {cull_mode:None,..Default::default()},
        multiview_mask:None,cache:crate::pipeline_cache::get().as_ref(),
    })
}
