//! The stable verbs through the whole client's host: each sends its request inside its call, as
//! the reference's bindings do (`0x4cae50`, `0x4caf20`, `0x4cb080`, `0x4cb420`).

use benilla_protocol::messages::ObjectType;
use benilla_protocol::ObjectFields;
use benilla_ui::script::{StablePetSlot, StableState, UiScript};
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
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let me = ObjectFields::from_pairs(&[
        (2, 0x19),
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
            slot: 1,
        }],
    );
    let mut script = UiScript::new().expect("a VM");
    script.set_money(10_000);
    script.set_stable(Some(StableState {
        num_stable_slots: 1,
        next_slot_cost: 500,
        slots: [
            None,
            Some(StablePetSlot {
                pet_number: 3,
                icon: Some("Interface\\Icons\\Ability_Hunter_Pet_Boar".into()),
                name: "Boar".into(),
                level: 10,
                ..Default::default()
            }),
            None,
        ],
        has_live_pet: false,
    }));
    app.world_mut().insert_non_send_resource(script);
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
