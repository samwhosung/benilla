//! The questgiver packet handlers: each fills the [`QuestGiver`] the feed reads, and each panel
//! packet replaces the open view.

use benilla_protocol::messages::{
    QuestComplete, QuestDetails, QuestGiverList, QuestOfferReward, QuestRequestItems, QuestShareMsg,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{QuestGiver, QuestLine, QuestLines};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::query_cache::Ask;
use crate::ui_action::UiError;
use crate::ui_quest_log::QuestLog;

/// One handler per kind, plus the session-end and world-enter listeners.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::QuestGiverStatus, on_giver_status)
        .net_handler(K::QuestGreeting, on_greeting)
        .net_handler(K::QuestDetail, on_detail)
        .net_handler(K::QuestProgress, on_progress)
        .net_handler(K::QuestOffer, on_offer)
        .net_handler(K::QuestComplete, on_complete)
        .net_handler(K::QuestObjectiveKill, on_objective_kill)
        .net_handler(K::QuestObjectiveItem, on_objective_item)
        .net_handler(K::QuestObjectivesComplete, on_objectives_complete)
        .net_handler(K::QuestFailed, on_failed)
        .net_handler(K::QuestLogFull, on_log_full)
        .net_handler(K::QuestGiverInvalid, on_giver_invalid)
        .net_handler(K::QuestGiverFailed, on_giver_failed)
        .net_handler(K::QuestTemplate, on_template_giver_failures)
        .net_handler(K::Disconnected, on_session_end)
        .net_handler(K::Connected, on_login)
        .net_handler(K::Worldport, on_worldport);
}

/// The reference's world-enter reset `0x500af0`, run by the enter-world cascade `0x4908c0` on a
/// login and on every worldport, is the `0xbe0824` latch's only clear.
fn on_login(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::Connected { .. } = ev {
        world_enter_reset(&mut quest);
    }
}

/// The same reset on a worldport.
fn on_worldport(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::Worldport { .. } = ev {
        world_enter_reset(&mut quest);
    }
}

fn world_enter_reset(quest: &mut QuestGiver) {
    quest.close_on_cancel = 0;
    // The same reset zeroes the chosen reward (`0x500b15`).
    quest.chosen_reward = 0;
}

fn on_giver_status(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestGiverStatus { npc, status } = ev {
        quest_giver_status(npc, status, &mut quest);
    }
}

fn on_greeting(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::QuestGreeting(list) = ev {
        commands.queue(move |world: &mut World| quest_greeting(world, list));
    }
}

fn on_detail(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::QuestDetail(d) = ev {
        commands.queue(move |world: &mut World| quest_detail(world, d));
    }
}

fn on_progress(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::QuestProgress(p) = ev {
        commands.queue(move |world: &mut World| quest_progress(world, p));
    }
}

fn on_offer(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::QuestOffer(o) = ev {
        commands.queue(move |world: &mut World| quest_offer(world, o));
    }
}

/// The turn-in's tail (`0x5dc6d0`-`0x5dc6e9`): `0x501130(next, 0)`, where `next` is whether the
/// quest's cached record names a quest that follows (`+0x24`). A chain's next step leaves the
/// panel up, for the server's next panel; a miss or a last step closes it.
fn on_complete(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    mut lines: ResMut<QuestLines>,
    quest_log: Res<QuestLog>,
    net_commands: Res<NetCommands>,
    mut commands: Commands,
) {
    if let SessionEvent::QuestComplete(c) = ev {
        // The turn-in looks the quest up with a callback (`0x5dc4cb`).
        let chained = quest_log
            .template(c.quest_id, Ask::Event(&net_commands))
            .is_some_and(|t| t.next_quest_in_chain != 0);
        quest_complete(c, &mut quest, &mut lines);
        if !chained {
            crate::ui_session::queue(&mut commands, close_without_report);
        }
    }
}

fn on_objective_kill(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestObjectiveKill {
        quest_id: _,
        entry,
        count,
        required,
    } = ev
    {
        quest_objective_kill(entry, count, required, &mut quest);
    }
}

fn on_objective_item(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestObjectiveItem { item_id, count } = ev {
        quest_objective_item(item_id, count, &mut quest);
    }
}

fn on_objectives_complete(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestObjectivesComplete { quest_id } = ev {
        quest_objectives_complete(quest_id, &mut quest);
    }
}

fn on_failed(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    quest_log: Res<QuestLog>,
    self_q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
) {
    if let SessionEvent::QuestFailed { quest_id, timed } = ev {
        let store = self_q.iter().next().map(|s| &s.0);
        quest_failed(quest_id, timed, store, script, &quest_log, &mut quest);
    }
}

fn on_log_full(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestLogFull = ev {
        quest_log_full(&mut quest);
    }
}

fn on_giver_invalid(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    mut commands: Commands,
) {
    if let SessionEvent::QuestGiverInvalid { reason } = ev {
        quest_giver_invalid(reason, &mut quest);
        crate::ui_session::queue(&mut commands, close_without_report);
    }
}

fn on_giver_failed(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    mut quest_log: ResMut<QuestLog>,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
    net_commands: Res<NetCommands>,
    mut commands: Commands,
) {
    if let SessionEvent::QuestGiverFailed { quest_id, reason } = ev {
        let latched = script.is_some_and(|s| s.quest_log_latched(quest_id));
        quest_giver_failed(
            quest_id,
            reason,
            latched,
            &mut quest,
            &mut quest_log,
            &net_commands,
        );
        crate::ui_session::queue(&mut commands, close_without_report);
    }
}

/// The record callback `0x5dc920`: each failure that found its quest uncached prints when the
/// record lands, by the reason as it stands then (`0x5dc968`) and the latch as it stands
/// (`0x5dc97e`); a refusal prints nothing (`0x5dc944`).
fn on_template_giver_failures(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    mut quest_log: ResMut<QuestLog>,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
) {
    let SessionEvent::QuestTemplate(t) = ev else {
        return;
    };
    let waiting = quest_log
        .giver_failures
        .iter()
        .filter(|&&id| id == t.quest_id)
        .count();
    if waiting == 0 {
        return;
    }
    quest_log.giver_failures.retain(|&id| id != t.quest_id);
    if t.title.is_empty() {
        return;
    }
    let latched = script.is_some_and(|s| s.quest_log_latched(t.quest_id));
    for _ in 0..waiting {
        giver_failed_lines(&t.title, quest_log.giver_failed_reason, latched, &mut quest);
    }
}

/// The handlers' close, `0x501130(0, 0)`: the turn-in of a quest with no next step (`0x5dc6e5`)
/// and the two refusals (`0x5dbd03`, `0x5dc8bc`-`0x5dc911`).
fn close_without_report(world: &mut World) {
    super::close_core(world, false);
}

/// An open questgiver panel dies with the socket.
fn on_session_end(In(_): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    quest.clear_session();
}

/// `SMSG_QUESTGIVER_STATUS`: one guid's `dialog_status`, for the markers and the minimap dot. A
/// GameObject's answer is dropped, as the reference's `0x5dc9f0` resolves the guid by
/// `TYPEMASK_UNIT` (`0x5dca22`) and exits at `0x5dca2f`; vmangos answers GameObjects
/// (`QuestHandler.cpp:40`). The guid's shape gives the same partition. The reference's second
/// test, `UNIT_NPC_FLAGS & 0x2` on the unit, is not built; the marker query's teardown covers a
/// giver whose flag clears.
fn quest_giver_status(npc: u64, status: u32, quest: &mut QuestGiver) {
    use benilla_protocol::guid;
    if !(guid::is_player(npc) || guid::is_creature_or_pet(npc)) {
        debug!("net: dropping a non-unit questgiver status ({npc:#x} → {status}) — typemask 8");
        quest.refuse_status(npc, status);
        return;
    }
    quest.set_status(npc, status);
}

/// `SMSG_QUESTGIVER_QUEST_LIST`: the greeting panel's quest rows.
pub(crate) fn quest_greeting(world: &mut World, list: QuestGiverList) {
    debug!(
        "net: quest greeting on {:#x} — {} quests",
        list.npc,
        list.quests.len()
    );
    super::open_panel(world, list.npc, crate::ui_quest::QuestView::Greeting(list));
}

/// `SMSG_QUESTGIVER_QUEST_DETAILS`: the accept panel.
pub(crate) fn quest_detail(world: &mut World, d: QuestDetails) {
    debug!("net: quest detail — quest {} on {:#x}", d.quest_id, d.npc);
    // A share landing on an open window is refused by the client with `BUSY` (`0x5dbf85`), and
    // the open panel stays; the server's own busy test only knows about other shares.
    if benilla_protocol::guid::is_player(d.npc) && world.resource::<QuestGiver>().is_open() {
        debug!(
            "net: busy — refusing {:#x}'s shared quest {}",
            d.npc, d.quest_id
        );
        if let Some(net) = world.get_resource::<NetCommands>() {
            let _ = net.0.send(ClientCommand::QuestPushResult {
                sharer: d.npc,
                msg: QuestShareMsg::BUSY,
            });
        }
        return;
    }
    // The trailing flag is the latch, not part of the view.
    world.resource_mut::<QuestGiver>().close_on_cancel = d.auto_finish;
    super::open_panel(world, d.npc, crate::ui_quest::QuestView::Detail(d));
}

/// `SMSG_QUESTGIVER_REQUEST_ITEMS`: the progress panel.
fn quest_progress(world: &mut World, p: QuestRequestItems) {
    debug!(
        "net: quest progress — quest {} on {:#x} (complete: {})",
        p.quest_id, p.npc, p.is_complete
    );
    // The progress publisher `0x501070` writes `closeOnCancel` into the same latch (`0x50111a`).
    world.resource_mut::<QuestGiver>().close_on_cancel = p.close_on_cancel;
    super::open_panel(world, p.npc, crate::ui_quest::QuestView::Progress(p));
}

/// `SMSG_QUESTGIVER_OFFER_REWARD`: the reward panel.
fn quest_offer(world: &mut World, o: QuestOfferReward) {
    debug!(
        "net: quest reward offer — quest {} on {:#x}",
        o.quest_id, o.npc
    );
    // The `u32` after the reward text lands in the latch through DETAILS' publisher `0x500ef0`
    // (`0x50101a`).
    world.resource_mut::<QuestGiver>().close_on_cancel = o.auto_finish;
    super::open_panel(world, o.npc, crate::ui_quest::QuestView::Reward(o));
}

/// `SMSG_QUESTGIVER_QUEST_COMPLETE` (`0x5dc400`): the turn-in's chat lines, in the reference's
/// order; the window then closes through the core. The XP, money and items themselves arrive by `UPDATE_OBJECT`
/// and `ITEM_PUSH_RESULT`.
fn quest_complete(c: QuestComplete, quest: &mut QuestGiver, lines: &mut QuestLines) {
    // The reference plays the `QUESTCOMPLETED` fanfare on this packet.
    quest.completed_fanfare = true;
    debug!(
        "net: quest {} complete — +{} XP, +{} copper, {} item(s)",
        c.quest_id,
        c.xp,
        c.money,
        c.items.len()
    );
    lines.push(QuestLine::Completed(c.quest_id));
    if c.xp != 0 {
        lines.push(QuestLine::Experience(c.xp));
    }
    // A signed test (`0x5dc50b`).
    if (c.money as i32) > 0 {
        lines.push(QuestLine::Money(c.money));
    }
    for &(item, _count) in &c.items {
        lines.push(QuestLine::Item(item));
    }
    if quest.chosen_reward != 0 {
        lines.push(QuestLine::Item(std::mem::take(&mut quest.chosen_reward)));
    }
    // A turn-in can move every giver's marker, so the reference re-asks from here.
    quest.bump_reask();
}

/// `SMSG_QUESTUPDATE_ADD_KILL`: its progress toast is the quest log's handler's
/// (`crate::ui_quest_log`), and the reference prints no chat line for it.
fn quest_objective_kill(entry: u32, count: u32, required: u32, quest: &mut QuestGiver) {
    debug!("net: quest kill/use objective {entry:#x} at {count}/{required}");
    quest.bump_reask();
}

/// `SMSG_QUESTUPDATE_ADD_ITEM`, which carries only the count added; its toast is resolved in
/// `crate::ui_quest_log`, from the bags as they stand when the packet arrives.
fn quest_objective_item(item_id: u32, count: u32, quest: &mut QuestGiver) {
    debug!("net: quest item objective {item_id} +{count}");
    quest.bump_reask();
}

/// `SMSG_QUESTUPDATE_COMPLETE` (0x198): its `ERR_QUEST_OBJECTIVE_COMPLETE_S` toast (kind 1, never
/// chat) is the quest log's handler's, as the progress toasts are.
fn quest_objectives_complete(quest_id: u32, quest: &mut QuestGiver) {
    debug!("net: quest {quest_id} objectives complete");
    // The turn-in `?` can turn gold with no quest-log change, so the reference re-asks here.
    quest.bump_reask();
}

/// `SMSG_QUESTUPDATE_FAILED` (`0x196`) or `_FAILEDTIMER` (`0x197`): `ERR_QUEST_FAILED_S` (msg
/// `0x8b`) in chat, named by the quest's title, unless the quest was already done: its row
/// latched or its slot failed (`0x5e5c16`-`0x5e5c81`), as read before the timer arm latches the
/// row (`0x4df550`, `0x5e5cd9`). Nothing shows without a cached template (`0x5e5ca0`).
fn quest_failed(
    quest_id: u32,
    timed: bool,
    store: Option<&benilla_protocol::ObjectFields>,
    mut script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    quest_log: &QuestLog,
    quest: &mut QuestGiver,
) {
    use benilla_protocol::messages::{quest_slot_state, PLAYER_QUEST_LOG_SLOTS};
    debug!("net: quest {quest_id} failed (timed: {timed})");
    let latched = script
        .as_ref()
        .is_some_and(|s| s.quest_log_latched(quest_id));
    let failed = store.is_some_and(|store| {
        (0..PLAYER_QUEST_LOG_SLOTS)
            .filter_map(|slot| store.player_quest_log(slot))
            .any(|s| s.quest_id == quest_id && s.state & quest_slot_state::FAIL != 0)
    });
    // Only the timed arm latches (`0x5e5cd9`), and the latch's quest-giver refresh (`0x4df531`)
    // reaches the givers through the feed; the plain arm (`0x5e5cde`) asks no giver again.
    if timed {
        if let Some(script) = script.as_mut() {
            script.quest_log_latch_quest(quest_id);
        }
    }
    // Read without a callback (`0x5e5ca0`).
    match quest_log.template(quest_id, Ask::Read) {
        Some(t) if !(latched || failed) => {
            quest.push_message(UiError::s("ERR_QUEST_FAILED_S", t.title.clone()));
        }
        Some(_) => {}
        None => {
            debug!("net: quest {quest_id} has no cached template — the reference shows nothing")
        }
    }
}

/// `SMSG_QUESTLOG_FULL`: the reference's `0x195` arm only raises msg 153, `ERR_QUEST_LOG_FULL`, a
/// red error line, and leaves the panel open.
fn quest_log_full(quest: &mut QuestGiver) {
    debug!("net: quest log full");
    quest.push_message(UiError::key("ERR_QUEST_LOG_FULL"));
}

/// `SMSG_QUESTGIVER_QUEST_INVALID` (`0x5dbca0`): a `QuestFailedReason` and no quest id, shown in
/// chat through `questgiver_invalid_key`. Both refusal handlers then close the window with
/// `0x501130(0,0)`, which zeroes the giver guid (`0xbe0810`) and fires `QUEST_FINISHED`.
fn quest_giver_invalid(reason: u32, quest: &mut QuestGiver) {
    debug!("net: questgiver refused to offer the quest (reason {reason})");
    quest.push_message(UiError::key(crate::ui_quest::questgiver_invalid_key(
        reason,
    )));
}

/// `SMSG_QUESTGIVER_QUEST_FAILED` (`0x5dc840`): the reason is kept as a global (`0xc4d794`) and
/// the line's `%s` is the title of the quest's cached record, looked up with a callback
/// (`0x5dc875`). On a miss nothing prints until the record lands ([`on_template_giver_failures`]);
/// either way the window closes as on an invalid quest (`0x5dc90d`).
fn quest_giver_failed(
    quest_id: u32,
    reason: u32,
    latched: bool,
    quest: &mut QuestGiver,
    quest_log: &mut QuestLog,
    net_commands: &NetCommands,
) {
    debug!("net: quest {quest_id} accept failed (reason {reason})");
    quest_log.giver_failed_reason = reason;
    match quest_log.template(quest_id, Ask::Event(net_commands)) {
        Some(t) => {
            let title = t.title.clone();
            giver_failed_lines(&title, reason, latched, quest);
        }
        None => quest_log.giver_failures.push(quest_id),
    }
}

/// The reason's lines (`0x5dc884`-`0x5dc903`): a full bag (4, 0x32) its title line and msg 0,
/// `ERR_INV_FULL`, on the red surface; 0x11 its own; any other `ERR_QUEST_FAILED_S`, unless the
/// quest's row holds the expiry latch (`0x5dc89b`).
fn giver_failed_lines(title: &str, reason: u32, latched: bool, quest: &mut QuestGiver) {
    let key = crate::ui_quest::questgiver_failed_key(reason);
    if key == "ERR_QUEST_FAILED_S" && latched {
        return;
    }
    quest.push_message(UiError::s(key, title.to_string()));
    if matches!(reason, 4 | 50) {
        quest.push_message(UiError::key("ERR_INV_FULL"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x5e5ad0`'s failure arms print unless the quest is done, read before the timer arm
    /// latches its row (`0x5e5c16`, `0x5e5cd9`), so a second timer failure prints nothing.
    #[test]
    fn a_timed_failure_latches_and_a_done_quest_prints_no_failure() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<QuestGiver>();
        let mut log = QuestLog::default();
        log.insert_template(crate::ui_quest::test_template(7, "Deliver the Message", 0));
        world.insert_resource(log);
        let mut script = benilla_ui::script::UiScript::new().unwrap();
        let mut q = crate::ui_quest_log::fixture::quest(7, "Deliver the Message", 1);
        q.record.objectives[0] = crate::ui_quest_log::fixture::kill(40, 3);
        crate::ui_quest_log::fixture::seat(&mut script, &[q]);
        world.insert_non_send_resource(script);
        let epoch = world.resource::<QuestGiver>().reask_epoch();
        let mut fail = |timed| {
            world
                .run_system_once_with(on_failed, SessionEvent::QuestFailed { quest_id: 7, timed })
                .unwrap();
            let mut quest = world.resource_mut::<QuestGiver>();
            let lines = quest.take_messages().len();
            let reasked = quest.reask_epoch();
            let mut script = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
            let refreshes = script.take_quest_giver_refreshes();
            (lines, script.quest_log_latched(7), reasked, refreshes)
        };
        assert_eq!(
            fail(false),
            (1, false, epoch, 0),
            "0x196 prints, latches nothing and asks no giver"
        );
        assert_eq!(
            fail(true),
            (1, true, epoch, 1),
            "0x197 prints, read before its latch, whose refresh the feed carries"
        );
        assert_eq!(
            fail(true),
            (0, true, epoch, 1),
            "done: the row's latch, which a latched row takes again (`0x4df64e`)"
        );
        assert_eq!(fail(false), (0, true, epoch, 0));
    }

    /// A world with the giver and a wire, for the handlers that open a panel through the latch.
    fn quest_world() -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.init_resource::<QuestGiver>();
        world.insert_resource(NetCommands(tx));
        (world, rx)
    }

    /// Typemask 8 is `TYPEMASK_UNIT`, which a creature (`0x9`) and a player (`0x19`) both carry.
    #[test]
    fn a_gameobjects_dialog_status_is_dropped_and_a_creatures_is_not() {
        use benilla_protocol::messages::dialog_status;

        // A GameObject (Goldshire's Wanted Poster), a transport (an elevator), a creature and a
        // player.
        const POSTER: u64 = 0xf110_0000_0044_68db;
        const ELEVATOR: u64 = 0xf120_0000_0384_1092;
        const CREATURE: u64 = 0xf130_0000_0060_0abc;
        const PLAYER: u64 = 0x0000_0000_0000_0007;

        let mut quest = QuestGiver::default();
        for (guid, status) in [
            (POSTER, dialog_status::AVAILABLE),
            (ELEVATOR, dialog_status::AVAILABLE),
            (CREATURE, dialog_status::AVAILABLE),
            (PLAYER, dialog_status::REWARD2),
        ] {
            quest_giver_status(guid, status, &mut quest);
        }
        assert_eq!(
            quest.status(POSTER),
            None,
            "a GameObject GUID misses typemask bit 3 and the handler exits at 0x5dca2f"
        );
        assert_eq!(quest.status(ELEVATOR), None, "…and so does a transport GO");
        assert_eq!(
            quest.status(CREATURE),
            Some(dialog_status::AVAILABLE),
            "the control: a creature's answer is what this packet is FOR"
        );
        assert_eq!(
            quest.status(PLAYER),
            Some(dialog_status::REWARD2),
            "and a player passes the same typemask, as it does in the reference"
        );
    }

    /// `0x5dc400`'s order: the completion, the XP, the money, each packet item, then the chosen
    /// reward, which the turn-in spends.
    #[test]
    fn a_turn_in_raises_its_lines_in_the_reference_order() {
        let mut giver = QuestGiver {
            chosen_reward: 2_047,
            ..Default::default()
        };
        let mut lines = QuestLines::default();
        quest_complete(
            QuestComplete {
                quest_id: 7,
                xp: 450,
                money: 1_025,
                items: vec![(6_529, 1), (159, 5)],
            },
            &mut giver,
            &mut lines,
        );
        assert_eq!(
            lines.queued(),
            [
                QuestLine::Completed(7),
                QuestLine::Experience(450),
                QuestLine::Money(1_025),
                QuestLine::Item(6_529),
                QuestLine::Item(159),
                QuestLine::Item(2_047),
            ]
        );
        assert_eq!(giver.chosen_reward, 0, "the turn-in zeroes the latch");

        // No XP (a capped level) and no money: only the completion line.
        let mut lines = QuestLines::default();
        quest_complete(
            QuestComplete {
                quest_id: 7,
                xp: 0,
                money: 0,
                items: vec![],
            },
            &mut giver,
            &mut lines,
        );
        assert_eq!(lines.queued(), [QuestLine::Completed(7)]);
    }

    fn open_detail(quest_id: u32) -> QuestGiver {
        let mut giver = QuestGiver::default();
        giver.open(
            0x4000_0000_0000_0BAD,
            crate::ui_quest::QuestView::Detail(QuestDetails {
                npc: 0x4000_0000_0000_0BAD,
                quest_id,
                title: "A Threat Within".into(),
                details: String::new(),
                objectives: String::new(),
                auto_finish: 0,
                choices: vec![],
                rewards: vec![],
                money: 0,
                reward_spell: 0,
            }),
        );
        giver
    }

    /// Reason 13: the quest is already in the log.
    #[test]
    fn an_already_on_refusal_speaks() {
        let mut giver = open_detail(373);
        quest_giver_invalid(13, &mut giver);
        let msgs = giver.take_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[0].key),
            benilla_ui::messages::MsgKind::Chat
        );
        assert_eq!(msgs[0], UiError::key("ERR_QUEST_ALREADY_ON"));
    }

    /// Both refusals and the turn-in close the panel with `0x501130(0, 0)` (`0x5dbd03`,
    /// `0x5dc8bc`-`0x5dc911`, `0x5dc6e9`): the giver cleared, the latch released, and a sharer
    /// owed nothing.
    #[test]
    fn a_refusal_or_a_turn_in_closes_the_panel_without_a_report() {
        use bevy::ecs::system::RunSystemOnce;
        const SHARER: u64 = 0x0000_0000_0000_002A;
        for ev in [
            SessionEvent::QuestGiverInvalid { reason: 13 },
            SessionEvent::QuestGiverFailed {
                quest_id: 200,
                reason: 4,
            },
            SessionEvent::QuestComplete(QuestComplete {
                quest_id: 200,
                xp: 0,
                money: 0,
                items: vec![],
            }),
        ] {
            let (mut world, rx) = quest_world();
            world.init_resource::<QuestLog>();
            world.init_resource::<QuestLines>();
            world.init_resource::<crate::ui_session::InteractLatch>();
            quest_detail(&mut world, detail(SHARER, 200));
            assert!(world.resource::<QuestGiver>().is_open());
            let name = format!("{ev:?}");
            let _ = match ev {
                SessionEvent::QuestGiverInvalid { .. } => {
                    world.run_system_once_with(on_giver_invalid, ev)
                }
                SessionEvent::QuestGiverFailed { .. } => {
                    world.run_system_once_with(on_giver_failed, ev)
                }
                _ => world.run_system_once_with(on_complete, ev),
            };
            assert!(!world.resource::<QuestGiver>().is_open(), "{name}");
            assert_eq!(
                world.resource::<crate::ui_session::InteractLatch>().guid(),
                None,
                "{name}"
            );
            assert!(
                !rx.try_iter()
                    .any(|c| matches!(c, ClientCommand::QuestPushResult { .. })),
                "no decline owed: {name}"
            );
        }
    }

    /// A turn-in whose cached record names a next step (`+0x24`) calls `0x501130(1, 0)`
    /// (`0x5dc6de`), which returns at once (`0x5011e9`): the panel stays for the chain's next one.
    #[test]
    fn a_chained_turn_in_leaves_the_panel_up() {
        use bevy::ecs::system::RunSystemOnce;
        const NPC: u64 = 0xF130_0000_0000_0007;
        for (next, open) in [(201, true), (0, false)] {
            let (mut world, _rx) = quest_world();
            world.init_resource::<QuestLines>();
            world.init_resource::<crate::ui_session::InteractLatch>();
            let mut log = QuestLog::default();
            log.insert_template(crate::ui_quest::lines::test_template(200, "Step", next));
            world.insert_resource(log);
            quest_detail(&mut world, detail(NPC, 200));
            let _ = world.run_system_once_with(
                on_complete,
                SessionEvent::QuestComplete(QuestComplete {
                    quest_id: 200,
                    xp: 0,
                    money: 0,
                    items: vec![],
                }),
            );
            assert_eq!(
                world.resource::<QuestGiver>().is_open(),
                open,
                "next step {next}"
            );
        }
    }

    #[test]
    fn a_full_bag_refusal_names_the_quest_and_adds_the_inventory_line() {
        let mut giver = open_detail(373);
        let mut log = QuestLog::default();
        log.insert_template(crate::ui_quest::test_template(373, "A Threat Within", 0));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        quest_giver_failed(373, 4, false, &mut giver, &mut log, &commands);
        let msgs = giver.take_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[0].key),
            benilla_ui::messages::MsgKind::Chat
        );
        assert_eq!(
            msgs[0],
            UiError::s("ERR_QUEST_FAILED_BAG_FULL_S", "A Threat Within")
        );
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[1].key),
            benilla_ui::messages::MsgKind::Error
        );
        assert_eq!(msgs[1], UiError::key("ERR_INV_FULL"));
    }

    #[test]
    fn a_full_log_takes_the_red_line_and_leaves_the_panel_alone() {
        let mut giver = open_detail(373);
        quest_log_full(&mut giver);
        let msgs = giver.take_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[0].key),
            benilla_ui::messages::MsgKind::Error
        );
        assert_eq!(msgs[0], UiError::key("ERR_QUEST_LOG_FULL"));
        assert!(
            giver.is_open(),
            "the ref's 0x195 arm does not close the panel"
        );
    }

    /// The plain reason prints `ERR_QUEST_FAILED_S` only for a quest whose row holds no expiry
    /// latch (`0x5dc89b`); the bag and count reasons print whatever the latch (`0x5dc88c`,
    /// `0x5dc891`).
    #[test]
    fn a_latched_quest_takes_no_plain_refusal_line() {
        let mut log = QuestLog::default();
        log.insert_template(crate::ui_quest::test_template(373, "A Threat Within", 0));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let keys = |reason: u32, latched: bool, log: &mut QuestLog| {
            let mut giver = QuestGiver::default();
            quest_giver_failed(373, reason, latched, &mut giver, log, &commands);
            giver
                .take_messages()
                .into_iter()
                .map(|m| m.key)
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(0, false, &mut log), ["ERR_QUEST_FAILED_S"]);
        assert!(keys(0, true, &mut log).is_empty());
        assert_eq!(keys(17, true, &mut log), ["ERR_QUEST_FAILED_MAX_COUNT_S"]);
        assert_eq!(
            keys(50, true, &mut log),
            ["ERR_QUEST_FAILED_BAG_FULL_S", "ERR_INV_FULL"]
        );
    }

    /// An uncached quest asks for its record and prints nothing (`0x5dc87e` → `0x5dc90d`), not
    /// even with an open panel's title; the window still closes. The line comes when the record
    /// lands, once per failure, by the reason last received (`0xc4d794`, `0x5dc968`) and the
    /// latch then (`0x5dc97e`); a refusal prints nothing (`0x5dc944`).
    #[test]
    fn an_uncached_refusal_prints_when_the_record_lands() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut world, rx) = quest_world();
        world.init_resource::<QuestLog>();
        world.init_resource::<QuestLines>();
        world.init_resource::<crate::ui_session::InteractLatch>();
        quest_detail(&mut world, detail(0xF130_0000_0000_0007, 373));
        let failed = |world: &mut World, quest_id: u32, reason: u32| {
            world
                .run_system_once_with(
                    on_giver_failed,
                    SessionEvent::QuestGiverFailed { quest_id, reason },
                )
                .unwrap();
        };
        let landed = |world: &mut World, t: benilla_protocol::messages::QuestTemplate| {
            world
                .run_system_once_with(
                    on_template_giver_failures,
                    SessionEvent::QuestTemplate(Box::new(t)),
                )
                .unwrap();
            world.resource_mut::<QuestGiver>().take_messages()
        };
        failed(&mut world, 373, 0);
        assert!(world
            .resource_mut::<QuestGiver>()
            .take_messages()
            .is_empty());
        assert!(!world.resource::<QuestGiver>().is_open());
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::QuestQuery { quest: 373 })));
        failed(&mut world, 373, 17);
        assert_eq!(
            landed(
                &mut world,
                crate::ui_quest::test_template(373, "A Threat Within", 0)
            ),
            [
                UiError::s("ERR_QUEST_FAILED_MAX_COUNT_S", "A Threat Within"),
                UiError::s("ERR_QUEST_FAILED_MAX_COUNT_S", "A Threat Within")
            ],
            "both callbacks read the reason as it stands"
        );
        assert!(
            landed(
                &mut world,
                crate::ui_quest::test_template(373, "A Threat Within", 0)
            )
            .is_empty(),
            "the callbacks ran once"
        );
        failed(&mut world, 900, 0);
        assert!(landed(&mut world, crate::ui_quest::test_template(900, "", 0)).is_empty());
        assert!(world.resource::<QuestLog>().giver_failures.is_empty());
    }

    // ── The share's BUSY refusal ─────────────────────────────────────────────────

    fn detail(npc: u64, quest_id: u32) -> QuestDetails {
        QuestDetails {
            npc,
            quest_id,
            title: "A Threat Within".into(),
            details: String::new(),
            objectives: String::new(),
            auto_finish: 0,
            choices: Vec::new(),
            rewards: Vec::new(),
            money: 0,
            reward_spell: 0,
        }
    }

    #[test]
    fn a_share_on_top_of_an_open_window_is_refused_as_busy() {
        const SHARER: u64 = 0x0000_0000_0000_002A;
        const NPC: u64 = 0xF130_0000_0000_0007;
        let (mut world, rx) = quest_world();

        quest_detail(&mut world, detail(NPC, 100));
        assert_eq!(world.resource::<QuestGiver>().npc, Some(NPC));
        assert!(rx.try_iter().next().is_none(), "opening sends nothing");

        quest_detail(&mut world, detail(SHARER, 200));
        assert_eq!(
            world.resource::<QuestGiver>().npc,
            Some(NPC),
            "the open window survives the refusal"
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::QuestPushResult {
                    sharer: SHARER,
                    msg: QuestShareMsg::BUSY,
                }]
            ),
            "the sharer is told we are busy: {sent:?}"
        );
    }

    #[test]
    fn a_share_with_no_window_open_just_opens() {
        const SHARER: u64 = 0x0000_0000_0000_002A;
        let (mut world, rx) = quest_world();

        quest_detail(&mut world, detail(SHARER, 200));
        assert_eq!(world.resource::<QuestGiver>().npc, Some(SHARER));
        assert!(rx.try_iter().next().is_none(), "no verdict, no refusal");
    }

    #[test]
    fn the_latch_is_written_by_every_panel_packet_and_cleared_on_world_enter() {
        const NPC: u64 = 0xF130_0000_0000_0007;
        let (mut world, _rx) = quest_world();
        let latch = |world: &World| world.resource::<QuestGiver>().close_on_cancel;

        let mut d = detail(NPC, 100);
        d.auto_finish = 3;
        quest_detail(&mut world, d);
        assert_eq!(latch(&world), 3, "DETAILS writes it");

        quest_progress(&mut world, progress(NPC, 100, 1));
        assert_eq!(latch(&world), 1, "REQUEST_ITEMS' closeOnCancel writes it");
        quest_progress(&mut world, progress(NPC, 100, 0));
        assert_eq!(latch(&world), 0, "…including a zero");

        quest_offer(&mut world, offer(NPC, 100, 1));
        assert_eq!(latch(&world), 1, "OFFER_REWARD writes it");

        world_enter_reset(&mut world.resource_mut::<QuestGiver>());
        assert_eq!(latch(&world), 0, "the world-enter reset zeroes it");
    }

    /// Every panel publish clears the already-acted latch (`0x500bd0`, `0x500d91`).
    #[test]
    fn a_panel_packet_clears_the_acted_latch() {
        let (mut world, _rx) = quest_world();
        world.resource_mut::<QuestGiver>().acted = true;
        quest_detail(&mut world, detail(0xF130_0000_0000_0007, 100));
        assert!(!world.resource::<QuestGiver>().acted);
    }

    fn progress(npc: u64, quest_id: u32, close_on_cancel: u32) -> QuestRequestItems {
        QuestRequestItems {
            npc,
            quest_id,
            title: "A Threat Within".into(),
            request_text: String::new(),
            emote: 0,
            close_on_cancel,
            required_money: 0,
            required_items: Vec::new(),
            is_complete: false,
        }
    }

    fn offer(npc: u64, quest_id: u32, auto_finish: u32) -> QuestOfferReward {
        QuestOfferReward {
            npc,
            quest_id,
            title: "A Threat Within".into(),
            offer_text: String::new(),
            auto_finish,
            choices: Vec::new(),
            rewards: Vec::new(),
            money: 0,
            quest_flags: 0,
            reward_spell: 0,
        }
    }
}
