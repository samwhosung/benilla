use benilla_protocol::field::{
    FIELD_UNIT_CHANNEL_SPELL, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH,
    FIELD_UNIT_MAXHEALTH, FIELD_UNIT_MOUNTDISPLAYID,
};
use benilla_protocol::ObjectFields;
use bevy::prelude::*;

use super::super::{UNIT_FLAG_STUNNED, UNIT_FLAG_TAXI_FLIGHT};
use super::{
    local_apply, stand_gates, stop_attack_for_stand_state, toggle_refused, toggle_worn, LocalApply,
    ToggleGates,
};
use crate::creature_anim::{toggle_sheath_next, Wielded};
use crate::net::{ClientCommand, NetCommands};

/// A living, standing player's descriptor with `extra` fields on top.
fn alive(extra: &[(u16, u32)]) -> ObjectFields {
    let mut pairs = vec![(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)];
    pairs.extend_from_slice(extra);
    ObjectFields::from_pairs(&pairs)
}

#[test]
fn the_local_apply_writes_the_prediction_only_on_a_change() {
    // Seated by the server (a drink) while the echo still says standing: predicted.
    assert!(local_apply(1, 0, false).predict);
    // vmangos's same-state re-send (its camera re-acquire): nothing written (`0x5f0799`).
    assert!(!local_apply(0, 0, false).predict);
    // A stand from the server while we predicted a sit: the newer state wins.
    assert!(local_apply(0, 1, false).predict);
}

/// `0x5f0790` past its early-out: a change to any state but 0 ends the loot window
/// (`0x5f07d5`) and the melee attack (`0x5f07e7`); a stand, or a re-apply of the state already
/// committed, does neither. The stow (`0x6127cc`) is apart: every call, any state but 0 and 2.
#[test]
fn sitting_ends_the_loot_and_the_attack_and_standing_does_not() {
    assert_eq!(
        local_apply(1, 0, true),
        LocalApply {
            predict: true,
            leave: true,
            stow: true,
        },
        "a sit from standing, weapons drawn"
    );
    assert_eq!(
        local_apply(2, 0, true),
        LocalApply {
            predict: true,
            leave: true,
            stow: false,
        },
        "the chair sit ends them too, and keeps the weapons out"
    );
    assert!(local_apply(8, 1, false).leave, "kneeling from a sit");
    assert_eq!(
        local_apply(0, 1, true),
        LocalApply {
            predict: true,
            leave: false,
            stow: false,
        },
        "standing up ends nothing"
    );
    assert_eq!(
        local_apply(1, 1, true),
        LocalApply {
            predict: false,
            leave: false,
            stow: true,
        },
        "the echo of a committed sit only re-stows"
    );
}

/// The stand gates' descriptor reads: the mount display, the taxi bit of `UNIT_FIELD_FLAGS`,
/// health 0 or the feign flag, and the channel field.
#[test]
fn the_stand_gates_read_the_mount_the_taxi_flag_death_and_the_channel() {
    let open = stand_gates(&alive(&[]), false);
    assert_eq!(open, Default::default(), "a living player on foot");
    assert!(stand_gates(&alive(&[(FIELD_UNIT_MOUNTDISPLAYID, 14_337)]), false).mounted);
    assert!(stand_gates(&alive(&[(FIELD_UNIT_FLAGS, UNIT_FLAG_TAXI_FLIGHT)]), false).on_taxi);
    assert!(stand_gates(&alive(&[(FIELD_UNIT_HEALTH, 0)]), false).reads_dead);
    assert!(stand_gates(&alive(&[(FIELD_UNIT_DYNAMIC_FLAGS, 0x20)]), false).reads_dead);
    assert!(stand_gates(&alive(&[(FIELD_UNIT_CHANNEL_SPELL, 689)]), false).channelling);
    assert!(stand_gates(&alive(&[]), true).precast);
}

/// `ToggleSheath`'s player guards (`0x5eb4d1`-`0x5eb553`, then `0x5eb5a0`-`0x5eb5e2`), each alone
/// on a living player on foot: every one refuses the press.
#[test]
fn the_toggle_refuses_while_stunned_channelling_casting_or_out_of_control() {
    let open = alive(&[]);
    assert!(
        !toggle_refused(Some(&open), ToggleGates::default()),
        "a living player on foot toggles"
    );
    let gates = |f: fn(&mut ToggleGates)| {
        let mut g = ToggleGates::default();
        f(&mut g);
        g
    };
    for (what, fields, g) in [
        (
            "out of control (`0x5eb4d1`)",
            open.clone(),
            gates(|g| g.control_lost = true),
        ),
        (
            "mounted (`0x5eb4e4`)",
            alive(&[(FIELD_UNIT_MOUNTDISPLAYID, 14_337)]),
            ToggleGates::default(),
        ),
        (
            "stunned (`0x5eb4f2`)",
            alive(&[(FIELD_UNIT_FLAGS, UNIT_FLAG_STUNNED)]),
            ToggleGates::default(),
        ),
        (
            "dead (`0x5eb502`)",
            alive(&[(FIELD_UNIT_HEALTH, 0)]),
            ToggleGates::default(),
        ),
        (
            "attacking (`0x5eb513`)",
            open.clone(),
            gates(|g| g.engaged = true),
        ),
        (
            "channelling (`0x5eb48e`)",
            alive(&[(FIELD_UNIT_CHANNEL_SPELL, 689)]),
            ToggleGates::default(),
        ),
        (
            "casting (`0x5eb49f`, `0x5eb52b`)",
            open.clone(),
            gates(|g| g.precast = true),
        ),
        (
            "seated (`0x5eb553`)",
            open.clone(),
            gates(|g| g.seated = true),
        ),
        (
            "mid-ceremony (`0x5eb5a0`)",
            open.clone(),
            gates(|g| g.mid_ceremony = true),
        ),
    ] {
        assert!(toggle_refused(Some(&fields), g), "refused while {what}");
    }
}

/// The local apply's StopAttack (`0x5ecac0`): `CMSG_ATTACKSTOP` while our attack target is set,
/// nothing without one.
#[test]
fn the_stand_state_stop_sends_the_attack_stop_only_while_attacking() {
    let mut world = World::new();
    let (tx, rx) = crossbeam_channel::unbounded();
    world.insert_resource(NetCommands(tx));
    world.init_resource::<crate::spell::PendingCast>();
    world.init_resource::<crate::spell::QueuedMeleeSpell>();
    world.init_resource::<crate::spell::SpellTargeting>();
    world.init_resource::<crate::ui_cast::CastReports>();
    let me = world.spawn(crate::net::SelfPlayer).id();
    let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        rx.try_iter().map(|c| format!("{c:?}")).collect::<Vec<_>>()
    };

    world
        .run_system_cached(stop_attack_for_stand_state)
        .unwrap();
    assert!(sent(&rx).is_empty(), "nothing to stop");

    world
        .entity_mut(me)
        .insert(crate::creature_anim::Engaged(0xf130_0000_0000_0001));
    world
        .run_system_cached(stop_attack_for_stand_state)
        .unwrap();
    assert_eq!(sent(&rx), ["AttackStop"], "the attack ends");
}

/// The local apply's leave, wired (`0x5f07c4`-`0x5f07e7`): a change to a state but 0 closes the
/// loot (`0x48f200` at `0x5f07d5`) and stops the attack (`0x5ecac0` at `0x5f07e7`); a stand does
/// neither.
#[test]
fn a_sit_closes_the_loot_and_stops_the_attack_and_a_stand_does_neither() {
    use bevy::ecs::message::Messages;
    use bevy::ecs::system::RunSystemOnce;

    let mut world = World::new();
    let (tx, rx) = crossbeam_channel::unbounded();
    world.insert_resource(NetCommands(tx));
    world.init_resource::<crate::spell::PendingCast>();
    world.init_resource::<crate::spell::QueuedMeleeSpell>();
    world.init_resource::<crate::spell::SpellTargeting>();
    world.init_resource::<crate::ui_cast::CastReports>();
    world.init_resource::<super::Player>();
    world.init_resource::<crate::ui_loot::LootMoveStart>();
    world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
    world.spawn((
        crate::net::SelfPlayer,
        crate::creature_anim::Engaged(0xf130_0000_0000_0001),
    ));
    let mut apply = |s: u8, stand_now: u8| {
        world
            .run_system_once(
                move |mut player: ResMut<super::Player>,
                      body: super::BodyQuery,
                      mut commands: Commands,
                      mut sheath: MessageWriter<crate::creature_anim::SheathRequest>,
                      mut loot: ResMut<crate::ui_loot::LootMoveStart>| {
                    super::apply_locally(
                        &mut player,
                        s,
                        stand_now,
                        &body,
                        &mut commands,
                        &mut sheath,
                        &mut loot,
                    );
                },
            )
            .unwrap();
        let closed = std::mem::take(&mut world.resource_mut::<crate::ui_loot::LootMoveStart>().0);
        let sent: Vec<String> = rx.try_iter().map(|c| format!("{c:?}")).collect();
        (closed, sent)
    };
    assert_eq!(
        apply(1, 0),
        (true, vec!["AttackStop".to_string()]),
        "the sit closes the loot and ends the attack"
    );
    assert_eq!(apply(0, 1), (false, vec![]), "the stand does neither");
}

/// The toggle's class test (`0x5eb616`-`0x5eb640`): a Paladin's libram, worn in slot 17 as vmangos
/// shows it, is no ranged candidate, so Z from drawn stows; a Warrior's bow is, so Z reaches for it.
#[test]
fn a_paladin_with_a_libram_goes_from_drawn_to_stowed() {
    let classes = benilla_formats::ChrClasses::from_rows([
        (1, "Warrior", "WARRIOR"),
        (2, "Paladin", "PALADIN"),
    ])
    .with_relic_slot(2);
    // `UNIT_FIELD_BYTES_0` (36) byte 1, the class the test reads at `+0x79`.
    let of_class = |class: u32| alive(&[(36, class << 8)]);
    let sword = Wielded {
        main: Some((2, 7)),
        ..Default::default()
    };
    let with_libram = Wielded {
        ranged: Some((4, 7)), // armour, Libram
        ..sword
    };
    let with_bow = Wielded {
        ranged: Some((2, 2)),
        ..sword
    };

    let paladin = toggle_worn(&with_libram, Some(&of_class(2)), Some(&classes));
    assert_eq!(paladin, (true, false), "the libram is no ranged candidate");
    assert_eq!(
        toggle_sheath_next(1, paladin),
        Some(0),
        "drawn, then stowed"
    );
    let libram_only = Wielded {
        main: None,
        ..with_libram
    };
    assert_eq!(
        toggle_sheath_next(
            0,
            toggle_worn(&libram_only, Some(&of_class(2)), Some(&classes))
        ),
        None,
        "a libram alone draws nothing (`0x5eb697`)"
    );

    let warrior = toggle_worn(&with_bow, Some(&of_class(1)), Some(&classes));
    assert_eq!(
        toggle_sheath_next(1, warrior),
        Some(2),
        "a Warrior reaches for the bow"
    );
    assert_eq!(
        toggle_worn(&with_bow, Some(&of_class(6)), Some(&classes)),
        (true, false),
        "a class with no row fails the test too"
    );
    assert_eq!(
        toggle_worn(&with_bow, Some(&of_class(1)), None),
        (true, true),
        "without the table the bow is kept"
    );
}
