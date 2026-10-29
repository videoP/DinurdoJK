// Render Studio live GPU compositor. Source textures contain the two separately
// rendered demo worlds. The optional scene data comes from their existing
// R32Float radial-depth prepasses (not guessed from their RGB pixels).
struct Params {
    rect: vec4<f32>,
    progress: f32,
    effect: u32,
    _pad0: u32,
    _pad1: u32,
};
@group(0) @binding(0) var outgoing: texture_2d<f32>;
@group(0) @binding(1) var incoming: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var depth_outgoing: texture_2d<f32>;
@group(0) @binding(4) var depth_incoming: texture_2d<f32>;
@group(0) @binding(5) var identity_outgoing: texture_2d<f32>;
@group(0) @binding(6) var identity_incoming: texture_2d<f32>;

@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0), vec2<f32>(3.0,-1.0), vec2<f32>(-1.0,3.0));
    return vec4<f32>(p[index],0.0,1.0);
}
fn soft(edge:f32,value:f32)->f32 {
    let t=clamp(value/edge+0.5,0.0,1.0);
    return t*t*(3.0-2.0*t);
}
// Stable random tile order, no flicker or time-dependent seeds.
fn tile_random(cell:vec2<u32>)->f32 {
    var n=cell.x*374761393u+cell.y*668265263u+1442695041u;
    n=(n^(n>>13u))*1274126177u;
    return f32(n^(n>>16u)) / 4294967295.0;
}
// JKA world units. Log scale retains visibility from character distances to
// map-scale geometry; 1e6 is the renderer's depth-clear/sky sentinel.
fn depth_order(raw:f32)->f32 {
    if raw>=999999.0 {return 1.0;}
    return clamp(log2(1.0+max(raw,0.0)/16.0)/log2(1025.0),0.0,0.9999);
}
@fragment fn fs_main(@builtin(position) pos:vec4<f32>)->@location(0) vec4<f32> {
    let xy=(pos.xy-params.rect.xy)/max(params.rect.zw,vec2<f32>(1.0));
    let p=clamp(params.progress,0.0,1.0);
    let px_a=clamp(vec2<i32>(xy*vec2<f32>(textureDimensions(outgoing))),vec2<i32>(0),vec2<i32>(textureDimensions(outgoing))-1);
    let px_b=clamp(vec2<i32>(xy*vec2<f32>(textureDimensions(incoming))),vec2<i32>(0),vec2<i32>(textureDimensions(incoming))-1);
    let a=textureLoad(outgoing,px_a,0).rgb;
    let b=textureLoad(incoming,px_b,0).rgb;
    var w=p;
    switch params.effect {
      case 0u: {w=select(0.0,1.0,p>=1.0);}
      case 3u: {w=soft(0.006,p-xy.x);}
      case 4u: {w=soft(0.006,p-(1.0-xy.x));}
      case 5u: {w=soft(0.006,p-xy.y);}
      case 6u: {w=soft(0.006,p-(1.0-xy.y));}
      case 7u: {w=soft(0.006,p*0.71-distance(xy,vec2<f32>(0.5)));}
      case 8u,9u,10u: {
        let d_a=textureLoad(depth_outgoing,clamp(vec2<i32>(xy*vec2<f32>(textureDimensions(depth_outgoing))),vec2<i32>(0),vec2<i32>(textureDimensions(depth_outgoing))-1),0).r;
        let z_a=depth_order(d_a);
        // Keep the mask thresholds inside the progress interval so nearest
        // surfaces and sky finish without an endpoint jump.
        if params.effect==8u {w=soft(0.07,p-(0.05+0.90*z_a));}
        else if params.effect==9u {w=soft(0.07,p-(0.05+0.90*(1.0-z_a)));}
        else {
          let d_b=textureLoad(depth_incoming,clamp(vec2<i32>(xy*vec2<f32>(textureDimensions(depth_incoming))),vec2<i32>(0),vec2<i32>(textureDimensions(depth_incoming))-1),0).r;
          let z=0.5*(z_a+depth_order(d_b));
          // Stepped geometric bands from both source POVs, instead of a
          // full-screen wipe or a random tile trick.
          w=soft(0.035,p-(0.05+0.90*floor(min(z,0.9999)*9.0)/8.0));
        }
      }
      case 11u,12u: {
        let luminance=clamp(dot(a,vec3<f32>(0.2126,0.7152,0.0722)),0.0,1.0);
        let order=select(luminance,1.0-luminance,params.effect==11u);
        w=soft(0.10,p-(0.06+0.88*order));
      }
      case 13u: {
        let cell=vec2<u32>(floor(xy*vec2<f32>(32.0,18.0)));
        w=soft(0.055,p-(0.035+0.93*tile_random(cell)));
      }
      case 14u: {
        let distance_from_center=distance(xy,vec2<f32>(0.5));
        w=soft(0.03,p*0.83-0.065-distance_from_center+0.025*sin(distance_from_center*64.0-p*12.0));
      }
      case 15u,16u: {
        let m_a=textureLoad(identity_outgoing,clamp(vec2<i32>(xy*vec2<f32>(textureDimensions(identity_outgoing))),vec2<i32>(0),vec2<i32>(textureDimensions(identity_outgoing))-1),0).rg;
        let m_b=textureLoad(identity_incoming,clamp(vec2<i32>(xy*vec2<f32>(textureDimensions(identity_incoming))),vec2<i32>(0),vec2<i32>(textureDimensions(identity_incoming))-1),0).rg;
        // Both scenes contribute their currently visible silhouettes; a
        // moving actor from either timeline is a valid reveal boundary.
        let player=max(m_a.r,m_b.r);
        let entity=max(m_a.g,m_b.g);
        if params.effect==15u {
          // The next clip appears through the posed player silhouette first,
          // and the surrounding scene resolves afterward.
          w=soft(0.22,p-(0.76-0.61*player));
        } else {
          // Players -> other dynamic entities -> static BSP environment.
          // The phases derive from actual rasterized geometry identity.
          let phase=select(select(0.80,0.48,entity>0.05),0.16,player>0.05);
          w=soft(0.23,p-phase);
        }
      }
      default: {}
    }
    // Every procedural mask must be exactly the unmodified A/B frame at the
    // endpoints, irrespective of the feather width or depth clear sentinel.
    if p<=0.0 {w=0.0;}
    if p>=1.0 {w=1.0;}
    var ga=1.0-w;
    var gb=w;
    if params.effect==2u {
        ga=clamp(1.0-2.0*p,0.0,1.0);
        gb=clamp(2.0*p-1.0,0.0,1.0);
    }
    return vec4<f32>(a*ga+b*gb,1.0);
}
