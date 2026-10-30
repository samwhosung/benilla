//! The equip, auto-equip and use soulbind confirmations (`EQUIP_BIND`, `AUTOEQUIP_BIND`,
//! `USE_BIND`). Unlike the loot arm (`bonding == 1` and `quality >= 2`, `0x4c2790`), they read
//! only the template's bonding at `+0x194`, never quality: `SwapItem` (`0x5e0c40`) and
//! `AutoEquipCursorItem` (`0x5e1480`) ask on 2, `CGItem::Use` (`0x5d8d00`) on 3.
//!
//! There is no confirm packet: `EquipPendingItem(index)` re-runs the original action with the
//! suppress flag set, and `CancelPendingEquip(index)` sends nothing and drops the record. The
//! dialogs are `exclusive`, so a new question hides the standing one, whose `OnHide` cancels it.
//! The two equip arms ask in the VM, where the item is held ([`benilla_ui::script::Swap`],
//! [`benilla_ui::script::AutoEquip`]); the app files their records, signals their dialogs inside
//! the call, and parks a move whose item record is not cached until the record answers.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ContainerMove, ScriptValue, UiScript};

use crate::items::{Enchants, Items, RecordCallback};
use crate::net::{NetCommands, Objects};
use crate::query_cache::Ask;

/// The use arm's `item_template.bonding` (`0x5d91d6`; vmangos `ItemPrototype.h`
/// `ItemBondingType`). The equip arms' value is the VM's ([`benilla_ui::script::BIND_WHEN_EQUIPPED`]).
pub(crate) const BIND_WHEN_USE: u32 = 3;

/// One deferred action, stored as its own coordinates rather than a built packet: accept re-runs
/// the same sender with `suppress` set, so a slot that changed under the dialog is re-judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingEquip {
    /// `EQUIP_BIND_CONFIRM`, in Lua space so the re-issue goes back through `SwapItem`, which
    /// re-derives the wire pair. `parked` is the entry a swap waits on with no question asked,
    /// its record not cached (`0x5e0dbe`), which the record's arrival replays (`0x5e1020`); an
    /// answer to its index works on it all the same (`0x5e1be0`).
    Swap {
        src_bag: i64,
        src_slot: u32,
        dst_bag: i64,
        dst_slot: u32,
        parked: Option<u32>,
    },
    /// `AUTOEQUIP_BIND_CONFIRM`, in Lua space, where the accept re-issues the pick-up and
    /// auto-equip from the slot as it stands (`0x5e1c23`). The guid is the item the cancel unlocks.
    AutoEquip { bag: i64, slot: u32, guid: u64 },
}

impl PendingEquip {
    /// A swap's record from its move, parked on `parked` or asked.
    pub(crate) fn swap(mv: &ContainerMove, parked: Option<u32>) -> Self {
        PendingEquip::Swap {
            src_bag: mv.src_bag,
            src_slot: mv.src_slot,
            dst_bag: mv.dst_bag,
            dst_slot: mv.dst_slot,
            parked,
        }
    }
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

    /// The swaps parked on `entry`, by index, in the table's order (`0x5e1087`-`0x5e10ed`).
    /// `0x5e1020` matches on the element's entry field `+0x18`, which a question's record holds
    /// too (`0x5e0ee0`, beside the parked one's `0x5e0e1f`), so it replays and frees a standing
    /// question on that entry as well; this walk takes the parked records alone, and a standing
    /// question keeps its record for its dialog.
    fn parked_on(&self, entry: u32) -> Vec<(u32, ContainerMove)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, rec)| match *rec {
                Some(PendingEquip::Swap {
                    src_bag,
                    src_slot,
                    dst_bag,
                    dst_slot,
                    parked: Some(e),
                }) if e == entry => Some((
                    u32::try_from(i).ok()?,
                    ContainerMove {
                        src_bag,
                        src_slot,
                        dst_bag,
                        dst_slot,
                        count: None,
                    },
                )),
                _ => None,
            })
            .collect()
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
    /// The use arm's predicate: `[0x5d91d3, 0x5d91f2)` holds exactly three branches to the cast,
    /// `bonding == 3`, not already bound (`0x5da2c0`) and `suppress == 0`. There is no can-use
    /// (`0x5ea930`) leg: that failure exits `0x5d8d00` before the bind arm.
    pub(crate) fn use_binds(&self, objects: &Objects, items: &Items, item_guid: u64) -> bool {
        let Some(fields) = objects.object(item_guid) else {
            return false;
        };
        let Some(entry) = fields.object_entry() else {
            return false;
        };
        let flags = crate::items::record_flags(items, entry, item_guid);
        if crate::items::already_bound(fields, flags, self.enchants.as_deref()) {
            return false;
        }
        items
            .template(entry, item_guid, Ask::Read)
            .is_some_and(|t| t.bonding == BIND_WHEN_USE)
    }

    /// Files a deferred equip and raises its dialog in place, inside the call that asked
    /// (`0x5e0ee4`, `0x5e16c6`). The two equip arms differ only in the event
    /// (`UIParent.lua:324-339`, where each hides the other's dialog first).
    pub(crate) fn defer_equip(&mut self, script: &mut UiScript, rec: PendingEquip) {
        let index = self.equips.add(rec);
        let event = match rec {
            PendingEquip::Swap { .. } => "EQUIP_BIND_CONFIRM",
            PendingEquip::AutoEquip { .. } => "AUTOEQUIP_BIND_CONFIRM",
        };
        debug!("ui_bind_confirm: {event} index {index} for {rec:?}");
        script.fire_event(event, vec![ScriptValue::Int(i64::from(index))]);
    }

    /// `SwapItem` on an uncached record (`0x5e0dbe`-`0x5e0e2e`): the move takes a record in the
    /// table, signalling nothing, and the lookup asked the record with the callback `0x5e1020`.
    pub(crate) fn park_swap(
        &mut self,
        mv: &ContainerMove,
        entry: u32,
        items: &Items,
        commands: &NetCommands,
    ) {
        let index = self.equips.add(PendingEquip::swap(mv, Some(entry)));
        debug!("ui_bind_confirm: swap {mv:?} parked at {index} on record {entry}");
        wait_for(entry, RecordCallback::Swap, items, commands);
    }

    /// `AutoEquipCursorItem` on an uncached record: the lookup asked it with the callback
    /// `0x5e17a0` (`0x5e15f4`-`0x5e1603`), nothing filed.
    pub(crate) fn await_auto_equip(&mut self, entry: u32, items: &Items, commands: &NetCommands) {
        debug!("ui_bind_confirm: auto-equip waits on record {entry}");
        wait_for(entry, RecordCallback::AutoEquip, items, commands);
    }

    /// Files the deferred use and raises `USE_BIND`, with no argument: a second deferred use
    /// replaces the first.
    pub(crate) fn defer_use(&mut self, script: &mut UiScript, pending: PendingUse) {
        debug!(
            "ui_bind_confirm: USE_BIND_CONFIRM for wire {}/{}",
            pending.bag_index, pending.slot
        );
        self.on_use.0 = Some(pending);
        // In place, inside the use: `CGItem::Use` stamps both guids, signals event 0x122 and
        // returns (`0x5d91fa`-`0x5d9222`).
        script.fire_event("USE_BIND_CONFIRM", vec![]);
    }
}

/// The lookup's miss (`0x55ba30`): ask the record with the callback, which its answer runs. A
/// record the cache holds never comes here: the call acts on it in its body (`0x5e163b`,
/// [`crate::ui_items::ItemSteps`]). A refused entry holds no record, its node destroyed with the
/// refusal (`0x55bed2`), so its lookup waits and asks again.
fn wait_for(entry: u32, callback: RecordCallback, items: &Items, commands: &NetCommands) {
    items.wait(entry, callback);
    let _ = items.template(entry, 0, Ask::Event(commands));
}

/// The VM, the item cache and the pending-equip table an equip callback runs on.
type ReplayVm<'w, 's> =
    crate::script_calls::VmWith<'w, 's, (Res<'static, Items>, ResMut<'static, PendingEquips>)>;

/// An equip callback for the record of `entry`, with the VM and the host lent, so its send goes
/// out inside it. `0x5e1020` walks the whole table, every element whose entry field matches, a
/// standing question's included: each replays when the record was found (`0x5e10c2`) and is
/// freed either way (`0x5e10cf`-`0x5e10e5`), a refusal leaving its items locked. This replays and
/// frees the swaps parked on the entry alone ([`PendingEquips::parked_on`]). `0x5e17a0` runs
/// `AutoEquipCursorItem` again for a found record ([`UiScript::replay_auto_equip`]).
fn replay(vm: &mut ReplayVm, entry: u32, callback: RecordCallback, found: bool) {
    match callback {
        RecordCallback::AutoEquip if found => vm.run(|script| {
            if let Some(Err(e)) = script.map(|s| s.replay_auto_equip(entry)) {
                debug!("ui_bind_confirm: auto-equip replay on {entry} raised: {e}");
            }
        }),
        RecordCallback::Swap => {
            let parked = vm.own().1.parked_on(entry);
            for (index, mv) in parked {
                if found {
                    vm.run(|script| {
                        if let Some(Err(e)) = script.map(|s| s.replay_swap(mv)) {
                            debug!("ui_bind_confirm: swap replay on {entry} raised: {e}");
                        }
                    });
                }
                vm.own().1.take(index);
            }
        }
        _ => {}
    }
}

/// The equip callback the record's answer runs inside its handler (`ItemCacheReceive`
/// `0x55bdb0`, `0x55bf39`, from `0x555140`), in its place among the entry's other callbacks
/// ([`crate::net::objects`]), once the VM's copy holds the record
/// ([`crate::ui_items::feed::push_answered_record`]): its send goes out at the packet, ahead of
/// a later packet's, and what it signals is parked at the packet's place among the frame's events
/// ([`crate::field_callbacks::Parked::split`]).
pub(crate) fn replay_at_answer(
    world: &mut World,
    entry: u32,
    callback: RecordCallback,
    found: bool,
) {
    fn run(In((entry, callback, found)): In<(u32, RecordCallback, bool)>, mut vm: ReplayVm) {
        replay(&mut vm, entry, callback, found);
    }
    let mut parked = crate::field_callbacks::Parked::split(world);
    parked.leg(world, |world| {
        if let Err(err) = world.run_system_cached_with(run, (entry, callback, found)) {
            debug!("ui_bind_confirm: the equip callback did not run: {err}");
        }
        crate::creature_anim::apply_unit_edits(world);
    });
}
