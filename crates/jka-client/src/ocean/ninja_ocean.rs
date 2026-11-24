//! Optional Ocean2 render/simulation adapter. All authored material expressions
//! and fluid stages live in the private provider. This module owns only host
//! coordinate transforms, resources, FFT crest injection and render scheduling.
use jka_ninja::{OceanDesc, OceanField, OceanFrame, WAKE_MAX_TRACKS as MAX_TRACKS};
use jka_river::river_ninja::{ActorHistory, ActorSample, checked_scope, timeline_epoch, wake_points};
use blue_water_gpu::{geometry, original_surface::OriginalSurfacePipeline, shader_composer};
use super::{OCEAN_MAX_SURFACES, OCEAN_UNITS_PER_METER};

const EXTENT_M: f32 = 300.0;
const CM_PER_UNIT: f32 = 100.0 / OCEAN_UNITS_PER_METER;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Authority {
    pub add_2_const_b: f32,
    pub divide_10_const_a: f32,
    pub subtract_9_const_a: f32,
    pub tile1_srgb: bool,
    pub tile2_srgb: bool,
    // Host cost control, not an upstream parameter override claim.
    #[serde(default = "reference_resolution")]
    pub resolution: u32,
}
fn reference_resolution() -> u32 { 2048 }
impl Authority {
    /// Coast has no omitted-class-default constants; only the host resolution applies.
    pub fn coast_reference() -> Self {
        Self { add_2_const_b: 1.0, divide_10_const_a: 1.0, subtract_9_const_a: 1.0,
            tile1_srgb: false, tile2_srgb: false, resolution: reference_resolution() }
    }
}
impl Authority {
    pub fn validate(&self) -> Result<(),String> {
        if ![self.add_2_const_b,self.divide_10_const_a,self.subtract_9_const_a].iter().all(|x|x.is_finite())
            || !matches!(self.resolution, 256|512|1024|2048) {
            return Err("Ocean2 invalid authority constants or resolution (256/512/1024/2048)".into());
        }
        Ok(())
    }
}
pub(crate) struct Assets {
    pub simulation: [wgpu::TextureView; 4],
    pub material: [wgpu::TextureView; 4],
    pub authority: Authority,
    /// Coast only: WATER `CausticsTileComponentVolumeTexture` (None when the asset is missing).
    pub caustics: Option<wgpu::TextureView>,
}

pub(super) fn uniform(binding:u32, visibility:wgpu::ShaderStages)->wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty:wgpu::BindingType::Buffer {
        ty:wgpu::BufferBindingType::Uniform, has_dynamic_offset:false,min_binding_size:None }, count:None }
}
pub(super) fn texture(binding:u32, visibility:wgpu::ShaderStages)->wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {binding,visibility,ty:wgpu::BindingType::Texture {sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None}
}
pub(super) fn sampler(binding:u32, visibility:wgpu::ShaderStages)->wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {binding,visibility,ty:wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),count:None}
}
pub(super) fn storage(binding:u32, read_only:bool)->wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry{binding,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only},has_dynamic_offset:false,min_binding_size:None},count:None}
}
pub(super) fn buf(device:&wgpu::Device,label:&str,size:u64,usage:wgpu::BufferUsages)->wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor{label:Some(label),size,usage,mapped_at_creation:false})
}
pub(super) fn b(binding:u32, v:&wgpu::Buffer)->wgpu::BindGroupEntry<'_>{wgpu::BindGroupEntry{binding,resource:v.as_entire_binding()}}
pub(super) fn t(binding:u32, v:&wgpu::TextureView)->wgpu::BindGroupEntry<'_>{wgpu::BindGroupEntry{binding,resource:wgpu::BindingResource::TextureView(v)}}
pub(super) fn s(binding:u32, v:&wgpu::Sampler)->wgpu::BindGroupEntry<'_>{wgpu::BindGroupEntry{binding,resource:wgpu::BindingResource::Sampler(v)}}

struct Slot {
    center:[f32;2], plane:f32, field:OceanField,
    params:wgpu::Buffer, capture_params:wgpu::Buffer, eye_params:wgpu::Buffer, probe_params:wgpu::Buffer, probe_camera:wgpu::Buffer, probe_eye:wgpu::Buffer, crest:wgpu::Texture,
    crest_view:wgpu::TextureView, points:wgpu::Buffer, flags:wgpu::Buffer,
    capture_group:Option<(wgpu::TextureView,wgpu::BindGroup)>,
    draw_group:Option<(wgpu::TextureView,wgpu::TextureView,wgpu::BindGroup)>,
}
impl Slot {
    fn new(device:&wgpu::Device, assets:&Assets, center:[f32;2], plane:f32)->Result<Self,String>{
        let field=OceanField::new(device,&OceanDesc{resolution:[assets.authority.resolution;2],extent_cm:[EXTENT_M*100.0;2],max_tracks:MAX_TRACKS as u32},assets.simulation.each_ref())?;
        let [width,height]=field.capture_resolution();
        let mem=device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let val=device.push_error_scope(wgpu::ErrorFilter::Validation);
        let crest=device.create_texture(&wgpu::TextureDescriptor{label:Some("Ocean2 pre-limit crest density"),size:wgpu::Extent3d{width,height,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba16Float,usage:wgpu::TextureUsages::STORAGE_BINDING|wgpu::TextureUsages::COPY_SRC,view_formats:&[]});
        let crest_view=crest.create_view(&Default::default());
        let uniform=wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST;
        let out=Self{center,plane,field,params:buf(device,"Ocean2 draw params",32,uniform),capture_params:buf(device,"Ocean2 capture params",48,uniform),eye_params:buf(device,"Ocean2 eye params",48,uniform),probe_params:buf(device,"Ocean2 probe params",48,uniform),probe_camera:buf(device,"Ocean2 probe camera",144,uniform),probe_eye:buf(device,"Ocean2 probe eye cache",16,wgpu::BufferUsages::STORAGE),crest,crest_view,
            points:buf(device,"Ocean2 contact points",(MAX_TRACKS*16) as u64,wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_DST),
            flags:buf(device,"Ocean2 contact flags",(MAX_TRACKS*4) as u64,wgpu::BufferUsages::STORAGE),capture_group:None,draw_group:None};
        let a=checked_scope(val);let m=checked_scope(mem);a?;m?;Ok(out)
    }
}

pub(super) struct Renderer {
    pub pipeline:wgpu::RenderPipeline,
    pub wire:Option<wgpu::RenderPipeline>,
    shader:wgpu::ShaderModule, pipeline_layout:wgpu::PipelineLayout,
    draw_layout:wgpu::BindGroupLayout,capture_layout:wgpu::BindGroupLayout,
    crest_pipeline:wgpu::ComputePipeline,contacts_pipeline:wgpu::ComputePipeline,empty:wgpu::BindGroup,
    height_layout:wgpu::BindGroupLayout, eye_layout:wgpu::BindGroupLayout, probe_layout:wgpu::BindGroupLayout,
    boundary_layout:wgpu::BindGroupLayout, eye_pipeline:wgpu::ComputePipeline, probe_eye_pipeline:wgpu::ComputePipeline, probe_pipeline:wgpu::ComputePipeline,
    clamp:wgpu::Sampler,repeat:wgpu::Sampler,gf:wgpu::TextureView,zero:wgpu::TextureView,
    assets:Option<Assets>,slots:[Option<Slot>;OCEAN_MAX_SURFACES],failed:[bool;OCEAN_MAX_SURFACES],
    unavailable:bool, history:ActorHistory, epoch:u64,last_time:Option<f32>,delta:f64,
    fft:bool,format:wgpu::TextureFormat,samples:u32,
}
pub(super) fn draw_pipeline(device:&wgpu::Device,layout:&wgpu::PipelineLayout,shader:&wgpu::ShaderModule,format:wgpu::TextureFormat,samples:u32,wire:bool)->wgpu::RenderPipeline{
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{label:Some("Ocean2 native surface"),layout:Some(layout),vertex:wgpu::VertexState{module:shader,entry_point:Some("vertex"),compilation_options:Default::default(),buffers:&[Some(geometry::OceanVertex::LAYOUT)]},fragment:Some(wgpu::FragmentState{module:shader,entry_point:Some(if wire{"ninja_wire"}else{"fragment"}),compilation_options:Default::default(),targets:&[Some(format.into())]}),primitive:wgpu::PrimitiveState{topology:wgpu::PrimitiveTopology::TriangleList,front_face:wgpu::FrontFace::Ccw,cull_mode:None,polygon_mode:if wire{wgpu::PolygonMode::Line}else{wgpu::PolygonMode::Fill},..Default::default()},depth_stencil:Some(wgpu::DepthStencilState{format:wgpu::TextureFormat::Depth32Float,depth_write_enabled:Some(!wire),depth_compare:Some(wgpu::CompareFunction::GreaterEqual),stencil:Default::default(),bias:Default::default()}),multisample:wgpu::MultisampleState{count:samples,..Default::default()},multiview_mask:None,cache:None})
}
impl Renderer {
    pub fn new(device:&wgpu::Device,queue:&wgpu::Queue,waves:&wgpu::BindGroupLayout,base:&OriginalSurfacePipeline,format:wgpu::TextureFormat,samples:u32,steps:u32,ssr:bool,fft:bool)->Result<Self,String>{
        let material=jka_ninja::ocean_material_wgsl().ok_or("Fluid Ninja private provider not included")?;
        let slw=jka_water::river::slw::SHADER;
        let module=shader_composer::ninja_ocean_module(&material,slw,steps,ssr)?;
        let capture_module=shader_composer::ninja_ocean_capture_module(&material)?;
        let memory=device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation=device.push_error_scope(wgpu::ErrorFilter::Validation);
        let both=wgpu::ShaderStages::VERTEX_FRAGMENT;
        let compute=wgpu::ShaderStages::COMPUTE;
        let draw_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("Ocean2 material inputs"),entries:&[uniform(0,both),texture(1,both),texture(2,both),texture(3,both),texture(4,both),texture(5,both),texture(6,both),sampler(7,both),sampler(8,both),texture(9,both)]});
        let pipeline_layout=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:Some("Ocean2 surface layout"),bind_group_layouts:&[Some(&base.camera_layout),Some(&base.surface_layout),Some(waves),Some(&base.environment_layout),Some(&draw_layout)],immediate_size:0});
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("Ocean2 surface"),source:wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module))});
        let pipeline=draw_pipeline(device,&pipeline_layout,&shader,format,samples,false);
        let capture_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("Ocean2 crest/contact capture"),entries:&[uniform(0,compute),wgpu::BindGroupLayoutEntry{binding:1,visibility:compute,ty:wgpu::BindingType::StorageTexture{access:wgpu::StorageTextureAccess::WriteOnly,format:wgpu::TextureFormat::Rgba16Float,view_dimension:wgpu::TextureViewDimension::D2},count:None},storage(2,true),storage(3,false),texture(4,compute),texture(5,compute),texture(6,compute),sampler(7,compute),sampler(8,compute)]});
        let empty_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("Ocean2 empty capture group"),entries:&[]});
        let empty=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("Ocean2 empty capture group"),layout:&empty_layout,entries:&[]});
        let capture_pl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:Some("Ocean2 capture"),bind_group_layouts:&[Some(&capture_layout),Some(&empty_layout),Some(waves)],immediate_size:0});
        let capture_shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("Ocean2 capture"),source:wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(capture_module))});
        let make=|entry|device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor{label:Some(entry),layout:Some(&capture_pl),module:&capture_shader,entry_point:Some(entry),compilation_options:Default::default(),cache:None});
        let crest_pipeline=make("crest");let contacts_pipeline=make("contacts");
        let height_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("Ocean2 height inputs"),entries:&[uniform(0,compute),texture(4,compute),texture(5,compute),texture(6,compute),sampler(7,compute),sampler(8,compute)]});
        let eye_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("Ocean2 eye output"),entries:&[storage(0,false)]});
        let boundary_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("Ocean2 probe boundary"),entries:&[uniform(16,compute)]});
        let probe_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("Ocean2 probe output"),entries:&[uniform(1,compute),wgpu::BindGroupLayoutEntry{binding:2,visibility:compute,ty:wgpu::BindingType::StorageTexture{access:wgpu::StorageTextureAccess::WriteOnly,format:wgpu::TextureFormat::Rgba16Float,view_dimension:wgpu::TextureViewDimension::D2},count:None},storage(3,false)]});
        let height_pipeline=|entry, middle:&wgpu::BindGroupLayout, output:&wgpu::BindGroupLayout| {
            let layout=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:Some(entry),bind_group_layouts:&[Some(&height_layout),Some(middle),Some(waves),Some(output)],immediate_size:0});
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor{label:Some(entry),layout:Some(&layout),module:&capture_shader,entry_point:Some(entry),compilation_options:Default::default(),cache:None})
        };
        let eye_pipeline=height_pipeline("eye_height",&empty_layout,&eye_layout);
        let probe_eye_pipeline=height_pipeline("underwater_eye_height",&boundary_layout,&probe_layout);
        let probe_pipeline=height_pipeline("underwater_probe",&boundary_layout,&probe_layout);
        let clamp=device.create_sampler(&wgpu::SamplerDescriptor{label:Some("Ocean2 clamp"),mag_filter:wgpu::FilterMode::Linear,min_filter:wgpu::FilterMode::Linear,mipmap_filter:wgpu::MipmapFilterMode::Linear,..Default::default()});
        let repeat=device.create_sampler(&wgpu::SamplerDescriptor{label:Some("Ocean2 wrap"),address_mode_u:wgpu::AddressMode::Repeat,address_mode_v:wgpu::AddressMode::Repeat,mag_filter:wgpu::FilterMode::Linear,min_filter:wgpu::FilterMode::Linear,mipmap_filter:wgpu::MipmapFilterMode::Linear,..Default::default()});
        let gf=jka_river::river_slw::PreintegratedGf::new(device,queue).texture.create_view(&Default::default());
        let zero=device.create_texture(&wgpu::TextureDescriptor{label:Some("Ocean2 initial unadvanced sim"),size:wgpu::Extent3d{width:1,height:1,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba16Float,usage:wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]}).create_view(&Default::default());
        let v=checked_scope(validation);let m=checked_scope(memory);v?;m?;
        Ok(Self{pipeline,wire:None,shader,pipeline_layout,draw_layout,capture_layout,crest_pipeline,contacts_pipeline,empty,height_layout,eye_layout,probe_layout,boundary_layout,eye_pipeline,probe_eye_pipeline,probe_pipeline,clamp,repeat,gf,zero,assets:None,slots:std::array::from_fn(|_|None),failed:[false;OCEAN_MAX_SURFACES],unavailable:false,history:ActorHistory::default(),epoch:timeline_epoch(),last_time:None,delta:0.0,fft,format,samples})
    }
    pub fn begin(&mut self,device:&wgpu::Device,time:f32,samples:&[ActorSample],wire:bool,load:impl FnOnce()->Result<Assets,String>){
        if self.unavailable{return;}
        if self.assets.is_none(){match load(){Ok(a)=>self.assets=Some(a),Err(e)=>{eprintln!("[OCEAN NINJA] unavailable: {e}; switch material to retry");self.unavailable=true;return;}}}
        let epoch=timeline_epoch();
        let delta=self.last_time.map_or(1.0/60.0,|t|f64::from(time-t));
        if self.epoch!=epoch || !delta.is_finite() || !(0.0..=0.5).contains(&delta){self.slots=std::array::from_fn(|_|None);self.failed=[false;OCEAN_MAX_SURFACES];self.history.reset();self.epoch=epoch;self.delta=0.0;}else{self.delta=delta;}
        self.last_time=Some(time);self.history.update(samples.iter().copied());
        if wire && self.wire.is_none() && device.features().contains(wgpu::Features::POLYGON_MODE_LINE){
            let scope=device.push_error_scope(wgpu::ErrorFilter::Validation);
            let p=draw_pipeline(device,&self.pipeline_layout,&self.shader,self.format,self.samples,true);
            if checked_scope(scope).is_ok(){self.wire=Some(p);}
        }
    }
    fn height_group(&self,device:&wgpu::Device,slot:&Slot,params:&wgpu::Buffer)->Option<wgpu::BindGroup>{
        let a=self.assets.as_ref()?;
        let pd=slot.field.outputs().pressure_divergence.unwrap_or(&self.zero);
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("Ocean2 height inputs"),layout:&self.height_layout,entries:&[b(0,params),t(4,pd),t(5,&a.material[2]),t(6,&a.material[3]),s(7,&self.clamp),s(8,&self.repeat)]}))
    }
    pub fn encode_eye(&self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,index:usize,waves:&wgpu::BindGroup,eye:[f32;3],time:f32,spacing:f32,result:&wgpu::Buffer)->bool{
        if self.epoch!=timeline_epoch() {return false;}
        let Some(slot)=self.slots[index].as_ref() else{return false};
        let Some(group)=self.height_group(device,slot,&slot.eye_params) else{return false};
        queue.write_buffer(&slot.eye_params,0,bytemuck::cast_slice(&[[slot.center[0],slot.center[1],EXTENT_M,slot.plane/OCEAN_UNITS_PER_METER],[eye[0],eye[1]-slot.plane/OCEAN_UNITS_PER_METER,eye[2],time],[if self.fft{1.0}else{0.0},0.0,spacing,0.0]]));
        let output=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("Ocean2 eye result"),layout:&self.eye_layout,entries:&[b(0,result)]});
        let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("Ocean2 combined eye height"),timestamp_writes:None});
        pass.set_pipeline(&self.eye_pipeline);pass.set_bind_group(0,&group,&[]);pass.set_bind_group(1,&self.empty,&[]);pass.set_bind_group(2,waves,&[]);pass.set_bind_group(3,&output,&[]);pass.dispatch_workgroups(1,1,1);true
    }
    pub fn encode_probe(&self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,index:usize,waves:&wgpu::BindGroup,camera:&blue_water_gpu::original_surface::SurfaceCamera,spacing:f32,near:bool,boundary:&wgpu::Buffer,target:&wgpu::TextureView){
        let Some(slot)=self.slots[index].as_ref() else{return};
        let Some(group)=self.height_group(device,slot,&slot.probe_params) else{return};
        queue.write_buffer(&slot.probe_params,0,bytemuck::cast_slice(&[[slot.center[0],slot.center[1],EXTENT_M,slot.plane/OCEAN_UNITS_PER_METER],camera.position_time,[if self.fft{1.0}else{0.0},0.0,spacing,0.0]]));
        let mut params=[[0.0f32;4];9];params[..4].copy_from_slice(&camera.inverse_projection);params[4..8].copy_from_slice(&camera.inverse_view);params[8]=[camera.position_time[0],camera.position_time[1],camera.position_time[2],if near{camera.near}else{0.0}];
        queue.write_buffer(&slot.probe_camera,0,bytemuck::cast_slice(&params));
        let bounds=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("Ocean2 water column"),layout:&self.boundary_layout,entries:&[b(16,boundary)]});
        let output=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("Ocean2 underwater probe"),layout:&self.probe_layout,entries:&[b(1,&slot.probe_camera),t(2,target),b(3,&slot.probe_eye)]});
        let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("Ocean2 combined waterline"),timestamp_writes:None});
        pass.set_bind_group(0,&group,&[]);
        pass.set_bind_group(1,&bounds,&[]);
        pass.set_bind_group(2,waves,&[]);
        pass.set_bind_group(3,&output,&[]);
        if near {
            pass.set_pipeline(&self.probe_eye_pipeline);
            pass.dispatch_workgroups(1,1,1);
        }
        pass.set_pipeline(&self.probe_pipeline);
        pass.dispatch_workgroups(16,9,1);
    }
    pub fn group(&self,index:usize)->Option<&wgpu::BindGroup>{self.slots[index].as_ref()?.draw_group.as_ref().map(|(_,_,g)|g)}

    /// The parameter buffer identifies this field even if its pool is reused.
    pub fn eye_source(&self, index: usize) -> Option<&wgpu::Buffer> {
        if self.epoch != timeline_epoch() {
            return None;
        }
        let slot = self.slots.get(index)?.as_ref()?;
        slot.draw_group.as_ref()?;
        Some(&slot.eye_params)
    }
    pub fn prepare(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,index:usize,waves:&wgpu::BindGroup,centre:[f32;2],plane:f32,eye:[f32;3],time:f32,spacing:f32,mask:&super::OceanMasks){
        if self.unavailable||self.failed[index]{return;}
        if let Err(e)=self.prepare_inner(device,queue,encoder,index,waves,centre,plane,eye,time,spacing,mask){
            eprintln!("[OCEAN NINJA] pool {index} unavailable: {e}; switch material to retry");self.slots[index]=None;self.failed[index]=true;
        }
    }
    fn prepare_inner(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,index:usize,waves:&wgpu::BindGroup,centre:[f32;2],plane:f32,eye:[f32;3],time:f32,spacing:f32,mask:&super::OceanMasks)->Result<(),String>{
        let Some(a)=self.assets.as_ref() else{return Ok(())};
        if self.slots[index].as_ref().is_none_or(|s|s.plane!=plane||(s.center[0]-centre[0]).abs().max((s.center[1]-centre[1]).abs())>EXTENT_M*0.25){self.slots[index]=Some(Slot::new(device,a,centre,plane)?);}
        let slot=self.slots[index].as_mut().expect("initialized slot");
        let domain=[slot.center[0],slot.center[1],EXTENT_M,plane/OCEAN_UNITS_PER_METER];
        let defaults=[a.authority.add_2_const_b,a.authority.divide_10_const_a,a.authority.subtract_9_const_a,if self.fft{1.0}else{0.0}];
        queue.write_buffer(&slot.params,0,bytemuck::cast_slice(&[domain,defaults]));
        let due=slot.field.wants_step(self.delta,self.fft)?;
        let mut tracks=Vec::new();let mut points=Vec::new();
        if due {
            // Shared actor wake (river_ninja::wake_points): joints or hull centre,
            // each contact-tested against its own water column.
            let center=slot.center;
            let rel=|q:[f32;3]|[q[0]*CM_PER_UNIT-center[0]*100.0,q[1]*CM_PER_UNIT+center[1]*100.0,(q[2]-plane)*CM_PER_UNIT];
            let mut wake=Vec::new();
            for tr in self.history.tracks(){
                let p=tr.current.origin_map;
                if !mask.contains(index,[p[0],-p[1]]){continue;}
                wake.clear();wake_points(tr,&mut wake);
                for w in &wake{
                    let position=rel(w.position_map);
                    if position[0].abs().max(position[1].abs())>=EXTENT_M*50.0{continue;}
                    tracks.push(w.track(rel));
                    let q=w.position_map;
                    points.push([q[0]/OCEAN_UNITS_PER_METER,-q[1]/OCEAN_UNITS_PER_METER,(w.mins_map[2]-plane)/OCEAN_UNITS_PER_METER,(w.maxs_map[2]-plane)/OCEAN_UNITS_PER_METER]);
                }
            }
            let previous=slot.field.outputs().pressure_divergence.unwrap_or(&self.zero);
            if slot.capture_group.as_ref().is_none_or(|(v,_)|v!=previous){
                let g=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("Ocean2 capture inputs"),layout:&self.capture_layout,entries:&[b(0,&slot.capture_params),t(1,&slot.crest_view),b(2,&slot.points),b(3,&slot.flags),t(4,previous),t(5,&a.material[2]),t(6,&a.material[3]),s(7,&self.clamp),s(8,&self.repeat)]});
                slot.capture_group=Some((previous.clone(),g));
            }
            queue.write_buffer(&slot.capture_params,0,bytemuck::cast_slice(&[domain,[eye[0],eye[1],eye[2],time],[defaults[3],tracks.len() as f32,spacing.max(0.01),0.0]]));
            if !points.is_empty(){queue.write_buffer(&slot.points,0,bytemuck::cast_slice(&points));}
            if self.fft||!points.is_empty(){
                let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("Ocean2 FFT crest + actor contact"),timestamp_writes:None});
                if self.fft{pass.set_pipeline(&self.crest_pipeline);pass.set_bind_group(0,&slot.capture_group.as_ref().unwrap().1,&[]);pass.set_bind_group(1,&self.empty,&[]);pass.set_bind_group(2,waves,&[]);let [w,h]=slot.field.capture_resolution();pass.dispatch_workgroups(w.div_ceil(8),h.div_ceil(8),1);}
                if !points.is_empty(){pass.set_pipeline(&self.contacts_pipeline);pass.set_bind_group(0,&slot.capture_group.as_ref().unwrap().1,&[]);pass.set_bind_group(1,&self.empty,&[]);pass.set_bind_group(2,waves,&[]);pass.dispatch_workgroups((points.len() as u32).div_ceil(64),1,1);}
            }
        }
        slot.field.encode(device,queue,encoder,&OceanFrame{game_delta:self.delta,tracks:&tracks,frame_velocity_cm_s:[0.0;3],contacts:Some(&slot.flags),crest_source:self.fft.then_some(&slot.crest)})?;
        let o=slot.field.outputs();
        let (vd,pd)=(o.velocity_density.unwrap_or(&self.zero),o.pressure_divergence.unwrap_or(&self.zero));
        if slot.draw_group.as_ref().is_none_or(|(v,p,_)|v!=vd||p!=pd){
            let g=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("Ocean2 surface graph inputs"),layout:&self.draw_layout,entries:&[b(0,&slot.params),t(1,vd),t(2,pd),t(3,&a.material[0]),t(4,&a.material[1]),t(5,&a.material[2]),t(6,&a.material[3]),s(7,&self.clamp),s(8,&self.repeat),t(9,&self.gf)]});
            slot.draw_group=Some((vd.clone(),pd.clone(),g));
        }
        Ok(())
    }
}

/// The selected private Fluid Ninja ocean preset (`r_ninjaOceanPreset`):
/// Ocean2 (Water_Sparse_Sea_Ship) or Coast (Water_Dense_Sea, 100 m).
pub(super) enum Surface { Ocean2(Renderer), Coast(super::ninja_coast::Renderer) }
macro_rules! each { ($self:expr, $r:ident => $e:expr) => { match $self { Surface::Ocean2($r) => $e, Surface::Coast($r) => $e } } }
impl Surface {
    pub fn new(coast:bool,device:&wgpu::Device,queue:&wgpu::Queue,waves:&wgpu::BindGroupLayout,base:&OriginalSurfacePipeline,format:wgpu::TextureFormat,samples:u32,steps:u32,ssr:bool,fft:bool,open_sea:bool)->Result<Self,String>{
        Ok(if coast { Self::Coast(super::ninja_coast::Renderer::new(device,queue,waves,base,format,samples,steps,ssr,fft,open_sea)?) }
        else { Self::Ocean2(Renderer::new(device,queue,waves,base,format,samples,steps,ssr,fft)?) })
    }
    pub fn pipeline(&self)->&wgpu::RenderPipeline{each!(self,r=>&r.pipeline)}
    pub fn wire(&self)->Option<&wgpu::RenderPipeline>{each!(self,r=>r.wire.as_ref())}
    /// Coast landscape wet-map receiver; Ocean2 has none.
    pub fn wet(&self)->Option<&wgpu::RenderPipeline>{match self{Self::Coast(r)=>Some(&r.wet),Self::Ocean2(_)=>None}}
    /// Coast r_ninjaCoastDeepWater deep-water pass, drawn before `pipeline`.
    pub fn deep(&self)->Option<&wgpu::RenderPipeline>{match self{Self::Coast(r)=>r.deep.as_ref(),Self::Ocean2(_)=>None}}
    /// `load(coast)` returns that preset's authored inputs.
    pub fn begin(&mut self,device:&wgpu::Device,time:f32,samples:&[ActorSample],wire:bool,load:impl FnOnce(bool)->Result<Assets,String>){
        match self { Self::Ocean2(r)=>r.begin(device,time,samples,wire,||load(false)), Self::Coast(r)=>r.begin(device,time,samples,wire,||load(true)) }
    }
    pub fn set_terrain_source(&mut self,source:Option<&std::sync::Arc<super::upstream_coast::Source>>,debug:u8,dump:u32,swell_deg:f32,
        tint:[f32;3],transparency:f32,coast:[f32;12],lod:[[f32;4];2],detail:[f32;4]){
        if let Self::Coast(r)=self { r.set_terrain_source(source,debug,dump,swell_deg,tint,transparency,coast,lod,detail); }
    }
    pub fn prepare(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,index:usize,waves:&wgpu::BindGroup,centre:[f32;2],plane:f32,eye:[f32;3],time:f32,spacing:f32,mask:&super::OceanMasks){
        each!(self,r=>r.prepare(device,queue,encoder,index,waves,centre,plane,eye,time,spacing,mask))
    }
    pub fn encode_eye(&self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,index:usize,waves:&wgpu::BindGroup,eye:[f32;3],time:f32,spacing:f32,result:&wgpu::Buffer)->bool{
        each!(self,r=>r.encode_eye(device,queue,encoder,index,waves,eye,time,spacing,result))
    }
    pub fn encode_probe(&self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,index:usize,waves:&wgpu::BindGroup,camera:&blue_water_gpu::original_surface::SurfaceCamera,spacing:f32,near:bool,boundary:&wgpu::Buffer,target:&wgpu::TextureView){
        each!(self,r=>r.encode_probe(device,queue,encoder,index,waves,camera,spacing,near,boundary,target))
    }
    pub fn group(&self,index:usize)->Option<&wgpu::BindGroup>{each!(self,r=>r.group(index))}
    /// Coast wet-map receiver can affect a pixel this frame (eye low enough).
    pub fn wet_visible(&self,index:usize)->bool{match self{Self::Coast(r)=>r.wet_visible(index),Self::Ocean2(_)=>false}}
    pub fn far_mesh_levels(&self,index:usize)->u8{match self{Self::Coast(r)=>r.far_mesh_levels(index),Self::Ocean2(_)=>0}}
    /// The Ninja sim can displace pool `index`'s vertices (Ocean2: always, its WPO covers its domain).
    pub fn sim_displaces(&self,index:usize)->bool{match self{Self::Coast(r)=>r.sim_displaces(index),Self::Ocean2(_)=>true}}
    /// Coast whitewater sprites drawn after the pool's surface; Ocean2 has none.
    pub fn whitewater(&self,index:usize)->Option<(&wgpu::RenderPipeline,&wgpu::BindGroup,&wgpu::Buffer)>{match self{Self::Coast(r)=>r.whitewater(index),Self::Ocean2(_)=>None}}
    pub fn eye_source(&self,index:usize)->Option<&wgpu::Buffer>{each!(self,r=>r.eye_source(index))}
}
