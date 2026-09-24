//! The chat window's packet handlers (in the net handler table since 2323, moved out of the
//! drain's chat arm file) — the spoken line itself, plus the server notices and query answers
//! that render as chat lines (the channel roster, whisper refusals, `/played`).
//!
//! Two handlers here do NOT render as chat: `SMSG_NOTIFICATION` and `SMSG_AREA_TRIGGER_MESSAGE`
//! are the reference's UIErrorsFrame toasts, and queue onto [`UiErrorTexts`]. They live in this
//! file because they are text-carrying server notices, not because they share a sink.

use benilla_protocol::messages::{ChatMessage, CHAT_MSG_WHISPER};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{Broadcast, ChatEvent, ChatEventKind, ChatLog};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, PlayedTimeAnswer, ServerSaidMessage};
use crate::ui_action::{UiErrorKeys, UiErrorTexts};
use crate::ui_social::SocialState;

/// Register the chat window's handlers — called from [`super::UiChatPlugin`]. One per kind
/// (the four world broadcasts share one), plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::Chat, on_chat)
        .net_handler(K::ChannelList, on_channel_list)
        .net_handler(K::ChatPlayerNotFound, on_whisper_refusal)
        .net_handler(K::ChatWrongFaction, on_whisper_refusal)
        .net_handler(K::ZoneUnderAttack, on_broadcast)
        .net_handler(K::DefenseMessage, on_broadcast)
        .net_handler(K::ServerMessage, on_broadcast)
        .net_handler(K::ChatRestricted, on_broadcast)
        .net_handler(K::Notification, on_toast)
        .net_handler(K::AreaTriggerMessage, on_toast)
        .net_handler(K::PlayedTime, on_played_time)
        .net_handler(K::ChannelNotify, on_channel_notify)
        .net_handler(K::RandomRoll, on_random_roll)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_chat(
    In(ev): In<SessionEvent>,
    mut chat_log: ResMut<ChatLog>,
    social: Res<SocialState>,
    commands: Res<NetCommands>,
    mut server_said: MessageWriter<ServerSaidMessage>,
) {
    if let SessionEvent::Chat(m) = ev {
        chat(m, &mut chat_log, &social, &commands, &mut server_said);
    }
}

fn on_channel_list(In(ev): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    if let SessionEvent::ChannelList {
        channel, members, ..
    } = ev
    {
        channel_list(channel, &members, &mut chat_log);
    }
}

fn on_whisper_refusal(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    match ev {
        SessionEvent::ChatPlayerNotFound { name } => chat_player_not_found(&name, &mut errors),
        SessionEvent::ChatWrongFaction => chat_wrong_faction(&mut errors),
        _ => {}
    }
}

/// The four world broadcasts — parked for [`super::broadcast`]'s resolve pass, which owns the
/// AreaTable/ServerMessages lookups and the joined-defense-channel walk.
fn on_broadcast(In(ev): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    let b = match ev {
        SessionEvent::ZoneUnderAttack { area_id } => Broadcast::ZoneUnderAttack { area_id },
        SessionEvent::DefenseMessage { zone_id, text } => Broadcast::Defense { zone_id, text },
        SessionEvent::ServerMessage { message_type, text } => {
            Broadcast::Server { message_type, text }
        }
        SessionEvent::ChatRestricted => Broadcast::ChatRestricted,
        _ => return,
    };
    broadcast(b, &mut chat_log);
}

fn on_toast(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorTexts>) {
    match ev {
        SessionEvent::Notification { text } => notification(text, &mut errors),
        SessionEvent::AreaTriggerMessage { text } => area_trigger_message(text, &mut errors),
        _ => {}
    }
}

/// BOTH halves, and they are not redundant. The chat breakdown is our stand-in for the
/// reference's `ChatFrame_DisplayTimePlayed`, which we do not ship; the mailbox is what becomes
/// `TIME_PLAYED_MSG(total, level)` for an addon that asked.
fn on_played_time(
    In(ev): In<SessionEvent>,
    mut answer: ResMut<PlayedTimeAnswer>,
    mut chat_log: ResMut<ChatLog>,
) {
    if let SessionEvent::PlayedTime { total, level } = ev {
        answer.0 = Some((total, level));
        played_time(total, level, &mut chat_log);
    }
}

fn on_channel_notify(In(ev): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    if let SessionEvent::ChannelNotify {
        notice,
        channel,
        tail,
    } = ev
    {
        chat_log.push_channel_notice(notice, channel, &tail);
    }
}

fn on_random_roll(In(ev): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    if let SessionEvent::RandomRoll {
        min,
        max,
        roll,
        guid,
    } = ev
    {
        chat_log.push_roll(min, max, roll, guid);
    }
}

/// The chat log's session state dies with the socket. A listener on the session end
/// (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(In(_): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    chat_log.clear_session();
}

/// A spoken line (`SMSG_MESSAGECHAT`) — the chat window's own feed (decision 0084):
/// [`super`] formats + colors per type, resolves the sender name ask-once, and
/// AddMessages it into ChatFrame1.
///
/// System lines (`CHAT_MSG_SYSTEM` 0x0A, vmangos `SharedDefines.h`) are the SERVER'S ANSWER to a GM
/// dot-command — "Premade gear template N applied", "No matching premade player template found",
/// "There is no such command". For a headless probe that is the only channel the server has to say
/// *why* something did not happen, so it rides at `info!`: at `debug!` it was in the log but
/// invisible at the default level, and a refused command read exactly like an applied one
/// (decision 0651 — the rig's whole batch silently no-op'd on a too-low GM level and nothing said
/// so). Ordinary chat stays at `debug!`: conversation, not diagnosis, and high volume.
fn chat(
    m: ChatMessage,
    chat_log: &mut ChatLog,
    social: &SocialState,
    net_commands: &NetCommands,
    server_said: &mut MessageWriter<ServerSaidMessage>,
) {
    // The ADDON gate (decision 1029, B215): a line whose `language` is `LANG_ADDON` is not speech
    // at all — it is one addon talking to another over the party/raid/guild/channel lane, because
    // 1.12.1 has no addon opcode and no addon `ChatMsg` type. The real client never renders it; it
    // fires `CHAT_MSG_ADDON` instead. Rendering it printed a partied real-client player's
    // `[Party] [Soreen]: Quiver VERSION:3.1.4` version ping as ordinary party chat, and a
    // mixed-client party makes that constant background noise.
    //
    // In `WoW.exe` (5875) the test is
    // `0x49a89b`/`0x49a89e cmp edi,-0x1`, in the DISPLAY function `0x49a870`, and every branch of
    // the addon arm `[0x49a8a7, 0x49a96d)` jumps below the normal render path at `0x49a970` — the
    // chat frame is unreachable for such a line by construction. The **language field is the whole
    // discriminator**: the type byte takes no part in the comparison, and the wire parser
    // `0x49d560` never inspects language at all.
    //
    // **Ignore beats addon**, so that check comes first even though it costs a duplicated call:
    // the reference drops an ignored sender's line at `0x49d72d`, *upstream* of the language test,
    // so an ignored player's addon traffic fires nothing at all. Unobservable while both paths
    // merely drop, but it is the precedence that has to already be right the day there is
    // something to fire. (`CMSG_CHAT_IGNORED` is not owed here — the server refuses `LANG_ADDON`
    // on WHISPER, the only type that answers on the wire.)
    //
    // **One divergence, named:** the reference suppresses *after* name resolution — an uncached
    // sender parks the line in `__AUPENDINGCHAT__` (`0x49d7ba`) behind `CMSG_NAME_QUERY`, and
    // `CHAT_MSG_ADDON` fires late from the callback `0x49ccc0`. We drop here instead, ahead of the
    // name query [`ChatLog::push_wire`] would queue, which saves a name query per addon sender.
    //
    // **THE CHANNEL IS NOW HALF-OPEN, and that is the state of it — not a claim that it works.**
    // Decision 1235 landed the SEND half: `SendAddonMessage` composes, validates against the
    // client's four-lane whitelist and puts a real `CMSG_MESSAGECHAT`/`LANG_ADDON` on the wire.
    // Nothing on THIS side changed, so an addon can broadcast and **cannot hear** — including its
    // own echo (vmangos's `Group::BroadcastPacket` takes no ignore-guid, so the party lane returns
    // a sender's own line, and an addon that sequences on that echo sees nothing).
    //
    // Opening the receive half is NOT deleting this `return`, which is why it is separate work:
    //
    //   - `CHAT_MSG_ADDON` (event 227, `0x49a95f push 0xe3`) is **not a `ChatTypeInfo` key** and
    //     cannot ride [`crate::ui_chat::event`]'s pipeline — it has its own format string and its
    //     own 4-argument shape `(prefix, message, distribution, sender)`, against the chat
    //     family's ten. `every_fired_event_name_is_a_chat_type_info_key` would rightly fail it.
    //   - `prefix`/`message` come from splitting the text on its **FIRST** tab (`0x49a8d0`); with
    //     no tab the whole text is the *prefix* and the message is `""`, not the other way round.
    //   - `distribution` is the remap table `0x49aff4` → jump table `0x49afe0`: only
    //     PARTY/RAID/GUILD/BATTLEGROUND get names, every other type byte reports `"UNKNOWN"`.
    //   - the *fire* must move downstream of the name resolve (`0x49ccc0`) so `sender` is a name,
    //     not a guid. The drop can stay here; the fire cannot.
    //
    // The payload rides `debug!` and its own `addon` trace tag rather than vanishing, so
    // mixed-client traffic stays diagnosable: invisible in chat, never invisible to us.
    if m.is_addon() {
        if !social.is_ignored(m.sender_guid) {
            debug!(
                "net: addon chat [{:#04x}] from {:#x}: {:?} (suppressed — not a chat line)",
                m.chat_type, m.sender_guid, m.text
            );
            if benilla_assets::trace::enabled() {
                // `<-` inbound, matching the `->` the outbound drain writes to this same tag
                // (`ui_chat::input::drain_addon_message_sends`): one tag, both directions, so a
                // live run's trace reads as the conversation it is.
                benilla_assets::trace::line(
                    "addon",
                    &format!("<- [{:#04x}] {:?}", m.chat_type, m.text),
                );
            }
            // **The channel is whole now.** Parked for the sender-name resolve rather than fired
            // here, because the reference fires `CHAT_MSG_ADDON` DOWNSTREAM of the name query
            // (`0x49ccc0`) so `sender` is a name and never a guid — the one requirement of the
            // four that is about WHERE rather than what. `push_addon` carries the other three: the
            // first-tab split, the no-tab-means-all-prefix direction, and the four-lane
            // distribution remap.
            chat_log.push_addon(&m.text, m.chat_type, m.sender_guid);
        }
        // The line still never reaches a chat window: an addon message is not speech, and the
        // reference's addon arm jumps below the render path (`0x49a970`). An IGNORED sender fires
        // nothing at all — ignore beats addon, which is why that check still wraps this.
        return;
    }
    if m.chat_type == 0x0A {
        info!("net: server says — {}", m.text);
        // …and as a message, for the senders that must know whether their command landed. Server
        // state with no descriptor field (god mode) has no other tell.
        server_said.write(ServerSaidMessage {
            text: m.text.clone(),
        });
    } else {
        debug!("net: chat [{:#04x}] {}", m.chat_type, m.text);
    }
    // …and on the trace clock too (decision 0624). A GM dot-command is the only way to ask the
    // SERVER what it believes — `.gps` reads back the server-side position of a mover whose packets
    // may or may not be reaching it — and its answer is only usable if it lands on the same
    // timeline as the `snd`/`rly`/`run` lines it must be read against. `debug!` timestamps are
    // wall-clock in a different format; this is one clock, one file.
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "sys",
            &format!("[{:#04x}] {}", m.chat_type, m.text.replace('\n', " ⏎ ")),
        );
    }
    // The ignore gate (decision 0668): an ignored speaker is dropped SILENTLY — no line at all —
    // which is the client's own `FriendList::IsIgnored 0x5ae5a0` check, read on the sibling
    // text-emote path (`0x49dbe0`). A dropped WHISPER
    // additionally tells the server, so the sender gets the "is ignoring you" answer: that is what
    // `CMSG_CHAT_IGNORED` is for, and only the client can send it.
    if social.is_ignored(m.sender_guid) {
        if m.chat_type == CHAT_MSG_WHISPER {
            let _ = net_commands.0.send(ClientCommand::ChatIgnored {
                guid: m.sender_guid,
            });
        }
        return;
    }
    chat_log.push_wire(m);
}

/// The `/chatlist` roster (`SMSG_CHANNEL_LIST`) — CHAT_CHANNEL_LIST_GET "[%s] " + the roster.
/// Names arrive as guids; v1 renders the count (the per-member resolve fan-out lands with the
/// P6 channel wiring — /chatlist is rare enough that a count is honest, never wrong).
fn channel_list(channel: String, members: &[(u64, u8)], chat_log: &mut ChatLog) {
    let mut ev = ChatEvent::text_only(
        ChatEventKind::ChannelList,
        format!("{} member(s)", members.len()),
    );
    ev.channel = channel;
    chat_log.push_event(ev);
}

/// A whisper target wasn't online — `ERR_CHAT_PLAYER_NOT_FOUND_S`, catalog row 241, whose `%s`
/// the server's own name fills.
///
/// The KEY travels rather than a composed sentence (decision 2045): this is the net-apply pass and
/// there is no VM here, so `ui_action`'s drain is what resolves it against the player's own
/// `GlobalStrings.lua` — and the row, not this call site, is what says the line goes to chat
/// (`kind 0`) and makes no sound.
fn chat_player_not_found(name: &str, errors: &mut crate::ui_action::UiErrorKeys) {
    errors.0.push(crate::ui_action::UiError::s(
        "ERR_CHAT_PLAYER_NOT_FOUND_S",
        name,
    ));
}

/// A cross-faction whisper was refused — `ERR_CHAT_WRONG_FACTION`, catalog row 240. Same route and
/// the same reason as [`chat_player_not_found`], with no argument to fill.
fn chat_wrong_faction(errors: &mut crate::ui_action::UiErrorKeys) {
    errors
        .0
        .push(crate::ui_action::UiError::key("ERR_CHAT_WRONG_FACTION"));
}

/// A server notice (`SMSG_NOTIFICATION`) — the **red UIErrorsFrame line**, never a chat line.
///
/// Byte-verified in the reference: opcode `0x1cb`'s handler is `0x401800` (registered at
/// `0x40172f`, torn down at `0x401f09`), whose whole body is "read the cstring, then
/// `mov edx,1; lea ecx,[buf]; call 0x4945b0`" plus a console log (`0x63cd00(…, 3, buf)` — our
/// `info!` below). `0x4945b0(text, 1)` fires FrameScript event `0xe0` = `UI_ERROR_MESSAGE`.
/// Nothing on this path touches the chat composer.
///
/// **This used to push into the chat feed**, as a stand-in from before benilla had an errors
/// frame. It has had a real one for a long time — the ref `UIErrorsFrame`, a genuine
/// `MessageFrame` (ours until 1751's fourteenth window, the chain's own
/// `Interface\FrameXML\UIErrorsFrame.xml` since) — and the stand-in outlived its reason: vmangos
/// `Player::SetGameMaster` answers `.gm on|off` with **both** `SendSysMessage` and
/// `SendNotification` (`Objects/Player.cpp:2676-2677`/`2701-2702`), so a client that sinks the
/// notification into chat prints "GM mode is ON" **twice** where the reference prints it once.
fn notification(text: String, errors: &mut UiErrorTexts) {
    // The handler's own console leg, and the same reason 0651 logs the dot-command answers: a
    // toast that flashed for five seconds and one that never arrived look identical afterwards.
    info!("net: notification — {text}");
    errors.error(text);
}

/// An area trigger's refusal (`SMSG_AREA_TRIGGER_MESSAGE`) — "You must be at least level 58 to
/// enter…", "You cannot enter … while in ghost form." The same frame as [`notification`], the
/// **yellow** arm: `0x2b8` shares the multi-opcode handler `0x48f690`, whose arm at `0x48f8ff`
/// ends `xor edx,edx; call 0x4945b0` — flag 0, so event `0xe1` = `UI_INFO_MESSAGE`. (The older
/// comment here called it the same sink as the notification and left it at that; the sink is the
/// same, the arm is not.) Without it, a refused portal is silent, which reads exactly like a
/// portal that is still broken.
fn area_trigger_message(text: String, errors: &mut UiErrorTexts) {
    // Logged for the same reason 0651 logs the server's dot-command answers: a trigger that
    // refused and a trigger the client never noticed look identical from outside.
    info!("net: area-trigger message — {text}");
    errors.info(text);
}

/// The four **world broadcasts** — parked on [`ChatLog`]'s broadcast queue for
/// [`super`]'s resolve pass, which holds the AreaTable/ServerMessages catalogs and the
/// joined-channel walk this site does not (`ui_chat::broadcast` carries the whole mechanism).
///
/// Only the parking happens here, deliberately: half-resolving at the packet — naming the area
/// here and picking the channels there — would put one mechanism in two files.
///
/// They are logged at `info!` for the same reason [`notification`] is, and more so: three of the
/// four are things the *world* did, so "the server is restarting in 15 minutes" and "nothing
/// arrived" have to be distinguishable in a log afterwards. A defense broadcast that reaches a
/// character in neither defense channel prints nothing on screen and is faithful in doing so — the
/// log line is the only trace it happened at all.
fn broadcast(b: Broadcast, chat_log: &mut ChatLog) {
    info!("net: world broadcast — {b:?}");
    chat_log.push_broadcast(b);
}

/// The `/played` answer (`SMSG_PLAYED_TIME`) — TIME_PLAYED_TOTAL/LEVEL over
/// TIME_DAYHOURMINUTESECOND (GlobalStrings:4243-4247; the ref's
/// ChatFrame_DisplayTimePlayed breakdown).
fn played_time(total: u32, level: u32, chat_log: &mut ChatLog) {
    for (label, secs) in [
        ("Total time played", total),
        ("Time played this level", level),
    ] {
        let (d, rem) = (secs / 86_400, secs % 86_400);
        let (h, rem) = (rem / 3_600, rem % 3_600);
        let (m, sec) = (rem / 60, rem % 60);
        chat_log.push_event(ChatEvent::text_only(
            ChatEventKind::System,
            format!("{label}: {d} days, {h} hours, {m} minutes, {sec} seconds"),
        ));
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use benilla_protocol::messages::{CHAT_MSG_PARTY, CHAT_MSG_SYSTEM, LANGUAGE_ADDON};

    fn a_party_line(language: u32, text: &str) -> ChatMessage {
        ChatMessage {
            chat_type: CHAT_MSG_PARTY,
            language,
            sender_guid: 0x21,
            target_guid: 0x21,
            sender_name: None,
            channel: None,
            text: text.to_string(),
            chat_tag: 0,
        }
    }

    /// Run one inbound line through the real arm body and report whether it reached the chat log.
    fn reaches_the_chat_window(m: ChatMessage) -> bool {
        chat_lines(m) > 0
    }

    /// How many lines one inbound message put in the chat window.
    fn chat_lines(m: ChatMessage) -> usize {
        let mut world = World::new();
        world.init_resource::<Messages<ServerSaidMessage>>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.insert_resource(SocialState::default());
        world.insert_resource(ChatLog::default());
        world
            .run_system_once(
                move |mut chat_log: ResMut<ChatLog>,
                      social: Res<SocialState>,
                      net_commands: Res<NetCommands>,
                      mut server_said: MessageWriter<ServerSaidMessage>| {
                    chat(
                        m.clone(),
                        &mut chat_log,
                        &social,
                        &net_commands,
                        &mut server_said,
                    );
                },
            )
            .unwrap();
        world.resource::<ChatLog>().pending_len()
    }

    /// B215 / decision 1029: an addon broadcast never reaches the chat window. It arrives as an
    /// ordinary `CHAT_MSG_PARTY` — the lane a mixed-client party's hunter addon talks over — so the
    /// `language` sentinel is the only thing standing between it and a rendered `[Party]` line.
    #[test]
    fn addon_chat_never_reaches_the_chat_window() {
        assert!(
            !reaches_the_chat_window(a_party_line(LANGUAGE_ADDON, "Quiver\tVERSION:3.1.4")),
            "an addon broadcast must be dropped before the chat log"
        );
        // A payload with no tab is still addon traffic — the gate reads the language field, never
        // the shape of the text.
        assert!(!reaches_the_chat_window(a_party_line(
            LANGUAGE_ADDON,
            "nopayloadseparator"
        )));
        // An empty addon payload likewise: nothing about the text can un-addon a line.
        assert!(!reaches_the_chat_window(a_party_line(LANGUAGE_ADDON, "")));
    }

    /// ...and since the receive half opened, "never reaches the chat window" must not be allowed to
    /// mean "vanished". The same line is PARKED for `CHAT_MSG_ADDON`, which is the distinction that
    /// stopped being free the moment `Pending::Addon` existed — `pending_len` excludes addon items
    /// precisely so the test above keeps asserting what it says, and this one covers the other half.
    #[test]
    fn an_addon_line_is_parked_for_the_addon_event_not_discarded() {
        let mut world = World::new();
        world.init_resource::<Messages<ServerSaidMessage>>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.insert_resource(SocialState::default());
        world.insert_resource(ChatLog::default());
        let m = a_party_line(LANGUAGE_ADDON, "Quiver\tVERSION:3.1.4");
        world
            .run_system_once(
                move |mut chat_log: ResMut<ChatLog>,
                      social: Res<SocialState>,
                      net_commands: Res<NetCommands>,
                      mut server_said: MessageWriter<ServerSaidMessage>| {
                    chat(
                        m.clone(),
                        &mut chat_log,
                        &social,
                        &net_commands,
                        &mut server_said,
                    );
                },
            )
            .unwrap();
        let log = world.resource::<ChatLog>();
        assert_eq!(
            log.pending_addons(),
            vec![(
                "Quiver".to_string(),
                "VERSION:3.1.4".to_string(),
                "PARTY".to_string()
            )],
            "the line must be queued for CHAT_MSG_ADDON, not dropped"
        );
        assert_eq!(log.pending_len(), 0, "and still never headed for a window");
    }

    /// The control the gate must not swallow: real speech on the same lane, in every tongue a
    /// character can speak. A gate that keyed on "unknown language" instead of the sentinel would
    /// silently eat Demonic and Gutterspeak.
    #[test]
    fn speech_on_the_addon_lane_still_renders() {
        for language in [0, 1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 33] {
            assert!(
                reaches_the_chat_window(a_party_line(language, "party control line")),
                "language {language} is a tongue — the line must render"
            );
        }
    }

    /// **The GM-mode double line** — the director's report, pinned end to end.
    ///
    /// vmangos answers `.gm on|off` with **two** packets, not one: `Player::SetGameMaster` calls
    /// `SendSysMessage(LANG_GM_ON)` *and* `GetSession()->SendNotification(LANG_GM_ON)`
    /// (`src/game/Objects/Player.cpp:2676-2677` and `2701-2702`). They carry the same words down
    /// different roads, and only the first is a chat line: the reference's `SMSG_NOTIFICATION`
    /// handler `0x401800` is "read the cstring, `mov edx,1`, `call 0x4945b0`" — event `0xe0`
    /// `UI_ERROR_MESSAGE`, the UIErrorsFrame toast — plus a console log, and it never reaches the
    /// chat composer. Benilla used to park the notice in the chat feed for want of an errors
    /// frame; it has had one since our own `ErrorsFrame.xml` (the chain's `UIErrorsFrame.xml`
    /// since 1751), and the stand-in was what printed "GM mode is ON" twice.
    ///
    /// The pair is the test: BOTH halves of one toggle, one chat line, one toast.
    #[test]
    fn toggling_gm_mode_prints_one_chat_line_and_one_toast() {
        let sys_line = ChatMessage {
            chat_type: CHAT_MSG_SYSTEM,
            ..a_party_line(0, "GM mode is ON")
        };
        assert_eq!(
            chat_lines(sys_line),
            1,
            "the SendSysMessage half is the chat line, and it stays"
        );

        let mut errors = UiErrorTexts::default();
        notification("GM mode is ON".to_string(), &mut errors);
        assert_eq!(
            errors.0,
            [(
                "GM mode is ON".to_string(),
                benilla_ui::messages::MsgKind::Error
            )],
            "the SendNotification half is the RED toast (0x4945b0's flag 1), never a second line"
        );
    }

    /// The notification's sibling arm shares the frame and **not** the colour: `0x2b8`
    /// `SMSG_AREA_TRIGGER_MESSAGE` runs the multi-opcode handler `0x48f690`, whose arm at
    /// `0x48f8ff` ends `xor edx,edx; call 0x4945b0` — flag 0, so event `0xe1` `UI_INFO_MESSAGE`,
    /// the yellow line. This is the half the old "same sink as the notification" comment missed.
    #[test]
    fn an_area_trigger_refusal_takes_the_yellow_arm() {
        let mut errors = UiErrorTexts::default();
        area_trigger_message(
            "You must be at least level 58 to enter.".to_string(),
            &mut errors,
        );
        assert_eq!(
            errors.0,
            [(
                "You must be at least level 58 to enter.".to_string(),
                benilla_ui::messages::MsgKind::Info
            )]
        );
    }
}
