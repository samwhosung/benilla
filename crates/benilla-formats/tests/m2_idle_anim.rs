//! Animation-keyframe regression test against real vanilla creatures.
//!
//! Guards the bug that froze most creatures in Milestone B: a per-sequence keyframe window pulled in a
//! keyframe from a *later* sequence (14–64 s away), which inflated the Bevy clip's duration and made
//! the whole animation crawl. We now select keys by absolute timestamp within each sequence's band, so
//! every keyframe of every sequence must land inside `[0, duration]`. Skips when the client isn't present.

use benilla_formats::{open_chain, parse_m2_animations, ModelAnimation};

/// A spread of vanilla creatures: simple (rabbit/chicken) through rigged humanoid-ish (kobold/murloc),
/// covering the ones that animated *and* the ones that froze before the fix.
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
            continue; // a model not in this install — skip
        };
        let anims = parse_m2_animations(&bytes);
        assert!(
            !anims.is_empty(),
            "{path}: a creature should have sequences"
        );
        // Every creature must carry a Stand (AnimationData id 0) somewhere — the records are NOT
        // ordered by id (e.g. Rabbit's record 0 is Walk), so the idle is found by id, not position.
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
            // No keyframe may sit past its sequence: a key beyond `duration` is the cross-sequence leak
            // that froze creatures (it stretched a clip to many seconds). Small epsilon for rounding.
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

/// The latest keyframe time across every bone/channel of the animation (0.0 if it has no keys).
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
