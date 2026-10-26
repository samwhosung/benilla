//! The gossip selects' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): each verb's gates and its packet before the verb
//! returns, as the reference's select functions send in their own bodies (`0x4e2320`, `0x4e24b0`,
//! `0x4e2600`), over the message's slots ([`GossipState`]).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{GossipCall, ScriptValue, UiScript};

use super::GossipState;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_quest::{row_is_active, row_is_one_click};

/// The option slots the store holds (`0x4e2343`).
const OPTION_SLOTS: u32 = 16;

/// The gossip menu and the socket its selects send through.
#[derive(SystemParam)]
pub(crate) struct GossipVerbs<'w, 's> {
    /// Absent in a world with no gossip window, which drops the calls.
    state: Option<ResMut<'w, GossipState>>,
    net: Res<'w, NetCommands>,
    objects: crate::net::Objects<'w, 's>,
}

impl GossipVerbs<'_, '_> {
    /// The menu's cell `[0xbc3f58]`.
    pub(crate) fn npc(&self) -> Option<u64> {
        self.state.as_deref().and_then(|s| s.npc)
    }

    /// The close core's zero of the cell (`0x4e1fcc`); the slots, the text and its latch stay.
    pub(crate) fn clear_npc(&mut self) {
        if let Some(state) = self.state.as_deref_mut() {
            state.npc = None;
        }
    }

    /// Apply one gossip call; the close is the host's ([`GossipCall::Close`]).
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: GossipCall) {
        let player = self.objects.player().is_some();
        let Some(state) = self.state.as_mut() else {
            debug!("ui_gossip: no gossip window in this world; {call:?} dropped");
            return;
        };
        let npc = state.npc.unwrap_or(0);
        match call {
            // The close core needs the latch, which the host lends beside this.
            GossipCall::Close => {}
            GossipCall::Option { index, code } => {
                // `0x4e2320`: refused while the greeting is pending (`0xbbb670`, `0x4e2335`), for a
                // slot outside the 16 (`0x4e233d`, `0x4e2346`) or an unused one, whose index is -1
                // (`0x4e235f`).
                let slot = index.wrapping_sub(1);
                if state.text_pending() || slot >= OPTION_SLOTS {
                    return;
                }
                let Some(opt) = state
                    .options
                    .get(slot as usize)
                    .filter(|o| o.index != u32::MAX)
                else {
                    return;
                };
                // A coded slot with no code asks for one, 1-based (`0x4e2365`-`0x4e237e`).
                if opt.coded && code.is_none() {
                    script.fire_event(
                        "GOSSIP_ENTER_CODE",
                        vec![ScriptValue::Int(i64::from(index))],
                    );
                    return;
                }
                debug!("ui_gossip: select option {index} (index {})", opt.index);
                // The code only for a coded slot (`0x4e23d4`).
                let _ = self.net.0.send(ClientCommand::GossipSelectOption {
                    guid: npc,
                    option: opt.index,
                    code: code.filter(|_| opt.coded),
                });
                // `ResetGossipOptions` (`0x4e23f4` → `0x4e26a0`): every slot's index to -1, which
                // no getter or select reads, and every quest id to 0, which ends the row scans.
                // No event: the menu the feed pushed is the VM's as reset.
                let state = state.bypass_change_detection();
                state.reset_slots();
                script.reset_gossip_slots();
            }
            GossipCall::Quest { active, index } => {
                // The scans stop at the first quest id of 0 (`0x4e24da`, `0x4e262a`) and count the
                // rows of their kind (`0x4e24e6`, `0x4e2632`); the send needs only the active
                // player (`0x4e250b`, `0x4e2657`), not the greeting.
                let Some(&(quest, icon, _, _)) = state
                    .quests
                    .iter()
                    .take_while(|q| q.0 != 0)
                    .filter(|q| row_is_active(q.1) == active)
                    .nth(index.wrapping_sub(1) as usize)
                else {
                    return;
                };
                if !player {
                    return;
                }
                // An active row is a turn-in (`0x4e267f`), and so is an available one with its
                // one-click icon (`0x4e2522`, `0x4e253c`); another is the query (`0x4e2554`).
                let command = if active || row_is_one_click(icon) {
                    ClientCommand::QuestgiverComplete { npc, quest }
                } else {
                    ClientCommand::QuestgiverQuery { npc, quest }
                };
                debug!("ui_gossip: quest row {index} (quest {quest}, icon {icon})");
                let _ = self.net.0.send(command);
            }
        }
    }
}
