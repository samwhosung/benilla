//! The loot window's reference functions, shared by the verbs' calls and the loot lane's turns
//! ([`LootSteps`]): the take dispatcher `0x4c2790`, `EndLootWindow 0x48f200` with the VM at hand,
//! the auto-loot sweep `0x4c1fa0`, the interactions' `SetAutoLoot 0x5df460`, `OpenItem
//! 0x5edc80`'s send and the master-loot give `0x4c2940`. A function that signals goes
//! on once its listeners have returned (`SignalEvent` runs them in place), so what it does after
//! the signal is a [`Then`], run by the call's tail ([`UiScript::resume_loot`]) or by the lane's
//! next turn ([`super::run_then`]): the listeners read the state as the signal left it.

use benilla_protocol::messages::{loot_type, slot_type};
use benilla_ui::script::{CallTail, LootCall, ScriptValue, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{
    bind_confirm_required, close_interaction_in_place, unlock_closed_source, AutoLootByte,
    DeadUnitDeselect, LootAction, LootConfig, LootLatch, LootState,
};
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_party::GroupState;

/// `OpenItem`'s item (`0x5edc80`): its guid, which becomes the loot target (`0x5edcc0`), and its
/// wire position for `CMSG_OPEN_ITEM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpenItem {
    pub(crate) guid: u64,
    pub(crate) bag_index: u8,
    pub(crate) slot: u8,
}

/// The byte an interaction passes `SetAutoLoot`: whether Shift is down (`EventIsKeyDown(0)`,
/// `0x492879`, `0x5edca7`), inverted by `autoLootDefault` ([`LootConfig::auto_loot`]), which is
/// not a 1.12 CVar and off by default, leaving Shift alone.
pub(crate) fn interaction_byte(cfg: &LootConfig, shift: bool) -> bool {
    cfg.auto_loot != shift
}

/// What a loot function does after its signal's listeners have returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Then {
    /// The take's stash takes the row it signalled `LOOT_BIND_CONFIRM` for (`0x4c2917`), its own
    /// whatever a nested function signalled meanwhile.
    StoreBind { row: u32 },
    /// The sweep after its `LOOT_BIND_CONFIRM` for `row` (`0x4c21dc` on): the stash, the rows
    /// after it, and its end, with the sweep's flag.
    SweepRest { row: u32, flag: bool },
    /// The sweep's rows and end after its money line's `CHAT_MSG_MONEY` (`0x4c2170` →
    /// `AddChatLine 0x49a870`, then `0x4c2180`), with the sweep's flag.
    SweepRows { flag: bool },
    /// The coin click after its `LOOT_SLOT_CLEARED` (`0x4c286a`-`0x4c287c`): the close with
    /// release when nothing is left.
    CloseIfEmpty,
    /// A close's two unlocks (`0x48f2bd`, `0x4c1d36`), after the line its flag shows
    /// (`0x48f27b`).
    Unlocks,
    /// A close's teardown after its unlocks' signals: the source zeroed and `LOOT_CLOSED`
    /// (`0x4c1da0`, `0x4c1dac`).
    Announce,
    /// A close's last step after its window went: a dead unit's deselect (`0x48f369`).
    Deselect(u64),
    /// `OpenItem` past its `SetAutoLoot` (`0x5edcbb` on), once a sweep that made has run.
    OpenItem(OpenItem),
}

/// The loot window's state and what its functions send and signal through: the engine half of
/// the loot calls ([`crate::script_calls::ScriptCallHost`]) and of the lane's and the drain's
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
    /// The name cache a candidate's lookup asks; absent in a harness without it.
    names: Option<Res<'w, crate::names::NameCache>>,
    chat: Res<'w, crate::ui_chat::ChatLog>,
    cfg: Res<'w, LootConfig>,
}

impl LootSteps<'_, '_> {
    /// A probe line's stand state change's loot close, `0x48f200(1, 1, 0)` at `0x5f07d5`, with
    /// nobody to signal between its steps: the movement start's close ([`super::LootMoveStart`]),
    /// its release sent now.
    pub(crate) fn close_for_stand(&mut self) {
        let closed = super::close_on_move_start(&mut self.loot, &mut self.latch, &self.net);
        self.dead.after_close(closed);
    }

    /// A stand state change's loot close inside its call, `0x48f200(1, 1, 0)` at `0x5f07d5`: what
    /// `dl` keeps stays ([`super::move_start_keeps`]), anything else closes as `CloseLoot` closes
    /// it, its release now and the rest owed past `depth`, the depth returned, for the caller to
    /// run step by step ([`Self::then_past`]) ahead of the stand's attack stop (`0x5f07e7`).
    pub(crate) fn close_for_stand_here(&mut self, script: &mut UiScript) -> usize {
        let depth = self.loot.then.len();
        if let Some(source) = self.loot.source() {
            if !super::move_start_keeps(&self.loot, source) {
                self.end_window(script, false);
            }
        }
        depth
    }

    /// The next step owed past `depth` ([`Self::then`]), as [`LootCall::Resume`] runs it, for the
    /// caller to run its signals' listeners before the next: whether one ran.
    pub(crate) fn then_past(&mut self, script: &mut UiScript, depth: usize) -> bool {
        if self.loot.then.len() <= depth {
            return false;
        }
        let Some(then) = self.loot.then.pop() else {
            return false;
        };
        self.then(script, then);
        true
    }

    /// A movement start's close inside its call, `0x48f200(1, 1, 0)` at `0x60e9dd`: what `dl`
    /// keeps stays ([`super::move_start_keeps`]), anything else closes as `CloseLoot` closes it,
    /// its release now and `LOOT_CLOSED` in the call's tail, before the script's next line.
    pub(crate) fn close_for_move(&mut self, script: &mut UiScript) -> Option<CallTail> {
        let source = self.loot.source()?;
        if super::move_start_keeps(&self.loot, source) {
            return None;
        }
        let depth = self.loot.then.len();
        self.end_window(script, false);
        self.resume_tail(depth)
    }

    /// `GetMasterLootCandidate`'s lookup inside its call
    /// ([`benilla_ui::script::RecordAsk::MasterLootName`]): the name cache asks a miss, and the
    /// callback `0x4c2fb0` waits on the name, once per call (`0x4c2f61`). The getter makes it only on
    /// a miss, having read the cache first ([`benilla_ui::script::MasterLootCandidateQuery`]).
    pub(crate) fn wait_for_name(&mut self, guid: u64) {
        if let Some(names) = self.names.as_deref() {
            let _ = names.resolve_player(guid, crate::query_cache::Ask::Event(&self.net));
        }
        self.loot.wait_for_name(guid);
    }

    /// Apply `call` inside its verb. What its signals' listeners leave it to do runs in its tail,
    /// a call of its own once they have returned ([`LootCall::Resume`]).
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: LootCall) -> Option<CallTail> {
        let mut depth = self.loot.then.len();
        match call {
            LootCall::Take(row) => self.take(script, row),
            LootCall::Confirm(row) => self.confirm(row),
            LootCall::Close { failed } => self.end_window(script, failed),
            LootCall::GiveMaster { slot, candidate } => self.give_master(slot, candidate),
            LootCall::SetAutoLoot(byte) => self.set_auto_loot(script, byte),
            LootCall::Resume { depth: owed } => {
                depth = owed;
                if self.loot.then.len() > depth {
                    if let Some(then) = self.loot.then.pop() {
                        self.then(script, then);
                    }
                }
            }
        }
        self.resume_tail(depth)
    }

    /// The tail that runs what the call owes past `depth`, once its signals' listeners have run.
    fn resume_tail(&self, depth: usize) -> Option<CallTail> {
        (self.loot.then.len() > depth)
            .then(|| Box::new(move |script: &mut UiScript| script.resume_loot(depth)) as CallTail)
    }

    /// `SetAutoLoot 0x5df460(byte)`: the byte stored (`0x5df468`), and a nonzero one with the loot
    /// target held sweeps the open window at once (`0x5df470` → `0x5df480`).
    pub(super) fn set_auto_loot(&mut self, script: &mut UiScript, byte: bool) {
        self.byte.0 = byte;
        if byte && self.latch.0.is_some() {
            self.sweep(script, false);
        }
    }

    /// `OpenItem 0x5edc80` from its `SetAutoLoot` (`0x5edcb6`) on, the call having chosen the
    /// item: the byte from Shift as the call saw it, which with a window open sweeps it, then,
    /// once that sweep's listeners have run, the item held as the loot target, its lock and
    /// `CMSG_OPEN_ITEM` ([`Self::send_open_item`]).
    pub(crate) fn open_item(&mut self, script: &mut UiScript, open: OpenItem) -> Option<CallTail> {
        let owed = self.loot.then.len();
        self.loot.then.push(Then::OpenItem(open));
        let byte = interaction_byte(&self.cfg, script.shift_key_down());
        self.set_auto_loot(script, byte);
        if self.loot.then.len() == owed + 1 {
            self.loot.then.pop();
            self.send_open_item(script, open);
            return None;
        }
        self.resume_tail(owed)
    }

    /// `OpenItem`'s end: the item becomes the loot target (`0x5edcc0`), which the type-1 answer's
    /// admission asks for (`0x5ebb3f`), it locks (`0x5edcd9`; `LockItem` signals nothing for a
    /// guid that is no item object, `0x495407`), and `CMSG_OPEN_ITEM` goes out.
    fn send_open_item(&mut self, script: &mut UiScript, open: OpenItem) {
        debug!("ui_loot: open item {:#x}", open.guid);
        self.latch.0 = Some(open.guid);
        if self.objects.object(open.guid).is_some_and(|f| f.is_item()) {
            script.lock_item(open.guid);
        }
        let _ = self.net.0.send(ClientCommand::OpenItem {
            bag_index: open.bag_index,
            slot: open.slot,
        });
    }

    /// `0x4c2790(slot, 0)`, a row's own click (`0x4c1867`), on the window's cells as they stand,
    /// whether a window is open or not: the dispatcher asks for no source, only the active player
    /// (`0x4c27ad`, `0x4c27b6`).
    fn take(&mut self, script: &mut UiScript, row: u32) {
        if self.objects.player().is_none() {
            return;
        }
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
                    self.loot.then.push(Then::StoreBind { row });
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
        if self.objects.player().is_none() || self.loot.pending_bind_confirm != Some(row) {
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
    /// window's: nothing with no window (`0x48f23f`); a disenchant window with contents is swept
    /// with the flag (`0x48f24f`, `0x48f264`); else `ERR_LOOT_NO_UI` for a set flag (`0x48f27b`),
    /// the release and the teardown. The line, which a call signals as it returns, comes before
    /// the unlocks ([`Then::Unlocks`]), and those before the window goes ([`Then::Announce`]).
    pub(super) fn end_window(&mut self, script: &mut UiScript, failed: bool) {
        let Some(source) = self.loot.source() else {
            return;
        };
        if self.loot.window_closed_since() {
            // Its release went at the later packet: the close's signals alone, if still owed.
            if !self.loot.take_queued_close(source) {
                return;
            }
            if failed {
                self.errors.push(UiError::key("ERR_LOOT_NO_UI"));
            }
            self.loot.unlock_item_source(source);
            self.loot.unlock_item_source(source);
            self.loot.then.push(Then::Announce);
            self.loot.then.push(Then::Unlocks);
            return;
        }
        if self.loot.disenchant_hold(&self.objects) {
            self.sweep(script, failed);
            return;
        }
        if failed {
            self.errors.push(UiError::key("ERR_LOOT_NO_UI"));
        }
        let closed = close_interaction_in_place(&mut self.loot, &mut self.latch, Some(&self.net));
        // The window goes (`0x48f33d` → `0x4c1cb0`) before the dead unit's deselect (`0x48f369`).
        if let Some(source) = self.dead.dead_unit(closed) {
            self.loot.then.push(Then::Deselect(source));
        }
        self.loot.then.push(Then::Announce);
        self.loot.then.push(Then::Unlocks);
    }

    /// `GiveMasterLoot` (`0x4c2fd0`): the candidate at the 1-based index of the list packet's
    /// array (`0x61c660`), nothing for a zero one (`0x4c3021`); then `0x4c2940`: nothing with no
    /// window (`0x4c2953`) or for a row that is no master row (`0x4c298e`); else
    /// `CMSG_LOOT_MASTER_GIVE` with the row's wire slot and the candidate.
    fn give_master(&mut self, slot: u32, candidate: u32) {
        let target = candidate
            .checked_sub(1)
            .and_then(|i| self.loot.master_candidate(i as usize));
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
        ) = (window.action_at(slot), target)
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
    /// sweep while the auto-loot byte is set (`0x4c1ed3`, `0x4c2b29`), else `LOOT_OPENED`. With
    /// `autoLootDefault` on, a pickpocket or disenchant window, which no interaction stores the
    /// byte for, stores it here from the Shift held now ([`LootConfig`]'s deviation). The
    /// reference ends the copier inside its packet, its window open; a close a later packet's
    /// handler made has already released the window, which leaves nothing to sweep, so the end
    /// clears the byte as the sweep's would (`0x4c2219`) and signals `LOOT_OPENED`.
    pub(super) fn copier_end(&mut self, script: &mut UiScript) {
        if self.cfg.auto_loot
            && matches!(
                self.loot.window.loot_type,
                loot_type::PICKPOCKETING | loot_type::DISENCHANTING
            )
        {
            self.byte.0 = interaction_byte(&self.cfg, script.shift_key_down());
        }
        let open = self.loot.window.source.is_some() && !self.loot.window_closed_since();
        if self.byte.0 && open {
            self.sweep(script, false);
        } else {
            self.byte.0 = false;
            script.fire_event("LOOT_OPENED", vec![]);
        }
    }

    /// `AutoLootAll 0x4c1fa0(flag)`. Nothing while an item record is pending (`0x4c1fb3`), only
    /// `LOOT_OPENED` with no active player (`0x4c1fd9`, `0x4c1fe0`). Then for a nonzero money
    /// cell, -1 included (`0x4c1ff4`): the coin request, sent while the loot target is held
    /// (`0x4c1ffc` → `0x5ea8c6`), and once a session for money above 0 (`0x4c2008`, `0x4c2015`)
    /// the money line, which a party (`0x4c2030`) or a raid (`0x4c203d`) skips, added in place
    /// (`0x4c2170` → `AddChatLine 0x49a870`), whose listeners run before the rows
    /// ([`Then::SweepRows`]). Its early coin (`0x4c201b`-`0x4c2020`) is not played
    /// (`crate::sound`'s money module). Then the rows.
    pub(super) fn sweep(&mut self, script: &mut UiScript, flag: bool) {
        if self.loot.records_pending(&self.items) {
            return;
        }
        if self.objects.player().is_none() {
            script.fire_event("LOOT_OPENED", vec![]);
            return;
        }
        let window = &self.loot.window;
        if window.coin_slot {
            if self.latch.0.is_some() {
                let _ = self.net.0.send(ClientCommand::LootMoney);
            }
            let grouped = self.group.party_slots().next().is_some_and(|m| m.guid != 0)
                || self.group.raid_held();
            if !self.loot.coin_line_shown && window.has_coin() && !grouped {
                let line = crate::ui_chat::ChatEvent::text_only(
                    crate::ui_chat::ChatEventKind::Money,
                    money_line(script, window.gold),
                );
                self.chat.show_now(script, &line);
                self.loot.then.push(Then::SweepRows { flag });
                return;
            }
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
                self.loot.then.push(Then::SweepRest { row, flag });
                return;
            }
            self.store(wire_slot, display_id);
        }
        self.sweep_end(script, flag);
    }

    /// The sweep's end: the auto-loot byte cleared (`0x4c2219`) and the money line's flag set
    /// (`0x4c221e`), then for a flag of 0
    /// `LOOT_OPENED` while anything is left (`0x4c2237`, `0x4c2240`), the rows it sent included,
    /// and with nothing left the close with release (`0x4c224f`).
    fn sweep_end(&mut self, script: &mut UiScript, flag: bool) {
        self.byte.0 = false;
        self.loot.coin_line_shown = true; // `0x4c221e`
        if self.loot.sweep_has_contents(&self.objects) {
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
            Then::StoreBind { row } => self.loot.pending_bind_confirm = Some(row),
            Then::SweepRest { row, flag } => {
                self.loot.pending_bind_confirm = Some(row); // `0x4c21dc`
                self.sweep_rows(script, row + 1, true, flag);
            }
            Then::SweepRows { flag } => self.sweep_rows(script, 1, false, flag),
            Then::CloseIfEmpty => {
                if !self.loot.has_contents(&self.objects) {
                    self.end_window(script, false);
                }
            }
            Then::Unlocks => unlock_closed_source(script, &mut self.loot, &self.objects),
            Then::Announce => self.loot.tear_down_here(script),
            Then::Deselect(source) => self.dead.deselect(source),
            Then::OpenItem(open) => self.send_open_item(script, open),
        }
    }
}

/// The sweep's money line (`0x4c2043`-`0x4c2170`): `YOU_LOOT_MONEY` over the coins, each paid one
/// as `"%d %s"` with its `GOLD`, `SILVER` or `COPPER` global, gold first, joined by the
/// reference's own tests: `", "` after the gold when silver or copper is paid (`0x4c20e4`-
/// `0x4c20f0`), and before the copper when gold or silver is (`0x4c20bc`-`0x4c20c9`), so a zero
/// middle coin leaves `", , "`.
fn money_line(script: &UiScript, copper: u32) -> String {
    // `SplitCopper 0x6c6260`.
    let (g, s, c) = (copper / 10_000, copper / 100 % 100, copper % 100);
    let coin = |n: u32, key: &str| match n {
        0 => String::new(),
        n => format!("{n} {}", script.global_text(key)),
    };
    let after_gold = if g != 0 && (s != 0 || c != 0) {
        ", "
    } else {
        ""
    };
    let before_copper = if (g != 0 || s != 0) && c != 0 {
        ", "
    } else {
        ""
    };
    let coins = format!(
        "{}{after_gold}{}{before_copper}{}",
        coin(g, "GOLD"),
        coin(s, "SILVER"),
        coin(c, "COPPER")
    );
    benilla_ui::strings::fill(
        &script.global_text("YOU_LOOT_MONEY"),
        &[benilla_ui::strings::Arg::S(&coins)],
    )
}

/// The group share's line (`0x5ebf20`): `LOOT_MONEY_SPLIT` over the coins, gold first, each nonzero
/// one `"%d %s"` (`0x847fdc`) with its `GOLD`, `SILVER` or `COPPER` global (`0x6c6230`) and every
/// later one `", %d %s"` (`0x860698`). Nothing for 0 (`0x5ebf3a`); the amount splits as a signed
/// word (`SplitCopper 0x6c6260`), so a negative one is all copper.
pub(super) fn share_line(script: &UiScript, amount: u32) -> Option<String> {
    let copper = amount as i32;
    if copper == 0 {
        return None;
    }
    let (g, s, c) = if copper < 0 {
        (0, 0, copper)
    } else {
        (copper / 10_000, copper / 100 % 100, copper % 100)
    };
    let mut coins = String::new();
    for (n, key) in [(g, "GOLD"), (s, "SILVER"), (c, "COPPER")] {
        if n != 0 {
            let sep = if coins.is_empty() { "" } else { ", " };
            coins.push_str(&format!("{sep}{n} {}", script.global_text(key)));
        }
    }
    Some(benilla_ui::strings::fill(
        &script.global_text("LOOT_MONEY_SPLIT"),
        &[benilla_ui::strings::Arg::S(&coins)],
    ))
}
