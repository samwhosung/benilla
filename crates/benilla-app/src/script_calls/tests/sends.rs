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

/// `ShowHelm(flag)` and `ShowCloak(flag)` (`0x48de20`, `0x48de70`) send their flip inside the call
/// (`0x5ee6e3`, `push 0x2b9`), ahead of the chunk's next verb, and `ShowingHelm()` on the next
/// line reads what the call left.
#[test]
fn show_helm_and_show_cloak_send_inside_their_calls() {
    let (mut app, rx) = client();
    seat(&mut app, &[]);
    let shown: Option<i64> = eval(
        &mut app,
        "ShowHelm(0) RandomRoll(1, 2) ShowCloak(0) return ShowingHelm()",
    );
    assert_eq!(shown, None);
    assert_eq!(sent(&rx), ["helm", "roll 1", "cloak"]);
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
/// which rolls, runs after the packet. The store takes the writes after the pass.
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
    app.world_mut()
        .run_system_cached(crate::ui_action::store_action_writes)
        .unwrap();
    let actions = app.world().resource::<crate::ui_action::PlayerActions>();
    assert!(!actions.buttons.contains_key(&2));
    assert_eq!(actions.buttons.get(&4).map(|b| b.action), Some(FIREBALL));
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
    spells(&mut app, &[(STUCK, "Stuck")]);
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
/// trainer inside its call, past its money gate.
#[test]
fn confirm_pet_unlearn_sends_inside_its_call() {
    let (mut app, rx) = client();
    seat(&mut app, &[(COINAGE, 10_000)]);
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
/// ahead of the send, so a channel the player cancels through its aura ends in the call.
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

/// A book of two pages read, the first shown: page 10, whose next is 11, not yet cached.
fn book(app: &mut App) {
    use crate::ui_session::NpcWindow;
    crate::ui_session::latch_tests::open(app, NpcWindow::ItemText, NPC);
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_item_text::PageTexts>()
        .insert(10, "one".into(), 11);
    world
        .resource_mut::<crate::ui_item_text::ItemTextOpen>()
        .show_page_for_test(10);
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
/// runs its listeners there (`0x4e33a2`), so the chunk's next line reads it.
#[test]
fn close_item_text_signals_inside_its_call() {
    let (mut app, _rx) = client();
    book(&mut app);
    listen(&mut app, &["ITEM_TEXT_CLOSED"], "");
    let n: i64 = eval(&mut app, "CloseItemText() return N");
    assert_eq!(n, 1);
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
