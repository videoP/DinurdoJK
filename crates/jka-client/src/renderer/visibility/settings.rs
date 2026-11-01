//! Visibility settings.
use crate::renderer::{
    competitive_probe as probe, competitive_probe_mask,
    create_gpu_cull_bind_group, create_hiz_build_bind_group, create_hiz_reduce_bind_groups,
    BlendFactor, BlendMode, CullDebugMode, DrawClass, GpuCullSettings, PipelineKey, Renderer,
};

/// Classified by the *resolved GPU blend equation*, not by material qpath or
/// the human-readable shader stage count. Zero means nonblended/unclassified.
/// The four nonzero categories are disjoint by construction. These codes are
/// written into previously unused cull-record bits 5..7 so GPU and CPU probes
/// reject the same stage even for AUTO-4 lazy recipes.
pub(in crate::renderer) fn world_blend_probe_bucket(key: PipelineKey) -> u32 {
    if key.class != DrawClass::Transparent {
        return 0;
    }
    match key.blend {
        BlendMode::Opaque => 0,
        // Common lightmap modulation (GL_DST_COLOR GL_ZERO). The dual form
        // GL_ZERO GL_SRC_COLOR also multiplies source and destination.
        BlendMode::Custom(BlendFactor::DstColor, BlendFactor::Zero)
        | BlendMode::Custom(BlendFactor::Zero, BlendFactor::SrcColor) => 1,
        // Additive emission/caustics, including alpha-weighted additive.
        BlendMode::Custom(BlendFactor::One, BlendFactor::One)
        | BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::One)
        | BlendMode::Custom(BlendFactor::One, BlendFactor::SrcAlpha) => 2,
        // Conventional and premultiplied source-alpha compositing.
        BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)
        | BlendMode::Custom(BlendFactor::One, BlendFactor::OneMinusSrcAlpha) => 3,
        // Other authored custom factors are not silently folded into alpha.
        BlendMode::Custom(_, _) => 4,
    }
}

/// 0..2 retain the original world-class A/B semantics; 3..6 omit one
/// blend category. 7/8 isolate additive stages on water / non-water BSP
/// materials. The authored surfaceparm is the source of water classification:
/// this is not a qpath-based shader-name heuristic. Depth-producing stages
/// are retained in modes 3..8.
pub(in crate::renderer) fn world_stage_probe_excludes(mode: u8, key: PipelineKey, water: bool) -> bool {
    match mode {
        1 => key.class == DrawClass::Transparent,
        2 => matches!(key.class, DrawClass::Opaque | DrawClass::Mask),
        3..=6 => world_blend_probe_bucket(key) == u32::from(mode - 2),
        7 => water && world_blend_probe_bucket(key) == 2,
        8 => !water && world_blend_probe_bucket(key) == 2,
        _ => false,
    }
}

/// Fast-sky clear colors retain TaystJK's palette for modes 2..10;
/// mode 1 is a pale blue sky instead of TaystJK's black.
/// The plain color replaces authored sky drawing instead of shading a sky fragment.
pub(in crate::renderer) fn fast_sky_clear_color(mode: u8) -> wgpu::Color {
    let rgb = match mode {
        1 => [0.60, 0.76, 0.90], // pale, slightly whitish sky blue (linear RGB)
        2 => [0.0, 1.0, 0.0], // ^2 green
        3 => [1.0, 1.0, 0.0], // ^3 yellow
        4 => [0.0, 0.0, 1.0], // ^4 blue
        5 => [0.0, 1.0, 1.0], // ^5 cyan
        6 => [1.0, 0.0, 1.0], // ^6 magenta
        7 => [1.0, 1.0, 1.0], // ^7 white
        8 => [1.0, 0.5, 0.0], // ^8 orange
        9 => [0.5, 0.5, 0.5], // ^9 grey
        10 => [1.0, 0.0, 0.0], // TaystJK special-case red
        _ => [0.0, 0.0, 0.0], // unused / disabled
    };
    wgpu::Color { r: rgb[0], g: rgb[1], b: rgb[2], a: 1.0 }
}

impl Renderer {
    pub(in crate::renderer) fn set_gpu_visibility(
        &mut self,
        gpu_driven: bool,
        hiz_occlusion: bool,
    ) {
        self.configured_gpu_driven = gpu_driven;
        self.configured_hiz_occlusion = hiz_occlusion;
        self.gpu_driven_enabled = gpu_driven && self.indirect_supported
            && !(5..=14).contains(&self.competitive_ab)
            && competitive_probe_mask(self.competitive_ab)
                & (probe::CPU_VISIBILITY_ONLY | probe::BSP_ONE_STAGE
                    | probe::HIDE_BSP_PROPS | probe::HIDE_TRANSPARENT) == 0;
        self.hiz_occlusion_enabled = hiz_occlusion && self.gpu_driven_enabled;
        self.rebuild_frame_plan();
        self.update_gpu_cull_settings();
    }

    pub(in crate::renderer) fn set_draw_inline_models(&mut self, enabled: bool) {
        if self.draw_inline_models == enabled { return; }
        self.draw_inline_models = enabled;
        // A previously cached local-light cubemap must not retain a hidden
        // model's shadow (nor miss it when models are re-enabled).
        if let Some(world) = self.world.as_mut() {
            let shadows = &mut world.local_shadows;
            for slot in 0..shadows.cache_entries.len() {
                if shadows.cache_entries[slot].light_index.is_some()
                    && !shadows.pending_slots.contains(&slot) {
                    shadows.pending_slots.push(slot);
                }
            }
        }
        self.update_gpu_cull_settings();
    }

    pub(in crate::renderer) fn set_sky_perf_probe(&mut self, mode: u8) {
        self.sky_perf_probe = mode.min(3);
    }

    pub(in crate::renderer) fn set_fast_sky(&mut self, mode: u8) {
        self.configured_fast_sky = mode.min(10);
        self.apply_fast_sky();
    }

    pub(in crate::renderer) fn set_competitive_ab(&mut self, mode: u32) {
        let mode = mode.min(probe::MAX_MODE);
        if self.competitive_ab == mode { return; }
        self.competitive_ab = mode;
        self.apply_fast_sky();
        // GPU indirect draws use separate cull masks; they cannot use the CPU
        // single-stage selection, so temporarily select the normal CPU draw
        // path. Restore the requested GPU mode when the experiment ends.
        self.set_gpu_visibility(self.configured_gpu_driven, self.configured_hiz_occlusion);
        // Changing an active surface set invalidates replay/compaction caches.
        // The original map and its prepared stages remain untouched.
        self.deferred_world_bundles.clear();
        self.update_gpu_cull_settings();
    }

    fn apply_fast_sky(&mut self) {
        let mode = if (5..=14).contains(&self.competitive_ab)
            || competitive_probe_mask(self.competitive_ab) & probe::FORCE_FAST_SKY != 0 {
            self.configured_fast_sky.max(1)
        } else {
            self.configured_fast_sky
        };
        if self.fast_sky == mode { return; }
        self.fast_sky = mode;
        // GPU visibility and cached draw bundles must agree on sky exclusion.
        self.update_gpu_cull_settings();
        self.deferred_world_bundles.clear();
    }

    pub(in crate::renderer) fn set_world_stage_debug(&mut self, draw_bsp_model_surfaces: bool, probe: u8) {
        self.draw_bsp_model_surfaces = draw_bsp_model_surfaces;
        self.world_stage_probe = probe.min(8);
        self.update_gpu_cull_settings();
        // A stale ordered render-bundle plan must not survive changing visibility domains.
        self.deferred_world_bundles.clear();
    }

    pub(in crate::renderer) fn update_gpu_cull_settings(&self) {
        let (active_cull_count, compact_group_count) = self
            .world
            .as_ref()
            .map(|world| {
                (
                    world.active_cull_count,
                    u32::try_from(world.compact_groups.len()).unwrap_or(u32::MAX),
                )
            })
            .unwrap_or((0, 0));
        let settings = GpuCullSettings {
            viewport_mips_flags: [
                self.targets.hiz_width,
                self.targets.hiz_height,
                self.targets.hiz_mip_count.max(1),
                u32::from(self.gpu_driven_enabled && self.hiz_occlusion_enabled)
                    | (u32::from(!self.draw_inline_models) << 1)
                    | (u32::from(!self.draw_bsp_model_surfaces) << 2)
                    | (u32::from(self.world_stage_probe == 1) << 3)
                    | (u32::from(self.world_stage_probe == 2) << 4)
                    | (u32::from(self.fast_sky != 0) << 5)
                    | (u32::from(self.face_plane_cull) << 6)
                    | (u32::from(!self.ui_state.hud.as_ref().is_some_and(|hud|
                        hud.force_powers_active & (1 << 14) != 0)) << 7)
                    // Bits 8..11 encode selective blend and water/non-water A/B.
                    | (u32::from(self.world_stage_probe) << 8),
            ],
            active_compaction: [
                active_cull_count,
                compact_group_count,
                u32::from(
                    self.gpu_driven_enabled
                        && self.gpu_compaction_supported
                        && compact_group_count != 0,
                ),
                u32::from(self.cull_debug_mode != CullDebugMode::Off),
            ],
        };
        self.queue.write_buffer(
            &self.gpu_cull_settings_buffer,
            0,
            bytemuck::bytes_of(&settings),
        );
    }

    pub(in crate::renderer) fn rebuild_gpu_visibility_resources(&mut self) {
        self.hiz_build_bind_group =
            create_hiz_build_bind_group(&self.device, &self.hiz_build_layout, &self.targets);
        self.hiz_reduce_bind_groups =
            create_hiz_reduce_bind_groups(&self.device, &self.hiz_reduce_layout, &self.targets);
        self.update_gpu_cull_settings();
        if let Some(world) = &mut self.world {
            world.cull_bind_group = Some(create_gpu_cull_bind_group(
                &self.device,
                &self.gpu_cull_layout,
                &world.cull_records_buffer,
                &world.indirect_buffer,
                &self.targets.hiz_view,
                &self.gpu_cull_settings_buffer,
                &world.active_cull_indices_buffer,
                &world.compact_indirect_buffer,
                &world.compact_count_buffer,
                &world.cull_debug_reason_buffer,
                &world.cull_debug_count_buffer,
                &world.early_indirect_buffer,
            ));
        }
    }
}

#[cfg(test)]
mod world_blend_probe_tests {
    use super::*;

    fn stage(class: DrawClass, blend: BlendMode) -> PipelineKey {
        PipelineKey {
            legacy_srgb_three_stage: false,
            additive_fusion_count: 0,
            auto_sprite2: false,
            sky_outer: false,
            class,
            blend,
            cull: crate::renderer::CullMode::Back,
            offset: false,
            depth_write: false,
            depth_equal: true,
        }
    }

    #[test]
    fn select_blend_modes_are_exclusive_and_keep_depth_stages() {
        let samples = [
            (BlendMode::Custom(BlendFactor::DstColor, BlendFactor::Zero), 1),
            (BlendMode::Custom(BlendFactor::Zero, BlendFactor::SrcColor), 1),
            (BlendMode::Custom(BlendFactor::One, BlendFactor::One), 2),
            (BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::One), 2),
            (BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha), 3),
            (BlendMode::Custom(BlendFactor::One, BlendFactor::OneMinusSrcAlpha), 3),
            (BlendMode::Custom(BlendFactor::DstColor, BlendFactor::SrcColor), 4),
            (BlendMode::Custom(BlendFactor::OneMinusDstColor, BlendFactor::One), 4),
        ];
        for (blend, expected_bucket) in samples {
            let key = stage(DrawClass::Transparent, blend);
            assert_eq!(world_blend_probe_bucket(key), expected_bucket);
            for mode in 3..=6_u8 {
                assert_eq!(world_stage_probe_excludes(mode, key, false), u32::from(mode - 2) == expected_bucket);
            }
            assert!(world_stage_probe_excludes(1, key, false));
        }
        let opaque = stage(DrawClass::Opaque, BlendMode::Opaque);
        let mask = stage(DrawClass::Mask, BlendMode::Opaque);
        for mode in 3..=6 {
            assert!(!world_stage_probe_excludes(mode, opaque, true));
            assert!(!world_stage_probe_excludes(mode, mask, true));
        }
        assert!(world_stage_probe_excludes(2, mask, true));
        assert!(!world_stage_probe_excludes(0, opaque, false));
        let additive = stage(DrawClass::Transparent,
            BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::One));
        assert!(world_stage_probe_excludes(7, additive, true));
        assert!(!world_stage_probe_excludes(7, additive, false));
        assert!(world_stage_probe_excludes(8, additive, false));
        assert!(!world_stage_probe_excludes(8, additive, true));
        assert!(!world_stage_probe_excludes(7, opaque, true));
        assert!(!world_stage_probe_excludes(8, opaque, false));
        assert!(!world_stage_probe_excludes(7, mask, true));
    }
}

#[cfg(test)]
mod fast_sky_tests {
    use super::fast_sky_clear_color;

    #[test]
    fn fast_sky_palette_and_red_special_case() {
        let cases = [
            (1, [0.60, 0.76, 0.90]),
            (2, [0.0, 1.0, 0.0]),
            (3, [1.0, 1.0, 0.0]),
            (4, [0.0, 0.0, 1.0]),
            (5, [0.0, 1.0, 1.0]),
            (6, [1.0, 0.0, 1.0]),
            (7, [1.0, 1.0, 1.0]),
            (8, [1.0, 0.5, 0.0]),
            (9, [0.5, 0.5, 0.5]),
            (10, [1.0, 0.0, 0.0]),
        ];
        for (mode, expected) in cases {
            let color = fast_sky_clear_color(mode);
            assert_eq!([color.r, color.g, color.b], expected, "mode {mode}");
        }
    }
}
