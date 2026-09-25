//! FrameXML `alphaMode="ADD"` for Bevy UI: a [`UiMaterial`] blending SrcAlpha/One, so a dim
//! glow brightens bright art under it instead of darkening it.

use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor,
};
use bevy::shader::ShaderRef;
use bevy::ui_render::ui_material::{UiMaterial, UiMaterialKey};

/// An additive UI overlay: `texture`'s uv sub-region `rect` `(u0, v0, u1, v1)`, drawn
/// `dst + src·srcAlpha`.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct AddUiMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub(crate) texture: Handle<Image>,
    #[uniform(2)]
    pub(crate) rect: Vec4,
}

impl UiMaterial for AddUiMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_app/shaders/ui_add.wgsl".into()
    }

    fn specialize(descriptor: &mut RenderPipelineDescriptor, _key: UiMaterialKey<Self>) {
        if let Some(target) = descriptor
            .fragment
            .as_mut()
            .and_then(|f| f.targets.first_mut())
            .and_then(|t| t.as_mut())
        {
            target.blend = Some(BlendState {
                color: BlendComponent {
                    src_factor: BlendFactor::SrcAlpha,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                },
                // The destination's coverage stays: the add never erodes what is under it.
                alpha: BlendComponent {
                    src_factor: BlendFactor::Zero,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                },
            });
        }
    }
}
