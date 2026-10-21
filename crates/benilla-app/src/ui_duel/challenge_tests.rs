//! `StartDuelUnit` on the whole client headless: the duel cast at whatever unit the token names,
//! through the cast path.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use benilla_ui::script::{UiScript, UnitGuids};

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::target::Selection;

const ME: u64 = 0x10;
/// Our one party member, a player in range.
const MEMBER: u64 = 0x51;
const CREATURE: u64 = 0xF130_0000_0000_0031;
/// 7266 "Duel": `Effect[0]` 0x53 and `AttributesEx` 0x80000, a spell its caster cannot take.
const DUEL: u32 = 7266;

/// The whole client with us, a player party member and a creature 5 yards off, the duel spell
/// known, and a VM whose resolver names the member as `party1`.
fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let unit = |guid: u64, kind: u32, x: f32| {
        (
            Guid(guid),
            Transform::from_xyz(x, 0.0, 0.0),
            GlobalTransform::from_translation(Vec3::new(x, 0.0, 0.0)),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, kind),
                (22, 100),
                (28, 100),
            ])),
        )
    };
    let me = world.spawn((SelfPlayer, unit(ME, 0x19, 0.0))).id();
    let member = world.spawn(unit(MEMBER, 0x19, 5.0)).id();
    let creature = world.spawn(unit(CREATURE, 0x9, 5.0)).id();
    world
        .resource_mut::<GuidIndex>()
        .0
        .extend([(ME, me), (MEMBER, member), (CREATURE, creature)]);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog = benilla_formats::SpellCatalog::from_displays(
        [(
            DUEL,
            benilla_formats::SpellDisplay {
                effects: [83, 0, 0],
                attributes_ex: 0x0008_0000,
                targets: 0x2,
                range_index: 5,
                ..Default::default()
            },
        )]
        .into(),
    );
    spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(
        [(
            5,
            benilla_formats::SpellRange {
                min: 0.0,
                max: 10.0,
                flags: 0,
            },
        )]
        .into(),
    );
    world.insert_resource(spells);
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(DUEL);
    let mut script = UiScript::new().expect("a VM");
    script.set_unit_guids(&UnitGuids {
        player: ME,
        party: [MEMBER, 0, 0, 0],
        ..Default::default()
    });
    world.insert_non_send_resource(script);
    (app, rx)
}

/// Select `guid` with its object, the app's and the VM's views both.
fn select(app: &mut App, guid: u64) {
    let world = app.world_mut();
    let target = world.resource::<GuidIndex>().0.get(&guid).copied();
    *world.resource_mut::<Selection>() = Selection {
        target,
        guid: Some(guid),
        ..Default::default()
    };
    let mut script = world.non_send_resource_mut::<UiScript>();
    let mut guids = UnitGuids {
        player: ME,
        target: guid,
        party: [MEMBER, 0, 0, 0],
        ..Default::default()
    };
    guids.held.extend([(ME, 0), (MEMBER, 0), (CREATURE, 0)]);
    script.set_unit_guids(&guids);
}

/// `lua`, then the duel drain; the duel casts it sent, by target.
fn challenge(app: &mut App, rx: &Receiver<ClientCommand>, lua: &str) -> Vec<Option<u64>> {
    let world = app.world_mut();
    world
        .non_send_resource_mut::<UiScript>()
        .run(lua)
        .expect("the challenge runs");
    world
        .run_system_once(super::drain_duel)
        .expect("the duel drain runs");
    rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell { spell_id, target } if spell_id == DUEL => Some(target),
            _ => None,
        })
        .collect()
}

fn cursor_is_up(app: &App) -> bool {
    app.world()
        .resource::<crate::spell::SpellTargeting>()
        .active()
}

/// `StartDuelUnit` resolves any token (`0x4d4caf`) and casts the duel spell at the guid through
/// the cast attempt (`0x4d4cbc` → `0x4d4810` → `0x6e5a90`): a party member's token names them, a
/// creature is cast at as any unit is and left to the server, and ourselves, whom the spell's
/// `AttributesEx` 0x80000 keeps the bind from taking (`0x6e5bd8`), waits at the cursor.
#[test]
fn start_duel_unit_casts_at_whatever_unit_the_token_names() {
    let (mut app, rx) = client();
    assert_eq!(
        challenge(&mut app, &rx, r#"StartDuelUnit("party1")"#),
        vec![Some(MEMBER)],
        "a party member"
    );

    let (mut app, rx) = client();
    select(&mut app, CREATURE);
    assert_eq!(
        challenge(&mut app, &rx, r#"StartDuelUnit("target")"#),
        vec![Some(CREATURE)],
        "a creature"
    );

    let (mut app, rx) = client();
    assert_eq!(
        challenge(&mut app, &rx, r#"StartDuelUnit("player")"#),
        Vec::<Option<u64>>::new(),
        "ourselves"
    );
    assert!(cursor_is_up(&app), "the word waits at the cursor");
}
