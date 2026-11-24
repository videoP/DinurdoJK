//! EchoField eap/emitter/PerlinNoise.java (GPL-3.0), direct scalar port.
use crate::random::JavaRandom;
#[derive(Debug, Clone)]
pub struct PerlinNoise {
    perm: [u8; 256],
}
impl PerlinNoise {
    pub fn new(seed: i64) -> Self {
        let mut perm = std::array::from_fn(|i| i as u8);
        let mut rng = JavaRandom::new(seed);
        for i in (1..256).rev() {
            perm.swap(i, rng.next_int(i as i32 + 1) as usize);
        }
        Self { perm }
    }
    pub fn sample(&self, x: f32) -> f32 {
        let xi = (x.floor() as i32 & 255) as usize;
        let xf = x - x.floor();
        let u = xf * xf * xf * (xf * (xf * 6.0 - 15.0) + 10.0);
        let grad = |hash: u8, x: f32| if hash & 1 == 0 { x } else { -x };
        let g0 = grad(self.perm[xi], xf);
        let g1 = grad(self.perm[(xi + 1) & 255], xf - 1.0);
        (g0 + u * (g1 - g0)) * 2.0
    }
    pub fn fbm(&self, x: f32, octaves: i32, lacunarity: f32, persistence: f32) -> f32 {
        let (mut sum, mut amp, mut freq, mut max_amp) = (0.0, 1.0, 1.0, 0.0);
        for _ in 0..octaves {
            sum += self.sample(x * freq) * amp;
            max_amp += amp;
            freq *= lacunarity;
            amp *= persistence;
        }
        sum / max_amp
    }
}
