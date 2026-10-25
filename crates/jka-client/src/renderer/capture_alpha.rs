//! Convert Studio's coverage + premultiplied light into straight SDR RGBA.
//! Additive light has no geometric coverage. The least opaque ordinary RGBA
//! representation that preserves its RGB over black uses alpha >= max(R,G,B)
//! in linear light. Opaque actors and authored translucent coverage are kept.
use std::sync::OnceLock;

struct Tables {
    minimum_alpha: [u8;256],
    straight: Vec<u8>,
}

fn decode(value:f32) -> f32 {
    if value<=0.04045 {value/12.92}else{((value+0.055)/1.055).powf(2.4)}
}
fn encode(value:f32) -> f32 {
    if value<=0.0031308 {value*12.92}else{1.055*value.powf(1.0/2.4)-0.055}
}
fn tables(srgb_surface:bool) -> &'static Tables {
    static SRGB:OnceLock<Tables>=OnceLock::new();
    static LINEAR:OnceLock<Tables>=OnceLock::new();
    (if srgb_surface {&SRGB}else{&LINEAR}).get_or_init(|| {
        let linear:[f32;256]=std::array::from_fn(|c| {
            let value=c as f32/255.0;
            if srgb_surface {decode(value)}else{value}
        });
        let minimum_alpha=std::array::from_fn(|c|(linear[c]*255.0).ceil().clamp(0.0,255.0) as u8);
        let mut straight=vec![0;256*256];
        for a in 1..256 {
            for c in 0..256 {
                straight[a*256+c]=(encode((linear[c]*255.0/a as f32).min(1.0))*255.0).round() as u8;
            }
        }
        Tables {minimum_alpha,straight}
    })
}

pub(in crate::renderer) fn resolve_bgra(pixels:&mut [u8],srgb_surface:bool) {
    let tables=tables(srgb_surface);
    for pixel in pixels.chunks_exact_mut(4) {
        let minimum=pixel[..3].iter().map(|&c|tables.minimum_alpha[c as usize]).max().unwrap_or(0);
        let alpha=pixel[3].max(minimum);
        for channel in &mut pixel[..3] {
            *channel=tables.straight[alpha as usize*256+*channel as usize];
        }
        pixel[3]=alpha;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn studio_alpha_fades_glow_without_erasing_black_opaque_clothing() {
        let mut pixels=vec![0,0,0,0, 0,0,60,0, 0,0,255,0, 3,3,3,255];
        resolve_bgra(&mut pixels,true);
        assert_eq!(&pixels[..4],&[0,0,0,0]);
        assert_eq!(pixels[7],12); // Faint red emission, not opaque alpha 255.
        assert!(pixels[6]>=248);
        assert_eq!(&pixels[8..12],&[0,0,255,255]);
        assert_eq!(&pixels[12..],&[3,3,3,255]);
    }
    #[test]
    fn studio_alpha_recomposes_original_light_and_preserves_authored_coverage() {
        for srgb in [false,true] {
            for input in [[10,20,60,0],[5,12,25,128],[100,140,200,255],[120,30,180,0]] {
                let mut pixel=input;
                resolve_bgra(&mut pixel,srgb);
                assert!(pixel[3]>=input[3]);
                let alpha=pixel[3] as f32/255.0;
                for c in 0..3 {
                    let original=if srgb {decode(input[c] as f32/255.0)}else{input[c] as f32/255.0};
                    let foreground=decode(pixel[c] as f32/255.0);
                    assert!((foreground*alpha-original).abs()<0.004);
                    // A faint halo leaves most of a light backdrop visible.
                    let white=foreground*alpha+1.0-alpha;
                    if input[3]==0 && *input[..3].iter().max().unwrap()<=60 {assert!(white>0.75);}
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "capture_alpha_gpu_tests.rs"]
mod gpu_tests;
