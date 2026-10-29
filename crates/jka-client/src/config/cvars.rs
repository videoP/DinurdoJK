//! Config adapters attached to the canonical cvar declarations.
use super::*;

pub(crate) struct ArchiveState<'a> {
    pub video: &'a VideoSettings,
    pub presentation: &'a ClientPresentationSettings,
    pub audio: &'a AudioSettings,
}
#[derive(Default)]
pub(crate) struct VideoLoadState {
    pub custom_width: Option<u32>,
    pub custom_height: Option<u32>,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    pub novis: Option<bool>,
    pub pvs_mode: Option<PvsMode>,
    pub anisotropy: Option<f32>,
    pub dynamic_shadows_mode_seen: bool,
    pub static_world_csm_seen: bool,
}
#[derive(Clone, Copy)]
pub(crate) struct CvarBinding {
    pub default: Option<fn(&ArchiveState<'_>) -> String>,
    pub load_video: Option<fn(&mut VideoSettings, &str, &str, &mut VideoLoadState)>,
    pub load_presentation: Option<fn(&mut ClientPresentationSettings, &str, &str)>,
    pub load_audio: Option<fn(&mut AudioSettings, &str, &str)>,
    pub apply_latched: Option<fn(&mut VideoSettings, &str)>,
    pub archive: Option<(usize, fn(&ArchiveState<'_>) -> String)>,
}
impl CvarBinding {
    pub const NONE: Self = Self {
        default: None,
        load_video: None,
        load_presentation: None,
        load_audio: None,
        apply_latched: None,
        archive: None,
    };
}
pub(crate) fn texture_mode(filter: TextureFilter) -> &'static str {
    match filter {
        TextureFilter::Nearest => "GL_NEAREST",
        TextureFilter::Bilinear => "GL_LINEAR_MIPMAP_NEAREST",
        _ => "GL_LINEAR_MIPMAP_LINEAR",
    }
}
pub(crate) fn anisotropy(filter: TextureFilter) -> u32 {
    match filter {
        TextureFilter::Anisotropic2x => 2,
        TextureFilter::Anisotropic4x => 4,
        TextureFilter::Anisotropic8x => 8,
        TextureFilter::Anisotropic16x => 16,
        _ => 0,
    }
}
