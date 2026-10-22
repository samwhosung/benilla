//! The gossip selects' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): each packet before the verb returns, as the
//! reference's select functions send in their own bodies (`0x4e2320`, `0x4e24b0`, `0x4e2600`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::GossipCall;

use super::GossipState;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_quest::{row_is_active, row_is_one_click};

/// The gossip menu and the socket its selects send through.
#[derive(SystemParam)]
pub(crate) struct GossipVerbs<'w> {
    /// Absent in a world with no gossip window, which drops the calls.
    state: Option<Res<'w, GossipState>>,
    net: Res<'w, NetCommands>,
}

impl GossipVerbs<'_> {
    /// Apply one gossip call: an option sends `CMSG_GOSSIP_SELECT_OPTION` with its wire `index`, a
    /// quest row its questgiver query or turn-in. A coded option is not sent: the reference's code
    /// prompt (`GOSSIP_ENTER_CODE`, `0x4e237e`) is not built.
    pub(crate) fn apply(&mut self, call: GossipCall) {
        let Some(state) = self.state.as_deref() else {
            debug!("ui_gossip: no gossip window in this world; {call:?} dropped");
            return;
        };
        let Some(npc) = state.npc else {
            return;
        };
        match call {
            GossipCall::Option(pos) => {
                // Refused while the text query is out, as the reference's `SelectGossipOption`
                // (`0x4e2320`) refuses under the latch `0xbbb670`.
                if state.text_pending() {
                    debug!(
                        "ui_gossip: SelectGossipOption({pos}) while the text query is in flight \
                         — refused"
                    );
                    return;
                }
                match pos
                    .checked_sub(1)
                    .and_then(|i| state.options.get(i as usize))
                {
                    Some(opt) if !opt.coded => {
                        debug!("ui_gossip: select option {pos} (index {})", opt.index);
                        let _ = self.net.0.send(ClientCommand::GossipSelectOption {
                            guid: npc,
                            option: opt.index,
                        });
                    }
                    Some(_) => {
                        debug!("ui_gossip: ignoring coded option {pos} (coded options are greyed)")
                    }
                    None => debug!("ui_gossip: SelectGossipOption({pos}) out of range — ignored"),
                }
            }
            GossipCall::Quest(pos) => {
                // Refused while pending too; the reference's latch is traced for option selects
                // only.
                if state.text_pending() {
                    debug!(
                        "ui_gossip: SelectGossipQuest({pos}) while the text query is in flight \
                         — refused"
                    );
                    return;
                }
                let Some(&(quest, icon, _, _)) = pos
                    .checked_sub(1)
                    .and_then(|i| state.quests.get(i as usize))
                else {
                    debug!("ui_gossip: SelectGossipQuest({pos}) out of range — ignored");
                    return;
                };
                // As on the quest greeting panel: an active row is a turn-in (`0x4e267f`), and so
                // is an available one with its one-click icon (`0x4e2522`, `0x4e253c`); another
                // is the query (`0x4e2554`).
                let command = if row_is_active(icon) || row_is_one_click(icon) {
                    ClientCommand::QuestgiverComplete { npc, quest }
                } else {
                    ClientCommand::QuestgiverQuery { npc, quest }
                };
                debug!("ui_gossip: quest row {pos} (quest {quest}, icon {icon})");
                let _ = self.net.0.send(command);
            }
        }
    }
}
