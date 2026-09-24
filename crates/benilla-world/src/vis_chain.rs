//! The visibility-chain-only idiom: entities that carry hide-propagation but never render.
//!
//! Bevy's visibility pipeline sweeps **every `ViewVisibility` row** three times a frame —
//! `reset_view_visibility` and `mark_newly_hidden_entities_invisible` once each, and
//! `check_visibility` once **per active camera** (its query takes `VisibilityClass` as an
//! `Option`, so carrying no renderable class does not exempt a row — verified in
//! bevy_camera 0.18.1, `visibility/mod.rs`). A second camera (the portrait booth) re-bills
//! the whole population.
//!
//! Most of our world entities that carry `Visibility` never render anything themselves: rig
//! anchors, joint entities, anim-host roots, tile roots, net-object roots, attachment
//! wrappers. They hold `Visibility` for one reason only — hide-propagation to renderable
//! *descendants* (a hidden unit must hide its weapon), and that chain runs entirely on
//! `Visibility` + `InheritedVisibility`. Their `ViewVisibility` row is pure sweep tax: at the
//! Goldshire pin it was ~6.7k of the 25.2k-row population.
//!
//! `Visibility` `#[require]`s `ViewVisibility`, so the component can't be left out at spawn —
//! it has to be removed right after. [`VisChainOnly::vis_chain_only`] is that removal, named:
//! chain a call onto the spawn of any never-rendering node. The inheritance chain stays whole
//! (B0004 validates `InheritedVisibility`, which stays), every `Mut`-write flip of
//! `Visibility` keeps working, and the row leaves all three sweeps.
//!
//! One trap this idiom creates: a later `.insert(Visibility::…)` on the same entity re-adds
//! `ViewVisibility` through the require machinery. Flip visibility on chain nodes through a
//! `Mut<Visibility>` write (as every cull authority already does), or re-strip after the
//! insert (see `transport.rs`, the one insert-based flip).

use bevy::camera::visibility::ViewVisibility;
use bevy::ecs::system::EntityCommands;

/// Chain onto the spawn of a never-rendering hierarchy node — see the module doc.
pub trait VisChainOnly {
    /// Keep `Visibility` + `InheritedVisibility` (the hide-propagation chain), remove
    /// `ViewVisibility` (the per-camera sweep row).
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

    /// The contract: a chain-only node keeps the inheritance pair and sheds the sweep row.
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
