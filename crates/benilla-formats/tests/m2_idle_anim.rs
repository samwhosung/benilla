//! Keys are selected by absolute timestamp within each sequence's band, so every key of every
//! sequence lands inside `[0, duration]`.

use benilla_formats::{open_chain, parse_m2_animations, ModelAnimation};

/// Simple rigs (rabbit, chicken) through humanoid ones (kobold, murloc).
const CREATURES: &[&str] = &[
    "Creature\\Rabbit\\Rabbit.m2",
    "Creature\\Chicken\\Chicken.m2",
    "Creature\\Deer\\Deer.m2",
    "Creature\\Kobold\\Kobold.m2",
    "Creature\\Wolf\\Wolf.m2",
    "Creature\\Murloc\\Murloc.m2",
    "Creature\\Bear\\Bear.m2",
    "Creature\\Cat\\Cat.m2",
];

#[test]
fn creature_animation_keyframes_stay_within_their_sequence() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    let mut checked = 0;
    for path in CREATURES {
        let Ok(bytes) = chain.read_file(path) else {
            continue; // not in this install
        };
        let anims = parse_m2_animations(&bytes);
        assert!(
            !anims.is_empty(),
            "{path}: a creature should have sequences"
        );
        // Records are not ordered by id (Rabbit's record 0 is Walk): the idle is found by id.
        let ids: Vec<u16> = anims.iter().map(|a| a.anim_id).collect();
        assert!(
            ids.contains(&0),
            "{path}: no Stand (id 0) among sequence ids {ids:?}"
        );
        for anim in &anims {
            assert!(
                anim.duration > 0.0,
                "{path}: anim {} has no duration",
                anim.anim_id
            );
            let max_t = max_key_time(anim);
            assert!(
                max_t <= anim.duration + 1e-3,
                "{path}: anim {} key at {max_t:.3}s exceeds duration {:.3}s — a cross-sequence leak",
                anim.anim_id,
                anim.duration
            );
        }
        checked += 1;
    }
    assert!(
        checked > 0,
        "no test creatures present — expected at least the rabbit"
    );
}

fn max_key_time(anim: &ModelAnimation) -> f32 {
    let mut max = 0.0_f32;
    for b in &anim.bones {
        for (t, _) in &b.translation {
            max = max.max(*t);
        }
        for (t, _) in &b.rotation {
            max = max.max(*t);
        }
        for (t, _) in &b.scale {
            max = max.max(*t);
        }
    }
    max
}
