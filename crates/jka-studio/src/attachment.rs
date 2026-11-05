//! Posed bone frame for Studio camera attachments, and the saber pseudo bones.

#[derive(Clone, Copy, Debug)]
pub struct ProfileAttachmentFrame {
    /// Renderer coordinates, matching Camera rather than the Z-up asset space.
    pub position: [f32;3],
    pub axis: [[f32;3];3],
}

impl ProfileAttachmentFrame {
    pub fn point(&self, offset: [f32;3], local: bool) -> [f32;3] {
        std::array::from_fn(|i| self.position[i] + if local {
            (0..3).map(|j|self.axis[j][i]*offset[j]).sum::<f32>()
        } else {offset[i]})
    }
    pub fn offset(&self, point: [f32;3], local: bool) -> [f32;3] {
        let delta=std::array::from_fn::<_,3,_>(|i|point[i]-self.position[i]);
        if local {
            let matrix=glam::Mat3::from_cols_array_2d(&self.axis);
            if matrix.determinant().abs()>1e-8 {(matrix.inverse()*glam::Vec3::from_array(delta)).to_array()}
            else {[0.;3]}
        } else {delta}
    }
}

/// Pseudo bone for the Studio camera: the primary saber's first blade tip.
pub const SABER_TIP_ANCHOR: &str = "@saber_tip";
/// Pseudo bone: the base of that blade (the hilt), held or thrown.
pub const SABER_HILT_ANCHOR: &str = "@saber_hilt";
