//! The chat window's packet handlers: the spoken line, and the notices and answers that print as
//! chat. `SMSG_NOTIFICATION` and `SMSG_AREA_TRIGGER_MESSAGE` are `UIErrorsFrame` toasts instead,
//! queued on [`UiErrorTexts`].

use benilla_protocol::messages::{ChatMessage, CHAT_MSG_WHISPER};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{Broadcast, ChatEvent, ChatEventKind, ChatLog};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, PlayedTimeAnswer, ServerSaidMessage};
use crate::ui_action::{UiErrorKeys, UiErrorTexts};
use crate::ui_social::SocialState;

/// Register the chat window's handlers and its session-end listener.
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

/// The four world broadcasts, parked for [`super::broadcast`]'s resolve pass.
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

/// Print the `/played` breakdown, and fill the mailbox `feed::played_time_bridge` fires as
/// `TIME_PLAYED_MSG`.
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

/// Clear the chat log's session state when the session ends.
fn on_session_end(In(_): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    chat_log.clear_session();
}

/// A spoken line (`SMSG_MESSAGECHAT`). System lines (`0x0A`), the server's answers to GM
/// commands, log at `info!` and go out as [`ServerSaidMessage`]; other chat logs at `debug!`.
fn chat(
    m: ChatMessage,
    chat_log: &mut ChatLog,
    social: &SocialState,
    net_commands: &NetCommands,
    server_said: &mut MessageWriter<ServerSaidMessage>,
) {
    // Addon traffic: 1.12 has no addon opcode or chat type, so `LANG_ADDON` on any lane is the
    // whole test (`0x49a89e`, in the display function `0x49a870`). The line never renders
    // (`0x49a970`); it fires `CHAT_MSG_ADDON` once the sender's name resolves (`0x49d7ba`,
    // `0x49ccc0`). An ignored sender's fires nothing, as the reference drops ignored lines first
    // (`0x49d72d`), and needs no `CMSG_CHAT_IGNORED`: vmangos refuses addon whispers
    // (`ChatHandler.cpp:84-103`).
    if m.is_addon() {
        if !social.is_ignored(m.sender_guid) {
            debug!(
                "net: addon chat [{:#04x}] from {:#x}: {:?} (suppressed — not a chat line)",
                m.chat_type, m.sender_guid, m.text
            );
            if benilla_assets::trace::enabled() {
                // `<-` inbound; the outbound drain writes `->` under the same tag.
                benilla_assets::trace::line(
                    "addon",
                    &format!("<- [{:#04x}] {:?}", m.chat_type, m.text),
                );
            }
            chat_log.push_addon(&m.text, m.chat_type, m.sender_guid);
        }
        return;
    }
    if m.chat_type == 0x0A {
        info!("net: server says — {}", m.text);
        // Also as a message: server state with no descriptor field (god mode) has no other tell.
        server_said.write(ServerSaidMessage {
            text: m.text.clone(),
        });
    } else {
        debug!("net: chat [{:#04x}] {}", m.chat_type, m.text);
    }
    // On the trace clock too, so a `.gps` answer lines up with the movement trace.
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "sys",
            &format!("[{:#04x}] {}", m.chat_type, m.text.replace('\n', " ⏎ ")),
        );
    }
    // An ignored speaker is dropped silently (`FriendList::IsIgnored`, `0x5ae5a0`); a dropped
    // whisper also sends `CMSG_CHAT_IGNORED`, so the sender hears "is ignoring you".
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

/// The `/chatlist` roster (`SMSG_CHANNEL_LIST`). Members arrive as guids and are not resolved,
/// so the line gives their count where the reference lists their names (`0x49c690`).
fn channel_list(channel: String, members: &[(u64, u8)], chat_log: &mut ChatLog) {
    let mut ev = ChatEvent::text_only(
        ChatEventKind::ChannelList,
        format!("{} member(s)", members.len()),
    );
    ev.channel = channel;
    chat_log.push_event(ev);
}

/// A whisper target is offline: message 241, `ERR_CHAT_PLAYER_NOT_FOUND_S`, with the server's
/// name. The key travels, since there is no VM here; its row routes it to chat.
fn chat_player_not_found(name: &str, errors: &mut crate::ui_action::UiErrorKeys) {
    errors.0.push(crate::ui_action::UiError::s(
        "ERR_CHAT_PLAYER_NOT_FOUND_S",
        name,
    ));
}

/// A cross-faction whisper was refused: message 240, `ERR_CHAT_WRONG_FACTION`.
fn chat_wrong_faction(errors: &mut crate::ui_action::UiErrorKeys) {
    errors
        .0
        .push(crate::ui_action::UiError::key("ERR_CHAT_WRONG_FACTION"));
}

/// `SMSG_NOTIFICATION` (`0x1cb`, handler `0x401800`): the red `UIErrorsFrame` line,
/// `UI_ERROR_MESSAGE` from `0x4945b0(text, 1)`, never a chat line. vmangos sends `.gm on` as both
/// a system line and a notification (`Objects/Player.cpp:2676-2677`).
fn notification(text: String, errors: &mut UiErrorTexts) {
    // The reference also logs it to the console (`0x63cd00`).
    info!("net: notification — {text}");
    errors.error(text);
}

/// An area trigger's refusal (`SMSG_AREA_TRIGGER_MESSAGE`, `0x2b8`, arm `0x48f8ff` of
/// `0x48f690`): the yellow `UIErrorsFrame` line, `UI_INFO_MESSAGE` from `0x4945b0(text, 0)`.
fn area_trigger_message(text: String, errors: &mut UiErrorTexts) {
    info!("net: area-trigger message — {text}");
    errors.info(text);
}

/// Park a world broadcast for `ui_chat::broadcast`'s resolve pass, logged: a defense broadcast
/// outside the defense channels prints nothing, so the log is its only trace.
fn broadcast(b: Broadcast, chat_log: &mut ChatLog) {
    info!("net: world broadcast — {b:?}");
    chat_log.push_broadcast(b);
}

/// The `/played` answer as two system lines, in English, shaped like `TIME_PLAYED_TOTAL` and
/// `TIME_PLAYED_LEVEL` over `TIME_DAYHOURMINUTESECOND` (`GlobalStrings.lua:4243-4247`).
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

    /// An addon line rides an ordinary lane; only its `language` keeps it out of the window.
    #[test]
    fn addon_chat_never_reaches_the_chat_window() {
        assert!(
            !reaches_the_chat_window(a_party_line(LANGUAGE_ADDON, "Quiver\tVERSION:3.1.4")),
            "an addon broadcast must be dropped before the chat log"
        );
        // No tab, or no text: still addon traffic, since the gate reads the language alone.
        assert!(!reaches_the_chat_window(a_party_line(
            LANGUAGE_ADDON,
            "nopayloadseparator"
        )));
        assert!(!reaches_the_chat_window(a_party_line(LANGUAGE_ADDON, "")));
    }

    /// `pending_len` excludes parked addon items; this checks the line is parked, not lost.
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

    #[test]
    fn speech_on_the_addon_lane_still_renders() {
        for language in [0, 1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 33] {
            assert!(
                reaches_the_chat_window(a_party_line(language, "party control line")),
                "language {language} is a tongue — the line must render"
            );
        }
    }

    /// `.gm on` arrives as a system line and a notification (`Objects/Player.cpp:2676-2677`): one
    /// chat line, one red toast.
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
