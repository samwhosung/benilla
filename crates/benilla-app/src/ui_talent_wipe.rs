//! The class trainer's respec question. Selecting "I wish to unlearn my talents" unlearns
//! nothing: the server closes the gossip and asks (`MSG_TALENT_WIPE_CONFIRM`, vmangos
//! `Player::SendTalentWipeConfirm`), and the reset runs (`Player::ResetTalents`, the trainer
//! casting 14867) only when the client sends the same opcode back with the trainer's guid.
//!
//! The stock surface (`StaticPopup.lua:1289-1304`, `UIParent.lua:123`, `533-538`) is
//! `CONFIRM_TALENT_WIPE`, whose one argument is the cost in copper for the dialog's money frame;
//! `ConfirmTalentWipe()`, the Accept; and `CheckTalentMasterDist()`, polled from `OnUpdate`,
//! which hides the dialog the frame it goes false and reads the latch through the call host
//! ([`crate::ui_talent::TalentLive`]). The reference's `0x5df980` serves both directions: a
//! question gates on `d² <= [0xc4c28c]`, the constant behind [`crate::target::SERVICE_RANGE_SQ`],
//! latches the guid (`0xc4d7a0`) and the cost and fires the event inside the handler (`0x5dfa3f`),
//! here on the notice lane at the packet's place; the Accept calls it with a zero guid, which
//! takes the latch and repeats the gate.
//!
//! Deviation: vmangos answers a reset with nothing to reset by asking again with a zero guid
//! (`SkillHandler.cpp:52`), which the 1.12 client takes as its Accept and answers, looping; this
//! drops a zero-guid question, because it means there is nothing to ask.

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

/// The pending respec question; everything the dialog reads rides the event's argument.
#[derive(Resource, Default)]
pub(crate) struct TalentWipeState {
    /// The trainer who asked, sent back on the wire; vmangos resets only for a live
    /// `UNIT_NPC_FLAG_TRAINER` in range.
    npc: Option<u64>,
    /// The reset's cost in copper: the event's `arg1`, and what the Accept compares with the purse.
    cost: u32,
}

impl TalentWipeState {
    /// Inbound `MSG_TALENT_WIPE_CONFIRM`: parks guid and cost. The net arm drops a zero guid
    /// before it gets here.
    pub(crate) fn ask(&mut self, npc: u64, cost: u32) {
        self.npc = Some(npc);
        self.cost = cost;
    }

    /// The guid to answer with, if a question is live.
    pub(crate) fn pending(&self) -> Option<u64> {
        self.npc
    }

    /// Retracts the question: the range guard's close, and the drain's once the answer is sent.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.cost = 0;
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
    /// A trainer this client holds no unit for (`0x5df9be`), or one past `d² <= [0xc4c28c]` from
    /// the player at the packet (`0x5dfa0d`), drops the question. Asking again after a decline is
    /// a second dialog, each its packet's.
    fn on_confirm(
        In(ev): In<SessionEvent>,
        mut wipe: ResMut<TalentWipeState>,
        mut lane: ResMut<crate::field_watch::NoticeLane>,
        index: Res<crate::net::GuidIndex>,
        me: Query<&Transform, With<crate::net::SelfPlayer>>,
        units: Query<&Transform>,
    ) {
        if let SessionEvent::TalentWipeConfirm { trainer, cost } = ev {
            let at = index.0.get(&trainer).and_then(|&e| units.get(e).ok());
            let in_range = at.zip(me.iter().next()).is_some_and(|(at, me)| {
                at.translation.distance_squared(me.translation) <= crate::target::SERVICE_RANGE_SQ
            });
            if trainer == 0 {
                debug!("net: talent wipe refused (no talents to reset) — no dialog");
            } else if !in_range {
                debug!("net: trainer {trainer:#x} missing or out of range — the question drops");
            } else {
                debug!("net: trainer {trainer:#x} asks to wipe talents for {cost} copper");
                // The latch, then the event (`0x5dfa3f`).
                wipe.ask(trainer, cost);
                lane.write(crate::field_watch::FieldNotice::TalentWipe { cost });
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
                // Range-close after the drain that despawns the NPC and ahead of the pass's resume,
                // the feed and the Accept's host after it, so walking away hides the dialog the
                // same frame.
                close_npc_session_out_of_range::<TalentWipeState>
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(crate::vm_pass::VmPass::Resume),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_again_takes_the_new_cost() {
        let mut wipe = TalentWipeState::default();
        assert_eq!(wipe.pending(), None);
        wipe.ask(0x2a, 15_000);
        assert_eq!(wipe.pending(), Some(0x2a));
        wipe.ask(0x2a, 25_000);
        assert_eq!(wipe.cost, 25_000, "a reset climbs the price");
    }

    #[test]
    fn closing_retracts_the_guid_and_the_cost() {
        let mut wipe = TalentWipeState::default();
        wipe.ask(0x2a, 15_000);
        wipe.close();
        assert_eq!(wipe.pending(), None);
        assert_eq!(wipe.cost, 0);
    }
}
