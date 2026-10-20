use benilla_protocol::field::{
    FIELD_UNIT_CHANNEL_SPELL, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH,
    FIELD_UNIT_MAXHEALTH, FIELD_UNIT_MOUNTDISPLAYID,
};
use benilla_protocol::ObjectFields;
use bevy::prelude::*;

use super::super::{UNIT_FLAG_STUNNED, UNIT_FLAG_TAXI_FLIGHT};
use super::{
    local_apply, stand_gates, stop_attack_for_stand_state, toggle_refused, LocalApply, ToggleGates,
};
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
