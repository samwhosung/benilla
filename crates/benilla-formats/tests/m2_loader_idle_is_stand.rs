//! The loader-idle seed is **Stand**, not the file-order-first sequence (the floating duel-flag
//! bug, 2026-07-25 — decision 0637).
//!
//! `DuelingFlag.m2` is the model that separates the two readings. Its sequence table is authored
//! **Spawn(145) / Stand(0) / Despawn(157)** — file order 0 is the *Spawn*, not the idle — and its
//! geometry is modelled in the air: the bind pose sits at z ≈ +8.9…+14.7, and bone 0's translation
//! track is what plants it (`t=700 → −9.124`, held to `t=5333`). So arming file-order-0 and looping
//! it flies the flag 9 yards up every 3.3 s, while arming Stand holds it planted — which is what
//! the reference does (`0x71019b`: the loader arms animation id 0 resolved through the model's
//! own `playableAnimationLookup`).
//!
//! This test pins the three facts the fix rests on, straight off the shipped file. Skips (passes)
//! when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_animations, parse_m2_playable_animation_lookup};

const DUEL_FLAG: &str = "World\\Generic\\PassiveDoodads\\DuelingFlag\\DuelingFlag.m2";
/// `AnimationData.dbc`: 0 = Stand, 145 = Spawn, 157 = Despawn.
const STAND: u16 = 0;
const SPAWN: u16 = 145;
/// Bone 0's planted height (raw WoW z, the value both keys bracketing the Stand band carry).
const PLANTED_Z: f32 = -9.124369;

#[test]
fn the_duel_flag_idle_resolves_to_stand_and_sits_planted() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file(DUEL_FLAG)
        .expect("DuelingFlag.m2 in the chain");
    let anims = parse_m2_animations(&bytes);

    // 1. File order really does lead with Spawn — without this the test proves nothing.
    assert_eq!(
        anims.first().map(|a| a.anim_id),
        Some(SPAWN),
        "the model's FIRST sequence is Spawn, not Stand — the whole point of this model"
    );

    // 2. The loader's seed resolves to Stand through the model's own table.
    let playable = parse_m2_playable_animation_lookup(&bytes).expect("playable lookup");
    let idle_id = playable.first().map_or(0, |p| p.resolved_id);
    assert_eq!(
        idle_id, STAND,
        "playableAnimationLookup[0] resolves to Stand"
    );

    // 3. Stand holds the flag IN THE GROUND: bone 0's translation over the Stand band is the
    //    constant −9.124 (both bracketing keys carry it), not the bind pose it would sit at with
    //    no clip. This is the assertion that actually catches the bug coming back.
    let stand = anims
        .iter()
        .find(|a| a.anim_id == STAND)
        .expect("DuelingFlag authors a Stand sequence");
    let root = stand
        .bones
        .iter()
        .find(|b| b.bone == 0)
        .expect("Stand keys bone 0");
    assert!(
        !root.translation.is_empty(),
        "Stand's band has no interior keys, so it must be bracketed to the authored pose — an \
         empty channel here means the flag renders at bind pose, 9 yards in the air"
    );
    for (_, v) in &root.translation {
        assert!(
            (v[2] - PLANTED_Z).abs() < 1e-3,
            "bone 0 z should be the planted {PLANTED_Z}, got {}",
            v[2]
        );
    }

    // 4. Stand's AUTHORED bounds describe the planted flag — ground to tip — and emphatically NOT
    //    the bind pose (z +8.9..+14.7). This is the mouseover picker's volume for the armed idle
    //    (entity M2 parts carry `NoFrustumCulling` — the view cull is the body ROOT's per-object
    //    election, never a per-part box; the "≈1e7 entity render bounds" reading behind 0648
    //    misread the node's position cache (`0x670db0`) as bounds — decision 1473).
    //    The bind-pose `Aabb` Bevy would derive sits a whole model-height ABOVE the geometry that
    //    draws — as a cull box it hid the planted flag from every ground-level camera, and as a
    //    pick box it would put the hover target in the sky.
    assert!(
        stand.bounds_min[2] > -1.0 && stand.bounds_min[2] < 0.5,
        "Stand's authored min z should sit at the ground, got {}",
        stand.bounds_min[2]
    );
    assert!(
        stand.bounds_max[2] > 4.0 && stand.bounds_max[2] < 8.0,
        "Stand's authored max z should be the flag's tip a few yards up, got {}",
        stand.bounds_max[2]
    );
    assert!(
        stand.bounds_min[2] < 8.9,
        "the authored box must NOT be the bind pose — that box floats a model-height in the air"
    );
}
