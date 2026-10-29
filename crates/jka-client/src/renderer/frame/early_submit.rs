//! Frame early submit.
//!
//! Without it, `render()` waits for a swapchain image before recording a single
//! pass, because the world pass draws straight into that image. The GPU then
//! idles through all of the frame's CPU recording, finish and submit. With
//! r_earlySubmit the frame renders into an offscreen copy of the swapchain
//! format and is submitted first; only then does the CPU wait for the image and
//! submit one copy into it. Late-latched view samples reach the GPU sooner too:
//! the image wait no longer sits between the latch and the submit.
use crate::renderer::{block_on, FrameInfo, Instant, RenderError, Renderer};

pub(in crate::renderer) struct EarlyPresentTarget {
    /// Also the screenshot source on early-submit frames.
    pub(in crate::renderer) texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Renderer {
    /// True when this frame's final image is produced entirely by the main frame
    /// encoder. Menus, CRT, SMAA and HDR output all touch the swapchain image
    /// after that encoder, so those frames keep the acquire-first order.
    /// Screenshots copy from the offscreen frame inside the main encoder.
    pub(in crate::renderer) fn early_submit_frame(&self, menu_backdrop_active: bool) -> bool {
        self.early_submit_enabled && self.offscreen_present_eligible(menu_backdrop_active)
    }

    /// r_presentCopy: TaystJK's Vulkan structure. Render into an offscreen image
    /// and copy it into the acquired swapchain image at the end of the same
    /// command buffer, so the swapchain image is only ever a copy destination.
    pub(in crate::renderer) fn present_copy_frame(&self, menu_backdrop_active: bool) -> bool {
        self.present_copy_enabled && self.offscreen_present_eligible(menu_backdrop_active)
    }

    fn offscreen_present_eligible(&self, menu_backdrop_active: bool) -> bool {
        !menu_backdrop_active
            // A menu-scene snapshot reads the scene back before any native UI
            // overlay; that needs the ordinary acquire-first frame structure.
            && !matches!(
                self.screenshot_requested.as_ref(),
                Some(crate::renderer::ScreenshotDestination::FrontendScene { .. })
            )
            && self.crt_scope == crate::renderer::ui::CrtScope::Off
            && !self.smaa_enabled
            && !self.egui_active
            && self.hdr_output.is_none()
            && !self.surface_diag_pending
            && !self.frame_diag_pending
            && self.config.usage.contains(wgpu::TextureUsages::COPY_DST)
    }

    /// Offscreen stand-in for the swapchain image: same size and format, so every
    /// pipeline built for the surface renders into it unchanged.
    pub(in crate::renderer) fn early_present_view(&mut self) -> wgpu::TextureView {
        let size = wgpu::Extent3d {
            width: self.config.width.max(1),
            height: self.config.height.max(1),
            depth_or_array_layers: 1,
        };
        let stale = self.early_present_target.as_ref().is_none_or(|target| {
            target.texture.size() != size || target.texture.format() != self.config.format
        });
        if stale {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("JKA early-submit frame"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.early_present_target = Some(EarlyPresentTarget { texture, view });
        }
        self.early_present_target
            .as_ref()
            .expect("early present target was just ensured")
            .view
            .clone()
    }

    /// Copy the submitted offscreen frame into the acquired swapchain image.
    pub(in crate::renderer) fn submit_early_present_copy(&self, frame: &super::super::context::RenderFrame) {
        let target = self
            .early_present_target
            .as_ref()
            .expect("early-submit frames render into the early present target");
        let size = target.texture.size();
        if frame.texture.size() != size {
            // A resize between record and acquire; the next frame reconfigures.
            return;
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA early-submit present copy"),
            });
        encoder.copy_texture_to_texture(
            target.texture.as_image_copy(),
            frame.texture.as_image_copy(),
            size,
        );
        self.queue.submit([encoder.finish()]);
    }

    /// Present only the embedded world rectangle. The demo world and its post
    /// processing are rendered to an offscreen texture, never the swapchain.
    /// Clear the editor backing surface first, then GPU-copy the live POV region;
    /// egui will paint its chrome, timeline and inspector over the clean result.
    /// Unlike an egui masking trick, no world pixels exist under those panels.
    /// Editor-owned swapchain clear. It must run even before egui has supplied
    /// a nonzero preview rect, or the acquired surface can display an old frame.
    pub(in crate::renderer) fn submit_studio_editor_clear(&self, view:&wgpu::TextureView) {
        let mut encoder=self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label:Some("Render Studio neutral editor clear"),
        });
        {
            let _clear=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label:Some("Studio neutral editor backing"),
                color_attachments:&[Some(wgpu::RenderPassColorAttachment {
                    view,depth_slice:None,resolve_target:None,
                    ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color {
                        r:0.039,g:0.063,b:0.098,a:1.0}),store:wgpu::StoreOp::Store},
                })],
                depth_stencil_attachment:None,timestamp_writes:None,
                occlusion_query_set:None,multiview_mask:None,
            });
        }
        self.queue.submit([encoder.finish()]);
    }

    pub(in crate::renderer) fn submit_studio_world_composite(
        &mut self,
        frame: &super::super::context::RenderFrame,
        viewport: [u32; 4],
    ) {
        let frame_view=self.surface_texture_view(frame);
        let mut encoder=self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label:Some("Render Studio world viewport composite"),
        });
        // Clear even with an unknown (zero) editor rect on the first frame.
        // Keep the clear + texture copy in ONE submit, preserving low latency.
        {
            let _clear=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label:Some("Studio neutral editor backing"),
                color_attachments:&[Some(wgpu::RenderPassColorAttachment {
                    view:&frame_view,depth_slice:None,resolve_target:None,
                    ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color {
                        r:0.039,g:0.063,b:0.098,a:1.0}),store:wgpu::StoreOp::Store},
                })],
                depth_stencil_attachment:None,timestamp_writes:None,
                occlusion_query_set:None,multiview_mask:None,
            });
        }
        let source=self.early_present_target.as_ref().map(|t|&t.texture);
        if let Some(source)=source {
            let size=source.size();
            if frame.texture.size()==size {
                if let Some([x,y,w,h])=Self::clamped_studio_composite_rect(viewport,size.width,size.height) {
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture:source,mip_level:0,
                            origin:wgpu::Origin3d {x,y,z:0},aspect:wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture:&frame.texture,mip_level:0,
                            origin:wgpu::Origin3d {x,y,z:0},aspect:wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {width:w,height:h,depth_or_array_layers:1},
                    );
                }
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    pub(in crate::renderer) fn clamped_studio_composite_rect(
        viewport: [u32;4], width:u32,height:u32
    ) -> Option<[u32;4]> {
        let [x,y,w,h]=viewport;
        let x=x.min(width);
        let y=y.min(height);
        let w=w.min(width.saturating_sub(x));
        let h=h.min(height.saturating_sub(y));
        (w>0 && h>0).then_some([x,y,w,h])
    }

    /// `Surface::get_current_texture()` with the JKA_SURFACE_DIAG scope and the
    /// error mapping `render()` uses for every frame.
    pub(in crate::renderer) fn acquire_surface_frame(
        &mut self,
        info: &mut FrameInfo,
    ) -> Result<wgpu::SurfaceTexture, RenderError> {
        // One-shot diagnostic for CurrentSurfaceTexture::Validation. Wgpu's
        // Validation variant means a validation error occurred inside
        // Surface::get_current_texture(). Capture that exact call in a validation
        // scope so we can print the real error rather than guessing from the
        // generic surface status. This path is completely dormant unless
        // JKA_SURFACE_DIAG=1 is set.
        let surface_scope = self
            .surface_diag_pending
            .then(|| self.device.push_error_scope(wgpu::ErrorFilter::Validation));
        if self.surface_diag_pending {
            eprintln!(
                "[JKA SURFACE DIAG] before acquire: config={}x{} format={:?} present={:?} latency={} usage={:?} alpha={:?} view_formats={:?}",
                self.config.width,
                self.config.height,
                self.config.format,
                self.config.present_mode,
                self.config.desired_maximum_frame_latency,
                self.config.usage,
                self.config.alpha_mode,
                self.config.view_formats,
            );
        }
        let acquire_started = Instant::now();
        let surface_result = self.surface.get_current_texture();
        info.cpu_acquire_ms = acquire_started.elapsed().as_secs_f64() * 1000.0;
        if let Some(scope) = surface_scope {
            let scoped_error = block_on(scope.pop());
            match scoped_error {
                Some(error) => {
                    eprintln!("[JKA SURFACE DIAG] get_current_texture validation: {error}");
                    eprintln!("[JKA SURFACE DIAG] debug: {error:?}");
                }
                None => {
                    eprintln!("[JKA SURFACE DIAG] get_current_texture scope: no validation error captured");
                }
            }
            eprintln!("[JKA SURFACE DIAG] returned: {surface_result:?}");
            self.surface_diag_pending = false;
        }
        match surface_result {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.flush_staged_queue_writes();
                Err(RenderError::Timeout)
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.flush_staged_queue_writes();
                Err(RenderError::Occluded)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.flush_staged_queue_writes();
                Err(RenderError::Outdated)
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.flush_staged_queue_writes();
                Err(RenderError::Lost)
            }
            wgpu::CurrentSurfaceTexture::Validation => Err(RenderError::Validation),
        }
    }
}

#[cfg(test)]
mod studio_composite_tests {
    use super::Renderer;

    #[test]
    fn studio_copy_never_exceeds_swapchain_bounds() {
        let clip = Renderer::clamped_studio_composite_rect;
        assert_eq!(clip([100, 70, 640, 360], 1920, 1080), Some([100,70,640,360]));
        assert_eq!(clip([1850, 990, 300, 200], 1920, 1080), Some([1850,990,70,90]));
        assert_eq!(clip([1920, 0, 100, 100], 1920, 1080), None);
        assert_eq!(clip([0, 0, 0, 360], 1920, 1080), None);
        assert_eq!(clip([0, 1000, u32::MAX, u32::MAX], 1920, 1080), Some([0,1000,1920,80]));
    }
}
