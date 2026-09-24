//! Where a colour lane's final pass lands, for the FFXGlow combine ([`crate::ffx_glow`]) and the UI
//! gamma decode (`benilla_app::ui_gamma`). A [`CameraOutputMode::Write`] camera keeps bevy's
//! ping-pong and `upscaling` blit, which honours a bake's clear colour and viewport; a
//! [`CameraOutputMode::Skip`] camera's final pass renders straight into the target, in its format,
//! clearing it rather than loading it, since the fullscreen triangle covers the viewport either
//! way. A world view the UI camera claims (`ffx_glow::FfxBackdrop`) resolves none: its combine is
//! the first draw of that camera's main pass. A `Skip` camera whose final pass never ran presents
//! a target nothing wrote, which is why `$WOW_NO_FFX` keeps the combine.

use bevy::camera::{CameraOutputMode, Viewport};
use bevy::color::LinearRgba;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{
    Operations, RenderPassColorAttachment, TextureFormat, TextureView,
};
use bevy::render::view::ViewTarget;

/// One final pass's source and destination for one view: [`Self::format`] at prepare for the
/// pipeline, [`Self::resolve`] in the node for the pass.
pub struct FinalPassTarget<'a> {
    /// The finished main texture the pass reads.
    pub source: &'a TextureView,
    /// The output texture (`Skip`) or the other main texture (`Write`).
    pub destination: RenderPassColorAttachment<'a>,
    /// A `Skip` camera's viewport, as the pass's scissor; `None` covers the whole target.
    pub scissor: Option<&'a Viewport>,
}

impl<'a> FinalPassTarget<'a> {
    /// The format the final pass renders in, which its pipeline is specialised on.
    pub fn format(output_mode: &CameraOutputMode, target: &ViewTarget) -> TextureFormat {
        match output_mode {
            CameraOutputMode::Skip => target.out_texture_view_format(),
            CameraOutputMode::Write { .. } => target.main_texture_format(),
        }
    }

    /// Resolve a view's final pass target. Call once per view per frame: the `Write` arm takes the
    /// view's post-process write, which flips the main texture.
    pub fn resolve(camera: &'a ExtractedCamera, target: &'a ViewTarget) -> Self {
        match camera.output_mode {
            CameraOutputMode::Skip => Self {
                source: target.main_texture_view(),
                destination: target.out_texture_color_attachment(Some(LinearRgba::NONE)),
                scissor: camera.viewport.as_ref(),
            },
            CameraOutputMode::Write { .. } => {
                let post = target.post_process_write();
                Self {
                    source: post.source,
                    destination: RenderPassColorAttachment {
                        view: post.destination,
                        depth_slice: None,
                        resolve_target: None,
                        ops: Operations::default(),
                    },
                    scissor: None,
                }
            }
        }
    }

    /// The pass's scissor rect, `(x, y, width, height)` in physical pixels.
    pub fn scissor_rect(&self) -> Option<(u32, u32, u32, u32)> {
        self.scissor.map(|viewport| {
            (
                viewport.physical_position.x,
                viewport.physical_position.y,
                viewport.physical_size.x,
                viewport.physical_size.y,
            )
        })
    }
}
