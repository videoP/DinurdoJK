//! Authored `q3map_dinurdoPromoted <system> [<shader>|none]` overrides.
//!
//! A mapper marks a world shader whose look an advanced promotion replaces
//! (for example seabed caustics under promoted water). While that system is
//! active, the shader is redirected to an authored variant or hidden. The
//! result is folded into `ShaderRemapState`'s promotion layer, so it uses the
//! existing cold remap rebuild and the hot path stays ordinary draw batches.
//! Stock renderers and q3map2 skip any `q3map*` line.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::{scene, App};

/// `r_shaderPromotion`: master switch for authored promotion overrides.
static ENABLED: AtomicBool = AtomicBool::new(true);

pub(in crate::app) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The next `sync_shader_promotion` sees the changed activity and rebuilds.
pub(in crate::app) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

const OCEAN: u8 = 1 << 0;
const RIVER: u8 = 1 << 1;
const FIRE: u8 = 1 << 2;
const SMOKE: u8 = 1 << 3;

/// Known system names, most specific first: when several active systems name
/// the same shader, the first match here wins.
const SYSTEMS: [(&str, u8); 5] = [
    ("ocean", OCEAN),
    ("river", RIVER),
    ("water", OCEAN | RIVER),
    ("fire", FIRE),
    ("smoke", SMOKE),
];

fn system_bits(system: &str) -> Option<u8> {
    SYSTEMS.iter().find(|(name, _)| *name == system).map(|&(_, bits)| bits)
}

fn system_rank(system: &str) -> usize {
    SYSTEMS.iter().position(|(name, _)| *name == system).unwrap_or(usize::MAX)
}

/// Directives and promotion facts of the loaded map, captured once at map
/// load so the per-frame check only compares a few booleans.
#[derive(Default)]
pub(in crate::app) struct ShaderPromotionState {
    /// Canonical BSP source shader -> (system, canonical target) pairs,
    /// sorted by system priority.
    directives: Vec<(String, Vec<(String, Option<String>)>)>,
    /// The map was prepared with ocean promotion and has a promotable surface.
    ocean_prepared: bool,
    /// River promotion replaces authored water on this map (mode 3 with plans).
    river_prepared: bool,
    /// Activity bits currently folded into `App::shader_remaps`.
    applied: Option<u8>,
}

impl ShaderPromotionState {
    pub(in crate::app) fn from_map(map: &scene::PreparedMap, ocean_prepared: bool) -> Self {
        let mut directives = Vec::new();
        let mut unknown = Vec::new();
        for entry in &map.material_debug.entries {
            let Some(shader) = jka_assets::shader::find_shader(&map.shader_library, &entry.name) else {
                continue;
            };
            if shader.promotion_overrides.is_empty() {
                continue;
            }
            let source = scene::canonical_shader_name(&entry.name);
            if directives.iter().any(|(name, _)| *name == source) {
                continue;
            }
            let mut overrides: Vec<(String, Option<String>)> = Vec::new();
            for directive in &shader.promotion_overrides {
                if system_bits(&directive.system).is_none() {
                    unknown.push(format!("{source}: {}", directive.system));
                    continue;
                }
                let target = directive.target.as_deref().map(scene::canonical_shader_name)
                    .filter(|target| *target != source);
                overrides.push((directive.system.clone(), target));
            }
            overrides.sort_by_key(|(system, _)| system_rank(system));
            if !overrides.is_empty() {
                directives.push((source, overrides));
            }
        }
        if !directives.is_empty() {
            rverbose!(1, "[SHADER PROMOTION] {} world shader(s) carry q3map_dinurdoPromoted", directives.len());
        }
        for line in unknown {
            eprintln!("[SHADER PROMOTION] unknown system ignored ({line}); expected ocean, river, water, fire or smoke");
        }
        Self {
            directives,
            ocean_prepared,
            river_prepared: map.river_promotion_mode == 3
                && !map.river_plans.is_empty()
                && map.sub_bsp_boundary.is_none(),
            applied: None,
        }
    }

    /// A runtime remap upload clears promoted rivers; the next sync re-resolves.
    pub(in crate::app) fn forget_rivers(&mut self) {
        self.river_prepared = false;
    }

    /// Source -> replacement (None hides) for the given active systems.
    fn resolve(&self, active: u8) -> BTreeMap<String, Option<String>> {
        self.directives.iter().filter_map(|(source, overrides)| {
            overrides.iter()
                .find(|(system, _)| system_bits(system).is_some_and(|bits| bits & active != 0))
                .map(|(_, target)| (source.clone(), target.clone()))
        }).collect()
    }
}

impl App {
    fn shader_promotion_active_bits(&self) -> u8 {
        let state = &self.shader_promotion;
        let mut bits = 0;
        if !enabled() {
            return bits;
        }
        if state.ocean_prepared && self.video.ocean {
            bits |= OCEAN;
        }
        if state.river_prepared {
            bits |= RIVER;
        }
        if jka_ninja::INCLUDED && crate::ninja_fire_debug::promote() {
            bits |= FIRE;
        }
        if jka_ninja::INCLUDED && crate::ninja_fire_debug::promote_smoke() {
            bits |= SMOKE;
        }
        bits
    }

    /// Map load: capture this map's directives and set the promotion layer
    /// before the first upload, so no second world upload is needed.
    /// `rivers_survive` is false when an R_RemapShader in the same upload
    /// drops the map's river plans.
    pub(in crate::app) fn install_shader_promotion(
        &mut self, map: &scene::PreparedMap, ocean_prepared: bool, rivers_survive: bool,
    ) {
        self.shader_promotion = ShaderPromotionState::from_map(map, ocean_prepared);
        if !rivers_survive {
            self.shader_promotion.forget_rivers();
        }
        let bits = self.shader_promotion_active_bits();
        self.shader_promotion.applied = Some(bits);
        let promotion = self.shader_promotion.resolve(bits);
        self.shader_remaps.set_promotion(promotion);
    }

    /// A new map registration drops the previous map's directives.
    pub(in crate::app) fn reset_shader_promotion(&mut self) {
        self.shader_promotion = ShaderPromotionState::default();
        self.shader_remaps.set_promotion(BTreeMap::new());
    }

    /// Per frame: a promotion toggle changed which overrides apply. Only a
    /// few booleans are compared unless the map carries a directive and the
    /// activity changed; then the existing cold remap rebuild runs.
    pub(in crate::app) fn sync_shader_promotion(&mut self) {
        if self.shader_promotion.directives.is_empty() || self.loading.is_some() {
            return;
        }
        let bits = self.shader_promotion_active_bits();
        if self.shader_promotion.applied == Some(bits) {
            return;
        }
        self.shader_promotion.applied = Some(bits);
        let promotion = self.shader_promotion.resolve(bits);
        if !self.shader_remaps.set_promotion(promotion) {
            return;
        }
        rverbose!(1, "[SHADER PROMOTION] active systems changed; rebuilding world materials");
        if let Err(error) = self.rebuild_shader_remapped_world() {
            self.push_console_line(format!("^1shader promotion: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(directives: &[(&str, &[(&str, Option<&str>)])]) -> ShaderPromotionState {
        ShaderPromotionState {
            directives: directives.iter().map(|(source, overrides)| {
                let mut overrides: Vec<_> = overrides.iter()
                    .map(|(system, target)| (system.to_string(), target.map(str::to_owned)))
                    .collect();
                overrides.sort_by_key(|(system, _)| system_rank(system));
                (source.to_string(), overrides)
            }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn water_covers_ocean_and_river_and_specific_system_wins() {
        let state = state(&[
            ("textures/a/sand", &[("water", None), ("ocean", Some("textures/a/sand_ocean"))][..]),
            ("textures/a/torch", &[("fire", None)][..]),
        ]);
        let ocean = state.resolve(OCEAN);
        assert_eq!(ocean.get("textures/a/sand"), Some(&Some("textures/a/sand_ocean".into())));
        assert!(!ocean.contains_key("textures/a/torch"));
        let river = state.resolve(RIVER | FIRE);
        assert_eq!(river.get("textures/a/sand"), Some(&None));
        assert_eq!(river.get("textures/a/torch"), Some(&None));
        assert!(state.resolve(SMOKE).is_empty());
    }
}
