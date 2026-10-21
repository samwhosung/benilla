//! The loot window's reference functions, shared by the verbs' calls and the feed's turns
//! ([`LootSteps`]): the take dispatcher `0x4c2790`, `EndLootWindow 0x48f200` with the VM at hand,
//! the auto-loot sweep `0x4c1fa0` and the master-loot give `0x4c2940`. A function that signals goes
//! on once its listeners have returned (`SignalEvent` runs them in place), so what it does after
//! the signal is a [`Then`], run by the call's tail ([`UiScript::resume_loot`]) or by the feed's
//! next turn ([`super::run_then`]): the listeners read the state as the signal left it.

use benilla_protocol::messages::slot_type;
use benilla_ui::script::{CallTail, LootCall, ScriptValue, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{
    bind_confirm_required, close_interaction, show, unlock_closed_source, AutoLootByte,
    DeadUnitDeselect, LootAction, LootLatch, LootState,
};
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_party::GroupState;

/// What a loot function does after its signal's listeners have returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Then {
    /// The take's stash takes the row it signalled `LOOT_BIND_CONFIRM` for (`0x4c2917`).
    StoreBind,
    /// The sweep after its `LOOT_BIND_CONFIRM` (`0x4c21dc` on): the stash, the rows after the
    /// confirmed one, and its end, with the sweep's flag.
    SweepRest { flag: bool },
    /// The coin click after its `LOOT_SLOT_CLEARED` (`0x4c286a`-`0x4c287c`): the close with
    /// release when nothing is left.
    CloseIfEmpty,
    /// A close's teardown after its unlocks' signals: the window goes and `LOOT_CLOSED`
    /// (`0x4c1dac`).
    Announce,
}

/// The loot window's state and what its functions send and signal through: the engine half of
/// the loot calls ([`crate::script_calls::ScriptCallHost`]) and of the feed's and the drain's
/// turns.
#[derive(SystemParam)]
pub(crate) struct LootSteps<'w, 's> {
    pub(super) loot: ResMut<'w, LootState>,
    pub(super) latch: ResMut<'w, LootLatch>,
    pub(super) net: Res<'w, NetCommands>,
    pub(super) dead: DeadUnitDeselect<'w, 's>,
    pub(super) objects: crate::net::Objects<'w, 's>,
    pub(super) byte: ResMut<'w, AutoLootByte>,
    items: Res<'w, Items>,
    group: Res<'w, GroupState>,
    pickup: MessageWriter<'w, crate::sound::LootPickupSound>,
    errors: Res<'w, UiErrorKeys>,
}

impl LootSteps<'_, '_> {
    /// Apply `call` inside its verb. What its signals' listeners leave it to do runs in its tail,
    /// a call of its own once they have returned ([`LootCall::Resume`]).
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: LootCall) -> Option<CallTail> {
        let mut owed = self.loot.then.len();
        match call {
            LootCall::Take(row) => self.take(script, row),
            LootCall::Confirm(row) => self.confirm(row),
            LootCall::Close { failed } => self.end_window(script, failed),
            LootCall::GiveMaster { slot, candidate } => self.give_master(slot, candidate),
            LootCall::Resume => {
                if let Some(then) = self.loot.then.pop() {
                    owed -= 1;
                    self.then(script, then);
                }
            }
        }
        (self.loot.then.len() > owed).then(|| Box::new(UiScript::resume_loot) as CallTail)
    }

    /// `0x4c2790(slot, 0)`, a row's own click (`0x4c1867`), on the window's cells as they stand,
    /// whether a window is open or not: the dispatcher asks for no source.
    fn take(&mut self, script: &mut UiScript, row: u32) {
        match self.loot.window.action_at(row) {
            Some(LootAction::Money) => {
                // `CMSG_LOOT_MONEY` while the player's loot target is held (`0x4c2847` →
                // `0x5ea8c6`). The coin sound is the coinage watcher's (`crate::sound`).
                if self.latch.0.is_some() {
                    let _ = self.net.0.send(ClientCommand::LootMoney);
                }
                // The money cell to -1 (`0x4c2858`) and `LOOT_SLOT_CLEARED` 1 (`0x4c2862`), whose
                // listeners read the coin row gone.
                self.loot.take_coin();
                if let Some(mut shown) = script.shown_loot() {
                    if shown.has_money {
                        if let Some(coin) = shown.rows.first_mut() {
                            *coin = None;
                        }
                    }
                    script.set_loot(Some(shown));
                }
                script.fire_event("LOOT_SLOT_CLEARED", vec![ScriptValue::Int(1)]);
                self.loot.then.push(Then::CloseIfEmpty);
            }
            // The wire's roll-pending row refuses (`0x4c28ae`-`0x4c28b8`).
            Some(LootAction::Item {
                slot_type: slot_type::ROLL_ONGOING,
                ..
            }) => self.errors.push(UiError::key("ERR_LOOT_ROLL_PENDING")),
            // A master-loot row opens the candidate list (`0x4c28c4`-`0x4c28d1`), whose anchor the
            // row's `<OnClick>` has already stashed.
            Some(LootAction::Item {
                slot_type: slot_type::MASTER,
                ..
            }) => script.fire_event("OPEN_MASTER_LOOT_LIST", vec![]),
            Some(LootAction::Item {
                wire_slot,
                display_id,
                item_id,
                ..
            }) => {
                // Bind on pickup and uncommon or better (`0x4c28f2`, `0x4c28fb`): the confirm,
                // whose listeners run before the stash takes the row (`0x4c290f`, `0x4c2917`).
                if bind_confirm_required(&self.items, item_id) {
                    script.fire_event("LOOT_BIND_CONFIRM", vec![ScriptValue::Int(i64::from(row))]);
                    self.loot.owed_bind_confirm = Some(row);
                    self.loot.then.push(Then::StoreBind);
                    return;
                }
                self.store(wire_slot, display_id);
            }
            None => {}
        }
    }

    /// The item's sound and `CMSG_AUTOSTORE_LOOT_ITEM` with its wire slot (`0x4c2926`,
    /// `0x4c2936`), before any answer.
    fn store(&mut self, wire_slot: u8, display_id: u32) {
        self.pickup
            .write(crate::sound::LootPickupSound { display_id });
        let _ = self
            .net
            .0
            .send(ClientCommand::AutostoreLootItem { slot: wire_slot });
    }

    /// `0x4c2790(slot, 1)`, `LootSlot`: only the pending row (`0x4c27c0`), and only while it holds
    /// an item (`0x4c27d9`); the send clears the stash (`0x4c281a`), so a doubled accept cannot
    /// loot twice.
    fn confirm(&mut self, row: u32) {
        if self.loot.pending_bind_confirm != Some(row) {
            return;
        }
        let Some(LootAction::Item {
            wire_slot,
            display_id,
            ..
        }) = self.loot.window.action_at(row)
        else {
            return;
        };
        self.store(wire_slot, display_id);
        self.loot.pending_bind_confirm = None;
    }

    /// `EndLootWindow 0x48f200(1, 0, failed)` with the VM at hand, `CloseLoot`'s and an emptied
    /// window's: nothing with no window (`0x48f23f`); a disenchant window with rows left is swept
    /// with the flag (`0x48f24f`, `0x48f264`); else `ERR_LOOT_NO_UI` for a set flag (`0x48f27b`),
    /// the release and the teardown, whose unlocks signal before the window goes
    /// ([`Then::Announce`]).
    pub(super) fn end_window(&mut self, script: &mut UiScript, failed: bool) {
        if self.loot.source().is_none() {
            return;
        }
        if self.loot.window.loot_type == benilla_protocol::messages::loot_type::DISENCHANTING
            && self.loot.has_contents(&self.objects)
        {
            self.sweep(script, failed);
            return;
        }
        if failed {
            self.errors.push(UiError::key("ERR_LOOT_NO_UI"));
        }
        let closed = close_interaction(&mut self.loot, &mut self.latch, Some(&self.net));
        unlock_closed_source(script, &mut self.loot, &self.objects);
        self.dead.after_close(closed);
        self.loot.then.push(Then::Announce);
    }

    /// `0x4c2940`: nothing with no window (`0x4c2953`) or for a row that is no master row
    /// (`0x4c298e`); else `CMSG_LOOT_MASTER_GIVE` with the row's wire slot and the candidate.
    fn give_master(&mut self, slot: u32, candidate: u32) {
        let window = &self.loot.window;
        let Some(guid) = window.source else {
            return;
        };
        let (
            Some(LootAction::Item {
                wire_slot,
                slot_type,
                ..
            }),
            Some(target),
        ) = (
            window.action_at(slot),
            window.master_candidate(candidate, &self.group),
        )
        else {
            debug!("ui_loot: GiveMasterLoot({slot}, {candidate}) unresolvable — ignored");
            return;
        };
        if slot_type != slot_type::MASTER {
            return;
        }
        debug!("ui_loot: master-give row {slot} (wire slot {wire_slot}) to {target:#x}");
        let _ = self.net.0.send(ClientCommand::LootMasterGive {
            guid,
            slot: wire_slot,
            target,
        });
    }

    /// The copier's end (`0x4c1ea8`-`0x4c1ee6`, the cache callback's `0x4c2afe`-`0x4c2b37`): the
    /// sweep while the auto-loot byte is set (`0x4c1ed3`, `0x4c2b29`), else `LOOT_OPENED`.
    pub(super) fn copier_end(&mut self, script: &mut UiScript) {
        if self.byte.0 {
            self.sweep(script, false);
        } else {
            script.fire_event("LOOT_OPENED", vec![]);
        }
    }

    /// `AutoLootAll 0x4c1fa0(flag)`: the coin request for a nonzero money cell, -1 included
    /// (`0x4c1ff4`), sent while the loot target is held (`0x5ea8c6`), then the rows. The coin
    /// sound and the money line (`0x4c2001`-`0x4c2170`) are not built.
    pub(super) fn sweep(&mut self, script: &mut UiScript, flag: bool) {
        if self.loot.window.coin_slot && self.latch.0.is_some() {
            let _ = self.net.0.send(ClientCommand::LootMoney);
        }
        self.sweep_rows(script, 1, false, flag);
    }

    /// The sweep's rows from display row `from` (`0x4c2180`-`0x4c220e`): each plain row
    /// (`0x4c2196`) sent as a click sends it, but a bind-on-pickup row of quality 2 or more, the
    /// first of which signals `LOOT_BIND_CONFIRM` (`0x4c21d4`), the sweep going on after its
    /// listeners ([`Then::SweepRest`]), and the later ones stay (`0x4c21c2`); then its end.
    fn sweep_rows(&mut self, script: &mut UiScript, from: u32, confirmed: bool, flag: bool) {
        for row in from..=self.loot.window.row_count() {
            let Some(LootAction::Item {
                wire_slot,
                display_id,
                item_id,
                slot_type: slot_type::ALLOW_LOOT,
            }) = self.loot.window.action_at(row)
            else {
                continue;
            };
            if bind_confirm_required(&self.items, item_id) {
                if confirmed {
                    continue;
                }
                script.fire_event("LOOT_BIND_CONFIRM", vec![ScriptValue::Int(i64::from(row))]);
                self.loot.owed_bind_confirm = Some(row);
                self.loot.then.push(Then::SweepRest { flag });
                return;
            }
            self.store(wire_slot, display_id);
        }
        self.sweep_end(script, flag);
    }

    /// The sweep's end: the auto-loot byte cleared (`0x4c2219`), then for a flag of 0
    /// `LOOT_OPENED` while anything is left (`0x4c2237`, `0x4c2240`), the rows it sent included,
    /// and with nothing left the close with release (`0x4c224f`).
    fn sweep_end(&mut self, script: &mut UiScript, flag: bool) {
        self.byte.0 = false;
        if self.loot.has_contents(&self.objects) {
            if !flag {
                script.fire_event("LOOT_OPENED", vec![]);
            }
        } else {
            self.end_window(script, false);
        }
    }

    /// Run what a function owed after its signal's listeners.
    pub(super) fn then(&mut self, script: &mut UiScript, then: Then) {
        match then {
            Then::StoreBind => {
                if let Some(row) = self.loot.owed_bind_confirm.take() {
                    self.loot.pending_bind_confirm = Some(row);
                }
            }
            Then::SweepRest { flag } => {
                let Some(row) = self.loot.owed_bind_confirm.take() else {
                    return;
                };
                self.loot.pending_bind_confirm = Some(row); // `0x4c21dc`
                self.sweep_rows(script, row + 1, true, flag);
            }
            Then::CloseIfEmpty => {
                if !self.loot.has_contents(&self.objects) {
                    self.end_window(script, false);
                }
            }
            Then::Announce => show(script, None),
        }
    }
}
