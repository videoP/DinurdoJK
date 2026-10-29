//! Deterministic CPU reference for Render Studio's GPU video compositor.
//! Live preview and offline video export blend the independently rendered GPU
//! scenes directly; the BGRA8 reference below enables regression tests.
//! No demo decoding or GameSession lifetime is owned here.
use super::render_studio_timeline::StudioDemoTransition;

fn smooth(edge: f32, value: f32) -> f32 {
    let x = (value/edge + 0.5).clamp(0.0,1.0);
    x*x*(3.0-2.0*x)
}

// CPU reference for deterministic tests and any future capture compositor.
// The live GPU shader implements the same mask functions without readbacks.
fn radial_depth_order(raw:f32)->f32 {
    if raw >= 999999.0 { return 1.0; }
    ((1.0 + raw.max(0.0)/16.0).log2() / 1025.0_f32.log2()).clamp(0.0,0.9999)
}
fn block_random(x:u32,y:u32)->f32 {
    let mut n=x.wrapping_mul(374761393).wrapping_add(y.wrapping_mul(668265263)).wrapping_add(1442695041);
    n=(n^(n>>13)).wrapping_mul(1274126177);
    (n^(n>>16)) as f32 / u32::MAX as f32
}
fn incoming_weight(kind:StudioDemoTransition, progress:f32, x:f32, y:f32,
    luminance:f32, depth_a:f32, depth_b:f32) -> f32 {
    let p=progress.clamp(0.0,1.0);
    if p<=0.0 {return 0.0;}
    if p>=1.0 {return 1.0;}
    let feather=0.006;
    match kind {
        StudioDemoTransition::Cut => 0.0,
        StudioDemoTransition::Dissolve | StudioDemoTransition::FadeThroughBlack => p,
        StudioDemoTransition::WipeLeft => smooth(feather, p-x),
        StudioDemoTransition::WipeRight => smooth(feather, p-(1.0-x)),
        StudioDemoTransition::WipeUp => smooth(feather, p-y),
        StudioDemoTransition::WipeDown => smooth(feather, p-(1.0-y)),
        StudioDemoTransition::Iris => smooth(feather,p*0.71-((x-0.5).powi(2)+(y-0.5).powi(2)).sqrt()),
        StudioDemoTransition::DepthNearToFar => smooth(0.07,p-(0.05+0.90*radial_depth_order(depth_a))),
        StudioDemoTransition::DepthFarToNear => smooth(0.07,p-(0.05+0.90*(1.0-radial_depth_order(depth_a)))),
        StudioDemoTransition::DepthLayers => {
            let z=0.5*(radial_depth_order(depth_a)+radial_depth_order(depth_b));
            smooth(0.035,p-(0.05+0.90*(z.min(0.9999)*9.0).floor()/8.0))
        }
        StudioDemoTransition::LumaBrightFirst => smooth(0.10,p-(0.06+0.88*(1.0-luminance.clamp(0.0,1.0)))),
        StudioDemoTransition::LumaDarkFirst => smooth(0.10,p-(0.06+0.88*luminance.clamp(0.0,1.0))),
        StudioDemoTransition::Mosaic => smooth(0.055,p-(0.035+0.93*block_random((x*32.0).floor() as u32,(y*18.0).floor() as u32))),
        StudioDemoTransition::RadialRipple => {
            let d=((x-0.5).powi(2)+(y-0.5).powi(2)).sqrt();
            smooth(0.03,p*0.83-0.065-d+0.025*(d*64.0-p*12.0).sin())
        }
        StudioDemoTransition::PlayerSilhouette | StudioDemoTransition::EntityCascade => p,
    }
}

/// Mix two opaque recorded worlds in linear-light RGB. Pixel alpha is opaque
/// because Render Studio explicitly forbids transparent demo/BSP captures.
/// Depth EXR export of an overlap is distinct and remains disabled.
pub(in crate::app) fn blend_demo_bgra(
    outgoing:&[u8], incoming:&mut [u8], width:u32, height:u32,
    kind:StudioDemoTransition, progress:f32,
)->Result<(),String> {
    blend_demo_bgra_with_depth(outgoing,incoming,width,height,kind,progress,None,None)
}

/// Depth-aware reference blend. Radial depth is in JKA world units; missing
/// depth for depth-based modes is an error, never an RGB-derived substitute.
pub(in crate::app) fn blend_demo_bgra_with_depth(
    outgoing:&[u8], incoming:&mut [u8], width:u32, height:u32,
    kind:StudioDemoTransition, progress:f32,
    depth_a:Option<&[f32]>, depth_b:Option<&[f32]>,
)->Result<(),String> {
    let needed=(width as usize).checked_mul(height as usize).and_then(|n|n.checked_mul(4))
        .ok_or("Transition capture size overflow")?;
    if width==0 || height==0 || outgoing.len()!=needed || incoming.len()!=needed {
        return Err("Transition captures have different dimensions or invalid BGRA lengths".into());
    }
    if matches!(kind,StudioDemoTransition::PlayerSilhouette | StudioDemoTransition::EntityCascade) {
        return Err("Identity-aware transition requires a GPU-visible geometry mask; CPU color/depth blending cannot reproduce it".into());
    }
    if kind.requires_depth() && (depth_a.is_none_or(|d| d.len()!=width as usize*height as usize)
        || depth_b.is_none_or(|d| d.len()!=width as usize*height as usize)) {
        return Err("Depth-based transition requires the radial-depth capture of both worlds".into());
    }
    let mut srgb=[0.0_f32;256];
    for (i,v) in srgb.iter_mut().enumerate() {
        let c=i as f32/255.0;
        *v=if c<=0.04045 {c/12.92} else {((c+0.055)/1.055).powf(2.4)};
    }
    let mut linear_to_srgb=[0_u8;4097];
    for (i,v) in linear_to_srgb.iter_mut().enumerate() {
        let c=i as f32/4096.0;
        let s=if c<=0.0031308 {12.92*c} else {1.055*c.powf(1.0/2.4)-0.055};
        *v=(s*255.0).round().clamp(0.0,255.0) as u8;
    }
    let encode=|value:f32|linear_to_srgb[(value.clamp(0.0,1.0)*4096.0).round() as usize];
    for (i,(a,b)) in outgoing.chunks_exact(4).zip(incoming.chunks_exact_mut(4)).enumerate() {
        let x=((i%width as usize) as f32+0.5)/width as f32;
        let y=((i/width as usize) as f32+0.5)/height as f32;
        let luminance=(srgb[a[2] as usize]*0.2126+srgb[a[1] as usize]*0.7152+srgb[a[0] as usize]*0.0722).clamp(0.0,1.0);
        let weight=incoming_weight(kind,progress,x,y,luminance,
            depth_a.map_or(0.0,|d|d[i]),depth_b.map_or(0.0,|d|d[i]));
        let (gain_a,gain_b)=if kind==StudioDemoTransition::FadeThroughBlack {
            ((1.0-2.0*progress).clamp(0.0,1.0),(2.0*progress-1.0).clamp(0.0,1.0))
        } else {(1.0-weight,weight)};
        for channel in 0..3 {
            b[channel]=encode(srgb[a[channel] as usize]*gain_a + srgb[b[channel] as usize]*gain_b);
        }
        b[3]=255;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blend_is_deterministic_and_linear_light() {
        let a=[0u8,0,0,255];let mut b=[255u8,255,255,255];
        blend_demo_bgra(&a,&mut b,1,1,StudioDemoTransition::Dissolve,0.5).unwrap();
        assert!(b[0]>=187&&b[0]<=189,"half of linear light is ~188 sRGB, not 128");
        assert_eq!(b[3],255);
    }
    #[test]
    fn directional_wipe_reveals_pixels_in_screen_order() {
        let a=[0,0,0,255,0,0,0,255];
        let mut b=[255u8;8];
        blend_demo_bgra(&a,&mut b,2,1,StudioDemoTransition::WipeLeft,0.5).unwrap();
        assert_eq!(b[0],255);assert_eq!(b[4],0);
    }
    #[test]
    fn depth_masks_reveal_near_before_far_and_reverse() {
        let a=[0u8,0,0,255,0,0,0,255];
        let depth_a=[64.0,8192.0];
        let depth_b=[128.0,4096.0];
        for (kind,first_pixel) in [
            (StudioDemoTransition::DepthNearToFar,true),
            (StudioDemoTransition::DepthFarToNear,false),
        ] {
            let mut b=[255u8;8];
            blend_demo_bgra_with_depth(&a,&mut b,2,1,kind,0.5,Some(&depth_a),Some(&depth_b)).unwrap();
            assert_eq!(b[0]>b[4],first_pixel);
        }
    }
    #[test]
    fn depth_effect_without_depth_is_explicitly_rejected() {
        let mut b=[255;4];
        assert!(blend_demo_bgra(&[0;4],&mut b,1,1,StudioDemoTransition::DepthNearToFar,0.5).is_err());
    }
    #[test]
    fn all_advanced_modes_have_exact_endpoints() {
        for (kind,_) in StudioDemoTransition::OPTIONS {
            if matches!(kind, StudioDemoTransition::PlayerSilhouette | StudioDemoTransition::EntityCascade) {
                // The offline CPU reference has no per-pixel dynamic model IDs.
                continue;
            }
            let a=[40,50,60,255];
            let depth=[100.0];
            for p in [0.0,1.0] {
                let mut b=[90,100,110,255];
                blend_demo_bgra_with_depth(&a,&mut b,1,1,kind,p,Some(&depth),Some(&depth)).unwrap();
                // sRGB LUT has 12-bit quantization; allow rounding by one.
                let expected=if p==0.0 {a}else{[90,100,110,255]};
                for c in 0..3 {assert!((b[c] as i16-expected[c] as i16).abs()<=1,"{kind:?} progress={p}");}
            }
        }
    }
    #[test]
    fn mismatched_capture_is_rejected() {
        let mut b=vec![0;7];
        assert!(blend_demo_bgra(&[0;8],&mut b,2,1,StudioDemoTransition::Dissolve,0.5).is_err());
    }
}

#[cfg(test)]
mod live_gpu_mode_contract_tests {
    use super::*;
    #[test]
    fn shader_effect_discriminants_follow_offline_blend_modes() {
        // The shader switch takes the enum discriminants as a u32. Make
        // accidental insertion/reordering fail a unit test instead of silently
        // producing a different export vs live preview transition.
        assert_eq!(StudioDemoTransition::Cut as u32,0);
        assert_eq!(StudioDemoTransition::Dissolve as u32,1);
        assert_eq!(StudioDemoTransition::FadeThroughBlack as u32,2);
        assert_eq!(StudioDemoTransition::WipeLeft as u32,3);
        assert_eq!(StudioDemoTransition::WipeRight as u32,4);
        assert_eq!(StudioDemoTransition::WipeUp as u32,5);
        assert_eq!(StudioDemoTransition::WipeDown as u32,6);
        assert_eq!(StudioDemoTransition::Iris as u32,7);
        assert_eq!(StudioDemoTransition::DepthNearToFar as u32,8);
        assert_eq!(StudioDemoTransition::DepthFarToNear as u32,9);
        assert_eq!(StudioDemoTransition::DepthLayers as u32,10);
        assert_eq!(StudioDemoTransition::LumaBrightFirst as u32,11);
        assert_eq!(StudioDemoTransition::LumaDarkFirst as u32,12);
        assert_eq!(StudioDemoTransition::Mosaic as u32,13);
        assert_eq!(StudioDemoTransition::RadialRipple as u32,14);
        assert_eq!(StudioDemoTransition::PlayerSilhouette as u32,15);
        assert_eq!(StudioDemoTransition::EntityCascade as u32,16);
    }
}
