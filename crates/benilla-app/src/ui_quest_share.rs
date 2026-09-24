//! **Party quest sharing** — the *Share Quest* button's two answer paths (decision 1733).
//!
//! Everything about a share that is not already the questgiver panel's job lives here, and that
//! turns out to be exactly the two things the sharer's own client cannot see: what each party
//! member *did* with the quest, and — for an escort quest — the confirm box a member gets when
//! somebody else starts it.
//!
//! **What is NOT here, deliberately.** The receiving half of an ordinary share needs no code of its
//! own: the server sends the receiver a plain `SMSG_QUESTGIVER_QUEST_DETAILS` whose giver guid is
//! the *sharer's player guid* (vmangos `QuestHandler.cpp:450`), so the shared quest arrives on the
//! questgiver panel already built, and Accept answers it with the ordinary
//! `CMSG_QUESTGIVER_ACCEPT_QUEST` addressed to that player. [`crate::ui_quest`] needs to know a
//! share from an NPC offer for exactly two things — the decline packet and the range guard — and it
//! reads both off the giver guid's own type bits ([`benilla_protocol::guid::is_player`]).
//!
//! **The two flows, and why one resource holds both.** They are one server-side latch
//! (`Player::SetQuestShareInfo` — one `{sharer, quest}` pair per player, set by the push and by an
//! escort accept alike), so a client that modelled them separately would be modelling one thing
//! twice.
//!
//! 1. **The verdicts.** `MSG_QUEST_PUSH_RESULT` comes back to the SHARER, once per member and
//!    usually twice (`SHARING_QUEST` when the push goes out, then the outcome). Each is an
//!    `ERR_QUEST_PUSH_*_S` line naming the member — `%s`-filled from the guid in the packet, which
//!    is the member the verdict is *about*, never the sharer (the direction trap on
//!    [`benilla_protocol::messages::QuestPushResult`]).
//! 2. **The escort confirm.** `SMSG_QUEST_CONFIRM_ACCEPT` arrives when a party member starts a
//!    `QUEST_FLAGS_PARTY_ACCEPT` quest. It raises `QUEST_ACCEPT_CONFIRM(memberName, questTitle)`,
//!    the reference's own event (`UIParent.lua:353` → `StaticPopup_Show("QUEST_ACCEPT", arg1,
//!    arg2)`); Yes calls `ConfirmAcceptQuest()`, and **No sends nothing at all** — which is why
//!    there is no decline command here.
//!
//! **A name we do not have yet holds its line — a NAMED divergence, not fidelity.** Both paths
//! address a party member by guid and display a *name*, so both go through the [`NameCache`]'s
//! ask-once resolve. The reference does not: it reads the object-name cache `0xc0e228` through
//! `0x55f080` with a **null callback**, so a miss returns 0 and the message is **silently dropped**
//! — no query, no defer, no line (decision 1738). That is a quirk of a synchronous object
//! manager that always has its party members to hand, not a behaviour worth reproducing: benilla's
//! cache may genuinely not hold a member yet, and losing "Thrall has declined your quest" because a
//! name query was in flight is strictly worse than showing it a frame later. So a verdict whose
//! name has not landed is kept and retried, with [`crate::ui_loot`]'s bounded budget so a guid the
//! server will never name cannot pin a line forever. [`crate::ui_duel`] takes the same divergence
//! against the same reference behaviour, and says so in its own header.

use benilla_protocol::messages::{QuestConfirmAccept, QuestShareMsg};
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, UiError};
use crate::ui_script::{UiFeed, UiInput};

/// How many frames a queued line waits for its `%s` before it is dropped — [`crate::ui_loot`]'s
/// `RECEIVE_MAX_TRIES`, and the same reasoning: a name query is one round trip, so any real answer
/// lands inside a couple of frames, and a guid the server declines to name (it caches the negative)
/// would otherwise hold its line for the rest of the session.
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
    /// The quest `ConfirmAcceptQuest()` answers — latched when the popup's event fires and kept
    /// after it, because the Lua verb takes no argument (ref `StaticPopup.lua:731-733`): the
    /// client answers the confirm it was last asked, and a *second* confirm replaces the first
    /// exactly as it replaces the server's own one-slot latch.
    confirm_quest: Option<u32>,
}

impl QuestShare {
    /// Queue one verdict on a quest we shared (`MSG_QUEST_PUSH_RESULT`).
    pub(crate) fn push_verdict(&mut self, member: u64, msg: QuestShareMsg) {
        self.verdicts.push(PendingVerdict {
            member,
            msg,
            tries: 0,
        });
    }

    /// Hold an escort confirm (`SMSG_QUEST_CONFIRM_ACCEPT`) for the feed to raise. A second
    /// confirm replaces a first that has not fired yet — the server keeps one latch, so the older
    /// question is already dead.
    pub(crate) fn set_confirm(&mut self, c: QuestConfirmAccept) {
        self.pending_confirm = Some(c);
    }

    /// The socket died: no verdict is worth showing after it and no confirm can still be answered
    /// (the server's latch went with the session).
    pub(crate) fn clear_session(&mut self) {
        self.verdicts.clear();
        self.pending_confirm = None;
        self.confirm_quest = None;
    }
}

/// The `ERR_QUEST_PUSH_*` GlobalStrings key one verdict shows, and the surface it shows on.
///
/// The value→key column is vmangos `QuestDef.h:62-70`, whose enum comments name the client's own
/// error constants one-for-one — the same corroboration decision 0669's refusal table started
/// from. `None` for an unmapped byte is the reference's data-suppression face, not a gap: an
/// unknown verdict shows nothing rather than an English guess.
///
/// **Both columns are now VERIFIED at the bytes** (decision 1738's fold-back). The inbound `0x276`
/// arm at `0x5e4781` maps `msg 0..8` onto message ids `0x181`-`0x189` — contiguous, in exactly this
/// order — and all nine records carry **`kind 0`** (the nine `push 0x0` at `0x487dc9`-`0x487e69`),
/// which `CGGameUI::DisplayError` dispatches to the chat window as `CHAT_MSG_SYSTEM`. Not one of
/// them reaches `UIErrorsFrame`, which is what 1733 guessed and is why the guess is recorded as
/// having been a guess.
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

/// The party quest-share's packet handlers (decision 1733; in the net handler table since 2320,
/// moved out of the drain's quests arm file): one member's verdict on a quest we pushed, and the
/// escort-quest confirm. Both park in [`QuestShare`] for this module to name and raise — the guid
/// needs a name query a packet handler cannot await.
mod net {
    use benilla_protocol::messages::{QuestConfirmAccept, QuestShareMsg};
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::QuestShare;
    use crate::net::NetHandlerApp;

    /// Register the handlers and the session-end listener — called from
    /// [`super::QuestSharePlugin`].
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

    /// A verdict on a share nobody is listening for any more, and a confirm whose server-side
    /// latch died with the socket (decision 1733). A listener on the session end
    /// (a second handler on the kind, after the bridge's own teardown).
    fn on_session_end(In(_): In<SessionEvent>, mut share: ResMut<QuestShare>) {
        share.clear_session();
    }

    /// One party member's verdict on a quest we shared (`MSG_QUEST_PUSH_RESULT`, decision 1733).
    ///
    /// Parked rather than shown: the line's `%s` is the member's NAME, which may still need a
    /// `CMSG_NAME_QUERY` round trip, and this pass has no VM to resolve GlobalStrings through either.
    /// [`crate::ui_quest_share`] owns both.
    fn quest_push_result(member: u64, msg: QuestShareMsg, share: &mut QuestShare) {
        debug!(
            "net: quest push result — member {member:#x} verdict {}",
            msg.0
        );
        share.push_verdict(member, msg);
    }

    /// A party member started a `QUEST_FLAGS_PARTY_ACCEPT` (escort) quest and we are being asked
    /// whether to start it too (`SMSG_QUEST_CONFIRM_ACCEPT`). Parked for the same reason: the popup
    /// names the member, and the name may not be cached yet.
    fn quest_confirm_accept(c: QuestConfirmAccept, share: &mut QuestShare) {
        debug!(
            "net: quest confirm accept — quest {} ({:?}) from {:#x}",
            c.quest_id, c.title, c.sender
        );
        share.set_confirm(c);
    }
}

/// Owns [`QuestShare`] and its two systems — a plugin of its own rather than a lodger in
/// [`crate::ui_quest`]'s, because nothing here is bound to the questgiver window: the verdicts are
/// fired from the quest LOG and the confirm has no window at all.
pub(crate) struct QuestSharePlugin;

impl Plugin for QuestSharePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // The [`crate::ui_duel`] shape exactly: feed before the input pass so a verdict line and
        // a confirm are on screen the same frame the packet landed, drain after it so the popup's
        // Yes goes out the same frame it was clicked.
        app.init_resource::<QuestShare>().add_systems(
            Update,
            (
                feed_quest_share.in_set(UiFeed),
                drain_quest_share.after(UiInput),
            ),
        );
    }
}

/// Resolve the queued names, then show what they unlocked: the verdict lines, and the escort
/// confirm's `QUEST_ACCEPT_CONFIRM`.
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

    // The escort confirm — held until the member's name lands, then raised with the reference's own
    // two args: `QUEST_ACCEPT = "%s is starting %s\nWould you like to as well?"`, player first.
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

/// `ConfirmAcceptQuest()` — the confirm popup's Yes, answered against the latched quest id.
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

    /// The nine keys are distinct — a copy-paste in the table would silently show one member's
    /// outcome as another's.
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

    /// The RUNTIME leg on the real client data ([`crate::ui_quest`]'s refusal-table test, same
    /// shape and same reason): every `ERR_QUEST_PUSH_*` key the table can emit resolves to a
    /// non-empty string in the shipped 1.12 `GlobalStrings.lua`, and every one of them carries the
    /// `%s` the member's name fills — a key that resolved but had no token would silently show the
    /// same line for every member. Skips without client data.
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

        // The two ends of the flow, filled: the opener the sharer sees the instant they click, and
        // the answer a decline produces.
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
