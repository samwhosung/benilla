//! The guid-tail channel notices through [`super::feed::feed_chat`] to the stock chat frame: the
//! reference hands every notice's channel name to its composer `0x49a870` (the cached leg
//! `0x49c59f`-`0x49c5b0`, the name-query leg `0x49cfb4`-`0x49cfc1`), which numbers it off the
//! joined channels (`0x49aa1e`-`0x49aa48`) and fills arg7-arg9 from that record (`0x49b10c`-
//! `0x49b12a`). Without that stamp the stock filter (`ChatFrame.lua:1373-1392`) prints nothing.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_protocol::messages::{channel_notice as n, ChannelNoticeTail};
use benilla_ui::script::UiScript;

use super::tests::{chat_vm, lines_in_window, SPY};

const ANN: u64 = 0x11;
const BOB: u64 = 0x12;

/// A world holding everything [`super::feed::feed_chat`] reads, the stock chat stack in its VM,
/// `ChatFrame1` carrying `Mychan`, and our slots `World`, `Trade - City` and `Mychan` (3).
pub(super) struct Feed {
    pub(super) world: World,
    /// The outbound side of `NetCommands`, where the name queries land.
    sent: crossbeam_channel::Receiver<crate::net::ClientCommand>,
}

impl Feed {
    pub(super) fn new() -> Self {
        let s = chat_vm();
        s.run(SPY).unwrap();
        s.run(
            r#"
            for _, e in { "CHAT_MSG_CHANNEL_JOIN", "CHAT_MSG_CHANNEL_LEAVE",
                          "CHAT_MSG_CHANNEL_NOTICE", "CHAT_MSG_CHANNEL_NOTICE_USER" } do
                BenillaChatSpy:RegisterEvent(e)
            end
            -- The text the stock handler hands the window, links and all.
            local add = ChatFrame1.AddMessage
            ChatFrame1.AddMessage = function(self, text, r, g, b, id)
                LastAdded = text
                return add(self, text, r, g, b, id)
            end
            -- `/join Mychan`'s window half (`ChatFrame.lua`'s `ChatFrame_AddChannel`).
            ChatFrame_AddChannel(ChatFrame1, "Mychan")
        "#,
        )
        .unwrap();

        let mut channels = super::edit::ChannelState::default();
        for name in ["World", "Trade - City", "Mychan"] {
            channels.claim_slot(name, channels.channels.zone_channel_id(name));
        }
        let (tx, sent) = crossbeam_channel::unbounded();

        let mut world = World::new();
        world.insert_non_send_resource(s);
        world.insert_resource(super::ChatLog::default());
        world.insert_resource(super::frames::ChatWindows::default());
        world.insert_resource(channels);
        world.insert_resource(crate::names::NameCache::default());
        world.insert_resource(crate::chat_bubble::BubbleQueue::default());
        world.insert_resource(crate::chat_bubble::BubbleConfig::default());
        world.insert_resource(crate::creature_anim::GestureQueue::default());
        world.insert_resource(crate::net::NetCommands(tx));
        world.insert_resource(crate::net::SelfGuid::default());
        world.insert_resource(crate::net::GuidIndex::default());
        world.insert_resource(crate::world_state::WorldStates::default());
        world.insert_resource(super::language::ChatLanguages::default());
        world.insert_resource(crate::items::Items::default());
        world.insert_resource(crate::go_templates::GameObjectTemplates::default());
        world.insert_resource(crate::text_filter::TextFilter::default());
        world.insert_resource(crate::text_filter::TextFilterSwitches::default());
        Feed { world, sent }
    }

    fn name(&mut self, guid: u64, name: &str) {
        self.world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(guid, name.into(), None);
    }

    fn push(&mut self, notice: u8, channel: &str, tail: ChannelNoticeTail) {
        self.world
            .resource_mut::<super::ChatLog>()
            .push_channel_notice(notice, channel.into(), &tail);
    }

    /// One frame of the drain.
    pub(super) fn drain(&mut self) {
        self.world.run_system_once(super::feed::feed_chat).unwrap();
    }

    pub(super) fn script(&self) -> &UiScript {
        self.world.non_send_resource::<UiScript>()
    }

    fn lines(&self) -> i64 {
        lines_in_window(self.script())
    }

    fn last_added(&self) -> String {
        self.script()
            .eval::<String>("return LastAdded or ''")
            .unwrap()
    }

    fn spy(&self) -> (String, String) {
        let s = self.script();
        (
            s.eval::<String>("return SpyEvent").unwrap(),
            s.eval::<String>("return SpyLine").unwrap(),
        )
    }
}

/// Another member's join, name cached: arg4 `"3. Mychan"`, arg7 0 (a custom channel has no
/// ChannelID), arg8 3 and arg9 `"Mychan"`, and the stock frame prints the line.
#[test]
fn a_members_join_reaches_the_stock_frame_numbered() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    f.name(ANN, "Ann");
    let before = f.lines();
    f.push(n::JOINED, "Mychan", ChannelNoticeTail::Guid(ANN));
    f.drain();

    assert_eq!(f.lines(), before + 1, "the stock filter found the channel");
    // `CHAT_CHANNEL_JOIN_GET` behind the `[arg4] ` prefix (`ChatFrame.lua:1451`, `:1463-1465`).
    assert_eq!(
        f.last_added(),
        "[3. Mychan] |Hplayer:Ann|h[Ann]|h joined channel."
    );
    assert_eq!(
        f.spy(),
        (
            "CHAT_MSG_CHANNEL_JOIN".to_string(),
            "|Ann||3. Mychan|||0|3|Mychan|0".to_string()
        )
    );
    assert!(f.script().errors().is_empty(), "{:?}", f.script().errors());
}

/// The join notice's second word is the slot's instance (`0x49bb20`, `+0x98`), which every event
/// the channel's record stamps reads as arg10 (`0x49b115`): our own join, another member's, and
/// a reused slot's, since no allocator writes it (`0x49ba01`).
#[test]
fn a_joins_instance_number_is_arg10_of_the_slots_events() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    f.name(ANN, "Ann");
    f.push(
        n::YOU_JOINED,
        "Mychan",
        ChannelNoticeTail::YouJoined {
            flags: 0,
            instance: 7,
        },
    );
    f.drain();
    assert_eq!(f.spy().1, "YOU_JOINED|||3. Mychan|||0|3|Mychan|7");
    f.push(n::JOINED, "Mychan", ChannelNoticeTail::Guid(ANN));
    f.drain();
    assert_eq!(f.spy().1, "|Ann||3. Mychan|||0|3|Mychan|7");

    // Freed and claimed again by another name, the slot keeps its instance until a join stores one.
    let mut channels = f.world.resource_mut::<super::edit::ChannelState>();
    channels.free_slot("Mychan");
    channels.claim_slot("Otherchan", 0);
    assert_eq!(channels.instances[2], 7);
    let mut line =
        super::event::ChatEvent::text_only(super::event::ChatEventKind::Channel, "".into());
    line.channel = "Otherchan".into();
    channels.stamp_channel(&mut line);
    assert_eq!((line.channel_number, line.instance), (3, 7));
    let mut missing =
        super::event::ChatEvent::text_only(super::event::ChatEventKind::Channel, "".into());
    missing.channel = "Nowhere".into();
    channels.stamp_channel(&mut missing);
    assert_eq!(missing.instance, 0, "no record, no instance");
}

/// A join whose name is not cached waits on the name query, then prints stamped, as the
/// reference's queued node carries the channel name to the callback (`0x49cfb4`-`0x49cfc1`).
#[test]
fn a_join_resolved_after_its_name_query_is_stamped_too() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    let before = f.lines();
    f.push(n::LEFT, "Mychan", ChannelNoticeTail::Guid(ANN));
    f.drain();
    assert_eq!(f.lines(), before, "held for the name");
    assert!(
        matches!(
            f.sent.try_recv(),
            Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
        ),
        "and the name was asked for"
    );

    f.name(ANN, "Ann");
    f.drain();
    assert_eq!(
        f.spy(),
        (
            "CHAT_MSG_CHANNEL_LEAVE".to_string(),
            "|Ann||3. Mychan|||0|3|Mychan|0".to_string()
        )
    );
    assert_eq!(f.lines(), before + 1);
    assert_eq!(
        f.last_added(),
        "[3. Mychan] |Hplayer:Ann|h[Ann]|h left channel."
    );
}

/// The owner change and a kick, `CHANNEL_NOTICE_USER` lines of one and two names.
#[test]
fn owner_change_and_kick_notices_print() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    f.name(ANN, "Ann");
    f.name(BOB, "Bob");

    let before = f.lines();
    f.push(n::OWNER_CHANGED, "Mychan", ChannelNoticeTail::Actor(BOB));
    f.drain();
    assert_eq!(
        f.spy(),
        (
            "CHAT_MSG_CHANNEL_NOTICE_USER".to_string(),
            "OWNER_CHANGED|Bob||3. Mychan|||0|3|Mychan|0".to_string()
        )
    );
    assert_eq!(f.lines(), before + 1);
    assert_eq!(f.last_added(), "[3. Mychan] Owner changed to Bob.");

    f.push(
        n::PLAYER_KICKED,
        "Mychan",
        ChannelNoticeTail::Actors {
            target: ANN,
            source: BOB,
        },
    );
    f.drain();
    assert_eq!(f.lines(), before + 2);
    assert_eq!(f.last_added(), "[3. Mychan] Player Ann kicked by Bob.");
    assert!(f.script().errors().is_empty(), "{:?}", f.script().errors());
}

/// A member's flags changing, name cached (`0x49e934`): bit 2 gained is `SET_MODERATOR`, lost
/// `UNSET_MODERATOR`, then bit 4 `SET_VOICE` / `UNSET_VOICE`, each a `CHANNEL_NOTICE_USER`
/// (`0x49e8a0`) naming the member; a bit that did not change prints nothing.
#[test]
fn a_mode_change_prints_its_moderator_and_voice_lines() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    f.name(ANN, "Ann");
    let mode = |old_flags, new_flags| ChannelNoticeTail::ModeChange {
        guid: ANN,
        old_flags,
        new_flags,
    };
    let before = f.lines();

    f.push(n::MODE_CHANGE, "Mychan", mode(0x00, 0x02));
    f.drain();
    assert_eq!(
        f.spy(),
        (
            "CHAT_MSG_CHANNEL_NOTICE_USER".to_string(),
            "SET_MODERATOR|Ann||3. Mychan|||0|3|Mychan|0".to_string()
        )
    );
    assert_eq!(
        f.last_added(),
        "[3. Mychan] Moderation privileges given to Ann."
    );

    f.push(n::MODE_CHANGE, "Mychan", mode(0x02, 0x00));
    f.drain();
    assert_eq!(f.spy().1, "UNSET_MODERATOR|Ann||3. Mychan|||0|3|Mychan|0");
    assert_eq!(
        f.last_added(),
        "[3. Mychan] Moderation privileges removed from Ann."
    );

    f.push(n::MODE_CHANGE, "Mychan", mode(0x00, 0x04));
    f.drain();
    assert_eq!(f.spy().1, "SET_VOICE|Ann||3. Mychan|||0|3|Mychan|0");
    assert_eq!(f.last_added(), "[3. Mychan] Voice given to Ann.");

    f.push(n::MODE_CHANGE, "Mychan", mode(0x04, 0x00));
    f.drain();
    assert_eq!(f.spy().1, "UNSET_VOICE|Ann||3. Mychan|||0|3|Mychan|0");
    assert_eq!(f.last_added(), "[3. Mychan] Ann muted.");
    assert_eq!(f.lines(), before + 4);

    // Both bits at once print the moderator line, then the voice line; the owner bit (1) and a
    // bit that stays set print nothing.
    f.push(n::MODE_CHANGE, "Mychan", mode(0x01, 0x06));
    f.drain();
    assert_eq!(f.lines(), before + 6);
    assert_eq!(f.spy().1, "SET_VOICE|Ann||3. Mychan|||0|3|Mychan|0");
    f.push(n::MODE_CHANGE, "Mychan", mode(0x03, 0x01));
    f.drain();
    assert_eq!(f.lines(), before + 7, "only the moderator bit was lost");
    f.push(n::MODE_CHANGE, "Mychan", mode(0x06, 0x07));
    f.drain();
    assert_eq!(f.lines(), before + 7, "the owner bit alone prints nothing");
    assert!(f.script().errors().is_empty(), "{:?}", f.script().errors());
}

/// A name the cache lacks shows nothing: `0x49e93d` returns on the miss and the callback `0x49e934`
/// registers holds no line. The miss still asks for the name.
#[test]
fn a_mode_change_for_an_unknown_name_prints_nothing_and_asks() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    let before = f.lines();
    f.push(
        n::MODE_CHANGE,
        "Mychan",
        ChannelNoticeTail::ModeChange {
            guid: ANN,
            old_flags: 0,
            new_flags: 2,
        },
    );
    f.drain();
    assert!(
        matches!(
            f.sent.try_recv(),
            Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
        ),
        "the name was asked for"
    );
    f.name(ANN, "Ann");
    f.drain();
    assert_eq!(f.lines(), before, "and the answer shows no line");
}

/// An invite names a channel we are not in, so it stays unstamped (the miss leg `0x49aa86`,
/// `0x49b12f`) and prints only through the stock filter's `INVITE` exemption (`ChatFrame.lua:1374`).
#[test]
fn an_invite_to_a_channel_we_are_not_in_still_prints() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    f.name(ANN, "Ann");
    let before = f.lines();
    f.push(n::INVITE, "Otherchan", ChannelNoticeTail::Actor(ANN));
    f.drain();
    assert_eq!(
        f.spy().1,
        "INVITE|Ann||Otherchan|||0|0||0",
        "no record, so the bare name and zeroes"
    );
    assert_eq!(f.lines(), before + 1);
    assert_eq!(
        f.last_added(),
        "Ann has invited you to join the channel 'Otherchan'."
    );
}

/// Our own join, a notice with no guid tail, goes the ready-event path and still prints once.
#[test]
fn our_own_join_notice_is_unchanged() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    let before = f.lines();
    f.push(
        n::YOU_JOINED,
        "Mychan",
        ChannelNoticeTail::YouJoined {
            flags: 0,
            instance: 0,
        },
    );
    f.drain();
    assert_eq!(
        f.spy(),
        (
            "CHAT_MSG_CHANNEL_NOTICE".to_string(),
            "YOU_JOINED|||3. Mychan|||0|3|Mychan|0".to_string()
        )
    );
    assert_eq!(f.lines(), before + 1);
    assert_eq!(f.last_added(), "Joined Channel: [3. Mychan]");
}

/// A chat line and a channel notice look their names up with the callback `0x49ccc0` at their
/// packet (`0x49d77f`, `0x49c50d`); the drains they wait through read. A refused name is asked
/// by no drain after it, only by the next packet that names its player.
#[test]
fn a_refused_chat_name_is_asked_by_the_next_packet_never_by_a_drain() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut f = Feed::new();
    let packets = |f: &mut Feed| {
        f.world.resource_mut::<super::ChatLog>().push_wire(
            benilla_protocol::messages::ChatMessage {
                chat_type: 0x00,
                language: 0,
                sender_guid: ANN,
                target_guid: 0,
                sender_name: None,
                channel: None,
                text: "hello".into(),
                chat_tag: 0,
            },
        );
        f.push(n::JOINED, "Mychan", ChannelNoticeTail::Guid(BOB));
        f.drain();
    };
    let asked = |f: &Feed| crate::query_cache::queries(&f.sent).len();
    packets(&mut f);
    assert_eq!(asked(&f), 2, "the speaker and the joiner");
    f.name(ANN, "");
    f.name(BOB, "");
    for _ in 0..600 {
        f.drain();
    }
    assert_eq!(asked(&f), 0, "no drain asks a refusal");
    packets(&mut f);
    assert_eq!(asked(&f), 2, "the next packets ask again");
}
