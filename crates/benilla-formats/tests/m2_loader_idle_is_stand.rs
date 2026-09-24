//! The loader's idle is animation id 0 through the model's `playableAnimationLookup` (`0x71019b`),
//! not its first sequence. `DuelingFlag.m2` authors Spawn (145), Stand, Despawn (157), and models
//! the flag in the air (bind z 8.9 to 14.7): Stand's bone 0 track plants it.

use benilla_formats::{open_chain, parse_m2_animations, parse_m2_playable_animation_lookup};

const DUEL_FLAG: &str = "World\\Generic\\PassiveDoodads\\DuelingFlag\\DuelingFlag.m2";
const STAND: u16 = 0;
const SPAWN: u16 = 145;
/// Bone 0's planted z, which both keys bracketing the Stand band carry.
const PLANTED_Z: f32 = -9.124369;

#[test]
fn the_duel_flag_idle_resolves_to_stand_and_sits_planted() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file(DUEL_FLAG)
        .expect("DuelingFlag.m2 in the chain");
    let anims = parse_m2_animations(&bytes);

    assert_eq!(
        anims.first().map(|a| a.anim_id),
        Some(SPAWN),
        "the model's FIRST sequence is Spawn, not Stand — the whole point of this model"
    );

    let playable = parse_m2_playable_animation_lookup(&bytes).expect("playable lookup");
    let idle_id = playable.first().map_or(0, |p| p.resolved_id);
    assert_eq!(
        idle_id, STAND,
        "playableAnimationLookup[0] resolves to Stand"
    );

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

    // Stand's authored box, the idle's mouseover pick volume, is the planted flag, ground to tip.
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
