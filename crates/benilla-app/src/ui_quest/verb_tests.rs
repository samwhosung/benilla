//! The questgiver, quest-log and share verbs inside their calls, through the whole client's call
//! host: each packet goes out before the verb returns, as the reference's bindings send in their
//! own bodies, so a chunk's packets leave in call order; and each verb's gates, the dialog kind
//! `[0xbe0818]` and the already-acted latch `[0xbe0844]`, refuse as the reference's do.

use benilla_protocol::messages::{
    QuestConfirmAccept, QuestDetails, QuestGiverList, QuestListEntry, QuestOfferReward,
    QuestRequestItems, QuestRewardItem,
};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::{QuestGiver, QuestView};
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::script_calls::with_hosted_vm;

const NPC: u64 = 0xF130_0000_0000_0007;
const ME: u64 = 0x10;
const ROLL: &str = "RandomRoll { min: 1, max: 100 }";

/// The packets these verbs send, by their debug text; the record and name queries aside.
fn verb_packet(c: &ClientCommand) -> Option<String> {
    let text = format!("{c:?}");
    [
        "Questgiver",
        "GossipHello",
        "GossipSelectOption",
        "RandomRoll",
        "QuestConfirmAccept",
        "QuestlogRemove",
        "PushQuestToParty",
        "QuestPushResult",
    ]
    .iter()
    .any(|p| text.starts_with(p))
    .then_some(text)
}

/// The whole client, headless, with the active player in the world and `NPC` streamed with
/// `npc_flags`.
pub(crate) struct Client {
    pub(crate) app: App,
    rx: Receiver<ClientCommand>,
}

impl Client {
    pub(crate) fn new(npc_flags: u32) -> Self {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        world.spawn((SelfPlayer, Guid(ME), ObjectStore::default()));
        // 147 = `UNIT_NPC_FLAGS` (the descriptor index `unit_npc_flags()` reads).
        let fields = benilla_protocol::ObjectFields::from_pairs(&[(147, npc_flags)]);
        let npc = world.spawn((Guid(NPC), ObjectStore(fields))).id();
        world.resource_mut::<GuidIndex>().0.insert(NPC, npc);
        world.insert_non_send_resource(UiScript::new().expect("a VM"));
        Self { app, rx }
    }

    /// Open `view` on `NPC` and run the feed, which pushes it to the VM and fires its event.
    pub(crate) fn open(&mut self, view: QuestView) {
        self.app
            .world_mut()
            .resource_mut::<QuestGiver>()
            .open(NPC, view);
        self.app
            .world_mut()
            .run_system_cached(super::feed_quest)
            .expect("the feed runs");
        self.sent();
    }

    /// Run `lua` with the host lent, as every entry that runs Lua lends it.
    pub(crate) fn run(&mut self, lua: &str) -> Result<(), String> {
        with_hosted_vm(self.app.world_mut(), |s| s.expect("a VM").run(lua))
            .map_err(|e| e.to_string())
    }

    /// The questgiver window's drain, the frame's after the input pass.
    pub(crate) fn drain(&mut self) {
        self.app
            .world_mut()
            .run_system_cached(super::drain_quest)
            .expect("the drain runs");
    }

    /// What went on the wire since the last read.
    pub(crate) fn sent(&self) -> Vec<String> {
        self.rx.try_iter().filter_map(|c| verb_packet(&c)).collect()
    }
}

fn row(quest_id: u32, icon: u32) -> QuestListEntry {
    QuestListEntry {
        quest_id,
        icon,
        level: 1,
        title: format!("Quest {quest_id}"),
    }
}

/// A greeting: active quest 5 (icon 3), available quest 6 (icon 5) and one-click quest 7 (icon 0).
fn greeting() -> QuestView {
    QuestView::Greeting(QuestGiverList {
        npc: NPC,
        greeting: "Hail.".into(),
        emote_delay: 0,
        emote: 0,
        quests: vec![row(5, 3), row(6, 5), row(7, 0)],
    })
}

fn detail() -> QuestView {
    QuestView::Detail(QuestDetails {
        npc: NPC,
        quest_id: 100,
        title: "A Threat Within".into(),
        details: String::new(),
        objectives: String::new(),
        auto_finish: 0,
        choices: Vec::new(),
        rewards: Vec::new(),
        money: 0,
        reward_spell: 0,
    })
}

fn progress() -> QuestView {
    QuestView::Progress(QuestRequestItems {
        npc: NPC,
        quest_id: 100,
        title: "A Threat Within".into(),
        request_text: String::new(),
        emote: 0,
        close_on_cancel: 0,
        required_money: 0,
        required_items: Vec::new(),
        is_complete: true,
    })
}

fn reward(choices: u32) -> QuestView {
    QuestView::Reward(QuestOfferReward {
        npc: NPC,
        quest_id: 100,
        title: "A Threat Within".into(),
        offer_text: String::new(),
        auto_finish: 0,
        choices: (0..choices)
            .map(|i| QuestRewardItem {
                item_id: 3000 + i,
                count: 1,
                display_id: 0,
            })
            .collect(),
        rewards: Vec::new(),
        money: 0,
        quest_flags: 0,
        reward_spell: 0,
    })
}

/// Each questgiver verb sends inside its call, so a roll on the chunk's next line follows it on
/// the wire: `SelectActiveQuest` (`0x501365`), `SelectAvailableQuest` (`0x50130e`, its one-click
/// leg `0x5012f1`), `AcceptQuest` (`0x5013c0`), `CompleteQuest` (`0x50159f`), `GetQuestReward`
/// (`0x501638`) and `DeclineQuest`'s re-open (`0x501526`).
#[test]
fn each_questgiver_verb_is_on_the_wire_before_the_next_line() {
    let mut c = Client::new(0);
    let cases = [
        (
            greeting(),
            "SelectActiveQuest(1)",
            format!("QuestgiverComplete {{ npc: {NPC}, quest: 5 }}"),
        ),
        (
            greeting(),
            "SelectAvailableQuest(1)",
            format!("QuestgiverQuery {{ npc: {NPC}, quest: 6 }}"),
        ),
        (
            greeting(),
            "SelectAvailableQuest(2)",
            format!("QuestgiverComplete {{ npc: {NPC}, quest: 7 }}"),
        ),
        (
            detail(),
            "AcceptQuest()",
            format!("QuestgiverAccept {{ npc: {NPC}, quest: 100 }}"),
        ),
        (
            progress(),
            "CompleteQuest()",
            format!("QuestgiverRequestReward {{ npc: {NPC}, quest: 100 }}"),
        ),
        (
            reward(0),
            "GetQuestReward()",
            format!("QuestgiverChooseReward {{ npc: {NPC}, quest: 100, choice: 0 }}"),
        ),
        (
            detail(),
            "DeclineQuest()",
            format!("QuestgiverHello {{ npc: {NPC} }}"),
        ),
    ];
    for (view, verb, packet) in cases {
        c.open(view);
        c.run(&format!("{verb} RandomRoll(1, 100)")).unwrap();
        assert_eq!(c.sent(), [packet, ROLL.to_string()], "{verb}");
        c.drain();
        assert!(c.sent().is_empty(), "{verb}: nothing waits for the drain");
    }
}

/// The already-acted latch: each of these verbs sets it as it sends (`0x5012f6`, `0x50136a`,
/// `0x5013c9`, `0x5015a4`, `0x501643`, `0x501549`) and refuses while it is set (`0x5012b3`,
/// `0x501333`, `0x5013b0`, `0x501570`, `0x5015f5`, `0x501401`), until the next panel clears it.
#[test]
fn the_acted_latch_takes_one_verb_per_panel() {
    let mut c = Client::new(0);
    for (view, verb) in [
        (greeting(), "SelectActiveQuest(1)"),
        (greeting(), "SelectAvailableQuest(1)"),
        (progress(), "CompleteQuest()"),
        (reward(2), "GetQuestReward(1)"),
        (detail(), "DeclineQuest()"),
    ] {
        c.open(view);
        c.run(&format!("{verb} {verb}")).unwrap();
        c.drain();
        assert_eq!(c.sent().len(), 1, "{verb} twice sends once");
    }
    // A select latches the panel against the other verbs too.
    c.open(greeting());
    c.run("SelectAvailableQuest(1) SelectActiveQuest(1)")
        .unwrap();
    c.drain();
    assert_eq!(c.sent().len(), 1);
}

/// Each panel verb tests the dialog kind first: `AcceptQuest` 1 (`0x501380`), `CompleteQuest` 2
/// (`0x501560`), `GetQuestReward` 3 (`0x5015b0`); off its panel it does nothing.
#[test]
fn each_panel_verb_does_nothing_off_its_panel() {
    let mut c = Client::new(0);
    for (view, verb) in [
        (progress(), "AcceptQuest()"),
        (reward(0), "AcceptQuest()"),
        (detail(), "CompleteQuest()"),
        (reward(0), "CompleteQuest()"),
        (detail(), "GetQuestReward()"),
        (progress(), "GetQuestReward()"),
    ] {
        c.open(view);
        c.run(verb).unwrap();
        c.drain();
        assert!(c.sent().is_empty(), "{verb}");
    }
}

/// `0x5015b0` tests the latch (`0x5015f5`) before the choice's range (`0x501608`), so a second
/// call with a bad choice returns quietly; a first one raises
/// `Invalid reward choice in GetQuestReward([choice])` and sends nothing.
#[test]
fn the_reward_latch_comes_before_the_choice_range() {
    let mut c = Client::new(0);
    c.open(reward(2));
    c.run("GetQuestReward(2) GetQuestReward(5)").unwrap();
    c.drain();
    assert_eq!(
        c.sent(),
        [format!(
            "QuestgiverChooseReward {{ npc: {NPC}, quest: 100, choice: 1 }}"
        )]
    );
    c.open(reward(2));
    let e = c.run("GetQuestReward(5)").unwrap_err();
    assert!(
        e.contains("Invalid reward choice in GetQuestReward([choice])"),
        "{e}"
    );
    c.drain();
    assert!(c.sent().is_empty());
}

/// `ConfirmAcceptQuest` (`0x5021b0`) sends the held quest id inside its call (`0x5021f4`).
#[test]
fn the_escort_confirm_is_on_the_wire_before_the_next_line() {
    let mut c = Client::new(0);
    {
        let world = c.app.world_mut();
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(0x9, "Bob".into(), None);
        world
            .resource_mut::<crate::ui_quest_share::QuestShare>()
            .set_confirm(QuestConfirmAccept {
                quest_id: 5,
                title: "Escort".into(),
                sender: 0x9,
            });
        world
            .run_system_cached(crate::ui_quest_share::feed_quest_share)
            .expect("the feed runs");
    }
    c.sent();
    c.run("ConfirmAcceptQuest() RandomRoll(1, 100)").unwrap();
    assert_eq!(c.sent(), ["QuestConfirmAccept { quest: 5 }", ROLL]);
}

/// `AbandonQuest` (`0x4dfe00` → `0x4df070`, the send at `0x4df0ca`) and `QuestLogPushQuest`
/// (`0x4e1340`, the send at `0x4e13b7`) send inside their calls.
#[test]
fn the_log_verbs_are_on_the_wire_before_the_next_line() {
    let mut c = Client::new(0);
    {
        let mut script = c.app.world_mut().non_send_resource_mut::<UiScript>();
        let mut q = crate::ui_quest_log::fixture::quest(7, "Deliver the Message", 1);
        q.record.pushable = true;
        crate::ui_quest_log::fixture::seat(&mut script, &[q]);
        script.set_party(benilla_ui::script::PartyState {
            members: vec![benilla_ui::script::PartyMemberInfo {
                name: "Mate".into(),
                guid: 0x300,
            }],
            ..Default::default()
        });
    }
    c.run("SelectQuestLogEntry(2) QuestLogPushQuest() RandomRoll(1, 100)")
        .unwrap();
    assert_eq!(c.sent(), ["PushQuestToParty { quest: 7 }", ROLL]);
    c.run("SetAbandonQuest() AbandonQuest() RandomRoll(1, 100)")
        .unwrap();
    assert_eq!(c.sent(), ["QuestlogRemove { slot: 0 }", ROLL]);
}
