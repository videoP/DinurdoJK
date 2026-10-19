//! Sky-portal depth for the volumetric cloud passes.
//!
//! `misc_skyportal` maps draw a second view (the sky camera) into the colour
//! target before the main world pass, and the main pass then clears depth. The
//! cloud passes march against the main depth, so portal geometry (distant
//! terrain, mountains) read as open sky and the clouds were composited over it.
//!
//! The portal view is the main view shifted by a translation, so the same
//! pixel unprojects along the same ray in both. A portal-camera depth sample
//! therefore gives a portal-space distance `d`, and `d / scale` is the distance
//! the cloud march should treat as solid geometry along that ray.
//!
//! `r_cloudPortalDepth` picks where this depth comes from:
//! - 0: not used (legacy behaviour, clouds draw over portal geometry).
//! - 1: depth-only redraw of the portal batches into this texture.
//! - 2: the portal pass itself renders into this texture instead of the
//!   shared depth buffer, so there is no extra draw. Needs a single-sample
//!   portal pass to be sampled afterwards.
use super::DEPTH_FORMAT;

struct PortalDepthTarget {
    size: (u32, u32),
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub(in crate::renderer) struct PortalDepth {
    /// Bound whenever no portal depth is live this frame. The shader gates on
    /// the uniform scale, so its contents are never read.
    _fallback: wgpu::Texture,
    fallback_view: wgpu::TextureView,
    target: Option<PortalDepthTarget>,
    /// `1 / misc_skyportal_orient scale` while a portal depth is live this frame,
    /// -1 for an unscaled portal (it just hides clouds), else 0.
    /// Written into the post uniform; the cloud shader ignores the texture at 0.
    pub(in crate::renderer) inverse_scale: f32,
    /// Whether the sky-portal pass currently renders into `target` rather than
    /// the shared depth buffer (mode 2). Tracked so the flare depth binding,
    /// which caches the view it was given, can be refreshed when it flips.
    pub(in crate::renderer) pass_uses_target: bool,
}

fn depth_texture(device: &wgpu::Device, label: &'static str, size: (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

impl PortalDepth {
    pub(in crate::renderer) fn new(device: &wgpu::Device) -> Self {
        let fallback = depth_texture(device, "JKA sky portal depth fallback", (1, 1));
        let fallback_view = fallback.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            _fallback: fallback,
            fallback_view,
            target: None,
            inverse_scale: 0.0,
            pass_uses_target: false,
        }
    }

    /// View for the post bind group: the live portal depth, or the fallback.
    pub(in crate::renderer) fn view(&self) -> &wgpu::TextureView {
        self.target
            .as_ref()
            .map_or(&self.fallback_view, |target| &target.view)
    }

    /// The live portal depth attachment, if one is allocated.
    pub(in crate::renderer) fn target_view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|target| &target.view)
    }

    /// Allocates (or frees) the depth texture. Returns true when the view the
    /// post bind groups hold changed and they must be rebuilt.
    pub(in crate::renderer) fn ensure(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        wanted: bool,
    ) -> bool {
        if !wanted {
            return self.target.take().is_some();
        }
        if self.target.as_ref().is_some_and(|target| target.size == size) {
            return false;
        }
        let texture = depth_texture(device, "JKA sky portal depth", size);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.target = Some(PortalDepthTarget {
            size,
            _texture: texture,
            view,
        });
        true
    }
}
