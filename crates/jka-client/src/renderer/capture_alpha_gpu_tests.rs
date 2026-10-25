use super::*;
use crate::renderer::{CameraUniform,DynamicModelAlphaMode as Mode,DynamicModelRenderer,
    DynamicModelVertex,GpuImage,DEPTH_FORMAT};
use crate::renderer::models::pipelines::create_dynamic_model_pipeline_inner;
use wgpu::util::DeviceExt;

#[test]
#[ignore = "requires a Vulkan GPU"]
fn studio_alpha_gpu_preserves_light_coverage_and_normal_rendering() {
    crate::renderer::block_on(async {
        let instance=wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends:wgpu::Backends::VULKAN,..wgpu::InstanceDescriptor::new_without_display_handle()});
        let adapter=instance.request_adapter(&Default::default()).await.unwrap();
        let (device,queue)=adapter.request_device(&Default::default()).await.unwrap();
        let camera_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:None,entries:&[wgpu::BindGroupLayoutEntry {binding:0,visibility:wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty:wgpu::BindingType::Buffer {ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None}]});
        let uniform=CameraUniform {view_proj:glam::Mat4::IDENTITY.to_cols_array_2d(),
            camera_pos_time:[0.0,0.0,1.0,0.0],clip_plane:[0.0;4],render_flags:[0;4],camera_forward:[0.0,0.0,-1.0,0.0],
            unjittered_view_proj:glam::Mat4::IDENTITY.to_cols_array_2d(),previous_unjittered_view_proj:glam::Mat4::IDENTITY.to_cols_array_2d(),jump_shade:[0.0;4]};
        let camera_buffer=device.create_buffer_init(&wgpu::util::BufferInitDescriptor {label:None,contents:bytemuck::bytes_of(&uniform),usage:wgpu::BufferUsages::UNIFORM});
        let camera=device.create_bind_group(&wgpu::BindGroupDescriptor {label:None,layout:&camera_layout,
            entries:&[wgpu::BindGroupEntry {binding:0,resource:camera_buffer.as_entire_binding()}]});
        let white=device.create_texture_with_data(&queue,&wgpu::TextureDescriptor {label:None,
            size:wgpu::Extent3d {width:1,height:1,depth_or_array_layers:1},mip_level_count:1,sample_count:1,
            dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba8UnormSrgb,
            usage:wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]},wgpu::util::TextureDataOrder::LayerMajor,&[255;4]);
        let white=GpuImage {view:white.create_view(&Default::default()),_texture:white};
        let glass_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {label:None,entries:&[]});
        let format=wgpu::TextureFormat::Rgba8UnormSrgb;
        let renderer=DynamicModelRenderer::new(&device,&queue,&camera_layout,&glass_layout,format,1,&white);
        for samples in [1,4] {
            let render=|draws:&[(Mode,[f32;4],f32)],capture:bool| -> Vec<u8> {
                let desc=wgpu::TextureDescriptor {label:None,size:wgpu::Extent3d {width:4,height:4,depth_or_array_layers:1},
                    mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format,
                    usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[]};
                let target=device.create_texture(&desc);
                let target_view=target.create_view(&Default::default());
                let msaa=(samples>1).then(||device.create_texture(&wgpu::TextureDescriptor {sample_count:samples,usage:wgpu::TextureUsages::RENDER_ATTACHMENT,..desc}));
                let msaa_view=msaa.as_ref().map(|texture|texture.create_view(&Default::default()));
                let depth=device.create_texture(&wgpu::TextureDescriptor {format:DEPTH_FORMAT,sample_count:samples,usage:wgpu::TextureUsages::RENDER_ATTACHMENT,..desc});
                let depth_view=depth.create_view(&Default::default());
                let pipelines:Vec<_>=draws.iter().map(|(mode,_,_)|create_dynamic_model_pipeline_inner(
                    &device,&renderer.pipeline_layout,&renderer.shader,format,samples,*mode,false,false,false,false,capture)).collect();
                let buffers:Vec<_>=draws.iter().map(|(_,color,z)| {
                    let vertices=[[-1.0,-1.0],[1.0,-1.0],[1.0,1.0],[-1.0,-1.0],[1.0,1.0],[-1.0,1.0]].map(|p|
                        DynamicModelVertex {position:[p[0],p[1],*z],normal:[0.0,0.0,1.0],uv:[0.5;2],color:*color,depth_hack:0.0,water_wetness:0.0});
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {label:None,contents:bytemuck::cast_slice(&vertices),usage:wgpu::BufferUsages::VERTEX})
                }).collect();
                let mut encoder=device.create_command_encoder(&Default::default());
                {
                    let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {label:None,
                        color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:msaa_view.as_ref().unwrap_or(&target_view),depth_slice:None,
                            resolve_target:msaa_view.as_ref().map(|_|&target_view),ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),store:wgpu::StoreOp::Store}})],
                        depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment {view:&depth_view,
                            depth_ops:Some(wgpu::Operations {load:wgpu::LoadOp::Clear(crate::renderer::camera_depth_clear()),store:wgpu::StoreOp::Store}),stencil_ops:None}),
                        timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
                    pass.set_bind_group(0,&camera,&[]);pass.set_bind_group(1,&renderer.fallback_bind_group,&[]);
                    for (pipeline,buffer) in pipelines.iter().zip(&buffers) {pass.set_pipeline(pipeline);pass.set_vertex_buffer(0,buffer.slice(..));pass.draw(0..6,0..1);}
                }
                let readback=device.create_buffer(&wgpu::BufferDescriptor {label:None,size:256*4,usage:wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::MAP_READ,mapped_at_creation:false});
                encoder.copy_texture_to_buffer(target.as_image_copy(),wgpu::TexelCopyBufferInfo {buffer:&readback,
                    layout:wgpu::TexelCopyBufferLayout {offset:0,bytes_per_row:Some(256),rows_per_image:Some(4)}},target.size());
                queue.submit([encoder.finish()]);
                let (tx,rx)=std::sync::mpsc::channel();
                readback.slice(..).map_async(wgpu::MapMode::Read,move |result|{tx.send(result).unwrap();});
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();rx.recv().unwrap().unwrap();
                let bytes=readback.slice(..).get_mapped_range().unwrap();bytes[256+4..256+8].to_vec()
            };
            for (mode,alpha) in [(Mode::AdditiveOne,1.0),(Mode::Additive,0.125)] {
                for intensity in [0.005,0.05,0.2,1.0] {
                    let draw=(mode,[intensity,0.0,0.0,alpha],0.5);
                    let normal=render(&[draw],false);
                    let mut exported=render(&[draw],true);
                    assert_eq!(&normal[..3],&exported[..3]); // RGB equation is unchanged.
                    assert!(normal[3]>0);
                    assert_eq!(exported[3],0); // Emission adds no material coverage.
                    resolve_bgra(&mut exported,true);
                    let expected=intensity*if mode==Mode::Additive {alpha}else{1.0};
                    let recomposed=decode(exported[0] as f32/255.0)*exported[3] as f32/255.0;
                    assert!((recomposed-expected).abs()<0.008,"{mode:?}: {normal:?} -> {exported:?}");
                    if expected<0.2 {assert!(exported[3]<64);}
                }
            }
            let mut overlap=render(&[(Mode::AdditiveOne,[0.05,0.0,0.0,1.0],0.5),(Mode::AdditiveOne,[0.05,0.0,0.0,1.0],0.5)],true);
            resolve_bgra(&mut overlap,true);assert!(overlap[3]>0 && overlap[3]<64);
            let body=(Mode::Opaque,[0.02,0.02,0.02,0.25],0.6);
            let opaque=render(&[body],true);assert_eq!(opaque[3],255);
            let occluded=render(&[body,(Mode::AdditiveOne,[1.0,0.0,0.0,1.0],0.4)],true);
            assert_eq!(opaque,occluded);
            let mut smoke=render(&[(Mode::BlendUnlit,[0.2,0.5,0.9,0.25],0.5)],true);
            resolve_bgra(&mut smoke,true);assert!((smoke[3] as i32-64).abs()<=1);
            // Verify the PNG stores straight alpha without changing the pixels.
            let mut png=Vec::new();
            image::ImageEncoder::write_image(image::codecs::png::PngEncoder::new(&mut png),&overlap,1,1,image::ExtendedColorType::Rgba8).unwrap();
            assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8().as_raw(),&overlap);
        }
    });
}
