//! What a packet handler signals inside itself, at the packet's place among the frame's other
//! packets (`0x5384d0`, `0x537aa0`): the pet's spell list and mode, the area spirit healer's time,
//! a game object's page text, the trainer list's open and our control flag, each interleaved with a
//! fall's `UNIT_COMBAT` from the chat lane.

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
    spy(
        &mut app,
        &[
            "AREA_SPIRIT_HEALER_IN_RANGE",
            "AREA_SPIRIT_HEALER_OUT_OF_RANGE",
        ],
    );
    let time = |ms| SessionEvent::AreaSpiritHealerTime { healer: HEALER, ms };
    let seen = frame(&mut app, vec![time(0), fall(5), time(30_000), fall(5)]);
    assert_eq!(seen, [FELL, "AREA_SPIRIT_HEALER_IN_RANGE", FELL]);
    // The time is signed (`0x4922b0 test esi,esi; jle`): a negative one arms nothing.
    assert!(frame(&mut app, vec![time(0x8000_0000)]).is_empty());

    // The healer gone, the rescan drops it with the wave pending (`0x4921fc`):
    // `AREA_SPIRIT_HEALER_OUT_OF_RANGE` inside the handler, at the packet's place.
    let world = app.world_mut();
    let healer = world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .remove(&HEALER)
        .expect("the healer's entity");
    world.despawn(healer);
    let seen = frame(&mut app, vec![fall(5), time(30_000), fall(5)]);
    assert_eq!(seen, [FELL, "AREA_SPIRIT_HEALER_OUT_OF_RANGE", FELL]);
}

/// The rescan's sends are the handler's own (`0x4921c0`): with no VM to fire the notice lane, and
/// the lane dropped, the dropped healer's `CMSG_CANCEL_AURA` still goes out.
#[test]
fn the_spirit_healer_rescan_sends_with_no_vm() {
    const HEALER: u64 = 0xF130_0000_1868_0001;
    let mut app = client();
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
    let time = SessionEvent::AreaSpiritHealerTime {
        healer: HEALER,
        ms: 30_000,
    };
    frame(&mut app, vec![time.clone()]);
    let (tx, rx) = crossbeam_channel::unbounded();
    let world = app.world_mut();
    world.insert_resource(crate::net::NetCommands(tx));
    world.remove_non_send_resource::<UiScript>();
    let healer = world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .remove(&HEALER)
        .expect("the healer's entity");
    world.despawn(healer);
    crate::net::handlers::dispatch(world, vec![time]);
    world
        .resource_mut::<crate::field_watch::NoticeLane>()
        .clear();
    let sent: Vec<_> = rx.try_iter().collect();
    assert!(
        sent.iter().any(|c| matches!(
            c,
            crate::net::ClientCommand::CancelAura {
                spell_id: crate::ui_dialog_verbs::AREA_SPIRIT_HEALER_AURA
            }
        )),
        "the cancel goes out at the packet: {sent:?}"
    );
}

/// "The Lay of Ameth'Aran", entry 17188, a GOOBER whose page is 358, streamed as [`TABLET`]; with
/// `cached`, its first page already in the page cache (`0xc0e174`).
fn tablet(app: &mut App, cached: bool) {
    let mut data = [0i32; 24];
    data[7] = 358;
    let world = app.world_mut();
    world
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(17188, 10, "The Lay of Ameth'Aran".into(), &data);
    if cached {
        world
            .resource_mut::<crate::ui_item_text::PageTexts>()
            .insert(358, "Page one.".into(), 0);
    }
    let tablet = world
        .spawn((
            crate::net::Guid(TABLET),
            crate::net::ObjectStore(crate::go_templates::test_typed_fields(17188, 10)),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(TABLET, tablet);
}

/// A readable tablet.
const TABLET: u64 = 0xF110_0043_2400_0001;

/// `SMSG_GAMEOBJECT_PAGETEXT` (`0x5f88c0`) opens the reader on the object (`0x5f8915` →
/// `0x4e32e0`); with its first page cached (`0x4e3465`, `0x4e3489`) the begin latches it
/// (`0x4e349a`) and signals `ITEM_TEXT_BEGIN` (`0x4e34a4`) inside the handler, ahead of the packets
/// after it.
#[test]
fn a_game_objects_page_text_begins_at_its_packet() {
    let mut app = client();
    tablet(&mut app, true);
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

/// A gossip menu's register closes the latched reader inside its handler (`0x4e26e0` →
/// `0x4930d0` → `0x493310` → `0x4e32e0(0, 0)`), so `ITEM_TEXT_CLOSED` (`0x4e33a2`) fires at the
/// menu's place among the frame's packets, though the reader's lane holds nothing then.
#[test]
fn a_register_at_a_packet_closes_the_reader_at_its_place() {
    const NPC: u64 = 0xF130_0000_0000_0043;
    let mut app = client();
    tablet(&mut app, true);
    frame(
        &mut app,
        vec![SessionEvent::GameObjectPageText { guid: TABLET }],
    );
    spy(&mut app, &["ITEM_TEXT_CLOSED"]);
    let seen = frame(
        &mut app,
        vec![
            fall(5),
            SessionEvent::GossipMenu {
                npc: NPC,
                text_id: 7,
                options: Vec::new(),
                quests: Vec::new(),
            },
            fall(5),
        ],
    );
    assert_eq!(seen, [FELL, "ITEM_TEXT_CLOSED", FELL]);
    assert_eq!(
        app.world()
            .resource::<crate::ui_item_text::ItemTextOpen>()
            .object(),
        None
    );
}

/// With the first page not cached the open asks for it and returns (`0x4e348b` → `0x4e35a0`); the
/// page's answer runs the callback `0x4e3280`, which opens again (`0x4e32a5`), so
/// `ITEM_TEXT_BEGIN` fires at the answer's place.
#[test]
fn an_uncached_page_begins_at_its_answer() {
    let mut app = client();
    tablet(&mut app, false);
    spy(&mut app, &["ITEM_TEXT_BEGIN"]);
    let page = SessionEvent::PageText {
        page_id: 358,
        text: "Page one.".into(),
        next_page_id: 0,
    };
    let seen = frame(
        &mut app,
        vec![
            SessionEvent::GameObjectPageText { guid: TABLET },
            fall(5),
            page,
            fall(5),
        ],
    );
    assert_eq!(seen, [FELL, "ITEM_TEXT_BEGIN", FELL]);
}

/// The session's end drops the signals the item-text lane holds, as the other lanes' ends drop
/// theirs, and its shutdown's sweep closes the reader the packet opened; so does the leave-world
/// sweep run ahead of the drain (a `/reload`'s), which closes the reader (`0x490abc` →
/// `0x4e3240`).
#[test]
fn the_session_end_and_a_sweep_drop_the_item_text_lane() {
    let reader = |app: &App| {
        app.world()
            .resource::<crate::ui_item_text::ItemTextOpen>()
            .object()
    };
    let page_text = || vec![SessionEvent::GameObjectPageText { guid: TABLET }];
    let mut app = client();
    tablet(&mut app, true);
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, page_text());
    world
        .resource_mut::<crate::ui_script::LeavingWorldArmed>()
        .arm();
    world.init_resource::<crate::player::CameraControl>();
    world.run_schedule(OnExit(crate::client_state::ClientState::InWorld));
    crate::lanes::fire_in_order(world);
    assert_eq!(reader(&app), None, "the session's end");

    let mut app = client();
    tablet(&mut app, true);
    spy(&mut app, &["ITEM_TEXT_BEGIN"]);
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, page_text());
    world
        .resource_mut::<crate::ui_script::LeavingWorldArmed>()
        .arm();
    crate::ui_script::run_leave_world_sweep(world);
    assert!(frame(&mut app, vec![]).is_empty(), "the sweep");
    assert_eq!(reader(&app), None);
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
    // With no gossip open its cell is 0 (`0xbc3f58`), which a zero guid matches (`0x5dcc97`).
    app.world_mut()
        .resource_mut::<crate::ui_gossip::GossipState>()
        .npc = None;
    frame(&mut app, vec![list(0)]);
    assert_eq!(open(&app), Some(0), "a zero guid against no gossip");
}

/// `SMSG_CLIENT_CONTROL_UPDATE` for us (`0x5fa6b1` → `SetPlayerControl 0x4958e0`): each change of
/// the flag signals at its packet, `PLAYER_CONTROL_LOST` (`0x49590b`) reading `HasFullControl()`
/// as the packet left it, then the usable refresh's `SPELL_UPDATE_USABLE` (`0x495910`); a packet
/// that changes nothing signals nothing (`0x4958e6`), nor one naming another unit, as `0x5fa600`
/// tells the interface for the active player alone (`0x5fa6a4`).
#[test]
fn each_control_change_signals_at_its_packet() {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"PLAYER_CONTROL_LOST", "PLAYER_CONTROL_GAINED",
                "SPELL_UPDATE_USABLE"}) do f:RegisterEvent(e) end
            f:SetScript("OnEvent", function()
                if event == "SPELL_UPDATE_USABLE" then
                    table.insert(Seen, event)
                else
                    table.insert(Seen, event .. ":" .. tostring(HasFullControl()))
                end
            end)
            "#,
        )
        .unwrap();
    let control = |mover, allow_move| SessionEvent::ClientControl { mover, allow_move };
    let seen = frame(
        &mut app,
        vec![
            fall(5),
            control(ME, false),
            control(ME, false),
            control(STRANGER, true),
            fall(5),
            control(ME, true),
        ],
    );
    assert_eq!(
        seen,
        [
            FELL,
            "PLAYER_CONTROL_LOST:nil",
            "SPELL_UPDATE_USABLE",
            FELL,
            "PLAYER_CONTROL_GAINED:1",
            "SPELL_UPDATE_USABLE",
        ]
    );
}

/// `UNIT_FIELD_SUMMON` and `UNIT_FIELD_POWER5`, the happiness.
const SUMMON: u16 = 8;
const HAPPINESS: u16 = 27;

/// The pet's happiness watch (the bridge's id 21, signalled through `0x515e7a`) runs inside its
/// update packet, so `UNIT_HAPPINESS` fires at that packet's place among the frame's other
/// packets, once per token naming the pet; under its `Deviation:` only a change of the bucket
/// (or its figures) signals, so a move within one bucket is silent.
#[test]
fn the_pets_happiness_signals_at_its_packet() {
    use benilla_formats::{PetLoyaltyNames, PetPersonalities, PetPersonality};
    let mut app = client();
    let row = PetPersonality {
        thresholds: [0, 333_000, 666_000],
        damage: [0.75, 1.0, 1.25],
        loyalty_rate: [-1.0, 0.0, 1.0],
    };
    app.world_mut()
        .insert_resource(crate::ui_pet_stats::PetStatTables {
            personalities: PetPersonalities::from_rows([(1, row)].into()),
            loyalty: PetLoyaltyNames::from_names(Default::default()),
        });
    frame(
        &mut app,
        vec![
            created(PET, EntityKind::Unit, &[(HAPPINESS, 500_000)]),
            values(
                ME,
                &[(SUMMON, PET as u32), (SUMMON + 1, (PET >> 32) as u32)],
            ),
        ],
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HAPPINESS")
            f:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. arg1) end)
            "#,
        )
        .unwrap();
    let happy = |raw: u32| values(PET, &[(HAPPINESS, raw)]);
    let seen = frame(
        &mut app,
        vec![
            fall(5),
            happy(900_000),
            happy(950_000),
            fall(5),
            happy(100_000),
        ],
    );
    assert_eq!(
        seen,
        [FELL, "UNIT_HAPPINESS:pet", FELL, "UNIT_HAPPINESS:pet"]
    );
}
