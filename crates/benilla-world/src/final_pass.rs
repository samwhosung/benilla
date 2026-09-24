//! **Where a colour lane's final pass lands** — the one rule the FFXGlow combine
//! ([`crate::ffx_glow`]) and the UI gamma decode (`benilla_app::ui_gamma`) share.
//!
//! Each of benilla's two colour lanes ends in a full-screen pass that owns the frame's one gamma
//! decode (0161 for the world, 0254 for the UI). Bevy's default for a camera is
//! [`CameraOutputMode::Write`]: that pass writes the view's main texture, and bevy's `upscaling`
//! node then **blits** the result into the camera's target — a pure copy of every pixel, once per
//! camera. On the Windows lab's RTX that copy was the costliest pass of the frame (2205: `upscaling`
//! 1.1–1.4 ms of a 4.3 ms GPU frame, both cameras' blits summed), and on a tile GPU it is one more
//! full-screen pass on the render thread 1975 called the floor.
//!
//! With the camera set to [`CameraOutputMode::Skip`] the blit is off, and the lane's final pass
//! renders **straight into the target** — a `Skip` bake's combine into its image, the UI camera's
//! decode into the swapchain. Same shader, same values, one full-screen pass fewer per camera;
//! the pipeline is specialised on the target's format because that is now the format it writes.
//! (The world camera's combine no longer lands anywhere of its own: since decision 2234 it is
//! the first draw of the UI camera's main pass, `ffx_glow::FfxBackdrop`, and the world's target is a size-carrier
//! nothing writes — so a claimed world view never resolves a destination at all.) The first use of the output in a frame is a clear rather than a load: a fast clear on an
//! immediate-mode GPU, and on a tile GPU the difference between not reading the old contents and
//! reading them — the fullscreen triangle covers every pixel of the viewport either way.
//!
//! A `Write` camera keeps the ping-pong ([`ViewTarget::post_process_write`]) and the blit, exactly
//! as before: a bake's own `clear_color` and viewport (the glue booth's pillarbox, 1619) are the
//! blit's to honour, and `benilla-worldview` has no reason to change. So the choice is the camera's
//! — made where the camera is spawned — and a node only reads it. **The one hazard is the
//! half-way state:** a `Skip` camera whose final pass never ran presents a target nothing wrote.
//! That is why `$WOW_NO_FFX` (`ffx_glow::ensure_ffx_glow`) strips the glow and keeps the combine.

use bevy::camera::{CameraOutputMode, Viewport};
use bevy::color::LinearRgba;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{
    Operations, RenderPassColorAttachment, TextureFormat, TextureView,
};
use bevy::render::view::ViewTarget;

/// One final pass's source and destination for one view — the seam's one noun. Its two
/// associated functions are the two halves of the rule: [`Self::format`] for the pipeline (at
/// prepare), [`Self::resolve`] for the pass (in the node).
pub struct FinalPassTarget<'a> {
    /// The finished main texture the pass reads.
    pub source: &'a TextureView,
    /// The attachment it renders into: the output texture (`Skip`) or the other main texture
    /// (`Write`).
    pub destination: RenderPassColorAttachment<'a>,
    /// A `Skip` camera's viewport, to be applied as the pass's scissor — what bevy's blit does for
    /// the same camera. `None` covers the whole target.
    pub scissor: Option<&'a Viewport>,
}

impl<'a> FinalPassTarget<'a> {
    /// The format a lane's final pass renders in, for a view whose camera runs `output_mode`:
    /// the output texture's own for a `Skip` camera, the main texture's for a `Write` one. What
    /// the pass's pipeline is specialised on.
    pub fn format(output_mode: &CameraOutputMode, target: &ViewTarget) -> TextureFormat {
        match output_mode {
            CameraOutputMode::Skip => target.out_texture_view_format(),
            CameraOutputMode::Write { .. } => target.main_texture_format(),
        }
    }

    /// Resolve a view's final pass target from its camera's output mode. Call **once** per view
    /// per frame: the `Write` arm takes the view's post-process write, which flips the main
    /// texture.
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

    /// The scissor rect the pass sets on its render pass — `(x, y, width, height)` in physical
    /// pixels — when [`Self::scissor`] names one.
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
