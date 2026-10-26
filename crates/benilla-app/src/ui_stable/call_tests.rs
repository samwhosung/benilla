//! The stable verbs through the whole client's host: each sends its request inside its call, as
//! the reference's bindings do (`0x4cae50`, `0x4caf20`, `0x4cb080`, `0x4cb420`).

use benilla_protocol::messages::ObjectType;
use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x10;
const MASTER: u64 = 0xF130_0000_4500_0001;
/// `UNIT_FIELD_SUMMON`, low word.
const SUMMON: u16 = 8;

struct Stable {
    app: App,
    rx: Receiver<ClientCommand>,
}

/// The whole client, headless, with us streamed (`summon` our summoned pet's guid, or none), a
/// stable open on its master with one slot bought and pet 3 in it, and 1g in the purse.
fn client(summon: u64) -> Stable {
    client_in_slot(summon, 1)
}

/// [`client`] with pet 3 in stable slot `slot` (1 or 2, the rebased wire byte).
fn client_in_slot(summon: u64, slot: u8) -> Stable {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let me = ObjectFields::from_pairs(&[
        (2, 0x19),
        // The purse the buy's gate reads (`0x4cb122`).
        (benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE, 10_000),
        (SUMMON, summon as u32),
        (SUMMON + 1, (summon >> 32) as u32),
    ])
    .into_created(ObjectType::Player);
    let me = world
        .spawn((
            crate::net::SelfPlayer,
            crate::net::Guid(ME),
            crate::net::ObjectStore(me),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.insert_resource(crate::net::SelfGuid(Some(ME)));
    world.resource_mut::<super::StableOpen>().open(
        MASTER,
        1,
        vec![benilla_protocol::messages::StabledPet {
            pet_number: 3,
            creature_entry: 1_126,
            level: 10,
            name: "Boar".into(),
            loyalty: 1,
            slot,
        }],
    );
    // The boar's template and its family's icon, which the pickup needs (`0x4950c0`).
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_creature(
            1_126,
            Some(crate::names::CreatureRecord {
                name: "Boar".into(),
                subname: None,
                creature_type: 1,
                pet_family: 5,
                rank: 0,
                type_flags: 0,
                pet_spell_list: 0,
                civilian: false,
                racial_leader: false,
                display_id: 607,
            }),
        );
    world.insert_resource(crate::ui_pet_stats::PetFamilyTables {
        families: benilla_formats::CreatureFamilies::from_rows([(
            5,
            benilla_formats::CreatureFamily {
                name: "Boar".into(),
                icon: "Interface\\Icons\\Ability_Hunter_Pet_Boar".into(),
                pet_food_mask: 0,
                min_scale: 1.0,
                min_scale_level: 0,
                max_scale: 1.0,
                max_scale_level: 0,
                skill_lines: [0, 0],
            },
        )]),
        foods: Default::default(),
    });
    // The next slot's price, 500 copper, which the buy's gate reads (`0x4cb122`).
    world.insert_resource(super::StableSlotPrices(
        benilla_formats::StableSlotPrices::from_rows([(2, 500)]),
    ));
    let script = UiScript::new().expect("a VM");
    app.world_mut().insert_non_send_resource(script);
    // The list's signal, which leaves the VM its copy.
    app.world_mut()
        .resource_mut::<super::StableLane>()
        .push(super::StableEvent::Show);
    crate::lanes::fire_in_order(app.world_mut());
    Stable { app, rx }
}

fn run(s: &mut Stable, lua: &str) {
    with_hosted_vm(s.app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
}

/// The requests sent since the last read, as their debug lines.
fn sent(s: &Stable) -> Vec<String> {
    s.rx.try_iter().map(|c| format!("{c:?}")).collect()
}

fn lines(commands: &[ClientCommand]) -> Vec<String> {
    commands.iter().map(|c| format!("{c:?}")).collect()
}

/// Each stable request is out when its call returns, before any drain runs.
#[test]
fn the_stable_verbs_send_inside_their_calls() {
    let mut s = client(0);
    run(&mut s, "StablePet() BuyStableSlot() UnstablePet(1)");
    assert_eq!(
        sent(&s),
        lines(&[
            ClientCommand::StablePet { npc: MASTER },
            ClientCommand::BuyStableSlot { npc: MASTER },
            ClientCommand::UnstablePet {
                npc: MASTER,
                pet_number: 3,
            },
        ])
    );
    // The drop of a stabled pet on the summoned slot with no pet out unstables it (`0x4cb6bf`).
    run(&mut s, "PickupStablePet(1) ClickStablePet(0)");
    assert_eq!(
        sent(&s),
        lines(&[ClientCommand::UnstablePet {
            npc: MASTER,
            pet_number: 3,
        }])
    );
}

/// The stabled pets pack in list order (`0x4cab10`, the array `0xb720c8`): with only the second
/// stable slot occupied, which happens after the first pet is taken out because the server does not
/// renumber, the pet is the first box and `UnstablePet(1)` takes it; the second box is empty.
#[test]
fn a_pet_in_the_second_slot_is_the_first_box() {
    let mut s = client_in_slot(0, 2);
    let name = |s: &mut Stable, i: u32| -> Option<String> {
        with_hosted_vm(s.app.world_mut(), |vm| {
            vm.expect("a VM")
                .eval(&format!("local _, n = GetStablePetInfo({i}) return n"))
                .unwrap()
        })
    };
    assert_eq!(name(&mut s, 1).as_deref(), Some("Boar"));
    assert_eq!(name(&mut s, 2), None);
    run(&mut s, "UnstablePet(2) UnstablePet(1)");
    assert_eq!(
        sent(&s),
        lines(&[ClientCommand::UnstablePet {
            npc: MASTER,
            pet_number: 3,
        }])
    );
}

/// The summoned pet is the row whose raw slot byte has bit 1 clear (`0x4cabe3`), not the one with
/// byte 1: a byte of 4 is summoned too, and fills box 0.
#[test]
fn the_summoned_row_is_the_one_whose_slot_byte_has_bit_one_clear() {
    use benilla_protocol::messages::StabledPet;
    let pet = |slot: u8| StabledPet {
        pet_number: 3,
        creature_entry: 1_126,
        level: 10,
        name: "Boar".into(),
        loyalty: 1,
        slot,
    };
    // Raw bytes 1, 4 and 5 are summoned; 2 and 3 stabled.
    assert_eq!(
        [0, 1, 2, 3, 4].map(|slot| pet(slot).is_summoned()),
        [true, false, false, true, true]
    );
    // A row with raw byte 4 (rebased 3) is the summoned pet: box 0, no stabled row.
    let mut s = client_in_slot(0, 3);
    let name = |s: &mut Stable, i: i32| -> Option<String> {
        with_hosted_vm(s.app.world_mut(), |vm| {
            vm.expect("a VM")
                .eval(&format!("local _, n = GetStablePetInfo({i}) return n"))
                .unwrap()
        })
    };
    assert_eq!(name(&mut s, 0).as_deref(), Some("Boar"));
    assert_eq!(name(&mut s, 1), None);
}

/// `UnstablePet` bails while the active player holds a charm or a summon (`0x4caf73`-`0x4caf88`),
/// read off its own descriptor, whatever the pet bar holds.
#[test]
fn unstable_pet_is_refused_while_we_hold_a_summon() {
    let mut s = client(0xF140_0000_0700_002A);
    run(&mut s, "UnstablePet(1)");
    assert_eq!(sent(&s), Vec::<String>::new());
}

/// After a close, which clears only the master's guid (`0x4cae10`), `StablePet` and
/// `BuyStableSlot` send nothing (`0x4cae56`, `0x4cb0ae`), while `UnstablePet` and a drop send with
/// guid 0, testing no master (`0x4cb001`, `0x4cb693`).
#[test]
fn unstable_and_drops_send_with_no_master_stored() {
    let mut s = client(0);
    s.app.world_mut().resource_mut::<super::StableOpen>().npc = None;
    run(
        &mut s,
        "StablePet() BuyStableSlot() UnstablePet(1) PickupStablePet(1) ClickStablePet(0)",
    );
    let unstable = ClientCommand::UnstablePet {
        npc: 0,
        pet_number: 3,
    };
    assert_eq!(sent(&s), lines(&[unstable.clone(), unstable]));
}
