//! Visibility-chain-only nodes: entities that carry hide-propagation to renderable descendants
//! but never render (rig anchors, joints, anim-host, tile and net-object roots, attachment
//! wrappers). Bevy sweeps every `ViewVisibility` row twice a frame and once more per active
//! camera, renderable or not (bevy_camera 0.18.1, `visibility/mod.rs`), and `Visibility` requires
//! the component, so [`VisChainOnly::vis_chain_only`] removes it after spawn and keeps
//! `Visibility` and `InheritedVisibility`.
//!
//! Trap: a later `.insert(Visibility::…)` re-adds `ViewVisibility`; flip a chain node through a
//! `Mut<Visibility>` write, or strip it again after the insert (as `transport.rs` does).

use bevy::camera::visibility::ViewVisibility;
use bevy::ecs::system::EntityCommands;

/// Chain onto the spawn of a never-rendering hierarchy node.
pub trait VisChainOnly {
    /// Keep `Visibility` and `InheritedVisibility`, the hide-propagation chain; remove
    /// `ViewVisibility`, the per-camera sweep row.
    fn vis_chain_only(&mut self) -> &mut Self;
}

impl VisChainOnly for EntityCommands<'_> {
    fn vis_chain_only(&mut self) -> &mut Self {
        self.remove::<ViewVisibility>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::visibility::{InheritedVisibility, Visibility};
    use bevy::prelude::*;

    #[test]
    fn a_chain_only_node_keeps_inheritance_and_leaves_the_sweep() {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        let e = commands
            .spawn((Transform::IDENTITY, Visibility::default()))
            .vis_chain_only()
            .id();
        queue.apply(&mut world);
        assert!(world.get::<Visibility>(e).is_some());
        assert!(world.get::<InheritedVisibility>(e).is_some());
        assert!(
            world.get::<ViewVisibility>(e).is_none(),
            "the require chain re-added ViewVisibility — the sweep row is back"
        );
    }
}
