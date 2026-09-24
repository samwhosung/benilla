//! UBRS's Rookery Eggs "don't play their animation when you walk close" (bug B140, decision 1404).
//!
//! The egg is `gameobject_template` 175124, a **TRAP** with radius 3 and one charge: walk inside it
//! and vmangos spends the charge, casts the whelp-spawner, and — because the spawn rows carry
//! `animprogress = 100` — sends `SMSG_GAMEOBJECT_DESPAWN_ANIM` (`GameObject.cpp:654-656` →
//! `WorldObject::SendObjectDeSpawnAnim`) immediately followed by `SMSG_DESTROY_OBJECT`. The real
//! client arms substate 12 off that packet (code table `0x80b0e0[6]` = 12, LUT `0x8607e4[12]` =
//! **157 Despawn**) and *pins* the object (`0x4683e0`) so the destroy waits for the play.
//!
//! This file pins the two asset facts that whole law rests on, because both are the kind of thing a
//! plausible reading gets wrong:
//!
//! 1. **The hatch is 157 Despawn, and it is unreachable from the `SetGoState` state machine
//!    (`0x5f8bd0`).** The model
//!    authors the door family (146..149) too — so a reader that only ever consults
//!    `GAMEOBJECT_STATE` finds sequences, arms one, and looks correct while never playing the
//!    animation anyone reported. Substate 1 (`state = 1`, `animprogress = 100`) resolves to 147
//!    Closed, a 0.333 s near-static hold; the hatch is eight times longer and lives on the other,
//!    disjoint arm channel.
//! 2. **The window the pin has to outlast is real time.** 157 is ~2.667 s — the entire visible
//!    event happens *after* the server has told the client the object is gone, which is why a
//!    client without the pin shows nothing at all rather than something clipped.
//!
//! Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_animation_lookup, parse_m2_animations};

/// `GameObjectDisplayInfo.dbc` row **3891** (the Rookery Egg's `displayId`) → this model.
const EGG: &str = "World\\Goober\\G_DragonEggFreeze.m2";
/// `AnimationData.dbc` 157 Despawn — the one-shot channel's code 6.
const DESPAWN: u16 = 157;
/// 147 Closed — what substate 1 (the spawn state) holds instead.
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

    // 1. The model OWNS 157 — slot 15 pre-gates the one-shot channel on exactly this
    //    (`0x5f423d`/`0x711960`), and takes no `0x80b0e0`/`0x8607e4` remap, so an unowned id
    //    would simply play nothing. This is the check that says the packet has somewhere to land.
    let lookup = parse_m2_animation_lookup(&bytes).expect("animation lookup");
    assert!(
        lookup.get(DESPAWN as usize).is_some_and(|&s| s != 0xffff),
        "the egg owns 157 Despawn"
    );

    // 2. The hatch's window — what the destroy pin has to hold the object alive for. Half a
    //    second of slack either side: the assertion is "seconds, not frames", not a float pin.
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

    // 3. …and it is where the CONTENT is. The state family's rest pose is a near-static hold; the
    //    hatch moves an order of magnitude more. A client that only reads `GAMEOBJECT_STATE` arms
    //    the left-hand column for ever and renders an egg that never hatches — the report.
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

    // 4. One pass. `R = max(1, min + roll)` with an empty replay range rolls to 1 (`0x7126be`),
    //    so the completion — and the pin's release — lands at one band length, which is what
    //    `release_despawn_pin` models.
    assert_eq!(
        (hatch.min_replay, hatch.max_replay),
        (0, 0),
        "R = 1: the pin drops after exactly one band"
    );
}
