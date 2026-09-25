//! Party quest sharing: each member's verdict on a quest we shared, and the escort confirm a
//! member gets when someone starts a party-accept quest, with one server latch
//! (`Player::SetQuestShareInfo`) behind both. The receiving half needs nothing here: the receiver
//! gets a plain `SMSG_QUESTGIVER_QUEST_DETAILS` whose giver is the sharer (vmangos
//! `QuestHandler.cpp:454`), which [`crate::ui_quest`] tells from an NPC offer by the guid's type.
//!
//! `QUEST_ACCEPT_CONFIRM(member, title)` raises the stock popup (`UIParent.lua:353`); Yes calls
//! `ConfirmAcceptQuest()` and No sends nothing.
//!
//! Deviation: a member's name not yet cached holds its line for a bounded retry, where the
//! reference reads its name cache (`0xc0e228` via `0x55f080`) with a null callback and drops the
//! message on a miss, because benilla's cache may not hold a party member yet.

use benilla_protocol::messages::{QuestConfirmAccept, QuestShareMsg};
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, UiError};
use crate::ui_script::{UiFeed, UiInput};

/// Frames a queued line waits for its name: a name query is one round trip, and a guid the server
/// will not name would otherwise hold its line all session.
const NAME_MAX_TRIES: u16 = 120;

/// One `MSG_QUEST_PUSH_RESULT` verdict waiting for the member's name.
struct PendingVerdict {
    member: u64,
    msg: QuestShareMsg,
    tries: u16,
}

/// The share state: the verdicts on quests we pushed, and the escort confirm we owe an answer.
#[derive(Resource, Default)]
pub(crate) struct QuestShare {
    verdicts: Vec<PendingVerdict>,
    /// The confirm the server sent, held until its sender's name resolves and the event fires.
    pending_confirm: Option<QuestConfirmAccept>,
    /// The quest `ConfirmAcceptQuest()` answers, latched when the event fires: the verb takes no
    /// argument (`StaticPopup.lua:731-733`), so it answers the last confirm asked.
    confirm_quest: Option<u32>,
}

impl QuestShare {
    /// Queues a `MSG_QUEST_PUSH_RESULT` verdict, one or two per member (`SHARING_QUEST`, then the
    /// outcome); the guid is the member it is about, never the sharer.
    pub(crate) fn push_verdict(&mut self, member: u64, msg: QuestShareMsg) {
        self.verdicts.push(PendingVerdict {
            member,
            msg,
            tries: 0,
        });
    }

    /// Holds an escort confirm for the feed; a second replaces an unfired first, as the server
    /// keeps one latch.
    pub(crate) fn set_confirm(&mut self, c: QuestConfirmAccept) {
        self.pending_confirm = Some(c);
    }

    /// On disconnect: the server's latch went with the session.
    pub(crate) fn clear_session(&mut self) {
        self.verdicts.clear();
        self.pending_confirm = None;
        self.confirm_quest = None;
    }
}

/// The `ERR_QUEST_PUSH_*` key a verdict shows, named as vmangos `QuestDef.h:62-70` names them. The
/// reference's `0x276` arm (`0x5e4781`) maps 0..8 onto messages `0x181`-`0x189` in this order, all
/// `kind 0` (`0x487dc9`-`0x487e69`): `CHAT_MSG_SYSTEM` lines, never `UIErrorsFrame`. An unmapped
/// byte shows nothing.
fn verdict_message(msg: QuestShareMsg) -> Option<&'static str> {
    let key = match msg {
        QuestShareMsg::SHARING_QUEST => "ERR_QUEST_PUSH_SUCCESS_S",
        QuestShareMsg::CANT_TAKE_QUEST => "ERR_QUEST_PUSH_INVALID_S",
        QuestShareMsg::ACCEPT_QUEST => "ERR_QUEST_PUSH_ACCEPTED_S",
        QuestShareMsg::DECLINE_QUEST => "ERR_QUEST_PUSH_DECLINED_S",
        QuestShareMsg::TOO_FAR => "ERR_QUEST_PUSH_TOO_FAR_S",
        QuestShareMsg::BUSY => "ERR_QUEST_PUSH_BUSY_S",
        QuestShareMsg::LOG_FULL => "ERR_QUEST_PUSH_LOG_FULL_S",
        QuestShareMsg::HAVE_QUEST => "ERR_QUEST_PUSH_ONQUEST_S",
        QuestShareMsg::FINISH_QUEST => "ERR_QUEST_PUSH_ALREADY_DONE_S",
        _ => return None,
    };
    Some(key)
}

/// The quest-share packet handlers. Both park in [`QuestShare`]: the guid may need a name query a
/// handler cannot await.
mod net {
    use benilla_protocol::messages::{QuestConfirmAccept, QuestShareMsg};
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::QuestShare;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::QuestPushResult, on_push_result)
            .net_handler(K::QuestConfirmAccept, on_confirm_accept)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_push_result(In(ev): In<SessionEvent>, mut share: ResMut<QuestShare>) {
        if let SessionEvent::QuestPushResult { member, msg } = ev {
            quest_push_result(member, msg, &mut share);
        }
    }

    fn on_confirm_accept(In(ev): In<SessionEvent>, mut share: ResMut<QuestShare>) {
        if let SessionEvent::QuestConfirmAccept(c) = ev {
            quest_confirm_accept(c, &mut share);
        }
    }

    fn on_session_end(In(_): In<SessionEvent>, mut share: ResMut<QuestShare>) {
        share.clear_session();
    }

    /// Parked, not shown: the member's name may need a `CMSG_NAME_QUERY` round trip, and this pass
    /// has no VM for the GlobalStrings.
    fn quest_push_result(member: u64, msg: QuestShareMsg, share: &mut QuestShare) {
        debug!(
            "net: quest push result — member {member:#x} verdict {}",
            msg.0
        );
        share.push_verdict(member, msg);
    }

    /// A member started a `QUEST_FLAGS_PARTY_ACCEPT` quest and the server asks whether we join;
    /// parked, since the popup names the member.
    fn quest_confirm_accept(c: QuestConfirmAccept, share: &mut QuestShare) {
        debug!(
            "net: quest confirm accept — quest {} ({:?}) from {:#x}",
            c.quest_id, c.title, c.sender
        );
        share.set_confirm(c);
    }
}

/// Owns [`QuestShare`] and its two systems; nothing here is bound to the questgiver window.
pub(crate) struct QuestSharePlugin;

impl Plugin for QuestSharePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // Feed before the input pass, so a line shows the frame its packet lands; drain after
        // it, so Yes goes out the frame it is clicked.
        app.init_resource::<QuestShare>().add_systems(
            Update,
            (
                feed_quest_share.in_set(UiFeed),
                drain_quest_share.after(UiInput),
            ),
        );
    }
}

/// Resolves the queued names and shows what they unlock: verdict lines and `QUEST_ACCEPT_CONFIRM`.
fn feed_quest_share(
    script: Option<NonSendMut<UiScript>>,
    mut share: ResMut<QuestShare>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut sink: MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };

    // The verdicts: one line per resolved name, the rest kept for the next frame.
    let mut lines: Vec<Shown> = Vec::new();
    let mut waiting = Vec::new();
    for mut v in std::mem::take(&mut share.verdicts) {
        let Some(key) = verdict_message(v.msg) else {
            debug!(
                "ui_quest_share: unmapped push verdict {} — no line",
                v.msg.0
            );
            continue;
        };
        match names.resolve(v.member, &commands).map(str::to_string) {
            Some(name) => {
                let err = UiError::s(key, name);
                let get = |k: &str| script.lua().globals().get::<String>(k).ok();
                if let Some(text) = ui_error_text(&err, &get) {
                    lines.push(Shown::keyed(err.key, text));
                }
            }
            None => {
                v.tries += 1;
                if v.tries < NAME_MAX_TRIES {
                    waiting.push(v);
                } else {
                    debug!(
                        "ui_quest_share: gave up naming {:#x} for verdict {}",
                        v.member, v.msg.0
                    );
                }
            }
        }
    }
    share.verdicts = waiting;
    show_messages(&mut script, &mut sink, "ui_quest_share", lines);

    // The escort confirm, once the member's name lands: player first, then title, as
    // `QUEST_ACCEPT` fills them.
    if let Some(c) = share.pending_confirm.as_ref() {
        if let Some(name) = names.resolve(c.sender, &commands).map(str::to_string) {
            let title = c.title.clone();
            share.confirm_quest = Some(c.quest_id);
            share.pending_confirm = None;
            script.fire_event(
                "QUEST_ACCEPT_CONFIRM",
                vec![ScriptValue::Str(name), ScriptValue::Str(title)],
            );
        }
    }
}

/// `ConfirmAcceptQuest()`, the popup's Yes, answered with the latched quest id.
fn drain_quest_share(
    script: Option<NonSendMut<UiScript>>,
    share: Res<QuestShare>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_quest_confirms() {
        let Some(quest) = share.confirm_quest else {
            debug!("ui_quest_share: ConfirmAcceptQuest with no confirm held — ignored");
            continue;
        };
        let _ = commands.0.send(ClientCommand::QuestConfirmAccept { quest });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wire_verdict_has_a_key_and_an_unknown_has_none() {
        for v in 0u8..=8 {
            assert!(
                verdict_message(QuestShareMsg(v)).is_some(),
                "verdict {v} unmapped"
            );
        }
        assert!(verdict_message(QuestShareMsg(9)).is_none());
        assert!(verdict_message(QuestShareMsg(0xFF)).is_none());
    }

    #[test]
    fn verdict_keys_are_distinct() {
        let mut keys: Vec<&str> = (0u8..=8)
            .filter_map(|v| verdict_message(QuestShareMsg(v)))
            .collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate ERR_QUEST_PUSH_* key");
    }

    #[test]
    fn every_verdict_key_resolves_and_names_a_member() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        for raw in 0u8..=8 {
            let key = verdict_message(QuestShareMsg(raw)).expect("mapped");
            let text = g(key).unwrap_or_default();
            assert!(!text.is_empty(), "{key} (verdict {raw}) missing");
            assert!(text.contains("%s"), "{key} names no member: {text:?}");
        }

        // The opener the sharer sees on the click, and a decline's answer.
        let line = |raw: u8| {
            let key = verdict_message(QuestShareMsg(raw)).unwrap();
            ui_error_text(&UiError::s(key, "Mate"), &g)
        };
        assert_eq!(
            line(QuestShareMsg::SHARING_QUEST.0).as_deref(),
            Some("Sharing quest with Mate...")
        );
        assert_eq!(
            line(QuestShareMsg::DECLINE_QUEST.0).as_deref(),
            Some("Mate has declined your quest")
        );
    }
}
