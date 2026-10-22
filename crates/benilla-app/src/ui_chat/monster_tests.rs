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
/// speaker's guid, read first for a say or a yell (`0x49d5dc`), is never passed on. With no
/// addressee `$n` fails, and `0x49dacc`-`0x49dad0` drop the line.
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
