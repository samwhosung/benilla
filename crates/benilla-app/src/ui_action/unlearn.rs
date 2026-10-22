//! The unlearn's bar clear: `UnlearnSpell 0x5e9fe0` ends in `0x4e5e20` (`0x5ea253`), which clears
//! every slot that stands for the spell.

use bevy::prelude::*;

use super::PlayerActions;

/// Drains the unlearned spells the removed-spell message queued ([`PlayerActions::unlearned`]).
pub(super) fn clear_unlearned_slots(mut actions: ResMut<PlayerActions>) {
    if actions.unlearned.is_empty() {
        return;
    }
    actions.unlearned.clear();
}
