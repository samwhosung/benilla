//! The **item soulbind confirmations** other than the loot one — the Era API surface behind
//! `EQUIP_BIND` / `AUTOEQUIP_BIND` / `USE_BIND` (decision 1750). Three globals, no snapshot: the
//! question arrives as an event and the answer goes straight back out, exactly like
//! [`super::binder`] and [`super::duel`].
//!
//! ## What the three verbs mean, at the 1.12 bytes
//!
//! `EquipPendingItem(index)` `0x4898f0` and `CancelPendingEquip(index)` `0x489960` both delegate to
//! `0x5e1be0(index, accept)`. **`index` is not a slot** — it is a 0-based index into the client's
//! own growable pending-equip array (capacity `0xc4c290`, count `0xc4c294`, base `0xc4c298`, stride
//! `0x20`), which is also what `EQUIP_BIND_CONFIRM`'s and `AUTOEQUIP_BIND_CONFIRM`'s `arg1` carries.
//! `UIParent.lua:324-339` hangs it on `dialog.data` and the entries hand it straight back, so the
//! number is opaque on both sides — which is why benilla keeps the reference's index space here
//! instead of translating it the way the loot arm's row number is translated (1744).
//!
//! **Accept is a re-issue of the original action with a suppress flag, not a confirm packet** —
//! there is no `CMSG_CONFIRM_*` in 1.12. Cancel sends nothing at all; it only releases the item
//! locks the deferred action took (`UnlockItem 0x495420`).
//!
//! `ConfirmBindOnUse()` `0x48d770` is the use arm's answer and takes **no** argument, because that
//! arm's pending state is not the array: it is two globals the fire site stamps (the item guid at
//! `0xc4c240`, the target guid at `0xc4c1d0`), and the accept is `CGItem::Use(&target, suppress=1)`.
//! `USE_BIND_CONFIRM` carries no arguments either, and the reference ships **no** cancel binding for
//! it — declining simply drops the question.
//!
//! All three decisions are **client-local**: the client reads the cached item template's `bonding`
//! and defers its own send. No packet raises any of them.
//!
//! The app owns the pending records, so this module holds only the intents.

use mlua::Lua;

use super::Model;

/// One answer to a pending-equip question: the 0-based array index the event handed out, and
/// whether the player accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingEquipAnswer {
    /// The index `EQUIP_BIND_CONFIRM`/`AUTOEQUIP_BIND_CONFIRM` carried out on `arg1`.
    pub index: u32,
    /// `true` from `EquipPendingItem` (re-issue the action), `false` from `CancelPendingEquip`
    /// (release the locks, send nothing).
    pub accept: bool,
}

impl super::UiScript {
    /// Drain the `EquipPendingItem`/`CancelPendingEquip` answers queued since the last drain, in
    /// call order. An index the app is not holding a record for is the app's to ignore — the
    /// reference's `0x5e1be0` bounds-checks against the live element count and returns.
    pub fn take_pending_equip_answers(&mut self) -> Vec<PendingEquipAnswer> {
        std::mem::take(&mut self.model_mut().pending_equip_answers)
    }

    /// Drain the `ConfirmBindOnUse()` calls queued since the last drain — a count, because the verb
    /// has no payload ([`super::UiScript::take_binder_confirms`]'s shape and its reason).
    pub fn take_bind_on_use_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().bind_on_use_confirms)
    }

    /// The client's item-usable predicate `0x5ea930` for an item known by entry — one of the two
    /// non-trivial conjuncts on the equip arms' deferral (an item the player *cannot* equip is
    /// never asked about, because the action it would confirm is one the server would refuse).
    ///
    /// Public because the deferral is decided app-side, where the send is: the engine holds the
    /// templates and the player's requirement state, so the question has to be asked here.
    /// Same conventions as every other caller ([`super::item_stats::item_usable_by_id`]): entry `0`
    /// and an unanswered template are both usable.
    pub fn item_usable(&self, item_id: u32) -> bool {
        super::item_stats::item_usable_by_id(&self.model_ref(), item_id)
    }
}

/// Register the three globals (the style [`super::binder`] registers its two).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // EquipPendingItem(index) — `StaticPopupDialogs["EQUIP_BIND"].OnAccept` and its AUTOEQUIP twin
    // (`StaticPopup.lua:612-647`). Re-issues the deferred action; the app owns what that action was.
    g.set(
        "EquipPendingItem",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pending_equip_answers.push(PendingEquipAnswer {
                index,
                accept: true,
            });
            Ok(())
        })?,
    )?;

    // CancelPendingEquip(index) — the same two entries' `OnCancel` AND their `OnHide`, which is not
    // redundancy: `exclusive = 1` means a second bind question hides the standing dialog, and the
    // reference relies on that `OnHide` to cancel the record it supersedes. A superseded pending
    // equip is CANCELLED, never silently overwritten — without it the old item's locks leak.
    g.set(
        "CancelPendingEquip",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pending_equip_answers.push(PendingEquipAnswer {
                index,
                accept: false,
            });
            Ok(())
        })?,
    )?;

    // ConfirmBindOnUse() — `StaticPopupDialogs["USE_BIND"].OnAccept` (`StaticPopup.lua:648-658`).
    // No argument and no cancel twin: the use arm's pending state is a single client global, and
    // declining just drops it.
    g.set(
        "ConfirmBindOnUse",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.bind_on_use_confirms += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// The two equip verbs share one queue and are distinguished only by their answer — the app
    /// needs the call ORDER as well as the verdicts, because a supersede is a cancel of one record
    /// arriving between two others.
    #[test]
    fn the_equip_answers_queue_in_call_order_with_their_verdicts() {
        let mut s = UiScript::new().unwrap();
        s.run("EquipPendingItem(0) CancelPendingEquip(1) EquipPendingItem(2)")
            .unwrap();
        let answers = s.take_pending_equip_answers();
        assert_eq!(
            answers
                .iter()
                .map(|a| (a.index, a.accept))
                .collect::<Vec<_>>(),
            vec![(0, true), (1, false), (2, true)]
        );
        assert!(s.take_pending_equip_answers().is_empty(), "drained");
    }

    /// `ConfirmBindOnUse` is a count, like `ConfirmBinder`: the verb carries no payload because the
    /// pending item is the app's.
    #[test]
    fn confirm_bind_on_use_counts() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.take_bind_on_use_confirms(), 0);
        s.run("ConfirmBindOnUse() ConfirmBindOnUse()").unwrap();
        assert_eq!(s.take_bind_on_use_confirms(), 2);
        assert_eq!(s.take_bind_on_use_confirms(), 0, "drained");
    }
}
