//! Direct ports of TerrainFeatureType.java and TerrainFeature.java at 4a595e7.
//! Material is generic; the host supplies the mapped material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Void,
    Surface,
    Ground,
}
impl Family {
    pub fn density_cap(self) -> i32 {
        match self {
            Self::Void => 16,
            Self::Surface => 20,
            Self::Ground => 12,
        }
    }
    pub fn base_gain(self) -> f32 {
        match self {
            Self::Void => 0.08,
            Self::Surface => 0.06,
            Self::Ground => 0.03,
        }
    }
    pub fn priority_weight(self) -> f32 {
        match self {
            Self::Void => 1.5,
            Self::Surface => 1.0,
            Self::Ground => 0.7,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Edge,
    Drop,
    Passage,
    Wall,
    Ceiling,
    SolidObject,
    Step,
}
impl FeatureKind {
    pub fn family(self) -> Family {
        match self {
            Self::Edge | Self::Drop | Self::Passage => Family::Void,
            Self::Wall | Self::Ceiling | Self::SolidObject => Family::Surface,
            Self::Step => Family::Ground,
        }
    }
    pub fn max_range(self) -> f32 {
        if self == Self::Step {
            12.0
        } else {
            24.0
        }
    }
    pub fn priority(self) -> i32 {
        match self {
            Self::Edge | Self::Drop => 5,
            Self::Passage => 4,
            Self::Wall | Self::SolidObject => 3,
            Self::Ceiling | Self::Step => 2,
        }
    }
    pub fn family_weight(self) -> f32 {
        self.family().priority_weight()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct TerrainFeature<M> {
    pub kind: FeatureKind,
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub magnitude: f32,
    pub saliency: f32,
    pub material: M,
}
impl<M> TerrainFeature<M> {
    pub fn family(&self) -> Family {
        self.kind.family()
    }
    pub fn distance_to(&self, p: [f32; 3]) -> f32 {
        let [x, y, z]: [f32; 3] = std::array::from_fn(|i| self.position[i] - p[i]);
        f64::from(x * x + y * y + z * z).sqrt() as f32
    }
}
pub fn compute_saliency(
    magnitude: f32,
    max_magnitude: f32,
    distance: f32,
    max_range: f32,
    normal: [f32; 3],
    direction_to_player: [f32; 3],
    novelty_boost: f32,
) -> f32 {
    if max_magnitude <= 0.0 || max_range <= 0.0 {
        return 0.0;
    }
    let magnitude_score =
        ((f64::from(magnitude) + 1.0).ln() / (f64::from(max_magnitude) + 1.0).ln()) as f32;
    let proximity = (1.0 - distance / max_range).max(0.0);
    let dot = normal[0] * direction_to_player[0]
        + normal[1] * direction_to_player[1]
        + normal[2] * direction_to_player[2];
    magnitude_score * proximity * (0.6 + 0.4 * dot.max(0.0)) * novelty_boost
}
/// Upstream uses Y as the vertical axis. Convert from JKA Z-up first.
pub fn compute_detection_saliency(
    magnitude: f32,
    max_magnitude: f32,
    feature_y: f32,
    player_y: f32,
    scan_radius: f32,
) -> f32 {
    let dy = feature_y - player_y;
    let distance = f64::from(dy * dy).sqrt() as f32;
    if max_magnitude <= 0.0 || scan_radius <= 0.0 {
        return 0.0;
    }
    let magnitude_score =
        ((f64::from(magnitude) + 1.0).ln() / (f64::from(max_magnitude) + 1.0).ln()) as f32;
    magnitude_score * (1.0 - distance / scan_radius).max(0.0)
}
