use jka_assets::ghoul2::{multiply_3x4, Matrix3x4, OPENJK_GHOUL2_ROOT_MATRIX};
pub mod pooled;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interpolation {
    #[default]
    Linear,
    /// Four-key Catmull-Rom, bounded between the adjacent coefficients to
    /// prevent overshooting contacts, scaled props and sharp authored poses.
    Cubic,
}

#[derive(Debug)]
pub struct PoseClip {
    pub frames: usize,
    pub bones: usize,
    matrices: Vec<Matrix3x4>,
}

#[cfg(test)]
mod packing_tests {
    use super::*;
    #[test]
    fn prediction_preserves_every_float_bit_and_rejects_corruption() {
        let matrices: Vec<Matrix3x4> = (0..90)
            .map(|frame| {
                std::array::from_fn(|r| {
                    std::array::from_fn(|c| {
                        if c == 3 {
                            -0.0
                        } else {
                            (frame as f32 * 0.1 + r as f32 + c as f32).sin()
                        }
                    })
                })
            })
            .collect();
        let raw = PoseClip::encode(30, 3, &matrices).unwrap();
        let packed = PoseClip::pack_lossless(&raw).unwrap();
        let decoded = PoseClip::parse(&packed).unwrap();
        assert_eq!(
            PoseClip::encode(decoded.frames, decoded.bones, decoded.matrices()).unwrap(),
            raw
        );
        assert!(PoseClip::parse(&packed[..packed.len() - 1]).is_err());
        let mut corrupt = packed;
        for (plane, byte) in f32::INFINITY
            .to_bits()
            .to_le_bytes()
            .into_iter()
            .enumerate()
        {
            corrupt[16 + plane * 30] = byte;
        }
        assert!(PoseClip::parse(&corrupt).is_err());
    }
    #[test]
    fn compact_precision_preserves_scale_and_bounds_all_coefficients() {
        let matrices: Vec<Matrix3x4> = (0..300)
            .map(|frame| {
                std::array::from_fn(|r| {
                    std::array::from_fn(|c| {
                        if c == 3 {
                            frame as f32 * -0.07113
                        } else {
                            (frame as f32 * 0.013 + r as f32 + c as f32).sin() * 3.0
                        }
                    })
                })
            })
            .collect();
        let raw = PoseClip::encode(100, 3, &matrices).unwrap();
        let packed = PoseClip::pack_compact(&raw).unwrap();
        assert!(packed.starts_with(b"DJKPOSE3"));
        let decoded = PoseClip::parse(&packed).unwrap();
        for (a, b) in matrices.iter().zip(decoded.matrices()) {
            for r in 0..3 {
                for c in 0..4 {
                    assert!((a[r][c] - b[r][c]).abs() <= if c == 3 { 0.00051 } else { 0.0000051 });
                }
            }
        }
        assert!(PoseClip::parse(&packed[..packed.len() - 1]).is_err());
    }
}
impl PoseClip {
    pub fn matrices(&self) -> &[Matrix3x4] {
        &self.matrices
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.starts_with(b"DJKPOSE4") {
            return pooled::decode(bytes);
        }
        if bytes.len() < 16 || !matches!(&bytes[..8], b"DJKPOSE1" | b"DJKPOSE2" | b"DJKPOSE3") {
            return Err("invalid baked pose header".into());
        }
        let frames = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let bones = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        if frames == 0 || frames > 1_000_000 || bones == 0 || bones > 4096 {
            return Err("invalid baked pose dimensions".into());
        }
        let count = frames.checked_mul(bones).ok_or("pose count overflow")?;
        if count.checked_mul(48).and_then(|n| n.checked_add(16)) != Some(bytes.len()) {
            return Err("baked pose size mismatch".into());
        }
        if matches!(&bytes[..8], b"DJKPOSE2" | b"DJKPOSE3") {
            let compact = &bytes[..8] == b"DJKPOSE3";
            let mut raw = vec![0u8; bytes.len()];
            raw[..16].copy_from_slice(&bytes[..16]);
            raw[..8].copy_from_slice(b"DJKPOSE1");
            for channel in 0..bones * 12 {
                let mut previous = 0u32;
                for frame in 0..frames {
                    let delta = u32::from_le_bytes(std::array::from_fn(|plane| {
                        bytes[16 + (channel * 4 + plane) * frames + frame]
                    }));
                    previous = if compact {
                        previous.wrapping_add(delta)
                    } else {
                        previous ^ delta
                    };
                    let at = 16 + (frame * bones * 12 + channel) * 4;
                    let bits = if compact {
                        let scale = if channel % 4 == 3 { 1000.0 } else { 100000.0 };
                        (previous as i32 as f32 / scale).to_bits()
                    } else {
                        previous
                    };
                    raw[at..at + 4].copy_from_slice(&bits.to_le_bytes());
                }
            }
            return Self::parse(&raw);
        }
        let mut matrices = Vec::with_capacity(count);
        for raw in bytes[16..].as_chunks::<48>().0 {
            let mut m = [[0.0; 4]; 3];
            for (r, row) in m.iter_mut().enumerate() {
                for (c, value) in row.iter_mut().enumerate() {
                    let at = (r * 4 + c) * 4;
                    let n = f32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
                    if !n.is_finite() || n.abs() > 10000.0 {
                        return Err("invalid baked pose transform".into());
                    }
                    *value = n;
                }
            }
            matrices.push(m);
        }
        Ok(Self {
            frames,
            bones,
            matrices,
        })
    }
    pub fn encode(frames: usize, bones: usize, matrices: &[Matrix3x4]) -> Result<Vec<u8>, String> {
        if matrices.len()
            != frames
                .checked_mul(bones)
                .ok_or("pose dimensions overflow")?
        {
            return Err("pose dimensions mismatch".into());
        }
        let mut bytes = Vec::with_capacity(16 + matrices.len() * 48);
        bytes.extend_from_slice(b"DJKPOSE1");
        bytes.extend_from_slice(&(frames as u32).to_le_bytes());
        bytes.extend_from_slice(&(bones as u32).to_le_bytes());
        for m in matrices {
            for row in m {
                for n in row {
                    bytes.extend_from_slice(&n.to_le_bytes());
                }
            }
        }
        Self::parse(&bytes)?;
        Ok(bytes)
    }
    /// Exact float-bit prediction and byte planes improve ZIP compression without
    /// changing any matrix value, animated scale or finger motion.
    pub fn pack_lossless(bytes: &[u8]) -> Result<Vec<u8>, String> {
        let clip = Self::parse(bytes)?;
        let mut out = vec![0u8; 16 + clip.frames * clip.bones * 48];
        out[..8].copy_from_slice(b"DJKPOSE2");
        out[8..12].copy_from_slice(&(clip.frames as u32).to_le_bytes());
        out[12..16].copy_from_slice(&(clip.bones as u32).to_le_bytes());
        for channel in 0..clip.bones * 12 {
            let mut previous = 0u32;
            for frame in 0..clip.frames {
                let m = &clip.matrices[frame * clip.bones + channel / 12];
                let bits = m[(channel % 12) / 4][channel % 4].to_bits();
                let delta = (bits ^ previous).to_le_bytes();
                previous = bits;
                for plane in 0..4 {
                    out[16 + (channel * 4 + plane) * clip.frames + frame] = delta[plane];
                }
            }
        }
        Ok(out)
    }
    /// Shipping precision: 0.00001 for linear coefficients, 0.001 game units
    /// for translation. Keep every frame, scale and joint. Each rounding error
    /// is checked; unusual coefficients fall back to the lossless representation.
    pub fn pack_compact(bytes: &[u8]) -> Result<Vec<u8>, String> {
        let clip = Self::parse(bytes)?;
        let mut out = vec![0u8; 16 + clip.frames * clip.bones * 48];
        out[..8].copy_from_slice(b"DJKPOSE3");
        out[8..12].copy_from_slice(&(clip.frames as u32).to_le_bytes());
        out[12..16].copy_from_slice(&(clip.bones as u32).to_le_bytes());
        for channel in 0..clip.bones * 12 {
            let scale = if channel % 4 == 3 { 1000.0 } else { 100000.0 };
            let tolerance = if channel % 4 == 3 { 0.00051 } else { 0.0000051 };
            let mut previous = 0u32;
            for frame in 0..clip.frames {
                let m = &clip.matrices[frame * clip.bones + channel / 12];
                let value = m[(channel % 12) / 4][channel % 4];
                let integer = (value as f64 * scale as f64).round() as i32;
                if (integer as f32 / scale - value).abs() > tolerance {
                    return Self::pack_lossless(bytes);
                }
                let bits = integer as u32;
                let delta = bits.wrapping_sub(previous).to_le_bytes();
                previous = bits;
                for plane in 0..4 {
                    out[16 + (channel * 4 + plane) * clip.frames + frame] = delta[plane];
                }
            }
        }
        Ok(out)
    }
    /// Interpolated skin matrices use the same matrix lerp as Ghoul2. The
    /// canonical OpenJK model-root transform is applied exactly once here.
    pub fn sample(&self, elapsed_ms: i32, fps: f32, looping: bool) -> Vec<Matrix3x4> {
        self.sample_range(elapsed_ms, fps, 0, self.frames, looping.then_some(0))
    }
    /// Sample a variant in a shared pose track without blending into a neighbor.
    pub fn sample_range(
        &self,
        elapsed_ms: i32,
        fps: f32,
        start: usize,
        end: usize,
        loop_start: Option<usize>,
    ) -> Vec<Matrix3x4> {
        self.sample_range_interpolated(
            elapsed_ms,
            fps,
            start,
            end,
            loop_start,
            Interpolation::Linear,
        )
    }
    pub fn sample_range_interpolated(
        &self,
        elapsed_ms: i32,
        fps: f32,
        start: usize,
        end: usize,
        loop_start: Option<usize>,
        interpolation: Interpolation,
    ) -> Vec<Matrix3x4> {
        self.sample_range_impl(elapsed_ms, fps, start, end, loop_start, true, interpolation)
    }
    /// Native prop-local matrices already use the engine's model coordinates.
    pub fn sample_unrooted(
        &self,
        elapsed_ms: i32,
        fps: f32,
        loop_start: Option<usize>,
    ) -> Vec<Matrix3x4> {
        self.sample_unrooted_interpolated(elapsed_ms, fps, loop_start, Interpolation::Linear)
    }
    pub fn sample_unrooted_interpolated(
        &self,
        elapsed_ms: i32,
        fps: f32,
        loop_start: Option<usize>,
        interpolation: Interpolation,
    ) -> Vec<Matrix3x4> {
        self.sample_range_impl(
            elapsed_ms,
            fps,
            0,
            self.frames,
            loop_start,
            false,
            interpolation,
        )
    }
    fn sample_range_impl(
        &self,
        elapsed_ms: i32,
        fps: f32,
        start: usize,
        end: usize,
        loop_start: Option<usize>,
        apply_root: bool,
        interpolation: Interpolation,
    ) -> Vec<Matrix3x4> {
        let start = start.min(self.frames - 1);
        let end = end.clamp(start + 1, self.frames);
        let looping = loop_start.is_some();
        let loop_start = loop_start.unwrap_or(start).clamp(start, end - 1);
        let fps = if fps.is_finite() && fps > 0. {
            fps
        } else {
            30.
        };
        let frame = elapsed_ms.max(0) as f64 * fps as f64 / 1000.0;
        let repeated = looping && frame >= (end - start) as f64;
        let frame = if looping && frame >= (end - start) as f64 {
            loop_start as f64 + (frame - (end - start) as f64) % (end - loop_start) as f64
        } else {
            (start as f64 + frame).min(if looping {
                end as f64 - f64::EPSILON * end as f64
            } else {
                (end - 1) as f64
            })
        };
        let first = frame.floor() as usize;
        let next = if first + 1 < end {
            first + 1
        } else if looping {
            loop_start
        } else {
            first
        };
        let weight = frame.fract() as f32;
        let previous = if first > start && (!repeated || first > loop_start) {
            first - 1
        } else if repeated {
            end - 1
        } else {
            first
        };
        let after = if next + 1 < end {
            next + 1
        } else if looping {
            loop_start
        } else {
            next
        };
        (0..self.bones)
            .map(|bone| {
                let a = self.matrices[first * self.bones + bone];
                let b = self.matrices[next * self.bones + bone];
                let p = self.matrices[previous * self.bones + bone];
                let q = self.matrices[after * self.bones + bone];
                let m = interpolate_matrix(p, a, b, q, weight, interpolation);
                if apply_root {
                    multiply_3x4(&OPENJK_GHOUL2_ROOT_MATRIX, &m)
                } else {
                    m
                }
            })
            .collect()
    }
}

pub(crate) fn interpolate_matrix(
    p: Matrix3x4,
    a: Matrix3x4,
    b: Matrix3x4,
    q: Matrix3x4,
    t: f32,
    mode: Interpolation,
) -> Matrix3x4 {
    if t <= 0. {
        return a;
    }
    if t >= 1. {
        return b;
    }
    std::array::from_fn(|r| {
        std::array::from_fn(|c| {
            let linear = a[r][c] + (b[r][c] - a[r][c]) * t;
            if mode == Interpolation::Linear {
                return linear;
            }
            let value = 0.5
                * ((2. * a[r][c])
                    + (-p[r][c] + b[r][c]) * t
                    + (2. * p[r][c] - 5. * a[r][c] + 4. * b[r][c] - q[r][c]) * t * t
                    + (-p[r][c] + 3. * a[r][c] - 3. * b[r][c] + q[r][c]) * t * t * t);
            if value.is_finite() {
                value.clamp(a[r][c].min(b[r][c]), a[r][c].max(b[r][c]))
            } else {
                linear
            }
        })
    })
}

#[cfg(test)]
mod interpolation_tests {
    use super::*;
    fn clip(values: &[f32]) -> PoseClip {
        let matrices = values
            .iter()
            .map(|&z| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., z]])
            .collect::<Vec<_>>();
        PoseClip::parse(&PoseClip::encode(values.len(), 1, &matrices).unwrap()).unwrap()
    }
    #[test]
    fn cubic_preserves_keys_and_never_overshoots_adjacent_contacts() {
        let p = clip(&[0., 1., 4., 9.]);
        for ms in 0..400 {
            let frame = (ms as f32 / 100.).min(3.);
            let a = frame.floor() as usize;
            let b = (a + 1).min(3);
            let z = p.sample_unrooted_interpolated(ms, 10., None, Interpolation::Cubic)[0][2][3];
            assert!(z >= p.matrices[a][2][3] - 1e-6 && z <= p.matrices[b][2][3] + 1e-6);
            if ms % 100 == 0 {
                assert_eq!(z, p.matrices[a][2][3]);
            }
        }
        assert_ne!(
            p.sample_unrooted_interpolated(150, 10., None, Interpolation::Cubic),
            p.sample_unrooted(150, 10., None)
        );
    }
    #[test]
    fn repeated_loop_controls_do_not_use_the_intro() {
        let p = clip(&[999., 10., 20., 30.]);
        assert!(
            (p.sample_unrooted_interpolated(450, 10., Some(1), Interpolation::Cubic)[0][2][3]
                - 13.125)
                .abs()
                < 1e-6
        );
        assert_eq!(
            p.sample_unrooted_interpolated(400, 10., Some(1), Interpolation::Cubic)[0][2][3],
            10.
        );
    }
    #[test]
    fn adjacent_variants_and_single_frame_clips_stay_isolated() {
        let p = clip(&[-10000., 1., 2., 10000.]);
        let z = p.sample_range_interpolated(50, 10., 1, 3, None, Interpolation::Cubic)[0][2][3];
        let expected = multiply_3x4(
            &OPENJK_GHOUL2_ROOT_MATRIX,
            &[[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 1.5]],
        );
        assert!((z - expected[2][3]).abs() < 1e-6);
        let p = clip(&[7.]);
        assert_eq!(
            p.sample_unrooted_interpolated(i32::MAX, 30., Some(0), Interpolation::Cubic),
            p.matrices
        );
    }
}
