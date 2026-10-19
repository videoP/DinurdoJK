//! Ordered submission runs. Filtering happens before merging so excluded
//! entities and glow stages cannot be pulled back into a combined range.
use crate::renderer::{DynamicPreparedDraw, FxGpuSpritePreparedDraw, DynamicModelAlphaMode,
    dynamic_model_writes_depth, DYNAMIC_MODEL_ALPHA_ORDER};
use std::ops::Range;

pub(in crate::renderer) fn compositing_requires_order(mode: DynamicModelAlphaMode) -> bool {
    matches!(mode, DynamicModelAlphaMode::Modulate | DynamicModelAlphaMode::Modulate2x
        | DynamicModelAlphaMode::DstColorAdd | DynamicModelAlphaMode::Darken)
}

/// Framebuffer filters do not commute with additive/alpha passes. Preserve
/// their submission sequence, batching only consecutive modes. Other draws
/// retain the existing mode grouping and bundle path without allocating.
pub(in crate::renderer) fn model_phase_runs(
    modes: impl Iterator<Item = DynamicModelAlphaMode>,
    preserve_order: bool,
    depth_writing_phase: bool,
) -> impl Iterator<Item = (DynamicModelAlphaMode, Range<usize>)> {
    let mut grouped = DYNAMIC_MODEL_ALPHA_ORDER.into_iter();
    let mut ordered = modes.enumerate().peekable();
    std::iter::from_fn(move || {
        if !preserve_order {
            return grouped.find(|mode| dynamic_model_writes_depth(*mode) == depth_writing_phase)
                .map(|mode| (mode, 0..usize::MAX));
        }
        let (start, mode) = ordered.find(|(_, mode)| dynamic_model_writes_depth(*mode) == depth_writing_phase)?;
        let mut end = start + 1;
        while ordered.peek().is_some_and(|(_, next)| *next == mode) {
            end = ordered.next().unwrap().0 + 1;
        }
        Some((mode, start..end))
    })
}

/// Capture-only API calls in draw_phase, including repeated secondary/glow views.
/// Does not include wireframe, shadow helpers, or compute skinning commands.
#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct ModelPhaseCalls {
    pub cpu_draw_indexed: u32,
    pub bundle_executes: u32,
    pub bundle_draw_indexed: u32,
    pub fx_draw: u32,
    pub skin_draw_indexed: u32,
    pub set_pipeline: u32,
    pub set_bind_group: u32,
    pub set_vertex_buffer: u32,
    pub set_index_buffer: u32,
}

pub(in crate::renderer) fn model_runs<'a>(
    draws: impl Iterator<Item = &'a DynamicPreparedDraw>,
    merge: bool,
) -> impl Iterator<Item = (&'a DynamicPreparedDraw, Range<u32>)> {
    let mut draws = draws.peekable();
    std::iter::from_fn(move || {
        let first = draws.next()?;
        let mut range = first.first_index..first.first_index.saturating_add(first.index_count);
        if merge && first.index_count % 3 == 0 {
            while let Some(&next) = draws.peek() {
                if next.first_index != range.end
                    || next.index_count % 3 != 0
                    || next.base_vertex != first.base_vertex
                    || next.texture_key != first.texture_key
                    || next.alpha_mode != first.alpha_mode
                    || next.wireframe_class != first.wireframe_class
                    || next.authored_glow != first.authored_glow
                {
                    break;
                }
                range.end = next.first_index.saturating_add(next.index_count);
                draws.next();
            }
        }
        Some((first, range))
    })
}

pub(in crate::renderer) fn fx_runs<'a>(
    draws: impl Iterator<Item = &'a FxGpuSpritePreparedDraw>,
    merge: bool,
) -> impl Iterator<Item = (&'a FxGpuSpritePreparedDraw, Range<u32>)> {
    let mut draws = draws.peekable();
    std::iter::from_fn(move || {
        let first = draws.next()?;
        let mut range =
            first.first_instance..first.first_instance.saturating_add(first.instance_count);
        if merge {
            while let Some(&next) = draws.peek() {
                if next.first_instance != range.end
                    || next.texture_key != first.texture_key
                    || next.alpha_mode != first.alpha_mode
                    || next.wireframe_class != first.wireframe_class
                    || next.authored_glow != first.authored_glow
                {
                    break;
                }
                range.end = next.first_instance.saturating_add(next.instance_count);
                draws.next();
            }
        }
        Some((first, range))
    })
}

/// Depth-only passes: merge contiguous index ranges of one alpha mode. Opaque
/// depth never samples the texture (`by_texture` false), so material, glow and
/// wireframe boundaries do not split it; alpha-tested depth keeps texture runs.
pub(in crate::renderer) fn depth_runs<'a>(
    draws: impl Iterator<Item = &'a DynamicPreparedDraw>,
    merge: bool,
    by_texture: bool,
) -> impl Iterator<Item = (&'a DynamicPreparedDraw, Range<u32>)> {
    let mut draws = draws.peekable();
    std::iter::from_fn(move || {
        let first = draws.next()?;
        let mut range = first.first_index..first.first_index.saturating_add(first.index_count);
        if merge && first.index_count % 3 == 0 {
            while let Some(&next) = draws.peek() {
                if next.first_index != range.end
                    || next.index_count % 3 != 0
                    || next.base_vertex != first.base_vertex
                    || (by_texture && next.texture_key != first.texture_key)
                {
                    break;
                }
                range.end = next.first_index.saturating_add(next.index_count);
                draws.next();
            }
        }
        Some((first, range))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{DynamicModelAlphaMode, DynamicWireframeClass};

    #[test]
    fn framebuffer_filters_keep_their_position_between_additive_passes() {
        use DynamicModelAlphaMode::{Opaque, Modulate2x, AdditiveOne};
        let modes = [Opaque, AdditiveOne, Modulate2x, AdditiveOne, AdditiveOne];
        let runs: Vec<_> = model_phase_runs(modes.into_iter(), true, false).collect();
        assert_eq!(runs, [(AdditiveOne, 1..2), (Modulate2x, 2..3), (AdditiveOne, 3..5)]);
        let reversed = [Opaque, Modulate2x, AdditiveOne];
        assert_eq!(model_phase_runs(reversed.into_iter(), true, false).collect::<Vec<_>>(),
            [(Modulate2x, 1..2), (AdditiveOne, 2..3)]);
        assert_eq!(model_phase_runs(modes.into_iter(), true, true).collect::<Vec<_>>(), [(Opaque, 0..1)]);
    }

    fn model(entity_num: u16, first_index: u32, index_count: u32) -> DynamicPreparedDraw {
        DynamicPreparedDraw {
            entity_num,
            first_index,
            index_count,
            base_vertex: 0,
            texture_key: None,
            alpha_mode: DynamicModelAlphaMode::Blend,
            wireframe_class: DynamicWireframeClass::Entity,
            authored_glow: false,
        }
    }

    #[test]
    fn depth_runs_ignore_material_only_for_opaque() {
        let mut draws = [model(1, 0, 3), model(2, 3, 3), model(3, 9, 3), model(4, 12, 3)];
        draws[1].texture_key = Some("other".into());
        draws[1].authored_glow = true;
        let ranges = |by_texture| depth_runs(draws.iter(), true, by_texture)
            .map(|(_, r)| r).collect::<Vec<_>>();
        assert_eq!(ranges(false), vec![0..6, 9..15]);
        assert_eq!(ranges(true), vec![0..3, 3..6, 9..15]);
        assert_eq!(depth_runs(draws.iter(), false, false).count(), 4);
        draws[3].base_vertex = 3;
        assert_eq!(depth_runs(draws.iter(), true, false).count(), 3);
    }

    #[test]
    fn model_runs_preserve_order_and_do_not_cross_filtered_geometry() {
        let draws = [
            model(1, 0, 3),
            model(2, 3, 6),
            model(3, 9, 3),
            model(4, 12, 3),
        ];
        let ranges = |merge| {
            model_runs(draws.iter(), merge)
                .map(|(_, r)| r)
                .collect::<Vec<_>>()
        };
        assert_eq!(ranges(false), vec![0..3, 3..9, 9..12, 12..15]);
        assert_eq!(ranges(true), vec![0..15]);
        let filtered = model_runs(draws.iter().filter(|d| d.entity_num != 2), true)
            .map(|(_, r)| r)
            .collect::<Vec<_>>();
        assert_eq!(filtered, vec![0..3, 9..15]);
    }

    #[test]
    fn model_runs_keep_material_base_vertex_glow_and_primitive_boundaries() {
        for incompatible in 0..5 {
            let mut draws = [model(1, 0, 3), model(2, 3, 3)];
            match incompatible {
                0 => draws[1].texture_key = Some("other".into()),
                1 => draws[1].base_vertex = 3,
                2 => draws[1].authored_glow = true,
                3 => draws[1].alpha_mode = DynamicModelAlphaMode::Opaque,
                _ => draws[0].index_count = 2,
            }
            assert_eq!(model_runs(draws.iter(), true).count(), 2);
        }
    }

    fn sprite(entity_num: u16, first_instance: u32) -> FxGpuSpritePreparedDraw {
        FxGpuSpritePreparedDraw {
            entity_num,
            first_instance,
            instance_count: 2,
            texture_key: None,
            alpha_mode: DynamicModelAlphaMode::Additive,
            wireframe_class: DynamicWireframeClass::Effect,
            authored_glow: false,
        }
    }

    #[test]
    fn fx_runs_preserve_instance_order_and_filtering() {
        let mut draws = [sprite(1, 0), sprite(2, 2), sprite(3, 4)];
        assert_eq!(fx_runs(draws.iter(), false).count(), 3);
        assert_eq!(
            fx_runs(draws.iter(), true)
                .map(|(_, r)| r)
                .collect::<Vec<_>>(),
            vec![0..6]
        );
        assert_eq!(
            fx_runs(draws.iter().filter(|d| d.entity_num != 2), true)
                .map(|(_, r)| r)
                .collect::<Vec<_>>(),
            vec![0..2, 4..6]
        );
        draws[1].texture_key = Some("other".into());
        assert_eq!(fx_runs(draws.iter(), true).count(), 3);
        draws[1].texture_key = None;
        draws[1].authored_glow = true;
        assert_eq!(fx_runs(draws.iter(), true).count(), 3);
    }
}
