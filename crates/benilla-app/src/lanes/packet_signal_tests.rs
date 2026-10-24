//! What a packet handler signals inside itself, at the packet's place among the frame's other
//! packets (`0x5384d0`, `0x537aa0`): the pet's spell list and mode, the area spirit healer's time,
//! a game object's page text, and the trainer list's open, each interleaved with a fall's
//! `UNIT_COMBAT` from the chat lane.

use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, fall, frame, values};

const ME: u64 = 0x7;
/// Our pet.
const PET: u64 = 0xF140_0000_0000_0077;
/// Another player's pet.
const STRANGER: u64 = 0xF140_0000_0000_0078;
/// A fall's `UNIT_COMBAT`, as [`client`]'s spy records it.
const FELL: &str = "UNIT_COMBAT:player:100";

/// A spy on argless events, recording each by name beside [`client`]'s.
fn spy(app: &mut App, events: &[&str]) {
    let list: Vec<String> = events.iter().map(|e| format!("{e:?}")).collect();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(&format!(
            r#"
            local f = CreateFrame("Frame")
            for _, e in ipairs({{{}}}) do f:RegisterEvent(e) end
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
            list.join(", ")
        ))
        .unwrap();
}

/// `SMSG_PET_SPELLS` naming `guid`, with no spells.
fn bar_for(guid: u64) -> SessionEvent {
    SessionEvent::PetSpells(Box::new(benilla_protocol::messages::PetSpells {
        pet_guid: guid,
        ..Default::default()
    }))
}

/// `SMSG_PET_MODE` for `guid` with the word `state`.
fn mode(guid: u64, state: u32) -> SessionEvent {
    SessionEvent::PetMode(benilla_protocol::messages::PetMode {
        pet_guid: guid,
        state,
    })
}

/// `SMSG_PET_SPELLS` sorts the pet book on every packet (`0x4bdadb` → `0x4b2fd0(0, 1)`), which
/// signals `SPELLS_CHANGED` (`0x4b302a`), ahead of `SetPetGuid`'s `UNIT_PET` and
/// `PET_BAR_UPDATE` (`0x4bdaf0`): a repeat of the same list signals again, an empty list over an
/// empty book too (`0x4bdacf`), each at its packet's place.
#[test]
fn each_pet_spell_list_signals_spells_changed_at_its_packet() {
    let mut app = client();
    spy(&mut app, &["SPELLS_CHANGED", "UNIT_PET", "PET_BAR_UPDATE"]);
    let seen = frame(&mut app, vec![bar_for(PET), fall(5), bar_for(PET)]);
    assert_eq!(
        seen,
        [
            "SPELLS_CHANGED",
            "UNIT_PET",
            "PET_BAR_UPDATE",
            FELL,
            "SPELLS_CHANGED",
            "PET_BAR_UPDATE",
        ]
    );
    let seen = frame(&mut app, vec![bar_for(0), bar_for(0)]);
    assert_eq!(
        seen,
        [
            "SPELLS_CHANGED",
            "UNIT_PET",
            "PET_BAR_UPDATE",
            "SPELLS_CHANGED",
            "PET_BAR_UPDATE",
        ],
        "the teardown, then an empty list over the empty book"
    );
}

/// `SMSG_PET_MODE` (`0x4bdb10`) stores the word and signals `PET_BAR_UPDATE` (`0x4bdb4f`) for
/// every packet naming the bar's pet (`0x4bdb33`, `0x4bdb3f`), the word it already holds
/// included, at the packet's place; another guid signals nothing.
#[test]
fn each_pet_mode_for_our_pet_signals_the_bar_at_its_packet() {
    let mut app = client();
    frame(&mut app, vec![bar_for(PET)]);
    spy(&mut app, &["PET_BAR_UPDATE"]);
    let seen = frame(
        &mut app,
        vec![
            mode(PET, 0x0101),
            fall(5),
            mode(PET, 0x0101),
            mode(STRANGER, 0x0202),
        ],
    );
    assert_eq!(seen, ["PET_BAR_UPDATE", FELL, "PET_BAR_UPDATE"]);
}

/// `SMSG_AREA_SPIRIT_HEALER_TIME` (`0x492290`): the rescan (`0x492296`), then for the cached
/// healer and a positive time the deadline and `AREA_SPIRIT_HEALER_IN_RANGE` inside the handler
/// (`0x4922cc`-`0x4922d1`), ahead of a later packet's events; a zero time signals nothing
/// (`0x4922b2`).
#[test]
fn the_spirit_healer_time_signals_in_range_at_its_packet() {
    const HEALER: u64 = 0xF130_0000_1868_0001;
    let mut app = client();
    // A ghost beside the healer, which the rescan keeps within 22 yd (`0x492406`).
    frame(
        &mut app,
        vec![
            values(ME, &[(benilla_protocol::field::FIELD_PLAYER_FLAGS, 0x10)]),
            created(HEALER, EntityKind::Unit, &[]),
        ],
    );
    let _ = app
        .world_mut()
        .resource_mut::<crate::ui_dialog_verbs::AreaSpiritHealer>()
        .set_healer(Some(HEALER));
    spy(&mut app, &["AREA_SPIRIT_HEALER_IN_RANGE"]);
    let time = |ms| SessionEvent::AreaSpiritHealerTime { healer: HEALER, ms };
    let seen = frame(&mut app, vec![time(0), fall(5), time(30_000), fall(5)]);
    assert_eq!(seen, [FELL, "AREA_SPIRIT_HEALER_IN_RANGE", FELL]);
}

/// `SMSG_GAMEOBJECT_PAGETEXT` (`0x5f88c0`) opens the reader on the object (`0x5f8915` →
/// `0x4e32e0`), whose begin latches it (`0x4e349a`) and signals `ITEM_TEXT_BEGIN` (`0x4e34a4`)
/// inside the handler, ahead of the packets after it: "The Lay of Ameth'Aran", entry 17188.
#[test]
fn a_game_objects_page_text_begins_at_its_packet() {
    const TABLET: u64 = 0xF110_0043_2400_0001;
    let mut app = client();
    let mut data = [0i32; 24];
    data[7] = 358;
    app.world_mut()
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(17188, 10, "The Lay of Ameth'Aran".into(), &data);
    let tablet = app
        .world_mut()
        .spawn((
            crate::net::Guid(TABLET),
            crate::net::ObjectStore(crate::go_templates::test_typed_fields(17188, 10)),
        ))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(TABLET, tablet);
    spy(&mut app, &["ITEM_TEXT_BEGIN"]);
    let seen = frame(
        &mut app,
        vec![
            fall(5),
            SessionEvent::GameObjectPageText { guid: TABLET },
            fall(5),
        ],
    );
    assert_eq!(seen, [FELL, "ITEM_TEXT_BEGIN", FELL]);
    assert_eq!(
        app.world()
            .resource::<crate::ui_item_text::ItemTextOpen>()
            .object(),
        Some(TABLET)
    );
}

/// `ReadTrainerList` (`0x5dca60`) opens the trainer (`0x5dccb5`) only when the list's guid is the
/// one `TalkToTrainer` asked (`0xc4d788`, `0x5dcc85`) or the open gossip window's (`0xbc3f58`,
/// `0x5dcc97`); any other list returns at `0x5dcd44` with nothing opened.
#[test]
fn a_trainer_list_opens_only_the_trainer_asked_or_the_gossip_npc() {
    const TRAINER: u64 = 0xF130_0000_0391_0001;
    const OTHER: u64 = 0xF130_0000_0392_0002;
    let mut app = client();
    let list = |trainer| SessionEvent::TrainerList {
        trainer,
        trainer_type: 0,
        services: Vec::new(),
        greeting: String::new(),
    };
    let open = |app: &App| {
        app.world()
            .resource::<crate::ui_trainer::TrainerOpen>()
            .trainer
    };
    frame(&mut app, vec![list(TRAINER)]);
    assert_eq!(open(&app), None, "a list nobody asked for");
    crate::ui_trainer::talk_to_trainer(app.world_mut(), TRAINER);
    frame(&mut app, vec![list(TRAINER)]);
    assert_eq!(open(&app), Some(TRAINER), "the trainer asked");
    frame(&mut app, vec![list(OTHER)]);
    assert_eq!(open(&app), Some(TRAINER), "another trainer's late list");
    app.world_mut()
        .resource_mut::<crate::ui_gossip::GossipState>()
        .npc = Some(OTHER);
    frame(&mut app, vec![list(OTHER)]);
    assert_eq!(open(&app), Some(OTHER), "the gossip window's NPC");
}
