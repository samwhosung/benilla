//! Our own player's arrival (`OnActiveArrival 0x5dea50`), the last step of its `PostInit`
//! (`0x5debe0`, at `0x5dec7b`), which the update packet's notify pass runs as our create's hook,
//! ahead of the later blocks' notifies: its writes are made here, inside the packet, so the next
//! packet's handlers find our player tagged, its control flag raised and its mover claimed. The
//! cascade's resets and signals still run off [`WorldEnterCascadeMessage`] after the dispatch, and
//! with them the world handle's seat (`0x5deb73`), which comes after the cascade, and the
//! controller's own lock, as a control packet's is the controller's.

use benilla_protocol::ObjectFields;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::Scene;
use crate::net::{ClientCommand, WorldEnterCascadeMessage};

/// What the arrival writes past the streamed world; each absent in a harness without it.
#[derive(SystemParam)]
pub(super) struct Arrival<'w> {
    own_control: Option<ResMut<'w, crate::player::OwnControlLost>>,
    mods: Option<ResMut<'w, crate::spell::SpellModifiers>>,
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    cascades: Option<ResMut<'w, Messages<WorldEnterCascadeMessage>>>,
}

/// The arrival's writes, in its order. Its spellbook, slot pass and ghost state (`0x5dea86`-
/// `0x5deb2a`) are the unit feed's world-enter turn yet.
pub(super) fn arrive(sc: &mut Scene, arrival: &mut Arrival, guid: u64, fields: &ObjectFields) {
    // `0x5deb49`: the manager's `+0xc8` gates the restore, the claim and the cascade; its
    // constructor (`0x4650e7`) is its only store, so it is clear at every create.
    // The controller's lock follows off the cascade (`player::regain_control_on_arrival`).
    if let Some(own) = arrival.own_control.as_deref_mut() {
        crate::player::restore_own_control(own, &mut sc.field_notices);
    }
    // `0x4958a0` (`0x5deb5b`) claims our own mover through `SetActiveMover` (`0x6006e0`), which
    // sends `CMSG_SET_ACTIVE_MOVER` only when the mover changes ([`crate::net::MoverClaim`]). The
    // world entry and the world load that purges the streamed world for every login, reconnect
    // and cross-map worldport zero the mover first, so each of those creates sends; a same-map
    // teleport creates nothing. The server has seated the player by then, so nothing sent here is
    // dropped.
    let claimed = match sc.mover.as_deref_mut() {
        Some(mover) => mover.claim(guid, &sc.net),
        None => sc
            .net
            .0
            .send(ClientCommand::SetActiveMover { guid })
            .is_ok(),
    };
    if claimed && benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "mvr",
            &format!("SET_ACTIVE_MOVER guid={guid:#x} (self create)"),
        );
    }
    // The world-enter cascade (`0x5deb60` → `0x4908c0`), run by its readers.
    if let Some(cascades) = arrival.cascades.as_mut() {
        cascades.write(WorldEnterCascadeMessage);
    }
    // The class family the spell modifiers gate on (`0x5debcc` → `0x6e6ca0`): `ChrClasses` field
    // 15 of the class, byte 1 of `UNIT_FIELD_BYTES_0`. Written only on a move, as the tooltip feed
    // diffs the tables whenever they are marked changed.
    if let (Some(mods), Some(classes), Some(class)) = (
        arrival.mods.as_mut(),
        arrival.classes.as_ref(),
        fields.unit_class(),
    ) {
        let family = classes.0.spell_family(u32::from(class));
        if mods.class_family() != family {
            mods.set_class_family(family);
        }
    }
}

#[cfg(test)]
mod tests;
