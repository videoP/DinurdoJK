//! Full-frame-equivalent thin-lens settings for Studio output frames.

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StudioLens {
    /// None is a pinhole: displayed as infinity, no lens blur or allocation.
    pub aperture: Option<f32>,
    pub focus_distance: f32,
}
impl Default for StudioLens {fn default()->Self{Self{aperture:None,focus_distance:256.0}}}
impl StudioLens {
    pub fn valid(self)->bool{self.aperture.is_none_or(|n|n.is_finite()&&(0.7..=128.0).contains(&n))
        &&self.focus_distance.is_finite()&&(1.0..=65536.0).contains(&self.focus_distance)}
    pub fn interpolate(self,other:Self,t:f32)->Self {
        let inverse=self.aperture.map_or(0.0,|n|1.0/n)*(1.0-t)+other.aperture.map_or(0.0,|n|1.0/n)*t;
        Self{aperture:(inverse>1e-6).then(||1.0/inverse),focus_distance:self.focus_distance+(other.focus_distance-self.focus_distance)*t}
    }
    /// cg_fov is horizontal at 4:3. Match actual output horizontal framing
    /// against a 36 mm-wide full-frame sensor cropped to the output aspect.
    pub fn focal_length_35mm(cg_fov:f32,aspect:f32)->f32 {
        18.0/(0.75*aspect.max(0.01)*(cg_fov.clamp(1.0,176.0).to_radians()*0.5).tan())
    }
}

