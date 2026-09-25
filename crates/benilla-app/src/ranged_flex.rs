//! The ranged weapon prop's own animation: a bow's limbs bend, a gun fires its muzzle blast.
//!
//! The reference keeps the equipped ranged weapon's M2 instance at `[CGUnit+0xd24]` (written by
//! `0x611e10` at `0x611eca` on any sheath to 2) and arms that model's own animation from two keys
//! on the body clip:
//!
//! - `$BWP` (LoadBow 105 at 0.434 s, LoadRifle 106 at 0.433 s) goes to `0x624cc0`: if the prop
//!   owns 160 BowPull (`0x624cee push 0xa0; call 0x711960`, a presence test it bails on), play it
//!   at a rate that fits its length into the body clip's remaining time (`0x624e31`).
//! - `$BWR` (AttackBow 46 at 0.033 s, AttackRifle 49 at 0.000 s) goes to `0x600159`, which forks on
//!   the body clip's family: a bow clip (`0x5fcfb0`: 46, 105, 109) arms 0 Stand (`0x600209`), the
//!   drawn limbs relaxing; a rifle clip (`0x5fcfd0`: 49, 106, 110, guns and crossbows) arms 161
//!   BowRelease (`0x600273`, `push 0xa1`).
//!
//! The rifle arm is the gun's muzzle blast. A firearm M2 authors only Stand and BowRelease:
//! `Firearm_2H_Rifle_A_01.m2`'s seven emitters hold a flat rate in every sequence and key only
//! their enabled gate (the `u8` step track at `def+0x1dc`), shut across Stand and open from the
//! head of 161 for 34 ms to 334 ms each. Bone 2, parent of all seven, is keyed +90° about Y in 161,
//! aiming the blast down the barrel. 21 of the 22 firearms author 161, 25 bows and crossbows author
//! 160 and 161, and no other weapon model authors either.
//!
//! Only four sites arm `[+0xd24]`: those three and the un-nock reset (`0x60f59d`). Thrown and wand
//! (`0x5fcf90`: 107, 111, 112) match neither `$BWR` arm, so they get no prop animation.
//!
//! Both `$BWR` arms are gated by `[CGUnit+0xac]`, the queue of `CMissile` nodes awaiting release
//! ([`crate::entities::PendingMissiles`]): `$BWR` reads it (`0x600182`) before draining it
//! (`0x600294` into `0x60c940`, the launcher), so no queued missile means no flex, and the flex
//! runs ahead of the drain.

use bevy::prelude::*;

use benilla_assets::ModelAnimations;

use crate::creature_anim::AnimSoundEvent;

/// `AnimationData.dbc` 160 BowPull, the prop clip `$BWP` arms (`0x624e2a push 0xa0`).
pub(crate) const BOW_PULL: u16 = 160;
/// `AnimationData.dbc` 161 BowRelease, the prop clip the rifle arm of `$BWR` plays (`0x60026c push
/// 0xa1`): a firearm's muzzle blast. A bow authors it but never reaches it from here.
pub(crate) const BOW_RELEASE: u16 = 161;
/// Animation 0 Stand, the model-load default: what the bow arm of `$BWR` plays (`0x600205 push
/// 0x0`) and the un-nock reset re-arms (`0x60f59d`).
const STAND: u16 = 0;

/// `0x5fcfb0`: the body clips of a bow shooter.
const BOW_FAMILY: [u16; 3] = [46, 105, 109]; // AttackBow · LoadBow · HoldBow
/// `0x5fcfd0`: the body clips of a rifle shooter (guns and crossbows).
const RIFLE_FAMILY: [u16; 3] = [49, 106, 110]; // AttackRifle · LoadRifle · HoldRifle

/// The unit's equipped ranged weapon prop, benilla's `[CGUnit+0xd24]`: on the prop root when the
/// ranged display's model authors 160 or 161, absent for every other held item (nothing else
/// reaches `0x624cc0`/`0x600209`/`0x600273`).
#[derive(Component)]
pub(crate) struct RangedProp {
    /// The wearer: the `$BWP`/`$BWR` keys arrive on its timeline, not the prop's.
    pub(crate) owner: Entity,
}

/// Arms the prop's clip from the wearer's `$BWP`/`$BWR` keys. Runs right after
/// [`crate::creature_anim::drive_nock_latch`], the other half of the same reference handlers
/// (`$BWP`: prop arm, then ammo attach `0x624b2f`; `$BWR`: prop arm, then detach `0x600299`), and
/// before the rider lane composes the prop's palette rows.
pub(crate) fn flex_ranged_props(
    mut events: MessageReader<AnimSoundEvent>,
    props: Query<(Entity, &RangedProp)>,
    // Disjoint from `props_mut` by the filter: a unit is never its own ranged prop.
    wearers: Query<(&AnimationPlayer, &ModelAnimations), Without<RangedProp>>,
    mut props_mut: Query<(&mut AnimationPlayer, &ModelAnimations), With<RangedProp>>,
    // The `[+0xac]` gate, drained by `entities::missile::spawn_missiles` on the same key. The
    // creature-anim chain runs `.before(EntityVisualsSet)`, so this read precedes the drain as
    // `0x600182` precedes `0x600294`.
    pending: Res<crate::entities::PendingMissiles>,
) {
    for ev in events.read() {
        let want = match &ev.ident {
            b"$BWP" => Some(BOW_PULL),
            // `0x60018a je 0x600299`: no projectile waiting, no prop block. Only `$BWR` is gated;
            // the nock-latch clear (`0x60016c`) and the ammo detach (`0x600299`) sit outside the
            // skip, so `drive_nock_latch` stays ungated.
            b"$BWR" if !pending.releasing(ev.entity) => None,
            b"$BWR" => {
                if BOW_FAMILY.contains(&ev.anim_id) {
                    Some(STAND)
                } else if RIFLE_FAMILY.contains(&ev.anim_id) {
                    Some(BOW_RELEASE)
                } else {
                    // Neither family: the reference falls through to `0x60027a` and touches no
                    // prop. No shipped character model keys `$BWR` outside 46/49.
                    None
                }
            }
            _ => continue,
        };
        let Some(want) = want else { continue };
        // Found from the wearer, as `[+0xd24]` is: one ranged prop per unit.
        let Some((prop, _)) = props.iter().find(|(_, p)| p.owner == ev.entity) else {
            continue;
        };
        let Ok((mut player, anims)) = props_mut.get_mut(prop) else {
            continue;
        };
        // `$BWP` is guarded by `0x711960`, the direct "does the model author this id" test, and
        // bails on a miss, confining BowPull to its 25 models. The `$BWR` arms have no guard:
        // `0x7121a0` first substitutes through the model's `PlayableAnimationLookup` (`0x711bf0`),
        // and weapon models that author neither flex clip bake both ids to Stand (571 of 571
        // shipped weapon models resolve), so a rifle-family body holding a model without 161
        // re-arms Stand.
        let want = if want == BOW_PULL {
            if !anims.owns(BOW_PULL) {
                continue;
            }
            BOW_PULL
        } else {
            anims
                .playable_animation_lookup
                .get(want as usize)
                .map_or(want, |row| row.resolved_id)
        };
        let Some(clip) = anims.clips.iter().find(|c| c.anim_id == want) else {
            continue;
        };
        // `$BWP` fits BowPull into the body clip's remaining time (`0x624e31`), so the draw ends
        // with the Load clip; read as `duration − seek` off the wearer's player. Both `$BWR` arms
        // push a literal 1.0 (`0x3f800000`).
        let speed = if want == BOW_PULL {
            // No time left means no flex that shot, not a flex at 1.0 (`0x624d91 jle`).
            let Some(left) = wearers
                .get(ev.entity)
                .ok()
                .and_then(|(p, a)| remaining(p, a, ev.anim_id))
                .filter(|left| *left > 0.0)
            else {
                continue;
            };
            (clip.duration / left).clamp(0.05, 20.0)
        } else {
            1.0
        };
        // Arming replaces, it does not layer: without the stop Bevy keeps the previous node active
        // and `playing_seq`, which the emitters' rate track reads, picks between two equal weights.
        player.stop_all();
        let active = player.play(clip.node);
        active.replay();
        active.set_speed(speed);
        // `WOW_MOVE_TRACE_TAGS=flex`, on the `aev` key's clock: tells a key that never arrived, a
        // fork the other way and a missing clip apart.
        if benilla_assets::trace::enabled_for("flex") {
            benilla_assets::trace::line(
                "flex",
                &format!(
                    "{} unit={} body={} prop={prop} arm={want} speed={speed:.3}",
                    String::from_utf8_lossy(&ev.ident),
                    ev.entity,
                    ev.anim_id,
                ),
            );
        }
        // The authored flags decide the wrap: a firearm's Stand loops and its BowRelease clamps, a
        // bow's three clips all clamp. A clamped flex holds its last frame until the next key, as
        // in the reference.
        if clip.looping {
            active.repeat();
        } else {
            active.set_repeat(bevy::animation::RepeatAnimation::Never);
        }
    }
}

/// The un-nock's reset (`0x60f59d`): re-arms Stand unless the prop is releasing.
///
/// ```text
/// 60f578  push -1 ; call 0x712090   ; the prop's CURRENT requested animation id
/// 60f57f  cmp eax,0xa1              ; 161 BowRelease
/// 60f584  je 0x60f5a2               ; …then SKIP the reset
/// ```
///
/// It sits in the un-nock `0x60f530`, which `$BWR` reaches every shot (`0x600294`, `0x60c940`,
/// `0x60c951`) after its arm at `0x600273`, so the guard keeps a gun's blast alive. Here the
/// un-nock is [`NockLatch`] leaving the wearer, on every `$BWR` and every cancel path, seen a frame
/// later. Its real use is a cancel mid-draw: BowPull clamps, so this relaxes limbs at full draw.
pub(crate) fn reset_ranged_props_on_unnock(
    mut unnocked: RemovedComponents<crate::creature_anim::NockLatch>,
    props: Query<(Entity, &RangedProp)>,
    mut players: Query<(&mut AnimationPlayer, &ModelAnimations), With<RangedProp>>,
) {
    for owner in unnocked.read() {
        let Some((prop, _)) = props.iter().find(|(_, p)| p.owner == owner) else {
            continue;
        };
        let Ok((mut player, anims)) = players.get_mut(prop) else {
            continue;
        };
        // `0x60f57f cmp eax,0xa1` reads the current requested id, not clip time: a finished
        // BowRelease still reads 161 and is skipped.
        let releasing = player
            .playing_animations()
            .filter_map(|(node, _)| anims.clips.iter().find(|c| c.node == *node))
            .any(|c| c.anim_id == BOW_RELEASE);
        if releasing {
            continue;
        }
        let Some(stand) = anims.clips.iter().find(|c| c.anim_id == STAND) else {
            continue;
        };
        player.stop_all();
        let active = player.play(stand.node);
        active.replay();
        active.set_speed(1.0);
        if stand.looping {
            active.repeat();
        } else {
            active.set_repeat(bevy::animation::RepeatAnimation::Never);
        }
    }
}

/// Seconds left in the wearer's playing clip for `anim_id`, the `$BWP` rate's denominator.
fn remaining(player: &AnimationPlayer, anims: &ModelAnimations, anim_id: u16) -> Option<f32> {
    let clip = anims
        .clips
        .iter()
        .filter(|c| c.anim_id == anim_id)
        .find_map(|c| Some((c, player.animation(c.node)?)))?;
    Some(clip.0.duration - clip.1.seek_time())
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::AnimClip;
    use bevy::animation::graph::AnimationNodeIndex;

    fn clip(anim_id: u16, node: u32, duration: f32, looping: bool) -> AnimClip {
        AnimClip {
            anim_id,
            seq_index: 0,
            node: AnimationNodeIndex::new(node as usize),
            looping,
            duration,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_center: Vec3::ZERO,
            bounds_radius: 0.0,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            events: Vec::new().into(),
            arm_nodes: None,
            upper_node: None,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
        }
    }

    /// A model's animation table with both lookups a real M2 carries: `animation_lookup` (the
    /// direct `0x711960` test) and `playable_animation_lookup` (the `0x711bf0` substitution, every
    /// unauthored id resolving to Stand). With either empty, `owns()` answers `false` for an
    /// authored clip.
    fn anims(clips: Vec<AnimClip>) -> ModelAnimations {
        let top = clips.iter().map(|c| c.anim_id).max().unwrap_or(0) as usize;
        let mut animation_lookup = vec![0xffffu16; top + 1];
        let mut playable = vec![
            benilla_formats::PlayableAnim {
                resolved_id: STAND,
                dir_flags: 0,
            };
            top + 1
        ];
        for (i, c) in clips.iter().enumerate() {
            animation_lookup[c.anim_id as usize] = i as u16;
            playable[c.anim_id as usize].resolved_id = c.anim_id;
        }
        ModelAnimations {
            graph: Handle::default(),
            clips,
            hand_close: [None, None],
            playable_animation_lookup: playable,
            animation_lookup,
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    /// A firearm prop, the real `Firearm_2H_Rifle_A_01.m2` shape: Stand looping, BowRelease
    /// clamped, no BowPull.
    fn gun() -> ModelAnimations {
        anims(vec![
            clip(STAND, 1, 0.333, true),
            clip(BOW_RELEASE, 2, 2.0, false),
        ])
    }

    /// A bow prop, `Bow_1H_Standard_A_01.m2`'s shape.
    fn bow() -> ModelAnimations {
        anims(vec![
            clip(STAND, 1, 0.033, false),
            clip(BOW_PULL, 2, 1.0, false),
            clip(BOW_RELEASE, 3, 0.166, false),
        ])
    }

    /// Spawns a wearer playing `body` and its prop, runs one key, and returns the prop's playing
    /// node and speed.
    fn fire(
        prop_anims: ModelAnimations,
        body: u16,
        body_clip: Option<(AnimClip, f32)>,
        tag: &[u8; 4],
    ) -> Option<(AnimationNodeIndex, f32)> {
        let mut app = App::new();
        app.add_message::<AnimSoundEvent>();
        app.init_resource::<crate::entities::PendingMissiles>();
        app.add_systems(Update, flex_ranged_props);
        let mut wearer_player = AnimationPlayer::default();
        let wearer_anims = match &body_clip {
            Some((c, seek)) => {
                let active = wearer_player.play(c.node);
                active.seek_to(*seek);
                anims(vec![c.clone()])
            }
            None => anims(Vec::new()),
        };
        let wearer = app.world_mut().spawn((wearer_player, wearer_anims)).id();
        let prop = app
            .world_mut()
            .spawn((
                AnimationPlayer::default(),
                prop_anims,
                RangedProp { owner: wearer },
            ))
            .id();
        // A shot queued on the wearer opens the `[+0xac]` gate.
        crate::entities::PendingMissiles::queue_a_shot(&mut app, wearer);
        app.world_mut().write_message(AnimSoundEvent {
            entity: wearer,
            ident: *tag,
            data: 0,
            anim_id: body,
            pos: None,
        });
        app.update();
        let player = app.world().entity(prop).get::<AnimationPlayer>().unwrap();
        let armed = player
            .playing_animations()
            .map(|(n, a)| (*n, a.speed()))
            .next();
        armed
    }

    /// `$BWR` forks on the body's weapon family (`0x600159`): a gun plays BowRelease, a bow Stand.
    #[test]
    fn bwr_forks_the_prop_clip_on_the_bodys_weapon_family() {
        // AttackRifle(49) → 161 on the gun.
        assert_eq!(
            fire(gun(), 49, None, b"$BWR"),
            Some((AnimationNodeIndex::new(2), 1.0)),
            "a rifle-family body clip fires the prop's BowRelease at the literal 1.0"
        );
        // AttackBow(46) → Stand(0) on the bow.
        assert_eq!(
            fire(bow(), 46, None, b"$BWR").map(|(n, _)| n),
            Some(AnimationNodeIndex::new(1)),
            "a bow-family body clip relaxes the prop to Stand, NOT to its authored 161"
        );
        // The Load/Hold ids are in the same two sets (`0x5fcfb0`/`0x5fcfd0` each test three).
        assert_eq!(
            fire(gun(), 106, None, b"$BWR").map(|(n, _)| n),
            Some(AnimationNodeIndex::new(2)),
            "LoadRifle is in the rifle family"
        );
        // Neither family: the reference touches no prop at all.
        assert_eq!(
            fire(gun(), 0, None, b"$BWR"),
            None,
            "a body clip in neither family leaves the prop alone"
        );
    }

    /// No projectile queued, no arm (`0x600182`/`0x60018a`): the same key and rifle body clip, with
    /// and without a queued shot.
    #[test]
    fn a_bwr_with_no_projectile_queued_arms_nothing() {
        let mut app = App::new();
        app.add_message::<AnimSoundEvent>();
        app.init_resource::<crate::entities::PendingMissiles>();
        app.add_systems(Update, flex_ranged_props);
        let wearer = app.world_mut().spawn(anims(Vec::new())).id();
        let prop = app
            .world_mut()
            .spawn((
                AnimationPlayer::default(),
                gun(),
                RangedProp { owner: wearer },
            ))
            .id();
        let fire = |app: &mut App| {
            app.world_mut().write_message(AnimSoundEvent {
                entity: wearer,
                ident: *b"$BWR",
                data: 0,
                anim_id: 49,
                pos: None,
            });
            app.update();
            app.world()
                .entity(prop)
                .get::<AnimationPlayer>()
                .unwrap()
                .playing_animations()
                .count()
        };
        assert_eq!(fire(&mut app), 0, "queue empty ⇒ the prop is not touched");
        crate::entities::PendingMissiles::queue_a_shot(&mut app, wearer);
        assert_eq!(
            fire(&mut app),
            1,
            "…and the same key arms 161 once a shot is queued"
        );
    }

    /// `$BWP` arms BowPull only on a model that authors it (`0x624cee push 0xa0; call 0x711960`);
    /// no shipped firearm authors 160.
    #[test]
    fn bwp_pulls_only_a_prop_that_owns_bowpull() {
        // Both legs have time left in the body clip, so the firearm's `None` is the presence test.
        let load = |id: u16| Some((clip(id, 9, 1.0, false), 0.433));
        assert_eq!(
            fire(bow(), 105, load(105), b"$BWP").map(|(n, _)| n),
            Some(AnimationNodeIndex::new(2)),
            "a bow draws on the pull key"
        );
        assert_eq!(
            fire(gun(), 106, load(106), b"$BWP"),
            None,
            "a firearm owns no BowPull — the pull key arms nothing on it"
        );
        // `0x624d91 jle`: no time left in the body clip ⇒ no flex that shot, not a flex at 1.0.
        assert_eq!(
            fire(bow(), 105, Some((clip(105, 9, 1.0, false), 1.0)), b"$BWP"),
            None,
            "a pull key at the very end of its Load clip arms nothing"
        );
    }

    /// HumanMale LoadBow(105) is 1.0 s and keys `$BWP` at 0.434 s, so a 1.0 s BowPull runs at about
    /// 1.767×.
    #[test]
    fn the_pull_is_rate_matched_to_the_rest_of_the_load_clip() {
        let load = clip(105, 9, 1.0, false);
        let (node, speed) = fire(bow(), 105, Some((load, 0.434)), b"$BWP").expect("armed");
        assert_eq!(node, AnimationNodeIndex::new(2));
        assert!(
            (speed - 1.0 / 0.566).abs() < 1e-3,
            "BowPull fits the 0.566 s left of LoadBow, got {speed}"
        );
    }

    /// The un-nock re-arms Stand unless the prop is releasing (`0x60f584 je`); the other way round,
    /// a gun's blast would die the frame it starts.
    #[test]
    fn the_unnock_resets_a_drawn_prop_but_never_a_releasing_one() {
        fn unnock(prop_anims: ModelAnimations, armed: u16) -> Option<AnimationNodeIndex> {
            let mut app = App::new();
            app.add_systems(Update, reset_ranged_props_on_unnock);
            let wearer = app.world_mut().spawn(crate::creature_anim::NockLatch).id();
            let mut player = AnimationPlayer::default();
            let node = prop_anims
                .clips
                .iter()
                .find(|c| c.anim_id == armed)
                .unwrap()
                .node;
            player.play(node);
            app.world_mut()
                .spawn((player, prop_anims, RangedProp { owner: wearer }));
            // The un-nock edge: the latch leaves the wearer.
            app.world_mut()
                .entity_mut(wearer)
                .remove::<crate::creature_anim::NockLatch>();
            app.update();
            let prop = app
                .world_mut()
                .query_filtered::<Entity, With<RangedProp>>()
                .iter(app.world())
                .next()
                .unwrap();
            let player = app.world().entity(prop).get::<AnimationPlayer>().unwrap();
            let armed = player.playing_animations().map(|(n, _)| *n).next();
            armed
        }
        // A bow left at full draw by a cancel relaxes.
        let bow_stand = bow().clips[0].node;
        assert_eq!(
            unnock(bow(), BOW_PULL),
            Some(bow_stand),
            "a cancel mid-draw returns the limbs to rest"
        );
        // A gun mid-blast is not touched.
        let gun_release = gun().clips[1].node;
        assert_eq!(
            unnock(gun(), BOW_RELEASE),
            Some(gun_release),
            "the reset skips a prop that is releasing — otherwise the blast dies at birth"
        );
    }

    /// On the real chain: a firearm's muzzle blast is authored only on BowRelease(161).
    #[test]
    fn a_firearms_muzzle_blast_is_authored_only_on_bowrelease() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let path = "Item\\ObjectComponents\\Weapon\\Firearm_2H_Rifle_A_01.m2";
        let bytes = chain.read_file(path).expect("the rifle model");

        let seqs = benilla_formats::parse_m2_animations(&bytes);
        let ids: Vec<u16> = seqs.iter().map(|s| s.anim_id).collect();
        assert_eq!(
            ids,
            vec![STAND, BOW_RELEASE],
            "the rifle authors exactly Stand and BowRelease"
        );
        assert!(!seqs[1].looping, "BowRelease clamps — one blast per arm");

        let emitters = benilla_formats::parse_m2_particle_emitters(&bytes).expect("emitters");
        assert_eq!(emitters.len(), 7, "the muzzle bank");
        // Each emitter holds a constant rate in every sequence and keys only `enabled` (the track
        // at `def+0x1dc`, `u8`, step): 0 across Stand, 1 from the head of BowRelease for its own
        // window.
        for (i, em) in emitters.iter().enumerate() {
            assert!(
                em.timing.rate(Some(0), 0.0, 0.0) > 0.0,
                "emitter {i} holds a rate on Stand — it is the gate that is shut"
            );
            for step in 0..8 {
                let t = step as f32 * 0.04;
                assert!(
                    !em.timing.emitting(Some(0), t, 0.0),
                    "emitter {i} is gated shut on Stand (t={t})"
                );
            }
            assert!(
                em.timing.emitting(Some(1), 0.0, 0.0),
                "emitter {i} opens at the head of BowRelease — this is the blast"
            );
            // 1.0 s clears every window in the bank; the widest is the 334 ms smoke tail.
            assert!(
                !em.timing.emitting(Some(1), 1.0, 0.0),
                "emitter {i} is over well before BowRelease's 2 s band ends"
            );
        }
    }

    /// Bows and crossbows author both flex clips and a melee weapon neither, which pins the
    /// attach's `flexes` gate to data.
    #[test]
    fn only_ranged_weapon_models_author_the_flex_clips() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let ids = |path: &str, chain: &mut benilla_formats::Chain| -> Vec<u16> {
            let bytes = chain
                .read_file(path)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            let mut v: Vec<u16> = benilla_formats::parse_m2_animations(&bytes)
                .iter()
                .map(|s| s.anim_id)
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        assert_eq!(
            ids(
                "Item\\ObjectComponents\\Weapon\\Bow_1H_Standard_A_01.m2",
                &mut chain
            ),
            vec![STAND, BOW_PULL, BOW_RELEASE],
            "a bow draws AND releases"
        );
        assert_eq!(
            ids(
                "Item\\ObjectComponents\\Weapon\\Bow_2H_Crossbow_A_01.m2",
                &mut chain
            ),
            vec![STAND, BOW_PULL, BOW_RELEASE],
            "so does a crossbow — the rifle family plays its 161"
        );
        let sword = ids(
            "ITEM\\ObjectComponents\\WEAPON\\Sword_2H_AhnQiraj_D_01.m2",
            &mut chain,
        );
        assert!(
            !sword.contains(&BOW_PULL) && !sword.contains(&BOW_RELEASE),
            "a melee weapon authors no flex clip: {sword:?}"
        );
    }
}
