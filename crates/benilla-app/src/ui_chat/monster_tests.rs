//! The `$`-macro subject of a monster and a battleground line (`0x49d560`, the expander
//! `0x506f70` and its tokens `0x5070a0`).

use benilla_protocol::messages::{
    ChatMessage, CHAT_MSG_BG_SYSTEM_NEUTRAL, CHAT_MSG_MONSTER_SAY, CHAT_MSG_MONSTER_YELL,
};
use bevy::prelude::*;

use crate::net::{GuidIndex, ObjectStore};

/// A creature that speaks, Puppet of Helcular (2946), its record cached.
const SPEAKER: u64 = 0xF130_000B_8200_0001;
/// A hunter's pet, pet number 7, streamed or not.
const PET: u64 = 0xF140_0000_0700_0001;
/// A player the name cache knows.
const FRIEND: u64 = 0x52;

/// The chat lane and its reads, with a VM whose spy records each `CHAT_MSG_*` and its text.
fn client() -> App {
    let mut app = App::new();
    app.add_plugins(crate::ui_unit::UiUnitPlugin);
    super::add_chat_lane(&mut app);
    let world = app.world_mut();
    let (tx, _rx) = crossbeam_channel::unbounded();
    world.insert_resource(crate::net::NetCommands(tx));
    super::insert_drain_reads(world);
    let mut names = world.resource_mut::<crate::names::NameCache>();
    names.insert_player(FRIEND, "Friend".into(), None);
    names.insert_pet(7, "Fang".into());
    names.insert_creature(
        2946,
        Some(crate::names::CreatureRecord {
            name: "Puppet of Helcular".into(),
            subname: None,
            creature_type: 6,
            pet_family: 0,
            rank: 0,
            type_flags: 0,
            pet_spell_list: 0,
            civilian: false,
            racial_leader: false,
            display_id: 0,
        }),
    );
    let s = benilla_ui::script::UiScript::new().unwrap();
    s.run(
        r#"
        UNKNOWNOBJECT = "Unknown"
        Heard = {}
        local f = CreateFrame("Frame")
        f:RegisterAllEvents()
        f:SetScript("OnEvent", function()
            if string.find(event, "^CHAT_MSG_") then
                table.insert(Heard, event .. ":" .. tostring(arg1))
            end
        end)
    "#,
    )
    .unwrap();
    world.insert_non_send_resource(s);
    app
}

/// Stream [`PET`]: a creature with pet number 7 whose `UNIT_FIELD_BYTES_0` class byte is 1, as
/// vmangos gives every hunter beast (`Pet.cpp:1256`).
fn stream_pet(app: &mut App) {
    let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (2, 0x09),
        // `UNIT_FIELD_BYTES_0`: race 0, class 1, gender 0.
        (36, 0x0000_0100),
        // `UNIT_FIELD_PETNUMBER`.
        (139, 7),
    ]));
    let e = app.world_mut().spawn(store).id();
    app.world_mut().resource_mut::<GuidIndex>().0.insert(PET, e);
}

/// `line` through the feed in one frame: what the interface heard.
fn heard(app: &mut App, line: ChatMessage) -> Vec<String> {
    app.world().resource::<super::ChatLog>().push_wire(line);
    crate::lanes::fire_in_order(app.world_mut());
    let s = app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>();
    let out = s.eval::<Vec<String>>("return Heard").unwrap();
    s.run("Heard = {}").unwrap();
    out
}

fn monster(chat_type: u8, target_guid: u64, text: &str) -> ChatMessage {
    ChatMessage {
        chat_type,
        language: 0,
        sender_guid: SPEAKER,
        target_guid,
        sender_name: Some("Puppet of Helcular".into()),
        channel: None,
        text: text.into(),
        chat_tag: 0,
    }
}

/// The monster arm (`0x49da3d`) expands against the addressee alone (`0x49daab`-`0x49dabd`); the
/// speaker's guid, read first for a say or a yell (`0x49d5dc`), never reaches the expander. With
/// no addressee `$n` fails, and `0x49dacc`-`0x49dad0` drop the line.
#[test]
fn an_unaddressed_monster_line_that_names_its_subject_is_dropped() {
    let mut app = client();
    for chat_type in [CHAT_MSG_MONSTER_SAY, CHAT_MSG_MONSTER_YELL] {
        assert!(
            heard(
                &mut app,
                monster(chat_type, 0, "Die in the name of the Dark Lady, $n.")
            )
            .is_empty(),
            "type {chat_type:#04x}"
        );
    }
    assert_eq!(
        heard(&mut app, monster(CHAT_MSG_MONSTER_SAY, FRIEND, "Die, $n.")),
        ["CHAT_MSG_MONSTER_SAY:Die, Friend."],
        "an addressee names it"
    );
}

/// A battleground system line carries one guid (`0x49d961`, read at `0x49d967`) and expands
/// against it (`0x49d9b2`-`0x49d9c4`).
#[test]
fn a_battleground_line_expands_against_its_one_guid() {
    let mut app = client();
    let line = ChatMessage {
        chat_type: CHAT_MSG_BG_SYSTEM_NEUTRAL,
        sender_guid: FRIEND,
        target_guid: 0,
        sender_name: None,
        ..monster(CHAT_MSG_BG_SYSTEM_NEUTRAL, 0, "$n has taken the flag!")
    };
    assert_eq!(
        heard(&mut app, line),
        ["CHAT_MSG_BG_SYSTEM_NEUTRAL:Friend has taken the flag!"]
    );
}

/// `$R` and `$C` read a race or class only for a unit with the player type bit (`0x50715d`-
/// `0x507169`, `0x5071ea`-`0x5071f5`); any other unit prints its own name (`0x50716b`,
/// `0x5071f7`), straight to the tail, so `$r` and `$c` do not lower-case it (`0x5071a3` is on the
/// table arm only).
#[test]
fn race_and_class_of_a_creature_addressee_are_its_name() {
    let mut app = client();
    stream_pet(&mut app);
    assert_eq!(
        heard(
            &mut app,
            monster(CHAT_MSG_MONSTER_YELL, PET, "Me no run from $C like you!")
        ),
        ["CHAT_MSG_MONSTER_YELL:Me no run from Fang like you!"]
    );
    assert_eq!(
        heard(
            &mut app,
            monster(CHAT_MSG_MONSTER_SAY, PET, "The only good $r is a dead $c!")
        ),
        ["CHAT_MSG_MONSTER_SAY:The only good Fang is a dead Fang!"]
    );
}

/// With no unit the expander's subject is the player-name cache's record alone (`0x506fcf`
/// `mov ecx,0xc0e228`, `0x506fd4`): a creature not streamed has none, so its line fails at its
/// packet and waits on the name query (`0x49dad6`-`0x49daea`) instead of showing.
#[test]
fn a_creature_addressee_not_streamed_is_no_subject() {
    let mut app = client();
    assert!(heard(&mut app, monster(CHAT_MSG_MONSTER_SAY, PET, "Good $n.")).is_empty());
}

/// One frame of the chat lane with nothing new: what the interface heard.
fn idle(app: &mut App) -> Vec<String> {
    crate::lanes::fire_in_order(app.world_mut());
    let s = app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>();
    let out = s.eval::<Vec<String>>("return Heard").unwrap();
    s.run("Heard = {}").unwrap();
    out
}

/// A name answer landing in a packet, as the name handler writes it.
fn named(app: &mut App, guid: u64, name: &str) {
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(guid, name.into(), None);
    app.world()
        .resource::<super::ChatLog>()
        .answer_landed(crate::ui_chat::Answer::Player(guid));
}

/// A line whose subject's name the cache lacks waits in the name cache's callback list
/// (`0x49daea` → `0x55f080`, `0x49ccc0`) until the name answers, however long that takes, and
/// shows nothing before it: never its raw text.
#[test]
fn a_monster_line_holds_for_its_subjects_name_and_never_shows_raw() {
    const LATE: u64 = 0x53;
    let mut app = client();
    let mut heard_any = heard(&mut app, monster(CHAT_MSG_MONSTER_SAY, LATE, "Hail, $n."));
    for _ in 0..300 {
        heard_any.extend(idle(&mut app));
    }
    assert!(heard_any.is_empty(), "{heard_any:?}");
    named(&mut app, LATE, "Latecomer");
    assert_eq!(idle(&mut app), ["CHAT_MSG_MONSTER_SAY:Hail, Latecomer."]);
}

/// A creature not streamed is never named by the player-name cache, so its line never shows.
#[test]
fn a_line_for_an_unseen_creature_never_shows() {
    let mut app = client();
    let mut heard_any = heard(&mut app, monster(CHAT_MSG_MONSTER_SAY, PET, "Good $n."));
    for _ in 0..300 {
        heard_any.extend(idle(&mut app));
    }
    assert!(heard_any.is_empty(), "{heard_any:?}");
}

/// At most [`super::feed::MAX_HELD_SUBJECT_LINES`] lines wait for a subject's name; past that
/// the oldest is dropped.
#[test]
fn the_lines_held_for_a_subject_are_capped_oldest_first() {
    const FIRST: u64 = 0x1000;
    let max = super::feed::MAX_HELD_SUBJECT_LINES as u64;
    let mut app = client();
    for i in 0..=max {
        assert!(heard(
            &mut app,
            monster(CHAT_MSG_MONSTER_SAY, FIRST + i, &format!("{i} $n"))
        )
        .is_empty());
    }
    for i in 0..=max {
        named(&mut app, FIRST + i, "X");
    }
    let shown = idle(&mut app);
    assert_eq!(shown.len() as u64, max);
    assert_eq!(
        shown[0], "CHAT_MSG_MONSTER_SAY:1 X",
        "the first held is dropped"
    );
}

/// A held unit is named by `GetUnitName` (`0x50710b` → `0x507124`, `0x609210`), whose miss is
/// `UNKNOWNOBJECT`: the expansion never fails on it, so its line shows at once, for a pet whose
/// name has not landed and for a player whose name is pending alike.
#[test]
fn a_held_subject_whose_name_is_missing_expands_as_unknown() {
    const NAMELESS_PET: u64 = 0xF140_0000_0800_0001;
    const STRANGER: u64 = 0x60;
    let mut app = client();
    for (guid, fields) in [
        (NAMELESS_PET, vec![(2, 0x09), (139, 8)]),
        (STRANGER, vec![(2, 0x19)]),
    ] {
        let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&fields));
        let e = app.world_mut().spawn(store).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
    }
    assert_eq!(
        heard(
            &mut app,
            monster(CHAT_MSG_MONSTER_SAY, NAMELESS_PET, "Good $n.")
        ),
        ["CHAT_MSG_MONSTER_SAY:Good Unknown."]
    );
    assert_eq!(
        heard(
            &mut app,
            monster(CHAT_MSG_MONSTER_SAY, STRANGER, "Hail, $n.")
        ),
        ["CHAT_MSG_MONSTER_SAY:Hail, Unknown."]
    );
}

/// The display latch queues a monster line with the text its packet expanded (`[ebp-0xa0c]` →
/// `0x49db8c`): the release shows the name the subject had there, not the one it has now.
#[test]
fn a_latched_line_keeps_the_text_its_packet_expanded() {
    let mut app = client();
    app.world().resource::<super::ChatLog>().latch();
    assert!(heard(&mut app, monster(CHAT_MSG_MONSTER_SAY, FRIEND, "Die, $n.")).is_empty());
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(FRIEND, "Stranger".into(), None);
    app.world_mut()
        .run_system_cached(super::release_display)
        .unwrap();
    crate::lanes::fire_in_order(app.world_mut());
    let s = app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>();
    assert_eq!(
        s.eval::<Vec<String>>("return Heard").unwrap(),
        ["CHAT_MSG_MONSTER_SAY:Die, Friend."]
    );
}

/// A player's yell gestures at its packet, ahead of the display latch (`0x49d7d0`-`0x49d8ae`,
/// before `0x49db5c`), and again at the latch's drain (`0x49d3bb`).
#[test]
fn a_latched_yell_gestures_at_its_packet_and_at_the_release() {
    use crate::creature_anim::{Gesture, GestureQueue};
    let mut app = client();
    app.world().resource::<super::ChatLog>().latch();
    let yell = ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_YELL,
        sender_guid: FRIEND,
        sender_name: None,
        ..monster(CHAT_MSG_MONSTER_YELL, 0, "For the Alliance")
    };
    assert!(heard(&mut app, yell).is_empty(), "held");
    assert_eq!(
        app.world().resource::<GestureQueue>().take_for_test(),
        [(FRIEND, Gesture::Shout)],
        "at the packet"
    );
    app.world_mut()
        .run_system_cached(super::release_display)
        .unwrap();
    crate::lanes::fire_in_order(app.world_mut());
    assert_eq!(
        app.world().resource::<GestureQueue>().take_for_test(),
        [(FRIEND, Gesture::Shout)],
        "at the release"
    );
}
