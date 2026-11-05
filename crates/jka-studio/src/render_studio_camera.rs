//! Project camera attachments: bone bindings, orbit editing and rig resolve.
use crate::attachment::ProfileAttachmentFrame;
use crate::render_studio_timeline::StudioCameraKey;
use serde::{Deserialize, Serialize};

/// Ghoul2 supplies renderer-space poses. Studio owns native JKA authoring data.
pub fn frame_point(frame: ProfileAttachmentFrame, offset: [f32;3], local: bool) -> [f32;3] {
    jka_render_core::coordinates::jka_position(frame.point(if local { offset } else { jka_render_core::coordinates::render_position(offset) }, local))
}
pub fn frame_offset(frame: ProfileAttachmentFrame, point: [f32;3], local: bool) -> [f32;3] {
    let offset=frame.offset(jka_render_core::coordinates::render_position(point),local);
    if local { offset } else { jka_render_core::coordinates::jka_position(offset) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StudioAttachmentPart { #[default] Player, Saber1, Saber2 }
impl StudioAttachmentPart {
    pub fn saber(self)->Option<usize> {match self {Self::Player=>None,Self::Saber1=>Some(0),Self::Saber2=>Some(1)}}
    pub fn label(self)->&'static str {match self {Self::Player=>"Player",Self::Saber1=>"Saber 1",Self::Saber2=>"Saber 2"}}
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StudioCameraAttachment {
    pub enabled: bool,
    pub lane: usize,
    pub part: StudioAttachmentPart,
    pub bone: String,
    pub offset: [f32;3],
    pub local_offset: bool,
    /// Seconds to remove half of a positional error. Zero is a rigid bolt.
    pub half_life: f32,
}
impl Default for StudioCameraAttachment {
    fn default()->Self {Self {enabled:false,lane:0,part:Default::default(),
        bone:"*head_top".into(),offset:[0.;3],local_offset:false,half_life:0.}}
}
impl StudioCameraAttachment {
    pub fn valid(&self,lanes:usize)->bool {
        self.lane<lanes && self.bone.len()<=256 && self.offset.iter().all(|v|v.is_finite())
            && self.half_life.is_finite() && (0.0..=2.0).contains(&self.half_life)
    }
}

/// Spherical editing of the actual eye-to-anchor displacement. Authored camera
/// keys add world movement, and damped bone axes can be scaled/skewed, so the
/// radius must be measured in world space rather than as the raw offset length.
#[derive(Clone,Copy,Debug,PartialEq,Serialize,Deserialize)]
pub struct StudioCameraOrbit {
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}
impl StudioCameraOrbit {
    pub fn valid(&self)->bool {self.distance.is_finite() && self.distance>=0. && self.yaw.is_finite()
        && self.pitch.is_finite() && (-90.0..=90.0).contains(&self.pitch)}
    fn basis(frame:ProfileAttachmentFrame,local:bool)->Option<glam::Mat3> {
        let basis=if local {
            glam::Mat3::from_cols_array_2d(&frame.axis.map(jka_render_core::coordinates::jka_position))
        } else {glam::Mat3::IDENTITY};
        (basis.is_finite() && basis.determinant().abs()>1e-6).then_some(basis)
    }
    pub fn from_binding(binding:&StudioCameraAttachment,frame:ProfileAttachmentFrame,key_delta:[f32;3])->Option<Self> {
        let vector=glam::Vec3::from_array(frame_point(frame,binding.offset,binding.local_offset))
            +glam::Vec3::from_array(key_delta)-glam::Vec3::from_array(jka_render_core::coordinates::jka_position(frame.position));
        if !vector.is_finite() {return None;}
        let direction=Self::basis(frame,binding.local_offset)?.inverse()*vector;
        let distance=vector.length();
        if !distance.is_finite() {return None;}
        let (yaw,pitch)=if distance>1e-5 {
            (direction.y.atan2(direction.x).to_degrees(),-direction.z.atan2(direction.x.hypot(direction.y)).to_degrees())
        } else {(180.,0.)};
        Some(Self {distance,yaw,pitch})
    }
    pub fn apply(&self,binding:&mut StudioCameraAttachment,frame:ProfileAttachmentFrame,key_delta:[f32;3])->bool {
        if !self.distance.is_finite() || self.distance<0. || !self.yaw.is_finite() || !self.pitch.is_finite() {return false;}
        let Some(basis)=Self::basis(frame,binding.local_offset) else{return false;};
        let (sy,cy)=self.yaw.to_radians().sin_cos();let (sp,cp)=self.pitch.to_radians().sin_cos();
        let vector=(basis*glam::Vec3::new(cy*cp,sy*cp,-sp)).normalize_or_zero()*self.distance;
        let eye=glam::Vec3::from_array(jka_render_core::coordinates::jka_position(frame.position))+vector-glam::Vec3::from_array(key_delta);
        let offset=frame_offset(frame,eye.to_array(),binding.local_offset);
        if offset.iter().any(|v|!v.is_finite()) {return false;}
        binding.offset=offset;true
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StudioCameraRig {
    pub position: StudioCameraAttachment,
    pub target: StudioCameraAttachment,
    /// Preserve the authored shot on attachment. Camera keys continue adding
    /// world-space movement relative to this reference, and keep their FOV.
    pub reference_position: [f32;3],
}
impl StudioCameraRig {
    pub fn active(&self)->bool {self.position.enabled || self.target.enabled}
    pub fn valid(&self,lanes:usize)->bool {
        self.position.valid(lanes) && self.target.valid(lanes)
            && self.reference_position.iter().all(|v|v.is_finite())
    }
    pub fn resolve(&self,mut base:StudioCameraKey,position:Option<[f32;3]>,target:Option<[f32;3]>)->StudioCameraKey {
        if self.position.enabled {
            if let Some(p)=position {for i in 0..3 {base.position[i]=p[i]+base.position[i]-self.reference_position[i];}}
        }
        if self.target.enabled {
            if let Some(p)=target {
                let delta=glam::Vec3::from_array(p)-glam::Vec3::from_array(base.position);
                if delta.length_squared()>1e-8 {
                    base.yaw=delta.y.atan2(delta.x).to_degrees();
                    base.pitch=-delta.z.atan2(delta.x.hypot(delta.y)).to_degrees().clamp(-89.9,89.9);
                }
            }
        }
        if base.focus_target.is_none() && self.target.enabled {
            if let Some(target)=target {base.lens.focus_distance=glam::Vec3::from_array(base.position).distance(glam::Vec3::from_array(target)).clamp(1.0,65536.0);}
        }
        base.resolve_focus_target();base
    }
    pub fn position_point(&self,base:StudioCameraKey,frame:ProfileAttachmentFrame)->[f32;3] {
        let mut binding=self.position.clone();
        if let Some(orbit)=base.rig_orbit {orbit.apply(&mut binding,frame,[0.;3]);}
        frame_point(frame,binding.offset,binding.local_offset)
    }
}

