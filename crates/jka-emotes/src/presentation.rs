//! Engine-neutral presentation tracks baked from the source animation.
use serde_json::Value;
use std::sync::Arc;
#[derive(Clone, Debug)]
pub struct Prop {
    pub name: String,
    pub model: String,
    pub bone: String,
    pub track: String,
    pub scale: f32,
    pub intervals: Vec<[f32; 2]>,
    pub pose: Option<Arc<crate::PoseClip>>,
    pub skin_track: Option<String>,
    pub skin_pose: Option<Arc<crate::PoseClip>>,
}
impl Prop {
    pub fn sample_elapsed_interpolated(
        &self,
        elapsed_ms: i32,
        fps: f32,
        loop_start: Option<usize>,
        interpolation: crate::Interpolation,
    ) -> Option<jka_assets::ghoul2::Matrix3x4> {
        self.pose
            .as_ref()?
            .sample_unrooted_interpolated(elapsed_ms, fps, loop_start, interpolation)
            .into_iter()
            .next()
    }
    /// Attachment tracks are already relative to the parent skin matrix;
    /// applying Ghoul2's root conversion here would rotate them twice.
    pub fn sample(
        &self,
        seconds: f32,
        fps: f32,
        loop_start: Option<usize>,
    ) -> Option<jka_assets::ghoul2::Matrix3x4> {
        self.sample_interpolated(seconds, fps, loop_start, crate::Interpolation::Linear)
    }
    pub fn sample_interpolated(
        &self,
        seconds: f32,
        fps: f32,
        loop_start: Option<usize>,
        interpolation: crate::Interpolation,
    ) -> Option<jka_assets::ghoul2::Matrix3x4> {
        let clip = self.pose.as_ref()?;
        let frame = (seconds * fps)
            .max(0.)
            .min(clip.frames as f32 - f32::EPSILON * clip.frames as f32);
        let i = (frame.floor() as usize).min(clip.frames - 1);
        let j = if i + 1 < clip.frames {
            i + 1
        } else {
            loop_start.filter(|&s| s < clip.frames).unwrap_or(i)
        };
        let a = clip.matrices()[i];
        let b = clip.matrices()[j];
        let p = clip.matrices()[i.saturating_sub(1)];
        let q = clip.matrices()[if j + 1 < clip.frames {
            j + 1
        } else {
            loop_start.filter(|&s| s < clip.frames).unwrap_or(j)
        }];
        Some(crate::pose::interpolate_matrix(
            p,
            a,
            b,
            q,
            frame.fract(),
            interpolation,
        ))
    }
    pub fn parse(v: &Value) -> Result<Self, String> {
        let string = |key| {
            v[key]
                .as_str()
                .ok_or_else(|| format!("presentation: missing {key}"))
        };
        let scale = v["scale"].as_f64().unwrap_or(1.) as f32;
        if !scale.is_finite() || !(0.001..=100.).contains(&scale) {
            return Err("invalid prop scale".into());
        }
        let intervals = v["intervals"]
            .as_array()
            .ok_or("missing prop lifetime")?
            .iter()
            .map(|a| {
                let start = a[0].as_f64().ok_or("invalid prop start")? as f32;
                let end = a[1].as_f64().ok_or("invalid prop end")? as f32;
                if !start.is_finite()
                    || !end.is_finite()
                    || start < 0.
                    || end <= start
                    || end > 3600.
                {
                    return Err("invalid prop interval".into());
                }
                Ok([start, end])
            })
            .collect::<Result<Vec<_>, String>>()?;
        if intervals.len() > 4096 {
            return Err("too many prop intervals".into());
        }
        Ok(Self {
            name: string("name")?.into(),
            model: crate::manifest::qpath(string("model")?)?,
            bone: string("bone")?.into(),
            track: crate::manifest::qpath(string("track")?)?,
            scale,
            intervals,
            pose: None,
            skin_track: v["skin_track"]
                .as_str()
                .map(crate::manifest::qpath)
                .transpose()?,
            skin_pose: None,
        })
    }
}
/// Local variant clock, shared by the character pose and its notify lifetimes.
pub fn clock(elapsed_ms: i32, fps: f32, frames: usize, loop_start: Option<usize>) -> f32 {
    let frame = (elapsed_ms.max(0) as f64 * fps as f64 / 1000.).max(0.);
    let count = frames.max(1) as f64;
    let frame = if frame >= count {
        if let Some(start) = loop_start.filter(|&s| s < frames) {
            start as f64 + (frame - count) % (count - start as f64)
        } else {
            count - 1.
        }
    } else {
        frame
    };
    frame as f32 / fps
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intro_and_loop_share_the_same_clock() {
        assert_eq!(clock(1000, 30., 90, Some(30)), 1.);
        assert_eq!(clock(3000, 30., 90, Some(30)), 1.);
        assert_eq!(clock(4000, 30., 90, Some(30)), 2.);
    }
    #[test]
    fn rejects_bad_paths_and_lifetimes() {
        let mut v = serde_json::json!({"name":"Box","model":"models/box.md3","bone":"lhand","track":"props/0.pose","intervals":[[0,2]],"scale":1});
        assert!(Prop::parse(&v).is_ok());
        v["track"] = serde_json::json!("../bad");
        assert!(Prop::parse(&v).is_err());
    }
    #[test]
    fn local_prop_tracks_do_not_apply_the_player_root_twice() {
        let m = [[1., 0., 0., 3.], [0., 1., 0., 4.], [0., 0., 1., 5.]];
        let clip = crate::PoseClip::parse(&crate::PoseClip::encode(1, 1, &[m]).unwrap()).unwrap();
        assert_eq!(clip.sample_unrooted(0, 30., None), vec![m]);
        assert_ne!(clip.sample(0, 30., false), vec![m]);
    }
}
