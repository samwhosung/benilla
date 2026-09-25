//! Unlearning your talents — the class trainer's respec question, its dialog, and its answer.
//!
//! **The law this module exists for: selecting the trainer's "I wish to unlearn my talents" line
//! unlearns nothing.** It makes the server close the gossip menu and ask
//! (`MSG_TALENT_WIPE_CONFIRM` inbound, vmangos `Player::SendTalentWipeConfirm` off
//! `GOSSIP_OPTION_UNLEARNTALENTS`); the reset happens only when the client sends the **same
//! opcode** back with the trainer's guid, at which point `Player::ResetTalents` runs and the
//! trainer casts 14867 "Untalent Visual Effect". Before this module the question was parsed as an
//! unknown opcode and dropped, so the line looked like it did nothing at all — the director's
//! "I can't unlearn my talents, the confirm window never pops up".
//!
//! It is the innkeeper bind's twin, not a talent-window affordance, and that is why it lives beside
//! [`crate::ui_binder`] rather than inside [`crate::ui_talent`]: same shape on the wire (a question
//! carrying a guid, an answer echoing it), same latch, same range gate, same free-standing dialog
//! over a gossip menu that is already gone. The bytes say so: the talent master
//! (`0xc4d7a0`/`0xc4d7a4`), the pet untrainer and the binder are **one latch/range family**, three
//! copies of a single shape.
//!
//! The Era surface it drives is the reference's own (`StaticPopup.lua:1289-1304`,
//! `UIParent.lua:123`/`533-538`):
//!
//! - the `CONFIRM_TALENT_WIPE` event, whose **one argument is the cost in copper** — the dialog's
//!   money frame, not its text (the text is a fixed GlobalString);
//! - `ConfirmTalentWipe()` — the dialog's Accept, the only call that sends the answer;
//! - `CheckTalentMasterDist()` — polled from the dialog's `OnUpdate`, and the dialog hides itself
//!   the frame it goes false.
//!
//! Both halves are **byte-pinned** against the reference's `0x5df980`, one function that serves
//! both directions of the opcode:
//!
//! 1. **Arrival** (`guid != 0`): resolve the unit, gate on `d² <= [0xc4c28c]` — the identical
//!    constant behind [`crate::target::SERVICE_RANGE_SQ`], which is what makes modelling the
//!    question as an [`NpcSession`] faithful rather than convenient — then latch guid + cost and
//!    `SignalEvent2(CONFIRM_TALENT_WIPE, "%d", cost)` at `0x5dfa3f`.
//! 2. **Accept** (`ConfirmTalentWipe()` calls the same function with a **zeroed** guid, which
//!    substitutes the latch): the same range gate, then a **client-side money check** —
//!    `cost > coinage` shows `ERR_NOT_ENOUGH_MONEY` (`DisplayError(0x25)`, `0x5dfa81`) and sends
//!    **nothing**; otherwise the answer goes out carrying the latched guid.
//!
//! **One deliberate deviation, and it is the zero guid.** vmangos answers a failed reset (no points
//! spent) by asking again with `trainer = 0` (`SkillHandler.cpp:52`; the field's own comment reads
//! "0 if player has no talents to reset"). At the bytes that lands on leg 2, not leg 1 — a zero
//! guid *is* the Accept path — so a 1.12 client re-sends the answer, is refused again, and the two
//! sit there trading packets. We treat a zero-guid question as what it means, "there is nothing to
//! ask", and drop it: no dialog, no send. A packet loop is not a look we owe anyone.

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

/// The pending respec question. Written by the net drain's `TalentWipeConfirm` arm, read by
/// [`feed_talent_wipe`] (which fires `CONFIRM_TALENT_WIPE` and publishes `CheckTalentMasterDist`'s
/// answer) and by [`drain_talent_wipe`] (which turns the dialog's Accept into the answer packet).
///
/// [`crate::ui_binder::BinderState`]'s shape exactly, down to the per-packet `ask` flag: everything
/// the dialog reads arrives as the event's argument, so there is no snapshot beside it.
#[derive(Resource, Default)]
pub(crate) struct TalentWipeState {
    /// The trainer that asked; `None` = no question pending. This guid is what goes back on the
    /// wire, and vmangos resolves it to a live `UNIT_NPC_FLAG_TRAINER` in range — a stale one
    /// unlearns nothing.
    npc: Option<u64>,
    /// What the reset costs, in copper — the event's `arg1`, and the number the Accept gate
    /// compares against the purse.
    cost: u32,
    /// A question the feed still owes the UI. Set per *packet*, not per state edge: declining and
    /// clicking the line again is two dialogs, and an edge-diff would swallow the second.
    ask: bool,
}

impl TalentWipeState {
    /// The inbound `MSG_TALENT_WIPE_CONFIRM` — park the trainer's guid and cost, and owe the UI a
    /// dialog. A **zero** guid never reaches here (the net arm drops it; module doc's deviation).
    pub(crate) fn ask(&mut self, npc: u64, cost: u32) {
        self.npc = Some(npc);
        self.cost = cost;
        self.ask = true;
    }

    /// The guid to answer with, if a question is live.
    fn pending(&self) -> Option<u64> {
        self.npc
    }

    /// Retract the question — the range guard's close, and the drain's once the answer is sent.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.cost = 0;
        self.ask = false;
    }
}

/// The question is an NPC session: the standardized range guard closes it — the same no-packet
/// clear declining does — when the player walks out of the trainer's service range or the trainer
/// despawns. That close is what `CheckTalentMasterDist()` reports, and it is the reference's own
/// second range test rather than a convenience of ours (module doc, leg 2).
impl NpcSession for TalentWipeState {
    fn npc(&self) -> Option<u64> {
        self.npc
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Fire `CONFIRM_TALENT_WIPE(cost)` for a question the UI is still owed, and publish
/// `CheckTalentMasterDist()`'s answer every frame.
fn feed_talent_wipe(script: Option<NonSendMut<UiScript>>, mut wipe: ResMut<TalentWipeState>) {
    let Some(mut script) = script else {
        return;
    };
    script.set_talent_master_pending(wipe.pending().is_some());

    if !wipe.ask {
        return;
    }
    wipe.ask = false;
    // arg1 is the cost in COPPER, as a number — the reference's own `"%d"` fire (`0x5dfa3f`), and
    // what `MoneyFrame_Update` on the dialog's money frame takes.
    script.fire_event(
        "CONFIRM_TALENT_WIPE",
        vec![ScriptValue::Int(i64::from(wipe.cost))],
    );
}

/// Turn the dialog's Accept into the outbound `MSG_TALENT_WIPE_CONFIRM` — or into the red
/// not-enough-money line, which is the reference's own fork at Accept time (module doc, leg 2).
///
/// Gated on a question still being pending, exactly as [`crate::ui_binder`]'s accept is: a
/// `ConfirmTalentWipe()` typed at the console with no trainer asking would otherwise send a zero
/// guid the server can only drop.
fn drain_talent_wipe(
    script: Option<NonSendMut<UiScript>>,
    mut wipe: ResMut<TalentWipeState>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let confirms = script.take_talent_wipe_confirms();
    if confirms == 0 {
        return;
    }
    let Some(npc) = wipe.pending() else {
        return;
    };
    // The client's own purse check (`0x5dfa79`): over budget is a red line and no packet, so the
    // player is told rather than met with silence — vmangos's refusal for the same case is the
    // zero-guid question this module drops.
    let money = self_q
        .single()
        .ok()
        .and_then(|store| store.0.player_money())
        .unwrap_or(0);
    if wipe.cost > money {
        debug!(
            "ui_talent_wipe: respec costs {} copper, purse holds {money} — refused client-side",
            wipe.cost
        );
        // The reference's `DisplayError(0x25)` = `ERR_NOT_ENOUGH_MONEY`: the red line AND, since
        // that row's `+0x0c` is `0x28`, the spoken one.
        let text = script
            .lua()
            .globals()
            .get::<String>("ERR_NOT_ENOUGH_MONEY")
            .unwrap_or_default();
        if !text.is_empty() {
            crate::ui_action::show_messages(
                &mut script,
                &mut sink,
                "ui_talent_wipe",
                [crate::ui_action::Shown::keyed("ERR_NOT_ENOUGH_MONEY", text)],
            );
        }
        return;
    }
    for _ in 0..confirms {
        debug!("ui_talent_wipe: confirming the wipe at trainer {npc:#x}");
        let _ = commands
            .0
            .send(ClientCommand::TalentWipeConfirm { trainer: npc });
    }
    // A question that has been answered is not a question. The reference never clears its latch
    // (`0xc4d7a0`) — the same deviation [`crate::ui_binder`] takes, and for the same reason: its
    // own dialog is gone by now either way, and a second Accept would be a second reset.
    wipe.clear();
}

/// The respec flow: the range guard, the dialog's feed, and its answer.
/// The talent-wipe question's packet handler (in the net handler table since 2313).
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::TalentWipeState;
    use crate::net::NetHandlerApp;

    /// Register the handler — called from [`super::UiTalentWipePlugin`].
    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::TalentWipeConfirm, on_confirm);
    }

    /// A zero trainer guid is vmangos's "you have no talents to reset" refusal, not a question —
    /// there is nothing to ask about, so nothing goes on screen (this module's header carries
    /// why the reference instead re-sends here).
    fn on_confirm(In(ev): In<SessionEvent>, mut wipe: ResMut<TalentWipeState>) {
        if let SessionEvent::TalentWipeConfirm { trainer, cost } = ev {
            if trainer == 0 {
                debug!("net: talent wipe refused (no talents to reset) — no dialog");
            } else {
                debug!("net: trainer {trainer:#x} asks to wipe talents for {cost} copper");
                wipe.ask(trainer, cost);
            }
        }
    }
}

pub(crate) struct UiTalentWipePlugin;

impl Plugin for UiTalentWipePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TalentWipeState>().add_systems(
            Update,
            (
                // Range-close before the feed so walking away takes the dialog down the same
                // frame (the binder question's ordering, for the same reason).
                close_npc_session_out_of_range::<TalentWipeState>.before(feed_talent_wipe),
                feed_talent_wipe.in_set(UiFeed),
                drain_talent_wipe.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second question from the same trainer is a second dialog (decline, then click the line
    /// again). The `ask` flag is per packet precisely so a state diff cannot swallow it.
    #[test]
    fn asking_twice_owes_two_dialogs() {
        let mut wipe = TalentWipeState::default();
        assert_eq!(wipe.pending(), None);

        wipe.ask(0x2a, 15_000);
        assert!(wipe.ask);
        wipe.ask = false; // the feed fired the first dialog
        assert_eq!(wipe.pending(), Some(0x2a), "the guid outlives the fire");
        assert_eq!(
            wipe.cost, 15_000,
            "and so does the cost the money frame reads"
        );

        wipe.ask(0x2a, 25_000);
        assert!(wipe.ask, "the same trainer asking again owes a dialog");
        assert_eq!(
            wipe.cost, 25_000,
            "at the NEW cost — a reset climbs the price"
        );
    }

    /// Closing (the range guard, or the answer going out) retracts the guid, the cost and any
    /// unfired question — so `CheckTalentMasterDist()` goes false and a later `ConfirmTalentWipe()`
    /// sends nothing.
    #[test]
    fn closing_retracts_the_guid_the_cost_and_the_unfired_dialog() {
        let mut wipe = TalentWipeState::default();
        wipe.ask(0x2a, 15_000);
        wipe.close();
        assert_eq!(wipe.pending(), None);
        assert_eq!(wipe.cost, 0);
        assert!(!wipe.ask);
    }
}
