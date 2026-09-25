//! Bevy's 2D opaque pass, skipped when both its phases are empty, which on our cameras they always
//! are (UI quads are transparent, Bevy UI and egui draw in their own nodes). The first pass to
//! touch the view target clears it, so the transparent pass after it takes the clear.
//! Registered under Bevy's own label: `add_node` is a map insert, so it replaces the node and the
//! edges keyed by label carry over.

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
        // The one line Bevy's node lacks: the transparent pass that follows opens unconditionally
        // and takes the target's first-call clear.
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

/// Replaces Bevy's 2D opaque node; must be added after `DefaultPlugins` registers the original
/// (by [`crate::ui_pass::PlayerUiPlugin`]).
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
