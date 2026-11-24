//! Host-side contract for the optional opaque blade material.

pub struct GrassImage {
    pub qpath: &'static str,
    pub size: [u32; 2],
    pub srgb: bool,
}

pub struct GrassBladeShaders {
    /// Fragment entry point `fs_ninja` using the host's blade inputs, fog and CSM.
    pub material: &'static str,
    /// `ninja_grass_offset` returns a world-space offset for both color and shadow.
    pub wind: &'static str,
    /// Albedo, normal, roughness, displacement mask, and color noise.
    pub images: [GrassImage; 5],
}
