//! Opt-in raster broad phase for stationary ET_FX runners and their descendants.
//! Groups are keyed by source entity, never by a particle's current spatial cell.
//! Bounds are rebuilt from live raster primitives: the emitter point alone is unsafe.

use super::{
    draw::FxView,
    system::{make_normal_vectors, FxDraw},
};
use std::time::Instant;

const MAX_RUNNERS: usize = 1024;

#[derive(Clone, Copy)]
struct Bounds {
    lo: [f32; 3],
    hi: [f32; 3],
    unknown: bool,
    hidden: bool,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            lo: [f32::INFINITY; 3],
            hi: [f32::NEG_INFINITY; 3],
            unknown: false,
            hidden: false,
        }
    }
}

/// All storage is lazy and retained across enabled frames. OFF never calls this module.
#[derive(Default)]
pub struct RunnerCullScratch {
    bounds: Vec<Bounds>,
    touched: Vec<usize>,
    visible: Vec<bool>,
    report_at: Option<Instant>,
}

impl RunnerCullScratch {
    pub fn mask(
        &mut self,
        draws: &[FxDraw],
        owners: &[(usize, u16)],
        view: &FxView,
        report: bool,
    ) -> &[bool] {
        let started = report.then(Instant::now);
        self.visible.clear();
        // Empty means fail open, including frames produced before tracking was enabled.
        if owners.is_empty() {
            return &self.visible;
        }
        self.visible.resize(draws.len(), true);
        if !view
            .origin
            .iter()
            .chain(view.axis.iter().flatten())
            .all(|v| v.is_finite())
        {
            return &self.visible;
        }
        let length = view.axis[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        if !length.is_finite() || length < 1.0e-6 {
            return &self.visible;
        }
        let forward = view.axis[0].map(|v| v / length);
        if self.bounds.is_empty() {
            self.bounds.resize(MAX_RUNNERS, Bounds::default());
        }
        for id in self.touched.drain(..) {
            self.bounds[id] = Bounds::default();
        }
        for &(index, id) in owners {
            let Some(draw) = draws.get(index) else {
                continue;
            };
            let Some(bound) = self.bounds.get_mut(usize::from(id)) else {
                continue;
            };
            if bound.lo[0] == f32::INFINITY && !bound.unknown {
                self.touched.push(usize::from(id));
            }
            match draw_sphere(draw, view) {
                Some((center, radius)) => {
                    for a in 0..3 {
                        bound.lo[a] = bound.lo[a].min(center[a] - radius);
                        bound.hi[a] = bound.hi[a].max(center[a] + radius);
                    }
                }
                None => bound.unknown = true,
            }
        }
        let mut rejected = 0;
        let mut unknown = 0;
        for &id in &self.touched {
            let b = &mut self.bounds[id];
            if b.unknown {
                unknown += 1;
                continue;
            }
            let center: [f32; 3] = std::array::from_fn(|a| b.lo[a] * 0.5 + b.hi[a] * 0.5);
            let extent: [f32; 3] = std::array::from_fn(|a| b.hi[a] - center[a]);
            let depth = (0..3)
                .map(|a| (center[a] - view.origin[a]) * forward[a])
                .sum::<f32>();
            let reach = (0..3).map(|a| extent[a] * forward[a].abs()).sum::<f32>();
            // No hardcoded FOV, point PVS test, or assumed wall occlusion.
            b.hidden = depth.is_finite() && reach.is_finite() && depth + reach < -1.0;
            rejected += usize::from(b.hidden);
        }
        let mut pruned = 0;
        for &(index, id) in owners {
            if self.bounds.get(usize::from(id)).is_some_and(|b| b.hidden) {
                if let Some(v) = self.visible.get_mut(index) {
                    pruned += usize::from(*v);
                    *v = false;
                }
            }
        }
        if let Some(start) = started {
            if self
                .report_at
                .is_none_or(|t| t.elapsed().as_secs_f32() >= 1.0)
            {
                println!("FX RUNNER A/B: groups={} rear_groups={} unknown_groups={} draws_pruned={} cpu_cull_us={} (CPU raster only; resident GPU particles retained)",
                    self.touched.len(), rejected, unknown, pruned, start.elapsed().as_micros());
                self.report_at = Some(Instant::now());
            }
        }
        &self.visible
    }
}

fn norm(v: [f32; 3]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn draw_sphere(draw: &FxDraw, view: &FxView) -> Option<([f32; 3], f32)> {
    let (center, radius) = match draw {
        FxDraw::Sprite { origin, radius, .. } => (
            *origin,
            radius.abs() * (norm(view.axis[1]) + norm(view.axis[2])),
        ),
        FxDraw::OrientedQuad {
            origin,
            axis,
            radius,
            ..
        } => {
            if !axis.iter().flatten().all(|v| v.is_finite()) {
                return None;
            }
            // Rotation mixes both basis vectors. This sum bound also covers scaled axes.
            (
                *origin,
                radius.abs() * 2.0 * (norm(axis[1]) + norm(axis[2])),
            )
        }
        FxDraw::Line {
            start, end, width, ..
        } => {
            if !start.iter().chain(end).all(|v| v.is_finite()) {
                return None;
            }
            let middle = std::array::from_fn(|a| start[a] * 0.5 + end[a] * 0.5);
            let delta = std::array::from_fn(|a| end[a] - start[a]);
            (middle, norm(delta) * 0.5 + width.abs())
        }
        FxDraw::Cylinder {
            start,
            end,
            axis,
            start_radius,
            end_radius,
            ..
        } => {
            if !start.iter().chain(end).chain(axis).all(|v| v.is_finite()) {
                return None;
            }
            if !start_radius.is_finite() || !end_radius.is_finite() {
                return None;
            }
            let (_, up) = make_normal_vectors(*axis);
            let middle = std::array::from_fn(|a| start[a] * 0.5 + end[a] * 0.5);
            let delta = std::array::from_fn(|a| end[a] - start[a]);
            (
                middle,
                norm(delta) * 0.5 + start_radius.abs().max(end_radius.abs()) * norm(up),
            )
        }
        // Unknown raster bounds retain the ENTIRE source group.
        _ => return None,
    };
    (center.iter().all(|v| v.is_finite()) && radius.is_finite()).then_some((center, radius + 1.0))
}

/// Resident promotion removes original draw indices. Keep opt-in ownership aligned,
/// including repeated presentation of a paused frame. Synthetic resident draws have no owner.
pub fn remove_promoted(
    owners: &mut Vec<(usize, u16)>,
    accepted: &std::collections::HashSet<usize>,
) {
    if owners.is_empty() {
        return;
    }
    let mut sorted: Vec<usize> = accepted.iter().copied().collect();
    sorted.sort_unstable();
    owners.retain_mut(|(index, _)| {
        if accepted.contains(index) {
            return false;
        }
        let shift = sorted.partition_point(|v| *v < *index);
        *index -= shift;
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> FxView {
        FxView {
            origin: [0.0; 3],
            axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            fov_x: 90.0,
            seconds: 0.0,
        }
    }
    fn sprite(x: f32) -> FxDraw {
        FxDraw::Sprite { shader_time: 0.0,
            origin: [x, 0.0, 0.0],
            radius: 2.0,
            rotation: 0.0,
            rgba: [255; 4],
            shader: "runner".into(),
        }
    }
    #[test]
    fn runner_cull_preserves_unrelated_and_escaped_particles() {
        let draws = [
            sprite(-600.0),
            sprite(-600.0),
            sprite(600.0),
            sprite(-600.0),
        ];
        let owners = [(0, 1), (1, 2), (2, 2)];
        let mut scratch = RunnerCullScratch::default();
        assert_eq!(
            scratch.mask(&draws, &owners, &view(), false),
            &[false, true, true, true]
        );
        // Recompute from current geometry and camera, never retain old visibility.
        let mut turned = view();
        turned.axis[0] = [-1.0, 0.0, 0.0];
        assert_eq!(
            scratch.mask(&draws, &owners, &turned, false),
            &[true, true, true, true]
        );
        assert!(scratch.mask(&draws, &[], &view(), false).is_empty());
    }
    #[test]
    fn runner_cull_unknown_and_crossing_bounds_fail_open() {
        let mut draws = vec![sprite(-600.0), sprite(-600.0), sprite(-2.0)];
        draws[1] = FxDraw::Mesh {
            positions: vec![],
            uvs: vec![],
            rgba: vec![],
            indices: vec![],
            shader: "unknown".into(),
        };
        let mut scratch = RunnerCullScratch::default();
        assert_eq!(
            scratch.mask(&draws, &[(0, 1), (1, 1), (2, 2)], &view(), false),
            &[true, true, true]
        );
        draws[0] = sprite(f32::NAN);
        assert_eq!(
            scratch.mask(&draws, &[(0, 1), (1, 1)], &view(), false),
            &[true, true, true]
        );
    }
    #[test]
    fn runner_cull_promotion_remaps_only_surviving_original_indices() {
        let mut owners = vec![(0, 1), (2, 1), (3, 2), (5, 3)];
        remove_promoted(&mut owners, &[0, 2, 4].into_iter().collect());
        assert_eq!(owners, [(1, 2), (2, 3)]);
    }
}
