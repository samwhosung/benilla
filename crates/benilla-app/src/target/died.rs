//! `CGUnit_C::OnDied 0x605860`, which the health watcher runs on every unit's alive-to-dead edge
//! (`0x6046f0` at `0x6047a3`: the new health not above 0, the old above).

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use bevy::prelude::*;

use super::Selection;
use crate::creature_anim::{AttackSeam, Engaged};
use crate::net::{FieldChanged, SelfPlayer};

/// Whether `e` is a unit's alive-to-dead edge, signed as the watcher compares (`0x604774 jg`,
/// `0x604778 jle`).
fn died(e: &FieldChanged) -> bool {
    e.unit_field(FIELD_UNIT_HEALTH) && e.old as i32 > 0 && e.new as i32 <= 0
}

/// Each unit's death, in the order the edges came.
pub(super) fn on_unit_died(
    mut edges: MessageReader<FieldChanged>,
    mut selection: ResMut<Selection>,
    mut seam: AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
) {
    for e in edges.read().filter(|e| died(e)) {
        // `SetSelection(0, 0)` at `0x605901`, so the dead target becomes the last one; a corpse
        // selected afterwards stays selected, gray.
        if selection.guid == Some(e.guid) {
            super::click::deselect(&mut selection, &mut seam, !engaged.is_empty());
        }
    }
}
