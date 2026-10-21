//! A joined channel's slot through its life: taken when the join is sent, confirmed by the server's
//! notice, freed after the line of a notice that drops it, and what the verbs that read it see.

use benilla_protocol::messages::channel_notice as n;
use benilla_ui::script::{ChannelCommand, UiScript};

use super::edit::{ChannelSlot, ChannelState, SlotState};
use super::event::{ChatEvent, ChatEventKind};
use crate::net::{ClientCommand, NetCommands};

/// Rows 1, 2, 22 and 23 as `ChatChannels.dbc` has them.
fn catalog() -> benilla_formats::ChatChannelsCatalog {
    benilla_formats::ChatChannelsCatalog::from_rows(
        [
            (1, 0x0_0003, "General - %s", "General"),
            (2, 0x0_003B, "Trade - %s", "Trade"),
            (22, 0x1_0003, "LocalDefense - %s", "LocalDefense"),
            (23, 0x1_0004, "WorldDefense", "WorldDefense"),
        ]
        .into_iter()
        .map(
            |(id, flags, pattern, shortcut)| benilla_formats::ChatChannelRow {
                id,
                flags,
                pattern: pattern.into(),
                shortcut: shortcut.into(),
            },
        )
        .collect(),
    )
}

/// The verbs' world: the slots, a VM, the wire and the two sinks.
struct Verbs {
    channels: ChannelState,
    script: UiScript,
    commands: NetCommands,
    sent: crossbeam_channel::Receiver<ClientCommand>,
    log: super::ChatLog,
    errors: crate::ui_action::UiErrorKeys,
}

impl Verbs {
    fn new(joined: Vec<Option<ChannelSlot>>) -> Self {
        let (tx, sent) = crossbeam_channel::unbounded();
        Verbs {
            channels: ChannelState {
                joined,
                channels: catalog(),
                zone_mask: Some(0),
                ..Default::default()
            },
            script: UiScript::new().expect("a VM"),
            commands: NetCommands(tx),
            sent,
            log: super::ChatLog::default(),
            errors: crate::ui_action::UiErrorKeys::default(),
        }
    }

    fn apply(&mut self, cmd: ChannelCommand) {
        super::input::apply_channel_command(
            cmd,
            &mut self.channels,
            &mut self.script,
            &self.commands,
            &self.log,
            &self.errors,
        );
    }

    fn sent(&self) -> Vec<ClientCommand> {
        self.sent.try_iter().collect()
    }

    fn error_keys(&self) -> Vec<&'static str> {
        self.errors.take().into_iter().map(|e| e.key).collect()
    }

    /// One server notice through [`super::feed::deliver`], returned as it was rendered.
    fn notice(&mut self, byte: u8, channel: &str) -> ChatEvent {
        let mut event =
            super::feed::notice_event(byte, channel.into(), None, None).expect("a line");
        let mut windows = super::frames::ChatWindows::default();
        super::feed::deliver(
            &mut self.script,
            &mut windows,
            &mut self.channels,
            &mut event,
        );
        event
    }
}

fn custom(name: &str) -> Option<ChannelSlot> {
    Some(ChannelSlot::confirmed(name, 0))
}

fn join(name: &str, id: u32) -> ChannelCommand {
    ChannelCommand::Join {
        name: name.into(),
        password: String::new(),
        id,
    }
}

/// The join body registers the slot before it builds the packet (`0x49edf3`, then `0x49ee02`): the
/// allocator `0x49b980` fills it in state 1 (`0x49ba0f`) with the row's id (`0x49ba01`).
#[test]
fn a_join_takes_its_slot_when_it_is_sent() {
    let mut v = Verbs::new(vec![custom("World")]);
    v.apply(join("General - Elwynn Forest", 1));
    assert!(matches!(&v.sent()[..],
        [ClientCommand::JoinChannel { name, .. }] if name == "General - Elwynn Forest"));
    assert_eq!(v.channels.number_of("General - Elwynn Forest"), Some(2));
    assert_eq!(
        v.channels.slot_state("General - Elwynn Forest"),
        Some(SlotState::Pending),
        "no notice yet: the slot waits for the server"
    );
    assert_eq!(v.channels.joined[1].as_ref().map(|s| s.id), Some(1));
}

/// The `YOU_JOINED` arm only confirms (`0x49c0c2` → `0x49bb20`): the first slot of the name not
/// yet in state 0 takes state 0 and sets its id's mask bit (`0x49bb8b`, `0x49bbaf`); a notice for a
/// name with no slot creates none and sets no bit.
#[test]
fn the_servers_join_notice_confirms_a_slot_and_creates_none() {
    let mut v = Verbs::new(Vec::new());
    v.apply(join("General - Elwynn Forest", 1));
    v.notice(n::YOU_JOINED, "General - Elwynn Forest");
    assert_eq!(
        v.channels.slot_state("General - Elwynn Forest"),
        Some(SlotState::Confirmed)
    );
    assert_eq!(v.channels.zone_mask, Some(0b1), "row 1's bit");

    v.notice(n::YOU_JOINED, "Trade - City");
    assert_eq!(
        v.channels.number_of("Trade - City"),
        None,
        "the join notice of a channel with no slot makes none"
    );
    assert_eq!(v.channels.zone_mask, Some(0b1), "and sets no bit");
}

/// With ten slots taken the allocator shows `0x199`, `ERR_TOO_MANY_CHAT_CHANNELS`
/// (`0x49b9c0`-`0x49b9ca`), and makes none; the join body ignores that (`0x49edf3`) and sends
/// (`0x49ee02`-`0x49ee42`).
#[test]
fn an_eleventh_join_shows_too_many_channels_and_is_still_sent() {
    let mut v = Verbs::new((1..=10).map(|i| custom(&format!("Chan{i}"))).collect());
    v.apply(join("Eleventh", 0));
    assert_eq!(v.error_keys(), ["ERR_TOO_MANY_CHAT_CHANNELS"]);
    assert!(matches!(&v.sent()[..],
        [ClientCommand::JoinChannel { name, .. }] if name == "Eleventh"));
    assert_eq!(v.channels.number_of("Eleventh"), None);

    v.apply(join("Chan3", 0));
    assert!(v.error_keys().is_empty(), "a held name is no new slot");
}

/// The notify handler sets its drop flag for `WRONG_PASSWORD` (`0x49c13c`), `NOT_MEMBER` and
/// `INVALID_NAME` (`0x49c46d`) and `WRONG_FACTION` (`0x49c446`), and after the line
/// (`0x49c5b0`) frees the slot of the notice's name (`0x49c5b5`-`0x49c5c2`). The line keeps the
/// number; the slot is no longer written to the chat cache (`0x499b90`-`0x499baa`).
#[test]
fn the_four_refusals_free_the_slot_after_their_line() {
    for byte in [
        n::WRONG_PASSWORD,
        n::NOT_MEMBER,
        n::WRONG_FACTION,
        n::INVALID_NAME,
    ] {
        let mut v = Verbs::new(vec![custom("World"), custom("MyChan")]);
        v.channels.custom = vec!["World".into(), "MyChan".into()];
        let line = v.notice(byte, "MyChan");
        assert_eq!(
            line.channel, "2. MyChan",
            "notice {byte}: the line is numbered"
        );
        assert_eq!(v.channels.number_of("MyChan"), None, "notice {byte}");
        assert_eq!(v.channels.number_of("World"), Some(1), "notice {byte}");
        assert_eq!(v.channels.custom, ["World"], "notice {byte}");
    }
    let mut v = Verbs::new(vec![custom("MyChan")]);
    v.notice(n::NOT_MODERATOR, "MyChan");
    assert_eq!(
        v.channels.number_of("MyChan"),
        Some(1),
        "any other refusal keeps it"
    );
}

/// The name a channel command went out under.
fn wire_name(c: &ClientCommand) -> &str {
    use ClientCommand as W;
    match c {
        W::ChannelList { name }
        | W::ChannelOwner { name }
        | W::ChannelModerate { name }
        | W::ChannelAnnouncements { name }
        | W::ChannelSetOwner { name, .. }
        | W::ChannelPassword { name, .. }
        | W::ChannelBan { name, .. }
        | W::ChannelInvite { name, .. }
        | W::ChannelKick { name, .. }
        | W::ChannelModerator { name, .. }
        | W::ChannelUnmoderator { name, .. }
        | W::ChannelMute { name, .. }
        | W::ChannelUnmute { name, .. }
        | W::ChannelUnban { name, .. } => name,
        other => panic!("not a channel command: {other:?}"),
    }
}

/// The fourteen admin verbs pass argument 1 through `0x49e880` (`0x4a014f`, `0x4a03df`,
/// `0x4a04f5`): a nonzero `SStrToInt` is the name of a slot in state 0 (`0x49be50`), and a number
/// naming none ends the call before the packet (`0x4a015a`, `0x4a03ea`, `0x4a0500`).
#[test]
fn the_admin_verbs_send_a_numbers_confirmed_slot_or_nothing() {
    let each = |key: &str| -> Vec<ChannelCommand> {
        let (name, player, password) = (key.to_string(), "Bob".to_string(), "pw".to_string());
        use ChannelCommand as C;
        vec![
            C::List { name: name.clone() },
            C::DisplayOwner { name: name.clone() },
            C::Moderate { name: name.clone() },
            C::ToggleAnnouncements { name: name.clone() },
            C::SetOwner {
                name: name.clone(),
                player: player.clone(),
            },
            C::SetPassword {
                name: name.clone(),
                password,
            },
            C::Ban {
                name: name.clone(),
                player: player.clone(),
            },
            C::Invite {
                name: name.clone(),
                player: player.clone(),
            },
            C::Kick {
                name: name.clone(),
                player: player.clone(),
            },
            C::Moderator {
                name: name.clone(),
                player: player.clone(),
            },
            C::Unmoderator {
                name: name.clone(),
                player: player.clone(),
            },
            C::Mute {
                name: name.clone(),
                player: player.clone(),
            },
            C::Unmute {
                name: name.clone(),
                player: player.clone(),
            },
            C::Unban { name, player },
        ]
    };
    let mut v = Verbs::new(vec![
        Some(ChannelSlot::confirmed("General - Elwynn Forest", 1)),
        Some(ChannelSlot::pending("MyChan", 0)),
    ]);
    for (key, wire) in [
        ("1", Some("General - Elwynn Forest")),
        ("2", None),
        ("9", None),
        ("MyChan", Some("MyChan")),
        ("0", Some("0")),
    ] {
        for cmd in each(key) {
            v.apply(cmd);
        }
        let sent = v.sent();
        match wire {
            Some(wire) => {
                assert_eq!(sent.len(), 14, "{key:?}");
                assert!(
                    sent.iter().all(|c| wire_name(c) == wire),
                    "{key:?}: {sent:?}"
                );
            }
            None => assert!(sent.is_empty(), "{key:?} names no confirmed slot: {sent:?}"),
        }
    }
}

/// A bare `/chatlist` is `ListChannels` (`0x4a01c0`): `"[%d. %s] "` (`0x844c94`) for each slot in
/// state 0 (`0x4a01f2`-`0x4a01fb`), posted as one `CHAT_MSG_CHANNEL_LIST` line with no channel
/// (`0x4a02a1`-`0x4a02c0`), empty or not.
#[test]
fn list_channels_posts_the_confirmed_slots_as_one_channel_list_line() {
    let mut v = Verbs::new(vec![
        Some(ChannelSlot::confirmed("General - Elwynn Forest", 1)),
        Some(ChannelSlot::pending("Trade - Elwynn Forest", 2)),
        Some(ChannelSlot::confirmed("LocalDefense - Elwynn Forest", 22)),
        None,
        Some(ChannelSlot {
            state: SlotState::Renamed,
            ..ChannelSlot::confirmed("General - Westfall", 1)
        }),
        custom("MyChan"),
    ]);
    v.apply(ChannelCommand::ListAll);
    let events = v.log.pending_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, Some(ChatEventKind::ChannelList));
    assert_eq!(
        events[0].text,
        "[1. General - Elwynn Forest] [3. LocalDefense - Elwynn Forest] [6. MyChan] "
    );
    assert_eq!(events[0].channel, "", "no channel argument");
    assert!(v.sent().is_empty());

    let mut v = Verbs::new(Vec::new());
    v.apply(ChannelCommand::ListAll);
    let events = v.log.pending_events();
    assert_eq!(
        (events.len(), events[0].text.as_str()),
        (1, ""),
        "an empty list still posts"
    );
}

/// The stock handler prints `arg1` alone for a channel list line with no channel
/// (`ChatFrame.lua:1408-1411`), and the log file the same.
#[test]
fn a_channel_list_line_with_no_channel_is_its_text_alone() {
    let mut event = ChatEvent::text_only(ChatEventKind::ChannelList, "[1. General] ".into());
    let get = |_: &str| Some("[%s] ".to_string());
    assert_eq!(
        super::frames::compose(&event, ChatEventKind::ChannelList, "", &get).as_deref(),
        Some("[1. General] ")
    );
    event.channel = "MyChan".into();
    assert_eq!(
        super::frames::compose(&event, ChatEventKind::ChannelList, "", &get).as_deref(),
        Some("[MyChan] [1. General] "),
        "a member list keeps its header"
    );
}

/// A line's arg7 is its slot's `+0x94` (`0x49b11e`): a custom channel whose name holds a row's
/// pattern is still id 0.
#[test]
fn a_channel_lines_id_is_its_slots_row() {
    let v = Verbs::new(vec![
        custom("WorldDefense2"),
        Some(ChannelSlot::confirmed("General - Elwynn Forest", 1)),
    ]);
    let stamped = |name: &str| {
        let mut e = ChatEvent::text_only(ChatEventKind::Channel, "hi".into());
        e.channel = name.into();
        v.channels.stamp_channel(&mut e);
        (e.channel_number, e.zone_channel_id)
    };
    assert_eq!(stamped("WorldDefense2"), (1, 0));
    assert_eq!(stamped("General - Elwynn Forest"), (2, 1));
}
