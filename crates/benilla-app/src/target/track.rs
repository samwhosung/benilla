//! The selection's object, as the object manager holds it this frame.

use bevy::prelude::*;

use crate::creature_anim::Engaged;
use crate::net::{GuidIndex, SelfPlayer};

use super::Selection;

/// End the selection when its object leaves the object manager, in the net stage, as the objects
/// that stream out apply: a unit's deactivation (`0x5fbb60`), destroyed or out of range, runs the
/// teardown with its send (`0x5fbbe5`).
pub(crate) fn follow_selection_object(
    mut selection: ResMut<Selection>,
    index: Res<GuidIndex>,
    mut seam: crate::creature_anim::AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
) {
    let Some(target) = selection.target else {
        return;
    };
    if selection.guid.and_then(|g| index.0.get(&g)) != Some(&target) {
        super::click::clear(&mut selection, &mut seam, !engaged.is_empty());
    }
}
