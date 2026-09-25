//! **The 2D opaque pass, skipped when it has nothing to draw**.
//!
//! bevy's `MainOpaquePass2dNode` opens a command encoder and a render pass whether or not its two
//! phases hold an item, and on every camera of ours they never do: each player-UI quad is in the
//! transparent phase (`AlphaMode2d::Blend`), Bevy UI paints through its own node,
//! egui through its own. 2197 parked the pass as "price it on an immediate-mode GPU"; 2205 priced
//! its GPU at zero; the crowd profile of 2225 priced its CPU at ~0.2 ms a frame, parked, traced —
//! an encoder and a pass that wgpu sizes and clears its usage trackers for, empty or not (the
//! residency term `perf::gpu`'s census names).
//!
//! The pass exists in bevy for the clear, and the clear does not need it: whichever pass touches
//! the view target first clears it (`ColorAttachment`'s first-call semantics, the same rule
//! `static_gx` and the FFX combine already lean on), and bevy's transparent 2D node opens its pass
//! unconditionally for exactly that reason. So an empty opaque pass has nothing to do that the
//! pass after it does not.
//!
//! This is bevy's node with one early return, registered under bevy's own label:
//! `RenderGraph::add_node` is a map insert, so the later registration replaces the node and the
//! graph's edges — keyed by label — carry over untouched.

use bevy::core_pipeline::core_2d::graph::{Core2d, Node2d};
use bevy::core_pipeline::core_2d::{AlphaMask2d, Opaque2d};
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::render_graph::{
    NodeRunError, RenderGraphContext, RenderGraphExt, ViewNode, ViewNodeRunner,
};
use bevy::render::render_phase::{TrackedRenderPass, ViewBinnedRenderPhases};
use bevy::render::render_resource::{CommandEncoderDescriptor, RenderPassDescriptor, StoreOp};
use bevy::render::renderer::RenderContext;
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::RenderApp;

#[derive(Default)]
struct SkipEmptyOpaque2dNode;

impl ViewNode for SkipEmptyOpaque2dNode {
    type ViewQuery = (
        &'static ExtractedCamera,
        &'static ExtractedView,
        &'static ViewTarget,
        &'static ViewDepthTexture,
    );

    fn run<'w>(
        &self,
        graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (camera, view, target, depth): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let (Some(opaque_phases), Some(alpha_mask_phases)) = (
            world.get_resource::<ViewBinnedRenderPhases<Opaque2d>>(),
            world.get_resource::<ViewBinnedRenderPhases<AlphaMask2d>>(),
        ) else {
            return Ok(());
        };
        let view_entity = graph.view_entity();
        let (Some(opaque_phase), Some(alpha_mask_phase)) = (
            opaque_phases.get(&view.retained_view_entity),
            alpha_mask_phases.get(&view.retained_view_entity),
        ) else {
            return Ok(());
        };
        // The one line bevy's node does not have. Nothing to draw means nothing to encode: the
        // transparent pass that follows opens its own pass unconditionally and takes the target's
        // first-call clear.
        if opaque_phase.is_empty() && alpha_mask_phase.is_empty() {
            return Ok(());
        }

        let diagnostics = render_context.diagnostic_recorder();
        let color_attachments = [Some(target.get_color_attachment())];
        let depth_stencil_attachment = Some(depth.get_attachment(StoreOp::Store));

        render_context.add_command_buffer_generation_task(move |render_device| {
            let mut command_encoder =
                render_device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("main_opaque_pass_2d_command_encoder"),
                });
            let render_pass = command_encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("main_opaque_pass_2d"),
                color_attachments: &color_attachments,
                depth_stencil_attachment,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut render_pass = TrackedRenderPass::new(&render_device, render_pass);
            let pass_span = diagnostics.pass_span(&mut render_pass, "main_opaque_pass_2d");
            if let Some(viewport) = camera.viewport.as_ref() {
                render_pass.set_camera_viewport(viewport);
            }
            if !opaque_phase.is_empty() {
                if let Err(err) = opaque_phase.render(&mut render_pass, world, view_entity) {
                    error!("Error encountered while rendering the 2d opaque phase {err:?}");
                }
            }
            if !alpha_mask_phase.is_empty() {
                if let Err(err) = alpha_mask_phase.render(&mut render_pass, world, view_entity) {
                    error!("Error encountered while rendering the 2d alpha mask phase {err:?}");
                }
            }
            pass_span.end(&mut render_pass);
            drop(render_pass);
            command_encoder.finish()
        });
        Ok(())
    }
}

/// Replaces bevy's 2D opaque node under its own label. Added by [`crate::ui_pass::PlayerUiPlugin`],
/// which owns every camera that runs the `Core2d` graph, after `DefaultPlugins` has registered the
/// node it replaces.
pub(crate) struct SkipEmptyOpaque2dPlugin;

impl Plugin for SkipEmptyOpaque2dPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.add_render_graph_node::<ViewNodeRunner<SkipEmptyOpaque2dNode>>(
            Core2d,
            Node2d::MainOpaquePass,
        );
    }
}
