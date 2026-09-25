//! The equip, auto-equip and use soulbind confirmations (`EQUIP_BIND`, `AUTOEQUIP_BIND`,
//! `USE_BIND`). Unlike the loot arm (`bonding == 1` and `quality >= 2`, `0x4c2790`), they read
//! only the template's bonding at `+0x194`, never quality: `SwapItem` (`0x5e0c40`) and
//! `AutoEquipCursorItem` (`0x5e1480`) ask on 2, `CGItem::Use` (`0x5d8d00`) on 3.
//!
//! There is no confirm packet: `EquipPendingItem(index)` re-runs the original action with the
//! suppress flag set, and `CancelPendingEquip(index)` sends nothing and drops the record. The
//! dialogs are `exclusive`, so a new question hides the standing one, whose `OnHide` cancels it.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::items::{Enchants, Items};
use crate::net::Objects;

/// The equip arms' `item_template.bonding` (vmangos `ItemPrototype.h` `ItemBondingType`).
pub(crate) const BIND_WHEN_EQUIPPED: u32 = 2;
/// The use arm's value (`0x5d91d6`).
pub(crate) const BIND_WHEN_USE: u32 = 3;

/// One deferred action, stored as its own coordinates rather than a built packet: accept re-runs
/// the same sender with `suppress` set, so a slot that changed under the dialog is re-judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingEquip {
    /// `EQUIP_BIND_CONFIRM`, in Lua space so the re-issue goes back through the move drain's own
    /// sender, which re-derives the wire pair and takes the pending-op lock.
    Swap {
        src_bag: i64,
        src_slot: u32,
        dst_bag: i64,
        dst_slot: u32,
    },
    /// `AUTOEQUIP_BIND_CONFIRM`, in wire space: the action bar's sender resolves an item entry to
    /// a position and has no Lua pair. The guid lets the re-issue re-take the ammo fork.
    AutoEquip { bag_index: u8, slot: u8, guid: u64 },
}

/// The client's pending-equip array (`0xc4c290`-`0xc4c29c`, stride `0x20`), allocated by
/// `0x5e1110`: the event's `arg1` is a 0-based index into it, passed through Lua untouched.
/// Not reset on world enter or leave: the reference zeroes it once at static init.
#[derive(Resource, Default)]
pub(crate) struct PendingEquips {
    slots: Vec<Option<PendingEquip>>,
}

impl PendingEquips {
    /// Files into the first free index, growing only when none is free, as the reference does.
    pub(crate) fn add(&mut self, rec: PendingEquip) -> u32 {
        let idx = match self.slots.iter().position(Option::is_none) {
            Some(i) => {
                self.slots[i] = Some(rec);
                i
            }
            None => {
                self.slots.push(Some(rec));
                self.slots.len() - 1
            }
        };
        u32::try_from(idx).unwrap_or(u32::MAX)
    }

    /// Frees the element. An index nobody filed does nothing, as `0x5e1be0` bounds-checks it.
    pub(crate) fn take(&mut self, index: u32) -> Option<PendingEquip> {
        self.slots.get_mut(index as usize)?.take()
    }

    /// Live records: a cancelled or accepted question must free its element.
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        self.slots.iter().flatten().count()
    }
}

/// The use arm's pending state, one cell with no index and no cancel verb: the reference stamps
/// the item guid (`0xc4c240`) and target guid (`0xc4c1d0`) and `ConfirmBindOnUse()` re-issues
/// `CGItem::Use(&target, suppress = 1)` off them.
#[derive(Resource, Default)]
pub(crate) struct PendingBindOnUse(pub(crate) Option<PendingUse>);

/// The deferred use: the [`crate::ui_items::ItemUse`] call about to be made, which carries what
/// the reference's item and target guids do.
pub(crate) type PendingUse = crate::ui_items::ItemUse;

/// The deferral's resources in one system parameter. [`Items`] is not in it: every caller already
/// holds it, and a second `ResMut<Items>` in one system conflicts.
#[derive(SystemParam)]
pub(crate) struct BindGate<'w> {
    pub(crate) equips: ResMut<'w, PendingEquips>,
    pub(crate) on_use: ResMut<'w, PendingBindOnUse>,
    /// An enchant can soulbind its item, and `0x5da2c0` walks all seven enchant slots; without
    /// client data only the instance flag counts.
    pub(crate) enchants: Option<Res<'w, Enchants>>,
}

impl BindGate<'_> {
    /// Whether this equip must ask first: not already bound (`0x5da2c0`), `bonding == 2` and
    /// equippable (`0x5ea930`). `item_guid` is the item that would bind (a swap's non-equip side,
    /// an auto-equip's clicked slot); an unknown item or uncached template fails open, as the
    /// reference's cache-hit conjunct does.
    pub(crate) fn equip_binds(
        &self,
        script: &UiScript,
        objects: &Objects,
        items: &Items,
        commands: &crate::net::NetCommands,
        item_guid: u64,
    ) -> bool {
        let Some(fields) = objects.object(item_guid) else {
            return false;
        };
        if crate::items::already_bound(fields, self.enchants.as_deref()) {
            return false;
        }
        let Some(entry) = objects.object(item_guid).and_then(|o| o.object_entry()) else {
            return false;
        };
        let Some(t) = items.template(entry, item_guid, commands) else {
            return false;
        };
        t.bonding == BIND_WHEN_EQUIPPED && script.item_usable(entry)
    }

    /// The use arm's predicate: `[0x5d91d3, 0x5d91f2)` holds exactly three branches to the cast,
    /// `bonding == 3`, not already bound (`0x5da2c0`) and `suppress == 0`. There is no can-use
    /// (`0x5ea930`) leg: that failure exits `0x5d8d00` before the bind arm.
    pub(crate) fn use_binds(
        &self,
        objects: &Objects,
        items: &Items,
        commands: &crate::net::NetCommands,
        item_guid: u64,
    ) -> bool {
        let Some(fields) = objects.object(item_guid) else {
            return false;
        };
        if crate::items::already_bound(fields, self.enchants.as_deref()) {
            return false;
        }
        let Some(entry) = objects.object(item_guid).and_then(|o| o.object_entry()) else {
            return false;
        };
        items
            .template(entry, item_guid, commands)
            .is_some_and(|t| t.bonding == BIND_WHEN_USE)
    }

    /// Files a deferred equip and raises its dialog. The two equip arms differ only in the event
    /// (`UIParent.lua:324-339`, where each hides the other's dialog first).
    pub(crate) fn defer_equip(&mut self, script: &mut UiScript, rec: PendingEquip) {
        let index = self.equips.add(rec);
        let event = match rec {
            PendingEquip::Swap { .. } => "EQUIP_BIND_CONFIRM",
            PendingEquip::AutoEquip { .. } => "AUTOEQUIP_BIND_CONFIRM",
        };
        debug!("ui_bind_confirm: {event} index {index} for {rec:?}");
        // Queued, not fired: the triggering action has already queued its `CURSOR_UPDATE`, which
        // hides both equip dialogs (`UIParent.lua:357-361`). Fired now, the question would come
        // first and be cancelled on the next tick.
        script.queue_event(event, vec![ScriptValue::Int(i64::from(index))]);
    }

    /// Files the deferred use and raises `USE_BIND`, with no argument: a second deferred use
    /// replaces the first.
    pub(crate) fn defer_use(&mut self, script: &mut UiScript, pending: PendingUse) {
        debug!(
            "ui_bind_confirm: USE_BIND_CONFIRM for wire {}/{}",
            pending.bag_index, pending.slot
        );
        self.on_use.0 = Some(pending);
        // Queued like the equip arms, though `CURSOR_UPDATE` does not hide `USE_BIND`.
        script.queue_event("USE_BIND_CONFIRM", vec![]);
    }
}
