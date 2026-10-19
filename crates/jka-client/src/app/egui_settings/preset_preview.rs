//! Read-only rendering preset comparisons for the settings UI.
//!
//! Only cvars with a corresponding visible settings row are mapped. Use the
//! preset's existing cvar table and the actual current getter, never an
//! approximation of the renderer state or a transient call to a cvar setter.
use crate::app::egui_settings::{theme, ui, App, QualityPreset, VideoSection, OCEAN_SEA_SPRAY, OCEAN_WIND_FOAM};
use crate::app::quality::cvar_values_match;

/// Single source of truth for which menu row presents a preset-owned cvar.
/// Several cvars intentionally share a row (e.g. FXAA/SMAA/TAA/MSAA).
fn row_for_preset_cvar(name: &str) -> Option<theme::Reset> {
    use theme::Reset;
    Some(match name {
        name if name.starts_with("r_glass") => Reset::Glass(crate::console::find(name)?.name),
        "r_maxFrameLatency" => Reset::Video(ui::VIDEO_ROW_MAX_FRAME_LATENCY),
        "r_ext_multisample" | "r_fxaa" | "r_smaa" | "r_taa" =>
            Reset::Video(ui::VIDEO_ROW_ANTI_ALIASING),
        "r_texturemode" | "r_ext_texture_filter_anisotropic" =>
            Reset::Video(ui::VIDEO_ROW_TEXTURE_FILTER),
        "r_detailTextures" => Reset::Video(ui::VIDEO_ROW_DETAIL_TEXTURES),
        "r_hdr" => Reset::Video(ui::VIDEO_ROW_HDR),
        "r_floatLightmap" => Reset::Video(ui::VIDEO_ROW_FLOAT_LIGHTMAP),
        "r_tonemap" => Reset::Video(ui::VIDEO_ROW_TONE_MAPPING),
        "r_autoExposure" => Reset::Video(ui::VIDEO_ROW_AUTO_EXPOSURE),
        "r_colorLut" => Reset::Video(ui::VIDEO_ROW_COLOR_LUT),
        "r_colorLutStrength" => Reset::Video(ui::VIDEO_ROW_LUT_STRENGTH),
        "r_glow" => Reset::Video(ui::VIDEO_ROW_GLOW),
        "r_halation" => Reset::Video(ui::VIDEO_ROW_HALATION),
        "r_ssao" | "r_staticBspAo" => Reset::Video(ui::VIDEO_ROW_AMBIENT_OCCLUSION),
        "r_staticBspAoSamples" => Reset::Video(ui::VIDEO_ROW_BAKED_AO_SAMPLES),
        "r_staticBspAoResolution" => Reset::Video(ui::VIDEO_ROW_BAKED_AO_RESOLUTION),
        "r_contactShadows" => Reset::Video(ui::VIDEO_ROW_CONTACT_SHADOWS),
        "r_distanceCullScale" => Reset::Video(ui::VIDEO_ROW_DISTANCE_CULL),
        "r_entityAmbientLighting" => Reset::Video(ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING),
        "r_dynamicLights" => Reset::Video(ui::VIDEO_ROW_DYNAMIC_LIGHTS),
        "r_promoteFxLights" => Reset::Video(ui::VIDEO_ROW_PROMOTE_FX_LIGHTS),
        "r_emissiveAreaLights" => Reset::Video(ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS),
        "r_voxelProbeGI" => Reset::Video(ui::VIDEO_ROW_VOXEL_PROBE_GI),
        "r_localLightShadows" => Reset::Video(ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS),
        "r_dynamicShadows" => Reset::Video(ui::VIDEO_ROW_DYNAMIC_SHADOWS),
        "r_pbr" => Reset::Video(ui::VIDEO_ROW_PBR),
        "r_genNormalMaps" => Reset::Video(ui::VIDEO_ROW_GEN_NORMAL_MAPS),
        "r_deluxeMapping" => Reset::Video(ui::VIDEO_ROW_DELUXE_MAPPING),
        "r_deluxeSpecular" => Reset::Video(ui::VIDEO_ROW_DELUXE_SPECULAR),
        "r_subdivisions" => Reset::Video(ui::VIDEO_ROW_PATCH_SUBDIVISIONS),
        "r_lodCurveError" => Reset::Video(ui::VIDEO_ROW_PATCH_LOD_ERROR),
        "r_patchStitching" => Reset::Video(ui::VIDEO_ROW_PATCH_STITCHING),
        "r_lodScale" => Reset::Video(ui::VIDEO_ROW_MODEL_LOD_SCALE),
        "r_lodbias" => Reset::Video(ui::VIDEO_ROW_GHOUL2_LOD_BIAS),
        "r_reflectionQuality" => Reset::Video(ui::VIDEO_ROW_SSR),
        "r_motionBlur" => Reset::Video(ui::VIDEO_ROW_MOTION_BLUR),
        "r_dofMode" => Reset::Video(ui::VIDEO_ROW_DOF_MODE),
        "r_depthOfField" => Reset::Video(ui::VIDEO_ROW_DEPTH_OF_FIELD),
        "r_dofQuality" => Reset::Video(ui::VIDEO_ROW_DOF_QUALITY),
        "r_chromaticAberration" => Reset::Video(ui::VIDEO_ROW_CHROMATIC_ABERRATION),
        "r_vignette" => Reset::Video(ui::VIDEO_ROW_VIGNETTE),
        "r_filmGrain" => Reset::Video(ui::VIDEO_ROW_FILM_GRAIN),
        "fx_physics" => Reset::Video(ui::VIDEO_ROW_FX_PHYSICS),
        "r_drawfog" => Reset::Environment(ui::ENV_ROW_FOG_MODE),
        "r_sunVisibility" => Reset::Environment(ui::ENV_ROW_SUN_VISIBILITY),
        "r_clouds" => Reset::Environment(ui::ENV_ROW_CLOUDS),
        "r_cloudQuality" => Reset::Environment(ui::ENV_ROW_CLOUD_QUALITY),
        "r_cloudRenderResolution" => Reset::Environment(ui::ENV_ROW_CLOUD_RENDER_RESOLUTION),
        "r_cloudShadows" => Reset::Environment(ui::ENV_ROW_CLOUD_SHADOWS),
        "r_cloudTemporal" => Reset::Environment(ui::ENV_ROW_CLOUD_TEMPORAL),
        "r_cloudTemporalDepthFix" => Reset::CloudTuning(ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX),
        "r_grass" => Reset::Environment(ui::ENV_ROW_GRASS),
        "r_footprints" => Reset::Environment(ui::ENV_ROW_FOOTPRINTS),
        "r_ocean" => Reset::Environment(ui::ENV_ROW_OCEAN),
        "r_oceanMapSize" => Reset::Ocean(0),
        "r_oceanMeshQuality" => Reset::Ocean(1),
        "r_oceanUpdates" => Reset::Ocean(2),
        "r_oceanSeaSpray" => Reset::Ocean(7),
        "r_oceanWindFoam" => Reset::Ocean(8),
        // Some preset controls intentionally have no corresponding video row.
        // Do not attach their values to an unrelated or misleading UI control.
        _ => return None,
    })
}

/// The section that actually owns a preset cvar's visible setting.
/// Cvars without an editable row (notably r_worldPath) are deliberately not
/// eligible for a page-only application, but remain in Apply To All.
pub(in crate::app) fn preset_cvar_section(name: &str) -> Option<VideoSection> {
    use VideoSection as S;
    Some(match name {
        name if name.starts_with("r_glass") && row_for_preset_cvar(name).is_some() => S::Glass,
        "r_maxFrameLatency" | "r_ext_multisample" | "r_fxaa" | "r_smaa" | "r_taa"
        | "r_texturemode" | "r_ext_texture_filter_anisotropic" | "r_detailTextures" => S::Display,
        "r_distanceCullScale" | "r_subdivisions" | "r_lodCurveError" | "r_patchStitching"
        | "r_lodScale" | "r_lodbias" => S::Geometry,
        "r_ssao" | "r_staticBspAo" | "r_staticBspAoSamples" | "r_staticBspAoResolution"
        | "r_entityAmbientLighting" | "r_dynamicLights" | "r_promoteFxLights"
        | "r_emissiveAreaLights" | "r_voxelProbeGI" | "r_pbr" | "r_genNormalMaps"
        | "r_deluxeMapping" | "r_deluxeSpecular" => S::Lighting,
        "r_contactShadows" | "r_localLightShadows" | "r_dynamicShadows" => S::Shadows,
        "fx_physics" | "r_reflectionQuality" => S::Effects,
        "r_hdr" | "r_floatLightmap" | "r_tonemap" | "r_autoExposure"
        | "r_colorLut" | "r_colorLutStrength" => S::Color,
        "r_glow" | "r_halation" | "r_motionBlur" | "r_dofMode" | "r_depthOfField"
        | "r_dofQuality" | "r_chromaticAberration" | "r_vignette" | "r_filmGrain" => S::PostProcessing,
        "r_sunVisibility" => S::Sun,
        "r_clouds" | "r_cloudShadows" => S::Clouds,
        "r_cloudQuality" | "r_cloudRenderResolution" | "r_cloudTemporal"
        | "r_cloudTemporalDepthFix" => S::Clouds,
        "r_drawfog" => S::Weather,
        "r_grass" | "r_footprints" => S::Surface,
        "r_ocean" => S::Water,
        "r_oceanMapSize" | "r_oceanMeshQuality" | "r_oceanUpdates"
        | "r_oceanSeaSpray" | "r_oceanWindFoam" => S::Water,
        _ => return None,
    })
}

fn preset_value(preset: QualityPreset, name: &str) -> Option<&'static str> {
    preset.settings().into_iter().find_map(|(cvar, value)| (cvar == name).then_some(value))
}

fn preview_target_label_for_row(preset: QualityPreset, reset: theme::Reset) -> Option<String> {
    use theme::Reset;
    Some(match reset {
        Reset::Video(ui::VIDEO_ROW_ANTI_ALIASING) => {
            if preset_value(preset, "r_taa") == Some("1") {
                "TAA".to_owned()
            } else if preset_value(preset, "r_smaa") == Some("1") {
                "SMAA".to_owned()
            } else if preset_value(preset, "r_fxaa") == Some("1") {
                "FXAA".to_owned()
            } else if let Some(samples) = preset_value(preset, "r_ext_multisample") {
                match samples.parse::<u32>().ok().filter(|samples| *samples > 1) {
                    Some(samples) => format!("{samples}× MSAA"),
                    None => "Off".to_owned(),
                }
            } else {
                "Off".to_owned()
            }
        }
        Reset::Video(ui::VIDEO_ROW_TEXTURE_FILTER) => match preset_value(preset, "r_ext_texture_filter_anisotropic") {
            Some("2") => "2× AF".to_owned(),
            Some("4") => "4× AF".to_owned(),
            Some("8") => "8× AF".to_owned(),
            Some("16") => "16× AF".to_owned(),
            _ => match preset_value(preset, "r_texturemode") {
                Some("GL_NEAREST") => "Nearest".to_owned(),
                Some("GL_LINEAR_MIPMAP_NEAREST") => "Bilinear".to_owned(),
                Some("GL_LINEAR_MIPMAP_LINEAR") => "Trilinear".to_owned(),
                _ => return None,
            },
        },
        Reset::Video(ui::VIDEO_ROW_DETAIL_TEXTURES)
        | Reset::Video(ui::VIDEO_ROW_FLOAT_LIGHTMAP)
        | Reset::Video(ui::VIDEO_ROW_TONE_MAPPING)
        | Reset::Video(ui::VIDEO_ROW_AUTO_EXPOSURE)
        | Reset::Video(ui::VIDEO_ROW_CONTACT_SHADOWS)
        | Reset::Video(ui::VIDEO_ROW_PROMOTE_FX_LIGHTS)
        | Reset::Video(ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS)
        | Reset::Video(ui::VIDEO_ROW_VOXEL_PROBE_GI)
        | Reset::Video(ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS)
        | Reset::Video(ui::VIDEO_ROW_PBR)
        | Reset::Video(ui::VIDEO_ROW_GEN_NORMAL_MAPS)
        | Reset::Video(ui::VIDEO_ROW_DELUXE_MAPPING)
        | Reset::Video(ui::VIDEO_ROW_PATCH_STITCHING)
        | Reset::Environment(ui::ENV_ROW_CLOUDS)
        | Reset::Environment(ui::ENV_ROW_CLOUD_SHADOWS)
        | Reset::Environment(ui::ENV_ROW_CLOUD_TEMPORAL)
        | Reset::CloudTuning(ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX)
        | Reset::Environment(ui::ENV_ROW_GRASS)
        | Reset::Environment(ui::ENV_ROW_OCEAN)
        | Reset::Ocean(OCEAN_SEA_SPRAY)
        | Reset::Ocean(OCEAN_WIND_FOAM) => {
            let cvar = match reset {
                Reset::Video(ui::VIDEO_ROW_DETAIL_TEXTURES) => "r_detailTextures",
                Reset::Video(ui::VIDEO_ROW_FLOAT_LIGHTMAP) => "r_floatLightmap",
                Reset::Video(ui::VIDEO_ROW_TONE_MAPPING) => "r_tonemap",
                Reset::Video(ui::VIDEO_ROW_AUTO_EXPOSURE) => "r_autoExposure",
                Reset::Video(ui::VIDEO_ROW_CONTACT_SHADOWS) => "r_contactShadows",
                Reset::Video(ui::VIDEO_ROW_PROMOTE_FX_LIGHTS) => "r_promoteFxLights",
                Reset::Video(ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS) => "r_emissiveAreaLights",
                Reset::Video(ui::VIDEO_ROW_VOXEL_PROBE_GI) => "r_voxelProbeGI",
                Reset::Video(ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS) => "r_localLightShadows",
                Reset::Video(ui::VIDEO_ROW_PBR) => "r_pbr",
                Reset::Video(ui::VIDEO_ROW_GEN_NORMAL_MAPS) => "r_genNormalMaps",
                Reset::Video(ui::VIDEO_ROW_DELUXE_MAPPING) => "r_deluxeMapping",
                Reset::Video(ui::VIDEO_ROW_PATCH_STITCHING) => "r_patchStitching",
                Reset::Environment(ui::ENV_ROW_CLOUDS) => "r_clouds",
                Reset::Environment(ui::ENV_ROW_CLOUD_SHADOWS) => "r_cloudShadows",
                Reset::Environment(ui::ENV_ROW_CLOUD_TEMPORAL) => "r_cloudTemporal",
                Reset::CloudTuning(ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX) => "r_cloudTemporalDepthFix",
                Reset::Environment(ui::ENV_ROW_GRASS) => "r_grass",
                Reset::Environment(ui::ENV_ROW_OCEAN) => "r_ocean",
                Reset::Ocean(OCEAN_SEA_SPRAY) => "r_oceanSeaSpray",
                Reset::Ocean(OCEAN_WIND_FOAM) => "r_oceanWindFoam",
                _ => unreachable!(),
            };
            if preset_value(preset, cvar) == Some("1") || preset_value(preset, cvar) == Some("on") {
                "On".to_owned()
            } else {
                "Off".to_owned()
            }
        }
        Reset::Video(ui::VIDEO_ROW_HDR) => {
            if preset_value(preset, "r_hdr") == Some("1") {
                "HDR Math Only".to_owned()
            } else if preset_value(preset, "r_hdr") == Some("0") {
                "Off".to_owned()
            } else {
                "Full HDR Output".to_owned()
            }
        }
        Reset::Video(ui::VIDEO_ROW_GLOW) => match preset_value(preset, "r_glow") {
            Some("off") => "Off".to_owned(),
            Some("authored") => "JKA Authored".to_owned(),
            Some("combined") => "JKA + Bloom".to_owned(),
            Some("1") => "Modern Bloom".to_owned(),
            _ => return None,
        },
        Reset::Video(ui::VIDEO_ROW_AMBIENT_OCCLUSION) => {
            if preset_value(preset, "r_ssao") == Some("1") {
                "Realtime".to_owned()
            } else if preset_value(preset, "r_staticBspAo") == Some("1") {
                "Baked".to_owned()
            } else {
                "Off".to_owned()
            }
        }
        Reset::Video(ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING) => match preset_value(preset, "r_entityAmbientLighting") {
            Some("off") => "Off".to_owned(),
            Some("bsp_lightgrid") => "BSP lightgrid".to_owned(),
            Some("bevy_irradiance_volume") => "Irradiance volume".to_owned(),
            _ => return None,
        },
        Reset::Video(ui::VIDEO_ROW_DYNAMIC_LIGHTS) => match preset_value(preset, "r_dynamicLights") {
            Some("off") => "Off".to_owned(),
            Some("vertex") => "Vertex".to_owned(),
            Some("legacy") => "Legacy".to_owned(),
            Some("clustered_lite") => "Clustered lite".to_owned(),
            Some("forward_plus") => "Forward+".to_owned(),
            Some("ray_traced") => "Ray traced".to_owned(),
            _ => return None,
        },
        Reset::Video(ui::VIDEO_ROW_PATCH_SUBDIVISIONS) => match preset_value(preset, "r_subdivisions") {
            Some("12") => "Minimal".to_owned(),
            Some("8") => "Low".to_owned(),
            Some("4") => "Medium".to_owned(),
            Some("2") => "High".to_owned(),
            Some("1") => "Ultra".to_owned(),
            _ => return None,
        },
        Reset::Video(ui::VIDEO_ROW_PATCH_LOD_ERROR) => match preset_value(preset, "r_lodCurveError") {
            Some("125") => "Short".to_owned(),
            Some("250") => "Normal".to_owned(),
            Some("1000") => "Far".to_owned(),
            Some("2500") => "Very far".to_owned(),
            _ => return None,
        },
        Reset::Video(ui::VIDEO_ROW_DYNAMIC_SHADOWS) => match preset_value(preset, "r_dynamicShadows") {
            Some("off") => "Off".to_owned(),
            Some("blob") => "Blob".to_owned(),
            Some("entity_map") => "Entity map".to_owned(),
            Some("csm") => "Cascaded maps (CSM)".to_owned(),
            Some("ray_traced") => "Ray traced".to_owned(),
            _ => return None,
        },
        Reset::Video(ui::VIDEO_ROW_SSR) => match preset_value(preset, "r_reflectionQuality") {
            Some("off") => "Off".to_owned(),
            Some("legacy") => "Legacy".to_owned(),
            Some("low") => "Low".to_owned(),
            Some("medium") => "Medium".to_owned(),
            Some("high") => "High".to_owned(),
            Some("ultra") => "Ultra".to_owned(),
            _ => return None,
        },
        Reset::Environment(ui::ENV_ROW_FOG_MODE) => match preset_value(preset, "r_drawfog") {
            Some("0") => "Off".to_owned(),
            Some("1") => "Legacy 1".to_owned(),
            Some("2") => "Legacy 2".to_owned(),
            Some("3") => "Volumetric".to_owned(),
            _ => return None,
        },
        Reset::Environment(ui::ENV_ROW_SUN_VISIBILITY) => match preset_value(preset, "r_sunVisibility") {
            Some("legacy") => "Legacy".to_owned(),
            Some("sky") | Some("sky_portals") => "Sky portals".to_owned(),
            Some("filtered") => "Filtered / High".to_owned(),
            _ => return None,
        },
        Reset::Environment(ui::ENV_ROW_CLOUD_RENDER_RESOLUTION) => match preset_value(preset, "r_cloudRenderResolution") {
            Some("quarter") => "25%".to_owned(),
            Some("half") => "50%".to_owned(),
            Some("three_quarter") => "75%".to_owned(),
            Some("full") => "100%".to_owned(),
            _ => return None,
        },
        Reset::Environment(ui::ENV_ROW_FOOTPRINTS) => match preset_value(preset, "r_footprints") {
            Some("off") => "Off".to_owned(),
            Some("2d") => "2D stamp".to_owned(),
            Some("3d") => "3D + 2D stamp".to_owned(),
            _ => return None,
        },
        _ => return None,
    })
}

impl App {
    pub(in crate::app::egui_settings) fn preset_row_preview(
        &self,
        preset: QualityPreset,
    ) -> Vec<theme::PresetRowPreview> {
        let mut rows = Vec::new();
        for (cvar, after) in preset.settings() {
            let Some(reset) = row_for_preset_cvar(cvar) else { continue; };
            let Some(before) = self.console_cvar_value(cvar) else { continue; };
            if cvar_values_match(&before, after) || rows.iter().any(|entry: &theme::PresetRowPreview| entry.reset == reset) {
                continue;
            }
            rows.push(theme::PresetRowPreview {
                reset,
                target_label: preview_target_label_for_row(preset, reset),
            });
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_preset_owned_cvar_with_a_known_row_is_consistent() {
        for preset in QualityPreset::ALL {
            for (cvar, _) in preset.settings() {
                if let Some(reset) = row_for_preset_cvar(cvar) {
                    assert_ne!(reset, theme::Reset::None, "{cvar}");
                }
            }
        }
    }

    #[test]
    fn preview_never_uses_an_unrelated_reset_tag() {
        assert_eq!(row_for_preset_cvar("r_clouds"), Some(theme::Reset::Environment(ui::ENV_ROW_CLOUDS)));
        assert_eq!(row_for_preset_cvar("r_reflectionQuality"), Some(theme::Reset::Video(ui::VIDEO_ROW_SSR)));
        assert_eq!(row_for_preset_cvar("r_fxaa"), row_for_preset_cvar("r_taa"));
        assert_eq!(row_for_preset_cvar("r_worldPath"), None);
    }

    #[test]
    fn scoped_mapping_matches_visible_rows() {
        for preset in QualityPreset::ALL {
            for (name, _) in preset.settings() {
                if name == "r_worldPath" {
                    assert_eq!(preset_cvar_section(name), None);
                } else {
                    assert!(preset_cvar_section(name).is_some(), "unmapped preset cvar: {name}");
                    assert!(row_for_preset_cvar(name).is_some(), "missing row: {name}");
                }
            }
        }
        assert_eq!(preset_cvar_section("r_drawfog"), Some(VideoSection::Weather));
        assert_eq!(preset_cvar_section("r_clouds"), Some(VideoSection::Clouds));
        assert_eq!(preset_cvar_section("r_cloudShadows"), Some(VideoSection::Clouds));
        assert_eq!(preset_cvar_section("r_ocean"), Some(VideoSection::Water));
        assert_eq!(preset_cvar_section("r_cloudQuality"), Some(VideoSection::Clouds));
        assert_eq!(preset_cvar_section("r_reflectionQuality"), Some(VideoSection::Effects));
    }

    #[test]
    fn row_target_labels_match_widget_labels() {
        assert_eq!(preview_target_label_for_row(QualityPreset::Ultra, theme::Reset::Video(ui::VIDEO_ROW_ANTI_ALIASING)).as_deref(), Some("TAA"));
        assert_eq!(preview_target_label_for_row(QualityPreset::Low, theme::Reset::Video(ui::VIDEO_ROW_TEXTURE_FILTER)).as_deref(), Some("16× AF"));
        assert_eq!(preview_target_label_for_row(QualityPreset::Minimal, theme::Reset::Environment(ui::ENV_ROW_FOG_MODE)).as_deref(), Some("Legacy 2"));
    }
}
