//! The class trainer's respec question. Selecting "I wish to unlearn my talents" unlearns
//! nothing: the server closes the gossip and asks (`MSG_TALENT_WIPE_CONFIRM`, vmangos
//! `Player::SendTalentWipeConfirm`), and the reset runs (`Player::ResetTalents`, the trainer
//! casting 14867) only when the client sends the same opcode back with the trainer's guid.
//!
//! The stock surface (`StaticPopup.lua:1289-1304`, `UIParent.lua:123`, `533-538`) is
//! `CONFIRM_TALENT_WIPE`, whose one argument is the cost in copper for the dialog's money frame;
//! `ConfirmTalentWipe()`, the Accept; and `CheckTalentMasterDist()`, polled from `OnUpdate`,
//! which hides the dialog the frame it goes false. The reference's `0x5df980` serves both
//! directions: a question gates on `d² <= [0xc4c28c]`, the constant behind
//! [`crate::target::SERVICE_RANGE_SQ`], latches the guid (`0xc4d7a0`) and the cost and fires the
//! event; the Accept calls it with a zero guid, which takes the latch and repeats the gate.
//!
//! Deviation: vmangos answers a reset with nothing to reset by asking again with a zero guid
//! (`SkillHandler.cpp:52`), which the 1.12 client takes as its Accept and answers, looping; this
//! drops a zero-guid question, because it means there is nothing to ask.

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::ui_script::UiFeed;
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

/// The pending respec question; everything the dialog reads rides the event's argument.
#[derive(Resource, Default)]
pub(crate) struct TalentWipeState {
    /// The trainer who asked, sent back on the wire; vmangos resets only for a live
    /// `UNIT_NPC_FLAG_TRAINER` in range.
    npc: Option<u64>,
    /// The reset's cost in copper: the event's `arg1`, and what the Accept compares with the purse.
    cost: u32,
    /// A dialog the feed still owes, set per packet: asking again after a decline is a second
    /// dialog, which a state diff would swallow.
    ask: bool,
}

impl TalentWipeState {
    /// Inbound `MSG_TALENT_WIPE_CONFIRM`: parks guid and cost and owes the UI a dialog. The net arm
    /// drops a zero guid before it gets here.
    pub(crate) fn ask(&mut self, npc: u64, cost: u32) {
        self.npc = Some(npc);
        self.cost = cost;
        self.ask = true;
    }

    /// The guid to answer with, if a question is live.
    fn pending(&self) -> Option<u64> {
        self.npc
    }

    /// Retracts the question: the range guard's close, and the drain's once the answer is sent.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.cost = 0;
        self.ask = false;
    }
}

/// The range guard closes the question without a packet, as declining does, when the player
/// leaves service range or the trainer despawns; that close is what `CheckTalentMasterDist()`
/// reports, the reference's Accept-time range test.
impl NpcSession for TalentWipeState {
    fn npc(&self) -> Option<u64> {
        self.npc
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Publishes `CheckTalentMasterDist()` every frame and fires `CONFIRM_TALENT_WIPE(cost)` for a
/// question still owed.
fn feed_talent_wipe(mut vm: crate::script_calls::VmWith<ResMut<'static, TalentWipeState>>) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    let (pending, owed) = {
        let mut wipe = vm.own();
        let owed = std::mem::take(&mut wipe.ask).then_some(wipe.cost);
        (wipe.pending().is_some(), owed)
    };
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        script.set_talent_master_pending(pending);
        // The cost in copper as a number, the reference's `"%d"` fire (`0x5dfa3f`), which the
        // dialog's `MoneyFrame_Update` takes.
        if let Some(cost) = owed {
            script.fire_event(
                "CONFIRM_TALENT_WIPE",
                vec![ScriptValue::Int(i64::from(cost))],
            );
        }
    })
}

/// `ConfirmTalentWipe()`'s answer inside its call (`0x5df980` with a zero guid): with a question
/// latched, over budget is a red line and no packet (`0x5dfa79`), else `MSG_TALENT_WIPE_CONFIRM`
/// with the trainer's guid (`0x5dfad6`). With no guid latched, the reference finds no unit and
/// sends nothing (`0x5df9be`); out of range the range guard has retracted the question already.
pub(crate) fn accept(
    wipe: &mut TalentWipeState,
    money: u32,
    net: &NetCommands,
    script: &mut UiScript,
    sink: &mut crate::ui_action::MessageSink,
) {
    let Some(npc) = wipe.pending() else {
        return;
    };
    if wipe.cost > money {
        debug!(
            "ui_talent_wipe: respec costs {} copper, purse holds {money} — refused client-side",
            wipe.cost
        );
        // `DisplayError(0x25)` at `0x5dfa81`, `ERR_NOT_ENOUGH_MONEY`: the red line and, as that
        // row's `+0x0c` is `0x28`, the spoken one, under the call's input events.
        let input = script.input_events();
        let text = script
            .lua()
            .globals()
            .get::<String>("ERR_NOT_ENOUGH_MONEY")
            .unwrap_or_default();
        if !text.is_empty() {
            crate::ui_action::show_messages(
                script,
                sink,
                "ui_talent_wipe",
                [crate::ui_action::Shown::keyed("ERR_NOT_ENOUGH_MONEY", text).under(input)],
            );
        }
        return;
    }
    debug!("ui_talent_wipe: confirming the wipe at trainer {npc:#x}");
    let _ = net
        .0
        .send(ClientCommand::TalentWipeConfirm { trainer: npc });
    // Deviation: the reference never clears its latch (`0xc4d7a0`); this does once the answer is
    // sent, because the reference's dialog is gone by then and a second Accept would reset twice.
    wipe.clear();
}

/// The talent-wipe question's packet handler.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::TalentWipeState;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::TalentWipeConfirm, on_confirm);
    }

    /// A zero trainer guid is vmangos's refusal, nothing to reset, so nothing goes on screen.
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
                // Range-close ahead of the VM pass, the feed and the Accept's host among it, so
                // walking away hides the dialog the same frame.
                close_npc_session_out_of_range::<TalentWipeState>
                    .before(crate::vm_pass::VmPass::Start),
                feed_talent_wipe.in_set(UiFeed),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
