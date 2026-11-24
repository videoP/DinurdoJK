//! java.util.Random's 48-bit LCG, required to preserve upstream seeded calls.
//! Oracle: Java Random's setSeed, next, nextInt(bound), nextFloat.
#[derive(Debug, Clone)]
pub struct JavaRandom {
    seed: u64,
    gaussian: Option<f64>,
}
impl JavaRandom {
    const MULTIPLIER: u64 = 0x5deece66d;
    const MASK: u64 = (1 << 48) - 1;
    pub fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64 ^ Self::MULTIPLIER) & Self::MASK,
            gaussian: None,
        }
    }
    fn next(&mut self, bits: u32) -> u32 {
        self.seed = self.seed.wrapping_mul(Self::MULTIPLIER).wrapping_add(11) & Self::MASK;
        (self.seed >> (48 - bits)) as u32
    }
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / 16_777_216.0
    }
    pub fn next_long(&mut self) -> i64 {
        let hi = self.next(32) as i32 as i64;
        let lo = self.next(32) as i32 as i64;
        (hi << 32).wrapping_add(lo)
    }
    pub fn next_double(&mut self) -> f64 {
        let hi = u64::from(self.next(26));
        let lo = u64::from(self.next(27));
        ((hi << 27) + lo) as f64 / 9_007_199_254_740_992.0
    }
    pub fn next_gaussian(&mut self) -> f64 {
        if let Some(x) = self.gaussian.take() {
            return x;
        }
        loop {
            let a = 2. * self.next_double() - 1.;
            let b = 2. * self.next_double() - 1.;
            let s = a * a + b * b;
            if s >= 1. || s == 0. {
                continue;
            }
            let m = (-2. * s.ln() / s).sqrt();
            self.gaussian = Some(b * m);
            return a * m;
        }
    }
    pub fn next_int(&mut self, bound: i32) -> i32 {
        assert!(bound > 0);
        if (bound & (bound - 1)) == 0 {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31) as i32;
            let val = bits % bound;
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }
}
