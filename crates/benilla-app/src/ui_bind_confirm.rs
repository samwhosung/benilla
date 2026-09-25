//! The **equip / auto-equip / use soulbind confirmations** — `EQUIP_BIND`, `AUTOEQUIP_BIND` and
//! `USE_BIND`. The loot arm's siblings, and NOT built the same way, because the
//! reference does not build them the same way either.
//!
//! ## The three predicates are three different predicates
//!
//! In the 1.12.1 client, one `item_template + 0x194` compare per arm and **no `+0x1c` (quality)
//! read in any of the three**:
//!
//! | arm | event | fires from | predicate | site |
//! |---|---|---|---|---|
//! | loot | `LOOT_BIND_CONFIRM` 287 | `0x4c2790` | `bonding == 1` **and** `quality >= 2` | `4c28f2`/`4c28fb` |
//! | equip | `EQUIP_BIND_CONFIRM` 288 | `0x5e0c40` `SwapItem` | `bonding == 2` | `5e0e54` |
//! | auto-equip | `AUTOEQUIP_BIND_CONFIRM` 289 | `0x5e1480` `AutoEquipCursorItem` | `bonding == 2` | `5e163b` |
//! | use | `USE_BIND_CONFIRM` 290 | `0x5d8d00` `CGItem::Use` | `bonding == 3` | `5d91d6` |
//!
//! Carrying the loot arm's `quality >= 2` across would have been wrong twice over: it would have
//! silenced the confirm on a white BoE, and it would have asked about the wrong `bonding` value
//! entirely. (This was benilla's working assumption until the reference refuted it; that is why the
//! table is here and not a sentence.)
//!
//! ## The other conjuncts, and where benilla already had them
//!
//! The equip arms' full set is: container and item resolve · the item is **not already soulbound**
//! · the call is not itself a suppressed re-issue · the item template is **cached** · `bonding == 2`
//! · the player **can equip it**. Both of the interesting ones were already in this codebase, byte-
//! derived for other reasons and reused here rather than re-written:
//!
//! - *not already soulbound* is [`crate::items::already_bound`] — literally `0x5da2c0`, built for
//!   the enchant cursor's own bind question and the tooltip's Soulbound override
//!   (B310/1562). The reference calls the same function from all three places.
//! - *can equip it* is `0x5ea930`, [`benilla_ui::script::UiScript::item_usable`] — built for the
//!   merchant's red rows.
//!
//! ## Accept is a re-issue, not a confirm packet
//!
//! There is no `CMSG_CONFIRM_*` in 1.12. `EquipPendingItem(index)` re-runs the *original action*
//! with the suppress flag set, which is why [`PendingEquip`] stores the action's own coordinates
//! and the accept path calls the very same sender the click did — `suppress` threaded through as a
//! real parameter, exactly as `0x5e0c40`/`0x5e1480` take it. `CancelPendingEquip(index)` sends
//! nothing and only drops the record.
//!
//! **A superseded record is CANCELLED, not overwritten** — the entries are `exclusive = 1`, so
//! `StaticPopup_Show` hides the standing dialog and runs its `OnHide`, which is
//! `CancelPendingEquip(slot)`. That is why both `OnCancel` and `OnHide` name it, and why this store
//! is an index-addressed array of optional records rather than a single cell: two questions can be
//! live in principle, and the second one's arrival is what retires the first.
//!
//! ## The index space is the reference's own
//!
//! `arg1` is a **0-based index into the pending array**, not a slot (allocated by `0x5e1110`).
//! It is opaque on both sides — the event hands it out, `dialog.data` carries it,
//! and the two verbs hand it straight back — so unlike the loot arm's row number (1744, translated
//! into benilla's display space) there is nothing to gain by re-basing it, and it is kept as-is.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::items::{Enchants, Items};
use crate::net::Objects;

/// `item_template.bonding` — the values the three arms compare against (vmangos
/// `ItemPrototype.h`'s `ItemBondingType`; the client reads the same field at `+0x194`).
pub(crate) const BIND_WHEN_EQUIPPED: u32 = 2;
/// The use arm's value (`0x5d91d6`).
pub(crate) const BIND_WHEN_USE: u32 = 3;

/// One deferred action, held until the player answers. Stored as the action's own **Lua-space**
/// coordinates rather than as a built packet, because accept is a re-issue: the same sender runs
/// again with `suppress` set and re-reads the world, so a slot that changed under the open dialog
/// is re-judged instead of being sent stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingEquip {
    /// Arm 288 — a placement into an equipment/bag/bank-bag slot (`EQUIP_BIND_CONFIRM`), in
    /// **Lua** space. Lua, because the re-issue goes back through the move drain's own sender,
    /// which re-derives the wire pair AND takes the pending-op lock — one code path for the
    /// deferred and the undeferred swap rather than two that must be kept agreeing.
    Swap {
        src_bag: i64,
        src_slot: u32,
        dst_bag: i64,
        dst_slot: u32,
    },
    /// Arm 289 — an auto-equip, in **wire** space plus the item's guid. Wire, because one of the
    /// three auto-equip senders is the action bar's, which resolves an item *entry* to a position
    /// and so has no Lua pair to re-derive from; the guid, so the re-issue can re-take the ammo
    /// fork off the item's own template exactly as the first attempt did.
    AutoEquip { bag_index: u8, slot: u8, guid: u64 },
}

/// The client's pending-equip array (`0xc4c290`-`0xc4c29c`, stride `0x20`), modelled as what it
/// is: an index-addressed store whose free elements are reused. The reference's "free ⇔ the src
/// container guid is 0" is our `None`; its grow-on-demand is our `push`.
///
/// It is deliberately NOT reset on world enter/leave, matching the reference (whose array is zeroed
/// once at CRT static-init and freed at shutdown) — a dialog cannot outlive its session anyway,
/// because the UI is torn down with it.
#[derive(Resource, Default)]
pub(crate) struct PendingEquips {
    slots: Vec<Option<PendingEquip>>,
}

impl PendingEquips {
    /// File a record into the first free index, growing only when none is free — the reference's
    /// own allocator shape. Returns the index the event carries out.
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

    /// Take the record at `index`, freeing the element. `None` for an index nobody filed — which is
    /// the reference's own answer too (`0x5e1be0` bounds-checks against the live element count and
    /// returns), so a stray `EquipPendingItem(99)` from an addon does nothing.
    pub(crate) fn take(&mut self, index: u32) -> Option<PendingEquip> {
        self.slots.get_mut(index as usize)?.take()
    }

    /// How many records are live — the step-back check that nothing leaks (a cancelled or accepted
    /// question must free its element).
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        self.slots.iter().flatten().count()
    }
}

/// Arm 290's pending state. The reference does **not** use the array here: it stamps two globals at
/// the fire site (the item guid `0xc4c240`, the target guid `0xc4c1d0`) and `ConfirmBindOnUse()`
/// re-issues `CGItem::Use(&target, suppress = 1)` off them. One cell, no index, and no cancel verb
/// at all — declining just drops the question.
#[derive(Resource, Default)]
pub(crate) struct PendingBindOnUse(pub(crate) Option<PendingUse>);

/// The deferred use — the *original action*, stored verbatim, because that is what the accept
/// re-issues. The reference stamps the item guid and the target guid and re-runs `CGItem::Use` off
/// them; [`crate::ui_items::ItemUse`] is benilla's whole equivalent of that pair of arguments, and
/// it is `Copy`, so the record is literally the call that was about to be made.
pub(crate) type PendingUse = crate::ui_items::ItemUse;

/// The three resources the deferral needs, bundled so a drain can take them in one parameter
/// (the shape `crate::spell::CastLadder` established). [`Items`] is deliberately NOT in here:
/// every caller already holds it, and a second `ResMut<Items>` in one system is a conflict.
#[derive(SystemParam)]
pub(crate) struct BindGate<'w> {
    pub(crate) equips: ResMut<'w, PendingEquips>,
    pub(crate) on_use: ResMut<'w, PendingBindOnUse>,
    /// `already_bound`'s enchant catalog — an enchant can soulbind the item it lands on, and the
    /// reference's `0x5da2c0` walks all seven slots. Absent (no client data) degrades to the
    /// instance flag alone, which is the same posture every other consumer takes.
    pub(crate) enchants: Option<Res<'w, Enchants>>,
}

impl BindGate<'_> {
    /// Does taking this equip action have to ask first? `item_guid` is the item that would BIND —
    /// for a swap that is the occupant of the non-equip side, for an auto-equip the clicked slot.
    ///
    /// `false` on every "we cannot tell" (no such item, no template yet): the reference's own
    /// cache-**hit** conjunct fails open the same way, and a miss there re-runs the whole decision
    /// when the template lands rather than asking about an item it cannot name.
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

    /// Arm 290's predicate, VERIFIED as an **earned census**: `[0x5d91d3, 0x5d91f2)` is 31
    /// contiguous bytes holding exactly three `jcc` — `5d91dd` (`bonding == 3`), `5d91e9`
    /// (`0x5da2c0` says not already bound) and `5d91f0` (`suppress == 0`) — all three jumping to
    /// the cast, with no other branch and no other call. Nothing narrower, nothing wider.
    ///
    /// The `0x5ea930` (can-use) leg is genuinely absent and is not an omission: a `0x5ea930`
    /// failure is one of the rungs that **exits** `0x5d8d00` ahead of the bind arm, so by the time
    /// the fire site is reached the item has already passed it.
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

    /// File a deferred equip and raise its dialog. `auto` picks the event, which is the only thing
    /// that differs between arms 288 and 289 on this side — `UIParent.lua:324-339` shows the two
    /// branches are otherwise identical, down to each hiding the other's dialog first.
    pub(crate) fn defer_equip(&mut self, script: &mut UiScript, rec: PendingEquip) {
        let index = self.equips.add(rec);
        let event = match rec {
            PendingEquip::Swap { .. } => "EQUIP_BIND_CONFIRM",
            PendingEquip::AutoEquip { .. } => "AUTOEQUIP_BIND_CONFIRM",
        };
        debug!("ui_bind_confirm: {event} index {index} for {rec:?}");
        // QUEUED, not fired (see `UiScript::queue_event`): the place that triggered this deferral
        // has already queued its own `CURSOR_UPDATE`, and `UIParent.lua:356-360` hides both equip
        // dialogs on it. Firing immediately would put the question AHEAD of that cursor change and
        // the next tick would cancel it — proven by a test, not reasoned about.
        script.queue_event(event, vec![ScriptValue::Int(i64::from(index))]);
    }

    /// File the deferred use and raise `USE_BIND`. No index and no argument: arm 290's state is one
    /// cell, and a second deferred use simply replaces the first (there is no cancel verb to
    /// retire it with, which is the reference's own shape).
    pub(crate) fn defer_use(&mut self, script: &mut UiScript, pending: PendingUse) {
        debug!(
            "ui_bind_confirm: USE_BIND_CONFIRM for wire {}/{}",
            pending.bag_index, pending.slot
        );
        self.on_use.0 = Some(pending);
        // Queued for the same reason as the equip arms, though `USE_BIND` is not on the
        // CURSOR_UPDATE arm: one rule for the three of them beats one rule and an exception.
        script.queue_event("USE_BIND_CONFIRM", vec![]);
    }
}
