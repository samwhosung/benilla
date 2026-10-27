//! The name caches' getters read the caches as they stand where they run, through the lent host:
//! `UnitName` names a unit through `GetUnitName` (`0x609210`), `UnitPVPName` builds on it
//! (`0x5172b0` → `0x609370`), and `UnitCreatureType` (`0x51a280`), `UnitClassification`
//! (`0x516d90`) and `UnitLevel`'s world-boss leg read the unit's cached creature record
//! (`0x605570`, `0x605620`). A name answer's events fire at its packet's place, as the callback
//! runs inside the handler that filed the record (`0x5fb840`, `0x604660`).

use benilla_protocol::SessionEvent;
use benilla_ui::script::live::ask;
use benilla_ui::script::PlayerNameQuery;
use bevy::prelude::*;

use super::unit_view_tests::{client, frame, health, values};
use crate::names::{CreatureRecord, NameCache};

/// The target [`client`] seats: a creature.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// The template entry the wolf's descriptor names.
const WOLF_ENTRY: u32 = 299;
/// `OBJECT_FIELD_ENTRY`.
const OBJECT_ENTRY: u16 = 3;
/// `UNIT_FIELD_PETNUMBER`.
const UNIT_PETNUMBER: u16 = 139;
/// A player the name cache holds, and one it lacks.
const BOB: u64 = 0x61;
const STRANGER: u64 = 0x62;

/// The wolf's template, an elite beast.
fn wolf_record() -> CreatureRecord {
    CreatureRecord {
        name: "Wolf".into(),
        subname: None,
        creature_type: 1,
        pet_family: 0,
        rank: 1,
        type_flags: 0,
        pet_spell_list: 0,
        civilian: false,
        racial_leader: false,
        display_id: 0,
    }
}

/// The wolf's answer packet, as the server sends it.
fn wolf_named() -> SessionEvent {
    SessionEvent::CreatureName {
        entry: WOLF_ENTRY,
        name: Some("Wolf".into()),
        subname: None,
        creature_type: Some(1),
        pet_family: 0,
        rank: 1,
        type_flags: 0,
        pet_spell_list: 0,
        display_id: 0,
        civilian: false,
        racial_leader: false,
    }
}

/// The client with the wolf's descriptor naming its template, the type words loaded and a spy on
/// the name events that reads the getters as each one fires.
fn named_client() -> App {
    let mut app = client();
    let world = app.world_mut();
    let entity = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    world
        .get_mut::<crate::net::ObjectStore>(entity)
        .expect("a store")
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(
            OBJECT_ENTRY,
            WOLF_ENTRY,
        )]));
    world.insert_resource(crate::target::CreatureTypes(
        benilla_formats::CreatureTypeTable::from_rows([(1, "Beast", 0), (7, "Humanoid", 0)]),
    ));
    app.world_mut()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"UNIT_NAME_UPDATE", "UNIT_CLASSIFICATION_CHANGED"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. arg1 .. ":" .. tostring(UnitName(arg1))
                    .. ":" .. tostring(UnitClassification(arg1)))
            end)
            "#,
        )
        .unwrap();
    app
}

/// `chunk` with the host lent, as a listener's call runs.
fn hosted<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").eval(chunk).expect("runs")
    })
}

/// A creature's record that lands after the frame's push is read by the getters as they run: its
/// name, its type word, its rank and the world-boss -1 of its level, where the snapshot, pushed
/// before, still holds none of them.
#[test]
fn the_creature_getters_read_the_record_as_they_run() {
    let mut app = named_client();
    const CHUNK: &str = "return UnitName('target'), UnitPVPName('target'), \
         UnitCreatureType('target'), UnitClassification('target'), UnitLevel('target')";
    type Answer = (Option<String>, Option<String>, Option<String>, String, i64);
    let before: Answer = hosted(&mut app, CHUNK);
    assert_eq!(
        before,
        (
            Some("Unknown Being".into()),
            Some("Unknown Being".into()),
            None,
            "normal".into(),
            0
        ),
        "no record yet: both name the unknown object (`0x609324`), no type, the normal rank"
    );
    // A boss template: its rank 3 hides the level (`0x51807d`).
    let mut record = wolf_record();
    record.rank = 3;
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_creature(WOLF_ENTRY, Some(record));
    let live: Answer = hosted(&mut app, CHUNK);
    assert_eq!(
        live,
        (
            Some("Wolf".into()),
            Some("Wolf".into()),
            Some("Beast".into()),
            "worldboss".into(),
            -1
        )
    );
    // The snapshot, pushed ahead of the record, holds none of it: a VM with no host lent.
    let snapshot: Answer = crate::script_calls::eval_in_vm(app.world(), CHUNK);
    assert_eq!(snapshot.2, None, "the push held no record");
    assert_eq!(snapshot.3, "normal");
}

/// A pet or charm reads rank 0 whatever its template says (`0x605620`), live as in the push.
#[test]
fn a_charmed_creature_reads_rank_zero_live() {
    let mut app = named_client();
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_creature(WOLF_ENTRY, Some(wolf_record()));
    let elite: String = hosted(&mut app, "return UnitClassification('target')");
    assert_eq!(elite, "elite");
    let world = app.world_mut();
    let entity = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    world
        .get_mut::<crate::net::ObjectStore>(entity)
        .expect("a store")
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(
            UNIT_PETNUMBER,
            5,
        )]));
    let charmed: String = hosted(&mut app, "return UnitClassification('target')");
    assert_eq!(charmed, "normal");
}

/// The player-name cache's record is read as the call runs: a name filed between two reads shows
/// in the second, a miss answers its own empty answer, and with no host lent nothing answers.
#[test]
fn a_player_name_reads_the_cache_as_it_runs() {
    let mut app = named_client();
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_player(BOB, "Bob".into(), None);
    let read = |app: &mut App, guid: u64| {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            ask(s.expect("a VM").lua(), &PlayerNameQuery(guid))
        })
    };
    assert_eq!(read(&mut app, BOB), Some(Some("Bob".to_string())));
    assert_eq!(read(&mut app, STRANGER), Some(None), "a miss");
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_player(STRANGER, "Stranger".into(), None);
    assert_eq!(read(&mut app, STRANGER), Some(Some("Stranger".to_string())));
    let unlent = ask(
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .lua(),
        &PlayerNameQuery(BOB),
    );
    assert_eq!(unlent, None, "no host lent, no live read");
}

/// A creature's answer raises `UNIT_NAME_UPDATE` then `UNIT_CLASSIFICATION_CHANGED` for each held
/// unit of the entry (`0x6046d1`, `0x6046dd`) at the answer packet's place, between the packets
/// ahead of it and behind it, and their listeners read the record the handler filed.
#[test]
fn a_creature_answers_its_name_events_at_its_packets_place() {
    let mut app = named_client();
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                wolf_named(),
                values(WOLF, &health(40)),
            ]
        ),
        [
            "UNIT_HEALTH:target:70",
            "UNIT_NAME_UPDATE:target:Wolf:elite",
            "UNIT_CLASSIFICATION_CHANGED:target:Wolf:elite",
            "UNIT_HEALTH:target:40",
        ]
    );
}

/// The civilian flag is the record's, read as the call runs: `UnitIsCivilian` and the `UnitPVPName`
/// prefix (`0x612550`, `0x609370`) see a template that landed after the frame's push, where the
/// snapshot holds a record-less unit. The PvP flag, the level and the reaction, an Orc wolf's
/// toward a Human, are read off the descriptors then too.
#[test]
fn the_civilian_flag_is_read_off_the_record_as_the_call_runs() {
    let mut app = named_client();
    let world = app.world_mut();
    let entity = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    use crate::target::relations::fixture::{factions, HUMAN, ORC};
    /// `UNIT_FIELD_FACTIONTEMPLATE`.
    const TEMPLATE: u16 = 35;
    world.insert_resource(factions());
    // `UNIT_FIELD_FLAGS` PvP, a level far below the player's, and an Orc's template.
    world
        .get_mut::<crate::net::ObjectStore>(entity)
        .expect("a store")
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x1000),
            (benilla_protocol::field::FIELD_UNIT_LEVEL, 1),
            (TEMPLATE, ORC),
        ]));
    // Our own level, which the grey test reads off our descriptor as the call runs.
    let me = world
        .query_filtered::<Entity, With<crate::net::SelfPlayer>>()
        .single(world)
        .expect("our player");
    world
        .get_mut::<crate::net::ObjectStore>(me)
        .expect("our store")
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_UNIT_LEVEL, 60),
            (TEMPLATE, HUMAN),
        ]));
    {
        let mut script = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        script.run(r#"PVP_RANK_CIVILIAN = "Civilian""#).unwrap();
        // The push, as a record-less unit hostile to us holds it.
        script.set_unit(
            "target",
            Some(benilla_ui::script::UnitState {
                exists: true,
                has_object: true,
                is_connected: true,
                guid: WOLF,
                name: Some("Wolf".into()),
                reaction: 1,
                pvp: true,
                level: 1,
                ..Default::default()
            }),
        );
    }
    const CHUNK: &str = "return UnitIsCivilian('target'), UnitPVPName('target')";
    let before: (Option<i64>, Option<String>) = hosted(&mut app, CHUNK);
    assert_eq!(
        before,
        (None, Some("Unknown Being".into())),
        "no record yet: the unknown object (`0x609324`)"
    );
    let mut record = wolf_record();
    record.civilian = true;
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_creature(WOLF_ENTRY, Some(record));
    let live: (Option<i64>, Option<String>) = hosted(&mut app, CHUNK);
    assert_eq!(live, (Some(1), Some("Civilian Wolf".into())));
    let snapshot: (Option<i64>, Option<String>) =
        crate::script_calls::eval_in_vm(app.world(), CHUNK);
    assert_eq!(snapshot.0, None, "the push held no record");
}
