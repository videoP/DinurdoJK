@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var depths: texture_2d<f32>;
struct Lens { viewport:vec4<f32>, optics:vec4<f32>, projection:vec4<f32> };
@group(0) @binding(3) var<uniform> lens: Lens;
struct Out { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Out {
    let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-3.0),vec2<f32>(-1.0,1.0),vec2<f32>(3.0,1.0))[i];
    var o:Out;o.position=vec4<f32>(p,0.0,1.0);o.uv=p*vec2<f32>(0.5,-0.5)+0.5;return o;
}
@fragment fn fs_main(input:Out)->@location(0) vec4<f32> {
    let size=lens.projection.zw;
    let pixel=vec2<i32>(clamp(input.uv*size,vec2<f32>(0.0),size-1.0));
    let radial=max(textureLoad(depths,pixel,0).r,0.01);
    let ndc=((input.uv*size-lens.viewport.xy)/lens.viewport.zw)*2.0-1.0;
    let z=radial*inverseSqrt(1.0+dot(ndc*lens.projection.xy,ndc*lens.projection.xy));
    let focus=max(lens.optics.x,0.01);let f=lens.optics.z;
    // One JKA unit is treated as one inch. CoC diameter on a 36 mm sensor,
    // projected into output pixels. Half diameter is the sampling radius.
    let radius=clamp(0.5*f*f/(max(lens.optics.y,0.7)*max(focus*25.4-f,0.01))*abs(z-focus)/max(z,0.01)*lens.viewport.z/36.0,0.0,24.0);
    if (radius<0.2){return textureSampleLevel(image,image_sampler,input.uv,0.0);}
    let lo=(lens.viewport.xy+0.5)/size;let hi=(lens.viewport.xy+lens.viewport.zw-0.5)/size;
    var color=vec4<f32>(0.0);
    for(var i=0u;i<32u;i+=1u){let angle=f32(i)*2.39996323;let r=radius*sqrt((f32(i)+0.5)/32.0);
        let uv=clamp(input.uv+vec2<f32>(cos(angle),sin(angle))*r/size,lo,hi);
        color+=textureSampleLevel(image,image_sampler,uv,0.0);
    }
    return color/32.0;
}
