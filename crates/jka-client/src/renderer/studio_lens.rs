//! Full-frame-equivalent thin-lens DOF, evaluated per Studio output frame.
use super::*;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StudioLens {
    /// None is a pinhole: displayed as infinity, no lens blur or allocation.
    pub aperture: Option<f32>,
    pub focus_distance: f32,
}
impl Default for StudioLens {fn default()->Self{Self{aperture:None,focus_distance:256.0}}}
impl StudioLens {
    pub fn valid(self)->bool{self.aperture.is_none_or(|n|n.is_finite()&&(0.7..=128.0).contains(&n))
        &&self.focus_distance.is_finite()&&(1.0..=65536.0).contains(&self.focus_distance)}
    pub fn interpolate(self,other:Self,t:f32)->Self {
        let inverse=self.aperture.map_or(0.0,|n|1.0/n)*(1.0-t)+other.aperture.map_or(0.0,|n|1.0/n)*t;
        Self{aperture:(inverse>1e-6).then(||1.0/inverse),focus_distance:self.focus_distance+(other.focus_distance-self.focus_distance)*t}
    }
    /// cg_fov is horizontal at 4:3. Match actual output horizontal framing
    /// against a 36 mm-wide full-frame sensor cropped to the output aspect.
    pub fn focal_length_35mm(cg_fov:f32,aspect:f32)->f32 {
        18.0/(0.75*aspect.max(0.01)*(cg_fov.clamp(1.0,176.0).to_radians()*0.5).tan())
    }
}

pub(in crate::renderer) struct StudioLensGpu {
    pub texture:wgpu::Texture,
    pub view:wgpu::TextureView,
    format:wgpu::TextureFormat,
    pipeline:wgpu::RenderPipeline,
    layout:wgpu::BindGroupLayout,
    uniform:wgpu::Buffer,
    sampler:wgpu::Sampler,
}
#[repr(C)]
#[derive(Clone,Copy,bytemuck::Pod,bytemuck::Zeroable)]
struct LensUniform{viewport:[f32;4],optics:[f32;4],projection:[f32;4]}
impl StudioLensGpu {
    pub fn matches(&self,size:[u32;2],format:wgpu::TextureFormat)->bool {
        self.texture.width()==size[0]&&self.texture.height()==size[1]&&self.format==format
    }
    pub fn new(device:&wgpu::Device,size:[u32;2],format:wgpu::TextureFormat)->Self {
        let texture=device.create_texture(&wgpu::TextureDescriptor{label:Some("Studio lens image"),
            size:wgpu::Extent3d{width:size[0],height:size[1],depth_or_array_layers:1},mip_level_count:1,
            sample_count:1,dimension:wgpu::TextureDimension::D2,format,
            usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::RENDER_ATTACHMENT,view_formats:&[]});
        let view=texture.create_view(&Default::default());
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("Studio lens bindings"),entries:&[
            super::texture_entry(0),super::sampler_entry(1),wgpu::BindGroupLayoutEntry{binding:2,visibility:wgpu::ShaderStages::FRAGMENT,
                ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None},
            super::uniform_entry(3,wgpu::ShaderStages::FRAGMENT)]});
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("Studio lens"),source:wgpu::ShaderSource::Wgsl(include_str!("../studio_lens.wgsl").into())});
        let pipeline_layout=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:Some("Studio lens layout"),bind_group_layouts:&[Some(&layout)],immediate_size:0});
        let pipeline=device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{label:Some("Studio aperture DOF"),layout:Some(&pipeline_layout),
            vertex:wgpu::VertexState{module:&shader,entry_point:Some("vs_main"),compilation_options:Default::default(),buffers:&[]},
            fragment:Some(wgpu::FragmentState{module:&shader,entry_point:Some("fs_main"),compilation_options:Default::default(),
                targets:&[Some(wgpu::ColorTargetState{format,blend:None,write_mask:wgpu::ColorWrites::ALL})]}),
            primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview_mask:None,cache:None});
        let uniform=device.create_buffer_init(&wgpu::util::BufferInitDescriptor{label:Some("Studio lens uniform"),contents:bytemuck::bytes_of(&LensUniform::zeroed()),usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST});
        let sampler=device.create_sampler(&wgpu::SamplerDescriptor{mag_filter:wgpu::FilterMode::Linear,min_filter:wgpu::FilterMode::Linear,..Default::default()});
        Self{texture,view,format,pipeline,layout,uniform,sampler}
    }
    pub fn encode(&self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,input:&wgpu::TextureView,
        depth:&wgpu::TextureView,destination:&wgpu::TextureView,camera:&Camera,lens:StudioLens,viewport:[u32;4]) {
        let aspect=viewport[2] as f32/viewport[3].max(1) as f32;
        let tan_y=(camera.cg_fov().to_radians()*0.5).tan()*0.75;
        let uniform=LensUniform{viewport:viewport.map(|v|v as f32),optics:[lens.focus_distance,lens.aperture.unwrap_or(128.0),StudioLens::focal_length_35mm(camera.cg_fov(),aspect),0.0],
            projection:[tan_y*aspect,tan_y,self.texture.width() as f32,self.texture.height() as f32]};
        queue.write_buffer(&self.uniform,0,bytemuck::bytes_of(&uniform));
        let bind=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("Studio lens frame"),layout:&self.layout,entries:&[
            wgpu::BindGroupEntry{binding:0,resource:wgpu::BindingResource::TextureView(input)},wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::Sampler(&self.sampler)},
            wgpu::BindGroupEntry{binding:2,resource:wgpu::BindingResource::TextureView(depth)},wgpu::BindGroupEntry{binding:3,resource:self.uniform.as_entire_binding()}]});
        let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor{label:Some("Studio lens composite"),color_attachments:&[Some(wgpu::RenderPassColorAttachment{
            view:destination,depth_slice:None,resolve_target:None,ops:wgpu::Operations{load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}})],
            depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
        pass.set_scissor_rect(viewport[0],viewport[1],viewport[2],viewport[3]);
        pass.set_pipeline(&self.pipeline);pass.set_bind_group(0,&bind,&[]);pass.draw(0..3,0..1);
    }
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn studio_lens_defaults_and_full_frame_projection_match(){
        assert_eq!(StudioLens::default().aperture,None);
        assert!((StudioLens::focal_length_35mm(90.0,4.0/3.0)-18.0).abs()<1e-4);
        assert!((StudioLens::focal_length_35mm(90.0,16.0/9.0)-13.5).abs()<1e-4);
        let a=StudioLens::default();let b=StudioLens{aperture:Some(2.0),..a};
        assert_eq!(a.interpolate(b,0.5).aperture,Some(4.0));assert!(b.valid());
    }
    #[test]
    #[ignore="requires a Vulkan GPU"]
    fn studio_lens_gpu_keeps_focus_and_alpha_and_stops_down() {
        block_on(async {
            let instance=wgpu::Instance::new(wgpu::InstanceDescriptor{
                backends:wgpu::Backends::VULKAN,..wgpu::InstanceDescriptor::new_without_display_handle()});
            let adapter=instance.request_adapter(&Default::default()).await.unwrap();
            let (device,queue)=adapter.request_device(&Default::default()).await.unwrap();
            let scope=device.push_error_scope(wgpu::ErrorFilter::Validation);
            let desc=wgpu::TextureDescriptor{label:None,size:wgpu::Extent3d{width:64,height:64,depth_or_array_layers:1},
                mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba8Unorm,
                usage:wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]};
            let pixels:Vec<u8>=(0..4096).flat_map(|i|{let c=if ((i%64)/4+(i/64)/4)%2==0{32}else{220};[c,c,c,128]}).collect();
            let input=device.create_texture_with_data(&queue,&desc,wgpu::util::TextureDataOrder::LayerMajor,&pixels);
            let gpu=StudioLensGpu::new(&device,[64,64],desc.format);
            let output=device.create_texture(&wgpu::TextureDescriptor{usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,..desc.clone()});
            let camera=Camera::new_with_fov([0.0;3],0.0,20.0);
            let render=|z:f32,n:f32| {
                let tan=(20.0_f32.to_radians()*0.5).tan()*0.75;
                let depths:Vec<f32>=(0..4096).map(|i|{
                    let x=((i%64) as f32+0.5)/32.0-1.0;let y=((i/64) as f32+0.5)/32.0-1.0;
                    z*(1.0+tan*tan*(x*x+y*y)).sqrt()
                }).collect();
                let depth=device.create_texture_with_data(&queue,&wgpu::TextureDescriptor{format:wgpu::TextureFormat::R32Float,..desc.clone()},
                    wgpu::util::TextureDataOrder::LayerMajor,bytemuck::cast_slice(&depths));
                let mut encoder=device.create_command_encoder(&Default::default());
                gpu.encode(&device,&queue,&mut encoder,&input.create_view(&Default::default()),&depth.create_view(&Default::default()),
                    &output.create_view(&Default::default()),&camera,StudioLens{aperture:Some(n),focus_distance:20.0},[0,0,64,64]);
                let readback=device.create_buffer(&wgpu::BufferDescriptor{label:None,size:64*256,
                    usage:wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::MAP_READ,mapped_at_creation:false});
                encoder.copy_texture_to_buffer(output.as_image_copy(),wgpu::TexelCopyBufferInfo{buffer:&readback,
                    layout:wgpu::TexelCopyBufferLayout{offset:0,bytes_per_row:Some(256),rows_per_image:Some(64)}},output.size());
                queue.submit([encoder.finish()]);readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let data=readback.slice(..).get_mapped_range().unwrap();data.to_vec()
            };
            let focused=render(20.0,1.0);let blurred=render(10.0,1.0);let stopped=render(10.0,128.0);
            assert!(scope.pop().await.is_none(),"lens GPU validation");
            assert_eq!(focused,pixels,"the focus plane must remain sharp");
            for image in [&focused,&blurred,&stopped]{assert!(image.chunks_exact(4).all(|p|p[3]==128),"alpha must survive lens filtering");}
            let variance=|image:&[u8]|image.chunks_exact(4).map(|p|(f64::from(p[0])-126.0).powi(2)).sum::<f64>();
            assert!(variance(&blurred)<variance(&focused)*0.5,"a wide aperture must blur defocused detail");
            assert!(variance(&stopped)>variance(&blurred)*2.0,"stopping down must restore depth of field: focused={} wide={} stopped={}",variance(&focused),variance(&blurred),variance(&stopped));
        });
    }
}
