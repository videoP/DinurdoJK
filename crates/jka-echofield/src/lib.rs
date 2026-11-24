//! EchoField algorithm port and static BSP acoustic fallback core.
//! Upstream: https://github.com/willnap/echofield, GPL-3.0.
//! Authority: clean 4a595e7, plus audited later SoundPhysics send blending.
//! See PORT_STATUS.md for remaining parity and integration gaps.
#![forbid(unsafe_code)]

pub mod acoustic;
pub mod environment;
pub mod excitation;
pub mod geometry;
pub mod motion;
pub mod noise;
pub mod random;
pub mod scheduler;
pub mod settings;
pub mod soundscape;
pub mod syrinx;
pub mod terrain;

/// Borrowed authored baseline. No approximation of EAX parameters, no DSP,
/// and no claim that the matrix lookup includes native EalMan diffraction.
/// All positions for the provider are in native JKA world coordinates.
#[derive(Debug, Clone, Copy)]
pub struct AuthoredBaseline<'a> {
    pub listener_environment_id: i32,
    pub source_environment_id: i32,
    pub listener_preset_words: &'a [u32],
    pub source_preset_words: &'a [u32],
    pub obstacle: Option<AuthoredObstacle>,
}
#[derive(Debug, Clone, Copy)]
pub struct AuthoredObstacle {
    pub index: i32,
    pub level_mb: i32,
    pub lf_ratio: f32,
    pub room_ratio: f32,
    pub obstructs: bool,
}

pub trait AuthoredAcoustics {
    fn baseline(&self, listener: [f32; 3], source: [f32; 3]) -> Option<AuthoredBaseline<'_>>;
}

pub mod procedural;
pub mod profile;
pub mod propagation;
pub mod reverb;
pub mod terrain_audio;
pub mod world;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinate_roundtrip() {
        let p = [128., -64., 32.];
        assert_eq!(geometry::to_upstream(p), [4., 1., 2.]);
        assert_eq!(geometry::from_upstream(geometry::to_upstream(p)), p);
    }
    #[test]
    fn efx_snapshot_ranges() {
        for s in reverb::base_slots(1., 1., 1.) {
            assert!((0.1..=20.).contains(&s.decay_time));
            assert!((0.0..=1.).contains(&s.gain));
        }
    }
    #[test]
    fn empty_world_propagation_is_direct() {
        struct Empty;
        impl profile::AcousticWorld for Empty {
            fn cast(&mut self, _: geometry::Vec3, _: geometry::Vec3) -> Option<profile::Hit> {
                None
            }
            fn occlusion(&mut self, _: geometry::Vec3, _: geometry::Vec3, _: f64, _: usize) -> f64 {
                0.
            }
        }
        let source = propagation::Source {
            position: [5., 0., 0.],
            max_distance: 30.,
            underwater: false,
            block_sound: false,
            voicechat: false,
            aux_only: false,
        };
        let r = propagation::evaluate(
            &mut Empty,
            source,
            [0.; 3],
            false,
            propagation::Config::default(),
        );
        assert_eq!(r.direct_gain, 1.);
        assert_eq!(r.direct_cutoff, 1.);
        assert_eq!(r.send_gain, [0.; 4]);
    }
}
