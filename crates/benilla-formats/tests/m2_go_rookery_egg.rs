//! The Rookery Egg (`gameobject_template` 175124) is a one-charge trap: entering its 3-yd radius
//! makes vmangos send `SMSG_GAMEOBJECT_DESPAWN_ANIM`, since its spawn rows carry
//! `animprogress = 100`, then `SMSG_DESTROY_OBJECT` (`GameObject.cpp:654-656`). The reference
//! arms substate 12 off it (`0x80b0e0[6]` = 12, `0x8607e4[12]` = 157 Despawn) and pins the object
//! (`0x4683e0`) so the destroy waits for the 2.667 s hatch, which the `SetGoState` machine
//! (`0x5f8bd0`) never reaches: it holds 147 Closed.

use benilla_formats::{open_chain, parse_m2_animation_lookup, parse_m2_animations};

/// `GameObjectDisplayInfo` 3891, the egg's display.
const EGG: &str = "World\\Goober\\G_DragonEggFreeze.m2";
/// Despawn, the one-shot channel's code 6.
const DESPAWN: u16 = 157;
/// Closed, what substate 1 (the spawn state) holds.
const CLOSED: u16 = 147;

#[test]
fn the_rookery_egg_keys_its_hatch_in_despawn_not_in_the_state_family() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain.read_file(EGG).expect("the egg M2 is in the chain");
    let anims = parse_m2_animations(&bytes);
    let seq = |id: u16| {
        anims
            .iter()
            .find(|a| a.anim_id == id)
            .unwrap_or_else(|| panic!("the egg authors animation id {id}"))
    };

    // The one-shot channel plays only an id the model owns, with no remap (`0x5f423d`, `0x711960`).
    let lookup = parse_m2_animation_lookup(&bytes).expect("animation lookup");
    assert!(
        lookup.get(DESPAWN as usize).is_some_and(|&s| s != 0xffff),
        "the egg owns 157 Despawn"
    );

    let hatch = seq(DESPAWN);
    assert!(
        (2.0..3.5).contains(&hatch.duration),
        "157 Despawn is a multi-second play (got {}s) — the whole of what B140 reported missing",
        hatch.duration
    );
    assert!(
        !hatch.looping,
        "it is a clamp band: one window, then the object goes"
    );

    let keys = |id: u16| -> usize {
        seq(id)
            .bones
            .iter()
            .map(|b| b.translation.len() + b.rotation.len() + b.scale.len())
            .sum()
    };
    assert!(
        keys(DESPAWN) > 4 * keys(CLOSED),
        "the hatch ({} keys) dwarfs the state pose ({} keys) it is reachable past",
        keys(DESPAWN),
        keys(CLOSED),
    );

    // `R = max(1, min + roll)`, and an empty replay range rolls 1 (`0x7126be`).
    assert_eq!(
        (hatch.min_replay, hatch.max_replay),
        (0, 0),
        "R = 1: the pin drops after exactly one band"
    );
}
