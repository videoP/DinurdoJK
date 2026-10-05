//! Lightweight secondary-motion deformation for stock `_humanoid.gla` player meshes.
//!
//! Stock JKA's 53-bone humanoid has no dedicated breast/soft-tissue bones, so this
//! intentionally works after Ghoul2 skinning. Explicit per-model `model.jiggle` files
//! remain supported as overrides, but ordinary `_humanoid` models can be classified
//! automatically from Raven's helper surfaces plus the mesh normals and real GLM skin
//! weights. Masks and spring anchors are compiled once at model registration; runtime
//! work is then only a handful of damped points plus masked offsets.

use jka_assets::ghoul2::{
    transform_point, GlaAnimation, Ghoul2SkinnedSurface, GlmModel, GlmSurface, GlmVertex,
    Matrix3x4,
};
use std::collections::HashMap;

const RESET_DT: f32 = 0.100;
const TELEPORT_DISTANCE: f32 = 96.0;
pub(crate) const MAX_GPU_JIGGLE_REGIONS: usize = 4;
const NO_GPU_JIGGLE_REGION: u32 = u32::MAX;

/// Live user tuning layered on top of the per-model/auto profile. The solver
/// keeps KawaiiPhysics-style damping/stiffness in the 0..1 domain; these are
/// multipliers so model.jiggle remains the authored baseline.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JiggleTuning {
    pub overall_strength: f32,
    pub breast_strength: f32,
    pub glute_strength: f32,
    pub stiffness_scale: f32,
    pub damping_scale: f32,
    /// Normalized vertical shift of the lower glute falloff. Positive moves the
    /// effective region upward without rebuilding GLM/GPU buffers.
    pub glute_lift: f32,
}

impl Default for JiggleTuning {
    fn default() -> Self {
        Self {
            overall_strength: 1.0,
            breast_strength: 1.0,
            glute_strength: 1.0,
            stiffness_scale: 1.0,
            damping_scale: 1.0,
            glute_lift: 0.15,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionSide {
    Both,
    NegativeX,
    PositiveX,
}

#[derive(Debug, Clone, Copy)]
struct AnchorTerm {
    bone_index: usize,
    weight: f32,
    point: [f32; 3],
}

#[derive(Debug, Clone)]
struct SemanticMask {
    /// Global `_humanoid.gla` bone indices that are allowed to contribute to the mask.
    allowed_bones: Vec<usize>,
    /// -1 = model-forward (-Y), +1 = model-rear (+Y).
    facing_y: f32,
}

#[derive(Debug, Clone)]
struct RegionSpec {
    name: String,
    surfaces: Vec<String>,
    anchor_override: Option<usize>,
    anchor_terms: Vec<AnchorTerm>,
    center: [f32; 3],
    radius: [f32; 3],
    response: [f32; 3],
    stiffness: f32,
    damping: f32,
    max_offset: f32,
    strength: f32,
    side: RegionSide,
    /// Present only for automatically classified `_humanoid` regions. Explicit
    /// `model.jiggle` files retain the exact ellipsoid semantics from version 1.
    semantic: Option<SemanticMask>,
}

#[derive(Debug, Clone)]
struct SurfaceMask {
    /// One smooth weight array per region. Arrays are the exact GLM vertex count.
    region_weights: Vec<Vec<f32>>,
    /// GPU skinning stores the strongest region per vertex. Auto regions are
    /// deliberately non-overlapping; explicit overlapping masks still keep their
    /// exact multi-region behavior on the CPU skinning path.
    dominant_region: Vec<u32>,
    dominant_weight: Vec<f32>,
    /// Bind-pose vertical coordinate normalized by the dominant region's
    /// center/radius. GPU shaders use this for live glute-height attenuation.
    dominant_coord: Vec<f32>,
}

#[derive(Debug, Clone)]
pub(crate) struct JiggleProfile {
    regions: Vec<RegionSpec>,
    /// LOD -> surface index -> compiled region masks.
    lod_masks: Vec<HashMap<usize, SurfaceMask>>,
    /// The compact GPU vertex payload stores one region per vertex. Profiles
    /// with overlapping regions retain exact behavior through the CPU fallback.
    gpu_compatible: bool,
}

#[derive(Debug, Clone)]
struct PendingRegion {
    name: String,
    surfaces: Vec<String>,
    anchor_name: Option<String>,
    center: Option<[f32; 3]>,
    radius: Option<[f32; 3]>,
    response: [f32; 3],
    stiffness: f32,
    damping: f32,
    max_offset: f32,
    strength: f32,
    side: RegionSide,
}

impl PendingRegion {
    fn new(name: String) -> Self {
        Self {
            name,
            surfaces: Vec::new(),
            anchor_name: None,
            center: None,
            radius: None,
            response: [1.0, 1.0, 1.0],
            stiffness: 90.0,
            damping: 13.0,
            max_offset: 1.0,
            strength: 1.0,
            side: RegionSide::Both,
        }
    }
}

impl JiggleProfile {
    pub(crate) fn parse(text: &str, glm: &GlmModel, gla: &GlaAnimation) -> Result<Self, String> {
        let mut version = None::<u32>;
        let mut pending = None::<PendingRegion>;
        let mut regions = Vec::<RegionSpec>::new();

        for (line_index, raw_line) in text.lines().enumerate() {
            let line_no = line_index + 1;
            let line = raw_line
                .split_once("//")
                .map_or(raw_line, |(head, _)| head);
            let line = line.split_once('#').map_or(line, |(head, _)| head).trim();
            if line.is_empty() {
                continue;
            }
            let tokens = line.split_whitespace().collect::<Vec<_>>();
            let directive = tokens[0].to_ascii_lowercase();

            if pending.is_none() {
                match directive.as_str() {
                    "version" => {
                        if version.is_some() {
                            return Err(format!("line {line_no}: duplicate jiggle version"));
                        }
                        if tokens.len() != 2 {
                            return Err(format!("line {line_no}: expected `version 1`"));
                        }
                        let parsed = tokens[1]
                            .parse::<u32>()
                            .map_err(|_| format!("line {line_no}: invalid jiggle version"))?;
                        if parsed != 1 {
                            return Err(format!("line {line_no}: unsupported jiggle version {parsed}"));
                        }
                        version = Some(parsed);
                    }
                    "region" => {
                        if version != Some(1) {
                            return Err(format!(
                                "line {line_no}: `version 1` must appear before any region"
                            ));
                        }
                        if tokens.len() != 2 {
                            return Err(format!("line {line_no}: expected `region <name>`"));
                        }
                        let name = tokens[1];
                        if regions
                            .iter()
                            .any(|region| region.name.eq_ignore_ascii_case(name))
                        {
                            return Err(format!("line {line_no}: duplicate region `{name}`"));
                        }
                        pending = Some(PendingRegion::new(name.to_owned()));
                    }
                    other => {
                        return Err(format!("line {line_no}: unexpected directive `{other}`"));
                    }
                }
                continue;
            }

            if directive == "end" {
                if tokens.len() != 1 {
                    return Err(format!("line {line_no}: `end` takes no arguments"));
                }
                let completed = pending.take().expect("region exists");
                regions.push(Self::finish_region(completed, gla, line_no)?);
                continue;
            }

            let region = pending.as_mut().expect("checked above");
            match directive.as_str() {
                "surface" => {
                    if tokens.len() != 2 {
                        return Err(format!("line {line_no}: expected `surface <name>`"));
                    }
                    region.surfaces.push(tokens[1].to_ascii_lowercase());
                }
                "anchor" => {
                    if tokens.len() != 2 {
                        return Err(format!("line {line_no}: expected `anchor auto|<bone>`"));
                    }
                    region.anchor_name = if tokens[1].eq_ignore_ascii_case("auto") {
                        None
                    } else {
                        Some(tokens[1].to_owned())
                    };
                }
                "center" => region.center = Some(parse_vec3(&tokens, line_no, "center")?),
                "radius" => region.radius = Some(parse_vec3(&tokens, line_no, "radius")?),
                "response" => region.response = parse_vec3(&tokens, line_no, "response")?,
                "stiffness" => region.stiffness = parse_scalar(&tokens, line_no, "stiffness")?,
                "damping" => region.damping = parse_scalar(&tokens, line_no, "damping")?,
                "max_offset" => region.max_offset = parse_scalar(&tokens, line_no, "max_offset")?,
                "strength" => region.strength = parse_scalar(&tokens, line_no, "strength")?,
                "side" => {
                    if tokens.len() != 2 {
                        return Err(format!(
                            "line {line_no}: expected `side both|negative_x|positive_x`"
                        ));
                    }
                    region.side = match tokens[1].to_ascii_lowercase().as_str() {
                        "both" => RegionSide::Both,
                        "negative_x" => RegionSide::NegativeX,
                        "positive_x" => RegionSide::PositiveX,
                        value => {
                            return Err(format!("line {line_no}: invalid side `{value}`"));
                        }
                    };
                }
                other => {
                    return Err(format!(
                        "line {line_no}: unknown region directive `{other}`"
                    ));
                }
            }
        }

        if let Some(region) = pending {
            return Err(format!("region `{}` is missing `end`", region.name));
        }
        if version != Some(1) {
            return Err("jiggle profile must begin with `version 1`".into());
        }
        if regions.is_empty() {
            return Err("jiggle profile contains no regions".into());
        }

        Self::compile(regions, glm, gla)
    }

    /// Build the stock `_humanoid` profile directly from Raven's semantic helper
    /// surfaces. `model.jiggle` remains the authority when present; this is the
    /// zero-authoring fallback for ordinary player GLMs.
    pub(crate) fn auto_humanoid(
        glm: &GlmModel,
        gla: &GlaAnimation,
    ) -> Result<Option<Self>, String> {
        if !glm.anim_name.to_ascii_lowercase().contains("_humanoid") {
            return Ok(None);
        }

        let torso_surfaces = surface_family(glm, "torso");
        let hips_surfaces = surface_family(glm, "hips");
        if torso_surfaces.is_empty() && hips_surfaces.is_empty() {
            return Ok(None);
        }

        let lower_lumbar = find_bone(gla, "lower_lumbar");
        let upper_lumbar = find_bone(gla, "upper_lumbar");
        let thoracic = find_bone(gla, "thoracic");
        let pelvis = find_bone(gla, "pelvis");
        let lclavical = find_bone(gla, "lclavical");
        let rclavical = find_bone(gla, "rclavical");
        let lfemur_yz = find_bone(gla, "lfemurYZ");
        let lfemur_x = find_bone(gla, "lfemurX");
        let rfemur_yz = find_bone(gla, "rfemurYZ");
        let rfemur_x = find_bone(gla, "rfemurX");

        let mut regions = Vec::with_capacity(MAX_GPU_JIGGLE_REGIONS);

        let chest_common = [lower_lumbar, upper_lumbar, thoracic];
        if let (Some(lower_lumbar), Some(upper_lumbar), Some(thoracic)) =
            (chest_common[0], chest_common[1], chest_common[2])
        {
            for (name, side, lower_marker, upper_marker, clavicle) in [
                ("breast_left", RegionSide::PositiveX, "*lchest_l", "*uchest_l", lclavical),
                ("breast_right", RegionSide::NegativeX, "*lchest_r", "*uchest_r", rclavical),
            ] {
                let (Some(lower), Some(upper), Some(clavicle)) = (
                    marker_centroid(glm, lower_marker),
                    marker_centroid(glm, upper_marker),
                    clavicle,
                ) else {
                    continue;
                };
                let z_min = lower[2].min(upper[2]) - 1.5;
                let z_max = lower[2].max(upper[2]) + 2.5;
                let semantic = SemanticMask {
                    allowed_bones: vec![lower_lumbar, upper_lumbar, thoracic, clavicle],
                    facing_y: -1.0,
                };
                if let Some((mut center, mut radius)) = fit_semantic_region(
                    glm,
                    &torso_surfaces,
                    side,
                    &semantic,
                    z_min,
                    z_max,
                    [2.8, 3.8],
                    [4.8, 5.5],
                ) {
                    // The chest helper triangles are exceptionally stable across
                    // Raven/community `_humanoid` GLMs; use them for lateral
                    // placement while the rendered vertices determine height.
                    center[0] = (lower[0] + upper[0]) * 0.5;
                    radius[0] = (center[0].abs() * 1.18).clamp(3.2, 4.0);
                    regions.push(RegionSpec {
                        name: name.into(),
                        surfaces: torso_surfaces.clone(),
                        anchor_override: None,
                        anchor_terms: Vec::new(),
                        center,
                        radius,
                        response: [0.28, 1.00, 0.65],
                        // KawaiiPhysics-style normalized settings.
                        stiffness: 0.10,
                        damping: 0.18,
                        max_offset: 1.15,
                        strength: 1.0,
                        side,
                        semantic: Some(semantic),
                    });
                }
            }
        }

        if let Some(pelvis) = pelvis {
            for (name, side, back_marker, femur_yz, femur_x) in [
                ("glute_left", RegionSide::PositiveX, "*hip_bl", lfemur_yz, lfemur_x),
                ("glute_right", RegionSide::NegativeX, "*hip_br", rfemur_yz, rfemur_x),
            ] {
                let (Some(back), Some(femur_yz), Some(femur_x)) =
                    (marker_centroid(glm, back_marker), femur_yz, femur_x)
                else {
                    continue;
                };
                let mut allowed_bones = vec![pelvis, femur_yz, femur_x];
                if let Some(lower_lumbar) = lower_lumbar {
                    allowed_bones.push(lower_lumbar);
                }
                let semantic = SemanticMask { allowed_bones, facing_y: 1.0 };
                if let Some((mut center, mut radius)) = fit_semantic_region(
                    glm,
                    &hips_surfaces,
                    side,
                    &semantic,
                    // Keep the automatic glute footprint above the upper thigh.
                    // The live glute-height control can further trim/lift it.
                    back[2] - 10.5,
                    back[2] + 2.5,
                    [3.4, 4.8],
                    [6.0, 7.5],
                ) {
                    center[0] = back[0] * 0.85;
                    center[2] += 1.25;
                    radius[0] = (back[0].abs() * 1.35).clamp(4.0, 5.5);
                    radius[2] = (radius[2] * 0.82).clamp(4.2, 6.0);
                    regions.push(RegionSpec {
                        name: name.into(),
                        surfaces: hips_surfaces.clone(),
                        anchor_override: None,
                        anchor_terms: Vec::new(),
                        center,
                        radius,
                        response: [0.20, 0.85, 0.85],
                        stiffness: 0.12,
                        damping: 0.20,
                        max_offset: 0.95,
                        strength: 1.0,
                        side,
                        semantic: Some(semantic),
                    });
                }
            }
        }

        if regions.is_empty() {
            return Ok(None);
        }
        Self::compile(regions, glm, gla).map(Some)
    }

    fn compile(
        mut regions: Vec<RegionSpec>,
        glm: &GlmModel,
        gla: &GlaAnimation,
    ) -> Result<Self, String> {
        let mut lod_masks = Vec::with_capacity(glm.lods.len());
        let mut gpu_compatible = regions.len() <= MAX_GPU_JIGGLE_REGIONS;
        let mut region_selected = vec![false; regions.len()];
        for lod in &glm.lods {
            let mut masks = HashMap::<usize, SurfaceMask>::new();
            for surface in &lod.surfaces {
                let Some(hierarchy) = glm.hierarchy.get(surface.surface_index) else {
                    continue;
                };
                let surface_name = hierarchy.name.to_ascii_lowercase();
                let mut region_weights = Vec::with_capacity(regions.len());
                let mut any_surface_weight = false;

                for (region_index, region) in regions.iter().enumerate() {
                    let enabled = region.surfaces.iter().any(|name| name == &surface_name);
                    let mut weights = vec![0.0f32; surface.vertices.len()];
                    if enabled {
                        for (index, vertex) in surface.vertices.iter().enumerate() {
                            let weight = region_vertex_weight(region, surface, vertex);
                            weights[index] = weight;
                            if weight > 0.0001 {
                                any_surface_weight = true;
                                region_selected[region_index] = true;
                            }
                        }
                    }
                    region_weights.push(weights);
                }

                if any_surface_weight {
                    let mut dominant_region = vec![NO_GPU_JIGGLE_REGION; surface.vertices.len()];
                    let mut dominant_weight = vec![0.0f32; surface.vertices.len()];
                    let mut dominant_coord = vec![0.0f32; surface.vertices.len()];
                    for vertex_index in 0..surface.vertices.len() {
                        let mut contributors = 0usize;
                        for (region_index, weights) in region_weights.iter().enumerate() {
                            let weight = weights[vertex_index];
                            if weight > 0.0001 {
                                contributors += 1;
                            }
                            if weight > dominant_weight[vertex_index] {
                                dominant_weight[vertex_index] = weight;
                                dominant_region[vertex_index] = region_index as u32;
                                let region = &regions[region_index];
                                dominant_coord[vertex_index] = if region
                                    .name
                                    .to_ascii_lowercase()
                                    .contains("glute")
                                {
                                    (surface.vertices[vertex_index].position[2] - region.center[2])
                                        / region.radius[2].max(0.0001)
                                } else {
                                    // Chest/non-glute sentinel.  Values in [-2, 2]
                                    // are reserved for the normalized glute Z coord.
                                    4.0
                                };
                            }
                        }
                        if contributors > 1 {
                            gpu_compatible = false;
                        }
                    }
                    masks.insert(
                        surface.surface_index,
                        SurfaceMask {
                            region_weights,
                            dominant_region,
                            dominant_weight,
                            dominant_coord,
                        },
                    );
                }
            }
            lod_masks.push(masks);
        }

        if lod_masks.iter().all(|masks| masks.is_empty()) {
            return Err("jiggle profile did not select any GLM vertices".into());
        }
        if let Some((index, _)) = region_selected
            .iter()
            .enumerate()
            .find(|(_, selected)| !**selected)
        {
            return Err(format!(
                "region `{}` did not select any GLM vertices",
                regions[index].name
            ));
        }

        for region in &mut regions {
            region.anchor_terms = compile_region_anchor(glm, gla, region)?;
        }

        Ok(Self { regions, lod_masks, gpu_compatible })
    }

    fn finish_region(
        region: PendingRegion,
        gla: &GlaAnimation,
        line_no: usize,
    ) -> Result<RegionSpec, String> {
        let PendingRegion {
            name,
            surfaces,
            anchor_name,
            center,
            radius,
            response,
            stiffness,
            damping,
            max_offset,
            strength,
            side,
        } = region;

        if surfaces.is_empty() {
            return Err(format!(
                "line {line_no}: region `{name}` has no `surface` entries"
            ));
        }
        let anchor_override = anchor_name
            .as_deref()
            .map(|anchor_name| {
                gla.skeleton
                    .iter()
                    .position(|bone| bone.name.eq_ignore_ascii_case(anchor_name))
                    .ok_or_else(|| {
                        format!(
                            "line {line_no}: region `{name}` references missing bone `{anchor_name}`"
                        )
                    })
            })
            .transpose()?;
        let center = center
            .ok_or_else(|| format!("line {line_no}: region `{name}` is missing `center`"))?;
        let radius = radius
            .ok_or_else(|| format!("line {line_no}: region `{name}` is missing `radius`"))?;

        if radius.iter().any(|value| !value.is_finite() || *value <= 0.0) {
            return Err(format!(
                "line {line_no}: region `{name}` radius components must be > 0"
            ));
        }
        if response.iter().any(|value| !value.is_finite() || *value < 0.0)
            || !stiffness.is_finite()
            || stiffness <= 0.0
            || !damping.is_finite()
            || damping < 0.0
            || !max_offset.is_finite()
            || max_offset <= 0.0
            || !strength.is_finite()
            || strength < 0.0
        {
            return Err(format!(
                "line {line_no}: region `{name}` contains invalid simulation parameters"
            ));
        }

        // Version-1 profiles created by the first Dinurdo jiggle prototype
        // used spring constants around 75/10.5. Preserve those files while
        // moving runtime semantics to KawaiiPhysics' normalized 0..1 controls.
        let stiffness = if stiffness > 1.0 { (stiffness / 750.0).clamp(0.0, 1.0) } else { stiffness };
        let damping = if damping > 1.0 { (damping / 60.0).clamp(0.0, 1.0) } else { damping };

        Ok(RegionSpec {
            name,
            surfaces,
            anchor_override,
            anchor_terms: Vec::new(),
            center,
            radius,
            response,
            stiffness,
            damping,
            max_offset,
            strength,
            side,
            semantic: None,
        })
    }

    pub(crate) fn affects_surface(&self, lod_index: usize, surface_index: usize) -> bool {
        self.lod_masks
            .get(lod_index)
            .is_some_and(|lod| lod.contains_key(&surface_index))
    }

    /// GPU skinning keeps one dominant region per vertex so the existing
    /// 80-byte Ghoul2 vertex can reuse its padding without growing every player
    /// vertex. Four regions cover the stock breasts/glutes profile.
    pub(crate) fn gpu_vertex_binding(
        &self,
        lod_index: usize,
        surface_index: usize,
        vertex_index: usize,
    ) -> (u32, f32, f32) {
        if !self.gpu_compatible {
            return (NO_GPU_JIGGLE_REGION, 0.0, 0.0);
        }
        let Some(mask) = self
            .lod_masks
            .get(lod_index)
            .and_then(|lod| lod.get(&surface_index))
        else {
            return (NO_GPU_JIGGLE_REGION, 0.0, 0.0);
        };
        let region = mask
            .dominant_region
            .get(vertex_index)
            .copied()
            .unwrap_or(NO_GPU_JIGGLE_REGION);
        let weight = mask.dominant_weight.get(vertex_index).copied().unwrap_or(0.0);
        let coord = mask.dominant_coord.get(vertex_index).copied().unwrap_or(0.0);
        (region, weight, coord)
    }

    pub(crate) fn gpu_supported(&self) -> bool {
        self.gpu_compatible
    }

    pub(crate) fn gpu_offsets(
        &self,
        offsets: &[[f32; 3]],
        tuning: JiggleTuning,
    ) -> [[f32; 4]; MAX_GPU_JIGGLE_REGIONS] {
        let mut packed = [[0.0f32; 4]; MAX_GPU_JIGGLE_REGIONS];
        for (index, offset) in offsets.iter().take(MAX_GPU_JIGGLE_REGIONS).enumerate() {
            packed[index] = [offset[0], offset[1], offset[2], 0.0];
        }
        // The .w lanes are otherwise unused. Pack live tuning there without
        // growing SkinDraw or the per-draw storage buffer.
        packed[0][3] = tuning.overall_strength.clamp(0.0, 2.0);
        packed[1][3] = tuning.breast_strength.clamp(0.0, 2.0);
        packed[2][3] = tuning.glute_strength.clamp(0.0, 2.0);
        packed[3][3] = tuning.glute_lift.clamp(-0.40, 0.60);
        packed
    }

    fn region_strength(&self, region_index: usize, tuning: JiggleTuning) -> f32 {
        let regional = self.regions.get(region_index).map_or(1.0, |region| {
            if region.name.to_ascii_lowercase().contains("glute") {
                tuning.glute_strength
            } else {
                tuning.breast_strength
            }
        });
        (regional * tuning.overall_strength).clamp(0.0, 4.0)
    }

    fn glute_vertical_factor(&self, region_index: usize, coord: f32, tuning: JiggleTuning) -> f32 {
        let Some(region) = self.regions.get(region_index) else { return 1.0 };
        if !region.name.to_ascii_lowercase().contains("glute") {
            return 1.0;
        }
        // Smoothly suppress the lower half of the ellipsoid. Raising this
        // threshold is the live equivalent of moving the glute mask upward,
        // and avoids dragging femur/thigh vertices.
        smoothstep(-0.70 + tuning.glute_lift, -0.15 + tuning.glute_lift, coord)
    }

    pub(crate) fn deform_surface(
        &self,
        lod_index: usize,
        surface_index: usize,
        region_offsets: &[[f32; 3]],
        tuning: JiggleTuning,
        skinned: &mut Ghoul2SkinnedSurface,
    ) {
        let Some(mask) = self
            .lod_masks
            .get(lod_index)
            .and_then(|lod| lod.get(&surface_index))
        else {
            return;
        };
        if region_offsets.len() != self.regions.len()
            || mask.region_weights.len() != self.regions.len()
        {
            return;
        }

        for (vertex_index, vertex) in skinned.vertices.iter_mut().enumerate() {
            let mut offset = [0.0f32; 3];
            for (region_index, weights) in mask.region_weights.iter().enumerate() {
                let Some(&weight) = weights.get(vertex_index) else {
                    continue;
                };
                if weight <= 0.0 {
                    continue;
                }
                let region_offset = region_offsets[region_index];
                let coord = mask
                    .dominant_region
                    .get(vertex_index)
                    .zip(mask.dominant_coord.get(vertex_index))
                    .and_then(|(&dominant, &coord)| (dominant as usize == region_index).then_some(coord))
                    .unwrap_or(0.0);
                let effective_weight = weight
                    * self.region_strength(region_index, tuning)
                    * self.glute_vertical_factor(region_index, coord, tuning);
                offset[0] += region_offset[0] * effective_weight;
                offset[1] += region_offset[1] * effective_weight;
                offset[2] += region_offset[2] * effective_weight;
            }
            vertex.position[0] += offset[0];
            vertex.position[1] += offset[1];
            vertex.position[2] += offset[2];
        }
        // Deliberately retain Ghoul2's authored/fast-path normal here.  The initial
        // deformation amplitudes are small, and recomputing triangle normals would
        // destroy hard/smoothed normal choices authored into the GLM.
    }

    pub(crate) fn region_count(&self) -> usize {
        self.regions.len()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RegionState {
    initialized: bool,
    position_world: [f32; 3],
    prev_position_world: [f32; 3],
    last_anchor_world: [f32; 3],
}

#[derive(Debug, Clone, Default)]
struct EntityState {
    last_time_ms: i32,
    substep_accumulator: f32,
    regions: Vec<RegionState>,
}

pub(crate) struct JiggleSystem {
    enabled: bool,
    hz: u32,
    max_substeps: usize,
    tuning: JiggleTuning,
    entities: HashMap<(u16, String), EntityState>,
}

impl Default for JiggleSystem {
    fn default() -> Self {
        Self {
            enabled: false,
            hz: 60,
            max_substeps: 4,
            tuning: JiggleTuning::default(),
            entities: HashMap::new(),
        }
    }
}

impl JiggleSystem {
    pub(crate) fn set_config(
        &mut self,
        enabled: bool,
        hz: u32,
        max_substeps: u32,
        tuning: JiggleTuning,
    ) {
        if self.enabled && !enabled {
            self.entities.clear();
        }
        self.enabled = enabled;
        self.hz = hz.clamp(30, 240);
        self.max_substeps = max_substeps.clamp(1, 8) as usize;
        self.tuning = JiggleTuning {
            overall_strength: tuning.overall_strength.clamp(0.0, 2.0),
            breast_strength: tuning.breast_strength.clamp(0.0, 2.0),
            glute_strength: tuning.glute_strength.clamp(0.0, 2.0),
            stiffness_scale: tuning.stiffness_scale.clamp(0.0, 3.0),
            damping_scale: tuning.damping_scale.clamp(0.0, 3.0),
            glute_lift: tuning.glute_lift.clamp(-0.40, 0.60),
        };
    }

    pub(crate) fn tuning(&self) -> JiggleTuning {
        self.tuning
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn clear(&mut self) {
        self.entities.clear();
    }

    pub(crate) fn retain_entities(&mut self, live: &std::collections::HashSet<u16>) {
        self.entities
            .retain(|(entity_num, _), _| live.contains(entity_num));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn simulate(
        &mut self,
        entity_num: u16,
        model_key: &str,
        profile: &JiggleProfile,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        time_ms: i32,
    ) -> Vec<[f32; 3]> {
        let state = self
            .entities
            .entry((entity_num, model_key.to_owned()))
            .or_default();
        if state.regions.len() != profile.region_count() {
            state.regions = vec![RegionState::default(); profile.region_count()];
            state.substep_accumulator = 0.0;
            state.last_time_ms = time_ms;
        }

        let raw_dt = (time_ms - state.last_time_ms) as f32 * 0.001;
        let time_discontinuity = raw_dt < 0.0 || raw_dt > RESET_DT;
        let dt = raw_dt.max(0.0);
        let fixed_step = 1.0 / self.hz as f32;
        if time_discontinuity {
            state.substep_accumulator = 0.0;
        } else {
            // KawaiiPhysics fixed-substep semantics: accumulate real frame time,
            // clamp catch-up to MaxSubsteps, and discard spiral-of-death excess.
            state.substep_accumulator = (state.substep_accumulator + dt)
                .min(self.max_substeps as f32 * fixed_step);
        }
        let steps = (state.substep_accumulator / fixed_step).floor() as usize;
        state.substep_accumulator -= steps as f32 * fixed_step;
        let mut offsets = vec![[0.0f32; 3]; profile.region_count()];

        for (index, (region, region_state)) in profile
            .regions
            .iter()
            .zip(state.regions.iter_mut())
            .enumerate()
        {
            let mut anchor_model = [0.0f32; 3];
            let mut anchor_valid = true;
            for term in &region.anchor_terms {
                let Some(anchor_matrix) = pose.get(term.bone_index) else {
                    anchor_valid = false;
                    break;
                };
                let point = transform_point(anchor_matrix, term.point);
                anchor_model[0] += point[0] * term.weight;
                anchor_model[1] += point[1] * term.weight;
                anchor_model[2] += point[2] * term.weight;
            }
            if !anchor_valid {
                continue;
            }
            let anchor_world = model_to_jka_world(anchor_model, axis, origin);
            let teleported = region_state.initialized
                && distance(region_state.last_anchor_world, anchor_world) > TELEPORT_DISTANCE;

            if !region_state.initialized || time_discontinuity || teleported {
                region_state.initialized = true;
                region_state.position_world = anchor_world;
                region_state.prev_position_world = anchor_world;
                region_state.last_anchor_world = anchor_world;
                continue;
            }

            if steps > 0 {
                let start_anchor = region_state.last_anchor_world;
                let damping = (region.damping * self.tuning.damping_scale).clamp(0.0, 0.999);
                let stiffness = (region.stiffness * self.tuning.stiffness_scale).clamp(0.0, 0.999);
                // Port the current KawaiiPhysics Verlet core to one virtual
                // soft-region point.  In its fixed-substep path DeltaTimeOld
                // and StepDeltaTime are both FixedDt; damping is deliberately
                // raw per-step, while stiffness keeps the source exponent form.
                let damping_factor = 1.0 - damping;
                let stiffness_pull =
                    1.0 - (1.0 - stiffness).powf(self.hz as f32 * fixed_step);
                for step in 0..steps {
                    let t = (step + 1) as f32 / steps as f32;
                    let target = lerp(start_anchor, anchor_world, t);
                    let previous = region_state.position_world;
                    let velocity = mul(
                        sub(region_state.position_world, region_state.prev_position_world),
                        1.0 / fixed_step,
                    );
                    region_state.prev_position_world = previous;
                    region_state.position_world = add(
                        region_state.position_world,
                        mul(velocity, damping_factor * fixed_step),
                    );
                    region_state.position_world = add(
                        region_state.position_world,
                        mul(sub(target, region_state.position_world), stiffness_pull),
                    );
                }
            }

            let mut world_offset = sub(region_state.position_world, anchor_world);
            let offset_length = length(world_offset);
            if offset_length > region.max_offset {
                let scale = region.max_offset / offset_length;
                world_offset = mul(world_offset, scale);
                region_state.position_world = add(anchor_world, world_offset);
                // Kawaii/Verlet state stores velocity implicitly in the previous
                // position. Collapse only the outward component at the clamp so
                // the next step does not immediately re-launch past the limit.
                let implicit_velocity = sub(region_state.position_world, region_state.prev_position_world);
                let radial = dot(implicit_velocity, world_offset)
                    / (region.max_offset * region.max_offset).max(0.0001);
                if radial > 0.0 {
                    let corrected = sub(implicit_velocity, mul(world_offset, radial));
                    region_state.prev_position_world = sub(region_state.position_world, corrected);
                }
            }

            let mut model_offset = world_vector_to_model(world_offset, axis);
            for component in 0..3 {
                model_offset[component] *= region.response[component];
            }
            offsets[index] = model_offset;
            region_state.last_anchor_world = anchor_world;
        }

        state.last_time_ms = time_ms;
        offsets
    }
}

fn region_vertex_weight(region: &RegionSpec, surface: &GlmSurface, vertex: &GlmVertex) -> f32 {
    let position = vertex.position;
    if !region_side_matches(region.side, position[0]) {
        return 0.0;
    }

    if let Some(semantic) = &region.semantic {
        let nx = (position[0] - region.center[0]) / region.radius[0];
        let nz = (position[2] - region.center[2]) / region.radius[2];
        let q = nx * nx + nz * nz;
        if q >= 1.0 {
            return 0.0;
        }
        let facing = (vertex.normal[1] * semantic.facing_y).clamp(0.0, 1.0);
        if facing <= 0.03 {
            return 0.0;
        }
        let bone_weight = semantic_bone_weight(surface, vertex, &semantic.allowed_bones);
        if bone_weight <= 0.10 {
            return 0.0;
        }
        // Keep rounded side vertices participating while strongly rejecting the
        // opposite side of the torso/hips. The geometry falloff still reaches
        // zero with zero derivative at the region boundary.
        let facing_weight = ((facing - 0.03) / 0.55).clamp(0.0, 1.0);
        let base = 1.0 - q;
        return (base * base
            * (0.30 + 0.70 * facing_weight)
            * bone_weight.clamp(0.0, 1.0)
            * region.strength)
            .clamp(0.0, 1.0);
    }

    let mut q = 0.0f32;
    for axis in 0..3 {
        let normalized = (position[axis] - region.center[axis]) / region.radius[axis];
        q += normalized * normalized;
    }
    if q >= 1.0 {
        return 0.0;
    }
    // Explicit v1 profiles retain the original smooth ellipsoid behavior.
    let base = 1.0 - q;
    (base * base * region.strength).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, Default)]
struct AnchorAccum {
    coefficient: f32,
    weighted_point: [f32; 3],
}

/// Build a pose target from the actual LOD0 GLM skin weights in the selected
/// region.  Because Ghoul2 skinning is linear, one weighted source-point term
/// per contributing bone reproduces the weighted centroid of the selected
/// vertices without re-skinning the region just to drive the spring.
fn compile_region_anchor(
    glm: &GlmModel,
    gla: &GlaAnimation,
    region: &RegionSpec,
) -> Result<Vec<AnchorTerm>, String> {
    if let Some(bone_index) = region.anchor_override {
        return Ok(vec![AnchorTerm {
            bone_index,
            weight: 1.0,
            point: region.center,
        }]);
    }

    let lod = glm
        .lods
        .first()
        .ok_or_else(|| format!("region `{}` cannot build an anchor: GLM has no LOD0", region.name))?;
    let mut accum = HashMap::<usize, AnchorAccum>::new();

    for surface in &lod.surfaces {
        let Some(hierarchy) = glm.hierarchy.get(surface.surface_index) else {
            continue;
        };
        if !region
            .surfaces
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&hierarchy.name))
        {
            continue;
        }

        for vertex in &surface.vertices {
            let mask_weight = region_vertex_weight(region, surface, vertex);
            if mask_weight <= 0.0001 {
                continue;
            }
            for skin_weight in &vertex.weights {
                let Some(&bone_index) = surface.bone_references.get(skin_weight.local_bone_index)
                else {
                    return Err(format!(
                        "region `{}` references an invalid local GLM bone index {}",
                        region.name, skin_weight.local_bone_index
                    ));
                };
                if bone_index >= gla.skeleton.len() {
                    return Err(format!(
                        "region `{}` references GLM bone {} but GLA has only {} bones",
                        region.name,
                        bone_index,
                        gla.skeleton.len()
                    ));
                }
                let coefficient = mask_weight * skin_weight.weight;
                if coefficient <= 0.0 {
                    continue;
                }
                let entry = accum.entry(bone_index).or_default();
                entry.coefficient += coefficient;
                for axis in 0..3 {
                    entry.weighted_point[axis] += vertex.position[axis] * coefficient;
                }
            }
        }
    }

    let total = accum.values().map(|entry| entry.coefficient).sum::<f32>();
    if !total.is_finite() || total <= 0.0001 {
        return Err(format!(
            "region `{}` could not derive an automatic skin-weight anchor",
            region.name
        ));
    }

    let mut terms = accum
        .into_iter()
        .filter_map(|(bone_index, entry)| {
            if entry.coefficient <= 0.0001 {
                return None;
            }
            let inv = 1.0 / entry.coefficient;
            Some(AnchorTerm {
                bone_index,
                weight: entry.coefficient / total,
                point: [
                    entry.weighted_point[0] * inv,
                    entry.weighted_point[1] * inv,
                    entry.weighted_point[2] * inv,
                ],
            })
        })
        .collect::<Vec<_>>();
    terms.sort_by_key(|term| term.bone_index);
    Ok(terms)
}

fn find_bone(gla: &GlaAnimation, name: &str) -> Option<usize> {
    gla.skeleton
        .iter()
        .position(|bone| bone.name.eq_ignore_ascii_case(name))
}

fn surface_family(glm: &GlmModel, prefix: &str) -> Vec<String> {
    let prefix = prefix.to_ascii_lowercase();
    glm.hierarchy
        .iter()
        .filter_map(|surface| {
            let name = surface.name.to_ascii_lowercase();
            (!name.starts_with('*') && name.starts_with(&prefix)).then_some(name)
        })
        .collect()
}

fn marker_centroid(glm: &GlmModel, name: &str) -> Option<[f32; 3]> {
    let surface_index = glm
        .hierarchy
        .iter()
        .position(|surface| surface.name.eq_ignore_ascii_case(name))?;
    let surface = glm
        .lods
        .first()?
        .surfaces
        .iter()
        .find(|surface| surface.surface_index == surface_index)?;
    if surface.vertices.is_empty() {
        return None;
    }
    let mut center = [0.0f32; 3];
    for vertex in &surface.vertices {
        for axis in 0..3 {
            center[axis] += vertex.position[axis];
        }
    }
    let inv = 1.0 / surface.vertices.len() as f32;
    for value in &mut center {
        *value *= inv;
    }
    Some(center)
}

fn semantic_bone_weight(surface: &GlmSurface, vertex: &GlmVertex, allowed: &[usize]) -> f32 {
    vertex
        .weights
        .iter()
        .filter_map(|weight| {
            let bone = *surface.bone_references.get(weight.local_bone_index)?;
            allowed.contains(&bone).then_some(weight.weight.max(0.0))
        })
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

/// Fit a stable X/Z soft-tissue footprint from the actual LOD0 body vertices.
/// The helper surfaces only establish anatomical meaning and a vertical window;
/// facing normals and skin weights decide which rendered vertices participate.
fn fit_semantic_region(
    glm: &GlmModel,
    surfaces: &[String],
    side: RegionSide,
    semantic: &SemanticMask,
    z_min: f32,
    z_max: f32,
    min_radius: [f32; 2],
    max_radius: [f32; 2],
) -> Option<([f32; 3], [f32; 3])> {
    let lod = glm.lods.first()?;
    let mut total = 0.0f32;
    let mut mean = [0.0f32; 3];
    let mut samples = Vec::<([f32; 3], f32)>::new();

    for surface in &lod.surfaces {
        let name = glm.hierarchy.get(surface.surface_index)?.name.to_ascii_lowercase();
        if !surfaces.iter().any(|candidate| candidate == &name) {
            continue;
        }
        for vertex in &surface.vertices {
            if !region_side_matches(side, vertex.position[0])
                || vertex.position[2] < z_min
                || vertex.position[2] > z_max
            {
                continue;
            }
            let facing = (vertex.normal[1] * semantic.facing_y).clamp(0.0, 1.0);
            if facing <= 0.03 {
                continue;
            }
            let bone_weight = semantic_bone_weight(surface, vertex, &semantic.allowed_bones);
            if bone_weight <= 0.10 {
                continue;
            }
            let score = bone_weight * (0.25 + 0.75 * facing);
            total += score;
            for axis in 0..3 {
                mean[axis] += vertex.position[axis] * score;
            }
            samples.push((vertex.position, score));
        }
    }

    if samples.len() < 6 || total <= 0.0001 {
        return None;
    }
    for value in &mut mean {
        *value /= total;
    }

    let mut variance_x = 0.0f32;
    let mut variance_z = 0.0f32;
    for (position, score) in samples {
        variance_x += (position[0] - mean[0]).powi(2) * score;
        variance_z += (position[2] - mean[2]).powi(2) * score;
    }
    variance_x /= total;
    variance_z /= total;
    let radius_x = (variance_x.sqrt() * 2.75).clamp(min_radius[0], max_radius[0]);
    let radius_z = (variance_z.sqrt() * 2.75).clamp(min_radius[1], max_radius[1]);
    Some((mean, [radius_x, 1.0, radius_z]))
}

fn parse_vec3(tokens: &[&str], line_no: usize, name: &str) -> Result<[f32; 3], String> {
    if tokens.len() != 4 {
        return Err(format!("line {line_no}: expected `{name} x y z`"));
    }
    Ok([
        parse_number(tokens[1], line_no, name)?,
        parse_number(tokens[2], line_no, name)?,
        parse_number(tokens[3], line_no, name)?,
    ])
}

fn parse_scalar(tokens: &[&str], line_no: usize, name: &str) -> Result<f32, String> {
    if tokens.len() != 2 {
        return Err(format!("line {line_no}: expected `{name} value`"));
    }
    parse_number(tokens[1], line_no, name)
}

fn parse_number(value: &str, line_no: usize, name: &str) -> Result<f32, String> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| format!("line {line_no}: invalid number for `{name}`"))?;
    if !parsed.is_finite() {
        return Err(format!("line {line_no}: non-finite number for `{name}`"));
    }
    Ok(parsed)
}

fn region_side_matches(side: RegionSide, x: f32) -> bool {
    match side {
        RegionSide::Both => true,
        RegionSide::NegativeX => x < 0.0,
        RegionSide::PositiveX => x >= 0.0,
    }
}

fn model_to_jka_world(point: [f32; 3], axis: [[f32; 3]; 3], origin: [f32; 3]) -> [f32; 3] {
    [
        origin[0] + axis[0][0] * point[0] + axis[1][0] * point[1] + axis[2][0] * point[2],
        origin[1] + axis[0][1] * point[0] + axis[1][1] * point[1] + axis[2][1] * point[2],
        origin[2] + axis[0][2] * point[0] + axis[1][2] * point[1] + axis[2][2] * point[2],
    ]
}

fn world_vector_to_model(vector: [f32; 3], axis: [[f32; 3]; 3]) -> [f32; 3] {
    [dot(vector, axis[0]), dot(vector, axis[1]), dot(vector, axis[2])]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn mul(value: [f32; 3], scalar: f32) -> [f32; 3] {
    [value[0] * scalar, value[1] * scalar, value[2] * scalar]
}

fn length(value: [f32; 3]) -> f32 {
    dot(value, value).sqrt()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    length(sub(a, b))
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let width = (edge1 - edge0).max(0.0001);
    let t = ((value - edge0) / width).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipsoid_side_split_is_stable() {
        assert!(region_side_matches(RegionSide::NegativeX, -0.1));
        assert!(!region_side_matches(RegionSide::NegativeX, 0.1));
        assert!(region_side_matches(RegionSide::PositiveX, 0.1));
        assert!(!region_side_matches(RegionSide::PositiveX, -0.1));
        assert!(region_side_matches(RegionSide::Both, -100.0));
    }

    #[test]
    fn world_model_round_trip_for_identity_axis() {
        let axis = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let point = [1.0, 2.0, 3.0];
        assert_eq!(model_to_jka_world(point, axis, [10.0, 20.0, 30.0]), [11.0, 22.0, 33.0]);
        assert_eq!(world_vector_to_model(point, axis), point);
    }
}
