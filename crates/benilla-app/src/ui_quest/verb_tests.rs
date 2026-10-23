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
        Self::streaming(Some(npc_flags))
    }

    /// [`Self::new`], with `NPC` streamed with `npc_flags`, or not at all for `None`.
    pub(crate) fn streaming(npc_flags: Option<u32>) -> Self {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        world.spawn((SelfPlayer, Guid(ME), ObjectStore::default()));
        if let Some(flags) = npc_flags {
            // 2 = `OBJECT_FIELD_TYPE`, a creature's mask 9; 147 = `UNIT_NPC_FLAGS`.
            let fields = benilla_protocol::ObjectFields::from_pairs(&[(2, 9), (147, flags)]);
            let npc = world.spawn((Guid(NPC), ObjectStore(fields))).id();
            world.resource_mut::<GuidIndex>().0.insert(NPC, npc);
        }
        world.insert_non_send_resource(UiScript::new().expect("a VM"));
        Self { app, rx }
    }

    /// Stream `guid` with the type mask `mask` (`OBJECT_FIELD_TYPE`).
    pub(crate) fn stream(&mut self, guid: u64, mask: u32) {
        let world = self.app.world_mut();
        let fields = benilla_protocol::ObjectFields::from_pairs(&[(2, mask)]);
        let e = world.spawn((Guid(guid), ObjectStore(fields))).id();
        world.resource_mut::<GuidIndex>().0.insert(guid, e);
    }

    /// Open `view` on `NPC` and run the feed, which pushes it to the VM and fires its event.
    pub(crate) fn open(&mut self, view: QuestView) {
        self.open_on(NPC, view);
    }

    /// [`Self::open`] on `source`.
    pub(crate) fn open_on(&mut self, source: u64, view: QuestView) {
        self.app
            .world_mut()
            .resource_mut::<QuestGiver>()
            .open(source, view);
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

    /// `lua`'s string value, read with no host lent.
    pub(crate) fn eval(&self, lua: &str) -> String {
        self.app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<String>(lua)
            .expect("the chunk runs")
    }

    /// Take the active player out of the world.
    pub(crate) fn drop_player(&mut self) {
        let world = self.app.world_mut();
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(world)
            .expect("the player");
        world.despawn(me);
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
        assert!(c.sent().is_empty(), "{verb}: nothing waits for a drain");
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
        assert_eq!(c.sent().len(), 1, "{verb} twice sends once");
    }
    // A select latches the panel against the other verbs too.
    c.open(greeting());
    c.run("SelectAvailableQuest(1) SelectActiveQuest(1)")
        .unwrap();
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
        crate::net::handlers::dispatch(
            world,
            vec![benilla_protocol::SessionEvent::QuestConfirmAccept(
                QuestConfirmAccept {
                    quest_id: 5,
                    title: "Escort".into(),
                    sender: 0x9,
                },
            )],
        );
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

// ── DeclineQuest and CloseQuest ──────────────────────────────────────────────

const SHARER: u64 = 0x0000_0000_0000_002A; // HIGHGUID_PLAYER: a zero high word

/// A detail panel on `source` with `0xbe0824` at `close_on_cancel`; the sends and whether the
/// window is still open after `lua`, its drain and the flush.
fn decline_after(
    npc_flags: Option<u32>,
    source: u64,
    close_on_cancel: u32,
    lua: &str,
) -> (Vec<String>, bool) {
    decline_on(npc_flags, (source, None), close_on_cancel, lua)
}

/// [`decline_after`] with the source streamed under the type mask beside it, if any.
fn decline_on(
    npc_flags: Option<u32>,
    (source, mask): (u64, Option<u32>),
    close_on_cancel: u32,
    lua: &str,
) -> (Vec<String>, bool) {
    let mut c = Client::streaming(npc_flags);
    if let Some(mask) = mask {
        c.stream(source, mask);
    }
    c.open_on(source, detail());
    // The latch is written by the packet, after the open (as `net::quest_detail` does).
    c.app
        .world_mut()
        .resource_mut::<QuestGiver>()
        .close_on_cancel = close_on_cancel;
    c.run(lua).unwrap();
    let open = c.app.world().resource::<QuestGiver>().is_open();
    (c.sent(), open)
}

/// Both verbs reach the teardown `0x501130(0,1)`: `CloseQuest` directly, `DeclineQuest` through
/// its player leg.
#[test]
fn every_way_out_of_a_shared_quest_answers_the_sharer() {
    let decline = format!(
        "{:?}",
        ClientCommand::QuestPushResult {
            sharer: SHARER,
            msg: benilla_protocol::messages::QuestShareMsg::DECLINE_QUEST,
        }
    );
    for verb in ["DeclineQuest()", "CloseQuest()"] {
        // A player's mask, 0x19.
        let (sent, open) = decline_on(None, (SHARER, Some(0x19)), 0, verb);
        assert_eq!(
            sent,
            std::slice::from_ref(&decline),
            "{verb} must answer the sharer"
        );
        assert!(!open, "{verb}");
    }
}

/// A plain questgiver gets `CMSG_QUESTGIVER_HELLO` (`0x501526`), a gossip-flagged one
/// `CMSG_GOSSIP_HELLO` (`0x501474`), whatever `0xbe0824` says (`0x50143b` before `0x5014a2`).
#[test]
fn a_decline_reopens_the_givers_list() {
    use crate::target::cursor_mode::npc_flags;
    let hello = format!("QuestgiverHello {{ npc: {NPC} }}");
    let gossip = format!("GossipHello {{ guid: {NPC} }}");
    assert_eq!(
        decline_after(Some(0), NPC, 0, "DeclineQuest()"),
        (vec![hello], true)
    );
    for latch in [0, 1] {
        assert_eq!(
            decline_after(Some(npc_flags::GOSSIP), NPC, latch, "DeclineQuest()"),
            (vec![gossip.clone()], true),
            "latch {latch}"
        );
    }
}

/// `0xbe0824`'s one reader is `0x5014a2`: set, a plain unit's decline takes the teardown
/// (`0x5014aa`), silently.
#[test]
fn a_latched_decline_closes_the_window_silently() {
    assert_eq!(
        decline_after(Some(0), NPC, 1, "DeclineQuest()"),
        (Vec::new(), false)
    );
}

/// `CloseQuest` tears the window down; `DeclineQuest` bails first (`0x50142c`) and leaves it.
#[test]
fn an_unstreamed_giver_ends_the_session_silently() {
    assert_eq!(
        decline_after(None, NPC, 0, "CloseQuest()"),
        (Vec::new(), false)
    );
    assert_eq!(
        decline_after(None, NPC, 0, "DeclineQuest()"),
        (Vec::new(), true)
    );
}

#[test]
fn declining_an_item_quest_closes_silently() {
    const ITEM: u64 = 0x4000_0000_0000_0099; // HIGHGUID_ITEM
                                             // An item's mask, 3.
    assert_eq!(
        decline_on(None, (ITEM, Some(3)), 0, "DeclineQuest()"),
        (Vec::new(), false)
    );
}

/// ESC, the X and the greeting's Goodbye all reach `CloseQuest` through `QuestFrame_OnHide`
/// (`QuestFrame.lua:293`), which sends nothing for an NPC.
#[test]
fn close_quest_on_an_npc_is_network_silent() {
    use crate::target::cursor_mode::npc_flags;
    for flags in [0, npc_flags::GOSSIP] {
        for latch in [0, 1] {
            assert_eq!(
                decline_after(Some(flags), NPC, latch, "CloseQuest()"),
                (Vec::new(), false),
                "flags {flags:#x}, latch {latch}"
            );
        }
    }
}

/// The acted latch `0xbe0844` refuses the second `DeclineQuest` and the `AcceptQuest`, and the
/// re-opening leg leaves the window up for the server's answer.
#[test]
fn a_reopening_decline_keeps_the_window_and_latches_acted() {
    assert_eq!(
        decline_after(
            Some(0),
            NPC,
            0,
            "DeclineQuest() DeclineQuest() AcceptQuest()"
        ),
        (vec![format!("QuestgiverHello {{ npc: {NPC} }}")], true)
    );
}

/// `AcceptQuest` closes the window as it sends (`0x5013d3`), before any answer.
#[test]
fn an_accept_closes_the_window_on_the_click() {
    assert_eq!(
        decline_after(Some(0), NPC, 0, "AcceptQuest()"),
        (
            vec![format!("QuestgiverAccept {{ npc: {NPC}, quest: 100 }}")],
            false
        )
    );
}

// ── The pools, the row past them and the confirm ─────────────────────────────

/// The two pools (`0xbdfe60`, `0xbde690`) are written by a greeting and read by the selects
/// alone, so they outlive the greeting panel, and the window's close, which zeroes the giver's
/// cell `[0xbe0810]` (`0x501201`) and not the pools: a select then sends for guid 0.
#[test]
fn the_pools_outlive_the_greeting_panel_and_the_window() {
    let mut c = Client::new(0);
    c.open(greeting());
    c.open(detail());
    c.run("SelectAvailableQuest(1)").unwrap();
    assert_eq!(
        c.sent(),
        [format!("QuestgiverQuery {{ npc: {NPC}, quest: 6 }}")]
    );

    c.open(greeting());
    c.run("CloseQuest()").unwrap();
    c.sent();
    c.run("SelectActiveQuest(1)").unwrap();
    assert_eq!(c.sent(), ["QuestgiverComplete { npc: 0, quest: 5 }"]);
}

/// The bound is `cmp index,count; ja` on the index less one (`0x5012a8`, `0x501328`), so the
/// 1-based row one past the pool is taken: the cleared row (`0x500c46`), quest 0, not one-click.
/// Two past it, and row 0, whose index wraps, send nothing.
#[test]
fn the_row_one_past_each_pool_is_its_cleared_row() {
    for (lua, sent) in [
        (
            "SelectActiveQuest(2)",
            vec![format!("QuestgiverComplete {{ npc: {NPC}, quest: 0 }}")],
        ),
        (
            "SelectAvailableQuest(3)",
            vec![format!("QuestgiverQuery {{ npc: {NPC}, quest: 0 }}")],
        ),
        ("SelectActiveQuest(3)", Vec::new()),
        ("SelectAvailableQuest(4)", Vec::new()),
        ("SelectAvailableQuest(0)", Vec::new()),
    ] {
        let mut c = Client::new(0);
        c.open(greeting());
        c.run(lua).unwrap();
        assert_eq!(c.sent(), sent, "{lua}");
    }
}

/// The world-enter reset zeroes both pools' rows and counts (`0x500b21`-`0x500b5c`).
#[test]
fn world_enter_clears_the_pools() {
    let mut c = Client::new(0);
    c.open(greeting());
    super::net::world_enter_reset(&mut c.app.world_mut().resource_mut::<QuestGiver>());
    c.run("SelectActiveQuest(1)").unwrap();
    assert_eq!(
        c.sent(),
        [format!("QuestgiverComplete {{ npc: {NPC}, quest: 0 }}")]
    );
}

/// `ConfirmAcceptQuest` sends `[0xbe0840]` with no test (`0x5021e0`): 0 before any confirm, and
/// the last confirm's quest, which its packet writes (`0x5019c7`) and nothing clears.
#[test]
fn confirm_accept_quest_sends_the_last_confirm_or_zero() {
    let mut c = Client::new(0);
    c.run("ConfirmAcceptQuest()").unwrap();
    assert_eq!(c.sent(), ["QuestConfirmAccept { quest: 0 }"]);
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![benilla_protocol::SessionEvent::QuestConfirmAccept(
            QuestConfirmAccept {
                quest_id: 5,
                title: "Escort".into(),
                sender: 0x9,
            },
        )],
    );
    c.run("ConfirmAcceptQuest() ConfirmAcceptQuest()").unwrap();
    assert_eq!(
        c.sent(),
        [
            "QuestConfirmAccept { quest: 5 }",
            "QuestConfirmAccept { quest: 5 }"
        ]
    );
}

/// `DeclineQuest` resolves its source first (`0x501423`, `0x50142c`) and does nothing for one
/// out of view; a streamed item or sharer takes the teardown by its type mask (`0x501496`,
/// `0x5014a0`).
#[test]
fn a_decline_needs_its_source_in_view() {
    const ITEM: u64 = 0x4000_0000_0000_0099;
    for source in [SHARER, ITEM] {
        let (sent, open) = decline_after(None, source, 0, "DeclineQuest()");
        assert_eq!((sent, open), (Vec::new(), true), "{source:#x} out of view");
    }
}

/// The gossip leg needs the active player too (`0x50144e`-`0x50146b`); without one a gossip
/// unit takes the plain leg, `CMSG_QUESTGIVER_HELLO` (`0x501526`).
#[test]
fn a_gossip_units_decline_with_no_player_reopens_the_quest_list() {
    use crate::target::cursor_mode::npc_flags;
    let mut c = Client::new(npc_flags::GOSSIP);
    c.open(detail());
    c.drop_player();
    c.run("DeclineQuest()").unwrap();
    assert_eq!(c.sent(), [format!("QuestgiverHello {{ npc: {NPC} }}")]);
}

// ── The closes in their calls ────────────────────────────────────────────────

/// `DeclineQuest`'s teardown `0x501130(0,1)` (`0x5014b1`) zeroes the cell inside the call
/// (`0x501201`) and leaves the latch clear, while the panel kind `[0xbe0818]` stays: an
/// `AcceptQuest` on the chunk's next line still sends, for guid 0.
#[test]
fn a_shared_quests_decline_zeroes_the_cell_before_the_next_line() {
    let mut c = Client::streaming(None);
    c.stream(SHARER, 0x19);
    c.open_on(SHARER, detail());
    c.run("DeclineQuest() AcceptQuest()").unwrap();
    assert_eq!(
        c.sent(),
        [
            format!(
                "{:?}",
                ClientCommand::QuestPushResult {
                    sharer: SHARER,
                    msg: benilla_protocol::messages::QuestShareMsg::DECLINE_QUEST,
                }
            ),
            "QuestgiverAccept { npc: 0, quest: 100 }".to_string(),
        ]
    );
}

/// `CloseQuest` (`0x501a10` → `0x501130(0,1)`) runs the teardown in its call too, and the panel
/// verbs read the kind it leaves.
#[test]
fn close_quest_zeroes_the_cell_before_the_next_line() {
    let mut c = Client::new(0);
    c.open(detail());
    c.run("CloseQuest() AcceptQuest()").unwrap();
    assert_eq!(c.sent(), ["QuestgiverAccept { npc: 0, quest: 100 }"]);
    assert!(!c.app.world().resource::<QuestGiver>().is_open());

    let mut c = Client::new(0);
    c.open(progress());
    c.run("CloseQuest()").unwrap();
    c.sent();
    c.run("CompleteQuest()").unwrap();
    assert_eq!(c.sent(), ["QuestgiverRequestReward { npc: 0, quest: 100 }"]);
}

/// A greeting of 32 available quests.
fn full_greeting() -> QuestView {
    QuestView::Greeting(QuestGiverList {
        npc: NPC,
        greeting: "Hail.".into(),
        emote_delay: 0,
        emote: 0,
        quests: (1..=32).map(|q| row(q, 5)).collect(),
    })
}

/// On a full pool the row past it is read from the cells after the pool, which the reference
/// sends and latches; benilla sends nothing and latches as the send would (`0x501313`).
#[test]
fn the_row_past_a_full_pool_sends_nothing_and_latches() {
    let mut c = Client::new(0);
    c.open(full_greeting());
    c.run("SelectAvailableQuest(33) SelectAvailableQuest(1)")
        .unwrap();
    assert!(c.sent().is_empty());
}
