//! The verbs that send a packet, each through the whole client's host: each sends inside its call,
//! so a chunk's packets go out in call order with another system's verb (`RandomRoll`,
//! `0x48c832`), and the chunk's next line reads what the verb did.

use benilla_protocol::ObjectFields;
use benilla_ui::script::{UiScript, UnitGuids, UnitState};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::systems::{client, eval, listen, NPC};
use crate::net::{ClientCommand, Guid, GuidIndex, ObjectStore, SelfGuid, SelfPlayer};

/// Our own guid in these chunks.
const ME: u64 = 0x10;
/// Another player, inspected.
const OTHER: u64 = 0x77;
/// `PLAYER_FIELD_COINAGE`.
const COINAGE: u16 = 1176;

/// What went out, in order, of the kinds these chunks send.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, .. } => format!("roll {min}"),
                ClientCommand::ToggleHelm => "helm".into(),
                ClientCommand::ToggleCloak => "cloak".into(),
                ClientCommand::SetActionBarToggles { toggles } => format!("toggles {toggles}"),
                ClientCommand::SetActionButton { button, packed } => {
                    format!("button {button} {packed:#x}")
                }
                ClientCommand::GmTicketGet => "ticket get".into(),
                ClientCommand::GmTicketSystemStatus => "ticket status".into(),
                ClientCommand::GmTicketDelete => "ticket delete".into(),
                ClientCommand::GmTicketCreate { category, text, .. } => {
                    format!("ticket new {category} {text}")
                }
                ClientCommand::GmTicketUpdate { category, text } => {
                    format!("ticket update {category} {text}")
                }
                ClientCommand::LotteryQuery => "lottery".into(),
                ClientCommand::BuyLotteryTicket { first, second } => {
                    format!("lottery buy {first} {second}")
                }
                ClientCommand::CastSpell { spell_id, .. } => format!("cast {spell_id}"),
                ClientCommand::Inspect { target } => format!("inspect {target:#x}"),
                ClientCommand::InspectHonorStats { target } => format!("honor {target:#x}"),
                ClientCommand::PetUnlearn { trainer } => format!("pet unlearn {trainer:#x}"),
                ClientCommand::CancelAura { spell_id } => format!("cancel aura {spell_id}"),
                ClientCommand::MinimapPing { x, y } => format!("ping {x} {y}"),
                ClientCommand::StandStateChange { state } => format!("stand {state}"),
                ClientCommand::SetSheathed { state } => format!("sheathed {state}"),
                ClientCommand::PageTextQuery { page_id, .. } => format!("page {page_id}"),
                _ => return None,
            })
        })
        .collect()
}

fn vm(app: &mut App) -> Mut<'_, UiScript> {
    app.world_mut().non_send_resource_mut::<UiScript>()
}

/// Our player seated with `fields`, in the guid index, and named to the VM as `"player"`.
fn seat(app: &mut App, fields: &[(u16, u32)]) -> Entity {
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let mut pairs = vec![(2, 0x19), (22, 100), (28, 100)];
    pairs.extend_from_slice(fields);
    let me = world
        .spawn((
            Guid(ME),
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&pairs)),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    let mut script = vm(app);
    script.set_unit_guids(&UnitGuids {
        player: ME,
        ..Default::default()
    });
    script.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            guid: ME,
            name: Some("Probe".into()),
            ..Default::default()
        }),
    );
    me
}

/// The spell catalog holding `spells` by name alone.
fn spells(app: &mut App, spells: &[(u32, &str)]) {
    app.world_mut().insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            spells
                .iter()
                .map(|&(id, name)| {
                    (
                        id,
                        benilla_formats::SpellDisplay {
                            name: name.into(),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
}

/// `UNIT_FIELD_DISPLAYID`, `UNIT_FIELD_NATIVEDISPLAYID` and `PLAYER_FLAGS`.
const DISPLAY: u16 = 131;
const NATIVE_DISPLAY: u16 = 132;
const PLAYER_FLAGS: u16 = 190;

/// `PLAYER_VISIBLE_ITEM_1_0`, the head slot's entry, and the helm worn there, whose record the
/// cache holds.
const HEAD_ENTRY: u16 = 260;
const HELM: u32 = 900;

/// Our player created off the wire with display `display` on native display 49, the helm on,
/// seated.
fn seat_created(app: &mut App, display: u32) -> Entity {
    app.world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(HELM, Some(crate::items::test_template("Helm")));
    let fields = ObjectFields::from_pairs(&[
        (2, 0x19),
        (22, 100),
        (28, 100),
        (DISPLAY, display),
        (NATIVE_DISPLAY, 49),
        (HEAD_ENTRY, HELM),
    ])
    .into_created(benilla_protocol::messages::ObjectType::Player);
    app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![
            benilla_protocol::SessionEvent::ObjectCreate {
                guid: ME,
                kind: benilla_protocol::EntityKind::Player,
                display_id: None,
                position: [0.0; 3],
                orientation: 0.0,
                scale: 1.0,
                speeds: None,
                mover: None,
                transport_progress: None,
                transport: None,
                spline: None,
                fields,
            },
            benilla_protocol::SessionEvent::ObjectUpdateEnd,
        ],
    );
    let me = app.world().resource::<GuidIndex>().0[&ME];
    app.world_mut().entity_mut(me).insert(SelfPlayer);
    me
}

/// The server's `PLAYER_FLAGS` for us, one update packet.
fn flags(app: &mut App, flags: u32) {
    values(app, &[(PLAYER_FLAGS, flags)]);
}

/// One values packet of ours.
fn values(app: &mut App, pairs: &[(u16, u32)]) {
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![
            benilla_protocol::SessionEvent::ObjectValues {
                guid: ME,
                fields: ObjectFields::from_pairs(pairs),
            },
            benilla_protocol::SessionEvent::ObjectUpdateEnd,
        ],
    );
}

/// Whether our model shows `(helm hidden, cloak hidden)`.
fn worn_hidden(app: &App, me: Entity) -> (bool, bool) {
    let w = app.world();
    crate::entities::WornPins::hidden(
        w.get::<crate::entities::WornPins>(me),
        &w.get::<ObjectStore>(me).unwrap().0,
    )
}

/// `ShowHelm(flag)` and `ShowCloak(flag)` (`0x48de20`, `0x48de70`) read the live `PLAYER_FLAGS`
/// bit inside the call and send the toggle only while it says the opposite (`0x5ee6c4`,
/// `0x5ee783`), ahead of the chunk's next verb; `ShowingHelm()` reads the same bit (`0x48dd8b`).
#[test]
fn show_helm_and_show_cloak_send_on_the_live_bit_inside_their_calls() {
    let (mut app, rx) = client();
    let none: Option<i64> = eval(&mut app, "ShowHelm(0) ShowCloak(1) return ShowingHelm()");
    assert_eq!(none, None, "no player: nil, and nothing sent (`0x48de43`)");
    seat_created(&mut app, 49);
    let shown: Option<i64> = eval(
        &mut app,
        "ShowHelm(0) ShowHelm(0) RandomRoll(1, 2) ShowCloak(1) ShowCloak(0) return ShowingHelm()",
    );
    assert_eq!(
        shown,
        Some(1),
        "the bit has not moved, so the getter says shown"
    );
    assert_eq!(
        sent(&rx),
        ["helm", "helm", "roll 1", "cloak"],
        "two hides on a shown bit are two toggles, a show on it none"
    );

    // The server's answers set both hide bits: now a show is owed the toggle and a hide none.
    flags(&mut app, 0xc00);
    let shown: Option<i64> = eval(
        &mut app,
        "ShowHelm(0) ShowHelm(1) ShowCloak(1) return ShowingHelm()",
    );
    assert_eq!(shown, None);
    assert_eq!(sent(&rx), ["helm", "cloak"]);
}

/// On the native display the call re-dresses the model ahead of the server, attaching
/// (`0x5ee6af`) or detaching (`0x5ee76e`) the piece, until the bit's next edge re-dresses it by
/// the flag (`0x5eea45`); on another display it leaves the model alone (`0x5ee696`).
#[test]
fn show_helm_redresses_the_native_model_until_the_bit_moves() {
    let hidden = worn_hidden;
    let (mut app, rx) = client();
    let me = seat_created(&mut app, 49);
    eval::<()>(&mut app, "ShowHelm(0) ShowCloak(1)");
    assert_eq!(
        hidden(&app, me),
        (true, false),
        "the helm comes off at the call; no cloak is worn to put on"
    );
    assert_eq!(sent(&rx), ["helm"]);

    // The answer, then a `/togglehelm` and its answer: each edge dresses by the flag.
    flags(&mut app, 0x400);
    assert_eq!(hidden(&app, me), (true, false));
    eval::<()>(&mut app, "ShowHelm(1)");
    assert_eq!(hidden(&app, me), (false, false), "back on at the call");
    flags(&mut app, 0);
    flags(&mut app, 0x400);
    assert_eq!(
        hidden(&app, me),
        (true, false),
        "the bit moved after the call, so the flag dresses it"
    );

    // Shapeshifted: the model is another display's, and only the toggle goes out.
    let (mut app, rx) = client();
    let me = seat_created(&mut app, 892);
    eval::<()>(&mut app, "ShowHelm(0)");
    assert_eq!(hidden(&app, me), (false, false));
    assert_eq!(sent(&rx), ["helm"]);

    // No head item with a display: a show leaves the model alone (`0x5ee66e`, `0x5ee67b`).
    let (mut app, _rx) = client();
    let me = seat_created(&mut app, 49);
    flags(&mut app, 0x400);
    values(&mut app, &[(HEAD_ENTRY, 0)]);
    eval::<()>(&mut app, "ShowHelm(1)");
    assert_eq!(hidden(&app, me), (true, false));
}

/// A call's re-dress ends at the next one by the flag: a display rebuild (`0x60abe0` →
/// `0x5fb200` → `0x5ed700`, the bit at `0x5ed802`) and a new head item (`0x5dee30` → `0x5df0de`).
#[test]
fn a_rebuild_or_a_new_helm_dresses_by_the_flag_again() {
    let (mut app, _rx) = client();
    app.world_mut()
        .insert_resource(crate::entities::Creatures::from_catalog(
            benilla_formats::CreatureCatalog::from_record_rows(
                &[(49, 1, 0, 0), (50, 2, 0, 0)],
                &[(1, 7), (2, 7)],
                &[7],
            ),
        ));
    let me = seat_created(&mut app, 49);
    eval::<()>(&mut app, "ShowHelm(0)");
    assert_eq!(worn_hidden(&app, me), (true, false));
    values(&mut app, &[(DISPLAY, 50)]);
    assert_eq!(
        worn_hidden(&app, me),
        (false, false),
        "the body rebuilt for another display wears the helm the flag shows"
    );

    values(&mut app, &[(DISPLAY, 49)]);
    eval::<()>(&mut app, "ShowHelm(0)");
    assert_eq!(worn_hidden(&app, me), (true, false));
    values(&mut app, &[(HEAD_ENTRY, HELM + 1)]);
    assert_eq!(
        worn_hidden(&app, me),
        (false, false),
        "a new helm, by the flag"
    );

    // The watch covers the slot's whole block (`0x5e2541`): an enchant's move re-dresses too.
    eval::<()>(&mut app, "ShowHelm(0)");
    assert_eq!(worn_hidden(&app, me), (true, false));
    values(&mut app, &[(HEAD_ENTRY + 1, 1900)]);
    assert_eq!(
        worn_hidden(&app, me),
        (false, false),
        "an enchant moved, by the flag"
    );
}

/// `SetActionBarToggles` (`0x4e76e0`) sends `CMSG_SET_ACTIONBAR_TOGGLES` inside its call
/// (`0x4e771d`), one per call with no change test.
#[test]
fn set_action_bar_toggles_sends_inside_its_call() {
    let (mut app, rx) = client();
    eval::<()>(
        &mut app,
        "SetActionBarToggles(1, nil, 1) RandomRoll(1, 2) SetActionBarToggles()",
    );
    assert_eq!(sent(&rx), ["toggles 5", "roll 1", "toggles 0"]);
}

/// `PickupAction` and `PlaceAction` write the slot through `0x4e58e0`, which sends
/// `CMSG_SET_ACTION_BUTTON` and then signals `ACTIONBAR_SLOT_CHANGED` (`0x4e5998`): the handler,
/// which rolls, runs after the packet. The slot is stored in the call (`0x4e590a`).
#[test]
fn pickup_and_place_action_send_ahead_of_their_slot_signal() {
    const FIREBALL: u32 = 133;
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    vm(&mut app).set_action(
        3,
        Some(benilla_ui::script::ActionSlot {
            kind: 0,
            action: FIREBALL,
            ..Default::default()
        }),
    );
    listen(&mut app, &["ACTIONBAR_SLOT_CHANGED"], "RandomRoll(9, 10)");
    eval::<()>(&mut app, "PickupAction(3) RandomRoll(1, 2) PlaceAction(5)");
    assert_eq!(
        sent(&rx),
        [
            "button 2 0x0",
            "roll 9",
            "roll 1",
            "button 4 0x85",
            "roll 9"
        ]
    );
    let slots = app.world().resource::<crate::ui_action::ActionSlots>();
    assert!(!slots.buttons.contains_key(&2));
    assert_eq!(slots.buttons.get(&4).map(|b| b.action), Some(FIREBALL));
}

/// `0x4e58e0` writes the slot arrays inside the call (`0x4e590a`), so a later verb of the chunk
/// reads the slot as the write left it: `UseAction` on a slot just picked up finds it empty, and on
/// one just placed casts what was placed (`0x4e5ee0`).
#[test]
fn a_slot_written_by_lua_is_read_by_the_chunks_next_verb() {
    const STUCK: u32 = 7355;
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    spell_rows(&mut app, &[(STUCK, 0, 84)]);
    app.world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(STUCK);
    let slot = benilla_ui::script::ActionSlot {
        kind: 0,
        action: STUCK,
        ..Default::default()
    };
    vm(&mut app).set_action(3, Some(slot));
    app.world_mut()
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            2,
            benilla_protocol::messages::ActionButton {
                slot: 2,
                action: STUCK,
                kind: 0,
            },
        );
    eval::<()>(
        &mut app,
        "PickupAction(3) UseAction(3) PlaceAction(5) UseAction(5) RandomRoll(1, 2)",
    );
    assert_eq!(
        sent(&rx),
        ["button 2 0x0", "button 4 0x1cbb", "cast 7355", "roll 1"]
    );
}

/// The GM ticket verbs send inside their calls (`0x48c9a0`, `0x48cfc0`, `0x48cb70`, `0x48c9d0`,
/// `0x48ca60`), and so do the lottery's (`0x4c4220`, `0x4c4400`), each where the chunk calls it.
#[test]
fn the_gm_ticket_and_lottery_verbs_send_inside_their_calls() {
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    eval::<()>(
        &mut app,
        r#"GetGMTicket() RandomRoll(1, 2) GetGMStatus() DeleteGMTicket() NewGMTicket(2, "stuck")
           RandomRoll(2, 3) UpdateGMTicket(3, "still") GetLastLotteryNumbers() BuyRandomPicks(2)"#,
    );
    assert_eq!(
        sent(&rx),
        [
            "ticket get",
            "roll 1",
            "ticket status",
            "ticket delete",
            "ticket new 2 stuck",
            "roll 2",
            "ticket update 3 still",
            "lottery",
            "lottery buy 2 0",
        ]
    );
}

/// `Stuck()` (`0x489370`) casts its spell through `0x6e5a90` inside its call.
#[test]
fn stuck_casts_inside_its_call() {
    const STUCK: u32 = 7355;
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    spell_rows(&mut app, &[(STUCK, 0, 84)]);
    // The book registered it ([`crate::ui_action::PlayerActions::stuck_spell`]).
    app.world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .stuck_spell = Some(STUCK);
    eval::<()>(&mut app, "Stuck() RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["cast 7355", "roll 1"]);
}

/// `NotifyInspect(unit)` sends `CMSG_INSPECT` and sets the inspect target inside its call
/// (`0x5ded10`, `0x5ded1c`), and `RequestInspectHonorData()` asks about that target inside its
/// own (`0x4c8112`).
#[test]
fn notify_inspect_and_the_honor_ask_send_inside_their_calls() {
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    let world = app.world_mut();
    let other = world
        .spawn((
            Guid(OTHER),
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[(2, 0x19), (22, 100), (28, 100)])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(OTHER, other);
    let mut selection = world.resource_mut::<crate::target::Selection>();
    selection.guid = Some(OTHER);
    selection.target = Some(other);
    eval::<()>(
        &mut app,
        r#"NotifyInspect("target") RandomRoll(1, 2) RequestInspectHonorData()"#,
    );
    assert_eq!(sent(&rx), ["inspect 0x77", "roll 1", "honor 0x77"]);
}

/// `ConfirmPetUnlearn()` (`0x48dca0` → `0x5dfba0`) sends `CMSG_PET_UNLEARN` to the remembered
/// trainer inside its call, in range and past its money gate.
#[test]
fn confirm_pet_unlearn_sends_inside_its_call() {
    let (mut app, rx) = client();
    seat(&mut app, &[(COINAGE, 10_000)]);
    unit_at(&mut app, NPC, 3.0);
    app.world_mut()
        .resource_mut::<crate::ui_dialog_verbs::PetUnlearnState>()
        .ask(NPC, 100);
    eval::<()>(&mut app, "ConfirmPetUnlearn() RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["pet unlearn 0xf130000000000042", "roll 1"]);
}

/// `DoCraft(index)` (`0x4f7d10`) casts the row's spell through `0x6e5a90` inside its call
/// (`0x4f7d64`).
#[test]
fn do_craft_casts_inside_its_call() {
    const ENCHANT: u32 = 7418;
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    spells(&mut app, &[(ENCHANT, "Enchant Bracer - Minor Health")]);
    vm(&mut app).set_craft(Some(benilla_ui::script::CraftState {
        name: "Enchanting".into(),
        craft_type: 3,
        display_line: Some("Enchanting".into()),
        rank: 1,
        max_rank: 75,
        recipes: vec![benilla_ui::script::CraftRecipe {
            spell_id: ENCHANT,
            name: "Enchant Bracer - Minor Health".into(),
            sub_name: String::new(),
            difficulty: benilla_ui::script::CraftDifficulty::Optimal,
            num_available: 0,
            icon: None,
            description: None,
            needs_item_target: false,
            reagents: Vec::new(),
            tools: Vec::new(),
            tooltip: benilla_ui::script::CraftTooltip::Spell(ENCHANT),
            spell_level: 1,
            group: 0,
            skill_line: None,
            training_point_cost: 0,
            required_level: 0,
            tools_pending: Vec::new(),
        }],
    }));
    eval::<()>(&mut app, "DoCraft(1) RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["cast 7418", "roll 1"]);
}

/// `CancelPlayerBuff` and `CancelTrackingBuff` (`0x4e49a0`, `0x4e4a80`) send `CMSG_CANCEL_AURA`
/// through `0x6e7040` inside their calls (`0x6e70eb`).
#[test]
fn the_aura_cancels_send_inside_their_calls() {
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    let mut script = vm(&mut app);
    script.set_player_auras(vec![benilla_ui::script::AuraState {
        spell_id: 1243,
        helpful: true,
        cancelable: true,
        count: 1,
        ..Default::default()
    }]);
    script.set_tracking(Some(benilla_ui::script::TrackingState {
        spell_id: 2580,
        cancelable: true,
        ..Default::default()
    }));
    eval::<()>(
        &mut app,
        "CancelPlayerBuff(0) RandomRoll(1, 2) CancelTrackingBuff()",
    );
    assert_eq!(
        sent(&rx),
        ["cancel aura 1243", "roll 1", "cancel aura 2580"]
    );
}

/// For the active player `0x6e7040` first ends the precast of the cancelled spell
/// (`0x60d040(player, spell, 1, 1)` at `0x6e7075`) and reaps its kits (`0x614150` at `0x6e707e`),
/// ahead of the send. In benilla the two are unit edits, queued in the call and applied after the
/// pass with the frame's others (the test applies them by hand): the components they write are
/// read by the systems that lend the call host, so the host cannot write them in the call.
#[test]
fn an_aura_cancel_ends_the_players_precast_of_that_spell() {
    const ARCANE_MISSILES: u32 = 5143;
    let (mut app, rx) = client();
    let me = seat(&mut app, &[]);
    app.world_mut()
        .entity_mut(me)
        .insert(crate::creature_anim::Precast {
            spell_id: ARCANE_MISSILES,
            busy: true,
        });
    vm(&mut app).set_player_auras(vec![benilla_ui::script::AuraState {
        spell_id: ARCANE_MISSILES,
        channeled: true,
        count: 1,
        ..Default::default()
    }]);
    eval::<()>(&mut app, "CancelPlayerBuff(0)");
    crate::creature_anim::apply_unit_edits(app.world_mut());
    assert_eq!(sent(&rx), ["cancel aura 5143"]);
    assert!(
        app.world()
            .get::<crate::creature_anim::Precast>(me)
            .is_none(),
        "the cancel ended the precast"
    );
}

/// A Minimap at our position, in a group, with `MINIMAP_PING` counted.
fn minimap(app: &mut App) {
    seat(app, &[]);
    app.world_mut()
        .resource_mut::<crate::ui_party::GroupState>()
        .in_group = true;
    vm(app).set_minimap_player(Some((100.0, 200.0)));
    eval::<()>(
        app,
        r#"m = CreateFrame("Minimap", "Minimap") m:SetWidth(140) m:SetHeight(140)"#,
    );
    listen(app, &["MINIMAP_PING"], "");
}

/// `Minimap:PingLocation()` (`0x4eeca0`) sends `MSG_MINIMAP_PING` in a group (`0x4eef72`), then
/// applies the ping (`0x4eefb5`), both inside the call.
#[test]
fn ping_location_sends_inside_its_call() {
    let (mut app, rx) = client();
    minimap(&mut app);
    let n: i64 = eval(&mut app, "m:PingLocation() RandomRoll(1, 2) return N");
    assert_eq!(n, 1, "the ping signalled in the call");
    assert_eq!(sent(&rx), ["ping 100 200", "roll 1"]);
}

/// A ground spell armed: the word set to `word` for `spell`, and the view pushed ahead of the
/// frame's input records.
fn arm_ground_spell(app: &mut App, spell: u32, word: u16) {
    app.world_mut()
        .resource_mut::<crate::spell::targeting::SpellTargeting>()
        .enter(spell, crate::spell::CastCommit::Spell, word);
    app.world_mut()
        .run_system_cached(crate::spell::targeting::feed_targeting_to_input)
        .unwrap();
}

/// With a ground spell armed (`0x6e48a0` and `0x6e6320` at `0x4eee67`, `0x4eee74`) the click casts
/// at the ground under it instead of pinging; a terrain trace that hits nothing does nothing at all
/// (`0x4eeedd`): no ping, no packet, and the cursor stays.
#[test]
fn ping_location_with_a_ground_spell_armed_and_no_ground_does_nothing() {
    const BLIZZARD: u32 = 10;
    let (mut app, rx) = client();
    minimap(&mut app);
    arm_ground_spell(&mut app, BLIZZARD, 0x40);
    let n: i64 = eval(&mut app, "m:PingLocation() RandomRoll(1, 2) return N");
    assert_eq!(n, 0, "no ping");
    assert_eq!(sent(&rx), ["roll 1"]);
    assert_eq!(
        app.world()
            .resource::<crate::spell::targeting::SpellTargeting>()
            .spell(),
        Some(BLIZZARD),
        "the cursor stays"
    );
}

/// A word that wants no location (a unit spell's) pings as ever: only `0x6e6320`'s `word & 0x60`
/// takes the click.
#[test]
fn ping_location_with_a_unit_spell_armed_pings() {
    const FIREBALL: u32 = 133;
    let (mut app, rx) = client();
    minimap(&mut app);
    arm_ground_spell(&mut app, FIREBALL, 0x0002);
    let n: i64 = eval(&mut app, "m:PingLocation() RandomRoll(1, 2) return N");
    assert_eq!(n, 1);
    assert_eq!(sent(&rx), ["ping 100 200", "roll 1"]);
}

/// A book of two pages read, the first shown: page 10, whose next is 11, not yet cached. The
/// latch harness's book is a TEXT object of entry 1, whose first page is `data[0]`.
fn book(app: &mut App) {
    use crate::ui_session::NpcWindow;
    let mut data = [0i32; 24];
    data[0] = 10;
    let world = app.world_mut();
    world
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(1, 9, "Book".into(), &data);
    world
        .resource_mut::<crate::ui_item_text::PageTexts>()
        .insert(10, "one".into(), 11);
    crate::ui_session::latch_tests::open(app, NpcWindow::ItemText, NPC);
}

/// `ItemTextNextPage()` (`0x4e3b60` → `0x4e38b0`) moves the page and shows it inside its call
/// (`0x4e35b0`): a page not cached is asked for there, ahead of the chunk's next verb.
#[test]
fn item_text_next_page_asks_for_the_page_inside_its_call() {
    let (mut app, rx) = client();
    book(&mut app);
    let _ = sent(&rx);
    eval::<()>(&mut app, "ItemTextNextPage() RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["page 11", "roll 1"]);
}

/// `CloseItemText()` (`0x4e3b70` → `0x4e32e0(0, 0)`) closes inside its call: `ITEM_TEXT_CLOSED`
/// runs its listeners there (`0x4e33a2`), so the chunk's next line reads it, twice, as the
/// latched book's release closes it once more inside the core (`0x4e3357` → `0x493310`).
#[test]
fn close_item_text_signals_inside_its_call() {
    let (mut app, _rx) = client();
    book(&mut app);
    listen(&mut app, &["ITEM_TEXT_CLOSED"], "");
    let n: i64 = eval(&mut app, "CloseItemText() return N");
    assert_eq!(n, 2);
    assert_eq!(
        app.world()
            .resource::<crate::ui_item_text::ItemTextOpen>()
            .object(),
        None
    );
}

/// `SitOrStand()` (`0x48b920`) runs `SetStandState` on the player inside its call: a standing
/// player sits (`0x48b951` → `0x5ed430(1)`), its `CMSG_STANDSTATECHANGE` ahead of the chunk's next
/// verb.
#[test]
fn sit_or_stand_sends_inside_its_call() {
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    eval::<()>(&mut app, "SitOrStand() RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["stand 1", "roll 1"]);
}

/// `ToggleSheath()` (`0x48a070` → `0x5eb480`) runs the setter inside its call: holding a sword,
/// stowed, it draws (`0x5eb642` → `0x611cf0`), its `CMSG_SETSHEATHED` (`0x611dbc`) ahead of the
/// chunk's next verb. The draw's animation starts inside the setter (`0x611cea` → `0x611a20`), so
/// a second toggle in the chunk finds it running and is refused (`0x5eb5a0`-`0x5eb5e2`). Main
/// toggled at the frame's posture pass.
#[test]
fn toggle_sheath_sends_inside_its_call() {
    let (mut app, rx) = client();
    let me = seat(&mut app, &[]);
    app.world_mut().entity_mut(me).insert((
        crate::creature_anim::AnimDriver::default(),
        AnimationPlayer::default(),
        bevy::animation::transition::AnimationTransitions::default(),
        crate::go_anim::owned_anims(&[89, 90]),
        crate::creature_anim::Wielded {
            main: Some((2, 7)),
            ..Default::default()
        },
    ));
    eval::<()>(&mut app, "ToggleSheath() RandomRoll(1, 2) ToggleSheath()");
    assert_eq!(sent(&rx), ["sheathed 1", "roll 1"]);
}

/// `PLAYER_FARSIGHT`'s low word.
const FARSIGHT: u16 = 712;

/// A unit `guid` standing `yards` along +X, in the guid index.
fn unit_at(app: &mut App, guid: u64, yards: f32) {
    let world = app.world_mut();
    let e = world
        .spawn((
            Guid(guid),
            Transform::from_xyz(yards, 0.0, 0.0),
            ObjectStore(ObjectFields::from_pairs(&[(2, 0x9), (22, 100), (28, 100)])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(guid, e);
}

/// The spell catalog holding `rows` as `(id, AttributesEx, Effect[0])`.
fn spell_rows(app: &mut App, rows: &[(u32, u32, u32)]) {
    app.world_mut().insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            rows.iter()
                .map(|&(id, attributes_ex, effect)| {
                    (
                        id,
                        benilla_formats::SpellDisplay {
                            id,
                            attributes_ex,
                            effects: [effect, 0, 0],
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
}

/// A far-sight aura (`AttributesEx` 0x2000 without 0x4) the player cancels: `0x6e7040` calls
/// `UpdateFarSight` (`0x6e709e`-`0x6e70ad`), and when it answers nonzero the cancel sends nothing
/// (`0x6e70b4` → `0x6e7122`). It answers 0 only for a `PLAYER_FARSIGHT` set on an object the
/// client does not hold (`0x5ee32b` → `0x5ee360`); then the cancel goes out.
#[test]
fn a_far_sight_aura_cancel_sends_only_when_far_sight_answers_zero() {
    const SENTRY_TOTEM: u32 = 6495;
    for farsight in [0u32, 0x77] {
        let (mut app, rx) = client();
        seat(&mut app, &[(FARSIGHT, farsight)]);
        spell_rows(&mut app, &[(SENTRY_TOTEM, 0x2000, 90)]);
        vm(&mut app).set_player_auras(vec![benilla_ui::script::AuraState {
            spell_id: SENTRY_TOTEM,
            helpful: true,
            cancelable: true,
            count: 1,
            ..Default::default()
        }]);
        eval::<()>(&mut app, "CancelPlayerBuff(0) RandomRoll(1, 2)");
        let expected: &[&str] = if farsight == 0 {
            &["roll 1"]
        } else {
            &["cancel aura 6495", "roll 1"]
        };
        assert_eq!(sent(&rx), expected, "far sight {farsight:#x}");
    }
}

/// The healer scan's cancel of a wave (`0x4921c0` → `0x6e7040(0xa18)` at `0x492201`) is the whole
/// aura cancel: the player's precast of 2584 ends too (`0x6e7075`).
#[test]
fn the_healer_scans_cancel_ends_the_players_precast_of_the_wave() {
    const HEALER: u64 = 0xF130_0000_0000_0A5A;
    const WAVE: u32 = 2584;
    let (mut app, rx) = client();
    let me = seat(&mut app, &[(22, 1), (190, 0x10)]);
    app.world_mut()
        .entity_mut(me)
        .insert(crate::creature_anim::Precast {
            spell_id: WAVE,
            busy: false,
        });
    unit_at(&mut app, HEALER, 30.0);
    let e = app.world().resource::<GuidIndex>().0[&HEALER];
    app.world_mut().entity_mut(e).insert(crate::net::NetEntity {
        kind: benilla_protocol::EntityKind::Unit,
        display_id: None,
        scale: 1.0,
    });
    {
        let mut spirit = app
            .world_mut()
            .resource_mut::<crate::ui_dialog_verbs::AreaSpiritHealer>();
        let _ = spirit.set_healer(Some(HEALER));
        spirit.on_time(HEALER, 30_000, std::time::Instant::now());
    }
    eval::<()>(&mut app, "AcceptAreaSpiritHeal()");
    crate::creature_anim::apply_unit_edits(app.world_mut());
    assert_eq!(sent(&rx), ["cancel aura 2584"]);
    assert!(
        app.world()
            .get::<crate::creature_anim::Precast>(me)
            .is_none(),
        "the scan's cancel ended the precast"
    );
}

/// `NotifyInspect(unit)` resolves any unit token (`0x515970` at `0x48a36a`) and sends for any
/// nonzero guid (`0x5deccf`): a creature target and the player alike.
#[test]
fn notify_inspect_sends_for_any_unit_the_token_names() {
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    unit_at(&mut app, NPC, 3.0);
    let world = app.world_mut();
    let npc = world.resource::<GuidIndex>().0[&NPC];
    let mut selection = world.resource_mut::<crate::target::Selection>();
    selection.guid = Some(NPC);
    selection.target = Some(npc);
    eval::<()>(
        &mut app,
        r#"NotifyInspect("target") RandomRoll(1, 2) NotifyInspect("player")"#,
    );
    assert_eq!(
        sent(&rx),
        ["inspect 0xf130000000000042", "roll 1", "inspect 0x10"]
    );
}

/// `ConfirmPetUnlearn()` tests the trainer's range at the call (`0x5dfbd5`, `0x5dfc1c` against
/// `[0xc4c28c]`): out of it nothing happens and the trainer stays remembered, so the confirm made
/// back in range sends.
#[test]
fn confirm_pet_unlearn_tests_the_trainers_range_at_the_call() {
    let (mut app, rx) = client();
    let me = seat(&mut app, &[(COINAGE, 10_000)]);
    unit_at(&mut app, NPC, 10.0);
    app.world_mut()
        .resource_mut::<crate::ui_dialog_verbs::PetUnlearnState>()
        .ask(NPC, 100);
    eval::<()>(&mut app, "ConfirmPetUnlearn() RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["roll 1"], "out of range");
    app.world_mut()
        .get_mut::<Transform>(me)
        .unwrap()
        .translation
        .x = 6.0;
    eval::<()>(&mut app, "ConfirmPetUnlearn()");
    assert_eq!(
        sent(&rx),
        ["pet unlearn 0xf130000000000042"],
        "back in range"
    );
}

/// `RequestInspectHonorData()` (`0x4c80a0`) sets its pending flag after the send (`0x4c811b`), and
/// with no inspect target returns before it (`0x4c80c2`): a request made before the target is set
/// leaves the next one free to send, and that one holds off a third.
#[test]
fn the_inspect_honor_ask_latches_only_once_it_sends() {
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    unit_at(&mut app, OTHER, 3.0);
    let world = app.world_mut();
    let other = world.resource::<GuidIndex>().0[&OTHER];
    world
        .entity_mut(other)
        .insert(ObjectStore(ObjectFields::from_pairs(&[
            (2, 0x19),
            (22, 100),
            (28, 100),
        ])));
    let mut selection = world.resource_mut::<crate::target::Selection>();
    selection.guid = Some(OTHER);
    selection.target = Some(other);
    eval::<()>(
        &mut app,
        r#"RequestInspectHonorData() NotifyInspect("target") RequestInspectHonorData()
           RequestInspectHonorData()"#,
    );
    assert_eq!(sent(&rx), ["inspect 0x77", "honor 0x77"]);
}

/// `Stuck()` casts the spell in `[0xb70098]` (`0x489370`), which `AddSpellToBook` stores for a
/// learned spell whose `Effect[0]` is 84 (`0x4b2614`-`0x4b261d`): that spell, whatever its id, and
/// none when no learned spell has the effect.
#[test]
fn stuck_casts_the_learned_spell_with_the_stuck_effect() {
    const UNSTUCK: u32 = 9999;
    for learned in [false, true] {
        let (mut app, rx) = client();
        seat(&mut app, &[]);
        spell_rows(&mut app, &[(UNSTUCK, 0, 84)]);
        if learned {
            app.world_mut()
                .resource_mut::<crate::ui_action::PlayerActions>()
                .stuck_spell = Some(UNSTUCK);
        }
        eval::<()>(&mut app, "Stuck() RandomRoll(1, 2)");
        let expected: &[&str] = if learned {
            &["cast 9999", "roll 1"]
        } else {
            &["roll 1"]
        };
        assert_eq!(sent(&rx), expected, "learned {learned}");
    }
}
