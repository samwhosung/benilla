//! The questgiver, quest-log and share verbs' engine half, which the call host applies inside each
//! call ([`crate::script_calls::ScriptCallHost`]): each verb's gates and its packet before the verb
//! returns, as the reference's bindings send in their own bodies (`0x5012a0`, `0x501320`,
//! `0x501380`, `0x5013f0`, `0x501560`, `0x5015b0`, `0x5021b0`, `0x4df070`, `0x4e1340`). A verb that
//! closes the window leaves its close core, which holds the whole world, to
//! [`super::drain_quest`] ([`QuestGiver::teardown`]).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{QuestCall, QuestSelect, UiScript};

use super::{decline_leg, DeclineLeg, QuestGiver, QuestPools, QuestView};
use crate::net::{ClientCommand, NetCommands};

/// What `GetQuestReward` raises for a choice outside the offer (`0x84eda4`).
const INVALID_REWARD: &str = "Invalid reward choice in GetQuestReward([choice])";

/// The questgiver window and what its verbs send through.
#[derive(SystemParam)]
pub(crate) struct QuestVerbs<'w, 's> {
    /// Absent in a world with no questgiver window, which drops the panel verbs.
    giver: Option<ResMut<'w, QuestGiver>>,
    net: Res<'w, NetCommands>,
    objects: crate::net::Objects<'w, 's>,
}

impl QuestVerbs<'_, '_> {
    /// Apply one quest call.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: QuestCall) {
        let send = |net: &NetCommands, command| {
            let _ = net.0.send(command);
        };
        match call {
            QuestCall::ConfirmAccept => {
                // `[0xbe0840]` with no test (`0x5021e0`); a world with no window holds none.
                let quest = self.giver.as_deref().map_or(0, |g| g.confirm);
                send(&self.net, ClientCommand::QuestConfirmAccept { quest })
            }
            QuestCall::Abandon(slot) => send(&self.net, ClientCommand::QuestlogRemove { slot }),
            QuestCall::Push(quest) => send(&self.net, ClientCommand::PushQuestToParty { quest }),
            QuestCall::Select(select) => self.select(select),
            QuestCall::Accept => self.accept(script),
            QuestCall::Decline => self.decline(),
            QuestCall::Complete => self.complete(),
            QuestCall::Reward(choice) => self.reward(script, choice),
        }
    }

    /// `0x5012a0` and `0x501320` over the pools, whatever panel shows: the row, less one, at most
    /// the pool's count (`cmp; ja`, `0x5012a8`, `0x501328`), the latch clear (`0x5012b3`,
    /// `0x501333`) and the active player (`0x5012d4`, `0x501352`); then `SelectActiveQuest`'s
    /// `CMSG_QUESTGIVER_COMPLETE_QUEST` (`0x501365`), or `SelectAvailableQuest`'s, for a one-click
    /// row (`0x5012e3`, `0x5012f1`), else its `CMSG_QUESTGIVER_QUERY_QUEST` (`0x50130e`), to the
    /// giver's cell, 0 once closed, and the latch (`0x5012f6`, `0x501313`, `0x50136a`).
    fn select(&mut self, select: QuestSelect) {
        let player = self.objects.player().is_some();
        let Some(giver) = self.giver.as_deref_mut() else {
            return;
        };
        let pools = &giver.pools;
        let (count, row) = match select.active {
            true => (
                pools.active.len(),
                pools
                    .active
                    .get(select.index.wrapping_sub(1) as usize)
                    .map(|&q| (q, false)),
            ),
            false => (
                pools.available.len(),
                pools
                    .available
                    .get(select.index.wrapping_sub(1) as usize)
                    .copied(),
            ),
        };
        let index = select.index.wrapping_sub(1) as usize;
        // The row one past the count is the cleared row (`0x500c46`); past a full pool it lies in
        // other cells, which nothing here holds.
        let Some((quest, one_click)) =
            row.or((index == count && count < QuestPools::ROWS).then_some((0, false)))
        else {
            debug!("ui_quest: greeting select {select:?} out of range — ignored");
            return;
        };
        if giver.acted || !player {
            return;
        }
        let npc = giver.npc.unwrap_or(0);
        let command = if select.active || one_click {
            ClientCommand::QuestgiverComplete { npc, quest }
        } else {
            ClientCommand::QuestgiverQuery { npc, quest }
        };
        let _ = self.net.0.send(command);
        giver.acted = true;
    }

    /// `0x501380`: the detail panel (`[0xbe0818] == 1`), the active player and the latch clear
    /// (`0x5013b0`); then `CMSG_QUESTGIVER_ACCEPT_QUEST` (`0x5013c0`), the latch (`0x5013c9`), the
    /// close `0x501130(0,0)` (`0x5013d3`) and the Quest Log tutorial (`0x5013df`).
    fn accept(&mut self, script: &mut UiScript) {
        let player = self.objects.player().is_some();
        let Some((npc, giver)) = open(&mut self.giver) else {
            return;
        };
        let Some(QuestView::Detail(d)) = giver.view.as_ref() else {
            return;
        };
        if !player || giver.acted {
            return;
        }
        let quest = d.quest_id;
        let _ = self
            .net
            .0
            .send(ClientCommand::QuestgiverAccept { npc, quest });
        giver.acted = true;
        // The close core needs the whole world: later verbs in this chunk see the window open.
        giver.teardown = Some(false);
        script.queue_tutorial_trigger(crate::tutorial::id::QUEST_LOG);
    }

    /// `DeclineQuest`'s fork `0x5013f0` ([`decline_leg`]); a re-open sets the latch (`0x501479`,
    /// `0x501549`).
    fn decline(&mut self) {
        let Some(npc) = self.giver.as_deref().and_then(|g| g.npc) else {
            return;
        };
        // The source as `0x468460` finds it, its type mask and `UNIT_NPC_FLAGS`; `None` unseen.
        let source = self
            .objects
            .object(npc)
            .map(|f| (f.object_type_mask().unwrap_or(0), f.unit_npc_flags()));
        let player = self.objects.player().is_some();
        let Some(giver) = self.giver.as_deref_mut() else {
            return;
        };
        let leg = decline_leg(giver.acted, giver.close_on_cancel, source, player);
        debug!("ui_quest: decline on {npc:#x} — {leg:?}");
        match leg {
            DeclineLeg::Nothing => {}
            // The close core needs the whole world: later verbs in this chunk see the window open.
            DeclineLeg::Teardown => giver.teardown = Some(true),
            DeclineLeg::GossipHello => {
                let _ = self.net.0.send(ClientCommand::GossipHello { guid: npc });
                giver.acted = true;
            }
            DeclineLeg::QuestgiverHello => {
                let _ = self.net.0.send(ClientCommand::QuestgiverHello { npc });
                giver.acted = true;
            }
        }
    }

    /// `0x501560`: the progress panel (`[0xbe0818] == 2`), the latch clear (`0x501570`) and the
    /// active player; then `CMSG_QUESTGIVER_REQUEST_REWARD` (`0x50159f`) and the latch
    /// (`0x5015a4`).
    fn complete(&mut self) {
        let player = self.objects.player().is_some();
        let Some((npc, giver)) = open(&mut self.giver) else {
            return;
        };
        let Some(QuestView::Progress(p)) = giver.view.as_ref() else {
            return;
        };
        if giver.acted || !player {
            return;
        }
        let quest = p.quest_id;
        let _ = self
            .net
            .0
            .send(ClientCommand::QuestgiverRequestReward { npc, quest });
        giver.acted = true;
    }

    /// `0x5015b0`: the reward panel (`[0xbe0818] == 3`, `0x5015bb`), the active player
    /// (`0x5015e4`) and the latch clear (`0x5015f5`), each else nothing; then, with choices on
    /// offer (`0x5016c0`), the choice inside them (`0x501608`, `0x50160e`), else the verb raises,
    /// and the chosen item latched for the turn-in's line (`0x50161a`); then
    /// `CMSG_QUESTGIVER_CHOOSE_REWARD` with the choice, a negative one as 0 (`0x501627`-`0x501638`),
    /// and the latch (`0x501643`).
    fn reward(&mut self, script: &mut UiScript, choice: i32) {
        let player = self.objects.player().is_some();
        let Some((npc, giver)) = open(&mut self.giver) else {
            return;
        };
        let Some(QuestView::Reward(o)) = giver.view.as_ref() else {
            return;
        };
        if !player || giver.acted {
            return;
        }
        let quest = o.quest_id;
        let offered = o.choices.len() as i32;
        if offered > 0 {
            let Some(item) = usize::try_from(choice)
                .ok()
                .and_then(|i| o.choices.get(i))
                .map(|it| it.item_id)
            else {
                script.raise_in_call(INVALID_REWARD);
                return;
            };
            giver.chosen_reward = item;
        }
        let _ = self.net.0.send(ClientCommand::QuestgiverChooseReward {
            npc,
            quest,
            choice: choice.max(0) as u32,
        });
        giver.acted = true;
    }
}

/// The window's giver and panel, `None` with none open.
fn open<'a>(giver: &'a mut Option<ResMut<'_, QuestGiver>>) -> Option<(u64, &'a mut QuestGiver)> {
    let giver = giver.as_deref_mut()?;
    Some((giver.npc?, giver))
}
