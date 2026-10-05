//! World detail textures.
use crate::renderer::{BTreeSet, DrawBatch, GpuImage, HashMap};

pub(in crate::renderer) const AUTO_DETAIL_ENABLED_BIT: u8 = 0x80;

pub(in crate::renderer) const AUTO_DETAIL_INDEX_MASK: u8 = 0x7f;

pub(in crate::renderer) const AUTO_DETAIL_TEXTURE_FILES: &[&str] = &[
    "dt_brick.tga",
    "dt_carpet1.tga",
    "dt_conc.tga",
    "dt_fabric1.tga",
    "dt_fabric2.tga",
    "dt_grass1.tga",
    "dt_ground1.tga",
    "dt_ground2.tga",
    "dt_ground3.tga",
    "dt_ground4.tga",
    "dt_ground5.tga",
    "dt_leather1.tga",
    "dt_metal1.tga",
    "dt_metal2.tga",
    "dt_plaster1.tga",
    "dt_plaster2.tga",
    "dt_rock1.tga",
    "dt_rough1.tga",
    "dt_smooth1.tga",
    "dt_smooth2.tga",
    "dt_snow.tga",
    "dt_snow1.tga",
    "dt_snow2.tga",
    "dt_ssteel1.tga",
    "dt_stone1.tga",
    "dt_stone2.tga",
    "dt_stone3.tga",
    "dt_stone4.tga",
    "dt_wood.tga",
    "dt_wood1.tga",
    "dt_wood2.tga",
    "dt_wood3.tga",
];

pub(in crate::renderer) fn auto_detail_candidates(material: u8) -> &'static [&'static str] {
    match material & 31 {
        1 | 2 => &[
            "dt_wood1.tga",
            "dt_wood2.tga",
            "dt_wood3.tga",
            "dt_wood.tga",
        ],
        3 | 4 => &["dt_metal1.tga", "dt_metal2.tga", "dt_ssteel1.tga"],
        5 | 6 => &["dt_grass1.tga"],
        7 => &[
            "dt_ground1.tga",
            "dt_ground3.tga",
            "dt_ground4.tga",
            "dt_ground5.tga",
        ],
        8 => &["dt_ground2.tga", "dt_ground1.tga"],
        9 => &["dt_ground3.tga", "dt_rough1.tga"],
        10 | 18 | 29 => &["dt_smooth1.tga", "dt_smooth2.tga"],
        11 => &["dt_conc.tga", "dt_brick.tga", "dt_rough1.tga"],
        12 => &["dt_smooth1.tga", "dt_stone1.tga", "dt_stone2.tga"],
        13 | 15 => &["dt_smooth1.tga", "dt_smooth2.tga"],
        14 => &["dt_snow.tga", "dt_snow1.tga", "dt_snow2.tga"],
        16 => &["dt_leather1.tga", "dt_fabric1.tga"],
        17 => &["dt_ground4.tga", "dt_ground5.tga", "dt_ground1.tga"],
        19 => &["dt_ground5.tga", "dt_ground3.tga"],
        20 => &["dt_grass1.tga", "dt_ground5.tga"],
        21 => &["dt_fabric1.tga", "dt_fabric2.tga"],
        22 => &["dt_fabric2.tga", "dt_fabric1.tga"],
        23 => &[
            "dt_rock1.tga",
            "dt_stone1.tga",
            "dt_stone2.tga",
            "dt_stone3.tga",
            "dt_stone4.tga",
            "dt_rough1.tga",
        ],
        24 => &["dt_smooth2.tga", "dt_rough1.tga"],
        25 => &["dt_smooth1.tga", "dt_smooth2.tga"],
        26 => &["dt_stone1.tga", "dt_stone2.tga", "dt_smooth1.tga"],
        27 => &["dt_carpet1.tga", "dt_fabric1.tga"],
        28 => &["dt_plaster1.tga", "dt_plaster2.tga"],
        30 => &["dt_ssteel1.tga", "dt_metal2.tga", "dt_metal1.tga"],
        31 => &["dt_metal1.tga", "dt_smooth1.tga", "dt_ssteel1.tga"],
        _ => &["dt_rough1.tga", "dt_smooth1.tga", "dt_conc.tga"],
    }
}

pub(in crate::renderer) fn auto_detail_texture_index(name: &str) -> Option<u8> {
    AUTO_DETAIL_TEXTURE_FILES
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(name))
        .and_then(|index| u8::try_from(index).ok())
}

pub(in crate::renderer) fn auto_detail_fallback_index() -> u8 {
    auto_detail_texture_index("dt_rough1.tga").unwrap_or(0)
}

pub(in crate::renderer) fn auto_detail_index_for_material(material: u8) -> u8 {
    auto_detail_candidates(material)
        .first()
        .and_then(|candidate| auto_detail_texture_index(candidate))
        .unwrap_or_else(auto_detail_fallback_index)
}

/// Resolve AUTO detail selection once per already-bound base texture. The BSP
/// compactor already requires identical `source.texture` to merge draws, so
/// making detail choice a pure function of that existing state cannot split a
/// packable group. If one base image is authored with conflicting MATERIAL_*
/// classes, use the generic fallback rather than smuggling new material state
/// into the draw key. No shader/texture filename heuristics are used.
pub(in crate::renderer) fn build_auto_detail_by_base_texture<'a>(
    texture_count: usize,
    sources: impl Iterator<Item = &'a DrawBatch>,
) -> Vec<u8> {
    let fallback = auto_detail_fallback_index();
    let mut selected = vec![None::<u8>; texture_count];
    let mut conflicted = vec![false; texture_count];
    let mut seen = vec![false; texture_count];
    let mut all_eligible = vec![true; texture_count];

    for source in sources {
        let Some(texture) = source.texture else {
            continue;
        };
        let Some(seen_slot) = seen.get_mut(texture) else {
            continue;
        };
        *seen_slot = true;
        if let Some(eligible) = all_eligible.get_mut(texture) {
            *eligible &= source.detail_texture_eligible;
        }
        if !source.detail_texture_eligible {
            continue;
        }

        let Some(slot) = selected.get_mut(texture) else {
            continue;
        };
        let desired = auto_detail_index_for_material(source.surface_material);
        match *slot {
            None => *slot = Some(desired),
            Some(existing) if existing == desired => {}
            Some(_) => {
                *slot = Some(fallback);
                if let Some(flag) = conflicted.get_mut(texture) {
                    *flag = true;
                }
            }
        }
    }

    selected
        .into_iter()
        .enumerate()
        .map(|(texture, choice)| {
            let index = if conflicted.get(texture).copied().unwrap_or(false) {
                fallback
            } else {
                choice.unwrap_or(fallback)
            } & AUTO_DETAIL_INDEX_MASK;
            let eligible = seen.get(texture).copied().unwrap_or(false)
                && all_eligible.get(texture).copied().unwrap_or(false);
            index | if eligible { AUTO_DETAIL_ENABLED_BIT } else { 0 }
        })
        .collect()
}

pub(in crate::renderer) fn required_auto_detail_indices(plan: &[u8]) -> BTreeSet<u8> {
    plan.iter()
        .copied()
        .filter(|word| (word & AUTO_DETAIL_ENABLED_BIT) != 0)
        .map(|word| word & AUTO_DETAIL_INDEX_MASK)
        .collect()
}

pub(in crate::renderer) fn detail_texture_for_source<'a>(
    source: &DrawBatch,
    manual: &'a GpuImage,
    auto_enabled: bool,
    auto_textures: &'a HashMap<String, GpuImage>,
    auto_by_base_texture: &[u8],
) -> &'a GpuImage {
    if !auto_enabled {
        return manual;
    }
    let fallback = auto_detail_fallback_index();
    let selected = source
        .texture
        .and_then(|texture| auto_by_base_texture.get(texture).copied())
        .map(|word| word & AUTO_DETAIL_INDEX_MASK)
        .unwrap_or(fallback);
    let name = AUTO_DETAIL_TEXTURE_FILES
        .get(usize::from(selected))
        .copied()
        .unwrap_or("dt_rough1.tga");
    auto_textures
        .get(name)
        .or_else(|| auto_textures.get("dt_rough1.tga"))
        .unwrap_or(manual)
}
