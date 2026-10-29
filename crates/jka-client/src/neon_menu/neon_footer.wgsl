// A quiet, integral stage footer, not individual button plaques. Taffy owns
// each control's hit rectangle; GPU only draws atmosphere and hover accents.
struct Footer {
    screen: vec4<f32>, // xy pixels, z footer top, w BACK/QUIT/APPLY hover bits
    back: vec4<f32>,
    quit: vec4<f32>,
    apply: vec4<f32>,
    mode: vec4<f32>,   // x: developer tools scene dimmer
};
@group(0) @binding(0) var<uniform> data: Footer;

struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Out {
    let xy = array<vec2<f32>,6>(vec2<f32>(0.,0.),vec2<f32>(1.,0.),vec2<f32>(0.,1.),vec2<f32>(0.,1.),vec2<f32>(1.,0.),vec2<f32>(1.,1.));
    let pos = xy[index];
    var out: Out;
    out.position = vec4<f32>(pos*vec2<f32>(2.,-2.)+vec2<f32>(-1.,1.),0.,1.);
    out.uv=pos;
    return out;
}
fn accent_line(p: vec2<f32>, rect: vec4<f32>, hovered: bool, gas: vec3<f32>) -> vec3<f32> {
    if (!hovered || rect.z <= rect.x) { return vec3<f32>(0.); }
    // A short illuminated rail below the type rather than a border or box.
    let x0 = mix(rect.x, rect.z, 0.23);
    let x1 = mix(rect.x, rect.z, 0.77);
    let x = smoothstep(x0-0.006,x0+0.006,p.x) * (1.0-smoothstep(x1-0.006,x1+0.006,p.x));
    let edge = rect.w - 0.009;
    let y = exp(-pow((p.y-edge)*data.screen.y/2.2,2.0));
    return gas * x * y * 0.55;
}
@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let p=in.uv;
    let top=data.screen.z;
    if (p.y < top) {
        if (data.mode.x < 0.5) { return vec4<f32>(0.); }
        // Darken the developer stage without obscuring the HDR tubes: this
        // pass is underneath the final neon composite.
        let soft = exp(-pow((p.x-0.20)/0.5,2.0));
        return vec4<f32>(vec3<f32>(0.001,0.005,0.010)*soft,0.20);
    }
    let v=clamp((p.y-top)/max(0.001,1.0-top),0.0,1.0);
    // No filled button plates, no segmented rectangles. A barely visible
    // continuous dark gradient with a hairline catching cyan from the scene.
    let edge=exp(-pow((p.y-top)*data.screen.y/1.5,2.0));
    let ambient=vec3<f32>(0.012,0.030,0.046) + vec3<f32>(0.015,0.023,0.036)*v;
    var col=ambient + vec3<f32>(0.025,0.105,0.16)*edge*0.50;
    let hover=u32(data.screen.w+0.5);
    col += accent_line(p,data.back,(hover & 1u)!=0u,vec3<f32>(0.08,0.64,1.0));
    col += accent_line(p,data.quit,(hover & 2u)!=0u,vec3<f32>(1.0,0.22,0.45));
    col += accent_line(p,data.apply,(hover & 4u)!=0u,vec3<f32>(1.0,0.56,0.12));
    let a = 0.26 + 0.26*v + edge*0.16;
    return vec4<f32>(col*a,a);
}
