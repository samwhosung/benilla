//! The questgiver panels' packet handlers (in the net handler table since 2320,
//! moved out of the drain's quests arm file) — each fills the [`QuestGiver`] the quest feed
//! ([`super`]) reads; each panel packet replaces the open view, and the greeting/gossip quest-row
//! clicks and the panel buttons flow back out through the quest/gossip drains. The quest log's
//! template answer is [`crate::ui_quest_log`]'s and the party share's pair is
//! [`crate::ui_quest_share`]'s.

use benilla_protocol::messages::{
    QuestComplete, QuestDetails, QuestGiverList, QuestOfferReward, QuestRequestItems, QuestShareMsg,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::QuestGiver;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_action::UiError;
use crate::ui_quest_log::QuestLog;

/// Register the questgiver handlers — called from [`super::UiQuestPlugin`]. One per kind, plus
/// the session-end listener.
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
        .net_handler(K::Disconnected, on_session_end)
        .net_handler(K::Connected, on_login)
        .net_handler(K::Worldport, on_worldport);
}

/// The world-enter reset `0x500af0` (sole caller `0x49099c`, inside the enter-world cascade
/// `0x4908c0` that runs on a fresh login and on every worldport) zeroes the `0xbe0824` latch —
/// its only clear (`0x500af0`). A second listener on each kind, after the bridge's.
fn on_login(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::Connected { .. } = ev {
        world_enter_reset(&mut quest);
    }
}

/// [`on_login`]'s twin for the other entry into the cascade, the worldport.
fn on_worldport(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::Worldport { .. } = ev {
        world_enter_reset(&mut quest);
    }
}

fn world_enter_reset(quest: &mut QuestGiver) {
    quest.close_on_cancel = 0;
}

fn on_giver_status(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestGiverStatus { npc, status } = ev {
        quest_giver_status(npc, status, &mut quest);
    }
}

fn on_greeting(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestGreeting(list) = ev {
        quest_greeting(list, &mut quest);
    }
}

fn on_detail(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>, commands: Res<NetCommands>) {
    if let SessionEvent::QuestDetail(d) = ev {
        quest_detail(d, &mut quest, &commands);
    }
}

fn on_progress(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestProgress(p) = ev {
        quest_progress(p, &mut quest);
    }
}

fn on_offer(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestOffer(o) = ev {
        quest_offer(o, &mut quest);
    }
}

fn on_complete(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestComplete(c) = ev {
        quest_complete(c, &mut quest);
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
    mut quest_log: ResMut<QuestLog>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::QuestFailed { quest_id, timed } = ev {
        quest_failed(quest_id, timed, &mut quest_log, &commands, &mut quest);
    }
}

fn on_log_full(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestLogFull = ev {
        quest_log_full(&mut quest);
    }
}

fn on_giver_invalid(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestGiverInvalid { reason } = ev {
        quest_giver_invalid(reason, &mut quest);
    }
}

fn on_giver_failed(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    mut quest_log: ResMut<QuestLog>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::QuestGiverFailed { quest_id, reason } = ev {
        quest_giver_failed(quest_id, reason, &mut quest, &mut quest_log, &commands);
    }
}

/// An open questgiver panel dies with the socket. A listener on the session end
/// (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(In(_): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    quest.clear_session();
}

/// A questgiver dialog status for one NPC (`SMSG_QUESTGIVER_STATUS`) — the `!`/`?` marker's
/// [`crate::messages::dialog_status`] value, stored per guid for the marker layer and the minimap
/// quest dot.
///
/// **A GameObject's answer is DROPPED, and that is the reference's own behaviour** (decision
/// 1872): the handler `0x5dc9f0` resolves the packet's GUID with `0x5dca22 mov ecx,8` — typemask
/// `TYPEMASK_UNIT` — and `0x468460` is a bitmask AND against `OBJECT_FIELD_TYPE`, so a
/// GameObject's `0x21` misses bit 3, returns NULL, and the handler exits at `0x5dca2f` without
/// ever reaching the `UNIT_NPC_FLAGS` test, let alone the marker.
///
/// This is not a hypothetical branch here: benilla now *sends* the query for quest-flagged
/// GameObjects, exactly as the reference does ([`crate::quest_markers`]), and **vmangos answers
/// it** — `GetObjectByTypeMask(guid, TYPEMASK_CREATURE_OR_GAMEOBJECT)`, `QuestHandler.cpp:41` —
/// where the real 1.12 service never did (zero GameObject GUIDs across 1292 in the sniff corpus).
/// So the drop is what keeps a vmangos-only answer from putting a `!` over a wanted poster that
/// the reference client leaves bare.
///
/// The test is the GUID's own shape rather than a live type lookup: it gives the same partition
/// as typemask 8 for anything a server can send us. It was chosen when this was an arm of the
/// drain's match, where a status arriving in the same drain as its object's create block would
/// have read a not-yet-seeded store (descriptors were flushed at the end of the drain) and
/// dropped a *unit's* answer that would then never be re-asked for. As a packet handler it runs
/// after the create has landed, so that reason is gone; the shape test stays
/// because it is the same partition. The reference's second conjunct — `UNIT_NPC_FLAGS & 0x2` on
/// the resolved unit — was left unmodelled for the same ordering reason and is buildable now;
/// until it is, [`crate::quest_markers::query`]'s teardown leg covers the flag-clearing case from
/// the other end.
fn quest_giver_status(npc: u64, status: u32, quest: &mut QuestGiver) {
    use benilla_protocol::guid;
    if !(guid::is_player(npc) || guid::is_creature_or_pet(npc)) {
        debug!("net: dropping a non-unit questgiver status ({npc:#x} → {status}) — typemask 8");
        quest.refuse_status(npc, status);
        return;
    }
    quest.set_status(npc, status);
}

/// The greeting panel: an NPC's offered/active quest rows (`SMSG_QUESTGIVER_QUEST_LIST`).
fn quest_greeting(list: QuestGiverList, quest: &mut QuestGiver) {
    debug!(
        "net: quest greeting on {:#x} — {} quests",
        list.npc,
        list.quests.len()
    );
    quest.open(list.npc, crate::ui_quest::QuestView::Greeting(list));
}

/// The accept panel: full quest text + rewards on offer (`SMSG_QUESTGIVER_QUEST_DETAILS`).
fn quest_detail(d: QuestDetails, quest: &mut QuestGiver, commands: &NetCommands) {
    debug!("net: quest detail — quest {} on {:#x}", d.quest_id, d.npc);
    // **A share that arrives on top of an open window is refused, by the client** (`0x5dbf85`,
    // decision 1738): `MSG_QUEST_PUSH_RESULT{sharer, BUSY}` and the panel we are reading stays.
    // `BUSY` is the second and last verdict the client originates. The server has a busy test of
    // its own (`Player::GetQuestShareInfo`), but it only knows about shares — it cannot see that we
    // are mid-turn-in at an NPC, which is exactly the case this covers.
    if benilla_protocol::guid::is_player(d.npc) && quest.is_open() {
        debug!(
            "net: busy — refusing {:#x}'s shared quest {}",
            d.npc, d.quest_id
        );
        let _ = commands.0.send(ClientCommand::QuestPushResult {
            sharer: d.npc,
            msg: QuestShareMsg::BUSY,
        });
        return;
    }
    // The trailing flag is a latch, not a field of the view (see `QuestGiver::close_on_cancel`).
    quest.close_on_cancel = d.auto_finish;
    quest.open(d.npc, crate::ui_quest::QuestView::Detail(d));
}

/// The progress panel: "bring me these" text + required items/money + completability
/// (`SMSG_QUESTGIVER_REQUEST_ITEMS`).
fn quest_progress(p: QuestRequestItems, quest: &mut QuestGiver) {
    debug!(
        "net: quest progress — quest {} on {:#x} (complete: {})",
        p.quest_id, p.npc, p.is_complete
    );
    // The progress publisher `0x501070` writes `closeOnCancel` into the same latch (`0x50111a`).
    quest.close_on_cancel = p.close_on_cancel;
    quest.open(p.npc, crate::ui_quest::QuestView::Progress(p));
}

/// The reward panel: turn-in text + rewards to grant (`SMSG_QUESTGIVER_OFFER_REWARD`).
fn quest_offer(o: QuestOfferReward, quest: &mut QuestGiver) {
    debug!(
        "net: quest reward offer — quest {} on {:#x}",
        o.quest_id, o.npc
    );
    // The `u32` after the reward text lands in the same latch as DETAILS' trailing one — one
    // publisher, `0x500ef0` (`0x50101a`).
    quest.close_on_cancel = o.auto_finish;
    quest.open(o.npc, crate::ui_quest::QuestView::Reward(o));
}

/// The turn-in result: XP/money granted + fixed items (`SMSG_QUESTGIVER_QUEST_COMPLETE`).
fn quest_complete(c: QuestComplete, quest: &mut QuestGiver) {
    // The completion fanfare (QUESTCOMPLETED kit → iQuestComplete.wav) — the client's C++ plays
    // it on exactly this packet; the giver feed drains the flag into the UI sound path.
    quest.completed_fanfare = true;
    // The turn-in result: log the reward summary and close the window (the XP/money/item
    // grants arrive separately via UPDATE_OBJECT + ITEM_PUSH_RESULT).
    debug!(
        "net: quest {} complete — +{} XP, +{} copper, {} item(s)",
        c.quest_id,
        c.xp,
        c.money,
        c.items.len()
    );
    quest.clear();
    // The turn-in result is one of the `SMSG_QUESTGIVER_*` the reference sweeps from: every
    // other giver's `!`/`?` can move the moment a quest is handed in.
    quest.bump_reask();
}

/// A kill/use objective ticked (`SMSG_QUESTUPDATE_ADD_KILL`) / an item-collection tick
/// (`SMSG_QUESTUPDATE_ADD_ITEM`). The visible surface — the yellow `UI_INFO_MESSAGE` toast, the
/// ref's `ERR_QUEST_ADD_*_SII` popups — no longer fires from here: it rides the quest-log
/// objective diff (`crate::ui_quest_log::feed_quest_log`), which composes the same line from the
/// same descriptor/template/bag state the SMSG announces (the wire's item tick carries only the
/// ADDED count — the "cur/req" is client-computed either way, the wire pin's finding). The old
/// chat-line stopgap here is retired: the reference shows no chat echo for objective progress
/// (inferred from reference screenshots, unconfirmed in the binary; if the real handler does
/// more, a sound or a distinct format, it goes here).
fn quest_objective_kill(entry: u32, count: u32, required: u32, quest: &mut QuestGiver) {
    debug!("net: quest kill/use objective {entry:#x} at {count}/{required}");
    quest.bump_reask();
}

/// See [`quest_objective_kill`] — same surface, item flavor.
fn quest_objective_item(item_id: u32, count: u32, quest: &mut QuestGiver) {
    debug!("net: quest item objective {item_id} +{count}");
    quest.bump_reask();
}

/// Every objective on the quest is complete (`SMSG_QUESTUPDATE_COMPLETE`, 0x198). The visible
/// surface — the yellow `"%s (Complete)"` toast (`ERR_QUEST_OBJECTIVE_COMPLETE_S`, verified
/// kind-1 → UI_INFO_MESSAGE, never a chat line) — rides the quest-log diff's COMPLETE-flip
/// detection (`crate::ui_quest_log::feed_quest_log`), same as the progress toasts; the slot's
/// state byte carries the durable fact.
fn quest_objectives_complete(quest_id: u32, quest: &mut QuestGiver) {
    debug!("net: quest {quest_id} objectives complete");
    // The turn-in `?` can go gold with no quest-log field change of its own, so the reference
    // sweeps from these `SMSG_QUESTUPDATE_*` handlers.
    quest.bump_reask();
}

/// The quest failed (`SMSG_QUESTUPDATE_FAILED` `0x196` / `_FAILEDTIMER` `0x197` — `timed` picks
/// which): the one quest-update with a CHAT surface — msgId `0x8b` = `ERR_QUEST_FAILED_S`, catalog
/// row 139, `kind 0`, named with the quest's title (the handler pushes `template+0x9c`).
///
/// **An uncached template is SILENT, and that is the handler's own gate rather than a fallback.**
/// `0x5e5ad0`'s FAILED arm reads the questId, then *bails* if the template is not in the cache (or
/// if the per-slot flag `byte[slot+7]&2` is set) — it never reaches `0x496720`. This used to
/// compose `"Quest failed."` there, a sentence 1.12 has no string for and never says (decision
/// 2045 named it as one of two remaining inventions).
///
/// The line rides `QuestGiver`'s by-key queue — the route `quest_log_full` and
/// `quest_giver_invalid` already take — so `ui_quest::feed_quest` resolves it against the player's
/// own `GlobalStrings.lua` and `show_messages` reads the row. **That also lands the sound the old
/// path was dropping**: row 139 names the cue `igQuestFailed`, which the reference plays here and
/// benilla had noted as an unbuilt follow-up.
fn quest_failed(
    quest_id: u32,
    timed: bool,
    quest_log: &mut QuestLog,
    net_commands: &NetCommands,
    quest: &mut QuestGiver,
) {
    debug!("net: quest {quest_id} failed (timed: {timed})");
    if let Some(t) = quest_log.template(quest_id, net_commands) {
        quest.push_message(UiError::s("ERR_QUEST_FAILED_S", t.title.clone()));
    } else {
        debug!("net: quest {quest_id} has no cached template — the reference shows nothing");
    }
    // A failure moves what the givers offer — the reference sweeps from these `SMSG_QUESTUPDATE_*`
    // handlers too.
    quest.bump_reask();
}

/// The log refused a new quest — no free slot (`SMSG_QUESTLOG_FULL`). The ref's `0x195` arm is a
/// bare `DisplayError(153)` and nothing else (no panel close): `ERR_QUEST_LOG_FULL` is a kind-2
/// record, so it is the RED line, not a chat line (it used to be a hardcoded
/// English chat push here).
fn quest_log_full(quest: &mut QuestGiver) {
    debug!("net: quest log full");
    quest.push_message(UiError::key("ERR_QUEST_LOG_FULL"));
}

/// The giver won't offer the quest (`SMSG_QUESTGIVER_QUEST_INVALID`, ref handler `0x5dbca0`): one
/// `QuestFailedReason` code, no quest id. Its chat line comes from [`questgiver_invalid_key`]'s
/// byte-verified table — reason 13 is "You are already on that quest", the line the director's
/// bare `Quest failed (0x0d).` used to be.
///
/// Then the ref **closes the window**: both refusal handlers end in `0x501130(0,0)`, which zeroes
/// the current questgiver guid (`0xbe0810`) and signals Lua event `0x130` = `QUEST_FINISHED` —
/// our [`QuestGiver::clear`] plus the feed's own `QUEST_FINISHED` on the cleared view. Without
/// this the panel sat open on a refused accept.
fn quest_giver_invalid(reason: u32, quest: &mut QuestGiver) {
    debug!("net: questgiver refused to offer the quest (reason {reason})");
    quest.push_message(UiError::key(crate::ui_quest::questgiver_invalid_key(
        reason,
    )));
    quest.clear();
}

/// The accept failed on a quest the giver DID offer (`SMSG_QUESTGIVER_QUEST_FAILED`, ref handler
/// `0x5dc840`): `{questId, reason}`, and the line names the quest — the ref pushes the quest
/// record's title (`+0x9c`) as the format's `%s`, so we fill it from the open view (the panel that
/// is refusing IS this quest) and fall back to the template cache. A full-bag refusal shows a
/// SECOND line, the ref's bare `DisplayError(0)` = `ERR_INV_FULL` on the red surface. Closes the
/// window like [`quest_giver_invalid`].
fn quest_giver_failed(
    quest_id: u32,
    reason: u32,
    quest: &mut QuestGiver,
    quest_log: &mut QuestLog,
    net_commands: &NetCommands,
) {
    debug!("net: quest {quest_id} accept failed (reason {reason})");
    let title = quest
        .view_title(quest_id)
        .or_else(|| {
            quest_log
                .template(quest_id, net_commands)
                .map(|t| t.title.clone())
        })
        .unwrap_or_default();
    quest.push_message(UiError::s(
        crate::ui_quest::questgiver_failed_key(reason),
        title,
    ));
    if matches!(reason, 4 | 50) {
        quest.push_message(UiError::key("ERR_INV_FULL"));
    }
    quest.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The typemask-8 refusal**. benilla now sends
    /// `CMSG_QUESTGIVER_STATUS_QUERY` for quest-flagged GameObjects because the reference
    /// does — and vmangos, unlike the real 1.12 service, *answers*. The reference's handler drops
    /// that answer at the lookup (`0x5dca22 mov ecx,8`), so we must too: otherwise a wanted poster
    /// would wear a gold `!` the reference client never puts there.
    ///
    /// The control is the half that must not change: a creature's answer, and a player's, still
    /// land — typemask 8 is `TYPEMASK_UNIT`, which a creature (`0x9`) and a player (`0x19`) both
    /// carry.
    #[test]
    fn a_gameobjects_dialog_status_is_dropped_and_a_creatures_is_not() {
        use benilla_protocol::messages::dialog_status;

        // The Goldshire `Wanted Poster` (HIGHGUID_GAMEOBJECT), an elevator (HIGHGUID_TRANSPORT —
        // a GameObject in every respect but its high word), a creature, and a player.
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

    /// The director's repro: the quest is already in the log, so the accept comes back
    /// `QUEST_INVALID` reason 13. The line must be the ref's string key — never the bare
    /// `Quest failed (0x0d).` — and the panel must CLOSE (`0x501130(0,0)` → `QUEST_FINISHED`),
    /// which is what left it sitting open before.
    #[test]
    fn an_already_on_refusal_speaks_and_closes_the_panel() {
        let mut giver = open_detail(373);
        quest_giver_invalid(13, &mut giver);
        let msgs = giver.take_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[0].key),
            benilla_ui::messages::MsgKind::Chat
        );
        assert_eq!(msgs[0], UiError::key("ERR_QUEST_ALREADY_ON"));
        assert!(!giver.is_open(), "the ref closes the window on a refusal");
    }

    /// The named half: the `%s` comes off the open panel, and a full bag adds the ref's second
    /// line (`DisplayError(0)` = `ERR_INV_FULL`) on the RED surface.
    #[test]
    fn a_full_bag_refusal_names_the_quest_and_adds_the_inventory_line() {
        let mut giver = open_detail(373);
        let mut log = QuestLog::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        quest_giver_failed(373, 4, &mut giver, &mut log, &commands);
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
        assert!(!giver.is_open());
    }

    /// `SMSG_QUESTLOG_FULL` is the odd one out: the ref's arm is a bare `DisplayError(153)` — the
    /// RED line, and NO close.
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

    /// A refusal for a quest the open panel is NOT showing leaves the `%s` empty rather than
    /// naming the wrong quest.
    #[test]
    fn a_refusal_for_another_quest_does_not_borrow_the_open_title() {
        let mut giver = open_detail(373);
        let mut log = QuestLog::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        quest_giver_failed(999, 17, &mut giver, &mut log, &commands);
        let msgs = giver.take_messages();
        assert_eq!(msgs[0].key, "ERR_QUEST_FAILED_MAX_COUNT_S");
        assert_eq!(msgs[0].arg_s(), Some(""));
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

    /// **A share arriving on top of an open window is refused by the CLIENT**, with `BUSY` — the
    /// second and last verdict the client originates (`0x5dbf85`). The window we are already
    /// reading is not replaced, which is the point: the server's own busy test only knows about
    /// other shares, so a player mid-turn-in at an NPC is invisible to it.
    #[test]
    fn a_share_on_top_of_an_open_window_is_refused_as_busy() {
        const SHARER: u64 = 0x0000_0000_0000_002A;
        const NPC: u64 = 0xF130_0000_0000_0007;
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut quest = QuestGiver::default();

        // An NPC's panel is up…
        quest_detail(detail(NPC, 100), &mut quest, &commands);
        assert_eq!(quest.npc, Some(NPC));
        assert!(rx.try_iter().next().is_none(), "opening sends nothing");

        // …and a party member's share lands on top of it.
        quest_detail(detail(SHARER, 200), &mut quest, &commands);
        assert_eq!(quest.npc, Some(NPC), "the open window survives the refusal");
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

    /// With nothing open, the same share opens the panel normally and answers nothing — the
    /// control the refusal above would pass without.
    #[test]
    fn a_share_with_no_window_open_just_opens() {
        const SHARER: u64 = 0x0000_0000_0000_002A;
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut quest = QuestGiver::default();

        quest_detail(detail(SHARER, 200), &mut quest, &commands);
        assert_eq!(quest.npc, Some(SHARER));
        assert!(rx.try_iter().next().is_none(), "no verdict, no refusal");
    }

    /// The `0xbe0824` latch is written by all THREE panel packets — DETAILS' trailing `u32`,
    /// OFFER_REWARD's `u32` after the reward text (both through `0x500ef0`, `0x50101a`) and
    /// REQUEST_ITEMS' `closeOnCancel` (`0x501070`, `0x50111a`) — and zeroed by the world-enter
    /// reset `0x500af0`. It is a latch, not a field of the open view: its one reader runs
    /// whichever panel is up. benilla used to write it from DETAILS alone and never clear it, so
    /// a single-quest auto-open's progress panel (`closeOnCancel` = 1) re-opened the NPC on
    /// Cancel, which answers with the same panel again.
    #[test]
    fn the_latch_is_written_by_every_panel_packet_and_cleared_on_world_enter() {
        const NPC: u64 = 0xF130_0000_0000_0007;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut quest = QuestGiver::default();

        let mut d = detail(NPC, 100);
        d.auto_finish = 3;
        quest_detail(d, &mut quest, &commands);
        assert_eq!(quest.close_on_cancel, 3, "DETAILS writes it");

        quest_progress(progress(NPC, 100, 1), &mut quest);
        assert_eq!(
            quest.close_on_cancel, 1,
            "REQUEST_ITEMS' closeOnCancel writes it"
        );
        quest_progress(progress(NPC, 100, 0), &mut quest);
        assert_eq!(quest.close_on_cancel, 0, "…including a zero");

        quest_offer(offer(NPC, 100, 1), &mut quest);
        assert_eq!(quest.close_on_cancel, 1, "OFFER_REWARD writes it");

        world_enter_reset(&mut quest);
        assert_eq!(quest.close_on_cancel, 0, "the world-enter reset zeroes it");
    }

    /// Every panel publish clears the already-acted latch (`0x500bd0`, `0x500d91`).
    #[test]
    fn a_panel_packet_clears_the_acted_latch() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut quest = QuestGiver {
            acted: true,
            ..Default::default()
        };
        quest_detail(detail(0xF130_0000_0000_0007, 100), &mut quest, &commands);
        assert!(!quest.acted);
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
